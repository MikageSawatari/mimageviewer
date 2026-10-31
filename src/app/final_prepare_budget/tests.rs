//! D3-4(a): exercise the real pending owner, presentation handler and root hook.
use super::*;
use crate::app::final_prepare::FinalEffectState;
use crate::app::final_prepare::tests::{app, start, wait_ready};
#[cfg(windows)]
use crate::ui_fullscreen::page_turn_submission_tests::adopt_interrupted_target;
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation,
};

fn begin(app: &mut App, number: u64) {
    app.speculative_gpu_frame.begin(number);
    app.speculative_gpu_frame.begin_pass();
}

fn ready_candidate(app: &mut App, ctx: &egui::Context) -> FinalCompositeKey {
    assert!(start(app, ctx, 1));
    wait_ready(app, 1)
}

fn assert_pending(app: &App, key: FinalCompositeKey) {
    assert!(matches!(
        app.final_effect_pending[&key].state,
        FinalEffectState::CpuReady(_)
    ));
    assert!(!app.final_composite_cache.contains_key(&key));
}

fn take_delta_ids(ctx: &egui::Context) -> (Vec<egui::TextureId>, Vec<egui::TextureId>) {
    let mut delta = ctx.tex_manager().write().take_delta();
    let observed = (
        delta.set.keys().copied().collect(),
        delta.free.iter().copied().collect(),
    );
    delta.clear();
    observed
}

#[test]
fn d3_4_activity_is_monotonic_across_passes_but_not_outer_frames() {
    let mut budget = SpeculativeGpuFrame::default();
    budget.begin(70);
    budget.note_page_turn_activity();
    budget.begin_pass();
    budget.begin(70);
    budget.note_ordinary_upload();
    assert!(!budget.claim(), "a discarded pass cannot refund activity");
    budget.begin(71);
    budget.begin_pass();
    assert!(
        budget.claim(),
        "activity must not stick across quiet frames"
    );
}

#[test]
fn d3_4_early_service_receives_cpu_result_but_only_root_finalizer_allocates() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let key = ready_candidate(&mut app, &ctx);
    begin(&mut app, 80);
    ctx.tex_manager().write().take_delta().clear();
    for _ in 0..3 {
        app.poll_final_effects(&ctx);
        app.service_final_prepare_before_lookup(&ctx);
        assert_pending(&app, key);
        assert_eq!(take_delta_ids(&ctx), (Vec::new(), Vec::new()));
    }
    app.observe_live_paged_for_final_prepare();
    app.finalize_speculative_gpu_output(&ctx, 81, 1);
    assert_pending(&app, key); // A post-run counter is not the producing frame.
    app.finalize_speculative_gpu_output(&ctx, 80, 1);
    let texture = app.final_composite_cache[&key].texture.id();
    assert!(!app.final_effect_pending.contains_key(&key));
    assert_eq!(take_delta_ids(&ctx).0, [texture]);
    app.finalize_speculative_gpu_output(&ctx, 80, 1);
    assert_eq!(take_delta_ids(&ctx), (Vec::new(), Vec::new()));
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_4_continuous_activity_keeps_ready_lease_and_first_quiet_frame_adopts() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    begin(&mut app, 89);
    app.note_page_turn_gpu_activity();
    // The real CPU producer can start and complete while GPU admission is held.
    let key = ready_candidate(&mut app, &ctx);
    let cancel = app.final_effect_pending[&key].cancel.clone();
    let reservation = app.final_prepare_cpu.snapshot();
    for number in 90..94 {
        begin(&mut app, number);
        app.observe_live_paged_for_final_prepare();
        app.note_page_turn_gpu_activity();
        app.finalize_speculative_gpu_output(&ctx, number, 2);
        assert_pending(&app, key);
        assert!(Arc::ptr_eq(&cancel, &app.final_effect_pending[&key].cancel));
        assert_eq!(app.final_prepare_cpu.snapshot(), reservation);
    }
    begin(&mut app, 94);
    app.observe_live_paged_for_final_prepare();
    app.finalize_speculative_gpu_output(&ctx, 94, 1);
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_4_display_promotion_and_synchronous_takeover_do_not_wait_for_activity() {
    for prepared in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let key = prepared.then(|| ready_candidate(&mut app, &ctx));
        begin(&mut app, 100);
        app.note_page_turn_gpu_activity();
        let display = app.ensure_final_composite_texture(&ctx, 1).unwrap();
        assert_eq!(
            app.exact_complete_final_composite_texture(1).unwrap().id(),
            display.id()
        );
        if let Some(key) = key {
            assert!(!app.final_effect_pending.contains_key(&key));
            assert_eq!(app.final_composite_cache[&key].texture.id(), display.id());
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        }
        assert!(!app.speculative_gpu_frame.claim());
    }
}

