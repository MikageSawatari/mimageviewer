//! The real ROOT finalizer, normal cache and headless renderer share admission.
use super::*;
use crate::gpu_lanczos::{NativeAllocationBytes, NativePrewarmQuery};
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation,
};

fn render_state() -> egui_wgpu::RenderState {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
    egui_wgpu::RenderState {
        instance,
        adapter,
        available_adapters: Vec::new(),
        device,
        queue,
        target_format,
        renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
    }
}

fn ready(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    ready_for_mode(ctx, crate::settings::SpreadMode::Single)
}

fn ready_for_mode(
    ctx: &egui::Context,
    mode: crate::settings::SpreadMode,
) -> crate::app::AppTestEnvForTest {
    install_presentation(ctx);
    let mut app = crate::app::final_prepare::tests::app(ctx);
    app.spread_mode = mode;
    app.settings.fullscreen_fit_mode = crate::settings::FullscreenFitMode::Page;
    app.settings.margin_fit_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.view_trim_apply_mode = crate::view_trim::ViewTrimApplyMode::None;
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.ensure_final_composite_texture(ctx, idx).unwrap();
    }
    frame(ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    assert!(app.page_turn_current_unit_presented());
    app.prepare_canonical_chrome_for_test(0);
    app.wgpu_render_state = Some(render_state());
    app
}

fn capture(app: &mut App, ctx: &egui::Context, number: u64) {
    app.speculative_gpu_frame.begin(number);
    app.speculative_gpu_frame.begin_pass();
    frame(ctx, |ui, _| {
        app.observe_live_paged_for_final_prepare();
        app.record_next_canonical_native_demand(ui);
    });
    assert_eq!(
        app.speculative_gpu_frame.canonical.len(),
        1,
        "frame={number} presented={} destination={:?} presentation={}",
        app.page_turn_current_unit_presented(),
        app.page_turn_destination(),
        app.page_turn_gpu_fingerprint_for_test()
    );
}

fn deliver(app: &App, ctx: &egui::Context) {
    let rs = app.wgpu_render_state.as_ref().unwrap();
    for idx in 0..4 {
        let texture = app.resident_final_prepare_texture(ctx, idx).unwrap();
        let delta = egui::epaint::ImageDelta::full(
            egui::ColorImage::filled(texture.size(), egui::Color32::WHITE),
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        rs.renderer
            .write()
            .update_texture(&rs.device, &rs.queue, texture.id(), &delta);
    }
}

#[test]
fn d3_4b_native_admission_includes_output_and_temporary_without_overflow() {
    let allocation = NativeAllocationBytes {
        output_bytes: 40,
        temporary_bytes: 80,
    };
    assert!(speculative_native_vram_admitted(100, allocation, Some(220)));
    assert!(!speculative_native_vram_admitted(
        100,
        allocation,
        Some(219)
    ));
    assert!(!speculative_native_vram_admitted(
        u64::MAX - 10,
        allocation,
        Some(u64::MAX)
    ));
}

#[test]
fn d3_4b_native_diagnostics_deduplicate_both_pages_until_unit_identity_changes() {
    let ctx = egui::Context::default();
    let app = crate::app::final_prepare::tests::app(&ctx);
    let mut diagnostics = NativeGpuDiagnostics {
        destination: app.page_turn_destination(),
        identity: String::new(),
        reasons: Vec::new(),
    };
    for opportunity in 0..3 {
        assert_eq!(
            diagnostics.first("pair: left-key/right-key", "missing_backing"),
            opportunity == 0
        );
        assert_eq!(
            diagnostics.first("pair: left-key/right-key", "vram_admission"),
            opportunity == 0
        );
    }
    assert!(diagnostics.first("pair: new-left-key/right-key", "missing_backing"));
    assert_eq!(diagnostics.reasons.len(), 1);
    let mut frame = SpeculativeGpuFrame::default();
    frame.begin(1);
    frame.observe(diagnostics.destination);
    frame.native_diagnostics.push(diagnostics);
    frame.begin(2);
    assert_eq!(frame.native_diagnostics.len(), 1);
    frame.begin(3);
    assert!(
        frame.native_diagnostics.is_empty(),
        "retired routes need no native hook to retire diagnostic values"
    );
}

#[test]
fn d3_4b_root_hook_waits_for_backing_without_claim_then_normal_prepare_hits() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    capture(&mut app, &ctx, 400);
    let unit = app.speculative_gpu_frame.canonical[0].clone();
    app.finalize_speculative_gpu_output(&ctx, 400, 1);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::Available
    );
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);
    deliver(&app, &ctx);
    capture(&mut app, &ctx, 401);
    app.finalize_speculative_gpu_output(&ctx, 401, 1);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::SpeculativeSubmitted
    );
    let page = &unit.pages[0];
    let resource = app
        .canonical_demand_resource_if_current(&ctx, &unit, page)
        .unwrap();
    assert!(matches!(
        app.fs_lanczos_cache.query_prewarm(
            app.wgpu_render_state.as_ref(),
            &resource,
            page.transform.total_scale,
            page.transform.pixels_per_point,
            page.visible_region,
            page.post_filter,
            page.smoothing_percent,
            page.anime_source_limit,
        ),
        NativePrewarmQuery::Hit(_)
    ));
    let prepared = app.prepare_fullscreen_paint_resource(
        &resource,
        page.transform.total_scale,
        page.transform.pixels_per_point,
        page.visible_region,
    );
    assert!(prepared.lanczos_output().is_some());
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 1);
    let accounted = app.vram_accounting_snapshot();
    assert_eq!(
        accounted.subsystem_texels(crate::vram_budget::VramSubsystem::LanczosOutputs),
        prepared
            .lanczos_output()
            .unwrap()
            .size()
            .into_iter()
            .product::<usize>()
    );
    // Hits neither allocate nor consume the next frame, even on repeated hooks.
    capture(&mut app, &ctx, 402);
    app.finalize_speculative_gpu_output(&ctx, 402, 2);
    app.finalize_speculative_gpu_output(&ctx, 402, 2);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::Available
    );
}

