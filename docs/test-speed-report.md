# テスト待ち時間の測定・改善記録

対象: `test-speed` worktree、基準 `f5d382385`（製品・テストコードは master `d3f2e913c` と同じ）。
製品コード・CLAUDE.md・依存バージョンは変更しない。

## 測定条件

- 2026-10-15、日本語 Windows、logical CPU 24。
- stable toolchain、通常の test profile、feature なし。
- `cargo test -p mimageviewer --lib --no-run --message-format=json` で特定した
  lib test executable を直接実行。実行時だけ `RUSTC_BOOTSTRAP=1` を設定し、
  `-Z unstable-options --format json --report-time --test-threads 24` を渡す。
  JSON と `exec_time` の取得に追加ツール・nightly・再ビルドは不要だった。
- 初回コンパイル時間は比較から除外する。未配置の LibRaw 0.22.2 / RAW samples は
  `G:\home\mimageviewer-nextfix\vendor` から、reparse point がないことを確認して実コピーした。
  FFmpeg DLL も既存 vendor から test executable の隣へコピーした。
- TurboJPEG の初回 MSBuild は並列実行で失敗し、`/m:1 /nr:false` で成功した。
  初回 Cargo コンパイルも `-j 1` で完了。テスト実行の並列数は前後とも24。
- 測定開始時に別セッションの Cargo（PID 109916 / 110632）が存在し、rustc はなかった。
  外部負荷の変動があるため、絶対値・小さな差より順位と割合を重視する。
- ログ: `target/test-speed-before.jsonl` / `target/test-speed-before.stderr.log`、
  `target/test-speed-before-summary.json` / `target/test-speed-before-load.json`。

## 読み方

libtest の各 `exec_time` は共有ロックの取得待ちも含む。24並列のため各テストの秒数を
足した値は全体の壁時計時間と一致しない。上位N件の割合は **テスト時間総和に対する割合**
として示し、壁時計時間に対する削減寄与と混同しない。

`setup_app_for_test` は `data_dir::test_override_lock()` を fixture の寿命全体で保持する。
App を必要とする別テストはその間直列化されるため、警告の出たテストが単独でも重いとは限らない。

## 作業A: 全件測定

全件は **13,149成功 / 0失敗 / 59 ignored**（計13,208件）、exit 0。
libtest の全体時間 **1,710.943秒（28分30.943秒）**、プロセス全体の壁時計 **1,713.900秒**。
個別時間は成功13,149件すべて取得でき、総和は **39,535.975秒**。
開始時に Cargo 2プロセス、rustc 0。測定途中も同じ Cargo だけが残り、終了付近は Cargo / rustc とも0だった。
別 worktree のコマンド特定（CIM）は sandbox で拒否されたため行っていない。

| 上位N件 | 個別時間総和に対する割合 |
| --- | ---: |
| 5 | 2.86% |
| 10 | 4.99% |
| 20 | 8.90% |
| 30 | 12.17% |
| 50 | 17.50% |
| 100 | 24.30% |

