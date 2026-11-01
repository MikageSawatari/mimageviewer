//! Bounded, process-lifetime AI change records and an asynchronous UI notice seam.
use std::sync::{Arc, Mutex, mpsc};
use std::time::SystemTime;

pub(crate) const MAX_CHANGE_CALLS: usize = 100;
pub(crate) const MAX_CHANGE_ROWS: usize = 500;
pub(crate) type SharedChangeRecords = Arc<Mutex<ChangeRecords>>;
pub(crate) type UiReceiver = mpsc::Receiver<String>;

/// App owns the installed receiver and its controller as one optional value.
pub(crate) struct UiChanges {
    pub(crate) controller: Arc<ChangeController>,
    pub(crate) notices: UiReceiver,
}

impl Drop for UiChanges {
    fn drop(&mut self) {
        *self.controller.ui.lock().unwrap() = UiState::Uninstalled;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct ChangeRecordId(u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChangeCounts {
    pub(crate) changed: usize,
    pub(crate) unchanged: usize,
    pub(crate) skipped: usize,
    pub(crate) queued: usize,
    pub(crate) failed: usize,
}

impl ChangeCounts {
    pub(crate) fn total(&self) -> usize {
        self.changed + self.unchanged + self.skipped + self.queued + self.failed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChangeRowOutcome {
    Changed,
    Unchanged,
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChangeRowResult {
    #[default]
    Pending,
    Done(ChangeRowOutcome),
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChangeRow {
    pub(crate) item: String,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) result: ChangeRowResult,
}

#[derive(Clone, Debug)]
pub(crate) struct CollectionTarget {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeRecord {
    pub(crate) id: ChangeRecordId,
    pub(crate) at: SystemTime,
    pub(crate) client_name: String,
    pub(crate) tool: String,
    pub(crate) collection: Option<CollectionTarget>,
    pub(crate) counts: ChangeCounts,
    pub(crate) summary: String,
    pub(crate) rows: Vec<ChangeRow>,
    pub(crate) notice: String,
}

#[derive(Default)]
pub(crate) struct ChangeRecords {
    next_id: u64,
    entries: Vec<Arc<ChangeRecord>>,
}

impl ChangeRecords {
    pub(crate) fn entries(&self) -> &[Arc<ChangeRecord>] {
        &self.entries
    }

    fn push(&mut self, mut record: ChangeRecord) -> ChangeRecordId {
        self.next_id += 1;
        record.id = ChangeRecordId(self.next_id);
        record.rows.truncate(MAX_CHANGE_ROWS);
        let id = record.id;
        self.entries.insert(0, Arc::new(record));
        self.entries.truncate(MAX_CHANGE_CALLS);
        id
    }
}

struct InstalledUi {
    sender: mpsc::Sender<String>,
    ctx: egui::Context,
}

enum UiState {
    Uninstalled,
    Starting(InstalledUi),
    Ready(InstalledUi),
}

pub(crate) struct ChangeController {
    records: SharedChangeRecords,
    ui: Mutex<UiState>,
}

impl ChangeController {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            records: Arc::new(Mutex::new(ChangeRecords::default())),
            ui: Mutex::new(UiState::Uninstalled),
        })
    }

    pub(crate) fn records(&self) -> SharedChangeRecords {
        self.records.clone()
    }

    pub(crate) fn install(&self, ctx: egui::Context) -> UiReceiver {
        let (sender, receiver) = mpsc::channel();
        *self.ui.lock().unwrap() = UiState::Starting(InstalledUi { sender, ctx });
        receiver
    }

    /// Only App publishes readiness, before any startup early return.
    pub(crate) fn set_ready(&self, ready: bool) {
        let mut state = self.ui.lock().unwrap();
        let installed = match std::mem::replace(&mut *state, UiState::Uninstalled) {
            UiState::Uninstalled => return,
            UiState::Starting(installed) | UiState::Ready(installed) => installed,
        };
        *state = if ready {
            UiState::Ready(installed)
        } else {
            UiState::Starting(installed)
        };
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(*self.ui.lock().unwrap(), UiState::Ready(_))
    }

    /// Called once after acceptance. The record is visible before the notice.
    pub(crate) fn publish(&self, record: ChangeRecord) -> ChangeRecordId {
        let notice = record.notice.clone();
        let id = self.records.lock().unwrap().push(record);
        if let UiState::Starting(installed) | UiState::Ready(installed) = &*self.ui.lock().unwrap()
        {
            let _ = installed.sender.send(notice);
            installed.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        id
    }

    /// Completes a queued record in place without producing another call/toast.
    /// Evicted records stay evicted; late completion never revives an old call.
    pub(crate) fn update(
        &self,
        id: ChangeRecordId,
        update: impl FnOnce(&mut ChangeRecord),
    ) -> bool {
        {
            let mut records = self.records.lock().unwrap();
            let Some(record) = records.entries.iter_mut().find(|record| record.id == id) else {
                return false;
            };
            let record = Arc::make_mut(record);
            update(record);
            record.rows.truncate(MAX_CHANGE_ROWS);
        }
        if let UiState::Starting(installed) | UiState::Ready(installed) = &*self.ui.lock().unwrap()
        {
            installed.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(index: usize) -> ChangeRecord {
        ChangeRecord {
            id: ChangeRecordId::default(),
            at: SystemTime::UNIX_EPOCH,
            client_name: format!("client{index}"),
            tool: "add_to_collection".into(),
            collection: None,
            counts: ChangeCounts {
                changed: 1,
                ..Default::default()
            },
            summary: "追加".into(),
            rows: vec![ChangeRow::default(); 501],
            notice: format!("notice{index}"),
        }
    }

    #[test]
    fn publish_and_update_wake_root_while_a_minimized_child_pass_is_current() {
        let controller = ChangeController::new();
        let ctx = egui::Context::default();
        let notices = controller.install(ctx.clone());
        let (repaints, repaint_requests) = mpsc::channel();
        ctx.set_repaint_observer(move |request| {
            repaints.send(request).unwrap();
        });
        let child = egui::ViewportId::from_hash_of("passive-child");
        let mut input = egui::RawInput {
            viewport_id: child,
            ..Default::default()
        };
        input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                minimized: Some(true),
                ..Default::default()
            },
        );
        ctx.run_pass(input, |ctx| {
            assert_eq!(ctx.viewport_id(), child);
            while repaint_requests.try_recv().is_ok() {}
            let worker_controller = controller.clone();
            let id = std::thread::spawn(move || worker_controller.publish(record(0)))
                .join()
                .unwrap();
            let wake = repaint_requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert_eq!(wake.viewport_id, egui::ViewportId::ROOT);
            assert_eq!(wake.delay, std::time::Duration::ZERO);
            assert_eq!(notices.try_recv().unwrap(), "notice0");

            let worker_controller = controller.clone();
            assert!(
                std::thread::spawn(move || worker_controller.update(id, |record| {
                    record.rows[0].result = ChangeRowResult::Done(ChangeRowOutcome::Changed);
                }))
                .join()
                .unwrap()
            );
            let wake = repaint_requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert_eq!(wake.viewport_id, egui::ViewportId::ROOT);
            assert_eq!(wake.delay, std::time::Duration::ZERO);
            assert!(notices.try_recv().is_err());
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn records_are_newest_first_and_bounded_at_both_owner_boundaries() {
        let controller = ChangeController::new();
        let oldest = controller.publish(record(0));
        for index in 1..101 {
            controller.publish(record(index));
        }
        let records = controller.records();
        let records = records.lock().unwrap();
        assert_eq!(records.entries().len(), 100);
        assert_eq!(records.entries()[0].client_name, "client100");
        assert_eq!(records.entries()[99].client_name, "client1");
        assert!(
            records
                .entries()
                .iter()
                .all(|record| record.rows.len() == 500)
        );
        drop(records);
        assert!(!controller.update(oldest, |_| panic!("evicted record must not be updated")));
    }

    #[test]
    fn install_readiness_and_queued_update_preserve_single_notification() {
        let controller = ChangeController::new();
        assert!(!controller.is_ready());
        controller.set_ready(true);
        assert!(!controller.is_ready());
        let receiver = controller.install(egui::Context::default());
        assert!(!controller.is_ready());
        controller.set_ready(true);
        assert!(controller.is_ready());
        let id = controller.publish(record(0));
        assert_eq!(receiver.try_recv().unwrap(), "notice0");
        let ui_snapshot = controller.records().lock().unwrap().entries()[0].clone();
        assert!(controller.update(id, |record| {
            record.rows[0].before = "前".into();
            record.rows[0].after = "後".into();
            record.rows[0].result = ChangeRowResult::Done(ChangeRowOutcome::Changed);
            record.rows.push(ChangeRow::default());
        }));
        assert!(receiver.try_recv().is_err());
        let shared = controller.records();
        let records = shared.lock().unwrap();
        assert_eq!(records.entries()[0].rows.len(), 500);
        assert_eq!(records.entries()[0].rows[0].before, "前");
        assert!(
            ui_snapshot.rows[0].before.is_empty(),
            "an adopted UI snapshot stays immutable"
        );
        drop(records);
        controller.set_ready(false);
        assert!(!controller.is_ready());
    }
}
