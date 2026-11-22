//! CPU/egui state tests; no wgpu renderer, native application or input injection.
use super::detached_window_manager::DetachedWindowPurpose;
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::detached_window_geometry::PhysicalWindowPlacement;

const WINDOW: u64 = 818_301;

fn session_identity(app: &App) -> Option<(u64, DetachedSource, DetachedSessionContentPhase)> {
    app.active_detached_session
        .map(|session| (session.window_id, session.source, session.content_phase))
}

fn fixture(
    purpose: DetachedWindowPurpose,
) -> (AppTestEnv, PhysicalWindowPlacement, PhysicalWindowPlacement) {
    let mut app = setup_app();
    app.settings.detached_viewer_enabled = true;
    app.settings.detached_viewer_open_images_in_window = false;
    let path = app.tmp.path().join("page.png");
    image::RgbaImage::new(2, 2).save(&path).unwrap();
    app.items = vec![GridItem::Image(path)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.fullscreen_idx = Some(0);
    app.fs_cache.insert(0, FsCacheEntry::Failed);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    app.detached_window_manager
        .create_with_purpose(WINDOW, true, purpose);
    app.begin_mounted_detached_session_for_test(WINDOW, DetachedSource::Image);
    let saved = app.default_detached_viewer_window_placement();
    let moved = PhysicalWindowPlacement {
        outer: crate::detached_window_geometry::PhysicalRect {
            left: saved.outer.left + 8,
            right: saved.outer.right + 8,
            ..saved.outer
        },
        ..saved
    };
    assert!(App::detached_window_runtime_placement_is_usable(moved));
    app.settings.detached_viewer_window_physical_placement = Some(saved);
    app.set_detached_window_runtime_placement(WINDOW, moved, "browser818_fixture");
    (app, saved, moved)
}

fn has_main_focus(output: &egui::FullOutput) -> bool {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|out| {
            out.commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Focus))
        })
}

#[test]
fn browser818_close_policy_table_covers_every_purpose_and_typed_cause() {
    for purpose in [
        DetachedWindowPurpose::Normal,
        DetachedWindowPurpose::BrowserView,
    ] {
        for cause in [
            ViewerCloseCause::User,
            ViewerCloseCause::DetachedToggle,
            ViewerCloseCause::BrowserReplacement,
            ViewerCloseCause::Replaced,
            ViewerCloseCause::RemoteAcquire,
            ViewerCloseCause::AppExit,
        ] {
            let owner = DetachedCloseOwner {
                lease: App::detached_session_lease(WINDOW),
                purpose,
                cause,
            };
            assert_eq!(
                owner.policy().save_placement,
                purpose == DetachedWindowPurpose::Normal
            );
            let expected_focus = match (purpose, cause) {
                (_, ViewerCloseCause::BrowserReplacement)
                | (
                    DetachedWindowPurpose::BrowserView,
                    ViewerCloseCause::User | ViewerCloseCause::DetachedToggle,
                ) => DetachedCloseFocus::Skip,
                _ => DetachedCloseFocus::Existing,
            };
            assert_eq!(
                owner.policy().main_focus,
                expected_focus,
                "{purpose:?}/{cause:?}"
            );
        }
    }
}

