//! Real material/layout receipts and input handlers, without launching a window.
use super::*;
use crate::settings::{FullscreenNScrollOrder, FullscreenUpDownKeyAction, FullscreenWheelAction};
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, ready_app};

#[cfg(windows)]
struct KeyboardCleanup;

#[cfg(windows)]
impl Drop for KeyboardCleanup {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

pub(super) fn app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.native_video_in_window_active = true;
    app.settings.video_in_window_mode = true;
    app.settings.detached_viewer_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.settings.margin_fit_enabled = false;
    app.settings.page_turn_animation_enabled = false;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
    app.settings.fullscreen_up_down_key_action = FullscreenUpDownKeyAction::InPageScroll;
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::InPageScroll;
    app.window_visible = true;
    install_fs_navigator_input_tracking(ctx);
    assert!(app.fullscreen_embedded_still_active());
    app
}

pub(super) fn full_rect() -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))
}

pub(super) fn run_in_viewport(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    rect: Rect,
    dpi: f32,
    events: Vec<egui::Event>,
    mut body: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(rect),
        focused: true,
        events,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            native_pixels_per_point: Some(dpi),
            focused: Some(true),
            minimized: Some(false),
            ..Default::default()
        },
    );
    let output = ctx.run_pass(raw, |ctx| {
        ctx.root_ui(|root| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root, |ui| body(ui, ctx));
        });
    });
    crate::test_ui::headless_paint_output(output).drop_without_applying_deltas();
}

pub(super) fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    body: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    run_in_viewport(ctx, egui::ViewportId::ROOT, full_rect(), 1.0, events, body);
}

pub(super) fn paint(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> (SpreadDisplayComposition, FsNavigatorTextureSources) {
    let idx = app.fullscreen_idx.unwrap();
    let viewport = app.fullscreen_media_rect(ctx.content_rect(), idx, false);
    let mut painted = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(viewport), |ui| {
        // The page-turn fixture intentionally hardcodes Page/zoom=None for a
        // single page. Exercise the ordinary renderer with this test's live
        // fit, zoom, trim and rotation instead.
        app.fs_page_turn_decision_for_frame(ctx, idx);
        let composition = app.spread_display_composition_for_anchor(idx);
        let screen = composition.pages_in_screen_order(app.spread_mode);
        if let [page] = screen.as_slice() {
            let material = app.resolve_fs_processed_material(ctx, idx, false);
            let payload = app.fullscreen_paint_payload_for_page(idx, material, false);
            let source_size = app.fs_page_coordinate_source_size(idx);
            let rotation = app.get_rotation(idx);
            let zoom_pan = app.fs_zoom_pan();
            let free_rotation = app.fs_free_rotation;
            let fit = app.effective_fullscreen_fit_mode();
            let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
            let trim = app.view_trim_single_content_bbox(idx);
            let mut sources = FsNavigatorTextureSources::default();
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Single);
            let transform = app
                .draw_fs_image_with_receipt(
                    ui,
                    viewport,
                    idx,
                    source_size,
                    FsPageLayoutSource::CurrentItem,
                    None,
                    None,
                    composition.singleton_placement(),
                    payload.as_ref(),
                    None,
                    false,
                    false,
                    false,
                    rotation,
                    zoom_pan,
                    free_rotation,
                    &FsBgStyle::Solid(egui::Color32::WHITE),
                    "",
                    false,
                    fit,
                    limits,
                    trim,
                    Some(&mut sources),
                    false,
                    None,
                    page_turn::FlatCropPaint::WithPage,
                )
                .expect("ordinary single page paints");
            app.fullscreen_page_layout.push_occurrence(transform, *page);
            assert!(sources.rebind_occurrences(&screen, || {
                app.page_turn_unit_binding(page.navigation_anchor_idx)
            }));
            painted = Some((composition, sources));
        } else {
            painted = Some(draw_current(app, ui, ctx));
        }
    });
    let (composition, sources) = painted.unwrap();
    app.record_fs_scroll_drawn_unit(ctx, viewport, &composition, &sources);
    assert!(
        app.fullscreen_page_layout.drawn_unit.is_some(),
        "complete current paint records a receipt"
    );
    let matches = app.fs_scroll_layout_matches(ctx);
    let input = app.fs_scroll_input_geometry(ctx);
    assert!(
        matches,
        "a freshly painted fixture must match its input metadata: drawn={:?}, input={:?}, zoom={:?}, navigation={}, barrier={}",
        app.fullscreen_page_layout.drawn_unit,
        input,
        app.fs_zoom_pan(),
        app.fs_navigation_sequence_blocks_new_target(),
        app.page_turn_barrier_active()
    );
    (composition, sources)
}

pub(super) fn seed(ctx: &egui::Context, app: &mut App) {
    frame(ctx, vec![], |ui, ctx| {
        paint(app, ui, ctx);
    });
}

#[test]
fn line_wheel_in_page_movement_and_edge_stop_repainting_within_three_passes() {
    use crate::wheel_input::tests::{repaint_delay, smoothing_input};
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for edge in [false, true] {
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        if edge {
            frame(&ctx, vec![], |_, ctx| {
                let zoom = app.fs_zoom;
                let pan = app.fs_pan;
                app.fs_pan =
                    app.fs_page_scroll_wheel_pan(ctx, zoom, pan, Vec2::new(0.0, -100_000.0));
            });
            seed(&ctx, &mut app);
        }
        let mut before = Vec2::ZERO;
        let mut after = Vec2::ZERO;
        let mut stopped = None;
        // Warm and settle the same production merge/paint path, with empty idle input.
        for pass in 0..63 {
            let input_pass = pass == 60;
            let events = if pass == 0 {
                vec![egui::Event::PointerMoved(full_rect().center())]
            } else if input_pass {
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: Vec2::new(0.0, -1.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                }]
            } else {
                vec![]
            };
            let output = crate::test_ui::headless_paint_output(ctx.run_pass(
                smoothing_input(pass, events),
                |ctx| {
                    ctx.root_ui(|root| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(root, |ui| {
                                if pass > 60 {
                                    ctx.input(|i| {
                                        assert!(i.events.is_empty());
                                        assert_eq!(i.smooth_scroll_delta, Vec2::ZERO);
                                        assert_eq!(i.zoom_delta(), 1.0);
                                    });
                                }
                                let expected = if input_pass {
                                    before = app.fs_pan;
                                    let speed = ctx.options(|o| o.input_options.line_scroll_speed);
                                    let zoom = app.fs_zoom;
                                    Some(app.fs_page_scroll_wheel_pan(
                                        ctx,
                                        zoom,
                                        before,
                                        Vec2::new(0.0, -speed),
                                    ))
                                } else {
                                    None
                                };
                                let (result, _) =
                                    crate::ui_fullscreen::wheel_frame_tests::actual_merge(
                                        &mut app, ui, false,
                                    );
                                assert!(result.page_nav.is_none());
                                if let Some(expected) = expected {
                                    after = app.fs_pan;
                                    assert_eq!(after, expected, "raw movement is preserved");
                                }
                                paint(&mut app, ui, ctx);
                            });
                    });
                },
            ));
            if pass == 59 {
                assert_ne!(
                    repaint_delay(&output),
                    std::time::Duration::ZERO,
                    "fixture must settle before the notch"
                );
            }
            if pass >= 60 && repaint_delay(&output) != std::time::Duration::ZERO {
                stopped.get_or_insert(pass - 60 + 1);
            }
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            before == after,
            edge,
            "edge={edge}, before={before:?}, after={after:?}"
        );
        assert!(
            stopped.is_some_and(|passes| passes <= 3),
            "edge={edge}, stopped={stopped:?}"
        );
    }
}

