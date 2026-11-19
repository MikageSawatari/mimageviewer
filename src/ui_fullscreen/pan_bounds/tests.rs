use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;
use crate::rotation_db::Rotation;
use crate::settings::FullscreenPageStartPosition;
use crate::ui_fullscreen::page_turn_submission_tests::ready_app;

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))
}

fn frame(ctx: &egui::Context, rect: egui::Rect, dpi: f32, mut body: impl FnMut(&mut egui::Ui)) {
    let mut input = egui::RawInput {
        screen_rect: Some(rect),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(dpi);
    crate::test_ui::headless_paint_output(ctx.run_ui(input, |ui| body(ui)))
        .drop_without_applying_deltas();
}

fn app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.settings.page_turn_animation_enabled = false;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.still_seek_strip_visible = false;
    app.settings.margin_fit_enabled = false;
    app
}

fn resource(app: &App, idx: usize, kind: FullscreenPageMaterialKind) -> FullscreenPaintPayload {
    let tex = match app.fs_cache.get(&idx).unwrap() {
        FsCacheEntry::Static { tex, .. } => tex.clone(),
        _ => panic!("static test source"),
    };
    FullscreenPaintResource::resampleable(
        idx,
        tex,
        crate::gpu_lanczos::FullscreenPaintSourceGeneration {
            items: app.items_generation,
            input: app.input_seq,
        },
    )
    .with_page_material_kind(kind)
    .into()
}

fn single(
    app: &mut App,
    ui: &mut egui::Ui,
    payload: &FullscreenPaintPayload,
    current: bool,
    original: bool,
    rotation: Rotation,
    side: SingletonSpreadPlacement,
) -> DisplayedImageTransform {
    let idx = app.fullscreen_idx.unwrap();
    let rect = ui.ctx().content_rect();
    let mut sources = FsNavigatorTextureSources::default();
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            rect,
            idx,
            Some(payload.full_source_size_vec2()),
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            side,
            Some(payload),
            None,
            false,
            false,
            false,
            rotation,
            app.fs_zoom_pan(),
            0.0,
            &FsBgStyle::Solid(egui::Color32::BLACK),
            "",
            false,
            app.effective_fullscreen_fit_mode(),
            app.fullscreen_fit_scale_limits(ui.ctx().pixels_per_point()),
            None,
            Some(&mut sources),
            false,
            None,
            page_turn::FlatCropPaint::WithPage,
            current,
            original,
        )
        .unwrap();
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    let composition = app.spread_display_composition_for_anchor(idx);
    let occurrence = composition
        .pages_in_screen_order(app.spread_mode)
        .into_iter()
        .find(|page| page.idx == idx)
        .unwrap();
    app.fullscreen_page_layout
        .push_occurrence(transform, occurrence);
    assert!(sources.rebind_occurrences(&[occurrence], || app.page_turn_unit_binding(idx)));
    if current {
        app.record_fs_scroll_drawn_unit(ui.ctx(), rect, &composition, &sources);
    }
    transform
}

fn spread(app: &mut App, ui: &mut egui::Ui, current: bool, original: bool) {
    let ctx = ui.ctx().clone();
    app.draw_fs_spread_with_placement(
        ui,
        &ctx,
        ctx.content_rect(),
        0,
        1,
        original,
        FsPageTurnDecision::normal(),
        None,
        StillSeekGeometry::bar_only(),
        false,
        None,
        current,
        None,
    );
}

fn assert_painted_pan(app: &App) {
    for page in app.fullscreen_page_layout.displayed_pages() {
        assert_eq!(normal_zoom_pan(page.transform).unwrap().1, app.fs_pan);
    }
    let pages: Vec<_> = app
        .fullscreen_page_layout
        .displayed_pages()
        .map(|p| p.transform)
        .collect();
    assert_eq!(draw_pan_correction(&pages), egui::Vec2::ZERO);
}

