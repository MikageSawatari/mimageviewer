use super::{phase_c_support::setup_app, *};
use egui_kittest::kittest::Queryable;

fn request_whats_new(app: &mut App) {
    app.settings.whats_new_acknowledged_version = Some("3.9.0".to_owned());
    app.whats_new_entries = crate::version_highlights::highlights_to_show(
        app.settings.whats_new_acknowledged_version.as_deref(),
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    );
    assert!(!app.whats_new_entries.is_empty());
    app.show_whats_new = true;
    app.show_mouse_nav_migration_prompt = false;
    app.network_data_dir_notice = None;
}

fn escape_input() -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    }
}

#[test]
fn startup_notices_defer_drawing_modal_and_escape_in_every_viewer_mode() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.items = vec![GridItem::Image(PathBuf::from("notice-test.jpg"))];
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.whats_new_visible(), "{presentation:?}");
        assert!(!app.common_modal_dialog_open(), "{presentation:?}");
        assert!(!app.any_modal_dialog_open_for_fullscreen_keys());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_whats_new_dialog(&ctx);
        assert!(
            app.dialog_escape_pressed(&ctx),
            "pending notice must not consume Escape"
        );
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty(), "pending notice must not draw");
        output.drop_without_applying_deltas();
        assert!(app.show_whats_new);
        assert_eq!(
            app.settings.whats_new_acknowledged_version.as_deref(),
            Some("3.9.0")
        );

        // Closing the viewer exposes the request; reopening hides it again without a latch.
        app.fullscreen_idx = None;
        assert!(app.whats_new_visible());
        assert_eq!(app.modal_dialog_block_reason(), Some("whats_new"));
    }
}

#[test]
fn startup_notices_embedded_viewer_escape_does_not_acknowledge_notice_in_same_pass() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.items = vec![GridItem::Image(PathBuf::from("notice-test.jpg"))];
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::MainWindow;
    let ctx = egui::Context::default();
    ctx.begin_pass(escape_input());
    // The deferred dialog does not inspect Escape. The real viewer handler consumes it.
    app.show_whats_new_dialog(&ctx);
    let action = app.handle_fs_key_input(&ctx, 0, false);
    assert!(action.close, "viewer must accept Escape");
    app.handle_fullscreen_close_request();
    assert!(
        app.fullscreen_idx.is_none(),
        "viewer Escape must remain usable"
    );
    app.show_whats_new_dialog(&ctx);
    assert!(
        app.show_whats_new,
        "the same Escape must not dismiss the newly visible notice"
    );
    assert_eq!(
        app.settings.whats_new_acknowledged_version.as_deref(),
        Some("3.9.0")
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn startup_notices_wait_for_active_and_other_detached_viewers() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    let ctx = egui::Context::default();
    app.build_active_context_for_test(Some(1232), DetachedSource::Image, |owner| {
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(app.fullscreen_idx.is_none());
    assert!(app.active_detached_context_is_at_rest());
    assert!(!app.whats_new_visible());
    assert!(!app.common_modal_dialog_open());
    app.remove_active_context_for_test();
    assert!(app.whats_new_visible());

    app.push_window_context_for_test(&ctx, 1233, |owner| {
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(app.fullscreen_idx.is_none());
    assert!(!app.whats_new_visible());
    assert!(!app.common_modal_dialog_open());
}

#[test]
fn startup_notices_network_notice_waits_for_viewer_and_prior_notices() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.network_data_dir_notice = Some(PathBuf::from(r"\\server\share\data"));
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.network_data_dir_notice_visible());
        app.show_whats_new = false;
        assert!(!app.network_data_dir_notice_visible());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_network_data_dir_notice_dialog(&ctx);
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(app.network_data_dir_notice.is_some());
        app.fullscreen_idx = None;
        app.show_whats_new = true;
        assert!(!app.network_data_dir_notice_visible());
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = true;
        assert!(!app.network_data_dir_notice_visible());
        app.show_mouse_nav_migration_prompt = false;
        assert!(app.network_data_dir_notice_visible());
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("network_data_dir_notice")
        );
    }
}