fn painted_unit(app: &App) -> Rect {
    app.fullscreen_page_layout
        .displayed_pages()
        .map(|page| {
            page.transform
                .paint_quad()
                .positions
                .into_iter()
                .fold(Rect::NOTHING, |rect, point| {
                    rect.union(Rect::from_min_max(point, point))
                })
        })
        .reduce(Rect::union)
        .expect("the fixture paints a complete unit")
}

fn wide_unit_rect() -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 400.0))
}

fn wide_unit_frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    body: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    run_in_viewport(
        ctx,
        egui::ViewportId::ROOT,
        wide_unit_rect(),
        1.0,
        events,
        body,
    );
}

fn wide_unit_app(ctx: &egui::Context, zoom: f32) -> crate::app::AppTestEnvForTest {
    let mut app = app(ctx, SpreadMode::Single);
    // A 25:1 page fitted to 400pt high paints a 10000pt-wide unit without
    // inventing a raster above the normal GPU material size limit.
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Height;
    app.settings.continuous_reading_key_scroll_percent = 100;
    app.settings.continuous_reading_wheel_scroll_percent = 20;
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageScrollRight = F24\n");
    replace_texture(&mut app, ctx, [1000, 40]);
    let FsCacheEntry::Static { source_dims, .. } = app.fs_cache.get_mut(&0).unwrap() else {
        panic!("static original fixture")
    };
    *source_dims = Some([1000, 40]);
    app.record_page_dims_for_spread(0, (1000, 40));
    app.fs_zoom = zoom;
    wide_unit_frame(ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        assert_eq!(painted_unit(&app).width(), 10_000.0);
        assert_eq!(painted_unit(&app).right(), 5500.0);
        assert_eq!(
            app.fs_zoom_pan(),
            None,
            "the fixture starts with a normalized fit placement"
        );
    });
    app
}

fn key(key: egui::Key, pressed: bool, repeat: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers,
    }
}

pub(super) fn tap(key_name: egui::Key) -> Vec<egui::Event> {
    vec![
        key(key_name, true, false, egui::Modifiers::NONE),
        key(key_name, false, false, egui::Modifiers::NONE),
    ]
}

fn wheel(delta: Vec2, modifiers: egui::Modifiers) -> crate::wheel_input::WheelInput {
    crate::wheel_input::WheelInput {
        delta,
        modifiers,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    }
}

pub(super) fn wheel_event(delta: Vec2, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    }
}

fn action(app: &App, input: crate::wheel_input::WheelInput, ordinal: u32) -> FsWheelAction {
    FsWheelAction {
        input,
        segment: crate::wheel_input::WheelSegment {
            input,
            first_raw_ordinal: Some(ordinal),
            last_raw_ordinal: Some(ordinal),
        },
        operation: None,
        class: app.fs_wheel_segment_in_page_scroll(input, false),
        scroll_origin: app.fullscreen_page_layout.drawn_unit.clone(),
        seek_hover: false,
    }
}

pub(super) fn deferred_wheel_proposal(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsWheelFrameProposal {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let geometry = app.still_seek_geometry_for_idx(full_rect(), idx, state.is_video);
    let (front_pointer, touch_excluded) = app.capture_fs_front_pointer_input(ctx, full_rect());
    let primary = app.advance_viewer_primary_pointer_frame(ctx, front_pointer.primary_owned);
    let result = app.handle_fs_wheel_and_click_frame(
        ui,
        ctx,
        full_rect(),
        &state,
        geometry,
        false,
        0,
        &mut None,
        None,
        primary,
        false,
        &mut None,
        false,
        true,
        front_pointer,
        &touch_excluded,
    );
    assert!(
        result.wheel_navigation_produced,
        "the Alt segment enters the real mixed-wheel route"
    );
    result
        .deferred
        .expect("mixed input must defer until the frame tail")
}

fn replace_texture(app: &mut App, ctx: &egui::Context, size: [usize; 2]) {
    let pixels = egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE);
    let texture = ctx.load_texture(
        "scroll-replacement-material",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let FsCacheEntry::Static {
        tex,
        pixels: stored_pixels,
        ..
    } = app.fs_cache.get_mut(&0).unwrap()
    else {
        panic!("static fixture")
    };
    *tex = texture;
    *stored_pixels = Arc::new(pixels);
    // Retire the old accepted composite as a real producer replacement does.
    // Merely changing fs_cache would leave its previously adopted final visible.
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
}

#[test]
fn current_receipt_rejects_missing_generation_other_page_and_recovers_after_new_paint() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |ui, ctx| {
        let original = app.fullscreen_page_layout.drawn_unit.clone();
        app.fullscreen_page_layout.drawn_unit = None;
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert_eq!(app.fs_pan, Vec2::ZERO);
        app.fullscreen_page_layout.drawn_unit = original;
        app.items_generation += 1;
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.items_generation -= 1;
        app.fullscreen_idx = Some(1);
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.fullscreen_idx = Some(0);
        assert!(app.fs_scroll_layout_matches(ctx));
        paint(&mut app, ui, ctx);
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_pan.y < 0.0);
    });
}

#[test]
fn holdover_partial_pair_and_comparison_paints_cannot_publish_scroll_receipts() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, mode);
        frame(&ctx, vec![], |ui, ctx| {
            let (composition, mut sources) = paint(&mut app, ui, ctx);
            let viewport = app
                .fullscreen_page_layout
                .displayed_pages()
                .next()
                .unwrap()
                .transform
                .viewport_rect;
            sources.pages.last_mut().unwrap().provenance = FsDisplayUnitPageProvenance::Holdover;
            app.record_fs_scroll_drawn_unit(ctx, viewport, &composition, &sources);
            assert!(app.fullscreen_page_layout.drawn_unit.is_none());
            assert!(!app.fs_scroll_layout_matches(ctx));
            sources.pages.last_mut().unwrap().provenance = FsDisplayUnitPageProvenance::Live;
            if mode == SpreadMode::Ltr {
                let first = app
                    .fullscreen_page_layout
                    .displayed_pages()
                    .next()
                    .unwrap()
                    .transform;
                app.fullscreen_page_layout
                    .begin(FullscreenPageLayoutKind::Spread);
                app.fullscreen_page_layout.push(first);
                app.record_fs_scroll_drawn_unit(ctx, viewport, &composition, &sources);
                assert!(app.fullscreen_page_layout.drawn_unit.is_none());
            }
            paint(&mut app, ui, ctx);
            app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5);
            app.record_fs_scroll_drawn_unit(ctx, viewport, &composition, &sources);
            app.compare_view_mode = crate::app::CompareViewMode::Off;
            assert!(
                !app.fs_scroll_layout_matches(ctx),
                "comparison release requires a new ordinary paint"
            );
            paint(&mut app, ui, ctx);
            assert!(app.fs_scroll_layout_matches(ctx));
        });
    }
}

#[test]
fn geometry_key_rejects_same_frame_fit_rotation_free_rotation_gap_direction_and_limits() {
    for change in 0..6 {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        frame(&ctx, vec![], |_, ctx| {
            match change {
                0 => app.settings.fullscreen_fit_mode = FullscreenFitMode::Height,
                1 => {
                    app.rotation_cache
                        .insert(0, crate::rotation_db::Rotation::Cw90);
                }
                2 => app.fs_free_rotation = 0.2,
                3 => app.settings.spread_page_gap_px += 1,
                4 => app.reading_direction = ReadingDirection::Rtl,
                5 => app.settings.fullscreen_fit_no_upscale = true,
                _ => unreachable!(),
            }
            assert!(!app.fs_scroll_layout_matches(ctx), "change={change}");
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            assert_eq!(app.fs_pan, Vec2::ZERO);
        });
    }
}

