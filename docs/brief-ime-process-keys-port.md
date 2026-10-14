# ブリーフ: IME が処理したキーの根本修正（I1 共有化 + I2）を master 系へ移植する

対象 worktree: `G:\home\mimageviewer-imefix`（ブランチ `ime-process-keys`、起点 master `c7c76d239`）
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

## 0. 背景

音楽ビデオのブランチ `musicvideo`（worktree `C:\home\mimageviewer-musicvideo`）で、日本語 IME の不具合
（画像注釈の本文で確定→すぐ Enter の改行が入らない等）を根本から直した。独立レビューはすべて ACCEPT、
設計 lead が隔離データの実機で Google 日本語入力と MS-IME の両方で確認済み。利用者の判断で、
音楽ビデオより先に master へ入れる（次のリリースに含める）。

設計と記録の正本は `musicvideo` ブランチの `docs/music-video-export-plan.md` の「I1」と「#### I2（IME が処理したキー）」節。
まず `git -C C:\home\mimageviewer-musicvideo show musicvideo:docs/music-video-export-plan.md` で該当節を読むこと。

## 1. 移植するコミット（`musicvideo` ブランチ、古い順）

1. `367f9c9d3` I1 段 2a: egui の IME 補正を `crates/egui-ime` へ切り出し（本体は `src/ime_focus.rs` を facade に）
2. `fde3e819e` I1 段 2b: **本体側の部分だけ**（`crates/egui-ime/**`、`src/egui_focus_policy.rs`、それに伴う `Cargo.lock`、
   `docs/architecture-overview.md`）。音楽ビデオのエディタ（`crates/musicvideo-ui`、`tools/musicvideo_lab`）への適用は持ち込まない。
3. `282f173c4`、`1cbe1888f` I2 段 1: egui-winit 0.33.3 を `vendor/egui-winit` に取り込み、診断 trace
4. `62bc9f47c`、`29367ba4d` I2 段 2a: Process キーを egui へ渡さない + 対症療法の撤去
5. `291dbf8e5`、`efce16eb0` I2 段 2b: 300ms grace の撤去、focus 復帰の作り直し、生キー経路の 0xE5

どのコミットからも、次は持ち込まない: `crates/musicvideo-ui/**`、`tools/musicvideo_lab/**`、`crates/musicvideo-core/**`、
`docs/music-video-export-plan.md`、`docs/brief-musicvideo-*.md`、そのほか音楽ビデオ専用のファイル。
`git cherry-pick -n` などで 1 つずつ当て、除外パスを戻してからコミットする（元の作者表記と要旨を残し、
メッセージに元のコミットハッシュを `(cherry picked from musicvideo <hash>)` の形で書く）。master 側で同じファイルが
進んでいて衝突した場合は、master 側の変更を保ったまま意図どおりに解決する。**このブランチではコミットしてよい**
（master への merge・push はしない）。

## 2. 追加の変更: 取り込んだ egui-winit の修正を本家 0.34 以降と同じ形に揃える

egui 本家は egui-winit 0.34.0（PR #7967、2026-03）で同じ問題を直している。`crates/egui-winit/src/lib.rs` の
`try_on_ime_processed_keyboard_input`（Windows のみ）: `allow_ime` のときだけ、`logical_key == NamedKey::Process` の press を
捨て、その `physical_key` を `pressed_processed_physical_keys: HashSet<PhysicalKey>` に記録し、対応する release も捨てる。
本家の現行コードは `https://raw.githubusercontent.com/emilk/egui/master/crates/egui-winit/src/lib.rs`（0.34.0〜0.36.0 で同じ）。
利用者の方針（2026-10-14）: egui を上げるときに `vendor/egui-winit` を消すだけで済み、上げる前後で動きが変わらないよう、
**本家と同じ判定・同じ状態に揃える**（段 2a の「release は通す・状態を持たない」を本家の形に置き換える）。
段 2a の純関数・配送経路テスト（`append_keyboard_input` 等）は、本家の形に合わせて書き直す（release も捨てる、allow_ime が false なら渡す、
press が Process でなかった物理キーの release は渡す）。`PATCHES.md` に「本家 0.34.0 の #7967 と同じ。egui 更新時はこの差分ごと消す」と
出典つきで記録する。trace はそのまま残してよい。

## 3. 文書

- master には音楽ビデオの計画書の I2 節が無い。`docs/ime-process-keys-plan.md` を新設し、I2 の経緯・実機記録（Google / MS-IME）・
  段 1 / 2a / 2b の設計と決定（利用者が全面修正 B を選んだこと、確定直後だけ違う動きを揃えたこと）・独立レビューの結果・
  egui 本家との関係（0.34 で同じ修正、更新時に vendor を消す）を簡潔にまとめる。I1（egui-ime への切り出し）の要点も短く載せる。
  `docs/README.md` の索引と CLAUDE.md「IME 対応」節からリンクする。
- CLAUDE.md・`docs/keymap-spec.md`・`docs/spec.md`・マニュアル（`htdocs/mimageviewer/manual/search.html`）等の変更は、
  移植したコミットのものをそのまま持ち込む（master 側の他の変更と衝突したら両方を保つ）。

## 4. 完了条件

1. `cargo fmt --check`、`cargo test -p egui-winit --lib`、`cargo test -p egui-ime`、`cargo test -p comic_lab`、
   `cargo test -p mimageviewer --lib ime` / `focus` / `keyboard` / `text_mode` / `ime_focus` / `egui_focus_policy`、
   `cargo check -p mimageviewer --bin mimageviewer-core`。`cargo tree -i egui-winit` が vendor の 1 系統であること。
   新しい worktree では lib テストの前に FFmpeg DLL を `target/debug/deps` へ置く必要がある（`vendor/ffmpeg/bin/*.dll`）。
2. `.\scripts\test-full.ps1`（必要なら `-SuppressCrashDialogs`）。master 既存の失敗（App のサイズ制限テスト、
   capture_region の stack overflow）以外が無いことを musicvideo での段 2a/2b と同じ補完手順で確認する。
   test-full の前提（`target\release` の core / remote / EPUB worker）が無ければ CLAUDE.md の手順で先に作る。
3. `$env:MSBUILDDISABLENODEREUSE='1'` で `.\scripts\build-dev.ps1`（実機確認は設計 lead が隔離データで行う）。
   PowerShell スクリプトをツールから呼ぶときは `*>&1` を付けない。
4. まれな失敗は割り切る（CLAUDE.md）。

## 5. 報告

各コミット（移植元との対応）、衝突と解決、§2 の変更点、テスト結果（コマンドと件数）、`git log --oneline master..HEAD`。短く。
アプリ起動はしない。自分で「独立レビュー」を名乗る確認は不要（設計 lead が別に依頼する）。
