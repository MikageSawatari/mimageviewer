//! Inputs outside the canvas-drag boundary retain their production behavior.
use super::drag_bound_handler_tests::{drag, pointer_frame};
use super::handler_tests::{app, frame, full_rect, seed, tap, wheel_event};
use super::*;
use crate::settings::{FsSidePanelMode, FullscreenLeftPanelTab, FullscreenWheelAction};

#[derive(Debug, PartialEq)]
struct ViewResult {
    pan: Vec2,
    zoom: f32,
    vertical_scroll: f32,
    index: Option<usize>,
}

fn result(app: &App) -> ViewResult {
    ViewResult {
        pan: app.fs_pan,
        zoom: app.fs_zoom,
        vertical_scroll: app.fs_vertical_scroll,
        index: app.fullscreen_idx,
    }
}

fn ready(ctx: &egui::Context, bound: bool) -> crate::app::AppTestEnvForTest {
    let mut app = app(ctx, SpreadMode::Single);
    app.settings.fullscreen_bound_drag_pan = bound;
    app.settings.fullscreen_side_panel_mode = FsSidePanelMode::ClickToShow;
    app.settings.touch_still_chrome_learned = true;
    app.fs_zoom = 2.0;
    seed(ctx, &mut app);
    app
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn drag_bound_setting_preserves_real_keyboard_scroll_handlers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for key in [
        egui::Key::ArrowDown,
        egui::Key::ArrowUp,
        egui::Key::J,
        egui::Key::K,
    ] {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            frame(&ctx, tap(key), |_, ctx| {
                assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
            });
            assert_ne!(app.fs_pan, Vec2::ZERO, "the actual key must move: {key:?}");
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "key={key:?}");
    }
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn drag_bound_setting_preserves_real_keyboard_zoom_handler() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let outcomes = [false, true].map(|bound| {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, bound);
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsZoomIn = F24\n");
        frame(&ctx, tap(egui::Key::F24), |_, ctx| {
            assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
        });
        assert!(app.fs_zoom > 2.0, "the configured zoom action must run");
        result(&app)
    });
    assert_eq!(outcomes[0], outcomes[1]);
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn drag_bound_setting_preserves_real_wheel_scroll_and_ctrl_wheel_pivot_zoom() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for modifiers in [egui::Modifiers::NONE, egui::Modifiers::CTRL] {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            app.settings.fullscreen_wheel_action = FullscreenWheelAction::InPageScroll;
            let pointer = egui::pos2(360.0, 220.0);
            pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(pointer)]);
            let nav = pointer_frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::PointerMoved(pointer),
                    wheel_event(Vec2::new(0.0, -40.0), modifiers),
                ],
            );
            assert!(nav.is_none());
            if modifiers.ctrl {
                assert_ne!(app.fs_zoom, 2.0, "Ctrl wheel must reach the zoom handler");
                assert_ne!(app.fs_pan.x, 0.0, "zoom preserves an off-center pivot");
            } else {
                assert!(app.fs_pan.y < 0.0, "plain wheel must scroll the image");
                assert_eq!(app.fs_zoom, 2.0);
            }
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "modifiers={modifiers:?}");
    }
}

#[test]
fn drag_bound_setting_preserves_gamepad_pan_and_zoom_handler_endpoints() {
    // This is the production analog-pan/zoom endpoint, without physical-device
    // polling or desktop foreground admission (which belong to gamepad_input).
    let outcomes = [false, true].map(|bound| {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, bound);
        assert!(app.apply_gamepad_fullscreen_pan(Vec2::new(700.0, 120.0)));
        assert_ne!(app.fs_pan, Vec2::ZERO);
        assert!(app.apply_gamepad_fullscreen_zoom(0.5, 0.25));
        assert!(app.fs_zoom > 2.0);
        result(&app)
    });
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn drag_bound_setting_preserves_real_navigator_primary_drag() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let outcomes = [false, true].map(|bound| {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, bound);
        app.settings.fullscreen_navigator_visible = true;
        let layout = app
            .fs_navigator_layout(full_rect())
            .expect("painted navigator");
        let start = layout.visible_rect.center();
        let finish = start + Vec2::new(18.0, 12.0);
        for events in [
            vec![egui::Event::PointerMoved(start)],
            vec![egui::Event::PointerMoved(start), button(start, true)],
            vec![egui::Event::PointerMoved(finish)],
            vec![button(finish, false)],
        ] {
            frame(&ctx, events, |ui, ctx| {
                assert!(app.handle_fs_navigator_input(ui, ctx, full_rect(), 0, None));
            });
        }
        assert_ne!(
            app.fs_pan,
            Vec2::ZERO,
            "navigator must actually move the page"
        );
        assert!(!app.fullscreen_navigator_interaction.is_active());
        result(&app)
    });
    assert_eq!(outcomes[0], outcomes[1]);
}

