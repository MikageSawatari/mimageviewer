//! D3-2 producer admission and immutable accepted-presentation contracts.
use super::*;
use crate::app::final_prepare::tests::{begin_registration_frame, wait_ready};
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

#[cfg(windows)]
#[derive(Debug, PartialEq)]
pub(crate) struct AcceptedCompletionReceiptForTest {
    destination: PresentationDestination,
    frame: u64,
    pass: u64,
    pages: Vec<ReceiptPage>,
    accepted: bool,
}

#[cfg(windows)]
impl App {
    pub(crate) fn accepted_completion_receipt_for_test(&self) -> AcceptedCompletionReceiptForTest {
        let PresentationState::Completing {
            completion: FlatCompletion::Submitted(receipt),
            ..
        } = &self.page_turn_presentation.state
        else {
            panic!("expected Completing::Submitted before and after park");
        };
        assert!(receipt.was_accepted(), "canonical output must be accepted");
        assert!(!receipt.curl, "receipt must describe canonical flat output");
        assert!(!receipt.pages().is_empty());
        assert_eq!(receipt.destination, self.page_turn_destination());
        AcceptedCompletionReceiptForTest {
            destination: receipt.destination,
            frame: receipt.frame,
            pass: receipt.pass,
            pages: receipt.pages().to_vec(),
            accepted: receipt.was_accepted(),
        }
    }
}

fn fixture(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    install(ctx);
    let mut app = ready_app(ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app
}

fn adjust(app: &mut App, pages: &[usize]) {
    for &idx in pages {
        let mut params = app.effective_params(idx).clone();
        params.brightness += 10.0;
        app.adjustment_page_params.insert(idx, params);
    }
}

fn accept_current(app: &mut App, ctx: &egui::Context) {
    frame(ctx, |ui, ctx| {
        draw_current(app, ui, ctx);
    });
    assert!(app.page_turn_current_unit_presented());
}

/// These fixtures drive the real paged draw without App::ui. Observe its live
/// dispatch explicitly, then invoke the same post-output hook as native wgpu.
fn finish_live_registration(app: &mut App, ctx: &egui::Context, root_frame: u64) {
    frame(ctx, |ui, ctx| {
        draw_current(app, ui, ctx);
        app.observe_live_paged_for_final_prepare();
    });
    eframe::App::on_native_wgpu_root_output_finalized(app, ctx, root_frame, 1);
}

#[test]
fn d3_2_real_producer_requires_accepted_output_before_initial_open_prefetch() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx, SpreadMode::Single);
    adjust(&mut app, &[1]);
    // The output-acquired hook runs before run_pass returns. Inspect the
    // ordinary unit after drawing but still inside the pass, before that hook
    // can accept it. Installed topology and resident input already exist.
    frame(&ctx, |ui, ctx| {
        draw_current(&mut app, ui, ctx);
        assert!(app.next_final_prepare_occurrences(0).is_some());
        assert!(!app.page_turn_current_unit_presented());
        app.prefetch_next_final_unit(ctx, 0);
        assert!(app.final_effect_pending.is_empty());
    });
    assert!(app.page_turn_current_unit_presented());
    app.prefetch_next_final_unit(&ctx, 0);
    assert_eq!(app.final_effect_pending.len(), 1);
    let key = wait_ready(&mut app, 1);
    assert!(!app.final_composite_cache.contains_key(&key));
    begin_registration_frame(&mut app, 10);
    app.service_final_prepare_before_lookup(&ctx);
    assert!(app.exact_complete_final_composite_texture(1).is_none());
    assert_eq!(
        app.final_effect_pending.len(),
        1,
        "early service retains CpuReady"
    );
    finish_live_registration(&mut app, &ctx, 10);
    assert!(app.exact_complete_final_composite_texture(1).is_some());
    assert!(app.final_effect_pending.is_empty());
}

