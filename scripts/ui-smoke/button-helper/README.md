# Native button helper

This directory is the tracked external owner for the
`NativeTopPanoramaClick` diagnostic scenario. It owns one real left-button
Down/Up gesture outside the application process, so it can release a Down that
it inserted even when the disposable application exits or stops replying.

`scripts/ui-smoke.ps1` loads these sources for `NativeTopPanoramaClick` and `ClipboardCapture`.
It launches the exact disposable App with a one-run pipe name, session nonce,
and expected server PID, then starts the helper with the returned App PID. The
App authenticates that endpoint and carries the gesture and step identity
through its prepared target, WndProc, pump/render, actual Response command, and
normal App handler receipt.

`ClipboardCapture` uses `ClipboardKeyHelperHandle` from this package as a
separate keyboard owner. It reuses the pinned process lease, current-SID local
pipe authentication and runner cleanup/join interlock. Its bounded 56-byte
Begin/Release requests carry session, gesture ID, root HWND, App PID and backend
token. Rust validates the current backend before requesting a gesture; the
external driver validates exact process/HWND/foreground/desktop and initially
released user modifiers before inserting Ctrl/V. The App waits for its actual
GetAsyncKeyState paste consumer counter, then requests Release. Only inserted
Down keys create release obligations. Death, disconnect, cancellation and a
five-second held-key deadline run cleanup outside the App process. A desktop
change or unconfirmed release keeps the existing App-kill interlock closed.
The keyboard path has independent wire/owner logic; the mouse protocol and
Response/WndProc receipt contract remain intact.

The implementation was promoted from the independently reviewed ignored
reducer/backend checkpoint under `target/v370-work`. The previously unexecuted
host draft is covered here by real current-SID own-process pipe tests for normal
completion, App death, EOF, reader/protocol faults, bounded reply failure,
retained joins, and unresolved release. Cleanup Up revalidates only the current
input desktop/access scope; it deliberately does not require the stale target,
foreground, cursor, or capture.

The tracked tests keep the protocol, owner, native preflight, cleanup, process,
pipe, and runner finalization contracts executable under both Windows
PowerShell 5.1 and PowerShell 7. Runner cleanup calls `CancelAndJoin` before it
may terminate the exact App. `StillRunning`, an unknown release state, or a
confirmed outstanding release keeps the App-kill interlock closed. An earlier
App timeout, nonzero exit, preparation error, or script failure remains the
reported primary failure even when helper cleanup succeeds.

The scenario requires the `portable,test-script` build. Its interactive run is
pending a separate, concrete approval; noninteractive checks do not construct
the native inserter or call `SendInput`.

Run the noninteractive fake and own-process pipe checks with:

```powershell
powershell.exe -NoProfile -File .\scripts\test-ui-smoke-button-helper.ps1
pwsh -NoProfile -File .\scripts\test-ui-smoke-button-helper.ps1
powershell.exe -NoProfile -File .\scripts\test-ui-smoke-button-runner.ps1
pwsh -NoProfile -File .\scripts\test-ui-smoke-button-runner.ps1
```

None of these test commands constructs the native input inserter or calls
`SendInput`.
