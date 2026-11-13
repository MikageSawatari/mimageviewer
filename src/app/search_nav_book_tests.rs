//! Explicit saved-book/collection opens share success-only search departure.
#![cfg(windows)]

use super::*;
use crate::collection_store::{CollectionId, CollectionStoreRuntime};
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use crate::ui_main::AddressBarNav;
use egui_kittest::kittest::Queryable;

#[derive(Clone, Debug)]
enum Target {
    Book(PathBuf),
    Collection(CollectionId),
}

/// The exhaustive action inventory calls the same producer/adoption/Back test.
pub(super) fn assert_saved_group_search_action(action: KeyAction, kind: usize) {
    assert_saved_group_search_entry(action, kind, |app, ctx| {
        app.apply_saved_group_key_action(ctx, action)
    });
}

pub(super) fn assert_saved_group_search_entry(
    action: KeyAction,
    kind: usize,
    trigger: impl FnOnce(&mut App, &egui::Context) -> Option<AddressBarNav>,
) {
    let mut app = fixture(Some(kind));
    let origin = app.tmp.path().join("pre-search");
    let target = configure_target(&mut app, action);
    let source = source_state(&app);
    let ctx = egui::Context::default();
    let nav = trigger(&mut app, &ctx);
    assert_eq!(
        source_state(&app),
        source,
        "producer must retain search: {action:?}"
    );
    dispatch(&mut app, &ctx, nav);
    settle_target(&mut app, &ctx, &target);
    assert_adopted_and_back(&mut app, &ctx, &target, &origin);
}

#[test]
fn search_nav_book_retained_result_child_records_saved_origin_route_and_real_back_restores_it() {
    for kind in 0..4 {
        let mut app = fixture(None);
        let origin = app.tmp.path().join("pre-search");
        let child = app.tmp.path().join("retained-result-child");
        std::fs::create_dir_all(&child).unwrap();
        let target = configure_target(&mut app, KeyAction::GridOpenActiveBook);
        let ctx = egui::Context::default();
        app.load_folder(origin.clone());
        super::section375_tests::settle(&mut app, &ctx);

        // A genuine saved lineage has more than a root scope. Flattening every
        // search departure to Path(origin) would make the path look right while
        // silently losing the original facet-navigation ancestry.
        let original_route = FacetRoute(vec![
            FacetScope::path(app.tmp.path()),
            FacetScope::path(&origin),
        ]);
        app.adopt_main_facet_route(original_route.clone());
        super::search_nav_entry_inventory_tests::admit_search_result_child(
            &mut app, &ctx, kind, &child,
        );
        assert!(search_active(&app));
        assert_eq!(app.current_folder.as_ref(), Some(&child));
        let canonical = app
            .top_level_grid_view
            .return_origin()
            .expect("real search retains its origin");
        assert_eq!(
            canonical.restore,
            top_level_grid_view::TopLevelGridRestore::Folder(origin.clone())
        );
        assert_eq!(canonical.route, original_route);
        let child_route = app.facet_navigation.route().clone();
        assert_ne!(child_route, original_route);
        assert!(!child_route.0.is_empty());

        // The request's evidence must retain the live child's route. Only the
        // history entry recorded after successful adoption may use the origin.
        let proof = app.copied_destination_source_proof();
        let navigation = app.capture_main_list_navigation(
            MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
                selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
            }),
            proof,
        );
        assert_eq!(
            navigation.source_location.as_ref().unwrap().route,
            child_route
        );
        assert!(app.main_list_navigation_is_current(&navigation));
        let nav = app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        assert_eq!(app.facet_navigation.route(), &child_route);
        dispatch(&mut app, &ctx, nav);
        settle_target(&mut app, &ctx, &target);
        assert!(!search_active(&app));
        assert_eq!(
            app.folder_history_back_entry(),
            Some(&FolderNavHistoryEntry::new(
                FolderNavHistoryTarget::Path(origin.clone()),
                original_route.clone(),
            )),
            "kind={kind}: history records the canonical location and its exact saved route",
        );
        let back = app
            .history_key_navigation(FolderHistoryDirection::Back)
            .expect("accepted book exposes real Back");
        dispatch(&mut app, &ctx, Some(back));
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.current_folder.as_ref() != Some(&origin)
            || app
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
            || app.sidecar_restore_active()
        {
            assert!(
                Instant::now() < deadline,
                "kind={kind}: original lineage Back"
            );
            app.poll_history_navigation_transition_for_test(&ctx);
            app.poll_sidecar_restore(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(app.facet_navigation.route(), &original_route);
        assert!(!search_active(&app));
    }
}

