use super::*;

struct MenuFrame {
    toggle: egui::Response,
    shapes: Vec<egui::epaint::ClippedShape>,
}

fn menu_frame(
    app: &mut crate::app::App,
    ctx: &egui::Context,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> MenuFrame {
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut toggle = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(full),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    app.draw_fullscreen_seek_overlay(ui, ctx, full, 3, false, false);
                    toggle =
                        ctx.read_response(egui::Id::new(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID));
                });
        },
    );
    MenuFrame {
        toggle: toggle.expect("still seek strip menu button response"),
        shapes: output.shapes,
    }
}

fn touch_toggle_frame(
    app: &mut crate::app::App,
    ctx: &egui::Context,
    size: egui::Vec2,
) -> MenuFrame {
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut toggle = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(full),
            ..Default::default()
        },
        |ctx| {
            test_arm_bar_button_touch_click(ctx, FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID, full);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    app.draw_fullscreen_seek_overlay(ui, ctx, full, 3, false, false);
                    toggle =
                        ctx.read_response(egui::Id::new(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID));
                });
        },
    );
    MenuFrame {
        toggle: toggle.expect("touch-only still seek menu button response"),
        shapes: output.shapes,
    }
}

fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn text_center(frame: &MenuFrame, expected: &str) -> egui::Pos2 {
    frame
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == expected => {
                Some(clipped.shape.visual_bounding_rect().center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing popup row: {expected}"))
}

fn open_menu(app: &mut crate::app::App, ctx: &egui::Context, size: egui::Vec2) -> MenuFrame {
    let frame = menu_frame(app, ctx, size, Vec::new());
    let _ = menu_frame(app, ctx, size, click_events(frame.toggle.rect.center()));
    assert!(fs_still_seek_strip_popup_open(ctx));
    menu_frame(app, ctx, size, Vec::new())
}

fn menu_key_frame(key: egui::Key) -> (FsKeyAction, bool) {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    crate::os_theme::apply_resolved(&ctx, crate::os_theme::ResolvedTheme::Dark);
    crate::ui_fonts::configure_fonts(&ctx);
    let size = egui::vec2(640.0, 360.0);
    egui::Popup::open_id(
        &ctx,
        egui::Id::new(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID).with("popup"),
    );
    let mut action = FsKeyAction {
        close: false,
        close_to_page_list: false,
        page_nav: FsPageNav::None,
        ctrl_nav: None,
        sibling_nav: None,
        mouse_nav: None,
        jump_to: None,
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events: vec![plain_key_press(key)],
            ..Default::default()
        },
        |ctx| {
            cache_fullscreen_keyboard_owner_for_test(ctx);
            action = app.handle_fs_key_input(ctx, 3, false);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    app.draw_fullscreen_seek_overlay(
                        ui,
                        ctx,
                        egui::Rect::from_min_size(egui::Pos2::ZERO, size),
                        3,
                        false,
                        false,
                    );
                });
        },
    );
    (action, fs_still_seek_strip_popup_open(&ctx))
}

#[test]
fn real_still_seek_popup_selects_visibility_and_all_five_configured_heights() {
    use crate::settings::{BottomBarLock, StillSeekStripHeight};

    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    crate::os_theme::apply_resolved(&ctx, crate::os_theme::ResolvedTheme::Dark);
    crate::ui_fonts::configure_fonts(&ctx);
    let size = egui::vec2(640.0, 360.0);
    app.settings.still_seek_strip_height_values.maximum = 311;
    app.settings.still_seek_strip_height_values.large = 223;
    app.settings.still_seek_strip_height_values.medium = 137;
    app.settings.still_seek_strip_height_values.small = 67;
    app.settings.still_seek_strip_height_values.smallest = 39;

    for preset in StillSeekStripHeight::ALL {
        let open = open_menu(&mut app, &ctx, size);
        assert!(text_center(&open, "非表示").is_finite());
        assert!(text_center(&open, "表示").is_finite());
        for listed in StillSeekStripHeight::ALL {
            let points = app.settings.still_seek_strip_height_values.points(listed);
            assert!(
                text_center(&open, &format!("高さ: {} ({points:.0} px)", listed.label()))
                    .is_finite(),
                "all configured presets must remain reachable"
            );
        }
        let points = app.settings.still_seek_strip_height_values.points(preset);
        let row = text_center(&open, &format!("高さ: {} ({points:.0} px)", preset.label()));
        let _ = menu_frame(&mut app, &ctx, size, click_events(row));
        assert_eq!(app.settings.still_seek_strip_height, preset);
        assert!(!fs_still_seek_strip_popup_open(&ctx));
    }

    let open = open_menu(&mut app, &ctx, size);
    let _ = menu_frame(
        &mut app,
        &ctx,
        size,
        click_events(text_center(&open, "非表示")),
    );
    assert!(!app.settings.still_seek_strip_visible);
    assert_eq!(app.settings.still_bottom_lock(), BottomBarLock::BarOnly);

    let open = open_menu(&mut app, &ctx, size);
    let _ = menu_frame(
        &mut app,
        &ctx,
        size,
        click_events(text_center(&open, "表示")),
    );
    assert!(app.settings.still_seek_strip_visible);
    assert_eq!(
        app.settings.still_bottom_lock(),
        BottomBarLock::BarOnly,
        "menu visibility must not silently acquire the strip lock"
    );
}

