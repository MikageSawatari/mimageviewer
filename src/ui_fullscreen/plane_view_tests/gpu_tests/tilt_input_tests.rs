//! Real pointer ownership, tilt-only lifetime, and the existing material/output boundaries.
use super::*;

struct ClearPhysicalInput;
impl Drop for ClearPhysicalInput {
    fn drop(&mut self) {
        crate::key_input::clear_test_synthetic_input();
    }
}
fn physical_modifiers(ctrl: bool, shift: bool) {
    physical_modifiers_in(egui::ViewportId::ROOT, ctrl, shift);
}
fn physical_modifiers_in(viewport: egui::ViewportId, ctrl: bool, shift: bool) {
    crate::key_input::clear_test_synthetic_input();
    crate::key_input::arm_test_synthetic_input(0xE920, viewport);
    let now = std::time::Instant::now();
    crate::key_input::enqueue_test_synthetic_command(crate::key_input::SyntheticKeyCommand::down(
        now,
        crate::key_input::SyntheticNavigationKey::Tab,
        crate::key_input::SyntheticModifiers {
            ctrl,
            shift,
            ..Default::default()
        },
    ));
    crate::key_input::advance_test_synthetic_input(now);
}
fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}
fn press(viewer: &mut RootDepthViewer, app: &mut App, pos: egui::Pos2) {
    app.fs_focus_regained_at = None;
    let viewport = if app.viewer_presentation == crate::app::ViewerPresentation::DetachedWindow {
        viewer.child_viewport()
    } else {
        egui::ViewportId::ROOT
    };
    physical_modifiers_in(viewport, true, true);
    viewer.frame(app, vec![egui::Event::PointerMoved(pos), button(pos, true)]);
}
fn move_to(viewer: &mut RootDepthViewer, app: &mut App, pos: egui::Pos2) {
    viewer.frame(app, vec![egui::Event::PointerMoved(pos)]);
}
fn release(viewer: &mut RootDepthViewer, app: &mut App, pos: egui::Pos2) {
    viewer.frame(app, vec![button(pos, false)]);
    assert!(!app.fs_canvas_drag.is_plane_drag());
}
fn tilt(app: &App) -> PlaneTilt {
    let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
        panic!("PlaneView remains active")
    };
    state.tilt
}
fn mode(app: &App) -> PlaneFillMode {
    let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
        panic!("PlaneView remains active")
    };
    state.fill_mode
}
fn snapshot(app: &App) -> Arc<crate::plane_view_wgpu::PlaneFrameSnapshot> {
    let FsExclusiveView::PlaneView(state) = &app.fs_exclusive_view else {
        panic!("PlaneView remains active")
    };
    state.last_drawn.clone().expect("real plane callback")
}
fn bind_reset(app: &mut App, key_name: &str) {
    app.keymap =
        crate::keymap::Keymap::from_ini_str(&format!("[FsImage]\nFsPlaneTiltReset = {key_name}\n"));
}
pub(super) fn enter_tilt(viewer: &mut RootDepthViewer, app: &mut App) {
    let _clear = ClearPhysicalInput;
    let start = egui::pos2(500., 300.);
    press(viewer, app, start);
    assert!(app.fs_canvas_drag.is_tilt());
    move_to(viewer, app, start + egui::vec2(70., 35.));
    assert!(app.fs_canvas_drag.is_tilt());
    assert_eq!(app.plane_view_pose().unwrap().strength, 0.);
    assert!(!tilt(app).is_identity());
    release(viewer, app, start + egui::vec2(70., 35.));
}
fn wheel_prediction(app: &mut App, ctx: &egui::Context) -> (f32, egui::Vec2, bool, bool) {
    let options = ctx.options(|o| o.input_options);
    let actions = ctx
        .input(|i| crate::wheel_input::remaining_wheel_segments(i, options))
        .into_iter()
        .map(|segment| FsWheelAction {
            input: segment.input,
            segment,
            operation: None,
            class: if segment.input.modifiers.ctrl && segment.input.modifiers.shift {
                FsWheelInPageClass::PlaneView
            } else {
                FsWheelInPageClass::Legacy
            },
            scroll_origin: None,
            page_owner: (
                app.items_generation,
                app.fullscreen_idx,
                app.fullscreen_page_slice,
            ),
            seek_hover: false,
        })
        .collect::<Vec<_>>();
    let idx = app.fullscreen_idx.unwrap();
    let canvas = app.fullscreen_media_rect(ctx.content_rect(), idx, false);
    app.predict_fs_wheel_frame(ctx, canvas, &actions)
}

