# §1.317 環境設定のエクスポート・インポート設計・実装記録

作成: 2026-10-04。対象: `next-file-ops` / `C:\home\mimageviewer-fileops`。
状態: **設定メニュー「設定の復元…」への入口移動と、設計担当の案 2 に従った同時表示時の未確定 draft 保護は実装済み (§8.6)。独立レビューで指摘された背面の操作カスタマイズのキー捕捉も修正し、限定独立再レビュー・自動検証・確認用 build は完了 (§8.7)。旧結果は §8 に履歴として保持する。今回の実機確認・検収は未完了**。
要件の正本は [next-release-backlog.md §1.317](next-release-backlog.md)。
前段 `e189fbe86` (§1.256)、`c608f0822` (§1.263) を含むツリーで調査した。
設計段階では本書だけを作成し、製品コード・他文書・実装テストは変更していなかった。
以下はその調査・判断の記録を保持し、2026-10-04 の利用者承認と現在の実装範囲を反映したもの。
§7 の推奨案はすべて承認済み。追加で `details_selection_bar_mode` と
`stack_script_enabled` を除外した。既存 OK の §1.305 / §1.295 は今回修正しない (§2.3)。
検証結果は実施後に追記し、計画の列挙を実施済みの証拠として扱わない。

## 1. 現状のコード事実と採用する境界

| 根拠 (調査時の file:line) | 事実 / 設計への意味 |
| --- | --- |
| `src/settings.rs:4115`、`:5702` | Settings は pub(crate) を含む 432 フィールド。利用データ、パス、互換 carrier、runtime 状態が混在する。Settings 全体の JSON 化・DB 複製は転送形式に使わない。 |
| `src/settings.rs:4265`、`src/ui_dialogs/preferences/pages.rs:8632` | ファイル整理先は環境設定で編集するが名前・パス・ID を持つ PC 固有情報。全体を除外する。 |
| `src/settings.rs:7145`、`src/settings_db.rs:20` | Default / serde(default) は欠落を既定値で補う。今回の「欠落は現在値を保持」と異なるので、ファイルを Settings へ deserialize しない。 |
| `src/settings_db.rs:48`、`:892`、`:937`、`:954` | DB schema family は 1。save_full は複合値と KV を既存 transaction に保存する。転送ファイル版は DB schema と独立。新テーブル・DB migration は不要。 |
| `src/ui_dialogs/preferences.rs:722`、`:1278`、`src/settings.rs:8382` | PreferencesState.settings が draft。生成時は preferences_snapshot() が、お気に入り overlay を外した共通設定を渡す。取り込み先はこの draft。書き出し元は確定済み Settings の preferences_snapshot() とする。 |
| `src/ui_dialogs/preferences.rs:1892`、`:1944`、`:1977`、`:2473`、`:2617` | OK は prepare_preferences_state_settings_for_commit → install_preferences_settings → 既存副作用 → save_checked。runtime / 利用データの移送、共通表示値の routing、保存失敗の通知を再利用する。 |
| `src/settings.rs:9692`、`src/ui_dialogs/preferences.rs:1898` | overwrite_non_preferences_from は環境設定外の live 値を引き継ぐ関数。転送対象判定ではない。標準表示値は呼出前に退避し route_preferences_view_state へ通る。 |
| `src/ui_dialogs/preferences.rs:1959`、`src/settings.rs:8393` | 再生位置・音声トラックは最新 live map を維持する。お気に入り専用の表示値を維持し、draft の表示値だけを標準へ反映する。import が map や clear_requested を触る必要はない。 |
| `src/ui_dialogs/preferences.rs:2603` | reading_history_limit の変更で prune が走る。履歴保持の要件に合わせ保持件数は初期対象から外す (§7)。 |
| `src/ui_dialogs/preferences.rs:2323`、`:2333` | OK はフォント準備、LUT コピー、整理先の検証も待つ。import 後もこの判定を維持する。 |
| `src/operation_customize_share.rs:22`、`:26`、`:57`、`:93`、`:141` | 既存共有は JSON / format / format_version と、keymap、ring_shortcuts、通常・右クリックメニュー、gamepad_enabled を転送する。環境設定内に同じ項目があっても今回には含めない。版相違警告は参考にするが欠落の既定補完は流用しない。 |
| `src/ui_dialogs/settings_restore.rs:1221`、`:1269`、`:1671`、`:1717` | 既存共有は rfd の JSON ファイル選択、worker で read/parse/write、メッセージ欄、取り込み確認を使う。形を揃える。世代選択・比較・取り込み前バックアップ・即時適用は持ち込まない。 |
| `src/ui_dialogs/preferences/pages.rs:33`、`:91`、`:128`、`:1448` | 全体設定・ビューワモード・サムネイルのカテゴリ表示順は環境設定で変更する。sort_order (名前・日付等) と grid_display_order (カテゴリの行構成) は別。後者は対象。 |
| `src/ui_dialogs/preferences/pages.rs:8960` | exif_hidden_tags は任意文字列のカスタム追加もある。Vec<String> を無条件転送すると個人情報・パスなしを保証できない。初期範囲では全体除外を推奨 (§7)。 |
| `src/settings.rs:6218`、`:8436`、`:8447`、`:8478` | バー固定の owner は BottomBarLock。静止画の列固定は列表示も含意。動画の固定 ON は非表示のストリップを last_choice で復元し、環境設定外の状態も変える。動画下部固定 2 フィールドは初期除外を推奨 (§7)。 |
| `src/ui_dialogs/preferences/pages.rs:7726`、`src/settings.rs:10000` | video_loop / archive_convert_without_dialog は現在の enum から導出する互換値。独立項目として転送しない。 |

読み合わせた運用根拠: `CLAUDE.md:111` (状態の組合せ削減)、`:132` (まれな失敗)、
`:509` (IME)、`:1722` (製品ページと privacy の照合)、`:1739` (マニュアル)、
`:1786` (出荷済み形式の互換)、`docs/architecture-overview.md` の永続化ストア一覧、
`docs/spec.md` §8、`docs/preferences-layout-guidelines.md`、`docs/ui-responsiveness.md` §4、
`docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`。

採用案は **小さい純粋な変換モジュール + SettingsRestoreState 所有のファイル処理**。
書き出しは確定済み設定を使う。取り込みの検証成功後に環境設定の全体設定を開き、
対象値だけを draft へ一度に載せ、画面で確認して OK。キャンセルなら draft を破棄する。
直接 Settings / SQLite / 他 DB を更新する案は、確認できず新しい確定経路も要るため採用しない。
既存 OK の viewer 終了・再表示、一覧再読込、native presenter 更新を使い、
import 専用の live rebuild や detached 述語・viewport 経路を新設しない。
それらの修正が必要になれば detached-rework-plan §2 の合意・§11 記録を別途行う。

## 2. 全フィールドの分類と分類漏れ防止

### 2.1 分類表 (この調査時点の全 432 フィールド)

同じ判定・根拠の項目をグループ化した。各セルは **実名の列挙** であり、prefix で将来の
フィールドを自動分類する仕様ではない。各行の定義は先頭フィールドの位置。
「出す」は §3 の検証に通る値だけ、「除く」は入出力両方で対象外。
除外は移行先を初期化する意味ではない。実装時の唯一の実行分類表は §2.2 の policy。
本表はその初期仕様で、別の allowlist / denylist を手書きして二重管理しない。

