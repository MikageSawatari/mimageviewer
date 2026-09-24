# EPUB → PDF technical spike

This binary is standalone. It is a workspace member, but no mImageViewer package
depends on it. Build and test with `-p epub-pdf-worker`.

## Commands and exit codes

```powershell
cargo build -p epub-pdf-worker
.\target\debug\mimageviewer-epub-pdf.exe inspect book.epub
.\target\debug\mimageviewer-epub-pdf.exe convert book.epub book.pdf --work-dir .\target\epub-spike\book --report .\target\epub-spike\book.json
.\target\debug\mimageviewer-epub-pdf.exe batch C:\path\to\epubs .\target\epub-spike\batch
```

`batch` searches recursively. It writes one JSON report per EPUB and
`summary.json` / `summary.md`. One failed book does not stop later books. The
batch process returns the first nonzero book code after writing the summary.

| Code | Meaning |
|---:|---|
| 0 | success |
| 2 | DRM marker or unsupported encryption detected |
| 3 | invalid EPUB, invalid path, or CLI input |
| 4 | WebView2 Runtime or loader missing |
| 5 | WebView2 initialization, rendering, PDF validation, or merge failure |
| 6 | timeout |

`inspect` reads the ZIP/package only. `convert` and `batch` use a unique
WebView2 user data folder below the chosen work directory. The batch work
directory is `<out-dir>/_work`. The folder is deleted after the controller and
environment are released; the report records deletion result and elapsed time.
Extracted content and intermediate PDFs remain in the work directory for
diagnosis. The input EPUB is never modified.

## Dependencies and implementation choices

The offline Cargo cache in this worktree has no `webview2-com` and the configured
crates.io connection is blocked. This spike therefore uses a narrow, local raw
COM binding in `webview.rs` with the WebView2 SDK interface IDs and vtable
slots. Win32 calls use `windows` 0.61 (resolved 0.61.3). The WebView2 loader is
loaded dynamically: `WebView2Loader.dll` must be next to the executable for a
portable build. For a local experiment, `MIV_WEBVIEW2_LOADER` can name an
existing loader DLL by absolute path. The WebView2 Runtime is still required.
No DLL is copied into the repository.

The parser uses `quick-xml` 0.41 and `zip` 2. Package reading rejects zip-slip
paths and unsupported encryption. Fixed spine items are printed in spine order
with named CSS pages matching each measured item size. Reflow items use a
1200 × 1700 CSS px page and 48 px margins. The renderer uses WebView2 virtual
host folder mapping and `Page.printToPDF`. Fixed runs are bounded to 50 items
per print document to limit Chromium memory use. The PDFs are merged by
`lopdf` 0.36.0, and RTL books set `/ViewerPreferences /Direction /R2L`.

The report lists PDF page boxes and image XObjects. A JPEG is marked `exact`
only when its DCTDecode stream has the same dimensions and SHA-256 as the
source. `dct_similar` means matching dimensions and compressed length within
±20%; it cannot prove the pixels are unchanged. PNG is marked `png_flate` for
matching dimensions and FlateDecode. This is a byte-level fidelity screen,
not a visual comparison. Image and layout quality must be checked separately
by rendering PDF pages.
