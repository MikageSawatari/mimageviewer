//! One writable connection, ordered admission, immutable read publication, and
//! worker-only maintenance leases. The cloneable front handle never joins.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};

use crate::book_scope::BookScopeKey;
use crate::content_identity::{
    ContentIdentityProducer, ContentIdentityRecorder, ContentIdentityTrigger,
};

use super::{
    ShiftClearAllReply, ShiftCountReply, ShiftOperation, ShiftReadReply, ShiftWatermarks,
    ShiftWriteReply, SpreadShiftPageRef, store,
};

#[derive(Clone)]
struct Snapshot {
    completed_through: u64,
    maintenance_epoch: u64,
    rows: BTreeMap<String, SpreadShiftPageRef>,
    status: Result<(), String>,
}

impl Snapshot {
    fn read(&self, request: ReadRequest) -> ShiftReadReply {
        ShiftReadReply {
            cutoff: request.cutoff,
            completed_through: self.completed_through,
            maintenance_epoch: self.maintenance_epoch,
            row: self.rows.get(&request.scope.storage_key()).cloned(),
            status: self.status.clone(),
        }
    }

    fn count(&self, cutoff: u64) -> ShiftCountReply {
        ShiftCountReply {
            cutoff,
            completed_through: self.completed_through,
            maintenance_epoch: self.maintenance_epoch,
            count: self.rows.len(),
            status: self.status.clone(),
        }
    }
}

struct ReadRequest {
    scope: BookScopeKey,
    cutoff: u64,
    reply: mpsc::Sender<ShiftReadReply>,
}

struct CountRequest {
    cutoff: u64,
    reply: mpsc::Sender<ShiftCountReply>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionMode {
    Running,
    StartingAfterMaintenance,
    Maintenance,
    Closed,
    ClosedStartingAfterMaintenance,
    ClosedMaintenance,
}

impl AdmissionMode {
    fn is_closed(self) -> bool {
        matches!(
            self,
            Self::Closed | Self::ClosedStartingAfterMaintenance | Self::ClosedMaintenance
        )
    }

    fn close(self) -> Self {
        match self {
            Self::StartingAfterMaintenance => Self::ClosedStartingAfterMaintenance,
            Self::Maintenance => Self::ClosedMaintenance,
            Self::Running => Self::Closed,
            closed => closed,
        }
    }
}

struct Admission {
    tx: Option<mpsc::Sender<Command>>,
    mode: AdmissionMode,
    watermarks: ShiftWatermarks,
    snapshot: Option<Arc<Snapshot>>,
    deferred_commands: VecDeque<Command>,
    results: VecDeque<ShiftWriteReply>,
    clear_results: VecDeque<ShiftClearAllReply>,
    changed_scope_keys: Vec<String>,
    errors: Vec<String>,
}

struct Shared {
    admission: Mutex<Admission>,
    pending: AtomicUsize,
    #[cfg(test)]
    phase_changed: Condvar,
}

#[cfg(test)]
type StartupPause = Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>;
#[cfg(not(test))]
type StartupPause = ();

enum Mutation {
    Set(SpreadShiftPageRef),
    Clear,
}

enum MaintenanceAction {
    PagePaths(Vec<(PathBuf, PathBuf)>),
    RenameBookRoot { source: String, target: String },
    PurgeBookRoot(String),
}

enum Command {
    Write {
        sequence: u64,
        scope: BookScopeKey,
        operation: ShiftOperation,
        mutation: Mutation,
    },
    Read(ReadRequest),
    Count(CountRequest),
    ClearAll {
        sequence: u64,
    },
    Maintain {
        sequence: u64,
        action: MaintenanceAction,
    },
    BeginMaintenance(mpsc::Sender<Result<(), String>>),
    EndMaintenance(mpsc::Sender<Result<(), String>>),
    #[cfg(test)]
    Pause(mpsc::Sender<()>, mpsc::Receiver<()>),
}

/// The registry keeps weak portals; a service or one current lease owns a portal.
struct MaintenancePortal {
    state: Mutex<PortalState>,
    released: Condvar,
}

#[derive(Default)]
struct PortalState {
    lease: PortalLease,
    next_generation: u64,
    service: Weak<Shared>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PortalLease {
    #[default]
    Idle,
    Leased(u64),
}

static PORTALS: OnceLock<Mutex<HashMap<String, Weak<MaintenancePortal>>>> = OnceLock::new();

fn portal_at(data_dir: &Path) -> Result<Arc<MaintenancePortal>, String> {
    // --data-dir keeps released relative-path support. Capture its lexical
    // absolute spelling without canonicalization or filesystem probing.
    let absolute = std::path::absolute(data_dir).map_err(|error| error.to_string())?;
    let key = crate::path_key::physical_path_spelling(&absolute)
        .ok_or("invalid spread shift data directory")?;
    let mut portals = PORTALS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(portal) = portals.get(&key).and_then(Weak::upgrade) {
        return Ok(portal);
    }
    let portal = Arc::new(MaintenancePortal {
        state: Mutex::new(PortalState::default()),
        released: Condvar::new(),
    });
    portals.insert(key, Arc::downgrade(&portal));
    Ok(portal)
}

#[derive(Clone)]
pub(crate) struct SpreadShiftService {
    shared: Arc<Shared>,
    // Keep the registry portal alive as long as a service front handle exists.
    _portal: Arc<MaintenancePortal>,
}

pub(crate) struct SpreadShiftProcessOwner {
    shared: Arc<Shared>,
    _portal: Arc<MaintenancePortal>,
    handle: Option<std::thread::JoinHandle<()>>,
    recorder: Option<ContentIdentityRecorder>,
}

impl SpreadShiftService {
    pub(crate) fn spawn_at(
        data_dir: PathBuf,
        observer: Option<ContentIdentityProducer>,
    ) -> Result<(Self, SpreadShiftProcessOwner), String> {
        Self::spawn_inner(data_dir, observer, StartupPause::default())
    }

