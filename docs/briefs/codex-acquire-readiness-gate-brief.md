# §1.31-B0 — どの待ちが実際に UI を止めているのかを先に測る

対象: [next-release-backlog.md](../next-release-backlog.md) §1.31 の後半 (B) の**着手判断のため**の計測。
前提 = §1.31-A (`4e6e5efe`) が master へ merge 済み。

> **2026-08-17 改訂**。本書は当初「acquire に readiness gate を入れる」実装ブリーフだった。
> Codex Sol の設計レビューで **P0 が 6 件**出て差し戻し、計測ブリーフに書き換えた。
> 旧案の設計と、それが差し戻された理由は §3 に残してある (捨てない。測定で正当化されたら使う)。

## 0. なぜ実装ではなく計測から始めるのか

当初の計画は「acquire は public API だけで閉じられると分かったので、そこから実装する」だった。
**API に手が届くことは、そこを先に直す根拠にならない。** 差し戻しの決め手は次の 2 つ。

### 0.1 acquire を閉じても message service latency は上限化されない ⚠️

非ゼロ `Resized` は message-dispatch 中に `painter.on_window_resized`
([wgpu_integration.rs](../../vendor/eframe/src/native/wgpu_integration.rs)) →
`configure_surface` ([winit.rs](../../vendor/egui-wgpu/src/winit.rs)) を通る。
その先の wgpu-hal DX12 `configure` は `ResizeBuffers` の前に必ずこれを実行する:

```rust
// wgpu-hal-27.0.4/src/dx12/mod.rs
unsafe { device.wait_for_present_queue_idle() }?;
```

```rust
// wgpu-hal-27.0.4/src/dx12/device.rs
unsafe { Threading::WaitForSingleObject(event.0, Threading::INFINITE) };
```

**`INFINITE` である。** unconfigure / drop 側にも同じ待ちがある。
つまり §1.31-A が残した inline resize 例外は、acquire より**手前**で無期限 GPU wait に入れる。
acquire 直前に gate を置いても、この経路は閉じない。

「acquire は最大 1 秒」も厳密には強すぎた。1000ms は frame-latency wait に渡す timeout であって、
acquire 全体の hard upper bound ではない。

### 0.2 §1.31-A の後、acquire が実害を起こしている証拠がまだ無い

現状で言えるのはここまで:

- コード上 1000ms の待ちが存在する
- §1.30 の実スタックが**本当にこの wait だったかは未確定** (backlog §1.30 の「次回に取るべき証拠」は未取得のまま)
- §1.31-A の後に acquire が UI を止めた観測はゼロ
- `configure` の `INFINITE` と Present も同格の候補

**競合候補が 3 つあって、どれが効いているか分からない状態で 1 つを選んで直さない。**
このプロジェクトは原因不明の不具合を推測で直して外した実績があり、直近では §4.2 で
「Z が効かない」と読んで入力経路を疑ったが、ログを見たら Z は効いていて別の場所が壊れていた。

## 1. 測ること

**現行の `Wait` 設定のまま**測る。`DontWait` に変えて測るのは正しい待機契約を壊すので、
比較実験に使わない。

### 1.1 区間 (それぞれ begin / end を別イベントで)

- `surface.configure` (← 0.1 の `INFINITE` はここ)
- `get_current_texture`
- `queue.submit`
- `SurfaceTexture::present`

### 1.2 各イベントに付ける文脈

- outer / inline の別と、inline なら reason (`Bootstrap` / `AccessKit` / `InteractiveResize`)
- WindowId / ViewportId / surface generation / size / visible / minimized

### 1.3 別スレッドからの service latency

別スレッドから定期的に `SendMessageTimeoutW` を打ち、**戻るまでの時間**を記録する。
これが §1.31 が本来下げたい量そのもの。上の 4 区間のどれと相関するかを見る。

### 1.4 出力の形

**毎フレーム全件を出さない。** ヒストグラムと、`>8 / 16 / 33 / 100 / 500 / 900ms` の
slow event だけを出す。`scripts/analyze_perf.py` に集計サブコマンドを足す。

### 1.5 遅いイベントのスタック

区間ログだけでは「どの Win32 wait か」までは割れない。長いイベントを捕まえたら
WPR / WPA の CPU sampling + Wait Analysis / DXGI で、
`wgpu-hal::SwapChain::wait` / present-queue idle fence / Present のどれかを確定する。

### 1.6 シナリオ

通常スクロール / 連続ページ送り / フルスクリーン / immediate・deferred detached /
複数窓同時 repaint / **resize ドラッグ** / tray hide・restore / 動画再生・VST /
モニター跨ぎ / GPU 高負荷時。同じ release profile で `MIV_WGPU_FRAME_LATENCY=1` と `2` も比較する。

## 2. 判断基準 (測ったあと何を決めるのか)

- **`configure` だけが長い** → acquire gate は先送り。inline resize 例外の扱い (§1.31-A §3) を
  再設計する方が効く。
