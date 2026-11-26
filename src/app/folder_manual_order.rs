//! UI-side ownership of folder order. Only the actor opens the persistent store.
use super::*;
use crate::folder_order::{
    FolderOrderCompletion, FolderOrderHandle, FolderOrderService, FolderOrderState,
    FolderOrderWriteIntent,
};

pub(super) fn start_service() -> Option<FolderOrderService> {
    match FolderOrderService::start() {
        Ok(service) => Some(service),
        Err(error) => {
            crate::logger::log(format!("folder_order: actor startup failed: {error}"));
            None
        }
    }
}

pub(super) fn projection_key(base: u64, state: &FolderOrderState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    base.hash(&mut hash);
    state.folder_key.hash(&mut hash);
    state.order_revision.hash(&mut hash);
    hash.finish()
}

pub(super) fn selection_key(item: &GridItem) -> Option<String> {
    match item {
        GridItem::Stack { representative, .. } => {
            Some(crate::folder_order::folder_key(representative))
        }
        _ => crate::folder_order::physical_item_key(item),
    }
}

fn completion_matches(
    state: &FolderOrderState,
    owner: u64,
    completion: &FolderOrderCompletion,
) -> bool {
    completion.owner == Some(owner)
        && state.folder_key.as_deref() == Some(completion.folder_key.as_str())
        && state.order_revision == completion.revision
}

impl App {
    pub(super) fn retire_folder_order_for_listing(&mut self) {
        // Retiring an already out-of-scope owner has no transition to publish.
        if matches!(
            self.folder_order.input,
            crate::folder_order::FolderOrderListingInput::NotApplicable
        ) {
            return;
        }
        let revision = self
            .folder_order
            .order_revision
            .checked_add(1)
            .expect("folder order revision exhausted");
        self.folder_order = FolderOrderState {
            order_revision: revision,
            ..Default::default()
        };
    }

    pub(crate) fn folder_order_handle(&self) -> Option<FolderOrderHandle> {
        self.folder_order_service
            .as_ref()
            .map(|service| service.handle.clone())
    }

