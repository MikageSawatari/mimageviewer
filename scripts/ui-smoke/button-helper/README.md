# Native button helper

This directory is the tracked external-owner checkpoint for the future
`NativeTopPanoramaClick` diagnostic scenario. It owns one real left-button
Down/Up gesture outside the application process, so it can release a Down that
it inserted even when the disposable application exits or stops replying.

The runner and Rust App are not connected at this checkpoint. In particular,
`scripts/ui-smoke.ps1` cannot load this helper or request input yet. The next
checkpoint must add the exact prepared-target, WndProc/pump/render, Response,
command, and App-effect receipts before the runner may expose the scenario.

The implementation was promoted from the independently reviewed ignored
reducer/backend checkpoint under `target/v370-work`. The previously unexecuted
host draft is covered here by real current-SID own-process pipe tests for normal
completion, App death, EOF, reader/protocol faults, bounded reply failure,
retained joins, and unresolved release. Cleanup Up revalidates only the current
input desktop/access scope; it deliberately does not require the stale target,
foreground, cursor, or capture.

The tracked tests keep the protocol, owner, native preflight, cleanup, process,
and pipe contracts executable under both Windows PowerShell 5.1 and PowerShell
7. The eventual application side requires the `portable,test-script` build and
an explicit interactive approval before this code may send input.

Run the noninteractive fake and own-process pipe checks with:

```powershell
powershell.exe -NoProfile -File .\scripts\test-ui-smoke-button-helper.ps1
pwsh -NoProfile -File .\scripts\test-ui-smoke-button-helper.ps1
```

Neither test command constructs the native input inserter or calls
`SendInput`.
