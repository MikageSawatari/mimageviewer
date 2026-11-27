//! Revision 2 observes the real viewer's ordinary zoom and pass-local lock lifetime.
use super::*;

fn mode(app: &App) -> PlaneFillMode {
    match &app.fs_exclusive_view {
        FsExclusiveView::PlaneView(state) => state.fill_mode,
        _ => panic!("PlaneView must remain active"),
    }
}
fn set_mode(app: &mut App, value: PlaneFillMode) {
    let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view else {
        panic!("PlaneView must remain active")
    };
    state.fill_mode = value;
}
fn snapshot(app: &App) -> Arc<crate::plane_view_wgpu::PlaneFrameSnapshot> {
    match &app.fs_exclusive_view {
        FsExclusiveView::PlaneView(state) => state.last_drawn.clone().expect("real plane paint"),
        _ => panic!("PlaneView must remain active"),
    }
}
fn ordinary_wheel(viewer: &mut RootDepthViewer, app: &mut App, delta: f32) {
    viewer.frame(
        app,
        vec![
            egui::Event::PointerMoved(egui::pos2(500., 300.)),
            wheel(0., delta, egui::Modifiers::CTRL),
        ],
    );
}
fn fisheye_wheel(viewer: &mut RootDepthViewer, app: &mut App, delta: f32) {
    viewer.frame(
        app,
        vec![wheel(
            0.,
            delta,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        )],
    );
}
struct ClearPhysicalInput;
impl Drop for ClearPhysicalInput {
    fn drop(&mut self) {
        crate::key_input::clear_test_synthetic_input();
    }
}
fn physical_modifiers(modifiers: crate::key_input::SyntheticModifiers) {
    // Same physical-level fixture as additional_coverage and the existing scrub tests.
    crate::key_input::clear_test_synthetic_input();
    crate::key_input::arm_test_synthetic_input(0xE919, egui::ViewportId::ROOT);
    let now = std::time::Instant::now();
    crate::key_input::enqueue_test_synthetic_command(crate::key_input::SyntheticKeyCommand::down(
        now,
        crate::key_input::SyntheticNavigationKey::Tab,
        modifiers,
    ));
    crate::key_input::advance_test_synthetic_input(now);
}
fn button(
    pos: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers,
    }
}
fn drag(
    viewer: &mut RootDepthViewer,
    app: &mut App,
    button_kind: egui::PointerButton,
    modifiers: egui::Modifiers,
    delta: egui::Vec2,
) {
    app.fs_focus_regained_at = None;
    let start = egui::pos2(500., 300.);
    viewer.frame(
        app,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(start),
            button(start, button_kind, true, modifiers),
        ],
    );
    viewer.frame(
        app,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(start + delta * 0.5),
        ],
    );
    viewer.frame(
        app,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(start + delta),
        ],
    );
    viewer.frame(
        app,
        vec![button(start + delta, button_kind, false, modifiers)],
    );
}
fn touch(id: u64, phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(919),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    }
}
fn pinch(viewer: &mut RootDepthViewer, app: &mut App) {
    app.settings.touch_still_chrome_learned = true;
    app.fs_focus_regained_at = None;
    let center = egui::pos2(500., 300.);
    for (phase, radius) in [
        (egui::TouchPhase::Start, 60.),
        (egui::TouchPhase::Move, 90.),
        (egui::TouchPhase::Move, 120.),
    ] {
        let first = center - egui::vec2(radius, 0.);
        let mut events = vec![touch(1, phase, first), egui::Event::PointerMoved(first)];
        if phase == egui::TouchPhase::Start {
            events.push(button(
                first,
                egui::PointerButton::Primary,
                true,
                egui::Modifiers::NONE,
            ));
        }
        events.push(touch(2, phase, center + egui::vec2(radius, 0.)));
        viewer.frame(app, events);
    }
    viewer.frame(
        app,
        vec![
            touch(1, egui::TouchPhase::End, center - egui::vec2(120., 0.)),
            button(
                center - egui::vec2(120., 0.),
                egui::PointerButton::Primary,
                false,
                egui::Modifiers::NONE,
            ),
            egui::Event::PointerGone,
            touch(2, egui::TouchPhase::End, center + egui::vec2(120., 0.)),
            egui::Event::PointerGone,
        ],
    );
}

