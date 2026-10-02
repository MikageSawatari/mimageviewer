# RAW via LibRaw S4 results

Initial S4 base: `85ccf0721`, branch `raw-libraw` (checkpoint `392f31f5a`);
no product executable was launched. Release/version/changelog files and the
lead-owned `raw-libraw-plan.md` were not changed.

## Implementation and premise corrections

- New root notices: `LIBRAW-LICENSE.txt` (official CDDL + COPYRIGHT + DCB/FBDD
  and X3F BSD headers), `ZLIB-LICENSE.txt` (zlib 1.3.1), and
  `LIBJPEG-TURBO-LICENSE.txt` (IJG attribution + upstream LICENSE.md/README.ijg).
  All upstream sections were compared byte-for-byte; `.gitattributes` prevents
  checkout conversion. Both installer and portable lists ship all three files.
  Both Japanese readmes retain UTF-8 BOM and CRLF.
- Libjpeg-turbo was mentioned in readmes but its full notice was not shipped;
  S4 adds it. There are no new DLL/exe bundle entries.
- `setup-libraw.sh` already generates VERSION and supports `check`;
  `bootstrap-vendor.sh` already fetches LibRaw/zlib. Neither needed editing.
  CLAUDE's bootstrap list was stale and is updated. Setup Bash is agent-run;
  the corresponding-source procedure supplies PowerShell user commands.
- Main `build.rs` emits `MIV_LIBRAW_BUILD_ID` from VERSION with rerun tracking
  and recovery errors. The native crate's earlier VERSION check also gains
  recovery text. About uses the baked version for the notice and source URL;
  individual zlib and libjpeg-turbo rows were added. A shared notice renderer
  has new light/dark headless snapshots.
- The official tag archive already downloaded by setup matches
  `627928088300ecde6ca91ffd202e189203f04ad61ad12f0fe9dc57b9a7a0fb3c`.
  Its unchanged bytes are staged at `htdocs/mimageviewer/libraw-0.22.2-source.tar.gz`.
  Only `.sha256` is added to repository source; the tarball is ignored, following
  the current FFmpeg policy (older tracked FFmpeg archives remain untouched).
  `libraw-source-distribution.md` explains fetch/verify/stage/publish/retain.
  Publishing and verifying the public URL remain the release lead's work.
- Spec already documents the S3 settings; S4 adds them beside the corrected
  format support section. Architecture already routes RAW to LibRaw but its
  extension count was still 17; corrected to 23. CLAUDE's tech stack, formats,
  source/notice rules and Phase 2 checklist and the docs index are updated.

## Every public text change

| Page/file | Change |
| --- | --- |
| `index.html` | Built-in RAW development, preview-to-developed display, format restrictions link, corrected Windows-format list, JSON-LD format feature, third-party LibRaw/zlib/JPEG notices and source link. |
| `manual/formats.html` | All 23 extensions, preview/developed display and thumbnails, unsupported HE/HE* and JPEG XL DNG, preview-only editing limits, original RAW books, Remote development, settings link; RAW removed from Store codec guidance. |
| `manual/settings.html` | File Processing > RAW development, concurrency 1-10/default 3 and immediate effect, both brightness choices/default, redevelopment and unchanged thumbnails. |
| `manual/getting-started.html` | RAW needs no additional install; Windows codec guidance applies to HEIC/AVIF/JPEG XL. |
| `manual/troubleshooting.html` | RAW support/preview limits; separate Store guidance for HEIC/AVIF/JPEG XL. |
| `migrate-honeyview.html`, `migrate-photos.html`, `migrate-massigra.html` | RAW built-in; Store guidance limited to Windows-decoded formats. |
| `migrate-leeyes.html`, `migrate-mangameeya.html` | Comparison-table RAW support is built-in rather than environment-dependent. |
| Installer and portable readmes | RAW built-in rather than WIC; three full notice filenames, source URL and IJG attribution; portable file inventory. |
| About dialog | LibRaw CDDL/version/source notice plus individual zlib/libjpeg-turbo rows. |

