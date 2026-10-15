use super::*;
use crate::{
    app::book_materialization::{
        materialize_archive_level, project_archive_composition, project_document_composition,
    },
    book_resume_db::{PageIdentity, ReadingPosition},
    book_scope::BookScopeKey,
    file_type_visibility::FileTypeVisibility,
    grid_item::GridItem,
    settings::{GridDisplayOrder, SortOrder},
    zip_tree::ZipTree,
};
use std::{io::Write, path::Path};

fn token() -> CompositionToken {
    CompositionToken {
        service_epoch: 3,
        publication_sequence: 7,
    }
}

fn raw_document(count: u32, offset: usize) -> Vec<CanonicalRawPage> {
    (0..count)
        .map(|page_num| CanonicalRawPage {
            page: fixtures::document_page(page_num),
            raw_index: offset + page_num as usize,
        })
        .collect()
}

fn composed(outcome: &CompositionProjectionOutcome) -> &AcceptedCompositionProjection {
    let CompositionProjectionOutcome::Composed(projection) = outcome else {
        panic!("expected composed, got {outcome:?}");
    };
    projection
}

#[test]
fn b_page_block_reorders_reindexes_overrides_targets_metadata_and_canonical_raw_together() {
    let mut record = fixtures::four_page_book(ReadingDirection::Rtl);
    record.cover_enabled = true;
    record.page_overrides.push(PageOverrideEntry {
        page: fixtures::document_page(3),
        directive: PageOverride::Alone,
    });
    record.page_order.rotate_left(1);
    let snapshot = record.clone();
    let scope = BookScopeKey::document("C:/source/book.epub").unwrap();
    let manifest = fixtures::document_manifest(4)
        .into_iter()
        .map(|mut entry| {
            if let ManifestAvailability::Present { item_index } = &mut entry.availability {
                *item_index += 1;
            }
            entry
        })
        .collect::<Vec<_>>();
    let result = project_composition(
        scope.clone(),
        &PageDomain::Document,
        token(),
        Some(&record),
        &raw_document(4, 1),
        &manifest,
        6,
    )
    .unwrap();
    assert_eq!(result.output_to_input, [0, 2, 3, 4, 1, 5]);
    let accepted = composed(&result.outcome);
    assert_eq!(accepted.cover_enabled, record.cover_enabled);
    assert_eq!(accepted.scope, scope);
    assert_eq!(accepted.token, token());
    assert_eq!(accepted.canonical_raw_index_for_projected(4), Some(1));
    assert_eq!(accepted.reading_ordinal(4), Some(4));
    assert_eq!(
        accepted.resolve_position(&ReadingPosition {
            page: 1,
            identity: None
        }),
        PageStatus::Visible { item_index: 4 }
    );
    assert_eq!(
        accepted.resolve_position(&ReadingPosition {
            page: 4,
            identity: Some(PageIdentity::Document {
                container: "C:/SOURCE/BOOK.EPUB".into(),
                page_num: 0
            })
        }),
        PageStatus::Visible { item_index: 4 }
    );
    assert_eq!(
        accepted.resolved.page_override(3),
        Some(PageOverride::Alone)
    );
    assert_eq!(record, snapshot);
    // The worker adapter publishes the same identity/metadata permutation for real PDF rows.
    let pages = (0..4)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: scope.logical_root.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    let metas = (0..4).map(|n| Some((n, n + 10))).collect();
    let worker = project_document_composition(
        &scope.logical_root,
        4,
        (pages, metas),
        FileTypeVisibility::default(),
        token(),
        Some(&record),
    )
    .unwrap();
    assert_eq!(
        worker.metas,
        [Some((1, 11)), Some((2, 12)), Some((3, 13)), Some((0, 10))]
    );
    assert_eq!(
        worker.page_identities.resolve(&ReadingPosition {
            page: 123,
            identity: Some(fixtures::document_page(0).resume_identity(&scope.logical_root)),
        }),
        crate::app::file_type_resume::PositionResolution::Restored(3)
    );
}