#[test]
fn d3_2_real_producer_stays_off_for_off_list_split_and_continuous_routes() {
    for case in [
        "off",
        "list",
        "split_ltr",
        "split_rtl",
        "vertical",
        "horizontal",
    ] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx, SpreadMode::Single);
        accept_current(&mut app, &ctx);
        adjust(&mut app, &[1]);
        match case {
            "off" => app.settings.page_turn_animation_enabled = false,
            "list" => app.fullscreen_idx = None,
            "split_ltr" => app.spread_mode = SpreadMode::SplitLtr,
            "split_rtl" => app.spread_mode = SpreadMode::SplitRtl,
            "vertical" => app.reading_flow = ReadingFlow::Vertical,
            "horizontal" => app.reading_flow = ReadingFlow::Horizontal,
            _ => unreachable!(),
        }
        app.prefetch_next_final_unit(&ctx, 0);
        assert!(app.final_effect_pending.is_empty(), "{case}");
        assert!(
            app.exact_complete_final_composite_texture(1).is_none(),
            "{case}"
        );
        // The ordinary synchronous miss remains available in this same pass.
        assert!(
            app.ensure_final_composite_texture(&ctx, 1).is_some(),
            "{case}"
        );
        assert!(
            app.exact_complete_final_composite_texture(1).is_some(),
            "{case}"
        );
        assert!(app.final_effect_pending.is_empty(), "{case}");
    }
}

#[test]
fn d3_2_plain_next_source_shares_handles_without_speculative_work_or_upload() {
    for ai_failed in [false, true] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx, SpreadMode::Single);
        let id = crate::app::source_texture_test_support::seed(&mut app, &ctx, 1);
        accept_current(&mut app, &ctx);
        if ai_failed {
            app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
            let mut params = app.effective_params(1).clone();
            params.upscale_model = Some(
                crate::ai::ModelKind::UpscaleRealEsrGeneralV3
                    .as_str()
                    .into(),
            );
            app.adjustment_page_params.insert(1, params);
            let key = app
                .final_ai_key_for_pixels(
                    app.current_edit_result_key(1),
                    [4, 6],
                    app.effective_params(1),
                )
                .expect("the configured AI request must exist");
            app.final_ai_failed.insert(key);
        }
        ctx.tex_manager().write().take_delta().clear();
        app.prefetch_next_final_unit(&ctx, 0);
        assert!(app.final_effect_pending.is_empty(), "ai_failed={ai_failed}");
        assert!(app.final_ai_pending.is_empty(), "ai_failed={ai_failed}");
        assert_eq!(
            app.ensure_final_composite_texture(&ctx, 1).unwrap().id(),
            id,
            "ai_failed={ai_failed}"
        );
        assert_eq!(app.current_edit_result_texture(1).unwrap().id(), id);
        assert_eq!(
            app.exact_complete_final_composite_texture(1).unwrap().id(),
            id
        );
        assert!(ctx.tex_manager().write().take_delta().is_empty());
    }
}

