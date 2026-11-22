//! Production pointer ingress and paint receipts for the canvas-only boundary.
use super::handler_tests::{app as scroll_app, frame, full_rect, paint, run_in_viewport, seed};
use super::tests::close;
use super::*;

pub(super) fn pointer_frame(
    app: &mut App,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> FsPageNav {
    let mut result = FsPageNav::None;
    frame(ctx, events, |ui, ctx| {
        let idx = app.fullscreen_idx.unwrap();
        let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
        let (nav, close) =
            app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
        assert!(!close);
        result = nav;
    });
    result
}

fn origin() -> egui::Pos2 {
    egui::pos2(300.0, 200.0)
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn press(app: &mut App, ctx: &egui::Context) {
    assert!(pointer_frame(app, ctx, vec![egui::Event::PointerMoved(origin())]).is_none());
    assert!(pointer_frame(app, ctx, vec![button(origin(), true)]).is_none());
}

pub(super) fn drag(app: &mut App, ctx: &egui::Context, delta: Vec2) {
    press(app, ctx);
    let end = origin() + delta;
    assert!(pointer_frame(app, ctx, vec![egui::Event::PointerMoved(end)]).is_none());
    assert!(pointer_frame(app, ctx, vec![button(end, false)]).is_none());
}

fn prepared(
    bound: bool,
    fit: FullscreenFitMode,
    zoom: f32,
) -> (egui::Context, crate::app::AppTestEnvForTest) {
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_bound_drag_pan = bound;
    app.settings.fullscreen_fit_mode = fit;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_zoom = zoom;
    seed(&ctx, &mut app);
    (ctx, app)
}

fn install_material(app: &mut App, ctx: &egui::Context, idx: usize, size: [usize; 2]) {
    let pixels = egui::ColorImage::filled(size, egui::Color32::WHITE);
    let tex = ctx.load_texture(
        format!("drag-bound-material-{idx}"),
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let pdf_metadata = matches!(app.items[idx], GridItem::PdfPage { .. }).then_some(
        crate::app::PdfPageMetadataSnapshot {
            layout_dims: (size[0] as u32, size[1] as u32),
            content_type: crate::pdf_loader::PdfPageContentType::Vector,
        },
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata,
            tex,
            pixels: Arc::new(pixels),
            source_dims: Some(size),
            load_seq: 2,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
}

fn move_bound_drag_to(app: &mut App, ctx: &egui::Context, total: Vec2) {
    assert!(pointer_frame(app, ctx, vec![egui::Event::PointerMoved(origin() + total)]).is_none());
}

fn bound_drag_origin_pan(app: &App) -> Vec2 {
    app.fs_pan_drag_start
        .expect("the real canvas owns this drag")
        .1
}

fn prepared_at_lower_edges(bound: bool) -> (egui::Context, crate::app::AppTestEnvForTest) {
    let (ctx, mut app) = prepared(bound, FullscreenFitMode::Width, 3.0);
    drag(&mut app, &ctx, Vec2::splat(-10000.0));
    seed(&ctx, &mut app);
    assert!(app.fs_pan_drag_start.is_none());
    (ctx, app)
}

#[test]
fn drag_bound_rebases_blocked_axes_and_reverses_from_the_edge_immediately() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared_at_lower_edges(true);
    let edge = app.fs_pan;
    let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
    close(
        edge,
        Vec2::new(geometry.axes[0].unwrap().min, geometry.axes[1].unwrap().min),
    );
    press(&mut app, &ctx);
    let outward = Vec2::new(-300.0, -180.0);
    move_bound_drag_to(&mut app, &ctx, outward);
    close(app.fs_pan, edge);
    close(bound_drag_origin_pan(&app), edge - outward);
    let rebased = app.fs_pan_drag_start;
    move_bound_drag_to(&mut app, &ctx, outward + Vec2::new(100.0, 60.0));
    close(app.fs_pan, edge + Vec2::new(100.0, 60.0));
    assert_eq!(
        app.fs_pan_drag_start, rebased,
        "an unconstrained reversal must retain both rebased axes"
    );
}

#[test]
fn drag_bound_diagonal_rebases_only_the_axis_that_hit_its_edge() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    app.fs_pan.y = 0.1;
    seed(&ctx, &mut app);
    let start = app.fs_pan;
    let min_x = scroll_geometry(&app.fullscreen_page_layout).unwrap().axes[0]
        .unwrap()
        .min;
    press(&mut app, &ctx);
    let total = Vec2::new(-10000.0, -120.0);
    move_bound_drag_to(&mut app, &ctx, total);
    close(app.fs_pan, Vec2::new(min_x, start.y + total.y));
    let stored = bound_drag_origin_pan(&app);
    close(Vec2::new(stored.x, 0.0), Vec2::new(min_x - total.x, 0.0));
    assert_eq!(
        stored.y.to_bits(),
        start.y.to_bits(),
        "recomputing (0.1 - 120) + 120 would accumulate f32 residue on a free axis"
    );
    move_bound_drag_to(&mut app, &ctx, Vec2::new(-9990.0, -180.0));
    close(app.fs_pan, Vec2::new(min_x + 10.0, start.y - 180.0));
    assert_eq!(bound_drag_origin_pan(&app).y.to_bits(), start.y.to_bits());
}

#[test]
fn drag_bound_waiting_layout_retains_the_absolute_origin_until_a_matching_paint() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    let initial = app.fs_pan;
    press(&mut app, &ctx);
    app.fullscreen_page_layout.drawn_unit = None;
    move_bound_drag_to(&mut app, &ctx, Vec2::new(25.0, -30.0));
    let waiting_origin = app.fs_pan_drag_start;
    close(bound_drag_origin_pan(&app), initial);
    close(app.fs_pan, initial);
    move_bound_drag_to(&mut app, &ctx, Vec2::new(45.0, -60.0));
    assert_eq!(app.fs_pan_drag_start, waiting_origin);
    close(app.fs_pan, initial);
    seed(&ctx, &mut app);
    pointer_frame(&mut app, &ctx, vec![]);
    close(app.fs_pan, initial + Vec2::new(45.0, -60.0));
    assert_eq!(
        app.fs_pan_drag_start, waiting_origin,
        "a fully accepted absolute candidate does not rebase"
    );
}

#[test]
fn drag_bound_rebased_origin_survives_stale_frames_and_catches_up_after_paint() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared_at_lower_edges(true);
    let edge = app.fs_pan;
    press(&mut app, &ctx);
    let outward = Vec2::new(-300.0, -180.0);
    move_bound_drag_to(&mut app, &ctx, outward);
    let rebased = app.fs_pan_drag_start;
    close(bound_drag_origin_pan(&app), edge - outward);
    app.fullscreen_page_layout.drawn_unit = None;
    move_bound_drag_to(&mut app, &ctx, outward + Vec2::new(100.0, 60.0));
    close(app.fs_pan, edge);
    assert_eq!(app.fs_pan_drag_start, rebased);
    move_bound_drag_to(&mut app, &ctx, outward + Vec2::new(120.0, 80.0));
    close(app.fs_pan, edge);
    assert_eq!(app.fs_pan_drag_start, rebased);
    seed(&ctx, &mut app);
    pointer_frame(&mut app, &ctx, vec![]);
    close(app.fs_pan, edge + Vec2::new(120.0, 80.0));
    assert_eq!(app.fs_pan_drag_start, rebased);
}

#[test]
fn drag_bound_off_retains_legacy_overdrag_until_the_absolute_pointer_passes_its_press() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared_at_lower_edges(false);
    let loose_edge = app.fs_pan;
    press(&mut app, &ctx);
    for total in [Vec2::new(-300.0, -180.0), Vec2::new(-200.0, -120.0)] {
        move_bound_drag_to(&mut app, &ctx, total);
        close(app.fs_pan, loose_edge);
        close(bound_drag_origin_pan(&app), loose_edge);
    }
    move_bound_drag_to(&mut app, &ctx, Vec2::new(100.0, 60.0));
    close(app.fs_pan, loose_edge + Vec2::new(100.0, 60.0));
    close(bound_drag_origin_pan(&app), loose_edge);
}