#[test]
fn rotated_single_is_bounded_in_the_same_frame_with_matching_layout_and_stamp() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    app.fs_pan = egui::vec2(2400.0, 37.25);
    frame(&ctx, viewport(), 1.0, |ui| {
        single(
            &mut app,
            ui,
            &payload,
            true,
            false,
            Rotation::None,
            SingletonSpreadPlacement::Center,
        );
        assert!(app.fs_pan.x < 600.0);
        assert_eq!(app.fs_pan.y, 37.25);
        assert_painted_pan(&app);
        assert!(app.fullscreen_page_layout.drawn_unit.is_some());
    });
    let pan = app.fs_pan;
    frame(&ctx, viewport(), 1.0, |ui| {
        single(
            &mut app,
            ui,
            &payload,
            true,
            false,
            Rotation::None,
            SingletonSpreadPlacement::Center,
        );
        assert_eq!(app.fs_pan, pan);
    });
}

#[test]
fn rotation_keeps_in_range_pan_exactly_and_rebounds_the_rest() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    for pan in [egui::vec2(19.125, 7.375), egui::vec2(2400.0, 0.0)] {
        app.fs_pan = pan;
        frame(&ctx, viewport(), 1.0, |ui| {
            single(
                &mut app,
                ui,
                &payload,
                true,
                false,
                Rotation::Cw90,
                SingletonSpreadPlacement::Center,
            );
            if pan.x < 100.0 {
                assert_eq!(app.fs_pan, pan);
            }
            assert_painted_pan(&app);
        });
        frame(&ctx, viewport(), 1.0, |ui| {
            single(
                &mut app,
                ui,
                &payload,
                true,
                false,
                Rotation::None,
                SingletonSpreadPlacement::Center,
            );
            assert_painted_pan(&app);
        });
    }
}

