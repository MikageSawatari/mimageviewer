# egui 0.37 段5 検証台帳

対象HEAD `26c0226ca`、branch `egui-036`。2026-10-17、Rust 1.99.0 / Windows / egui 0.37.0 / wgpu 30、offline。開始時worktreeはclean。以下は**実装担当の内部確認**であり、設計担当による画像の目視採択は比較資料をもとに別途記録する。§5の字面/baseline/hinting、色付きemoji、margin/min size、HEX同batch操作差の採択を引き継ぐ。

## 対象集合・実行方法

段0bの `docs/egui-0.37-baseline.md` と `target/egui037-stage0b/lib-snapshot-inventory.json` を照合した。両integration target全体と、20生成元・50 lib filter（旧版103 test、macro生成8件を含む）を対象とする。source棚卸しで生成元/filterの欠落・改名はなかった。snapshot基準は `tests/snapshots/` の追跡PNG **445枚**。製品素材やfixtureのPNGを混ぜない。

更新前に全445枚をgitのHEADから `target/egui037-stage5-review/old/` へ保存し、worktreeのbytesとの一致を確認した。SHA-256は `target/egui037-stage5/png-before.json`。stage4からの新規/削除を勝手に加えず、更新後の全体分類もこの集合で数える。

更新は専用の子PowerShell `target/egui037-stage5-snapshots.ps1 -Mode update` 内だけで `UPDATE_SNAPSHOTS=1` を設定する。各cargoのexitはPython→子PowerShellへ伝播し、0選択も非ゼロで止める。通常比較は別の子PowerShellの `-Mode compare` で環境変数を除去し、同じ52 commandを実行する。MSBuild node reuse無効 / jobs1 / offline、既存の本体font・CPU adapter優先・DX12|VULKAN・Harness独立renderer・test並列性を維持する。

## 警告の基準と判断

段0bと同じくcargo JSONの `compiler-message` / level=warningを1診断1件で数える。子note/helpやcargoの総数表示は加算しない。通常coreとall-targetsは別グラフであり、lib/testの重複を含むall-targets総数を旧coreの722件と直接比較しない。生JSONと診断一覧は `target/egui037-stage5/*-{before,after}*` に保存する。

| コマンド | 段0b | 更新前 | 更新後 |
|---|---:|---:|---:|
| `cargo check -p mimageviewer --bin mimageviewer-core --message-format json --offline` | 722 | 654、exit0 / 16.78秒 | **254**、exit0 / 13.63秒（final2） |
| `cargo check -p mimageviewer --all-targets --message-format json --offline` | 採取なし | 1548、exit0 / 160.53秒 | **379**、exit0 / 45.66秒（final2） |

更新前core内訳: deprecated373 / dead_code232 / private_interfaces20 / unused_variables10 / unused_imports8 / unused_mut8 / unused_assignments3。all-targets内訳: deprecated1076 / dead_code310 / private_interfaces42 / unused_variables65 / unused_imports19 / unused_mut17 / unused_must_use9 / unused_assignments7 / unused_doc_comments2 / unused_parens1。

更新後はcoreが400件減、all-targetsが1169件減。coreの残りはdead_code232 / private_interfaces20 / unused_imports1 / unused_assignments1で、**deprecatedは0**。raw moduleの既存re-exportとRemote containerの既存priority代入は、別feature/APIや既存の処理に関わるため削除していない。Rust 1.99/egui 0.37に対応する置換を終え、残る既存警告を一括allowで隠さない。all-targetsはdead_code310 / private_interfaces42 / unused_imports9 / unused_must_use9 / unused_assignments3 / unused_variables3 / unused_doc_comments2 / unused_parens1、deprecated0（`targets-final2-warnings.json`）。通常coreと全target双方でvendor egui/eframe由来の非推奨を除去した。