#[derive(Clone, Copy, Debug)]
enum ZoomRoute {
    CtrlWheel,
    SettingWheel,
    Key,
    Middle,
    Scrub,
    Pinch,
    Gamepad,
}
fn zoom_route(viewer: &mut RootDepthViewer, app: &mut App, route: ZoomRoute) {
    match route {
        ZoomRoute::CtrlWheel => ordinary_wheel(viewer, app, 40.),
        ZoomRoute::SettingWheel => {
            app.settings.fullscreen_wheel_action = crate::settings::FullscreenWheelAction::Zoom;
            viewer.frame(
                app,
                vec![
                    egui::Event::PointerMoved(egui::pos2(500., 300.)),
                    wheel(0., 40., egui::Modifiers::NONE),
                ],
            );
        }
        ZoomRoute::Key => {
            app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsZoomIn = F15\n");
            viewer.frame(app, key(egui::Key::F15));
        }
        ZoomRoute::Middle => drag(
            viewer,
            app,
            egui::PointerButton::Middle,
            egui::Modifiers::NONE,
            egui::vec2(0., -80.),
        ),
        ZoomRoute::Scrub => {
            let _clear = ClearPhysicalInput;
            app.keymap =
                crate::keymap::Keymap::from_ini_str("[FsImage]\nFsScrubZoomHold = Ctrl+Tab\n");
            physical_modifiers(crate::key_input::SyntheticModifiers {
                ctrl: true,
                ..Default::default()
            });
            drag(
                viewer,
                app,
                egui::PointerButton::Primary,
                egui::Modifiers::CTRL,
                egui::vec2(80., 0.),
            );
        }
        ZoomRoute::Pinch => {
            crate::touch_correlation::with_gestures_disabled(false, || pinch(viewer, app))
        }
        ZoomRoute::Gamepad => viewer.frame_with_before(app, vec![], |app, _| {
            app.apply_gamepad_fullscreen_zoom(0.5, 0.25);
        }),
    }
}

#[test]
fn plane_revision2_every_ordinary_zoom_entry_locks_only_when_the_zoom_changes() {
    let _keys = crate::key_input::lock_test_input();
    for route in [
        ZoomRoute::CtrlWheel,
        ZoomRoute::SettingWheel,
        ZoomRoute::Key,
        ZoomRoute::Middle,
        ZoomRoute::Scrub,
        ZoomRoute::Pinch,
        ZoomRoute::Gamepad,
    ] {
        for limited in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            enter(&mut app, &mut viewer);
            if limited {
                app.fs_zoom = ZOOM_MAX;
                viewer.frame(&mut app, vec![]);
            }
            let before = snapshot(&app);
            let zoom = app.fs_zoom;
            assert_eq!(mode(&app), PlaneFillMode::Auto);
            zoom_route(&mut viewer, &mut app, route);
            if limited {
                assert_eq!(app.fs_zoom, zoom, "{route:?}");
                assert_eq!(mode(&app), PlaneFillMode::Auto, "{route:?}");
            } else {
                assert!(app.fs_zoom > zoom, "{route:?}");
                assert_eq!(
                    mode(&app),
                    PlaneFillMode::Locked(before.k_fill),
                    "{route:?}"
                );
                assert_eq!(snapshot(&app).k_fill, before.k_fill, "{route:?}");
            }
            assert_eq!(app.fullscreen_idx, Some(0));
        }
    }
}

#[test]
fn plane_revision2_ordinary_shrink_keeps_visible_distortion_and_second_zoom_keeps_the_lock() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    let before = snapshot(&app);
    let pose = app.plane_view_pose();
    ordinary_wheel(&mut viewer, &mut app, -480.);
    assert_eq!(app.plane_view_pose(), pose);
    assert_eq!(mode(&app), PlaneFillMode::Locked(before.k_fill));
    let shrunk = snapshot(&app);
    assert_eq!(shrunk.k_fill, before.k_fill);
    assert_eq!(shrunk.params.values[2][0], before.params.values[2][0]);
    assert!(shrunk.params.values[2][0] > 0.);
    let accepted = app.fullscreen_page_layout.plane_cover.unwrap();
    let drawn = *app.fullscreen_page_layout.displayed_pages().next().unwrap();
    let quad = plane_effective_quad(&drawn.transform, accepted.material).unwrap();
    let canvas = drawn.transform.viewport_rect;
    assert_eq!(
        plane_fill_coefficient(canvas, canvas.center(), quad, app.settings.plane_view_lens),
        0.
    );
    ordinary_wheel(&mut viewer, &mut app, 40.);
    assert_eq!(mode(&app), PlaneFillMode::Locked(before.k_fill));
    assert_eq!(snapshot(&app).k_fill, before.k_fill);
}

