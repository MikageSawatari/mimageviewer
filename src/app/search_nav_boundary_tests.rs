//! Search departure must hand the departing viewer to its window before changing main.
#![cfg(windows)]

use super::collection_grid::CollectionHistoryPreparePoll;
use super::tests::phase_c_support::AppTestEnv;
use super::top_level_grid_view::{
    CollectionGridIdentity, CollectionGridRestore, TopLevelGridSurface,
};
use super::*;
use crate::collection_store::CollectionStoreRuntime;
use crate::ui_main::LocationJumpTarget;

const WINDOW: u64 = 68_812;

#[derive(Clone, Copy, Debug)]
enum Destination {
    Location,
    SavedBook,
    Collection,
    Rating,
}

#[derive(Debug, PartialEq)]
struct Departure {
    context: ViewerContextId,
    folder: Option<PathBuf>,
    rows: Vec<GridItem>,
    selected: Option<usize>,
    generation: u64,
    surface: TopLevelGridSurface,
    return_origin: String,
    global_active: bool,
    global_view: String,
    global_query: String,
    global_owner: String,
    global_run: u64,
    global_saved_folder: Option<PathBuf>,
    search_rows: [bool; 2],
    player: Option<(u64, bool, bool)>,
    permission_demand: usize,
}

fn departure(app: &App) -> Departure {
    Departure {
        context: app.projected_viewer_context_id(),
        folder: app.current_folder.clone(),
        rows: app.items.clone(),
        selected: app.selected,
        generation: app.items_generation,
        surface: app.top_level_grid_view.surface().clone(),
        return_origin: format!("{:?}", app.top_level_grid_view.return_origin()),
        global_active: app.global_search.active,
        global_view: format!("{:?}", app.global_search.view()),
        global_query: app.global_search.last_executed.clone(),
        global_owner: match &app.global_search.accepted {
            crate::global_search_ui::SearchAccepted::Metadata { applied } => {
                format!("Metadata({applied:?})")
            }
            crate::global_search_ui::SearchAccepted::Everything {
                submitted,
                dataset,
                position,
            } => {
                let position = match position {
                    crate::global_search_ui::EverythingPosition::Root => "Root".into(),
                    crate::global_search_ui::EverythingPosition::PhysicalChild {
                        logical_source,
                        return_anchor,
                        route,
                    } => {
                        format!("Child({logical_source:?}, {return_anchor:?}, {route:?})")
                    }
                };
                format!(
                    "Everything({submitted:?}, {:p}, {position})",
                    Arc::as_ptr(dataset)
                )
            }
        },
        global_run: app.global_search.run_sequence,
        global_saved_folder: app.global_search.saved_folder.clone(),
        search_rows: [app.items_are_global_search_view, app.items_are_tag_view],
        player: match app.fs_cache.get(&0) {
            Some(FsCacheEntry::Video { player, .. }) => Some((
                player.media_identity(),
                player.intent_playing(),
                player.is_playing(),
            )),
            _ => None,
        },
        permission_demand: app.presentation_demand_count_for_test(),
    }
}

fn searching(app: &App) -> bool {
    app.global_search.active || app.favsearch.active || app.tag_view.active
}

fn fixture(kind: usize, child: bool) -> AppTestEnv {
    let mut app = if child {
        super::tests::phase_c_support::setup_app()
    } else {
        App::navigation_search_fixture_for_test(kind)
    };
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    let origin = app.tmp.path().join("pre-search");
    std::fs::create_dir_all(&origin).unwrap();
    if child {
        let child = app.tmp.path().join("result-child");
        std::fs::create_dir_all(&child).unwrap();
        app.load_folder(origin);
        super::section375_tests::settle(&mut app, &egui::Context::default());
        super::search_nav_entry_inventory_tests::admit_search_result_child(
            &mut app,
            &egui::Context::default(),
            kind,
            &child,
        );
    }
    // Use a disconnected player below; no native decoder or application window starts.
    let media = if child {
        app.current_folder.as_ref().unwrap().join("departing.mp4")
    } else {
        app.tmp.path().join("departing.mp4")
    };
    std::fs::write(&media, b"media").unwrap();
    app.install_new_items(vec![GridItem::Video(media)], vec![None]);
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.finish_page_identity_preparation_for_test();
    app
}