#[test]
fn replacement_material_key_uses_dimensions_rather_than_texture_identity() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        replace_texture(&mut app, ctx, [80, 120]);
        assert!(
            app.fs_scroll_layout_matches(ctx),
            "same geometry permits a different texture"
        );
        replace_texture(&mut app, ctx, [81, 120]);
        assert!(
            !app.fs_scroll_layout_matches(ctx),
            "rounded replacement raster changes the real geometry inputs"
        );
        app.fs_cache.clear();
        app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
        assert!(
            !app.fs_scroll_layout_matches(ctx),
            "missing material dimensions never reuse the old key"
        );
    });
}

#[test]
fn resized_or_dpi_changed_viewport_rejects_old_receipt() {
    for dpi_change in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        let rect = if dpi_change {
            full_rect()
        } else {
            Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(620.0, 430.0))
        };
        run_in_viewport(
            &ctx,
            egui::ViewportId::ROOT,
            rect,
            if dpi_change { 1.5 } else { 1.0 },
            vec![],
            |ui, ctx| {
                assert!(!app.fs_scroll_layout_matches(ctx));
                app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
                assert_eq!(app.fs_pan, Vec2::ZERO);
                paint(&mut app, ui, ctx);
                app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
                assert!(app.fs_pan.y < 0.0);
            },
        );
    }
}

#[test]
fn root_dispatch_uses_destination_viewport_size_and_dpi() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.native_video_in_window_active = false;
    app.settings.video_in_window_mode = false;
    let child = app.fullscreen_viewport_id();
    assert_ne!(child, egui::ViewportId::ROOT);
    run_in_viewport(&ctx, child, full_rect(), 1.5, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
    });
    run_in_viewport(
        &ctx,
        egui::ViewportId::ROOT,
        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)),
        1.0,
        vec![],
        |_, ctx| {
            assert!(
                app.fs_scroll_layout_matches(ctx),
                "ROOT size/DPI must not replace the destination values"
            );
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            assert!(app.fs_pan.y < 0.0);
        },
    );
}

#[test]
fn changed_zoom_skips_scroll_until_new_paint_then_cancels_initial_request() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        app.settings.fullscreen_page_start_position =
            crate::settings::FullscreenPageStartPosition::ReadingStart;
        app.request_fs_initial_placement(0);
        let automatic = app.fs_automatic_initial_view;
        app.fs_zoom = 2.0;
        assert!(
            !app.fs_scroll_layout_matches(ctx),
            "a different zoom must not project the previous paint"
        );
        let request = app.fs_initial_placement_request;
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert_eq!(app.fs_pan, Vec2::ZERO);
        assert_eq!(app.fs_initial_placement_request, request);
        assert_eq!(app.fs_automatic_initial_view, automatic);
    });
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        assert!(
            app.fs_scroll_layout_matches(ctx),
            "a new paint admits the unchanged new zoom"
        );
        app.request_fs_initial_placement(0);
        let automatic = app.fs_automatic_initial_view;
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_pan.y < 0.0);
        assert!(app.fs_initial_placement_request.is_none());
        assert_eq!(app.fs_automatic_initial_view, automatic);
    });
}

#[test]
fn up_down_handler_obeys_all_four_independent_settings_combinations() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for up_down in [
        FullscreenUpDownKeyAction::PageTurn,
        FullscreenUpDownKeyAction::InPageScroll,
    ] {
        for wheel in [
            FullscreenWheelAction::PageTurn,
            FullscreenWheelAction::InPageScroll,
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_up_down_key_action = up_down;
            app.settings.fullscreen_wheel_action = wheel;
            seed(&ctx, &mut app);
            frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
                let action = app.handle_fs_key_input(ctx, 0, false);
                assert_eq!(
                    action.page_nav.is_none(),
                    up_down == FullscreenUpDownKeyAction::InPageScroll
                );
                assert_eq!(
                    app.fs_pan.y < 0.0,
                    up_down == FullscreenUpDownKeyAction::InPageScroll
                );
                assert_eq!(app.fullscreen_idx, Some(0));
            });
        }
    }
}

#[test]
fn up_down_short_tap_multiple_edges_and_released_repeat_move_at_most_once() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
        app.handle_fs_key_input(ctx, 0, false);
        let once = app.fs_pan;
        assert!(once.y < 0.0);
        app.handle_fs_key_input(ctx, 0, false);
        assert_eq!(app.fs_pan, once);
    });
    let once = app.fs_pan;
    // End this pass with the key still held. egui derives repeat from its
    // keys_down history, so a repeat after the preceding completed tap would
    // correctly become a new initial press regardless of RawInput.repeat.
    frame(
        &ctx,
        vec![
            key(egui::Key::ArrowDown, true, false, egui::Modifiers::NONE),
            key(egui::Key::ArrowDown, true, true, egui::Modifiers::NONE),
            key(egui::Key::ArrowDown, true, true, egui::Modifiers::NONE),
        ],
        |_, ctx| {
            app.handle_fs_key_input(ctx, 0, false);
            let step = app
                .fullscreen_page_layout
                .drawn_unit
                .as_ref()
                .unwrap()
                .geometry
                .viewport
                .height()
                * app.settings.continuous_reading_key_scroll_percent as f32
                / 100.0;
            assert!((app.fs_pan.y - (once.y - step)).abs() < 0.001);
        },
    );
    let before_released_repeat = app.fs_pan;
    frame(
        &ctx,
        vec![
            key(egui::Key::ArrowDown, true, true, egui::Modifiers::NONE),
            key(egui::Key::ArrowDown, false, false, egui::Modifiers::NONE),
        ],
        |_, ctx| {
            assert!(
                ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: true,
                        repeat: true,
                        ..
                    }
                ))),
                "the fixture must deliver an actual repeat, not a rewritten initial press"
            );
            app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(
                app.fs_pan, before_released_repeat,
                "released repeat must not move"
            );
        },
    );
}

#[test]
fn shift_arrows_and_horizontal_arrows_keep_page_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for (key_name, modifiers) in [
        (egui::Key::ArrowDown, egui::Modifiers::SHIFT),
        (egui::Key::ArrowRight, egui::Modifiers::NONE),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        frame(
            &ctx,
            vec![key(key_name, true, false, modifiers)],
            |_, ctx| {
                let action = app.handle_fs_key_input(ctx, 0, false);
                assert!(!action.page_nav.is_none());
                assert_eq!(app.fs_pan, Vec2::ZERO);
            },
        );
    }
}

#[test]
fn fallback_menu_leaves_navigation_arrows_unconsumed_without_moving_or_turning() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for in_page in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_up_down_key_action = if in_page {
            FullscreenUpDownKeyAction::InPageScroll
        } else {
            FullscreenUpDownKeyAction::PageTurn
        };
        seed(&ctx, &mut app);
        app.fs_context_menu_idx = Some(0);
        frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    ..
                }
            ))));
        });
    }
}

