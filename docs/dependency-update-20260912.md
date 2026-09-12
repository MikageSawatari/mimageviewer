# PDFium / FFmpeg dependency update — 2026-09-12

This record covers the post-v3.9.0 dependency refresh. It records the exact
binary and source identities used by the build; it is not a release or
publication record.

## Result

| Dependency | Before | Result |
| --- | --- | --- |
| PDFium | `chromium/8035`, `154.0.8035.0` | Updated to `chromium/8044`, `155.0.8044.0` |
| FFmpeg | `n7.1.5-16-g9a4bb2c579` | Kept; the newest retained versioned BtbN n7.1 LGPL shared asset is the older `n7.1.5-12-g1fdbca85aa` build |

PDFium continues to use `pdfium-win-x64.tgz`, the x64 build without V8 or
XFA. The package's `args.gn` retains `target_cpu = "x64"`,
`pdf_enable_v8 = false`, and `pdf_enable_xfa = false`.

The official `chromium/8044` asset is 3,818,370 bytes with SHA-256
`78a17d9a5f14467631c26a3ac8741b27a0471ecc05bd6a119b523598160a0537`.
The installed `pdfium.dll` reports `155.0.8044.0` and has SHA-256
`04100c03e41cac1f979e36e5e26fb860bcb5a7461f53830d3c098716624a27a9`.
The package's DLL, headers, CMake metadata, import library, and license files
were compared with `vendor/pdfium/`; all 45 package files matched. The old
`licenses/libtiff.txt`, which is absent from the new package, was removed so
the vendor directory exactly mirrors the selected asset.

## FFmpeg identity and LGPL source

The six bundled DLLs still report
`n7.1.5-16-g9a4bb2c579-20260816`, with DLL ABI names
`avcodec-61`, `avformat-61`, `avutil-59`, `avfilter-10`, `swscale-8`, and
`swresample-5`. The generated LGPL report found the same 55 external-library
flags as the previous report, retained `--enable-version3` and
`--enable-libsvtav1`, and found neither `--enable-libx264` nor
`--enable-libx265`.

The exact corresponding source remains:

- FFmpeg commit: `9a4bb2c579a16b0469759743d6917d9e8e3cb8c6`
- archive: `ffmpeg-n7.1.5-16-g9a4bb2c579-source.tar.gz`
- archive SHA-256: `0fc3518ed595a37a507add4ecdfdf834dfceba27020b539a6b7ac1e6fd243bb0`
- archive `RELEASE`: `7.1.5`

The archive in `htdocs/mimageviewer/` and the backup in
`C:\home\mimageviewer_vendor_backup\ffmpeg-lgpl-source\` have identical
bytes and match the tracked `.sha256` file. The product page already names
this current build and source archive, and retains links for older builds; no
old source archive was removed.

`scripts/setup-ffmpeg.sh check` currently compares asset names for equality.
Because BtbN retains the last monthly n7.1 build while the locally selected
August build is newer, it can describe the retained `-12` asset as an update
and an unchecked setup run could downgrade `-16`. This refresh therefore did
not run the FFmpeg setup command. A future script fix needs an explicit pinned
asset or trustworthy branch-aware ordering; comparing only the numeric
distance from the release tag is not a general version ordering rule.

## Build and validation boundary

The PDFium bytes are embedded in the normal core and carried inside the
launcher. Portable builds use the same selected DLL as a loose file. FFmpeg
remains dynamically linked by the core and is embedded, together with the
core and remote service, by the launcher. Validation must therefore cover:

1. PDFium package/DLL/header/import-library identity and PDF worker startup.
2. Normal and password-protected PDF enumeration/rendering regressions.
3. FFmpeg DLL/header/import-library ABI identity, license/configure flags, and
   a noninteractive decode path.
4. The full automated gate.
5. A release core, embedded-web remote service, and launcher build using
   `scripts/build-release.ps1 -PreserveRuntime`, followed by hash checks that
   the launcher inputs use the selected dependency bytes.

No application is launched as part of this dependency update. Interactive PDF
and video confirmation remains a separate explicitly approved verification
session.

## Validation record

- `cargo test -p mimageviewer --lib pdf_loader::tests --jobs 1 -- --nocapture`:
  73 passed.
- A disposable PDF-worker protocol probe enumerated and rendered two pages from
  both a normal PDF and an encrypted PDF, and rejected an incorrect password.
  The worker extracted the selected PDFium DLL byte-for-byte.
- The FFmpeg seek-decode probe opened a generated MP4 through the bundled DLLs;
  software keyframe and full-frame paths completed without missed frames, and
  hardware requests followed the supported software fallback path.
- `cargo check -p mimageviewer --bin mimageviewer-core --jobs 1` succeeded.
- The full workspace gate completed every target except that the parallel
  `ui_snapshot` process ended with Windows status `0xc0000005`. The bounded
  single-thread rerun then passed all 48 snapshots. This is recorded as a gate
  anomaly rather than a PDFium regression; a similar wgpu snapshot-process
  crash predates this update, but its cause remains unproven.

After the user closed the existing mImageViewer processes, the unsigned release
core, embedded-web remote service, and launcher were built with
`scripts/build-release.ps1 -PreserveRuntime`. The first incremental wrapper run
returned success without rebuilding the core: its modification time predated
this update and the new PDFium bytes were absent. This is the stale release-cache
boundary already documented by the wrapper. The verification therefore removed
only the `mimageviewer` package's release artifacts with
`cargo clean -p mimageviewer --release`, then reran the wrapper with
`-PreserveRuntime -SkipVst3Bridge`. This update does not change the VST3 bridge,
and the package-only clean preserved the existing bridge, so that unchanged
binary was reused.

The rebuilt core contains the new PDFium DLL exactly once and contains no exact
copy of the previous `154.0.8035.0` DLL. The rebuilt launcher contains the exact
rebuilt core and remote executable once each, and contains each of the six
selected FFmpeg DLLs exactly once. The application was not launched.
