use super::*;
use crate::delete_worker::{DeleteMsg, DeletePending};
use std::time::{Duration, Instant, SystemTime};

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll_open_path_classification(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_external_rescan(ctx);
        app.poll_sidecar_restore(ctx);
        app.poll_stack_script(ctx);
        if app.external_rescan_pending.is_none()
            && !app.sidecar_restore_active()
            && app.stack_script_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
        {
            break;
        }
        assert!(Instant::now() < deadline, "reconciliation did not finish");
        std::thread::yield_now();
    }
}

fn fixture(names: &[&str]) -> (AppTestEnvForTest, PathBuf, egui::Context) {
    let mut app = setup_app_for_test();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    let folder = app.tmp.path().join("section1375");
    std::fs::create_dir(&folder).unwrap();
    for name in names {
        std::fs::write(folder.join(name), b"image-source").unwrap();
    }
    app.load_folder(folder.clone());
    let ctx = egui::Context::default();
    settle(&mut app, &ctx);
    assert!(
        app.folder_observation
            .as_ref()
            .is_some_and(|record| record.rows.known)
    );
    (app, folder, ctx)
}

fn delete_terminal(app: &mut App, paths: Vec<PathBuf>) {
    let (tx, rx) = mpsc::channel();
    let source = app.capture_stack_deletion();
    tx.send(DeleteMsg::Batch {
        succeeded: paths.clone(),
        failed: vec![],
        purged_pdf_password_paths: vec![],
        purge_deferred: false,
    })
    .unwrap();
    let succeeded = paths.clone();
    let completed = std::thread::spawn(move || {
        crate::delete_worker::completed_delete(false, Default::default(), source, &succeeded)
    })
    .join()
    .unwrap();
    tx.send(completed).unwrap();
    app.delete_pending = Some(DeletePending {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        total: paths.len(),
        succeeded: vec![],
        failed: vec![],
        purged_pdf_password_paths: vec![],
        purge_deferred: false,
        source_scopes: paths
            .into_iter()
            .map(crate::delete_worker::DeleteSourceScope::Exact)
            .collect(),
    });
    app.poll_delete_pending();
}

#[test]
fn section1375_worker_equal_preserves_cursor_checks_scroll_and_late_thumbnail() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg", "c.jpg", "d.jpg"]);
    // Isolate the in-flight request from fixture decoding workers, then deliver its
    // actual correlated result only after the production Equal branch has run.
    let (tx, rx) = mpsc::channel();
    app.tx = tx;
    app.rx = rx;
    app.selected = Some(2);
    app.checked = [1, 3].into_iter().collect();
    app.scroll_offset_y = 237.0;
    app.thumbnails[2] = ThumbnailState::Pending;
    app.keep_set.insert(2);
    app.keep_range = (0, 4);
    app.requested.insert(2, false);
    let generation = app.items_generation;
    let previous_rows = app.folder_observation.as_ref().unwrap().rows.clone();
    app.start_external_rescan(&ctx, folder, SystemTime::UNIX_EPOCH);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert!(
        !Arc::ptr_eq(
            &previous_rows,
            &app.folder_observation.as_ref().unwrap().rows
        ),
        "production Equal must adopt the new scan record, not reject the result"
    );
    assert_eq!(app.selected, Some(2));
    assert_eq!(app.checked, [1, 3].into_iter().collect());
    assert_eq!(app.scroll_offset_y, 237.0);
    assert!(
        app.requested.contains_key(&2),
        "Equal must retain the request owner"
    );
    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            2,
            Some(egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255; 16])),
            crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            false,
            None,
            Some((2, 2)),
            None,
            false,
            false,
            0,
            generation,
        )
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[2], ThumbnailState::Loaded { .. }));
    assert!(!app.requested.contains_key(&2));
    assert_eq!(app.selected, Some(2));
    assert_eq!(app.scroll_offset_y, 237.0);
}

#[test]
fn section1375_actual_delete_no_notify_equal_preserves_rows_and_rating_zeros() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg", "c.jpg", "d.jpg"]);
    app.rating_cache = [(0, 0), (1, 5), (2, 4), (3, 0)].into_iter().collect();
    for index in 0..app.items.len() {
        let _ = app.get_rotation(index);
    }
    let before = app.items_generation;
    let row_record = app.folder_observation.as_ref().unwrap().rows.clone();
    let tags_before = RATING_INSTALL_UI_TAG_PREWARMS.with(std::cell::Cell::get);
    std::fs::remove_file(folder.join("b.jpg")).unwrap();
    delete_terminal(&mut app, vec![folder.join("b.jpg")]);
    assert!(
        app.external_rescan_pending.is_some(),
        "terminal schedules a scan without notify"
    );
    assert!(Arc::ptr_eq(
        &row_record,
        &app.folder_observation.as_ref().unwrap().rows
    ));
    assert_eq!(
        app.folder_observation
            .as_ref()
            .unwrap()
            .removed
            .paths()
            .count(),
        1
    );
    assert_eq!(app.items_generation, before.wrapping_add(1));
    settle(&mut app, &ctx);
    assert_eq!(
        app.items_generation,
        before.wrapping_add(1),
        "Equal must not install"
    );
    assert_eq!(
        app.rating_cache,
        [(0, 0), (1, 4), (2, 0)].into_iter().collect()
    );
    assert_eq!(
        RATING_INSTALL_UI_TAG_PREWARMS.with(std::cell::Cell::get),
        tags_before
    );
    // Footer/address rating and global count invalidation are legitimate on both normal
    // full reload and Delete. Warm those same one-shot consumers before draw/scroll timing.
    let _ = app.current_folder_rating();
    let _ = app.rating_counts();
    let reads = crate::rating_db::point_read_operations_for_test();
    let all_reads = crate::rating_db::read_operations_for_test();
    let tag_reads = crate::tags_db::read_operations_for_test();
    let rotation_reads = crate::rotation_db::read_operations_for_test();
    assert_eq!(app.get_rating(0), 0);
    assert_eq!(app.get_rating(1), 4);
    assert_eq!(app.get_rating(2), 0);
    app.rebuild_visible_indices();
    for offset in [0.0, 100.0, 400.0] {
        app.scroll_offset_y = offset;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                let _ = app.render_grid(ctx);
            },
        );
    }
    assert_eq!(crate::rating_db::read_operations_for_test(), all_reads);
    assert_eq!(crate::tags_db::read_operations_for_test(), tag_reads);
    assert_eq!(
        crate::rotation_db::read_operations_for_test(),
        rotation_reads
    );
    assert_eq!(
        crate::rating_db::point_read_operations_for_test(),
        reads,
        "survivor drawing/scroll must not add rating SELECTs"
    );
    let stable = app.items_generation;
    for _ in 0..3 {
        app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        settle(&mut app, &ctx);
    }
    assert_eq!(app.items_generation, stable);
}

