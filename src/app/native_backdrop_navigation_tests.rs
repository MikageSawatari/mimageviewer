//! Native MAIN's early return must consume actual gamepad navigation values.
//! Video items are metadata only: these tests never create a player or a HWND.
#![cfg(windows)]

use super::tests::phase_c_support::AppTestEnv;
use super::*;
use crate::ring_shortcut::{
    GamepadFavoritePickerState, GamepadFavoritePickerTab, GamepadListCursor, GamepadLocationEntry,
    GamepadLocationNav, GamepadLocationPickerState,
};
use crate::ui_main::{AddressBarNav, LocationJumpTarget};

#[test]
fn native_backdrop_gamepad_favorite_each_search_adopts_and_returns_before_search() {
    let ctx = egui::Context::default();
    for kind in 0..4 {
        let mut app = search_backdrop(kind);
        let origin = app.tmp.path().join("pre-search");
        let target = app.tmp.path().join("native-favorite");
        std::fs::create_dir(&target).unwrap();
        let nav = confirm_favorite(&mut app, &ctx, &target);
        assert!(matches!(&nav, AddressBarNav::Favorite(path) if path == &target));
        assert_terminal_close_is_pending_on_backdrop(&app);
        app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
        assert_search_active(&app, kind);
        assert!(
            app.folder_pane_open_pending.is_some(),
            "Favorite must reach its scan"
        );
        settle(&mut app, Some(&target));
        assert_search_ended(&app);
        app.verify_admitted_user_navigation_for_test(&target, &origin);
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
        assert_close_completed(&app);
    }
}

