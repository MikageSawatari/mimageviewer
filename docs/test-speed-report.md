# テスト待ち時間の測定・改善記録

対象: `test-speed` worktree、基準 `f5d382385`（製品・テストコードは master `d3f2e913c` と同じ）。
製品コード・CLAUDE.md・依存バージョンは変更しない。

## 独立レビュー前の最終結果（ec0eeebffまで）

| 検証 | 結果 |
| --- | --- |
| 作業A libtest | 1,710.943秒、13,149成功 / 59 ignored（同名3関数の二重登録を含む） |
| 修正後nextest本体lib全件 | **422.185秒、13,146成功 / 0失敗 / 59 ignored**、exit 0 |
| 比較 | **75.32%短縮、4.05倍**。重複3登録の除去後は13,205件、テスト名集合は一致 |
| C1の2回目 | **HIT、3.022秒、exit 0、新規run directory 0** |
| 最終cargo gateの本体lib | **13,146成功 / 0失敗 / 59 ignored**、2,118.17秒（pack-build-tools） |
| EOF負荷確認 | CPU使用率100%で20/20成功、負荷側24/24成功、計720 epoch |
| I2準備待ち | 関連3件×20回＝60実行すべて成功 |
| 最終test-full | **1回でPASS、exit 0、3,053.825秒（50分53.8秒）**。従来cargo testのまま |

全件のwrapper wallは430.623秒、Cargo command自体は424.459秒（上の422.185秒はsuite時間）。
通常test profile・featureなし・24並列・Aと同じnative/RAW fixture。開始時に別作業のrustcを2本観測し、
並列負荷はAと完全同一ではないため、絶対値は変動し得る。追加の負荷試験は終了させてから計測した。
ログ: `target/test-speed-c1-verified.log` / `target/test-speed-c1-verified-load-start.json` /
`target/test-speed-c1-hit.log`。成功receiptは
`target/test-lib-full/c0d544d7310803d5bf5b97484557ad6087f0011670bb3a25ee42b8dd094a9b98.json`、
source識別子は `18559d98db9e314ee671a1411a9517512542247cb0bc0d834a8ad796b13d90ed`。
test-full終了後も同じsource識別子であることを読み取りで確認した。失敗試行を成功へ読み替えず、
修正後の全件成功と上のreceiptをこの段階の証拠とする。独立レビュー後のP2対応・追加検証は末尾に記録する。

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

**状態: 本体全件・台帳HIT・最終test-fullが成功、実装と自動検証を完了。** 前回のEOF原因判定は以下の設計判断で訂正する。
テスト修正は `dfd01d4a6`、runner 4本は `d266be9c2` としてOpusがcommit済み。
以後もエージェントはcommitせず、追加のEOFテスト修正・config・文書を作業ツリーへ残す。

Aの報告はOpusが `b9bf6c578` としてcommitした。続行依頼により、以後はcommitせず
作業ツリーに変更を残す。Git管理領域の書き込み制限は続行の停止条件にしない。

### B1: nextestのプロセス分離

利用者が導入した `cargo-nextest 0.9.148` を使用（エージェントによるinstallなし）。
`cargo nextest run -p mimageviewer --lib --color never --no-tests fail`、
通常test profile・featureなし・24並列で **13,146成功 / 0失敗 / 59 skipped**、exit 0。
実行 **449.106秒（7分29.106秒）**、command全体452.304秒（compile 0.75秒等を含む）。
Aの1,710.943秒から **73.75%短縮、約3.81倍**。外部負荷・cacheの変動はあるが、
並列数・Cargo profile・native/RAW fixtureは同じ。観測したCargoは今回のrunnerだけでrustcはなし。
ログ: `target/test-speed-nextest-b1.log` / `target/test-speed-nextest-b1-summary.json` /
`target/test-speed-nextest-b1.junit.xml` / `target/test-speed-nextest-b1-load.json`。

件数差3件は、次の関数に `#[test]` が二重指定されていたことによる。
libtestの `--list` に同一名が各2回現れ、nextestの一覧は名前単位なので各1件だった。
一覧の**テスト名の集合は完全一致**、ignoredも59件で一致した。

- `app::tests::favorite_adjustment_defaults_tests::the_info_panel_lock_and_open_belong_to_one_viewer_context_each`
- `materializer::tests::the_fingerprint_separates_requests_that_bake_to_different_depths`
- `panorama::tests::only_a_single_page_of_360_material_may_enter_the_view`

重複した属性だけを各1行除去した。関数・assert・入力・分岐は変更しない。
以降の正規件数は **13,205件＝実行13,146＋ignored59**。
修正対象の両ランナーによるexact実行はどちらも **3成功 / 0失敗**。
ログ: `target/test-speed-duplicate-nextest.log` / `target/test-speed-duplicate-libtest.log`。
最後のcargo-test gateでも本体は13,146成功 / 59 ignoredとなり、nextestの実行・成功・ignored件数が一致した。

初回B1の失敗は0件。`current_exe`でhelperを起動するlogger・log writer・
AI runtime等も全件実行で通過した。DLLはAと同じtest executable隣のFFmpeg DLLを使用。
各testのcwd・TempDir・環境変数の扱いを変える修正は不要だった。
固定名の一時GIFを使う1件も通過し、他testとの衝突は観測していない。

C1経由の次の全件試行（priority設定あり）は **378.345秒、13,144成功 / 2失敗 / 59 skipped**、
exit 100で、成功台帳は作られなかった。ログは `target/test-speed-c1-full.log`、
`target/test-lib-full/runs/20261015T001006214Z-97d24f96/`。

