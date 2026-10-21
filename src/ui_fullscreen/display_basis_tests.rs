//! Released ordinary-image 100% means one selected texture pixel per physical display pixel.
//! Canonical annotation dimensions remain independent of that display basis after GPU clamping
//! or AI enlargement. These are headless draw/selector tests, not pixel-decoding tests.

use super::*;
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
        crate::app::EditResultEntry {
            pixels: source_pixels.clone(),
            texture: Some(source_texture.clone()),
        },
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
        crate::app::FinalCompositeEntry {
            canonical_dims,
            source_size: source,
            pixels: output_pixels,
            texture: output_texture.clone(),
            texture_options: egui::TextureOptions::LINEAR,
            complete: true,
        },
    );
    // ensure_final_composite_texture checks exact_complete_final_composite_texture before any
    // edit/AI CPU production. The valid complete key makes the empty-pixel metadata fixture safe.
    assert_eq!(
        app.resolve_fs_processed_texture(ctx, idx, false)
            .unwrap()
            .id(),
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
                            let selected = app.resolve_fs_processed_texture(ctx, 0, false);
                            let resource = selected.map(|texture| {
                                app.fullscreen_paint_resource_for_texture(0, texture)
                            });
                            let thumbnail = app.fs_thumbnail_texture_for_display(0);
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
                for idx in 0..2 {
                    let tex = metadata_texture(
                        &ctx,
                        format!("clamped-preview-{idx}"),
                        image_metadata([512, 384]),
                    );
                    app.thumbnails[idx] = ThumbnailState::Loaded {
                        tex,
                        origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                        from_edit_preview: true,
                        rendered_at_px: 512,
                        source_coverage: coverage,
                        source_dims: Some((4096, 3072)),
                        layout_dims: None,
                    };
                    app.record_page_dims_for_spread(idx, (20000, 15000));
                    app.thumb_pixels.insert(
                        idx,
                        Arc::new(egui::ColorImage::filled([512, 384], egui::Color32::WHITE)),
                    );
                }
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
                    let texture = app
                        .cached_passthrough_rendition(side)
                        .expect("rendition prepared");
                    let input = app.fs_fit_input_for_resource(side, mode, &texture).unwrap();
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
            let selected = app.resolve_fs_processed_texture(ctx, page.idx, false);
            let resource = single_payload
                .cloned()
                .or_else(|| app.fullscreen_paint_payload_for_page(page.idx, selected, false));
            let thumbnail = app.fs_thumbnail_texture_for_display(page.idx);
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
                    FullscreenFitMode::SpecifiedScale,
                    app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
                    content_bbox,
                    Some(&mut sources),
                    true,
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
            for idx in 0..2 {
                let texture = metadata_texture(
                    &ctx,
                    format!("held-partial-{idx}"),
                    image_metadata([512, 384]),
                );
                app.thumbnails[idx] = ThumbnailState::Loaded {
                    tex: texture,
                    origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                    from_edit_preview: true,
                    rendered_at_px: 512,
                    source_coverage: coverage,
                    source_dims: Some((4096, 3072)),
                    layout_dims: None,
                };
                app.record_page_dims_for_spread(idx, (20000, 15000));
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
fn display_basis_specified_full_spread_thumbnail_keeps_source_original_contract() {
    let ctx = egui::Context::default();
    let mut app = fixture();
    app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
    app.settings.fullscreen_fit_no_upscale = true;
    app.settings.fullscreen_fit_no_downscale = true;
    for idx in 0..2 {
        app.thumbnails[idx] = ThumbnailState::Loaded {
            tex: metadata_texture(
                &ctx,
                format!("full-spread-thumb-{idx}"),
                image_metadata([80, 60]),
            ),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            from_edit_preview: false,
            rendered_at_px: 80,
            source_coverage: SourceCoverage::FullSource,
            source_dims: Some((20000, 15000)),
            layout_dims: None,
        };
        app.record_page_dims_for_spread(idx, (20000, 15000));
    }
    for transform in draw_spread(&mut app, &ctx, egui::vec2(1200.0, 900.0)) {
        let FullscreenFitInput::SpecifiedScale { basis, .. } = transform.fit_input else {
            panic!("specified fit descriptor")
        };
        assert_eq!(basis.original_size, egui::vec2(20000.0, 15000.0));
        close_physical_size(transform, [20000, 15000], 1.0);
    }
}
