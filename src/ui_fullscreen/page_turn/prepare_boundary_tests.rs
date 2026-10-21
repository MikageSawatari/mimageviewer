//! Preserve the released prepare boundary while Native presentations own no source handle.
use super::*;
use crate::gpu_lanczos::{FullscreenPaintPayload, FullscreenPaintResource};
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn native_pages(app: &App, pages: &[PreparedPagedPage]) -> Vec<PreparedPagedPage> {
    pages
        .iter()
        .map(|page| {
            let texture = page
                .resource
                .source_texture()
                .expect("ordinary template owns its source")
                .clone();
            let source = app.fullscreen_paint_resource_for_texture(page.occurrence.idx, texture);
            let visible = page
                .transform
                .visible_region_request(page.painter.clip_rect())
                .expect("visible page");
            let physical_scale = page.transform.total_scale * page.transform.pixels_per_point;
            let resource = if physical_scale < 1.0 {
                let size = source.size();
                let target = size.map(|length| {
                    crate::displayed_image_transform::physical_pixel_extent(
                        length as f32,
                        physical_scale,
                    )
                });
                crate::gpu_lanczos::test_native_resource(source, target)
            } else {
                crate::gpu_lanczos::test_native_resource_with_region(
                    source,
                    visible.target_size,
                    visible.source_uv_rect,
                )
            };
            let material = resource.capture_presentation(page.painter.ctx());
            assert!(
                material.matches_prepare(
                    page.transform.total_scale,
                    page.transform.pixels_per_point,
                    Some(visible),
                    crate::adjustment::PostFilter::None,
                    app.settings.downscale_smoothing_percent,
                    app.settings.anime_upscale_source_limit,
                ),
                "fixture must match the actual ordinary prepare request"
            );
            if let Some(roi) = material.visible_source_uv_rect() {
                assert!(
                    canonical_roi_uv(canonical_source_uv(&page.transform), roi, material.size())
                        .is_ok()
                );
            }
            PreparedPagedPage {
                resource: resource.into(),
                ..page.clone()
            }
        })
        .collect()
}

fn start_native_turn(
    mode: SpreadMode,
) -> (
    egui::Context,
    crate::app::AppTestEnvForTest,
    Vec<PreparedPagedPage>,
) {
    start_native_turn_with_source_size(mode, None)
}

fn start_native_turn_with_source_size(
    mode: SpreadMode,
    source_size: Option<[usize; 2]>,
) -> (
    egui::Context,
    crate::app::AppTestEnvForTest,
    Vec<PreparedPagedPage>,
) {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, mode);
    if let Some(size) = source_size {
        for idx in 0..4 {
            let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE));
            let tex = ctx.load_texture(
                format!("prepare-source-{idx}"),
                (*pixels).clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    tex,
                    pixels,
                    source_dims: Some(size),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
        }
    }
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    let target = if mode.is_spread() { 2 } else { 1 };
    let mut templates = Vec::new();
    for anchor in [target, 0] {
        app.fullscreen_idx = Some(anchor);
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("ordinary flat");
        };
        templates.push(native_pages(&app, &unit.pages));
    }
    frame(&ctx, |ui, ctx| {
        for page in &mut templates[1] {
            page.painter = ui.painter().with_clip_rect(page.painter.clip_rect());
        }
        app.submit_paged_display_unit(ctx, &templates[1], None);
    });
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    frame(&ctx, |ui, ctx| {
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "prepare-boundary");
        app.offer_page_turn_with_operation(ctx, target, true, Some(operation));
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!("real offer: {:?}", take_test_events());
        };
        let now = offer.accepted_at + Duration::from_millis(1);
        app.fullscreen_idx = Some(target);
        for page in &mut templates[0] {
            page.painter = ui.painter().with_clip_rect(page.painter.clip_rect());
        }
        let receipt = app
            .submit_paged_display_unit_with_clock(ctx, &templates[0], None, || now)
            .unwrap();
        assert!(receipt.curl, "real Native start: {:?}", take_test_events());
    });
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("turning");
    };
    let held = incoming.pages.clone();
    assert!(
        held.iter()
            .all(|page| page.resource.source_texture().is_none())
    );
    drop(templates);
    app.fs_lanczos_cache.take_test_resolve_input_ids();
    (ctx, app, held)
}