fn legacy_effect_params(app: &mut App, idx: usize, lut: bool) -> crate::adjustment::AdjustParams {
    let mut params = app.effective_params(idx).clone();
    if lut {
        params.creative_lut.id = Some(crate::creative_lut::BuiltinCreativeLut::WarmFilm.id());
        params.creative_lut.strength = 1.0;
        app.creative_lut_library = crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(
            crate::creative_lut::BuiltinCreativeLut::WarmFilm,
        );
        assert!(app.resolved_creative_lut(&params).is_some());
    } else {
        params.colorize.mode = crate::colorize::ColorizeMode::AllImages;
    }
    app.adjustment_page_params.insert(idx, params.clone());
    params
}

#[test]
fn d3_4_legacy_ready_deferral_has_no_poll_loop_but_computing_keeps_its_wake() {
    for reason in [
        FinalPrepareGpuDeferred::AnimationActivity,
        FinalPrepareGpuDeferred::LivePresentation,
        FinalPrepareGpuDeferred::OverBudget,
    ] {
        for computing in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            legacy_effect_params(&mut app, 1, false);
            let key = crate::app::final_prepare_tests::queued_final(&mut app, 1);
            let (tx, rx) = mpsc::channel();
            app.final_effect_pending.get_mut(&key).unwrap().origin =
                FinalEffectOrigin::LegacyPrefetch;
            if computing {
                app.final_effect_pending.get_mut(&key).unwrap().state = FinalEffectState::Computing(
                    crate::app::final_prepare::FinalEffectReceiver::Legacy(rx),
                );
            }
            for _ in 0..3 {
                ctx.run_pass(Default::default(), |_| {})
                    .drop_without_applying_deltas();
            }
            let wakes = Arc::new(Mutex::new(Vec::new()));
            let observed = wakes.clone();
            ctx.set_request_repaint_callback(move |event| observed.lock().unwrap().push(event));
            app.poll_final_effects(&ctx);
            if computing {
                assert!(
                    wakes
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|event| event.delay == Duration::from_millis(16))
                );
            } else {
                app.adopt_final_prepare_results(
                    &ctx,
                    vec![key],
                    FinalPrepareAdoption::RootFinalized(
                        Some(reason),
                        crate::ui_fullscreen::FinalUnitPosition::Next,
                    ),
                );
                assert_pending(&app, key);
                assert!(
                    wakes.lock().unwrap().is_empty(),
                    "ready {reason:?} cannot create an independent poll/repaint loop"
                );
            }
            drop(tx); // Keep the Computing channel connected throughout service.
        }
    }
}

fn legacy_prefetch(app: &mut App, ctx: &egui::Context, idx: usize, lut: bool) -> FinalCompositeKey {
    let params = legacy_effect_params(app, idx, lut);
    let pixels = match app.fs_cache.get(&idx).unwrap() {
        FsCacheEntry::Static { pixels, .. } => pixels.clone(),
        _ => panic!("resident static fixture required"),
    };
    let key =
        app.final_composite_key_for_pixels(app.current_edit_result_key(idx), pixels.size, &params);
    let size = pixels.size;
    let plan = build_final_composite_plan_without_ai(&params, app.resolved_creative_lut(&params));
    assert!(app.spawn_final_effect_job(ctx, key, pixels, size, plan, true, true));
    assert!(matches!(
        app.final_effect_pending[&key].origin,
        FinalEffectOrigin::LegacyPrefetch
    ));
    assert_eq!(wait_ready(app, idx), key);
    key
}

#[test]
fn d3_4_legacy_colorize_and_lut_prefetch_wait_for_quiet_final_output() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let key = legacy_prefetch(&mut app, &ctx, 1, lut);
        begin(&mut app, 150);
        ctx.tex_manager().write().take_delta().clear();
        app.poll_final_effects(&ctx);
        app.service_final_prepare_before_lookup(&ctx);
        assert_pending(&app, key);
        assert_eq!(take_delta_ids(&ctx), (Vec::new(), Vec::new()));
        app.observe_live_paged_for_final_prepare();
        app.note_page_turn_gpu_activity();
        app.finalize_speculative_gpu_output(&ctx, 150, 1);
        assert_pending(&app, key);
        begin(&mut app, 151);
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 151, 1);
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(!app.final_effect_pending.contains_key(&key));
        assert_eq!(
            take_delta_ids(&ctx).0,
            [app.final_composite_cache[&key].texture.id()]
        );
    }
}

