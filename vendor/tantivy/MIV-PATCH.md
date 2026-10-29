# mImageViewer local Tantivy patch

Baseline: crates.io `tantivy` 0.27.0, upstream commit
`67889269cae67063c372bc236b102d632a021568` (the packaged
`.cargo_vcs_info.json`). The original crate archive SHA-256 is
`ad1ebacf6bc124b741ffbfb088f9835cb55e1e2ff57e182e9219e3efa1377b97`.
This directory is a real copy, not a junction/symlink; the registry copy is
never modified. Upstream's MIT license, AUTHORS and package files are retained.

Historical baseline: crates.io `tantivy` 0.26.1, upstream commit
`d8f4c0b703120ed98f06297724dc1522df6019b9` (the packaged
`.cargo_vcs_info.json`). The original crate archive SHA-256 is
`edde6a10743fff00a4e1a8c9ef020bf5f3cbad301b7d2d39f2b07f123c4eac07`,
matching the pre-patch Cargo.lock checksum. This directory is a real copy of
that package, not a junction/symlink; the registry copy is never modified.
Upstream's MIT license is retained in [LICENSE](LICENSE), with AUTHORS and
the original package/source files.

2026-10-15: upstream 0.26.1 -> 0.26.2 (tag commit `72d1ef9a6468aa68bbc69dcc80cdf60aaf64364d`) was applied on top of
this copy with the GitHub compare diff (aggregation count/flush fixes, buffered union
`seek_danger` override removal + `src/query/seek_danger_tests.rs`). It touches none of the
files changed by the local patch below. Only the version lines of Cargo.toml /
Cargo.toml.orig were edited by hand. The 0.26.1 baseline text below is kept as history.

Local changes, designed in `docs/fts-integrity-plan.md` r3 §§5/6.1:

- `src/directory/mmap_directory/mod.rs`: retry only a completed TempPath's
  persist on Windows raw errors 5/32/33; at most four attempts, separated by
  10/25/50 ms. Temp create/write/flush/sync run once. No target removal,
  truncation, whole-commit retry or non-Windows retry. Failure diagnostics
  include stage, destination, raw error and attempt; recovered persist has
  one summary record.
- `src/directory/atomic_write_diagnostics.rs` / `directory/mod.rs`: scoped
  diagnostic origin for merge-worker and end-merge task I/O, without using
  diagnostic state to decide retries or reporting.
- `src/indexer/segment_updater.rs` / `index_writer.rs`: per-writer terminal
  merge failure observer outside task/Directory/segment locks, independent
  of Future consumers. A private typed scheduler admission refusal excludes
  normal shutdown of an unexecuted end-merge task. Executed errors during
  shutdown still report. GC deletion refusals and synchronous commits are
  outside this observer. The observer follows the writer across its existing
  rollback replacement of the SegmentUpdater; rollback behavior is otherwise
  unchanged.
- `src/indexer/segment_manager.rs` / `segment_updater.rs`: keep the exact
  end-merge source-set rejection as a private typed outcome. Ordinary upsert
  commits can remove fully deleted sources during a merge; that obsolete
  result changes neither registers nor committed metas. Keep its diagnostics,
  skip the failure observer, then convert it to the original public Future's
  InvalidArgument. Other InvalidArgument and all I/O failures still report;
  no error-message matching, public error variant, retry or merge wait is added.
- `Cargo.toml`: opt-in `miv-test-hooks` provides per-directory atomic-stage
  injection and per-writer before-end/terminal observations plus an executed
  end-merge task error injection. The application
  enables it only as a dev dependency; runtime builds exclude those hooks.

The root Cargo patch fixes the package to this local source. The original
0.26.1 patch did not update Tantivy's dependencies; the 0.27.0 port below does.
Focused on-disk and shutdown regressions
are in `tests/fts_tantivy_integrity.rs`. When updating upstream, compare these
listed source files against the original crate and port/review each local
change. The bounded persist retry is local mitigation, not an upstream fix
or proof of the original incident's cause.

