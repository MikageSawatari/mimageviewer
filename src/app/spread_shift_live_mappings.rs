//! Pure migration of accepted live anchors after the existing book-edit admission.
//! Durable migration remains owned by the spread-shift maintenance worker.

use std::path::{Path, PathBuf};

use super::spread_shift_owner::{ViewerShift, ViewerSpreadShift};
use crate::books::BookPathMapping;
use crate::spread_shift::{SpreadShiftPageRef, SpreadShiftState};

/// Page edits remap the accepted anchor within its original scope. Root renames
/// migrate only saved rows, retaining the released old-path viewer contexts.
pub(super) fn migrate(owner: &mut ViewerSpreadShift, mappings: &[BookPathMapping]) -> bool {
    let Some(scope) = owner.scope.as_ref() else {
        return false;
    };
    let replacement = match &owner.state {
        ViewerShift::Book(SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::RelativePath(relative),
            ..
        }) => map_relative(&scope.storage_key(), relative, mappings)
            .map(SpreadShiftPageRef::RelativePath)
            .filter(|reference| reference.validate().is_ok())
            .filter(|reference| reference != &SpreadShiftPageRef::RelativePath(relative.clone())),
        _ => None,
    };
    let Some(reference) = replacement else {
        return false;
    };
    if let ViewerShift::Book(SpreadShiftState::Adjusted { anchor_ref, .. }) = &mut owner.state {
        *anchor_ref = reference;
    }
    owner.invalidate_projection();
    true
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
    use crate::book_scope::BookScopeKey;
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
            assert!(migrate(&mut live, &maps));
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
    fn unmatched_sibling_and_outside_page_move_are_untouched() {
        let maps = [mapping("c:/books/book/001.jpg", "c:/books/other/001.jpg")];
        for root in ["c:/books/book", "c:/books/book-other", "d:/books/book"] {
            let mut live = owner(
                BookScopeKey::folder(root).unwrap(),
                SpreadShiftPageRef::RelativePath("001.jpg".into()),
            );
            let original = live.clone();
            assert!(!migrate(&mut live, &maps));
            assert_eq!(live, original);
        }
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
            ]
        ));
        assert!(matches!(
            live.state,
            ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref: SpreadShiftPageRef::RelativePath(ref relative), ..
            }) if relative == "Specific.JPG"
        ));
    }

    #[test]
    fn archive_and_document_page_refs_ignore_page_maps() {
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
            let mut live = owner(scope, reference);
            let original = live.clone();
            assert!(!migrate(
                &mut live,
                &[mapping(
                    "c:/books/book/Chapter/Page.JPG",
                    "c:/books/book/New.JPG"
                )],
            ));
            assert_eq!(live, original);
        }
    }
}
