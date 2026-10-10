use super::fixtures::*;
use super::*;

fn resolve(book: &BookComposition, manifest: &[ManifestPage]) -> ResolvedComposition {
    resolve_composition(book, &PageDomain::Document, manifest).unwrap()
}

fn visible_numbers(projection: &ResolvedComposition) -> Vec<u32> {
    projection
        .visible_pages
        .iter()
        .map(|page| {
            let CompositionPageRef::PdfPage { page_num } = page.page else {
                panic!("document fixture")
            };
            page_num
        })
        .collect()
}

fn assert_single(
    unit: &ResolvedUnit,
    number: u32,
    index: usize,
    placement: SinglePlacement,
    companion: ResolvedCompanion,
) {
    assert_eq!(
        *unit,
        ResolvedUnit::Single {
            page: ResolvedPage {
                page: document_page(number),
                item_index: index
            },
            placement,
            companion
        }
    );
}

#[test]
fn all_unit_roles_and_hidden_round_trip_without_physical_keys() {
    let book = BookComposition {
        schema_version: SCHEMA_VERSION,
        revision: 42,
        direction: ReadingDirection::Rtl,
        cover: Some(document_page(0)),
        units: vec![
            single(0, SinglePlacement::Center, SingleCompanion::Vacant),
            pair(1, 2),
            single(3, SinglePlacement::Leading, SingleCompanion::Blank),
            single(
                4,
                SinglePlacement::Trailing,
                SingleCompanion::FrontCoverSupplement {
                    page: document_page(0),
                },
            ),
        ],
        hidden_pages: vec![document_page(2)],
    };
    let json = book.to_json(&PageDomain::Document).unwrap();
    assert_eq!(
        BookComposition::from_json(&json, &PageDomain::Document).unwrap(),
        book
    );
    assert!(!String::from_utf8(json).unwrap().contains("item_index"));
    let projection = resolve(&book, &document_manifest(5));
    assert_eq!(visible_numbers(&projection), [0, 1, 3, 4]);
    assert_eq!(projection.revision, 42);
    assert_single(
        &projection.units[1],
        1,
        1,
        SinglePlacement::Leading,
        ResolvedCompanion::Vacant,
    );
    assert_single(
        &projection.units[2],
        3,
        3,
        SinglePlacement::Leading,
        ResolvedCompanion::Blank,
    );
    assert_eq!(
        projection.page_status(&document_page(2)),
        PageStatus::HiddenByComposition
    );
    assert_eq!(
        projection.units.last().unwrap().anchor().page,
        document_page(4)
    );
    assert_eq!(
        projection.units.last().unwrap().navigation_pages().count(),
        1
    );
}

#[test]
fn unsafe_relative_and_archive_references_are_rejected() {
    for path in [
        "",
        "..",
        "../A.png",
        "a/../../A.png",
        "/A.png",
        "\\\\server\\A.png",
        "C:\\A.png",
        "C:A.png",
        "a\0.png",
        "a\n.png",
        "a//b.png",
        "./a.png",
        "a./b.png",
        "a /b.png",
        "a.png/",
        "\\?\\C:\\a.png",
    ] {
        let relative = BookComposition::new(
            ReadingDirection::Ltr,
            vec![CompositionUnit::Single {
                page: CompositionPageRef::RelativePath { path: path.into() },
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            }],
        );
        assert!(
            relative.validate(&PageDomain::ImageFolder).is_err(),
            "{path:?}"
        );
        let archive = BookComposition::new(
            ReadingDirection::Ltr,
            vec![CompositionUnit::Single {
                page: CompositionPageRef::ArchiveEntry {
                    entry_name: path.into(),
                },
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            }],
        );
        assert!(
            archive
                .validate(&PageDomain::ArchiveLevel {
                    prefix: String::new()
                })
                .is_err(),
            "{path:?}"
        );
    }
}

