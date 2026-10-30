//! Previous GUI-run evidence. Only the admitted main owner may request a clean
//! terminal; dropping an App, owner, or unwinding never manufactures one.
//! All file operations and report formatting belong to the dedicated writer.
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SCHEMA: u32 = 1;
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_REFERENCE_BYTES: u64 = 1024;
const RUNNING: u8 = 0;
const RETURNED_OK: u8 = 1;
const RETURNED_ERROR: u8 = 2;
const ABANDONED: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiRunOutcome {
    ReturnedOk,
    ReturnedError,
}

#[derive(Clone, Debug)]
pub struct PreviousRunReport {
    body: String,
}
impl PreviousRunReport {
    /// The exact same allowlisted text is displayed and explicitly copied.
    pub fn body(&self) -> &str {
        &self.body
    }
}

struct Shared {
    state: AtomicU8,
    writer: OnceLock<std::thread::Thread>,
    report: OnceLock<PreviousRunReport>,
    report_taken: AtomicBool,
    initialized: AtomicBool,
    repaint: OnceLock<egui::Context>,
    completion: AtomicU8,
    #[cfg(test)]
    ready: (std::sync::Mutex<bool>, std::sync::Condvar),
}
impl Shared {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            writer: OnceLock::new(),
            report: OnceLock::new(),
            report_taken: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            repaint: OnceLock::new(),
            completion: AtomicU8::new(0),
            #[cfg(test)]
            ready: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
        }
    }
    fn wake(&self) {
        if let Some(writer) = self.writer.get() {
            writer.unpark();
        }
    }
    fn initialized(&self) {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(test)]
        {
            *self.ready.0.lock().unwrap() = true;
            self.ready.1.notify_all();
        }
        if let Some(context) = self.repaint.get() {
            context.request_repaint();
        }
    }
}

#[derive(Clone)]
pub struct PreviousRunReportHandle(Arc<Shared>);
impl PreviousRunReportHandle {
    /// Register once, without file I/O or waiting for initialization. Either
    /// side of the registration/completion race requests the necessary repaint.
    pub fn set_repaint_context(&self, context: &egui::Context) {
        let _ = self.0.repaint.set(context.clone());
        if self.0.initialized.load(Ordering::Acquire) {
            context.request_repaint();
        }
    }

    /// Call only when a normal root can actually present the notification.
    pub fn take_report(&self) -> Option<PreviousRunReport> {
        if !self.0.initialized.load(Ordering::Acquire) {
            return None;
        }
        let report = self.0.report.get()?;
        if self.0.report_taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(report.clone())
    }
}

enum Owner {
    Inert,
    Admitted {
        shared: Arc<Shared>,
        completed: mpsc::Receiver<bool>,
    },
}

pub struct GuiRunDiagnostics {
    owner: Owner,
}
impl Default for GuiRunDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}
impl GuiRunDiagnostics {
    /// Inert construction is safe before parsing modes or single-instance
    /// admission: it reads, creates, rotates, and consumes no evidence.
    pub fn new() -> Self {
        Self {
            owner: Owner::Inert,
        }
    }

    /// Caller has resolved data_dir and holds first-instance admission in main.
    /// Excluded modes and activation-only processes must never call this.
    pub fn admit(&mut self, resolved_data_dir: &Path) {
        let Some(identity) = miv_startup::process_identity() else {
            return;
        };
        if identity.role != miv_startup::Role::Core {
            return;
        }
        let data_dir = resolved_data_dir.to_path_buf();
        self.admit_resolving(
            move || resolve_directory_with(&data_dir, miv_startup::gui_diagnostics_directory),
            RunIdentity::from(identity),
        );
    }

    #[cfg(test)]
    fn admit_at(&mut self, directory: PathBuf, identity: RunIdentity) {
        self.admit_resolving(move || Ok(directory), identity);
    }

