//! Windows context transitions with real D3 workers and their existing owners.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation,
};

fn seed_owner(owner: &mut App, ctx: &egui::Context, generation: u64) {
    owner.items = (0..4)
        .map(|idx| {
            GridItem::Image(PathBuf::from(format!(
                "C:/d3-lifecycle/{generation}/{idx}.png"
            )))
        })
        .collect();
    owner.items_generation = generation;
    owner.visible_indices = vec![0, 1, 2, 3];
    owner.details_order = owner.visible_indices.clone();
    owner.thumbnails = vec![ThumbnailState::Pending; 4];
    owner.image_metas = vec![None; 4];
    owner.fullscreen_idx = Some(0);
    owner.spread_mode = crate::settings::SpreadMode::Single;
    owner.reading_flow = crate::settings::ReadingFlow::Paged;
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(
            [80, 120],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("d3-lifecycle-{generation}-{idx}"),
            pixels.clone(),
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        owner.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        owner.record_page_dims_for_spread(idx, (80, 120));
        if idx != 0 {
            let mut params = crate::adjustment::AdjustParams::default();
            params.brightness = 15.0;
            owner.adjustment_page_params.insert(idx, params);
        }
    }
    let _ = owner.page_turn_prefetch_targets(0, 2, 2);
}

fn active_owner(app: &mut App, ctx: &egui::Context, window: u64) -> ViewerContextId {
    let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
        seed_owner(owner, ctx, 71);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.set_detached_window_binding_for_test(Some(window));
    });
    app.with_active_viewer_context(|owner| {
        owner.activate_mounted_as_independent_detached(window);
        frame(ctx, |ui, ctx| {
            draw_current_without_sources(owner, ui, ctx);
        });
        assert!(owner.page_turn_current_unit_presented());
    })
    .unwrap();
    id
}

fn pending_key(app: &App, idx: usize) -> FinalCompositeKey {
    *app.final_effect_pending
        .keys()
        .find(|key| key.edit_key.idx == idx)
        .expect("pending owner")
}

fn reservation_ptr(pending: &FinalEffectPending) -> usize {
    match &pending.state {
        FinalEffectState::Computing(FinalEffectReceiver::Prepared { _reservation, .. }) => {
            Arc::as_ptr(_reservation) as usize
        }
        FinalEffectState::CpuReady(FinalEffectReady::Prepared(ready)) => {
            Arc::as_ptr(&ready.reservation) as usize
        }
        _ => panic!("D3 reservation required"),
    }
}

#[test]
fn d3_2_worker_finishes_while_really_parked_and_resume_adopts_before_lookup() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    let mut app = app(&ctx);
    let id = active_owner(&mut app, &ctx, 94_101);
    let hold = WorkerHold::new();
    let (key, cancel, reservation) = app
        .with_active_viewer_context(|owner| {
            assert!(start(owner, &ctx, 1));
            let key = pending_key(owner, 1);
            (
                key,
                owner.final_effect_pending[&key].cancel.clone(),
                reservation_ptr(&owner.final_effect_pending[&key]),
            )
        })
        .unwrap();
    hold.entered();
    let before = app.final_prepare_cpu.snapshot();
    assert_eq!(before.running, 1);
    assert!(app.pause_current_active_viewer_context(&ctx));
    assert!(app.active_viewer_context_id().is_none());
    assert_eq!(app.viewer_context_residence(id), ContextResidence::AtRest);
    assert_eq!(
        app.detached_window_state(94_101),
        Some(DetachedWindowState::Parked)
    );
    assert_eq!(app.final_prepare_cpu.snapshot(), before);
    assert!(!cancel.load(Ordering::Relaxed));

    hold.release();
    wait_idle(&app); // Completion happens while no owner is mounted or serviced.
    let ready_budget = app.final_prepare_cpu.snapshot();
    assert_eq!(ready_budget.running, 0);
    assert_eq!(ready_budget.units, before.units);
    assert_eq!(ready_budget.bytes, before.bytes);
    for _ in 0..2 {
        app.with_viewer_context(id, |owner| {
            let pending = &owner.final_effect_pending[&key];
            assert!(Arc::ptr_eq(&pending.cancel, &cancel));
            assert_eq!(reservation_ptr(pending), reservation);
            assert!(
                matches!(
                    pending.state,
                    FinalEffectState::Computing(FinalEffectReceiver::Prepared { .. })
                ),
                "mount must leave the channel unread"
            );
            assert_eq!(
                owner.final_effect_pending.len(),
                1,
                "mount cannot start the next stage/job"
            );
            assert!(!owner.final_composite_cache.contains_key(&key));
        })
        .unwrap();
    }
    assert_eq!(app.final_prepare_cpu.snapshot(), ready_budget);
    begin_registration_frame(&mut app, 100);
    assert!(app.activate_detached_image_window_snapshot(&ctx, 94_101));
    app.with_active_viewer_context(|owner| {
        // Resume receives CpuReady, but D3-4(a) reserves new speculative
        // allocation for finalized output. A display request still takes it now.
        assert!(owner.final_effect_pending.contains_key(&key));
        assert!(owner.ensure_final_composite_texture(&ctx, 1).is_some());
        assert!(owner.final_composite_cache.contains_key(&key));
        crate::app::final_prepare_tests::expect_adopted_before_lookup(key, || {
            assert!(owner.resolve_fs_display_tex(1, false).is_some());
        });
        assert!(owner.exact_complete_final_composite_texture(2).is_none());
    })
    .unwrap();
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(app.final_prepare_cpu.snapshot().units, 0);
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    assert!(
        app.final_effect_pending.is_empty(),
        "main receives no sibling result"
    );
}