    fn spawn_inner(
        data_dir: PathBuf,
        observer: Option<ContentIdentityProducer>,
        startup_pause: StartupPause,
    ) -> Result<(Self, SpreadShiftProcessOwner), String> {
        // Capture before worker launch so a later cwd change cannot retarget DB I/O.
        let data_dir = std::path::absolute(data_dir).map_err(|error| error.to_string())?;
        let portal = portal_at(&data_dir)?;
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            admission: Mutex::new(Admission {
                tx: Some(tx),
                mode: AdmissionMode::Running,
                watermarks: ShiftWatermarks {
                    admitted: 0,
                    completed_through: 0,
                },
                snapshot: None,
                deferred_commands: VecDeque::new(),
                results: VecDeque::new(),
                clear_results: VecDeque::new(),
                changed_scope_keys: Vec::new(),
                errors: Vec::new(),
            }),
            pending: AtomicUsize::new(0),
            #[cfg(test)]
            phase_changed: Condvar::new(),
        });
        let startup_wait = {
            let mut state = portal.state.lock().unwrap();
            if state.service.upgrade().is_some() {
                return Err("a spread shift service already owns this data directory".into());
            }
            let wait = match state.lease {
                PortalLease::Idle => None,
                PortalLease::Leased(generation) => Some(generation),
            };
            if wait.is_some() {
                shared.admission.lock().unwrap().mode = AdmissionMode::StartingAfterMaintenance;
            }
            state.service = Arc::downgrade(&shared);
            wait
        };
        let worker_shared = Arc::clone(&shared);
        let worker_portal = Arc::clone(&portal);
        let handle = std::thread::Builder::new()
            .name("spread-shift-writer".into())
            .spawn(move || {
                run_worker(
                    data_dir,
                    worker_shared,
                    worker_portal,
                    startup_wait,
                    observer,
                    rx,
                    startup_pause,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                shared: Arc::clone(&shared),
                _portal: Arc::clone(&portal),
            },
            SpreadShiftProcessOwner {
                shared,
                _portal: portal,
                handle: Some(handle),
                recorder: None,
            },
        ))
    }

    pub(crate) fn watermarks(&self) -> ShiftWatermarks {
        self.shared.admission.lock().unwrap().watermarks
    }

    pub(crate) fn read_after(
        &self,
        scope: BookScopeKey,
        cutoff: u64,
    ) -> mpsc::Receiver<ShiftReadReply> {
        let (reply, rx) = mpsc::channel();
        let request = ReadRequest {
            scope,
            cutoff,
            reply,
        };
        let mut admission = self.shared.admission.lock().unwrap();
        if let Err(error) = request.scope.validate() {
            send_read_error(&admission, request, error);
        } else if cutoff > admission.watermarks.admitted || admission.mode.is_closed() {
            send_read_error(
                &admission,
                request,
                "spread shift admission closed or cutoff not admitted".into(),
            );
        } else if matches!(
            admission.mode,
            AdmissionMode::Maintenance | AdmissionMode::StartingAfterMaintenance
        ) {
            self.shared.pending.fetch_add(1, Ordering::AcqRel);
            admission
                .deferred_commands
                .push_back(Command::Read(request));
        } else if admission.watermarks.admitted == admission.watermarks.completed_through
            && let Some(snapshot) = &admission.snapshot
            && snapshot.completed_through >= cutoff
        {
            let tx = request.reply.clone();
            let _ = tx.send(snapshot.read(request));
        } else {
            let backup = request.reply.clone();
            let error_scope = request.scope.clone();
            if let Err(error) = send_command(&self.shared, &mut admission, Command::Read(request)) {
                send_read_error(
                    &admission,
                    ReadRequest {
                        scope: error_scope,
                        cutoff,
                        reply: backup,
                    },
                    error,
                );
            }
        }
        rx
    }

    pub(crate) fn count_after(&self, cutoff: u64) -> mpsc::Receiver<ShiftCountReply> {
        let (reply, rx) = mpsc::channel();
        let request = CountRequest { cutoff, reply };
        let mut admission = self.shared.admission.lock().unwrap();
        if cutoff > admission.watermarks.admitted || admission.mode.is_closed() {
            send_count_error(
                &admission,
                request,
                "spread shift admission closed or cutoff not admitted".into(),
            );
        } else if matches!(
            admission.mode,
            AdmissionMode::Maintenance | AdmissionMode::StartingAfterMaintenance
        ) {
            self.shared.pending.fetch_add(1, Ordering::AcqRel);
            admission
                .deferred_commands
                .push_back(Command::Count(request));
        } else if admission.watermarks.admitted == admission.watermarks.completed_through
            && let Some(snapshot) = &admission.snapshot
            && snapshot.completed_through >= cutoff
        {
            let _ = request.reply.send(snapshot.count(cutoff));
        } else {
            let backup = request.reply.clone();
            if let Err(error) = send_command(&self.shared, &mut admission, Command::Count(request))
            {
                send_count_error(
                    &admission,
                    CountRequest {
                        cutoff,
                        reply: backup,
                    },
                    error,
                );
            }
        }
        rx
    }

    pub(crate) fn clear_all(&self) -> Result<u64, String> {
        let mut admission = self.shared.admission.lock().unwrap();
        require_write_admission(&admission)?;
        let sequence = admission
            .watermarks
            .admitted
            .checked_add(1)
            .ok_or("spread shift sequence exhausted")?;
        send_command(&self.shared, &mut admission, Command::ClearAll { sequence })?;
        admission.watermarks.admitted = sequence;
        Ok(sequence)
    }

    pub(crate) fn set_shift(
        &self,
        scope: BookScopeKey,
        anchor: SpreadShiftPageRef,
        operation: ShiftOperation,
    ) -> Result<u64, String> {
        scope.validate()?;
        anchor.validate()?;
        self.write(scope, operation, Mutation::Set(anchor))
    }

    pub(crate) fn clear_shift(
        &self,
        scope: BookScopeKey,
        operation: ShiftOperation,
    ) -> Result<u64, String> {
        scope.validate()?;
        self.write(scope, operation, Mutation::Clear)
    }

    fn write(
        &self,
        scope: BookScopeKey,
        operation: ShiftOperation,
        mutation: Mutation,
    ) -> Result<u64, String> {
        let mut admission = self.shared.admission.lock().unwrap();
        require_write_admission(&admission)?;
        let sequence = admission
            .watermarks
            .admitted
            .checked_add(1)
            .ok_or("spread shift sequence exhausted")?;
        send_command(
            &self.shared,
            &mut admission,
            Command::Write {
                sequence,
                scope,
                operation,
                mutation,
            },
        )?;
        // Publication is after successful send, while every &self sender and
        // completion publisher still share this same short admission lock.
        admission.watermarks.admitted = sequence;
        Ok(sequence)
    }

