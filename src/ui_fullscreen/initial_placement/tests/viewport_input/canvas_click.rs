use super::*;
use crate::app::HistoryTrigger;

fn click_pos() -> egui::Pos2 {
    egui::pos2(450.0, 200.0)
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

// Use the existing reset tests' isolated backing handler, its real navigation
// consumer, and the actual initial-placement paint owner.
fn paint_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    app.frame_counter += 1;
    crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), Vec::new());
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(arrival_viewport()),
            focused: true,
            time: Some(app.frame_counter as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            crate::keyboard_input::cache_keyboard_owner(
                ctx,
                crate::keyboard_input::KeyboardOwner::ApplicationShortcut {
                    scope: crate::keyboard_input::ShortcutScope::new(
                        ctx.viewport_id(),
                        crate::keyboard_input::ShortcutSurface::Fullscreen,
                    ),
                },
            );
            let idx = app.fullscreen_idx.unwrap();
            let keys = app.handle_fs_key_input(ctx, idx, false);
            let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
            let (pointer_nav, close) = app.handle_fs_wheel_and_click(
                ui,
                ctx,
                arrival_viewport(),
                &state,
                false,
                0,
                &mut None,
            );
            let nav = if pointer_nav.is_none() {
                keys.page_nav
            } else {
                pointer_nav.into()
            };
            app.handle_fs_navigation(
                ctx,
                close || keys.close,
                keys.close_to_page_list,
                keys.ctrl_nav,
                keys.sibling_nav,
                keys.mouse_nav,
                nav,
                keys.jump_to,
                idx,
            );
            let idx = app.fullscreen_idx.unwrap();
            app.reconcile_fullscreen_page_slice(idx);
            app.fs_page_turn_decision_for_frame(ctx, idx);
            assert!(app.fs_nav_holdover_for_draw().is_none());
            paint_single(
                app,
                ui,
                ctx,
                arrival_viewport(),
                true,
                crate::rotation_db::Rotation::None,
                None,
                SingletonSpreadPlacement::Center,
            );
            // The resident singleton has been painted. Seal its presentation through
            // the existing test boundary so the next navigation is admitted.
            app.observe_fs_navigation_pages_for_test(&[idx]);
            assert!(!app.fs_navigation_sequence_blocks_new_target());
        },
    )
    .drop_without_applying_deltas();
}

fn prepared(reading_start: bool) -> (egui::Context, crate::app::AppTestEnvForTest) {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single, false);
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.settings.fullscreen_fit_mode = if reading_start {
        FullscreenFitMode::Width
    } else {
        FullscreenFitMode::Page
    };
    app.settings.fullscreen_page_start_position = if reading_start {
        FullscreenPageStartPosition::ReadingStart
    } else {
        FullscreenPageStartPosition::Center
    };
    app.request_fs_initial_placement(0);
    paint_frame(&mut app, &ctx, vec![]);
    paint_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(click_pos())]);
    assert!(app.fs_initial_placement_request.is_none());
    (ctx, app)
}

fn click(app: &mut App, ctx: &egui::Context) {
    paint_frame(app, ctx, vec![button(click_pos(), true)]);
    paint_frame(app, ctx, vec![button(click_pos(), false)]);
}

fn drag(app: &mut App, ctx: &egui::Context, delta: egui::Vec2) {
    let end = click_pos() + delta;
    paint_frame(app, ctx, vec![button(click_pos(), true)]);
    paint_frame(app, ctx, vec![egui::Event::PointerMoved(end)]);
    paint_frame(app, ctx, vec![button(end, false)]);
    paint_frame(app, ctx, vec![egui::Event::PointerMoved(click_pos())]);
}