途中の `targets-after` はtest用re-exportの可視性とfailure-injectionの可変束縛の2 compile errorでexit101だった。その補修後の `targets-after-fixed` は404警告・exit0で、さらにMSRV確認後のatomicと残るegui aliasを移して上表の最終379件となった。失敗時の不完全な297警告を更新後の総数として扱わない。

警告だけを理由に既存APIや機能を削除しない。crate全体のallow(deprecated)は置かず、IME producer/互換終端のイベントやassertは保つ。既存のdead_code/private_interfacesは、警告削減のための広範な所有・公開範囲変更を行わず分類する。

### ID値・IME互換・警告削減の境界

`Id::new` はvendor 0.37の実装で `Self::unique(source)` への直接委譲であるため、同じ引数の `Id::unique` へ移した。`IdSalt`への変更やsaltの追加はしない。`Ui::id` → `scope_id` も同じfieldを返すalias、TextEditのhorizontal_alignは同じLEFT_TOPからxを変えるCENTER_TOP、ScrollAreaのoffsetはclamped_offsetへそのまま委譲するaliasである。WidgetTypeは旧matchと同じAccessKit Roleへ移した。

回帰 `keyboard_input::tests::migrated_id_keys_preserve_root_child_and_session_memory_identity` は旧/newのstring・tuple・numeric root ID、`.with` した子ID、実memoryへの旧keyでの保存/new keyでの読出しを検査する。keyboard/touch/page-turn/IME/startup overlay/window系の保存keyの値を維持する。意図的に旧constructorをoracleにする3呼出しだけを関数局所のreason付きexpect(deprecated)で残した。rootのeframe dependencyはpersistence featureを有効にせず、App側のegui Memory serialize/deserializeやpersisted-map利用はない。将来egui memoryを永続化する場合も、alias置換では既存keyのhashは変わらない。settings.dbやHWND placementは別ownerであり変更しない。

IME互換イベント32関数はproducer・取消終端・検査を維持し、関数単位のreason付きexpectで意図を明示した。未使用Context cloneや可変指定などは副作用がないものだけ整理した。DB接続の代入による即時dropは `drop(connection.take())` に明示化し、migration前の解放時点を保つ。unused/private/dead警告は広範な削除や可視性変更によって隠さない。独立確認で指摘されたtest用re-exportの可視性とfailure injectionの可変束縛は維持して補修済み。

`fetch_update` → `try_update` は当初MSRV確認待ちだったが、ローカルrustdocで標準ライブラリのinline再exportを生成し、14型のmethod headerすべてが **Stable since Rust 1.95.0** と確認できた。証拠 `target/egui037-stage5/atomic-msrv-evidence.json` / `atomic-docs/atomic_msrv/struct.Atomic.html`（Rust 1.99のcompiled std metadata、ネットワーク未使用）。そのうえで15呼出しを同じ引数・Ordering・closureのまま改名した。workspace MSRVは1.95のまま。rootの通常/feature/testコードに加え、eframeの既存witness1箇所も同じ改名のみ。

## snapshot契約の補修

### Context-root fixtureの余白混入

段4の引継ぎに従って全20生成元を棚卸しした。egui_kittest 0.37の `build_ui` は外側に8ptの中央Panelを作るため、旧Context-root callbackをそのままUiへ置換したfixtureでは、製品にない外余白が重なっていた。これは利用者が採択した製品のmargin差とは分ける。libの18ファイル・47 callbackとintegrationの6 Panelを、段4でraw rootとの全画素・geometry・入力等価性を検証済みの既存 `full_viewport_ui` に通した。純粋なContext layer描画、既に補修済みのfixture、free-floating Windowは追加で包まない。全viewport内の試験fixtureだけの変更であり、製品レイアウト・assert・入力・予算は維持した。

