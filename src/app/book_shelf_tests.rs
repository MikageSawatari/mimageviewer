//! Deterministic channel tests for the App-wide bookshelf order writer.
use super::*;
use crate::book_order::{BookOrderPolicy, BookOrderWriteError, BookShelfCompletion};

fn shelf(names: &[&str]) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    let root = app.tmp.path().join("ordered-shelf");
    app.settings.book_root = Some(root.clone());
    app.book_shelf.snapshot = Some(
        names
            .iter()
            .map(|name| crate::books::BookInfo {
                name: (*name).into(),
                path: root.join(name),
                page_count: 1,
            })
            .collect::<Vec<_>>()
            .into(),
    );
    app
}

fn names(app: &App) -> Vec<&str> {
    app.book_shelf
        .snapshot
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.name.as_str())
        .collect()
}

fn manual(names: &[&str]) -> BookOrderPolicy {
    BookOrderPolicy::Manual(names.iter().map(|name| (*name).into()).collect())
}

fn succeed(app: &mut App, request: book_shelf::BookOrderWriteForTest, ctx: &egui::Context) {
    request
        .reply
        .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Ok(
            request.token
        ))))
        .unwrap();
    app.poll_book_op_pending(ctx);
}

fn book_order_close_input() -> egui::RawInput {
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    input
}

#[test]
fn book_shelf_logic_only_ready_success_accepts_close_before_receipt() {
    for explicit_quit in [false, true] {
        let mut app = shelf(&["A", "B"]);
        app.settings.minimize_to_tray_on_close = explicit_quit;
        app.shutdown_requested
            .store(explicit_quit, std::sync::atomic::Ordering::SeqCst);
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        let writes = app.book_order_writes_for_test();
        let ctx = egui::Context::default();
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let frame = app.frame_counter;
        let sequence = app.input_seq;
        app.move_book_order("A", 1, &ctx);
        let first = writes.try_recv().unwrap();
        assert!(handle.try_local_modal_lease().is_none());
        first
            .reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Ok(
                first.token
            ))))
            .unwrap();

        let (output, _) = backend.update_logic_only(&mut app.app, book_order_close_input());
        assert!(!app.book_order_saving());
        assert!(app.book_op_pending.is_none());
        assert!(handle.try_local_modal_lease().is_some());
        let commands = &output.viewport_commands[&egui::ViewportId::ROOT];
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert_eq!(
            commands
                .iter()
                .filter(|command| **command == egui::ViewportCommand::Close)
                .count(),
            1
        );
        assert!(!backend.should_close(), "resumed Close is delivered next");
        backend.update_logic_only(&mut app.app, book_order_close_input());
        assert!(backend.should_close());
        assert_eq!(app.frame_counter, frame);
        assert_eq!(app.input_seq, sequence);
        assert!(writes.try_recv().is_err());
    }
}

#[test]
fn book_shelf_logic_only_failure_or_disconnect_releases_lease_and_resumes_close() {
    for disconnected in [false, true] {
        let mut app = shelf(&["A", "B", "C"]);
        app.settings.minimize_to_tray_on_close = false;
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        let writes = app.book_order_writes_for_test();
        let ctx = egui::Context::default();
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        app.move_book_order("A", 1, &ctx);
        let first = writes.try_recv().unwrap();
        app.move_book_order("A", 1, &ctx);
        let (waiting, _) = backend.update_logic_only(&mut app.app, book_order_close_input());
        assert!(
            waiting.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(app.book_order_saving());
        if disconnected {
            drop(first);
        } else {
            first
                .reply
                .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Err(
                    BookOrderWriteError::Failed("logic-only failure".into()),
                ))))
                .unwrap();
        }
        let (terminal, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert!(!app.book_order_saving());
        assert_eq!(app.book_order_status(), Some("未保存"));
        assert!(app.book_op_pending.is_none());
        assert!(handle.try_local_modal_lease().is_some());
        assert!(
            terminal.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(
            writes.try_recv().is_err(),
            "failure must stop the unsent latest write"
        );
    }
}