- **Present だけが長い** → acquire gate は先送り。wgpu patch か render thread 分離の検討へ。
- **`get_current_texture` が長い** → §3 の acquire gate 設計を、下記の訂正を織り込んで書き直す。
- **どれも短い** → §1.31-B は着手しない。§1.31-A で得た効果を記録して閉じる。

## 3. acquire gate の設計 — 差し戻された旧案と、その訂正 (測定で正当化されたら使う)

到達性の結論は**正しかった** ので残す:

- `Dx12UseFrameLatencyWaitableObject` の既定は `Wait`。`DontWait` は doc に
  「application 自身が待つ用途」と明記。
- wgpu 27 に `hal` feature は無いが、native build では `cfg(wgpu_core)` により
  `wgpu::hal` が `pub extern crate wgpu_hal as hal` で re-export される。
  `Surface::as_hal` も同 cfg 下で public (ただし `unsafe`)。
- mIV の依存構成では `wgpu/dx12` と `wgpu-hal/dx12` が有効で、`wgpu-hal 27.0.4` 一つに解決される。
- → **acquire handle へ到達するための wgpu vendoring は不要**。

以下は旧案の誤りで、実装するなら**先に潰すこと**。

### 3.1 borrowed handle を worker で `INFINITE` 待ちするのは unsafe ⚠️

実際の署名は `pub unsafe fn waitable_handle(&self) -> Option<HANDLE>` で、doc に
**「Handle is only valid while the swap chain is alive」**と書いてある。
raw handle をコピーして worker に渡し、その間に resize / configure / drop が HAL 側で
`CloseHandle` すると、pending wait 中に handle が閉じられる (Windows は動作未定義)。

安全な形:

- `as_hal` guard が有効な **UI スレッド上で** original handle を読む
- `DuplicateHandle` で **worker 所有の handle** を作る
- guard はその場で drop し、HAL surface / 参照を別スレッドへ渡さない
- worker は duplicated handle と cancel / rebind event を `WaitForMultipleObjects` する
- surface の configure / drop は cancel / rebind を signal する
- original handle はアプリ側で閉じない。duplicated handle だけ worker が閉じる

### 3.2 UI と worker を同時 waiter にしない ⚠️

worker の wait 自体が「そのフレームの wait」を消費する。worker が signal を取った後に
UI が同じ object を `WaitForSingleObject(handle, 0)` すると、**UI が `WAIT_TIMEOUT` へ落ちる競合**になる。

単一の typed state を置き、**同時 waiter を作らない**ことを不変条件にする:

```text
Unsupported
Idle
Armed      { handle_generation, arm_seq }
ReadyPermit{ handle_generation, arm_seq }
```

worker が `Armed → ReadyPermit` にして user event を post し、UI は `ReadyPermit` を消費して acquire する。
surface miss ごとに thread を作らない (resize で handle は頻繁に変わる)。
surface / viewport 単位の再利用 worker か中央 wait broker にする。

### 3.3 §1.31-A の `surface_generation` は handle generation の正本ではない ⚠️

旧案の「dirty が既に surface generation を持っているので対応付ける」は**不十分**。

- 現在の generation は非ゼロ resize と window initialize では増えるが、
  `SurfaceErrorAction::RecreateSurface` の再 configure では**増えない**。
- `set_window(None)` ([winit.rs](../../vendor/egui-wgpu/src/winit.rs)) は対象 viewport だけでなく
  **全 surface を clear する**。ある viewport の recreate で sibling の handle まで消えるのに、
  sibling の eframe generation は増えない。

分離する:

- handle generation の正本は `egui-wgpu::Painter` の `SurfaceState` 側。
  configure 成功 / clear / gc / drop ごとに local generation を更新
- wake は `(ViewportId, handle_generation, arm_seq)` を持つ
- eframe scheduler generation は window / size の stale claim 判定に使う
- **同じ `u64` を流用しない**

### 3.4 gate は outer だけでは足りない — immediate viewport ⚠️

immediate child は親の `App::update` 内で `render_immediate_viewport`
([wgpu_integration.rs](../../vendor/eframe/src/native/wgpu_integration.rs)) へ同期再帰し、
そこで**独自 surface の `get_current_texture`** を呼ぶ。§1.31-A の scheduler はこの再帰を見ない。

→ **gate の実体は per-surface の acquire seam、つまり `egui-wgpu::Painter` 内**に要る。
run.rs の outer claim 前だけに置いても child の acquire が残る。

子が not-ready になった時点で、子の egui pass と texture delta 生成は既に終わっており、
親も進行中なので**親フレーム全体をロールバックできない**。構造的な扱い:

- ready な親 surface は通常どおり submit する
- child の `DeferredNotReady(token)` を frame aggregate に記録する
- token の readiness wake は child ではなく **immediate child を生成する親 window** を dirty にする
- 親 scheduler claim の reason (特に `InteractiveResize`) を保持する
- 複数 child の blocker を集合として扱う
- signal 一回につき最大一回だけ親を再要求し、**複数 surface の交互 wake が busy-loop にならない**
  ことをテストする

### 3.5 inline resize の liveness が未設計 ⚠️