#[test]
fn d3_4_legacy_and_bounded_candidates_share_one_registration_slot() {
    for mixed in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let first = if mixed {
            ready_candidate(&mut app, &ctx)
        } else {
            legacy_prefetch(&mut app, &ctx, 1, false)
        };
        let second = legacy_prefetch(&mut app, &ctx, 2, true);
        begin(&mut app, 160);
        app.poll_final_effects(&ctx);
        assert_pending(&app, first);
        assert_pending(&app, second);
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 160, 1);
        assert_eq!(
            [first, second]
                .iter()
                .filter(|key| app.final_composite_cache.contains_key(key))
                .count(),
            1
        );
        assert_eq!(app.final_effect_pending.len(), 1);
        begin(&mut app, 161);
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 161, 1);
        assert!(app.final_composite_cache.contains_key(&first));
        assert!(app.final_composite_cache.contains_key(&second));
        assert!(app.final_effect_pending.is_empty());
    }
}

#[test]
fn d3_4_legacy_prefetch_off_continuous_split_and_display_keep_early_adoption() {
    for lut in [false, true] {
        for mode in [
            "off",
            "vertical",
            "horizontal",
            "split_ltr",
            "split_rtl",
            "display",
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            match mode {
                "off" => app.settings.page_turn_animation_enabled = false,
                "vertical" => app.reading_flow = crate::settings::ReadingFlow::Vertical,
                "horizontal" => app.reading_flow = crate::settings::ReadingFlow::Horizontal,
                "split_ltr" => app.spread_mode = crate::settings::SpreadMode::SplitLtr,
                "split_rtl" => app.spread_mode = crate::settings::SpreadMode::SplitRtl,
                _ => {}
            }
            let key = legacy_prefetch(&mut app, &ctx, 1, lut);
            if mode == "display" {
                app.final_effect_pending
                    .get_mut(&key)
                    .unwrap()
                    .origin
                    .promote();
            }
            begin(&mut app, 170);
            app.note_page_turn_gpu_activity();
            app.poll_final_effects(&ctx);
            assert!(
                app.final_composite_cache.contains_key(&key),
                "lut={lut} mode={mode}"
            );
            assert!(!app.final_effect_pending.contains_key(&key));
        }
    }
}

#[test]
fn d3_4_legacy_ready_display_promotes_and_adopts_same_pass_without_edit_upload() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let key = legacy_prefetch(&mut app, &ctx, 1, lut);
        let cancel = app.final_effect_pending[&key].cancel.clone();
        begin(&mut app, 180);
        app.note_page_turn_gpu_activity();
        app.fullscreen_idx = Some(1);
        let display = app.ensure_final_composite_texture(&ctx, 1).unwrap();
        assert_eq!(app.final_composite_cache[&key].texture.id(), display.id());
        assert!(app.final_effect_pending.is_empty());
        assert!(app.edit_result_cache.is_empty());
        assert!(!cancel.load(Ordering::Relaxed));
    }
}

#[test]
fn d3_4_legacy_computing_display_promotes_original_receiver_without_blocking() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let params = legacy_effect_params(&mut app, 1, lut);
        let key =
            app.final_composite_key_for_pixels(app.current_edit_result_key(1), [80, 120], &params);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        // A held legacy sender makes the Computing lifecycle deterministic.
        // Completion after promotion proves that the receiver was preserved.
        let items_generation = app.items_generation;
        app.final_effect_pending.insert(
            key,
            FinalEffectPending {
                diagnostic: None,
                pdf_metadata: None,
                canonical_dims: None,
                source_size: [80, 120],
                cancel: cancel.clone(),
                state: FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)),
                items_generation,
                output_complete: true,
                nearest_sampler: false,
                origin: FinalEffectOrigin::LegacyPrefetch,
                gpu_diagnostics: Default::default(),
            },
        );
        begin(&mut app, 190);
        app.note_page_turn_gpu_activity();
        app.fullscreen_idx = Some(1);
        assert!(app.ensure_final_composite_texture(&ctx, 1).is_none());
        let pending = &app.final_effect_pending[&key];
        assert!(matches!(pending.origin, FinalEffectOrigin::LegacyDisplay));
        assert!(matches!(
            pending.state,
            FinalEffectState::Computing(FinalEffectReceiver::Legacy(_))
        ));
        assert!(Arc::ptr_eq(&cancel, &pending.cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.edit_result_cache.is_empty());
        tx.send(FinalCompositeResult::Ready {
            pixels: Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GOLD)),
            elapsed_ms: 0.0,
            timing: FinalCompositeTiming::default(),
        })
        .unwrap();
        app.poll_final_effects(&ctx);
        assert!(!app.final_effect_pending.contains_key(&key));
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(!cancel.load(Ordering::Relaxed));
    }
}

