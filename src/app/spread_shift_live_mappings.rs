//! Pure migration of accepted live anchors after the existing book-edit admission.
//! Durable migration remains owned by the spread-shift maintenance worker.

use std::path::{Path, PathBuf};

use super::spread_shift_owner::{ViewerShift, ViewerSpreadShift};
use crate::book_scope::BookScopeKey;
use crate::books::BookPathMapping;
use crate::spread_shift::{SpreadShiftPageRef, SpreadShiftState};

/// Root moves match KeepDrive path components, retaining the original suffix
/// spelling for actual reads. A neighbouring prefix is never a descendant.
pub(super) fn map_root_path(path: &Path, source: &Path, target: &Path) -> Option<PathBuf> {
    let source_key = crate::path_key::normalize_keep_drive(source);
    let path_key = crate::path_key::normalize_keep_drive(path);
    if path_key != source_key
        && !path_key.starts_with(&format!("{}/", source_key.trim_end_matches('/')))
    {
        return None;
    }
    let suffix = path
        .components()
        .skip(source.components().count())
        .collect::<PathBuf>();
    let mapped = if suffix.as_os_str().is_empty() {
        target.to_path_buf()
    } else {
        target.join(suffix)
    };
    (mapped != path).then_some(mapped)
}

/// Migrate physical row sources without changing their accepted indices or media
/// kinds. The same helper serves mounted and parked context ownership boundaries.
pub(super) fn migrate_items_root(
    items: &mut [crate::grid_item::GridItem],
    source: &Path,
    target: &Path,
) -> bool {
    use crate::grid_item::GridItem;
    let move_path = |path: &mut PathBuf| {
        if let Some(mapped) = map_root_path(path, source, target) {
            *path = mapped;
            true
        } else {
            false
        }
    };
    let mut changed = false;
    for item in items {
        changed |= match item {
            GridItem::Folder(path)
            | GridItem::Image(path)
            | GridItem::Video(path)
            | GridItem::Audio(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. }
            | GridItem::CollectionPlaceholder { path, .. } => move_path(path),
            GridItem::ZipImage { zip_path, .. } | GridItem::ZipDir { zip_path, .. } => {
                move_path(zip_path)
            }
            GridItem::PdfPage { pdf_path, .. } => move_path(pdf_path),
            GridItem::Stack { representative, .. } => move_path(representative),
            GridItem::SearchContainer {
                path,
                representative,
                ..
            } => {
                let moved = move_path(path);
                let representative_moved = representative
                    .as_mut()
                    .is_some_and(|value| move_path(&mut value.path));
                moved || representative_moved
            }
        };
    }
    changed
}

pub(super) fn migrate_accepted_root_sources(
    current_folder: &mut Option<PathBuf>,
    address: &mut String,
    archive_source_override: &mut Option<PathBuf>,
    zip_nav: &mut Option<crate::zip_tree::ZipNavState>,
    items: &mut [crate::grid_item::GridItem],
    index: &mut std::sync::Arc<super::file_type_resume::PreparedPageIdentityIndex>,
    source: &Path,
    target: &Path,
) -> bool {
    let mut changed = false;
    if let Some(folder) = current_folder
        && let Some(mapped) = map_root_path(folder, source, target)
    {
        *folder = mapped;
        *address = folder.to_string_lossy().into_owned();
        changed = true;
    }
    if let Some(logical) = archive_source_override
        && let Some(mapped) = map_root_path(logical, source, target)
    {
        *logical = mapped;
        changed = true;
    }
    if let Some(nav) = zip_nav
        && let Some(mapped) = map_root_path(&nav.tree.zip_path, source, target)
    {
        std::sync::Arc::make_mut(&mut nav.tree).zip_path = mapped;
        changed = true;
    }
    changed |= migrate_items_root(items, source, target);
    if changed {
        *index = std::sync::Arc::new(index.with_moved_root(source, target));
    }
    changed
}