#[test]
fn d3_2_pair_cpu_ready_pages_share_one_registration_per_root_frame() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx, SpreadMode::Ltr);
    accept_current(&mut app, &ctx);
    adjust(&mut app, &[2, 3]);
    app.prefetch_next_final_unit(&ctx, 0);
    wait_ready(&mut app, 2);
    app.prefetch_next_final_unit(&ctx, 0);
    wait_ready(&mut app, 3);
    assert_eq!(app.final_effect_pending.len(), 2);
    assert!(app.exact_complete_final_composite_texture(2).is_none());
    assert!(app.exact_complete_final_composite_texture(3).is_none());

    begin_registration_frame(&mut app, 20);
    app.service_final_prepare_before_lookup(&ctx);
    assert_eq!(
        app.final_effect_pending.len(),
        2,
        "early service cannot allocate"
    );
    assert!(app.exact_complete_final_composite_texture(2).is_none());
    assert!(app.exact_complete_final_composite_texture(3).is_none());
    finish_live_registration(&mut app, &ctx, 20);
    let first = [2, 3]
        .into_iter()
        .filter(|&idx| app.exact_complete_final_composite_texture(idx).is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        first.len(),
        1,
        "one complete page cannot register its partner too"
    );
    assert_eq!(app.final_effect_pending.len(), 1);
    let id = app
        .exact_complete_final_composite_texture(first[0])
        .unwrap()
        .id();
    ctx.tex_manager().write().take_delta().clear();
    begin_registration_frame(&mut app, 20); // Another viewport/service in this frame.
    app.service_final_prepare_before_lookup(&ctx);
    assert_eq!(app.final_effect_pending.len(), 1);
    assert!(ctx.tex_manager().write().take_delta().is_empty());
    begin_registration_frame(&mut app, 21);
    app.service_final_prepare_before_lookup(&ctx);
    assert_eq!(app.final_effect_pending.len(), 1);
    finish_live_registration(&mut app, &ctx, 21);
    assert!(app.exact_complete_final_composite_texture(2).is_some());
    assert!(app.exact_complete_final_composite_texture(3).is_some());
    assert_eq!(
        app.exact_complete_final_composite_texture(first[0])
            .unwrap()
            .id(),
        id
    );
    assert!(app.final_effect_pending.is_empty());
}

/// As in prepare_boundary_tests, give the headless accepted presentation a
/// Native output matching its actual final texture and ordinary transform.
/// The final texture itself comes from the real speculative CPU worker.
fn native_pages(app: &App, pages: &[PreparedPagedPage]) -> Vec<PreparedPagedPage> {
    pages
        .iter()
        .map(|page| {
            let texture = page.resource.source_texture().unwrap().clone();
            let source = app.fullscreen_paint_resource_for_texture(page.occurrence.idx, texture);
            let visible = page
                .transform
                .visible_region_request(page.painter.clip_rect())
                .unwrap();
            let scale = page.transform.total_scale * page.transform.pixels_per_point;
            let resource = if scale < 1.0 {
                let target = source.size().map(|length| {
                    crate::displayed_image_transform::physical_pixel_extent(length as f32, scale)
                });
                crate::gpu_lanczos::test_native_resource(source, target)
            } else {
                crate::gpu_lanczos::test_native_resource_with_region(
                    source,
                    visible.target_size,
                    visible.source_uv_rect,
                )
            };
            assert!(
                resource
                    .capture_presentation(page.painter.ctx())
                    .matches_prepare(
                        page.transform.total_scale,
                        page.transform.pixels_per_point,
                        Some(visible),
                        crate::adjustment::PostFilter::None,
                        app.settings.downscale_smoothing_percent,
                        app.settings.anime_upscale_source_limit,
                    )
            );
            PreparedPagedPage {
                resource: resource.into(),
                ..page.clone()
            }
        })
        .collect()
}

fn prepared_native_turn() -> (
    egui::Context,
    crate::app::AppTestEnvForTest,
    Vec<PreparedPagedPage>,
    egui::TextureId,
) {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx, SpreadMode::Single);
    accept_current(&mut app, &ctx);
    adjust(&mut app, &[1]);
    app.prefetch_next_final_unit(&ctx, 0);
    wait_ready(&mut app, 1);
    begin_registration_frame(&mut app, 30);
    app.service_final_prepare_before_lookup(&ctx);
    assert!(app.exact_complete_final_composite_texture(1).is_none());
    finish_live_registration(&mut app, &ctx, 30);
    let prepared = app.exact_complete_final_composite_texture(1).unwrap().id();
    assert!(app.current_edit_result_texture(1).is_none());
    let mut templates = Vec::new();
    for anchor in [1, 0] {
        app.fullscreen_idx = Some(anchor);
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("ordinary flat template");
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
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "d3-2-frozen");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!("real accepted offer: {:?}", take_test_events());
        };
        let now = offer.accepted_at + Duration::from_millis(1);
        app.fullscreen_idx = Some(1);
        for page in &mut templates[0] {
            page.painter = ui.painter().with_clip_rect(page.painter.clip_rect());
        }
        assert!(
            app.submit_paged_display_unit_with_clock(ctx, &templates[0], None, || now)
                .unwrap()
                .curl
        );
    });
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("actual accepted turn");
    };
    assert!(incoming.evidence.was_accepted());
    let held = incoming.pages.clone();
    assert!(
        held.iter()
            .all(|page| page.resource.source_texture().is_none())
    );
    drop(templates);
    app.fs_lanczos_cache.take_test_resolve_input_ids();
    ctx.tex_manager().write().take_delta().clear();
    (ctx, app, held, prepared)
}