補修前のupdate開始はpreflightでexit4に止め、PNGを書き換えていない。47 callbackの逆変換による元bytes一致は `target/egui037-stage5/wrapper-repair-inventory.json` に記録した。独立の読み取り専用確認（Sol/xhigh）は、atomicのMSRV・同義性、追加のID置換、lib47/integration6 wrapperをACCEPT。画像の一致や目視採択はこのソース確認に含めない。

`folder_toolbar_flexible_snapshots` のnormal/middle/inline/full controlsは、段4で採択したWidgetAtomのwrapによりrowと同一画像になる。旧「23個すべて異なる」前提を、**既存23個のbaseline名をすべて生成し、採択された22種類のpixel layoutを別に検査する契約**へ改める。名前選択まで新aliasへ変えると既存のinline PNGが生成されなくなるため、保存対象とpixel aliasを分離した。全36条件で既存の順序・寸法assertを維持し、viewport包含、input最小幅、固定操作の実hover受付とclipを追加する。製品layoutや許容誤差を変えていない。実行でmatrix全体の成立を確認する。

## PNG分類・比較資料

初回更新 `update1` はui_snapshot 186成功の後、book_composition_editor_snapshotsの13成功/1失敗（exit101を子へ伝播）で停止した。200%・1093×614の実カード全体のclip内到達assertであり、PNG差分ではない。新描画ではheader/tab/directive controlsの採択済み寸法が増え、結果list viewportが64pt、カードが74ptとなり下端10ptが切れる。旧PNGでは全体が収まる。従来は100%のPNG不一致で先に停止して200%へ到達していなかった。`docs/book-composition-plan.md` B2の「カード全体の実clip内到達」契約を維持し、期待値の緩和で更新しない。診断画像 `target/egui037-stage5/composition-200-diagnostic.png` を保存し、仮のcapture行は削除済み。独立した50 lib filterの更新は別の専用子PowerShell `update-lib` で実行する。

この経路はfixture余白ではなく製品の共通rendererの高さ配分に起因する。独立ソース確認も同判断。`src/ui_composition_editor.rs` で結果viewportから `ResultRowLayout` を一度計算し、ラベル帯と説明文の測定高を保ち、サムネイルだけを通常52pt上限から残り高へ縮める。画像のaspect/full image、操作、ラベル、正常高の52ptは維持する。行pitch・page card・表紙・白紙companionは同じ値を受け取る。font/header/footer/clipの拡大や旧spacingへの差戻しはない。既存integrationのカード全体assertをそのまま保持し、lib回帰 `bounded_result_cards_share_viewport_geometry_with_companions` で1x/2x・通常/短viewport・LTR/RTL・空/白紙/表紙の全24条件を実ScrollAreaの行とviewportで検査する。

同回帰を含む `ui_composition_editor::row_tests` は7選択/7成功、exit0。補修後のbook integrationも14選択/14成功、exit0で、元の200%のカードclip assertを通過した。追加範囲の独立ソース確認（Sol/xhigh）もACCEPT。実行証拠は `card-row-tests.json` / `update2/01-book_composition_editor_snapshots.json`。

別の候補として、book/media/reserved resume meter、音楽/動画サムネイル、cut表示、clipboard previewでfixture画像が黒くなる差を確認した。旧kittest0.33はdeltaのsetだけを適用しfreeを処理しなかったが、0.37のWgpuTestRenderer::handle_deltaはsetの直後にfreeも処理し、後からrenderする。関数内でload/draw/dropするfixture textureがcapture前に消えていた。epaintのmapは同frameのset/freeを保持し、製品のTextureDeliveryはpaint後freeのままである。これは色付きemojiや採択済み色変更には分類しない。fixture資産の寿命をContextへ持たせてcaptureまで保持する修正と、実RGBA readbackの回帰を行う。