#[test]
fn kind_and_direct_scope_membership_are_validated_even_when_missing() {
    let page = CompositionPageRef::ArchiveEntry {
        entry_name: "Outer/CH01.zip/A.PNG".into(),
    };
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionUnit::Single {
            page: page.clone(),
            placement: SinglePlacement::Leading,
            companion: SingleCompanion::Blank,
        }],
    );
    let domain = PageDomain::ArchiveLevel {
        prefix: "outer\\ch01.ZIP".into(),
    };
    book.validate(&domain).unwrap();
    let projection = resolve_composition(&book, &domain, &[]).unwrap();
    assert_eq!(projection.page_status(&page), PageStatus::Missing);
    assert!(projection.units.is_empty());
    for prefix in ["", "outer", "other/ch01.zip", "../outer", "outer/ch01.zip/"] {
        assert!(
            book.validate(&PageDomain::ArchiveLevel {
                prefix: prefix.into()
            })
            .is_err()
        );
    }
    assert!(book.validate(&PageDomain::Document).is_err());
    assert!(
        four_page_book(ReadingDirection::Ltr)
            .validate(&domain)
            .is_err()
    );
}

#[test]
fn duplicates_use_bookmark_case_slash_rules_and_preserve_read_spelling() {
    let domain = PageDomain::ArchiveLevel {
        prefix: "chapter".into(),
    };
    let page = CompositionPageRef::ArchiveEntry {
        entry_name: "CHAPTER\\A.png".into(),
    };
    let alias = CompositionPageRef::ArchiveEntry {
        entry_name: "chapter/a.PNG".into(),
    };
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionUnit::Single {
            page: page.clone(),
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    );
    let manifest = vec![ManifestPage {
        page: alias.clone(),
        availability: ManifestAvailability::Present { item_index: 6 },
    }];
    let resolved = resolve_composition(&book, &domain, &manifest).unwrap();
    assert_eq!(resolved.visible_pages[0].page, alias);
    assert_eq!(
        resolved.page_status(&page),
        PageStatus::Visible { item_index: 6 }
    );
    assert!(resolved.new_tail.is_empty());
    let mut duplicate = book.clone();
    duplicate.units.push(CompositionUnit::Single {
        page: alias.clone(),
        placement: SinglePlacement::Center,
        companion: SingleCompanion::Vacant,
    });
    assert!(duplicate.validate(&domain).is_err());
    duplicate = book.clone();
    duplicate.hidden_pages = vec![page, alias];
    assert!(duplicate.validate(&domain).is_err());
    let mut bad_manifest = manifest.clone();
    bad_manifest.extend(manifest);
    assert!(resolve_composition(&book, &domain, &bad_manifest).is_err());
}

#[test]
fn schema_unknown_fields_and_malformed_blank_shapes_are_rejected() {
    let domain = PageDomain::Document;
    let book = four_page_book(ReadingDirection::Ltr);
    let mut unknown = book.clone();
    unknown.schema_version = 2;
    assert_eq!(
        unknown.validate(&domain),
        Err(CompositionError::UnknownSchema(2))
    );
    let mut value = serde_json::to_value(&book).unwrap();
    value["schema_version"] = 2.into();
    assert_eq!(
        BookComposition::from_json(&serde_json::to_vec(&value).unwrap(), &domain),
        Err(CompositionError::UnknownSchema(2))
    );
    value["schema_version"] = 1.into();
    for units in [
        serde_json::json!([{"kind":"blank"}]),
        serde_json::json!([{"kind":"pair","first":{"kind":"blank"},"second":{"kind":"blank"}}]),
        serde_json::json!([{"kind":"single","placement":"leading","companion":{"kind":"blank"}}]),
        serde_json::json!([{"kind":"single","page":{"kind":"pdf_page","page_num":0},"placement":"leading","companion":[{"kind":"blank"},{"kind":"blank"}]}]),
        serde_json::json!([{"kind":"single","page":{"kind":"pdf_page","page_num":0},"placement":"leading","companion":{"kind":"blank"},"second_companion":{"kind":"blank"}}]),
    ] {
        value["units"] = units;
        assert!(BookComposition::from_json(&serde_json::to_vec(&value).unwrap(), &domain).is_err());
    }
    let mut value = serde_json::to_value(book).unwrap();
    value["future_field"] = true.into();
    assert!(BookComposition::from_json(&serde_json::to_vec(&value).unwrap(), &domain).is_err());
}

