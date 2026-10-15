//! Shared record fixtures: pairing is always produced by the common builder.
#![allow(dead_code)]
use super::*;
pub(crate) fn document_page(page_num: u32) -> CompositionPageRef {
    CompositionPageRef::PdfPage { page_num }
}
pub(crate) fn four_page_book(direction: ReadingDirection) -> BookComposition {
    BookComposition::new(direction, (0..4).map(document_page).collect())
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
pub(crate) fn hidden_blank_deletion_book(direction: ReadingDirection) -> BookComposition {
    four_page_book(direction)
        .edited(
            &PageDomain::Document,
            CompositionEdit::SetOverride {
                page: document_page(1),
                directive: Some(PageOverride::BlankOpposite {
                    page_side: PageSide::Leading,
                }),
            },
        )
        .unwrap()
        .edited(
            &PageDomain::Document,
            CompositionEdit::SetHidden {
                page: document_page(1),
                hidden: true,
            },
        )
        .unwrap()
}
