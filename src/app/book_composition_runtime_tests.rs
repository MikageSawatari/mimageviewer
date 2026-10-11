//! G2 tests prove immutable record adoption and ownership only. G3 layout,
//! pairing, hidden admission and materialized page order are not wired here.
use super::{
    book_composition_runtime::ViewerBookComposition, tests::phase_c_support::setup_app, *,
};
use crate::book_composition::{
    BookCompositionProcessOwner, BookCompositionService, PageDomain, ReadingDirection,
    StoredComposition, fixtures::four_page_book,
};
use crate::book_scope::BookScopeKey;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn install_document(app: &mut App, root: &Path) -> BookScopeKey {
    app.current_folder = Some(root.to_path_buf());
    app.zip_nav = None;
    app.items = (0..4)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: root.to_path_buf(),
            page_num,
            content_type: None,
        })
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.image_metas = vec![None; 4];
    app.visible_indices = vec![0, 1, 2, 3];
    app.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &app.items,
        Some(root),
        app.settings.file_type_visibility.clone(),
    ));
    app.projection_recipe = book_materialization::ProjectionRecipe::Book(
        book_materialization::BookScope::Document(root.to_path_buf()),
    );
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.book_composition = ViewerBookComposition::Automatic;
    let scope = BookScopeKey::document(root).unwrap();
    assert_eq!(app.current_spread_shift_scope(), Some(scope.clone()));
    scope
}

fn runtime(app: &mut App, data: &Path) -> (BookCompositionService, BookCompositionProcessOwner) {
    let (service, owner) = BookCompositionService::spawn_at(data.to_path_buf(), None).unwrap();
    app.install_book_composition_runtime(service.clone());
    (service, owner)
}

/// A later FIFO Prepare reply proves every earlier runtime request has replied.
fn barrier(service: &BookCompositionService, scope: &BookScopeKey) {
    let reply = service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(WAIT)
        .unwrap();
    assert!(reply.status.is_ok(), "{:?}", reply.status);
}

fn adopt(app: &mut App, service: &BookCompositionService, scope: &BookScopeKey) {
    app.poll_book_composition_read(&egui::Context::default());
    barrier(service, scope);
    app.poll_book_composition_read(&egui::Context::default());
    assert!(
        matches!(&app.book_composition, ViewerBookComposition::Accepted { scope: accepted, .. }
        if accepted == scope)
    );
}

fn record(app: &App) -> Arc<StoredComposition> {
    let ViewerBookComposition::Accepted {
        record: Some(record),
        ..
    } = &app.book_composition
    else {
        panic!(
            "expected accepted stored record, got {:?}",
            app.book_composition
        );
    };
    Arc::clone(record)
}

fn save(service: &BookCompositionService, scope: &BookScopeKey, direction: ReadingDirection) {
    let reply = service
        .save(
            scope.clone(),
            PageDomain::Document,
            four_page_book(direction),
        )
        .recv_timeout(WAIT)
        .unwrap();
    assert!(reply.status.is_ok(), "{:?}", reply.status);
    assert!(reply.changed);
}

fn database_bytes(data: &Path) -> Vec<(String, Vec<u8>)> {
    [
        "book_composition.db",
        "book_composition.db-wal",
        "book_composition.db-shm",
    ]
    .into_iter()
    .filter_map(|name| {
        let path = data.join(name);
        path.exists()
            .then(|| (name.to_owned(), std::fs::read(path).unwrap()))
    })
    .collect()
}

fn assert_accepted_token(app: &App, service: &BookCompositionService, scope: &BookScopeKey) {
    let ViewerBookComposition::Accepted {
        scope: accepted,
        token,
        ..
    } = &app.book_composition
    else {
        panic!("expected Accepted, got {:?}", app.book_composition);
    };
    assert_eq!(accepted, scope);
    assert_eq!(*token, service.snapshot().token_for(scope));
}

fn root_repaint_delay(output: &egui::FullOutput) -> Duration {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .expect("headless pass emits the root viewport")
        .repaint_delay
}

fn idle_repaint_context() -> egui::Context {
    let ctx = egui::Context::default();
    // Fixed empty passes consume egui's initial immediate repaint, independently
    // of worker timing. These are neither worker polls nor completion retries.
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    assert_eq!(
        root_repaint_delay(&ctx.run(egui::RawInput::default(), |_| {})),
        Duration::MAX
    );
    ctx
}