#[test]
fn default_j_k_actions_work_with_page_turn_settings_and_use_the_selected_order() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for order in [
        FullscreenNScrollOrder::VerticalFirst,
        FullscreenNScrollOrder::HorizontalFirst,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_up_down_key_action = FullscreenUpDownKeyAction::PageTurn;
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
        app.settings.fullscreen_n_scroll_order = order;
        app.fs_zoom = 3.0;
        seed(&ctx, &mut app);
        frame(&ctx, tap(egui::Key::K), |_, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert!(if order == FullscreenNScrollOrder::VerticalFirst {
                app.fs_pan.y < 0.0 && app.fs_pan.x == 0.0
            } else {
                app.fs_pan.x < 0.0 && app.fs_pan.y == 0.0
            });
        });
        let after_next = app.fs_pan;
        frame(&ctx, tap(egui::Key::J), |_, ctx| {
            app.handle_fs_key_input(ctx, 0, false);
            assert!(if order == FullscreenNScrollOrder::VerticalFirst {
                app.fs_pan.y > after_next.y
            } else {
                app.fs_pan.x > after_next.x
            });
            assert_eq!(app.fullscreen_idx, Some(0));
        });
    }
}

#[test]
fn new_action_is_left_for_front_ui_or_surface_outside_but_consumed_for_stale_layout() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for gate in 0..6 {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageNScrollNext = F24\n");
        seed(&ctx, &mut app);
        match gate {
            0 => app.fs_context_menu_idx = Some(0),
            1 => app.show_preferences = true,
            2 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
            3 => app.view_trim_mode = true,
            4 => app.reading_flow = ReadingFlow::Vertical,
            5 => app.fullscreen_page_layout.drawn_unit = None,
            _ => unreachable!(),
        }
        frame(&ctx, tap(egui::Key::F24), |_, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert_eq!(
                app.keymap.consume_action(ctx, KeyAction::FsPageNScrollNext),
                gate != 5,
                "gate={gate}"
            );
        });
    }
}

#[test]
fn wheel_classifier_preserves_modifier_seek_and_surface_legacy_routes() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::SHIFT,
        egui::Modifiers::CTRL,
        egui::Modifiers::ALT,
    ] {
        for seek in [false, true] {
            let input = wheel(Vec2::new(-0.1, -0.5), modifiers);
            assert_eq!(
                matches!(
                    app.fs_wheel_segment_in_page_scroll(input, seek),
                    FsWheelInPageClass::InPageScroll(_)
                ),
                !seek && !modifiers.ctrl && !modifiers.alt
            );
        }
    }
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    assert_eq!(
        app.fs_wheel_segment_in_page_scroll(
            wheel(Vec2::new(-20.0, 0.0), egui::Modifiers::SHIFT),
            false
        ),
        FsWheelInPageClass::Legacy
    );
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::InPageScroll;
    app.reading_flow = ReadingFlow::Vertical;
    assert_eq!(
        app.fs_wheel_segment_in_page_scroll(
            wheel(Vec2::new(0.0, -20.0), egui::Modifiers::NONE),
            false
        ),
        FsWheelInPageClass::Legacy
    );
}

#[test]
fn wheel_prediction_uses_fractional_two_axis_scroll_and_stops_after_legacy_navigation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_zoom = 3.0;
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        let delta = Vec2::new(-0.1, -0.5);
        let actions = vec![action(&app, wheel(delta, egui::Modifiers::NONE), 0)];
        let (zoom, pan) = (app.fs_zoom, app.fs_pan);
        let expected = app.fs_page_scroll_wheel_pan(ctx, zoom, pan, delta);
        let (zoom, pan) = app.propose_fs_wheel_click_transform(ctx, viewport, &actions);
        assert_eq!(zoom, app.fs_zoom);
        assert_eq!(pan, expected);
        assert!(pan.x < 0.0 && pan.y < 0.0);
        assert_eq!(
            app.fs_pan,
            Vec2::ZERO,
            "click planning must not publish its prediction"
        );
        let actions = vec![
            action(&app, wheel(Vec2::new(0.0, -20.0), egui::Modifiers::ALT), 0),
            action(&app, wheel(delta, egui::Modifiers::NONE), 1),
        ];
        let (_, pan) = app.propose_fs_wheel_click_transform(ctx, viewport, &actions);
        assert_eq!(
            pan, app.fs_pan,
            "later page-local input belongs to the page being left"
        );
    });
}

#[test]
fn zoom_then_page_scroll_prediction_discards_scroll_for_the_unpainted_zoom() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        let zoom_action = action(&app, wheel(Vec2::new(0.0, 20.0), egui::Modifiers::CTRL), 0);
        let (zoom, pan) = app.propose_fs_wheel_click_transform(ctx, viewport, &[zoom_action]);
        let delta = Vec2::new(0.0, -10.0);
        let expected = app.fs_page_scroll_wheel_pan(ctx, zoom, pan, delta);
        assert_eq!(
            expected, pan,
            "wheel prediction must check its proposed zoom too"
        );
        let actions = vec![
            action(&app, wheel(Vec2::new(0.0, 20.0), egui::Modifiers::CTRL), 0),
            action(&app, wheel(delta, egui::Modifiers::NONE), 1),
        ];
        assert_eq!(
            app.propose_fs_wheel_click_transform(ctx, viewport, &actions),
            (zoom, expected)
        );
        assert!(zoom > app.fs_zoom);
        assert_eq!(app.fs_zoom, 1.0, "prediction does not publish the zoom");
        assert_eq!(app.fs_pan, Vec2::ZERO);
    });
}

#[test]
fn deferred_wheel_projects_only_segments_remaining_after_front_ui_consumption() {
    let ctx = egui::Context::default();
    let app = app(&ctx, SpreadMode::Single);
    let events = vec![
        wheel_event(Vec2::new(0.0, -0.1), egui::Modifiers::NONE),
        wheel_event(Vec2::new(0.0, -20.0), egui::Modifiers::CTRL),
        wheel_event(Vec2::new(-0.5, 0.0), egui::Modifiers::NONE),
    ];
    frame(&ctx, events, |_, ctx| {
        let options = ctx.options(|options| options.input_options);
        let segments =
            ctx.input(|input| crate::wheel_input::remaining_wheel_segments(input, options));
        assert_eq!(segments.len(), 3);
        let eligible: Vec<_> = segments
            .iter()
            .map(|segment| {
                let mut action = action(&app, segment.input, segment.first_raw_ordinal.unwrap());
                action.segment = *segment;
                action
            })
            .collect();
        crate::wheel_input::consume_wheel_segment(ctx, segments[0]);
        let remaining = app.remaining_fs_wheel_frame_actions(ctx, &eligible);
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].class, FsWheelInPageClass::Legacy);
        assert_eq!(
            remaining[1].class,
            FsWheelInPageClass::InPageScroll(Vec2::new(-0.5, 0.0))
        );
        assert_eq!(remaining[1].scroll_origin, eligible[2].scroll_origin);
    });
}

#[test]
fn context_swap_keeps_receipt_and_scroll_pan_with_the_owning_context() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    let main_pan = app.fs_pan;
    let main_stamp = app.fullscreen_page_layout.drawn_unit.clone();
    let sibling = app.build_window_context_for_test(47401, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/scroll-sibling.png"))];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.visible_indices = vec![0];
        owner.fullscreen_idx = Some(0);
        owner.fs_zoom = 2.0;
        owner.fs_pan = Vec2::new(11.0, 19.0);
        owner.fullscreen_page_layout.clear();
    });
    app.with_window_context_for_test(sibling, |owner| {
        frame(&ctx, vec![], |_, ctx| {
            assert!(owner.fullscreen_page_layout.drawn_unit.is_none());
            owner.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            assert_eq!(owner.fs_pan, Vec2::new(11.0, 19.0));
        });
    });
    assert_eq!(app.fs_pan, main_pan);
    assert_eq!(app.fullscreen_page_layout.drawn_unit, main_stamp);
    frame(&ctx, vec![], |_, ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert!(app.fs_pan.y < main_pan.y);
    });
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.fs_pan, Vec2::new(11.0, 19.0));
        assert!(owner.fullscreen_page_layout.drawn_unit.is_none());
    });
}

