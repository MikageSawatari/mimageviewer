//! App-wide D3 registration admission, shared by bounded final preparation and
//! later comic/Native producers. Ordinary display uploads never wait on it.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegistrationState {
    #[default]
    Available,
    OrdinaryUpload,
    PageTurnActivity,
    SpeculativeSubmitted,
}

#[derive(Default)]
pub(super) struct SpeculativeGpuFrame {
    pub(super) frame: Option<u64>,
    state: RegistrationState,
    /// Final-pass dispatch observations only; no work or resources are owned here.
    live_paged: Vec<crate::ui_fullscreen::PresentationDestination>,
    /// Value-only demands from the owning UI callbacks, valid for this pass only.
    canonical: Vec<crate::ui_fullscreen::canonical_demand::CanonicalUnitDemand>,
    /// Perf-only bounded values, never consulted by admission or repaint policy.
    native_diagnostics: Vec<NativeGpuDiagnostics>,
}

struct NativeGpuDiagnostics {
    destination: crate::ui_fullscreen::PresentationDestination,
    identity: String,
    reasons: Vec<&'static str>,
}

impl NativeGpuDiagnostics {
    fn first(&mut self, identity: &str, reason: &'static str) -> bool {
        if self.identity != identity {
            self.identity = identity.to_owned();
            self.reasons.clear();
        }
        if self.reasons.contains(&reason) {
            return false;
        }
        self.reasons.push(reason);
        true
    }
}

fn speculative_native_vram_admitted(
    resident_bytes: u64,
    allocation: crate::gpu_lanczos::NativeAllocationBytes,
    budget: Option<u64>,
) -> bool {
    budget.is_none_or(|limit| {
        resident_bytes
            .checked_add(allocation.output_bytes)
            .and_then(|total| total.checked_add(allocation.temporary_bytes))
            .is_some_and(|total| total <= limit)
    })
}

impl SpeculativeGpuFrame {
    pub(super) fn begin(&mut self, root_frame: u64) {
        if self.frame != Some(root_frame) {
            // Also bound diagnostics on backends without the native ROOT hook.
            let previous_live = &self.live_paged;
            self.native_diagnostics
                .retain(|saved| previous_live.contains(&saved.destination));
            self.frame = Some(root_frame);
            self.state = RegistrationState::Available;
            self.live_paged.clear();
            self.canonical.clear();
        }
    }

    pub(super) fn begin_pass(&mut self) {
        self.live_paged.clear();
        self.canonical.clear();
    }

    pub(super) fn observe(&mut self, destination: crate::ui_fullscreen::PresentationDestination) {
        if !self.live_paged.contains(&destination) {
            self.live_paged.push(destination);
        }
    }

    pub(super) fn note_page_turn_activity(&mut self) {
        // A canceled/parked presentation and a discarded pass cannot refund the frame.
        self.state = RegistrationState::PageTurnActivity;
    }

    pub(super) fn note_ordinary_upload(&mut self) {
        if self.state == RegistrationState::Available {
            self.state = RegistrationState::OrdinaryUpload;
        }
    }

    #[allow(dead_code)] // Activated only with the bounded D3 producers.
    fn claim(&mut self) -> bool {
        if self.frame.is_none() || self.state != RegistrationState::Available {
            return false;
        }
        self.state = RegistrationState::SpeculativeSubmitted;
        true
    }
}

/// Options are the allocation's real options, including Nearest's absent mip
/// chain. Cache hits and shared handles bypass this allocation API and consume
/// no registration slot.
pub(super) fn speculative_vram_admitted(
    resident_bytes: u64,
    size: [usize; 2],
    options: egui::TextureOptions,
    budget: Option<u64>,
) -> bool {
    let texels = crate::vram_budget::texture_size_texels(size, options.mipmap_mode.is_some());
    let increment = crate::vram_budget::rgba8_texels_to_bytes(texels);
    budget.is_none_or(|limit| {
        resident_bytes
            .checked_add(increment)
            .is_some_and(|total| total <= limit)
    })
}

