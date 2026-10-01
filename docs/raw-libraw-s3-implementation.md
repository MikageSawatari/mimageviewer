# RAW LibRaw S3 implementation handoff

Branch `raw-libraw`; S3 review baseline `ef0b2dac5`, following WIP `3e57109e4` and design decisions J (`999c8e46b`) and K (`453b0baa2`). The independent review rejected the baseline. The review corrections described below are uncommitted and require independent re-review. No product binary was launched; build-dev is explicitly deferred to the design lead. Remote and user-facing documentation remain outside S3.

## Independent-review corrections

The preview cancellation bug came from removing the pending receiver without retiring the request in `RawPageStore`. `cancel_fs_page_load` now owns both operations in paged and continuous prefetch, including animation-promotion cleanup. The equivalent held-page-turn deferral also uses that boundary. Resolving sources are discarded, Requested previews return to NotRequested, and re-entry allocates a fresh request ID. Completed receivers still disarm their ticket without cancellation so backlog uploads remain valid. The regressions exercise both prefetch routes before/after info, reject superseded info, re-enter the page through the real producer, and cover held-page-turn deferral.

The paint-time screen-filter gate now matches the embedded preview's texture identity independently of the development axis. Supported developing previews are unchanged, while display-only renditions and unrelated JPEG holdovers retain their filters. RAW Full completion follows ordinary-image admission for synchronous adjustment: only the current page runs it. Offscreen source completion remains within the existing bounded upload/final pipeline. The regression checks both offscreen exclusion and preserved current-page processing.

Continuous reading distinguishes a RAW development wait from work that can advance next frame. Such a wait is excluded from deferred/selected processing and from the fast pending-work repaint; current-page development requests 100ms when no display work requires the existing 16ms path. A real continuous draw checks the repaint request. Running progress below the shared processing boundary (35) displays reading; progress 35 and above displays developing. A channel-blocked executor job exercises the actual typed ticket and label through 0, 5, 34, 35, 62, 85 and 100.

The new layout regression draws through App's fullscreen state, single-page paint, Z paint, spread paint and continuous paint using a 1200x800 preview, an actual 8192×5461 texture, and a complete final texture. Source dimensions come from App, and paint rectangles and source/screen round trips are compared across stages, fit modes and rotation. The preferences regression edits the actual brightness control, clicks the real OK button, saves/reloads settings, reopens preferences, and verifies executor desired parallelism plus source/cache invalidation. It also exercises the executor-error commit path. The existing executor partial-spawn regression remains required. Actual `poll_prefetch` tests cover Full/preview in both backlog orders and preview arriving after Full installation.

No new design decision was needed. The only extension to the review directions is applying the same cancellation boundary to held-page-turn deferral, found by auditing equivalent receiver-removal paths. The render/settings/backlog findings were missing coverage of already-connected behavior; mutation checks validate that the new tests detect removal of those connections, rather than changing correct behavior to manufacture a baseline failure.

## Approved Blocked-preview behavior (7.10 K)

The previous failing regression exposed a validated preview with failed catalog thumbnail, blocked development and a final-effect gate that could never complete. The lead resolved this with K: Blocked development is processing-unavailable, and the embedded preview is displayed unchanged regardless of catalog rendition availability.

`src/app/raw_page_store.rs:2266` derives `raw_development_blocked` from the owner development axis in one place. The shared color/LUT gate (`src/ui_fullscreen.rs:9618`) excludes it. Both texture resolvers (`:9372`, `:10630`) select the original preview before processed caches. The shared rendition getters (`src/app.rs:80775`, `:81062`) return that same preview for Blocked pages, covering held paging and atomic spread/continuous rendition requests, and paint-time post filters (`:9496`) are bypassed for that preview texture; an unrelated JPEG holdover sharing the idx retains its existing filtering. The existing status overlay and edit notice use the same Japanese notice (`src/ui_raw.rs:4`, `src/app/raw_page_store.rs:2604`). No worker, time window or navigation release path was added. Failed returning to Idle during the existing brightness transaction restores the normal gate.

