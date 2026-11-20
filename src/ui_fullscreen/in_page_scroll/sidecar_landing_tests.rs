//! Both existing sidecar continuations retain the exact page-arrival request.
use super::*;
use crate::ui_fullscreen::{FsInitialPlacementKind, FsPageLanding};

#[test]
fn sidecar_live_and_index_only_resume_with_the_retained_landing() {
    for index_only in [false, true] {
        for landing in [FsPageLanding::Start, FsPageLanding::End] {
            let mut app = crate::app::tests::phase_c_support::setup_app();
            let folder = app.tmp.path().join("scroll-sidecar");
            app.current_folder = Some(folder.clone());
            app.items = vec![GridItem::Image(folder.join("page.jpg"))];
            app.visible_indices = vec![0];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.image_metas = vec![None];
            app.settings.detached_viewer_open_images_in_window = false;
            app.settings.fullscreen_page_start_position =
                crate::settings::FullscreenPageStartPosition::Center;
            app.adjustment_page_params
                .insert(0, crate::adjustment::AdjustParams::default());
            let _worker = app.activate_live_sidecar_restore_for_test(folder);
            if index_only {
                app.sidecar_restore.as_mut().unwrap().common.continuation =
                    ContinuationOwner::IndexOnly {
                        cancel: Arc::new(AtomicBool::new(false)),
                        after_prepare: IndexOnlyAfterPrepare::None,
                    };
            }
            app.open_fullscreen_with_metadata_boundary_and_adoption_and_landing(
                0,
                HistoryTrigger::UserChosen,
                FsOpenMaterialization::NavigationTargetMaterializationOnly,
                FsPageLoadContract::Sequential,
                None,
                FsMetadataBoundary::Manual,
                None,
                super::super::ViewerOpenAdoption::ContextContinuation,
                Some(landing),
            );
            assert!(app.fullscreen_idx.is_none());
            assert_eq!(
                app.sidecar_restore
                    .as_ref()
                    .unwrap()
                    .common
                    .deferred_fullscreen
                    .as_ref()
                    .unwrap()
                    .landing,
                Some(landing)
            );
            app.finish_page_identity_preparation_for_test();
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(
                app.fs_initial_placement_request.unwrap().kind,
                match landing {
                    FsPageLanding::Start => FsInitialPlacementKind::LandingStart,
                    FsPageLanding::End => FsInitialPlacementKind::LandingEnd,
                }
            );
        }
    }
}

#[test]
fn sidecar_replacing_an_open_or_its_generation_cannot_retain_a_previous_landing() {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    let folder = app.tmp.path().join("replace-scroll-sidecar");
    app.current_folder = Some(folder.clone());
    app.items = vec![
        GridItem::Image(folder.join("one.jpg")),
        GridItem::Image(folder.join("two.jpg")),
    ];
    app.visible_indices = vec![0, 1];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.image_metas = vec![None; 2];
    let _worker = app.activate_live_sidecar_restore_for_test(folder);
    assert!(
        app.defer_sidecar_restore_fullscreen_with_adoption_and_landing(
            0,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
            FsMetadataBoundary::Manual,
            None,
            super::super::ViewerOpenAdoption::ContextContinuation,
            Some(FsPageLanding::End),
        )
    );
    assert!(app.defer_sidecar_restore_fullscreen(
        1,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential
    ));
    let deferred = app
        .sidecar_restore
        .as_ref()
        .unwrap()
        .common
        .deferred_fullscreen
        .as_ref()
        .unwrap();
    assert_eq!(deferred.idx, 1);
    assert_eq!(deferred.landing, None);
    app.items_generation += 1;
    assert!(!app.sidecar_restore_deferred_fullscreen_wait_active());
    app.finish_page_identity_preparation_for_test();
    assert!(app.fullscreen_idx.is_none());
    assert!(app.fs_initial_placement_request.is_none());
}
