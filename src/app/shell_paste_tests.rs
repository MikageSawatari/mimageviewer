//! Handler regressions for the three mIV Shell paste entrances. Never invokes Shell.

use super::*;
use crate::app::{AppTestEnvForTest, NormalFolderOmittedEntries, setup_app_for_test};
use crate::post_operation_selection::ExpectedOutputs;
use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest};
use crate::shell_paste::{ShellPasteError, ShellPasteService};
use crate::ui_dialogs::file_organize::{
    FileOrganizeRequest, FileOrganizeRunning, FileOrganizeSelection,
};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

fn ready() -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.indexer_init.mark_unavailable_if_not_ready();
    app.main_hwnd = Some(1);
    app.settings.book_root = Some(app.tmp.path().join("books"));
    let folder = app.tmp.path().join("current");
    std::fs::create_dir(&folder).unwrap();
    app.current_folder = Some(folder.clone());
    app.current_folder_last_mtime = Some(std::time::SystemTime::now());
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
        folder,
        omitted: Default::default(),
    });
    app
}

struct PasteWorker {
    entered: mpsc::Receiver<(std::thread::ThreadId, isize, PathBuf)>,
    finish: mpsc::Sender<Result<(), ShellPasteError>>,
}

fn install_worker(app: &mut App) -> PasteWorker {
    let (entered_tx, entered) = mpsc::channel();
    let (finish, finish_rx) = mpsc::channel();
    app.shell_paste_service = ShellPasteService::with_test_invoker(move |hwnd, folder| {
        entered_tx
            .send((std::thread::current().id(), hwnd, folder.to_owned()))
            .unwrap();
        finish_rx
            .recv_timeout(WAIT)
            .expect("release fake paste worker")
    });
    PasteWorker { entered, finish }
}

fn finish_paste(app: &mut App, ctx: &egui::Context, worker: &PasteWorker, succeeds: bool) {
    worker
        .finish
        .send(if succeeds {
            Ok(())
        } else {
            Err(ShellPasteError::Invocation("fake invocation error".into()))
        })
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while app.shell_paste_request.is_some() {
        app.poll_shell_paste(ctx);
        assert!(
            Instant::now() < deadline,
            "fake paste completion was not consumed"
        );
        std::thread::yield_now();
    }
}

fn enter(app: &mut App, ctx: &egui::Context, origin: ShellPasteOrigin) {
    let folder = app.current_folder.clone().unwrap();
    match origin {
        ShellPasteOrigin::Tree => app.paste_tree_files_to_folder(ctx, folder),
        ShellPasteOrigin::CtrlV => app.handle_clipboard_paste_with(
            ctx,
            || Ok(crate::clipboard_capture::data::ClipboardKind::Files),
            |app, ctx, _, _, folder| app.paste_files_to_folder(ctx, folder),
        ),
        ShellPasteOrigin::GridMenu => app.dispatch_grid_paste_command_for_test(ctx, folder),
    }
}

fn transfer(folder: PathBuf) -> ShellTransferRequest {
    ShellTransferRequest {
        sources: vec![folder.join("source.jpg")],
        destination: folder.join("destination"),
        operation: ShellTransferOperation::Copy,
        default_move_needs_volume_check: false,
    }
}

fn selecting(request: &ShellTransferRequest) -> FileOrganizeRequest {
    FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
        sources: request.sources.clone(),
        destinations: vec![crate::settings::FileOrganizeDestination {
            name: "destination".into(),
            path: request.destination.clone(),
        }],
        focus: None,
    }))
}

fn install_receipt(
    app: &mut App,
    root: &std::path::Path,
) -> crate::book_append_undo::AppendOperation {
    let folder = root.join("Book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(root, &folder, "Book".into()).unwrap();
    let output = folder.join("added.jpg");
    std::fs::write(&output, b"test-page").unwrap();
    builder.record(&output).unwrap();
    let receipt = crate::book_append_undo::BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 41,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    };
    let operation = receipt.operation();
    app.book_append_undo_receipt = Some(receipt);
    operation
}

#[test]
fn shell_paste_tree_ctrl_v_and_grid_menu_invoke_once_off_caller_and_keep_origin() {
    for origin in [
        ShellPasteOrigin::Tree,
        ShellPasteOrigin::CtrlV,
        ShellPasteOrigin::GridMenu,
    ] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        let caller = std::thread::current().id();
        enter(&mut app, &ctx, origin);
        let (thread, hwnd, folder) = worker.entered.recv_timeout(WAIT).unwrap();
        assert_ne!(thread, caller);
        assert_eq!(hwnd, 1);
        assert_eq!(Some(folder), app.current_folder);
        assert_eq!(app.shell_paste_request.as_ref().unwrap().origin, origin);
        assert!(worker.entered.try_recv().is_err());
        // The invocation remains blocked while the handler has already returned.
        app.poll_shell_paste(&ctx);
        assert!(app.shell_paste_request.is_some());
        finish_paste(&mut app, &ctx, &worker, true);
        app.poll_shell_paste(&ctx);
        assert!(worker.entered.try_recv().is_err());
    }
}

