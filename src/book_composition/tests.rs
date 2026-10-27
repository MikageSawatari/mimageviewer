use super::fixtures::*;
use super::*;

fn resolve(book: &BookComposition, manifest: &[ManifestPage]) -> ResolvedComposition {
    resolve_composition(book, &PageDomain::Document, manifest).unwrap()
}
fn numbers(projection: &ResolvedComposition) -> Vec<u32> {
    projection
        .visible_pages
        .iter()
        .map(|page| match page.page {
            CompositionPageRef::PdfPage { page_num } => page_num,
            _ => unreachable!(),
        })
        .collect()
}

#[test]
fn record_round_trip_saves_only_binding_order_hidden_and_directives() {
    let mut book = four_page_book(ReadingDirection::Rtl);
    book.revision = 42;
    book.cover_enabled = true;
    book.page_order.rotate_left(2);
    book.hidden_pages.push(document_page(1));
    book.page_overrides = vec![
        PageOverrideEntry {
            page: document_page(1),
            directive: PageOverride::Alone,
        },
        PageOverrideEntry {
            page: document_page(3),
            directive: PageOverride::BlankOpposite {
                page_side: PageSide::Trailing,
            },
        },
    ];
    let bytes = book.to_json(&PageDomain::Document).unwrap();
    assert_eq!(
        BookComposition::from_json(&bytes, &PageDomain::Document).unwrap(),
        book
    );
    let wire = String::from_utf8(bytes).unwrap();
    for forbidden in [
        "units",
        "item_index",
        "placement",
        "supplement",
        "dimensions",
    ] {
        assert!(!wire.contains(forbidden), "{forbidden}");
    }
    let projection = resolve(&book, &document_manifest(4));
    assert_eq!(numbers(&projection), [2, 3, 0]);
    assert_eq!(
        projection.page_override(3),
        Some(PageOverride::BlankOpposite {
            page_side: PageSide::Trailing
        })
    );
    assert_eq!(projection.page_override(1), None);
    assert!(projection.cover_enabled);
    assert_eq!(projection.revision, 42);
}

#[test]
fn hidden_missing_policy_filtered_and_new_tail_retain_distinct_statuses() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.hidden_pages.push(document_page(0));
    book.page_overrides.push(PageOverrideEntry {
        page: document_page(1),
        directive: PageOverride::Alone,
    });
    let mut manifest = document_manifest(6);
    manifest.remove(1);
    manifest[1].availability = ManifestAvailability::ExcludedByPolicy;
    manifest[2].availability = ManifestAvailability::FilteredOut;
    let before = book.clone();
    let projection = resolve(&book, &manifest);
    assert_eq!(numbers(&projection), [4, 5]);
    for (page, status) in [
        (0, PageStatus::HiddenByComposition),
        (1, PageStatus::Missing),
        (2, PageStatus::ExcludedByPolicy),
        (3, PageStatus::FilteredOut),
    ] {
        assert_eq!(projection.page_status(&document_page(page)), status);
    }
    assert_eq!(
        projection.new_tail,
        vec![document_page(4), document_page(5)]
    );
    assert!(!projection.complete);
    assert_eq!(book, before);
    assert_eq!(
        resolve(&book, &document_manifest(6)).page_override(1),
        Some(PageOverride::Alone)
    );
}

#[test]
fn normalized_order_hidden_and_override_duplicates_are_rejected() {
    let page = |path: &str| CompositionPageRef::ArchiveEntry {
        entry_name: path.into(),
    };
    let domain = PageDomain::ArchiveLevel {
        prefix: "chapter".into(),
    };
    let mut book = BookComposition::new(ReadingDirection::Ltr, vec![page("Chapter/Page.JPG")]);
    book.page_order.push(page("chapter\\PAGE.jpg"));
    assert!(book.validate(&domain).is_err());
    book.page_order.pop();
    book.hidden_pages = vec![page("Chapter/Page.JPG"), page("chapter/page.jpg")];
    assert!(book.validate(&domain).is_err());
    book.hidden_pages.clear();
    book.page_overrides = vec![
        PageOverrideEntry {
            page: page("Chapter/Page.JPG"),
            directive: PageOverride::Alone,
        },
        PageOverrideEntry {
            page: page("chapter/page.jpg"),
            directive: PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
        },
    ];
    assert!(book.validate(&domain).is_err());
    let bytes = serde_json::to_vec(&book).unwrap();
    assert!(BookComposition::from_json(&bytes, &domain).is_err());
    book.page_overrides.truncate(1);
    book.validate(&domain).unwrap();
    book.page_overrides[0].page = page("chapter/other.jpg");
    assert!(book.validate(&domain).is_err());
}

