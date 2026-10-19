# 更新後 初回起動の「重要な変更点」表示 実装計画

ステータス: **v2.0.0 で仕組み実装済み / v2.9.0 entry・選択方式移行を追加済み** (2026-07-30)。2026-06-19 起案。

## 実装状況 (2026-07-30)

- データ + 選択ロジック: [src/version_highlights.rs](../src/version_highlights.rs)。
  `VersionHighlights { version, must_read, highlights }` の exe 埋め込みテーブル `table()` +
  純関数 `highlights_to_show(prev, current, table)` (unit test 12 本) + ヘルプ再表示用 `for_version` +
  `latest_not_newer_than` + 描画 `render(ui, entries)` (App 非依存 → snapshot から呼べる)。
- ダイアログ: [src/ui_dialogs/whats_new.rs](../src/ui_dialogs/whats_new.rs)。⚠ 必読 → ・ 新機能 →
  「すべての変更を見る」(changelog.html) / 閉じる。display-only。
- トリガ: `App` 構築時に `settings.whats_new_acknowledged_version` と現行版で
  `highlights_to_show` を評価し、非空なら表示要求を保持する。利用者が閉じるまで未読を持ち越す。
  `last_seen_version` は起動時の移行・snapshot用の記録として独立に維持する。
- 開発用 `--whatsnew-from <ver>` で任意の前バージョンから強制表示 (`cli_flag_takes_value` 登録済)。
  ヘルプメニュー「重要な変更点を表示」で再表示。
- テスト: `highlights_to_show` の unit test + egui_kittest snapshot `whats_new_multi_version_dark`。
- **v2.0.0 の must_read = ツールバー右クリック化 + 左右クリック統一** (③ の告知)。Cargo.toml が
  2.0.0 になった初回起動で自動表示される。
- **v2.2.0 entry** として、`?` コンテキストヘルプ、キー割り当て表示の設定追従、
  環境設定「表示 → メニュー構成」、右ドラッグのマウスジェスチャ、サブフォルダ展開ビューを告知する。Cargo.toml が 2.2.0 に上がる前に entry を
  埋め込んでも、ヘルプメニューの再表示は現行版以下の最新 entry にだけフォールバックするため、
  開発中の 2.1.0 ビルドで v2.2.0 の告知を先取り表示しない。
- **v2.9.0 entry** は、一覧の通常クリックをエクスプローラー方式へ切り替えることを
  `must_read` で告知する。既存設定の一度きりの切替は新しい永続フラグを持たず、
  `highlights_to_show(previous_last_seen_version, current, table())` が返した集合にこの entry が
  含まれるかから直接導出する。初回移行時の告知と切替の範囲は一致し、未読告知の再表示では
  設定を再移行しない。v2.9.0 以後に
  ユーザーがチェック方式へ戻しても次回起動では上書きしない。

## 0. 背景と目的

標準動作が変わる変更 (例: ツールバー左右クリックの規約変更、
[toolbar-customization-plan.md](toolbar-customization-plan.md)) をユーザーへ確実に伝えたい。
前回のマウス進む/戻るのような**バージョンごとの個別ダイアログを増やすと破綻**するので、
**更新後の初回起動で、そのバージョンの重要な変更点 (主要部分) を 1 画面で表示する汎用の
仕組み**にする。

`update_check` ([src/update_check.rs](../src/update_check.rs)) とは**別物**:

| | update_check (既存) | 本機能 (新規) |
| --- | --- | --- |
| タイミング | 更新**前** (新版が出たと気づかせる) | 更新**後**の初回起動 |
| 取得元 | ネットワーク (GitHub Releases、本文 8KB cap) | **exe 埋め込み (オフライン)** |
| 内容 | 全文 changelog | **操作・既定の変更を中心に主要部分だけ** |
| 届く相手 | 更新通知を見た人 | インストーラ/ポータブルで黙って更新した人にも確実に |

## 1. 方針 (確定 2026-06-19)

- **スコープ = 主要部分のみ**。README の更新履歴は詳細なので、本画面は**操作・既定の変更を
  中心に短く**。末尾に「すべての変更を見る」→ マニュアルの changelog.html へのリンク。