#[test]
fn drag_bound_release_discards_the_rebased_origin_and_next_drag_starts_at_current_pan() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared_at_lower_edges(true);
    let edge = app.fs_pan;
    press(&mut app, &ctx);
    let outward = Vec2::new(-300.0, -180.0);
    move_bound_drag_to(&mut app, &ctx, outward);
    assert_ne!(bound_drag_origin_pan(&app), edge);
    pointer_frame(&mut app, &ctx, vec![button(origin() + outward, false)]);
    assert!(app.fs_pan_drag_start.is_none());
    seed(&ctx, &mut app);
    press(&mut app, &ctx);
    move_bound_drag_to(&mut app, &ctx, Vec2::new(20.0, 10.0));
    close(app.fs_pan, edge + Vec2::new(20.0, 10.0));
    close(bound_drag_origin_pan(&app), edge);
}

#[test]
fn drag_bound_one_finger_passthrough_reverses_immediately_after_rebasing() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    crate::touch_correlation::with_gestures_disabled(false, || {
        assert!(!crate::touch_correlation::touch_gestures_disabled());
        let (ctx, mut app) = prepared_at_lower_edges(true);
        app.settings.touch_still_chrome_learned = true;
        let edge = app.fs_pan;
        let touch = |phase, pos| egui::Event::Touch {
            device_id: egui::TouchDeviceId(8348),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        };
        pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(origin())]);
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(egui::TouchPhase::Start, origin()),
                egui::Event::PointerMoved(origin()),
                button(origin(), true),
            ],
        );
        for (total, expected) in [
            (Vec2::new(-300.0, -180.0), edge),
            (Vec2::new(-200.0, -120.0), edge + Vec2::new(100.0, 60.0)),
        ] {
            let end = origin() + total;
            frame(
                &ctx,
                vec![
                    touch(egui::TouchPhase::Move, end),
                    egui::Event::PointerMoved(end),
                ],
                |ui, ctx| {
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let (nav, closed) = app.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        full_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                    assert!(nav.is_none() && !closed);
                    let replay = crate::touch_correlation::drive_egui_touch_input(
                        ctx,
                        crate::touch_correlation::TouchSurface::StillFullscreen,
                        crate::touch_input::TapZoneGeometry {
                            surface: full_rect(),
                            excluded: vec![],
                            behavior: crate::touch_input::TouchSurfaceBehavior::Viewer {
                                accepts_pinch: true,
                                tap_zones: true,
                            },
                        },
                        ctx.cumulative_frame_nr(),
                        true,
                    );
                    assert!(replay.commands().is_empty());
                    assert_eq!(
                        replay.owner(),
                        crate::touch_input::TouchOwner::ViewerPointerPassthrough
                    );
                },
            );
            close(app.fs_pan, expected);
            close(bound_drag_origin_pan(&app), edge + Vec2::new(300.0, 180.0));
        }
        let end = origin() + Vec2::new(-200.0, -120.0);
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(egui::TouchPhase::End, end),
                button(end, false),
                egui::Event::PointerGone,
            ],
        );
        assert!(app.fs_pan_drag_start.is_none());
        close(app.fs_pan, edge + Vec2::new(100.0, 60.0));
    });
}

#[test]
fn drag_bound_animation_rebases_at_the_edge_and_reverses_from_logical_not_painted_pan() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 1.0);
    app.settings.fullscreen_smooth_page_scroll = true;
    app.settings.continuous_reading_key_scroll_percent = 100;
    frame(
        &ctx,
        super::handler_tests::tap(egui::Key::ArrowDown),
        |_, ctx| {
            assert!(app.handle_fs_key_input(ctx, 0, false).page_nav.is_none());
        },
    );
    let target = app.fs_pan;
    assert!(target.y < 0.0);
    seed(&ctx, &mut app);
    let painted = normal_zoom_pan(
        app.fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform,
    )
    .unwrap()
    .1;
    assert_ne!(
        painted, target,
        "the real receipt must contain the interpolated pan"
    );
    assert!(app.fs_page_scroll_animation.is_some());
    press(&mut app, &ctx);
    move_bound_drag_to(&mut app, &ctx, Vec2::new(0.0, -180.0));
    close(app.fs_pan, target);
    close(bound_drag_origin_pan(&app), target + Vec2::new(0.0, 180.0));
    assert!(
        app.fs_page_scroll_animation.is_some(),
        "blocking alone must not cancel interpolation"
    );
    move_bound_drag_to(&mut app, &ctx, Vec2::new(0.0, -120.0));
    close(app.fs_pan, target + Vec2::new(0.0, 60.0));
    frame(&ctx, vec![], |ui, ctx| {
        app.retire_fs_page_scroll_animation(ctx);
        assert!(app.fs_page_scroll_animation.is_none());
        paint(&mut app, ui, ctx);
    });
    let drawn_after = normal_zoom_pan(
        app.fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform,
    )
    .unwrap()
    .1;
    close(drawn_after, target + Vec2::new(0.0, 60.0));
}

fn bound_drag_wheel_zoom_at(app: &mut App, ctx: &egui::Context, pivot: egui::Pos2, factor: f32) {
    let delta = WHEEL_SENSITIVITY * factor.ln() / 1.1_f32.ln();
    pointer_frame(
        app,
        ctx,
        vec![
            egui::Event::PointerMoved(pivot),
            egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
            super::handler_tests::wheel_event(Vec2::new(0.0, delta), egui::Modifiers::CTRL),
        ],
    );
}

fn assert_drag_bound_held_ctrl_zoom(
    bound: bool,
    pivot_total: Vec2,
    zoom_pan: Vec2,
    resumed_pan: Vec2,
) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let (ctx, mut app) = prepared(bound, FullscreenFitMode::Page, 1.0);
    close(app.fs_pan, Vec2::ZERO);
    press(&mut app, &ctx);
    let first_total = Vec2::new(80.0, 40.0);
    move_bound_drag_to(&mut app, &ctx, first_total);
    let before_ctrl = if bound { Vec2::ZERO } else { first_total };
    close(app.fs_pan, before_ctrl);
    close(bound_drag_origin_pan(&app), before_ctrl - first_total);
    let press_pos = app.fs_pan_drag_start.unwrap().0;

    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::ModifiersChanged(egui::Modifiers::CTRL)],
    );
    move_bound_drag_to(&mut app, &ctx, pivot_total);
    close(app.fs_pan, before_ctrl);
    close(
        bound_drag_origin_pan(&app),
        if bound {
            before_ctrl - pivot_total
        } else {
            Vec2::ZERO
        },
    );
    assert_eq!(app.fs_pan_drag_start.unwrap().0, press_pos);
    assert!(
        app.fs_rotation_drag_start.is_none(),
        "Ctrl added after press cannot start rotation"
    );
    assert_eq!(app.fs_free_rotation, 0.0);

    bound_drag_wheel_zoom_at(&mut app, &ctx, origin() + pivot_total, 4.0);
    assert!((app.fs_zoom - 4.0).abs() < 0.0001);
    close(app.fs_pan, zoom_pan);
    close(
        bound_drag_origin_pan(&app),
        if bound {
            zoom_pan - pivot_total
        } else {
            Vec2::ZERO
        },
    );
    assert_eq!(app.fs_pan_drag_start.unwrap().0, press_pos);
    assert!(app.fs_rotation_drag_start.is_none());
    assert_eq!(app.fs_free_rotation, 0.0);
    seed(&ctx, &mut app);
    assert!(app.fs_scroll_layout_matches(&ctx));
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
    );
    close(app.fs_pan, resumed_pan);
    move_bound_drag_to(&mut app, &ctx, pivot_total + Vec2::new(30.0, 20.0));
    close(app.fs_pan, resumed_pan + Vec2::new(30.0, 20.0));
    pointer_frame(
        &mut app,
        &ctx,
        vec![button(
            origin() + pivot_total + Vec2::new(30.0, 20.0),
            false,
        )],
    );
    assert!(app.fs_pan_drag_start.is_none());
}