Verification on Windows, 2026-10-11: `cargo test -p mimageviewer --test
fts_tantivy_integrity` passed all 11 tests (Cargo exit 0). The local log is
`target/I-1367-tantivy-tests.log`; a baseline-relative source patch is recorded
in `target/I-1367-tantivy-local.patch`. `cargo tree -p mimageviewer -e
features,normal,build -i tantivy --locked` confirms runtime dependency features
exclude `miv-test-hooks` (recorded in `target/I-1367-tantivy-normal-features.txt`).
The GC regression holds an old mmap searcher plus a deliberate no-delete-sharing
read handle, since this Windows/Rust version can otherwise delete mapped files;
it verifies actual GC deletion refusal does not reserve a rebuild or notify.

2026-10-12 source-set rejection correction: the same integration target passed
all 14 tests (`target/fts-merge-tantivy-tests.log`). New regressions reproduce
real automatic end-merge rejection during 20 upsert commits, with complete and
partial source removal, verify exact live/reopened contents and unchanged
scan/rebuild/notification state, preserve the explicit Future error, and prove
that an identical-text InvalidArgument from another origin still reports.

2026-10-24: `src/query/intersection.rs`: keep the upstream `Intersection::score`
expression, routing only the right-hand score through `score_out_of_line` with
`#[inline(never)]`. Rust 1.95+ combined the two BM25 bodies into SIMD whose
stale upper lanes cause denormal assists and slow two-term AND scoring. An
explicit loop improved the bench bin but still left the problematic SIMD in the
product exe. With the helper, ClaudeCode's disassembly of the Rust 1.99 product
release build (`mimageviewer-core`) shows scalar code only. `#[inline(never)]`
is a hint, not a guarantee, so keep it and recheck the product function after
toolchain or Tantivy updates. The original sum and addition/call order are
unchanged.
Tests cover two- and four-term Must AND scores, negative zero, call order and
rounding. Upstream 0.27 and main still use the original expression. The full investigation (measurements and rejected
options) is backlog §1.424 in `docs/next-release-backlog.md` at commit
`94017f82e`; the section was removed after v4.8.1 shipped. Rerun the bench regression check and the
product disassembly check (`docs/release-checklist.md` §9.5) when updating the
Rust toolchain or Tantivy.

2026-10-28: ported to upstream 0.27.0 using a three-way merge (base: original
0.26.2 package, ours: local 0.26.2, theirs: original 0.27.0). The original
0.26.2 archive SHA-256 is
`861facfabd71044968f364837f9a083b56464ba5a59079f88706ee5c451ca069`.
All unmodified source files come from 0.27.0, including its manifest
dependencies and VCS metadata; seven obsolete upstream files were removed.
The standalone Cargo.lock is intentionally untracked and removed after tests.
`miv-test-hooks` and the three local-only files are retained.

- `IndexWriter::new` now owns `Index`: rollback passes `self.index.clone()`
  and still inherits the writer's failure observer onto the new updater.
- The end-merge conflict retains `try_schedule_task`, diagnostic scope and
  executed-task injection, while adopting upstream's `debug!` log level.
  Typed scheduler refusal and source-set rejection retain their original
  ownership boundaries. Upstream plugin-aware merge/GC changes are retained.
- The other patched sources merge without semantic conflicts. The local
  recording-scorer test constructor follows the upstream field rename from
  `num_docs` to `segment_num_docs`.
- The right-hand `score_out_of_line` barrier remains in `Intersection::score`.
  The new `block_wand_intersection` is selected for a pure frequency-reading
  term intersection at the top-level pruning entry (including `bench_search`).
  mIV's nested bigram/field/token/filter queries also need the boxed scorer
  path and therefore still use `Intersection::score`. The new block path
  can also be reached when nested children unwrap to individual TermScorers
  (e.g. one unique bigram per token, one source, no extra Basic filters).
  A pure-term benchmark alone therefore cannot verify the old product path.
  It computes leader BM25 scores into a candidate buffer before separately
  seeking/scoring secondary terms; it does not contain the old adjacent
  left/right BM25 expression. No additional barrier has been added. This is
  source analysis; product disassembly confirmation is recorded below.