- **無効化設定は付けない** (毎回、重要事項だけを出す前提)。あとから**ヘルプメニューで再表示**は可。
- **表示のみ (display-only) で統一**。インタラクティブな移行選択 (前回のマウス進む/戻るのような
  二択 UI) は**新規には設けない**:
  - 既定が変わるが**旧動作が設定で残る**場合 → 本文に**その設定の場所を明記** (「従来の動作は
    設定 > X で選べます」)。
  - **強制変更** (旧動作なし) → 新動作の説明だけ。
  - ※ 既にリリース済みのマウス進む/戻る移行 (v1.8.0) は**そのまま**。本方針は今後の変更に適用。
    - **2 ダイアログ並走の確認 (2026-06-19)**: マウス移行プロンプトは `mouse_nav_prompt_done`
      フラグで**一度きり** ([src/settings.rs](../src/settings.rs) `mouse_nav_upgrade_prompt_pending`、
      4041 付近)。一度答えれば以後どのバージョンでも出ない。よって本機能 ④ を載せた版に
      更新したとき、マウス案内 + ④ の**2 つが並ぶのは「v1.8.0 を一度も起動せず、それ以前から
      直接ジャンプした少数のユーザー」のみ** (v1.8.0 を通った大多数は ④ のみ)。一回限りで漸減
      するため**このまま許容**。④ は display-only でマウスの選択 (標準 vs 従来) の代替には
      ならないので、この少数ケースで対話プロンプトが別途必要なのは構造上やむを得ない。
      ④ の highlight テーブルに **v1.8.0 のマウス変更を重複して載せない** (対話プロンプトが担当)。
  - 利点: 画面が純粋な情報表示になり、§5 のテスト容易性がさらに高まる。

## 2. トリガと既読記録 (§1.232、2026-10-19)

- `settings.whats_new_acknowledged_version` から現行版までを `highlights_to_show` で選ぶ。
  `acknowledged == current` → 出さない。
- `acknowledged < current` → acknowledged より新しく current 以下の**全バージョンの highlight を累積**して
  1 画面に表示する (バージョンを飛ばした人も、途中で入った重要変更を見逃さない)。
- リリース済み設定に項目がない場合、`Settings::load` で
  `previous_last_seen_version.unwrap_or(current)` へ初期化し、起動版と同じ bootstrap save で
  その起動中に保存する。同じ版からの移行でも保存する。clean install は current となり表示しない。
- `whats_new_visible` を描画の早期 return と共通モーダル判定の両方で使う。
  表示要求があり、ビューアを開いていない場合だけ true (要求を作る入口でエントリの非空を保証する)。毎回既存状態を参照し、
  全画面・ウィンドウ内・別ウィンドウ (active が App 外に退避中の場合、他の still/media 窓も含む)
  の間は保留する。表示中だけ Esc / × / 閉じるを検出する。
  閉じるEscは消費し、同じpassで次に表示されるネットワーク通知へ渡さない。
- `startup_notice_viewer_open` が通知用の単一述語。detached の開閉・表示先・input routing は変えない。
  `fullscreen_idx` が空でも、既存の `fs_nav_deferred_reopen_wait_active()` が所有する
  PDF/ZIP 列挙、製本の projection/提示許可、アーカイブ変換、sidecar 復元の reopen 待ちは
  ビューア継続として扱う。detached は既存の生存要求と退避済み context/窓も読む。
  viewport の表示済みフラグ単独は閉じた後の cleanup まで残るので、保留条件にしない。
  再開予約の共通終点は `retire_viewer_reopen_requests(context)` とする。
  待機画面の Esc / × などの `finish_fullscreen_navigation_for_true_close()`、backend close が
  通る `close_fullscreen_to_completion()`、および `retire_context()` からここへ合流し、
  製本の deferred open を既存の permission 退役処理で退役させ、書庫・EPUB 変換の
  `deferred_fullscreen` も外す。EPUB ダイアログ自身の Abort 契約は変えない。
  製本の projection・変換処理と一覧の更新は継続できるが、遅着完了に再オープン予約を引き継がない。
  変換 state は App 共通なので、復帰予約には発行元の既存 `ViewerContextId` を保持する。
  真の終了ではその context が一致する予約だけ外し、別ビューアの close は元の予約を消費しない。
  context 退役成功時には予約も同時に外す規則とし、投影中の兄弟の navigation lock / holdover は触らない。
  直接読取り・変換キャッシュの完了は一覧採用後の同じ再開 helper を通り、発行元 context の
  residence が Mounted / AtRest（生存中）の場合だけ reopen を許可する。
  内部 close→reopen はこの終了処理へ通さず、従来どおり閲覧を継続する。
  マウス操作の移行案内と初回設定も main だけに描き共通モーダルに入る同型なので、
  それぞれの visible helper で同じ保留条件を使う。ネットワーク data_dir 通知にも適用し、
  先行の設定・移行・重要な変更点の要求が完了するまで待つ順序を維持する。
