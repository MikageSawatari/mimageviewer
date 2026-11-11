//! Real menubar clicks must use the same search-exit adoption as other jumps.
#![cfg(windows)]

use super::tests::phase_c_support::AppTestEnv;
use super::*;
use crate::keymap::{MenuCommandId, TopMenuId};
use crate::ui_main::LocationJumpTarget;
use egui_kittest::kittest::{NodeT, Queryable};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
enum Entrance {
    Favorite,
    BooksRoot,
}

#[test]
fn menubar_favorite_each_search_adopts_then_real_back_returns_before_search() {
    search_success(Entrance::Favorite);
}

#[test]
fn menubar_books_root_each_search_adopts_then_real_back_returns_before_search() {
    search_success(Entrance::BooksRoot);
}

fn search_success(entrance: Entrance) {
    for kind in 0..4 {
        let mut app = App::navigation_search_fixture_for_test(kind);
        let origin = app.tmp.path().join("pre-search");
        let target = app.tmp.path().join("menubar-destination");
        std::fs::create_dir_all(&origin).unwrap();
        if matches!(entrance, Entrance::Favorite) {
            std::fs::create_dir(&target).unwrap();
        }
        configure_destination(&mut app, entrance, &target);
        let source = source_state(&app);
        let app = Rc::new(RefCell::new(app));
        let candidate = click_menu(&app, entrance).expect("search menu emits navigation");
        assert_candidate(&candidate, entrance, &target);
        let mut app = app.borrow_mut();
        assert_eq!(
            source_state(&app),
            source,
            "rendering only produces a candidate"
        );
        assert_search_active(&app, kind);
        app.post_render_main_navigation(&egui::Context::default(), navigation(Some(candidate)));
        settle_menu_navigation(&mut app, Some(&target));
        assert!(
            !app.global_search.active && !app.favsearch.active && !app.tag_view.active,
            "kind={kind} entrance={entrance:?}: only successful adoption ends search"
        );
        assert_eq!(
            app.folder_history_back_entry(),
            Some(&FolderNavHistoryTarget::Path(origin.clone()).into())
        );
        // The real worker has already been polled and adopted. The shared bridge
        // now exercises the production Back handler, without replacing a scan.
        assert!(app.folder_pane_open_pending.is_none());
        app.verify_admitted_user_navigation_for_test(&target, &origin);
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
    }
}

#[test]
fn menubar_missing_favorite_each_search_keeps_results_selection_and_origin() {
    search_failure(Entrance::Favorite);
}

#[test]
fn menubar_failed_books_root_each_search_keeps_results_selection_and_origin() {
    search_failure(Entrance::BooksRoot);
}

fn search_failure(entrance: Entrance) {
    for kind in 0..4 {
        let mut app = App::navigation_search_fixture_for_test(kind);
        let target = app.tmp.path().join("unavailable-menubar-destination");
        if matches!(entrance, Entrance::BooksRoot) {
            // A real file prevents create_dir_all from preparing the shelf.
            std::fs::write(&target, b"not a directory").unwrap();
        }
        configure_destination(&mut app, entrance, &target);
        let source = source_state(&app);
        let app = Rc::new(RefCell::new(app));
        let candidate = click_menu(&app, entrance).expect("search menu emits navigation");
        assert_candidate(&candidate, entrance, &target);
        let mut app = app.borrow_mut();
        assert_eq!(source_state(&app), source);
        app.post_render_main_navigation(&egui::Context::default(), navigation(Some(candidate)));
        settle_menu_navigation(&mut app, None);
        assert_search_active(&app, kind);
        assert_eq!(
            source_state(&app),
            source,
            "kind={kind} entrance={entrance:?}: failure must retain the accepted source"
        );
        assert!(
            app.fs_feedback_toast.is_some(),
            "failed destination must notify"
        );
    }
}

#[test]
fn menubar_plain_books_root_opens_during_render_without_deferred_candidate() {
    let mut app = setup_app_for_test();
    let origin = app.tmp.path().join("plain-origin");
    let target = app.tmp.path().join("plain-books-root");
    std::fs::create_dir(&origin).unwrap();
    app.current_folder = Some(origin);
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    configure_destination(&mut app, Entrance::BooksRoot, &target);
    let app = Rc::new(RefCell::new(app));
    // click_menu never invokes a navigation dispatcher. Any change here must
    // therefore come from the released immediate open_books_root menu handler.
    assert!(click_menu(&app, Entrance::BooksRoot).is_none());
    let app = app.borrow();
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert!(target.is_dir());
    assert!(app.folder_pane_open_pending.is_none());
    assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
}