#[test]
fn b_hidden_deletion_compacts_flat_order_and_keeps_saved_b_ref() {
    let original = fixtures::four_page_book(ReadingDirection::Ltr);
    let mut hidden = original.clone();
    hidden.hidden_pages.push(fixtures::document_page(1));
    let scope = BookScopeKey::document("C:/book.pdf").unwrap();
    let result = project_composition(
        scope.clone(),
        &PageDomain::Document,
        token(),
        Some(&hidden),
        &raw_document(4, 0),
        &fixtures::document_manifest(4),
        4,
    )
    .unwrap();
    assert_eq!(result.output_to_input, [0, 2, 3]);
    let accepted = composed(&result.outcome);
    assert_eq!(
        accepted.resolve_position(&ReadingPosition {
            page: 1,
            identity: None
        }),
        PageStatus::HiddenByComposition
    );
    // Projected slot 1 now belongs to C. A captured target for B must not
    // acquire C merely because the old integer slot still exists.
    let captured_b = ReadingPosition {
        page: 1,
        identity: Some(fixtures::document_page(1).resume_identity(&scope.logical_root)),
    };
    assert_eq!(
        accepted.resolve_position(&captured_b),
        PageStatus::HiddenByComposition
    );
    assert_eq!(
        accepted.position_for_projected(1).unwrap().identity,
        Some(fixtures::document_page(2).resume_identity(&scope.logical_root))
    );
    assert_eq!(
        accepted
            .resolved
            .visible_pages
            .iter()
            .map(|page| page.page.clone())
            .collect::<Vec<_>>(),
        [0, 2, 3].map(fixtures::document_page)
    );
    let full = project_composition(
        scope,
        &PageDomain::Document,
        token(),
        Some(&original),
        &raw_document(4, 0),
        &fixtures::document_manifest(4),
        4,
    )
    .unwrap();
    let deleted = composed(&full.outcome)
        .remap_removed_items(
            &[Some(0), None, Some(1), Some(2)],
            &[fixtures::document_page(1)],
        )
        .unwrap();
    assert_eq!(
        deleted.resolved.page_status(&fixtures::document_page(1)),
        PageStatus::Missing
    );
    assert_eq!(deleted.resolve_position(&captured_b), PageStatus::Missing);
    assert_eq!(deleted.canonical_raw_index_for_projected(1), Some(1));
    assert_eq!(
        deleted.position_for_projected(1).unwrap().identity,
        Some(fixtures::document_page(2).resume_identity(&deleted.scope.logical_root))
    );
    assert_eq!(
        deleted
            .resolved
            .visible_pages
            .iter()
            .map(|page| page.page.clone())
            .collect::<Vec<_>>(),
        [0, 2, 3].map(fixtures::document_page)
    );
    assert_eq!(original.pages().count(), 4);
    let reloaded_manifest = [0, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(item_index, page_num)| ManifestPage {
            page: fixtures::document_page(page_num),
            availability: ManifestAvailability::Present { item_index },
        })
        .collect::<Vec<_>>();
    let reloaded_raw = [0, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(raw_index, page_num)| CanonicalRawPage {
            page: fixtures::document_page(page_num),
            raw_index,
        })
        .collect::<Vec<_>>();
    let reloaded = project_composition(
        deleted.scope.clone(),
        &PageDomain::Document,
        token(),
        Some(&original),
        &reloaded_raw,
        &reloaded_manifest,
        3,
    )
    .unwrap();
    assert_eq!(composed(&reloaded.outcome).resolved, deleted.resolved);
}