The replacement regression (`src/ui_fullscreen.rs:48575`) covers Unsupported/Failed, failed/available catalog thumbnails (including a warm rendition), colorization/LUT/disabled effects, single/spread/continuous modes, normal and forced-rendition navigation retirement by actual preview presentation, folder-lock readiness, the notice, paint-time filtering with unchanged JPEG holdovers, edit prohibition and Failed-to-Idle gate restoration. Its latest result is recorded below.

## Original S3 files (review baseline)

The uncommitted review diff changes nine files: `src/app.rs`, `src/app/raw_page_store.rs`, `src/raw/executor.rs`, `src/raw/raw_decoder.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_fullscreen.rs`, `docs/async-architecture.md`, `docs/display-pipeline.md`, and this report. The executor addition is a test-only desired-parallelism accessor. No snapshot PNG or settings schema changed. The original S3 inventory below is retained as baseline context.

`docs/async-architecture.md`, `docs/detached-rework-plan.md`, `docs/display-pipeline.md`, `docs/raw-libraw-s3-implementation.md`, `docs/spec.md`, `src/app.rs`, `src/app/collection_navigation.rs`, `src/app/raw_page_store.rs`, `src/app/snapshot_ops.rs`, `src/app/test_script_support.rs`, `src/app/tests.rs`, `src/app/viewer_context_registry.rs`, `src/app/vram_accounting.rs`, `src/cache_maintenance.rs`, `src/canonical_image_loader.rs`, `src/creative_lut.rs`, `src/external_tool.rs`, `src/fs_animation.rs`, `src/fs_page_load_scheduler.rs`, `src/lib.rs`, `src/materializer.rs`, `src/pipeline_debug.rs`, `src/raw/executor.rs`, `src/raw/mod.rs`, `src/raw/raw_decoder.rs`, `src/test_script.rs`, `src/test_script/pointer_input.rs`, `src/tray_integration.rs`, `src/ui_adjustment_panel.rs`, `src/ui_analysis_panel.rs`, `src/ui_conceal.rs`, `src/ui_crop.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs`, `src/ui_dialogs/preferences/search_index.rs`, `src/ui_erase.rs`, `src/ui_fullscreen.rs`, `src/ui_raw.rs`, `src/ui_sns_split.rs`, `src/ui_text.rs`, `tests/snapshots/preferences_parallelism_pdf_count.png`, `tests/snapshots/raw_progress_dark.png`, `tests/snapshots/raw_settings_dark.png`, `tests/snapshots/raw_settings_light.png`, `tests/ui_snapshot.rs`, `tests/snapshots/raw_blocked_preview_notice_dark.png`.

## Implementation map

Review-fix anchors (current tree):

| Finding | Fix and failing regression |
| --- | --- |
| Preview cancellation / re-entry | `src/app/raw_page_store.rs:1962`: shared receiver/ticket/owner cancellation; `src/app.rs:67454`, `:67495` and `src/ui_fullscreen.rs:37762`, `:37784`: both prefetch routes; `src/app.rs:73849`: held-page-turn deferral. Real-path tests: `src/ui_fullscreen.rs:48140`, `:48145`, `src/app/raw_page_store.rs:1154`. |
| Screen filters on embedded preview | `src/ui_fullscreen.rs:9496`: texture identity bypass independent of Blocked. Supported-preview/rendition regression `:48229`; existing K matrix also protects JPEG holdovers. |
| Offscreen completion synchronous correction/upload | `src/app/raw_page_store.rs:2068`: same current-page condition as ordinary image completion. Regression `:1118` covers excluded prefetch and preserved current behavior. |
| Continuous repaint cadence | `src/ui_fullscreen.rs:38027`, `:38067`, `:38142`, `:38251`, `:38265`: exclude RAW development waits from next-frame work and request 100ms if current development is the only pending work. Actual-draw regression `:48271`; predicted frame time is zero in the fixture so the requested interval is measured directly. |
| Missing production connections/backlog coverage | `src/ui_fullscreen.rs:48334`: App state and real single/Z/spread/continuous paint, canonical dimensions and final-cache selection. `src/ui_dialogs/preferences.rs:3536`: actual edit/OK/save/reload/reopen, executor update/error and source invalidation. `src/app/raw_page_store.rs:1211`: actual poll/upload admission in both backlog orders and late preview after Full installation. |
| Unpack label | `src/raw/raw_decoder.rs:7`: shared 35 boundary used by callback mapping; `src/app/raw_page_store.rs:2604`: typed ticket plus interval label. Channel-controlled Running-ticket regression `:1184`. |