pub(super) fn migrate(
    owner: &mut ViewerSpreadShift,
    mappings: &[BookPathMapping],
    root_move: Option<(&Path, &Path)>,
) -> bool {
    let Some(original_scope) = owner.scope.as_ref() else {
        return false;
    };
    // Both replacements use the original snapshot. A swap must never remap its output.
    let replacement_scope = root_move.and_then(|(from, to)| {
        let source = BookScopeKey::folder(from).ok()?;
        let target = BookScopeKey::folder(to).ok()?;
        let source_root = crate::path_key::normalize_keep_drive(&source.logical_root);
        let original_root = crate::path_key::normalize_keep_drive(&original_scope.logical_root);
        let destination = if original_root == source_root {
            target.logical_root
        } else {
            let prefix = format!("{}/", source_root.trim_end_matches('/'));
            let suffix = original_root.strip_prefix(&prefix)?;
            target.logical_root.join(suffix)
        };
        BookScopeKey::new(destination, &original_scope.level_prefix)
            .ok()
            .filter(|scope| scope != original_scope)
    });
    // The admitted durable operation is either a root move or page mappings.
    let replacement_anchor = if root_move.is_none() {
        match &owner.state {
            ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref: SpreadShiftPageRef::RelativePath(relative),
                ..
            }) => map_relative(&original_scope.storage_key(), relative, mappings)
                .map(SpreadShiftPageRef::RelativePath)
                .filter(|reference| reference.validate().is_ok())
                .filter(|reference| {
                    reference != &SpreadShiftPageRef::RelativePath(relative.clone())
                }),
            _ => None,
        }
    } else {
        None
    };

    let changed = replacement_scope.is_some() || replacement_anchor.is_some();
    if let Some(scope) = replacement_scope {
        owner.scope = Some(scope);
    }
    if let Some(reference) = replacement_anchor
        && let ViewerShift::Book(SpreadShiftState::Adjusted { anchor_ref, .. }) = &mut owner.state
    {
        *anchor_ref = reference;
    }
    if changed {
        owner.invalidate_projection();
    }
    changed
}

