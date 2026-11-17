use super::tests::{prepare_snapshot, recv, start_ready_app};
use super::*;
use crate::app::{AppTestEnvForTest, DetachedSource, ViewerNavigationScope, ViewerPresentation};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use std::time::{Duration, Instant};

fn fixture(
    kind: CollectionResolvedKind,
) -> (
    tempfile::TempDir,
    AppTestEnvForTest,
    Arc<CollectionPreparedSnapshot>,
) {
    fixture_with_next_kind(kind, CollectionResolvedKind::Folder)
}

fn fixture_with_next_kind(
    kind: CollectionResolvedKind,
    next_kind: CollectionResolvedKind,
) -> (
    tempfile::TempDir,
    AppTestEnvForTest,
    Arc<CollectionPreparedSnapshot>,
) {
    let temp = tempfile::tempdir().unwrap();
    let before = temp.path().join("z-before");
    let after = temp
        .path()
        .join(if next_kind == CollectionResolvedKind::Image {
            "a-after.png"
        } else {
            "a-after"
        });
    for folder in std::iter::once(&before)
        .chain((next_kind == CollectionResolvedKind::Folder).then_some(&after))
    {
        std::fs::create_dir_all(folder).unwrap();
        image::RgbImage::new(2, 2)
            .save(folder.join("page.png"))
            .unwrap();
    }
    if next_kind == CollectionResolvedKind::Image {
        image::RgbImage::new(2, 2).save(&after).unwrap();
    }
    let source = temp.path().join(match kind {
        CollectionResolvedKind::Zip => "middle.zip",
        CollectionResolvedKind::Image => "middle.png",
        _ => "middle",
    });
    match kind {
        CollectionResolvedKind::Folder => {
            std::fs::create_dir_all(&source).unwrap();
            image::RgbImage::new(2, 2)
                .save(source.join("page.png"))
                .unwrap();
        }
        CollectionResolvedKind::Image => image::RgbImage::new(2, 2).save(&source).unwrap(),
        CollectionResolvedKind::Zip => {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
            for name in ["root.jpg", "book-a/page.jpg"] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut writer, b"fixture page").unwrap();
            }
            writer.finish().unwrap();
        }
        _ => std::fs::write(&source, b"source identity").unwrap(),
    }
    let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
    app.active_quick_folder_slot = None;
    let created = recv(client.create_collection("Sibling order".into()).unwrap());
    let added = recv(
        client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &before,
                        CollectionResolvedKind::Folder,
                    )
                    .unwrap(),
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &source, kind,
                    )
                    .unwrap(),
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &after, next_kind,
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
    );
    let prepared = prepare_snapshot(&added.snapshot);
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.collection_catalog_contains(prepared.collection_id) {
        assert!(Instant::now() < deadline);
        app.poll_collection_ui(&ctx);
        std::thread::yield_now();
    }
    app.top_level_grid_view.begin(
        TopLevelGridSurface::Collection(CollectionGridIdentity {
            collection_id: prepared.collection_id,
        }),
        None,
    );
    app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
    assert_eq!(prepared.entries[1].source_path, source);
    if kind != CollectionResolvedKind::Image {
        let entry = &prepared.entries[1];
        app.commit_collection_grid_source_open(
            CollectionGridViewportAnchor {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            },
            source.clone(),
        );
        app.current_folder = Some(source.clone());
        app.items = vec![GridItem::Image(source.join("page.png"))];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.visible_indices = vec![0];
        app.image_metas = vec![None];
        app.selected = Some(0);
    } else {
        app.navigation_scope = ViewerNavigationScope::CollectionRoot;
        app.selected = Some(1);
    }
    (temp, app, prepared)
}

fn grid_key(app: &mut App, key: egui::Key) {
    let ctx = egui::Context::default();
    let modifiers = egui::Modifiers::CTRL;
    ctx.begin_pass(egui::RawInput {
        events: vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    });
    assert!(app.handle_keyboard(&ctx).is_none());
    ctx.end_pass().drop_without_applying_deltas();
}

fn pending_action(app: &mut App) -> CollectionNavigationAction {
    let pending = app
        .top_level_grid_view
        .take_collection_navigation_pending()
        .expect("collection owns navigation, never physical siblings");
    let action = pending.request().unwrap().action.clone();
    app.top_level_grid_view
        .set_collection_navigation_pending(Some(pending));
    assert!(app.folder_nav_pending.is_none());
    action
}