fn publish_quality(
    app: &mut App,
    ctx: &egui::Context,
    pages: &[PreparedPagedPage],
    size: [usize; 2],
) -> Vec<egui::TextureId> {
    pages
        .iter()
        .map(|page| {
            let idx = page.occurrence.idx;
            let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE));
            let texture = ctx.load_texture(
                format!("prepare-published-{idx}-{}x{}", size[0], size[1]),
                (*pixels).clone(),
                egui::TextureOptions::LINEAR,
            );
            let key = app.final_composite_key_for_pixels(
                app.current_edit_result_key(idx),
                size,
                app.effective_params(idx),
            );
            app.final_composite_cache
                .retain(|cached, _| cached.edit_key.idx != idx);
            app.final_composite_cache.insert(
                key,
                crate::app::FinalCompositeEntry {
                    texture_options: egui::TextureOptions::LINEAR,
                    canonical_dims: None,
                    source_size: size,
                    pixels,
                    texture: texture.clone(),
                    complete: true,
                },
            );
            texture.id()
        })
        .collect()
}

// Actual draw_current reads the real clock. Keep the fixture far from expiry so these
// prepare tests cannot turn a scheduler stall into an unrelated duration assertion.
fn retain_turn_clock(app: &mut App) {
    let PresentationState::Turning { clock, offer, .. } = &mut app.page_turn_presentation.state
    else {
        panic!("turning");
    };
    *clock = MotionClock::Running {
        start_at: Instant::now(),
    };
    offer.duration = Duration::from_secs(60);
}

fn assert_frozen_layout(app: &App, held: &[PreparedPagedPage]) {
    for old in held {
        let current = app
            .fullscreen_page_layout
            .page_by_occurrence(old.occurrence)
            .unwrap()
            .transform;
        assert_eq!(current.viewport_rect, old.transform.viewport_rect);
        assert_eq!(
            current.layout_full_image_rect,
            old.transform.layout_full_image_rect
        );
        assert_eq!(current.full_image_rect, old.transform.full_image_rect);
        assert_eq!(current.paint_rect, old.transform.paint_rect);
        assert_eq!(
            canonical_source_uv(&current),
            canonical_source_uv(&old.transform)
        );
    }
}

#[test]
fn stable_presentation_quality_publication_defers_new_source_prepare_single_and_pair() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for (size, bump_epoch) in [([80, 120], false), ([160, 240], false), ([160, 240], true)] {
            let (ctx, mut app, held) = start_native_turn(mode);
            if bump_epoch {
                for page in &held {
                    let epoch = app.input_generation.entry(page.occurrence.idx).or_default();
                    *epoch += 1;
                }
            }
            let published = publish_quality(&mut app, &ctx, &held, size);
            retain_turn_clock(&mut app);
            frame(&ctx, |ui, ctx| {
                for (old, published_id) in held.iter().zip(&published) {
                    assert_eq!(
                        app.resolve_fs_processed_texture(ctx, old.occurrence.idx, false)
                            .unwrap()
                            .id(),
                        *published_id,
                        "ordinary source publication must still progress"
                    );
                }
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                    panic!("actual receipt");
                };
                assert!(receipt.curl);
                assert!(
                    sources.pages.iter().zip(&held).all(|(current, old)| current
                        .resource
                        .paint_texture_id()
                        == old.resource.paint_texture_id()
                        && current.resource.source_texture().is_none())
                );
            });
            assert_frozen_layout(&app, &held);
            assert!(
                app.fs_lanczos_cache
                    .take_test_resolve_input_ids()
                    .is_empty(),
                "stable {mode:?} with source {size:?} epoch_bump={bump_epoch} must not start normal resampling of new quality"
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ViewChange {
    Resize,
    Dpi,
    Fit,
    Clip,
    Underlay,
    Filter,
}

fn changed_transform(
    old: &DisplayedImageTransform,
    texture_size: egui::Vec2,
    change: ViewChange,
) -> DisplayedImageTransform {
    DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        pixel_fit: old.pixel_fit,
        page_idx: old.page_idx,
        viewport_rect: if matches!(change, ViewChange::Resize) {
            egui::Rect::from_min_size(
                old.viewport_rect.min,
                old.viewport_rect.size() + egui::vec2(80.0, 40.0),
            )
        } else {
            old.viewport_rect
        },
        source_size: old.source_size,
        texture_size,
        rotation: old.rotation,
        free_rotation_rad: old.free_rotation_rad,
        content_bbox: old.content_bbox,
        fit_mode: if matches!(change, ViewChange::Fit) {
            FullscreenFitInput::legacy(FullscreenFitMode::Width)
        } else {
            old.fit_input
        },
        fit_scale_limits: old.fit_scale_limits,
        pixels_per_point: if matches!(change, ViewChange::Dpi) {
            old.pixels_per_point * 1.25
        } else {
            old.pixels_per_point
        },
        placement: old.placement,
    })
    .unwrap()
}