#[test]
fn all_three_model_limits_are_enforced_on_read_and_draft() {
    let domain = PageDomain::Document;
    let count = BookComposition::new(
        ReadingDirection::Ltr,
        (0..=MAX_PAGES as u32)
            .map(|page| single(page, SinglePlacement::Center, SingleCompanion::Vacant))
            .collect(),
    );
    assert_eq!(
        count.validate(&domain),
        Err(CompositionError::Limit("page count"))
    );
    let oversized = vec![b' '; MAX_JSON_BYTES + 1];
    assert_eq!(
        BookComposition::from_json(&oversized, &domain),
        Err(CompositionError::Limit("JSON bytes"))
    );
    let path = "a".repeat(MAX_REFERENCE_BYTES + 1);
    assert_eq!(
        PageDomain::ImageFolder.validate_page(&CompositionPageRef::RelativePath { path }),
        Err(CompositionError::Limit("reference bytes"))
    );
    let large = BookComposition::new(
        ReadingDirection::Ltr,
        (0..520)
            .map(|page| CompositionUnit::Single {
                page: CompositionPageRef::RelativePath {
                    path: format!("{page:04}{}", "a".repeat(MAX_REFERENCE_BYTES - 4)),
                },
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            })
            .collect(),
    );
    assert_eq!(
        large.validate(&PageDomain::ImageFolder),
        Err(CompositionError::Limit("JSON bytes"))
    );
    assert_eq!(
        large.to_json(&PageDomain::ImageFolder),
        Err(CompositionError::Limit("JSON bytes"))
    );
}

#[test]
fn near_limit_document_composition_is_valid_and_linear_projection_covers_all_pages() {
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        (0..MAX_PAGES as u32)
            .map(|page| single(page, SinglePlacement::Center, SingleCompanion::Vacant))
            .collect(),
    );
    let projection = resolve(&book, &document_manifest(MAX_PAGES as u32));
    assert_eq!(projection.visible_pages.len(), MAX_PAGES);
    assert!(projection.complete);
    assert_eq!(
        resolve_composition(
            &book,
            &PageDomain::Document,
            &[ManifestPage {
                page: document_page(MAX_PAGES as u32),
                availability: ManifestAvailability::Present { item_index: 0 }
            }]
        ),
        Err(CompositionError::Limit("resolved page count"))
    );
}

#[test]
fn unit_boundary_consecutive_blanks_rejected_for_both_directions() {
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let book = BookComposition::new(
            direction,
            vec![
                single(0, SinglePlacement::Leading, SingleCompanion::Blank),
                single(1, SinglePlacement::Trailing, SingleCompanion::Blank),
            ],
        );
        assert_eq!(
            book.validate(&PageDomain::Document),
            Err(CompositionError::Invalid("consecutive blank slots"))
        );
        let mut separated = book.clone();
        separated.units.insert(
            1,
            single(2, SinglePlacement::Center, SingleCompanion::Vacant),
        );
        separated.validate(&PageDomain::Document).unwrap();
    }
}

#[test]
fn second_blank_and_move_or_placement_creating_consecutive_blanks_leave_draft_unchanged() {
    let domain = PageDomain::Document;
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![
            single(0, SinglePlacement::Leading, SingleCompanion::Blank),
            single(2, SinglePlacement::Center, SingleCompanion::Vacant),
            single(1, SinglePlacement::Trailing, SingleCompanion::Blank),
        ],
    );
    let before = book.clone();
    assert!(
        book.edited(
            &domain,
            UnitEdit::InsertBlank {
                page: document_page(0),
                placement: SinglePlacement::Leading
            }
        )
        .is_err()
    );
    assert!(
        book.edited(&domain, UnitEdit::MoveUnit { from: 2, to: 1 })
            .is_err()
    );
    assert!(
        book.edited(
            &domain,
            UnitEdit::MovePage {
                page: document_page(1),
                to: 1
            }
        )
        .is_err()
    );
    let valid = BookComposition::new(
        ReadingDirection::Ltr,
        vec![
            single(0, SinglePlacement::Leading, SingleCompanion::Blank),
            single(1, SinglePlacement::Leading, SingleCompanion::Blank),
        ],
    );
    assert!(
        valid
            .edited(
                &domain,
                UnitEdit::SetSingle {
                    page: document_page(1),
                    placement: SinglePlacement::Trailing,
                    companion: SingleCompanion::Blank
                }
            )
            .is_err()
    );
    assert_eq!(book, before);
}

