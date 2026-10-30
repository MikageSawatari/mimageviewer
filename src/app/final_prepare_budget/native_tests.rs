//! The real ROOT finalizer, normal cache and headless renderer share admission.
use super::*;
use crate::gpu_lanczos::{NativeAllocationBytes, NativePrewarmQuery};
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation,
};

fn render_state() -> egui_wgpu::RenderState {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let instance = wgpu::Instance::new(descriptor);
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
    app.view_trim_book_preference = crate::view_trim::ViewTrimBookPreference::None;
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
        stage: PrepareDiagnosticStage::Native,
        position: crate::ui_fullscreen::FinalUnitPosition::Next,
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
    let unit = app.speculative_gpu_frame.canonical[0].next.clone().unwrap();
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
    crate::ui_fullscreen::reset_native_topology_work_counts_for_test();
    capture(&mut app, &ctx, 402);
    app.finalize_speculative_gpu_output(&ctx, 402, 2);
    app.finalize_speculative_gpu_output(&ctx, 402, 2);
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::Available
    );
    assert_eq!(
        crate::ui_fullscreen::native_topology_work_counts_for_test(),
        [0; 5]
    );
}

#[test]
fn d3_4b_activity_and_changed_inputs_do_not_claim_and_quiet_opportunity_resumes() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx);
    deliver(&app, &ctx);
    crate::ui_fullscreen::reset_native_topology_work_counts_for_test();
    capture(&mut app, &ctx, 410);
    app.note_page_turn_gpu_activity();
    app.finalize_speculative_gpu_output(&ctx, 410, 1);
    assert_eq!(app.fs_lanczos_cache.outputs().count(), 0);
    assert_eq!(
        crate::ui_fullscreen::native_topology_work_counts_for_test(),
        [0; 5]
    );
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
    let unit = app.speculative_gpu_frame.canonical[0].next.clone().unwrap();
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
        let unit = app.speculative_gpu_frame.canonical[0].next.clone().unwrap();
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