| 判定 | フィールド (Rust 名) | 理由 / UI の根拠 | 定義 |
| --- | --- | --- | --- |
| 出す | `ui_theme`, `text_contrast`, `ai_feature_mode` | 全体設定のテーマ・文字と AI 利用範囲 (pages.rs:33 / 57 / 91)。性能の手動 tuning とは分ける。 | `src/settings.rs:5047` |
| 出す | `detached_viewer_open_images_in_window`, `auto_fullscreen_zip_pdf`, `auto_fullscreen_image_folders`, `fullfeature_media_window` | 全体設定の閲覧モード (pages.rs:128)。実効値ではなく保存された選択値を出す。 | `src/settings.rs:5625` |
| 出す | `restore_last_cursor`, `startup_window_state` | 起動時の振舞いだけ (pages.rs:409 / 476)。実際の場所・座標は出さない。 | `src/settings.rs:4288` |
| 出す | `grid_click_selection_mode`, `grid_open_selected_item_on_click`, `grid_cursor_wrap`, `remember_favorite_view_state`, `grid_display_order`, `video_thumbnail_indicator`, `thumb_show_media_duration`, `thumb_show_resume_meter`, `selection_info_display_mode` | 表示→サムネイルの操作・情報表示 (pages.rs:1362–1641)。カテゴリの行構成は環境設定内、名前等のソートは対象外。 | `src/settings.rs:4121` |
| 出す | `thumb_tooltip_show_filename`, `thumb_tooltip_show_image_dimensions`, `thumb_tooltip_show_video_duration`, `thumb_tooltip_show_kind`, `thumb_tooltip_show_page_count`, `thumb_tooltip_show_file_size`, `thumb_tooltip_show_modified`, `thumb_tooltip_show_created`, `thumb_tooltip_show_video_dimensions`, `thumb_tooltip_show_video_codec`, `thumb_tooltip_show_location`, `thumb_tooltip_show_full_location` | 表示項目の bool のみ (pages.rs:1581)。名前・場所・履歴の実データを含めない。 | `src/settings.rs:4500` |
| 出す | `thumb_tooltip_show_reading_history_last_read`, `thumb_tooltip_show_reading_history_progress` | 同上。閲覧履歴の内容ではなく表示するかどうか。 | `src/settings.rs:4536` |
| 出す | `show_windows_context_menu_inline`, `skip_recycle_bin_delete_confirmation` | エクスプローラ連携の表示・削除確認方針 (pages.rs:1194 / 1329)。Shell 登録そのものは移さない。 | `src/settings.rs:4660` |
| 出す | `rating_sort_unrated_position`, `slideshow_interval_secs`, `slideshow_continuous_wait_secs`, `slideshow_continuous_scroll_secs`, `slideshow_continuous_scroll_percent`, `slideshow_end_action` | 未評価位置・スライドショー方針 (pages.rs:1427 / 1652)。評価値やソート選択とは別。 | `src/settings.rs:4357` |
| 出す | `capture_format`, `bake_stage_book`, `bake_stage_export`, `bake_stage_export_batch`, `bake_stage_external_tool` | キャプチャ形式・各出力の焼き込み方針 (pages.rs:1753 / 1845)。ツール登録、画像、保存先を含めない。 | `src/settings.rs:4725` |
| 出す | `archive_file_handling`, `epub_file_handling`, `show_hidden_files`, `folder_thumb_sort`, `folder_thumb_depth`, `folder_skip_limit`, `edit_restore_prompt_enabled`, `sidecar_backup_enabled`, `tag_sidecar_backup_enabled`, `skip_zip_if_folder_exists`, `skip_archive_if_zip_exists` | ファイル処理・代表選定・バックアップの方針 (pages.rs:7286 / 8637 / 8820)。実データ・パスを含めない。 | `src/settings.rs:4442` |
| 出す | `skip_epub_if_pdf_exists`, `skip_image_if_video_exists`, `skip_duplicate_images`, `image_ext_priority` | 同名ファイルの扱いと拡張子の優先順 (pages.rs:8837 / 8868)。拡張子列は組込み候補だけ (§3)。 | `src/settings.rs:4690` |
| 出す | `minimize_to_tray_on_close`, `pause_indexer_while_minimized`, `write_rating_to_xmp`, `reading_history_enabled` | 常駐・索引一時停止・記録方針 (pages.rs:7444 / 7463 / 7503 / 10149)。保持件数・全件クリアを含めない。 | `src/settings.rs:5344` |
| 出す | `default_spread_mode`, `follow_document_reading_direction`, `default_reading_flow`, `default_reading_direction`, `final_cover_spread_enabled`, `singleton_spread_first_enabled`, `singleton_spread_last_enabled`, `page_after_cover_alone_enabled`, `last_page_alone_enabled`, `spread_page_gap_px`, `continuous_reading_gap_px`, `fullscreen_image_margin_color` | 表示→閲覧表示の標準設定 (pages.rs:9405–9795)。本別設定・読書位置・補正を含めない。 | `src/settings.rs:4851` |
| 出す | `fullscreen_fit_mode`, `fullscreen_fit_no_upscale`, `fullscreen_fit_no_downscale`, `fullscreen_side_panel_mode`, `fullscreen_boundary_notice_visible`, `fullscreen_processing_status_visible`, `fullscreen_prefetch_status_visible`, `panorama_projection`, `fullscreen_top_bar_locked`, `fullscreen_fixed_bar_gap_px`, `fullscreen_seek_direction`, `fullscreen_horizontal_cursor_direction` | 同上。表示モード・クローム・カーソル方向。 | `src/settings.rs:4887` |
| 出す | `fullscreen_page_number_overlay`, `fullscreen_keep_on_app_switch`, `fullscreen_cursor_hide_delay_secs`, `fullscreen_jump_mode`, `fullscreen_jump_percent`, `fullscreen_fixed_jump_count`, `continuous_reading_wheel_scroll_percent`, `continuous_reading_key_scroll_percent`, `continuous_reading_gamepad_scroll_percent_per_sec` | 同上。表示・ジャンプ・連結スクロール量。 | `src/settings.rs:4991` |
| 出す | `fullscreen_seek_bar_locked`, `still_seek_strip_locked`, `still_seek_strip_visible`, `still_seek_strip_height`, `still_seek_strip_height_values`, `still_seek_preview_size`, `still_seek_preview_size_values`, `still_seek_hover_preview_mode`, `still_seek_bar_with_strip` | 静止画シーク UI (pages.rs:9324 / 9564)。先頭 3 フィールドは論理項目 still_bottom_chrome にまとめる (§3)。 | `src/settings.rs:4938` |
| 出す | `video_volume`, `video_seek_small_secs`, `video_seek_medium_secs`, `video_seek_large_secs`, `video_seek_thumbnail_tolerance_secs`, `video_seek_strip_min_interval_secs`, `video_seek_strip_waveform_span_secs`, `video_seek_strip_height`, `video_seek_preview_size`, `video_seek_preview_size_values`, `video_seek_strip_height_values`, `video_seek_strip_cycle` | 動画ページの表示・再生方針 (pages.rs:7594–7917 / 9140)。音量はアプリ内 gain、OS 音声デバイスの設定ではない。 | `src/settings.rs:5433` |
| 出す | `video_top_bar_locked`, `video_seek_hover_preview_mode`, `video_seek_bar_with_strip`, `video_loop_mode`, `video_start_muted`, `video_thumb_use_sidecar_image` | 同上。ループ旧 bool は導出値。バー/プレビュー表示の選択値だけ。 | `src/settings.rs:5486` |
| 出す | `video_grid_open_starts_from_beginning`, `video_nav_resume`, `book_open_resume`, `book_nav_resume`, `music_open_resume`, `music_nav_resume` | 履歴と復元の 6 セル (pages.rs:10023)。前回位置ではなく復元するかどうかの方針。 | `src/settings.rs:5563` |
| 除く | `startup_folder_mode`, `startup_folder_path`, `capture_output_dir`, `book_root`, `export_last_directory`, `export_batch_directory`, `file_organize_destinations` | 起動・保存・整理先の PC 固有パス。startup_folder_mode も Specific とパスが一組なので保持する。 | `src/settings.rs:4280` |
| 除く | `ui_font`, `external_tools`, `creative_luts`, `susie_enabled`, `susie_allow_parallel`, `vst3_enabled`, `vst3_plugins`, `vst3_plugin_path`, `vst3_plugin_state`, `vst3_chain_slots` | フォント / ツール / LUT / Susie / VST の実ファイル・導入状態・任意 state に依存。ON/OFF や微調整も一組で保持。 | `src/settings.rs:5059` |
| 除く | `parallelism`, `pdf_worker_count`, `prefetch_back`, `prefetch_forward`, `thumb_prev_pages`, `thumb_next_pages`, `gpu_memory_percent`, `ai_upscale_prefetch_back`, `ai_upscale_prefetch_forward`, `retained_final_ai_cache_max_entries`, `retained_final_ai_cache_max_mib`, `ai_upscale_skip_px` | CPU/GPU/メモリ/ドライバ/速度の tuning。移行先で設定する (§7)。 | `src/settings.rs:4334` |
| 除く | `ai_denoise_skip_px`, `ai_upscale_size_limit`, `ai_denoise_size_limit`, `ai_backend`, `video_hw_decode`, `anime_upscale_source_limit`, `indexer_speed_profile`, `skip_offline_change_scan` | 同上。旧 AI サイズ carrier、処理上限・backend・索引の速度/scan 方針も含む。 | `src/settings.rs:5261` |
| 除く | `cache_policy`, `cache_threshold_ms`, `cache_size_threshold_bytes`, `cache_videos_always`, `cache_webp_always`, `cache_pdf_always`, `cache_zip_always`, `edit_preview_cache_enabled`, `edit_preview_cache_max_bytes`, `archive_cache_max_bytes`, `batch_cache_zip_contents`, `batch_cache_pdf_contents` | キャッシュ容量・速度・保持方式は移行先のディスク/性能調整として一組で除外。キャッシュ実体も別ストア (§7)。 | `src/settings.rs:4410` |
| 除く | `thumb_idle_upgrade` | 同上。サムネイルの idle 品質更新は性能方針として除外。 | `src/settings.rs:4488` |
| 除く | `remote_service_enabled`, `remote_video_streaming_enabled`, `remote_video_encoder`, `remote_video_quality_default`, `remote_video_segment_window`, `remote_video_mute_local_output`, `remote_video_hide_local_output`, `update_check_enabled` | 接続/送出・ローカル出力方針、自動通信の opt-in は移行先を維持。接続情報は Settings 外も転送しない。 | `src/settings.rs:5397` |
| 除く | `reading_history_limit`, `exif_hidden_tags`, `video_seek_bar_locked`, `video_seek_strip_locked`, `video_deinterlace` | 保持数の prune、EXIF 任意文字列、動画下部固定の scope 外状態変更、デインターレースの性能 tuning。推奨初期除外 (§7)。 | `src/settings.rs:5590` |
| 除く | `details_selection_bar_mode` | Dedicated への変更を OK すると、除外対象の詳細列設定を既存 A→C 複製で書き換えるため。取り込みでは変更しない。 | `src/settings.rs` |
| 除く | `stack_script_enabled` | 移行先の `stack_rules.rhai` に依存する有効化設定。本体を転送しないため一組で除外する。 | `src/settings.rs` |
| 除く | `keymap`, `ring_shortcuts`, `menu_layout`, `context_menu_layout`, `gamepad_enabled` | 既存の操作カスタマイズ共有が正本。環境設定にも編集入口があっても重複転送しない。 | `src/settings.rs:5198` |
| 除く | `favorites`, `smart_folders`, `tags`, `recent_folders`, `quick_folder_recent_folders`, `quick_folder_slots`, `quick_folder_drive_current_dirs`, `last_folder`, `startup_list_restore`, `last_cursor_name`, `last_cursor_rows_above`, `search_index_checks`, `active_book_name` | 利用データ・登録先・履歴・検索対象。名前や ID も含めない。起動復元先とその一覧カーソルも利用データとして除外する。 | `src/settings.rs:4262` |
| 除く | `pinned_books`, `pinned_collections`, `toolbar_collection_target_id`, `video_resume_positions`, `video_watched_to_end`, `video_audio_track_choices` | 同上。本棚・コレクション参照・再生位置・音声トラック選択。 | `src/settings.rs:4752` |
| 除く | `favorite_view_overlay`, `window_pos`, `window_size`, `window_maximized`, `detached_viewer_window_placement`, `effetune_gui_pos`, `effetune_gui_size`, `vst3_panel_pos` | runtime overlay / PC のウィンドウ配置。serde(skip) も明示分類。 | `src/settings.rs:4273` |
| 除く | `first_setup_completed`, `touch_still_chrome_learned`, `touch_video_chrome_learned`, `last_seen_version`, `update_check_dismissed_version`, `network_data_dir_notice_dismissed_for`, `perf_log_enabled` | 初回/学習/通知/保存版の内部記録。診断ログは移行先で明示有効化。 | `src/settings.rs:5063` |
| 除く | `archive_convert_without_dialog`, `video_loop` | 現行 enum の互換 mirror。転送せず既存の OK/保存で enum から導出。独立した設定ではない。 | `src/settings.rs:4450` |
| 除く | `grid_cols`, `grid_view_mode`, `details_sort_key`, `details_page_count_sort_stash`, `details_place_sort_stash`, `details_sort_ascending`, `details_size_display_mode`, `details_timestamp_show_seconds`, `details_row_style` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4117` |
| 除く | `details_column_order`, `details_column_widths`, `details_rated_at_width`, `details_page_count_column_index_stash`, `details_page_count_column_width_stash`, `details_place_column_index_stash`, `details_place_column_width_stash`, `details_selection_bar_place_column_index_stash`, `details_selection_bar_place_column_width_stash` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4144` |
| 除く | `details_show_preview`, `details_show_rating`, `details_show_rated_at`, `details_show_tags`, `details_show_kind`, `details_show_page_count`, `details_show_place`, `details_show_size`, `details_show_modified` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4166` |
| 除く | `details_show_created`, `details_show_state`, `details_show_image_dimensions`, `details_show_video_duration`, `details_show_video_dimensions`, `details_show_video_codec`, `details_name_width_auto`, `details_name_width`, `details_selection_bar_column_order` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4184` |
| 除く | `details_selection_bar_column_widths`, `details_selection_bar_rated_at_width`, `details_selection_bar_show_preview`, `details_selection_bar_show_rating`, `details_selection_bar_show_rated_at`, `details_selection_bar_show_tags`, `details_selection_bar_show_kind`, `details_selection_bar_show_page_count`, `details_selection_bar_show_place` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4210` |
| 除く | `details_selection_bar_show_size`, `details_selection_bar_show_modified`, `details_selection_bar_show_created`, `details_selection_bar_show_state`, `details_selection_bar_show_image_dimensions`, `details_selection_bar_show_video_duration`, `details_selection_bar_show_video_dimensions`, `details_selection_bar_show_video_codec`, `details_selection_bar_name_width_auto` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4229` |
| 除く | `details_selection_bar_name_width`, `facet_filter`, `thumb_aspect`, `thumb_aspect_auto`, `always_on_top`, `sort_order`, `rating_view_sort`, `subfolder_expansion_order`, `subfolder_expansion_max_depth` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4249` |
| 除く | `subfolder_expansion_filter_kinds`, `subfolder_expansion_filter_date_preset`, `subfolder_expansion_filter_size_preset`, `stack_separator`, `thumb_px`, `text_preview_scale`, `text_smart_snap_enabled`, `thumb_quality`, `show_toolbar_favorites` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4367` |
| 除く | `show_toolbar_smart_folders`, `show_toolbar_tags`, `folder_tree_pane_visible`, `folder_tree_sort_order`, `folder_tree_pane_width_ratio`, `show_toolbar_folder`, `show_toolbar_folder_tree_button`, `show_toolbar_effetune`, `show_toolbar_bookshelf` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4552` |
| 除く | `show_toolbar_collections`, `show_address_bar_history_nav`, `show_address_bar_quick_folders`, `show_toolbar_parent_button`, `show_toolbar_prev_folder`, `show_toolbar_next_folder`, `show_toolbar_vst3`, `show_toolbar_rating`, `show_toolbar_facet_filter` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4579` |
| 除く | `show_address_bar_favorite_button`, `show_address_bar_history_menu`, `show_address_bar_folder_pin`, `show_address_bar_stack_toggle`, `show_address_bar_omitted_entries`, `show_location_drive_list`, `show_location_reading_history`, `show_location_rating`, `show_location_bookshelf` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4610` |
| 除く | `show_location_desktop`, `show_location_pictures`, `show_location_downloads`, `show_location_drive_roots`, `use_native_shell_context_menu`, `rating_filter`, `conceal_type`, `conceal_mosaic_tile_mode`, `conceal_mosaic_boundary` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4641` |
| 除く | `conceal_fill_opacity_percent`, `conceal_fill_edge`, `conceal_blur_radius_px`, `conceal_blur_mode`, `conceal_blur_feather`, `conceal_brush_radius`, `conceal_line_width`, `conceal_presets`, `export_embed_metadata` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4771` |
| 除く | `export_fallback_format`, `export_default_scale`, `export_batch_selection`, `export_batch_template`, `export_batch_format`, `export_batch_scale`, `sns_split_target`, `sns_split_count`, `sns_split_seam_permille` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4811` |
| 除く | `sns_split_frame_ratio`, `downscale_smoothing_percent`, `fullscreen_left_panel_tab`, `adjustment_settings_tab`, `fullscreen_navigator_visible`, `fullscreen_navigator_corner`, `fullscreen_navigator_size`, `margin_fit_enabled`, `ui_scale_factor` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:4846` |
| 除く | `toolbar_cols_items`, `toolbar_cols_20_options_migrated`, `toolbar_cols_details_visible`, `toolbar_aspect_items`, `toolbar_aspect_auto_visible`, `toolbar_cols_display`, `toolbar_aspect_display`, `toolbar_sort_display`, `toolbar_favorites_display` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5072` |
| 除く | `toolbar_smart_folders_display`, `toolbar_tags_display`, `toolbar_bookshelf_display`, `toolbar_collections_display`, `toolbar_favorites_collapsed`, `toolbar_smart_folders_collapsed`, `toolbar_tags_collapsed`, `toolbar_bookshelf_collapsed`, `toolbar_collections_collapsed` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5101` |
| 除く | `toolbar_sort_items`, `toolbar_sort_size_options_migrated`, `toolbar_sort_name_numeric_desc_options_migrated`, `toolbar_sort_rating_options_migrated`, `toolbar_facet_filter_items`, `toolbar_facet_name_filter_index_stash`, `facet_name_filter_width`, `toolbar_section_order`, `show_toolbar_cols` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5129` |
| 除く | `show_toolbar_aspect`, `show_toolbar_sort`, `toolbar_section_new_row`, `toolbar_section_drag_enabled`, `recent_open_with_apps`, `custom_open_with_apps`, `ai_upscale_enabled`, `ai_upscale_model_override`, `erase_inpaint_mono_tolerance` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5168` |
| 除く | `global_preset`, `preset_slots`, `post_filter_global_preset_stash`, `post_filter_preset_slot_stashes`, `colorize_preset_slots`, `metadata_export_recursive`, `video_playback_speed`, `video_seek_strip_state`, `video_seek_strip_last_choice` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5290` |
| 除く | `video_seek_strip_span`, `video_autoplay`, `video_autoplay_mode`, `video_continuous_mode`, `video_muted`, `video_adjustments`, `video_scale_filter`, `video_downscale_smoothing_percent`, `video_anime4k_budget` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5467` |
| 除く | `video_anime4k_measurement`, `video_preset_slots`, `video_tile_columns`, `video_in_window_mode`, `detached_viewer_enabled`, `vst3_gui_visible`, `vst3_video_compact`, `audio_normalize_enabled`, `audio_normalize_target_lufs_milli` | 環境設定外で管理する表示/ツールバー/補正/編集/再生状態、またはその互換 carrier。各定義コメントと overwrite_non_preferences_from (settings.rs:9692) が根拠。 | `src/settings.rs:5544` |

現在の内訳: 出す131 / 除く310 / 合計441フィールド（`all_settings_fields_are_classified` の確認値）。互換フィールドをまとめるため転送ファイルの項目数は129。§1.333追加watched集合は個人の視聴履歴・pathとして除外する。§1.335の起動復元レコードは、最後に明示した利用者の一覧とその cursor を対で持つため利用データとして除外する。転送や Preferences OK で移行済み record を旧 `last_folder` から作り直さず、live record を保持する。表示・復元の実装と自動検証は完了、実機確認待ち。結果は [設計 §10](startup-restore-target-plan.md#10-phase-2-の安全な区切りと-viewport-完了経路への追加合意事項) に集約する。調査時点の432という記録は履歴として保持する。

### 2.2 実装時の唯一の policy と強制テスト

`src/settings_transfer.rs` に、全フィールドを一度ずつ記す policy macro を置く。
各行は `export(field, wire_key, validator)` または `exclude(field, reason)`。
論理的に一体の値だけは `export_group([fields...], wire_key, validator, draft_setter)`。
型定義を生成する巨大な Settings macro への改造、reflection 用の全体シリアライズ、
DB の COMPLEX_FIELDS からの対象推定はしない。

policy から、(a) 分類メタデータ、(b) 対象値だけの projection、(c) 検証済み項目の
draft setter、(d) **全 Settings フィールドの、`..` のない分解パターン**を生成する。
分解は `&Settings` を借りるだけ。exclude の値を clone / serialize しない。
同じ field の重複列挙もコンパイル時に失敗する。Rust 側のフィールド名と wire_key は区別し、
serde rename (`gpu_memory_percent` → `thumb_vram_cap_percent` 等) や skip に依存しない。

`settings_transfer::tests::all_settings_fields_are_classified` は生成された全列挙関数を使い、
重複分類・wire_key の重複・空の除外理由・setter の未対応を検査する。
Settings に新規フィールドを足して policy に足さないと **このテストを含む target の
コンパイルが失敗する**。既定値の JSON キー集合だけを比較する方法は、skip / 空値 /
carrier を見落とすので主 gate にしない。必要な serde 特性の確認は補助テストで行う。
テストを無視して製品 target だけ build しても同じ exhaustive pattern が compile gate になる。

新規フィールドの統合は、同じ policy に対象と検証または除外理由を 1 行足すだけ。
`show_facet_sort`、`toolbar_folder_section_migrated` とフォルダバーのツールバー区画は
環境設定外 / 内部移行として除外する (master 統合時の分類は §2.4)。「..」行の表示設定は、実名と環境設定 UI、
既存の OK 経路を確認して bool の export 行を足す。分類前は build/test が失敗する。
現在の 432 という件数だけに依存するテストにはしない。次の追加でも明示判断を必須にする。

### 2.3 対象外保持の意味

import は対象外フィールドに setter を持たず、未知キーとして受け取っても読み替えない。
対象外を含む既存 draft 全体、PreferencesState の各入力 buffer、プラグイン候補、
LUT transaction、フォント準備、clear_requested、インストール要求を作り直さない。

OK の既存副作用で生じる **導出状態**は区別する。video_loop / archive_convert_without_dialog
の同期、表示用キャッシュの失効や viewer の閉鎖は、同じ設定を手で変更した場合と同じ。
`details_selection_bar_mode` は Dedicated への変更時に除外された列設定を A→C 複製するため、
今回の対象から外す。転送してもこのモードを変更せず、既存の列構成を保持する。
★・編集・タグ・コレクション・履歴行等の利用データを消す副作用は認めない。

**既存 OK の限界 (利用者承認済み、今回未修正)**: 取り込みで対象外に setter を持たないことと、
OK が最新 live 値を必ず保持することは別。`video_playback_speed` 等が環境設定の表示中に
変わると、既存の全体差し替えで開いた時点の値へ戻る §1.305 を取り込み後の OK も引き継ぐ。
また `show_hidden_files` 等の変更後に OK すると、通常フォルダを UI スレッドで同期再読込する
§1.295 を引き継ぐ。今回の worker 化は転送ファイルの処理だけで、OK 経路全体の非同期化ではない。

### 2.4 master 統合時の追加分類 (2026-10-05)

初期の 432 フィールドの表は調査履歴として保持する。統合後は全 439 フィールドを
exhaustive pattern で分類し、出す 131 / 除く 308、転送キーは複合項目を含め 129 個。

| 判定 | フィールド | 理由 |
| --- | --- | --- |
| 出す | `raw_brightness` | RAW 専用環境設定の明るさ方針。パス・性能 tuning ではない。現行 UI の `MatchPreview` / `None` のみ検証して既存 OK へ渡す。 |
| 除く (PC 固有) | `raw_develop_parallelism` | CPU・メモリ・速度に依存する worker 数。既存の `parallelism` / `pdf_worker_count` と同じ境界。 |
| 除く (利用状態) | `active_quick_folder_slot` | A/B 登録先・履歴と一組の現在のワークスペース。移行先の選択を保持する。 |
| 除く (環境設定外) | `show_facet_sort` | §1.292 の独立ソート区画。既存の `show_toolbar_sort` と同じくツールバー側が所有する。 |
| 除く (内部移行) | `toolbar_folder_section_migrated` | §1.207 の一度だけの配置移行記録。転送で再移行させない。 |
| 除く (PC 固有) | `effetune_pre_limiter_enabled`, `effetune_keep_visible_when_minimized` | 移行先の EffeTune/VST 導入状態・音声構成・窓の運用と一組の微調整。既存の VST 有効化・微調整の除外規則に従う。 |

§1.207 の `ToolbarSectionId::Folder` は既存の順序・行頭集合に含まれる enum 値で、
新しい Settings field ではない。`toolbar_section_order` / `toolbar_section_new_row` /
`toolbar_section_drag_enabled`、フォルダバーの表示・補助ボタンも既存の除外を維持する。
起動窓の `window_pos` / `window_size` / `window_maximized` も既存の PC 固有分類を維持する。
統合時点に「..」行表示を保存する Settings field はなく、将来追加する際は §2.2 の通り再分類する。
追加回帰試験は RAW 方針の転送、未知 enum の保持、除外キーの出力不在・偽造入力からの
PC 固有値・ツールバー配置の保持を検証する。

## 3. ファイル形式・版・検証規則

### 3.1 形式と互換

推奨名は `preferences.mivprefs.json`、UTF-8 (BOM なし) の pretty JSON + 終端改行。
import は UTF-8 の BOM も許容する。拡張子ではなく format で識別する。

```json
{
  "format": "mimageviewer.preferences",
  "format_version": 1,
  "app_version": "4.3.0",
  "preferences": {
    "ui_theme": "Dark",
    "show_hidden_files": true,
    "video_seek_small_secs": 3,
    "still_bottom_chrome": { "lock": "BarOnly", "visible": true }
  }
}
```

例は一部だけ。通常 export は全対象の確定済み共通設定値を出す。
app_version は build の定数から生成し、import の参考表示だけに用いる。
ラベル、ユーザー名、PC 名、生成元 data-dir、ファイルパス、自由記述、時刻は埋め込まない。
PreferencesState の UI 状態、Settings の保存メタ、DB の版番号も入れない。

format / format_version / preferences は必須。preferences は object、
format_version は正の u32 整数で v1 だけ対応。app_version は任意で欠落可。
他のトップレベルキーは無視する。app_version が違うだけでは拒否しない。
v1 のまま項目を追加・削除してよいが、既存キーの型・意味を変更したり名前を再利用しない。

| 入力 | 扱い |
| --- | --- |
| 古いアプリが書いた v1 / 新しいアプリが書いた v1 | 既知で正しい項目だけ draft に載せる。未知キーは無視し件数を知らせる。欠落は取り込み直前の draft 値を保持。 |
| v1 の未知 enum variant / 現行 UI にない値 | その論理項目だけ無視し、現在 draft 値を保持。既定値へ置換しない。 |
| format_version = 0、欠落、型違い、負数、v2 以上 | ファイル全体を拒否して draft 不変。未来の破壊的形式を推測して読まない。 |
| 別 format、Settings 全体の JSON、操作カスタマイズ JSON、SQLite | 全体を拒否。自動変換や DB open をしない。 |
| 新版で項目を廃止した v1 を旧アプリで取り込む | 欠落なので旧アプリのその設定を維持。新アプリが知らない旧キーは無視。 |

形式 v1 は本機能で初めて導入する未出荷形式。旧 DB / JSON migration は変更しない。
初回出荷後の互換 v1 fixture は残す。将来、破壊的変更で版を上げる場合は、
旧版の読替えと fixtures を別の設計で決める。今は移行エンジンを作らない。

### 3.2 項目の検証・適用単位

worker がサイズ上限 **1 MiB** + 1 byte まで bounded read して超過を拒否する。
serde_json の標準深さ制限を維持し、JSON の不正構文・不正 UTF-8 は全体エラー。
同一 object 内の重複キーは、どの値を採用するか曖昧にせず全体エラーにする
(小さい duplicate-key 検査 visitor)。未知キーにもサイズ・深さ・重複の制限が効く。

ファイル全体の parse と各項目の検証を完了してから、検証済み値の集合と結果を返す。
UI はそれを一度に draft に載せる。検証中の順次適用、途中保存、再試行、
部分適用 transaction、ロールバック、完了済み項目の永続 journal は作らない。
「正しい項目だけを採用」は **未確定の draft 編集** であり DB への部分適用ではない。

bool は JSON bool のみ。数値・文字列の型変換はしない。整数に小数・負数・overflow を
受け入れず、浮動小数は finite のみ。null は今回の対象で許容しない。
enum は現在 UI の候補 (ALL / all() 等) に入るものだけ。Settings の Deserialize /
sanitize に未知値を既定値へ落とす型があっても、その挙動を取り込みに使わない。
範囲外は clamp せず、その論理項目を丸ごと無視する。

| 対象 | 許容値 / 根拠 |
| --- | --- |
| 各 bool / enum | 現行 UI と同じ型・候補。旧 UiTheme::Standard のような UI にない値は無視。 |
| slideshow_interval_secs / continuous_wait_secs / continuous_scroll_secs / continuous_scroll_percent | 0.5–30 / 0.1–30 / 0–5 秒 / 1–100 % (pages.rs:1652–1688 の Slider 範囲)。 |
| folder_thumb_sort / depth / folder_skip_limit | UI の 4 種 FileName / Numeric / DateAsc / DateDesc、0–10、1–30 (pages.rs:8651–8698)。 |
| spread_page_gap_px / continuous_reading_gap_px / fullscreen_fixed_bar_gap_px | 0–200 / 0–200 / 0–FULLSCREEN_FIXED_BAR_GAP_MAX_PX (pages.rs:9644 / 9740 / 9752)。 |
| fullscreen_jump_percent / fixed_jump_count / cursor_hide_delay_secs | Settings の MIN / MAX 定数。現行 1–100 / 1–100 / 0.1–5.0 (`settings.rs:6785`)。 |
| continuous_reading_wheel_scroll_percent / key_scroll_percent / gamepad_scroll_percent_per_sec | 1–100 / 1–100 / 10–300 (pages.rs:9766–9784)。 |
| video_volume | 0–VIDEO_VOLUME_MAX の finite gain。現行 0–7.943282347242816 (`settings.rs:5770`)。 |
| video_seek_small / medium / large_secs | VIDEO_SEEK_SECONDS_MIN..MAX、現行 1–600 (`settings.rs:6174`)。UI と同じく大小の順序条件を新設しない。 |
| video_seek_thumbnail_tolerance / strip_min_interval / strip_waveform_span_secs | 各 MIN / MAX 定数。現行 0–30 / 0.1–1800 / 5–10800 (`settings.rs:6199`)。 |
| *_seek_preview_size_values / *_seek_strip_height_values | smallest / small / medium / large / maximum の 5 整数が必須。各 UI と同じ MIN_POINTS / MAX_POINTS (`settings.rs:6354`、`:6374`、`video/seek_strip_layout.rs:44`)。値の大小関係は新設しない。 |
| fullscreen_image_margin_color | ちょうど 3 個の u8。文字列や第 4 要素を許容しない。 |
| grid_display_order | ちょうど 4 行の配列で各組込みカテゴリがちょうど 1 回。空行は保持。重複・欠落・未知カテゴリ・不正行数は項目全体を無視。GridDisplayOrder の寛容な Deserialize による補完は使わない (`settings.rs:2387`)。 |
| video_seek_strip_cycle | thumbnails_window / thumbnails_whole / waveform_window / waveform_whole の 4 bool が必須で、少なくとも 1 個が true。全 false / 欠落 / 型違いは項目全体を無視し、normalized() による既定補完を使わない (`video/seek_strip_layout.rs:284`、`:341`)。 |
| image_ext_priority | 組込み default_image_ext_priority() の候補の重複なし完全な並べ替えのみ。余分な任意文字列、パス、URL、Susie 固有拡張子が入る列は項目全体を無視。 |

複合項目の欠落した **トップレベルキー** は現在値を保持。キーが存在する複合値では
必須子値の欠落・不正を項目全体のエラーにする。未知子キーだけは無視する。
子値ごとの欠落補完や部分更新を新設しない。enum 表現は現行 serde 表現を基本にするが、
export 前の検証と厳密な専用読取により寛容な deserialize の fallback を迂回する。

静止画下部の 3 フィールドは `still_bottom_chrome` の一項目へまとめる。
lock は None / BarOnly / BarAndStrip、visible は bool。BarAndStrip + visible=false は
項目全体を無視し、移行先の 3 値を全部保持する。妥当な値を
set_still_bottom_lock + set_still_seek_strip_visible の既存 owner へ通し、
操作順に左右される二つの bool の個別 setter を作らない。
独立キー fullscreen_seek_bar_locked / still_seek_strip_locked / still_seek_strip_visible を
ファイルに混在させず、これらを偽造しても未知キーとして無視する。

archive_file_handling の export は archive_file_handling_resolved() の値を使う
(`settings.rs:8521`)。Default の Legacy はファイルへ出さず、UI と同じ Ask / Convert /
Ignore に解決する。import はこの 3 値に限定し set_archive_file_handling を通す。
video_loop_mode は UI と同様に互換 video_loop を同期する。
setter が対象以外に触れるのは分類表で明示した互換 mirror だけ。
PreferencesState の補助入力値を同期する必要が生じた場合は対象 field の setter に集約し、
PreferencesState 全体の new() や draft 全体の sanitize() を呼ばない。

export も同じ validator を使う。確定済み設定に不正値や許容外の任意文字列があれば
該当項目を省いて一覧で知らせ、除外値を補充しない。
出力 key 集合が policy の export wire_key 以下であることをテストする。
正常な状態なら全項目を出す。不正を理由に任意の文字列をそのまま通知・ログへ埋め込まない。
項目表示名・理由だけを使い、未知キーは件数中心で表示する。

## 4. UI・非同期所有・OK / キャンセル

入口は **設定メニュー →「設定の復元…」**に集約する。
同ダイアログに「環境設定を書き出し…」「環境設定を取り込み…」ボタンを置く。
環境設定の「全体設定」から「設定の持ち運び」欄とその検索索引を削除する。
環境設定が表示中なら新しい二つの転送ボタンだけを無効にし、
「環境設定を閉じてから行ってください」を disabled tooltip で示す。
従来の復元・操作カスタマイズの入口と操作は制限しない。
入口の無効化は書き出し・取り込みで揃えるが、受付済みの書き出しは後から環境設定が
開いても確定済み値の保存を完了させる。
書き出し・取り込みの説明には同じ見た目の egui Modal を使い、転送対象の範囲を
ファイル選択前に伝える。世代を選んで「この時点に戻す」既存復元とは用途・対象が違う。

書き出し説明では、移行先に依存しない環境設定の一部だけを保存し、パス・利用データ・
操作カスタマイズは含まないことを示す。「書き出す」で保存先選択へ進む。
書き出しは **確定済み Settings の preferences_snapshot()** を使い、お気に入り専用値や
未確定 draft を含めない。rfd の JSON filter と既定名 preferences.mivprefs.json を使い、
同名の上書き確認は既存の保存ダイアログに任せる。保存先の記憶を Settings に追加しない。
選択後は worker でファイルを書き、完了・失敗の結果通知を説明 Modal に表示する。
書き出しは live 設定や DB を変更しない。

取り込み説明では、「この時点に戻す」と対象が異なり、移行可能な環境設定の一部だけが
変わること、移行先の保存先・利用データ・操作カスタマイズ等を保持することを明示する。
「ファイルを選ぶ」で rfd の open_file へ進み、1 ファイルを選択する。
worker の読込・検証が成功したら、受入境界一か所で既存の show_preferences を確認する。
環境設定が独立して開かれていれば読み込み結果を捨てて中止を通知し、draft / live / DB は触らない。
開かれていなければ設定の復元を閉じ、環境設定の「全体設定」
(PreferencesPage::General) を開く。正常項目を新しい draft へ一度に載せ、既存の結果欄を表示する。
「N 項目を読み込みました。OK で保存します。キャンセルで今回の変更を取り消します。」
変更なしの有効項目数と実際に変わった項目数は区別できるようにする。
折りたたみの一覧で、変わった項目の日本語名・不正項目の名と理由・未知項目の件数を表示。
未知キーや値の原文を大量表示しない。利用者は通常のページへ移動して値を確認・再編集できる。
有効項目が 0 の場合も成功したように見せず「取り込める項目がありませんでした」。
ファイル全体エラーは説明 Modal の結果通知 + ログ、環境設定を開かず live / DB を変更しない。

**組合せ削減の採用**: ファイル選択後の read/parse/write は短いモーダル処理にする。
SettingsRestoreState に説明・転送処理・完了/失敗を一つの enum で所有させる。
`None` が Idle、`Explanation / Importing(receiver) / Exporting(receiver) / Exported`
が各段階を表す一つの job owner とし、失敗は Explanation 内の結果として通知する。
状態を別の bool / pending 欄に分散させない。
同時処理や後続 job の queue を作らず、PreferencesState に転送 receiver を持たせない。
App の環境設定と操作カスタマイズの draft は、それぞれ `Option<Box<PreferencesState>>` に格納する。
大きな一時編集状態を App の常時スタック配置から外す収納変更は維持し、draft の
take・OK・Cancel・再開時の寿命は変えない。取り込み結果だけを PreferencesState 内の
`Option<PreferencesTransferFeedback>` に渡し、既存の結果表示を再利用する。
処理中は説明 Modal を busy 表示にし、設定の復元の操作・閉鎖と説明 Modal の
実行・キャンセル・Enter / Escape を無効にする。結果受信は try_recv、worker 終了で repaint を要求する。
説明・処理中・結果通知のいずれも、転送 Modal が実際に表示される間は背後の環境設定の
UI / Enter / Escape / 閉鎖受付 / 破棄確認を止める。表示は show_settings_restore と既存 owner
から導出し、worker の busy 判定と区別する。通知を閉じるキーで既存 draft を変更しない。
操作カスタマイズの「押して入力」捕捉も、IME と転送 Modal の実表示を同じキー捕捉の
可否判定へ通す。Modal の表示中は待機 slot・入力欄・編集通知を変えず、閉じれば捕捉を再開する。
通常の設定復元が表示されているだけなら捕捉を止めず、既存復元項目も無効化しない。
メイン viewport の背面メニュー・ツールバーへの入力は、show_settings_restore が登録済みの
`common_modal_dialog_open` と説明 Modal に任せる。転送専用の menu/toolbar guard は作らない。
**別窓・fullscreen・native 動画の入力は転送のために止めない。** 転送はファイルの
読み書きと draft への反映だけで、別窓の閲覧状態と関係しないためである。
転送専用の viewport sanitizer、passive event batch の破棄、detached activation/watch close の拒否、
native HUD dim、semantic event・hold/repeat/長押しの終了をすべて撤去する。
従来の設定復元・環境設定/common modal が持つ挙動を保ち、別窓の入力経路・状態 owner を変更しない。
ファイル I/O、JSON parse/serialize、flush、置換は worker 上。UI で DB open / stat /
ファイル読込 / 同期 join をしない。取り込み成功後は通常の環境設定を開くため、
既存のフォント/LUT等の準備と OK の判定をそのまま使う。

設定復元に対する既存の背面入力ブロックを使い、処理中にメイン viewport の
環境設定・操作カスタマイズ・ツールバー操作が始まらないことを本番 UI の入力経路で確認する。
別窓切替の禁止や転送専用の共通入力 owner は追加しない。
既存の裏で動く再生位置保存等は止めず、修正済みの再生位置・音声トラックは OK の live merge で維持する。
他の環境設定外の最新値には §1.305 の限界が残る (§2.3)。
通常の設定操作は Idle で従来どおり。長時間待ち対策の retry / sleep / supersession /
resume / 保存中のキャンセル後 rollback は追加しない。

App の終了等で SettingsRestoreState が drop された場合は receiver も消えるので、
完了値は別のダイアログへ到達しない。worker は Settings / App / DB の参照を保持せず、
export では捕捉済み対象値だけ、import ではファイルと結果だけを所有する。
export はアプリ終了後でも選択済みファイルへの書き出しを完了し得る。
その理由だけで実設定の保存や次のセッションへの pending 復元を作らない。

OK は手編集と同じ prepare → install → 副作用 → save_checked の一経路。
Cancel / × は今回の import と環境設定で行った未確定手編集をまとめて取り消す。
import を始める前の draft に戻す専用ボタン・履歴・部分 rollback は設けない。
説明 Modal の Cancel とファイル選択の Cancel は設定を変更せず、環境設定も開かない。
独自ショートカットは追加しない。Enter / Escape は dialog_enter_pressed /
dialog_escape_pressed を使い、IME 変換中の入力で import・OK・閉鎖が発火しない。

### 4.1 まれな失敗の範囲

読込・検証・worker 起動失敗は通知して draft / live / DB を変えない。
書き込み失敗は通知して成功扱いにしない。既存の選択先ファイルを途中で truncate
しないため、同じ親の一時ファイルへ完成 JSON を書き flush/sync 後、一度だけ置換する。
既存 `archive_converter::replace_file_atomic` (`src/archive_converter.rs:726`) を使える。
共有 helper の大規模移動は不要。失敗時は既存 destination を維持し、
自分の一時ファイルだけ best-effort 除去する。除去失敗は log のみ。
これは単一ファイルの通常保存で、復旧 journal・世代管理・自動 retry・終了後回収は作らない。

OK 時の DB 保存失敗は、既存通りメモリには確定済み、永続化は未完了の可能性がある
(`preferences.rs:2617`)。既存の通知を出す。「何もしない」は転送の I/O / 構文失敗を
指し、既存の OK 保存失敗まで巻き戻す仕様にはしない。
import 専用の DB rollback、バックアップ復旧、再起動時の自動再適用は追加しない。
利用者の設定や既存書き出しファイルを削除する割り切りも採用しない。

## 5. 製品ページ・privacy の突き合わせ

次の二か所を実際に照合した。

| 文書と位置 | 現状 / 影響 |
| --- | --- |
| `htdocs/mimageviewer/index.html:1203`「安心して使えます」、`:1212`、`:1234` | 通信は更新確認・任意 component 取得・Remote の 3 場面、設定や履歴は PC に保存と記載。新しいアプリ通信先・認証・自動送信は増えない。利用者が指定したファイルへの設定保存は説明を足す。 |
| `htdocs/mimageviewer/privacy.html:135`「端末内に保存されるデータ」、`:151`、`:198`「ネットワーク通信」 | Settings は既存の data-dir に保存と記載。利用者指定の書き出しファイルは data-dir 外にも置けるので保存先の説明を両文書で補足する。通信の列挙は変更不要。 |

両文書へ同じ事実を追記した:
「環境設定のうち移行できる項目は、利用者が選んだファイルにも保存できます。
ファイルに閲覧履歴・登録先・接続情報は含めず、アプリから外部へ送信しません。」
「全設定のバックアップ」「機密情報が必ず全て消える」など本範囲以上の保証はしない。

アプリ独自の HTTP / IPC 転送、Telemetry、アップロードはない。
ファイル選択で UNC / ネットワーク共有 / 同期フォルダを指定した場合は、その保存先の
OS・同期ソフトによる通信があり得る。「物理的にローカルディスクだけ」とは保証せず、
利用者が選んだファイルの read/write という既存のモデルに揃える。
取り込みにより Remote / 更新確認の有効値を変えず、追加ダウンロードや接続を始めない。
URL、実行ファイル、LUT、プラグインの読み込み指定として JSON を解釈する経路もない。

## 6. 実装後のテスト・検証・文書更新計画

### 6.1 自動テスト (実装担当が所有)

| 層 | 必須確認 |
| --- | --- |
| 分類 / projection | 全 432 フィールドを一回分類、wire_key 唯一、skip/carrier も含む。新フィールド未分類で target がコンパイル失敗することを一時的な追加で確認して戻す。出力キーは export policy だけ。 |
| 個人情報・パス漏れ | 除外される PathBuf / String / ネストした利用データへ、ユーザー名・絶対パス・UNC・URL・PIN風文字列・タグ/本/コレクション名の固有 sentinel を入れた Settings から出力して、一つも含まれないことを確認。全対象値は妥当な非既定値。EXIF の任意文字列は除外、image_ext_priority のパス混入はその項目を出さない。serde(skip) の runtime overlay にも sentinel を置く。 |
| 通信の自動有効化 | policy から生成しない明示テストで remote_service_enabled / remote_video_streaming_enabled / update_check_enabled をそれぞれ固定。true の source から出力 JSON にキーがないこと、true 入力でも移行先 false が draft / 本番 OK / DB 再読込で保持されること。正常なテーマ変更も同時に取り込んで取込成功を確認する。 |
| 保持 / 差分 | 複数対象と複数対象外を異なる非既定値で埋めた移行先 draft に apply。対象が復元し、それ以外の全フィールドが不変 (互換 mirror の明示例外だけ別 assertion)。JSON の全体比較だけでなく skip フィールドも比較する生成テスト projection を使う。 |
| 不正値 | 一つの型違い・範囲外・null・overflow・未知 enum と正常項目を同居させ、正常項目だけ draft 更新、不正項目は元値 + 警告一覧。範囲の両端/直外、複合欠落・重複・余分な配列要素も確認。 |
| 全体不正 / 互換 | 空・壊れた JSON・不正 UTF-8・サイズ/深さ超過・重複キー・別 format・版欠落/0/未来 v2 は draft/live/DB 不変。旧アプリ v1 の項目欠落、新アプリ v1 の未知キー、未知 enum は単純規則どおり。初回出荷 v1 fixture を保持。 |
| 状態 owner | still_bottom_chrome 全到達状態と不正状態、複合キー欠落による 3 値保持。ファイルで独立 bool キーを偽造しても不変。loop と archive の enum/互換 mirror の通し。Default の Legacy は export で Ask、旧 mirror=true の Legacy は Convert として出力し、import の Legacy は無視。 |
| ファイル保存 | 一時 data-dir 外の tempfile へ書いて読める。既存 export に対する write/置換失敗で既存ファイル保持、通知あり。失敗 cleanup を再帰 recovery にしない。 |
| 非同期 lifecycle | SettingsRestoreState の job 二重開始なし、説明 Modal の busy 中は実行/Cancel/×/Enter/Escape 無効、spawn失敗・channel切断で Idle に戻り通知、state drop の結果が次の SettingsRestoreState や PreferencesState に届かない。App/DB を worker が所有しない。 |
| 入力 handler | 転送処理中にメイン viewport の設定メニューの「環境設定」「操作カスタマイズ」、ツールバーの操作を、本番設定復元と説明 Modal を描画した UI へ raw pointer 入力しても開始されないこと。common modal と唯一の job が維持され、ダイアログを閉じた後に通常操作が通ること。転送専用 guard や共通述語だけのテストで代替しない。 |
| 入口 / draft / 実際の OK | 書き出しと取り込みで同じ見た目の説明 Modal を表示。説明/ファイル選択の Cancel は設定不変、全体エラーでは環境設定を開かない。検証成功時だけ設定復元から環境設定 General へ移り、既存結果欄を表示する。既存 PreferencesTestApp と handler-level/headless UI を利用 (`preferences.rs:3619`、`:3692` の整理先テストが precedent)。取込直後の live / DB 不変、OK → install → save_checked → DB 再読込 → 環境設定再 open で同値。Cancel は import と手編集を破棄。export は確定済み標準値を使い、未確定 draft を含まず live を変更しない。 |
| 別 data-dir 往復 | TempDir A/B に独立 DB を用意。A の対象を export、B は異なるパス・整理先・フォント・Susie/VST/LUT・操作共有・利用データを持たせ import→OK→save→再読込。対象だけが A に一致し B の対象外が保持される。global data-dir を使う試験は既存 guard / 直列化を使い APPDATA に触らない。 |
| 利用データ / 全設定の通し | ★・編集・本棚・タグ・コレクション・履歴・読書位置・normalize 等の DB と cache 行を B に用意し、import で削除/変更されないこと。read-only 比較と既存の fixture API を使う。reading_history_limit 不変で prune なし、clear_requested/インストール要求なし。対象設定による既存表示 cache の失効は別 assertion。 |
| 標準/専用値・再生 | お気に入り overlay 適用中に export は標準値だけ。import→OK で標準だけ更新、favorite の専用値/記録と修正済みの live 再生位置・音声トラック更新を維持。details_selection_bar_mode と stack_script_enabled は出力・取込対象外。§1.305 の未修正値まで保持できるとは判定しない。 |
| 共有 / 回帰 | keymap、ring、menu、context menu、gamepad は出力にも setter にもない。既存操作共有テストを再利用。通常画像/ZIP/PDF/動画、main/detached/Remote の設定反映は既存 OK の検証を再利用し、import 固有の経路がないことを確認。 |
| UI / IME | 設定復元の二つの入口、書き出し/取り込み説明 Modal、結果/不正一覧、処理中状態の snapshot。環境設定 General の旧入口がないことも確認。既定幅/狭幅、明暗テーマ、長いメッセージの折返し。IME fake-input で変換確定が実行/OK/キャンセルを起こさない。既存 EXIF 入力をついでに変更しない。 |

実行順は純粋 `cargo test -p mimageviewer --lib settings_transfer`、関連 preferences/
settings/operation_customize_share の狭い filter、core check、fmt / glyph check、
共有 OK 経路と多数の設定を扱うため最終 `.\scripts\test-full.ps1`。
cold compile は 10 分以上、broad/full test は 15 分以上の実行枠を確保する。
有効な既存結果を再利用し、変更がない領域の検証を重ねない。
その後、実装時には `.\scripts\build-dev.ps1` で確認用 core を用意する。
設計書だけを作成した前段では cargo / build / 製品起動を実施していない。
現在の実装に対する検証結果・確認用 build の証拠は実施後に追記する。

### 6.2 後の対話検証と更新する文書

実アプリ確認は別の承認済み検証枠に残す。具体的な予定は disposable portable / 独立
A/B data-dir とテスト画像のみで 10–15 分、ファイルダイアログ・環境設定の画面確認、
import→Cancel / import→OK→再起動、暗/明テーマ・狭幅、IME 中の Enter/Escape。
デスクトップ/input を使う承認を得てから prepare-portable-smoke.ps1 のコピーだけを起動する。
実データ・通常 APPDATA を agent が起動・変更しない。

実装後のユーザー向け handoff は
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常 APPDATA を使い設定/データを更新し得ること、共通 single-instance mutex のため
インストール済み/トレイ常駐の mIV を先に終了することを伝える。
別 data-dir 往復、import の確認・OK/Cancel、対象外の整理先/操作設定保持を確認項目にする。
実機結果や独立レビューの完了を、未実施のまま済みと記録しない。

実装時の文書更新先:

- `docs/spec.md` §8: 対象の境界、draft/OK、ファイル形式・互換・不正項目の規則。
- `docs/architecture-overview.md`: 変換モジュールと SettingsRestoreState の job ownership、
  永続化ストア一覧に利用者指定の JSON (正本 DB ではない) を追加。
- `docs/README.md` と本書: 索引、確定判断、実装/検証/独立レビューの記録。
- `htdocs/mimageviewer/manual/settings.html`: 「設定メニュー → 設定の復元」の操作、
  説明画面、確定済み値の export、import 後の環境設定・OK/Cancel、非対象、操作カスタマイズ共有への案内。
- `htdocs/mimageviewer/index.html` と `privacy.html`: §5 の同一事実を同時更新。
- `docs/keymap-spec.md`: ボタン操作・dialog helper の固定入力を必要な範囲で記録。
  新規 KeyAction / keymap.ini の変更は不要。
- backlog §1.317 は検収後に既存運用で整理する。今回は消さない。

マニュアル・製品ページには「vX で追加」等の版固有記述を置かない。
JSON の形式版は技術仕様の本書/spec に記録する。実装・独立レビューは別 context で、
範囲と不変条件を本書から渡す。検証結果の所有は実装担当、検収は設計担当。

## 7. 利用者・設計担当の判断記録

設計段階に返した以下の推奨案は、2026-10-04 に利用者がすべて承認した。
その後、同日の追加指示で入口を設定の復元へ移し、書き出し元を確定済み値へ変更した。
最初の draft 書き出し判断は履歴であり、現在は §4 と下表の追加判断を適用する。
代案は判断の経緯として残す。追加で詳細表示下部バーのモードとスタック script 有効化を
除外し、§1.305 / §1.295 の既存 OK の限界を今回の修正対象に含めないことを承認した。

| 判断 | 推奨案 / 代案とコスト |
| --- | --- |
| 現在の draft を書き出すか (初期判断・変更済み) | 当初は draft を出し、未確定も含むと表示していた。下記の追加指示により確定済み値へ変更した。 |
| 入口と書き出し元 (追加指示) | 設定メニュー「設定の復元…」に「環境設定を書き出し…」「環境設定を取り込み…」を置き、同じ見た目の説明 Modal から進む。環境設定 General の旧欄を削除し、確定済み共通設定から書き出す。書き出し元と取り込み先を分け、環境設定 draft にファイル処理の owner を持たせない。 |
| 取り込み後の確認 (追加指示) | 説明に「この時点に戻す」と対象が異なることと、移行先の利用データ等を保持することを明記する。ファイル検証成功後だけ環境設定 General を開き、draft と既存結果欄へ反映する。既存 OK で保存し、Cancel で破棄する。 |
| CPU/GPU/AI/キャッシュ tuning | §2 表の手動並列数・先読み・容量・backend 等は一組で除外。AI 利用範囲 ai_feature_mode は閲覧機能の選択として含め、性能注意は既存 UI で確認できる。tuning まで移すなら移行先性能の影響と対象分類を再検討する。 |
| 接続・自動通信 | Remote 全関連値と update_check_enabled は除外して移行先を保持。「取り込みで自動的に通信を有効化しない」を簡単に守る。更新確認まで移す代案は通信 opt-in の扱いを別途説明する必要がある。 |
| 起動フォルダのモードだけ移すか | パスと一組で除外。Specific だけ載せると移行先の古い specific path が採用されるので、部分移行をしない。 |
| 履歴保持件数 | 除外。取り込みの OK が移行先の履歴を prune しない。含める代案は「既存履歴が減る」仕様の明示承認が必要で、保存経路に import 専用例外を足す案は推奨しない。 |
| EXIF 非表示タグ | 任意文字列を含むため field 全体を初期除外。組込みタグだけ移す代案は custom tag を保持して既知 subset だけ合成する仕様/検証が増える。全任意文字列の無検証転送は採らない。 |
| 画像拡張子の優先順 | 組込み候補の完全な順序だけ含める。不正/追加候補があれば field 全体を省いて知らせる。Susie 固有候補まで移す案はインストール依存になり初期要件から外れる。 |
| 動画下部バー・ストリップ固定 | 2 フィールドとも初期除外。固定 ON が scope 外のストリップ state を復元するため。含めるなら既存 setter のこの付随変更を仕様上許容するか判断し、scope 外の既存値保持に例外を明記して回帰を追加する。状態を直接書き換えて setter を迂回する案は採らない。 |
| デインターレース | 移行先で性能を見て選ぶ tuning として除外。見え方の好みとして含めたい場合は enum の export 1 行とテストを追加できる。 |
| 未来の形式版 | 同じ v1 の未知項目は無視、破壊的な未来 v2 は全体拒否。未来版を読み取れる限り読む案は意味変更の推測を要するので採らない。 |
| 結果確認 / 不正 / まれな失敗 | 正常値だけ draft に載せ、不正項目一覧を見て OK または Cancel。書込み/全体読込失敗は通知して転送の設定変更なし。DB 保存失敗は既存通知に委ねる。backup世代選択・途中保存・rollback・retry・journal は追加しない。 |
| ファイル保存 | 同じ親の temp → 一度だけ既存 atomic replace helper。既存共有と同じ UI に揃えつつ、既存 export の truncate は避ける。失敗時の temp cleanup は best-effort、追加 recovery は設計しない。 |
| 詳細表示下部バーのモード | details_selection_bar_mode を除外。Dedicated への変更による除外列の A→C 複製を取り込みから発生させない。 |
| スタック script の有効化 | stack_script_enabled を除外。移行先の stack_rules.rhai に依存するため、本体を転送せず有効化だけ移す案は採らない。 |
| 既存 OK の限界 | §1.305 の最新 live 値巻き戻りと §1.295 の UI 同期再読込は今回未修正。取り込みも手編集と同じ OK 経路を使い、各 backlog に継続影響を記録する。 |
| 入力遮断の範囲 (独立レビュー後の変更) | メイン viewport の環境設定/common modal 内だけ。ファイル I/O と draft に無関係な別窓・fullscreen・native 動画の転送専用遮断はすべて撤去し、HEAD と同じ挙動へ戻す。detached 凍結ルールの構造合意は得られておらず、今回 detached を変更しない。 |
| 通信除外の試験 (P2) | 三つの自動有効化 field の出力キー不在と true 入力時の false 保持を、policy 非依存の明示 assertion で固定する。生成された分類/保持 assertion のみでは受入条件を満たさない。 |

受入条件は、(1) 全フィールドの分類 gate、(2) 個人情報・パスが出力されない、
(3) 対象外 draft と利用データを保持 (既存 OK の限界は §2.3)、(4) draft → 既存 OK の一経路、
(5) 壊れた入力/未来の破壊的形式は変更なし、(6) 通信・自動有効化を増やさない、の六点。
上記は実装範囲の承認であり、実装・テスト・独立レビューの検収完了を意味しない。

## 8. 実装・検証の記録 (2026-10-04)

以下の §8.1–§8.4 は、入口を環境設定の全体設定に置いていた時点の実装・検証履歴。
SettingsRestoreState へ入口と job owner を移す今回の変更に対する検証結果ではない。
変更がない分類・変換等の証拠は範囲を確認して再利用し、入口・所有・UI の現在の証拠は別途追記する。

### 8.1 変更ファイルと確認した境界

- 変換と分類: `src/settings_transfer.rs` (新規)、登録 `src/lib.rs`。
- draft と UI: `src/ui_dialogs/preferences/transfer.rs` (新規)、`preferences.rs`、
  `preferences/pages.rs`、`preferences/search_index.rs`。
- draft 収納: `src/app.rs` の二つの Box owner のみ。入力受付は既存 common modal を使い、
  `src/ui_main.rs`、`src/ui_dialogs/settings_restore.rs`、別窓・native・sidecar の変更は撤去。
- 試験: draft 収納に合わせた既存 fixture の `src/app/tests.rs`、`tests/ui_snapshot.rs` と
  `tests/snapshots/preferences_transfer_{light,dark,narrow_result,busy_dark}.png`。
- 文書: 本書、`docs/{README,architecture-overview,spec,keymap-spec,next-release-backlog}.md`、
  `htdocs/mimageviewer/{index,privacy}.html`、`htdocs/mimageviewer/manual/settings.html`。

| 実装時の根拠 | 確認した前提 |
| --- | --- |
| `src/settings_transfer.rs:179`、`:252` | 唯一の policy から全フィールドの列挙・対象 projection・型/範囲検証・適用・分類試験を生成。`..` のない Settings 分解で未分類をコンパイル拒否する。 |
| `src/ui_dialogs/preferences.rs:1303`、`:1971`、`:2546`、`:2547`、`:2690` | 標準設定の draft 生成から、本番 OK の prepare → install → save_checked の一経路を使う。転送完了は draft のみ編集。 |
| `src/ui_dialogs/preferences/transfer.rs:8`、`:40`、`:67`、`:99` | receiver は PreferencesState の一つの enum が所有する。worker に JSON/I/O を置き、UI は try_recv と型付き draft 適用だけを行う。 |
| `src/settings_transfer.rs:794`、`:810`、`:827` | 上限付き読込、同じ親の一時ファイル、flush/sync、既存 atomic replace を利用。DB schema と Settings 本体には変更なし。 |
| `src/settings.rs:9270`、`:9692`、`src/filename_stack_script.rs:81` | 列複製のモードと実ファイル依存の script 有効化を除外。既存 live merge の限界は §1.305 として維持。 |
| `src/ui_dialogs/preferences.rs:2791`、`src/app.rs:22471` | 取り込み後の OK も既存の再読込を使うため、§1.295 の同期走査を継承。 |
| `src/app.rs:19835`、`src/ui_dialogs/preferences.rs:2854` | show_preferences は既存 common modal に登録済み。メインの処理中 Modal を使い、転送専用の別窓/input guard は不要。HEAD 比較で撤去を確認する。 |
| `src/app.rs:13884`、`:13888`、`src/ui_dialogs/preferences.rs:3141` | 大きな二つの draft は Box に収納し、private 操作カスタマイズ適用 helper も Box を消費する。owner / 確定経路は変えない。 |

### 8.2 自動検証

以下は入力遮断の範囲変更前の実行結果。初回の試験 fixture の前提違いは修正した。
**今回の範囲変更後の結果は §8.4 に記録し、古い成功を修正後ソースの証拠として扱わない。**

| コマンド / 確認 | exit code / 結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 |
| `cargo test -p mimageviewer --lib settings_transfer` | 0、14 件 |
| `cargo test -p mimageviewer --lib preferences_transfer` | 0、16 件 |
| `cargo test -p mimageviewer --lib preferences` | 0、110 件 |
| `cargo test -p mimageviewer --lib history_transition_storage_keeps_app_stack_footprint_bounded -- --nocapture` | 0、1 件。二つの draft の Box 化後の App は 90,312 bytes、既存上限 `<110,128` を維持。 |
| `cargo test -p mimageviewer --lib capture_region_focus_and_escape_terminals_preserve_event_order -- --nocapture` | 0、1 件。標準スタックで既存試験を変更せず成功。 |
| `cargo test --test ui_snapshot` | 0、69 件。新規 4 画像は実 renderer から生成し、明暗・狭幅・処理中を目視。fixture の幅だけ本番の右パネルへ揃えた最終画像も検証した。 |
| `python scripts/check_ui_glyphs.py` | 0、危険な glyph なし |
| `git diff --check` | 0 |
| 未分類フィールドの一時追加 | `#[serde(skip)] pub(crate)` の unit フィールドと Default 初期値を一時追加し、`settings_transfer.rs:180` の網羅分解で cargo check が 101 (期待どおり)。元の settings.rs は byte 単位で復元し、通常 check は 0。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0、PASS。メイン lib は 10,265 passed / 52 ignored。workspace / integration / snapshot / doctest と vendor の egui / egui-wgpu / eframe まで完走。初回は launcher の release 入力 3 本の欠落 (exit 1)、次は App の既存サイズ gate (exit 1)、結果表示のみ Box 化した回は既存キャプチャ試験の stack overflow (exit 101)。最終収納修正後は標準スタックで成功。ログ `target/preferences-transfer-full-final.log`。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0。範囲変更前のソースから core / remote / EPUB PDF worker を dev-runtime に用意。VCRT 検査 runtime=4 / pe=3 も成功。ログ `target/preferences-transfer-build-dev-final.log`。製品起動なし。 |