fn assert_pending_root_wake(output: &egui::FullOutput) {
    let delay = root_repaint_delay(output);
    assert!(delay > Duration::ZERO && delay <= Duration::from_millis(50));
}

#[test]
fn g2_runtime_read_adopts_absence_without_creating_database_or_changing_layout() {
    let mut env = setup_app();
    let root = env.tmp.path().join("book.pdf");
    let data = env.tmp.path().join("composition-empty");
    let scope = install_document(&mut env, &root);
    let before_items = env.items.clone();
    let (service, owner) = runtime(&mut env, &data);
    adopt(&mut env, &service, &scope);
    assert!(matches!(
        &env.book_composition,
        ViewerBookComposition::Accepted { record: None, .. }
    ));
    assert!(!data.join("book_composition.db").exists());
    env.poll_book_composition_service(&egui::Context::default());
    assert_eq!(
        env.items, before_items,
        "G2 accepts records; G3 owns page projection"
    );
    assert_eq!(env.spread_mode, crate::settings::SpreadMode::Ltr);
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
    assert!(!data.join("book_composition.db").exists());
}

#[test]
fn g2_snapshot_parks_accepted_record_and_a2_read_leaves_saved_database_unchanged() {
    let mut env = setup_app();
    let root = env.tmp.path().join("snapshot-book.pdf");
    let data = env.tmp.path().join("composition-snapshot");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope);
    let accepted = record(&env);
    let before = database_bytes(&data);
    let before_items = env.items.clone();
    env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(env.snapshot.is_some());
    assert!(env.extracted_result_surface().is_some());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Automatic
    ));
    let ViewerBookComposition::Accepted {
        record: Some(parked),
        ..
    } = &env
        .snapshot
        .as_ref()
        .unwrap()
        .saved_viewing_state
        .composition
    else {
        panic!("snapshot must retain the accepted book record");
    };
    assert!(Arc::ptr_eq(parked, &accepted));
    env.poll_book_composition_service(&egui::Context::default());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Automatic
    ));
    assert_eq!(database_bytes(&data), before);
    env.deactivate_snapshot();
    assert!(Arc::ptr_eq(&record(&env), &accepted));
    assert_eq!(env.items, before_items);
    assert_eq!(database_bytes(&data), before);
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g2_materialized_fork_shares_accepted_arc_and_local_reset_keeps_sibling_record() {
    let mut env = setup_app();
    let root = env.tmp.path().join("fork-book.pdf");
    let data = env.tmp.path().join("composition-fork");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope);
    let original = record(&env);
    let before = database_bytes(&data);
    let fork = env.fork_materialized_still_context(&root, 1);
    env.with_window_context_for_test(fork, |app| {
        assert!(Arc::ptr_eq(&record(app), &original));
        app.book_composition = ViewerBookComposition::Automatic;
    });
    assert!(Arc::ptr_eq(&record(&env), &original));
    env.with_window_context_for_test(fork, |app| {
        assert!(matches!(
            app.book_composition,
            ViewerBookComposition::Automatic
        ));
    });
    env.retire_context(fork, "composition_fork_test", |_| ())
        .unwrap();
    assert!(Arc::ptr_eq(&record(&env), &original));
    assert_eq!(database_bytes(&data), before);
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
}

