use super::{fixtures::*, *};

fn draft(record: BookComposition) -> CompositionDraft {
    CompositionDraft::new(record, PageDomain::Document, document_manifest(4)).unwrap()
}

fn edit(draft: &mut CompositionDraft, operation: UnitEdit) {
    let preview = draft.preview(operation).unwrap();
    draft.apply(preview, true).unwrap();
}

#[test]
fn draft_rejects_duplicate_blank_without_record_revision_or_history_change() {
    let mut value = draft(editor_unit_edits(ReadingDirection::Ltr).blank);
    let original = value.record().clone();
    assert!(
        value
            .preview(UnitEdit::InsertBlank {
                page: document_page(1),
                placement: SinglePlacement::Trailing,
            })
            .is_err()
    );
    assert_eq!(value.record(), &original);
    assert!(!value.can_undo());
    assert!(!value.can_redo());
}

#[test]
fn draft_move_rejects_consecutive_blank_and_keeps_original_checkpoint() {
    let record = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            single(0, SinglePlacement::Leading, SingleCompanion::Blank),
            pair(2, 3),
            single(1, SinglePlacement::Trailing, SingleCompanion::Blank),
        ],
    );
    let value = draft(record.clone());
    assert!(
        value
            .preview(UnitEdit::MoveUnit { from: 2, to: 1 })
            .is_err()
    );
    assert_eq!(value.record(), &record);
    assert!(!value.can_undo());
}

#[test]
fn draft_page_moves_keep_blank_and_unedited_pair_and_undo_selection() {
    let mut value = draft(editor_unit_edits(ReadingDirection::Ltr).blank);
    value
        .select(Some(DraftSelection::Page(document_page(1))))
        .unwrap();
    let original = value.record().clone();
    edit(
        &mut value,
        UnitEdit::MovePage {
            page: document_page(1),
            to: 2,
        },
    );
    assert_eq!(value.record().units[1], pair(2, 3));
    assert_eq!(
        value.record().units[2],
        single(1, SinglePlacement::Leading, SingleCompanion::Blank)
    );
    let moved = value.record().clone();
    assert!(value.undo());
    assert_eq!(value.record(), &original);
    assert_eq!(
        value.selection(),
        Some(&DraftSelection::Page(document_page(1)))
    );
    assert!(value.redo());
    assert_eq!(value.record(), &moved);
    assert_eq!(value.record().revision, original.revision);
}

#[test]
fn draft_direction_only_mirrors_slots_without_repairing() {
    let mut value = draft(editor_unit_edits(ReadingDirection::Ltr).blank);
    let units = value.record().units.clone();
    edit(&mut value, UnitEdit::SetDirection(ReadingDirection::Rtl));
    assert_eq!(value.record().units, units);
    assert_eq!(
        SinglePlacement::Leading.screen_slot(value.record().direction),
        ScreenSlot::Right
    );
    assert!(value.undo());
    assert!(!value.is_dirty());
}

#[test]
fn draft_confirmation_keeps_cover_helpers_until_explicit_acceptance() {
    let mut value = draft(editor_unit_edits(ReadingDirection::Ltr).with_cover_helper);
    let original = value.record().clone();
    let preview = value.preview(UnitEdit::SetCover(None)).unwrap();
    assert!(
        preview
            .confirmations()
            .contains(&DraftConfirmation::CoverChanged)
    );
    assert!(
        preview
            .confirmations()
            .contains(&DraftConfirmation::CompanionRemoved {
                page: document_page(3)
            })
    );
    assert!(value.apply(preview.clone(), false).is_err());
    assert_eq!(value.record(), &original);
    assert!(!value.can_undo());
    value.apply(preview, true).unwrap();
    assert_eq!(value.record().cover, None);
    assert!(value.record().units.iter().all(|unit| !matches!(
        unit,
        CompositionUnit::Single {
            companion: SingleCompanion::FrontCoverSupplement { .. },
            ..
        }
    )));
    assert!(value.undo());
    assert_eq!(value.record(), &original);
}

