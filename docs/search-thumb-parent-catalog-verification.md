# §1.741 実装の自動検証記録 (2026-11-09)

対象は `G:\home\mimageviewer-1741` / `fix-1741-search-thumb-reuse` の `8c4fd0def` + `9960419d2` と、その後の未コミット差分。
独立実装レビューの REVISE (P2 2件) を設計担当が確認し、今回修正した。独立再レビュー・検収は未実施。
正本は [設計 §16](search-thumb-parent-catalog-plan.md#16-実装メモ-2026-11-09独立再レビュー待ち)。

## 独立レビュー指摘の修正

- P2 (Ctrl+G): Ready 棄却後の `MetadataResources` 更新を Metadata / Everything 共通にした。
  owner 交換・cache clear 後も新しい resources で再投影し、identity 検証を通った結果を採用する。
- P2 (遅延ピン): 最終分岐がピン付きなら Parent から Fixed へ移し、同じ `CatalogWork` を保持する。
  親の通常表紙を読まず上書きせず、ピン行を一覧の Fixed へ保存する。先行色判定lookupも受付証明を検証する。
  外側キーは受付時に捕捉・継承し、誤った Parent 派生は stat/親DB lookup 前に assert/log で拒否する。
  clear を跨いだ保存は禁止し、CacheOnly の一回 RO 継続を維持する。

新規9件：

- `section1741_metadata_current_owner_replaced_while_ready_pending_retries_and_adopts`
- `section1741_metadata_prepare_owner_replaced_before_ready_retries_and_adopts`
- `section1741_metadata_ready_after_cache_clear_recaptures_admission_and_adopts`
- `section1741_color_pinned_zip_ignores_existing_unpinned_parent_row`
- `section1741_color_pinned_zip_saves_fixed_without_unpinned_parent_row`
- `section1741_color_pinned_pdf_ignores_existing_unpinned_parent_row`
- `section1741_color_pinned_pdf_saves_fixed_without_unpinned_parent_row`
- `section1741_color_late_pin_catalog_read_keeps_captured_admission`
- `parent_late_pin_fixed_preserves_admission_and_cache_only_continuation`

ZIP/PDF の4件はそれぞれスマートフォルダ・サブフォルダ展開の両 MultiParent で実行する。
既存 `parent_derived_key_mismatch_asserts_before_any_parent_write` も、初回 worker 前の派生キー変更・
parent_row 消去に対し stat 0 / catalog open 0 を検査する形へ補強した。
§1.731 と §1.741 の既存 Fixed assertion は variant の field 追加に合わせた機械的更新だけ。

実行条件は引き続き `MSBUILDDISABLENODEREUSE=1` / build jobs 4 / 通常 feature / test profile。
本体 lib 全件の外部実行とは Cargo lock を共有する。新規9件と補強1件の focused は10/10成功。
初回 compile は既存 Collection assertion のパターン更新漏れ1か所で exit 101（実行0件）。
型の field 追加に合わせて修正し、同じフィルターを再実行した。

| 検査 | 結果・ログ |
| --- | --- |
| 新規9件＋補強1件 focused nextest | **10/10成功**、exit 0、1.395秒（compile 4分40秒）。run `19e8fd76-af92-4013-a37b-1046a98ff586`、`target/section1741-review-fixes-focused-final.log` |
| 初回 compile の準備失敗 | exit 101、実行0件。`target/section1741-review-fixes-focused.log`。上記再実行で解消 |
| 関連回帰 nextest | **533/533成功**、exit 0、43.793秒（compile 1分13秒）。run `b9036848-c0c1-429a-94cc-a289020dbcd9`、`target/section1741-review-fixes-regression.log`。検索・Parent・色フィルター・ピン・EPUB・§1.731を含む |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 4` | exit 0、46.57秒。`target/section1741-review-fixes-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script -j 4` | exit 0、47.72秒。`target/section1741-review-fixes-check-test-script.log` |
| `cargo fmt` / `cargo fmt --check` | workspace 全体で実行、exit 0 |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0。通常 feature の dev-runtime core（4分12秒）と companion 4本を生成。VCRT 検査も runtime 4 DLL / PE 5本で成功。`target/section1741-review-fixes-build-dev.log`、`target/vcrt-pe-reports/dev-runtime.json`。製品バイナリは起動していない |
| `git diff --check` / 文書 encoding | exit 0。更新7文書は UTF-8 BOMなし / CRLF保持 |

lib 全件は今回の実装担当から追加実行していない。設計担当が管理する全件 gate と独立再レビューへ引き継ぐ。
UI 文言・レイアウトの変更はなく、snapshot / glyph 検査は追加実行していない。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/section1741_metadata_|section1741_color_|parent_late_pin_fixed|parent_derived_key_mismatch/)' --no-fail-fast
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/^thumb_loader::/) | test(/^global_search_ui::/) | test(/^catalog::tests::/) | test(/^app::color_filter::/) | test(/section1741|auto_aspect|idle_upgrade|collection_physical|folder_thumb_pin|folder_pin_to|zipdir_thumb_request|epub.*pin|pin.*epub/)' --no-fail-fast
```

## WIP 後の PDF メタ補完 (設計担当決定の実装)

Parent の catch-up を、observed のある親 DB hit に限定した。メモリ hit では過去の observed があっても
stat/親 DB open/catch-up を行わず、stat 失敗の RO hit でも補完しない。Fixed のメモリ hit と生成時の保存は維持する。
`maybe_spawn_pdf_meta_catchup` の受付境界をテストだけで捕捉し、実 PDF worker を起動せずに、
RW 接続が親 DB に渡されることと通常の MetaOnly handler による保存まで検査する。

追加4件 (`src/thumb_loader/catalog_provider.rs`)：

- `parent_pdf_memory_hit_does_not_stat_open_or_enqueue_catchup`
- `parent_pdf_observed_db_hit_enqueues_writable_parent_catchup_with_observed_meta`
- `parent_pdf_stat_failure_ro_hit_does_not_enqueue_catchup`
- `fixed_pdf_memory_hit_keeps_existing_catchup`

追補の全 Cargo コマンドも `MSBUILDDISABLENODEREUSE=1`。型チェック/nextest は build jobs 4。
追補の Rust 型チェックとテストは失敗0。確認用 build の native 準備結果は別記する。

| 検査 | 結果・ログ |
| --- | --- |
| 新規4テストの focused nextest | 4/4成功、exit 0、0.217秒（compile 2分23秒）。run `6f23bda8-fcb2-4b39-89d8-f1f8342ba00d`、`target/section1741-pdf-meta-focused.log` |
| 関連回帰 nextest | **424/424成功**、exit 0、34.665秒（compile 1.03秒）。run `79b63097-d397-4912-858e-acc5cbef19bf`、`target/section1741-pdf-meta-regression.log`。前回の成功証拠の再利用に加え、設計判断後の製品コードで全対象を再実行 |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 4` | exit 0、19.30秒。`target/section1741-pdf-meta-check.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script -j 4` | exit 0、19.37秒。`target/section1741-pdf-meta-check-test-script.log` |
| `cargo fmt` / `cargo fmt --check` | 最後のソース変更後に workspace 全体で実行、exit 0 |

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/parent_pdf_memory_hit_does_not_stat_open_or_enqueue_catchup|parent_pdf_observed_db_hit_enqueues_writable_parent_catchup_with_observed_meta|parent_pdf_stat_failure_ro_hit_does_not_enqueue_catchup|fixed_pdf_memory_hit_keeps_existing_catchup/)' --no-fail-fast
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/^thumb_loader::/) | test(/^global_search_ui::/) | test(/^catalog::tests::/) | test(/section1741|auto_aspect|idle_upgrade|collection_physical/)' --no-fail-fast
```

確認用 build の準備：

- `CARGO_BUILD_JOBS=4` での `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は
  初回の `turbojpeg-sys` CMake/MSBuild 段階で exit 101。製品テストの失敗ではない。
  script host の transcript は `target/section1741-pdf-meta-build-dev.log`。
- 生成済み TurboJPEG project を `cmake --build .\target\dev-runtime\build\turbojpeg-sys-ad9ab6b47bbc75e3\out\build --target install --config RelWithDebInfo --parallel 1`
  で直列ビルドし、exit 0。`target/section1741-turbojpeg-serial.log`。
  ソース・build script・個人設定は変更していない。
- ランタイム全体を `CARGO_BUILD_JOBS=1` で
  `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` により再ビルドし、exit 0。
  core、remote、EPUB PDF worker、MCP adapter、music-video editor の5本を生成した。
  VCRT PE 依存検査も runtime 4 DLL / PE 5本で成功。
  script host transcript は `target/section1741-pdf-meta-build-dev-serial.log`、
  PE 検査結果は `target/vcrt-pe-reports/dev-runtime.json`。
  実装担当は製品バイナリを起動していない。

## 初回 WIP の実行条件と結果

Windows / 通常 feature の test profile / nextest default。全 Cargo コマンドで
`MSBUILDDISABLENODEREUSE=1` を設定、build jobs は4。実装担当は製品バイナリを起動していない。
App テストは既存の一時 data-dir fixture、PDF は既存の fake render injection を使う。

| 検査 | 結果・ログ |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 4` | 初回 WIP の製品コードで exit 0、20.99秒。`target/section1741-check-final.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script -j 4` | exit 0、1分06秒。`target/section1741-check-test-script.log` |
| 最初の focused nextest | 47/47成功、exit 0。run `a87118d7-2f27-40b7-b9d2-c8396bb94504`、`target/section1741-focused-final.log`。後続の seed 障害時対応・scope assertion 補強前 |
| 関連回帰 nextest | 420実行 / 419成功 / 1失敗、exit 100。run `07675dea-3ac5-4f32-b9b4-67d5b59e78e3`、`target/section1741-regression.log` |
| assertion 修正後の影響範囲 nextest | 54/54成功、exit 0、5.935秒（compile 1分51秒）。run `e08c206f-30d2-4c54-81cd-0c2b59f6a1aa`、`target/section1741-correction.log` |
| `cargo fmt` / `cargo fmt --check` | workspace 全体で実行、exit 0 |
| `git diff --check` | exit 0。既存の autocrlf によるソースの改行警告は差分エラーではない |
| 文書の encoding | 更新した6設計文書は UTF-8 BOMなし / CRLF保持 |

回帰1失敗は追加したテスト helper の誤った assertion。
`collection_physical_stack_and_expansion_use_destination_key_policy` が使う展開一覧まで
「常に Fixed」を要求していた。設計は展開一覧を MultiParent としているため、
assertion を SingleFolder に限定した。製品コードや既存の期待キーを変えていない。
修正後54件は関連420件の部分集合で、残る366件は同じ製品コードでの直前の成功証拠を再利用する。

初回 test compile は新規 App テストの `setup_app` import 漏れ3件で exit 101（実行0件）。
修正後に上記を実行した。`target/section1741-focused.log` はこの準備失敗のログ。
本体 lib 全件・全 workspace gate は設計担当へ引き継ぐ。確認用 build の最新結果は上記追補に記録する。
UI 文言・レイアウトは変更していないため snapshot / glyph 検査は追加実行していない。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/^thumb_loader::/) | test(/^global_search_ui::/) | test(/^catalog::tests::/) | test(/section1741|auto_aspect|idle_upgrade|collection_physical/)' --no-fail-fast
cargo nextest run -p mimageviewer --lib -j 4 -E 'test(/section1741/) | test(/^thumb_loader::catalog_provider::tests::/) | test(/collection_physical/)' --no-fail-fast
```

## 初回 WIP で追加したテスト (34件)

以下は WIP `8c4fd0def` の親と関数名を比較した新規 `#[test]` の一覧。上記追補4件と合わせて38件。

