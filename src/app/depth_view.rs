use super::*;
use crate::depth_parallax::{
    DepthEntryBlock, DepthEntryInput, DepthNextTarget, DepthSettled, DepthSettledOutcome,
    DepthSourceMaterial, DepthSourcePhase, DepthSourceWait, DepthViewState,
};
use crate::fs_exclusive_view::{ExclusiveViewKind, FsExclusiveView};

/// A lazy model check may only resume the original image in its original context.
/// Textures can refine during the check, so content/list identity owns this intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DepthEntryRequest {
    pub(crate) context: ViewerContextId,
    items_generation: u64,
    fs_idx: usize,
    item_id: crate::item_identity::ItemId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthPageRole {
    Current,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthPageProgress {
    Ready,
    Producing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthSlideshowGate {
    Go,
    Wait,
}

#[cfg(all(test, windows))]
pub(super) mod gpu_tests {
    use super::*;

    #[test]
    fn depth_slideshow_failed_fact_waits_for_new_material_and_resumes_without_retry() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        crate::app::depth_view_tests::install_pages(&mut app, &ctx);
        app.ai_runtime_init =
            crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("failed-material fixture");
        let render = app.wgpu_render_state.clone().unwrap();
        let texture = app
            .resolve_fs_processed_material(&ctx, 0, false)
            .unwrap()
            .source_texture()
            .clone();
        deliver_textures(&ctx, &render);
        let edit_key = app.current_edit_result_key(0);
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: texture.id(),
        };
        let mut state = DepthViewState::new(app.edit_request_owner_context());
        state.source = DepthSourcePhase::Failed {
            key,
            reason: "completed failure".into(),
        };
        state.settled = Some(DepthSettled {
            fs_idx: 0,
            edit_key,
            key,
            outcome: DepthSettledOutcome::Failed,
        });
        app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "failed_fact_regression");
        app.advance_depth_source(
            &ctx,
            DepthSourceMaterial::Ready {
                texture: texture.clone(),
            },
        );
        assert!(app.depth_current_settled());
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 15.0;
        app.adjustment_page_params.insert(0, params);
        assert_eq!(app.current_edit_result_key(0), edit_key);
        let pending_key = crate::app::final_prepare_tests::queued_final(&mut app, 0);
        app.advance_depth_source(
            &ctx,
            DepthSourceMaterial::NotYet(DepthSourceWait::IncompleteComposite),
        );
        assert!(!app.depth_current_settled());
        assert!(
            matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state)
            if state.current_material.is_none() && matches!(state.source, DepthSourcePhase::Failed { key: old, .. } if old == key)
                && state.settled.as_ref().is_some_and(|fact| fact.key == key))
        );
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            DepthPageProgress::Producing
        );
        app.final_effect_pending.remove(&pending_key);
        app.adjustment_page_params.remove(&0);
        crate::depth_estimation::DepthWorker::take_submission_for_test();
        app.advance_depth_source(&ctx, DepthSourceMaterial::Ready { texture });
        assert!(app.depth_current_settled());
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            DepthPageProgress::Ready
        );
        assert!(crate::depth_estimation::DepthWorker::take_submission_for_test().is_none());
    }

    pub(in crate::app) fn deliver_textures(ctx: &egui::Context, render: &egui_wgpu::RenderState) {
        let mut textures = ctx.tex_manager().write().take_delta();
        for (id, deltas) in &textures.set {
            for delta in deltas {
                // These tests can deliver image backing before the first UI
                // pass ends, while egui's font atlas is still a placeholder.
                if delta.image.size().contains(&0) {
                    assert_eq!(*id, egui::TextureId::Managed(0));
                    continue;
                }
                render
                    .renderer
                    .write()
                    .update_texture(&render.device, &render.queue, *id, delta);
            }
        }
        for id in &textures.free {
            render.renderer.write().free_texture(id);
        }
        textures.clear();
    }

    #[test]
    fn depth_next_material_and_input_changes_retire_readback_and_estimation() {
        use crate::depth_estimation::{DepthPending, DepthSourceKey};
        use crate::depth_parallax::{DepthNextPhase, DepthNextSlot};
        for estimating in [false, true] {
            for change in 0..3 {
                let ctx = egui::Context::default();
                let mut app = crate::app::setup_app_for_test();
                crate::app::depth_view_tests::install_pages(&mut app, &ctx);
                app.ai_runtime_init =
                    crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("supersession fixture");
                let render = app.wgpu_render_state.clone().unwrap();
                let texture = app
                    .resolve_fs_processed_material(&ctx, 0, false)
                    .unwrap()
                    .source_texture()
                    .clone();
                let next_texture = app
                    .resolve_fs_processed_material(&ctx, 2, false)
                    .unwrap()
                    .source_texture()
                    .clone();
                deliver_textures(&ctx, &render);
                let key = DepthSourceKey {
                    texture_id: texture.id(),
                };
                let next_key = DepthSourceKey {
                    texture_id: next_texture.id(),
                };
                let target = app.depth_next_target(2);
                let mut state = DepthViewState::new(app.edit_request_owner_context());
                state.source = DepthSourcePhase::Ready { key };
                state.current_material = Some(key);
                state.settled = Some(DepthSettled {
                    fs_idx: 0,
                    edit_key: app.current_edit_result_key(0),
                    key,
                    outcome: DepthSettledOutcome::Presented,
                });
                let next_backing = render
                    .renderer
                    .read()
                    .texture(&next_key.texture_id)
                    .unwrap()
                    .texture
                    .clone()
                    .unwrap();
                let cancellation = if estimating {
                    let (pending, _complete) =
                        DepthPending::deferred_for_test(state.context, 781, next_key);
                    let cancel = pending.cancellation_token_for_test();
                    state.next = Some(DepthNextSlot {
                        target,
                        key: next_key,
                        phase: DepthNextPhase::Estimating { pending },
                        started_at: std::time::Instant::now(),
                    });
                    Some(cancel)
                } else {
                    state.start_next(
                        &render.device,
                        &render.queue,
                        &next_backing,
                        target,
                        next_key,
                    );
                    None
                };
                app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "supersession_fixture");
                app.slideshow_playing = true;
                assert!(app.depth_current_settled());
                let mut current_reply = None;
                let selected = match change {
                    0 => DepthSourceMaterial::NotYet(DepthSourceWait::IncompleteComposite),
                    1 => {
                        app.input_generation.insert(0, 771);
                        DepthSourceMaterial::Ready {
                            texture: texture.clone(),
                        }
                    }
                    _ => {
                        let replacement = ctx.load_texture(
                            "replaced-source",
                            egui::ColorImage::filled([2, 3], egui::Color32::BLACK),
                            Default::default(),
                        );
                        deliver_textures(&ctx, &render);
                        let replacement_key = DepthSourceKey {
                            texture_id: replacement.id(),
                        };
                        let context = app.edit_request_owner_context();
                        let (pending, reply) =
                            DepthPending::deferred_for_test(context, 782, replacement_key);
                        current_reply = Some(reply);
                        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                            unreachable!()
                        };
                        state.source = DepthSourcePhase::Estimating {
                            key: replacement_key,
                            pending,
                        };
                        DepthSourceMaterial::Ready {
                            texture: replacement,
                        }
                    }
                };
                app.advance_depth_source(&ctx, selected);
                assert!(!app.depth_current_settled());
                assert!(
                    matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if state.next.is_none() && state.readback_bytes() == 0)
                );
                if let Some(cancel) = cancellation {
                    assert!(cancel.load(Ordering::Acquire));
                }
                drop(current_reply);
            }
        }
    }

    #[test]
    fn depth_next_manual_transition_keeps_request_and_handoff_rejects_cancelled_or_wrong_target() {
        use crate::depth_estimation::{DepthPending, DepthSourceKey};
        use crate::depth_parallax::{DepthNextPhase, DepthNextSlot};
        for mutation in 0..4 {
            let ctx = egui::Context::default();
            let mut app = crate::app::setup_app_for_test();
            crate::app::depth_view_tests::install_pages(&mut app, &ctx);
            app.ai_runtime_init =
                crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("handoff fixture");
            let render = app.wgpu_render_state.clone().unwrap();
            let material = app.resolve_fs_processed_material(&ctx, 2, false).unwrap();
            let key = DepthSourceKey {
                texture_id: material.source_texture().id(),
            };
            deliver_textures(&ctx, &render);
            let target = app.depth_next_target(2);
            let mut state = DepthViewState::new(app.edit_request_owner_context());
            let (pending, _complete) = DepthPending::deferred_for_test(state.context, 771, key);
            let token = pending.cancellation_token_for_test();
            if mutation == 1 {
                pending.cancel();
            }
            let mut saved_target = target;
            if mutation == 2 {
                saved_target.items_generation += 1;
            }
            let saved_key = if mutation == 3 {
                DepthSourceKey {
                    texture_id: egui::TextureId::Managed(771_999),
                }
            } else {
                key
            };
            state.next = Some(DepthNextSlot {
                target: saved_target,
                key: saved_key,
                phase: DepthNextPhase::Estimating { pending },
                started_at: std::time::Instant::now(),
            });
            app.set_fs_exclusive_view(FsExclusiveView::Depth(state), "handoff_test");
            app.slideshow_playing = true;
            app.fullscreen_idx = Some(2);
            if mutation == 0 {
                app.advance_depth_source(
                    &ctx,
                    DepthSourceMaterial::NotYet(DepthSourceWait::PageTransition),
                );
                assert!(
                    matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if state.next.is_some())
                );
                assert!(
                    !token.load(Ordering::Acquire),
                    "manual transition cannot cancel the incoming request"
                );
            }
            let backing = render
                .renderer
                .read()
                .texture(&key.texture_id)
                .unwrap()
                .texture
                .clone()
                .unwrap();
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state
                .select_source_for_target(
                    &render.device,
                    &render.queue,
                    &backing,
                    key,
                    Some(target),
                )
                .unwrap();
            assert!(state.next.is_none());
            if mutation == 0 {
                assert!(matches!(state.source, DepthSourcePhase::Estimating { .. }));
                assert!(!token.load(Ordering::Acquire));
            } else {
                assert!(matches!(state.source, DepthSourcePhase::ReadingBack { .. }));
                assert!(token.load(Ordering::Acquire));
            }
            app.set_fs_exclusive_view(FsExclusiveView::None, "handoff_test_drop");
            assert!(token.load(Ordering::Acquire));
        }
    }

    #[test]
    fn depth_next_readback_and_failure_handoff_and_shared_one_upload_per_frame() {
        use crate::depth_estimation::{DepthMap, DepthPending, DepthSourceKey};
        use crate::depth_parallax::{DepthNextPhase, DepthNextSlot};
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        crate::app::depth_view_tests::install_pages(&mut app, &ctx);
        let render = app.wgpu_render_state.clone().unwrap();
        let material = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
        let key = DepthSourceKey {
            texture_id: material.source_texture().id(),
        };
        deliver_textures(&ctx, &render);
        let backing = render
            .renderer
            .read()
            .texture(&key.texture_id)
            .unwrap()
            .texture
            .clone()
            .unwrap();
        let target = app.depth_next_target(0);
        let mut state = DepthViewState::new(app.edit_request_owner_context());
        state.start_next(&render.device, &render.queue, &backing, target, key);
        let bytes = state.readback_bytes();
        state
            .select_source_for_target(&render.device, &render.queue, &backing, key, Some(target))
            .unwrap();
        assert!(state.next.is_none());
        assert_eq!(state.readback_bytes(), bytes);
        assert!(matches!(state.source, DepthSourcePhase::ReadingBack { .. }));
        state.source = DepthSourcePhase::WaitingForStableTexture;
        state.next = Some(DepthNextSlot {
            target,
            key,
            phase: DepthNextPhase::Failed {
                reason: "test failure".into(),
            },
            started_at: std::time::Instant::now(),
        });
        state
            .select_source_for_target(&render.device, &render.queue, &backing, key, Some(target))
            .unwrap();
        assert!(
            matches!(&state.source, DepthSourcePhase::Failed { reason, .. } if reason == "test failure")
        );
        assert!(state.next.is_none());

        let map = |key| DepthMap {
            key,
            width: 2,
            height: 3,
            values: vec![half::f16::from_f32(0.5); 6],
            median: 0.5,
        };
        state.source = DepthSourcePhase::Estimating {
            key,
            pending: DepthPending::completed_for_test(state.context, 772, map(key)),
        };
        let next_key = DepthSourceKey {
            texture_id: egui::TextureId::Managed(771_998),
        };
        state.next = Some(DepthNextSlot {
            target,
            key: next_key,
            phase: DepthNextPhase::Estimating {
                pending: DepthPending::completed_for_test(state.context, 773, map(next_key)),
            },
            started_at: std::time::Instant::now(),
        });
        state.advance(
            &render.device,
            &render.queue,
            &ctx,
            ctx.viewport_id(),
            |_, _, _| panic!("no readback submit"),
        );
        state.advance_next(
            &render.device,
            &render.queue,
            &ctx,
            ctx.viewport_id(),
            0,
            |_, _, _| panic!("no prefetch submit"),
        );
        assert!(state.lru.contains(key));
        assert!(!state.lru.contains(next_key));
        assert!(
            state.next.is_some(),
            "next reply must remain in its receiver for this frame"
        );
        ctx.run_pass(Default::default(), |_| {})
            .drop_without_applying_deltas();
        state.advance_next(
            &render.device,
            &render.queue,
            &ctx,
            ctx.viewport_id(),
            0,
            |_, _, _| panic!("no prefetch submit"),
        );
        assert!(state.lru.contains(next_key));
        assert!(state.next.is_none());
    }

    #[test]
    fn depth_slideshow_prefetched_source_and_d3_off_final_paint_on_first_landing() {
        use crate::depth_estimation::{DepthMap, DepthPending, DepthSourceKey};
        use crate::depth_parallax::{DepthNextPhase, DepthSettled, DepthSettledOutcome};
        use crate::ui_fullscreen::page_turn_submission_tests::{
            draw_current_without_sources, frame,
        };
        for (adjusted, evict_source) in [(false, false), (true, false), (true, true)] {
            let ctx = egui::Context::default();
            let mut app = crate::app::setup_app_for_test();
            crate::app::depth_view_tests::install_pages(&mut app, &ctx);
            // The general Depth fixture uses non-mipmapped, unbound textures.
            // Source sharing must exercise the immutable display allocations
            // produced by the ordinary fullscreen loader instead.
            for idx in [0, 2, 3] {
                crate::app::source_texture_test_support::seed(&mut app, &ctx, idx);
            }
            let mut render = app.wgpu_render_state.clone().unwrap();
            render.renderer = Arc::new(egui::epaint::mutex::RwLock::new(egui_wgpu::Renderer::new(
                &render.device,
                render.target_format,
                Default::default(),
            )));
            app.wgpu_render_state = Some(render.clone());
            app.ai_runtime_init =
                crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("cached-depth fixture");
            app.settings.page_turn_animation_enabled = false;
            if adjusted {
                let mut params = crate::adjustment::AdjustParams::default();
                params.brightness = 15.0;
                app.adjustment_page_params.insert(2, params);
            }
            frame(&ctx, |ui, ctx| {
                draw_current_without_sources(&mut app, ui, ctx);
                deliver_textures(ctx, &render);
            });
            let current = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
            let current_key = DepthSourceKey {
                texture_id: current.source_texture().id(),
            };
            app.enter_depth_parallax(&ctx);
            app.slideshow_playing = true;
            let edit_key = app.current_edit_result_key(0);
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state.source = DepthSourcePhase::Ready { key: current_key };
            state.current_material = Some(current_key);
            state.settled = Some(DepthSettled {
                fs_idx: 0,
                edit_key,
                key: current_key,
                outcome: DepthSettledOutcome::Presented,
            });
            // The first Depth paint after playback starts owns this baseline;
            // the preceding 2D paint happened while curl and playback were off.
            app.spread_display_composition_for_anchor(0);
            assert!(app.next_unit_current_presented());
            app.prefetch_next_final_unit(&ctx, 0);
            if adjusted {
                let key = crate::app::final_prepare::tests::wait_ready(&mut app, 2);
                app.speculative_gpu_frame.begin(1);
                app.adopt_final_prepare_results(
                    &ctx,
                    vec![key],
                    crate::app::final_prepare_budget::FinalPrepareAdoption::RootFinalized(
                        None,
                        crate::ui_fullscreen::FinalUnitPosition::Next,
                    ),
                );
                assert!(app.exact_complete_final_composite_entry(2).is_some());
            }
            if evict_source {
                app.fs_cache.remove(&2);
                assert!(matches!(
                    app.fs_display_page_load_state(2),
                    FsPageLoadState::DisplayReady(FsPageDisplaySource::ResidentFinalComposite)
                ));
            }
            let target = app.depth_next_target(2);
            let material = app
                .resident_final_prepare_material(&ctx, 2)
                .unwrap_or_else(|| {
                    panic!("next material must be prepared before display: adjusted={adjusted}")
                });
            let key = DepthSourceKey {
                texture_id: material.source_texture().id(),
            };
            if adjusted {
                assert_eq!(
                    app.depth_page_progress(&ctx, 2, DepthPageRole::Next),
                    DepthPageProgress::Producing,
                    "managed final waits only for this frame's backend registration"
                );
            } else if let Some(FsCacheEntry::Static { tex, .. }) = app.fs_cache.get(&2) {
                assert_eq!(
                    key.texture_id,
                    tex.id(),
                    "unedited final shares the prefetched source"
                );
            }
            deliver_textures(&ctx, &render);
            let backing = render
                .renderer
                .read()
                .texture(&key.texture_id)
                .unwrap()
                .texture
                .clone()
                .unwrap();
            let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
                unreachable!()
            };
            state.start_next(&render.device, &render.queue, &backing, target, key);
            assert!(state.readback_bytes() > 0);
            let context = state.context;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while matches!(
                state.next.as_ref().map(|slot| &slot.phase),
                Some(DepthNextPhase::ReadingBack { .. })
            ) {
                assert!(std::time::Instant::now() < deadline);
                state.advance_next(
                    &render.device,
                    &render.queue,
                    &ctx,
                    ctx.viewport_id(),
                    0,
                    |key, _, _| {
                        Ok(DepthPending::completed_for_test(
                            context,
                            771,
                            DepthMap {
                                key,
                                width: 2,
                                height: 1,
                                values: vec![half::f16::from_f32(0.5); 2],
                                median: 0.5,
                            },
                        ))
                    },
                );
                std::thread::yield_now();
            }
            assert!(state.next.is_none());
            assert!(state.lru.contains(key));
            assert_eq!(state.readback_bytes(), 0);
            assert_eq!(
                app.depth_page_progress(&ctx, 2, DepthPageRole::Next),
                DepthPageProgress::Ready
            );
            crate::depth_estimation::DepthWorker::take_submission_for_test();
            app.complete_book_slideshow_target(
                &ctx,
                2,
                None,
                crate::app::book_composition_permission::BookTargetPurpose::Next,
            );
            assert_eq!(app.fullscreen_idx, Some(2));
            assert!(
                app.fs_pending.get(&2).is_none(),
                "resident D3 output must not require source reload"
            );
            let (_, selected) = app.resolve_fs_processed_payload_and_depth(&ctx, 2, false);
            assert!(
                matches!(&selected, DepthSourceMaterial::Ready { texture } if texture.id() == key.texture_id)
            );
            app.advance_depth_source(&ctx, selected);
            let output = frame(&ctx, |ui, ctx| {
                draw_current_without_sources(&mut app, ui, ctx)
            });
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| matches!(shape.shape, egui::Shape::Callback(_)))
            );
            assert!(app.depth_current_settled());
            assert!(crate::depth_estimation::DepthWorker::take_submission_for_test().is_none());
            assert!(
                matches!(&app.fs_exclusive_view, FsExclusiveView::Depth(state) if matches!(state.source, DepthSourcePhase::Ready { key: actual } if actual == key))
            );
        }
    }

    /// Retained paper-turn evidence must not start D3 during a depth slideshow.
    #[test]
    fn depth_slideshow_ignores_retained_flat_until_depth_is_presented() {
        use crate::depth_estimation::{DepthMap, DepthSourceKey};
        use crate::ui_fullscreen::page_turn_submission_tests::{
            draw_current_without_sources, frame, install_presentation,
        };

        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        crate::app::depth_view_tests::install_pages(&mut app, &ctx);
        // Keep callback slots private while sharing the existing test device.
        let mut render = app.wgpu_render_state.clone().unwrap();
        render.renderer = Arc::new(egui::epaint::mutex::RwLock::new(egui_wgpu::Renderer::new(
            &render.device,
            render.target_format,
            Default::default(),
        )));
        app.wgpu_render_state = Some(render.clone());
        app.ai_runtime_init =
            crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("retained-flat fixture");
        app.settings.page_turn_animation_enabled = true;
        install_presentation(&ctx);
        frame(&ctx, |ui, ctx| {
            draw_current_without_sources(&mut app, ui, ctx);
            // The headless frame helper discards deltas at the end of UI.
            // Deliver the selected source's real backing before that boundary.
            deliver_textures(ctx, &render);
        });
        assert!(app.page_turn_current_unit_presented());
        assert_eq!(app.depth_entry_allowed(0), Ok(()));

        app.slideshow_playing = true;
        app.enter_depth_parallax(&ctx);
        assert!(app.slideshow_playing);
        assert!(app.fs_exclusive_view.is_depth());
        assert!(app.page_turn_current_unit_presented(), "entry retains Flat");
        assert!(
            !app.next_unit_current_presented(),
            "Flat cannot authorize depth Next"
        );
        let material = app.resolve_fs_processed_material(&ctx, 0, false).unwrap();
        let key = DepthSourceKey {
            texture_id: material.source_texture().id(),
        };
        deliver_textures(&ctx, &render);
        let backing = render
            .renderer
            .read()
            .texture(&key.texture_id)
            .unwrap()
            .texture
            .clone()
            .unwrap();
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.lru.insert(
            key,
            Arc::new(
                crate::depth_parallax_wgpu::DepthTexture::upload(
                    &render.device,
                    &render.queue,
                    DepthMap {
                        key,
                        width: 2,
                        height: 3,
                        values: vec![half::f16::from_f32(0.5); 6],
                        median: 0.5,
                    },
                )
                .unwrap(),
            ),
        );
        state
            .select_source(&render.device, &render.queue, &backing, key)
            .unwrap();
        assert!(
            !app.next_unit_current_presented(),
            "Ready is not yet presented"
        );
        app.advance_depth_source(
            &ctx,
            DepthSourceMaterial::Ready {
                texture: material.source_texture().clone(),
            },
        );
        let output = frame(&ctx, |ui, ctx| {
            draw_current_without_sources(&mut app, ui, ctx);
        });
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Callback(_))),
            "the normal image draw must emit the depth callback"
        );
        assert!(app.fs_exclusive_view.is_depth());
        assert!(app.depth_current_settled());
        assert!(app.next_unit_current_presented());
        assert!(
            app.page_turn_current_unit_presented(),
            "an actual depth draw still retains the earlier accepted Flat evidence"
        );
        app.settings.page_turn_animation_enabled = false;
        assert!(app.next_unit_final_prepare_enabled());
        assert!(app.next_unit_current_presented());
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.source = DepthSourcePhase::Failed {
            key,
            reason: "settled failure".into(),
        };
        state.settled.as_mut().unwrap().outcome = DepthSettledOutcome::Failed;
        assert!(app.depth_current_settled());
        assert!(
            !app.next_unit_current_presented(),
            "Failed is terminal but cannot authorize D3 Next"
        );
        let old_key = app.current_edit_result_key(0);
        app.input_generation
            .insert(0, old_key.source_gen.wrapping_add(1));
        assert!(
            !app.depth_current_settled(),
            "input-only refinement invalidates the fact"
        );
        assert!(!app.next_unit_current_presented());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DepthWaitStatus {
    PreparingModel,
    Calculating,
}