- 設定の保護起動案内は復元・終了を選ぶ別の契約なので変更しない。worker の通知は非モーダル、
  トレイ案内は起動要求ではなく利用者操作起点なので今回の起動通知保留には加えない。
  新版の自動更新確認はバッジだけで、結果ダイアログを開くのは手動確認/バッジクリック時だけ。
  改名ジャーナル復旧は lazy load の破損時の回復操作であり、隔離worker実行中もCASの排他を
  保つモーダルなので、起動時の情報通知と同じ保留条件へは変更しない。
- 簡素化: 通知を3表示先へ移さず、main に戻るまで表示と入力停止を一緒に保留する。
  latch・新しい非同期owner・detached遷移は不要。既読保存は既存の Settings 保存経路を使い、
  まれな保存失敗はログと通知で割り切り、再試行・復旧状態は追加しない。環境設定の古い draft から既読を巻き戻さず、
  環境設定の持ち運びからは内部記録として除外する。

### 2.1 終了入口の監査（§1.232 第4回レビュー、2026-10-20）

基準ソースは `48e2ddff4`。変更前に `src/**/*.rs` の close 系の全呼出し、
`fullscreen_idx` の None / take / 代入、`fs_viewport_shown=false`、
`fs_viewport_presentation=None`、`native_video_in_window_active=false`、
`viewer_presentation` の代入、detached session の close / take を機械抽出した。
コメント・文字列・`cfg(test)` の関数/module とテストファイルを除き、関数単位で下表に分類した。
機械抽出は重複パターン・後述の除外候補を含め 211 箇所、116 関数。
`rg -n 'close_fullscreen|close_all_detached_viewers|retire_context|fullscreen_idx|fs_viewport_shown|fs_viewport_presentation|viewer_presentation|active_detached_session' src`
でも候補を再抽出できる。比較式・新規 open の Some 代入・パネルだけの close は終了入口ではない。

抽出したうち、`enter_page_edit_single_view` / `leave_page_edit_single_view` /
`leave_local_page_edit_single_view` / `seek_to_continuous_page_with_contract` /
`adopt_shift_layout_without_navigation` / `reanchor_continuous_reading_viewer` /
`prepare_parked_live_audio_continuous_target` / `install_collection_navigation_root` /
`defer_native_video_source_swap_until_decoder_free` / `finish_viewer_index_swap` /
`open_fullscreen_with_metadata_boundary` は Some への再アンカー/open のみ。
`start_active_detached_loading_context` / `start_active_detached_collection_root_image` は初回表示設定、
`note_bookmark_media_open_wait` はローカル変数への読取りのみ。
`close_touch_owned_side_panels` / `close_touch_side_panels_for_file_change` /
`handle_native_video_output_event_inner` / `begin_metadata_panel_frame` / `draw_fs_music_right_panel`
の close は情報/調整パネルだけで、viewer session は終了しない。
`remove_active_context_for_test` は test 用 impl 内の関数なので本番入口に数えない。

T=真の終了、I=内容の差替え後に閲覧を継続、H=同じ reader の表示先/所有場所の移譲、
C=終了済み状態の表示 cleanup。表は修正後の接続先を記す。
通常の T は `close_fullscreen` → `finish_fullscreen_navigation_for_true_close` →
`retire_viewer_reopen_requests` へ寄せる。従来は通常終了と内部差替えが同じ関数を共有していたため、
I/H の低位 teardown を `close_fullscreen_content` として明示し、分類した内部 caller だけ移す。
metadata 境界を持つ終了、live return を後段 dispatcher へ渡す終了、context 退役は
通常 teardown で置き換えず、既存の終端から同じ予約所有者へ合流させる。