#[test]
fn d3_4_incomplete_legacy_display_keeps_the_ordinary_quality_producer_path() {
    for lut in [false, true] {
        for ready in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            let (key, _held_sender) = if ready {
                let key = legacy_prefetch(&mut app, &ctx, 1, lut);
                app.final_effect_pending
                    .get_mut(&key)
                    .unwrap()
                    .output_complete = false;
                (key, None)
            } else {
                let params = legacy_effect_params(&mut app, 1, lut);
                let key = app.final_composite_key_for_pixels(
                    app.current_edit_result_key(1),
                    [80, 120],
                    &params,
                );
                let (tx, rx) = mpsc::channel();
                let items_generation = app.items_generation;
                app.final_effect_pending.insert(
                    key,
                    FinalEffectPending {
                        diagnostic: None,
                        pdf_metadata: None,
                        canonical_dims: None,
                        source_size: [80, 120],
                        cancel: Arc::new(AtomicBool::new(false)),
                        state: FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)),
                        items_generation,
                        output_complete: false,
                        nearest_sampler: false,
                        origin: FinalEffectOrigin::LegacyPrefetch,
                        gpu_diagnostics: Default::default(),
                    },
                );
                (key, Some(tx))
            };
            let cancel = app.final_effect_pending[&key].cancel.clone();
            begin(&mut app, 195);
            app.note_page_turn_gpu_activity();
            app.fullscreen_idx = Some(1);
            // This is the actual early-return decision consumed by ensure_final:
            // an intermediate result cannot finish the ordinary display request.
            assert!(matches!(
                app.take_prepared_final_for_display(&ctx, 1),
                final_prepare::PreparedFinalDisplay::Continue
            ));
            if ready {
                assert!(!app.final_effect_pending.contains_key(&key));
                assert!(!app.final_composite_cache[&key].complete);
            } else {
                let pending = &app.final_effect_pending[&key];
                assert!(matches!(
                    pending.state,
                    FinalEffectState::Computing(FinalEffectReceiver::Legacy(_))
                ));
                assert!(Arc::ptr_eq(&pending.cancel, &cancel));
            }
            assert!(!cancel.load(Ordering::Relaxed));
        }
    }
}

#[cfg(windows)]
#[test]
fn d3_4_live_sibling_excludes_legacy_prefetch_until_park() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        let key = legacy_prefetch(&mut app, &ctx, 1, lut);
        let id = active_owner(&mut app, &ctx, 94_406);
        app.with_active_viewer_context(|owner| set_presentation(owner, &ctx, "turning"))
            .unwrap();
        begin(&mut app, 200);
        app.observe_live_paged_for_final_prepare();
        app.with_active_viewer_context(|owner| owner.observe_live_paged_for_final_prepare())
            .unwrap();
        app.poll_final_effects(&ctx);
        app.finalize_speculative_gpu_output(&ctx, 200, 1);
        assert_pending(&app, key);
        assert!(app.pause_current_active_viewer_context(&ctx));
        let parked = app
            .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
            .unwrap();
        begin(&mut app, 201);
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 201, 1);
        assert!(app.final_composite_cache.contains_key(&key));
        assert_eq!(
            app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                .unwrap(),
            parked
        );
    }
}