- `explicit_quarantine_publishes_all_local_work_before_reopening_admission`:
  Appが隔離完了を受け取ると直ちに置換journalの保存を始めるため、その後の
  `!journal.exists()` は保存workerと競合していた。既存の `task.join_terminal()` で
  隔離workerを待ち、Appの完了採用前に元journal消失・隔離ファイル1個・元byte一致を確認する。
  その後に完了採用・Waiting・全3jobの保存確認を行う。元のassertはすべて維持した。
- `multiwindow_scenario_collection_order_full_mode_control`:
  full-modeのcollection表示中、期待画像がNoneになる失敗。2件の20回stressは40実行すべて成功したが、
  24processでこのscenarioを96回走らせると11件失敗した。既存loggerで
  `fullscreen: main focus guard closing session` を確認した。driverが全画面open後もROOT focusedと
  報告し続けるため、500ms grace後の製品ガードが正しく閉じていた（slideshow開始前にも再現）。
  テストのopen helperで既存fullscreen viewportへfocusを移し、そのviewportでpaintを確認する。
  製品ガード・タイマー・待機期限・期待画像は変更しない。調査用logger設定は差分から除去した。

focused log: `target/test-speed-failure-focused.log`、
負荷再現: `target/collection-order-load/summary.json` と同directoryの個別log。
診断log: 同directoryの `diag-case-85.log` / `diagnostic-73168.log`（500ms focus guardの証拠）。
修正後は対象2件×20回＝40実行が成功（`target/test-speed-failure-focused-final.log`）。
同scenarioの24並列・96回も **96成功 / 0失敗、wall 12.714秒**
（`target/collection-order-load/fixed-summary.json`）。
修正後のlibtest `--list` は重複なし13,205件、元のテスト名集合との一致を再確認した。

次のC1全件試行は **355.441秒、13,143成功 / 3失敗 / 59 skipped**、exit 100
（command wall 359.693秒）。元の2件は成功したが、以下を検出した。
ログ: `target/test-speed-c1-final.log`、
`target/test-lib-full/runs/20261015T003150386Z-76a865e8/`。

- `multiwindow_i2_last_endpoint_and_mixed_inheritance_affect_real_paint`:
  previewがpaint済みでもpending final loadは残り得る。12 frame打ち切りでstable前にbaselineを
  読んでいた。park後と同じ `stable_content` 条件とpaintの両方を10秒期限内で待つようにした。
  painted page数・位置・before/after状態・I2 paint一致のassertは維持する。
- `checking_publishes_a_proof_only_after_matching_owner_install`:
  10,000回のyieldでworker待ちを打ち切っていた。回数はworker完了の保証にならないため、
  元のproof条件を10秒期限内で待ち、sidecar install・proof reuseの全assertを維持する。
- `real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch`:
  `mid4ms.mkv` / 48kHz / serial 1が **95,664 frames、期待96,000**（336 frames欠落）。
  EOFの製品分岐が `processed/raw/queued < 20ms` と `quiet >= 48ms` でseekと
  `clear_audio_output_buffer()` を行うことを確認した（`src/video/mod.rs` のnative EOF分岐）。
  callbackとtickを同一threadで固定量ずつ模擬したため、UI側のスケジューリング遅延を
  デバイス停止として扱っていた。前回の製品不具合という解釈は以下の設計判断で訂正する。

#### EOF模擬の設計判断と修正（Opus、続行依頼）

製品ではcpal callback threadが実時間でprocessed bufferを消費する。UI tickが48ms以上
遅れても、残り10msはその間に消費済みとなる。callback自体が48ms以上止まる場合は
既に音声が途切れており、その場合の末尾10ms破棄は製品の想定内とする。製品EOFの変更は行わない。

前回の60ms停止診断（processed=0.010秒、quiet=65.0448ms、audible PTS=1.990秒、
95,520/96,000 frames）はデバイス停止の再現だった。ログ
`target/test-speed-eof-diagnostic.log` と `target/test-speed-eof-stall-diagnostic.patch` は
履歴証拠として残すが、UI遅延だけで製品が音声を欠落させる証拠とは扱わない。

テストの `capture_real_eof_full_loops` だけを変更し、前回callbackからの実経過nanosecond×rateを
要求frame数へ変換する。余りを次回へ持ち越し、44.1kHzでも端数を失わない。要求bufferは
最大100ms分に制限し、長い経過時間は同じcallback APIへの複数要求で消費する。
未処理の経過frameを捨てず、callbackをpresenter/tickより先に呼ぶ。
製品API・clock hook・EOF判定・96,000 framesの期待値とexact-length assert・入力素材・
各3周・PCMおよび全callback出力のバースト検査は維持する。
`threads-required`による単独実行・retry・skipは採用しない。

独立レビューP2-1では、旧模擬がtick→callbackの順だったため、末尾10msがprocessedに
残ったtickで静寂判定を開始し、判定期間中にデバイスが鳴らし切る経路も通っていたと指摘された。
現在の実時間比例・callback先行の全周回テストは負荷に依存しない一方、この部分残量経路を通る保証はない。
そこで既存の全周回テストを維持し、別の1テスト
`real_decoder_eof_partial_tail_drains_during_quiet_before_loop_seek`で決まった手順を検査する。
実decoder/pumpの`tail10ms.wav`（48kHz、2秒）をcallbackで95,520 framesまで消費し、
EOS tail処理の完了後も480 frames（10ms、0 < processed < 20ms）が残ることをassertする。
production tickでquiet_sinceの設定・seek未発行を確認し、callbackで残り480 framesを消費する。
その後だけ既存`backdate_eof_quiet_for_test(49ms)`を呼び、production tickのseek serial増加、
PCMのexact length 96,000 framesと元の末尾burstを検査する。静寂期間は実sleepで作らない。
新hook・製品コード・fixtureの変更はない。

