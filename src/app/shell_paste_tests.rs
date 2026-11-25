//! Handler regressions for the three mIV Shell paste entrances. Never invokes Shell.

use super::*;
use crate::app::{AppTestEnvForTest, NormalFolderOmittedEntries, setup_app_for_test};
use crate::post_operation_selection::ExpectedOutputs;
use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest};
use crate::shell_paste::{
    FocusApi, FocusSnapshot, ShellPasteError, ShellPasteResult, ShellPasteService,
};
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

type FocusTrace =
    std::sync::Arc<std::sync::Mutex<Vec<(&'static str, isize, std::thread::ThreadId)>>>;

struct HandlerFocus {
    foreign_process: bool,
    trace: FocusTrace,
}

impl FocusApi for HandlerFocus {
    fn snapshot(&mut self, main_hwnd: isize) -> FocusSnapshot {
        self.trace
            .lock()
            .unwrap()
            .push(("snapshot", main_hwnd, std::thread::current().id()));
        FocusSnapshot {
            main_valid: true,
            main_thread: 10,
            current_process: 100,
            foreground_hwnd: 2,
            foreground_thread: 11,
            foreground_process: if self.foreign_process { 101 } else { 100 },
        }
    }

    fn set_foreground(&mut self, main_hwnd: isize) -> bool {
        self.trace
            .lock()
            .unwrap()
            .push(("restore", main_hwnd, std::thread::current().id()));
        true
    }

    fn class_name(&self, _: isize) -> String {
        "<fake-shell-dialog>".into()
    }
}

fn assert_handler_focus_completion(foreign_process: bool, invocation: ShellPasteResult) {
    for origin in [
        ShellPasteOrigin::Tree,
        ShellPasteOrigin::CtrlV,
        ShellPasteOrigin::GridMenu,
    ] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let trace = FocusTrace::default();
        let invoked_trace = trace.clone();
        let result = invocation.clone();
        app.shell_paste_service = ShellPasteService::with_test_invoker_and_focus(
            move |hwnd, _| {
                invoked_trace
                    .lock()
                    .unwrap()
                    .push(("invoke", hwnd, std::thread::current().id()));
                result.clone()
            },
            HandlerFocus {
                foreign_process,
                trace: trace.clone(),
            },
        );
        SHELL_PASTE_END_FIELDS.with_borrow_mut(Vec::clear);
        enter(&mut app, &ctx, origin);
        let deadline = Instant::now() + WAIT;
        while app.shell_paste_request.is_some() {
            app.poll_shell_paste(&ctx);
            assert!(
                Instant::now() < deadline,
                "fake focus completion was not consumed"
            );
            std::thread::yield_now();
        }
        let start_failed = matches!(invocation, Err(ShellPasteError::Start(_)));
        let expected_calls: &[&str] = if start_failed {
            &["invoke"] // fake invoker simulates failing before Shell was called
        } else if foreign_process {
            &["invoke", "snapshot"]
        } else {
            &["invoke", "snapshot", "restore"]
        };
        let calls = trace.lock().unwrap().clone();
        assert_eq!(
            calls.iter().map(|call| call.0).collect::<Vec<_>>(),
            expected_calls
        );
        assert!(
            calls.iter().all(|call| call.1 == 1),
            "only main's HWND is used"
        );
        assert!(calls.iter().all(|call| call.2 == calls[0].2));
        assert_ne!(calls[0].2, std::thread::current().id());
        SHELL_PASTE_END_FIELDS.with_borrow(|events| {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["origin"], origin.perf_tag());
            assert_eq!(
                events[0]["outcome"],
                if start_failed {
                    "start_failed"
                } else if invocation.is_ok() {
                    "invoked"
                } else {
                    "failed"
                }
            );
            assert_eq!(
                events[0]["focus_return"],
                if start_failed {
                    "not_invoked"
                } else if foreign_process {
                    "other_process"
                } else {
                    "restored"
                }
            );
        });
        assert_eq!(app.post_operation_selection.is_some(), invocation.is_ok());
        app.poll_shell_paste(&ctx);
        assert_eq!(trace.lock().unwrap().len(), expected_calls.len());
        SHELL_PASTE_END_FIELDS.with_borrow(|events| assert_eq!(events.len(), 1));
    }
}

#[test]
fn shell_paste_handler_restores_main_once_after_invocation_and_records_end() {
    assert_handler_focus_completion(false, Ok(()));
}