The original A–K/6/7 implementation and complete consumer inventory follow with references refreshed for this diff.

The references below describe the current diff, not the historical line numbers in the design plan.

| Rule | Implementation |
| --- | --- |
| 6.1 / 7.10 E | `src/app/raw_page_store.rs:41`: independent installed, preview and development states. Preparing owns cancellation and highest priority; publish rejects superseded tickets. `:270`, `:378`, `:431`: context-owned store, identity validation and the sole RAW cache writer. Info/preview/develop/error all pass the same gate. `src/app/viewer_context_registry.rs:1109`: registered ContextAsyncOwner, polling, parking, generation assignment, bundle construction/swap/drop. |
| 6.2 | `src/fs_animation.rs:92`: RawPreview holds optional pixels/texture plus canonical developed dimensions and load sequence. Development replaces it with ordinary Static; preview allocation occurs only after owner validation. |
| 6.3 / 7.10 A, G | `src/app/raw_page_store.rs:398`: one six-way classification. `src/app.rs:73550`: legacy load-state projection. `src/ui_fullscreen.rs:9193`: shared RAW navigation readiness for typed sequences and folder lock. Ready/Presenting are reclassified; terminal RAW counts as LoadFailed. Fallback requires decoded and orientation-accepted preview through `raw_fullscreen_fallback_allowed`. |
| 6.4 / 7.10 F | `src/app/raw_page_store.rs:1969`: atomic snapshot transfer takes state and cache entry before generation replacement, cancels work, verifies item identity, and restores both. `:2104`: discard removes owner, cache, pending work, dimensions, bbox and backlog. Park cancels active requests while preserving Done/Blocked; generation/drop use context-owned map disposal. |
| 7.1 / 7.10 E | `src/app/raw_page_store.rs:2305`: worker-only source fingerprint/info/preview. `:2436`: cancellable source preparation followed by executor development. D1 permit is released before executor submission/wait; canonical routing, clamp and panorama tee remain shared. Interim FsRawJobState/Full-only path removed. |
| 7.2 / 7.9 / 7.10 C | `src/app/raw_page_store.rs:646`, `:2171`: combined demand: every displayed source High, next two/previous one Normal. Cancellation uses that union; Preparing and Submitted promote. Paged/continuous cache retention and derived keep sets include the union, including distant cover helpers and every visible continuous source. Ordinary Static retention remains the existing keep range; no additional RAW disk/LRU retention. |
| 7.3 / 7.10 K | `src/ui_fullscreen.rs:9372`, `:10630`, `:9618`: final-required color/LUT display accepts faithful rendition after preview validation; owner-derived Blocked pages display the unchanged embedded preview even with a cached rendition. Preview is display-only; edit/final inputs remain Static. Continuous reading uses existing one-page processing admission. |
| 7.4 / 7.10 J | `src/app/raw_page_store.rs:2262`: shared resolved-target edit predicate always allows non-RAW and requires Developed for RAW. All six tool entries, SNS direct entry, spread target switches, in-tool page switches and button availability check it before mutation; existing fullscreen-index predicates remain. |
| 7.5 / 7.10 B | `src/app/raw_page_store.rs:2291`, `src/app.rs:75538`, `src/ui_fullscreen.rs:26636`: canonical dimensions drive RAW layout in every stage, including clamped/derived textures, fit, Original, Z, rotation, spread and continuous layout. Spread pairing/offset also reads owner dimensions. |
| 7.6 / 7.10 G | `src/ui_fullscreen.rs:11109`, `:11810`: typed/legacy navigation and folder lock share classification/readiness. Target producer and backlog-upload admission share the existing fresh typed navigation decision. PreviewAbsent never settles on a warm thumbnail; failure is terminal; actual presentation retires the sequence. LatestSeek retains both current RAW preview and preparation requests, while preserving sibling contexts. |
| 7.7 | Consumer-by-consumer inventory follows. |
| 7.8 / 7.10 I | `src/raw/executor.rs:265`: typed Queued/Running/Cancelling state. `src/app/raw_page_store.rs:2604`: Japanese progress derived from owner/ticket; current development repaints at 100 ms. `:2642`: preview/develop presentation events carry context, request ID and elapsed time. Existing prefetch row renders RAW development. `src/ui_raw.rs:30`: shared settings/progress presentation and snapshot fixtures. |
| 7.10 D, F | `src/app/raw_page_store.rs:2114`, `:2149`: mounted/parked RAW source transaction, independent brightness transitions, development-only cancellation/backlog removal, existing processed holdover capture, page-local invalidation and reserved request ID. Physical Stale discards old identity and reacquires it on a worker. No global retained-AI epoch bump. `src/app.rs:72788`, `:73921`: fingerprint and brightness in retained-AI keys and completion validation; unrelated JPEG completion remains valid. |
| 7.10 F worker validation | `src/raw/raw_decoder.rs:30`: full NTFS timestamp and size fingerprint. `src/canonical_image_loader.rs:396`, `:483`: before/after info and preview validation. Executor validates queued product source before opening and after development, including failures; typed Stale is rejected by request identity when superseded. |
| 7.10 H | `src/materializer.rs:204`, `:624`, `:666`: request brightness snapshot, RAW-specific reuse key, per-request decode context. Both external-tool request producers supply current settings. |
| 7.10 I / 13 | `src/raw/executor.rs:456`: transactional desired parallelism; partial spawn failure retains previous desired, removes unspawned reservations and retires surplus workers normally. `src/ui_dialogs/preferences.rs:2436`: setting committed only after success, Japanese error returned to UI. Parallelism 1..10/default 3 and existing RawBrightness settings use unreleased settings fields, no migration. |

