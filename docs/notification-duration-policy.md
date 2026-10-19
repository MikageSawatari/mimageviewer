# 一時通知の表示時間（backlog §1.402）

## 所有と設定

`feedback::resolve_duration` だけが「通知の表示時間」の倍率を適用する。
通常通知は App の発行時に秒数を確定し、一覧・画像 viewer・F12/別窓の既存 toast state と
native overlay へ同じ秒数を渡す。描画時には設定を読まず、窓の切替・mount/deposit で再計算しない。
保存先を開く `fs_feedback_toast_reveal_path`、発火面、連続通知による置換は維持する。

動画内部が発行する通知（フィルタ準備・失敗、再生位置操作を使えない案内）は、既存の
`NativeChromeSnapshot.policy` に表示時間の選択値を追加し、その設定経路を使う。
`NativeRenderCore::show_overlay_toast` は未確定の通知だけを同じ関数で確定し、App から届いた
明示 linger は再計算しない。環境設定OK後の既存 `sync_all_native_chrome_policy` が全登録contextへ反映する。
新しい設定同期 channel、worker、context state は追加しない。
native の fade は従来、linger と無関係に 1.8/2.5 秒で透明になっていたため、確定 linger に揃える。

状態の組合せを増やさない簡素化として、既存通知 state と設定 snapshot の利用を採用した。
設定変更は次に発行する通知から反映し、表示済みの通知を延長・再生成する専用状態は設けない。
処理の進捗 overlay や入力所有、detached の述語・placement・viewport 経路は変えない。
まれな失敗は既定で割り切り（ログ＋通知）、回復機構は追加しない。

`Settings.notification_duration` は `Short / Standard / Long`（保存名 `short / standard / long`）。
リリース済み settings.db の既存 KV へ項目を追加する。欠落した旧設定は serde の既定値
`Standard` へ読み替える加算的移行で、既存行・schema version は変更しない。
将来の未知値も標準へ正規化する。全体設定ページの既存 anchor/helper、OK/キャンセル、
設定保存・復元・転送の経路を使う。

## 秒数

| 種類 | 短い | 標準（既定） | 長い |
| --- | --- | --- | --- |
| 情報・成功 | 基本時間×0.6 | 基本時間そのまま | 基本時間×1.5 |
| 警告・エラー・拒否理由・対処案内 | 基本時間そのまま | 基本時間そのまま | 基本時間×1.5 |

本文長方式は Unicode scalar value の数で、20文字以下1.2秒、超過1文字ごと0.12秒、上限5.5秒。
fade を含む。情報・成功の上限も同じ倍率（短い3.3秒、長い8.25秒）にする。
長い設定は上限も延ばさないと長文で選択の効果が消える。短い設定で短縮可能なのは、
対象名・件数を確認する成功/状態通知に限り、読解・判断が要る説明を保護する。
[右クリックメニュー一本化 §1-11](context-menu-unification-plan.md) の長文拒否理由は短縮しない。

| 入口・用途 | 標準の基本時間 | 適用/例外 |
| --- | --- | --- |
| `show_feedback_toast` / warning / error | 本文長方式 | 適用。種別は発行元で明示 |
| 面指定 `*_on` / `show_feedback_result_on` | 従来の1.2秒 | 適用。面は時間計算へ影響しない |
| `*_with_duration` / `*_on_with_duration` | 呼び出し元の指定秒数 | 適用。指定値は標準の基本時間であり設定回避ではない |
| 索引 worker の再起動・失敗通知集約 | 5秒 | 警告として適用 |
| 動画ピン・未変換書庫の代表画像設定拒否 | 5秒（2箇所） | 警告として適用 |
| 本へのページ追加の完了（除外・簡易再補完・取り消せない旨を含み得る） | 4.5秒 | 警告として適用。注意事項は省略しない |
| メディア窓からメインへの切替と開き直し案内 | 3.5秒 | 警告として適用 |
| ゲームパッド X ピッカーのメタデータ入口案内 | 4秒（`X_PICKER_HINT_TOAST_SECS`） | 警告として適用 |
| パイプラインデバッグ出力開始 | 2秒 | 一時通知として適用。処理中 overlay の寿命とは独立 |
| キャプチャ保存・デバッグ出力完了 | 1.2秒 | 適用。クリックで保存先/manifest を開く既存機能を保持。native も同じ確定秒数 |
| native の通常/中央 toast | 1.8秒/2.5秒 | 適用。準備通知は情報、失敗・利用不能は明示した警告 |
| native 相対シークの先頭/末尾 | 700ms | `Immediate` の例外。リピートで更新され、離した後すぐ消える操作フィードバック |
| `FsBoundaryHint` のナビゲーション境界・該当フォルダなし | 従来の専用2.5秒/4秒、native は専用中央表示 | 専用の操作フィードバックとして例外。通常 toast の設定に混ぜない |
| 処理の寿命で表示/消滅する進捗、AI status、スキャン/保存/変換モーダル、seek中表示 | 処理の寿命 | 対象外。開始時に一度出る toast とは区別 |