#[test]
fn plane_revision2_delayed_zoom_uses_the_candidate_captured_before_paint() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    let before = snapshot(&app);
    let old_zoom = app.fs_zoom;
    let mut retained = None;
    // Collect input once in this pass. A real viewer frame captures front input itself,
    // so collecting in frame_with_before would capture the same delivery twice.
    app.frame_counter += 1;
    viewer
        .ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewer.size)),
                time: Some(app.frame_counter as f64 / 60.),
                events: vec![
                    egui::Event::PointerMoved(egui::pos2(500., 300.)),
                    wheel(0., -40., egui::Modifiers::CTRL),
                    wheel(0., -40., egui::Modifiers::NONE),
                ],
                ..Default::default()
            },
            |ui| {
                let ctx = ui.ctx().clone();
                let (mut result, operation) =
                    crate::ui_fullscreen::wheel_frame_tests::collect_actual_merge(
                        &mut app, ui, false,
                    );
                let mut proposal = result
                    .deferred
                    .take()
                    .expect("mixed wheel input defers the zoom until after paint");
                assert_eq!(proposal.plane_fill_lock.unwrap().k_fill, before.k_fill);
                // The later UI owner consumes the unrelated page wheel. Claim only the zoom.
                let options = ctx.options(|o| o.input_options);
                let ordinary_page = ctx
                    .input(|i| crate::wheel_input::remaining_wheel_segments(i, options))
                    .into_iter()
                    .find(|s| !s.input.modifiers.ctrl)
                    .unwrap();
                crate::wheel_input::consume_wheel_segment(&ctx, ordinary_page);
                app.claim_fs_wheel_frame_after_ui(&mut proposal);
                retained = Some((proposal, operation));
            },
        )
        .drop_without_applying_deltas();
    assert!(
        Arc::ptr_eq(&snapshot(&app), &before),
        "collection precedes paint"
    );
    // The retained zoom has not been applied: this pass paints the previous state.
    viewer.frame(&mut app, vec![]);
    assert_eq!(app.fs_zoom, old_zoom);
    assert_eq!(mode(&app), PlaneFillMode::Auto);
    let pending_paint = snapshot(&app);
    assert!(
        !Arc::ptr_eq(&pending_paint, &before),
        "the viewer painted a new frame"
    );
    assert_eq!(pending_paint.params.values, before.params.values);
    assert_eq!(pending_paint.k_fill, before.k_fill);
    app.clear_plane_last_drawn();
    let (proposal, mut operation) = retained.unwrap();
    let result = app.apply_fs_wheel_frame_proposal(proposal, true, &mut operation);
    assert!(result.page_nav.is_none());
    assert!(app.fs_zoom < old_zoom);
    assert_eq!(mode(&app), PlaneFillMode::Locked(before.k_fill));
    let new_zoom = app.fs_zoom;
    viewer.frame(&mut app, vec![]);
    assert_eq!(app.fs_zoom, new_zoom);
    let applied_paint = snapshot(&app);
    assert_eq!(applied_paint.k_fill, before.k_fill);
    assert_eq!(
        applied_paint.params.values[2][0],
        before.params.values[2][0]
    );
    assert_ne!(
        applied_paint.params.values[3], pending_paint.params.values[3],
        "the next pass paints the changed image scale"
    );
}