fn mount_media(app: &mut App) {
    let GridItem::Video(source) = &app.items[0] else {
        panic!("video fixture")
    };
    let source = source.clone();
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.set_detached_window_binding_for_test(Some(WINDOW));
    app.begin_mounted_detached_session_for_test(WINDOW, DetachedSource::Video);
    app.fs_cache.insert(
        0,
        FsCacheEntry::Video {
            player: Box::new(crate::video::VideoPlayer::disconnected_for_test(
                source, 12.0,
            )),
            load_seq: 0,
        },
    );
    let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&0) else {
        unreachable!()
    };
    player.confirm_playback_start_for_test(crate::video::PlaybackStartOrigin::NewSource);
    assert!(player.is_playing());
    app.install_allowed_presentation_for_test(&[permission_occurrence()]);
}

fn permission_occurrence() -> crate::book_composition::PresentationOccurrence {
    use crate::book_composition::*;
    PresentationOccurrence {
        item_index: 0,
        navigation_anchor: 0,
        role: PresentationRole::Navigation,
        slice: PresentationSlice::Full,
        slot: PresentationSlot::Left,
    }
}

fn collection(app: &mut AppTestEnv) -> CollectionGridRestore {
    let runtime =
        CollectionStoreRuntime::start_at(app.tmp.path().join("boundary-collections.db")).unwrap();
    let client = runtime.client();
    app.install_collection_runtime(runtime);
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !matches!(app.collection_store_client_for_read(), Ok(Some(_))) {
        assert!(
            Instant::now() < deadline,
            "collection runtime initialization"
        );
        app.poll_collection_ui(&ctx);
        std::thread::yield_now();
    }
    let snapshot = client
        .create_collection("Destination".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .unwrap();
    while app.collection_catalog_revision(snapshot.collection_id()) != Some(snapshot.revision()) {
        assert!(
            Instant::now() < deadline,
            "collection catalog initialization"
        );
        app.poll_collection_ui(&ctx);
        std::thread::yield_now();
    }
    CollectionGridRestore {
        identity: CollectionGridIdentity {
            collection_id: snapshot.collection_id(),
        },
        revision_at_open: snapshot.revision(),
        viewport_anchor: None,
        return_to: None,
    }
}

fn dispatch(app: &mut App, ctx: &egui::Context, nav: Option<crate::ui_main::AddressBarNav>) {
    app.post_render_main_navigation(
        ctx,
        update_phases::MainFrameNavigation {
            fav_nav: None,
            toolbar_fav_nav: None,
            fullscreen_close_nav: None,
            keyboard_nav: nav,
            keyboard_nav_from_fullscreen_close: false,
            gamepad_nav: None,
            address_nav: None,
            open_folder_nav: None,
            context_nav: None,
            folder_pane_nav: None,
            grid_nav: None,
        },
    );
}

fn poll(app: &mut App, ctx: &egui::Context) {
    app.settle_open_path_classification_for_test();
    app.poll_book_op_pending(ctx);
    app.poll_collection_ui(ctx);
    app.poll_saved_group_open(ctx);
    app.poll_history_navigation_transition_for_test(ctx);
    app.poll_collection_grid(ctx);
    app.poll_rating_view();
    app.poll_sidecar_restore(ctx);
    dispatch(app, ctx, None);
}

fn settle(app: &mut App, ctx: &egui::Context, accepted: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        poll(app, ctx);
        if accepted(app)
            && !app.sidecar_restore_active()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.saved_group_open_request_id_for_test().is_none()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "destination did not settle; folder={:?} toast={:?}",
            app.current_folder,
            app.fs_feedback_toast.as_ref().map(|toast| &toast.0)
        );
        std::thread::yield_now();
    }
}

