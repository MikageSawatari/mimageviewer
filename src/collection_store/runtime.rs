use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, select_biased, unbounded};

use super::db::CollectionStoreDb;
use super::{
    CollectionBatchAddOutcome, CollectionCatalogSnapshot, CollectionEntryId, CollectionId,
    CollectionMigrationOutcome, CollectionOrderMode, CollectionRegistration,
    CollectionRevisionNotice, CollectionSourceMigration, CollectionSourceMigrationBatch,
    CollectionStoreError,
};
use crate::settings::SortOrder;

const COMMAND_CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionRuntimeEvent {
    Ready(CollectionCatalogSnapshot),
    Failed(CollectionStoreError),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdmissionPhase {
    Starting,
    Running,
    Closing,
    Closed,
}

struct Admission {
    phase: AdmissionPhase,
    command_tx: Sender<Command>,
}

#[derive(Default)]
struct RevisionHub {
    latest: Option<CollectionRevisionNotice>,
    subscribers: Vec<Weak<RevisionWatchSlot>>,
}

struct RevisionWatchSlot {
    latest: Mutex<Option<CollectionRevisionNotice>>,
    wake_tx: Sender<()>,
}

#[derive(Clone)]
pub struct CollectionStoreClient {
    admission: Arc<Mutex<Admission>>,
    revision_hub: Arc<Mutex<RevisionHub>>,
}

pub struct CollectionRevisionWatch {
    slot: Arc<RevisionWatchSlot>,
    wake_rx: Receiver<()>,
}

impl CollectionRevisionWatch {
    pub fn take_latest(&self) -> Option<CollectionRevisionNotice> {
        while self.wake_rx.try_recv().is_ok() {}
        self.slot
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

pub struct CollectionStoreRuntime {
    client: CollectionStoreClient,
    control_tx: Sender<Control>,
    event_rx: Receiver<CollectionRuntimeEvent>,
    join: Option<JoinHandle<()>>,
    shutdown_ack: Option<Receiver<()>>,
}

impl CollectionStoreRuntime {
    /// `path` は App 統合側または test が明示する。module loadだけでは実profileを開かない。
    pub fn start_at(path: PathBuf) -> Result<Self, CollectionStoreError> {
        let (command_tx, command_rx) = bounded(COMMAND_CAPACITY);
        let (control_tx, control_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let admission = Arc::new(Mutex::new(Admission {
            phase: AdmissionPhase::Starting,
            command_tx,
        }));
        let revision_hub = Arc::new(Mutex::new(RevisionHub::default()));
        let actor_admission = Arc::clone(&admission);
        let actor_hub = Arc::clone(&revision_hub);
        let panic_admission = Arc::clone(&admission);
        let panic_events = event_tx.clone();
        let join = std::thread::Builder::new()
            .name("collection-store".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actor_main(
                        path,
                        command_rx,
                        control_rx,
                        event_tx,
                        actor_admission,
                        actor_hub,
                    )
                }));
                if result.is_err() {
                    set_phase(&panic_admission, AdmissionPhase::Closed);
                    let _ = panic_events.send(CollectionRuntimeEvent::Failed(
                        CollectionStoreError::Persistence(
                            "collection store actor terminated unexpectedly".into(),
                        ),
                    ));
                }
            })
            .map_err(|error| CollectionStoreError::Persistence(error.to_string()))?;
        Ok(Self {
            client: CollectionStoreClient {
                admission,
                revision_hub,
            },
            control_tx,
            event_rx,
            join: Some(join),
            shutdown_ack: None,
        })
    }

    pub fn client(&self) -> CollectionStoreClient {
        self.client.clone()
    }

    pub fn try_recv_event(&self) -> Option<CollectionRuntimeEvent> {
        self.event_rx.try_recv().ok()
    }

    /// request admission と同じmutex内でClosingへ遷移してからShutdownをpublishする。
    pub fn begin_shutdown(&mut self) {
        if self.shutdown_ack.is_some() {
            return;
        }
        let (ack_tx, ack_rx) = bounded(1);
        {
            let mut admission = self
                .client
                .admission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(
                admission.phase,
                AdmissionPhase::Closing | AdmissionPhase::Closed
            ) {
                return;
            }
            admission.phase = AdmissionPhase::Closing;
            let _ = self.control_tx.send(Control::Shutdown { ack: ack_tx });
        }
        self.shutdown_ack = Some(ack_rx);
    }

    /// App final exit用。通常frameから呼ばない。
    pub fn shutdown_and_join(mut self) {
        self.begin_shutdown();
        if let Some(ack) = self.shutdown_ack.take() {
            let _ = ack.recv();
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for CollectionStoreRuntime {
    fn drop(&mut self) {
        self.begin_shutdown();
        if self.join.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
        // 通常DropはUIを待たせない。unfinished JoinHandleはdetachされ、明示Shutdownがactorを止める。
    }
}

impl CollectionStoreClient {
    pub fn subscribe(&self) -> Result<CollectionRevisionWatch, CollectionStoreError> {
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match admission.phase {
            AdmissionPhase::Starting => return Err(CollectionStoreError::Starting),
            AdmissionPhase::Running => {}
            AdmissionPhase::Closing | AdmissionPhase::Closed => {
                return Err(CollectionStoreError::Unavailable);
            }
        }
        let (wake_tx, wake_rx) = bounded(1);
        let mut hub = self
            .revision_hub
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = Arc::new(RevisionWatchSlot {
            latest: Mutex::new(hub.latest.clone()),
            wake_tx,
        });
        if hub.latest.is_some() {
            let _ = slot.wake_tx.try_send(());
        }
        hub.subscribers.push(Arc::downgrade(&slot));
        Ok(CollectionRevisionWatch { slot, wake_rx })
    }

    pub fn list_catalog(
        &self,
    ) -> Result<
        Receiver<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::ListCatalog { reply })
    }

    pub fn load_collection(
        &self,
        id: CollectionId,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::LoadCollection { id, reply })
    }

    pub fn create_collection(
        &self,
        name: String,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::Create { name, reply })
    }

    pub fn rename_collection(
        &self,
        id: CollectionId,
        expected_revision: u64,
        name: String,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::Rename {
            id,
            expected_revision,
            name,
            reply,
        })
    }

    pub fn delete_collection(
        &self,
        id: CollectionId,
        expected_revision: u64,
    ) -> Result<
        Receiver<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::Delete {
            id,
            expected_revision,
            reply,
        })
    }

    pub fn set_order(
        &self,
        id: CollectionId,
        expected_revision: u64,
        mode: CollectionOrderMode,
        standard_sort: SortOrder,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::SetOrder {
            id,
            expected_revision,
            mode,
            standard_sort,
            reply,
        })
    }

    pub fn add_batch(
        &self,
        id: CollectionId,
        expected_revision: u64,
        registrations: Vec<CollectionRegistration>,
    ) -> Result<
        Receiver<Result<CollectionBatchAddOutcome, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::AddBatch {
            id,
            expected_revision,
            registrations,
            reply,
        })
    }

    pub fn remove_entries(
        &self,
        id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::RemoveEntries {
            id,
            expected_revision,
            entry_ids,
            reply,
        })
    }

    pub fn reorder_manual(
        &self,
        id: CollectionId,
        expected_revision: u64,
        order: Vec<CollectionEntryId>,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::ReorderManual {
            id,
            expected_revision,
            order,
            reply,
        })
    }

    pub fn relink(
        &self,
        id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
        registration: CollectionRegistration,
    ) -> Result<
        Receiver<Result<super::CollectionSnapshot, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::Relink {
            id,
            expected_revision,
            entry_id,
            registration,
            reply,
        })
    }

    pub fn migrate_sources(
        &self,
        migration: CollectionSourceMigration,
    ) -> Result<
        Receiver<Result<CollectionMigrationOutcome, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::MigrateSources { migration, reply })
    }

    pub fn migrate_source_batch(
        &self,
        batch: CollectionSourceMigrationBatch,
    ) -> Result<
        Receiver<Result<CollectionMigrationOutcome, CollectionStoreError>>,
        CollectionStoreError,
    > {
        self.request(|reply| Command::MigrateSourceBatch { batch, reply })
    }

    #[cfg(test)]
    pub(super) fn test_barrier(
        &self,
    ) -> Result<
        (
            Receiver<Result<(), CollectionStoreError>>,
            Receiver<()>,
            Sender<()>,
        ),
        CollectionStoreError,
    > {
        let (entered_tx, entered_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let reply = self.request(|reply| Command::TestBarrier {
            entered: entered_tx,
            release: release_rx,
            reply,
        })?;
        Ok((reply, entered_rx, release_tx))
    }

    #[cfg(test)]
    pub(super) fn test_panic(&self) -> Result<(), CollectionStoreError> {
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match admission.phase {
            AdmissionPhase::Running => match admission.command_tx.try_send(Command::TestPanic) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => Err(CollectionStoreError::Busy),
                Err(TrySendError::Disconnected(_)) => Err(CollectionStoreError::Unavailable),
            },
            AdmissionPhase::Starting => Err(CollectionStoreError::Starting),
            AdmissionPhase::Closing | AdmissionPhase::Closed => {
                Err(CollectionStoreError::Unavailable)
            }
        }
    }

    fn request<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, CollectionStoreError>>) -> Command,
    ) -> Result<Receiver<Result<T, CollectionStoreError>>, CollectionStoreError> {
        let (reply, receiver) = bounded(1);
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match admission.phase {
            AdmissionPhase::Starting => Err(CollectionStoreError::Starting),
            AdmissionPhase::Running => match admission.command_tx.try_send(build(reply)) {
                Ok(()) => Ok(receiver),
                Err(TrySendError::Full(_)) => Err(CollectionStoreError::Busy),
                Err(TrySendError::Disconnected(_)) => Err(CollectionStoreError::Unavailable),
            },
            AdmissionPhase::Closing | AdmissionPhase::Closed => {
                Err(CollectionStoreError::Unavailable)
            }
        }
    }
}