#[test]
fn g3_all_hidden_remains_empty_and_blank_tail_policy_facet_statuses_are_distinct() {
    let mut record = fixtures::hidden_blank_deletion_book(ReadingDirection::Rtl);
    let saved = record.clone();
    let mut manifest = fixtures::document_manifest(6);
    manifest[2].availability = ManifestAvailability::ExcludedByPolicy;
    manifest[3].availability = ManifestAvailability::FilteredOut;
    manifest[4].availability = ManifestAvailability::Present { item_index: 2 };
    manifest[5].availability = ManifestAvailability::Present { item_index: 3 };
    let result = project_composition(
        BookScopeKey::document("D:/book.pdf").unwrap(),
        &PageDomain::Document,
        token(),
        Some(&record),
        &raw_document(6, 0),
        &manifest,
        4,
    )
    .unwrap();
    let projection = composed(&result.outcome);
    assert_eq!(
        projection.resolved.page_status(&fixtures::document_page(1)),
        PageStatus::HiddenByComposition
    );
    assert_eq!(
        projection.resolved.page_status(&fixtures::document_page(2)),
        PageStatus::ExcludedByPolicy
    );
    assert_eq!(
        projection.resolved.page_status(&fixtures::document_page(3)),
        PageStatus::FilteredOut
    );
    assert!(!projection.resolved.complete);
    assert_eq!(
        projection.resolved.new_tail,
        [fixtures::document_page(4), fixtures::document_page(5)]
    );
    assert_eq!(projection.resolved.visible_pages.len(), 3);
    assert_eq!(record, saved);
    record.hidden_pages = record.pages().cloned().collect();
    let empty = project_composition(
        projection.scope.clone(),
        &PageDomain::Document,
        token(),
        Some(&record),
        &raw_document(4, 0),
        &fixtures::document_manifest(4),
        4,
    )
    .unwrap();
    assert!(empty.output_to_input.is_empty());
    assert!(composed(&empty.outcome).resolved.visible_pages.is_empty());
    assert!(composed(&empty.outcome).resolved.complete);
}

#[test]
fn g3_unsafe_and_ambiguous_actual_archives_preserve_legacy_rows_source_bytes_and_saved_record() {
    for names in [
        vec!["a//b.jpg", "a/c.jpg"],
        vec!["/b.jpg", "c.jpg"],
        vec!["b\u{1f}.jpg", "c.jpg"],
        vec!["B.jpg", "b.jpg", "c.jpg"],
        vec!["a/B.jpg", "a/b.jpg", "a/c.jpg"],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("unsafe.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        for name in &names {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"released payload").unwrap();
        }
        writer.finish().unwrap();
        let source_before = std::fs::read(&file).unwrap();
        let entries = crate::zip_loader::enumerate_image_entries(&file).unwrap();
        // The product enumerator defines released admission. The projection
        // must preserve those rows, rather than manufacture archive admission.
        assert!(!entries.is_empty(), "expected released rows for {names:?}");
        let tree = ZipTree::build(file.clone(), entries);
        let prefix = tree.collapse_redundant(&[]);
        let domain = PageDomain::ArchiveLevel {
            prefix: prefix.join("/"),
        };
        // A valid stored row is retained even if the current source cannot be composed.
        let safe = if prefix.is_empty() {
            "c.jpg".to_owned()
        } else {
            format!("{}/c.jpg", prefix.join("/"))
        };
        let mut stored = BookComposition::new(
            ReadingDirection::Ltr,
            vec![CompositionPageRef::ArchiveEntry { entry_name: safe }],
        );
        let database = dir.path().join("book_composition.db");
        store::initialize_database_at(&database).unwrap();
        let mut connection = store::open_existing(&database).unwrap().unwrap();
        if let Ok(scope) = BookScopeKey::archive(&file, &prefix)
            && stored.validate(&domain).is_ok()
        {
            let tx = connection.transaction().unwrap();
            store::replace_row_in(
                &tx,
                "book_compositions",
                &scope.storage_key(),
                &domain,
                &stored,
            )
            .unwrap();
            tx.commit().unwrap();
            stored = store::load_rows(&connection).unwrap()[&scope.storage_key()]
                .composition
                .clone();
        }
        let rows_before = store::load_rows(&connection).unwrap();
        let database_before = std::fs::read(&database).unwrap();
        let record_before = serde_json::to_vec(&stored).unwrap();
        let rows = materialize_archive_level(
            &tree,
            &prefix,
            SortOrder::FileName,
            Some(&GridDisplayOrder::default()),
        );
        let legacy = rows.clone();
        let output = project_archive_composition(
            &tree,
            &file,
            &prefix,
            rows,
            SortOrder::FileName,
            Some(&GridDisplayOrder::default()),
            FileTypeVisibility::default(),
            token(),
            Some(&stored),
        )
        .unwrap();
        assert!(
            matches!(
                output.composition,
                CompositionProjectionOutcome::Unsupported { .. }
            ),
            "{names:?}, {domain:?}"
        );
        assert_eq!(output.items, legacy.0);
        assert_eq!(output.metas, legacy.1);
        assert_eq!(serde_json::to_vec(&stored).unwrap(), record_before);
        assert_eq!(store::load_rows(&connection).unwrap(), rows_before);
        assert_eq!(std::fs::read(&database).unwrap(), database_before);
        assert_eq!(std::fs::read(&file).unwrap(), source_before);
    }
}

