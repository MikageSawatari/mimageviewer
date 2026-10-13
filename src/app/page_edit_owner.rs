//! Context-owned editing documents. Indexed maps are rendering projections;
//! asynchronous writes and history carry captured page identity instead.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageEditOwner {
    pub context: ViewerContextId,
    pub target: PageAdjustmentTarget,
    pub items_generation: u64,
    /// All resident projections of this key in `items_generation`.
    pub slots: Vec<usize>,
}

#[derive(Default)]
pub(crate) enum LocalPageEditDocument {
    #[default]
    Idle,
    Selection {
        owner: PageEditOwner,
        selected_layer: usize,
        revision: u64,
    },
    Editing {
        owner: PageEditOwner,
        layers: local_adjust_core::LocalAdjustmentLayers,
        revision: u64,
        selected_layer: usize,
    },
}

pub(super) enum PageEditCommit {
    Local {
        origin: LocalAdjustWriteOrigin,
        before: local_adjust_core::LocalAdjustmentLayers,
        after: local_adjust_core::LocalAdjustmentLayers,
        summary: String,
    },
    Page(AdjustmentDragSession),
    Standard(AdjustmentStandardDragSession),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PageEditBoundary {
    History,
    Exit,
}

impl App {
    pub(crate) fn reproject_page_edit_owners(&mut self) {
        self.settle_local_page_edit_document();
        self.reproject_local_page_edit_pivot();
        let mut positions = std::collections::HashMap::<String, Vec<usize>>::new();
        let mut remember = |owner: &PageEditOwner| {
            positions.entry(owner.target.page_key.clone()).or_default();
        };
        match &self.local_page_edit_document {
            LocalPageEditDocument::Selection { owner, .. }
            | LocalPageEditDocument::Editing { owner, .. } => remember(owner),
            LocalPageEditDocument::Idle => {}
        }
        if let Some(session) = &self.adjustment_drag_session {
            remember(&session.owner);
        }
        if let Some(session) = &self.local_adjust_shape_key_edit {
            remember(&session.owner);
        }
        if let Some(session) = &self.local_adjust_shape_drag {
            remember(&session.owner);
        }
        if let Some(session) = &self.local_adjust_canvas_drag {
            remember(&session.owner);
        }
        if let Some(session) = &self.local_adjust_mask_brush_stroke {
            remember(&session.owner);
        }
        if let Some(pending) = &self.local_adjust_lut_pending {
            remember(&pending.owner);
        }
        if let Some(pending) = &self.local_adjust_segmentation_pending {
            remember(&pending.owner);
        }
        // Only the few keys owned by current interactions retain position
        // vectors. Do one order pass at adoption, not one list scan per input.
        if !positions.is_empty() {
            if let Some(snapshot) = &self.page_edit_snapshot {
                for idx in 0..snapshot.order.len() {
                    if let Some(key) = snapshot.order.key_at(idx)
                        && let Some(slots) = positions.get_mut(key)
                    {
                        slots.push(idx);
                    }
                }
            } else {
                for idx in 0..self.items.len() {
                    if let Some(key) = self.page_path_key(idx)
                        && let Some(slots) = positions.get_mut(&key)
                    {
                        slots.push(idx);
                    }
                }
            }
        }
        let reader = self.fullscreen_idx;
        let locate = |owner: &mut PageEditOwner| {
            owner.items_generation = self.items_generation;
            owner.slots = positions
                .get(&owner.target.page_key)
                .cloned()
                .unwrap_or_default();
            reader
                .filter(|idx| owner.slots.contains(idx))
                .or_else(|| owner.slots.first().copied())
                .unwrap_or(usize::MAX)
        };
        match &mut self.local_page_edit_document {
            LocalPageEditDocument::Selection { owner, .. }
            | LocalPageEditDocument::Editing { owner, .. } => {
                locate(owner);
            }
            LocalPageEditDocument::Idle => {}
        }
        if let Some(session) = self.adjustment_drag_session.as_mut() {
            session.fs_idx = locate(&mut session.owner);
        }
        if let Some(session) = self.local_adjust_shape_key_edit.as_mut() {
            session.fs_idx = locate(&mut session.owner);
        }
        if let Some(session) = self.local_adjust_shape_drag.as_mut() {
            session.fs_idx = locate(&mut session.owner);
        }
        if let Some(session) = self.local_adjust_canvas_drag.as_mut() {
            session.fs_idx = locate(&mut session.owner);
        }
        if let Some(session) = self.local_adjust_mask_brush_stroke.as_mut() {
            session.fs_idx = locate(&mut session.owner);
        }
        if let Some(pending) = self.local_adjust_lut_pending.as_mut() {
            pending.fs_idx = locate(&mut pending.owner);
        }
        if let Some(pending) = self.local_adjust_segmentation_pending.as_mut() {
            pending.fs_idx = locate(&mut pending.owner);
        }
        self.project_adjustment_page_edit_document();
        self.project_local_page_edit_document(0);
        self.refresh_pending_page_edit_projections();
    }

