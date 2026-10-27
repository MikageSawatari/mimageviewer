//! Direct in-memory record editing. Derived spread results belong to the editor
//! result worker, and are regenerated from the current record and rule snapshot.
use super::*;
use std::collections::HashSet;

const UNDO_LIMIT: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DraftSelection {
    Page(CompositionPageRef),
}

#[derive(Clone, Debug)]
struct DraftCheckpoint {
    record: BookComposition,
    selection: Option<DraftSelection>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompositionDraft {
    domain: PageDomain,
    manifest: Vec<ManifestPage>,
    original: BookComposition,
    current: DraftCheckpoint,
    undo: Vec<DraftCheckpoint>,
    redo: Vec<DraftCheckpoint>,
    generation: u64,
}

impl CompositionDraft {
    pub(crate) fn new(
        mut record: BookComposition,
        domain: PageDomain,
        manifest: Vec<ManifestPage>,
    ) -> CompositionResult<Self> {
        record.validate(&domain)?;
        resolve_composition(&record, &domain, &manifest)?;
        let original = record.clone();
        let mut registered: HashSet<_> = record
            .pages()
            .map(CompositionPageRef::matching_key)
            .collect();
        for entry in &manifest {
            if registered.insert(entry.page.matching_key()) {
                record.page_order.push(entry.page.clone());
            }
        }
        record.validate(&domain)?;
        Ok(Self {
            domain,
            manifest,
            original,
            current: DraftCheckpoint {
                record,
                selection: None,
            },
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
        })
    }
    pub(crate) fn record(&self) -> &BookComposition {
        &self.current.record
    }
    pub(crate) fn domain(&self) -> &PageDomain {
        &self.domain
    }
    pub(crate) fn manifest(&self) -> &[ManifestPage] {
        &self.manifest
    }
    pub(crate) fn selection(&self) -> Option<&DraftSelection> {
        self.current.selection.as_ref()
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.current.record != self.original
    }
    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub(crate) fn select(&mut self, selection: Option<DraftSelection>) -> CompositionResult<()> {
        let selection = selection
            .map(|DraftSelection::Page(page)| {
                self.record()
                    .pages()
                    .find(|stored| stored.matching_key() == page.matching_key())
                    .cloned()
                    .map(DraftSelection::Page)
                    .ok_or(CompositionError::Invalid("selected page is not registered"))
            })
            .transpose()?;
        self.current.selection = selection;
        Ok(())
    }
    pub(crate) fn apply(&mut self, edit: CompositionEdit) -> CompositionResult<bool> {
        self.apply_edits(std::iter::once(edit))
    }
    /// An invalid edit in a batch rejects the entire action; revision is store-owned.
    pub(crate) fn apply_edits(
        &mut self,
        edits: impl IntoIterator<Item = CompositionEdit>,
    ) -> CompositionResult<bool> {
        let mut record = self.record().clone();
        for edit in edits {
            record = record.edited(&self.domain, edit)?;
        }
        if record == self.current.record {
            return Ok(false);
        }
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        let next = DraftCheckpoint {
            record,
            selection: self.current.selection.clone(),
        };
        self.undo.push(std::mem::replace(&mut self.current, next));
        self.redo.clear();
        self.generation = self.generation.wrapping_add(1);
        Ok(true)
    }
    pub(crate) fn undo(&mut self) -> bool {
        let Some(checkpoint) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.current, checkpoint));
        self.generation = self.generation.wrapping_add(1);
        true
    }
    pub(crate) fn redo(&mut self) -> bool {
        let Some(checkpoint) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push(std::mem::replace(&mut self.current, checkpoint));
        self.generation = self.generation.wrapping_add(1);
        true
    }
}
