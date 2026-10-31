//! Released ordinary-image 100% means one selected texture pixel per physical display pixel.
//! Canonical annotation dimensions remain independent of that display basis after GPU clamping
//! or AI enlargement. These are headless draw/selector tests, not pixel-decoding tests.

use super::*;
use crate::fullscreen_specified_scale::LegacySourceLayoutBasis;
use crate::page_dims::CanonicalPageDimensions;
use crate::source_coverage::{EditPreviewGeometry, SourceCoverage};

fn image_metadata(size: [usize; 2]) -> Arc<egui::ColorImage> {
    Arc::new(egui::ColorImage {
        size,
        source_size: egui::vec2(size[0] as f32, size[1] as f32),
        pixels: Vec::new(),
    })
}

fn metadata_texture(
    ctx: &egui::Context,
    name: String,
    image: Arc<egui::ColorImage>,
) -> egui::TextureHandle {
    // Same metadata lease technique as page_turn/material_budget_tests.rs. No 8192x6144
    // CPU raster or GPU upload is needed to inspect selection, shapes and geometry.
    let manager = ctx.tex_manager();
    let id = manager.write().alloc(
        name,
        egui::ImageData::Color(image),
        egui::TextureOptions::LINEAR,
    );
    egui::TextureHandle::new(manager, id)
}

fn fixture() -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..2)
        .map(|idx| GridItem::Image(app.tmp.path().join(format!("display-basis-{idx}.png"))))
        .collect();
    app.fullscreen_idx = Some(0);
    app.visible_indices = vec![0, 1];
    app.details_order = vec![0, 1];
    app.reading_flow = ReadingFlow::Paged;
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.settings.spread_page_gap_px = 0;
    app
}

/// Exercise the worker-result acceptance boundary rather than pre-populating
/// Loaded and manually supplying the canonical dimensions that acceptance owns.
fn accept_thumbnails(
    app: &mut App,
    ctx: &egui::Context,
    coverage: SourceCoverage,
    source_dims: (u32, u32),
    sampler: [usize; 2],
) {
    accept_thumbnails_with_layout(app, ctx, coverage, source_dims, sampler, None);
}

fn accept_thumbnails_with_layout(
    app: &mut App,
    ctx: &egui::Context,
    coverage: SourceCoverage,
    source_dims: (u32, u32),
    sampler: [usize; 2],
    layout_dims: Option<(u32, u32)>,
) {
    accept_thumbnail_receipts(app, ctx, coverage, Some(source_dims), sampler, layout_dims);
}

fn accept_thumbnail_receipts(
    app: &mut App,
    ctx: &egui::Context,
    coverage: SourceCoverage,
    source_dims: Option<(u32, u32)>,
    sampler: [usize; 2],
    layout_dims: Option<(u32, u32)>,
) {
    app.image_metas.resize(app.items.len(), None);
    app.keep_range = (0, app.items.len());
    for idx in 0..2 {
        app.keep_set.insert(idx);
        app.requested.insert(idx, false);
        let message = crate::thumb_loader::ThumbMsg::from_legacy_parts(
            idx,
            Some(egui::ColorImage::filled(sampler, egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            !matches!(coverage, SourceCoverage::FullSource),
            None,
            source_dims,
            layout_dims,
            false,
            false,
            0,
            app.items_generation,
        )
        .with_source_coverage(coverage)
        .source_pixels();
        app.poll_thumbnail_result_for_test(ctx, message);
        // SourcePixels and cache/admission completion are separate worker
        // messages. Complete the real receipt protocol before retaining paint.
        let finalized = crate::thumb_loader::ThumbMsg::from_legacy_parts(
            idx,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            false,
            None,
            None,
            None,
            false,
            true,
            0,
            app.items_generation,
        );
        app.poll_thumbnail_result_for_test(ctx, finalized);
        assert!(matches!(app.thumbnails[idx], ThumbnailState::Loaded { .. }));
        assert!(!app.requested.contains_key(&idx));
    }
}

fn accept_legacy_catalog_thumbnails_without_source_dimensions(app: &mut App, ctx: &egui::Context) {
    app.image_metas.resize(app.items.len(), None);
    app.keep_range = (0, app.items.len());
    for idx in 0..2 {
        app.keep_set.insert(idx);
        app.requested.insert(idx, false);
        let message = crate::thumb_loader::ThumbMsg::from_legacy_parts(
            idx,
            Some(egui::ColorImage::filled([60, 80], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            false,
            None,
            None,
            None,
            false,
            false,
            0,
            app.items_generation,
        );
        assert!(matches!(
            &message.payload,
            crate::thumb_loader::ThumbMsgPayload::Pixels(_)
        ));
        // A cache hit is already terminal. No source-save Finalized receipt follows.
        app.poll_thumbnail_result_for_test(ctx, message);
        assert!(matches!(app.thumbnails[idx], ThumbnailState::Loaded { .. }));
        assert!(!app.requested.contains_key(&idx));
    }
}

fn install_complete(
    app: &mut App,
    ctx: &egui::Context,
    idx: usize,
    canonical: [usize; 2],
    source: [usize; 2],
    output: [usize; 2],
    ai: bool,
) -> egui::TextureHandle {
    let source_pixels = image_metadata(source);
    let source_texture = metadata_texture(
        ctx,
        format!("basis-source-{idx}-{source:?}"),
        source_pixels.clone(),
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: source_texture.clone(),
            pixels: source_pixels.clone(),
            source_dims: Some(canonical),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.record_page_dims_for_spread(idx, (canonical[0] as u32, canonical[1] as u32));
    let edit_key = app.current_edit_result_key(idx);
    app.edit_result_cache.insert(
        edit_key,
        crate::app::EditResultEntry::raster(source_pixels.clone(), Some(source_texture.clone())),
    );
    let output_pixels = if output == source {
        source_pixels
    } else {
        image_metadata(output)
    };
    let output_texture = if output == source {
        source_texture
    } else {
        metadata_texture(
            ctx,
            format!("basis-final-{idx}-{output:?}"),
            output_pixels.clone(),
        )
    };
    let canonical_dims = Some(CanonicalPageDimensions::new(
        canonical[0] as u32,
        canonical[1] as u32,
    ));
    if ai {
        let key = app
            .final_ai_key_for_pixels(edit_key, source, app.effective_params(idx))
            .expect("explicit upscale preset has an AI key");
        app.final_ai_cache.insert(
            key,
            crate::app::FinalAiEntry {
                diagnostic_metadata: None,
                canonical_dims,
                source_size: source,
                pixels: output_pixels.clone(),
                used_upscale: true,
            },
        );
    }
    let key = app.final_composite_key_for_pixels(edit_key, source, app.effective_params(idx));
    app.final_composite_cache.insert(
        key,
        crate::app::FinalCompositeEntry::raster(
            canonical_dims,
            source,
            output_pixels,
            output_texture.clone(),
            egui::TextureOptions::LINEAR,
            true,
        ),
    );
    // ensure_final_composite_texture checks exact_complete_final_composite_texture before any
    // edit/AI CPU production. The valid complete key makes the empty-pixel metadata fixture safe.
    assert_eq!(
        app.resolve_fs_processed_material(ctx, idx, false)
            .unwrap()
            .source_texture_id(),
        output_texture.id()
    );
    assert!(app.final_effect_pending.is_empty());
    assert!(app.final_ai_pending.is_empty());
    output_texture
}

fn draw_single(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::Vec2,
) -> DisplayedImageTransform {
    app.spread_mode = SpreadMode::Single;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewport);
    let mut transform = None;
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root, |ui| {
                            let resource = app.resolve_fs_processed_material(ctx, 0, false);
                            let thumbnail = app.fs_thumbnail_resource_for_display(0).ok();
                            transform = app.draw_fs_image(
                                ui,
                                rect,
                                0,
                                app.fs_page_coordinate_source_size(0),
                                FsPageLayoutSource::CurrentItem,
                                None,
                                None,
                                SingletonSpreadPlacement::Center,
                                resource.as_ref(),
                                thumbnail.as_ref(),
                                false,
                                false,
                                false,
                                crate::rotation_db::Rotation::None,
                                None,
                                0.0,
                                &FsBgStyle::Solid(egui::Color32::WHITE),
                                "",
                                false,
                                app.effective_fullscreen_fit_mode(),
                                app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
                                None,
                            );
                        });
                });
            },
        )
        .drop_without_applying_deltas();
    transform.expect("ordinary single-page draw resolves geometry")
}

fn draw_spread(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::Vec2,
) -> [DisplayedImageTransform; 2] {
    draw_spread_with_decision(app, ctx, viewport, FsPageTurnDecision::normal())
}

fn draw_spread_with_decision(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::Vec2,
    decision: FsPageTurnDecision,
) -> [DisplayedImageTransform; 2] {
    app.spread_mode = SpreadMode::Ltr;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewport);
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root, |ui| {
                            app.draw_fs_spread(ui, ctx, rect, 0, 1, false, decision, None);
                        });
                });
            },
        )
        .drop_without_applying_deltas();
    std::array::from_fn(|idx| {
        app.fullscreen_page_layout
            .page_by_idx(idx)
            .expect("ordinary spread draws each page")
            .transform
    })
}

fn close_physical_size(transform: DisplayedImageTransform, size: [usize; 2], ppp: f32) {
    let actual = transform.full_image_rect.size() * ppp;
    assert!(
        (actual.x - size[0] as f32).abs() <= 1.01 && (actual.y - size[1] as f32).abs() <= 1.01,
        "7474e5cf1 selected-texture 100% contract: actual={actual:?}, expected={size:?}, transform={transform:?}"
    );
}