    fn admit_resolving(
        &mut self,
        resolve: impl FnOnce() -> io::Result<PathBuf> + Send + 'static,
        identity: RunIdentity,
    ) {
        if !matches!(self.owner, Owner::Inert) {
            return;
        }
        let shared = Arc::new(Shared::new());
        let (completed_tx, completed_rx) = mpsc::channel();
        self.owner = Owner::Admitted {
            shared: shared.clone(),
            completed: completed_rx,
        };
        let worker_shared = shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("gui-run-diagnostics".into())
            .spawn(move || {
                let _ = worker_shared.writer.set(std::thread::current());
                let result =
                    resolve().and_then(|directory| writer(&directory, identity, &worker_shared));
                if let Err(error) = &result {
                    log::warn!("GUI run diagnostics unavailable: {error}");
                }
                worker_shared.initialized();
                worker_shared
                    .completion
                    .store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                let _ = completed_tx.send(result.is_ok());
            })
        {
            log::warn!("GUI run diagnostics worker unavailable: {error}");
            shared.completion.store(2, Ordering::Release);
            shared.initialized();
        }
    }

    pub fn report_handle(&self) -> Option<PreviousRunReportHandle> {
        match &self.owner {
            Owner::Inert => None,
            Owner::Admitted { shared, .. } => Some(PreviousRunReportHandle(shared.clone())),
        }
    }

    /// Main's last explicit boundary, using the deadline already spent by the
    /// other log writers. No join, retries, fresh budget, or UI-frame wait.
    pub fn finish_until(&mut self, outcome: GuiRunOutcome, deadline: Instant) -> bool {
        let Owner::Admitted { shared, completed } = &self.owner else {
            return true;
        };
        let terminal = match outcome {
            GuiRunOutcome::ReturnedOk => RETURNED_OK,
            GuiRunOutcome::ReturnedError => RETURNED_ERROR,
        };
        let _ =
            shared
                .state
                .compare_exchange(RUNNING, terminal, Ordering::AcqRel, Ordering::Acquire);
        shared.wake();
        match shared.completion.load(Ordering::Acquire) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        completed
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(false)
    }
}

fn resolve_directory_with(
    data_dir: &Path,
    select: impl FnOnce(&Path) -> Option<PathBuf>,
) -> io::Result<PathBuf> {
    // Match the single-instance namespace, including relative/absolute spelling,
    // Windows case, verbatim prefixes, and existing junction aliases. This may
    // canonicalize the filesystem and must only execute on the writer thread.
    let key = crate::single_instance::resolved_data_dir_key(data_dir);
    select(Path::new(&key)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "GUI diagnostic directory unavailable",
        )
    })
}
impl Drop for GuiRunDiagnostics {
    fn drop(&mut self) {
        if let Owner::Admitted { shared, .. } = &self.owner {
            // An abandoned run stays Running on disk. A prior explicit terminal
            // remains absorbing even if its I/O outlives main's deadline.
            let _ = shared.state.compare_exchange(
                RUNNING,
                ABANDONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            shared.wake();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIdentity {
    role: RunRole,
    run: u64,
    core_pid: u32,
    core_started_utc_ms: u64,
    core_started_ticks: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RunRole {
    Core,
}
impl From<miv_startup::ProcessIdentity> for RunIdentity {
    fn from(value: miv_startup::ProcessIdentity) -> Self {
        Self {
            role: RunRole::Core,
            run: value.run,
            core_pid: value.pid,
            core_started_utc_ms: value.started_utc_ms,
            core_started_ticks: value.started_ticks,
        }
    }
}
impl RunIdentity {
    fn name(self) -> String {
        format!(
            "{:016x}-core-{}-{}-{:016x}",
            self.run, self.core_pid, self.core_started_utc_ms, self.core_started_ticks
        )
    }
    fn valid(self) -> bool {
        self.core_pid != 0 && self.core_started_utc_ms != 0 && self.core_started_ticks != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RunState {
    Running,
    ReturnedOk,
    ReturnedError,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    identity: RunIdentity,
    version: [u16; 3],
    state: RunState,
    last_recorded_utc_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema: u32,
    current: RunIdentity,
    previous: Option<RunIdentity>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn version() -> [u16; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
    ]
}
fn bounded_read<T: serde::de::DeserializeOwned>(path: &Path, max: u64) -> io::Result<T> {
    let file = File::open(path)?;
    if file.metadata()?.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid diagnostic record"))
}
fn replace<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "diagnostic parent missing")
    })?)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.persist(path).map(|_| ()).map_err(|error| error.error)
}
fn open_lease(directory: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("alive.lock"))?;
    if !file.try_lock_exclusive()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "GUI diagnostic namespace is alive",
        ));
    }
    Ok(file)
}

