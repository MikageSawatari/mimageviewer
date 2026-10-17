# egui 0.37 段4 検証台帳

対象は `65beeb2f0` からの未コミット差分。Rust 1.99.0、Windows、egui/eframe 0.37.0、wgpu 30、offline。以下は**実装担当の内部確認**であり、利用者による外観採択・設計担当の受入とは区別する。段3の分類第3版と独立レビューACCEPTは過去の証拠として保持する。

baseline PNG、既存の寸法・操作assert、ignoreは変更しない。HUD z-orderとmetadata transferの既存テスト課題2件は対象外。実Window、live、製品起動、製品バイナリ、test-full、lib全件は実行しない。GPU試験はoffscreen、native入力試験はhandlerへのfake入力であり、デスクトップ入力やOS clipboardを使わない。

## 4a 入力・非表示・popup

### 変更と判断

- `src/video/native_presenter/render_core.rs`: 既存clipboard readerのtest側だけにスコープ付きtext/read-count fixtureを置いた。製品reader・イベント配送の動作は変えていない。通常のCtrl+Shift+C/Xに加え、Ctrl+Shift+Vの改行正規化・一度だけの読取り・空clipboard・release時の重複抑止・IME中断とcommit後の配送を実handlerで検査する。
- root/FS/兄弟contextの退役、取消、focus/repeat、復帰、viewport所有入力、gamepad保持軸、wheel、popup、IMEは段3の正式graphを再利用し、今回の限定filterにも対応付ける。新しい非表示受付やpending stateは作らない。
- 連続クリックは利用者が既に採択した本家0.37の6pt条件を維持する。再採択を求めない。
- **HEX同一batchのfocus切替は判断待ち。** `hex_focus_transfer_keeps_the_new_editors_partial_input` は今回も元assertで失敗する。0.37のTextEditではAtom計測中のイベント処理がpointer focus確定より先に走り、別fieldをclick→Ctrl+A→Textした新fieldが同batchのTextを処理できず、未処理で残る（段3の観測でも新fieldはfocus/gained=true、changed=false）。margin変更では直らない。入力再送・delay・guardや期待値変更は行わず、TextEditのイベント/focus所有を修正する範囲の設計判断へ返す。従来操作の維持を確認したとは記録しない。

### 回帰の対応付け

| 契約 | 回帰・証拠 |
|---|---|
| root Close・保存ACK・modal取消 | `root_lifecycle_actual_app_logic_reaches_backend_close_decision_without_ui`、`root_lifecycle_actual_app_save_ack_precedes_resumed_backend_close`、`root_lifecycle_actual_app_modal_rejection_consumes_only_one_delivery` |
| FSのactive/parked/passive・兄弟寿命・Remote | `viewport_retirement_mode_change_transfers_active_parked_and_passive_exact_lifetimes`、`viewport_retirement_managed_hook_consumes_modal_reject_then_accepts_new_close`、`viewport_retirement_remote_logic_waits_for_command_delivery_and_existing_save_ack` |
| 非描画の入力保留・復帰新規入力 | 段3 eframe `logic_only_child_full_root_full_preserves_owner_and_hook_order`、`skips_do_not_drain_and_resume_appends_new_input_after_terminal_events`、`state_replacement_retains_queued_terminal_events_and_owner`、`child_skip_preserves_keys_focus_ime_terminal_and_real_text_edit`（vendor変更なし、成立済み証拠を再利用） |
| focus loss・repeat・兄弟context | `page_turn_initial_press_survives_focus_loss_after_routing`、`page_turn_auto_repeat_without_focused_level_permit_is_dropped`、`page_turn_edge_from_sibling_viewport_is_no_match`、focus/keymap filter |
| gamepad終端・edge順序・保持軸 | gamepad filter。段3のlogic物理drain→UI配送、無効時終端の集約、復帰後保持軸の回帰を含む |
| IME producer・input/output・viewport中断 | ime filter。段3のegui `ime_interrupt`、winit `disabled_producer_*`、native intent/control/WindowEpoch成立済み証拠を再利用。native実Windowは今回も分離 |
| pointer capture・loss/cancel | `mouse_capture_releases_only_after_the_last_successfully_captured_button`、`lifecycle_cleanup_emits_one_lossless_release_for_each_reported_button`、`hud_capture_loss_cancel_releases_primary_before_pointer_gone`、`capture_region_*` |
| click分類 | 段3 `adjudicated_releases_use_upstream_six_point_click_distance`（5.999/6/6.001pt、scale1/2）、time/triple/raw-order/replay。vendor無変更の成立済み証拠を再利用 |
| wheel単位・mixed modifiers・消費・残量・実子 | `egui037_wheel_*`、`egui037_wheel_frame_*`。実immediate子と異なるroot pointer/wheel、childクリック由来操作ID、discard再passも含む |
| popup open/closeの入力所有 | `music_overflow_real_input_wheel_ctrl_wheel_and_close_frame_do_not_leak`、`music_overflow_real_key_close_forwards_loop_without_wheel_tail`、`music_overflow_pointer_stream_release_cancel_focus_context_and_replay` |
| 通常clipboard | 段3 egui-ime `clipboard_shortcut_modifier_truth_table_preserves_bridge_differences`と通常bridgeのsemantic/fallback/release回帰。Ctrl+Shift+C/X/Vを保持 |
| native clipboard | 既存C/X2件と追加V3件（下記）。OS clipboardを変更せず実native handlerを通す |

