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
    /// The manager owns this button's admission. Keep that dialog open while preparing
    /// the destination; its own modal is not a competing navigation blocker.
    pub(crate) fn open_book_manager_target(&mut self, path: std::path::PathBuf) {
        if self.global_search.active || self.favsearch.active || self.tag_view.active {
            if !self.location_jump_snapshot_blocked() {
                if crate::folder_tree::path_eq(&path, &self.book_root_path()) {
                    if let Some(root) = self.prepare_books_root_for_navigation() {
                        self.begin_location_jump_to_physical(root);
                    }
                } else {
                    self.begin_location_jump_to_physical(path);
                }
            }
        } else if crate::folder_tree::path_eq(&path, &self.book_root_path()) {
            self.open_books_root();
        } else {
            self.load_folder(path);
        }
    }

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

    pub(super) fn location_jump_admission_blocked(&mut self) -> bool {
        self.location_jump_snapshot_blocked()
            || self.main_chrome_pointer_blocked()
            || self.document_open_modal_admission_blocked()
    }

    pub(crate) fn dispatch_address_input(&mut self, target: LocationJumpTarget) {
        if self.global_search.active || self.favsearch.active || self.tag_view.active {
            self.dispatch_location_jump(target);
        } else {
            // Address Enter keeps its released Direct / DriveList admission outside search.
            self.dispatch_plain_location_jump(target);
        }
    }

    /// Search favorites are consumed here; Plain paths continue through the caller's
    /// existing Direct consumer (including the native/immediate loader and cleanup).
    pub(crate) fn dispatch_favorite_navigation(
        &mut self,
        path: std::path::PathBuf,
    ) -> Option<std::path::PathBuf> {
        if self.global_search.active || self.favsearch.active || self.tag_view.active {
            self.dispatch_location_jump(LocationJumpTarget::Physical(path));
            None
        } else {
            Some(path)
        }
    }

    pub(crate) fn dispatch_location_jump(&mut self, target: LocationJumpTarget) {
        if self.location_jump_admission_blocked() {
            return;
        }
        if !(self.global_search.active || self.favsearch.active || self.tag_view.active) {
            self.dispatch_plain_location_jump(target);
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

    fn dispatch_plain_location_jump(&mut self, target: LocationJumpTarget) {
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
        let delete_targets = vec![
            crate::ui_dialogs::context_menu::DeleteTarget::from_grid_item(
                &app.items[selected.unwrap()],
            )
            .unwrap(),
        ];
        let _owner = app.keyboard_owner_for_pass(&ctx);
        app.handle_delete_key(&ctx);
        assert!(app.show_delete_confirm);
        assert_eq!(app.modal_dialog_block_reason(), Some("delete_confirm"));
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

    fn toolbar_favorite_frame(path: PathBuf) -> super::super::update_phases::MainFrameNavigation {
        super::super::update_phases::MainFrameNavigation {
            fav_nav: None,
            toolbar_fav_nav: Some(path),
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

    fn finish_toolbar_jump_and_go_back(
        app: &mut App,
        target: &std::path::Path,
        origin: &std::path::Path,
    ) {
        complete_physical_jump(app, target);
        finish_admitted_user_jump_and_go_back(app, target, origin);
    }

    fn finish_admitted_user_jump_and_go_back(
        app: &mut App,
        target: &std::path::Path,
        origin: &std::path::Path,
    ) {
        settle_location_jump(app, |app| app.current_folder.as_deref() == Some(target));
        assert_search_dismissed_with_history(app, origin.to_path_buf());
        let ctx = egui::Context::default();
        let mut back = toolbar_favorite_frame(target.to_path_buf());
        back.toolbar_fav_nav = None;
        back.keyboard_nav = Some(AddressBarNav::HistoryBack);
        app.post_render_main_navigation(&ctx, back);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while app.current_folder.as_deref() != Some(origin)
            || app
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
            || app.sidecar_restore_active()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "toolbar favorite history Back did not adopt"
            );
            app.settle_open_path_classification_for_test();
            app.poll_history_navigation_transition_for_test(&ctx);
            app.poll_sidecar_restore(&ctx);
            std::thread::yield_now();
        }
        assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        assert_eq!(
            app.folder_history_forward_target(),
            Some(&FolderNavHistoryTarget::Path(target.to_path_buf()))
        );
    }

    // The UI test module cannot call an app-private frame phase. This bridge is
    // test-only and exercises the same toolbar lane, scan adoption and Back handler.
    impl App {
        pub(crate) fn navigation_search_fixture_for_test(kind: usize) -> AppTestEnv {
            search_fixture(kind)
        }

        pub(crate) fn verify_admitted_user_navigation_for_test(
            &mut self,
            target: &std::path::Path,
            origin: &std::path::Path,
        ) {
            if self.folder_pane_open_pending.is_some() {
                complete_physical_jump(self, target);
            }
            assert_eq!(
                self.current_folder.as_deref(),
                Some(target),
                "user navigation must admit its actual requested path before Back; search=({},{},{}) pending={:?}",
                self.global_search.active,
                self.favsearch.active,
                self.tag_view.active,
                self.folder_pane_open_pending
                    .as_ref()
                    .map(|pending| &pending.path)
            );
            finish_admitted_user_jump_and_go_back(self, target, origin);
        }

        pub(crate) fn verify_failed_address_input_navigation_for_test(
            &mut self,
            candidate: AddressBarNav,
            kind: usize,
        ) {
            let expected = user_navigation_source_state(self);
            let mut frame = toolbar_favorite_frame(PathBuf::new());
            frame.toolbar_fav_nav = None;
            frame.address_nav = Some(candidate);
            self.post_render_main_navigation(&egui::Context::default(), frame);
            let pending = self
                .folder_pane_open_pending
                .take()
                .expect("actual address candidate starts a scan");
            let scan = pending
                .rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("vanished address scan reply");
            assert!(
                scan.is_err(),
                "the candidate's resolved directory vanished after Enter"
            );
            let ready = super::super::FolderPaneOpenReady {
                navigation: pending.navigation,
                epub_restore: pending.epub_restore,
                path: pending.path,
                purpose: pending.purpose,
                scan,
            };
            assert!(
                self.resolve_main_folder_open_ready(&egui::Context::default(), ready)
                    .is_none()
            );
            assert_search_active(self, kind);
            assert_eq!(user_navigation_source_state(self), expected);
            assert!(self.fs_feedback_toast.is_some());
        }

        pub(crate) fn verify_address_input_navigation_for_test(
            &mut self,
            candidate: AddressBarNav,
            origin: &std::path::Path,
        ) {
            let target = match &candidate {
                AddressBarNav::AddressInput(LocationJumpTarget::Physical(path)) => path.clone(),
                AddressBarNav::AddressInput(LocationJumpTarget::DriveList) => {
                    super::super::drive_list_synthetic_path()
                }
                _ => panic!("expected an address Enter candidate"),
            };
            let mut frame = toolbar_favorite_frame(target.clone());
            frame.toolbar_fav_nav = None;
            frame.address_nav = Some(candidate);
            self.post_render_main_navigation(&egui::Context::default(), frame);
            self.verify_admitted_user_navigation_for_test(&target, origin);
        }

        pub(crate) fn verify_toolbar_favorite_jump_for_test(
            &mut self,
            target: PathBuf,
            origin: &std::path::Path,
        ) {
            self.post_render_main_navigation(
                &egui::Context::default(),
                toolbar_favorite_frame(target.clone()),
            );
            assert!(self.folder_pane_open_pending.is_some());
            assert!(self.global_search.active || self.favsearch.active || self.tag_view.active);
            finish_toolbar_jump_and_go_back(self, &target, origin);
        }
    }

    #[test]
    fn toolbar_favorite_search_success_and_failures_preserve_the_owned_state_until_adoption() {
        for kind in 0..4 {
            for outcome in 0..3 {
                let mut app = search_fixture(kind);
                let origin = app.tmp.path().join("pre-search");
                std::fs::create_dir_all(&origin).unwrap();
                let target = app.tmp.path().join("toolbar-favorite");
                if outcome != 1 {
                    std::fs::create_dir(&target).unwrap();
                }
                let source_folder = app.current_folder.clone();
                let source_address = app.address.clone();
                let selected = app.selected;
                let generation = app.items_generation;
                let history = app.folder_nav_history_snapshot();
                let drafts = (
                    app.global_search.metadata_draft.clone(),
                    app.global_search.everything_draft.clone(),
                    app.favsearch.query.clone(),
                    app.tag_view.query.clone(),
                );
                app.post_render_main_navigation(
                    &egui::Context::default(),
                    toolbar_favorite_frame(target.clone()),
                );
                assert_search_active(&app, kind);
                assert_eq!(app.current_folder, source_folder);
                assert_eq!(app.selected, selected);
                assert_eq!(app.folder_nav_history_snapshot(), history);
                if outcome == 0 {
                    finish_toolbar_jump_and_go_back(&mut app, &target, &origin);
                } else {
                    let pending = app
                        .folder_pane_open_pending
                        .take()
                        .expect("toolbar explicit jump scan");
                    let scan = if outcome == 1 {
                        let result = pending
                            .rx
                            .recv_timeout(std::time::Duration::from_secs(30))
                            .expect("missing path scan reply");
                        assert!(result.is_err());
                        result
                    } else {
                        pending
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
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
                    assert_search_active(&app, kind);
                    assert_eq!(app.current_folder, source_folder);
                    assert_eq!(app.address, source_address);
                    assert_eq!(app.selected, selected);
                    assert_eq!(app.items_generation, generation);
                    assert_eq!(app.folder_nav_history_snapshot(), history);
                    assert_eq!(
                        (
                            app.global_search.metadata_draft.clone(),
                            app.global_search.everything_draft.clone(),
                            app.favsearch.query.clone(),
                            app.tag_view.query.clone()
                        ),
                        drafts
                    );
                    assert!(app.fs_feedback_toast.is_some());
                }
            }
        }
    }

    fn favorite_handler_candidate(
        app: &mut App,
        ctx: &egui::Context,
        entrance: usize,
    ) -> AddressBarNav {
        use crate::keymap::KeyAction;
        use crate::ring_shortcut::{
            GamepadFavoritePickerState, GamepadFavoritePickerTab, GamepadListCursor, RingActionId,
            RingShortcutContext,
        };
        match entrance {
            0..=2 => app.apply_location_navigation_key_action(
                ctx,
                [
                    KeyAction::GridOpenFavorite1,
                    KeyAction::GridFavoritePrev,
                    KeyAction::GridFavoriteNext,
                ][entrance],
                "favorite-test",
            ),
            3 => app.apply_ring_action(
                ctx,
                RingShortcutContext::Grid,
                RingActionId::OpenFavorite1,
                "favorite-test",
            ),
            _ => {
                app.gamepad_favorite_picker = Some(GamepadFavoritePickerState {
                    tab: GamepadFavoritePickerTab::Favorites,
                    favorites: GamepadListCursor::new(0),
                    smart_folders: GamepadListCursor::new(0),
                });
                app.confirm_gamepad_favorite_picker(ctx)
            }
        }
        .expect("registered favorite handler must produce a candidate")
    }

    #[test]
    fn favorite_key_ring_and_gamepad_handlers_exit_each_search_only_after_success() {
        for kind in 0..4 {
            for entrance in 0..5 {
                for fail in [false, true] {
                    let mut app = search_fixture(kind);
                    let origin = app.tmp.path().join("pre-search");
                    std::fs::create_dir_all(&origin).unwrap();
                    let target = app.tmp.path().join("handler-favorite");
                    if !fail {
                        std::fs::create_dir(&target).unwrap();
                    }
                    app.settings.favorites = vec![crate::settings::FavoriteEntry::new(
                        "test".into(),
                        target.clone(),
                    )];
                    let source = user_navigation_source_state(&app);
                    let ctx = egui::Context::default();
                    let candidate = favorite_handler_candidate(&mut app, &ctx, entrance);
                    assert!(matches!(&candidate, AddressBarNav::Favorite(path) if path == &target));
                    assert_search_active(&app, kind);
                    let mut frame = toolbar_favorite_frame(PathBuf::new());
                    frame.toolbar_fav_nav = None;
                    frame.gamepad_nav = Some(candidate);
                    app.post_render_main_navigation(&ctx, frame);
                    if fail {
                        settle_location_jump(&mut app, |app| {
                            app.folder_pane_open_pending.is_none()
                        });
                        assert_eq!(
                            user_navigation_source_state(&app),
                            source,
                            "kind={kind} entrance={entrance}"
                        );
                        assert_search_active(&app, kind);
                        assert!(app.fs_feedback_toast.is_some());
                    } else {
                        finish_toolbar_jump_and_go_back(&mut app, &target, &origin);
                    }
                }
            }
        }
    }

    #[test]
    fn favorite_typed_dispatch_rechecks_modal_snapshot_and_plain_immediate() {
        for kind in 0..4 {
            for block in 0..3 {
                let mut app = search_fixture(kind);
                let target = app.tmp.path().join("blocked-favorite");
                std::fs::create_dir(&target).unwrap();
                let candidate = AddressBarNav::Favorite(target);
                match block {
                    0 => app.show_delete_confirm = true,
                    1 => app.show_preferences = true,
                    _ => app.toggle_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed),
                }
                let source = user_navigation_source_state(&app);
                assert!(app.apply_fullscreen_close_nav_immediate(candidate));
                assert_eq!(user_navigation_source_state(&app), source);
                assert!(app.folder_pane_open_pending.is_none());
            }
        }
        let mut app = setup_app();
        let origin = app.tmp.path().join("plain-origin");
        let target = app.tmp.path().join("plain-favorite");
        std::fs::create_dir(&origin).unwrap();
        std::fs::create_dir(&target).unwrap();
        app.current_folder = Some(origin);
        app.top_level_grid_view
            .replace_surface(TopLevelGridSurface::Folder);
        assert!(app.apply_fullscreen_close_nav_immediate(AddressBarNav::Favorite(target.clone())));
        settle_location_jump(&mut app, |app| app.current_folder.as_ref() == Some(&target));
        assert!(app.folder_pane_open_pending.is_none());
    }

    #[test]
    fn toolbar_favorite_plain_path_keeps_the_existing_direct_navigation() {
        for toolbar in [false, true] {
            let mut app = setup_app();
            let origin = app.tmp.path().join("plain-origin");
            let target = app.tmp.path().join("plain-favorite");
            std::fs::create_dir(&origin).unwrap();
            std::fs::create_dir(&target).unwrap();
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Folder);
            let mut frame = toolbar_favorite_frame(target.clone());
            if !toolbar {
                frame.toolbar_fav_nav = None;
                frame.address_nav = Some(AddressBarNav::Direct(
                    target.clone(),
                    StartupListIntent::ExplicitList,
                ));
            }
            app.post_render_main_navigation(&egui::Context::default(), frame);
            settle_location_jump(&mut app, |app| app.current_folder.as_ref() == Some(&target));
            assert_search_dismissed_with_history(&app, origin);
            assert!(app.folder_pane_open_pending.is_none());
        }
    }

    #[test]
    fn toolbar_favorite_priority_keeps_menubar_then_toolbar_then_keyboard_ahead_of_folder_navigation()
     {
        for kind in 0..4 {
            for winner in 0..3 {
                let mut app = search_fixture(kind);
                let menu = app.tmp.path().join("menu-favorite");
                let toolbar = app.tmp.path().join("toolbar-favorite");
                let folder = app.tmp.path().join("folder-nav-loser");
                for path in [&menu, &toolbar, &folder] {
                    std::fs::create_dir(path).unwrap();
                }
                let (tx, rx) = std::sync::mpsc::channel();
                tx.send(super::super::FolderNavThreadResult {
                    target: Some(super::super::FolderNavTarget {
                        route: super::super::FolderNavRoute::FullFeature(folder.clone()),
                        smart_kind: None,
                        scanned: super::super::FolderNavScanResult::Ready(
                            super::super::scan_directory(&folder),
                        ),
                    }),
                    hit_image_folder: false,
                })
                .unwrap();
                app.folder_nav_pending = Some(super::super::FolderNavPending {
                    cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    rx,
                    forward: true,
                    mode: super::super::FolderNavMode::Grid,
                });
                let mut frame = toolbar_favorite_frame(toolbar.clone());
                frame.keyboard_nav =
                    Some(AddressBarNav::LocationJump(LocationJumpTarget::DriveList));
                if winner == 0 {
                    frame.fav_nav = Some(LocationJumpTarget::Physical(menu.clone()));
                }
                if winner == 2 {
                    frame.toolbar_fav_nav = None;
                }
                app.post_render_main_navigation(&egui::Context::default(), frame);
                match winner {
                    0 => {
                        complete_physical_jump(&mut app, &menu);
                        let origin = app.tmp.path().join("pre-search");
                        assert_search_dismissed_with_history(&app, origin);
                    }
                    1 => {
                        assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, toolbar);
                        complete_physical_jump(&mut app, &toolbar);
                        let origin = app.tmp.path().join("pre-search");
                        assert_search_dismissed_with_history(&app, origin);
                    }
                    _ => {
                        assert!(app.items_are_drive_list);
                        let origin = app.tmp.path().join("pre-search");
                        assert_search_dismissed_with_history(&app, origin);
                    }
                }
                assert_ne!(app.current_folder.as_ref(), Some(&folder));
            }
        }
    }

    #[test]
    fn toolbar_favorite_search_dispatch_rechecks_modal_and_everything_fetching() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let target = app.tmp.path().join("blocked-favorite");
            std::fs::create_dir(&target).unwrap();
            let current = app.current_folder.clone();
            let history = app.folder_nav_history_snapshot();
            app.show_delete_confirm = true;
            app.post_render_main_navigation(
                &egui::Context::default(),
                toolbar_favorite_frame(target),
            );
            assert_search_active(&app, kind);
            assert_eq!(app.current_folder, current);
            assert_eq!(app.selected, Some(0));
            assert_eq!(app.folder_nav_history_snapshot(), history);
            assert!(app.folder_pane_open_pending.is_none());
        }
        let mut app = search_fixture(1);
        let target = app.tmp.path().join("fetch-blocked-favorite");
        let cancel = crate::global_search_ui::start_everything_for_test(
            &mut app,
            &egui::Context::default(),
            "location-jump",
        );
        let current = app.current_folder.clone();
        let history = app.folder_nav_history_snapshot();
        app.post_render_main_navigation(&egui::Context::default(), toolbar_favorite_frame(target));
        assert!(app.everything_search_progress_visible());
        assert!(!cancel.is_cancelled());
        assert_eq!(app.current_folder, current);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.folder_nav_history_snapshot(), history);
        assert!(app.folder_pane_open_pending.is_none());
    }

    #[test]
    fn toolbar_favorite_pending_candidate_keeps_the_snapshot_that_replaced_search() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let target = app.tmp.path().join("outside-pin");
            std::fs::create_dir(&target).unwrap();
            // A toolbar path was already captured when pinning ends the search.
            let frame = toolbar_favorite_frame(target);
            app.toggle_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            assert!(app.is_snapshot_active());
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
            let current = app.current_folder.clone();
            let selected = app.selected;
            let generation = app.items_generation;
            let history = app.folder_nav_history_snapshot();
            let snapshot_generation = app.snapshot.as_ref().unwrap().generation_id;
            app.post_render_main_navigation(&egui::Context::default(), frame);
            assert!(app.is_snapshot_active());
            assert_eq!(
                app.snapshot.as_ref().unwrap().generation_id,
                snapshot_generation
            );
            assert_eq!(app.current_folder, current);
            assert_eq!(app.selected, selected);
            assert_eq!(app.items_generation, generation);
            assert_eq!(app.folder_nav_history_snapshot(), history);
            assert!(app.folder_pane_open_pending.is_none());
            assert!(app.fs_feedback_toast.is_some());
        }
    }

    fn user_navigation_source_state(
        app: &App,
    ) -> (
        Option<PathBuf>,
        String,
        Option<usize>,
        u64,
        super::super::FolderNavHistorySnapshot,
        [String; 4],
    ) {
        (
            app.current_folder.clone(),
            app.address.clone(),
            app.selected,
            app.items_generation,
            app.folder_nav_history_snapshot(),
            [
                app.global_search.metadata_draft.clone(),
                app.global_search.everything_draft.clone(),
                app.favsearch.query.clone(),
                app.tag_view.query.clone(),
            ],
        )
    }

    fn start_address_or_user_pane(app: &mut App, target: PathBuf, pane: bool) {
        if pane {
            app.start_user_folder_pane_open(target);
        } else {
            let mut frame = toolbar_favorite_frame(target.clone());
            frame.toolbar_fav_nav = None;
            frame.address_nav = Some(AddressBarNav::AddressInput(LocationJumpTarget::Physical(
                target,
            )));
            app.post_render_main_navigation(&egui::Context::default(), frame);
        }
    }

    #[test]
    fn address_input_and_user_pane_fail_cancel_and_stale_keep_the_current_search() {
        for kind in 0..4 {
            for pane in [false, true] {
                for outcome in 0..4 {
                    let mut app = search_fixture(kind);
                    let target = app.tmp.path().join("user-destination");
                    std::fs::create_dir(&target).unwrap();
                    // The destination existed when the address or cached tree row was chosen.
                    if outcome == 0 {
                        std::fs::remove_dir(&target).unwrap();
                    }
                    start_address_or_user_pane(&mut app, target.clone(), pane);
                    let pending = app
                        .folder_pane_open_pending
                        .as_ref()
                        .expect("user explicit scan");
                    assert!(matches!(
                        pending.purpose,
                        super::super::FolderOpenScanPurpose::JumpToPhysicalFolder {
                            selection: JumpToFolderSelection::None,
                            origin: None
                        }
                    ));
                    assert!(matches!(
                        &pending.navigation.as_ref().unwrap().history,
                        MainHistoryOperation::Direct(
                            DirectNavigationPurpose::ExitTemporaryView { .. }
                        )
                    ));
                    if outcome == 2 {
                        let cancel = pending.cancel.clone();
                        let expected = user_navigation_source_state(&app);
                        app.cancel_folder_pane_open(super::super::PaneOpenRestoreExit::Abandoned);
                        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
                        assert_eq!(user_navigation_source_state(&app), expected);
                    } else {
                        let pending = app.folder_pane_open_pending.take().unwrap();
                        if outcome == 3 {
                            app.retire_main_list_requests_for_surface_switch();
                            match kind {
                                0 => app.global_search.metadata_draft = "replacement".into(),
                                1 => app.global_search.everything_draft = "replacement".into(),
                                2 => app.favsearch.query = "replacement".into(),
                                _ => app.tag_view.query = "replacement".into(),
                            }
                        }
                        let expected = user_navigation_source_state(&app);
                        let scan = match outcome {
                            0 => {
                                let result = pending
                                    .rx
                                    .recv_timeout(std::time::Duration::from_secs(30))
                                    .expect("missing-directory scan reply");
                                assert!(result.is_err());
                                result
                            }
                            1 => Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                "denied",
                            )),
                            _ => Ok(super::super::scan_directory(&target)),
                        };
                        pending
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
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
                        assert_eq!(user_navigation_source_state(&app), expected);
                        if outcome < 2 {
                            assert!(app.fs_feedback_toast.is_some());
                        }
                    }
                    assert_search_active(&app, kind);
                    assert!(app.folder_pane_open_pending.is_none());
                }
            }
        }
    }

    #[test]
    fn user_pane_gamepad_search_entry_adopts_and_history_back_restores_pre_search() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let origin = app.tmp.path().join("pre-search");
            let target = app.tmp.path().join("gamepad-pane");
            std::fs::create_dir_all(&origin).unwrap();
            std::fs::create_dir(&target).unwrap();
            app.settings.folder_tree_pane_visible = true;
            let tree_root = app.tmp.path().to_path_buf();
            app.folder_pane =
                crate::folder_pane::FolderPaneState::test_tree(&tree_root, vec![target.clone()]);
            app.folder_pane.set_cursor(target.clone());
            assert!(
                app.handle_gamepad_grid_accept(&egui::Context::default())
                    .is_none()
            );
            assert!(app.context_folder_jump_pending());
            finish_toolbar_jump_and_go_back(&mut app, &target, &origin);
        }
    }

    #[test]
    fn internal_pane_scans_keep_transparent_search_navigation() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let target = app.tmp.path().join("internal-pane");
            std::fs::create_dir(&target).unwrap();
            let history = (
                app.folder_history_back_entry().cloned(),
                app.folder_history_forward_entry().cloned(),
            );
            app.start_folder_pane_open(target.clone());
            assert!(matches!(
                app.folder_pane_open_pending.as_ref().unwrap().purpose,
                super::super::FolderOpenScanPurpose::PaneNavigation
            ));
            settle_location_jump(&mut app, |app| app.current_folder.as_ref() == Some(&target));
            assert_search_active(&app, kind);
            assert_eq!(
                (
                    app.folder_history_back_entry().cloned(),
                    app.folder_history_forward_entry().cloned()
                ),
                history
            );
        }
    }

    #[test]
    fn address_input_and_user_pane_plain_entries_keep_existing_direct_and_pane_routing() {
        for entry in 0..5 {
            let mut app = setup_app();
            let origin = app.tmp.path().join("plain-user-origin");
            let shelf = app.tmp.path().join("plain-user-shelf");
            app.settings.book_root = Some(shelf.clone());
            let target = match entry {
                3 => shelf,
                4 => crate::books::book_folder(&shelf, "named-book"),
                _ => app.tmp.path().join("plain-user-target"),
            };
            std::fs::create_dir(&origin).unwrap();
            std::fs::create_dir_all(&target).unwrap();
            app.current_folder = Some(origin.clone());
            app.top_level_grid_view
                .replace_surface(TopLevelGridSurface::Folder);
            if entry == 0 {
                app.start_user_folder_pane_open(target.clone());
                assert!(matches!(
                    app.folder_pane_open_pending.as_ref().unwrap().purpose,
                    super::super::FolderOpenScanPurpose::PaneNavigation
                ));
            } else {
                app.dispatch_address_input(if entry == 1 {
                    LocationJumpTarget::DriveList
                } else {
                    LocationJumpTarget::Physical(target.clone())
                });
            }
            settle_location_jump(&mut app, |app| {
                if entry == 1 {
                    app.items_are_drive_list
                } else {
                    app.current_folder.as_ref() == Some(&target)
                }
            });
            assert_search_dismissed_with_history(&app, origin);
        }
    }

    #[test]
    fn user_pane_ready_replacement_uses_search_exit_and_new_search_cancels_plain_scan() {
        for kind in 0..4 {
            let mut app = search_fixture(kind);
            let old = app.tmp.path().join("old-pane-candidate");
            let new = app.tmp.path().join("new-pane-candidate");
            std::fs::create_dir(&old).unwrap();
            std::fs::create_dir(&new).unwrap();
            app.start_user_folder_pane_open(old.clone());
            let pending = app.folder_pane_open_pending.take().unwrap();
            pending
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let ready = super::super::FolderPaneOpenReady {
                navigation: pending.navigation,
                epub_restore: pending.epub_restore,
                path: pending.path,
                purpose: pending.purpose,
                scan: Ok(super::super::scan_directory(&old)),
            };
            app.replace_ready_folder_pane_open(new.clone(), Some(ready));
            assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, new);
            assert!(app.context_folder_jump_pending());
            assert_search_active(&app, kind);
            complete_physical_jump(&mut app, &new);
            let origin = app.tmp.path().join("pre-search");
            assert_search_dismissed_with_history(&app, origin);
        }
        let mut app = setup_app();
        let origin = app.tmp.path().join("plain-source");
        let target = app.tmp.path().join("replaced-pane");
        std::fs::create_dir(&origin).unwrap();
        std::fs::create_dir(&target).unwrap();
        app.current_folder = Some(origin.clone());
        app.top_level_grid_view
            .replace_surface(TopLevelGridSurface::Folder);
        app.start_user_folder_pane_open(target);
        let cancel = app
            .folder_pane_open_pending
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        app.open_global_search();
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(app.folder_pane_open_pending.is_none());
        assert!(app.global_search.active);
        assert_eq!(app.current_folder.as_ref(), Some(&origin));
    }
}
