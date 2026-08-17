# §1.31-B (前半) — acquire の待ちに上限を持たせる

対象: [next-release-backlog.md](../next-release-backlog.md) §1.31 の後半のうち **acquire 側だけ**。
前提 = §1.31-A (`4e6e5efe`) が master へ merge 済み。

**Present 側には手を出さない (§8)。**

## 0. なぜ acquire だけを先にやるか

§1.31-A は「同期メッセージ**自身が** GPU 待ちを開始する」経路を消した。だが UI スレッドが
外側の paint で GPU を待っている間も message pump は止まるので、その最中に他スレッドが
`SendMessage` すれば sender は待たされる。これを閉じるのが B。

B は 2 つに分かれ、**難易度が桁違いに違う**:

- **acquire (本ブリーフ)**: wgpu の vendoring **不要**。public API だけで組める (§2 で実証済み)。
- **Present**: `SurfaceTexture::present()` の戻り値が `()` で、HAL の FIFO Present は interval 1
  かつ `DXGI_PRESENT_DO_NOT_WAIT` 無し。厳密に bounded にするには wgpu / core / hal の patch か
  render thread 分離が要る。**A 後の実測を見てから判断する**ので、今回は対象外。

## 1. 現状 — 既定で「待って、結果を捨てる」

- `Dx12UseFrameLatencyWaitableObject` の既定は **`Wait`**
  (`wgpu-types-27.0.1/src/instance.rs`)。swapchain は
  `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` 付きで作られ、
  `acquire_texture` が waitable object を待つ。
- wgpu-core は `FRAME_TIMEOUT_MS = 1000` を渡す (`wgpu-core-27.0.3/src/present.rs`)。
- wgpu-hal DX12 は `unsafe { sc.wait(timeout) }?;` と書いており、**戻り値の bool を捨てる**
  (`wgpu-hal-27.0.4/src/dx12/mod.rs`)。timeout してもそのまま `GetCurrentBackBufferIndex` へ進む。

つまり **UI スレッドは 1 フレームあたり最大 1 秒ブロックし得て、timeout しても何も起きない**。
§1.30 が「デッドロックか飢餓か区別できない」と書いていたのはこの構造。

## 2. 設計 — `DontWait` + 自前の非ブロッキング readiness gate

`DontWait` の doc に明記がある:

> Create the swapchain with the `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` flag and
> obtain a waitable handle, but do not wait for it before acquiring the next swapchain image.
> **This is useful if the application wants to wait for the waitable object itself.**

到達経路はすべて public (確認済み):

- `wgpu::Surface::as_hal::<A: hal::Api>()` (`wgpu-27.0.1/src/api/surface.rs`)
- `wgpu_hal::dx12::Surface::waitable_handle() -> Option<HANDLE>` (`wgpu-hal-27.0.4/src/dx12/mod.rs`)

### 手順

1. instance descriptor で `Dx12UseFrameLatencyWaitableObject::DontWait` を設定する。
   mIV は既に `MIV_WGPU_FRAME_LATENCY` で `desired_maximum_frame_latency` を触っている
   ([lib.rs](../../src/lib.rs)) ので、同じ場所が素直。
2. §1.31-A の outer paint phase で、その viewport の paint に入る**前**に
   `WaitForSingleObject(handle, 0)` を呼ぶ (timeout 0 = 非ブロッキング)。
3. `WAIT_OBJECT_0` なら通常どおり paint する。`acquire_texture` は `DontWait` なので待たない。
4. `WAIT_TIMEOUT` なら **`DeferredNotReady`** としてその frame を捨てる。
   - damage は**保持する** (次に readiness が来たら描く)
   - readiness の wake を arm する (§3)
   - **warning にしない**。通常動作である

### 2.1 これは時間窓ではない (憲法 5)

判定は `WaitForSingleObject(handle, 0)` という **OS が持つ事実**であり、
debounce / grace / settle ms ではない。憲法 5 に抵触しない。
**「N ms 待ってダメなら諦める」形にしないこと。**

### 2.2 DX12 以外へのフォールバック

`as_hal::<Dx12>()` は backend が DX12 でなければ `None` を返す。その場合は
**現行挙動 (gate 無し) にフォールバックする**。新しい失敗経路を作らない。
handle が取れない場合も同じ。フォールバックしたことは起動時に 1 回ログへ残す
(毎フレーム出さない)。

## 3. readiness の wake — ここが設計の要 ⚠️

frame を捨てたあと、**誰が次の paint を起こすか**を決めないと止まる。

- **drop 後に即 repost しないこと**。spin する (Codex 指摘)。
- 時間で起こさないこと (憲法 5)。
- waitable handle が signal されたことを事実として拾って起こす。