#[test]
fn changed_presentation_view_and_filter_reenter_ordinary_prepare() {
    for change in [
        ViewChange::Resize,
        ViewChange::Dpi,
        ViewChange::Fit,
        ViewChange::Clip,
        ViewChange::Underlay,
        ViewChange::Filter,
    ] {
        let (ctx, mut app, held) = start_native_turn(SpreadMode::Single);
        let published = publish_quality(&mut app, &ctx, &held, [160, 240]);
        if matches!(change, ViewChange::Filter) {
            app.settings.global_preset.post_filter = crate::adjustment::PostFilter::Nearest;
        }
        frame(&ctx, |ui, _| {
            let old = &held[0];
            let texture = app
                .current_final_composite_texture(old.occurrence.idx)
                .unwrap();
            assert_eq!(texture.id(), published[0]);
            let payload: FullscreenPaintPayload = app
                .fullscreen_paint_resource_for_texture(old.occurrence.idx, texture)
                .into();
            let transform = changed_transform(&old.transform, payload.size_vec2(), change);
            let painter = ui
                .painter()
                .with_clip_rect(if matches!(change, ViewChange::Clip) {
                    old.painter.clip_rect().shrink(1.0)
                } else {
                    old.painter.clip_rect()
                });
            let underlay = if matches!(change, ViewChange::Underlay) {
                FsBgStyle::Solid(egui::Color32::RED)
            } else {
                old.underlay.clone()
            };
            let prepared = app
                .prepare_fs_paged_paint_payload(
                    &payload,
                    &transform,
                    &painter,
                    &underlay,
                    FsPageLayoutSource::CurrentItem,
                    true,
                )
                .expect("resident source still enters ordinary prepare");
            assert!(
                matches!(prepared, FullscreenPaintPayload::NormalResource(_)),
                "{change:?}"
            );
        });
        assert_eq!(
            app.fs_lanczos_cache.take_test_resolve_input_ids(),
            published,
            "{change:?}"
        );
    }
}

#[test]
fn canonical_flat_then_thaw_prepares_published_source() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for enabled in [true, false] {
            let (ctx, mut app, held) = start_native_turn(mode);
            let published = publish_quality(&mut app, &ctx, &held, [160, 240]);
            app.page_turn_presentation.cancel("prepare-boundary-thaw");
            app.settings.page_turn_animation_enabled = enabled;
            frame(&ctx, |ui, ctx| {
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                    panic!("canonical receipt");
                };
                assert!(!receipt.curl);
                assert!(
                    sources
                        .pages
                        .iter()
                        .zip(&held)
                        .all(|(current, old)| current.resource.paint_texture_id()
                            == old.resource.paint_texture_id())
                );
            });
            assert!(
                app.fs_lanczos_cache
                    .take_test_resolve_input_ids()
                    .is_empty(),
                "canonical flat remains frozen"
            );
            frame(&ctx, |ui, ctx| {
                app.flush_submitted_page_turn_ready(ctx);
                assert!(
                    !app.commit_page_turn_continuation(ctx),
                    "no navigation continuation in fixture"
                );
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Flat(_)
                ));
                let (_, sources) = draw_current(&mut app, ui, ctx);
                assert!(
                    sources
                        .pages
                        .iter()
                        .zip(&published)
                        .all(|(current, id)| current.resource.source_texture_id() == *id)
                );
            });
            assert_eq!(
                app.fs_lanczos_cache.take_test_resolve_input_ids(),
                published,
                "{mode:?}: normal preparation begins when canonical adoption thaws the turn"
            );
        }
    }
}