#[test]
fn section1375_delete_hidden_candidate_and_no_visible_idx_still_reconcile() {
    for (names, removed, revealed) in [
        (vec!["a.jpg", "a.png", "b.jpg"], "a.jpg", "a.png"),
        (
            vec!["book.pdf", "book.epub", "b.jpg"],
            "book.pdf",
            "book.epub",
        ),
    ] {
        let (mut app, folder, ctx) = fixture(&names);
        app.settings.image_ext_priority = vec!["jpg".into(), "png".into()];
        app.settings.skip_epub_if_pdf_exists = true;
        app.load_folder(folder.clone());
        settle(&mut app, &ctx);
        assert!(
            !app.items
                .iter()
                .any(|item| item.drag_source_path() == Some(folder.join(revealed).as_path()))
        );
        let generation = app.items_generation;
        std::fs::remove_file(folder.join(removed)).unwrap();
        delete_terminal(&mut app, vec![folder.join(removed)]);
        let removed_generation = app.items_generation;
        settle(&mut app, &ctx);
        assert!(
            app.items_generation > removed_generation,
            "new winner requires a normal install"
        );
        assert!(app.items_generation > generation);
        assert!(
            app.items
                .iter()
                .any(|item| item.drag_source_path() == Some(folder.join(revealed).as_path()))
        );
    }
    let (mut app, folder, ctx) = fixture(&["a.jpg", "a.png", "b.jpg"]);
    app.settings.skip_duplicate_images = true;
    app.settings.image_ext_priority = vec!["jpg".into(), "png".into()];
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let hidden = folder.join("a.png");
    assert!(
        !app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(hidden.as_path()))
    );
    let generation = app.items_generation;
    std::fs::remove_file(&hidden).unwrap();
    delete_terminal(&mut app, vec![hidden]);
    assert!(
        app.external_rescan_pending.is_some(),
        "no visible success idx still scans"
    );
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
}

#[test]
fn section1375_physical_folder_zip_pdf_epub_video_audio_delete_equal() {
    for name in [
        "a.jpg", "a.zip", "a.pdf", "a.epub", "a.mp4", "a.mp3", "child",
    ] {
        let (mut app, folder, ctx) = fixture(&["keep.jpg"]);
        let removed = folder.join(name);
        if name == "child" {
            std::fs::create_dir(&removed).unwrap();
        } else {
            std::fs::write(&removed, b"source").unwrap();
        }
        app.load_folder(folder.clone());
        settle(&mut app, &ctx);
        let generation = app.items_generation;
        if name == "child" {
            std::fs::remove_dir(&removed).unwrap();
        } else {
            std::fs::remove_file(&removed).unwrap();
        }
        delete_terminal(&mut app, vec![removed]);
        let terminal_generation = app.items_generation;
        assert!(terminal_generation > generation);
        settle(&mut app, &ctx);
        assert_eq!(app.items_generation, terminal_generation, "{name}");
    }
}

#[test]
fn section1375_sidecar_backup_flush_and_hard_purge_do_not_install() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    app.settings.sidecar_backup_enabled = true;
    let mut sidecar = crate::sidecar::SidecarFile::load(&folder);
    sidecar.set_adjust(
        "a.jpg",
        crate::adjustment::AdjustParams {
            brightness: 12.0,
            ..Default::default()
        },
    );
    assert!(sidecar.flush_blocking());
    let generation = app.items_generation;
    let imports = sidecar_restore::import_starts_for_test();
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert_eq!(sidecar_restore::import_starts_for_test(), imports);
    std::fs::remove_file(folder.join("a.jpg")).unwrap();
    let report = crate::rename_key_migration::purge_removed_paths_at(
        &crate::data_dir::get(),
        &[folder.join("a.jpg")],
        &[],
    );
    assert!(report.errors.is_empty(), "{report:?}");
    delete_terminal(&mut app, vec![folder.join("a.jpg")]);
    let generation = app.items_generation;
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert_eq!(sidecar_restore::import_starts_for_test(), imports);
}

#[test]
fn section1375_automatic_origin_survives_match_diff_restart_resume_and_defer() {
    let _input = crate::key_input::lock_test_input();
    for route in ["notify", "restart", "resumed", "deferred"] {
        for diff in [false, true] {
            let (mut app, folder, ctx) = fixture(&["a.jpg"]);
            app.settings.sidecar_backup_enabled = true;
            let mut sidecar = crate::sidecar::SidecarFile::load(&folder);
            sidecar.set_adjust(
                "a.jpg",
                crate::adjustment::AdjustParams {
                    brightness: 12.0,
                    ..Default::default()
                },
            );
            assert!(sidecar.flush_blocking());
            if diff {
                std::fs::write(folder.join("b.jpg"), b"new").unwrap();
            }
            let imports = sidecar_restore::import_starts_for_test();
            let generation = app.items_generation;
            if route == "deferred" {
                app.fullscreen_idx = Some(0);
                app.viewer_presentation = ViewerPresentation::Fullscreen;
            }
            if route == "resumed" {
                app.current_folder_last_mtime = Some(SystemTime::UNIX_EPOCH);
            }
            app.check_external_folder_changes(
                &ctx,
                if route == "resumed" {
                    ExternalChangeCheck::Resumed
                } else {
                    ExternalChangeCheck::Notified
                },
            );
            if route == "restart" {
                app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
            }
            settle(&mut app, &ctx);
            assert_eq!(
                sidecar_restore::import_starts_for_test(),
                imports,
                "{route} diff={diff}"
            );
            if route == "deferred" {
                assert_eq!(app.items_generation, generation);
                app.fullscreen_idx = None;
                app.viewer_presentation = ViewerPresentation::MainWindow;
                app.consume_folder_refresh_pending(&ctx);
                settle(&mut app, &ctx);
                assert_eq!(
                    sidecar_restore::import_starts_for_test(),
                    imports,
                    "return must keep automatic origin"
                );
            }
            let key = crate::adjustment_db::normalize_path(&folder.join("a.jpg"));
            assert!(!app.adjusted_page_keys.contains(&key));
            app.keymap = crate::keymap::Keymap::from_ini_str(
                "[Grid]\nGridReload = Ctrl+F5\n[Rating]\nRatingItem5 = none\n",
            );
            let modifiers = egui::Modifiers {
                ctrl: true,
                ..Default::default()
            };
            ctx.begin_pass(egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::F5,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            });
            let _ = app.handle_keyboard(&ctx);
            let _ = ctx.end_pass();
            settle(&mut app, &ctx);
            assert!(
                sidecar_restore::import_starts_for_test() > imports,
                "explicit reload must retain configured imports"
            );
            let after_explicit = sidecar_restore::import_starts_for_test();
            app.load_folder(folder.clone());
            settle(&mut app, &ctx);
            assert!(
                sidecar_restore::import_starts_for_test() > after_explicit,
                "normal open must retain configured imports"
            );
        }
    }
}