#[test]
fn drag_bound_r4_ctrl_return_to_press_preserves_center_zoom_and_resumes_from_current_pan() {
    assert_drag_bound_held_ctrl_zoom(true, Vec2::ZERO, Vec2::ZERO, Vec2::ZERO);
}

#[test]
fn drag_bound_ctrl_offcenter_zoom_preserves_pan_and_resumes_from_current_pan() {
    // With cur=0 and pivot offset (60,30), 4x zoom gives -3 * offset.
    assert_drag_bound_held_ctrl_zoom(
        true,
        Vec2::new(60.0, 30.0),
        Vec2::new(-180.0, -90.0),
        Vec2::new(-180.0, -90.0),
    );
}

#[test]
fn drag_bound_off_ctrl_return_to_press_keeps_legacy_absolute_drag_origin() {
    // OFF accepts (80,40) before Ctrl: centered 4x zoom makes (320,160).
    // Returning to the press still proposes the unchanged origin plus zero.
    assert_drag_bound_held_ctrl_zoom(false, Vec2::ZERO, Vec2::new(320.0, 160.0), Vec2::ZERO);
}

#[test]
fn drag_bound_off_ctrl_offcenter_zoom_keeps_legacy_absolute_drag_origin() {
    // 4*(80,40) - 3*(60,30) = (140,70); OFF resumes at cumulative (60,30).
    assert_drag_bound_held_ctrl_zoom(
        false,
        Vec2::new(60.0, 30.0),
        Vec2::new(140.0, 70.0),
        Vec2::new(60.0, 30.0),
    );
}

#[test]
fn drag_bound_held_center_pivot_zoom_crosses_fitted_and_overflow_rules_in_both_directions() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Page, 1.0);
    press(&mut app, &ctx);
    move_bound_drag_to(&mut app, &ctx, Vec2::new(80.0, 40.0));
    close(app.fs_pan, Vec2::ZERO);
    close(bound_drag_origin_pan(&app), Vec2::new(-80.0, -40.0));
    move_bound_drag_to(&mut app, &ctx, Vec2::ZERO);
    close(app.fs_pan, Vec2::ZERO);
    close(bound_drag_origin_pan(&app), Vec2::ZERO);
    bound_drag_wheel_zoom_at(&mut app, &ctx, origin(), 4.0);
    assert!((app.fs_zoom - 4.0).abs() < 0.0001);
    close(app.fs_pan, Vec2::ZERO);
    assert_eq!(
        app.fs_free_rotation, 0.0,
        "Ctrl-wheel is not a Ctrl-drag rotation"
    );
    seed(&ctx, &mut app);
    let overflow = scroll_geometry(&app.fullscreen_page_layout).unwrap();
    assert!(overflow.axes.iter().all(Option::is_some));
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
    );
    close(app.fs_pan, Vec2::ZERO);
    move_bound_drag_to(&mut app, &ctx, Vec2::new(30.0, 20.0));
    close(app.fs_pan, Vec2::new(30.0, 20.0));
    move_bound_drag_to(&mut app, &ctx, Vec2::ZERO);
    close(app.fs_pan, Vec2::ZERO);
    bound_drag_wheel_zoom_at(&mut app, &ctx, origin(), 0.25);
    assert!((app.fs_zoom - 1.0).abs() < 0.0001);
    close(app.fs_pan, Vec2::ZERO);
    seed(&ctx, &mut app);
    let fitted = scroll_geometry(&app.fullscreen_page_layout).unwrap();
    assert!(fitted.axes.iter().all(Option::is_none));
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
    );
    move_bound_drag_to(&mut app, &ctx, Vec2::new(30.0, 20.0));
    close(app.fs_pan, Vec2::ZERO);
    close(bound_drag_origin_pan(&app), Vec2::new(-30.0, -20.0));
    pointer_frame(&mut app, &ctx, vec![]);
    close(app.fs_pan, Vec2::ZERO);
}

#[cfg(windows)]
#[test]
fn drag_bound_r11_asymmetric_visibility_rebase_has_the_documented_path_dependent_endpoint() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let outcomes = [false, true].map(|split| {
        let ctx = egui::Context::default();
        // The r11 geometry uses a 3000-pixel-tall fixture texture.
        ctx.input_mut(|input| input.max_texture_side = 4096);
        let mut app = scroll_app(&ctx, SpreadMode::Ltr);
        app.fs_prev_focused = true;
        app.settings.fullscreen_bound_drag_pan = true;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.settings.spread_page_gap_px = 20;
        install_material(&mut app, &ctx, 0, [1000, 3000]);
        install_material(&mut app, &ctx, 1, [1000, 1010]);
        app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::Manual;
        app.view_trim_page_overrides.insert(
            1,
            crate::view_trim::ViewTrimPageOverride::from_margins(
                crate::view_trim::ViewTrimMargins {
                    left: 0.0,
                    top: 0.2,
                    right: 0.0,
                    bottom: 0.0,
                },
            ),
        );
        let initial_pan = Vec2::new(-442.0, 1000.0);
        app.fs_pan = initial_pan;
        let rect = super::tests::viewport();
        let pages = render_frame(&mut app, &ctx, rect, 1.0, vec![]);
        assert_eq!(pages.len(), 2);
        // Real Original/gap/trim layout equals asymmetric_spread(true,false),
        // translated by cur; the valid stamp comes from the actual renderer.
        close(pages[0].paint_rect.min.to_vec2(), Vec2::new(-922.0, 50.0));
        close(pages[0].paint_rect.max.to_vec2(), Vec2::new(78.0, 3050.0));
        close(pages[1].paint_rect.min.to_vec2(), Vec2::new(98.0, 1247.0));
        close(pages[1].paint_rect.max.to_vec2(), Vec2::new(1098.0, 2055.0));
        assert!(app.fs_scroll_layout_matches(&ctx));
        render_frame(
            &mut app,
            &ctx,
            rect,
            1.0,
            vec![egui::Event::PointerMoved(origin())],
        );
        render_frame(&mut app, &ctx, rect, 1.0, vec![button(origin(), true)]);
        if split {
            let first = Vec2::new(-51.0, -225.0);
            render_frame(
                &mut app,
                &ctx,
                rect,
                1.0,
                vec![egui::Event::PointerMoved(origin() + first)],
            );
            close(app.fs_pan, Vec2::new(-442.0, 775.0));
            close(bound_drag_origin_pan(&app), Vec2::new(-391.0, 1000.0));
        }
        let total = Vec2::new(-68.0, -300.0);
        render_frame(
            &mut app,
            &ctx,
            rect,
            1.0,
            vec![egui::Event::PointerMoved(origin() + total)],
        );
        let endpoint = app.fs_pan;
        close(
            endpoint,
            if split {
                Vec2::new(-459.0, 700.0)
            } else {
                Vec2::new(-510.0, 700.0)
            },
        );
        if split {
            render_frame(
                &mut app,
                &ctx,
                rect,
                1.0,
                vec![egui::Event::PointerMoved(
                    origin() + Vec2::new(-119.0, -300.0),
                )],
            );
            close(app.fs_pan, Vec2::new(-510.0, 700.0));
        }
        endpoint
    });
    assert_eq!(
        outcomes,
        [Vec2::new(-510.0, 700.0), Vec2::new(-459.0, 700.0)]
    );
}

