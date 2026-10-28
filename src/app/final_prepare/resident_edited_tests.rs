//! E3: completed cumulative edit inputs, with the ordinary stage owners intact.
use super::tests::{WorkerHold, app, begin_registration_frame, start, wait_idle, wait_ready};
use super::*;

fn local_layers(app: &mut App, idx: usize, active: bool) {
    let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
        "resident-input",
        local_adjust_core::LocalMask::Full,
        local_adjust_core::LocalEffect::None,
    );
    layer.enabled = active;
    app.local_adjust_pages.insert(idx);
    app.local_adjust_page_layers.insert(
        idx,
        local_adjust_core::LocalAdjustmentLayers::new(vec![layer]),
    );
}

pub(super) fn stage(
    app: &mut App,
    ctx: &egui::Context,
    idx: usize,
    name: &str,
) -> Arc<egui::ColorImage> {
    let pixels = Arc::new(egui::ColorImage::filled([12, 18], egui::Color32::GOLD));
    let texture = ctx.load_texture(name, pixels.clone(), DISPLAY_IMAGE_TEXTURE_OPTIONS);
    app.bind_display_texture_identity(&texture, app.item_id(idx));
    let display_basis =
        app.accept_current_producer_display_basis(idx, pixels.size, pixels.size, false);
    match name {
        "erase" => {
            app.mask_pages.insert(idx);
            app.erase_result_cache.insert(
                app.current_erase_result_key(idx),
                EraseResultCacheEntry {
                    texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    display_basis,
                    pixels: pixels.clone(),
                    texture,
                },
            );
        }
        "local" => {
            local_layers(app, idx, true);
            app.local_adjust_cache.insert(
                app.current_local_adjust_key(idx),
                LocalAdjustCacheEntry {
                    texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    display_basis,
                    pixels: pixels.clone(),
                    texture: Some(texture),
                },
            );
        }
        "conceal" => {
            app.conceal_pages.insert(idx);
            app.conceal_cache.insert(
                idx,
                ConcealCacheEntry {
                    texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    display_basis,
                    pixels: pixels.clone(),
                    texture: Some(texture),
                    generation: app.conceal_generation,
                },
            );
        }
        _ => unreachable!(),
    }
    pixels
}

fn after_ai(app: &mut App, idx: usize) -> Arc<egui::ColorImage> {
    let mut params = app.effective_params(idx).clone();
    params.upscale_model = Some("realesr_general_v3".to_owned());
    app.adjustment_page_params.insert(idx, params.clone());
    let key = app
        .final_ai_key_for_pixels(app.current_edit_result_key(idx), [12, 18], &params)
        .unwrap();
    let pixels = Arc::new(egui::ColorImage::filled([24, 36], egui::Color32::GREEN));
    app.final_ai_cache.insert(
        key,
        FinalAiEntry {
            canonical_dims: None,
            source_size: [12, 18],
            pixels: pixels.clone(),
            used_upscale: true,
        },
    );
    pixels
}

fn producer(app: &mut App, idx: usize, name: &str, ready: bool) {
    if name == "erase" {
        use crate::ui_erase::{
            EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey, EraseInpaintProgress,
            EraseInpaintPurpose, EraseInpaintResult,
        };
        let key = app.current_erase_result_key(idx);
        let (tx, rx) = mpsc::channel();
        if ready {
            tx.send(EraseInpaintResult::MaskAbsent).unwrap();
        }
        let (_, progress_rx) = mpsc::channel();
        app.erase_inpaint_pending.insert(
            EraseInpaintPendingKey {
                idx,
                kind: EraseInpaintKind::Commit,
            },
            EraseInpaintPending {
                display_basis: app.accept_current_producer_display_basis(
                    idx,
                    [12, 18],
                    [12, 18],
                    false,
                ),
                idx,
                items_generation: app.items_generation,
                path_key: app.page_path_key(idx),
                rx,
                progress_rx,
                progress: EraseInpaintProgress::Preparing,
                cancel: Arc::new(AtomicBool::new(false)),
                started_at: std::time::Instant::now(),
                input_generation: key.input_gen,
                mask_generation: key.mask_gen,
                purpose: EraseInpaintPurpose::Commit { origin: "e3-test" },
            },
        );
    } else {
        let request = EditMaterializeRequest {
            items_generation: app.items_generation,
            input_seq: app.input_seq,
            kind: if name == "local" {
                EditMaterializeKind::Local {
                    key: app.current_local_adjust_key(idx),
                    continuation: LocalMaterializeContinuation::Display,
                }
            } else {
                EditMaterializeKind::Conceal {
                    key: app.current_edit_result_key(idx),
                    purpose: ConcealMaterializePurpose::Display,
                }
            },
        };
        let (_, rx) = mpsc::channel();
        app.local_adjust_pending.insert(
            idx,
            LocalAdjustRenderPending {
                request,
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
                ready: ready.then(|| EditMaterializeResult::Ready {
                    request,
                    payload: EditMaterializePayload::LocalInactive {
                        layers: local_adjust_core::LocalAdjustmentLayers::new(vec![]),
                    },
                }),
            },
        );
    }
}

