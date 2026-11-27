use super::*;
use crate::app::book_materialization::{BookScope, ProjectionRecipe};
use crate::app::page_edit_snapshot::PageEditAvailability;
use crate::book_composition::{BookComposition, ReadingDirection, fixtures};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::page_edit_write_epoch::PAGE_EDIT_WRITES;
use std::path::Path;
use std::time::Duration;

fn seed_document(app: &mut App, root: &Path) -> (BookScopeKey, Arc<AcceptedCompositionProjection>) {
    app.current_folder = Some(root.to_path_buf());
    app.items = (0..4)
        .map(|page_num| crate::app::book_materialization::document_page_item(root, page_num))
        .collect();
    app.image_metas = vec![None; 4];
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.visible_indices = (0..4).collect();
    app.details_order = app.visible_indices.clone();
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(root.to_path_buf()));
    app.pdf_placeholder_count = Some(4);
    let token = CompositionToken {
        service_epoch: 1,
        publication_sequence: 1,
    };
    let prepared = crate::app::book_materialization::project_document_composition(
        root,
        4,
        (app.items.clone(), app.image_metas.clone()),
        app.settings.file_type_visibility.clone(),
        token,
        Some(&fixtures::four_page_book(ReadingDirection::Ltr)),
    )
    .unwrap();
    let CompositionProjectionOutcome::Composed(projection) = prepared.composition else {
        panic!("expected document composition");
    };
    app.page_identity_index = prepared.page_identities;
    app.install_composition_for_test(projection.clone());
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.fullscreen_idx = Some(1);
    app.selected = Some(1);
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    app.viewer_navigation_caches.invalidate();
    (BookScopeKey::document(root).unwrap(), projection)
}

fn seed_conceal_draft(app: &mut App) -> String {
    let key = app.page_path_key(1).unwrap();
    assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
    app.conceal_mode = true;
    app.conceal_mask_size = [2, 2];
    app.conceal_mask = Some(vec![true, false, true, false]);
    app.conceal_shapes.clear();
    key
}

fn assert_saved_conceal(app: &App, key: &str) {
    let (mask, shapes, dimensions) = app
        .conceal_db
        .as_ref()
        .unwrap()
        .get_full_checked(key)
        .unwrap()
        .unwrap();
    assert_eq!(dimensions, [2, 2]);
    assert_eq!(mask, vec![true, false, true, false]);
    assert!(shapes.is_empty());
}

fn refresh_origin(
    app: &App,
    previous: Arc<AcceptedCompositionProjection>,
) -> (BookCompositionSource, ProjectionOrigin) {
    let source = app.composition_source();
    let scope = &previous.scope;
    let previous_order = app
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                index,
                crate::book_resume_db::PageIdentity::for_item(item, Some(&scope.logical_root))
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let anchor = Some(previous_order[1].1.clone());
    let origin = ProjectionOrigin::Refresh {
        source: source.clone(),
        previous: Some(previous),
        anchor,
        previous_order: previous_order.into(),
    };
    (source, origin)
}

fn replacement_record(hidden: &[u32]) -> Arc<StoredComposition> {
    let mut composition = BookComposition::new(
        ReadingDirection::Ltr,
        [2, 0, 1, 3]
            .into_iter()
            .map(fixtures::document_page)
            .collect(),
    );
    composition.hidden_pages = hidden
        .iter()
        .copied()
        .map(fixtures::document_page)
        .collect();
    Arc::new(StoredComposition {
        domain: PageDomain::Document,
        composition,
    })
}

fn prepare_reply(
    app: &App,
    scope: &BookScopeKey,
    record: &StoredComposition,
    source: &BookCompositionSource,
    origin: &ProjectionOrigin,
) -> BookProjectionReply {
    // Use the production row/edit producer, including its stable write stamp.
    prepare_book_projection(
        scope,
        CompositionToken {
            service_epoch: 1,
            publication_sequence: 2,
        },
        Some(record),
        source,
        origin,
        &app.settings.grid_display_order,
        app.settings.file_type_visibility.clone(),
        PageEditAvailability::for_app(app),
        &crate::data_dir::get(),
        None,
        app.rotation_cache.invalidation_revision(),
        None,
        None,
        false,
        None,
    )
    .unwrap()
}