### 追加回帰

1. `native_clipboard_ctrl_shift_paste_normalizes_once_and_suppresses_raw_v`
2. `native_clipboard_empty_ctrl_shift_paste_suppresses_raw_v_and_release_read`
3. `native_clipboard_paste_waits_for_pending_ime_commit_before_reading`

## 4c GPU・native HUD

### 変更と判断

- `src/egui037_stage4_gpu_tests.rs`、`src/compare_wgpu.rs`、`src/panorama_wgpu.rs`: 実product callbackのprepare→finish_prepare→paintを実egui-wgpu rendererで実行し、offscreen textureのRGBAをreadbackする。WindowsではDX12、他OSではPRIMARYを使うfixtureであり、今回すべてのbackendを実行したとの主張ではない。GPU初期化不成立はpanicであり、成功/skipにしない。compare Main/Navigatorを同時prepareした場合のuniform独立性、透明背景/不透明背景への合成、panorama settle alpha=0/0.5/1を検査する。
- 製品shader・submit/present gate・surface寿命は変更していない。native HUDの実galley計測/描画共有、font変更・DPI変更時のcache失効、計測とhit rect一致は既存回帰を再確認する。
- delta全量/partial/free順序とheadless破棄、mipmap再生成/odd edge、surface取得失敗/非描画時のresource consumerは段3で成立済み。vendor無変更のoffscreen readback/submission証拠を再利用し、今回の本体filterと併記する。
- Win32 HWND/DComp/monitor DPI/実surface presentの実機成立をこの試験で代用しない。native IMEは段3のintent/control検証を再利用する。

### 追加回帰

| 回帰 | 検査 |
|---|---|
| `compare_offscreen_callbacks_keep_main_and_navigator_uniforms_distinct` | 異なるUVのMain/Navigatorを同時prepare、描画後の赤/青pixelを検査 |
| `compare_offscreen_callback_composites_translucent_pixels` | 半透明textureを透明/不透明背景へ実合成、RGBA各成分差1以内 |
| `panorama_offscreen_callbacks_composite_settle_alpha_zero_half_and_one` | base RGB=(20,40,80)/settle=(220,140,40)、alpha3値を実合成、RGBA各成分差1以内 |

## 4b 比較準備

### 製品の明白な移行漏れを修正

`src/ui_main.rs` の統合folder rowは、旧2 Panelが予約していたseparatorの**外margin**を落としていた。新しいfont寸法の採択ではなく、Panel統一後のframe所有の移行漏れである。`Panel::resolve_frame`と同じ丸め済みstroke予約・実strokeの描画位置・次frameの背景開始位置を使う。既存の独立した実2 Panel oracleとのgeometry/全画素比較を維持し、stroke 0/1.5/2、Light/Dark、100/150%、tree表示有無を追加した。定数+1の症状修正ではない。