fn same_frame_and_common_pixel(
    before: DisplayedImageTransform,
    after: DisplayedImageTransform,
    ppp: f32,
) {
    assert!(
        (before.full_image_rect.min - after.full_image_rect.min).length() * ppp <= 1.01
            && (before.full_image_rect.max - after.full_image_rect.max).length() * ppp <= 1.01,
        "page frame changed: {before:?} -> {after:?}"
    );
    let common = egui::pos2(0.4, 0.4);
    assert!(
        (before.source_normalized_to_screen(common) - after.source_normalized_to_screen(common))
            .length()
            * ppp
            <= 1.01,
        "a pixel present in both materials moved"
    );
}

#[test]
fn display_basis_gpu_clamped_crop_arrival_preserves_single_and_spread_frames_and_common_pixels() {
    for ppp in [1.0, 1.25] {
        for mode in [FullscreenFitMode::Original, FullscreenFitMode::Page] {
            for order in [[0, 1], [1, 0]] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(ppp);
                let mut app = fixture();
                app.settings.fullscreen_fit_mode = mode;
                let coverage = SourceCoverage::Known(
                    EditPreviewGeometry::new(
                        CanonicalPageDimensions::new(20000, 15000),
                        (8192, 6144),
                        (2048, 1536, 4096, 3072),
                    )
                    .unwrap(),
                );
                accept_thumbnails(&mut app, &ctx, coverage, (4096, 3072), [512, 384]);
                let viewport = egui::vec2(20000.0, 15000.0) / ppp;
                let before_single = draw_single(&mut app, &ctx, viewport);
                let before_spread = draw_spread(&mut app, &ctx, viewport);
                let pass_through = draw_spread_with_decision(
                    &mut app,
                    &ctx,
                    viewport,
                    FsPageTurnDecision {
                        paint_source: FsPageTurnPaintSource::PassThrough,
                        work_admission: FsPageTurnWorkAdmission::Deferred,
                        passthrough_rendition_ready: true,
                    },
                );
                for side in 0..2 {
                    same_frame_and_common_pixel(before_spread[side], pass_through[side], ppp);
                    let material = app
                        .cached_passthrough_material(side)
                        .expect("rendition prepared");
                    let input = app
                        .fs_fit_input_for_payload(side, mode, &material.into())
                        .unwrap();
                    if mode == FullscreenFitMode::Original {
                        assert_eq!(
                            input.original_display_dimensions().unwrap().as_tuple(),
                            (8192, 6144)
                        );
                    }
                }
                for idx in order {
                    let _ = install_complete(
                        &mut app,
                        &ctx,
                        idx,
                        [20000, 15000],
                        [8192, 6144],
                        [8192, 6144],
                        false,
                    );
                    let after_single = draw_single(&mut app, &ctx, viewport);
                    let after_spread = draw_spread(&mut app, &ctx, viewport);
                    same_frame_and_common_pixel(before_single, after_single, ppp);
                    for side in 0..2 {
                        same_frame_and_common_pixel(before_spread[side], after_spread[side], ppp);
                    }
                    if mode == FullscreenFitMode::Original {
                        close_physical_size(after_single, [8192, 6144], ppp);
                        for page in after_spread {
                            close_physical_size(page, [8192, 6144], ppp);
                        }
                    }
                }
            }
        }
    }
}

fn released_completed_material_basis(ai: bool) {
    let (canonical, source, final_size) = if ai {
        ([400, 600], [400, 600], [1600, 2400])
    } else {
        ([20000, 15000], [8192, 6144], [8192, 6144])
    };
    for ppp in [1.0, 1.25] {
        for (mode, no_upscale, no_downscale) in [
            (FullscreenFitMode::Original, false, false),
            (FullscreenFitMode::Page, true, false),
            (FullscreenFitMode::Page, false, true),
        ] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let mut app = fixture();
            app.settings.fullscreen_fit_mode = mode;
            app.settings.fullscreen_fit_no_upscale = no_upscale;
            app.settings.fullscreen_fit_no_downscale = no_downscale;
            let viewport = if no_upscale {
                egui::vec2(final_size[0] as f32 * 4.0, final_size[1] as f32 * 3.0) / ppp
            } else if no_downscale {
                egui::vec2(source[0] as f32 / 8.0, source[1] as f32 / 8.0) / ppp
            } else {
                egui::vec2(1200.0, 900.0) / ppp
            };
            for idx in 0..2 {
                let _ = install_complete(&mut app, &ctx, idx, canonical, source, source, false);
            }
            close_physical_size(draw_single(&mut app, &ctx, viewport), source, ppp);
            for page in draw_spread(&mut app, &ctx, viewport) {
                close_physical_size(page, source, ppp);
            }
            if ai {
                // AI completion retains the 400x600 source/annotation canvas while the selected
                // 1600x2400 texture defines ordinary 100%, as it did before this change.
                app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
                app.settings.global_preset.upscale_model = Some(
                    crate::ai::ModelKind::UpscaleRealEsrGeneralV3
                        .as_str()
                        .into(),
                );
                for idx in 0..2 {
                    let _ =
                        install_complete(&mut app, &ctx, idx, canonical, source, final_size, true);
                }
            }
            close_physical_size(draw_single(&mut app, &ctx, viewport), final_size, ppp);
            for page in draw_spread(&mut app, &ctx, viewport) {
                close_physical_size(page, final_size, ppp);
            }
        }
    }
}

#[test]
fn display_basis_uncropped_gpu_clamp_keeps_released_physical_original_and_limits_in_single_and_spread()
 {
    released_completed_material_basis(false);
}

#[test]
fn display_basis_uncropped_ai_keeps_released_completed_texture_original_and_limits_in_single_and_spread()
 {
    released_completed_material_basis(true);
}

fn specified_frame(
    ctx: &egui::Context,
    viewport: egui::Vec2,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| run(ui, ctx));
            });
        },
    ));
}

fn draw_specified_current(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    single_payload: Option<&FullscreenPaintPayload>,
) -> Vec<DisplayedImageTransform> {
    let anchor = app.fullscreen_idx.unwrap();
    let decision = app.fs_page_turn_decision_for_frame(ctx, anchor);
    let composition = app.spread_display_composition_for_anchor(anchor);
    let pages = composition.pages_in_screen_order(app.spread_mode);
    match pages.as_slice() {
        [page] => {
            let selected = app.resolve_fs_processed_material(ctx, page.idx, false);
            let resource = single_payload
                .cloned()
                .or_else(|| app.fullscreen_paint_payload_for_page(page.idx, selected, false));
            let thumbnail = app.fs_thumbnail_resource_for_display(page.idx).ok();
            let content_bbox = app.view_trim_single_content_bbox(page.idx);
            let mut sources = FsNavigatorTextureSources::default();
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Single);
            let transform = app
                .draw_fs_image_with_receipt(
                    ui,
                    ui.max_rect(),
                    page.idx,
                    app.fs_page_coordinate_source_size(page.idx),
                    FsPageLayoutSource::CurrentItem,
                    None,
                    None,
                    composition.singleton_placement(),
                    resource.as_ref(),
                    thumbnail.as_ref(),
                    false,
                    false,
                    false,
                    app.rotation_cache
                        .get(&page.idx)
                        .copied()
                        .unwrap_or(crate::rotation_db::Rotation::None),
                    None,
                    0.0,
                    &FsBgStyle::Solid(egui::Color32::WHITE),
                    "",
                    false,
                    app.effective_fullscreen_fit_mode(),
                    app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
                    content_bbox,
                    Some(&mut sources),
                    true,
                    None,
                    crate::ui_fullscreen::page_turn::FlatCropPaint::WithPage,
                )
                .expect("accepted partial page remains drawable");
            app.fullscreen_page_layout.push_occurrence(transform, *page);
        }
        [left, right] => {
            app.draw_fs_spread_with_submission(
                ui,
                ctx,
                ui.max_rect(),
                left.idx,
                right.idx,
                false,
                decision,
                None,
                StillSeekGeometry::bar_only(),
                true,
                Some(&composition),
            );
        }
        _ => panic!("test draws one ordinary Single or Pair"),
    }
    pages
        .iter()
        .map(|page| {
            app.fullscreen_page_layout
                .page_by_idx(page.idx)
                .expect("selected page submitted")
                .transform
        })
        .collect()
}