    pub(crate) fn align_page_edit_slots_with_exact_reader(&mut self) {
        let Some(reader) = self.fullscreen_idx else {
            return;
        };
        macro_rules! align {
            ($session:expr) => {
                if let Some(session) = $session.as_mut()
                    && session.owner.items_generation == self.items_generation
                    && session.owner.slots.contains(&reader)
                {
                    session.fs_idx = reader;
                }
            };
        }
        align!(self.adjustment_drag_session);
        align!(self.local_adjust_shape_key_edit);
        align!(self.local_adjust_shape_drag);
        align!(self.local_adjust_canvas_drag);
        align!(self.local_adjust_mask_brush_stroke);
    }

    pub(crate) fn page_edit_owner_is_current(&self, owner: &PageEditOwner) -> bool {
        owner.context == self.projected_viewer_context_id()
    }

    pub(crate) fn capture_page_edit_owner(&self, idx: usize) -> Option<PageEditOwner> {
        let reusable = |owner: &PageEditOwner| {
            owner.context == self.projected_viewer_context_id()
                && owner.items_generation == self.items_generation
                && owner.slots.contains(&idx)
                && self.page_path_key(idx).as_deref() == Some(owner.target.page_key.as_str())
        };
        match &self.local_page_edit_document {
            LocalPageEditDocument::Selection { owner, .. }
            | LocalPageEditDocument::Editing { owner, .. }
                if reusable(owner) =>
            {
                return Some(owner.clone());
            }
            _ => {}
        }
        if let Some(session) = &self.adjustment_drag_session
            && reusable(&session.owner)
        {
            return Some(session.owner.clone());
        }
        let mut target = self.page_adjustment_target_for_idx(idx)?;
        target.idx_hint = PageIndexHint::Unresolved;
        let slots = if let Some(snapshot) = &self.page_edit_snapshot {
            (0..snapshot.order.len())
                .filter(|slot| snapshot.order.key_at(*slot) == Some(target.page_key.as_str()))
                .collect()
        } else {
            (0..self.items.len())
                .filter(|slot| {
                    self.page_path_key(*slot).as_deref() == Some(target.page_key.as_str())
                })
                .collect()
        };
        Some(PageEditOwner {
            context: self.projected_viewer_context_id(),
            target,
            items_generation: self.items_generation,
            slots,
        })
    }

    /// Selection is reading/editor state; it does not create a history boundary.
    pub(crate) fn select_local_page_edit_layer(&mut self, idx: usize, selected_layer: usize) {
        let Some(owner) = self.capture_page_edit_owner(idx) else {
            return;
        };
        let selected_layer = self
            .local_adjust_page_layers
            .get(&idx)
            .filter(|layers| !layers.is_empty())
            .map(|layers| selected_layer.min(layers.len() - 1))
            .unwrap_or(selected_layer);
        let gesture_active = self.local_page_edit_gesture_active();
        match &mut self.local_page_edit_document {
            LocalPageEditDocument::Selection {
                owner: existing,
                selected_layer: selected,
                ..
            }
            | LocalPageEditDocument::Editing {
                owner: existing,
                selected_layer: selected,
                ..
            } if existing.target.page_key == owner.target.page_key => {
                *existing = owner.clone();
                *selected = selected_layer;
            }
            _ if !gesture_active => {
                self.local_page_edit_document = LocalPageEditDocument::Selection {
                    owner: owner.clone(),
                    selected_layer,
                    revision: 0,
                };
            }
            _ => {}
        }
        for slot in owner.slots {
            self.local_adjust_selected_layers
                .insert(slot, selected_layer);
        }
    }

