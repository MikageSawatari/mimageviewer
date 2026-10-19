use super::*;

#[cfg(windows)]
mod lifecycle_tests;

struct StageGate {
    entered: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
thread_local! {
    static NEXT_GATE: std::cell::RefCell<Option<Arc<StageGate>>> = const { std::cell::RefCell::new(None) };
}
static GATES: Mutex<Vec<(usize, std::sync::Weak<StageGate>)>> = Mutex::new(Vec::new());

pub(crate) struct WorkerRegistration(usize);
impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        GATES.lock().unwrap().retain(|(id, _)| *id != self.0);
    }
}
pub(crate) fn attach_worker(cancel: &AtomicBool) -> Option<WorkerRegistration> {
    NEXT_GATE.with(|next| next.borrow_mut().take()).map(|gate| {
        let id = cancel as *const _ as usize;
        GATES.lock().unwrap().push((id, Arc::downgrade(&gate)));
        WorkerRegistration(id)
    })
}
pub(crate) fn worker_checkpoint(cancel: &AtomicBool) {
    let id = cancel as *const _ as usize;
    let gate = GATES
        .lock()
        .unwrap()
        .iter()
        .find_map(|(key, gate)| (*key == id).then(|| gate.upgrade()).flatten());
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        gate.resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .expect("test stage released");
    }
}

pub(crate) struct WorkerHold {
    _gate: Arc<StageGate>,
    entered: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
}
impl WorkerHold {
    pub(crate) fn new() -> Self {
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::channel();
        let gate = Arc::new(StageGate {
            entered: entered_tx,
            resume: Mutex::new(resume_rx),
        });
        NEXT_GATE.with(|next| assert!(next.borrow_mut().replace(gate.clone()).is_none()));
        Self {
            _gate: gate,
            entered,
            resume,
        }
    }
    pub(crate) fn entered(&self) {
        self.entered
            .recv_timeout(Duration::from_secs(10))
            .expect("worker paused after tone");
    }
    pub(crate) fn release(&self) {
        let _ = self.resume.send(());
    }
}
impl Drop for WorkerHold {
    fn drop(&mut self) {
        NEXT_GATE.with(|next| {
            next.borrow_mut().take();
        });
        self.release();
    }
}

pub(crate) fn begin_registration_frame(app: &mut App, frame: u64) {
    app.speculative_gpu_frame.begin(frame);
}

pub(crate) fn wait_idle(app: &App) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while app.final_prepare_cpu.snapshot().running != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not release its permit"
        );
        std::thread::yield_now();
    }
}

pub(crate) fn wait_ready(app: &mut App, idx: usize) -> FinalCompositeKey {
    let key = *app
        .final_effect_pending
        .keys()
        .find(|key| key.edit_key.idx == idx)
        .expect("worker was not started");
    let pending = app.final_effect_pending.get_mut(&key).unwrap();
    let ready = match &pending.state {
        FinalEffectState::Computing(FinalEffectReceiver::Prepared { rx, .. }) => {
            FinalEffectReady::Prepared(
                rx.recv_timeout(Duration::from_secs(10))
                    .expect("prepared pixels"),
            )
        }
        FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)) => FinalEffectReady::Legacy(
            rx.recv_timeout(Duration::from_secs(10))
                .expect("legacy pixels"),
        ),
        FinalEffectState::CpuReady(_) => {
            wait_idle(app);
            return key;
        }
    };
    pending.state = FinalEffectState::CpuReady(ready);
    wait_idle(app);
    key
}

pub(crate) fn unit(app: &App, idx: usize) -> FinalPrepareUnit {
    FinalPrepareUnit {
        context: app.projected_viewer_context_id(),
        items_generation: app.items_generation,
        pages: vec![crate::ui_fullscreen::SpreadPageOccurrence::navigation(
            idx, idx,
        )],
    }
}

pub(crate) fn start(app: &mut App, ctx: &egui::Context, idx: usize) -> bool {
    let Some((key, pixels, size, plan)) = app.final_prepare_plan_from_resident(idx) else {
        return false;
    };
    app.spawn_prepared_final_job(ctx, key, pixels, size, plan, unit(app, idx))
}

pub(crate) fn app(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = crate::ui_fullscreen::page_turn_submission_tests::ready_app(
        ctx,
        crate::settings::SpreadMode::Single,
    );
    app.settings.page_turn_animation_enabled = true;
    for idx in 1..4 {
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 15.0;
        app.adjustment_page_params.insert(idx, params);
    }
    let _ = app.page_turn_prefetch_targets(0, 2, 2);
    app
}