#[test]
fn native_backdrop_gamepad_missing_favorite_each_search_preserves_source() {
    let ctx = egui::Context::default();
    for kind in 0..4 {
        let mut app = search_backdrop(kind);
        let target = app.tmp.path().join("missing-native-favorite");
        let nav = confirm_favorite(&mut app, &ctx, &target);
        assert_terminal_close_is_pending_on_backdrop(&app);
        let source = search_source(&app);
        app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
        assert!(
            app.folder_pane_open_pending.is_some(),
            "missing Favorite must reach its scan"
        );
        settle(&mut app, None);
        assert_search_active(&app, kind);
        assert_eq!(search_source(&app), source, "kind={kind}");
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn native_backdrop_gamepad_location_drive_list_and_physical_exit_each_search() {
    let ctx = egui::Context::default();
    for kind in 0..4 {
        for physical in [false, true] {
            let mut app = search_backdrop(kind);
            let origin = app.tmp.path().join("pre-search");
            let target = if physical {
                let target = app.tmp.path().join("native-location");
                std::fs::create_dir(&target).unwrap();
                target
            } else {
                drive_list_synthetic_path()
            };
            let nav = confirm_location(
                &mut app,
                &ctx,
                if physical {
                    GamepadLocationNav::Direct(target.clone())
                } else {
                    GamepadLocationNav::DriveList
                },
            );
            if physical {
                assert!(
                    matches!(&nav, AddressBarNav::LocationJump(LocationJumpTarget::Physical(path)) if path == &target)
                );
            } else {
                assert!(matches!(
                    nav,
                    AddressBarNav::LocationJump(LocationJumpTarget::DriveList)
                ));
            }
            assert_terminal_close_is_pending_on_backdrop(&app);
            app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
            if physical {
                assert_search_active(&app, kind);
                assert!(
                    app.folder_pane_open_pending.is_some(),
                    "Physical must reach its scan"
                );
            } else {
                assert!(
                    app.items_are_drive_list,
                    "DriveList must install at the early tail"
                );
            }
            settle(&mut app, Some(&target));
            assert_search_ended(&app);
            assert_eq!(app.items_are_drive_list, !physical);
            app.verify_admitted_user_navigation_for_test(&target, &origin);
            assert_eq!(app.current_folder.as_ref(), Some(&origin));
            assert_close_completed(&app);
        }
    }
}

#[test]
fn native_backdrop_gamepad_vanished_location_each_search_preserves_source() {
    let ctx = egui::Context::default();
    for kind in 0..4 {
        let mut app = search_backdrop(kind);
        let target = app.tmp.path().join("vanished-native-location");
        std::fs::create_dir(&target).unwrap();
        let nav = confirm_location(&mut app, &ctx, GamepadLocationNav::Direct(target.clone()));
        assert!(
            matches!(&nav, AddressBarNav::LocationJump(LocationJumpTarget::Physical(path)) if path == &target)
        );
        assert_terminal_close_is_pending_on_backdrop(&app);
        // The picker resolved an existing directory. It vanishes before the
        // winning candidate is dispatched, so the real worker must fail.
        std::fs::remove_dir(&target).unwrap();
        let source = search_source(&app);
        app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
        assert!(
            app.folder_pane_open_pending.is_some(),
            "vanished Physical must reach its scan"
        );
        settle(&mut app, None);
        assert_search_active(&app, kind);
        assert_eq!(search_source(&app), source, "kind={kind}");
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn native_backdrop_plain_favorite_and_direct_keep_immediate_loader_cleanup() {
    let ctx = egui::Context::default();
    for favorite in [false, true] {
        let mut app = plain_backdrop();
        let origin = app.current_folder.clone().unwrap();
        let target = app.tmp.path().join("plain-native-destination");
        std::fs::create_dir(&target).unwrap();
        let nav = if favorite {
            confirm_favorite(&mut app, &ctx, &target)
        } else {
            app.close_fullscreen();
            AddressBarNav::Direct(target.clone(), StartupListIntent::ExplicitList)
        };
        assert_terminal_close_is_pending_on_backdrop(&app);
        app.pending_folder_nav_steps = 3;
        app.pending_folder_nav_mode = FolderNavMode::SiblingGrid;
        app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(matches!(app.pending_folder_nav_mode, FolderNavMode::Grid));
        assert_eq!(app.current_folder.as_ref(), Some(&target));
        assert!(app.folder_pane_open_pending.is_none());
        settle(&mut app, Some(&target));
        assert_search_ended(&app);
        assert_eq!(
            app.folder_history_back_entry(),
            Some(&FolderNavHistoryTarget::Path(origin).into())
        );
    }
}

#[test]
fn native_backdrop_history_back_and_forward_are_consumed() {
    let ctx = egui::Context::default();
    let mut app = plain_backdrop();
    let origin = app.current_folder.clone().unwrap();
    let target = app.tmp.path().join("native-history-destination");
    std::fs::create_dir(&target).unwrap();
    let nav = confirm_favorite(&mut app, &ctx, &target);
    app.dispatch_native_backdrop_navigation(&ctx, None, false, Some(nav));
    settle(&mut app, Some(&target));
    for (nav, expected) in [
        (AddressBarNav::HistoryBack, &origin),
        (AddressBarNav::HistoryForward, &target),
    ] {
        // No video/player is opened: only metadata is installed to make the
        // exact early-return predicate true for each history command.
        install_video_backdrop(&mut app, false);
        assert!(app.native_video_fullscreen_active_for_main_backdrop());
        app.dispatch_native_backdrop_navigation(&ctx, Some(nav), false, None);
        settle(&mut app, Some(expected));
        assert_eq!(app.current_folder.as_ref(), Some(expected));
    }
    assert_eq!(
        app.folder_history_back_entry(),
        Some(&FolderNavHistoryTarget::Path(origin).into())
    );
}

fn search_backdrop(kind: usize) -> AppTestEnv {
    let mut app = App::navigation_search_fixture_for_test(kind);
    let origin = app.tmp.path().join("pre-search");
    std::fs::create_dir_all(&origin).unwrap();
    let video = app.tmp.path().join("search-result.mp4");
    if kind == 1 {
        // Keep the accepted Everything dataset and its displayed Video row in
        // agreement; no transport, player, decoder or native presenter is used.
        crate::global_search_ui::adopt_everything_for_test(
            &mut app,
            &egui::Context::default(),
            "native-backdrop-video",
            crate::global_search_ui::everything_dataset_for_test(vec![GridItem::Video(video)]),
        );
    } else {
        app.items[0] = GridItem::Video(video);
    }
    app.selected = Some(0);
    app.visible_indices = vec![0];
    install_video_backdrop(&mut app, true);
    assert_search_active(&app, kind);
    app
}

fn plain_backdrop() -> AppTestEnv {
    let mut app = setup_app_for_test();
    let origin = app.tmp.path().join("plain-origin");
    std::fs::create_dir(&origin).unwrap();
    app.current_folder = Some(origin);
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    install_video_backdrop(&mut app, true);
    app
}

fn install_video_backdrop(app: &mut App, transitioning: bool) {
    if !matches!(app.items.first(), Some(GridItem::Video(_))) {
        let path = app
            .current_folder
            .as_ref()
            .unwrap()
            .join("metadata-only.mp4");
        app.install_new_items(vec![GridItem::Video(path)], vec![None]);
        app.visible_indices = vec![0];
        app.selected = Some(0);
    }
    assert!(
        app.fs_cache.is_empty(),
        "metadata fixture has no native player"
    );
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.native_video_in_window_active = false;
    app.video_presentation_transition
        .reset_stable(ViewerPresentation::Fullscreen);
    if transitioning {
        app.video_presentation_transition.request_transition(
            ViewerPresentation::MainWindow,
            true,
            false,
            0,
            None,
            None,
        );
        app.video_presentation_transition.drive();
        assert!(app.video_presentation_transition.is_transitioning());
    }
    assert!(app.native_video_fullscreen_active_for_main_backdrop());
}

fn confirm_favorite(app: &mut App, ctx: &egui::Context, target: &Path) -> AddressBarNav {
    app.settings.favorites = vec![crate::settings::FavoriteEntry {
        id: uuid::Uuid::new_v4(),
        name: "native test favorite".into(),
        path: target.to_path_buf(),
        auto_index_structure: false,
        auto_index_metadata: false,
        auto_index_thumbs: false,
        auto_index_similar: false,
    }];
    app.gamepad_favorite_picker = Some(GamepadFavoritePickerState {
        tab: GamepadFavoritePickerTab::Favorites,
        favorites: GamepadListCursor::new(0),
        smart_folders: GamepadListCursor::new(0),
    });
    let nav = app
        .confirm_gamepad_favorite_picker(ctx)
        .expect("actual favorite confirmation emits navigation");
    assert!(app.gamepad_favorite_picker.is_none());
    nav
}

fn confirm_location(
    app: &mut App,
    ctx: &egui::Context,
    target: GamepadLocationNav,
) -> AddressBarNav {
    app.gamepad_location_picker = Some(GamepadLocationPickerState {
        selected: 0,
        scroll_top: 0,
        entries: vec![GamepadLocationEntry {
            label: "native test location".into(),
            value: String::new(),
            nav: target,
        }],
    });
    let nav = app
        .confirm_gamepad_location_picker(ctx)
        .expect("actual location confirmation emits navigation");
    assert!(app.gamepad_location_picker.is_none());
    nav
}

fn assert_terminal_close_is_pending_on_backdrop(app: &App) {
    assert!(
        app.fullscreen_idx.is_some(),
        "close has not executed content teardown"
    );
    assert!(!app.video_presentation_transition.is_transitioning());
    assert!(app.video_presentation_transition.has_pending_effects());
    assert!(app.native_video_fullscreen_active_for_main_backdrop());
    assert!(app.fs_cache.is_empty());
}

fn settle(app: &mut App, target: Option<&Path>) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // App::update polls presentation transitions before the main native
        // branch or the ordinary navigation tail. Execute the queued terminal
        // close through that same owner before polling the requested scan.
        app.poll_video_presentation_transition(&ctx);
        app.post_render_main_navigation(&ctx, empty_navigation());
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(&ctx);
        app.poll_sidecar_restore(&ctx);
        if app.folder_pane_open_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
            && !app.video_presentation_transition.has_pending_effects()
            && target.is_none_or(|target| app.current_folder.as_deref() == Some(target))
        {
            assert_close_completed(app);
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native navigation did not adopt"
        );
        std::thread::yield_now();
    }
}

fn assert_close_completed(app: &App) {
    assert!(
        app.fullscreen_idx.is_none(),
        "terminal close must finish before adoption"
    );
    assert!(
        !app.video_presentation_transition.has_pending_effects(),
        "adoption and history must not leave terminal-close effects queued"
    );
}

fn assert_search_active(app: &App, kind: usize) {
    assert!(match kind {
        0 | 1 => app.global_search.active,
        2 => app.favsearch.active,
        _ => app.tag_view.active,
    });
}

fn assert_search_ended(app: &App) {
    assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
}

fn search_source(app: &App) -> String {
    format!(
        "{:?}",
        (
            (
                &app.current_folder,
                &app.address,
                app.selected,
                app.items_generation
            ),
            (
                app.items
                    .iter()
                    .map(GridItem::drag_source_path)
                    .collect::<Vec<_>>(),
                &app.checked,
                app.folder_nav_history_snapshot(),
            ),
            (
                app.top_level_grid_view.surface(),
                app.top_level_grid_view.return_to()
            ),
            (
                &app.global_search.saved_folder,
                &app.favsearch.saved_folder,
                &app.tag_view.saved_folder,
            ),
            (
                &app.global_search.metadata_draft,
                &app.global_search.everything_draft,
                &app.favsearch.query,
                &app.tag_view.query,
            ),
        )
    )
}

fn empty_navigation() -> update_phases::MainFrameNavigation {
    update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}