fn touch(id: u64, phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(834),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    }
}

#[test]
fn drag_bound_setting_preserves_two_finger_pan_through_the_real_touch_recognizer() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let outcomes = [false, true].map(|bound| {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, bound);
        let left = egui::pos2(250.0, 180.0);
        let right = egui::pos2(350.0, 180.0);
        let delta = Vec2::new(60.0, 35.0);
        pointer_frame(&mut app, &ctx, vec![]);
        for events in [
            vec![
                touch(1, egui::TouchPhase::Start, left),
                egui::Event::PointerMoved(left),
                button(left, true),
            ],
            vec![touch(2, egui::TouchPhase::Start, right)],
            vec![
                touch(1, egui::TouchPhase::Move, left + delta),
                touch(2, egui::TouchPhase::Move, right + delta),
                egui::Event::PointerMoved(left + delta),
            ],
        ] {
            assert!(pointer_frame(&mut app, &ctx, events).is_none());
        }
        assert_ne!(
            app.fs_pan,
            Vec2::ZERO,
            "raw two-contact input must reach pan"
        );
        assert!(
            (app.fs_zoom - 2.0).abs() < 0.0001,
            "translation keeps contact spacing"
        );
        let moved = result(&app);
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(1, egui::TouchPhase::End, left + delta),
                touch(2, egui::TouchPhase::End, right + delta),
                button(left + delta, false),
            ],
        );
        assert_eq!(
            result(&app),
            moved,
            "ending a pinch cannot add canvas movement"
        );
        moved
    });
    assert_eq!(outcomes[0], outcomes[1]);
}

#[derive(Clone, Copy, Debug)]
enum EditSurface {
    Erase,
    Conceal,
    Crop,
    Text,
    LocalAdjust,
    SnsSplit,
}

impl EditSurface {
    fn activate(self, app: &mut App) {
        match self {
            Self::Erase => app.erase_mode = true,
            Self::Conceal => app.conceal_mode = true,
            Self::Crop => app.export_crop_mode = true,
            Self::Text => app.text_mode = true,
            Self::LocalAdjust => app.local_adjust_mode = true,
            Self::SnsSplit => {
                app.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
                    crate::sns_split::SnsTarget::X,
                    2,
                    [800, 1200],
                ));
            }
        }
        assert!(app.is_overlay_edit_mode_active());
        assert!(!app.fs_in_page_scroll_surface());
    }
}

const EDIT_SURFACES: [EditSurface; 6] = [
    EditSurface::Erase,
    EditSurface::Conceal,
    EditSurface::Crop,
    EditSurface::Text,
    EditSurface::LocalAdjust,
    EditSurface::SnsSplit,
];

#[test]
fn drag_bound_setting_leaves_all_overlay_edit_canvas_drags_with_the_tools() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for surface in EDIT_SURFACES {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            surface.activate(&mut app);
            drag(&mut app, &ctx, Vec2::new(160.0, 80.0));
            assert_eq!(app.fs_pan, Vec2::ZERO, "plain drag belongs to {surface:?}");
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "surface={surface:?}");
    }
}