fn planned_click(app: &mut App, is_video: bool, double: bool) -> FsCanvasClickAction {
    let ctx = egui::Context::default();
    app.fs_front_pointer = Default::default();
    let mut action = FsCanvasClickAction::None;
    for frame in 0..if double { 5 } else { 3 } {
        let mut events = vec![egui::Event::PointerMoved(click_pos())];
        if frame > 0 {
            events.push(button(click_pos(), frame % 2 == 1));
        }
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(arrival_viewport()),
                time: Some(frame as f64 * 0.05),
                events,
                ..Default::default()
            },
            |ui| {
                let (front, _) = app.capture_fs_front_pointer_input(&ctx, arrival_viewport());
                let response = ui.interact(
                    arrival_viewport(),
                    egui::Id::unique("canvas-move-745"),
                    egui::Sense::click_and_drag(),
                );
                app.fs_front_pointer
                    .accept_backing_click(&response, false, &front);
                action = app.plan_fs_canvas_click(
                    &ctx,
                    &response,
                    arrival_viewport(),
                    StillSeekGeometry::bar_only(),
                    is_video,
                    false,
                    !app.reading_flow.is_paged(),
                    false,
                    None,
                    false,
                    app.fs_zoom,
                    app.fs_pan,
                    None,
                    FullscreenPrimarySuppressionDecision::Allow,
                    false,
                );
            },
        )
        .drop_without_applying_deltas();
    }
    action
}

#[test]
fn canvas_move_745_fit_drag_click_stays_double_click_centers_then_click_turns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(false);
    drag(&mut app, &ctx, egui::vec2(40.0, 20.0));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(app.fs_pan.length_sq() > PAN_EPSILON_SQ);
    click(&mut app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "first release must stay on the moved page"
    );
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
        (1.0, egui::Vec2::ZERO, 0.0)
    );
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn canvas_move_745_zoomed_out_click_stays_and_double_click_resets() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(false);
    assert!(app.apply_touch_zoom_factor_about_pivot(0.8, click_pos(), arrival_viewport().center()));
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!((app.fs_zoom, app.fs_pan), (1.0, egui::Vec2::ZERO));
}

#[test]
fn canvas_move_745_automatic_reading_start_double_click_turns_two_pages() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(true);
    assert!(app.fs_pan.length_sq() > PAN_EPSILON_SQ);
    click(&mut app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(1),
        "first release turns immediately"
    );
    paint_frame(&mut app, &ctx, vec![]);
    assert!(app.fs_pan.length_sq() > PAN_EPSILON_SQ);
    click(&mut app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(2),
        "the second release must not reset the landed page"
    );
}

#[test]
fn canvas_move_745_user_drag_after_automatic_placement_stops_click_turns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(true);
    let automatic_pan = app.fs_pan;
    drag(&mut app, &ctx, egui::vec2(40.0, 20.0));
    assert_ne!(app.fs_pan, automatic_pan);
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
fn canvas_move_745_unmoved_fit_click_is_immediate_and_double_click_turns_two_pages() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(false);
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(1));
    click(&mut app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(2),
        "pan={:?} response={:?}",
        app.fs_pan,
        ctx.read_response(egui::Id::unique("fs_click"))
    );
}

#[test]
fn canvas_move_745_zoomed_in_and_rotated_clicks_stay_and_double_click_resets() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    for rotation in [false, true] {
        let (ctx, mut app) = prepared(false);
        if rotation {
            app.fs_free_rotation = 0.35;
        } else {
            assert!(app.apply_touch_zoom_factor_about_pivot(
                1.25,
                click_pos(),
                arrival_viewport().center()
            ));
        }
        click(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        click(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(
            (app.fs_zoom, app.fs_pan, app.fs_free_rotation),
            (1.0, egui::Vec2::ZERO, 0.0)
        );
    }
}

#[test]
fn canvas_move_745_motion_at_click_threshold_still_turns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    for distance in [0.0, 2.0, 3.0] {
        let (ctx, mut app) = prepared(false);
        drag(&mut app, &ctx, egui::vec2(distance, 0.0));
        assert_eq!(app.fullscreen_idx, Some(1), "distance={distance}");
    }
}