callbackを挟まず10ms残したまま49msを経過させるケースは、デバイス自体の停止であり、
Opusの設計判断で末尾破棄を想定内とする。旧テストが検査した「判定期間中に実際に鳴らし切る」
経路とは異なるため、その停止ケースを欠落なしの回帰テストには追加しない。

実時間比例への変更だけで単独nextestは75.123秒で成功した。
続く負荷試験ではEOF 20processと音楽解析24processを同時実行し、CPU使用率100%を観測。
EOFは8成功 / 12失敗、負荷側24件は成功（`target/test-speed-eof-load/`）。
12失敗はすべてPCM長ではなくFirstFrameReadyの即時try_send assertだった。
製品presenterはbounded event queueが満杯なら通知を保持して後続で届けるのに対し、
模擬presenterは即時成功を仮定していた。既存 `drain_engine_events()` で実consumerを進め、
同じ通知の到達を元の20秒期限内でassertするようにした。通知を落とす・成功扱いする変更はなく、
テスト自体のretryでもない。製品のpending通知・event処理は変更しない。

この修正後、同じEOF 20process＋音楽解析24processの負荷で **EOF 20/20成功**。
再びCPU使用率100%を観測した（`target/test-speed-eof-load-final/cpu-load.json`）。
実行体をexact指定で1testずつ別processへ起動し、nextestと同じ分離方法で合計720 epochを検査した。
負荷側も24/24成功。EOF各回は約80.8〜83.2秒、44process全体wall 186.408秒。
結果は `target/test-speed-eof-load-final/summary.json` と各exact実行logに保存した。

その後の本体全件は459.176秒、13,145成功 / 1失敗 / 59 ignored、exit 100
（`target/test-speed-c1-complete.log`、run `20261015T010620695Z-5705a8d8`）。
EOFは76.047秒で成功。失敗は `multiwindow_scenario_b_activation_keeps_singleton_paint` の
`stable_content(...).expect("active paint is stable")` で、前回のI2 endpointと同じ
preview paint完了≠final load完了という前提の取り違えだった。
兄弟ルートを点検し、singleton・endpoint・白companionのactive/activatedの計4か所を
共通のtest helper `stable_paint_frame` に揃えた。paint（必要な白companionを含む）と
既存 `stable_content` が両方readyになるまで10秒期限で待つ。元のpage数・配置・
owner状態・before/afterのI2一致assertを維持し、製品述語やviewport経路は変更しない。
関連3件（singleton、endpoint、白companion）のnextest 20回stressは **60実行すべて成功**、
88.963秒（`target/test-speed-stable-paint-focused.log`）。fmt / diff checkも成功。

前回の他2件の修正後focusedは各20回、計40実行すべて成功
（`target/test-speed-last-focused.log`、54.409秒）。今回のEOF修正後の負荷検証・全件結果は上記と最終結果節を参照。

### B2: 長いテストを先行実行

B1で60秒を超えた4件は、物理走査239.307秒、RAW181.395秒、音楽合成信号98.874秒、
real decoder75.362秒。F4の180ケースは59.967秒であり、検証内容を縮める変更はしない。
終盤に残った音楽・decoder・RAWを含む上の4件を `.config/nextest.toml` でpriority 100にし、
他testと並行して早く開始する。時計・fixture・反復・期待値を変えず、ignore追加もない。
この設定での全件確認はC1の本体実行と兼用した。
priority設定後の378.345秒／355.441秒はともに失敗を含むため、成功時の高速化結果には採用しない。
scan / RAW / 音楽は最終試行で214.033 / 158.138 / 104.948秒で先行完了しており、
その時点では失敗を含むため、設定の最終検収は保留した。
続行後の全件ではF4が最後の1件として79.345秒かかり、459.176秒で完了した。
そこで5件目としてF4も同じpriorityへ追加した。180ケースの入力・反復・assertは変更しない。
成功した最終全件ではEOF 78.308秒、F4 90.247秒、音楽202.908秒、RAW268.661秒、
走査296.578秒で完了した。5件とも全体422.185秒より前に終了し、F4の終盤単独実行を解消した。
負荷変動があるためpriority単独の短縮率とは断定しないが、critical pathの尾を避ける効果を確認して維持する。

## 作業C: 実行道具の検証

PowerShell 5.1、追加4スクリプトはASCIIのみ。構文parse・ASCII検査成功。
実nextest/Gitと使い捨て小規模crateを使う `scripts/test-test-tools.ps1` は成功。
作業ツリー・indexを変更しないfingerprintについて、staged / unstaged・tracked削除・
untracked Unicode path・ignored RAW fixtureを検出し、target生成物では変わらないことを確認した。
Windows標準CNGのSHA-256既知vectorも確認。全repoの識別子計算は **1.465秒**。

確認済み:

- C1: 初回MISS、同じ状態でHIT（run directoryが増えない）、差分時MISSと再実行、`-Force`、feature変更。
- C1: 実行件数が閾値未満ならexit 2、0件ならexit 4、失敗ならexit 100。
  失敗は記録せず再呼出しもMISS。既存成功の後のforced failureも旧成功を再利用しない。
- C2: module filterは2件、integration targetは1件、他workspace crateは1件で成功。
  0件一致はexit 4、広域 `src/app/tests.rs` はexit 2で明示案内、選択なしはexit 4、repo外pathはexit 2。
- C2実repo: `-Path src/activity_gate.rs` は8件成功（0.445秒）。
  `-PlanOnly` の既定Git差分検出は `.config/nextest.toml` と `src/app/tests.rs` を広域と表示しexit 2。

fixture/log: `target/test-tools-regression-210c65bb10414c1ab2b8c29293d192e6`、
`target/test-speed-tools-regression.log`、`target/test-speed-focused-actual.log`、
`target/test-speed-focused-changes.log`。初期の実repo C1は失敗を記録せず、上記の修正後に
全件成功・receipt作成・同一状態のHITを確認した。HITは3.022秒、run directory増加なし。
実repoで無関係な差分を作って全件を繰り返す確認はせず、差分時のMISS/再実行は小規模fixtureの
実行証拠を再利用する。