全体テスト用の launcher 入力は、既存 `C:\home\mimageviewer\target\release` の
2026-10-04 04:45 の core / remote / EPUB worker の実ファイルをこの worktree の
`target/release` へコピーし、SHA-256 一致を確認した。履歴は
`target/preferences-transfer-launcher-prerequisites.json`。これは launcher の埋込み試験の
前提であり、今回の実装を確認する実行ファイルではない。今回の確認用 core は
範囲変更前のソースから dev-runtime に前回 build 済み。範囲変更後の再 build は §8.4 に記録する。
いずれの製品実行ファイルも起動していない。

最終ログは `target/settings-transfer-test-final.log` (14 件)、
`target/preferences-transfer-test-final.log` (16 件)、
`target/preferences-regression-test-final.log` (110 件)、
`target/preferences-transfer-check-final.log`、`target/preferences-transfer-snapshot-final.log` (69 件)、
`target/preferences-transfer-stack-test-final.log`、`target/preferences-transfer-capture-test-final.log`。
分類 gate の負例ログは `target/preferences-transfer-classification-probe.log`。

通し試験では、実 PreferencesState に worker の結果を取り込み、本番 OK をクリックし、
DB 再読込・load-time migration・開き直しまで確認した。別 data-dir の対象外全フィールド
(互換 mirror の明示例外を除く)、★・履歴・タグ・collection の実ストアと編集/本/キャッシュの
独立ファイルを比較した。Cancel は本番の破棄確認を通して DB が変わらないことを確認。
IME fake-input、処理中の OK/Cancel/×/Escape、メインの背面メニュー・ツールバーの本番 UI 入力を試験する。
別窓の遮断試験と native/passive の転送専用 fixture は撤去した。

