use super::*;
use crate::page_dims::{CanonicalPageDimensions, PagePixelDimensions};
use crate::source_coverage::{EditPreviewGeometry, SourceCoverage};

fn fixture(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![GridItem::Image(app.tmp.path().join("coverage.png"))];
    app.fullscreen_idx = Some(0);
    app.visible_indices = vec![0];
    app.details_order = vec![0];
    let tex = ctx.load_texture(
        "crop-material",
        egui::ColorImage::filled([100, 100], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    let coverage = SourceCoverage::Known(
        EditPreviewGeometry::new(
            CanonicalPageDimensions::new(800, 600),
            (800, 600),
            (200, 100, 400, 400),
        )
        .unwrap(),
    );
    app.thumbnails = vec![ThumbnailState::Loaded {
        tex,
        origin: crate::thumb_loader::ThumbLoadOrigin::EditPreviewCache { epoch: 0 },
        from_edit_preview: true,
        source_coverage: coverage,
        rendered_at_px: 100,
        source_dims: Some((400, 400)),
        layout_dims: None,
    }];
    let generation = app.items_generation;
    app.page_dims_cache.record(
        generation,
        0,
        PagePixelDimensions::Canonical(CanonicalPageDimensions::new(800, 600)),
    );
    app
}

fn draw(
    app: &mut App,
    ctx: &egui::Context,
    resource: Option<&FullscreenPaintResource>,
    rotation: crate::rotation_db::Rotation,
    free: f32,
    trim: Option<egui::Rect>,
) -> DisplayedImageTransform {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
    let thumb = app.fs_thumbnail_texture_for_display(0);
    let mut result = None;
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root_ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root_ui, |ui| {
                            result = app.draw_fs_image(
                                ui,
                                rect,
                                0,
                                app.fs_page_coordinate_source_size(0),
                                FsPageLayoutSource::CurrentItem,
                                None,
                                None,
                                SingletonSpreadPlacement::Center,
                                resource,
                                thumb.as_ref(),
                                false,
                                false,
                                false,
                                rotation,
                                None,
                                free,
                                &FsBgStyle::Solid(egui::Color32::BLACK),
                                "",
                                false,
                                app.effective_fullscreen_fit_mode(),
                                app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
                                trim,
                            );
                        })
                });
            },
        )
        .drop_without_applying_deltas();
    result.unwrap()
}

