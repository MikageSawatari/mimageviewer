use super::fixtures::*;
use super::*;
fn draft() -> CompositionDraft {
    CompositionDraft::new(
        four_page_book(ReadingDirection::Ltr),
        PageDomain::Document,
        document_manifest(4),
    )
    .unwrap()
}

#[test]
fn direct_operations_preserve_selected_identity_and_undo_redo_record() {
    let mut draft = draft();
    draft
        .select(Some(DraftSelection::Page(document_page(1))))
        .unwrap();
    let initial = draft.record().clone();
    draft
        .apply(CompositionEdit::MovePage {
            page: document_page(1),
            to: 3,
        })
        .unwrap();
    assert_eq!(draft.record().page_order, [0, 2, 3, 1].map(document_page));
    assert_eq!(
        draft.selection(),
        Some(&DraftSelection::Page(document_page(1)))
    );
    let moved = draft.record().clone();
    assert!(draft.undo());
    assert_eq!(draft.record(), &initial);
    assert!(draft.redo());
    assert_eq!(draft.record(), &moved);
    assert!(draft.is_dirty());
    assert_eq!(draft.record().revision, initial.revision);
}

#[test]
fn override_auto_hidden_and_direction_cover_are_direct_exclusive_record_changes() {
    let mut draft = draft();
    draft
        .apply(CompositionEdit::SetOverride {
            page: document_page(1),
            directive: Some(PageOverride::Alone),
        })
        .unwrap();
    draft
        .apply(CompositionEdit::SetOverride {
            page: document_page(1),
            directive: Some(PageOverride::BlankOpposite {
                page_side: PageSide::Trailing,
            }),
        })
        .unwrap();
    assert_eq!(draft.record().page_overrides.len(), 1);
    draft
        .apply(CompositionEdit::SetHidden {
            page: document_page(1),
            hidden: true,
        })
        .unwrap();
    assert_eq!(
        draft.record().page_override(&document_page(1)),
        Some(PageOverride::BlankOpposite {
            page_side: PageSide::Trailing
        })
    );
    draft
        .apply_edits([
            CompositionEdit::SetDirection(ReadingDirection::Rtl),
            CompositionEdit::SetCover(true),
        ])
        .unwrap();
    assert!(draft.record().cover_enabled);
    assert_eq!(draft.record().page_order, [0, 1, 2, 3].map(document_page));
    assert!(draft.undo());
    assert!(!draft.record().cover_enabled);
    assert_eq!(draft.record().direction, ReadingDirection::Ltr);
    draft
        .apply(CompositionEdit::SetOverride {
            page: document_page(1),
            directive: None,
        })
        .unwrap();
    assert!(draft.record().page_overrides.is_empty());
    assert!(!draft.can_redo());
}

#[test]
fn invalid_batches_noop_and_bad_selection_leave_checkpoint_history_unchanged() {
    let mut draft = draft();
    let record = draft.record().clone();
    assert!(
        draft
            .apply_edits([
                CompositionEdit::SetCover(true),
                CompositionEdit::MovePage {
                    page: document_page(8),
                    to: 1
                }
            ])
            .is_err()
    );
    assert_eq!(draft.record(), &record);
    assert_eq!(draft.generation(), 0);
    assert!(!draft.can_undo());
    assert!(!draft.apply(CompositionEdit::SetCover(false)).unwrap());
    assert!(!draft.can_undo());
    assert!(
        draft
            .select(Some(DraftSelection::Page(document_page(8))))
            .is_err()
    );
    assert!(draft.selection().is_none());
    assert!(
        draft
            .apply(CompositionEdit::SetOrder(vec![document_page(0)]))
            .is_err()
    );
}

#[test]
fn new_tail_appends_all_raw_membership_and_missing_hidden_directives_survive() {
    let mut record = hidden_blank_deletion_book(ReadingDirection::Ltr);
    record.page_order.push(document_page(9));
    record.page_overrides.push(PageOverrideEntry {
        page: document_page(9),
        directive: PageOverride::Alone,
    });
    let mut manifest = document_manifest(6);
    manifest[4].availability = ManifestAvailability::ExcludedByPolicy;
    let draft = CompositionDraft::new(record.clone(), PageDomain::Document, manifest).unwrap();
    assert_eq!(
        draft.record().page_order,
        [0, 1, 2, 3, 9, 4, 5].map(document_page)
    );
    assert!(draft.is_dirty());
    assert_eq!(draft.record().hidden_pages, record.hidden_pages);
    assert_eq!(draft.record().page_overrides, record.page_overrides);
}

#[test]
fn undo_is_bounded_and_selection_does_not_change_result_generation() {
    let mut draft = draft();
    draft
        .select(Some(DraftSelection::Page(document_page(0))))
        .unwrap();
    assert_eq!(draft.generation(), 0);
    for n in 0..40 {
        draft.apply(CompositionEdit::SetCover(n % 2 == 0)).unwrap();
    }
    let mut count = 0;
    while draft.undo() {
        count += 1;
    }
    assert_eq!(count, 32);
    assert_eq!(
        draft.selection(),
        Some(&DraftSelection::Page(document_page(0)))
    );
}