fn complete_location(app: &mut App, destination: &Path) {
    let pending = app
        .folder_pane_open_pending
        .take()
        .expect("physical location scan");
    pending
        .cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let ready = FolderPaneOpenReady {
        navigation: pending.navigation,
        epub_restore: pending.epub_restore,
        path: pending.path,
        purpose: pending.purpose,
        scan: Ok(scan_directory(destination)),
    };
    assert!(
        app.resolve_main_folder_open_ready(&egui::Context::default(), ready)
            .is_none()
    );
}

fn assert_back(app: &mut App, ctx: &egui::Context, origin: &Path) {
    assert!(
        !searching(app),
        "main destination must have no search owner"
    );
    assert_eq!(
        app.folder_history_back_entry(),
        Some(&FolderNavHistoryTarget::Path(origin.to_path_buf()).into())
    );
    let nav = app
        .history_key_navigation(FolderHistoryDirection::Back)
        .expect("real Back");
    dispatch(app, ctx, Some(nav));
    settle(app, ctx, |app| {
        app.current_folder.as_deref() == Some(origin)
    });
    assert!(!searching(app), "Back returns before the search");
}

fn matrix(destination: Destination) {
    for kind in 0..4 {
        for child in [false, true] {
            for detached in [false, true] {
                let mut app = fixture(kind, child);
                let ctx = egui::Context::default();
                let origin = app.tmp.path().join("pre-search");
                let path = app.tmp.path().join("destination");
                std::fs::create_dir_all(&path).unwrap();
                let collection =
                    matches!(destination, Destination::Collection).then(|| collection(&mut app));
                if matches!(destination, Destination::SavedBook) {
                    let shelf = app.tmp.path().join("books");
                    std::fs::create_dir_all(shelf.join("Book01")).unwrap();
                    app.settings.book_root = Some(shelf);
                    app.settings.active_book_name = "Book01".into();
                    app.request_book_list_refresh();
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while app.book_op_pending.is_some() || app.book_shelf.snapshot.is_none() {
                        assert!(Instant::now() < deadline, "saved book catalog");
                        app.poll_book_op_pending(&ctx);
                        std::thread::yield_now();
                    }
                }
                if detached {
                    mount_media(&mut app);
                }
                let before = departure(&app);
                match destination {
                    Destination::Location => {
                        app.dispatch_location_jump(LocationJumpTarget::Physical(path.clone()));
                        assert_eq!(departure(&app), before, "scan admission must retain source");
                        complete_location(&mut app, &path);
                        settle(&mut app, &ctx, |app| {
                            app.current_folder.as_ref() == Some(&path)
                        });
                    }
                    Destination::SavedBook => {
                        let nav =
                            app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
                        assert_eq!(departure(&app), before, "book admission must retain source");
                        dispatch(&mut app, &ctx, nav);
                        let path = app.tmp.path().join("books/Book01");
                        settle(&mut app, &ctx, |app| {
                            app.current_folder.as_ref() == Some(&path)
                        });
                    }
                    Destination::Collection => {
                        let id = collection.as_ref().unwrap().identity.collection_id;
                        app.settings.toolbar_collection_target_id = Some(id.as_uuid());
                        let nav = app.apply_saved_group_key_action(
                            &ctx,
                            KeyAction::GridOpenCollectionTarget,
                        );
                        assert_eq!(
                            departure(&app),
                            before,
                            "collection admission must retain source"
                        );
                        dispatch(&mut app, &ctx, nav);
                        settle(&mut app, &ctx, |app| {
                            app.top_level_grid_view
                                .collection_session()
                                .is_some_and(|session| session.identity.collection_id == id)
                        });
                    }
                    Destination::Rating => {
                        app.dispatch_location_jump(LocationJumpTarget::Rating(3));
                        assert_eq!(
                            departure(&app),
                            before,
                            "rating admission must retain source"
                        );
                        settle(&mut app, &ctx, |app| {
                            app.items_are_rating_view && app.rating_view_pending.is_none()
                        });
                        assert_eq!(app.rating_view_stars, 3);
                    }
                }
                assert!(
                    !searching(&app),
                    "{destination:?} kind={kind} child={child} detached={detached}"
                );
                if detached {
                    app.with_window_viewer_context(WINDOW, |window| {
                        assert_eq!(departure(window), before,
                            "{destination:?} kind={kind} child={child}: transferred bundle was changed");
                        assert_eq!(window.fullscreen_idx, Some(0));
                    }).expect("departing window retains its context");
                } else {
                    assert!(app.locate_window_context(WINDOW).is_none());
                }
                assert_back(&mut app, &ctx, &origin);
                app.shutdown_collection_runtime_for_exit();
            }
        }
    }
}

