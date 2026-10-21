//! Process-owned single writer. Front handles do no I/O and never join a worker.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};

use super::{
    BookComposition, PageDomain,
    store::{self, StoredComposition},
};
use crate::book_scope::BookScopeKey;
use crate::content_identity::{ContentIdentityProducer, ContentIdentityTrigger};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompositionToken {
    pub(crate) service_epoch: u64,
    pub(crate) publication_sequence: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct CompositionSnapshot {
    pub(crate) status: Result<(), String>,
    pub(crate) rows: BTreeMap<String, Arc<StoredComposition>>,
    epoch: u64,
    sequence: u64,
    tokens: BTreeMap<String, u64>,
}

impl CompositionSnapshot {
    pub(crate) fn token_for(&self, scope: &BookScopeKey) -> CompositionToken {
        CompositionToken {
            service_epoch: self.epoch,
            publication_sequence: self.tokens.get(&scope.storage_key()).copied().unwrap_or(0),
        }
    }

    fn token_for_key(&self, key: &str) -> CompositionToken {
        CompositionToken {
            service_epoch: self.epoch,
            publication_sequence: self.tokens.get(key).copied().unwrap_or(0),
        }
    }
}

#[derive(Debug)]
pub(crate) struct CompositionPrepareReply {
    pub(crate) scope: BookScopeKey,
    pub(crate) domain: PageDomain,
    pub(crate) token: CompositionToken,
    pub(crate) status: Result<Option<StoredComposition>, String>,
}

#[derive(Debug)]
pub(crate) struct CompositionWriteReply {
    pub(crate) scope: BookScopeKey,
    pub(crate) changed: bool,
    pub(crate) token: CompositionToken,
    pub(crate) status: Result<(), String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Running { outstanding_mutations: u64 },
    Maintenance,
    Closed,
}

struct Admission {
    mode: Mode,
    tx: Option<mpsc::Sender<Command>>,
    snapshot: Arc<CompositionSnapshot>,
    changed: Vec<String>,
    errors: Vec<String>,
}

impl Admission {
    fn admit_mutation(&mut self) {
        let Mode::Running {
            outstanding_mutations,
        } = &mut self.mode
        else {
            unreachable!("mutations are admitted only while running");
        };
        *outstanding_mutations = outstanding_mutations
            .checked_add(1)
            .expect("composition mutation admission exhausted");
    }
}

/// Retirement follows the mutation's final publication and precedes its reply.
/// Maintenance rejects new mutations and its Begin is FIFO behind all admitted ones.
fn finish_mutation(shared: &Shared) {
    let mut admission = shared.admission.lock().unwrap();
    if let Mode::Running {
        outstanding_mutations,
    } = &mut admission.mode
    {
        *outstanding_mutations = outstanding_mutations
            .checked_sub(1)
            .expect("composition mutation completion without admission");
    }
}

struct Shared {
    admission: Mutex<Admission>,
}

enum Mutation {
    Save(BookComposition),
    Remove,
}
enum MaintenanceAction {
    Rename(
        String,
        String,
        crate::rename_key_migration::PathMigrationCollision,
    ),
    #[cfg(test)]
    Purge(String),
    Pages(Vec<(PathBuf, PathBuf)>),
}
enum Command {
    ViewTrim(
        crate::view_trim_db::ViewTrimWriteBatch,
        crate::page_edit_write_epoch::EditWriteGuard<'static>,
        mpsc::Sender<Result<(), String>>,
    ),
    RemoveViewTrimPage(
        String,
        crate::page_edit_write_epoch::EditWriteGuard<'static>,
        mpsc::Sender<Result<(), String>>,
    ),
    ViewTrimBook(
        crate::spread_db::SpreadContainerKey,
        crate::view_trim::ViewTrimBookState,
        super::CompositionWriteAdmission,
        crate::page_edit_write_epoch::EditWriteGuard<'static>,
        mpsc::Sender<Result<crate::view_trim::ViewTrimBookState, String>>,
    ),
    #[cfg(test)]
    HoldWorker(mpsc::Sender<()>, mpsc::Receiver<()>),
    ViewMode(
        BookScopeKey,
        crate::settings::SpreadMode,
        mpsc::Sender<Result<(), String>>,
    ),
    ViewFlow(
        BookScopeKey,
        crate::settings::SpreadMode,
        crate::settings::ReadingFlow,
        mpsc::Sender<Result<(), String>>,
    ),
    Candidate(
        super::CandidateCorrelation,
        super::CandidatePage,
        mpsc::Sender<super::CandidatePermissionReply>,
    ),
    Prepare(
        BookScopeKey,
        PageDomain,
        mpsc::Sender<CompositionPrepareReply>,
    ),
    Write(
        BookScopeKey,
        PageDomain,
        Mutation,
        mpsc::Sender<CompositionWriteReply>,
    ),
    Begin(mpsc::Sender<()>),
    End(mpsc::Sender<Result<(), String>>),
    Maintain(MaintenanceAction),
}

#[derive(Default)]
struct PortalState {
    lease_generation: Option<u64>,
    next_generation: u64,
    service: Weak<Shared>,
}
struct Portal {
    state: Mutex<PortalState>,
    released: Condvar,
}
static PORTALS: OnceLock<Mutex<HashMap<String, Weak<Portal>>>> = OnceLock::new();
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

fn portal_at(data_dir: &Path) -> Result<Arc<Portal>, String> {
    let absolute = std::path::absolute(data_dir).map_err(|error| error.to_string())?;
    let key = crate::path_key::physical_path_spelling(&absolute)
        .ok_or("invalid composition data directory")?;
    let mut portals = PORTALS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(portal) = portals.get(&key).and_then(Weak::upgrade) {
        return Ok(portal);
    }
    let portal = Arc::new(Portal {
        state: Mutex::new(PortalState::default()),
        released: Condvar::new(),
    });
    portals.insert(key, Arc::downgrade(&portal));
    Ok(portal)
}

#[derive(Clone)]
pub(crate) struct BookCompositionService {
    shared: Arc<Shared>,
    _portal: Arc<Portal>,
}
pub(crate) struct BookCompositionProcessOwner {
    shared: Arc<Shared>,
    _portal: Arc<Portal>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BookCompositionService {
    pub(crate) fn close_admission(&self) {
        let mut admission = self.shared.admission.lock().unwrap();
        admission.mode = Mode::Closed;
        admission.tx = None;
    }

    pub(crate) fn spawn_at(
        data_dir: PathBuf,
        observer: Option<ContentIdentityProducer>,
    ) -> Result<(Self, BookCompositionProcessOwner), String> {
        let data_dir = std::path::absolute(data_dir).map_err(|error| error.to_string())?;
        let portal = portal_at(&data_dir)?;
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            admission: Mutex::new(Admission {
                mode: Mode::Running {
                    outstanding_mutations: 0,
                },
                tx: Some(tx),
                snapshot: Arc::new(CompositionSnapshot {
                    status: Err("composition store initializing".into()),
                    rows: BTreeMap::new(),
                    epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
                    sequence: 0,
                    tokens: BTreeMap::new(),
                }),
                changed: Vec::new(),
                errors: Vec::new(),
            }),
        });
        let startup_wait = {
            let mut state = portal.state.lock().unwrap();
            if state.service.upgrade().is_some() {
                return Err("a composition service already owns this data directory".into());
            }
            state.service = Arc::downgrade(&shared);
            state.lease_generation
        };
        let worker_shared = Arc::clone(&shared);
        let worker_portal = Arc::clone(&portal);
        #[cfg(test)]
        let test_epoch = crate::page_edit_write_epoch::TestEpochScope::capture();
        let handle = std::thread::Builder::new()
            .name("book-composition-writer".into())
            .spawn(move || {
                #[cfg(test)]
                let _test_epoch =
                    test_epoch.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                run_worker(
                    data_dir,
                    worker_shared,
                    worker_portal,
                    startup_wait,
                    observer,
                    rx,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                shared: Arc::clone(&shared),
                _portal: Arc::clone(&portal),
            },
            BookCompositionProcessOwner {
                shared,
                _portal: portal,
                handle: Some(handle),
            },
        ))
    }

    pub(crate) fn snapshot(&self) -> Arc<CompositionSnapshot> {
        Arc::clone(&self.shared.admission.lock().unwrap().snapshot)
    }
    pub(crate) fn is_busy(&self) -> bool {
        let admission = self.shared.admission.lock().unwrap();
        !matches!(
            admission.mode,
            Mode::Running {
                outstanding_mutations: 0
            } | Mode::Closed
        )
    }

    /// A new presentation can consult this immutable snapshot only after every
    /// earlier mutation has completed. Reads retain the worker FIFO when unknown.
    /// This lock contains only admission/publication operations, never worker I/O.
    pub(crate) fn current_candidate_snapshot(&self) -> Option<Arc<CompositionSnapshot>> {
        let admission = self.shared.admission.lock().unwrap();
        (matches!(
            admission.mode,
            Mode::Running {
                outstanding_mutations: 0
            }
        ) && admission.snapshot.status.is_ok())
        .then(|| Arc::clone(&admission.snapshot))
    }
    pub(crate) fn drain_errors(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.admission.lock().unwrap().errors)
    }
    pub(crate) fn drain_changed_scope_keys(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.admission.lock().unwrap().changed)
    }

    pub(crate) fn prepare(
        &self,
        scope: BookScopeKey,
        domain: PageDomain,
    ) -> mpsc::Receiver<CompositionPrepareReply> {
        let (reply, rx) = mpsc::channel();
        let admission = self.shared.admission.lock().unwrap();
        let sent = admission.mode != Mode::Closed
            && admission.tx.as_ref().is_some_and(|tx| {
                tx.send(Command::Prepare(
                    scope.clone(),
                    domain.clone(),
                    reply.clone(),
                ))
                .is_ok()
            });
        if !sent {
            let _ = reply.send(CompositionPrepareReply {
                token: admission.snapshot.token_for(&scope),
                scope,
                domain,
                status: Err("composition service is closing".into()),
            });
        }
        rx
    }

    pub(crate) fn check_candidate(
        &self,
        correlation: super::CandidateCorrelation,
        candidate: super::CandidatePage,
    ) -> mpsc::Receiver<super::CandidatePermissionReply> {
        let (reply, rx) = mpsc::channel();
        let admission = self.shared.admission.lock().unwrap();
        if admission.mode == Mode::Closed
            || !admission.tx.as_ref().is_some_and(|tx| {
                tx.send(Command::Candidate(
                    correlation.clone(),
                    candidate,
                    reply.clone(),
                ))
                .is_ok()
            })
        {
            let _ = reply.send(super::CandidatePermissionReply {
                correlation,
                status: Err(super::CandidatePermissionError::Unavailable(
                    "composition service is closing".into(),
                )),
            });
        }
        rx
    }

    pub(crate) fn set_view_mode(
        &self,
        scope: BookScopeKey,
        mode: crate::settings::SpreadMode,
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (reply, rx) = mpsc::channel();
        self.send_view_command(Command::ViewMode(scope, mode, reply))?;
        Ok(rx)
    }
    pub(crate) fn save_view_trim(
        &self,
        batch: crate::view_trim_db::ViewTrimWriteBatch,
    ) -> mpsc::Receiver<Result<(), String>> {
        let (reply, rx) = mpsc::channel();
        let mut admission = self.shared.admission.lock().unwrap();
        let sent = matches!(admission.mode, Mode::Running { .. })
            && admission.tx.as_ref().is_some_and(|tx| {
                let write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
                tx.send(Command::ViewTrim(batch, write, reply.clone()))
                    .is_ok()
            });
        if sent {
            admission.admit_mutation();
        }
        if !sent {
            let _ = reply.send(Err(
                "view trim writes unavailable during maintenance or exit".into(),
            ));
        }
        rx
    }
    pub(crate) fn remove_view_trim_page(&self, key: String) -> mpsc::Receiver<Result<(), String>> {
        let (reply, rx) = mpsc::channel();
        let mut admission = self.shared.admission.lock().unwrap();
        let sent = matches!(admission.mode, Mode::Running { .. })
            && admission.tx.as_ref().is_some_and(|tx| {
                let write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
                tx.send(Command::RemoveViewTrimPage(key, write, reply.clone()))
                    .is_ok()
            });
        if sent {
            admission.admit_mutation();
        }
        if !sent {
            let _ = reply.send(Err(
                "view trim writes unavailable during maintenance or exit".into(),
            ));
        }
        rx
    }
    #[cfg(test)]
    pub(crate) fn hold_worker_for_test(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, receiver) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let admission = self.shared.admission.lock().unwrap();
        assert!(matches!(admission.mode, Mode::Running { .. }));
        admission
            .tx
            .as_ref()
            .unwrap()
            .send(Command::HoldWorker(entered, wait))
            .unwrap();
        (receiver, release)
    }
    pub(crate) fn set_view_trim_book(
        &self,
        key: crate::spread_db::SpreadContainerKey,
        state: crate::view_trim::ViewTrimBookState,
        admission_proof: super::CompositionWriteAdmission,
    ) -> mpsc::Receiver<Result<crate::view_trim::ViewTrimBookState, String>> {
        let (reply, rx) = mpsc::channel();
        let mut admission = self.shared.admission.lock().unwrap();
        let sent = matches!(admission.mode, Mode::Running { .. })
            && admission.tx.as_ref().is_some_and(|tx| {
                let write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin();
                tx.send(Command::ViewTrimBook(
                    key,
                    state,
                    admission_proof,
                    write,
                    reply.clone(),
                ))
                .is_ok()
            });
        if sent {
            admission.admit_mutation();
        }
        if !sent {
            let _ = reply.send(Err(
                "view trim writes unavailable during maintenance or exit".into(),
            ));
        }
        rx
    }
    pub(crate) fn set_view_flow(
        &self,
        scope: BookScopeKey,
        mode: crate::settings::SpreadMode,
        flow: crate::settings::ReadingFlow,
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (reply, rx) = mpsc::channel();
        self.send_view_command(Command::ViewFlow(scope, mode, flow, reply))?;
        Ok(rx)
    }
    fn send_view_command(&self, command: Command) -> Result<(), String> {
        let mut admission = self.shared.admission.lock().unwrap();
        if !matches!(admission.mode, Mode::Running { .. }) {
            return Err("view mode writes unavailable during maintenance or exit".into());
        }
        admission
            .tx
            .as_ref()
            .ok_or("composition worker stopped")?
            .send(command)
            .map_err(|error| error.to_string())?;
        admission.admit_mutation();
        Ok(())
    }

    pub(crate) fn save(
        &self,
        scope: BookScopeKey,
        domain: PageDomain,
        composition: BookComposition,
    ) -> mpsc::Receiver<CompositionWriteReply> {
        self.write(scope, domain, Mutation::Save(composition))
    }
    pub(crate) fn remove(
        &self,
        scope: BookScopeKey,
        domain: PageDomain,
    ) -> mpsc::Receiver<CompositionWriteReply> {
        self.write(scope, domain, Mutation::Remove)
    }

    #[cfg(test)]
    pub(crate) fn rename_book_root(&self, source: &Path, target: &Path) -> Result<(), String> {
        self.rename_book_root_with_collision(
            source,
            target,
            crate::rename_key_migration::PathMigrationCollision::DestinationWins,
        )
    }

    pub(crate) fn rename_book_root_with_collision(
        &self,
        source: &Path,
        target: &Path,
        collision: crate::rename_key_migration::PathMigrationCollision,
    ) -> Result<(), String> {
        self.maintain(MaintenanceAction::Rename(
            BookScopeKey::folder(source)?.storage_key(),
            BookScopeKey::folder(target)?.storage_key(),
            collision,
        ))
    }
    #[cfg(test)]
    pub(crate) fn purge_book_root(&self, root: &Path) -> Result<(), String> {
        self.maintain(MaintenanceAction::Purge(
            BookScopeKey::folder(root)?.storage_key(),
        ))
    }
    pub(crate) fn migrate_page_paths(
        &self,
        mappings: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), String> {
        self.maintain(MaintenanceAction::Pages(mappings))
    }
    fn maintain(&self, action: MaintenanceAction) -> Result<(), String> {
        let mut admission = self.shared.admission.lock().unwrap();
        if !matches!(admission.mode, Mode::Running { .. }) {
            return Err("composition writes unavailable during maintenance or exit".into());
        }
        admission
            .tx
            .as_ref()
            .ok_or("composition worker stopped")?
            .send(Command::Maintain(action))
            .map_err(|error| error.to_string())?;
        admission.admit_mutation();
        Ok(())
    }

    fn write(
        &self,
        scope: BookScopeKey,
        domain: PageDomain,
        mutation: Mutation,
    ) -> mpsc::Receiver<CompositionWriteReply> {
        let (reply, rx) = mpsc::channel();
        let mut admission = self.shared.admission.lock().unwrap();
        let sent = matches!(admission.mode, Mode::Running { .. })
            && admission.tx.as_ref().is_some_and(|tx| {
                tx.send(Command::Write(
                    scope.clone(),
                    domain,
                    mutation,
                    reply.clone(),
                ))
                .is_ok()
            });
        if sent {
            admission.admit_mutation();
        }
        if !sent {
            let _ = reply.send(CompositionWriteReply {
                token: admission.snapshot.token_for(&scope),
                scope,
                changed: false,
                status: Err("composition writes unavailable during maintenance or exit".into()),
            });
        }
        rx
    }
}

