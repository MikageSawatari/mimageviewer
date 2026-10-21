# §1.165 表示基準の素材所有 — selector境界の設計判断

2026-10-22 / 対象HEAD `76eeab77b`。**設計担当の決定により、selector素材化を含む一括修正を実装する。**

実装試作で「素材から生成元を逆引きする分岐」が増えたため、設計担当の指示
「逆に分岐が増えるなら、止めて理由を報告する」に従って停止した。
最初の停止時点では試作製品コードを作業ツリーから撤去し、実worker受理を通る再現テストを残した。
以下の確定範囲で実装を再開した。Unknownの判断関数・永続schemaは変更しない。

## 確認した違反と停止理由

通常thumbnailのworker結果は `poll_thumbnails` が `PagePixelDimensions::Thumbnail` として
記録する。正準寸法を別途登録しない実受理では、NormalResourceの保持後にLoadedを退去すると、
指定倍率のidx探索がsource寸法を失い、6144×8192からsampler 60×80へ戻る。
既存arrival fixtureの手動canonical登録は、この経路を覆っていなかった。

試作では受理済みdescriptorをresourceへ保持し、fit側のidx探索を削除した。しかし、既存selectorが
返すbare `TextureHandle` からdescriptorを再取得するため、cacheの列挙を行う第二のselector
`accepted_material_display_basis_for_texture` が必要になった。この逆引きは採用しない。

- 通過renditionは無加工時にcatalogのtextureを共有する。通常thumbnailと通過素材では
  旧4方式の100%契約が異なるため、texture IDでは論理上の素材を特定できない。
- final/edit/sourceも恒等変換時にallocationを共有する。通常表示と元画像表示の選択を失ってから
  cache優先順で復元すると、元画像／解析表示のlayoutへprocessed素材の基準が混入する。
- 逆引きのcaseを増やす過程で、局所補正のpreview、消しゴムpreview、独立uploadされる
  `erase_base_tex_cache` がそれぞれ抜けた。これは個別case追加で収束させる修正ではない。

boundedな独立構造確認でも、逆引きの拡張を止め、実際のselectorから素材を返す境界へ変えるべき
との指摘を受けた。これは完成レビュー・受入判定ではない。

## 確定した修正境界

既存のnormal/source/processed selectorが、選択したbranchでhandleと受理済みdescriptorを
一体にして返す。例えば以下の素材APIを正本にし、既存handle getterはその結果の射影とする。

```rust
resolve_fs_display_material(idx, include_thumb) -> Option<FullscreenPaintResource>
resolve_fs_processed_material(ctx, idx, original_preview_active) -> Option<FullscreenPaintResource>
resolve_original_preview_material(idx) -> Option<FullscreenPaintResource>
```

この三つはproducerを選ぶAPIであり、保持済みpresentationの選択とは分離する。
producer選択後に保持素材を重ねる表示APIは `FullscreenPaintPayload` を返し、
`NormalResource` と `PresentationMaterial` の両方をそのまま表現する。
保持素材からhandleを取り出してproducerを再選択する経路は作らない。

descriptorに固定するのは不変の寸法・由来だけとする。現在のfit方式、指定値、倍率制限は
描画時の設定から解決する。frozen captureは従来どおり設定を含むfit descriptorをsnapshotし、
この不変素材属性と責務を分ける。

各producerを選んだ時点で、そのproducerの保持済み値を添える。生成を伴う`ensure_*`の戻り値も、
描画・fit・保持へ渡す区間では素材のまま運ぶ。handleを返した後のidx／texture ID逆引きは撤去する。
画像の生成範囲・選択順位・受付世代・context境界・非同期処理の順序は維持する。

| 境界 | 確定・継承する内容 |
| --- | --- |
| thumbnail worker受理 | workerのsource寸法、生成coverage、既存GPU clamp関数から表示descriptorを確定しLoadedへ保持 |
| source／PDF／RAW受理 | sourceの不変metadata、PDF page-box/content type、RAW developed寸法を受理素材へ保持 |
| edit／AI／final | 入力素材の指定A/Oを継承し、旧4方式の完成raster原寸は実出力に対応付ける |
| 通過表示 | thumbnailの指定A/Oを継承。全面素材の旧4方式source契約は通過素材の受理時に確定 |
| 注釈preview／完成素材 | 指定A/Oは下地から継承。旧4方式の実sampler基準は注釈素材の受理時に確定 |
| 消しゴム・局所補正・隠蔽・下地 | 実際の入力ownerからdescriptorを継承し、preview昇格でも再探索しない |
| GPU変換・presentation・保持・capture | descriptorを値で継承。native出力への変換で100%基準を変更しない |

