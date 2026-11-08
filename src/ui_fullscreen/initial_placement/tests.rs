use super::*;
use crate::settings::FullscreenPageStartPosition;
use crate::ui_fullscreen::page_turn_submission_tests::{frame, ready_app};

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(30.0, 50.0), egui::vec2(600.0, 400.0))
}

fn overflowing_unit() -> egui::Rect {
    egui::Rect::from_center_size(viewport().center(), egui::vec2(900.0, 800.0))
}

fn reading_app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
    app
}

fn close_to(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.01,
        "actual={actual}, expected={expected}"
    );
}

fn paint_single(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
    selected_current: bool,
    rotation: crate::rotation_db::Rotation,
    trim: Option<egui::Rect>,
    placement: SingletonSpreadPlacement,
) -> DisplayedImageTransform {
    let idx = app.fullscreen_idx.unwrap();
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let resource = app.fullscreen_paint_payload_for_page(idx, material, false);
    let source_size = app
        .known_page_dims_for_idx(idx)
        .map(|(w, h)| egui::vec2(w as f32, h as f32));
    let zoom_pan = app.fs_zoom_pan();
    let fit = app.effective_fullscreen_fit_mode();
    let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            rect,
            idx,
            source_size,
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            placement,
            resource.as_ref(),
            None,
            false,
            false,
            false,
            rotation,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::BLACK),
            "",
            false,
            fit,
            limits,
            trim,
            None,
            app.settings.page_turn_animation_enabled,
            None,
            page_turn::FlatCropPaint::WithPage,
            selected_current,
        )
        .expect("ready single page paints");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout.push(transform);
    transform
}

fn paint_spread(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
    current: bool,
) {
    let anchor = app.fullscreen_idx.unwrap();
    let composition = app.spread_display_composition_for_anchor(anchor);
    let pages = composition.pages_in_screen_order(app.spread_mode);
    assert_eq!(pages.len(), 2);
    app.draw_fs_spread_with_placement(
        ui,
        ctx,
        rect,
        pages[0].idx,
        pages[1].idx,
        false,
        FsPageTurnDecision::normal(),
        None,
        StillSeekGeometry::bar_only(),
        false,
        Some(&composition),
        current,
        None,
    );
}

#[test]
fn initial_placement_geometry_aligns_only_overflowing_axes_and_respects_direction() {
    let v = viewport();
    let base = egui::vec2(17.0, -23.0);
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let tall = egui::Rect::from_center_size(v.center(), egui::vec2(300.0, 800.0));
        assert_eq!(
            reading_start_pan(tall, v, direction, base),
            egui::vec2(base.x, 200.0)
        );
        let wide = egui::Rect::from_center_size(v.center(), egui::vec2(900.0, 200.0));
        let x = if direction == ReadingDirection::Ltr {
            150.0
        } else {
            -150.0
        };
        assert_eq!(
            reading_start_pan(wide, v, direction, base),
            egui::vec2(x, base.y)
        );
        assert_eq!(
            reading_start_pan(overflowing_unit(), v, direction, base),
            egui::vec2(x, 200.0)
        );
        assert_eq!(reading_start_pan(v, v, direction, base), base);
        let small = egui::Rect::from_min_size(v.min, egui::vec2(150.0, 200.0));
        assert_eq!(reading_start_pan(small, v, direction, base), base);
    }
}

#[test]
fn initial_placement_center_creates_no_request_and_does_not_change_split_pan() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.fs_pan = egui::vec2(24.0, -31.0);
    app.request_fs_initial_placement(0);
    assert!(app.fs_initial_placement_request.is_none());
    assert!(!app.resolve_fs_initial_placement(0, true, viewport(), Some(overflowing_unit())));
    assert_eq!(app.fs_pan, egui::vec2(24.0, -31.0));
}

#[test]
fn initial_placement_holdover_and_missing_geometry_retain_until_first_current_frame() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    let request = app.fs_initial_placement_request;
    assert!(!app.resolve_fs_initial_placement(0, false, viewport(), Some(overflowing_unit())));
    assert_eq!(app.fs_initial_placement_request, request);
    assert!(!app.resolve_fs_initial_placement(0, true, viewport(), None));
    assert_eq!(app.fs_initial_placement_request, request);
    assert!(app.resolve_fs_initial_placement(0, true, viewport(), Some(overflowing_unit())));
    assert!(app.fs_initial_placement_request.is_none());
    let pan = app.fs_pan;
    assert!(!app.resolve_fs_initial_placement(0, true, viewport(), Some(viewport())));
    assert_eq!(app.fs_pan, pan);
}

