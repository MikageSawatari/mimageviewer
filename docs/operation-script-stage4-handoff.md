# §1.363 段4 引き継ぎ（2026-11-25、作業途中）

**段4は未完、UIへ未接続。Rustのコンパイル・テストは未実行。** 利用者指示に従いcargo build/test/check、
アプリ起動、コミットを行っていない。独立レビュー・ClaudeCodeの受入・detachedの追加合意も得ていない。

止めた理由は、既存の軽量鍵で全producerを覆えないこと、および採用済みの論理場所を既存状態だけでは読めないこと。
個別producerへ新しい世代bumpを足したり、毎フレーム全件走査したりして進めない、という今回の指示に従った。
以下は本セッションのソース調査であり、実行時の観測や独立レビューの結論ではない。

## 実装した部分と残した境界

- `src/operation_script/loader.rs` の `CompiledOperationScript::evaluate_event`：既存の上限付きengine・出力parseを
  slotと共用。on_event無し、全eventsの順序、操作数制限の回帰を追加した。
- `src/app/operation_script_runtime/events.rs`：main専用のbaseline、検出済みROOT frame番号u64、直近10回の
  実行時刻、起動中1回の上限通知判定を単一ownerに置いた。Appの`operation_script_events`はbundle外。
  `begin_operation_script_event_detection`はOFF／受け口無しなら利用資格も収集しない。利用不可ならbaselineも
  frameも触らず、使えるpassだけROOTの`cumulative_frame_nr_for(ROOT)`で1回にする。
- `observe`はopen/view/page/selectの順でまとめて返し、イベント無しでも呼び出し側が先にframeを記録する。
  selectは（location, Option<typed cursor item>）で比べる。チェック集合は含めない。初回空一覧はopenのみ、
  以後別の空一覧へ移ればopen/select。viewerの面ではselectを出さず、gridの面ではpageを出さない。
  cursorの値自体はviewer中も保持し、同じcursorのまま一覧へ戻るだけではselectを作らない。
- `finish_event`はイベント起動直後の取り直した識別を採用する。slotの実行経路には追加せず、キーからの変化は
  比較に残る。`admit_execution`はrolling 1秒・最大10回、超過はDropと通知要否だけ返し、予約を持たない。
  **これらの結合をUIの実経路から呼ぶ処理は未実装。** 実際のevent評価→全件検査→操作→同frame再snapshotも未接続。
- `src/operation_script/contract.rs`：現在の識別を変えないAだけに明示的な印を付けた（下）。B〜Eと印無しAを
  safe Aの後に置いても検査全体が失敗し、実行可能なprefixを返さない回帰を追加した。
- `script/event_compare`は純比較とbaseline置換のms、イベント数を記録する。識別構築・Rhai・操作の時間は含まない。
  UIから未呼出しなので、実検出の計測結果と扱わない。

一覧・全画面・nativeの入力、描画、移動dispatcher、early return、cleanupは変更していない。
段3のROOT予備入力と演出中の転送の事実も変更しておらず、§1.897の解決を前提にしていない。

## 必要な設計判断1：reader orderの採用と軽量鍵

現在の`operation_script_page_identity`はreader orderを全件typed itemへ写す。compositionは保持済みArcの
同一allocationなら定数比較、別allocationでは意味を比較する（全ページ・directive）。これを毎frame呼ぶのは不可。
単にitems_generationやVecのpointer/lenを鍵にする案は、下の2経路で同じ鍵のまま内容が変わる。

1. `rebuild_visible_indices_impl`（`src/app.rs:77147`）はthumbnailでvisible_indicesを再構成するが、
   `details_lazy_visible_revision`はlazy列／全件metadata要求のあるときだけ進む。details_order_revisionも
   details側の変更だけで、通常thumbnailのvisible変更を覆わない。
2. `finalize_direct_media_reader_order`（`src/app/startup_ops.rs:2835`、代入2877）は同じsnapshot.generation_id・
   items_generationのままreader_orderを差し替える。既存のnavigation cacheをinvalidateするだけ。

`current_reader_order`（`src/app.rs:78340`）の分岐はDirectMedia.reader_order → installed collection rootの
visible_indices → Detailsのdetails_order／visible_indices。各source種別とcontext IDも鍵に必要。