impl DepthWaitStatus {
    pub(crate) fn panel_text(self) -> &'static str {
        match self {
            Self::PreparingModel => "モデルを準備中",
            Self::Calculating => "奥行きを計算中",
        }
    }

    pub(crate) fn overlay_text(self) -> &'static str {
        match self {
            Self::PreparingModel => "3D 写真のモデルを準備中…",
            Self::Calculating => "奥行きを計算中…",
        }
    }

    pub(crate) fn visible_at(
        self,
        started_at: std::time::Instant,
        now: std::time::Instant,
    ) -> bool {
        super::animation_expansion_progress_visible(Some(started_at), now)
    }
}

/// Presentation only: the source phase, never the delay gate, owns job lifetime.
pub(crate) fn depth_wait_status(
    view: &FsExclusiveView,
    runtime: &crate::ai::runtime::AiRuntimeInitSnapshot,
) -> Option<DepthWaitStatus> {
    use crate::ai::runtime::AiRuntimeInitSnapshot;
    use crate::depth_parallax::DepthSourcePhase;
    match view {
        FsExclusiveView::Depth(state)
            if !matches!(
                state.source,
                DepthSourcePhase::Ready { .. } | DepthSourcePhase::Failed { .. }
            ) =>
        {
            Some(
                if matches!(
                    runtime,
                    AiRuntimeInitSnapshot::Dormant | AiRuntimeInitSnapshot::Initializing
                ) {
                    DepthWaitStatus::PreparingModel
                } else {
                    DepthWaitStatus::Calculating
                },
            )
        }
        _ => None,
    }
}

