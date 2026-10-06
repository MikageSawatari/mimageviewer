# Startup diagnostics

`miv-startup` records process-local startup evidence without becoming an
application-state owner. Call `init_process` before argument handling, and
explicitly finish spans only when their actual operation returns. Dropping a
span restores the parent stage but leaves its begin unmatched. Spans are bound
to their creating thread; construct worker spans on that worker. Parallel
registration and request events use `event_span` to avoid publishing a shared
lane's current stage.

Publishers update atomics first, then make one `try_lock` attempt to append a
fixed-size event. Both journal and writer queue are bounded at 1,024 records.
The writer takes a fixed batch and releases the lock before serialization or
I/O. Diagnostic errors publish one consumable unavailable notice; there is no
logger recursion, fallback disk, retry, or shutdown join.

The separate watchdog has five fixed, non-reusable startup slots. A core
reserves its later slots before the watchdog starts, preventing an early exit
between present and dispatch. Owners must retire unneeded slots. Only the first
dispatch receives a capability; Remote takeover retires it with
`SuspendedNotWatched`. Metadata registration and redispatch remain timeline
events and cannot extend or restart the watchdog. Intentional suspensions pause
the effective clock for parent and watched children. Threshold masks belong to
the observer, so heartbeat, child changes, and parent restoration do not reset
the 5/15/30-second notices. `RunNative` is explicitly excluded as a child.
The core parent ends at its first root `present()` return; first-update timing
is separate. The normal-paint parent starts only when normal paint is requested.
Reserved slots cannot be resumed or accidentally restarted by repeated begin
calls after a zero-duration intentional pause. The writer parks while idle and
publishers wake it after releasing the queue lock.

Launcher snapshots use the validated `MIV_STARTUP_TRACE_V1` internal environment
value, with a shared QPC origin/frequency and at most 170 recent 72-byte records
within 24 KiB. Inherited records retain their span IDs and are marked inherited
in the core log. No inherited pathname is opened. The header preserves the
entry clock when recent-event eviction loses the original entry event.
Each event carries its source process/thread IDs, role and raw QPC tick value,
including inherited launcher records. The per-lane snapshot's total duration
uses the shared run origin; stage duration uses that stage's own begin clock.

Explicit `--data-dir` and portable invocations write below that data directory;
normal invocations use LOCALAPPDATA. Files are capped at 2 MiB and pruning keeps
the ten most recent run IDs, skips locked active writers, and only recognizes
this crate's exact log filename format. Data-directory environment probes run
on a separate worker. UNC recognition is pure; mapped-drive and reparse probes
never run in publishers. Unproven redirection remains `unknown`.
