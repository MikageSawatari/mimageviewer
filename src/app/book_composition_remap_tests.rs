use super::*;
use crate::app::{book_materialization, tests::phase_c_support::setup_app, *};
use crate::book_composition::ReadingDirection;
use crate::book_composition::*;
use crate::book_resume_db::{PageIdentity, ReadingPosition};

fn install(app: &mut App, root: &std::path::Path, book: &BookComposition) {
    let token = CompositionToken {
        service_epoch: 8,
        publication_sequence: 3,
    };
    let rows = (
        (0..4)
            .map(|page| book_materialization::document_page_item(root, page))
            .collect(),
        vec![None; 4],
    );
    let projected = book_materialization::project_document_composition(
        root,
        4,
        rows,
        app.settings.file_type_visibility.clone(),
        token,
        Some(book),
    )
    .unwrap();
    app.current_folder = Some(root.to_path_buf());
    app.zip_nav = None;
    app.projection_recipe = book_materialization::ProjectionRecipe::Book(
        book_materialization::BookScope::Document(root.to_path_buf()),
    );
    app.items = projected.items;
    app.image_metas = projected.metas;
    app.page_identity_index = projected.page_identities;
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.details_order = app.visible_indices.clone();
    app.viewer_navigation_caches.invalidate();
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.book_composition = ViewerBookComposition::Accepted {
        scope: crate::book_scope::BookScopeKey::document(root).unwrap(),
        token,
        record: Some(Arc::new(StoredComposition {
            domain: PageDomain::Document,
            composition: book.clone(),
        })),
        layout: projected.composition,
        source: crate::app::book_composition_runtime::BookCompositionSource::Document {
            count: 4,
            metadata: None,
            physical_source: root.to_path_buf(),
            read_target: None,
        },
        generation: app.items_generation,
        cause: crate::app::book_composition_runtime::ProjectionAdoptionCause::Normal,
    };
    app.spread_shift.availability = crate::spread_shift::PairingEditAvailability::Composed;
}

fn accepted(
    app: &App,
) -> (
    &Arc<AcceptedCompositionProjection>,
    &Arc<StoredComposition>,
    u64,
) {
    let ViewerBookComposition::Accepted {
        layout: CompositionProjectionOutcome::Composed(projection),
        record: Some(record),
        generation,
        ..
    } = &app.book_composition
    else {
        panic!("expected composed book");
    };
    (projection, record, *generation)
}