    /// Reports admission only; the worker closes its connection before applying
    /// the mapping and publishes terminal errors through drain_errors().
    pub(crate) fn migrate_page_paths(
        &self,
        mappings: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), String> {
        self.maintain(MaintenanceAction::PagePaths(mappings))
    }

    pub(crate) fn rename_book_root(&self, source: &Path, target: &Path) -> Result<(), String> {
        self.maintain(MaintenanceAction::RenameBookRoot {
            source: BookScopeKey::folder(source)?.storage_key(),
            target: BookScopeKey::folder(target)?.storage_key(),
        })
    }

    pub(crate) fn purge_book_root(&self, root: &Path) -> Result<(), String> {
        self.maintain(MaintenanceAction::PurgeBookRoot(
            BookScopeKey::folder(root)?.storage_key(),
        ))
    }

    fn maintain(&self, action: MaintenanceAction) -> Result<(), String> {
        let mut admission = self.shared.admission.lock().unwrap();
        let root_operation = matches!(
            &action,
            MaintenanceAction::RenameBookRoot { .. } | MaintenanceAction::PurgeBookRoot(_)
        );
        let defer = root_operation
            && matches!(
                admission.mode,
                AdmissionMode::Maintenance | AdmissionMode::StartingAfterMaintenance
            );
        if !defer {
            require_write_admission(&admission)?;
        }
        let sequence = admission
            .watermarks
            .admitted
            .checked_add(1)
            .ok_or("spread shift sequence exhausted")?;
        let command = Command::Maintain { sequence, action };
        if defer {
            // The filesystem operation already succeeded. Accept it as a lease
            // dependency, in the same FIFO as reads, without touching the DB.
            self.shared.pending.fetch_add(1, Ordering::AcqRel);
            admission.deferred_commands.push_back(command);
        } else {
            send_command(&self.shared, &mut admission, command)?;
        }
        admission.watermarks.admitted = sequence;
        Ok(())
    }

    pub(crate) fn drain_results(&self) -> Vec<ShiftWriteReply> {
        self.shared
            .admission
            .lock()
            .unwrap()
            .results
            .drain(..)
            .collect()
    }

    pub(crate) fn drain_errors(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.admission.lock().unwrap().errors)
    }

    pub(crate) fn drain_clear_results(&self) -> Vec<ShiftClearAllReply> {
        self.shared
            .admission
            .lock()
            .unwrap()
            .clear_results
            .drain(..)
            .collect()
    }

    /// Exact storage keys of actual row changes, for context-scoped D2 refresh.
    pub(crate) fn drain_changed_scope_keys(&self) -> Vec<String> {
        let keys = std::mem::take(&mut self.shared.admission.lock().unwrap().changed_scope_keys);
        keys.into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn is_busy(&self) -> bool {
        let admission = self.shared.admission.lock().unwrap();
        self.shared.pending.load(Ordering::Acquire) > 0
            || !admission.results.is_empty()
            || !admission.clear_results.is_empty()
            || !admission.errors.is_empty()
    }

    pub(crate) fn is_ready(&self) -> bool {
        let admission = self.shared.admission.lock().unwrap();
        admission.mode == AdmissionMode::Running && admission.snapshot.is_some()
    }

    pub(crate) fn close_admission(&self) {
        let mut admission = self.shared.admission.lock().unwrap();
        admission.mode = admission.mode.close();
    }

    #[cfg(test)]
    pub(crate) fn pause_for_test(&self) -> mpsc::Sender<()> {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let mut admission = self.shared.admission.lock().unwrap();
        send_command(
            &self.shared,
            &mut admission,
            Command::Pause(entered_tx, resume_rx),
        )
        .unwrap();
        drop(admission);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        resume_tx
    }
}

fn require_write_admission(admission: &Admission) -> Result<(), String> {
    if admission.mode != AdmissionMode::Running {
        return Err("spread shift writes unavailable during maintenance or exit".into());
    }
    if let Some(snapshot) = &admission.snapshot {
        snapshot.status.clone()?;
    }
    Ok(())
}

fn send_command(
    shared: &Shared,
    admission: &mut Admission,
    command: Command,
) -> Result<(), String> {
    shared.pending.fetch_add(1, Ordering::AcqRel);
    if admission
        .tx
        .as_ref()
        .is_some_and(|tx| tx.send(command).is_ok())
    {
        Ok(())
    } else {
        shared.pending.fetch_sub(1, Ordering::Release);
        Err("spread shift worker unavailable".into())
    }
}

fn send_read_error(admission: &Admission, request: ReadRequest, error: String) {
    let _ = request.reply.send(ShiftReadReply {
        cutoff: request.cutoff,
        completed_through: admission.watermarks.completed_through,
        maintenance_epoch: admission
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.maintenance_epoch),
        row: None,
        status: Err(error),
    });
}

fn send_count_error(admission: &Admission, request: CountRequest, error: String) {
    let _ = request.reply.send(ShiftCountReply {
        cutoff: request.cutoff,
        completed_through: admission.watermarks.completed_through,
        maintenance_epoch: admission
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.maintenance_epoch),
        count: 0,
        status: Err(error),
    });
}

impl SpreadShiftProcessOwner {
    pub(crate) fn retain_recorder_for_exit(&mut self, recorder: Option<ContentIdentityRecorder>) {
        if recorder.is_some() {
            self.recorder = recorder;
        }
    }

    /// Process-exit owner only. Close all front admission, drain FIFO, close the
    /// SQLite connection/producer, then join before releasing the recorder.
    pub(crate) fn shutdown_and_join(mut self) -> Vec<ShiftWriteReply> {
        {
            let mut admission = self.shared.admission.lock().unwrap();
            admission.mode = admission.mode.close();
            admission.tx = None;
            #[cfg(test)]
            self.shared.phase_changed.notify_all();
        }
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                crate::logger::log("spread shift writer panicked during exit");
            }
        }
        let mut portal = self._portal.state.lock().unwrap();
        if portal
            .service
            .upgrade()
            .is_some_and(|service| Arc::ptr_eq(&service, &self.shared))
        {
            portal.service = Weak::new();
        }
        drop(portal);
        let results = self
            .shared
            .admission
            .lock()
            .unwrap()
            .results
            .drain(..)
            .collect();
        // The recorder is deliberately retained until all successful Set edits
        // have been sent and the producer's sender has been dropped.
        self.recorder = None;
        results
    }
}

