# §1.223 比較ワイプ境界の初回案内

更新: 2026-09-12

## 目的

Shift+C でワイプ比較へ入った直後は、合成境界とドラッグ用ハンドルを明暗どちらの画像上でも
確認できるようにする。利用者が境界へ初めて意図的に pointer を移動するかドラッグした後は、
従来の hover / drag 中だけ白線を表示する挙動へ戻す。時間待ちや frame 数では案内を消さない。

## 所有境界

比較の mode、fraction、drag owner は従来 App-global であり、detached viewer を park すると
`deactivate_compare_view` で終了する。passive detached window は frozen snapshot を描画し、
background 用の一時 context mount は入力を処理しない。今回も「同時に一つの foreground 比較」
という既存契約を維持する。

案内だけを context map や別 bool にすると mode / fraction / drag と寿命が分裂するため、
`CompareViewMode::Wipe` が `CompareWipeInteraction` も所有する。

- `Guidance(Unclassified)`: entry 後、比較をまだ描画可能な frame で初期 pointer を分類していない
- `Guidance(AwaitingExit)`: 初回描画時に pointer が偶然境界上にあり、いったん離れるのを待つ
- `Guidance(Armed)`: pointer が境界外にあり、次の境界への進入を意図的 hover として受理できる
- `Ready`: 従来の hover-only 表示
- `Dragging`: 従来の drag owner

状態遷移は、合成境界と同じ fitted / zoomed / panned rect を使う
`handle_compare_wipe_drag` だけが行う。描画の main CPU/GPU と navigator CPU/GPU の4経路は状態を
読むだけである。準備済み pair が現在ページと一致しない間、hover は Guidance を分類・消費しない。
明示的な press + hit は従来の drag admission を維持する。

## ライフサイクル

- Shift+C の entry は毎回 `Guidance(Unclassified)` を生成する。
- Shift+C 再押下、Esc、C / Alt+C への切替、pin解除・交換、spread modeへの切替、fullscreen close、
  detached park は既存の比較終了経路を使う。
- page / source preparation の失効、sidecar restore の入力 cleanup は `Dragging` だけを `Ready` へ
  戻す。未消費の Guidance は保持する。
- 現行UIにはWipeを開始するmenu routeはなく、`KeyAction::FsCompareWipe` が唯一の入口である。
  将来menu入口を追加する場合は同じtoggle helperへ接続する。

## 表示

Guidance は既存の28 logical point幅grab bandを変えず、その内側に収まるhandleと、暗色under-stroke
＋明色foregroundの境界線を描く。Ready / Dragging は既存の白alpha線をそのまま使う。Ctrlによる
Dragging中の線抑止、fraction、clip、shader uniform、画像合成、touch操作は変更しない。

## 回帰

- cursor がentry時から境界上でも案内を維持し、leave→re-enterでだけReadyになる
- cursor が境界外なら最初の描画から案内が見え、最初のenterでReadyになる
- 準備中hoverは案内を消費せず、press+hit / drag / releaseは既存挙動を保つ
- Off後の再entry、park後の新entryでfresh guidanceになる
- invalidationはGuidanceを保持し、Draggingだけを終了する
- Readyのhover-onlyとDraggingのCtrl suppression、fraction / grab geometryを維持する
- guidanceの暗線は白背景、明線は黒背景で十分なcontrastを持ち、handleはgrab band内に収まる

## 検証記録（2026-09-13）

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
