# 履歴の全件消去操作

2026-11-13 利用者決定、設計担当 ClaudeCode。

- 範囲: `GridClearSearchInputHistory` / `GridClearReadingHistory` は Grid / Press、標準キーなし。
  リングの `ClearSearchInputHistory` / `ClearReadingHistory` と同じ入口を通す。
  マウスジェスチャ、マウスボタン、ゲームパッドの既存の候補にも載せる。場所移動は行わない。
- 検索入力履歴: `App::clear_search_input_history` を環境設定と共有する。live と環境設定の
  draft、件数、結果欄を同時に更新する。環境設定ボタンの保存時点は変更しない（OK の
  `save_checked`、または終了時等の通常保存）。キー／リングは空なら通知だけ、空でなければ
  共有処理後に `save_checked` で即時保存し、件数を通知する。保存失敗はログと警告。
  環境設定は共通モーダルに登録済みで背面のキー／リング入力を止める。
  OK 時も `prepare_preferences_settings_for_commit` → `overwrite_non_preferences_from` で
  最新 live の入力履歴を採用し、古い draft の履歴を戻さない。
- 閲覧履歴: `App::clear_reading_history` を環境設定ボタンと共有する。既存の
  `admit_book_append_followup_mutation`、DB 全件削除、ログ、表示中の履歴ビュー再読込を維持する。
  環境設定の結果表示は従来どおり。キー／リングだけ、空なら通知し、それ以外は確認を出す。
  状態は `show_reading_history_clear_confirm` だけ。
  `reading_history_clear_dialog_visible` を描画と共通モーダル判定で共有し、
  Enter / Escape は IME 対応 helper を通す。Enter のリピートでは確定せず、新しい押下を要する。
  キャンセル／Escape では削除せず、確定時に件数を数え直す。
  確認前と確定時は `ReadingHistoryDb::try_count` で取得失敗を区別し、0 件のときだけ
  「閲覧履歴はありません」と通知する。取得失敗はログ＋警告で終了し、確認も削除も行わない。
  DB 不在時は従来どおり確認後の共有削除処理でエラーを通知する。既存の `count()` の
  呼び出し元とエラー時 0 件の互換動作は維持する。
  成功通知は実際の削除件数、失敗はログ＋通知とする。
- UI スレッドの DELETE: 利用者が提示した設計担当の判断に従い、出荷済みの環境設定と同じ
  同期 SQLite DELETE と削除後の count を共有する。既存区間に perf event はない。
- UI スレッドの件数確認: キー／リングの確認前と確定時に `SELECT COUNT(*)` を同期実行する。
  接続の busy timeout は 3 秒。ローカル DB の行数は保持件数の上限（最大 1000 件）までで、
  利用者の明示操作時だけ実行するため、今回は既存の DELETE と同じ扱いで許容する。
  応答時間は未測定。件数確認の worker 化は DELETE とあわせて別件の検討対象とする。
- 設定保存:
  検索入力履歴の即時設定保存は最近開いたフォルダ履歴の既存コマンドと同じ保存境界を使う。
- 簡素化: 短い既存処理を共有し、確認中をモーダルにする。新しい worker、保留、再開、
  rollback や世代は追加しない。まれな失敗は割り切り（ログ＋通知、再試行なし）。
- 検証: handler からの消去／保存／通知、空の no-save、確認と取消、確定時の再集計、
  閲覧履歴ビュー再読込、環境設定の入力遮断、リング同等性、ini／候補の往復を自動テストする。
  本体 lib 全件と build-dev は利用者側の担当。対話起動は行わない。

## 実装・検証記録

- サブ PC 担当の実機観測（確認画面の背後のタイルが選択された）を受け、確認を削除確認と同じ `egui::Modal` に変更。共通モーダル登録を保ち、背景クリックは閉じずに吸収する。実際の一覧へのクリック・ホイール・キー入力の遮断とボタン操作を新規 2 件の egui 入力テストで確認済み。run-heavy 経由で履歴操作 15 件、関連 1410 件（run ID `20d5f67b-490b-4464-a228-6252bf3f61a6`）、ui_snapshot 257 件が成功、PNG 更新なし。

