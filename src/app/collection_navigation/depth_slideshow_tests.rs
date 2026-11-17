//! Collection outer slideshow retirement through the three accepted installers.
//! Actor preparation and physical adoption are real; no native window or GPU is created.
use super::tests::{prepare_snapshot, prepared_thumbnail_sources, recv, start_ready_app};
use super::*;
use crate::app::AppTestEnvForTest;
use crate::depth_parallax::DepthViewState;
use crate::fs_exclusive_view::FsExclusiveView;

type PreparedOuter = (
    tempfile::TempDir,
    AppTestEnvForTest,
    egui::Context,
    CollectionNavigationRequest,
    CollectionRevisionWatch,
    Arc<CollectionPreparedSnapshot>,
    CollectionNavigationPreflightReady,
);

fn prepared_outer(resume_slideshow: bool) -> PreparedOuter {
    let files = tempfile::tempdir().unwrap();
    let source = files.path().join("source.png");
    let target = files.path().join("next-book");
    std::fs::create_dir(&target).unwrap();
    for path in [&source, &target.join("page.png")] {
        image::RgbaImage::from_pixel(4, 6, image::Rgba([40, 80, 120, 255]))
            .save(path)
            .unwrap();
    }
    let (mut app, client) = start_ready_app(&files.path().join("collection.db"));
    app.active_quick_folder_slot = None;
    let created = recv(
        client
            .create_collection("Depth outer navigation".into())
            .unwrap(),
    );
    let added = recv(
        client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &source,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &target,
                        CollectionResolvedKind::Folder,
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
    );
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.collection_catalog_contains(added.snapshot.collection_id()) {
        assert!(
            Instant::now() < deadline,
            "the actor catalog must observe its registered collection"
        );
        app.poll_collection_ui(&ctx);
        std::thread::yield_now();
    }
    let root = prepare_snapshot(&added.snapshot);
    app.top_level_grid_view.begin(
        TopLevelGridSurface::Collection(CollectionGridIdentity {
            collection_id: root.collection_id,
        }),
        None,
    );
    let reuse_key =
        app.collection_grid_prepare_reuse_key(root.collection_id, root.collection_revision);
    let page_edit_revision = app.page_edit_revision;
    let mut edits = super::super::page_edit_snapshot::PageEditSnapshot::default();
    edits.stamp = Some(crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample());
    let page_identity_index = super::super::collection_grid::prepare_collection_page_identity_index(
        &root,
        app.settings.file_type_visibility.clone(),
    );
    app.apply_collection_grid_prepared_install(CollectionGridPreparedInstall {
        page_identity_index,
        page_edits: None,
        retained_page_edits: Some(Arc::new(edits)),
        page_edit_revision,
        prepared: (*root).clone(),
        thumbnail_sources: prepared_thumbnail_sources(CollectionGridThumbnailSources::default()),
        auto_aspect_lookup: None,
        reuse_key,
    }, None);
    let source_idx = root
        .entries
        .iter()
        .position(|entry| entry.source_path == source)
        .unwrap();
    let target_idx = root
        .entries
        .iter()
        .position(|entry| entry.source_path == target)
        .unwrap();
    app.fullscreen_idx = Some(source_idx);
    app.selected = Some(source_idx);
    app.slideshow_playing = resume_slideshow;
    app.settings.slideshow_interval_secs = 30.0;
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsDepthParallax = F24\n");
    let owner = app.edit_request_owner_context();
    app.set_fs_exclusive_view(
        FsExclusiveView::Depth(DepthViewState::new(owner)),
        "collection_depth_test",
    );
    app.feedback_toasts_for_test.clear();
    assert!(app.start_collection_outer_fullscreen_navigation(
        &ctx,
        source_idx,
        target_idx > source_idx,
        resume_slideshow,
        false,
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    let (request, watch, root, ready) = loop {
        assert!(
            Instant::now() < deadline,
            "the real outer request must reach physical preflight"
        );
        let pending = app
            .top_level_grid_view
            .take_collection_navigation_pending()
            .expect("the accepted outer request remains owned until adoption");
        if let CollectionNavigationPending::Preflighting {
            request,
            watch,
            prepared,
            receiver,
            ..
        } = pending
        {
            let ready = receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .expect("the registered folder has a displayable page");
            break (request, watch, prepared, ready);
        }
        app.top_level_grid_view
            .set_collection_navigation_pending(Some(pending));
        app.poll_collection_navigation(&ctx);
        std::thread::yield_now();
    };
    assert_eq!(ready.target.source_path, target);
    assert!(matches!(
        &ready.payload,
        CollectionNavigationPreflightPayload::Folder(_)
    ));
    assert!(request.navigation.is_some());
    assert!(request.root_thumbnail_sources.is_some());
    assert!(
        app.fs_exclusive_view.is_depth(),
        "preflight is read-only to the source viewer"
    );
    assert_eq!(notice_count(&app), 0);
    (files, app, ctx, request, watch, root, ready)
}

fn notice_count(app: &App) -> usize {
    app.feedback_toasts_for_test
        .iter()
        .filter(|(message, _)| {
            message.starts_with("3D 写真を終了しました: 次のフォルダへ進みました。")
        })
        .count()
}

fn settle_landing(app: &mut App, ctx: &egui::Context, target: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.current_folder.as_deref() != Some(target) || app.fullscreen_idx.is_none() {
        app.poll_sidecar_restore(ctx);
        app.poll_open_path_classification(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_composition_read(ctx);
        app.poll_collection_ui(ctx);
        app.poll_collection_navigation(ctx);
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_smart_folder(ctx);
        assert!(
            Instant::now() < deadline,
            "accepted Collection physical navigation did not reopen its page"
        );
        std::thread::yield_now();
    }
}

#[derive(Clone, Copy, Debug)]
enum InstallPath {
    PhysicalContinuation,
    OuterAdoption,
    Legacy,
}

fn run_accepted(path: InstallPath, resume_slideshow: bool) {
    let (_files, mut app, ctx, mut request, watch, root, ready) = prepared_outer(resume_slideshow);
    let target = ready.target.source_path.clone();
    match path {
        InstallPath::PhysicalContinuation => {
            let navigation = request.navigation.take().unwrap();
            let restore_intent = request.action.startup_list_intent();
            request.book_owner = Some(ready.owner);
            let owner = Box::new(PreparedCollectionPhysicalNavigation {
                request,
                watch,
                root,
                target: ready.target,
                restore_video_tile: false,
            });
            let CollectionNavigationPreflightPayload::Folder(scan) = ready.payload else {
                unreachable!()
            };
            assert!(app.start_prepared_collection_physical_navigation(
                owner,
                navigation,
                PhysicalHistoryPreflightPayload::Folder(scan),
                restore_intent,
                &ctx,
            ));
        }
        InstallPath::OuterAdoption => {
            // A Folder payload plus the real main navigation proof selects the
            // prepared outer adopter and its actual installation closure.
            app.commit_collection_navigation(&ctx, request, watch, root, ready);
        }
        InstallPath::Legacy => {
            request.navigation = None;
            app.commit_collection_navigation(&ctx, request, watch, root, ready);
        }
    }
    settle_landing(&mut app, &ctx, &target);
    assert!(!app.fs_exclusive_view.is_depth(), "{path:?}");
    assert_eq!(app.slideshow_playing, resume_slideshow, "{path:?}");
    let expected = usize::from(resume_slideshow);
    assert_eq!(notice_count(&app), expected, "{path:?}");
    if resume_slideshow {
        assert!(app.feedback_toasts_for_test.iter().any(|(message, _)| {
            message.contains("次のフォルダへ進みました。F24 キーでもう一度使えます")
        }));
    }
    // A subsequent accepted close sees the already retired Depth owner.
    app.notify_depth_slideshow_folder_nav_exit(resume_slideshow);
    app.close_fullscreen_for_folder_nav_reopen();
    assert_eq!(
        notice_count(&app),
        expected,
        "a second close cannot repeat the notice: {path:?}"
    );
}

#[test]
fn collection_depth_slideshow_physical_continuation_notifies_only_for_automatic_resume() {
    for resume in [true, false] {
        run_accepted(InstallPath::PhysicalContinuation, resume);
    }
}

#[test]
fn collection_depth_slideshow_outer_adoption_notifies_only_for_automatic_resume() {
    for resume in [true, false] {
        run_accepted(InstallPath::OuterAdoption, resume);
    }
}

#[test]
fn collection_depth_slideshow_legacy_commit_notifies_only_for_automatic_resume() {
    for resume in [true, false] {
        run_accepted(InstallPath::Legacy, resume);
    }
}

#[test]
fn collection_depth_slideshow_rejected_or_unprepared_outer_owner_keeps_depth_without_notice() {
    for legacy_missing_owner in [false, true] {
        let (_files, mut app, ctx, mut request, watch, root, ready) = prepared_outer(true);
        let original_idx = app.fullscreen_idx;
        let original_generation = app.items_generation;
        if legacy_missing_owner {
            request.navigation = None;
            request.root_thumbnail_sources = None;
            app.commit_collection_navigation(&ctx, request, watch, root, ready);
        } else {
            let CollectionNavigationPreflightPayload::Folder(scan) = ready.payload else {
                unreachable!()
            };
            let mut target = ready.target;
            target.entry_id = CollectionEntryId::new();
            let restore_intent = request.action.startup_list_intent();
            assert!(
                app.prepare_collection_outer_physical_adoption(
                    &mut request,
                    &root,
                    &target,
                    PhysicalHistoryPreflightPayload::Folder(scan),
                    restore_intent,
                )
                .is_none(),
                "an unregistered prepared target cannot reach installation"
            );
        }
        assert!(app.fs_exclusive_view.is_depth());
        assert_eq!(app.fullscreen_idx, original_idx);
        assert_eq!(app.items_generation, original_generation);
        assert_eq!(notice_count(&app), 0);
    }
}