#[test]
fn changed_slice_and_spread_screen_order_do_not_match_previous_unit() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |ui, ctx| {
        app.fullscreen_page_slice = crate::page_split::PageSlice::Right;
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
        assert!(app.fs_scroll_layout_matches(ctx));
        app.spread_mode = SpreadMode::Rtl;
        assert!(
            !app.fs_scroll_layout_matches(ctx),
            "same two indices in the opposite screen order are a different unit"
        );
        paint(&mut app, ui, ctx);
        assert!(app.fs_scroll_layout_matches(ctx));
    });
}

#[test]
fn pending_navigation_sequence_consumes_scroll_without_using_previous_pixels() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, vec![], |_, ctx| {
        assert!(app.begin_fs_page_navigation_sequence(ctx, 0, 1, false));
        assert!(app.fs_navigation_sequence_blocks_new_target());
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
        assert_eq!(app.fs_pan, Vec2::ZERO);
    });
}

#[test]
fn fit_change_and_down_in_one_real_handler_pass_discard_old_geometry() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(
        &ctx,
        vec![
            key(egui::Key::Num0, true, false, egui::Modifiers::NONE),
            key(egui::Key::ArrowDown, true, false, egui::Modifiers::NONE),
        ],
        |_, ctx| {
            let before = app.effective_fullscreen_fit_mode();
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert_ne!(app.effective_fullscreen_fit_mode(), before);
            assert!(action.page_nav.is_none());
            assert_eq!(
                app.fs_pan,
                Vec2::ZERO,
                "the down input runs after fit mutation and must reject the old receipt"
            );
        },
    );
}

#[cfg(windows)]
#[test]
fn native_and_egui_down_share_one_consume_and_use_the_edge_modifier() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(
        &ctx,
        vec![
            egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
            key(egui::Key::ArrowDown, true, false, egui::Modifiers::NONE),
        ],
        |_, ctx| {
            let edge = crate::key_input::KeyEdge {
                operation_id: crate::key_input::KeyOperationId::unassigned(),
                source_hwnd: 1,
                source_viewport: ctx.viewport_id(),
                virtual_key: 0x28,
                scan_code: 0,
                extended: false,
                pressed: true,
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            };
            crate::key_input::set_test_frame(vec![
                edge,
                crate::key_input::KeyEdge {
                    repeat: true,
                    ..edge
                },
            ]);
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(
                action.page_nav.is_none(),
                "current Shift level must not replace the matched plain chord"
            );
            let viewport = app
                .fullscreen_page_layout
                .drawn_unit
                .as_ref()
                .unwrap()
                .geometry
                .viewport;
            let step = viewport.height()
                * app.settings.continuous_reading_key_scroll_percent as f32
                / 100.0;
            assert!((app.fs_pan.y + step).abs() < 0.001);
            let once = app.fs_pan;
            app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(app.fs_pan, once, "native and egui copies cannot move twice");
            crate::key_input::clear_test_frame();
        },
    );
}

#[test]
fn stack_shift_arrow_stays_in_stack_route_even_with_page_scroll_enabled() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    app.stack_showing_flat = true;
    frame(
        &ctx,
        vec![key(
            egui::Key::ArrowDown,
            true,
            false,
            egui::Modifiers::SHIFT,
        )],
        |_, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            // This minimal fixture has no next stack: consume at the stack boundary.
            assert!(action.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert!(!app.keymap.consume_action(ctx, KeyAction::FsStackJumpNext));
        },
    );
}

#[test]
fn actual_wheel_acceptance_keeps_fractional_scroll_and_legacy_thresholds_separate() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for in_page in [false, true] {
        for delta in [
            Vec2::new(0.0, -0.1),
            Vec2::new(0.0, -0.5),
            Vec2::new(-0.1, 0.0),
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_wheel_action = if in_page {
                FullscreenWheelAction::InPageScroll
            } else {
                FullscreenWheelAction::PageTurn
            };
            app.fs_zoom = 3.0;
            seed(&ctx, &mut app);
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(full_rect().center()),
                    wheel_event(delta, egui::Modifiers::NONE),
                ],
                |ui, ctx| {
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let result = app.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        full_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                    assert!(result.0.is_none());
                    assert_eq!(app.fs_pan != Vec2::ZERO, in_page, "delta={delta:?}");
                    let options = ctx.options(|options| options.input_options);
                    assert_eq!(
                        ctx.input(|input| crate::wheel_input::remaining_wheel_segments(
                            input, options
                        )
                        .is_empty()),
                        in_page
                    );
                },
            );
        }
    }
}

#[test]
fn real_preferences_controls_ok_persistence_then_handler_and_wheel_use_new_values() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_up_down_key_action = FullscreenUpDownKeyAction::PageTurn;
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    app.settings.fullscreen_n_scroll_order = FullscreenNScrollOrder::VerticalFirst;
    let data_dir = app.tmp.path().to_path_buf();
    let persisted = crate::ui_dialogs::preferences::commit_in_page_scroll_preferences_ui_for_test(
        &mut app, &data_dir,
    );
    assert_eq!(
        persisted.fullscreen_up_down_key_action,
        FullscreenUpDownKeyAction::InPageScroll
    );
    assert_eq!(
        persisted.fullscreen_wheel_action,
        FullscreenWheelAction::InPageScroll
    );
    assert_eq!(
        persisted.fullscreen_n_scroll_order,
        FullscreenNScrollOrder::HorizontalFirst
    );
    app.fs_zoom = 3.0;
    seed(&ctx, &mut app);
    frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
        let action = app.handle_fs_key_input(ctx, 0, false);
        assert!(action.page_nav.is_none());
        assert!(app.fs_pan.y < 0.0);
    });
    let after_down = app.fs_pan;
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        app.handle_fs_key_input(ctx, 0, false);
        assert!(app.fs_pan.x < after_down.x);
        assert_eq!(
            app.fs_pan.y, after_down.y,
            "the saved horizontal order drives K"
        );
        let wheel = wheel(Vec2::new(-0.1, 0.0), egui::Modifiers::NONE);
        assert!(matches!(
            app.fs_wheel_segment_in_page_scroll(wheel, false),
            FsWheelInPageClass::InPageScroll(_)
        ));
        let (zoom, current_pan) = (app.fs_zoom, app.fs_pan);
        let pan = app.fs_page_scroll_wheel_pan(ctx, zoom, current_pan, wheel.delta);
        assert!(pan.x < app.fs_pan.x);
        assert_eq!(pan.y, app.fs_pan.y);
    });
}