追加: `section207_folder_separator_reservation_matches_the_separate_panels`。既存 `section207_default_folder_geometry_matches_the_separate_panels` も成功を必要とする。

### snapshot fixtureの所有を補修

旧Context起点の固定サイズfixtureを機械的に `build_ui` へ移した箇所には、kittest側の8pt wrapperに加え、元fixtureのPanel/明示8pt frameが残っていた。比較へfixture由来のinsetが混ざるため、`tests/support/ui_harness.rs::full_viewport_ui` でviewportを所有する子Uiを作り、親へallocateせず元fixtureを描く。親frameの背景を拡大せず、clipをviewportへ戻す。fit-to-contentsや元Ui起点のfixtureには使わない。

対象は `tests/ui_snapshot.rs` の共通Context-root helper・folder toolbar・startup dialog（小画面/resize両経路）、libのannotation dialog・local adjustment snapshot helper。新しいroot oracle `context_root_fixture_panel_matches_raw_root_without_wrapper_margin` は独立したraw `Context::run_ui` とrect/interact rect/pointer受付/全RGBA/透明外周を比較する。baseline更新や画像の事後移動で辻褄を合わせない。

目視確認でstartupの旧Context-rootにもwrapperの背景拡大が混じっていると分かったため、同じhelperへ合流した。またkittest 0.37が新たに既定描画する合成cursorは `with_render_cursor(false)` で旧版同様のUI paintだけのcaptureへ揃える。入力event・hover/click・hit rect検査はそのままであり、製品cursor描画を変更していない。上記raw root oracleは共通builderを使って全RGBAを検査するため、合成cursorを含めていないことも確認する。選択外の個別Context-root snapshot fixtureは段5の全snapshot棚卸しで同型のwrapper残存を点検する。

folder比較fixtureのTextEditには描画を変えないaccessibility名を付けた。追加 `egui037_stage4_folder_toolbar_comparison_materials` は同じfixtureの通常/narrow/DPI150を描き、論理viewport内のinput・全固定操作と実pointer受付を検査してPNGを保存する。元 `folder_toolbar_flexible_snapshots` のalias assertも残す。0.37のwrapで異なるcaseが同じ描画になる差は、旧alias前提の失敗として報告し、成功に読み替えない。

### 段3からの7件の比較案（採択前）

段3台帳の第3版は変更せず、ここを段4の追跡表とする。旧寸法案/0.37案ともbaseline・期待値は未変更。案を実装した比較と誤認しないよう、画像は「保存済み旧baseline」と「現0.37」の2列であり、未実装の設定案は文章で示す。