Remote pages had no stale RAW claims. Product assurances and privacy's network/
storage sections were compared: RAW adds neither network activity nor a new
storage location, so no privacy text change is needed. Whole-tree HTML checks
find no Microsoft Raw Image Extension claim or RAW/WIC line; added public text
contains no specific downloader/posting-site name or new implementation jargon.

## Initial S4 verification (before independent-review corrections)

All Cargo commands use `MSBUILDDISABLENODEREUSE=1`; logs are in
`target/s4-verification/` (ignored). Automated verification and build passed.

- `cargo check -p mimageviewer --bin mimageviewer-core`: passed, including the
  final tree (38.77 s). `cargo check --bin mimageviewer-core --features portable`:
  passed on the final tree (14.84 s). Existing warnings remain (138 lib warnings).
- `cargo test -p mimageviewer --lib ui_dialogs::about::tests`: 1 passed.
- New notice snapshots: 2 passed; `cargo test --test ui_snapshot`: 61 passed.
  Both new PNGs visually reviewed: legible, complete, unclipped.
- Glyph lint: zero dangerous glyphs. PowerShell 5.1 parser: no errors in the
  edited portable script. `cargo fmt --all --check` and `git diff --check`: clean.
- `scripts/build-dev.ps1 -PreserveRuntime`: passed; core built in 2m 15s,
  Remote and EPUB converter also built. Its PE gate passed (`runtime=4 pe=3`).
  The explicit fresh-core `check-vcrt-pe-dependencies.ps1 -InputPaths
  target\dev-runtime\mimageviewer-core.exe` also passed (`runtime=4 pe=1`).
  Core SHA-256: `41cac631cc06836401c1f7ddade5f758503680f18bf2274e14c176beebe9d74e`.
- Static packaging, complete upstream notices, preserved encoding, source
  checksum, exact 23-extension table, JSON-LD, ignore rules and text-policy
  checks passed. No distribution build/signing was run, as authorized.
- No `.sh` changed. Attempted `bash -n` on setup/bootstrap could not start:
  Git Bash failed with `CreateFileMapping ... Win32 error 5` under the sandbox.

## Independent-review corrections: P1 / P2 / P3

Base: `392f31f5a`. Corrections are uncommitted. No product executable was
launched, and `build-dev` was not run, as explicitly instructed.

- P1: About embeds all three tracked root notices with `include_str!`. Full
  texts are exposed through the existing egui license pattern (collapsible
  headers, wrapped monospace text and independent vertical scroll areas).
  The renderer is shared with egui's existing notices; original egui header
  and scroll IDs are preserved. IJG attribution stays visible while collapsed.
  Launcher-only downloads now carry the notices inside the core. Original
  notice files and installer/portable copies remain unchanged.
- One new unit test compares all three embedded texts exactly with the tracked
  files and checks CDDL, DCB/FBDD/X3F BSD, zlib and IJG attribution/conditions.
  Light/dark notice snapshots are updated. A new expanded snapshot clicks all
  three real headers through the headless harness and shows their scroll areas.
  All three PNGs were reviewed; attribution, headings and text are legible.
- P2/P3: formats, troubleshooting, getting-started, all five affected migration
  pages, both readmes and spec use the lead-verified Japanese Store names.
  HEIC guidance includes HEIF plus HEVC on PCs without HEVC support; it states
  that HEVC may be paid and gives no price. Unconditional codec-free claims
  were removed. HTML guidance links directly to all applicable Store listings.
  This agrees with Microsoft's HEIF/HEVC guidance:
  https://support.microsoft.com/en-us/windows/apps/photos/photos-app-video-editor-error-can-t-view-this-file-type
- Product-page license text, CLAUDE and the source-distribution document now
  describe in-app full notices for every distribution form. Readmes preserve
  UTF-8 BOM/CRLF. No RAW behavior, dependency version or release metadata changed.

### Correction verification

`MSBUILDDISABLENODEREUSE=1` is set for Cargo. Logs are in
`target/s4-review-fixes/` (ignored). All requested automated checks passed.