#[test]
fn canvas_move_745_key_or_wheel_page_entry_reenables_click_turns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    for wheel in [false, true] {
        let (ctx, mut app) = prepared(false);
        drag(&mut app, &ctx, egui::vec2(40.0, 20.0));
        assert!(app.fs_pan.length_sq() > PAN_EPSILON_SQ);
        let events = if wheel {
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            }]
        } else {
            vec![key_event(egui::Key::ArrowRight, true)]
        };
        paint_frame(&mut app, &ctx, events);
        assert_eq!(app.fullscreen_idx, Some(1));
        paint_frame(
            &mut app,
            &ctx,
            vec![key_event(egui::Key::ArrowRight, false)],
        );
        assert_eq!((app.fs_zoom, app.fs_pan), (1.0, egui::Vec2::ZERO));
        click(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(2));
    }
}

#[test]
fn canvas_move_745_mounted_user_pan_does_not_block_sibling_automatic_placement() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(true);
    let original_pan = app.fs_pan;
    let child = app.build_window_context_for_test(8745, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from(
            "c:/test/moved-canvas-child.png",
        ))];
        owner.fullscreen_idx = Some(0);
        owner.fs_zoom = 1.0;
        owner.fs_pan = egui::vec2(40.0, 20.0);
    });
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(owner.fs_pan, egui::vec2(40.0, 20.0));
    });
    assert_eq!(app.fs_pan, original_pan);
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(1));
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(owner.fs_pan, egui::vec2(40.0, 20.0));
    });
}

#[test]
fn canvas_move_745_accepted_open_discards_previous_page_manual_transform() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (ctx, mut app) = prepared(true);
    let pan = app.fs_pan + egui::vec2(40.0, 20.0);
    app.set_fs_pan_from_input(1.0, pan);
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    paint_frame(&mut app, &ctx, vec![]);
    click(&mut app, &ctx);
    assert_eq!(app.fullscreen_idx, Some(2));
}

#[test]
fn canvas_move_745_video_keeps_legacy_playback_click_action() {
    for (zoom, pan) in [
        (1.0, egui::Vec2::ZERO),
        (1.0, egui::vec2(40.0, 20.0)),
        (0.8, egui::Vec2::ZERO),
    ] {
        let ctx = egui::Context::default();
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.fs_zoom = zoom;
        app.fs_pan = pan;
        assert!(matches!(
            planned_click(&mut app, true, false),
            FsCanvasClickAction::ToggleVideo(0)
        ));
    }
}

#[test]
fn canvas_move_745_z_compare_and_trim_keep_their_legacy_canvas_clicks() {
    for mode in 0..3 {
        let ctx = egui::Context::default();
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.fs_pan = egui::vec2(40.0, 20.0);
        match mode {
            0 => app.fs_zoom_active = true,
            1 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
            _ => app.view_trim_mode = true,
        }
        assert!(matches!(
            planned_click(&mut app, false, false),
            FsCanvasClickAction::PageSide(1)
        ));
        assert!(matches!(
            planned_click(&mut app, false, true),
            FsCanvasClickAction::ResetTransform
        ));
    }
}

#[test]
fn canvas_move_745_continuous_scroll_does_not_turn_mouse_clicks_into_page_navigation() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.reading_flow = ReadingFlow::Vertical;
    app.fs_vertical_scroll = 150.0;
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
    assert!(matches!(
        planned_click(&mut app, false, true),
        FsCanvasClickAction::None
    ));
}

#[test]
fn canvas_move_745_provisional_resize_and_complete_keep_automatic_click_baseline() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    for (height, kind) in [
        (800.0, FullscreenPageMaterialKind::Provisional),
        (1000.0, FullscreenPageMaterialKind::Complete),
    ] {
        let unit = egui::Rect::from_center_size(viewport().center(), egui::vec2(500.0, height));
        assert!(app.resolve_fs_initial_placement(0, true, viewport(), Some(unit), kind));
        assert!(matches!(
            planned_click(&mut app, false, true),
            FsCanvasClickAction::PageSide(1)
        ));
    }
    assert!(app.fs_initial_placement_request.is_none());
    app.clear_fs_initial_placement_request();
    assert!(matches!(
        planned_click(&mut app, false, true),
        FsCanvasClickAction::PageSide(1)
    ));
}

