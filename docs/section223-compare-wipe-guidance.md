# §1.223 比較ワイプ境界の常時表示

更新: 2026-09-12

## 目的

Shift+Cでワイプ比較中は、合成境界とdrag用handleを明暗どちらの画像上でも常時確認できるように
する。利用者による初回実装の確認後、hover後だけ表示する仕様は撤回した。既存のCtrl非表示入力を
押している間だけ、drag中かどうかにかかわらず境界とhandleを隠す。

## 所有境界

比較の mode、fraction、drag owner は従来 App-global であり、detached viewer を park すると
`deactivate_compare_view` で終了する。passive detached window は frozen snapshot を描画し、
background 用の一時 context mount は入力を処理しない。今回も「同時に一つの foreground 比較」
という既存契約を維持する。

表示だけをcontext mapや別boolにするとmode / fraction / dragと寿命が分裂するため、
`CompareViewMode::Wipe`が`CompareWipeInteraction::{Ready, Dragging}`も所有する。

状態遷移は、合成境界と同じ fitted / zoomed / panned rect を使う
`handle_compare_wipe_drag` だけが行う。描画の main CPU/GPU と navigator CPU/GPU の4経路は状態を
読むだけである。hoverは可視性や状態を所有せず、明示的なpress + hit、release、invalidationだけが
drag ownerを遷移させる。

## ライフサイクル

- Shift+Cのentryは毎回`Ready`を生成する。
- Shift+C 再押下、Esc、C / Alt+C への切替、pin解除・交換、spread modeへの切替、fullscreen close、
  detached park は既存の比較終了経路を使う。
- page / source preparationの失効、sidecar restoreの入力cleanupは`Dragging`だけを`Ready`へ戻す。
- 現行UIにはWipeを開始するmenu routeはなく、`KeyAction::FsCompareWipe` が唯一の入口である。
  将来menu入口を追加する場合は同じtoggle helperへ接続する。

## 表示

準備済みpairを描画できる間は、既存の28 logical point幅grab bandを変えず、その内側に収まるhandleと、
暗色under-stroke＋明色foregroundの境界線を描く。focused viewportの既存OS Ctrl factがtrueの間だけ
`Ready / Dragging`の双方で非表示にし、解除したframeで再表示する。fraction、clip、shader uniform、
画像合成、touch操作は変更しない。

## 回帰

- pointer位置や`Ready / Dragging`にかかわらず通常は常時表示し、Ctrl中だけ非表示になる
- releaseをpressより優先し、同frame clickでdrag ownerを残さない
- Off後の再entry、park後の新entryは`Ready`で始まる
- invalidationは`Dragging`だけを終了する
- 暗線は白背景、明線は黒背景で十分なcontrastを持ち、handleはgrab band内に収まる
- fraction / grab geometry、準備worker、CPU / GPU合成、single-foreground teardownを維持する

## 初回仕様の検証記録（2026-09-13）

最終freezeで次を確認した。ログは
`target/section223-compare-wipe-guidance-20260913/` に保存した。

- `cargo test -p mimageviewer --lib wipe_ -- --nocapture`: 13 passed / 0 failed
- `compare_preparation_invalidation_preserves_guidance_but_ends_dragging`: 1 passed / 0 failed
- `RUST_TEST_THREADS=1` + `scripts/test-full.ps1 -SuppressCrashDialogs`: main 8307 passed /
  0 failed / 45 ignored。workspace、integration、doc-test、vendor egui / egui-wgpu / eframe も全て成功
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
  `python scripts/check_ui_glyphs.py`、対象pathの `git diff --check`: 成功

製品4pathのSHA-256は `src/app.rs` =
`E91B5115B1CBA60C8CCA55DD45BE536069141B4FD49D1C7D40AAB328B143D937`、
`src/ui_fullscreen.rs` =
`18AF1A44CEC4D923695C54D2D7A79FF9E446958F6B6FFFE7F3AE7F44DC097F08`、
`src/app/sidecar_restore.rs` =
`DA330888920AC8BFC5D587F0D1BCD9DE715CA15DD527049D5931EAA4A500F4FE`、
`src/app/tests.rs` =
`99B4F8A1DD245D6F1DE25879C79D74EC30EEA4BD1C1AA45E1DC0F2B768D0CB4F`。

利用者が§1.228確認用の同じ `target/dev-runtime` core / remoteを終了した後、exact residentが
無いことを確認して `build-dev.ps1 -PreserveRuntime` を実行し、exit 0で完了した。agentはアプリを
起動・停止していない。成果物は次のとおり。

- core: SHA-256
  `3E3A7260D6D17A2BC8B604F95A3BBD1F176F8F2EBBC2F33B6198649ECDF0C8C8`、
  UTC `2026-09-12T15:23:18.8241760Z`
- remote: SHA-256
  `A89F6516CC2EB65B39E5BA17E92E867CAB7B83BD29D954E03A3197901A50B53C`、
  UTC `2026-09-12T14:33:12.2129161Z`（内容不変のため既存成果物を再利用）

ビルド後も上記製品4pathのSHA-256は一致した。利用者による§1.223の画面確認は未実施である。

## 常時表示仕様の検証記録（2026-09-13）

利用者確認を受けて初回 Guidance 状態を撤去し、通常は境界とhandleを常時表示、focused viewportで
Ctrlが押されている間だけ非表示とした最終仕様を確認した。ログは
`target/section212-223-followup-20260913/` に保存した。

- `cargo test -p mimageviewer --lib wipe_ -- --nocapture`: 12 passed / 0 failed
- 比較 preparation invalidation: 1 passed / 0 failed
- `cargo check -p mimageviewer --bin mimageviewer-core`、viewer-context audit、
  `cargo fmt --all -- --check`、UI glyph、対象pathの`git diff --check`: 成功
- 直前§1.218のfull gate（main 8323 / 0、45 ignored、workspace / integration / doc / vendor全成功）を
  共通の未変更範囲に再利用し、今回変更した入力reducerと4描画経路は上記focused回帰で確認した
- exact residentが無いことを確認して`build-dev.ps1 -PreserveRuntime`を実行し、exit 0。core SHA-256は
  `A8E9C67FE9F0AD4DD463A33A402D97C158413B5845D718D4B039C5E48D038E43`、remote serviceは
  `A03CE402A7137613E063867C9F8338BEDC5C338FE8897325007BB35110935C64`。agentはアプリを起動・停止していない

利用者によるこの常時表示仕様の画面確認は未実施である。
