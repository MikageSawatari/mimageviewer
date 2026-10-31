# mImageViewer: temporary mimalloc v3 update (§1.666)

## Sources and scope

- Rust wrapper: crates.io `libmimalloc-sys` **0.1.49**, copied from the registry
  package supplied in `target/mimalloc-src/libmimalloc-sys-0.1.49/`.
- Bundled v3 implementation: **microsoft/mimalloc v3.5.3**.
- Upstream archive: <https://github.com/microsoft/mimalloc/archive/refs/tags/v3.5.3.tar.gz>
- Archive SHA-256:
  `3b4a15153a59905995f7070296ed604bb5ccc00cabb8b93446931aff77224d47`.
- `c_src/mimalloc/v3/` contains only upstream `include/`, `src/`, and `LICENSE`.
  These files are byte-identical to the archive; there are **no local C changes**.
  `build.rs` builds `src/static.c`, whose includes are all within these trees.
  Upstream documentation, tests, binaries, IDE and CMake files are not needed.
- `c_src/mimalloc/v2/` is retained unchanged from the crate to preserve its
  feature structure. mImageViewer does not enable `v2`.
- Registry installation marker `.cargo-ok` and the crate's standalone
  `Cargo.lock` are omitted. The root `Cargo.lock` owns dependency resolution.
  The original manifests, crate license, and `.cargo_vcs_info.json` are retained.

## Reason

The design lead's §1.666 investigation (recorded on master in
`docs/next-release-backlog.md`, absent from this v4.10.1-based worktree) matched
the reported v4.9.0/v4.10.0 crash instructions and PDBs to mimalloc 3.3.2:
`mi_heap_main` → `_mi_subproc` dereferenced `theap->tld->subproc` with null `tld`
during allocation after thread cleanup. This matches the upstream reports:

- [polars #29883](https://github.com/pola-rs/polars/issues/29883)
- [mimalloc #1287](https://github.com/microsoft/mimalloc/issues/1287)
- [mimalloc PR #1289](https://github.com/microsoft/mimalloc/pull/1289): null
  `theap->tld` check, included from v3.4.1.
- [mimalloc PR #1391](https://github.com/microsoft/mimalloc/pull/1391): also
  fall back when `tld->subproc` has not been set.

Both checks are present in v3.5.3 `src/subproc.c::_mi_subproc`. It returns
`_mi_subproc_main()` when `theap`, `theap->tld`, or `theap->tld->subproc` is null.
The shipped Rust wrapper still bundles 3.3.2, so the root `[patch.crates-io]`
selects this local crate. No allocator switch, new feature, `MIMALLOC_*` setting,
or application recovery mechanism is introduced.

## Wrapper compatibility changes

`build.rs` is unchanged, including MSVC C++17 compilation, defines, flags and
Windows libraries. The basic allocation declarations in `src/lib.rs` are
unchanged. Comparing the 3.3.2 and 3.5.3 public headers requires two adjustments
in the optional `extended` bindings:

- v3 `mi_cfree` now returns `bool`; v2 retains its original void declaration.
- v3 `_mi_option_last` is 48 after `mi_option_collect_merges_stats` was appended;
  the v2 value remains 38. No options are enabled or set by this patch.

New upstream API functions are not added to the wrapper. Existing opaque types
and the other declared function signatures are unchanged by this update.

## Verification and limits

The mImageViewer lib test `allocator_tests::linked_mimalloc_is_v3_5_3` calls the
public C `mi_version()` in the same linked static library as the global allocator
and requires **30503**. It runs in the ordinary full-library test gate, without
enabling `extended` in the product. The extended wrapper tests also cover the
changed checked-free return type.

A deterministic public-API test of the **null-tld branch itself** is not
available: `mi_theap_t`/thread-local state and `_mi_subproc` are internal;
the public thread cleanup API does not expose or let a caller retain/set the
internal default-theap/tld state at the failing teardown instant. Ordinary
thread exit/reallocation exercises lifecycle behavior but cannot prove that
particular branch was reached. Do not manufacture stale internal pointers,
export private functions, or modify upstream C to force it. Source/archive
identity plus linked-version evidence establishes inclusion of the upstream
fix, not reproduction or end-user confirmation of the intermittent crash.

## Removal condition

**When an official Rust wrapper release bundles mimalloc v3.4.1 or later,
remove this vendor patch and return to the official crate.** Check the bundled
C version and upstream fixes (including PR #1391) when selecting that release;
do not infer them from the Rust crate version alone. Update the wrapper and
lockfile, remove the root patch/workspace exclusion, this directory and its
`.gitattributes` rule, and adjust the linked-version test to the official bundled
version. Repeat the allocator-wide verification. The dependency-update report
discovers the crate from the root manifest and links this maintenance record.