### 8.3 独立レビューと残る確認

前回報告後の独立 `gpt-6.1-sol` / `xhigh` レビューで P2 が二件あり、detached の構造的修正への
合意も得られなかった。設計担当は入力遮断をメインの環境設定/common modal 内へ縮小し、
通信自動有効化の除外を独立した明示テストで固定すると決定した。処理中 DOWN → 完了 → UP を
新しい短い右クリックとして解釈する P2-1 は、別窓の転送専用遮断を経路ごと撤去して解消する。
以下の draft 収納は別窓の入力遮断とは別の変更で、維持する。修正後の限定独立再レビューと全体テストは完了し、新たな指摘なし。
転送状態追加後の App は 110,200 bytes となり既存サイズ gate に失敗した。結果表示だけの
Box 化では 110,088 bytes となってサイズ gate は通ったが、複数 App を生成する既存
キャプチャ試験で標準スタックの overflow が残った。独立レビューの構造合意に基づき、
App が持つ二つの draft を `Option<Box<PreferencesState>>` とし、結果表示の個別 Box は戻した。
操作カスタマイズの private 適用 helper も Box を消費し、大きな State 全体をスタックへ戻さない。
App は最終的に 90,312 bytes。実際の収納差分も独立再レビュー済みで、未解消の指摘なし。
既存の `<110,128` bytes gate とキャプチャ試験、標準スタックサイズは変更しない。