#[test]
fn batch_removal_reindexes_topology_targets_resume_and_export_before_reload() {
    let mut env = setup_app();
    let root = env.tmp.path().join("deletion.pdf");
    install(
        &mut env,
        &root,
        &fixtures::four_page_book(ReadingDirection::Ltr),
    );
    let saved = Arc::clone(accepted(&env).1);
    let token = accepted(&env).0.token;
    let original_generation = env.items_generation;
    env.fullscreen_idx = Some(2);
    env.selected = Some(2);
    let key = env.page_path_key(2).unwrap();
    let nav = env.get_nav_indices();
    assert_eq!(
        env.spread_display_unit_pages_for_test(&nav),
        vec![vec![0, 1], vec![2, 3]]
    );
    env.remove_items_batch(&[1]);
    let (projection, record, generation) = accepted(&env);
    assert!(Arc::ptr_eq(record, &saved));
    assert_eq!(
        record.composition,
        fixtures::four_page_book(ReadingDirection::Ltr)
    );
    assert_eq!(projection.token, token);
    assert_eq!(generation, original_generation + 1);
    assert_eq!(generation, env.items_generation);
    assert_eq!(
        projection
            .resolved
            .units
            .iter()
            .map(|unit| unit
                .navigation_pages()
                .map(|page| page.item_index)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        vec![vec![0], vec![1, 2]]
    );
    assert_eq!(
        projection.resolved.page_status(&fixtures::document_page(1)),
        PageStatus::Missing
    );
    assert_eq!(projection.canonical_raw_index_for_projected(1), Some(1));
    assert_eq!(projection.reading_ordinal(1), Some(2));
    let position = projection.position_for_projected(1).unwrap();
    assert_eq!(position.page, 1);
    assert_eq!(
        position.identity,
        PageIdentity::for_item(&env.items[1], Some(&root))
    );
    assert_eq!(
        projection.resolve_position(&position),
        PageStatus::Visible { item_index: 1 }
    );
    assert_eq!(
        projection.resolve_position(&ReadingPosition {
            page: 2,
            identity: None
        }),
        PageStatus::Visible { item_index: 2 }
    );
    assert_eq!(
        env.page_identity_index.resolve(&position),
        file_type_resume::PositionResolution::Restored(1)
    );
    assert_eq!(env.fullscreen_idx, Some(1));
    assert_eq!(env.selected, Some(1));
    let (targets, skipped) = env.grid_batch_export_targets();
    assert_eq!(skipped, 0);
    assert_eq!(
        targets.into_iter().collect::<Vec<_>>(),
        vec![crate::ui_dialogs::export_batch::ExportBatchTarget::Item {
            idx: 1,
            page_key: key
        }]
    );
    let nav = env.get_nav_indices();
    assert_eq!(
        env.spread_display_unit_pages_for_test(&nav),
        vec![vec![0], vec![1, 2]]
    );
    assert_eq!(
        env.spread_display_composition_for_anchor(1)
            .navigation_pages(),
        vec![1, 2]
    );
}

#[test]
fn removal_preserves_hidden_reference_and_empty_projection_without_repairing_record() {
    let mut env = setup_app();
    let root = env.tmp.path().join("hidden.pdf");
    let book = fixtures::hidden_blank_deletion_book(ReadingDirection::Ltr);
    install(&mut env, &root, &book);
    let saved = Arc::clone(accepted(&env).1);
    assert_eq!(env.items.len(), 3);
    env.remove_items_batch(&[2, 1, 0]);
    let (projection, record, generation) = accepted(&env);
    assert!(Arc::ptr_eq(record, &saved));
    assert_eq!(record.composition, book);
    assert_eq!(generation, env.items_generation);
    assert!(projection.resolved.units.is_empty());
    assert!(projection.resolved.visible_pages.is_empty());
    assert_eq!(
        projection.resolved.page_status(&fixtures::document_page(1)),
        PageStatus::HiddenByComposition
    );
    for page in [0, 2, 3] {
        assert_eq!(
            projection
                .resolved
                .page_status(&fixtures::document_page(page)),
            PageStatus::Missing
        );
    }
    assert_eq!(projection.canonical_raw_index_for_projected(0), None);
    assert!(env.get_nav_indices().is_empty());
}

#[test]
fn removal_adoption_invalidates_permission_proof_and_same_slot_cache() {
    let mut env = setup_app();
    let root = env.tmp.path().join("proof.pdf");
    install(
        &mut env,
        &root,
        &fixtures::four_page_book(ReadingDirection::Ltr),
    );
    let (service, process) =
        BookCompositionService::spawn_at(env.tmp.path().join("proof-authority"), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            fixtures::four_page_book(ReadingDirection::Ltr),
        )
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    env.book_composition_service = Some(service.clone());
    let occurrence = PresentationOccurrence {
        item_index: 0,
        navigation_anchor: 0,
        role: PresentationRole::Navigation,
        slot: PresentationSlot::Center,
        slice: PresentationSlice::Full,
    };
    env.retain_book_presentation_demand(&[occurrence]);
    // This later FIFO reply proves the preceding candidate permission replied;
    // one subsequent demand poll can consume its actual authority result.
    service
        .prepare(scope, PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    env.retain_book_presentation_demand(&[occurrence]);
    assert!(matches!(
        env.book_presentation_allowed(&[occurrence]),
        PresentationDecision::Allowed(_)
    ));
    let nav = env.get_nav_indices();
    assert_eq!(
        env.spread_display_unit_pages_for_test(&nav),
        vec![vec![0, 1], vec![2, 3]]
    );
    let count = crate::ui_fullscreen::spread_display_units_build_count_for_test();
    env.remove_items_batch(&[1, 0]);
    assert!(matches!(
        env.book_presentation_allowed(&[occurrence]),
        PresentationDecision::Checking
    ));
    let nav = env.get_nav_indices();
    assert_eq!(
        env.spread_display_unit_pages_for_test(&nav),
        vec![vec![0, 1]]
    );
    assert_eq!(
        crate::ui_fullscreen::spread_display_units_build_count_for_test(),
        count + 1
    );
    assert!(matches!(
        env.fs_continuous_fit_reference,
        crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy
    ));
    assert!(matches!(
        env.items[0],
        GridItem::PdfPage { page_num: 2, .. }
    ));
    process.shutdown_and_join();
}

#[test]
fn invalid_removal_preparation_leaves_owner_and_rows_unchanged() {
    let mut env = setup_app();
    let root = env.tmp.path().join("invalid.pdf");
    install(
        &mut env,
        &root,
        &fixtures::four_page_book(ReadingDirection::Ltr),
    );
    let before = env.book_composition.clone();
    let identities = Arc::clone(&env.page_identity_index);
    for indices in [&[2, 1][..], &[1, 1][..], &[4][..]] {
        assert!(env.prepare_book_composition_item_removal(indices).is_err());
    }
    assert_eq!(env.book_composition, before);
    assert!(Arc::ptr_eq(&env.page_identity_index, &identities));
    assert_eq!(env.items.len(), 4);
}

#[cfg(windows)]
#[test]
fn removal_updates_only_mounted_composed_context() {
    let mut env = setup_app();
    let root = env.tmp.path().join("mounted.pdf");
    let sibling_root = env.tmp.path().join("sibling.pdf");
    install(
        &mut env,
        &root,
        &fixtures::four_page_book(ReadingDirection::Ltr),
    );
    let sibling = env.build_window_context_for_test(139_301, |app| {
        install(
            app,
            &sibling_root,
            &fixtures::four_page_book(ReadingDirection::Ltr),
        );
    });
    let mut before = None;
    env.with_window_context_for_test(sibling, |app| {
        before = Some((
            app.book_composition.clone(),
            app.items_generation,
            Arc::clone(&app.page_identity_index),
            app.items.clone(),
        ));
    });
    env.remove_items_batch(&[1]);
    env.with_window_context_for_test(sibling, |app| {
        let (composition, generation, index, rows) = before.as_ref().unwrap();
        assert_eq!(&app.book_composition, composition);
        assert_eq!(app.items_generation, *generation);
        assert!(Arc::ptr_eq(&app.page_identity_index, index));
        assert_eq!(&app.items, rows);
        let nav = app.get_nav_indices();
        assert_eq!(
            app.spread_display_unit_pages_for_test(&nav),
            vec![vec![0, 1], vec![2, 3]]
        );
    });
    env.retire_context(sibling, "g3_item_removal_sibling_test", |_| ())
        .unwrap();
}