- Static format review found no incompatibility: top-level format 7/minimum 4,
  columnar V2, sstable writer v3 with v2/v3 readers, docstore V2 and postings
  serialization are retained. Existing upstream v6/v7 fixtures are unchanged;
  v7 was generated by 0.23.0, not 0.26.2. A dedicated registry-0.26.2 fixture
  was subsequently added at `tests/fixtures/fts_tantivy_0262` with its generator
  and provenance. `tests/fts_tantivy_compat.rs` copies it to a temporary directory
  and verifies the normal application open/search/update/delete/mixed-generation
  merge/reopen paths. The fixture regression and all 14 integrity tests passed.

Initial port checkpoint: the normal core `cargo check --locked
--offline` passed, but standalone vendor nextest metadata resolution requires
uncached `frostem` 1.20260804.0. Per the offline handoff instruction, work
stopped without fetching packages or changing features. In-flight integration
test and symbol-core build were interrupted; they are not successful evidence.
See `docs/dependency-update-20261028.md` stage C for commands and remaining gates.

The offline cache was subsequently supplied by ClaudeCode; standalone vendor
resolution selects compatible `frostem` 1.20260821.7. With `miv-test-hooks`,
31 focused scoring/mmap/format tests and 59 index-writer/updater tests passed.
The normal/build feature tree excludes `miv-test-hooks`. The standalone vendor
lockfile is not committed and is removed after verification. Final benchmark,
disassembly and broad-gate results are recorded in the stage C update log.

2026-10-28 product-code verification: Rust 1.99.1 release core, incremental off,
strip=false, debug=line-tables-only, normal product features. Dumpbin confirms
both `Intersection::score` instantiations, both `score_out_of_line` helpers and
`block_wand_intersection` contain no `unpcklps` / `addps` / `divps` instructions.
The TermScorer paths use scalar BM25 arithmetic; the old helper call remains.
The new block function is also called by the product's pruning path. Keep the
existing barrier; no additional barrier is required for this generated code.
Evidence: `target/dependency-update-20261028/stage-c/dumpbin-scoring.json` and
`scoring-function-{1..5}.txt`; core SHA-256
`4df2aa47adc5858150e0a324c869061eaedb3fe582a04da5b733eaba58635517`.
Benchmark and broad test results belong in the dependency update record.

A separate generated-code review agrees with the decision above. This does not
claim that all callees are scalar: `BlockSegmentPostings::block_max_score`
contains intentional packed arithmetic (4 `addps`, 2 `divps`). Its eight-document
loop initializes every lane from complete term-frequency loads, conversion and
fieldnorm/weight broadcasts before division. It does not reproduce the stale
upper-lane mechanism; no barrier is added there. Performance acceptance is
separately checked by the benchmark, not inferred from instruction counts.

2026-10-28 benchmark acceptance is pending: `generic_jp` (dense two-bigram AND)
measured 38.5156 / 38.2961 / 38.5754 ms against the unchanged 25.0732 ms baseline.
The best result fails the regression gate (+52.7%, +13.2229 ms). No baseline save
or performance fallback is included. The bench binary's five corresponding
scoring functions were also disassembled: the three problematic instructions
are absent there too, but the runtime regression's root cause remains unproven.
See stage C for full test-gate failures, isolated rechecks and the design-owner
acceptance decision; passing focused tests does not make this update accepted.

2026-10-28 design-owner decision: adopt 0.27.0 with one additional local patch
in `src/query/boolean_query/boolean_weight.rs`. Only the `TermIntersection`
arm of `for_each_pruning` now calls `into_box_scorer` (including 0.27's
`num_docs` argument), then `for_each_pruning_scorer`. This restores 0.26's
ordinary `Intersection` scoring/pruning route while retaining 0.27's scorer
construction, union optimization, and the existing `score_out_of_line` barrier.
The new `block_wand_intersection.rs` implementation and all upstream tests are
retained without modification.

Reason: the initial dense two-bigram `generic_jp` benchmark took
38.5156 / 38.2961 / 38.5754 ms versus the unchanged 25.0732 ms baseline;
post-filtering did not regress and the other nine queries stayed within the
gate. Other worktrees were building at that time, so alternating unpatched /
patched measurements are required to separate load from the routing change.
The results and generated-code verification are recorded in the stage C
dependency update log. Do not overwrite the baseline to accept this patch.
Remove this local routing change when the upstream route is as fast as or
faster than 0.26 for mIV's dense bigram AND queries.

