//! Real composition permission owners carry only their own arrival request.
use super::*;
use crate::ui_fullscreen::{FsInitialPlacementKind, FsPageLanding};

fn setup(
    ctx: &egui::Context,
) -> (
    crate::app::AppTestEnvForTest,
    BookCompositionService,
    BookCompositionProcessOwner,
    crate::book_scope::BookScopeKey,
) {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    let root = app.tmp.path().join("landing-permission.pdf");
    app.items = (0..3)
        .map(|page_num| crate::grid_item::GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    app.current_folder = None;
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::OrdinaryList;
    app.visible_indices = vec![0, 1, 2];
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 3];
    app.image_metas = vec![None; 3];
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.page_turn_animation_enabled = false;
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.native_video_in_window_active = true;
    app.settings.video_in_window_mode = true;
    for idx in 0..3 {
        let pixels = std::sync::Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("permission-page-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            crate::fs_animation::FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
    }
    app.settings.fullscreen_page_start_position =
        crate::settings::FullscreenPageStartPosition::Center;
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(app.tmp.path().join("landing-permission-data"), None)
            .unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            BookComposition::new(
                ReadingDirection::Ltr,
                (0..3)
                    .map(|page_num| CompositionPageRef::PdfPage { page_num })
                    .collect(),
            ),
        )
        .recv()
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service.clone());
    (app, service, owner, scope)
}

fn pending(app: &App) -> &OpenIntent {
    match &app.book_composition_permission.open {
        OpenPermission::Checking { intent, .. }
        | OpenPermission::Ready { intent, .. }
        | OpenPermission::Resuming { intent, .. } => intent,
        _ => panic!("a qualified request must own the pending arrival"),
    }
}

fn complete(
    app: &mut App,
    ctx: &egui::Context,
    service: &BookCompositionService,
    scope: &crate::book_scope::BookScopeKey,
    idx: usize,
) {
    // FIFO preparation is a worker barrier, not a timed wait or sleep. Internal
    // requalification can create another episode, so drive each queued stage.
    for _ in 0..8 {
        service
            .prepare(scope.clone(), PageDomain::Document)
            .recv()
            .unwrap()
            .status
            .unwrap();
        app.poll_book_presentation_permission(ctx);
        if app.fullscreen_idx == Some(idx) && app.fs_initial_placement_request.is_some() {
            return;
        }
    }
    panic!("the qualified open did not complete its finite permission stages");
}

#[test]
fn permission_navigation_explicit_and_inner_open_resume_with_the_exact_landing() {
    for route in 0..3 {
        for landing in [FsPageLanding::Start, FsPageLanding::End] {
            let ctx = egui::Context::default();
            let (mut app, service, owner, scope) = setup(&ctx);
            if route == 2 {
                app.open_fullscreen_with_metadata_boundary_and_adoption_and_landing(
                    1,
                    HistoryTrigger::UserChosen,
                    FsOpenMaterialization::NavigationTargetMaterializationOnly,
                    FsPageLoadContract::Sequential,
                    None,
                    FsMetadataBoundary::Manual,
                    None,
                    crate::app::ViewerOpenAdoption::ContextContinuation,
                    Some(landing),
                );
            } else {
                app.complete_book_navigation_target_with_landing(
                    &ctx,
                    1,
                    HistoryTrigger::UserChosen,
                    FsPageLoadContract::Sequential,
                    if route == 0 {
                        BookTargetPurpose::Next
                    } else {
                        BookTargetPurpose::Explicit
                    },
                    Some(landing),
                );
            }
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(pending(&app).landing, Some(landing));
            complete(&mut app, &ctx, &service, &scope, 1);
            assert_eq!(
                app.fs_initial_placement_request.unwrap().kind,
                landing.initial_placement_kind()
            );
            assert_eq!(
                app.settings.fullscreen_page_start_position,
                crate::settings::FullscreenPageStartPosition::Center
            );
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn permission_replacement_and_generation_retirement_discard_the_old_landing() {
    let ctx = egui::Context::default();
    let (mut app, service, owner, scope) = setup(&ctx);
    assert!(app.defer_book_composition_navigation_open_with_landing(
        1,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        BookTargetPurpose::Next,
        Some(FsPageLanding::End),
    ));
    assert_eq!(pending(&app).landing, Some(FsPageLanding::End));
    assert!(app.defer_book_composition_navigation_open(
        1,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        BookTargetPurpose::Next,
    ));
    assert_eq!(pending(&app).landing, None);
    assert!(app.defer_book_composition_navigation_open_with_landing(
        2,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        BookTargetPurpose::Next,
        Some(FsPageLanding::Start),
    ));
    app.items_generation += 1;
    app.bind_book_presentation_demand();
    assert!(matches!(
        app.book_composition_permission.open,
        OpenPermission::Idle
    ));
    service
        .prepare(scope, PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    app.poll_book_presentation_permission(&ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert!(
        app.fs_initial_placement_request
            .is_none_or(|request| request.kind == FsInitialPlacementKind::Setting)
    );
    owner.shutdown_and_join();
}