#[test]
fn initial_placement_stale_identity_and_changed_transform_discard_without_moving() {
    let ctx = egui::Context::default();
    for change in 0..4 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        let mut anchor = 0;
        match change {
            0 => app.items_generation += 1,
            1 => anchor = 1,
            2 => app.fs_zoom = 2.0,
            _ => app.fs_pan = egui::vec2(9.0, 12.0),
        }
        let before = (app.fs_zoom, app.fs_pan);
        assert!(!app.resolve_fs_initial_placement(anchor, false, viewport(), None));
        assert!(app.fs_initial_placement_request.is_none(), "case {change}");
        assert_eq!((app.fs_zoom, app.fs_pan), before);
    }
}

#[test]
fn initial_placement_replacement_request_uses_latest_anchor_and_transform() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(14.0, 22.0);
    app.request_fs_initial_placement(1);
    assert!(app.resolve_fs_initial_placement(1, true, viewport(), Some(viewport())));
    assert_eq!((app.fs_zoom, app.fs_pan), (2.0, egui::vec2(14.0, 22.0)));
}

#[test]
fn initial_placement_single_uses_book_direction_when_mode_has_no_direction() {
    let ctx = egui::Context::default();
    for (mode, book, expected_x) in [
        (SpreadMode::Single, ReadingDirection::Ltr, 150.0),
        (SpreadMode::Single, ReadingDirection::Rtl, -150.0),
        (SpreadMode::Ltr, ReadingDirection::Rtl, 150.0),
        (SpreadMode::Rtl, ReadingDirection::Ltr, -150.0),
    ] {
        let mut app = reading_app(&ctx, mode);
        app.reading_direction = book;
        app.request_fs_initial_placement(0);
        assert!(app.resolve_fs_initial_placement(0, true, viewport(), Some(overflowing_unit())));
        assert_eq!(app.fs_pan, egui::vec2(expected_x, 200.0));
    }
}

#[test]
fn initial_placement_single_paint_starts_at_top_in_the_first_painted_frame() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let t = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
        assert!(t.paint_rect.height() > viewport().height());
        close_to(t.paint_rect.top(), viewport().top());
        close_to(t.paint_rect.center().x, viewport().center().x);
        assert!(app.fs_initial_placement_request.is_none());
        assert_eq!(app.fs_zoom, 1.0);
    });
    let pan = app.fs_pan;
    frame(&ctx, |ui, ctx| {
        let resized = egui::Rect::from_min_size(viewport().min, egui::vec2(500.0, 300.0));
        paint_single(
            &mut app,
            ui,
            ctx,
            resized,
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
    });
    assert_eq!(app.fs_pan, pan, "resize must not reapply entry alignment");
}

#[test]
fn initial_placement_single_paint_respects_trim_rotation_and_reserved_viewport() {
    let ctx = egui::Context::default();
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.reading_direction = direction;
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Height;
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            let t = paint_single(
                &mut app,
                ui,
                ctx,
                viewport(),
                true,
                crate::rotation_db::Rotation::Cw90,
                Some(egui::Rect::from_min_max(
                    egui::pos2(0.1, 0.0),
                    egui::pos2(0.9, 1.0),
                )),
                SingletonSpreadPlacement::Center,
            );
            assert!(t.paint_rect.width() > viewport().width());
            match direction {
                ReadingDirection::Ltr => close_to(t.paint_rect.left(), viewport().left()),
                ReadingDirection::Rtl => close_to(t.paint_rect.right(), viewport().right()),
            }
            close_to(t.paint_rect.center().y, viewport().center().y);
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn initial_placement_single_paint_keeps_nonoverflowing_singleton_side() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::LtrCover);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    let mut before = None;
    frame(&ctx, |ui, ctx| {
        before = Some(paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Right,
        ));
    });
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let after = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Right,
        );
        assert_eq!(after.paint_rect, before.unwrap().paint_rect);
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    });
}