| 順位 | テスト | 秒 | 主な分類（待ちも含む） |
| ---: | --- | ---: | --- |
| 1 | `ui_fullscreen::tests::still_seek_f4_screen_last_pixel_closes_strip_with_real_geometry` | 286.216 | 共有Appロック / 180ケース / egui多フレーム |
| 2 | `ui_fullscreen::tests::raw_blocked_preview_settles_navigation_and_folder_lock_with_or_without_rendition` | 251.473 | 共有Appロック / 36ケース / 3×2のfake画像・LUT |
| 3 | `ui_fullscreen::tests::second_page_image_folder_strip_preview_survives_loaded_shift_s_strip` | 217.355 | 共有Appロック / ページfixture / egui多フレーム |
| 4 | `ui_fullscreen::tests::real_book_strip_press_previews_one_page_over_wide_and_compressed_spreads` | 201.834 | 共有Appロック / ページfixture / egui多フレーム |
| 5 | `ui_fullscreen::tests::original_preview_hold_remains_active_without_navigation_in_flight` | 175.404 | 共有Appロック / ページfixture / egui多フレーム |
| 6 | `ui_fullscreen::tests::held_page_turn_does_not_create_an_original_preview_context_blocker` | 170.345 | 共有Appロック / ページfixture / egui多フレーム |
| 7 | `ui_fullscreen::page_turn::cover_tests::page_turn_endpoint_override_keeps_saved_global_and_book_values_and_other_readers` | 169.422 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 8 | `ui_fullscreen::tests::fs_zoom_video_context_collapses_state_and_tracks_the_held_zoom_key` | 167.923 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 9 | `ui_fullscreen::tests::g1_extracted_key_handlers_ignore_old_db_rows_and_never_persist` | 167.610 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 10 | `ui_fullscreen::tests::capture_region_focus_and_escape_terminals_preserve_event_order` | 164.973 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 11 | `ui_fullscreen::tests::g1_extracted_key_fixture_exposes_foreign_native_frame_interference` | 164.674 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 12 | `ui_fullscreen::tests::fs_zoom_key_still_consumes_continuous_reading_z_and_reports_why` | 160.473 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 13 | `ui_fullscreen::tests::fs_zoom_key_leaves_video_context_z_press_for_its_owner` | 158.410 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 14 | `ui_fullscreen::tests::fs_cursor_zoom_aim_start_hides_cursor` | 157.447 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 15 | `ui_fullscreen::tests::fs_cursor_zoom_aim_falling_edge_restores_cursor` | 156.110 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 16 | `ui_fullscreen::tests::original_preview_hold_is_inactive_while_page_turn_input_is_held` | 155.003 | 共有Appロック / ページfixture / egui多フレーム |
| 17 | `ui_main::grid_reclick_open_tests::grid_cursor_rating_grid_undo_restores_shift_origin_in_thumbnail_and_details` | 153.864 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 18 | `ui_main::grid_reclick_open_tests::grid_cursor_rating_next_previous_and_ctrl_z_rebase_shift_click_in_both_views` | 150.569 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 19 | `ui_fullscreen::tests::original_preview_hold_is_inactive_while_navigation_sequence_is_in_flight` | 149.689 | 共有Appロック / ページfixture / egui多フレーム |
| 20 | `ui_main::grid_reclick_open_tests::grid_cursor_rating_last_page_return_and_ctrl_z_rebase_shift_click_in_both_views` | 140.628 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 21 | `ui_main::section207_tests::section207_production_layout_all_controls_and_positions_keep_input_usable` | 138.852 | 共有Appロック / 36ケース / egui・フォント |
| 22 | `ui_main::grid_reclick_open_tests::rating_grid_right_drag_start_then_center_cancel_keeps_explicit_selection_for_undo_redo` | 135.316 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 23 | `app::rating_membership_tests::rating_undo_grid_unadmitted_keys_and_automatic_cursor_moves_keep_held_restore` | 130.129 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 24 | `app::section375_tests::section1375_automatic_origin_survives_match_diff_restart_resume_and_defer` | 129.707 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 25 | `ui_fullscreen::page_turn_submission_tests::page_turn_actual_wheel_producer_overrides_unrelated_key_identity` | 128.418 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 26 | `ui_fullscreen::page_turn_submission_tests::page_turn_actual_root_close_is_resolved_before_continuation_capture` | 127.622 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 27 | `ui_fullscreen::page_turn::permission_service_regression_tests::permission_service_handlers_pdf_zip_automatic_and_composed_turn_on_receipt` | 127.411 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 28 | `ui_fullscreen::page_turn_submission_tests::page_turn_actual_navigation_waits_for_adopted_flat_then_bookmark_uses_new_page` | 126.454 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 29 | `ui_fullscreen::page_turn::slideshow_regression_tests::slideshow_regression_short_interval_and_disabled_animation_keep_flat_cadence` | 124.870 | 共有Appロック / 実DB fixture / 入力・状態遷移 |
| 30 | `ui_fullscreen::page_turn::slideshow_regression_tests::slideshow_regression_expired_outgoing_deadline_turns_and_preserves_full_display_interval` | 124.476 | 共有Appロック / 実DB fixture / 入力・状態遷移 |

