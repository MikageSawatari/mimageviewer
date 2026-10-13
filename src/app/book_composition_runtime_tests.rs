#![cfg(test)]

//! Runtime ownership and worker projection adoption, including parked contexts.
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
    settle_projection(app);
    barrier(service, scope);
    app.poll_book_composition_read(&egui::Context::default());
    settle_projection(app);
    assert!(
        matches!(&app.book_composition, ViewerBookComposition::Accepted { scope: accepted, .. }
        if accepted == scope)
    );
}

/// Await exactly the admitted worker, then deliver its result to the production
/// poll path. No timing assumptions or retry polling are needed.
fn settle_projection(app: &mut App) {
    settle_projection_with_context(app, &egui::Context::default());
}

fn settle_projection_with_context(app: &mut App, ctx: &egui::Context) {
    let state = std::mem::take(&mut app.book_composition);
    if let ViewerBookComposition::Projecting {
        scope,
        token,
        record,
        source,
        generation,
        origin,
        receiver,
    } = state
    {
        let reply = receiver
            .recv_timeout(WAIT)
            .expect("projection worker completion");
        let (deliver, ready) = std::sync::mpsc::channel();
        deliver.send(reply).unwrap();
        app.book_composition = ViewerBookComposition::Projecting {
            scope,
            token,
            record,
            source,
            generation,
            origin,
            receiver: ready,
        };
        app.poll_book_composition_read(ctx);
    } else {
        app.book_composition = state;
    }
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
        "an absent composition keeps the automatic page rows"
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
            origin: Default::default(),
        };
    });
    barrier(&service, &scope_a);
    env.poll_book_composition_service(&egui::Context::default());
    settle_projection(&mut env);
    env.with_window_context_for_test(same_book, settle_projection);
    let main_after = record(&env);
    assert_eq!(
        main_after.composition.revision,
        main_before.composition.revision + 1
    );
    assert_eq!(main_after.composition.direction, ReadingDirection::Ltr);
    assert_eq!(
        env.items, main_items,
        "changing direction keeps the logical page order"
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
    env.settle_snapshot_page_index_for_test();
    adopt(&mut env, &service, &scope);
    let refreshed = record(&env);
    assert_eq!(
        refreshed.composition.revision,
        retained.composition.revision + 1
    );
    assert_eq!(refreshed.composition.direction, ReadingDirection::Ltr);
    assert_eq!(
        env.items, before_items,
        "changing direction keeps the logical page order"
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
            origin: Default::default(),
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
    env.with_window_context_for_test(parked, settle_projection);
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
    settle_projection(&mut env);
    assert_accepted_token(&env, &service, &scope_a);
    let accepted_a = env.book_composition.clone();

    install_document(&mut env, &root_b);
    // A book switch may retain the prior adopted state until this boundary.
    env.book_composition = accepted_a;
    assert_eq!(env.fullscreen_idx, Some(0));
    env.poll_mounted_document_open_owners(&ctx);
    barrier(&service, &scope_b);
    env.poll_mounted_document_open_owners(&ctx);
    settle_projection(&mut env);
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
    settle_projection(&mut env);
    assert_accepted_token(&env, &service, &scope_b);
    assert_eq!(
        record(&env).composition.revision,
        before.composition.revision + 1
    );
    assert_eq!(record(&env).composition.direction, ReadingDirection::Ltr);
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(
        env.items, before_items,
        "changing direction preserves canonical page order"
    );
    assert!(env.items_generation > before_generation);
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
        origin: Default::default(),
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
    let projecting = ctx.run(egui::RawInput::default(), |ctx| {
        env.poll_mounted_document_open_owners(ctx);
    });
    assert_eq!(root_repaint_delay(&projecting), Duration::ZERO);
    settle_projection_with_context(&mut env, &ctx);
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
    assert_eq!(
        env.items, before_items,
        "direction does not change canonical page order"
    );
    assert_eq!(database_bytes(&data), before_database);
    owner.shutdown_and_join();
}

#[test]
fn g3_unsupported_source_notifies_once_and_keeps_original_rows_and_stored_bytes() {
    use crate::book_composition::{
        BookComposition, CompositionPageRef, CompositionProjectionOutcome,
    };
    use std::io::Write;
    for names in [vec!["/b.jpg", "c.jpg"], vec!["B.jpg", "b.jpg", "c.jpg"]] {
        let mut env = setup_app();
        let root = env.tmp.path().join("unsupported.zip");
        let data = env.tmp.path().join("unsupported-composition");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
        for name in names {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"unchanged source payload").unwrap();
        }
        writer.finish().unwrap();
        let source_before = std::fs::read(&root).unwrap();
        let tree = Arc::new(crate::zip_tree::ZipTree::build(
            root.clone(),
            crate::zip_loader::enumerate_image_entries(&root).unwrap(),
        ));
        let nav = crate::zip_tree::ZipNavState::new(tree);
        let prefix = nav.current_raw().to_vec();
        let (items, metas) = nav.materialize_current(crate::settings::SortOrder::FileName);
        env.current_folder = Some(root.clone());
        env.items = items;
        env.image_metas = metas;
        env.thumbnails = vec![ThumbnailState::Pending; env.items.len()];
        env.visible_indices = (0..env.items.len()).collect();
        env.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
            &env.items,
            Some(&root),
            env.settings.file_type_visibility.clone(),
        ));
        env.projection_recipe = book_materialization::ProjectionRecipe::Book(
            book_materialization::BookScope::Archive {
                logical_root: root.clone(),
                prefix: prefix.clone(),
            },
        );
        env.zip_nav = Some(nav);
        env.book_composition = ViewerBookComposition::Automatic;
        let scope = BookScopeKey::archive(&root, &prefix).unwrap();
        let domain = PageDomain::ArchiveLevel {
            prefix: scope.level_prefix.clone(),
        };
        let (service, owner) = runtime(&mut env, &data);
        let stored = BookComposition::new(
            ReadingDirection::Ltr,
            vec![CompositionPageRef::ArchiveEntry {
                entry_name: "c.jpg".into(),
            }],
        );
        service
            .save(scope.clone(), domain.clone(), stored)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        service.drain_changed_scope_keys();
        let before_rows = env.items.clone();
        let before_generation = env.items_generation;
        let before_database = database_bytes(&data);
        let before_record = service.snapshot().rows[&scope.storage_key()].clone();
        env.fs_feedback_toast = None;
        env.poll_book_composition_read(&egui::Context::default());
        service
            .prepare(scope.clone(), domain)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        env.poll_book_composition_read(&egui::Context::default());
        settle_projection(&mut env);
        assert!(matches!(
            &env.book_composition,
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Unsupported { .. },
                ..
            }
        ));
        assert_eq!(
            env.fs_feedback_toast
                .as_ref()
                .map(|(message, _, _)| message.as_str()),
            Some(crate::book_composition::UNSUPPORTED_COMPOSITION_NOTICE)
        );
        env.fs_feedback_toast = None;
        env.poll_book_composition_read(&egui::Context::default());
        assert!(env.fs_feedback_toast.is_none());
        assert_eq!(env.items, before_rows);
        assert_eq!(env.items_generation, before_generation);
        assert_eq!(std::fs::read(&root).unwrap(), source_before);
        assert_eq!(database_bytes(&data), before_database);
        assert_eq!(service.snapshot().rows[&scope.storage_key()], before_record);
        owner.shutdown_and_join();
    }
}