#[test]
fn search_nav_boundary_physical_location_preserves_departing_context_and_real_back() {
    matrix(Destination::Location);
}

#[test]
fn search_nav_boundary_saved_book_preserves_departing_context_and_real_back() {
    matrix(Destination::SavedBook);
}

#[test]
fn search_nav_boundary_collection_preserves_departing_context_and_real_back() {
    matrix(Destination::Collection);
}

#[test]
fn search_nav_boundary_rating_preserves_departing_context_and_real_back() {
    matrix(Destination::Rating);
}

#[test]
fn search_nav_boundary_drive_list_preserves_departing_context_and_real_back() {
    for kind in 0..4 {
        let mut app = fixture(kind, false);
        mount_media(&mut app);
        let before = departure(&app);
        app.dispatch_location_jump(LocationJumpTarget::DriveList);
        assert!(app.items_are_drive_list);
        assert!(!searching(&app));
        app.with_window_viewer_context(WINDOW, |window| assert_eq!(departure(window), before))
            .expect("drive list handoff preserves departing media");
        let origin = app.tmp.path().join("pre-search");
        assert_back(&mut app, &egui::Context::default(), &origin);
    }
}

#[test]
fn search_nav_boundary_shared_search_owners_match_existing_plain_collection_open() {
    for kind in [2, 3] {
        let mut owners = Vec::new();
        for explicit in [false, true] {
            let mut app = fixture(kind, false);
            let restore = collection(&mut app);
            mount_media(&mut app);
            if explicit {
                let navigation = exit_navigation(&mut app);
                let prepared = prepare_collection(&mut app, restore.clone());
                assert!(app.adopt_collection_history_root(restore, prepared, None, navigation));
            } else {
                app.open_collection_grid(restore.identity.collection_id, None);
            }
            owners.push(
                app.with_window_viewer_context(WINDOW, |window| {
                    [window.favsearch.active, window.tag_view.active]
                })
                .unwrap(),
            );
            app.shutdown_collection_runtime_for_exit();
        }
        // This is a parity check, not a contract requiring the existing shared-owner gap.
        assert_eq!(
            owners[0], owners[1],
            "kind={kind}: explicit open must match Plain"
        );
    }
}

fn exit_navigation(app: &mut App) -> MainListNavigation {
    let proof = app.copied_destination_source_proof();
    app.capture_main_list_navigation(
        MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
            selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
        }),
        proof,
    )
}

fn prepare_collection(
    app: &mut App,
    restore: CollectionGridRestore,
) -> top_level_grid_view::CollectionGridPreparedInstall {
    let mut pending = app.start_collection_history_prepare(restore).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match app.poll_collection_history_prepare(&mut pending) {
            CollectionHistoryPreparePoll::Pending => {
                assert!(Instant::now() < deadline, "collection prepare");
                std::thread::yield_now();
            }
            CollectionHistoryPreparePoll::Ready(prepared) => return prepared,
            CollectionHistoryPreparePoll::Failed(message) => panic!("{message}"),
        }
    }
}