- 共通処理は `src/ui_dialogs/clear_history.rs`、環境設定の one-shot 要求は
  `apply_search_input_history_clear_request` / `apply_reading_history_clear_request`。
- 独立レビュー（Sol / xhigh）は、リング操作の網羅台帳と Enter リピートの指摘を修正確認し、
  未解消の指摘なし。通常 Window が OS テーマを継承する配色も規約適合と確認。
- 指定の nextest 関連群: 1404 件成功、失敗 0（run ID `2f53dbcf-e49d-4d6e-85b2-ea2acad9642a`）。
  `target/clear-history-related-junit.xml` に結果を保存。
- 初回実装の最終ソースの追加確認: 14 件成功、失敗 0（run ID `bad41f2c-2dd1-4282-87bd-bf4e8ca63e7f`）。
  新規 12 テストと keymap / ring 網羅性の確認を含む。
  `target/clear-history-final-junit.xml` に結果を保存。
- `cargo check -p mimageviewer --bin mimageviewer-core` 成功。
  `cargo test -p mimageviewer --test ui_snapshot` は 257 件成功、失敗 0、PNG 更新なし。
- 重処理はすべて指定の `run-heavy.ps1 -Owner codex-clear` を使用。
  共有ロック書込不可のため、ラッパー自身が警告を出して BelowNormal / CARGO_BUILD_JOBS=6 へ移行。
  Cargo のビルドロックで順次実行された。
- `cargo fmt`（引数なし）、`git diff --check`、UI グリフ検査も成功。
  lib 全件、build-dev、対話起動、Git への書込は指示どおり未実施。

### e1d67ebf7 の独立レビュー指摘への対応

- P2: 確認前・確定時の件数取得を `try_count()` へ変更し、取得失敗はログと警告で終了する。
  既存の `count()` と DB 不在時の扱いは維持。再試行は追加しない。
- 新規回帰 3 件成功（run ID `cfff9842-c5ac-4a41-80b1-ee42c988d233`）。
  別の SQLite 接続の `BEGIN EXCLUSIVE` により実際の読み取り失敗を発生させ、確認前・確定時の
  警告、ダイアログの終了、履歴保持、一覧の非更新を検査。DB 単体では `try_count()` の失敗と
  既存 `count()` の 0 件へのフォールバックを検査。使い捨て DB だけを使用する。
  結果は `target/clear-history-review-focused-junit.xml`。
- P3: 同期 COUNT の実行箇所、busy timeout、今回の許容理由、未測定の応答時間と
  DELETE とあわせた worker 化の別件扱いを上記へ明記した。
- 前回と同じ nextest フィルタで関連 1408 件成功、失敗 0
  （run ID `e524d94a-c8e8-40fb-bf76-250bf6db8214`、上記の新規 3 件を含む）。
  結果は `target/clear-history-review-related-junit.xml`。
  重処理は指定の run-heavy 経由。UI グリフ検査も成功。

変更ファイル（初回実装とレビュー修正の合計 20 件）:

- `src/app.rs`、`src/app/gamepad_input.rs`、`src/app/search_nav_entry_inventory_tests.rs`、
  `src/app/clear_history_tests.rs`、`src/keymap.rs`、`src/ring_shortcut.rs`、`src/reading_history_db.rs`
- `src/ui_dialogs/mod.rs`、`src/ui_dialogs/clear_history.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`
- `docs/README.md`、`docs/clear-history-actions.md`、`docs/keymap.ini.default`、
  `docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`
- `htdocs/mimageviewer/index.html`、`htdocs/mimageviewer/manual/shortcuts.html`、
  `htdocs/mimageviewer/manual/settings.html`、`htdocs/mimageviewer/manual/search.html`