#[test]
fn draft_centering_or_pairing_requires_preview_of_removed_blank() {
    let mut value = draft(editor_unit_edits(ReadingDirection::Ltr).blank);
    for operation in [
        UnitEdit::SetSingle {
            page: document_page(1),
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        },
        UnitEdit::PairSingles {
            first_unit: 0,
            clear_cover: false,
        },
    ] {
        let before = value.record().clone();
        let preview = value.preview(operation).unwrap();
        assert!(preview.requires_confirmation());
        assert!(value.apply(preview, false).is_err());
        assert_eq!(value.record(), &before);
    }
}

#[test]
fn draft_rebuild_is_confirmed_and_preserves_hidden_direction_revision_and_membership() {
    let mut record = four_page_book(ReadingDirection::Rtl);
    record.hidden_pages.push(document_page(2));
    record.revision = 19;
    let mut value = draft(record.clone());
    let units = vec![pair(3, 2), pair(1, 0)];
    let preview = value.preview_rebuild(units.clone()).unwrap();
    assert!(
        preview
            .confirmations()
            .contains(&DraftConfirmation::RebuildAutomatic)
    );
    assert_eq!(preview.record().direction, ReadingDirection::Rtl);
    assert_eq!(preview.record().hidden_pages, record.hidden_pages);
    assert_eq!(preview.record().revision, 19);
    assert!(value.apply(preview.clone(), false).is_err());
    value.apply(preview, true).unwrap();
    assert_eq!(value.record().units, units);
    assert!(value.preview_rebuild(vec![pair(0, 1)]).is_err());
    assert!(value.undo());
    assert_eq!(value.record(), &record);
}

#[test]
fn draft_initialization_keeps_missing_hidden_refs_and_adds_policy_excluded_raw_tail() {
    let record = hidden_blank_deletion_book(ReadingDirection::Ltr);
    let mut manifest = document_manifest(5);
    manifest.retain(|entry| entry.page != document_page(1));
    manifest[3].availability = ManifestAvailability::ExcludedByPolicy;
    let value = CompositionDraft::new(record.clone(), PageDomain::Document, manifest).unwrap();
    assert_eq!(
        &value.record().units[..record.units.len()],
        record.units.as_slice()
    );
    assert_eq!(value.record().hidden_pages, record.hidden_pages);
    assert_eq!(
        value.record().units.last(),
        Some(&single(4, SinglePlacement::Center, SingleCompanion::Vacant))
    );
    assert_eq!(
        resolve_composition(value.record(), value.domain(), value.manifest())
            .unwrap()
            .page_status(&document_page(1)),
        PageStatus::Missing
    );
}

#[test]
fn draft_stale_preview_and_cross_draft_preview_cannot_overwrite_edits() {
    let mut value = draft(four_page_book(ReadingDirection::Ltr));
    let preview = value.preview(UnitEdit::SplitPair { unit: 0 }).unwrap();
    edit(&mut value, UnitEdit::SetDirection(ReadingDirection::Rtl));
    let current = value.record().clone();
    assert!(value.apply(preview, true).is_err());
    assert_eq!(value.record(), &current);
    let other = draft(editor_unit_edits(ReadingDirection::Ltr).split);
    let foreign = other
        .preview(UnitEdit::SetDirection(ReadingDirection::Rtl))
        .unwrap();
    let mut fresh = draft(four_page_book(ReadingDirection::Ltr));
    assert!(fresh.apply(foreign, true).is_err());
}

#[test]
fn draft_unit_selection_tracks_real_anchor_and_new_edit_retires_redo() {
    let mut value = draft(four_page_book(ReadingDirection::Ltr));
    value.select(Some(DraftSelection::Unit(1))).unwrap();
    edit(&mut value, UnitEdit::MoveUnit { from: 1, to: 0 });
    assert_eq!(value.selection(), Some(&DraftSelection::Unit(0)));
    assert!(value.undo());
    assert_eq!(value.selection(), Some(&DraftSelection::Unit(1)));
    edit(
        &mut value,
        UnitEdit::SetHidden {
            page: document_page(3),
            hidden: true,
        },
    );
    assert!(!value.can_redo());
    assert!(value.select(Some(DraftSelection::Unit(99))).is_err());
    assert!(
        value
            .select(Some(DraftSelection::Page(document_page(99))))
            .is_err()
    );
}