impl App {
    pub(crate) fn depth_slideshow_prefetch_active(&self) -> bool {
        self.fs_exclusive_view.is_depth()
            && self.slideshow_playing
            && self.reading_flow.is_paged()
            && !self.spread_mode.is_split()
    }

    pub(crate) fn next_unit_final_prepare_enabled(&self) -> bool {
        self.reading_flow.is_paged()
            && !self.spread_mode.is_split()
            && !self.remote_session_blocks_local_control()
            && (self.settings.page_turn_animation_enabled || self.depth_slideshow_prefetch_active())
    }

    pub(crate) fn depth_current_settled(&self) -> bool {
        let FsExclusiveView::Depth(state) = &self.fs_exclusive_view else {
            return false;
        };
        let Some(fact) = &state.settled else {
            return false;
        };
        self.fullscreen_idx == Some(fact.fs_idx)
            && state.context == self.edit_request_owner_context()
            && fact.edit_key == self.current_edit_result_key(fact.fs_idx)
            && state.current_material == Some(fact.key)
            && matches!(state.source, DepthSourcePhase::Ready { key } | DepthSourcePhase::Failed { key, .. } if key == fact.key)
    }

    pub(crate) fn next_unit_current_presented(&self) -> bool {
        if self.depth_slideshow_prefetch_active() {
            self.depth_current_settled()
                && matches!(&self.fs_exclusive_view,
                FsExclusiveView::Depth(state) if state.settled.as_ref().is_some_and(|fact| fact.outcome == DepthSettledOutcome::Presented))
        } else {
            self.page_turn_current_unit_presented()
        }
    }

