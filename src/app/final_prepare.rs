//! Bounded D3 final preparation. Legacy effect requests keep their existing policy.
use super::final_prepare_cpu::{FinalPrepareReservation, FinalPrepareUnit, FinalPrepareWake};
use super::*;
use crate::ui_fullscreen::{FinalUnitPosition, NextFinalUnitDemand};

pub(super) fn next_final_prepare_request_id() -> u64 {
    static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

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
    pub(super) fn unit(&self) -> Option<&FinalPrepareUnit> {
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
    Edited {
        input: Arc<super::prepared_edit::PreparedEditInput>,
        rx: mpsc::Receiver<super::prepared_edit::PreparedEditPixels>,
        _reservation: Arc<FinalPrepareReservation>,
    },
}

pub(super) enum FinalEffectState {
    Computing(FinalEffectReceiver),
    CpuReady(FinalEffectReady),
    Registering(super::prepared_edit::PreparedEditRegistration),
}

pub(super) enum FinalEffectReady {
    Legacy(FinalCompositeResult),
    Prepared(PreparedFinalPixels),
    Edited(super::prepared_edit::PreparedEditPixels),
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

/// Mutually exclusive, complete resident inputs. Stage display fallback is not
/// evidence that all edits have completed, and must never become a final input.
enum ResidentFinalPrepareInput {
    AfterAi {
        key: EditResultKey,
        pixels: Arc<egui::ColorImage>,
        source_size: [usize; 2],
        used_upscale: bool,
    },
    ResidentEdited {
        key: EditResultKey,
        pixels: Arc<egui::ColorImage>,
    },
    Unedited {
        key: EditResultKey,
        pixels: Arc<egui::ColorImage>,
    },
}

impl FinalEffectState {
    pub(super) fn edit_input(&self) -> Option<&super::prepared_edit::PreparedEditInput> {
        match self {
            Self::Computing(FinalEffectReceiver::Edited { input, .. }) => Some(input),
            Self::CpuReady(FinalEffectReady::Edited(ready)) => Some(&ready.input),
            Self::Registering(registration) => Some(&registration.ready.input),
            _ => None,
        }
    }
    /// Receipt changes only the existing owner. It never selects a display source.
    pub(super) fn receive(&mut self) -> Result<(), mpsc::TryRecvError> {
        let ready = match self {
            Self::CpuReady(_) | Self::Registering(_) => return Ok(()),
            Self::Computing(FinalEffectReceiver::Legacy(rx)) => {
                FinalEffectReady::Legacy(rx.try_recv()?)
            }
            Self::Computing(FinalEffectReceiver::Prepared { rx, .. }) => {
                FinalEffectReady::Prepared(rx.try_recv()?)
            }
            Self::Computing(FinalEffectReceiver::Edited { rx, .. }) => {
                FinalEffectReady::Edited(rx.try_recv()?)
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
    pub(crate) fn resident_final_prepare_material(
        &self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<crate::gpu_lanczos::FullscreenPaintResource> {
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
                .then(|| {
                    self.fs_resource_for_accepted_texture(
                        idx,
                        comic.texture.clone(),
                        crate::source_coverage::SourceCoverage::FullSource,
                        comic.display_basis,
                    )
                });
            }
            return Some(self.fs_resource_for_accepted_texture(
                idx,
                final_entry.texture.clone(),
                crate::source_coverage::SourceCoverage::FullSource,
                final_entry.display_basis,
            ));
        }
        if self.comic_pages.contains(&idx) || self.comic_demo_enabled {
            return None;
        }
        let (edit_key, pixels) = match self.resident_final_prepare_input(idx)? {
            ResidentFinalPrepareInput::ResidentEdited { key, pixels }
            | ResidentFinalPrepareInput::Unedited { key, pixels } => (key, pixels),
            ResidentFinalPrepareInput::AfterAi { .. } => return None,
        };
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
        if let Some(entry) = self.edit_result_cache.get(&edit_key)
            && let Some(texture) = entry.texture.as_ref()
            && self.display_texture_matches_page(texture, idx)
        {
            return Some(self.fs_resource_for_accepted_texture(
                idx,
                texture.clone(),
                crate::source_coverage::SourceCoverage::FullSource,
                entry.display_basis,
            ));
        }
        // The shared-allocation eligibility is established by this producer;
        // its semantic owner is the accepted edit input, never an ID lookup.
        self.shared_resident_texture_for_edit(ctx, &edit_key, &pixels)
    }

    pub(crate) fn resident_final_prepare_texture(
        &self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<egui::TextureHandle> {
        self.resident_final_prepare_material(ctx, idx)
            .map(|resource| resource.source_texture().clone())
    }

    pub(super) fn final_prepare_wake(&self, ctx: &egui::Context) -> FinalPrepareWake {
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
        self.validate_final_prepare_pending(key, pending).is_ok()
    }

    pub(super) fn validate_final_prepare_identity(
        &self,
        key: EditResultKey,
    ) -> Result<(), PreparedEditReject> {
        if key.items_generation != self.items_generation {
            return Err(PreparedEditReject::ItemsGeneration);
        }
        if !self.edit_key_describes_current_page(&key, key.idx) {
            return Err(PreparedEditReject::ItemIdentity);
        }
        Ok(())
    }

    pub(super) fn validate_final_prepare_pending(
        &self,
        key: FinalCompositeKey,
        pending: &FinalEffectPending,
    ) -> Result<(), PreparedEditReject> {
        if pending.cancel.load(Ordering::Relaxed) {
            return Err(PreparedEditReject::Cancelled);
        }
        if pending.items_generation != self.items_generation {
            return Err(PreparedEditReject::ItemsGeneration);
        }
        if let Some(unit) = pending.origin.unit() {
            // Binding is payload identity, independent of F12's temporary host.
            if unit.context != self.projected_viewer_context_id() {
                return Err(PreparedEditReject::Context);
            }
            if unit.items_generation != self.items_generation {
                return Err(PreparedEditReject::ItemsGeneration);
            }
            if !unit.pages.iter().any(|page| page.idx == key.edit_key.idx) {
                return Err(PreparedEditReject::ItemIdentity);
            }
        }
        if let Some(input) = pending.state.edit_input() {
            self.validate_prepared_edit_input(input)?;
            if key != input.key {
                return Err(PreparedEditReject::CompositeKey);
            }
        } else {
            self.validate_final_prepare_identity(key.edit_key)?;
        }
        if key
            != self.final_composite_key_for_pixels(
                self.current_edit_result_key(key.edit_key.idx),
                pending.source_size,
                self.effective_params(key.edit_key.idx),
            )
        {
            return Err(PreparedEditReject::CompositeKey);
        }
        Ok(())
    }

    /// No materialization, I/O, texture allocation, mask loading or AI start.
    fn resident_final_prepare_input(&self, idx: usize) -> Option<ResidentFinalPrepareInput> {
        if self.fs_entry_is_animated(idx)
            || self.comic_pages.contains(&idx)
            || self.commit_pending_matches_current_erase_key(idx)
            || self.current_edit_stage_producer_pending(idx)
        {
            return None;
        }
        let key = self.current_edit_result_key(idx);
        let params = self.effective_params(idx);
        // An adopted exact AI result already contains its complete adjusted
        // input. Still wait for any current edit producer, including CpuReady:
        // its adoption owns invalidation even if it does not advance the key.
        if let Some((_, ai)) = self.final_ai_cache.iter().find(|(ai_key, ai)| {
            ai_key.edit_key == key
                && self.final_ai_key_for_pixels(key, ai.source_size, params) == Some(**ai_key)
        }) {
            return Some(ResidentFinalPrepareInput::AfterAi {
                key,
                pixels: ai.pixels.clone(),
                source_size: ai.source_size,
                used_upscale: ai.used_upscale,
            });
        }
        // Presence-only metadata cannot distinguish inactive layers from a
        // missing upper stage. Hydration belongs to the existing local owner.
        if self.local_adjust_pages.contains(&idx)
            && !self.local_adjust_page_layers.contains_key(&idx)
        {
            return None;
        }
        if let Some(edit) = self.edit_result_cache.get(&key) {
            return Some(ResidentFinalPrepareInput::ResidentEdited {
                key,
                pixels: edit.pixels.clone(),
            });
        }
        // Each stage is cumulative. Require only the highest active stage,
        // never its evicted lower inputs, and never fall through a missing
        // upper stage to a display-only lower fallback.
        let edited = if self.conceal_pages.contains(&idx)
            || (self.conceal_mode
                && self.conceal_preview_active
                && self.fullscreen_idx == Some(idx))
        {
            Some(
                self.conceal_cache
                    .get(&idx)
                    .filter(|entry| entry.generation == self.conceal_generation)?
                    .pixels
                    .clone(),
            )
        } else if self.has_active_local_adjust_layers(idx) {
            Some(self.current_local_adjust_pixels(idx)?)
        } else if self.mask_pages.contains(&idx) {
            Some(self.current_erase_result_pixels(idx)?)
        } else {
            None
        };
        if let Some(pixels) = edited {
            Some(ResidentFinalPrepareInput::ResidentEdited { key, pixels })
        } else {
            self.current_raw_source_pixels(idx)
                .map(|pixels| ResidentFinalPrepareInput::Unedited { key, pixels })
        }
    }

    pub(super) fn final_prepare_plan_from_resident(
        &self,
        idx: usize,
    ) -> Option<(
        FinalCompositeKey,
        Arc<egui::ColorImage>,
        [usize; 2],
        FinalCompositePlan,
    )> {
        let params = self.effective_params(idx);
        let lut = self.resolved_creative_lut(params);
        let (edit_key, pixels) = match self.resident_final_prepare_input(idx)? {
            ResidentFinalPrepareInput::AfterAi {
                key,
                pixels,
                source_size,
                used_upscale,
            } => {
                return Some((
                    self.final_composite_key_for_pixels(key, source_size, params),
                    pixels,
                    source_size,
                    build_final_composite_plan_after_ai(params, lut, used_upscale),
                ));
            }
            ResidentFinalPrepareInput::ResidentEdited { key, pixels }
            | ResidentFinalPrepareInput::Unedited { key, pixels } => (key, pixels),
        };
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

    /// Next starts only at ordinary fullscreen work. ROOT finalized output may
    /// additionally start one resident-only Previous job after every live Next
    /// is satisfied. Mount and pre-lookup adoption never start producers.
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
            self.cancel_final_effect_key(key, PreparedEditCancel::DemandObsolete);
        }
        let Some(unit) = demand else {
            return;
        };
        if !self.page_turn_current_unit_presented() {
            return;
        }
        let mut reclaim_attempted = false;
        for page in &unit.pages {
            let idx = page.idx;
            if self.comic_pages.contains(&idx) {
                self.request_comic_fonts_for_page(ctx, idx);
            }
            if self.prepared_edit_page(idx) {
                if self.spawn_prepared_edit_job(ctx, idx, unit.clone(), &mut reclaim_attempted) {
                    return;
                }
                continue;
            }
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
            if self.spawn_prepared_final_job(
                ctx,
                key,
                source,
                source_size,
                plan,
                unit.clone(),
                &mut reclaim_attempted,
            ) {
                return;
            }
        }
    }

    fn d3_final_prepare_unit(&self, current_idx: usize) -> Option<FinalPrepareUnit> {
        self.d3_final_prepare_unit_at(current_idx, FinalUnitPosition::Next)
    }

    pub(super) fn d3_final_prepare_unit_at(
        &self,
        current_idx: usize,
        position: FinalUnitPosition,
    ) -> Option<FinalPrepareUnit> {
        if !self.settings.page_turn_animation_enabled
            || !self.reading_flow.is_paged()
            || self.spread_mode.is_split()
            || self.post_filter_bypassed
            || self.remote_session_blocks_local_control()
        {
            return None;
        }
        let pages = match position {
            FinalUnitPosition::Current => self.current_final_prepare_occurrences(current_idx),
            FinalUnitPosition::Next => self.next_final_prepare_occurrences(current_idx),
            FinalUnitPosition::Previous => self.previous_final_prepare_occurrences(current_idx),
        };
        pages.map(|pages| FinalPrepareUnit {
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
        if self
            .d3_final_prepare_unit_at(idx, FinalUnitPosition::Previous)
            .as_ref()
            == Some(unit)
        {
            return true;
        }
        self.current_final_prepare_occurrences(idx)
            .is_some_and(|pages| {
                unit.context == self.projected_viewer_context_id()
                    && unit.items_generation == self.items_generation
                    && unit.pages == pages
            })
    }

    /// Classification uses adopted topology and exact identity, not job origin
    /// direction. Unknown is deliberately protected during pressure recovery.
    pub(super) fn final_prepare_unit_position(
        &self,
        unit: &FinalPrepareUnit,
    ) -> Option<FinalUnitPosition> {
        if unit.context != self.projected_viewer_context_id()
            || unit.items_generation != self.items_generation
        {
            return None;
        }
        let idx = self.fullscreen_idx?;
        for position in [
            FinalUnitPosition::Current,
            FinalUnitPosition::Next,
            FinalUnitPosition::Previous,
        ] {
            let demand = self.final_unit_demand_from_snapshot(idx, position)?;
            if let NextFinalUnitDemand::Unit(pages) = demand
                && pages == unit.pages
            {
                return Some(position);
            }
        }
        None
    }

    pub(super) fn final_prepare_key_position(
        &self,
        key: FinalCompositeKey,
    ) -> Option<FinalUnitPosition> {
        let idx = self.fullscreen_idx?;
        for position in [
            FinalUnitPosition::Current,
            FinalUnitPosition::Next,
            FinalUnitPosition::Previous,
        ] {
            let demand = self.final_unit_demand_from_snapshot(idx, position)?;
            if let NextFinalUnitDemand::Unit(pages) = demand
                && pages.iter().any(|page| page.idx == key.edit_key.idx)
            {
                return Some(position);
            }
        }
        None
    }

    /// This call performs no adoption/registration or producer work. Only a
    /// reservation-pressure snapshot (at most two units) chooses these owners.
    pub(super) fn reclaim_previous_final_results(
        &mut self,
        units: &[FinalPrepareUnit],
        requester: &FinalPrepareUnit,
    ) {
        let mut owners = Vec::with_capacity(units.len());
        for unit in units {
            if !owners.contains(&unit.context) {
                owners.push(unit.context);
            }
        }
        for owner in owners {
            if self.with_viewer_context_ref(owner, |_| ()).is_none() {
                continue;
            }
            #[cfg(test)]
            tests::record_pressure_owner_visit(owner);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "fs",
                    "final_prepare_cpu_pressure",
                    None,
                    self.input_seq,
                    &[
                        ("context", owner.serial().into()),
                        ("requester_context", requester.context.serial().into()),
                        ("requester_unit", format!("{:?}", requester.pages).into()),
                        (
                            "reserved_units",
                            format!(
                                "{:?}",
                                units
                                    .iter()
                                    .filter(|unit| unit.context == owner)
                                    .collect::<Vec<_>>()
                            )
                            .into(),
                        ),
                        ("demand_position", "unknown".into()),
                        ("requested_position", "unknown".into()),
                        ("reason", "owner_visit".into()),
                    ],
                );
            }
            let _ = self.with_viewer_context_state(owner, |app| {
                let keys: Vec<_> = app.final_effect_pending.keys().copied().collect();
                for key in keys {
                    let pending = &app.final_effect_pending[&key];
                    let FinalEffectOrigin::Speculative(unit) = &pending.origin else { continue };
                    let request = (pending.gpu_diagnostics.request_id(), pending.gpu_diagnostics.requested_position());
                    if !units.contains(unit) { continue; }
                    let protected = app.final_prepare_unit_position(unit) != Some(FinalUnitPosition::Previous)
                        || !app.final_prepare_pending_is_current(key, pending)
                        // Key sharing (e.g. a cover occurrence) protects material
                        // even when the complete unit has a different identity.
                        || app.final_prepare_key_position(key) != Some(FinalUnitPosition::Previous);
                    if protected {
                        app.record_final_cpu_pressure(key, "protected", "current_next_or_unknown", request);
                        continue;
                    }
                    let location = match &pending.state {
                        FinalEffectState::CpuReady(FinalEffectReady::Prepared(_)) => "cpu_ready",
                        FinalEffectState::Computing(FinalEffectReceiver::Prepared { .. }) => "channel",
                        FinalEffectState::CpuReady(FinalEffectReady::Edited(_)) => "cpu_ready",
                        FinalEffectState::Computing(FinalEffectReceiver::Edited { .. }) => "channel",
                        FinalEffectState::Registering(_) => "registering",
                        _ => continue,
                    };
                    let received = app.final_effect_pending.get_mut(&key).unwrap().state.receive();
                    if matches!(received, Err(mpsc::TryRecvError::Empty)) {
                        app.record_final_cpu_pressure(key, "retained_computing", location, request);
                        continue;
                    }
                    // Drop channel, temporary result and pending before retry.
                    app.cancel_final_effect_key(key, PreparedEditCancel::Pressure);
                    app.record_final_cpu_pressure(key, "reclaimed", location, request);
                }
            });
        }
    }

    fn record_final_cpu_pressure(
        &self,
        key: FinalCompositeKey,
        reason: &'static str,
        location: &'static str,
        request: (u64, &'static str),
    ) {
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "final_prepare_cpu_pressure",
                None,
                self.input_seq,
                &[
                    (
                        "context",
                        self.projected_viewer_context_id().serial().into(),
                    ),
                    ("idx", key.edit_key.idx.into()),
                    ("request_id", request.0.into()),
                    ("requested_position", request.1.into()),
                    (
                        "demand_position",
                        self.final_prepare_key_position(key)
                            .map_or("unknown", FinalUnitPosition::as_str)
                            .into(),
                    ),
                    ("reason", reason.into()),
                    ("location", location.into()),
                ],
            );
        }
    }

    pub(super) fn cancel_previous_computing_for_next(&mut self) {
        let keys: Vec<_> = self
            .final_effect_pending
            .iter()
            .filter_map(|(key, pending)| {
                (matches!(
                    pending.state,
                    FinalEffectState::Computing(
                        FinalEffectReceiver::Prepared { .. } | FinalEffectReceiver::Edited { .. }
                    )
                ) && matches!(&pending.origin, FinalEffectOrigin::Speculative(unit)
                    if self.final_prepare_unit_position(unit) == Some(FinalUnitPosition::Previous))
                    && self.final_prepare_key_position(*key) == Some(FinalUnitPosition::Previous)
                    && self.final_prepare_pending_is_current(*key, pending))
                .then_some(*key)
            })
            .collect();
        for key in keys {
            // A just-completed channel is a result, not an executing worker.
            if matches!(
                self.final_effect_pending
                    .get_mut(&key)
                    .unwrap()
                    .state
                    .receive(),
                Err(mpsc::TryRecvError::Empty)
            ) {
                if let Some(pending) = self.final_effect_pending.get(&key) {
                    let request = (
                        pending.gpu_diagnostics.request_id(),
                        pending.gpu_diagnostics.requested_position(),
                    );
                    // Publish the cancellation marker before the worker can
                    // observe it and publish actual completion.
                    self.record_final_cpu_pressure(key, "cancelled_for_next", "computing", request);
                }
                self.cancel_final_effect_key(key, PreparedEditCancel::NextPriority);
            }
        }
    }

    pub(super) fn prefetch_previous_final_unit(&mut self, ctx: &egui::Context) -> bool {
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        if !self.page_turn_current_unit_presented()
            || !self
                .final_prepare_work_admission(idx)
                .admits_non_target_full_resolution_work()
        {
            return false;
        }
        let Some(unit) = self.d3_final_prepare_unit_at(idx, FinalUnitPosition::Previous) else {
            return false;
        };
        let mut reclaim_attempted = false;
        for page in &unit.pages {
            if self.resident_final_prepare_texture(ctx, page.idx).is_some() {
                continue;
            }
            let Some((key, pixels, size, plan)) = self.final_prepare_plan_from_resident(page.idx)
            else {
                continue;
            };
            if self.final_effect_pending.contains_key(&key) {
                continue;
            }
            if self.spawn_prepared_final_job(
                ctx,
                key,
                pixels,
                size,
                plan,
                unit.clone(),
                &mut reclaim_attempted,
            ) {
                return true;
            }
        }
        false
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
            if let Some(resource) =
                self.shared_resident_texture_for_edit(ctx, &key.edit_key, &ready.pixels)
            {
                return Some(resource.source_texture().clone());
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
        reclaim_attempted: &mut bool,
    ) -> bool {
        let wake = self.final_prepare_wake(ctx);
        let Some(bytes) = prepared_output_bytes(source.size, plan.post_filter) else {
            return false;
        };
        let requested_position = self.final_prepare_unit_position(&unit);
        #[cfg(test)]
        tests::record_pressure_admission(key.edit_key.idx);
        let mut admission = self
            .final_prepare_cpu
            .try_start(unit.clone(), bytes, wake.clone());
        if !*reclaim_attempted
            && requested_position == Some(FinalUnitPosition::Next)
            && let Err(final_prepare_cpu::FinalPrepareStartDenied::Reservation {
                units,
                bytes: reserved,
                reason,
            }) = &admission
        {
            // One recovery belongs to the whole unit producer invocation,
            // including both Pair pages. Later pages still try normal admission.
            *reclaim_attempted = true;
            let reserved = *reserved;
            let reason = *reason;
            self.reclaim_previous_final_results(units, &unit);
            self.record_final_cpu_pressure(key, "retry_after_reclaim", reason, (0, "next"));
            // Owner visits restore the original mount. Do not reuse a changed
            // demand or key; no recursive producer or repeated retry is allowed.
            if self
                .fullscreen_idx
                .and_then(|idx| self.d3_final_prepare_unit(idx))
                .as_ref()
                != Some(&unit)
                || self
                    .final_prepare_plan_from_resident(key.edit_key.idx)
                    .is_none_or(|(current, _, _, _)| current != key)
            {
                return false;
            }
            #[cfg(test)]
            tests::record_pressure_admission(key.edit_key.idx);
            admission = self.final_prepare_cpu.try_start(unit.clone(), bytes, wake);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "fs",
                    "final_prepare_cpu_pressure",
                    None,
                    self.input_seq,
                    &[
                        ("context", unit.context.serial().into()),
                        ("idx", key.edit_key.idx.into()),
                        ("demand_position", "next".into()),
                        ("requested_position", "next".into()),
                        ("reason", "retry_result".into()),
                        ("reserved_before", reserved.into()),
                        (
                            "reserved_after",
                            self.final_prepare_cpu.reserved_bytes().into(),
                        ),
                        ("admitted", admission.is_ok().into()),
                    ],
                );
            }
        }
        let (permit, reservation) = match admission {
            Ok(leases) => leases,
            Err(denied) => {
                self.record_final_cpu_admission_denied(&unit, bytes, &denied);
                return false;
            }
        };
        let requested_position = requested_position.map_or("unknown", FinalUnitPosition::as_str);
        let request_id = next_final_prepare_request_id();
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
                struct Finished {
                    id: u64,
                    context: u64,
                    idx: usize,
                    requested: &'static str,
                    started: std::time::Instant,
                    cancel: Arc<AtomicBool>,
                }
                impl Drop for Finished {
                    fn drop(&mut self) {
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "fs",
                                "final_prepare_worker_finished",
                                None,
                                0,
                                &[
                                    ("request_id", self.id.into()),
                                    ("context", self.context.into()),
                                    ("idx", self.idx.into()),
                                    ("demand_position", "unknown".into()),
                                    ("requested_position", self.requested.into()),
                                    ("cancelled", self.cancel.load(Ordering::Relaxed).into()),
                                    (
                                        "worker_ms",
                                        (self.started.elapsed().as_secs_f64() * 1000.0).into(),
                                    ),
                                ],
                            );
                        }
                    }
                }
                // Reverse drop order: return the actual permit before recording
                // completion, including cancellation, executor panic and send failure.
                let _finished = Finished {
                    id: request_id,
                    context,
                    idx: key.edit_key.idx,
                    requested: requested_position,
                    started: std::time::Instant::now(),
                    cancel: worker_cancel.clone(),
                };
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
                                    ("request_id", request_id.into()),
                                    ("params_hash", key.params_hash.into()),
                                    ("requested_position", requested_position.into()),
                                    ("demand_position", "unknown".into()),
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
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "fs",
                        "final_prepare_cpu_started",
                        None,
                        self.input_seq,
                        &[
                            ("request_id", request_id.into()),
                            ("context", unit.context.serial().into()),
                            ("idx", key.edit_key.idx.into()),
                            ("items_generation", unit.items_generation.into()),
                            ("params_hash", key.params_hash.into()),
                            ("source_generation", key.edit_key.source_gen.into()),
                            ("demand_position", requested_position.into()),
                            ("requested_position", requested_position.into()),
                            ("unit_pages", format!("{:?}", unit.pages).into()),
                            ("reservation_bytes", bytes.into()),
                        ],
                    );
                }
                self.final_effect_pending.insert(
                    key,
                    FinalEffectPending {
                        pdf_metadata: self.current_pdf_page_metadata_snapshot(key.edit_key.idx),
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
                        gpu_diagnostics:
                            final_prepare_budget::FinalPrepareGpuDiagnostics::requested(
                                requested_position,
                                request_id,
                            ),
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
        self.resolve_prepared_edit_for_display(ctx, idx);
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
        } else {
            // Receiver removal revokes publication; the worker still owns permit
            // and reservation until it really finishes. Never join on the UI.
            self.cancel_final_effect_key(key, PreparedEditCancel::DisplayComputing);
        }
        PreparedFinalDisplay::Continue
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod resident_edited_tests;