#[cfg(windows)]
#[test]
fn drag_bound_sub_pc_pivot_shrink_returns_a_fitting_unit_inside_then_blocks_outward_drag() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.fs_prev_focused = true;
    app.settings.fullscreen_bound_drag_pan = true;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_zoom = 4.0;
    render_frame(&mut app, &ctx, full_rect(), 1.0, vec![]);
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(origin())],
    );
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![button(origin(), true)],
    );
    let enlarged_end = origin() + Vec2::splat(10000.0);
    let edge_pages = render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(enlarged_end)],
    );
    let edge = edge_pages[0];
    assert!(edge.paint_rect.width() > edge.viewport_rect.width());
    assert!(edge.paint_rect.height() > edge.viewport_rect.height());
    assert!((edge.paint_rect.top() - edge.viewport_rect.top()).abs() <= 1.0);
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![button(enlarged_end, false)],
    );
    assert!(app.fs_pan_drag_start.is_none());
    let before_zoom = app.fs_pan;
    let factor = 0.2_f32;
    let delta = WHEEL_SENSITIVITY * factor.ln() / 1.1_f32.ln();
    let shrunk = render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![
            egui::Event::PointerMoved(origin()),
            egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
            super::handler_tests::wheel_event(Vec2::new(0.0, delta), egui::Modifiers::CTRL),
        ],
    )[0];
    assert!((app.fs_zoom - 0.8).abs() < 0.0001);
    close(app.fs_pan, before_zoom * factor);
    assert!(shrunk.paint_rect.width() <= shrunk.viewport_rect.width());
    assert!(shrunk.paint_rect.height() <= shrunk.viewport_rect.height());
    assert!(
        shrunk.paint_rect.bottom() > shrunk.viewport_rect.bottom() + 50.0,
        "center-pivot shrink must reproduce the reported fitting-but-outside unit: {shrunk:?}"
    );
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
    );
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![button(origin(), true)],
    );
    let inward_total = Vec2::new(0.0, -200.0);
    let returned = render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(origin() + inward_total)],
    )[0];
    let returned_pan = app.fs_pan;
    assert!(returned_pan.y < before_zoom.y * factor);
    assert!((returned.paint_rect.bottom() - returned.viewport_rect.bottom()).abs() <= 1.0);
    assert!(returned.paint_rect.top() >= returned.viewport_rect.top() - 1.0);
    assert!(
        (returned_pan.x - before_zoom.x * factor).abs() < 0.001,
        "a fitting and already-inside horizontal axis stays at its noncentral pan"
    );
    let origin_after_return = app.fs_pan_drag_start;
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(origin() + Vec2::new(0.0, -100.0))],
    );
    assert_eq!(
        app.fs_pan, returned_pan,
        "reversing outwards cannot move the now-fitted unit"
    );
    assert_ne!(
        app.fs_pan_drag_start, origin_after_return,
        "the newly blocked outward demand must discard its overdrag"
    );
}

#[test]
fn drag_bound_off_preserves_fitting_and_overflowing_legacy_drag() {
    for (fit, zoom) in [
        (FullscreenFitMode::Page, 1.0),
        (FullscreenFitMode::Width, 3.0),
    ] {
        let (ctx, mut app) = prepared(false, fit, zoom);
        let delta = Vec2::new(700.0, -900.0);
        let expected = clamp_fullscreen_pan_from_layout(&app.fullscreen_page_layout, zoom, delta);
        assert_ne!(expected, Vec2::ZERO);
        drag(&mut app, &ctx, delta);
        close(app.fs_pan, expected);
        assert!(app.fs_pan_drag_start.is_none());
    }
}

#[test]
fn drag_bound_on_fitting_image_keeps_inside_axis_and_returns_only_its_outside_axis() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Page, 1.0);
    app.fs_pan = Vec2::new(19.0, -23.0);
    seed(&ctx, &mut app);
    let before = app.fs_pan;
    let page = app
        .fullscreen_page_layout
        .displayed_pages()
        .next()
        .unwrap()
        .transform;
    assert!(page.paint_rect.top() < page.viewport_rect.top());
    assert!((page.paint_rect.height() - page.viewport_rect.height()).abs() < 0.001);
    assert!(page.paint_rect.left() >= page.viewport_rect.left());
    assert!(page.paint_rect.right() <= page.viewport_rect.right());
    drag(&mut app, &ctx, Vec2::new(80.0, 70.0));
    // Page fit is 266x400 in the 600x400 viewport. Nonzero x is inside,
    // while y=-23 leaves the top 23 points outside; §8.2 returns only y.
    close(app.fs_pan, Vec2::new(before.x, 0.0));
}

#[test]
fn drag_bound_on_stops_at_all_four_edges_and_diagonals() {
    for delta in [
        Vec2::new(-10000.0, 0.0),
        Vec2::new(10000.0, 0.0),
        Vec2::new(0.0, -10000.0),
        Vec2::new(0.0, 10000.0),
        Vec2::splat(-10000.0),
        Vec2::splat(10000.0),
    ] {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
        let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
        let expected = Vec2::new(
            step_axis(geometry.axes[0], 0.0, delta.x),
            step_axis(geometry.axes[1], 0.0, delta.y),
        );
        drag(&mut app, &ctx, delta);
        close(app.fs_pan, expected);
    }
}

#[test]
fn drag_bound_outside_start_does_not_jump_on_stationary_or_outward_drag() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
    app.fs_pan.x = geometry.axes[0].unwrap().min - 20.0;
    seed(&ctx, &mut app);
    let before = app.fs_pan;
    press(&mut app, &ctx);
    pointer_frame(&mut app, &ctx, vec![]);
    assert_eq!(app.fs_pan, before);
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(origin() - Vec2::new(10.0, 0.0))],
    );
    assert_eq!(app.fs_pan, before);
    close(bound_drag_origin_pan(&app), before + Vec2::new(10.0, 0.0));
    pointer_frame(&mut app, &ctx, vec![]);
    assert_eq!(app.fs_pan, before);
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(origin() + Vec2::new(10.0, 0.0))],
    );
    // §8.1 discards the blocked -10, so -10 -> +10 is 20 inward
    // points from the last pointer position, not 10 from the original press.
    close(app.fs_pan, before + Vec2::new(20.0, 0.0));
}

#[test]
fn drag_bound_geometry_changes_wait_for_paint_then_resume_absolute_candidate() {
    for change in 0..4 {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
        press(&mut app, &ctx);
        match change {
            0 => app.fs_zoom = 1.005,
            1 => install_material(&mut app, &ctx, 0, [81, 120]),
            _ => {}
        }
        let rect = if change == 2 {
            Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(620.0, 430.0))
        } else {
            full_rect()
        };
        let dpi = if change == 3 { 1.5 } else { 1.0 };
        let before = app.fs_pan;
        let end = origin() + Vec2::new(0.0, -30.0);
        for pass in 0..2 {
            run_in_viewport(
                &ctx,
                egui::ViewportId::ROOT,
                rect,
                dpi,
                if pass == 0 {
                    vec![egui::Event::PointerMoved(end)]
                } else {
                    vec![]
                },
                |ui, ctx| {
                    let matches = app.fs_scroll_layout_matches(ctx);
                    assert_eq!(matches, pass != 0, "change={change}, pass={pass}");
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let (nav, closed) =
                        app.handle_fs_wheel_and_click(ui, ctx, rect, &state, false, 0, &mut None);
                    assert!(nav.is_none() && !closed);
                    if pass == 0 {
                        assert_eq!(app.fs_pan, before);
                        paint(&mut app, ui, ctx);
                    } else {
                        close(app.fs_pan, before + Vec2::new(0.0, -30.0));
                    }
                },
            );
        }
    }
}

#[test]
fn drag_bound_entry_rejects_stale_receipt_and_clears_only_request() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    let before = app.fs_pan;
    let automatic = app.fs_automatic_initial_view;
    app.fullscreen_page_layout.drawn_unit = None;
    app.request_fs_initial_placement(0);
    frame(&ctx, vec![], |_, ctx| {
        app.apply_fs_bounded_drag_pan(ctx, Vec2::new(17.0, 23.0));
    });
    assert_eq!(app.fs_pan, before);
    assert_eq!(app.fs_automatic_initial_view, automatic);
    assert!(app.fs_initial_placement_request.is_none());
}

