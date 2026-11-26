//! UI-side ownership of folder order. Only the actor opens the persistent store.
use super::*;
use crate::folder_order::{
    FolderOrderCompletion, FolderOrderHandle, FolderOrderService, FolderOrderState,
    FolderOrderWriteIntent,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderSortChoice {
    Standard(crate::settings::SortOrder),
    Manual,
}

pub(crate) struct FolderOrderResetConfirmation {
    context: ViewerContextId,
    folder_key: String,
}

pub(super) fn start_service() -> Option<Box<FolderOrderService>> {
    match FolderOrderService::start() {
        Ok(service) => Some(Box::new(service)),
        Err(error) => {
            crate::logger::log(format!("folder_order: actor startup failed: {error}"));
            None
        }
    }
}

/// Extend the ordinary projection only while this listing applies manual order.
/// The caller supplies the requested listing's eligibility and stack decision;
/// `state.input` can still describe the previously adopted stack presentation.
pub(super) fn projection_key(base: u64, state: &FolderOrderState, manual_applied: bool) -> u64 {
    if !manual_applied || !state.manual_active() || !state.input.is_read() {
        return base;
    }
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    base.hash(&mut hash);
    state.folder_key.hash(&mut hash);
    state.mode.hash(&mut hash);
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
    /// Every sort control uses this mode owner. Standard callers retain their
    /// existing settings-save/refresh tail; entering Manual owns its refresh here.
    pub(crate) fn select_folder_sort(&mut self, choice: FolderSortChoice) -> bool {
        match choice {
            FolderSortChoice::Manual => {
                if self
                    .listing_sort_capabilities()
                    .manual_order_disabled_reason
                    .is_some()
                {
                    return false;
                }
                let revision = self.folder_order.order_revision;
                self.enter_folder_manual_order();
                self.folder_order.order_revision != revision
            }
            FolderSortChoice::Standard(order) => {
                let revision = self.folder_order.order_revision;
                self.leave_folder_manual_order();
                let changed = self.settings.sort_order != order
                    || self.folder_order.order_revision != revision;
                self.settings.sort_order = order;
                changed
            }
        }
    }

    pub(crate) fn folder_order_reset_available(&self) -> bool {
        self.listing_sort_capabilities()
            .manual_order_disabled_reason
            .is_none()
            && self.folder_order.record.is_some()
    }

    pub(crate) fn request_folder_order_reset_confirmation(&mut self) {
        if self.folder_order_reset_available() {
            self.folder_order_reset_confirmation = Some(Box::new(FolderOrderResetConfirmation {
                context: self.projected_viewer_context_id(),
                folder_key: self.folder_order.folder_key.clone().unwrap(),
            }));
        }
    }

    pub(crate) fn folder_order_reset_dialog_visible(&self) -> bool {
        self.folder_order_reset_confirmation.is_some()
    }

    pub(crate) fn resolve_folder_order_reset_confirmation(&mut self, confirmed: bool) {
        let Some(target) = self.folder_order_reset_confirmation.take() else {
            return;
        };
        if confirmed
            && target.context == self.projected_viewer_context_id()
            && self.folder_order.folder_key.as_ref() == Some(&target.folder_key)
            && self.folder_order_reset_available()
        {
            self.reset_folder_manual_order();
        }
    }

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
        *self.folder_order = FolderOrderState {
            order_revision: revision,
            ..Default::default()
        };
    }

    pub(crate) fn folder_order_handle(&self) -> Option<FolderOrderHandle> {
        self.folder_order_service
            .as_ref()
            .map(|service| service.handle.clone())
    }

    /// Only synchronous callers use this proof; worker producers still issue Read.
    pub(super) fn folder_order_record_is_known_absent(&self, folder: &Path) -> bool {
        self.folder_order_service
            .as_ref()
            .is_some_and(|service| service.handle.record_is_known_absent(folder))
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
            self.prepare_stack_mode_for_reload(false);
            self.cancel_stack_script_pending();
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
            if self.stack_mode_requested {
                self.prepare_stack_mode_for_reload(false);
            }
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
        // Classification adopts rows before identity hydration releases refresh polling.
        app.finish_page_identity_preparation_for_test();
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
                app.finish_page_identity_preparation_for_test();
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "manual refresh did not finish"
            );
            std::thread::yield_now();
        }
    }

    fn recorded_fixture() -> (AppTestEnvForTest, PathBuf) {
        let (mut app, folder) = fixture();
        app.select_folder_sort(FolderSortChoice::Manual);
        finish_refresh(&mut app);
        let mut order = keys(&app);
        order.reverse();
        app.commit_folder_manual_reorder(&order);
        finish_refresh(&mut app);
        assert!(read_record(&app, &folder).is_some());
        (app, folder)
    }

    #[test]
    fn folder_manual_choice_is_enter_only_and_preserves_details_header() {
        let (mut app, folder) = fixture();
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        assert!(app.select_folder_sort(FolderSortChoice::Manual));
        finish_refresh(&mut app);
        let revision = app.folder_order.order_revision;
        assert!(!app.select_folder_sort(FolderSortChoice::Manual));
        assert_eq!(app.folder_order.order_revision, revision);
        assert_eq!(
            app.settings.details_sort_key,
            crate::settings::DetailsSortKey::Name
        );
        assert!(read_record(&app, &folder).is_none());
    }

    #[test]
    fn folder_manual_assigned_grid_key_enters_once_through_common_handler() {
        let (mut app, folder) = fixture();
        app.keymap =
            crate::keymap::Keymap::from_ini_str("[Grid]\nGridEnterFolderManualOrder = F19\n");
        let ctx = egui::Context::default();
        let mut revision = app.folder_order.order_revision;
        for (pressed, changes) in [(true, true), (false, false), (true, false)] {
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F19,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            });
            let _ = app.handle_keyboard(&ctx);
            ctx.end_pass().drop_without_applying_deltas();
            if changes {
                assert!(app.folder_order.order_revision > revision);
                finish_refresh(&mut app);
                revision = app.folder_order.order_revision;
            } else {
                assert_eq!(app.folder_order.order_revision, revision);
            }
            assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
        }
        assert!(read_record(&app, &folder).is_none());
    }

    #[test]
    fn folder_manual_standard_choice_deactivates_record_even_when_base_value_matches() {
        let (mut app, folder) = recorded_fixture();
        let base = app.settings.sort_order;
        let revision = app.folder_order.order_revision;
        assert!(app.select_folder_sort(FolderSortChoice::Standard(base)));
        assert_eq!(app.folder_order.mode, FolderOrderMode::Normal);
        assert!(app.folder_order.order_revision > revision);
        assert!(!read_record(&app, &folder).unwrap().active);
        assert_eq!(app.settings.sort_order, base);
    }

    #[test]
    fn folder_manual_stack_toggle_and_manual_entry_keep_active_record_without_stack_writes() {
        let (mut app, folder) = recorded_fixture();
        let record = read_record(&app, &folder);
        app.toggle_stack_mode();
        app.settle_open_path_classification_for_test();
        app.finish_page_identity_preparation_for_test();
        assert!(app.stack_mode_requested);
        assert!(!app.folder_manual_order_applied());
        assert_eq!(read_record(&app, &folder), record);
        assert!(app.select_folder_sort(FolderSortChoice::Manual));
        assert!(!app.stack_mode_requested);
        finish_refresh(&mut app);
        assert!(app.folder_manual_order_applied());
        assert_eq!(read_record(&app, &folder), record);
    }

    #[test]
    fn folder_manual_reset_confirmation_cancel_escape_and_enter_preserve_or_reset_record() {
        let (mut app, folder) = recorded_fixture();
        let record = read_record(&app, &folder);
        app.request_folder_order_reset_confirmation();
        assert!(app.common_modal_dialog_open());
        assert_eq!(
            app.modal_dialog_block_reason_filtered(false, false),
            Some("folder_order_reset")
        );
        app.resolve_folder_order_reset_confirmation(false);
        assert!(!app.folder_order_reset_dialog_visible());
        assert_eq!(read_record(&app, &folder), record);
        let ctx = egui::Context::default();
        for key in [egui::Key::Escape, egui::Key::Enter] {
            app.request_folder_order_reset_confirmation();
            ctx.begin_pass(egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            });
            app.show_folder_order_reset_confirm_dialog(&ctx);
            ctx.end_pass().drop_without_applying_deltas();
            assert!(!app.folder_order_reset_dialog_visible());
            if key == egui::Key::Escape {
                assert_eq!(read_record(&app, &folder), record);
            } else {
                assert!(read_record(&app, &folder).is_none());
                assert_eq!(app.folder_order.mode, FolderOrderMode::ManualUnrecorded);
                finish_refresh(&mut app);
            }
        }
    }

    #[test]
    fn folder_manual_reset_confirmation_does_not_reset_a_different_folder_owner() {
        let (mut app, folder) = recorded_fixture();
        let record = read_record(&app, &folder);
        app.request_folder_order_reset_confirmation();
        app.folder_order.folder_key = Some(folder_key(&folder.join("other")));
        app.resolve_folder_order_reset_confirmation(true);
        assert!(!app.folder_order_reset_dialog_visible());
        assert_eq!(read_record(&app, &folder), record);
    }

    #[cfg(windows)]
    #[test]
    fn folder_manual_reset_confirmation_does_not_reset_same_folder_sibling() {
        let (mut app, folder) = recorded_fixture();
        let record = read_record(&app, &folder);
        let original = app.folder_order.clone();
        app.request_folder_order_reset_confirmation();
        let sibling_state = original.clone();
        let sibling_folder = folder.clone();
        let sibling = app.build_window_context_for_test(91913, |owner| {
            owner.folder_order = sibling_state;
            owner.current_folder = Some(sibling_folder);
        });
        app.with_viewer_context(sibling, |owner| {
            owner.resolve_folder_order_reset_confirmation(true);
            assert_eq!(owner.folder_order, original);
        })
        .unwrap();
        assert_eq!(app.folder_order, original);
        assert_eq!(read_record(&app, &folder), record);
    }

    #[test]
    fn folder_manual_order_sync_known_absent_adopts_immediately_without_actor_read() {
        let (mut app, _) = fixture();
        let destination = app.tmp.path().join("sync-unrecorded");
        std::fs::create_dir(&destination).unwrap();
        for name in ["b.jpg", "a.jpg"] {
            std::fs::write(destination.join(name), b"fixture").unwrap();
        }
        let handle = app.folder_order_handle().unwrap();
        assert!(handle.index_ready_for_test());
        assert!(handle.record_is_known_absent(&destination));
        let reads = handle.read_request_count_for_test();
        let generation = app.items_generation;
        assert!(matches!(
            app.app.load_folder_or_convert_archive(destination.clone()),
            FolderOpenOutcome::Loaded
        ));
        assert_eq!(app.current_folder.as_deref(), Some(destination.as_path()));
        assert_eq!(
            keys(&app),
            ["a.jpg", "b.jpg"]
                .into_iter()
                .map(|name| folder_key(&destination.join(name)))
                .collect::<Vec<_>>()
        );
        assert_eq!(app.items_generation, generation + 1);
        assert_eq!(
            app.folder_order.input,
            FolderOrderListingInput::from_read(Ok(None))
        );
        assert!(app.top_level_grid_view.open_path_classification().is_none());
        assert_eq!(handle.read_request_count_for_test(), reads);
        app.finish_page_identity_preparation_for_test();
    }

    #[test]
    fn folder_manual_order_sync_recorded_destination_reads_before_single_adoption() {
        let (mut app, origin) = fixture();
        let destination = app.tmp.path().join("sync-recorded");
        std::fs::create_dir(&destination).unwrap();
        let order = ["b.jpg", "a.jpg"]
            .into_iter()
            .map(|name| {
                let path = destination.join(name);
                std::fs::write(&path, b"fixture").unwrap();
                folder_key(&path)
            })
            .collect::<Vec<_>>();
        let handle = app.folder_order_handle().unwrap();
        handle
            .save(
                &destination,
                FolderOrderSave {
                    listing_order: order.clone(),
                    inventory: order.iter().cloned().collect(),
                    revision: 71,
                },
            )
            .unwrap();
        assert!(!handle.record_is_known_absent(&destination));
        let reads = handle.read_request_count_for_test();
        let generation = app.items_generation;
        let items = app.items.clone();
        assert!(matches!(
            app.app.load_folder_or_convert_archive(destination.clone()),
            FolderOpenOutcome::Classifying
        ));
        assert_eq!(app.current_folder.as_deref(), Some(origin.as_path()));
        assert_eq!(app.items, items);
        assert_eq!(app.items_generation, generation);
        app.settle_open_path_classification_for_test();
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.current_folder.as_deref(), Some(destination.as_path()));
        assert_eq!(keys(&app), order);
        assert!(app.folder_order.input.is_read());
        assert_eq!(app.items_generation, generation + 1);
        assert!(handle.read_request_count_for_test() > reads);
    }

    #[test]
    fn folder_manual_order_sync_unprepared_index_defers_unrecorded_destination() {
        let (mut app, origin) = fixture();
        (*app.folder_order_service.take().unwrap()).shutdown();
        let (service, start) = FolderOrderService::start_at_paused_for_test(
            app.tmp.path().join(crate::folder_order::DB_NAME),
        )
        .unwrap();
        let handle = service.handle.clone();
        app.folder_order_service = Some(Box::new(service));
        let destination = app.tmp.path().join("sync-startup-pending");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("a.jpg"), b"fixture").unwrap();
        assert!(!handle.index_ready_for_test());
        assert!(!handle.record_is_known_absent(&destination));
        let generation = app.items_generation;
        let items = app.items.clone();
        assert!(matches!(
            app.app.load_folder_or_convert_archive(destination.clone()),
            FolderOpenOutcome::Classifying
        ));
        assert_eq!(app.current_folder.as_deref(), Some(origin.as_path()));
        assert_eq!(app.items, items);
        assert_eq!(app.items_generation, generation);
        assert!(!handle.index_ready_for_test());
        start.send(()).unwrap();
        app.settle_open_path_classification_for_test();
        app.finish_page_identity_preparation_for_test();
        assert!(handle.index_ready_for_test());
        assert!(handle.record_is_known_absent(&destination));
        assert!(handle.read_request_count_for_test() > 0);
        assert_eq!(app.current_folder.as_deref(), Some(destination.as_path()));
        assert_eq!(
            app.folder_order.input,
            FolderOrderListingInput::from_read(Ok(None))
        );
        assert_eq!(app.items_generation, generation + 1);
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
    fn folder_manual_order_first_filtered_reorder_preserves_hidden_slots_and_selection() {
        let (mut app, folder) = fixture();
        for name in ["c.jpg", "d.jpg"] {
            std::fs::write(folder.join(name), b"fixture").unwrap();
        }
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        let [a, b, c, d] =
            ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(|name| folder_key(&folder.join(name)));
        assert_eq!(keys(&app), vec![a.clone(), b.clone(), c.clone(), d.clone()]);
        assert!(read_record(&app, &folder).is_none());
        // The filter hides b's row, while hidden.txt never entered the listing.
        app.visible_indices = vec![0, 2, 3];
        app.move_grid_cursor(Some(2), GridCursorIntent::Automatic);
        app.checked = [0, 1, 3].into_iter().collect();
        let generation = app.items_generation;
        app.commit_folder_manual_reorder(&[d.clone(), a.clone(), c.clone()]);
        let expected = vec![d.clone(), b.clone(), a.clone(), c.clone()];
        let saved = read_record(&app, &folder).unwrap();
        assert_eq!(saved.items, expected);
        assert!(saved.active);
        assert_eq!(app.folder_order.mode, FolderOrderMode::ManualRecorded);
        assert!(
            app.folder_order
                .inventory
                .contains(&folder_key(&folder.join("hidden.txt")))
        );
        // Saving does not permute the live rows and their indexed state in place.
        assert_eq!(app.items_generation, generation);
        assert_eq!(keys(&app), vec![a.clone(), b.clone(), c.clone(), d.clone()]);
        finish_refresh(&mut app);
        assert!(app.items_generation > generation);
        assert_eq!(keys(&app), expected);
        assert_eq!(app.selected, Some(3));
        assert_eq!(
            physical_item_key(&app.items[app.selected.unwrap()]),
            Some(c)
        );
        let checked = app
            .checked
            .iter()
            .map(|&index| physical_item_key(&app.items[index]).unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(checked, [a, b, d].into_iter().collect());
    }

    #[test]
    fn folder_manual_order_noop_submits_no_write_or_refresh_before_or_after_recording() {
        let (mut app, folder) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        for recorded in [false, true] {
            let order = keys(&app);
            let before = read_record(&app, &folder);
            assert_eq!(before.is_some(), recorded);
            // Read is a FIFO barrier: all previous write completions are available.
            app.folder_order_service
                .as_ref()
                .unwrap()
                .completions
                .try_iter()
                .for_each(drop);
            let state = app.folder_order.clone();
            let generation = app.items_generation;
            app.commit_folder_manual_reorder(&order);
            assert_eq!(app.folder_order, state);
            assert_eq!(app.items_generation, generation);
            assert!(app.folder_pane_open_pending.is_none());
            assert_eq!(read_record(&app, &folder), before);
            assert!(
                app.folder_order_service
                    .as_ref()
                    .unwrap()
                    .completions
                    .try_iter()
                    .next()
                    .is_none(),
                "a no-op must not submit even an equivalent actor write"
            );
            if !recorded {
                let reversed = order.into_iter().rev().collect::<Vec<_>>();
                app.commit_folder_manual_reorder(&reversed);
                finish_refresh(&mut app);
            }
        }
    }

    #[test]
    fn folder_manual_order_refresh_restores_selection_as_of_adoption() {
        let (mut app, folder) = fixture();
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
        let a = folder_key(&folder.join("a.jpg"));
        let b = folder_key(&folder.join("b.jpg"));
        app.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
        app.checked = [0].into_iter().collect();
        app.commit_folder_manual_reorder(&[b.clone(), a.clone()]);
        // Ordinary selection remains usable while the replacement scan is pending.
        app.move_grid_cursor(Some(1), GridCursorIntent::Automatic);
        app.checked = [1].into_iter().collect();
        finish_refresh(&mut app);
        assert_eq!(keys(&app), vec![b.clone(), a]);
        assert_eq!(app.selected, Some(0));
        assert_eq!(
            physical_item_key(&app.items[app.selected.unwrap()]),
            Some(b)
        );
        assert_eq!(app.checked, [0].into_iter().collect());
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
        assert!(app.external_rescan_pending.is_none());
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
        let Some(ExternalRescanResult::Equal(_, _, _, _, _, inventory)) = &response else {
            panic!("hidden-only changes must retain the displayed rows");
        };
        assert!(inventory.contains(&folder_key(&hidden)));
        let (tx, rx) = mpsc::channel();
        tx.send(response).unwrap();
        app.external_rescan_pending.as_mut().unwrap().rx = rx;
        app.poll_external_rescan(&ctx);
        assert!(app.external_rescan_pending.is_none());
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
        let old_projection = projection_key(7, &state, false);
        state.enter_manual();
        assert!(!completion_matches(&state, 11, &completion));
        assert_ne!(old_projection, projection_key(7, &state, true));
        state.folder_key = Some("c:/other".into());
        assert!(!completion_matches(&state, 11, &completion));
    }

    fn projection_state(input: FolderOrderListingInput) -> FolderOrderState {
        FolderOrderState::from_listing(
            Path::new("C:/folder"),
            input,
            ["c:/folder/a.jpg", "c:/folder/b.jpg", "c:/folder/hidden.jpg"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        )
    }

    #[test]
    fn folder_order_normal_projection_preserves_base_through_inventory_and_revision_changes() {
        let inactive = FolderOrderRecord {
            active: false,
            revision: 12,
            items: vec!["c:/folder/b.jpg".into(), "c:/folder/a.jpg".into()],
        };
        for input in [
            FolderOrderListingInput::NotApplicable,
            FolderOrderListingInput::from_read(Ok(None)),
            FolderOrderListingInput::from_read(Ok(Some(inactive))),
            FolderOrderListingInput::from_read(Err(FolderOrderError::new("read fixture"))),
        ] {
            let mut state = projection_state(input);
            assert_eq!(state.mode, FolderOrderMode::Normal);
            for base in [0, 7, u64::MAX] {
                // Even an overly broad caller cannot give Normal or NotApplicable
                // a manual identity merely because inventory/revision exists.
                assert_eq!(projection_key(base, &state, true), base);
            }
            state.inventory.insert("c:/folder/added.jpg".into());
            assert_eq!(projection_key(7, &state, true), 7);
            let revision = state.order_revision;
            assert!(state.delete(&["c:/folder/a.jpg".into()]));
            assert!(state.order_revision > revision);
            assert_eq!(projection_key(7, &state, true), 7);
            let revision = state.order_revision;
            assert!(state.rename(&[FolderOrderRename::new(
                Path::new("c:/folder/b.jpg"),
                Path::new("c:/folder/renamed.jpg"),
                RenameScope::Exact,
            )]));
            assert!(state.order_revision > revision);
            assert_eq!(projection_key(7, &state, true), 7);
            if state.folder_key.is_some() {
                let revision = state.order_revision;
                let input = state.listing_input();
                let inventory = state.inventory.clone();
                state.adopt_listing(input, inventory);
                assert!(state.order_revision > revision);
                assert_eq!(projection_key(7, &state, true), 7);
            }
        }
    }

    #[test]
    fn folder_order_manual_projection_tracks_mode_reorder_reset_rename_delete_and_exit() {
        let base = 7;
        let mut state = projection_state(FolderOrderListingInput::from_read(Ok(None)));
        assert_eq!(projection_key(base, &state, true), base);
        state.enter_manual();
        let entered = projection_key(base, &state, true);
        assert_ne!(entered, base);
        state.enter_manual();
        assert_eq!(projection_key(base, &state, true), entered);

        let full = vec!["c:/folder/a.jpg".into(), "c:/folder/b.jpg".into()];
        let visible = full.iter().cloned().collect();
        let reordered = vec!["c:/folder/b.jpg".into(), "c:/folder/a.jpg".into()];
        assert!(
            state
                .commit_reorder(&full, &visible, &reordered)
                .unwrap()
                .is_some()
        );
        let recorded = projection_key(base, &state, true);
        assert_ne!(recorded, entered);
        assert!(state.reset().is_some());
        let reset = projection_key(base, &state, true);
        assert_ne!(reset, recorded);
        assert_ne!(reset, entered);
        assert!(state.rename(&[FolderOrderRename::new(
            Path::new("c:/folder/a.jpg"),
            Path::new("c:/folder/renamed.jpg"),
            RenameScope::Exact,
        )]));
        let renamed = projection_key(base, &state, true);
        assert_ne!(renamed, reset);
        assert!(state.delete(&["c:/folder/b.jpg".into()]));
        assert_ne!(projection_key(base, &state, true), renamed);
        state.leave_manual();
        assert_eq!(projection_key(base, &state, true), base);
    }

    #[test]
    fn folder_order_suppressed_manual_projection_preserves_base_and_snapshot() {
        let mut state = projection_state(FolderOrderListingInput::from_read(Ok(Some(
            FolderOrderRecord {
                active: true,
                revision: 12,
                items: vec!["c:/folder/b.jpg".into(), "c:/folder/a.jpg".into()],
            },
        ))));
        let manual = projection_key(7, &state, true);
        assert_ne!(manual, 7);
        let before = state.clone();
        // Both an ineligible listing and a requested stack presentation suppress
        // manual identity without deactivating the retained record.
        assert_eq!(projection_key(7, &state, false), 7);
        assert_eq!(state, before);
        state.input = state.input.with_stack(true);
        assert_eq!(projection_key(7, &state, false), 7);
        // A new stack-off request uses its actual decision rather than the
        // previous adopted input's stack bit.
        assert_eq!(projection_key(7, &state, true), manual);
        state.delete(&["c:/folder/a.jpg".into()]);
        assert_eq!(projection_key(7, &state, false), 7);
        assert_ne!(projection_key(7, &state, true), manual);
    }
}
