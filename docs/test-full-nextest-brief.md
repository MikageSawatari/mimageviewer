# 実装依頼: リリース前の全体テスト `scripts/test-full.ps1` を cargo-nextest へ移す

- 担当: Codex GPT-6.1 Sol / high (実装・テスト)。完了後に別の Sol / xhigh が独立レビューする
- 作業場所: worktree `G:\home\mimageviewer-fullnextest`、ブランチ `full-nextest` (master 21673c767 から分岐)
- 依頼者: ClaudeCode Opus (設計・検収)
- **コミットはしない**。この sandbox からは linked worktree の git 管理領域へ書けない。変更は作業ツリーに残し、
  最終報告に「論理単位ごとのファイル一覧とコミットメッセージ案」を書く。私がコミットする。git の書き込みで止まらない。

作業前に `AGENTS.md`、`docs/development-build-and-test.md`、`docs/test-speed-report.md`、`scripts/test-full.ps1`、
`scripts/build-dist.ps1`、`.config/nextest.toml`、`scripts/test-tools-common.ps1` を読むこと。

## 背景

- 2026-10-16 に本体 lib の反復用として cargo-nextest を導入した (`test-lib-full.ps1` / `test-focused.ps1`)。
  本体 lib 全件は cargo test 28分31秒 → nextest 約6〜7分 (App テストが process-global な
  `data_dir::test_override_lock()` で直列化していたのが、プロセス分離で解消)。
- リリース前ゲート `scripts/test-full.ps1` は `cargo test` のままで、最後の実行は 50分54秒。
  `build-dist.ps1` が clean 前に必ず呼ぶので、配布ビルドのたびにこの時間がかかる。
- この PC には cargo-nextest 0.9.148 が入っている。他のツールはインストールしない。

## 目的

`test-full.ps1` を nextest ベースにして所要時間を大きく減らす。**ゲートとして検査する範囲は1件も減らさない**。

## 要件

1. **実行範囲は今と同じ**:
   - workspace 全体 `--features pack-build-tools` の lib・bin・integration test (`tests/*.rs`)・各 crate
   - 本体以外の workspace crate (remote-ipc、remote-web、comic-core、music-core、local-adjust-core、launcher など)
   - workspace 外の vendor 4 本 (vendor/egui の lib、root graph の `-p egui-winit --lib`、vendor/egui-wgpu `--features winit --lib`、
     vendor/eframe `--no-default-features --features wgpu --lib`)。今のコメントにある理由 (filter を付けない、root graph から選ぶ) を保つ
   - **doctest は nextest で動かないので、`cargo test --doc` を別段で必ず実行する** (今 doctest として走っているものと同じ範囲。
     今回の報告では「実行0件・ignored 3件」だった。範囲を変えずに件数を突き合わせる)
2. **件数の突き合わせ**: 移行前 (`cargo test`) と移行後 (nextest + doc) で、graph ごと (workspace、vendor 4 本、doc) に
   実行・成功・ignored の件数とテスト名の集合が一致することを確かめ、`docs/test-speed-report.md` に追記する。
   差があれば理由を説明する (例: 二重登録の解消)。説明できない差は残さない。
3. **プロセス分離で挙動が変わるテスト**が integration test や他の crate にあれば、テスト側だけを直す。
   期待値の緩和・削除・skip・retry で隠さない。製品コードの変更が要るなら止めて報告する。
4. **`-SuppressCrashDialogs`** は今、PowerShell 自身の `SetErrorMode` を子プロセスへ継承させている。nextest が起動する
   テストプロセスにも同じ error mode が届くかを確かめる (nextest のプロセス生成フラグ次第で継承されない可能性がある)。
   届かないなら、同じ効果を得る方法を考えて報告する (無人のリリースゲートで WER ダイアログが出て止まらないことが目的)。
5. **nextest が無い環境**では、黙って `cargo test` に戻さない。インストール方法 (`cargo install cargo-nextest --locked`) を示して
   非ゼロで終了する。
6. 失敗の扱いは今と同じ: 段ごとに exit code を見て、失敗したら後段を走らせず非ゼロで終える (`--no-fail-fast` で段内は全件実行)。
   retry は 0。flaky を retry で通さない。
7. nextest の設定は既存の `.config/nextest.toml` を使ってよい。ゲート専用の profile が必要なら追加する
   (slow-timeout、出力など)。先行実行 (priority) の設定は維持してよい。
8. `.ps1` は ASCII のみ、PowerShell 5.1 で動くこと。
9. 文書: `docs/development-build-and-test.md`、`docs/release-checklist.md` の 9.8 (Rust 全体テストゲート)、
   `.config/nextest.toml` 冒頭のコメント (「test-full は cargo test のまま」) を新しい構成に合わせる。CLAUDE.md は編集しない
   (必要な変更があれば報告に書く)。

## 確認

- 移行前の `cargo test` 側の件数は、今の `test-full.ps1` の各段から取る (全件の再実行が必要なら 1 回だけ)。
- 新しい `test-full.ps1` を最後に 1 回通して所要時間を記録する (前回 50分54秒)。
- `scripts/test-test-tools.ps1` も通す (共通部品を触った場合)。

## 進め方と報告

- 止めてよいのは、製品コードの変更が要るとき、新しいツールのインストールが要るとき、前提の矛盾を見つけたときだけ。
- まれな失敗 (ログの書き込み失敗など) は既定の割り切り方針に従う。回復の仕組みは作らない。
- 最終報告 (日本語): 移行前後の所要時間、graph ごとの件数の突き合わせ結果、プロセス分離で直したテスト、
  SuppressCrashDialogs の確認結果、変更ファイルの論理単位とコミットメッセージ案、未解決の点。