fn drain(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.top_level_grid_view.collection_navigation_pending() {
        assert!(Instant::now() < deadline, "collection navigation stalled");
        app.poll_collection_navigation(ctx);
        std::thread::yield_now();
    }
}

#[test]
fn section1763_collection_grid_and_fullscreen_siblings_share_ctrl_targets() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for kind in [CollectionResolvedKind::Folder, CollectionResolvedKind::Zip] {
        for fullscreen in [false, true] {
            for sibling in [false, true] {
                for forward in [false, true] {
                    let (_temp, mut app, prepared) = fixture(kind);
                    let ctx = egui::Context::default();
                    if fullscreen {
                        app.fullscreen_idx = Some(0);
                        if sibling {
                            app.handle_fullscreen_sibling_nav_context(&ctx, 0, forward, false);
                        } else {
                            app.handle_fullscreen_ctrl_nav_context(&ctx, 0, forward, false);
                        }
                        assert!(
                            matches!(pending_action(&mut app), CollectionNavigationAction::OuterFullscreen { forward: f, .. } if f == forward)
                        );
                    } else {
                        grid_key(
                            &mut app,
                            match (sibling, forward) {
                                (true, true) => egui::Key::PageDown,
                                (true, false) => egui::Key::PageUp,
                                (false, true) => egui::Key::ArrowDown,
                                (false, false) => egui::Key::ArrowUp,
                            },
                        );
                        assert!(
                            matches!(pending_action(&mut app), CollectionNavigationAction::OuterGrid { forward: f, .. } if f == forward)
                        );
                    }
                    drain(&mut app, &ctx);
                    assert_eq!(
                        app.effective_folder().as_ref(),
                        Some(&prepared.entries[if forward { 2 } else { 0 }].source_path)
                    );
                    app.shutdown_collection_runtime_for_exit();
                }
            }
        }
    }
}

