//! Bounded D3 final preparation. Legacy effect requests keep their existing policy.
use super::final_prepare_cpu::{FinalPrepareReservation, FinalPrepareUnit, FinalPrepareWake};
use super::*;

pub(super) enum FinalEffectOrigin {
    LegacyDisplay,
    LegacyPrefetch,
    Speculative(FinalPrepareUnit),
    DisplayPrepared(FinalPrepareUnit),
}

impl FinalEffectOrigin {
    pub(super) fn is_prefetch(&self) -> bool {
        matches!(self, Self::LegacyPrefetch | Self::Speculative(_))
    }
    pub(super) fn is_legacy(&self) -> bool {
        matches!(self, Self::LegacyDisplay | Self::LegacyPrefetch)
    }
    pub(super) fn is_speculative(&self) -> bool {
        matches!(self, Self::Speculative(_))
    }
    fn unit(&self) -> Option<&FinalPrepareUnit> {
        match self {
            Self::Speculative(unit) | Self::DisplayPrepared(unit) => Some(unit),
            _ => None,
        }
    }
    pub(super) fn promote(&mut self) {
        if let Self::Speculative(unit) = self {
            *self = Self::DisplayPrepared(unit.clone());
        } else if matches!(self, Self::LegacyPrefetch) {
            *self = Self::LegacyDisplay;
        }
    }
}

pub(super) enum FinalEffectReceiver {
    Legacy(mpsc::Receiver<FinalCompositeResult>),
    Prepared {
        rx: mpsc::Receiver<PreparedFinalPixels>,
        _reservation: Arc<FinalPrepareReservation>,
    },
}

pub(super) enum FinalEffectState {
    Computing(FinalEffectReceiver),
    CpuReady(FinalEffectReady),
}

pub(super) enum FinalEffectReady {
    Legacy(FinalCompositeResult),
    Prepared(PreparedFinalPixels),
}

pub(super) struct PreparedFinalPixels {
    pub(super) pixels: Arc<egui::ColorImage>,
    pub(super) upload: Arc<egui::ColorImage>,
    pub(super) elapsed_ms: f64,
    pub(super) timing: crate::final_composite::FinalCompositeTiming,
    pub(super) clamp_ms: f64,
    pub(super) reservation: Arc<FinalPrepareReservation>,
}

pub(super) enum PreparedFinalDisplay {
    Continue,
    Handled(Option<egui::TextureHandle>),
}

impl FinalEffectState {
    /// Receipt changes only the existing owner. It never selects a display source.
    pub(super) fn receive(&mut self) -> Result<(), mpsc::TryRecvError> {
        let ready = match self {
            Self::CpuReady(_) => return Ok(()),
            Self::Computing(FinalEffectReceiver::Legacy(rx)) => {
                FinalEffectReady::Legacy(rx.try_recv()?)
            }
            Self::Computing(FinalEffectReceiver::Prepared { rx, .. }) => {
                FinalEffectReady::Prepared(rx.try_recv()?)
            }
        };
        *self = Self::CpuReady(ready);
        Ok(())
    }
}

/// Same Triangle path as synchronous display; within the limit the Arc is shared.
pub(super) fn final_upload_pixels(pixels: &Arc<egui::ColorImage>) -> Arc<egui::ColorImage> {
    match clamp_for_gpu(pixels) {
        std::borrow::Cow::Borrowed(_) => Arc::clone(pixels),
        std::borrow::Cow::Owned(image) => Arc::new(image),
    }
}

fn prepared_output_bytes(size: [usize; 2], filter: crate::adjustment::PostFilter) -> Option<u64> {
    let output = crate::post_filter::output_dimensions(size, filter);
    let upload = gpu_clamped_dimensions(output);
    let bytes = |[w, h]: [usize; 2]| (w as u64).checked_mul(h as u64)?.checked_mul(4);
    bytes(output)?.checked_add(if output == upload { 0 } else { bytes(upload)? })
}