#[test]
fn drag_bound_entry_keeps_drawn_zoom_and_automatic_origin() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 1.005);
    let automatic = app.fs_automatic_initial_view;
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_scroll_layout_matches(ctx));
        let drawn = app.fs_page_scroll_drawn_zoom();
        let proposed = Vec2::new(0.0, -10.0);
        app.apply_fs_bounded_drag_pan(ctx, proposed);
        assert_eq!(app.fs_zoom, drawn);
        assert_eq!(app.fs_pan, proposed);
        assert_eq!(app.fs_automatic_initial_view, automatic);
    });
}

#[test]
fn drag_bound_stale_drag_waits_then_catches_up_or_releases_without_write() {
    for release_early in [false, true] {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
        press(&mut app, &ctx);
        app.fullscreen_page_layout.drawn_unit = None;
        app.request_fs_initial_placement(0);
        let before = app.fs_pan;
        let end = origin() + Vec2::new(30.0, -40.0);
        assert!(pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]).is_none());
        assert_eq!(app.fs_pan, before, "stale pointer frame cannot publish pan");
        assert!(app.fs_initial_placement_request.is_none());
        if release_early {
            pointer_frame(&mut app, &ctx, vec![button(end, false)]);
            seed(&ctx, &mut app);
            pointer_frame(&mut app, &ctx, vec![]);
            assert_eq!(app.fs_pan, before);
            assert!(app.fs_pan_drag_start.is_none());
        } else {
            seed(&ctx, &mut app);
            pointer_frame(&mut app, &ctx, vec![]);
            close(app.fs_pan, before + Vec2::new(30.0, -40.0));
            pointer_frame(&mut app, &ctx, vec![button(end, false)]);
        }
    }
}

#[test]
fn drag_bound_surface_uses_painted_rotation_and_matching_receipt() {
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Single);
    app.fs_free_rotation = 0.2;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(!app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
    });
    app.spread_mode = SpreadMode::Ltr;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
    });
    app.spread_mode = SpreadMode::Single;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(!app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
    });
    app.fullscreen_page_layout.drawn_unit = None;
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
    });
}

#[test]
fn drag_bound_surface_matches_the_actual_paint_rotation_threshold() {
    for (angle, rotated) in [
        (0.0, false),
        (1.0e-6, false),
        (2.0e-6, true),
        (0.0005, true),
    ] {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
        app.fs_free_rotation = angle;
        seed(&ctx, &mut app);
        let page = app
            .fullscreen_page_layout
            .displayed_pages()
            .next()
            .unwrap()
            .transform;
        let unrotated = [
            page.paint_rect.left_top(),
            page.paint_rect.right_top(),
            page.paint_rect.right_bottom(),
            page.paint_rect.left_bottom(),
        ];
        assert_eq!(
            page.paint_quad().positions != unrotated,
            rotated,
            "angle={angle}"
        );
        frame(&ctx, vec![], |_, ctx| {
            assert_eq!(
                app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)),
                !rotated,
                "angle={angle}"
            );
        });
    }
}

#[test]
fn drag_bound_small_painted_free_rotation_keeps_the_legacy_drag() {
    for angle in [-0.0005, 0.0005] {
        let outcomes = [false, true].map(|bound| {
            let (ctx, mut app) = prepared(bound, FullscreenFitMode::Width, 3.0);
            app.fs_free_rotation = angle;
            seed(&ctx, &mut app);
            let delta = Vec2::new(900.0, -1800.0);
            let expected = clamp_fullscreen_pan_from_layout(
                &app.fullscreen_page_layout,
                app.fs_zoom,
                app.fs_pan + delta,
            );
            drag(&mut app, &ctx, delta);
            close(app.fs_pan, expected);
            (app.fs_zoom, app.fs_pan)
        });
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

#[test]
fn drag_bound_bookmarks_tab_keeps_the_reading_boundary_active() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Page, 1.0);
    app.settings.fullscreen_left_panel_tab = crate::settings::FullscreenLeftPanelTab::Bookmarks;
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle;
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
    });
    let before = app.fs_pan;
    drag(&mut app, &ctx, Vec2::new(80.0, 40.0));
    assert!(app.adjustment_mode.is_open());
    close(app.fs_pan, before);
}

#[test]
fn drag_bound_view_trim_editing_keeps_the_existing_pointer_owner() {
    let outcomes = [false, true].map(|bound| {
        let (ctx, mut app) = prepared(bound, FullscreenFitMode::Width, 3.0);
        app.view_trim_mode = true;
        frame(&ctx, vec![], |_, ctx| {
            assert!(!app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
        });
        let expected = clamp_fullscreen_pan_from_layout(
            &app.fullscreen_page_layout,
            app.fs_zoom,
            Vec2::new(80.0, 40.0),
        );
        assert_ne!(expected, Vec2::ZERO);
        drag(&mut app, &ctx, Vec2::new(80.0, 40.0));
        close(app.fs_pan, expected);
        (app.fs_zoom, app.fs_pan)
    });
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn drag_bound_saved_quarter_rotation_and_upright_spread_with_retained_tilt_are_bounded() {
    for spread in [false, true] {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
        if spread {
            app.fs_free_rotation = 0.2;
            app.spread_mode = SpreadMode::Ltr;
        } else {
            app.rotation_cache
                .insert(0, crate::rotation_db::Rotation::Cw90);
        }
        seed(&ctx, &mut app);
        frame(&ctx, vec![], |_, ctx| {
            assert!(app.fs_bounded_drag_pan_surface(app.fs_scroll_layout_matches(ctx)))
        });
        let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
        let delta = Vec2::splat(-10000.0);
        let expected = Vec2::new(
            step_axis(geometry.axes[0], app.fs_pan.x, delta.x),
            step_axis(geometry.axes[1], app.fs_pan.y, delta.y),
        );
        drag(&mut app, &ctx, delta);
        close(app.fs_pan, expected);
    }
}

#[cfg(windows)]
pub(super) fn render_frame(
    app: &mut App,
    ctx: &egui::Context,
    rect: Rect,
    dpi: f32,
    events: Vec<egui::Event>,
) -> Vec<DisplayedImageTransform> {
    assert!(
        app.fs_prev_focused && app.fs_focus_regained_at.is_none(),
        "full-render drag fixtures must start already focused, without a focus-regain gesture"
    );
    app.frame_counter += 1;
    crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), Vec::new());
    let output = ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(rect),
            focused: true,
            // Use egui's previous time + predicted_dt, just like frame() and
            // run_in_viewport(). Mixing f64 frame_counter/60 with that f32
            // duration can otherwise move time backwards by a fraction of ns.
            events,
            viewports: [(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(dpi),
                    focused: Some(true),
                    minimized: Some(false),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|ui| app.render_fullscreen_viewport_with_root_input(ui, None));
        },
    );
    crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
    app.fullscreen_page_layout
        .displayed_pages()
        .map(|page| page.transform)
        .collect()
}