## 呼び出し元の棚卸し

棚卸し対象の dotted call（`show_feedback_toast*` と `show_native_video_overlay_toast`）を、
文字列内の括弧を除いて引数末尾まで抽出した。専用 tests ファイルを除いた抽出は1,037箇所。
`rg` と引数本文の「失敗/エラー/できません/ありません/必要/拒否/対象外/してください/
error/warning/reason/failure」等の検索で481候補を得た後、残りの固定文言を読み、変数・
helper 本文を渡す箇所を producer の分岐/戻り値まで追った。生成時に種類を指定しており、
実行時に本文から種類を推測する処理はない。

警告/エラーはファイル操作、クリップボード、タグ・評価、読書位置、本の構成/追加/並替え、
検索/索引、書庫/EPUB、補正/LUT/マスク/キャプチャ、外部ツール、動画/VST、復元/設定の各入口を
対象にした。成功文は対象名・件数・完了操作、表示設定・スロット・ツール変更は情報を既定とした。
失敗した二度目の操作に「処理中です」と返す箇所、空スロット/対象なし、古い結果の破棄も保護する。

判断に迷った/複合結果の一覧:

| producer | 決定 |
| --- | --- |
| 手動クリップボード保存・選択画像保存 | 既存の metadata error / failed / metadata_failed / errors で種別を決める。成功だけなら情報 |
| 一括編集 | failed・対象外・cancelled があれば警告。適用済み保持の説明を残す |
| 外部ツール完了集約 | failures の有無で分ける |
| Undo/Redo | 追加 notice があるときだけ警告 |
| 動画/音楽のブックマーク一括登録 | errors の有無で分ける。重複件数自体は成功を妨げない |
| コレクション追加/操作結果 | 既存 is_error を使用。保存先 path が必要な export 完了文は維持 |
| 本の読書位置クリア | Result の成功/失敗で分ける |
| バッチ書庫変換 | cancelled・失敗・既存/同名除外があれば警告 |
| 本追加の4.5秒summary | 簡易補完・hidden除外・Undo不能が混ざるため警告として保守的に保護 |
| native フィルタ確定通知 | 適用不能時の代替処理・対処が同じproducerに混ざるため警告として保護 |
| 画像フォルダのZIP変換結果 | 元フォルダが残ることと作成先pathの注意を含むため警告として保護 |
| スタックまとめの複合案内、類似 preview の非同期 notice | 拒否/読込不能を含むため警告として保護 |
| ドラッグ終了の除外/解決失敗集約、検索の visibility notice | 実行対象の除外を伝えるため警告 |
| 360°開始ヒント・操作方法の案内 | 判断や次の入力が必要な案内は保護、単純な表示モード名は情報 |

## 簡潔化した成功文

