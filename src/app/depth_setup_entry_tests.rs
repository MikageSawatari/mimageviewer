//! Lazy installed-model discovery through the real mode-entry handler.
//! Uses a tiny local addon pack, without a network request or model inference.
#![cfg(windows)]

use super::depth_view_tests::{depth_key_frame, install_pages};
use super::*;
use crate::addon::{AddonKind, AddonStatus};
use crate::fs_exclusive_view::FsExclusiveView;
use crate::ui_dialogs::depth_addon::DepthSetup;
use std::time::{Duration, Instant};

fn installed_pack(app: &mut AppTestEnvForTest) -> PathBuf {
    let root = app.tmp.path().join("depth-setup-entry-addon");
    let (index, bodies) = crate::depth_addon::tests::fixture();
    let pack = root.join("packs").join(&index.model_id);
    std::fs::create_dir_all(&pack).unwrap();
    for (file, body) in index.files.iter().zip(bodies) {
        std::fs::write(pack.join(&file.name), body).unwrap();
    }
    std::fs::write(
        pack.join(AddonKind::Depth.index_name()),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    std::fs::write(pack.join("INSTALL_OK"), b"1").unwrap();
    std::fs::write(
        root.join("active.json"),
        serde_json::to_vec(&serde_json::json!({"schema": 1, "model_id": index.model_id})).unwrap(),
    )
    .unwrap();
    app.depth_setup = DepthSetup::for_test(root);
    pack.join(crate::depth_addon::MODEL_FILENAME)
}

fn press_d(app: &mut App, ctx: &egui::Context) {
    depth_key_frame(app, ctx, &[(egui::Key::D, true), (egui::Key::D, false)]);
    assert!(app.depth_setup.checking_for_test());
    assert!(!app.fs_exclusive_view.is_depth());
}

fn finish_scan(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.depth_setup.checking_for_test() {
        app.poll_depth_setup(ctx);
        assert!(Instant::now() < deadline, "depth addon scan did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn depth_setup_first_d_enters_after_installed_scan_without_completion_dialog() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    let model = installed_pack(&mut app);
    press_d(&mut app, &ctx);
    assert!(app.depth_setup.modal_open());
    finish_scan(&mut app, &ctx);
    assert_eq!(
        app.depth_setup.installed_model_path(),
        Some(model.as_path())
    );
    assert!(app.fs_exclusive_view.is_depth());
    assert!(!app.depth_setup.dialog_open_for_test());
    assert!(app.fs_feedback_toast.is_none());
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_first_d_missing_or_corrupt_scan_keeps_download_confirmation() {
    let _input_guard = crate::key_input::lock_test_input();
    for corrupt in [false, true] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        let root = app.tmp.path().join("depth-missing-or-corrupt");
        if corrupt {
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("active.json"), b"invalid").unwrap();
        }
        app.depth_setup = DepthSetup::for_test(root);
        press_d(&mut app, &ctx);
        finish_scan(&mut app, &ctx);
        assert!(app.depth_setup.confirming_for_test(), "corrupt={corrupt}");
        assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
        assert!(app.depth_setup.installed_model_path().is_none());
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_preferences_installed_scan_never_enters_mode() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    installed_pack(&mut app);
    app.request_depth_addon_setup(&ctx);
    assert!(app.depth_setup.checking_for_test());
    finish_scan(&mut app, &ctx);
    assert!(app.depth_setup.installed_model_path().is_some());
    assert!(matches!(app.fs_exclusive_view, FsExclusiveView::None));
    assert!(app.depth_setup.dialog_open_for_test());
}

#[test]
fn depth_setup_installed_scan_cannot_enter_a_changed_image_or_list() {
    let _input_guard = crate::key_input::lock_test_input();
    for change in ["page", "same-slot-new-image", "list-generation", "closed"] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        installed_pack(&mut app);
        press_d(&mut app, &ctx);
        match change {
            "page" => app.fullscreen_idx = Some(3),
            "same-slot-new-image" => app.items[0] = GridItem::Image("changed.jpg".into()),
            "list-generation" => app.bump_items_generation(),
            "closed" => app.fullscreen_idx = None,
            _ => unreachable!(),
        }
        finish_scan(&mut app, &ctx);
        assert!(
            matches!(app.fs_exclusive_view, FsExclusiveView::None),
            "{change}"
        );
        assert!(!app.depth_setup.dialog_open_for_test(), "{change}");
        assert!(app.depth_setup.installed_model_path().is_some(), "{change}");
        app.fullscreen_idx = Some(0);
        app.poll_depth_setup(&ctx);
        assert!(
            !app.fs_exclusive_view.is_depth(),
            "{change}: request consumed"
        );
    }
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_installed_scan_cannot_enter_a_different_viewer_context() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    installed_pack(&mut app);
    press_d(&mut app, &ctx);
    let sibling = app.build_window_context_for_test(80_451, |owner| {
        install_pages(owner, &ctx);
    });
    app.with_viewer_context(sibling, |owner| {
        finish_scan(owner, &ctx);
        assert!(!owner.fs_exclusive_view.is_depth());
        assert!(!owner.depth_setup.dialog_open_for_test());
    })
    .unwrap();
    app.poll_depth_setup(&ctx);
    assert!(!app.fs_exclusive_view.is_depth());
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_installed_scan_revalidates_entry_and_only_notifies_on_rejection() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    installed_pack(&mut app);
    press_d(&mut app, &ctx);
    app.reading_flow = crate::settings::ReadingFlow::Vertical;
    finish_scan(&mut app, &ctx);
    assert!(!app.fs_exclusive_view.is_depth());
    assert!(!app.depth_setup.dialog_open_for_test());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        crate::depth_parallax::DepthEntryBlock::ContinuousReading.message()
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_installed_scan_rejects_remote_acquisition_before_local_image_closes() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    installed_pack(&mut app);
    press_d(&mut app, &ctx);
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    let acquired = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "depth-check-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    assert_eq!(acquired.status, mimageviewer_ipc::SessionStatus::Active);
    assert!(app.remote_session_blocks_local_control());
    assert_eq!(app.fullscreen_idx, Some(0));
    // No UI poll/finish_acquire: the original local image still exists during takeover.
    finish_scan(&mut app, &ctx);
    assert!(!app.fs_exclusive_view.is_depth());
    assert!(!app.depth_setup.dialog_open_for_test());
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "リモート閲覧中のため、3D 写真に切り替えられません"
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_confirmed_missing_does_not_resume_after_model_becomes_installed() {
    let _input_guard = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    app.depth_setup = DepthSetup::for_test(app.tmp.path().join("missing-depth"));
    press_d(&mut app, &ctx);
    finish_scan(&mut app, &ctx);
    assert!(app.depth_setup.confirming_for_test());
    app.depth_setup.set_status_for_test(AddonStatus::Valid {
        model_id: "completed-install".into(),
        bytes: 1,
        model_path: "test-completed-model.onnx".into(),
        mmproj_path: None,
    });
    app.poll_depth_setup(&ctx);
    assert!(!app.fs_exclusive_view.is_depth());
    crate::key_input::clear_test_frame();
}