#[test]
fn d3_5b_manual_wrap_retires_obsolete_computing_and_ready_without_late_publication() {
    use crate::ui_fullscreen::page_turn_submission_tests::{
        draw_current_without_sources, frame, install_presentation,
    };

    for computing in [false, true] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        app.settings.fullscreen_manual_nav_wrap = true;
        frame(&ctx, |ui, ctx| {
            draw_current_without_sources(&mut app, ui, ctx);
        });
        assert!(app.page_turn_current_unit_presented());
        let hold = computing.then(WorkerHold::new);
        app.prefetch_next_final_unit(&ctx, 0);
        let key = *app.final_effect_pending.keys().next().unwrap();
        assert_eq!(key.edit_key.idx, 1);
        let cancel = app.final_effect_pending[&key].cancel.clone();
        if let Some(hold) = &hold {
            hold.entered();
        } else {
            assert_eq!(wait_ready(&mut app, 1), key);
        }
        let nav = app.spread_page_nav(-1);
        app.handle_fs_navigation(&ctx, false, false, None, None, None, nav, None, 0);
        assert_eq!(app.fullscreen_idx, Some(3));
        assert!(!app.page_turn_completion_active());
        let landed = app.page_turn_gpu_fingerprint_for_test();
        app.prefetch_next_final_unit(&ctx, 3);
        assert!(app.final_effect_pending.is_empty());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!app.final_composite_cache.contains_key(&key));
        assert_eq!(app.page_turn_gpu_fingerprint_for_test(), landed);
        if let Some(hold) = &hold {
            assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
            hold.release();
            wait_idle(&app);
        }
        app.poll_final_effects(&ctx);
        assert!(app.final_effect_pending.is_empty());
        assert!(!app.final_composite_cache.contains_key(&key));
        assert_eq!(app.page_turn_gpu_fingerprint_for_test(), landed);
        frame(&ctx, |ui, ctx| {
            draw_current_without_sources(&mut app, ui, ctx);
            let composition = app.spread_display_composition_for_anchor(3);
            assert_eq!(
                composition.pages_in_reading_order(),
                vec![crate::ui_fullscreen::SpreadPageOccurrence::navigation(3, 3)]
            );
        });
        assert!(app.page_turn_current_unit_presented());
        assert!(!app.final_composite_cache.contains_key(&key));
    }
}