### `src/app/tests/search_thumb_1741.rs` (3)

- `section1741_everything_zip_generation_reuses_parent_catalog_and_folder_selection`
- `section1741_parent_rows_are_only_unpinned_single_file_cover_branches`
- `section1741_storage_scope_and_root_keys_preserve_single_folder_contract`

### `src/global_search_ui/thumbnail_owner_tests.rs` (8)

- `section1741_nonsearch_pool_capture_never_borrows_outgoing_resources`
- `section1741_prepare_checks_replaced_owner_and_settings`
- `section1741_current_catalog_failure_retains_pool_identity`
- `section1741_prepare_loads_search_catalog_instead_of_origin_catalog`
- `section1741_prepare_catalog_failure_notifies_once_and_keeps_memory_owner`
- `section1741_prepare_catalog_open_failure_preserves_video_pin_seed_in_memory`
- `section1741_seed_catalog_write_failure_preserves_memory_without_retry_or_stale_seed`
- `section1741_search_pool_adoption_leaves_sibling_context_unchanged`

### `src/global_search_ui/everything/tests.rs` (2)

- `section1741_search_from_folder_replaces_pool_once_for_both_producers`
- `section1741_prepare_owner_replaced_before_ready_retries_with_current_search_pool`

### `src/app/release_checks.rs` (2)