| 場所 | 変更前 | 変更後・残す情報 |
| --- | --- | --- |
| `ui_dialogs/rename_item.rs` | `名前を変更しました: <フルパス>` | `名前を変更: <変更後のファイル/フォルダ名>`。日本語・拡張子・長い名前を省略しない |
| `ui_dialogs/new_folder.rs` | `フォルダを作成しました: <フルパス>` | `フォルダを作成: <フォルダ名>` |
| `ui_dialogs/archive_convert.rs` | `ZIP ファイルを作成しました: <出力名>` | `ZIP を作成: <出力名>` |
| `ui_adjustment_panel.rs` | `LUT読み込み完了: <LUT名> (<size>^3) / <フルパス>` | `LUT読み込み: <LUT名> (<size>^3) / <ファイル名>`。LUT内部名・格子寸法を保持 |

キャプチャ保存は既にファイル名方式なので本文を変えない。コレクション export の保存先、
部分失敗や設定引継ぎの注意は削らない。完了文を短くした結果、標準設定でも文字数方式の通知は
短くなるが、時間式・係数自体は変えない。

## 検証台帳

対象は HEAD `28e0aa575` からの未コミット差分。通常feature / debug test、
`MSBUILDDISABLENODEREUSE=1`。実製品の起動・実機測定は行っていない。

- `cargo check -p mimageviewer --bin mimageviewer-core -j 1`: コンパイル成功。
  初回のturbojpeg依存ビルドが並列MSBuildで終了1となったため、同じ生成先を
  `cmake --build ... --target install --config Debug --parallel 1 -- /v:normal /nr:false`
  でビルドし直し、依存側0エラー・cargo check成功を確認した。製品コードのコンパイル失敗ではない。
- `cargo nextest run -p mimageviewer --lib -E 'test(/notification_duration|feedback_toast|rename|new_folder|batch_convert|settings_transfer|format_completion_toast|external_launch_completion_summary|native_command_bus|a_toast_does_not_swallow|chrome_suppression_command_bus/)' --no-fail-fast`:
  **188件成功・失敗0・exit 0**。`target/notice-audit/targeted-verified.{stdout,stderr}.log` と
  `verification.json`。時間の境界・警告/エラー下限・明示秒数・DB往復/項目欠落/未知値・
  名前変更の日本語/長い名前・native転送の一度だけの適用/設定snapshot・保存先リンク保持・
  toast置換/発火面・native fade・既存HUDクリック透過を含む。
- 初回は187件中186成功、設定転送の登録項目数の固定期待値1件だけが失敗。
  新しい項目を含む総数464 / 転送可148 / wire key146へ更新して上記で再確認した。
- `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --test ui_snapshot notification_duration_preferences -- --nocapture`:
  2件成功。更新フラグを外した通常比較も**2件成功・exit 0**。
  `target/notice-audit/snapshot-compare.{stdout,stderr}.log`。
- 新規PNGは `notification_duration_preferences_light_standard.png`（17,989 bytes）と
  `notification_duration_preferences_dark_short_narrow.png`（17,694 bytes）、計35,683 bytes。
  両方を目視確認し、3択の選択状態・全文の可読性・狭幅の折返しを確認した。
- ワークスペース全体の `cargo fmt` 実行後 `cargo fmt --check` 成功。
  `python scripts/check_ui_glyphs.py` は0件、`python scripts/check_site_cards.py` は49枚・上限超過0件。
- lib全件・full gate・独立レビューはbriefに従い後続担当へ引き継ぐ。
- `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`:
  **exit 0**。通常feature / dev-runtime、core・Remote・EPUB workerを生成。
  `target/notice-audit/build-dev.{stdout,stderr}.log` と `build-dev-result.json`。
  portable featureは使用していない。VCRT検査もruntime 4 / PE 3で成功。

未確認は実機の経過秒数・F12/複数窓/native overlayの見え方・保存通知のクリック動作。
通常profileをこちらから起動していない。利用者確認はインストール版/常駐版を終了後、
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
既定では実利用中の `%APPDATA%\mimageviewer` を開き、設定/データを更新し得る。
全体設定の3段階、長い親pathにある日本語名の名前変更、短い設定でも残る拒否理由、
動画の通常通知と700msのシーク境界を確認する。新たな範囲外問題は検出していない。