#[cfg(windows)]
pub(super) fn active_owner(app: &mut App, ctx: &egui::Context, window: u64) -> ViewerContextId {
    let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
        // Construct only this context's resident data. Creating another
        // AppTestEnv here would recursively acquire its process-wide test lock.
        owner.items = (0..4)
            .map(|idx| GridItem::Image(PathBuf::from(format!("C:/d3-4-owner/{window}/{idx}.png"))))
            .collect();
        owner.items_generation = 72;
        owner.visible_indices = vec![0, 1, 2, 3];
        owner.details_order = owner.visible_indices.clone();
        owner.thumbnails = vec![ThumbnailState::Pending; 4];
        owner.image_metas = vec![None; 4];
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = crate::settings::SpreadMode::Single;
        owner.reading_flow = crate::settings::ReadingFlow::Paged;
        owner.settings.page_turn_animation_enabled = true;
        owner.settings.page_turn_animation_duration_ms = 600;
        for idx in 0..4 {
            let pixels = Arc::new(egui::ColorImage::filled(
                [80, 120],
                egui::Color32::LIGHT_BLUE,
            ));
            let tex = ctx.load_texture(
                format!("d3-4-owner-{window}-{idx}"),
                pixels.clone(),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            owner.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels,
                    source_dims: Some([80, 120]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            owner.record_page_dims_for_spread(idx, (80, 120));
        }
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.set_detached_window_binding_for_test(Some(window));
    });
    app.with_active_viewer_context(|owner| {
        owner.activate_mounted_as_independent_detached(window);
        frame(ctx, |ui, ctx| draw_current_without_sources(owner, ui, ctx));
        assert!(owner.page_turn_current_unit_presented());
    })
    .unwrap();
    id
}

fn navigate(app: &mut App, ctx: &egui::Context) {
    let destination = app.page_turn_destination();
    app.page_turn_presentation
        .seed_operation_for_test(destination, 71);
    app.handle_fs_navigation_with_operation(
        ctx,
        false,
        false,
        None,
        None,
        None,
        crate::ui_fullscreen::FsPageNav::Target(1),
        None,
        0,
        Some(crate::ui_fullscreen::ViewerOperationId::Pad {
            batch: 71,
            ordinal: 0,
        }),
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.page_turn_presentation_pending());
}

#[cfg(windows)]
pub(super) fn set_presentation(app: &mut App, ctx: &egui::Context, state: &str) {
    frame(ctx, |ui, ctx| {
        navigate(app, ctx);
        if state != "offered" {
            draw_current_without_sources(app, ui, ctx);
            if state == "completing" {
                app.page_turn_presentation.cancel("d3-4-test-completion");
            }
        }
    });
    assert!(app.page_turn_gpu_fingerprint_for_test().starts_with(state));
}

#[cfg(windows)]
#[test]
fn d3_4_each_parked_presentation_blocks_same_frame_only_and_keeps_ownership() {
    for state in ["offered", "turning", "completing"] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        let key = ready_candidate(&mut app, &ctx);
        let id = active_owner(&mut app, &ctx, 94_401);
        begin(&mut app, 110);
        app.with_active_viewer_context(|owner| set_presentation(owner, &ctx, state))
            .unwrap();
        assert!(app.pause_current_active_viewer_context(&ctx));
        assert_eq!(
            app.detached_window_state(94_401),
            Some(DetachedWindowState::Parked)
        );
        assert_eq!(app.viewer_context_residence(id), ContextResidence::AtRest);
        let parked = app
            .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
            .unwrap();
        let snapshot = app
            .detached_image_windows
            .iter()
            .find(|window| window.id == 94_401)
            .unwrap();
        let view = app.deferred_detached_image_window_view(
            snapshot,
            app.detached_viewer_window_placement(),
            false,
        );
        let painted = frame(&ctx, |ui, _| {
            App::draw_detached_snapshot_for_gpu_test(ui, ui.max_rect(), &view);
        });
        assert!(
            !painted.shapes.is_empty(),
            "the real passive snapshot must paint"
        );
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 110, 1);
        assert_pending(&app, key);
        assert_eq!(
            app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                .unwrap(),
            parked
        );
        begin(&mut app, 111);
        frame(&ctx, |ui, _| {
            App::draw_detached_snapshot_for_gpu_test(ui, ui.max_rect(), &view);
        });
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, 111, 1);
        assert!(app.final_composite_cache.contains_key(&key));
        assert_eq!(
            app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                .unwrap(),
            parked
        );
    }
}

