//! Accepted viewer-local shift and its explicit action receipts. The service owns durable rows.
use super::*;
use crate::{book_scope::BookScopeKey, spread_shift::*};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ViewerShift {
    #[default]
    None,
    Ordinary(usize),
    Book(SpreadShiftState),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ShiftCommand {
    #[default]
    Idle,
    Pending {
        sequence: u64,
        operation: ShiftOperation,
        clear: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum FencedResume {
    #[default]
    Unprepared,
    Ready {
        key: String,
        position: Option<crate::book_resume_db::ReadingPosition>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewerSpreadShift {
    pub(crate) scope: Option<BookScopeKey>,
    pub(crate) state: ViewerShift,
    pub(crate) availability: PairingEditAvailability,
    operation_sequence: u64,
    command: ShiftCommand,
    resume: FencedResume,
    retained: RetainedReadoption,
    projection: Option<(
        Arc<file_type_resume::PreparedPageIdentityIndex>,
        Arc<Vec<usize>>,
    )>,
}

/// Retained adoption is a read dependency owned by this context, not the global hydration job.
#[derive(Default)]
enum RetainedReadoption {
    #[default]
    Idle,
    Reading {
        restore: Box<ViewerBookRestore>,
        started_at: std::time::Instant,
    },
}
impl Clone for RetainedReadoption {
    // Fork copies accepted session/layout only. A receiver is never shared with the new context.
    fn clone(&self) -> Self {
        Self::Idle
    }
}
impl std::fmt::Debug for RetainedReadoption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("Idle"),
            Self::Reading { restore, .. } => {
                f.debug_tuple("Reading").field(&restore.receipt).finish()
            }
        }
    }
}
impl PartialEq for RetainedReadoption {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Idle, Self::Idle) => true,
            (Self::Reading { restore: left, .. }, Self::Reading { restore: right, .. }) => {
                left.receipt == right.receipt
            }
            _ => false,
        }
    }
}
impl Eq for RetainedReadoption {}

impl Default for PairingEditAvailability {
    fn default() -> Self {
        Self::Automatic
    }
}

impl ViewerSpreadShift {
    /// A materialized fork inherits accepted layout, never the source context's command.
    pub(crate) fn fork_accepted(&self) -> Self {
        let mut fork = self.clone();
        fork.command = ShiftCommand::Idle;
        fork.resume = FencedResume::Unprepared;
        if matches!(
            fork.state,
            ViewerShift::Book(SpreadShiftState::Adjusted {
                persistence: ShiftPersistence::Saving(_) | ShiftPersistence::SaveFailed,
                ..
            })
        ) {
            fork.session_only();
        }
        fork
    }

    /// The captured position serves only the open continuation's initial adoption.
    pub(crate) fn finish_fenced_book_resume(&mut self) {
        self.resume = FencedResume::Unprepared;
    }

    pub(crate) fn retained_readoption_pending(&self) -> bool {
        matches!(self.retained, RetainedReadoption::Reading { .. })
    }
    pub(crate) fn retained_readoption_started_at(&self) -> Option<std::time::Instant> {
        match self.retained {
            RetainedReadoption::Reading { started_at, .. } => Some(started_at),
            _ => None,
        }
    }
    pub(crate) fn cancel_retained_readoption(&mut self) {
        self.retained = RetainedReadoption::Idle;
    }
    pub(crate) fn invalidate_projection(&mut self) {
        self.cancel_retained_readoption();
        self.projection = None;
        self.command = ShiftCommand::Idle;
        if let ViewerShift::Book(SpreadShiftState::Adjusted {
            persistence: ShiftPersistence::Saving(_),
            ..
        }) = &self.state
        {
            self.session_only();
        }
    }
    pub(crate) fn detach_book_scope_for_ordinary_list(&mut self) {
        let anchor = self.anchor_idx();
        *self = Self {
            state: anchor.map_or(ViewerShift::None, ViewerShift::Ordinary),
            ..Default::default()
        };
    }
    pub(crate) fn anchor_idx(&self) -> Option<usize> {
        if self.availability != PairingEditAvailability::Automatic {
            return None;
        }
        match &self.state {
            ViewerShift::Ordinary(index) => Some(*index),
            ViewerShift::Book(SpreadShiftState::Adjusted {
                resolution: ShiftResolution::Resolved(index),
                ..
            }) => Some(*index),
            _ => None,
        }
    }