#[test]
fn e3_highest_complete_cumulative_stage_needs_no_lower_cache() {
    for highest in ["erase", "local", "conceal"] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        app.mask_pages.insert(1);
        if highest != "erase" {
            local_layers(&mut app, 1, true);
        }
        let pixels = stage(&mut app, &ctx, 1, highest);
        app.fs_cache.remove(&1);
        let key = app.current_edit_result_key(1);
        let (final_key, input, size, plan) = app.final_prepare_plan_from_resident(1).unwrap();
        assert_eq!(final_key.edit_key, key);
        assert_eq!(size, pixels.size);
        assert!(Arc::ptr_eq(&input, &pixels));
        assert!(plan.adjust_before_effect.is_some());
        assert!(app.edit_result_cache.is_empty());
        assert!(app.local_adjust_pending.is_empty());
        assert!(start(&mut app, &ctx, 1));
        let pending_key = wait_ready(&mut app, 1);
        assert_eq!(pending_key, final_key);
        assert_eq!(app.final_effect_pending[&final_key].source_size, [12, 18]);
        // No stage materialization was introduced by the final worker.
        assert!(app.edit_result_cache.is_empty());
        assert!(app.local_adjust_pending.is_empty());
        app.cancel_final_effects_for_idx(1);
    }
}

#[test]
fn e3_missing_upper_or_unhydrated_local_never_promotes_lower_fallback() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let erase = stage(&mut app, &ctx, 1, "erase");
    app.local_adjust_pages.insert(1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    local_layers(&mut app, 1, true);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    let _ = stage(&mut app, &ctx, 1, "local");
    app.conceal_pages.insert(1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    let _ = stage(&mut app, &ctx, 1, "conceal");
    app.conceal_cache.get_mut(&1).unwrap().generation += 1;
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    app.conceal_pages.remove(&1);
    app.conceal_cache.clear();
    local_layers(&mut app, 1, false);
    app.local_adjust_cache.clear();
    let (_, pixels, _, _) = app.final_prepare_plan_from_resident(1).unwrap();
    assert!(
        Arc::ptr_eq(&erase, &pixels),
        "hydrated disabled layers need no stage"
    );
    app.mask_pages.remove(&1);
    assert!(matches!(
        app.resident_final_prepare_input(1),
        Some(ResidentFinalPrepareInput::Unedited { .. })
    ));
    app.local_adjust_page_layers.remove(&1);
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    assert!(app.local_adjust_pending.is_empty());
}

#[test]
fn e3_erase_only_without_final_effect_keeps_cpu_early_return_and_stage_material() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.adjustment_page_params.remove(&1);
    let pixels = stage(&mut app, &ctx, 1, "erase");
    let texture = app.erase_result_cache[&app.current_erase_result_key(1)]
        .texture
        .id();
    assert!(app.final_prepare_plan_from_resident(1).is_none());
    let resource = app.resident_final_prepare_material(&ctx, 1).unwrap();
    assert_eq!(resource.source_texture().id(), texture);
    assert!(app.edit_result_cache.is_empty());
    assert_eq!(pixels.size, [12, 18]);
}