fn admit_projection(app: &mut App, service: &BookCompositionService, scope: &BookScopeKey) {
    use super::book_composition_runtime::ProjectionOrigin;
    let origin = match &app.book_composition {
        ViewerBookComposition::Accepted {
            scope: old,
            source,
            layout,
            ..
        } if old == scope => ProjectionOrigin::Refresh {
            source: source.clone(),
            previous: match layout {
                crate::book_composition::CompositionProjectionOutcome::Composed(projection) => {
                    Some(projection.clone())
                }
                _ => None,
            },
            anchor: app
                .fullscreen_idx
                .and_then(|index| app.items.get(index))
                .and_then(|item| crate::book_resume_db::PageIdentity::for_item(item, None)),
            previous_order: app
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    crate::book_resume_db::PageIdentity::for_item(item, None)
                        .map(|identity| (index, identity))
                })
                .collect::<Vec<_>>()
                .into(),
        },
        _ => ProjectionOrigin::Fresh,
    };
    let reply = service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(WAIT)
        .unwrap();
    let (deliver, receiver) = std::sync::mpsc::channel();
    deliver.send(reply).unwrap();
    app.book_composition = ViewerBookComposition::Preparing {
        scope: scope.clone(),
        receiver,
        origin: Box::new(origin),
    };
    app.poll_book_composition_read(&egui::Context::default());
    assert!(matches!(
        app.book_composition,
        ViewerBookComposition::Projecting { .. }
    ));
}

fn current_physical_page(app: &App) -> Option<u32> {
    app.fullscreen_idx
        .and_then(|index| app.items.get(index))
        .and_then(|item| match item {
            GridItem::PdfPage { page_num, .. } => Some(*page_num),
            _ => None,
        })
}