#[test]
fn drag_bound_setting_preserves_overlay_space_pan_handler_for_every_edit_mode() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for surface in EDIT_SURFACES {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            surface.activate(&mut app);
            let start = full_rect().center();
            let finish = start + Vec2::new(160.0, 80.0);
            for events in [
                vec![
                    egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerMoved(start),
                    button(start, true),
                ],
                vec![egui::Event::PointerMoved(finish)],
                vec![button(finish, false)],
            ] {
                frame(&ctx, events, |_, ctx| {
                    let (space, pressed, down, released, pos) = ctx.input(|i| {
                        (
                            i.key_down(egui::Key::Space),
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.pointer.primary_down(),
                            i.pointer.button_released(egui::PointerButton::Primary),
                            i.pointer.interact_pos(),
                        )
                    });
                    assert!(app.handle_overlay_space_pan_drag(
                        ctx,
                        space,
                        false,
                        full_rect().center(),
                        true,
                        pressed,
                        down,
                        released,
                        pos,
                    ));
                });
            }
            assert_ne!(app.fs_pan, Vec2::ZERO, "Space pan must move: {surface:?}");
            assert!(app.fs_pan_drag_start.is_none());
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "surface={surface:?}");
    }
}

#[test]
fn drag_bound_setting_preserves_adjustment_and_view_trim_tab_canvas_drags() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for tab in [
        FullscreenLeftPanelTab::Adjustment,
        FullscreenLeftPanelTab::ViewTrim,
    ] {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            app.settings.fullscreen_left_panel_tab = tab;
            app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle;
            drag(&mut app, &ctx, Vec2::new(160.0, 80.0));
            assert!(
                app.adjustment_mode.is_open(),
                "the tested tab must remain open"
            );
            assert_ne!(app.fs_pan, Vec2::ZERO);
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "tab={tab:?}");
    }
}

#[test]
fn drag_bound_setting_preserves_z_zoom_and_both_continuous_canvas_drag_routes() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for flow in [
        ReadingFlow::Paged,
        ReadingFlow::Vertical,
        ReadingFlow::Horizontal,
    ] {
        let outcomes = [false, true].map(|bound| {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, bound);
            app.reading_flow = flow;
            if flow.is_paged() {
                app.fs_zoom_active = true;
                app.fs_zoom_factor = 2.0;
            } else {
                assert!(app.continuous_reading_active_for_idx(0));
            }
            assert!(!app.fs_in_page_scroll_surface());
            drag(&mut app, &ctx, Vec2::new(120.0, 80.0));
            assert!(app.fs_pan != Vec2::ZERO || app.fs_vertical_scroll != 0.0);
            result(&app)
        });
        assert_eq!(outcomes[0], outcomes[1], "flow={flow:?}");
    }
}

#[test]
fn drag_bound_setting_preserves_free_rotated_single_page_canvas_drags() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let outcomes = [false, true].map(|bound| {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, bound);
        app.fs_free_rotation = 0.23;
        seed(&ctx, &mut app);
        let page = app.fullscreen_page_layout.displayed_pages().next().unwrap();
        assert!(page.transform.free_rotation_rad.abs() > f32::EPSILON);
        frame(&ctx, vec![], |_, ctx| {
            assert!(!app.fs_bounded_drag_pan_surface(ctx));
        });
        drag(&mut app, &ctx, Vec2::new(160.0, 80.0));
        assert_ne!(app.fs_pan, Vec2::ZERO);
        let first_single = result(&app);

        // The stored tilt survives a switch to a pair, whose production
        // renderer intentionally draws upright. Eligibility follows that
        // actual transform rather than the retained fs_free_rotation value.
        app.spread_mode = SpreadMode::Ltr;
        seed(&ctx, &mut app);
        assert_eq!(app.fs_free_rotation, 0.23);
        assert_eq!(app.fullscreen_page_layout.displayed_pages().count(), 2);
        assert!(
            app.fullscreen_page_layout
                .displayed_pages()
                .all(|page| { page.transform.free_rotation_rad.abs() <= f32::EPSILON })
        );
        frame(&ctx, vec![], |_, ctx| {
            assert!(app.fs_bounded_drag_pan_surface(ctx));
        });

        app.spread_mode = SpreadMode::Single;
        seed(&ctx, &mut app);
        assert!(
            app.fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform
                .free_rotation_rad
                .abs()
                > f32::EPSILON
        );
        frame(&ctx, vec![], |_, ctx| {
            assert!(!app.fs_bounded_drag_pan_surface(ctx));
        });
        let before = app.fs_pan;
        drag(&mut app, &ctx, Vec2::new(-80.0, 40.0));
        assert_ne!(app.fs_pan, before);
        (first_single, result(&app))
    });
    assert_eq!(outcomes[0], outcomes[1]);
}