`test_miv_dense_term_intersection_pruning_matches_scorer` checks two- and
four-term dense intersections across posting-block boundaries: all document
IDs and score bits match the ordinary scorer, and TopDocs at limits 1, 10 and
all matches preserves exact scores and tie-breaking order.

Generated-code verification after this routing change: the release product core
SHA-256 is `dc5ccb2ec046359e35c59d160eb6da9af55a6f27a4790a7f34a3e24d76ee2649`.
Both `Intersection::score` instantiations and both `score_out_of_line` helpers
still contain zero `unpcklps` / `addps` / `divps`; `block_wand_intersection` is
absent from the product. The pruning entry calls `into_box_scorer`, which calls
`intersect_scorers`; the returned vtables' score slots point at the verified
Intersection implementations. Both implementations still call their helpers.
Identical-code folding aliases those helpers with existing score functions;
the shared addresses, rather than a single displayed alias, identify the calls.
MSVC's PDB reader rejected this PDB, so LLVM's reader (matching the PE's
GUID/age) supplied exact function addresses and sizes for the raw dumpbin
disassembly. No product rebuild or code change was used to obtain symbols.
See `stage-c-and/dumpbin-scoring.json` in the dependency update's evidence
directory for the PDB provenance, function ranges, vtable pointers and calls.

The required alternating benchmark did not meet acceptance. Against the unchanged
25.0732 ms baseline, unpatched `generic_jp` took 46.9803 / 60.3189 / 55.1229 ms;
patched runs interleaved between them took 54.1618 / 54.7336 / 58.2245 ms.
Every individual comparison and both best-of-three comparisons failed. The
patched best result is +116.0% (+29.0886 ms) versus baseline. `en_common` and
`medium_jp` also exceed the gate in both groups. One paired run improved and two
worsened, with substantial variation, so this does not establish a performance
benefit or isolate routing from load. The patch remains a design-owner-requested
candidate pending a decision on its disposition and further measurement
conditions. No further performance patch, baseline save, or extra benchmark run
was made. Full six-run results are in the stage C follow-up log.

2026-10-29 diagnosis and prototype (design-owner acceptance is pending):

- `src/collector/sort_key/sort_by_score.rs`: restore only
  `SortBySimilarityScore::collect_segment_top_k` to the 0.26 `TopNComputer`
  buffer/median algorithm. 0.27 had changed this collector to `TopNHeap` for an
  exact K-th-score pruning threshold. The benchmark's paginated score search raises
  K from 500 to 10000; maintaining a heap on accepted hits is expensive there.
  The ordinary Intersection pruning loop still scores every matching document,
  so tighter thresholds do not remove that BM25 work. Changing the Weight route
  alone left this collector regression in place.
- This is one selection algorithm for all score paging, not a workload guard
  or failure fallback. The existing heap implementation and upstream tests are
  retained as a reference. Buffered selection needs capacity 2K instead of K,
  and its threshold is a conservative, less frequently updated lower bound.
  Small-K or sparse Block-WAND searches may lose the tighter-threshold speed
  advantage. Results, ascending DocAddress tie-breaking and deletion filtering
  retain the 0.26 behavior; no correctness fix is removed.
  K is offset plus page size. Product post-filtering can push the offset beyond
  10000 before reaching 10000 valid hits, so 10000 is not a general capacity cap.
  The 2K capacity restores the released 0.26 memory profile, rather than adding
  capacity relative to that release.
- Keep the existing BooleanWeight arm for a separate, demonstrated reason:
  score compatibility. Intersection uses `(a+b)+sum(rest)` whereas 0.27 block
  AND accumulates left to right. On one shared 0.26-generated index, removing
  the arm changed 4732 returned score bit patterns across five queries (the
  document order happened to match in that corpus). With the arm and buffered
  collector, all 78388 ordinary-score records and 30762 paged records across
  ten queries exactly match 0.26, including score bits and document addresses.
  Its comment and regression name now describe grouped-score compatibility,
  rather than claiming a standalone performance benefit.
