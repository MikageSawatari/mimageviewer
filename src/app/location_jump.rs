//! Location inputs carry only their destination. Search ownership and admission
//! are resolved when the winning navigation is dispatched, after search widgets.

use super::{
    App, DirectNavigationPurpose, FolderNavHistoryTarget, MainHistoryOperation,
    MainListRequestStage, OpenRequestOwner, ReadingHistoryEntrance, StartupListIntent,
};
use crate::ui_dialogs::context_menu::{
    JumpToFolderDestination, JumpToFolderRequest, JumpToFolderSelection,
};
use crate::ui_main::{AddressBarNav, LocationJumpTarget};

impl App {
    pub(super) fn location_jump_snapshot_blocked(&mut self) -> bool {
        if self.is_snapshot_active() {
            self.show_feedback_warning(
                "スナップショット中は他のフォルダに移動できません".to_owned(),
            );
            true
        } else {
            false
        }
    }

    pub(crate) fn location_jump_nav(
        &mut self,
        ctx: &egui::Context,
        target: LocationJumpTarget,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        // Keep the early fixed-list refusal before closing a fullscreen viewer.
        // The same check runs again at dispatch because this is only a candidate.
        if self.location_jump_snapshot_blocked() {
            return None;
        }
        if self.projected_viewer_session_open() {
            self.close_fullscreen();
        }
        self.bump_input_seq(source, Some(&format!("location_jump={target:?}")));
        ctx.request_repaint();
        Some(AddressBarNav::LocationJump(target))
    }

