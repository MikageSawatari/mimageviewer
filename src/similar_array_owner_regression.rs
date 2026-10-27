//! Product-route regression for the former overlapping-loader witness.
use super::*;
use crate::similar_search_array::startup_investigation_probe::{self as base_probe, Phase};
use std::sync::mpsc;

#[test]
fn reconfigure_overlapping_requests_share_one_load_and_full_writes_marker() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let second = temp.path().join("second");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&second).unwrap();
    let mut favorite = crate::settings::FavoriteEntry::new("root".into(), root.clone());
    favorite.auto_index_similar = true;
    let mut other = crate::settings::FavoriteEntry::new("second".into(), second);
    other.auto_index_similar = true;
    let manager = SimilarIndexManager::new(temp.path().to_path_buf());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let resume_rx = Mutex::new(resume_rx);
    let replacements = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&replacements);
    let _probe = base_probe::install(
        similar_search_array::base_path(temp.path()),
        Arc::new(move |phase| {
            if phase == Phase::BeforeReplace {
                assert_eq!(
                    observed.fetch_add(1, Ordering::AcqRel),
                    0,
                    "only one initial base writer"
                );
                entered_tx.send(()).unwrap();
                resume_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap();
            }
        }),
    );
    let mut passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
    manager.configure(
        std::slice::from_ref(&favorite),
        passwords.clone(),
        None,
        Vec::new(),
    );
    entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(manager.array.observe().published().is_none());
    // Normal accepted configure calls return while the real file writer is blocked.
    manager.configure(
        &[favorite.clone(), other.clone()],
        passwords.clone(),
        None,
        vec![root.join("excluded")],
    );
    passwords.bump_credential_revision_for_test(&root.join("locked.pdf"));
    manager.configure(
        &[favorite.clone(), other],
        passwords,
        None,
        vec![root.join("excluded")],
    );
    for _ in 0..8 {
        manager.array.ensure(ArrayDemand::LoadOrCreate);
    }
    assert_eq!(manager.array.operation_count(), 1);
    resume_tx.send(()).unwrap();
    {
        let mut state = manager.scheduler.state.lock().unwrap();
        for watch in state.watch_by_root.values_mut() {
            watch.health = WatchHealth::Ready;
        }
        state.phase = SchedulerPhase::Starting;
    }
    Arc::clone(&manager.scheduler).worker_loop();
    assert!(
        matches!(manager.progress(), IndexProgress::Complete(_)),
        "{:?}",
        manager.progress()
    );
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
    assert!(db.scanned_once_matches(&fingerprint).unwrap());
    assert_eq!(manager.reconcile_job_counts_for_test(), (1, 0));
    assert_eq!(replacements.load(Ordering::Acquire), 1);
    assert!(manager.array.observe().published().is_some());
    manager.scheduler.shutdown();
    manager.array.wait_closed_for_test();
}

#[test]
fn stale_item_completion_cannot_refill_cache_after_scope_or_publication_change() {
    for change_scope in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let rows = db.load_base_search_rows(current_hash_version()).unwrap();
        let snapshot = Arc::new(SearchSnapshot::from_base(similar_search_array::BaseArray {
            records: rows.records.into_boxed_slice(),
            store_id: rows.store_id,
            applied_seq: rows.applied_seq,
        }));
        manager.install_snapshot_for_test(Arc::clone(&snapshot));
        manager.query_scope.lock().unwrap().roots = Arc::new(vec!["c:/library".into()]);
        let old = manager.query_stamp_for_test();
        let (started, started_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let (completed, completed_rx) = mpsc::channel();
        *manager.item_query_test_hook.lock().unwrap() = Some(ItemQueryTestHook {
            started,
            resume: resume_rx,
            completed,
        });
        assert_eq!(
            *manager.query_item("c:/library/missing.jpg"),
            ItemQuery::Preparing
        );
        started_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        if change_scope {
            manager.configure(
                &[],
                crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
                None,
                Vec::new(),
            );
        } else {
            manager.install_snapshot_for_test(snapshot);
        }
        let current = manager.query_stamp_for_test();
        assert_ne!(old, current);
        // No replacement cache entry is needed to reject the old completion.
        resume.send(()).unwrap();
        completed_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let scope = manager.query_scope.lock().unwrap();
        assert_eq!(
            *cached_item_query(&scope.cache, "c:/library/missing.jpg", old).unwrap(),
            ItemQuery::Preparing
        );
        assert!(cached_item_query(&scope.cache, "c:/library/missing.jpg", current).is_none());
    }
}