#[test]
fn g2_remote_registration_worker_prepare_is_read_only_and_matches_local_record() {
    let mut env = setup_app();
    let root = env.tmp.path().join("remote-book.pdf");
    let data = env.tmp.path().join("composition-remote");
    let scope = install_document(&mut env, &root);
    let session = crate::remote_ipc::session::SessionHandle::new();
    env.set_remote_session_handle(session.clone());
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope);
    let before = database_bytes(&data);
    let (send, receive) = std::sync::mpsc::channel();
    let remote_scope = scope.clone();
    let worker = std::thread::spawn(move || {
        let registered = session
            .book_composition_service()
            .expect("App registered service");
        let reply = registered
            .prepare(remote_scope, PageDomain::Document)
            .recv_timeout(WAIT)
            .unwrap();
        send.send(reply).unwrap();
    });
    let reply = receive.recv_timeout(WAIT).unwrap();
    worker.join().unwrap();
    assert_eq!(reply.scope, scope);
    assert_eq!(reply.status.unwrap().unwrap(), *record(&env));
    assert_eq!(database_bytes(&data), before);
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g2_changed_scope_refreshes_same_book_contexts_and_preserves_other_book_arc() {
    let mut env = setup_app();
    let root_a = env.tmp.path().join("changed-a.pdf");
    let root_b = env.tmp.path().join("unchanged-b.pdf");
    let data = env.tmp.path().join("composition-contexts");
    let scope_a = install_document(&mut env, &root_a);
    let scope_b = BookScopeKey::document(&root_b).unwrap();
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope_a, ReadingDirection::Rtl);
    save(&service, &scope_b, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope_a);
    let same_book = env.fork_materialized_still_context(&root_a, 1);
    install_document(&mut env, &root_b);
    adopt(&mut env, &service, &scope_b);
    let other_book = env.fork_materialized_still_context(&root_b, 1);
    let other_before = env.with_window_context_for_test(other_book, |app| {
        (
            record(app),
            app.items.clone(),
            app.items_generation,
            app.book_composition.clone(),
        )
    });
    install_document(&mut env, &root_a);
    adopt(&mut env, &service, &scope_a);
    let main_before = record(&env);
    let main_items = env.items.clone();
    let same_before = env.with_window_context_for_test(same_book, |app| record(app));
    save(&service, &scope_a, ReadingDirection::Ltr);
    // First poll admits the changed contexts' reads. A later FIFO reply is the
    // explicit completion fence, including a parked context's pending request.
    env.poll_book_composition_service(&egui::Context::default());
    // Hold the at-rest owner in Preparing explicitly so this exercises its
    // completion with the changed notification already consumed. The channel
    // belongs to the real service and the request uses the production API.
    env.with_window_context_for_test(same_book, |app| {
        app.book_composition = ViewerBookComposition::Preparing {
            scope: scope_a.clone(),
            receiver: service.prepare(scope_a.clone(), PageDomain::Document),
        };
    });
    barrier(&service, &scope_a);
    env.poll_book_composition_service(&egui::Context::default());
    let main_after = record(&env);
    assert_eq!(
        main_after.composition.revision,
        main_before.composition.revision + 1
    );
    assert_eq!(main_after.composition.direction, ReadingDirection::Ltr);
    assert_eq!(
        env.items, main_items,
        "G3, not storage adoption, owns reordered items"
    );
    env.with_window_context_for_test(same_book, |app| {
        let after = record(app);
        assert_eq!(
            after.composition.revision,
            same_before.composition.revision + 1
        );
        assert_eq!(after.composition.direction, ReadingDirection::Ltr);
    });
    env.with_window_context_for_test(other_book, |app| {
        assert!(Arc::ptr_eq(&record(app), &other_before.0));
        assert_eq!(app.items, other_before.1);
        assert_eq!(app.items_generation, other_before.2);
        assert_eq!(app.book_composition, other_before.3);
    });
    let after_save = database_bytes(&data);
    env.retire_context(same_book, "composition_changed_same_test", |_| ())
        .unwrap();
    env.retire_context(other_book, "composition_changed_other_test", |_| ())
        .unwrap();
    assert_eq!(database_bytes(&data), after_save);
    owner.shutdown_and_join();
}

#[test]
fn g2_snapshot_retains_record_during_result_view_and_refreshes_after_return() {
    let mut env = setup_app();
    let root = env.tmp.path().join("retained-book.pdf");
    let data = env.tmp.path().join("composition-retained");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope);
    let retained = record(&env);
    let before_items = env.items.clone();
    env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(env.snapshot.is_some());
    save(&service, &scope, ReadingDirection::Ltr);
    let after_save = database_bytes(&data);
    env.poll_book_composition_service(&egui::Context::default());
    barrier(&service, &scope);
    env.poll_book_composition_service(&egui::Context::default());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Automatic
    ));
    let ViewerBookComposition::Accepted {
        record: Some(saved),
        ..
    } = &env
        .snapshot
        .as_ref()
        .unwrap()
        .saved_viewing_state
        .composition
    else {
        panic!("saved book record must remain parked while results are viewed");
    };
    assert!(Arc::ptr_eq(saved, &retained));
    env.deactivate_snapshot();
    assert!(Arc::ptr_eq(&record(&env), &retained));
    adopt(&mut env, &service, &scope);
    let refreshed = record(&env);
    assert_eq!(
        refreshed.composition.revision,
        retained.composition.revision + 1
    );
    assert_eq!(refreshed.composition.direction, ReadingDirection::Ltr);
    assert_eq!(
        env.items, before_items,
        "G2 changes the accepted record only"
    );
    assert_eq!(database_bytes(&data), after_save);
    owner.shutdown_and_join();
}