    pub(crate) fn session_only(&mut self) {
        if self.availability != PairingEditAvailability::Automatic {
            return;
        }
        self.cancel_retained_readoption();
        if let ViewerShift::Book(SpreadShiftState::Adjusted { persistence, .. }) = &mut self.state {
            *persistence = ShiftPersistence::SessionOnly;
        }
    }

    pub(crate) fn resolve(
        &mut self,
        index: &Arc<file_type_resume::PreparedPageIdentityIndex>,
        nav: &Arc<Vec<usize>>,
    ) -> bool {
        if self.availability != PairingEditAvailability::Automatic {
            return false;
        }
        if self
            .projection
            .as_ref()
            .is_some_and(|(old_index, old_nav)| {
                Arc::ptr_eq(old_index, index) && Arc::ptr_eq(old_nav, nav)
            })
        {
            return false;
        }
        self.projection = Some((Arc::clone(index), Arc::clone(nav)));
        let old = self.anchor_idx();
        if let (
            Some(scope),
            ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref,
                resolution,
                ..
            }),
        ) = (&self.scope, &mut self.state)
        {
            let resolved = anchor_ref
                .to_identity(scope)
                .ok()
                .and_then(|identity| index.resolve_shift_identity(&identity))
                .filter(|slot| nav.contains(slot));
            *resolution = resolved.map_or(ShiftResolution::NotInView, ShiftResolution::Resolved);
        }
        old != self.anchor_idx()
    }

    pub(crate) fn remap(&mut self, removed: &[usize]) {
        let remap = |idx: usize| match removed.binary_search(&idx) {
            Ok(_) => None,
            Err(shift) => Some(idx - shift),
        };
        self.remap_with(&remap);
    }

    pub(crate) fn remap_with(&mut self, remap: &impl Fn(usize) -> Option<usize>) {
        if self.availability != PairingEditAvailability::Automatic {
            return;
        }
        match &mut self.state {
            ViewerShift::Ordinary(idx) => {
                self.state = remap(*idx).map_or(ViewerShift::None, ViewerShift::Ordinary);
            }
            ViewerShift::Book(SpreadShiftState::Adjusted { resolution, .. }) => {
                if let ShiftResolution::Resolved(idx) = resolution {
                    *resolution =
                        remap(*idx).map_or(ShiftResolution::NotInView, ShiftResolution::Resolved);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn accept_write_reply(
        &mut self,
        reply: &ShiftWriteReply,
        memory: bool,
    ) -> Option<bool> {
        if self.availability != PairingEditAvailability::Automatic {
            return None;
        }
        let ShiftCommand::Pending {
            sequence,
            operation,
            clear,
        } = &self.command
        else {
            return None;
        };
        if *sequence != reply.sequence
            || operation != &reply.operation
            || self.scope.as_ref() != Some(&reply.scope)
        {
            return None;
        }
        let clear = *clear;
        if let ViewerShift::Book(SpreadShiftState::Adjusted { persistence, .. }) = &mut self.state {
            *persistence = if !memory {
                ShiftPersistence::SessionOnly
            } else if reply.status.is_ok() {
                ShiftPersistence::Saved(reply.sequence)
            } else {
                ShiftPersistence::SaveFailed
            };
        }
        self.command = ShiftCommand::Idle;
        Some(clear)
    }
}

pub(super) struct ViewerBookRestore {
    receipt: RestoreReceipt,
    reads: RestoreReadDependencies,
    resume_key: Option<String>,
    memory: ShiftMemoryPolicy,
    cause: ShiftRestoreCause,
    layout_eligibility: PairingEditAvailability,
}

impl ViewerBookRestore {
    #[cfg(test)]
    pub(super) fn open_id_for_test(&self) -> u64 {
        self.receipt.open_id
    }

    pub(super) fn refresh_memory_policy(
        &mut self,
        generation: u64,
        memory: bool,
        service: Option<&SpreadShiftService>,
        availability: PairingEditAvailability,
    ) {
        self.receipt.settings_generation = generation;
        self.receipt.shift_cutoff = service.map_or(0, |service| service.watermarks().admitted);
        self.memory = if memory {
            ShiftMemoryPolicy::Remember
        } else {
            ShiftMemoryPolicy::SessionOnly
        };
        self.layout_eligibility = availability;
        self.reads.refresh_shift_dependency(
            self.receipt.clone(),
            (memory && availability == PairingEditAvailability::Automatic)
                .then_some(service)
                .flatten(),
        );
    }
}

impl App {
    pub(crate) fn begin_retained_shift_readoption(&mut self, ctx: &egui::Context) {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic
            || !self.settings.remember_spread_shift_per_book
        {
            return;
        }
        self.spread_shift.cancel_retained_readoption();
        let id = self
            .frame_counter
            .wrapping_add(self.input_seq)
            .wrapping_add(1);
        let Some(mut restore) = self.capture_book_shift_restore(id, true) else {
            return;
        };
        match self.poll_book_shift_restore(&mut restore) {
            Ok(true) => return,
            Ok(false) => {
                self.spread_shift.retained = RetainedReadoption::Reading {
                    restore: Box::new(restore),
                    started_at: std::time::Instant::now(),
                }
            }
            Err(error) => {
                crate::logger::log(format!("retained spread shift restore failed: {error}"));
                self.show_feedback_warning(format!("見開き調整を復元できませんでした: {error}"));
                return;
            }
        }
        self.consume_input_during_sidecar_restore(ctx);
        ctx.request_repaint();
    }

    pub(crate) fn poll_retained_shift_readoption(&mut self, ctx: &egui::Context) {
        let RetainedReadoption::Reading {
            mut restore,
            started_at,
        } = std::mem::take(&mut self.spread_shift.retained)
        else {
            return;
        };
        match self.poll_book_shift_restore(&mut restore) {
            Ok(true) => ctx.request_repaint(),
            Ok(false) => {
                self.spread_shift.retained = RetainedReadoption::Reading {
                    restore,
                    started_at,
                };
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            Err(error) => {
                crate::logger::log(format!("retained spread shift restore failed: {error}"));
                self.show_feedback_warning(format!("見開き調整を復元できませんでした: {error}"));
            }
        }
    }

    pub(super) fn admit_imported_shift_read(
        &self,
        changed: bool,
    ) -> Option<(BookScopeKey, std::sync::mpsc::Receiver<ShiftReadReply>)> {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic
            || !changed
            || !self.settings.remember_spread_shift_per_book
        {
            return None;
        }
        let scope = self.current_spread_shift_scope()?;
        let service = self.spread_shift_service.as_ref()?;
        let read = service.read_after(scope.clone(), service.watermarks().admitted);
        Some((scope, read))
    }

    pub(super) fn adopt_imported_shift_read(&mut self, scope: BookScopeKey, read: ShiftReadReply) {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic
            || !self.settings.remember_spread_shift_per_book
            || self.current_spread_shift_scope().as_ref() != Some(&scope)
        {
            return;
        }
        let receipt = RestoreReceipt {
            open_id: self
                .frame_counter
                .wrapping_add(self.input_seq)
                .wrapping_add(1),
            context_id: self.spread_shift_context_id(),
            scope: scope.clone(),
            items_generation: self.items_generation,
            settings_generation: self.spread_shift_settings_generation,
            shift_cutoff: read.cutoff,
            resume_cutoff: 0,
        };
        let epoch = self
            .spread_shift_service
            .as_ref()
            .map_or(0, |service| service.maintenance_epoch_for(&scope));
        let state = match self.resolve_captured_automatic_shift(
            receipt,
            ShiftRestoreCause::RetainedReadoption,
            ShiftMemoryPolicy::Remember,
            Some(read),
            epoch,
        ) {
            Ok(state) => state,
            Err(error) => {
                crate::logger::log(format!("imported spread shift restore rejected: {error}"));
                return;
            }
        };
        self.spread_shift.scope = Some(scope);
        self.spread_shift.availability = PairingEditAvailability::Automatic;
        self.spread_shift.projection = None;
        self.spread_shift.command = ShiftCommand::Idle;
        self.spread_shift.state = if state == SpreadShiftState::None {
            ViewerShift::None
        } else {
            ViewerShift::Book(state)
        };
        self.invalidate_fullscreen_display_unit_owners();
        self.adopt_shift_layout_without_navigation();
    }

    pub(crate) fn spread_shift_anchor_idx(&self) -> Option<usize> {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic {
            return None;
        }
        self.spread_shift.anchor_idx()
    }

    pub(crate) fn current_spread_shift_scope(&self) -> Option<BookScopeKey> {
        if self.extracted_result_surface().is_some()
            || self.book_bookmark_view_is_synthetic()
            || !self.page_order_locked_for_current_view()
            || self.snapshot.is_some()
        {
            return None;
        }
        let root = self.effective_folder()?;
        if let Some(nav) = &self.zip_nav {
            BookScopeKey::archive(root, nav.current()).ok()
        } else if self
            .items
            .first()
            .is_some_and(|item| matches!(item, GridItem::PdfPage { .. }))
        {
            BookScopeKey::document(root).ok()
        } else {
            BookScopeKey::folder(root).ok()
        }
    }

    pub(crate) fn spread_shift_context_id(&self) -> ViewerContextId {
        #[cfg(windows)]
        {
            self.projected_viewer_context_id()
        }
        #[cfg(not(windows))]
        {
            ViewerContextId::single_context()
        }
    }

    pub(super) fn begin_book_shift_restore(&mut self, open_id: u64) -> Option<ViewerBookRestore> {
        self.capture_book_shift_restore(open_id, false)
    }

    /// A completed composition removal creates exactly one new Automatic readoption.
    /// OFF ignores dormant storage; ON resolves its identity using E's current rules.
    /// This transition carries no resume read or write and does not count as reading.
    pub(super) fn begin_composition_removed_shift_restore(
        &mut self,
        open_id: u64,
    ) -> Option<ViewerBookRestore> {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic {
            return None;
        }
        self.capture_shift_restore_for_cause(open_id, ShiftRestoreCause::CompositionRemoved)
    }

    pub(super) fn start_composition_removed_shift_restore(&mut self, open_id: u64) {
        let Some(mut restore) = self.begin_composition_removed_shift_restore(open_id) else {
            return;
        };
        self.spread_shift.cancel_retained_readoption();
        match self.poll_book_shift_restore(&mut restore) {
            Ok(true) => {}
            Ok(false) => {
                self.spread_shift.retained = RetainedReadoption::Reading {
                    restore: Box::new(restore),
                    started_at: std::time::Instant::now(),
                };
            }
            Err(error) => {
                crate::logger::log(format!("composition removed shift restore failed: {error}"));
                self.show_feedback_warning(format!("見開き調整を復元できませんでした: {error}"));
            }
        }
    }

    pub(super) fn capture_book_shift_restore(
        &mut self,
        open_id: u64,
        retained: bool,
    ) -> Option<ViewerBookRestore> {
        self.capture_shift_restore_for_cause(
            open_id,
            if retained {
                ShiftRestoreCause::RetainedReadoption
            } else {
                ShiftRestoreCause::FreshOpen
            },
        )
    }

    fn capture_shift_restore_for_cause(
        &mut self,
        open_id: u64,
        cause: ShiftRestoreCause,
    ) -> Option<ViewerBookRestore> {
        let availability = self.pairing_edit_availability();
        let retained = cause != ShiftRestoreCause::FreshOpen;
        if retained && availability != PairingEditAvailability::Automatic {
            return None;
        }
        let scope = self.current_spread_shift_scope();
        if !retained && availability == PairingEditAvailability::Automatic {
            self.spread_shift = ViewerSpreadShift {
                scope: scope.clone(),
                ..Default::default()
            };
        }
        self.spread_shift.availability = availability;
        let scope = scope?;
        let receipt = RestoreReceipt {
            open_id,
            context_id: self.spread_shift_context_id(),
            scope,
            items_generation: self.items_generation,
            settings_generation: self.spread_shift_settings_generation,
            shift_cutoff: self
                .spread_shift_service
                .as_ref()
                .map_or(0, |s| s.watermarks().admitted),
            resume_cutoff: self
                .book_resume_writer
                .as_ref()
                .map_or(0, |w| w.watermarks().admitted),
        };
        // Use the same root qualification and physical key as the resume producer,
        // including collapsed wrappers and converted archives.
        let resume_key = (!retained)
            .then(|| {
                self.book_resume_recording_folder()
                    .map(crate::path_key::normalize)
            })
            .flatten();
        let memory = self.settings.remember_spread_shift_per_book;
        let reads = RestoreReadDependencies::admit(
            receipt.clone(),
            (memory && availability == PairingEditAvailability::Automatic)
                .then_some(self.spread_shift_service.as_ref())
                .flatten(),
            (!retained)
                .then_some(self.book_resume_writer.as_ref())
                .flatten(),
            resume_key.as_deref(),
        );
        Some(ViewerBookRestore {
            receipt,
            reads,
            resume_key,
            memory: if memory {
                ShiftMemoryPolicy::Remember
            } else {
                ShiftMemoryPolicy::SessionOnly
            },
            cause,
            layout_eligibility: availability,
        })
    }

    pub(super) fn poll_book_shift_restore(
        &mut self,
        restore: &mut ViewerBookRestore,
    ) -> Result<bool, String> {
        let availability = self.pairing_edit_availability();
        if restore.layout_eligibility != availability {
            restore.layout_eligibility = availability;
            restore.reads.refresh_shift_dependency(
                restore.receipt.clone(),
                (restore.memory == ShiftMemoryPolicy::Remember
                    && availability == PairingEditAvailability::Automatic)
                    .then_some(self.spread_shift_service.as_ref())
                    .flatten(),
            );
        }
        let mut current = restore.receipt.clone();
        current.context_id = self.spread_shift_context_id();
        current.items_generation = self.items_generation;
        current.settings_generation = self.spread_shift_settings_generation;
        let root = self
            .effective_folder()
            .ok_or("book scope changed during restore")?;
        current.scope = if let Some(nav) = &self.zip_nav {
            BookScopeKey::archive(root, nav.current())?
        } else {
            BookScopeKey::new(root, "")?
        };
        let epoch = self
            .spread_shift_service
            .as_ref()
            .map_or(0, |s| s.maintenance_epoch_for(&current.scope));
        // Classification owns shift-read admission. Keep this continuation (and
        // its independently admitted resume fence) alive until it is classified.
        if matches!(
            self.book_composition,
            book_composition_runtime::ViewerBookComposition::Preparing { .. }
        ) {
            if current != restore.receipt {
                return Err("book restore receipt changed during classification".into());
            }
            return Ok(false);
        }
        let Some(reads) = restore.reads.try_complete(&current, epoch)? else {
            return Ok(false);
        };
        // Fresh capture already seeded an empty session. Retained capture kept
        // its accepted session. Only a current read may replace either one;
        // an expired recipe is never evidence that the saved row is absent.
        if availability == PairingEditAvailability::Automatic
            && (restore.memory == ShiftMemoryPolicy::SessionOnly
                || matches!(reads.shift, CompletedShiftRead::Captured(_)))
        {
            let read = match reads.shift {
                CompletedShiftRead::Captured(read) => Some(read),
                CompletedShiftRead::Unrequested | CompletedShiftRead::Expired => None,
            };
            let state = self.resolve_captured_automatic_shift(
                current.clone(),
                restore.cause,
                restore.memory,
                read,
                epoch,
            )?;
            self.spread_shift.scope = Some(current.scope.clone());
            self.spread_shift.availability = availability;
            self.spread_shift.projection = None;
            self.spread_shift.command = ShiftCommand::Idle;
            self.spread_shift.state = if state == SpreadShiftState::None {
                ViewerShift::None
            } else {
                ViewerShift::Book(state)
            };
            self.invalidate_fullscreen_display_unit_owners();
            if restore.cause != ShiftRestoreCause::FreshOpen {
                self.adopt_shift_layout_without_navigation();
            }
        }
        if self.book_resume_writer.is_some()
            && let Some(key) = &restore.resume_key
        {
            self.spread_shift.resume = FencedResume::Ready {
                key: key.clone(),
                position: reads.resume,
            };
        }
        Ok(true)
    }

    /// Delegate policy, cause and saved/retained state to E. Only the actual saved
    /// anchor needs an identity/member proof: E never resolves any other page in
    /// this call. Narrowing the proof avoids an O(book-size) UI identity-map clone.
    fn resolve_captured_automatic_shift(
        &mut self,
        receipt: RestoreReceipt,
        cause: ShiftRestoreCause,
        memory: ShiftMemoryPolicy,
        read: Option<ShiftReadReply>,
        maintenance_epoch: u64,
    ) -> Result<SpreadShiftState, String> {
        let retained_state = match &self.spread_shift.state {
            ViewerShift::Book(state) => state.clone(),
            _ => SpreadShiftState::None,
        };
        let anchor = if memory == ShiftMemoryPolicy::Remember {
            read.as_ref().and_then(|read| read.row.as_ref())
        } else if cause == ShiftRestoreCause::RetainedReadoption {
            match &retained_state {
                SpreadShiftState::Adjusted { anchor_ref, .. } => Some(anchor_ref),
                _ => None,
            }
        } else {
            None
        };
        let identity_and_slot = anchor.and_then(|reference| {
            let identity = reference.to_identity(&receipt.scope).ok()?;
            let slot = self.page_identity_index.resolve_shift_identity(&identity)?;
            Some((identity, slot))
        });
        let identity_index =
            ShiftIdentityIndex::prepare(&receipt.scope, identity_and_slot.clone())?;
        let nav = self.get_nav_indices();
        let anchor_member = identity_and_slot
            .as_ref()
            .map(|(_, slot)| *slot)
            .filter(|slot| nav.contains(slot));
        let request = ResolveShiftRequest {
            scope: receipt.scope.clone(),
            manifest: ShiftManifest {
                generation: receipt.items_generation,
            },
            nav_order: ShiftNavOrder {
                // This synchronous adoption consumes the accepted generation now;
                // no delayed G reply or second navigation owner is introduced.
                generation: self.items_generation,
                indices: anchor_member.into_iter().collect::<Vec<_>>().into(),
            },
            identity_index,
            rules: self.accepted_spread_shift_rules(),
            cause,
            memory_policy: memory,
            layout_eligibility: self.pairing_edit_availability(),
            receipt,
        };
        let BookLayoutSource::Automatic { shift, .. } = resolve_book_layout_source::<()>(
            PreparedLayoutRecipe::Automatic {
                shift_read: read,
                retained_state,
                maintenance_epoch,
            },
            request,
        )?
        else {
            return Err("automatic shift resolver returned another recipe".into());
        };
        Ok(shift.state)
    }

    fn next_shift_operation(&mut self) -> ShiftOperation {
        self.spread_shift.operation_sequence = self.spread_shift.operation_sequence.wrapping_add(1);
        ShiftOperation {
            context_id: self.spread_shift_context_id(),
            items_generation: self.items_generation,
            operation_sequence: self.spread_shift.operation_sequence,
        }
    }

    fn persist_live_shift(&mut self, clear: bool) {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic {
            return;
        }
        let Some(scope) = self.spread_shift.scope.clone() else {
            return;
        };
        let operation = self.next_shift_operation();
        let result = self
            .spread_shift_service
            .as_ref()
            .ok_or_else(|| "保存サービスがありません".to_owned())
            .and_then(|service| {
                if clear {
                    service.clear_shift(scope, operation.clone())
                } else {
                    let ViewerShift::Book(SpreadShiftState::Adjusted { anchor_ref, .. }) =
                        &self.spread_shift.state
                    else {
                        return Err("調整がありません".into());
                    };
                    service.set_shift(scope, anchor_ref.clone(), operation.clone())
                }
            });
        match result {
            Ok(sequence) => {
                self.spread_shift.command = ShiftCommand::Pending {
                    sequence,
                    operation,
                    clear,
                };
                if let ViewerShift::Book(SpreadShiftState::Adjusted { persistence, .. }) =
                    &mut self.spread_shift.state
                {
                    *persistence = ShiftPersistence::Saving(sequence);
                }
            }
            Err(error) => {
                if let ViewerShift::Book(SpreadShiftState::Adjusted { persistence, .. }) =
                    &mut self.spread_shift.state
                {
                    *persistence = ShiftPersistence::SaveFailed;
                }
                crate::logger::log(format!("spread shift admission failed: {error}"));
                self.show_feedback_warning(format!("見開き調整を保存できませんでした: {error}"));
            }
        }
    }

    pub(crate) fn set_live_spread_shift(&mut self, anchor_idx: usize) -> bool {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic
            || self.spread_shift.retained_readoption_pending()
        {
            return false;
        }
        // Resolve the accepted target before changing the live draft. Hydration
        // and readoption use their separate read-only owners above.
        let scope = self.current_spread_shift_scope();
        let reference = if let Some(scope) = &scope {
            let reference = self
                .items
                .get(anchor_idx)
                .and_then(|item| {
                    crate::book_resume_db::PageIdentity::for_item(item, Some(&scope.logical_root))
                })
                .and_then(|identity| SpreadShiftPageRef::from_identity(scope, &identity).ok());
            let Some(reference) = reference else {
                self.show_feedback_warning("見開き調整の基点を保存できませんでした".into());
                return false;
            };
            Some(reference)
        } else {
            None
        };
        if !self.admit_book_append_followup_mutation() {
            return false;
        }
        self.spread_shift.projection = None;
        self.spread_shift.scope = scope;
        if let Some(anchor_ref) = reference {
            self.spread_shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref,
                resolution: ShiftResolution::Resolved(anchor_idx),
                persistence: ShiftPersistence::SessionOnly,
            });
            if self.settings.remember_spread_shift_per_book {
                self.persist_live_shift(false);
            } else {
                self.show_feedback_toast("見開きを1ページずらしました（この閲覧中のみ）".into());
            }
        } else {
            self.spread_shift.state = ViewerShift::Ordinary(anchor_idx);
            self.show_feedback_toast("見開きを1ページずらしました（この閲覧中のみ）".into());
        }
        self.invalidate_fullscreen_display_unit_owners();
        true
    }

    /// Explicit reset after the existing synchronous base setter succeeds (Q3).
    pub(crate) fn reset_live_spread_shift(&mut self, explicit_clear: bool) -> bool {
        if self.pairing_edit_availability() != PairingEditAvailability::Automatic {
            return false;
        }
        let had_shift = !matches!(self.spread_shift.state, ViewerShift::None);
        let has_saved = self.spread_shift.scope.as_ref().is_some_and(|scope| {
            self.spread_shift_service
                .as_ref()
                .and_then(|service| service.observed_row(scope))
                .is_some()
        });
        let persists_clear = self.spread_shift.scope.is_some()
            && (had_shift || has_saved)
            && (explicit_clear || self.settings.remember_spread_shift_per_book);
        if (had_shift || persists_clear) && !self.admit_book_append_followup_mutation() {
            return false;
        }
        self.spread_shift.state = ViewerShift::None;
        self.spread_shift.projection = None;
        if persists_clear {
            self.persist_live_shift(true);
        } else if had_shift {
            self.show_feedback_toast("見開き調整を解除しました".into());
        }
        self.invalidate_fullscreen_display_unit_owners();
        had_shift
    }

    pub(crate) fn spread_shift_memory_setting_changed(&mut self, enabled: bool) {
        if self.settings.remember_spread_shift_per_book == enabled {
            return;
        }
        let availability = self.pairing_edit_availability();
        if !self.admit_book_append_followup_mutation() {
            return;
        }
        self.spread_shift_settings_generation =
            self.spread_shift_settings_generation.wrapping_add(1);
        self.settings.remember_spread_shift_per_book = enabled;
        if availability == PairingEditAvailability::Automatic {
            self.spread_shift.session_only();
        }
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.saved_viewing_state.shift.session_only();
        }
        #[cfg(windows)]
        self.make_at_rest_spread_shifts_session_only();
        if enabled
            && availability == PairingEditAvailability::Automatic
            && matches!(
                self.spread_shift.state,
                ViewerShift::Book(SpreadShiftState::Adjusted { .. })
            )
        {
            self.persist_live_shift(false);
        }
        self.refresh_pending_book_shift_memory_policy();
    }

    /// HUD memory changes are pairing actions. The global preferences hook above
    /// remains available without changing this composed book's dormant adjustment.
    pub(crate) fn set_spread_shift_memory_from_pairing(&mut self, enabled: bool) -> bool {
        let availability = self.pairing_edit_availability();
        let rules = self.accepted_spread_shift_rules();
        if let PairingAdmission::Rejected(reason) = admit_pairing_action(
            PairingContextToken {
                rules: &rules,
                availability,
            },
            PairingAction::SetMemoryPolicy,
        ) {
            self.show_feedback_warning(reason.into());
            return false;
        }
        self.spread_shift_memory_setting_changed(enabled);
        true
    }

    pub(super) fn fenced_book_resume(
        &self,
        folder: &Path,
    ) -> Option<Option<crate::book_resume_db::ReadingPosition>> {
        match &self.spread_shift.resume {
            FencedResume::Ready { key, position } if *key == crate::path_key::normalize(folder) => {
                Some(position.clone())
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_spread_shift_anchor_for_test(&mut self, anchor: Option<usize>) {
        self.spread_shift.projection = None;
        self.spread_shift.state = anchor.map_or(ViewerShift::None, ViewerShift::Ordinary);
    }
}

#[cfg(test)]
mod composition_tests {
    include!("spread_shift_composition_tests.rs");
}