#[test]
fn menubar_plain_favorite_uses_existing_direct_navigation_and_history() {
    let mut app = setup_app_for_test();
    let origin = app.tmp.path().join("plain-origin");
    let target = app.tmp.path().join("plain-favorite");
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir(&target).unwrap();
    app.current_folder = Some(origin.clone());
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    configure_destination(&mut app, Entrance::Favorite, &target);
    let app = Rc::new(RefCell::new(app));
    let candidate = click_menu(&app, Entrance::Favorite).expect("favorite is a frame candidate");
    assert_candidate(&candidate, Entrance::Favorite, &target);
    let mut app = app.borrow_mut();
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    app.post_render_main_navigation(&egui::Context::default(), navigation(Some(candidate)));
    assert!(
        app.folder_pane_open_pending.is_none(),
        "plain favorite must keep Direct rather than search-exit pane scan"
    );
    settle_menu_navigation(&mut app, Some(&target));
    app.verify_admitted_user_navigation_for_test(&target, &origin);
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
}

fn configure_destination(app: &mut App, entrance: Entrance, target: &Path) {
    match entrance {
        Entrance::Favorite => {
            app.settings.favorites = vec![crate::settings::FavoriteEntry {
                id: uuid::Uuid::new_v4(),
                name: "search jump test favorite".into(),
                path: target.to_path_buf(),
                auto_index_structure: false,
                auto_index_metadata: false,
                auto_index_thumbs: false,
                auto_index_similar: false,
            }];
        }
        Entrance::BooksRoot => app.settings.book_root = Some(target.to_path_buf()),
    }
}

fn click_menu(app: &Rc<RefCell<AppTestEnv>>, entrance: Entrance) -> Option<LocationJumpTarget> {
    let command_label = match entrance {
        Entrance::Favorite => "search jump test favorite".to_owned(),
        Entrance::BooksRoot => app
            .borrow()
            .keymap
            .menu_command_label(MenuCommandId::BooksOpenRoot),
    };
    let top_menu = match entrance {
        Entrance::Favorite => TopMenuId::Favorites,
        Entrance::BooksRoot => TopMenuId::Books,
    };
    let app = Rc::clone(app);
    let candidates = Rc::new(RefCell::new(Vec::new()));
    let rendered_candidates = Rc::clone(&candidates);
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(1600.0, 700.0))
        .build_ui(move |ui| {
            let mut app = app.borrow_mut();
            app.note_main_chrome_pointer_block_transition(ui.ctx());
            if let (Some(candidate), _) = app.render_menubar(ui) {
                rendered_candidates.borrow_mut().push(candidate);
            }
        });
    harness.run();
    click(&mut harness, top_menu.label());
    click(&mut harness, &command_label);
    let mut candidates = candidates.borrow_mut();
    assert!(
        candidates.len() <= 1,
        "one menu click must emit at most one candidate"
    );
    candidates.pop()
}

fn click(harness: &mut egui_kittest::Harness<'_, ()>, label: &str) {
    let position = harness.get_by_label(label).rect().center();
    harness.hover_at(position);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
}

fn assert_candidate(candidate: &LocationJumpTarget, entrance: Entrance, target: &Path) {
    match (entrance, candidate) {
        (Entrance::Favorite, LocationJumpTarget::Physical(path)) => assert_eq!(path, target),
        (Entrance::BooksRoot, LocationJumpTarget::BooksRoot) => {}
        _ => panic!("unexpected {entrance:?} menu candidate: {candidate:?}"),
    }
}

fn settle_menu_navigation(app: &mut App, target: Option<&Path>) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.post_render_main_navigation(&ctx, navigation(None));
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(&ctx);
        app.poll_sidecar_restore(&ctx);
        let idle = app.folder_pane_open_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active();
        if idle && target.is_none_or(|target| app.current_folder.as_deref() == Some(target)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "menu navigation did not settle"
        );
        std::thread::yield_now();
    }
}

fn assert_search_active(app: &App, kind: usize) {
    assert!(match kind {
        0 => app.global_search.active,
        1 => {
            app.global_search.active
                && matches!(
                    app.global_search.accepted,
                    crate::global_search_ui::SearchAccepted::Everything { .. }
                )
        }
        2 => app.favsearch.active,
        _ => app.tag_view.active,
    });
}

fn source_state(app: &App) -> String {
    format!(
        "{:?}",
        (
            (
                &app.current_folder,
                &app.address,
                app.selected,
                app.items_generation,
            ),
            (
                &app.checked,
                app.items
                    .iter()
                    .map(|item| item.drag_source_path().map(Path::to_path_buf))
                    .collect::<Vec<_>>(),
                app.folder_nav_history_snapshot(),
            ),
            (
                app.top_level_grid_view.surface(),
                app.top_level_grid_view.return_to(),
            ),
            (
                &app.global_search.metadata_draft,
                &app.global_search.everything_draft,
                &app.favsearch.query,
                &app.tag_view.query,
            ),
            (
                &app.global_search.saved_folder,
                &app.favsearch.saved_folder,
                &app.tag_view.saved_folder,
            ),
        )
    )
}

fn navigation(fav_nav: Option<LocationJumpTarget>) -> update_phases::MainFrameNavigation {
    update_phases::MainFrameNavigation {
        fav_nav,
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