    pub(crate) fn depth_next_target(&self, idx: usize) -> DepthNextTarget {
        DepthNextTarget {
            idx,
            item_id: self.item_id(idx),
            items_generation: self.items_generation,
        }
    }

    pub(crate) fn depth_slideshow_gate(
        &self,
        ctx: &egui::Context,
        idx: usize,
        _now: std::time::Instant,
    ) -> DepthSlideshowGate {
        if !self.fs_exclusive_view.is_depth()
            || self.fs_entry_is_animated(idx)
            || self.fs_transparent_bg_mode != 0
            || self.depth_export_operation.is_some()
        {
            return DepthSlideshowGate::Go;
        }
        if self.fullscreen_idx.is_some_and(|current| {
            self.depth_page_progress(ctx, current, DepthPageRole::Current)
                == DepthPageProgress::Producing
        }) || self.depth_page_progress(ctx, idx, DepthPageRole::Next)
            == DepthPageProgress::Producing
        {
            DepthSlideshowGate::Wait
        } else {
            DepthSlideshowGate::Go
        }
    }
    /// A successful independent activation takes the live pass away from main's
    /// fullscreen viewport. Ordinary mounts/updates and embedded main do not.
    #[cfg(windows)]
    pub(super) fn retire_main_depth_for_independent_activation(
        &mut self,
        destination: ViewerContextId,
    ) {
        let main = self.viewer_context_main();
        if destination == main {
            return;
        }
        self.with_viewer_context(main, |app| {
            if app.fullscreen_idx.is_some()
                && app.viewer_presentation == ViewerPresentation::Fullscreen
                && !app.fullscreen_embedded_still_active()
            {
                app.depth_setup.invalidate_mode_entry(main);
                app.retire_exclusive_view_for_owner_change("main_live_pass_handoff");
            }
        })
        .expect("independent activation keeps the main context available");
    }

