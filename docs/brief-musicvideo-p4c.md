# 実装 brief: P4c 起動引数と診断（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の **D10**（「起動と引数」「エディタが書くもの」「エディタの保存データ」「ライフサイクルと runtime」）と
§7 の **P4c** 行。D10 は P4a で独立レビュー済み（Q9 / Q10 と runtime 競合の割り切りは利用者決定）。
前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 作業範囲

1. **起動引数**（製品 bin `mimageviewer-musicvideo`）: `--audio <path>`（新規プロジェクトとして音声を開く）、`--project <path>`、
   `--data-dir <dir>`。相対パスはカレントディレクトリ基準で解決する。誤り（存在しないパス、`--audio` と `--project` の両方、
   値の欠落、知らない引数）はウィンドウ内に理由を表示し、空のプロジェクトで開く。終了コードだけで知らせない。
   引数の解析は純関数にして単体テストする。ファイルの存在確認は UI スレッドで同期に行わない（既存の読込 worker の失敗として
   表示するか、起動時の worker で確かめる）。`--audio` / `--project` の読込は既存の読込入口（H1 / P4b-4 の規則）を通す。
2. **最近使ったプロジェクト**: `<data_dir>/musicvideo/recent-projects.json`。D10 の規則どおり:
   - 入れるのは「読込が採用された時点」と「保存が成功した時点」の解決済み絶対パスだけ（要求の開始・取消・失敗・破棄は入れない）。
   - 読み書きと「一覧から開く」ときの存在確認はホストの worker が行う（UI スレッドから同期でファイルに触れない）。
   - 書込ごとに固有の一時ファイルから置き換え。複数エディタの同時書込は後勝ち。件数上限で古いものを落とす（上限は 10）。
   - 一覧の書き込み失敗は保存成功を覆さない（通知とログだけ）。壊れた・読めない一覧は空として扱いログに残す。
   - 一覧から開くときに無いファイルは理由を表示し、一覧から外せる。
   - 「最近使ったものから開く」は P4b-4 の `開く(パス)` 意図に合流させる（専用の判定分岐を作らない）。
   - `--data-dir` 無しでは一覧を読まず・書かない（メニューは無効表示でよい）。
3. **ログ**: `<data_dir>/logs/musicvideo.log`（追記。起動時に 4 MiB を超えていれば 1 世代だけ `.1` へ退避）。
   panic hook で `<data_dir>/logs/musicvideo-panic.log` へ追記。mIV の `mimageviewer.log` / `panic.log` には書かない・触らない。
   `--data-dir` 無しでは標準エラーだけに出す（ファイルを作らない）。ログの書き込みは UI を止めない形にする（小さな追記で
   よいが、遅いドライブで UI スレッドが待たないこと。必要なら既存の worker パターン）。
4. **runtime の shared lease**: 本体 lib に公開の入口を 1 つ足し（中身は既存の `runtime_cleanup::pin_running_version` と同じ規則。
   版番号は**本体の** `CARGO_PKG_VERSION` を使う。エディタ crate の版番号を使わない）、エディタは起動直後に取得して終了まで保持する。
   exe が `runtime/<version>/` 配下に無い場合（portable・開発ビルド）は取らない。取得に失敗したら編集を始めず、理由を表示して
   mIV から開き直すよう案内する（D10 の割り切り。再試行・待機・IPC は作らない）。
5. 設計書に「P4c 実装記録」を追記し、§7 の P4c 行を更新する。

範囲外: mIV 側の入口（P4d）、配布（P4e）、文書（P4f）。

## 3. テスト

- 引数解析: 正常 3 種・組み合わせ・誤り各種・相対パスの解決。誤りでウィンドウ内に理由が出ること（handler テスト）。
- 最近使った一覧: 壊れた JSON・無いファイル・上限・重複（同じパスは先頭へ移動）・取消 / 失敗 / 破棄された読込を入れない・
  保存成功で入る・一覧の書込失敗で保存成功が覆らない・`--data-dir` 無しで一切読み書きしない・一覧から開くが `開く(パス)` 意図を通る。
- ログ: `--data-dir` 無しでファイルを作らない、4 MiB 超の退避、panic hook の出力先、mIV のログファイルに触れない。
- lease: runtime 配下でない exe では取らない、版ディレクトリの判定、取得失敗時に編集を始めず案内を出す
  （取得関数は差し替え口でテストする）。版番号が本体のものであること。
- mIV の settings.db・キャッシュ DB・ログを開かないこと（コードの経路で示す）。
- 既存の H1 / H2 / H3 / I1 / L1〜L3 / P4b-1〜4 のテストがすべて通る。
- 各新テストが、意図的に壊した実装で落ちることを確かめて記録する。
- 確認コマンド（`MSBUILDDISABLENODEREUSE=1`）:
  ```
  cargo test -p musicvideo-editor
  cargo check -p musicvideo-editor --features dev-tools --bins
  cargo test -p musicvideo-core -p musicvideo-ui
  cargo test -p mimageviewer --lib runtime_
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
  ```

## 4. 注意

- 本体アプリもエディタ GUI も起動しない。コミットしない。`cargo fmt` はかける。テストで data dir を使うときは一時ディレクトリだけを使い、
  `%APPDATA%\mimageviewer` に触れない。
- まれな失敗（一覧・ログの書き込み失敗、lease 取得失敗）はログと通知だけ。回復の仕組みは足さない。
- 状態を増やす前に、既存の pending・worker・P4b-4 の意図の仕組みで表せないかを先に考える。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 5. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、正本との相違（あれば）を日本語で。