| ファイル・抽出された入口 / 状態を書き換える関数 | 分類・判定理由 | 終点 / 保持する経路 |
| --- | --- | --- |
| app: `set_ui_scale_factor`、preferences: `apply_preferences_dialog` のフォント変更、remote: `service_remote_session_requests` | T。一括終了。index は None でも viewer の待機がある | `close_all_viewer_sessions_with_cause` → detached 一括終了 + main の `close_fullscreen_to_completion` |
| app: `close_all_detached_viewers_for_mode_change` / `close_all_detached_viewers_with_cause` | T。detached 設定の変更は従来どおり対象窓だけ。専用全画面 main の待機も閉じる | 投影中の session 所有者（index 不在の待機も含む）を同じ述語で判定し `close_fullscreen_to_completion`、他 context は `close_and_retire_context`。App 共通 viewport flag の cleanup だけでは main の別要求を終了させない |
| app: `handle_fullscreen_close_request` / immediate dispatcher / `on_viewport_close_requested`、viewport_retirement: `accept_viewport_close_request` / `accept_active_detached_window_close` | T。Esc / Enter / HUD / OS close | 既存の finish / close-to-completion → 共通所有者。live return は finish 後に通常 navigation へ渡す |
| startup_list_restore: `close_fullscreen_to_page_list` / `close_main_still_fullscreen_for_tray`、ui_fullscreen: `handle_fs_navigation_with_render_perf` / `handle_video_input` | T。ページ一覧 / トレイへの退出 | 通常終了、または既存 finish → parent restore。waiting Esc は次行 |
| ui_fullscreen: `keep_fullscreen_viewport_alive` の待機 Esc / close、`render_embedded_fs_nav_holdover` | T。index 不在の待機を明示取消 | 既存 finish。Preparing lease は既存の exact-lease cancel を維持 |
| gamepad_input: `apply_ring_close_fullscreen`、native_video: `drain_native_video_source_swap_pending_events` / `dispatch_native_video_key_event` | T。ring / native のページ一覧戻り | `close_fullscreen_to_page_list` → 通常終了 |
| app: `close_detached_viewer_for_virtual_page_list_parent_nav`、`open_tag_view_for_tag`、`update_active_viewer_context` の Live return | T。閲覧ではなく一覧へ戻る | 通常終了。タグ遷移は main の待機も閉じ、detached sibling は閉じない |
| gamepad_input: `confirm_gamepad_favorite_picker`、`apply_ring_favorite_slot` / `apply_ring_drive_letter` / `apply_current_drive_root` / `apply_switch_drive_letter` / `apply_ring_location_action` | T。有効な新しい場所を受理済み。index がなくても旧閲覧は終了 | 通常終了。旧 index-only guard を外す |
| snapshot_ops: `snapshot_return_to_list_view`、filename_stack_ui: `poll_stack_grouping` | T。固定一覧 / 集約一覧へ戻す | 通常終了。旧 index-only guard を外す。park した sibling は保持 |
| app: `finish_everything_edit_boundary` | T。新しい Everything 一覧の受付。metadata Undo の扱いを維持 | finish → 必要な場合だけ既存 rating metadata teardown |
| app: `reconcile_fullscreen_after_main_focus` | 条件成立時だけ T。自動 focus close は実ページに対する既存 policy | 通常終了。待機中は自動 close 対象ではなく viewport を維持するため、index guard は残す |
| app: `zip_nav_dfs_fullscreen`、book_composition_permission: `remap_after_book_projection` / `poll_book_presentation_permission`、book_composition_runtime: `adopt_book_projection` | T。hidden / missing / 空の本で有限探索が尽きた | 通常終了。DFS の次の候補がある場合は I として継続 |
| app: `poll_video`、native_video: `poll_native_video_open_pending_with_readiness` / `poll_native_video_source_swap_pending_after_owner_gate` | T。native の close / host 消滅 / timeout | mounted は通常終了。parked は既存 window-id close → context 退役 |
| sidecar_restore: `poll_sidecar_restore_projected` の page-index worker 切断 | T。準備失敗で閲覧を終了 | 通常終了。index-only guard を外し、list/discard の処理は維持 |
| snapshot_ops: `prepare_viewer_index_swap` の key なし / member 不在 | T。新しい一覧で reader を解決できない | 通常終了。membership policy は既存 caller が判断 |
| app: `remove_items_batch` | 残存ページへ隣接移動するなら I、旧実ページが None になった場合だけ T | None の分岐は finish。index shift / cache の再整合は既存処理を維持 |
| app: `release_viewer_surfaces_for_removed_paths_with_metadata_boundary` | Manual は T。静止画 main は隣接再整合を維持し、表示/再生していない row の削除だけでは終了しない。BookAppend は handle 解放後に再採用する I | Manual は通常終了、BookAppend は既存 metadata teardown。detached は context 退役 |
| image_folder_zip_operation: `finish_image_zip_adoption`、collection_grid: `adopt_direct_collection_shell` の fullscreen-close-origin | T。成功後に親一覧 / collection 一覧へ戻す | 通常終了。後続 scan / 一覧採用は継続 |
| rating_membership_ops: `adopt_rating_membership` の Empty | T。評価隣接先もない | finish → rating metadata teardown（Undo 方針を通常 close に変えない） |
| app: `take_and_close_active_context_with_cause` / `close_current_active_viewer_context` / `teardown_paused_media_bundles_with_cause`、registry: `close_and_retire_context` / `retire_context` | T。mounted / at-rest / passive reader の寿命終了 | 既存 context 終端（`begin_retire` 成功後）→ 共通所有者 |
| app: `park_and_close_current_active_detached_viewer_inner` | park 成功は H。snapshot が作れない fallback だけ T | park 成功は保持、fallback は通常終了 / context 退役 |
| app: `terminate_active_detached_open_before_viewport` / `cancel_detached_initial_open_for_lease` / `cancel_detached_grid_archive_open_owner`、startup_ops: `cancel_bookmark_open_request` | T。初回準備の取消 / 失敗 | 既存 exact owner / lease の cancel → context 終端。別 lease の完了は採用しない |
| app: `close_active_detached_session_exact`、native_video: `execute_video_presentation_host_effect` の TerminalSessionClose | T の effect の実行、または I の遅延 content teardown | T の発行元は通常終了 / close-to-completion。metadata / I は既存 typed effect を保持。session/output retirement だけで別の予約を消さない |
| app: `load_zip_as_folder_with_prepared_enumeration` / `handle_grid_parent_folder_action` の preserve 後 close、`change_main_context_for_visible_grid_with_metadata_boundary`、smart_folder: `install_adopted_smart_root_payload` | I/H。一覧/世代の採用に使う共通 teardown。root の独立 navigation は既存 admission が旧 open owner を cancel し、folder-nav は新しい継続 owner を保持する | 低位 close を維持。ここから finish を呼ぶと新しい reopen を取り消すため、T と一括置換しない |
| app: `commit_physical_history_transition` / `apply_folder_nav_result`、collection_navigation: `install` / `install_collection_outer_physical_adoption` / `commit_collection_navigation`、snapshot_ops: `prepare_required_fullscreen_navigation` | I。既存 typed navigation が次の閲覧を所有 | `close_fullscreen_for_folder_nav_reopen` → 低位 close。予約保持 |
| snapshot_ops: `snapshot_load_and_open`、rating_membership_ops: `prepare_rating_membership_items_swap` / `adopt_rating_membership` の neighbor、app: `close_fullscreen_for_rating_membership` | I。exact target / metadata / rating landing を再採用 | 既存 content teardown と caller の再開を維持 |
| app: `park_detached_session_for_stack_aggregation` / `park_current_viewer_context_as_live_media_inner` / `park_active_detached_context_as_live_media` / `promote_active_detached_video_for_main_context_change` | H。index=None は App 投影を空にするため。reader は移譲先で生存 | context ID / bundle / passive window を保持。予約を退役しない |
| app: `handoff_active_detached_viewport_to_passive` / `finish_active_detached_session_handoff`、registry: `activate_mounted_as_independent_detached` / `become_mounted_independent_detached_viewer` | H。active designation / native in-window flag を外すが窓/reader は維持 | 既存の移譲。terminal と誤認しない |
| app: `prepare_viewer_presentation_open` / `apply_egui_viewer_presentation_for_current_item`、native_video: `toggle_still_window_mode` / `apply_video_presentation_switched` | H / open。F12、全画面↔ウィンドウ内、メディア表示切替 | 同じ閲覧を保持。index-only 判定の変更なし |
| app: `retire_terminal_detached_viewport_identity` / `begin_active_detached_session_close` / `finish_active_detached_session_close` / `finalize_closed_active_detached_viewport`、`update_active_viewer_context` の finalized cleanup | C。上流の T / context 終端、または H が所有判断済み | output / registration の破棄だけ。二重の別 context 取消を追加しない |
| app: `reset_active_detached_viewport_runtime_for_new_window`、ui_fullscreen: `hide_native_video_black_backdrop_if_shown` / `hide_embedded_still_viewport_if_shown` / `hide_current_fullscreen_viewport_for_recreate` | H/C。backdrop を外す / root へ移す / viewport を再作成するだけ | viewer の予約を維持。fs_viewport_shown=false を終了述語にしない |
| app: `prepare_viewer_presentation_close` / `close_fullscreen_now` / `close_fullscreen_now_with_metadata_boundary` | T/I/H 共通の最下位内容 teardown。index=None の実書込元 | 退役判断は上流。内部 reopen も通るため、ここには finish を足さない |
| startup_ops: `open_bookmark_media_in_detached_context` の target-not-opened | 初回 build の失敗 T | build abort が一時 bundle / 予約を drop。stable context の強制 mount はしない |
| update_phases: `input_test_script_actions` の CloseFullscreen | T（test-script の明示操作） | 既存 close-to-completion。`prepare_test_script_run` は変更しない |
| App の `on_exit` / `on_exit_inner` / Drop | プロセス寿命の T。通常の viewport close と異なり以後の UI poll がない | 既存の全 owner shutdown / Drop。揮発予約は次回へ保存されない。保存/flush/worker 停止の順序を変えないため、通常の content teardown へ寄せない |

