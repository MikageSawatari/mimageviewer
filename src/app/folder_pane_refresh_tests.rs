use super::section375_tests::{fixture, settle};
use super::*;
use crate::folder_pane::FolderPaneState;
use std::time::SystemTime;

#[test]
fn folder_pane_reported_delete_is_reflected_after_main_list_acceptance_without_tree_input() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    let aaa = folder.join("aaa");
    std::fs::create_dir(&aaa).unwrap();
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    app.folder_pane = FolderPaneState::test_tree(&folder, vec![aaa.clone()]);
    app.folder_pane.test_seed_listing(&aaa, vec![], true);
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item,GridItem::Folder(path) if path==&aaa))
    );
    std::fs::remove_dir(&aaa).unwrap();
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    settle(&mut app, &ctx);
    assert!(
        !app.items
            .iter()
            .any(|item| matches!(item,GridItem::Folder(path) if path==&aaa))
    );
    assert!(app.folder_pane.test_unverified(&folder));
    // Only the ordinary completion pump runs; no tree click, reopen or reload.
    app.folder_pane.test_settle();
    assert!(app.folder_pane.test_children(&folder).is_empty());
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == aaa)
    );
}

#[test]
fn folder_pane_only_accepted_equal_and_different_main_observations_publish_hints() {
    for route in ["equal", "different", "defer", "failure", "stale"] {
        let (mut app, folder, ctx) = fixture(&["a.jpg"]);
        app.folder_pane = FolderPaneState::test_tree(&folder, vec![]);
        let generation = app.items_generation;
        if matches!(route, "different" | "defer") {
            std::fs::write(folder.join("b.jpg"), b"new").unwrap();
        }
        if route == "defer" {
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = ViewerPresentation::Fullscreen;
        }
        app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
        let pending = app
            .external_rescan_pending
            .as_mut()
            .expect("real rescan request");
        if route == "stale" {
            pending.items_generation = pending.items_generation.wrapping_sub(1);
        }
        if route == "failure" {
            let (tx, rx) = mpsc::channel();
            tx.send(None).unwrap();
            pending.rx = rx;
        }
        settle(&mut app, &ctx);
        assert_eq!(
            app.folder_pane.test_unverified(&folder),
            matches!(route, "equal" | "different"),
            "{route}"
        );
        if route == "equal" {
            assert_eq!(app.items_generation, generation);
        }
        if route == "defer" {
            assert!(app.folder_refresh_pending.is_some());
            assert_eq!(app.items_generation, generation);
        }
    }
}

#[cfg(windows)]
#[test]
fn folder_pane_parked_main_acceptance_only_invalidates_main_pane_not_independent_context() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    app.folder_pane = FolderPaneState::test_tree(&folder, vec![]);
    let main = app.viewer_context_main();
    let independent = app.fork_materialized_still_context(&folder, 0);
    app.with_viewer_context(independent, |app| {
        let generation = app.items_generation;
        let items = app.items.as_ptr();
        let cancel = app.cancel_token.clone();
        // Non-main active operations may reconcile, but must never notify the main pane.
        app.start_external_rescan_with_cause(
            &ctx,
            folder.clone(),
            SystemTime::UNIX_EPOCH,
            book_append_refresh::FolderRefreshCause::Operation,
        );
        settle(app, &ctx);
        assert!(!app.folder_pane.test_unverified(&folder));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.items.as_ptr(), items);
        app.with_viewer_context(main, |app| {
            app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
            settle(app, &ctx);
            assert!(app.folder_pane.test_unverified(&folder));
        })
        .unwrap();
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.items.as_ptr(), items);
        assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.current_folder_watch.is_none());
    })
    .unwrap();
}

#[test]
fn folder_pane_main_return_rechecks_other_branch_even_when_list_a_is_equal() {
    for interruption in ["chrome_not_drawn", "tray", "minimized"] {
        let (mut app, a, ctx) = fixture(&["a.jpg"]);
        let root = a.parent().unwrap().to_path_buf();
        let b = root.join("B");
        std::fs::create_dir(&b).unwrap();
        app.settings.folder_tree_pane_visible = true;
        app.folder_pane = FolderPaneState::test_tree(&root, vec![a.clone(), b.clone()]);
        app.folder_pane.test_seed_listing(&b, vec![], true);
        match interruption {
            "chrome_not_drawn" => app.finish_folder_pane_presentation(&ctx, false),
            "tray" => {
                app.window_visible = false;
                app.service_folder_pane_visibility(&ctx);
            }
            _ => {
                let mut raw = egui::RawInput::default();
                raw.viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .minimized = Some(true);
                ctx.run_ui(raw, |_| app.service_folder_pane_visibility(&ctx))
                    .drop_without_applying_deltas();
            }
        }
        std::fs::create_dir(b.join("new")).unwrap();
        let generation = app.items_generation;
        app.start_external_rescan(&ctx, a.clone(), SystemTime::UNIX_EPOCH);
        settle(&mut app, &ctx);
        assert_eq!(app.items_generation, generation, "A stayed Equal");
        assert!(app.folder_pane.test_children(&b).is_empty());
        app.window_visible = true;
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                app.render_folder_pane(ui);
            },
        )
        .drop_without_applying_deltas();
        app.folder_pane.test_settle();
        assert_eq!(
            app.folder_pane.test_children(&b),
            vec![b.join("new")],
            "{interruption}"
        );
        assert_eq!(app.items_generation, generation);
    }
}