inline resize claim を `DeferredNotReady` で単に finish すると内容が固まる
(modal size/move 中は outer `about_to_wait` が回らない)。

- deferred demand の所有者が元の `InteractiveResize` provenance を保持する
- **readiness user event が modal loop 中にも winit から配送されるか**を Windows process test で確認する
- 配送されるなら、保存済み claim の causal continuation として inline 再開する
- **配送されないなら `EventLoopProxy` worker 案では resize liveness を満たさない**ので設計を止める

「readiness wake も message-dispatch から描く」は §1.31-A の例外拡張になる。
時間や geometry ではなく保存済み provenance による構造的拡張だが、
ブリーフと `detached-rework-plan` §11 に明記して**事前合意が要る**。

### 3.6 outcome の境界は 2 段 ⚠️

`PaintOutcome` は現在 private で、`paint_and_update_textures` は `f32` しか返さない。
**variant を 1 つ足すだけでは eframe へ伝播しない。**

- egui-wgpu 側: per-surface の delta transaction outcome に `DeferredNotReady(token)` を足す。
  gate は `begin_delivery` の後、encoder / `update_buffers` / acquire の**前**。
  全 outcome が `finish_delivery` を通る (§1.86 の契約)
- eframe 側: root / descendant を集約した `FrameRenderOutcome`。scheduler claim を clean にするか
  readiness pending owner へ移管するかを決め、immediate child token を親へ routing する

旧案は「outer preflight」と「生成済み frame の non-submit outcome」を混同していた。
**outer claim の前に UI pass 自体を止めるなら、その時点では texture delta が存在しないので
§1.86 の transaction は関係ない。**

### 3.7 落とすと壊れるもの

- **初回表示**: `post_rendering` ([epi_integration.rs](../../vendor/eframe/src/native/epi_integration.rs))
  が最初の paint 後に visible にする。現在 surface submit の成否を見ていないので、
  Deferred でも呼ぶと**未描画の白 / 黒 window を表示する**。
  root surface が `Submitted` になった後だけ first-frame visible を解除する。
- **screenshot**: 同じ surface が `Submitted` になるまで保持する。`DeferredNotReady` だけでなく
  `SurfaceRecreated` / `Skipped` も同じ原則に揃えないと「Submitted まで保持」という契約にならない。
- **hidden / tray**: `Visible(true)` 等を処理するため UI pass 自体は走らせる必要がある。
  **surface gate を `run_ui_and_paint` の前に置いてはいけない。**
- **bootstrap**: deferred demand を hidden 100ms throttle へ落とさず、元の Bootstrap reason を保持する。
- **close / gc / recreate**: worker を cancel し、queued wake を generation で破棄する。
  read-only の open / close で sibling の wake を誤って invalidate しない。

### 3.8 `MsgWaitForMultipleObjects` 案は現行 winit では不可

`MsgWaitForMultipleObjects` は window を所有し message queue を pump する thread の outer loop で
呼ぶ必要がある。現在その loop を所有しているのは winit の `run_app` で、`ControlFlow` に
任意 HANDLE を登録する API は無い。winit を変更するか `pump_app_events` で eframe が
Windows message loop 全体を所有し直すかで、後者は event-loop architecture の置換に等しい。
**第一候補は cancel 可能な worker + `EventLoopProxy` のままでよい。**

### 3.9 Windows API 依存の追加が要る

worker が Win32 wait / `DuplicateHandle` / event を使うため:

- `vendor/egui-wgpu` には現在 Windows 依存が**無い**
- `vendor/eframe` の `windows-sys` に `Win32_System_Threading` feature が**無い**

両方の `Cargo.toml` が触る対象に入る。

## 4. スコープの正直な表現

本作業 (仮に acquire gate まで進んだとして) で言えるのは

> eframe の DX12 surface acquire にある frame-latency wait を外出しした

まで。**「§1.31-B 完了」「message service latency を上限化した」とは言えない。**
`configure` の `INFINITE` と Present が残るため。

さらに、native presenter 内の `NativeEguiOverlay` は**別の `wgpu::Instance`**
([render_core.rs](../../src/video/native_presenter/render_core.rs)) で既定 `Wait` のまま動くので、
`src/lib.rs` の `DontWait` 設定は届かない。UI の message pump ではなく native render thread を
止める経路なので §1.31 の主目的とは別だが、**「mIV の acquire 全体を直した」とも言えない**。

## 5. 触ってよいファイル (B0 = 計測のみ)

- `vendor/egui-wgpu/src/winit.rs` (区間の perf event。**挙動は変えない**)
- `vendor/eframe/src/native/run.rs` / `wgpu_integration.rs` (文脈の付与)
- `src/` の perf イベント定義と `scripts/analyze_perf.py`
- `docs/`

**この段階で `Dx12UseFrameLatencyWaitableObject` を変えない。gate を入れない。**

## 6. 凍結ルール

[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。
B0 は計測のみで production の制御フローを変えないが、着手前に §2 を読むこと。
gate の実装に進む段階では §11 記録と事前合意が要る (特に §3.5 の inline 再開)。