#[test]
fn d3_4b_activity_and_changed_inputs_do_not_claim_and_quiet_opportunity_resumes() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    capture(&mut app, &ctx, 410);
    app.note_page_turn_gpu_activity();
    app.finalize_speculative_gpu_output(&ctx, 410, 1);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);
    capture(&mut app, &ctx, 411);
    let old = app.effective_params(1).clone();
    let mut changed = old.clone();
    changed.brightness += 1.0;
    app.adjustment_page_params.insert(1, changed.clone());
    app.clear_caches_for_param_change(1, &old, &changed);
    app.finalize_speculative_gpu_output(&ctx, 411, 1);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::Available
    );
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);
    app.ensure_final_composite_texture(&ctx, 1).unwrap();
    deliver(&app, &ctx);
    capture(&mut app, &ctx, 412);
    app.finalize_speculative_gpu_output(&ctx, 412, 1);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 1);
}

#[cfg(windows)]
#[test]
fn d3_4b_remote_acquisition_invalidates_collected_native_demand_without_claim() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    capture(&mut app, &ctx, 460);
    let unit = app.speculative_gpu_frame.canonical[0].clone();
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    let _ = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "d3-4b-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("test".into()),
        },
    });
    assert!(app.remote_session_blocks_local_control());
    assert!(!app.canonical_unit_demand_is_current(&ctx, &unit));
    app.finalize_speculative_gpu_output(&ctx, 460, 1);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::Available
    );
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);

    assert!(handle.finish_acquire(handle.snapshot().generation));
    handle.local_disconnect();
    assert!(handle.complete_app_drain(handle.snapshot().generation));
    assert!(!app.remote_session_blocks_local_control());
    capture(&mut app, &ctx, 461);
    app.finalize_speculative_gpu_output(&ctx, 461, 1);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 1);
}

#[test]
fn d3_4b_pair_native_misses_share_one_slot_and_converge_in_two_opportunities() {
    let ctx = egui::Context::default();
    let mut app = ready_for_mode(&ctx, crate::settings::SpreadMode::Ltr);
    assert!(app.page_turn_current_unit_presented());
    deliver(&app, &ctx);
    for (number, expected) in [(450, 1), (451, 2), (452, 2)] {
        capture(&mut app, &ctx, number);
        let unit = app.speculative_gpu_frame.canonical[0].clone();
        assert_eq!(unit.pages.len(), 2);
        app.finalize_speculative_gpu_output(&ctx, number, 1);
        assert_eq!(app.fs_lanczos_cache.outputs().count(), expected);
        assert_eq!(
            app.speculative_gpu_frame.state,
            if number == 452 {
                RegistrationState::Available
            } else {
                RegistrationState::SpeculativeSubmitted
            }
        );
    }
}