#[test]
fn g3_actual_traversal_archive_keeps_existing_enumerator_rejection_and_source_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("traversal.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
    for name in ["../b.jpg", "../c.jpg"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"released payload").unwrap();
    }
    writer.finish().unwrap();
    let before = std::fs::read(&file).unwrap();
    assert!(
        crate::zip_loader::enumerate_image_entries(&file)
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read(&file).unwrap(), before);
    // A producer that already has traversal metadata is still refused by the
    // pure preparation boundary, without changing that producer's legacy rows.
    let raw = [CanonicalRawPage {
        page: CompositionPageRef::ArchiveEntry {
            entry_name: "../b.jpg".into(),
        },
        raw_index: 0,
    }];
    let manifest = [ManifestPage {
        page: raw[0].page.clone(),
        availability: ManifestAvailability::Present { item_index: 0 },
    }];
    let projected = project_composition(
        BookScopeKey::archive(&file, &[]).unwrap(),
        &PageDomain::ArchiveLevel {
            prefix: String::new(),
        },
        token(),
        None,
        &raw,
        &manifest,
        1,
    )
    .unwrap();
    assert!(matches!(
        projected.outcome,
        CompositionProjectionOutcome::Unsupported { .. }
    ));
    assert_eq!(projected.output_to_input, [0]);
}

#[test]
fn g3_case_distinct_raw_levels_are_legacy_children_and_not_one_composition_scope() {
    let tree = ZipTree::build(
        "C:/book.zip".into(),
        ["A/b.jpg", "a/c.jpg"]
            .into_iter()
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                mtime: 1,
                uncompressed_size: 1,
                container_ancestry: Vec::new(),
            })
            .collect(),
    );
    for prefix in [vec!["A".to_owned()], vec!["a".to_owned()]] {
        let rows = materialize_archive_level(&tree, &prefix, SortOrder::FileName, None);
        let before = rows.clone();
        let result = project_archive_composition(
            &tree,
            &tree.zip_path,
            &prefix,
            rows,
            SortOrder::FileName,
            None,
            FileTypeVisibility::default(),
            token(),
            None,
        )
        .unwrap();
        assert!(matches!(
            result.composition,
            CompositionProjectionOutcome::Unsupported { .. }
        ));
        assert_eq!((result.items, result.metas), before);
    }
}

#[test]
fn g3_uncomposed_preserves_exact_order_even_when_record_order_would_differ() {
    let manifest = fixtures::document_manifest(4);
    let automatic = project_composition(
        BookScopeKey::document("C:/book.pdf").unwrap(),
        &PageDomain::Document,
        token(),
        None,
        &raw_document(4, 0),
        &manifest,
        4,
    )
    .unwrap();
    assert_eq!(automatic.output_to_input, [0, 1, 2, 3]);
    assert_eq!(automatic.outcome, CompositionProjectionOutcome::Automatic);
    let mut invalid = fixtures::four_page_book(ReadingDirection::Ltr);
    invalid.schema_version += 1;
    let blocked = project_composition(
        BookScopeKey::document("C:/book.pdf").unwrap(),
        &PageDomain::Document,
        token(),
        Some(&invalid),
        &raw_document(4, 0),
        &manifest,
        4,
    )
    .unwrap();
    assert_eq!(blocked.output_to_input, [0, 1, 2, 3]);
    assert!(matches!(
        blocked.outcome,
        CompositionProjectionOutcome::Unreadable { .. }
    ));
}