#[test]
fn g3_projection_keeps_hidden_pending_trim_in_existing_page_store() {
    let mut env = setup_app();
    let root = env.tmp.path().join("pending-trim.pdf");
    let scope = install_document(&mut env, &root);
    let data_dir = crate::data_dir::get();
    assert!(data_dir.starts_with(env.tmp.path()));
    let trim_path = data_dir.join("view_trim.db");
    env.view_trim_db = Some(crate::view_trim_db::ViewTrimDb::open_at(&trim_path).unwrap());
    let (service, owner) = runtime(&mut env, &data_dir);
    save(&service, &scope, ReadingDirection::Ltr);
    adopt(&mut env, &service, &scope);
    let page_key = env.page_path_key(1).unwrap();
    let trim =
        crate::view_trim::ViewTrimPageOverride::from_margins(crate::view_trim::ViewTrimMargins {
            left: 0.1,
            ..Default::default()
        });
    env.view_trim_page_overrides.insert(1, trim);
    env.view_trim_dirty_page_overrides.insert(1);
    env.view_trim_save_pending = true;
    let mut composition = four_page_book(ReadingDirection::Ltr);
    composition
        .hidden_pages
        .push(crate::book_composition::fixtures::document_page(1));
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    admit_projection(&mut env, &service, &scope);
    settle_projection(&mut env);
    assert_eq!(env.items.len(), 3);
    assert_eq!(
        crate::view_trim_db::ViewTrimDb::open_at(&trim_path)
            .unwrap()
            .get_page_override(&page_key),
        Some(trim)
    );
    assert!(!env.view_trim_save_pending);
    assert!(env.view_trim_dirty_page_overrides.is_empty());
    let bookmark = crate::book_bookmarks::BookBookmark {
        id: 1,
        container_key: scope.logical_root.to_string_lossy().into_owned(),
        container_path: root,
        container_kind: crate::book_bookmarks::BookContainerKind::Pdf,
        page_identity: crate::book_bookmarks::PageIdentity::PdfPage(1),
        page_index_hint: 1,
        created_at_ms: 0,
        title: None,
    };
    assert!(env.composition_hides_bookmark(&bookmark));
    assert_eq!(
        env.book_bookmark_panel_resolution(&bookmark),
        BookBookmarkPanelResolution {
            current_item_index: None,
            target_item_index: None,
        }
    );
    owner.shutdown_and_join();
}