#[test]
fn plane_revision2_inward_wheel_and_assigned_key_restore_auto_even_without_geometry_change() {
    let _keys = crate::key_input::lock_test_input();
    for keyboard in [false, true] {
        for missing_record in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let mut viewer = viewer(&mut app);
            enter(&mut app, &mut viewer);
            app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsFisheyeIn = F13\n");
            app.fs_zoom = ZOOM_MAX;
            app.fs_pan = egui::Vec2::ZERO;
            if let FsExclusiveView::PlaneView(state) = &mut app.fs_exclusive_view {
                state.pose.strength = 1.;
                state.pose.lens_center = egui::Vec2::splat(0.5);
            }
            set_mode(&mut app, PlaneFillMode::Locked(0.1));
            viewer.frame(&mut app, vec![]);
            let geometric = (app.fs_zoom, app.fs_pan, app.plane_view_pose());
            if missing_record {
                app.fullscreen_page_layout.plane_cover = None;
            }
            viewer.frame(
                &mut app,
                if keyboard {
                    key(egui::Key::F13)
                } else {
                    vec![wheel(
                        0.,
                        40.,
                        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    )]
                },
            );
            assert_eq!((app.fs_zoom, app.fs_pan, app.plane_view_pose()), geometric);
            assert_eq!(
                mode(&app),
                if missing_record {
                    PlaneFillMode::Locked(0.1)
                } else {
                    PlaneFillMode::Auto
                }
            );
        }
    }
}

#[test]
fn plane_revision2_locked_outward_input_scales_strength_and_inward_input_covers_again() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    fisheye_wheel(&mut viewer, &mut app, 40.);
    ordinary_wheel(&mut viewer, &mut app, -480.);
    let locked = mode(&app);
    let before = snapshot(&app);
    assert_eq!(app.plane_view_pose().unwrap().strength, 0.5);
    fisheye_wheel(&mut viewer, &mut app, -40.);
    assert_eq!(mode(&app), locked);
    assert_eq!(app.plane_view_pose().unwrap().strength, 0.25);
    assert!((snapshot(&app).params.values[2][0] * 2. - before.params.values[2][0]).abs() < 1.0e-6);
    fisheye_wheel(&mut viewer, &mut app, 40.);
    assert_eq!(mode(&app), PlaneFillMode::Auto);
    assert!(snapshot(&app).k_fill > 0.);
    let record = app.fullscreen_page_layout.plane_cover.unwrap();
    let drawn = app
        .fullscreen_page_layout
        .displayed_pages()
        .next()
        .unwrap()
        .transform;
    let quad = plane_effective_quad(&drawn, record.material).unwrap();
    assert!(
        plane_fill_coefficient(
            drawn.viewport_rect,
            drawn.viewport_rect.center(),
            quad,
            app.settings.plane_view_lens
        ) > 0.
    );
    ordinary_wheel(&mut viewer, &mut app, -40.);
    fisheye_wheel(&mut viewer, &mut app, -80.);
    assert!(!app.fs_exclusive_view.is_plane_view());
}

#[test]
fn plane_revision2_snapshot_absence_and_exit_reentry_discard_the_old_lock_candidate() {
    let _keys = crate::key_input::lock_test_input();
    for reenter in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        let zoom = app.fs_zoom;
        let events = if reenter {
            let mut inward = wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
            // Distinct delivered sources keep exit and re-entry as separate ordered segments.
            if let egui::Event::MouseWheel { source, .. } = &mut inward {
                *source = egui::MouseWheelSource::Wheel;
            }
            vec![
                wheel(0., -40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT),
                inward,
                wheel(0., -40., egui::Modifiers::CTRL),
            ]
        } else {
            vec![wheel(0., -40., egui::Modifiers::CTRL)]
        };
        viewer.frame_with_before(&mut app, events, |app, ctx| {
            if !reenter {
                app.clear_plane_last_drawn();
            } else {
                let options = ctx.options(|o| o.input_options);
                let segments =
                    ctx.input(|i| crate::wheel_input::remaining_wheel_segments(i, options));
                assert_eq!(
                    segments.len(),
                    3,
                    "exit and re-entry must not coalesce to a zero segment"
                );
                assert_eq!(segments[0].vertical_before_axis_mapping, -40.);
                assert_eq!(segments[1].vertical_before_axis_mapping, 40.);
                let input = wheel_input_for_current_view(app, ctx, ctx.content_rect(), None, -1.);
                let cover = input.cover;
                let exited = plane_view_wheel_transition(input);
                assert!(exited.pose.is_none());
                let reentered = plane_view_wheel_transition(
                    app.plane_view_wheel_input(
                        cover.unwrap().transform.viewport_rect,
                        None,
                        1.,
                        exited.zoom,
                        exited.pan,
                        exited.pose,
                        cover,
                    )
                    .unwrap(),
                );
                assert_eq!(reentered.pose.unwrap().strength, 0.25);
            }
        });
        assert_eq!(mode(&app), PlaneFillMode::Auto);
        assert_eq!(app.plane_view_pose().unwrap().strength, 0.25);
        assert!(app.fs_zoom < zoom || reenter);
    }
}