| O(N)のproducer | 既存の変化の証拠と限界 |
| --- | --- |
| items共通採用 `install_new_items_inner` (`src/app.rs:46649`) | items_generationをbump。通常folder・ZIP/PDF・検索・aggregate・rating・collection等が共用 |
| `remove_items_batch` (`src/app.rs:49764`) | remove後にitems_generationをbump |
| `push_grid_item_pending` (`src/app.rs:23834`) | appendとnav invalidate、bump無し。lenは変わる |
| play-test動画注入 (`src/app.rs:34576`、bump34579) | 後続のitems_generation bumpがある。製品の通常採用とは区別 |
| ZIP列挙開始 (`src/app.rs:40857`) | items/visible/detailsをclear、bump無し。lenは変わる。完了の共通採用とは別 |
| Snapshot activate (`snapshot_ops.rs:1093`) | mem::replace後、1168でbump |
| Snapshot deactivate (`snapshot_ops.rs:1491`) | 差し戻し後、1510でbump |
| Snapshot materialize (`snapshot_ops.rs:1633`) | 差し替え後、1649でbump |
| DirectMedia basis正規化 (`snapshot_ops.rs:885`、truncate904) | items.truncateとvisible/details.retain、bump無し。長さ/sourceが変わる |
| DirectMedia初期採用 (`startup_ops.rs:2761`) | 2779でbump。後のfinalize（上記）は別producerで鍵が欠ける |
| SmartFolder retained復元 (`smart_folder.rs:8691`) | 8699でbump。未採用prepared payloadのremove_pathsは採用時にbump |
| context split (`viewer_context_registry.rs:5128`) | 後続fork完了・世代retireの範囲。context IDも必要 |
| visible_indices再構成 (`src/app.rs:77147`) | **通常thumbnailで既存revisionが無い**。in-place retainとallocation再利用もあるためVec pointer/lenは証拠にならない |
| details_order再構成 (`src/app.rs:78613`) | details_order_revisionが必ず進む。grid view切替のclearも進む |
| rating survivors (`src/app.rs:40040`／`rating_membership_ops.rs:141`) | items採用bumpまたは後続details再構成に含まれる |
| composition採用・更新 (`book_composition_runtime.rs:363`、1374) | 古いArcをmemoで保持すればArc::make_mutは別allocationとなる。意味の比較自体は現在どおり必要 |
| PDF content_typeのin-place更新 (`src/app.rs:83949`、100920) | ScriptItemIdentityの入力ではないため、このsemantic値を変えない |

既存`ViewerNavigationCaches`はinvalidateでsource等をNoneへ戻すだけで、公開revisionが無い。
nav_indicesのArcをproxyにする案も調べたが、`get_nav_indices`（ui_fullscreen.rs:34333）はcold O(N)だけでなく
spread_shift.resolveとdisplay-owner invalidationを行う。read-onlyの識別収集から呼べない。
read-onlyの`operation_script_navigation_indices`もVec clone／cold O(N)で、軽量鍵ではない。
invalidate前後がNone→Noneの場合もあり、cache sourceのOptionだけでは変更を区別できない。

**推奨案（判断待ち、実装していない）**：reader orderの採用を単一typed owner／immutable Arc publicationへ集約し、
その同じ採用ownerでlogical locationも保持する。scalar入力とO(N)の意味snapshotを分け、context IDとそのpublication
identityでmemoを再利用する。別案は既存navigation cache ownerへrevisionを持たせる設計変更だが、DirectMedia/source
採用も含めた完全producer監査が必要。既存invalidateの呼び出し位置ごとへad hoc bumpは足さない。

既存段2bの要求照合も同じsemantic値を使うので、判断後に同じsplitを使える。ただし構成のgenerationやprepared
geometryを意味の比較へ入れてはならない。O(N) memo／5万件の変更なし鍵frame計測テストは、この鍵の判断待ち。
既存ignored `perf_operation_script_page_identity`は旧構築／比較コストの計測として残した。
指示書で共有された既存release計測（cf1fce276、N=50,000）は構築1.1〜2.2ms（p95最大3.4ms）、
保持heap概算約15MB、比較1〜1.5ms。本セッションで再計測した値ではない。

## 必要な設計判断2：採用済みの論理場所

`operation_script_location`はTopLevelGridSurface全10種と検索3種をmatchで覆う：Folder、DriveList、
Search(Favorite/Global/Tag)、Snapshot、SubfolderExpansion、SmartFolder、ReadingHistory、Bookmarks、Rating、Collection。
PDF/ZIP等のBook recipeは採用済みlogical root/prefixを保持している（既存回帰あり）。

しかしOrdinaryList／ExtractedResultのphysical fallbackはcurrent_folderを読む。
ZIP開始は列挙完了前にcurrent_folderを進め、folder_observationも落とす。非本／動画混在の物理folderは
`ProjectionRecipe::for_physical`でOrdinaryListなので、前の採用済みpathをrecipeから取り直せない。
「pendingなら止める」だけでは、すでに採用された別surfaceや空一覧／失敗後を正しく区別できない。

検索もFavoriteのlast_executed、favorite_filter、or_mode、kind_filter、Tagのlast_executed等を読む。
Tagはlast_executedを更新してからworkerを起動（`src/app.rs:35713`、起動35733）、Favoriteも同様で（36139）、
同じSearch surfaceで再検索するとrows採用前にopenの条件が変わる。query欄だけの問題ではない。
したがって一覧の完了採用で確定したsourceを読むownerが必要。推奨案は上のpublicationへ合わせる。

