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
        identity: BookFileIdentity,
    },
    /// Move a page. Cross-volume moves converge through copy + delete.
    MoveFile {
        from: PathBuf,
        to: PathBuf,
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
            // Rename/create are atomic: an error proves that step did not
            // apply. Copy/move can fail after create-new bytes or the copy
            // half of a cross-volume move and must include the current step.
            let affected_steps = if matches!(
                step,
                BookFsStep::CopyFile { .. } | BookFsStep::MoveFile { .. }
            ) {
                idx + 1
            } else {
                idx
            };
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
    fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error>;
    fn copy_create_new(&self, from: &Path, to: &Path) -> Result<(), String>;
    fn remove_file(&self, path: &Path) -> Result<(), String>;
    fn remove_dir(&self, path: &Path) -> Result<(), String>;
    fn identity(&self, path: &Path) -> Result<BookFileIdentity, String>;
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

    fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
        fs::rename(from, to)
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
}

fn apply_step<O: BookFileOps>(ops: &O, step: &BookFsStep) -> Result<(), String> {
    match step {
        BookFsStep::CreateDir { path } => match ops.state(path)? {
            PathState::Missing => ops.create_dir(path),
            PathState::Directory => Ok(()),
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
            (PathState::Missing, _) => Ok(()),
            (_, PathState::Missing) => ops
                .rename(from, to)
                .map_err(|error| format!("{} -> {}: {error}", from.display(), to.display())),
            _ => Err(format!(
                "rename source and destination both exist: {} -> {}",
                from.display(),
                to.display()
            )),
        },
        BookFsStep::CopyFile { from, to, identity } => apply_copy(ops, from, to, identity),
        BookFsStep::MoveFile { from, to, identity } => apply_move(ops, from, to, identity),
    }
}

fn rollback_step<O: BookFileOps>(ops: &O, step: &BookFsStep) -> Result<(), String> {
    match step {
        BookFsStep::CreateDir { path } => match ops.state(path)? {
            PathState::Missing => Ok(()),
            PathState::Directory => ops.remove_dir(path),
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
            (_, PathState::Missing) => Ok(()),
            (PathState::Missing, _) => ops
                .rename(to, from)
                .map_err(|error| format!("{} -> {}: {error}", to.display(), from.display())),
            _ => Err(format!(
                "rollback rename source and destination both exist: {} <- {}",
                from.display(),
                to.display()
            )),
        },
        BookFsStep::CopyFile { from, to, identity } => rollback_copy(ops, from, to, identity),
        BookFsStep::MoveFile { from, to, identity } => rollback_move(ops, from, to, identity),
    }
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

fn apply_copy<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    if ops.state(from)? != PathState::File {
        return Err(format!("copy source is missing: {}", from.display()));
    }
    require_identity(ops, from, identity)?;
    match ops.state(to)? {
        PathState::Missing => {}
        PathState::File if ops.identity(to).as_ref() == Ok(identity) => return Ok(()),
        PathState::File => ops.remove_file(to)?, // interrupted partial copy
        _ => return Err(format!("copy destination is occupied: {}", to.display())),
    }
    ops.copy_create_new(from, to)?;
    require_identity(ops, to, identity)
}

fn rollback_copy<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    if ops.state(from)? != PathState::File {
        return Err(format!(
            "rollback copy source is missing: {}",
            from.display()
        ));
    }
    require_identity(ops, from, identity)?;
    match ops.state(to)? {
        PathState::Missing => Ok(()),
        PathState::File => ops.remove_file(to),
        _ => Err(format!(
            "rollback copy destination is occupied: {}",
            to.display()
        )),
    }
}

fn apply_move<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let from_state = ops.state(from)?;
    let to_state = ops.state(to)?;
    match (from_state, to_state) {
        (PathState::Missing, PathState::File) => require_identity(ops, to, identity),
        (PathState::File, PathState::File) => {
            require_identity(ops, from, identity)?;
            if ops.identity(to).as_ref() == Ok(identity) {
                ops.remove_file(from)
            } else {
                // A process crash can leave a partial create-new copy. The
                // source still proves the original identity, so retry safely.
                ops.remove_file(to)?;
                apply_move(ops, from, to, identity)
            }
        }
        (PathState::File, PathState::Missing) => {
            require_identity(ops, from, identity)?;
            match ops.rename(from, to) {
                Ok(()) => require_identity(ops, to, identity),
                Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                    ops.copy_create_new(from, to)?;
                    require_identity(ops, to, identity)?;
                    ops.remove_file(from)
                }
                Err(error) => Err(format!("{} -> {}: {error}", from.display(), to.display())),
            }
        }
        (PathState::Missing, PathState::Missing) => Err(format!(
            "move source and destination are both missing: {} -> {}",
            from.display(),
            to.display()
        )),
        _ => Err(format!(
            "move path is occupied: {} -> {}",
            from.display(),
            to.display()
        )),
    }
}

fn rollback_move<O: BookFileOps>(
    ops: &O,
    from: &Path,
    to: &Path,
    identity: &BookFileIdentity,
) -> Result<(), String> {
    let from_state = ops.state(from)?;
    let to_state = ops.state(to)?;
    match (from_state, to_state) {
        (PathState::File, PathState::Missing) => require_identity(ops, from, identity),
        (PathState::File, PathState::File) => {
            require_identity(ops, from, identity)?;
            // Destination is either the completed copy or an interrupted
            // partial copy; source identity proves it is safe to remove.
            ops.remove_file(to)
        }
        (PathState::Missing, PathState::File) => {
            require_identity(ops, to, identity)?;
            match ops.rename(to, from) {
                Ok(()) => require_identity(ops, from, identity),
                Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                    ops.copy_create_new(to, from)?;
                    require_identity(ops, from, identity)?;
                    ops.remove_file(to)
                }
                Err(error) => Err(format!("{} -> {}: {error}", to.display(), from.display())),
            }
        }
        (PathState::Missing, PathState::Missing) => Err(format!(
            "rollback move source and destination are both missing: {} <- {}",
            from.display(),
            to.display()
        )),
        _ => Err(format!(
            "rollback move path is occupied: {} <- {}",
            from.display(),
            to.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Clone, Copy)]
    enum Fault {
        Rename,
        Delete,
        CrossDeviceThenCopy,
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

        fn rename(&self, from: &Path, to: &Path) -> Result<(), std::io::Error> {
            match self.fault {
                Fault::Rename => {
                    self.failures.set(self.failures.get() + 1);
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected rename failure",
                    ))
                }
                Fault::CrossDeviceThenCopy => Err(std::io::Error::new(
                    std::io::ErrorKind::CrossesDevices,
                    "injected cross-device path",
                )),
                Fault::Delete => RealBookFileOps.rename(from, to),
            }
        }

        fn copy_create_new(&self, from: &Path, to: &Path) -> Result<(), String> {
            if matches!(self.fault, Fault::CrossDeviceThenCopy) {
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
    }

    fn file_step(move_file: bool) -> (tempfile::TempDir, BookFsOperationPlan) {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from.jpg");
        let to = temp.path().join("to.jpg");
        std::fs::write(&from, b"identity").unwrap();
        let identity = BookFileIdentity::read(&from).unwrap();
        let step = if move_file {
            BookFsStep::MoveFile { from, to, identity }
        } else {
            BookFsStep::CopyFile { from, to, identity }
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
        let copy_ops = FaultOps::new(Fault::CrossDeviceThenCopy);
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