#[test]
fn original_preview_nonfinal_and_faithful_spread_never_reuse_frozen_native() {
    for case in ["original_preview", "nonfinal", "faithful"] {
        let (ctx, mut app, held) = start_native_turn(SpreadMode::Ltr);
        let published = publish_quality(&mut app, &ctx, &held, [160, 240]);
        let expected = if case == "original_preview" {
            held.iter()
                .map(|page| {
                    app.resolve_original_preview_tex(page.occurrence.idx)
                        .unwrap()
                        .id()
                })
                .collect::<Vec<_>>()
        } else if case == "faithful" {
            held.iter()
                .map(|page| {
                    let idx = page.occurrence.idx;
                    let pixels =
                        Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::LIGHT_BLUE));
                    let texture = ctx.load_texture(
                        format!("prepare-faithful-{idx}"),
                        (*pixels).clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    app.thumbnails[idx] = ThumbnailState::Loaded {
                        tex: texture,
                        origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                        from_edit_preview: false,
                        rendered_at_px: 12,
                        source_coverage: crate::source_coverage::SourceCoverage::FullSource,
                        source_dims: Some((80, 120)),
                        layout_dims: None,
                    };
                    app.thumb_pixels.insert(idx, pixels);
                    app.ensure_passthrough_rendition(&ctx, idx)
                        .expect("resident faithful rendition")
                        .id()
                })
                .collect::<Vec<_>>()
        } else {
            published
        };
        retain_turn_clock(&mut app);
        app.fs_lanczos_cache.take_test_resolve_input_ids();
        frame(&ctx, |ui, ctx| {
            let anchor = app.fullscreen_idx.unwrap();
            let composition = app.spread_display_composition_for_anchor(anchor);
            let decision = if case == "faithful" {
                FsPageTurnDecision {
                    paint_source: FsPageTurnPaintSource::PassThrough,
                    work_admission: FsPageTurnWorkAdmission::Deferred,
                    passthrough_rendition_ready: true,
                }
            } else {
                FsPageTurnDecision::normal()
            };
            let rect = ui.max_rect();
            let sources = app.draw_fs_spread_with_submission(
                ui,
                ctx,
                rect,
                held[0].occurrence.idx,
                held[1].occurrence.idx,
                case == "original_preview",
                decision,
                None,
                StillSeekGeometry::bar_only(),
                case != "nonfinal",
                Some(&composition),
            );
            assert_eq!(sources.pages.len(), 2, "{case}");
            for ((selected, expected_id), old) in sources.pages.iter().zip(&expected).zip(&held) {
                assert!(
                    matches!(selected.resource, FullscreenPaintPayload::NormalResource(_)),
                    "{case}"
                );
                assert_eq!(selected.resource.paint_texture_id(), *expected_id, "{case}");
                assert_ne!(
                    selected.resource.paint_texture_id(),
                    old.resource.paint_texture_id(),
                    "{case}"
                );
            }
        });
        assert_eq!(
            app.fs_lanczos_cache.take_test_resolve_input_ids(),
            expected,
            "{case}: the caller's ordinary paint eligibility must reach prepare before reuse"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum AbsentStage {
    Turning,
    CompletingOn,
    CompletingOff,
}

fn prepare_absent_stage(app: &mut App, stage: AbsentStage) {
    retain_turn_clock(app);
    if !matches!(stage, AbsentStage::Turning) {
        app.page_turn_presentation.cancel("source-absent-fixture");
        app.settings.page_turn_animation_enabled = matches!(stage, AbsentStage::CompletingOn);
        assert!(app.page_turn_completion_active());
    }
}

fn evict_normal_sources(app: &mut App, ctx: &egui::Context, held: &[PreparedPagedPage]) {
    app.fs_cache.clear();
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
    app.comic_cache.clear();
    app.conceal_cache.clear();
    app.erase_result_cache.clear();
    app.erase_preview_cache.clear();
    app.erase_base_tex_cache.clear();
    app.local_adjust_cache.clear();
    app.local_adjust_layer_bypass_cache.clear();
    app.local_adjust_prefix_preview_cache.clear();
    for page in held {
        let idx = page.occurrence.idx;
        assert!(
            app.resolve_original_preview_tex(idx).is_none(),
            "raw source must actually be absent"
        );
        assert!(
            app.resolve_fs_display_tex(idx, false).is_none(),
            "processed normal source must actually be absent"
        );
        assert!(
            ctx.tex_manager()
                .read()
                .meta(page.resource.source_texture_id())
                .is_none(),
            "presentation cannot keep its old source"
        );
    }
    app.fs_lanczos_cache.take_test_resolve_input_ids();
}

fn install_allowed_thumbnails(
    app: &mut App,
    ctx: &egui::Context,
    held: &[PreparedPagedPage],
) -> Vec<egui::TextureId> {
    held.iter()
        .map(|page| {
            let idx = page.occurrence.idx;
            let size = [8, 12];
            let texture = ctx.load_texture(
                format!("source-absent-thumb-{idx}"),
                egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE),
                egui::TextureOptions::LINEAR,
            );
            let id = texture.id();
            app.thumbnails[idx] = ThumbnailState::Loaded {
                tex: texture,
                origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                from_edit_preview: false,
                rendered_at_px: 12,
                source_coverage: crate::source_coverage::SourceCoverage::FullSource,
                source_dims: Some((80, 120)),
                layout_dims: None,
            };
            assert_eq!(app.fs_thumbnail_texture_for_display(idx).unwrap().id(), id);
            id
        })
        .collect()
}

/// The existing draw_current helper requires a Single transform. Use the same real
/// resolver/selector/draw entry without unwrapping so loading is observable too.
fn draw_with_absent_source(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    held: &[PreparedPagedPage],
    original_preview_active: bool,
) -> FsNavigatorTextureSources {
    let anchor = app.fullscreen_idx.unwrap();
    let composition = app.spread_display_composition_for_anchor(anchor);
    let rect = ui.max_rect();
    if held.len() == 2 {
        return app.draw_fs_spread_with_submission(
            ui,
            ctx,
            rect,
            held[0].occurrence.idx,
            held[1].occurrence.idx,
            original_preview_active,
            FsPageTurnDecision::normal(),
            None,
            StillSeekGeometry::bar_only(),
            true,
            Some(&composition),
        );
    }
    let idx = held[0].occurrence.idx;
    let ordinary = app.resolve_fs_processed_texture(ctx, idx, original_preview_active);
    let payload = app.fullscreen_paint_payload_for_page(idx, ordinary, original_preview_active);
    let thumbnail = app.fs_thumbnail_texture_for_display(idx);
    let source_size = app
        .known_page_dims_for_idx(idx)
        .map(|(w, h)| egui::vec2(w as f32, h as f32));
    let live = !original_preview_active
        && app.settings.page_turn_animation_enabled
        && app.page_turn_view_eligible(idx);
    let fit_mode = app.effective_fullscreen_fit_mode();
    let limits = FullscreenFitScaleLimits {
        pixels_per_point: ctx.pixels_per_point(),
        ..Default::default()
    };
    let mut sources = FsNavigatorTextureSources::default();
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    if let Some(transform) = app.draw_fs_image_with_receipt(
        ui,
        rect,
        idx,
        source_size,
        FsPageLayoutSource::CurrentItem,
        None,
        None,
        composition.singleton_placement(),
        payload.as_ref(),
        thumbnail.as_ref(),
        false,
        false,
        false,
        crate::rotation_db::Rotation::None,
        None,
        0.0,
        &held[0].underlay,
        "",
        false,
        fit_mode,
        limits,
        None,
        Some(&mut sources),
        live,
    ) {
        app.fullscreen_page_layout
            .push_occurrence(transform, held[0].occurrence);
    }
    sources
}

fn texture_ids_in_output(output: &egui::FullOutput) -> Vec<egui::TextureId> {
    fn collect(shape: &egui::Shape, ids: &mut Vec<egui::TextureId>) {
        match shape {
            egui::Shape::Mesh(mesh) => ids.push(mesh.texture_id),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, ids);
                }
            }
            _ => {}
        }
    }
    let mut ids = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut ids);
    }
    ids
}