fn read_reference(directory: &Path) -> Option<Reference> {
    let value: Reference =
        bounded_read(&directory.join("current.json"), MAX_REFERENCE_BYTES).ok()?;
    (value.schema == SCHEMA
        && value.current.valid()
        && value.previous.is_none_or(RunIdentity::valid))
    .then_some(value)
}
fn previous_report(directory: &Path, reference: &Reference) -> Option<PreviousRunReport> {
    let record: Record = bounded_read(
        &directory.join(reference.current.name()).join("state.json"),
        MAX_RECORD_BYTES,
    )
    .ok()?;
    if record.schema != SCHEMA
        || record.identity != reference.current
        || record.state != RunState::Running
        || record.last_recorded_utc_ms < record.identity.core_started_utc_ms
        || record.last_recorded_utc_ms > now_ms()
    {
        return None;
    }
    Some(format_report(&record))
}
fn format_report(record: &Record) -> PreviousRunReport {
    // Numeric schema alone is accepted. Neither arbitrary JSON fields nor
    // native/panic/logger text can reach the display or clipboard in stage A.
    let [major, minor, patch] = record.version;
    PreviousRunReport {
        body: format!(
            "前回の正常終了を確認できませんでした。原因は記録されていません。\n最終記録時刻: Unix {} ms (UTC) / 終了との間隔不明\nmIV {major}.{minor}.{patch} / 記録ID {}\n強制終了・OS終了・電源断・記録失敗とクラッシュは区別できません。\n最終記録以降に状態が変わった可能性があります。",
            record.last_recorded_utc_ms,
            record.identity.name()
        ),
    }
}

fn writer(directory: &Path, identity: RunIdentity, shared: &Shared) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    // Held across initialization and every outstanding I/O, including after a
    // main deadline. A newer run never competes with this writer's reference
    // publication or retention. Failure is logged once; no wait/retry.
    let _alive = open_lease(directory)?;
    let previous = read_reference(directory);
    if let Some(report) = previous
        .as_ref()
        .and_then(|reference| previous_report(directory, reference))
    {
        let _ = shared.report.set(report);
    }
    let run_directory = directory.join(identity.name());
    std::fs::create_dir(&run_directory)?;
    let path = run_directory.join("state.json");
    let mut record = Record {
        schema: SCHEMA,
        identity,
        version: version(),
        state: RunState::Running,
        last_recorded_utc_ms: now_ms(),
    };
    replace(&path, &record)?;
    // Initial publication must also happen when main requested a terminal
    // during initialization: otherwise an old Running reference survives a
    // successful short run. The alive lease excludes every newer writer until
    // this publication and the final private-state replacement have completed.
    // Initial reference publication and bounded retention form one startup
    // transaction even for a short run. Terminal processing never republishes
    // a reference or performs generation cleanup.
    let reference = Reference {
        schema: SCHEMA,
        current: identity,
        previous: previous.map(|r| r.current),
    };
    replace(&directory.join("current.json"), &reference)?;
    retain_two(directory, &reference)?;
    shared.initialized();
    loop {
        match shared.state.load(Ordering::Acquire) {
            RUNNING => std::thread::park(),
            ABANDONED => return Ok(()),
            terminal => {
                record.state = if terminal == RETURNED_OK {
                    RunState::ReturnedOk
                } else {
                    RunState::ReturnedError
                };
                record.last_recorded_utc_ms = now_ms();
                return replace(&path, &record);
            }
        }
    }
}

fn retain_two(directory: &Path, reference: &Reference) -> io::Result<()> {
    // Bounded enumeration; never recurse through unknown/reparse directories.
    // Only a schema-valid record determines a directory that we own.
    for entry in std::fs::read_dir(directory)?.take(64) {
        let entry = entry?;
        if !plain_directory(&entry.path())? {
            continue;
        }
        let path = entry.path();
        let Ok(record) = bounded_read::<Record>(&path.join("state.json"), MAX_RECORD_BYTES) else {
            continue;
        };
        if record.schema != SCHEMA
            || !record.identity.valid()
            || entry.file_name() != record.identity.name().as_str()
            || record.identity == reference.current
            || Some(record.identity) == reference.previous
        {
            continue;
        }
        std::fs::remove_file(path.join("state.json"))?;
        // No recursive deletion: unrecognized contents are never removed.
        let _ = std::fs::remove_dir(path);
    }
    Ok(())
}

