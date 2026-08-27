# 実装ブリーフ: 動画側の投影方式を一覧から選ぶ (静止画と UI を揃える)

対象 worktree: `C:\home\mimageviewer-pano` (branch `panorama-projection`)。
**この worktree で他の codex を並行させないこと。**

## 0. 何を直すか

**利用者報告 (2026-08-27 実機)**: 静止画の 360 では投影方式をプルダウン (一覧) から
選ぶのに、動画では押すたびに順送りするボタンになっている。**UI を静止画へ揃えたい。**

静止画側は [ui_fullscreen.rs](../../src/ui_fullscreen.rs) の `draw_fs_hover_bar` 内で、
投影ボタンを押すと一覧が開き、アイコン + 方式名 + 1 行説明が並び、現在の方式が
ハイライトされる。**同じ体験を動画の上バーにも作る。**

順送りをボタンから外す理由も静止画と同じ: 4 方式あって「今どれで、次に何になるか」が
押す前に分からない。**キー (`FsPanoramaProjection` = 既定 Shift+V) の順送りは残す。**

## 1. 現状

動画の上バーは [overlay_draw.rs](../../src/video/native_presenter/overlay_draw.rs) の
`draw_native_top_bar` (4100 行付近)。投影ボタンは
`NativeOverlayCommand::CyclePanoramaProjection` を投げるだけになっている。

コマンド経路 (5 箇所):
`overlay_draw` → `NativeOverlayCommand` ([render_core.rs:2441](../../src/video/native_presenter/render_core.rs))
→ [video/mod.rs:5505](../../src/video/mod.rs) → `NativeVideoOutputEvent` ([video/mod.rs:658](../../src/video/mod.rs))
→ [app/native_video.rs:4202](../../src/app/native_video.rs) → `App::cycle_panorama_projection`

## 2. やること

1. **`NativeOverlayCommand::SetPanoramaProjection(PanoProjection)` を足す**。
   上の 5 箇所を通して `App::set_panorama_projection(mode)` へつなぐ。
   **`App::set_panorama_projection` は既にある** (静止画と共有の適用経路)。新設しない。
   `CyclePanoramaProjection` は**キー用に残す** (消さない)。
2. **一覧の開閉状態を `NativeRenderCore` に持たせる**。既存の `video_speed_popup_open` /
   `tag_picker_open` と同じ持ち方にする。
3. **上バーの投影ボタンを「押すと一覧を開く」に変える**。ON 中は押下状態を強調する。
4. **一覧を描く**。静止画の見た目に合わせる:
   - 1 行に アイコン (`draw_panorama_projection_icon`) + `label()` + `short_description()`
   - 現在の方式をハイライト
   - 一覧の外をクリックで閉じる
   - **⚠ 画面内へクランプする。** 投影ボタンは右端寄りなので、左寄せのままだと
     画面外へ出る (静止画側で実際に切れた。`clamp_bar_popup_rect` と同じ考え方)。
   - **⚠ 幅は実測する。** 方式名と説明の幅は UI 表示倍率とフォントで変わる。固定値だと
     文字が枠から出る (静止画側は `layout_no_wrap` で測っている)。
5. **入力捕捉 RECT へ登録する**。[render_core.rs:8385](../../src/video/native_presenter/render_core.rs)
   付近の doc comment に「どの UI 領域が入力を捕捉するか」の一覧がある。
   **一覧を出している間はその矩形を含めること。** 忘れると一覧のクリックが背面の
   動画キャンバスへ抜けて、見回しドラッグが始まる。
6. **360 を抜けたら一覧を閉じる**。持ち主のボタンが消えるため。
   音声モードへ移ったときも閉じる。

## 3. やらないこと

- 静止画側 (`ui_fullscreen.rs` の既存一覧) は**変更しない**。動画側を合わせるだけ。
- キーの順送り (`FsPanoramaProjection`) の挙動は**変えない**。
- 視点リセットボタンは今のままでよい。

## 4. テスト

- `SetPanoramaProjection` が `App::set_panorama_projection` へ届き、方式が変わること。
- **一覧を出している間、入力捕捉 RECT に一覧の矩形が含まれること**
  (抜けると背面でドラッグが始まる回帰)。
- 360 を抜ける / 音声モードへ移ると一覧が閉じること。
- 一覧の各行が画面内に収まること (右端寄りのボタンから吊るしても外へ出ない)。

`cargo test -p mimageviewer --lib` 緑、`cargo fmt`、
`python scripts/check_ui_glyphs.py` 0 件、新規コードに clippy 指摘なし、
`.\scripts\build-dev.ps1` が通ること。**アプリは起動しない。**

## 5. 迷ったら

- **症状を消す guard / delay / retry / silent fallback を入れない。**
- 構造判断で迷ったら実装せずに backlog §1.112 へ質問を書いて止める。
