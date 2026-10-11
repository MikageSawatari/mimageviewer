//! In-memory composition editing. Source preparation, modal admission and writes
//! belong to the application; this owner has no UI, database or filesystem access.
use std::collections::{HashMap, HashSet};

use super::*;

const UNDO_LIMIT: usize = 32;

/// Only ordinary real occurrences or their containing unit can be selected.
/// A blank or a supplementary cover has no independent selection identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DraftSelection {
    Page(CompositionPageRef),
    Unit(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DraftConfirmation {
    CoverChanged,
    PairBroken {
        first: CompositionPageRef,
        second: CompositionPageRef,
    },
    CompanionRemoved {
        page: CompositionPageRef,
    },
    RebuildAutomatic,
}

#[derive(Clone, Debug)]
struct DraftCheckpoint {
    record: BookComposition,
    selection: Option<DraftSelection>,
}

/// Immutable candidate for a visible preview. Applying a stale candidate fails;
/// requiring confirmation does not mutate the draft or allocate an undo step.
#[derive(Clone, Debug)]
pub(crate) struct DraftEditPreview {
    generation: u64,
    baseline: BookComposition,
    candidate: DraftCheckpoint,
    confirmations: Vec<DraftConfirmation>,
}

impl DraftEditPreview {
    pub(crate) fn record(&self) -> &BookComposition {
        &self.candidate.record
    }

    pub(crate) fn confirmations(&self) -> &[DraftConfirmation] {
        &self.confirmations
    }

    pub(crate) fn requires_confirmation(&self) -> bool {
        !self.confirmations.is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompositionDraft {
    domain: PageDomain,
    /// Complete raw membership, including policy-excluded pages. Missing saved
    /// references remain in the record and do not need a manifest occurrence.
    manifest: Vec<ManifestPage>,
    original: BookComposition,
    current: DraftCheckpoint,
    undo: Vec<DraftCheckpoint>,
    redo: Vec<DraftCheckpoint>,
    generation: u64,
}

impl CompositionDraft {
    /// `record` is either the saved manual topology or a once-frozen automatic
    /// topology. Newly discovered raw pages append as centered Singles; opening
    /// an editor never re-pairs existing manual units or discards Missing refs.
    pub(crate) fn new(
        mut record: BookComposition,
        domain: PageDomain,
        manifest: Vec<ManifestPage>,
    ) -> CompositionResult<Self> {
        record.validate(&domain)?;
        // The resolver also checks duplicate normalized manifest identities and
        // item slots. It is not used to reconstruct the editable topology.
        resolve_composition(&record, &domain, &manifest)?;
        let original = record.clone();
        let mut registered: HashSet<_> = record.pages().map(|page| page.matching_key()).collect();
        for entry in &manifest {
            if registered.insert(entry.page.matching_key()) {
                record.units.push(CompositionUnit::Single {
                    page: entry.page.clone(),
                    placement: SinglePlacement::Center,
                    companion: SingleCompanion::Vacant,
                });
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
            .map(|selected| self.canonical_selection(selected))
            .transpose()?;
        if self.current.selection != selection {
            self.current.selection = selection;
            self.generation = self.generation.wrapping_add(1);
        }
        Ok(())
    }

    fn canonical_selection(&self, selected: DraftSelection) -> CompositionResult<DraftSelection> {
        match selected {
            DraftSelection::Page(page) => self
                .record()
                .pages()
                .find(|stored| stored.matching_key() == page.matching_key())
                .cloned()
                .map(DraftSelection::Page)
                .ok_or(CompositionError::Invalid("selected page is not registered")),
            DraftSelection::Unit(index) if index < self.record().units.len() => {
                Ok(DraftSelection::Unit(index))
            }
            DraftSelection::Unit(_) => {
                Err(CompositionError::Invalid("selected unit is out of range"))
            }
        }
    }

    pub(crate) fn preview(&self, edit: UnitEdit) -> CompositionResult<DraftEditPreview> {
        self.preview_edits(std::iter::once(edit))
    }

    /// Several explicit unit edits form one candidate and one undo step. Any
    /// invalid intermediate edit rejects the whole operation without mutation.
    pub(crate) fn preview_edits(
        &self,
        edits: impl IntoIterator<Item = UnitEdit>,
    ) -> CompositionResult<DraftEditPreview> {
        let mut record = self.record().clone();
        for edit in edits {
            record = record.edited(&self.domain, edit)?;
        }
        Ok(self.make_preview(record, false))
    }

    /// The caller builds automatic units from explicitly chosen editor conditions,
    /// never global defaults. Membership, visibility, direction, cover and store
    /// revision are retained by this boundary; the whole replacement is confirmed.
    pub(crate) fn preview_rebuild(
        &self,
        units: Vec<CompositionUnit>,
    ) -> CompositionResult<DraftEditPreview> {
        let mut record = self.record().clone();
        record.units = units;
        record.validate(&self.domain)?;
        let before: HashSet<_> = self
            .record()
            .pages()
            .map(|page| page.matching_key())
            .collect();
        let after: HashSet<_> = record.pages().map(|page| page.matching_key()).collect();
        if before != after {
            return Err(CompositionError::Invalid(
                "rebuild must preserve every registered real page",
            ));
        }
        Ok(self.make_preview(record, true))
    }

    fn make_preview(&self, record: BookComposition, rebuild: bool) -> DraftEditPreview {
        let mut confirmations = Vec::new();
        let pairs: HashSet<_> = record
            .units
            .iter()
            .filter_map(|unit| match unit {
                CompositionUnit::Pair { first, second } => {
                    Some((first.matching_key(), second.matching_key()))
                }
                _ => None,
            })
            .collect();
        for unit in &self.record().units {
            if let CompositionUnit::Pair { first, second } = unit
                && !pairs.contains(&(first.matching_key(), second.matching_key()))
            {
                confirmations.push(DraftConfirmation::PairBroken {
                    first: first.clone(),
                    second: second.clone(),
                });
            }
        }
        if self
            .record()
            .cover
            .as_ref()
            .map(CompositionPageRef::matching_key)
            != record.cover.as_ref().map(CompositionPageRef::matching_key)
            && self.record().cover.is_some()
        {
            confirmations.push(DraftConfirmation::CoverChanged);
        }
        let companions: HashMap<_, _> = record
            .units
            .iter()
            .filter_map(|unit| match unit {
                CompositionUnit::Single {
                    page, companion, ..
                } => Some((page.matching_key(), companion)),
                _ => None,
            })
            .collect();
        for unit in &self.record().units {
            if let CompositionUnit::Single {
                page, companion, ..
            } = unit
                && *companion != SingleCompanion::Vacant
                && companions.get(&page.matching_key()).copied() != Some(companion)
            {
                confirmations.push(DraftConfirmation::CompanionRemoved { page: page.clone() });
            }
        }
        if rebuild {
            confirmations.push(DraftConfirmation::RebuildAutomatic);
        }
        let selection = match &self.current.selection {
            Some(DraftSelection::Unit(index)) => self
                .record()
                .units
                .get(*index)
                .and_then(|unit| unit.pages().next())
                .and_then(|anchor| {
                    record.units.iter().position(|unit| {
                        unit.pages()
                            .any(|page| page.matching_key() == anchor.matching_key())
                    })
                })
                .map(DraftSelection::Unit),
            other => other.clone(),
        };
        DraftEditPreview {
            generation: self.generation,
            baseline: self.record().clone(),
            candidate: DraftCheckpoint { record, selection },
            confirmations,
        }
    }

    /// `confirmed` means the application presented the preview and received the
    /// user's explicit confirmation. Rejections leave record/history unchanged.
    pub(crate) fn apply(
        &mut self,
        preview: DraftEditPreview,
        confirmed: bool,
    ) -> CompositionResult<bool> {
        if preview.generation != self.generation || preview.baseline != *self.record() {
            return Err(CompositionError::Invalid("draft preview is stale"));
        }
        if preview.requires_confirmation() && !confirmed {
            return Err(CompositionError::Invalid(
                "draft change requires confirmation",
            ));
        }
        preview.candidate.record.validate(&self.domain)?;
        if preview.candidate.record == self.current.record {
            return Ok(false);
        }
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo
            .push(std::mem::replace(&mut self.current, preview.candidate));
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