#[test]
fn section1763_collection_root_image_sibling_uses_outer_order() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Image);
    let ctx = egui::Context::default();
    app.fullscreen_idx = Some(1);
    app.handle_fullscreen_sibling_nav_context(&ctx, 1, true, false);
    pending_action(&mut app);
    drain(&mut app, &ctx);
    assert_eq!(
        app.effective_folder().as_ref(),
        Some(&prepared.entries[2].source_path)
    );
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn section1763_collection_snapshot_siblings_keep_playable_traversal_and_overview() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, prepared) =
        fixture_with_next_kind(CollectionResolvedKind::Image, CollectionResolvedKind::Image);
    app.navigation_scope = ViewerNavigationScope::Main;
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    app.finish_page_identity_preparation_for_test();
    assert!(matches!(
        app.top_level_grid_view.surface(),
        TopLevelGridSurface::Snapshot
    ));
    assert!(app.top_level_grid_view.collection_session().is_none());
    assert!(matches!(
        app.top_level_grid_view.return_to(),
        Some(TopLevelGridRestore::Collection(_))
    ));
    assert!(app.has_detached_collection_navigation_origin()); // Legacy Ctrl predicate also matches.
    assert!(!app.collection_sibling_navigation_origin_available());
    app.fullscreen_idx = Some(1);
    let ctx = egui::Context::default();
    for (forward, expected) in [(true, 2), (false, 1)] {
        let action = if forward {
            crate::keymap::KeyAction::FsSiblingNext
        } else {
            crate::keymap::KeyAction::FsSiblingPrev
        };
        assert!(app.page_overview_executable_actions().contains(&action));
        let idx = app.fullscreen_idx.unwrap();
        app.handle_fullscreen_sibling_nav_context(&ctx, idx, forward, false);
        assert_eq!(app.fullscreen_idx, Some(expected));
        assert_eq!(
            app.items[expected],
            GridItem::Image(prepared.entries[expected].source_path.clone())
        );
        assert!(!app.top_level_grid_view.collection_navigation_pending());
        assert!(app.folder_nav_pending.is_none());
    }
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn section1763_collection_search_and_local_filter_siblings_keep_noop_and_overview() {
    use crate::ui_fullscreen::{FsBoundaryHint, FsNavNoOpReason};
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for search in ["global", "favorite", "local"] {
        let (_temp, mut app, _) = fixture(CollectionResolvedKind::Image);
        // Main's local filter retains the Collection surface/session. Unlike an
        // independent root-media viewer, it must reach the existing filter refusal.
        app.navigation_scope = ViewerNavigationScope::Main;
        match search {
            "global" => app.open_global_search(),
            "favorite" => app.open_favsearch(),
            _ => app.open_local_metadata_search(),
        }
        app.finish_page_identity_preparation_for_test();
        if search == "local" {
            assert!(matches!(
                app.top_level_grid_view.surface(),
                TopLevelGridSurface::Collection(_)
            ));
            assert!(app.top_level_grid_view.collection_session().is_some());
        } else {
            assert!(matches!(
                app.top_level_grid_view.surface(),
                TopLevelGridSurface::Search(_)
            ));
            assert!(app.top_level_grid_view.collection_session().is_none());
            assert!(matches!(
                app.top_level_grid_view.return_to(),
                Some(TopLevelGridRestore::Collection(_))
            ));
            assert!(!app.collection_sibling_navigation_origin_available());
        }
        app.fullscreen_idx = Some(1);
        let ctx = egui::Context::default();
        for forward in [true, false] {
            let action = if forward {
                crate::keymap::KeyAction::FsSiblingNext
            } else {
                crate::keymap::KeyAction::FsSiblingPrev
            };
            assert!(
                !app.page_overview_executable_actions().contains(&action),
                "{search}"
            );
            app.handle_fullscreen_sibling_nav_context(&ctx, 1, forward, false);
            let expected = if search == "local" {
                FsNavNoOpReason::LocalFilterActive
            } else {
                FsNavNoOpReason::SearchSiblingUnsupported
            };
            assert!(
                matches!(app.fs_boundary_hint, Some(FsBoundaryHint::NavNoOp { reason, .. }) if reason == expected),
                "{search}"
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(!app.top_level_grid_view.collection_navigation_pending());
            assert!(app.folder_nav_pending.is_none());
        }
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn section1763_temporary_collection_return_keeps_legacy_ctrl_navigation() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for search in ["snapshot", "global", "favorite"] {
        let (_temp, mut app, _) = fixture(CollectionResolvedKind::Image);
        app.navigation_scope = ViewerNavigationScope::Main;
        match search {
            "snapshot" => app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed),
            "global" => app.open_global_search(),
            _ => app.open_favsearch(),
        }
        app.finish_page_identity_preparation_for_test();
        app.fullscreen_idx = Some(1);
        assert!(
            app.page_overview_executable_actions()
                .contains(&crate::keymap::KeyAction::FsCtrlNavNext)
        );
        app.handle_fullscreen_ctrl_nav_context(&egui::Context::default(), 1, true, false);
        assert!(matches!(
            pending_action(&mut app),
            CollectionNavigationAction::OuterFullscreen { forward: true, .. }
        ));
        assert!(
            !app.page_overview_executable_actions()
                .contains(&crate::keymap::KeyAction::FsSiblingNext)
        );
        app.handle_fullscreen_sibling_nav_context(&egui::Context::default(), 1, true, false);
        assert!(matches!(
            pending_action(&mut app),
            CollectionNavigationAction::OuterFullscreen {
                queued_steps: 0,
                ..
            }
        ));
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn section1763_collection_mixed_burst_accumulates_before_sequence_gate() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for fullscreen in [false, true] {
        let (_temp, mut app, _) = fixture(CollectionResolvedKind::Folder);
        let ctx = egui::Context::default();
        if fullscreen {
            app.fullscreen_idx = Some(0);
            app.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
            assert!(app.fs_navigation_sequence_blocks_new_target());
            app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
            app.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
            app.handle_fullscreen_ctrl_nav_context(&ctx, 0, false, false);
            assert!(matches!(
                pending_action(&mut app),
                CollectionNavigationAction::OuterFullscreen {
                    queued_steps: 1,
                    ..
                }
            ));
        } else {
            for key in [
                egui::Key::PageDown,
                egui::Key::ArrowDown,
                egui::Key::PageDown,
                egui::Key::ArrowUp,
            ] {
                grid_key(&mut app, key);
            }
            assert!(matches!(
                pending_action(&mut app),
                CollectionNavigationAction::OuterGrid {
                    queued_steps: 1,
                    ..
                }
            ));
        }
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn section1763_collection_nested_zip_sibling_skips_internal_dfs() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for fullscreen in [false, true] {
        for sibling in [false, true] {
            let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Zip);
            let zip = prepared.entries[1].source_path.clone();
            let entries = ["root.jpg", "book-a/page.jpg"]
                .map(|name| crate::zip_loader::ZipImageEntry {
                    entry_name: name.into(),
                    uncompressed_size: 0,
                    mtime: 0,
                    container_ancestry: Vec::new(),
                })
                .into();
            app.zip_nav = Some(crate::zip_tree::ZipNavState::new(Arc::new(
                crate::zip_tree::ZipTree::build(zip, entries),
            )));
            app.zip_nav_show_current_level(crate::app::StartupListIntent::ExplicitList);
            // Materialization prepares the page identity index asynchronously. The main
            // keyboard correctly rejects input until this initial listing has settled.
            app.finish_page_identity_preparation_for_test();
            assert!(!app.any_dialog_open());
            let ctx = egui::Context::default();
            if fullscreen {
                let index = app.items.iter().position(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == "root.jpg")).unwrap();
                app.fullscreen_idx = Some(index);
                if sibling {
                    app.handle_fullscreen_sibling_nav_context(&ctx, index, true, false);
                } else {
                    app.handle_fullscreen_ctrl_nav_context(&ctx, index, true, false);
                }
            } else {
                grid_key(
                    &mut app,
                    if sibling {
                        egui::Key::PageDown
                    } else {
                        egui::Key::ArrowDown
                    },
                );
            }
            if sibling {
                pending_action(&mut app);
            } else {
                assert!(!app.top_level_grid_view.collection_navigation_pending());
                assert!(app.items.iter().any(|item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == "book-a/page.jpg")), "fullscreen={fullscreen}, rows={:?}, nav={:?}, input_seq={}", app.items, app.zip_nav.as_ref().map(|nav| nav.current()), app.input_seq);
            }
            app.shutdown_collection_runtime_for_exit();
        }
    }
}