#[test]
fn g3_document_reentry_keeps_physical_extent_after_composed_row_reorder() {
    let mut env = setup_app();
    let root = env.tmp.path().join("reordered-extent.pdf");
    let scope = install_document(&mut env, &root);
    let data = env.tmp.path().join("reordered-extent-data");
    let (service, owner) = runtime(&mut env, &data);
    let composition = crate::book_composition::BookComposition::new(
        ReadingDirection::Ltr,
        vec![
            crate::book_composition::fixtures::document_page(2),
            crate::book_composition::fixtures::document_page(3),
            crate::book_composition::fixtures::document_page(0),
            crate::book_composition::fixtures::document_page(1),
        ],
    );
    service
        .save(scope.clone(), PageDomain::Document, composition.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    adopt(&mut env, &service, &scope);
    assert!(matches!(
        env.items.last(),
        Some(GridItem::PdfPage { page_num: 1, .. })
    ));
    env.pdf_placeholder_count = None;
    env.bump_items_generation();
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    adopt(&mut env, &service, &scope);
    assert_eq!(
        env.items.len(),
        4,
        "display order cannot shorten the physical source extent"
    );
    assert_eq!(env.canonical_raw_book_page(0), 2);
    assert_eq!(env.canonical_raw_book_page(1), 3);
    owner.shutdown_and_join();
}

#[test]
fn g3_bookmark_hint_uses_raw_position_and_composed_detail_order_is_locked() {
    let mut env = setup_app();
    let root = env.tmp.path().join("bookmark-hint.pdf");
    let scope = install_document(&mut env, &root);
    let data = env.tmp.path().join("bookmark-hint-data");
    let (service, owner) = runtime(&mut env, &data);
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            crate::book_composition::BookComposition::new(
                ReadingDirection::Ltr,
                vec![
                    crate::book_composition::fixtures::document_page(2),
                    crate::book_composition::fixtures::document_page(3),
                    crate::book_composition::fixtures::document_page(0),
                    crate::book_composition::fixtures::document_page(1),
                ],
            ),
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    adopt(&mut env, &service, &scope);
    let draft = env.current_book_bookmark_draft(0).unwrap();
    assert_eq!(draft.page_index_hint, 2);
    assert_eq!(
        draft.page_identity,
        crate::book_bookmarks::PageIdentity::PdfPage(2)
    );
    assert_eq!(
        env.details_header_sort_lock_reason(),
        Some(GridSortLockReason::PageOrderFixed)
    );
    env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(env.extracted_result_surface().is_some());
    assert!(env.details_header_sort_lock_reason().is_none());
    // The extracted surface owns its result order even if its retained source
    // path happens to name a physical book folder.
    env.current_folder = Some(env.book_root_path().join("source-book"));
    assert!(env.current_folder_is_book_folder());
    env.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    env.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
    assert!(env.details_header_sort_active());
    owner.shutdown_and_join();
}

#[test]
fn g3_initial_moved_projection_preserves_raw_resume_identity_and_history_ordinal() {
    use super::file_type_resume::PositionResolution;
    use crate::book_composition::{BookComposition, fixtures::document_page};
    use crate::book_resume_db::{PageIdentity, ReadingPosition};
    let mut env = setup_app();
    let root = env.tmp.path().join("moved.pdf");
    let data = env.tmp.path().join("moved-data");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    let composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            document_page(2),
            document_page(3),
            document_page(0),
            document_page(1),
        ],
    );
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    adopt(&mut env, &service, &scope);
    let pages = env
        .items
        .iter()
        .filter_map(|item| match item {
            GridItem::PdfPage { page_num, .. } => Some(*page_num),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(pages, vec![2, 3, 0, 1]);
    assert_eq!(
        env.reading_direction,
        crate::settings::ReadingDirection::Rtl
    );
    assert_eq!(
        env.resolve_current_book_position(&ReadingPosition {
            page: 0,
            identity: None
        }),
        PositionResolution::Restored(2)
    );
    assert_eq!(
        env.resolve_current_book_position(&ReadingPosition {
            page: 0,
            identity: Some(PageIdentity::for_item(&env.items[1], None).unwrap())
        }),
        PositionResolution::Restored(1)
    );
    assert_eq!(env.current_reading_history_page_position(0), Some((1, 4)));
    let ViewerBookComposition::Accepted {
        layout: crate::book_composition::CompositionProjectionOutcome::Composed(projection),
        ..
    } = &env.book_composition
    else {
        panic!("composed projection");
    };
    assert_eq!(projection.position_for_projected(0).unwrap().page, 2);
    owner.shutdown_and_join();
}

#[test]
fn g3_initial_deferred_entry_waits_for_projection_then_opens_first_visible_real() {
    use crate::book_composition::{BookComposition, fixtures::document_page};
    for hide_all in [false, true] {
        let mut env = setup_app();
        let root = env.tmp.path().join("first-entry.pdf");
        let data = env.tmp.path().join("first-entry-data");
        let scope = install_document(&mut env, &root);
        let (service, owner) = runtime(&mut env, &data);
        let mut composition = BookComposition::new(
            ReadingDirection::Rtl,
            vec![
                document_page(2),
                document_page(3),
                document_page(0),
                document_page(1),
            ],
        );
        composition.hidden_pages = if hide_all {
            (0..4).map(document_page).collect()
        } else {
            vec![document_page(0)]
        };
        service
            .save(scope.clone(), PageDomain::Document, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        env.fullscreen_idx = None;
        env.selected = None;
        let outcome = env.open_deferred_fullscreen_after_enumerate(DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        });
        assert_eq!(outcome, DeferredFsOpenOutcome::WaitingComposition);
        assert!(env.fs_nav_after_pdf_enumerate.is_none());
        assert!(env.book_deferred_open_wait_active());
        assert!(env.fs_nav_deferred_reopen_wait_active());
        assert_eq!(env.selected, None);
        assert_eq!(env.fullscreen_idx, None);
        adopt(&mut env, &service, &scope);
        // The FIFO barrier includes the candidate unit admitted only after the
        // projection adopted its real order and hidden-page removal.
        barrier(&service, &scope);
        env.poll_book_composition_read(&egui::Context::default());
        assert!(env.fs_nav_after_pdf_enumerate.is_none());
        assert!(!env.book_deferred_open_wait_active());
        assert_eq!(
            current_physical_page(&env),
            if hide_all { None } else { Some(2) }
        );
        if hide_all {
            assert_eq!(env.selected, None);
        } else {
            assert_eq!(env.selected, env.fullscreen_idx);
        }
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_composition_poll_cannot_consume_source_owned_deferred_reopen() {
    for with_service in [false, true] {
        let mut env = setup_app();
        let root = env.tmp.path().join("pending-source.zip");
        env.current_folder = Some(root.clone());
        env.items.clear();
        env.visible_indices.clear();
        let (source_sender, source_receiver) = mpsc::channel();
        env.zip_enumerate_pending = Some(ZipEnumeratePending {
            zip_path: root,
            restore_intent: StartupListIntent::PageContinuation,
            input_seq: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: source_receiver,
        });
        let deferred = DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        };
        env.fs_nav_after_pdf_enumerate = Some(deferred.clone());
        let process = with_service.then(|| {
            let data = env.tmp.path().join("pending-source-data");
            runtime(&mut env, &data).1
        });
        env.poll_book_composition_read(&egui::Context::default());
        assert_eq!(env.fs_nav_after_pdf_enumerate.as_ref(), Some(&deferred));
        assert!(env.zip_enumerate_pending.is_some());
        assert!(!env.book_deferred_open_wait_active());
        assert!(env.fs_nav_deferred_reopen_wait_active());
        assert!(env.fullscreen_idx.is_none());
        drop(source_sender);
        if let Some(process) = process {
            process.shutdown_and_join();
        }
    }
}

#[test]
fn g3_source_terminal_reopen_survives_automatic_projection() {
    let mut env = setup_app();
    let root = env.tmp.path().join("automatic-entry.pdf");
    let scope = install_document(&mut env, &root);
    let data = env.tmp.path().join("automatic-entry-data");
    let (service, owner) = runtime(&mut env, &data);
    env.fullscreen_idx = None;
    env.selected = None;
    let outcome = env.open_deferred_fullscreen_after_enumerate(DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::None,
        resume_to_last_page: false,
        from_explicit_open: true,
        preserve_after_password_prompt: true,
    });
    assert_eq!(outcome, DeferredFsOpenOutcome::WaitingComposition);
    assert!(env.book_deferred_open_wait_active());
    adopt(&mut env, &service, &scope);
    barrier(&service, &scope);
    env.poll_book_composition_read(&egui::Context::default());
    assert_eq!(current_physical_page(&env), Some(0));
    assert_eq!(env.selected, env.fullscreen_idx);
    assert!(!env.book_deferred_open_wait_active());
    owner.shutdown_and_join();
}

#[test]
fn g3_source_terminal_required_hidden_or_missing_target_never_uses_another_page() {
    use crate::book_composition::fixtures::document_page;
    for requested_page in [0, 99] {
        let mut env = setup_app();
        let root = env.tmp.path().join("required-entry.pdf");
        let scope = install_document(&mut env, &root);
        let data = env.tmp.path().join("required-entry-data");
        let (service, owner) = runtime(&mut env, &data);
        let mut composition = four_page_book(ReadingDirection::Ltr);
        composition.hidden_pages.push(document_page(0));
        service
            .save(scope.clone(), PageDomain::Document, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        env.fullscreen_idx = None;
        // Keep an unrelated grid selection to prove the refused viewer open
        // cannot select a replacement page. Projection itself remaps this row.
        env.selected = Some(3);
        let deferred = DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: DeferredFsTarget::Required(crate::snapshot::SnapshotTarget::PdfPage {
                pdf_path: root,
                page_num: requested_page,
            }),
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: true,
        };
        let outcome = env.open_deferred_fullscreen_after_enumerate(deferred.clone());
        assert_eq!(outcome, DeferredFsOpenOutcome::WaitingComposition);
        adopt(&mut env, &service, &scope);
        barrier(&service, &scope);
        env.poll_book_composition_read(&egui::Context::default());
        assert_eq!(env.fullscreen_idx, None);
        assert!(matches!(
            env.selected.and_then(|index| env.items.get(index)),
            Some(GridItem::PdfPage { page_num: 3, .. })
        ));
        assert!(!env.book_deferred_open_wait_active());
        let accepted_grid_selection = env.selected;
        assert_eq!(
            env.grid_click_selection_anchor
                .and_then(|anchor| anchor.index_for_generation(env.items_generation)),
            accepted_grid_selection,
            "projection remaps the cursor and range anchor into the same generation",
        );
        assert_eq!(
            env.open_deferred_fullscreen_after_enumerate(deferred),
            if requested_page == 0 {
                DeferredFsOpenOutcome::RequiredTargetHidden
            } else {
                DeferredFsOpenOutcome::RequiredTargetMissing
            }
        );
        assert_eq!(env.selected, accepted_grid_selection);
        assert_eq!(env.fullscreen_idx, None);
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_hidden_refresh_follows_next_previous_first_and_closes_without_resume_write() {
    use crate::book_composition::fixtures::document_page;
    let mut env = setup_app();
    let root = env.tmp.path().join("hidden-refresh.pdf");
    let data = env.tmp.path().join("hidden-refresh-data");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Ltr);
    adopt(&mut env, &service, &scope);
    env.last_book_resume = Some((
        root.clone(),
        1,
        None,
        Some(document_page(1).resume_identity(&root)),
    ));
    let saved_resume = env.last_book_resume.clone();
    for (anchor, hidden, expected) in [
        (1, vec![1], Some(2)),
        (3, vec![1, 3], Some(2)),
        (2, vec![0, 2], Some(1)),
        (1, vec![0, 1, 2, 3], None),
    ] {
        let index = env
            .items
            .iter()
            .position(
                |item| matches!(item, GridItem::PdfPage { page_num, .. } if *page_num == anchor),
            )
            .unwrap();
        env.fullscreen_idx = Some(index);
        env.selected = Some(index);
        let mut composition = four_page_book(ReadingDirection::Ltr);
        composition.hidden_pages = hidden.into_iter().map(document_page).collect();
        service
            .save(scope.clone(), PageDomain::Document, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        admit_projection(&mut env, &service, &scope);
        settle_projection(&mut env);
        assert_eq!(current_physical_page(&env), expected);
        assert_eq!(
            env.grid_click_selection_anchor
                .and_then(|anchor| anchor.index_for_generation(env.items_generation)),
            env.selected,
            "automatic fullscreen landing keeps the shared cursor/anchor owner coherent",
        );
        if let Some(index) = env.fullscreen_idx {
            env.record_book_resume(index);
        }
        assert_eq!(
            env.last_book_resume, saved_resume,
            "automatic projection landing is not a reading action"
        );
    }
    assert!(env.fullscreen_idx.is_none());
    owner.shutdown_and_join();
}

#[test]
fn g3_refresh_keeps_latest_navigation_and_ignores_superseded_source_generation() {
    use crate::book_composition::{BookComposition, fixtures::document_page};
    let mut env = setup_app();
    let root = env.tmp.path().join("refresh-navigation.pdf");
    let data = env.tmp.path().join("refresh-navigation-data");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Ltr);
    adopt(&mut env, &service, &scope);
    env.fullscreen_idx = Some(0);
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            BookComposition::new(
                ReadingDirection::Rtl,
                vec![
                    document_page(2),
                    document_page(3),
                    document_page(0),
                    document_page(1),
                ],
            ),
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    admit_projection(&mut env, &service, &scope);
    env.fullscreen_idx = Some(2);
    env.selected = Some(2);
    settle_projection(&mut env);
    assert_eq!(current_physical_page(&env), Some(2));
    assert_eq!(env.fullscreen_idx, Some(0));
    save(&service, &scope, ReadingDirection::Ltr);
    admit_projection(&mut env, &service, &scope);
    let old_rows = env.items.clone();
    env.items_generation += 1;
    settle_projection(&mut env);
    assert_eq!(
        env.items, old_rows,
        "stale worker generation cannot install rows"
    );
    assert!(!matches!(
        env.book_composition,
        ViewerBookComposition::Accepted { .. }
    ));
    install_document(&mut env, &root);
    admit_projection(&mut env, &service, &scope);
    let old_projection = std::mem::take(&mut env.book_composition);
    let sibling_root = env.tmp.path().join("replacement-source.pdf");
    install_document(&mut env, &sibling_root);
    let sibling_rows = env.items.clone();
    env.book_composition = old_projection;
    settle_projection(&mut env);
    assert_eq!(
        env.items, sibling_rows,
        "old source epoch cannot install rows into a new book"
    );
    assert!(
        env.book_composition
            .scope()
            .is_none_or(|accepted| accepted != &scope)
    );
    owner.shutdown_and_join();
}

#[test]
fn g3_app_trim_edit_and_reset_follow_superseded_projection_saves() {
    use crate::view_trim::{ViewTrimMargins, ViewTrimPageOverride};
    use crate::view_trim_db::{ViewTrimDb, ViewTrimWriteBatch};
    let mut env = setup_app();
    let root = env.tmp.path().join("app-trim-fifo.pdf");
    let data = env.tmp.path().join("app-trim-fifo-data");
    let scope = install_document(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Ltr);
    env.view_trim_db = Some(ViewTrimDb::open_at(&data.join("view_trim.db")).unwrap());
    let key = env.page_path_key(0).unwrap();
    let before = service.snapshot();
    let older = ViewTrimPageOverride::from_margins(ViewTrimMargins {
        left: 0.05,
        ..Default::default()
    });
    let latest = ViewTrimPageOverride::from_margins(ViewTrimMargins {
        right: 0.12,
        ..Default::default()
    });

    // Hold the actual writer so an accidentally synchronous App write would be
    // overwritten by the older queued projection batch when it resumes.
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    drop(service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(key.clone(), older)],
    }));
    env.persist_view_trim_page_override(0, latest);
    release.send(()).unwrap();
    barrier(&service, &scope);
    assert_eq!(
        env.view_trim_db
            .as_ref()
            .unwrap()
            .load_page_overrides_many(&[key.as_str()])
            .get(&key),
        Some(&latest),
        "the real App edit must follow the superseded projection save"
    );
    let after_edit = service.snapshot();
    assert_eq!(before.rows, after_edit.rows);
    assert_eq!(before.token_for(&scope), after_edit.token_for(&scope));

    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    drop(service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(key.clone(), older)],
    }));
    env.remove_view_trim_page_override(0);
    release.send(()).unwrap();
    barrier(&service, &scope);
    assert!(
        env.view_trim_db
            .as_ref()
            .unwrap()
            .load_page_overrides_many(&[key.as_str()])
            .is_empty(),
        "the real App reset must prevent an older projection from restoring the row"
    );
    let after_reset = service.snapshot();
    assert_eq!(before.rows, after_reset.rows);
    assert_eq!(before.token_for(&scope), after_reset.token_for(&scope));
    owner.shutdown_and_join();
}