#[test]
fn browser818_close_policy_esc_and_x_keep_owner_after_early_runtime_removal() {
    for purpose in [
        DetachedWindowPurpose::Normal,
        DetachedWindowPurpose::BrowserView,
    ] {
        for os_close in [false, true] {
            let (mut app, saved, moved) = fixture(purpose);
            let ctx = egui::Context::default();
            let owner = app
                .projected_detached_close_owner(ViewerCloseCause::User)
                .unwrap();
            let output = ctx.run_pass(Default::default(), |ctx| {
                if os_close {
                    assert!(matches!(
                        app.accept_viewport_close_request(
                            ctx,
                            App::detached_image_window_viewport_id(WINDOW)
                        ),
                        eframe::ViewportCloseResponse::RetirementTransferred
                    ));
                } else {
                    app.handle_fullscreen_close_request();
                }
                assert!(
                    app.detached_window_manager.runtime(WINDOW).is_none(),
                    "teardown precedes finalization"
                );
                assert_eq!(owner.purpose, purpose);
                app.finalize_closed_active_detached_viewport(
                    ctx,
                    App::detached_image_window_viewport_id(WINDOW),
                    owner,
                );
            });
            assert_eq!(
                has_main_focus(&output),
                purpose == DetachedWindowPurpose::Normal
            );
            assert_eq!(
                app.settings.detached_viewer_window_physical_placement,
                Some(if purpose == DetachedWindowPurpose::Normal {
                    moved
                } else {
                    saved
                })
            );
            assert!(app.fullscreen_idx.is_none());
            assert!(app.active_detached_session.is_none());
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn browser818_close_policy_browser_replacement_saves_only_normal_and_skips_focus() {
    for purpose in [
        DetachedWindowPurpose::Normal,
        DetachedWindowPurpose::BrowserView,
    ] {
        let (mut app, saved, moved) = fixture(purpose);
        let owner = app
            .projected_detached_close_owner(ViewerCloseCause::BrowserReplacement)
            .unwrap();
        let ctx = egui::Context::default();
        let output = ctx.run_pass(Default::default(), |ctx| {
            app.begin_detached_close_owner(owner, "browser818_replace");
            app.finish_detached_close_owner(owner, "browser818_replace");
            app.close_fullscreen();
            app.finalize_closed_active_detached_viewport(
                ctx,
                App::detached_image_window_viewport_id(WINDOW),
                owner,
            );
        });
        assert!(!has_main_focus(&output));
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(if purpose == DetachedWindowPurpose::Normal {
                moved
            } else {
                saved
            })
        );
        output.drop_without_applying_deltas();
    }
}

#[test]
fn browser818_close_policy_settings_remote_and_exit_preserve_existing_focus_policy() {
    for purpose in [
        DetachedWindowPurpose::Normal,
        DetachedWindowPurpose::BrowserView,
    ] {
        for cause in [
            ViewerCloseCause::Replaced,
            ViewerCloseCause::RemoteAcquire,
            ViewerCloseCause::AppExit,
            ViewerCloseCause::BrowserReplacement,
        ] {
            let (mut app, saved, moved) = fixture(purpose);
            let ctx = egui::Context::default();
            let output = ctx.run_pass(Default::default(), |ctx| {
                assert!(app.close_all_viewer_sessions_with_cause(ctx, cause).is_ok());
            });
            assert!(app.detached_window_manager.is_empty());
            assert!(app.fullscreen_idx.is_none());
            assert_eq!(
                has_main_focus(&output),
                cause != ViewerCloseCause::BrowserReplacement,
                "settings/Remote/exit retain the existing bulk-close focus path"
            );
            assert_eq!(
                app.settings.detached_viewer_window_physical_placement,
                Some(if purpose == DetachedWindowPurpose::Normal {
                    moved
                } else {
                    saved
                })
            );
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn browser818_close_policy_passive_batch_captures_each_window_before_teardown() {
    for normal_in_batch in [false, true] {
        let mut app = setup_app();
        let saved = app.default_detached_viewer_window_placement();
        app.settings.detached_viewer_window_physical_placement = Some(saved);
        let browser = WINDOW + 1;
        app.detached_window_manager.create_with_purpose(
            browser,
            false,
            DetachedWindowPurpose::BrowserView,
        );
        app.build_window_context_for_test(browser, |_| {});
        app.set_detached_window_runtime_placement(browser, saved, "browser818_passive");
        let mut ids = vec![browser];
        if normal_in_batch {
            let normal = WINDOW + 2;
            app.detached_window_manager.create_with_purpose(
                normal,
                false,
                DetachedWindowPurpose::Normal,
            );
            app.build_window_context_for_test(normal, |_| {});
            app.set_detached_window_runtime_placement(normal, saved, "browser818_passive");
            ids.push(normal);
        }
        let ctx = egui::Context::default();
        let output = ctx.run_pass(Default::default(), |ctx| {
            app.close_detached_image_windows_by_ids(ctx, &ids, "browser818_passive_close", None);
        });
        for id in ids {
            assert!(app.detached_window_manager.runtime(id).is_none());
            assert!(app.locate_window_context(id).is_none());
        }
        assert_eq!(has_main_focus(&output), normal_in_batch);
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(saved)
        );
        output.drop_without_applying_deltas();
    }
}

#[test]
fn browser818_close_policy_browser_close_preserves_sibling_binding_cache_and_placement() {
    let (mut app, saved, _) = fixture(DetachedWindowPurpose::BrowserView);
    let sibling_window = WINDOW + 3;
    app.detached_window_manager.create_with_purpose(
        sibling_window,
        false,
        DetachedWindowPurpose::Normal,
    );
    let sibling = app.build_window_context_for_test(sibling_window, |owner| {
        owner.fs_cache.insert(77, FsCacheEntry::Failed);
    });
    app.set_detached_window_runtime_placement(sibling_window, saved, "browser818_sibling");
    let sibling_state = app.detached_window_state(sibling_window);
    let before = app
        .with_viewer_context(sibling, |owner| {
            (
                owner.items_generation,
                std::sync::Arc::clone(&owner.cancel_token),
            )
        })
        .unwrap();
    let owner = app
        .projected_detached_close_owner(ViewerCloseCause::User)
        .unwrap();
    let ctx = egui::Context::default();
    let output = ctx.run_pass(Default::default(), |ctx| {
        app.handle_fullscreen_close_request();
        app.finalize_closed_active_detached_viewport(
            ctx,
            App::detached_image_window_viewport_id(WINDOW),
            owner,
        );
    });
    assert!(!has_main_focus(&output));
    assert_eq!(app.viewer_context_window(sibling), Some(sibling_window));
    assert_eq!(app.detached_window_state(sibling_window), sibling_state);
    assert_eq!(
        app.detached_window_manager.placement(sibling_window),
        Some(saved)
    );
    app.with_viewer_context(sibling, |owner| {
        assert_eq!(owner.items_generation, before.0);
        assert!(std::sync::Arc::ptr_eq(&owner.cancel_token, &before.1));
        assert!(
            !owner
                .cancel_token
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(matches!(
            owner.fs_cache.get(&77),
            Some(FsCacheEntry::Failed)
        ));
    })
    .unwrap();
    output.drop_without_applying_deltas();
}

#[test]
fn browser818_close_policy_f11_round_trip_and_park_keep_purpose_and_saved_position() {
    for purpose in [
        DetachedWindowPurpose::Normal,
        DetachedWindowPurpose::BrowserView,
    ] {
        let (mut app, saved, moved) = fixture(purpose);
        app.window_visible = true;
        app.set_detached_window_host_for_test(WINDOW, 0x8183);
        let ctx = egui::Context::default();
        crate::app::detached_placement::with_host_geometry_for_test(
            WINDOW,
            moved.outer,
            false,
            || {
                app.toggle_detached_viewer_borderless_fullscreen(&ctx);
                let transition = app.detached_viewer_borderless_transition.take().unwrap();
                assert!(transition.target_borderless);
                assert!(app.apply_detached_viewer_borderless_target(&ctx, transition, true));
                assert!(app.detached_viewer_borderless_fullscreen);
                assert_eq!(
                    app.detached_window_manager
                        .runtime(WINDOW)
                        .unwrap()
                        .purpose(),
                    purpose
                );
                assert_eq!(
                    app.settings.detached_viewer_window_physical_placement,
                    Some(saved)
                );
                app.toggle_detached_viewer_borderless_fullscreen(&ctx);
                let transition = app.detached_viewer_borderless_transition.take().unwrap();
                assert!(!transition.target_borderless);
                assert!(app.apply_detached_viewer_borderless_target(&ctx, transition, true));
                assert!(!app.detached_viewer_borderless_fullscreen);
                assert_eq!(app.detached_window_manager.placement(WINDOW), Some(moved));
            },
        );
        for state in [
            DetachedWindowState::Parked,
            DetachedWindowState::Resuming,
            DetachedWindowState::Active,
        ] {
            app.transition_detached_window_state(WINDOW, state, "browser818_lifecycle");
            assert_eq!(
                app.detached_window_manager
                    .runtime(WINDOW)
                    .unwrap()
                    .purpose(),
                purpose
            );
        }
        // A later creation attempt cannot relabel an existing runtime.
        app.detached_window_manager.create_with_purpose(
            WINDOW,
            true,
            DetachedWindowPurpose::Normal,
        );
        assert_eq!(
            app.detached_window_manager
                .runtime(WINDOW)
                .unwrap()
                .purpose(),
            purpose
        );
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(saved)
        );
        let owner = app
            .projected_detached_close_owner(ViewerCloseCause::User)
            .unwrap();
        app.begin_detached_close_owner(owner, "browser818_f11_close");
        app.finish_detached_close_owner(owner, "browser818_f11_close");
        assert_eq!(
            app.settings.detached_viewer_window_physical_placement,
            Some(if purpose == DetachedWindowPurpose::Normal {
                moved
            } else {
                saved
            })
        );
    }
}

#[test]
fn browser818_toggle_main_viewer_remapping_and_gamepad_share_the_browser_refusal() {
    let _input = crate::key_input::lock_test_input();
    for route in ["main", "viewer", "gamepad"] {
        for remapped in [false, true] {
            let (mut app, _, _) = fixture(DetachedWindowPurpose::BrowserView);
            let (key, chord) = if remapped {
                (egui::Key::F10, "Ctrl+Alt+F10")
            } else {
                (egui::Key::F12, "F12")
            };
            app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                "[Global]\nToggleDetachedViewerMode = {chord}\n"
            ));
            let before = app.settings.detached_viewer_enabled;
            let session = session_identity(&app);
            assert!(app.detached_toggle_row_state().disabled_reason.is_some());
            let ctx = egui::Context::default();
            let modifiers = egui::Modifiers {
                ctrl: remapped,
                alt: remapped,
                ..Default::default()
            };
            let input = egui::RawInput {
                focused: true,
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            let output = ctx.run_pass(input, |ctx| {
                app.input_begin_key_frame(ctx);
                match route {
                    "main" => app.input_main_grid_shortcuts(ctx, false),
                    "viewer" => {
                        app.handle_fs_key_input(ctx, 0, false);
                    }
                    "gamepad" => {
                        app.apply_ring_action(
                            ctx,
                            crate::ring_shortcut::RingShortcutContext::ImageFullscreen,
                            crate::ring_shortcut::RingActionId::ToggleDetachedViewer,
                            "gamepad",
                        );
                    }
                    _ => unreachable!(),
                }
            });
            assert_eq!(
                app.settings.detached_viewer_enabled, before,
                "{route}/{chord}"
            );
            assert_eq!(session_identity(&app), session);
            assert_eq!(app.detached_viewer_window_id(), Some(WINDOW));
            assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
            let warning = &app
                .fs_feedback_toast
                .as_ref()
                .expect("common action refused")
                .0;
            assert!(
                warning.contains(
                    &app.keymap
                        .first_chord_label(crate::keymap::KeyAction::ToggleDetachedViewerMode)
                        .unwrap()
                ),
                "{route}: {warning}"
            );
            assert!(warning.contains("ブラウザから開いた窓"));
            assert!(!has_main_focus(&output));
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn browser818_toggle_normal_f12_round_trip_still_changes_preference_and_retires_lease() {
    let (mut app, _, moved) = fixture(DetachedWindowPurpose::Normal);
    assert!(!app.browser_view_blocks_detached_toggle());
    app.toggle_detached_viewer_mode();
    assert!(!app.settings.detached_viewer_enabled);
    assert!(app.active_detached_session.is_none());
    assert!(app.detached_window_manager.runtime(WINDOW).is_none());
    assert_eq!(
        app.settings.detached_viewer_window_physical_placement,
        Some(moved)
    );
    app.toggle_detached_viewer_mode();
    assert!(app.settings.detached_viewer_enabled);
    let new_window = app.detached_viewer_window_id().unwrap();
    assert_ne!(new_window, WINDOW);
    assert_eq!(
        app.detached_window_manager
            .runtime(new_window)
            .unwrap()
            .purpose(),
        DetachedWindowPurpose::Normal
    );
}

#[test]
fn browser818_toggle_root_targets_active_browser_but_mounted_normal_uses_its_own_lease() {
    let (mut app, _, _) = fixture(DetachedWindowPurpose::BrowserView);
    let browser_session = session_identity(&app);
    let sibling_window = WINDOW + 7;
    let sibling = app.build_window_context_for_test(sibling_window, |owner| {
        owner.items = vec![GridItem::Image("normal.png".into())];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    app.detached_window_manager.create_with_purpose(
        sibling_window,
        false,
        DetachedWindowPurpose::Normal,
    );
    app.with_viewer_context(sibling, |owner| {
        assert!(!owner.browser_view_blocks_detached_toggle());
        assert!(owner.detached_toggle_row_state().disabled_reason.is_none());
    })
    .unwrap();
    assert_eq!(session_identity(&app), browser_session);
    assert!(app.browser_view_blocks_detached_toggle());
    // A main/root action during an index-less active reopening still targets that lease.
    app.fullscreen_idx = None;
    assert!(app.browser_view_blocks_detached_toggle());
    app.toggle_detached_viewer_mode();
    assert!(app.settings.detached_viewer_enabled);
    assert_eq!(session_identity(&app), browser_session);
}