#[test]
fn e3_each_current_producer_and_unadopted_ready_blocks_every_plan_branch() {
    for ai in [false, true] {
        for name in ["erase", "local", "conceal"] {
            for ready in [false, true] {
                let ctx = egui::Context::default();
                let mut app = app(&ctx);
                stage(&mut app, &ctx, 1, "erase");
                if ai {
                    after_ai(&mut app, 1);
                }
                assert!(app.final_prepare_plan_from_resident(1).is_some());
                producer(&mut app, 1, name, ready);
                assert!(
                    app.final_prepare_plan_from_resident(1).is_none(),
                    "{name} ready={ready} ai={ai}"
                );
                assert!(!start(&mut app, &ctx, 1));
                assert!(app.final_effect_pending.is_empty());
                assert_eq!(app.final_prepare_cpu.snapshot().running, 0);
                if name == "erase" {
                    app.erase_inpaint_pending.clear();
                } else {
                    app.local_adjust_pending.clear();
                }
                assert!(app.final_prepare_plan_from_resident(1).is_some());
            }
        }
    }
}

#[test]
fn e3_obsolete_or_cancelled_producer_is_not_a_current_stage_wait() {
    for name in ["erase", "local", "conceal"] {
        for cancelled in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            stage(&mut app, &ctx, 1, "erase");
            producer(&mut app, 1, name, true);
            if name == "erase" {
                let pending = app.erase_inpaint_pending.values_mut().next().unwrap();
                if cancelled {
                    pending.cancel.store(true, Ordering::Relaxed);
                } else {
                    pending.items_generation += 1;
                }
            } else {
                let pending = app.local_adjust_pending.get_mut(&1).unwrap();
                if cancelled {
                    pending.cancel.store(true, Ordering::Relaxed);
                } else {
                    pending.request.items_generation += 1;
                }
            }
            assert!(
                app.final_prepare_plan_from_resident(1).is_some(),
                "{name} cancelled={cancelled}"
            );
        }
    }
}

#[test]
fn e3_previous_uses_the_same_resident_plan_without_starting_edit_producers() {
    use crate::ui_fullscreen::page_turn_submission_tests::{
        draw_current_without_sources, frame, install_presentation,
    };
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    app.fullscreen_idx = Some(2);
    let _ = app.page_turn_prefetch_targets(2, 2, 2);
    frame(&ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    assert!(app.page_turn_current_unit_presented());
    app.prepare_final_demand_topologies();
    assert_eq!(
        app.d3_final_prepare_unit_at(2, FinalUnitPosition::Previous)
            .unwrap()
            .pages[0]
            .idx,
        1
    );
    stage(&mut app, &ctx, 1, "erase");
    app.local_adjust_pages.insert(1);
    assert!(!app.prefetch_previous_final_unit(&ctx));
    assert!(app.local_adjust_pending.is_empty());
    assert!(app.final_effect_pending.is_empty());
    local_layers(&mut app, 1, false);
    let (key, input, size, _) = app.final_prepare_plan_from_resident(1).unwrap();
    assert_eq!(size, input.size);
    assert!(app.prefetch_previous_final_unit(&ctx));
    assert_eq!(wait_ready(&mut app, 1), key);
    assert_eq!(app.final_effect_pending[&key].source_size, size);
    assert!(app.local_adjust_pending.is_empty());
    assert!(app.edit_result_cache.keys().all(|key| key.idx != 1));
    app.cancel_final_effects_for_idx(1);
}

#[test]
fn e3_after_ai_freezes_pre_ai_size_and_never_reapplies_tone() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let pixels = after_ai(&mut app, 1);
    app.fs_cache.remove(&1);
    let (key, input, size, plan) = app.final_prepare_plan_from_resident(1).unwrap();
    assert!(Arc::ptr_eq(&pixels, &input));
    assert_eq!(size, [12, 18]);
    assert_eq!(
        key,
        app.final_composite_key_for_pixels(
            app.current_edit_result_key(1),
            size,
            app.effective_params(1)
        )
    );
    assert!(plan.adjust_before_effect.is_none());
    assert!(start(&mut app, &ctx, 1));
    assert_eq!(wait_ready(&mut app, 1), key);
    assert_eq!(app.final_effect_pending[&key].source_size, size);
    app.cancel_final_effects_for_idx(1);
}