#[test]
fn tiny_rotated_pages_remain_visible_and_stationary_after_snap_correction() {
    for dpi in [1.0, 1.25, 1.5] {
        for thin_pixels in [10, 20] {
            for horizontal in [true, false] {
                for sign in [-1.0, 1.0] {
                    let ctx = egui::Context::default();
                    let mut app = app(&ctx, SpreadMode::Single);
                    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
                    app.fs_zoom = 0.1;
                    // Keep the unrotated long side at 100pt for every DPI, so
                    // its edge position still meets the input-side 48pt rule.
                    let long_pixels = (1000.0 * dpi) as usize;
                    let size = if horizontal {
                        [long_pixels, thin_pixels]
                    } else {
                        [thin_pixels, long_pixels]
                    };
                    let image = egui::ColorImage::filled(size, egui::Color32::WHITE);
                    let texture = ctx.load_texture(
                        "tiny-rotated-page",
                        image.clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    let FsCacheEntry::Static {
                        tex,
                        pixels,
                        source_dims,
                        ..
                    } = app.fs_cache.get_mut(&0).unwrap()
                    else {
                        panic!("static source")
                    };
                    *tex = texture;
                    *pixels = Arc::new(image);
                    *source_dims = Some(size);
                    app.record_page_dims_for_spread(0, (size[0] as u32, size[1] as u32));
                    let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
                    let pixel = 1.0 / dpi;
                    let pan = sign
                        * if horizontal {
                            egui::vec2(viewport().width() * 0.5 + pixel, 0.0)
                        } else {
                            egui::vec2(0.0, viewport().height() * 0.5 + pixel)
                        };
                    app.fs_pan = pan;
                    frame(&ctx, viewport(), dpi, |ui| {
                        single(
                            &mut app,
                            ui,
                            &payload,
                            true,
                            false,
                            Rotation::None,
                            SingletonSpreadPlacement::Center,
                        );
                        assert_eq!(app.fs_pan, pan);
                        assert_eq!(
                            clamp_fullscreen_pan_from_layout(
                                &app.fullscreen_page_layout,
                                app.fs_zoom,
                                pan
                            ),
                            pan
                        );
                    });
                    // Independent oracle: actual painted bounds, not the result
                    // of the pan helper whose tolerance caused the regression.
                    let assert_visible = |t: DisplayedImageTransform| {
                        let overlap_x = t.paint_rect.right().min(t.viewport_rect.right())
                            - t.paint_rect.left().max(t.viewport_rect.left());
                        let overlap_y = t.paint_rect.bottom().min(t.viewport_rect.bottom())
                            - t.paint_rect.top().max(t.viewport_rect.top());
                        assert!(
                            overlap_x > 0.0 && overlap_y > 0.0,
                            "dpi={dpi}, thin={thin_pixels}, horizontal={horizontal}, sign={sign}, rect={:?}",
                            t.paint_rect
                        );
                        let thin_extent = if horizontal {
                            t.paint_rect.width()
                        } else {
                            t.paint_rect.height()
                        };
                        assert!((thin_extent * dpi - thin_pixels as f32 * 0.1).abs() < 0.001);
                    };
                    app.rotation_cache.insert(0, Rotation::Cw90);
                    frame(&ctx, viewport(), dpi, |ui| {
                        let t = single(
                            &mut app,
                            ui,
                            &payload,
                            true,
                            false,
                            Rotation::Cw90,
                            SingletonSpreadPlacement::Center,
                        );
                        assert_visible(t);
                        assert_eq!(normal_zoom_pan(t).unwrap().1, app.fs_pan);
                        assert!((app.fs_pan - pan).abs().max_elem() * dpi <= 2.001);
                    });
                    let bounded = app.fs_pan;
                    for _ in 0..2 {
                        frame(&ctx, viewport(), dpi, |ui| {
                            let t = single(
                                &mut app,
                                ui,
                                &payload,
                                true,
                                false,
                                Rotation::Cw90,
                                SingletonSpreadPlacement::Center,
                            );
                            assert_visible(t);
                            assert_eq!(app.fs_pan, bounded);
                            assert_eq!(draw_pan_correction(&[t]), egui::Vec2::ZERO);
                        });
                    }
                }
            }
        }
    }
}

#[test]
fn snap_tolerance_requires_positive_area_and_preserves_visible_residuals() {
    let vp = viewport();
    let page = |rect, dpi| {
        DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: vp,
                source_size: egui::vec2(1.0, 20.0),
                texture_size: egui::vec2(1.0, 20.0),
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::Legacy(FullscreenFitMode::Original),
                fit_scale_limits: Default::default(),
                pixels_per_point: dpi,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                pixel_fit: RectPixelFit::Proportional,
            },
            rect,
        )
        .unwrap()
    };
    for dpi in [1.0, 1.25, 1.5] {
        for width in [1.0, 2.0] {
            let pixel = 1.0 / dpi;
            let visible = egui::Rect::from_min_size(
                egui::pos2(vp.right() - width * pixel * 0.5, 100.0),
                egui::vec2(width * pixel, 20.0),
            );
            assert_eq!(draw_pan_correction(&[page(visible, dpi)]), egui::Vec2::ZERO);
            // A two-pixel page touches the edge; a one-pixel page is one pixel
            // beyond it. Both need <= 2px and both used to stay entirely hidden.
            let outside = egui::Rect::from_min_size(
                egui::pos2(vp.right() + (2.0 - width) * pixel, 100.0),
                egui::vec2(width * pixel, 20.0),
            );
            let correction = draw_pan_correction(&[page(outside, dpi)]);
            assert!(correction.x < 0.0);
            assert_eq!(correction.y, 0.0);
            let actual = outside.translate(correction);
            assert!(actual.right().min(vp.right()) - actual.left().max(vp.left()) > 0.0);
            assert!(actual.bottom().min(vp.bottom()) - actual.top().max(vp.top()) > 0.0);
            assert_eq!(draw_pan_correction(&[page(actual, dpi)]), egui::Vec2::ZERO);
        }
    }
}

#[test]
fn partner_rotation_to_single_restricts_pan_without_a_rotation_history() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Height;
    app.settings.spread_page_gap_px = 0;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
    app.fs_pan = egui::vec2(850.0, 0.0);
    frame(&ctx, rect, 1.0, |ui| spread(&mut app, ui, true, false));
    assert_eq!(app.fs_pan.x, 850.0);
    // The composition producer drops the rotated partner; the remaining page's
    // rotation is unchanged. There is no previous-rotation state to consult.
    app.spread_mode = SpreadMode::Single;
    let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    frame(&ctx, rect, 1.0, |ui| {
        single(
            &mut app,
            ui,
            &payload,
            true,
            false,
            Rotation::None,
            SingletonSpreadPlacement::Center,
        );
        assert_eq!(app.fs_pan.x, 652.0);
        assert_painted_pan(&app);
    });
}