| 段3からのtest | 今回の判断・観測 | 旧へ寄せる案と費用/副作用 | 0.37を採る案と到達性の条件 |
|---|---|---|---|
| `adjust_scope_action_labels_fit_258px_body` | action高25pt、旧assert24。258pt本文幅とは別 | 当該actionの縦paddingを限定再調整。クリック領域と他buttonの高さの再確認が必要 | 25ptを本文予算へ反映。固定操作が欠けず末尾までscroll可能な根拠を添えてから期待値判断 |
| `version_highlights_initial_width_keeps_previous_reading_area` | 1093×614/200%で旧546ptに対し537pt。Window→Resizeの既定window margin制約 | そのdialogのframe/Resizeサイズ算定を見直し、画面外に出ない旧reading areaを確保。vendor全Windowのclamp変更は範囲を広げるので別設計 | 537ptと折返しを採用し、本文Scrolls・末尾/固定Closeを保証。100%比較と200%現描画を併記 |
| `section207_reported_controls_keep_left_input_right_order` | width1487/zoom1.5でwrapし旧950pt閾値と不一致 | 固定controlの実幅から同一rowの予算/切替条件を再設計。input幅を削らずpointer reachabilityを維持する費用 | 複数rowを認め、nodeのviewport包含・最小input幅・全操作の到達性を契約にする。単なる順序assert緩和はしない |
| `section207_default_folder_geometry_matches_the_separate_panels` | separator外margin移行漏れ。今回修正、独立oracleを維持 | 実Panelと同じ予約を復元済み。font/spacingは変えていない | バグを新寸法として採る案は提示しない。追加stroke matrixも成功が必要 |
| `bubble_body_stays_within_panel_width_with_fill_on_or_off` | fillあり268.21875pt/本文268、fillなしは通る | 色/slider行の実available幅で固定幅予算を配分。縮小後の操作領域・末尾を再確認 | 必要な最小body幅を明示的に拡げる/折り返すlayout案を提示し、実描画がclipしない根拠を追加。誤差許容だけの期待値変更はしない |
| `a_combo_stays_inside_the_panel_only_when_we_reserve_its_place` | WidgetAtomの自動wrapで旧negative controlのoverflowが0 | 全体を旧layoutへ戻すにはAtom/wrap境界を触る。製品のslot予約は残っており、negative controlのための変更は推奨しない | 本体のviewport包含・最小input幅・固定操作に基づき試験目的を再設計。元assertは利用者判断まで保持 |
| `hex_focus_transfer_keeps_the_new_editors_partial_input` | margin外の入力差。4a受入の保留事項 | 同一batchのpointer focusとTextEditイベント処理を正しいownerで揃える別設計が必要。vendor共通TextEditの影響範囲がある | 操作を次batchへ分ける変更は利用者の操作変更承認が必要。今回採らず、期待値も変えない |

### §1.380との区別

clipboard失敗cardの可変エラーとvirtual row高、長い保存先名、annotation詳細/listの縦min sizeと沈み、crop/trim frame予算などは既存§1.380の残件である。今回のseparator予約・fixture追加inset・bubbleの水平268.21875pt・HEX入力順とは別。今回の比較だけで既存残件を解消済みにしない。font previewは既存ab_glyph workerのままで、本体の新rasterizerとの差を無条件に統一しない。Twemoji注釈SVGとfont fallback emojiも別経路を維持する。

## コマンド・選択件数・画像一覧

以下の17 filterはすべて `cargo test -p mimageviewer --lib <filter> --offline`。行ごとに選択件数を記載し、0件を成功としない。filter間の重複があるため件数をユニーク件数として合算しない。環境は `CARGO_BUILD_JOBS=1`、`MSBUILDDISABLENODEREUSE=1`、`CARGO_NET_OFFLINE=true`、`UPDATE_SNAPSHOTS` 未設定。

ログ/JSONは `target/egui037-stage4-final-<段>-<filter>.log/.json`。最初のclean HEAD上の実行も `final-` なしで保持し、下表を変異復元後の最終ソースの正本とする。

| 段 | filter | 選択 | 成功 / 失敗 / 既存ignore | exit | 秒 |
|---|---|---:|---|---:|---:|
| 4a | `ime` | 504 | 498 / 3 PNG / 3 | 101 | 206.04 |
| 4a | `focus` | 136 | 135 / 1 HEX / 0 | 101 | 11.95 |
| 4a | `keyboard` | 51 | 51 / 0 / 0 | 0 | 6.00 |
| 4a | `keymap` | 172 | 172 / 0 / 0 | 0 | 2.03 |
| 4a | `gamepad` | 105 | 105 / 0 / 0 | 0 | 18.83 |
| 4a | `specified_scale` | 70 | 70 / 0 / 0 | 0 | 16.40 |
| 4a | `fullscreen_overflow` | 8 | 4 / 4 PNG / 0 | 101 | 2.71 |
| 4a | `navigator` | 66 | 64 / 2 PNG / 0 | 101 | 6.32 |
| 4c | `compare` | 53 | 53 / 0 / 0 | 0 | 2.16 |
| 4c | `panorama` | 82 | 82 / 0 / 0 | 0 | 2.75 |
| 4c | `gpu_lanczos` | 53 | 53 / 0 / 0 | 0 | 1.28 |
| 4c | `native_presenter` | 231 | 225 / 4 PNG / 2 | 101 | 3.01 |
| 4c | `bottom_hud` | 4 | 4 / 0 / 0 | 0 | 0.83 |
| 4b | `ui_fonts` | 12 | 12 / 0 / 0 | 0 | 0.88 |
| 4b | `ui_font_catalog` | 7 | 7 / 0 / 0 | 0 | 0.80 |
| 4b | `calculated_` | 12 | 12 / 0 / 0 | 0 | 2.52 |
| 4b | `startup_dialog` | 5 | 5 / 0 / 0 | 0 | 0.88 |