fn retain_clock(app: &mut App) {
    let PresentationState::Turning { clock, offer, .. } = &mut app.page_turn_presentation.state
    else {
        panic!("turning");
    };
    *clock = MotionClock::Running {
        start_at: Instant::now(),
    };
    offer.duration = Duration::from_secs(60);
}

fn assert_held(app: &App, held: &[PreparedPagedPage]) {
    let incoming = match &app.page_turn_presentation.state {
        PresentationState::Turning { incoming, .. }
        | PresentationState::Completing { incoming, .. } => incoming,
        _ => panic!("presentation must remain held"),
    };
    assert_eq!(incoming.pages.len(), held.len());
    for (current, old) in incoming.pages.iter().zip(held) {
        assert_eq!(current.occurrence, old.occurrence);
        assert_eq!(
            current.resource.paint_texture_id(),
            old.resource.paint_texture_id()
        );
        assert_eq!(
            current.resource.source_texture_id(),
            old.resource.source_texture_id()
        );
        assert_eq!(current.transform.viewport_rect, old.transform.viewport_rect);
        assert_eq!(
            current.transform.full_image_rect,
            old.transform.full_image_rect
        );
        assert_eq!(current.transform.paint_rect, old.transform.paint_rect);
        assert_eq!(
            canonical_source_uv(&current.transform),
            canonical_source_uv(&old.transform)
        );
    }
}

fn assert_canonical(app: &App, expected: &SubmittedUnitReceipt) {
    let PresentationState::Completing {
        completion: FlatCompletion::Submitted(current),
        ..
    } = &app.page_turn_presentation.state
    else {
        panic!("accepted canonical receipt must remain owned by completion");
    };
    assert!(current.was_accepted());
    assert_eq!(current.pages(), expected.pages());
    assert_eq!(current.destination(), expected.destination());
}

#[test]
fn d3_2_frozen_prepared_hit_reuses_final_without_edit_upload_or_resample() {
    let (ctx, mut app, held, prepared) = prepared_native_turn();
    for _ in 0..2 {
        retain_clock(&mut app);
        frame(&ctx, |ui, ctx| {
            app.service_final_prepare_before_lookup(ctx);
            assert_eq!(
                app.ensure_final_composite_texture(ctx, 1).unwrap().id(),
                prepared
            );
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                panic!("receipt");
            };
            assert!(receipt.curl);
            assert_eq!(
                sources.pages[0].resource.paint_texture_id(),
                held[0].resource.paint_texture_id()
            );
        });
        assert_held(&app, &held);
        assert!(app.current_edit_result_texture(1).is_none());
        assert!(ctx.tex_manager().write().take_delta().is_empty());
        assert!(
            app.fs_lanczos_cache
                .take_test_resolve_input_ids()
                .is_empty()
        );
    }
}