#[cfg(windows)]
#[test]
fn d3_4_parked_submitted_canonical_receipt_survives_other_context_registration_and_snapshot() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    let key = ready_candidate(&mut app, &ctx);
    let window = 94_405;
    let id = active_owner(&mut app, &ctx, window);
    begin(&mut app, 120);
    let accepted = app
        .with_active_viewer_context(|owner| {
            set_presentation(owner, &ctx, "completing");
            adopt_interrupted_target(&ctx, owner);
            (
                owner.accepted_completion_receipt_for_test(),
                owner.page_turn_gpu_fingerprint_for_test(),
            )
        })
        .unwrap();
    assert!(app.pause_current_active_viewer_context(&ctx));
    assert_eq!(
        app.detached_window_state(window),
        Some(DetachedWindowState::Parked)
    );
    assert_eq!(app.viewer_context_residence(id), ContextResidence::AtRest);
    let snapshot = app
        .detached_image_windows
        .iter()
        .find(|snapshot| snapshot.id == window)
        .unwrap();
    let view = app.deferred_detached_image_window_view(
        snapshot,
        app.detached_viewer_window_placement(),
        false,
    );
    for number in [120, 121] {
        if number == 121 {
            begin(&mut app, number);
        }
        app.observe_live_paged_for_final_prepare();
        app.finalize_speculative_gpu_output(&ctx, number, 1);
        if number == 120 {
            assert_pending(&app, key);
        } else {
            assert!(app.final_composite_cache.contains_key(&key));
        }
        let painted = frame(&ctx, |ui, _| {
            App::draw_detached_snapshot_for_gpu_test(ui, ui.max_rect(), &view);
        });
        assert!(!painted.shapes.is_empty(), "parked A must actually paint");
        assert_eq!(
            app.with_viewer_context(id, |owner| (
                owner.accepted_completion_receipt_for_test(),
                owner.page_turn_gpu_fingerprint_for_test(),
            ))
            .unwrap(),
            accepted,
            "park, B registration and A snapshot must preserve accepted receipt/material/lease"
        );
    }
}