SubfolderExpansion.rootsのVec複製やSearch条件のformatもO(N)／文字列長依存なので「scalar取得」に含めない。
Book recipeだけのretained/adoptionの回帰を追加したが、OrdinaryList／検索の解決を済ませたとは扱わない。

## Aの印と設計例の不一致

現在の印は**38操作**。以下はコードの経路監査と追加した未実行回帰による候補で、テスト成功の主張ではない。

| 印を付けたA | 根拠と回帰 |
| --- | --- |
| 画像4：FsZoomIn/Out、FsToggleMetadata、FsNavigatorToggle | zoom/pan/factor・side panel・navigator設定を変更。location/context/pageの入力とcursorを変えない。Single/Ltr/SplitLtr×自由回転0/15度で4種識別を比較する回帰 |
| 動画13：再生・mute・音量・loop・seek全7・seek strip | playerの再生/時計と既存の再生設定・stripだけ。native／egui video／musicを共通fixtureで確認する回帰。musicは既存非対応のstripを除く |
| 一覧21：列数1..20、詳細切替 | gridの識別はlocation/context/cursor。pageはgridでは対象外なので、列数操作がThumbnailへ切り替えても同一。Detailsから各操作を実行して比較する回帰 |

| 印を付けないA | コード上の理由 |
| --- | --- |
| 見開き6操作 | spread_mode、pairing origins、位置正規化と委譲を変え得る |
| FsReadingDirectionToggle | Single等でreading_directionそのものがpage識別の入力。見開きlayoutの正規化もある |
| FsPageTurnAnimationToggle | page_turn_animationがpage識別の入力 |
| FsFitModeCycle | reset_fullscreen_fit_transformが自由回転を0へ戻す。Split＋自由回転時にsplit_directionがNone→Someとなる反例 |
| FsBgCycle | resolve_visible_spread_pair→get_nav_indices→spread_shift.resolveで、再表示された保存基点がNotInView→Resolvedとなりpairing_originが変わり得る |
| FsPixelGrid | analysis_modeでpair照会が同じresolveを行う。背景とともに保存基点の具体fixtureによる反例を追加 |

現在の雛形on_eventはFsReadingDirectionToggleを返すので、§3.3／§5.1の「識別を変えないAのみ」と矛盾する。
PNG背景の例も現在のraw pairing-originを識別とする限り上の副作用がある。既存入力操作の動作や順序を変えたり、
新しいguardで特定状態だけ拒否したりせず、印を外した。雛形／設計例を変えるか、page識別またはイベント可の
契約を見直すかは設計担当へ返す。雛形本体はこの作業では変更していない。

## 再開後に接続する場所（今回は変更していない）

- Grid：ROOTの一覧キー処理より前。main_viewer_blocked等の既存owner判断を維持する。
- 画像／egui動画・音楽：`render_fs_body`内、IME更新後で転送キー・ページ一覧・通常キーの全てより前の1点。
  ウィンドウ内stillはROOT、それ以外は実際の子viewport pass。同frameのROOT予備キーは従来どおり先に処理し得る。
- native：ROOTの`poll_video`でnativeイベントを処理する前。NativeOverlayInputRoutingの既存snapshotを使う。
- 検出＋実行はROOT cumulative frame番号で共通に1回。on_eventは全件validateしてからsafe Aを既存実行関数へ渡し、
  失敗／上限の通知を処理する。イベント起動直後だけ同frame内で再snapshotする。

scalar/O(N) split・memo、構築込み`perf::event`検出区間、5万件long ZIP名／変更なしframeのignored性能テスト、
転送キーより前の実経路テスト、ZIP/検索の実採用テスト、event→操作→再snapshotのhandler結合は未完。
helperの直接テストや`event_compare`計時でこれらを済ませたことにしない。

## detachedへの触れ方

既存のdetached述語、viewport経路、窓の寿命、placement、取り付け、swap_fieldは変更していない。
新しい`begin_operation_script_event_detection`は既存`operation_script_eligibility`を再利用するだけで、
現context・現在のviewer表示先／遷移先でmain専用を判断する。新しいApp stateはmain専用でbundleに入れていない。
独立context・linked F12・切替中viewerのskip、parked siblingがmainを阻害しないこと、main一覧の継続は新規helper回帰。
既存detachedテストは変更していない。今回の変更について独立合意は未受領なのでdetached-rework-plan.md §11へ
合意済みの記録を足していない。UIへ接続するときは改めて触った関数・行と構造上の理由を記録する。

## 検証とテスト依頼