#[test]
fn d3_2_frozen_sync_miss_and_quality_publication_progress_without_rewriting_canonical() {
    for publish_larger_source in [false, true] {
        let (ctx, mut app, held, prepared) = prepared_native_turn();
        app.page_turn_presentation.cancel("d3-2-canonical");
        let mut canonical = None;
        frame(&ctx, |ui, ctx| {
            let (_, sources) = draw_current(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("canonical receipt");
            };
            assert!(!receipt.curl);
            canonical = Some(receipt);
        });
        let canonical = canonical.unwrap();
        assert!(canonical.was_accepted());
        let receipt_pages = canonical.pages().to_vec();
        assert_held(&app, &held);
        assert_canonical(&app, &canonical);
        if publish_larger_source {
            let pixels = Arc::new(egui::ColorImage::filled(
                [160, 240],
                egui::Color32::LIGHT_BLUE,
            ));
            let tex = ctx.load_texture(
                "d3-2-quality-source",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                1,
                FsCacheEntry::Static {
                    tex,
                    pixels,
                    source_dims: Some([160, 240]),
                    load_seq: 2,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            *app.input_generation.entry(1).or_default() += 1;
        } else {
            app.final_composite_cache
                .retain(|key, _| key.edit_key.idx != 1);
        }
        ctx.tex_manager().write().take_delta().clear();
        // A miss must still synchronously produce the ordinary current final;
        // Frozen is never used to suppress that producer or quality publication.
        let published = app.ensure_final_composite_texture(&ctx, 1).unwrap().id();
        assert_ne!(published, prepared);
        assert_eq!(
            app.exact_complete_final_composite_texture(1).unwrap().id(),
            published
        );
        let mut delta = ctx.tex_manager().write().take_delta();
        let produced_upload = !delta.is_empty();
        delta.clear();
        assert!(produced_upload);
        assert_held(&app, &held);
        assert_canonical(&app, &canonical);
        assert!(canonical.was_accepted());
        assert_eq!(canonical.pages(), receipt_pages.as_slice());
        frame(&ctx, |ui, ctx| {
            app.service_final_prepare_before_lookup(ctx);
            let (_, sources) = draw_current(&mut app, ui, ctx);
            assert_eq!(
                sources.pages[0].resource.paint_texture_id(),
                held[0].resource.paint_texture_id()
            );
        });
        assert_held(&app, &held);
        assert_canonical(&app, &canonical);
        assert_eq!(canonical.pages(), receipt_pages.as_slice());
        assert!(canonical.was_accepted());
        assert!(
            app.fs_lanczos_cache
                .take_test_resolve_input_ids()
                .is_empty()
        );
    }
}

#[cfg(windows)]
fn seed_window_owner(app: &mut App, ctx: &egui::Context) {
    app.items = (0..4)
        .map(|idx| GridItem::Image(PathBuf::from(format!("c:/d3-2-window/{idx}.png"))))
        .collect();
    app.visible_indices = vec![0, 1, 2, 3];
    app.details_order = app.visible_indices.clone();
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.fullscreen_idx = Some(0);
    app.items_generation = 51;
    app.spread_mode = SpreadMode::Single;
    app.reading_flow = ReadingFlow::Paged;
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(
            [80, 120],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("d3-2-window-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (80, 120));
    }
    accept_current(app, ctx);
    adjust(app, &[1]);
    app.prefetch_next_final_unit(ctx, 0);
    assert_eq!(app.final_effect_pending.len(), 1);
    wait_ready(app, 1);
}

#[cfg(windows)]
#[test]
fn d3_2_real_ready_result_survives_mount_deposit_until_live_hook_or_display_promotion() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx, SpreadMode::Single);
    accept_current(&mut app, &ctx);
    adjust(&mut app, &[1]);
    app.prefetch_next_final_unit(&ctx, 0);
    wait_ready(&mut app, 1);
    let sibling = app.build_window_context_for_test(93_201, |owner| seed_window_owner(owner, &ctx));
    for _ in 0..2 {
        app.with_window_context_for_test(sibling, |owner| {
            assert_eq!(owner.final_effect_pending.len(), 1);
            assert!(
                owner.exact_complete_final_composite_texture(1).is_none(),
                "mount is not display service"
            );
        });
        assert_eq!(
            app.final_effect_pending.len(),
            1,
            "sibling deposit preserves main work"
        );
    }
    begin_registration_frame(&mut app, 40);
    app.service_final_prepare_before_lookup(&ctx);
    assert!(app.exact_complete_final_composite_texture(1).is_none());
    finish_live_registration(&mut app, &ctx, 40);
    let main_id = app.exact_complete_final_composite_texture(1).unwrap().id();
    assert!(app.final_effect_pending.is_empty());
    app.with_window_context_for_test(sibling, |owner| {
        owner.service_final_prepare_before_lookup(&ctx);
        assert_eq!(
            owner.final_effect_pending.len(),
            1,
            "mount and early service preserve the sibling CpuReady"
        );
        assert!(owner.exact_complete_final_composite_texture(1).is_none());
        begin_registration_frame(owner, 41);
        owner.service_final_prepare_before_lookup(&ctx);
        owner.observe_live_paged_for_final_prepare();
    });
    assert!(app.active_detached_window_id().is_none());
    eframe::App::on_native_wgpu_root_output_finalized(&mut *app, &ctx, 41, 1);
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.final_effect_pending.len(), 1);
        assert!(
            owner.exact_complete_final_composite_texture(1).is_none(),
            "a fork without an active runtime binding is not a live final-hook route"
        );
        // An actual display request still promotes and consumes that exact
        // prepared result immediately; it does not wait for speculative admission.
        assert!(owner.ensure_final_composite_texture(&ctx, 1).is_some());
        assert!(owner.exact_complete_final_composite_texture(1).is_some());
        assert!(owner.final_effect_pending.is_empty());
    });
    assert_eq!(
        app.exact_complete_final_composite_texture(1).unwrap().id(),
        main_id
    );
    assert!(app.final_effect_pending.is_empty());
}

