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
  ×／キャンセルでは削除せず、確定時に件数を数え直す。
  成功通知は実際の削除件数、失敗はログ＋通知とする。
- UI スレッドの I/O: 利用者が提示した設計担当の判断に従い、出荷済みの環境設定と同じ
  同期 SQLite DELETE / count を共有する。最大 1000 件。既存区間に perf event はない。
  worker 化は今回行わず別件の検討対象。新しい DB 問合せ方式や同期 I/O 経路は作らない。
  検索入力履歴の即時設定保存は最近開いたフォルダ履歴の既存コマンドと同じ保存境界を使う。
- 簡素化: 短い既存処理を共有し、確認中をモーダルにする。新しい worker、保留、再開、
  rollback や世代は追加しない。まれな失敗は割り切り（ログ＋通知、再試行なし）。
- 検証: handler からの消去／保存／通知、空の no-save、確認と取消、確定時の再集計、
  閲覧履歴ビュー再読込、環境設定の入力遮断、リング同等性、ini／候補の往復を自動テストする。
  本体 lib 全件と build-dev は利用者側の担当。対話起動は行わない。

## 実装・検証記録

- 共通処理は `src/ui_dialogs/clear_history.rs`、環境設定の one-shot 要求は
  `apply_search_input_history_clear_request` / `apply_reading_history_clear_request`。
- 独立レビュー（Sol / xhigh）は、リング操作の網羅台帳と Enter リピートの指摘を修正確認し、
  未解消の指摘なし。通常 Window が OS テーマを継承する配色も規約適合と確認。
- 指定の nextest 関連群: 1404 件成功、失敗 0（run ID `2f53dbcf-e49d-4d6e-85b2-ea2acad9642a`）。
  `target/clear-history-related-junit.xml` に結果を保存。
- 最終ソースの追加確認: 14 件成功、失敗 0（run ID `bad41f2c-2dd1-4282-87bd-bf4e8ca63e7f`）。
  新規 12 テストと keymap / ring 網羅性の確認を含む。
  `target/clear-history-final-junit.xml` に結果を保存。
- `cargo check -p mimageviewer --bin mimageviewer-core` 成功。
  `cargo test -p mimageviewer --test ui_snapshot` は 257 件成功、失敗 0、PNG 更新なし。
- 重処理はすべて指定の `run-heavy.ps1 -Owner codex-clear` を使用。
  共有ロック書込不可のため、ラッパー自身が警告を出して BelowNormal / CARGO_BUILD_JOBS=6 へ移行。
  Cargo のビルドロックで順次実行された。
- `cargo fmt`（引数なし）、`git diff --check`、UI グリフ検査も成功。
  lib 全件、build-dev、対話起動、Git への書込は指示どおり未実施。

変更ファイル（19 件）:

- `src/app.rs`、`src/app/gamepad_input.rs`、`src/app/search_nav_entry_inventory_tests.rs`、
  `src/app/clear_history_tests.rs`、`src/keymap.rs`、`src/ring_shortcut.rs`
- `src/ui_dialogs/mod.rs`、`src/ui_dialogs/clear_history.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`
- `docs/README.md`、`docs/clear-history-actions.md`、`docs/keymap.ini.default`、
  `docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`
- `htdocs/mimageviewer/index.html`、`htdocs/mimageviewer/manual/shortcuts.html`、
  `htdocs/mimageviewer/manual/settings.html`、`htdocs/mimageviewer/manual/search.html`