#[test]
fn plane_revision2_mixed_wheel_order_and_key_stage_determine_the_final_fill_mode() {
    let _keys = crate::key_input::lock_test_input();
    for inward_first in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        ordinary_wheel(&mut viewer, &mut app, -40.);
        let ordinary = wheel(0., -40., egui::Modifiers::CTRL);
        let inward = wheel(0., 40., egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
        let mut predicted = None;
        viewer.frame_with_before(
            &mut app,
            if inward_first {
                vec![inward, ordinary]
            } else {
                vec![ordinary, inward]
            },
            |app, ctx| {
                let options = ctx.options(|o| o.input_options);
                let actions = ctx
                    .input(|i| crate::wheel_input::remaining_wheel_segments(i, options))
                    .into_iter()
                    .map(|segment| FsWheelAction {
                        input: segment.input,
                        segment,
                        operation: None,
                        class: if segment.input.modifiers.shift {
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
                let canvas = app.fullscreen_media_rect(
                    ctx.content_rect(),
                    app.fullscreen_idx.unwrap(),
                    false,
                );
                predicted = Some(app.predict_fs_wheel_frame(ctx, canvas, &actions));
            },
        );
        let predicted = predicted.unwrap();
        assert_eq!(
            (app.fs_zoom, app.fs_pan, app.plane_view_pose().is_some()),
            (predicted.0, predicted.1, predicted.3)
        );
        assert_eq!(matches!(mode(&app), PlaneFillMode::Locked(_)), inward_first);
        viewer.frame(&mut app, vec![]);
        assert!(snapshot(&app).k_fill > 0.);

        app.keymap =
            crate::keymap::Keymap::from_ini_str("[FsImage]\nFsFisheyeIn = F13\nFsZoomOut = F14\n");
        let mut events = vec![wheel(0., -40., egui::Modifiers::CTRL)];
        events.extend(key(egui::Key::F13));
        viewer.frame(&mut app, events);
        assert!(
            matches!(mode(&app), PlaneFillMode::Locked(_)),
            "key stage restores Auto before wheel locks it"
        );
        let mut events = vec![wheel(
            0.,
            40.,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        )];
        events.extend(key(egui::Key::F14));
        viewer.frame(&mut app, events);
        assert_eq!(
            mode(&app),
            PlaneFillMode::Auto,
            "ordinary key precedes the inward wheel"
        );
    }
}

#[test]
fn plane_revision2_locked_zero_can_restore_auto_and_end_through_existing_inputs() {
    let _keys = crate::key_input::lock_test_input();
    for ending in [0, 1, 2] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        set_mode(&mut app, PlaneFillMode::Locked(0.));
        viewer.frame(&mut app, vec![]);
        assert_eq!(snapshot(&app).k_fill, 0.);
        assert_eq!(snapshot(&app).params.values[2][0], 0.);
        match ending {
            0 => {
                fisheye_wheel(&mut viewer, &mut app, 40.);
                assert_eq!(mode(&app), PlaneFillMode::Auto);
                assert!(snapshot(&app).k_fill > 0.);
            }
            1 => {
                fisheye_wheel(&mut viewer, &mut app, -40.);
                assert!(!app.fs_exclusive_view.is_plane_view());
            }
            _ => {
                app.fs_focus_regained_at = None;
                viewer.frame(
                    &mut app,
                    transition_and_click_tests::click(egui::pos2(500., 300.), true),
                );
                assert!(!app.fs_exclusive_view.is_plane_view());
                assert_eq!(app.fs_zoom, 1.);
            }
        }
    }
}

#[test]
fn plane_revision2_drags_rotation_and_resize_preserve_the_fill_owner() {
    let _keys = crate::key_input::lock_test_input();
    let _clear = ClearPhysicalInput;
    for locked in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = viewer(&mut app);
        enter(&mut app, &mut viewer);
        if locked {
            ordinary_wheel(&mut viewer, &mut app, -480.);
        }
        let original_mode = mode(&app);
        let before_pan = app.fs_pan;
        drag(
            &mut viewer,
            &mut app,
            egui::PointerButton::Primary,
            egui::Modifiers::NONE,
            egui::vec2(900., 0.),
        );
        assert_ne!(
            app.fs_pan, before_pan,
            "the actual primary drag moves the image"
        );
        assert_eq!(mode(&app), original_mode);
        if locked {
            assert!(snapshot(&app).params.values[2][0] > 0.);
        } else {
            assert_eq!(
                snapshot(&app).k_fill,
                0.,
                "Auto responds to an exposed image edge"
            );
        }
        let pan = app.fs_pan;
        let lens = app.plane_view_pose().unwrap().lens_center;
        physical_modifiers(crate::key_input::SyntheticModifiers {
            shift: true,
            ..Default::default()
        });
        drag(
            &mut viewer,
            &mut app,
            egui::PointerButton::Primary,
            egui::Modifiers::SHIFT,
            egui::vec2(60., 40.),
        );
        assert_ne!(app.plane_view_pose().unwrap().lens_center, lens);
        assert_eq!(
            app.fs_pan, pan,
            "Shift drag owns the lens rather than image pan"
        );
        assert_eq!(mode(&app), original_mode);
        physical_modifiers(crate::key_input::SyntheticModifiers {
            ctrl: true,
            ..Default::default()
        });
        let rotation = app.fs_free_rotation;
        drag(
            &mut viewer,
            &mut app,
            egui::PointerButton::Primary,
            egui::Modifiers::CTRL,
            egui::vec2(-100., 80.),
        );
        assert_ne!(app.fs_free_rotation, rotation);
        assert_eq!(mode(&app), original_mode);
        crate::key_input::clear_test_synthetic_input();
        viewer.size = egui::vec2(800., 700.);
        viewer.frame(
            &mut app,
            vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
        );
        assert_eq!(mode(&app), original_mode);
        assert!(snapshot(&app).k_fill.is_finite());
    }
}

#[test]
fn plane_revision2_candidates_reject_foreign_context_or_anchor_and_mounts_keep_separate_modes() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    let valid = app.plane_fill_lock_candidate().unwrap();
    let bad_context = PlaneFillLockCandidate {
        context: crate::app::ViewerContextId::for_test(917_202),
        ..valid
    };
    let bad_anchor = PlaneFillLockCandidate {
        anchor: PlaneViewAnchor {
            slice: crate::page_split::PageSlice::Left,
            ..valid.anchor
        },
        ..valid
    };
    for candidate in [bad_context, bad_anchor] {
        viewer.frame_with_before(&mut app, vec![], |app, _| {
            app.lock_plane_fill_for_zoom(2., 3., Some(candidate));
            assert_eq!(mode(app), PlaneFillMode::Auto);
        });
    }
    ordinary_wheel(&mut viewer, &mut app, -40.);
    let main_mode = mode(&app);
    let main = app.edit_request_owner_context();
    let child_items = app.items.clone();
    let child = app.push_window_context_for_test(&viewer.ctx, 917_203, |owner| {
        owner.items = child_items;
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
            strength: 0.25,
            lens_center: egui::Vec2::splat(0.5),
        };
        owner.set_fs_exclusive_view(
            FsExclusiveView::PlaneView(PlaneViewState::new(pose)),
            "revision2_child",
        );
    });
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(mode(owner), PlaneFillMode::Auto);
        assert!(!owner.plane_fill_lock_candidate_matches(valid));
        owner.lock_plane_fill_for_zoom(2., 3., Some(valid));
        assert_eq!(mode(owner), PlaneFillMode::Auto);
        set_mode(owner, PlaneFillMode::Locked(0.2));
    });
    assert_eq!(mode(&app), main_mode);
    app.with_window_context_for_test(main, |owner| assert_eq!(mode(owner), main_mode));
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(mode(owner), PlaneFillMode::Locked(0.2))
    });
    assert_eq!(mode(&app), main_mode);
}