#[test]
fn b_cover_flag_survives_hidden_missing_and_reindex() {
    let scope = BookScopeKey::archive("C:/cover.zip", &[]).unwrap();
    let page = CompositionPageRef::ArchiveEntry {
        entry_name: "FrontCover.JPG".into(),
    };
    let mut record = BookComposition::new(ReadingDirection::Rtl, vec![page.clone()]);
    record.cover_enabled = true;
    let raw = [CanonicalRawPage {
        page: page.clone(),
        raw_index: 0,
    }];
    let manifest = [ManifestPage {
        page: page.clone(),
        availability: ManifestAvailability::Present { item_index: 0 },
    }];
    let prepared = project_composition(
        scope.clone(),
        &PageDomain::ArchiveLevel {
            prefix: String::new(),
        },
        token(),
        Some(&record),
        &raw,
        &manifest,
        1,
    )
    .unwrap();
    let projection = composed(&prepared.outcome);
    assert_eq!(projection.cover_enabled, record.cover_enabled);
    let deleted = projection
        .remap_removed_items(&[None], &[page.clone()])
        .unwrap();
    assert!(deleted.cover_enabled);
    assert_eq!(deleted.resolved.page_status(&page), PageStatus::Missing);
    assert!(deleted.resolved.visible_pages.is_empty());
    record.hidden_pages.push(page.clone());
    let hidden = project_composition(
        scope,
        &PageDomain::ArchiveLevel {
            prefix: String::new(),
        },
        token(),
        Some(&record),
        &raw,
        &manifest,
        1,
    )
    .unwrap();
    let hidden = composed(&hidden.outcome);
    assert!(hidden.cover_enabled);
    assert_eq!(
        hidden.resolved.page_status(&page),
        PageStatus::HiddenByComposition
    );
    assert!(hidden.resolved.visible_pages.is_empty());
    assert_eq!(hidden.clone().cover_enabled, record.cover_enabled);
    let reindexed = hidden.remap_removed_items(&[], &[]).unwrap();
    assert!(reindexed.cover_enabled);
    assert_eq!(record.page_order, vec![page]);
}

#[test]
fn g3_raw_structure_keeps_policy_excluded_parent_and_original_effective_scope() {
    let tree = ZipTree::build(
        "C:/cache/book.zip".into(),
        ["root.jpg", "chapter/child.jpg"]
            .into_iter()
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                mtime: 1,
                uncompressed_size: 10,
                container_ancestry: Vec::new(),
            })
            .collect(),
    );
    let mut policy = FileTypeVisibility::default();
    policy.excluded_extensions.insert("jpg".into());
    // Excluding all pages may prune the display hierarchy; the original is still exact.
    let projected = tree.project(&policy);
    assert_eq!(projected.raw_source().root.images.len(), 1);
    assert_eq!(
        projected.raw_source().collapse_redundant(&[]),
        Vec::<String>::new()
    );
    assert_eq!(projected.raw_source().root.dirs.len(), 1);
    let again = projected.project(&FileTypeVisibility::default());
    assert_eq!(again.root.images.len(), 1);
    assert_eq!(again.root.dirs["chapter"].images.len(), 1);
    assert_eq!(again.raw_source().root.images[0].entry_name, "root.jpg");
    assert!(std::ptr::eq(projected.raw_source(), again.raw_source()));
    let source = Path::new("D:/source/book.rar");
    let prefix = vec!["chapter".into()];
    let semantic = crate::app::book_materialization::BookScope::Archive {
        logical_root: source.to_owned(),
        prefix: prefix.clone(),
    };
    let (scope, _) = crate::app::book_materialization::composition_scope_domain(&semantic)
        .unwrap()
        .unwrap();
    assert_eq!(scope, BookScopeKey::archive(source, &prefix).unwrap());
    assert_ne!(
        scope,
        BookScopeKey::archive(&tree.zip_path, &prefix).unwrap()
    );
    assert!(
        crate::app::book_materialization::composition_scope_domain(
            &crate::app::book_materialization::BookScope::ImageFolder(source.to_owned())
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn g3_navigation_retains_raw_level_when_policy_collapse_changes_display_prefix() {
    let tree = ZipTree::build(
        "C:/book.zip".into(),
        ["root.jpg", "chapter/a.png", "chapter/branch/b.png"]
            .into_iter()
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                mtime: 1,
                uncompressed_size: 1,
                container_ancestry: Vec::new(),
            })
            .collect(),
    );
    let mut policy = FileTypeVisibility::default();
    policy.excluded_extensions.insert("jpg".into());
    let display_tree = std::sync::Arc::new(tree.project(&policy));
    let mut nav = crate::zip_tree::ZipNavState::new(display_tree.clone());
    assert_eq!(nav.current(), ["chapter"]);
    assert!(nav.current_raw().is_empty());
    let restored = crate::zip_tree::ZipNavState::restore_prefix(display_tree, "chapter/").unwrap();
    assert_eq!(restored.current(), nav.current());
    assert_eq!(restored.current_raw(), nav.current_raw());
    nav.enter("chapter/branch/");
    assert_eq!(nav.current_raw(), ["chapter", "branch"]);
    assert!(nav.back());
    assert!(nav.current_raw().is_empty());
    assert!(nav.dfs_step(true, SortOrder::FileName));
    assert_eq!(nav.current_raw(), ["chapter", "branch"]);
    assert!(nav.dfs_step(false, SortOrder::FileName));
    assert!(nav.current_raw().is_empty());
}

