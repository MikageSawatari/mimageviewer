# EPUB → PDF technical spike

This binary is standalone. It is a workspace member, but no mImageViewer package
depends on it. Build and test with `-p epub-pdf-worker`.

## Commands and exit codes

```powershell
cargo build -p epub-pdf-worker
.\target\debug\mimageviewer-epub-pdf.exe inspect book.epub
.\target\debug\mimageviewer-epub-pdf.exe convert book.epub book.pdf --work-dir .\target\epub-spike\book --report .\target\epub-spike\book.json
.\target\debug\mimageviewer-epub-pdf.exe convert book.epub book.pdf.part --work-dir .\target\epub-spike\book --user-data-dir .\target\epub-spike\epub-1234 --progress-json
.\target\debug\mimageviewer-epub-pdf.exe batch C:\path\to\epubs .\target\epub-spike\batch
```

`batch` searches recursively. It writes one JSON report per EPUB and
`summary.json` / `summary.md`. One failed book does not stop later books. The
batch process returns the first nonzero book code after writing the summary.
`convert` writes the complete PDF exactly to the requested output path. It
merges and verifies a temporary file in `--work-dir` before renaming; a failed
or timed-out conversion leaves no partial output at the requested path. The
input EPUB is opened only for reading. `--user-data-dir` selects a dedicated
WebView2 folder; it must be absent or empty. If omitted, a unique folder below
`--work-dir` is used. The report records whether cleanup succeeded.

`--progress-json` applies to `convert`. It makes stdout a line-delimited JSON
stream with no human messages. Phases are `parse`, `extract`, `init`, `print`,
`merge`, and `verify`. For example:

```json
{"event":"progress","phase":"print","done":3,"total":0}
{"event":"result","status":"success","exit_code":0,"page_count":3,"direction":"rtl","layout":"pre-paginated","profile":"reflow-v1","blocked_requests":0,"message":""}
```

`print.done` is the number of PDF pages printed so far; `print.total` is 0
because reflow pagination is not known in advance. Other phases use 0/1 and
1/1. Result statuses are `success`, `drm`, `invalid`, `webview2_missing`,
`render_failed`, and `timeout`. `blocked_requests` counts blocked attempts.
The detailed per-book report contains up to 200 distinct blocked URLs and the
total attempt count, plus `book_script_ran`.

| Code | Meaning |
|---:|---|
| 0 | success |
| 2 | DRM marker or unsupported encryption detected |
| 3 | invalid EPUB, invalid path, or CLI input |
| 4 | WebView2 Runtime missing |
| 5 | WebView2 initialization, rendering, PDF validation, or merge failure |
| 6 | timeout |

`inspect` reads the ZIP/package only. `batch` uses a unique
WebView2 user data folder below the chosen work directory. The batch work
directory is `<out-dir>/_work`. The folder is deleted after the controller and
environment are released; the report records deletion result and elapsed time.
Extracted content and intermediate PDFs remain in the work directory for
diagnosis.

## Dependencies and implementation choices

`webview.rs` uses `webview2-com` 0.39 for WebView2 COM interfaces and completion
handlers. This crate uses `windows` 0.62. On MSVC, `webview2-com-sys` links
`WebView2LoaderStatic.lib`, so no `WebView2Loader.dll` is needed next to the
executable. The WebView2 Runtime is still required. Build offline with
`cargo build -p epub-pdf-worker --offline` when crates.io is unavailable.

The parser uses `quick-xml` 0.41 and `zip` 2. Package reading rejects zip-slip
paths and unsupported encryption. Fixed spine items are printed in spine order
with named CSS pages matching each measured item size. Reflow items use a
the named `reflow-v1` profile: 720 × 1024 CSS px with 32 px page margins.
Book CSS is retained. The renderer uses WebView2 virtual host folder mapping
and `Page.printToPDF`. Fixed runs are bounded to 50 items
per print document to limit Chromium memory use. The PDFs are merged by
`lopdf` 0.36.0, and RTL books set `/ViewerPreferences /Direction /R2L`.

The report lists PDF page boxes and image XObjects. A JPEG is marked `exact`
only when its DCTDecode stream has the same dimensions and SHA-256 as the
source. `dct_similar` means matching dimensions and compressed length within
±20%; it cannot prove the pixels are unchanged. PNG is marked `png_flate` for
matching dimensions and FlateDecode. This is a byte-level fidelity screen,
not a visual comparison. Image and layout quality must be checked separately
by rendering PDF pages.

All WebView2 resource contexts are filtered. Only the mapped
`https://epub.invalid` origin is allowed; other requests receive a synthetic
403 response and cross-origin navigations are cancelled. Page scripts are
disabled. Host readiness and script probes use CDP `Runtime.evaluate` rather
than `ExecuteScript`, whose interaction with disabled page scripts is not
specified by the locally available bindings. Runtime behavior still needs an
outside-sandbox check.

For the outside-sandbox network and script check, run both synthetic books and
inspect each JSON report's `blocked_requests`, `blocked_request_count`, and
`book_script_ran`. Confirm zero outbound requests with an external process
network trace; the report alone records blocked attempts, not packets.

```powershell
.\target\debug\mimageviewer-epub-pdf.exe convert C:\home\mimageviewer_testdata_epub\synthetic\probe_net_script_reflow.epub .\target\epub-probe\reflow.pdf.part --work-dir .\target\epub-probe\reflow-work --user-data-dir .\target\epub-probe\reflow-user-data --report .\target\epub-probe\reflow.json --progress-json
.\target\debug\mimageviewer-epub-pdf.exe convert C:\home\mimageviewer_testdata_epub\synthetic\probe_net_script_fixed.epub .\target\epub-probe\fixed.pdf.part --work-dir .\target\epub-probe\fixed-work --user-data-dir .\target\epub-probe\fixed-user-data --report .\target\epub-probe\fixed.json --progress-json
```

## Job Object acceptance probe (not shipped)

Build both binaries with `cargo build -p epub-pdf-worker --offline`. The probe
starts the converter suspended, assigns it to a Job with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, then resumes it. It polls the Job PID
list and independently snapshots process parent chains. It prints one JSON
summary and succeeds only when all WebView2 descendants belonged to the Job
and all observed processes disappear after cancellation. Run outside the
restricted sandbox while the 300-page book is converting:

```powershell
.\target\debug\epub-pdf-job-probe.exe cancel C:\home\mimageviewer_testdata_epub\synthetic\manga_rtl_300p_large.epub .\target\epub-probe\cancel 60
.\target\debug\epub-pdf-job-probe.exe parent-kill C:\home\mimageviewer_testdata_epub\synthetic\manga_rtl_300p_large.epub .\target\epub-probe\parent-kill 60
```

`parent-kill` relaunches the probe as an intermediate owner, terminates that
owner, then checks that its converter and every observed WebView2 PID vanished.
The summary includes `job_pids`, `webview_pids`, `webview_outside_job`, `gone`,
and `success`. Probe outputs are disposable.