#[test]
fn consecutive_blank_directives_are_valid_and_hidden_override_is_retained() {
    let mut book = four_page_book(ReadingDirection::Rtl);
    book.page_overrides = (0..4)
        .map(|n| PageOverrideEntry {
            page: document_page(n),
            directive: PageOverride::BlankOpposite {
                page_side: if n % 2 == 0 {
                    PageSide::Leading
                } else {
                    PageSide::Trailing
                },
            },
        })
        .collect();
    book.hidden_pages.push(document_page(1));
    book.validate(&PageDomain::Document).unwrap();
    assert_eq!(
        book.page_override(&document_page(1)),
        Some(PageOverride::BlankOpposite {
            page_side: PageSide::Trailing
        })
    );
    assert_eq!(
        resolve(&book, &document_manifest(4)).page_overrides.len(),
        3
    );
}

#[test]
fn unsafe_refs_unknown_fields_schema_variants_and_limits_rejected() {
    let domain = PageDomain::ArchiveLevel {
        prefix: String::new(),
    };
    for name in [
        "../a.jpg",
        "/a.jpg",
        "C:/a.jpg",
        "a//b.jpg",
        "a/./b.jpg",
        "a\u{0}.jpg",
    ] {
        let book = BookComposition::new(
            ReadingDirection::Ltr,
            vec![CompositionPageRef::ArchiveEntry {
                entry_name: name.into(),
            }],
        );
        assert!(book.validate(&domain).is_err(), "{name:?}");
    }
    let mut wire = serde_json::to_value(four_page_book(ReadingDirection::Ltr)).unwrap();
    wire["units"] = serde_json::json!([]);
    assert!(
        BookComposition::from_json(&serde_json::to_vec(&wire).unwrap(), &PageDomain::Document)
            .is_err()
    );
    wire.as_object_mut().unwrap().remove("units");
    wire["schema_version"] = serde_json::json!(1);
    assert!(matches!(
        BookComposition::from_json(&serde_json::to_vec(&wire).unwrap(), &PageDomain::Document),
        Err(CompositionError::UnknownSchema(1))
    ));
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.page_order = (0..=MAX_PAGES as u32).map(document_page).collect();
    assert!(matches!(
        book.validate(&PageDomain::Document),
        Err(CompositionError::Limit(_))
    ));
    assert!(
        BookComposition::from_json(&vec![b' '; MAX_JSON_BYTES + 1], &PageDomain::Document).is_err()
    );
}

#[test]
fn reindex_and_physical_deletion_remap_override_without_transferring_identity() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.page_overrides.push(PageOverrideEntry {
        page: document_page(2),
        directive: PageOverride::Alone,
    });
    let projection = resolve(&book, &document_manifest(4));
    let reordered = projection
        .reindex_items(&[Some(3), Some(2), Some(1), Some(0)])
        .unwrap();
    assert_eq!(reordered.page_override(1), Some(PageOverride::Alone));
    assert_eq!(reordered.page_override(2), None);
    let deleted = projection
        .remap_removed_items(&[Some(0), None, Some(1), Some(2)], &[document_page(1)])
        .unwrap();
    assert_eq!(deleted.page_override(1), Some(PageOverride::Alone));
    assert_eq!(deleted.page_status(&document_page(1)), PageStatus::Missing);
    assert_eq!(numbers(&deleted), [0, 2, 3]);
    assert_eq!(book.page_order.len(), 4);
    let removed_override = projection
        .remap_removed_items(&[Some(0), Some(1), None, Some(2)], &[document_page(2)])
        .unwrap();
    assert!(removed_override.page_overrides.is_empty());
    assert_eq!(
        book.page_override(&document_page(2)),
        Some(PageOverride::Alone)
    );
    assert!(
        projection
            .remap_removed_items(&[Some(0), None, Some(1), Some(2)], &[document_page(2)])
            .is_err()
    );
    assert!(
        projection
            .reindex_items(&[Some(0), Some(0), Some(1), Some(2)])
            .is_err()
    );
}

#[test]
fn manifest_collisions_refuse_resolution_without_changing_saved_record() {
    let book = four_page_book(ReadingDirection::Ltr);
    let mut manifest = document_manifest(4);
    manifest.push(manifest[0].clone());
    assert!(resolve_composition(&book, &PageDomain::Document, &manifest).is_err());
    manifest.pop();
    manifest[1].availability = ManifestAvailability::Present { item_index: 0 };
    assert!(resolve_composition(&book, &PageDomain::Document, &manifest).is_err());
    assert_eq!(book, four_page_book(ReadingDirection::Ltr));
}

