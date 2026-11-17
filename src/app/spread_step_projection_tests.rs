use super::*;
use crate::app::spread_shift_owner::SpreadStep;
use crate::book_composition::{BookCompositionService, ReadingDirection, fixtures};
use crate::grid_item::ThumbnailState;
use std::{collections::HashMap, path::PathBuf, time::Duration};

fn fixture() -> (
    crate::app::AppTestEnvForTest,
    BookScopeKey,
    Arc<AcceptedCompositionProjection>,
) {
    let mut app = crate::app::setup_app_for_test();
    let root = PathBuf::from("c:/step-composed.pdf");
    app.current_folder = Some(root.clone());
    app.items = (0..4)
        .map(|page_num| crate::app::book_materialization::document_page_item(&root, page_num))
        .collect();
    app.image_metas = vec![None; 4];
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.visible_indices = (0..4).collect();
    app.details_order = app.visible_indices.clone();
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Document(root.clone()),
    );
    let book = fixtures::four_page_book(ReadingDirection::Ltr);
    let token = CompositionToken {
        service_epoch: 1,
        publication_sequence: 1,
    };
    let prepared = crate::app::book_materialization::project_document_composition(
        &root,
        4,
        (app.items.clone(), app.image_metas.clone()),
        app.settings.file_type_visibility.clone(),
        token,
        Some(&book),
    )
    .unwrap();
    let CompositionProjectionOutcome::Composed(projection) = prepared.composition else {
        panic!()
    };
    app.page_identity_index = prepared.page_identities;
    app.install_composition_for_test(projection.clone());
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.fullscreen_idx = Some(0);
    app.viewer_navigation_caches.invalidate();
    (app, BookScopeKey::document(root).unwrap(), projection)
}

fn reply(app: &App, outcome: CompositionProjectionOutcome) -> BookProjectionReply {
    BookProjectionReply {
        trace: Default::default(),
        rows: crate::app::book_materialization::MaterializedBookRows {
            items: app.items.clone(),
            metas: app.image_metas.clone(),
            composition: outcome,
            page_identities: app.page_identity_index.clone(),
        },
        landing: Some(1),
        rotations: HashMap::new(),
        rotation_invalidation_revision: app.rotation_cache.invalidation_revision(),
        edits: None,
        automatic_preferences: None,
        refresh_landings: HashMap::new(),
        saved_trim: None,
    }
}

#[test]
fn spread_step_projection_automatic_reacceptance_keeps_but_source_replacement_retires() {
    let (mut app, scope, projection) = fixture();
    let token = projection.token;
    app.book_composition = ViewerBookComposition::Automatic;
    app.spread_step = Some(SpreadStep { start_idx: 1 });
    let automatic = reply(&app, CompositionProjectionOutcome::Automatic);
    app.adopt_book_projection(
        scope.clone(),
        token,
        None,
        BookCompositionSource::Unavailable,
        ProjectionOrigin::Fresh,
        automatic,
        &egui::Context::default(),
    );
    assert!(
        app.spread_step.is_some(),
        "Automatic remaining Automatic is not a source switch"
    );
    app.install_composition_for_test(projection.clone());
    let unsupported = reply(
        &app,
        CompositionProjectionOutcome::Unsupported {
            reason: "injected accepted unsupported level".into(),
        },
    );
    let generation = app.items_generation;
    app.adopt_book_projection(
        scope,
        token,
        None,
        BookCompositionSource::Unavailable,
        ProjectionOrigin::Refresh {
            source: BookCompositionSource::Unavailable,
            previous: Some(projection.clone()),
            anchor: None,
            previous_order: Arc::from([]),
        },
        unsupported,
        &egui::Context::default(),
    );
    assert_eq!(app.items_generation, generation);
    assert!(app.spread_step.is_none());
    // Archive activation has a second accepted Unsupported publication path.
    app.install_composition_for_test(projection);
    app.spread_step = Some(SpreadStep { start_idx: 1 });
    app.accept_unsupported_archive_composition(
        std::path::Path::new("c:/step-unsupported.zip"),
        "accepted unsupported level".into(),
        "unsupported fixture",
        crate::feedback::FeedbackKind::Info,
    );
    assert_eq!(app.items_generation, generation);
    assert!(app.spread_step.is_none());
}

#[test]
fn spread_step_projection_source_change_during_real_permission_wait_cannot_revive_step() {
    let (mut app, scope, projection) = fixture();
    let data = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            fixtures::four_page_book(ReadingDirection::Ltr),
        )
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service.clone());
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(Duration::from_secs(30)).unwrap();
    let ctx = egui::Context::default();
    let mut action = crate::ui_fullscreen::FsKeyAction::default();
    app.queue_spread_step_action(0, 1, &mut action);
    app.handle_fs_navigation(
        &ctx,
        false,
        false,
        None,
        None,
        None,
        action.page_nav,
        None,
        0,
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(!app.pending_book_target_demand_for_test().is_empty());
    assert!(app.spread_step.is_some());
    let source = BookCompositionSource::Document {
        count: 4,
        metadata: None,
        physical_source: PathBuf::from("c:/step-composed.pdf"),
        read_target: None,
    };
    let automatic = reply(&app, CompositionProjectionOutcome::Automatic);
    app.adopt_book_projection(
        scope.clone(),
        projection.token,
        None,
        source.clone(),
        ProjectionOrigin::Refresh {
            source,
            previous: Some(projection),
            anchor: None,
            previous_order: Arc::from([]),
        },
        automatic,
        &ctx,
    );
    assert!(app.spread_step.is_none());
    release.send(()).unwrap();
    service
        .prepare(scope, PageDomain::Document)
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    app.poll_book_presentation_permission(&ctx);
    assert!(app.spread_step.is_none());
    owner.shutdown_and_join();
}
