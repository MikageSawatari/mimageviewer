//! The presentation owner holds one already-resolved navigation until its target
//! has contributed a complete canonical flat to an adopted output.
use super::*;

pub(super) enum FlatCompletion {
    NeedFlat,
    Submitted(SubmittedUnitReceipt),
}

pub(super) struct ResolvedPageTurnNavigation {
    pub(super) fact: ViewerOperationFact,
    pub(super) from_idx: usize,
    pub(super) target_idx: usize,
    intent: ResolvedNavigationIntent,
    pub(super) contract: crate::fs_page_load_scheduler::FsPageLoadContract,
    pub(super) items: u64,
    pub(super) layout: Arc<Vec<SpreadDisplayUnit>>,
}

#[derive(Clone, Copy)]
enum ResolvedNavigationIntent {
    Page {
        slice: Option<crate::page_split::PageSlice>,
    },
    Slideshow {
        slice: Option<crate::page_split::PageSlice>,
        purpose: crate::app::book_composition_permission::BookTargetPurpose,
    },
    SpreadShift {
        anchor: usize,
    },
}

#[cfg(test)]
impl PageTurnPresentation {
    pub(super) fn continuation_slideshow_purpose_for_test(
        &self,
    ) -> Option<crate::app::book_composition_permission::BookTargetPurpose> {
        let PresentationState::Completing {
            continuation: Some(command),
            ..
        } = &self.state
        else {
            return None;
        };
        match command.intent {
            ResolvedNavigationIntent::Slideshow { purpose, .. } => Some(purpose),
            _ => None,
        }
    }
}

impl App {
    pub(crate) fn page_turn_wheel_navigation_operation(
        &self,
        ctx: &egui::Context,
        raw_ordinal: Option<u32>,
    ) -> Option<ViewerOperationId> {
        let ordinal = raw_ordinal?;
        self.page_turn_presentation
            .transaction
            .facts
            .iter()
            .find(|fact| {
                fact.destination == self.page_turn_destination()
                    && fact.source == ctx.viewport_id()
                    && fact.cause == "wheel"
                    && matches!(fact.operation_id, ViewerOperationId::Pointer { ordinal: actual, .. } if actual == ordinal)
            })
            .map(|fact| fact.operation_id)
    }

    pub(crate) fn page_turn_pointer_navigation_operation(
        &self,
        ctx: &egui::Context,
        cause: &'static str,
    ) -> Option<ViewerOperationId> {
        self.page_turn_presentation
            .transaction
            .facts
            .iter()
            .rev()
            .find(|fact| {
                fact.destination == self.page_turn_destination()
                    && fact.source == ctx.viewport_id()
                    && fact.cause == cause
            })
            .map(|fact| fact.operation_id)
    }
    pub(crate) fn page_turn_presentation_pending(&self) -> bool {
        matches!(
            self.page_turn_presentation.state,
            PresentationState::Offered { .. }
                | PresentationState::Turning { .. }
                | PresentationState::Completing { .. }
        )
    }
    pub(crate) fn page_turn_barrier_active(&self) -> bool {
        matches!(
            self.page_turn_presentation.state,
            PresentationState::Turning { .. } | PresentationState::Completing { .. }
        )
    }
    pub(super) fn record_page_turn_completion_flat(&mut self, receipt: &SubmittedUnitReceipt) {
        if receipt.curl {
            return;
        }
        let PresentationState::Completing {
            incoming,
            completion,
            ..
        } = &mut self.page_turn_presentation.state
        else {
            return;
        };
        if receipt.destination() != incoming.destination
            || receipt.pages().len() != incoming.pages.len()
            || !incoming.pages.iter().all(|page| {
                receipt
                    .pages()
                    .iter()
                    .any(|(occurrence, source, paint, area)| {
                        *occurrence == page.occurrence
                            && *source == page.resource.source_texture_id()
                            && *paint == page.resource.paint_texture_id()
                            && *area > 0.0
                    })
            })
        {
            return;
        }
        if let FlatCompletion::Submitted(previous) = completion
            && previous.was_accepted()
            && previous.frame() < receipt.frame()
        {
            return;
        }
        *completion = FlatCompletion::Submitted(receipt.clone());
    }