#[cfg(windows)]
struct KeyboardCleanup;
#[cfg(windows)]
impl Drop for KeyboardCleanup {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

#[cfg(windows)]
#[test]
fn drag_bound_fractional_zoom_dpi_edges_settle_after_real_repaint() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    for dpi in [1.0, 1.25, 1.5] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr] {
            let ctx = egui::Context::default();
            let mut app = scroll_app(&ctx, mode);
            app.settings.fullscreen_bound_drag_pan = true;
            app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
            app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
            // This suite drags in an already focused viewer. App's default
            // false would model a focus-regain click, which production owns
            // for 300 ms using Instant and suppresses through its release.
            app.fs_prev_focused = true;
            app.fs_zoom = 2.317;
            let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 800.0));
            render_frame(&mut app, &ctx, rect, dpi, vec![]);
            render_frame(
                &mut app,
                &ctx,
                rect,
                dpi,
                vec![egui::Event::PointerMoved(origin())],
            );
            render_frame(&mut app, &ctx, rect, dpi, vec![button(origin(), true)]);
            let end = origin() + Vec2::splat(-10000.0);
            let mut samples = Vec::new();
            for _ in 0..10 {
                let pages = render_frame(
                    &mut app,
                    &ctx,
                    rect,
                    dpi,
                    vec![egui::Event::PointerMoved(end)],
                );
                assert!(!pages.is_empty());
                assert!(app.fs_scroll_layout_matches(&ctx));
                let unit = pages
                    .iter()
                    .map(|page| fs_navigator_source_rect_aabb(page, page.uv_rect))
                    .reduce(Rect::union)
                    .unwrap();
                samples.push((app.fs_pan, unit));
            }
            assert!(
                app.fs_pan_drag_start.is_some(),
                "the actual fractional-zoom canvas must own the drag: dpi={dpi}, mode={mode:?}, samples={samples:?}"
            );
            for pair in samples[3..].windows(2) {
                assert_eq!(pair[0], pair[1], "dpi={dpi}, mode={mode:?} must settle");
            }
            let unit = samples.last().unwrap().1;
            let viewport = app
                .fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform
                .viewport_rect;
            let right_residual_px = (unit.right() - viewport.right()) * dpi;
            let bottom_residual_px = (unit.bottom() - viewport.bottom()) * dpi;
            let pages: Vec<_> = app
                .fullscreen_page_layout
                .displayed_pages()
                .map(|page| page.transform)
                .collect();
            assert!(
                right_residual_px.abs() <= 2.01,
                "right residual={right_residual_px}px, bottom residual={bottom_residual_px}px, dpi={dpi}, mode={mode:?}, viewport={viewport:?}, unit={unit:?}, pan={:?}, drag_start={:?}, suppression={:?}, samples={samples:?}, pages={pages:?}",
                app.fs_pan,
                app.fs_pan_drag_start,
                app.fs_primary_suppression
            );
            assert!(
                bottom_residual_px.abs() <= 2.01,
                "bottom residual={bottom_residual_px}px, right residual={right_residual_px}px, dpi={dpi}, mode={mode:?}, viewport={viewport:?}, unit={unit:?}, pan={:?}, samples={samples:?}, pages={pages:?}",
                app.fs_pan
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn drag_bound_odd_width_spread_settles_at_painted_edge() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = scroll_app(&ctx, SpreadMode::Ltr);
    app.settings.fullscreen_bound_drag_pan = true;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.spread_page_gap_px = 0;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_prev_focused = true;
    install_material(&mut app, &ctx, 0, [1000, 1500]);
    install_material(&mut app, &ctx, 1, [1001, 1500]);
    let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(1000.0));
    let initial = render_frame(&mut app, &ctx, rect, 1.0, vec![]);
    assert_eq!(initial.len(), 2);
    close(
        Vec2::new(initial[0].paint_rect.width(), initial[1].paint_rect.width()),
        Vec2::new(1000.0, 1001.0),
    );
    render_frame(
        &mut app,
        &ctx,
        rect,
        1.0,
        vec![egui::Event::PointerMoved(origin())],
    );
    render_frame(&mut app, &ctx, rect, 1.0, vec![button(origin(), true)]);
    let end = origin() - Vec2::new(10000.0, 0.0);
    let mut samples = Vec::new();
    for _ in 0..10 {
        let pages = render_frame(
            &mut app,
            &ctx,
            rect,
            1.0,
            vec![egui::Event::PointerMoved(end)],
        );
        assert!(app.fs_scroll_layout_matches(&ctx));
        let edge = pages
            .iter()
            .map(|page| page.paint_rect.right())
            .fold(f32::NEG_INFINITY, f32::max);
        samples.push((app.fs_pan, edge));
    }
    assert!(
        app.fs_pan_drag_start.is_some(),
        "the actual odd-width spread canvas must own the drag: samples={samples:?}"
    );
    for pair in samples[3..].windows(2) {
        assert_eq!(pair[0], pair[1]);
    }
    let right_residual_px = samples.last().unwrap().1 - initial[0].viewport_rect.right();
    let pages: Vec<_> = app
        .fullscreen_page_layout
        .displayed_pages()
        .map(|page| page.transform)
        .collect();
    assert!(
        right_residual_px.abs() <= 2.01,
        "right residual={right_residual_px}px, viewport={:?}, pan={:?}, drag_start={:?}, suppression={:?}, initial={initial:?}, samples={samples:?}, pages={pages:?}",
        initial[0].viewport_rect,
        app.fs_pan,
        app.fs_pan_drag_start,
        app.fs_primary_suppression
    );
}