#[test]
fn section1763_collection_physical_grid_request_uses_stable_source_owner() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for kind in [CollectionResolvedKind::Folder, CollectionResolvedKind::Zip] {
        let (_temp, mut app, prepared) = fixture(kind);
        let ctx = egui::Context::default();
        assert!(app.start_collection_outer_grid_navigation(&ctx, true));
        let pending = app
            .top_level_grid_view
            .take_collection_navigation_pending()
            .unwrap();
        let request = pending.request().unwrap();
        assert!(app.collection_navigation_request_is_current(request));
        app.items_generation += 1;
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision += 1;
        assert!(
            app.collection_navigation_request_is_current(request),
            "same book row refresh and unadopted revision notice preserve the source owner"
        );
        app.fullscreen_idx = Some(0);
        assert!(!app.collection_navigation_request_is_current(request));
        app.fullscreen_idx = None;
        let current = app.current_folder.clone();
        app.current_folder = Some(prepared.entries[2].source_path.clone());
        assert!(
            !app.collection_navigation_request_is_current(request),
            "a different visible book rejects the old request"
        );
        app.current_folder = current;
        let position = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .position
            .clone();
        if let CollectionGridPosition::PhysicalSource { path, .. } = &mut app
            .top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position
        {
            *path = prepared.entries[2].source_path.clone();
        }
        assert!(
            !app.collection_navigation_request_is_current(request),
            "a changed source path rejects the old request even with the same entry identity"
        );
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position = position.clone();
        let other = &prepared.entries[2];
        app.commit_collection_grid_source_open(
            CollectionGridViewportAnchor {
                entry_id: other.entry_id,
                source_key: other.source_key.clone(),
            },
            prepared.entries[1].source_path.clone(),
        );
        assert!(
            !app.collection_navigation_request_is_current(request),
            "a different entry rejects the old request even at the same path"
        );
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position = position;
        assert!(app.collection_navigation_request_is_current(request));
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .accepted_revision += 1;
        assert!(
            !app.collection_navigation_request_is_current(request),
            "an adopted collection revision retires the old source owner"
        );
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
fn section1763_ring_sibling_uses_collection_grid_owner_and_root_stays_put() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Folder);
    let ctx = egui::Context::default();
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::Grid,
        RingActionId::SiblingFolderNext,
        "section1763",
    );
    assert!(matches!(
        pending_action(&mut app),
        CollectionNavigationAction::OuterGrid { forward: true, .. }
    ));
    app.top_level_grid_view
        .set_collection_navigation_pending(None);
    app.apply_collection_grid_prepared(prepared, None);
    let before = app.items.clone();
    grid_key(&mut app, egui::Key::PageDown);
    assert_eq!(app.items, before);
    assert!(!app.top_level_grid_view.collection_navigation_pending());
    assert!(app.folder_nav_pending.is_none());
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn section1763_collection_sibling_edge_uses_ctrl_hint() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    for fullscreen in [false, true] {
        let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Folder);
        let last = &prepared.entries[2];
        app.commit_collection_grid_source_open(
            CollectionGridViewportAnchor {
                entry_id: last.entry_id,
                source_key: last.source_key.clone(),
            },
            last.source_path.clone(),
        );
        app.current_folder = Some(last.source_path.clone());
        let ctx = egui::Context::default();
        if fullscreen {
            app.fullscreen_idx = Some(0);
            app.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
        } else {
            grid_key(&mut app, egui::Key::PageDown);
        }
        drain(&mut app, &ctx);
        assert_eq!(app.effective_folder().as_ref(), Some(&last.source_path));
        if fullscreen {
            assert!(matches!(
                app.fs_boundary_hint,
                Some(crate::ui_fullscreen::FsBoundaryHint::NoImageFolder { forward: true, .. })
            ));
        } else {
            assert!(
                app.feedback_toasts_for_test
                    .iter()
                    .any(|(message, _)| message == "コレクション内の次のコンテナはありません")
            );
        }
        app.shutdown_collection_runtime_for_exit();
    }
}