## Consumer inventory (7.7 and 7.10 G)

| Consumer | RAW behavior / reference |
| --- | --- |
| Display and processed texture priority | RawPreview branch, faithful rendition gate when final effects required; Blocked bypasses the gate and always displays embedded preview; `src/ui_fullscreen.rs:9372`, `:10630`. |
| Load state, typed navigation and old readiness branches | Shared six-state classification, RAW Terminal as failure, Ready/Presenting recheck, producer/upload admission; `src/app.rs:73550`, `src/ui_fullscreen.rs:11109`. |
| Folder-move lock | Same RAW readiness predicate; validated preview or faithful rendition releases lock before full development according to the shared color gate; Blocked uses unchanged preview, and Terminal releases the lock; `src/ui_fullscreen.rs:11810`. |
| Pass-through / faithful rendition | Validation gate precedes thumbnail/pixel lookup and cached rendition lookup; Blocked returns the unchanged preview before either lookup, including forced rendition calls; display-only output, no edit input; `src/app.rs:80775`, `:80858`, `:81062`. |
| Original-image hold | Valid embedded preview is permitted as display source; gated fallback; `src/ui_fullscreen.rs:10567`. |
| Loupe / overview navigator | Resolved display source and shared source-coordinate transform; thumbnail fallback is gated; `src/ui_fullscreen.rs:32154`, `:40016`. |
| Automatic margin bbox | RawPreview pixels and load sequence; recalculated after replacement; `src/ui_fullscreen.rs:35740`. |
| Spread dimensions / pairing / cache-presence layout | Canonical owner dimensions, RawPreview dimensions, RAW source-size layout in every stage, including spread offset; `src/ui_fullscreen.rs:16369`, `:40401`. |
| Edit result, erase, local adjustment, conceal, saved masks, synchronous adjustment, final composite, final AI and AI prefetch | Source pixels remain Static-only. RAW preview cannot enter these pipelines. AI prefetch indicators exclude RAW outside its development demand; `src/app.rs:72281`, `:72560`, `:78366`. |
| Export / copy / fullscreen comparison capture | Existing complete-final requirement retained; RAW preview cannot satisfy it; `src/ui_fullscreen.rs:45814`. |
| Grid comparison pin | RAW source is developed High while pending; Terminal reports failure; `src/app.rs:41547`, `src/ui_fullscreen.rs:44075`. |
| Comparison source preparation | Waits for development as well as fs_pending; `src/ui_fullscreen.rs:38449`. |
| Panorama | Static-only detection and canonical dimensions; full development uses existing high-resolution tee and page-local invalidation; `src/app/raw_page_store.rs:1905`, `:2114`. |
| Analysis / histogram | Static-only; preview stage displays Japanese development-wait text; `src/ui_analysis_panel.rs:1096`. |
| Color-search palette | Existing Static-only source unchanged; no palette cached from preview; `src/app/color_filter.rs:120`. |
| Pipeline debug | RawPreview recorded as missing full-source stage; processing/output dumps remain Static-only; `src/pipeline_debug.rs:303`. |
| Alpha, shrink warning, AI toast, hover dimensions | Alpha/full-processing remain Static-only; RawPreview top bar uses canonical dimensions and clamp warning; `src/app.rs:68421`, `src/ui_fullscreen.rs:26688`. |
| Ctrl+E export dialog | Existing canonical full decoder and executor retained (S2 routing); no embedded preview export. |
| Continuous fallback / pending | Thumbnail fallback gated; all visible RAW pages demanded High, pending uses classification, faithful rendition uses existing bounded per-frame admission; `src/ui_fullscreen.rs:37053`, `:37922`. |
| Coordinates / layout | Canonical source size for RAW throughout; thumbnail dimensions cannot be substituted before info; `src/ui_fullscreen.rs:26636`. |
| Paint resource / Lanczos identity and producer binding | RawPreview load sequence participates in identity; owner installs bind texture item ID like Static; `src/fs_animation.rs:217`, `src/app/raw_page_store.rs:2006`. |
| VRAM accounting | Optional preview texture counted in owning context; `src/app/vram_accounting.rs:14`. |
| Automated readiness | Distinct page_ready (validated preview/developed) and edit_ready (developed only); `src/app/test_script_support.rs:344`. |
| Still seek thumbnail strip | Catalog navigation thumbnails remain catalog UI; main-page fallback and overview rendering use the validation gate. |