`python scripts/check_ui_glyphs.py`: exit0、危険glyphのlint検出0（実行秒数は末尾の集計に記載）。全font/glyphの実機coverageを保証するlintではない。`ime` の時間には復元後のtest binary再コンパイルを含む。ignoreは既存のままで追加していない。

### 指定filterの失敗分類

| filter | 失敗名（共通prefix省略） | 判断 |
|---|---|---|
| ime | `timeline_bass_partial_raster_snapshot`、`timeline_key_states_raster_snapshot`、`native_video_adjustment_anime4k_panel_snapshot` | 全3件とも実画像比較まで到達したPNG差 |
| focus | `hex_focus_transfer_keeps_the_new_editors_partial_input` | 入力/focus順序差。4a判断待ち、成功扱いしない |
| fullscreen_overflow | `fullscreen_overflow_panel_snapshot_navigator_submenu` / `page_turn_on` / `root` / `root_tall_without_scrollbar` | 4件ともPNG差。tallのFits assertも維持 |
| navigator | 上記navigator submenu、`similar_preview_navigator_snapshot_dark` | 2件ともPNG差 |
| native_presenter | `native_top_bar_long_title_snapshot`、`native_video_adjustment_tone_panel_snapshot` / `lut_panel_snapshot` / `anime4k_panel_snapshot` | 4件ともPNG差。GPU/IME/galley/submit等の機能回帰と混同しない |

### 4cの既存回帰・source確認

| 契約 | 今回の証拠・再利用する証拠 |
|---|---|
| callback資源・uniform・透明合成 | compare53、panorama82、新規実GPU readback3件。Main/Navigator同時prepareとsettle alphaを検査 |
| mipmap・sampling・native texture lease | compare/panorama/gpu_lanczos filter内のNaga・size/format/sampler・lease寿命回帰。段3vendorの `managed_mipmaps_regenerate_after_partial_upload_and_respect_view_options`、`public_mipmap_generator_preserves_odd_edge_texels`、`mipmap_submission_tracks_later_writes_without_duplicate_empty_submit` の実GPU証拠を再利用 |
| delta配送・free・submit | 今回 `texture_output_` 2件。段3 `discard_texture_delivery` はegui4・eframe wgpu2/glow2・egui-wgpu2成功。実再pass前の移譲、offscreen submission/readbackはvendor無変更の同じowner |
| no surface / acquisition failure | 段3 `no_surface_drains_ordered_full_partial_submits_uploads_then_frees`、`acquisition_failure_equivalent_and_normal_submission_share_completion_owner`。native `present_batch` はset→upload→共通finish/freeを通り、submit/presentは既存gate内のまま |
| font/DPI/galley | 今回native presenter内 `native_tracks_hud_cached_labels_reused_by_real_draw_until_values_or_fonts_change`、`native_tracks_hud_dpi_820px_draw_and_scan_use_identical_hit_rects`。同じgalleyでdrawとhit計測、set_fonts/DPI変更の失効を検査 |
| surface寿命 | native既存DeviceEpoch/generation・surface swap・drop順/visualのpure回帰とsource確認。DCompのpremultiplied alpha/latency設定を維持。実HWNDや実monitorでのpresent成立は未確認 |
| native IME | 段3 `native_ime_interrupt` 3成功のintent/control/epochを再利用。今回ime/native_presenterにも選択される。実Windowは分離 |

egui/eframe/egui-winit/egui-wgpuに今回の差分はない。段3台帳の `ime-interrupt` 6、`egui-winit` 43、`viewport_input` wgpu5/glow5、`minimized_full` wgpu9/glow11の成功記録を再利用する。これらを今回新規実行したとは記録しない。