#[test]
fn book_shelf_logic_only_sends_latest_write_and_defers_close_until_its_terminal() {
    for fail_latest in [false, true] {
        let mut app = shelf(&["A", "B", "C"]);
        app.settings.minimize_to_tray_on_close = false;
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        let writes = app.book_order_writes_for_test();
        let ctx = egui::Context::default();
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        app.move_book_order("A", 1, &ctx);
        let first = writes.try_recv().unwrap();
        app.move_book_order("A", 1, &ctx);
        backend.update_logic_only(&mut app.app, book_order_close_input());
        first
            .reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Ok(
                first.token
            ))))
            .unwrap();
        let (middle, _) = backend.update_logic_only(&mut app.app, Default::default());
        let latest = writes.try_recv().unwrap();
        assert_eq!(latest.policy, manual(&["B", "C", "A"]));
        assert_eq!(names(&app), ["B", "C", "A"]);
        assert!(app.book_order_saving());
        assert!(handle.try_local_modal_lease().is_none());
        assert!(
            !middle
                .viewport_commands
                .values()
                .flatten()
                .any(|command| *command == egui::ViewportCommand::Close)
        );
        latest
            .reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(
                if fail_latest {
                    Err(BookOrderWriteError::Failed("latest failed".into()))
                } else {
                    Ok(latest.token)
                },
            )))
            .unwrap();
        let (terminal, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert!(!app.book_order_saving());
        assert!(app.book_op_pending.is_none());
        assert!(handle.try_local_modal_lease().is_some());
        assert!(
            terminal.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(writes.try_recv().is_err());
    }
}

#[test]
fn book_shelf_logic_only_leaves_list_adoption_for_the_visible_driver() {
    let mut app = shelf(&["A"]);
    let scope = app.begin_book_shelf_read();
    let (reply, rx) = mpsc::channel();
    app.book_op_pending = Some(crate::books::BookOpPending {
        rx,
        intent: crate::books::BookOpIntent::List { navigation: None },
        shelf_scope: Some(scope),
    });
    reply
        .send(Err("List must not be consumed by the order service".into()))
        .unwrap();
    let mut backend = eframe::HeadlessEpiIntegration::new(egui::Context::default());
    backend.update_logic_only(&mut app.app, Default::default());
    assert!(app.book_op_pending.is_some());
    assert_eq!(app.book_order_status(), Some("読込中…"));
}

#[test]
fn book_shelf_writer_coalesces_clicks_and_old_success_keeps_latest_display() {
    let mut app = shelf(&["A", "B", "C"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    assert_eq!(first.policy, manual(&["B", "A", "C"]));
    app.move_book_order("A", 1, &ctx);
    app.move_book_order("B", 1, &ctx);
    assert_eq!(names(&app), ["C", "B", "A"]);
    assert!(writes.try_recv().is_err());
    assert_eq!(app.book_order_status(), Some("保存中…"));
    let mut saved_token = first.token.clone();
    saved_token.directories.push("saved-token-marker".into());
    first
        .reply
        .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Ok(
            saved_token.clone(),
        ))))
        .unwrap();
    app.poll_book_op_pending(&ctx);
    assert_eq!(names(&app), ["C", "B", "A"]);
    assert!(app.book_order_saving());
    let latest = writes.try_recv().unwrap();
    assert_eq!(latest.policy, manual(&["C", "B", "A"]));
    assert_eq!(latest.token, saved_token);
    succeed(&mut app, latest, &ctx);
    assert!(!app.book_order_saving());
    assert!(app.book_op_pending.is_none());
    assert!(writes.try_recv().is_err());
}