## Documentation and deviations

Updated display-pipeline RAW stages/layout/color/source changes, async-architecture owner/demand/cancel/Stale, spec settings and detached-rework-plan section 11 context resource record. No detached predicates or viewport paths changed. The design plan remains owned by the lead.

No unapproved design deviation is intended; approved 7.10 J and K are part of the baseline. Implementation detail: an exclusive Resolving record represents the worker-only initial fingerprint discovery, rather than inventing a placeholder source identity or statting on the UI thread. The existing preview/development request IDs remain independent. Invalidated phases reject late completion immediately; the source transaction also reserves the next monotonic request identity.

## Regression coverage

| Requirement | Tests (current source) |
| --- | --- |
| 15 / E: monotonic stages, early Full, identity, cancellation and isolation | `src/app/raw_page_store.rs:781`, `:810`, `:958`, `:1026`, `:1554`; dimensions/result rejection precedes allocation, late preview cannot replace Static, park/cancel/drop reject completion without affecting siblings. |
| A: corrupt/orientation-rejected/no preview with warm half thumbnail | `src/app/raw_page_store.rs:849`, `:1283`; all display/rendition paths reject fallback until decoded preview validation. |
| B / 7.5: clamped layout through preview/develop/final | `raw_canonical_layout_survives_preview_clamped_develop_and_final_in_all_fit_modes` in `src/ui_fullscreen.rs:48906`; fit, Original, Z, pan, rotation and singleton spread are compared at canonical 12000 x 8000 dimensions. |
| C: prefetch 0/1, distant cover, every continuous visible page | `src/app/raw_page_store.rs:1048`, `:1501`; displayed High demand and union retention survive narrow keep settings. |
| D / I: independent brightness transitions, pending upload, preparing and parked | `src/app/raw_page_store.rs:896`, `:945`, `:1463`, `:1794`; every preview axis is preserved, brightness-specific failure returns to Idle, mounted/parked transactions and processed holdovers cover single/spread/continuous color/LUT modes. |
| F: overwrite, superseded Stale, retained cached/running RAW AI and surviving JPEG | `src/app/raw_page_store.rs:994`, `:1362`, `:1392`, `:1535`; worker stat, request identity, atomic snapshot transfer and page-local retained-key validation. |
| G / 15: typed navigation, terminal RAW, actual presentation and folder locks | `src/app/raw_page_store.rs:1611`, `:1717`; absent-preview navigation regression (`src/ui_fullscreen.rs:48817`) and K regression (`:48575`); scheduler LatestSeek regression preserves both RAW request purposes and sibling contexts. |
| H: materializer request brightness and stale cached rendition prevention | `src/materializer.rs:1756` RAW brightness/cache-key fixture checks distinct output and same-key reuse without launching an external application. |
| I: partial spawn failure and typed ticket progress | Fake executor transaction/state tests in `src/raw/executor.rs:969`; raw progress/preferences snapshot tests use shared UI helpers. |
| J / 15: resolved edit target and mutation order | `src/app/raw_page_store.rs:1330`, `:1763`; all six tools, in-tool switches and JPEG anchor with undeveloped RAW spread partner. Script feature regression distinguishes page_ready from edit_ready. |
| K: blocked preview exception and gate restoration | `src/ui_fullscreen.rs:48575`; complete effect/catalog/mode/block-reason matrix, existing navigation presentation retirement, folder lock, unchanged preview, post-filter bypass and edit notices. `tests/ui_snapshot.rs` adds the shared blocked-preview overlay PNG. |