    pub(super) fn poll_folder_order_completions(&mut self) {
        let completions = self
            .folder_order_service
            .as_ref()
            .map(|service| service.completions.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for completion in completions {
            if self.folder_order_completion_owner_is_current(&completion) {
                if let Err(error) = completion.result {
                    crate::logger::log(format!(
                        "folder_order: current listing write failed: {error}"
                    ));
                    self.show_feedback_warning("手動順を保存できませんでした".into());
                }
            }
        }
    }

    fn folder_order_completion_owner_is_current(&self, completion: &FolderOrderCompletion) -> bool {
        #[cfg(windows)]
        {
            self.viewer_context_ids()
                .into_iter()
                .find(|id| Some(id.serial()) == completion.owner)
                .and_then(|id| {
                    self.with_viewer_context_ref(id, |context| {
                        completion_matches(context.folder_order(), id.serial(), completion)
                    })
                })
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            completion_matches(
                &self.folder_order,
                self.projected_viewer_context_id().serial(),
                completion,
            )
        }
    }

    pub(super) fn notify_folder_order_read_failure(&mut self) {
        if let Some(error) = self.folder_order.input.error() {
            crate::logger::log(format!("folder_order: listing read failed: {error}"));
            self.show_feedback_warning(
                "手動順を読み取れなかったため、通常の並び順で表示しました".into(),
            );
        }
    }

    fn submit_folder_order_intent(&mut self, intent: Option<FolderOrderWriteIntent>) {
        let Some(intent) = intent else { return };
        let Some(folder) = self.folder_order.folder_key.as_deref().map(PathBuf::from) else {
            return;
        };
        let owner = self.projected_viewer_context_id().serial();
        let result = self
            .folder_order_handle()
            .ok_or_else(|| "folder order actor unavailable".to_owned())
            .and_then(|handle| {
                let handle = handle.for_owner(owner);
                match intent {
                    FolderOrderWriteIntent::Save(save) => handle.save(&folder, save),
                    FolderOrderWriteIntent::SetActive { active, revision } => {
                        handle.set_active(&folder, active, revision)
                    }
                    FolderOrderWriteIntent::Reset { revision } => handle.reset(&folder, revision),
                }
                .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            crate::logger::log(format!("folder_order: submit failed: {error}"));
            self.show_feedback_warning("手動順を保存できませんでした".into());
        }
    }

    /// Shared handlers; controls and drag input are connected in the next stages.
    pub(crate) fn enter_folder_manual_order(&mut self) {
        if !self.folder_manual_order_eligible() {
            return;
        }
        let stack_was_active = self.stack_mode_requested;
        if stack_was_active {
            self.stack_mode_requested = false;
            self.cancel_stack_script_pending();
            self.stack_selection_target = None;
        }
        let revision = self.folder_order.order_revision;
        let intent = self.folder_order.enter_manual();
        if stack_was_active && self.folder_order.order_revision == revision {
            self.folder_order.order_revision = revision
                .checked_add(1)
                .expect("folder order revision exhausted");
        }
        self.submit_folder_order_intent(intent);
        if self.folder_order.order_revision != revision || stack_was_active {
            self.apply_sort_change_reload();
        }
    }

    /// Standard-sort controls call this before their existing setting commit and
    /// `apply_sort_change_reload`; this handler does not start a second refresh.
    pub(crate) fn leave_folder_manual_order(&mut self) {
        if !self.folder_manual_order_eligible() {
            return;
        }
        let intent = self.folder_order.leave_manual();
        self.submit_folder_order_intent(intent);
    }

    pub(crate) fn reset_folder_manual_order(&mut self) {
        if !self.folder_manual_order_eligible() {
            return;
        }
        let revision = self.folder_order.order_revision;
        let intent = self.folder_order.reset();
        self.submit_folder_order_intent(intent);
        if self.folder_order.order_revision != revision {
            self.stack_mode_requested = false;
            self.cancel_stack_script_pending();
            self.stack_selection_target = None;
            self.apply_sort_change_reload();
        }
    }

    pub(crate) fn commit_folder_manual_reorder(&mut self, reordered: &[String]) {
        if !self.folder_manual_order_drag_enabled() {
            return;
        }
        let full = self
            .items
            .iter()
            .filter_map(crate::folder_order::physical_item_key)
            .collect::<Vec<_>>();
        let visible = self
            .visible_indices
            .iter()
            .filter_map(|&index| self.items.get(index))
            .filter_map(crate::folder_order::physical_item_key)
            .collect::<HashSet<_>>();
        match self.folder_order.commit_reorder(&full, &visible, reordered) {
            Ok(Some(intent)) => {
                self.submit_folder_order_intent(Some(intent));
                self.apply_sort_change_reload();
            }
            Ok(None) => {}
            Err(error) => {
                crate::logger::log(format!("folder_order: invalid reorder: {error}"));
                self.show_feedback_warning("手動順を変更できませんでした".into());
            }
        }
    }

    pub(crate) fn folder_manual_order_eligible(&self) -> bool {
        self.current_listing_accepts_folder_scan()
            && !self.snapshot_internal_nav
            && matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
            && !self.items_are_subfolder_expansion_view
            && !self.items_are_smart_folder_view
            && self.folder_order.folder_key.as_deref().is_some_and(|key| {
                self.current_folder.as_deref().is_some_and(|path| {
                    crate::folder_order::folder_key(path) == key
                        && crate::folder_order::physical_folder_order_eligible(
                            &self.settings,
                            path,
                            &self.items,
                        )
                })
            })
    }

    pub(crate) fn folder_manual_order_applied(&self) -> bool {
        self.folder_manual_order_eligible()
            && !self.stack_mode_requested
            && self.current_folder.as_deref().is_some_and(|path| {
                self.folder_order
                    .applies_to_physical_listing(&self.settings, path, &self.items)
            })
    }

    pub(crate) fn folder_manual_order_drag_enabled(&self) -> bool {
        self.folder_manual_order_applied()
            && self.settings.grid_view_mode == crate::settings::GridViewMode::Thumbnail
    }

    /// Called immediately after accepting Shell rename success, before new reads/saves.
    pub(crate) fn adopt_folder_order_rename(
        &mut self,
        mappings: Vec<crate::folder_order::FolderOrderRename>,
    ) {
        self.folder_order.rename(&mappings);
        // The actor must migrate even when the displayed context is out of scope.
        let folder = self
            .folder_order
            .folder_key
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| self.current_folder.clone())
            .unwrap_or_default();
        let owner = self.projected_viewer_context_id().serial();
        let result = self
            .folder_order_handle()
            .ok_or_else(|| "folder order actor unavailable".to_owned())
            .and_then(|handle| {
                handle
                    .for_owner(owner)
                    .rename(&folder, self.folder_order.order_revision, mappings)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = result {
            crate::logger::log(format!("folder_order: rename submit failed: {error}"));
            self.show_feedback_warning("手動順の名前を更新できませんでした".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder_order::*;

    fn read_record(app: &App, path: &Path) -> Option<FolderOrderRecord> {
        app.folder_order_handle()
            .unwrap()
            .read(path)
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap()
    }

    fn fixture() -> (AppTestEnvForTest, PathBuf) {
        let mut app = setup_app_for_test();
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.settings.auto_fullscreen_image_folders = false;
        app.active_quick_folder_slot = None;
        app.settings.sort_order = crate::settings::SortOrder::FileName;
        app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
        let folder = app.tmp.path().join("manual-order");
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["a.jpg", "b.jpg", "hidden.txt"] {
            std::fs::write(folder.join(name), b"fixture").unwrap();
        }
        app.load_folder(folder.clone());
        (app, folder)
    }

    fn keys(app: &App) -> Vec<String> {
        app.items.iter().filter_map(physical_item_key).collect()
    }

    fn finish_refresh(app: &mut App) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_sidecar_restore(&ctx);
            if let Some(ready) = app.poll_folder_pane_open(&ctx) {
                let FolderOpenScanPurpose::CurrentViewOrderRefresh {
                    order,
                    reload_owner,
                } = ready.purpose
                else {
                    panic!("unexpected folder open purpose");
                };
                assert!(app.apply_current_view_order_refresh(
                    ready.path,
                    ready.scan.unwrap(),
                    order,
                    *reload_owner
                ));
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "manual refresh did not finish"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn folder_manual_order_handlers_record_only_committed_reorder_and_restore_identity() {
        let (mut app, folder) = fixture();
        let a = folder_key(&folder.join("a.jpg"));
        let b = folder_key(&folder.join("b.jpg"));
        assert!(read_record(&app, &folder).is_none());
        let revision = app.folder_order.order_revision;
        app.enter_folder_manual_order();
        assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
        assert!(app.folder_order.order_revision > revision);
        finish_refresh(&mut app);
        assert!(read_record(&app, &folder).is_none());
        let selected = app
            .items
            .iter()
            .position(|item| physical_item_key(item).as_deref() == Some(a.as_str()))
            .unwrap();
        app.selected = Some(selected);
        app.checked = [selected].into_iter().collect();
        let details = app.settings.details_sort_key;
        app.commit_folder_manual_reorder(&[b.clone(), a.clone()]);
        assert_eq!(
            read_record(&app, &folder).unwrap().items,
            vec![b.clone(), a.clone()]
        );
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![b.clone(), a.clone()]);
        assert_eq!(
            app.selected
                .and_then(|index| physical_item_key(&app.items[index])),
            Some(a.clone())
        );
        assert!(app.checked.contains(&1));
        assert_eq!(app.settings.details_sort_key, details);
        app.leave_folder_manual_order();
        assert!(!read_record(&app, &folder).unwrap().active);
        app.apply_sort_change_reload();
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![a.clone(), b.clone()]);
        app.enter_folder_manual_order();
        assert!(read_record(&app, &folder).unwrap().active);
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![b, a]);
        let revision = app.folder_order.order_revision;
        app.reset_folder_manual_order();
        assert!(app.folder_order.order_revision > revision);
        assert!(read_record(&app, &folder).is_none());
        finish_refresh(&mut app);
        assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
        assert!(keys(&app)[0].ends_with("/a.jpg"));
    }

    #[test]
    fn folder_manual_order_sync_entry_preserves_source_and_adopts_read_once() {
        let (mut app, source) = fixture();
        let destination = app.tmp.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        for name in ["a.jpg", "b.jpg", "hidden.txt"] {
            std::fs::write(destination.join(name), b"fixture").unwrap();
        }
        let a = folder_key(&destination.join("a.jpg"));
        let b = folder_key(&destination.join("b.jpg"));
        let hidden = folder_key(&destination.join("hidden.txt"));
        app.folder_order_handle()
            .unwrap()
            .save(
                &destination,
                FolderOrderSave {
                    listing_order: vec![b.clone(), a.clone()],
                    inventory: [a.clone(), b.clone(), hidden.clone()].into_iter().collect(),
                    revision: 1,
                },
            )
            .unwrap();
        let generation = app.items_generation;
        let source_keys = keys(&app);
        // Call App directly: the old fixture convenience method waits for adoption.
        app.app.load_folder(destination.clone());
        assert_eq!(app.current_folder.as_ref(), Some(&source));
        assert_eq!(keys(&app), source_keys);
        assert_eq!(app.items_generation, generation);
        assert!(app.top_level_grid_view.open_path_classification().is_some());
        let request = app
            .top_level_grid_view
            .open_path_classification()
            .unwrap()
            .request_id;
        app.app.load_folder(destination.clone());
        assert_eq!(
            app.top_level_grid_view
                .open_path_classification()
                .unwrap()
                .request_id,
            request
        );
        app.settle_open_path_classification_for_test();
        assert_eq!(app.current_folder.as_ref(), Some(&destination));
        assert_eq!(keys(&app), vec![b, a]);
        assert!(app.folder_order.inventory.contains(&hidden));
        let adopted_generation = app.items_generation;
        app.poll_open_path_classification(&egui::Context::default());
        assert_eq!(app.items_generation, adopted_generation);
    }

    #[test]
    fn folder_manual_order_unrecorded_choice_disappears_after_other_folder_adopts() {
        let (mut app, folder) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
        let other = app.tmp.path().join("other-listing");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("other.jpg"), b"fixture").unwrap();
        app.load_folder(other.clone());
        assert_eq!(app.folder_order.folder_key, Some(folder_key(&other)));
        assert_eq!(app.folder_order.mode, FolderOrderMode::Normal);
        app.load_folder(folder.clone());
        assert_eq!(app.folder_order.mode, FolderOrderMode::Normal);
        assert!(read_record(&app, &folder).is_none());
    }

    #[test]
    fn folder_manual_order_supplied_scan_gets_worker_read_without_rescanning() {
        let (mut app, source) = fixture();
        let folder = app.tmp.path().join("prepared-listing");
        std::fs::create_dir(&folder).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            std::fs::write(folder.join(name), b"fixture").unwrap();
        }
        let scan = app.folder_scan_policy().scan(&folder, None).unwrap();
        let a = folder_key(&folder.join("a.jpg"));
        let b = folder_key(&folder.join("b.jpg"));
        app.folder_order_handle()
            .unwrap()
            .save(
                &folder,
                FolderOrderSave {
                    listing_order: vec![b.clone(), a.clone()],
                    inventory: scan.folder_order_inventory.clone(),
                    revision: 1,
                },
            )
            .unwrap();
        std::fs::write(folder.join("later.jpg"), b"arrived after supplied scan").unwrap();
        app.app.load_folder_with_scan(folder.clone(), Some(scan));
        assert_eq!(app.current_folder.as_ref(), Some(&source));
        app.settle_open_path_classification_for_test();
        assert_eq!(keys(&app), vec![b, a]);
        assert!(app.folder_order.input.is_read());
        assert!(
            !app.folder_order
                .inventory
                .contains(&folder_key(&folder.join("later.jpg")))
        );
    }

    #[test]
    fn folder_manual_order_refresh_rejects_previous_mode_revision() {
        let (mut app, folder) = fixture();
        let mut order = CurrentViewOrderSnapshot::from_settings(&app.settings);
        order.order_revision = app.folder_order.order_revision;
        let mut scan = app.folder_scan_policy().scan(&folder, None).unwrap();
        scan.folder_order_input = app.folder_order.listing_input();
        app.folder_order.enter_manual();
        let generation = app.items_generation;
        let owner = app.current_folder_reload_owner(&folder);
        assert!(!app.apply_current_view_order_refresh(folder, scan, order, owner));
        assert_eq!(app.items_generation, generation);
        assert!(
            app.folder_pane_open_pending.is_some(),
            "the current revision must be refreshed"
        );
    }

    #[test]
    fn folder_manual_order_rescan_rejects_previous_mode_revision_and_retains_refresh() {
        let (mut app, folder) = fixture();
        let ctx = egui::Context::default();
        app.start_external_rescan_with_cause(
            &ctx,
            folder,
            std::time::SystemTime::UNIX_EPOCH,
            book_append_refresh::FolderRefreshCause::Operation,
        );
        let response = app
            .external_rescan_pending
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(response).unwrap();
        app.external_rescan_pending.as_mut().unwrap().rx = rx;
        let generation = app.items_generation;
        app.folder_order.enter_manual();
        app.poll_external_rescan(&ctx);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
        assert!(app.folder_refresh_pending.is_some());
    }

    #[test]
    fn folder_manual_order_reset_uses_remembered_favorite_base_order() {
        let (mut app, _) = fixture();
        let folder = app.tmp.path().join("favorite-base");
        std::fs::create_dir(&folder).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            std::fs::write(folder.join(name), b"fixture").unwrap();
        }
        let favorite = crate::settings::FavoriteEntry::new("base".into(), folder.clone());
        let mut presentation = crate::settings::FavoriteViewState::from_settings(&app.settings);
        presentation.sort_order = crate::settings::SortOrder::FileNameDesc;
        app.favorite_view_states.insert(favorite.id, presentation);
        app.settings.favorites.push(favorite);
        app.settings.remember_favorite_view_state = true;
        app.load_folder(folder.clone());
        let a = folder_key(&folder.join("a.jpg"));
        let b = folder_key(&folder.join("b.jpg"));
        assert_eq!(keys(&app), vec![b.clone(), a.clone()]);
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        app.commit_folder_manual_reorder(&[a.clone(), b.clone()]);
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![a.clone(), b.clone()]);
        app.reset_folder_manual_order();
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![b, a]);
        assert_eq!(
            app.settings.sort_order,
            crate::settings::SortOrder::FileNameDesc
        );
    }

    #[test]
    fn folder_manual_order_details_header_keeps_manual_base_and_disables_reorder_drag() {
        let (mut app, _) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        let mut recorded = keys(&app);
        recorded.reverse();
        app.commit_folder_manual_reorder(&recorded);
        finish_refresh(&mut app);
        app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        assert!(app.details_header_sort_active());
        assert!(app.folder_manual_order_eligible());
        assert!(app.folder_manual_order_applied());
        assert!(!app.folder_manual_order_drag_enabled());
        app.leave_folder_manual_order();
        app.apply_sort_change_reload();
        finish_refresh(&mut app);
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        assert_eq!(keys(&app), recorded);
        assert_eq!(
            app.settings.details_sort_key,
            crate::settings::DetailsSortKey::Name
        );
        app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
        assert!(app.folder_manual_order_drag_enabled());
        assert_eq!(keys(&app), recorded);
    }

    #[test]
    fn folder_manual_order_watcher_uses_memory_and_adopts_hidden_inventory() {
        let (mut app, folder) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        let mut reversed = keys(&app);
        reversed.reverse();
        app.commit_folder_manual_reorder(&reversed);
        finish_refresh(&mut app);
        let recorded = keys(&app);
        let mut different = recorded.clone();
        different.reverse();
        app.folder_order_handle()
            .unwrap()
            .save(
                &folder,
                FolderOrderSave {
                    listing_order: different.clone(),
                    inventory: app.folder_order.inventory.clone(),
                    revision: 99,
                },
            )
            .unwrap();
        assert_eq!(read_record(&app, &folder).unwrap().items, different);
        let hidden = folder.join("added.txt");
        std::fs::write(&hidden, b"hidden inventory").unwrap();
        let ctx = egui::Context::default();
        app.start_external_rescan_with_cause(
            &ctx,
            folder.clone(),
            std::time::SystemTime::UNIX_EPOCH,
            book_append_refresh::FolderRefreshCause::Operation,
        );
        let response = app
            .external_rescan_pending
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(matches!(&response, Some(ExternalRescanResult::Equal(..))));
        let (tx, rx) = mpsc::channel();
        tx.send(response).unwrap();
        app.external_rescan_pending.as_mut().unwrap().rx = rx;
        app.poll_external_rescan(&ctx);
        assert_eq!(keys(&app), recorded);
        assert!(app.folder_order.inventory.contains(&folder_key(&hidden)));
    }

    #[test]
    fn folder_manual_order_rename_then_read_and_delete_then_save_keep_new_inventory() {
        let (mut app, folder) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        let mut reordered = keys(&app);
        reordered.reverse();
        app.commit_folder_manual_reorder(&reordered);
        finish_refresh(&mut app);
        let old = folder.join("a.jpg");
        let new = folder.join("z.jpg");
        std::fs::rename(&old, &new).unwrap();
        app.adopt_folder_order_rename(vec![FolderOrderRename::new(&old, &new, RenameScope::Exact)]);
        assert!(!app.folder_order.inventory.contains(&folder_key(&old)));
        assert!(app.folder_order.inventory.contains(&folder_key(&new)));
        assert_eq!(
            read_record(&app, &folder).unwrap().items,
            app.folder_order.record.as_ref().unwrap().items
        );
        // Accept an operation refresh using the already rewritten memory snapshot.
        app.apply_sort_change_reload();
        finish_refresh(&mut app);
        std::fs::remove_file(&new).unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(crate::delete_worker::DeleteMsg::Done {
            canceled: false,
            store_mutations: Default::default(),
        })
        .unwrap();
        app.delete_pending = Some(crate::delete_worker::DeletePending {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            total: 1,
            succeeded: vec![new.clone()],
            failed: vec![],
            purged_pdf_password_paths: vec![],
            purge_deferred: false,
            source_scopes: vec![crate::delete_worker::DeleteSourceScope::Exact(new.clone())],
        });
        app.poll_delete_pending();
        assert!(!app.folder_order.inventory.contains(&folder_key(&new)));
        assert!(
            !app.folder_order
                .record
                .as_ref()
                .unwrap()
                .items
                .contains(&folder_key(&new))
        );
        assert!(!keys(&app).contains(&folder_key(&new)));
        let added = folder.join("c.jpg");
        std::fs::write(&added, b"new image").unwrap();
        app.apply_sort_change_reload();
        finish_refresh(&mut app);
        let mut reordered = keys(&app);
        reordered.reverse();
        app.commit_folder_manual_reorder(&reordered);
        let saved = read_record(&app, &folder).unwrap();
        assert!(!saved.items.contains(&folder_key(&old)));
        assert!(!saved.items.contains(&folder_key(&new)));
        assert!(saved.items.contains(&folder_key(&added)));
        finish_refresh(&mut app);
    }

    #[cfg(windows)]
    #[test]
    fn folder_manual_order_bundle_swap_and_fork_keep_sibling_snapshot() {
        let (mut app, _) = fixture();
        app.folder_order.enter_manual();
        let main = app.folder_order.clone();
        let mut fork = app.split_current_context_preserving_main_grid();
        assert_eq!(fork.folder_order, main);
        assert_eq!(app.folder_order, main);
        app.swap_viewer_context_bundle(&mut fork);
        app.folder_order.leave_manual();
        let changed = app.folder_order.clone();
        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(app.folder_order, main);
        assert_eq!(fork.folder_order, changed);
        drop(fork);
        assert_eq!(app.folder_order, main);
    }

    #[cfg(windows)]
    #[test]
    fn folder_manual_order_completion_finds_parked_owner_without_changing_same_folder_sibling() {
        let (mut app, _) = fixture();
        let owner = app.projected_viewer_context_id();
        let state = app.folder_order.clone();
        let completion = FolderOrderCompletion {
            owner: Some(owner.serial()),
            folder_key: state.folder_key.clone().unwrap(),
            revision: state.order_revision,
            operation: FolderOrderOperation::Save,
            result: Ok(()),
        };
        let sibling_state = state.clone();
        let sibling = app.build_window_context_for_test(91911, |sibling| {
            sibling.folder_order = sibling_state;
        });
        app.with_viewer_context(sibling, |mounted| {
            assert!(mounted.folder_order_completion_owner_is_current(&completion));
            assert!(!completion_matches(
                &mounted.folder_order,
                sibling.serial(),
                &completion
            ));
            assert_eq!(mounted.folder_order, state);
            let mut unrelated = completion.clone();
            unrelated.owner = Some(u64::MAX);
            assert!(!mounted.folder_order_completion_owner_is_current(&unrelated));
        })
        .unwrap();
        assert_eq!(app.folder_order, state);
    }

    #[test]
    fn folder_order_completion_requires_folder_and_current_context_revision() {
        let mut state = FolderOrderState::from_listing(
            Path::new("C:/folder"),
            FolderOrderListingInput::from_read(Ok(None)),
            HashSet::new(),
        );
        let completion = FolderOrderCompletion {
            owner: Some(11),
            folder_key: "c:/folder".into(),
            revision: state.order_revision,
            operation: FolderOrderOperation::Reset,
            result: Ok(()),
        };
        assert!(completion_matches(&state, 11, &completion));
        assert!(!completion_matches(&state.clone(), 12, &completion));
        let old_projection = projection_key(7, &state);
        state.enter_manual();
        assert!(!completion_matches(&state, 11, &completion));
        assert_ne!(old_projection, projection_key(7, &state));
        state.folder_key = Some("c:/other".into());
        assert!(!completion_matches(&state, 11, &completion));
    }
}