- `section1741_release_search_image_observation_captures_parent_and_request_keys`
- `section1741_release_container_pin_witness_is_explicitly_unsupported`

### `src/test_script/release_checks.rs` (3)

- `section1741_release_probe_checks_parent_catalog_and_request_memory_independently`
- `section1741_release_probe_is_read_only_and_refuses_unknown_routes`
- `section1741_release_probe_preserves_captured_catalog_admission`

### `src/thumb_loader/catalog_provider.rs` (16)

- `parent_generated_row_uses_observed_but_memory_uses_request_and_survives_offline`
- `parent_inverse_lookup_observed_hit_populates_request_memory_without_rewrite`
- `parent_legacy_memory_hit_and_cache_only_never_stat_or_migrate`
- `parent_stat_failure_reads_request_signature_but_never_writes`
- `parent_source_only_and_auto_no_save_do_not_create_or_read_catalogs`
- `parent_raw_continuation_inherits_observation_without_second_stat`
- `parent_epub_observation_uses_prepared_generation_without_source_stat`
- `parent_clear_keeps_cache_only_ro_continuation_and_rejects_old_saves`
- `parent_same_names_use_distinct_dbs_and_lazy_kinds_do_not_alias`
- `parent_catalog_open_failure_keeps_source_display_and_records_notice`
- `parent_pdf_meta_uses_admitted_writer_observed_stamp_and_password_rules`
- `parent_pdf_meta_catchup_writes_parent_writer_without_touching_view_catalog`
- `parent_pdf_worker_render_saves_observed_thumb_and_meta_only_to_parent`
- `parent_epub_worker_legacy_memory_hit_then_source_only_saves_generation`
- `parent_source_only_worker_applies_kind_suffix_to_parent_and_request_keys`
- `parent_derived_key_mismatch_asserts_before_any_parent_write`

## 既存テストの補強

- `collection_physical_thumbnail_keys_match_ordinary_folder`
- `collection_physical_load_preserves_ordinary_thumbnail_rows`
- `ordinary_load_preserves_collection_physical_thumbnail_rows`

上記 §1.731 の3件に、Collection root / PhysicalSource の storage scope と Fixed admission の検査を追加。
同じ helper を使用する `collection_physical_stack_and_expansion_use_destination_key_policy` も再検査した。
既存 Everything の動画 pin seed / owner freshness テストは、新しい検索 owner の契約へ更新している。
既存 provider テストの Fixed ケースは、受付時に `RequestCatalog::Fixed` を選ぶ契約へ更新した。
既存のキー・署名・password 安全条件の期待は保持した。