## Review verification

All Cargo commands set `MSBUILDDISABLENODEREUSE=1`. The first reviewed-code bug run failed all five new regressions (0 passed / 5 failed), before the fixes. After adding real-path coverage, a temporary mutation restored the reviewed bugs, omitted layout/settings connections and removed late-preview guards: all ten regressions failed (0 passed / 10 failed). All three mutated source files were restored byte-for-byte before running the suites below. No verification command was interrupted by a timeout.

| Command / filter | Current result |
| --- | --- |
| `cargo test -p mimageviewer --lib raw_review_ -- --test-threads=1` | 10 passed, 0 failed |
| `cargo test -p mimageviewer --lib raw:: -- --test-threads=1` | 63 passed, 0 failed |
| `cargo test -p mimageviewer --lib raw_page_store -- --test-threads=1` | 27 passed, 0 failed |
| `cargo test -p mimageviewer --lib app::tests:: -- --test-threads=1` | 2,071 passed, 0 failed, 2 ignored |
| `cargo test -p mimageviewer --lib ui_fullscreen::tests:: -- --test-threads=1` | 629 passed, 0 failed, 1 ignored |
| `cargo test -p mimageviewer --lib settings:: -- --test-threads=1` | 260 passed, 0 failed, 12 ignored |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences:: -- --test-threads=1` | 70 passed, 0 failed |
| `cargo test -p mimageviewer --lib fs_page_load_scheduler:: -- --test-threads=1` | 11 passed, 0 failed |
| `cargo test -p mimageviewer --lib materializer:: -- --test-threads=1` | 33 passed, 0 failed |
| `cargo test -p mimageviewer --lib passthrough -- --test-threads=1` | 18 passed, 0 failed |
| `cargo test --test ui_snapshot -- --test-threads=1` | 59 passed, 0 failed; no PNG changes |
| `cargo test -p libraw-sys` | 3 passed, 0 failed; 0 doctests |
| `cargo check -p mimageviewer --bin mimageviewer-core` | Exit 0 |
| `cargo check --bin mimageviewer-core --features portable` | Exit 0 |
| `cargo clippy -p mimageviewer --lib --tests` | Exit 0; baseline 1,925 lib-test warnings (1,489 duplicates); the two new fixture warnings were removed |
| `.\scripts\test-full.ps1` | Exit 0 / PASS; **11,098 passed, 0 failed, 57 ignored**, across 59 top-level Cargo suites. Main library: 10,047 passed / 51 ignored; workspace total 11,048 passed, plus vendored egui 25, egui-wgpu 9 and eframe 16. Nested child-harness results are excluded from the totals. |
| `cargo fmt --check`, `git diff --check` | Exit 0 |
| `python scripts/check_ui_glyphs.py` | Exit 0; zero dangerous UI glyphs |

Logs are `target/s3-review-*.log`. RAW progress and dark settings PNGs were visually reviewed again; no expected PNG changed. The preferences test includes real UI control editing and the OK commit path, rather than a direct settings assignment. The layout test repeats source cycles through the existing invalidation transaction; differing preview aspect ratios retain the explicitly permitted contain difference in §7.5. Initial compile/fixture failures (test-only visibility/default executor size, egui maximum texture size and predicted frame time, stale fixture edit textures, and the contain exception) were corrected before the successful suites.

Final full-gate evidence: `target/s3-review-full-summary.log` records the 59 suite headers/results and reconciled totals; `target/s3-review-full-native.log` records captured native output; `target/s3-review-full-transcript.log` records the PowerShell invocation. The required release core/remote/epub-pdf executables already existed. No build-dev or product launch occurred. After removing the two test-only style warnings, the focused ten regressions and clippy were rerun successfully; the full gate then covered the final source tree.

## Original S3 verification (ef0b2dac5)

Results below are post-K reruns; the final texture-specific post-filter guard was followed by another complete fullscreen suite. The full gate validates the integrated final tree. Counts are not summed across targeted filters because they overlap. All Cargo commands used `MSBUILDDISABLENODEREUSE=1`; builds were allowed to finish without runner timeouts.

| Command | Actual result |
| --- | --- |
| `cargo test -p mimageviewer --lib raw::` | 63 passed, 0 failed, 0 ignored |
| `cargo test -p mimageviewer --lib raw_page_store -- --test-threads=1` | 23 passed, 0 failed, 0 ignored |
| `cargo test -p mimageviewer --lib raw_canonical_layout -- --test-threads=1` | 1 passed, 0 failed |
| `cargo test -p mimageviewer --lib fs_page_load_scheduler:: -- --test-threads=1` | 11 passed, 0 failed |
| `cargo test -p mimageviewer --lib materializer:: -- --test-threads=1` | 33 passed, 0 failed |
| `cargo test -p mimageviewer --lib passthrough -- --test-threads=1` | 18 passed, 0 failed (final shared getter correction) |
| `cargo test -p mimageviewer --lib app::tests:: -- --test-threads=1` | 2,071 passed, 0 failed, 2 ignored |
| `cargo test -p mimageviewer --lib ui_fullscreen::tests:: -- --test-threads=1` | 624 passed, 0 failed, 1 ignored (final texture-specific guard included) |
| `cargo test -p mimageviewer --lib settings:: -- --test-threads=1` | 260 passed, 0 failed, 12 ignored |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences:: -- --test-threads=1` | 69 passed, 0 failed |
| `cargo test --test ui_snapshot -- --test-threads=1` | 59 passed, 0 failed |
| `cargo test -p libraw-sys` | 3 passed, 0 failed; 0 doctests |
| `cargo check -p mimageviewer --bin mimageviewer-core` | Exit 0 |
| `cargo check --bin mimageviewer-core --features portable` | Exit 0 |
| `cargo check --bin mimageviewer-core --features test-script` | Exit 0 |
| `cargo test -p mimageviewer --lib --features test-script preview_readiness_and_developed_edit_readiness_are_distinct_in_script_snapshot` | 1 passed, 0 failed |
| `cargo clippy -p mimageviewer -p libraw-sys --lib --tests --bin mimageviewer-core` | Exit 0, warnings |
| `python scripts/check_ui_glyphs.py` | Exit 0; zero dangerous UI glyphs |
| `cargo fmt --all` / `cargo fmt --all --check` | Exit 0 |
| `git diff --check` | Exit 0 |
| `cargo test -p mimageviewer --lib raw_blocked_preview_settles -- --test-threads=1` | 1 passed, 0 failed; also passed in the final fullscreen suite with the JPEG holdover guard and forced-rendition path |
| `.\scripts\test-full.ps1` | **Final tree: exit 0; 11,088 passed / 0 failed / 57 ignored**, including workspace tests, integration tests, doctests and all three vendor gates. Main library: 10,037 passed / 0 failed / 51 ignored. Vendor egui / egui-wgpu / eframe: 25 / 9 / 16 passed, zero failures. The first pre-correction run also passed with the same counts. All three prerequisite release executables existed; rebuilding them was unnecessary. |
| `.\scripts\build-dev.ps1` | **Not run**, per the explicit design-lead instruction; the lead builds after independent review. |