#[test]
fn initial_placement_selected_holdover_does_not_consume_single_or_spread_request() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            if mode == SpreadMode::Single {
                paint_single(
                    &mut app,
                    ui,
                    ctx,
                    viewport(),
                    false,
                    crate::rotation_db::Rotation::None,
                    None,
                    SingletonSpreadPlacement::Center,
                );
            } else {
                paint_spread(&mut app, ui, ctx, viewport(), false);
            }
            assert!(app.fs_initial_placement_request.is_some());
            assert_eq!(app.fs_pan, egui::Vec2::ZERO);
        });
    }
}

#[test]
fn initial_placement_spread_paint_aligns_unit_union_in_the_first_frame() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        frame(&ctx, |ui, ctx| {
            paint_spread(&mut app, ui, ctx, viewport(), true);
            let left = app
                .fullscreen_page_layout
                .page_by_idx(0)
                .expect("page zero")
                .transform;
            let right = app
                .fullscreen_page_layout
                .page_by_idx(1)
                .expect("page one")
                .transform;
            let union = left.paint_rect.union(right.paint_rect);
            assert!(union.height() > viewport().height());
            close_to(union.top(), viewport().top());
            close_to(union.center().x, viewport().center().x);
            assert!(app.fs_initial_placement_request.is_none());
            assert_eq!(app.fs_zoom, 1.0);
        });
    }
}

#[test]
fn initial_placement_navigation_holdover_preserves_old_single_and_spread_position() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let mut app = reading_app(&ctx, mode);
        app.request_fs_initial_placement(0);
        let mut old_rect = None;
        frame(&ctx, |ui, ctx| {
            if mode == SpreadMode::Single {
                paint_single(
                    &mut app,
                    ui,
                    ctx,
                    viewport(),
                    true,
                    crate::rotation_db::Rotation::None,
                    None,
                    SingletonSpreadPlacement::Center,
                );
            } else {
                paint_spread(&mut app, ui, ctx, viewport(), true);
            }
            old_rect = Some(
                app.fullscreen_page_layout
                    .page_by_idx(0)
                    .unwrap()
                    .transform
                    .paint_rect,
            );
        });
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        let unit = app
            .capture_fs_display_unit_with_rendition(Some(&ctx), 0)
            .expect("capture painted unit");
        app.fullscreen_idx = Some(2);
        app.fs_pan = egui::Vec2::ZERO;
        app.request_fs_initial_placement(2);
        frame(&ctx, |ui, ctx| {
            app.draw_fs_display_unit_holdover_for_selection(
                ui,
                ctx,
                viewport(),
                &unit,
                StillSeekGeometry::bar_only(),
                FsHoldoverTransform::NavigationPrevious,
            );
            let held = app
                .fullscreen_page_layout
                .page_by_idx(0)
                .unwrap()
                .transform
                .paint_rect;
            close_to(held.left(), old_rect.unwrap().left());
            close_to(held.top(), old_rect.unwrap().top());
            assert_eq!(app.fs_pan, egui::Vec2::ZERO);
            assert!(app.fs_initial_placement_request.is_some());
        });
    }
}

#[test]
fn initial_placement_source_reload_holdover_keeps_live_manual_pan() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
    });
    let unit = app
        .capture_fs_display_unit_with_rendition(Some(&ctx), 0)
        .unwrap();
    let old_top = app
        .fullscreen_page_layout
        .page_by_idx(0)
        .unwrap()
        .transform
        .paint_rect
        .top();
    app.fs_pan.y += 35.0;
    frame(&ctx, |ui, ctx| {
        app.draw_fs_display_unit_holdover_for_selection(
            ui,
            ctx,
            viewport(),
            &unit,
            StillSeekGeometry::bar_only(),
            FsHoldoverTransform::Current,
        );
        close_to(
            app.fullscreen_page_layout
                .page_by_idx(0)
                .unwrap()
                .transform
                .paint_rect
                .top(),
            old_top + 35.0,
        );
    });
}

#[test]
fn initial_placement_input_pan_pinch_and_key_zoom_cancel_even_when_values_return_to_base() {
    let ctx = egui::Context::default();
    for input in 0..4 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match input {
            0 => app.set_fs_pan_from_input(1.0, egui::vec2(10.0, 15.0)),
            1 => {
                assert!(app.apply_touch_zoom_factor_about_pivot(
                    1.25,
                    viewport().center(),
                    viewport().center()
                ));
            }
            2 => {
                assert!(app.apply_fs_key_zoom_step(&ctx, true));
            }
            _ => app.apply_touch_pan_delta(egui::vec2(10.0, 15.0), viewport()),
        }
        assert!(app.fs_initial_placement_request.is_none(), "input {input}");
        app.fs_zoom = 1.0;
        app.fs_pan = egui::Vec2::ZERO;
        assert!(!app.resolve_fs_initial_placement(0, true, viewport(), Some(overflowing_unit())));
        assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    }
}

