# 作業 brief: master 取り込み（egui 0.37）の仕上げ（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high。設計・検収: ClaudeCode Opus 5.5。報告は日本語。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。**merge の途中**（`MERGE_HEAD` = master `961e49372`）。

## 1. 状況

master（748 コミット先）を `git merge --no-ff --no-commit master` で取り込み中。設計担当が次を解消済み:
文書 4 本（両方を残した）、`src/app.rs`（master の `render_other_worker_results` へ集約された側を採用し、
`poll_musicvideo_launches(ctx)` を `src/app/update_phases.rs` の `poll_external_tool_launch` の直後へ移した）。

残り:
1. `src/ui_music_spectrum.rs` の衝突（テスト 1 か所、`draw_bass_marker` の引数の形が両側で違う）。ブランチ側は
   スペクトラム・鍵盤の描画を `music-core` と共有するために書き換えていた（`git log <merge-base>..HEAD -- src/ui_music_spectrum.rs`）。
   master 側はその後 HarmNet 等で進んだ。**master の挙動と描画を基準**にし、ブランチの共有化（書き出しと同じ描画を使う）が
   壊れないように合わせる。非テスト部分が自動マージで矛盾していないかも確認する。
2. `Cargo.lock`、`tests/snapshots/preferences_context_menu_layout{,_open_with}.png`（master 側を起点にし、ロックは cargo に
   作り直させ、スナップショットは「音楽ビデオを作成」の項目追加の分だけを `UPDATE_SNAPSHOTS=1` で更新して目視で確かめる）。
3. **egui 0.37 への追従**: master は egui / eframe を 0.33 → 0.37（wgpu 30）へ上げた（vendor の patch 含む。master の
   `docs/egui-0.37-*.md` 等の記録を読む）。`crates/musicvideo-editor` と `crates/musicvideo-ui` は 0.33 のままなので 0.37 に上げ、
   API の変更に追従する。`egui-ime` は master で既に 0.37。master の本体側の移行の仕方（同じ API の置き換え）に揃える。
4. 自動マージで入った master の変更と、ブランチの変更（mvexport、runtime_cleanup、launcher、build スクリプト、context menu など）が
   コンパイル・テストで噛み合っているかを確かめ、壊れていれば直す。

## 2. 確認

```
cargo check -p mimageviewer --bin mimageviewer-core
cargo test -p musicvideo-core -p musicvideo-ui -p musicvideo-editor
cargo test -p music-core
.\scripts\test-focused.ps1
cargo fmt --check
python scripts/check_ui_glyphs.py
```
本体 lib の全件は `.\scripts\test-lib-full.ps1` で 1 回（filter なしの `cargo test -p mimageviewer --lib` を直接打たない。
CLAUDE.md の現行規則）。`MSBUILDDISABLENODEREUSE=1` を付ける。
H3a / H3b（プレビューと書き出しの一致）、P4b〜P4f のテストがすべて通ること。描画結果が変わるテストがあれば、
変わった理由（egui 0.37 による表示側の差か、書き出しの画素の差か）を分けて報告する。**書き出しの画素（mvexport の RGBA）は
egui に依存しないので変わらないはず**。変わったら止めて報告する。

## 3. 注意

- アプリ・エディタ GUI は起動しない。`%APPDATA%\mimageviewer` に触れない。**コミットしない**（merge のコミットは設計担当が行う。
  このサンドボックスは git dir に書けない）。`cargo fmt` はかける。
- master 側の挙動を、ブランチの都合で変えない。迷ったら止めて報告する。
- 自分の確認を独立レビューと書かない。緑の区切りで止めて報告しなくてよい。

## 4. 報告

解消した衝突と判断、egui 0.37 への変更点の一覧、テスト結果（件数）、変わったスナップショット・描画の理由を日本語で。