#[cfg(windows)]
#[test]
fn plane_revision2_linked_f11_and_scoped_mount_preserve_locked_fill() {
    // Match the linked-viewer precedent: its thread-local viewport renderer dies with this thread.
    std::thread::spawn(|| {
        let _keys = crate::key_input::lock_test_input();
        let mut app = crate::app::setup_app_for_test();
        let mut viewer = RootDepthViewer::linked(&mut app);
        viewer.ready(&mut app);
        app.set_fs_exclusive_view(FsExclusiveView::None, "revision2_linked_plane");
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
        app.settings.fullscreen_wheel_zoom_step =
            crate::settings::FullscreenWheelZoomStep::Percent10;
        viewer.frame(&mut app, vec![]);
        enter(&mut app, &mut viewer);
        ordinary_wheel(&mut viewer, &mut app, -40.);
        let locked = mode(&app);
        assert!(matches!(locked, PlaneFillMode::Locked(_)));
        let pose = app.plane_view_pose();
        let context = app.edit_request_owner_context();
        app.with_window_context_for_test(context, |owner| assert_eq!(mode(owner), locked));
        app.window_visible = false; // Established linked F11 fixture: no native host or model dispatch.
        for borderless in [false, true] {
            app.detached_viewer_borderless_fullscreen = borderless;
            app.detached_viewer_borderless_transition = None;
            viewer.frame(&mut app, key(egui::Key::F11));
            assert!(app.detached_viewer_borderless_transition.is_some());
            assert_eq!(mode(&app), locked);
            assert_eq!(app.plane_view_pose(), pose);
            assert_eq!(
                app.viewer_presentation,
                crate::app::ViewerPresentation::DetachedWindow
            );
        }
    })
    .join()
    .unwrap();
}