    /// A finished document must not overwrite a later authoritative projection.
    /// Retain only the key-owned layer choice until a new edit starts.
    pub(crate) fn settle_local_page_edit_document(&mut self) {
        if self.local_page_edit_gesture_active() {
            return;
        }
        if let LocalPageEditDocument::Editing {
            owner,
            selected_layer,
            revision,
            ..
        } = &self.local_page_edit_document
        {
            self.local_page_edit_document = LocalPageEditDocument::Selection {
                owner: owner.clone(),
                selected_layer: *selected_layer,
                revision: *revision,
            };
        }
    }

    fn local_page_edit_gesture_active(&self) -> bool {
        self.local_adjust_shape_key_edit.is_some()
            || self.local_adjust_shape_drag.is_some()
            || self.local_adjust_canvas_drag.is_some()
            || self.local_adjust_mask_brush_stroke.is_some()
            || self.local_adjust_mask_shape_drag_start.is_some()
            || !self.local_adjust_mask_lasso_points.is_empty()
    }

    pub(crate) fn begin_local_page_edit(&mut self, idx: usize) -> bool {
        let Some(owner) = self.capture_page_edit_owner(idx) else {
            return false;
        };
        // Resuming an unfinished pointer gesture is new input too. Register
        // completed sibling edits before it can produce another completed value,
        // while retaining this context's legal pointer/key continuation.
        let commits = self.take_parked_page_edit_commits();
        self.deliver_page_edit_commits(commits);
        if let LocalPageEditDocument::Editing {
            owner: existing, ..
        } = &self.local_page_edit_document
            && existing.target.page_key == owner.target.page_key
            && existing.context == owner.context
            && (self.local_adjust_shape_key_edit.is_some()
                || self.local_adjust_shape_drag.is_some()
                || self.local_adjust_canvas_drag.is_some()
                || self.local_adjust_mask_brush_stroke.is_some())
        {
            return true;
        }
        // A continuation within a page retains the existing pointer/key
        // interleaving. A genuinely new input owner closes older history first.
        self.prepare_page_edit_history_boundary();
        let selected_layer = match &self.local_page_edit_document {
            LocalPageEditDocument::Selection {
                owner: selected_owner,
                selected_layer,
                ..
            }
            | LocalPageEditDocument::Editing {
                owner: selected_owner,
                selected_layer,
                ..
            } if selected_owner.target.page_key == owner.target.page_key => *selected_layer,
            _ => self
                .local_adjust_selected_layers
                .get(&idx)
                .copied()
                .unwrap_or(0),
        };
        let layers = self
            .local_adjust_write_pending
            .get(&owner.target.page_key)
            .map(|write| write.layers.clone())
            .or_else(|| self.local_adjust_page_layers.get(&idx).cloned())
            .unwrap_or_default();
        let mut revision = self
            .local_page_edit_revision_for_target(&owner.target)
            .unwrap_or(0);
        if let Some(write) = self.local_adjust_write_pending.get(&owner.target.page_key)
            && write.origin.context == owner.context
        {
            revision = revision.max(write.origin.revision);
        }
        self.local_page_edit_document = LocalPageEditDocument::Editing {
            owner,
            layers,
            revision,
            selected_layer,
        };
        self.project_local_page_edit_document(idx);
        true
    }