#[test]
fn d3_2_legacy_colorize_and_lut_computing_channel_survives_real_park() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        let mut app = app(&ctx);
        let id = active_owner(&mut app, &ctx, 94_102);
        let (tx, rx) = mpsc::channel();
        let (key, cancel) = app
            .with_active_viewer_context(|owner| {
                let mut params = owner.effective_params(1).clone();
                if lut {
                    params.creative_lut.id =
                        Some(crate::creative_lut::BuiltinCreativeLut::WarmFilm.id());
                    params.creative_lut.strength = 1.0;
                    owner.creative_lut_library =
                        crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(
                            crate::creative_lut::BuiltinCreativeLut::WarmFilm,
                        );
                    assert!(owner.resolved_creative_lut(&params).is_some());
                    assert!(owner.creative_lut_requires_final_effect(&params));
                } else {
                    params.colorize.mode = crate::colorize::ColorizeMode::AllImages;
                }
                owner.adjustment_page_params.insert(1, params);
                let key = owner.final_composite_key_for_pixels(
                    owner.current_edit_result_key(1),
                    [80, 120],
                    owner.effective_params(1),
                );
                let cancel = Arc::new(AtomicBool::new(false));
                owner.final_effect_pending.insert(
                    key,
                    FinalEffectPending {
                        canonical_dims: None,
                        source_size: [80, 120],
                        cancel: cancel.clone(),
                        state: FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)),
                        items_generation: owner.items_generation,
                        output_complete: true,
                        nearest_sampler: false,
                        origin: FinalEffectOrigin::LegacyDisplay,
                        gpu_diagnostics: Default::default(),
                    },
                );
                (key, cancel)
            })
            .unwrap();
        assert!(app.pause_current_active_viewer_context(&ctx));
        app.with_viewer_context(id, |owner| {
            let pending = &owner.final_effect_pending[&key];
            assert!(matches!(pending.origin, FinalEffectOrigin::LegacyDisplay));
            assert!(matches!(
                pending.state,
                FinalEffectState::Computing(FinalEffectReceiver::Legacy(_))
            ));
            assert!(Arc::ptr_eq(&pending.cancel, &cancel));
            assert!(!pending.cancel.load(Ordering::Relaxed));
            assert!(!owner.final_composite_cache.contains_key(&key));
        })
        .unwrap();
        // The sender identifies this exact pre-park job/channel. A replacement
        // receiver or cancellation would break this completion handoff.
        let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GOLD));
        tx.send(FinalCompositeResult::Ready {
            pixels: pixels.clone(),
            elapsed_ms: 0.0,
            timing: FinalCompositeTiming::default(),
        })
        .expect("the original legacy receiver survives park");
        app.with_viewer_context(id, |owner| {
            assert!(
                matches!(
                    owner.final_effect_pending[&key].state,
                    FinalEffectState::Computing(FinalEffectReceiver::Legacy(_))
                ),
                "plain mount never consumes the completed channel"
            );
            assert!(Arc::ptr_eq(
                &owner.final_effect_pending[&key].cancel,
                &cancel
            ));
        })
        .unwrap();
        begin_registration_frame(&mut app, 101);
        assert!(app.activate_detached_image_window_snapshot(&ctx, 94_102));
        app.with_active_viewer_context(|owner| {
            assert!(!owner.final_effect_pending.contains_key(&key));
            assert!(Arc::ptr_eq(
                &owner.final_composite_cache.get(&key).unwrap().pixels,
                &pixels
            ));
            crate::app::final_prepare_tests::expect_adopted_before_lookup(key, || {
                assert!(owner.resolve_fs_display_tex(1, false).is_some());
            });
        })
        .unwrap();
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(
            app.final_prepare_cpu.snapshot().units,
            0,
            "legacy display is not a D3 reservation"
        );
    }
}