#[test]
fn g3_completed_projection_reprepares_after_newer_persisted_app_trim_edit() {
    use crate::view_trim::{ViewTrimMargins, ViewTrimPageOverride};
    use crate::view_trim_db::ViewTrimDb;
    let mut env = setup_app();
    let root = env.tmp.path().join("stale-trim-projection.pdf");
    let scope = install_document(&mut env, &root);
    let data = crate::data_dir::get();
    assert!(data.starts_with(env.tmp.path()));
    env.view_trim_db = Some(ViewTrimDb::open_at(&data.join("view_trim.db")).unwrap());
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Ltr);
    admit_projection(&mut env, &service, &scope);
    let state = std::mem::take(&mut env.book_composition);
    let ViewerBookComposition::Projecting {
        scope,
        token,
        record,
        source,
        generation,
        origin,
        receiver,
    } = state
    else {
        panic!("captured real projection worker");
    };
    let reply = receiver
        .recv_timeout(WAIT)
        .expect("old worker completed before the newer edit")
        .unwrap();
    assert!(reply.saved_trim.is_none());
    let prepared = reply
        .edits
        .as_ref()
        .expect("replacement owns its stable page-edit projection");
    assert!(crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared.stamp));
    let original_items = env.items.clone();
    let original_generation = env.items_generation;
    let latest = ViewTrimPageOverride::from_margins(ViewTrimMargins {
        bottom: 0.11,
        ..Default::default()
    });
    let page_key = env.page_path_key(1).unwrap();
    env.persist_view_trim_page_override(1, latest);
    barrier(&service, &scope);
    assert!(!env.view_trim_save_pending);
    assert!(
        env.view_trim_dirty_page_overrides.is_empty(),
        "the newer edit is already persisted, so no pending trim batch can reveal this race"
    );
    assert_eq!(
        env.view_trim_db
            .as_ref()
            .unwrap()
            .get_page_override(&page_key),
        Some(latest)
    );
    assert!(!crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared.stamp));
    let (deliver, receiver) = std::sync::mpsc::channel();
    deliver.send(Ok(reply)).unwrap();
    env.book_composition = ViewerBookComposition::Projecting {
        scope: scope.clone(),
        token,
        record,
        source,
        generation,
        origin,
        receiver,
    };
    env.poll_book_composition_read(&egui::Context::default());
    assert!(
        matches!(
            env.book_composition,
            ViewerBookComposition::Projecting { .. }
        ),
        "a completed old edit snapshot must reprepare instead of adopting"
    );
    assert_eq!(env.items, original_items);
    assert_eq!(env.items_generation, original_generation);
    settle_projection(&mut env);
    assert_accepted_token(&env, &service, &scope);
    assert_eq!(
        env.view_trim_page_overrides.get(&1),
        Some(&latest),
        "the replacement projection installs the newly persisted override"
    );
    assert!(!env.view_trim_save_pending);
    owner.shutdown_and_join();
}