    pub(crate) fn local_page_edit_layers_mut(
        &mut self,
        idx: usize,
    ) -> Option<&mut local_adjust_core::LocalAdjustmentLayers> {
        let slots = match &self.local_page_edit_document {
            LocalPageEditDocument::Editing { owner, .. }
                if owner.context == self.projected_viewer_context_id()
                    && owner.items_generation == self.items_generation
                    && owner.slots.contains(&idx) =>
            {
                owner.slots.clone()
            }
            _ => return None,
        };
        // Rendering aliases must not force a full raster COW on every input
        // frame. The document is reprojected after this mutable borrow ends.
        for i in slots {
            self.local_adjust_page_layers.remove(&i);
        }
        match &mut self.local_page_edit_document {
            LocalPageEditDocument::Editing { layers, .. } => Some(layers),
            _ => None,
        }
    }

    pub(crate) fn note_local_page_edit_document_changed(&mut self) {
        if let LocalPageEditDocument::Editing { revision, .. } = &mut self.local_page_edit_document
        {
            *revision = revision.wrapping_add(1);
        }
    }

    pub(crate) fn local_page_edit_layers_for_target(
        &self,
        target: &PageAdjustmentTarget,
    ) -> local_adjust_core::LocalAdjustmentLayers {
        if let LocalPageEditDocument::Editing { owner, layers, .. } = &self.local_page_edit_document
            && owner.target.page_key == target.page_key
        {
            return layers.clone();
        }
        self.local_adjust_write_pending
            .get(&target.page_key)
            .map(|write| write.layers.clone())
            .or_else(|| {
                self.local_page_edit_slots_for_target(target)
                    .into_iter()
                    .find_map(|idx| self.local_adjust_page_layers.get(&idx).cloned())
            })
            .unwrap_or_default()
    }

    pub(crate) fn local_page_edit_document_for_producer(
        &self,
        owner: &PageEditOwner,
        claim: &local_adjust_core::LocalAdjustmentLayers,
    ) -> local_adjust_core::LocalAdjustmentLayers {
        if let LocalPageEditDocument::Editing {
            owner: draft_owner,
            layers,
            ..
        } = &self.local_page_edit_document
            && draft_owner.target.page_key == owner.target.page_key
        {
            return layers.clone();
        }
        if let Some(write) = self.local_adjust_write_pending.get(&owner.target.page_key) {
            return write.layers.clone();
        }
        owner
            .slots
            .iter()
            .find_map(|idx| self.local_adjust_page_layers.get(idx).cloned())
            .unwrap_or_else(|| claim.clone())
    }

    pub(crate) fn local_page_edit_revision_for_target(
        &self,
        target: &PageAdjustmentTarget,
    ) -> Option<u64> {
        match &self.local_page_edit_document {
            LocalPageEditDocument::Selection {
                owner, revision, ..
            }
            | LocalPageEditDocument::Editing {
                owner, revision, ..
            } if owner.context == self.projected_viewer_context_id()
                && owner.target.page_key == target.page_key =>
            {
                Some(*revision)
            }
            _ => None,
        }
    }

    pub(crate) fn project_local_page_edit_document(&mut self, _idx: usize) {
        self.project_local_page_edit_document_inner(true);
    }

    pub(crate) fn restore_local_page_edit_document_projection(&mut self) {
        self.project_local_page_edit_document_inner(false);
    }

    fn project_local_page_edit_document_inner(&mut self, publish_edit: bool) {
        let (owner, layers, selected_layer) = match &self.local_page_edit_document {
            LocalPageEditDocument::Idle => return,
            LocalPageEditDocument::Selection {
                owner,
                selected_layer,
                ..
            } => (owner, None, *selected_layer),
            LocalPageEditDocument::Editing {
                owner,
                layers,
                selected_layer,
                ..
            } => (owner, Some(layers.clone()), *selected_layer),
        };
        if owner.context != self.projected_viewer_context_id()
            || owner.items_generation != self.items_generation
        {
            return;
        }
        let slots = owner.slots.clone();
        for idx in slots {
            if let Some(layers) = &layers {
                if publish_edit {
                    self.set_local_adjust_layers_for_idx_memory_only_inner(idx, layers.clone());
                } else if layers.is_empty() {
                    self.local_adjust_page_layers.remove(&idx);
                } else {
                    self.local_adjust_page_layers.insert(idx, layers.clone());
                }
                if layers.is_empty() {
                    continue;
                }
            }
            let selected = self
                .local_adjust_page_layers
                .get(&idx)
                .filter(|layers| !layers.is_empty())
                .map(|layers| selected_layer.min(layers.len() - 1))
                .unwrap_or(selected_layer);
            self.local_adjust_selected_layers.insert(idx, selected);
        }
    }

