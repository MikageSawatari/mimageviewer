# mImageViewer local Tantivy patch

Baseline: crates.io `tantivy` 0.26.1, upstream commit
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

The root Cargo patch fixes the package to this local 0.26.1 source; it does
not update Tantivy's dependencies. Focused on-disk and shutdown regressions
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