#[test]
fn g3_mode_only_removal_and_document_reentry_preserve_saved_rtl_direction() {
    use crate::settings::{ReadingDirection as Direction, ReadingFlow, SpreadMode};
    use crate::ui_fullscreen::FullscreenViewMode;

    for legacy in [SpreadMode::Single, SpreadMode::Vertical] {
        let mut env = setup_app();
        let root = env.tmp.path().join("dormant-direction.pdf");
        let scope = install_document(&mut env, &root);
        let data = crate::data_dir::get();
        assert!(data.starts_with(env.tmp.path()));
        env.spread_db = Ok(crate::spread_db::SpreadDb::open_at(&data.join("spread.db")).unwrap());
        env.spread_db
            .as_ref()
            .unwrap()
            .set_presentation_state(
                &root,
                legacy,
                ReadingFlow::Paged,
                Direction::Rtl,
                (SpreadMode::Single, ReadingFlow::Paged, Direction::Ltr),
            )
            .unwrap();
        let (service, owner) = runtime(&mut env, &data);
        save(&service, &scope, ReadingDirection::Ltr);
        admit_projection(&mut env, &service, &scope);
        settle_projection(&mut env);
        assert!(env.composed_book_layout().is_some());

        // Both changes use the actual mode-only producer while the composition
        // deliberately has the opposite direction from the dormant binding.
        env.apply_fullscreen_view_mode(&egui::Context::default(), 0, FullscreenViewMode::Single);
        env.apply_fullscreen_view_mode(&egui::Context::default(), 0, FullscreenViewMode::Spread);
        barrier(&service, &scope);
        assert_eq!(env.reading_direction, Direction::Ltr);
        assert_eq!(env.spread_mode, SpreadMode::Ltr);
        service
            .remove(scope.clone(), PageDomain::Document)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        admit_projection(&mut env, &service, &scope);
        settle_projection(&mut env);
        assert!(env.composed_book_layout().is_none());
        assert_eq!(env.spread_mode, SpreadMode::Rtl, "{legacy:?}");
        assert_eq!(env.reading_direction, Direction::Rtl, "{legacy:?}");
        let flow = if legacy == SpreadMode::Vertical {
            ReadingFlow::Vertical
        } else {
            ReadingFlow::Paged
        };
        assert_eq!(env.reading_flow, flow);
        let stored = env
            .spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&root, None);
        assert_eq!(stored.mode, Some(legacy));
        assert_eq!(stored.direction, Some(Direction::Rtl));

        // Fresh entry passes through the same production restore as a new open,
        // and must agree with the actual composition-removal adoption.
        install_document(&mut env, &root);
        let defaults = SpreadRestoreDefaults::for_book(&env.settings);
        env.apply_spread_for_key(&root, defaults).unwrap();
        adopt(&mut env, &service, &scope);
        assert_eq!(env.spread_mode, SpreadMode::Rtl, "{legacy:?}");
        assert_eq!(env.reading_direction, Direction::Rtl);
        assert_eq!(env.reading_flow, flow);
        assert_eq!(
            env.spread_db
                .as_ref()
                .unwrap()
                .get_state_with_fallback(&root, None),
            stored,
            "read-only removal/reentry must retain dormant binding columns"
        );
        owner.shutdown_and_join();
    }
}