#[cfg(windows)]
#[test]
fn d3_2_snapshot_resume_adopts_its_real_ready_result_without_changing_main() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx, SpreadMode::Single);
    accept_current(&mut app, &ctx);
    let main_id = app.exact_complete_final_composite_texture(0).unwrap().id();
    let sibling =
        app.push_window_context_for_test(&ctx, 93_202, |owner| seed_window_owner(owner, &ctx));
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.final_effect_pending.len(), 1);
        assert!(owner.exact_complete_final_composite_texture(1).is_none());
    });
    begin_registration_frame(&mut app, 50);
    assert!(app.activate_detached_image_window_snapshot(&ctx, 93_202));
    app.with_active_viewer_context(|owner| {
        assert_eq!(owner.final_effect_pending.len(), 1);
        assert!(owner.exact_complete_final_composite_texture(1).is_none());
    })
    .unwrap();
    eframe::App::on_native_wgpu_root_output_finalized(&mut *app, &ctx, 50, 1);
    begin_registration_frame(&mut app, 51);
    app.with_active_viewer_context(|owner| {
        assert_eq!(
            owner.final_effect_pending.len(),
            1,
            "resume alone is not live drawing"
        );
        assert_eq!(owner.detached_viewer_window_id(), Some(93_202));
        frame(&ctx, |ui, ctx| {
            draw_current(owner, ui, ctx);
            owner.observe_live_paged_for_final_prepare();
        });
    })
    .unwrap();
    assert_eq!(app.active_detached_window_id(), Some(93_202));
    eframe::App::on_native_wgpu_root_output_finalized(&mut *app, &ctx, 51, 1);
    app.with_active_viewer_context(|owner| {
        assert!(owner.final_effect_pending.is_empty());
        assert!(owner.exact_complete_final_composite_texture(1).is_some());
    })
    .unwrap();
    assert_eq!(
        app.exact_complete_final_composite_texture(0).unwrap().id(),
        main_id
    );
    assert!(app.final_effect_pending.is_empty());
}