    pub(crate) fn page_turn_completion_active(&self) -> bool {
        matches!(
            self.page_turn_presentation.state,
            PresentationState::Completing { .. }
        )
    }

    /// Admission precedes every target mutation and producer/history side effect.
    /// Returning true means this request belongs to the presentation barrier (or
    /// was dropped by its existing one-operation budget), never to a second queue.
    pub(crate) fn page_turn_defer_navigation(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        slice: Option<crate::page_split::PageSlice>,
        contract: crate::fs_page_load_scheduler::FsPageLoadContract,
        operation: Option<ViewerOperationId>,
    ) -> bool {
        self.page_turn_defer_navigation_intent(
            ctx,
            target_idx,
            contract,
            operation,
            ResolvedNavigationIntent::Page { slice },
        )
    }

    pub(crate) fn page_turn_defer_slideshow_navigation(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        slice: Option<crate::page_split::PageSlice>,
        purpose: crate::app::book_composition_permission::BookTargetPurpose,
        operation: ViewerOperationId,
    ) -> bool {
        self.page_turn_defer_navigation_intent(
            ctx,
            target_idx,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            Some(operation),
            ResolvedNavigationIntent::Slideshow { slice, purpose },
        )
    }

    pub(crate) fn page_turn_defer_spread_shift(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        operation: Option<ViewerOperationId>,
        anchor: usize,
    ) -> bool {
        self.page_turn_defer_navigation_intent(
            ctx,
            target_idx,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            operation,
            ResolvedNavigationIntent::SpreadShift { anchor },
        )
    }

    fn page_turn_defer_navigation_intent(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        contract: crate::fs_page_load_scheduler::FsPageLoadContract,
        operation: Option<ViewerOperationId>,
        intent: ResolvedNavigationIntent,
    ) -> bool {
        if !matches!(
            self.page_turn_presentation.state,
            PresentationState::Turning { .. } | PresentationState::Completing { .. }
        ) {
            return false;
        }
        self.page_turn_presentation
            .cancel("navigation_flat_barrier");
        let destination = self.page_turn_destination();
        let facts = &self.page_turn_presentation.transaction.facts;
        let fact = if let Some(operation_id) = operation {
            facts
                .iter()
                .find(|fact| fact.destination == destination && fact.operation_id == operation_id)
                .copied()
                .or_else(|| {
                    Some(ViewerOperationFact {
                        destination,
                        operation_id,
                        source: ctx.viewport_id(),
                        phase: OperationPhase::Press,
                        cause: match operation_id {
                            ViewerOperationId::Key(_) => "key",
                            ViewerOperationId::Pointer { .. } => "pointer",
                            ViewerOperationId::Pad { .. } => "pad",
                            ViewerOperationId::Slideshow(_) => "slideshow",
                            ViewerOperationId::UnattributedNavigation(_) => {
                                "unidentified_navigation"
                            }
                        },
                        diagnostic_input_seq: self.input_seq,
                    })
                })
        } else {
            // Non-key producers already enter through page_turn_external_operation
            // with one immutable pad/slideshow fact. Never substitute a key fact
            // for a mouse/touch producer whose event has already been resolved.
            facts
                .iter()
                .rev()
                .find(|fact| {
                    fact.destination == destination
                        && !matches!(fact.operation_id, ViewerOperationId::Key(_))
                })
                .copied()
        };
        let command = fact.and_then(|fact| {
            self.fullscreen_idx
                .map(|from_idx| ResolvedPageTurnNavigation {
                    fact,
                    from_idx,
                    target_idx,
                    intent,
                    contract,
                    items: self.items_generation,
                    layout: Arc::clone(&self.spread_display_units_cache.units),
                })
        });
        if let PresentationState::Completing {
            continuation,
            completion,
            ..
        } = &mut self.page_turn_presentation.state
            && continuation.is_none()
        {
            *continuation = command;
            if let Some(command) = continuation {
                emit(
                    &command.fact,
                    "flat_barrier",
                    "navigation_deferred_until_canonical_flat",
                    &[("target_idx", command.target_idx.into())],
                );
            }
            // Do not replace a previously adopted flat on a same-frame replay.
            if matches!(completion, FlatCompletion::NeedFlat) {
                ctx.request_repaint();
            }
        }
        true
    }