#[test]
fn plane_tilt_press_owns_without_entry_and_first_tiny_move_publishes_without_losing_owner() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    let start = egui::pos2(500., 300.);
    let canvas = app
        .fullscreen_page_layout
        .displayed_pages()
        .next()
        .unwrap()
        .transform
        .viewport_rect;
    let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    press(&mut viewer, &mut app, start);
    assert!(!app.fs_exclusive_view.is_plane_view());
    assert!(app.fs_canvas_drag.is_tilt());
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
    move_to(&mut viewer, &mut app, start + egui::vec2(0.001, 0.));
    assert!(app.fs_canvas_drag.is_tilt());
    let pose = app.plane_view_pose().unwrap();
    assert_eq!(pose.strength, 0.);
    assert_eq!(pose.lens_center, (start - canvas.min) / canvas.size());
    assert_eq!(mode(&app), PlaneFillMode::Auto);
    assert_ne!(
        tilt(&app),
        PlaneTilt::default(),
        "the sub-identity-threshold movement is retained while held"
    );
    assert!(tilt(&app).is_identity());
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
    release(&mut viewer, &mut app, start + egui::vec2(0.001, 0.));
    assert!(!app.fs_exclusive_view.is_plane_view());
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "release does not become a page click"
    );
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
}

#[test]
fn plane_tilt_same_pass_press_release_claims_the_click_and_publishes_only_nonzero_rotation() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for moved in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        app.fs_focus_regained_at = None;
        physical_modifiers(true, true);
        let start = egui::pos2(500., 300.);
        let end = start
            + if moved {
                egui::vec2(55., 25.)
            } else {
                egui::Vec2::ZERO
            };
        let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        viewer.frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(start),
                button(start, true),
                egui::Event::PointerMoved(end),
                button(end, false),
            ],
        );
        assert_eq!(app.fs_exclusive_view.is_plane_view(), moved);
        assert!(!app.fs_canvas_drag.is_plane_drag());
        if moved {
            assert_eq!(app.plane_view_pose().unwrap().strength, 0.);
            assert!(!tilt(&app).is_identity());
        }
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "completed tilt admission cannot leak a PageSide click"
        );
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
    }
}

#[test]
fn plane_tilt_modal_front_ui_does_not_admit_the_canvas_press() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    app.show_about_dialog = true;
    viewer.frame(&mut app, vec![]);
    let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    press(&mut viewer, &mut app, egui::pos2(500., 300.));
    move_to(&mut viewer, &mut app, egui::pos2(560., 340.));
    release(&mut viewer, &mut app, egui::pos2(560., 340.));
    assert!(!app.fs_exclusive_view.is_plane_view());
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
}

