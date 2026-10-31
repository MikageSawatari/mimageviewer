//! Diagnostics describe existing decisions; they do not own work or change admission.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedEditReject {
    Cancelled,
    ItemsGeneration,
    ItemIdentity,
    EditGeneration,
    CompositeKey,
    Context,
    PageIdentity,
    Params,
    ProducerConflict,
    LayersChanged,
    NormalizationRequired,
    FinalBaseChanged,
    SourceMissing,
    SourceArcChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComicExclusion {
    Generation,
    DocumentChanged,
    FontSetChanged,
    FontKeysChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FinalPipelineInvalidation {
    CreativeLutLoaded,
    CreativeLutSettingsChanged,
    ItemsChanged,
    PageEditsReplaced,
    VirtualPagesReprepared,
    EraseGenerationChanged,
    ConcealGenerationChanged,
    EditStagesChanged,
    AdjustmentChanged,
    AiGenerationChanged,
    FinalAiCompleted,
    DisplayEffectReplaced,
    AcceptedAdjustment,
    SessionClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedEditCancel {
    Rejected(PreparedEditReject),
    EditingEntry,
    DisplayComputing,
    DemandObsolete,
    KeepSetEviction,
    Pressure,
    NextPriority,
    FullResolutionDeferred,
    PipelineInvalidation(FinalPipelineInvalidation),
    RequestSuperseded,
    WorkerDisconnected,
    WorkerFailed,
    SessionRetired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedEditDisposition {
    Cancelled(PreparedEditCancel),
    ComicExcluded(ComicExclusion),
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedEditEvent {
    pub request_id: u64,
    pub context: u64,
    pub key: FinalCompositeKey,
    pub state: &'static str,
    pub frame: Option<u64>,
    pub pass: Option<usize>,
    pub entry: &'static std::panic::Location<'static>,
    pub reason: PreparedEditDisposition,
}

#[cfg(test)]
thread_local! {
    static EVENTS: std::cell::RefCell<Option<Vec<PreparedEditEvent>>> = const { std::cell::RefCell::new(None) };
}

/// Opt-in, thread-local observation of the same typed records sent to perf.
/// It is available with perf disabled and never participates in state decisions.
#[cfg(test)]
pub(crate) struct PreparedEditEvents;

#[cfg(test)]
impl PreparedEditEvents {
    pub(crate) fn start() -> Self {
        EVENTS.with_borrow_mut(|events| assert!(events.replace(Vec::new()).is_none()));
        Self
    }
    pub(crate) fn take(&self) -> Vec<PreparedEditEvent> {
        EVENTS.with_borrow_mut(|events| std::mem::take(events.as_mut().unwrap()))
    }
}

#[cfg(test)]
impl Drop for PreparedEditEvents {
    fn drop(&mut self) {
        EVENTS.with_borrow_mut(|events| *events = None);
    }
}

pub(crate) fn observing() -> bool {
    if crate::perf::is_enabled() {
        return true;
    }
    #[cfg(test)]
    return EVENTS.with_borrow(|events| events.is_some());
    #[cfg(not(test))]
    false
}

pub(crate) fn record_pending_disposition(
    pending: &FinalEffectPending,
    key: FinalCompositeKey,
    reason: PreparedEditDisposition,
    frame: Option<u64>,
    pass: Option<usize>,
    entry: &'static std::panic::Location<'static>,
    proof: Option<serde_json::Value>,
) {
    let Some(input) = pending.state.edit_input() else {
        return;
    };
    if !observing() {
        return;
    }
    let event = PreparedEditEvent {
        request_id: pending.gpu_diagnostics.request_id(),
        context: pending
            .origin
            .unit()
            .map_or(0, |unit| unit.context.serial()),
        key,
        state: match &pending.state {
            FinalEffectState::Computing(_) => "computing",
            FinalEffectState::CpuReady(_) => "cpu_ready",
            FinalEffectState::Registering(_) => "registering",
        },
        frame,
        pass,
        entry,
        reason,
    };
    if crate::perf::is_enabled() {
        crate::perf::event(
            "fs",
            "prepared_edit_disposition",
            Some(&input.page_key),
            0,
            &[
                ("request_id", event.request_id.into()),
                ("context", event.context.into()),
                ("idx", key.edit_key.idx.into()),
                ("edit_key", format!("{:?}", key.edit_key).into()),
                ("composite_key", format!("{key:?}").into()),
                ("params_hash", key.params_hash.into()),
                ("state", event.state.into()),
                (
                    "frame",
                    frame.map(serde_json::Value::from).unwrap_or_default(),
                ),
                (
                    "pass",
                    pass.map(serde_json::Value::from).unwrap_or_default(),
                ),
                ("entry", format!("{}:{}", entry.file(), entry.line()).into()),
                (
                    "disposition",
                    match reason {
                        PreparedEditDisposition::Cancelled(_) => "cancelled",
                        PreparedEditDisposition::ComicExcluded(_) => "comic_excluded",
                    }
                    .into(),
                ),
                ("reason", format!("{reason:?}").into()),
                ("proof", proof.unwrap_or_default()),
            ],
        );
    }
    #[cfg(test)]
    EVENTS.with_borrow_mut(|events| {
        if let Some(events) = events {
            events.push(event);
        }
    });
}

impl App {
    #[track_caller]
    pub(in crate::app) fn record_prepared_edit_disposition(
        &self,
        key: FinalCompositeKey,
        reason: PreparedEditDisposition,
    ) {
        if !observing() {
            return;
        }
        let Some(pending) = self.final_effect_pending.get(&key) else {
            return;
        };
        let Some(input) = pending.state.edit_input() else {
            return;
        };
        // No document serialization, mask hashing, or allocation identity work
        // unless an observer is enabled. The same source resolver validates B.
        let proof = if crate::perf::is_enabled() {
            let current = self.current_prepared_edit_source(input);
            Some(serde_json::json!({
                "expected_allocation": Arc::as_ptr(&input.source) as usize,
                "expected_size": input.source.size,
                "current_allocation": current.as_ref().map(|p| Arc::as_ptr(p) as usize),
                "current_size": current.as_ref().map(|p| p.size),
                "memory_layers_equal": match &input.local {
                    Some(LocalMaterializeLayers::Memory(layers)) => Some(self.local_adjust_page_layers.get(&key.edit_key.idx) == Some(layers)),
                    _ => None,
                },
            }))
        } else {
            None
        };
        record_pending_disposition(
            pending,
            key,
            reason,
            Some(self.frame_counter),
            self.edit_preview_repaint_ctx
                .as_ref()
                .map(egui::Context::current_pass_index),
            std::panic::Location::caller(),
            proof,
        );
    }

    /// Every cancellation records while the request, resources and state still
    /// belong to this map. Legacy jobs retain their previous cancellation policy.
    #[track_caller]
    pub(in crate::app) fn cancel_final_effect_key(
        &mut self,
        key: FinalCompositeKey,
        reason: PreparedEditCancel,
    ) {
        self.record_prepared_edit_disposition(key, PreparedEditDisposition::Cancelled(reason));
        if let Some(pending) = self.final_effect_pending.remove(&key) {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