#[test]
fn blank_survives_hidden_missing_and_restoration_without_a_fake_source() {
    let book = BookComposition::new(
        ReadingDirection::Rtl,
        vec![single(0, SinglePlacement::Trailing, SingleCompanion::Blank)],
    );
    let before = book.clone();
    let hidden = book
        .edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(0),
                hidden: true,
            },
        )
        .unwrap();
    assert!(resolve(&hidden, &document_manifest(1)).units.is_empty());
    assert!(resolve(&book, &[]).units.is_empty());
    assert_eq!(
        resolve(&book, &[]).page_status(&document_page(0)),
        PageStatus::Missing
    );
    assert_eq!(
        BookComposition::from_json(
            &hidden.to_json(&PageDomain::Document).unwrap(),
            &PageDomain::Document
        )
        .unwrap(),
        hidden
    );
    let shown = hidden
        .edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(0),
                hidden: false,
            },
        )
        .unwrap();
    assert_eq!(shown, before);
    assert_single(
        &resolve(&shown, &document_manifest(1)).units[0],
        0,
        0,
        SinglePlacement::Trailing,
        ResolvedCompanion::Blank,
    );
    assert_eq!(book, before);
}

#[test]
fn hidden_subset_cover_and_companion_consistency_are_required() {
    let domain = PageDomain::Document;
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.hidden_pages.push(document_page(4));
    assert!(book.validate(&domain).is_err());
    book.hidden_pages.clear();
    book.cover = Some(document_page(0));
    assert!(book.validate(&domain).is_err());
    for companion in [
        SingleCompanion::Blank,
        SingleCompanion::FrontCoverSupplement {
            page: document_page(0),
        },
    ] {
        let book = BookComposition::new(
            ReadingDirection::Ltr,
            vec![single(0, SinglePlacement::Center, companion)],
        );
        assert!(book.validate(&domain).is_err());
    }
    let mut book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![
            single(0, SinglePlacement::Center, SingleCompanion::Vacant),
            single(
                1,
                SinglePlacement::Trailing,
                SingleCompanion::FrontCoverSupplement {
                    page: document_page(0),
                },
            ),
        ],
    );
    assert!(book.validate(&domain).is_err());
    book.cover = Some(document_page(2));
    assert!(book.validate(&domain).is_err());
    book.cover = Some(document_page(0));
    book.validate(&domain).unwrap();
    book.units[0] = single(
        0,
        SinglePlacement::Leading,
        SingleCompanion::FrontCoverSupplement {
            page: document_page(0),
        },
    );
    assert!(book.validate(&domain).is_err());
}

#[test]
fn hidden_missing_policy_and_facet_never_repair_pairs() {
    let original = four_page_book(ReadingDirection::Ltr);
    for status in [
        ManifestAvailability::ExcludedByPolicy,
        ManifestAvailability::FilteredOut,
    ] {
        let mut manifest = document_manifest(4);
        manifest[1].availability = status;
        let projection = resolve(&original, &manifest);
        assert_single(
            &projection.units[0],
            0,
            0,
            SinglePlacement::Leading,
            ResolvedCompanion::Vacant,
        );
        assert!(matches!(projection.units[1], ResolvedUnit::Pair { .. }));
        assert_eq!(visible_numbers(&projection), [0, 2, 3]);
        assert_eq!(
            projection.complete,
            status != ManifestAvailability::FilteredOut
        );
        assert_eq!(
            projection.page_status(&document_page(1)),
            match status {
                ManifestAvailability::ExcludedByPolicy => PageStatus::ExcludedByPolicy,
                _ => PageStatus::FilteredOut,
            }
        );
    }
    let hidden = original
        .edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(0),
                hidden: true,
            },
        )
        .unwrap();
    let projection = resolve(&hidden, &document_manifest(4));
    assert_single(
        &projection.units[0],
        1,
        1,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    let shown = hidden
        .edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(0),
                hidden: false,
            },
        )
        .unwrap();
    assert_eq!(
        resolve(&shown, &document_manifest(4)),
        resolve(&original, &document_manifest(4))
    );
}