#[test]
fn deletion_identities_cover_policy_facet_tail_and_reject_inconsistent_visible_maps() {
    let book = four_page_book(ReadingDirection::Ltr);
    for availability in [
        ManifestAvailability::ExcludedByPolicy,
        ManifestAvailability::FilteredOut,
    ] {
        let mut manifest = document_manifest(5);
        manifest[1].availability = availability;
        manifest[4].availability = availability;
        let before = resolve(&book, &manifest);
        let deleted = before
            .remap_removed_items(
                &[Some(0), None, Some(1), Some(2), None],
                &[document_page(1), document_page(4)],
            )
            .unwrap();
        let manifest = vec![
            manifest[0].clone(),
            ManifestPage {
                page: document_page(2),
                availability: ManifestAvailability::Present { item_index: 1 },
            },
            ManifestPage {
                page: document_page(3),
                availability: ManifestAvailability::Present { item_index: 2 },
            },
        ];
        assert_eq!(deleted, resolve(&book, &manifest));
        assert_eq!(deleted.page_status(&document_page(1)), PageStatus::Missing);
        assert_eq!(deleted.page_status(&document_page(4)), PageStatus::Invalid);
        assert!(deleted.complete);
    }
    let projection = resolve(&book, &document_manifest(4));
    for removed in [
        vec![],
        vec![document_page(0)],
        vec![document_page(1), document_page(1)],
        vec![document_page(99)],
    ] {
        assert!(
            projection
                .remap_removed_items(&[Some(0), None, Some(1), Some(2)], &removed)
                .is_err()
        );
    }
    assert!(
        projection
            .remap_removed_items(&[Some(0), Some(1), Some(2), Some(3)], &[document_page(1)])
            .is_err()
    );
}

#[test]
fn deleting_unsaved_tail_matches_fresh_manifest_and_other_projection_is_unchanged() {
    let book = four_page_book(ReadingDirection::Ltr);
    let before = resolve(&book, &document_manifest(5));
    let sibling = before.clone();
    let deleted = before
        .remap_removed_items(
            &[Some(0), Some(1), Some(2), Some(3), None],
            &[document_page(4)],
        )
        .unwrap();
    assert_eq!(deleted, resolve(&book, &document_manifest(4)));
    assert_eq!(before, sibling);
    assert_eq!(deleted.page_status(&document_page(4)), PageStatus::Invalid);
}

#[test]
fn epub_edit_key_equivalence_uses_original_source_and_same_page_after_reconversion() {
    use std::path::{Path, PathBuf};
    let source = Path::new("C:/Books/Original.EPUB");
    let reference = document_page(7);
    let expected = crate::adjustment_db::zip_entry_key(source, "page_7");
    for (generation, content_by_number, expected_content) in [
        ("cache/generation-1.pdf", [(7, "A"), (8, "B")], "A"),
        ("cache/generation-2.pdf", [(7, "B"), (8, "A")], "B"),
    ] {
        // Physical target/content are deliberately different and absent from the
        // production reference/resolver inputs; no reflow mapping is introduced.
        let physical_target = PathBuf::from(generation);
        assert_ne!(physical_target, source);
        let item = crate::app::book_materialization::document_page_item(source, 7);
        let remote = mimageviewer_ipc::RemoteSubresource::PdfPage { page_number: 7 };
        assert_eq!(reference.document_edit_key(source), Some(expected.clone()));
        assert_eq!(
            reference.document_edit_key(source),
            crate::edit_source::page_key_for_grid_item(&item)
        );
        assert_eq!(
            reference.document_edit_key(source),
            crate::edit_source::page_key_for_remote(source, &remote)
        );
        assert_eq!(
            reference.resume_identity(source),
            crate::book_resume_db::PageIdentity::Document {
                container: crate::path_key::normalize_keep_drive(source),
                page_num: 7
            }
        );
        assert_eq!(
            reference.bookmark_identity(),
            crate::book_bookmarks::PageIdentity::PdfPage(7)
        );
        let book = BookComposition::new(ReadingDirection::Ltr, vec![document_page(7)]);
        let manifest: Vec<_> = content_by_number
            .iter()
            .enumerate()
            .map(|(item_index, (page_num, _))| ManifestPage {
                page: document_page(*page_num),
                availability: ManifestAvailability::Present { item_index },
            })
            .collect();
        let projection = resolve(&book, &manifest);
        assert_eq!(projection.visible_pages[0].page, reference);
        assert_eq!(projection.visible_pages[0].item_index, 0);
        // A changed reflow still selects number 7, and thus B in generation 2.
        // It does not search the new document for the old content A at number 8.
        assert_eq!(
            content_by_number[projection.visible_pages[0].item_index].1,
            expected_content
        );
    }
    assert_ne!(
        reference.document_edit_key(source),
        document_page(8).document_edit_key(source)
    );
}

