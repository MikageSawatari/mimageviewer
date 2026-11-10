//! Keep the pre-input preparation and actual viewport paint in the same pass.
use super::*;

struct PaintedFrame {
    pages: Vec<DisplayedImageTransform>,
    unit: egui::Rect,
    pan: egui::Vec2,
    request: Option<FsInitialPlacementRequest>,
}

fn app(ctx: &egui::Context, mode: SpreadMode, landscape: bool) -> crate::app::AppTestEnvForTest {
    let mut app = composition_switch_app(ctx, mode, landscape, ReadingDirection::Ltr);
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.native_video_in_window_active = true;
    app.settings.video_in_window_mode = true;
    app.settings.detached_viewer_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.settings.margin_fit_enabled = false;
    app.window_visible = true;
    crate::ui_fullscreen::install_fs_navigator_input_tracking(ctx);
    assert!(app.fullscreen_embedded_still_active());
    app
}

fn key_event(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn paint_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> PaintedFrame {
    app.frame_counter += 1;
    crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), Vec::new());
    let output = ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(arrival_viewport()),
            focused: true,
            time: Some(app.frame_counter as f64 / 60.0),
            events,
            viewports: [(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
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
    let pages: Vec<_> = app
        .fullscreen_page_layout
        .displayed_pages()
        .map(|page| page.transform)
        .collect();
    assert!(
        !pages.is_empty(),
        "the production viewport must paint resident pages"
    );
    let unit = pages
        .iter()
        .map(|page| page.paint_rect)
        .reduce(egui::Rect::union)
        .unwrap();
    PaintedFrame {
        pages,
        unit,
        pan: app.fs_pan,
        request: app.fs_initial_placement_request,
    }
}

fn assert_unit(frame: &PaintedFrame, count: usize, width: f32, height: f32) {
    assert_eq!(frame.pages.len(), count);
    close_to(frame.unit.width(), width);
    close_to(frame.unit.height(), height);
}

fn assert_reading_start(frame: &PaintedFrame, direction: ReadingDirection) {
    let viewport = frame.pages[0].viewport_rect;
    assert!(frame.unit.width() > viewport.width());
    assert!(frame.unit.height() > viewport.height());
    close_to(frame.unit.top(), viewport.top());
    match direction {
        ReadingDirection::Ltr => close_to(frame.unit.left(), viewport.left()),
        ReadingDirection::Rtl => close_to(frame.unit.right(), viewport.right()),
    }
    let expected = egui::vec2(
        (frame.unit.width() - viewport.width())
            * if direction == ReadingDirection::Ltr {
                0.5
            } else {
                -0.5
            },
        (frame.unit.height() - viewport.height()) * 0.5,
    );
    close_to(frame.pan.x, expected.x);
    close_to(frame.pan.y, expected.y);
    assert!(
        frame.request.is_none(),
        "the selected complete unit consumes placement"
    );
}

fn assert_switch_key_frame(
    frame: &PaintedFrame,
    new: (usize, f32, f32),
    direction: ReadingDirection,
) {
    // All target materials are complete and resident: the input pass must
    // prepare and align the new unit itself, without a stale transition frame.
    assert_unit(frame, new.0, new.1, new.2);
    assert_reading_start(frame, direction);
}

#[test]
fn initial_placement_viewport_key9_single_landscape_to_split_rtl_uses_new_half_geometry() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single, true);
    app.request_fs_initial_placement(0);
    let before = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&before, 1, 1600.0, 800.0);
    assert_reading_start(&before, ReadingDirection::Ltr);

    let switched = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num9, true)]);
    assert_eq!(app.spread_mode, SpreadMode::SplitRtl);
    assert_eq!(app.fs_zoom, 1.0);
    assert_switch_key_frame(&switched, (1, 800.0, 800.0), ReadingDirection::Rtl);
    let next = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num9, false)]);
    assert_unit(&next, 1, 800.0, 800.0);
    assert_eq!(
        next.pages[0].uv_rect,
        crate::page_split::PageSlice::Right.source_bbox(crate::rotation_db::Rotation::None)
    );
    assert_reading_start(&next, ReadingDirection::Rtl);
}

#[test]
fn initial_placement_viewport_key4_single_portrait_to_rtl_spread_uses_new_pair_geometry() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single, false);
    app.request_fs_initial_placement(0);
    let before = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&before, 1, 720.0, 1020.0);
    assert_reading_start(&before, ReadingDirection::Ltr);

    let switched = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num4, true)]);
    assert_eq!(app.spread_mode, SpreadMode::Rtl);
    assert_eq!(app.fs_zoom, 1.0);
    assert_switch_key_frame(&switched, (2, 1452.0, 1020.0), ReadingDirection::Rtl);
    let next = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num4, false)]);
    assert_unit(&next, 2, 1452.0, 1020.0);
    assert_eq!(
        next.pages
            .iter()
            .map(|page| page.page_idx)
            .collect::<Vec<_>>(),
        vec![1, 0]
    );
    assert_reading_start(&next, ReadingDirection::Rtl);
}