fn specified_partial_survives_thumbnail_eviction(evict_before_prepare: bool) {
    for ppp in [1.0, 1.25] {
        let modes = if evict_before_prepare {
            vec![SpreadMode::Single, SpreadMode::Ltr]
        } else {
            vec![SpreadMode::Single]
        };
        for mode in modes {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            page_turn::install(&ctx);
            let mut app = fixture();
            for idx in 2..4 {
                let path = app.tmp.path().join(format!("outgoing-{idx}.png"));
                app.items.push(GridItem::Image(path));
                app.visible_indices.push(idx);
                app.details_order.push(idx);
                app.thumbnails.push(ThumbnailState::Pending);
                let _ =
                    install_complete(&mut app, &ctx, idx, [80, 120], [80, 120], [80, 120], false);
            }
            app.spread_mode = mode;
            app.settings.page_turn_animation_enabled = evict_before_prepare;
            app.settings.page_turn_animation_duration_ms = 600;
            app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
            app.settings.fullscreen_fit_no_downscale = true;
            app.settings.fullscreen_specified_scale = crate::settings::FullscreenSpecifiedScale {
                width: crate::settings::SpecifiedScaleAxisSetting::disabled(
                    crate::settings::SpecifiedScaleAxis::PercentTenths(800),
                ),
                height: crate::settings::SpecifiedScaleAxisSetting::enabled(
                    crate::settings::SpecifiedScaleAxis::LogicalPixels(100),
                ),
            };
            let coverage = SourceCoverage::Known(
                EditPreviewGeometry::new(
                    CanonicalPageDimensions::new(20000, 15000),
                    (8192, 6144),
                    (2048, 1536, 4096, 3072),
                )
                .unwrap(),
            );
            accept_thumbnails(&mut app, &ctx, coverage, (4096, 3072), [512, 384]);
            for idx in 0..2 {
                if mode.is_spread() {
                    // The ordinary builder pairs portrait pages. Rotate this landscape
                    // source through its real saved-rotation route without changing A/O.
                    app.rotation_cache
                        .insert(idx, crate::rotation_db::Rotation::Cw90);
                }
            }
            let viewport = egui::vec2(20000.0, 10000.0) / ppp;
            // Apply the requested pixels-per-point and viewport before measuring.
            // egui otherwise uses its default first-pass viewport at fractional DPI.
            specified_frame(&ctx, viewport, |_, _| {});
            let physical_size = if mode.is_spread() {
                [6144, 8192]
            } else {
                [8192, 6144]
            };
            let mut baseline = Vec::new();
            specified_frame(&ctx, viewport, |ui, ctx| {
                baseline = draw_specified_current(&mut app, ui, ctx, None);
            });
            assert_eq!(baseline.len(), if mode.is_spread() { 2 } else { 1 });
            for transform in &baseline {
                close_physical_size(*transform, physical_size, ppp);
            }
            if !evict_before_prepare {
                // Capture through the production material owner, then submit that
                // actual Direct presentation payload through ordinary Single drawing.
                // Geometry preservation does not require a large curl to pass its
                // independent mesh-error/visibility admission rules.
                let resource = app.fs_thumbnail_resource_for_display(0).unwrap();
                let retained = FullscreenPaintPayload::PresentationMaterial(
                    resource.capture_presentation(&ctx),
                );
                app.thumbnails[..2].fill(ThumbnailState::Evicted);
                specified_frame(&ctx, viewport, |ui, ctx| {
                    let actual = draw_specified_current(&mut app, ui, ctx, Some(&retained));
                    for (before, after) in baseline.iter().zip(actual) {
                        same_frame_and_common_pixel(*before, after, ppp);
                        close_physical_size(after, physical_size, ppp);
                        let FullscreenFitInput::SpecifiedScale { basis, .. } = after.fit_input
                        else {
                            panic!("specified presentation descriptor")
                        };
                        assert_eq!(basis.original_size, egui::vec2(8192.0, 6144.0));
                    }
                });
                continue;
            }
            let captured_pair = mode.is_spread().then(|| {
                app.capture_fs_display_unit_with_rendition_for_pair(
                    None,
                    0,
                    SpreadPair::Double { left: 0, right: 1 },
                )
                .expect("capture the actual selected pair before thumbnail eviction")
            });
            app.fullscreen_idx = Some(2);
            specified_frame(&ctx, viewport, |ui, ctx| {
                draw_specified_current(&mut app, ui, ctx, None);
            });
            specified_frame(&ctx, viewport, |ui, ctx| {
                let operation = page_turn::ViewerOperationId::Pad {
                    batch: page_turn::next_operation_batch(),
                    ordinal: 0,
                };
                app.page_turn_external_operation(
                    ctx,
                    operation,
                    page_turn::OperationPhase::Press,
                    "specified-basis-eviction",
                );
                app.offer_page_turn_with_operation(ctx, 0, false, Some(operation));
                for idx in 0..baseline.len() {
                    assert!(matches!(
                        app.page_turn_held_resource(idx),
                        Some(FullscreenPaintPayload::NormalResource(_))
                    ));
                }
                if evict_before_prepare {
                    app.thumbnails[..2].fill(ThumbnailState::Evicted);
                }
                app.fullscreen_idx = Some(0);
                let actual = draw_specified_current(&mut app, ui, ctx, None);
                for (before, after) in baseline.iter().zip(actual) {
                    same_frame_and_common_pixel(*before, after, ppp);
                    let FullscreenFitInput::SpecifiedScale { basis, .. } = after.fit_input else {
                        panic!("specified fit descriptor")
                    };
                    assert_eq!(basis.original_size, egui::vec2(8192.0, 6144.0));
                }
            });
            if evict_before_prepare {
                // Offered NormalResource is sufficient to reproduce the live bug.
                // A large untrimmed unit may correctly fall back to a canonical flat
                // instead of starting a curl. Its captured pair must still replay.
                if let Some(captured_pair) = &captured_pair {
                    specified_frame(&ctx, viewport, |ui, ctx| {
                        app.draw_fs_display_unit_holdover(ui, ctx, ui.max_rect(), captured_pair);
                        for (idx, before) in baseline.iter().enumerate() {
                            let after = app
                                .fullscreen_page_layout
                                .page_by_idx(idx)
                                .expect("captured pair replay submits both pages")
                                .transform;
                            same_frame_and_common_pixel(*before, after, ppp);
                            close_physical_size(after, physical_size, ppp);
                            let FullscreenFitInput::SpecifiedScale { basis, .. } = after.fit_input
                            else {
                                panic!("captured specified descriptor")
                            };
                            assert_eq!(basis.original_size, egui::vec2(8192.0, 6144.0));
                        }
                    });
                }
                continue;
            }
        }
    }
}

#[test]
fn display_basis_specified_partial_normal_resource_survives_thumbnail_eviction() {
    specified_partial_survives_thumbnail_eviction(true);
}

#[test]
fn display_basis_specified_partial_presentation_material_survives_thumbnail_eviction() {
    specified_partial_survives_thumbnail_eviction(false);
}

#[test]
fn display_basis_worker_accepted_normal_material_keeps_fit_after_thumbnail_eviction() {
    for partial in [false, true] {
        for mode in [
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
            FullscreenFitMode::Page,
        ] {
            for spread in [SpreadMode::Single, SpreadMode::Ltr] {
                let ctx = egui::Context::default();
                page_turn::install(&ctx);
                let mut app = fixture();
                for idx in 2..4 {
                    let path = app
                        .tmp
                        .path()
                        .join(format!("worker-basis-outgoing-{idx}.png"));
                    app.items.push(GridItem::Image(path));
                    app.visible_indices.push(idx);
                    app.details_order.push(idx);
                    app.thumbnails.push(ThumbnailState::Pending);
                    let _ = install_complete(
                        &mut app,
                        &ctx,
                        idx,
                        [80, 120],
                        [80, 120],
                        [80, 120],
                        false,
                    );
                }
                app.spread_mode = spread;
                app.settings.page_turn_animation_enabled = true;
                app.settings.page_turn_animation_duration_ms = 600;
                configure_specified_original(&mut app);
                app.settings.fullscreen_fit_mode = mode;
                app.settings.fullscreen_fit_no_downscale =
                    mode == FullscreenFitMode::SpecifiedScale;
                let coverage = if partial {
                    SourceCoverage::Known(
                        EditPreviewGeometry::new(
                            CanonicalPageDimensions::new(15000, 20000),
                            (6144, 8192),
                            (1536, 2048, 3072, 4096),
                        )
                        .unwrap(),
                    )
                } else {
                    SourceCoverage::FullSource
                };
                accept_thumbnails(
                    &mut app,
                    &ctx,
                    coverage,
                    if partial {
                        (3072, 4096)
                    } else {
                        (15000, 20000)
                    },
                    [60, 80],
                );
                let viewport = egui::vec2(1200.0, 900.0);
                specified_frame(&ctx, viewport, |_, _| {});
                let mut baseline = Vec::new();
                specified_frame(&ctx, viewport, |ui, ctx| {
                    baseline = draw_specified_current(&mut app, ui, ctx, None);
                });
                assert_eq!(baseline.len(), if spread.is_spread() { 2 } else { 1 });
                app.fullscreen_idx = Some(2);
                specified_frame(&ctx, viewport, |ui, ctx| {
                    draw_specified_current(&mut app, ui, ctx, None);
                });
                specified_frame(&ctx, viewport, |ui, ctx| {
                    let operation = page_turn::ViewerOperationId::Pad {
                        batch: page_turn::next_operation_batch(),
                        ordinal: 0,
                    };
                    app.page_turn_external_operation(
                        ctx,
                        operation,
                        page_turn::OperationPhase::Press,
                        "worker-accepted-material-basis-eviction",
                    );
                    app.offer_page_turn_with_operation(ctx, 0, false, Some(operation));
                    for idx in 0..baseline.len() {
                        assert!(matches!(
                            app.page_turn_held_resource(idx),
                            Some(FullscreenPaintPayload::NormalResource(_))
                        ));
                    }
                    app.thumbnails[..2].fill(ThumbnailState::Evicted);
                    app.thumb_pixels.retain(|idx, _| *idx >= 2);
                    app.fullscreen_idx = Some(0);
                    let actual = draw_specified_current(&mut app, ui, ctx, None);
                    assert_eq!(actual.len(), baseline.len());
                    for (before, after) in baseline.iter().zip(actual) {
                        assert_eq!(
                            before.fit_input, after.fit_input,
                            "accepted material owns fit basis"
                        );
                        same_frame_and_common_pixel(*before, after, 1.0);
                        if mode == FullscreenFitMode::SpecifiedScale {
                            assert_specified_raster_basis(after.fit_input, [6144, 8192]);
                            close_physical_size(after, [6144, 8192], 1.0);
                        }
                    }
                });
            }
        }
    }
}