    pub(crate) fn panorama_state(&self) -> Option<&crate::panorama::PanoramaState> {
        self.fs_exclusive_view.panorama_state()
    }

    pub(crate) fn panorama_state_mut(&mut self) -> Option<&mut crate::panorama::PanoramaState> {
        self.fs_exclusive_view.panorama_state_mut()
    }

    /// All live transitions retire the old owner before publishing its replacement.
    pub(crate) fn set_fs_exclusive_view(&mut self, target: FsExclusiveView, _cause: &'static str) {
        let leaving_panorama = matches!(self.fs_exclusive_view, FsExclusiveView::Panorama(_));
        self.fs_canvas_drag.retire_lens();
        if leaving_panorama {
            self.panorama_projection_popup_open = false;
            self.clear_pano_upload();
            self.clear_pano_refinement();
            for (_, request) in self.pano_high_res_pending.drain() {
                request
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        // Depth's Drop cancels the per-request receiver and clears only its context's slot.
        let previous = std::mem::take(&mut self.fs_exclusive_view);
        drop(previous);
        if matches!(target, FsExclusiveView::Depth(_)) {
            self.panorama_intent.on = false;
        }
        self.fs_exclusive_view = target;
        #[cfg(windows)]
        if leaving_panorama && let Some(idx) = self.fullscreen_idx {
            if matches!(self.fs_cache.get(&idx), Some(FsCacheEntry::Video { .. })) {
                self.sync_native_video_panorama_state(idx);
            }
        }
    }

    pub(crate) fn fs_exclusive_view_active(&self, fs_idx: usize) -> Option<ExclusiveViewKind> {
        match &self.fs_exclusive_view {
            FsExclusiveView::Depth(_) => Some(ExclusiveViewKind::Depth),
            FsExclusiveView::PlaneView(_) => Some(ExclusiveViewKind::PlaneView),
            FsExclusiveView::Panorama(_) if self.is_panorama_mode_active(fs_idx) => {
                Some(ExclusiveViewKind::Panorama)
            }
            _ => None,
        }
    }

    pub(crate) fn depth_entry_allowed(&mut self, fs_idx: usize) -> Result<(), DepthEntryBlock> {
        if self.fullscreen_idx != Some(fs_idx) {
            return Err(DepthEntryBlock::NotImageViewer);
        }
        #[cfg(windows)]
        let (other_window_depth, wgpu_available) = (
            self.other_viewer_context_ids().into_iter().any(|context| {
                self.with_viewer_context_ref(context, |owner| {
                    owner.texture_caches().fs_exclusive_view.is_depth()
                })
                .unwrap_or(false)
            }),
            self.wgpu_render_state.is_some(),
        );
        #[cfg(not(windows))]
        let (other_window_depth, wgpu_available) = (false, false);
        crate::depth_parallax::entry_allowed(DepthEntryInput {
            image_viewer: self.fullscreen_idx == Some(fs_idx)
                && self.items.get(fs_idx).is_some_and(GridItem::has_page_data),
            is_video: matches!(
                self.items.get(fs_idx),
                Some(GridItem::Video(_) | GridItem::Audio(_))
            ),
            is_animation: self.fs_entry_is_animated(fs_idx),
            spread_double: matches!(
                self.resolve_visible_spread_pair(fs_idx),
                crate::ui_fullscreen::SpreadPair::Double { .. }
            ),
            continuous_reading: !self.reading_flow.is_paged(),
            other_window_depth,
            wgpu_available,
            editing: self.is_overlay_edit_mode_active(),
            page_turning: self.page_turn_barrier_active() || self.page_turn_completion_active(),
        })
    }

    pub(crate) fn depth_view_may_continue(&mut self, fs_idx: usize) -> bool {
        if !self.fs_exclusive_view.is_depth() {
            return false;
        }
        if let Err(reason) = self.depth_entry_allowed(fs_idx) {
            self.set_fs_exclusive_view(FsExclusiveView::None, "continuation_rejected");
            self.show_feedback_toast(format!("3D 写真を終了しました: {}", reason.message()));
            return false;
        }
        true
    }

    /// Retire before changing hosts, even between eligible presentations. This keeps
    /// pending requests, their viewport wake target and GPU resources in one lifetime.
    #[cfg(windows)]
    pub(crate) fn commit_viewer_presentation(
        &mut self,
        target: ViewerPresentation,
        cause: &'static str,
    ) {
        self.commit_viewer_session(target, self.detached_viewer_independent_active, cause);
    }

    /// Retire on a real session change before publishing its independence flag.
    #[cfg(windows)]
    pub(crate) fn commit_viewer_session(
        &mut self,
        target: ViewerPresentation,
        target_independent: bool,
        cause: &'static str,
    ) {
        if let Some(message) =
            self.commit_viewer_session_deferred_feedback(target, target_independent, cause)
        {
            self.show_feedback_toast(message);
        }
    }

    /// A caller with its own final notification combines this notice before publishing once.
    /// Both immediate and deferred feedback use this single presentation commit.
    #[cfg(windows)]
    pub(crate) fn commit_viewer_presentation_deferred_feedback(
        &mut self,
        target: ViewerPresentation,
        cause: &'static str,
    ) -> Option<String> {
        self.commit_viewer_session_deferred_feedback(
            target,
            self.detached_viewer_independent_active,
            cause,
        )
    }

    #[cfg(windows)]
    pub(crate) fn commit_viewer_session_deferred_feedback(
        &mut self,
        target: ViewerPresentation,
        target_independent: bool,
        cause: &'static str,
    ) -> Option<String> {
        let owner_changed = target != self.viewer_presentation
            || target_independent != self.detached_viewer_independent_active;
        let was_depth = owner_changed && self.retire_exclusive_view_for_owner_change(cause);
        let feedback = if was_depth {
            let message = match self.keymap.first_chord_label(KeyAction::FsDepthParallax) {
                    Some(key) => format!(
                        "3D 写真を終了しました: 表示を切り替えました。{key} キーでもう一度使えます"
                    ),
                    None => "3D 写真を終了しました: 表示を切り替えました。上部の 3D ボタンでもう一度使えます".into(),
            };
            Some(message)
        } else {
            None
        };
        self.viewer_presentation = target;
        self.detached_viewer_independent_active = target_independent;
        feedback
    }

    pub(crate) fn toggle_depth_parallax(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fs_exclusive_view.is_depth() {
            self.set_fs_exclusive_view(FsExclusiveView::None, "depth_toggle_off");
            ctx.request_repaint();
            return;
        }
        if let Err(reason) = self.depth_entry_allowed(fs_idx) {
            self.show_feedback_toast(reason.message().to_owned());
            return;
        }
        // Setup owns its lazy scan. Unknown installation is Checking, never Missing.
        if self.depth_setup.installed_model_path().is_none() {
            if self.depth_setup.entry_check_opens_modal()
                && self.depth_modal_entry_waiting_for_transition()
            {
                self.show_feedback_toast("読み込みが終わってから使ってください".into());
                return;
            }
            self.request_depth_addon_for_entry(
                ctx,
                DepthEntryRequest {
                    context: self.edit_request_owner_context(),
                    items_generation: self.items_generation,
                    fs_idx,
                    item_id: self.item_id(fs_idx),
                },
            );
            return;
        }
        self.enter_depth_parallax(ctx);
    }

    /// ROOT owns polling; only an independent request needs a scoped owner mount.
    pub(crate) fn resume_depth_entry_in_request_context(
        &mut self,
        ctx: &egui::Context,
        request: DepthEntryRequest,
    ) {
        if request.context == self.viewer_context_main() {
            self.resume_depth_entry_after_check(ctx, request);
        } else {
            #[cfg(windows)]
            let _ = self.with_viewer_context(request.context, |owner| {
                owner.resume_depth_entry_after_check(ctx, request);
            });
        }
    }

    pub(crate) fn resume_depth_entry_after_check(
        &mut self,
        ctx: &egui::Context,
        request: DepthEntryRequest,
    ) {
        // Modal keyboard ownership does not block association opens or Remote takeover.
        if self.edit_request_owner_context() != request.context
            || self.items_generation != request.items_generation
            || self.fullscreen_idx != Some(request.fs_idx)
            || self.item_id(request.fs_idx) != request.item_id
        {
            return;
        }
        if !self.viewer_context_live(request.context) {
            return;
        }
        if self.remote_session_blocks_local_control() {
            self.show_feedback_toast("リモート閲覧中のため、3D 写真に切り替えられません".into());
            return;
        }
        if let Err(reason) = self.depth_entry_allowed(request.fs_idx) {
            self.show_feedback_toast(reason.message().to_owned());
            return;
        }
        self.enter_depth_parallax(ctx);
    }

    fn enter_depth_parallax(&mut self, ctx: &egui::Context) {
        let mut state = DepthViewState::new(self.edit_request_owner_context());
        #[cfg(windows)]
        if let Some(renderer) = &self.wgpu_render_state {
            state.attach_renderer(renderer);
        }
        self.set_fs_exclusive_view(FsExclusiveView::Depth(state), "depth_toggle_on");
        self.close_fs_exclusive_conflicts();
        ctx.request_repaint();
    }

    pub(crate) fn toggle_depth_panel(&mut self) {
        if let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view {
            state.panel_visible = !state.panel_visible;
            if !state.panel_visible {
                let key = self
                    .keymap
                    .first_chord_label(crate::keymap::KeyAction::FsToggleMetadata);
                self.show_feedback_toast(match key {
                    Some(key) => {
                        format!("3D 写真のパネルを隠しました。{key} でもう一度表示します")
                    }
                    None => "3D 写真のパネルを隠しました".to_owned(),
                });
            }
        }
    }

    fn depth_panel_toggle_keys(&self) -> Option<String> {
        let labels = self
            .keymap
            .chord_labels(crate::keymap::KeyAction::FsToggleMetadata);
        (!labels.is_empty()).then(|| labels.join(" / "))
    }

    /// Same-pass adoption boundary; neither payload preparation nor geometry is involved.
    pub(crate) fn depth_paint_ready(
        &mut self,
        payload: &crate::gpu_lanczos::FullscreenPaintPayload,
    ) -> bool {
        #[cfg(windows)]
        {
            let ready = self.wgpu_render_state.is_some()
                && matches!(&self.fs_exclusive_view, FsExclusiveView::Depth(state)
                    if state.ready().is_some_and(|resource| payload.source_texture()
                        .is_some_and(|texture| texture.id() == resource.key.texture_id)));
            if !ready && let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view {
                // This pass will paint 2D, so its focus request has no matching source.
                state.pending_focus_click = None;
            }
            ready
        }
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn try_paint_depth(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        payload: &crate::gpu_lanczos::FullscreenPaintPayload,
        transform: &crate::displayed_image_transform::DisplayedImageTransform,
        export_transform: &crate::displayed_image_transform::DisplayedImageTransform,
        canvas: egui::Rect,
        underlay: &crate::ui_fullscreen::FsBgStyle,
    ) -> bool {
        #[cfg(windows)]
        {
            let panel_rect_id = self.depth_panel_rect_id();
            let current = self
                .fullscreen_idx
                .map(|idx| (idx, self.current_edit_result_key(idx)));
            let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
                return false;
            };
            // Consume even on an early return. A request belongs to this pass only.
            let focus_click = state
                .pending_focus_click
                .take()
                .filter(|(_, pass)| *pass == ctx.cumulative_pass_nr())
                .map(|(pos, _)| pos);
            let Some(render) = self.wgpu_render_state.as_ref() else {
                return false;
            };
            let Some(resource) = state.ready() else {
                return false;
            };
            if payload
                .source_texture()
                .is_none_or(|texture| texture.id() != resource.key.texture_id)
            {
                return false;
            }
            let panel = state
                .panel_visible
                .then(|| ctx.data(|data| data.get_temp::<egui::Rect>(panel_rect_id)))
                .flatten();
            let (pointer, dt, visible) = ctx.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.stable_dt,
                    input.viewport().minimized != Some(true),
                )
            });
            let settings = &self.settings.depth_parallax;
            let (offset, moving) = if settings.motion == crate::depth_parallax::MotionKind::Mouse {
                let moving = state.mouse.update(
                    pointer,
                    crate::depth_parallax_wgpu::depth_visible_center(transform, canvas),
                    canvas,
                    pointer.is_some_and(|point| panel.is_some_and(|rect| rect.contains(point))),
                    settings.strength,
                    dt,
                );
                (state.mouse.current, moving)
            } else {
                (
                    crate::depth_parallax::motion_offset(
                        settings.motion,
                        settings.strength,
                        state.started_at.elapsed().as_secs_f32() / settings.period_secs,
                    ),
                    true,
                )
            };
            let mut params = crate::depth_parallax_wgpu::DepthRenderParams::from_settings(
                settings,
                resource.depth.median,
                offset,
                transform.uv_rect,
                transform.rotation.degrees() as u32 / 90,
                transform.paint_rect.aspect_ratio(),
            );
            let mapping = crate::depth_parallax_wgpu::DepthScreenMapping::from_transform(transform);
            let now = std::time::Instant::now();
            if let Some(focus) = focus_click.and_then(|pos| {
                crate::depth_parallax_wgpu::focus_depth_at(
                    pos,
                    mapping,
                    params,
                    resource.depth.width,
                    resource.depth.height,
                    &resource.depth.values,
                )
            }) {
                self.settings.depth_parallax.focus = focus;
                self.settings.depth_parallax.normalize();
                self.settings.save();
                params.focus = self.settings.depth_parallax.focus;
                state.focus_highlight = Some(now);
            }
            let intensity =
                crate::depth_parallax::focus_highlight_intensity(state.focus_highlight, now);
            params = params.with_focus_highlight(intensity);
            if (moving || intensity > 0.0)
                && self.window_visible
                && visible
                && self.depth_export_operation.is_none()
            {
                ctx.request_repaint();
            }
            let callback = crate::depth_parallax_wgpu::DepthShaderCallback {
                context: resource.context,
                key: resource.key,
                resource_id: resource.resource_id,
                params,
                mapping,
                target_format: render.target_format,
            };
            state.publish(&mut render.renderer.write().callback_resources);
            crate::ui_fullscreen::paint_image_underlay(painter, transform.paint_quad(), underlay);
            super::depth_export::record_display(
                ctx,
                resource.context,
                resource.resource_id,
                self.items_generation,
                export_transform,
                underlay,
            );
            let callback_rect = egui::Rect::from_points(&transform.paint_quad().positions)
                .intersect(painter.clip_rect());
            if callback_rect.is_positive() {
                painter.add(egui::Shape::Callback(
                    egui_wgpu::Callback::new_paint_callback(callback_rect, callback),
                ));
            }
            if let Some((fs_idx, edit_key)) = current {
                state.settled = Some(DepthSettled {
                    fs_idx,
                    edit_key,
                    key: resource.key,
                    outcome: DepthSettledOutcome::Presented,
                });
            }
            return true;
        }
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn depth_wait_status(&self) -> Option<DepthWaitStatus> {
        depth_wait_status(&self.fs_exclusive_view, &self.ai_runtime_init.snapshot())
    }