impl Drop for SpreadShiftProcessOwner {
    fn drop(&mut self) {
        // Only explicit shutdown_and_join waits. Dropping an App-facing guard
        // must not add a UI join; process owner handoff provides the normal drain.
        let mut admission = self.shared.admission.lock().unwrap();
        admission.mode = admission.mode.close();
        admission.tx = None;
    }
}

/// Call and drop this lease only inside the existing maintenance worker. Its
/// ACKs close/reload the single writer connection; frontend service Drop never waits.
pub(crate) struct SpreadShiftMaintenanceLease {
    portal: Arc<MaintenancePortal>,
    service: Option<(Arc<Shared>, mpsc::Sender<Command>)>,
}

pub(crate) fn maintenance_at(data_dir: &Path) -> Result<SpreadShiftMaintenanceLease, String> {
    let portal = portal_at(data_dir)?;
    let service = {
        let mut state = portal.state.lock().unwrap();
        while state.lease != PortalLease::Idle {
            state = portal.released.wait(state).unwrap();
        }
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .expect("maintenance generation exhausted");
        state.lease = PortalLease::Leased(state.next_generation);
        state.service.upgrade()
    };
    let service = if let Some(service) = service {
        let (reply, rx) = mpsc::channel();
        let begin = {
            let mut admission = service.admission.lock().unwrap();
            if admission.mode.is_closed() {
                Err("spread shift service is closing".into())
            } else {
                admission.mode = AdmissionMode::Maintenance;
                let result =
                    send_command(&service, &mut admission, Command::BeginMaintenance(reply))
                        .map(|()| admission.tx.as_ref().unwrap().clone());
                #[cfg(test)]
                service.phase_changed.notify_all();
                result
            }
        };
        match begin.and_then(|tx| {
            rx.recv().map_err(|error| error.to_string())??;
            Ok(tx)
        }) {
            Ok(tx) => Some((service, tx)),
            Err(error) => {
                portal.state.lock().unwrap().lease = PortalLease::Idle;
                portal.released.notify_all();
                return Err(error);
            }
        }
    } else {
        None
    };
    Ok(SpreadShiftMaintenanceLease { portal, service })
}

impl Drop for SpreadShiftMaintenanceLease {
    fn drop(&mut self) {
        if let Some((service, tx)) = &self.service {
            let (reply, rx) = mpsc::channel();
            // Keep the actor alive through lease release even after process-exit
            // admission closes, so accepted dependencies still drain normally.
            service.pending.fetch_add(1, Ordering::AcqRel);
            let sent = tx.send(Command::EndMaintenance(reply)).map_err(|error| {
                service.pending.fetch_sub(1, Ordering::Release);
                error.to_string()
            });
            let result = sent.and_then(|()| rx.recv().map_err(|error| error.to_string())?);
            if let Err(error) = result {
                crate::logger::log(format!("spread shift maintenance end failed: {error}"));
            }
        }
        self.portal.state.lock().unwrap().lease = PortalLease::Idle;
        self.portal.released.notify_all();
    }
}

fn open_snapshot(
    data_dir: &Path,
    completed_through: u64,
    maintenance_epoch: u64,
) -> (Option<rusqlite::Connection>, Arc<Snapshot>) {
    let loaded = (|| {
        let connection = store::open_existing(&data_dir.join("spread_shift.db"))?;
        let rows = match &connection {
            Some(connection) => store::load_rows(connection)?,
            None => BTreeMap::new(),
        };
        Ok::<_, String>((connection, rows))
    })();
    let (connection, rows, status) = match loaded {
        Ok((connection, rows)) => (connection, rows, Ok(())),
        Err(error) => {
            crate::logger::log(format!("spread shift load failed: {error}"));
            (None, BTreeMap::new(), Err(error))
        }
    };
    (
        connection,
        Arc::new(Snapshot {
            completed_through,
            maintenance_epoch,
            rows,
            status,
        }),
    )
}

fn changed_keys(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    if before.status.is_err() || after.status.is_err() {
        // An unreadable snapshot does not prove deletion of any stored row.
        return Vec::new();
    }
    before
        .rows
        .iter()
        .filter_map(|(key, value)| (after.rows.get(key) != Some(value)).then(|| key.clone()))
        .chain(
            after
                .rows
                .keys()
                .filter(|key| !before.rows.contains_key(*key))
                .cloned(),
        )
        .collect()
}

