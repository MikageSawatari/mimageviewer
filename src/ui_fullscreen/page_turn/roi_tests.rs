//! ROI coverage at the ordinary prepared resource -> canonical material boundary.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, ready_app};

const MEASURED_RIGHT_EDGE: f32 = 0.9999999403953552_f64 as f32;

#[test]
fn canonical_roi_roundoff_is_source_texel_bounded_and_never_leaks_native_uv() {
    use crate::rotation_db::Rotation::*;
    for size in [[90, 130], [900, 1300], [8192, 8192]] {
        for rotation in [None, Cw90, Cw180, Cw270] {
            let uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
                let (x, y) = crate::displayed_image_transform::inverse_uv(rotation, u, v);
                egui::pos2(x, y)
            });
            for edge in 0..4 {
                let mut roi = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                match edge {
                    0 => roi.min.x = 1.0 - MEASURED_RIGHT_EDGE,
                    1 => roi.min.y = 1.0 - MEASURED_RIGHT_EDGE,
                    2 => roi.max.x = MEASURED_RIGHT_EDGE,
                    _ => roi.max.y = MEASURED_RIGHT_EDGE,
                }
                let normalized = canonical_roi_uv(uv, roi, size).unwrap();
                assert_eq!(
                    normalized, uv,
                    "rotation={rotation:?}, edge={edge}, size={size:?}"
                );
                assert!(normalized.iter().all(|p| roi_unit().contains(*p)));

                // One hundredth of a source texel is still ten times the
                // admitted numerical allowance; genuine clipping is rejected.
                let missing = 0.01 / size[edge % 2] as f32;
                match edge {
                    0 => roi.min.x = missing,
                    1 => roi.min.y = missing,
                    2 => roi.max.x = 1.0 - missing,
                    _ => roi.max.y = 1.0 - missing,
                }
                assert_eq!(canonical_roi_uv(uv, roi, size), Err("partial_roi"));
            }
        }
    }
}

fn roi_unit() -> egui::Rect {
    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
}

fn gpu_frame(
    ctx: &egui::Context,
    gpu: &egui_wgpu::RenderState,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let output = ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2560.0, 1440.0),
            )),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root_ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root_ui, |ui| run(ui, ctx))
            });
        },
    );
    // Ordinary native preparation reads registered egui source textures.
    // Previous routing fixtures deliberately have no RenderState, which
    // bypasses native ROI generation altogether, regardless of aspect ratio.
    let mut renderer = gpu.renderer.write();
    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
    }
    for id in &output.textures_delta.free {
        renderer.free_texture(id);
    }
}