#[test]
fn section1375_detached_physical_reader_defers_changed_rows_until_list_return() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(!app.viewer_session_blocks_main_window());
    let generation = app.items_generation;
    let rows = app.folder_observation.as_ref().unwrap().rows.clone();
    let selected_path = app.items[0].drag_source_path().unwrap().to_path_buf();
    let added = folder.join("b.jpg");
    std::fs::write(&added, b"external-change").unwrap();
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.items[0].drag_source_path(),
        Some(selected_path.as_path())
    );
    assert!(Arc::ptr_eq(
        &rows,
        &app.folder_observation.as_ref().unwrap().rows
    ));
    let pending = app.folder_refresh_pending.as_ref().unwrap();
    assert_eq!(pending.folder, folder);
    assert!(matches!(
        pending.cause,
        book_append_refresh::FolderRefreshCause::External
    ));
    app.fullscreen_idx = None;
    app.consume_folder_refresh_pending(&ctx);
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(added.as_path()))
    );
    assert!(app.items_generation > generation);
}

#[test]
fn section1375_main_list_still_refreshes_with_detached_media_presentation() {
    let (mut app, folder, ctx) = fixture(&["a.mp4", "b.jpg"]);
    app.navigation_scope = ViewerNavigationScope::Main;
    app.fullscreen_idx = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Video(_)));
    assert!(app.fullscreen_idx.is_some());
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(!app.viewer_session_blocks_main_window());
    let generation = app.items_generation;
    let added = folder.join("c.jpg");
    std::fs::write(&added, b"external-change").unwrap();
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert!(app.folder_refresh_pending.is_none());
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(added.as_path()))
    );
    assert!(app.items_generation > generation);
}

#[test]
fn section1375_stack_self_delete_equal_opens_remaining_stack_after_image_or_folder() {
    for folder_first in [false, true] {
        let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg"]);
        let removed = if folder_first {
            // The default rule deliberately does not collapse a single media group.
            // Keep the singleton so this is a real Folder + Image + Stack load.
            let removed = folder.join("a-folder");
            std::fs::create_dir(&removed).unwrap();
            removed
        } else {
            folder.join("a_1.jpg")
        };
        app.stack_mode_requested = true;
        app.load_folder(folder.clone());
        settle(&mut app, &ctx);
        let survivor_count = if folder_first { 2 } else { 1 };
        assert_eq!(app.items.len(), survivor_count + 1);
        assert_eq!(app.items[0].drag_source_path(), Some(removed.as_path()));
        let sibling_view = app.stack_view.as_ref().unwrap().clone();
        let previous_rows = app.folder_observation.as_ref().unwrap().rows.clone();
        if folder_first {
            std::fs::remove_dir(&removed).unwrap();
        } else {
            std::fs::remove_file(&removed).unwrap();
        }
        delete_terminal(&mut app, vec![removed.clone()]);
        let generation = app.items_generation;
        assert!(app.external_rescan_pending.is_some());
        settle(&mut app, &ctx);
        assert_eq!(
            app.items_generation, generation,
            "self Delete must reach Equal without install"
        );
        assert!(
            !Arc::ptr_eq(
                &previous_rows,
                &app.folder_observation.as_ref().unwrap().rows
            ),
            "Equal must accept the worker record"
        );
        let stack_index = survivor_count - 1;
        assert!(matches!(
            app.items[stack_index],
            GridItem::Stack { count: 2, .. }
        ));
        assert_eq!(
            sibling_view.materialize_aggregated().0[0].drag_source_path(),
            Some(removed.as_path()),
            "shared sibling snapshot must remain unchanged"
        );
        assert!(app.stack_try_open_from_grid(&ctx, stack_index, false));
        settle(&mut app, &ctx);
        assert!(app.stack_showing_flat);
        assert_eq!(app.fullscreen_idx, Some(stack_index));
        assert_eq!(
            app.items[stack_index].drag_source_path(),
            Some(folder.join("b_1.jpg").as_path())
        );
        assert_eq!(app.items.len(), survivor_count + 1);
        app.close_fullscreen();
        app.stack_reconcile_after_fullscreen_close(&ctx);
        settle(&mut app, &ctx);
        assert_eq!(app.items.len(), survivor_count);
        assert!(matches!(
            app.items[stack_index],
            GridItem::Stack { count: 2, .. }
        ));
    }
}

#[test]
fn section1375_stack_flat_delete_rebuilds_retained_singleton_metadata_and_maps() {
    let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg"]);
    let key = crate::adjustment_db::normalize_path(&folder.join("b_2.jpg"));
    app.mask_db
        .as_ref()
        .unwrap()
        .set(&key, &[true], &[], 1, 1)
        .unwrap();
    app.rating_db.as_ref().unwrap().set(&key, 5).unwrap();
    app.tags_db
        .as_mut()
        .unwrap()
        .set_item_tags(&key, ["survivor-tag"], "test")
        .unwrap();
    app.stack_mode_requested = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    assert!(app.stack_try_open_from_grid(&ctx, 1, false));
    settle(&mut app, &ctx);
    assert!(app.stack_showing_flat);
    assert!(app.mask_pages.contains(&2));
    std::fs::remove_file(folder.join("b_1.jpg")).unwrap();
    delete_terminal(&mut app, vec![folder.join("b_1.jpg")]);
    assert_eq!(app.stack_view.as_ref().unwrap().groups[1].members.len(), 1);
    assert!(app.mask_pages.contains(&1));
    settle(&mut app, &ctx);
    app.close_fullscreen();
    app.stack_reconcile_after_fullscreen_close(&ctx);
    settle(&mut app, &ctx);
    assert!(!app.stack_showing_flat);
    assert_eq!(app.items.len(), 2);
    assert_eq!(
        app.items[1].drag_source_path(),
        Some(folder.join("b_2.jpg").as_path())
    );
    assert!(
        app.mask_pages.contains(&1),
        "new singleton must inherit its saved edit"
    );
    assert_eq!(app.get_rating(1), 5);
    assert_eq!(
        app.tags_cache.get(&key),
        Some(&vec!["#survivor-tag".to_owned()])
    );
    assert_eq!(
        app.stack_view
            .as_ref()
            .unwrap()
            .flat_index_for_aggregated(1),
        Some(1)
    );
}

#[test]
fn section1375_stack_zero_success_keeps_view_and_pending_switch() {
    let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg"]);
    app.stack_mode_requested = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let original = app.stack_view.as_ref().unwrap().clone();
    assert!(app.stack_try_open_from_grid(&ctx, 1, false));
    assert!(app.stack_script_pending.is_some());
    delete_terminal(&mut app, vec![]);
    assert!(Arc::ptr_eq(&original, app.stack_view.as_ref().unwrap()));
    settle(&mut app, &ctx);
    assert!(app.stack_showing_flat);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(
        app.items[1].drag_source_path(),
        Some(folder.join("b_1.jpg").as_path())
    );
}