#[test]
fn book_shelf_writer_saves_inverse_even_when_it_matches_original_rows() {
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    app.move_book_order("B", 1, &ctx);
    assert_eq!(names(&app), ["A", "B"]);
    succeed(&mut app, first, &ctx);
    assert!(app.book_order_saving());
    let inverse = writes.try_recv().unwrap();
    assert_eq!(inverse.policy, manual(&["A", "B"]));
    succeed(&mut app, inverse, &ctx);
    assert!(!app.book_order_saving());
    assert_eq!(app.book_order_mode_label(), "手動順");
}

#[test]
fn book_shelf_writer_same_content_finishes_latest_revision_without_extra_write() {
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    let first_revision = app.shelf_scope().revision;
    app.move_book_order("B", 1, &ctx);
    app.move_book_order("A", 1, &ctx);
    let latest_revision = app.shelf_scope().revision;
    assert!(latest_revision > first_revision);
    succeed(&mut app, first, &ctx);
    assert!(!app.book_order_saving());
    assert!(app.book_op_pending.is_none());
    assert_eq!(
        app.book_shelf.snapshot.as_ref().unwrap().revision,
        latest_revision
    );
    assert_eq!(names(&app), ["B", "A"]);
    assert!(writes.try_recv().is_err());
}

#[test]
fn book_shelf_writer_compares_policy_when_manual_and_name_have_same_rows() {
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.fix_book_order(&ctx);
    let first = writes.try_recv().unwrap();
    assert_eq!(first.policy, manual(&["A", "B"]));
    app.reset_book_order(&ctx);
    assert_eq!(names(&app), ["A", "B"]);
    succeed(&mut app, first, &ctx);
    let name = writes.try_recv().unwrap();
    assert_eq!(name.policy, BookOrderPolicy::Name);
    app.fix_book_order(&ctx);
    succeed(&mut app, name, &ctx);
    let manual_again = writes.try_recv().unwrap();
    assert_eq!(manual_again.policy, manual(&["A", "B"]));
    succeed(&mut app, manual_again, &ctx);
    assert!(!app.book_order_saving());
}

#[test]
fn book_shelf_writer_failure_stops_unsent_latest_until_next_explicit_edit() {
    let mut app = shelf(&["A", "B", "C"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    app.move_book_order("A", 1, &ctx);
    first
        .reply
        .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Err(
            BookOrderWriteError::Failed("test write failure".into()),
        ))))
        .unwrap();
    app.poll_book_op_pending(&ctx);
    assert_eq!(names(&app), ["B", "C", "A"]);
    assert_eq!(app.book_order_status(), Some("未保存"));
    assert!(!app.book_order_saving());
    assert!(app.book_op_pending.is_none());
    assert!(writes.try_recv().is_err());
    app.poll_book_op_pending(&ctx);
    assert!(writes.try_recv().is_err());
    app.move_book_order("B", 1, &ctx);
    let explicit = writes.try_recv().unwrap();
    assert_eq!(explicit.policy, manual(&["C", "B", "A"]));
    succeed(&mut app, explicit, &ctx);
    assert_eq!(app.book_order_status(), None);
}

#[test]
fn book_shelf_writer_survives_manager_close_reopen_without_read_or_extra_write() {
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.show_book_manager = true;
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    let request_revision = app.shelf_scope().revision;
    app.show_book_manager = false;
    app.open_book_manager_from_action();
    assert!(app.show_book_manager);
    assert!(app.book_order_saving());
    assert_eq!(app.shelf_scope().revision, request_revision);
    assert!(!app.book_list_refresh_pending());
    assert_eq!(names(&app), ["B", "A"]);
    assert!(writes.try_recv().is_err());
    succeed(&mut app, first, &ctx);
    assert!(!app.book_order_saving());
}