#[test]
fn canvas_move_745_round2_resized_automatic_half_step_keeps_click_turns() {
    resized_half_step_click_rule(false);
}

#[test]
fn canvas_move_745_round2_resized_manual_half_step_keeps_clicks_stopped() {
    resized_half_step_click_rule(true);
}

fn resized_half_step_click_rule(manual: bool) {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    for wheel in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::SplitLtr, true);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.request_fs_initial_placement(0);
        let placed = super::paint_frame_in_rect(&mut app, &ctx, arrival_viewport(), vec![]);
        assert!(
            placed.request.is_none(),
            "complete paint must retire placement"
        );
        assert!(placed.unit.height() > placed.pages[0].viewport_rect.height());
        assert!(placed.pan.y.abs() > TRANSFORM_EPSILON);
        assert_eq!(
            app.fullscreen_page_slice,
            crate::page_split::PageSlice::Left
        );

        let taller = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 800.0));
        let resized = super::paint_frame_in_rect(&mut app, &ctx, taller, vec![]);
        assert!(resized.unit.height() < resized.pages[0].viewport_rect.height());
        assert!(resized.request.is_none());
        assert_eq!(
            resized.pan, placed.pan,
            "resize retains the completed automatic pan"
        );
        if manual {
            let pan = app.fs_pan + egui::vec2(0.0, -30.0);
            app.set_fs_pan_from_input(1.0, pan);
        }
        let origin_pan = app.fs_pan;
        assert!(matches!(
            (manual, planned_click(&mut app, false, false)),
            (true, FsCanvasClickAction::None) | (false, FsCanvasClickAction::PageSide(1))
        ));
        let event = if wheel {
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            }
        } else {
            key_event(egui::Key::ArrowRight, true)
        };
        super::paint_frame_in_rect(&mut app, &ctx, taller, vec![event]);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(
            app.fullscreen_page_slice,
            crate::page_split::PageSlice::Right
        );
        // The accepted step can retain the old half for its input frame.
        // Resolve against the new half on the following real paint.
        let events = if wheel {
            vec![]
        } else {
            vec![key_event(egui::Key::ArrowRight, false)]
        };
        let stepped = super::paint_frame_in_rect(&mut app, &ctx, taller, events);
        assert_eq!(
            stepped.pages[0].uv_rect,
            crate::page_split::PageSlice::Right.source_bbox(crate::rotation_db::Rotation::None)
        );
        assert!(stepped.request.is_none());
        assert_eq!(
            stepped.pan.y, origin_pan.y,
            "the non-placed axis survives the half step"
        );
        assert!(
            matches!(
                (manual, planned_click(&mut app, false, false)),
                (true, FsCanvasClickAction::None) | (false, FsCanvasClickAction::PageSide(1))
            ),
            "wheel={wheel}, manual={manual}"
        );
    }
}

#[test]
fn canvas_move_745_round2_provisional_placement_keeps_manual_origin_identity() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.set_fs_pan_from_input(1.0, egui::vec2(0.0, 70.0));
    app.request_fs_initial_placement(0);
    let small = viewport();
    let unit = egui::Rect::from_center_size(small.center(), egui::vec2(600.0, 600.0));
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        small,
        Some(unit),
        FullscreenPageMaterialKind::Provisional
    ));
    assert_eq!(app.fs_pan.y, 100.0);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::PageSide(1)
    ));
    let large = egui::Rect::from_min_size(small.min, egui::vec2(600.0, 800.0));
    let unit = egui::Rect::from_center_size(large.center(), egui::vec2(600.0, 600.0));
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        large,
        Some(unit),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(app.fs_pan.y, 70.0);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}