#[test]
fn focused_text_input_keeps_new_action_and_up_down_away_from_the_canvas() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let mut app = app(&ctx, SpreadMode::Single);
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsPageNScrollNext = F24\n");
    seed(&ctx, &mut app);
    let id = egui::Id::new("page-scroll-front-text");
    let mut text = String::new();
    frame(&ctx, vec![], |ui, _| {
        crate::ime_focus::add_singleline(ui, &mut text, None, |edit| edit.id(id)).request_focus();
    });
    frame(
        &ctx,
        vec![
            key(egui::Key::ArrowDown, true, false, egui::Modifiers::NONE),
            key(egui::Key::F24, true, false, egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let response =
                crate::ime_focus::add_singleline(ui, &mut text, None, |edit| edit.id(id));
            assert!(response.has_focus());
            assert!(
                matches!(app.keyboard_owner_for_pass(ctx),
                    crate::keyboard_input::KeyboardOwner::TextInput { widget_id, .. } if widget_id == id
                ),
                "the real helper-managed text field must own the keyboard"
            );
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            assert!(
                !app.keymap.consume_action(ctx, KeyAction::FsPageNScrollNext),
                "the shared keymap gate refuses canvas actions while TextInput owns the pass"
            );
            assert!(
                ctx.input(|input| input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::F24,
                        pressed: true,
                        ..
                    }
                ))),
                "the canvas handler must leave the new action's physical event unclaimed"
            );
        },
    );
}

#[test]
fn wheel_and_single_click_same_frame_uses_the_scrolled_view_before_deciding_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    let pos = egui::pos2(450.0, 200.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &ctx,
        vec![egui::Event::PointerMoved(pos), button(true)],
        |ui, ctx| {
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
        },
    );
    frame(
        &ctx,
        vec![
            button(false),
            wheel_event(Vec2::new(0.0, -20.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            let result =
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
            assert!(app.fs_pan.y < 0.0);
            assert!(
                result.0.is_none(),
                "single click cannot turn after the same-frame wheel moved the page"
            );
            assert_eq!(app.fullscreen_idx, Some(0));
        },
    );
}

#[test]
fn key_zoom_and_down_same_frame_consume_scroll_then_new_paint_can_reach_the_edge() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsZoomIn = F24\n");
    app.settings.continuous_reading_key_scroll_percent = 100;
    seed(&ctx, &mut app);
    let events = tap(egui::Key::ArrowDown)
        .into_iter()
        .chain(tap(egui::Key::F24))
        .collect();
    frame(&ctx, events, |_, ctx| {
        let action = app.handle_fs_key_input(ctx, 0, false);
        assert!(action.page_nav.is_none());
        assert!(app.fs_zoom > 1.0);
        assert_eq!(
            app.fs_pan,
            Vec2::ZERO,
            "the down input cannot use pre-zoom geometry"
        );
        assert!(!app.fs_scroll_layout_matches(ctx));
    });
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        app.handle_fs_key_input(ctx, 0, false);
        assert_eq!(
            app.fs_pan,
            Vec2::ZERO,
            "discarded input is not replayed after repaint"
        );
    });
    frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
        let action = app.handle_fs_key_input(ctx, 0, false);
        assert!(action.page_nav.is_none());
        assert!(app.fs_pan.y < 0.0);
    });
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        assert!((painted_unit(&app).bottom() - viewport.bottom()).abs() < 1.01);
    });
}

#[test]
fn ctrl_wheel_then_scroll_consumes_without_projection_and_resumes_after_new_paint() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(Vec2::new(0.0, 20.0), egui::Modifiers::CTRL),
            wheel_event(Vec2::new(0.0, -1200.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let viewport = app
                .fullscreen_page_layout
                .drawn_unit
                .as_ref()
                .unwrap()
                .geometry
                .viewport;
            let zoom_only = action(&app, wheel(Vec2::new(0.0, 20.0), egui::Modifiers::CTRL), 1);
            let expected = app.propose_fs_wheel_click_transform(ctx, viewport, &[zoom_only]);
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            let result =
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
            assert!(result.0.is_none());
            assert!(app.fs_zoom > 1.0);
            assert_eq!(
                (app.fs_zoom, app.fs_pan),
                expected,
                "the second segment is consumed without moving the zoomed view"
            );
            let options = ctx.options(|options| options.input_options);
            assert!(ctx.input(|input| {
                crate::wheel_input::remaining_wheel_segments(input, options).is_empty()
            }));
            assert!(!app.fs_scroll_layout_matches(ctx));
        },
    );
    let after_zoom = app.fs_pan;
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        assert_eq!(
            app.fs_pan, after_zoom,
            "the old segment does not carry to the new paint"
        );
    });
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(full_rect().center()),
            wheel_event(Vec2::new(0.0, -1200.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            let result =
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
            assert!(result.0.is_none());
            assert!(app.fs_pan.y < after_zoom.y);
        },
    );
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        assert!((painted_unit(&app).bottom() - viewport.bottom()).abs() < 1.01);
    });
}

#[test]
fn gap_200_spread_rejects_old_zoom_geometry_and_aligns_to_the_new_painted_edge() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    app.settings.spread_page_gap_px = 200;
    app.settings.continuous_reading_key_scroll_percent = 100;
    seed(&ctx, &mut app);
    let old_unit = painted_unit(&app);
    assert!((old_unit.width() - full_rect().width()).abs() < 1.01);
    frame(&ctx, vec![], |_, ctx| {
        app.fs_zoom = 2.0;
        assert!(!app.fs_scroll_layout_matches(ctx));
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollRight);
        assert_eq!(
            app.fs_pan,
            Vec2::ZERO,
            "the old ratio projection would overshoot by half the extra gap"
        );
        let before = app.fs_pan;
        let pan = app.fs_page_scroll_wheel_pan(ctx, 2.0, before, Vec2::new(-1200.0, 0.0));
        assert_eq!(pan, Vec2::ZERO);
    });
    let mut expected_pan = 0.0;
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        let unit = painted_unit(&app);
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        let old_ratio_edge = viewport.right()
            - (viewport.center().x + (old_unit.right() - viewport.center().x) * 2.0);
        expected_pan = viewport.right() - unit.right();
        assert!(
            (expected_pan - old_ratio_edge - 100.0).abs() < 1.01,
            "the fixture must expose the 100-point fixed-gap/projection discrepancy"
        );
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollRight);
        assert!((app.fs_pan.x - expected_pan).abs() < 0.001);
        assert_eq!(app.fs_pan.y, 0.0);
    });
    frame(&ctx, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        let viewport = app
            .fullscreen_page_layout
            .drawn_unit
            .as_ref()
            .unwrap()
            .geometry
            .viewport;
        assert!((painted_unit(&app).right() - viewport.right()).abs() < 1.01);
        let pan = app.fs_pan;
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollRight);
        assert_eq!(
            app.fs_pan, pan,
            "new input stops at the actually painted edge"
        );
        assert!((app.fs_pan.x - expected_pan).abs() < 0.001);
    });
}

#[test]
fn near_fit_zoom_normalization_does_not_permanently_discard_keys_or_wheel() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for zoom in [0.9995, 1.0005] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.fs_zoom = zoom;
        seed(&ctx, &mut app);
        let page = app.fullscreen_page_layout.single_page().unwrap().transform;
        assert_eq!(
            normal_zoom_pan(page).unwrap().0,
            1.0,
            "the fixture must cover fit-near placement normalization"
        );
        frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
            assert!(app.fs_scroll_layout_matches(ctx), "raw zoom={zoom}");
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert!(app.fs_pan.y < 0.0, "raw zoom={zoom}");
            assert_eq!(
                app.fs_zoom, 1.0,
                "scroll commits the effective painted zoom"
            );
        });
        let after_key = app.fs_pan;
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(full_rect().center()),
                wheel_event(Vec2::new(0.0, -0.5), egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                let result = app.handle_fs_wheel_and_click(
                    ui,
                    ctx,
                    full_rect(),
                    &state,
                    false,
                    0,
                    &mut None,
                );
                assert!(result.0.is_none());
                assert!(app.fs_pan.y < after_key.y, "raw zoom={zoom}");
                assert_eq!(app.fs_zoom, 1.0);
            },
        );
        let after_wheel = app.fs_pan;
        frame(&ctx, vec![], |ui, ctx| {
            paint(&mut app, ui, ctx);
            assert_eq!(app.fs_pan, after_wheel);
            assert!(app.fs_scroll_layout_matches(ctx));
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
            assert!(
                app.fs_pan.y < after_wheel.y,
                "a subsequent actual paint keeps raw zoom={zoom} usable"
            );
        });
    }
}