#[test]
fn d3_2_fork_does_not_share_computing_or_cpu_ready_pending_and_its_lease() {
    for ready in [false, true] {
        for live in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            let main = app.viewer_context_main();
            let hold = (!ready).then(WorkerHold::new);
            assert!(start(&mut app, &ctx, 1));
            let key = if let Some(hold) = &hold {
                hold.entered();
                pending_key(&app, 1)
            } else {
                wait_ready(&mut app, 1)
            };
            let cancel = app.final_effect_pending[&key].cancel.clone();
            let reservation = reservation_ptr(&app.final_effect_pending[&key]);
            let before = app.final_prepare_cpu.snapshot();
            let physical = app.tmp.path().join("fork-physical");
            let fork = if live {
                app.bind_window(main, 94_103).unwrap();
                app.fork_mounted_live_media_context(94_103)
            } else {
                app.fork_materialized_still_context(&physical, 0)
            };
            assert_ne!(fork, main);
            assert_eq!(app.final_prepare_cpu.snapshot(), before);
            assert!(Arc::ptr_eq(&app.final_effect_pending[&key].cancel, &cancel));
            assert_eq!(
                reservation_ptr(&app.final_effect_pending[&key]),
                reservation
            );
            app.with_viewer_context(fork, |owner| {
                assert!(
                    owner.final_effect_pending.is_empty(),
                    "fork owns no source receiver/cancel/ready lease"
                );
                assert!(owner.final_composite_cache.is_empty());
            })
            .unwrap();
            app.retire_context(fork, "d3-2-fork-test", |_| ()).unwrap();
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(app.final_prepare_cpu.snapshot(), before);
            if let Some(hold) = &hold {
                hold.release();
            }
            wait_ready(&mut app, 1);
            app.cancel_final_effects_for_idx(1);
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        }
    }
}

#[test]
fn d3_2_promote_transfers_same_pending_lease_and_sibling_mutations_are_isolated() {
    for ready in [false, true] {
        for mutation in ["close", "book", "adjustment"] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            let main_id = app.ensure_final_composite_texture(&ctx, 0).unwrap().id();
            let original = app.viewer_context_main();
            let generation = app.items_generation;
            let edit = app.current_edit_result_key(1);
            let hold = (!ready).then(WorkerHold::new);
            assert!(start(&mut app, &ctx, 1));
            let key = if let Some(hold) = &hold {
                hold.entered();
                pending_key(&app, 1)
            } else {
                wait_ready(&mut app, 1)
            };
            let cancel = app.final_effect_pending[&key].cancel.clone();
            let reservation = reservation_ptr(&app.final_effect_pending[&key]);
            let before = app.final_prepare_cpu.snapshot();
            assert_eq!(app.stash_mounted_and_start_fresh("d3-2-promote"), original);
            assert_ne!(app.viewer_context_main(), original);
            assert!(app.final_effect_pending.is_empty());
            assert_eq!(app.final_prepare_cpu.snapshot(), before);
            seed_owner(&mut app, &ctx, 72);
            match mutation {
                "close" => app.close_fullscreen(),
                "book" => app.invalidate_idx_state_and_queues(),
                "adjustment" => app.bump_input_generation(1),
                _ => unreachable!(),
            }
            app.with_viewer_context(original, |owner| {
                assert_eq!(owner.projected_viewer_context_id(), original);
                assert_eq!(owner.items_generation, generation);
                assert_eq!(owner.current_edit_result_key(1), edit);
                assert_eq!(
                    owner
                        .final_composite_cache
                        .iter()
                        .find(|(key, _)| key.edit_key.idx == 0)
                        .unwrap()
                        .1
                        .texture
                        .id(),
                    main_id
                );
                let pending = &owner.final_effect_pending[&key];
                assert!(Arc::ptr_eq(&pending.cancel, &cancel));
                assert_eq!(reservation_ptr(pending), reservation);
                assert!(!pending.cancel.load(Ordering::Relaxed));
                assert_eq!(pending.origin.unit().unwrap().context, original);
                assert_eq!(owner.final_effect_pending.len(), 1);
                assert!(matches!(pending.state, FinalEffectState::CpuReady(_)) == ready);
            })
            .unwrap();
            assert_eq!(app.final_prepare_cpu.snapshot(), before);
            if let Some(hold) = &hold {
                hold.release();
            }
            app.with_viewer_context(original, |owner| {
                wait_ready(owner, 1);
                begin_registration_frame(owner, 102);
                owner.service_final_prepare_before_lookup(&ctx);
                assert!(owner.final_effect_pending.contains_key(&key));
                assert!(owner.ensure_final_composite_texture(&ctx, 1).is_some());
                assert!(owner.exact_complete_final_composite_texture(1).is_some());
                assert!(owner.final_effect_pending.is_empty());
            })
            .unwrap();
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
            assert!(app.final_effect_pending.is_empty());
        }
    }
}