補足:
- `raw::tests::all_cc0_samples_info_develop_and_preview_orientation`: **123.108秒**。CC0実ファイル・LibRawフル現像・プレビュー照合。
- `video::tests::real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch`: **79.860秒**。実FFmpeg・2 sample rate×6 WAV/MKV・各3 EOF loop・5ms callbackの実時間 pacing。
- section207 snapshot label: **110.036秒**、reported controls: **120.004秒**。それぞれ24 / 12ケースとApp・font・egui準備。
- `src/app/tests.rs` の sleep は主にworker完了を待つ1 / 2 / 5 / 10msの条件付きpoll。120ms固定待ちは1か所。
  既存collectionの `TestReadClock` はcollection固有であり、decoder・App初期化の時計を置換するものではない。
- `tests::all_cc0_sample_dimensions_match` は本体libではなく `libraw-sys` のテスト。上の13,208件と順位には含まれない。
- テスト内の子プロセスがpretty形式の別集計をstdoutへ出す。JSONの本体suite終端と個別testイベントを集計し、子の件数を二重計上しない。

### 主要候補の単独実行

同じ実行体を1 thread・exact filterで実行し、9成功 / 0失敗（73.627秒）。
ログ: `target/test-speed-isolated-before.jsonl` / `target/test-speed-isolated-before.stderr.log`。

| テスト（末尾名） | 24並列の全件測定 | 単独実行 |
| --- | ---: | ---: |
| rating_undo_grid_unadmitted_keys_and_automatic_cursor_moves_keep_held_restore | 130.129 | 2.856 |
| section1375_automatic_origin_survives_match_diff_restart_resume_and_defer | 129.707 | 1.928 |
| shared_navigation_fixture_uses_an_explicit_no_sidecar_profile | — | 0.184 |
| raw_blocked_preview_settles_navigation_and_folder_lock_with_or_without_rendition | 251.473 | 6.851 |
| still_seek_f4_screen_last_pixel_closes_strip_with_real_geometry | 286.216 | 38.072 |
| still_seek_overlay_requests_survive_continuous_landing | 93.349 | 2.463 |
| section207_production_layout_all_controls_and_positions_keep_input_usable | 138.852 | 7.454 |
| section207_reported_controls_keep_left_input_right_order | 120.004 | 6.518 |
| section207_snapshot_label_keeps_input_minimum_order_and_panel_bounds | 110.036 | 7.290 |

100秒超のAppテストが単独では2〜3秒で終わる例を確認した。警告回数・全件測定の順位だけで
通常の回帰テストをignoreへ移すのは適切ではない。F4の180ケース、section207の準備反復、
実デコーダ・全RAW sampleが次の検討対象であり、改善は未実装。

## 作業B: 高速化と全体ゲートの維持

未実施。以下のGit書き込み制限を停止条件の「前提の矛盾」と判断した。
高速化・ignore追加・assert変更・テスト削除は行っていない。作業後の全件時間は未測定。

## 作業C: 実行道具の検証

未実施。C1台帳のhit / miss / 差分時再実行、C2の0件一致エラーは未検証。
検討した設計は、全追跡ファイルと追加ファイルの内容ハッシュ、worktree内の
`target/test-lib-full/`台帳、lib本体の最後の集計を採用する件数判定。
追跡3,800ファイルを.NETで直接読む試測は0.865秒であり、作業ツリー・indexの変更は不要。

## 最終ゲート・残課題

`scripts/test-full.ps1` は未実行。Aの測定と単独9件だけ実行し、製品・テストコードは変更していない。
`cargo fmt` は実行済みで、追跡ファイルへの変更はない。CLAUDE.mdも未変更。

停止理由: `git add -- docs/test-speed-report.md` と、指定された
`git commit -m "docs: Record full lib test timings and lock contention" -- docs/test-speed-report.md`
を試みたが、次のエラーでindex更新ができなかった。

```text
fatal: Unable to create 'C:/home/mimageviewer/.git/worktrees/mimageviewer-testspeed/index.lock': Permission denied
error: pathspec 'docs/test-speed-report.md' did not match any file(s) known to git
```

現在の書き込み可能rootは `G:\home\mimageviewer-testspeed` であり、linked worktreeのGit管理領域は
そこに含まれない。commit可能な前提が成立しないため、briefの停止条件に従ってAの成果を保存して停止した。
branchは `test-speed` のまま、コミットは0件、masterへのmergeは行っていない。
本報告は未追跡ファイルとして残る。B・C・最終gateの続行には、worktreeのGit管理領域へ
書き込める実行環境が必要。測定ログと実行体はtarget内に残してあり、Aの再測定は不要。