#[test]
fn d3_2_cancel_takeover_holds_real_worker_permit_and_release_wakes_without_input() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let hold = WorkerHold::new();
    assert!(start(&mut app, &ctx, 1));
    hold.entered(); // The existing executor has completed tone, but not its later stages.
    let old_key = *app.final_effect_pending.keys().next().unwrap();
    app.cancel_final_effects_for_idx(1);
    assert_eq!(
        app.final_prepare_cpu.snapshot().running,
        1,
        "map removal cannot return a running permit"
    );
    assert!(app.final_prepare_cpu.snapshot().bytes > 0);
    assert!(
        !start(&mut app, &ctx, 2),
        "reverse/next demand cannot overlap canceled work"
    );
    // A synchronous display miss proceeds while exactly one old D3 worker is
    // still held. Removing its receiver has already revoked late publication.
    let display = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
    for _ in 0..3 {
        ctx.run_pass(Default::default(), |_| {})
            .drop_without_applying_deltas();
    }
    let (wake_tx, wake_rx) = mpsc::channel();
    ctx.set_request_repaint_callback(move |event| {
        let _ = wake_tx.send(event.viewport_id);
    });
    hold.release();
    wake_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("resource release must wake existing service with no input or periodic poll");
    wait_idle(&app);
    assert!(
        start(&mut app, &ctx, 2),
        "the release event permits the blocked demand"
    );
    let key = wait_ready(&mut app, 2);
    assert_eq!(
        app.final_composite_cache
            .get(&old_key)
            .unwrap()
            .texture
            .id(),
        display.id()
    );
    assert!(app.final_composite_cache.get(&key).is_none());
    app.cancel_final_effects_for_idx(2);
    app.poll_final_effects(&ctx);
    assert_eq!(
        app.final_composite_cache
            .get(&old_key)
            .unwrap()
            .texture
            .id(),
        display.id(),
        "late canceled work cannot overwrite takeover"
    );
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_2_same_key_computing_takeover_revokes_publication_without_waiting() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let hold = WorkerHold::new();
    assert!(start(&mut app, &ctx, 1));
    hold.entered();
    let key = *app.final_effect_pending.keys().next().unwrap();
    let cancel = app.final_effect_pending[&key].cancel.clone();
    let texture = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.final_effect_pending.contains_key(&key));
    assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
    hold.release();
    wait_idle(&app);
    app.poll_final_effects(&ctx);
    assert_eq!(
        app.exact_complete_final_composite_texture(1).unwrap().id(),
        texture.id()
    );
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_2_colorize_display_promotion_keeps_the_same_worker_and_reservation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.adjustment_page_params
        .get_mut(&1)
        .unwrap()
        .colorize
        .mode = crate::colorize::ColorizeMode::AllImages;
    let hold = WorkerHold::new();
    assert!(start(&mut app, &ctx, 1));
    hold.entered();
    let key = *app.final_effect_pending.keys().next().unwrap();
    let cancel = app.final_effect_pending[&key].cancel.clone();
    let reservation = app.final_prepare_cpu.snapshot();
    assert!(app.ensure_final_composite_texture(&ctx, 1).is_none());
    assert!(app.edit_result_cache.is_empty());
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(matches!(
        app.final_effect_pending[&key].origin,
        FinalEffectOrigin::DisplayPrepared(_)
    ));
    assert_eq!(app.final_prepare_cpu.snapshot(), reservation);
    hold.release();
    wait_ready(&mut app, 1);
    app.poll_final_effects(&ctx);
    assert!(app.exact_complete_final_composite_texture(1).is_some());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_2_entering_prepared_unit_preserves_computing_before_fullscreen_work_and_display_promotion() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.adjustment_page_params
        .get_mut(&1)
        .unwrap()
        .colorize
        .mode = crate::colorize::ColorizeMode::AllImages;
    let hold = WorkerHold::new();
    assert!(start(&mut app, &ctx, 1));
    hold.entered();
    let key = *app.final_effect_pending.keys().next().unwrap();
    let cancel = app.final_effect_pending[&key].cancel.clone();
    app.fullscreen_idx = Some(1);
    app.prefetch_next_final_unit(&ctx, 1); // Ordinary work runs before the first ensure.
    assert!(app.final_effect_pending.contains_key(&key));
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.ensure_final_composite_texture(&ctx, 1).is_none());
    assert!(Arc::ptr_eq(&cancel, &app.final_effect_pending[&key].cancel));
    assert!(matches!(
        app.final_effect_pending[&key].origin,
        FinalEffectOrigin::DisplayPrepared(_)
    ));
    hold.release();
    wait_ready(&mut app, 1);
    app.poll_final_effects(&ctx);
    assert!(app.exact_complete_final_composite_texture(1).is_some());
}

