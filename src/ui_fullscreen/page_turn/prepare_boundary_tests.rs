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
            assert!(page.transform.total_scale * page.transform.pixels_per_point > 1.0);
            let resource = crate::gpu_lanczos::test_native_resource_with_region(
                source,
                visible.target_size,
                visible.source_uv_rect,
            );
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
            assert!(
                canonical_roi_uv(
                    canonical_source_uv(&page.transform),
                    material.visible_source_uv_rect().unwrap(),
                    material.size()
                )
                .is_ok()
            );
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
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, mode);
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
    let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state else {
        panic!("turning");
    };
    offer.accepted_at = Instant::now();
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
            let prepared = app.prepare_fs_paged_paint_payload(
                &payload,
                &transform,
                &painter,
                &underlay,
                FsPageLayoutSource::CurrentItem,
                true,
            );
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