fn back_current_two(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = ready(ctx);
    app.fullscreen_idx = Some(2);
    let mut params = app.effective_params(3).clone();
    params.post_filter = crate::adjustment::PostFilter::Nearest;
    app.adjustment_page_params.insert(3, params);
    // Direct fixture parameter edits must mirror the real settings handler's
    // invalidation. Keeping the old final would leave ordinary display's cache
    // selection dependent on HashMap iteration instead of the intended source.
    app.final_composite_cache
        .retain(|key, _| key.edit_key.idx != 3);
    app.ensure_final_composite_texture(ctx, 3).unwrap();
    frame(ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    app.prepare_final_demand_topologies();
    app
}

#[test]
fn back_root_waits_for_next_native_then_starts_and_registers_previous_resident_job() {
    let ctx = egui::Context::default();
    let mut app = back_current_two(&ctx);
    let key = app.exact_complete_final_composite_entry(1).unwrap().0;
    app.final_composite_cache.remove(&key);
    // Next Nearest needs no Native. No renderer can accidentally provide a hit.
    app.wgpu_render_state = None;
    capture(&mut app, &ctx, 4100);
    assert_eq!(
        app.next_preparation_state(&ctx, app.speculative_gpu_frame.canonical.first()),
        NextPreparationState::Satisfied
    );
    app.finalize_speculative_gpu_output(&ctx, 4100, 1);
    assert!(
        app.final_effect_pending.contains_key(&key),
        "Previous resident worker starts after all Next stages"
    );
    crate::app::final_prepare::tests::wait_ready(&mut app, 1);
    capture(&mut app, &ctx, 4101);
    app.finalize_speculative_gpu_output(&ctx, 4101, 1);
    assert!(app.exact_complete_final_composite_entry(1).is_some());
    assert_eq!(
        app.speculative_gpu_frame.state,
        RegistrationState::SpeculativeSubmitted
    );
    assert_eq!(app.final_prepare_cpu.snapshot().units, 0);
}

#[test]
fn back_next_native_pending_and_unknown_block_previous_cpu_and_registration() {
    for prepared in [false, true] {
        for unknown in [false, true] {
            let ctx = egui::Context::default();
            let mut app = back_current_two(&ctx);
            let key = app.exact_complete_final_composite_entry(1).unwrap().0;
            app.final_composite_cache.remove(&key);
            if prepared {
                assert!(crate::app::final_prepare::tests::start(&mut app, &ctx, 1));
                crate::app::final_prepare::tests::wait_ready(&mut app, 1);
            }
            // Next final is ready, but normal resampling has no backing (Pending),
            // or its resident rotation evidence is missing (Unknown).
            app.adjustment_page_params.get_mut(&3).unwrap().post_filter =
                crate::adjustment::PostFilter::None;
            app.final_composite_cache
                .retain(|key, _| key.edit_key.idx != 3);
            app.ensure_final_composite_texture(&ctx, 3).unwrap();
            if unknown {
                app.rotation_cache.remove(&3);
            }
            capture(&mut app, &ctx, 4200);
            app.finalize_speculative_gpu_output(&ctx, 4200, 1);
            assert!(!app.final_composite_cache.contains_key(&key));
            assert_eq!(app.final_effect_pending.contains_key(&key), prepared);
            assert_eq!(
                app.speculative_gpu_frame.state,
                RegistrationState::Available
            );
        }
    }
}

#[test]
fn back_previous_ready_is_taken_by_display_on_direction_reversal_without_slot() {
    let ctx = egui::Context::default();
    let mut app = back_current_two(&ctx);
    let key = app.exact_complete_final_composite_entry(1).unwrap().0;
    app.final_composite_cache.remove(&key);
    assert!(crate::app::final_prepare::tests::start(&mut app, &ctx, 1));
    crate::app::final_prepare::tests::wait_ready(&mut app, 1);
    let old_current = app
        .exact_complete_final_composite_entry(2)
        .unwrap()
        .1
        .texture
        .id();
    app.fullscreen_idx = Some(1);
    app.speculative_gpu_frame.begin(4300);
    app.speculative_gpu_frame.note_page_turn_activity();
    let texture = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    assert_eq!(
        app.exact_complete_final_composite_entry(1)
            .unwrap()
            .1
            .texture
            .id(),
        texture.id()
    );
    assert_eq!(app.final_prepare_cpu.snapshot().units, 0);
    app.fullscreen_idx = Some(2);
    assert_eq!(
        app.ensure_final_composite_texture(&ctx, 2).unwrap().id(),
        old_current
    );
}

#[cfg(windows)]
#[test]
fn back_all_live_next_precede_previous_in_both_route_orders_and_unknown_sibling() {
    for reverse in [false, true] {
        for unknown in [false, true] {
            let ctx = egui::Context::default();
            let mut app = back_current_two(&ctx);
            let previous = app.exact_complete_final_composite_entry(1).unwrap().0;
            app.final_composite_cache.remove(&previous);
            assert!(crate::app::final_prepare::tests::start(&mut app, &ctx, 1));
            crate::app::final_prepare::tests::wait_ready(&mut app, 1);
            let sibling = super::scheduling_tests::active_owner(&mut app, &ctx, 94420);
            let next = app
                .with_viewer_context_state(sibling, |owner| {
                    let mut params = crate::adjustment::AdjustParams::default();
                    params.brightness = 15.0;
                    owner.adjustment_page_params.insert(1, params);
                    for idx in 0..4 {
                        owner
                            .rotation_cache
                            .insert(idx, crate::rotation_db::Rotation::None);
                    }
                    owner.settings.still_seek_strip_visible = false;
                    owner.prepare_final_demand_topologies();
                    assert!(crate::app::final_prepare::tests::start(owner, &ctx, 1));
                    let key = crate::app::final_prepare::tests::wait_ready(owner, 1);
                    if unknown {
                        owner.rotation_cache.remove(&1);
                    }
                    key
                })
                .unwrap();
            app.speculative_gpu_frame.begin(4400);
            let main = app.projected_viewer_context_id();
            let owners = if reverse {
                [sibling, main]
            } else {
                [main, sibling]
            };
            for owner in owners {
                app.with_viewer_context_state(owner, |owner| {
                    frame(&ctx, |ui, _| {
                        owner.observe_live_paged_for_final_prepare();
                        owner.record_next_canonical_native_demand(ui);
                    });
                })
                .unwrap();
            }
            app.finalize_speculative_gpu_output(&ctx, 4400, 1);
            assert!(!app.final_composite_cache.contains_key(&previous));
            assert!(app.final_effect_pending.contains_key(&previous));
            app.with_viewer_context_state(sibling, |owner| {
                assert!(
                    owner.final_composite_cache.contains_key(&next),
                    "sibling Next final owns the frame first"
                );
            })
            .unwrap();
        }
    }
}

#[test]
fn back_early_service_discards_previous_after_animation_off() {
    let ctx = egui::Context::default();
    let mut app = back_current_two(&ctx);
    let key = app.exact_complete_final_composite_entry(1).unwrap().0;
    app.final_composite_cache.remove(&key);
    assert!(crate::app::final_prepare::tests::start(&mut app, &ctx, 1));
    crate::app::final_prepare::tests::wait_ready(&mut app, 1);
    let cancel = app.final_effect_pending[&key].cancel.clone();
    app.settings.page_turn_animation_enabled = false;
    app.service_final_prepare_before_lookup(&ctx);
    assert!(app.final_effect_pending.is_empty());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!app.final_composite_cache.contains_key(&key));
    assert_eq!(app.final_prepare_cpu.snapshot().units, 0);
}