#[test]
fn automatic_rotation_correction_preserves_clicks_but_manual_pan_stays_manual() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    for automatic in [true, false] {
        app.fs_pan = egui::vec2(2200.0, 0.0);
        if automatic {
            let pan = app.fs_pan;
            app.record_fs_automatic_initial_view(pan);
        } else {
            app.fs_automatic_initial_view = None;
        }
        frame(&ctx, viewport(), 1.0, |ui| {
            single(
                &mut app,
                ui,
                &payload,
                true,
                false,
                Rotation::None,
                SingletonSpreadPlacement::Center,
            );
        });
        assert_eq!(
            app.fs_view_has_moved(app.fs_zoom, app.fs_pan, 0.0),
            !automatic
        );
    }
}

#[test]
fn provisional_and_unselected_frames_keep_pan_until_complete_current_material() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    let complete = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    let provisional = resource(&app, 0, FullscreenPageMaterialKind::Provisional);
    let pan = egui::vec2(2200.0, -1700.0);
    for (payload, current) in [(&complete, false), (&provisional, true)] {
        app.fs_pan = pan;
        frame(&ctx, viewport(), 1.0, |ui| {
            single(
                &mut app,
                ui,
                payload,
                current,
                false,
                Rotation::None,
                SingletonSpreadPlacement::Center,
            );
        });
        assert_eq!(app.fs_pan, pan);
    }
    frame(&ctx, viewport(), 1.0, |ui| {
        single(
            &mut app,
            ui,
            &complete,
            true,
            false,
            Rotation::None,
            SingletonSpreadPlacement::Center,
        );
        assert_painted_pan(&app);
    });
    assert_ne!(app.fs_pan, pan);
}

#[test]
fn partial_spread_does_not_restrict_pan_and_first_complete_spread_does() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    let right = app.fs_cache.remove(&1).unwrap();
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.fs_pan = egui::vec2(2200.0, 0.0);
    frame(&ctx, viewport(), 1.0, |ui| {
        spread(&mut app, ui, true, false)
    });
    assert_eq!(app.fs_pan.x, 2200.0);
    app.fs_cache.insert(1, right);
    frame(&ctx, viewport(), 1.0, |ui| {
        spread(&mut app, ui, true, false);
        assert_painted_pan(&app);
    });
    assert!(app.fs_pan.x < 1000.0);
}

#[test]
fn original_preview_round_trip_keeps_processed_pan_for_single_and_spread() {
    let ctx = egui::Context::default();
    ctx.input_mut(|input| input.max_texture_side = 4096);
    let pixels = Arc::new(egui::ColorImage::filled([1600, 2400], egui::Color32::WHITE));
    let processed_textures = [0, 1].map(|idx| {
        ctx.load_texture(
            format!("processed-original-round-trip-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        )
    });
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let mut app = app(&ctx, mode);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
        for idx in 0..2 {
            let key = app.final_composite_key_for_pixels(
                app.current_edit_result_key(idx),
                [80, 120],
                app.effective_params(idx),
            );
            app.final_composite_cache.insert(
                key,
                crate::app::FinalCompositeEntry::raster(
                    Some(crate::page_dims::CanonicalPageDimensions::new(80, 120)),
                    [80, 120],
                    pixels.clone(),
                    processed_textures[idx].clone(),
                    egui::TextureOptions::LINEAR,
                    true,
                ),
            );
        }
        let processed: FullscreenPaintPayload = FullscreenPaintResource::resampleable(
            0,
            processed_textures[0].clone(),
            crate::gpu_lanczos::FullscreenPaintSourceGeneration {
                items: app.items_generation,
                input: app.input_seq,
            },
        )
        .with_page_material_kind(FullscreenPageMaterialKind::Complete)
        .into();
        let original = resource(&app, 0, FullscreenPageMaterialKind::Complete);
        app.fs_pan = egui::vec2(1000.0, 0.0);
        let pan = app.fs_pan;
        for preview in [false, true, false] {
            frame(&ctx, viewport(), 1.0, |ui| {
                if mode == SpreadMode::Single {
                    single(
                        &mut app,
                        ui,
                        if preview { &original } else { &processed },
                        true,
                        preview,
                        Rotation::None,
                        SingletonSpreadPlacement::Center,
                    );
                } else {
                    spread(&mut app, ui, true, preview);
                }
                assert_eq!(app.fs_pan, pan, "{mode:?}, original={preview}");
            });
        }
    }
}

#[test]
fn white_singleton_reading_start_is_unchanged_on_later_complete_frames_ltr_and_rtl() {
    let ctx = egui::Context::default();
    for (mode, side) in [
        (SpreadMode::Ltr, SingletonSpreadPlacement::RightWhite),
        (SpreadMode::Rtl, SingletonSpreadPlacement::LeftWhite),
    ] {
        let mut app = app(&ctx, mode);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Height;
        app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 855.0));
        let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
        app.request_fs_initial_placement(0);
        frame(&ctx, rect, 1.0, |ui| {
            single(&mut app, ui, &payload, true, false, Rotation::None, side);
        });
        let pan = app.fs_pan;
        assert!(pan.x.abs() > 200.0);
        for _ in 0..3 {
            frame(&ctx, rect, 1.0, |ui| {
                single(&mut app, ui, &payload, true, false, Rotation::None, side);
                assert_eq!(app.fs_pan, pan);
                assert_painted_pan(&app);
            });
        }
    }
}