検出した index-only の取りこぼしは、専用全画面の一括 close、倍率の fallback、
フォントの fallback、Remote の mounted close、上表の場所アクション、snapshot 一覧戻り、
集約一覧採用、sidecar worker 失敗。倍率/フォント/Remote は一つの全 session 終了 API へ寄せる。
detached 表示設定だけの変更は、開いていない embedded viewer まで新たに閉じる変更にしない。
focus 自動 close / tray の入口 policy、I/H の実ページだけを操作する index 判定は、
待機中に viewport を閉じていないので取りこぼしとは区別して維持する。

main の終了対象と通知の保留は同じ `projected_viewer_session_open` を読む。
index / 既存 navigation・製本・変換の再開所有者 / native の準備・遷移を数え、
空の viewport cleanup や一覧だけの PDF password / 列挙では main の終了を新たに発生させない。
active detached の退役後も App 共通 viewport flag が残る場合があるため、一括終了内部でも
この所有述語で投影中 reader の有無を判定する。兄弟の窓を閉じるだけで main の一覧要求を消費しない。
App 共通の書庫変換予約は `owner_context` が現在の投影 context と一致する場合だけ数える。
設定変更は一旦全 viewer を閉じる既存方針を維持し、倍率やフォントを生存中の各窓へ伝搬する
新しい待機状態は増やさない。終了しても変換・製本 projection の一覧採用は続ける。
一括終了中の native 表示遷移も、直接 effect を drain する代わりに close-to-completion を使う。
content teardown が先に index / presentation を解除して後段の終了判定をすり抜けないよう、
終了要求の受付で予約退役を完了する。