#[test]
fn initial_placement_viewport_key1_panned_ltr_spread_to_single_uses_new_page_geometry() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr, false);
    app.request_fs_initial_placement(0);
    let before = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&before, 2, 1452.0, 1020.0);
    assert_reading_start(&before, ReadingDirection::Ltr);
    app.set_fs_pan_from_input(1.0, egui::vec2(53.0, -71.0));
    let panned = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&panned, 2, 1452.0, 1020.0);
    assert_eq!(panned.pan, egui::vec2(53.0, -71.0));

    let switched = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num1, true)]);
    assert_eq!(app.spread_mode, SpreadMode::Single);
    assert_eq!(app.fs_zoom, 1.0);
    assert_switch_key_frame(&switched, (1, 720.0, 1020.0), ReadingDirection::Ltr);
    let next = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::Num1, false)]);
    assert_unit(&next, 1, 720.0, 1020.0);
    assert_eq!(next.pages[0].page_idx, 0);
    assert_reading_start(&next, ReadingDirection::Ltr);
}

#[test]
fn initial_placement_viewport_accepted_open_resets_before_complete_unit_consumes_request() {
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single, false);
    app.request_fs_initial_placement(0);
    assert_reading_start(&paint_frame(&mut app, &ctx, vec![]), ReadingDirection::Ltr);
    app.fs_zoom = 2.0;
    app.set_fs_pan_from_input(2.0, egui::vec2(53.0, -71.0));
    let panned = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&panned, 1, 1440.0, 2040.0);

    let accepted = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::ArrowRight, true)]);
    // Navigation commits after painting. Its reset and placement request
    // belong to the new anchor, never to this already-painted old unit.
    assert_unit(&accepted, 1, 1440.0, 2040.0);
    assert_eq!(accepted.pages[0].page_idx, 0);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!((app.fs_zoom, app.fs_pan), (1.0, egui::Vec2::ZERO));
    let request = app
        .fs_initial_placement_request
        .expect("accepted open requests placement");
    assert_eq!(
        (request.origin_zoom, request.origin_pan),
        (1.0, egui::Vec2::ZERO)
    );
    let opened = paint_frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::ArrowRight, false)],
    );
    assert_unit(&opened, 1, 720.0, 1020.0);
    assert_eq!(opened.pages[0].page_idx, 1);
    assert_reading_start(&opened, ReadingDirection::Ltr);
    let next = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&next, 1, 720.0, 1020.0);
    assert_eq!(next.unit, opened.unit);
    assert_eq!(next.pan, opened.pan);
    assert!(next.request.is_none());
}

#[test]
fn initial_placement_viewport_accepted_same_image_split_key_keeps_old_paint_from_consuming_request()
{
    let _serial = crate::key_input::lock_test_input();
    let _cleanup = TestKeyboardFrameCleanup;
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::SplitLtr, true);
    app.request_fs_initial_placement(0);
    let before = paint_frame(&mut app, &ctx, vec![]);
    assert_unit(&before, 1, 800.0, 800.0);
    assert_eq!(
        before.pages[0].uv_rect,
        crate::page_split::PageSlice::Left.source_bbox(crate::rotation_db::Rotation::None)
    );
    assert_reading_start(&before, ReadingDirection::Ltr);
    let manual_pan = egui::vec2(53.0, -71.0);
    app.set_fs_pan_from_input(1.0, manual_pan);

    let accepted = paint_frame(&mut app, &ctx, vec![key_event(egui::Key::ArrowRight, true)]);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "move within the same source image"
    );
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Right
    );
    assert_eq!(app.fs_zoom, 1.0);
    assert_unit(&accepted, 1, 800.0, 800.0);
    if accepted.pages[0].uv_rect == before.pages[0].uv_rect {
        let request = accepted
            .request
            .expect("old half paint cannot consume the accepted new-half request");
        assert_eq!(accepted.pan, manual_pan);
        assert_eq!(request.origin_pan, manual_pan);
        assert_eq!(request.applied_pan, manual_pan);
    } else {
        assert_eq!(
            accepted.pages[0].uv_rect,
            crate::page_split::PageSlice::Right.source_bbox(crate::rotation_db::Rotation::None)
        );
        assert_reading_start(&accepted, ReadingDirection::Ltr);
    }
    let next = paint_frame(
        &mut app,
        &ctx,
        vec![key_event(egui::Key::ArrowRight, false)],
    );
    assert_unit(&next, 1, 800.0, 800.0);
    assert_eq!(
        next.pages[0].uv_rect,
        crate::page_split::PageSlice::Right.source_bbox(crate::rotation_db::Rotation::None)
    );
    assert_reading_start(&next, ReadingDirection::Ltr);
}
