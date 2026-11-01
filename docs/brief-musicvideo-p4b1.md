# 実装 brief: P4b-1 クレート移動と FFmpeg デコード（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の D16 末尾「#### P4b の設計」節の **P4b-1** が正本（実装前レビュー済み）。
D10・D16 も読むこと。前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 作業範囲

1. `tools/musicvideo_lab` を `crates/musicvideo-editor` へ `git mv` で移す（履歴を残す）。
   package 名 `musicvideo-editor`、製品 bin `mimageviewer-musicvideo`。ラボは残さない。
   - 共通のホスト（App・worker・診断の本体・既存のテスト）はクレートの **lib** に置く。
   - 製品 bin は入口だけ: 引数なしで GUI を起動する（P4c の引数はまだ足さない。知らない引数は今は無視でよい）。
   - 開発用 bin `musicvideo-dev`（`required-features = ["dev-tools"]`）に、ラボ `main.rs` の診断・ベンチマーク用の
     引数（`--benchmark` / `--preview-benchmark` / `--sample-frame` / `--lyrics-sample` / `--keyboard-distribution` /
     `--stationary-note-response` / `--kick-envelope-response` / `--project-*` など全部）を移す。
     `--preview-benchmark` のように GUI を起動するものも同じ lib のホストを使う。
   - 製品 bin と開発 bin の両方の入口で、I1 の `setup_egui` の登録を通す。
2. workspace: members の `tools/musicvideo_lab` を `crates/musicvideo-editor` に置き換える。default-members には
   入れない（`-p musicvideo-editor` で扱う）。`Cargo.lock` を更新する。
3. 依存: 本体 lib `mimageviewer`（path 依存）を加え、音声デコードを
   `mimageviewer::audio_decode::decode_audio_file_to_stereo_f32` と
   `mimageviewer::video::mvexport::default_audio_stream_index`（書き出しと同じ既定トラック）に替える。
   symphonia とラボ独自の resample を外す。`AudioData` の形（stereo f32 / 48kHz / duration）と
   PCM の識別（`ResourceRevision`、H3a の key）は維持する。デコードは従来どおり worker 上で、取消を渡す。
   本体の feature の既定（`default-features`）の扱いは、ビルドが通る最小の形にする（本体の build.rs の vendor 検査は
   省けない。D16 で決定済み）。
4. パスの参照を更新する: `crates/musicvideo-core/tests/analysis_routes.rs` の旧ラボのソースパス、
   ドキュメント内のコマンド（`cargo ... -p musicvideo_lab` を使っている brief 以外の現行手順、`scripts/` 内の
   参照があれば）、`.gitignore` 等。過去の記録（設計書の実施記録、過去の brief）の文中は書き換えない。
5. `scripts/test-full.ps1` がこのクレートのテストを従来どおり実行することを確認する（workspace 全体なら自動）。
   `dev-tools` 付きの検査コマンド（`cargo check -p musicvideo-editor --features dev-tools --bins`）を設計書の
   記録に書く。
6. 設計書の P4b 設計節の後ろに「P4b-1 実装記録」を追記し、§7 の P4b 行の状態を更新する。

範囲外: 音声スレッド（P4b-2）、書き出し（P4b-3）、未保存確認（P4b-4）、起動引数（P4c）。挙動は変えない
（デコーダが symphonia から FFmpeg に替わる差だけ）。

## 3. テスト

- 移動前と移動後で、ラボのテストが**件数も名前も**同じだけ通ること（一覧を突き合わせる。件数だけで判定しない）。
- 追加: デコードが FFmpeg 経路を通り、書き出しと同じ既定トラックを選ぶこと（複数音声トラックの合成素材で
  `default_audio_stream_index` と一致すること。素材はテスト内で生成するか既存の fixture を使う）。
  取消でデコードが打ち切られること。
- H1 / H2 / H3a（A1b を含む）/ I1 / L1〜L3 の既存テストがそのまま通ること。
- 確認コマンド:
  ```
  cargo test -p musicvideo-editor
  cargo check -p musicvideo-editor --features dev-tools --bins
  cargo test -p musicvideo-core -p musicvideo-ui -p music-core
  cargo test -p mimageviewer --lib mvexport
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  ```
- `cargo test -p mimageviewer` 系とエディタのテストは `target/debug/deps` に FFmpeg DLL が要る（配置済みのはず。
  無ければ `vendor/ffmpeg/bin/*.dll` をコピー）。`MSBUILDDISABLENODEREUSE=1` を付ける。
- release ビルドが通ること: `cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo`。
  exe サイズとビルド時間を記録する（D16: 推測せず測る）。release の exe を直接動かす場合に必要な DLL の配置は
  記録に書くが、起動はしない。

## 4. 注意

- 本体アプリもエディタも起動しない。コミットしない（このサンドボックスは git dir に書けない。`git mv` が
  使えない場合は通常の移動で構わない。設計担当がコミット時に rename として扱う）。`cargo fmt` はかける。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 5. 報告

変更・移動したファイル、移動前後のテスト一覧の突き合わせ結果、追加テストと失敗確認、release の exe サイズと
ビルド時間、正本との相違（あれば）を日本語で。