実装候補 (いずれかを選び、理由を報告に書くこと):

1. **専用の待機スレッド**: handle を `WaitForSingleObject(handle, INFINITE)` で待ち、
   signal されたら event loop へ user event を post する。UI スレッドは一切待たない。
   viewport ごとに handle が違うので、生成 / 破棄のライフサイクルを surface と揃える必要がある。
2. **`MsgWaitForMultipleObjects`**: event loop の待機に handle を混ぜる。winit の
   `ControlFlow` と噛み合うかの確認が要る (winit を触らずに実現できるかを先に確かめること)。

**1 を第一候補とする**。winit に触らずに済み、§1.31-A の outer phase と自然に繋がる。
ただし surface の再生成 (resize / device loss) で handle が変わるので、
**surface generation と対応付けて古い wake を捨てる**こと (§1.31-A の dirty が既に
surface generation を持っている)。

## 4. §1.86 の契約を必ず通す ⚠️

`DeferredNotReady` でも `begin_delivery` / `finish_delivery` を必ず通す。
これは §1.86 でこの契約を作った理由そのもの。

- `DeferredNotReady` を `Skipped` と**別の variant** にする。通常動作なので warning 対象ではない。
- `textures_delta` の `set` / `free` はちょうど一度ずつ配送される。

## 5. screenshot

現在は painter 呼び出しの**前**に `actions_requested` から screenshot command を除去している
([wgpu_integration.rs](../../vendor/eframe/src/native/wgpu_integration.rs))。
`DeferredNotReady` で frame を捨てると **screenshot 要求が消える**。

`Submitted` になるまで viewport が要求を保持し、一度だけ成功するようにすること。

## 6. 触ってよいファイル

- `src/lib.rs` (instance descriptor の `Dx12UseFrameLatencyWaitableObject`)
- `vendor/eframe/src/native/run.rs` (outer phase の readiness gate と wake)
- `vendor/eframe/src/native/wgpu_integration.rs` (screenshot 保持、outcome 配線)
- `vendor/egui-wgpu/src/winit.rs` (`DeferredNotReady` outcome の追加のみ。
  §1.86 の transaction 構造を壊さない)
- `scripts/test-full.ps1` / `docs/`

winit / wgpu / wgpu-hal を vendor しない。

## 7. 検証

### 7.1 Windows process test (§1.31-A の §6.1 と対になる)

**性質**: presentation が不能な間も、外側の service latency が有界である。

- deterministic な readiness handle を unsignaled にする。
- outer paint を要求する。
- `DeferredNotReady` が**一度だけ**出ることを確認する (spin していない)。
- gate 閉鎖中、別スレッドから ping message を連続 `SendMessageTimeoutW` し、
  **全部が hard limit 内で戻る**こと。
- gate 閉鎖中に paint attempt / wake が増えないこと。
- readiness を一度 signal する。
- **generation 一致の wake が一度だけ**入り、最終的に `Submitted` になること。
- dirty が消えること。
- screenshot 要求が一度だけ成功すること。
- `textures_delta` の `set` / `free` が一度ずつ配送されること。

§1.31-A の Windows test と同じ形 (子プロセス + `SendMessageTimeoutW` +
`SMTO_BLOCK | SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT` + 親側 watchdog) を踏襲する。

### 7.2 unit test

- `DeferredNotReady` で damage が保持される
- stale generation の wake が捨てられる
- 同じ readiness signal で wake が二重に入らない
- `DeferredNotReady` でも delta 配送が一度ずつ行われる

### 7.3 実機 (利用者に依頼する分)

- idle health 全シナリオ (`scripts/check-idle-health.ps1`)。
  **特に「readiness 待ちで spin していないか」**。
- perf smoke。`DeferredNotReady` の発生率と、入力 → 提示完了 latency。
- 通常操作、フルスクリーン、detached、動画再生、resize ドラッグ。

## 8. やらないこと

- **Present 側に手を出さない** (§0)。`present()` は現行のまま。
- 時間窓で readiness を代替しない (§2.1)。
- drop 後の即 repost をしない (§3)。
- DX12 以外で新しい失敗経路を作らない (§2.2)。
- §1.86 の transaction 構造を壊さない (§4)。

## 9. 凍結ルール

[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に読むこと。
§11 への記録が要る。憲法 5 (時間窓で競合を吸収しない) が特に効くので §2.1 を守ること。

**着手前に ClaudeCode / Codex Sol の設計レビューを通すこと。** §1.31-A では私の前提が
3 点誤っていて、レビューで訂正された。B も同じ手順を踏む。
