# RAW LibRaw S3 implementation handoff

Branch `raw-libraw`; design baseline `999c8e46b` (including 7.10 J). No commit or product launch. Remote and user-facing documentation are outside this diff.

## Design blocker: valid preview without a usable catalog rendition

Implementation is stopped for a design decision under the original brief. S3 is not complete and the diff is not ready for acceptance.

Confirmed state: validated `PreviewShown` + `ThumbnailState::Failed` with no resident thumbnail pixels + development `Blocked(Unsupported)` + colorization enabled. The catalog pipeline can reach failure even with a valid embedded preview: `src/thumb_loader.rs:4030` requires sufficient preview resolution, and `:4043` rejects the half-development path for unsupported sources. Half-development errors also publish a failed thumbnail. A validated fullscreen preview therefore does not guarantee a resident catalog rendition source.

The existing faithful rendition implementation (`src/app.rs:80870`, `:80885`) requires a Loaded catalog texture and thumb_pixels. The specified classification (`src/app/raw_page_store.rs:410`) preserves PreviewShown before examining blocked development; Terminal is limited to absent/failed previews. The final-effect gate correctly refuses raw preview pixels. Neither materialized readiness nor terminal failure can satisfy navigation, and `src/ui_fullscreen.rs:11241` excludes RAW catalog failure from rendition failure. The same shared readiness keeps the folder lock held.

Focused regression `src/ui_fullscreen.rs:48137` confirms indefinite Awaiting without any remaining producer: **0 passed / 1 failed**, exit 101. It is deliberately left as a failing regression in the uncommitted diff, not ignored or weakened. Log: `target/s3-blocked-preview-repro.log`.

The lead must define either a bounded, worker-produced faithful-display source independent of the catalog thumbnail, or a terminal presentation transition when development and catalog rendition are unavailable. Using the full preview for new synchronous processing on the UI thread, bypassing color/LUT, or adding an ad-hoc navigation release would contradict the current constraints; none was implemented. The lead-owned plan was not changed.

## Files changed

`docs/async-architecture.md`, `docs/detached-rework-plan.md`, `docs/display-pipeline.md`, `docs/raw-libraw-s3-implementation.md`, `docs/spec.md`, `src/app.rs`, `src/app/collection_navigation.rs`, `src/app/raw_page_store.rs`, `src/app/snapshot_ops.rs`, `src/app/test_script_support.rs`, `src/app/tests.rs`, `src/app/viewer_context_registry.rs`, `src/app/vram_accounting.rs`, `src/cache_maintenance.rs`, `src/canonical_image_loader.rs`, `src/creative_lut.rs`, `src/external_tool.rs`, `src/fs_animation.rs`, `src/fs_page_load_scheduler.rs`, `src/lib.rs`, `src/materializer.rs`, `src/pipeline_debug.rs`, `src/raw/executor.rs`, `src/raw/mod.rs`, `src/raw/raw_decoder.rs`, `src/test_script.rs`, `src/test_script/pointer_input.rs`, `src/tray_integration.rs`, `src/ui_adjustment_panel.rs`, `src/ui_analysis_panel.rs`, `src/ui_conceal.rs`, `src/ui_crop.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs`, `src/ui_dialogs/preferences/search_index.rs`, `src/ui_erase.rs`, `src/ui_fullscreen.rs`, `src/ui_raw.rs`, `src/ui_sns_split.rs`, `src/ui_text.rs`, `tests/snapshots/preferences_parallelism_pdf_count.png`, `tests/snapshots/raw_progress_dark.png`, `tests/snapshots/raw_settings_dark.png`, `tests/snapshots/raw_settings_light.png`, `tests/ui_snapshot.rs`.

## Implementation map

The references below describe the current diff, not the historical line numbers in the design plan.