#[test]
fn book_shelf_root_epoch_rejects_old_completed_list_even_after_return() {
    let mut app = shelf(&["A", "B"]);
    let original = app.book_root_path();
    let scope = app.begin_book_shelf_read();
    let old_snapshot = app.book_shelf.snapshot.as_ref().unwrap().clone();
    app.settings.book_root = Some(app.tmp.path().join("other-shelf"));
    app.book_shelf_root_changed();
    app.settings.book_root = Some(original);
    app.book_shelf_root_changed();
    app.adopt_book_shelf_completion(
        BookShelfCompletion {
            snapshot: Ok(old_snapshot),
            warning: None,
        },
        Some(&scope),
    );
    assert!(app.book_shelf.snapshot.is_none());
    assert!(!app.shelf_scope_current(&scope));
}

#[test]
fn book_shelf_root_change_keeps_captured_mutation_receiver() {
    for append in [false, true] {
        let mut app = shelf(&["A"]);
        let scope = app.shelf_scope();
        let (reply, rx) = mpsc::channel();
        let intent = if append {
            crate::books::BookOpIntent::Append {
                operation_id: 1683,
                source_context: app.projected_viewer_context_id(),
                accepted_revision: app.book_append_undo_revision,
                target_folder: app.book_root_path().join("A"),
                skipped_containers: 0,
                observations: Default::default(),
            }
        } else {
            crate::books::BookOpIntent::Unrelated
        };
        app.book_op_pending = Some(crate::books::BookOpPending {
            rx,
            intent,
            shelf_scope: Some(scope.clone()),
        });
        let mut settings = app.settings.clone();
        settings.book_root = Some(app.tmp.path().join("new-shelf"));
        app.install_preferences_settings(settings);
        assert!(!app.shelf_scope_current(&scope));
        let pending = app.book_op_pending.as_ref().unwrap();
        assert_eq!(pending.shelf_scope.as_ref().unwrap().epoch, scope.epoch);
        assert!(reply.send(Err("old-root mutation terminal".into())).is_ok());
        assert!(pending.rx.try_recv().unwrap().is_err());
    }
}

#[test]
fn book_shelf_writer_lease_blocks_remote_and_other_mutations_until_terminal() {
    let mut app = shelf(&["A", "B"]);
    let root = app.book_root_path();
    let folder = root.join("A");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "A".into()).unwrap();
    let page = folder.join("0001_page.jpg");
    std::fs::write(&page, b"temporary page").unwrap();
    builder.record(&page).unwrap();
    let receipt = crate::book_append_undo::BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 1683,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    };
    app.book_append_undo_receipt = Some(receipt.clone());
    let writes = app.book_order_writes_for_test();
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    let ctx = egui::Context::default();
    let undo_revision = app.book_append_undo_revision;
    app.move_book_order("A", 1, &ctx);
    let first = writes.try_recv().unwrap();
    assert!(handle.try_local_modal_lease().is_none());
    assert!(!app.admit_book_append_followup_mutation());
    assert_eq!(app.book_append_undo_revision, undo_revision);
    assert_eq!(
        app.book_append_undo_receipt.as_ref().unwrap().operation(),
        receipt.operation(),
    );
    let root_before = app.book_root_path();
    let mut settings = app.settings.clone();
    settings.book_root = Some(app.tmp.path().join("rejected-shelf"));
    app.install_preferences_settings(settings);
    assert_eq!(app.book_root_path(), root_before);
    succeed(&mut app, first, &ctx);
    assert!(handle.try_local_modal_lease().is_some());
    assert_eq!(app.book_append_undo_revision, undo_revision);
    assert_eq!(
        app.book_append_undo_receipt.as_ref().unwrap().operation(),
        receipt.operation(),
    );
}

#[test]
fn book_shelf_writer_existing_modal_lease_rejection_preserves_display_and_mode() {
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let handle = crate::remote_ipc::session::SessionHandle::new();
    let _lease = handle.try_local_modal_lease().unwrap();
    app.set_remote_session_handle(handle);
    app.move_book_order("A", 1, &egui::Context::default());
    assert_eq!(names(&app), ["A", "B"]);
    assert_eq!(app.book_order_mode_label(), "名前順");
    assert!(!app.book_order_saving());
    assert!(writes.try_recv().is_err());
}