impl BookCompositionProcessOwner {
    /// Process-exit worker only: close admission and drain every already accepted request.
    pub(crate) fn shutdown_and_join(mut self) {
        self.close();
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            crate::logger::log("composition writer panicked during exit");
        }
        let mut portal = self._portal.state.lock().unwrap();
        if portal
            .service
            .upgrade()
            .is_some_and(|service| Arc::ptr_eq(&service, &self.shared))
        {
            portal.service = Weak::new();
        }
    }
    fn close(&self) {
        let mut admission = self.shared.admission.lock().unwrap();
        admission.mode = Mode::Closed;
        admission.tx = None;
    }
}
impl Drop for BookCompositionProcessOwner {
    fn drop(&mut self) {
        self.close();
    }
}

/// Acquire/drop only in an existing maintenance worker. ACK means the single
/// writable connection is closed; release reloads and diffs immutable snapshots.
pub(crate) struct BookCompositionMaintenanceLease {
    portal: Arc<Portal>,
    service: Option<(Arc<Shared>, mpsc::Sender<Command>)>,
}

pub(crate) fn maintenance_at(data_dir: &Path) -> Result<BookCompositionMaintenanceLease, String> {
    let portal = portal_at(data_dir)?;
    let service = {
        let mut state = portal.state.lock().unwrap();
        while state.lease_generation.is_some() {
            state = portal.released.wait(state).unwrap();
        }
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .expect("composition maintenance generation exhausted");
        state.lease_generation = Some(state.next_generation);
        state.service.upgrade()
    };
    let service = if let Some(service) = service {
        let (reply, rx) = mpsc::channel();
        let result = {
            let mut admission = service.admission.lock().unwrap();
            if admission.mode == Mode::Closed {
                Err("composition service is closing".to_owned())
            } else {
                admission.mode = Mode::Maintenance;
                admission
                    .tx
                    .as_ref()
                    .ok_or("composition worker stopped".to_owned())
                    .and_then(|tx| {
                        tx.send(Command::Begin(reply))
                            .map(|()| tx.clone())
                            .map_err(|error| error.to_string())
                    })
            }
        }
        .and_then(|tx| rx.recv().map(|()| tx).map_err(|error| error.to_string()));
        match result {
            Ok(tx) => Some((service, tx)),
            Err(error) => {
                portal.state.lock().unwrap().lease_generation = None;
                portal.released.notify_all();
                return Err(error);
            }
        }
    } else {
        None
    };
    Ok(BookCompositionMaintenanceLease { portal, service })
}

