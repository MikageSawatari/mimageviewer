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
fn depth_main_live_pass_handoff_invalidates_checking_and_confirm_entry_but_keeps_preferences() {
    let _input = crate::key_input::lock_test_input();
    for embedded in [false, true] {
        for origin in ["checking", "confirm", "preferences"] {
            let ctx = egui::Context::default();
            let mut app = setup_app_for_test();
            install_pages(&mut app, &ctx);
            app.viewer_presentation = if embedded {
                ViewerPresentation::MainWindow
            } else {
                ViewerPresentation::Fullscreen
            };
            app.native_video_in_window_active = embedded;
            let model = installed_pack(&mut app);
            let child = app.push_window_context_for_test(&ctx, 80_480, |owner| {
                install_pages(owner, &ctx);
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
                owner.native_video_in_window_active = false;
            });
            if origin == "preferences" {
                app.request_depth_addon_setup(&ctx);
            } else if origin == "confirm" {
                app.depth_setup.set_status_for_test(AddonStatus::Missing);
                app.toggle_depth_parallax(&ctx, 0);
                assert!(app.depth_setup.confirming_for_test());
            } else {
                press_d(&mut app, &ctx);
            }
            let keep = embedded || origin == "preferences";
            // Merely mounting a sibling must not consume main's entry intent.
            app.with_viewer_context(child, |_| {}).unwrap();
            assert!(app.depth_setup.modal_open());
            assert!(app.activate_detached_image_window_snapshot(&ctx, 80_480));
            assert_eq!(
                app.depth_setup.modal_open(),
                keep,
                "{origin} embedded={embedded}"
            );
            if origin != "confirm" {
                let deadline = Instant::now() + Duration::from_secs(5);
                // Observe the real scan completion even when the entry dialog
                // has expired. This proves the addon scan itself was retained.
                while app.depth_setup.installed_model_path().is_none() {
                    app.poll_depth_setup(&ctx);
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                app.poll_depth_setup(&ctx);
                assert_eq!(
                    app.depth_setup.installed_model_path(),
                    Some(model.as_path())
                );
                assert_eq!(
                    app.fs_exclusive_view.is_depth(),
                    embedded && origin == "checking"
                );
                assert_eq!(
                    app.depth_setup.dialog_open_for_test(),
                    origin == "preferences"
                );
            } else {
                assert_eq!(app.depth_setup.confirming_for_test(), embedded);
                assert!(!app.fs_exclusive_view.is_depth());
            }
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(app.active_viewer_context_id(), Some(child));
            crate::key_input::clear_test_frame();
        }
    }
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

fn independent_scan_case(change: &str) {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    install_pages(&mut app, &ctx);
    installed_pack(&mut app);
    let main_generation = app.items_generation;
    let context = app.build_active_context_for_test(Some(80_460), DetachedSource::Image, |owner| {
        install_pages(owner, &ctx);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
    });
    app.with_viewer_context(context, |owner| {
        owner.mark_active_detached_viewport_rendered(&ctx);
        assert!(owner.viewer_context_live(context));
        press_d(owner, &ctx);
        assert_eq!(owner.depth_setup.dialog_owner_context(), Some(context));
        if change == "page" {
            owner.fullscreen_idx = Some(3);
        }
    })
    .unwrap();
    if change == "inactive" {
        app.build_active_context_for_test(Some(80_461), DetachedSource::Image, |owner| {
            install_pages(owner, &ctx);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.detached_viewer_independent_active = true;
        });
    }
    // This is ROOT's poll, with main mounted rather than the requesting context.
    finish_scan(&mut app, &ctx);
    assert!(!app.fs_exclusive_view.is_depth());
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    app.with_viewer_context(context, |owner| {
        assert_eq!(owner.fs_exclusive_view.is_depth(), change == "unchanged");
        if let FsExclusiveView::Depth(state) = &owner.fs_exclusive_view {
            assert_eq!(state.context, context);
        }
    })
    .unwrap();
    assert!(!app.depth_setup.dialog_open_for_test());
    app.poll_depth_setup(&ctx);
    assert!(
        !app.fs_exclusive_view.is_depth(),
        "completion cannot enter ROOT later"
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn depth_setup_independent_installed_scan_resumes_only_the_requesting_context_from_root() {
    independent_scan_case("unchanged");
}
#[test]
fn depth_setup_independent_installed_scan_discards_changed_page() {
    independent_scan_case("page");
}
#[test]
fn depth_setup_independent_installed_scan_discards_inactive_window() {
    independent_scan_case("inactive");
}

/// Exercise the actual shared admission owners, including an expired timer at
/// the last image where NextFolder would otherwise start a traversal.
pub(crate) fn assert_independent_modal_protection(
    app: &mut App,
    ctx: &egui::Context,
    owner: ViewerContextId,
    window: u64,
    sibling_window: u64,
) {
    let main_generation = app.items_generation;
    let main_index = app.fullscreen_idx;
    assert!(app.document_open_modal_admission_blocked());
    assert!(!app.activate_detached_image_window_snapshot(ctx, sibling_window));
    assert_eq!(app.active_viewer_context_id(), Some(owner));
    assert_eq!(
        app.accept_active_detached_window_close(ctx, window, "depth_modal_test"),
        Some(false)
    );
    let requested = PathBuf::from("depth-modal-forwarded.jpg");
    app.activation_open_path_tx
        .send(crate::single_instance::ActivationOpenPath::received(
            requested.clone(),
            None,
        ))
        .unwrap();
    app.poll_activation_open_paths(ctx);
    assert!(app.startup_open_path_resolve_pending.is_none());
    // Existing admission defers forwarded requests; it must not consume or open one.
    assert_eq!(
        app.activation_open_path_rx.try_recv().unwrap().path,
        requested
    );
    app.with_viewer_context(owner, |mounted| {
        let generation = mounted.items_generation;
        let presentation = mounted.viewer_presentation;
        let initial = mounted.fullscreen_idx;
        mounted.settings.page_turn_animation_enabled = false;
        mounted.settings.slideshow_end_action = crate::settings::SlideshowEndAction::NextFolder;
        for idx in [0, mounted.items.len() - 1] {
            mounted.fullscreen_idx = Some(idx);
            mounted.slideshow_playing = true;
            mounted.slideshow_anchor_idx = Some(idx);
            mounted.slideshow_next_at = Instant::now() - Duration::from_secs(1);
            let deadline = mounted.slideshow_next_at;
            for nav in [
                crate::ui_fullscreen::FsPageNav::Target(2),
                crate::ui_fullscreen::FsPageNav::None,
            ] {
                mounted.handle_fs_navigation(ctx, false, false, None, None, None, nav, None, idx);
                assert_eq!(mounted.fullscreen_idx, Some(idx));
                assert_eq!(mounted.slideshow_next_at, deadline);
                assert!(mounted.slideshow_playing);
                assert!(mounted.folder_nav_pending.is_none());
                assert_eq!(mounted.items_generation, generation);
                assert_eq!(mounted.viewer_presentation, presentation);
            }
        }
        mounted.fullscreen_idx = initial;
        mounted.slideshow_playing = false;
    })
    .unwrap();
    assert!(app.request_detached_image_window_close_by_ids(
        ctx,
        &[sibling_window],
        "depth_modal_sibling_close",
        None
    ));
    assert_eq!(app.active_viewer_context_id(), Some(owner));
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(app.fullscreen_idx, main_index);
}

#[test]
fn depth_setup_independent_checking_and_confirm_protect_owner_but_allow_sibling_close() {
    for checking in [true, false] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        app.push_window_context_for_test(&ctx, 80_470, |owner| install_pages(owner, &ctx));
        app.depth_setup = DepthSetup::for_test(app.tmp.path().join("missing-independent-modal"));
        if !checking {
            app.depth_setup.set_status_for_test(AddonStatus::Missing);
        }
        let owner =
            app.build_active_context_for_test(Some(80_471), DetachedSource::Image, |owner| {
                install_pages(owner, &ctx);
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
            });
        app.with_viewer_context(owner, |owner| {
            owner.mark_active_detached_viewport_rendered(&ctx);
            owner.toggle_depth_parallax(&ctx, 0);
        })
        .unwrap();
        assert_eq!(app.depth_setup.checking_for_test(), checking);
        assert_eq!(app.depth_setup.confirming_for_test(), !checking);
        assert_independent_modal_protection(&mut app, &ctx, owner, 80_471, 80_470);
    }
}

#[test]
fn depth_setup_main_and_preferences_keep_external_open_activation_and_slideshow_admission() {
    for preferences in [false, true] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        app.depth_setup.set_status_for_test(AddonStatus::Missing);
        if preferences {
            app.request_depth_addon_setup(&ctx);
        } else {
            app.toggle_depth_parallax(&ctx, 0);
        }
        assert!(app.depth_setup.modal_open());
        assert!(!app.document_open_modal_admission_blocked());
        app.settings.page_turn_animation_enabled = false;
        app.handle_fs_navigation(
            &ctx,
            false,
            false,
            None,
            None,
            None,
            crate::ui_fullscreen::FsPageNav::Target(3),
            None,
            0,
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(3),
            "main/preferences navigation remains admitted"
        );
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = Some(3);
        app.slideshow_next_at = Instant::now() - Duration::from_secs(1);
        app.settings.slideshow_end_action = crate::settings::SlideshowEndAction::Stop;
        app.handle_fs_navigation(
            &ctx,
            false,
            false,
            None,
            None,
            None,
            crate::ui_fullscreen::FsPageNav::None,
            None,
            3,
        );
        assert!(
            !app.slideshow_playing,
            "main/preferences timer reaches its normal end"
        );
        app.push_window_context_for_test(&ctx, 80_472, |owner| {
            install_pages(owner, &ctx);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.detached_viewer_independent_active = true;
        });
        assert!(app.activate_detached_image_window_snapshot(&ctx, 80_472));
        app.activation_open_path_tx
            .send(crate::single_instance::ActivationOpenPath::received(
                app.tmp.path().join("external-main-addon.jpg"),
                None,
            ))
            .unwrap();
        app.poll_activation_open_paths(&ctx);
        assert!(
            app.startup_open_path_resolve_pending.is_some(),
            "main/preferences forwarded path starts resolving"
        );
    }
}

#[test]
fn depth_setup_running_operations_remain_modeless_during_accepted_independent_transition() {
    for operation in [
        crate::addon::AddonOperation::Installing,
        crate::addon::AddonOperation::Cancelling,
        crate::addon::AddonOperation::Removing,
    ] {
        let ctx = egui::Context::default();
        let mut app = setup_app_for_test();
        install_pages(&mut app, &ctx);
        app.depth_setup.set_status_for_test(AddonStatus::Missing);
        app.depth_setup.set_operation_for_test(operation);
        let current = app.tmp.path().join("modeless-transition");
        std::fs::create_dir(&current).unwrap();
        let owner =
            app.build_active_context_for_test(Some(80_473), DetachedSource::Image, |owner| {
                install_pages(owner, &ctx);
                owner.current_folder = Some(current.clone());
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
            });
        app.with_viewer_context(owner, |owner| {
            owner.mark_active_detached_viewport_rendered(&ctx);
            owner.start_folder_nav(current.clone(), true, FolderNavMode::Fullscreen);
            assert!(owner.active_detached_transition_outstanding());
            owner.toggle_depth_parallax(&ctx, 0);
            assert!(owner.depth_setup.dialog_open_for_test(), "{operation:?}");
            assert_eq!(
                owner.depth_setup.dialog_owner_context(),
                Some(owner.edit_request_owner_context())
            );
            assert!(!owner.depth_setup.modal_open());
            assert!(!owner.document_open_modal_admission_blocked());
            assert!(
                owner.fs_feedback_toast.is_none(),
                "modeless progress is not refused"
            );
            assert_eq!(
                owner.accept_active_detached_window_close(&ctx, 80_473, "modeless_operation_close"),
                Some(true)
            );
        })
        .unwrap();
    }
}