#[test]
fn canvas_move_745_half_step_retains_manual_pan_on_the_axis_not_automatically_placed() {
    let ctx = egui::Context::default();
    let mut app = composition_switch_app(&ctx, SpreadMode::SplitLtr, true, ReadingDirection::Ltr);
    app.fs_pan = egui::vec2(25.0, 40.0);
    app.accept_fullscreen_page_slice_navigation(0, crate::page_split::PageSlice::Right);
    let unit = egui::Rect::from_center_size(viewport().center(), egui::vec2(500.0, 800.0));
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(unit),
        FullscreenPageMaterialKind::Complete
    ));
    assert_eq!(
        app.fs_pan.x, 25.0,
        "half navigation must retain its released pan behavior"
    );
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}

#[test]
fn canvas_move_745_revision_baseline_keeps_zoom_discriminator() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.fs_zoom = 1.5;
    app.request_fs_initial_placement(0);
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(overflowing_unit()),
        FullscreenPageMaterialKind::Complete
    ));
    app.fs_zoom = 1.0;
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}

#[test]
fn canvas_move_745_context_swap_keeps_each_automatic_baseline_and_manual_difference() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    assert!(app.resolve_fs_initial_placement(
        0,
        true,
        viewport(),
        Some(overflowing_unit()),
        FullscreenPageMaterialKind::Complete
    ));
    let main_pan = app.fs_pan;
    let generation = app.items_generation;
    let child = app.build_window_context_for_test(8746, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from(
            "c:/test/automatic-canvas-child.png",
        ))];
        owner.items_generation = generation;
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = SpreadMode::Single;
        owner.reading_flow = ReadingFlow::Paged;
        owner.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
        owner.fs_zoom = 1.0;
        owner.fs_pan = egui::Vec2::ZERO;
        owner.request_fs_initial_placement(0);
        let unit = egui::Rect::from_center_size(viewport().center(), egui::vec2(500.0, 1200.0));
        assert!(owner.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(unit),
            FullscreenPageMaterialKind::Complete
        ));
        assert_ne!(owner.fs_pan, main_pan);
    });
    assert!(matches!(
        planned_click(&mut app, false, true),
        FsCanvasClickAction::PageSide(1)
    ));
    app.with_window_context_for_test(child, |owner| {
        assert!(matches!(
            planned_click(owner, false, true),
            FsCanvasClickAction::PageSide(1)
        ));
        owner.fs_pan = main_pan;
        assert!(matches!(
            planned_click(owner, false, false),
            FsCanvasClickAction::None
        ));
    });
    assert!(matches!(
        planned_click(&mut app, false, true),
        FsCanvasClickAction::PageSide(1)
    ));
}

#[test]
fn canvas_move_745_fit_and_reading_direction_reset_retire_old_automatic_baseline() {
    for direction_reset in [false, true] {
        let ctx = egui::Context::default();
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        assert!(app.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(overflowing_unit()),
            FullscreenPageMaterialKind::Complete
        ));
        let old_pan = app.fs_pan;
        if direction_reset {
            app.reset_continuous_reading_transform();
        } else {
            app.reset_fullscreen_fit_transform(0);
        }
        app.fs_pan = old_pan;
        assert!(matches!(
            planned_click(&mut app, false, false),
            FsCanvasClickAction::None
        ));
    }
}

fn placed_page_one() -> (egui::Context, crate::app::AppTestEnvForTest) {
    let (ctx, mut app) = prepared(true);
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    paint_frame(&mut app, &ctx, vec![]);
    assert!(app.fs_pan.length_sq() > PAN_EPSILON_SQ);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::PageSide(1)
    ));
    (ctx, app)
}

#[test]
fn canvas_move_745_revision_remove_other_item_keeps_automatic_clicks() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    for removed in [3, 0] {
        let (_ctx, mut app) = placed_page_one();
        let item = app.items[1].clone();
        let generation = app.items_generation;
        let transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        app.remove_items_batch(&[removed]);
        assert_ne!(app.items_generation, generation);
        let idx = app.fullscreen_idx.expect("the viewed page survives");
        assert_eq!(app.items[idx], item);
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), transform);
        assert!(
            matches!(
                planned_click(&mut app, false, false),
                FsCanvasClickAction::PageSide(1)
            ),
            "removed={removed}"
        );
    }
}