#[test]
fn b_initial_capture_binding_and_raw_order_without_frozen_units() {
    let pages = (0..4).map(fixtures::document_page).collect::<Vec<_>>();
    let initial =
        capture_initial_composition(&PageDomain::Document, ReadingDirection::Rtl, true, &pages)
            .unwrap();
    assert_eq!(initial.page_order, pages);
    assert!(initial.cover_enabled);
    assert!(initial.page_overrides.is_empty());
    let mut duplicate = pages;
    duplicate.push(fixtures::document_page(0));
    assert!(
        capture_initial_composition(
            &PageDomain::Document,
            ReadingDirection::Rtl,
            true,
            &duplicate
        )
        .is_err()
    );
}

#[test]
fn g3_e_joint_scope_identity_and_opaque_composed_do_not_consume_dormant_shift() {
    use crate::spread_shift::{
        self, BookLayoutSource, PreparedLayoutRecipe, SpreadShiftPageRef, SpreadShiftState,
        contract_fixtures,
    };
    let e = contract_fixtures::four_page_book("C:/joint");
    let pages = e
        .pages
        .iter()
        .map(|page| match page {
            SpreadShiftPageRef::RelativePath(path) => {
                CompositionPageRef::RelativePath { path: path.clone() }
            }
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let record = capture_initial_composition(
        &PageDomain::ImageFolder,
        ReadingDirection::Ltr,
        false,
        &pages,
    )
    .unwrap();
    let raw = pages
        .iter()
        .enumerate()
        .map(|(raw_index, page)| CanonicalRawPage {
            raw_index,
            page: page.clone(),
        })
        .collect::<Vec<_>>();
    let manifest = pages
        .iter()
        .enumerate()
        .map(|(item_index, page)| ManifestPage {
            page: page.clone(),
            availability: ManifestAvailability::Present { item_index },
        })
        .collect::<Vec<_>>();
    let result = project_composition(
        e.scope.clone(),
        &PageDomain::ImageFolder,
        token(),
        Some(&record),
        &raw,
        &manifest,
        4,
    )
    .unwrap();
    let projection = composed(&result.outcome);
    assert_eq!(projection.scope, e.scope);
    for (index, page) in pages.iter().enumerate() {
        let identity = page.resume_identity(&e.scope.logical_root);
        assert_eq!(
            SpreadShiftPageRef::from_identity(&e.scope, &identity).unwrap(),
            e.pages[index]
        );
        assert_eq!(
            projection.resolve_position(&ReadingPosition {
                page: 99,
                identity: Some(identity)
            }),
            PageStatus::Visible { item_index: index }
        );
    }
    let receipt = e.receipt(81);
    let dormant = contract_fixtures::completed_read(&receipt, Some(e.pages[1].clone()));
    let before = dormant.row.clone();
    let automatic = contract_fixtures::resolve_automatic(
        e.request(receipt.clone()),
        Some(dormant.clone()),
        SpreadShiftState::None,
    );
    assert_eq!(automatic.optional_shift.unwrap().anchor_idx, 1);
    // E returns G's immutable value before validating/resolving any automatic request.
    let mut invalid_request = e.request(receipt);
    invalid_request.manifest.generation += 1;
    let BookLayoutSource::Composed(returned) = spread_shift::resolve_book_layout_source(
        PreparedLayoutRecipe::Composed(projection.clone()),
        invalid_request,
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(returned, *projection);
    assert_eq!(dormant.row, before);
    assert_eq!(returned.token, token());
}