## 変更ファイル（commit引継ぎ用）

既存82件＋新規5件。commitは行っていない。

新規ファイル（未追跡）:

- `docs/notification-duration-policy.md`
- `src/app/tests/notification_duration.rs`
- `src/feedback.rs`
- `tests/snapshots/notification_duration_preferences_dark_short_narrow.png`
- `tests/snapshots/notification_duration_preferences_light_standard.png`

既存ファイル:

- `docs/README.md`
- `docs/architecture-overview.md`
- `docs/display-pipeline.md`
- `docs/next-release-backlog.md`
- `docs/spec.md`
- `docs/ui-snapshot-policy.md`
- `docs/video-architecture.md`
- `htdocs/mimageviewer/index.html`
- `htdocs/mimageviewer/manual/settings.html`
- `src/app.rs`
- `src/app/book_append_refresh.rs`
- `src/app/book_append_undo.rs`
- `src/app/book_composition_editor.rs`
- `src/app/book_composition_permission.rs`
- `src/app/book_composition_runtime.rs`
- `src/app/book_resume_meter.rs`
- `src/app/collection_grid.rs`
- `src/app/collection_navigation.rs`
- `src/app/color_filter.rs`
- `src/app/content_identity_detection.rs`
- `src/app/content_identity_restore.rs`
- `src/app/gamepad_input.rs`
- `src/app/image_folder_zip_operation.rs`
- `src/app/native_video.rs`
- `src/app/rating_membership_ops.rs`
- `src/app/saved_group_actions.rs`
- `src/app/sidecar_restore.rs`
- `src/app/smart_folder.rs`
- `src/app/snapshot_ops.rs`
- `src/app/spread_shift_owner.rs`
- `src/app/startup_ops.rs`
- `src/app/subfolder_expansion.rs`
- `src/app/tests.rs`
- `src/app/update_phases.rs`
- `src/app/viewer_context_registry.rs`
- `src/edit_bundle_app.rs`
- `src/edit_bundle_bulk.rs`
- `src/external_tool.rs`
- `src/filename_stack_ui.rs`
- `src/global_search_ui.rs`
- `src/global_search_ui/everything.rs`
- `src/lib.rs`
- `src/pipeline_debug.rs`
- `src/remote_ipc/ui.rs`
- `src/settings.rs`
- `src/settings_transfer.rs`
- `src/tag_ops.rs`
- `src/ui_adjustment_panel.rs`
- `src/ui_conceal.rs`
- `src/ui_crop.rs`
- `src/ui_dialogs/archive_convert.rs`
- `src/ui_dialogs/batch_convert.rs`
- `src/ui_dialogs/clipboard_capture.rs`
- `src/ui_dialogs/collections.rs`
- `src/ui_dialogs/context_menu.rs`
- `src/ui_dialogs/epub_convert.rs`
- `src/ui_dialogs/export_batch.rs`
- `src/ui_dialogs/file_organize.rs`
- `src/ui_dialogs/metadata_cleanup.rs`
- `src/ui_dialogs/metadata_transfer.rs`
- `src/ui_dialogs/new_folder.rs`
- `src/ui_dialogs/preferences.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_dialogs/preferences/search_index.rs`
- `src/ui_dialogs/rename_item.rs`
- `src/ui_dialogs/tag_apply.rs`
- `src/ui_dialogs/tag_editor.rs`
- `src/ui_dialogs/vst3_actions.rs`
- `src/ui_dialogs/whats_new.rs`
- `src/ui_erase.rs`
- `src/ui_fullscreen.rs`
- `src/ui_main.rs`
- `src/ui_metadata_panel.rs`
- `src/ui_music_panels.rs`
- `src/ui_sns_split.rs`
- `src/ui_text.rs`
- `src/ui_view_trim.rs`
- `src/undo_ops.rs`
- `src/video/mod.rs`
- `src/video/native_presenter/overlay_draw.rs`
- `src/video/native_presenter/render_core.rs`
- `tests/ui_snapshot.rs`
