//! Prepared reading-position identities belong to the projected reader order.
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use crate::{
    book_resume_db::{PageIdentity, ReadingPosition},
    file_type_visibility::FileTypeVisibility,
    grid_item::GridItem,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreparedPageIdentityIndex {
    pub(crate) policy: FileTypeVisibility,
    // Logical paths never need cloning during UI deletion. Original slots remain stable
    // while the small integer projection changes with the same row-removal rule.
    identities: Arc<HashMap<PageIdentity, usize>>,
    shift_identities: Arc<HashMap<PageIdentity, usize>>,
    current_slots: Vec<Option<usize>>,
    readable_indices: HashSet<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PositionResolution {
    Restored(usize),
    HiddenIdentity,
    LegacyWithExclusions,
    InvalidRaw,
}

impl PreparedPageIdentityIndex {
    /// Called by the preparation worker after projection and final page ordering.
    pub(crate) fn prepare(
        items: &[GridItem],
        logical_container: Option<&Path>,
        policy: FileTypeVisibility,
    ) -> Self {
        let mut identities = HashMap::new();
        let mut readable_indices = std::collections::HashSet::new();
        for (index, item) in items.iter().enumerate() {
            if let Some(identity) = PageIdentity::for_item(item, logical_container) {
                // Same-name real-directory/nested-archive collisions are unsupported.
                identities.entry(identity).or_insert(index);
                readable_indices.insert(index);
            }
        }
        let shift_identities = identities
            .iter()
            .map(|(identity, index)| (shift_identity_key(identity), *index))
            .collect();
        Self {
            policy,
            identities: Arc::new(identities),
            shift_identities: Arc::new(shift_identities),
            current_slots: (0..items.len()).map(Some).collect(),
            readable_indices,
        }
    }

    pub(crate) fn empty(policy: FileTypeVisibility) -> Self {
        Self {
            policy,
            ..Default::default()
        }
    }

    /// Pure integer reindexing, matching the owning grid's exact removed row slots.
    pub(crate) fn without_removed_indices(&self, removed: &[usize]) -> Self {
        let mut removed = removed.to_vec();
        removed.sort_unstable();
        removed.dedup();
        let remap = |index: usize| match removed.binary_search(&index) {
            Ok(_) => None,
            Err(shift) => Some(index - shift),
        };
        Self {
            policy: self.policy.clone(),
            identities: Arc::clone(&self.identities),
            shift_identities: Arc::clone(&self.shift_identities),
            current_slots: self
                .current_slots
                .iter()
                .map(|slot| slot.and_then(remap))
                .collect(),
            readable_indices: self
                .readable_indices
                .iter()
                .filter_map(|index| remap(*index))
                .collect(),
        }
    }

    /// Worker-only merge. A candidate may be omitted or already present in the owner;
    /// only slots accepted into the final tail are added, retaining the owner's policy.
    pub(crate) fn with_mapped_additions(&self, additions: &Self, slots: &[Option<usize>]) -> Self {
        let mut identities = HashMap::new();
        let mut readable_indices = self.readable_indices.clone();
        for (identity, original_slot) in self.identities.iter() {
            if let Some(index) = self.current_slots.get(*original_slot).copied().flatten() {
                identities.insert(identity.clone(), index);
            }
        }
        for (identity, original_slot) in additions.identities.iter() {
            if let Some(index) = additions
                .current_slots
                .get(*original_slot)
                .copied()
                .flatten()
                .and_then(|local| slots.get(local).copied().flatten())
            {
                identities.entry(identity.clone()).or_insert(index);
            }
        }
        readable_indices.extend(
            additions
                .readable_indices
                .iter()
                .filter_map(|local| slots.get(*local).copied().flatten()),
        );
        let len = readable_indices
            .iter()
            .max()
            .map_or(0, |max| max + 1)
            .max(
                self.current_slots
                    .iter()
                    .flatten()
                    .max()
                    .map_or(0, |max| max + 1),
            )
            .max(slots.iter().flatten().max().map_or(0, |max| max + 1));
        let shift_identities = identities
            .iter()
            .map(|(identity, index)| (shift_identity_key(identity), *index))
            .collect();
        Self {
            policy: self.policy.clone(),
            identities: Arc::new(identities),
            shift_identities: Arc::new(shift_identities),
            current_slots: (0..len).map(Some).collect(),
            readable_indices,
        }
    }

    pub(crate) fn resolve(&self, position: &ReadingPosition) -> PositionResolution {
        if let Some(identity) = &position.identity {
            return self
                .identities
                .get(identity)
                .and_then(|slot| self.current_slots.get(*slot).copied().flatten())
                .map(PositionResolution::Restored)
                .unwrap_or(PositionResolution::HiddenIdentity);
        }
        if !self.policy.is_all_visible() {
            return PositionResolution::LegacyWithExclusions;
        }
        if self.readable_indices.contains(&position.page) {
            PositionResolution::Restored(position.page)
        } else {
            PositionResolution::InvalidRaw
        }
    }

    /// Shift lookup shares the accepted row slots, with its own specified slash/case rule.
    /// It never invokes the legacy resume raw-index fallback.
    pub(crate) fn resolve_shift_identity(&self, identity: &PageIdentity) -> Option<usize> {
        self.shift_identities
            .get(&shift_identity_key(identity))
            .and_then(|slot| self.current_slots.get(*slot).copied().flatten())
    }
}

fn shift_identity_key(identity: &PageIdentity) -> PageIdentity {
    let path = |value: &str| value.replace('\\', "/").to_lowercase();
    match identity {
        PageIdentity::Image { path: value } => PageIdentity::Image { path: path(value) },
        PageIdentity::Archive {
            container,
            entry_name,
        } => PageIdentity::Archive {
            container: path(container),
            entry_name: path(entry_name),
        },
        PageIdentity::Document {
            container,
            page_num,
        } => PageIdentity::Document {
            container: path(container),
            page_num: *page_num,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_resume_never_substitutes_same_index_and_preserves_session_policy() {
        let rows = ["1.jpg", "2.png", "3.jpg"].map(|name| GridItem::Image(name.into()));
        let position = ReadingPosition {
            page: 1,
            identity: PageIdentity::for_item(&rows[1], None),
        };
        let mut policy = FileTypeVisibility::default();
        policy.excluded_extensions.insert("png".into());
        let projected =
            PreparedPageIdentityIndex::prepare(&[rows[0].clone(), rows[2].clone()], None, policy);
        assert_eq!(
            projected.resolve(&position),
            PositionResolution::HiddenIdentity
        );
        assert_eq!(
            projected.resolve(&ReadingPosition {
                page: 1,
                identity: None
            }),
            PositionResolution::LegacyWithExclusions
        );
        let session =
            PreparedPageIdentityIndex::prepare(&rows, None, FileTypeVisibility::default());
        assert_eq!(session.resolve(&position), PositionResolution::Restored(1));
        assert_eq!(
            session.resolve(&ReadingPosition {
                page: 1,
                identity: None
            }),
            PositionResolution::Restored(1)
        );
    }

    #[test]
    fn logical_archive_and_document_identities_survive_cache_path_and_sort_changes() {
        let logical = Path::new("book.cbr");
        let page = GridItem::ZipImage {
            zip_path: "cache-old.zip".into(),
            entry_name: "ch/2.png".into(),
        };
        let position = ReadingPosition {
            page: 9,
            identity: PageIdentity::for_item(&page, Some(logical)),
        };
        let rows = [GridItem::ZipImage {
            zip_path: "cache-new.zip".into(),
            entry_name: "ch/2.png".into(),
        }];
        assert_eq!(
            PreparedPageIdentityIndex::prepare(&rows, Some(logical), FileTypeVisibility::default())
                .resolve(&position),
            PositionResolution::Restored(0)
        );
        let doc = GridItem::PdfPage {
            pdf_path: "cache.pdf".into(),
            page_num: 7,
            content_type: None,
        };
        let epub = Path::new("book.epub");
        let position = ReadingPosition {
            page: 2,
            identity: PageIdentity::for_item(&doc, Some(epub)),
        };
        let index =
            PreparedPageIdentityIndex::prepare(&[doc], Some(epub), FileTypeVisibility::default());
        assert_eq!(index.resolve(&position), PositionResolution::Restored(0));
    }
}

#[cfg(test)]
mod remap_tests {
    use super::*;
    fn position(item: &GridItem) -> ReadingPosition {
        ReadingPosition {
            page: 99,
            identity: PageIdentity::for_item(item, None),
        }
    }
    #[test]
    fn visibility_deletion_remaps_repeated_current_slots_and_preserves_saved_arc() {
        let rows = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(|p| GridItem::Image(p.into()));
        let mut policy = FileTypeVisibility::default();
        policy.excluded_extensions.insert("png".into());
        let saved = Arc::new(PreparedPageIdentityIndex::prepare(
            &rows,
            None,
            policy.clone(),
        ));
        let first = saved.without_removed_indices(&[0]);
        assert!(Arc::ptr_eq(&saved.identities, &first.identities));
        assert_eq!(first.policy, policy);
        assert_eq!(
            first.resolve(&position(&rows[1])),
            PositionResolution::Restored(0)
        );
        assert_eq!(
            first.resolve(&position(&rows[2])),
            PositionResolution::Restored(1)
        );
        let second = first.without_removed_indices(&[2, 0, 0]);
        assert_eq!(
            second.resolve(&position(&rows[2])),
            PositionResolution::Restored(0)
        );
        for i in [0, 1, 3] {
            assert_eq!(
                second.resolve(&position(&rows[i])),
                PositionResolution::HiddenIdentity
            );
        }
        assert_eq!(
            saved.resolve(&position(&rows[1])),
            PositionResolution::Restored(1)
        );
    }
    #[test]
    fn visibility_worker_merge_accepts_only_mapped_rows_and_readmits_removed_identity() {
        let rows = ["a.jpg", "b.jpg", "c.jpg"].map(|p| GridItem::Image(p.into()));
        let mut policy = FileTypeVisibility::default();
        policy.excluded_extensions.insert("png".into());
        let base = PreparedPageIdentityIndex::prepare(&rows[..2], None, policy.clone())
            .without_removed_indices(&[0]);
        let additions = PreparedPageIdentityIndex::prepare(&rows, None, Default::default());
        let merged = base.with_mapped_additions(&additions, &[Some(1), None, Some(2)]);
        assert_eq!(merged.policy, policy);
        assert_eq!(
            merged.resolve(&position(&rows[0])),
            PositionResolution::Restored(1)
        );
        assert_eq!(
            merged.resolve(&position(&rows[1])),
            PositionResolution::Restored(0)
        );
        assert_eq!(
            merged.resolve(&position(&rows[2])),
            PositionResolution::Restored(2)
        );
        assert_eq!(
            base.resolve(&position(&rows[0])),
            PositionResolution::HiddenIdentity
        );
    }
}