#[test]
fn d3_2_active_viewer_production_update_restarts_blocked_sibling_on_release_repaint() {
    let ctx = egui::Context::default();
    install_presentation(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let mut app = app(&ctx);
    let main_hold = WorkerHold::new();
    assert!(start(&mut app, &ctx, 1));
    main_hold.entered();
    let main_key = pending_key(&app, 1);
    let id = active_owner(&mut app, &ctx, 94_104);
    begin_registration_frame(&mut app, 103);
    frame(&ctx, |ui, _| {
        assert!(app.update_active_viewer_context(ui, None).updated);
    });
    app.with_active_viewer_context(|owner| {
        assert!(
            owner.final_effect_pending.is_empty(),
            "B cannot start while A owns the global permit"
        );
        assert!(owner.exact_complete_final_composite_texture(1).is_none());
    })
    .unwrap();
    assert!(app.final_effect_pending.contains_key(&main_key));
    app.cancel_final_effects_for_idx(1);
    assert!(!app.final_effect_pending.contains_key(&main_key));
    assert_eq!(
        app.final_prepare_cpu.snapshot().running,
        1,
        "removing A does not release its real permit"
    );

    // Settle egui repaint coalescing, without invoking B's service, sending a
    // key, or running periodic producer work while A remains stopped.
    for _ in 0..3 {
        ctx.run_pass(Default::default(), |_| {})
            .drop_without_applying_deltas();
    }
    let (wake_tx, wake_rx) = mpsc::channel();
    ctx.set_request_repaint_callback(move |event| {
        let _ = wake_tx.send(event.viewport_id);
    });
    main_hold.release();
    let event = wake_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("actual A completion must wake B's existing service");
    assert_eq!(
        event,
        egui::ViewportId::ROOT,
        "ROOT repaint drives the active detached owner service"
    );
    wait_idle(&app);

    let hold = WorkerHold::new();
    // This call is the reaction to that release event; no additional input or
    // private spawn helper is used to make B start.
    begin_registration_frame(&mut app, 104);
    frame(&ctx, |ui, _| {
        assert!(app.update_active_viewer_context(ui, None).updated);
    });
    hold.entered(); // Only the actual mounted production work boundary starts this job.
    let key = app.with_active_viewer_context(|owner| {
        let key = pending_key(owner, 1);
        assert!(matches!(owner.final_effect_pending[&key].origin, FinalEffectOrigin::Speculative(ref unit) if unit.context == id));
        assert_eq!(owner.final_effect_pending.len(), 1);
        assert!(!owner.final_composite_cache.contains_key(&key));
        key
    }).unwrap();
    assert!(app.final_effect_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
    hold.release();
    app.with_active_viewer_context(|owner| {
        wait_ready(owner, 1);
    })
    .unwrap();
    begin_registration_frame(&mut app, 105);
    frame(&ctx, |ui, _| {
        assert!(app.update_active_viewer_context(ui, None).updated);
    });
    app.finalize_speculative_gpu_output(&ctx, 105, 1);
    app.with_active_viewer_context(|owner| {
        assert!(owner.final_composite_cache.contains_key(&key));
        assert!(owner.final_effect_pending.is_empty());
        assert!(owner.exact_complete_final_composite_texture(1).is_some());
    })
    .unwrap();
    assert!(app.final_effect_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}