#[test]
fn d3_2_entering_prepared_unit_keeps_cpu_ready_until_display_takes_it_without_edit() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    assert!(start(&mut app, &ctx, 1));
    let key = wait_ready(&mut app, 1);
    app.fullscreen_idx = Some(1);
    app.prefetch_next_final_unit(&ctx, 1);
    assert!(app.final_effect_pending.contains_key(&key));
    begin_registration_frame(&mut app, 1);
    app.speculative_gpu_frame.note_ordinary_upload();
    assert!(app.ensure_final_composite_texture(&ctx, 1).is_some());
    assert!(app.edit_result_cache.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn d3_2_noop_worker_result_shares_exact_source_without_consuming_registration_slot() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let pixels = Arc::new(egui::ColorImage::new(
        [80, 120],
        (0..80 * 120)
            .map(|n| {
                [
                    egui::Color32::RED,
                    egui::Color32::GREEN,
                    egui::Color32::BLUE,
                ][n % 3]
            })
            .collect(),
    ));
    let texture = ctx.load_texture(
        "color-source",
        pixels.clone(),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let source = texture.id();
    app.bind_display_texture_identity(&texture, app.item_id(1));
    app.fs_cache.insert(
        1,
        FsCacheEntry::Static {
            tex: texture,
            pixels: pixels.clone(),
            source_dims: Some(pixels.size),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    let params = app.adjustment_page_params.get_mut(&1).unwrap();
    params.brightness = 0.0;
    params.colorize.mode = crate::colorize::ColorizeMode::MonochromeOnly;
    assert!(!crate::colorize::should_apply(&pixels, &params.colorize));
    assert!(start(&mut app, &ctx, 1));
    let key = wait_ready(&mut app, 1);
    ctx.tex_manager().write().take_delta().clear();
    begin_registration_frame(&mut app, 1);
    app.poll_final_effects(&ctx);
    assert_eq!(
        app.final_composite_cache.get(&key).unwrap().texture.id(),
        source
    );
    let mut delta = ctx.tex_manager().write().take_delta();
    let no_delta = delta.is_empty();
    delta.clear();
    assert!(no_delta);
    assert!(matches!(
        app.admit_final_prepare_registration([1, 1], DISPLAY_IMAGE_TEXTURE_OPTIONS),
        crate::app::final_prepare_budget::FinalPrepareGpuAdmission::Admitted
    ));
}

#[test]
fn d3_2_worker_pixels_match_actual_synchronous_final_and_display_takeover_never_uploads_edit() {
    use crate::adjustment::PostFilter;
    for case in [
        "tone",
        "sharpen",
        "post",
        "crt",
        "nearest",
        "colorize_lut",
        "ai",
        "clamp",
    ] {
        let ctx = egui::Context::default();
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    max_texture_side: Some(MAX_TEXTURE_DIM),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        let mut app = app(&ctx);
        if case == "clamp" {
            let size = [MAX_TEXTURE_DIM + 1, 2];
            let pixels = Arc::new(egui::ColorImage::new(
                size,
                (0..size[0] * size[1])
                    .map(|n| {
                        egui::Color32::from_rgba_unmultiplied(
                            (n % 251) as u8,
                            (n % 193) as u8,
                            (n % 127) as u8,
                            80 + (n % 176) as u8,
                        )
                    })
                    .collect(),
            ));
            let texture = ctx.load_texture(
                "oversize-source",
                final_upload_pixels(&pixels),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            app.fs_cache.insert(
                1,
                FsCacheEntry::Static {
                    tex: texture,
                    pixels,
                    source_dims: Some(size),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
        }
        let mut params = app.effective_params(1).clone();
        match case {
            "sharpen" => params.smart_sharpen = 50,
            "post" => params.post_filter = PostFilter::Sepia,
            "crt" => params.post_filter = PostFilter::CrtSimple,
            "nearest" => params.post_filter = PostFilter::Nearest,
            "colorize_lut" => {
                params.colorize.mode = crate::colorize::ColorizeMode::AllImages;
                params.creative_lut.id =
                    Some(crate::creative_lut::BuiltinCreativeLut::WarmFilm.id());
                params.creative_lut.strength = 1.0;
                app.creative_lut_library =
                    crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(
                        crate::creative_lut::BuiltinCreativeLut::WarmFilm,
                    );
                assert!(app.resolved_creative_lut(&params).is_some());
            }
            "ai" => {
                params.upscale_model = Some("realesr_general_v3".to_owned());
                params.smart_sharpen = 80;
            }
            _ => {}
        }
        app.adjustment_page_params.insert(1, params.clone());
        if case == "ai" {
            let key = app
                .final_ai_key_for_pixels(app.current_edit_result_key(1), [80, 120], &params)
                .unwrap();
            app.final_ai_cache.insert(
                key,
                FinalAiEntry {
                    canonical_dims: None,
                    source_size: [80, 120],
                    pixels: Arc::new(egui::ColorImage::filled(
                        [160, 240],
                        egui::Color32::from_rgb(81, 117, 139),
                    )),
                    used_upscale: true,
                },
            );
        }
        let texture = app.ensure_final_composite_texture(&ctx, 1);
        if texture.is_none() {
            wait_ready(&mut app, 1);
            app.poll_final_effects(&ctx);
        }
        let (key, expected) = app.exact_complete_final_composite_entry(1).expect(case);
        let expected = expected.pixels.clone();
        let key = key;
        app.final_composite_cache.remove(&key);
        app.edit_result_cache.clear();
        assert!(start(&mut app, &ctx, 1), "{case}");
        wait_ready(&mut app, 1);
        let FinalEffectState::CpuReady(FinalEffectReady::Prepared(ready)) =
            &app.final_effect_pending[&key].state
        else {
            panic!("ready");
        };
        assert_eq!(*ready.pixels, *expected, "{case}");
        assert_eq!(*ready.upload, *final_upload_pixels(&expected), "{case}");
        assert_eq!(app.final_prepare_cpu.snapshot().units, 1);
        // No speculative frame is available; display must take CpuReady anyway.
        assert!(
            app.ensure_final_composite_texture(&ctx, 1).is_some(),
            "{case}"
        );
        assert!(
            app.edit_result_cache.is_empty(),
            "display consumed ready before edit for {case}"
        );
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn d3_2_cpu_ready_revalidates_items_item_edit_params_and_context_binding() {
    for mutation in ["items", "item", "edit", "params", "context"] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        assert!(start(&mut app, &ctx, 1));
        let key = wait_ready(&mut app, 1);
        match mutation {
            "items" => app.items_generation += 1,
            "item" => app.items[1] = GridItem::Image(PathBuf::from("C:/other.png")),
            "edit" => app.bump_input_generation(1),
            "params" => app.adjustment_page_params.get_mut(&1).unwrap().brightness += 1.0,
            _ => {
                if let FinalEffectOrigin::Speculative(unit) =
                    &mut app.final_effect_pending.get_mut(&key).unwrap().origin
                {
                    unit.context = ViewerContextId::for_test(999_999);
                }
            }
        }
        begin_registration_frame(&mut app, 1);
        app.poll_final_effects(&ctx);
        assert!(app.final_composite_cache.get(&key).is_none(), "{mutation}");
        assert!(!app.final_effect_pending.contains_key(&key), "{mutation}");
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn d3_2_cpu_only_inputs_skip_missing_edit_materialization_and_reuse_completed_ai() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.mask_pages.insert(1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    assert!(app.erase_result_cache.is_empty());
    app.mask_pages.clear();
    app.local_adjust_pages.insert(1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    assert!(app.local_adjust_pending.is_empty());
    app.local_adjust_pages.clear();
    app.conceal_pages.insert(1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    assert!(app.conceal_cache.is_empty());
    app.conceal_pages.clear();
    let mut params = app.effective_params(1).clone();
    params.upscale_model = Some("realesr_general_v3".to_owned());
    app.adjustment_page_params.insert(1, params.clone());
    let ai_key = app
        .final_ai_key_for_pixels(app.current_edit_result_key(1), [80, 120], &params)
        .unwrap();
    let pixels = Arc::new(egui::ColorImage::filled([160, 240], egui::Color32::GOLD));
    app.final_ai_cache.insert(
        ai_key,
        FinalAiEntry {
            canonical_dims: None,
            source_size: [80, 120],
            pixels: pixels.clone(),
            used_upscale: true,
        },
    );
    app.fs_cache.remove(&1);
    let (_, source, _, plan) = app.final_prepare_plan_from_resident(1).unwrap();
    assert!(Arc::ptr_eq(&source, &pixels));
    assert!(plan.adjust_before_effect.is_none());
    assert!(app.edit_result_cache.is_empty());
}

#[test]
fn d3_2_output_reservation_includes_post_extent_and_distinct_clamp() {
    use crate::adjustment::PostFilter;
    assert_eq!(prepared_output_bytes([4, 6], PostFilter::None), Some(96));
    assert_eq!(
        prepared_output_bytes([4, 6], PostFilter::CrtSimple),
        Some(96 * 16)
    );
    let size = [MAX_TEXTURE_DIM + 1, 1];
    assert_eq!(
        prepared_output_bytes(size, PostFilter::Nearest),
        Some(((MAX_TEXTURE_DIM + 1 + MAX_TEXTURE_DIM) * 4) as u64)
    );
}

#[test]
fn d3_2_upload_preserves_arc_within_limit_and_triangle_pixels_above_it() {
    let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::GOLD));
    assert!(Arc::ptr_eq(&pixels, &final_upload_pixels(&pixels)));
    let pixels = Arc::new(egui::ColorImage::filled(
        [MAX_TEXTURE_DIM + 1, 1],
        egui::Color32::GOLD,
    ));
    let expected = clamp_for_gpu(&pixels).into_owned();
    let actual = final_upload_pixels(&pixels);
    assert!(!Arc::ptr_eq(&pixels, &actual));
    assert_eq!(*actual, expected);
}

#[test]
fn back_measurement_scripts_parse_without_running_any_ui() {
    let mut engine = rhai::Engine::new();
    engine.set_max_expr_depths(64, 64);
    for script in [
        include_str!("../../../scripts/page-turn/back-main.rhai"),
        include_str!("../../../scripts/page-turn/back-f12-immediate.rhai"),
        include_str!("../../../scripts/page-turn/back-f12-idle.rhai"),
        include_str!("../../../scripts/page-turn/back-two-context-reversal.rhai"),
        include_str!("../../../scripts/page-turn/back-boundary-wrap.rhai"),
        include_str!("../../../scripts/page-turn/back-forward-control.rhai"),
    ] {
        engine
            .compile(script)
            .expect("Rhai syntax only; never execute the measurement");
    }
}