#[test]
fn search_nav_boundary_collection_lock_after_prepare_keeps_source_and_history() {
    for kind in 0..4 {
        let mut app = fixture(kind, false);
        let restore = collection(&mut app);
        mount_media(&mut app);
        let navigation = exit_navigation(&mut app);
        let prepared = prepare_collection(&mut app, restore.clone());
        let before = departure(&app);
        let history = app.folder_nav_history_snapshot();
        let sequence = app.top_level_grid_view.collection_navigation_sequence();
        let contexts = app.viewer_context_ids();
        let binding = app.locate_window_context(WINDOW);
        app.fs_nav_locked_gen = Some(app.items_generation);
        assert!(!app.adopt_collection_history_root(restore, prepared, None, navigation));
        assert_eq!(departure(&app), before);
        assert_eq!(
            app.folder_nav_history_snapshot().back_stack,
            history.back_stack
        );
        assert_eq!(
            app.folder_nav_history_snapshot().forward_stack,
            history.forward_stack
        );
        assert_eq!(
            app.top_level_grid_view.collection_navigation_sequence(),
            sequence
        );
        assert_eq!(app.viewer_context_ids(), contexts);
        assert_eq!(app.locate_window_context(WINDOW), binding);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(searching(&app));
        app.fs_nav_locked_gen = None;
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn search_nav_boundary_image_park_keeps_picture_reopen_identity_and_window() {
    for kind in 0..4 {
        for collection_target in [false, true] {
            let mut app = fixture(kind, false);
            let ctx = egui::Context::default();
            let restore = collection_target.then(|| collection(&mut app));
            let picture = app.tmp.path().join("original.png");
            image::RgbImage::new(2, 1).save(&picture).unwrap();
            app.install_new_items(vec![GridItem::Image(picture.clone())], vec![None]);
            app.visible_indices = vec![0];
            app.selected = Some(0);
            app.settings.detached_viewer_open_images_in_window = true;
            let pixels = egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2]);
            let tex = ctx.load_texture(
                "departure-image",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            let texture_id = tex.id();
            app.fs_cache.insert(
                0,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels: Arc::new(pixels),
                    source_dims: Some([2, 1]),
                    load_seq: 0,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = ViewerPresentation::DetachedWindow;
            app.set_detached_window_binding_for_test(Some(WINDOW));
            app.begin_mounted_detached_session_for_test(WINDOW, DetachedSource::Image);
            let generation = app.items_generation;
            if let Some(restore) = restore {
                let navigation = exit_navigation(&mut app);
                let prepared = prepare_collection(&mut app, restore.clone());
                assert!(app.adopt_collection_history_root(restore, prepared, None, navigation));
            } else {
                let destination = app.tmp.path().join("image-destination");
                std::fs::create_dir_all(&destination).unwrap();
                app.dispatch_location_jump(LocationJumpTarget::Physical(destination.clone()));
                complete_location(&mut app, &destination);
            }
            assert!(!searching(&app));
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.detached_image_windows.len(), 1);
            assert_eq!(
                app.detached_window_state(WINDOW),
                Some(DetachedWindowState::Parked)
            );
            let parked = &app.detached_image_windows[0];
            assert_eq!(parked.id, WINDOW);
            let DetachedImageWindowDisplay::Still(resource) = &parked.display else {
                panic!("parked picture must keep its original display")
            };
            assert_eq!(resource.source_texture_id(), texture_id);
            assert_eq!(
                parked.reopen_descriptor,
                Some(ViewerContextDescriptor::Image {
                    path: picture.clone()
                })
            );
            assert_eq!(
                parked
                    .reopen_sync_stamp
                    .as_ref()
                    .map(|stamp| stamp.items_generation),
                Some(generation)
            );
            app.shutdown_collection_runtime_for_exit();
        }
    }
}

#[test]
fn search_nav_boundary_receipt_prevents_pdf_and_loading_items_from_closing_again() {
    for pdf in [false, true] {
        let mut app = fixture(1, false);
        mount_media(&mut app);
        let source = departure(&app);
        let path = app.tmp.path().join(if pdf {
            "destination.pdf"
        } else {
            "destination"
        });
        if pdf {
            std::fs::write(&path, b"prepared pdf rows").unwrap();
        } else {
            std::fs::create_dir_all(&path).unwrap();
        }
        let rows = if pdf {
            vec![GridItem::PdfPage {
                pdf_path: path.clone(),
                page_num: 0,
                content_type: None,
            }]
        } else {
            Vec::new()
        };
        let token = app
            .prepare_visible_install(&path, &rows, Some(&path))
            .unwrap();
        let plan = app.physical_main_list_boundary_plan(&token);
        let navigation = exit_navigation(&mut app);
        let destination = app.main_list_requested_destination(
            &navigation,
            FolderNavHistoryTarget::Path(path.clone()),
            pdf.then_some(""),
        );
        let adoption = app
            .prepare_main_list_adoption(navigation, destination)
            .unwrap();
        assert!(
            app.adopt_main_list_navigation(adoption, plan, |app, boundary| {
                assert!(!boundary.uses_installer_boundary());
                // A fresh main starts after promotion. Establish observable state after
                // the first boundary: a second close would advance this sequence and
                // retire this valid presentation demand.
                for _ in 0..17 {
                    app.top_level_grid_view
                        .advance_collection_navigation_sequence();
                }
                app.items = vec![GridItem::Image(path.with_extension("png"))];
                app.install_allowed_presentation_for_test(&[permission_occurrence()]);
                let sequence = app.top_level_grid_view.collection_navigation_sequence();
                assert_eq!(app.presentation_demand_count_for_test(), 1);
                if pdf {
                    let lease = crate::pdf_loader::LeasedEpubPath::try_new(path.clone()).unwrap();
                    app.prepare_pdf_visible_adoption(&lease, boundary);
                    assert_eq!(
                        app.top_level_grid_view.collection_navigation_sequence(),
                        sequence,
                        "PDF preparation must consume the already-handled boundary"
                    );
                    assert_eq!(
                        app.presentation_demand_count_for_test(),
                        1,
                        "PDF preparation must not retire a new main permission"
                    );
                }
                let hydration = app.install_loading_items_inner(
                    path.clone(),
                    rows.clone(),
                    vec![None; rows.len()],
                    HashSet::new(),
                    Vec::new(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    VisibleInstallAuthority::Ordinary,
                    token.with_install_boundary(boundary),
                    None,
                );
                assert_eq!(
                    app.top_level_grid_view.collection_navigation_sequence(),
                    sequence,
                    "loading-items must not repeat the close boundary"
                );
                hydration
            })
        );
        app.with_window_viewer_context(WINDOW, |window| assert_eq!(departure(window), source))
            .unwrap();
    }
}

#[test]
fn search_nav_boundary_direct_pdf_preserves_search_before_destination_presentation() {
    for kind in 0..4 {
        let mut app = fixture(kind, false);
        mount_media(&mut app);
        let before = departure(&app);
        let path = app.tmp.path().join("prepared-destination.pdf");
        std::fs::write(&path, b"enumeration already prepared").unwrap();
        let source = crate::pdf_loader::LeasedEpubPath::try_new(path.clone()).unwrap();
        let navigation = exit_navigation(&mut app);
        let adoption = Box::new(DirectPdfAdoption {
            navigation,
            grid_open_effects: None,
        });
        assert!(app.adopt_direct_pdf_rows(
            &path,
            &OpenRequestOwner::Navigation,
            &source,
            Some(adoption),
            vec![GridItem::PdfPage {
                pdf_path: path.clone(),
                page_num: 0,
                content_type: None
            }],
            vec![None],
            HashSet::new(),
            sidecar_restore::FirstDisplayAction::None,
            StartupListIntent::ExplicitList
        ));
        assert!(!searching(&app));
        assert_eq!(app.current_folder.as_ref(), Some(&path));
        app.with_window_viewer_context(WINDOW, |window| assert_eq!(departure(window), before))
            .unwrap();
        let ctx = egui::Context::default();
        settle(&mut app, &ctx, |app| {
            app.current_folder.as_ref() == Some(&path)
        });
        let origin = app.tmp.path().join("pre-search");
        assert_back(&mut app, &ctx, &origin);
    }
}

#[test]
fn search_nav_boundary_collection_child_preserves_departure_through_nested_physical_install() {
    use crate::collection_store::{CollectionRegistration, CollectionResolvedKind};
    use top_level_grid_view::CollectionGridViewportAnchor;
    for kind in 0..4 {
        let one_boundary_sequence = {
            let mut reference = fixture(kind, false);
            let restore = collection(&mut reference);
            mount_media(&mut reference);
            reference.open_collection_grid(restore.identity.collection_id, None);
            let sequence = reference
                .top_level_grid_view
                .collection_navigation_sequence();
            reference.shutdown_collection_runtime_for_exit();
            sequence
        };
        let mut app = fixture(kind, false);
        let mut restore = collection(&mut app);
        let child = app.tmp.path().join("collection-child");
        std::fs::create_dir_all(&child).unwrap();
        let client = app
            .collection_store_client_for_read()
            .unwrap()
            .unwrap()
            .clone();
        let added = client
            .add_batch(
                restore.identity.collection_id,
                restore.revision_at_open,
                vec![
                    CollectionRegistration::from_trusted_path(
                        &child,
                        CollectionResolvedKind::Folder,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap()
            .snapshot;
        restore.revision_at_open = added.revision();
        restore.viewport_anchor = Some(CollectionGridViewportAnchor {
            entry_id: added.entries[0].id,
            source_key: added.entries[0].source_key.clone(),
        });
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.collection_catalog_revision(restore.identity.collection_id)
            != Some(added.revision())
        {
            assert!(
                Instant::now() < deadline,
                "collection child catalog revision"
            );
            app.poll_collection_ui(&ctx);
            std::thread::yield_now();
        }
        mount_media(&mut app);
        let before = departure(&app);
        let prepared = prepare_collection(&mut app, restore.clone());
        let target = collection_grid::CollectionHistoryChildTarget {
            root: restore,
            root_source_path: child.clone(),
            visible_path: child.clone(),
            kind: CollectionResolvedKind::Folder,
        };
        let navigation = exit_navigation(&mut app);
        assert!(app.adopt_collection_history_child_with_backing(
            &target,
            prepared,
            collection_navigation::PhysicalHistoryPreflightPayload::Folder(scan_directory(&child)),
            None,
            None,
            None,
            navigation
        ));
        assert!(!searching(&app));
        assert_eq!(app.current_folder.as_ref(), Some(&child));
        app.with_window_viewer_context(WINDOW, |window| assert_eq!(departure(window), before))
            .unwrap();
        // Both paths hand off once and begin one Collection session. The existing
        // Plain open also retires any App-shared search owner, which may advance
        // the new main's sequence before that begin. Compare to that real boundary
        // rather than fixing its current shared-owner behavior into this test.
        assert_eq!(
            app.top_level_grid_view.collection_navigation_sequence(),
            one_boundary_sequence,
            "kind={kind}: nested physical install must not repeat the close boundary"
        );
        settle(&mut app, &ctx, |app| {
            app.current_folder.as_ref() == Some(&child)
        });
        let origin = app.tmp.path().join("pre-search");
        assert_back(&mut app, &ctx, &origin);
        app.shutdown_collection_runtime_for_exit();
    }
}