    /// Called only at the owning frame's common close/commit/capture boundary.
    /// The observer consumes the flat receipt before the old operation is removed.
    fn page_turn_take_continuation(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<ResolvedPageTurnNavigation> {
        let destination = self.page_turn_destination();
        let invalid = match &self.page_turn_presentation.state {
            PresentationState::Completing {
                incoming,
                continuation,
                ..
            } => {
                incoming.destination != destination
                    || incoming.items != self.items_generation
                    || incoming.mode != self.spread_mode
                    || incoming.slice != self.fullscreen_page_slice
                    || !Arc::ptr_eq(&incoming.layout, &self.spread_display_units_cache.units)
                    || continuation
                        .as_ref()
                        .is_some_and(|command| self.fullscreen_idx != Some(command.from_idx))
                    || incoming
                        .pages
                        .iter()
                        .zip(&incoming.content)
                        .any(|(page, content)| {
                            *content != self.page_turn_content(page.occurrence.idx)
                        })
            }
            _ => false,
        };
        if invalid {
            self.page_turn_presentation
                .deactivate("completion_binding_or_generation_changed");
            return None;
        }
        let ready = match &self.page_turn_presentation.state {
            PresentationState::Completing {
                incoming,
                completion: FlatCompletion::Submitted(receipt),
                ..
            } => {
                !receipt.curl
                    && receipt.was_accepted()
                    && receipt.frame() < ctx.cumulative_frame_nr()
                    && receipt.destination() == destination
                    && incoming.destination == destination
                    && incoming.items == self.items_generation
                    && incoming.mode == self.spread_mode
                    && incoming.slice == self.fullscreen_page_slice
                    && Arc::ptr_eq(&incoming.layout, &self.spread_display_units_cache.units)
                    && incoming.pages.iter().all(|page| {
                        receipt
                            .pages()
                            .iter()
                            .any(|(occurrence, source, paint, area)| {
                                *occurrence == page.occurrence
                                    && *source == page.resource.source_texture_id()
                                    && *paint == page.resource.paint_texture_id()
                                    && *area > 0.0
                            })
                    })
            }
            _ => false,
        };
        if !ready {
            return None;
        }
        self.service_final_prepare_before_lookup(ctx);
        self.flush_submitted_page_turn_ready(ctx);
        // Presentation adoption precedes ordinary navigation completion. Keep
        // the same typed command if its matching observer has not retired yet.
        if self.fs_navigation_sequence_blocks_new_target() {
            return None;
        }
        let old = std::mem::replace(
            &mut self.page_turn_presentation.state,
            PresentationState::Inactive,
        );
        let PresentationState::Completing {
            incoming,
            continuation,
            ..
        } = old
        else {
            unreachable!("validated the same exclusively owned state")
        };
        incoming.reserved_bytes.store(0, Ordering::Relaxed);
        self.page_turn_presentation.state = PresentationState::Flat(incoming);
        let command = continuation?;
        if command.fact.destination != destination
            || command.items != self.items_generation
            || !Arc::ptr_eq(&command.layout, &self.spread_display_units_cache.units)
            || self.fullscreen_idx != Some(command.from_idx)
        {
            emit(&command.fact, "skip", "continuation_binding_changed", &[]);
            return None;
        }
        Some(command)
    }

    pub(crate) fn commit_page_turn_continuation(&mut self, ctx: &egui::Context) -> bool {
        let flat_receipt_frame = crate::perf::is_enabled()
            .then(|| match &self.page_turn_presentation.state {
                PresentationState::Completing {
                    completion: FlatCompletion::Submitted(receipt),
                    ..
                } => Some(receipt.frame()),
                _ => None,
            })
            .flatten();
        let Some(command) = self.page_turn_take_continuation(ctx) else {
            return false;
        };
        let drawing_path_before =
            crate::perf::is_enabled().then(|| self.page_turn_current_drawing_path());
        let mut perf = None;
        // It is a resolved ordinary navigation, never another leaf offer. Keep
        // the original operation identity and use the established landing owner.
        emit(
            &command.fact,
            "skip",
            "stage1_continuation_instant",
            &[("target_idx", command.target_idx.into())],
        );
        let slice = match command.intent {
            ResolvedNavigationIntent::Page { slice }
            | ResolvedNavigationIntent::Slideshow { slice, .. } => slice,
            ResolvedNavigationIntent::SpreadShift { .. } => None,
        };
        if let ResolvedNavigationIntent::SpreadShift { anchor } = command.intent {
            if !self.set_live_spread_shift(anchor) {
                return true;
            }
            self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
        }
        if let ResolvedNavigationIntent::Slideshow { slice, purpose } = command.intent {
            self.complete_book_slideshow_target(ctx, command.target_idx, slice, purpose);
        } else {
            if command.target_idx == command.from_idx
                && let ResolvedNavigationIntent::Page { slice: Some(slice) } = command.intent
            {
                self.set_fullscreen_page_slice_for_display_unit(slice);
                self.update_fs_page_turn_burst_after_navigation(ctx, command.from_idx, None);
                ctx.request_repaint();
                return true;
            }
            if command.target_idx == command.from_idx
                || command.contract == crate::fs_page_load_scheduler::FsPageLoadContract::LatestSeek
                || self.begin_fs_page_navigation_sequence(
                    ctx,
                    command.from_idx,
                    command.target_idx,
                    false,
                )
            {
                self.land_still_page_navigation_target(
                    ctx,
                    command.from_idx,
                    command.target_idx,
                    command.contract,
                    &mut perf,
                );
                if let Some(slice) = slice
                    && self.fullscreen_idx == Some(command.target_idx)
                {
                    self.set_fullscreen_page_slice_for_display_unit(slice);
                }
            }
        }
        if let Some(before) = drawing_path_before {
            let after = self.page_turn_current_drawing_path();
            emit(
                &command.fact,
                "continuation_commit",
                "stage1_instant",
                &[
                    ("drawing_path_before", before.into()),
                    ("drawing_path_after", after.into()),
                    ("drawing_path_changed", (before != after).into()),
                    (
                        "continuation_commit_frame",
                        ctx.cumulative_frame_nr().into(),
                    ),
                    ("flat_receipt_frame", flat_receipt_frame.into()),
                    ("target_idx", command.target_idx.into()),
                    (
                        "target_reached",
                        (self.fullscreen_idx == Some(command.target_idx)).into(),
                    ),
                ],
            );
        }
        true
    }

    /// Read the same current-media predicates as the normal renderer. This is
    /// perf correlation only; drawing dispatch remains the existing owner's job.
    fn page_turn_current_drawing_path(&self) -> &'static str {
        let Some(idx) = self.fullscreen_idx else {
            return "closed";
        };
        #[cfg(windows)]
        if self.native_video_backdrop_target_for_fs(idx) {
            return if self.native_video_in_window_active {
                "native_in_window"
            } else {
                "native_fullscreen"
            };
        }
        #[cfg(not(windows))]
        let _ = idx;
        if self.fullscreen_embedded_still_active() {
            "embedded_still"
        } else {
            "viewport_still"
        }
    }
}