利用者の実機確認と設計担当の検収は未実施。§6.2 の別 data-dir 往復、取り込み後の
OK/Cancel、対象外パス/操作設定と自動通信 OFF の保持、実 IME の Enter/Escape を確認する。
別窓・native 動画の操作は転送のために遮断せず、既存挙動を使う。既知の §1.305 / §1.295 は承認どおり未修正。
コミット・製品起動・通常 APPDATA のテスト操作は行っていない。

### 8.4 入力遮断の範囲変更後の検証

2026-10-04 の設計担当決定で、転送用の別窓・fullscreen・native 入力遮断と
その fixture/test を撤去した。`src/ui_fullscreen.rs`、`src/app/native_video.rs`、
`src/app/sidecar_restore.rs`、`src/ui_main.rs`、`src/ui_dialogs/settings_restore.rs`、
`docs/detached-rework-plan.md` は HEAD と内容差分ゼロ。
`src/app.rs` は二つの draft を Box に収納する差分のみで、別窓の関数・native HUD は HEAD と同じ。
比較記録は `target/preferences-transfer-scope-head-comparison.json`。改行コードを正規化した
全文の一致を assertion し、App も Box の二つの型変更以外の全文が HEAD と一致することを確認した。

撤去した主な受付経路は `detached_window_can_activate`、
`queue_deferred_detached_window_activation`、`queue_recognized_detached_right_drag_command`、
`drain_deferred_detached_activation_watcher`、`execute_pending_right_drag_command_in_mounted_context`、
`activate_detached_image_window_snapshot`、`native_video_hud_dimmed_for_current_poll`、
`apply_detached_image_window_event_batch` と active/passive viewport の描画入口。
`native_video_mouse_seek_hold_valid`、native event 受付、
`maybe_open_native_video_secondary_long_press_menu` も HEAD へ復元した。
追加していた `native_video_event_blocked_by_preferences_transfer`、
`consume_blocked_modal_viewport_input`、`consume_preferences_transfer_viewport_input` は削除した。
転送専用のメニュー/ツールバー disable と設定復元/操作カスタマイズの入口 guard も撤去した。
新規だった `src/app/preferences_transfer_tests.rs` と `src/ui_fullscreen/preferences_transfer_tests.rs` は削除し、
純粋な job 二重開始/drop とメイン UI の試験は環境設定の transfer test に残した。