#[test]
fn book_wait_is_dispatched_after_missing_fault_or_worker_exit_without_another_ui_query() {
    use crate::similar_array_owner::{ArrayFailure, ArrayOperation, OwnerPhase};
    for terminal in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let book = "c:/library/book";
        let page = StoredItem {
            item_key: format!("{book}/0.jpg"),
            kind: ItemKind::Image,
            container_key: Some(book.into()),
            page_index: Some(0),
            mtime: 1,
            file_size: 1,
            hash_version: current_hash_version(),
            pdq256: [9; 32],
            quality: 50,
            width: 100,
            height: 100,
            format: 1,
        };
        let generation = db
            .begin_container_build(book, ContainerKind::ImageFolder, 1, 1, 1)
            .unwrap();
        db.stage_item(generation, &page).unwrap();
        db.complete_container(book, generation).unwrap();
        let manager = SimilarIndexManager::new(temp.path().to_path_buf());
        manager.query_scope.lock().unwrap().roots = Arc::new(vec!["c:/library".into()]);
        let (phase, mut control) =
            crate::similar_book_query_test_probe::BookQueryTestPhaseProbe::new(
                crate::similar_book_query_test_probe::BookQueryTestPhase::AfterMetadata,
            );
        manager.set_book_query_test_phase_probe(phase);
        let (entered, entered_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        if terminal == 2 {
            manager.array.set_probe(Arc::new(move |phase| {
                if phase == OwnerPhase::BeforePublish {
                    let _ = entered.send(());
                    resume_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .expect("release worker-exit barrier");
                    panic!("injected worker exit");
                }
            }));
            manager.array.ensure(ArrayDemand::LoadOrCreate);
            entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        } else {
            manager
                .array
                .install_for_test(ArrayObservation::Uninitialized { busy: true });
        }
        let client = SimilarBookQueryClient::new();
        let (waiting, waiting_rx) = mpsc::channel();
        let (allow_wait, allow_wait_rx) = mpsc::channel();
        let allow_wait_rx = Mutex::new(allow_wait_rx);
        let once = AtomicBool::new(false);
        *manager.scheduler.startup_test_probe.lock().unwrap() = Some(Arc::new(move |phase| {
            if phase == "book_dispatch_wait" && !once.swap(true, Ordering::AcqRel) {
                waiting.send(()).unwrap();
                allow_wait_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .expect("release the fixed Wait decision");
            }
        }));
        assert_eq!(*manager.query_book(&client, book), BookQuery::Preparing);
        waiting_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("dispatcher captured its ticket and decided Wait");
        if terminal == 2 {
            resume.send(()).unwrap();
        } else {
            manager.array.install_for_test(if terminal == 0 {
                ArrayObservation::Missing
            } else {
                ArrayObservation::Faulted {
                    published: None,
                    failure: ArrayFailure {
                        operation: ArrayOperation::Demand(ArrayDemand::LoadIfPresent),
                        detail: "injected load failure".into(),
                    },
                }
            });
        }
        allow_wait.send(()).unwrap();
        // This checkpoint belongs to the DB runtime, not query polling/repaint. Without the
        // readiness notification the queued Wait request cannot reach it.
        control.wait_reached();
        control.release();
        manager.scheduler.shutdown();
        manager.array.wait_closed_for_test();
    }
}