#[test]
fn plane_tilt_roundtrip_keeps_zero_strength_through_identity_until_release() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    let start = egui::pos2(500., 300.);
    let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    press(&mut viewer, &mut app, start);
    for delta in [
        egui::vec2(60., 0.),
        egui::Vec2::ZERO,
        egui::vec2(-60., 0.),
        egui::Vec2::ZERO,
    ] {
        move_to(&mut viewer, &mut app, start + delta);
        assert!(app.fs_canvas_drag.is_tilt());
        assert_eq!(app.plane_view_pose().unwrap().strength, 0.);
        assert_eq!(tilt(&app).is_identity(), delta == egui::Vec2::ZERO);
    }
    let mut predicted = None;
    viewer.frame_with_before(
        &mut app,
        vec![wheel(
            0.,
            -40.,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        )],
        |app, ctx| {
            predicted = Some(wheel_prediction(app, ctx));
        },
    );
    assert!(
        predicted.unwrap().3,
        "identity tilt remains alive while its drag is owned"
    );
    assert!(app.fs_exclusive_view.is_plane_view());
    assert!(app.fs_canvas_drag.is_tilt());
    release(&mut viewer, &mut app, start);
    assert!(!app.fs_exclusive_view.is_plane_view());
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
fn plane_tilt_owner_ignores_modifier_changes_and_repress_starts_a_new_operation() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    let start = egui::pos2(500., 300.);
    let before = (app.fs_pan, app.fs_free_rotation);
    press(&mut viewer, &mut app, start);
    move_to(&mut viewer, &mut app, start + egui::vec2(30., 20.));
    let old = tilt(&app);
    physical_modifiers(false, false);
    viewer.frame(
        &mut app,
        vec![
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::PointerMoved(start + egui::vec2(50., 40.)),
        ],
    );
    assert!(app.fs_canvas_drag.is_tilt());
    assert_ne!(tilt(&app), old);
    assert_eq!((app.fs_pan, app.fs_free_rotation), before);
    let old = tilt(&app);
    let next = start + egui::vec2(80., 60.);
    physical_modifiers(true, true);
    viewer.frame(
        &mut app,
        vec![
            button(start + egui::vec2(50., 40.), false),
            egui::Event::PointerMoved(next),
            button(next, true),
        ],
    );
    assert!(app.fs_canvas_drag.is_tilt());
    assert_eq!(
        tilt(&app),
        old,
        "new press does not rotate from the previous operation's origin"
    );
    let FsCanvasDrag::PlaneTilt {
        start: new_start,
        previous,
        ..
    } = app.fs_canvas_drag
    else {
        panic!("new tilt owns the second press")
    };
    assert_eq!(new_start, next);
    assert_eq!(previous, next);
    move_to(&mut viewer, &mut app, next + egui::vec2(20., 10.));
    assert_ne!(tilt(&app), old);
    release(&mut viewer, &mut app, next + egui::vec2(20., 10.));
    assert_eq!((app.fs_pan, app.fs_free_rotation), before);
}

#[test]
fn plane_tilt_focus_modal_and_page_changes_retire_only_the_owning_drag() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for route in [0, 1, 2] {
        for moved in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            press(&mut viewer, &mut app, egui::pos2(500., 300.));
            if moved {
                move_to(&mut viewer, &mut app, egui::pos2(540., 320.));
            }
            let before = moved.then(|| tilt(&app));
            match route {
                0 => viewer.frame_with_before(&mut app, vec![], |_, ctx| {
                    ctx.input_mut(|i| {
                        i.raw
                            .viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .focused = Some(false)
                    });
                }),
                1 => {
                    app.show_about_dialog = true;
                    viewer.frame(&mut app, vec![]);
                }
                _ => {
                    app.fullscreen_idx = Some(3);
                    viewer.frame(&mut app, vec![]);
                }
            }
            assert!(!app.fs_canvas_drag.is_plane_drag());
            if route < 2 {
                assert_eq!(app.plane_view_pose().map(|_| tilt(&app)), before);
            } else {
                assert!(!app.fs_exclusive_view.is_plane_view());
            }
        }
    }
}

#[test]
fn plane_tilt_ineligible_animation_keeps_free_rotation_and_spread_keeps_its_no_rotation_rule() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for spread in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        if spread {
            // Same guaranteed double-spread fixture as depth_main_window_root_predraw_rechecks_eligibility.
            app.items[1] = GridItem::Image("tilt-spread-fixture.jpg".into());
            crate::app::source_texture_test_support::seed(&mut app, &viewer.ctx, 1);
            app.spread_mode = crate::settings::SpreadMode::Ltr;
            assert!(matches!(
                app.resolve_visible_spread_pair(0),
                SpreadPair::Double { .. }
            ));
            assert_eq!(
                app.plane_view_entry_allowed(0),
                Err(PlaneViewEntryBlock::Spread)
            );
        } else {
            let Some(FsCacheEntry::Static { animation, .. }) = app.fs_cache.get_mut(&0) else {
                panic!("still fixture")
            };
            *animation = crate::fs_animation::StaticAnimationState::FirstFrameOnly(
                crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
            );
            assert_eq!(
                app.plane_view_entry_allowed(0),
                Err(PlaneViewEntryBlock::Animation)
            );
        }
        app.fs_focus_regained_at = None;
        physical_modifiers(true, true);
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let start = egui::pos2(500., 300.);
        let before = app.fs_free_rotation;
        viewer.frame(
            &mut app,
            vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                },
            ],
        );
        for delta in [egui::vec2(30., 20.), egui::vec2(-60., 80.)] {
            viewer.frame(
                &mut app,
                vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::PointerMoved(start + delta),
                ],
            );
            assert!(!app.fs_canvas_drag.is_plane_drag());
            assert!(!app.fs_exclusive_view.is_plane_view());
        }
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: start + egui::vec2(-60., 80.),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
        );
        if spread {
            assert_eq!(app.fs_free_rotation, before);
        } else {
            assert!((app.fs_free_rotation - before).abs() > 1.0e-4);
        }
    }
}