#[test]
fn initial_placement_fit_continuous_and_edit_resets_clear_the_request() {
    let ctx = egui::Context::default();
    for reset in 0..3 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match reset {
            0 => app.reset_fullscreen_fit_transform(0),
            1 => app.reset_continuous_reading_transform(),
            _ => app.enter_page_edit_single_view(0),
        }
        assert!(app.fs_initial_placement_request.is_none(), "reset {reset}");
    }
}

#[test]
fn initial_placement_split_reconciliation_never_recreates_a_consumed_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
    app.record_page_dims_for_spread(0, (240, 120));
    app.fullscreen_page_slice = crate::page_split::PageSlice::Full;
    app.fs_pan = egui::vec2(27.0, 33.0);
    app.reconcile_fullscreen_page_slice(0);
    assert!(app.fullscreen_page_slice.is_half());
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_pan, egui::vec2(27.0, 33.0));
    app.spread_mode = SpreadMode::Single;
    app.reconcile_fullscreen_page_slice(0);
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Full
    );
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(app.fs_pan, egui::vec2(27.0, 33.0));
}

#[test]
fn initial_placement_accepted_same_image_split_step_requests_only_reading_start() {
    let ctx = egui::Context::default();
    for reading_start in [false, true] {
        let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
        if !reading_start {
            app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
        }
        app.record_page_dims_for_spread(0, (240, 120));
        app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
        app.fs_zoom = 2.0;
        app.fs_pan = egui::vec2(13.0, 17.0);
        app.handle_fs_navigation(
            &ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Split(crate::page_split::PresentationStep {
                source_idx: 0,
                slice: crate::page_split::PageSlice::Right,
            }),
            None,
            0,
        );
        assert_eq!(
            app.fullscreen_page_slice,
            crate::page_split::PageSlice::Right
        );
        assert_eq!(app.fs_zoom, 2.0);
        assert_eq!(app.fs_pan, egui::vec2(13.0, 17.0));
        assert_eq!(app.fs_initial_placement_request.is_some(), reading_start);
    }
}

#[test]
fn initial_placement_excluded_modes_retire_requests_without_changing_position() {
    let ctx = egui::Context::default();
    for mode in 0..12 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        match mode {
            0 => app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center,
            1 => app.fs_zoom_active = true,
            2 => app.fs_zoom_aiming = true,
            3 => app.analysis_mode = true,
            4 => app.reading_flow = ReadingFlow::Vertical,
            5 => app.reading_flow = ReadingFlow::Horizontal,
            6 => app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5),
            7 => app.view_trim_mode = true,
            8 => app.erase_mode = true,
            9 => app.items[0] = GridItem::Video(PathBuf::from("c:/test/entry-video.mp4")),
            10 => app.items[0] = GridItem::Audio(PathBuf::from("c:/test/entry-audio.mp3")),
            _ => {
                app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Depth(
                    crate::depth_parallax::DepthViewState::new(
                        crate::app::ViewerContextId::for_test(82),
                    ),
                )
            }
        }
        let before = (app.fs_zoom, app.fs_pan);
        app.retire_fs_initial_placement_request();
        assert!(app.fs_initial_placement_request.is_none(), "mode {mode}");
        assert_eq!((app.fs_zoom, app.fs_pan), before);
    }
}

#[test]
fn initial_placement_slideshow_same_image_slice_creates_request_without_reopening() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::SplitLtr);
    app.record_page_dims_for_spread(0, (240, 120));
    app.fullscreen_page_slice = crate::page_split::PageSlice::Left;
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(13.0, 17.0);
    let generation = app.items_generation;
    app.complete_book_slideshow_target(
        &ctx,
        0,
        Some(crate::page_split::PageSlice::Right),
        crate::app::book_composition_permission::BookTargetPurpose::Slideshow,
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.fullscreen_page_slice,
        crate::page_split::PageSlice::Right
    );
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.fs_zoom, 2.0);
    assert_eq!(app.fs_pan, egui::vec2(13.0, 17.0));
    let request = app
        .fs_initial_placement_request
        .expect("accepted same-image slideshow half");
    assert_eq!(request.anchor_idx, 0);
    assert_eq!(request.base_zoom, 2.0);
    assert_eq!(request.base_pan, egui::vec2(13.0, 17.0));
}