#[test]
fn still_seek_popup_keeps_the_bottom_bar_visible_away_from_the_hover_band() {
    let mut app = still_seek_edge_test_app();
    app.settings
        .set_still_bottom_lock(crate::settings::BottomBarLock::None);
    let ctx = egui::Context::default();
    crate::os_theme::apply_resolved(&ctx, crate::os_theme::ResolvedTheme::Dark);
    crate::ui_fonts::configure_fonts(&ctx);
    let size = egui::vec2(640.0, 360.0);

    let hover = menu_frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(egui::pos2(620.0, 350.0))],
    );
    let _ = menu_frame(
        &mut app,
        &ctx,
        size,
        click_events(hover.toggle.rect.center()),
    );
    assert!(fs_still_seek_strip_popup_open(&ctx));

    let open = menu_frame(&mut app, &ctx, size, Vec::new());
    let popup_row = text_center(&open, "非表示");
    let away = menu_frame(
        &mut app,
        &ctx,
        size,
        vec![egui::Event::PointerMoved(popup_row)],
    );
    assert!(fs_still_seek_strip_popup_open(&ctx));
    assert!(away.toggle.rect.is_positive());
    assert!(app.fs_seek_overlay_visible);
}

#[test]
fn still_seek_popup_toggles_once_for_mouse_and_touch_logical_clicks() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    let size = egui::vec2(640.0, 360.0);
    let closed = menu_frame(&mut app, &ctx, size, Vec::new());
    let _ = menu_frame(
        &mut app,
        &ctx,
        size,
        click_events(closed.toggle.rect.center()),
    );
    assert!(fs_still_seek_strip_popup_open(&ctx));
    let _ = menu_frame(
        &mut app,
        &ctx,
        size,
        click_events(closed.toggle.rect.center()),
    );
    assert!(!fs_still_seek_strip_popup_open(&ctx));

    let opened = touch_toggle_frame(&mut app, &ctx, size);
    assert!(
        !opened.toggle.clicked(),
        "fixture must exercise only the correlated touch click"
    );
    assert!(fs_still_seek_strip_popup_open(&ctx));

    let closed = touch_toggle_frame(&mut app, &ctx, size);
    assert!(!closed.toggle.clicked());
    assert!(
        !fs_still_seek_strip_popup_open(&ctx),
        "the second touch tap must toggle the same popup closed exactly once"
    );
}

#[test]
fn still_seek_popup_owns_keyboard_before_fullscreen_shortcuts() {
    let (escape, escape_popup_open) = menu_key_frame(egui::Key::Escape);
    assert!(
        !escape.close,
        "Escape must not close fullscreen behind the popup"
    );
    assert!(escape.page_nav.is_none());
    assert!(
        !escape_popup_open,
        "egui Popup must receive Escape and close itself"
    );

    for key in [
        egui::Key::ArrowLeft,
        egui::Key::ArrowRight,
        egui::Key::Enter,
    ] {
        let (action, _) = menu_key_frame(key);
        assert!(
            !action.close,
            "{key:?} must not close fullscreen behind the popup"
        );
        assert!(
            action.page_nav.is_none()
                && action.ctrl_nav.is_none()
                && action.sibling_nav.is_none()
                && action.mouse_nav.is_none()
                && action.jump_to.is_none(),
            "{key:?} must stay with the popup instead of triggering background navigation"
        );
    }
}

#[test]
fn still_seek_popup_regular_and_narrow_snapshots() {
    let mut snapshots = egui_kittest::SnapshotResults::new();
    for (name, size) in [
        ("still_seek_strip_menu_regular", egui::vec2(640.0, 360.0)),
        ("still_seek_strip_menu_narrow", egui::vec2(280.0, 360.0)),
    ] {
        let mut app = still_seek_edge_test_app();
        app.settings.still_seek_strip_height_values.maximum = 288;
        app.settings.still_seek_strip_height_values.large = 192;
        app.settings.still_seek_strip_height_values.medium = 120;
        app.settings.still_seek_strip_height_values.small = 64;
        app.settings.still_seek_strip_height_values.smallest = 36;
        app.settings.still_seek_strip_height = crate::settings::StillSeekStripHeight::Medium;
        let mut initialized = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build(|ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !initialized {
                    crate::ui_fonts::configure_fonts(ctx);
                    egui::Popup::open_id(
                        ctx,
                        egui::Id::new(FULLSCREEN_STILL_SEEK_STRIP_BUTTON_ID).with("popup"),
                    );
                    initialized = true;
                    ctx.request_repaint();
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let full = ctx.content_rect();
                        app.draw_fullscreen_seek_overlay(ui, ctx, full, 3, false, false);
                    });
            });
        harness.run();
        harness.snapshot(name);
        snapshots.extend_harness(&mut harness);
    }
    snapshots.unwrap();
}