#[test]
fn startup_notices_whats_new_escape_does_not_dismiss_following_network_notice() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.network_data_dir_notice = Some(PathBuf::from(r"\\server\share\data"));
    let ctx = egui::Context::default();
    let mut input = escape_input();
    // A modifier may have been released later in the same input batch. Consume the
    // actual Escape events rather than matching their modifier against current level.
    if let egui::Event::Key { modifiers, .. } = &mut input.events[0] {
        *modifiers = egui::Modifiers::CTRL;
    }
    ctx.begin_pass(input);
    app.show_whats_new_dialog(&ctx);
    assert!(!app.show_whats_new);
    assert!(app.network_data_dir_notice_visible());
    app.show_network_data_dir_notice_dialog(&ctx);
    assert!(app.network_data_dir_notice.is_some());
    assert_eq!(
        app.modal_dialog_block_reason(),
        Some("network_data_dir_notice")
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn startup_notices_help_preserves_pending_cumulative_entries_and_reopens_latest_after_dismissal() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    let expected: Vec<_> = app
        .whats_new_entries
        .iter()
        .map(|entry| entry.version)
        .collect();
    assert!(expected.len() > 1);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.request_whats_new_from_help();
    assert_eq!(
        app.whats_new_entries
            .iter()
            .map(|entry| entry.version)
            .collect::<Vec<_>>(),
        expected
    );
    assert!(!app.whats_new_visible());
    app.fullscreen_idx = None;
    app.show_whats_new = false;
    app.whats_new_entries.clear();
    app.request_whats_new_from_help();
    assert!(app.whats_new_visible());
    let latest = crate::version_highlights::latest_not_newer_than(
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    )
    .unwrap();
    assert_eq!(app.whats_new_entries.len(), 1);
    assert_eq!(app.whats_new_entries[0].version, latest.version);
}

#[test]
fn startup_notices_acknowledgement_save_failure_notifies_without_removing_stored_unread_version() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    assert!(app.settings.save_checked());
    crate::settings_db::set_save_suppressed(true);
    let ctx = egui::Context::default();
    ctx.begin_pass(escape_input());
    app.show_whats_new_dialog(&ctx);
    assert!(!app.show_whats_new);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("保存できませんでした")
    );
    assert_eq!(
        app.fs_feedback_toast_surface,
        Some(ActionSurface::MainWindow)
    );
    ctx.end_pass().drop_without_applying_deltas();
    crate::settings_db::set_save_suppressed(false);
    let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
    assert_eq!(
        stored.whats_new_acknowledged_version.as_deref(),
        Some("3.9.0")
    );
}

#[test]
fn startup_notices_constructor_uses_acknowledgement_instead_of_startup_version() {
    let settings = crate::settings::Settings {
        last_seen_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        whats_new_acknowledged_version: Some("3.9.0".to_owned()),
        ..Default::default()
    };
    let app = super::phase_c_support::setup_paused_similar_app_with_fixture(settings, |_| {});
    assert!(app.show_whats_new);
    let expected = crate::version_highlights::highlights_to_show(
        Some("3.9.0"),
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    );
    assert_eq!(
        app.whats_new_entries
            .iter()
            .map(|e| e.version)
            .collect::<Vec<_>>(),
        expected.iter().map(|e| e.version).collect::<Vec<_>>()
    );
}

#[test]
fn startup_notices_first_setup_and_mouse_migration_share_visible_modal_policy() {
    let mut app = setup_app();
    app.settings.first_setup_completed = false;
    app.show_mouse_nav_migration_prompt = true;
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.first_setup_visible());
        assert!(!app.mouse_nav_migration_prompt_visible());
        assert!(!app.common_modal_dialog_open());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_first_setup_dialog(&ctx);
        app.show_mouse_nav_migration_prompt_dialog(&ctx);
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(!app.settings.first_setup_completed);
        assert!(app.show_mouse_nav_migration_prompt);
        app.fullscreen_idx = None;
        assert!(app.first_setup_visible());
        assert!(app.mouse_nav_migration_prompt_visible());
        assert!(app.common_modal_dialog_open());
    }
}

#[test]
fn startup_notices_dismissal_saves_current_version_for_all_close_actions() {
    for action in ["閉じる", "Close window", "Escape"] {
        let mut app = setup_app();
        request_whats_new(&mut app);
        let mut fonts_ready = false;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                move |ui, app: &mut super::phase_c_support::AppTestEnv| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    app.show_whats_new_dialog(ui.ctx());
                },
                app,
            );
        harness.run_steps(5);
        harness.get_by_label("閉じる");
        if action == "Escape" {
            harness.event(escape_input().events.remove(0));
        } else {
            harness.get_by_label(action).click_accesskit();
        }
        harness.run_steps(2);
        assert!(!harness.state().show_whats_new, "{action}");
        let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
        assert_eq!(
            stored.whats_new_acknowledged_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION")),
            "{action}"
        );
        assert!(
            crate::version_highlights::highlights_to_show(
                stored.whats_new_acknowledged_version.as_deref(),
                env!("CARGO_PKG_VERSION"),
                crate::version_highlights::table(),
            )
            .is_empty()
        );
    }
}