| Rule | Implementation |
| --- | --- |
| 6.1 / 7.10 E | `src/app/raw_page_store.rs:41`: independent installed, preview and development states. Preparing owns cancellation and highest priority; publish rejects superseded tickets. `:270`, `:378`, `:431`: context-owned store, identity validation and the sole RAW cache writer. Info/preview/develop/error all pass the same gate. `src/app/viewer_context_registry.rs:1109`: registered ContextAsyncOwner, polling, parking, generation assignment, bundle construction/swap/drop. |
| 6.2 | `src/fs_animation.rs:92`: RawPreview holds optional pixels/texture plus canonical developed dimensions and load sequence. Development replaces it with ordinary Static; preview allocation occurs only after owner validation. |
| 6.3 / 7.10 A, G | `src/app/raw_page_store.rs:398`: one six-way classification. `src/app.rs:73557`: legacy load-state projection. `src/ui_fullscreen.rs:9193`: shared RAW navigation readiness for typed sequences and folder lock. Ready/Presenting are reclassified; terminal RAW counts as LoadFailed. Fallback requires decoded and orientation-accepted preview through `raw_fullscreen_fallback_allowed`. |
| 6.4 / 7.10 F | `src/app/raw_page_store.rs:1763`: atomic snapshot transfer takes state and cache entry before generation replacement, cancels work, verifies item identity, and restores both. `:1895`: discard removes owner, cache, pending work, dimensions, bbox and backlog. Park cancels active requests while preserving Done/Blocked; generation/drop use context-owned map disposal. |
| 7.1 / 7.10 E | `src/app/raw_page_store.rs:2097`: worker-only source fingerprint/info/preview. `:2228`: cancellable source preparation followed by executor development. D1 permit is released before executor submission/wait; canonical routing, clamp and panorama tee remain shared. Interim FsRawJobState/Full-only path removed. |
| 7.2 / 7.9 / 7.10 C | `src/app/raw_page_store.rs:646`, `:1962`: combined demand: every displayed source High, next two/previous one Normal. Cancellation uses that union; Preparing and Submitted promote. Paged/continuous cache retention and derived keep sets include the union, including distant cover helpers and every visible continuous source. Ordinary Static retention remains the existing keep range; no additional RAW disk/LRU retention. |
| 7.3 | `src/ui_fullscreen.rs:9372`, `:10620`: final-required color/LUT display only accepts faithful rendition after preview validation. Preview is display-only; edit/final inputs remain Static. Continuous reading uses existing one-page processing admission. |
| 7.4 / 7.10 J | `src/app/raw_page_store.rs:2053`: shared resolved-target edit predicate always allows non-RAW and requires Developed for RAW. All six tool entries, SNS direct entry, spread target switches, in-tool page switches and button availability check it before mutation; existing fullscreen-index predicates remain. |
| 7.5 / 7.10 B | `src/app/raw_page_store.rs:2083`, `src/app.rs:75542`, `src/ui_fullscreen.rs:26635`: canonical dimensions drive RAW layout in every stage, including clamped/derived textures, fit, Original, Z, rotation, spread and continuous layout. Spread pairing/offset also reads owner dimensions. |
| 7.6 / 7.10 G | `src/ui_fullscreen.rs:11096`, `:11797`: typed/legacy navigation and folder lock share classification/readiness. Target producer and backlog-upload admission share the existing fresh typed navigation decision. PreviewAbsent never settles on a warm thumbnail; failure is terminal; actual presentation retires the sequence. LatestSeek retains both current RAW preview and preparation requests, while preserving sibling contexts. |
| 7.7 | Consumer-by-consumer inventory follows. |
| 7.8 / 7.10 I | `src/raw/executor.rs:265`: typed Queued/Running/Cancelling state. `src/app/raw_page_store.rs:2396`: Japanese progress derived from owner/ticket; current development repaints at 100 ms. `:2429`: preview/develop presentation events carry context, request ID and elapsed time. Existing prefetch row renders RAW development. `src/ui_raw.rs:31`: shared settings/progress presentation and snapshot fixtures. |
| 7.10 D, F | `src/app/raw_page_store.rs:1905`, `:1940`: mounted/parked RAW source transaction, independent brightness transitions, development-only cancellation/backlog removal, existing processed holdover capture, page-local invalidation and reserved request ID. Physical Stale discards old identity and reacquires it on a worker. No global retained-AI epoch bump. `src/app.rs:72795`, `:73925`: fingerprint and brightness in retained-AI keys and completion validation; unrelated JPEG completion remains valid. |
| 7.10 F worker validation | `src/raw/raw_decoder.rs:27`: full NTFS timestamp and size fingerprint. `src/canonical_image_loader.rs:396`, `:483`: before/after info and preview validation. Executor validates queued product source before opening and after development, including failures; typed Stale is rejected by request identity when superseded. |
| 7.10 H | `src/materializer.rs:204`, `:624`, `:666`: request brightness snapshot, RAW-specific reuse key, per-request decode context. Both external-tool request producers supply current settings. |
| 7.10 I / 13 | `src/raw/executor.rs:452`: transactional desired parallelism; partial spawn failure retains previous desired, removes unspawned reservations and retires surplus workers normally. `src/ui_dialogs/preferences.rs:2436`: setting committed only after success, Japanese error returned to UI. Parallelism 1..10/default 3 and existing RawBrightness settings use unreleased settings fields, no migration. |

## Consumer inventory (7.7 and 7.10 G)