enum Control {
    Shutdown { ack: Sender<()> },
}

enum Command {
    ListCatalog {
        reply: Sender<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
    },
    LoadCollection {
        id: CollectionId,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    Create {
        name: String,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    Rename {
        id: CollectionId,
        expected_revision: u64,
        name: String,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    Delete {
        id: CollectionId,
        expected_revision: u64,
        reply: Sender<Result<CollectionCatalogSnapshot, CollectionStoreError>>,
    },
    SetOrder {
        id: CollectionId,
        expected_revision: u64,
        mode: CollectionOrderMode,
        standard_sort: SortOrder,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    AddBatch {
        id: CollectionId,
        expected_revision: u64,
        registrations: Vec<CollectionRegistration>,
        reply: Sender<Result<CollectionBatchAddOutcome, CollectionStoreError>>,
    },
    RemoveEntries {
        id: CollectionId,
        expected_revision: u64,
        entry_ids: Vec<CollectionEntryId>,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    ReorderManual {
        id: CollectionId,
        expected_revision: u64,
        order: Vec<CollectionEntryId>,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    Relink {
        id: CollectionId,
        expected_revision: u64,
        entry_id: CollectionEntryId,
        registration: CollectionRegistration,
        reply: Sender<Result<super::CollectionSnapshot, CollectionStoreError>>,
    },
    MigrateSources {
        migration: CollectionSourceMigration,
        reply: Sender<Result<CollectionMigrationOutcome, CollectionStoreError>>,
    },
    MigrateSourceBatch {
        batch: CollectionSourceMigrationBatch,
        reply: Sender<Result<CollectionMigrationOutcome, CollectionStoreError>>,
    },
    #[cfg(test)]
    TestBarrier {
        entered: Sender<()>,
        release: Receiver<()>,
        reply: Sender<Result<(), CollectionStoreError>>,
    },
    #[cfg(test)]
    TestPanic,
}

impl Command {
    fn reject(self) {
        match self {
            Self::ListCatalog { reply } | Self::Delete { reply, .. } => {
                let _ = reply.send(Err(CollectionStoreError::Unavailable));
            }
            Self::LoadCollection { reply, .. }
            | Self::Create { reply, .. }
            | Self::Rename { reply, .. }
            | Self::SetOrder { reply, .. }
            | Self::RemoveEntries { reply, .. }
            | Self::ReorderManual { reply, .. }
            | Self::Relink { reply, .. } => {
                let _ = reply.send(Err(CollectionStoreError::Unavailable));
            }
            Self::AddBatch { reply, .. } => {
                let _ = reply.send(Err(CollectionStoreError::Unavailable));
            }
            Self::MigrateSources { reply, .. } | Self::MigrateSourceBatch { reply, .. } => {
                let _ = reply.send(Err(CollectionStoreError::Unavailable));
            }
            #[cfg(test)]
            Self::TestBarrier { reply, .. } => {
                let _ = reply.send(Err(CollectionStoreError::Unavailable));
            }
            #[cfg(test)]
            Self::TestPanic => {}
        }
    }
}

fn actor_main(
    path: PathBuf,
    command_rx: Receiver<Command>,
    control_rx: Receiver<Control>,
    event_tx: Sender<CollectionRuntimeEvent>,
    admission: Arc<Mutex<Admission>>,
    revision_hub: Arc<Mutex<RevisionHub>>,
) {
    let mut db = match CollectionStoreDb::open_at(&path) {
        Ok(db) => db,
        Err(error) => {
            set_phase(&admission, AdmissionPhase::Closed);
            let _ = event_tx.send(CollectionRuntimeEvent::Failed(error));
            return;
        }
    };
    let catalog = match db.catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            set_phase(&admission, AdmissionPhase::Closed);
            let _ = event_tx.send(CollectionRuntimeEvent::Failed(error));
            return;
        }
    };
    let entered_running = {
        let mut state = admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.phase == AdmissionPhase::Starting {
            state.phase = AdmissionPhase::Running;
            true
        } else {
            false
        }
    };
    if entered_running {
        publish_revision(
            &revision_hub,
            CollectionRevisionNotice::from_catalog(&catalog),
        );
        let _ = event_tx.send(CollectionRuntimeEvent::Ready(catalog));
    }

    let shutdown_ack = loop {
        select_biased! {
            recv(control_rx) -> control => match control {
                Ok(Control::Shutdown { ack }) => break Some(ack),
                Err(_) => break None,
            },
            recv(command_rx) -> command => match command {
                Ok(command) if entered_running => process_command(command, &mut db, &revision_hub),
                Ok(command) => command.reject(),
                Err(_) => break None,
            }
        }
    };
    while let Ok(command) = command_rx.try_recv() {
        command.reject();
    }
    drop(db);
    set_phase(&admission, AdmissionPhase::Closed);
    let _ = event_tx.send(CollectionRuntimeEvent::Closed);
    if let Some(ack) = shutdown_ack {
        let _ = ack.send(());
    }
}

fn process_command(
    command: Command,
    db: &mut CollectionStoreDb,
    revision_hub: &Arc<Mutex<RevisionHub>>,
) {
    let mut mutated = false;
    match command {
        Command::ListCatalog { reply } => {
            let _ = reply.send(db.catalog());
        }
        Command::LoadCollection { id, reply } => {
            let _ = reply.send(db.snapshot(id));
        }
        Command::Create { name, reply } => {
            let result = db.create_collection(&name);
            mutated = result.is_ok();
            let _ = reply.send(result);
        }
        Command::Rename {
            id,
            expected_revision,
            name,
            reply,
        } => {
            let before = db.snapshot(id).ok().map(|snapshot| snapshot.revision());
            let result = db.rename_collection(id, expected_revision, &name);
            mutated = result
                .as_ref()
                .is_ok_and(|snapshot| Some(snapshot.revision()) != before);
            let _ = reply.send(result);
        }
        Command::Delete {
            id,
            expected_revision,
            reply,
        } => {
            let result = db.delete_collection(id, expected_revision);
            mutated = result.is_ok();
            let _ = reply.send(result);
        }
        Command::SetOrder {
            id,
            expected_revision,
            mode,
            standard_sort,
            reply,
        } => {
            let before = db.snapshot(id).ok().map(|snapshot| snapshot.revision());
            let result = db.set_order(id, expected_revision, mode, standard_sort);
            mutated = result
                .as_ref()
                .is_ok_and(|snapshot| Some(snapshot.revision()) != before);
            let _ = reply.send(result);
        }
        Command::AddBatch {
            id,
            expected_revision,
            registrations,
            reply,
        } => {
            let result = db.add_batch(id, expected_revision, registrations);
            mutated = result
                .as_ref()
                .is_ok_and(|outcome| !outcome.added.is_empty());
            let _ = reply.send(result);
        }
        Command::RemoveEntries {
            id,
            expected_revision,
            entry_ids,
            reply,
        } => {
            let before = db.snapshot(id).ok().map(|snapshot| snapshot.revision());
            let result = db.remove_entries(id, expected_revision, entry_ids);
            mutated = result
                .as_ref()
                .is_ok_and(|snapshot| Some(snapshot.revision()) != before);
            let _ = reply.send(result);
        }
        Command::ReorderManual {
            id,
            expected_revision,
            order,
            reply,
        } => {
            let before = db.snapshot(id).ok().map(|snapshot| snapshot.revision());
            let result = db.reorder_manual(id, expected_revision, order);
            mutated = result
                .as_ref()
                .is_ok_and(|snapshot| Some(snapshot.revision()) != before);
            let _ = reply.send(result);
        }
        Command::Relink {
            id,
            expected_revision,
            entry_id,
            registration,
            reply,
        } => {
            let before = db.snapshot(id).ok().map(|snapshot| snapshot.revision());
            let result = db.relink(id, expected_revision, entry_id, registration);
            mutated = result
                .as_ref()
                .is_ok_and(|snapshot| Some(snapshot.revision()) != before);
            let _ = reply.send(result);
        }
        Command::MigrateSources { migration, reply } => {
            let result = db.migrate_sources(migration);
            mutated = result
                .as_ref()
                .is_ok_and(|outcome| outcome.updated_entries != 0);
            let _ = reply.send(result);
        }
        Command::MigrateSourceBatch { batch, reply } => {
            let result = db.migrate_source_batch(batch);
            mutated = result
                .as_ref()
                .is_ok_and(|outcome| outcome.updated_entries != 0);
            let _ = reply.send(result);
        }
        #[cfg(test)]
        Command::TestBarrier {
            entered,
            release,
            reply,
        } => {
            let _ = entered.send(());
            let result = release
                .recv()
                .map_err(|_| CollectionStoreError::Unavailable)
                .map(|_| ());
            let _ = reply.send(result);
        }
        #[cfg(test)]
        Command::TestPanic => panic!("collection actor panic test seam"),
    }
    if mutated && let Ok(catalog) = db.catalog() {
        publish_revision(
            revision_hub,
            CollectionRevisionNotice::from_catalog(&catalog),
        );
    }
}

fn publish_revision(hub: &Arc<Mutex<RevisionHub>>, notice: CollectionRevisionNotice) {
    let subscribers = {
        let mut hub = hub.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        hub.latest = Some(notice.clone());
        let mut subscribers = Vec::new();
        hub.subscribers.retain(|subscriber| {
            if let Some(subscriber) = subscriber.upgrade() {
                subscribers.push(subscriber);
                true
            } else {
                false
            }
        });
        subscribers
    };
    for subscriber in subscribers {
        *subscriber
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(notice.clone());
        let _ = subscriber.wake_tx.try_send(());
    }
}

fn set_phase(admission: &Arc<Mutex<Admission>>, phase: AdmissionPhase) {
    admission
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .phase = phase;
}
