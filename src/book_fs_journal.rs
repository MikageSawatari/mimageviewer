//! Crash-safe filesystem operation plans for compiled-book mutations.
//!
//! The bookmark DB owns persistence and phase transitions. This module owns the
//! deterministic filesystem steps and their idempotent forward/rollback rules.
//! A persisted `next_step` is only a hint: every step also proves its state from
//! the source/destination paths and, for copied files, a SHA-256 identity.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BookFileIdentity {
    len: u64,
    sha256: [u8; 32],
}

impl BookFileIdentity {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        RealBookFileOps.identity(path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BookFsStep {
    /// The directory did not exist when the plan was prepared, so rollback may
    /// remove it after all later steps have been restored.
    CreateDir { path: PathBuf },
    /// Rename a file or a directory. Unique persisted temporary names make
    /// swap/cycle operations unambiguous after a crash.
    Rename { from: PathBuf, to: PathBuf },
    /// Copy a page while retaining the source.
    CopyFile {
        from: PathBuf,
        to: PathBuf,
        /// Sibling staging file reserved by this journal. `None` is accepted
        /// only so pre-fix journals can be retained with a diagnostic instead
        /// of being deserialized as an unsafe direct-to-destination copy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        staging: Option<PathBuf>,
        identity: BookFileIdentity,
    },
    /// Move a page through a durable sibling staging file. Rollback has its
    /// own staging file beside `from`, so it is also safe across volumes.
    MoveFile {
        from: PathBuf,
        to: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        staging: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rollback_staging: Option<PathBuf>,
        identity: BookFileIdentity,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BookFsOperationPlan {
    pub(crate) steps: Vec<BookFsStep>,
}

impl BookFsOperationPlan {
    pub(crate) fn new(steps: Vec<BookFsStep>) -> Self {
        Self { steps }
    }

    pub(crate) fn len(&self) -> usize {
        self.steps.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BookFsRunError {
    /// Prefix that may have been changed and must be considered by rollback.
    pub(crate) affected_steps: usize,
    pub(crate) message: String,
}

impl std::fmt::Display for BookFsRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Continue a plan from its durable progress marker. A crash between the
/// filesystem call and `record_progress` is safe because the step itself is
/// idempotent and proves whether it already happened.
pub(crate) fn execute_forward(
    plan: &BookFsOperationPlan,
    next_step: usize,
    mut record_progress: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), BookFsRunError> {
    execute_forward_with(&RealBookFileOps, plan, next_step, &mut record_progress)
}

/// Roll back the possibly-applied prefix `[0, remaining_steps)`. Progress moves
/// downward and is persisted after every proven inverse step.
pub(crate) fn execute_rollback(
    plan: &BookFsOperationPlan,
    remaining_steps: usize,
    mut record_progress: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), BookFsRunError> {
    execute_rollback_with(
        &RealBookFileOps,
        plan,
        remaining_steps,
        &mut record_progress,
    )
}

fn execute_forward_with<O: BookFileOps>(
    ops: &O,
    plan: &BookFsOperationPlan,
    next_step: usize,
    record_progress: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(), BookFsRunError> {
    if next_step > plan.steps.len() {
        return Err(BookFsRunError {
            affected_steps: plan.steps.len(),
            message: format!(
                "filesystem journal progress is out of range: {next_step} > {}",
                plan.steps.len()
            ),
        });
    }
    for (idx, step) in plan.steps.iter().enumerate().skip(next_step) {
        if let Err(error) = apply_step(ops, step) {
            // Every step can fail after its namespace mutation while flushing
            // the parent directory. Include the current step so rollback (or
            // startup recovery) proves the actual paths before deciding what
            // remains to be undone.
            let affected_steps = idx + 1;
            return Err(BookFsRunError {
                affected_steps,
                message: format!("filesystem journal step {idx} failed: {error}"),
            });
        }
        if let Err(error) = record_progress(idx + 1) {
            return Err(BookFsRunError {
                affected_steps: idx + 1,
                message: format!("filesystem journal progress {idx} failed: {error}"),
            });
        }
    }
    Ok(())
}

fn execute_rollback_with<O: BookFileOps>(
    ops: &O,
    plan: &BookFsOperationPlan,
    remaining_steps: usize,
    record_progress: &mut impl FnMut(usize) -> Result<(), String>,
) -> Result<(), BookFsRunError> {
    if remaining_steps > plan.steps.len() {
        return Err(BookFsRunError {
            affected_steps: remaining_steps,
            message: format!(
                "filesystem rollback progress is out of range: {remaining_steps} > {}",
                plan.steps.len()
            ),
        });
    }
    let mut failures = Vec::new();
    let mut progress_blocked = false;
    let mut durable_remaining = remaining_steps;
    for idx in (0..remaining_steps).rev() {
        match rollback_step(ops, &plan.steps[idx]) {
            Ok(()) if !progress_blocked => match record_progress(idx) {
                Ok(()) => durable_remaining = idx,
                Err(error) => {
                    failures.push(format!("progress {idx}: {error}"));
                    progress_blocked = true;
                }
            },
            Ok(()) => {
                // Keep trying lower inverse steps to restore as much as
                // possible, but do not move the contiguous durable marker past
                // an earlier failure. Recovery rechecks those steps idempotently.
            }
            Err(error) => {
                failures.push(format!("step {idx}: {error}"));
                progress_blocked = true;
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(BookFsRunError {
            affected_steps: durable_remaining,
            message: format!(
                "filesystem journal rollback failed: {}",
                failures.join("; ")
            ),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathState {
    Missing,
    File,
    Directory,
    Other,
}

trait BookFileOps {
    fn state(&self, path: &Path) -> Result<PathState, String>;
    fn create_dir(&self, path: &Path) -> Result<(), String>;
    fn rename_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error>;
    fn publish_file_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error>;
    fn copy_create_new(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn remove_file(&self, path: &Path) -> Result<(), String>;
    fn remove_dir(&self, path: &Path) -> Result<(), String>;
    fn identity(&self, path: &Path) -> Result<BookFileIdentity, String>;
    fn sync_dir(&self, path: &Path) -> Result<(), String>;
}

struct RealBookFileOps;

impl BookFileOps for RealBookFileOps {
    fn state(&self, path: &Path) -> Result<PathState, String> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_file() => Ok(PathState::File),
            Ok(meta) if meta.file_type().is_dir() => Ok(PathState::Directory),
            Ok(_) => Ok(PathState::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PathState::Missing),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    fn create_dir(&self, path: &Path) -> Result<(), String> {
        fs::create_dir(path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn rename_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
        rename_no_replace(from, to)
    }

    fn publish_file_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
        publish_file_no_replace(from, to)
    }

    fn copy_create_new(&self, from: &Path, to: &Path) -> Result<(), String> {
        let mut input =
            fs::File::open(from).map_err(|error| format!("{}: {error}", from.display()))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)
            .map_err(|error| format!("{}: {error}", to.display()))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("{}: {error}", to.display()))?;
        output
            .flush()
            .and_then(|_| output.sync_all())
            .map_err(|error| format!("{}: {error}", to.display()))
    }

    fn remove_file(&self, path: &Path) -> Result<(), String> {
        fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn remove_dir(&self, path: &Path) -> Result<(), String> {
        fs::remove_dir(path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn identity(&self, path: &Path) -> Result<BookFileIdentity, String> {
        let mut input =
            fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut len = 0u64;
        let mut buffer = [0u8; 128 * 1024];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            len = len.saturating_add(read as u64);
            hasher.update(&buffer[..read]);
        }
        Ok(BookFileIdentity {
            len,
            sha256: hasher.finalize().into(),
        })
    }

    fn sync_dir(&self, path: &Path) -> Result<(), String> {
        sync_directory(path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

/// Windows needs `FILE_FLAG_BACKUP_SEMANTICS` to open a directory. A write
/// handle is intentional: `FlushFileBuffers` rejects a read-only directory
/// handle, while the namespace mutation already proves write permission on the
/// parent. On Unix, `File::sync_all` on the directory is the fsync barrier.
#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

    OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?
        .sync_all()
}

#[cfg(not(windows))]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

/// Journal renames never replace an existing destination. On Windows,
/// `MOVEFILE_WRITE_THROUGH` also asks the filesystem to complete the move on
/// disk before returning; parent-directory sync below remains the common
/// commit barrier for create/rename/delete recovery.
#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows::core::PCWSTR;

    let from_wide = from
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let to_wide = to.as_os_str().encode_wide().chain([0]).collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(from_wide.as_ptr()),
            PCWSTR(to_wide.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    // The shipping platform uses the atomic Windows implementation above.
    // Preserve the pre-existing POSIX behavior for directory rename steps;
    // final file publication uses the no-clobber hard-link path below.
    fs::rename(from, to)
}

#[cfg(windows)]
fn publish_file_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    rename_no_replace(from, to)
}

#[cfg(not(windows))]
fn publish_file_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    // link(2) is atomic and fails with EEXIST instead of replacing `to`.
    // Removing the journal-owned staging link afterwards completes the move.
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

fn apply_step<O: BookFileOps>(ops: &O, step: &BookFsStep) -> Result<(), String> {
    match step {
        BookFsStep::CreateDir { path } => match ops.state(path)? {
            PathState::Missing => {
                ops.create_dir(path)?;
                sync_parent(ops, path)
            }
            PathState::Directory => sync_parent(ops, path),
            _ => Err(format!(
                "directory destination is occupied: {}",
                path.display()
            )),
        },
        BookFsStep::Rename { from, to } => match (ops.state(from)?, ops.state(to)?) {
            (PathState::Missing, PathState::Missing) => Err(format!(
                "rename source and destination are both missing: {} -> {}",
                from.display(),
                to.display()
            )),
            (PathState::Missing, _) => sync_rename_parents(ops, from, to),
            (_, PathState::Missing) => {
                ops.rename_no_replace(from, to)
                    .map_err(|error| format!("{} -> {}: {error}", from.display(), to.display()))?;
                sync_rename_parents(ops, from, to)
            }
            _ => Err(format!(
                "rename source and destination both exist: {} -> {}",
                from.display(),
                to.display()
            )),
        },
        BookFsStep::CopyFile {
            from,
            to,
            staging,
            identity,
        } => apply_copy(ops, from, to, staging.as_deref(), identity),
        BookFsStep::MoveFile {
            from,
            to,
            staging,
            rollback_staging,
            identity,
        } => apply_move(
            ops,
            from,
            to,
            staging.as_deref(),
            rollback_staging.as_deref(),
            identity,
        ),
    }
}

fn rollback_step<O: BookFileOps>(ops: &O, step: &BookFsStep) -> Result<(), String> {
    match step {
        BookFsStep::CreateDir { path } => match ops.state(path)? {
            PathState::Missing => sync_parent(ops, path),
            PathState::Directory => {
                ops.remove_dir(path)?;
                sync_parent(ops, path)
            }
            _ => Err(format!(
                "rollback directory path is occupied: {}",
                path.display()
            )),
        },
        BookFsStep::Rename { from, to } => match (ops.state(from)?, ops.state(to)?) {
            (PathState::Missing, PathState::Missing) => Err(format!(
                "rollback rename source and destination are both missing: {} <- {}",
                from.display(),
                to.display()
            )),
            (_, PathState::Missing) => sync_rename_parents(ops, from, to),
            (PathState::Missing, _) => {
                ops.rename_no_replace(to, from)
                    .map_err(|error| format!("{} -> {}: {error}", to.display(), from.display()))?;
                sync_rename_parents(ops, from, to)
            }
            _ => Err(format!(
                "rollback rename source and destination both exist: {} <- {}",
                from.display(),
                to.display()
            )),
        },
        BookFsStep::CopyFile {
            from,
            to,
            staging,
            identity,
        } => rollback_copy(ops, from, to, staging.as_deref(), identity),
        BookFsStep::MoveFile {
            from,
            to,
            staging,
            rollback_staging,
            identity,
        } => rollback_move(
            ops,
            from,
            to,
            staging.as_deref(),
            rollback_staging.as_deref(),
            identity,
        ),
    }
}

fn sync_parent<O: BookFileOps>(ops: &O, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("filesystem path has no parent: {}", path.display()))?;
    ops.sync_dir(parent).map_err(|error| {
        format!(
            "parent directory sync failed for {}: {error}",
            path.display()
        )
    })
}

fn sync_rename_parents<O: BookFileOps>(ops: &O, from: &Path, to: &Path) -> Result<(), String> {
    sync_parent(ops, from)?;
    if from.parent() != to.parent() {
        sync_parent(ops, to)?;
    }
    Ok(())
}

fn require_identity<O: BookFileOps>(
    ops: &O,
    path: &Path,
    expected: &BookFileIdentity,
) -> Result<(), String> {
    let actual = ops.identity(path)?;
    if &actual == expected {
        Ok(())
    } else {
        Err(format!("file identity changed: {}", path.display()))
    }
}

fn expected_file_present<O: BookFileOps>(
    ops: &O,
    path: &Path,
    expected: &BookFileIdentity,
    role: &str,
) -> Result<bool, String> {
    match ops.state(path)? {
        PathState::Missing => Ok(false),
        PathState::File if ops.identity(path).as_ref() == Ok(expected) => Ok(true),
        PathState::File => Err(format!(
            "{role} identity differs; refusing to replace or delete unrelated file: {}",
            path.display()
        )),
        _ => Err(format!(
            "{role} is occupied by a non-file: {}",
            path.display()
        )),
    }
}

fn required_staging<'a>(
    staging: Option<&'a Path>,
    anchor: &Path,
    role: &str,
) -> Result<&'a Path, String> {
    let staging = staging.ok_or_else(|| {
        format!("legacy {role} plan has no journal staging path; retained for manual recovery")
    })?;
    let valid_name = staging
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".miv-book-op-"));
    if staging == anchor || staging.parent() != anchor.parent() || !valid_name {
        return Err(format!(
            "invalid {role} journal staging path: {} (anchor {})",
            staging.display(),
            anchor.display()
        ));
    }
    Ok(staging)
}

fn prepare_staging<O: BookFileOps>(
    ops: &O,
    source: &Path,
    staging: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    require_identity(ops, source, identity)?;
    match ops.state(staging)? {
        PathState::Missing => {}
        PathState::File if ops.identity(staging).as_ref() == Ok(identity) => {
            // Recovery may observe the create-new copy after file sync but
            // before the parent-directory barrier.
            return sync_parent(ops, staging);
        }
        PathState::File => {
            // Only the UUID-namespaced journal staging path may contain a
            // partial copy. It is never the user-visible final destination.
            ops.remove_file(staging)?;
            sync_parent(ops, staging)?;
        }
        _ => {
            return Err(format!(
                "journal staging path is occupied by a non-file: {}",
                staging.display()
            ));
        }
    }
    ops.copy_create_new(source, staging)?;
    require_identity(ops, staging, identity)?;
    sync_parent(ops, staging)
}

fn cleanup_owned_staging<O: BookFileOps>(ops: &O, staging: &Path) -> Result<(), String> {
    match ops.state(staging)? {
        PathState::Missing => sync_parent(ops, staging),
        PathState::File => {
            ops.remove_file(staging)?;
            sync_parent(ops, staging)
        }
        _ => Err(format!(
            "journal staging path is occupied by a non-file: {}",
            staging.display()
        )),
    }
}

fn publish_staging<O: BookFileOps>(
    ops: &O,
    staging: &Path,
    to: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    // The final path was checked immediately before this call, but only the OS
    // no-clobber primitive closes the race with another process.
    ops.publish_file_no_replace(staging, to)
        .map_err(|error| format!("{} -> {}: {error}", staging.display(), to.display()))?;
    sync_rename_parents(ops, staging, to)?;
    require_identity(ops, to, identity)
}

fn remove_expected_file<O: BookFileOps>(
    ops: &O,
    path: &Path,
    identity: &BookFileIdentity,
    role: &str,
) -> Result<(), String> {
    if !expected_file_present(ops, path, identity, role)? {
        return sync_parent(ops, path);
    }
    // Recheck immediately before deletion so an already-visible external
    // replacement is never classified as this journal's output.
    require_identity(ops, path, identity)?;
    ops.remove_file(path)?;
    sync_parent(ops, path)
}

fn apply_copy<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    staging: Option<&Path>,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let staging = required_staging(staging, to, "copy")?;
    if expected_file_present(ops, to, identity, "copy destination")? {
        sync_parent(ops, to)?;
        cleanup_owned_staging(ops, staging)?;
        return Ok(());
    }
    if !expected_file_present(ops, from, identity, "copy source")? {
        return Err(format!("copy source is missing: {}", from.display()));
    }
    prepare_staging(ops, from, staging, identity)?;
    publish_staging(ops, staging, to, identity)
}

fn rollback_copy<O: BookFileOps>(
    ops: &O,
    _from: &Path,
    to: &Path,
    staging: Option<&Path>,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let staging = required_staging(staging, to, "copy rollback")?;
    // Validate the user-visible final path before touching even journal-owned
    // staging. A conflict must remain byte-for-byte intact with a diagnostic.
    let final_present = expected_file_present(ops, to, identity, "rollback copy destination")?;
    if !matches!(ops.state(staging)?, PathState::Missing | PathState::File) {
        return Err(format!(
            "journal staging path is occupied by a non-file: {}",
            staging.display()
        ));
    }
    if final_present {
        remove_expected_file(ops, to, identity, "rollback copy destination")?;
    } else {
        sync_parent(ops, to)?;
    }
    cleanup_owned_staging(ops, staging)
}

fn apply_move<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    staging: Option<&Path>,
    rollback_staging: Option<&Path>,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let staging = required_staging(staging, to, "move")?;
    // Validate the reverse path up front as well. A malformed legacy/tampered
    // journal must stop before forward mutation, not discover this on rollback.
    let _ = required_staging(rollback_staging, from, "move rollback")?;
    let final_present = expected_file_present(ops, to, identity, "move destination")?;
    let source_present = expected_file_present(ops, from, identity, "move source")?;
    if final_present {
        cleanup_owned_staging(ops, staging)?;
        sync_parent(ops, to)?;
        if source_present {
            remove_expected_file(ops, from, identity, "move source")?;
        } else {
            sync_parent(ops, from)?;
        }
        return Ok(());
    }

    if source_present {
        prepare_staging(ops, from, staging, identity)?;
    } else if expected_file_present(ops, staging, identity, "move staging")? {
        sync_parent(ops, staging)?;
    } else {
        return Err(format!(
            "move source, staging, and destination are all missing: {} -> {}",
            from.display(),
            to.display()
        ));
    }
    publish_staging(ops, staging, to, identity)?;
    if source_present {
        remove_expected_file(ops, from, identity, "move source")?;
    } else {
        sync_parent(ops, from)?;
    }
    Ok(())
}

fn rollback_move<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    staging: Option<&Path>,
    rollback_staging: Option<&Path>,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let staging = required_staging(staging, to, "move")?;
    let rollback_staging = required_staging(rollback_staging, from, "move rollback")?;

    // The final destination is the only path that may have been replaced by a
    // user after the crash. Validate it before any cleanup and stop on a
    // mismatch; never infer ownership merely because the original source is
    // present.
    let final_present = expected_file_present(ops, to, identity, "rollback move destination")?;
    let source_present = expected_file_present(ops, from, identity, "rollback move source")?;
    for path in [staging, rollback_staging] {
        if !matches!(ops.state(path)?, PathState::Missing | PathState::File) {
            return Err(format!(
                "journal staging path is occupied by a non-file: {}",
                path.display()
            ));
        }
    }

    if !source_present {
        let rollback_ready = matches!(ops.state(rollback_staging)?, PathState::File)
            && ops.identity(rollback_staging).as_ref() == Ok(identity);
        if !rollback_ready {
            let restore_source = if final_present {
                to
            } else if matches!(ops.state(staging)?, PathState::File)
                && ops.identity(staging).as_ref() == Ok(identity)
            {
                staging
            } else {
                return Err(format!(
                    "rollback move has no intact journal-owned copy: {} <- {}",
                    from.display(),
                    to.display()
                ));
            };
            prepare_staging(ops, restore_source, rollback_staging, identity)?;
        } else {
            sync_parent(ops, rollback_staging)?;
        }
        publish_staging(ops, rollback_staging, from, identity)?;
    } else {
        sync_parent(ops, from)?;
        cleanup_owned_staging(ops, rollback_staging)?;
    }

    if final_present {
        remove_expected_file(ops, to, identity, "rollback move destination")?;
    } else {
        sync_parent(ops, to)?;
    }
    cleanup_owned_staging(ops, staging)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Clone, Copy)]
    enum Fault {
        Rename,
        Delete,
        Copy,
        Sync,
    }

    struct FaultOps {
        fault: Fault,
        failures: Cell<usize>,
    }

    impl FaultOps {
        fn new(fault: Fault) -> Self {
            Self {
                fault,
                failures: Cell::new(0),
            }
        }

        fn fail(&self, message: &str) -> String {
            self.failures.set(self.failures.get() + 1);
            message.to_string()
        }
    }

    impl BookFileOps for FaultOps {
        fn state(&self, path: &Path) -> Result<PathState, String> {
            RealBookFileOps.state(path)
        }

        fn create_dir(&self, path: &Path) -> Result<(), String> {
            RealBookFileOps.create_dir(path)
        }

        fn rename_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
            match self.fault {
                Fault::Rename => {
                    self.failures.set(self.failures.get() + 1);
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected rename failure",
                    ))
                }
                _ => RealBookFileOps.rename_no_replace(from, to),
            }
        }

        fn publish_file_no_replace(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
            self.rename_no_replace(from, to)
        }

        fn copy_create_new(&self, from: &Path, to: &Path) -> Result<(), String> {
            if matches!(self.fault, Fault::Copy) {
                Err(self.fail("injected copy failure"))
            } else {
                RealBookFileOps.copy_create_new(from, to)
            }
        }

        fn remove_file(&self, path: &Path) -> Result<(), String> {
            if matches!(self.fault, Fault::Delete) {
                Err(self.fail("injected delete failure"))
            } else {
                RealBookFileOps.remove_file(path)
            }
        }

        fn remove_dir(&self, path: &Path) -> Result<(), String> {
            RealBookFileOps.remove_dir(path)
        }

        fn identity(&self, path: &Path) -> Result<BookFileIdentity, String> {
            RealBookFileOps.identity(path)
        }

        fn sync_dir(&self, path: &Path) -> Result<(), String> {
            if matches!(self.fault, Fault::Sync) {
                Err(self.fail("injected directory sync failure"))
            } else {
                RealBookFileOps.sync_dir(path)
            }
        }
    }

    fn file_step(move_file: bool) -> (tempfile::TempDir, BookFsOperationPlan) {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from.jpg");
        let to = temp.path().join("to.jpg");
        let staging = temp.path().join(".miv-book-op-test-forward.tmp");
        let rollback_staging = temp.path().join(".miv-book-op-test-rollback.tmp");
        std::fs::write(&from, b"identity").unwrap();
        let identity = BookFileIdentity::read(&from).unwrap();
        let step = if move_file {
            BookFsStep::MoveFile {
                from,
                to,
                staging: Some(staging),
                rollback_staging: Some(rollback_staging),
                identity,
            }
        } else {
            BookFsStep::CopyFile {
                from,
                to,
                staging: Some(staging),
                identity,
            }
        };
        let plan = BookFsOperationPlan::new(vec![step]);
        execute_forward_with(&RealBookFileOps, &plan, 0, &mut |_| Ok(())).unwrap();
        (temp, plan)
    }

    #[test]
    fn transfer_rollback_propagates_rename_copy_and_delete_failures() {
        let (_temp, rename_plan) = file_step(true);
        let rename_ops = FaultOps::new(Fault::Rename);
        let error = execute_rollback_with(&rename_ops, &rename_plan, 1, &mut |_| Ok(()))
            .expect_err("rename rollback must fail");
        assert!(error.message.contains("injected rename failure"));

        let (_temp, copy_plan) = file_step(true);
        let copy_ops = FaultOps::new(Fault::Copy);
        let error = execute_rollback_with(&copy_ops, &copy_plan, 1, &mut |_| Ok(()))
            .expect_err("cross-device copy rollback must fail");
        assert!(error.message.contains("injected copy failure"));

        let (_temp, delete_plan) = file_step(false);
        let delete_ops = FaultOps::new(Fault::Delete);
        let error = execute_rollback_with(&delete_ops, &delete_plan, 1, &mut |_| Ok(()))
            .expect_err("copy destination delete rollback must fail");
        assert!(error.message.contains("injected delete failure"));
    }

    #[test]
    fn conflicting_final_file_is_never_deleted_by_forward_or_rollback() {
        for move_file in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let from = temp.path().join("from.jpg");
            let to = temp.path().join("to.jpg");
            let staging = temp.path().join(".miv-book-op-conflict-forward.tmp");
            let rollback_staging = temp.path().join(".miv-book-op-conflict-rollback.tmp");
            std::fs::write(&from, b"planned").unwrap();
            std::fs::write(&to, b"external").unwrap();
            let identity = BookFileIdentity::read(&from).unwrap();
            let step = if move_file {
                BookFsStep::MoveFile {
                    from: from.clone(),
                    to: to.clone(),
                    staging: Some(staging.clone()),
                    rollback_staging: Some(rollback_staging),
                    identity,
                }
            } else {
                BookFsStep::CopyFile {
                    from: from.clone(),
                    to: to.clone(),
                    staging: Some(staging.clone()),
                    identity,
                }
            };
            let plan = BookFsOperationPlan::new(vec![step]);
            let error = execute_forward_with(&RealBookFileOps, &plan, 0, &mut |_| Ok(()))
                .expect_err("external destination must block forward recovery");
            assert!(error.message.contains("unrelated file"));
            assert_eq!(std::fs::read(&to).unwrap(), b"external");
            assert_eq!(std::fs::read(&from).unwrap(), b"planned");
            assert!(!staging.exists());

            let error = execute_rollback_with(&RealBookFileOps, &plan, 1, &mut |_| Ok(()))
                .expect_err("external destination must block rollback recovery");
            assert!(error.message.contains("unrelated file"));
            assert_eq!(std::fs::read(&to).unwrap(), b"external");
            assert_eq!(std::fs::read(&from).unwrap(), b"planned");
        }

        for move_file in [false, true] {
            let (temp, plan) = file_step(move_file);
            let from = temp.path().join("from.jpg");
            let to = temp.path().join("to.jpg");
            std::fs::remove_file(&to).unwrap();
            std::fs::write(&to, b"external-after-crash").unwrap();

            let error = execute_rollback_with(&RealBookFileOps, &plan, 1, &mut |_| Ok(()))
                .expect_err("external replacement must block applied-step rollback");
            assert!(error.message.contains("unrelated file"));
            assert_eq!(std::fs::read(&to).unwrap(), b"external-after-crash");
            assert_eq!(from.exists(), !move_file);
        }
    }

    #[test]
    fn final_publication_uses_persisted_staging_and_never_exposes_partial_copy() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from.jpg");
        let to = temp.path().join("to.jpg");
        let staging = temp.path().join(".miv-book-op-partial-forward.tmp");
        std::fs::write(&from, b"complete-source").unwrap();
        std::fs::write(&staging, b"partial").unwrap();
        let identity = BookFileIdentity::read(&from).unwrap();
        let plan = BookFsOperationPlan::new(vec![BookFsStep::CopyFile {
            from,
            to: to.clone(),
            staging: Some(staging.clone()),
            identity,
        }]);

        execute_forward_with(&RealBookFileOps, &plan, 0, &mut |_| Ok(())).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"complete-source");
        assert!(!staging.exists());
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains(".miv-book-op-partial-forward.tmp"));
    }

    #[test]
    fn namespace_sync_failure_keeps_current_step_out_of_durable_progress() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("created");
        let plan = BookFsOperationPlan::new(vec![BookFsStep::CreateDir { path: path.clone() }]);
        let ops = FaultOps::new(Fault::Sync);
        let mut progress = Vec::new();
        let error = execute_forward_with(&ops, &plan, 0, &mut |next| {
            progress.push(next);
            Ok(())
        })
        .expect_err("directory barrier failure must block progress");
        assert!(error.message.contains("injected directory sync failure"));
        assert_eq!(error.affected_steps, 1);
        assert!(progress.is_empty());
        assert!(path.is_dir(), "mutation may precede the failed barrier");

        let rename_temp = tempfile::tempdir().unwrap();
        let from = rename_temp.path().join("from");
        let to = rename_temp.path().join("to");
        std::fs::write(&from, b"rename").unwrap();
        let rename_plan = BookFsOperationPlan::new(vec![BookFsStep::Rename {
            from: from.clone(),
            to: to.clone(),
        }]);
        let mut progress = Vec::new();
        execute_forward_with(&ops, &rename_plan, 0, &mut |next| {
            progress.push(next);
            Ok(())
        })
        .expect_err("rename barrier failure must block progress");
        assert!(progress.is_empty());
        assert!(!from.exists() && to.exists());

        let (copy_temp, copy_plan) = file_step(false);
        let copy_to = copy_temp.path().join("to.jpg");
        let mut progress = Vec::new();
        execute_rollback_with(&ops, &copy_plan, 1, &mut |next| {
            progress.push(next);
            Ok(())
        })
        .expect_err("remove-file barrier failure must block rollback progress");
        assert!(progress.is_empty());
        assert!(!copy_to.exists());

        let remove_dir_temp = tempfile::tempdir().unwrap();
        let created = remove_dir_temp.path().join("created");
        std::fs::create_dir(&created).unwrap();
        let remove_dir_plan = BookFsOperationPlan::new(vec![BookFsStep::CreateDir {
            path: created.clone(),
        }]);
        let mut progress = Vec::new();
        execute_rollback_with(&ops, &remove_dir_plan, 1, &mut |next| {
            progress.push(next);
            Ok(())
        })
        .expect_err("remove-dir barrier failure must block rollback progress");
        assert!(progress.is_empty());
        assert!(!created.exists());
    }

    #[test]
    fn reorder_rollback_aggregates_failures_and_keeps_persisted_temp_names() {
        for applied_steps in [2usize, 4usize] {
            let temp = tempfile::tempdir().unwrap();
            let a = temp.path().join("a.jpg");
            let b = temp.path().join("b.jpg");
            let temp_a = temp.path().join(".persisted-a.tmp");
            let temp_b = temp.path().join(".persisted-b.tmp");
            std::fs::write(&a, b"a").unwrap();
            std::fs::write(&b, b"b").unwrap();
            let plan = BookFsOperationPlan::new(vec![
                BookFsStep::Rename {
                    from: a.clone(),
                    to: temp_a.clone(),
                },
                BookFsStep::Rename {
                    from: b.clone(),
                    to: temp_b.clone(),
                },
                BookFsStep::Rename {
                    from: temp_a.clone(),
                    to: b,
                },
                BookFsStep::Rename {
                    from: temp_b.clone(),
                    to: a,
                },
            ]);
            let prefix = BookFsOperationPlan::new(plan.steps[..applied_steps].to_vec());
            execute_forward_with(&RealBookFileOps, &prefix, 0, &mut |_| Ok(())).unwrap();

            let fault_ops = FaultOps::new(Fault::Rename);
            let error = execute_rollback_with(&fault_ops, &plan, applied_steps, &mut |_| Ok(()))
                .expect_err("injected reorder rollback must fail");
            assert!(
                fault_ops.failures.get() >= 2,
                "all inverse steps should be attempted and aggregated"
            );
            assert!(error.message.matches("step ").count() >= 2);
            let json = serde_json::to_string(&plan).unwrap();
            assert!(json.contains(".persisted-a.tmp"));
            assert!(json.contains(".persisted-b.tmp"));
        }
    }
}