impl App {
    pub(crate) fn canonical_native_demand_was_observed(
        &self,
        demand: &crate::ui_fullscreen::canonical_demand::CanonicalUnitDemand,
    ) -> bool {
        self.speculative_gpu_frame.canonical.contains(demand)
    }

    pub(crate) fn observe_canonical_native_demand(
        &mut self,
        demand: crate::ui_fullscreen::canonical_demand::CanonicalUnitDemand,
    ) {
        self.speculative_gpu_frame
            .canonical
            .retain(|previous| previous.destination != demand.destination);
        self.speculative_gpu_frame.canonical.push(demand);
    }

    pub(crate) fn observe_canonical_native_unavailable(&mut self, reason: &'static str) {
        let route = self.page_turn_destination();
        self.speculative_gpu_frame
            .canonical
            .retain(|candidate| candidate.destination != route);
        if crate::perf::is_enabled() {
            let identity = format!(
                "{}:{:?}:resident",
                self.items_generation, self.fullscreen_idx
            );
            self.record_native_prepare_gpu_deferred(route, &identity, reason, None);
        }
    }

    fn record_native_prepare_gpu_deferred(
        &mut self,
        destination: crate::ui_fullscreen::PresentationDestination,
        identity: &str,
        reason: &'static str,
        key: Option<crate::gpu_lanczos::LanczosCacheKey>,
    ) {
        if !crate::perf::is_enabled() {
            return;
        }
        let diagnostics = &mut self.speculative_gpu_frame.native_diagnostics;
        let index = diagnostics
            .iter()
            .position(|saved| saved.destination == destination);
        let saved = match index {
            Some(index) => &mut diagnostics[index],
            None => {
                diagnostics.push(NativeGpuDiagnostics {
                    destination,
                    identity: identity.to_owned(),
                    reasons: Vec::new(),
                });
                diagnostics.last_mut().unwrap()
            }
        };
        if !saved.first(identity, reason) {
            return;
        }
        crate::perf::event(
            "fs",
            "final_prepare_gpu_deferred",
            None,
            self.input_seq,
            &[
                ("context", destination.context.serial().into()),
                ("stage", "native".into()),
                ("reason", reason.into()),
                (
                    "native_key",
                    key.map(|key| key.perf_key().into())
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("outer_frame", self.speculative_gpu_frame.frame.into()),
            ],
        );
    }

    fn finalize_native_prewarm(
        &mut self,
        ctx: &egui::Context,
        unit: &crate::ui_fullscreen::canonical_demand::CanonicalUnitDemand,
        blocked: bool,
    ) {
        use crate::gpu_lanczos::{NativePrewarmExecution, NativePrewarmQuery};
        let identity = crate::perf::is_enabled().then(|| format!("{unit:?}"));
        let identity = identity.as_deref().unwrap_or("");
        if !self.canonical_unit_demand_is_current(ctx, unit) {
            self.record_native_prepare_gpu_deferred(
                unit.destination,
                identity,
                "inputs_changed",
                None,
            );
            return;
        }
        for page in &unit.pages {
            let Some(resource) = self.canonical_demand_resource_if_current(ctx, unit, page) else {
                self.record_native_prepare_gpu_deferred(
                    unit.destination,
                    identity,
                    "inputs_changed",
                    None,
                );
                return;
            };
            let render_state = self.wgpu_render_state.clone();
            let query = self.fs_lanczos_cache.query_prewarm(
                render_state.as_ref(),
                &resource,
                page.transform.total_scale,
                page.transform.pixels_per_point,
                page.visible_region,
                page.post_filter,
                page.smoothing_percent,
                page.anime_source_limit,
            );
            let plan = match query {
                NativePrewarmQuery::Hit(_) | NativePrewarmQuery::NotNeeded => continue,
                NativePrewarmQuery::Unavailable(reason) => {
                    self.record_native_prepare_gpu_deferred(
                        unit.destination,
                        identity,
                        reason.as_str(),
                        None,
                    );
                    if matches!(
                        reason,
                        crate::gpu_lanczos::NativePrewarmUnavailable::MissingBacking
                    ) && matches!(
                        self.speculative_gpu_frame.state,
                        RegistrationState::OrdinaryUpload | RegistrationState::SpeculativeSubmitted
                    ) {
                        // An allocation in this output has not reached the normal
                        // delivery owner yet. Reevaluate after that delivery.
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                        ctx.request_repaint_of(unit.destination.viewport);
                    }
                    continue;
                }
                NativePrewarmQuery::Miss(plan) => plan,
            };
            let reason = match self.speculative_gpu_frame.state {
                RegistrationState::PageTurnActivity => {
                    Some(FinalPrepareGpuDeferred::AnimationActivity)
                }
                _ if blocked => Some(FinalPrepareGpuDeferred::LivePresentation),
                RegistrationState::OrdinaryUpload => Some(FinalPrepareGpuDeferred::OrdinaryUpload),
                RegistrationState::SpeculativeSubmitted => Some(FinalPrepareGpuDeferred::FrameBusy),
                RegistrationState::Available => None,
            };
            if let Some(reason) = reason {
                self.record_native_prepare_gpu_deferred(
                    unit.destination,
                    identity,
                    reason.name(),
                    Some(plan.key()),
                );
                if matches!(
                    reason,
                    FinalPrepareGpuDeferred::OrdinaryUpload | FinalPrepareGpuDeferred::FrameBusy
                ) {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ctx.request_repaint_of(unit.destination.viewport);
                }
                return;
            }
            // query released its renderer lock. The normal walker accounts every
            // context/cache/retained batch, deduplicating the usual output IDs.
            let allocation = plan.allocation();
            if !speculative_native_vram_admitted(
                self.vram_accounting_snapshot().total_bytes(),
                allocation,
                self.vram_pool_budget().pool_bytes,
            ) {
                self.record_native_prepare_gpu_deferred(
                    unit.destination,
                    identity,
                    "vram_admission",
                    Some(plan.key()),
                );
                continue;
            }
            let Some(render_state) = render_state else {
                continue;
            };
            let outcome =
                self.fs_lanczos_cache
                    .execute_prewarm(&render_state, &resource, plan, |_| {
                        self.speculative_gpu_frame.claim()
                    });
            match outcome {
                NativePrewarmExecution::Hit => {}
                NativePrewarmExecution::Unavailable(reason) => {
                    self.record_native_prepare_gpu_deferred(
                        unit.destination,
                        identity,
                        reason.as_str(),
                        Some(plan.key()),
                    );
                }
                NativePrewarmExecution::Submitted {
                    output_texture_id,
                    stats,
                } => {
                    if crate::perf::is_enabled() {
                        let key = plan.key();
                        crate::perf::event(
                            "fs",
                            "final_prepare_native_submitted",
                            None,
                            self.input_seq,
                            &[
                                ("renderer", render_state.renderer.read().diag_id().into()),
                                ("context", unit.destination.context.serial().into()),
                                (
                                    "source_texture_id",
                                    format!("{:?}", key.source_texture_id()).into(),
                                ),
                                ("output_texture_id", format!("{output_texture_id:?}").into()),
                                ("native_key", key.perf_key().into()),
                                ("scale_branch", key.scale_branch().as_str().into()),
                                ("roi", format!("{:?}", key.visible_source_uv_rect()).into()),
                                ("target", serde_json::json!(key.target_size())),
                                ("encode_submit_cpu_ms", stats.encode_submit_cpu_ms.into()),
                                ("output_bytes", allocation.output_bytes.into()),
                                ("temporary_bytes", allocation.temporary_bytes.into()),
                                ("outer_frame", self.speculative_gpu_frame.frame.into()),
                            ],
                        );
                    }
                    // Only a remaining miss may request the existing busy-frame
                    // wake. A final hit/Native submission adds no idle repaint.
                }
            }
        }
    }

    pub(super) fn defer_legacy_final_prefetch_registration(&self) -> bool {
        self.settings.page_turn_animation_enabled
            && self.reading_flow.is_paged()
            && !self.spread_mode.is_split()
    }

    /// Allocation policy follows the non-display request, not the D3 CPU-owner
    /// label. Older colorize/LUT prefetch is also speculative in ON/paged.
    pub(super) fn final_prepare_is_speculative_allocation(
        &self,
        pending: &FinalEffectPending,
    ) -> bool {
        pending.origin.is_speculative()
            || (matches!(
                pending.origin,
                final_prepare::FinalEffectOrigin::LegacyPrefetch
            ) && self.defer_legacy_final_prefetch_registration()
                && matches!(
                    pending.state,
                    final_prepare::FinalEffectState::CpuReady(
                        final_prepare::FinalEffectReady::Legacy(FinalCompositeResult::Ready { .. })
                    )
                ))
    }

    pub(super) fn admit_final_prepare_registration(
        &mut self,
        size: [usize; 2],
        options: egui::TextureOptions,
    ) -> FinalPrepareGpuAdmission {
        if !speculative_vram_admitted(
            self.vram_accounting_snapshot().total_bytes(),
            size,
            options,
            self.vram_pool_budget().pool_bytes,
        ) {
            FinalPrepareGpuAdmission::OverBudget
        } else if self.speculative_gpu_frame.claim() {
            FinalPrepareGpuAdmission::Admitted
        } else {
            FinalPrepareGpuAdmission::FrameBusy
        }
    }

    #[allow(dead_code)] // D3-1 never invokes an allocation through this gate.
    pub(super) fn admit_speculative_gpu_allocation(
        &mut self,
        size: [usize; 2],
        options: egui::TextureOptions,
    ) -> bool {
        // The normal resident walker includes every mounted/at-rest/parked owner
        // and live presentation/batch leases. Never evict to make D3 fit.
        speculative_vram_admitted(
            self.vram_accounting_snapshot().total_bytes(),
            size,
            options,
            self.vram_pool_budget().pool_bytes,
        ) && self.speculative_gpu_frame.claim()
    }
}

pub(super) enum FinalPrepareGpuAdmission {
    Admitted,
    FrameBusy,
    OverBudget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FinalPrepareGpuDeferred {
    BeforeFinalOutput,
    AnimationActivity,
    LivePresentation,
    OrdinaryUpload,
    FrameBusy,
    OverBudget,
}

impl FinalPrepareGpuDeferred {
    fn name(self) -> &'static str {
        match self {
            Self::BeforeFinalOutput => "before_final_output",
            Self::AnimationActivity => "animation_activity",
            Self::LivePresentation => "live_presentation",
            Self::OrdinaryUpload => "ordinary_upload",
            Self::FrameBusy => "frame_busy",
            Self::OverBudget => "vram_admission",
        }
    }
}

/// Diagnostics only, retained by the existing candidate until promotion/drop.
/// Each reason is emitted once for that candidate, even across frames/retries.
#[derive(Default)]
pub(super) struct FinalPrepareGpuDiagnostics(u8);

impl FinalPrepareGpuDiagnostics {
    fn first(&mut self, reason: FinalPrepareGpuDeferred) -> bool {
        let bit = 1 << reason as u8;
        let first = self.0 & bit == 0;
        self.0 |= bit;
        first
    }
}

#[derive(Clone, Copy)]
pub(super) enum FinalPrepareAdoption {
    BeforeLookup,
    RootFinalized(Option<FinalPrepareGpuDeferred>),
}

impl App {
    pub(crate) fn observe_live_paged_for_final_prepare(&mut self) {
        if self.fullscreen_idx.is_some()
            && self.reading_flow.is_paged()
            && !self.spread_mode.is_split()
        {
            self.speculative_gpu_frame
                .observe(self.page_turn_destination());
        }
    }