#[test]
fn section1375_delete_during_initial_stack_extraction_or_grouping_resumes_only_after_success() {
    for grouping in [false, true] {
        for succeeded in [false, true] {
            let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg", "c_1.jpg"]);
            app.stack_mode_requested = true;
            app.load_folder(folder.clone());
            let deadline = Instant::now() + Duration::from_secs(20);
            while app.sidecar_restore_active() {
                app.poll_sidecar_restore(&ctx);
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            if grouping {
                app.poll_stack_script(&ctx);
                assert!(matches!(
                    app.stack_script_pending,
                    Some(crate::filename_stack_ui::StackPreparePending::Grouping(_))
                ));
            } else {
                assert!(matches!(
                    app.stack_script_pending,
                    Some(crate::filename_stack_ui::StackPreparePending::Extracting(_))
                ));
            }
            assert!(app.stack_view.is_none());
            assert!(
                app.grid_item_input_allowed(),
                "initial grouping accepts ordinary Delete"
            );
            let generation = app.items_generation;
            if succeeded {
                let removed = folder.join("a_1.jpg");
                std::fs::remove_file(&removed).unwrap();
                delete_terminal(&mut app, vec![removed]);
                assert!(
                    app.folder_observation.is_none(),
                    "unfinished consumer cannot authorize Equal after input changes"
                );
                assert!(
                    app.external_rescan_pending
                        .as_ref()
                        .unwrap()
                        .proof
                        .as_ref()
                        .unwrap()
                        .baseline
                        .is_none()
                );
            } else {
                delete_terminal(&mut app, vec![]);
                assert_eq!(app.items_generation, generation);
                assert!(app.stack_script_pending.is_some());
            }
            settle(&mut app, &ctx);
            assert!(app.stack_mode_requested);
            assert!(
                app.stack_view.is_some(),
                "Stack intent must eventually materialize"
            );
            assert!(
                app.items
                    .iter()
                    .any(|item| matches!(item, GridItem::Stack { count: 2, .. }))
            );
            assert_eq!(app.items.len(), if succeeded { 2 } else { 3 });
            let stable = app.items_generation;
            app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
            settle(&mut app, &ctx);
            assert_eq!(
                app.items_generation, stable,
                "completed grouping restores Equal eligibility"
            );
        }
    }
}

#[test]
fn section1375_delete_during_initial_subfolder_stack_uses_current_survivor_snapshot() {
    for grouping in [false, true] {
        let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg", "c_1.jpg"]);
        app.start_subfolder_expansion_scan_roots(folder.clone(), vec![folder.clone()]);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app.items_are_subfolder_expansion_view || app.sidecar_restore_active() {
            app.poll_subfolder_expansion(&ctx);
            app.poll_sidecar_restore(&ctx);
            assert!(Instant::now() < deadline, "subfolder input did not settle");
            std::thread::yield_now();
        }
        app.toggle_stack_mode();
        if grouping {
            app.poll_stack_script(&ctx);
        }
        assert!(app.stack_view.is_none());
        assert!(app.stack_script_pending.is_some());
        let shared_entries = app
            .subfolder_expansion_snapshot
            .as_ref()
            .unwrap()
            .entries
            .clone();
        let removed = folder.join("a_1.jpg");
        std::fs::remove_file(&removed).unwrap();
        delete_terminal(&mut app, vec![removed.clone()]);
        assert!(
            app.stack_script_pending.is_some(),
            "synthetic input resumes through its existing grouping owner"
        );
        settle(&mut app, &ctx);
        assert!(app.stack_mode_requested && app.items_are_subfolder_expansion_view);
        assert!(app.stack_view.is_some());
        assert_eq!(app.items.len(), 2);
        assert!(
            app.items
                .iter()
                .any(|item| matches!(item, GridItem::Stack { count: 2, .. }))
        );
        assert!(
            !app.stack_view
                .as_ref()
                .unwrap()
                .materialize_flat()
                .0
                .iter()
                .any(|item| item.drag_source_path() == Some(removed.as_path()))
        );
        assert_eq!(
            shared_entries.len(),
            4,
            "shared snapshot retains its original immutable entries"
        );
    }
}

#[test]
fn section1375_stack_retained_read_failure_keeps_flat_until_ordinary_reload() {
    let (mut app, folder, ctx) = fixture(&["a_1.jpg", "b_1.jpg", "b_2.jpg"]);
    app.stack_mode_requested = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    assert!(app.stack_try_open_from_grid(&ctx, 1, false));
    settle(&mut app, &ctx);
    let source = app.capture_stack_deletion().unwrap();
    let removed = folder.join("b_1.jpg");
    std::fs::remove_file(&removed).unwrap();
    let mut prepared = std::thread::spawn(move || source.prepare(&[removed]))
        .join()
        .unwrap();
    prepared.discard_retained_for_test();
    app.adopt_stack_deletion(prepared);
    // The successful terminal removes the visible flat member in the same frame.
    app.remove_items_batch(&[1]);
    assert!(app.folder_observation.is_none());
    assert!(app.folder_refresh_pending.is_some());
    app.close_fullscreen();
    app.stack_reconcile_after_fullscreen_close(&ctx);
    assert!(
        app.stack_showing_flat,
        "flat rows must not acquire aggregate navigation before install"
    );
    assert!(
        app.stack_script_pending.is_none(),
        "do not enter aggregate Recovering with flat rows"
    );
    app.consume_folder_refresh_pending(&ctx);
    settle(&mut app, &ctx);
    assert!(!app.stack_showing_flat);
    assert_eq!(app.items.len(), 2);
    assert_eq!(
        app.items[1].drag_source_path(),
        Some(folder.join("b_2.jpg").as_path())
    );
}

#[test]
fn section1375_stack_member_changes_and_equal_facet_details_keep_projection() {
    let (mut app, folder, ctx) =
        fixture(&["post_1.jpg", "post_2.jpg", "post_3.jpg", "other_1.jpg"]);
    app.stack_mode_requested = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item, GridItem::Stack { count: 3, .. }))
    );
    let generation = app.items_generation;
    let record = app.folder_observation.as_ref().unwrap();
    assert_eq!(record.source, folder);
    assert_eq!(
        record.generation, generation,
        "Stack adoption must bind the resulting generation"
    );
    assert_eq!(
        record.projection_key,
        app.current_folder_projection_key(&folder)
    );
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    assert!(
        app.external_rescan_pending
            .as_ref()
            .unwrap()
            .proof
            .as_ref()
            .unwrap()
            .baseline
            .is_some(),
        "unchanged Stack must retain its physical baseline proof"
    );
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    for (removed, count) in [("post_3.jpg", 2), ("post_2.jpg", 1)] {
        std::fs::remove_file(folder.join(removed)).unwrap();
        let generation = app.items_generation;
        app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        settle(&mut app, &ctx);
        assert!(app.items_generation > generation);
        assert_eq!(app.items.len(), 2);
        if count == 1 {
            assert!(
                app.items
                    .iter()
                    .all(|item| matches!(item, GridItem::Image(_)))
            );
        } else {
            assert!(
                app.items
                    .iter()
                    .any(|item| matches!(item, GridItem::Stack { count: 2, .. }))
            );
        }
    }
    app.settings.facet_filter.exts.insert("jpg".into());
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.reload_current_folder_preserving_override();
    settle(&mut app, &ctx);
    let visible = app.visible_indices.clone();
    let details = app.details_order.clone();
    let generation = app.items_generation;
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.visible_indices, visible);
    assert_eq!(app.details_order, details);
}