- `test_miv_buffered_score_paging_with_deletes` covers 50023 documents in two
  segments, repeated 2K selection boundaries, ties, deletion of the top 64
  documents, limits 1/10/500/10000 and offset 9500. It compares the real score
  collector with exhaustive scoring, including exact bits and addresses.
  `test_miv_term_intersection_preserves_grouped_scores` retains two-/four-term
  score-order coverage. Upstream heap, block-AND and pagination tests remain.
- Same-index alternating measurements (external rustc=1, cl/core=0) had best
  generic_jp 26.1909 ms for 0.26, 33.3185 ms for 0.27 heap, and 26.2465 ms for
  0.27 buffer (+0.21% versus 0.26). Normal benchmark and final test evidence is
  recorded in the 2026-10-29 stage C follow-up, not inferred from this auxiliary
  measurement. The 2026-04 baseline is unchanged.
- The preceding normal benchmark alternated 0.26 / 0.27 heap / buffer three times.
  Best generic_jp was 34.7889 / 58.3459 / 42.1025 ms: buffer improves on heap
  by 27.84%, but remains 21.02% slower than 0.26. All target process counts
  were zero; host CPU sampled for 0.25 seconds before each run was 58.1–98.8%,
  so this is not a quiet-host acceptance result. A preceding cohort also left
  a 23.16% gap. The residual cause and full recovery remain unverified; do not
  attribute all of it to load or index layout. Full conditions and all-query
  results are in the dependency update log.
- No change is retained in postings/codec, k-ary search, allocation-free seek,
  dense count, schema or index format. The pre-existing right-hand
  `score_out_of_line` barrier remains unchanged.

Remove the collector patch when the upstream score collector matches or beats
0.26 on mIV's high-K dense paging while preserving its scores and ordering,
using matching compiler, profile and features for the comparison.
Removing the BooleanWeight arm additionally requires the block route to retain
0.26's grouped addition and tie behavior. The previous speed-only removal
condition is superseded by these verified compatibility requirements.

Follow-up on index generation (2026-10-29): upstream LogMergePolicy is byte
identical in 0.26.2 and 0.27.0. Worker limits, memory budgeting, flush conditions,
docstore LZ4/block defaults and the ordinary postings block format are unchanged.
The normal benchmark drops its writer after commit without waiting for merges;
the product keeps a writer across commits. Multiworker segment/doc-id assignment
and merge completion therefore need to be separated from reader performance.
Plugin serialization order changed, but it has not been established as the cause
of the residual. No extra writer/merge/codec patch is added on that hypothesis.
See the index-layout and CPU-gated measurement follow-up in the dependency log.
The BooleanWeight arm remains specifically for grouped-score compatibility.

The supplied 0.26 benchmark was subsequently found to use a different Cargo
profile: debug=0, strip=true, incremental=1. The 0.27 symbol builds use
line-tables-only, strip=false, incremental=0. The shared-index diagnostic
binaries already used matching profiles, so the earlier normal/shared-index
comparison changed both the input and the 0.26 build profile. Its residual
cannot be assigned to a writer change. A root 0.26 benchmark was rebuilt with
the exact 0.27 profile, without changing the old worktree; the latest dependency
log records the matched-profile comparison. No extra writer patch is added.

With the rebuilt 0.26 reference, the nine-run alternating comparison met the
requested pre-run CPU limit (one-second means 11.6–14.4%, rustc/cl/core all zero).
Best generic_jp was 23.7365 / 39.1629 / 24.9440 ms for 0.26 / heap / buffer.
Buffer is 36.31% faster than heap and still 5.09% slower than matched 0.26.
The exact contribution of debug/strip/incremental and the remaining difference
is not separated; neither full recovery nor a writer regression is established.
Medium AND and unique-ID queries remain +0.1800 and +0.1242 ms, respectively.
All three buffer runs pass the unchanged April baseline gate. Adoption remains
with the design owner; no writer/codec setting or baseline is changed.