#[test]
fn plane_tilt_assigned_reset_is_remappable_and_resets_held_tilt_immediately() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for strength_positive in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        bind_reset(&mut app, "F15");
        let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        viewer.frame(&mut app, key(egui::Key::F15));
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
        if strength_positive {
            enter(&mut app, &mut viewer);
        }
        let start = egui::pos2(500., 300.);
        press(&mut viewer, &mut app, start);
        move_to(&mut viewer, &mut app, start + egui::vec2(55., 25.));
        let tilted = tilt(&app);
        let before = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        let fill_mode = mode(&app);
        bind_reset(&mut app, "F16");
        viewer.frame(&mut app, key(egui::Key::F15));
        assert_eq!(
            tilt(&app),
            tilted,
            "the former binding is no longer the reset action"
        );
        viewer.frame(&mut app, key(egui::Key::F16));
        assert!(
            !app.fs_canvas_drag.is_plane_drag(),
            "reset is effective before physical release"
        );
        if strength_positive {
            assert_eq!(app.plane_view_pose().unwrap().strength, 0.25);
            assert_eq!(tilt(&app), PlaneTilt::default());
            assert_eq!(mode(&app), fill_mode);
        } else {
            assert!(!app.fs_exclusive_view.is_plane_view());
        }
        move_to(&mut viewer, &mut app, start + egui::vec2(75., 45.));
        release(&mut viewer, &mut app, start + egui::vec2(75., 45.));
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), before);
        assert_eq!(app.fullscreen_idx, Some(0));
    }
}

#[test]
fn plane_tilt_reset_key_then_wheel_and_click_use_the_same_predicted_end_condition() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for positive in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        if positive {
            enter(&mut app, &mut viewer);
        }
        let start = egui::pos2(500., 300.);
        press(&mut viewer, &mut app, start);
        move_to(&mut viewer, &mut app, start + egui::vec2(50., 25.));
        bind_reset(&mut app, "F15");
        let mut events = key(egui::Key::F15);
        events.push(wheel(
            0.,
            if positive { -40. } else { 40. },
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        ));
        // Release of the old tilt must not become an ordinary click after reset ends it.
        events.push(button(start + egui::vec2(50., 25.), false));
        let mut predicted = None;
        viewer.frame_with_before(&mut app, events, |app, ctx| {
            assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
            assert!(!app.fs_canvas_drag.is_plane_drag());
            if positive {
                assert!(tilt(app).is_identity());
            } else {
                assert!(!app.fs_exclusive_view.is_plane_view());
            }
            predicted = Some(wheel_prediction(app, ctx));
        });
        let predicted = predicted.unwrap();
        assert_eq!(
            (app.fs_zoom, app.fs_pan, app.plane_view_pose().is_some()),
            (predicted.0, predicted.1, predicted.3)
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.fs_free_rotation, 0.);
        if app.fs_exclusive_view.is_plane_view() {
            assert_eq!(tilt(&app), PlaneTilt::default());
        }
    }
}

#[test]
fn plane_tilt_survives_strength_zero_wheel_prediction_then_reset_ends_it() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    let start = egui::pos2(500., 300.);
    press(&mut viewer, &mut app, start);
    move_to(&mut viewer, &mut app, start + egui::vec2(60., 30.));
    release(&mut viewer, &mut app, start + egui::vec2(60., 30.));
    let tilted = tilt(&app);
    let mut predicted = None;
    viewer.frame_with_before(
        &mut app,
        vec![wheel(
            0.,
            -80.,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        )],
        |app, ctx| {
            predicted = Some(wheel_prediction(app, ctx));
        },
    );
    let predicted = predicted.unwrap();
    assert!(predicted.3);
    assert_eq!((app.fs_zoom, app.fs_pan), (predicted.0, predicted.1));
    assert_eq!(app.plane_view_pose().unwrap().strength, 0.);
    assert_eq!(tilt(&app), tilted);
    assert!(!app.fs_canvas_drag.is_plane_drag());
    bind_reset(&mut app, "F17");
    viewer.frame(&mut app, key(egui::Key::F17));
    assert!(!app.fs_exclusive_view.is_plane_view());
}