#[test]
fn g2_scope_read_rejection_notifies_once_and_keeps_saved_record() {
    let mut env = setup_app();
    let root = env.tmp.path().join("domain-rejected.pdf");
    let data = env.tmp.path().join("composition-domain-rejection");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    service
        .save(
            scope.clone(),
            PageDomain::ArchiveLevel {
                prefix: String::new(),
            },
            crate::book_composition::BookComposition::new(ReadingDirection::Rtl, Vec::new()),
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    let before = database_bytes(&data);
    env.fs_feedback_toast = None;
    env.poll_book_composition_read(&egui::Context::default());
    service
        .prepare(
            scope.clone(),
            PageDomain::ArchiveLevel {
                prefix: String::new(),
            },
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Blocked { .. }
    ));
    assert!(env.fs_feedback_toast.is_some());
    env.fs_feedback_toast = None;
    env.poll_book_composition_read(&egui::Context::default());
    assert!(
        env.fs_feedback_toast.is_none(),
        "same rejected token does not notify every frame"
    );
    assert_eq!(database_bytes(&data), before);
    assert!(service.snapshot().rows.contains_key(&scope.storage_key()));
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g2_at_rest_stale_prepare_after_drained_change_reprepares_without_accepting_old_record() {
    let mut env = setup_app();
    let root_a = env.tmp.path().join("stale-parked-a.pdf");
    let root_b = env.tmp.path().join("stable-mounted-b.pdf");
    let data = env.tmp.path().join("composition-stale-parked");
    let scope_a = install_document(&mut env, &root_a);
    let scope_b = BookScopeKey::document(&root_b).unwrap();
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope_a, ReadingDirection::Rtl);
    save(&service, &scope_b, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    adopt(&mut env, &service, &scope_a);
    let old_record = record(&env);
    let old_token = service.snapshot().token_for(&scope_a);
    let parked = env.fork_materialized_still_context(&root_a, 1);

    // Fence the real old Prepare reply before the write. Its receiver remains
    // undrained, so the next adoption observes precisely the stale publication.
    let old_receiver = service.prepare(scope_a.clone(), PageDomain::Document);
    barrier(&service, &scope_a);
    env.with_window_context_for_test(parked, |app| {
        app.book_composition = ViewerBookComposition::Preparing {
            scope: scope_a.clone(),
            receiver: old_receiver,
        };
    });
    install_document(&mut env, &root_b);
    adopt(&mut env, &service, &scope_b);
    let sibling_record = record(&env);
    let sibling_state = env.book_composition.clone();
    let sibling_items = env.items.clone();
    let sibling_generation = env.items_generation;
    save(&service, &scope_a, ReadingDirection::Ltr);
    let latest_token = service.snapshot().token_for(&scope_a);
    assert_ne!(latest_token, old_token);
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![scope_a.storage_key()]
    );
    let after_save = database_bytes(&data);

    let ctx = idle_repaint_context();
    let pending_output = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_book_composition_service(ctx);
    });
    env.with_window_context_for_test(parked, |app| {
        assert!(matches!(
            &app.book_composition,
            ViewerBookComposition::Preparing { scope, .. } if scope == &scope_a
        ));
    });
    assert_pending_root_wake(&pending_output);
    assert!(Arc::ptr_eq(&record(&env), &sibling_record));
    assert_eq!(env.book_composition, sibling_state);

    // The new Prepare is complete before the second global poll, with no
    // remaining change notification and no mounted frame for the parked owner.
    barrier(&service, &scope_a);
    assert!(service.drain_changed_scope_keys().is_empty());
    env.poll_book_composition_service(&ctx);
    env.with_window_context_for_test(parked, |app| {
        assert_accepted_token(app, &service, &scope_a);
        let latest = record(app);
        assert_eq!(
            latest.composition.revision,
            old_record.composition.revision + 1
        );
        assert_eq!(latest.composition.direction, ReadingDirection::Ltr);
    });
    assert!(Arc::ptr_eq(&record(&env), &sibling_record));
    assert_eq!(env.book_composition, sibling_state);
    assert_eq!(env.items, sibling_items);
    assert_eq!(env.items_generation, sibling_generation);
    assert_eq!(database_bytes(&data), after_save);
    env.retire_context(parked, "composition_stale_parked_test", |_| ())
        .unwrap();
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g2_fullscreen_mounted_boundary_admits_open_switch_and_same_scope_update() {
    let mut env = setup_app();
    let root_a = env.tmp.path().join("fullscreen-a.pdf");
    let root_b = env.tmp.path().join("fullscreen-b.pdf");
    let data = env.tmp.path().join("composition-fullscreen-boundary");
    let scope_a = install_document(&mut env, &root_a);
    let scope_b = BookScopeKey::document(&root_b).unwrap();
    env.fullscreen_idx = Some(0);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope_a, ReadingDirection::Rtl);
    save(&service, &scope_b, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    let ctx = egui::Context::default();

    // This is the production mounted-document async dispatch used before the
    // fullscreen frame. No grid tail or direct composition-read call admits it.
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Automatic
    ));
    env.poll_mounted_document_open_owners(&ctx);
    barrier(&service, &scope_a);
    env.poll_mounted_document_open_owners(&ctx);
    assert_accepted_token(&env, &service, &scope_a);
    let accepted_a = env.book_composition.clone();

    install_document(&mut env, &root_b);
    // A book switch may retain the prior adopted state until this boundary.
    env.book_composition = accepted_a;
    assert_eq!(env.fullscreen_idx, Some(0));
    env.poll_mounted_document_open_owners(&ctx);
    barrier(&service, &scope_b);
    env.poll_mounted_document_open_owners(&ctx);
    assert_accepted_token(&env, &service, &scope_b);
    let before = record(&env);
    let before_items = env.items.clone();
    let before_generation = env.items_generation;

    save(&service, &scope_b, ReadingDirection::Ltr);
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![scope_b.storage_key()]
    );
    let after_save = database_bytes(&data);
    env.poll_mounted_document_open_owners(&ctx);
    barrier(&service, &scope_b);
    env.poll_mounted_document_open_owners(&ctx);
    assert_accepted_token(&env, &service, &scope_b);
    assert_eq!(
        record(&env).composition.revision,
        before.composition.revision + 1
    );
    assert_eq!(record(&env).composition.direction, ReadingDirection::Ltr);
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(
        env.items, before_items,
        "G3 layout is not wired by G2 reads"
    );
    assert_eq!(env.items_generation, before_generation);
    assert_eq!(database_bytes(&data), after_save);
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn g2_input_free_pending_read_wakes_root_until_fifo_reply_is_adopted() {
    let mut env = setup_app();
    let root = env.tmp.path().join("input-free-fullscreen.pdf");
    let data = env.tmp.path().join("composition-input-free");
    let scope = install_document(&mut env, &root);
    env.fullscreen_idx = Some(0);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Rtl);
    service.drain_changed_scope_keys();
    let before_items = env.items.clone();
    let before_database = database_bytes(&data);

    // A real FIFO worker reply is held behind an explicit channel delivery gate.
    // Holding delivery makes Empty deterministic without worker sleeps, retries,
    // synthetic tokens, or a race against SQLite completing a tiny read.
    let receiver = service.prepare(scope.clone(), PageDomain::Document);
    barrier(&service, &scope);
    let reply = receiver.recv_timeout(WAIT).unwrap();
    assert!(reply.status.is_ok());
    let (deliver, pending) = std::sync::mpsc::channel();
    env.book_composition = ViewerBookComposition::Preparing {
        scope: scope.clone(),
        receiver: pending,
    };
    let ctx = idle_repaint_context();
    let requests = Arc::new(std::sync::Mutex::new(Vec::<egui::RequestRepaintInfo>::new()));
    let requests_callback = Arc::clone(&requests);
    ctx.set_request_repaint_callback(move |info| {
        requests_callback.lock().unwrap().push(info);
    });

    let first = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    assert_pending_root_wake(&first);
    let second = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    assert_pending_root_wake(&second);
    assert!(matches!(
        env.book_composition,
        ViewerBookComposition::Preparing { .. }
    ));
    assert!(requests.lock().unwrap().iter().any(|request| {
        request.viewport_id == egui::ViewportId::ROOT
            && request.delay > Duration::ZERO
            && request.delay <= Duration::from_millis(50)
    }));

    deliver.send(reply).unwrap();
    let accepted = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    assert_accepted_token(&env, &service, &scope);
    assert_eq!(root_repaint_delay(&accepted), Duration::ZERO);
    let accepted_record = record(&env);
    // egui deliberately carries an immediate request over one more pass.
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    requests.lock().unwrap().clear();
    let idle = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    assert_eq!(root_repaint_delay(&idle), Duration::MAX);
    assert!(requests.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&record(&env), &accepted_record));
    assert_eq!(env.items, before_items, "G2 adopts records only");
    assert_eq!(database_bytes(&data), before_database);
    owner.shutdown_and_join();
}
