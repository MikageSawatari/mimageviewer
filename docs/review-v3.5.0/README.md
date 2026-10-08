# v3.5.0 リリース差分レビュー

**最新の再レビューは [第8回レビュー](round8/README.md) を参照。** `200c7781b` を確認。U01 は通常の一意キー一覧で解消し、新規の修正必須指摘なし。検索部分は 1 万件 / 5,000 ページで約 14.7 秒 → 7.3 ms。新規 4 件 / 既存 owner 12 件 / fmt は PASS。launcher 以外の workspace / vendor は 8,186 passed / 0 failed / 36 ignored (本体 7,269 件、UI snapshot 43 件を含む)。標準全体ゲートは launcher の release 入力 exe 未生成で未完了、launcher 7 件は未検証として区別する。延期承認事項は維持。[第7回](round7/README.md)、[第6回](round6/README.md)、[第5回](round5/README.md)、[第4回](round4/README.md)、[第3回](round3/README.md)、[第2回](round2/README.md)、以下の第1回 (`512b49d4d`) の記録は履歴として保持している。

確認日: 2026-09-03。担当: Codex。アプリコードを修正しない、機能別のリリース前レビュー。

**結論: 現時点でリリース可とは判断しない。** 自動テストは通るが、一括編集の viewer 所有境界を含む未解消指摘が 15 件ある。P1: 1 件 / P2: 13 件 / P3: 1 件。最初に検出した統合テストのコンパイル不整合 1 件は、レビュー中の別作業で解消された。

- [指摘と根拠](findings.md): 条件、違反した不変条件、修正する所有境界、回帰条件。
- [機能別レビュー](feature-review.md): 修正機能の棚卸し、適切だった設計、残る問題、入力・端数・Remote の横断確認。
- [検証記録と実機チェック](verification.md): 実行結果、再現資料、未実施の確認。
- [差分ファイルの対応表](file-coverage.tsv): 全差分ファイルとレビュー領域。

## 対象と前提

- 基点: `v3.4.0` (`6055aa2bd7977214f51e2ae32e7493bedfccd371`)。
- 開始 HEAD: `af46722162bffb1ec9c3f368c64406cc72885c54`、153 commits、157 files、32,237 insertions / 3,877 deletions。
- 最終 HEAD: `512b49d4dbb1fb3d64801458c629d769331bf881`、156 commits、162 files、32,290 insertions / 3,894 deletions。
- 開始時の未コミット `src/version_highlights.rs` は保持。途中で別作業の 3 commit (`8da3f07cf` / `52e811cf1` / `512b49d4d`) が入り、その差分も確認した。
- 基本方針は高速・軽快な操作。比較的高性能な PC でも、UI thread の DB / decode / Shell / 全件走査 / wait を無制限に許容しない。機能制限で不具合を隠す修正は提案しない。
- detached は `detached-rework-plan.md` §2 の所有境界を基準に確認。今回の指摘は構造的な修正が必要な箇所の報告であり、viewport 条件の追加等の症状対策を実装していない。
- 通常・開発・リリースのアプリは起動していない。通常の設定データは操作していない。静的確認、自動テスト、計算再現と、Windows / 実機未確認を区別する。

## 機能別の進捗

全領域の差分レビューを完了。以下の「完了」は静的・自動検証の範囲であり、問題なし・実機合格という意味ではない。

| ID | 機能・所有境界 | 状態・関連指摘 |
|---|---|---|
| R01 | 外部ツール登録・引数・起動・関連付け | 完了: F02、F15 |
| R02 | 実体化・一時ファイル・起動 ACK・キャンセル | 完了: F03、F05、F09、F10 |
| R03 | 右クリックメニュー統一・HWND・入力 | 完了: F13、F16 |
| R04 | 編集一括貼付・解除・保存順・undo | 完了: F08 |
| R05 | 一括書き出し・隠蔽 preset・焼き込み | 完了: F03、F04、F09。F01 は解消 |
| R06 | 描画 geometry・端数 px・Lanczos | 完了: F12 |
| R07 | 動画ストリップ全尺・高さ・波形 cache | 完了: F07 |
| R08 | 情報パネル固定・静止画 / 動画 / 音楽 | 完了: F11。cursor / touch は追加実機項目 |
| R09 | ★日時順・一覧・smart folder・Remote | 完了: 確定指摘なし |
| R10 | 設定互換保護・移行・バックアップ | 完了: F15 |
| R11 | panorama 引継ぎ・crop のドラッグ | 完了: 確定指摘なし |
| R12 | gamepad OFF・操作カスタマイズ横断 | 完了: F06、F07、F16 |
| R13 | mIV Remote・IPC・AI 所有権 | 完了: F09。DB mutation 競合は追加確認項目 |
| R14 | その他・削除コード・マージ整合 | 完了: F14 |
| R15 | 自動検証・更新された HEAD の追認 | 完了: 全体 gate PASS。F01 解消 |

## 自動検証の結果

- `scripts/test-full.ps1`: **PASS、8,133 passed / 0 failed / 36 ignored**。workspace 全 target、integration、doc test、vendored egui-wgpu / eframe を含む。件数は harness の報告値の合計。
- Remote Web: 9 test files をそれぞれ Node で直接実行し **382 passed / 0 failed**。通常の `node --test` は環境の子プロセス起動制限で実行できなかった。
- `cargo fmt --check`: PASS。
- UI glyph 検査: dangerous glyph 0。
- 端数 geometry: 実ソースの関数を使った計算で F11 / F12 を再現。
- 設定移行: 使い捨て DB への障害注入で F15 を再現。

本レビューの変更物はこのディレクトリの記録・再現資料のみ。アプリ動作を変更していないため、検証用アプリのビルド・起動、commit、push、リリース操作は行っていない。

## 範囲の記録

`changed-files.tsv` / `commits.txt` は開始時点を保存。`changed-files-final.tsv` / `commits-final.txt` は最終 HEAD のスナップショット。`file-coverage.tsv` は後者の全ファイルを R01–R15 に対応付ける。