fn finish_reprepared_projection(app: &mut App, ctx: &egui::Context) {
    let ViewerBookComposition::Projecting {
        scope,
        token,
        record,
        source,
        generation,
        origin,
        receiver,
    } = std::mem::take(&mut app.book_composition)
    else {
        panic!("the edit save must reject the old snapshot and start the existing worker");
    };
    assert_eq!(generation, app.items_generation);
    let reply = receiver
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert!(PAGE_EDIT_WRITES.accepts(reply.edits.as_ref().unwrap().stamp));
    app.adopt_book_projection(scope, token, record, source, *origin, reply, ctx);
}

fn page_numbers(app: &App) -> Vec<u32> {
    app.items
        .iter()
        .map(|item| match item {
            GridItem::PdfPage { page_num, .. } => *page_num,
            _ => panic!("unexpected non-document row"),
        })
        .collect()
}

#[test]
fn page_edit_920_composition_save_precedes_epoch_check_and_reprepare_for_all_memberships() {
    for (hidden, expected_pages, expected_landing, expected_badge) in [
        (vec![], vec![2, 0, 1, 3], Some(2), Some(2)),
        (vec![1], vec![2, 0, 3], Some(0), None),
        (vec![0, 1, 2, 3], vec![], None, None),
    ] {
        let mut app = crate::app::setup_app_for_test();
        let root = app.tmp.path().join("page-edit-920.pdf");
        let (scope, previous) = seed_document(&mut app, &root);
        let (source, origin) = refresh_origin(&app, previous);
        let record = replacement_record(&hidden);
        let old_key = app.page_path_key(1).unwrap();
        let reply = prepare_reply(&app, &scope, &record, &source, &origin);
        let prepared = reply.edits.as_ref().unwrap();
        let before_save = prepared.stamp;
        assert!(PAGE_EDIT_WRITES.accepts(before_save));
        assert!(!prepared.snapshot.conceal.contains(&old_key));
        assert!(prepared.projection.conceal.is_empty());
        assert_eq!(seed_conceal_draft(&mut app), old_key);
        let generation = app.items_generation;
        let ctx = egui::Context::default();

        app.adopt_book_projection(
            scope,
            CompositionToken {
                service_epoch: 1,
                publication_sequence: 2,
            },
            Some(record),
            source,
            origin,
            reply,
            &ctx,
        );

        assert!(!app.conceal_mode);
        assert!(app.conceal_mask.is_none());
        assert_saved_conceal(&app, &old_key);
        assert!(
            !PAGE_EDIT_WRITES.accepts(before_save),
            "saving the old page advances the epoch"
        );
        assert_eq!(
            app.items_generation, generation,
            "stale edits must not replace the old rows"
        );
        assert_eq!(page_numbers(&app), vec![0, 1, 2, 3]);
        assert_eq!(app.fullscreen_idx, Some(1));

        finish_reprepared_projection(&mut app, &ctx);
        assert_eq!(page_numbers(&app), expected_pages);
        assert_eq!(app.fullscreen_idx, expected_landing);
        assert_eq!(
            app.conceal_pages.iter().copied().collect::<Vec<_>>(),
            expected_badge.into_iter().collect::<Vec<_>>()
        );
        assert!(!app.conceal_mode);
        assert!(app.conceal_spread_ctx.is_none());
        assert_saved_conceal(&app, &old_key);
        for (index, item) in app.items.iter().enumerate() {
            if !matches!(item, GridItem::PdfPage { page_num: 1, .. }) {
                let key = app.page_path_key(index).unwrap();
                assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
                assert!(!app.conceal_pages.contains(&index));
            }
        }
        app.reconcile_page_edit_spread_pivots();
        assert_eq!(app.fullscreen_idx, expected_landing);
    }
}