#[test]
fn search_nav_book_saved_actions_plain_preserve_book_display_and_back() {
    for action in SAVED_GROUP_ACTIONS.iter().copied().filter(|action| {
        action.book_slot_number().is_some()
            || action.collection_slot_number().is_some()
            || matches!(
                action,
                KeyAction::GridOpenActiveBook
                    | KeyAction::GridOpenCollectionTarget
                    | KeyAction::GridBookPrev
                    | KeyAction::GridBookNext
                    | KeyAction::GridCollectionPrev
                    | KeyAction::GridCollectionNext
            )
    }) {
        let mut app = fixture(None);
        let origin = app.tmp.path().join("pre-search");
        let target = configure_target(&mut app, action);
        let ctx = egui::Context::default();
        let nav = app.apply_saved_group_key_action(&ctx, action);
        dispatch(&mut app, &ctx, nav);
        settle_target(&mut app, &ctx, &target);
        assert_adopted_and_back(&mut app, &ctx, &target, &origin);
    }
}

#[test]
fn search_nav_book_saved_scan_failure_cancel_and_stale_retain_each_search() {
    for kind in 0..4 {
        for outcome in ["failure", "cancel", "stale", "late-modal"] {
            let mut app = fixture(Some(kind));
            let Target::Book(target) = configure_target(&mut app, KeyAction::GridOpenActiveBook)
            else {
                unreachable!()
            };
            if outcome == "failure" {
                std::fs::remove_dir(&target).unwrap();
            }
            let before = source_state(&app);
            let ctx = egui::Context::default();
            assert!(
                app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook)
                    .is_none()
            );
            let id = app.saved_group_open_request_id_for_test().unwrap();
            match outcome {
                "cancel" => assert!(app.cancel_saved_group_open_request(id)),
                "stale" => {
                    // Source replacement uses the existing switch epoch and
                    // retires the old request without changing accepted rows.
                    app.retire_main_list_requests_for_surface_switch();
                }
                _ => {}
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            while app.saved_group_open_request_id_for_test().is_some() {
                assert!(
                    Instant::now() < deadline,
                    "{outcome}: saved book request did not finish"
                );
                app.poll_saved_group_open(&ctx);
                if app.saved_group_ready_nav().is_some() {
                    if outcome == "late-modal" {
                        app.show_delete_confirm = true;
                    }
                    app.adopt_saved_group_ready(id);
                }
                std::thread::yield_now();
            }
            assert_eq!(source_state(&app), before, "kind={kind}, outcome={outcome}");
            assert!(search_active(&app));
            if matches!(outcome, "failure" | "late-modal") {
                assert!(app.fs_feedback_toast.is_some());
            }
        }
    }
}

#[test]
fn search_nav_book_ring_actions_share_the_saved_group_success_boundary() {
    for kind in 0..4 {
        for ring in [
            RingActionId::OpenBook1,
            RingActionId::BookPrev,
            RingActionId::BookNext,
            RingActionId::OpenCollection1,
            RingActionId::CollectionPrev,
            RingActionId::CollectionNext,
        ] {
            let action = ring.saved_group_key_action().unwrap();
            let mut app = fixture(Some(kind));
            let origin = app.tmp.path().join("pre-search");
            let target = configure_target(&mut app, action);
            let source = source_state(&app);
            let ctx = egui::Context::default();
            let nav = app.apply_ring_action(
                &ctx,
                RingShortcutContext::Grid,
                ring,
                "search-nav-book-test",
            );
            assert_eq!(source_state(&app), source);
            dispatch(&mut app, &ctx, nav);
            settle_target(&mut app, &ctx, &target);
            assert_adopted_and_back(&mut app, &ctx, &target, &origin);
        }
    }
}

