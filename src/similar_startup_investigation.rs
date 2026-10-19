//! Temporary opt-in witnesses for the startup-marker investigation; no fixes.
use super::*;
use crate::similar_search_array::startup_investigation_probe::{self as base_probe, Phase};
use std::sync::mpsc;

#[test]
#[ignore = "investigation only: forces an overlapping base publication failure"]
#[cfg(windows)]
fn reconfigure_overlapping_loaders_fail_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut favorite = crate::settings::FavoriteEntry::new("root".to_owned(), root.clone());
    favorite.auto_index_similar = true;
    let manager = SimilarIndexManager::new(temp.path().to_path_buf());
    let (old_entered_tx, old_entered_rx) = mpsc::channel();
    let (publish_old_tx, publish_old_rx) = mpsc::channel();
    let (old_published_tx, old_published_rx) = mpsc::channel();
    let (drop_old_tx, drop_old_rx) = mpsc::channel();
    let publish_old_rx = Mutex::new(publish_old_rx);
    let old_published_rx = Mutex::new(old_published_rx);
    let drop_old_rx = Mutex::new(drop_old_rx);
    let old_thread = Mutex::new(None);
    let _probe = base_probe::install(
        similar_search_array::base_path(temp.path()),
        Arc::new(move |phase| {
            let thread = std::thread::current().id();
            let is_old = *old_thread.lock().unwrap().get_or_insert(thread) == thread;
            eprintln!("BASE_PROBE old={is_old} phase={phase:?}");
            match (is_old, phase) {
                (true, Phase::BeforeReplace) => {
                    old_entered_tx.send(()).unwrap();
                    publish_old_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
                (false, Phase::BeforeReplace) => {
                    publish_old_tx.send(()).unwrap();
                    old_published_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
                (true, Phase::AfterReplace) => {
                    old_published_tx.send(()).unwrap();
                    drop_old_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
                (false, Phase::AfterReplace) => drop_old_tx.send(()).unwrap(),
            }
        }),
    );
    let (published_tx, published_rx) = mpsc::channel();
    let (discarded_tx, discarded_rx) = mpsc::channel();
    *manager.scheduler.startup_test_probe.lock().unwrap() = Some(Arc::new(move |phase| {
        if phase == "memory_load_published" {
            published_tx.send(()).unwrap();
        } else if phase == "memory_load_discarded" {
            discarded_tx.send(()).unwrap();
        }
    }));
    manager.configure(
        std::slice::from_ref(&favorite),
        crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
        None,
        Vec::new(),
    );
    old_entered_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    // A normal accepted configuration change invalidates Loading and starts a new loader.
    manager.configure(
        std::slice::from_ref(&favorite),
        crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
        None,
        vec![root.join("excluded")],
    );
    published_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    discarded_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let error = match &*manager.memory.lock().unwrap() {
        MemoryState::Failed(error) => error.clone(),
        _ => panic!("overlapping replacement unexpectedly succeeded"),
    };
    assert!(error.contains("base publish failed"), "{error}");
    assert!(error.contains("0x80070005"), "{error}");
    // Make the fixture's watcher ready, then exercise the real Full -> ack -> terminal path.
    {
        let mut state = manager.scheduler.state.lock().unwrap();
        state.watch_by_root.get_mut(&favorite.id).unwrap().health = WatchHealth::Ready;
        state.phase = SchedulerPhase::Starting;
    }
    Arc::clone(&manager.scheduler).worker_loop();
    manager
        .scheduler
        .trace_startup_investigation("controlled_reconfigure_terminal");
    assert!(matches!(
        &*manager.progress.lock().unwrap(),
        IndexProgress::Degraded { reason: IndexDegradedReason::ArrayPublication(detail), .. }
            if detail == &error
    ));
    let db = manager.scheduler.db_for_worker().unwrap();
    let fingerprint = similar_scan_fingerprint(
        manager
            .scheduler
            .state
            .lock()
            .unwrap()
            .desired_config
            .as_ref()
            .unwrap(),
    );
    assert!(!db.scanned_once_matches(&fingerprint).unwrap());
    assert_eq!(manager.reconcile_job_counts_for_test(), (1, 0));
    assert_eq!(db.full_inventory_load_count(), 1);
    *manager.scheduler.startup_test_probe.lock().unwrap() = None;
}