#[cfg(windows)]
#[test]
fn d3_4_each_live_presentation_blocks_without_a_candidate_and_completion_resumes() {
    for state in ["offered", "turning", "completing"] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        let key = ready_candidate(&mut app, &ctx);
        let id = active_owner(&mut app, &ctx, 94_404);
        app.with_active_viewer_context(|owner| set_presentation(owner, &ctx, state))
            .unwrap();
        // Discard the earlier activity record: the current live owner alone
        // must exclude B, even though A has no speculative work of its own.
        begin(&mut app, 130);
        app.observe_live_paged_for_final_prepare();
        app.with_active_viewer_context(|owner| {
            assert!(owner.final_effect_pending.is_empty());
            owner.observe_live_paged_for_final_prepare();
        })
        .unwrap();
        app.finalize_speculative_gpu_output(&ctx, 130, 1);
        assert_pending(&app, key);
        if state == "completing" {
            app.with_active_viewer_context(|owner| adopt_interrupted_target(&ctx, owner))
                .unwrap();
            // The normal canonical output has been accepted. On its following
            // owning frame, the existing completion handler retires the barrier.
            begin(&mut app, 131);
            app.observe_live_paged_for_final_prepare();
            app.with_active_viewer_context(|owner| {
                frame(&ctx, |_, ctx| {
                    assert!(!owner.commit_page_turn_continuation(ctx));
                    assert!(
                        owner
                            .page_turn_gpu_fingerprint_for_test()
                            .starts_with("flat:")
                    );
                    owner.observe_live_paged_for_final_prepare();
                });
            })
            .unwrap();
            app.finalize_speculative_gpu_output(&ctx, 131, 1);
            assert!(app.final_composite_cache.contains_key(&key));
            assert!(
                !app.with_viewer_context(id, |owner| owner.page_turn_presentation_pending())
                    .unwrap()
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn d3_4_live_sibling_does_not_stop_cpu_or_other_context_display_producers() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    let id = active_owner(&mut app, &ctx, 94_405);
    app.with_active_viewer_context(|owner| set_presentation(owner, &ctx, "turning"))
        .unwrap();
    begin(&mut app, 140);
    app.observe_live_paged_for_final_prepare();
    let held = app
        .with_active_viewer_context(|owner| {
            owner.observe_live_paged_for_final_prepare();
            owner.page_turn_gpu_fingerprint_for_test()
        })
        .unwrap();
    let key = ready_candidate(&mut app, &ctx);
    app.poll_final_effects(&ctx);
    app.finalize_speculative_gpu_output(&ctx, 140, 1);
    assert_pending(&app, key);
    // A normal display consumes B's real prepared pixels without asking the
    // speculative slot. An unrelated ordinary miss can also allocate now.
    let promoted = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    let ordinary = app.ensure_final_composite_texture(&ctx, 2).unwrap();
    assert_eq!(app.final_composite_cache[&key].texture.id(), promoted.id());
    assert_eq!(
        app.exact_complete_final_composite_texture(2).unwrap().id(),
        ordinary.id()
    );
    assert!(app.final_effect_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    assert_eq!(
        app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
            .unwrap(),
        held
    );
}

#[cfg(windows)]
#[test]
fn d3_4_active_at_rest_blocks_but_mount_and_passive_runtime_do_not() {
    for runtime in [
        None,
        Some(DetachedWindowState::Active),
        Some(DetachedWindowState::Parked),
        Some(DetachedWindowState::ParkedLive),
        Some(DetachedWindowState::Closing),
    ] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        let key = ready_candidate(&mut app, &ctx);
        let id = active_owner(&mut app, &ctx, 94_402);
        app.with_active_viewer_context(|owner| set_presentation(owner, &ctx, "turning"))
            .unwrap();
        if let Some(runtime) = runtime {
            app.transition_detached_window_state(94_402, runtime, "d3-4-runtime-test");
        }
        // The frame starts after the offer, so only live ownership can exclude it.
        begin(&mut app, 120);
        app.observe_live_paged_for_final_prepare();
        let held = app
            .with_viewer_context(id, |owner| {
                if runtime.is_some() {
                    owner.observe_live_paged_for_final_prepare();
                }
                owner.page_turn_gpu_fingerprint_for_test()
            })
            .unwrap();
        assert_eq!(app.viewer_context_residence(id), ContextResidence::AtRest);
        app.finalize_speculative_gpu_output(&ctx, 120, 1);
        if runtime == Some(DetachedWindowState::Active) {
            assert_pending(&app, key);
        } else {
            assert!(
                app.final_composite_cache.contains_key(&key),
                "runtime={runtime:?}"
            );
        }
        assert_eq!(
            app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                .unwrap(),
            held
        );
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetryScenario {
    Quiet,
    OfferOnRetry,
    OfferThenPark,
}

#[cfg(windows)]
struct RetryApp {
    app: crate::app::AppTestEnvForTest,
    scenario: RetryScenario,
    final_calls: Vec<(u64, usize)>,
    ui_calls: usize,
    parked: Option<(ViewerContextId, String)>,
}

#[cfg(windows)]
impl eframe::App for RetryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let producing = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
        begin(&mut self.app, producing);
        self.app.observe_live_paged_for_final_prepare();
        self.app.poll_final_effects(&ctx);
        self.ui_calls += 1;
        let pass = ctx.current_pass_index();
        if self.scenario == RetryScenario::OfferThenPark && pass == 1 {
            let id = self.app.active_viewer_context_id().unwrap();
            assert!(self.app.pause_current_active_viewer_context(&ctx));
            let fingerprint = self
                .app
                .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                .unwrap();
            self.parked = Some((id, fingerprint));
        } else if self.scenario != RetryScenario::Quiet {
            let offer_now = match self.scenario {
                RetryScenario::OfferOnRetry => pass == 1,
                RetryScenario::OfferThenPark => pass == 0,
                RetryScenario::Quiet => false,
            };
            self.app
                .with_active_viewer_context(|owner| {
                    owner.observe_live_paged_for_final_prepare();
                    if offer_now {
                        navigate(owner, &ctx);
                        draw_current_without_sources(owner, ui, &ctx);
                        assert!(
                            owner
                                .page_turn_gpu_fingerprint_for_test()
                                .starts_with("turning:")
                        );
                    }
                })
                .unwrap();
        }
    }

    fn on_native_wgpu_root_output_finalized(
        &mut self,
        ctx: &egui::Context,
        frame: u64,
        passes: usize,
    ) {
        self.final_calls.push((frame, passes));
        self.app.finalize_speculative_gpu_output(ctx, frame, passes);
    }
}

#[cfg(windows)]
#[test]
fn d3_4_real_end_pass_retry_offer_excludes_allocation_and_quiet_retry_delivers_once() {
    for scenario in [
        RetryScenario::Quiet,
        RetryScenario::OfferOnRetry,
        RetryScenario::OfferThenPark,
    ] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        ctx.options_mut(|options| options.max_passes = 2.try_into().unwrap());
        let mut app = app(&ctx);
        let key = ready_candidate(&mut app, &ctx);
        if scenario != RetryScenario::Quiet {
            active_owner(&mut app, &ctx, 94_403);
        }
        ctx.tex_manager().write().take_delta().clear();
        ctx.on_end_pass(
            "D3-4 late discard",
            Arc::new(|ui| {
                if ui.ctx().current_pass_index() == 0 {
                    ui.ctx().request_discard("after App work");
                }
            }),
        );
        let discarded = Arc::new(Mutex::new(Vec::new()));
        let captured = discarded.clone();
        let delivery_ctx = ctx.clone();
        let delivery = ctx.enter_discard_texture_delivery(move |mut delta| {
            let observed: Vec<_> = delta
                .set
                .keys()
                .map(|id| {
                    let name = delivery_ctx
                        .tex_manager()
                        .read()
                        .meta(*id)
                        .map(|meta| meta.name.clone());
                    (*id, name)
                })
                .collect();
            // Ordinary font/source deltas may be delivered on the discarded
            // pass. Inspect their identity and explicitly consume every delta.
            delta.clear();
            captured.lock().unwrap().push(observed);
        });
        let mut driver = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let producing = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
        let mut wrapper = RetryApp {
            app,
            scenario,
            final_calls: Vec::new(),
            ui_calls: 0,
            parked: None,
        };
        let mut output = driver.update_native_wgpu(
            &mut wrapper,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            },
        );
        let output_sets: Vec<_> = output.textures_delta.set.keys().copied().collect();
        output.textures_delta.clear(); // assertions must not double-panic on drop
        let candidate_name = format!(
            "final_effect_{}_{}_{}_{}",
            key.edit_key.idx, key.edit_key.source_gen, key.params_hash, key.bg
        );
        assert_eq!(wrapper.ui_calls, 2);
        assert_eq!(wrapper.final_calls, [(producing, 2)]);
        assert_eq!(wrapper.app.speculative_gpu_frame.frame, Some(producing));
        assert_eq!(
            ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT),
            producing + 1
        );
        assert!(
            discarded
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .all(|(_, name)| name.as_deref() != Some(candidate_name.as_str())),
            "no speculative candidate delta in a discarded pass"
        );
        if scenario != RetryScenario::Quiet {
            assert_pending(&wrapper.app, key);
            assert!(output_sets.iter().all(|id| {
                ctx.tex_manager()
                    .read()
                    .meta(*id)
                    .is_none_or(|meta| meta.name != candidate_name)
            }));
        } else {
            let texture = wrapper.app.final_composite_cache[&key].texture.id();
            assert_eq!(output_sets.iter().filter(|id| **id == texture).count(), 1);
        }
        assert_eq!(
            take_delta_ids(&ctx),
            (Vec::new(), Vec::new()),
            "hook deltas transferred to FullOutput"
        );
        output.drop_without_applying_deltas();
        if scenario == RetryScenario::OfferThenPark {
            let (id, parked) = wrapper.parked.take().expect("real retry must park A");
            assert!(wrapper.app.active_viewer_context_id().is_none());
            assert_eq!(
                wrapper.app.detached_window_state(94_403),
                Some(DetachedWindowState::Parked)
            );
            assert_eq!(
                wrapper
                    .app
                    .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                    .unwrap(),
                parked
            );
            assert_eq!(
                wrapper.app.speculative_gpu_frame.state,
                RegistrationState::PageTurnActivity,
                "actual discarded curl activity survives park and the final quiet pass"
            );
            wrapper.scenario = RetryScenario::Quiet;
            let next_frame = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
            let mut next_output = driver.update_native_wgpu(
                &mut wrapper,
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    ..Default::default()
                },
            );
            let next_sets: Vec<_> = next_output.textures_delta.set.keys().copied().collect();
            next_output.textures_delta.clear();
            assert_eq!(wrapper.final_calls, [(producing, 2), (next_frame, 2)]);
            assert!(!wrapper.app.final_effect_pending.contains_key(&key));
            let texture = wrapper.app.final_composite_cache[&key].texture.id();
            assert_eq!(next_sets.iter().filter(|id| **id == texture).count(), 1);
            assert_eq!(
                wrapper
                    .app
                    .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
                    .unwrap(),
                parked
            );
            assert_eq!(take_delta_ids(&ctx), (Vec::new(), Vec::new()));
            next_output.drop_without_applying_deltas();
        }
        drop(delivery);
    }
}