`DisplayOriginalDimensions` と `CanonicalPageDimensions` を分離し、coverageや注釈の座標を
物理100%へ流用しない。sampler密度は素材UV／native ROIの用途に保つ。
明示的な素材欠落・基準欠落だけを型で表し、描画側で寸法を推測しない。

## 一つのownerに保持する契約

一つのimmutable `MaterialDisplayBasis` に、旧4方式Single／Spread／Continuousと指定A/Oの
名前付き射影を保持する。これらは別のlifecycle状態ではなく、同時に存在する既存契約。
一つの受理値から構成し、別々のidx mapや独立した寸法更新経路は追加しない。

例: source 15000×20000、thumbnail 60×80、到着GPU raster 6144×8192。

| 消費者 | 現在維持すべき基準 |
| --- | --- |
| 単ページ・旧4方式の通常thumbnail | 実sampler 60×80 |
| 見開き・旧4方式の通常thumbnail | 到着予定raster 6144×8192（受入済み仕様差） |
| 連結・旧4方式 | 従来source/layout。processedのaspect選択・元画像／解析表示の分離を含む |
| 指定倍率・全flow | AI前raster 6144×8192 |

完成AI素材では旧4方式の実出力と指定のAI前基準が異なる。従って全契約を一つの寸法へ統一する
変更は今回行わない。RAW／PDFの既存layout単位も物理pixel型と混ぜない。
描画側はview/fit方式に対応する受理値を選択するだけとする。
legacy source-layoutはlayout単位と、単ページでそのaspectを正規化する受理rasterの
参照寸法を `LegacySourceLayoutBasis` に保持する。後者をlive座標寸法から引き直さない。
PDF thumbnailはown samplerの参照寸法を保持し、page-box metadataの再受理でも維持する。
source／派生PDFは自身のsource rasterを受理する。見開き・連結のlayout単位は従来どおりである。

Unknownの利用許可は既存の `resolve_preview_coverage_for_display` に残す。
current cropによる表示可否を受理descriptorへ焼き込んではならない。cropありでUnknownを受理し、
後でcropを解除した場合も、既存の動的許可判定へ戻れる必要がある。
旧行のsource寸法なしと不正な0寸法は区別する。source寸法なしに対する旧4方式の代役canvasは
受理時の明示的なlegacy契約として扱い、指定倍率の物理基準を描画時fallbackで作らない。

## 実装した所有境界

- `ThumbnailState::Loaded.display_basis` はworker受理時に確定する。sourceは自身が保持する
  寸法・PDF metadataをResourceへ射影し、派生cache／pendingは生成時のdescriptorを所有する。
- `resolve_fs_display_material`／`resolve_fs_processed_material`／`resolve_original_preview_material`
  はproducer選択を行う。保持済み素材は `fullscreen_paint_payload_for_page` が別に選ぶ。
  Normalは保持値を使い、Direct presentationも同じ選択元なら値を維持する。
  Native presentationにordinary入力がある場合は既存prepareへ渡し、そこでheld出力のreuseを判定する。
  `FsFrameState` の通常素材と代役もResourceを保持する。代役がcatalog thumbnailか加工済み通過素材かを
  描画側で再探索せず、全cache退去後もそのframeで選んだdescriptorを維持する。
- `fs_material_fit_input`／`fs_material_layout_size` は選択したPayloadのdescriptorだけを読む。
  単ページ・見開き外枠／各ページ・captureが共有し、連結reference／measurementも同じ素材selectorへ通す。
  元画像／解析表示のsource選択をprocessedの寸法探索で上書きしない。
  legacyの未描画placeholderだけは正準layoutを維持し、素材なしの指定100%へ流用しない。
  ページ送りの受付は同じfit resolverの型付き欠落理由を既存skip経路へ渡し、描画不能な入力をpinしない。
