//! Read-only slideshow readiness. Only existing, current, uncancelled producers
//! may hold a page; eligibility for work is never evidence that work started.
use super::depth_view::{DepthPageProgress, DepthPageRole};
use super::final_prepare::{FinalEffectReady, FinalEffectState};
use super::*;
use crate::depth_parallax::{DepthNextPhase, DepthSourcePhase};
use crate::fs_exclusive_view::FsExclusiveView;

impl App {
    pub(crate) fn depth_page_progress(
        &self,
        ctx: &egui::Context,
        idx: usize,
        role: DepthPageRole,
    ) -> DepthPageProgress {
        use DepthPageProgress::{Producing, Ready, Unavailable};
        let FsExclusiveView::Depth(state) = &self.fs_exclusive_view else {
            return Unavailable;
        };
        if idx >= self.items.len()
            || state.context != self.edit_request_owner_context()
            || (role == DepthPageRole::Current && self.fullscreen_idx != Some(idx))
        {
            return Unavailable;
        }
        // Stage 1: current requires an actual presentation/failure receipt.
        if role == DepthPageRole::Current && self.depth_current_settled() {
            return Ready;
        }
        let material = self.resident_final_prepare_material(ctx, idx);
        let key = material
            .as_ref()
            .map(|material| crate::depth_estimation::DepthSourceKey {
                texture_id: material.source_texture().id(),
            });
        if let Some(key) = key {
            if role == DepthPageRole::Next && state.lru.contains(key) {
                return Ready;
            }
            // Stage 2: a key belonging to the page's present material, not an
            // old phase left behind by navigation, editing or RAW refinement.
            if role == DepthPageRole::Current && state.source.key() == Some(key) {
                match &state.source {
                    DepthSourcePhase::ReadingBack { .. } => return Producing,
                    DepthSourcePhase::Estimating { pending, .. } if !pending.is_cancelled() => {
                        return Producing;
                    }
                    _ => {}
                }
            }
            if role == DepthPageRole::Next
                && let Some(next) = &state.next
                && next.target.idx == idx
                && next.target.item_id == self.item_id(idx)
                && next.target.items_generation == self.items_generation
                && next.key == key
            {
                match &next.phase {
                    DepthNextPhase::ReadingBack { .. } => return Producing,
                    DepthNextPhase::Estimating { pending } if !pending.is_cancelled() => {
                        return Producing;
                    }
                    // Stage 3: this source already failed, so do not wait on
                    // lower pipeline work that cannot repair this exact key.
                    DepthNextPhase::Failed { .. } => return Unavailable,
                    _ => {}
                }
            }
            // Stage 4: only a managed texture has the end-of-frame registration
            // guarantee. A user texture with no backing has no such producer.
            if matches!(key.texture_id, egui::TextureId::Managed(_))
                && ctx.tex_manager().read().meta(key.texture_id).is_some()
                && self.wgpu_render_state.as_ref().is_some_and(|render| {
                    render
                        .renderer
                        .read()
                        .texture(&key.texture_id)
                        .is_none_or(|texture| texture.texture.is_none())
                })
            {
                return Producing;
            }
        }
        // Stage 5: do not use fs_page_load_state's RAW PreviewAbsent projection.
        // Pending owns a live scheduler ticket; backlog owns an accepted reply
        // after its ticket was disarmed and removed. Both are generation scoped.
        if (self
            .fs_pending
            .accepts_generation(idx, self.items_generation)
            && self
                .fs_pending
                .get(&idx)
                .is_some_and(|pending| !pending.ticket.cancel_token().load(Ordering::Relaxed)))
            || (self
                .fs_upload_backlog
                .accepts_generation(idx, self.items_generation)
                && self.fs_upload_backlog_contains(idx))
            || self.depth_raw_development_pending(idx)
        {
            return Producing;
        }
        // Stages 6, 7, 8 use precisely the same identity/cancellation preflight
        // as adoption. No diagnostic record or CPU admission guess is consulted.
        let final_pending = self.final_effect_pending.iter().find(|(key, pending)| {
            key.edit_key.idx == idx && self.final_prepare_pending_is_current(**key, pending)
        });
        if final_pending
            .is_some_and(|(_, pending)| matches!(pending.state, FinalEffectState::Computing(_)))
        {
            return Producing;
        }
        // Stage 6b: the ordinary selector's annotation, local/conceal and erase
        // producers. These own NotYet even when D3 has never been started.
        if self.depth_comic_bake_pending(idx)
            || self.current_edit_stage_producer_pending(idx)
            || self.commit_pending_matches_current_erase_key(idx)
        {
            return Producing;
        }
        if let Some((key, pending)) = final_pending {
            return if self.depth_final_registration_admitted(ctx, *key, pending) {
                Producing
            } else {
                Unavailable
            };
        }
        // Stage 9: an AI request really exists, with the current input and AI
        // parameters. Eligibility for AI or an initialization hint is not work.
        let edit_key = self.current_edit_result_key(idx);
        let params = self.effective_params(idx);
        if let Some(size) = self.final_ai_prefetch_input_size(idx, params)
            && let Some(key) = self.final_ai_key_for_pixels(edit_key, size, params)
            && self
                .final_ai_pending
                .get(&key)
                .is_some_and(|pending| !pending.cancel.load(Ordering::Relaxed))
        {
            return Producing;
        }
        // Stage 10 includes NeedsLoad/LoadFailed, CPU reservation refusal and
        // an unstarted D3/AI job. Never create work from this classifier.
        Unavailable
    }