| Consumer | RAW behavior / reference |
| --- | --- |
| Display and processed texture priority | RawPreview branch, faithful rendition gate when final effects required; `src/ui_fullscreen.rs:9372`, `:10620`. |
| Load state, typed navigation and old readiness branches | Shared six-state classification, RAW Terminal as failure, Ready/Presenting recheck, producer/upload admission; `src/app.rs:73557`, `src/ui_fullscreen.rs:11096`. |
| Folder-move lock | Same RAW readiness predicate; valid faithful preview rendition releases lock before full development; Terminal releases it; `src/ui_fullscreen.rs:11797`. |
| Pass-through / faithful rendition | Validation gate precedes thumbnail/pixel lookup and cached rendition lookup; display-only output, no edit input; `src/app.rs:80779`, `:80859`, `:81063`. |
| Original-image hold | Valid embedded preview is permitted as display source; gated fallback; `src/ui_fullscreen.rs:10573`. |
| Loupe / overview navigator | Resolved display source and shared source-coordinate transform; thumbnail fallback is gated; `src/ui_fullscreen.rs:32169`, `:40013`. |
| Automatic margin bbox | RawPreview pixels and load sequence; recalculated after replacement; `src/ui_fullscreen.rs:35741`. |
| Spread dimensions / pairing / cache-presence layout | Canonical owner dimensions, RawPreview dimensions, RAW source-size layout in every stage, including spread offset; `src/ui_fullscreen.rs:16356`, `:40398`. |
| Edit result, erase, local adjustment, conceal, saved masks, synchronous adjustment, final composite, final AI and AI prefetch | Source pixels remain Static-only. RAW preview cannot enter these pipelines. AI prefetch indicators exclude RAW outside its development demand; `src/app.rs:72288`, `:72567`, `:78370`. |
| Export / copy / fullscreen comparison capture | Existing complete-final requirement retained; RAW preview cannot satisfy it; `src/ui_fullscreen.rs:45811`. |
| Grid comparison pin | RAW source is developed High while pending; Terminal reports failure; `src/app.rs:41547`, `src/ui_fullscreen.rs:44072`. |
| Comparison source preparation | Waits for development as well as fs_pending; `src/ui_fullscreen.rs:38446`. |
| Panorama | Static-only detection and canonical dimensions; full development uses existing high-resolution tee and page-local invalidation; `src/app/raw_page_store.rs:1708`, `:1905`. |
| Analysis / histogram | Static-only; preview stage displays Japanese development-wait text; `src/ui_analysis_panel.rs:1096`. |
| Color-search palette | Existing Static-only source unchanged; no palette cached from preview; `src/app/color_filter.rs:120`. |
| Pipeline debug | RawPreview recorded as missing full-source stage; processing/output dumps remain Static-only; `src/pipeline_debug.rs:303`. |
| Alpha, shrink warning, AI toast, hover dimensions | Alpha/full-processing remain Static-only; RawPreview top bar uses canonical dimensions and clamp warning; `src/app.rs:68428`, `src/ui_fullscreen.rs:26687`. |
| Ctrl+E export dialog | Existing canonical full decoder and executor retained (S2 routing); no embedded preview export. |
| Continuous fallback / pending | Thumbnail fallback gated; all visible RAW pages demanded High, pending uses classification, faithful rendition uses existing bounded per-frame admission; `src/ui_fullscreen.rs:37054`, `:37929`. |
| Coordinates / layout | Canonical source size for RAW throughout; thumbnail dimensions cannot be substituted before info; `src/ui_fullscreen.rs:26635`. |
| Paint resource / Lanczos identity and producer binding | RawPreview load sequence participates in identity; owner installs bind texture item ID like Static; `src/fs_animation.rs:217`, `src/app/raw_page_store.rs:1800`. |
| VRAM accounting | Optional preview texture counted in owning context; `src/app/vram_accounting.rs:14`. |
| Automated readiness | Distinct page_ready (validated preview/developed) and edit_ready (developed only); `src/app/test_script_support.rs:344`. |
| Still seek thumbnail strip | Catalog navigation thumbnails remain catalog UI; main-page fallback and overview rendering use the validation gate. |

## Documentation and deviations

Updated display-pipeline RAW stages/layout/color/source changes, async-architecture owner/demand/cancel/Stale, spec settings and detached-rework-plan section 11 context resource record. No detached predicates or viewport paths changed. The design plan remains owned by the lead.

Apart from the blocker above, no additional design deviation is intended; approved 7.10 J is part of the baseline. Implementation detail: an exclusive Resolving record represents the worker-only initial fingerprint discovery, rather than inventing a placeholder source identity or statting on the UI thread. The existing preview/development request IDs remain independent. Invalidated phases reject late completion immediately; the source transaction also reserves the next monotonic request identity.