- 消しゴムのCPU下地は `EraseBaseSource` がpixelsとdescriptorを所有する。独立upload、
  worker pending／preview／確定昇格へ同じ値を継承する。注釈はactual final inputの
  key／pixels／descriptorを同時に受け取り、縮小previewでもAI前基準を失わない。
- texture ID／Arc pointerからの汎用descriptor逆引きを撤去した。navigator／SNSの
  captured source ID照合は既存素材ownerからResourceを借りる用途に限り、fit基準を推測しない。
- PDF thumbnailはcontent type未確定でも旧4方式のlegacy canvasを保つ。指定A/Oの未確定と
  legacy layoutの可用性を同descriptor内で分け、PDF metadataの既存採用境界でLoadedを再受理する。
  それ以前にcaptureしたResourceのdescriptorは変更しない。
  descriptorが変わったページの依存renditionだけを退役し、同値の再受理・兄弟ページには触れない。
  RAW thumbnailも別metadataの到着を待たず、workerのdeveloped寸法／生成coverageから受理する。

## 検証と検収の単位

実worker受理→NormalResource保持→Loaded／CPU thumbnail退去→再描画を、cropあり／なし、
Single／Pair／Continuous、Original／Specified＋縮小禁止／Pageで通す。
fixtureでcanonical寸法を別途登録しない。texture共有でも素材属性が混入しない回帰、
元画像／解析表示、消しゴム下地・preview昇格、注釈preview、Unknown受理後のcrop解除も対象とする。
既存のGPU clamp・AI前基準・RAW／PDF・多context・ページ送りの回帰を維持する。
PDFの既存描画fixtureも、page-boxをidx cacheへ記録するだけでなく、実際のStaticPdf受理と同じ
metadataをsource素材へ持たせる。連結Originalの可視unit範囲を小さいsampler単位へ変えてしまい、
非同期の許可応答時間によって表示ページ数が変わるfixtureにはしない。

共通descriptor導入とselector返却型の変更を一つの所有境界として設計レビューし、実装後に
関連nextest、ui_snapshot、test-lib-full、core check、cargo fmt、build-dev -PreserveRuntimeを行う。
製品バイナリの起動・コミットは行わない。detachedの受理素材保持へ触れた範囲は、実装時に
detached-rework-plan §11へ記録する。
現在の実装に対する最終実行ログは `target/material-owner-related-final.log`、
`target/material-owner-ui-snapshot.log`、`target/test-lib-full/` の成功台帳、
`target/material-owner-core-check-final.log` とする。最終件数・結果は引き渡し報告に記載する。
この下の停止時記録は、撤去した旧試作の履歴であり、現在の実装に対する成功証拠ではない。

設計担当はselector返却型の構造変更まで今回の一括修正範囲として確定した。
見開きOriginalの縮小→再拡大は7474になかった退行であり、後続化しない。
逆引きadapterのcase追加、既存方式の倍率契約の統一、再現テストのignoreは採らない。
連結回帰はprocessed素材の観測から保持する製品経路を通し、保持mapへ直接挿入しない。

## 停止時点の実行証拠

製品試作を撤去したHEAD＋再現fixtureに対し、`MSBUILDDISABLENODEREUSE=1` で実行した。

- `cargo nextest run -p mimageviewer --lib -E 'test(/display_basis_worker_accepted/)' --no-fail-fast`:
  compile成功、2件実行・2件失敗。見開きOriginalのRaster原寸と、連結SpecifiedのA/Oが、
  いずれも6144×8192から60×80へ変わる箇所で失敗した。受付前の手動canonical登録はない。
  SourcePixels→Finalizedのworker receiptを実handlerで受理した後に保持・退去を進める。
  ログ: `target/material-owner-reproduction.log`。
- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功。
  ログ: `target/material-owner-handoff-core-check.log`。
- `cargo fmt`、`cargo fmt --check`、`git diff --check`: 成功。
- ui_snapshot、test-lib-full、build-devは未実行。修正未完・再現テストが赤の設計停止であり、
  完成gateの成功を報告する状態ではない。アプリ起動・コミットは行っていない。

撤去した未完成試作は `target/material-display-owner-prototype.zip` と同`.patch`へ保存した。
この試作は逆引きresolver等の欠陥を含み、そのまま適用・受入する対象ではない。