#[test]
fn near_fit_wide_unit_key_scroll_keeps_exact_edge_and_painted_width_after_multiple_steps() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for raw_zoom in [0.9995, 1.0005] {
        let ctx = egui::Context::default();
        let mut app = wide_unit_app(&ctx, raw_zoom);
        for step in 1..=5 {
            let expected_x = (-1000.0_f32 * step as f32).max(-4500.0);
            wide_unit_frame(&ctx, tap(egui::Key::F24), |ui, ctx| {
                assert!(app.fs_scroll_layout_matches(ctx));
                let action = app.handle_fs_key_input(ctx, 0, false);
                assert!(action.page_nav.is_none());
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.fs_zoom, 1.0, "raw zoom={raw_zoom}, step={step}");
                assert_eq!(app.fs_pan, Vec2::new(expected_x, 0.0));
                paint(&mut app, ui, ctx);
                let unit = painted_unit(&app);
                assert_eq!(unit.width(), 10_000.0);
                assert_eq!(unit.right(), 5500.0 + expected_x);
            });
        }
        wide_unit_frame(&ctx, vec![], |ui, ctx| {
            paint(&mut app, ui, ctx);
            let unit = painted_unit(&app);
            assert_eq!(app.fs_zoom, 1.0);
            assert_eq!(app.fs_pan, Vec2::new(-4500.0, 0.0));
            assert_eq!(
                unit.right(),
                wide_unit_rect().right(),
                "no horizontal gap after the next paint"
            );
            assert!(unit.left() <= wide_unit_rect().left());
        });
        wide_unit_frame(&ctx, tap(egui::Key::F24), |_, ctx| {
            app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(app.fs_zoom, 1.0);
            assert_eq!(app.fs_pan, Vec2::new(-4500.0, 0.0));
        });
    }
}

#[test]
fn near_fit_wide_unit_wheel_prediction_and_apply_keep_exact_edge_after_multiple_steps() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for raw_zoom in [0.9995, 1.0005] {
        let ctx = egui::Context::default();
        let mut app = wide_unit_app(&ctx, raw_zoom);
        let delta = Vec2::new(-600.0, 0.0);
        for step in 1..=5 {
            let expected_pan = Vec2::new((-1000.0_f32 * step as f32).max(-4500.0), 0.0);
            wide_unit_frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(wide_unit_rect().center()),
                    wheel_event(delta, egui::Modifiers::NONE),
                ],
                |ui, ctx| {
                    let viewport = app
                        .fullscreen_page_layout
                        .drawn_unit
                        .as_ref()
                        .unwrap()
                        .geometry
                        .viewport;
                    let eligible = action(&app, wheel(delta, egui::Modifiers::NONE), 1);
                    let before = (app.fs_zoom, app.fs_pan);
                    let prediction =
                        app.propose_fs_wheel_click_transform(ctx, viewport, &[eligible]);
                    assert_eq!(
                        prediction,
                        (1.0, expected_pan),
                        "raw zoom={raw_zoom}, step={step}"
                    );
                    assert_eq!(
                        (app.fs_zoom, app.fs_pan),
                        before,
                        "prediction has no state side effects"
                    );
                    let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                    let result = app.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        wide_unit_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                    assert!(result.0.is_none());
                    assert_eq!((app.fs_zoom, app.fs_pan), prediction);
                    assert_eq!(app.fullscreen_idx, Some(0));
                    paint(&mut app, ui, ctx);
                    let unit = painted_unit(&app);
                    assert_eq!(unit.width(), 10_000.0);
                    assert_eq!(unit.right(), 5500.0 + expected_pan.x);
                },
            );
        }
        wide_unit_frame(&ctx, vec![], |ui, ctx| {
            paint(&mut app, ui, ctx);
            let unit = painted_unit(&app);
            assert_eq!(app.fs_zoom, 1.0);
            assert_eq!(app.fs_pan, Vec2::new(-4500.0, 0.0));
            assert_eq!(
                unit.right(),
                wide_unit_rect().right(),
                "no horizontal gap after the next paint"
            );
            assert!(unit.left() <= wide_unit_rect().left());
        });
        wide_unit_frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(wide_unit_rect().center()),
                wheel_event(delta, egui::Modifiers::NONE),
            ],
            |ui, ctx| {
                let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                let result = app.handle_fs_wheel_and_click(
                    ui,
                    ctx,
                    wide_unit_rect(),
                    &state,
                    false,
                    0,
                    &mut None,
                );
                assert!(result.0.is_none());
                assert_eq!(app.fs_zoom, 1.0);
                assert_eq!(app.fs_pan, Vec2::new(-4500.0, 0.0));
            },
        );
    }
}

#[test]
fn deferred_scroll_keeps_its_acceptance_receipt_across_repaint_and_click_prediction() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for matching_at_acceptance in [false, true] {
        for click in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.fullscreen_idx = Some(3);
            app.settings.fullscreen_manual_nav_wrap = false;
            app.settings.fullscreen_fit_mode = if matching_at_acceptance {
                FullscreenFitMode::Width
            } else {
                FullscreenFitMode::Page
            };
            seed(&ctx, &mut app);
            let pos = egui::pos2(450.0, 200.0);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            if click {
                frame(&ctx, vec![egui::Event::PointerMoved(pos)], |ui, ctx| {
                    let state = app.prepare_fullscreen_state(ctx, 3, FsPageTurnDecision::normal());
                    app.handle_fs_wheel_and_click(
                        ui,
                        ctx,
                        full_rect(),
                        &state,
                        false,
                        0,
                        &mut None,
                    );
                });
                frame(
                    &ctx,
                    vec![egui::Event::PointerMoved(pos), button(true)],
                    |ui, ctx| {
                        let state =
                            app.prepare_fullscreen_state(ctx, 3, FsPageTurnDecision::normal());
                        app.handle_fs_wheel_and_click(
                            ui,
                            ctx,
                            full_rect(),
                            &state,
                            false,
                            0,
                            &mut None,
                        );
                    },
                );
            }
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if click {
                events.push(button(false));
            }
            events.extend([
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::ALT),
            ]);
            frame(&ctx, events, |ui, ctx| {
                // The key/setting owner changed fit after the previous paint.
                // New paint happens only after wheel acceptance and click planning.
                app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
                assert_eq!(app.fs_scroll_layout_matches(ctx), matching_at_acceptance);
                let mut proposal = deferred_wheel_proposal(&mut app, ui, ctx);
                let FsWheelFrameInput::Unconsumed(eligible) = &proposal.wheel_input else {
                    panic!("real defer leaves eligibility for the front UI owner")
                };
                assert_eq!(eligible.len(), 2);
                assert!(matches!(
                    eligible[0].class,
                    FsWheelInPageClass::InPageScroll(_)
                ));
                assert_eq!(eligible[0].scroll_origin.is_some(), matching_at_acceptance);
                assert_eq!(eligible[1].class, FsWheelInPageClass::Legacy);
                let accepted_origin = eligible[0].scroll_origin.clone();
                let prediction = app.propose_fs_wheel_click_transform(
                    ctx,
                    proposal.default_image_rect,
                    eligible,
                );
                assert_eq!(prediction.0, 1.0);
                assert_eq!(prediction.1.x, 0.0);
                if matching_at_acceptance {
                    assert!(prediction.1.y < 0.0);
                } else {
                    assert_eq!(prediction.1, Vec2::ZERO);
                }
                assert_eq!(
                    matches!(proposal.canvas_click, FsCanvasClickAction::PageSide(_)),
                    click && !matching_at_acceptance,
                    "single click uses the same accepted scroll prediction as application"
                );
                let expected_navigation = match proposal.canvas_click {
                    FsCanvasClickAction::PageSide(base) => FsPageNav::Delta(base),
                    _ => FsPageNav::Delta(1),
                };
                assert_eq!(
                    app.fs_pan,
                    Vec2::ZERO,
                    "defer has not published its prediction"
                );

                // This is the ordinary deferred route: front UI claims the
                // intervals, the renderer publishes its new layout, then merge runs.
                app.claim_fs_wheel_frame_after_ui(&mut proposal);
                paint(&mut app, ui, ctx);
                assert!(
                    app.fs_scroll_layout_matches(ctx),
                    "new Width geometry is now available"
                );
                let FsWheelFrameInput::Owned { actions, .. } = &proposal.wheel_input else {
                    panic!("front UI claim owns the surviving intervals")
                };
                assert_eq!(
                    actions[0].scroll_origin, accepted_origin,
                    "claim/repaint must never manufacture a missing acceptance receipt"
                );
                if matching_at_acceptance {
                    assert_eq!(
                        actions[0].scroll_origin,
                        app.fullscreen_page_layout.drawn_unit
                    );
                } else {
                    assert!(actions[0].scroll_origin.is_none());
                }
                let after_paint_prediction =
                    app.propose_fs_wheel_click_transform(ctx, proposal.default_image_rect, actions);
                assert_eq!(
                    after_paint_prediction, prediction,
                    "a new valid layout cannot revive scroll rejected at acceptance"
                );
                let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
                assert_eq!(
                    (app.fs_zoom, app.fs_pan),
                    prediction,
                    "tail apply must match the click-time prediction"
                );
                assert_eq!(app.fullscreen_idx, Some(3));
                assert_eq!(
                    result.page_nav, expected_navigation,
                    "ordinary Alt/click navigation intent is preserved at the final image"
                );
                assert!(!result.close);
            });
        }
    }
}