`ui_helpers::load_snapshot_texture` をsnapshot専用6 producerへ接続した（grid_paint 3 / ui_main 1 / clipboard_capture 2）。同名の別画像やsampling差を混ぜないよう、name・size・全pixel・TextureOptionsを含むkeyでContextのtemp dataへimmutable assetを保持する。同じ資産は再利用し、Context破棄で終える。製品loaderやkittest/vendorのfree時点を変えない。回帰 `snapshot_texture_assets_survive_local_drop_without_name_or_context_aliasing` はlocal handleをdraw内でdropした後の実GPU readback、同名の赤/緑、別Contextの青/黄、反復passでの割当数不変、兄弟Context破棄後の画素を検査する。この追加範囲の独立ソース確認（Sol/xhigh）もACCEPT。

同GPU回帰は1選択/1成功、skip0・exit0（`texture-owner.json`）。adapter/GPU初期化やreadbackの不成立を成功にする分岐はない。

`folder_bar_organize_destinations_scrolled` はpointerを一時path付き行に置いたままcaptureし、0.37で出たtooltipへランダムなtemp pathが混入していた。試験の対象はscrolled menuなので、capture前に `PointerGone` を配送する。tooltip不存在に加え、元のwheel消費・末尾行のviewport包含・実クリック・正しいpathへの遷移・menu閉鎖を維持する。tooltip機能、製品のdelay、repaintには変更を加えない。

ページめくり55枚と回転6枚のpalette減少は、意図したNEAREST指定が0.37のpredictable shaderで適用されるようになったためである。旧0.33 shaderのpredictable branchはsamplerに関係なくmanual bilinear、0.37 `vendor/egui-wgpu/src/egui.wgsl` はTEX_FLAG_NEARESTでtextureLoadを使う。fixtureのTextureOptions::NEARESTを保ち、補間を勝手に旧版へ戻さない。これらは文字rasterizeと分けて「その他」とし、原寸比較も出す。自動のblank候補30枚はこのpalette減少による偽陽性で、ページの存在・向き・順・画面寸法を目視とsourceで確認した。

このfiltering差はkittestのRendererOptions::PREDICTABLEに関するものであり、製品の画像を新たにnearestへ変更したものではない。旧clip、白紙、非対称頁、回転、方向の既存意味assertも対象filterで維持する。

### 最終分類（補修後）

更新後の全445枚をgit版のSHA-256と照合し、**変更429枚・不変16枚**。canvas寸法の変更は0枚。旧/新の取り違え・新たな空白画像・未説明の欠落/色反転は確認されなかった。複数理由が重なるものは主分類で一度だけ数え、合計を445枚と一致させる。

| 主分類 | 枚数 | 共通理由・代表 |
|---|---:|---|
| 文字rasterize | 164 | 採択済みの字面/baseline/hinting・文字幅。text_contrast、UI font alignment、video thumbnail indicator |
| 色付きemoji | 17 | 採択済みのfolder/pin/保存印・metadata emoji。book/media/reserved meter、metadata_text_fallback_emoji_symbols |
| 寸法・余白 | 187 | 採択済みのmin size・wrap・spacing・widget形状。folder toolbar、設定/modal、構成editor、HUD。上記の実clip/操作回帰も維持 |
| その他 | 61 | sourceで確認したPREDICTABLE NEAREST補間差（page_turn_projection55枚、still seek rotation6枚）。幾何/順/白紙/向きに欠落なし |
| 不変 | 16 | page_turn_animation9枚、mouse/touch handle・hidden help6枚、timeline bass1枚。git版とbytes/hash同一 |
| 合計 | **445** | **429変更 / 16不変** |

比較資料は [索引](../target/egui037-stage5-review/README.md)。理由別contact sheet **38ページ**（font14 / emoji2 / layout16 / other6）、その他の**61原寸比較**、old/new各445枚を用意した。左右に同じ縮小率・市松背景を使い、原寸比較では両側のpixelを拡縮/移動しない。`manifest.json` に全445枚のhash・寸法・主分類をまとめた。個別の採択票を作らず、本台帳と共通理由・例外で管理する。内部分担の全429枚の確認と、補修後の21 texture画像・全28 composition画像・menuを再確認済み。最終画像に未解決の候補は残っていない。設計担当の目視採択は別途記録する。