    pub(crate) fn depth_panel_status(&self) -> String {
        if let FsExclusiveView::Depth(state) = &self.fs_exclusive_view
            && let crate::depth_parallax::DepthSourcePhase::Failed { reason, .. } = &state.source
        {
            return format!("失敗しました ({reason})");
        }
        self.depth_wait_status()
            .map_or_else(String::new, |status| status.panel_text().to_owned())
    }

    pub(crate) fn draw_depth_panel(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> Option<crate::ui_depth_parallax::DepthPanelResponse> {
        if self.depth_entry_allowed(fs_idx).is_err() {
            return None;
        }
        let FsExclusiveView::Depth(state) = &self.fs_exclusive_view else {
            return None;
        };
        if !state.panel_visible {
            return None;
        }
        let status = self.depth_panel_status();
        let export_enabled = self.depth_export_available(ctx);
        let panel_toggle_hint = self
            .depth_panel_toggle_keys()
            .map(|keys| format!("{keys}: パネルを隠す・出す"));
        let response = crate::ui_depth_parallax::draw_depth_parallax_panel_with_export(
            ctx,
            &mut self.settings.depth_parallax,
            &status,
            export_enabled,
            panel_toggle_hint,
            None,
        );
        ctx.data_mut(|data| data.insert_temp(self.depth_panel_rect_id(), response.rect));
        if response.changed {
            self.settings.depth_parallax.normalize();
            self.settings.save();
        }
        if response.focus_adjusting
            && let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view
        {
            state.focus_highlight = Some(std::time::Instant::now());
            ctx.request_repaint();
        }
        if response.close_requested {
            self.set_fs_exclusive_view(FsExclusiveView::None, "depth_panel_close");
        }
        Some(response)
    }

    #[cfg(windows)]
    pub(crate) fn advance_depth_source(
        &mut self,
        ctx: &egui::Context,
        material: DepthSourceMaterial,
    ) {
        // A normal show pass resumes polling and adoption; hidden passes stay idle.
        if !self.window_visible || ctx.input(|input| input.viewport().minimized == Some(true)) {
            return;
        }
        let Some(render) = self.wgpu_render_state.clone() else {
            return;
        };
        let backing = match &material {
            DepthSourceMaterial::Ready { texture } => render
                .renderer
                .read()
                .texture(&texture.id())
                .and_then(|texture| texture.texture.clone()),
            _ => None,
        };
        self.ensure_ai_runtime();
        let runtime = self.ai_runtime_init.clone();
        let activity = self.local_ai_activity_lease();
        let context = self.edit_request_owner_context();
        let viewport = ctx.viewport_id();
        let input_seq = self.input_seq;
        let current_target = self.fullscreen_idx.map(|idx| self.depth_next_target(idx));
        let current_edit = self
            .fullscreen_idx
            .map(|idx| self.current_edit_result_key(idx));
        let selected_key = match &material {
            DepthSourceMaterial::Ready { texture } => {
                Some(crate::depth_estimation::DepthSourceKey {
                    texture_id: texture.id(),
                })
            }
            DepthSourceMaterial::NotYet(_) => None,
        };
        let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
            return;
        };
        // Sole production writer: capture the normal selector's value for
        // this visible pass, including NotYet while a Failed key is retained.
        state.current_material = selected_key;
        let failed_before = match &state.source {
            crate::depth_parallax::DepthSourcePhase::Failed { key, .. } => Some(*key),
            _ => None,
        };
        let selected = match material {
            DepthSourceMaterial::Ready { texture } => match backing {
                Some(backing) => state.select_source_for_target(
                    &render.device,
                    &render.queue,
                    &backing,
                    crate::depth_estimation::DepthSourceKey {
                        texture_id: texture.id(),
                    },
                    current_target,
                ),
                None => {
                    state.wait_for_source(DepthSourceWait::TexturePending);
                    // Managed textures are registered by the renderer after UI.
                    // Consume that registration next pass even when the pointer is idle.
                    ctx.request_repaint_of(viewport);
                    Ok(())
                }
            },
            DepthSourceMaterial::NotYet(reason) => {
                state.wait_for_source(reason);
                Ok(())
            }
        };
        if let Err(error) = selected {
            crate::logger::log(format!("[Depth] {error}"));
        }
        state.advance(
            &render.device,
            &render.queue,
            ctx,
            viewport,
            |key, input, guide| {
                crate::depth_estimation::DepthWorker::shared()?.submit(
                    context,
                    viewport,
                    key,
                    input,
                    guide,
                    runtime,
                    activity,
                    ctx.clone(),
                    input_seq,
                )
            },
        );
        if let crate::depth_parallax::DepthSourcePhase::Failed { key, reason } = &state.source {
            if selected_key == Some(*key)
                && let (Some(target), Some(edit_key)) = (current_target, current_edit)
            {
                state.settled = Some(DepthSettled {
                    fs_idx: target.idx,
                    edit_key,
                    key: *key,
                    outcome: DepthSettledOutcome::Failed,
                });
            }
            if failed_before != Some(*key) {
                let message = format!("3D 写真の処理に失敗しました: {reason}");
                self.show_feedback_result(message, crate::feedback::FeedbackKind::Error);
            }
        }
        // Material selection, supersession and current adoption above must all
        // precede this pass's evidence check and any next-page submission.
        self.advance_depth_next(ctx, current_target, selected_key);
    }

