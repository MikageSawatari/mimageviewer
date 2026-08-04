# ブリーフ: 上部バーロック時にインジケータが隠れる (バックログ 1.44) + 既知の問題の棚卸し

対象: v2.11.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
**今夜のリリースに載せる小規模修正。範囲を広げないこと。**

---

## 1. バックログ 1.44 の修正

正本は [docs/next-release-backlog.md](next-release-backlog.md) §1.44。着手前に読むこと。

**症状** (2026-08-04 利用者報告): 静止画で上部ツールバーをロックしていると、
スライドショーのインジケータが見えない。

**壊れている前提**: `draw_slideshow_progress_indicator` が円形インジケータを
`full_rect.max.x - 22, full_rect.min.y + 22` = **ウィンドウ全体矩形の右上**に描く
([ui_fullscreen.rs:18895](../src/ui_fullscreen.rs))。上部バーをロックすると
(`fullscreen_top_bar_locked`) バーが常時表示になり、**インジケータより後に**描かれる
([ui_fullscreen.rs:9270](../src/ui_fullscreen.rs) → [9493](../src/ui_fullscreen.rs)) ため
同じ帯を上書きする。非ロック時はバーがホバー時しか出ないので見えていた。

**同型の穴を通る他のインジケータも同時に直すこと**:

- `draw_fs_transparent_bg_indicator` ([18862](../src/ui_fullscreen.rs))
- `draw_original_preview_indicator` ([18888](../src/ui_fullscreen.rs))
- 下端の `draw_compare_pin_indicator` はシークバーのロックに対して同じ関係になる

**対応方針**: インジケータの基準を、ロック時にバーが確保した後のコンテンツ矩形
(`fullscreen_media_rect` 系) に統一する。**個別に「ロック中は 40px 下げる」のような定数を
足さないこと。** バー高さの変更・DPI・UI 表示倍率で再びずれる。

**完了条件 / 回帰テスト**:

- 上部バーのロック ON / OFF、ホバー中 / 非ホバーのいずれでもインジケータが見える
- 透過背景インジケータと原画プレビュー表示も同様に隠れない
- 下端の比較ピンインジケータもシークバーのロックで隠れない
- UI スナップショット (`cargo test --test ui_snapshot`) に差分が出るなら意図確認のうえ更新
  (更新した場合は PNG を目視確認したうえで報告すること)

規模: Small / P3。見た目のみで機能欠落は無い。

---

## 2. 既知の問題ページの棚卸し

`htdocs/mimageviewer/manual/known-issues.html` から **VST カーソルの項目を削除する**。

- 「全画面で動画を再生中に VST プラグインの画面を開くと、マウスカーソルが消えたままになる」
- これはバックログ §1.28 で、**v2.10.0 の commit `e1940617` で修正済み**。
  v2.10.0 のリリース時に手順 Phase 1 の 6.5 (直した項目の削除) が漏れていた
- backlog 側は既に「1.28 対応済み」へ更新済み

残るのは「一部の MPEG ファイルでシークバーによる移動ができない」(§1.13) の 1 件。
**掲載が 0 件にはならないので、「現時点で把握している不具合はありません。」への差し替えは不要。**

残った項目の記述が v2.11.0 で正しいか (回避方法が変わっていないか) も確認すること。

---

## 3. 制約

- **範囲を広げないこと。** 今夜のリリースに載せる小規模修正である
- アプリは起動しないこと。検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- `cargo fmt` (引数なし) と `cargo test -p mimageviewer --lib` を通すこと
- UI 文言を変えたら `python scripts/check_ui_glyphs.py` を通すこと
- detached-rework 凍結ルールは有効。触れた範囲は
  [detached-rework-plan.md](detached-rework-plan.md) へ記録する

完了したら変更内容・触れた範囲・テスト結果を報告すること。
