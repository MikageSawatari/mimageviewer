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
    page_overview::{PageOverviewEntry, PageOverviewMetadata, PageOverviewPage},
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
    overview_metadata: Arc<PageOverviewMetadata>,
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
        let mut overview_entries = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if let Some(identity) = PageIdentity::for_item(item, logical_container) {
                overview_entries.push(PageOverviewEntry {
                    items_idx: index,
                    page: Arc::new(PageOverviewPage {
                        identity: identity.clone(),
                        label: Arc::from(item.name().as_ref()),
                        white_companion: None,
                    }),
                });
                // Same-name real-directory/nested-archive collisions are unsupported.
                identities.entry(identity).or_insert(index);
                readable_indices.insert(index);
            }
        }
        let shift_identities = prepare_shift_identities(&identities);
        Self {
            policy,
            identities: Arc::new(identities),
            shift_identities: Arc::new(shift_identities),
            current_slots: (0..items.len()).map(Some).collect(),
            readable_indices,
            overview_metadata: Arc::new(
                PageOverviewMetadata::new(overview_entries)
                    .expect("prepared items have unique integer indices"),
            ),
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
            overview_metadata: Arc::new(
                self.overview_metadata
                    .remap_items(self.overview_metadata.entries().filter_map(|entry| {
                        remap(entry.items_idx).map(|idx| (entry.items_idx, idx))
                    }))
                    .expect("row deletion preserves unique integer indices"),
            ),
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
        let shift_identities = prepare_shift_identities(&identities);
        let mut overview_entries: HashMap<usize, PageOverviewEntry> = self
            .overview_metadata
            .entries()
            .map(|entry| (entry.items_idx, entry.clone()))
            .collect();
        for entry in additions.overview_metadata.entries() {
            if let Some(index) = slots.get(entry.items_idx).copied().flatten() {
                overview_entries
                    .entry(index)
                    .or_insert_with(|| PageOverviewEntry {
                        items_idx: index,
                        page: Arc::clone(&entry.page),
                    });
            }
        }
        Self {
            policy: self.policy.clone(),
            identities: Arc::new(identities),
            shift_identities: Arc::new(shift_identities),
            current_slots: (0..len).map(Some).collect(),
            readable_indices,
            overview_metadata: Arc::new(
                PageOverviewMetadata::new(overview_entries.into_values())
                    .expect("worker merge has unique accepted integer indices"),
            ),
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

    /// Shared canonical payload prepared with the items. Opening needs no name/path work.
    pub(crate) fn overview_metadata(&self) -> &Arc<PageOverviewMetadata> {
        &self.overview_metadata
    }

    /// Shift lookup shares the accepted row slots, with its own specified slash/case rule.
    /// It never invokes the legacy resume raw-index fallback.
    pub(crate) fn resolve_shift_identity(&self, identity: &PageIdentity) -> Option<usize> {
        self.shift_identities
            .get(&shift_identity_key(identity))
            .and_then(|slot| self.current_slots.get(*slot).copied().flatten())
    }
}

/// D1 admits the first row in accepted order for a slash/case collision. These
/// inputs are HashMaps, so choose that row explicitly rather than their iteration order.
fn prepare_shift_identities(
    identities: &HashMap<PageIdentity, usize>,
) -> HashMap<PageIdentity, usize> {
    let mut result = HashMap::new();
    for (identity, index) in identities {
        result
            .entry(shift_identity_key(identity))
            .and_modify(|first: &mut usize| *first = (*first).min(*index))
            .or_insert(*index);
    }
    result
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
    fn shift_case_collisions_choose_first_accepted_row_in_both_preparation_paths() {
        for archive in [false, true] {
            let item = |name: &str| {
                if archive {
                    GridItem::ZipImage {
                        zip_path: "book.zip".into(),
                        entry_name: name.into(),
                    }
                } else {
                    GridItem::Image(name.into())
                }
            };
            let rows = [item("A.jpg"), item("a.jpg")];
            let lookup = PageIdentity::for_item(&rows[1], None).unwrap();
            // Repeat to exercise independently seeded maps; either spelling must use
            // the deterministic row-order choice without altering resume matching.
            for _ in 0..32 {
                let prepared = PreparedPageIdentityIndex::prepare(&rows, None, Default::default());
                assert_eq!(prepared.resolve_shift_identity(&lookup), Some(0));
                assert_eq!(
                    prepared.resolve(&position(&rows[1])),
                    PositionResolution::Restored(usize::from(archive))
                );
                let first =
                    PreparedPageIdentityIndex::prepare(&rows[..1], None, Default::default());
                let second =
                    PreparedPageIdentityIndex::prepare(&rows[1..], None, Default::default());
                let merged = first.with_mapped_additions(&second, &[Some(1)]);
                assert_eq!(merged.resolve_shift_identity(&lookup), Some(0));
                // The first row is the smallest final slot, even if it comes from additions.
                let reversed = second.with_mapped_additions(&first, &[Some(1)]);
                assert_eq!(reversed.resolve_shift_identity(&lookup), Some(0));
            }
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

    #[test]
    fn overview_metadata_prepares_logical_archive_document_and_display_labels() {
        let rows = [
            GridItem::Image("folder/first.jpg".into()),
            GridItem::Folder("folder/chapter".into()),
            GridItem::ZipImage {
                zip_path: "cache/book.zip".into(),
                entry_name: "chapter/second.png".into(),
            },
            GridItem::PdfPage {
                pdf_path: "cache/book.pdf".into(),
                page_num: 4,
                content_type: None,
            },
        ];
        let logical = Path::new("books/source.epub");
        let index = PreparedPageIdentityIndex::prepare(&rows, Some(logical), Default::default());
        assert_eq!(index.overview_metadata().entries().count(), 3);
        assert!(index.overview_metadata().entry(1).is_none());
        for idx in [0, 2, 3] {
            let entry = index.overview_metadata().entry(idx).unwrap();
            assert_eq!(
                entry.page.identity,
                PageIdentity::for_item(&rows[idx], Some(logical)).unwrap()
            );
            assert_eq!(entry.page.label.as_ref(), rows[idx].name().as_ref());
            assert!(entry.page.white_companion.is_none());
        }
        let shared = index.clone();
        assert!(Arc::ptr_eq(
            index.overview_metadata(),
            shared.overview_metadata()
        ));
    }

    #[test]
    fn overview_metadata_repeated_deletion_and_worker_merge_share_payloads() {
        let rows = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(|p| GridItem::Image(p.into()));
        let original = PreparedPageIdentityIndex::prepare(&rows, None, Default::default());
        let removed = original
            .without_removed_indices(&[0])
            .without_removed_indices(&[1]);
        assert_eq!(removed.overview_metadata().entries().count(), 2);
        for (old, new) in [(1, 0), (3, 1)] {
            assert!(Arc::ptr_eq(
                &original.overview_metadata().entry(old).unwrap().page,
                &removed.overview_metadata().entry(new).unwrap().page,
            ));
        }
        let additions = PreparedPageIdentityIndex::prepare(&rows[..3], None, Default::default());
        let merged = removed.with_mapped_additions(&additions, &[Some(2), None, Some(3)]);
        assert_eq!(merged.overview_metadata().entries().count(), 4);
        for (source, old, new) in [
            (&removed, 0, 0),
            (&removed, 1, 1),
            (&additions, 0, 2),
            (&additions, 2, 3),
        ] {
            assert!(Arc::ptr_eq(
                &source.overview_metadata().entry(old).unwrap().page,
                &merged.overview_metadata().entry(new).unwrap().page,
            ));
            assert_eq!(
                merged.resolve(&ReadingPosition {
                    page: 99,
                    identity: Some(
                        merged
                            .overview_metadata()
                            .entry(new)
                            .unwrap()
                            .page
                            .identity
                            .clone()
                    ),
                }),
                PositionResolution::Restored(new)
            );
        }
        assert_eq!(original.overview_metadata().entry(0).unwrap().items_idx, 0);
        assert_eq!(removed.overview_metadata().entries().count(), 2);
    }
}