fn run_worker(
    data_dir: PathBuf,
    shared: Arc<Shared>,
    portal: Arc<MaintenancePortal>,
    startup_wait: Option<u64>,
    observer: Option<ContentIdentityProducer>,
    rx: mpsc::Receiver<Command>,
    startup_pause: StartupPause,
) {
    #[cfg(test)]
    if let Some((entered, resume)) = startup_pause {
        let _ = entered.send(());
        let _ = resume.recv();
    }
    #[cfg(not(test))]
    let _ = startup_pause;
    if let Some(generation) = startup_wait {
        let mut state = portal.state.lock().unwrap();
        // Only the pre-registration external lease blocks bootstrap. A later
        // lease already references this actor and awaits its FIFO Begin ACK;
        // waiting for that later lease would deadlock its requester.
        while state.lease == PortalLease::Leased(generation) {
            state = portal.released.wait(state).unwrap();
        }
    }
    let (mut connection, snapshot) = open_snapshot(&data_dir, 0, 0);
    let mut ready_commands = {
        let mut admission = shared.admission.lock().unwrap();
        if let Err(error) = &snapshot.status {
            admission.errors.push(error.clone());
        }
        admission.snapshot = Some(Arc::clone(&snapshot));
        if matches!(
            admission.mode,
            AdmissionMode::StartingAfterMaintenance | AdmissionMode::ClosedStartingAfterMaintenance
        ) {
            if !admission.mode.is_closed() {
                admission.mode = AdmissionMode::Running;
            } else {
                admission.mode = AdmissionMode::Closed;
            }
            std::mem::take(&mut admission.deferred_commands)
        } else {
            VecDeque::new()
        }
    };
    drop(snapshot);
    // Lease dependencies run before any commands admitted after the release.
    while let Some(command) = ready_commands.pop_front().or_else(|| rx.recv().ok()) {
        match command {
            Command::Write {
                sequence,
                scope,
                operation,
                mutation,
            } => {
                let mut snapshot = shared.admission.lock().unwrap().snapshot.take().unwrap();
                // COW and all DB/observer work happen outside the UI admission lock.
                let state = Arc::make_mut(&mut snapshot);
                let key = scope.storage_key();
                let result = (|| {
                    state.status.clone()?;
                    match &mutation {
                        Mutation::Set(reference) => {
                            if connection.is_none() {
                                store::initialize_database_at(&data_dir.join("spread_shift.db"))?;
                                connection =
                                    store::open_existing(&data_dir.join("spread_shift.db"))?;
                            }
                            store::set(
                                connection
                                    .as_mut()
                                    .ok_or("spread shift database unavailable")?,
                                &key,
                                reference,
                            )
                        }
                        Mutation::Clear => match connection.as_mut() {
                            Some(connection) => store::clear(connection, &key),
                            None => Ok(false),
                        },
                    }
                })();
                let (changed, status) = match result {
                    Ok(changed) => {
                        match mutation {
                            Mutation::Set(reference) => {
                                // Source kind comes from the accepted relative-reference
                                // domain, not the extension of a folder named e.g. book.zip.
                                let observes_file =
                                    !matches!(reference, SpreadShiftPageRef::RelativePath(_));
                                state.rows.insert(key, reference);
                                if changed
                                    && observes_file
                                    && let Some(observer) = &observer
                                {
                                    observer.record_for_path(
                                        &scope.logical_root,
                                        ContentIdentityTrigger::Edit,
                                    );
                                }
                            }
                            Mutation::Clear => {
                                state.rows.remove(&key);
                            }
                        }
                        (changed, Ok(()))
                    }
                    Err(error) => {
                        crate::logger::log(format!("spread shift write failed: {error}"));
                        state.status = Err(error.clone());
                        (false, Err(error))
                    }
                };
                state.completed_through = sequence;
                let mut admission = shared.admission.lock().unwrap();
                admission.snapshot = Some(snapshot);
                admission.watermarks.completed_through = sequence;
                if changed {
                    admission.changed_scope_keys.push(scope.storage_key());
                }
                admission.results.push_back(ShiftWriteReply {
                    sequence,
                    scope,
                    operation,
                    changed,
                    status,
                });
            }
            Command::Read(request) => {
                let snapshot = shared
                    .admission
                    .lock()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .clone();
                let tx = request.reply.clone();
                let _ = tx.send(snapshot.read(request));
            }
            Command::Count(request) => {
                let snapshot = shared
                    .admission
                    .lock()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .clone();
                let _ = request.reply.send(snapshot.count(request.cutoff));
            }
            Command::ClearAll { sequence } => {
                let mut snapshot = shared.admission.lock().unwrap().snapshot.take().unwrap();
                let state = Arc::make_mut(&mut snapshot);
                let result = state
                    .status
                    .clone()
                    .and_then(|()| match connection.as_mut() {
                        Some(connection) => store::clear_all(connection),
                        None => Ok(0),
                    });
                let (removed, status, changed_keys) = match result {
                    Ok(removed) => {
                        let keys = if removed > 0 {
                            state.rows.keys().cloned().collect()
                        } else {
                            Vec::new()
                        };
                        state.rows.clear();
                        (removed, Ok(()), keys)
                    }
                    Err(error) => {
                        crate::logger::log(format!("spread shift clear all failed: {error}"));
                        state.status = Err(error.clone());
                        (0, Err(error), Vec::new())
                    }
                };
                state.completed_through = sequence;
                let mut admission = shared.admission.lock().unwrap();
                admission.snapshot = Some(snapshot);
                admission.watermarks.completed_through = sequence;
                admission.changed_scope_keys.extend(changed_keys);
                admission.clear_results.push_back(ShiftClearAllReply {
                    sequence,
                    removed,
                    status,
                });
            }
            Command::Maintain { sequence, action } => {
                // All book completion operations share the actor's admission order.
                // Page mapping reuses an external primitive only after closing its connection.
                let result = match action {
                    MaintenanceAction::PagePaths(mappings) => {
                        connection = None;
                        store::migrate_page_paths_at(&data_dir, &mappings).map(|_| ())
                    }
                    MaintenanceAction::RenameBookRoot { source, target } => {
                        connection.as_mut().map_or(Ok(()), |connection| {
                            store::rename_book_root(connection, &source, &target).map(|_| ())
                        })
                    }
                    MaintenanceAction::PurgeBookRoot(root) => {
                        connection.as_mut().map_or(Ok(()), |connection| {
                            store::purge_book_root(connection, &root).map(|_| ())
                        })
                    }
                };
                connection = None;
                let previous = shared.admission.lock().unwrap().snapshot.take().unwrap();
                let epoch = previous.maintenance_epoch;
                let (opened, mut snapshot) = open_snapshot(&data_dir, sequence, epoch + 1);
                connection = opened;
                if let Err(error) = &result {
                    crate::logger::log(format!("spread shift book maintenance failed: {error}"));
                    Arc::make_mut(&mut snapshot).status = Err(error.clone());
                }
                let changed = changed_keys(&previous, &snapshot);
                drop(previous); // Potential full-map drop stays outside admission.
                let mut admission = shared.admission.lock().unwrap();
                if let Err(error) = &snapshot.status {
                    admission.errors.push(error.clone());
                }
                admission.snapshot = Some(snapshot);
                admission.watermarks.completed_through = sequence;
                admission.changed_scope_keys.extend(changed);
            }
            Command::BeginMaintenance(reply) => {
                connection = None;
                // Earlier successful Sets already published Edit to the observer
                // before their terminal result. Connection drop precedes this ACK.
                let _ = reply.send(Ok(()));
            }
            Command::EndMaintenance(reply) => {
                let previous = shared.admission.lock().unwrap().snapshot.take().unwrap();
                let (completed, epoch) = (previous.completed_through, previous.maintenance_epoch);
                let (opened, snapshot) = open_snapshot(&data_dir, completed, epoch + 1);
                connection = opened;
                let status = snapshot.status.clone();
                let changed = changed_keys(&previous, &snapshot);
                drop(previous);
                let mut admission = shared.admission.lock().unwrap();
                if let Err(error) = &status {
                    admission.errors.push(error.clone());
                }
                admission.snapshot = Some(Arc::clone(&snapshot));
                admission.changed_scope_keys.extend(changed);
                admission.mode = if admission.mode.is_closed() {
                    AdmissionMode::Closed
                } else {
                    AdmissionMode::Running
                };
                // Preserve root-operation/read order. Each read captures the
                // snapshot after its preceding roots, before any later mutation.
                ready_commands.extend(std::mem::take(&mut admission.deferred_commands));
                drop(admission);
                let _ = reply.send(status);
            }
            #[cfg(test)]
            Command::Pause(entered, resume) => {
                let _ = entered.send(());
                let _ = resume.recv();
            }
        }
        shared.pending.fetch_sub(1, Ordering::Release);
    }
    connection = None;
    drop(connection);
    let mut admission = shared.admission.lock().unwrap();
    let deferred = std::mem::take(&mut admission.deferred_commands);
    for request in deferred {
        match request {
            Command::Read(request) => send_read_error(
                &admission,
                request,
                "spread shift service exited during maintenance".into(),
            ),
            Command::Count(request) => send_count_error(
                &admission,
                request,
                "spread shift service exited during maintenance".into(),
            ),
            _ => crate::logger::log(
                "spread shift worker exited with unprocessed maintenance dependencies",
            ),
        }
        shared.pending.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ViewerContextId;
    use std::time::Duration;

    fn operation(sequence: u64) -> ShiftOperation {
        ShiftOperation {
            context_id: ViewerContextId::for_test(1),
            items_generation: 4,
            operation_sequence: sequence,
        }
    }

    fn scope() -> BookScopeKey {
        BookScopeKey::new("C:/books/book.zip", "chapter").unwrap()
    }

    fn anchor(page: u32) -> SpreadShiftPageRef {
        SpreadShiftPageRef::ArchiveEntry(format!("chapter/page{page}.png"))
    }

    fn read(service: &SpreadShiftService, scope: BookScopeKey, cutoff: u64) -> ShiftReadReply {
        service
            .read_after(scope, cutoff)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }

    #[test]
    fn fifo_pending_set_clear_and_later_sibling_do_not_replace_read() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        assert_eq!(read(&service, scope(), 0).row, None);
        let paused = service.pause_for_test();
        let first = service.set_shift(scope(), anchor(2), operation(1)).unwrap();
        assert_eq!(
            service.watermarks(),
            ShiftWatermarks {
                admitted: first,
                completed_through: 0
            }
        );
        let pending = service.read_after(scope(), first);
        assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let cleared = service.clear_shift(scope(), operation(2)).unwrap();
        let clear_read = service.read_after(scope(), cleared);
        let later = service.set_shift(scope(), anchor(8), operation(3)).unwrap();
        drop(service.clone()); // Front handle drop never joins the paused writer.
        paused.send(()).unwrap();
        let first_read = pending.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(first_read.row, Some(anchor(2)));
        assert_eq!(first_read.completed_through, first);
        assert_eq!(first_read.status, Ok(()));
        let cleared_read = clear_read.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(cleared_read.row, None);
        assert_eq!(cleared_read.completed_through, cleared);
        assert_eq!(read(&service, scope(), later).row, Some(anchor(8)));
        assert!(service.is_busy()); // Completed results still belong to their operations.
        let replies = service.drain_results();
        assert_eq!(
            replies
                .iter()
                .map(|reply| reply.sequence)
                .collect::<Vec<_>>(),
            vec![first, cleared, later]
        );
        assert!(
            replies
                .iter()
                .all(|reply| reply.changed && reply.status.is_ok())
        );
        assert_eq!(replies[2].operation, operation(3));
        owner.shutdown_and_join();
    }

    #[test]
    fn warm_reads_and_noop_clear_do_not_create_database() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        assert_eq!(read(&service, scope(), 0).status, Ok(()));
        assert!(!temp.path().join("spread_shift.db").exists());
        let clear = service.clear_shift(scope(), operation(1)).unwrap();
        assert_eq!(read(&service, scope(), clear).row, None);
        assert!(!service.drain_results()[0].changed);
        assert!(!temp.path().join("spread_shift.db").exists());
        assert!(service.is_ready());
        owner.shutdown_and_join();
    }

    #[test]
    fn relative_data_directory_alias_uses_one_portal_and_captured_worker_database() {
        let target = std::env::current_dir().unwrap().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("relative-shift-")
            .tempdir_in(&target)
            .unwrap();
        let relative = Path::new("target").join(temp.path().file_name().unwrap());
        let absolute_portal = portal_at(temp.path()).unwrap();
        let relative_portal = portal_at(&relative).unwrap();
        assert!(Arc::ptr_eq(&absolute_portal, &relative_portal));
        let (service, owner) = SpreadShiftService::spawn_at(relative, None).unwrap();
        assert!(read(&service, scope(), 0).status.is_ok());
        assert!(SpreadShiftService::spawn_at(temp.path().into(), None).is_err());
        let saved = service.set_shift(scope(), anchor(3), operation(1)).unwrap();
        assert_eq!(read(&service, scope(), saved).row, Some(anchor(3)));
        let lease = maintenance_at(temp.path()).unwrap();
        assert!(!service.is_ready());
        let count = service.count_after(saved);
        assert!(matches!(count.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(lease);
        assert_eq!(
            count.recv_timeout(Duration::from_secs(10)).unwrap().count,
            1
        );
        owner.shutdown_and_join();
        let connection = store::open_existing(&temp.path().join("spread_shift.db"))
            .unwrap()
            .unwrap();
        assert_eq!(
            store::load_rows(&connection).unwrap()[&scope().storage_key()],
            anchor(3)
        );
    }

    #[test]
    fn process_owner_closes_admission_and_drains_normal_exit() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let paused = service.pause_for_test();
        service.set_shift(scope(), anchor(1), operation(1)).unwrap();
        service.clear_shift(scope(), operation(2)).unwrap();
        let final_sequence = service.set_shift(scope(), anchor(9), operation(3)).unwrap();
        service.close_admission();
        assert!(
            service
                .set_shift(scope(), anchor(10), operation(4))
                .is_err()
        );
        paused.send(()).unwrap();
        let replies = owner.shutdown_and_join();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies.last().unwrap().sequence, final_sequence);
        let connection = store::open_existing(&temp.path().join("spread_shift.db"))
            .unwrap()
            .unwrap();
        assert_eq!(
            store::load_rows(&connection).unwrap()[&scope().storage_key()],
            anchor(9)
        );
        // Old frontend handles remain harmless after the process owner retired.
        assert!(read(&service, scope(), final_sequence).status.is_err());
    }

    #[test]
    fn maintenance_closes_writer_and_defers_reads_until_refreshed_epoch() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let saved = service.set_shift(scope(), anchor(1), operation(1)).unwrap();
        assert_eq!(read(&service, scope(), saved).row, Some(anchor(1)));
        service.drain_results();
        service.drain_changed_scope_keys();
        let lease = maintenance_at(temp.path()).unwrap();
        assert!(!service.is_ready());
        assert!(service.set_shift(scope(), anchor(5), operation(2)).is_err());
        let deferred = service.read_after(scope(), saved);
        let count = service.count_after(saved);
        assert!(matches!(
            deferred.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let mut connection = store::open_existing(&temp.path().join("spread_shift.db"))
            .unwrap()
            .unwrap();
        store::set(&mut connection, &scope().storage_key(), &anchor(6)).unwrap();
        drop(connection);
        drop(lease);
        let restored = deferred.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(restored.row, Some(anchor(6)));
        assert_eq!(restored.maintenance_epoch, 1);
        assert_eq!(restored.completed_through, saved);
        let counted = count.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(counted.count, 1);
        assert_eq!(counted.maintenance_epoch, 1);
        assert_eq!(
            service.drain_changed_scope_keys(),
            vec![scope().storage_key()]
        );
        assert!(service.is_ready());
        assert_eq!(read(&service, scope(), saved).maintenance_epoch, 1);
        owner.shutdown_and_join();
    }

    #[test]
    fn lease_dependencies_preserve_root_read_count_fifo_and_completion_proofs() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let saved = service.set_shift(scope(), anchor(1), operation(1)).unwrap();
        assert_eq!(read(&service, scope(), saved).row, Some(anchor(1)));
        let retained = service
            .shared
            .admission
            .lock()
            .unwrap()
            .snapshot
            .clone()
            .unwrap();
        let lease = maintenance_at(temp.path()).unwrap();
        let before = service.read_after(scope(), saved);
        service
            .rename_book_root(
                Path::new("C:/books/book.zip"),
                Path::new("C:/books/new.zip"),
            )
            .unwrap();
        let renamed = BookScopeKey::new("C:/books/new.zip", "chapter").unwrap();
        let after_rename = service.read_after(renamed.clone(), saved + 1);
        let rename_count = service.count_after(saved + 1);
        service
            .purge_book_root(Path::new("C:/books/new.zip"))
            .unwrap();
        let after_delete = service.read_after(renamed.clone(), saved + 2);
        let delete_count = service.count_after(saved + 2);
        assert_eq!(service.watermarks().admitted, saved + 2);
        assert_eq!(service.watermarks().completed_through, saved);
        assert!(retained.completed_through < saved + 1);
        for receiver in [&before, &after_rename, &after_delete] {
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
        drop(lease);
        let before = before.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(before.row, Some(anchor(1)));
        assert_eq!(before.completed_through, saved);
        let renamed_reply = after_rename.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(renamed_reply.row, Some(anchor(1)));
        assert_eq!(renamed_reply.completed_through, saved + 1);
        let counted = rename_count.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(counted.count, 1);
        assert_eq!(counted.completed_through, saved + 1);
        let deleted = after_delete.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(deleted.row, None);
        assert_eq!(deleted.completed_through, saved + 2);
        assert!(deleted.status.is_ok());
        let counted = delete_count.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(counted.count, 0);
        assert_eq!(counted.completed_through, saved + 2);
        assert_eq!(read(&service, scope(), saved + 2).row, None);
        owner.shutdown_and_join();
        assert!(!service.is_busy());
    }

    #[test]
    fn normal_exit_during_real_lease_drains_accepted_root_and_read() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let saved = service.set_shift(scope(), anchor(1), operation(1)).unwrap();
        read(&service, scope(), saved);
        service.drain_results();
        let lease = maintenance_at(temp.path()).unwrap();
        service
            .purge_book_root(Path::new("C:/books/book.zip"))
            .unwrap();
        let pending = service.read_after(scope(), saved + 1);
        service.close_admission();
        assert!(
            service
                .purge_book_root(Path::new("C:/books/book.zip"))
                .is_err()
        );
        let (done_tx, done_rx) = mpsc::channel();
        let exit = std::thread::spawn(move || {
            owner.shutdown_and_join();
            done_tx.send(()).unwrap();
        });
        {
            let mut admission = service.shared.admission.lock().unwrap();
            while admission.tx.is_some() {
                admission = service.shared.phase_changed.wait(admission).unwrap();
            }
        }
        drop(lease);
        let deleted = pending.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(deleted.row, None);
        assert_eq!(deleted.completed_through, saved + 1);
        assert!(deleted.status.is_ok());
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        exit.join().unwrap();
        let (reopened, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        assert_eq!(read(&reopened, scope(), 0).row, None);
        owner.shutdown_and_join();
    }

    #[test]
    fn root_dependency_at_bootstrap_survives_closed_admission() {
        let temp = tempfile::tempdir().unwrap();
        let lease = maintenance_at(temp.path()).unwrap();
        store::initialize_database_at(&temp.path().join("spread_shift.db")).unwrap();
        let mut connection = store::open_existing(&temp.path().join("spread_shift.db"))
            .unwrap()
            .unwrap();
        store::set(&mut connection, &scope().storage_key(), &anchor(1)).unwrap();
        drop(connection);
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        service
            .rename_book_root(
                Path::new("C:/books/book.zip"),
                Path::new("C:/books/new.zip"),
            )
            .unwrap();
        let renamed = BookScopeKey::new("C:/books/new.zip", "chapter").unwrap();
        let pending = service.read_after(renamed.clone(), 1);
        service.close_admission();
        drop(lease);
        let reply = pending.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply.row, Some(anchor(1)));
        assert_eq!(reply.completed_through, 1);
        assert!(reply.status.is_ok());
        owner.shutdown_and_join();
        let (reopened, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        assert_eq!(read(&reopened, renamed, 0).row, Some(anchor(1)));
        assert_eq!(read(&reopened, scope(), 0).row, None);
        owner.shutdown_and_join();
    }

    #[test]
    fn portal_serializes_maintenance_without_a_running_service() {
        let temp = tempfile::tempdir().unwrap();
        let first = maintenance_at(temp.path()).unwrap();
        let data_dir = temp.path().to_path_buf();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let second = maintenance_at(&data_dir).unwrap();
            acquired_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(second);
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            acquired_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(Arc::ptr_eq(
            &portal_at(temp.path()).unwrap(),
            &portal_at(&temp.path().join(".")).unwrap()
        ));
    }

    #[test]
    fn service_starting_during_external_lease_waits_on_worker_only() {
        let temp = tempfile::tempdir().unwrap();
        let lease = maintenance_at(temp.path()).unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let deferred = service.read_after(scope(), 0);
        assert!(matches!(
            deferred.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!service.is_ready());
        store::initialize_database_at(&temp.path().join("spread_shift.db")).unwrap();
        let mut connection = store::open_existing(&temp.path().join("spread_shift.db"))
            .unwrap()
            .unwrap();
        store::set(&mut connection, &scope().storage_key(), &anchor(3)).unwrap();
        drop(connection);
        drop(lease);
        assert_eq!(
            deferred.recv_timeout(Duration::from_secs(10)).unwrap().row,
            Some(anchor(3))
        );
        assert!(service.is_ready());
        owner.shutdown_and_join();
    }

    #[test]
    fn second_lease_before_bootstrap_wake_does_not_deadlock_or_resume_admission() {
        let temp = tempfile::tempdir().unwrap();
        let first_lease = maintenance_at(temp.path()).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (service, owner) = SpreadShiftService::spawn_inner(
            temp.path().into(),
            None,
            Some((entered_tx, resume_rx)),
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let deferred = service.read_after(scope(), 0);
        drop(first_lease);
        let data_dir = temp.path().to_path_buf();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let second_worker = std::thread::spawn(move || {
            let lease = maintenance_at(&data_dir).unwrap();
            acquired_tx.send(()).unwrap();
            store::initialize_database_at(&data_dir.join("spread_shift.db")).unwrap();
            let mut connection = store::open_existing(&data_dir.join("spread_shift.db"))
                .unwrap()
                .unwrap();
            store::set(&mut connection, &scope().storage_key(), &anchor(12)).unwrap();
            drop(connection);
            release_rx.recv().unwrap();
            drop(lease);
        });
        {
            let mut admission = service.shared.admission.lock().unwrap();
            while admission.mode != AdmissionMode::Maintenance {
                admission = service.shared.phase_changed.wait(admission).unwrap();
            }
        }
        // The second lease's Begin is in the actor FIFO before bootstrap wakes.
        resume_tx.send(()).unwrap();
        acquired_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!service.is_ready());
        assert!(service.set_shift(scope(), anchor(7), operation(1)).is_err());
        assert!(matches!(
            deferred.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        second_worker.join().unwrap();
        let reply = deferred.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply.row, Some(anchor(12)));
        assert_eq!(reply.maintenance_epoch, 1);
        owner.shutdown_and_join();
    }

    #[test]
    fn count_and_clear_all_share_mutation_cutoff_and_report_exact_changes() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let other = BookScopeKey::new("C:/books/other.zip", "").unwrap();
        service.set_shift(scope(), anchor(1), operation(1)).unwrap();
        let saved = service
            .set_shift(other.clone(), anchor(2), operation(2))
            .unwrap();
        assert_eq!(
            service
                .count_after(saved)
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .count,
            2
        );
        service.drain_results();
        assert_eq!(service.drain_changed_scope_keys().len(), 2);
        let paused = service.pause_for_test();
        let cleared = service.clear_all().unwrap();
        let counted = service.count_after(cleared);
        let restored = service.read_after(scope(), cleared);
        assert!(matches!(counted.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(matches!(
            restored.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        paused.send(()).unwrap();
        let count = counted.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(count.count, 0);
        assert_eq!(count.completed_through, cleared);
        assert_eq!(count.status, Ok(()));
        assert_eq!(
            restored.recv_timeout(Duration::from_secs(10)).unwrap().row,
            None
        );
        let clear = service.drain_clear_results();
        assert_eq!(clear.len(), 1);
        assert_eq!(clear[0].sequence, cleared);
        assert_eq!(clear[0].removed, 2);
        assert_eq!(clear[0].status, Ok(()));
        assert_eq!(
            service.drain_changed_scope_keys(),
            vec![scope().storage_key(), other.storage_key()]
        );
        let noop = service.clear_all().unwrap();
        service
            .count_after(noop)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(service.drain_clear_results()[0].removed, 0);
        assert!(service.drain_changed_scope_keys().is_empty());
        owner.shutdown_and_join();
    }

    #[test]
    fn unreadable_original_is_terminal_not_successful_absence_or_recreated() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread_shift.db");
        let original = b"invalid sqlite original";
        std::fs::write(&path, original).unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let reply = read(&service, scope(), 0);
        assert!(reply.status.is_err());
        assert!(service.is_ready());
        assert_eq!(service.drain_errors().len(), 1);
        assert!(service.set_shift(scope(), anchor(1), operation(1)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        owner.shutdown_and_join();
    }

    #[test]
    fn page_migration_is_async_and_next_fenced_read_sees_mapped_anchor() {
        let temp = tempfile::tempdir().unwrap();
        let (service, owner) = SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
        let folder = BookScopeKey::folder("C:/books/folder").unwrap();
        let saved = service
            .set_shift(
                folder.clone(),
                SpreadShiftPageRef::RelativePath("old.png".into()),
                operation(1),
            )
            .unwrap();
        assert!(read(&service, folder.clone(), saved).status.is_ok());
        let paused = service.pause_for_test();
        service
            .migrate_page_paths(vec![(
                "C:/books/folder/old.png".into(),
                "C:/books/folder/new.png".into(),
            )])
            .unwrap();
        let cutoff = service.watermarks().admitted;
        let next = service.read_after(folder, cutoff);
        assert!(matches!(next.try_recv(), Err(mpsc::TryRecvError::Empty)));
        paused.send(()).unwrap();
        let reply = next.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            reply.row,
            Some(SpreadShiftPageRef::RelativePath("new.png".into()))
        );
        assert_eq!(reply.maintenance_epoch, 1);
        assert_eq!(reply.completed_through, cutoff);
        assert_eq!(reply.status, Ok(()));
        owner.shutdown_and_join();
    }
}