#[cfg(windows)]
#[test]
fn drag_bound_tiny_overflow_crosses_both_actual_painted_edges_and_returns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    for dpi in [1.0, 1.25, 1.5] {
        // At 1.5 DPI, 1000/1.5 points is rounded by egui's 1/32-point
        // layout grid to 999.984375 physical pixels. Use 1002 physical
        // pixels (668 points) so both viewport and image lie on the true
        // integer grid; the overflow itself is still exactly 1/2/3 pixels.
        let viewport_px = if dpi == 1.5 { 1002 } else { 1000 };
        for overflow_px in [1, 2, 3] {
            for axis in 0..2 {
                let ctx = egui::Context::default();
                let mut app = scroll_app(&ctx, SpreadMode::Single);
                app.settings.fullscreen_bound_drag_pan = true;
                app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
                app.settings.fullscreen_side_panel_mode =
                    crate::settings::FsSidePanelMode::ClickToShow;
                app.fs_prev_focused = true;
                let mut size = [viewport_px; 2];
                size[axis] += overflow_px;
                install_material(&mut app, &ctx, 0, size);
                let rect =
                    Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(viewport_px as f32 / dpi));
                let initial = render_frame(&mut app, &ctx, rect, dpi, vec![]);
                assert_eq!(initial.len(), 1);
                let viewport = initial[0].viewport_rect;
                let initial_unit = initial[0].paint_rect;
                assert!(
                    (viewport.size()[axis] * dpi - viewport_px as f32).abs() < 1.0e-3,
                    "integer viewport: dpi={dpi}, axis={axis}, viewport={viewport:?}"
                );
                assert!(
                    (initial_unit.size()[axis] * dpi - size[axis] as f32).abs() < 1.0e-3,
                    "integer image: dpi={dpi}, axis={axis}, page={:?}",
                    initial[0]
                );
                let min_pan = viewport.max[axis] - initial_unit.max[axis];
                let max_pan = viewport.min[axis] - initial_unit.min[axis];
                assert!(
                    ((max_pan - min_pan) * dpi - overflow_px as f32).abs() < 1.0e-3,
                    "initial physical travel: dpi={dpi}, axis={axis}, min={min_pan}, max={max_pan}, viewport={viewport:?}, unit={initial_unit:?}"
                );
                let mut end_pans = Vec::new();
                for sign in [-1.0, 1.0, -1.0] {
                    render_frame(
                        &mut app,
                        &ctx,
                        rect,
                        dpi,
                        vec![egui::Event::PointerMoved(origin())],
                    );
                    render_frame(&mut app, &ctx, rect, dpi, vec![button(origin(), true)]);
                    let mut delta = Vec2::ZERO;
                    delta[axis] = sign * 10000.0;
                    let end = origin() + delta;
                    let mut samples = Vec::new();
                    for _ in 0..10 {
                        let pages = render_frame(
                            &mut app,
                            &ctx,
                            rect,
                            dpi,
                            vec![egui::Event::PointerMoved(end)],
                        );
                        assert_eq!(pages.len(), 1);
                        assert!(app.fs_scroll_layout_matches(&ctx));
                        samples.push((app.fs_pan, pages[0].paint_rect));
                    }
                    assert!(
                        app.fs_pan_drag_start.is_some(),
                        "the actual canvas must own the tiny-range drag"
                    );
                    for pair in samples[3..].windows(2) {
                        assert_eq!(
                            pair[0], pair[1],
                            "tiny edge must settle: dpi={dpi}, overflow={overflow_px}, axis={axis}, sign={sign}, samples={samples:?}"
                        );
                    }
                    let (pan, unit) = samples.last().copied().unwrap();
                    let expected_pan = if sign < 0.0 { min_pan } else { max_pan };
                    let edge_error_px = if sign < 0.0 {
                        (unit.max[axis] - viewport.max[axis]) * dpi
                    } else {
                        (unit.min[axis] - viewport.min[axis]) * dpi
                    };
                    assert!(
                        (pan[axis] - expected_pan).abs() * dpi < 1.0e-3,
                        "tiny pan: dpi={dpi}, overflow={overflow_px}, axis={axis}, sign={sign}, expected={expected_pan}, samples={samples:?}"
                    );
                    assert!(
                        edge_error_px.abs() < 1.0e-3,
                        "tiny edge gap={edge_error_px}px: dpi={dpi}, overflow={overflow_px}, axis={axis}, sign={sign}, viewport={viewport:?}, samples={samples:?}"
                    );
                    assert_eq!(pan[1 - axis], 0.0);
                    render_frame(&mut app, &ctx, rect, dpi, vec![button(end, false)]);
                    assert_eq!(
                        app.fs_pan, pan,
                        "release must retain the tiny-range endpoint"
                    );
                    end_pans.push(pan);
                }
                assert!(
                    ((end_pans[1][axis] - end_pans[0][axis]) * dpi - overflow_px as f32).abs()
                        < 1.0e-3,
                    "the entire tiny physical range must remain usable: dpi={dpi}, overflow={overflow_px}, axis={axis}, endpoints={end_pans:?}"
                );
                assert_eq!(
                    end_pans[0], end_pans[2],
                    "returning to the same edge must not drift"
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn drag_bound_blank_companion_drag_is_gradual_and_survives_actual_draw_correction() {
    use crate::book_composition::{PageOverride, PageOverrideEntry, PageSide, fixtures};
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    for page_side in [PageSide::Leading, PageSide::Trailing] {
        let ctx = egui::Context::default();
        let mut book = fixtures::four_page_book(crate::book_composition::ReadingDirection::Ltr);
        book.page_overrides.push(PageOverrideEntry {
            page: fixtures::document_page(0),
            directive: PageOverride::BlankOpposite { page_side },
        });
        let mut app = crate::ui_fullscreen::book_composition_tests::composed_app(&book);
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.window_visible = true;
        app.settings.video_in_window_mode = true;
        app.settings.detached_viewer_enabled = false;
        app.settings.still_seek_strip_visible = false;
        app.settings.margin_fit_enabled = false;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.page_turn_animation_enabled = false;
        app.settings.fullscreen_bound_drag_pan = true;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
        app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
        app.fs_prev_focused = true;
        app.fs_zoom = 4.0;
        for idx in 0..app.items.len() {
            install_material(&mut app, &ctx, idx, [80, 120]);
        }
        let demand = app.book_target_presentation_demand(0);
        app.install_allowed_presentation_for_test(&demand);
        install_fs_navigator_input_tracking(&ctx);
        let initial = render_frame(&mut app, &ctx, full_rect(), 1.0, vec![]);
        assert_eq!(initial.len(), 1);
        let white = white_companion_rect(&initial[0])
            .expect("explicit composition paints a white companion");
        let pan = Vec2::new(initial[0].viewport_rect.center().x - white.center().x, 0.0);
        app.fs_pan = pan;
        let pages = render_frame(&mut app, &ctx, full_rect(), 1.0, vec![]);
        assert_eq!(app.fs_pan, pan);
        assert!(
            !pages[0]
                .paint_rect
                .intersect(pages[0].viewport_rect)
                .is_positive()
        );
        assert!(app.fs_scroll_layout_matches(&ctx));
        let side_sign = if white.center().x < initial[0].paint_rect.center().x {
            1.0
        } else {
            -1.0
        };
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![egui::Event::PointerMoved(origin())],
        );
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![button(origin(), true)],
        );
        let inward = Vec2::new(-side_sign * 10.0, 0.0);
        // Centering the white slot leaves it fully visible. Moving 10 points
        // toward the real page stays within both the unit edge and that white
        // element's minimum-visible box; this expected movement is intentional.
        close(
            drag_bound::bounded_drag_pan(&app.fullscreen_page_layout, pan, pan + inward),
            pan + inward,
        );
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![egui::Event::PointerMoved(origin() + inward)],
        );
        assert!(
            app.fs_pan_drag_start.is_some(),
            "actual canvas must own the white-slot drag: side={page_side:?}, pan={:?}, expected={:?}, focused={}, focus_regained={:?}, suppression={:?}, initial={initial:?}, white={white:?}",
            app.fs_pan,
            pan + inward,
            app.fs_prev_focused,
            app.fs_focus_regained_at,
            app.fs_primary_suppression
        );
        close(app.fs_pan, pan + inward);
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![button(origin() + inward, false)],
        );
        close(app.fs_pan, pan + inward);
        let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
        let edge = if side_sign < 0.0 {
            geometry.axes[0].unwrap().min
        } else {
            geometry.axes[0].unwrap().max
        };
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![egui::Event::PointerMoved(origin())],
        );
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![button(origin(), true)],
        );
        let end = origin() + Vec2::new(side_sign * 10000.0, 0.0);
        for _ in 0..10 {
            render_frame(
                &mut app,
                &ctx,
                full_rect(),
                1.0,
                vec![egui::Event::PointerMoved(end)],
            );
            assert!((app.fs_pan.x - edge).abs() <= 2.01);
            assert_eq!(app.fs_pan.y, pan.y);
        }
    }
}

#[test]
fn drag_bound_single_finger_passthrough_uses_the_canvas_bound_setting() {
    fn touch(phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(834),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        }
    }
    let outcomes = [false, true].map(|bound| {
        let (ctx, mut app) = prepared(bound, FullscreenFitMode::Page, 1.0);
        app.settings.touch_still_chrome_learned = true;
        pointer_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(origin())]);
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(egui::TouchPhase::Start, origin()),
                egui::Event::PointerMoved(origin()),
                button(origin(), true),
            ],
        );
        let end = origin() + Vec2::new(80.0, 40.0);
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(egui::TouchPhase::Move, end),
                egui::Event::PointerMoved(end),
            ],
        );
        let moved = app.fs_pan;
        pointer_frame(
            &mut app,
            &ctx,
            vec![
                touch(egui::TouchPhase::End, end),
                button(end, false),
                egui::Event::PointerGone,
            ],
        );
        assert_eq!(app.fs_pan, moved);
        assert!(app.fs_pan_drag_start.is_none());
        moved
    });
    assert_ne!(outcomes[0], Vec2::ZERO);
    assert_eq!(outcomes[1], Vec2::ZERO);
}