### 変異による追加回帰の検出力

`target/egui037-stage4-mutate-effective.py` は5ファイルを一時的に変異させ、終了時に元bytesへ復元する。8選択、2成功、**6失敗**、exit100。`target/egui037-stage4-mutation-effective-junit.xml` / `.json` / `.log` に保存した。失敗はcompile/GPU不成立ではなく、次の契約assertである。

| 一時変異 | 検出した追加回帰 | 観測 |
|---|---|---|
| compare paintのslotを常にMainにする | `compare_offscreen_callbacks_keep_main_and_navigator_uniforms_distinct` | NavigatorのRGBAが期待blueと不一致 |
| panoramaがGPUへ送るsettle alphaを常に1にする | `panorama_offscreen_callbacks_composite_settle_alpha_zero_half_and_one` | alpha0の画素がbase色と不一致 |
| native Pasteの改行正規化を省く | `native_clipboard_ctrl_shift_paste_normalizes_once_and_suppresses_raw_v`、`native_clipboard_paste_waits_for_pending_ime_commit_before_reading` | CRLF/CRがPasteに残る。空clipboard負例は引き続き成功 |
| 統合folder rowのseparator外margin予約を除く | `section207_folder_separator_reservation_matches_the_separate_panels` | 独立Panel oracleとinput yが49.5/51.5ptに分裂 |
| root fixtureを再び8pt insetする | `context_root_fixture_panel_matches_raw_root_without_wrapper_margin` | rect/clipが(8,8)へずれ、実pointer受付もfalse |

最初の試行 `mutation` は8選択/5失敗だった。panoramaでは描画に使われない診断用Refのalphaだけを変異させており、画素回帰が通るのは妥当だった。実uniformへ送るalphaを変異させた上記 `mutation-effective` で画素不一致を確認した。過去の試行も消さず、最終判定は実uniform変異の結果に基づく。変異復元後の回帰成功を別途必要とする。

startupでは抽象helperのoracleに加え、既存 `startup_dialogs_small_viewport` のarchive captureに、Window/shadowの外側かつ旧wrapper内部のRGBAが透明である検査を追加した。100/200%両条件で検査し、既存の包含・hover・click・snapshot比較を保つ。`mutation-startup-root-consumer` はこのconsumerだけを通常のallocating scopeへ戻し、**1選択1失敗、exit100 / 14.85秒**。新しい「Harness wrapperを描かない」assertでRGBA不一致を検出し、元bytesへ復元した。PNG不一致だけを検出力の証拠には使っていない。

### 実装担当の内部レビュー

分担した入力・GPU・separatorの差分と、後続のroot fixture/cursor補修について、実装とは別contextの `gpt-6.1-sol / xhigh` が読み取り専用で確認した。source確認はACCEPT、新たな指摘なし。root oracleは初回1選択1成功、後続の共通cursor設定を含むlib/integrationでも成功した。startup consumerの透明assert追加と変異結果もsource最終確認ACCEPT。HEXは4a判断待ち、実Window成立は未確認、旧PNGが存在しない条件の補足は旧/新と呼ばない、という区別も確認した。設計担当による独立受入を代替する記録ではない。detached/viewportの新しい受付・退役・pending設計は追加していない。

### 補完filter・代表snapshot・最終gate

nextestは `cargo nextest run -p mimageviewer <target> -E '<filter>' --no-tests fail --no-fail-fast --retries 0 --offline`。正確なfilter/引数は各JSONの `command` に保存し、同名の `-junit.xml` に全選択名/結果を保存した。runnerがlog内の個別test結果を一時表示する値は使わず、JUnitで選択件数を確定している。