回帰は ZIP/PDF 列挙、製本 projection / 提示許可、書庫変換 / 直接読取りに対し、
実際の倍率変更（専用全画面・ウィンドウ内・detached）、detached 設定の一括終了、
フォント/Remote が使う全 session 終了 API、通常 close・ページ一覧・Everything・tray 終了を駆動する。
既存の backend close / context 退役 / 内部 reopen の保持・兄弟 context の保持も併せて確認する。
PDF は実 coordinator の request ID を保った完了、ZIP は退役 receiver への成功送信、
製本/書庫は閉じた後の成功採用を使い、通知復帰と再オープンしないことを固定する。

検証記録（2026-10-20）:

| 対象 | 回帰と結果 |
| --- | --- |
| ZIP/PDF の遅着列挙 | `startup_notices_settings_and_list_exits_retire_enumeration_before_late_success`。12 種類の終了入口で receiver / 待ち手の退役、通知復帰、遅着成功が採用されないことを確認 |
| 製本 projection / 提示許可 | `startup_notices_book_wait_escape_and_close_retire_reopen_before_late_completion`。既存 backend / context / Esc に同じ12入口を追加。projection は一覧へ採用し、閉じた context の返信は受信口ごと終わる |
| 書庫変換 / 直接読取り | `startup_notices_archive_wait_escape_and_close_keep_work_but_retire_late_reopen`。同じ12入口と既存終了入口。変換を止めず成功後に一覧採用するが reader は開かない。内部 reopen と兄弟予約保持も確認。兄弟 Esc は実際の detached viewport の入力として駆動 |
| EPUB / 一覧だけの PDF | `startup_notices_epub_settings_close_keeps_conversion_but_retires_late_reopen`、`startup_notices_bulk_settings_close_preserves_list_only_password_request`。変換本体・一覧採用と、viewer ではない password request の保持を確認。別窓がない場合に加え、main の password と active detached が併存する場合も後者だけ終了 |
| 既存の終了・失敗・所有移譲 | close-return / viewport retirement / context registry / native source swap・park / rating membership / 削除・snapshot / folder navigation / sidecar の既存回帰を併走 |

先行の `test(/startup_notices/)` 単独確認は **18 passed**。
併存ケース追補後の最終ソースで、同じ18件を含む以下の関連範囲は **1,508 passed**
（run `b5463cdc-b9d8-4896-a983-03a69ba028f7`、91.186秒）。
本体 lib 全件はこの実装作業では実行せず、ClaudeCode の最終 gate に残す。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/startup_notices|book_composition|archive_convert|epub_convert|close|retire|ui_scale|snapshot.*swap|rating_membership|remove_items|removed_path|fs_nav|folder_nav|sidecar_restore::tests|smart.*(fullscreen|navigation)|collection.*navigation|viewport_retirement|viewer_context_registry::tests|park/)'
```

`cargo fmt`（workspace 全体）/ `cargo fmt --check`、UI glyph 検査、`git diff --check` は通過。
`scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は core / remote / EPUB worker の
生成と VCRT/PE 検査まで成功（runtime=4、pe=3）。製品バイナリは起動していない。
表示内容/レイアウトは変更せず snapshot PNG の更新はない。マニュアルの既存 changelog にある
「ビューア中は保留し、閉じてから表示」は今回の挙動と一致するため変更しない。
Windows 実機での表示・OS close の最終確認は未実施。