/// Match the most specific original path, like the durable D1 migration. Root moves
/// and moves outside this book leave its relative payload unchanged.
fn map_relative(scope: &str, relative: &str, mappings: &[BookPathMapping]) -> Option<String> {
    let root = Path::new(scope);
    let original = root.join(relative.replace(['/', '\\'], std::path::MAIN_SEPARATOR_STR));
    let scope_prefix = format!("{}/", scope.trim_end_matches('/'));
    for candidate in original.ancestors() {
        let candidate_key = crate::path_key::normalize_keep_drive(candidate);
        let Some(mapping) = mappings
            .iter()
            .find(|mapping| crate::path_key::normalize_keep_drive(&mapping.from) == candidate_key)
        else {
            continue;
        };
        if !candidate_key.starts_with(&scope_prefix) {
            return None;
        }
        let suffix = original
            .components()
            .skip(mapping.from.components().count())
            .collect::<PathBuf>();
        let mapped = mapping.to.join(suffix);
        if !crate::path_key::normalize_keep_drive(&mapped).starts_with(&scope_prefix) {
            return None;
        }
        return Some(
            mapped
                .components()
                .skip(root.components().count())
                .collect::<PathBuf>()
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spread_shift::{ShiftPersistence, ShiftResolution};

    fn owner(scope: BookScopeKey, reference: SpreadShiftPageRef) -> ViewerSpreadShift {
        let mut owner = ViewerSpreadShift::default();
        owner.scope = Some(scope);
        owner.state = ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: reference,
            resolution: ShiftResolution::Resolved(3),
            persistence: ShiftPersistence::SessionOnly,
        });
        owner
    }

    fn mapping(from: &str, to: &str) -> BookPathMapping {
        BookPathMapping {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn page_swap_maps_each_original_anchor_once_and_preserves_off_session() {
        let scope = BookScopeKey::folder("c:/books/book").unwrap();
        let maps = [
            mapping("C:/BOOKS/BOOK/001.jpg", "c:/books/book/002.jpg"),
            mapping("c:/books/book/002.jpg", "c:/books/book/001.jpg"),
        ];
        for (from, to) in [("001.jpg", "002.jpg"), ("002.jpg", "001.jpg")] {
            let mut live = owner(scope.clone(), SpreadShiftPageRef::RelativePath(from.into()));
            assert!(migrate(&mut live, &maps, None));
            assert_eq!(
                live.state,
                ViewerShift::Book(SpreadShiftState::Adjusted {
                    anchor_ref: SpreadShiftPageRef::RelativePath(to.into()),
                    resolution: ShiftResolution::Resolved(3),
                    persistence: ShiftPersistence::SessionOnly,
                })
            );
        }
    }

    #[test]
    fn root_move_changes_canonical_scope_and_retains_relative_anchor() {
        let mut live = owner(
            BookScopeKey::folder("c:/books/old").unwrap(),
            SpreadShiftPageRef::RelativePath("Chapter\\Page.JPG".into()),
        );
        let original_state = live.state.clone();
        assert!(migrate(
            &mut live,
            &[mapping("C:/books/old", "D:/books/New")],
            Some((Path::new("C:/books/old"), Path::new("D:/books/New")))
        ));
        assert_eq!(
            live.scope,
            Some(BookScopeKey::folder("d:/books/new").unwrap())
        );
        assert_eq!(live.state, original_state);
    }

    #[test]
    fn unmatched_sibling_and_outside_page_move_are_untouched() {
        let maps = [mapping("c:/books/book/001.jpg", "c:/books/other/001.jpg")];
        for root in ["c:/books/book", "c:/books/book-other", "d:/books/book"] {
            let mut live = owner(
                BookScopeKey::folder(root).unwrap(),
                SpreadShiftPageRef::RelativePath("001.jpg".into()),
            );
            let original = live.clone();
            assert!(!migrate(&mut live, &maps, None));
            assert_eq!(live, original);
        }
        let mut sibling = owner(
            BookScopeKey::folder("c:/books/book-other").unwrap(),
            SpreadShiftPageRef::RelativePath("001.jpg".into()),
        );
        let original = sibling.clone();
        assert!(!migrate(
            &mut sibling,
            &[],
            Some((Path::new("c:/books/book"), Path::new("c:/books/renamed")))
        ));
        assert_eq!(sibling, original);
    }

    #[test]
    fn inner_directory_mapping_preserves_spelling_and_prefers_specific_page_map() {
        let scope = BookScopeKey::folder("c:/books/book").unwrap();
        let mut live = owner(
            scope,
            SpreadShiftPageRef::RelativePath("Chapter\\Page.JPG".into()),
        );
        assert!(migrate(
            &mut live,
            &[
                mapping("C:/BOOKS/BOOK/chapter", "c:/books/book/NewChapter"),
                mapping(
                    "c:/books/book/chapter/page.jpg",
                    "c:/books/book/Specific.JPG"
                ),
            ],
            None
        ));
        assert!(matches!(
            live.state,
            ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref: SpreadShiftPageRef::RelativePath(ref relative), ..
            }) if relative == "Specific.JPG"
        ));
    }

    #[test]
    fn archive_and_document_page_refs_ignore_page_maps_but_follow_root_move() {
        for (scope, reference) in [
            (
                BookScopeKey::archive("c:/books/book/outer.zip", &["Chapter".into()]).unwrap(),
                SpreadShiftPageRef::ArchiveEntry("Chapter/Page.JPG".into()),
            ),
            (
                BookScopeKey::document("c:/books/book/document.pdf").unwrap(),
                SpreadShiftPageRef::DocumentPageIdentity(4),
            ),
        ] {
            let mut live = owner(scope.clone(), reference);
            let original = live.clone();
            assert!(!migrate(
                &mut live,
                &[mapping(
                    "c:/books/book/Chapter/Page.JPG",
                    "c:/books/book/New.JPG"
                )],
                None
            ));
            assert_eq!(live, original);
            assert!(migrate(
                &mut live,
                &[],
                Some((Path::new("c:/books/book"), Path::new("d:/books/new")))
            ));
            assert_eq!(live.state, original.state);
            assert_eq!(
                live.scope.as_ref().unwrap().level_prefix,
                scope.level_prefix
            );
            assert!(
                live.scope
                    .as_ref()
                    .unwrap()
                    .storage_key()
                    .starts_with("d:/books/new/")
            );
        }
    }
}