#[test]
fn plane_tilt_preserves_auto_or_locked_fill_and_ordinary_zoom_rules() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for locked in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        if locked {
            viewer.frame(&mut app, vec![wheel(0., -40., egui::Modifiers::CTRL)]);
        }
        let before_mode = mode(&app);
        let before = snapshot(&app);
        let zoom_pan = (app.fs_zoom, app.fs_pan);
        let start = egui::pos2(500., 300.);
        press(&mut viewer, &mut app, start);
        move_to(&mut viewer, &mut app, start + egui::vec2(80., 40.));
        release(&mut viewer, &mut app, start + egui::vec2(80., 40.));
        assert_eq!(mode(&app), before_mode);
        assert_eq!(snapshot(&app).k_fill, before.k_fill);
        assert_eq!(
            (app.fs_zoom, app.fs_pan),
            zoom_pan,
            "tilting never expands the board to hide margins"
        );
        let tilted = tilt(&app);
        crate::key_input::clear_test_synthetic_input();
        viewer.frame(&mut app, vec![wheel(0., -80., egui::Modifiers::CTRL)]);
        assert!(matches!(mode(&app), PlaneFillMode::Locked(_)));
        assert_eq!(tilt(&app), tilted);
        viewer.frame(
            &mut app,
            vec![wheel(
                0.,
                40.,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(mode(&app), PlaneFillMode::Auto);
        assert_eq!(tilt(&app), tilted);
        // The ordinary primary drag keeps the same tilt and fill policy.
        app.fs_focus_regained_at = None;
        viewer.frame(
            &mut app,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        for delta in [egui::vec2(40., 20.), egui::vec2(80., 40.)] {
            move_to(&mut viewer, &mut app, start + delta);
        }
        release(&mut viewer, &mut app, start + egui::vec2(80., 40.));
        assert_eq!(tilt(&app), tilted);
        assert_eq!(mode(&app), PlaneFillMode::Auto);
    }
}

#[test]
fn plane_tilt_double_click_and_real_navigation_remove_both_pose_and_drag() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for double in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter_tilt(&mut viewer, &mut app);
        app.fs_focus_regained_at = None;
        app.frame_counter += 60;
        let events = if double {
            transition_and_click_tests::click(egui::pos2(500., 300.), true)
        } else {
            vec![wheel(0., -40., egui::Modifiers::NONE)]
        };
        viewer.frame(&mut app, events);
        if !double {
            viewer.frame(&mut app, vec![]);
        }
        assert!(!app.fs_exclusive_view.is_plane_view());
        assert!(!app.fs_canvas_drag.is_plane_drag());
        assert_eq!(app.fullscreen_idx, Some(if double { 0 } else { 1 }));
    }
}

#[test]
fn plane_tilt_double_click_ends_the_projection_even_while_tilt_owns_the_primary_stream() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    let start = egui::pos2(500., 300.);
    press(&mut viewer, &mut app, start);
    move_to(&mut viewer, &mut app, start + egui::vec2(60., 30.));
    assert!(app.fs_canvas_drag.is_tilt());
    assert!(!tilt(&app).is_identity());
    let mut events = vec![button(start + egui::vec2(60., 30.), false)];
    events.extend(transition_and_click_tests::click(start, true));
    viewer.frame(&mut app, events);
    assert!(!app.fs_exclusive_view.is_plane_view());
    assert!(!app.fs_canvas_drag.is_plane_drag());
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
        (1., egui::Vec2::ZERO, 0.)
    );
}