識別範囲と検知しない変更、使い方は `docs/development-build-and-test.md` の
「nextest と成功台帳」を参照。各runは固有のconfig・JUnit・stdout/stderrを持ち、
別実行のJUnitを成功の根拠にしない。計測出力である本報告だけはsource hashから除外する。
Git共通directoryへの書き込みや別worktree共有は行わない。

## 最終ゲート・残課題

EOFの設計判断は上記のとおり確定し、テスト側の模擬だけを修正する。
本体全件・実repo台帳HITの確認後に、変更していない `scripts/test-full.ps1` を最後に**1回だけ実行**し、
**PASS、exit 0、3,053.825秒（50分53.8秒）**で完了した。
command: `powershell -NoProfile -NonInteractive -File scripts/test-full.ps1 -SuppressCrashDialogs`。
crash dialog抑制はこのprocessと子だけに適用され、終了時に元のerror modeへ復元された。

- workspaceのunit・補助bin・integration: 成功。本体libは13,146成功 / 0失敗 / 59 ignored、2,118.17秒。
  `ui_snapshot` integrationも177件成功。nextestと本体の実行・成功・ignored件数が一致した。
- doctest gate: 成功。現在は実行0件・既存ignored 3件（本体）。新たなignoreは追加していない。
- vendor: egui **25**、egui-winit **29**、egui-wgpu **11**、eframe **18**、すべて成功。
- ログ: `target/test-speed-test-full-final.log`、summary: `target/test-speed-test-full-final-summary.json`、
  doctest内訳: `target/test-speed-test-full-evidence.json`。

未解決の実装事項はない。製品コード・test hook・入力fixture・exact-length assert・周回数は維持し、
新しいignore・単独実行予約・テストretryは追加していない。失敗した初期試行と修正経緯は上記に残した。
launcher unit testのcompileに必要な埋め込みPE 3本は
`C:\home\mimageviewer\target\release` の既存ファイル（2026-10-12）を実コピーした。
test fixtureとしての埋め込み・展開確認だけに用い、起動せず、現行sourceの確認用binaryとは扱わない。
製品runtimeの変更はなく、実機確認用binaryのbuild・UI操作は不要。

