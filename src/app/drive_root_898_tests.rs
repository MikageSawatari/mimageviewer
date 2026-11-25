//! Headless handler tests. Shell calls are replaced; no product UI or file operation is started.
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
    for item in [
        GridItem::Folder(r"C:\foo".into()),
        GridItem::Folder(r"C:\foo\..\bar".into()),
        GridItem::Image(r"C:\foo.jpg".into()),
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
fn drive_root_898_ctrl_x_refuses_root_but_ctrl_c_and_ordinary_cut_invoke_shell() {
    // 旧コードは有効な HWND があるとルートの Cut を Shell 差し替え関数へ渡し、呼出回数ゼロ検査が落ちる。
    let mut app = drive_list();
    for (item, cut, expected) in [
        (GridItem::Folder(r"C:\".into()), true, 0),
        (GridItem::Folder(r"C:\".into()), false, 1),
        (GridItem::Folder(r"C:\foo".into()), true, 1),
        (GridItem::Folder(r"C:\foo\..\bar".into()), true, 1),
        (GridItem::Image(r"C:\foo.jpg".into()), true, 1),
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
            assert_notice(&app, FileOperationRefusal::VolumeRoot, "切り取り");
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
    for (path, is_file) in [
        (r"C:\foo", false),
        (r"C:\foo\..\bar", false),
        (r"C:\foo.jpg", true),
    ] {
        app.fs_feedback_toast = None;
        app.request_rename_dialog(path.into(), is_file);
        assert!(app.show_rename_dialog);
        assert_eq!(app.rename_target.as_deref(), Some(Path::new(path)));
        assert!(app.fs_feedback_toast.is_none());
        app.show_rename_dialog = false;
        app.rename_target = None;
    }
}

#[test]
fn drive_root_898_organize_refuses_move_keeps_selection_and_accepts_copy() {
    // 旧コードはルートの移動を転送差し替え関数へ渡し、呼出ゼロ／Selecting 維持の検査が落ちる。
    let mut app = drive_list();
    let destination = app.tmp.path().join("destination");
    app.settings.file_organize_destinations = vec![crate::settings::FileOrganizeDestination {
        name: "destination".into(),
        path: destination.clone(),
    }];
    let ctx = egui::Context::default();
    app.request_file_organize_dialog(None);
    assert!(app.file_organize_dialog_visible());
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
    assert_eq!(selection.sources, vec![PathBuf::from(r"C:\")]);
    assert_eq!(selection.destinations[0].path, destination);
    assert_notice(&app, FileOperationRefusal::VolumeRoot, "移動");
    app.fs_feedback_toast = None;
    app.submit_file_organize_with(
        &ctx,
        0,
        ShellTransferOperation::Copy,
        App::book_root_path,
        |hwnd, request, _| {
            assert_eq!(hwnd, Some(1));
            assert_eq!(request.sources, vec![PathBuf::from(r"C:\")]);
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
        GridItem::Folder(r"C:\foo".into()),
        GridItem::Folder(r"C:\foo\..\bar".into()),
        GridItem::Image(r"C:\foo.jpg".into()),
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
    for root_first in [false, true] {
        let mut items = vec![
            GridItem::Folder(r"C:\foo".into()),
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
fn drive_root_898_start_delete_files_refuses_before_worker_and_invalidation() {
    // 旧コードは直接渡したルートで worker を開始し、delete_pending.is_none() の検査が落ちる。
    let mut app = drive_list();
    let generation = app.items_generation;
    let targets = vec![DeleteTarget::from_grid_item(&app.items[0]).unwrap()];
    app.start_delete_files(&egui::Context::default(), targets);
    assert!(app.delete_pending.is_none());
    assert_eq!(app.items_generation, generation);
    assert_notice(&app, FileOperationRefusal::VolumeRoot, "削除");
}