Snapshot updates: one full preferences-page snapshot and three RAW progress/settings snapshots generated successfully, then all four PNGs visually reviewed. The K blocked-preview notice PNG also passed generation and visual review using the actual fullscreen status painter. Japanese text, dark/light selection states and the complete RAW/PDF settings layout are readable without clipping. The post-K unmodified snapshot comparison passed all 59 tests.

Earlier iterations included corrected compile errors and fixture failures: owner LUT fixture (19 passed/1 failed), materializer process ownership (32/1), navigation terminal draw transition (0/1), and the search-index shared-renderer assumption (68/1). Each was corrected and rerun successfully. A Windows test-executable linker lock caused one build retry after the concurrently running RAW fixture suite finished; it was not a product-test failure. The original blocked-preview reproduction failed 0/1 as recorded in the WIP; it now passes under approved K. A later consumer audit found direct held-paging/spread/continuous rendition requests outside the texture resolvers; their shared getter was corrected and the regression extended to both navigation policies. The first K compile failed on private snapshot helpers; a test-only fixture now exercises the existing atomic transfer without widening product visibility. Logs are under `target/s3-final-*.log`; earlier iteration evidence remains under `target/s3-*.log`. Final gate: `target/s3-full-final-summary.log` records all 59 top-level Cargo suite results (excluding nested child-harness duplicates); `target/s3-full-final-native.log` contains captured native output, and `target/s3-full-final.log` is the PowerShell transcript. First-run summaries remain in `target/s3-full-before-rendition-fix.log`. Targeted Cargo commands all exited 0; no fixture was skipped because a RAW sample was missing.


## Open work

Independent re-review of this uncommitted diff and real-device verification remain pending. All requested automated gates passed; there is no known unresolved review finding or required new design decision. The design lead owns build-dev after review, per the explicit handoff instruction. The review fixes follow the requested directions, with the equivalent held-page-turn cancellation route included in the ownership audit. The forced-rendition correction in the baseline implements existing K and adds no navigation exception.

## Real-device verification

No product binary was launched. These checks remain pending after independent review and the design lead's verification build. Verify a large RAW folder with warm half-developed thumbnails, missing/corrupt/orientation-rejected previews, rapid paging with parallelism 1 and 3, distant cover/spread partners and multiple continuous pages, Original/Z/pan/rotation across preview/develop/final, colorization/LUT plus brightness changes, edit availability, physical overwrite during development, and independent viewer contexts. Remote and binding remain manual regression scenarios from the plan.