#[test]
fn initial_placement_close_clears_unresolved_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    app.close_fullscreen();
    assert!(app.fs_initial_placement_request.is_none());
    assert!(app.fullscreen_idx.is_none());
}

#[test]
fn initial_placement_panorama_retires_request() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
        *source_dims = Some([2000, 1000]);
    }
    app.fs_exclusive_view = crate::fs_exclusive_view::FsExclusiveView::Panorama(
        crate::panorama::PanoramaState::new(0.0, 0.0, crate::panorama::PanoProjection::Perspective),
    );
    assert_eq!(app.fs_zoom_owner(), FsZoomOwner::Panorama);
    app.retire_fs_initial_placement_request();
    assert!(app.fs_initial_placement_request.is_none());
}

#[cfg(windows)]
#[test]
fn initial_placement_context_swap_keeps_sibling_requests_and_pan_separate() {
    let ctx = egui::Context::default();
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.request_fs_initial_placement(0);
    let main_request = app.fs_initial_placement_request;
    let child = app.build_window_context_for_test(8126, |owner| {
        assert!(owner.fs_initial_placement_request.is_none());
        owner.items = vec![GridItem::Image(PathBuf::from("c:/test/child-entry.png"))];
        owner.fullscreen_idx = Some(0);
        owner.items_generation = 93;
        owner.fs_pan = egui::vec2(12.0, 18.0);
        owner.request_fs_initial_placement(0);
    });
    assert_eq!(app.fs_initial_placement_request, main_request);
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(
            owner.fs_initial_placement_request.unwrap().items_generation,
            93
        );
        assert!(owner.resolve_fs_initial_placement(0, true, viewport(), Some(overflowing_unit())));
        assert_ne!(owner.fs_pan, egui::Vec2::ZERO);
    });
    assert_eq!(app.fs_initial_placement_request, main_request);
    assert_eq!(app.fs_pan, egui::Vec2::ZERO);
    app.with_window_context_for_test(child, |owner| {
        assert!(owner.fs_initial_placement_request.is_none())
    });
}

#[cfg(windows)]
#[test]
fn initial_placement_parked_geometry_resets_clear_only_the_affected_context_request() {
    let ctx = egui::Context::default();
    for reset in 0..3 {
        let mut app = reading_app(&ctx, SpreadMode::Single);
        app.request_fs_initial_placement(0);
        let main_request = app.fs_initial_placement_request;
        let child = app.build_window_context_for_test(8127 + reset, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/test/parked-entry.png"))];
            owner.fullscreen_idx = Some(0);
            owner.spread_mode = SpreadMode::LtrCover;
            owner.fs_pan = egui::vec2(12.0, 18.0);
            owner.request_fs_initial_placement(0);
        });
        match reset {
            0 => app.invalidate_final_cover_spread_display_in_parked_contexts(),
            1 => app.invalidate_singleton_spread_placement_in_parked_contexts(true, true),
            _ => app.invalidate_page_alone_in_parked_contexts(true, true),
        }
        assert_eq!(app.fs_initial_placement_request, main_request);
        app.with_window_context_for_test(child, |owner| {
            assert!(
                owner.fs_initial_placement_request.is_none(),
                "reset {reset}"
            );
            assert_eq!(owner.fs_pan, egui::Vec2::ZERO);
        });
    }
}

#[test]
fn initial_placement_page_turn_target_paints_aligned_before_disabling_nonzero_pan_animation() {
    let ctx = egui::Context::default();
    page_turn::install(&ctx);
    let mut app = reading_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        let t = paint_single(
            &mut app,
            ui,
            ctx,
            viewport(),
            true,
            crate::rotation_db::Rotation::None,
            None,
            SingletonSpreadPlacement::Center,
        );
        close_to(t.paint_rect.top(), viewport().top());
        assert_ne!(app.fs_pan, egui::Vec2::ZERO);
        assert!(!app.page_turn_view_eligible(0));
    });
    assert!(app.fs_initial_placement_request.is_none());
}