#[test]
fn d3_4b_retry_discards_candidate_values_without_refunding_activity() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    capture(&mut app, &ctx, 420);
    app.note_page_turn_gpu_activity();
    app.speculative_gpu_frame.begin_pass();
    assert!(app.speculative_gpu_frame.canonical.is_empty());
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::PageTurnActivity
    );
    app.finalize_speculative_gpu_output(&ctx, 420, 2);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);
    capture(&mut app, &ctx, 421);
    app.finalize_speculative_gpu_output(&ctx, 421, 1);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 1);
}

#[cfg(windows)]
#[test]
fn d3_4b_live_sibling_blocks_native_and_park_preserves_material_while_b_resumes() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    let id = super::scheduling_tests::active_owner(&mut app, &ctx, 94_501);
    // Building/activating the other viewer changes the live dispatch setup.
    // Model main's real draw before observing its next-unit demand again.
    frame(&ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    assert!(app.page_turn_current_unit_presented());
    app.with_active_viewer_context(|owner| {
        super::scheduling_tests::set_presentation(owner, &ctx, "turning");
    })
    .unwrap();
    capture(&mut app, &ctx, 430);
    app.with_active_viewer_context(|owner| owner.observe_live_paged_for_final_prepare())
        .unwrap();
    app.finalize_speculative_gpu_output(&ctx, 430, 1);
    assert!(
        app.fs_lanczos_cache
            .outputs()
            .all(|(source, _)| source != crate::gpu_lanczos::FullscreenPaintSourceId::Page(1))
    );
    assert!(app.pause_current_active_viewer_context(&ctx));
    let parked = app
        .with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
        .unwrap();
    // Park returns main to its normal live dispatch. Adopt that actual output
    // before asking to prepare the next unit for this resumed presentation.
    frame(&ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    assert!(app.page_turn_current_unit_presented());
    capture(&mut app, &ctx, 431);
    app.finalize_speculative_gpu_output(&ctx, 431, 1);
    let output = app
        .fs_lanczos_cache
        .outputs()
        .find(|(source, _)| *source == crate::gpu_lanczos::FullscreenPaintSourceId::Page(1))
        .expect("B's next page must be prepared after A parks")
        .1
        .clone();
    let snapshot = app
        .detached_image_windows
        .iter()
        .find(|window| window.id == 94_501)
        .unwrap();
    let view = app.deferred_detached_image_window_view(
        snapshot,
        app.detached_viewer_window_placement(),
        false,
    );
    let painted = frame(&ctx, |ui, _| {
        App::draw_detached_snapshot_for_gpu_test(ui, ui.max_rect(), &view)
    });
    assert!(!painted.shapes.is_empty());
    assert_eq!(
        app.with_viewer_context(id, |owner| owner.page_turn_gpu_fingerprint_for_test())
            .unwrap(),
        parked
    );
    assert_eq!(
        app.with_viewer_context(id, |owner| owner.fs_lanczos_cache.outputs().count())
            .unwrap(),
        0
    );
    let main_generation = app.items_generation;
    app.retire_context(id, "d3-4b-sibling-close", |_| ())
        .unwrap();
    assert_eq!(app.items_generation, main_generation);
    assert!(Arc::ptr_eq(
        app.fs_lanczos_cache
            .outputs()
            .find(|(source, _)| *source == crate::gpu_lanczos::FullscreenPaintSourceId::Page(1))
            .unwrap()
            .1,
        &output
    ));
}

#[cfg(windows)]
#[test]
fn d3_4b_all_contexts_final_registration_precedes_first_context_native_miss() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    super::scheduling_tests::active_owner(&mut app, &ctx, 94_502);
    frame(&ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    assert!(app.page_turn_current_unit_presented());
    let final_key = app
        .with_active_viewer_context(|owner| {
            let mut params = owner.effective_params(1).clone();
            params.brightness = 15.0;
            owner.adjustment_page_params.insert(1, params);
            assert!(crate::app::final_prepare::tests::start(owner, &ctx, 1));
            crate::app::final_prepare::tests::wait_ready(owner, 1)
        })
        .unwrap();
    capture(&mut app, &ctx, 440); // main's Native candidate is visited first
    app.with_active_viewer_context(|owner| owner.observe_live_paged_for_final_prepare())
        .unwrap();
    app.finalize_speculative_gpu_output(&ctx, 440, 1);
    assert!(
        app.with_active_viewer_context(|owner| owner
            .final_composite_cache
            .contains_key(&final_key))
            .unwrap()
    );
    assert!(
        app.fs_lanczos_cache
            .outputs()
            .all(|(source, _)| source != crate::gpu_lanczos::FullscreenPaintSourceId::Page(1))
    );
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::SpeculativeSubmitted
    );
}