#[test]
fn g3_mode_only_archive_removal_and_reentry_preserve_inherited_rtl_cover() {
    use crate::book_composition::{BookComposition, CompositionPageRef};
    use crate::settings::{ReadingDirection as Direction, ReadingFlow, SpreadMode};
    use crate::ui_fullscreen::FullscreenViewMode;
    use std::io::Write;

    let mut env = setup_app();
    let root = env.tmp.path().join("inherited-cover.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
    for page in 0..4 {
        writer
            .start_file(
                format!("chapter/{page}.jpg"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"source payload").unwrap();
    }
    writer.finish().unwrap();
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        root.clone(),
        crate::zip_loader::enumerate_image_entries(&root).unwrap(),
    ));
    let nav = crate::zip_tree::ZipNavState::new(tree);
    let prefix = nav.current_raw().to_vec();
    assert_eq!(prefix, vec!["chapter"]);
    let (items, metas) = nav.materialize_current(crate::settings::SortOrder::FileName);
    env.current_folder = Some(root.clone());
    env.items = items;
    env.image_metas = metas;
    env.thumbnails = vec![ThumbnailState::Pending; env.items.len()];
    env.visible_indices = (0..env.items.len()).collect();
    env.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &env.items,
        Some(&root),
        env.settings.file_type_visibility.clone(),
    ));
    env.projection_recipe =
        book_materialization::ProjectionRecipe::Book(book_materialization::BookScope::Archive {
            logical_root: root.clone(),
            prefix: prefix.clone(),
        });
    env.zip_nav = Some(nav);
    env.book_composition = ViewerBookComposition::Automatic;
    env.spread_mode = SpreadMode::Ltr;
    let scope = BookScopeKey::archive(&root, &prefix).unwrap();
    let domain = PageDomain::ArchiveLevel {
        prefix: scope.level_prefix.clone(),
    };
    let data = crate::data_dir::get();
    assert!(data.starts_with(env.tmp.path()));
    env.spread_db = Ok(crate::spread_db::SpreadDb::open_at(&data.join("spread.db")).unwrap());
    env.spread_db
        .as_ref()
        .unwrap()
        .set_presentation_state(
            &root,
            SpreadMode::RtlCover,
            ReadingFlow::Horizontal,
            Direction::Rtl,
            (SpreadMode::Single, ReadingFlow::Paged, Direction::Ltr),
        )
        .unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let composition = BookComposition::new(
        ReadingDirection::Ltr,
        (0..4)
            .map(|page| CompositionPageRef::ArchiveEntry {
                entry_name: format!("chapter/{page}.jpg"),
            })
            .collect(),
    );
    service
        .save(scope.clone(), domain.clone(), composition)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    service
        .prepare(scope.clone(), domain.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    settle_projection(&mut env);
    assert!(env.composed_book_layout().is_some());
    env.apply_fullscreen_view_mode(&egui::Context::default(), 0, FullscreenViewMode::Single);
    env.apply_fullscreen_view_mode(&egui::Context::default(), 0, FullscreenViewMode::Spread);
    service
        .remove(scope.clone(), domain.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    service
        .prepare(scope.clone(), domain.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    settle_projection(&mut env);
    assert!(env.composed_book_layout().is_none());
    assert_eq!(env.spread_mode, SpreadMode::RtlCover);
    assert_eq!(env.reading_direction, Direction::Rtl);
    assert_eq!(env.reading_flow, ReadingFlow::Horizontal);

    let key = crate::spread_db::container_key(&root, &prefix);
    let db = env.spread_db.as_ref().unwrap();
    assert_eq!(db.get_direction(&key), None);
    assert_eq!(db.get_flow(&key), None);
    let stored = db.get_state_with_fallback(&key, Some(&root));
    assert_eq!(stored.mode, Some(SpreadMode::RtlCover));
    assert_eq!(
        stored.view_mode,
        Some(crate::spread_db::StoredViewMode::Spread)
    );
    env.book_composition = ViewerBookComposition::Automatic;
    env.spread_mode = SpreadMode::Single;
    env.reading_direction = Direction::Ltr;
    let defaults = SpreadRestoreDefaults::for_book(&env.settings);
    env.apply_spread_for_key_with_fallback(&key, Some(&root), defaults)
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    service
        .prepare(scope, domain)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.poll_book_composition_read(&egui::Context::default());
    settle_projection(&mut env);
    assert_eq!(env.spread_mode, SpreadMode::RtlCover);
    assert_eq!(env.reading_direction, Direction::Rtl);
    assert_eq!(env.reading_flow, ReadingFlow::Horizontal);
    assert_eq!(
        env.spread_db.as_ref().unwrap().get(&root),
        Some(SpreadMode::RtlCover)
    );
    assert_eq!(
        env.spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&key, Some(&root)),
        stored
    );
    owner.shutdown_and_join();
}

include!("book_composition_revision_tests.rs");