#[test]
#[cfg(windows)]
fn section1763_native_video_sibling_enters_collection_navigation() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, _) = fixture(CollectionResolvedKind::Folder);
    let ctx = egui::Context::default();
    app.fullscreen_idx = Some(0);
    app.handle_native_video_key_event(
        &ctx,
        0,
        crate::video::native_window::NativeVideoKeyEvent {
            receipt: crate::mouse_seek_debug::test_receipt(1763),
            virtual_key: 0x22,
            scan_code: 0,
            extended: false,
            shift: false,
            ctrl: true,
            alt: false,
            repeat: false,
        },
    );
    assert!(matches!(
        pending_action(&mut app),
        CollectionNavigationAction::OuterFullscreen {
            forward: true,
            native_toast: true,
            ..
        }
    ));
    app.shutdown_collection_runtime_for_exit();
}

#[test]
fn section1763_physical_tag_and_reading_history_keep_sibling_routes() {
    for origin in ["folder", "tag", "history"] {
        for fullscreen in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            app.active_quick_folder_slot = None;
            let first = app.tmp.path().join("a");
            let next = app.tmp.path().join("b");
            for folder in [&first, &next] {
                std::fs::create_dir_all(folder).unwrap();
            }
            app.current_folder = Some(first);
            app.items = vec![GridItem::Image(app.tmp.path().join("a/page.png"))];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.visible_indices = vec![0];
            app.image_metas = vec![None];
            app.tag_view.active = origin == "tag";
            app.top_level_grid_view.begin(
                TopLevelGridSurface::Folder,
                (origin == "history").then(|| {
                    TopLevelGridRestore::ReadingHistory(
                        super::super::top_level_grid_view::ReadingHistoryRootRestore::unavailable(),
                    )
                }),
            );
            let ctx = egui::Context::default();
            if fullscreen {
                app.fullscreen_idx = Some(0);
                app.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
            } else {
                grid_key(&mut app, egui::Key::PageDown);
            }
            if origin == "tag" && !fullscreen {
                assert!(app.folder_nav_pending.is_none());
            } else {
                let pending = app
                    .folder_nav_pending
                    .take()
                    .expect("unchanged physical sibling path");
                assert_eq!(
                    pending.mode.perf_tag(),
                    if fullscreen {
                        "fullscreen_sibling"
                    } else {
                        "grid_sibling"
                    }
                );
                let result = pending.rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert_eq!(result.target.as_ref().unwrap().logical_source(), next);
            }
            assert!(!app.top_level_grid_view.collection_navigation_pending());
        }
    }
}