fn output_has_loading(output: &egui::FullOutput) -> bool {
    output.shapes.iter().any(|shape| {
        matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text().contains("\u{8aad}\u{8fbc}\u{4e2d}"))
    })
}

fn paint_fallback_navigator_and_emit_ready(
    app: &mut App,
    ui: &egui::Ui,
    ctx: &egui::Context,
    sources: &FsNavigatorTextureSources,
    thumbnails: &[egui::TextureId],
) {
    for page in &sources.pages {
        let transform = app
            .fullscreen_page_layout
            .page_by_occurrence(page.occurrence)
            .expect("fallback layout occurrence")
            .transform;
        assert!(sources.paint_page(ui.painter(), page.idx(), &transform, None));
    }
    let composition = app.spread_display_composition_for_anchor(app.fullscreen_idx.unwrap());
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
        panic!("fallback has an actual paged receipt");
    };
    if receipt.was_accepted() {
        // OFF's ordinary flat receipt is already accepted and not deferred.
        assert!(app.pending_fs_page_turn_ready.is_none());
        return;
    }
    let pending = app
        .pending_fs_page_turn_ready
        .as_ref()
        .expect("fallback submission awaits the accepted paint pass");
    assert_eq!(pending.sources.pages.len(), thumbnails.len());
    assert!(
        pending
            .sources
            .pages
            .iter()
            .zip(thumbnails)
            .all(|(page, id)| page.resource.paint_texture_id() == *id),
        "ready notification must retain the actual thumbnail selection"
    );
}