    #[cfg(windows)]
    fn advance_depth_next(
        &mut self,
        ctx: &egui::Context,
        current: Option<DepthNextTarget>,
        selected: Option<crate::depth_estimation::DepthSourceKey>,
    ) {
        use crate::ui_fullscreen::SlideshowNextTarget;
        let desired = if self.depth_slideshow_prefetch_active()
            && self.depth_current_settled()
            && self.depth_export_operation.is_none()
            && self.fs_transparent_bg_mode == 0
        {
            self.fullscreen_idx
                .and_then(|idx| match self.slideshow_next_target(idx) {
                    SlideshowNextTarget::InItems { idx } => Some(self.depth_next_target(idx)),
                    SlideshowNextTarget::Other => None,
                })
        } else {
            None
        };
        let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
            return;
        };
        if !self.slideshow_playing
            || state.next.as_ref().is_some_and(|slot| {
                Some(slot.target) != desired
                    && !(Some(slot.target) == current && selected.is_none_or(|key| key == slot.key))
            })
        {
            state.next = None;
        }
        let Some(target) = desired else { return };
        let Some(material) = self.resident_final_prepare_material(ctx, target.idx) else {
            return;
        };
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: material.source_texture().id(),
        };
        let Some(render) = self.wgpu_render_state.clone() else {
            return;
        };
        let backing = render
            .renderer
            .read()
            .texture(&key.texture_id)
            .and_then(|texture| texture.texture.clone());
        let Some(backing) = backing else { return };
        let context = self.edit_request_owner_context();
        let runtime = self.ai_runtime_init.clone();
        let activity = self.local_ai_activity_lease();
        let viewport = ctx.viewport_id();
        let input_seq = self.input_seq;
        let FsExclusiveView::Depth(state) = &mut self.fs_exclusive_view else {
            return;
        };
        if state.lru.contains(key) {
            return;
        }
        if state
            .next
            .as_ref()
            .is_none_or(|slot| slot.target != target || slot.key != key)
        {
            state.start_next(&render.device, &render.queue, &backing, target, key);
            crate::depth_parallax::record_next_event(
                "start",
                target,
                key,
                input_seq,
                state.next.as_ref().unwrap().started_at,
            );
        }
        state.advance_next(
            &render.device,
            &render.queue,
            ctx,
            viewport,
            input_seq,
            |key, input, guide| {
                crate::depth_estimation::DepthWorker::shared()?.submit(
                    context,
                    viewport,
                    key,
                    input,
                    guide,
                    runtime,
                    activity,
                    ctx.clone(),
                    input_seq,
                )
            },
        );
    }
}
