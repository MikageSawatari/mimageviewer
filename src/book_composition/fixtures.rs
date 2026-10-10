//! Shared pure fixtures for G2A and the later G3/G4 handler/adoption tests.
#![allow(dead_code)] // Some returned checkpoints are first consumed by G3/G4.

use super::*;

pub(crate) fn document_page(page_num: u32) -> CompositionPageRef {
    CompositionPageRef::PdfPage { page_num }
}

pub(crate) fn pair(first: u32, second: u32) -> CompositionUnit {
    CompositionUnit::Pair {
        first: document_page(first),
        second: document_page(second),
    }
}

pub(crate) fn single(
    page: u32,
    placement: SinglePlacement,
    companion: SingleCompanion,
) -> CompositionUnit {
    CompositionUnit::Single {
        page: document_page(page),
        placement,
        companion,
    }
}

/// [A,B],[C,D], with raw source slots 0..4. No dimensions/automatic settings.
pub(crate) fn four_page_book(direction: ReadingDirection) -> BookComposition {
    BookComposition::new(direction, vec![pair(0, 1), pair(2, 3)])
}

pub(crate) fn document_manifest(count: u32) -> Vec<ManifestPage> {
    (0..count)
        .map(|page_num| ManifestPage {
            page: document_page(page_num),
            availability: ManifestAvailability::Present {
                item_index: page_num as usize,
            },
        })
        .collect()
}

/// B remains an ordinary saved page with a subordinate Blank even when hidden.
/// Raw source slots are still A/B/C/D; deletion must name B, not just drop a slot.
pub(crate) fn hidden_blank_deletion_book(direction: ReadingDirection) -> BookComposition {
    editor_unit_edits(direction)
        .blank
        .edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(1),
                hidden: true,
            },
        )
        .unwrap()
}

pub(crate) struct EditorUnitEditFixture {
    pub original: BookComposition,
    pub split: BookComposition,
    pub repaired: BookComposition,
    pub blank: BookComposition,
    pub moved: BookComposition,
    pub hidden: BookComposition,
    pub with_cover_helper: BookComposition,
}

pub(crate) fn editor_unit_edits(direction: ReadingDirection) -> EditorUnitEditFixture {
    let domain = PageDomain::Document;
    let original = four_page_book(direction);
    let split = original
        .edited(&domain, UnitEdit::SplitPair { unit: 0 })
        .unwrap();
    let repaired = split
        .edited(
            &domain,
            UnitEdit::PairSingles {
                first_unit: 0,
                clear_cover: false,
            },
        )
        .unwrap();
    let blank = split
        .edited(
            &domain,
            UnitEdit::InsertBlank {
                page: document_page(1),
                placement: SinglePlacement::Leading,
            },
        )
        .unwrap();
    let moved = blank
        .edited(
            &domain,
            UnitEdit::MovePage {
                page: document_page(1),
                to: 2,
            },
        )
        .unwrap();
    let hidden = moved
        .edited(
            &domain,
            UnitEdit::SetHidden {
                page: document_page(1),
                hidden: true,
            },
        )
        .unwrap();
    let with_cover_helper = original
        .edited(&domain, UnitEdit::SetCover(Some(document_page(0))))
        .unwrap()
        .edited(
            &domain,
            UnitEdit::SetSingle {
                page: document_page(3),
                placement: SinglePlacement::Trailing,
                companion: SingleCompanion::FrontCoverSupplement {
                    page: document_page(0),
                },
            },
        )
        .unwrap();
    EditorUnitEditFixture {
        original,
        split,
        repaired,
        blank,
        moved,
        hidden,
        with_cover_helper,
    }
}