#[test]
fn source_absent_original_preview_never_paints_frozen_native_single_and_pair() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for stage in [
            AbsentStage::Turning,
            AbsentStage::CompletingOn,
            AbsentStage::CompletingOff,
        ] {
            for with_thumbnail in [false, true] {
                let (ctx, mut app, held) = start_native_turn(mode);
                prepare_absent_stage(&mut app, stage);
                evict_normal_sources(&mut app, &ctx, &held);
                let thumbnails = if with_thumbnail {
                    install_allowed_thumbnails(&mut app, &ctx, &held)
                } else {
                    Vec::new()
                };
                for page in &held {
                    assert!(
                        app.fullscreen_paint_payload_for_page(page.occurrence.idx, None, true)
                            .is_none(),
                        "original preview cannot select a processed frozen material after raw eviction"
                    );
                }
                let output = frame(&ctx, |ui, ctx| {
                    let sources = draw_with_absent_source(&mut app, ui, ctx, &held, true);
                    if with_thumbnail {
                        assert_eq!(sources.pages.len(), held.len());
                        assert!(
                            sources
                                .pages
                                .iter()
                                .zip(&thumbnails)
                                .all(|(page, id)| page.resource.paint_texture_id() == *id)
                        );
                        paint_fallback_navigator_and_emit_ready(
                            &mut app,
                            ui,
                            ctx,
                            &sources,
                            &thumbnails,
                        );
                    } else {
                        assert!(sources.pages.is_empty());
                    }
                });
                let painted = texture_ids_in_output(&output);
                assert!(
                    held.iter()
                        .all(|page| !painted.contains(&page.resource.paint_texture_id())),
                    "{mode:?} {stage:?}"
                );
                if with_thumbnail {
                    assert!(thumbnails.iter().all(|id| painted.contains(id)));
                } else {
                    assert!(
                        output_has_loading(&output),
                        "existing loading paint must remain visible"
                    );
                    assert!(
                        matches!(
                            app.page_turn_presentation.state,
                            PresentationState::Inactive
                        ),
                        "{mode:?} {stage:?}: missing candidates must retire the frozen presentation"
                    );
                }
            }
        }
    }
}