#[test]
fn half_page_initial_placement_bounds_retained_axis_and_preserves_origin_identity() {
    let ctx = egui::Context::default();
    for automatic in [true, false] {
        for x in [25.0, 2400.0] {
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
            app.fs_pan = egui::vec2(x, 40.0);
            if automatic {
                let pan = app.fs_pan;
                app.record_fs_automatic_initial_view(pan);
            }
            app.request_fs_initial_placement(0);
            assert_eq!(
                app.fs_initial_placement_request.unwrap().origin_automatic,
                automatic
            );
            let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
            frame(&ctx, viewport(), 1.0, |ui| {
                single(
                    &mut app,
                    ui,
                    &payload,
                    true,
                    false,
                    Rotation::None,
                    SingletonSpreadPlacement::Center,
                );
                if x == 25.0 {
                    assert_eq!(app.fs_pan, egui::vec2(25.0, 250.0));
                } else {
                    assert!(app.fs_pan.x < 600.0);
                }
                assert_painted_pan(&app);
                assert!(app.fullscreen_page_layout.drawn_unit.is_some());
            });
            assert_eq!(app.fs_view_has_moved(1.0, app.fs_pan, 0.0), !automatic);
        }
    }
}

#[test]
fn tiny_real_pages_and_white_companion_accept_input_candidates() {
    let vp = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    let page = |idx, rect, side: Option<SingletonSpreadPlacement>| {
        DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: idx,
                viewport_rect: vp,
                source_size: egui::vec2(20.0, 40.0),
                texture_size: egui::vec2(20.0, 40.0),
                rotation: Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::Legacy(FullscreenFitMode::Original),
                fit_scale_limits: Default::default(),
                pixels_per_point: 1.0,
                placement: side.map_or(
                    ResolvedDisplayPlacement::Normal { zoom_pan: None },
                    |side| ResolvedDisplayPlacement::SingletonSpread {
                        side,
                        gap: 12.0,
                        zoom_pan: None,
                    },
                ),
                pixel_fit: RectPixelFit::Proportional,
            },
            rect,
        )
        .unwrap()
    };
    let left = egui::Rect::from_min_size(egui::pos2(-32.0, 100.0), egui::vec2(20.0, 40.0));
    let right = left.translate(egui::vec2(32.0, 0.0));
    assert_eq!(
        draw_pan_correction(&[page(0, left, None), page(1, right, None)]),
        egui::Vec2::ZERO
    );
    assert_eq!(
        draw_pan_correction(&[page(0, right, Some(SingletonSpreadPlacement::RightWhite))]),
        egui::Vec2::ZERO
    );
}