## 変更ファイル一覧

### 製品コードと共有型

- `src/app.rs`
- `src/app/final_prepare.rs`
- `src/app/raw_page_store.rs`
- `src/app/viewer_context_registry.rs`
- `src/fs_animation.rs`
- `src/fullscreen_specified_scale.rs`
- `src/displayed_image_transform.rs`
- `src/gpu_lanczos.rs`
- `src/grid_item.rs`
- `src/lib.rs`
- `src/material_display_basis.rs`
- `src/ui_erase.rs`
- `src/ui_fullscreen.rs`
- `src/ui_fullscreen/canonical_demand.rs`
- `src/ui_fullscreen/page_turn.rs`

### 文書

- `docs/architecture-overview.md`
- `docs/async-architecture.md`
- `docs/detached-rework-plan.md`
- `docs/display-pipeline.md`
- `docs/material-display-basis-ownership-plan.md`

### 呼出元・回帰テストとfixtureの型追従

- `src/app/collection_grid.rs`
- `src/app/collection_navigation.rs`
- `src/app/composite_pixels_revision_tests.rs`
- `src/app/egui037_connection_tests.rs`
- `src/app/final_prepare/tests.rs`
- `src/app/final_prepare/tests/lifecycle_tests.rs`
- `src/app/final_prepare_budget/tests.rs`
- `src/app/final_prepare_tests.rs`
- `src/app/grid_paint.rs`
- `src/app/image_folder_zip_operation.rs`
- `src/app/material_demand_revision_tests.rs`
- `src/app/multiwindow_scenario_tests.rs`
- `src/app/original_material_tests.rs`
- `src/app/page_edit_owner_tests.rs`
- `src/app/page_edit_release_tests.rs`
- `src/app/paged_prefetch_tests.rs`
- `src/app/similar_navigation_tests.rs`
- `src/app/similar_preview_context_tests.rs`
- `src/app/snapshot_ops.rs`
- `src/app/source_texture_test_support.rs`
- `src/app/tests.rs`
- `src/app/tests/detached_binding_identity.rs`
- `src/app/tests/manual_nav_wrap_book.rs`
- `src/app/tests/still_seek_thumbnail_ownership.rs`
- `src/app/tests/thumbnail_request_lifecycle.rs`
- `src/app/vram_accounting.rs`
- `src/global_search_ui.rs`
- `src/ui_dialogs/preferences.rs`
- `src/ui_fullscreen/book_composition_tests.rs`
- `src/ui_fullscreen/canonical_demand/draw_key_tests.rs`
- `src/ui_fullscreen/canonical_demand/tests.rs`
- `src/ui_fullscreen/coverage_spread_tests.rs`
- `src/ui_fullscreen/coverage_tests.rs`
- `src/ui_fullscreen/display_basis_tests.rs`
- `src/ui_fullscreen/final_demand_topology_tests.rs`
- `src/ui_fullscreen/front_popup_input_tests.rs`
- `src/ui_fullscreen/overflow_input_tests.rs`
- `src/ui_fullscreen/page_turn/cover_tests.rs`
- `src/ui_fullscreen/page_turn/coverage_lifecycle_tests.rs`
- `src/ui_fullscreen/page_turn/draw_submit_tests.rs`
- `src/ui_fullscreen/page_turn/final_prepare_tests.rs`
- `src/ui_fullscreen/page_turn/input_regression_tests.rs`
- `src/ui_fullscreen/page_turn/landscape_tests.rs`
- `src/ui_fullscreen/page_turn/material_budget_tests.rs`
- `src/ui_fullscreen/page_turn/merge_master_tests.rs`
- `src/ui_fullscreen/page_turn/nonfinal_tests.rs`
- `src/ui_fullscreen/page_turn/permission_service_regression_tests.rs`
- `src/ui_fullscreen/page_turn/prepare_boundary_tests.rs`
- `src/ui_fullscreen/page_turn/roi_tests.rs`
- `src/ui_fullscreen/prefetch_revision_tests.rs`
- `src/ui_fullscreen/retained_material_entry_tests.rs`
- `src/ui_fullscreen/tests/still_seek_rotation.rs`
- `src/ui_main.rs`
- `src/ui_sns_split.rs`