#[test]
fn configuration_query_watch_and_full_do_not_retry_a_faulted_owner() {
    use crate::similar_array_owner::OwnerPhase;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut favorite = crate::settings::FavoriteEntry::new("root".into(), root.clone());
    favorite.auto_index_similar = true;
    let manager = SimilarIndexManager::new(temp.path().to_path_buf());
    manager.array.fail_at_for_test(OwnerPhase::BeforePublish);
    manager.configure(
        std::slice::from_ref(&favorite),
        crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
        None,
        Vec::new(),
    );
    let db = manager.scheduler.db_for_worker().unwrap();
    assert!(matches!(
        manager
            .array
            .request_through(db.change_watermark().unwrap())
            .recv_timeout(Duration::from_secs(20))
            .unwrap(),
        ArrayAck::Failed(_)
    ));
    let operations = manager.array.operation_count();
    for n in 0..3 {
        {
            let mut state = manager.scheduler.state.lock().unwrap();
            for watch in state.watch_by_root.values_mut() {
                watch.health = WatchHealth::Ready;
            }
            state.phase = SchedulerPhase::Starting;
        }
        manager.configure(
            std::slice::from_ref(&favorite),
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            None,
            vec![root.join(format!("excluded-{n}"))],
        );
        manager.request_full_all();
        manager.scheduler.request_array_refresh();
        assert!(matches!(
            *manager.query_item(&format!(
                "{}/missing.jpg",
                crate::search_index_db::normalize_path(&root)
            )),
            ItemQuery::Failed(_)
        ));
        Arc::clone(&manager.scheduler).worker_loop();
        assert!(matches!(
            manager.progress(),
            IndexProgress::Degraded {
                reason: IndexDegradedReason::ArrayPublication(_),
                ..
            }
        ));
        assert_eq!(manager.array.operation_count(), operations);
    }
    let config = manager
        .scheduler
        .state
        .lock()
        .unwrap()
        .desired_config
        .clone()
        .unwrap();
    let fingerprint = similar_scan_fingerprint(&config);
    assert!(!db.scanned_once_matches(&fingerprint).unwrap());
    manager.scheduler.shutdown();
    manager.array.wait_closed_for_test();
    drop(manager);
    drop(db);
    let reopened = SimilarIndexManager::new(temp.path().to_path_buf());
    reopened.configure(
        std::slice::from_ref(&favorite),
        config.pdf_passwords.clone(),
        None,
        config.excluded_roots.clone(),
    );
    {
        let mut state = reopened.scheduler.state.lock().unwrap();
        for watch in state.watch_by_root.values_mut() {
            watch.health = WatchHealth::Ready;
        }
        state.phase = SchedulerPhase::Starting;
    }
    Arc::clone(&reopened.scheduler).worker_loop();
    assert!(matches!(reopened.progress(), IndexProgress::Complete(_)));
    assert!(
        reopened
            .scheduler
            .db_for_worker()
            .unwrap()
            .scanned_once_matches(&fingerprint)
            .unwrap()
    );
    reopened.scheduler.shutdown();
    reopened.array.wait_closed_for_test();
}

#[test]
fn configuring_and_closing_a_manager_keeps_sibling_query_stamp_and_cached_arc() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let manager_a = SimilarIndexManager::new(a.path().to_path_buf());
    let manager_b = SimilarIndexManager::new(b.path().to_path_buf());
    let db = SimilarDb::open_at(&SimilarDb::db_path_at(b.path())).unwrap();
    let rows = db.load_base_search_rows(current_hash_version()).unwrap();
    manager_b.install_snapshot_for_test(Arc::new(SearchSnapshot::from_base(
        similar_search_array::BaseArray {
            records: rows.records.into_boxed_slice(),
            store_id: rows.store_id,
            applied_seq: rows.applied_seq,
        },
    )));
    manager_b.query_scope.lock().unwrap().roots = Arc::new(vec!["c:/library".into()]);
    let key = "c:/library/missing.jpg";
    let (started, started_rx) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    let (completed, completed_rx) = mpsc::channel();
    *manager_b.item_query_test_hook.lock().unwrap() = Some(ItemQueryTestHook {
        started,
        resume: resume_rx,
        completed,
    });
    manager_b.query_item(key);
    started_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    resume.send(()).unwrap();
    completed_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let result = manager_b.query_item(key);
    assert_eq!(*result, ItemQuery::NotIndexed);
    let stamp = manager_b.query_stamp_for_test();
    let snapshot = manager_b.snapshot_for_test();
    for n in 0..3 {
        manager_a.configure(
            &[],
            crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
            None,
            vec![a.path().join(format!("excluded-{n}"))],
        );
        assert_eq!(manager_b.query_stamp_for_test(), stamp);
        assert!(Arc::ptr_eq(&manager_b.query_item(key), &result));
    }
    manager_a.scheduler.shutdown();
    manager_a.array.wait_closed_for_test();
    drop(manager_a);
    let reopened_a = SimilarIndexManager::new(a.path().to_path_buf());
    reopened_a.configure(
        &[],
        crate::pdf_passwords::PdfPasswordStore::empty_for_test(),
        None,
        Vec::new(),
    );
    assert_eq!(manager_b.query_stamp_for_test(), stamp);
    assert!(Arc::ptr_eq(&manager_b.snapshot_for_test(), &snapshot));
    assert!(Arc::ptr_eq(&manager_b.query_item(key), &result));
    reopened_a.scheduler.shutdown();
    reopened_a.array.wait_closed_for_test();
    manager_b.scheduler.shutdown();
    manager_b.array.wait_closed_for_test();
}