fn draw_observed(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsNavigatorTextureSources {
    let (composition, sources) = draw_current(app, ui, ctx);
    let decision = app.fs_page_turn_decision_for_frame(ctx, app.fullscreen_idx.unwrap());
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    sources
}

fn measured_book(
    ctx: &egui::Context,
    gpu: &egui_wgpu::RenderState,
    mode: SpreadMode,
) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.spread_page_gap_px = 4;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    app.wgpu_render_state = Some(gpu.clone());
    for idx in 0..4 {
        let image = egui::ColorImage::filled([900, 1300], egui::Color32::WHITE);
        let tex = ctx.load_texture(
            format!("measured-roi-{idx}"),
            image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::new(image),
                source_dims: Some([900, 1300]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (900, 1300));
    }
    install(ctx);
    gpu_frame(ctx, gpu, |_, _| {});
    app
}

fn warm_unit(app: &mut App, ctx: &egui::Context, gpu: &egui_wgpu::RenderState, anchor: usize) {
    app.fullscreen_idx = Some(anchor);
    // The ordinary selector may publish its processed egui texture on this
    // first draw. Its textures_delta is uploaded at frame end, so native
    // preparation can consume it only on the next ordinary frame, just as in
    // the application. This is preparation, not an animation-specific wait.
    for _ in 0..2 {
        gpu_frame(ctx, gpu, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw_observed(app, ui, ctx);
        });
    }
    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
        panic!("ordinary flat");
    };
    assert!(
        unit.pages
            .iter()
            .all(|p| p.resource.lanczos_output().is_some()),
        "native pages: {:?}",
        unit.pages
            .iter()
            .map(|p| (
                p.resource.source_texture_id(),
                p.resource.size(),
                p.transform.total_scale,
                p.transform.pixels_per_point,
                p.transform.paint_rect,
                p.transform.visible_region_request(p.painter.clip_rect()),
                gpu.renderer
                    .read()
                    .texture(&p.resource.source_texture_id())
                    .is_some(),
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        unit.pages.iter().any(|p| {
            p.resource
                .visible_source_uv_rect()
                .is_some_and(|roi| roi.max.x == MEASURED_RIGHT_EDGE)
        }),
        "fixture must reproduce the measured right-edge roundoff"
    );
}

#[test]
fn page_turn_actual_native_roi_roundoff_starts_and_completes_ltr_rtl_forward_backward() {
    // One headless GPU and two app factories cover both directions and the
    // genuine-partial fallback. No desktop, profile, file, or product launch.
    let gpu = egui_kittest::wgpu::create_render_state(crate::test_ui::wgpu_setup());
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.5);
        let mut app = measured_book(&ctx, &gpu, mode);
        for (from, to, forward) in [(0, 2, true), (2, 0, false)] {
            warm_unit(&mut app, &ctx, &gpu, to);
            warm_unit(&mut app, &ctx, &gpu, from);
            take_test_events();
            let operation = ViewerOperationId::Slideshow(next_operation_batch());
            gpu_frame(&ctx, &gpu, |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                app.page_turn_external_operation(
                    ctx,
                    operation,
                    OperationPhase::Press,
                    "native-roi-regression",
                );
                app.handle_fs_navigation_with_operation(
                    ctx,
                    false,
                    false,
                    None,
                    None,
                    None,
                    FsPageNav::Target(to),
                    None,
                    from,
                    Some(operation),
                );
                let sources = draw_observed(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                    panic!("paged receipt")
                };
                assert!(
                    receipt.curl,
                    "mode={mode:?} {from}->{to}, events={:?}",
                    take_test_events()
                );
                assert_eq!(receipt.pages().len(), 2);
            });
            let tracker = match &mut app.page_turn_presentation.state {
                PresentationState::Turning {
                    clock,
                    offer,
                    projection,
                    ..
                } => {
                    assert_eq!(offer.forward, forward);
                    assert_eq!(offer.submission.status(), SubmissionStatus::Started);
                    for page in projection
                        .outgoing
                        .pages
                        .iter()
                        .chain(&projection.incoming.pages)
                    {
                        if let crate::page_turn_animation::PageMaterial::Texture { uv, .. } =
                            &page.material
                        {
                            assert!(uv.iter().all(|p| roi_unit().contains(*p)));
                        }
                    }
                    *clock = MotionClock::Running {
                        start_at: Instant::now() - offer.duration - Duration::from_millis(1),
                    };
                    offer.submission.clone()
                }
                _ => panic!("started native turn"),
            };
            gpu_frame(&ctx, &gpu, |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                let sources = draw_observed(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                    panic!("canonical flat")
                };
                assert!(!receipt.curl);
                assert_eq!(receipt.pages().len(), 2);
            });
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            let events = take_test_events();
            for kind in ["start", "finish", "fully_visible"] {
                assert_eq!(
                    events
                        .iter()
                        .filter(|(f, k, _)| f.operation_id == operation && *k == kind)
                        .count(),
                    1,
                    "{events:?}"
                );
            }
            assert!(!events.iter().any(|(f, k, _)| f.operation_id == operation && matches!(*k, "skip" | "cancel")), "{events:?}");
            gpu_frame(&ctx, &gpu, |_, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
            });
        }
        // Submit an actually prepared partial native output through the same
        // flat owner, then navigate normally: missing image data still skips.
        warm_unit(&mut app, &ctx, &gpu, 2);
        gpu_frame(&ctx, &gpu, |_, ctx| {
            let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
                panic!("flat outgoing")
            };
            let mut pages = unit.pages.clone();
            let page = &mut pages[1];
            let source_limit = app.settings.anime_upscale_source_limit;
            // Presentation has no source handle. Re-prepare only from the
            // ordinary resident owner, exactly as a new view request would.
            let texture = app
                .resolve_fs_processed_texture(ctx, page.occurrence.idx, false)
                .unwrap();
            let normal = app.fullscreen_paint_resource_for_texture(page.occurrence.idx, texture);
            let (resource, _) = app.fs_lanczos_cache.resolve(
                Some(&gpu),
                &normal,
                page.transform.total_scale,
                1.5,
                Some(crate::displayed_image_transform::VisibleRegionRequest {
                    source_uv_rect: egui::Rect::from_min_max(
                        egui::Pos2::ZERO,
                        egui::pos2(0.99, 1.0),
                    ),
                    target_size: [1480, 2160],
                }),
                crate::adjustment::PostFilter::None,
                0,
                source_limit,
            );
            assert_eq!(resource.visible_source_uv_rect().unwrap().max.x, 0.99);
            page.resource = resource.into();
            let receipt = app.submit_paged_display_unit(ctx, &pages, None).unwrap();
            assert!(!receipt.curl);
        });
        let PresentationState::Flat(partial) = &app.page_turn_presentation.state else {
            panic!("submitted partial outgoing");
        };
        assert_eq!(canonical_unit(partial).err(), Some("partial_roi"));
        assert!(partial.evidence.was_accepted());
        take_test_events();
        let operation = ViewerOperationId::Slideshow(next_operation_batch());
        gpu_frame(&ctx, &gpu, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.page_turn_external_operation(
                ctx,
                operation,
                OperationPhase::Press,
                "partial-native-roi-regression",
            );
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(0),
                None,
                2,
                Some(operation),
            );
            let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
                panic!("partial source is admitted before projection coverage validation");
            };
            let tracker = offer.submission.clone();
            let sources = draw_observed(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("partial fallback receipt")
            };
            assert!(!receipt.curl);
            assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
            let PresentationState::Flat(incoming) = &app.page_turn_presentation.state else {
                panic!("partial ROI must switch to ordinary incoming flat");
            };
            assert!(incoming.pages.iter().all(|p| p.occurrence.idx < 2));
        });
        let events = take_test_events();
        assert_eq!(
            events
                .iter()
                .filter(|(f, k, reason)| f.operation_id == operation
                    && *k == "skip"
                    && *reason == "geometry_visibility_or_roi")
                .count(),
            1,
            "{events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|(f, k, _)| f.operation_id == operation && *k == "start")
        );
    }
}