#[test]
fn book_shelf_first_order_edit_waits_for_pending_and_unadopted_book_results() {
    for buffered in [false, true] {
        for append in [false, true] {
            let mut app = shelf(&["A", "B"]);
            let writes = app.book_order_writes_for_test();
            let (reply, rx) = mpsc::channel();
            let intent = if append {
                crate::books::BookOpIntent::Append {
                    operation_id: 1683,
                    source_context: app.projected_viewer_context_id(),
                    accepted_revision: app.book_append_undo_revision,
                    target_folder: app.book_root_path().join("A"),
                    skipped_containers: 0,
                    observations: Default::default(),
                }
            } else {
                crate::books::BookOpIntent::List { navigation: None }
            };
            app.book_op_pending = Some(crate::books::BookOpPending {
                rx,
                intent,
                shelf_scope: Some(app.shelf_scope()),
            });
            if buffered {
                reply.send(Err("unadopted terminal".into())).unwrap();
            }
            app.move_book_order("A", 1, &egui::Context::default());
            assert_eq!(names(&app), ["A", "B"]);
            assert_eq!(app.book_order_mode_label(), "名前順");
            assert!(!app.book_order_saving());
            assert!(writes.try_recv().is_err());
            assert!(app.book_op_pending.is_some());
        }
    }
    let mut app = shelf(&["A", "B"]);
    let writes = app.book_order_writes_for_test();
    let _held_structure = app.hold_book_structure_for_test();
    app.move_book_order("A", 1, &egui::Context::default());
    assert_eq!(names(&app), ["A", "B"]);
    assert!(!app.book_order_saving());
    assert!(writes.try_recv().is_err());
}

#[test]
fn book_shelf_normal_quit_waits_for_latest_write_terminal_including_failure() {
    for fail in [false, true] {
        let mut app = shelf(&["A", "B", "C"]);
        let writes = app.book_order_writes_for_test();
        let ctx = egui::Context::default();
        app.move_book_order("A", 1, &ctx);
        let first = writes.try_recv().unwrap();
        app.move_book_order("A", 1, &ctx);
        let mut waiting = ctx.run_pass(Default::default(), |ctx| {
            assert!(app.accept_book_order_root_close(
                ctx,
                root_lifecycle::RootCloseIntent::ApplicationQuit
            ));
        });
        assert!(
            waiting.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(
            !waiting.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        // This lifecycle test has no renderer and deliberately discards atlas uploads.
        waiting.textures_delta.clear();
        succeed(&mut app, first, &ctx);
        let latest = writes.try_recv().unwrap();
        assert!(app.book_order_saving());
        latest
            .reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(if fail {
                Err(BookOrderWriteError::Failed("test".into()))
            } else {
                Ok(latest.token)
            })))
            .unwrap();
        let mut terminal = ctx.run_pass(Default::default(), |ctx| app.poll_book_op_pending(ctx));
        assert!(!app.book_order_saving());
        assert!(
            terminal.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        terminal.textures_delta.clear();
        assert!(writes.try_recv().is_err());
    }
}

#[test]
fn book_shelf_refresh_of_disappeared_existing_root_retains_last_complete_rows() {
    let mut app = shelf(&["A"]);
    let root = app.book_root_path();
    std::fs::create_dir_all(root.join("A")).unwrap();
    app.book_shelf.snapshot = Some(crate::book_order::read_snapshot(&root).unwrap());
    // Only this test's freshly created shelf is removed; no user data is used.
    std::fs::remove_dir(root.join("A")).unwrap();
    std::fs::remove_dir(&root).unwrap();
    app.request_book_list_refresh();
    let pending = app.book_op_pending.take().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(result.is_err());
    app.complete_book_op_result(
        &egui::Context::default(),
        result,
        &pending.intent,
        pending.shelf_scope.as_ref(),
    );
    assert_eq!(names(&app), ["A"]);
    assert!(!root.exists());
}

#[test]
fn book_shelf_initial_empty_save_adopts_created_root_observation() {
    let mut app = shelf(&[]);
    let root = app.book_root_path();
    app.book_shelf.snapshot = Some(crate::book_order::read_snapshot(&root).unwrap());
    let writes = app.book_order_writes_for_test();
    let ctx = egui::Context::default();
    app.fix_book_order(&ctx);
    let request = writes.try_recv().unwrap();
    let token = crate::book_order::write_order(&root, &request.token, &request.policy).unwrap();
    request
        .reply
        .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(Ok(token))))
        .unwrap();
    app.poll_book_op_pending(&ctx);
    assert_eq!(
        app.book_shelf.snapshot.as_ref().unwrap().root_presence,
        crate::book_order::RootPresence::Existing
    );
    std::fs::remove_file(root.join(crate::book_order::ORDER_FILE_NAME)).unwrap();
    std::fs::remove_dir(&root).unwrap();
    app.request_book_list_refresh();
    let pending = app.book_op_pending.take().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(result.is_err());
    app.complete_book_op_result(&ctx, result, &pending.intent, pending.shelf_scope.as_ref());
    assert_eq!(app.book_order_mode_label(), "手動順");
    assert_eq!(
        app.book_shelf.snapshot.as_ref().unwrap().root_presence,
        crate::book_order::RootPresence::Existing
    );
}