fn close_rect(a: egui::Rect, b: egui::Rect) {
    assert!(
        (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01,
        "{a:?} != {b:?}"
    );
}

#[test]
fn crop_coverage_single_arrival_and_eviction_keep_whole_frame_and_source_mapping() {
    use crate::rotation_db::Rotation;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    for mode in [
        FullscreenFitMode::Page,
        FullscreenFitMode::Width,
        FullscreenFitMode::Height,
        FullscreenFitMode::Original,
        FullscreenFitMode::SpecifiedScale,
    ] {
        app.settings.fullscreen_fit_mode = mode;
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            for free in [0.0, 0.173] {
                for trim in [
                    None,
                    Some(egui::Rect::from_min_max(
                        egui::pos2(0.1, 0.05),
                        egui::pos2(0.95, 0.9),
                    )),
                ] {
                    app.fs_cache.clear();
                    app.fs_early_dims.clear();
                    let before = draw(&mut app, &ctx, None, rotation, free, trim);
                    app.fs_early_dims.insert(0, [800, 600]);
                    let dims = draw(&mut app, &ctx, None, rotation, free, trim);
                    close_rect(before.full_image_rect, dims.full_image_rect);
                    let pixels =
                        Arc::new(egui::ColorImage::filled([800, 600], egui::Color32::WHITE));
                    let raw = ctx.load_texture(
                        "coverage-raw",
                        (*pixels).clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    app.fs_cache.insert(
                        0,
                        FsCacheEntry::Static {
                            tex: raw.clone(),
                            pixels,
                            source_dims: Some([800, 600]),
                            load_seq: 0,
                            animation: crate::fs_animation::StaticAnimationState::Still,
                        },
                    );
                    let raw_resource = app.fullscreen_paint_resource_for_texture(0, raw);
                    let after = draw(&mut app, &ctx, Some(&raw_resource), rotation, free, trim);
                    close_rect(before.full_image_rect, after.full_image_rect);
                    let final_size = if mode == FullscreenFitMode::Original {
                        [800, 600]
                    } else {
                        [1600, 1200]
                    };
                    let ai = ctx.load_texture(
                        "coverage-ai",
                        egui::ColorImage::filled(final_size, egui::Color32::WHITE),
                        egui::TextureOptions::LINEAR,
                    );
                    let ai_resource = app.fullscreen_paint_resource_for_texture(0, ai);
                    let final_transform =
                        draw(&mut app, &ctx, Some(&ai_resource), rotation, free, trim);
                    close_rect(before.full_image_rect, final_transform.full_image_rect);
                    let source = egui::pos2(0.37, 0.48);
                    assert!(
                        (before.source_normalized_to_screen(source)
                            - final_transform.source_normalized_to_screen(source))
                        .length()
                            < 0.01
                    );
                    app.fs_cache.clear();
                    app.fs_early_dims.clear();
                    let evicted = draw(&mut app, &ctx, None, rotation, free, trim);
                    close_rect(before.full_image_rect, evicted.full_image_rect);
                    let held = app.fs_thumbnail_resource_for_display(0).unwrap();
                    let thumbnail = app.thumbnails[0].clone();
                    app.thumbnails[0] = ThumbnailState::Evicted;
                    let held_draw = draw(&mut app, &ctx, Some(&held), rotation, free, trim);
                    close_rect(before.full_image_rect, held_draw.full_image_rect);
                    app.thumbnails[0] = thumbnail;
                }
            }
        }
    }
    app.thumbnails[0] = ThumbnailState::Evicted;
    assert_eq!(app.known_page_dims_for_idx(0), Some((800, 600)));
    assert_eq!(
        app.vertical_reading_base_size(0, egui::vec2(1.0, 1.0), false)
            .x
            / app
                .vertical_reading_base_size(0, egui::vec2(1.0, 1.0), false)
                .y,
        800.0 / 600.0
    );
}

#[test]
fn crop_coverage_navigator_and_snapshot_retain_mapping_after_thumbnail_replacement() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let material = app.fs_thumbnail_resource_for_display(0).unwrap();
    let captured = material.capture_presentation(&ctx);
    let mut sources = FsNavigatorTextureSources::single(0, Some(material));
    let expected = sources.pages[0].resource.source_coverage();
    let input = sources
        .auxiliary_resource_for_page(0)
        .unwrap()
        .source_texture()
        .unwrap()
        .clone();
    let native = crate::gpu_lanczos::test_native_resource_with_region(
        FullscreenPaintResource::resampleable(
            0,
            input,
            crate::gpu_lanczos::FullscreenPaintSourceGeneration {
                items: app.items_generation,
                input: 0,
            },
        )
        .with_source_coverage(expected),
        [50, 50],
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(0.5, 0.5)),
    );
    let ordinary_native_sources = FsNavigatorTextureSources::single(0, Some(native));
    let auxiliary = ordinary_native_sources
        .auxiliary_resource_for_page(0)
        .unwrap();
    assert_eq!(auxiliary.source_coverage(), expected);
    assert_eq!(
        auxiliary.paint_texture_id(),
        sources.pages[0].resource.source_texture_id()
    );
    app.thumbnails[0] = ThumbnailState::Evicted;
    assert_eq!(captured.source_coverage(), expected);
    assert_eq!(sources.pages[0].resource.source_coverage(), expected);
    sources.pages[0].resource = FullscreenPaintPayload::PresentationMaterial(captured);
    let geometry = DisplayedImageGeometry::from_resolved_rect(
        DisplayedImageGeometryInput {
            pixel_fit: RectPixelFit::Proportional,
            viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
            source_size: egui::vec2(800.0, 600.0),
            texture_size: egui::vec2(200.0, 150.0),
            rotation: crate::rotation_db::Rotation::None,
            free_rotation_rad: 0.0,
            content_bbox: None,
            fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
            fit_scale_limits: Default::default(),
            pixels_per_point: 1.25,
            placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
        },
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
    )
    .unwrap();
    let _ = ctx
        .run_pass(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            assert!(sources.paint_page_resource(&painter, 0, &geometry, None));
        })
        .drop_without_applying_deltas();
    let quad = geometry
        .material_quad(expected.source_uv_rect().unwrap(), None)
        .unwrap();
    close_rect(
        egui::Rect::from_two_pos(quad.positions[0], quad.positions[2]),
        egui::Rect::from_min_size(egui::pos2(200.0, 100.0), egui::vec2(400.0, 400.0)),
    );
}

#[test]
fn crop_coverage_legacy_without_current_crop_keeps_coordinate_and_specified_basis() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.page_dims_cache.clear();
    if let ThumbnailState::Loaded {
        source_coverage, ..
    } = &mut app.thumbnails[0]
    {
        *source_coverage = SourceCoverage::Unknown;
    }
    assert_eq!(
        app.fs_page_coordinate_source_size(0),
        Some(egui::vec2(400.0, 400.0))
    );
    assert!(app.continuous_specified_fit_basis_for_idx(0).is_some());
    app.export_crop_pages.insert(0);
    assert!(app.fs_page_coordinate_source_size(0).is_none());
    assert!(app.fs_thumbnail_resource_for_display(0).is_err());
}