#[test]
fn page_edit_920_composition_captures_landing_after_consuming_the_old_spread_pivot() {
    let mut app = crate::app::setup_app_for_test();
    let root = app.tmp.path().join("page-edit-920-spread.pdf");
    let (scope, previous) = seed_document(&mut app, &root);
    let (source, origin) = refresh_origin(&app, previous);
    let record = replacement_record(&[]);
    let reply = prepare_reply(&app, &scope, &record, &source, &origin);
    let old_key = seed_conceal_draft(&mut app);
    app.conceal_spread_ctx = Some(crate::app::PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    });
    let ctx = egui::Context::default();
    app.adopt_book_projection(
        scope,
        CompositionToken {
            service_epoch: 1,
            publication_sequence: 2,
        },
        Some(record),
        source,
        origin,
        reply,
        &ctx,
    );
    assert_saved_conceal(&app, &old_key);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
    assert!(app.conceal_spread_ctx.is_none());

    finish_reprepared_projection(&mut app, &ctx);
    assert_eq!(page_numbers(&app), vec![2, 0, 1, 3]);
    assert_eq!(
        app.fullscreen_idx,
        Some(1),
        "the restored anchor is remapped to its new row"
    );
    assert!(
        app.conceal_pages.contains(&2),
        "the edited page remains distinct from the reading anchor"
    );
    app.reconcile_page_edit_spread_pivots();
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
}

#[cfg(windows)]
#[test]
fn page_edit_920_sibling_composition_adoption_preserves_foreground_draft_and_save_target() {
    let mut app = crate::app::setup_app_for_test();
    let root = app.tmp.path().join("page-edit-920-active.pdf");
    let sibling_root = app.tmp.path().join("page-edit-920-sibling.pdf");
    seed_document(&mut app, &root);
    let foreground = app.viewer_context_main();
    let mut sibling_setup = None;
    let sibling = app.build_window_context_for_test(19207, |owner| {
        sibling_setup = Some(seed_document(owner, &sibling_root));
    });
    let (scope, previous) = sibling_setup.unwrap();
    let old_key = seed_conceal_draft(&mut app);
    let generation = app.items_generation;
    let stamp = PAGE_EDIT_WRITES.sample();
    let ctx = egui::Context::default();
    let mut sibling_keys = Vec::new();

    app.with_viewer_context_state(sibling, |owner| {
        assert_eq!(owner.foreground_viewer_context_id(), Some(foreground));
        assert!(!owner.page_edit_tools_owned_by_projected_context());
        let (source, origin) = refresh_origin(owner, previous);
        let record = replacement_record(&[]);
        let reply = prepare_reply(owner, &scope, &record, &source, &origin);
        let generation = owner.items_generation;
        owner.adopt_book_projection(
            scope,
            CompositionToken {
                service_epoch: 1,
                publication_sequence: 2,
            },
            Some(record),
            source,
            origin,
            reply,
            &ctx,
        );
        assert_ne!(
            owner.items_generation, generation,
            "the sibling replacement was adopted"
        );
        assert_eq!(page_numbers(owner), vec![2, 0, 1, 3]);
        assert_eq!(owner.fullscreen_idx, Some(2));
        assert!(owner.conceal_mode);
        assert_eq!(
            owner.conceal_mask.as_deref(),
            Some([true, false, true, false].as_slice())
        );
        assert_eq!(owner.conceal_mask_size, [2, 2]);
        assert!(owner.conceal_pages.is_empty());
        sibling_keys = (0..owner.items.len())
            .map(|index| owner.page_path_key(index).unwrap())
            .collect();
        for key in &sibling_keys {
            assert_eq!(owner.conceal_db.as_ref().unwrap().dimensions(key), None);
        }
    })
    .unwrap();

    assert_eq!(app.items_generation, generation);
    assert_eq!(page_numbers(&app), vec![0, 1, 2, 3]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.conceal_mode);
    assert_eq!(
        app.conceal_mask.as_deref(),
        Some([true, false, true, false].as_slice())
    );
    assert_eq!(
        PAGE_EDIT_WRITES.sample(),
        stamp,
        "sibling adoption must not save the foreground draft"
    );
    assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&old_key), None);

    app.reset_conceal_mode();
    assert_saved_conceal(&app, &old_key);
    for key in sibling_keys {
        assert_eq!(app.conceal_db.as_ref().unwrap().dimensions(&key), None);
    }
}