## Verification

Results below are the last successful relevant runs before adding the focused failing design reproduction. Counts are per command and are not summed because filters overlap. All Cargo commands used `MSBUILDDISABLENODEREUSE=1`; builds were allowed to finish without runner timeouts.

| Command | Actual result |
| --- | --- |
| `cargo test -p mimageviewer --lib raw::` | 63 passed, 0 failed, 0 ignored |
| `cargo test -p mimageviewer --lib raw_page_store -- --test-threads=1` | 23 passed, 0 failed, 0 ignored |
| `cargo test -p mimageviewer --lib raw_canonical_layout -- --test-threads=1` | 1 passed, 0 failed |
| `cargo test -p mimageviewer --lib fs_page_load_scheduler:: -- --test-threads=1` | 11 passed, 0 failed |
| `cargo test -p mimageviewer --lib materializer:: -- --test-threads=1` | 33 passed, 0 failed |
| `cargo test -p mimageviewer --lib app::tests:: -- --test-threads=1` | 2,071 passed, 0 failed, 2 ignored |
| `cargo test -p mimageviewer --lib ui_fullscreen::tests:: -- --test-threads=1` | 623 passed, 0 failed, 1 ignored (before adding the design reproduction) |
| `cargo test -p mimageviewer --lib settings:: -- --test-threads=1` | 260 passed, 0 failed, 12 ignored |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences:: -- --test-threads=1` | 69 passed, 0 failed |
| `cargo test --test ui_snapshot -- --test-threads=1` | 58 passed, 0 failed |
| `cargo test -p libraw-sys` | 3 passed, 0 failed; 0 doctests |
| `cargo check -p mimageviewer --bin mimageviewer-core` | Exit 0 |
| `cargo check --bin mimageviewer-core --features portable` | Exit 0 |
| `cargo check --bin mimageviewer-core --features test-script` | Exit 0 |
| `cargo test -p mimageviewer --lib --features test-script preview_readiness_and_developed_edit_readiness_are_distinct_in_script_snapshot` | 1 passed, 0 failed |
| `cargo clippy -p mimageviewer -p libraw-sys --lib --tests --bin mimageviewer-core` | Exit 0, warnings (before adding the design reproduction) |
| `python scripts/check_ui_glyphs.py` | Exit 0; zero dangerous UI glyphs |
| `cargo fmt --all` / `cargo fmt --all --check` | Exit 0 |
| `git diff --check` | Exit 0 |
| `cargo test -p mimageviewer --lib raw_valid_preview_with_failed_catalog_and_blocked_development_cannot_leave_navigation_waiting -- --test-threads=1` | **Exit 101: 0 passed, 1 failed. Confirmed design blocker.** |
| `.\scripts\test-full.ps1` | **Not run**: stopped for the confirmed design decision; focused regression is not green. All three prerequisite release executables exist. |
| `.\scripts\build-dev.ps1` | **Not run**: design blocker unresolved and tests not green. No S3 verification binary was produced. |

Snapshot updates: one full preferences-page snapshot and three RAW progress/settings snapshots generated successfully, then all four PNGs visually reviewed. Japanese text, dark/light selection states and the complete RAW/PDF settings layout are readable without clipping. The subsequent unmodified snapshot comparison passed all 58 tests.

Earlier iterations included corrected compile errors and fixture failures: owner LUT fixture (19 passed/1 failed), materializer process ownership (32/1), navigation terminal draw transition (0/1), and the search-index shared-renderer assumption (68/1). Each was corrected and rerun successfully. A Windows test-executable linker lock caused one build retry after the concurrently running RAW fixture suite finished; it was not a product-test failure. No failure was hidden by broadening tolerances or ignoring the new reproduction. Logs are under `target/s3-*.log`.


## Open work

Resolve the design blocker and its failing regression, rerun the affected owner/navigation tests, then run the full gate and build-dev handoff. Independent review and real-device verification remain pending. No other design contradiction is currently recorded.

## Real-device verification

No product binary was launched. These checks remain pending after the lead decision, automated/full gates and a new verification build. Verify a large RAW folder with warm half-developed thumbnails, missing/corrupt/orientation-rejected previews, rapid paging with parallelism 1 and 3, distant cover/spread partners and multiple continuous pages, Original/Z/pan/rotation across preview/develop/final, colorization/LUT plus brightness changes, edit availability, physical overwrite during development, and independent viewer contexts. Remote and binding remain manual regression scenarios from the plan.
