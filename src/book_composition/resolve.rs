use std::collections::{HashMap, HashSet};

use super::*;

/// Facts from a safe raw manifest, after the source's normal duplicate removal.
/// Excluded/filtered entries must remain here to distinguish them from Missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestPage {
    pub page: CompositionPageRef,
    pub availability: ManifestAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestAvailability {
    /// Input manifest row space. Resolution preserves this space; when G3
    /// reorders rows, it must reindex the whole result in the same adoption.
    Present {
        item_index: usize,
    },
    ExcludedByPolicy,
    FilteredOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageStatus {
    Visible { item_index: usize },
    HiddenByComposition,
    ExcludedByPolicy,
    FilteredOut,
    Missing,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPage {
    pub page: CompositionPageRef,
    pub item_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedComposition {
    pub revision: u64,
    pub direction: ReadingDirection,
    pub cover_enabled: bool,
    pub page_overrides: HashMap<usize, PageOverride>,
    /// Single/Split, page counts and export use this real-only reading order.
    pub visible_pages: Vec<ResolvedPage>,
    /// Every unregistered raw page, in source order. Not automatically persisted.
    pub new_tail: Vec<CompositionPageRef>,
    /// All policy-admitted, composition-visible pages are present; facet subsets
    /// are incomplete. This does not certify a later Remote response budget.
    pub complete: bool,
    statuses: HashMap<CompositionPageRef, PageStatus>,
}

impl ResolvedComposition {
    pub fn page_override(&self, item_index: usize) -> Option<PageOverride> {
        self.page_overrides.get(&item_index).copied()
    }

    pub fn page_status(&self, page: &CompositionPageRef) -> PageStatus {
        self.statuses
            .get(&page.matching_key())
            .copied()
            .unwrap_or(PageStatus::Invalid)
    }

    /// Pure deletion transaction: applies the same old -> new slots to navigation,
    /// directives and targets. Caller must adopt this with its rows/identity
    /// manifest in one generation, and retain the saved identity-based composition.
    /// Removed identities are required even for hidden/policy/facet pages, whose
    /// presentation status deliberately has no row index. They must describe the
    /// same deletion as the slot map; hiding/filtering is not physical deletion.
    pub fn remap_removed_items(
        &self,
        old_to_new: &[Option<usize>],
        removed_pages: &[CompositionPageRef],
    ) -> CompositionResult<Self> {
        for (expected, index) in old_to_new.iter().flatten().enumerate() {
            if *index != expected {
                return Err(CompositionError::Invalid("invalid deletion slot map"));
            }
        }
        let mut removed = HashSet::new();
        for page in removed_pages {
            let key = page.matching_key();
            if !removed.insert(key.clone())
                || matches!(
                    self.page_status(&key),
                    PageStatus::Missing | PageStatus::Invalid
                )
            {
                return Err(CompositionError::Invalid("invalid removed page identity"));
            }
        }
        for page in &self.visible_pages {
            let slot = old_to_new
                .get(page.item_index)
                .ok_or(CompositionError::Invalid(
                    "slot map does not cover projection",
                ))?;
            if slot.is_none() != removed.contains(&page.page.matching_key()) {
                return Err(CompositionError::Invalid(
                    "deletion identities disagree with slot map",
                ));
            }
        }
        let mut result = self.remap_slots(old_to_new)?;
        for key in removed {
            result.statuses.insert(key, PageStatus::Missing);
        }
        // An unsaved tail page has no saved occurrence to retain as Missing.
        result.new_tail.retain(|page| {
            let key = page.matching_key();
            if result.statuses.get(&key) == Some(&PageStatus::Missing) {
                result.statuses.remove(&key);
                false
            } else {
                true
            }
        });
        result.complete = !result
            .statuses
            .values()
            .any(|status| *status == PageStatus::FilteredOut);
        Ok(result)
    }

    /// Reorder/filter materialized rows without declaring any real page Missing.
    /// Hidden/policy/facet rows may have no output slot, but all navigation and
    /// page occurrences must survive. The output is a dense permutation/filter
    /// of the input rows: exactly 0..count(Some), with no holes or new rows.
    /// This is not a Remote budget projection or a physical deletion operation.
    pub fn reindex_items(&self, old_to_new: &[Option<usize>]) -> CompositionResult<Self> {
        let mut targets = HashSet::new();
        let output_len = old_to_new.iter().flatten().count();
        for index in old_to_new.iter().flatten() {
            if *index >= output_len || !targets.insert(*index) {
                return Err(CompositionError::Invalid("invalid item reindex map"));
            }
        }
        for page in &self.visible_pages {
            if old_to_new.get(page.item_index).copied().flatten().is_none() {
                return Err(CompositionError::Invalid(
                    "item reindex map loses a visible page",
                ));
            }
        }
        self.remap_slots(old_to_new)
    }

    fn remap_slots(&self, old_to_new: &[Option<usize>]) -> CompositionResult<Self> {
        let remap_page = |page: &ResolvedPage| -> CompositionResult<Option<ResolvedPage>> {
            let index = old_to_new
                .get(page.item_index)
                .ok_or(CompositionError::Invalid(
                    "slot map does not cover projection",
                ))?;
            Ok(index.map(|item_index| ResolvedPage {
                page: page.page.clone(),
                item_index,
            }))
        };
        let mut result = self.clone();
        result.page_overrides = self
            .page_overrides
            .iter()
            .filter_map(|(old, directive)| {
                old_to_new
                    .get(*old)
                    .copied()
                    .flatten()
                    .map(|new| (new, *directive))
            })
            .collect();
        result.visible_pages = self
            .visible_pages
            .iter()
            .map(remap_page)
            .collect::<CompositionResult<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        for status in result.statuses.values_mut() {
            if let PageStatus::Visible { item_index } = *status {
                *status = match old_to_new
                    .get(item_index)
                    .ok_or(CompositionError::Invalid("slot map does not cover targets"))?
                {
                    Some(index) => PageStatus::Visible { item_index: *index },
                    None => PageStatus::Missing,
                };
            }
        }
        Ok(result)
    }
}

/// Resolve identity, visibility and flat reading order only. All automatic and
/// explicit spread formation belongs to the common builder.
pub fn resolve_composition(
    composition: &BookComposition,
    domain: &PageDomain,
    manifest: &[ManifestPage],
) -> CompositionResult<ResolvedComposition> {
    composition.validate(domain)?;
    if manifest.len() > MAX_PAGES {
        return Err(CompositionError::Limit("manifest pages"));
    }
    let hidden: HashSet<_> = composition
        .hidden_pages
        .iter()
        .map(CompositionPageRef::matching_key)
        .collect();
    let mut facts = HashMap::with_capacity(manifest.len());
    let mut indices = HashSet::new();
    for entry in manifest {
        domain.validate_page(&entry.page)?;
        if facts.insert(entry.page.matching_key(), entry).is_some() {
            return Err(CompositionError::Invalid("duplicate manifest identity"));
        }
        if let ManifestAvailability::Present { item_index } = entry.availability
            && !indices.insert(item_index)
        {
            return Err(CompositionError::Invalid("duplicate manifest item index"));
        }
    }
    let mut statuses = HashMap::new();
    let registered: HashSet<_> = composition
        .pages()
        .map(CompositionPageRef::matching_key)
        .collect();
    let new_tail: Vec<_> = manifest
        .iter()
        .filter(|entry| !registered.contains(&entry.page.matching_key()))
        .map(|entry| entry.page.clone())
        .collect();
    if registered.len() + new_tail.len() > MAX_PAGES {
        return Err(CompositionError::Limit("resolved page count"));
    }
    for page in composition.pages().chain(new_tail.iter()) {
        let key = page.matching_key();
        let status = match facts.get(&key) {
            None => PageStatus::Missing,
            Some(_) if hidden.contains(&key) => PageStatus::HiddenByComposition,
            Some(entry) => match entry.availability {
                ManifestAvailability::Present { item_index } => PageStatus::Visible { item_index },
                ManifestAvailability::ExcludedByPolicy => PageStatus::ExcludedByPolicy,
                ManifestAvailability::FilteredOut => PageStatus::FilteredOut,
            },
        };
        statuses.insert(key, status);
    }
    let resolve_page = |page: &CompositionPageRef| {
        match statuses.get(&page.matching_key()) {
            Some(PageStatus::Visible { item_index }) => {
                // Preserve the manifest spelling for byte reads, not saved spelling.
                let actual = facts[&page.matching_key()].page.clone();
                Some(ResolvedPage {
                    page: actual,
                    item_index: *item_index,
                })
            }
            _ => None,
        }
    };
    let visible_pages: Vec<_> = composition
        .pages()
        .chain(new_tail.iter())
        .filter_map(resolve_page)
        .collect();
    let directives: HashMap<_, _> = composition
        .page_overrides
        .iter()
        .map(|entry| (entry.page.matching_key(), entry.directive))
        .collect();
    let page_overrides = visible_pages
        .iter()
        .filter_map(|page| {
            directives
                .get(&page.page.matching_key())
                .map(|directive| (page.item_index, *directive))
        })
        .collect();
    let complete = !statuses
        .values()
        .any(|status| *status == PageStatus::FilteredOut);
    Ok(ResolvedComposition {
        revision: composition.revision,
        direction: composition.direction,
        cover_enabled: composition.cover_enabled,
        page_overrides,
        visible_pages,
        new_tail,
        complete,
        statuses,
    })
}
