//! Worker/pure adoption boundary for one direct book page block.
//! No service writes, UI state, automatic pairing rules or filesystem reads live here.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::*;
use crate::{
    book_resume_db::{PageIdentity, ReadingPosition},
    book_scope::BookScopeKey,
};

pub(crate) const UNSUPPORTED_COMPOSITION_NOTICE: &str =
    "この本のページ名では本の構成を使えません。構成を使わずに開きます。";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalRawPage {
    pub(crate) page: CompositionPageRef,
    /// Original canonical column slot, including child-container rows.
    pub(crate) raw_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AcceptedCompositionProjection {
    pub(crate) scope: BookScopeKey,
    pub(crate) token: CompositionToken,
    pub(crate) cover_enabled: bool,
    pub(crate) resolved: ResolvedComposition,
    /// Worker-prepared transient geometry inputs; never part of the saved record.
    pub(crate) prepared_rotations: Arc<HashMap<String, crate::rotation_db::Rotation>>,
    pub(crate) raw_pages: Arc<[CanonicalRawPage]>,
    raw_by_index: Arc<HashMap<usize, CompositionPageRef>>,
    projected_positions: Arc<HashMap<usize, ReadingPosition>>,
    ordinals: Arc<HashMap<usize, usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CompositionProjectionOutcome {
    Automatic,
    Composed(Arc<AcceptedCompositionProjection>),
    /// Legacy viewing continues. A reason is returned for the adoption owner's log/notice.
    Unsupported {
        reason: String,
    },
    Unreadable {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedCompositionProjection {
    pub(crate) output_to_input: Vec<usize>,
    pub(crate) outcome: CompositionProjectionOutcome,
}

/// Unsafe/ambiguous source identities refuse composition only. Nothing is deduplicated,
/// normalized into a replacement page, dropped from legacy rows, or written back.
pub(crate) fn project_composition(
    scope: BookScopeKey,
    domain: &PageDomain,
    token: CompositionToken,
    record: Option<&BookComposition>,
    raw_pages: &[CanonicalRawPage],
    manifest: &[ManifestPage],
    row_count: usize,
) -> CompositionResult<PreparedCompositionProjection> {
    let passthrough = |outcome| PreparedCompositionProjection {
        output_to_input: (0..row_count).collect(),
        outcome,
    };
    if let Err(reason) = validate_raw_manifest(&scope, domain, raw_pages, manifest) {
        return Ok(passthrough(CompositionProjectionOutcome::Unsupported {
            reason: reason.to_string(),
        }));
    }
    let Some(record) = record else {
        return Ok(passthrough(CompositionProjectionOutcome::Automatic));
    };
    let resolved = match resolve_composition(record, domain, manifest) {
        Ok(resolved) => resolved,
        Err(error) => {
            return Ok(passthrough(CompositionProjectionOutcome::Unreadable {
                reason: error.to_string(),
            }));
        }
    };
    let page_slots: HashSet<_> = manifest
        .iter()
        .filter_map(|entry| match entry.availability {
            ManifestAvailability::Present { item_index } => Some(item_index),
            _ => None,
        })
        .collect();
    if page_slots.iter().any(|slot| *slot >= row_count) {
        return Err(CompositionError::Invalid(
            "manifest index is outside materialized rows",
        ));
    }
    let first = page_slots.iter().min().copied();
    if let Some(first) = first {
        let last = *page_slots.iter().max().unwrap();
        if last - first + 1 != page_slots.len() {
            return Err(CompositionError::Invalid(
                "direct book pages are not one row block",
            ));
        }
    }
    let mut output_to_input = Vec::with_capacity(row_count);
    for old in 0..row_count {
        if Some(old) == first {
            output_to_input.extend(resolved.visible_pages.iter().map(|page| page.item_index));
        }
        if !page_slots.contains(&old) {
            output_to_input.push(old);
        }
    }
    let mut old_to_new = vec![None; row_count];
    for (new, old) in output_to_input.iter().copied().enumerate() {
        old_to_new[old] = Some(new);
    }
    let resolved = resolved.reindex_items(&old_to_new)?;
    Ok(PreparedCompositionProjection {
        output_to_input,
        outcome: CompositionProjectionOutcome::Composed(Arc::new(
            AcceptedCompositionProjection::new(
                scope,
                token,
                resolved,
                raw_pages.to_vec().into(),
                record.cover_enabled,
            ),
        )),
    })
}

fn validate_raw_manifest(
    scope: &BookScopeKey,
    domain: &PageDomain,
    raw_pages: &[CanonicalRawPage],
    manifest: &[ManifestPage],
) -> CompositionResult<()> {
    super::store::validate_scope_domain(scope, domain).map_err(CompositionError::Json)?;
    if raw_pages.len() > MAX_PAGES {
        return Err(CompositionError::Limit("manifest pages"));
    }
    let mut identities = HashSet::new();
    let mut slots = HashSet::new();
    for raw in raw_pages {
        domain.validate_page(&raw.page)?;
        match &raw.page {
            CompositionPageRef::RelativePath { path } => {
                crate::book_scope::normalize_relative_reference(path)
                    .map_err(CompositionError::Json)?;
            }
            CompositionPageRef::ArchiveEntry { entry_name } => {
                crate::book_scope::normalize_relative_reference(entry_name)
                    .map_err(CompositionError::Json)?;
            }
            CompositionPageRef::PdfPage { .. } => {}
        }
        if !identities.insert(raw.page.matching_key()) {
            return Err(CompositionError::Invalid(
                "ambiguous normalized raw page identity",
            ));
        }
        if !slots.insert(raw.raw_index) {
            return Err(CompositionError::Invalid("duplicate canonical raw slot"));
        }
    }
    let mut members = HashSet::new();
    let mut present_slots = HashSet::new();
    for entry in manifest {
        domain.validate_page(&entry.page)?;
        let key = entry.page.matching_key();
        if !identities.contains(&key) || !members.insert(key) {
            return Err(CompositionError::Invalid(
                "manifest disagrees with raw membership",
            ));
        }
        if let ManifestAvailability::Present { item_index } = entry.availability
            && !present_slots.insert(item_index)
        {
            return Err(CompositionError::Invalid("duplicate manifest row slot"));
        }
    }
    if members.len() != identities.len() {
        return Err(CompositionError::Invalid("manifest loses raw membership"));
    }
    Ok(())
}

impl AcceptedCompositionProjection {
    fn new(
        scope: BookScopeKey,
        token: CompositionToken,
        resolved: ResolvedComposition,
        raw_pages: Arc<[CanonicalRawPage]>,
        cover_enabled: bool,
    ) -> Self {
        let raw_by_index = raw_pages
            .iter()
            .map(|raw| (raw.raw_index, raw.page.clone()))
            .collect();
        let raw_by_identity: HashMap<_, _> = raw_pages
            .iter()
            .map(|raw| (raw.page.matching_key(), raw.raw_index))
            .collect();
        let projected_positions = resolved
            .visible_pages
            .iter()
            .filter_map(|page| {
                Some((
                    page.item_index,
                    ReadingPosition {
                        page: *raw_by_identity.get(&page.page.matching_key())?,
                        identity: Some(page.page.resume_identity(&scope.logical_root)),
                    },
                ))
            })
            .collect();
        let ordinals = resolved
            .visible_pages
            .iter()
            .enumerate()
            .map(|(ordinal, page)| (page.item_index, ordinal + 1))
            .collect();
        Self {
            scope,
            token,
            cover_enabled,
            resolved,
            prepared_rotations: Default::default(),
            raw_pages,
            raw_by_index: Arc::new(raw_by_index),
            projected_positions: Arc::new(projected_positions),
            ordinals: Arc::new(ordinals),
        }
    }

    pub(crate) fn canonical_raw_index_for_projected(&self, item_index: usize) -> Option<usize> {
        self.projected_positions
            .get(&item_index)
            .map(|position| position.page)
    }

    pub(crate) fn position_for_projected(&self, item_index: usize) -> Option<ReadingPosition> {
        self.projected_positions.get(&item_index).cloned()
    }

    /// Identity always wins. Legacy numbers address the original raw column only.
    pub(crate) fn resolve_position(&self, position: &ReadingPosition) -> PageStatus {
        let page = if let Some(identity) = &position.identity {
            match composition_page_from_identity(&self.scope, identity) {
                Some(page) => page,
                None => return PageStatus::Invalid,
            }
        } else {
            match self.raw_by_index.get(&position.page) {
                Some(page) => page.clone(),
                None => return PageStatus::Invalid,
            }
        };
        self.resolved.page_status(&page)
    }

    pub(crate) fn reading_ordinal(&self, item_index: usize) -> Option<usize> {
        self.ordinals.get(&item_index).copied()
    }

    /// Physical deletion preserves saved refs/token. Canonical raw slots lose the same
    /// physical pages, including ones absent from the current policy/facet presentation.
    pub(crate) fn remap_removed_items(
        &self,
        old_to_new: &[Option<usize>],
        removed_pages: &[CompositionPageRef],
    ) -> CompositionResult<Self> {
        let resolved = self
            .resolved
            .remap_removed_items(old_to_new, removed_pages)?;
        let removed: HashSet<_> = removed_pages
            .iter()
            .map(CompositionPageRef::matching_key)
            .collect();
        let mut removed_raw: Vec<_> = self
            .raw_pages
            .iter()
            .filter(|raw| removed.contains(&raw.page.matching_key()))
            .map(|raw| raw.raw_index)
            .collect();
        removed_raw.sort_unstable();
        let raw_pages = self
            .raw_pages
            .iter()
            .filter(|raw| !removed.contains(&raw.page.matching_key()))
            .map(|raw| CanonicalRawPage {
                page: raw.page.clone(),
                raw_index: raw.raw_index
                    - removed_raw.partition_point(|slot| *slot < raw.raw_index),
            })
            .collect::<Vec<_>>()
            .into();
        let mut remapped = Self::new(
            self.scope.clone(),
            self.token,
            resolved,
            raw_pages,
            self.cover_enabled,
        );
        // Geometry inputs are keyed by stable page identity, not dense slots.
        remapped.prepared_rotations = Arc::clone(&self.prepared_rotations);
        Ok(remapped)
    }
}

pub(crate) fn composition_page_from_identity(
    scope: &BookScopeKey,
    identity: &PageIdentity,
) -> Option<CompositionPageRef> {
    use crate::spread_shift::SpreadShiftPageRef;
    match SpreadShiftPageRef::from_identity(scope, identity).ok()? {
        SpreadShiftPageRef::RelativePath(path) => Some(CompositionPageRef::RelativePath { path }),
        SpreadShiftPageRef::ArchiveEntry(entry_name) => {
            Some(CompositionPageRef::ArchiveEntry { entry_name })
        }
        SpreadShiftPageRef::DocumentPageIdentity(page_num) => {
            Some(CompositionPageRef::PdfPage { page_num })
        }
    }
}

/// Capture the resolved reading binding and complete raw order. Pairing and
/// presentation rules remain live inputs and are never frozen into this record.
pub(crate) fn capture_initial_composition(
    domain: &PageDomain,
    direction: ReadingDirection,
    cover_enabled: bool,
    pages: &[CompositionPageRef],
) -> CompositionResult<BookComposition> {
    let mut record = BookComposition::new(direction, pages.to_vec());
    record.cover_enabled = cover_enabled;
    record.validate(domain)?;
    Ok(record)
}