## 最終検証・残課題

### snapshot更新と通常比較

最終の専用子PowerShell `update3` は全52 command成功、別processの `compare1` はUPDATE_SNAPSHOTSを除去して同じ全対象が成功した。更新時だけ `--nocapture` で診断を保存し、target/filterと既存の比較threshold/許容画素数は変更しない。`UPDATE_SNAPSHOTS=1` はkittest既定のUpdateFailingであり、`force`や許容差拡大は使っていない。

| 対象 | 更新選択/成功 | 通常比較選択/成功 | 更新/比較 exit |
|---|---:|---:|---:|
| `cargo test -p mimageviewer --test ui_snapshot --offline` | 186 / 186 | 186 / 186 | 0 / 0 |
| `cargo test -p mimageviewer --test book_composition_editor_snapshots --offline` | 14 / 14 | 14 / 14 | 0 / 0 |
| `cargo test -p mimageviewer --lib <filter> --offline` 全50 filter | 103 / 103 | 103 / 103 | 全50本 0 / 0 |
| 全52 command | **303 / 303** | **303 / 303** | **全0** |

全対象skip0、0選択なし。個別のcommand/件数/秒数/exit/ログは `target/egui037-stage5/{update3,compare1}/results.json`。下記のfilter表は両runの件数一致を機械照合して記録する。

### 最終gate

全test targetコンパイル、fmt、glyph lintを実行後、`scripts/test-lib-full.ps1` を最後に一回だけ実行する。live・製品起動・test-fullは実行しない。git add/commit/branch操作/pushなし。製品確認用buildと実機受入れは設計書の段6/7に残す。

初回の最終gateは `--tests --no-run` exit0（58.77秒）、fmt exit0（14.33秒）、glyph exit0（2.27秒、dangerous glyph0）。追加の `--features test-script` core checkで、警告整理により `_rating_menu` へ改名した変数をfeature側の登録だけが旧名で参照している1 compile errorを発見した（exit101）。`register_clickable_widget` も同じbindingを読むよう補修した。登録するResponse・ID・操作は維持する。この1行はtest-scriptだけの経路だが、最終sourceでfeature/core/all-targets check・全testコンパイル・全52通常比較・fmt/glyphを改めて実行し、その後lib全件へ進む。

比較資料の機械検査は `target/egui037-stage5-review/verification.json`。old445枚の元hash、新445枚のworktree hash、理由別contact38ページ、READMEの全45リンク、other61原寸比較のpixelを検査した。原寸比較の両画像領域は、同じ市松へ元RGBAを合成したものと全pixel一致し、片側だけの拡縮/移動はない。実装担当の内部確認であり、設計担当の目視採択ではない。

| 最終sourceでのコマンド | 件数・結果 | exit / 秒数 |
|---|---|---:|
| core check（通常） | 警告254、非推奨0 | 0 / 13.63 |
| core check `--features test-script` | 登録Responseの参照を含めコンパイル成功 | 0 / 36.97 |
| root `--all-targets` check | 警告379、非推奨0 | 0 / 45.66 |
| `cargo test -p mimageviewer --tests --no-run --offline` | 23 executableを生成 | 0 / 156.29 |
| 別子PowerShellでの全52通常比較 `compare2` | 186+14+103=303選択/成功、skip0、0選択なし | 全52本0 |
| `cargo fmt --check` | clean | 0 / 15.08 |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0 | 0 / 2.30 |

最終の通常比較は `compare2/results.json`。`compare1`と同じ全target/filter/選択件数であり、下の個別filter表も同じ。`UPDATE_SNAPSHOTS`は全通常比較で未設定。code/PNGの変更546入力（Rust117 + PNG429）のhashを `target/egui037-stage5/final-code-and-png-hashes.json` へ保存した。これ以降、lib全件終了までsource/PNGを編集しない。