#[test]
fn canvas_move_745_revision_remove_current_item_keeps_automatic_clicks() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (_ctx, mut app) = placed_page_one();
    let neighbour = app.items[2].clone();
    let transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    app.remove_items_batch(&[1]);
    assert_eq!(app.items[app.fullscreen_idx.unwrap()], neighbour);
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), transform);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::PageSide(1)
    ));
}

#[test]
fn canvas_move_745_revision_remove_other_item_keeps_user_move() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (_ctx, mut app) = placed_page_one();
    let pan = app.fs_pan + egui::vec2(40.0, -20.0);
    app.set_fs_pan_from_input(1.0, pan);
    let transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    app.remove_items_batch(&[3]);
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), transform);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}

#[test]
fn canvas_move_745_revision_retained_half_and_composition_keep_automatic_clicks() {
    for half in [true, false] {
        let ctx = egui::Context::default();
        let mut app = composition_switch_app(
            &ctx,
            if half {
                SpreadMode::SplitLtr
            } else {
                SpreadMode::Single
            },
            true,
            ReadingDirection::Ltr,
        );
        app.request_fs_initial_placement(0);
        assert!(app.resolve_fs_initial_placement(
            0,
            true,
            viewport(),
            Some(overflowing_unit()),
            FullscreenPageMaterialKind::Complete
        ));
        let transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
        app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
        if half {
            app.accept_fullscreen_page_slice_navigation(0, crate::page_split::PageSlice::Right);
        } else {
            app.apply_fullscreen_view_mode(&ctx, 0, FullscreenViewMode::LandscapeSplit);
        }
        assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), transform);
        assert!(
            matches!(
                planned_click(&mut app, false, false),
                FsCanvasClickAction::PageSide(1)
            ),
            "half={half}"
        );
    }
}

#[test]
fn canvas_move_745_revision_new_page_entry_discards_previous_baseline() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (_ctx, mut app) = prepared(true);
    let previous_pan = app.fs_pan;
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    app.set_fs_pan_from_input(1.0, previous_pan);
    assert_eq!(app.fs_pan, previous_pan);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}

fn resume_captured_view(user_moved: bool) -> (egui::Context, crate::app::AppTestEnvForTest) {
    let (ctx, mut app) = prepared(true);
    if user_moved {
        let pan = app.fs_pan + egui::vec2(40.0, -20.0);
        app.set_fs_pan_from_input(1.0, pan);
    }
    let transform = (app.fs_zoom, app.fs_pan, app.fs_free_rotation);
    app.settings.detached_viewer_enabled = true;
    app.native_video_in_window_active = false;
    app.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
    app.settings.detached_viewer_window_physical_placement =
        Some(crate::detached_window_geometry::PhysicalWindowPlacement {
            outer: crate::detached_window_geometry::PhysicalRect {
                left: 0,
                top: 0,
                right: 600,
                bottom: 400,
            },
            maximized: false,
        });
    let snapshot = app
        .capture_detached_image_window_snapshot_for_test(&ctx, 8748)
        .expect("capture the real automatically placed or moved still view");
    assert!(snapshot.reopen_sync_stamp.is_some());
    assert_eq!(snapshot.zoom_pan, Some((transform.0, transform.1)));
    app.close_fullscreen();
    // Clone is also a saved-view transfer boundary.
    app.detached_image_windows.push(snapshot.clone());
    assert!(app.activate_detached_image_window_snapshot(&ctx, 8748));
    assert_eq!((app.fs_zoom, app.fs_pan, app.fs_free_rotation), transform);
    assert!(app.fs_initial_placement_request.is_none());
    (ctx, app)
}

#[test]
fn canvas_move_745_revision_snapshot_automatic_view_resumes_clicks() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (_ctx, mut app) = resume_captured_view(false);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::PageSide(1)
    ));
}

#[test]
fn canvas_move_745_revision_snapshot_user_move_stays_moved() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let (_ctx, mut app) = resume_captured_view(true);
    assert!(matches!(
        planned_click(&mut app, false, false),
        FsCanvasClickAction::None
    ));
}