#[test]
fn book_shelf_cold_read_failure_is_terminal_until_an_explicit_reload() {
    let mut app = shelf(&[]);
    let root = app.book_root_path();
    std::fs::write(&root, b"not a directory").unwrap();
    app.book_shelf.snapshot = None;
    app.ensure_book_list_loaded();
    let pending = app.book_op_pending.take().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(result.is_err());
    app.complete_book_op_result(
        &egui::Context::default(),
        result,
        &pending.intent,
        pending.shelf_scope.as_ref(),
    );
    assert_eq!(app.book_order_status(), Some("本棚一覧を読み込めません"));
    let revision = app.shelf_scope().revision;
    for _ in 0..3 {
        app.ensure_book_list_loaded();
    }
    assert!(app.book_op_pending.is_none());
    assert_eq!(app.shelf_scope().revision, revision);
    // The existing explicit update is the one recovery entry.
    std::fs::remove_file(&root).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    app.request_book_list_refresh();
    let pending = app.book_op_pending.take().unwrap();
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(result.is_ok());
    app.complete_book_op_result(
        &egui::Context::default(),
        result,
        &pending.intent,
        pending.shelf_scope.as_ref(),
    );
    assert_eq!(app.book_order_status(), None);
    assert!(app.book_shelf.snapshot.is_some());
}