    fn depth_comic_bake_pending(&self, idx: usize) -> bool {
        let Some(pending) = self.comic_bake_pending.get(&idx) else {
            return false;
        };
        let preview_scale = if self.text_mode {
            self.settings.text_preview_scale.clamp(1, 8)
        } else {
            1
        };
        !pending.cancel.load(Ordering::Relaxed)
            && pending.items_gen == self.items_generation
            && pending.comic_gen == self.comic_generation
            && pending.preview_scale == preview_scale
            && self
                .exact_complete_final_composite_entry(idx)
                .is_some_and(|(_, entry)| {
                    Arc::ptr_eq(&pending.base, &entry.pixels) && pending.dims == entry.pixels.size
                })
    }

    fn depth_final_registration_admitted(
        &self,
        ctx: &egui::Context,
        key: FinalCompositeKey,
        pending: &FinalEffectPending,
    ) -> bool {
        let options = if pending.nearest_sampler {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        let allocations = match &pending.state {
            FinalEffectState::CpuReady(FinalEffectReady::Edited(_))
            | FinalEffectState::Registering(_) => {
                let Some(allocations) =
                    self.prepared_edit_remaining_allocations(ctx, key, &pending.state)
                else {
                    return false;
                };
                allocations
            }
            FinalEffectState::CpuReady(FinalEffectReady::Prepared(ready)) => {
                if !self.final_prepare_is_speculative_allocation(pending)
                    || self
                        .shared_prepared_final_texture(ctx, key, ready, pending.nearest_sampler)
                        .is_some()
                {
                    return true;
                }
                vec![(ready.upload.size, options)]
            }
            FinalEffectState::CpuReady(FinalEffectReady::Legacy(FinalCompositeResult::Ready {
                pixels,
                ..
            })) => {
                if !self.final_prepare_is_speculative_allocation(pending) {
                    return true;
                }
                vec![(gpu_clamped_dimensions(pixels.size), options)]
            }
            _ => return false,
        };
        final_prepare_budget::speculative_vram_allocations_admitted(
            self.vram_accounting_snapshot().total_bytes(),
            allocations,
            self.vram_pool_budget().pool_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::depth_parallax::{DepthNextSlot, DepthNextTarget, DepthViewState};
    use DepthPageProgress::{Producing, Unavailable};

    fn fixture(ctx: &egui::Context) -> AppTestEnvForTest {
        let mut app = setup_app_for_test();
        app.items = vec![
            GridItem::Image("progress-current.jpg".into()),
            GridItem::Image("progress-next.jpg".into()),
        ];
        app.image_metas = vec![None; 2];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.rebuild_visible_indices();
        app.fullscreen_idx = Some(0);
        app.reading_flow = crate::settings::ReadingFlow::Paged;
        app.spread_mode = crate::settings::SpreadMode::Single;
        app.fs_exclusive_view =
            FsExclusiveView::Depth(DepthViewState::new(app.edit_request_owner_context()));
        app.wgpu_render_state = None;
        for idx in 0..2 {
            let pixels = Arc::new(egui::ColorImage::filled([4, 4], egui::Color32::WHITE));
            let tex = ctx.load_texture(
                format!("progress-{idx}"),
                pixels.clone(),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            app.bind_display_texture_identity(&tex, app.item_id(idx));
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_metadata: None,
                    pdf_rerender_failures: Vec::new(),
                    tex,
                    pixels,
                    source_dims: Some([4, 4]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
        }
        app
    }

    fn next(app: &App, ctx: &egui::Context) -> DepthPageProgress {
        app.depth_page_progress(ctx, 1, DepthPageRole::Next)
    }

    fn next_key(app: &App, ctx: &egui::Context) -> crate::depth_estimation::DepthSourceKey {
        crate::depth_estimation::DepthSourceKey {
            texture_id: app
                .resident_final_prepare_material(ctx, 1)
                .unwrap()
                .source_texture()
                .id(),
        }
    }

    #[test]
    fn unstarted_source_and_d3_work_are_terminal_without_mutation() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        for enabled in [false, true] {
            app.settings.page_turn_animation_enabled = enabled;
            app.slideshow_playing = true;
            assert_eq!(
                next(&app, &ctx),
                Unavailable,
                "resident source alone has no depth job"
            );
            app.fs_cache.remove(&1);
            assert_eq!(app.fs_page_load_state(1), FsPageLoadState::NeedsLoad);
            assert_eq!(next(&app, &ctx), Unavailable);
            app.fs_cache.insert(1, FsCacheEntry::Failed);
            assert_eq!(next(&app, &ctx), Unavailable);
        }
        assert!(app.fs_pending.is_empty());
        assert!(app.final_effect_pending.is_empty());
        assert!(app.final_ai_pending.is_empty());
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            Unavailable
        );
    }

    #[test]
    fn depth_estimation_must_match_page_texture_generation_and_cancellation() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let key = next_key(&app, &ctx);
        let (pending, complete) = crate::depth_estimation::DepthPending::deferred_for_test(
            app.edit_request_owner_context(),
            1,
            key,
        );
        let cancel = pending.cancellation_token_for_test();
        let target = DepthNextTarget {
            idx: 1,
            item_id: app.item_id(1),
            items_generation: app.items_generation,
        };
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.next = Some(DepthNextSlot {
            target,
            key,
            phase: DepthNextPhase::Estimating { pending },
        });
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Release);
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "cancelled try_result never completes"
        );
        cancel.store(false, Ordering::Release);
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.next.as_mut().unwrap().target.items_generation += 1;
        assert_eq!(next(&app, &ctx), Unavailable);
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.next.as_mut().unwrap().target = target;
        state.next.as_mut().unwrap().key.texture_id = egui::TextureId::Managed(u64::MAX);
        assert_eq!(next(&app, &ctx), Unavailable);
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.next.as_mut().unwrap().key = key;
        drop(complete);
        let DepthNextPhase::Estimating { pending } = &state.next.as_ref().unwrap().phase else {
            unreachable!()
        };
        assert!(pending.try_result(state.context, key).unwrap().is_err());
        state.next.as_mut().unwrap().phase = DepthNextPhase::Failed {
            reason: "worker disconnected".into(),
        };
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "terminal failure cannot hold a slideshow"
        );
    }

    #[test]
    fn load_wait_uses_issued_ticket_and_accepted_backlog_not_state_projection() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.fs_cache.remove(&1);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.fs_pending.insert(
            1,
            FsPendingValue::new(cancel.clone(), rx, 1, FsLoadPurpose::Prefetch),
        );
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        cancel.store(false, Ordering::Relaxed);
        app.items_generation += 1;
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "stale map is not this page's job"
        );
        app.items_generation -= 1;
        drop(tx);
        app.cancel_fs_page_load(1);
        assert_eq!(next(&app, &ctx), Unavailable);
        app.fs_upload_backlog.push(FsUploadResult::new(
            1,
            FsLoadResult::Static {
                ci: egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
                source_dims: [4, 4],
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
            1,
            FsLoadPurpose::Prefetch,
        ));
        assert_eq!(next(&app, &ctx), Producing);
        app.fs_upload_backlog.clear();
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "accepted reply no longer owns an upload"
        );
    }

    #[test]
    fn raw_preview_absence_waits_only_for_an_issued_uncancelled_develop_job() {
        use super::super::raw_page_store::{
            RawDevelopPhase, RawInstalledStage, RawPageRecord, RawPageState, RawPresentation,
            RawPreviewPhase, RawSourceIdentity,
        };
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.items[1] = GridItem::Image("progress-next.dng".into());
        app.fs_cache.remove(&1);
        let item_key = app.page_path_key(1).unwrap();
        app.raw_pages.pages.insert(
            1,
            RawPageRecord::Page(RawPageState {
                source: RawSourceIdentity {
                    item_key,
                    path: "progress-next.dng".into(),
                    size: 1,
                    mtime_ticks: 1,
                },
                developed_dims: Some([4, 4]),
                stage: RawInstalledStage::PreviewAbsent,
                preview: RawPreviewPhase::Absent,
                develop: Arc::new(Mutex::new(RawDevelopPhase::Idle)),
                preview_started_at: std::time::Instant::now(),
                develop_started_at: std::time::Instant::now(),
                presented: RawPresentation::Nothing,
            }),
        );
        assert_eq!(app.fs_page_load_state(1), FsPageLoadState::LoadPending);
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "projected pending has no producer"
        );
        let scheduler = FsPageLoadScheduler::new();
        let ticket = scheduler.request(
            app.fs_page_load_context_serial(),
            1,
            FsPageLoadPriority::Normal,
            FsPageLoadContract::Sequential,
            None,
            0,
        );
        let cancel = ticket.cancel_token();
        let phase = app.raw_pages.page(1).unwrap().develop.clone();
        *phase.lock().unwrap() = RawDevelopPhase::Preparing {
            request_id: 1,
            cancel: Arc::new(Mutex::new(ticket)),
            highest_priority: crate::raw::RawPriority::Normal,
            brightness: app.settings.raw_brightness,
        };
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        *phase.lock().unwrap() = RawDevelopPhase::Done;
        assert_eq!(next(&app, &ctx), Unavailable);
        let _held = phase.lock().unwrap();
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "query never waits for the publication lock"
        );
    }

    #[test]
    fn final_effect_current_request_terminates_on_cancel_or_receiver_completion() {
        use super::super::final_prepare::{FinalEffectOrigin, FinalEffectReceiver};
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let edit_key = app.current_edit_result_key(1);
        let key = app.final_composite_key_for_pixels(edit_key, [4, 4], app.effective_params(1));
        let (tx, rx) = mpsc::channel();
        let items_generation = app.items_generation;
        app.final_effect_pending.insert(
            key,
            FinalEffectPending {
                diagnostic: None,
                pdf_metadata: None,
                canonical_dims: None,
                source_size: [4, 4],
                cancel: Arc::new(AtomicBool::new(false)),
                state: FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)),
                items_generation,
                output_complete: true,
                nearest_sampler: false,
                origin: FinalEffectOrigin::LegacyDisplay,
                gpu_diagnostics: Default::default(),
            },
        );
        assert_eq!(next(&app, &ctx), Producing);
        app.final_effect_pending[&key]
            .cancel
            .store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        app.final_effect_pending[&key]
            .cancel
            .store(false, Ordering::Relaxed);
        app.input_generation.insert(1, edit_key.source_gen + 1);
        assert_eq!(next(&app, &ctx), Unavailable);
        app.input_generation.insert(1, edit_key.source_gen);
        tx.send(FinalCompositeResult::Cancelled).unwrap();
        app.final_effect_pending
            .get_mut(&key)
            .unwrap()
            .state
            .receive()
            .unwrap();
        assert_eq!(next(&app, &ctx), Unavailable);
        app.poll_final_effects(&ctx);
        assert!(!app.final_effect_pending.contains_key(&key));
        assert_eq!(next(&app, &ctx), Unavailable);
    }

    #[test]
    fn comic_bake_wait_checks_exact_base_generation_and_worker_terminal() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let pixels = app.current_raw_source_pixels(1).unwrap();
        let texture = app
            .resident_final_prepare_material(&ctx, 1)
            .unwrap()
            .source_texture()
            .clone();
        let key = app.final_composite_key_for_pixels(
            app.current_edit_result_key(1),
            [4, 4],
            app.effective_params(1),
        );
        app.final_composite_cache.insert(
            key,
            FinalCompositeEntry::raster(
                None,
                [4, 4],
                pixels.clone(),
                texture,
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
                true,
            ),
        );
        let basis = app.final_composite_cache[&key].display_basis;
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.comic_pages.insert(1);
        let comic_gen = app.comic_generation;
        let items_gen = app.items_generation;
        app.comic_bake_pending.insert(
            1,
            ComicBakePending {
                display_basis: basis,
                comic_gen,
                items_gen,
                base: pixels.clone(),
                dims: [4, 4],
                preview_scale: 1,
                started: std::time::Instant::now(),
                cancel: cancel.clone(),
                rx,
            },
        );
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        cancel.store(false, Ordering::Relaxed);
        app.comic_generation += 1;
        assert_eq!(next(&app, &ctx), Unavailable);
        app.comic_generation -= 1;
        app.comic_bake_pending.get_mut(&1).unwrap().base = Arc::new((*pixels).clone());
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "same dimensions cannot substitute for the actual base"
        );
        app.comic_bake_pending.get_mut(&1).unwrap().base = pixels;
        drop(tx);
        app.poll_comic_bake(&ctx);
        assert!(!app.comic_bake_pending.contains_key(&1));
        assert_eq!(next(&app, &ctx), Unavailable);
    }

    #[test]
    fn remaining_registration_budget_uses_all_allocations_and_actual_sampler() {
        use super::super::final_prepare_budget::{
            speculative_vram_admitted, speculative_vram_allocations_admitted,
        };
        let size = [8, 4];
        let options = DISPLAY_IMAGE_TEXTURE_OPTIONS;
        let bytes = crate::vram_budget::rgba8_texels_to_bytes(
            crate::vram_budget::texture_size_texels(size, options.mipmap_mode.is_some()),
        );
        assert!(speculative_vram_admitted(
            100,
            size,
            options,
            Some(100 + bytes)
        ));
        assert!(
            !speculative_vram_allocations_admitted(100, [(size, options); 2], Some(100 + bytes)),
            "first fits but second stalls forever"
        );
        assert!(speculative_vram_allocations_admitted(
            100,
            [(size, options); 2],
            Some(100 + 2 * bytes)
        ));
        assert!(
            speculative_vram_allocations_admitted(
                100 + bytes,
                [(size, options)],
                Some(100 + 2 * bytes)
            ),
            "registered first texture is already resident"
        );
        assert!(
            speculative_vram_allocations_admitted(100 + 2 * bytes, [], Some(0)),
            "shared/already registered handles add no allocation even when over budget"
        );
        assert!(speculative_vram_allocations_admitted(
            0,
            [(size, egui::TextureOptions::NEAREST)],
            Some(8 * 4 * 4)
        ));
        assert!(
            !speculative_vram_allocations_admitted(0, [(size, options)], Some(8 * 4 * 4)),
            "mip storage uses the real registration policy"
        );
    }

    #[test]
    fn current_estimation_does_not_count_cancelled_or_unmatched_sources() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let key = crate::depth_estimation::DepthSourceKey {
            texture_id: app
                .resident_final_prepare_material(&ctx, 0)
                .unwrap()
                .source_texture()
                .id(),
        };
        let (pending, complete) = crate::depth_estimation::DepthPending::deferred_for_test(
            app.edit_request_owner_context(),
            2,
            key,
        );
        let cancel = pending.cancellation_token_for_test();
        let FsExclusiveView::Depth(state) = &mut app.fs_exclusive_view else {
            unreachable!()
        };
        state.source = DepthSourcePhase::Estimating { key, pending };
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            Producing
        );
        cancel.store(true, Ordering::Release);
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            Unavailable
        );
        cancel.store(false, Ordering::Release);
        app.adjustment_page_params.entry(0).or_default().brightness = 15.0;
        assert_eq!(
            app.depth_page_progress(&ctx, 0, DepthPageRole::Current),
            Unavailable,
            "old texture is not the new adjusted input"
        );
        drop(complete);
    }

    #[test]
    fn local_and_conceal_materializers_wait_only_for_current_uncancelled_request() {
        for conceal in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            let request = EditMaterializeRequest {
                items_generation: app.items_generation,
                input_seq: 1,
                kind: if conceal {
                    EditMaterializeKind::Conceal {
                        key: app.current_edit_result_key(1),
                        purpose: ConcealMaterializePurpose::Display,
                    }
                } else {
                    EditMaterializeKind::Local {
                        key: app.current_local_adjust_key(1),
                        continuation: LocalMaterializeContinuation::Display,
                    }
                },
            };
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            app.local_adjust_pending.insert(
                1,
                LocalAdjustRenderPending {
                    request,
                    cancel: cancel.clone(),
                    rx,
                    ready: None,
                },
            );
            assert_eq!(next(&app, &ctx), Producing);
            cancel.store(true, Ordering::Relaxed);
            assert_eq!(next(&app, &ctx), Unavailable);
            cancel.store(false, Ordering::Relaxed);
            app.input_generation.insert(1, 1);
            assert_eq!(next(&app, &ctx), Unavailable);
            app.input_generation.remove(&1);
            drop(tx);
            app.poll_local_adjust_render(&ctx);
            assert!(!app.local_adjust_pending.contains_key(&1));
            assert_eq!(
                next(&app, &ctx),
                Unavailable,
                "worker disconnection has a terminal owner path"
            );
        }
    }

    #[test]
    fn erase_materializer_wait_ends_at_cancel_generation_change_and_disconnect() {
        use crate::ui_erase::{
            EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey, EraseInpaintProgress,
            EraseInpaintPurpose,
        };
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let key = EraseInpaintPendingKey {
            idx: 1,
            kind: EraseInpaintKind::Commit,
        };
        let (tx, rx) = mpsc::channel();
        let (_, progress_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let items_generation = app.items_generation;
        let path_key = app.page_path_key(1);
        app.erase_inpaint_pending.insert(
            key,
            EraseInpaintPending {
                display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                    [4, 4],
                    crate::source_coverage::SourceCoverage::FullSource,
                ),
                idx: 1,
                items_generation,
                path_key,
                rx,
                progress_rx,
                progress: EraseInpaintProgress::Preparing,
                cancel: cancel.clone(),
                started_at: std::time::Instant::now(),
                input_generation: 0,
                mask_generation: 0,
                purpose: EraseInpaintPurpose::Commit {
                    origin: "depth-progress-test",
                },
            },
        );
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        cancel.store(false, Ordering::Relaxed);
        app.erase_mask_generation.insert(1, 1);
        assert_eq!(next(&app, &ctx), Unavailable);
        app.erase_mask_generation.remove(&1);
        drop(tx);
        app.poll_erase_inpaint(&ctx);
        assert!(!app.erase_inpaint_pending.contains_key(&key));
        assert_eq!(next(&app, &ctx), Unavailable);
    }

    #[test]
    fn final_ai_wait_is_current_key_only_and_result_retires_it() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.adjustment_page_params
            .entry(1)
            .or_default()
            .upscale_model = Some("realesr_general_v3".into());
        let key = app
            .final_ai_key_for_pixels(
                app.current_edit_result_key(1),
                [4, 4],
                app.effective_params(1),
            )
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let epoch = app.retained_final_ai_epoch;
        app.final_ai_pending.insert(
            key,
            FinalAiPending {
                feedback_request: Default::default(),
                job_id: 731,
                cancel: cancel.clone(),
                retained_key: None,
                retained_epoch: epoch,
                allow_retained_orphan: false,
                cancel_on_drop: true,
            },
        );
        let (tx, rx) = mpsc::channel();
        app.final_ai_rx = Some(rx);
        assert_eq!(next(&app, &ctx), Producing);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(next(&app, &ctx), Unavailable);
        cancel.store(false, Ordering::Relaxed);
        app.adjustment_page_params.get_mut(&1).unwrap().brightness = 1.0;
        assert_eq!(
            next(&app, &ctx),
            Unavailable,
            "AI hash must match, not merely idx"
        );
        app.adjustment_page_params.get_mut(&1).unwrap().brightness = 0.0;
        assert_eq!(next(&app, &ctx), Producing);
        tx.send(FinalAiResult::Cancelled {
            diagnostic: None,
            job_id: 731,
            key,
        })
        .unwrap();
        app.poll_final_ai(&ctx);
        assert!(!app.final_ai_pending.contains_key(&key));
        assert_eq!(next(&app, &ctx), Unavailable);
    }

    #[test]
    fn cpu_reservation_denial_does_not_create_a_slideshow_wait() {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        app.adjustment_page_params.entry(1).or_default().brightness = 15.0;
        app.final_prepare_cpu = final_prepare_cpu::FinalPrepareCpuBudget::with_limits(0, 0);
        assert!(!final_prepare::tests::start(&mut app, &ctx, 1));
        assert!(app.final_effect_pending.is_empty());
        assert_eq!(next(&app, &ctx), Unavailable);
    }

    #[test]
    fn edited_registration_tracks_shared_and_already_registered_handles() {
        use super::super::final_prepare_budget::FinalPrepareAdoption;
        use crate::app::final_prepare::tests::{begin_registration_frame, wait_ready};
        use crate::ui_fullscreen::FinalUnitPosition;
        for shared in [false, true] {
            let ctx = egui::Context::default();
            let mut app = fixture(&ctx);
            app.settings.gpu_memory_percent = 0;
            app.settings.page_turn_animation_enabled = true;
            if !shared {
                app.adjustment_page_params.entry(1).or_default().brightness = 15.0;
            }
            app.comic_pages.insert(1);
            let page_key = app.page_path_key(1).unwrap();
            app.comic_docs.insert(
                page_key,
                vec![comic_core::AnnotationObject::new_bubble(
                    1,
                    (2.0, 2.0),
                    comic_core::BubbleObject {
                        shape: comic_core::BubbleShape::RoundRect {
                            half_w: 1.0,
                            half_h: 1.0,
                            corner_px: 0.0,
                        },
                        fill: Some(comic_core::Rgba::new(255, 0, 0, 255)),
                        auto_size: false,
                        ..Default::default()
                    },
                )],
            );
            app.comic_fonts.seed_for_test(
                Some(Arc::new(comic_core::FontSet::new())),
                Default::default(),
            );
            app.comic_font_registry_loaded = true;
            app.prepare_final_demand_topologies();
            let unit = app
                .d3_final_prepare_unit_at(0, FinalUnitPosition::Next)
                .unwrap();
            assert!(app.spawn_prepared_edit_job(&ctx, 1, unit, &mut false));
            let key = wait_ready(&mut app, 1);
            let allocations = app
                .prepared_edit_remaining_allocations(
                    &ctx,
                    key,
                    &app.final_effect_pending[&key].state,
                )
                .unwrap();
            assert_eq!(allocations.len(), if shared { 1 } else { 2 });
            assert_eq!(
                next(&app, &ctx),
                Producing,
                "CpuReady is finite when every remaining allocation fits"
            );
            app.adopt_prepared_edit(&ctx, key, FinalPrepareAdoption::BeforeLookup, false);
            assert!(matches!(
                app.final_effect_pending[&key].state,
                FinalEffectState::Registering(_)
            ));
            begin_registration_frame(&mut app, 9);
            app.adopt_prepared_edit(
                &ctx,
                key,
                FinalPrepareAdoption::RootFinalized(None, FinalUnitPosition::Next),
                false,
            );
            if !shared {
                let remaining = app
                    .prepared_edit_remaining_allocations(
                        &ctx,
                        key,
                        &app.final_effect_pending[&key].state,
                    )
                    .unwrap();
                assert_eq!(
                    remaining.len(),
                    1,
                    "first private handle is resident and must not be charged again"
                );
                assert_eq!(next(&app, &ctx), Producing);
                begin_registration_frame(&mut app, 10);
                app.adopt_prepared_edit(
                    &ctx,
                    key,
                    FinalPrepareAdoption::RootFinalized(None, FinalUnitPosition::Next),
                    false,
                );
            }
            assert!(!app.final_effect_pending.contains_key(&key));
            assert!(app.resident_final_prepare_material(&ctx, 1).is_some());
            assert_eq!(
                next(&app, &ctx),
                Unavailable,
                "completed registration is no longer a producer"
            );
        }
    }
}