| label / コマンド | 選択と結果 | exit / 秒 |
|---|---|---|
| `final-seven-and-new` / nextest `--lib`、7件＋新規回帰＋annotation/local adjustment PNG | 19選択、11成功、8失敗=未採択6＋PNG2、skip0 | 100 / 15.91 |
| `final-representative-integration` / nextest `--test ui_snapshot` | 19選択、4成功、15失敗=PNG14＋folder alias前提1、skip0。root oracle・新folder素材・既存高DPI負例・startup resizeは成功 | 100 / 36.88 |
| `final-supplement` / nextest `--lib`、下表の8 filterの和集合 | 101選択、98成功、3失敗はすべてmusic overflow PNG、skip0 | 100 / 5.61 |
| `final-ime-bridge` / `cargo test -p egui-ime --lib --offline` | 30選択/30成功/失敗・ignore0。通常/native clipboard共有truth tableを含む | 0 / 0.30 |
| `final-core-check` / `cargo check -p mimageviewer --bin mimageviewer-core --offline` | check成功 | 0 / 0.61 |
| `final-all-tests-compile` / `cargo test -p mimageviewer --tests --no-run --offline` | 全23 test executableをコンパイル、実行なし | 0 / 0.75 |
| `final-fmt` / `cargo fmt --check` | 成功 | 0 / 13.53 |
| `final-diff-check` / `git diff --check` | 成功 | 0 / 0.07 |
| `final-4b-glyphs` / `python scripts/check_ui_glyphs.py` | 危険glyph lint検出0 | 0 / 2.11 |

| 補完filterの部分集合（和集合101と一致） | 選択 / 成功 / 失敗 |
|---|---|
| `egui037_wheel_` | 31 / 31 / 0 |
| `music_overflow_` | 20 / 17 / 3 PNG |
| `capture_region_` | 14 / 14 / 0 |
| `native_clipboard_` | 5 / 5 / 0 |
| `root_lifecycle` | 11 / 11 / 0 |
| `viewport_retirement` | 10 / 10 / 0 |
| `context_state` | 8 / 8 / 0 |
| `texture_output_` | 2 / 2 / 0 |

追加した**9個のtest名はすべて成功**。lib側はnative Paste3・実GPU3・separator matrix1・raw root oracle1、integration側はfolder素材1である。共通root oracleはintegrationでも重複して実行し成功した。追加したstartupのconsumer画素assertも復元後に通り、同testの失敗は最後の集約PNG比較だけである。変異の失敗を残したまま引き渡していない。

`startup_dialogs_small_viewport` は30種×2画面×2 scale=120条件の包含・hover・activationを通過してPNG比較で失敗する。`startup_dialogs_refit_after_viewport_resize` は30種×2 scale×3 resize先の包含・clip・pointer受付を通過して成功した。実データの保存/worker/URL/アプリ終了はfixtureが破棄する。`spread_shift_hud_snapshots` もnormal/narrow・Light/Dark・resolved/unresolvedの操作包含と実clickを通り、PNG差のみ。fullscreen tallは元のFits assertを維持してPNG比較へ到達する。

補完のPNG3件は `music_overflow_snapshot_hud_narrow`、`music_overflow_snapshot_hud_normal`、`music_overflow_snapshot_panel`。integrationの唯一の非PNG失敗は元 `folder_toolbar_flexible_snapshots` のalias assertで、WidgetAtomのwrap変更により旧想定より多く同一画像になる。新素材fixtureのviewport包含・input最小幅・全6固定操作のpointer受付は成功している。元alias期待値は維持し、上の段3持越し2件（reported controls / combo negative control）のwrap採択と同じ判断材料へ紐付ける。全件113分類の再作成やlib全件実行は行っていない。

### 比較の一覧と判断待ち

索引: [比較資料README](../target/egui037-stage4b-compare/README.md)、[24組の縮小一覧](../target/egui037-stage4b-compare/00-contact-sheet.png)、[原寸/hash manifest](../target/egui037-stage4b-compare/manifest.json)。各組は保存済み旧PNGと0.37新PNGを原寸で横に並べた。画像に旧寸法案の未承認実装や事後的なglyph/位置補正は混ぜていない。索引に各比較の観測・旧へ寄せる設定案/0.37のままの案・費用/副作用を記した。