impl Drop for BookCompositionMaintenanceLease {
    fn drop(&mut self) {
        if let Some((_, tx)) = &self.service {
            let (reply, rx) = mpsc::channel();
            let result = tx
                .send(Command::End(reply))
                .map_err(|error| error.to_string())
                .and_then(|()| rx.recv().map_err(|error| error.to_string())?);
            if let Err(error) = result {
                crate::logger::log(format!("composition maintenance reload failed: {error}"));
            }
        }
        self.portal.state.lock().unwrap().lease_generation = None;
        self.portal.released.notify_all();
    }
}

fn publish(
    shared: &Shared,
    snapshot: &CompositionSnapshot,
    changed: Vec<String>,
    error: Option<String>,
) {
    let next = Arc::new(snapshot.clone());
    let previous = {
        let mut admission = shared.admission.lock().unwrap();
        admission.changed.extend(changed);
        if let Some(error) = error {
            admission.errors.push(error);
        }
        std::mem::replace(&mut admission.snapshot, next)
    };
    drop(previous); // Any large old record is released on the writer, outside the UI mutex.
}

fn block(state: &mut CompositionSnapshot, error: &str) {
    // An unreadable store cannot prove the prior accepted record (including
    // absence). Qualify all old read permits without announcing row mutations.
    if state.status.is_ok() {
        state.epoch = NEXT_EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    state.status = Err(error.to_owned());
}

fn reload(
    path: &Path,
    connection: &mut Option<rusqlite::Connection>,
    state: &mut CompositionSnapshot,
) -> Result<Vec<String>, String> {
    *connection = None;
    let opened = store::open_existing(path)?;
    let rows = match &opened {
        Some(connection) => store::load_rows(connection)?,
        None => BTreeMap::new(),
    };
    let mut changed: Vec<_> = state
        .rows
        .keys()
        .chain(rows.keys())
        .filter(|key| state.rows.get(*key).map(Arc::as_ref) != rows.get(*key))
        .cloned()
        .collect();
    changed.sort();
    changed.dedup();
    for key in &changed {
        state.sequence = state
            .sequence
            .checked_add(1)
            .expect("composition publication sequence exhausted");
        state.tokens.insert(key.clone(), state.sequence);
    }
    state.rows = rows
        .into_iter()
        .map(|(key, row)| {
            let record = state
                .rows
                .get(&key)
                .filter(|previous| previous.as_ref() == &row)
                .cloned()
                .unwrap_or_else(|| Arc::new(row));
            (key, record)
        })
        .collect();
    state.status = Ok(());
    *connection = opened;
    Ok(changed)
}

fn run_worker(
    data_dir: PathBuf,
    shared: Arc<Shared>,
    portal: Arc<Portal>,
    startup_wait: Option<u64>,
    observer: Option<ContentIdentityProducer>,
    rx: mpsc::Receiver<Command>,
) {
    if let Some(generation) = startup_wait {
        let mut state = portal.state.lock().unwrap();
        while state.lease_generation == Some(generation) {
            state = portal.released.wait(state).unwrap();
        }
    }
    let path = data_dir.join(store::DB_FILE);
    let mut state = (*shared.admission.lock().unwrap().snapshot).clone();
    let mut connection = None;
    // Startup adopts existing rows without publishing mutation notifications.
    if let Err(error) = reload(&path, &mut connection, &mut state) {
        block(&mut state, &error);
        publish(&shared, &state, Vec::new(), Some(error));
    } else {
        publish(&shared, &state, Vec::new(), None);
    }
    let mut maintenance = false;
    let mut deferred = VecDeque::new();
    loop {
        let command = if !maintenance && !deferred.is_empty() {
            deferred.pop_front().unwrap()
        } else {
            match rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };
        match command {
            #[cfg(test)]
            Command::HoldWorker(entered, wait) => {
                let _ = entered.send(());
                let _ = wait.recv();
            }
            Command::ViewTrim(batch, write, reply) => {
                let result =
                    crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
                        .and_then(|mut db| db.apply_write_batch(&batch))
                        .map_err(|error| error.to_string());
                drop(write);
                if let Err(error) = &result {
                    publish(&shared, &state, Vec::new(), Some(error.clone()));
                }
                finish_mutation(&shared);
                let _ = reply.send(result);
            }
            Command::RemoveViewTrimPage(key, write, reply) => {
                let result =
                    crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
                        .and_then(|db| db.remove_page_override(&key))
                        .map_err(|error| error.to_string());
                drop(write);
                if let Err(error) = &result {
                    publish(&shared, &state, Vec::new(), Some(error.clone()));
                }
                finish_mutation(&shared);
                let _ = reply.send(result);
            }
            Command::ViewTrimBook(key, mut value, admission, write, reply) => {
                let result: Result<crate::view_trim::ViewTrimBookState, String> = (|| {
                    admission.validate(&state)?;
                    value.apply_mode =
                        crate::view_trim::view_trim_base_apply_mode(value.apply_mode);
                    value.book_settings = value.book_settings.clamped();
                    let db =
                        crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db"))
                            .map_err(|error| error.to_string())?;
                    let previous_separate = db
                        .get_book_state(&key.exact)
                        .or_else(|| {
                            key.fallback
                                .as_deref()
                                .and_then(|fallback| db.get_book_state(fallback))
                        })
                        .unwrap_or_default()
                        .book_settings
                        .spread_separate;
                    if previous_separate != value.book_settings.spread_separate {
                        let separate = value.book_settings.spread_separate;
                        value.book_settings.spread_separate = previous_separate;
                        value.book_settings = value.book_settings.with_spread_separate(separate);
                    }
                    db.set_book_state(&key.exact, value)
                        .map_err(|error| error.to_string())?;
                    Ok(value)
                })(
                );
                drop(write);
                if let Err(error) = &result {
                    publish(&shared, &state, Vec::new(), Some(error.clone()));
                }
                finish_mutation(&shared);
                let _ = reply.send(result);
            }
            Command::ViewMode(scope, mode, reply) => {
                let result = (|| {
                    scope.validate()?;
                    let segments: Vec<_> = scope
                        .level_prefix
                        .split('/')
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect();
                    let key = crate::spread_db::container_key(&scope.logical_root, &segments);
                    crate::spread_db::SpreadDb::open_at(&data_dir.join("spread.db"))
                        .map_err(|error| error.to_string())?
                        .set_mode_only(&key, mode)
                        .map_err(|error| error.to_string())
                })();
                if let Err(error) = &result {
                    publish(&shared, &state, Vec::new(), Some(error.clone()));
                }
                finish_mutation(&shared);
                let _ = reply.send(result);
            }
            Command::ViewFlow(scope, mode, flow, reply) => {
                let result = (|| {
                    scope.validate()?;
                    let segments: Vec<_> = scope
                        .level_prefix
                        .split('/')
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect();
                    let key = crate::spread_db::container_key(&scope.logical_root, &segments);
                    crate::spread_db::SpreadDb::open_at(&data_dir.join("spread.db"))
                        .map_err(|error| error.to_string())?
                        .set_flow_only(&key, mode, flow)
                        .map_err(|error| error.to_string())
                })();
                if let Err(error) = &result {
                    publish(&shared, &state, Vec::new(), Some(error.clone()));
                }
                finish_mutation(&shared);
                let _ = reply.send(result);
            }
            Command::Begin(reply) => {
                connection = None;
                maintenance = true;
                let _ = reply.send(());
            }
            Command::End(reply) => {
                let result = reload(&path, &mut connection, &mut state);
                let (changed, error) = match &result {
                    Ok(changed) => (changed.clone(), None),
                    Err(error) => {
                        block(&mut state, error);
                        (Vec::new(), Some(error.clone()))
                    }
                };
                publish(&shared, &state, changed, error);
                maintenance = false;
                {
                    let mut admission = shared.admission.lock().unwrap();
                    if admission.mode != Mode::Closed {
                        admission.mode = Mode::Running {
                            outstanding_mutations: 0,
                        };
                    }
                }
                let _ = reply.send(result.map(|_| ()));
            }
            command @ Command::Prepare(..) if maintenance => deferred.push_back(command),
            command @ Command::Candidate(..) if maintenance => deferred.push_back(command),
            Command::Candidate(correlation, candidate, reply) => {
                let status = super::candidate_permission(&state, &candidate);
                let _ = reply.send(super::CandidatePermissionReply {
                    correlation,
                    status,
                });
            }
            Command::Prepare(scope, mut domain, reply) => {
                let status = (|| {
                    state.status.clone()?;
                    domain = store::canonical_domain(&domain)?;
                    store::validate_scope_domain(&scope, &domain)?;
                    let row = state
                        .rows
                        .get(&scope.storage_key())
                        .map(|row| row.as_ref().clone());
                    if row.as_ref().is_some_and(|row| row.domain != domain) {
                        Err("saved composition kind does not match source".into())
                    } else {
                        Ok(row)
                    }
                })();
                let _ = reply.send(CompositionPrepareReply {
                    token: state.token_for(&scope),
                    scope,
                    domain,
                    status,
                });
            }
            Command::Maintain(action) => {
                let result = (|| {
                    state.status.clone()?;
                    match action {
                        MaintenanceAction::Rename(source, target, collision) => {
                            if let Some(connection) = connection.as_mut() {
                                store::rename_book_root_with_collision(
                                    connection, &source, &target, collision,
                                )?;
                            }
                        }
                        #[cfg(test)]
                        MaintenanceAction::Purge(root) => {
                            if let Some(connection) = connection.as_mut() {
                                store::purge_book_root(connection, &root)?;
                            }
                        }
                        MaintenanceAction::Pages(mappings) => {
                            connection = None;
                            store::migrate_page_paths_at(&data_dir, &mappings)?;
                        }
                    }
                    reload(&path, &mut connection, &mut state)
                })();
                match result {
                    Ok(changed) => publish(&shared, &state, changed, None),
                    Err(error) => {
                        block(&mut state, &error);
                        publish(&shared, &state, Vec::new(), Some(error));
                    }
                }
                finish_mutation(&shared);
            }
            Command::Write(scope, mut domain, mutation, reply) => {
                let key = scope.storage_key();
                let result = (|| {
                    state.status.clone()?;
                    domain = store::canonical_domain(&domain)?;
                    store::validate_scope_domain(&scope, &domain)?;
                    if state.rows.get(&key).is_some_and(|row| row.domain != domain) {
                        return Err("saved composition kind does not match source".to_owned());
                    }
                    if let Mutation::Save(composition) = &mutation {
                        composition
                            .validate(&domain)
                            .map_err(|error| error.to_string())?;
                    }
                    if connection.is_none() {
                        if matches!(mutation, Mutation::Remove) {
                            return Ok(false);
                        }
                        store::initialize_database_at(&path)?;
                        connection = store::open_existing(&path)?;
                    }
                    let connection = connection
                        .as_mut()
                        .ok_or("composition writer unavailable")?;
                    let transaction = connection
                        .transaction()
                        .map_err(|error| error.to_string())?;
                    if let Err(error) = store::validate_database_rows(&transaction) {
                        block(&mut state, &error);
                        return Err(error);
                    }
                    let changed = match &mutation {
                        Mutation::Save(composition) => store::replace_row_in(
                            &transaction,
                            "book_compositions",
                            &key,
                            &domain,
                            composition,
                        )?,
                        Mutation::Remove => {
                            store::remove_row_in(&transaction, "book_compositions", &key)?
                        }
                    };
                    transaction.commit().map_err(|error| error.to_string())?;
                    Ok(changed)
                })();
                let (changed, status) = match result {
                    Ok(false) => (false, Ok(())),
                    Ok(true) => {
                        if matches!(mutation, Mutation::Save(_))
                            && domain != PageDomain::ImageFolder
                            && let Some(observer) = &observer
                        {
                            observer
                                .record_for_path(&scope.logical_root, ContentIdentityTrigger::Edit);
                        }
                        match reload(&path, &mut connection, &mut state) {
                            Ok(keys) => {
                                publish(&shared, &state, keys, None);
                                (true, Ok(()))
                            }
                            Err(error) => {
                                block(&mut state, &error);
                                publish(&shared, &state, vec![key.clone()], Some(error.clone()));
                                (true, Err(error))
                            }
                        }
                    }
                    Err(error) => {
                        crate::logger::log(format!("composition write failed: {error}"));
                        publish(&shared, &state, Vec::new(), Some(error.clone()));
                        (false, Err(error))
                    }
                };
                finish_mutation(&shared);
                let _ = reply.send(CompositionWriteReply {
                    scope,
                    changed,
                    token: state.token_for_key(&key),
                    status,
                });
            }
        }
    }
}