    fn local_page_edit_slots_for_target(&self, target: &PageAdjustmentTarget) -> Vec<usize> {
        match &self.local_page_edit_document {
            LocalPageEditDocument::Selection { owner, .. }
            | LocalPageEditDocument::Editing { owner, .. }
                if owner.context == self.projected_viewer_context_id()
                    && owner.items_generation == self.items_generation
                    && owner.target.page_key == target.page_key =>
            {
                return owner.slots.clone();
            }
            _ => {}
        }
        self.page_adjustment_indices(target)
    }

    pub(crate) fn set_local_adjust_layers_for_target_memory_only(
        &mut self,
        target: &PageAdjustmentTarget,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) {
        if let LocalPageEditDocument::Editing {
            owner,
            layers: draft,
            revision,
            ..
        } = &mut self.local_page_edit_document
            && owner.target.page_key == target.page_key
        {
            *draft = layers.clone();
            *revision = revision.wrapping_add(1);
        }
        for idx in self.local_page_edit_slots_for_target(target) {
            self.set_local_adjust_layers_for_idx_memory_only(idx, layers.clone());
        }
    }

    pub(crate) fn set_local_adjust_layers_for_target(
        &mut self,
        target: &PageAdjustmentTarget,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) {
        self.set_local_adjust_layers_for_target_memory_only(target, layers.clone());
        self.ensure_local_adjust_write_handle();
        let revision = self
            .local_page_edit_revision_for_target(target)
            .unwrap_or(0);
        let origin = LocalAdjustWriteOrigin {
            context: self.projected_viewer_context_id(),
            target: target.clone(),
            revision,
        };
        self.submit_local_page_edit_request(origin, layers);
    }

    fn submit_local_page_edit_request(
        &mut self,
        origin: LocalAdjustWriteOrigin,
        layers: local_adjust_core::LocalAdjustmentLayers,
    ) {
        self.ensure_local_adjust_write_handle();
        let target = &origin.target;
        self.cancel_superseded_page_edit_producers(&target.page_key, &layers, true);
        self.invalidate_edit_preview_cache_for_key(&target.page_key);
        Self::set_page_key_presence(
            &mut self.local_adjust_page_keys,
            &target.page_key,
            !layers.is_empty(),
        );
        let layer_count = layers.len();
        let key = target.page_key.clone();
        let submitted = self
            .local_adjust_write_handle
            .as_ref()
            .expect("initialized writer")
            .submit_with_origin(target.page_key.clone(), layers.clone(), origin.clone());
        match submitted {
            Ok(generation) => {
                crate::perf::event(
                    "local_adjust",
                    "save_enqueue",
                    Some(&target.page_key),
                    generation,
                    &[("layers", serde_json::Value::from(layer_count))],
                );
                self.local_adjust_write_pending.insert(
                    target.page_key.clone(),
                    LocalAdjustWritePending {
                        generation,
                        origin,
                        layers: layers.clone(),
                    },
                );
                // The accepted document is authoritative before completion.
                // Publish its presence and pending status through the same
                // boundary used by the latest successful completion.
                self.publish_accepted_local_page_edits(&HashMap::from([(key, layers)]));
            }
            Err(outcome) => {
                self.edit_store_write_succeeded("補正レイヤー", outcome);
            }
        }
    }

    pub(crate) fn project_adjustment_page_edit_document(&mut self) {
        let Some(session) = self.adjustment_drag_session.as_ref() else {
            return;
        };
        if session.owner.context != self.projected_viewer_context_id()
            || session.owner.items_generation != self.items_generation
        {
            return;
        }
        let slots = session.owner.slots.clone();
        let draft = session.draft.clone();
        for idx in slots {
            if let Some(params) = &draft {
                self.adjustment_page_params.insert(idx, params.clone());
            } else {
                self.adjustment_page_params.remove(&idx);
            }
        }
    }