| 番号 | 比較画面 | 組数 | 主な判断材料 |
|---|---|---:|---|
| 01–04 | メイン一覧toolbar/folder、inline/stacked/通常/DPI150 | 4 | 字幅、wrap、status/input、最小幅と固定操作 |
| 05–06 | fullscreen overflow panel、通常/tall | 2 | 行高、Fits/Scrolls、末尾の固定操作 |
| 07–09 | native HUD 543/360pt、狭幅navigation | 3 | font/baseline、galley配置、狭幅 |
| 10–12 | 設定Light/Dark、local adjustment | 3 | font、button/row/min size、設定欄 |
| 13–16 | 起動1093×614、first setup 100/200%、archive 200%、What's New 100% | 4 | 小画面、折返し、本文Scrolls、固定footer |
| 17 | テキスト注釈/吹き出しdialog | 1 | previewと本文/shape controls、文字とframe |
| 18–19 | metadata emoji/symbol/CJK、mixed filename | 2 | 単色→色付きemoji、CJK/欧文/記号のbaseline |
| 20–21 | Meiryo Bold、Meiryo 10pt | 2 | 同じ選択font/保存値の字面、太さ、baseline/hinting |
| 22–24 | fullscreen見開き調整HUD、music HUD、fullscreen fit popup | 3 | 実HUDの文字、button、popup配置と到達性 |

全体一覧と15/18/21の原寸を目視確認した。startupの背景拡大とHarness合成cursorを補修してから再生成しており、製品外観差に数えていない。18のheart等は実際に旧単色→新色付きになっている。15は新描画で初期本文の表示高が減り、長いfilename/統計にscrollが必要となる。固定操作は両scaleで到達するが、旧と同じ本文表示量を今回採択したものではない。

補足の現描画は `current-materials/` に保存した。scope action高の画像/全rect、What's New 100/200%、bubble fill有無、1487/zoom1.5のreported folder rowを含む。What's New同条件200%とbubble詳細body単体には保存済み旧PNGがないため、旧/新2列を捏造せず現描画と元の寸法契約を明示した。

| 判断待ち | 参照・選択肢 |
|---|---|
| 字面 / baseline / hinting | 20/21、toolbar、HUD。現font/保存値を維持した0.37か、対象familyに絞る縦補正/hinting候補か。視覚scaleだけではlayout字幅は旧に戻らない |
| color emoji | 18/19。現色付きか、単色fallbackを別設計するか。glyph coverageと字面変更の費用、Twemoji SVGとは別経路である点を明示 |
| margin / min size / wrap / test前提 | 持越し7表のうちseparator1を補修。残る寸法/wrap5件は旧へ寄せる/0.37を採る2案を提示、期待値未変更 |
| HEX入力 | 持越しの残り1件。外観採択で解決せず、pointer focusとTextEditイベント順序の設計判断が必要。4a受入保留 |

### 引渡し時点

- **4a:** 指定・補完入力回帰を実行し、新規clipboard回帰も成立。ただしHEX1件の既知機能差を未解決のまま明示し、4a完了/受入済みとはしない。正しい修正には共通TextEditのfocus/イベント境界の設計判断が必要であり、局所の入力再送・遅延や操作制限は採らない。
- **4c:** 自動検査境界ではPNG差のみ。実Windowを分離したままGPU callback/readbackとnativeのpure契約を確認した。実機成立の受入は今回の範囲外。
- **4b:** 比較24組と補足、費用/副作用、7件の追跡・選択肢を用意した。利用者の採択はしていない。
- 開始時に固定したbaseline PNG **445ファイルのSHA-256はすべて不変**。期待値弱化・ignore追加なし。段3台帳・分類第3版・HUD/metadataの既存課題2件は変更なし。
- 最終artifact検証 `target/egui037-stage4-final-artifact-validation.json`: 24組、原寸/hash、最終Rust source11ファイルのhash、9新規test成功、索引の全linkを照合した。HEAD `65beeb2f0` / branch `egui-036` のまま。
- 段5へは採択後のPNG更新/Id警告整理と、選択外Context-root fixtureのwrapper棚卸しを渡す。§1.380既存残件を解消扱いにしない。
- cargo test greenの前提はHEX/未採択assertで成立していないため、通常profileのverification binaryは作成していない。製品起動/live/test-full/lib全件なし。git add/commit/branch操作/pushなし。