通信三フラグの出力キー不在/true 入力からの false 保持を、生成 helper を使わない純粋テストと
実 PreferencesState の worker → 本番 OK → DB 再読込の明示 assertion で固定した。
背面 UI 試験は本番環境設定/root modal を描画し、実メニューヘッダー・ツールバーへ
raw pointer 入力する。単に消去済み popup 項目の旧座標をクリックするだけの証拠にはしない。
完了だけでは環境設定の common modal は残り、閉鎖後に各メニュー handler とツールバーが通ることも確認する。

関連試験の初回は本番の busy spinner が継続 repaint するため `Harness::run` の max_steps に達した
(exit 101)。busy 中は時間制限を緩めず、既存の `run_steps` / `step` で必要な pass 数だけ実行する fixture に修正した。
修正後の個別テスト・全体 gate・確認用 build の結果を以下に記録する。

独立 `gpt-6.1-sol` / `xhigh` の限定再レビューで、メニュー試験は消去済み popup 項目の
旧座標への click だけでは証拠にならないと指摘された。実ヘッダーへの入力・項目不在・
閉鎖後の各 command handler の positive control を追加し、指摘を解消した。
不在確認に `get_all_by_label` (不在なら panic) を使った試験のミスも `query_by_label` に修正した。
本番の処理中 Modal だけを一時無効にした負例では、DateDesc から RatingDesc への
背面 toolbar の不正変更を検出して exit 101。ログ `target/preferences-transfer-main-modal-negative.log`。
preferences.rs は finally で元 bytes を復元し、その後の限定再レビューも新たな finding なし。
転送 busy の本番参照は Preferences 内だけ、指定 6 ファイルと App の HEAD 比較も再確認済み。