#[test]
fn white_singleton_input_clamp_is_preserved_by_complete_draw_at_small_zoom() {
    let ctx = egui::Context::default();
    for side in [
        SingletonSpreadPlacement::LeftWhite,
        SingletonSpreadPlacement::RightWhite,
    ] {
        for dpi in [1.0, 1.25, 1.5] {
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
            let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
            frame(&ctx, viewport(), dpi, |ui| {
                single(&mut app, ui, &payload, true, false, Rotation::None, side);
            });
            for zoom in [0.1, 0.2, 0.4, 1.0] {
                for delta in [egui::vec2(10000.0, 0.0), egui::vec2(-10000.0, 10000.0)] {
                    app.set_fs_pan_from_input(zoom, delta);
                    app.fs_zoom = zoom;
                    let pan = app.fs_pan;
                    frame(&ctx, viewport(), dpi, |ui| {
                        single(&mut app, ui, &payload, true, false, Rotation::None, side);
                        assert_eq!(app.fs_pan, pan, "{side:?}, dpi={dpi}, zoom={zoom}");
                        assert_painted_pan(&app);
                    });
                }
            }
        }
    }
}

#[test]
fn repeated_edge_drag_and_fractional_dpi_draws_are_stationary_after_first_correction() {
    let ctx = egui::Context::default();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for dpi in [1.0, 1.25, 1.5] {
            let mut app = app(&ctx, mode);
            let payload = resource(&app, 0, FullscreenPageMaterialKind::Complete);
            let paint = |app: &mut App, ui: &mut egui::Ui| {
                if mode == SpreadMode::Single {
                    single(
                        app,
                        ui,
                        &payload,
                        true,
                        false,
                        Rotation::None,
                        SingletonSpreadPlacement::Center,
                    );
                } else {
                    spread(app, ui, true, false);
                }
            };
            app.fs_pan = egui::vec2(2400.6, -2000.4);
            frame(&ctx, viewport(), dpi, |ui| paint(&mut app, ui));
            let bounded = app.fs_pan;
            for _ in 0..4 {
                frame(&ctx, viewport(), dpi, |ui| paint(&mut app, ui));
                assert_eq!(app.fs_pan, bounded);
            }
            for _ in 0..4 {
                app.set_fs_pan_from_input(1.0, egui::vec2(2400.6, -2000.4));
                let input_pan = app.fs_pan;
                frame(&ctx, viewport(), dpi, |ui| paint(&mut app, ui));
                assert_eq!(
                    app.fs_pan, input_pan,
                    "same-scale input must remain valid: {mode:?} {dpi}"
                );
            }
        }
    }
}

#[test]
fn z_continuous_comparison_and_editing_do_not_bound_the_shared_pan() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    assert!(app.fs_draw_pan_eligible(true, false));
    app.reading_flow = ReadingFlow::Vertical;
    assert!(!app.fs_draw_pan_eligible(true, false));
    app.reading_flow = ReadingFlow::Paged;
    app.fs_zoom_active = true;
    assert!(!app.fs_draw_pan_eligible(true, false));
    app.fs_zoom_active = false;
    app.view_trim_mode = true;
    assert!(!app.fs_draw_pan_eligible(true, false));
    app.view_trim_mode = false;
    app.analysis_mode = true;
    assert!(!app.fs_draw_pan_eligible(true, false));
    app.analysis_mode = false;
    app.compare_view_mode = crate::app::CompareViewMode::wipe(0.5);
    assert!(!app.fs_draw_pan_eligible(true, false));
}

#[test]
fn rejected_native_discards_pan_proposal_before_provisional_fallback() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.fs_pan = egui::vec2(2300.0, -1800.0);
    let pan = app.fs_pan;
    let source = match resource(&app, 0, FullscreenPageMaterialKind::Complete) {
        FullscreenPaintPayload::NormalResource(source) => source,
        _ => unreachable!(),
    };
    let native = crate::gpu_lanczos::test_native_resource(source.clone(), [20, 30]);
    let native = FullscreenPaintPayload::PresentationMaterial(native.capture_presentation(&ctx));
    let fallback = source.with_page_material_kind(FullscreenPageMaterialKind::Provisional);
    frame(&ctx, viewport(), 1.0, |ui| {
        let t = app
            .draw_fs_image_with_initial_placement(
                ui,
                viewport(),
                0,
                Some(egui::vec2(80.0, 120.0)),
                FsPageLayoutSource::CurrentItem,
                None,
                None,
                SingletonSpreadPlacement::Center,
                Some(&native),
                Some(&fallback),
                false,
                false,
                false,
                Rotation::None,
                Some((1.0, pan)),
                0.0,
                &FsBgStyle::Solid(egui::Color32::BLACK),
                "",
                false,
                FullscreenFitMode::Original,
                Default::default(),
                None,
                None,
                false,
                None,
                page_turn::FlatCropPaint::WithPage,
                true,
                false,
            )
            .unwrap();
        assert_eq!(t.texture_size, fallback.full_source_size_vec2());
        assert_eq!(normal_zoom_pan(t).unwrap().1, pan);
        assert_eq!(app.fs_pan, pan);
    });
    frame(&ctx, viewport(), 1.0, |ui| {
        let complete = resource(&app, 0, FullscreenPageMaterialKind::Complete);
        single(
            &mut app,
            ui,
            &complete,
            true,
            false,
            Rotation::None,
            SingletonSpreadPlacement::Center,
        );
        assert_painted_pan(&app);
    });
    assert_ne!(app.fs_pan, pan);
}