fn configure_specified_original(app: &mut App) {
    app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
    app.settings.fullscreen_fit_no_upscale = false;
    app.settings.fullscreen_fit_no_downscale = true;
    app.settings.fullscreen_specified_scale = crate::settings::FullscreenSpecifiedScale {
        width: crate::settings::SpecifiedScaleAxisSetting::disabled(
            crate::settings::SpecifiedScaleAxis::PercentTenths(800),
        ),
        height: crate::settings::SpecifiedScaleAxisSetting::enabled(
            crate::settings::SpecifiedScaleAxis::LogicalPixels(100),
        ),
    };
}

fn install_uncropped_thumbnails(
    app: &mut App,
    ctx: &egui::Context,
    canonical: [usize; 2],
    raster: [usize; 2],
    known_coverage: bool,
) {
    let coverage = if known_coverage {
        SourceCoverage::Known(
            EditPreviewGeometry::new(
                CanonicalPageDimensions::new(canonical[0] as u32, canonical[1] as u32),
                (raster[0] as u32, raster[1] as u32),
                (0, 0, raster[0] as u32, raster[1] as u32),
            )
            .unwrap(),
        )
    } else {
        SourceCoverage::FullSource
    };
    assert!(!coverage.is_partial());
    accept_thumbnails(
        app,
        ctx,
        coverage,
        (canonical[0] as u32, canonical[1] as u32),
        [60, 80],
    );
}

fn assert_specified_raster_basis(input: FullscreenFitInput, raster: [usize; 2]) {
    let FullscreenFitInput::SpecifiedScale { basis, .. } = input else {
        panic!("specified fit descriptor")
    };
    assert_eq!(
        basis.original_size,
        egui::vec2(raster[0] as f32, raster[1] as f32),
        "ordinary specified 100% follows the arriving pre-AI GPU raster"
    );
}

#[test]
fn display_basis_specified_uncropped_thumbnail_arrival_keeps_single_and_actual_pair_raster_original()
 {
    for (canonical, raster) in [([15000, 20000], [6144, 8192]), ([600, 800], [600, 800])] {
        for ppp in [1.0, 1.25] {
            for mode in [SpreadMode::Single, SpreadMode::Ltr] {
                for order in [[0, 1], [1, 0]] {
                    for known_coverage in [false, true] {
                        let ctx = egui::Context::default();
                        ctx.set_pixels_per_point(ppp);
                        page_turn::install(&ctx);
                        let mut app = fixture();
                        app.spread_mode = mode;
                        configure_specified_original(&mut app);
                        install_uncropped_thumbnails(
                            &mut app,
                            &ctx,
                            canonical,
                            raster,
                            known_coverage,
                        );
                        let viewport = egui::vec2(1200.0, 900.0) / ppp;
                        specified_frame(&ctx, viewport, |_, _| {});
                        let mut baseline = Vec::new();
                        specified_frame(&ctx, viewport, |ui, ctx| {
                            baseline = draw_specified_current(&mut app, ui, ctx, None);
                        });
                        assert_eq!(baseline.len(), if mode.is_spread() { 2 } else { 1 });
                        for transform in &baseline {
                            assert_specified_raster_basis(transform.fit_input, raster);
                            close_physical_size(*transform, raster, ppp);
                        }
                        for idx in order {
                            let _ = install_complete(
                                &mut app, &ctx, idx, canonical, raster, raster, false,
                            );
                            specified_frame(&ctx, viewport, |ui, ctx| {
                                let actual = draw_specified_current(&mut app, ui, ctx, None);
                                assert_eq!(actual.len(), baseline.len());
                                for (before, after) in baseline.iter().zip(actual) {
                                    assert_specified_raster_basis(after.fit_input, raster);
                                    close_physical_size(after, raster, ppp);
                                    same_frame_and_common_pixel(*before, after, ppp);
                                }
                            });
                        }
                        // The no-material fallback must agree with actual Single/Pair
                        // painting; rediscovering canonical source pixels is not 100%.
                        for idx in 0..2 {
                            assert_specified_raster_basis(
                                app.fs_fit_input_for_idx(idx, FullscreenFitMode::SpecifiedScale)
                                    .unwrap(),
                                raster,
                            );
                            assert_eq!(
                                app.fs_page_coordinate_source_size(idx),
                                Some(egui::vec2(canonical[0] as f32, canonical[1] as f32)),
                            );
                        }
                    }
                }
            }
        }
    }
}

fn continuous_specified_rects(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::Vec2,
    raster: [usize; 2],
) -> Vec<(usize, egui::Rect)> {
    for idx in 0..2 {
        assert_specified_raster_basis(
            app.fs_fit_input_for_idx(idx, FullscreenFitMode::SpecifiedScale)
                .unwrap(),
            raster,
        );
        assert_eq!(
            app.vertical_reading_base_size(idx, viewport, true),
            egui::vec2(raster[0] as f32, raster[1] as f32),
            "continuous base geometry shares the ordinary raster basis"
        );
    }
    let (units, current_pos) = app.continuous_reading_units_and_pos(0).unwrap();
    assert_eq!(
        units[current_pos].presentation.len(),
        if app.spread_mode.is_spread() { 2 } else { 1 },
    );
    let (pages, _, _, _) = app
        .continuous_reading_layout(
            ctx,
            egui::Rect::from_min_size(egui::Pos2::ZERO, viewport),
            &units,
            current_pos,
            true,
        )
        .unwrap();
    assert!(!pages.is_empty());
    pages
        .into_iter()
        .map(|page| {
            let physical = page.rect.size() * ctx.pixels_per_point();
            assert!(
                (physical.x - raster[0] as f32).abs() <= 1.01
                    && (physical.y - raster[1] as f32).abs() <= 1.01,
                "continuous layout must retain raster 100%: {physical:?}, expected={raster:?}",
            );
            (page.idx(), page.rect)
        })
        .collect()
}

#[test]
fn display_basis_worker_accepted_continuous_transition_keeps_material_after_thumbnail_eviction() {
    for partial in [false, true] {
        for mode in [
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
            FullscreenFitMode::Page,
        ] {
            for spread in [SpreadMode::Single, SpreadMode::Ltr] {
                let ctx = egui::Context::default();
                let mut app = fixture();
                app.reading_flow = ReadingFlow::Vertical;
                app.spread_mode = spread;
                configure_specified_original(&mut app);
                app.settings.fullscreen_fit_mode = mode;
                app.settings.fullscreen_fit_no_downscale =
                    mode == FullscreenFitMode::SpecifiedScale;
                // This is the existing continuous retained-material owner used
                // while the replacement final effect has not arrived yet.
                app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
                let coverage = if partial {
                    SourceCoverage::Known(
                        EditPreviewGeometry::new(
                            CanonicalPageDimensions::new(15000, 20000),
                            (6144, 8192),
                            (1536, 2048, 3072, 4096),
                        )
                        .unwrap(),
                    )
                } else {
                    SourceCoverage::FullSource
                };
                accept_thumbnails(
                    &mut app,
                    &ctx,
                    coverage,
                    if partial {
                        (3072, 4096)
                    } else {
                        (15000, 20000)
                    },
                    [60, 80],
                );
                for idx in 0..2 {
                    // Produce a real faithful rendition before the final effect
                    // finishes, then submit it through the same observer used by
                    // continuous processed selection. Colorize here is the small
                    // CPU rendition filter, with no model or external worker.
                    let resource = app.ensure_passthrough_material(&ctx, idx).unwrap();
                    app.fs_vertical_cache_keep_set.insert(idx);
                    app.observe_continuous_page_processed_texture(idx, &resource);
                    assert_eq!(
                        app.continuous_page_transition_texture(idx)
                            .unwrap()
                            .display_basis(),
                        resource.display_basis(),
                        "the production observer retains the accepted material",
                    );
                }
                let viewport = egui::vec2(1200.0, 900.0);
                specified_frame(&ctx, viewport, |_, _| {});
                let mut baseline = Vec::new();
                specified_frame(&ctx, viewport, |ui, ctx| {
                    let sources = app.draw_fs_continuous_reading(
                        ui,
                        ctx,
                        ui.max_rect(),
                        0,
                        false,
                        FsPageTurnDecision::normal(),
                    );
                    baseline = sources
                        .pages
                        .iter()
                        .map(|page| {
                            app.fullscreen_page_layout
                                .page_by_idx(page.idx())
                                .unwrap()
                                .transform
                        })
                        .collect::<Vec<_>>();
                });
                assert!(
                    !baseline.is_empty(),
                    "retained continuous materials are painted"
                );
                app.thumbnails[..2].fill(ThumbnailState::Evicted);
                app.thumb_pixels.clear();
                specified_frame(&ctx, viewport, |ui, ctx| {
                    let sources = app.draw_fs_continuous_reading(
                        ui,
                        ctx,
                        ui.max_rect(),
                        0,
                        false,
                        FsPageTurnDecision::normal(),
                    );
                    assert_eq!(sources.pages.len(), baseline.len());
                    for (before, source) in baseline.iter().zip(&sources.pages) {
                        assert!(
                            app.continuous_page_transition_texture(source.idx())
                                .is_some()
                        );
                        let after = app
                            .fullscreen_page_layout
                            .page_by_idx(source.idx())
                            .unwrap()
                            .transform;
                        assert_eq!(before.fit_input, after.fit_input);
                        same_frame_and_common_pixel(*before, after, 1.0);
                    }
                });
            }
        }
    }
}