## 3. データ (exe 埋め込み)

- バージョン別 highlight を**ソースに埋め込む** (オフライン・バイナリ一致)。構造例:

  ```rust
  struct VersionHighlights {
      version: semver::Version,
      must_read: Vec<HighlightItem>,   // ⚠ 操作・既定の変更 (必読)
      highlights: Vec<HighlightItem>,  // 主な新機能 (任意)
  }
  struct HighlightItem { title: String, body: String } // 短文。内部用語は出さない
  ```

- `update_check` のネットワーク changelog とは別管理 (役割が違う)。
- authoring はリリース手順に 1 ステップ追加 (§9)。

## 4. 画面

- `egui::Window` + スクロール可能リスト (既存ダイアログの idiom)。
- 2 段構成: **⚠ 操作・既定の変更 (必読)** → **主な新機能 (任意)** → 「すべての変更を見る」リンク。
- バージョンをまたぐ場合はバージョン見出しごとに項目を並べる。
- 閉じるボタン / タイトル × / Esc のいずれかで利用者が閉じたときだけ
  `whats_new_acknowledged_version = current` にして保存する。「すべての変更を見る」は閉じず既読にしない。
  `--whatsnew-from` は表示範囲だけを強制し、閉じたときは同じく current を記録する。
  `prepare_test_script_run` は要求をメモリで落とすだけで、既読を保存しない。
- ヘルプからの再表示は既存の現行版/最新entryへのfallbackを維持する。保留中の要求があれば
  その累積/強制表示範囲を保持し、最新版だけへの置換で未読を取りこぼさない。

### 4.1 小画面での本文幅と操作行（2026-10-09 修正）

- `ctx.content_rect()` とリサイズ後の利用可能高さから本文領域を確保し、見出しと
  「すべての変更を見る」「閉じる」は縦スクロールの外に置く。短文の自然高、
  ウィンドウの移動・リサイズ、タイトルバーの × は既存の所有経路を維持する。
- 本文は縦レイアウトの一つの折り返し `Label` として割り当てる。先頭行の14pt字下げと
  最小行高は文字レイアウトに設定し、折り返し幅を超える行矩形を親へ個別登録しない。
  これは本文の利用可能幅を所有する境界での修正であり、告知文ごとの幅指定や待ち処理は不要。
- 初期の読み幅は従来snapshotと同じ外枠650ptとし、本文長による自動拡大へ依存させない。
  `Window::default_width` には、650ptを現在の表示領域幅で制限し整数ptへ内側に丸め、
  実際の枠・余白を引いた本文幅を渡す。200%でも表示領域に収まれば650ptを維持する。本文はそのWindowの
  利用可能幅内で折り返し、以後の移動・手動リサイズ・画面縮小は既存のWindow所有とする。
  既存の最小幅440ptは変更せず、利用者の手動リサイズを650ptで禁止しない。
- 原因: 従来の `horizontal_wrapped` 内の `Label` は各行の矩形を登録する。
  eguiのgalley全体には折り返し幅を超えない丸め補正があるが、個別の行矩形には同じ上限が
  適用されない（`vendor/egui/src/widgets/label.rs` の水平折り返し分岐）。実データでは
  各項目で0.5〜1ptの超過が積み重なり、横スクロールなしの `ScrollArea` が内容幅へ拡大、
  `Window` の `Resize` がその幅を次フレームへ戻すため、タイトル × も移動した。
  v4.5.0の告知追加で顕在化したmaster由来の不具合で、告知データ自体は変更しない。

## 5. テスト容易性 (← 最重要。実機テストを最小化する設計)

「複数バージョンまたぎは実機で再現しにくい / 実機に時間をかけられない」懸念に対し、
リスクをほぼ全部 CI 側へ寄せる:

- **選択ロジックを純関数化**: `highlights_to_show(prev: Option<&Version>, current: &Version,
  table: &[VersionHighlights]) -> Vec<&VersionHighlights>`。**またぎ累積の核心**をここに集約し、
  unit test で網羅 (None / 同一 / 1 段 / 多段スキップ / 降格 / parse 不能 / 空テーブル)。**実機ゼロ**。
- **fail-safe**: parse 不能や空でも**黙ってスキップ** (起動を絶対止めない)。`update_check` の
  silent-fail と同じ哲学。unit test で担保。