#[test]
fn plane_tilt_context_mounts_keep_independent_quaternions_and_drag_owners() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter_tilt(&mut viewer, &mut app);
    let original = tilt(&app);
    let main = app.edit_request_owner_context();
    let items = app.items.clone();
    let child_tilt = PlaneTilt::from_rotation_vector([0.5, -0.3, 0.2]);
    let child = app.push_window_context_for_test(&viewer.ctx, 919_201, |owner| {
        owner.items = items;
        owner.image_metas = vec![None; owner.items.len()];
        owner.thumbnails = (0..owner.items.len())
            .map(|_| crate::grid_item::ThumbnailState::Pending)
            .collect();
        owner.rebuild_visible_indices();
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        let pose = PlaneViewPose {
            anchor: owner.plane_view_anchor(0),
            strength: 0.,
            lens_center: egui::Vec2::splat(0.5),
        };
        let mut state = PlaneViewState::new(pose);
        state.tilt = child_tilt;
        owner.set_fs_exclusive_view(FsExclusiveView::PlaneView(state), "tilt_child_fixture");
        owner.fs_canvas_drag = FsCanvasDrag::PlaneTilt {
            anchor: pose.anchor,
            start: egui::pos2(300., 200.),
            previous: egui::pos2(330., 220.),
        };
    });
    assert_eq!(tilt(&app), original);
    assert!(!app.fs_canvas_drag.is_plane_drag());
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(tilt(owner), child_tilt);
        assert!(owner.fs_canvas_drag.is_tilt());
    });
    app.with_window_context_for_test(main, |owner| {
        assert_eq!(tilt(owner), original);
        assert!(!owner.fs_canvas_drag.is_plane_drag());
    });
    app.with_window_context_for_test(child, |owner| assert_eq!(tilt(owner), child_tilt));
    assert_eq!(tilt(&app), original);
}

#[cfg(windows)]
#[test]
fn plane_tilt_linked_f11_and_scoped_mount_preserve_quaternion() {
    std::thread::spawn(|| {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::linked(&mut app);
        viewer.ready(&mut app);
        app.set_fs_exclusive_view(FsExclusiveView::None, "linked_tilt_fixture");
        app.settings.fullscreen_navigator_visible = false;
        viewer.frame(&mut app, vec![]);
        enter_tilt(&mut viewer, &mut app);
        let original = tilt(&app);
        let pose = app.plane_view_pose();
        let context = app.edit_request_owner_context();
        app.with_window_context_for_test(context, |owner| assert_eq!(tilt(owner), original));
        app.window_visible = false;
        for borderless in [false, true] {
            app.detached_viewer_borderless_fullscreen = borderless;
            app.detached_viewer_borderless_transition = None;
            viewer.frame(&mut app, key(egui::Key::F11));
            assert!(app.detached_viewer_borderless_transition.is_some());
            assert_eq!(tilt(&app), original);
            assert_eq!(app.plane_view_pose(), pose);
        }
    })
    .join()
    .unwrap();
}