#[test]
fn display_basis_specified_uncropped_thumbnail_arrival_keeps_continuous_base_and_layout_raster_original()
 {
    for (canonical, raster) in [([15000, 20000], [6144, 8192]), ([600, 800], [600, 800])] {
        for ppp in [1.0, 1.25] {
            for mode in [SpreadMode::Single, SpreadMode::Ltr] {
                for order in [[0, 1], [1, 0]] {
                    for known_coverage in [false, true] {
                        let ctx = egui::Context::default();
                        ctx.set_pixels_per_point(ppp);
                        let mut app = fixture();
                        app.spread_mode = mode;
                        app.reading_flow = ReadingFlow::Vertical;
                        configure_specified_original(&mut app);
                        install_uncropped_thumbnails(
                            &mut app,
                            &ctx,
                            canonical,
                            raster,
                            known_coverage,
                        );
                        let viewport = egui::vec2(1200.0, 900.0) / ppp;
                        specified_frame(&ctx, viewport, |_, _| {});
                        let baseline = continuous_specified_rects(&mut app, &ctx, viewport, raster);
                        for idx in order {
                            let _ = install_complete(
                                &mut app, &ctx, idx, canonical, raster, raster, false,
                            );
                            let actual =
                                continuous_specified_rects(&mut app, &ctx, viewport, raster);
                            assert_eq!(actual, baseline, "raster arrival moves no continuous page");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn display_basis_shared_thumbnail_and_passthrough_allocation_keeps_distinct_material_contracts() {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    let ctx = egui::Context::default();
    let mut app = fixture();
    accept_thumbnails(
        &mut app,
        &ctx,
        SourceCoverage::FullSource,
        (15000, 20000),
        [60, 80],
    );
    let thumbnail = app.fs_thumbnail_resource_for_display(0).unwrap();
    let rendition = app.ensure_passthrough_material(&ctx, 0).unwrap();
    assert_eq!(
        thumbnail.source_texture_id(),
        rendition.source_texture_id(),
        "identity processing shares its allocation with the catalog thumbnail",
    );
    assert_eq!(
        thumbnail.display_basis().legacy(V::Single),
        L::Raster {
            layout_size: egui::vec2(60.0, 80.0),
            original: crate::page_dims::DisplayOriginalDimensions::new(60, 80),
        },
    );
    assert_eq!(
        rendition.display_basis().legacy(V::Single),
        L::SourceLayout {
            basis: LegacySourceLayoutBasis {
                layout_size: egui::vec2(15000.0, 20000.0),
                reference_size: egui::vec2(15000.0, 20000.0),
            },
        },
        "the throughput material retains its existing whole-source legacy contract",
    );
    assert_eq!(
        thumbnail.display_basis().specified(),
        rendition.display_basis().specified(),
    );
    assert_eq!(
        rendition.display_basis().specified().unwrap().original_size,
        egui::vec2(6144.0, 8192.0),
    );
    let captured = rendition.capture_presentation(&ctx);
    app.thumbnails[..2].fill(ThumbnailState::Evicted);
    app.thumb_pixels.clear();
    assert_eq!(captured.display_basis(), rendition.display_basis());
    assert_ne!(captured.display_basis(), thumbnail.display_basis());
}

#[test]
fn display_basis_original_and_analysis_select_source_contract_even_when_final_shares_allocation() {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    let ctx = egui::Context::default();
    let mut app = fixture();
    // A processed canvas and a source can share an allocation yet own different
    // continuous extents. Selection must preserve the producer, not look up its ID.
    let texture = install_complete(&mut app, &ctx, 0, [400, 600], [4, 4], [4, 4], false);
    let processed = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
    assert_eq!(processed.source_texture_id(), texture.id());
    assert_eq!(
        processed.display_basis().legacy(V::Continuous),
        L::SourceLayout {
            basis: LegacySourceLayoutBasis {
                layout_size: egui::vec2(4.0, 4.0),
                reference_size: egui::vec2(400.0, 600.0),
            },
        },
    );
    let original = app.resolve_fs_processed_material(&ctx, 0, true).unwrap();
    app.analysis_mode = true;
    let analysis = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
    app.analysis_mode = false;
    for source in [original, analysis] {
        assert_eq!(source.source_texture_id(), processed.source_texture_id());
        assert_eq!(
            source.display_basis().legacy(V::Continuous),
            L::SourceLayout {
                basis: LegacySourceLayoutBasis {
                    layout_size: egui::vec2(400.0, 600.0),
                    reference_size: egui::vec2(400.0, 600.0),
                },
            },
        );
        assert_ne!(source.display_basis(), processed.display_basis());
    }
    assert_eq!(
        app.resolve_fs_processed_material(&ctx, 0, false)
            .unwrap()
            .display_basis(),
        processed.display_basis(),
        "leaving analysis restores the processed producer's descriptor",
    );
}

#[test]
fn display_basis_unknown_accepted_with_crop_becomes_drawable_when_crop_is_removed() {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    let ctx = egui::Context::default();
    let mut app = fixture();
    configure_specified_original(&mut app);
    app.export_crop_pages.extend([0, 1]);
    accept_thumbnails(
        &mut app,
        &ctx,
        SourceCoverage::Unknown,
        (600, 800),
        [60, 80],
    );
    for idx in 0..2 {
        assert!(app.fs_thumbnail_resource_for_display(idx).is_err());
    }
    // The coverage permission is dynamic. Receipt while crop was active must not
    // make the accepted sampler basis permanently unavailable.
    app.export_crop_pages.clear();
    for idx in 0..2 {
        let resource = app.fs_thumbnail_resource_for_display(idx).unwrap();
        assert_eq!(resource.source_coverage(), SourceCoverage::FullSource);
        assert_eq!(
            resource.display_basis().legacy(V::Single),
            L::Raster {
                layout_size: egui::vec2(60.0, 80.0),
                original: crate::page_dims::DisplayOriginalDimensions::new(60, 80),
            },
        );
        assert_eq!(
            resource.display_basis().specified().unwrap().original_size,
            egui::vec2(600.0, 800.0),
        );
    }
    let viewport = egui::vec2(1200.0, 900.0);
    specified_frame(&ctx, viewport, |_, _| {});
    for spread in [SpreadMode::Single, SpreadMode::Ltr] {
        app.spread_mode = spread;
        for mode in [
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
            FullscreenFitMode::Page,
        ] {
            app.settings.fullscreen_fit_mode = mode;
            specified_frame(&ctx, viewport, |ui, ctx| {
                let actual = draw_specified_current(&mut app, ui, ctx, None);
                assert_eq!(actual.len(), if spread.is_spread() { 2 } else { 1 });
            });
        }
    }
}

#[test]
fn display_basis_erase_base_and_worker_preview_promotion_keep_their_material_owner() {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    use crate::ui_erase::{
        EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey, EraseInpaintProgress,
        EraseInpaintPurpose, EraseInpaintResult,
    };
    let ctx = egui::Context::default();
    let mut app = fixture();
    crate::app::source_texture_test_support::seed(&mut app, &ctx, 0);
    let source = app.source_material_for_idx(0).unwrap();
    app.erase_mode = true;
    app.erase_preview_active = false;
    let base = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
    assert_ne!(
        base.source_texture_id(),
        source.source_texture_id(),
        "the independent erase-base upload is selected as its own material",
    );
    assert_eq!(base.display_basis(), source.display_basis());

    // Complete a real queued preview receipt without running MI-GAN. The result
    // handler, preview selector and commit promotion are production code.
    let (tx, rx) = std::sync::mpsc::channel();
    let (_, progress_rx) = std::sync::mpsc::channel();
    let pending = EraseInpaintPending {
        display_basis: base.display_basis(),
        idx: 0,
        items_generation: app.items_generation,
        path_key: app.page_path_key(0),
        rx,
        progress_rx,
        progress: EraseInpaintProgress::Preparing,
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        started_at: std::time::Instant::now(),
        input_generation: app.input_generation.get(&0).copied().unwrap_or(0),
        mask_generation: app.erase_mask_generation.get(&0).copied().unwrap_or(0),
        purpose: EraseInpaintPurpose::Preview {
            origin: "display-basis-preview",
        },
    };
    app.erase_inpaint_pending.insert(
        EraseInpaintPendingKey {
            idx: 0,
            kind: EraseInpaintKind::Preview,
        },
        pending,
    );
    tx.send(EraseInpaintResult::Pixels(egui::ColorImage::filled(
        [4, 6],
        egui::Color32::BLUE,
    )))
    .unwrap();
    app.poll_erase_inpaint(&ctx);
    app.erase_preview_active = true;
    let preview = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
    assert_eq!(preview.display_basis(), source.display_basis());
    assert_eq!(
        preview.display_basis().legacy(V::Continuous),
        L::SourceLayout {
            basis: LegacySourceLayoutBasis {
                layout_size: egui::vec2(400.0, 600.0),
                reference_size: egui::vec2(400.0, 600.0),
            },
        },
    );
    app.erase_mask_size = [4, 6];
    app.erase_mask = Some(vec![true; 24]);
    app.execute_erase_inpaint(&ctx, 0);
    assert!(!app.erase_mode);
    assert!(app.erase_inpaint_pending.is_empty());
    let committed = app.current_erase_result_material(0).unwrap();
    assert_eq!(committed.source_texture_id(), preview.source_texture_id());
    assert_eq!(committed.display_basis(), preview.display_basis());
}

#[test]
fn display_basis_live_comic_preview_keeps_base_specified_and_continuous_basis() {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    let ctx = egui::Context::default();
    let mut app = fixture();
    crate::app::source_texture_test_support::seed(&mut app, &ctx, 0);
    let base = app.ensure_final_composite_material(&ctx, 0).unwrap();
    crate::app::composite_pixels_revision_tests::seed_annotation(&mut app, [400, 600]);
    app.text_mode = true;
    app.settings.text_preview_scale = 2;
    let preview = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
    assert_eq!(preview.source_texture().size(), [2, 3]);
    assert_ne!(preview.source_texture_id(), base.source_texture_id());
    assert_eq!(
        preview.display_basis().legacy(V::Single),
        L::Raster {
            layout_size: egui::vec2(2.0, 3.0),
            original: crate::page_dims::DisplayOriginalDimensions::new(2, 3),
        },
    );
    assert_eq!(
        preview.display_basis().specified(),
        base.display_basis().specified(),
        "reduced annotation editing preview retains the accepted pre-AI basis",
    );
    assert_eq!(
        preview.display_basis().legacy(V::Continuous),
        base.display_basis().legacy(V::Continuous),
    );
    let held = preview.capture_presentation(&ctx);
    app.comic_cache.clear();
    app.final_composite_cache.clear();
    assert_eq!(held.display_basis(), preview.display_basis());
}

fn pdf_thumbnails_without_metadata(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = fixture();
    let pdf_path = app.tmp.path().join("accepted-basis.pdf");
    app.items = (0..2)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    accept_thumbnails_with_layout(
        &mut app,
        ctx,
        SourceCoverage::FullSource,
        (600, 800),
        [60, 80],
        Some((600_000, 800_000)),
    );
    app
}

fn retain_pdf_metadata_snapshot(app: &mut App, idx: usize) {
    let key = crate::app::RetainedPdfPageKey {
        item_key: app.metadata_cache_key(idx).unwrap(),
    };
    app.retained_pdf_page_cache.insert(
        key,
        crate::app::RetainedPdfPageEntry {
            canonical_dims: Some(CanonicalPageDimensions::new(600, 800)),
            metadata: Some(crate::app::PdfPageMetadataSnapshot {
                layout_dims: (600_000, 800_000),
                content_type: crate::pdf_loader::PdfPageContentType::Vector,
            }),
            kind: crate::app::RetainedPdfPageCacheKind::Raster {
                pixels: Arc::new(egui::ColorImage::filled([60, 80], egui::Color32::WHITE)),
                source_dims: [600, 800],
                render_long_edge: 800,
            },
            bytes: 60 * 80 * 4,
            last_used: 0,
        },
    );
}

#[test]
fn display_basis_pdf_thumbnail_without_content_type_keeps_legacy_page_then_adopts_metadata() {
    use crate::app::{FsLoadPurpose, FsUploadResult, PollPrefetchOrigin};
    use crate::fs_animation::FsLoadResult;
    use crate::material_display_basis::{
        MaterialBasisUnavailable as U, MaterialDisplayView as V, MaterialLegacyBasis as L,
    };
    let ctx = egui::Context::default();
    let mut app = pdf_thumbnails_without_metadata(&ctx);
    let initial = app.fs_thumbnail_resource_for_display(0).unwrap();
    let initial_basis = initial.display_basis();
    assert_eq!(initial_basis.specified(), Err(U::PdfMetadataUnavailable));
    for view in [V::Single, V::Spread, V::Continuous] {
        assert_eq!(
            initial_basis.legacy(view),
            L::SourceLayout {
                basis: LegacySourceLayoutBasis {
                    layout_size: egui::vec2(600_000.0, 800_000.0),
                    reference_size: egui::vec2(60.0, 80.0),
                },
            },
        );
    }
    let viewport = egui::vec2(1200.0, 900.0);
    specified_frame(&ctx, viewport, |_, _| {});
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    for spread in [SpreadMode::Single, SpreadMode::Ltr] {
        app.spread_mode = spread;
        specified_frame(&ctx, viewport, |ui, ctx| {
            let actual = draw_specified_current(&mut app, ui, ctx, None);
            assert_eq!(actual.len(), if spread.is_spread() { 2 } else { 1 });
            assert!(actual.iter().all(|page| page.paint_rect.is_positive()));
        });
    }

    // Adopt a real fullscreen PDF worker result, including content type and
    // page-box metadata. Loaded thumbnail re-acceptance belongs to this boundary.
    for idx in 0..2 {
        let generation = app.items_generation;
        app.fs_upload_backlog.push_for_generation(
            generation,
            FsUploadResult::new(
                idx,
                FsLoadResult::StaticPdf {
                    ci: egui::ColorImage::filled([6, 8], egui::Color32::WHITE),
                    source_dims: [600, 800],
                    page_size_points: crate::pdf_loader::PdfPageSizePoints {
                        width: 600.0,
                        height: 800.0,
                    },
                    content_type: crate::pdf_loader::PdfPageContentType::Vector,
                },
                1,
                FsLoadPurpose::for_page(true),
            ),
        );
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    }
    let accepted = app.fs_thumbnail_resource_for_display(0).unwrap();
    assert_eq!(accepted.source_texture_id(), initial.source_texture_id());
    let accepted_basis = accepted.display_basis();
    assert_eq!(
        accepted_basis.specified().unwrap(),
        FullscreenFitBasis::new(egui::vec2(600_000.0, 800_000.0), egui::vec2(3072.0, 4096.0),)
            .unwrap(),
    );
    assert_eq!(
        initial.display_basis(),
        initial_basis,
        "metadata adoption creates a new accepted descriptor; old material is immutable",
    );

    configure_specified_original(&mut app);
    app.spread_mode = SpreadMode::Single;
    let accepted_payload = FullscreenPaintPayload::NormalResource(accepted.clone());
    let mut baseline = Vec::new();
    specified_frame(&ctx, viewport, |ui, ctx| {
        baseline = draw_specified_current(&mut app, ui, ctx, Some(&accepted_payload));
    });
    let captured =
        FullscreenPaintPayload::PresentationMaterial(accepted.capture_presentation(&ctx));
    app.fs_cache.clear();
    app.thumbnails[..2].fill(ThumbnailState::Evicted);
    app.thumb_pixels.clear();
    app.page_dims_cache.clear();
    for item in &mut app.items {
        if let GridItem::PdfPage { content_type, .. } = item {
            *content_type = None;
        }
    }
    assert_eq!(captured.display_basis(), accepted_basis);
    specified_frame(&ctx, viewport, |ui, ctx| {
        let actual = draw_specified_current(&mut app, ui, ctx, Some(&captured));
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].fit_input, baseline[0].fit_input);
        same_frame_and_common_pixel(baseline[0], actual[0], 1.0);
    });
}

#[test]
fn display_basis_pdf_metadata_readoption_retires_only_its_dependent_passthrough() {
    use crate::material_display_basis::MaterialBasisUnavailable as U;
    let ctx = egui::Context::default();
    let mut app = pdf_thumbnails_without_metadata(&ctx);
    let old = app.ensure_passthrough_material(&ctx, 0).unwrap();
    let sibling = app.ensure_passthrough_material(&ctx, 1).unwrap();
    assert_eq!(
        old.display_basis().specified(),
        Err(U::PdfMetadataUnavailable)
    );
    let captured = old.capture_presentation(&ctx);
    let old_generation = app.input_generation.get(&0).copied();
    let old_edit_key = app.current_edit_result_key(0);
    retain_pdf_metadata_snapshot(&mut app, 0);
    app.adopt_retained_pdf_page_metadata(0);
    assert_eq!(app.input_generation.get(&0).copied(), old_generation);
    assert_eq!(app.current_edit_result_key(0), old_edit_key);
    let adopted = app.ensure_passthrough_material(&ctx, 0).unwrap();
    assert_eq!(
        adopted.source_texture_id(),
        old.source_texture_id(),
        "metadata refresh preserves the identity-processing sampler allocation",
    );
    assert_eq!(
        adopted.display_basis().specified().unwrap().original_size,
        egui::vec2(3072.0, 4096.0),
    );
    assert_eq!(
        app.cached_passthrough_material(0).unwrap().display_basis(),
        adopted.display_basis(),
    );
    let untouched = app.cached_passthrough_material(1).unwrap();
    assert_eq!(untouched.source_texture_id(), sibling.source_texture_id());
    assert_eq!(untouched.display_basis(), sibling.display_basis());
    assert_eq!(captured.display_basis(), old.display_basis());
    assert_eq!(
        captured.display_basis().specified(),
        Err(U::PdfMetadataUnavailable)
    );
}

#[test]
fn display_basis_direct_held_sampler_does_not_hide_readopted_material_basis() {
    use crate::material_display_basis::MaterialBasisUnavailable as U;
    let ctx = egui::Context::default();
    page_turn::install(&ctx);
    let mut app = pdf_thumbnails_without_metadata(&ctx);
    for idx in 2..4 {
        let path = app
            .tmp
            .path()
            .join(format!("direct-held-outgoing-{idx}.png"));
        app.items.push(GridItem::Image(path));
        app.visible_indices.push(idx);
        app.details_order.push(idx);
        app.thumbnails.push(ThumbnailState::Pending);
        let _ = install_complete(&mut app, &ctx, idx, [600, 800], [60, 80], [60, 80], false);
    }
    app.spread_mode = SpreadMode::Single;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    let viewport = egui::vec2(1200.0, 900.0);
    specified_frame(&ctx, viewport, |_, _| {});
    app.fullscreen_idx = Some(2);
    specified_frame(&ctx, viewport, |ui, ctx| {
        draw_specified_current(&mut app, ui, ctx, None);
    });
    specified_frame(&ctx, viewport, |ui, ctx| {
        let operation = page_turn::ViewerOperationId::Pad {
            batch: page_turn::next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(
            ctx,
            operation,
            page_turn::OperationPhase::Press,
            "direct-held-metadata-readoption",
        );
        app.offer_page_turn_with_operation(ctx, 0, false, Some(operation));
        app.fullscreen_idx = Some(0);
        draw_specified_current(&mut app, ui, ctx, None);
    });
    let held = app
        .page_turn_held_resource(0)
        .expect("incoming presentation is held");
    assert!(
        matches!(&held, FullscreenPaintPayload::PresentationMaterial(material) if material.source_id().is_none()),
        "the live turn captures a Direct presentation, not a prepared Native lease",
    );
    assert_eq!(
        held.display_basis().specified(),
        Err(U::PdfMetadataUnavailable)
    );
    let initial = app.fs_thumbnail_resource_for_display(0).unwrap();
    assert!(matches!(
        app.fullscreen_paint_payload_for_resource(0, initial),
        FullscreenPaintPayload::PresentationMaterial(_)
    ));
    retain_pdf_metadata_snapshot(&mut app, 0);
    let generation = app.input_generation.get(&0).copied();
    app.adopt_retained_pdf_page_metadata(0);
    assert_eq!(app.input_generation.get(&0).copied(), generation);
    let updated = app.fs_thumbnail_resource_for_display(0).unwrap();
    assert_eq!(updated.source_texture_id(), held.source_texture_id());
    assert_ne!(updated.display_basis(), held.display_basis());
    let selected = app.fullscreen_paint_payload_for_resource(0, updated.clone());
    assert!(matches!(
        selected,
        FullscreenPaintPayload::NormalResource(_)
    ));
    assert_eq!(selected.display_basis(), updated.display_basis());
    assert_eq!(
        held.display_basis().specified(),
        Err(U::PdfMetadataUnavailable)
    );
}

fn draw_prepared_single_frame(
    app: &mut App,
    ctx: &egui::Context,
    viewport: egui::Vec2,
    state: &FsFrameState,
) -> DisplayedImageTransform {
    let mut transform = None;
    specified_frame(ctx, viewport, |ui, ctx| {
        transform = app.draw_fs_image(
            ui,
            ui.max_rect(),
            0,
            app.fs_page_coordinate_source_size(0),
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            SingletonSpreadPlacement::Center,
            state.tex.as_ref(),
            state.thumb_tex.as_ref(),
            state.is_video,
            state.vst3_waiting_for_video,
            state.fs_load_failed,
            crate::rotation_db::Rotation::None,
            None,
            0.0,
            &FsBgStyle::Solid(egui::Color32::WHITE),
            "",
            false,
            app.effective_fullscreen_fit_mode(),
            app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
            None,
        );
    });
    transform.expect("the frame's accepted stand-in survives producer-cache eviction")
}

#[test]
fn display_basis_prepared_frame_keeps_selected_thumbnail_or_passthrough_after_all_caches_evict() {
    for known_uncropped in [false, true] {
        for mode in [
            FullscreenFitMode::Original,
            FullscreenFitMode::SpecifiedScale,
            FullscreenFitMode::Page,
        ] {
            let ctx = egui::Context::default();
            let mut app = fixture();
            app.spread_mode = SpreadMode::Single;
            configure_specified_original(&mut app);
            app.settings.fullscreen_fit_mode = mode;
            app.settings.fullscreen_fit_no_downscale = mode == FullscreenFitMode::SpecifiedScale;
            install_uncropped_thumbnails(&mut app, &ctx, [600, 800], [600, 800], known_uncropped);
            let ordinary = app.prepare_fullscreen_state(&ctx, 0, FsPageTurnDecision::normal());
            let throughput = app.prepare_fullscreen_state(
                &ctx,
                0,
                FsPageTurnDecision {
                    paint_source: FsPageTurnPaintSource::PassThrough,
                    work_admission: FsPageTurnWorkAdmission::Deferred,
                    passthrough_rendition_ready: true,
                },
            );
            assert!(ordinary.tex.is_none() && throughput.tex.is_none());
            let catalog = ordinary.thumb_tex.as_ref().unwrap();
            let rendition = throughput.thumb_tex.as_ref().unwrap();
            assert_eq!(
                catalog.source_texture_id() == rendition.source_texture_id(),
                !known_uncropped,
                "identity ordinary thumbnails alias; edit previews upload their own rendition",
            );
            assert_ne!(catalog.display_basis(), rendition.display_basis());
            assert_eq!(
                catalog.display_basis().specified(),
                rendition.display_basis().specified(),
            );
            let viewport = egui::vec2(1200.0, 900.0);
            specified_frame(&ctx, viewport, |_, _| {});
            let ordinary_before = draw_prepared_single_frame(&mut app, &ctx, viewport, &ordinary);
            let throughput_before =
                draw_prepared_single_frame(&mut app, &ctx, viewport, &throughput);
            app.thumbnails[..2].fill(ThumbnailState::Evicted);
            app.thumb_pixels.clear();
            app.page_dims_cache.clear();
            app.passthrough_rendition_cache = Default::default();
            app.fs_cache.clear();
            app.edit_result_cache.clear();
            app.clear_all_final_pipeline_caches(
                crate::app::FinalPipelineInvalidation::AdjustmentChanged,
            );
            let ordinary_after = draw_prepared_single_frame(&mut app, &ctx, viewport, &ordinary);
            let throughput_after =
                draw_prepared_single_frame(&mut app, &ctx, viewport, &throughput);
            assert_eq!(ordinary_before.fit_input, ordinary_after.fit_input);
            assert_eq!(throughput_before.fit_input, throughput_after.fit_input);
            same_frame_and_common_pixel(ordinary_before, ordinary_after, 1.0);
            same_frame_and_common_pixel(throughput_before, throughput_after, 1.0);
            if mode == FullscreenFitMode::Original {
                close_physical_size(ordinary_after, [60, 80], 1.0);
                close_physical_size(throughput_after, [600, 800], 1.0);
            } else if mode == FullscreenFitMode::SpecifiedScale {
                close_physical_size(ordinary_after, [600, 800], 1.0);
                close_physical_size(throughput_after, [600, 800], 1.0);
            }
        }
    }
}

#[test]
fn display_basis_pdf_noninteger_page_box_keeps_accepted_sampler_reference_after_frame_and_capture_eviction()
 {
    use crate::material_display_basis::{MaterialDisplayView as V, MaterialLegacyBasis as L};
    for ppp in [1.0, 1.25] {
        for mode in [FullscreenFitMode::Original, FullscreenFitMode::Page] {
            for no_downscale in [false, true] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(ppp);
                let mut app = fixture();
                let pdf_path = app.tmp.path().join("fractional-page-box.pdf");
                app.items = (0..2)
                    .map(|page_num| GridItem::PdfPage {
                        pdf_path: pdf_path.clone(),
                        page_num,
                        content_type: None,
                    })
                    .collect();
                app.spread_mode = SpreadMode::Single;
                app.settings.fullscreen_fit_mode = mode;
                app.settings.fullscreen_fit_no_downscale = no_downscale;
                accept_thumbnails_with_layout(
                    &mut app,
                    &ctx,
                    SourceCoverage::FullSource,
                    (317, 409),
                    [31, 40],
                    Some((613_125, 791_875)),
                );
                let state = app.prepare_fullscreen_state(&ctx, 0, FsPageTurnDecision::normal());
                assert!(state.tex.is_none());
                let resource = state.thumb_tex.as_ref().unwrap();
                let accepted = LegacySourceLayoutBasis {
                    layout_size: egui::vec2(613_125.0, 791_875.0),
                    reference_size: egui::vec2(31.0, 40.0),
                };
                for view in [V::Single, V::Spread, V::Continuous] {
                    assert_eq!(
                        resource.display_basis().legacy(view),
                        L::SourceLayout { basis: accepted },
                    );
                }
                assert_eq!(accepted.normalized_size().max_elem(), 40.0);
                let captured = FullscreenPaintPayload::PresentationMaterial(
                    resource.capture_presentation(&ctx),
                );
                let viewport = egui::vec2(240.0, 180.0) / ppp;
                specified_frame(&ctx, viewport, |_, _| {});
                let before = draw_prepared_single_frame(&mut app, &ctx, viewport, &state);
                assert_eq!(
                    before.fit_input,
                    FullscreenFitInput::SourceLayout {
                        mode,
                        basis: accepted,
                    },
                );
                app.thumbnails[..2].fill(ThumbnailState::Evicted);
                app.thumb_pixels.clear();
                app.page_dims_cache.clear();
                app.fs_cache.clear();
                app.passthrough_rendition_cache = Default::default();
                app.clear_all_final_pipeline_caches(
                    crate::app::FinalPipelineInvalidation::AdjustmentChanged,
                );
                let after = draw_prepared_single_frame(&mut app, &ctx, viewport, &state);
                assert_eq!(after.fit_input, before.fit_input);
                same_frame_and_common_pixel(before, after, ppp);
                specified_frame(&ctx, viewport, |ui, ctx| {
                    let replay = draw_specified_current(&mut app, ui, ctx, Some(&captured));
                    assert_eq!(replay.len(), 1);
                    assert_eq!(replay[0].fit_input, before.fit_input);
                    same_frame_and_common_pixel(before, replay[0], ppp);
                });
                assert_eq!(
                    captured.display_basis().legacy(V::Single),
                    L::SourceLayout { basis: accepted },
                    "page-box units, source raster metadata and sampler reference remain distinct",
                );
            }
        }
    }
}

#[test]
fn display_basis_pdf_known_worker_thumbnail_and_real_raster_arrival_keep_single_frame() {
    use crate::app::{
        FsLoadPurpose, FsNormalLoadPurpose, FsPendingValue, OriginalMaterialEntry,
        PdfRerenderRequestProbe, PollPrefetchOrigin,
    };
    use crate::fs_animation::FsLoadResult;
    for (source, page_box, bounds, sampler) in [
        (
            [600, 800],
            (600_000, 800_000),
            (150, 200, 300, 400),
            [60, 80],
        ),
        (
            [317, 409],
            (613_125, 791_875),
            (79, 102, 158, 204),
            [31, 40],
        ),
        // A crop-free edit preview still carries Known full-source coordinates.
        // Annotations/local edits do not turn it into an ordinary FullSource row.
        ([600, 800], (600_000, 800_000), (0, 0, 600, 800), [60, 80]),
    ] {
        for ppp in [1.0, 1.25] {
            for mode in [
                FullscreenFitMode::Original,
                FullscreenFitMode::Page,
                FullscreenFitMode::SpecifiedScale,
            ] {
                for no_downscale in [false, true] {
                    let ctx = egui::Context::default();
                    ctx.set_pixels_per_point(ppp);
                    let mut app = fixture();
                    // Keep all real PDF demand and adoption logic, but intercept
                    // dispatch so this synthetic path cannot race a missing-file
                    // Failed result against the successful raster below.
                    let pdf_rerender = PdfRerenderRequestProbe::start();
                    let pdf_path = app.tmp.path().join("known-pdf-arrival.pdf");
                    let content_type = crate::pdf_loader::PdfPageContentType::Raster {
                        w: source[0],
                        h: source[1],
                    };
                    app.items = (0..2)
                        .map(|page_num| GridItem::PdfPage {
                            pdf_path: pdf_path.clone(),
                            page_num,
                            content_type: Some(content_type),
                        })
                        .collect();
                    // Installing these synthetic items must advance every source
                    // queue owner, as the real item-install path does, before
                    // either thumbnail or fullscreen worker results are accepted.
                    app.bump_items_generation();
                    app.spread_mode = SpreadMode::Single;
                    configure_specified_original(&mut app);
                    app.settings.fullscreen_fit_mode = mode;
                    app.settings.fullscreen_fit_no_downscale = no_downscale;
                    let coverage = SourceCoverage::Known(
                        EditPreviewGeometry::new(
                            CanonicalPageDimensions::new(source[0], source[1]),
                            (source[0], source[1]),
                            bounds,
                        )
                        .unwrap(),
                    );
                    accept_thumbnails_with_layout(
                        &mut app,
                        &ctx,
                        coverage,
                        (bounds.2, bounds.3),
                        sampler,
                        Some(page_box),
                    );
                    let accepted = app.fs_thumbnail_resource_for_display(0).unwrap();
                    let crate::material_display_basis::MaterialLegacyBasis::SourceLayout { basis } =
                        accepted
                            .display_basis()
                            .legacy(crate::material_display_basis::MaterialDisplayView::Single)
                    else {
                        panic!("Known PDF previews retain their page-box layout contract");
                    };
                    assert_eq!(
                        basis.reference_size,
                        egui::vec2(source[0] as f32, source[1] as f32)
                    );
                    if !coverage.is_partial() {
                        assert!(!app.export_crop_pages.contains(&0));
                    }
                    let viewport = egui::vec2(1200.0, 900.0) / ppp;
                    specified_frame(&ctx, viewport, |_, _| {});
                    // The normal source worker is already in flight while its
                    // thumbnail is displayed. Let the actual edit/final selector
                    // join this pending owner instead of dispatching the fake PDF
                    // path, whose Failed result could overwrite an injected result.
                    let (source_tx, source_rx) = std::sync::mpsc::channel();
                    app.fs_pending.insert(
                        0,
                        FsPendingValue::new(
                            Arc::new(std::sync::atomic::AtomicBool::new(false)),
                            source_rx,
                            1,
                            FsLoadPurpose::Display,
                        ),
                    );
                    let before = draw_single(&mut app, &ctx, viewport);
                    let generation = app.items_generation;
                    assert_eq!(
                        app.fs_pending.get(&0).unwrap().purpose,
                        FsLoadPurpose::ResumeAfterNormalLoad {
                            normal: FsNormalLoadPurpose::Display,
                            entry: OriginalMaterialEntry::MaterialOnly,
                        },
                        "ordinary painting joins the initial worker through its real owner",
                    );
                    assert_eq!(app.fs_pending.current_items_generation(), generation);
                    source_tx
                        .send(FsLoadResult::StaticPdf {
                            ci: egui::ColorImage::filled(
                                [source[0] as usize, source[1] as usize],
                                egui::Color32::WHITE,
                            ),
                            source_dims: [source[0] as usize, source[1] as usize],
                            page_size_points: crate::pdf_loader::PdfPageSizePoints {
                                width: page_box.0 as f32 / 1000.0,
                                height: page_box.1 as f32 / 1000.0,
                            },
                            content_type,
                        })
                        .expect("the original pending worker still owns the result channel");
                    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
                    let cache_kind = match app.fs_cache.get(&0) {
                        Some(FsCacheEntry::Static { .. }) => "Static",
                        Some(FsCacheEntry::Failed) => "Failed",
                        Some(_) => "Other",
                        None => "None",
                    };
                    assert!(
                        app.source_material_for_idx(0).is_some(),
                        "PDF raster was not adopted: source={source:?}, ppp={ppp}, mode={mode:?}, no_downscale={no_downscale}, cache={cache_kind}, pending={:?}, backlog={}, generation={generation}",
                        app.fs_pending.get(&0).map(|pending| pending.purpose),
                        app.fs_upload_backlog.len(),
                    );
                    let after = draw_single(&mut app, &ctx, viewport);
                    for (_, target) in pdf_rerender.take_requests() {
                        assert!(
                            target <= source[0].max(source[1]),
                            "native raster demand must retain its real cap: {target}",
                        );
                    }
                    assert_eq!(before.fit_input, after.fit_input);
                    same_frame_and_common_pixel(before, after, ppp);
                }
            }
        }
    }
}

#[test]
fn display_basis_worker_thumbnail_without_source_dimensions_retains_specified_sampler_basis() {
    for flow in [0, 1, 2] {
        let ctx = egui::Context::default();
        let mut app = fixture();
        configure_specified_original(&mut app);
        app.settings.fullscreen_specified_scale.height.value =
            crate::settings::SpecifiedScaleAxis::LogicalPixels(10);
        app.spread_mode = if flow == 1 {
            SpreadMode::Ltr
        } else {
            SpreadMode::Single
        };
        app.reading_flow = if flow == 2 {
            ReadingFlow::Vertical
        } else {
            ReadingFlow::Paged
        };
        accept_legacy_catalog_thumbnails_without_source_dimensions(&mut app, &ctx);
        let accepted = app.fs_thumbnail_resource_for_display(0).unwrap();
        assert_eq!(
            accepted.display_basis().specified().unwrap(),
            FullscreenFitBasis::new(egui::vec2(60.0, 80.0), egui::vec2(60.0, 80.0)).unwrap(),
        );
        let captured =
            FullscreenPaintPayload::PresentationMaterial(accepted.capture_presentation(&ctx));
        let viewport = egui::vec2(1200.0, 900.0);
        specified_frame(&ctx, viewport, |_, _| {});
        let mut baseline = Vec::new();
        specified_frame(&ctx, viewport, |ui, ctx| {
            baseline = if flow == 2 {
                let sources = app.draw_fs_continuous_reading(
                    ui,
                    ctx,
                    ui.max_rect(),
                    0,
                    false,
                    FsPageTurnDecision::normal(),
                );
                assert!(!sources.pages.is_empty());
                sources
                    .pages
                    .iter()
                    .map(|page| {
                        app.fullscreen_page_layout
                            .page_by_idx(page.idx())
                            .unwrap()
                            .transform
                    })
                    .collect()
            } else {
                draw_specified_current(&mut app, ui, ctx, None)
            };
        });
        assert!(!baseline.is_empty());
        for transform in &baseline {
            assert_specified_raster_basis(transform.fit_input, [60, 80]);
            close_physical_size(*transform, [60, 80], 1.0);
        }
        app.thumbnails[..2].fill(ThumbnailState::Evicted);
        app.thumb_pixels.clear();
        app.page_dims_cache.clear();
        app.fs_cache.clear();
        app.clear_all_final_pipeline_caches(
            crate::app::FinalPipelineInvalidation::AdjustmentChanged,
        );
        assert_eq!(captured.display_basis(), accepted.display_basis());
        app.spread_mode = SpreadMode::Single;
        app.reading_flow = ReadingFlow::Paged;
        specified_frame(&ctx, viewport, |ui, ctx| {
            let replay = draw_specified_current(&mut app, ui, ctx, Some(&captured));
            assert_eq!(replay.len(), 1);
            assert_specified_raster_basis(replay[0].fit_input, [60, 80]);
            close_physical_size(replay[0], [60, 80], 1.0);
            if flow == 0 {
                same_frame_and_common_pixel(baseline[0], replay[0], 1.0);
            }
        });
    }
}
