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
merges and verifies a unique temporary file named `<out>.tmp-<pid>-<n>` beside
`<out>` before renaming; a failed
or timed-out conversion leaves no partial output at the requested path. The
input EPUB is opened only for reading. `--user-data-dir` selects a dedicated
WebView2 folder; it must not already exist, even if empty. If omitted, a unique folder below
`--work-dir` is used. The converter rejects a user data folder that contains
the input, output, output directory, or work directory. This containment
pre-check compares path components without case (conservatively, it may reject
an otherwise distinct path, but must not accept a containment risk). The worker
creates and owns the requested user data folder and reports whether cleanup succeeded.
It deletes that folder recursively only when WebView2 confirms it used that
folder. If WebView2 used another folder or identity could not be verified, it
removes the requested folder only when empty and reports
`user_data_cleanup.skipped_reason` when content remains. The S2 host passes a fresh
path and will remove all
`WEBVIEW2_*` variables before spawning the worker, and the worker clears them
again before using the loader.

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
`render_failed`, `timeout`, and `webview2_unsupported`.
`blocked_requests` counts blocked attempts.
When `--progress-json` is present, argument errors and panics also produce
exactly one final `result` line. Panics use `render_failed` / exit code 5.
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
| 7 | unused |
| 8 | WebView2 lacks a required interface, such as the all-source request filter |

`--timeout-secs 0` is invalid CLI input (exit 3); it cannot exercise the
WebView2 timeout path. The debug build has a test-only
`MIV_EPUB_PDF_TEST_PANIC=after_user_data` hook immediately after creating the
user data folder. Caught panics release WebView2 objects and delete a folder
created by this worker before emitting the final result line.

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
with named CSS pages matching each measured item size. Reflow items use the
named `reflow-v1` profile: 720 × 1024 CSS px with 32 px page margins.
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

All WebView2 resource contexts and request source kinds require
`ICoreWebView2_22`; an older runtime fails with `webview2_unsupported` rather
than using the narrower legacy filter. The per-book report's
`web_resource_filter` identifies the selected scope. Only the mapped
`https://epub.invalid` origin is allowed; other requests receive a synthetic
403 response and cross-origin navigations are cancelled. Page scripts are
disabled. Host readiness and script probes use CDP `Runtime.evaluate` rather
than `ExecuteScript`, whose interaction with disabled page scripts is not
specified by the locally available bindings. Runtime behavior still needs an
outside-sandbox check.

The worker requests these browser arguments before a WebView2 controller is
created. WebView2 policies may change the effective settings:

| Argument | Reason |
| --- | --- |
| `--host-resolver-rules="MAP * ^NOTFOUND"` | Fail DNS resolution for arbitrary host names. |
| `--disable-background-networking` | Suppress Chromium background services. |
| `--dns-prefetch-disable` | Suppress speculative DNS lookups. |
| `--disable-preconnect` | Request suppression of speculative TCP preconnects, including IP literal hints; confirm on Runtime 153 with a connection trace. |
| `--no-pings` | Suppress hyperlink auditing pings. |
| `--disable-sync` | Disable account synchronization. |
| `--disable-component-update` | Disable component update checks. |
| `--disable-extensions` | Disable extension activity. |
| `--no-first-run` | Skip first-run browser activity. |

The virtual host maps to a local folder through
`SetVirtualHostNameToFolderMapping`; the local WebView2 bindings expose the
folder mapping but do not document DNS behavior. Successful conversion with
the resolver rule, together with a DNS/network trace, is the required runtime
confirmation that this mapping does not resolve `epub.invalid` externally.
Before environment creation, the worker removes every `WEBVIEW2_*` process
variable, including `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`,
`WEBVIEW2_USER_DATA_FOLDER`, `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`,
`WEBVIEW2_RELEASE_CHANNEL_PREFERENCE`, and
`WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER`. The worker does not inspect or reject
WebView2 registry policies; WebView2 follows any policies present. After
environment creation, the worker compares
`ICoreWebView2Environment7::UserDataFolder` with the requested folder by
directory identity (volume serial and file ID from `FileIdInfo`). A different
actual folder is logged and recorded as `user_data_folder_redirected` in the
per-book report, and conversion continues. A failed folder query or identity
comparison is recorded as `user_data_folder_check_error` and also does not stop
conversion. Cleanup never recursively deletes a redirected or unverifiable
folder; it never targets the reported actual folder when redirected.
The new filter includes service-worker and shared-worker request sources; a
fresh requested user data folder and disabled page scripts prevent book service
workers from being registered when WebView2 uses that folder. Cross-origin
iframes are cancelled at navigation and
their resources receive 403. Page scripts cannot initiate WebSockets while
disabled. WebSocket handshakes are not guaranteed to raise
`WebResourceRequested`; the resolver rule blocks hostnames, while a direct-IP
WebSocket would require script execution. Confirm zero DNS and connection
attempts with an external trace, including declarative preconnects.