#[cfg(windows)]
#[test]
fn drag_bound_provisional_to_complete_material_waits_for_new_paint_then_resumes() {
    use crate::gpu_lanczos::FullscreenPageMaterialKind;
    use crate::ui_fullscreen::initial_placement::tests::{
        install_arriving_full, thumbnail_arrival_app,
    };
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Width);
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.native_video_in_window_active = true;
    app.settings.video_in_window_mode = true;
    app.settings.detached_viewer_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.settings.margin_fit_enabled = false;
    app.settings.page_turn_animation_enabled = false;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.fullscreen_bound_drag_pan = true;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_prev_focused = true;
    app.window_visible = true;
    app.fs_zoom = 2.0;
    // Hold loader ingress while the test owns material completion.
    let mut load_senders = Vec::new();
    for idx in 0..app.items.len() {
        let (tx, rx) = std::sync::mpsc::channel();
        app.fs_pending.insert(
            idx,
            crate::app::FsPendingValue::new(
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                rx,
                0,
                crate::app::FsLoadPurpose::for_page(false),
            ),
        );
        load_senders.push(tx);
    }
    install_fs_navigator_input_tracking(&ctx);
    render_frame(&mut app, &ctx, full_rect(), 1.0, vec![]);
    frame(&ctx, vec![], |_, ctx| {
        let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
        assert!(state.tex.is_none());
        assert_eq!(
            state.thumb_tex.as_ref().unwrap().page_material_kind(),
            FullscreenPageMaterialKind::Provisional
        );
    });
    assert!(app.fullscreen_page_layout.drawn_unit.is_some());
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(origin())],
    );
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![button(origin(), true)],
    );
    let before = app.fs_pan;
    app.cancel_fs_page_load(0);
    install_arriving_full(&mut app, &ctx, 0);
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
    frame(&ctx, vec![], |_, ctx| {
        let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
        assert_eq!(
            state.tex.as_ref().unwrap().page_material_kind(),
            FullscreenPageMaterialKind::Complete
        );
        assert!(!app.fs_scroll_layout_matches(ctx));
    });
    let end = origin() - Vec2::new(0.0, 30.0);
    render_frame(
        &mut app,
        &ctx,
        full_rect(),
        1.0,
        vec![egui::Event::PointerMoved(end)],
    );
    close(app.fs_pan, before);
    assert!(app.fs_scroll_layout_matches(&ctx));
    render_frame(&mut app, &ctx, full_rect(), 1.0, vec![]);
    close(app.fs_pan, before - Vec2::new(0.0, 30.0));
    render_frame(&mut app, &ctx, full_rect(), 1.0, vec![button(end, false)]);
    close(app.fs_pan, before - Vec2::new(0.0, 30.0));
    drop(load_senders);
}

#[cfg(windows)]
#[test]
fn drag_bound_blocked_drag_release_is_not_click_but_next_single_click_turns() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Page, 1.0);
    app.request_fs_initial_placement(0);
    seed(&ctx, &mut app);
    let before = app.fs_pan;
    let automatic = app.fs_automatic_initial_view;
    drag(&mut app, &ctx, Vec2::new(80.0, 0.0));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.fs_pan, before);
    assert_eq!(app.fs_automatic_initial_view, automatic);
    // Separate the subsequent single click from egui's double-click interval.
    for _ in 0..40 {
        pointer_frame(&mut app, &ctx, vec![]);
    }
    press(&mut app, &ctx);
    let nav = pointer_frame(&mut app, &ctx, vec![button(origin(), false)]);
    assert!(
        !nav.is_none(),
        "the unchanged automatic origin permits a single click"
    );
}

#[test]
fn drag_bound_moved_drag_keeps_next_single_click_on_current_page() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    app.request_fs_initial_placement(0);
    seed(&ctx, &mut app);
    let before = app.fs_pan;
    drag(&mut app, &ctx, Vec2::new(0.0, 40.0));
    assert_ne!(app.fs_pan, before);
    for _ in 0..40 {
        pointer_frame(&mut app, &ctx, vec![]);
    }
    press(&mut app, &ctx);
    assert!(pointer_frame(&mut app, &ctx, vec![button(origin(), false)]).is_none());
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[cfg(windows)]
#[test]
fn drag_bound_animation_changes_only_when_logical_pan_changes() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    for blocked in [false, true] {
        let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 1.0);
        app.settings.fullscreen_smooth_page_scroll = true;
        frame(&ctx, vec![], |_, ctx| {
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown)
        });
        let target = app.fs_pan;
        let animation = app.fs_page_scroll_animation.clone().unwrap();
        // At the initial animation sample, the painted pan differs from cur.
        seed(&ctx, &mut app);
        assert_ne!(app.fs_page_scroll_display_pan(&ctx), target);
        let painted = normal_zoom_pan(
            app.fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform,
        )
        .unwrap()
        .1;
        assert_ne!(
            painted, target,
            "the actual transform must contain the interpolated pan"
        );
        close(painted, app.fs_page_scroll_display_pan(&ctx));
        drag(
            &mut app,
            &ctx,
            if blocked {
                Vec2::new(80.0, 0.0)
            } else {
                Vec2::new(0.0, 40.0)
            },
        );
        frame(&ctx, vec![], |_, ctx| {
            app.retire_fs_page_scroll_animation(ctx)
        });
        if blocked {
            assert_eq!(app.fs_pan, target);
            let actual = app.fs_page_scroll_animation.as_ref().unwrap();
            assert_eq!(actual.to, animation.to);
            assert_eq!(actual.origin, animation.origin);
        } else {
            close(app.fs_pan, target + Vec2::new(0.0, 40.0));
            assert!(app.fs_page_scroll_animation.is_none());
            seed(&ctx, &mut app);
            drag(&mut app, &ctx, Vec2::new(0.0, -10000.0));
            let geometry = scroll_geometry(&app.fullscreen_page_layout).unwrap();
            assert!((app.fs_pan.y - geometry.axes[1].unwrap().min).abs() < 1.0);
        }
    }
}

#[cfg(windows)]
#[test]
fn drag_bound_child_drag_preserves_all_sibling_owned_view_state() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Width, 3.0);
    app.settings.fullscreen_smooth_page_scroll = true;
    frame(&ctx, vec![], |_, ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown)
    });
    let pan = app.fs_pan;
    let zoom = app.fs_zoom;
    let stamp = app.fullscreen_page_layout.drawn_unit.clone();
    let animation = app.fs_page_scroll_animation.clone().unwrap();
    app.request_fs_initial_placement(0);
    let request = app.fs_initial_placement_request;
    app.fs_pan_drag_start = Some((origin(), Vec2::new(17.0, 23.0)));
    let start = app.fs_pan_drag_start;
    let items = app.items.clone();
    let child = app.build_window_context_for_test(1834001, |owner| {
        owner.items = items;
        owner.visible_indices = (0..owner.items.len()).collect();
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = SpreadMode::Single;
        owner.reading_flow = ReadingFlow::Paged;
        for idx in 0..owner.items.len() {
            install_material(owner, &ctx, idx, [80, 120]);
        }
        owner.fs_pan = Vec2::ZERO;
        owner.fs_zoom = 1.0;
        owner.fs_page_scroll_animation = None;
        owner.fs_pan_drag_start = None;
        owner.clear_fs_initial_placement_request();
    });
    app.with_window_context_for_test(child, |owner| {
        // The existing mounted-context fixture exercises ownership without
        // changing detached predicates or creating native windows.
        for separate_viewport in [false, true] {
            owner.native_video_in_window_active = !separate_viewport;
            owner.settings.video_in_window_mode = !separate_viewport;
            let viewport = owner.fs_page_scroll_destination_viewport();
            assert_eq!(viewport == egui::ViewportId::ROOT, !separate_viewport);
            run_in_viewport(&ctx, viewport, full_rect(), 1.0, vec![], |ui, ctx| {
                paint(owner, ui, ctx);
            });
            let before = owner.fs_pan;
            let end = origin() + Vec2::new(0.0, -40.0);
            for events in [
                vec![egui::Event::PointerMoved(origin())],
                vec![button(origin(), true)],
                vec![egui::Event::PointerMoved(end)],
                vec![button(end, false)],
            ] {
                run_in_viewport(&ctx, viewport, full_rect(), 1.0, events, |ui, ctx| {
                    let state =
                        owner.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let (nav, closed) = owner.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        full_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                    assert!(nav.is_none() && !closed);
                });
            }
            assert_ne!(owner.fs_pan, before);
        }
    });
    assert_eq!(app.fs_pan, pan);
    assert_eq!(app.fs_zoom, zoom);
    assert_eq!(app.fullscreen_page_layout.drawn_unit, stamp);
    assert_eq!(app.fs_initial_placement_request, request);
    assert_eq!(app.fs_pan_drag_start, start);
    let actual = app.fs_page_scroll_animation.as_ref().unwrap();
    assert_eq!(actual.from, animation.from);
    assert_eq!(actual.to, animation.to);
    assert_eq!(actual.start_time, animation.start_time);
    assert_eq!(actual.last_painted_pan, animation.last_painted_pan);
    assert_eq!(actual.last_painted_time, animation.last_painted_time);
    assert_eq!(actual.origin, animation.origin);
    assert_eq!(actual.viewport, animation.viewport);
}
