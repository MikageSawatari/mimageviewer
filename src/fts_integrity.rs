//! One manager-owned exit for failures of the rebuildable item index.
//! No retry, runtime wipe, recovery worker, or writer ownership lives here.

use std::fmt::{Debug, Display};
use std::sync::{Arc, Mutex};

use crate::fts_meta::FtsMetaDb;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaintenanceState {
    #[default]
    Ready,
    /// Projection of the existing durable rebuild marker, never cleared by later ingest.
    RebuildOnRestart,
    /// Reservation failed; do not promise a rebuild on restart.
    RebuildRequestFailed,
}

pub const UPDATE_FAILURE_NOTICE: &str =
    "索引の更新に失敗しました。mIV を再起動すると索引を作り直します。";
pub const REBUILD_REQUEST_FAILURE_NOTICE: &str =
    "索引の更新に失敗しました。再構築を予約できませんでした。ログを確認してください。";

#[derive(Default)]
struct View {
    maintenance: MaintenanceState,
    notifications: Vec<&'static str>,
}

pub struct FailureReporter {
    meta: Arc<FtsMetaDb>,
    view: Mutex<View>,
}

impl FailureReporter {
    /// Called on the initialization worker, before dispatcher/runtime publication.
    pub fn new(meta: Arc<FtsMetaDb>) -> Arc<Self> {
        let mut view = View::default();
        match meta.tantivy_rebuild_pending() {
            Ok(true) => view.maintenance = MaintenanceState::RebuildOnRestart,
            Ok(false) => {}
            Err(error) => {
                crate::logger::log(format!(
                    "fts integrity: read rebuild marker failed: {error:?}"
                ));
                view.maintenance = MaintenanceState::RebuildRequestFailed;
                view.notifications.push(REBUILD_REQUEST_FAILURE_NOTICE);
            }
        }
        Arc::new(Self {
            meta,
            view: Mutex::new(view),
        })
    }

    /// Called only at a terminal result boundary after Directory/writer/DB locks are released.
    pub fn report<E: Display + Debug + ?Sized>(&self, origin: &str, error: &E) {
        crate::logger::log(format!(
            "fts integrity: {origin} failed: {error}; detail={error:?}"
        ));
        // This method owns and releases the SQLite lock before touching the short UI view.
        let reservation = self.meta.request_item_index_rebuild();
        let (maintenance, notice) = match reservation {
            Ok(()) => (MaintenanceState::RebuildOnRestart, UPDATE_FAILURE_NOTICE),
            Err(error) => {
                crate::logger::log(format!(
                    "fts integrity: {origin}: rebuild reservation failed: {error}; detail={error:?}"
                ));
                (
                    MaintenanceState::RebuildRequestFailed,
                    REBUILD_REQUEST_FAILURE_NOTICE,
                )
            }
        };
        let mut view = self.view.lock().unwrap();
        view.maintenance = maintenance;
        view.notifications.push(notice);
    }

    pub fn snapshot(&self) -> MaintenanceState {
        self.view.lock().unwrap().maintenance
    }

    pub fn take_notifications(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.view.lock().unwrap().notifications)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_reserves_durable_rebuild_and_invalidates_scan_without_wiping_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Arc::new(FtsMetaDb::open_at(&tmp.path().join("fts_meta.db")).unwrap());
        meta.upsert_meta_ok(
            "c:/pictures/x.jpg",
            uuid::Uuid::new_v4(),
            std::path::Path::new("c:/pictures"),
            crate::fts_index::IndexKind::Image,
            1,
            1,
        )
        .unwrap();
        meta.mark_scanned_once("c:/pictures", "fixture").unwrap();
        let reporter = FailureReporter::new(meta.clone());
        reporter.report("test commit", &std::io::Error::from_raw_os_error(5));
        assert!(meta.tantivy_rebuild_pending().unwrap());
        assert!(meta.get("c:/pictures/x.jpg").unwrap().is_some());
        assert_eq!(meta.scanned_once("c:/pictures").unwrap(), None);
        assert_eq!(reporter.snapshot(), MaintenanceState::RebuildOnRestart);
        assert_eq!(reporter.take_notifications(), vec![UPDATE_FAILURE_NOTICE]);
        assert!(reporter.take_notifications().is_empty());
        assert_eq!(reporter.snapshot(), MaintenanceState::RebuildOnRestart);
    }

    #[test]
    fn reservation_failure_notifies_without_claiming_pending_or_resetting_stored_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fts_meta.db");
        let meta = Arc::new(FtsMetaDb::open_at(&path).unwrap());
        meta.mark_scanned_once("c:/pictures", "fixture").unwrap();
        let reporter = FailureReporter::new(meta.clone());
        let observer = rusqlite::Connection::open(&path).unwrap();
        observer.execute_batch("CREATE TRIGGER fail_reservation BEFORE INSERT ON index_state BEGIN SELECT RAISE(FAIL,'injected reservation failure'); END;").unwrap();
        reporter.report("test SQL", &"failure");
        assert!(!meta.tantivy_rebuild_pending().unwrap());
        assert_eq!(
            meta.scanned_once("c:/pictures").unwrap().as_deref(),
            Some("fixture")
        );
        assert_eq!(reporter.snapshot(), MaintenanceState::RebuildRequestFailed);
        assert_eq!(
            reporter.take_notifications(),
            vec![REBUILD_REQUEST_FAILURE_NOTICE]
        );
    }
}
