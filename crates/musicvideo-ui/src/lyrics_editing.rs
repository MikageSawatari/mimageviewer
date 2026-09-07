use std::any::TypeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::text::CCursorRange;
use egui::text_edit::TextEditState;
use egui::{Context, Event, Id, ImeEvent, Key, TextBuffer};
use musicvideo_core::{LayerId, LyricLine};

const NATIVE_SIDECAR_LIMIT: usize = 256;
const EVICTED_TEXT_MARKER_LIMIT: usize = 256;
const NATIVE_HISTORY_ENUMERATION_LIMIT: usize = 512;

static NEXT_WIDGET_GENERATION: AtomicU64 = AtomicU64::new(1);

type NativeTextUndoer = egui::util::undoer::Undoer<(CCursorRange, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LyricLineId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LyricsIdentitySnapshot {
    line_ids: Vec<LyricLineId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NativeCandidateId(u64);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedLineChoice {
    pub(crate) line_id: LyricLineId,
    pub(crate) source_text: String,
    pub(crate) time_secs: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativeHistoryChoice {
    pub(crate) candidate_id: NativeCandidateId,
    pub(crate) lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LyricsEditConflictKind {
    TimedLineMerge { choices: Vec<TimedLineChoice> },
    AmbiguousNativeHistory { choices: Vec<NativeHistoryChoice> },
    UnrecordedNativeHistory { target_is_empty: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LyricsEditConflict {
    pub(crate) before: Vec<LyricLine>,
    pub(crate) after_text: String,
    pub(crate) kind: LyricsEditConflictKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LyricsPassResult {
    Unchanged,
    Applied(Vec<LyricLine>),
    NeedsResolution(LyricsEditConflict),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyTextMeaning {
    NoLines,
    OneUntimedLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LyricsResolutionChoice {
    KeepTimedLine(LyricLineId),
    UseNativeCandidate(NativeCandidateId),
    ApplyAsNewUntimed(EmptyTextMeaning),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LyricsSyncError {
    TextOrIdentityChanged,
    SnapshotLineCountMismatch,
    SnapshotContainsDuplicateIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LyricsResolutionError {
    NoPendingResolution,
    TextOrIdentityChanged,
    ChoiceDoesNotMatchConflict,
}

/// State which egui mutates before `TextBuffer::replace_with` is called for
/// native TextEdit Undo/Redo. A clone of `TextEditState` alone is insufficient:
/// its undoer is behind an `Arc`, so the undoer must be copied separately.
pub(crate) struct TextEditRollback {
    state: TextEditState,
    undoer: NativeTextUndoer,
    reachability: NativeHistoryReachability,
}

impl TextEditRollback {
    fn capture(context: &Context, id: Id, current_text: &str) -> Option<Self> {
        let state = TextEditState::load(context, id)?;
        let undoer = state.undoer();
        let reachability = native_history_reachability(&state, &undoer, current_text);
        Some(Self {
            state,
            undoer,
            reachability,
        })
    }

    pub(crate) fn restore(mut self, context: &Context, id: Id) {
        self.state.set_undoer(self.undoer);
        self.state.store(context, id);
    }
}

#[derive(Clone)]
enum NativeHistoryReachability {
    Known(HashSet<u64>),
    Unknown,
}

pub(crate) struct PendingLyricsInvalidation {
    rollback: Option<TextEditRollback>,
}

impl PendingLyricsInvalidation {
    pub(crate) fn restore_text_edit(self, context: &Context, id: Id) {
        if let Some(rollback) = self.rollback {
            rollback.restore(context, id);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct TrackedLine {
    id: LyricLineId,
    text: String,
    time_secs: Option<f64>,
}

impl TrackedLine {
    fn as_lyric_line(&self) -> LyricLine {
        LyricLine {
            text: self.text.clone(),
            time_secs: self.time_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticRevision {
    id: NativeCandidateId,
    text: String,
    line_ids: Vec<LyricLineId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mutation {
    Delete(Range<usize>),
    Insert { at: usize, text: String },
    NativeReplace(String),
}

struct ActivePass {
    mutations: Vec<Mutation>,
    rollback: Option<TextEditRollback>,
    post_reachability: Option<NativeHistoryReachability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayDecision {
    KeepLine(LyricLineId),
    UseNativeCandidate(NativeCandidateId),
    ApplyAsNewUntimed(EmptyTextMeaning),
}

struct PendingResolution {
    mutations: Vec<Mutation>,
    decisions: Vec<ReplayDecision>,
    conflict: LyricsEditConflictKind,
    rollback: Option<TextEditRollback>,
    post_reachability: Option<NativeHistoryReachability>,
}

#[derive(Debug, Clone)]
struct ReplayState {
    lines: Vec<TrackedLine>,
    tombstones: HashMap<LyricLineId, Option<f64>>,
    next_line_id: u64,
}

enum ReplayResult {
    Applied {
        state: ReplayState,
        trace: Vec<SemanticRevision>,
    },
    Conflict(LyricsEditConflictKind),
    InvalidChoice,
}

/// Per-layer lyrics text state.
///
/// `LyricLine` remains the serialized model. This buffer owns only the editing
/// identity needed to keep a cue attached to the line the user edited.
pub(crate) struct LyricsTextBuffer {
    text: String,
    lines: Vec<TrackedLine>,
    tombstones: HashMap<LyricLineId, Option<f64>>,
    next_line_id: u64,
    next_revision_id: u64,
    widget_generation: u64,
    sidecar: VecDeque<SemanticRevision>,
    evicted_text_hashes: HashSet<u64>,
    all_native_targets_incomplete: bool,
    native_reachable_text_hashes: Option<HashSet<u64>>,
    pass: Option<ActivePass>,
    pending: Option<PendingResolution>,
}

impl LyricsTextBuffer {
    pub(crate) fn new_session(lines: &[LyricLine]) -> Self {
        Self::new(
            lines,
            NEXT_WIDGET_GENERATION.fetch_add(1, Ordering::Relaxed),
        )
    }

    pub(crate) fn from_identity_snapshot_session(
        lines: &[LyricLine],
        snapshot: &LyricsIdentitySnapshot,
    ) -> Result<Self, LyricsSyncError> {
        Self::from_identity_snapshot(
            lines,
            snapshot,
            NEXT_WIDGET_GENERATION.fetch_add(1, Ordering::Relaxed),
        )
    }

    pub(crate) fn new(lines: &[LyricLine], widget_generation: u64) -> Self {
        let mut next_line_id = 1;
        let tracked = lines
            .iter()
            .map(|line| {
                let id = LyricLineId(next_line_id);
                next_line_id = next_line_id.saturating_add(1);
                TrackedLine {
                    id,
                    text: line.text.clone(),
                    time_secs: line.time_secs,
                }
            })
            .collect::<Vec<_>>();
        let mut this = Self {
            text: joined_text(&tracked),
            lines: tracked,
            tombstones: HashMap::new(),
            next_line_id,
            next_revision_id: 1,
            widget_generation,
            sidecar: VecDeque::new(),
            evicted_text_hashes: HashSet::new(),
            all_native_targets_incomplete: false,
            native_reachable_text_hashes: None,
            pass: None,
            pending: None,
        };
        this.record_current_revision();
        this
    }

    pub(crate) fn from_identity_snapshot(
        lines: &[LyricLine],
        snapshot: &LyricsIdentitySnapshot,
        widget_generation: u64,
    ) -> Result<Self, LyricsSyncError> {
        validate_snapshot(lines, snapshot)?;
        let tracked = lines
            .iter()
            .zip(&snapshot.line_ids)
            .map(|(line, id)| TrackedLine {
                id: *id,
                text: line.text.clone(),
                time_secs: line.time_secs,
            })
            .collect::<Vec<_>>();
        let next_line_id = next_unused_line_id(&tracked, 1);
        let mut this = Self {
            text: joined_text(&tracked),
            lines: tracked,
            tombstones: HashMap::new(),
            next_line_id,
            next_revision_id: 1,
            widget_generation,
            sidecar: VecDeque::new(),
            evicted_text_hashes: HashSet::new(),
            all_native_targets_incomplete: false,
            native_reachable_text_hashes: None,
            pass: None,
            pending: None,
        };
        this.record_current_revision();
        Ok(this)
    }

    pub(crate) fn widget_id(&self, layer: LayerId) -> Id {
        Id::new(("musicvideo-lyrics", self.widget_generation, layer.0))
    }

    pub(crate) fn identity_snapshot(&self) -> LyricsIdentitySnapshot {
        LyricsIdentitySnapshot {
            line_ids: self.lines.iter().map(|line| line.id).collect(),
        }
    }

    pub(crate) fn committed_lines(&self) -> Vec<LyricLine> {
        self.lines.iter().map(TrackedLine::as_lyric_line).collect()
    }

    /// Synchronizes tap/timeline time edits without changing text identity or
    /// adding a native-text history entry.
    pub(crate) fn sync_times(
        &mut self,
        project_lines: &[LyricLine],
    ) -> Result<(), LyricsSyncError> {
        self.sync_current_times(project_lines)
    }

    pub(crate) fn has_pending_resolution(&self) -> bool {
        self.pending.is_some()
    }

    /// Copies the native TextEdit state only on a pass which can mutate text.
    /// Idle draw passes and cursor-only key presses do not clone the bounded
    /// native undo history.
    pub(crate) fn capture_text_edit_rollback_if_needed(
        &self,
        context: &Context,
        id: Id,
    ) -> Option<TextEditRollback> {
        if context.memory(|memory| memory.focused()) != Some(id)
            || !context.input(|input| input.events.iter().any(event_may_mutate_text))
        {
            return None;
        }
        TextEditRollback::capture(context, id, &self.text)
    }

    /// Refreshes sidecar eviction markers before a non-TextEdit replacement
    /// (for example LRC or global Undo). Unlike the per-pass capture this is an
    /// explicit, infrequent deep clone of egui's bounded native undoer.
    pub(crate) fn refresh_native_history_reachability(&mut self, context: &Context, id: Id) {
        let reachability =
            TextEditState::load(context, id).map_or(NativeHistoryReachability::Unknown, |state| {
                let undoer = state.undoer();
                native_history_reachability(&state, &undoer, &self.text)
            });
        if let Some(pass) = self.pass.as_mut() {
            pass.post_reachability = Some(reachability);
        } else if self.pending.is_none() {
            self.observe_native_reachability(&reachability);
        }
    }

    /// A context-free external replacement cannot inspect egui's current
    /// native history. Forget the last observation so any required eviction is
    /// marked incomplete instead of trusting stale reachability information.
    pub(crate) fn forget_native_history_reachability(&mut self) {
        self.native_reachable_text_hashes = None;
    }

    pub(crate) fn begin_pass(
        &mut self,
        project_lines: &[LyricLine],
        rollback: Option<TextEditRollback>,
    ) -> Result<(), LyricsSyncError> {
        self.sync_current_times(project_lines)?;
        if self.pending.is_some() {
            return Ok(());
        }
        if let Some(rollback) = rollback.as_ref() {
            self.observe_native_reachability(&rollback.reachability);
        }
        self.pass = Some(ActivePass {
            mutations: Vec::new(),
            rollback,
            post_reachability: None,
        });
        Ok(())
    }

    pub(crate) fn finish_pass(&mut self) -> LyricsPassResult {
        let Some(pass) = self.pass.take() else {
            return LyricsPassResult::Unchanged;
        };
        if pass.mutations.is_empty() {
            return LyricsPassResult::Unchanged;
        }
        let ActivePass {
            mutations,
            rollback,
            post_reachability,
        } = pass;
        let decisions = Vec::new();
        match self.replay(&mutations, &decisions) {
            ReplayResult::Applied { state, trace } => {
                if let Some(reachability) = post_reachability.as_ref() {
                    self.observe_native_reachability(reachability);
                }
                self.commit_replay(state, trace)
            }
            ReplayResult::Conflict(conflict) => {
                let view = self.conflict_view(conflict.clone());
                self.pending = Some(PendingResolution {
                    mutations,
                    decisions,
                    conflict,
                    rollback,
                    post_reachability,
                });
                LyricsPassResult::NeedsResolution(view)
            }
            ReplayResult::InvalidChoice => {
                unreachable!("a replay without decisions cannot contain an invalid choice")
            }
        }
    }

    pub(crate) fn pending_conflict(&self) -> Option<LyricsEditConflict> {
        self.pending
            .as_ref()
            .map(|pending| self.conflict_view(pending.conflict.clone()))
    }

    /// Re-evaluates a pending proposal against the latest cue times. This is
    /// cheap (lyrics text only) and lets a timeline/tap edit remove or change a
    /// conflict without overwriting the newer Project values.
    pub(crate) fn refresh_pending(
        &mut self,
        project_lines: &[LyricLine],
    ) -> Result<LyricsPassResult, LyricsResolutionError> {
        self.sync_current_times(project_lines)
            .map_err(|_| LyricsResolutionError::TextOrIdentityChanged)?;
        let Some(mut pending) = self.pending.take() else {
            return Err(LyricsResolutionError::NoPendingResolution);
        };
        match self.replay(&pending.mutations, &pending.decisions) {
            ReplayResult::Applied { state, trace } => {
                if let Some(reachability) = pending.post_reachability.as_ref() {
                    self.observe_native_reachability(reachability);
                }
                Ok(self.commit_replay(state, trace))
            }
            ReplayResult::Conflict(conflict) => {
                let view = self.conflict_view(conflict.clone());
                pending.conflict = conflict;
                self.pending = Some(pending);
                Ok(LyricsPassResult::NeedsResolution(view))
            }
            ReplayResult::InvalidChoice => {
                self.pending = Some(pending);
                Err(LyricsResolutionError::ChoiceDoesNotMatchConflict)
            }
        }
    }

    pub(crate) fn resolve_pending(
        &mut self,
        project_lines: &[LyricLine],
        choice: LyricsResolutionChoice,
    ) -> Result<LyricsPassResult, LyricsResolutionError> {
        self.sync_current_times(project_lines)
            .map_err(|_| LyricsResolutionError::TextOrIdentityChanged)?;
        let Some(mut pending) = self.pending.take() else {
            return Err(LyricsResolutionError::NoPendingResolution);
        };
        match self.replay(&pending.mutations, &pending.decisions) {
            ReplayResult::Applied { state, trace } => {
                if let Some(reachability) = pending.post_reachability.as_ref() {
                    self.observe_native_reachability(reachability);
                }
                return Ok(self.commit_replay(state, trace));
            }
            ReplayResult::Conflict(conflict) => pending.conflict = conflict,
            ReplayResult::InvalidChoice => {
                self.pending = Some(pending);
                return Err(LyricsResolutionError::ChoiceDoesNotMatchConflict);
            }
        }
        let decision = match (&pending.conflict, choice) {
            (
                LyricsEditConflictKind::TimedLineMerge { choices },
                LyricsResolutionChoice::KeepTimedLine(id),
            ) if choices.iter().any(|candidate| candidate.line_id == id) => {
                ReplayDecision::KeepLine(id)
            }
            (
                LyricsEditConflictKind::AmbiguousNativeHistory { choices },
                LyricsResolutionChoice::UseNativeCandidate(id),
            ) if choices.iter().any(|candidate| candidate.candidate_id == id) => {
                ReplayDecision::UseNativeCandidate(id)
            }
            (
                LyricsEditConflictKind::UnrecordedNativeHistory { .. },
                LyricsResolutionChoice::ApplyAsNewUntimed(empty),
            ) => ReplayDecision::ApplyAsNewUntimed(empty),
            _ => {
                self.pending = Some(pending);
                return Err(LyricsResolutionError::ChoiceDoesNotMatchConflict);
            }
        };
        pending.decisions.push(decision);
        match self.replay(&pending.mutations, &pending.decisions) {
            ReplayResult::Applied { state, trace } => {
                if let Some(reachability) = pending.post_reachability.as_ref() {
                    self.observe_native_reachability(reachability);
                }
                Ok(self.commit_replay(state, trace))
            }
            ReplayResult::Conflict(conflict) => {
                let view = self.conflict_view(conflict.clone());
                pending.conflict = conflict;
                self.pending = Some(pending);
                Ok(LyricsPassResult::NeedsResolution(view))
            }
            ReplayResult::InvalidChoice => {
                self.pending = Some(pending);
                Err(LyricsResolutionError::ChoiceDoesNotMatchConflict)
            }
        }
    }

    pub(crate) fn cancel_pending(&mut self) -> Option<PendingLyricsInvalidation> {
        let pending = self.pending.take()?;
        let committed_text = joined_text(&self.lines);
        self.text = committed_text;
        Some(PendingLyricsInvalidation {
            rollback: pending.rollback,
        })
    }

    /// Applies an authoritative replacement such as an imported LRC file.
    /// Existing native-history correspondence stays in the bounded sidecar.
    pub(crate) fn replace_external(
        &mut self,
        lines: &[LyricLine],
    ) -> Option<PendingLyricsInvalidation> {
        let invalidated = self.cancel_pending();
        self.pass = None;
        if same_project_lines(&self.lines, lines) {
            let _ = self.sync_current_times(lines);
            self.text = joined_text(&self.lines);
            self.record_current_revision();
            return invalidated;
        }

        self.tombstone_removed_lines(&[]);
        let mut replacement = Vec::with_capacity(lines.len());
        for line in lines {
            let id = self.allocate_line_id();
            replacement.push(TrackedLine {
                id,
                text: line.text.clone(),
                time_secs: line.time_secs,
            });
        }
        self.lines = replacement;
        self.text = joined_text(&self.lines);
        self.record_current_revision();
        invalidated
    }

    /// Restores the identity stored alongside a global Project snapshot while
    /// retaining the bounded union of native TextEdit history correspondence.
    pub(crate) fn restore_global_snapshot(
        &mut self,
        lines: &[LyricLine],
        snapshot: &LyricsIdentitySnapshot,
    ) -> Result<Option<PendingLyricsInvalidation>, LyricsSyncError> {
        validate_snapshot(lines, snapshot)?;
        let invalidated = self.cancel_pending();
        self.pass = None;

        let restored_ids = snapshot.line_ids.iter().copied().collect::<HashSet<_>>();
        for line in &self.lines {
            if !restored_ids.contains(&line.id) {
                self.tombstones.insert(line.id, line.time_secs);
            }
        }
        self.lines = lines
            .iter()
            .zip(&snapshot.line_ids)
            .map(|(line, id)| TrackedLine {
                id: *id,
                text: line.text.clone(),
                time_secs: line.time_secs,
            })
            .collect();
        self.next_line_id = next_unused_line_id(&self.lines, self.next_line_id);
        self.text = joined_text(&self.lines);
        self.record_current_revision();
        Ok(invalidated)
    }

    fn ensure_pass(&mut self) -> &mut ActivePass {
        self.pass.get_or_insert_with(|| ActivePass {
            mutations: Vec::new(),
            rollback: None,
            post_reachability: None,
        })
    }

    fn sync_current_times(&mut self, project_lines: &[LyricLine]) -> Result<(), LyricsSyncError> {
        if self.lines.len() != project_lines.len()
            || self
                .lines
                .iter()
                .zip(project_lines)
                .any(|(tracked, project)| tracked.text != project.text)
        {
            return Err(LyricsSyncError::TextOrIdentityChanged);
        }
        for (tracked, project) in self.lines.iter_mut().zip(project_lines) {
            tracked.time_secs = project.time_secs;
        }
        Ok(())
    }

    fn replay(&self, mutations: &[Mutation], decisions: &[ReplayDecision]) -> ReplayResult {
        let mut state = ReplayState {
            lines: self.lines.clone(),
            tombstones: self.tombstones.clone(),
            next_line_id: self.next_line_id,
        };
        // Native Undo/Redo can occur after ordinary input in the same egui
        // pass. Keep each successfully replayed intermediate mapping available
        // to later replace_with calls; the persistent sidecar does not contain
        // those states until the pass commits.
        let mut replay_sidecar = self.sidecar.clone();
        let mut replay_revision_id = self.next_revision_id;
        let mut trace = Vec::new();
        let mut decision_index = 0;
        let mut mutation_index = 0;
        while mutation_index < mutations.len() {
            let result = match &mutations[mutation_index] {
                Mutation::Delete(range)
                    if matches!(
                        mutations.get(mutation_index + 1),
                        Some(Mutation::Insert { at, .. }) if *at == range.start
                    ) =>
                {
                    let Mutation::Insert { text, .. } = &mutations[mutation_index + 1] else {
                        unreachable!()
                    };
                    mutation_index += 2;
                    apply_splice(
                        &mut state,
                        range.clone(),
                        text,
                        decisions.get(decision_index),
                    )
                }
                Mutation::Delete(range) => {
                    mutation_index += 1;
                    apply_splice(&mut state, range.clone(), "", decisions.get(decision_index))
                }
                Mutation::Insert { at, text } => {
                    mutation_index += 1;
                    apply_splice(&mut state, *at..*at, text, decisions.get(decision_index))
                }
                Mutation::NativeReplace(target) => {
                    mutation_index += 1;
                    apply_native_replace(
                        &mut state,
                        target,
                        &replay_sidecar,
                        self.native_candidates_incomplete(target),
                        decisions.get(decision_index),
                    )
                }
            };
            match result {
                Ok(used_decision) => {
                    decision_index += usize::from(used_decision);
                    if let Some(revision) =
                        record_replay_revision(&mut replay_sidecar, &state, &mut replay_revision_id)
                    {
                        trace.push(revision);
                    }
                }
                Err(ReplayFailure::Conflict(conflict)) => {
                    if decisions.get(decision_index).is_some() {
                        return ReplayResult::InvalidChoice;
                    }
                    return ReplayResult::Conflict(conflict);
                }
                Err(ReplayFailure::InvalidChoice) => return ReplayResult::InvalidChoice,
            }
        }
        if decision_index != decisions.len() {
            ReplayResult::InvalidChoice
        } else {
            ReplayResult::Applied { state, trace }
        }
    }

    fn commit_replay(
        &mut self,
        state: ReplayState,
        trace: Vec<SemanticRevision>,
    ) -> LyricsPassResult {
        debug_assert_eq!(
            joined_text(&state.lines),
            self.text,
            "semantic lyrics must exactly match the TextBuffer result"
        );
        let changed = self.lines != state.lines;
        self.lines = state.lines;
        self.tombstones = state.tombstones;
        self.next_line_id = state.next_line_id;
        self.text = joined_text(&self.lines);
        self.pending = None;
        if let Some(reachable) = self.native_reachable_text_hashes.as_ref() {
            let promoted = trace
                .into_iter()
                .filter(|revision| reachable.contains(&text_hash(&revision.text)))
                .collect::<Vec<_>>();
            for revision in promoted {
                self.store_revision(revision);
            }
        }
        self.record_current_revision();
        if changed {
            LyricsPassResult::Applied(self.committed_lines())
        } else {
            LyricsPassResult::Unchanged
        }
    }

    fn conflict_view(&self, kind: LyricsEditConflictKind) -> LyricsEditConflict {
        LyricsEditConflict {
            before: self.committed_lines(),
            after_text: self.text.clone(),
            kind,
        }
    }

    fn allocate_line_id(&mut self) -> LyricLineId {
        let id = LyricLineId(self.next_line_id);
        self.next_line_id = self.next_line_id.saturating_add(1);
        id
    }

    fn tombstone_removed_lines(&mut self, retained: &[LyricLineId]) {
        let retained = retained.iter().copied().collect::<HashSet<_>>();
        for line in &self.lines {
            if !retained.contains(&line.id) {
                self.tombstones.insert(line.id, line.time_secs);
            }
        }
    }

    fn record_current_revision(&mut self) {
        let line_ids = self.lines.iter().map(|line| line.id).collect::<Vec<_>>();
        if self
            .sidecar
            .iter()
            .any(|revision| revision.text == self.text && revision.line_ids == line_ids)
        {
            self.retain_live_tombstones();
            return;
        }
        let id = NativeCandidateId(self.next_revision_id);
        self.next_revision_id = self.next_revision_id.saturating_add(1);
        self.store_revision(SemanticRevision {
            id,
            text: self.text.clone(),
            line_ids,
        });
        self.retain_live_tombstones();
    }

    fn store_revision(&mut self, revision: SemanticRevision) {
        self.next_revision_id = self.next_revision_id.max(revision.id.0.saturating_add(1));
        if self
            .sidecar
            .iter()
            .any(|known| known.text == revision.text && known.line_ids == revision.line_ids)
        {
            return;
        }
        self.sidecar.push_back(revision);
        while self.sidecar.len() > NATIVE_SIDECAR_LIMIT {
            let last = self.sidecar.len() - 1;
            let eviction_index = (0..last)
                .find(|index| {
                    self.native_reachable_text_hashes
                        .as_ref()
                        .is_none_or(|reachable| {
                            !reachable.contains(&text_hash(&self.sidecar[*index].text))
                        })
                })
                .unwrap_or(0);
            let evicted = self
                .sidecar
                .remove(eviction_index)
                .expect("sidecar eviction index is in bounds");
            let same_mapping_remains = self
                .sidecar
                .iter()
                .any(|known| known.text == evicted.text && known.line_ids == evicted.line_ids);
            if !same_mapping_remains {
                self.mark_evicted_text(&evicted.text);
            }
        }
    }

    fn mark_evicted_text(&mut self, text: &str) {
        let hash = text_hash(text);
        if self
            .native_reachable_text_hashes
            .as_ref()
            .is_some_and(|reachable| !reachable.contains(&hash))
        {
            return;
        }
        if self.all_native_targets_incomplete {
            return;
        }
        if self.evicted_text_hashes.contains(&hash) {
            return;
        }
        if self.evicted_text_hashes.len() == EVICTED_TEXT_MARKER_LIMIT {
            self.evicted_text_hashes.clear();
            self.all_native_targets_incomplete = true;
        } else {
            self.evicted_text_hashes.insert(hash);
        }
    }

    fn native_candidates_incomplete(&self, target: &str) -> bool {
        self.all_native_targets_incomplete || self.evicted_text_hashes.contains(&text_hash(target))
    }

    fn observe_native_reachability(&mut self, reachability: &NativeHistoryReachability) {
        let NativeHistoryReachability::Known(reachable) = reachability else {
            self.native_reachable_text_hashes = None;
            return;
        };
        if self.all_native_targets_incomplete {
            self.evicted_text_hashes = reachable.clone();
            self.all_native_targets_incomplete = false;
        } else {
            self.evicted_text_hashes
                .retain(|hash| reachable.contains(hash));
        }
        self.native_reachable_text_hashes = Some(reachable.clone());
    }

    fn retain_live_tombstones(&mut self) {
        let mut retained = self
            .lines
            .iter()
            .map(|line| line.id)
            .collect::<HashSet<_>>();
        for revision in &self.sidecar {
            retained.extend(revision.line_ids.iter().copied());
        }
        self.tombstones.retain(|id, _| retained.contains(id));
    }
}

impl TextBuffer for LyricsTextBuffer {
    fn is_mutable(&self) -> bool {
        self.pending.is_none()
    }

    fn as_str(&self) -> &str {
        &self.text
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        if self.pending.is_some() {
            return 0;
        }
        let text = normalize_newlines(text);
        let inserted_chars = text.chars().count();
        let char_index = char_index.min(self.text.chars().count());
        let byte_index = byte_index(&self.text, char_index);
        self.text.insert_str(byte_index, &text);
        self.ensure_pass().mutations.push(Mutation::Insert {
            at: char_index,
            text,
        });
        inserted_chars
    }

    fn delete_char_range(&mut self, char_range: Range<usize>) {
        if self.pending.is_some() {
            return;
        }
        assert!(char_range.start <= char_range.end);
        let char_count = self.text.chars().count();
        let char_range = char_range.start.min(char_count)..char_range.end.min(char_count);
        let byte_start = byte_index(&self.text, char_range.start);
        let byte_end = byte_index(&self.text, char_range.end);
        self.text.replace_range(byte_start..byte_end, "");
        self.ensure_pass()
            .mutations
            .push(Mutation::Delete(char_range));
    }

    fn clear(&mut self) {
        let char_count = self.text.chars().count();
        self.delete_char_range(0..char_count);
    }

    fn replace_with(&mut self, text: &str) {
        if self.pending.is_some() {
            return;
        }
        let text = normalize_newlines(text);
        text.clone_into(&mut self.text);
        self.ensure_pass()
            .mutations
            .push(Mutation::NativeReplace(text));
    }

    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

enum ReplayFailure {
    Conflict(LyricsEditConflictKind),
    InvalidChoice,
}

fn apply_splice(
    state: &mut ReplayState,
    range: Range<usize>,
    inserted: &str,
    decision: Option<&ReplayDecision>,
) -> Result<bool, ReplayFailure> {
    let old_text = joined_text(&state.lines);
    let char_count = old_text.chars().count();
    if range.start > range.end || range.end > char_count {
        return Err(ReplayFailure::InvalidChoice);
    }
    if state.lines.is_empty() {
        if range != (0..0) {
            return Err(ReplayFailure::InvalidChoice);
        }
        if inserted.is_empty() {
            return Ok(false);
        }
        state.lines = split_new_lines(inserted)
            .into_iter()
            .map(|text| new_tracked_line(state, text))
            .collect();
        return Ok(false);
    }

    let start =
        locate_char_boundary(&state.lines, range.start).ok_or(ReplayFailure::InvalidChoice)?;
    let end = locate_char_boundary(&state.lines, range.end).ok_or(ReplayFailure::InvalidChoice)?;
    let affected = state.lines[start.line..=end.line].to_vec();
    let start_line = &affected[0];
    let end_line = affected.last().expect("affected range is never empty");
    let prefix = char_slice(&start_line.text, 0..start.column).to_owned();
    let suffix = char_slice(&end_line.text, end.column..end_line.text.chars().count()).to_owned();
    let inserted_parts = inserted.split('\n').collect::<Vec<_>>();
    let mut output_texts = if inserted_parts.len() == 1 {
        vec![format!("{prefix}{}{suffix}", inserted_parts[0])]
    } else {
        let mut output = Vec::with_capacity(inserted_parts.len());
        output.push(format!("{prefix}{}", inserted_parts[0]));
        output.extend(
            inserted_parts[1..inserted_parts.len() - 1]
                .iter()
                .map(|part| (*part).to_owned()),
        );
        output.push(format!(
            "{}{suffix}",
            inserted_parts[inserted_parts.len() - 1]
        ));
        output
    };

    let mut candidates = vec![Vec::<TrackedLine>::new(); output_texts.len()];
    if start.line == end.line {
        let owner_output = if output_texts.len() == 1 || start.column > 0 {
            0
        } else if end.column < start_line.text.chars().count() {
            output_texts.len() - 1
        } else {
            0
        };
        candidates[owner_output].push(start_line.clone());
    } else {
        let start_len = start_line.text.chars().count();
        let start_survives = start.column > 0 || start_len == 0;
        let end_len = end_line.text.chars().count();
        let end_survives = end.column < end_len || end_len == 0;
        if start_survives {
            candidates[0].push(start_line.clone());
        }
        if end_survives {
            candidates[output_texts.len() - 1].push(end_line.clone());
        }
        if range.start == 0
            && range.end == char_count
            && !start_survives
            && !end_survives
            && inserted.is_empty()
            && prefix.is_empty()
            && suffix.is_empty()
        {
            output_texts.clear();
            candidates.clear();
        }
    }

    let conflict = candidates.iter().find_map(timed_merge_conflict);
    let mut used_decision = false;
    let selected_conflict_id = if let Some(choices) = conflict {
        match decision {
            Some(ReplayDecision::KeepLine(id))
                if choices.iter().any(|choice| choice.line_id == *id) =>
            {
                used_decision = true;
                Some(*id)
            }
            Some(_) => return Err(ReplayFailure::InvalidChoice),
            None => {
                return Err(ReplayFailure::Conflict(
                    LyricsEditConflictKind::TimedLineMerge { choices },
                ));
            }
        }
    } else if let Some(ReplayDecision::KeepLine(id)) = decision
        && candidates
            .iter()
            .any(|owners| owners.len() > 1 && owners.iter().any(|owner| owner.id == *id))
    {
        used_decision = true;
        Some(*id)
    } else {
        None
    };

    let mut replacement = Vec::with_capacity(output_texts.len());
    for (text, owners) in output_texts.into_iter().zip(candidates) {
        let owner = choose_owner(&owners, selected_conflict_id);
        replacement.push(match owner {
            Some(owner) => TrackedLine {
                id: owner.id,
                text,
                time_secs: owner.time_secs,
            },
            None => new_tracked_line(state, text),
        });
    }

    let retained_ids = replacement
        .iter()
        .map(|line| line.id)
        .collect::<HashSet<_>>();
    for line in affected {
        if !retained_ids.contains(&line.id) {
            state.tombstones.insert(line.id, line.time_secs);
        }
    }
    state.lines.splice(start.line..=end.line, replacement);
    Ok(used_decision)
}

fn apply_native_replace(
    state: &mut ReplayState,
    target: &str,
    sidecar: &VecDeque<SemanticRevision>,
    candidates_incomplete: bool,
    decision: Option<&ReplayDecision>,
) -> Result<bool, ReplayFailure> {
    let current_times = state
        .lines
        .iter()
        .map(|line| (line.id, line.time_secs))
        .collect::<HashMap<_, _>>();
    let revisions = sidecar
        .iter()
        .filter(|revision| revision.text == target)
        .collect::<Vec<_>>();
    let mut distinct = Vec::<&SemanticRevision>::new();
    for revision in revisions {
        if !distinct
            .iter()
            .any(|known| known.line_ids == revision.line_ids)
        {
            distinct.push(revision);
        }
    }

    if distinct.is_empty() || candidates_incomplete {
        return match decision {
            Some(ReplayDecision::ApplyAsNewUntimed(empty)) => {
                for line in &state.lines {
                    state.tombstones.insert(line.id, line.time_secs);
                }
                let replacement = new_untimed_lines(state, target, *empty);
                state.lines = replacement;
                Ok(true)
            }
            Some(_) => Err(ReplayFailure::InvalidChoice),
            None => Err(ReplayFailure::Conflict(
                LyricsEditConflictKind::UnrecordedNativeHistory {
                    target_is_empty: target.is_empty(),
                },
            )),
        };
    }

    if distinct.len() > 1 {
        let choices = distinct
            .iter()
            .filter_map(|revision| {
                materialize_revision(revision, target, &current_times, &state.tombstones).map(
                    |lines| NativeHistoryChoice {
                        candidate_id: revision.id,
                        lines: lines.iter().map(TrackedLine::as_lyric_line).collect(),
                    },
                )
            })
            .collect::<Vec<_>>();
        if choices.len() != distinct.len() {
            return Err(ReplayFailure::Conflict(
                LyricsEditConflictKind::UnrecordedNativeHistory {
                    target_is_empty: target.is_empty(),
                },
            ));
        }
        let selected = match decision {
            Some(ReplayDecision::UseNativeCandidate(id)) => distinct
                .iter()
                .find(|revision| revision.id == *id)
                .copied()
                .ok_or(ReplayFailure::InvalidChoice)?,
            Some(_) => return Err(ReplayFailure::InvalidChoice),
            None => {
                return Err(ReplayFailure::Conflict(
                    LyricsEditConflictKind::AmbiguousNativeHistory { choices },
                ));
            }
        };
        replace_with_revision(state, selected, target, &current_times)?;
        return Ok(true);
    }

    replace_with_revision(state, distinct[0], target, &current_times)?;
    Ok(false)
}

fn record_replay_revision(
    sidecar: &mut VecDeque<SemanticRevision>,
    state: &ReplayState,
    next_revision_id: &mut u64,
) -> Option<SemanticRevision> {
    let text = joined_text(&state.lines);
    let line_ids = state.lines.iter().map(|line| line.id).collect::<Vec<_>>();
    if sidecar
        .back()
        .is_some_and(|known| known.text == text && known.line_ids == line_ids)
    {
        return None;
    }
    let revision = SemanticRevision {
        id: NativeCandidateId(*next_revision_id),
        text,
        line_ids,
    };
    *next_revision_id = next_revision_id.saturating_add(1);
    sidecar.push_back(revision.clone());
    Some(revision)
}

fn replace_with_revision(
    state: &mut ReplayState,
    revision: &SemanticRevision,
    target: &str,
    current_times: &HashMap<LyricLineId, Option<f64>>,
) -> Result<(), ReplayFailure> {
    let restored = materialize_revision(revision, target, current_times, &state.tombstones)
        .ok_or_else(|| {
            ReplayFailure::Conflict(LyricsEditConflictKind::UnrecordedNativeHistory {
                target_is_empty: target.is_empty(),
            })
        })?;
    let restored_ids = restored.iter().map(|line| line.id).collect::<HashSet<_>>();
    for line in &state.lines {
        if !restored_ids.contains(&line.id) {
            state.tombstones.insert(line.id, line.time_secs);
        }
    }
    state.lines = restored;
    Ok(())
}

fn materialize_revision(
    revision: &SemanticRevision,
    target: &str,
    current_times: &HashMap<LyricLineId, Option<f64>>,
    tombstones: &HashMap<LyricLineId, Option<f64>>,
) -> Option<Vec<TrackedLine>> {
    let texts = split_text_for_known_line_count(target, revision.line_ids.len())?;
    revision
        .line_ids
        .iter()
        .zip(texts)
        .map(|(id, text)| {
            let time_secs = current_times
                .get(id)
                .copied()
                .or_else(|| tombstones.get(id).copied())?;
            Some(TrackedLine {
                id: *id,
                text,
                time_secs,
            })
        })
        .collect()
}

fn new_untimed_lines(
    state: &mut ReplayState,
    target: &str,
    empty: EmptyTextMeaning,
) -> Vec<TrackedLine> {
    if target.is_empty() {
        return match empty {
            EmptyTextMeaning::NoLines => Vec::new(),
            EmptyTextMeaning::OneUntimedLine => vec![new_tracked_line(state, String::new())],
        };
    }
    split_new_lines(target)
        .into_iter()
        .map(|text| new_tracked_line(state, text))
        .collect()
}

fn timed_merge_conflict(owners: &Vec<TrackedLine>) -> Option<Vec<TimedLineChoice>> {
    let mut choices = Vec::<TimedLineChoice>::new();
    for owner in owners {
        let Some(time_secs) = owner.time_secs else {
            continue;
        };
        if !choices
            .iter()
            .any(|choice| same_time(choice.time_secs, time_secs))
        {
            choices.push(TimedLineChoice {
                line_id: owner.id,
                source_text: owner.text.clone(),
                time_secs,
            });
        }
    }
    (choices.len() > 1).then_some(choices)
}

fn choose_owner(
    owners: &[TrackedLine],
    selected_conflict_id: Option<LyricLineId>,
) -> Option<&TrackedLine> {
    if let Some(id) = selected_conflict_id
        && let Some(owner) = owners.iter().find(|owner| owner.id == id)
    {
        return Some(owner);
    }
    if let Some(timed) = owners.iter().find(|owner| owner.time_secs.is_some()) {
        return Some(timed);
    }
    owners.first()
}

fn new_tracked_line(state: &mut ReplayState, text: String) -> TrackedLine {
    let id = LyricLineId(state.next_line_id);
    state.next_line_id = state.next_line_id.saturating_add(1);
    TrackedLine {
        id,
        text,
        time_secs: None,
    }
}

#[derive(Debug, Clone, Copy)]
struct TextPosition {
    line: usize,
    column: usize,
}

fn locate_char_boundary(lines: &[TrackedLine], char_index: usize) -> Option<TextPosition> {
    let mut offset = 0;
    for (line, value) in lines.iter().enumerate() {
        let len = value.text.chars().count();
        if char_index <= offset + len {
            return Some(TextPosition {
                line,
                column: char_index - offset,
            });
        }
        offset += len;
        if line + 1 < lines.len() {
            offset += 1;
        }
    }
    None
}

fn validate_snapshot(
    lines: &[LyricLine],
    snapshot: &LyricsIdentitySnapshot,
) -> Result<(), LyricsSyncError> {
    if lines.len() != snapshot.line_ids.len() {
        return Err(LyricsSyncError::SnapshotLineCountMismatch);
    }
    let unique = snapshot.line_ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != snapshot.line_ids.len() {
        return Err(LyricsSyncError::SnapshotContainsDuplicateIdentity);
    }
    Ok(())
}

fn next_unused_line_id(lines: &[TrackedLine], minimum: u64) -> u64 {
    lines
        .iter()
        .map(|line| line.id.0)
        .max()
        .map_or(minimum.max(1), |maximum| {
            minimum.max(maximum.saturating_add(1))
        })
}

fn event_may_mutate_text(event: &Event) -> bool {
    match event {
        Event::Cut | Event::Paste(_) | Event::Text(_) => true,
        Event::Ime(ImeEvent::Preedit(_) | ImeEvent::Commit(_)) => true,
        Event::Key {
            key:
                Key::Backspace
                | Key::Delete
                | Key::Enter
                | Key::Tab
                | Key::H
                | Key::K
                | Key::U
                | Key::W
                | Key::Y
                | Key::Z,
            pressed: true,
            ..
        } => true,
        _ => false,
    }
}

fn native_history_reachability(
    state: &TextEditState,
    undoer: &NativeTextUndoer,
    current_text: &str,
) -> NativeHistoryReachability {
    let Some(cursor) = state.cursor.char_range() else {
        return NativeHistoryReachability::Unknown;
    };
    let current = (cursor, current_text.to_owned());
    let mut hashes = HashSet::from([text_hash(current_text)]);

    // Walk to the oldest undo point, then use the same clone to walk all the
    // way to the newest redo point. This remains a conservative superset when
    // a host-side replacement shortened the text and egui has not clamped the
    // saved cursor yet: the first undo inserts that cursor variant into redo,
    // and the round trip still visits the pre-existing redo branch.
    let mut history = undoer.clone();
    let mut history_cursor = current;
    for step in 0..=NATIVE_HISTORY_ENUMERATION_LIMIT {
        let Some(previous) = history.undo(&history_cursor).cloned() else {
            break;
        };
        if step == NATIVE_HISTORY_ENUMERATION_LIMIT || previous == history_cursor {
            return NativeHistoryReachability::Unknown;
        }
        hashes.insert(text_hash(&previous.1));
        history_cursor = previous;
    }

    for step in 0..=NATIVE_HISTORY_ENUMERATION_LIMIT {
        let Some(next) = history.redo(&history_cursor).cloned() else {
            break;
        };
        if step == NATIVE_HISTORY_ENUMERATION_LIMIT || next == history_cursor {
            return NativeHistoryReachability::Unknown;
        }
        hashes.insert(text_hash(&next.1));
        history_cursor = next;
    }

    NativeHistoryReachability::Known(hashes)
}

fn joined_text(lines: &[TrackedLine]) -> String {
    lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_new_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

fn split_text_for_known_line_count(text: &str, line_count: usize) -> Option<Vec<String>> {
    if text.is_empty() {
        return match line_count {
            0 => Some(Vec::new()),
            1 => Some(vec![String::new()]),
            _ => None,
        };
    }
    let lines = split_new_lines(text);
    (lines.len() == line_count).then_some(lines)
}

fn same_project_lines(tracked: &[TrackedLine], project: &[LyricLine]) -> bool {
    tracked.len() == project.len()
        && tracked.iter().zip(project).all(|(left, right)| {
            left.text == right.text && same_optional_time(left.time_secs, right.time_secs)
        })
}

fn same_optional_time(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => same_time(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn same_time(left: f64, right: f64) -> bool {
    left == right || left.to_bits() == right.to_bits()
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

fn char_slice(text: &str, range: Range<usize>) -> &str {
    &text[byte_index(text, range.start)..byte_index(text, range.end)]
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::text::CCursor;

    fn line(text: &str, time_secs: Option<f64>) -> LyricLine {
        LyricLine {
            text: text.to_owned(),
            time_secs,
        }
    }

    fn edit(
        buffer: &mut LyricsTextBuffer,
        project: &[LyricLine],
        operation: impl FnOnce(&mut LyricsTextBuffer),
    ) -> LyricsPassResult {
        buffer.begin_pass(project, None).unwrap();
        operation(buffer);
        buffer.finish_pass()
    }

    fn applied(result: LyricsPassResult) -> Vec<LyricLine> {
        match result {
            LyricsPassResult::Applied(lines) => lines,
            other => panic!("expected applied lyrics, got {other:?}"),
        }
    }

    #[test]
    fn leading_line_insertion_keeps_existing_line_owners() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let original_ids = buffer.identity_snapshot().line_ids;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..0);
            TextBuffer::insert_text(buffer, "X\n", 0);
        }));

        assert_eq!(
            edited,
            vec![line("X", None), line("A", Some(1.0)), line("B", Some(2.0))]
        );
        assert_eq!(buffer.lines[1].id, original_ids[0]);
        assert_eq!(buffer.lines[2].id, original_ids[1]);
    }

    #[test]
    fn unicode_char_ranges_and_interior_split_keep_the_first_fragment() {
        let original = vec![line("あ🙂い", Some(3.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let original_id = buffer.lines[0].id;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 2..2);
            TextBuffer::insert_text(buffer, "\n", 2);
        }));

        assert_eq!(edited, vec![line("あ🙂", Some(3.0)), line("い", None)]);
        assert_eq!(buffer.lines[0].id, original_id);
    }

    #[test]
    fn whole_body_replacement_in_one_pass_keeps_identity_and_time() {
        let original = vec![line("old", Some(4.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let original_id = buffer.lines[0].id;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..3);
            TextBuffer::insert_text(buffer, "new", 0);
        }));

        assert_eq!(edited, vec![line("new", Some(4.0))]);
        assert_eq!(buffer.lines[0].id, original_id);
    }

    #[test]
    fn deleting_only_a_timed_body_keeps_one_empty_cue_line() {
        let original = vec![line("cue", Some(5.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..3);
        }));

        assert_eq!(edited, vec![line("", Some(5.0))]);
        assert_eq!(buffer.as_str(), "");
        assert_eq!(buffer.lines.len(), 1);
    }

    #[test]
    fn deleting_a_complete_line_removes_its_owner_and_time() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let deleted_id = buffer.lines[0].id;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..2);
        }));

        assert_eq!(edited, vec![line("B", Some(2.0))]);
        assert!(!buffer.lines.iter().any(|line| line.id == deleted_id));
        assert_eq!(buffer.tombstones.get(&deleted_id), Some(&Some(1.0)));
    }

    #[test]
    fn deleting_complete_leading_bodies_keeps_an_undeleted_separator_and_later_splices_valid() {
        let original = vec![
            line("A", Some(1.0)),
            line("B", Some(2.0)),
            line("C", Some(3.0)),
        ];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let c_id = buffer.lines[2].id;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..3);
            assert_eq!(buffer.as_str(), "\nC");
            TextBuffer::insert_text(buffer, "X", 2);
        }));

        assert_eq!(edited, vec![line("", None), line("CX", Some(3.0))]);
        assert_eq!(buffer.as_str(), "\nCX");
        assert_eq!(buffer.lines[1].id, c_id);
    }

    #[test]
    fn joining_one_timed_and_one_untimed_line_keeps_the_timed_owner() {
        let original = vec![line("A", None), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let timed_id = buffer.lines[1].id;

        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 1..2);
        }));

        assert_eq!(edited, vec![line("AB", Some(2.0))]);
        assert_eq!(buffer.lines[0].id, timed_id);
    }

    #[test]
    fn joining_different_timed_lines_waits_and_uses_latest_time_on_resolution() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let second_id = buffer.lines[1].id;

        let result = edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 1..2);
        });
        assert!(matches!(result, LyricsPassResult::NeedsResolution(_)));
        assert_eq!(buffer.committed_lines(), original);
        assert_eq!(buffer.as_str(), "AB");
        assert!(!buffer.is_mutable());

        let latest = vec![line("A", Some(1.0)), line("B", Some(9.0))];
        let resolved = buffer
            .resolve_pending(&latest, LyricsResolutionChoice::KeepTimedLine(second_id))
            .unwrap();
        assert_eq!(applied(resolved), vec![line("AB", Some(9.0))]);
    }

    #[test]
    fn joining_equal_timed_lines_needs_no_confirmation() {
        let original = vec![line("A", Some(1.0)), line("B", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 1..2);
        }));
        assert_eq!(edited, vec![line("AB", Some(1.0))]);
    }

    #[test]
    fn native_restore_keeps_current_times_for_surviving_ids() {
        let original = vec![line("A", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..1);
            TextBuffer::insert_text(buffer, "B", 0);
        }));
        let latest = vec![line("B", Some(8.0))];
        assert_eq!(edited, vec![line("B", Some(1.0))]);

        let restored = applied(edit(&mut buffer, &latest, |buffer| {
            TextBuffer::replace_with(buffer, "A");
        }));
        assert_eq!(restored, vec![line("A", Some(8.0))]);
    }

    #[test]
    fn native_restore_uses_tombstone_time_for_a_deleted_line() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let edited = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..2);
        }));
        let latest = vec![line("B", Some(7.0))];
        assert_eq!(edited, vec![line("B", Some(2.0))]);

        let restored = applied(edit(&mut buffer, &latest, |buffer| {
            TextBuffer::replace_with(buffer, "A\nB");
        }));
        assert_eq!(restored, vec![line("A", Some(1.0)), line("B", Some(7.0))]);
    }

    #[test]
    fn duplicate_native_text_with_different_identities_requires_a_choice() {
        let original = vec![line("same", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let first_id = buffer.lines[0].id;
        buffer.replace_external(&[line("same", Some(2.0))]);
        let second_id = buffer.lines[0].id;
        assert_ne!(first_id, second_id);

        let result = edit(&mut buffer, &[line("same", Some(2.0))], |buffer| {
            TextBuffer::replace_with(buffer, "same")
        });
        let LyricsPassResult::NeedsResolution(conflict) = result else {
            panic!("native identity collision must be explicit")
        };
        let LyricsEditConflictKind::AmbiguousNativeHistory { choices } = conflict.kind else {
            panic!("expected native ambiguity")
        };
        assert_eq!(choices.len(), 2);
    }

    #[test]
    fn duplicate_native_text_with_the_same_identity_is_unambiguous() {
        let original = vec![line("A", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let b = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..1);
            TextBuffer::insert_text(buffer, "B", 0);
        }));
        let a = applied(edit(&mut buffer, &b, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..1);
            TextBuffer::insert_text(buffer, "A", 0);
        }));
        let c = applied(edit(&mut buffer, &a, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..1);
            TextBuffer::insert_text(buffer, "C", 0);
        }));

        let restored = edit(&mut buffer, &c, |buffer| {
            TextBuffer::replace_with(buffer, "A");
        });
        assert!(matches!(restored, LyricsPassResult::Applied(_)));
    }

    #[test]
    fn an_evicted_native_target_is_never_guessed() {
        let original = vec![line("initial", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let mut project = original;
        for index in 0..=NATIVE_SIDECAR_LIMIT {
            let next = format!("revision-{index}");
            project = applied(edit(&mut buffer, &project, |buffer| {
                let len = buffer.as_str().chars().count();
                TextBuffer::delete_char_range(buffer, 0..len);
                TextBuffer::insert_text(buffer, &next, 0);
            }));
        }

        let result = edit(&mut buffer, &project, |buffer| {
            TextBuffer::replace_with(buffer, "initial");
        });
        assert!(matches!(
            result,
            LyricsPassResult::NeedsResolution(LyricsEditConflict {
                kind: LyricsEditConflictKind::UnrecordedNativeHistory { .. },
                ..
            })
        ));
    }

    #[test]
    fn a_remaining_same_text_candidate_is_not_trusted_after_an_older_identity_was_evicted() {
        let original = vec![line("same", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let mut project = original;
        for index in 0..=NATIVE_SIDECAR_LIMIT {
            let next = format!("revision-{index}");
            project = applied(edit(&mut buffer, &project, |buffer| {
                let len = buffer.as_str().chars().count();
                TextBuffer::delete_char_range(buffer, 0..len);
                TextBuffer::insert_text(buffer, &next, 0);
            }));
        }
        buffer.replace_external(&[line("same", Some(9.0))]);

        let result = edit(&mut buffer, &[line("same", Some(9.0))], |buffer| {
            TextBuffer::replace_with(buffer, "same");
        });

        assert!(matches!(
            result,
            LyricsPassResult::NeedsResolution(LyricsEditConflict {
                kind: LyricsEditConflictKind::UnrecordedNativeHistory { .. },
                ..
            })
        ));
    }

    #[test]
    fn reachable_native_history_garbage_collects_eviction_uncertainty() {
        let original = vec![line("initial", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let context = Context::default();
        let id = buffer.widget_id(LayerId(1));
        let cursor = CCursorRange::one(CCursor::new(original[0].text.chars().count()));
        let mut text_state = TextEditState::default();
        text_state.cursor.set_char_range(Some(cursor));
        let mut undoer = NativeTextUndoer::default();
        undoer.add_undo(&(cursor, original[0].text.clone()));
        text_state.set_undoer(undoer);
        text_state.store(&context, id);

        let mut project = original;
        for index in 0..(NATIVE_SIDECAR_LIMIT + 64) {
            let rollback = TextEditRollback::capture(&context, id, buffer.as_str()).unwrap();
            buffer.begin_pass(&project, Some(rollback)).unwrap();
            let len = buffer.as_str().chars().count();
            TextBuffer::delete_char_range(&mut buffer, 0..len);
            TextBuffer::insert_text(&mut buffer, &format!("revision-{index}"), 0);
            project = applied(buffer.finish_pass());

            let mut state = TextEditState::load(&context, id).unwrap();
            let cursor = CCursorRange::one(CCursor::new(buffer.as_str().chars().count()));
            state.cursor.set_char_range(Some(cursor));
            let mut undoer = state.undoer();
            undoer.add_undo(&(cursor, buffer.as_str().to_owned()));
            state.set_undoer(undoer);
            state.store(&context, id);
        }

        assert!(!buffer.all_native_targets_incomplete);
        assert!(buffer.evicted_text_hashes.len() <= 100);
        let previous = format!("revision-{}", NATIVE_SIDECAR_LIMIT + 62);
        let restored = edit(&mut buffer, &project, |buffer| {
            TextBuffer::replace_with(buffer, &previous);
        });
        assert!(matches!(restored, LyricsPassResult::Applied(_)));
    }

    #[test]
    fn native_reachability_enumerates_both_undo_and_redo_branches() {
        let cursor = CCursorRange::one(CCursor::new(1));
        let a = (cursor, "A".to_owned());
        let b = (cursor, "B".to_owned());
        let c = (cursor, "C".to_owned());
        let mut undoer = NativeTextUndoer::default();
        undoer.add_undo(&a);
        undoer.add_undo(&b);
        undoer.add_undo(&c);
        assert_eq!(undoer.undo(&c), Some(&b));

        let mut state = TextEditState::default();
        state.cursor.set_char_range(Some(cursor));
        let NativeHistoryReachability::Known(reachable) =
            native_history_reachability(&state, &undoer, "B")
        else {
            panic!("bounded egui history should be enumerable")
        };

        assert_eq!(
            reachable,
            HashSet::from([text_hash("A"), text_hash("B"), text_hash("C")])
        );
    }

    #[test]
    fn native_reachability_keeps_redo_when_the_saved_cursor_needs_clamping() {
        let valid_cursor = CCursorRange::one(CCursor::new(1));
        let stale_cursor = CCursorRange::one(CCursor::new(100));
        let a = (valid_cursor, "A".to_owned());
        let b = (valid_cursor, "B".to_owned());
        let mut undoer = NativeTextUndoer::default();
        undoer.add_undo(&a);
        undoer.add_undo(&b);
        assert_eq!(undoer.undo(&b), Some(&a));

        let mut state = TextEditState::default();
        state.cursor.set_char_range(Some(stale_cursor));
        let NativeHistoryReachability::Known(reachable) =
            native_history_reachability(&state, &undoer, "A")
        else {
            panic!("bounded egui history should be enumerable")
        };

        assert!(reachable.contains(&text_hash("A")));
        assert!(reachable.contains(&text_hash("B")));
    }

    #[test]
    fn native_checkpoint_survives_many_unsaved_intermediate_input_passes() {
        let original = vec![line("保存点", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let context = Context::default();
        let id = buffer.widget_id(LayerId(1));
        let cursor = CCursorRange::one(CCursor::new(original[0].text.chars().count()));
        let mut state = TextEditState::default();
        state.cursor.set_char_range(Some(cursor));
        let mut undoer = NativeTextUndoer::default();
        undoer.add_undo(&(cursor, original[0].text.clone()));
        state.set_undoer(undoer);
        state.store(&context, id);

        let mut project = original;
        for index in 0..(NATIVE_SIDECAR_LIMIT + 64) {
            let rollback = TextEditRollback::capture(&context, id, buffer.as_str()).unwrap();
            buffer.begin_pass(&project, Some(rollback)).unwrap();
            let len = buffer.as_str().chars().count();
            TextBuffer::delete_char_range(&mut buffer, 0..len);
            TextBuffer::insert_text(&mut buffer, &format!("入力中-{index}"), 0);
            project = applied(buffer.finish_pass());

            let mut state = TextEditState::load(&context, id).unwrap();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(
                    buffer.as_str().chars().count(),
                ))));
            state.store(&context, id);
        }

        assert!(buffer.sidecar.iter().any(|revision| {
            revision.text == "保存点" && revision.line_ids == buffer.identity_snapshot().line_ids
        }));
        let restored = edit(&mut buffer, &project, |buffer| {
            TextBuffer::replace_with(buffer, "保存点");
        });
        assert_eq!(applied(restored), vec![line("保存点", Some(1.0))]);
    }

    #[test]
    fn an_unreachable_eviction_marker_does_not_poison_a_later_same_text_identity() {
        let original = vec![line("same", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let original_id = buffer.lines[0].id;
        let mut project = original;
        for index in 0..=NATIVE_SIDECAR_LIMIT {
            let next = format!("revision-{index}");
            project = applied(edit(&mut buffer, &project, |buffer| {
                let len = buffer.as_str().chars().count();
                TextBuffer::delete_char_range(buffer, 0..len);
                TextBuffer::insert_text(buffer, &next, 0);
            }));
        }
        assert!(buffer.evicted_text_hashes.contains(&text_hash("same")));

        buffer.observe_native_reachability(&NativeHistoryReachability::Known(HashSet::from([
            text_hash(buffer.as_str()),
        ])));
        assert!(!buffer.evicted_text_hashes.contains(&text_hash("same")));

        buffer.replace_external(&[line("same", Some(9.0))]);
        let replacement_id = buffer.lines[0].id;
        assert_ne!(replacement_id, original_id);
        let result = edit(&mut buffer, &[line("same", Some(9.0))], |buffer| {
            TextBuffer::replace_with(buffer, "same");
        });
        assert!(matches!(result, LyricsPassResult::Unchanged));
        assert_eq!(buffer.lines[0].id, replacement_id);
    }

    #[test]
    fn a_decision_for_a_later_splice_passes_an_earlier_unique_native_restore() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let second_id = buffer.lines[1].id;
        let changed = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..1);
            TextBuffer::insert_text(buffer, "X", 0);
        }));

        let result = edit(&mut buffer, &changed, |buffer| {
            TextBuffer::replace_with(buffer, "A\nB");
            TextBuffer::delete_char_range(buffer, 1..2);
        });
        assert!(matches!(result, LyricsPassResult::NeedsResolution(_)));

        let resolved = buffer
            .resolve_pending(&changed, LyricsResolutionChoice::KeepTimedLine(second_id))
            .unwrap();
        assert_eq!(applied(resolved), vec![line("AB", Some(2.0))]);
    }

    #[test]
    fn same_pass_native_redo_never_chooses_an_older_same_text_identity() {
        let original = vec![line("同じ", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let old_id = buffer.lines[0].id;
        buffer.replace_external(&[line("現在", Some(1.0))]);
        let current_id = buffer.lines[0].id;
        assert_ne!(old_id, current_id);

        let result = edit(&mut buffer, &[line("現在", Some(1.0))], |buffer| {
            let len = buffer.as_str().chars().count();
            TextBuffer::delete_char_range(buffer, 0..len);
            TextBuffer::insert_text(buffer, "同じ", 0);
            TextBuffer::replace_with(buffer, "現在");
            TextBuffer::replace_with(buffer, "同じ");
        });
        let LyricsPassResult::NeedsResolution(conflict) = result else {
            panic!("the pass-local and older identities must remain explicit")
        };
        let LyricsEditConflictKind::AmbiguousNativeHistory { choices } = conflict.kind else {
            panic!("expected native history ambiguity")
        };
        assert!(
            choices
                .iter()
                .any(|choice| { choice.lines == vec![line("同じ", Some(1.0))] })
        );
        assert!(
            choices
                .iter()
                .any(|choice| { choice.lines == vec![line("同じ", Some(2.0))] })
        );
        let current_choice = choices
            .iter()
            .find(|choice| choice.lines == vec![line("同じ", Some(1.0))])
            .unwrap()
            .candidate_id;

        let resolved = buffer
            .resolve_pending(
                &[line("現在", Some(1.0))],
                LyricsResolutionChoice::UseNativeCandidate(current_choice),
            )
            .unwrap();
        assert_eq!(applied(resolved), vec![line("同じ", Some(1.0))]);
        assert_eq!(buffer.lines[0].id, current_id);
    }

    #[test]
    fn unrecorded_empty_native_target_requires_explicit_zero_or_one_line_meaning() {
        let original = vec![line("A", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let result = edit(&mut buffer, &original, |buffer| {
            buffer.sidecar.clear();
            TextBuffer::replace_with(buffer, "");
        });
        assert!(matches!(result, LyricsPassResult::NeedsResolution(_)));

        let result = buffer
            .resolve_pending(
                &original,
                LyricsResolutionChoice::ApplyAsNewUntimed(EmptyTextMeaning::OneUntimedLine),
            )
            .unwrap();
        assert_eq!(applied(result), vec![line("", None)]);
    }

    #[test]
    fn global_snapshot_restores_identity_without_discarding_native_sidecar() {
        let original = vec![line("A", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 7);
        let snapshot = buffer.identity_snapshot();
        let original_sidecar_len = buffer.sidecar.len();
        buffer.replace_external(&[line("B", Some(2.0))]);
        let sidecar_after_external = buffer.sidecar.len();

        buffer
            .restore_global_snapshot(&original, &snapshot)
            .unwrap();

        assert_eq!(buffer.identity_snapshot(), snapshot);
        assert!(sidecar_after_external > original_sidecar_len);
        assert!(buffer.sidecar.len() >= sidecar_after_external);
        assert_eq!(buffer.widget_generation, 7);
    }

    #[test]
    fn widget_id_separates_session_or_layer_lifetimes() {
        let lines = vec![line("A", None)];
        let first = LyricsTextBuffer::new(&lines, 1);
        let next_session = LyricsTextBuffer::new(&lines, 2);
        assert_ne!(
            first.widget_id(LayerId(10)),
            next_session.widget_id(LayerId(10))
        );
        assert_ne!(first.widget_id(LayerId(10)), first.widget_id(LayerId(11)));
    }

    #[test]
    fn cancelling_pending_native_restore_can_restore_cursor_and_undoer() {
        let context = Context::default();
        let id = Id::new("lyrics-native-rollback");
        let mut native_state = TextEditState::default();
        let old_cursor = CCursorRange::one(CCursor::new(1));
        native_state.cursor.set_char_range(Some(old_cursor));
        let mut undoer = NativeTextUndoer::default();
        undoer.add_undo(&(old_cursor, "A".to_owned()));
        native_state.set_undoer(undoer);
        native_state.store(&context, id);

        let rollback = TextEditRollback::capture(&context, id, "A").unwrap();
        let original = vec![line("A", Some(1.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        buffer.sidecar.clear();
        buffer.begin_pass(&original, Some(rollback)).unwrap();
        TextBuffer::replace_with(&mut buffer, "unknown");
        assert!(matches!(
            buffer.finish_pass(),
            LyricsPassResult::NeedsResolution(_)
        ));

        let mut advanced = TextEditState::load(&context, id).unwrap();
        advanced
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(7))));
        advanced.clear_undoer();
        advanced.store(&context, id);

        let cancelled = buffer.cancel_pending().unwrap();
        cancelled.restore_text_edit(&context, id);
        let restored = TextEditState::load(&context, id).unwrap();
        assert_eq!(restored.cursor.char_range(), Some(old_cursor));
        assert!(
            restored
                .undoer()
                .has_undo(&(old_cursor, "changed".to_owned()))
        );
        assert_eq!(buffer.as_str(), "A");
    }

    #[test]
    fn cancelling_pending_keeps_uncertainty_for_the_restored_native_history() {
        let original = vec![line("戻る本文", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let mut project = original;
        for index in 0..=NATIVE_SIDECAR_LIMIT {
            let next = format!("revision-{index}");
            project = applied(edit(&mut buffer, &project, |buffer| {
                let len = buffer.as_str().chars().count();
                TextBuffer::delete_char_range(buffer, 0..len);
                TextBuffer::insert_text(buffer, &next, 0);
            }));
        }
        assert!(buffer.evicted_text_hashes.contains(&text_hash("戻る本文")));

        buffer.replace_external(&[line("戻る本文", Some(9.0))]);
        let current = vec![line("A", Some(1.0)), line("B", Some(3.0))];
        buffer.replace_external(&current);
        buffer.observe_native_reachability(&NativeHistoryReachability::Known(HashSet::from([
            text_hash("A\nB"),
            text_hash("戻る本文"),
        ])));
        buffer.begin_pass(&current, None).unwrap();
        TextBuffer::delete_char_range(&mut buffer, 1..2);
        buffer.ensure_pass().post_reachability =
            Some(NativeHistoryReachability::Known(HashSet::from([
                text_hash("AB"),
            ])));
        assert!(matches!(
            buffer.finish_pass(),
            LyricsPassResult::NeedsResolution(_)
        ));
        assert!(buffer.evicted_text_hashes.contains(&text_hash("戻る本文")));

        buffer.cancel_pending().unwrap();
        let native_result = edit(&mut buffer, &current, |buffer| {
            TextBuffer::replace_with(buffer, "戻る本文");
        });
        assert!(matches!(
            native_result,
            LyricsPassResult::NeedsResolution(LyricsEditConflict {
                kind: LyricsEditConflictKind::UnrecordedNativeHistory { .. },
                ..
            })
        ));
    }

    #[test]
    fn tombstones_are_released_when_no_bounded_revision_references_them() {
        let original = vec![line("A", Some(1.0)), line("B", Some(2.0))];
        let mut buffer = LyricsTextBuffer::new(&original, 1);
        let deleted_id = buffer.lines[0].id;
        let mut project = applied(edit(&mut buffer, &original, |buffer| {
            TextBuffer::delete_char_range(buffer, 0..2);
        }));
        assert!(buffer.tombstones.contains_key(&deleted_id));

        for index in 0..=NATIVE_SIDECAR_LIMIT {
            let next = format!("B-{index}");
            project = applied(edit(&mut buffer, &project, |buffer| {
                let len = buffer.as_str().chars().count();
                TextBuffer::delete_char_range(buffer, 0..len);
                TextBuffer::insert_text(buffer, &next, 0);
            }));
        }
        assert!(!buffer.tombstones.contains_key(&deleted_id));
        assert!(buffer.sidecar.len() <= NATIVE_SIDECAR_LIMIT);
    }
}