#[test]
fn shell_paste_handler_preserves_foreign_foreground_and_records_end() {
    assert_handler_focus_completion(true, Ok(()));
}

#[test]
fn shell_paste_handler_failed_invocation_still_returns_focus_and_records_end() {
    assert_handler_focus_completion(
        false,
        Err(ShellPasteError::Invocation("fake failure".into())),
    );
}

#[test]
fn shell_paste_handler_start_failure_records_not_invoked_without_focus_query() {
    assert_handler_focus_completion(
        false,
        Err(ShellPasteError::Start("fake OLE failure".into())),
    );
}

#[test]
fn shell_paste_outcome_perf_tags_describe_invocation() {
    for (outcome, expected) in [
        (ShellPasteOutcome::Invoked, "invoked"),
        (ShellPasteOutcome::Failed, "failed"),
        (ShellPasteOutcome::StartFailed, "start_failed"),
        (ShellPasteOutcome::Disconnected, "disconnected"),
    ] {
        assert_eq!(outcome.perf_tag(), expected);
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

#[cfg(windows)]
mod fixed_ctrl_v_tests {
    use super::*;
    use crate::key_input::{KeyEdge, KeyOperationId};

    fn edge(vk: u32, pressed: bool, ctrl: bool) -> KeyEdge {
        KeyEdge {
            operation_id: KeyOperationId::unassigned(),
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: vk,
            scan_code: if vk == 0x56 { 0x2F } else { 0x1D },
            extended: false,
            pressed,
            repeat: false,
            ctrl,
            shift: false,
            alt: false,
        }
    }

    fn tap(ctrl: bool) -> Vec<KeyEdge> {
        let mut edges = Vec::new();
        if ctrl {
            edges.push(edge(0x11, true, true));
        }
        edges.push(edge(0x56, true, ctrl));
        edges.push(edge(0x56, false, ctrl));
        if ctrl {
            edges.push(edge(0x11, false, false));
        }
        edges
    }

    fn begin(ctx: &egui::Context, edges: Vec<KeyEdge>, focused: bool) {
        // Native ingress stamps pending edges before the next App frame drains them.
        for edge in edges {
            crate::key_input::enqueue_test_pending_edge(edge);
        }
        crate::key_input::begin_frame();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(focused);
        ctx.begin_pass(input);
    }

    fn dispatch_files(app: &mut App, ctx: &egui::Context) {
        // Run the complete clipboard shortcut handler; replace only OS clipboard I/O.
        app.handle_clipboard_shortcuts_with(ctx, || {
            Ok(crate::clipboard_capture::data::ClipboardKind::Files)
        });
    }

    fn assert_not_dispatched(app: &mut App, ctx: &egui::Context) {
        app.handle_clipboard_shortcuts_with(ctx, || {
            panic!("blocked Ctrl+V must not query the clipboard")
        });
        assert!(app.shell_paste_request.is_none());
        assert!(app.fs_feedback_toast.is_none());
    }

    #[test]
    fn shell_paste_ctrl_v_released_before_frame_submits_once_and_preserves_edge() {
        let mut app = ready(); // AppTestEnv owns lock_test_input until App is dropped.
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        begin(&ctx, tap(true), true);
        assert!(!ctx.input(|input| input.key_down(egui::Key::V)));
        assert!(!crate::key_input::physical_key_down(
            crate::key_input::PhysicalKeySlot::new(0x56, false)
        ));
        dispatch_files(&mut app, &ctx);
        worker.entered.recv_timeout(WAIT).unwrap();
        assert_eq!(
            app.shell_paste_request.as_ref().unwrap().origin,
            ShellPasteOrigin::CtrlV
        );
        assert!(crate::key_input::pressed_key_down(
            egui::ViewportId::ROOT,
            |edge| { edge.virtual_key == 0x56 && edge.ctrl && !edge.repeat }
        ));
        finish_paste(&mut app, &ctx, &worker, true);
        ctx.end_pass().drop_without_applying_deltas();

        // Unconsumed edges expire at the normal next-frame boundary.
        begin(&ctx, Vec::new(), true);
        assert_not_dispatched(&mut app, &ctx);
        assert!(worker.entered.try_recv().is_err());
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn shell_paste_ctrl_v_layout_replay_after_completion_dispatches_once() {
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        for edge in tap(true) {
            crate::key_input::enqueue_test_pending_edge(edge);
        }
        let mut passes = 0;
        let mut queries = 0;
        ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            crate::key_input::begin_frame_for_context_with_tracking(ctx, false);
            assert!(!app.shortcuts_blocked_by_text_input(ctx));
            assert!(app.grid_item_input_allowed());
            assert!(app.shell_paste_request.is_none());
            app.handle_clipboard_shortcuts_with(ctx, || {
                queries += 1;
                Ok(crate::clipboard_capture::data::ClipboardKind::Files)
            });
            if ctx.current_pass_index() == 0 {
                worker.entered.recv_timeout(WAIT).unwrap();
                assert_eq!(
                    app.shell_paste_request.as_ref().unwrap().origin,
                    ShellPasteOrigin::CtrlV
                );
                // Finish and poll the initial request before the actual egui
                // replay. Busy admission must not be what prevents re-dispatch.
                finish_paste(&mut app, ctx, &worker, true);
                assert!(app.shell_paste_request.is_none());
                ctx.request_discard("clipboard paste replay after completion");
            } else {
                assert!(app.shell_paste_request.is_none());
                assert!(worker.entered.try_recv().is_err());
                assert!(app.fs_feedback_toast.is_none());
            }
            assert!(crate::key_input::pressed_key_down(
                egui::ViewportId::ROOT,
                |edge| { edge.virtual_key == 0x56 && edge.ctrl && !edge.repeat }
            ));
            passes += 1;
        })
        .drop_without_applying_deltas();
        assert_eq!(passes, 2);
        assert_eq!(queries, 1);

        // A different press in a later backend frame is still eligible.
        for edge in tap(true) {
            crate::key_input::enqueue_test_pending_edge(edge);
        }
        ctx.run_ui(Default::default(), |ui| {
            crate::key_input::begin_frame_for_context_with_tracking(ui.ctx(), false);
            dispatch_files(&mut app, ui.ctx());
            worker.entered.recv_timeout(WAIT).unwrap();
            finish_paste(&mut app, ui.ctx(), &worker, true);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn shell_paste_ctrl_v_keeps_shift_alt_and_routed_ownership_after_focus_loss() {
        for (shift, alt) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut app = ready();
            let ctx = egui::Context::default();
            let worker = install_worker(&mut app);
            let mut edges = tap(true);
            for edge in &mut edges {
                edge.shift = shift;
                edge.alt = alt;
            }
            begin(&ctx, edges, false);
            dispatch_files(&mut app, &ctx);
            worker.entered.recv_timeout(WAIT).unwrap();
            assert_eq!(
                app.shell_paste_request.as_ref().unwrap().origin,
                ShellPasteOrigin::CtrlV
            );
            finish_paste(&mut app, &ctx, &worker, true);
            ctx.end_pass().drop_without_applying_deltas();
        }
    }

    #[test]
    fn shell_paste_ctrl_v_rejects_plain_v_repeat_and_key_up() {
        for edges in [
            tap(false),
            vec![KeyEdge {
                repeat: true,
                ..edge(0x56, true, true)
            }],
            vec![edge(0x56, false, true)],
        ] {
            let mut app = ready();
            let ctx = egui::Context::default();
            begin(&ctx, edges, true);
            assert_not_dispatched(&mut app, &ctx);
            ctx.end_pass().drop_without_applying_deltas();
        }
    }

    #[test]
    fn shell_paste_ctrl_v_text_modal_ime_and_focused_ui_block_routed_edges() {
        use crate::keyboard_input::{
            KeyboardOwner, ShortcutScope, ShortcutSurface, TextInputPhase,
        };
        let viewport = egui::ViewportId::ROOT;
        let widget_id = egui::Id::unique("ctrl_v_blocking_owner");
        for focused in [true, false] {
            for owner in [
                KeyboardOwner::Modal,
                KeyboardOwner::TextInput {
                    viewport,
                    widget_id,
                    phase: TextInputPhase::Focused,
                },
                KeyboardOwner::TextInput {
                    viewport,
                    widget_id,
                    phase: TextInputPhase::FocusRecovery,
                },
                KeyboardOwner::TextInput {
                    viewport,
                    widget_id,
                    phase: TextInputPhase::ImeComposing,
                },
                KeyboardOwner::FocusedUi {
                    viewport,
                    widget_id,
                },
                KeyboardOwner::ApplicationShortcut {
                    scope: ShortcutScope::new(viewport, ShortcutSurface::Fullscreen),
                },
            ] {
                let mut app = ready();
                let ctx = egui::Context::default();
                begin(&ctx, tap(true), focused);
                crate::keyboard_input::cache_keyboard_owner(&ctx, owner);
                assert_not_dispatched(&mut app, &ctx);
                assert!(
                    crate::key_input::pressed_key_down(viewport, |edge| {
                        edge.virtual_key == 0x56 && edge.ctrl
                    }),
                    "blocking must not consume the edge"
                );
                ctx.end_pass().drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn shell_paste_ctrl_v_ignores_foreign_edges_and_unfocused_egui_only_input() {
        let mut app = ready();
        let ctx = egui::Context::default();
        let mut edges = tap(true);
        for edge in &mut edges {
            edge.source_viewport = egui::ViewportId::from_hash_of("foreign_clipboard_source");
        }
        begin(&ctx, edges, true);
        assert_not_dispatched(&mut app, &ctx);
        ctx.end_pass().drop_without_applying_deltas();

        begin(&ctx, Vec::new(), false);
        ctx.input_mut(|input| {
            input.events.push(egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            });
        });
        assert_not_dispatched(&mut app, &ctx);
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn shell_paste_ctrl_v_synthetic_tap_submits_after_timeline_release() {
        use crate::key_input::{SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey};
        let mut app = ready();
        let ctx = egui::Context::default();
        let worker = install_worker(&mut app);
        crate::key_input::arm_test_synthetic_input(1, egui::ViewportId::ROOT);
        let at = Instant::now() - Duration::from_secs(1);
        crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::down_for_target(
            at,
            SyntheticNavigationKey::V,
            SyntheticModifiers {
                ctrl: true,
                ..Default::default()
            },
            crate::key_input::SyntheticRoutingTarget {
                hwnd: 1,
                viewport: egui::ViewportId::ROOT,
            },
        ));
        crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::up(
            at,
            SyntheticNavigationKey::V,
        ));
        crate::key_input::advance_test_synthetic_input(Instant::now());
        begin(&ctx, Vec::new(), true);
        for vk in [0x11, 0x56] {
            assert!(!crate::key_input::physical_key_down(
                crate::key_input::PhysicalKeySlot::new(vk, false)
            ));
        }
        dispatch_files(&mut app, &ctx);
        worker.entered.recv_timeout(WAIT).unwrap();
        assert_eq!(
            app.shell_paste_request.as_ref().unwrap().origin,
            ShellPasteOrigin::CtrlV
        );
        finish_paste(&mut app, &ctx, &worker, true);
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn shell_paste_ctrl_v_keeps_copy_cut_precedence_and_semantic_focus_gate() {
        use crate::native_context_menu::ShellClipboardVerb;
        for (keys, verb) in [
            (vec![0x43], ShellClipboardVerb::Copy),
            (vec![0x58], ShellClipboardVerb::Cut),
            (vec![0x43, 0x58], ShellClipboardVerb::Cut),
        ] {
            let app = ready();
            let ctx = egui::Context::default();
            let mut edges = tap(true);
            for vk in keys {
                edges.extend([edge(vk, true, true), edge(vk, false, true)]);
            }
            begin(&ctx, edges, false);
            assert_eq!(app.grid_clipboard_requests(&ctx), Some((Some(verb), true)));
            ctx.end_pass().drop_without_applying_deltas();
        }

        let app = ready();
        let ctx = egui::Context::default();
        // An unrelated stamped ROOT key must not give generic clipboard events
        // ownership in an unfocused pass.
        begin(&ctx, vec![edge(0x41, true, false)], false);
        ctx.input_mut(|input| input.events.extend([egui::Event::Copy, egui::Event::Cut]));
        assert_eq!(app.grid_clipboard_requests(&ctx), Some((None, false)));
        ctx.end_pass().drop_without_applying_deltas();
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
    SHELL_PASTE_END_FIELDS.with_borrow_mut(Vec::clear);
    super::take_modifier_resync_calls_for_test();
    app.poll_shell_paste(&ctx);
    assert!(app.shell_paste_request.is_none());
    assert!(app.post_operation_selection.is_none());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "貼り付けを完了できませんでした"
    );
    assert_eq!(super::take_modifier_resync_calls_for_test(), 1);
    SHELL_PASTE_END_FIELDS.with_borrow(|events| {
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["outcome"], "disconnected");
        assert_eq!(events[0]["focus_return"], "not_invoked");
    });
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