| 修正後のコマンド / 確認 | exit code / 結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 0 |
| `cargo test -p mimageviewer --lib settings_transfer` | 0、15 件。`target/settings-transfer-scope-test.log`。 |
| `cargo test -p mimageviewer --lib preferences_transfer` | 0、9 件。`target/preferences-transfer-scope-test.log`。 |
| `cargo test -p mimageviewer --lib preferences` | 0、103 件。`target/preferences-scope-regression-test.log`。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。`target/preferences-transfer-scope-check.log`。 |
| `cargo test --test ui_snapshot` | 0、69 件。`target/preferences-transfer-scope-snapshot.log`。 |
| `python scripts/check_ui_glyphs.py` | 0、危険な glyph なし。 |
| 指定 6 ファイルの `git diff --exit-code` | 0。`--stat` の出力も空。 |
| `git diff --check` | 0。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0、PASS。メイン lib は 10,259 passed / 52 ignored。workspace / integration / snapshot / doctest と vendor の egui / egui-wgpu / eframe まで完走。`target/preferences-transfer-scope-full.log`。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0。修正後ソースから core / remote / EPUB PDF worker を dev-runtime に再 build。VCRT 検査 runtime=4 / pe=3 も成功。`target/preferences-transfer-scope-build-dev.log`。製品起動なし。 |

### 8.5 「設定の復元」への入口移動 (2026-10-04 利用者要望)

利用者の実機確認後、§4 の入口を「設定の復元」へ移した。今回の変更は転送 UI・
環境設定への受渡し・関連試験/文書だけ。開始時からあるファイル整理・本の読書位置等の
未コミット差分を保持し、コミットや製品バイナリ起動は行わない。

| 確認した file:line | 前提 / 変更 |
| --- | --- |
| `src/ui_dialogs/settings_restore.rs:1153`、`:1164` | 設定復元の既存画面に二つの入口を配置。他の復元・操作共有の処理や子ダイアログがある間は開始しない。 |
| `src/app.rs:19783`、`:19835`、`:19843` | 環境設定と設定復元は既存 common modal に登録済み。新しい main / detached 入力 owner は作らない。 |
| `src/ui_dialogs/settings_restore.rs:41`、`:413` | 説明・単一 worker receiver・結果を一つの Box owner に収納。説明中/処理中は親の操作・閉鎖を止め、child Escape を親へ漏らさない。 |
| `src/ui_dialogs/preferences/transfer.rs:65`、`:102`、`src/settings.rs:8382` | 説明の確認後だけファイル選択へ進み、確定済み標準設定の preferences_snapshot から portable projection を書き出す。favorite overlay や未確定 draft は出さない。 |
| `src/ui_dialogs/preferences/transfer.rs:130`、`src/ui_dialogs/preferences.rs:2143` | worker の検証成功後だけ、既存初期化 helper で新規 draft を生成し、取り込み結果を載せて General を開く。 |
| `src/ui_dialogs/preferences/pages.rs:28` | 旧「設定の持ち運び」入口・検索 anchor を削除。取り込み結果欄は全体設定の先頭で表示。 |
| `src/ui_dialogs/preferences.rs:2535`、`:2536`、`:2679` | OK の prepare → install → save_checked は既存経路のまま。Cancel は既存の破棄確認へ通す。 |

独立 `gpt-6.1-sol` / `xhigh` は単一 owner → fresh draft → 既存 OK の設計境界を確認した。
試験の補強として、背面の環境設定入口の遮断/解除と、非処理中の通常 Escape が
説明だけを閉じることを追加した。初回の「製品コードに修正必須の指摘なし」は、
以下の追加確認で見つかった未確定 draft の破棄経路について撤回した。
`ui_main.rs:6657`、`:7690`、`:7713` のメニューには pointer の common-modal guard がなく、
通常の環境設定は `egui::Window` である。common predicate の登録だけでは、環境設定と
設定復元をどちらの順でも同時に開くことを止めない。現在の取り込み成功処理は新規 draft を
作るため、その状況では既存の未確定 draft を捨てる。これは利用者の前提との矛盾であり、
環境設定表示中の復元入口を拒否するか、既存復元 UI を保って転送入口だけを止めるかの
判断を利用者へ照会した。設計担当は案 2 (既存復元を保ち、新しい転送だけを制限) を採用した。
両順の保護と修正後の検証は §8.6 に記録する。§8.5 の成功はその修正前の履歴である。
テストの初回コンパイルは新 fixture の型指定/借用順序で exit 101。製品 core check は 0。
fixture を修正し、修正後の検証結果を以下へ記録する。