### lib snapshot filter (update / compare)

| filter | selected (each run) | update / compare exit |
|---|---:|---:|
| `resume_meter_snapshot` | 14 | 0 / 0 |
| `everything_search_bar_not_responding_narrow_dark` | 1 | 0 / 0 |
| `local_adjust_combo_popup_is_dark_inside_a_light_app` | 1 | 0 / 0 |
| `page_turn_projection_snapshot_` | 23 | 0 / 0 |
| `local_adjust_panel_snapshot_` | 7 | 0 / 0 |
| `colorize_preset_slots_snapshot_single_row` | 1 | 0 / 0 |
| `colorize_gradient_preview_snapshot_custom_palette` | 1 | 0 / 0 |
| `image_creative_lut_popup_is_dark_and_shows_builtin_presets_in_light_app` | 1 | 0 / 0 |
| `sort_control_and_reload_fit_the_minimum_folder_pane_width` | 1 | 0 / 0 |
| `fullscreen_overflow_panel_snapshot_` | 4 | 0 / 0 |
| `similar_preview_navigator_snapshot_dark` | 1 | 0 / 0 |
| `still_seek_real_overlay_snapshots` | 1 | 0 / 0 |
| `last_page_supplement_visual_snapshots` | 1 | 0 / 0 |
| `page_turn_animation_mesh_visual_snapshots` | 1 | 0 / 0 |
| `music_overflow_snapshot_` | 3 | 0 / 0 |
| `still_seek_popup_regular_and_narrow_snapshots` | 1 | 0 / 0 |
| `still_seek_f6_real_overlay_rotation_snapshots` | 1 | 0 / 0 |
| `book_manager_numbered_dark_snapshot` | 1 | 0 / 0 |
| `file_organize_destinations_location_menu_navigation_and_snapshot` | 1 | 0 / 0 |
| `file_organize_destinations_many_location_entries_scroll_and_select_last` | 1 | 0 / 0 |
| `file_organize_destinations_location_toggle_snapshot` | 1 | 0 / 0 |
| `section292_facet_sort_snapshot` | 2 | 0 / 0 |
| `music_audio_track_selector_open_dark_snapshot` | 1 | 0 / 0 |
| `timeline_bass_partial_raster_snapshot` | 1 | 0 / 0 |
| `timeline_key_states_raster_snapshot` | 1 | 0 / 0 |
| `annotation_add_dialog_snapshot_dark` | 1 | 0 / 0 |
| `collection_manager_populated_dark_snapshot` | 1 | 0 / 0 |
| `collection_manager_import_preview_light_snapshot` | 1 | 0 / 0 |
| `collection_grid_remove_confirm_light_snapshot` | 1 | 0 / 0 |
| `collection_reorder_dark_snapshot` | 1 | 0 / 0 |
| `delete_confirm_modal_blocks_background_pointer_action` | 1 | 0 / 0 |
| `favorites_full_outcomes_and_maintenance_snapshot_dark` | 1 | 0 / 0 |
| `similar_index_progress_snapshot_dark` | 1 | 0 / 0 |
| `preferences_search_results_snapshot` | 1 | 0 / 0 |
| `preferences_context_menu_layout_snapshot` | 1 | 0 / 0 |
| `preferences_parallelism_pdf_count_snapshot` | 1 | 0 / 0 |
| `preferences_duplicate_files_epub_option_snapshot` | 1 | 0 / 0 |
| `preferences_recycle_bin_delete_confirmation_snapshot` | 1 | 0 / 0 |
| `preferences_folder_edit_restore_snapshot` | 1 | 0 / 0 |
| `preferences_viewer_notice_visibility_snapshot` | 1 | 0 / 0 |
| `spread_page_supplement_setting_snapshot` | 1 | 0 / 0 |
| `singleton_spread_placement_setting_snapshot` | 4 | 0 / 0 |
| `preferences_still_seek_strip_height_settings_snapshot_` | 2 | 0 / 0 |
| `preferences_video_seek_strip_height_settings_snapshot_` | 2 | 0 / 0 |
| `page_turn_animation_settings_snapshot_` | 2 | 0 / 0 |
| `operation_customize_diff_preview_snapshot` | 1 | 0 / 0 |
| `native_top_bar_long_title_snapshot` | 1 | 0 / 0 |
| `native_video_adjustment_tone_panel_snapshot` | 1 | 0 / 0 |
| `native_video_adjustment_lut_panel_snapshot` | 1 | 0 / 0 |
| `native_video_adjustment_anime4k_panel_snapshot` | 1 | 0 / 0 |