#[test]
fn book_shelf_returned_book_append_then_order_conflict_reloads_and_next_save_succeeds() {
    for action in ["move", "fix", "name"] {
        let mut app = shelf(&["A", "Z"]);
        let root = app.book_root_path();
        for name in ["A", "Z"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let initial = crate::book_order::read_snapshot(&root).unwrap();
        if action != "fix" {
            crate::book_order::write_order(&root, initial.token().unwrap(), &manual(&["Z", "A"]))
                .unwrap();
        }
        app.book_shelf.snapshot = Some(crate::book_order::read_snapshot(&root).unwrap());
        let old_token = app
            .book_shelf
            .snapshot
            .as_ref()
            .unwrap()
            .token()
            .unwrap()
            .clone();
        let original = std::fs::read(root.join(crate::book_order::ORDER_FILE_NAME)).ok();

        // The destination returned externally after this list was observed.
        std::fs::create_dir(root.join("B")).unwrap();
        app.settings.active_book_name = "B".into();
        app.current_folder = None;
        let source = app.tmp.path().join("returned-source.jpg");
        std::fs::write(&source, b"appended page").unwrap();
        let (append_reply, rx) = mpsc::channel();
        app.book_op_pending = Some(crate::books::BookOpPending {
            rx,
            intent: crate::books::BookOpIntent::Append {
                operation_id: 1683,
                source_context: app.projected_viewer_context_id(),
                accepted_revision: app.book_append_undo_revision,
                target_folder: root.join("B"),
                skipped_containers: 0,
                observations: Default::default(),
            },
            shelf_scope: Some(app.shelf_scope()),
        });
        app.open_book_manager_from_action();
        assert!(app.show_book_manager);
        assert!(matches!(
            app.book_op_pending.as_ref().unwrap().intent,
            crate::books::BookOpIntent::Append { .. }
        ));
        assert!(
            !app.book_list_refresh_pending(),
            "manager open cannot read during Append"
        );
        let data = crate::data_dir::get();
        let appended = crate::books::append_pages_at_with_raw_and_shelf(
            data.clone(),
            root.clone(),
            "B".into(),
            vec![crate::books::BookPageSource::File {
                src: source,
                original_name: "returned-source.jpg".into(),
            }],
            crate::book_append_undo::AppendMetadataStores::existing_at(&data),
            None,
        )
        .unwrap();
        assert!(
            matches!(&appended, crate::books::BookOpResult::Append(summary) if summary.added == 1 && summary.shelf.is_none())
        );
        append_reply.send(Ok(appended)).unwrap();
        let ctx = egui::Context::default();
        // Real Append can retain this receiver through asynchronous metadata
        // presence preparation. Drive that terminal too, rather than assuming
        // the initial file-copy reply alone completes the operation.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.book_op_pending.is_some() {
            app.poll_book_op_pending(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "Append terminal timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(names(&app).contains(&"B"));
        assert_eq!(
            app.book_shelf.snapshot.as_ref().unwrap().token(),
            Some(&old_token)
        );
        assert_eq!(old_token.directories, ["A", "Z"]);

        let writes = app.book_order_writes_for_test();
        let edit = |app: &mut App| match action {
            "move" => app.move_book_order("B", -1, &ctx),
            "fix" => app.fix_book_order(&ctx),
            "name" => app.reset_book_order(&ctx),
            _ => unreachable!(),
        };
        edit(&mut app);
        let first = writes.try_recv().unwrap();
        assert_eq!(
            first.token, old_token,
            "do not infer a token from the display delta"
        );
        let result = crate::book_order::write_order(&root, &first.token, &first.policy);
        assert_eq!(result, Err(BookOrderWriteError::Conflict), "{action}");
        assert_eq!(
            std::fs::read(root.join(crate::book_order::ORDER_FILE_NAME)).ok(),
            original
        );
        first
            .reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(result)))
            .unwrap();
        app.poll_book_op_pending(&ctx);
        assert_eq!(app.book_order_status(), Some("読込中…"));
        assert!(app.book_list_refresh_pending());
        assert!(
            writes.try_recv().is_err(),
            "conflict must not resend the old desired order"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.book_list_refresh_pending() {
            app.poll_book_op_pending(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "conflict reload timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(app.book_order_status(), None);
        assert_eq!(
            app.book_shelf
                .snapshot
                .as_ref()
                .unwrap()
                .token()
                .unwrap()
                .directories,
            ["A", "B", "Z"]
        );
        edit(&mut app);
        let next = writes.try_recv().unwrap();
        let expected_policy = next.policy.clone();
        let result = crate::book_order::write_order(&root, &next.token, &next.policy);
        assert!(result.is_ok(), "{action}: {result:?}");
        next.reply
            .send(Ok(crate::books::BookOpResult::ShelfOrderSaved(result)))
            .unwrap();
        app.poll_book_op_pending(&ctx);
        assert!(!app.book_order_saving());
        assert!(app.book_op_pending.is_none());
        assert_eq!(app.book_order_status(), None);
        assert_eq!(
            crate::book_order::read_snapshot(&root).unwrap().policy,
            expected_policy
        );
        assert!(writes.try_recv().is_err());
    }
}