#[cfg(windows)]
#[test]
fn back_mixed_unknown_pending_siblings_cancel_previous_in_both_visit_orders() {
    use crate::app::final_prepare::tests::{WorkerHold, start, wait_idle, wait_ready};
    for reverse in [false, true] {
        let ctx = egui::Context::default();
        let mut app = back_current_two(&ctx);
        let main = app.projected_viewer_context_id();
        let previous = app.exact_complete_final_composite_entry(1).unwrap().0;
        app.final_composite_cache.remove(&previous);
        let hold = WorkerHold::new();
        assert!(start(&mut app, &ctx, 1));
        hold.entered();
        let cancel = app.final_effect_pending[&previous].cancel.clone();
        app.rotation_cache.remove(&3); // main Next is Unknown
        let held = app.page_turn_gpu_fingerprint_for_test();
        let sibling = super::scheduling_tests::active_owner(&mut app, &ctx, 94421);
        app.with_viewer_context_state(sibling, |owner| {
            for idx in 0..4 {
                owner
                    .rotation_cache
                    .insert(idx, crate::rotation_db::Rotation::None);
            }
            let mut params = owner.effective_params(1).clone();
            params.brightness = 15.0;
            owner.adjustment_page_params.insert(1, params);
            owner.settings.still_seek_strip_visible = false;
            owner.prepare_canonical_chrome_for_test(0);
            owner.prepare_final_demand_topologies();
        })
        .unwrap();
        app.speculative_gpu_frame.begin(4500);
        let owners = if reverse {
            [sibling, main]
        } else {
            [main, sibling]
        };
        for id in owners {
            app.with_viewer_context_state(id, |owner| {
                frame(&ctx, |ui, _| {
                    owner.observe_live_paged_for_final_prepare();
                    owner.record_next_canonical_native_demand(ui);
                });
            })
            .unwrap();
        }
        let observations = app.speculative_gpu_frame.canonical.clone();
        for id in [main, sibling] {
            let observation = observations
                .iter()
                .find(|row| row.destination.context == id)
                .unwrap();
            app.with_viewer_context_state(id, |owner| {
                let state = owner.next_preparation_state(&ctx, Some(observation));
                if id == main {
                    assert!(
                        matches!(state, NextPreparationState::Unknown(_)),
                        "{state:?}"
                    );
                } else {
                    assert_eq!(state, NextPreparationState::Pending("final_pending"));
                }
            })
            .unwrap();
        }
        app.finalize_speculative_gpu_output(&ctx, 4500, 1);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!app.final_effect_pending.contains_key(&previous));
        assert_eq!(app.page_turn_gpu_fingerprint_for_test(), held);
        assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
        assert!(app.final_prepare_cpu.snapshot().bytes > 0);
        app.with_viewer_context_state(sibling, |owner| {
            owner.prefetch_next_final_unit(&ctx, 0);
            assert!(
                owner.final_effect_pending.is_empty(),
                "canceled worker still owns the real permit"
            );
        })
        .unwrap();
        hold.release();
        wait_idle(&app);
        assert_eq!(app.final_prepare_cpu.snapshot().units, 0);
        app.with_viewer_context_state(sibling, |owner| {
            owner.prefetch_next_final_unit(&ctx, 0);
            wait_ready(owner, 1);
        })
        .unwrap();
    }
}