#[test]
fn shell_paste_duplicate_preserves_undo_receipt_revision_and_exact_selection() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    let selection = app.post_operation_selection.as_ref().unwrap().id();
    let root = app.tmp.path().join("receipt");
    let operation = install_receipt(&mut app, &root);
    let revision = app.book_append_undo_revision;
    for origin in [
        ShellPasteOrigin::Tree,
        ShellPasteOrigin::CtrlV,
        ShellPasteOrigin::GridMenu,
    ] {
        enter(&mut app, &ctx, origin);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "貼り付けが完了するまでお待ちください"
        );
        assert_eq!(app.book_append_undo_revision, revision);
        assert!(app.book_append_operation_matches_receipt(operation));
        assert_eq!(
            app.post_operation_selection.as_ref().unwrap().id(),
            selection
        );
        assert_eq!(
            app.shell_paste_request.as_ref().unwrap().selection_id,
            Some(selection)
        );
        assert!(worker.entered.try_recv().is_err());
    }
    finish_paste(&mut app, &ctx, &worker, true);
}

#[test]
fn shell_paste_running_refuses_tree_transfer_and_organize_before_mutation() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    let revision = app.book_append_undo_revision;
    let request = transfer(app.tmp.path().to_owned());
    app.submit_shell_transfer_with(
        &ctx,
        request.clone(),
        |_| panic!("paste refusal must precede root resolution"),
        |_, _, _| panic!("paste refusal must precede transfer spawn"),
    );
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "貼り付けが完了するまでお待ちください"
    );
    app.file_organize_request = selecting(&request);
    app.submit_file_organize_with(
        &ctx,
        0,
        request.operation,
        |_| panic!("paste refusal must precede root resolution"),
        |_, _, _| panic!("paste refusal must precede organize spawn"),
    );
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "貼り付けが完了するまでお待ちください"
    );
    assert_eq!(app.book_append_undo_revision, revision);
    assert!(matches!(
        app.file_organize_request,
        FileOrganizeRequest::Hidden
    ));
    finish_paste(&mut app, &ctx, &worker, true);
}

#[test]
fn shell_paste_organize_running_refuses_tree_but_preserves_ctrl_v_and_grid_menu() {
    for origin in [
        ShellPasteOrigin::Tree,
        ShellPasteOrigin::CtrlV,
        ShellPasteOrigin::GridMenu,
    ] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        let request = transfer(app.tmp.path().to_owned());
        let (_held, rx) = mpsc::channel();
        app.file_organize_request =
            FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
        let revision = app.book_append_undo_revision;
        enter(&mut app, &ctx, origin);
        assert!(matches!(
            app.file_organize_request,
            FileOrganizeRequest::Running(_)
        ));
        if origin == ShellPasteOrigin::Tree {
            assert!(app.shell_paste_request.is_none());
            assert!(worker.entered.try_recv().is_err());
            assert_eq!(app.book_append_undo_revision, revision);
            assert_eq!(
                app.fs_feedback_toast.as_ref().unwrap().0,
                "整理操作が完了するまでお待ちください"
            );
        } else {
            worker.entered.recv_timeout(WAIT).unwrap();
            assert_eq!(app.book_append_undo_revision, revision.wrapping_add(1));
            finish_paste(&mut app, &ctx, &worker, true);
        }
    }
}

#[test]
fn shell_paste_failed_completion_retires_only_its_selection_and_new_folder_completion_survives() {
    for new_folder in [false, true] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
        worker.entered.recv_timeout(WAIT).unwrap();
        let paste_id = app.post_operation_selection.as_ref().unwrap().id();
        let mut new_folder_id = None;
        if new_folder {
            let parent = app.current_folder.clone().unwrap();
            let created = parent.join("created-by-new-folder");
            std::fs::create_dir(&created).unwrap();
            let (tx, rx) = mpsc::channel();
            app.new_folder_pending = Some(rx);
            tx.send(Ok(crate::ui_dialogs::new_folder::NewFolderCreated {
                parent,
                path: created.clone(),
            }))
            .unwrap();
            app.poll_new_folder_pending(&ctx);
            assert!(app.new_folder_pending.is_none());
            assert!(app.pending_reload);
            let request = app.post_operation_selection.as_ref().unwrap();
            assert_ne!(request.id(), paste_id);
            new_folder_id = Some(request.id());
            assert_eq!(
                crate::post_operation_selection::decide(
                    request,
                    app.current_folder.as_deref(),
                    &[(0, created)],
                    Instant::now()
                ),
                crate::post_operation_selection::Step::Apply(vec![0])
            );
        }
        finish_paste(&mut app, &ctx, &worker, false);
        assert_eq!(
            app.post_operation_selection
                .as_ref()
                .map(|request| request.id()),
            new_folder_id
        );
    }
}