#[test]
fn section1375_tag_and_edit_sidecar_policy_off_on_explicit_and_open() {
    for edits in [false, true] {
        for tags in [false, true] {
            let (mut app, folder, ctx) = fixture(&["a.jpg"]);
            app.settings.sidecar_backup_enabled = edits;
            app.settings.tag_sidecar_backup_enabled = tags;
            let mut sidecar = crate::sidecar::SidecarFile::load(&folder);
            sidecar.set_adjust(
                "a.jpg",
                crate::adjustment::AdjustParams {
                    brightness: 13.0,
                    ..Default::default()
                },
            );
            sidecar.set_tags("a.jpg", ["external-tag"]);
            assert!(sidecar.flush_blocking());
            std::fs::write(folder.join("b.jpg"), b"changed-row").unwrap();
            let starts = sidecar_restore::import_starts_for_test();
            app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
            settle(&mut app, &ctx);
            assert_eq!(sidecar_restore::import_starts_for_test(), starts);
            let key = crate::adjustment_db::normalize_path(&folder.join("a.jpg"));
            assert!(!app.adjusted_page_keys.contains(&key));
            app.reload_current_folder_preserving_override();
            settle(&mut app, &ctx);
            assert_eq!(
                sidecar_restore::import_starts_for_test() > starts,
                edits || tags
            );
            assert_eq!(app.adjusted_page_keys.contains(&key), edits);
            let a = app
                .items
                .iter()
                .position(|item| item.drag_source_path() == Some(folder.join("a.jpg").as_path()))
                .unwrap();
            assert_eq!(
                app.cell_tag_list(a)
                    .iter()
                    .any(|tag| tag == "#external-tag"),
                tags
            );
            let starts = sidecar_restore::import_starts_for_test();
            app.load_folder(folder.clone());
            settle(&mut app, &ctx);
            assert_eq!(
                sidecar_restore::import_starts_for_test() > starts,
                edits || tags
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn section1375_same_folder_fork_park_mount_independent_subscription_and_close() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let a = app.projected_viewer_context_id();
    let a_epoch = app.current_folder_watch.as_ref().unwrap().epoch;
    let old_rows = app.folder_observation.as_ref().unwrap().rows.clone();
    let b = app.fork_materialized_still_context(&folder, 0);
    app.with_viewer_context(b, |app| {
        assert!(Arc::ptr_eq(
            &old_rows,
            &app.folder_observation.as_ref().unwrap().rows
        ));
        assert_eq!(
            app.folder_observation.as_ref().unwrap().generation,
            app.items_generation
        );
        assert!(app.current_folder_watch.is_none());
        let generation = app.items_generation;
        app.ensure_current_folder_watch(&ctx);
        settle(app, &ctx);
        assert_ne!(app.current_folder_watch.as_ref().unwrap().epoch, a_epoch);
        assert_eq!(
            app.items_generation, generation,
            "first confirmation scan can Equal"
        );
        std::fs::remove_file(folder.join("b.jpg")).unwrap();
        delete_terminal(app, vec![folder.join("b.jpg")]);
        let terminal = app.items_generation;
        settle(app, &ctx);
        assert_eq!(
            app.items_generation, terminal,
            "first fork Delete can Equal"
        );
    })
    .unwrap();
    app.with_viewer_context(a, |app| {
        assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, a_epoch);
        assert!(Arc::ptr_eq(
            &old_rows,
            &app.folder_observation.as_ref().unwrap().rows
        ));
        app.pause_mounted_background_work_keep_current_frame();
        app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        settle(app, &ctx);
        assert_eq!(
            app.items.len(),
            1,
            "parked sibling reconciles its own old record"
        );
        assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, a_epoch);
    })
    .unwrap();
    let b_cancel = app
        .with_viewer_context(b, |app| {
            app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
            app.external_rescan_pending.as_ref().unwrap().cancel.clone()
        })
        .unwrap();
    app.retire_context(b, "section1375-test-close", |_| ())
        .unwrap();
    assert!(b_cancel.load(Ordering::Relaxed));
    assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, a_epoch);
}

#[cfg(windows)]
#[test]
fn section1375_split_primitive_retains_original_dirty_and_uses_inherited_baseline() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let epoch = app.current_folder_watch.as_ref().unwrap().epoch;
    let original = app.folder_observation.as_ref().unwrap().rows.clone();
    let mut fork = app.split_current_context_preserving_main_grid();
    assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
    app.swap_viewer_context_bundle(&mut fork);
    assert!(app.current_folder_watch.is_none());
    assert!(Arc::ptr_eq(
        &original,
        &app.folder_observation.as_ref().unwrap().rows
    ));
    let generation = app.items_generation;
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    let fork_epoch = app.current_folder_watch.as_ref().unwrap().epoch;
    app.swap_viewer_context_bundle(&mut fork);
    assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
    assert_ne!(epoch, fork_epoch);
}

#[test]
fn section1375_cutoff_after_scan_notification_restarts_and_stale_never_acknowledges() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let watch = app.current_folder_watch.as_mut().unwrap();
    let sequence = watch.sequence.fetch_add(1, Ordering::AcqRel) + 1;
    watch
        .tx
        .send((
            sequence,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(folder.join("a.jpg"))),
        ))
        .unwrap();
    watch.collect();
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    // Freeze the first completed scan before the external mutation. Acknowledging
    // the newest sequence instead of the captured cutoff must now fail below.
    let pending = app.external_rescan_pending.as_mut().unwrap();
    let first = pending.rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(matches!(first, Some(ExternalRescanResult::Equal(..))));
    let cutoff = pending.proof.as_ref().unwrap().subscription.unwrap().1;
    assert_eq!(cutoff, sequence);
    let (tx, rx) = mpsc::channel();
    tx.send(first).unwrap();
    pending.rx = rx;
    std::fs::write(folder.join("later.jpg"), b"external-new-file").unwrap();
    let watch = app.current_folder_watch.as_mut().unwrap();
    let later = watch.sequence.fetch_add(1, Ordering::AcqRel) + 1;
    watch
        .tx
        .send((
            later,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(folder.join("a.jpg"))),
        ))
        .unwrap();
    app.poll_external_rescan(&ctx);
    let watch = app.current_folder_watch.as_ref().unwrap();
    assert_eq!(watch.acknowledged, cutoff);
    assert!(
        !watch.dirty.is_empty(),
        "later notification must remain dirty"
    );
    assert!(
        app.external_rescan_pending.is_some(),
        "later notification requires another scan"
    );
    assert!(
        !app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(folder.join("later.jpg").as_path()))
    );
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(folder.join("later.jpg").as_path()))
    );
    assert!(app.current_folder_watch.as_ref().unwrap().acknowledged >= later);
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    app.external_rescan_pending
        .as_mut()
        .unwrap()
        .items_generation = app.items_generation.wrapping_add(1);
    let ack = app.current_folder_watch.as_ref().unwrap().acknowledged;
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder_watch.as_ref().unwrap().acknowledged, ack);
    assert!(app.folder_refresh_pending.is_some());
}