#[test]
fn deletion_remap_preserves_c_d_and_b_missing_before_reload() {
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let book = four_page_book(direction);
        let before = book.clone();
        let projection = resolve(&book, &document_manifest(4));
        let remapped = projection
            .remap_removed_items(&[Some(0), None, Some(1), Some(2)])
            .unwrap();
        assert_single(
            &remapped.units[0],
            0,
            0,
            SinglePlacement::Leading,
            ResolvedCompanion::Vacant,
        );
        assert_eq!(
            remapped.units[1],
            ResolvedUnit::Pair {
                first: ResolvedPage {
                    page: document_page(2),
                    item_index: 1
                },
                second: ResolvedPage {
                    page: document_page(3),
                    item_index: 2
                }
            }
        );
        assert_eq!(visible_numbers(&remapped), [0, 2, 3]);
        assert_eq!(remapped.page_status(&document_page(1)), PageStatus::Missing);
        assert_eq!(remapped.units[1].anchor().item_index, 1);
        let mut manifest = document_manifest(4);
        manifest.remove(1);
        for (index, entry) in manifest.iter_mut().enumerate() {
            entry.availability = ManifestAvailability::Present { item_index: index };
        }
        assert_eq!(remapped, resolve(&book, &manifest));
        assert_eq!(book, before);
        assert_eq!(resolve(&book, &document_manifest(4)), projection);
    }
}