#[test]
fn plane_tilt_front_and_back_snapshots_capture_and_export_the_same_board() {
    let _keys = crate::key_input::lock_test_input();
    for back in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        // RootDepthViewer replaces the original white depth fixture with this canonical source.
        let source_rgba = [60, 120, 180, 255];
        let Some(FsCacheEntry::Static { pixels, .. }) = app.fs_cache.get(&0) else {
            panic!("canonical still source")
        };
        assert!(
            pixels
                .pixels
                .iter()
                .all(|pixel| pixel.to_array() == source_rgba)
        );
        enter_tilt(&mut viewer, &mut app);
        let orientation = if back {
            PlaneTilt {
                q: [0., 0., 1., 0.],
            }
        } else {
            PlaneTilt::from_rotation_vector([0.3, -0.5, 0.1])
        };
        let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view else {
            panic!("tilt-only fixture")
        };
        state.tilt = orientation;
        state.pose.lens_center = egui::Vec2::splat(0.5);
        viewer.frame(&mut app, vec![]);
        let frozen = snapshot(&app);
        let matrix = orientation.matrix();
        for (column, expected) in matrix.into_iter().enumerate() {
            assert_eq!(&frozen.params.values[13 + column][..3], &expected);
        }
        assert_eq!(
            frozen.params.values[2][0], 0.,
            "tilt-only output uses perspective without fisheye"
        );
        if back {
            assert!(matrix[0][0] < 0. && matrix[2][2] < 0.);
        }
        let drawn = app
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform;
        let canvas = drawn.viewport_rect;
        let pose = app.plane_view_pose().unwrap();
        let unit = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        assert!(drawn.rotation.is_none());
        assert_eq!(drawn.free_rotation_rad, 0.);
        assert_eq!(drawn.paint_rect, drawn.full_image_rect);
        assert_eq!(drawn.uv_rect, unit);
        assert_eq!(frozen.material.description.coverage, unit);
        assert_eq!(frozen.material.description.sampler_roi, unit);
        let crate::capture::CapturePixelWork::PlaneView {
            request: capture, ..
        } = app.prepare_capture_pixel_work(&viewer.ctx, 0).unwrap()
        else {
            panic!("tilt capture")
        };
        let crate::export_dialog::ExportComposite::PlaneView(export) = app
            .prepare_export_dialog_target(&viewer.ctx, 0)
            .unwrap()
            .composite
        else {
            panic!("tilt export")
        };
        let copy = app
            .plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
            .unwrap();
        app.reset_plane_tilt(&viewer.ctx);
        assert!(!app.fs_exclusive_view.is_plane_view());
        for request in [copy, capture, export] {
            assert!(Arc::ptr_eq(&request.snapshot, &frozen));
            let rgba = request.render().unwrap();
            let [w, h] = request.size;
            let mut inside_pixels = 0;
            let mut outside_pixels = 0;
            for (fx, fy) in [(0.2, 0.2), (0.4, 0.4), (0.5, 0.5), (0.6, 0.6), (0.8, 0.8)] {
                let x = (fx * w as f32) as u32;
                let y = (fy * h as f32) as u32;
                let ppp = drawn.pixels_per_point;
                let p = canvas.min * ppp
                    + egui::vec2(
                        (x as f32 + 0.5) * canvas.width() * ppp / w as f32,
                        (y as f32 + 0.5) * canvas.height() * ppp / h as f32,
                    );
                // Invert the forward pinhole homography independently of camera_point's
                // ray/plane intersection. k=0 here, and the uncropped paint rectangle is Q.
                let physical_canvas = canvas * ppp;
                let c = physical_canvas.min + pose.lens_center * physical_canvas.size();
                let v = p - c;
                let focal = 0.5
                    * f64::from(physical_canvas.width()).hypot(f64::from(physical_canvas.height()))
                    / 25_f64.to_radians().tan();
                let [nx, ny, n] = orientation.matrix().map(|column| column.map(f64::from));
                let [vx, vy] = [f64::from(v.x), f64::from(v.y)];
                let den = n[0] * vx + n[1] * vy + n[2] * focal;
                let epsilon = 1.0e-6 * (vx * vx + vy * vy + focal * focal).sqrt();
                let analytic = if n[2].abs() <= 1.0e-6 || den.abs() <= epsilon {
                    None
                } else {
                    let a = nx[0] - vx / focal * nx[2];
                    let b = ny[0] - vx / focal * ny[2];
                    let c_y = nx[1] - vy / focal * nx[2];
                    let d = ny[1] - vy / focal * ny[2];
                    let determinant = a * d - b * c_y;
                    let x = (vx * d - b * vy) / determinant;
                    let y = (a * vy - vx * c_y) / determinant;
                    let depth = focal + nx[2] * x + ny[2] * y;
                    (depth > 0.).then(|| c + egui::vec2(x as f32, y as f32))
                };
                let mapped = camera_point(
                    p,
                    canvas * ppp,
                    pose.lens_center,
                    0.,
                    app.settings.plane_view_lens,
                    orientation,
                );
                assert_eq!(
                    mapped.is_some(),
                    analytic.is_some(),
                    "back={back}, pixel=({x},{y})"
                );
                if let (Some(mapped), Some(analytic)) = (mapped, analytic) {
                    assert!(
                        (mapped - analytic).length() <= 1. / 64.,
                        "independent pinhole inverse, back={back}, pixel=({x},{y})"
                    );
                }
                let expected_inside = analytic.is_some_and(|q| drawn.paint_rect.contains(q / ppp));
                let source_uv = mapped.and_then(|q| {
                    texture_point(
                        q / ppp,
                        &drawn,
                        frozen.material.description.coverage,
                        frozen.material.description.sampler_roi,
                    )
                });
                assert_eq!(
                    source_uv.is_some(),
                    expected_inside,
                    "material support, back={back}, pixel=({x},{y})"
                );
                let pixel = &rgba[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
                if expected_inside {
                    inside_pixels += 1;
                    assert_eq!(
                        pixel, &source_rgba,
                        "inside the projected board, back={back}, pixel=({x},{y})"
                    );
                } else {
                    outside_pixels += 1;
                    let bg = app.settings.fullscreen_image_margin_color;
                    assert_eq!(pixel, &[bg[0], bg[1], bg[2], 255]);
                }
            }
            assert!(
                inside_pixels > 0 && outside_pixels > 0,
                "both the board and margin must be sampled"
            );
        }
    }
}