#[test]
fn shell_paste_success_keeps_selection_repaints_root_and_resyncs_modifiers_once() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    let id = app.post_operation_selection.as_ref().unwrap().id();
    super::take_modifier_resync_calls_for_test();
    ctx.run_pass(Default::default(), |_| {})
        .drop_without_applying_deltas();
    ctx.run_pass(Default::default(), |_| {})
        .drop_without_applying_deltas();
    let output = ctx.run_pass(Default::default(), |ctx| {
        finish_paste(&mut app, ctx, &worker, true)
    });
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        Duration::ZERO
    );
    output.drop_without_applying_deltas();
    assert_eq!(app.post_operation_selection.as_ref().unwrap().id(), id);
    assert_eq!(super::take_modifier_resync_calls_for_test(), 1);
    app.poll_shell_paste(&ctx);
    assert_eq!(super::take_modifier_resync_calls_for_test(), 0);
}

#[test]
fn shell_paste_disconnected_completion_warns_and_retires_its_selection() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    let (_tx, disconnected) = mpsc::channel();
    drop(_tx);
    app.shell_paste_request.as_mut().unwrap().rx = disconnected;
    super::take_modifier_resync_calls_for_test();
    app.poll_shell_paste(&ctx);
    assert!(app.shell_paste_request.is_none());
    assert!(app.post_operation_selection.is_none());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "貼り付けを完了できませんでした"
    );
    assert_eq!(super::take_modifier_resync_calls_for_test(), 1);
    // Release the detached fake worker; it still never reaches the real Shell.
    worker.finish.send(Ok(())).unwrap();
}

