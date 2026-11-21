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
fn drag_bound_on_fitting_image_keeps_noncentral_pan() {
    let (ctx, mut app) = prepared(true, FullscreenFitMode::Page, 1.0);
    app.fs_pan = Vec2::new(19.0, -23.0);
    seed(&ctx, &mut app);
    let before = app.fs_pan;
    drag(&mut app, &ctx, Vec2::new(80.0, 70.0));
    close(app.fs_pan, before);
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
    pointer_frame(&mut app, &ctx, vec![]);
    assert_eq!(app.fs_pan, before);
    pointer_frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(origin() + Vec2::new(10.0, 0.0))],
    );
    close(app.fs_pan, before + Vec2::new(10.0, 0.0));
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
        assert!(!app.fs_bounded_drag_pan_surface(ctx))
    });
    app.spread_mode = SpreadMode::Ltr;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_bounded_drag_pan_surface(ctx))
    });
    app.spread_mode = SpreadMode::Single;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(!app.fs_bounded_drag_pan_surface(ctx))
    });
    app.fullscreen_page_layout.drawn_unit = None;
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.fs_bounded_drag_pan_surface(ctx))
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
                app.fs_bounded_drag_pan_surface(ctx),
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
        assert!(app.fs_bounded_drag_pan_surface(ctx))
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
            assert!(!app.fs_bounded_drag_pan_surface(ctx))
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
            assert!(app.fs_bounded_drag_pan_surface(ctx))
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
    app.frame_counter += 1;
    crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), Vec::new());
    let output = ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(rect),
            focused: true,
            time: Some(app.frame_counter as f64 / 60.0),
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
            assert!((unit.right() - viewport.right()).abs() * dpi <= 2.01);
            assert!((unit.bottom() - viewport.bottom()).abs() * dpi <= 2.01);
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
    for pair in samples[3..].windows(2) {
        assert_eq!(pair[0], pair[1]);
    }
    assert!((samples.last().unwrap().1 - initial[0].viewport_rect.right()).abs() <= 2.01);
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
        render_frame(
            &mut app,
            &ctx,
            full_rect(),
            1.0,
            vec![egui::Event::PointerMoved(origin() + inward)],
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
