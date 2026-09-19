# Rename migration journal recovery (M-2)

Status: implemented and independently reviewed 2026-09-20; automated verification and dev-runtime build passed. Interactive verification remains pending.

## Failure and invariant

`rename_migration_journal.json` is the recovery record for unfinished path-key and
collection source migrations. Previously `journal_load` returned an empty list for
every read or parse failure. The next empty snapshot could delete the unreadable
file, while a nonempty snapshot could replace it. Only a genuine `NotFound` may
mean an empty journal. Any other read error and a document that parses as neither
the current nor the legacy format are recovery failures. The original bytes must
remain untouched until a later successful read merges the record into App-owned
state. The legacy pair list remains readable and becomes Tree-scoped jobs.

`RenameMigrationJournalAdmission` owns the single recovery and persistence gate:
unloaded, recovery-read failure, explicit retry in progress, durable, write
pending, and write failure. The separate `rename_migration_journal_loaded` bool
is removed. The first lazy read remains the existing one-time UI read; ordinary
operations add no filesystem probe, read, or wait. A user action after a read
failure explicitly retries the read on a worker. While that retry is pending,
the action is held before physical change with a visible reason; the user may
repeat it after completion. A failed retry retains the failure state and old
bytes. Successful retry prepends recovered jobs to any locally retained work,
publishes a complete snapshot if needed, and awaits its save ACK before starting
any migration. No periodic retry is added. The successful retry-worker spawn reserves the next repaint, and the retry poll keeps a bounded 100 ms wakeup until its result is consumed; an idle window cannot strand the visible “rechecking” state.

The App owner refuses every snapshot write, empty deletion, migration start, and
immediate delete invalidation while the prior journal is unloaded or unreadable.
Successful delete paths arriving from an already-running worker are retained in
the same typed recovery admission state, then applied to the merged queue after
a successful retry. In-flight migration completion likewise remains in memory
for reconciliation; it does not rewrite the unreadable file. Exit does not
publish an empty/new snapshot in this state and warns that recovery is still
needed. Existing bounded retries for a
**write** failure stay separate from recovery-read failure.

## Physical operation boundary

The admission check sits before starting a Shell rename or delete worker and
before releasing a viewer for deletion. It also sits in the shared worker
entrypoints for book rename, book delete, reorder flush, and transfer **Move and
Copy**. Copy may first commit the source page numbering via a physical rename.
Book append/create/list and viewing remain available. A rejected request shows
the recovery reason. This is limited to mIV-owned operations; external file
managers and a full durable-before-filesystem transaction are outside M-2.

Delete completion must invalidate queued generic migration stages by the exact
successful paths, not by a reconstructed UI setting. The book delete result
therefore carries its successful folder path back to App. The same guarded
invalidation is used for ordinary delete and book delete. A Shell delete
pending request and a book rename/delete/reorder/transfer pending request hold
the **generic** migration queue head until App consumes the worker result.
The book pending owner distinguishes unrelated list/create/append work from
path mutations and captures the exact requested delete folder. This closes the
physical-success-to-result-consumption gap: success invalidates by its actual
path before releasing the gate, while failure/cancel leaves the job intact.
A generic migration already running before the delete request remains subject
to the existing non-cancellable-worker limitation; M-2 prevents new starts
while a deletion is pending. A prior in-flight completion cannot bypass the
recovery gate to overwrite the old journal.

Book delete invalidates by its exact folder path only when `remove_dir_all`
fully succeeds. A partial removal followed by `Err` does not report individual
successful paths today; after that failure, a queued generic migration could
still recreate metadata for a removed page. This pre-existing partial-delete
case needs a separate typed result design and is outside M-2.

The Shell rename pending request also owns its captured `Exact`/`Tree` scope
beside the receiver (M-1). Clearing the dialog and polling an empty receiver
cannot change that scope. Cancel, error, and success each consume the same typed
request.

## Verification matrix

- Current and legacy journal files load and merge in FIFO order; genuine
  `NotFound` loads an empty queue.
- Inject read and parse failures and compare the old bytes after empty and
  nonempty persistence attempts, delete invalidation, operation admission,
  migration poll, and exit.
- Retry performs no UI-thread read, merges prior jobs and locally retained
  completions, waits for the complete-snapshot ACK, and starts only the durable
  queue head. Retry failure leaves original bytes unchanged.
- Shell rename/delete and book rename/delete/reorder/Move/Copy reject before
  filesystem worker spawn when recovery fails. Book delete uses the exact
  successful path for invalidation; ordinary delete keeps its existing path
  scope. Book append/create and viewing remain unaffected.
- Rename pending retains captured scope through dialog clear and Empty poll;
  success, cancellation, and failure clear it exactly once.

Automated verification on 2026-09-20:

- `cargo test -p mimageviewer --lib journal -- --nocapture`: 32 passed.
- `cargo test -p mimageviewer --lib rename_migration -- --nocapture`: 8 passed.
- `scripts/test-full.ps1`: passed, including 8,686 mimageviewer library tests, workspace integration tests, and vendored egui/eframe tests.
- After the final retry-wakeup change, `cargo test -p mimageviewer --lib recovery -- --nocapture`: 17 passed; `cargo test -p mimageviewer --lib`: 8,686 passed, 45 ignored. Unchanged workspace/vendor checks use the preceding full-gate result.
- `cargo fmt --all -- --check`, `python scripts/check_ui_glyphs.py`, and `git diff --check`: passed.
- `scripts/build-dev.ps1 -PreserveRuntime`: passed; core and remote service built in `target/dev-runtime`.

No normal-profile application launch, real-data edit, or interactive input was
part of automated verification. A user-run interactive check remains pending.

The existing persistence-**write** failure path still accepts a new filesystem
operation before its resulting migration intent has a durable ACK, then holds
the migration and performs bounded save retries. M-2 only closes the distinct
unreadable-prior-record hazard; a durable-before-filesystem transaction for
write failure remains outside this change.