fn plain_directory(path: &Path) -> io::Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Ok(false);
        }
    }
    Ok(metadata.is_dir() && !metadata.file_type().is_symlink())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;
    static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

    pub(super) fn identity() -> RunIdentity {
        RunIdentity {
            role: RunRole::Core,
            run: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
            core_pid: std::process::id(),
            core_started_utc_ms: now_ms(),
            core_started_ticks: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Isolated fixture only: no process-global startup identity or normal logs.
    pub(crate) fn admit(owner: &mut GuiRunDiagnostics, directory: &Path) {
        owner.admit_at(directory.to_path_buf(), identity());
        wait_ready(owner);
    }

    pub(super) fn wait_ready(owner: &GuiRunDiagnostics) {
        let Owner::Admitted { shared, .. } = &owner.owner else {
            panic!("fixture admission failed")
        };
        let ready = shared.ready.0.lock().unwrap();
        let (_ready, timeout) = shared
            .ready
            .1
            .wait_timeout_while(ready, Duration::from_secs(20), |ready| !*ready)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "diagnostic fixture did not initialize"
        );
        assert_ne!(
            shared.completion.load(Ordering::Acquire),
            2,
            "diagnostic fixture initialization failed"
        );
    }

    pub(crate) fn terminal(directory: &Path) -> Option<GuiRunOutcome> {
        match record(directory)?.state {
            RunState::ReturnedOk => Some(GuiRunOutcome::ReturnedOk),
            RunState::ReturnedError => Some(GuiRunOutcome::ReturnedError),
            RunState::Running => None,
        }
    }

    /// A ready immutable report for headless presentation/hidden-root tests.
    pub(crate) fn report_handle() -> PreviousRunReportHandle {
        let shared = Arc::new(Shared::new());
        let _ = shared.report.set(format_report(&Record {
            schema: SCHEMA,
            identity: identity(),
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
        }));
        shared.initialized();
        PreviousRunReportHandle(shared)
    }

    pub(super) fn record(directory: &Path) -> Option<Record> {
        let reference = read_reference(directory)?;
        bounded_read(
            &directory.join(reference.current.name()).join("state.json"),
            MAX_RECORD_BYTES,
        )
        .ok()
    }

    pub(super) fn wait_abandoned(mut owner: GuiRunDiagnostics) {
        let Owner::Admitted { completed, .. } = &mut owner.owner else {
            panic!("missing fixture")
        };
        let completed = std::mem::replace(completed, mpsc::channel().1);
        drop(owner);
        assert!(completed.recv_timeout(Duration::from_secs(20)).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn finish(owner: &mut GuiRunDiagnostics, outcome: GuiRunOutcome) {
        assert!(owner.finish_until(outcome, Instant::now() + Duration::from_secs(20)));
    }

    #[test]
    fn inert_owner_does_not_create_or_consume_any_evidence() {
        let mut owner = GuiRunDiagnostics::new();
        assert!(owner.report_handle().is_none());
        assert!(owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        assert!(owner.report_handle().is_none());
    }

    #[test]
    fn equivalent_data_dir_spellings_share_previous_run_and_resolve_on_worker() {
        use std::hash::{Hash, Hasher};
        let directory = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let data_dir = directory.path().join("FixtureData");
        std::fs::create_dir(&data_dir).unwrap();
        let absolute = std::fs::canonicalize(&data_dir).unwrap();
        // Use std::path::absolute (without a verbatim prefix) for strip_prefix.
        let plain_absolute = std::path::absolute(&data_dir).unwrap();
        let relative = plain_absolute
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap()
            .to_path_buf();
        let mut spellings = vec![absolute.clone(), absolute.join("."), relative];
        #[cfg(windows)]
        spellings.push(PathBuf::from(absolute.to_string_lossy().to_uppercase()));
        let caller = std::thread::current().id();
        for (index, spelling) in spellings.drain(..).enumerate() {
            let logs = directory.path().join("logs");
            let mut owner = GuiRunDiagnostics::new();
            owner.admit_resolving(
                move || {
                    assert_ne!(
                        std::thread::current().id(),
                        caller,
                        "directory canonicalization must stay off the caller/UI thread"
                    );
                    resolve_directory_with(&spelling, |key| {
                        let mut hash = std::collections::hash_map::DefaultHasher::new();
                        key.hash(&mut hash);
                        Some(logs.join(format!("{:016x}", hash.finish())))
                    })
                },
                test_support::identity(),
            );
            test_support::wait_ready(&owner);
            assert_eq!(
                owner.report_handle().unwrap().take_report().is_some(),
                index != 0
            );
            test_support::wait_abandoned(owner);
        }
    }

    #[test]
    fn normal_and_controlled_error_are_terminal_and_do_not_warn_next_time() {
        for outcome in [GuiRunOutcome::ReturnedOk, GuiRunOutcome::ReturnedError] {
            let directory = tempfile::tempdir().unwrap();
            let mut owner = GuiRunDiagnostics::new();
            test_support::admit(&mut owner, directory.path());
            assert!(owner.report_handle().unwrap().take_report().is_none());
            finish(&mut owner, outcome);
            assert_eq!(test_support::terminal(directory.path()), Some(outcome));
            let mut next = GuiRunDiagnostics::new();
            test_support::admit(&mut next, directory.path());
            assert!(next.report_handle().unwrap().take_report().is_none());
            finish(&mut next, GuiRunOutcome::ReturnedOk);
        }
    }

    #[test]
    fn drop_preserves_running_and_report_is_consumed_once_across_handles() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        test_support::wait_abandoned(owner);
        assert_eq!(
            test_support::record(directory.path()).unwrap().state,
            RunState::Running
        );
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        let handle = next.report_handle().unwrap();
        let other = handle.clone();
        let report = handle.take_report().unwrap();
        assert!(
            report
                .body()
                .contains("前回の正常終了を確認できませんでした")
        );
        assert!(other.take_report().is_none());
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn terminal_is_absorbing_and_no_late_running_replaces_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        finish(&mut owner, GuiRunOutcome::ReturnedError);
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedError)
        );
    }

    #[test]
    fn live_namespace_is_neither_consumed_nor_modified() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        let before = std::fs::read(directory.path().join("current.json")).unwrap();
        let shared = Shared::new();
        let error = writer(directory.path(), test_support::identity(), &shared).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(shared.report.get().is_none());
        assert_eq!(
            before,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn namespace_retains_exactly_current_and_previous_owned_run() {
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..4 {
            let mut owner = GuiRunDiagnostics::new();
            test_support::admit(&mut owner, directory.path());
            finish(&mut owner, GuiRunOutcome::ReturnedOk);
        }
        let reference = read_reference(directory.path()).unwrap();
        let run_directories: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.file_type().unwrap().is_dir())
            .collect();
        assert_eq!(run_directories.len(), 2);
        for entry in run_directories {
            assert!(
                entry.file_name() == reference.current.name().as_str()
                    || entry.file_name() == reference.previous.unwrap().name().as_str()
            );
        }
    }

    #[test]
    fn unknown_corrupt_oversized_and_identity_mismatched_records_are_not_reports() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let reference = Reference {
            schema: SCHEMA,
            current: identity,
            previous: None,
        };
        replace(&directory.path().join("current.json"), &reference).unwrap();
        let path = directory.path().join(identity.name()).join("state.json");
        for bytes in [
            b"not JSON".to_vec(),
            vec![b' '; MAX_RECORD_BYTES as usize + 1],
            serde_json::to_vec(&serde_json::json!({"schema":99,"identity":identity,"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema":1,"identity":test_support::identity(),"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema":1,"identity":identity,"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms(),"path":"C:/private/secret.png"})).unwrap(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(previous_report(directory.path(), &reference).is_none());
        }
        std::fs::write(directory.path().join("current.json"), b"{\"schema\":99}").unwrap();
        assert!(read_reference(directory.path()).is_none());
    }

    #[test]
    fn same_pid_and_run_with_a_different_start_is_not_the_same_identity() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let other = RunIdentity {
            core_started_ticks: identity.core_started_ticks + 1,
            ..identity
        };
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let record = Record {
            schema: SCHEMA,
            identity: other,
            version: [1, 2, 3],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
        };
        replace(
            &directory.path().join(identity.name()).join("state.json"),
            &record,
        )
        .unwrap();
        assert!(
            previous_report(
                directory.path(),
                &Reference {
                    schema: SCHEMA,
                    current: identity,
                    previous: None
                }
            )
            .is_none()
        );
    }

    #[test]
    fn impossible_record_time_is_not_adopted_and_old_time_has_no_freshness_claim() {
        let directory = tempfile::tempdir().unwrap();
        let identity = RunIdentity {
            core_started_utc_ms: 1000,
            ..test_support::identity()
        };
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let reference = Reference {
            schema: SCHEMA,
            current: identity,
            previous: None,
        };
        let path = directory.path().join(identity.name()).join("state.json");
        for time in [999, u64::MAX] {
            replace(
                &path,
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::Running,
                    last_recorded_utc_ms: time,
                },
            )
            .unwrap();
            assert!(previous_report(directory.path(), &reference).is_none());
        }
        replace(
            &path,
            &Record {
                schema: SCHEMA,
                identity,
                version: [1, 2, 3],
                state: RunState::Running,
                last_recorded_utc_ms: 1000,
            },
        )
        .unwrap();
        let report = previous_report(directory.path(), &reference).unwrap();
        assert!(report.body().contains("終了との間隔不明"));
        assert!(!report.body().contains("秒前"));
    }

    #[test]
    fn namespace_initialization_failure_does_not_retry_or_change_user_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-a-directory");
        std::fs::write(&path, b"preserve user data").unwrap();
        let shared = Shared::new();
        assert!(writer(&path, test_support::identity(), &shared).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"preserve user data");
    }

    #[test]
    fn deadline_does_not_join_or_release_a_still_writing_old_run() {
        // Block a writer at a real outstanding-I/O boundary, then finish main
        // with no budget. The next writer cannot acquire this old run's lease.
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker_shared = shared.clone();
        let root = directory.path().to_path_buf();
        std::thread::spawn(move || {
            let lease = open_lease(&root).unwrap();
            let run_directory = root.join(identity.name());
            std::fs::create_dir(&run_directory).unwrap();
            replace(
                &run_directory.join("state.json"),
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::Running,
                    last_recorded_utc_ms: now_ms(),
                },
            )
            .unwrap();
            replace(
                &root.join("current.json"),
                &Reference {
                    schema: SCHEMA,
                    current: identity,
                    previous: None,
                },
            )
            .unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert_eq!(worker_shared.state.load(Ordering::Acquire), RETURNED_OK);
            // Simulated completion writes only this run's private state.
            replace(
                &run_directory.join("state.json"),
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::ReturnedOk,
                    last_recorded_utc_ms: now_ms(),
                },
            )
            .unwrap();
            drop(lease);
            worker_shared.completion.store(1, Ordering::Release);
            completed_tx.send(true).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut owner = GuiRunDiagnostics {
            owner: Owner::Admitted {
                shared,
                completed: completed_rx,
            },
        };
        assert!(!owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        let before = std::fs::read(directory.path().join("current.json")).unwrap();
        let error = writer(directory.path(), test_support::identity(), &Shared::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            before,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        release_tx.send(()).unwrap();
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        let next_reference = std::fs::read(directory.path().join("current.json")).unwrap();
        drop(owner);
        assert_eq!(
            next_reference,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn terminal_requested_before_initialization_is_saved_last_and_published() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let shared = Shared::new();
        shared.state.store(RETURNED_ERROR, Ordering::Release);
        writer(directory.path(), identity, &shared).unwrap();
        let record: Record = bounded_read(
            &directory.path().join(identity.name()).join("state.json"),
            MAX_RECORD_BYTES,
        )
        .unwrap();
        assert_eq!(record.state, RunState::ReturnedError);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedError)
        );
    }

    #[test]
    fn consecutive_terminal_requests_during_initialization_still_retain_two_runs() {
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..5 {
            let shared = Shared::new();
            shared.state.store(RETURNED_ERROR, Ordering::Release);
            writer(directory.path(), test_support::identity(), &shared).unwrap();
            assert_eq!(
                test_support::terminal(directory.path()),
                Some(GuiRunOutcome::ReturnedError)
            );
        }
        let run_count = std::fs::read_dir(directory.path())
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .count();
        assert_eq!(run_count, 2);
    }

    #[test]
    fn finish_during_gated_initialization_replaces_old_running_reference() {
        let directory = tempfile::tempdir().unwrap();
        let mut previous = GuiRunDiagnostics::new();
        test_support::admit(&mut previous, directory.path());
        test_support::wait_abandoned(previous);
        let old = read_reference(directory.path()).unwrap().current;
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let worker_shared = shared.clone();
        let root = directory.path().to_path_buf();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = worker_shared.writer.set(std::thread::current());
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let result = writer(&root, identity, &worker_shared);
            worker_shared
                .completion
                .store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
            completed_tx.send(result.is_ok()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut owner = GuiRunDiagnostics {
            owner: Owner::Admitted {
                shared,
                completed: completed_rx,
            },
        };
        assert!(!owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        assert_eq!(read_reference(directory.path()).unwrap().current, old);
        release_tx.send(()).unwrap();
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        assert_eq!(read_reference(directory.path()).unwrap().current, identity);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedOk)
        );
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        assert!(next.report_handle().unwrap().take_report().is_none());
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn untrusted_details_cannot_enter_the_allowlisted_report() {
        let identity = test_support::identity();
        let record = Record {
            schema: SCHEMA,
            identity,
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
        };
        let report = format_report(&record);
        for forbidden in [
            "settings.db",
            "C:/",
            "\\Users\\",
            "panic.log",
            "native-exception.log",
            "backtrace",
            "secret.png",
            "search query",
        ] {
            assert!(!report.body().contains(forbidden));
        }
        assert!(report.body().len() < MAX_RECORD_BYTES as usize);
        assert!(serde_json::to_vec(&record).unwrap().len() < 1024);
    }

    #[test]
    fn diagnostic_child() {
        let Some(directory) = std::env::var_os("MIV_GUI_DIAGNOSTIC_CHILD_DIR") else {
            return;
        };
        let mode = std::env::var("MIV_GUI_DIAGNOSTIC_CHILD_MODE").unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, Path::new(&directory));
        println!("GUI_DIAGNOSTIC_READY");
        std::io::stdout().flush().unwrap();
        match mode.as_str() {
            "ok" => finish(&mut owner, GuiRunOutcome::ReturnedOk),
            "error" => finish(&mut owner, GuiRunOutcome::ReturnedError),
            "early-exit" => {
                finish(&mut owner, GuiRunOutcome::ReturnedError);
                std::process::exit(7);
            }
            "panic" => panic!("fixture main panic"),
            "worker-panic-ok" => {
                assert!(
                    std::thread::spawn(|| panic!("fixture worker panic"))
                        .join()
                        .is_err()
                );
                finish(&mut owner, GuiRunOutcome::ReturnedOk);
            }
            "kill" => loop {
                std::thread::park();
            },
            _ => panic!("unknown child fixture"),
        }
    }

    #[test]
    fn bounded_child_process_return_error_early_exit_panic_worker_panic_and_kill() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        for (mode, terminal) in [
            ("ok", Some(GuiRunOutcome::ReturnedOk)),
            ("error", Some(GuiRunOutcome::ReturnedError)),
            ("early-exit", Some(GuiRunOutcome::ReturnedError)),
            ("panic", None),
            ("worker-panic-ok", Some(GuiRunOutcome::ReturnedOk)),
            ("kill", None),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "previous_run_diagnostics::tests::diagnostic_child",
                    "--nocapture",
                ])
                .env("MIV_GUI_DIAGNOSTIC_CHILD_DIR", directory.path())
                .env("MIV_GUI_DIAGNOSTIC_CHILD_MODE", mode)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    if line.contains("GUI_DIAGNOSTIC_READY") {
                        let _ = ready_tx.send(());
                    }
                }
            });
            if ready_rx.recv_timeout(Duration::from_secs(20)).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not initialize for {mode}");
            }
            if mode == "kill" {
                child.kill().unwrap();
            }
            let wait_deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= wait_deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("child did not exit for {mode}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                status.success(),
                matches!(mode, "ok" | "error" | "worker-panic-ok"),
                "{mode}"
            );
            assert_eq!(test_support::terminal(directory.path()), terminal, "{mode}");
            let mut next = GuiRunDiagnostics::new();
            test_support::admit(&mut next, directory.path());
            assert_eq!(
                next.report_handle().unwrap().take_report().is_some(),
                terminal.is_none(),
                "{mode}"
            );
            finish(&mut next, GuiRunOutcome::ReturnedOk);
        }
    }
}