#[test]
fn plane_revision2_a_tiny_gamepad_zoom_locks_even_when_the_existing_endpoint_returns_false() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    app.fs_zoom = ZOOM_MIN;
    viewer.frame(&mut app, vec![]);
    let before = snapshot(&app);
    viewer.frame_with_before(&mut app, vec![], |app, _| {
        let old_zoom = app.fs_zoom;
        assert!(!app.apply_gamepad_fullscreen_zoom(0.5, 1.0e-6));
        assert_ne!(app.fs_zoom, old_zoom);
        assert!((app.fs_zoom - old_zoom).abs() <= f32::EPSILON);
    });
    assert_eq!(mode(&app), PlaneFillMode::Locked(before.k_fill));
}

#[test]
fn plane_revision2_locked_capture_copy_and_export_keep_the_displayed_uniform() {
    let _keys = crate::key_input::lock_test_input();
    let mut app = crate::app::setup_app_for_test();
    let mut viewer = viewer(&mut app);
    enter(&mut app, &mut viewer);
    ordinary_wheel(&mut viewer, &mut app, -480.);
    let frozen = snapshot(&app);
    assert!(matches!(mode(&app), PlaneFillMode::Locked(_)));
    assert!(frozen.params.values[2][0] > 0.);
    let canvas_request = app
        .plane_frame_request(crate::plane_view_wgpu::PlaneOutputChoice::Canvas)
        .unwrap();
    let crate::capture::CapturePixelWork::PlaneView {
        request: capture, ..
    } = app.prepare_capture_pixel_work(&viewer.ctx, 0).unwrap()
    else {
        panic!("capture uses PlaneView")
    };
    let crate::export_dialog::ExportComposite::PlaneView(export) = app
        .prepare_export_dialog_target(&viewer.ctx, 0)
        .unwrap()
        .composite
    else {
        panic!("export uses PlaneView")
    };
    set_mode(&mut app, PlaneFillMode::Auto);
    app.fs_zoom = 1.;
    viewer.frame(&mut app, vec![]);
    assert_eq!(snapshot(&app).params.values[2][0], 0.);
    for request in [canvas_request, capture, export] {
        assert!(Arc::ptr_eq(&request.snapshot, &frozen));
        assert_eq!(request.snapshot.k_fill, frozen.k_fill);
        assert_eq!(
            request.snapshot.params.values[2][0],
            frozen.params.values[2][0]
        );
        assert_eq!(
            request.snapshot.params.for_output(request.size).values[2][0],
            frozen.params.values[2][0]
        );
        assert_eq!(
            request.color_image().unwrap().size,
            [request.size[0] as usize, request.size[1] as usize]
        );
    }
}