#[cfg(windows)]
#[test]
fn crop_coverage_snapshot_resource_geometry_survives_owner_eviction() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let resource = app.fs_thumbnail_resource_for_display(0).unwrap();
    let placement = crate::detached_window_geometry::PhysicalWindowPlacement {
        outer: crate::detached_window_geometry::PhysicalRect {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 900,
        },
        maximized: false,
    };
    for mode in [
        FullscreenFitMode::Page,
        FullscreenFitMode::Original,
        FullscreenFitMode::SpecifiedScale,
    ] {
        app.settings.fullscreen_fit_mode = mode;
        let before = app
            .detached_single_image_snapshot_resource_layout(
                0,
                placement,
                1.25,
                &resource,
                crate::rotation_db::Rotation::None,
                None,
                0.0,
            )
            .unwrap();
        app.thumbnails[0] = ThumbnailState::Evicted;
        let after = app
            .detached_single_image_snapshot_resource_layout(
                0,
                placement,
                1.25,
                &resource,
                crate::rotation_db::Rotation::None,
                None,
                0.0,
            )
            .unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn crop_coverage_spread_both_arrival_orders_keep_page_frames_and_gap() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let right_path = app.tmp.path().join("right.png");
    app.items.push(GridItem::Image(right_path));
    app.thumbnails.clear();
    let coverage = SourceCoverage::Known(
        EditPreviewGeometry::new(
            CanonicalPageDimensions::new(600, 800),
            (600, 800),
            (100, 200, 400, 400),
        )
        .unwrap(),
    );
    let mut originals = Vec::new();
    for idx in 0..2 {
        let tex = ctx.load_texture(
            format!("pair-thumb-{idx}"),
            egui::ColorImage::filled([100, 100], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        app.thumbnails.push(ThumbnailState::Loaded {
            tex,
            origin: crate::thumb_loader::ThumbLoadOrigin::EditPreviewCache { epoch: 0 },
            from_edit_preview: true,
            source_coverage: coverage,
            rendered_at_px: 100,
            source_dims: Some((400, 400)),
            layout_dims: None,
        });
        let generation = app.items_generation;
        app.page_dims_cache.record(
            generation,
            idx,
            PagePixelDimensions::Canonical(CanonicalPageDimensions::new(600, 800)),
        );
        let pixels = Arc::new(egui::ColorImage::filled([600, 800], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("pair-original-{idx}"),
            (*pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        originals.push((tex, pixels));
    }
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
    let frames = [
        egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(400.0, 600.0)),
        egui::Rect::from_min_size(egui::pos2(453.0, 50.0), egui::vec2(400.0, 600.0)),
    ];
    let resolve = |app: &mut App| -> Vec<DisplayedImageTransform> {
        (0..2)
            .map(|idx| {
                let resource = match app.fs_cache.get(&idx) {
                    Some(FsCacheEntry::Static { tex, .. }) => {
                        app.fullscreen_paint_resource_for_texture(idx, tex.clone())
                    }
                    _ => app.fs_thumbnail_resource_for_display(idx).unwrap(),
                };
                app.resolve_fs_spread_page_transform(
                    &resource,
                    viewport,
                    frames[idx],
                    idx,
                    app.fs_page_coordinate_source_size(idx),
                    FsPageLayoutSource::CurrentItem,
                    crate::rotation_db::Rotation::None,
                    None,
                    1.25,
                    ResolvedDisplayPlacement::Normal { zoom_pan: None },
                )
                .unwrap()
            })
            .collect()
    };
    let baseline = resolve(&mut app);
    for order in [[0, 1], [1, 0]] {
        app.fs_cache.clear();
        for idx in order {
            let (tex, pixels) = &originals[idx];
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    tex: tex.clone(),
                    pixels: pixels.clone(),
                    source_dims: Some([600, 800]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            let current = resolve(&mut app);
            for page in 0..2 {
                close_rect(
                    baseline[page].full_image_rect,
                    current[page].full_image_rect,
                );
                assert_eq!(app.known_page_dims_for_idx(page), Some((600, 800)));
            }
            assert_eq!(
                align_spread_pages_for_gap(&baseline[0], &baseline[1], 3.0, 1.25),
                align_spread_pages_for_gap(&current[0], &current[1], 3.0, 1.25)
            );
        }
    }
}