    pub(crate) fn commit_adjustment_page_edit_session(&mut self, session: AdjustmentDragSession) {
        self.deliver_page_edit_commits(vec![PageEditCommit::Page(session)]);
    }

    pub(crate) fn finish_adjustment_drag_context_boundary(&mut self) {
        let mut commits = Vec::new();
        if let Some(session) = self.adjustment_drag_session.take() {
            commits.push(PageEditCommit::Page(session));
        }
        if let Some(session) = self.adjustment_standard_drag_session.take() {
            commits.push(PageEditCommit::Standard(session));
        }
        self.deliver_page_edit_commits(commits);
        self.adjustment_dragging = false;
        self.thumb_adjust_drag_color_dirty = false;
    }

    pub(crate) fn prepare_page_edit_history_boundary(&mut self) {
        // Each existing terminal takes its payload before recording history.
        // The low-level history push never calls this boundary recursively.
        self.commit_local_adjust_pending_edits();
        self.finish_adjustment_drag_context_boundary();
        let commits = self.take_parked_page_edit_commits();
        self.deliver_page_edit_commits(commits);
        self.settle_local_page_edit_document();
    }

    pub(super) fn deliver_page_edit_commits(&mut self, commits: Vec<PageEditCommit>) {
        for commit in commits {
            match commit {
                PageEditCommit::Local {
                    origin,
                    before,
                    after,
                    summary,
                } => {
                    if before == after {
                        continue;
                    }
                    let target = origin.target.clone();
                    self.submit_local_page_edit_request(origin, after.clone());
                    self.capture_local_adjustment_undo_for_target(target, before, after, summary);
                }
                PageEditCommit::Page(session) => {
                    let after = session.draft.clone().filter(|params| {
                        *params != self.adjustment_standard_params_for_target(&session.owner.target)
                    });
                    let standard =
                        self.adjustment_standard_params_for_target(&session.owner.target);
                    let before_effective = session.before.as_ref().unwrap_or(&standard);
                    let after_effective = after.as_ref().unwrap_or(&standard);
                    if !before_effective.color_settings_eq(after_effective) {
                        self.invalidate_thumb_adjust_for_page_key(&session.owner.target.page_key);
                    }
                    self.publish_finished_adjustment_page_edit(&session.owner, &after);
                    if session.before == after {
                        continue;
                    }
                    let outcome = if let Some(params) = &session.draft {
                        self.set_page_params_for_target(&session.owner.target, params.clone())
                    } else {
                        self.clear_page_params_for_target(&session.owner.target)
                    };
                    self.edit_store_write_succeeded("補正", outcome);
                    self.capture_adjustment_undo(
                        crate::undo_stack::AdjustUndoScope::PageKey(session.owner.target),
                        session.before,
                        after,
                        "ページ個別の補正".to_owned(),
                    );
                }
                PageEditCommit::Standard(session) => {
                    if session.before == session.draft {
                        continue;
                    }
                    match session.scope {
                        AdjustmentStandardScope::Favorite(id) => {
                            self.adjustment_favorite_params
                                .insert(id, session.before.clone());
                            self.set_favorite_default(id, session.draft.clone());
                        }
                        AdjustmentStandardScope::Global => {
                            self.settings.global_preset = session.before.clone();
                            self.copy_params_to_global(session.draft.clone());
                        }
                    }
                    self.capture_adjustment_undo(
                        match session.scope {
                            AdjustmentStandardScope::Favorite(id) => {
                                crate::undo_stack::AdjustUndoScope::Favorite(id)
                            }
                            AdjustmentStandardScope::Global => {
                                crate::undo_stack::AdjustUndoScope::Global
                            }
                        },
                        Some(session.before),
                        Some(session.draft),
                        "標準の補正".to_owned(),
                    );
                }
            }
        }
    }
}
