//! 破壊的な操作のテストでは、本物の Shell を実パスに呼ばせない。
//! 実ドライブのルートは判定・実行しない入口だけに渡し、実行経路は一時フォルダの偽パスと差し替えで確かめる。
use super::*;
use crate::grid_item::FileOperationRefusal;
use crate::native_context_menu::ShellClipboardVerb;
use crate::shell_file_ops::ShellTransferOperation;
use crate::ui_dialogs::context_menu::DeleteTarget;
use crate::ui_dialogs::file_organize::FileOrganizeRequest;
use std::sync::mpsc;

fn drive_list() -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.indexer_init.mark_unavailable_if_not_ready();
    // Same nonzero test HWND as main_drop/shell_paste tests; all Shell calls use a seam.
    app.main_hwnd = Some(1);
    app.items_are_drive_list = true;
    app.items = vec![GridItem::Folder(r"C:\".into())];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    let books = app.tmp.path().join("books");
    app.settings.book_root = Some(books);
    app
}

fn assert_notice(app: &App, reason: FileOperationRefusal, action: &str) {
    assert_eq!(
        app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
        Some(reason.message(action).as_str())
    );
}

fn begin_grid_key_pass(ctx: &egui::Context, vk: u32, scan_code: u16, ctrl: bool) {
    // Match shell_paste_tests: stamped native ingress, with AppTestEnv owning TestInputGuard.
    for pressed in [true, false] {
        crate::key_input::enqueue_test_pending_edge(crate::key_input::KeyEdge {
            operation_id: crate::key_input::KeyOperationId::unassigned(),
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: vk,
            scan_code,
            extended: vk == 0x2E,
            pressed,
            repeat: false,
            ctrl,
            shift: false,
            alt: false,
        });
    }
    crate::key_input::begin_frame();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(true);
    ctx.begin_pass(input);
}

fn delete_key(app: &mut App, ctx: &egui::Context) {
    begin_grid_key_pass(ctx, 0x2E, 0x53, false);
    let _owner = app.keyboard_owner_for_pass(ctx);
    app.handle_delete_key(ctx);
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn drive_root_898_delete_key_refuses_root_and_preserves_ordinary_items() {
    // 旧コードは C:\ の Delete で削除確認を開くため、最初の false 検査が落ちる。
    let mut app = drive_list();
    let ctx = egui::Context::default();
    delete_key(&mut app, &ctx);
    assert!(!app.show_delete_confirm);
    assert!(app.delete_pending.is_none());
    assert_notice(&app, FileOperationRefusal::VolumeRoot, "削除");
    app.items_are_drive_list = false;
    let root = app.tmp.path().to_owned();
    for item in [
        GridItem::Folder(root.join("foo")),
        GridItem::Folder(root.join(r"foo\..\bar")),
        GridItem::Image(root.join("foo.jpg")),
    ] {
        app.items = vec![item];
        app.fs_feedback_toast = None;
        delete_key(&mut app, &ctx);
        assert!(app.show_delete_confirm);
        assert!(app.fs_feedback_toast.is_none());
        app.show_delete_confirm = false;
        app.delete_targets.clear();
    }
}

#[test]
fn drive_root_898_ctrl_x_root_admission_and_temporary_clipboard_execution() {
    // 拒否無効の変異ではルートの純粋受付と、合成パスの呼出ゼロ／拒否理由の検査が落ちる。
    let mut app = drive_list();
    for cut in [true, false] {
        let ctx = egui::Context::default();
        begin_grid_key_pass(
            &ctx,
            if cut { 0x58 } else { 0x43 },
            if cut { 0x2D } else { 0x2E },
            true,
        );
        let _owner = app.keyboard_owner_for_pass(&ctx);
        let (verb, _) = app.grid_clipboard_requests(&ctx).unwrap();
        let verb = verb.unwrap();
        assert_eq!(
            verb,
            if cut {
                ShellClipboardVerb::Cut
            } else {
                ShellClipboardVerb::Copy
            }
        );
        let paths = app.collect_shell_clipboard_paths().unwrap();
        // Roots never enter invoke_shell_clipboard_verb_for_paths_with, including Copy.
        assert_eq!(
            App::shell_clipboard_paths_refusal(&paths, verb),
            cut.then_some(FileOperationRefusal::VolumeRoot)
        );
        ctx.end_pass().drop_without_applying_deltas();
    }
    let root = app.tmp.path().to_owned();
    app.items_are_drive_list = false;
    for (item, cut, expected) in [
        (GridItem::Folder(root.join("__drive_list__")), true, 0),
        (GridItem::Folder(root.join("__drive_list__")), false, 1),
        (GridItem::Folder(root.join("foo")), true, 1),
        (GridItem::Folder(root.join(r"foo\..\bar")), true, 1),
        (GridItem::Image(root.join("foo.jpg")), true, 1),
    ] {
        app.items = vec![item];
        app.fs_feedback_toast = None;
        let ctx = egui::Context::default();
        begin_grid_key_pass(
            &ctx,
            if cut { 0x58 } else { 0x43 },
            if cut { 0x2D } else { 0x2E },
            true,
        );
        let _owner = app.keyboard_owner_for_pass(&ctx);
        let (verb, _) = app.grid_clipboard_requests(&ctx).unwrap();
        let verb = verb.unwrap();
        assert_eq!(
            verb,
            if cut {
                ShellClipboardVerb::Cut
            } else {
                ShellClipboardVerb::Copy
            }
        );
        let paths = app.collect_shell_clipboard_paths().unwrap();
        let mut calls = 0;
        app.invoke_shell_clipboard_verb_for_paths_with(
            &ctx,
            &paths,
            verb,
            |hwnd, actual, actual_verb, _| {
                assert_eq!(hwnd, 1);
                assert_eq!(actual, paths);
                assert_eq!(actual_verb, verb);
                calls += 1;
                Ok(7)
            },
        );
        assert_eq!(calls, expected);
        if expected == 0 {
            assert_notice(&app, FileOperationRefusal::VirtualLocation, "切り取り");
        } else {
            assert!(app.fs_feedback_toast.is_none());
        }
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
fn drive_root_898_rename_refuses_root_and_preserves_ordinary_dialogs() {
    // 旧コードはルートで汎用の「この項目は名前を変更できません」を出し、VolumeRoot の通知検査が落ちる。
    let mut app = drive_list();
    app.request_rename_dialog(r"C:\".into(), false);
    assert!(!app.show_rename_dialog);
    assert!(app.rename_target.is_none());
    assert_notice(&app, FileOperationRefusal::VolumeRoot, "名前の変更");
    let root = app.tmp.path().to_owned();
    for (path, is_file) in [
        (root.join("foo"), false),
        (root.join(r"foo\..\bar"), false),
        (root.join("foo.jpg"), true),
    ] {
        app.fs_feedback_toast = None;
        app.request_rename_dialog(path.clone(), is_file);
        assert!(app.show_rename_dialog);
        assert_eq!(app.rename_target.as_deref(), Some(path.as_path()));
        assert!(app.fs_feedback_toast.is_none());
        app.show_rename_dialog = false;
        app.rename_target = None;
    }
}

#[test]
fn drive_root_898_organize_root_admission_and_temporary_move_copy_execution() {
    // 拒否無効の変異ではルートの純粋受付と、合成パスの呼出ゼロ／Selecting 維持の検査が落ちる。
    let mut app = drive_list();
    let root = app.tmp.path().to_owned();
    let destination = app.tmp.path().join("destination");
    app.settings.file_organize_destinations = vec![crate::settings::FileOrganizeDestination {
        name: "destination".into(),
        path: destination.clone(),
    }];
    let ctx = egui::Context::default();
    app.request_file_organize_dialog(None);
    assert!(app.file_organize_dialog_visible());
    // The drive selection is inspected only; never submit it to the execution method.
    assert_eq!(
        app.file_organize_request
            .structural_submission_refusal(ShellTransferOperation::Move),
        Some(FileOperationRefusal::VolumeRoot)
    );
    assert_eq!(
        app.file_organize_request
            .structural_submission_refusal(ShellTransferOperation::Copy),
        None
    );
    app.file_organize_request = FileOrganizeRequest::Hidden;
    let synthetic = root.join("__drive_list__");
    app.items_are_drive_list = false;
    app.items = vec![GridItem::Folder(synthetic.clone())];
    app.request_file_organize_dialog(None);
    let mut calls = 0;
    app.submit_file_organize_with(
        &ctx,
        0,
        ShellTransferOperation::Move,
        App::book_root_path,
        |_, _, _| {
            calls += 1;
            mpsc::channel().1
        },
    );
    assert_eq!(calls, 0);
    let FileOrganizeRequest::Selecting(selection) = &app.file_organize_request else {
        panic!("selection must remain")
    };
    assert_eq!(selection.sources, vec![synthetic.clone()]);
    assert_eq!(selection.destinations[0].path, destination);
    assert_notice(&app, FileOperationRefusal::VirtualLocation, "移動");
    app.fs_feedback_toast = None;
    app.submit_file_organize_with(
        &ctx,
        0,
        ShellTransferOperation::Copy,
        App::book_root_path,
        |hwnd, request, _| {
            assert_eq!(hwnd, Some(1));
            assert_eq!(request.sources, vec![synthetic]);
            assert_eq!(request.operation, ShellTransferOperation::Copy);
            calls += 1;
            mpsc::channel().1
        },
    );
    assert_eq!(calls, 1);
    assert!(matches!(
        app.file_organize_request,
        FileOrganizeRequest::Running(_)
    ));
    assert!(app.fs_feedback_toast.is_none());
    for item in [
        GridItem::Folder(root.join("foo")),
        GridItem::Folder(root.join(r"foo\..\bar")),
        GridItem::Image(root.join("foo.jpg")),
    ] {
        app.file_organize_request = FileOrganizeRequest::Hidden;
        app.items = vec![item.clone()];
        app.request_file_organize_dialog(None);
        let mut moved = false;
        app.submit_file_organize_with(
            &ctx,
            0,
            ShellTransferOperation::Move,
            App::book_root_path,
            |_, request, _| {
                moved = true;
                assert_eq!(
                    request.sources,
                    vec![item.drag_source_path().unwrap().to_owned()]
                );
                mpsc::channel().1
            },
        );
        assert!(moved);
        assert!(app.fs_feedback_toast.is_none());
    }
}

#[test]
fn drive_root_898_delete_synthetic_folder_refuses_virtual_location() {
    // 旧コードは合成パスを実 Folder として Delete の確認を開き、確認なしの検査が落ちる。
    let mut app = drive_list();
    app.items_are_drive_list = false;
    app.items = vec![GridItem::Folder(drive_list_synthetic_path())];
    delete_key(&mut app, &egui::Context::default());
    assert!(!app.show_delete_confirm);
    assert!(app.delete_pending.is_none());
    assert_notice(&app, FileOperationRefusal::VirtualLocation, "削除");
}

#[test]
fn drive_root_898_mixed_delete_request_refuses_every_target() {
    // 旧コードはルートと通常フォルダの全体を削除確認へ登録し、確認なし／対象未登録の検査が落ちる。
    let mut app = drive_list();
    let folder = app.tmp.path().join("foo");
    for root_first in [false, true] {
        let mut items = vec![
            GridItem::Folder(folder.clone()),
            GridItem::Folder(r"C:\".into()),
        ];
        if root_first {
            items.reverse();
        }
        app.request_delete_confirm(
            items
                .iter()
                .filter_map(DeleteTarget::from_grid_item)
                .collect(),
        );
        assert!(!app.show_delete_confirm);
        assert!(app.delete_targets.is_empty());
        assert_notice(&app, FileOperationRefusal::VolumeRoot, "削除");
    }
}

#[test]
fn drive_root_898_delete_virtual_guard_before_worker_and_invalidation() {
    // 拒否が壊れた旧コード相当では偽 worker の呼出が 1 回になり、ゼロの検査が落ちる。
    let mut app = drive_list();
    let root = app.tmp.path().to_owned();
    app.items_are_drive_list = false;
    app.items = vec![GridItem::Folder(root.join("__drive_list__"))];
    let generation = app.items_generation;
    let targets = vec![DeleteTarget::from_grid_item(&app.items[0]).unwrap()];
    let mut calls = 0;
    app.start_delete_files_with(&egui::Context::default(), targets, |paths, scopes, _, _| {
        calls += 1;
        fake_delete_pending(paths, scopes)
    });
    assert_eq!(calls, 0);
    assert!(app.delete_pending.is_none());
    assert_eq!(app.items_generation, generation);
    assert_notice(&app, FileOperationRefusal::VirtualLocation, "削除");
    for item in [
        GridItem::Folder(root.join("foo")),
        GridItem::Folder(root.join(r"foo\..\bar")),
        GridItem::Image(root.join("foo.jpg")),
    ] {
        let targets = vec![DeleteTarget::from_grid_item(&item).unwrap()];
        app.delete_pending = None;
        app.fs_feedback_toast = None;
        let mut calls = 0;
        app.start_delete_files_with(&egui::Context::default(), targets, |paths, scopes, _, _| {
            calls += 1;
            assert_eq!(paths, vec![item.drag_source_path().unwrap().to_owned()]);
            fake_delete_pending(paths, scopes)
        });
        assert_eq!(calls, 1);
        assert!(app.fs_feedback_toast.is_none());
    }
}

fn fake_delete_pending(
    paths: Vec<PathBuf>,
    source_scopes: Vec<crate::delete_worker::DeleteSourceScope>,
) -> crate::delete_worker::DeletePending {
    crate::delete_worker::DeletePending {
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rx: mpsc::channel().1,
        total: paths.len(),
        succeeded: Vec::new(),
        failed: Vec::new(),
        purged_pdf_password_paths: Vec::new(),
        purge_deferred: false,
        source_scopes,
    }
}
