use std::collections::VecDeque;

use crate::EditorSnapshot;

pub(crate) const HISTORY_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSurface {
    Properties,
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinuousKind {
    Pointer,
    Text(egui::Id),
}

#[derive(Debug, Clone, PartialEq)]
struct HistoryEntry {
    snapshot: EditorSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
struct ContinuousEdit {
    before: EditorSnapshot,
    surface: EditSurface,
    kind: ContinuousKind,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct UndoHistory {
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    continuous: Option<ContinuousEdit>,
}

impl UndoHistory {
    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.continuous = None;
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.continuous.is_some()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.continuous.is_none()
    }

    pub(crate) fn undo_len(&self) -> usize {
        self.undo.len() + usize::from(self.continuous.is_some())
    }

    pub(crate) fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub(crate) fn record_atomic(&mut self, before: EditorSnapshot, after: &EditorSnapshot) {
        self.finish_continuous(after);
        if before != *after {
            self.redo.clear();
            self.push_undo(HistoryEntry { snapshot: before });
        }
    }

    /// Observes one complete draw pass for one editor surface.
    ///
    /// Pointer edits start while the primary button is down and finish on the
    /// release pass. Text edits start while egui owns keyboard focus and finish
    /// when that focus is lost. All mutations between those boundaries share
    /// the first pre-edit snapshot, so a drag or a focus session is one step.
    pub(crate) fn observe_ui_pass(
        &mut self,
        surface: EditSurface,
        before: EditorSnapshot,
        after: &EditorSnapshot,
        changed: bool,
        pointer_active: bool,
        text_focus: Option<egui::Id>,
    ) {
        if self
            .continuous
            .as_ref()
            .is_some_and(|edit| edit.surface == surface)
        {
            match self.continuous.as_ref().map(|edit| edit.kind) {
                Some(ContinuousKind::Pointer) if !pointer_active => {
                    self.finish_continuous(after);
                    return;
                }
                Some(ContinuousKind::Text(id)) if text_focus != Some(id) => {
                    // The old field lost focus before this pass's edits. End it
                    // at the pass-entry snapshot, then let a newly focused
                    // field (if any) start its own operation below.
                    self.finish_continuous(&before);
                }
                Some(_) => return,
                None => {}
            }
        } else if self.continuous.is_some() && changed {
            // Interaction moved to another editor surface in the same pass
            // (for example, a focused property TextEdit followed by a timeline
            // drag). Close the old operation at this surface's entry state so
            // the two user gestures never overwrite or merge their history.
            self.finish_continuous(&before);
        }

        if !changed {
            return;
        }
        if pointer_active || text_focus.is_some() {
            self.redo.clear();
            self.continuous = Some(ContinuousEdit {
                before,
                surface,
                kind: if pointer_active {
                    ContinuousKind::Pointer
                } else {
                    ContinuousKind::Text(text_focus.expect("text focus was checked"))
                },
            });
        } else {
            self.record_atomic(before, after);
        }
    }

    pub(crate) fn undo(&mut self, current: EditorSnapshot) -> Option<EditorSnapshot> {
        self.finish_continuous(&current);
        let entry = self.undo.pop_back()?;
        self.redo.push(HistoryEntry { snapshot: current });
        Some(entry.snapshot)
    }

    pub(crate) fn redo(&mut self, current: EditorSnapshot) -> Option<EditorSnapshot> {
        let entry = self.redo.pop()?;
        self.push_undo(HistoryEntry { snapshot: current });
        Some(entry.snapshot)
    }

    fn finish_continuous(&mut self, after: &EditorSnapshot) {
        let Some(edit) = self.continuous.take() else {
            return;
        };
        if edit.before != *after {
            self.push_undo(HistoryEntry {
                snapshot: edit.before,
            });
        }
    }

    fn push_undo(&mut self, entry: HistoryEntry) {
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(entry);
    }
}