#[cfg(windows)]
#[test]
fn draw_pan_correction_only_changes_the_mounted_context() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_pan = egui::vec2(12.25, 18.75);
    let main_pan = app.fs_pan;
    let complete = resource(&app, 0, FullscreenPageMaterialKind::Complete);
    let child = app.build_window_context_for_test(806810, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/test/draw-pan.png"))];
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = SpreadMode::Single;
        owner.reading_flow = ReadingFlow::Paged;
        owner.fs_pan = egui::vec2(2400.0, -1900.0);
    });
    app.with_window_context_for_test(child, |owner| {
        frame(&ctx, viewport(), 1.0, |ui| {
            single(
                owner,
                ui,
                &complete,
                true,
                false,
                Rotation::None,
                SingletonSpreadPlacement::Center,
            );
            assert_painted_pan(owner);
        });
    });
    assert_eq!(app.fs_pan, main_pan);
    app.with_window_context_for_test(child, |owner| {
        assert!(owner.fs_pan.x < 1000.0);
    });
}

#[test]
fn center_native_fallback_keeps_the_original_slot_gap_offset_and_pan() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    let source = match resource(&app, 0, FullscreenPageMaterialKind::Complete) {
        FullscreenPaintPayload::NormalResource(source) => source,
        _ => unreachable!(),
    };
    let native = crate::gpu_lanczos::test_native_resource(source, [20, 30]);
    let native = FullscreenPaintPayload::PresentationMaterial(native.capture_presentation(&ctx));
    let thumbnail = ctx.load_texture(
        "center-fallback-thumbnail",
        egui::ColorImage::filled([8, 12], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.thumbnails[0] = ThumbnailState::loaded_raster(
        thumbnail,
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 12,
        },
        false,
        12,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((80, 120)),
        None,
    );
    for pan in [egui::Vec2::ZERO, egui::vec2(2300.0, -1800.0)] {
        app.fs_pan = pan;
        // These are the original complete candidate's slot and joint-gap
        // offset. The thumbnail's own 8x12 layout must not replace this slot.
        let slot =
            egui::Rect::from_min_size(egui::pos2(214.0, 140.0) + pan, egui::vec2(80.0, 120.0));
        let offset = egui::vec2(-3.0, 0.0);
        frame(&ctx, viewport(), 1.0, |ui| {
            let page = app
                .prepare_fs_spread_page(
                    ui.painter(),
                    viewport(),
                    slot,
                    0,
                    Some(egui::vec2(80.0, 120.0)),
                    FsPageLayoutSource::CurrentItem,
                    Rotation::None,
                    &FsBgStyle::Solid(egui::Color32::BLACK),
                    "",
                    Some(&native),
                    true,
                    None,
                    1.0,
                    ResolvedDisplayPlacement::Normal {
                        zoom_pan: Some((1.0, pan)),
                    },
                    offset,
                    false,
                )
                .unwrap();
            assert_eq!(
                page.resource.page_material_kind(),
                FullscreenPageMaterialKind::Provisional
            );
            assert_eq!(page.transform.texture_size, egui::vec2(8.0, 12.0));
            assert_eq!(page.transform.paint_rect, slot.translate(offset));
            assert_eq!(normal_zoom_pan(page.transform).unwrap().1, pan);
            assert_eq!(app.fs_pan, pan);
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}