この作業で実行を許された確認はbare `cargo fmt`、glyph checker、`git diff --check`だけ。
Rustテスト・build/check、既存ignored性能計測、実機は未実施。検証用バイナリも利用者指示に従い作っていない。
実行結果：bare `cargo fmt` exit 0、`python scripts/check_ui_glyphs.py` exit 0（危険グリフ0）、
`git diff --check` exit 0。gitのLF→CRLFの注意だけが出た（空白エラーではない）。

ClaudeCodeのrun-heavyへ渡すfilter（新規＋変更した既存moduleを含む）：

```powershell
# 既存runtime回帰と新規events、contract/loaderの既存回帰も含む
cargo test -p mimageviewer --lib operation_script
# 既存画像・動画の共用実行／入力ownerテストは上のfilterに含まれる
# 旧O(N)の構築計測だけ。新memoや変更なしframeの証拠にはしない
cargo test -p mimageviewer --lib perf_operation_script_page_identity --release -- --ignored --nocapture
```

新規event回帰名（`app::operation_script_runtime::tests::events::`）：

- event_baseline_collects_all_changes_and_records_even_an_unchanged_frame
- event_view_changes_grid_image_video_and_audio_shares_video_context
- event_book_recipe_retains_logical_open_until_adoption_and_same_place_reload_is_quiet
- event_baseline_uses_typed_pdf_zip_halves_and_location_not_index_or_generation
- event_select_tracks_cursor_location_and_ignores_checks_and_same_place_reload
- event_empty_grid_select_keeps_location_but_initial_empty_has_only_open
- event_logical_location_ratings_and_collections_are_distinct_and_reload_is_not_open
- event_unmarked_direction_and_animation_actions_really_change_page_identity
- event_unmarked_fit_cycle_reenables_split_after_free_rotation
- event_unmarked_background_and_analysis_pixel_grid_can_resolve_pairing_origin
- event_script_resnapshot_suppresses_sync_changes_but_input_changes_fire_next_frame
- event_limit_is_rolling_ten_attempts_and_notification_is_lifetime_once
- event_rate_drop_advances_baseline_and_never_queues_a_late_replay
- event_detection_off_absent_loading_and_failed_do_not_claim_the_frame
- event_detection_mounted_context_and_linked_detached_skip_without_touching_main_owner
- event_detection_linked_f12_and_transition_skip_viewer_but_main_list_continues
- event_detection_edit_and_page_overview_coalesce_after_exit
- event_detection_native_overlay_uses_existing_snapshot_and_does_not_claim_blocked_frame
- event_safe_image_actions_preserve_all_four_identity_components
- event_safe_grid_actions_preserve_all_four_identity_components_even_from_details
- event_safe_video_actions_preserve_all_four_identity_components_for_native_and_music

加えて`operation_script::loader::tests::evaluate_event_uses_bounded_engine_and_delivers_every_event_in_order`、
`operation_script::contract::tests::validation_event_mark_inventory_and_mixed_batches_are_atomic`を追加し、
既存`validation_rejects_events_without_audited_mark`を現在の印へ更新した。
全fixtureは同時にsetup_appを二重に作らず、同じdata-dir mutexを再取得しない。

## 変更ファイルと確認の入口

行番号はこの作業の最終整形時点。Rustの意味や実行結果はrun-heavyでの確認待ち。

| ファイル:行 | 変更 |
| --- | --- |
| `src/app.rs:18655`、21976 | main専用event ownerのfield／初期値 |
| `src/app/operation_script_runtime.rs:18` | event helperのmoduleとApp field用の型公開 |
| `src/app/operation_script_runtime/events.rs:25`、59、107、127 | baseline/frame/times、比較、rolling上限、利用資格＋ROOT frame受付 |
| `src/app/operation_script_runtime/tests.rs:6` | 新規events回帰moduleの登録。既存回帰の中身は変えていない |
| `src/app/operation_script_runtime/tests/events.rs:32` | 新規21回帰。実行予定の全名は上記 |
| `src/app/operation_script_runtime/tests/events.rs:269`、291、307 | direction/animation、fit＋自由回転、背景/analysis pixel＋基点解決の除外反例 |
| `src/app/operation_script_runtime/tests/events.rs:615`、661、684 | 印のある画像／一覧／動画Aの4種識別不変を操作ごとに確認する回帰 |
| `src/operation_script/contract.rs:94`、602、643 | 38個の印、既存イベント拒否回帰の更新、全mixed batch拒否回帰 |
| `src/operation_script/loader.rs:44`、260 | event評価APIと既存slotとの内部共用、event評価回帰 |
| `docs/ui-responsiveness.md:44` | UI未接続、比較だけの計時、O(N)費用と計測未完の記録 |
| `docs/operation-script-plan.md:580`、1256 | A監査による設計例との不一致、段4の未完状態と引き継ぎへの参照 |
| `docs/operation-script-stage4-handoff.md:1` | 本記録（設計判断、producer、除外A、未完、検証依頼） |