#[test]
fn search_nav_book_cold_collection_catalog_reenters_the_shared_prepared_open() {
    for kind in 0..4 {
        let mut app = fixture(Some(kind));
        let origin = app.tmp.path().join("pre-search");
        // Starting the runtime leaves its first catalog read outstanding. The
        // actual saved-group request owns that wait, then hands off preparation.
        let runtime = initialized_collection_runtime(app.tmp.path().join("cold-collections.db"));
        let client = runtime.client();
        let created = client
            .create_collection("Cold target".into())
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let target = Target::Collection(created.collection_id());
        app.install_collection_runtime(runtime);
        let before = source_state(&app);
        let ctx = egui::Context::default();
        let nav = app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenCollection1);
        assert!(nav.is_none());
        assert!(app.saved_group_open_request_id_for_test().is_some());
        assert_eq!(source_state(&app), before);
        settle_target(&mut app, &ctx, &target);
        assert_adopted_and_back(&mut app, &ctx, &target, &origin);
    }
}

#[test]
fn search_nav_book_collection_catalog_cancel_and_stale_keep_each_search() {
    for kind in 0..4 {
        for stale in [false, true] {
            let mut app = fixture(Some(kind));
            app.collection_ui.set_read_phase_for_test(true);
            let before = source_state(&app);
            let ctx = egui::Context::default();
            assert!(
                app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenCollection1)
                    .is_none()
            );
            let id = app.saved_group_open_request_id_for_test().unwrap();
            if stale {
                app.retire_main_list_requests_for_surface_switch();
                app.poll_saved_group_open(&ctx);
            } else {
                assert!(app.cancel_saved_group_open_request(id));
            }
            assert!(app.saved_group_open_request_id_for_test().is_none());
            assert_eq!(source_state(&app), before);
            assert!(search_active(&app));
        }
    }
}

#[test]
fn search_nav_book_cached_collection_and_late_preparation_reject_same_source_epoch_change() {
    for kind in 0..4 {
        for handed_off in [false, true] {
            let mut app = fixture(Some(kind));
            configure_target(&mut app, KeyAction::GridOpenCollection1);
            let before = source_state(&app);
            let ctx = egui::Context::default();
            assert!(
                app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenCollection1)
                    .is_none()
            );
            if handed_off {
                dispatch(&mut app, &ctx, None);
                let pending = app
                    .top_level_grid_view
                    .take_history_navigation_transition()
                    .expect("cached saved collection hands its source proof to preparation");
                app.retire_main_list_requests_for_surface_switch();
                // Supply the old worker's still-live completion after replacement
                // to verify the owning navigation proof, not merely cancellation.
                app.top_level_grid_view
                    .set_history_navigation_transition(Some(pending));
                app.poll_history_navigation_transition_for_test(&ctx);
                assert!(
                    app.top_level_grid_view
                        .history_navigation_transition()
                        .is_none()
                );
            } else {
                app.retire_main_list_requests_for_surface_switch();
                app.poll_saved_group_open(&ctx);
                assert!(app.saved_group_open_request_id_for_test().is_none());
            }
            assert_eq!(source_state(&app), before);
            assert!(search_active(&app));
        }
    }
}