#[cfg(windows)]
#[test]
fn saved_k_scrub_hold_and_default_k_press_both_reach_real_handlers_and_none_preserves_hold() {
    use crate::key_input::{SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey};

    let _serial = crate::key_input::lock_test_input();
    for disable_page_scroll in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.fs_prev_focused = true;
        app.settings.touch_still_chrome_learned = true;
        let ini = if disable_page_scroll {
            "[FsImage]\nFsScrubZoomHold = K\nFsPageNScrollNext = none\nFsPageNScrollPrev = none\n"
        } else {
            "[FsImage]\nFsScrubZoomHold = K\n"
        };
        app.keymap = crate::keymap::Keymap::from_ini_str(ini);
        seed(&ctx, &mut app);
        crate::key_input::arm_test_synthetic_input(0xE474, egui::ViewportId::ROOT);
        let now = std::time::Instant::now();
        crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            now,
            SyntheticNavigationKey::K,
            SyntheticModifiers::default(),
        ));
        crate::key_input::advance_test_synthetic_input(now);
        crate::key_input::begin_frame();

        let start = full_rect().center();
        let primary = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            vec![
                key(egui::Key::K, true, false, egui::Modifiers::NONE),
                egui::Event::PointerMoved(start),
                primary(start, true),
            ],
            |ui, ctx| {
                let action = app.handle_fs_key_input(ctx, 0, false);
                assert!(action.page_nav.is_none());
                assert_eq!(
                    app.fs_pan.y < 0.0,
                    !disable_page_scroll,
                    "the default Press scrolls on the same physical K that owns the hold"
                );
                assert_eq!(app.fs_zoom, 1.0, "scroll keeps the drawn effective zoom");
                let permit = crate::keyboard_input::focused_key_state_permit(ctx).unwrap();
                assert!(
                    app.keymap
                        .key_held_action(ctx, permit, KeyAction::FsScrubZoomHold),
                    "consuming the Press edge must not clear the physical hold level"
                );
                let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                let result = app.handle_fs_wheel_and_click(
                    ui,
                    ctx,
                    full_rect(),
                    &state,
                    false,
                    0,
                    &mut None,
                );
                assert!(result.0.is_none());
                assert!(
                    app.fs_middle_zoom_drag
                        .as_ref()
                        .is_some_and(|drag| drag.input == ZoomDragInput::Scrub
                            && drag.surface == ZoomDragSurface::Viewer),
                    "the real pointer handler also starts the saved K scrub hold"
                );
                assert_eq!(app.fullscreen_idx, Some(0));
            },
        );

        let moved = start + Vec2::new(100.0, 0.0);
        crate::key_input::begin_frame();
        frame(&ctx, vec![egui::Event::PointerMoved(moved)], |ui, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
            let result =
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
            assert!(result.0.is_none());
            assert_eq!(
                app.fs_zoom, 2.0,
                "the saved hold still zooms after the Press edge was consumed, including with new actions set to none"
            );
            assert!(app.fs_middle_zoom_drag.is_some());
            assert_eq!(app.fullscreen_idx, Some(0));
        });

        let released = now + std::time::Duration::from_millis(1);
        crate::key_input::enqueue_test_synthetic_command(SyntheticKeyCommand::up(
            released,
            SyntheticNavigationKey::K,
        ));
        crate::key_input::advance_test_synthetic_input(released);
        crate::key_input::begin_frame();
        frame(
            &ctx,
            vec![
                key(egui::Key::K, false, false, egui::Modifiers::NONE),
                primary(moved, false),
            ],
            |ui, ctx| {
                app.handle_fs_key_input(ctx, 0, false);
                let state = app.prepare_fullscreen_state(ctx, 0, FsPageTurnDecision::normal());
                app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
                assert!(app.fs_middle_zoom_drag.is_none());
                assert_eq!(app.fs_zoom, 2.0);
                assert_eq!(app.fullscreen_idx, Some(0));
            },
        );
        crate::key_input::clear_test_synthetic_input();
    }
}

#[test]
fn saved_k_press_runs_in_existing_handler_order_before_new_page_scroll_press() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    let _cleanup = KeyboardCleanup;
    for disable_page_scroll in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.keymap = crate::keymap::Keymap::from_ini_str(if disable_page_scroll {
            "[FsImage]\nFsPixelGrid = K\nFsPageNScrollNext = none\nFsPageNScrollPrev = none\n"
        } else {
            "[FsImage]\nFsPixelGrid = K\n"
        });
        seed(&ctx, &mut app);
        let pixel_grid = app.fs_pixel_grid_enabled;
        frame(&ctx, tap(egui::Key::K), |_, ctx| {
            let action = app.handle_fs_key_input(ctx, 0, false);
            assert!(action.page_nav.is_none());
            assert_eq!(
                app.fs_pixel_grid_enabled, !pixel_grid,
                "the existing FsPixelGrid Press handler consumes K first"
            );
            assert_eq!(
                app.fs_pan,
                Vec2::ZERO,
                "the later page-scroll Press cannot reuse the consumed K edge"
            );
            assert!(!app.keymap.consume_action(ctx, KeyAction::FsPageNScrollNext));
            assert_eq!(app.fullscreen_idx, Some(0));
        });
    }
}