impl App {
    /// Exact resident inputs only: no ensure, DB hydration, materialization or upload.
    /// Native prewarm must target the handle ordinary preparation can really adopt.
    pub(crate) fn resident_final_prepare_texture(
        &self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<egui::TextureHandle> {
        if self.fs_entry_is_animated(idx) {
            return None;
        }
        if let Some((_, final_entry)) = self.exact_complete_final_composite_entry(idx) {
            if self.comic_pages.contains(&idx)
                || self.comic_demo_enabled
                || self.comic_cache.contains_key(&idx)
            {
                let comic = self.comic_cache.get(&idx)?;
                let preview_scale = if self.text_mode {
                    self.settings.text_preview_scale.clamp(1, 8)
                } else {
                    1
                };
                return (Arc::ptr_eq(&comic.base, &final_entry.pixels)
                    && comic.comic_gen == self.comic_generation
                    && comic.dims == final_entry.pixels.size
                    && comic.preview_scale == preview_scale
                    && self.display_texture_matches_page(&comic.texture, idx))
                .then(|| comic.texture.clone());
            }
            return Some(final_entry.texture.clone());
        }
        if self.comic_pages.contains(&idx) || self.comic_demo_enabled {
            return None;
        }
        let (edit_key, pixels) = self.resident_final_prepare_input(idx)?;
        let params = self.effective_params(idx);
        if !params.is_color_identity()
            || params.effective_smart_sharpen(false) != 0
            || params.colorize.is_enabled()
            || self.creative_lut_requires_final_effect(params)
            || !matches!(
                params.post_filter,
                crate::adjustment::PostFilter::None | crate::adjustment::PostFilter::Nearest
            )
            || self
                .final_ai_key_for_pixels(edit_key, pixels.size, params)
                .is_some_and(|key| !self.final_ai_failed.contains(&key))
        {
            return None;
        }
        self.edit_result_cache
            .get(&edit_key)
            .and_then(|entry| entry.texture.as_ref())
            .filter(|texture| self.display_texture_matches_page(texture, idx))
            .cloned()
            .or_else(|| self.shared_source_texture_for_edit(ctx, &edit_key, &pixels))
    }

    fn final_prepare_wake(&self, ctx: &egui::Context) -> FinalPrepareWake {
        FinalPrepareWake {
            ctx: ctx.clone(),
            viewport: self.page_turn_destination().viewport,
        }
    }

    pub(super) fn final_prepare_pending_is_current(
        &self,
        key: FinalCompositeKey,
        pending: &FinalEffectPending,
    ) -> bool {
        !pending.cancel.load(Ordering::Relaxed)
            && pending.items_generation == self.items_generation
            && self.edit_key_describes_current_page(&key.edit_key, key.edit_key.idx)
            && key
                == self.final_composite_key_for_pixels(
                    self.current_edit_result_key(key.edit_key.idx),
                    pending.source_size,
                    self.effective_params(key.edit_key.idx),
                )
            && pending.origin.unit().is_none_or(|unit| {
                // Binding is the payload identity, not its temporary host/viewport.
                // F12 transfers this owner; fork creates an empty pending map.
                unit.context == self.projected_viewer_context_id()
                    && unit.items_generation == self.items_generation
                    && unit.pages.iter().any(|page| page.idx == key.edit_key.idx)
            })
    }

    /// No materialization, I/O, texture allocation, mask loading or AI start.
    fn resident_final_prepare_input(
        &self,
        idx: usize,
    ) -> Option<(EditResultKey, Arc<egui::ColorImage>)> {
        if self.fs_entry_is_animated(idx) || self.comic_pages.contains(&idx) {
            return None;
        }
        let key = self.current_edit_result_key(idx);
        if let Some(edit) = self.edit_result_cache.get(&key) {
            return Some((key, edit.pixels.clone()));
        }
        if self.mask_pages.contains(&idx)
            || self.local_adjust_pages.contains(&idx)
            || self.local_adjust_page_layers.contains_key(&idx)
            || self.conceal_pages.contains(&idx)
        {
            return None;
        }
        self.current_raw_source_pixels(idx)
            .map(|pixels| (key, pixels))
    }

    fn final_prepare_plan_from_resident(
        &self,
        idx: usize,
    ) -> Option<(
        FinalCompositeKey,
        Arc<egui::ColorImage>,
        [usize; 2],
        FinalCompositePlan,
    )> {
        if self.fs_entry_is_animated(idx) || self.comic_pages.contains(&idx) {
            return None;
        }
        let edit_key = self.current_edit_result_key(idx);
        let params = self.effective_params(idx);
        let lut = self.resolved_creative_lut(params);
        // A completed exact AI result already owns its adjusted input. Neither
        // hydrate raw/edit material nor apply tone again on the UI thread.
        if let Some((_, ai)) = self.final_ai_cache.iter().find(|(key, ai)| {
            key.edit_key == edit_key
                && self.final_ai_key_for_pixels(edit_key, ai.source_size, params) == Some(**key)
        }) {
            return Some((
                self.final_composite_key_for_pixels(edit_key, ai.source_size, params),
                ai.pixels.clone(),
                ai.source_size,
                build_final_composite_plan_after_ai(params, lut, ai.used_upscale),
            ));
        }
        let (edit_key, pixels) = self.resident_final_prepare_input(idx)?;
        if let Some(ai_key) = self.final_ai_key_for_pixels(edit_key, pixels.size, params) {
            if !self.final_ai_failed.contains(&ai_key) {
                return None;
            }
        }
        if params.is_color_identity()
            && params.effective_smart_sharpen(false) == 0
            && !params.colorize.is_enabled()
            && lut.is_none()
            && params.post_filter == crate::adjustment::PostFilter::None
        {
            return None;
        }
        Some((
            self.final_composite_key_for_pixels(edit_key, pixels.size, params),
            pixels.clone(),
            pixels.size,
            build_final_composite_plan_without_ai(params, lut),
        ))
    }

    /// Only the ordinary fullscreen work boundary starts producers. A mount or
    /// pre-lookup adoption service cannot call this method.
    pub(crate) fn prefetch_next_final_unit(&mut self, ctx: &egui::Context, current_idx: usize) {
        self.prepare_final_demand_topologies();
        let demand = self.d3_final_prepare_unit(current_idx);
        let obsolete: Vec<_> = self
            .final_effect_pending
            .iter()
            .filter_map(|(key, pending)| match &pending.origin {
                FinalEffectOrigin::Speculative(unit)
                    if !self.speculative_final_unit_is_needed(unit) =>
                {
                    Some(*key)
                }
                _ => None,
            })
            .collect();
        for key in obsolete {
            if let Some(pending) = self.final_effect_pending.remove(&key) {
                pending.cancel.store(true, Ordering::Relaxed);
            }
        }
        let Some(unit) = demand else {
            return;
        };
        if !self.page_turn_current_unit_presented() {
            return;
        }
        for page in &unit.pages {
            let idx = page.idx;
            if self.exact_complete_final_composite_entry(idx).is_some() {
                continue;
            }
            let Some((key, source, source_size, plan)) = self.final_prepare_plan_from_resident(idx)
            else {
                continue;
            };
            if self.final_effect_pending.contains_key(&key) {
                continue;
            }
            if self.spawn_prepared_final_job(ctx, key, source, source_size, plan, unit.clone()) {
                return;
            }
        }
    }

    fn d3_final_prepare_unit(&self, current_idx: usize) -> Option<FinalPrepareUnit> {
        if !self.settings.page_turn_animation_enabled
            || !self.reading_flow.is_paged()
            || self.spread_mode.is_split()
            || self.post_filter_bypassed
            || self.remote_session_blocks_local_control()
        {
            return None;
        }
        self.next_final_prepare_occurrences(current_idx)
            .map(|pages| FinalPrepareUnit {
                context: self.projected_viewer_context_id(),
                items_generation: self.items_generation,
                pages,
            })
    }

    pub(super) fn speculative_final_unit_is_needed(&self, unit: &FinalPrepareUnit) -> bool {
        if !self.settings.page_turn_animation_enabled
            || !self.reading_flow.is_paged()
            || self.spread_mode.is_split()
        {
            return false;
        }
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        if self.d3_final_prepare_unit(idx).as_ref() == Some(unit) {
            return true;
        }
        self.current_final_prepare_occurrences(idx)
            .is_some_and(|pages| {
                unit.context == self.projected_viewer_context_id()
                    && unit.items_generation == self.items_generation
                    && unit.pages == pages
            })
    }

    pub(super) fn shared_prepared_final_texture(
        &self,
        ctx: &egui::Context,
        key: FinalCompositeKey,
        ready: &PreparedFinalPixels,
        nearest: bool,
    ) -> Option<egui::TextureHandle> {
        if !Arc::ptr_eq(&ready.pixels, &ready.upload) {
            return None;
        }
        let options = if nearest {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        if !nearest {
            if let Some(texture) =
                self.shared_source_texture_for_edit(ctx, &key.edit_key, &ready.pixels)
            {
                return Some(texture);
            }
        }
        let edit = self.edit_result_cache.get(&key.edit_key)?;
        if !Arc::ptr_eq(&edit.pixels, &ready.pixels) {
            return None;
        }
        let texture = edit.texture.as_ref()?;
        let matches = ctx
            .tex_manager()
            .read()
            .meta(texture.id())
            .is_some_and(|meta| meta.size == ready.pixels.size && meta.options == options);
        (matches && self.display_texture_matches_page(texture, key.edit_key.idx))
            .then(|| texture.clone())
    }

    fn spawn_prepared_final_job(
        &mut self,
        ctx: &egui::Context,
        key: FinalCompositeKey,
        source: Arc<egui::ColorImage>,
        source_size: [usize; 2],
        plan: FinalCompositePlan,
        unit: FinalPrepareUnit,
    ) -> bool {
        let wake = self.final_prepare_wake(ctx);
        let Some(bytes) = prepared_output_bytes(source.size, plan.post_filter) else {
            return false;
        };
        let Some((permit, reservation)) =
            self.final_prepare_cpu.try_start(unit.clone(), bytes, wake)
        else {
            return false;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_reservation = reservation.clone();
        let nearest_sampler = plan.needs_nearest_sampler();
        let context = unit.context.serial();
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        let test_registration = tests::attach_worker(&worker_cancel);
        let spawn = std::thread::Builder::new()
            .name(format!("miv-final-prepare-{}", key.edit_key.idx))
            .spawn(move || {
                let _permit = permit;
                // Make this a local owned after the permit: canceled output
                // reservations are dropped before the final completion wake.
                let worker_reservation = worker_reservation;
                #[cfg(test)]
                let _test_registration = test_registration;
                if let FinalCompositeResult::Ready {
                    pixels,
                    elapsed_ms,
                    timing,
                } = execute_final_composite(source, plan, &worker_cancel)
                {
                    let started = std::time::Instant::now();
                    let upload = final_upload_pixels(&pixels);
                    let clamp_ms = started.elapsed().as_secs_f64() * 1000.0;
                    if !worker_cancel.load(Ordering::Relaxed) {
                        let size = pixels.size;
                        let upload_size = upload.size;
                        let sent = tx.send(PreparedFinalPixels {
                            pixels,
                            upload,
                            elapsed_ms: elapsed_ms + clamp_ms,
                            timing,
                            clamp_ms,
                            reservation: worker_reservation,
                        });
                        if sent.is_ok() && crate::perf::is_enabled() {
                            crate::perf::event(
                                "fs",
                                "final_prepare_cpu_ready",
                                None,
                                0,
                                &[
                                    ("context", context.into()),
                                    ("idx", key.edit_key.idx.into()),
                                    ("items_generation", key.edit_key.items_generation.into()),
                                    ("source_generation", key.edit_key.source_gen.into()),
                                    ("worker_ms", (elapsed_ms + clamp_ms).into()),
                                    ("clamp_ms", clamp_ms.into()),
                                    ("w", size[0].into()),
                                    ("h", size[1].into()),
                                    ("upload_w", upload_size[0].into()),
                                    ("upload_h", upload_size[1].into()),
                                    ("reservation_bytes", bytes.into()),
                                ],
                            );
                        }
                    }
                }
            });
        match spawn {
            Ok(_) => {
                self.final_effect_pending.insert(
                    key,
                    FinalEffectPending {
                        canonical_dims: self.canonical_source_dimensions(key.edit_key.idx),
                        source_size,
                        cancel,
                        state: FinalEffectState::Computing(FinalEffectReceiver::Prepared {
                            rx,
                            _reservation: reservation,
                        }),
                        items_generation: self.items_generation,
                        output_complete: true,
                        nearest_sampler,
                        origin: FinalEffectOrigin::Speculative(unit),
                        gpu_diagnostics: Default::default(),
                    },
                );
                true
            }
            Err(error) => {
                crate::logger::log(format!(
                    "[final-prepare] worker spawn failed idx={} error={error}",
                    key.edit_key.idx
                ));
                false
            }
        }
    }

    /// Display requests own this exact key now. Do not force an edit upload or
    /// wait for a speculative registration slot before consuming prepared pixels.
    pub(super) fn take_prepared_final_for_display(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> PreparedFinalDisplay {
        let Some(key) = self.final_effect_pending.iter().find_map(|(key, pending)| {
            (key.edit_key.idx == idx
                && (!pending.origin.is_legacy()
                    || (matches!(pending.origin, FinalEffectOrigin::LegacyPrefetch)
                        && self.defer_legacy_final_prefetch_registration()))
                && self.final_prepare_pending_is_current(*key, pending))
            .then_some(*key)
        }) else {
            return PreparedFinalDisplay::Continue;
        };
        self.collect_final_prepare_results();
        if !self.final_effect_pending.contains_key(&key) {
            return PreparedFinalDisplay::Continue;
        }
        let ready = self
            .final_effect_pending
            .get(&key)
            .is_some_and(|p| matches!(p.state, FinalEffectState::CpuReady(_)));
        if ready {
            let legacy = matches!(
                self.final_effect_pending[&key].origin,
                FinalEffectOrigin::LegacyPrefetch
            );
            let output_complete = self.final_effect_pending[&key].output_complete;
            self.final_effect_pending
                .get_mut(&key)
                .unwrap()
                .origin
                .promote();
            self.adopt_final_prepare_results_before_lookup(ctx, vec![key]);
            if legacy && !output_complete {
                // As with the old early poll, publication of an intermediate AI
                // result must still let the ordinary quality producer run now.
                return PreparedFinalDisplay::Continue;
            }
            return PreparedFinalDisplay::Handled(if legacy {
                self.final_composite_cache
                    .get(&key)
                    .map(|entry| entry.texture.clone())
            } else {
                self.exact_complete_final_composite_texture(idx)
            });
        }
        if matches!(
            self.final_effect_pending[&key].origin,
            FinalEffectOrigin::LegacyPrefetch
        ) && !self.final_effect_pending[&key].output_complete
        {
            return PreparedFinalDisplay::Continue;
        }
        let params = self.effective_params(idx);
        if !self.post_filter_bypassed
            && (params.colorize.is_enabled() || self.creative_lut_requires_final_effect(params))
        {
            self.final_effect_pending
                .get_mut(&key)
                .unwrap()
                .origin
                .promote();
            return PreparedFinalDisplay::Handled(
                self.final_composite_cache
                    .get(&key)
                    .map(|entry| entry.texture.clone()),
            );
        } else if let Some(pending) = self.final_effect_pending.remove(&key) {
            // Receiver removal revokes publication; the worker still owns permit
            // and reservation until it really finishes. Never join on the UI.
            pending.cancel.store(true, Ordering::Relaxed);
        }
        PreparedFinalDisplay::Continue
    }
}

#[cfg(test)]
pub(crate) mod tests;