#[test]
fn section1375_large_equal_poll_and_repeated_delete_overlay_do_not_clone_history() {
    for count in [10_000, 100_000] {
        let mut app = setup_app_for_test();
        let folder = app.tmp.path().join("large-adoption");
        std::fs::create_dir(&folder).unwrap();
        let mut scan = folder_scan::signature_tests::fixture(&folder);
        scan.folders.clear();
        scan.all_media.clear();
        scan.all_media
            .extend((0..count).map(|index| folder_scan::ScannedMediaEntry {
                path: folder.join(format!("row-{index:06}.jpg")),
                kind: folder_scan::ScanMediaKind::Image,
                mtime: 10,
                file_size: 20,
                sort_meta: crate::settings::ListingSortMetadata::new(10, Some(20)),
            }));
        let settings = app.settings.clone();
        let worker_folder = folder.clone();
        let listing = std::thread::spawn(move || {
            folder_scan::materialize_local_folder_listing(&worker_folder, scan, &settings)
        })
        .join()
        .unwrap();
        let rows = listing.row_record;
        app.items = listing.items;
        app.image_metas = listing.metas;
        app.thumbnails = (0..count).map(|_| ThumbnailState::Pending).collect();
        app.current_folder = Some(folder.clone());
        app.current_folder_last_mtime = Some(SystemTime::UNIX_EPOCH);
        app.rating_cache = (0..count).map(|index| (index, 0)).collect();
        app.folder_observation = Some(FolderObservation {
            source: folder.clone(),
            generation: app.items_generation,
            projection_key: app.current_folder_projection_key(&folder),
            rows: rows.clone(),
            removed: Arc::new(FolderRemovedRows::default()),
            revision: 0,
        });
        let (tx, rx) = mpsc::channel();
        tx.send(Some(ExternalRescanResult::Equal(
            folder_scan::FolderScanSignature::for_test(1),
            rows,
            Default::default(),
            SystemTime::UNIX_EPOCH,
        )))
        .unwrap();
        let baseline = app.folder_observation.clone();
        app.external_rescan_pending = Some(ExternalRescanPending {
            proof: Some(FolderRescanProof {
                baseline,
                projection_key: app.current_folder_projection_key(&folder),
                subscription: None,
            }),
            cause: Default::default(),
            owner_context_id: app.edit_request_owner_context(),
            folder: folder.clone(),
            mtime: SystemTime::UNIX_EPOCH,
            items_generation: app.items_generation,
            scan_options: app.current_external_scan_options(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            restart_requested: false,
        });
        let items_ptr = app.items.as_ptr();
        let generation = app.items_generation;
        let start = Instant::now();
        app.poll_external_rescan(&egui::Context::default());
        let poll_us = start.elapsed().as_secs_f64() * 1_000_000.0;
        assert_eq!(app.items.as_ptr(), items_ptr);
        assert_eq!(app.items_generation, generation);
        let mut previous = app.folder_observation.as_ref().unwrap().removed.clone();
        let start = Instant::now();
        for _ in 0..32 {
            let last = app.items.len() - 1;
            app.remove_items_batch(&[last]);
            let current = app.folder_observation.as_ref().unwrap().removed.clone();
            assert!(
                Arc::ptr_eq(current.previous.as_ref().unwrap(), &previous),
                "only the new removal chunk is allocated"
            );
            previous = current;
        }
        let delete_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            app.folder_observation
                .as_ref()
                .unwrap()
                .removed
                .paths()
                .count(),
            32
        );
        assert_eq!(app.rating_cache.len(), count - 32);
        eprintln!(
            "section1375 rows={count} ui_equal_poll_us={poll_us:.3} existing_remove_32_ms={delete_ms:.3} overlay_history_clones=0"
        );
    }
}

#[test]
fn section1375_favorite_effective_projection_key_matches_worker_and_first_equal() {
    let (mut app, folder, ctx) = fixture(&["z.jpg", "a.jpg"]);
    let favorite =
        crate::settings::FavoriteEntry::new("section1375 favorite".into(), folder.clone());
    let mut state = crate::settings::FavoriteViewState::from_settings(&app.settings);
    state.sort_order = crate::settings::SortOrder::FileNameDesc;
    app.favorite_view_states.insert(favorite.id, state);
    app.settings.favorites.push(favorite);
    app.settings.remember_favorite_view_state = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let generation = app.items_generation;
    assert_eq!(
        app.folder_observation.as_ref().unwrap().projection_key,
        app.current_folder_projection_key(&folder)
    );
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert!(app.folder_refresh_pending.is_none());
}

#[test]
fn section1375_delete_mixed_external_changes_notification_before_after_or_none() {
    for ordering in ["before", "after", "none"] {
        let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
        let generation = app.items_generation;
        if ordering == "before" {
            app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        }
        std::fs::remove_file(folder.join("a.jpg")).unwrap();
        std::fs::write(folder.join("b.jpg"), b"overwritten-source-is-longer").unwrap();
        std::fs::write(folder.join("c.jpg"), b"new-source").unwrap();
        delete_terminal(&mut app, vec![folder.join("a.jpg")]);
        if ordering == "after" {
            app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        }
        settle(&mut app, &ctx);
        assert!(
            app.items_generation > generation.wrapping_add(1),
            "{ordering}: external differences need an install"
        );
        let names = app
            .items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            names,
            ["b.jpg".to_owned(), "c.jpg".to_owned()]
                .into_iter()
                .collect()
        );
        let b = app
            .items
            .iter()
            .position(|item| item.name() == "b.jpg")
            .unwrap();
        assert_eq!(
            app.image_metas[b].unwrap().1,
            b"overwritten-source-is-longer".len() as i64
        );
    }
}

#[test]
fn section1375_delete_mixed_directory_mtime_observes_without_display_sort_stamp_change() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    let child = folder.join("child");
    std::fs::create_dir(&child).unwrap();
    app.settings.sort_order = crate::settings::SortOrder::DateDesc;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let child_idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(child.as_path()))
        .unwrap();
    let displayed = app.image_metas[child_idx];
    std::fs::write(child.join("active.part"), b"partial-download").unwrap();
    std::fs::remove_file(folder.join("a.jpg")).unwrap();
    delete_terminal(&mut app, vec![folder.join("a.jpg")]);
    let generation = app.items_generation;
    let terminal_order = app.items.clone();
    settle(&mut app, &ctx);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.items, terminal_order);
    let child_idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(child.as_path()))
        .unwrap();
    assert_eq!(app.image_metas[child_idx], displayed);
}