test-fullをnextestへ寄せる場合は、workspace / integration / pack builderに加えて
workspace-excluded vendor 4 graphも個別に移す必要がある。doctestはnextest対象外なので
`cargo test --workspace --features pack-build-tools --doc` 相当を別gateとして残す。
現在のdoctestは0実行 / 3 ignoredだが、将来追加される例を取り落とさないためにも分離したcargo gateが必要。
本体libの成功だけでは他crateのprocess-global状態・固定temp path・native helper・
child process・DLL探索・GPU並列負荷の等価性は証明できない。全graphでテスト名・ignored・
件数を突合し、同じ自動gateで失敗することを確認する段階的移行が必要。
今回は移行せず、従来gateの証拠を維持する。
仕様: [nextest running](https://nexte.st/docs/running/)、
[JUnit](https://nexte.st/docs/machine-readable/junit/)。

CLAUDE.mdは未編集。Opus側で必要なら、既存の無filter本体lib実行例を
`scripts/test-lib-full.ps1` へ揃え、最終cargo-test gateの例は維持する。

## 独立レビューP2対応（ec0eeebff後）

Opusが既存変更を8 commitへ整理した後の追加対応。製品コード・CLAUDE.mdは変更せず、commitしない。

- P2-1: 上記の決定的な部分末尾テストを1件追加。既存の全周回・実時間比例テストは変更しない。
- P2-2: `.cargo/config.toml` の`[env]`キー（文字列／inline table、`[env.NAME]`形式も含む）を
  自動で収集し、process環境の未設定／値を台帳へ反映する。現在のFFMPEG_DIR・LIBSQLITE3_FLAGS、
  既存のRUST/CARGO/NEXTEST/MIV/MIMV系・LIBCLANG_PATH等を含む。値はhashのみ保存する。
  解釈できないenv宣言は成功再利用を止め、警告して通常実行する。
- P2-3: sourceの生成物除外は維持し、native runtime識別子を台帳キーへ追加する。
  Cargo target directoryの指定profile直下・depsとtarget triple配下の同profileを調べ、
  FFmpeg 6本を含むnative DLLのパス集合・全byteを照合する。Cargoの16桁hash付きRust DLLは除外する。
  削除・差し替え・追加で識別子が変わり、実行中に変わった場合も成功を記録しない。
- P2-4: 広域／対応付けできない変更を1つでも含めば、その全パスを表示してexit 2。
  `test-lib-full.ps1`と必要な専用検査を案内し、残りのmodule等だけを走らせて成功にしない。
  文書のみもこの扱いとし、変更なし／対応filterの0件一致は従来どおりexit 4。

追加EOFテストの単独nextest stressは **20/20成功、12.735秒**。
実repoのForce全件と並行したnextest stressも **20/20成功、17.017秒**。
起動時に全件側の7プロセス以上を待ち、反復中のsnapshotで本体lib test実行体21プロセスを確認した。
load時も各反復で部分残量・quiet開始・残量消費・seek・exact lengthをすべて検査している。
ログ: `target/test-speed-p2-eof-alone.log` / `target/test-speed-p2-eof-load.log`、
並列snapshot: `target/test-speed-p2-eof-load-start.json` / `target/test-speed-p2-eof-load-cpu.json`
（後者のCPU率はWMI権限不足で未取得、process数は取得済み）。

`scripts/test-test-tools.ps1`は **PASS、exit 0**。実Git/nextestの使い捨てcrateで、
configから自動収集する任意名2種類を含む環境変数10種類について、変更→MISS・再実行、同値→HITを確認した。
FFmpeg 6本それぞれの内容交換・削除もMISS・再実行を確認した（fake DLLはロードしない小規模crateのみ）。
選択できる`src/foo.rs`と未対応入力の混在12ケースは、すべて該当pathと全件案内を表示してexit 2、
nextestのrun directory増加0。既存のmodule/integration/workspace選択、0件一致exit 4、
失敗・件数不足・force失敗後の旧成功無効化・index非変更も通過した。
実repoの`test-focused.ps1 -PlanOnly`も、今回の文書・scriptsの全6パスを表示してexit 2、
`src/video/mod.rs`だけの部分実行を行わないことを確認した
（`target/test-speed-p2-focused-actual.log`）。
ログ: `target/test-speed-p2-tools.log`、fixture:
`target/test-tools-regression-33efdf55b9754d93bf343b66283b8e09/`。
PowerShell構文parse・ASCII検査、`cargo fmt --check`、`git diff --check`も成功。

最初のForce全件は **399.616秒、13,146成功 / 1失敗 / 59 ignored、exit 100**。
source `62256f226ce5b8ae242b175bf59308cc09bcc401af2a3d8b40bd077c77717ed6`、
run `20261015T024050444Z-7f1b54a1`、ログ`target/test-speed-p2-full.log`。
追加EOFテストと既存全周回EOF（77.773秒）は成功した。失敗は
`embedded_similar_move_update_pump_gives_same_frame_page_navigation_priority`のcancel assertで、
成功台帳は作られなかった。

同テストは単独30/30成功だったが、24プロセス・96回で27成功 / 69失敗となった。
一時的なテスト内logger診断でも53成功 / 43失敗となり、失敗logで
`all keyboard input is now held by a dialog: sidecar_restore`を確認した。
初期updateを1フレームしか進めない準備helperが起動時の非同期sidecar復元を残し、
検査用sceneへ置換した後のArrowDownを製品が正しく保留していた。
`settle_root_app_update_fixture`を、使い捨ての空startup folderを開き、
初期navigationとsidecar復元がproduction updateで完了するまで10秒期限内で進める形に修正した。
共通helperを使う兄弟5テストに同じ準備を適用し、元のkey・controlled scan・cancel・
遷移先・項目一致assertは維持する。製品ガード・新hookの追加はなく、診断用loggerは差分から除去した。
初期再現log: `target/test-speed-p2-navigation-load/`、診断log:
`target/test-speed-p2-navigation-diag/001.log`等、単独log: `target/test-speed-p2-navigation-alone.log`。
修正後の同helper利用5件×20回は **100実行すべて成功、15.159秒**
（`target/test-speed-p2-navigation-fixed.log`）。失敗した1件の24プロセス・96回も
**96成功 / 0失敗、6.790秒**（`target/test-speed-p2-navigation-fixed-load/summary.json`）。

修正後の検証sourceは `89ea39185c5ba2ef1e47ed1043cb0c3e6c6c337d6d063d7ea7c6c407e30ef74b`。
`scripts/test-lib-full.ps1 -Force`は **PASS、exit 0、373.979秒（6分13.979秒）**、
**13,147成功 / 0失敗 / 59 ignored**（合計13,206件）。既存13,146件＋追加EOF 1件で、ignoredは不変。
wrapper全体は380.998秒。元の作業A 1,710.943秒に対して78.14%短縮、約4.58倍だが、
並列負荷の変動も含む比較であり今回のP2修正だけの効果とは扱わない。
ログ: `target/test-speed-p2-full-verified.log`、run `20261015T025951716Z-c3b811d1`。
成功receipt:
`target/test-lib-full/1ca6cfc03ec9b9d9d82ab185bbb600308d28d96ad1711cc9a682d767c32843e6.json`。
既存EOF全周回も76.710秒で成功。実行前後のsource/environment/native runtime一致を確認して記録された。

同一状態の次回呼び出しは **HIT、exit 0、3.080秒、新規run directory 0（6→6）**。
ログ: `target/test-speed-p2-hit.log` / `target/test-speed-p2-hit-summary.json`。
初回の失敗を成功へ読み替えず、テスト準備を直したsourceで全件成功を検証した。
P2の未解決事項はない。依頼により`test-full.ps1`は再実行しない。
前節の最終gateは追加テストより前の証拠として維持し、今回の変更を検証した結果とは区別する。

### Opusへの未コミット単位

commit・masterへのmerge・pushは実行しない。以下の論理単位で作業ツリーに残す。
共通helperが台帳・focusedの両方を持つため、runnerとその回帰確認を1単位にまとめる。

| ファイル | commitメッセージ案 |
| --- | --- |
| `src/video/mod.rs`（test module内のみ） | `test: Cover partial EOF tail draining before loop seek` |
| `src/app/similar_navigation_tests.rs`（test moduleのみ） | `test: Settle isolated startup before similar navigation input` |
| `scripts/test-tools-common.ps1`, `scripts/test-lib-full.ps1`, `scripts/test-focused.ps1`, `scripts/test-test-tools.ps1` | `test: Harden success identities and reject incomplete focused selections` |
| `docs/development-build-and-test.md`, `docs/test-speed-report.md` | `docs: Record review fixes and test identity boundaries` |

## 全体ゲートのnextest移行（2026-10-16）

**状態: 完了。依存不足を解消して通常Cargo homeで再開し、旧／新gateとも完走。全graphの名前・実行・成功・ignoredが一致。**

対象は `full-nextest` worktree（基準 `21673c767`、HEAD `9b4299c13` はbrief追加のみ）。製品コード・CLAUDE.md・依存バージョンは
変更しない。`test-full.ps1` のworkspace（`pack-build-tools`）とvendor 4 graphをnextestへ移し、
workspace doctestは同じfeatureの `cargo test --doc --no-fail-fast` として独立段に残す。
standalone vendorもルートの `.config/nextest.toml` を明示指定する。
各nextest段はdefault filterを無視し、既存ignored以外の全件、retry 0、段内no-fail-fast、
0件は失敗とする。各段の非ゼロ終了で後続を止め、未導入時はinstall方法を案内して終了する。

### 完走時間と測定条件

| gate | exit | 全体の壁時計時間 | 成功 / 失敗 / ignored |
| --- | ---: | ---: | ---: |
| 旧cargo test gate（再開後の完走1回） | 0 | **2,678.144秒（44分38.144秒）** | 14,596 / 0 / 66 |
| 新nextest + doc gate（最終1回） | 0 | **491.718秒（8分11.718秒）** | 14,596 / 0 / 66 |

今回の実測は **81.64%短縮、約5.45倍**。同じソース・通常test profile・`pack-build-tools`、
logical CPU 24、テスト並列数の環境変数上書きなし。buildだけ `CARGO_BUILD_JOBS=1`。
`CARGO_HOME`／`CARGO_TARGET_DIR` は未設定で、通常のCargo homeと各graphの既定targetを使った。
全体時間はbuild／metadata／出力を含む。旧gateは並行した一覧準備のegui-winit初回build
（Cargo表示1分23秒）によるartifact lock待ちも含む（待ち時間のみは未計測）。新gateは
準備済みcacheを使うため、厳密な同一cache条件の
benchmarkではない。負荷・cacheで絶対値は変わるが、全検査範囲を維持した実際のgate時間である。
旧本体libは13,217成功／59 ignored、2,316.08秒。新workspace全体は14,513成功／63 ignored、
464.048秒だった（対象範囲が異なるため、この2値だけの速度比は出さない）。

コマンドは旧script保存版／新 `scripts/test-full.ps1` をそれぞれ
`powershell -NoProfile -NonInteractive -File <script> -SuppressCrashDialogs` で実行。
新gateだけ結果保存のため `NEXTEST_FINAL_STATUS_LEVEL=all` を指定した。
通常のslow警告・重い5件の先行実行・retry 0は維持する。
両gateともPASS・error mode復元を記録し、強制再試行や個別失敗の成功扱いはない。
実行体隣のnative DLL識別子も実行前後で同じ
`7731f6994704e03105591acdee5e6e0c2f19a804a02e1b1e576cd69c3be72262`。
ログ／時間: `target/full-nextest-evidence/before-complete.log`・同summary、
`after.log`・同summary。graph別JUnitは同directoryの `after-*.junit.xml` に保存した。

### error modeとスクリプトの回帰

PowerShell 5.1.26100.9711 / cargo-nextest 0.9.148の専用・非対話probeで、
親の `GetErrorMode()` と、実nextestテスト子、その子が起動するhelperの値を照合した。

| 条件 | PowerShell親 | nextestテスト子 | helper |
| --- | --- | --- | --- |
| 通常 | `0x8001` | `0x8001` | `0x8001` |
| 必須bit `0x3`を除く対照 | `0x8000` | `0x8000` | `0x8000` |
| suppression | `0x8003` | `0x8003` | `0x8003` |

各条件2件、合計6件成功、exit 0。finally後の親も元の `0x8001` に一致した。
必須bitの継承を直接検査したもので、実クラッシュやダイアログを発生させる検査ではない。
現行のSetErrorMode／復元処理を維持でき、追加wrapperやWER設定変更は不要。
ローカルのnextest-runner 0.126.0ソースでも、通常のCommand→Tokio spawnを使用し、
Windowsのerror modeをリセットするcreation flagやAPI呼び出しはない。
証跡と再現用crate／scriptは `target/full-nextest-evidence/error-mode-probe/`。

`powershell -NoProfile -File scripts/test-release-build-safety.ps1` はPASS、exit 0。
native cargo stubで通常成功・suppression成功と、version確認／workspace／doc／vendor 4段の
全7か所の失敗を検査した。失敗コード37の維持、retry／後段実行なし、子への必須bit継承、
親mode復元、未導入時の `cargo install cargo-nextest --locked` 案内を確認した。
共通の `test-tools-common.ps1` は変更しない。

### 測定の準備

初回の旧gate起動はTurboJPEGのMSBuild `--parallel 24` でコンパイル失敗し、テストは未実行
（exit 101、97.013秒）。以前の測定と同じく、生成済みnative buildを
`cmake --build target/debug/build/turbojpeg-sys-b95e4463f170e2a1/out/build --target install --config Debug -- /m:1 /nr:false`
で準備した。ツール導入や製品・依存ソースの修正は行わない。
ログ: `target/full-nextest-evidence/before-build-failed.log` と同summary。
以後の比較はbuild並列数だけ `CARGO_BUILD_JOBS=1` とし、テスト並列数の既定24は維持する。

### 一覧・実行結果の突合

`compare_inventory.py` は旧scriptの各selectorを確認し、`cargo test --no-run --message-format json`
のtest artifactから各実行体の `--list` と `--ignored --list` を採取する。nextest側は同じselectorの
`cargo nextest list --message-format json --ignore-default-filter` を使い、package ID・target種別・
binary名・テスト名・ignored指定を比較する。空のtest binaryも対象に含める。
nextest一覧の `mismatch: ignored` は既存ignoredの正常な除外として照合し、それ以外の除外は失敗とする。

| graph | 旧: 実行 / 成功 / ignored | 新: 実行 / 成功 / ignored | 名前・指定・結果の照合 |
| --- | ---: | ---: | --- |
| workspace（54実行体） | 14,513 / 14,513 / 63 | 14,513 / 14,513 / 63 | 全14,576件・空実行体を含め完全一致 |
| vendor/egui | 25 / 25 / 0 | 25 / 25 / 0 | 完全一致 |
| root graph egui-winit | 29 / 29 / 0 | 29 / 29 / 0 | 完全一致 |
| vendor/egui-wgpu | 11 / 11 / 0 | 11 / 11 / 0 | 完全一致 |
| vendor/eframe | 18 / 18 / 0 | 18 / 18 / 0 | 完全一致 |
| workspace doc（12 package） | 0 / 0 / 3 | 0 / 0 / 3 | 空packageを含め完全一致 |
| 合計 | **14,596 / 14,596 / 66** | **14,596 / 14,596 / 66** | 差分0件、全graph失敗0件 |

証跡: `target/full-nextest-evidence/inventory/workspace/comparison.json`、同directoryの
旧／新inventoryとraw一覧、`inventory-resumed/summary.json` と各graphの旧／新inventory・raw一覧。
一覧比較はテスト本体を実行せず、成功件数の証明には使わない。

実行結果は `compare_execution.py` で旧gateの各Cargo実行体の最終libtest集計と、新gateの
graph別JUnit／doc出力を照合した。子helperがstdoutへ出す集計や同名testは二重計上しない。
libtestの表示用 ` - should panic` だけを実名へ戻し、テスト名・成功／ignoredを全件照合する。
空binaryのnextest JUnit省略は一覧の一致で補い、testを含むbinaryの欠落は認めない。
`python target/full-nextest-evidence/compare_execution.py` はexit 0、`match: true`、errors 0。
再現用script、raw入力のhash、各binaryの最終集計・名前／状態は同directoryの
`execution-comparison.json` に保存。二重登録解消・新しいignore・説明できない差は0件。

### 依存不足による中断と再開の経緯

nextest 0.9.148は実行対象の列挙前に `cargo metadata --all-features` を呼ぶ。
vendor/eguiでは既存の通常featureのlibtest build／一覧は成功したが、metadata用の
`backtrace 0.3.76` のglobal Cargo sourceへの展開がsandboxの書込み制限で失敗した（nextest exit 102）。
registryのsrc／index／cacheだけを、reparse point 0件・日時保持で作業ツリー内の
`target/full-nextest-evidence/cargo-home/` へ実コピーして調べたところ、同lockfileの24版本の
sourceが未展開、うち21版本はarchiveもなかった。既存Cargoで取得を試みても
`addr2line 0.25.1` の `static.crates.io` 取得が `127.0.0.1:9` への接続失敗となった
（`CARGO_NET_OFFLINE=false`、`CARGO_NET_RETRY=0`、exit 101）。
ツールのinstall・lockfile／依存バージョンの変更・global cacheへの書込みは行っていない。

不足一覧: `target/full-nextest-evidence/cargo-home-egui-missing-sources.json`。
取得失敗: 同directoryの `cargo-home-egui-network-enabled-metadata.stderr.log` とsummary。
local Cargo homeで通常featureのegui `--lib --no-run` は14.06秒で成功したが、28 artifactsが
すべて再buildされたため、rootの測定条件へこの環境変更を広げていない。
別案の `--cargo-metadata` 明示経路は対象graphの等価性を別途検証する設計変更となるため未採用・未検証。

この停止条件の確定後、進行中の旧gateは所有するCargo→本体test executableの親子関係と
worktree内の実行パスを確認して終了させた。経過 **1,401.028秒（23分21.0秒、中断）**、
PowerShell exit `4294967295`。終了時の親error modeは元の `0x1` へ復元された。
`before.log`／`before-summary.json` は中断記録であり、製品テストの失敗や全件成功として扱わない。
停止時点では旧gateの完走はなく、移行後 `test-full.ps1` の最終一回は未実施だった。
前回の **3,053.825秒（50分53.8秒）** は履歴値として残し、今回の速度比には再開後に
完走した旧gateの2,678.144秒を使う。中断分や初回compile失敗をその時間へ加算しない。

その後、Opusがsandbox外でrootとstandalone vendor 3 graphに `cargo fetch --locked` を実行し、
各graphの `cargo metadata --locked --all-features` がexit 0になったと報告した。
lockfile・依存バージョンは変更なし。エージェントは**通常のCargo home**で再開し、
local Cargo homeは使用しない。再開後は全graphの一覧・実行結果が一致し、
旧／新gateとも各1回の完走でPASSとなった。

プロセス分離に伴うテスト修正は0件。製品／テスト本体の変更も0件。
独立Sol/xhighの最終レビューも **ACCEPT、修正必須指摘なし**。
ログ・JUnit・27入力のhash・比較処理を確認し、成功したgateの再実行は行っていない。
PowerShell構文parse・ASCII検査・fmt check・diff checkは成功。
共通部品は未変更のため `test-test-tools.ps1` の再実行は行っていない。
スクリプト・文書のみの変更であり、アプリ検証build・UI起動は行わない。
このbriefの実装・検証に未解決事項はない。

### 引き継ぐ論理コミット単位

commit／merge／pushは行わず、6ファイルを作業ツリーに残す。

| ファイル | commitメッセージ案 |
| --- | --- |
| `scripts/test-full.ps1`, `scripts/test-release-build-safety.ps1`, `.config/nextest.toml` | `test: Move complete release gate to cargo-nextest with separate doctests` |
| `docs/development-build-and-test.md`, `docs/release-checklist.md`, `docs/test-speed-report.md` | `docs: Record complete nextest gate coverage and timings` |

CLAUDE.mdの必須変更は見つからず未編集。AGENTS.md:248の「最終gateはcargo testのまま」という
旧記述は設計担当側で更新が必要（今回のbriefを優先し、同ファイルは未編集）。

## ファイルstatによるハッシュキャッシュ（2026-10-16）

対象: `ledger-stat-cache` worktree、製品コード・依存関係は変更なし。
本体worktreeでの既存測定は台帳HIT 63秒、うちsource計算49.9秒・21,247件・59.1 GB
（設計担当からの報告）。ignored `testimage/` を照合対象に残し、変更のないファイルの
SHA-256再計算を省く。DLL識別子・成功台帳の形式・source manifestの形式は維持した。

相対パス→サイズ・更新日時UTC ticks・作成日時UTC ticks・SHA-256・記録時刻を
`target/test-lib-full/source-hashes-v1.tsv` に保存する。PowerShell 5.1向けに
タブ区切りとDictionaryを使用し、パスはUnicodeやタブを壊さないBase64とした。
サイズ/更新日時/作成日時が一致し、更新日時が記録時刻より2秒超古い場合だけ再利用する。
記録時刻は内容を読む直前に採り、再利用時には更新しない。読み取り中にstatが変わった
エントリは保存しない。入れ子repositoryも同じ経路を通る。
通常ファイルのreparse pointはキャッシュせず従来どおり内容を読む。

まれな失敗は既定の割り切り方針: キャッシュの読み書き・形式検証に失敗したら警告し、
そのsource照合をキャッシュなしの全ハッシュで行う。自動再試行や回復状態は追加しない。
壊れた原本は失敗処理で削除・再作成しない。`-Force` の明示実行は、キャッシュを読まず
実行前後を全ハッシュし、正常に書ける場合は新しいキャッシュを保存する。

### 再現用fixtureと検証

```powershell
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File scripts/test-test-tools.ps1 -Benchmark
```

Windows PowerShell 5.1.26100.9711、使い捨てrepositoryに21,247ファイル×64 KiBを実生成。
総量1,392,443,392 bytes（約1.39 GB）、20,000件はignored `testimage/flat/`、
1,247件はignored `testimage/nested/` 内の入れ子repository。Cargo等の入力4件を合わせ
source照合は21,251件。更新日時は1日前へ設定し、初回から非racy条件を満たす。
OSのファイルキャッシュは消去していない。「キャッシュなし」は今回のSHA-256キャッシュを
使わない意味であり、cold diskの測定ではない。台帳HITはPowerShell子プロセスの起動、
source/env/native識別子、JUnit照合を含むwall時間を計測する。準備用の1件テスト実行は別枠。
HIT各回はexit 0・同じsource・新規run directoryなしをassertする。

最終測定（リンク非キャッシュ化・公開`-Force`回帰を含む未コミット差分）:

| 対象 | 秒 |
| --- | ---: |
| source、キャッシュなし | 4.787 |
| source、キャッシュ初回作成 | 5.096 |
| source、キャッシュあり3回 | 2.993 / 2.975 / 2.980 |
| 台帳HIT全体3回 | **3.770 / 3.736 / 3.752** |

台帳HITは3回とも目標5秒以内。全条件でsource識別子は
`cf1ae86b8b66ff0fe644a6668b6444de755b4ddca686d119de86be406e9f17b5`。
最終コマンドは **exit 0、回帰全体PASS**。
ログ: `target/stat-cache-regression-final.log`。
fixture: `target/test-tools-regression-4e78bea217454978bac6c452d7f43dd1/target/stat-cache-benchmark/`。
その`target/measurements.json`と`target/hit-{1,2,3}.log`に数値・source・HIT結果を保存した。
初回も回帰PASS・台帳HIT 3.741〜3.842秒（`target/stat-cache-regression.log`）。
上記2点のスクリプト追補によって検証条件が変わったため最終版で再実行した。

回帰はキャッシュ有無の識別子一致、温まった入力の内容を読まないこと（排他ロックで検査）、
内容/サイズ変更、古い更新日時へ戻したサイズ変更、作成日時のみの変更、racyな同サイズ編集、
racyエントリの再照合、`ForceHash`、破損/切り詰め・キャッシュ読込/書込失敗を検査する。
既存のGit index無変更、ignored/nested入力、削除、新規Unicodeパス、台帳HIT/MISS、
native DLL追加/差替え/削除、環境、feature、失敗台帳、0件、focused選択の回帰も維持する。
シンボリックリンク作成はこのsandboxでは権限不足のため、既存のnested file-link実検査はskip。
変更した3本の`.ps1`はASCII以外0バイト、PowerShell 5.1の構文parseエラー0。
`git diff --check`も成功。
実装者とは別のGPT-6.1 Sol / xhighが3スクリプト・文書2本を独立レビューし、
**P1/P2指摘なし**。最終ログ・source一致・測定値を突合し、テストは重複実行していない。

### 検知範囲と引き継ぎ

サイズ・更新日時・作成日時をすべて保った内容変更は、非racyなエントリでは検知できない。
statを信頼するgitと同様の前提で、疑わしければ `test-lib-full.ps1 -Force` を使う。
公開`-Force`経路にも、同stat編集が通常HITした後で全ハッシュしてMISSになる回帰を追加した。
本体の59.1 GB・実画像での最終測定は利用者側の担当であり、この約1.39 GBのfixture値から
本体の絶対時間は断定しない。スクリプトと文書だけの変更のため製品全体gate・確認用build・
アプリ起動は行わない。

コミットは行わず、次の論理単位で作業ツリーに残す。

| 対象 | commitメッセージ案 |
| --- | --- |
| `scripts/test-tools-common.ps1`, `scripts/test-lib-full.ps1`, `scripts/test-test-tools.ps1` | `test: Cache source hashes by file stat for faster ledger hits` |
| `docs/development-build-and-test.md`, `docs/test-speed-report.md` | `docs: Document stat-cache limits and ledger hit benchmarks` |