#[test]
fn remap_covers_both_pair_sides_empty_units_helpers_and_blank_partner() {
    let fixture = editor_unit_edits(ReadingDirection::Ltr);
    let projection = resolve(&fixture.with_cover_helper, &document_manifest(4));
    let mapped = projection
        .remap_removed_items(&[None, Some(0), Some(1), Some(2)])
        .unwrap();
    assert_single(
        mapped.units.last().unwrap(),
        3,
        2,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    let blank = resolve(&fixture.blank, &document_manifest(4));
    let mapped = blank
        .remap_removed_items(&[Some(0), None, Some(1), Some(2)])
        .unwrap();
    assert!(!mapped.units.iter().any(|unit| matches!(
        unit,
        ResolvedUnit::Single {
            companion: ResolvedCompanion::Blank,
            ..
        }
    )));
    assert!(matches!(
        fixture.blank.units[1],
        CompositionUnit::Single {
            companion: SingleCompanion::Blank,
            ..
        }
    ));
    let pair = resolve(&fixture.original, &document_manifest(4));
    let mapped = pair
        .remap_removed_items(&[None, Some(0), None, None])
        .unwrap();
    assert_eq!(mapped.units.len(), 1);
    assert_single(
        &mapped.units[0],
        1,
        0,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    assert!(
        pair.remap_removed_items(&[None; 4])
            .unwrap()
            .units
            .is_empty()
    );
    for map in [
        vec![],
        vec![Some(0)],
        vec![Some(0), Some(0), Some(1), Some(2)],
        vec![Some(1), Some(0), Some(2), Some(3)],
        vec![Some(4), None, None, None],
        vec![Some(1), Some(2), None, None],
    ] {
        assert!(pair.remap_removed_items(&map).is_err());
    }
}

#[test]
fn new_tail_is_source_order_centered_and_does_not_move_helpers() {
    let fixture = editor_unit_edits(ReadingDirection::Rtl);
    let book = fixture.with_cover_helper;
    let before = book.clone();
    let mut manifest = document_manifest(6);
    manifest.swap(4, 5);
    let projection = resolve(&book, &manifest);
    assert_eq!(projection.new_tail, [document_page(5), document_page(4)]);
    assert_eq!(visible_numbers(&projection), [0, 1, 2, 3, 5, 4]);
    assert!(matches!(
        projection.units[3],
        ResolvedUnit::Single {
            companion: ResolvedCompanion::FrontCoverSupplement(_),
            ..
        }
    ));
    assert_single(
        &projection.units[4],
        5,
        5,
        SinglePlacement::Center,
        ResolvedCompanion::Vacant,
    );
    assert_single(
        &projection.units[5],
        4,
        4,
        SinglePlacement::Center,
        ResolvedCompanion::Vacant,
    );
    assert_eq!(book, before);
}

#[test]
fn hidden_missing_and_excluded_cover_supplement_degrades_to_vacant_only() {
    let fixture = editor_unit_edits(ReadingDirection::Ltr);
    let domain = PageDomain::Document;
    let hidden = fixture
        .with_cover_helper
        .edited(
            &domain,
            UnitEdit::SetHidden {
                page: document_page(0),
                hidden: true,
            },
        )
        .unwrap();
    assert_single(
        resolve(&hidden, &document_manifest(4))
            .units
            .last()
            .unwrap(),
        3,
        3,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    let mut manifest = document_manifest(4);
    manifest[0].availability = ManifestAvailability::ExcludedByPolicy;
    assert_single(
        resolve(&fixture.with_cover_helper, &manifest)
            .units
            .last()
            .unwrap(),
        3,
        3,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    manifest.remove(0);
    assert_single(
        resolve(&fixture.with_cover_helper, &manifest)
            .units
            .last()
            .unwrap(),
        3,
        3,
        SinglePlacement::Trailing,
        ResolvedCompanion::Vacant,
    );
    assert!(matches!(
        resolve(&fixture.with_cover_helper, &document_manifest(4))
            .units
            .last()
            .unwrap(),
        ResolvedUnit::Single {
            companion: ResolvedCompanion::FrontCoverSupplement(_),
            ..
        }
    ));
}

#[test]
fn all_hidden_is_valid_empty_projection_and_never_adds_registered_pages_as_tail() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.hidden_pages = book.pages().cloned().collect();
    let projection = resolve(&book, &document_manifest(4));
    assert!(projection.visible_pages.is_empty());
    assert!(projection.units.is_empty());
    assert!(projection.new_tail.is_empty());
    assert!(projection.complete);
}

#[test]
fn reusable_editor_fixture_preserves_unedited_units_and_attaches_blank_to_real_page() {
    for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
        let fixture = editor_unit_edits(direction);
        assert_eq!(fixture.repaired, fixture.original);
        assert_eq!(fixture.split.units[2], fixture.original.units[1]);
        assert_eq!(fixture.moved.units[1], fixture.original.units[1]);
        assert_eq!(
            fixture.moved.units[2],
            single(1, SinglePlacement::Leading, SingleCompanion::Blank)
        );
        assert_eq!(fixture.hidden.units, fixture.moved.units);
        let shown = fixture
            .hidden
            .edited(
                &PageDomain::Document,
                UnitEdit::SetHidden {
                    page: document_page(1),
                    hidden: false,
                },
            )
            .unwrap();
        assert_eq!(shown, fixture.moved);
        assert_eq!(
            visible_numbers(&resolve(&fixture.moved, &document_manifest(4))),
            [0, 2, 3, 1]
        );
    }
}

#[test]
fn extracting_pair_page_retains_the_partner_slot_without_repairing_rest() {
    let book = four_page_book(ReadingDirection::Rtl);
    let moved = book
        .edited(
            &PageDomain::Document,
            UnitEdit::MovePage {
                page: document_page(1),
                to: 2,
            },
        )
        .unwrap();
    assert_eq!(
        moved.units,
        [
            single(0, SinglePlacement::Leading, SingleCompanion::Vacant),
            pair(2, 3),
            single(1, SinglePlacement::Trailing, SingleCompanion::Vacant)
        ]
    );
    let moved = book
        .edited(
            &PageDomain::Document,
            UnitEdit::MovePage {
                page: document_page(0),
                to: 2,
            },
        )
        .unwrap();
    assert_eq!(
        moved.units,
        [
            single(1, SinglePlacement::Trailing, SingleCompanion::Vacant),
            pair(2, 3),
            single(0, SinglePlacement::Leading, SingleCompanion::Vacant)
        ]
    );
}

#[test]
fn pairing_removes_blank_in_the_same_operation_and_cover_requires_explicit_clear() {
    let domain = PageDomain::Document;
    let fixture = editor_unit_edits(ReadingDirection::Ltr);
    let paired = fixture
        .blank
        .edited(
            &domain,
            UnitEdit::PairSingles {
                first_unit: 0,
                clear_cover: false,
            },
        )
        .unwrap();
    assert_eq!(paired, fixture.original);
    let with_cover = fixture
        .split
        .edited(&domain, UnitEdit::SetCover(Some(document_page(0))))
        .unwrap();
    assert!(
        with_cover
            .edited(
                &domain,
                UnitEdit::PairSingles {
                    first_unit: 0,
                    clear_cover: false
                }
            )
            .is_err()
    );
    assert_eq!(
        with_cover
            .edited(
                &domain,
                UnitEdit::PairSingles {
                    first_unit: 0,
                    clear_cover: true
                }
            )
            .unwrap(),
        fixture.original
    );
    let cleared = with_cover
        .edited(&domain, UnitEdit::SetCover(None))
        .unwrap();
    assert_eq!(
        cleared
            .edited(
                &domain,
                UnitEdit::PairSingles {
                    first_unit: 0,
                    clear_cover: false
                }
            )
            .unwrap(),
        fixture.original
    );
    let cleared = fixture
        .with_cover_helper
        .edited(&domain, UnitEdit::SetCover(None))
        .unwrap();
    assert!(cleared.units.iter().all(|unit| !matches!(
        unit,
        CompositionUnit::Single {
            companion: SingleCompanion::FrontCoverSupplement { .. },
            ..
        }
    )));
}

#[test]
fn direction_only_mirrors_screen_slots_and_does_not_change_unit_or_navigation_order() {
    let fixture = editor_unit_edits(ReadingDirection::Ltr);
    let rtl = fixture
        .moved
        .edited(
            &PageDomain::Document,
            UnitEdit::SetDirection(ReadingDirection::Rtl),
        )
        .unwrap();
    assert_eq!(rtl.units, fixture.moved.units);
    assert_eq!(rtl.revision, fixture.moved.revision);
    assert_eq!(
        visible_numbers(&resolve(&rtl, &document_manifest(4))),
        visible_numbers(&resolve(&fixture.moved, &document_manifest(4)))
    );
    assert_eq!(
        SinglePlacement::Leading.screen_slot(ReadingDirection::Ltr),
        ScreenSlot::Left
    );
    assert_eq!(
        SinglePlacement::Leading.screen_slot(ReadingDirection::Rtl),
        ScreenSlot::Right
    );
    assert_eq!(
        SinglePlacement::Trailing.screen_slot(ReadingDirection::Rtl),
        ScreenSlot::Left
    );
    assert_eq!(
        SinglePlacement::Center.screen_slot(ReadingDirection::Rtl),
        ScreenSlot::Center
    );
}

#[test]
fn invalid_editor_positions_and_targets_leave_original_revision_untouched() {
    let mut book = four_page_book(ReadingDirection::Ltr);
    book.revision = 19;
    let before = book.clone();
    for edit in [
        UnitEdit::MoveUnit { from: 0, to: 5 },
        UnitEdit::SplitPair { unit: 4 },
        UnitEdit::MovePage {
            page: document_page(1),
            to: 99,
        },
        UnitEdit::PairSingles {
            first_unit: usize::MAX,
            clear_cover: false,
        },
        UnitEdit::SetHidden {
            page: document_page(8),
            hidden: true,
        },
        UnitEdit::SetCover(Some(document_page(8))),
    ] {
        assert!(book.edited(&PageDomain::Document, edit).is_err());
        assert_eq!(book, before);
    }
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
        let book = BookComposition::new(
            ReadingDirection::Ltr,
            vec![single(7, SinglePlacement::Center, SingleCompanion::Vacant)],
        );
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
fn image_folder_reference_round_trip_adapters_and_windows_devices() {
    use std::path::Path;
    let page = CompositionPageRef::RelativePath {
        path: "Page.PNG".into(),
    };
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionUnit::Single {
            page: page.clone(),
            placement: SinglePlacement::Trailing,
            companion: SingleCompanion::Blank,
        }],
    );
    let domain = PageDomain::ImageFolder;
    assert_eq!(
        BookComposition::from_json(&book.to_json(&domain).unwrap(), &domain).unwrap(),
        book
    );
    assert_eq!(
        page.bookmark_identity(),
        crate::book_bookmarks::PageIdentity::RelativePath("Page.PNG".into())
    );
    assert_eq!(
        page.resume_identity(Path::new("C:/Book")),
        crate::book_resume_db::PageIdentity::Image {
            path: "c:/book/page.png".into()
        }
    );
    assert_ne!(
        page.resume_identity(Path::new("C:/Book")),
        page.resume_identity(Path::new("D:/Book"))
    );
    for path in [
        "CON.png",
        "NUL.jpg",
        "PRN.png",
        "COM1.jpg",
        "lpt9.png",
        "com¹.png",
        "A.png:stream",
        "A?.png",
    ] {
        assert!(
            domain
                .validate_page(&CompositionPageRef::RelativePath { path: path.into() })
                .is_err()
        );
    }
}

#[test]
fn archive_member_bytes_are_not_restricted_to_windows_filenames() {
    let domain = PageDomain::ArchiveLevel {
        prefix: "chapter.".into(),
    };
    let page = CompositionPageRef::ArchiveEntry {
        entry_name: "chapter./name:*?one|.PNG".into(),
    };
    let book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionUnit::Single {
            page: page.clone(),
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    );
    assert_eq!(
        BookComposition::from_json(&book.to_json(&domain).unwrap(), &domain).unwrap(),
        book
    );
    let projection = resolve_composition(
        &book,
        &domain,
        &[ManifestPage {
            page: page.clone(),
            availability: ManifestAvailability::Present { item_index: 0 },
        }],
    )
    .unwrap();
    assert_eq!(projection.visible_pages[0].page, page);
}

#[test]
fn adoption_reindex_keeps_units_helpers_and_targets_in_one_row_space() {
    let fixture = editor_unit_edits(ReadingDirection::Ltr);
    let projection = resolve(&fixture.moved, &document_manifest(4));
    let remapped = projection
        .reindex_items(&[Some(0), Some(3), Some(1), Some(2)])
        .unwrap();
    assert_eq!(
        remapped
            .visible_pages
            .iter()
            .map(|page| page.item_index)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert_eq!(
        remapped.page_status(&document_page(1)),
        PageStatus::Visible { item_index: 3 }
    );
    assert_single(
        &remapped.units[2],
        1,
        3,
        SinglePlacement::Leading,
        ResolvedCompanion::Blank,
    );
    let helper = resolve(&fixture.with_cover_helper, &document_manifest(4))
        .reindex_items(&[Some(3), Some(2), Some(1), Some(0)])
        .unwrap();
    assert_single(
        helper.units.last().unwrap(),
        3,
        0,
        SinglePlacement::Trailing,
        ResolvedCompanion::FrontCoverSupplement(ResolvedPage {
            page: document_page(0),
            item_index: 3,
        }),
    );
    assert!(
        projection
            .reindex_items(&[Some(0), None, Some(1), Some(2)])
            .is_err()
    );
    assert!(
        projection
            .reindex_items(&[Some(0), Some(0), Some(1), Some(2)])
            .is_err()
    );
    let hidden = resolve(&fixture.hidden, &document_manifest(4));
    assert_eq!(
        hidden
            .reindex_items(&[Some(0), None, Some(1), Some(2)])
            .unwrap()
            .page_status(&document_page(1)),
        PageStatus::HiddenByComposition
    );
}

#[test]
fn same_cover_and_visibility_edits_preserve_helpers_hidden_order_and_revision() {
    let fixture = editor_unit_edits(ReadingDirection::Rtl);
    let mut book = fixture.with_cover_helper;
    book.revision = 31;
    assert_eq!(
        book.edited(
            &PageDomain::Document,
            UnitEdit::SetCover(Some(document_page(0)))
        )
        .unwrap(),
        book
    );
    book.hidden_pages = vec![document_page(1), document_page(0)];
    assert_eq!(
        book.edited(
            &PageDomain::Document,
            UnitEdit::SetHidden {
                page: document_page(1),
                hidden: true
            }
        )
        .unwrap(),
        book
    );
}

#[test]
fn deleting_unsaved_tail_matches_fresh_manifest_and_other_projection_is_unchanged() {
    let book = four_page_book(ReadingDirection::Ltr);
    let before = resolve(&book, &document_manifest(5));
    let sibling = before.clone();
    let deleted = before
        .remap_removed_items(&[Some(0), Some(1), Some(2), Some(3), None])
        .unwrap();
    assert_eq!(deleted, resolve(&book, &document_manifest(4)));
    assert_eq!(before, sibling);
    assert_eq!(deleted.page_status(&document_page(4)), PageStatus::Invalid);
}

#[test]
fn manifest_index_collision_and_manifest_limit_are_rejected() {
    let book = four_page_book(ReadingDirection::Ltr);
    let mut manifest = document_manifest(4);
    manifest[1].availability = ManifestAvailability::Present { item_index: 0 };
    assert!(resolve_composition(&book, &PageDomain::Document, &manifest).is_err());
    assert_eq!(
        resolve_composition(
            &book,
            &PageDomain::Document,
            &document_manifest(MAX_PAGES as u32 + 1)
        ),
        Err(CompositionError::Limit("manifest pages"))
    );
}