- **描画は egui_kittest スナップショット**: 合成の多バージョン payload を食わせて
  [tests/ui_snapshot.rs](../tests/ui_snapshot.rs) に 1 本追加。レイアウト崩れを**実機なし**で検知。
- **開発用の強制表示フラグ** `--whatsnew-from <version>` (または env): 任意の「前バージョン」から
  画面を即表示。**ダウングレード/再インストール不要**で、どのまたぎ組合せも目視できる。
- 状態遷移・実DBテスト: 3モードの表示/入力停止保留、同じイベント列のEsc消費、閉じたときの保存、
  閉じずに再起動/版飛ばし、項目なしの移行と即時保存、clean install、通知順序を固定する。
  実機では引数起動の3モードでキー・右クリックが使え、ビューアを閉じると通知が出ることを確認する。
- **本文幅の回帰**: 実フォントで複数の本文幅とUI倍率100%/200%を通し、描画内容が親の横幅を
  広げないことを検査する。現行の全告知に加え、さらに長い必読・新機能の見出しと本文を表示し、
  1093×614 / 1366×728の往復リサイズ後も、連続フレームでウィンドウとタイトル ×・固定操作行の
  矩形が変わらず、hoverと実ポインタイベントでクリックできることを検査する。
  両画面サイズの初期幅も100%/200%で検査し、表示領域に収まれば外枠650pt、収まらなければ
  表示領域幅を整数ptへ内側に丸めた幅まで確保され、内容の都合で約450ptへ縮まないことを確認する。
  既存の小画面snapshot/クリック検査の条件は緩めない。

## 6. 既存資産との接続

| 必要な処理 | 流用元 |
| --- | --- |
| 未読範囲 / 起動時移行 | `whats_new_acknowledged_version` / 初回だけ `previous_last_seen_version` ([src/settings.rs](../src/settings.rs)) |
| ダイアログ (Window + ScrollArea) | 既存ダイアログ ([src/ui_dialogs/update_notice.rs](../src/ui_dialogs/update_notice.rs) ほか) の idiom |
| ヘルプメニューからの再表示 | 既存メニュー構造 |
| 「すべての変更を見る」リンク | マニュアル changelog.html ([src/external_links.rs](../src/external_links.rs)) |

## 7. 他計画との関係

- **ツールバー左右クリック規約の変更告知**はこの仕組みで行う
  ([toolbar-customization-plan.md](toolbar-customization-plan.md))。強制変更なので display-only で
  「新しい規約の説明 + カスタマイズは右クリック/⚙ から」を出す。
- 今後の標準動作変更は全部この器に集約 (個別ダイアログを増やさない)。

## 8. 永続データへの影響
- 追加ストアなし。既存 `settings.db` の汎用KVに `whats_new_acknowledged_version` を追加する。
  欠落項目の初期化・即時保存は §2。highlight は引き続きコード埋め込み。

## 9. リリース手順への追加
- [CLAUDE.md] のリリースチェックリスト (Phase 1 付近) に「version highlight テーブルに
  このバージョンの主要/操作変更を追記する」を 1 ステップ追加 (authoring 忘れ防止)。

## 10. ドキュメント同時更新 (実装時)
- [spec.md](spec.md) / [architecture-overview.md](architecture-overview.md) /
  htdocs マニュアル (ヘルプから再表示できる旨。内部用語を出さない方針)。

## 11. §1.232 の実装・検証記録 (2026-10-19)

- `next6-notice`、起点 `be318299f` の未コミット差分。App状態/実入力handler/Settings実DBの
  回帰15件を追加し、既存回帰と合わせた焦点テスト82件、設定backup関連9件がすべて成功。
- `ui_snapshot` の変更点・ネットワーク通知・小画面・画面サイズ変更の4件が成功。
  描画本文/レイアウトは変更せず、期待PNGの追加・更新はない。
- 初回のリンク操作テストは日本語フォント未設定で失敗し、製品のfont helperを設定して修正した。
  上記82件は修正後の結果。本体lib全件は今回の実装担当では実行せず、ClaudeCodeの最終gateへ引き継ぐ。
- ソースhash、command、nextest run ID、件数、環境条件は `target/notice-verification.json` に記録。
  native依存物の並列CMakeが失敗する環境のため、ビルドは
  `MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1` を使用する。
- `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` が成功し、通常featureの
  core / Remote / EPUB変換器と依存DLLを配置済み。VC runtime/PE検査も `runtime=4 pe=3` で成功。
  `cargo fmt` / `cargo fmt --check` / UIグリフ検査 / `git diff --check` も成功。
  製品exeの起動・実機入力検証・独立レビューは未実施。
  引数起動の3モードでキー/右クリックが使え、ビューアを閉じると通知が出る実機確認を残す。