    pub(crate) fn note_page_turn_gpu_activity(&mut self) {
        self.speculative_gpu_frame.note_page_turn_activity();
    }

    fn final_prepare_route_is_current(
        &self,
        route: crate::ui_fullscreen::PresentationDestination,
    ) -> bool {
        #[cfg(windows)]
        if let Some(window) = route.window {
            return self.active_detached_window_id() == Some(window)
                && self
                    .locate_window_context(window)
                    .is_some_and(|(id, _)| id == route.context)
                && matches!(
                    self.detached_window_state(window),
                    Some(
                        DetachedWindowState::Opening
                            | DetachedWindowState::Resuming
                            | DetachedWindowState::Active
                    )
                );
        }
        #[cfg(not(windows))]
        if route.window.is_some() {
            return false;
        }
        route.context == self.viewer_context_main()
    }

    pub(super) fn finalize_speculative_gpu_output(
        &mut self,
        ctx: &egui::Context,
        frame: u64,
        passes: usize,
    ) {
        let started = std::time::Instant::now();
        // The counter has advanced on return from run_ui. Never begin a new frame here.
        if self.speculative_gpu_frame.frame != Some(frame) {
            return;
        }
        let routes = self.speculative_gpu_frame.live_paged.clone();
        let live: Vec<_> = routes
            .into_iter()
            .filter(|route| self.final_prepare_route_is_current(*route))
            .collect();
        let mut blocked = false;
        for route in &live {
            blocked |= self
                .with_viewer_context_state(route.context, |app| {
                    app.page_turn_destination() == *route
                        && app.fullscreen_idx.is_some()
                        && app.reading_flow.is_paged()
                        && !app.spread_mode.is_split()
                        && app.page_turn_presentation_pending()
                })
                .unwrap_or(false);
        }
        let reason = match self.speculative_gpu_frame.state {
            RegistrationState::PageTurnActivity => Some(FinalPrepareGpuDeferred::AnimationActivity),
            _ if blocked => Some(FinalPrepareGpuDeferred::LivePresentation),
            RegistrationState::OrdinaryUpload => Some(FinalPrepareGpuDeferred::OrdinaryUpload),
            RegistrationState::SpeculativeSubmitted => Some(FinalPrepareGpuDeferred::FrameBusy),
            RegistrationState::Available => None,
        };
        for route in &live {
            let _ = self.with_viewer_context_state(route.context, |app| {
                if app.page_turn_destination() != *route
                    || app.fullscreen_idx.is_none()
                    || !app.reading_flow.is_paged()
                    || app.spread_mode.is_split()
                {
                    return;
                }
                let completed = app.collect_final_prepare_results();
                app.adopt_final_prepare_results(
                    ctx,
                    completed,
                    FinalPrepareAdoption::RootFinalized(reason),
                );
            });
        }
        // All contexts' final/comic registrations get first use of the common
        // slot. Native requests must never jump ahead of another owner's final.
        // Keep the current callback values available during revalidation: an
        // older view must not become current merely by replaying its saved view.
        let candidates = self.speculative_gpu_frame.canonical.clone();
        for unit in candidates {
            if !live.contains(&unit.destination) {
                continue;
            }
            let _ = self.with_viewer_context_state(unit.destination.context, |app| {
                app.finalize_native_prewarm(ctx, &unit, blocked);
            });
        }
        self.speculative_gpu_frame.canonical.clear();
        self.speculative_gpu_frame
            .native_diagnostics
            .retain(|saved| live.contains(&saved.destination));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "final_prepare_output_finalized",
                None,
                self.input_seq,
                &[
                    ("outer_frame", frame.into()),
                    ("passes", passes.into()),
                    ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                ],
            );
        }
    }

    pub(super) fn record_final_prepare_gpu_deferred(
        &mut self,
        key: FinalCompositeKey,
        reason: FinalPrepareGpuDeferred,
    ) {
        if !crate::perf::is_enabled() {
            return;
        }
        if !self
            .final_effect_pending
            .get(&key)
            .is_some_and(|pending| self.final_prepare_is_speculative_allocation(pending))
        {
            return;
        }
        let Some(pending) = self.final_effect_pending.get_mut(&key) else {
            return;
        };
        if !matches!(pending.state, final_prepare::FinalEffectState::CpuReady(_)) {
            return;
        }
        if !pending.gpu_diagnostics.first(reason) {
            return;
        }
        crate::perf::event(
            "fs",
            "final_prepare_gpu_deferred",
            None,
            self.input_seq,
            &[
                (
                    "context",
                    self.projected_viewer_context_id().serial().into(),
                ),
                ("idx", key.edit_key.idx.into()),
                ("params_hash", key.params_hash.into()),
                ("reason", reason.name().into()),
                ("outer_frame", self.speculative_gpu_frame.frame.into()),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d3_4_deferred_diagnostics_emit_each_candidate_reason_only_once() {
        let mut candidate = FinalPrepareGpuDiagnostics::default();
        for iteration in 0..3 {
            for reason in [
                FinalPrepareGpuDeferred::AnimationActivity,
                FinalPrepareGpuDeferred::LivePresentation,
                FinalPrepareGpuDeferred::OrdinaryUpload,
                FinalPrepareGpuDeferred::FrameBusy,
                FinalPrepareGpuDeferred::OverBudget,
                FinalPrepareGpuDeferred::BeforeFinalOutput,
            ] {
                assert_eq!(candidate.first(reason), iteration == 0);
            }
        }
    }

    #[test]
    fn d3_registration_budget_survives_reentry_and_ordinary_upload_after_claim() {
        let mut frame = SpeculativeGpuFrame::default();
        assert!(!frame.claim());
        frame.begin(10);
        assert!(frame.claim());
        frame.begin(10); // viewport, context swap, discard retry
        frame.note_ordinary_upload(); // allowed, does not refund speculative work
        assert!(!frame.claim());
        frame.begin(11);
        frame.note_ordinary_upload();
        assert!(!frame.claim());
        frame.begin(12);
        assert!(frame.claim());
    }

    #[test]
    fn d3_vram_admission_uses_actual_options_and_exact_mip_sum() {
        // Resident includes thumbnails as well as fullscreen allocations. Its
        // matching budget is the whole pool, not the fullscreen suballocation.
        let budget = crate::vram_budget::VramPoolBudget::from_pool_bytes(
            Some(1_000),
            crate::vram_budget::VramBudgetMode::Fullscreen,
        );
        assert!(speculative_vram_admitted(
            750,
            [5, 5],
            egui::TextureOptions::NEAREST,
            budget.pool_bytes,
        ));
        let size = [8000, 8192];
        let base = 8000 * 8192 * 4;
        assert!(speculative_vram_admitted(
            0,
            size,
            egui::TextureOptions::NEAREST,
            Some(base)
        ));
        assert!(!speculative_vram_admitted(
            1,
            size,
            egui::TextureOptions::NEAREST,
            Some(base)
        ));
        assert!(!speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(base)
        ));
        assert!(speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(349_525_084)
        ));
        assert!(!speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(349_525_083)
        ));
        assert!(speculative_vram_admitted(
            u64::MAX,
            size,
            egui::TextureOptions::NEAREST,
            None
        ));
        assert!(!speculative_vram_admitted(
            u64::MAX,
            size,
            egui::TextureOptions::NEAREST,
            Some(u64::MAX)
        ));
    }
}

#[cfg(test)]
#[path = "final_prepare_budget/tests.rs"]
mod scheduling_tests;

#[cfg(test)]
#[path = "final_prepare_budget/native_tests.rs"]
mod native_tests;