For the outside-sandbox network and script check, run both synthetic books and
inspect each JSON report's `blocked_requests`, `blocked_request_count`, and
`book_script_ran`. Confirm zero outbound requests with an external process
network trace; the report alone records blocked attempts, not packets.

```powershell
.\target\debug\mimageviewer-epub-pdf.exe convert C:\home\mimageviewer_testdata_epub\synthetic\probe_net_script_reflow.epub .\target\epub-probe\reflow.pdf.part --work-dir .\target\epub-probe\reflow-work --user-data-dir .\target\epub-probe\reflow-user-data --report .\target\epub-probe\reflow.json --progress-json
.\target\debug\mimageviewer-epub-pdf.exe convert C:\home\mimageviewer_testdata_epub\synthetic\probe_net_script_fixed.epub .\target\epub-probe\fixed.pdf.part --work-dir .\target\epub-probe\fixed-work --user-data-dir .\target\epub-probe\fixed-user-data --report .\target\epub-probe\fixed.json --progress-json
```

To test connection hints that may run before `WebResourceRequested`, make a
disposable variant of the reflow probe. This adds a direct-IP preconnect and a
DNS-prefetch hint without changing the source EPUB:

```powershell
New-Item -ItemType Directory -Force .\target\epub-probe | Out-Null
@'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as source, zipfile.ZipFile(sys.argv[2], 'w') as target:
    for entry in source.infolist():
        data = source.read(entry.filename)
        if entry.filename == 'OEBPS/p1.xhtml':
            data = data.replace(b'</head>', b'<link rel="preconnect" href="http://127.0.0.1:8765"/><link rel="dns-prefetch" href="//mimageviewer-net-probe.invalid"/></head>')
        target.writestr(entry, data)
'@ | python - C:\home\mimageviewer_testdata_epub\synthetic\probe_net_script_reflow.epub .\target\epub-probe\preconnect.epub
.\target\debug\mimageviewer-epub-pdf.exe convert .\target\epub-probe\preconnect.epub .\target\epub-probe\preconnect.pdf.part --work-dir .\target\epub-probe\preconnect-work --user-data-dir .\target\epub-probe\preconnect-user-data --report .\target\epub-probe\preconnect.json --progress-json
```

Capture DNS queries and TCP connection attempts from the converter and its
WebView2 descendants while that conversion runs. Also confirm the mapped book
still prints with the resolver rule enabled. A local HTTP server's request
log alone cannot detect a preconnect that opened a socket without sending HTTP.

## Job Object acceptance probe (not shipped)

Build both binaries with `cargo build -p epub-pdf-worker --offline`. The probe
starts the converter suspended, assigns it to a Job with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, then resumes it. It polls the Job PID
list and independently snapshots process parent chains and command lines. Each
run gives the converter a unique user data folder containing a `miv-job-...`
marker. The probe scans every WebView2 process for that marker in its
`--user-data-dir` argument, even if its parent chain no longer reaches the
converter. It prints one JSON summary and succeeds only when every observed
marker PID belonged to the Job, a nonempty live marker set and the converter
belonged to the Job immediately before cancellation, and a system-wide scan
finds no marker process or converter afterward.

The probe reads process command lines with
`NtQueryInformationProcess(ProcessCommandLineInformation)` using limited query
access. If a live WebView2 command line cannot be read, the probe fails with
that PID rather than silently omitting it. Run outside the restricted sandbox
while the 300-page book is converting:

```powershell
.\target\debug\epub-pdf-job-probe.exe cancel C:\home\mimageviewer_testdata_epub\synthetic\manga_rtl_300p_large.epub .\target\epub-probe\cancel 60
.\target\debug\epub-pdf-job-probe.exe parent-kill C:\home\mimageviewer_testdata_epub\synthetic\manga_rtl_300p_large.epub .\target\epub-probe\parent-kill 60
```

`parent-kill` relaunches the probe as an intermediate owner, terminates that
owner, then checks that its converter and every live marker-matching WebView2
PID vanished.
Immediately before closing the Job or killing the owner, the probe requires
the converter and every currently live marker-matching WebView2 PID to be in
the Job. Previously observed short-lived PIDs may already have exited.
The summary includes `job_pids`, `pre_cancel_job_pids`,
`pre_cancel_webview_pids`, `webview_pids`,
`webview_outside_job`, `marker`, `marker_pids`, `marker_outside_job`,
`pre_cancel_marker_pids`, `pre_cancel_alive`, `termination_ms`, `gone`, and
`success`. Probe outputs are disposable.