### lib全件（一回実行）と失敗分類

最終の `scripts/test-lib-full.ps1 -Force` を**一回だけ**実行し、**exit0**。runは `target/test-lib-full/runs/20261017T100547177Z-92795ee7`、source fingerprint `76a953e46610cd2653fa7c365b7cd2800c3c9f546a973e5881fa2c69a90574ee`。JUnit実行391.964秒 / wrapper全体427.85秒。UPDATE_SNAPSHOTSなし / retries0 / no-fail-fast / 既存profileとskip方針のまま。

| 区分 | 件数 |
|---|---:|
| 登録総数 | **13,507** |
| 実行・成功 | **13,448** |
| 既存skip | **59** |
| 失敗・error | **0** |
| PNG由来失敗 | **0** |
| layout/入力/機能回帰失敗 | **0** |
| 既存HUD z-order / metadata transferの今回の失敗 | **0 / 0** |

JUnitの全case数・skip・failure/errorをroot属性と照合した（`target/egui037-stage5/lib-full-classification.json`）。台帳の全103 lib snapshot case、およびID互換・短viewportのカード・texture資産寿命の3追加回帰がskipなしで成功した。既存の `hud_geometry_ignores_nozorder_but_external_zorder_still_requests_raise` と `zip_save_retired_owner_precedes_metadata_preview_import_and_export` も今回は成功した。両件の安定性を直したという扱いにはせず、設計担当の別管理を継続する。ignore追加、期待値の一括緩和、失敗の除外はない。

receipt `target/test-lib-full/d8bec910a63f6832b6a6375ae966fd06d3f4c5e1c5704f95f7479d6babd103d9.json`。試験後は結果の文書追記とPATCHES.md末尾の余分な空行だけを整理し、Rust/PNGの546入力は全件hash不変を再確認する。receiptの全tree fingerprintと結果追記後の文書は区別し、同じ実装・画像の証拠を引き継ぐ。

### 引継ぎ・残課題

- 設計担当による [比較資料](../target/egui037-stage5-review/README.md) の目視採択と、段5の独立受入れは別途記録する。ここでの分類・実行・ソース整合は実装担当の内部確認。
- 段6の非対話全体gate/確認用build、段7の承認付き実機確認、§1.380既存残件はこの段で完了扱いにしない。live・製品起動・test-fullは未実行。
- HUD z-order / metadata transferの既存テスト課題には変更を加えていない。今回の成功を恒久解決と読み替えない。
- 未説明のPNG差・新規機能回帰・crate不足・設計判断待ちの新論点はない。コミット/ git add / branch操作 / pushは行わず、差分を設計担当へ返す。

最終照合 `target/egui037-stage5/finish-audit.json`: HEAD `26c0226ca` / branch `egui-036` 不変、staged 0、追跡PNG445・変更429、Rust/PNG546入力のhash不変、`git diff --check` exit0。結果追記後にも資料検査 `verification.json` が成功した。内部の読み取り専用確認（Sol/xhigh）は、警告・資産寿命・カードgeometry・台帳/索引の整合に必須修正なし。画像採択と設計担当による段5受入れをこの確認に含めない。