#[test]
fn search_nav_book_deleted_collection_preparation_preserves_each_search() {
    for kind in 0..4 {
        let mut app = fixture(Some(kind));
        let Target::Collection(id) = configure_target(&mut app, KeyAction::GridOpenCollection1)
        else {
            unreachable!()
        };
        let ctx = egui::Context::default();
        let nav = app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenCollection1);
        assert!(nav.is_none());
        assert!(app.saved_group_open_request_id_for_test().is_some());
        let before = source_state(&app);
        let client = app.collection_store_client_for_read().unwrap().unwrap();
        let snapshot = client
            .load_collection(id)
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        client
            .delete_collection(id, snapshot.revision())
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        // Keep the already-resolved cached-catalog candidate, as a real delete
        // can race between input and frame-tail dispatch.
        dispatch(&mut app, &ctx, nav);
        let deadline = Instant::now() + Duration::from_secs(15);
        while app
            .top_level_grid_view
            .history_navigation_transition()
            .is_some()
        {
            assert!(Instant::now() < deadline, "deleted collection preparation");
            app.poll_collection_ui(&ctx);
            app.poll_history_navigation_transition_for_test(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(source_state(&app), before);
        assert!(search_active(&app));
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn search_nav_book_manager_owned_opens_adopt_with_dialog_open_and_keep_plain() {
    for kind in [Some(0), Some(1), Some(2), Some(3), None] {
        for collection in [false, true] {
            let mut app = fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            let action = if collection {
                KeyAction::GridOpenCollection1
            } else {
                KeyAction::GridOpenActiveBook
            };
            let target = configure_target(&mut app, action);
            let before = source_state(&app);
            let ctx = egui::Context::default();
            match &target {
                Target::Book(path) => {
                    app.show_book_manager = true;
                    app.open_book_manager_target(path.clone());
                }
                Target::Collection(id) => {
                    app.open_collection_manager(Some(*id));
                    assert!(app.collection_manager_open());
                    app.open_collection_grid_from_manager(*id);
                }
            }
            if kind.is_some() {
                assert_eq!(
                    source_state(&app),
                    before,
                    "manager admission must keep search while preparing"
                );
            }
            settle_target(&mut app, &ctx, &target);
            if collection {
                // The manager uses an egui Window close control, not an Escape
                // handler. Close its actual rendered title-bar button before Back.
                {
                    let mut harness = crate::test_ui::builder()
                        .with_context_setup(crate::ui_fonts::configure_fonts)
                        .with_size(egui::vec2(1200.0, 800.0))
                        .build_ui(|ui| {
                            app.update_ime_state(ui.ctx());
                            app.input_root_keyboard(ui.ctx());
                            app.show_collection_manager(ui.ctx());
                        });
                    harness.run_steps(4);
                    harness.get_by_label("Close window").click();
                    harness.run_steps(2);
                }
                assert!(!app.collection_manager_open());
            } else {
                app.show_book_manager = false;
            }
            assert_adopted_and_back(&mut app, &ctx, &target, &origin);
        }
    }
}

fn fixture(kind: Option<usize>) -> AppTestEnvForTest {
    let mut app = kind
        .map(App::navigation_search_fixture_for_test)
        .unwrap_or_else(setup_app_for_test);
    let origin = app.tmp.path().join("pre-search");
    std::fs::create_dir_all(&origin).unwrap();
    if kind.is_none() {
        app.current_folder = Some(origin);
        app.top_level_grid_view
            .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    }
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app
}

fn configure_target(app: &mut AppTestEnvForTest, action: KeyAction) -> Target {
    if action.book_slot_number().is_some()
        || matches!(
            action,
            KeyAction::GridOpenActiveBook | KeyAction::GridBookPrev | KeyAction::GridBookNext
        )
    {
        let count = action.book_slot_number().unwrap_or(2).max(2);
        let root = app.tmp.path().join("books");
        for index in 1..=count {
            std::fs::create_dir_all(root.join(format!("Book{index:02}"))).unwrap();
        }
        app.settings.book_root = Some(root.clone());
        app.settings.active_book_name = "Book02".into();
        app.request_book_list_refresh();
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.book_op_pending.is_some() || app.book_shelf.snapshot.is_none() {
            assert!(Instant::now() < deadline, "book management list");
            app.poll_book_op_pending(&ctx);
            std::thread::yield_now();
        }
        let index = action.book_slot_number().unwrap_or_else(|| {
            if matches!(action, KeyAction::GridOpenActiveBook) {
                2
            } else if matches!(action, KeyAction::GridBookPrev) {
                count
            } else {
                1
            }
        });
        Target::Book(root.join(format!("Book{index:02}")))
    } else {
        let count = action.collection_slot_number().unwrap_or(2).max(2);
        let runtime = initialized_collection_runtime(app.tmp.path().join("collections.db"));
        let client = runtime.client();
        for index in 1..=count {
            client
                .create_collection(format!("Collection{index:02}"))
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
        app.install_collection_runtime(runtime);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(15);
        let ids = loop {
            assert!(Instant::now() < deadline, "collection management catalog");
            app.poll_collection_ui(&ctx);
            if let Ok((ids, _)) = app.collection_action_catalog() {
                if ids.len() == count {
                    break ids;
                }
            }
            std::thread::yield_now();
        };
        app.settings.toolbar_collection_target_id = Some(ids[count - 1].as_uuid());
        let index = action.collection_slot_number().unwrap_or_else(|| {
            if matches!(
                action,
                KeyAction::GridCollectionPrev | KeyAction::GridOpenCollectionTarget
            ) {
                count
            } else {
                1
            }
        });
        Target::Collection(ids[index - 1])
    }
}

fn initialized_collection_runtime(path: PathBuf) -> CollectionStoreRuntime {
    let runtime = CollectionStoreRuntime::start_at(path).unwrap();
    let client = runtime.client();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        // Observe the actor's actual admission boundary before fixture writes.
        // Keep its Ready event queued: the cold-catalog test must let App consume
        // that event itself after accepting its saved-group navigation request.
        match client.subscribe() {
            Ok(_) => return runtime,
            Err(crate::collection_store::CollectionStoreError::Starting) => {
                assert!(
                    Instant::now() < deadline,
                    "collection fixture runtime initialization"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("collection fixture runtime initialization failed: {error:?}"),
        }
    }
}

fn settle_target(app: &mut App, ctx: &egui::Context, target: &Target) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_book_op_pending(ctx);
        app.poll_collection_ui(ctx);
        app.poll_saved_group_open(ctx);
        dispatch(app, ctx, None);
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_sidecar_restore(ctx);
        if target_adopted(app, target)
            && app.saved_group_open_request_id_for_test().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "saved-group target not adopted: {target:?}; current={:?} search={} toast={:?}",
            app.current_folder,
            search_active(app),
            app.fs_feedback_toast.as_ref().map(|toast| &toast.0)
        );
        std::thread::yield_now();
    }
}

fn target_adopted(app: &App, target: &Target) -> bool {
    match target {
        Target::Book(path) => app.current_folder.as_ref() == Some(path),
        Target::Collection(id) => app
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| session.identity.collection_id == *id),
    }
}

fn assert_adopted_and_back(app: &mut App, ctx: &egui::Context, target: &Target, origin: &Path) {
    assert!(
        !search_active(app),
        "adopted explicit saved-group target must end search"
    );
    if let Target::Book(path) = target {
        assert_eq!(app.address, app.book_address_label_for_path(path).unwrap());
        assert!(matches!(
            app.top_level_grid_view.surface(),
            top_level_grid_view::TopLevelGridSurface::Folder
        ));
    }
    assert_eq!(
        app.folder_history_back_entry(),
        Some(&FolderNavHistoryTarget::Path(origin.to_path_buf()).into())
    );
    let nav = app
        .history_key_navigation(FolderHistoryDirection::Back)
        .expect("real Back accepts pre-search origin");
    dispatch(app, ctx, Some(nav));
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.current_folder.as_deref() != Some(origin)
        || app
            .top_level_grid_view
            .history_navigation_transition()
            .is_some()
        || app.sidecar_restore_active()
    {
        assert!(
            Instant::now() < deadline,
            "saved-group Back did not adopt original folder"
        );
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_sidecar_restore(ctx);
        std::thread::yield_now();
    }
    assert!(!search_active(app));
}

#[derive(Debug, PartialEq)]
struct SourceState {
    folder: Option<PathBuf>,
    address: String,
    rows: Vec<String>,
    selected: Option<usize>,
    checked: Vec<usize>,
    generation: u64,
    surface: String,
    restore: String,
    history: String,
    search: [bool; 3],
}

fn source_state(app: &App) -> SourceState {
    let mut checked = app.checked.iter().copied().collect::<Vec<_>>();
    checked.sort_unstable();
    SourceState {
        folder: app.current_folder.clone(),
        address: app.address.clone(),
        rows: app.items.iter().map(GridItem::display_path).collect(),
        selected: app.selected,
        checked,
        generation: app.items_generation,
        surface: format!("{:?}", app.top_level_grid_view.surface()),
        restore: format!("{:?}", app.top_level_grid_view.return_to()),
        history: format!("{:?}", app.folder_nav_history_snapshot()),
        search: [
            app.global_search.active,
            app.favsearch.active,
            app.tag_view.active,
        ],
    }
}

fn search_active(app: &App) -> bool {
    app.global_search.active || app.favsearch.active || app.tag_view.active
}

fn dispatch(app: &mut App, ctx: &egui::Context, keyboard_nav: Option<AddressBarNav>) {
    app.post_render_main_navigation(
        ctx,
        update_phases::MainFrameNavigation {
            fav_nav: None,
            toolbar_fav_nav: None,
            fullscreen_close_nav: None,
            keyboard_nav,
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