#[test]
fn direct_scope_kind_folder_devices_and_archive_spelling_are_validated() {
    let folder = |name: &str| CompositionPageRef::RelativePath { path: name.into() };
    let archive = |name: &str| CompositionPageRef::ArchiveEntry {
        entry_name: name.into(),
    };
    for name in [
        "CON.jpg",
        "lpt1",
        "aux.txt",
        "COM?.jpg",
        "a:b.jpg",
        "b?.jpg",
        "tail. ",
        "sub/a.jpg",
    ] {
        assert!(
            BookComposition::new(ReadingDirection::Ltr, vec![folder(name)])
                .validate(&PageDomain::ImageFolder)
                .is_err(),
            "{name}"
        );
    }
    let domain = PageDomain::ArchiveLevel {
        prefix: "chapter".into(),
    };
    for page in [
        folder("a.jpg"),
        document_page(0),
        archive("other/a.jpg"),
        archive("chapter/sub/a.jpg"),
    ] {
        assert!(
            BookComposition::new(ReadingDirection::Ltr, vec![page])
                .validate(&domain)
                .is_err()
        );
    }
    let original = archive("Chapter/???:A.JPG");
    let book = BookComposition::new(ReadingDirection::Rtl, vec![original.clone()]);
    book.validate(&domain).unwrap();
    let actual = archive("CHAPTER/???:a.jpg");
    let resolved = resolve_composition(
        &book,
        &domain,
        &[ManifestPage {
            page: actual.clone(),
            availability: ManifestAvailability::Present { item_index: 0 },
        }],
    )
    .unwrap();
    assert_eq!(resolved.visible_pages[0].page, actual);
    assert_eq!(book.page_order, vec![original]);
}

#[test]
fn reference_byte_limit_and_unknown_override_shapes_are_rejected() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    let mut wire = serde_json::to_value(&book).unwrap();
    for directive in [
        serde_json::json!({"kind":"automatic"}),
        serde_json::json!({"kind":"blank_opposite"}),
        serde_json::json!({"kind":"alone","placement":"leading"}),
        serde_json::json!({"kind":"blank_opposite","page_side":"center"}),
    ] {
        wire["page_overrides"] =
            serde_json::json!([{"page":{"kind":"pdf_page","page_num":0},"directive":directive}]);
        assert!(
            BookComposition::from_json(&serde_json::to_vec(&wire).unwrap(), &PageDomain::Document)
                .is_err()
        );
    }
    book.page_order = vec![CompositionPageRef::ArchiveEntry {
        entry_name: "a".repeat(MAX_REFERENCE_BYTES + 1),
    }];
    assert!(matches!(
        book.validate(&PageDomain::ArchiveLevel {
            prefix: String::new()
        }),
        Err(CompositionError::Limit("reference bytes"))
    ));
}

#[test]
fn maximum_document_record_and_repeated_visibility_directive_edits_remain_valid() {
    let book = BookComposition::new(
        ReadingDirection::Rtl,
        (0..MAX_PAGES as u32).map(document_page).collect(),
    );
    book.validate(&PageDomain::Document).unwrap();
    let projection = resolve(&book, &document_manifest(MAX_PAGES as u32));
    assert_eq!(projection.visible_pages.len(), MAX_PAGES);
    assert!(projection.complete);
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.hidden_pages = vec![document_page(1), document_page(0)];
    book.page_overrides = vec![
        PageOverrideEntry {
            page: document_page(1),
            directive: PageOverride::Alone,
        },
        PageOverrideEntry {
            page: document_page(0),
            directive: PageOverride::Alone,
        },
    ];
    assert_eq!(
        book.edited(
            &PageDomain::Document,
            CompositionEdit::SetHidden {
                page: document_page(1),
                hidden: true
            }
        )
        .unwrap(),
        book
    );
    assert_eq!(
        book.edited(
            &PageDomain::Document,
            CompositionEdit::SetOverride {
                page: document_page(1),
                directive: Some(PageOverride::Alone)
            }
        )
        .unwrap(),
        book
    );
}

#[test]
fn cumulative_json_byte_limit_and_hidden_subset_are_validated_before_writes() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.hidden_pages = vec![document_page(9)];
    assert!(book.validate(&PageDomain::Document).is_err());
    let prefix = "a".repeat(MAX_REFERENCE_BYTES - 32);
    let page_order = (0..600)
        .map(|n| CompositionPageRef::ArchiveEntry {
            entry_name: format!("{prefix}{n}.jpg"),
        })
        .collect();
    let book = BookComposition::new(ReadingDirection::Ltr, page_order);
    assert!(matches!(
        book.validate(&PageDomain::ArchiveLevel {
            prefix: String::new()
        }),
        Err(CompositionError::Limit("JSON bytes"))
    ));
}
