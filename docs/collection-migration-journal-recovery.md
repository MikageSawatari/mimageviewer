# Rename migration journal recovery (M-2 / RB-1, RB-8, RB-9)

Status: the unreadable-journal gate and explicit quarantine recovery are implemented. The 2026-09-21 focused automated checks passed; the final repository-wide gate is owned by the integrating task.

## Failure and invariant

`rename_migration_journal.json` records unfinished path-key and collection source
migrations. Only a genuine `NotFound` means an empty journal. Other read errors
remain `JournalLoadError::Read`; bytes that parse as neither the current nor the
legacy format remain `JournalLoadError::Parse` together with the exact bytes that
failed. A read or parse failure must never be converted to an empty snapshot.

`RenameMigrationJournalAdmission` is the single owner of recovery and
persistence admission. Its states cover unloaded, failed read/parse, retrying,
quarantining, durable, save-waiting, and save-failed. While recovery is failed or
running, the App refuses snapshot replacement, empty deletion, migration start,
and every mIV-owned physical operation that could require a path-key migration.
Late worker results and successful delete invalidations remain in the typed owner
for later reconciliation.

Read errors are retried by rereading on a worker. Parse errors show the Japanese
「名前変更の復旧記録」 dialog with these explicit choices:

- 「再読み込み」 rereads the journal on a worker. A current or legacy journal is
  merged ahead of locally retained work.
- 「壊れた記録を退避して再開」 verifies and moves aside only the exact invalid
  record described below.
- 「閉じる」 and Escape close an idle failure dialog while retaining the exact
  bytes and admission gate. During quarantine they request cancellation, but the
  modal and its pointer-input block remain until the worker reports a terminal
  result.

The dialog explains that a corrupt record can prevent automatic continuation of
settings and collection-reference migration for an unfinished rename. A
successful quarantine does not replay the blocked rename or any other physical
operation; the user repeats the desired operation after recovery completes.

## Secure quarantine boundary on Windows

Quarantine runs outside the UI thread. The worker opens the journal once with
read and delete access, sharing disabled, `FILE_FLAG_WRITE_THROUGH`, and
`FILE_FLAG_OPEN_REPARSE_POINT`. It then uses that same handle to:

1. read the current bytes and compare them byte-for-byte with the confirmed parse
   failure;
2. recheck both current and legacy formats;
3. rename the still-open file with `SetFileInformationByHandle(FileRenameInfo)`.

The destination is an absolute UTF-16 name in the same directory, with an
`.invalid-<unique>` suffix. `ReplaceIfExists` is false. A collision selects a new
name and never overwrites an existing file. A path-based verify-then-rename is
not used, so another process cannot swap the source between verification and the
namespace change.

If the source disappeared, became a valid current/legacy journal, or changed to
different invalid bytes, the quarantine action adopts none of it. The App keeps
the originally confirmed parse failure and gate, asks the user to choose
「再読み込み」, and only that separate action reads and adopts the latest missing,
valid, or invalid state. Open, read, comparison, and rename failures keep the
original pathname protected.

Cancellation and rename commitment share one atomic `Running -> Cancelled` or
`Running -> Renaming` transition. If cancellation wins, the handle rename is
never called and the original file remains. If the worker claims `Renaming`, the
UI keeps the modal open until the Win32 call completes. A failed call leaves the
source protected; successful `SetFileInformationByHandle` is the namespace
linearization point and remains success even if cancellation is requested later.

After a successful quarantine, the old prior is treated as empty. The App merges
its in-flight job, queued jobs, boot-retry jobs, and deferred successful-delete
invalidations into one complete snapshot. Admission remains closed in the
save-waiting state until the journal writer acknowledges that snapshot. This is
also true when the complete snapshot is empty. At shutdown, an active quarantine
worker is cancelled and joined; a rename that already linearized is integrated
as success before the final journal flush. Remaining recovery failure is written
to the log while the original journal remains in place; shutdown does not show a
new blocking warning dialog.

## Physical operation boundary

The admission check runs before a Shell rename/delete worker and before releasing
a viewer for deletion. The same boundary covers book rename, book delete,
reorder flush, and transfer Move and Copy. Copy is included because it may first
commit source page numbering through a physical rename. Book append/create/list
and viewing remain available.

Delete completion invalidates queued generic migration stages by the exact paths
that were successfully removed. The collection stage remains because it carries
the renamed source reference. A pending Shell or book path mutation holds the
generic queue head until App consumes the worker result. An already-running
generic worker remains subject to its existing non-cancellable limitation.

The Shell rename request captures its `Exact`/`Tree` scope from the typed
`GridItem` producer. The UI no longer calls `Path::is_file`: a missing image is
still `Exact`, and a folder is `Tree`. Clearing the dialog or polling an empty
receiver cannot change the captured scope.

## Verification matrix

- Exact invalid bytes move once; a restart sees no source journal. Changed bytes
  are detected even when length and modification time are unchanged.
- Missing, current, legacy, and differently invalid source changes remain blocked
  until a separate explicit reload. Destination collision never overwrites;
  read/open/rename failure and cancellation before the atomic rename claim
  preserve the source. Cancellation after that claim cannot demote the terminal
  rename result.
- Closing the parse dialog retains exact bytes and the gate. Read failures cannot
  enter the parse-quarantine action. The real modal blocks background pointer
  actions; Escape and close during quarantine retain that block until terminal.
- Quarantine success publishes in-flight, queued, boot-retry, and deferred-delete
  state, then waits for the full-snapshot save ACK before reopening admission.
- A physical rename rejected by the recovery gate is not replayed after
  quarantine.
- Grid rename derives scope from `GridItem`, including missing image and folder
  paths, without UI-thread filesystem I/O.

Focused verification on 2026-09-21:

- `cargo test --lib rename_key_migration::tests -- --nocapture`: 39 passed.
- `cargo test --lib recovery -- --nocapture`: 18 passed.
- `cargo test --lib quarantine_ -- --nocapture`: 12 passed.
- Direct read-failure and missing-grid-item producer regressions: passed.

No normal-profile application launch, real-data edit, or interactive input was
part of this verification.

The existing persistence-write failure path still accepts a new filesystem
operation before its resulting migration intent has a durable ACK, then holds the
migration and performs bounded save retries. This document covers the distinct
unreadable-prior-record hazard; a durable-before-filesystem transaction for an
ordinary write failure remains separate work.