#[test]
fn e3_waiting_spread_page_does_not_block_resident_partner() {
    use crate::ui_fullscreen::page_turn_submission_tests::{
        draw_current_without_sources, frame, install_presentation,
    };
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    app.spread_mode = crate::settings::SpreadMode::Rtl;
    let _ = app.page_turn_prefetch_targets(0, 2, 2);
    frame(&ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx)
    });
    app.prepare_final_demand_topologies();
    let next = app.d3_final_prepare_unit(0).unwrap();
    assert_eq!(next.pages.len(), 2);
    let waiting = next.pages[0].idx;
    let partner = next.pages[1].idx;
    stage(&mut app, &ctx, waiting, "erase");
    stage(&mut app, &ctx, partner, "erase");
    producer(&mut app, waiting, "local", true);
    app.prefetch_next_final_unit(&ctx, 0);
    let key = wait_ready(&mut app, partner);
    assert_eq!(app.final_effect_pending.len(), 1);
    assert_eq!(key.edit_key.idx, partner);
    assert!(app.local_adjust_pending[&waiting].ready.is_some());
    app.cancel_final_effects_for_idx(partner);
}

#[test]
fn e3_same_key_legacy_prefetch_does_not_ensure_edit_inputs_again() {
    for ai in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        stage(&mut app, &ctx, 1, "erase");
        app.adjustment_page_params
            .get_mut(&1)
            .unwrap()
            .colorize
            .mode = crate::colorize::ColorizeMode::AllImages;
        if ai {
            after_ai(&mut app, 1);
        }
        assert!(start(&mut app, &ctx, 1));
        let key = wait_ready(&mut app, 1);
        // Also cover the dimensions fallback retained by the D3 request.
        app.fs_cache.remove(&1);
        assert_eq!(
            app.final_ai_prefetch_input_size(1, app.effective_params(1)),
            Some([12, 18])
        );
        app.prefetch_final_effects(&ctx, 0, None);
        app.prefetch_final_ai(&ctx, 0);
        assert!(app.edit_result_cache.is_empty());
        assert!(app.local_adjust_pending.is_empty());
        assert!(app.fs_pending.is_empty());
        assert_eq!(app.final_effect_pending.len(), 1);
        assert!(app.final_effect_pending.contains_key(&key));
        app.cancel_final_effects_for_idx(1);
    }
}

#[test]
fn e3_later_same_key_inactive_normalization_invalidates_ready_and_computing_jobs() {
    for ready in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        stage(&mut app, &ctx, 1, "erase");
        local_layers(&mut app, 1, false);
        let edit_key = app.current_edit_result_key(1);
        let hold = (!ready).then(WorkerHold::new);
        assert!(start(&mut app, &ctx, 1));
        let key = if let Some(hold) = &hold {
            hold.entered();
            *app.final_effect_pending.keys().next().unwrap()
        } else {
            wait_ready(&mut app, 1)
        };
        let cancel = app.final_effect_pending[&key].cancel.clone();
        let request = EditMaterializeRequest {
            items_generation: app.items_generation,
            input_seq: app.input_seq,
            kind: EditMaterializeKind::Local {
                key: app.current_local_adjust_key(1),
                continuation: LocalMaterializeContinuation::Display,
            },
        };
        let result = EditMaterializeResult::Ready {
            request,
            payload: EditMaterializePayload::LocalInactive {
                layers: app.local_adjust_page_layers[&1].clone(),
            },
        };
        assert!(app.apply_edit_materialize_result(&ctx, 1, result, request));
        assert_eq!(
            edit_key,
            app.current_edit_result_key(1),
            "normalization need not advance generations"
        );
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.final_effect_pending.is_empty());
        if let Some(hold) = &hold {
            hold.release();
        }
        wait_idle(&app);
        begin_registration_frame(&mut app, 100);
        app.poll_final_effects(&ctx);
        assert!(!app.final_composite_cache.contains_key(&key));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}
