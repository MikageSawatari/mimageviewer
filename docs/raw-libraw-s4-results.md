# RAW via LibRaw S4 results

Base: `85ccf0721`, branch `raw-libraw`. Uncommitted implementer handoff;
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

## Verification

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