| 今回のコマンド / 確認 | exit code / 結果 |
| --- | --- |
| `cargo fmt` | 0。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。`target/preferences-transfer-entry-check.log`。 |
| `cargo test -p mimageviewer --lib settings_transfer` | 0、15 件。`target/preferences-transfer-entry-settings_transfer.log`。分類・通信三フラグ・形式検証・atomic replace は変更していない。 |
| `cargo test -p mimageviewer --lib preferences_transfer` | 0、9 件。`target/preferences-transfer-entry-preferences-transfer.log`。 |
| `cargo test -p mimageviewer --lib preferences` | 0、103 件。`target/preferences-transfer-entry-preferences.log`。 |
| `cargo test -p mimageviewer --lib settings_restore` | 0、28 件。`target/preferences-transfer-entry-settings_restore.log`。 |
| `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot preferences_transfer` | 0、8 件。入口の明暗/書き出し・取り込み説明を追加し、結果の明暗/狭幅/処理中を更新。全 8 画像を目視確認。初回は busy spinner の継続 repaint で exit 101。処理中画像だけ run_steps(4) とし、他画像の既存 settle 動作は維持した。 |
| `cargo test --test ui_snapshot` | 0、75 件。`target/preferences-transfer-entry-snapshot.log`。 |
| `cargo fmt --check`、`python scripts/check_ui_glyphs.py`、`git diff --check` | 0。危険な glyph なし。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0、PASS。メイン lib は 10,261 passed / 52 ignored (873.13 秒)。workspace / integration / ui_snapshot (75 件) / doctest と vendor 3 crate を完走。`target/preferences-transfer-entry-full.log`。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 未実施。同時表示時の draft 保護を含む前提修正を待つ。現在の自動 gate の成功を、未解決の入口問題の検収として扱わない。製品起動なし。 |

別窓・native・メインメニューの source、App 本体、分類 module、製品ページ/privacy は
今回変更していない。開始時の bytes と SHA-256 を比較して一致を確認した。
記録: `target/preferences-transfer-entry-untouched-baseline.json`。

### 8.6 案 2: 既存操作を保持し、転送入口と取り込み受入で draft を保護

設計担当の決定 (2026-10-04) により、既存の「この時点に戻す」、完全リセット、
操作カスタマイズの共有等は変更しない。表示述語は既存の show_preferences だけを使い、
新しい状態を追加しない。書き出し・取り込みの入口は両方とも表示中に無効化して揃えた。
受付済みの書き出しは確定値だけを読むため、後から環境設定が開いても完了させる。
取り込みは poll の成功結果受入一か所で表示状態を確認し、開かれていれば既存の失敗通知へ
移して終了する。新規 draft 生成・既存 draft の置換・live 変更・DB 保存を行わない。

試験は両画面をどちらの順でも開いて新転送ボタンの disabled と既存復元ボタンの enabled を
確認し、環境設定を閉じれば転送を再度使えることを確認する。逆順の読込完了では
draft の pointer / 設定 / 検索入力、live 設定、DB と save generation の不変、通知の描画を確認する。
追加確認で、失敗通知が busy=false のため背後の環境設定にも raw Escape が届くことを確認した。
実際に転送 Modal を描く条件 (show_settings_restore と既存 owner の存在) を派生述語にし、
環境設定の UI / Enter / Escape / 閉鎖受付 / 破棄確認の表示を同じ条件で止める。
新しい永続状態や別窓の制約は追加しない。通知の Escape と書き出し結果の Enter は、
本番の描画順 (環境設定 → 設定復元) で背後の検索入力・draft を保つことを試験する。
修正後の自動検証・確認用 build の結果を以下に記録する。

| 確認した file:line | 案 2 の境界 |
| --- | --- |
| `src/ui_dialogs/settings_restore.rs:1159`、`:1174`、`:1179`、`:1186` | show_preferences から新しい二つの入口だけを無効化し、disabled tooltip で理由を表示。既存の復元入口は変更しない。 |
| `src/ui_dialogs/preferences/transfer.rs:175` | worker 成功結果の受入一か所で既存の表示 flag を確認し、独立して開いた draft を置き換えない。 |
| `src/ui_dialogs/preferences/transfer.rs:48`、`src/ui_dialogs/preferences.rs:2079`、`:2251`、`:2836` | 実表示条件から UI / キー / 閉鎖 / 破棄確認を保護。worker busy とは別の派生述語で、状態は増やさない。 |
| `src/ui_dialogs/preferences/transfer.rs:689`、`:741`、`:837` | 両順の入口無効化・既存復元維持、完了時の draft / live / DB 保護、通知 Escape / Enter の入力漏れを本番順で試験。 |

限定独立再レビュー (`gpt-6.1-sol` / `xhigh`) は表示境界の製品修正に追加指摘なし。
試験の本番描画順と一致する検索語についての助言を反映した。
初回の保護追加後テストは 10 pass / 1 fail (exit 101)。再確認用の環境設定を開いたまま
次の取り込みへ進む旧 fixture を、通常のキャンセルで閉じてから次の取り込みへ進む形に修正した。

| 修正後のコマンド / 確認 | exit code / 結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 0。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。`target/preferences-transfer-final-check.log`。 |
| `cargo test -p mimageviewer --lib settings_transfer` | 0、15 件。`target/preferences-transfer-final-settings_transfer.log`。分類 gate・通信三フラグ・検証・atomic replace は維持。 |
| `cargo test -p mimageviewer --lib preferences_transfer` | 0、12 件。`target/preferences-transfer-final-preferences_transfer.log`。両順の保護と通知のキーボード境界を含む。 |
| `cargo test -p mimageviewer --lib preferences` | 0、106 件。`target/preferences-transfer-final-preferences.log`。 |
| `cargo test -p mimageviewer --lib settings_restore` | 0、28 件。`target/preferences-transfer-final-settings_restore.log`。 |
| `cargo test --test ui_snapshot` | 0、76 件。`target/preferences-transfer-final-snapshot.log`。新しい無効状態の dark snapshot は生成後に目視確認。 |
| `python scripts/check_ui_glyphs.py` | 0、危険な glyph なし。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0、PASS。メイン lib は 10,264 passed / 52 ignored (884.84 秒)。workspace / integration / ui_snapshot (76 件) / doctest と vendor 3 crate を完走。`target/preferences-transfer-final-full.log`。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0。core / remote / EPUB PDF worker を通常 feature の dev-runtime で作成。VCRT/PE 検査も成功。`target/preferences-transfer-final-build.log`。製品起動なし。 |
| `git diff --check`、保護ファイルの SHA-256 比較 | 0。前段変更・別窓・native・分類・製品ページ/privacy の保護対象は開始時 bytes と一致。 |

§1.263 の表形式化・§1.256 の左→右化のコードと関連画像は変更していない。
開始時の SHA-256 と照合し、一致を確認 (`target/preferences-transfer-guard-protected-files.json`)。
別窓・native・App・メインメニュー・分類・製品ページ/privacy の bytes も前記 baseline と一致。
コミット・製品バイナリ起動は行わず、実機確認と設計担当の検収は利用者へ引き継ぐ。

### 8.7 P2: 操作カスタマイズの「押して入力」を転送 Modal の背面で捕捉しない

独立レビューで、操作カスタマイズは環境設定より後、設定復元より前に描かれるため
(`src/app.rs:85115` / `:85117` / `:85118`)、転送 Modal の Enter / Escape を背面の
割り当て編集が先に捕捉する経路が指摘された。既存の捕捉可否は IME だけを見ていた。
`show_operation_customize_dialog` で IME または既存の preferences_transfer_dialog_open を
使って判定し、割り当て編集から poll_command_chord_capture まで同じ値を渡す。
引数名を keyboard_capture_blocked に揃え、IME の状態を偽装したり新しい状態を足したりしない。
捕捉の入口で停止するため Win32 key edge と egui event の両経路で消費も draft 変更も行わない。

本番順の Harness は説明の Escape / 通常キー、完了通知の Enter / Escape、処理中の Enter /
Escape で、待機 slot・入力欄・編集対象・通知・エラー・live 設定と save generation の不変を
検証する。説明の Enter による native ファイル選択は headless 試験で起動しない。
Modal を閉じた後、通常の設定復元を開いたまま A が入力されて捕捉待機が終了することも確認する。
既存の復元項目・分類・DB・別窓の入力経路は変更しない。修正後の検証結果は完了後に記録する。

限定独立再レビュー (`gpt-6.1-sol` / `xhigh`) は追加指摘なし。native / egui の両捕捉経路より
前に停止し、停止した key edge は次 frame に持ち越して後日捕捉しないことを確認した。
実装境界は `src/ui_dialogs/preferences.rs:3033` と `pages.rs:4047` / `:4458`、
本番順の回帰試験は `src/ui_dialogs/preferences/transfer.rs:837`。

初回の全体 gate は exit 101。メイン lib は 10,266 pass / 1 fail / 52 ignored。
失敗した既存の往復試験は、処理中 spinner の継続 repaint に Harness::run の max_steps=4
で静止を要求していた (`transfer.rs:594`)。取り込み worker を待つ三つのループを
run_steps(2) に揃え、deadline・設定・対象外・DB・利用データ・Cancel の assertion は維持した。
製品の処理・待機・キャンセルは変更せず、修正した fixture で関連試験と全体 gate を再実行する。

| 今回のコマンド / 確認 | exit code / 結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 0。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。`target/preferences-transfer-capture-check.log`。 |
| `cargo test -p mimageviewer --lib preferences_transfer` | 0、13 件。`target/preferences-transfer-capture-preferences_transfer.log`。 |
| `cargo test -p mimageviewer --lib preferences` | 0、107 件。`target/preferences-transfer-capture-preferences.log`。 |
| `cargo test -p mimageviewer --lib settings_restore` | 0、28 件。`target/preferences-transfer-capture-settings_restore.log`。 |
| `cargo test --test ui_snapshot` | 0、76 件。`target/preferences-transfer-capture-snapshot.log`。画像変更なし。 |
| `python scripts/check_ui_glyphs.py` | 0、危険な glyph なし。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 修正後 0、PASS。メイン lib は 10,267 passed / 52 ignored (669.72 秒)。workspace / integration / ui_snapshot (76 件) / doctest と vendor 3 crate を完走。`target/preferences-transfer-capture-full-final.log`。初回 exit 101 の記録は `target/preferences-transfer-capture-full.log`。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0。通常 feature の core / remote / EPUB PDF worker を作成。VCRT/PE 検査も成功。`target/preferences-transfer-capture-build.log`。製品起動なし。 |
| `git diff --check`、保護ファイルの SHA-256 比較 | 0。保護対象の bytes と一致。 |

保護対象は `target/preferences-transfer-capture-protected-files.json` の SHA-256 と一致。
今回編集前から存在する §1.263 / §1.256 の未コミット変更と snapshot、既存復元、分類、
App・メインメニュー・別窓・native 動画、製品ページ/privacy は今回変更していない。
修正したのは捕捉判定と引数名、回帰試験、設計記録と keymap-spec の対応記述だけ。
spinner fixture の修正は cfg(test) の待機ループだけのため、製品コードと UI の変わらない
core check / settings_restore / ui_snapshot / glyph の成功を再利用した。
修正後の全体 gate でも設定復元と UI snapshot を再確認した。コミット・製品起動なし。
