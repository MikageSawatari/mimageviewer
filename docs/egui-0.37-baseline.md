# egui 0.37 更新・段0b 旧版基準台帳

2026-10-16、`egui-036` / `94bd03828de513a917f711e834bd3e0145afb0c7`。
egui 0.33.3の基準を実装担当が採取した。製品・テスト・scriptは変更せず、
製品起動・live試験・コミット・ブランチ操作は行わない。実行時の挙動の確認記録ではない。
[段0aの検証](egui-0.37-stage0a-verification.md)を再利用し、
[設計書第4.2版](egui-0.37-update-plan.md)の段1〜7で比較する。
detachedの構造合意案は[リワーク計画§11末尾](detached-rework-plan.md#2026-10-16-egui-037-更新に伴う構造修正-合意待ち)。双方の合意は未取得。

## source・環境・feature

開始時の`git status --short`は空、branch・HEADは依頼と一致。

| 項目 | 採取値 |
| --- | --- |
| `rustc -V` | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| `cargo -V` | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| `cargo nextest --version` | `cargo-nextest 0.9.148 (cd1d6d546 2026-10-07)` |
| host / shell | `x86_64-pc-windows-msvc` / PowerShell 5.1.26100.9711 |
| toolchain / workspace MSRV | `rust-toolchain.toml`: 1.99.0、minimal＋rustfmt/clippy / 1.88 |
| check条件 | 通常feature、dev profile、`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1` |
| Cargo設定 | `rust-lld.exe`、`-C target-feature=+crt-static`、`FFMPEG_DIR=vendor/ffmpeg`、`LIBSQLITE3_FLAGS=SQLITE_MAX_ATTACHED=20` |
| Cargo.lock SHA-256 | `3678a05cf90a102ae9919023d75958a3c674816b1f51f8b82a608177c370ec47` |
| RAW fixture | `vendor/raw-samples/885.dng`あり、SHA-256 `6275226bfa2d86ba479130999d045ea0b8da6be4b011130c9eb91cd6a0eb2233` |

feature集合はmanifestと`cargo tree -e normal,build`の静的採取。追加featureのbuild/test成功を意味しない。

| 組合せ | root feature / 診断featureの接続 |
| --- | --- |
| 通常 | 追加なし（rootのdefault定義なし）。eframeのdefault＋wgpuを使用 |
| test-script | `test-script` → `eframe/miv-test-script-window-witness` → `egui/miv-repaint-witness` |
| portable | `portable`。上記診断featureは有効にしない |
| 診断portable | `portable,test-script`。portableと上記witnessを併用 |
| 通常lib / snapshot test graph | root追加featureなしでもdev-dependencyのeframeがwitnessを有効化。egui_kittestは`snapshot,wgpu` |
| その他のopt-in | `manual-e2e`、`dev-tools`（→`pack-build-tools`）、`pack-build-tools`。今回有効化・実行なし |

通常production graphのeframe解決featureは
`accesskit,default,default_fonts,glow,wayland,web_screen_reader,wgpu,x11`、
eguiは`accesskit,bytemuck,default,default_fonts`。
test-scriptではそれぞれ上記witnessが加わる。manifest上のglow feature解決と製品rendererの選択は区別する。
portableによるegui/wgpu版の変化はない。

| 解決済みpackage | 版 | 供給元 |
| --- | --- | --- |
| eframe / egui / egui-wgpu / egui-winit | 各0.33.3 | root patchの`vendor/`実体 |
| egui_glow / egui_kittest / epaint / epaint_default_fonts / emath / ecolor | 各0.33.3 | registry |
| egui-ime | 0.1.0 | `crates/egui-ime` |
| kittest / winit | 0.3.0 / 0.30.13 | registry |
| wgpu / wgpu-types | 各27.0.1 | registry |
| wgpu-core / naga | 各27.0.3 | registry |
| wgpu-hal / wgpu-core-deps-windows-linux-android | 27.0.4 / 27.0.0 | registry |

証跡は`target/egui037-stage0b/`の`tree-normal.txt`（dev辺も含む）、
`tree-production.txt`、`tree-test-script.txt`、`tree-portable.txt`、
`tree-portable-test-script.txt`、`metadata.json`。上表はWindows targetのtreeと照合した。

## rustc警告基準

`cargo check -p mimageviewer --bin mimageviewer-core --message-format json`はexit 0。
`compiler-message`の`message.level == warning`を1診断1件として数え、
package ID・lint codeで分類した。子note/help、Cargoの総数表示、cached診断の別加算はしない。
本体libは688件、vendor eguiは33件、ortは1件、合計722件。core bin自身の診断は0件。

| 種類（lint / 内容） | mimageviewer lib | vendor egui | ort patched | 旧版合計 | 0.37合計 | 差 |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| `float_literal_f32_fallback` | 413 | 32 | 0 | 445 | | |
| `deprecated`: `fetch_update` | 10 | 0 | 0 | 10 | | |
| `deprecated`: `Context::screen_rect` | 1 | 0 | 0 | 1 | | |
| `deprecated`: `std::f32::EPSILON` | 0 | 1 | 0 | 1 | | |
| `dead_code` | 222 | 0 | 1 | 223 | | |
| `private_interfaces` | 20 | 0 | 0 | 20 | | |
| `unused_imports` | 8 | 0 | 0 | 8 | | |
| `unused_mut` | 6 | 0 | 0 | 6 | | |
| `unused_variables` | 5 | 0 | 0 | 5 | | |
| `unused_assignments` | 3 | 0 | 0 | 3 | | |
| **計** | **688** | **33** | **1** | **722** | | |

f32 fallbackのprimary spanは計36ファイル。vendor分を含む445件と本体413件を混同しない。
本体非推奨11件のうち10件がatomicの`fetch_update`、残る1件は`src/ui_fullscreen.rs:11705`。
eguiの非推奨1件は`vendor/egui/src/containers/scene.rs:79`。
eframe / egui-wgpu / egui-winitから今回のcheckに出たrustc警告は各0件。
これは通常core checkの集計であり、lib test・portable・診断featureの警告総数へ外挿しない。
全crateの警告抑制や閾値変更は行っていない。

生JSONは`core-check.jsonl`、stderrは`core-check.stderr.log`、exitは`core-check.exit.txt`、
分類前の診断一覧（package / target / code / message / primary span）は`warnings.json`。
PowerShellのリダイレクト出力はUTF-16、加工したJSON・manifestはUTF-8。
段5でも同じcommand・feature/profile・集計規則で再採取する。

## UI snapshot対象集合

`git ls-files 'tests/snapshots/*.png'`は**445枚、30,572,520 bytes**。
repository全体の追跡PNGは597枚で、製品素材・Web素材・fixture等を含むためsnapshot基準には使わない。
445枚のpath＋SHA-256一覧は`target/egui037-stage0b/snapshot-png-sha256.txt`。
このUTF-8 manifestをLF正規化（CRLFをLFへ変換）した後のSHA-256は
`2660181e631b432106b7104cdb141077f5217aa58cfcf2d50427ceec63e9756e`。
画像の生成・更新・今回の比較実行・目視採択は行っていない。

| integration target | 実PNG生成入口（現HEAD行） | 段5の対象 |
| --- | --- | --- |
| `ui_snapshot` | `tests/ui_snapshot.rs`: 183, 486（try_snapshot）, 750, 820, 1020, 1272, 1507, 1534, 3491, 3564, 3991 | target全体 |
| `book_composition_editor_snapshots` | 同名`.rs`: 188, 278, 333, 472 | target全体。小画面footer・確認/blocked・確認modal・HUD entry |

libは実`harness.snapshot(...)`とそのhelper呼出元から列挙した20ファイル。
次表のfilterは部分文字列であり、`--lib snapshot`では拾えない名前も含める。
件数は段0a成功runのJUnitからfilter集合を照合した値で、今回の実行件数ではない。
全50 filterの和集合は103 test、当該runですべて成功・skipなし。
1 testが複数PNGを生成するため、test数とPNG数は一致しない。
マクロ生成の`page_turn_projection_snapshot_{single,spread}_{ltr,rtl}_{forward,back}`8 testも含む。
完全な選択名一覧は`target/egui037-stage0b/lib-snapshot-inventory.json`。

| lib生成元（snapshot呼出行） | filter集合（各要素を個別実行） | 選択test数 |
| --- | --- | ---: |
| `src/app/grid_paint.rs`:2805, 2861 | `resume_meter_snapshot` | 14 |
| `src/global_search_ui/everything/ui.rs`:193 | `everything_search_bar_not_responding_narrow_dark` | 1 |
| `src/local_adjust_effect_ui.rs`:10981 | `local_adjust_combo_popup_is_dark_inside_a_light_app` | 1 |
| `src/page_turn_animation/snapshot_tests.rs`:479, 628, 1148, 1547, 1698 | `page_turn_projection_snapshot_` | 23 |
| `src/ui_adjustment_panel.rs`:1667, 1693, 1718, 1787, 1924, 10795 | `local_adjust_panel_snapshot_`<br>`colorize_preset_slots_snapshot_single_row`<br>`colorize_gradient_preview_snapshot_custom_palette`<br>`image_creative_lut_popup_is_dark_and_shows_builtin_presets_in_light_app` | 10 |
| `src/ui_folder_pane.rs`:466 | `sort_control_and_reload_fit_the_minimum_folder_pane_width` | 1 |
| `src/ui_fullscreen.rs`:61997, 66766, 78025, 79913, 89182, 89270 | `fullscreen_overflow_panel_snapshot_`<br>`similar_preview_navigator_snapshot_dark`<br>`still_seek_real_overlay_snapshots`<br>`last_page_supplement_visual_snapshots`<br>`page_turn_animation_mesh_visual_snapshots`<br>`music_overflow_snapshot_` | 11 |
| `src/ui_fullscreen/tests/still_seek_menu.rs`:631 | `still_seek_popup_regular_and_narrow_snapshots` | 1 |
| `src/ui_fullscreen/tests/still_seek_rotation.rs`:864 | `still_seek_f6_real_overlay_rotation_snapshots` | 1 |
| `src/ui_main.rs`:26628, 29617, 29722, 29751, 30923 | `book_manager_numbered_dark_snapshot`<br>`file_organize_destinations_location_menu_navigation_and_snapshot`<br>`file_organize_destinations_many_location_entries_scroll_and_select_last`<br>`file_organize_destinations_location_toggle_snapshot`<br>`section292_facet_sort_snapshot` | 6 |
| `src/ui_music_panels.rs`:3573 | `music_audio_track_selector_open_dark_snapshot` | 1 |
| `src/ui_music_timeline.rs`:2800, 2856 | `timeline_bass_partial_raster_snapshot`<br>`timeline_key_states_raster_snapshot` | 2 |
| `src/ui_text.rs`:10874 | `annotation_add_dialog_snapshot_dark` | 1 |
| `src/ui_dialogs/collections.rs`:6639, 6683, 6716 | `collection_manager_populated_dark_snapshot`<br>`collection_manager_import_preview_light_snapshot`<br>`collection_grid_remove_confirm_light_snapshot`<br>`collection_reorder_dark_snapshot` | 4 |
| `src/ui_dialogs/context_menu.rs`:4617 | `delete_confirm_modal_blocks_background_pointer_action` | 1 |
| `src/ui_dialogs/favorites_editor.rs`:1810, 2150 | `favorites_full_outcomes_and_maintenance_snapshot_dark`<br>`similar_index_progress_snapshot_dark` | 2 |
| `src/ui_dialogs/preferences.rs`:5933, 5973, 5984, 6073, 6125, 6148, 6182, 6216, 6239, 6271, 6335, 6373 | `preferences_search_results_snapshot`<br>`preferences_context_menu_layout_snapshot`<br>`preferences_parallelism_pdf_count_snapshot`<br>`preferences_duplicate_files_epub_option_snapshot`<br>`preferences_recycle_bin_delete_confirmation_snapshot`<br>`preferences_folder_edit_restore_snapshot`<br>`preferences_viewer_notice_visibility_snapshot`<br>`spread_page_supplement_setting_snapshot`<br>`singleton_spread_placement_setting_snapshot`<br>`preferences_still_seek_strip_height_settings_snapshot_`<br>`preferences_video_seek_strip_height_settings_snapshot_` | 16 |
| `src/ui_dialogs/preferences/pages.rs`:10547 | `page_turn_animation_settings_snapshot_` | 2 |
| `src/ui_dialogs/settings_restore.rs`:2109 | `operation_customize_diff_preview_snapshot` | 1 |
| `src/video/native_presenter/overlay_draw.rs`:8738, 8850 | `native_top_bar_long_title_snapshot`<br>`native_video_adjustment_tone_panel_snapshot`<br>`native_video_adjustment_lut_panel_snapshot`<br>`native_video_adjustment_anime4k_panel_snapshot` | 4 |

段5では表の全filterを通常lib graphで実行する。例えば各filterに対し
`cargo test -p mimageviewer --lib <filter>`を使い、両integrationも全体実行する。
更新は`UPDATE_SNAPSHOTS=1`の専用子PowerShell内だけに限定し、各非ゼロexitを伝播する。
終了後は環境変数を持たない別processで両integration＋同じ全filterを再比較する。
filter変更時は選択0件を成功とせず、この生成元一覧と445枚の集合を照合する。
本体フォント、固定fixture、DX12|VULKAN、CPU adapter優先、lazyかつHarnessごとに独立したrendererを維持する。
PNG更新前にaccessibilityと入力/geometry回帰を解決し、以下へ画面群の共通理由と例外を記入する。

| 画面群・対象集合 | 文字ラスタ/字幅/行高・panel/margin・操作回帰の分類 | 共通理由・例外 | 採択/確認者 | 更新後の通常比較 |
| --- | --- | --- | --- | --- |
| | | | | |

## 再利用した検証・既知の失敗履歴

段0aの成功台帳は`target/test-lib-full/c0d801cb5e990b7261aa0fd993f542f44708627e5901f2faf9ec92f49b2457ef.json`、
runは`target/test-lib-full/runs/20261016T023209103Z-93afae00/`。
通常feature / test profile / nextest default、13,231 passed・0 failed・59 skipped、386.420秒、exit 0。
元のcommand・JUnit・環境条件と経緯は[段0a台帳](egui-0.37-stage0a-verification.md#設計レビュー修正後の検証)を正本とし、全件は再実行しない。

再利用の範囲は段0a最終ソースの過去成功結果。現HEADはその段0aをコミットしたもので、
本段は文書のみを編集する。文書まで含むsource hashは段0aの
`64f86f8ec4a6a3af23d4bab7a788aa7828dffe5fabaf6f9681839c2160f7ed8a`と
本段編集前の`d2d055ca4593653c94474016e95f19ac668fff39ce46252a9ca603eb3103c48a`で異なる。
環境hashも段0aの`8d84056a2dd8950bebf2fc8144c302d2752553431b98c05acf6f2358379c3bc6`と
今回の`5d90ea6bba64b3bebf4ffafbec2dec6167ebe349e51dfdaefcd7b483d23e3367`で異なり、
hashのみから環境差の内訳は確定できない。compiler/nextest版は同じ、test executable隣DLLの
native runtime hashは`67334d6cfc1adcf91410e592f910027fc5bc4b69e178e6613b1c7f3ce2e29d8f`で一致した。
`reuse-identity.json`へ採取した。自動台帳HITや現環境での全件再成功を主張するものではない。

既知の間欠失敗履歴は
`ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`。
段0a初回全件で`draw_submit_tests.rs:456`のold9可視性assert（progress=0.75）が1回失敗し、
単独再実行と最終全件では成功した。原因未確定。恒常的baseline失敗とは断定せず、
期待値・上限・fixture・ignoreを緩めて隠さない。初回のRAW fixture欠落37件は段0aで解消済み。

段1b・段4用filterの旧版選択数も同じ成功JUnitから採取した（substring一致、重複あり）。
追加のnextest実行はしていない。新設予定の`ime_interrupt`等に旧版成功件数を割り当てない。

| filter | 選択 | passed | failed | skipped |
| --- | ---: | ---: | ---: | ---: |
| tray | 47 | 47 | 0 | 0 |
| startup | 270 | 269 | 0 | 1 |
| keyboard | 50 | 50 | 0 | 0 |
| sidecar_restore | 62 | 62 | 0 | 0 |
| gamepad | 86 | 86 | 0 | 0 |
| ime | 489 | 486 | 0 | 3 |
| focus | 130 | 130 | 0 | 0 |
| keymap | 172 | 172 | 0 | 0 |
| specified_scale | 70 | 70 | 0 | 0 |
| fullscreen_overflow | 7 | 7 | 0 | 0 |
| navigator | 59 | 59 | 0 | 0 |
| ui_fonts | 12 | 12 | 0 | 0 |
| ui_font_catalog | 7 | 7 | 0 | 0 |
| calculated_ | 12 | 12 | 0 | 0 |
| startup_dialog | 5 | 5 | 0 | 0 |
| compare | 50 | 50 | 0 | 0 |
| panorama | 81 | 81 | 0 | 0 |
| gpu_lanczos | 46 | 46 | 0 | 0 |
| native_presenter | 216 | 214 | 0 | 2 |
| bottom_hud | 4 | 4 | 0 | 0 |

## 今回実行したコマンドと未実施範囲

| コマンド・検査 | 結果 / exit |
| --- | --- |
| `git status --short` / `git branch --show-current` / `git rev-parse HEAD` | 開始時clean、指定branch/HEAD / 各0 |
| `rustc -V` / `cargo -V` / `cargo nextest --version` | 上記の版 / 各0 |
| `cargo check -p mimageviewer --bin mimageviewer-core --message-format json` | 成功、722 rustc警告 / 0 |
| `cargo tree -p mimageviewer --target x86_64-pc-windows-msvc --prefix none --format '{p}'` | dev辺込み解決版採取 / 0 |
| `cargo tree --locked --offline -p mimageviewer --target x86_64-pc-windows-msvc -e normal,build --prefix none --format '{p} features=[{f}]'` | 通常、`--features test-script`、`--features portable`、`--features portable,test-script`の4組 / 各0 |
| `cargo metadata --locked --offline --format-version 1` | package・feature情報採取 / 0 |
| `cargo fmt --check` | 成功、`target/egui037-stage0b/fmt.log` / 0 |
| `git ls-files`＋Pythonによる診断JSON・PNG SHA-256・生成元・既存JUnit照合 | 上記の件数 / 0 |
| `Get-MivSourceId` / `Get-MivEnvironmentId` / `Get-MivNativeRuntimeId` | 既存test-tools helperで識別子採取のみ / 0 |
| `git diff --check` / 文書差分・生成元の機械照合 | 成功。設計書はリンク追加のみ、§11は末尾追加のみ、製品・test・script差分なし / 0 |

全live / ignored hardware、製品起動、`test-full.ps1`、全snapshot更新/比較は未実施。
`test-lib-full.ps1`の再実行なし。文書のみなので確認用製品buildは対象外。
設計書の445 PNG、f32 445件/36ファイル、本体lib 688警告と一致した。
設計変更はなく、段0bへの追記は本台帳と未合意§11案へのリンクだけとする。