#[test]
fn shell_paste_tree_refusals_resolve_books_only_after_predicate_and_keep_selection() {
    for blocker in ["modal", "no-hwnd", "shutdown", "books-root", "books-child"] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        let books = app.tmp.path().join("books");
        let folder = match blocker {
            "books-root" => books.clone(),
            "books-child" => books.join("Book/subfolder"),
            _ => app.current_folder.clone().unwrap(),
        };
        let prior =
            app.request_post_operation_selection(folder.clone(), ExpectedOutputs::Known(vec![]));
        match blocker {
            "modal" => app.show_stats_dialog = true,
            "no-hwnd" => app.main_hwnd = None,
            "shutdown" => app
                .shutdown_requested
                .store(true, std::sync::atomic::Ordering::SeqCst),
            _ => {}
        }
        let revision = app.book_append_undo_revision;
        let mut resolutions = 0;
        app.start_shell_paste_with(&ctx, folder, ShellPasteOrigin::Tree, |_| {
            resolutions += 1;
            books
        });
        assert_eq!(
            resolutions,
            usize::from(blocker.starts_with("books")),
            "{blocker}"
        );
        assert!(app.shell_paste_request.is_none());
        assert!(worker.entered.try_recv().is_err());
        assert_eq!(app.book_append_undo_revision, revision);
        assert_eq!(app.post_operation_selection.as_ref().unwrap().id(), prior);
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn shell_paste_selection_ids_are_unique_and_survive_wait_apply_take_restore() {
    let mut app = ready();
    let folder = app.current_folder.clone().unwrap();
    let first =
        app.request_post_operation_selection(folder.clone(), ExpectedOutputs::Known(vec![]));
    let second = app.request_post_operation_selection_for_added_items(folder.clone());
    assert_ne!(first, second);
    // Empty output list takes and returns the same request while waiting.
    assert_eq!(app.apply_post_operation_checks(), None);
    assert_eq!(app.post_operation_selection.as_ref().unwrap().id(), second);
    let output = folder.join("new.jpg");
    app.items = vec![crate::grid_item::GridItem::Image(output)];
    app.visible_indices = vec![0];
    assert_eq!(app.apply_post_operation_checks(), Some(0));
    assert_eq!(app.post_operation_selection.as_ref().unwrap().id(), second);
    assert!(app.checked.is_empty());
    let taken = app.post_operation_selection.take().unwrap();
    assert_eq!(taken.id(), second);
    app.post_operation_selection = Some(taken);
    let third = app.request_post_operation_selection(folder, ExpectedOutputs::Known(vec![]));
    assert_ne!(first, third);
    assert_ne!(second, third);
}

#[test]
fn shell_paste_pending_file_cut_copy_have_no_write_side_effects_and_completion_restores_admission()
{
    use crate::native_context_menu::ShellClipboardVerb::{Copy, Cut};
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    let file = app.current_folder.as_ref().unwrap().join("source.jpg");
    app.items = vec![crate::grid_item::GridItem::Image(file.clone())];
    app.selected = Some(0);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    let reservations =
        crate::ui_dialogs::context_menu::clipboard_file_write_reservations_for_test();
    let local_writes = crate::cut_clipboard::local_write_calls_for_test();
    app.fullscreen_idx = Some(0);
    for action in [
        crate::keymap::KeyAction::GridCopyFiles,
        crate::keymap::KeyAction::FsCopyFiles,
    ] {
        app.execute_copy_key_action(&ctx, action);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "貼り付けが完了するまでお待ちください"
        );
        assert_eq!(
            crate::ui_dialogs::context_menu::clipboard_file_write_reservations_for_test(),
            reservations
        );
        assert_eq!(
            crate::cut_clipboard::local_write_calls_for_test(),
            local_writes
        );
    }
    // Keyboard cut/copy and menu path dispatch meet at the same guard. Calling
    // their real wrappers is safe here precisely because pending must reject.
    for verb in [Copy, Cut] {
        app.invoke_shell_clipboard_verb(&ctx, verb);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "貼り付けが完了するまでお待ちください"
        );
        app.fs_feedback_toast = None;
        app.dispatch_grid_item_command_for_test(
            &ctx,
            crate::grid_item::GridItem::Image(file.clone()),
            if verb == Cut {
                crate::context_menu_model::MenuCommand::CutFiles
            } else {
                crate::context_menu_model::MenuCommand::CopyFiles
            },
        );
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "貼り付けが完了するまでお待ちください"
        );
        app.invoke_shell_clipboard_verb_for_paths_with(
            &ctx,
            std::slice::from_ref(&file),
            verb,
            |_, _, _, _| panic!("pending paste invoked a file clipboard verb"),
        );
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "貼り付けが完了するまでお待ちください"
        );
        assert_eq!(
            crate::ui_dialogs::context_menu::clipboard_file_write_reservations_for_test(),
            reservations
        );
        assert_eq!(
            crate::cut_clipboard::local_write_calls_for_test(),
            local_writes
        );
        assert!(!app.cut_clipboard.contains(&file));
    }
    finish_paste(&mut app, &ctx, &worker, true);
    for (index, verb) in [Copy, Cut].into_iter().enumerate() {
        let mut calls = 0;
        app.invoke_shell_clipboard_verb_for_paths_with(
            &ctx,
            std::slice::from_ref(&file),
            verb,
            |hwnd, actual, actual_verb, _| {
                calls += 1;
                assert_eq!(hwnd, 1);
                assert_eq!(actual, std::slice::from_ref(&file));
                assert_eq!(actual_verb, verb);
                Ok(100 + index as u32)
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(
            crate::ui_dialogs::context_menu::clipboard_file_write_reservations_for_test(),
            reservations + index as u64 + 1
        );
        assert_eq!(
            crate::cut_clipboard::local_write_calls_for_test(),
            local_writes + index + 1
        );
    }
}

#[test]
fn shell_paste_pending_preserves_grid_and_fullscreen_image_copy_dispatch() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let worker = install_worker(&mut app);
    let file = app.current_folder.as_ref().unwrap().join("image.jpg");
    app.items = vec![crate::grid_item::GridItem::Image(file)];
    app.selected = Some(0);
    app.fullscreen_idx = Some(0);
    enter(&mut app, &ctx, ShellPasteOrigin::CtrlV);
    worker.entered.recv_timeout(WAIT).unwrap();
    for action in [
        crate::keymap::KeyAction::GridCopyImage,
        crate::keymap::KeyAction::FsCopyImage,
    ] {
        let mut calls = 0;
        app.dispatch_copy_key_action_for_test(&ctx, action, |_, _, command| {
            calls += 1;
            assert_eq!(
                command,
                crate::context_menu_model::MenuCommand::CopyImageToClipboard
            );
        });
        assert_eq!(calls, 1, "{action:?}");
    }
    assert!(app.shell_paste_request.is_some());
    finish_paste(&mut app, &ctx, &worker, true);
}