#[test]
fn section1375_same_second_file_overwrite_is_observed_by_actual_worker() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    let b = folder.join("b.jpg");
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(123);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let generation = app.items_generation;
    std::fs::write(&b, b"image-source").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(stamp + Duration::from_micros(1))
        .unwrap();
    assert_ne!(std::fs::metadata(&b).unwrap().modified().unwrap(), stamp);
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert!(app.items_generation > generation);
}

#[test]
fn section1375_stack_late_hydration_keeps_automatic_import_policy_and_group_cursor_owner() {
    for rebase in [false, true] {
        let (mut app, folder, ctx) = fixture(&["a_1.jpg", "a_2.jpg", "b_1.jpg", "b_2.jpg"]);
        app.stack_mode_requested = true;
        app.load_folder(folder.clone());
        settle(&mut app, &ctx);
        app.move_grid_cursor(Some(0), GridCursorIntent::Explicit(GridCursorMove::Move));
        app.settings.sidecar_backup_enabled = true;
        let mut sidecar = crate::sidecar::SidecarFile::load(&folder);
        sidecar.set_adjust(
            "b_1.jpg",
            crate::adjustment::AdjustParams {
                brightness: 15.0,
                ..Default::default()
            },
        );
        assert!(sidecar.flush_blocking());
        std::fs::write(folder.join("c_1.jpg"), b"new").unwrap();
        let imports = sidecar_restore::import_starts_for_test();
        app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.external_rescan_pending.is_some() {
            app.poll_external_rescan(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        while app.sidecar_restore_active() {
            app.poll_sidecar_restore(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(app.stack_script_pending.is_some());
        if rebase {
            while !matches!(
                app.stack_script_pending,
                Some(crate::filename_stack_ui::StackPreparePending::Grouping(_))
            ) {
                app.poll_stack_script(&ctx);
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            app.settings
                .grid_display_order
                .assign(crate::settings::GridItemDisplayKind::Archive, 3);
            app.poll_stack_script(&ctx);
            assert!(
                matches!(
                    app.stack_script_pending,
                    Some(crate::filename_stack_ui::StackPreparePending::Extracting(_))
                ),
                "changed display order must rebase the group source"
            );
        }
        let desired = folder.join("b_1.jpg");
        let desired_idx = app
            .items
            .iter()
            .position(|item| item.drag_source_path() == Some(desired.as_path()))
            .unwrap();
        app.move_grid_cursor(
            Some(desired_idx),
            GridCursorIntent::Explicit(GridCursorMove::Move),
        );
        app.stack_selection_target = Some(
            crate::filename_stack_ui::StackSelectionTarget::MemberPath(desired.clone()),
        );
        settle(&mut app, &ctx);
        assert_eq!(
            sidecar_restore::import_starts_for_test(),
            imports,
            "late group hydration must remain automatic"
        );
        assert!(
            matches!(&app.items[app.selected.unwrap()], GridItem::Stack { representative, .. } if representative == &desired)
        );
    }
}

#[cfg(windows)]
#[test]
fn section1375_two_context_diffs_wait_for_global_hydration_without_new_notification() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    let a = app.projected_viewer_context_id();
    let b = app.fork_materialized_still_context(&folder, 0);
    std::fs::write(folder.join("b.jpg"), b"new").unwrap();
    app.with_viewer_context(b, |app| {
        app.ensure_current_folder_watch(&ctx);
    })
    .unwrap();
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !app.sidecar_restore_active() {
        assert!(
            app.external_rescan_pending.is_some(),
            "A Diff must start its normal hydration"
        );
        app.poll_external_rescan(&ctx);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(app.sidecar_restore_active());
    app.with_viewer_context(b, |app| {
        let generation = app.items_generation;
        app.poll_external_rescan(&ctx);
        assert!(app.external_rescan_pending.is_some());
        assert_eq!(app.items_generation, generation);
    })
    .unwrap();
    app.with_viewer_context(a, |app| {
        settle(app, &ctx);
    })
    .unwrap();
    app.with_viewer_context(b, |app| {
        settle(app, &ctx);
        assert_eq!(app.items.len(), 2);
        assert!(app.external_rescan_pending.is_none());
    })
    .unwrap();
}

#[test]
fn section1375_search_root_delete_does_not_dispatch_physical_folder_reload() {
    let (mut app, folder, ctx) = fixture(&["a.jpg", "b.jpg"]);
    app.items_are_global_search_view = true;
    app.global_search.active = true;
    let generation = app.items_generation;
    std::fs::remove_file(folder.join("a.jpg")).unwrap();
    delete_terminal(&mut app, vec![folder.join("a.jpg")]);
    assert!(app.external_rescan_pending.is_none());
    assert!(app.items_are_global_search_view);
    assert!(app.global_search.active);
    assert_eq!(app.items_generation, generation.wrapping_add(1));
    app.ensure_current_folder_watch(&ctx);
    app.poll_current_folder_watch(&ctx);
    assert!(app.external_rescan_pending.is_none());
    assert!(
        app.current_folder_watch.is_none(),
        "synthetic root must not retain an unregistered physical watch"
    );
    app.items_are_global_search_view = false;
    app.global_search.active = false;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let physical_generation = app.items_generation;
    app.ensure_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    assert_eq!(
        app.items_generation, physical_generation,
        "return confirmation can Equal"
    );
    app.poll_current_folder_watch(&ctx);
    app.poll_external_rescan(&ctx);
    assert!(app.current_folder_watch.as_ref().unwrap().watcher.is_some());
    std::fs::write(folder.join("c.jpg"), b"returned physical notification").unwrap();
    app.current_folder_watch
        .as_ref()
        .unwrap()
        .tx
        .send((
            1,
            Ok(
                notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
                    .add_path(folder.join("c.jpg")),
            ),
        ))
        .unwrap();
    app.poll_current_folder_watch(&ctx);
    app.current_folder_watch.as_mut().unwrap().debounce_until = Some(Instant::now());
    app.poll_current_folder_watch(&ctx);
    settle(&mut app, &ctx);
    assert!(
        app.items
            .iter()
            .any(|item| item.drag_source_path() == Some(folder.join("c.jpg").as_path())),
        "first physical notification after return must reconcile"
    );
}

#[cfg(windows)]
#[test]
fn section1375_direct_reader_both_forks_rebind_only_saved_physical_basis() {
    for live in [false, true] {
        let (mut app, folder, ctx) = fixture(&["a.jpg", "z.jpg"]);
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        std::fs::write(folder.join("m.png"), b"excluded direct target").unwrap();
        app.load_folder(folder.clone());
        settle(&mut app, &ctx);
        let basis_rows = app.folder_observation.as_ref().unwrap().rows.clone();
        let basis_identity = app.page_identity_index.clone();
        let target = startup_ops::DirectMediaPrepared::image_fixture_for_test(folder.join("m.png"));
        app.install_direct_media_reader(target).unwrap();
        app.fullscreen_idx = Some(2);
        let transient_generation = app.items_generation;
        let _detached = if live {
            app.split_current_context_preserving_main_grid()
        } else {
            app.split_materialized_physical_context_for_detached_scope(
                &folder,
                2,
                transient_generation,
            )
        };
        assert_eq!(app.items.len(), 2);
        assert!(Arc::ptr_eq(&basis_identity, &app.page_identity_index));
        assert!(Arc::ptr_eq(
            &basis_rows,
            &app.folder_observation.as_ref().unwrap().rows
        ));
        assert_eq!(
            app.folder_observation.as_ref().unwrap().generation,
            app.items_generation
        );
        let generation = app.items_generation;
        app.ensure_current_folder_watch(&ctx);
        settle(&mut app, &ctx);
        assert_eq!(
            app.items_generation, generation,
            "first unchanged direct-reader basis confirmation can Equal"
        );
        std::fs::remove_file(folder.join("z.jpg")).unwrap();
        delete_terminal(&mut app, vec![folder.join("z.jpg")]);
        let terminal = app.items_generation;
        settle(&mut app, &ctx);
        assert_eq!(
            app.items_generation, terminal,
            "first Delete on restored basis can Equal"
        );
    }
    let (mut app, folder, _) = fixture(&["a.jpg", "z.jpg"]);
    app.folder_observation.as_mut().unwrap().generation = app.items_generation.wrapping_sub(1);
    let stale = app.folder_observation.as_ref().unwrap().generation;
    let target = startup_ops::DirectMediaPrepared::image_fixture_for_test(folder.join("m.png"));
    app.install_direct_media_reader(target).unwrap();
    let _detached = app.split_current_context_preserving_main_grid();
    assert_eq!(
        app.folder_observation.as_ref().unwrap().generation,
        stale,
        "unproven basis must stay unknown"
    );
}

#[test]
fn section1375_queued_different_cancel_and_stale_retire_off_ui_at_large_sizes() {
    for count in [10_000, 100_000] {
        for route in ["cancel", "replace", "stale"] {
            let mut app = setup_app_for_test();
            let folder = app.tmp.path().join("queued-retire");
            std::fs::create_dir(&folder).unwrap();
            let mut scan = folder_scan::signature_tests::fixture(&folder);
            scan.folders.clear();
            scan.all_media = (0..count)
                .map(|index| folder_scan::ScannedMediaEntry {
                    path: folder.join(format!("row-{index:06}.jpg")),
                    kind: folder_scan::ScanMediaKind::Image,
                    mtime: 10,
                    file_size: 20,
                    sort_meta: crate::settings::ListingSortMetadata::new(10, Some(20)),
                })
                .collect();
            let settings = app.settings.clone();
            let worker_folder = folder.clone();
            let (result, weak) = std::thread::spawn(move || {
                let signature = folder_scan::signature_from_scan(&scan);
                let materialized = folder_scan::materialize_local_folder_listing(
                    &worker_folder,
                    scan.clone_for_projection(),
                    &settings,
                );
                let weak = Arc::downgrade(&materialized.row_record);
                scan.reconcile = Some(folder_scan::FolderScanReconcile {
                    observed_signature: signature,
                    parent_mtime: SystemTime::UNIX_EPOCH,
                    materialized: Box::new(materialized),
                });
                (
                    ExternalRescanResult::Different(scan, signature, Default::default()),
                    weak,
                )
            })
            .join()
            .unwrap();
            let (tx, rx) = mpsc::channel();
            tx.send(Some(result)).unwrap();
            app.current_folder = Some(folder.clone());
            app.current_folder_last_mtime = Some(SystemTime::UNIX_EPOCH);
            let cancel = Arc::new(AtomicBool::new(false));
            app.external_rescan_pending = Some(ExternalRescanPending {
                proof: None,
                cause: Default::default(),
                owner_context_id: app.edit_request_owner_context(),
                folder: if route == "replace" {
                    folder.join("old")
                } else {
                    folder.clone()
                },
                mtime: SystemTime::UNIX_EPOCH,
                items_generation: if route == "stale" {
                    app.items_generation.wrapping_sub(1)
                } else {
                    app.items_generation
                },
                scan_options: app.current_external_scan_options(),
                cancel: cancel.clone(),
                rx,
                restart_requested: false,
            });
            let drops = folder_scan::row_record_drops_for_test();
            let start = Instant::now();
            match route {
                "cancel" => app
                    .external_rescan_pending
                    .take()
                    .unwrap()
                    .cancel_and_retire(),
                "replace" => app.start_external_rescan(
                    &egui::Context::default(),
                    folder,
                    SystemTime::UNIX_EPOCH,
                ),
                "stale" => app.poll_external_rescan(&egui::Context::default()),
                _ => unreachable!(),
            }
            let ui_us = start.elapsed().as_secs_f64() * 1_000_000.0;
            assert_eq!(
                folder_scan::row_record_drops_for_test(),
                drops,
                "large queued record must not destruct on UI route={route}"
            );
            if route != "stale" {
                assert!(cancel.load(Ordering::Relaxed));
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            while weak.strong_count() != 0 {
                assert!(Instant::now() < deadline, "retired payload did not release");
                std::thread::yield_now();
            }
            if let Some(pending) = app.external_rescan_pending.take() {
                pending.cancel_and_retire();
            }
            eprintln!(
                "section1375 queued_rows={count} route={route} ui_retire_us={ui_us:.3} ui_record_drops=0"
            );
        }
    }
}

#[test]
fn section1375_stack_flat_and_retained_aggregate_rebind_physical_record_for_equal() {
    let (mut app, folder, ctx) =
        fixture(&["post_1.jpg", "post_2.jpg", "post_3.jpg", "other_1.jpg"]);
    app.stack_mode_requested = true;
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    let stack = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Stack { count: 3, .. }))
        .unwrap();
    assert!(app.stack_try_open_from_grid(&ctx, stack, false));
    settle(&mut app, &ctx);
    assert!(app.stack_showing_flat);
    assert!(app.fullscreen_idx.is_some());
    let flat_generation = app.items_generation;
    assert_eq!(
        app.folder_observation.as_ref().unwrap().generation,
        flat_generation
    );
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(
        app.items_generation, flat_generation,
        "unchanged flat physical observation must Equal"
    );
    app.close_fullscreen();
    app.stack_reconcile_after_fullscreen_close(&ctx);
    settle(&mut app, &ctx);
    assert!(!app.stack_showing_flat);
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item, GridItem::Stack { count: 3, .. }))
    );
    let aggregate_generation = app.items_generation;
    assert_eq!(
        app.folder_observation.as_ref().unwrap().generation,
        aggregate_generation
    );
    app.check_external_folder_changes(&ctx, ExternalChangeCheck::Notified);
    settle(&mut app, &ctx);
    assert_eq!(
        app.items_generation, aggregate_generation,
        "retained unchanged aggregate observation must Equal"
    );
}