    pub(crate) fn dispatch_location_jump(&mut self, target: LocationJumpTarget) {
        if self.location_jump_snapshot_blocked()
            || self.main_chrome_pointer_blocked()
            || self.document_open_modal_admission_blocked()
        {
            return;
        }
        if !(self.global_search.active || self.favsearch.active || self.tag_view.active) {
            // Preserve each destination's released plain-navigation entry point.
            match target {
                LocationJumpTarget::Physical(path) => self.open_direct_navigation_target_at_stage(
                    path,
                    None,
                    OpenRequestOwner::Navigation,
                    None,
                    None,
                    StartupListIntent::ExplicitList,
                    MainListRequestStage::Admission,
                ),
                LocationJumpTarget::DriveList => self.enter_drive_list_from_navigation(None),
                LocationJumpTarget::ReadingHistory => {
                    self.enter_reading_history_with_entrance(ReadingHistoryEntrance::Menu);
                }
                LocationJumpTarget::BooksRoot => self.open_books_root(),
                LocationJumpTarget::Rating(stars) => self.enter_rating_view(stars),
            }
            return;
        }

        let history = MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
            selection: JumpToFolderSelection::None,
        });
        match target {
            LocationJumpTarget::Physical(path) => self.begin_location_jump_to_physical(path),
            LocationJumpTarget::BooksRoot => {
                if let Some(root) = self.prepare_books_root_for_navigation() {
                    self.begin_location_jump_to_physical(root);
                }
            }
            LocationJumpTarget::DriveList => {
                self.adopt_synthetic_surface(
                    FolderNavHistoryTarget::Path(super::drive_list_synthetic_path()),
                    history,
                    Some(None),
                );
            }
            LocationJumpTarget::ReadingHistory => {
                self.enter_reading_history_with_history(ReadingHistoryEntrance::Menu, history)
            }
            LocationJumpTarget::Rating(stars) => {
                self.enter_rating_view_with_history(stars, history)
            }
        }
    }

    fn begin_location_jump_to_physical(&mut self, path: std::path::PathBuf) {
        // The producers already resolve existing destinations. Extension classification
        // adds no stat/read to the UI path, and matches the explicit archive-jump route.
        let archive = crate::folder_tree::is_virtual_folder(&path)
            || Self::path_needs_open_classification(&path);
        let destination = if archive {
            JumpToFolderDestination::ArchiveContainer(path)
        } else {
            JumpToFolderDestination::PhysicalDirectory(path)
        };
        self.begin_context_jump_to_folder(JumpToFolderRequest {
            destination,
            selection: JumpToFolderSelection::None,
            origin: None,
        });
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
    use crate::app::top_level_grid_view::{
        TopLevelGridRestore, TopLevelGridSurface, TopLevelSearchView,
    };
    use crate::grid_item::GridItem;
    use std::path::PathBuf;

    fn search_fixture(kind: usize) -> AppTestEnv {
        let mut app = setup_app();
        let origin = app.tmp.path().join("pre-search");
        let row = app.tmp.path().join("result.zip");
        if kind == 1 {
            std::fs::create_dir(&origin).unwrap();
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Folder);
            crate::global_search_ui::adopt_everything_for_test(
                &mut app,
                &egui::Context::default(),
                "location-jump",
                crate::global_search_ui::everything_dataset_for_test(vec![GridItem::ZipFile(row)]),
            );
            assert!(matches!(
                app.global_search.accepted,
                crate::global_search_ui::SearchAccepted::Everything { .. }
            ));
            assert!(
                matches!(app.top_level_grid_view.return_to(), Some(TopLevelGridRestore::Folder(path)) if path == &origin)
            );
            assert_eq!(app.global_search.saved_folder.as_ref(), Some(&origin));
            app.selected = Some(0);
            return app;
        }
        app.install_new_items(vec![GridItem::ZipFile(row)], vec![None]);
        app.visible_indices = vec![0];
        app.selected = Some(0);
        app.current_folder = Some(super::super::search_results_synthetic_path());
        let search = match kind {
            0 => {
                app.global_search.active = true;
                app.global_search.saved_folder = Some(origin.clone());
                app.items_are_global_search_view = true;
                TopLevelSearchView::Global
            }
            2 => {
                app.favsearch.active = true;
                app.favsearch.saved_folder = Some(origin.clone());
                TopLevelSearchView::Favorite
            }
            _ => {
                app.tag_view.active = true;
                app.tag_view.saved_folder = Some(origin.clone());
                app.items_are_tag_view = true;
                TopLevelSearchView::Tag
            }
        };
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Search(search),
            Some(TopLevelGridRestore::Folder(origin)),
        );
        app
    }

    fn assert_search_active(app: &App, kind: usize) {
        assert!(match kind {
            0 => app.global_search.active,
            1 =>
                app.global_search.active
                    && matches!(
                        app.global_search.accepted,
                        crate::global_search_ui::SearchAccepted::Everything { .. }
                    ),
            2 => app.favsearch.active,
            _ => app.tag_view.active,
        });
    }

    fn assert_search_dismissed_with_history(app: &App, origin: PathBuf) {
        assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        assert_eq!(
            app.folder_history_back_entry(),
            Some(&FolderNavHistoryTarget::Path(origin).into())
        );
    }

    #[test]
    fn location_jump_drive_list_exits_each_search_and_records_pre_search_origin() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            app.dispatch_location_jump(LocationJumpTarget::DriveList);
            assert!(app.items_are_drive_list);
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    #[test]
    fn location_jump_reading_history_exits_each_search_and_records_pre_search_origin() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            app.dispatch_location_jump(LocationJumpTarget::ReadingHistory);
            assert!(app.items_are_reading_history_view);
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    #[test]
    fn location_jump_physical_adopts_success_only_for_each_search() {
        for kind in 0..4 {
            for success in [false, true] {
                let mut app = search_fixture(kind);
                let origin = app.tmp.path().join("pre-search");
                let destination = app.tmp.path().join("destination");
                std::fs::create_dir(&destination).unwrap();
                app.dispatch_location_jump(LocationJumpTarget::Physical(destination.clone()));
                assert_search_active(&app, kind);
                assert_eq!(app.selected, Some(0));
                let pending = app
                    .folder_pane_open_pending
                    .take()
                    .expect("explicit jump scan");
                pending
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                assert!(matches!(
                    &pending.navigation.as_ref().unwrap().history,
                    MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView { .. })
                ));
                let scan = if success {
                    Ok(super::super::scan_directory(&destination))
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "denied",
                    ))
                };
                let ready = super::super::FolderPaneOpenReady {
                    navigation: pending.navigation,
                    epub_restore: pending.epub_restore,
                    path: pending.path,
                    purpose: pending.purpose,
                    scan,
                };
                assert!(
                    app.resolve_main_folder_open_ready(&egui::Context::default(), ready)
                        .is_none()
                );
                if success {
                    assert_search_dismissed_with_history(&app, origin);
                    assert_eq!(app.current_folder.as_ref(), Some(&destination));
                } else {
                    assert_search_active(&app, kind);
                    assert_eq!(app.items.len(), 1);
                    assert_eq!(app.selected, Some(0));
                    assert!(app.fs_feedback_toast.is_some());
                }
            }
        }
    }

    #[test]
    fn location_jump_plain_candidate_rechecks_search_started_before_dispatch() {
        let ctx = egui::Context::default();
        for target in [
            LocationJumpTarget::DriveList,
            LocationJumpTarget::ReadingHistory,
            LocationJumpTarget::BooksRoot,
            LocationJumpTarget::Rating(3),
            LocationJumpTarget::Physical(PathBuf::from(r"C:\destination")),
        ] {
            let mut app = setup_app();
            let nav = app.location_jump_nav(&ctx, target, "test").unwrap();
            // This is the same ordering as a key candidate followed by a toolbar send.
            let cancel =
                crate::global_search_ui::start_everything_for_test(&mut app, &ctx, "query");
            assert!(app.apply_fullscreen_close_nav_immediate(nav));
            assert!(app.global_search.active);
            assert!(app.global_search.pending.is_some());
            assert!(!cancel.is_cancelled());
            assert!(!app.items_are_drive_list);
            assert!(!app.items_are_reading_history_view);
            assert!(app.rating_view_pending.is_none());
            assert!(app.folder_pane_open_pending.is_none());
        }
    }

    #[test]
    fn location_jump_rating_keeps_exit_purpose_on_its_staged_request() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            app.dispatch_location_jump(LocationJumpTarget::Rating(3));
            assert_search_active(&app, kind);
            assert!(matches!(
                app.rating_view_pending
                    .as_ref()
                    .and_then(|build| build.navigation.as_ref())
                    .map(|transition| &transition.navigation.history),
                Some(MainHistoryOperation::Direct(
                    DirectNavigationPurpose::ExitTemporaryView { .. }
                ))
            ));
        }
    }

    fn complete_physical_jump(app: &mut App, destination: &std::path::Path) {
        let pending = app
            .folder_pane_open_pending
            .take()
            .expect("physical jump scan");
        complete_physical_jump_request(app, pending, destination);
    }

    fn complete_physical_jump_request(
        app: &mut App,
        pending: super::super::FolderPaneOpenPending,
        destination: &std::path::Path,
    ) {
        pending
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let ready = super::super::FolderPaneOpenReady {
            navigation: pending.navigation,
            epub_restore: pending.epub_restore,
            path: pending.path,
            purpose: pending.purpose,
            scan: Ok(super::super::scan_directory(destination)),
        };
        assert!(
            app.resolve_main_folder_open_ready(&egui::Context::default(), ready)
                .is_none()
        );
    }

    #[test]
    fn location_jump_adoption_retires_each_search_producer_before_late_completion() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let destination = app.tmp.path().join("destination");
            std::fs::create_dir(&destination).unwrap();
            app.dispatch_location_jump(LocationJumpTarget::Physical(destination.clone()));
            let assert_retired: Box<dyn FnOnce()> = match kind {
                0 => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (tx, rx) = crossbeam_channel::unbounded();
                    app.global_search.pending =
                        Some(crate::global_search_ui::SearchProducer::Metadata(
                            crate::indexer_manager::SearchHandle {
                                cancel: cancel.clone(),
                                rx,
                            },
                        ));
                    Box::new(move || {
                        assert!(cancel.load(Ordering::SeqCst));
                        assert!(
                            tx.send(crate::global_search::SearchStreamEvent::Error(
                                "late".into()
                            ))
                            .is_err()
                        );
                    })
                }
                1 => {
                    let cancel = crate::global_search_ui::start_everything_for_test(
                        &mut app,
                        &egui::Context::default(),
                        "location-jump",
                    );
                    Box::new(move || assert!(cancel.is_cancelled()))
                }
                2 => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (tx, rx) = mpsc::channel();
                    app.favsearch_pending = Some(super::super::FavSearchPending {
                        cancel: cancel.clone(),
                        rx,
                    });
                    Box::new(move || {
                        assert!(cancel.load(Ordering::Relaxed));
                        assert!(tx.send(Err(rusqlite::Error::InvalidQuery)).is_err());
                    })
                }
                _ => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (tx, rx) = mpsc::channel();
                    app.tag_view_pending = Some(crate::tag_view::TagViewPending::for_test(
                        cancel.clone(),
                        rx,
                    ));
                    Box::new(move || {
                        assert!(cancel.load(Ordering::Relaxed));
                        assert!(tx.send(Err("late".into())).is_err());
                    })
                }
            };
            complete_physical_jump(&mut app, &destination);
            assert_retired();
            app.poll_global_search_events(&egui::Context::default());
            app.poll_favsearch();
            app.poll_tag_view();
            assert_eq!(app.current_folder.as_ref(), Some(&destination));
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        }
    }

    #[test]
    fn location_jump_same_query_refresh_adopts_but_new_query_rejects_old_scan() {
        for kind in 0..4 {
            for new_query in [false, true] {
                let mut app = search_fixture(kind);
                let source_folder = app.current_folder.clone();
                let destination = app.tmp.path().join("destination");
                std::fs::create_dir(&destination).unwrap();
                app.dispatch_location_jump(LocationJumpTarget::Physical(destination.clone()));
                let pending = app.folder_pane_open_pending.take().unwrap();
                if new_query {
                    // A new submission uses the existing replacement boundary. Keep a saved
                    // completed scan to exercise the stale reply rather than only cancellation.
                    app.retire_main_list_requests_for_surface_switch();
                    match kind {
                        0 => app.global_search.metadata_draft = "replacement".into(),
                        1 => app.global_search.everything_draft = "replacement".into(),
                        2 => app.favsearch.query = "replacement".into(),
                        _ => app.tag_view.query = "replacement".into(),
                    }
                }
                // Same-query row generations deliberately do not invalidate copied destinations.
                app.items_generation = app.items_generation.wrapping_add(1);
                assert_eq!(
                    app.main_list_navigation_is_current(pending.navigation.as_ref().unwrap()),
                    !new_query
                );
                complete_physical_jump_request(&mut app, pending, &destination);
                if new_query {
                    assert_search_active(&app, kind);
                    assert_eq!(app.items.len(), 1);
                    assert_eq!(app.selected, Some(0));
                    assert_eq!(app.current_folder, source_folder);
                } else {
                    assert_eq!(app.current_folder.as_ref(), Some(&destination));
                    let origin = app.tmp.path().join("pre-search");
                    assert_search_dismissed_with_history(&app, origin);
                }
            }
        }
    }

    #[test]
    fn location_jump_dispatch_rechecks_delete_confirmation_for_each_search() {
        for kind in 0..4 {
            assert_location_jump_delete_race_is_blocked(search_fixture(kind));
        }
    }

    #[test]
    fn location_jump_plain_dispatch_rechecks_delete_confirmation() {
        let mut app = setup_app();
        let origin = app.tmp.path().join("plain-origin");
        let row = origin.join("selected.zip");
        std::fs::create_dir(&origin).unwrap();
        app.current_folder = Some(origin);
        app.top_level_grid_view
            .replace_surface(TopLevelGridSurface::Folder);
        app.install_new_items(vec![GridItem::ZipFile(row)], vec![None]);
        app.visible_indices = vec![0];
        app.selected = Some(0);
        assert_location_jump_delete_race_is_blocked(app);
    }

    fn assert_location_jump_delete_race_is_blocked(mut app: AppTestEnv) {
        use super::super::update_phases::MainFrameNavigation;
        let ctx = egui::Context::default();
        let physical = app.tmp.path().join("blocked-destination");
        std::fs::create_dir(&physical).unwrap();
        let shelf = app.tmp.path().join("blocked-shelf");
        app.settings.book_root = Some(shelf.clone());
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        assert!(!app.main_chrome_pointer_blocked());
        let mut candidates = Vec::new();
        for target in [
            LocationJumpTarget::DriveList,
            LocationJumpTarget::ReadingHistory,
            LocationJumpTarget::BooksRoot,
            LocationJumpTarget::Physical(physical),
            LocationJumpTarget::Rating(3),
        ] {
            for lane in 0..4 {
                let candidate = app
                    .location_jump_nav(&ctx, target.clone(), "delete-race")
                    .expect("candidate is generated before the delete dialog opens");
                candidates.push((lane, candidate));
            }
        }
        let current_folder = app.current_folder.clone();
        let surface = app.top_level_grid_view.surface().clone();
        let return_to = format!("{:?}", app.top_level_grid_view.return_to());
        let selected = app.selected;
        let checked = app.checked.clone();
        let generation = app.items_generation;
        let search_state = (
            app.global_search.active,
            app.favsearch.active,
            app.tag_view.active,
        );
        let search_origin = app.capture_search_open_origin();
        let history = app.folder_nav_history_snapshot();
        let selected_path = app.items[selected.unwrap()]
            .drag_source_path()
            .unwrap()
            .to_path_buf();
        let _owner = app.keyboard_owner_for_pass(&ctx);
        app.handle_delete_key(&ctx);
        assert!(app.show_delete_confirm);
        assert_eq!(app.modal_dialog_block_reason(), Some("delete_confirm"));
        let delete_targets = vec![(selected.unwrap(), selected_path)];
        assert_eq!(app.delete_targets, delete_targets);
        for (lane, candidate) in candidates {
            if lane == 3 {
                assert!(app.apply_fullscreen_close_nav_immediate(candidate));
            } else {
                let mut frame = MainFrameNavigation {
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
                };
                match lane {
                    0 => frame.keyboard_nav = Some(candidate),
                    1 => frame.address_nav = Some(candidate),
                    _ => frame.grid_nav = Some(candidate),
                }
                app.post_render_main_navigation(&ctx, frame);
            }
            assert_eq!(app.current_folder, current_folder);
            assert_eq!(app.top_level_grid_view.surface(), &surface);
            assert_eq!(
                format!("{:?}", app.top_level_grid_view.return_to()),
                return_to
            );
            assert_eq!(app.items_generation, generation);
            assert_eq!(app.selected, selected);
            assert_eq!(app.checked, checked);
            assert_eq!(app.folder_nav_history_snapshot(), history);
            assert_eq!(
                (
                    app.global_search.active,
                    app.favsearch.active,
                    app.tag_view.active
                ),
                search_state
            );
            if let Some(origin) = &search_origin {
                assert!(app.search_open_origin_is_current(origin));
            } else {
                assert!(app.capture_search_open_origin().is_none());
            }
            assert!(app.show_delete_confirm);
            assert_eq!(app.delete_targets, delete_targets);
            assert!(app.folder_pane_open_pending.is_none());
            assert!(
                app.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
            );
            assert!(app.rating_view_pending.is_none());
        }
        assert!(
            !shelf.exists(),
            "blocked BooksRoot must not create its directory"
        );
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn location_jump_dispatch_excludes_menus_from_the_modal_gate() {
        let mut app = search_fixture(0);
        let nav = app
            .location_jump_nav(
                &egui::Context::default(),
                LocationJumpTarget::DriveList,
                "test",
            )
            .unwrap();
        app.context_menu_idx = Some(app.capture_grid_context_menu_owner(0));
        assert!(app.common_modal_dialog_open());
        assert!(!app.main_chrome_pointer_blocked());
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        assert!(app.items_are_drive_list);
        let origin = app.tmp.path().join("pre-search");
        assert_search_dismissed_with_history(&app, origin);
    }

    #[test]
    fn location_jump_dispatch_rechecks_snapshot_and_keeps_fullscreen_on_early_refusal() {
        let mut app = search_fixture(0);
        let ctx = egui::Context::default();
        let nav = app
            .location_jump_nav(&ctx, LocationJumpTarget::DriveList, "test")
            .unwrap();
        app.toggle_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        // Pinning itself ends search by its existing mutual-exclusion contract.
        assert!(!app.global_search.active);
        let pin = app.snapshot.as_ref().unwrap();
        let pinned_identity = (pin.generation_id, pin.origin.clone(), pin.items.len());
        let current_folder = app.current_folder.clone();
        let selected = app.selected;
        let history = app.folder_nav_history_snapshot();
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        assert!(!app.items_are_drive_list);
        assert!(app.is_snapshot_active());
        assert!(!app.global_search.active);
        let pin = app.snapshot.as_ref().unwrap();
        assert_eq!(
            (pin.generation_id, pin.origin.clone(), pin.items.len()),
            pinned_identity
        );
        assert_eq!(app.current_folder, current_folder);
        assert_eq!(app.selected, selected);
        assert_eq!(app.folder_nav_history_snapshot(), history);
        app.fullscreen_idx = Some(0);
        assert!(
            app.location_jump_nav(&ctx, LocationJumpTarget::DriveList, "test")
                .is_none()
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.fs_feedback_toast.is_some());
    }

    #[test]
    fn location_jump_cbz_uses_archive_route_instead_of_directory_scan() {
        let mut app = search_fixture(0);
        let archive = app.tmp.path().join("book.cbz");
        let file = std::fs::File::create(&archive).unwrap();
        zip::ZipWriter::new(file).finish().unwrap();
        app.dispatch_location_jump(LocationJumpTarget::Physical(archive));
        assert!(
            app.folder_pane_open_pending.is_none(),
            "CBZ must use the archive open route"
        );
    }

    #[test]
    fn location_jump_frame_input_address_and_grid_lanes_share_dispatch() {
        use super::super::update_phases::MainFrameNavigation;
        for kind in 0..4 {
            for lane in 0..3 {
                let mut app = search_fixture(kind);
                let nav = AddressBarNav::LocationJump(LocationJumpTarget::DriveList);
                let mut frame = MainFrameNavigation {
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
                };
                match lane {
                    0 => frame.keyboard_nav = Some(nav),
                    1 => frame.address_nav = Some(nav),
                    _ => frame.grid_nav = Some(nav),
                }
                app.post_render_main_navigation(&egui::Context::default(), frame);
                assert!(app.items_are_drive_list, "kind={kind} lane={lane}");
                let origin = app.tmp.path().join("pre-search");
                assert_search_dismissed_with_history(&app, origin);
            }
        }
    }

    #[test]
    fn location_jump_everything_physical_child_exits_to_drive_list_with_pre_search_history() {
        let mut app = search_fixture(1);
        let row = app.tmp.path().join("result.zip");
        let origin = app.capture_search_open_origin();
        app.commit_staged_search_navigation_effects(&row, false, false, origin);
        assert!(matches!(&app.global_search.accepted,
            crate::global_search_ui::SearchAccepted::Everything {
                position: crate::global_search_ui::EverythingPosition::PhysicalChild { logical_source, .. }, ..
            } if logical_source == &row));
        app.current_folder = Some(row);
        app.items_are_global_search_view = false;
        app.dispatch_location_jump(LocationJumpTarget::DriveList);
        assert!(app.items_are_drive_list);
        let pre_search = app.tmp.path().join("pre-search");
        assert_search_dismissed_with_history(&app, pre_search);
    }

    #[test]
    fn location_jump_plain_candidate_uses_search_started_before_nonmodal_dispatch() {
        for favorite in [false, true] {
            let mut app = setup_app();
            let origin = app.tmp.path().join("pre-search");
            std::fs::create_dir(&origin).unwrap();
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Folder);
            let nav = app
                .location_jump_nav(
                    &egui::Context::default(),
                    LocationJumpTarget::DriveList,
                    "test",
                )
                .unwrap();
            if favorite {
                app.open_favsearch();
            } else {
                app.open_global_search();
            }
            assert!(app.global_search.active || app.favsearch.active);
            assert!(app.apply_fullscreen_close_nav_immediate(nav));
            assert!(app.items_are_drive_list);
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    fn settle_location_jump(app: &mut App, ready: impl Fn(&App) -> bool) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            app.poll_rating_view();
            app.poll_sidecar_restore(&ctx);
            app.post_render_main_navigation(
                &ctx,
                super::super::update_phases::MainFrameNavigation {
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
                },
            );
            if ready(app)
                && app.rating_view_pending.is_none()
                && app.folder_pane_open_pending.is_none()
                && !app.sidecar_restore_active()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "location jump did not settle"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn location_jump_books_root_exits_each_search_after_successful_scan() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            let shelf = app.tmp.path().join("location-shelf");
            app.settings.book_root = Some(shelf.clone());
            assert!(!shelf.exists());
            app.dispatch_location_jump(LocationJumpTarget::BooksRoot);
            assert!(
                shelf.is_dir(),
                "the existing books-root entry creates its directory"
            );
            assert_search_active(&app, kind);
            complete_physical_jump(&mut app, &shelf);
            assert_eq!(app.current_folder.as_ref(), Some(&shelf));
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    #[test]
    fn location_jump_rating_exits_each_search_after_successful_worker_adoption() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            app.dispatch_location_jump(LocationJumpTarget::Rating(3));
            assert_search_active(&app, kind);
            settle_location_jump(&mut app, |app| app.items_are_rating_view);
            assert_eq!(app.rating_view_stars, 3);
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    #[test]
    fn location_jump_plain_dispatch_preserves_each_destination_and_history() {
        for destination in 0..5 {
            let mut app = setup_app();
            let origin = app.tmp.path().join("plain-origin");
            let physical = app.tmp.path().join("plain-destination");
            let shelf = app.tmp.path().join("plain-shelf");
            std::fs::create_dir(&origin).unwrap();
            std::fs::create_dir(&physical).unwrap();
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Folder);
            app.settings.book_root = Some(shelf.clone());
            let target = match destination {
                0 => LocationJumpTarget::DriveList,
                1 => LocationJumpTarget::ReadingHistory,
                2 => LocationJumpTarget::BooksRoot,
                3 => LocationJumpTarget::Physical(physical.clone()),
                _ => LocationJumpTarget::Rating(3),
            };
            app.dispatch_location_jump(target);
            settle_location_jump(&mut app, |app| match destination {
                0 => app.items_are_drive_list,
                1 => app.items_are_reading_history_view,
                2 => app.current_folder.as_ref() == Some(&shelf),
                3 => app.current_folder.as_ref() == Some(&physical),
                _ => app.items_are_rating_view && app.rating_view_stars == 3,
            });
            assert_search_dismissed_with_history(&app, origin);
        }
    }
}