#[test]
fn source_absent_stable_view_keeps_native_single_and_pair() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for stage in [
            AbsentStage::Turning,
            AbsentStage::CompletingOn,
            AbsentStage::CompletingOff,
        ] {
            let (ctx, mut app, held) = start_native_turn(mode);
            prepare_absent_stage(&mut app, stage);
            evict_normal_sources(&mut app, &ctx, &held);
            let output = frame(&ctx, |ui, ctx| {
                let sources = draw_with_absent_source(&mut app, ui, ctx, &held, false);
                assert_eq!(sources.pages.len(), held.len());
                assert!(
                    sources.pages.iter().zip(&held).all(|(page, old)| page
                        .resource
                        .paint_texture_id()
                        == old.resource.paint_texture_id()
                        && page.resource.source_texture().is_none()),
                    "{mode:?} {stage:?}"
                );
            });
            assert_frozen_layout(&app, &held);
            assert!(
                app.fs_lanczos_cache
                    .take_test_resolve_input_ids()
                    .is_empty()
            );
            let painted = texture_ids_in_output(&output);
            assert!(
                held.iter()
                    .any(|page| painted.contains(&page.resource.paint_texture_id()))
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum AbsentChange {
    Resize,
    Dpi,
    Fit,
    DownscaleSmoothing,
}

#[test]
fn source_absent_changed_view_or_prepare_key_uses_existing_fallback() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for stage in [AbsentStage::Turning, AbsentStage::CompletingOff] {
            for change in [
                AbsentChange::Resize,
                AbsentChange::Dpi,
                AbsentChange::Fit,
                AbsentChange::DownscaleSmoothing,
            ] {
                let (ctx, mut app, held) = if matches!(change, AbsentChange::DownscaleSmoothing) {
                    start_native_turn_with_source_size(mode, Some([320, 480]))
                } else {
                    start_native_turn(mode)
                };
                prepare_absent_stage(&mut app, stage);
                evict_normal_sources(&mut app, &ctx, &held);
                let thumbnails = if matches!(change, AbsentChange::Resize) {
                    Vec::new()
                } else {
                    install_allowed_thumbnails(&mut app, &ctx, &held)
                };
                if matches!(change, AbsentChange::Fit) {
                    app.settings.fullscreen_fit_mode = FullscreenFitMode::Width;
                }
                if matches!(change, AbsentChange::DownscaleSmoothing) {
                    assert!(
                        held.iter()
                            .all(|p| p.resource.visible_source_uv_rect().is_none())
                    );
                    app.settings.downscale_smoothing_percent = 20;
                }
                let screen_size = if matches!(change, AbsentChange::Resize) {
                    egui::vec2(800.0, 500.0)
                } else {
                    egui::vec2(600.0, 400.0)
                };
                let input_ppp = if matches!(change, AbsentChange::Dpi) {
                    1.25
                } else {
                    1.0
                };
                ctx.set_pixels_per_point(input_ppp);
                let output = crate::test_ui::headless_paint_output(ctx.run_pass(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.root_ui(|root_ui| {
                            egui::CentralPanel::default().frame(egui::Frame::NONE).show(
                                root_ui,
                                |ui| {
                                    assert_eq!(ctx.pixels_per_point(), input_ppp);
                                    let sources =
                                        draw_with_absent_source(&mut app, ui, ctx, &held, false);
                                    if thumbnails.is_empty() {
                                        assert!(sources.pages.is_empty());
                                    } else {
                                        assert_eq!(sources.pages.len(), held.len());
                                        assert!(
                                            sources
                                                .pages
                                                .iter()
                                                .zip(&thumbnails)
                                                .all(|(page, id)| page.resource.paint_texture_id()
                                                    == *id),
                                            "{mode:?} {stage:?} {change:?}"
                                        );
                                        paint_fallback_navigator_and_emit_ready(
                                            &mut app,
                                            ui,
                                            ctx,
                                            &sources,
                                            &thumbnails,
                                        );
                                    }
                                },
                            )
                        });
                    },
                ));
                let painted = texture_ids_in_output(&output);
                assert!(
                    held.iter()
                        .all(|page| !painted.contains(&page.resource.paint_texture_id())),
                    "{mode:?} {stage:?} {change:?}: rejected frozen Native must not be painted"
                );
                if thumbnails.is_empty() {
                    assert!(output_has_loading(&output));
                    assert!(
                        matches!(
                            app.page_turn_presentation.state,
                            PresentationState::Inactive
                        ),
                        "{mode:?} {stage:?} {change:?}: missing candidates must retire the frozen presentation"
                    );
                } else {
                    assert!(thumbnails.iter().all(|id| painted.contains(id)));
                }
                let resolved = app.fs_lanczos_cache.take_test_resolve_input_ids();
                assert!(
                    resolved.iter().all(|id| thumbnails.contains(id)),
                    "no absent source or Native paint ID may become an input"
                );

                output.drop_without_applying_deltas();
            }
        }
    }
}