- `cargo test -p mimageviewer --lib ui_dialogs::about::tests`: 2 passed,
  0 failed, including the new embed test. The isolated `cargo test -p
  mimageviewer --lib embedded_raw_notices_equal_tracked_files_with_bsd_and_ijg_attribution`
  also passed: 1 passed, 0 failed (the same test, not an additional unique test).
- `cargo test --test ui_snapshot`: 62 passed, 0 failed (includes the expanded
  interaction snapshot); all three affected PNGs visually reviewed.
- `cargo check -p mimageviewer --bin mimageviewer-core`: passed (16.42 s).
  `cargo check --bin mimageviewer-core --features portable`: passed (16.23 s).
  Existing lib warnings remain (138); no compilation errors.
- `python scripts/check_ui_glyphs.py`: zero dangerous glyphs.
  `cargo fmt --all --check` and `git diff --check`: clean.
- Guidance checks: all 10 public guidance files match HEIF/conditional HEVC;
  four verified Store names/URLs, no old names or RAW Store requirement,
  removed unconditional codec-free claims, preserved readme encoding, and
  unchanged full notice files / installer / portable lists all passed.
- Fresh upstream/public archive hash verification and distribution-build
  contents remain release-lead work; this correction run does not claim them.

## Released extension-priority completion

Base: `17f03f1a3`. Append-only correction for saved `image_ext_priority` lists.
The literal master/v4.2.0 default has 15 RAW formats; loading it now appends
exactly `crw`, `srw`, `3fr`, `erf`, `kdc`, `dcr`, `mrw`, `mos`.

- `Settings::sanitize` is the shared finalization step: SQLite startup calls
  it directly, and JSON migration calls it via `apply_load_time_migrations`
  before saving the migrated database. Missing current defaults are appended
  with case-insensitive membership checks. Existing order, casing, duplicates
  and custom entries are preserved; repeated normalization does nothing.
- Startup persists completion through the existing bootstrap save without
  consuming user-save backup rotation, even when the version marker matches.
  The Remote listing mirror reads individual database keys outside `sanitize`,
  so it uses the same helper after overlaying those keys, without writing.
- Whole-settings backup/restore preserves stored data and takes effect on
  restart through `Settings::load`; operation-customization import/export
  does not carry this field. No separate import/export migration is needed.
- The image-folder page-count fingerprint already hashes the full ordered
  list: completion changes the cache key once; later loads keep it stable.
  Preferences consumes the completed list directly. No UI layout changed.
- Spec and the preferences manual now describe append-only completion.
  No network behavior or data-storage location changed. No product was
  launched; no commit or `build-dev` run was made, as requested.

### Completion verification

All commands exited 0 with `MSBUILDDISABLENODEREUSE=1` for Cargo. Logs are
in `target/s4-priority-fix/` (ignored).

- `cargo test -p mimageviewer --lib image_ext_priority`: 9 passed, 0 failed.
  Includes exact released-default completion, custom order/entries, already
  complete lists, casing/idempotence, real SQLite and JSON loads, preferences
  state after load, the Remote overlay and fingerprint stability. The SQLite
  writeback test starts from fully initialized settings so unrelated first-load
  migrations cannot mask a missing writeback; the final focused rerun passed.
- `cargo test -p mimageviewer --lib app::folder_scan::`: 19 passed, 0 failed
  (includes the fingerprint test above).
- `cargo test -p mimageviewer --lib remote_listing_settings_`: 2 passed,
  0 failed. These three filters cover 29 distinct tests, not 30.
- `cargo check -p mimageviewer --bin mimageviewer-core`: passed (17.81 s),
  with existing warnings. The later edit only strengthened a `#[cfg(test)]`
  test, so this core-check evidence remains valid.
- `cargo fmt --all --check`, `git diff --check`: passed. Glyph lint reports
  zero dangerous glyphs. Edited documentation retains its original BOM state
  and decodes as UTF-8. No snapshots changed: priority data is covered by the
  real-load preferences-state test; the renderer and layout are unchanged.
