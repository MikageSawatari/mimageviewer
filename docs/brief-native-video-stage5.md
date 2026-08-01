# ブリーフ: native video Stage 5 (VST owner handoff と focus / cursor 境界)

実装 = Codex Sol / レビュー = ClaudeCode。v2.9.1 出荷前。

正本は [native-video-window-thread-plan.md](native-video-window-thread-plan.md) の
**Stage 5**、および [next-release-backlog.md](next-release-backlog.md) の **§1.28**。
着手前に plan の §4.2 (thread boundary)、Stage 4 実装記録、Stage 5、§10 の 5 / 6、§11 を読むこと。

## 1. 範囲

Stage 5 の定義どおり。要点:

- VST bridge の `set_chain_owner` に request id と **owner-applied ack** を追加する
  (現状は fire-and-forget、plan §11 参照)。
- owner 切替中は pump が所有する hidden owner anchor を保持し、**ack 後に**旧 presenter HWND を
  destroy する。**UI / pump は ack を同期 wait しない。**
- bridge GUI task timeout 時は既存の bridge isolation / termination policy へ収束させ、
  main / pump を待たせない。
- `WM_MOUSEACTIVATE`、foreground claim、IME preedit / commit、HUD focus return の sequence test。
- **backlog §1.28 のカーソル所有の修正** (下記 §2)。

変えないもの (plan より): plugin の DSP / audio 経路、editor の見た目、fullscreen 時のみ
presenter を editor owner にする policy。Stage 4 時点の挙動を劣化させず ack 付きへ強化する。

## 2. カーソル所有 (backlog §1.28)

2026-08-01 の実機で再現した既知バグ。フルスクリーン再生中に VST エディタを開くと、
カーソルが非表示のまま戻らない。**mIV の動画ウィンドウ上でだけ**継続し、別モニタでは正常、
戻るとまた消える。**クリックしても復帰しない。**

壊れている前提: [native_window_host.rs](../src/video/native_window_host.rs) の `observe()` が作る
`cursor_within_client` は `GetCursorPos` → `ScreenToClient` → `GetClientRect` の**純粋な幾何判定**
なのに、[render_core.rs](../src/video/native_presenter/render_core.rs) の
`cursor_within_focus_window()` がそれを**所有権の判定として**使い、true の間だけ `SetCursor` intent を
出している。presenter の client 矩形の内側に別 top-level 窓が乗ると 2 つの答えがずれる。

直し方の指定:

- `cursor_within_client` を、幾何ではなく**「presenter または HUD が実際にそのカーソルの入力先か」**
  を答える述語に置き換える。判定不能時のフォールバックが現在 `_ => true` (= 隠す側に倒れる)
  になっている点も直す。
- auto-hide 状態は現在 **producer が 3 つ (presenter frame intent / HUD wndproc /
  `push_native_event`) あって所有者がいない**。reducer が単一 owner となり、placement / VST owner
  切替の遷移で明示的にリセットされる形へ集約する。
- **VST 固有の分岐を足さない。** presenter の上に別の窓が乗る全ケースで成立する問題であり、
  VST は最も踏みやすい形にすぎない。

## 3. 制約

**3-1. 症状パッチにしない。** 次はいずれも採用しない:

- `WM_SETCURSOR` で復帰させる — **2026-06-06 に意図的に外した経路**であり、戻すと当時の
  「静止カーソルの下で HUD が広がると zero-delta move でカーソルが復活する」不具合が再発する
  ([hud_window.rs](../src/video/native_window_host/hud_window.rs) の該当コメント参照)。
- タイマー / グレースでの強制復帰。
- ack を UI / pump が同期 wait する (plan §11 が明示的に不採用としている)。

**3-2. C++ bridge を変更したら必ず再ビルドすること。**
`crates/vst3-host/src/protocol.h` や C++ 側を変更した場合、

```
cmake --build crates/vst3-host/build --config Release
```

で `vendor/vst3-host/mimageviewer-vst3-host.exe` を作り直す。**古い exe を流用しないこと。**
IPC プロトコルがずれた exe を使うと bridge が起動直後にクラッシュし、VST 有効時に動画再生が
「激重」になる (CLAUDE.md「VST3 host bridge 管理」に 2026-05-14 の実害として記録あり)。
SDK (`vendor/vst3sdk/`) と build dir は配置済みなので追加取得は不要。

**3-3. pump → render の逆向き wait を作らない** (plan §10 の 4)。watchdog / health も同様。

**3-4. detached 凍結ルール。** detached 述語 / viewport 経路に触れる場合は CLAUDE.md
「Detached viewer リワーク中のルール」に従い、着手前に
[detached-rework-plan.md](detached-rework-plan.md) §2 を読み、触れた範囲と判断理由を §11 へ記録する。

**3-5. Stage 6 へ踏み込まない。** hidden / source / EOF / placement failure の全 sequence
hardening は次の作業。今回は Stage 5 の範囲で止める。

## 4. テストで縛ること

plan の Stage 5 単独 gate + §7.1 の該当項目:

- fake bridge で ack / stall / restart を再現し、**ack が遅延・欠落しても UI / pump が待たない**
- owner 切替の順序 (old owner → request → ack → old destroy) と timeout / restart (§7.1 の 8)
- `WM_MOUSEACTIVATE` / foreground claim / IME preedit・commit / HUD focus return の sequence
- カーソル: presenter の矩形内に別窓が乗っている状態で auto-hide が**解除される**こと
- カーソル: placement 切替と VST owner 切替の遷移で auto-hide 状態がリセットされること
- カーソル: 判定不能時に「隠す」側へ倒れないこと
- 既存 ignored test `production_parent_destroy_remains_bounded_during_render_stall` が通ること

## 5. 検証

```
cargo fmt --all
cargo test -p mimageviewer --lib
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --lib -- --ignored --nocapture production_parent_destroy_remains_bounded_during_render_stall
python scripts/check_ui_glyphs.py
```

C++ を変更した場合は上記 3-2 の cmake も実行し、その旨を報告に明記すること。

実機確認はレビュー側が依頼する (plan §7.2 のシナリオ 4 / 5、およびカーソルの再現手順)。

## 6. スコープ外

- Stage 6 (hidden / source / EOF / placement failure の lifecycle hardening)
- Stage 7 の残り (legacy path 再流入の source / type gate、最終実機 gate)。
  §7.3 の health detection は実装済み ([src/video/native_window_health.rs](../src/video/native_window_health.rs))
