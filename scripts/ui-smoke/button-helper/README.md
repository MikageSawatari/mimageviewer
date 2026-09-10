# Native button helper

This directory is the tracked external owner for the
`NativeTopPanoramaClick` diagnostic scenario. It owns one real left-button
Down/Up gesture outside the application process, so it can release a Down that
it inserted even when the disposable application exits or stops replying.

`scripts/ui-smoke.ps1` loads these sources only for `NativeTopPanoramaClick`.
It launches the exact disposable App with a one-run pipe name, session nonce,
and expected server PID, then starts the helper with the returned App PID. The
App authenticates that endpoint and carries the gesture and step identity
through its prepared target, WndProc, pump/render, actual Response command, and
normal App handler receipt.

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