#[test]
#[cfg(windows)]
fn section1763_independent_collection_root_sibling_ignores_main_filters() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Image);
    app.navigation_scope = ViewerNavigationScope::Main;
    app.show_search_bar = true;
    app.favsearch.active = true;
    let main_items = app.items.clone();
    let active = app.build_active_context_for_test(Some(97633), DetachedSource::Book, |owner| {
        owner.navigation_scope = ViewerNavigationScope::CollectionRoot;
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        owner.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        owner.fullscreen_idx = Some(1);
    });
    let ctx = egui::Context::default();
    app.with_viewer_context(active, |owner| {
        assert!(
            owner
                .page_overview_executable_actions()
                .contains(&crate::keymap::KeyAction::FsSiblingNext)
        );
        owner.handle_fullscreen_sibling_nav_context(&ctx, 1, true, false);
        pending_action(owner);
        drain(owner, &ctx);
        assert_eq!(
            owner.effective_folder().as_ref(),
            Some(&prepared.entries[2].source_path)
        );
    })
    .unwrap();
    assert_eq!(app.items, main_items);
    assert!(!app.top_level_grid_view.collection_navigation_pending());
    app.shutdown_collection_runtime_for_exit();
}

#[test]
#[cfg(windows)]
fn section1763_detached_collection_sibling_only_mutates_its_owner() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (_temp, mut app, prepared) = fixture(CollectionResolvedKind::Folder);
    let ctx = egui::Context::default();
    let sibling = app.push_window_context_for_test(&ctx, 97631, |owner| {
        owner.items = vec![GridItem::Image("C:/unrelated/page.png".into())];
        owner.fs_nav_locked_gen = Some(owner.items_generation);
    });
    let sibling_before = app
        .with_viewer_context(sibling, |owner| {
            (
                owner.items.clone(),
                owner.items_generation,
                owner.fs_nav_locked_gen,
                owner.folder_nav_pending.is_some(),
                owner.top_level_grid_view.collection_navigation_pending(),
            )
        })
        .unwrap();
    let main_before = (
        app.items.clone(),
        app.items_generation,
        app.current_folder.clone(),
        app.folder_nav_pending.is_some(),
        app.top_level_grid_view.collection_navigation_pending(),
    );
    // These filters belong to main, not to the independent collection book.
    app.show_search_bar = true;
    app.favsearch.active = true;
    let entry = &prepared.entries[1];
    let active = app.build_active_context_for_test(Some(97632), DetachedSource::Book, |owner| {
        owner.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.top_level_grid_view.begin(
            TopLevelGridSurface::Folder,
            Some(TopLevelGridRestore::Collection(
                super::super::top_level_grid_view::CollectionGridRestore {
                    identity: CollectionGridIdentity {
                        collection_id: prepared.collection_id,
                    },
                    revision_at_open: prepared.collection_revision,
                    viewport_anchor: Some(CollectionGridViewportAnchor {
                        entry_id: entry.entry_id,
                        source_key: entry.source_key.clone(),
                    }),
                    return_to: None,
                },
            )),
        );
        owner.current_folder = Some(entry.source_path.clone());
        owner.items = vec![GridItem::Image(entry.source_path.join("page.png"))];
        owner.visible_indices = vec![0];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.image_metas = vec![None];
        owner.fullscreen_idx = Some(0);
    });
    app.with_viewer_context(active, |owner| {
        assert!(
            owner
                .page_overview_executable_actions()
                .contains(&crate::keymap::KeyAction::FsSiblingNext)
        );
        owner.handle_fullscreen_sibling_nav_context(&ctx, 0, true, false);
        assert!(matches!(
            pending_action(owner),
            CollectionNavigationAction::OuterFullscreen { forward: true, .. }
        ));
        drain(owner, &ctx);
        assert_eq!(
            owner.effective_folder().as_ref(),
            Some(&prepared.entries[2].source_path)
        );
    })
    .unwrap();
    assert_eq!(
        (
            app.items.clone(),
            app.items_generation,
            app.current_folder.clone(),
            app.folder_nav_pending.is_some(),
            app.top_level_grid_view.collection_navigation_pending()
        ),
        main_before
    );
    assert_eq!(
        app.with_viewer_context(sibling, |owner| (
            owner.items.clone(),
            owner.items_generation,
            owner.fs_nav_locked_gen,
            owner.folder_nav_pending.is_some(),
            owner.top_level_grid_view.collection_navigation_pending()
        ))
        .unwrap(),
        sibling_before
    );
    app.shutdown_collection_runtime_for_exit();
}