#[test]
fn draft_pairing_cover_requires_explicit_cover_clear_and_confirmation() {
    let mut value = draft(
        four_page_book(ReadingDirection::Ltr)
            .edited(
                &PageDomain::Document,
                UnitEdit::SetCover(Some(document_page(0))),
            )
            .unwrap(),
    );
    let original = value.record().clone();
    assert!(
        value
            .preview(UnitEdit::PairSingles {
                first_unit: 0,
                clear_cover: false
            })
            .is_err()
    );
    assert_eq!(value.record(), &original);
    let preview = value
        .preview(UnitEdit::PairSingles {
            first_unit: 0,
            clear_cover: true,
        })
        .unwrap();
    assert!(preview.requires_confirmation());
    assert!(value.apply(preview.clone(), false).is_err());
    assert_eq!(value.record(), &original);
    value.apply(preview, true).unwrap();
    assert_eq!(value.record().cover, None);
    assert_eq!(
        value.record().units,
        original
            .edited(
                &PageDomain::Document,
                UnitEdit::PairSingles {
                    first_unit: 0,
                    clear_cover: true
                }
            )
            .unwrap()
            .units
    );
}

#[test]
fn draft_multi_edit_is_atomic_and_undoes_as_one_checkpoint() {
    let mut value = draft(four_page_book(ReadingDirection::Ltr));
    let original = value.record().clone();
    assert!(
        value
            .preview_edits([
                UnitEdit::SplitPair { unit: 0 },
                UnitEdit::InsertBlank {
                    page: document_page(0),
                    placement: SinglePlacement::Leading
                },
                UnitEdit::InsertBlank {
                    page: document_page(1),
                    placement: SinglePlacement::Trailing
                },
            ])
            .is_err()
    );
    assert_eq!(value.record(), &original);
    assert!(!value.can_undo());
    let preview = value
        .preview_edits([
            UnitEdit::SplitPair { unit: 0 },
            UnitEdit::SetHidden {
                page: document_page(1),
                hidden: true,
            },
        ])
        .unwrap();
    value.apply(preview, true).unwrap();
    assert_eq!(value.record().units.len(), 3);
    assert_eq!(value.record().hidden_pages, vec![document_page(1)]);
    assert!(value.undo());
    assert_eq!(value.record(), &original);
    assert!(!value.can_undo());
}

#[test]
fn draft_pair_break_confirmation_compares_final_topology_and_preserves_history() {
    let mut value = draft(four_page_book(ReadingDirection::Ltr));
    edit(&mut value, UnitEdit::SetDirection(ReadingDirection::Rtl));
    let before = value.record().clone();
    let preview = value
        .preview_edits([
            UnitEdit::SplitPair { unit: 0 },
            UnitEdit::SplitPair { unit: 2 },
            UnitEdit::PairSingles {
                first_unit: 1,
                clear_cover: false,
            },
        ])
        .unwrap();
    assert_eq!(
        preview.confirmations(),
        &[
            DraftConfirmation::PairBroken {
                first: document_page(0),
                second: document_page(1)
            },
            DraftConfirmation::PairBroken {
                first: document_page(2),
                second: document_page(3)
            },
        ]
    );
    assert_eq!(value.record(), &before);
    assert!(value.apply(preview.clone(), false).is_err());
    assert_eq!(value.record(), &before);
    // Discarding a candidate leaves the earlier direction edit and its undo intact.
    drop(preview.clone());
    value.apply(preview, true).unwrap();
    assert_eq!(value.record().units[1], pair(1, 2));
    assert!(value.undo());
    assert_eq!(value.record(), &before);
    assert!(value.undo());
    assert_eq!(value.record(), &four_page_book(ReadingDirection::Ltr));
    assert!(!value.can_undo());

    // Moving intact units, changing direction, or temporarily splitting then
    // reconstructing the same pair does not destroy the final pair topology.
    for edits in [
        vec![UnitEdit::MoveUnit { from: 1, to: 0 }],
        vec![UnitEdit::SetDirection(ReadingDirection::Rtl)],
        vec![
            UnitEdit::SplitPair { unit: 0 },
            UnitEdit::PairSingles {
                first_unit: 0,
                clear_cover: false,
            },
        ],
    ] {
        assert!(!value.preview_edits(edits).unwrap().requires_confirmation());
    }
}
