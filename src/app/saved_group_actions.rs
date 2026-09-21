use super::{App, ScannedDir, smart_folder::SmartFolderSourceLease};
use crate::collection_store::CollectionId;
use crate::keymap::KeyAction;
use crate::ui_main::AddressBarNav;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Numbered actions use the complete management list, not toolbar pins.
fn ordered_target_index(len: usize, current: Option<usize>, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, forward) {
        (Some(index), true) => (index + 1) % len,
        (Some(index), false) => (index + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    })
}

enum SavedGroupOpenPhase {
    WaitingBookList,
    ScanningBook {
        path: PathBuf,
        receiver: Receiver<std::io::Result<ScannedDir>>,
        cancel: Arc<AtomicBool>,
    },
    ReadyBook {
        path: PathBuf,
        scan: ScannedDir,
    },
    WaitingCollectionCatalog {
        requested: bool,
    },
    ReadyCollection {
        id: CollectionId,
    },
}

/// One main-grid navigation request owns its source, preparation, and ready payload.
pub(super) struct SavedGroupOpenTransition {
    id: u64,
    action: KeyAction,
    source: SmartFolderSourceLease,
    current_book: Option<PathBuf>,
    current_collection: Option<CollectionId>,
    requested_collection_target: Option<CollectionId>,
    lease: crate::collection_store::CollectionReadLease,
    phase: SavedGroupOpenPhase,
}

impl Drop for SavedGroupOpenTransition {
    fn drop(&mut self) {
        if let SavedGroupOpenPhase::ScanningBook { cancel, .. } = &self.phase {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn book_action_target(
    rows: &[crate::books::BookInfo],
    action: KeyAction,
    current: Option<&PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(slot) = action.book_slot_number() {
        return rows
            .get(slot - 1)
            .map(|row| row.path.clone())
            .ok_or_else(|| format!("本 {slot} は未登録です"));
    }
    let forward = match action {
        KeyAction::GridBookPrev => false,
        KeyAction::GridBookNext => true,
        _ => return Err("本棚の移動操作を確認できません".into()),
    };
    let current = current.and_then(|path| {
        rows.iter()
            .position(|row| crate::folder_tree::path_eq(&row.path, path))
    });
    let index = ordered_target_index(rows.len(), current, forward)
        .ok_or_else(|| "本が登録されていません".to_string())?;
    Ok(rows[index].path.clone())
}

fn collection_action_target(
    ids: &[CollectionId],
    target: Option<CollectionId>,
    action: KeyAction,
    current: Option<CollectionId>,
) -> Result<CollectionId, String> {
    if action == KeyAction::GridOpenCollectionTarget {
        return target
            .filter(|id| ids.contains(id))
            .ok_or_else(|| "追加先のコレクションが未設定または削除されました".into());
    }
    if let Some(slot) = action.collection_slot_number() {
        return ids
            .get(slot - 1)
            .copied()
            .ok_or_else(|| format!("コレクション {slot} は未登録です"));
    }
    let forward = match action {
        KeyAction::GridCollectionPrev => false,
        KeyAction::GridCollectionNext => true,
        _ => return Err("コレクションの移動操作を確認できません".into()),
    };
    let current = current.and_then(|id| ids.iter().position(|candidate| *candidate == id));
    let index = ordered_target_index(ids.len(), current, forward)
        .ok_or_else(|| "コレクションが登録されていません".to_string())?;
    Ok(ids[index])
}

fn saved_group_source_unavailable_message(projected_main_context: bool) -> &'static str {
    if projected_main_context {
        "メインの表示状態を確認できません"
    } else {
        "この操作はメインウィンドウでのみ使用できます"
    }
}

impl App {
    pub(crate) fn open_book_manager_from_action(&mut self) {
        self.show_book_manager = true;
        self.book_manager_rename_name = self.active_book_name();
        self.request_book_list_refresh();
    }

    /// Menu, toolbar, and keymap entry points use the same owned scan/adoption path.
    pub(crate) fn open_active_book_from_action(&mut self, ctx: &egui::Context) {
        let _ = self.apply_saved_group_key_action(ctx, KeyAction::GridOpenActiveBook);
    }

    fn saved_group_other_main_nav_pending(&self) -> bool {
        self.folder_nav_pending.is_some()
            || self
                .folder_pane_open_pending
                .as_ref()
                .is_some_and(|pending| {
                    !matches!(
                        pending.purpose,
                        super::FolderOpenScanPurpose::DetachedFolder
                            | super::FolderOpenScanPurpose::DetachedImage { .. }
                    )
                })
            || self.smart_folder_transition.is_some()
            || self.bookmark_open_pending.is_some()
            || matches!(
                self.bookmark_view_state,
                Some(super::BookmarkViewState::Opening { .. })
            )
            || self.startup_open_path_resolve_pending.is_some()
            || self.archive_convert.is_some()
            || self.remote_session_blocks_local_control()
    }

    fn next_saved_group_open_id(&mut self) -> u64 {
        self.saved_group_open_sequence = self.saved_group_open_sequence.wrapping_add(1).max(1);
        self.saved_group_open_sequence
    }

    fn book_action_current_folder(&self) -> Option<PathBuf> {
        if !matches!(
            self.top_level_grid_view.surface(),
            super::top_level_grid_view::TopLevelGridSurface::Folder
        ) {
            return None;
        }
        self.current_folder.as_ref().and_then(|folder| {
            crate::books::is_direct_book_folder(&self.book_root_path(), folder)
                .then(|| folder.clone())
        })
    }

    fn start_saved_book_scan(
        &mut self,
        transition: &mut SavedGroupOpenTransition,
        path: PathBuf,
    ) -> Result<(), String> {
        let include_convertible = !self.settings.archive_file_handling_ignores_convertible();
        let show_hidden = self.settings.show_hidden_files;
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_path = path.clone();
        std::thread::Builder::new()
            .name("saved-book-open-scan".into())
            .spawn(move || {
                let result = super::folder_scan::scan_directory_with_convertible_archives_cancel(
                    &worker_path,
                    include_convertible,
                    show_hidden,
                    Some(&worker_cancel),
                );
                let _ = sender.send(result);
            })
            .map_err(|error| format!("本の読み込みを開始できません: {error}"))?;
        transition.lease.phase_progress(Instant::now(), "book_scan");
        transition.phase = SavedGroupOpenPhase::ScanningBook {
            path,
            receiver,
            cancel,
        };
        Ok(())
    }

    fn detach_saved_group_book_list(&mut self, id: u64) {
        if let Some(pending) = self.book_op_pending.as_mut()
            && let crate::books::BookOpIntent::List { navigation } = &mut pending.intent
            && *navigation == Some(id)
        {
            *navigation = None;
        }
    }

    pub(crate) fn cancel_saved_group_open_request(&mut self, id: u64) -> bool {
        if !self
            .saved_group_open
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            return false;
        }
        if let Some(mut request) = self.saved_group_open.take() {
            request.lease.finish(Instant::now(), "cancelled");
        }
        self.detach_saved_group_book_list(id);
        true
    }

    pub(crate) fn cancel_saved_group_open_for_other_nav(&mut self) {
        if let Some(id) = self.saved_group_open.as_ref().map(|request| request.id) {
            self.cancel_saved_group_open_request(id);
        }
    }

    /// Retire an older prepared open before frame-tail polling consumes a newer async
    /// navigation's terminal result, including a failed or cancelled result.
    pub(crate) fn retire_saved_group_open_if_replaced(&mut self) {
        if self.sidecar_restore_active() {
            return;
        }
        let replaced = self.saved_group_open.as_ref().is_some_and(|request| {
            self.smart_folder_source_lease().as_ref() != Some(&request.source)
                || self.saved_group_other_main_nav_pending()
        });
        if replaced {
            self.cancel_saved_group_open_for_other_nav();
        }
    }

    pub(crate) fn saved_group_open_modal_visible(&self) -> bool {
        !self.sidecar_restore_active()
            && self.saved_group_open.as_ref().is_some_and(|request| {
                matches!(
                    request.phase,
                    SavedGroupOpenPhase::WaitingBookList
                        | SavedGroupOpenPhase::ScanningBook { .. }
                        | SavedGroupOpenPhase::ReadyBook { .. }
                        | SavedGroupOpenPhase::WaitingCollectionCatalog { .. }
                        | SavedGroupOpenPhase::ReadyCollection { .. }
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn saved_group_open_request_id_for_test(&self) -> Option<u64> {
        self.saved_group_open.as_ref().map(|request| request.id)
    }

    pub(crate) fn render_saved_group_open_modal(&mut self, ctx: &egui::Context) {
        // The existing sidecar modal owns this interval; after its terminal, the same source
        // lease is checked again before our modal becomes visible or a ready candidate is offered.
        if !self.saved_group_open_modal_visible() {
            return;
        }
        let Some(request) = self.saved_group_open.as_ref() else {
            return;
        };
        let id = request.id;
        let label = match request.phase {
            SavedGroupOpenPhase::WaitingBookList => "本棚一覧を読み込み中...",
            SavedGroupOpenPhase::ScanningBook { .. } | SavedGroupOpenPhase::ReadyBook { .. } => {
                "本を開く準備中..."
            }
            SavedGroupOpenPhase::WaitingCollectionCatalog { .. }
            | SavedGroupOpenPhase::ReadyCollection { .. } => "コレクション一覧を読み込み中...",
        };
        let mut cancel = false;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        egui::Modal::new(egui::Id::new("saved_group_open_modal")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading(label);
            });
            ui.add_space(8.0);
            cancel = ui.button("中止").clicked();
        });
        if cancel || escape_pressed {
            self.cancel_saved_group_open_request(id);
        }
    }

    pub(crate) fn apply_saved_group_key_action(
        &mut self,
        ctx: &egui::Context,
        action: KeyAction,
    ) -> Option<AddressBarNav> {
        match action {
            KeyAction::GridManageFavorites => {
                self.show_favorites_editor = true;
                return None;
            }
            KeyAction::GridManageBooks => {
                self.open_book_manager_from_action();
                return None;
            }
            KeyAction::GridManageCollections => {
                self.open_collection_manager(None);
                return None;
            }
            _ => {}
        }
        if self.is_snapshot_active() {
            self.show_feedback_toast("スナップショット中は他のフォルダに移動できません".into());
            return None;
        }
        if self.sidecar_restore_active() {
            self.show_feedback_toast("復元処理が進行中です".into());
            return None;
        }
        if self.saved_group_open.is_some() || self.saved_group_other_main_nav_pending() {
            self.show_feedback_toast("別の場所を開く処理が進行中です".into());
            return None;
        }
        let Some(source) = self.smart_folder_source_lease() else {
            self.show_feedback_toast(
                saved_group_source_unavailable_message(
                    self.projected_viewer_context_id() == self.viewer_context_main(),
                )
                .into(),
            );
            return None;
        };
        let current_book = self.book_action_current_folder();
        let current_collection = self
            .top_level_grid_view
            .collection_session()
            .map(|session| session.identity.collection_id);
        let requested_collection_target = self
            .settings
            .toolbar_collection_target_id
            .map(CollectionId::from_uuid);
        if action == KeyAction::GridOpenActiveBook
            || action == KeyAction::GridBookPrev
            || action == KeyAction::GridBookNext
            || action.book_slot_number().is_some()
        {
            if self.book_op_pending.as_ref().is_some_and(|pending| {
                !matches!(pending.intent, crate::books::BookOpIntent::List { .. })
            }) {
                self.show_feedback_toast("本棚の更新処理が進行中です".into());
                return None;
            }
            let id = self.next_saved_group_open_id();
            let lease = crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::viewer(
                    "saved_group",
                    self.collection_grid_context_id().serial(),
                    self.top_level_grid_view.generation(),
                ),
                Instant::now(),
                "book_list",
            );
            let mut transition = SavedGroupOpenTransition {
                id,
                action,
                source,
                current_book,
                current_collection,
                requested_collection_target,
                lease,
                phase: SavedGroupOpenPhase::WaitingBookList,
            };
            let target = if action == KeyAction::GridOpenActiveBook {
                Some(self.active_book_folder_path())
            } else if let Some(pending) = self.book_op_pending.as_mut() {
                if let crate::books::BookOpIntent::List { navigation } = &mut pending.intent {
                    *navigation = Some(id);
                    None
                } else {
                    self.show_feedback_toast("本棚の更新処理が進行中です".into());
                    return None;
                }
            } else if let Some(rows) = self.book_list_cache.as_ref() {
                match book_action_target(rows, action, transition.current_book.as_ref()) {
                    Ok(path) => Some(path),
                    Err(message) => {
                        self.show_feedback_toast(message);
                        return None;
                    }
                }
            } else {
                self.request_book_list_refresh();
                if let Some(pending) = self.book_op_pending.as_mut()
                    && let crate::books::BookOpIntent::List { navigation } = &mut pending.intent
                {
                    *navigation = Some(id);
                    None
                } else {
                    self.show_feedback_toast("本棚一覧を読み込めませんでした".into());
                    return None;
                }
            };
            if let Some(path) = target
                && let Err(message) = self.start_saved_book_scan(&mut transition, path)
            {
                self.show_feedback_toast(message);
                return None;
            }
            self.saved_group_open = Some(transition);
            ctx.request_repaint();
            return None;
        }
        if action == KeyAction::GridOpenCollectionTarget
            || action == KeyAction::GridCollectionPrev
            || action == KeyAction::GridCollectionNext
            || action.collection_slot_number().is_some()
        {
            if let Ok((ids, target)) = self.collection_action_catalog() {
                let mut lease = crate::collection_store::CollectionReadLease::new(
                    crate::collection_store::CollectionReadScope::viewer(
                        "saved_group",
                        self.collection_grid_context_id().serial(),
                        self.top_level_grid_view.generation(),
                    ),
                    Instant::now(),
                    "catalog_ready",
                );
                let target = if action == KeyAction::GridOpenCollectionTarget {
                    requested_collection_target
                } else {
                    target
                };
                match collection_action_target(&ids, target, action, current_collection) {
                    Ok(id) => {
                        lease.finish(Instant::now(), "adopted");
                        return Some(AddressBarNav::CollectionOpen(id));
                    }
                    Err(message) => self.show_feedback_toast(message),
                }
                lease.finish(Instant::now(), "rejected");
                return None;
            }
            let id = self.next_saved_group_open_id();
            let lease = crate::collection_store::CollectionReadLease::new(
                crate::collection_store::CollectionReadScope::viewer(
                    "saved_group",
                    self.collection_grid_context_id().serial(),
                    self.top_level_grid_view.generation(),
                ),
                Instant::now(),
                "catalog",
            );
            self.saved_group_open = Some(SavedGroupOpenTransition {
                id,
                action,
                source,
                current_book,
                current_collection,
                requested_collection_target,
                lease,
                phase: SavedGroupOpenPhase::WaitingCollectionCatalog {
                    requested: self.collection_action_catalog_read_pending(),
                },
            });
            ctx.request_repaint();
        }
        None
    }

    pub(crate) fn poll_saved_group_open(&mut self, ctx: &egui::Context) {
        let Some(mut request) = self.saved_group_open.take() else {
            return;
        };
        let now = Instant::now();
        if self.sidecar_restore_active() {
            let delay = request
                .lease
                .completion_poll_delay()
                .unwrap_or(Duration::from_millis(50));
            self.saved_group_open = Some(request);
            ctx.request_repaint_after(delay);
            return;
        }
        if self.smart_folder_source_lease().as_ref() != Some(&request.source)
            || self.saved_group_other_main_nav_pending()
        {
            request.lease.finish(now, "replaced");
            self.detach_saved_group_book_list(request.id);
            return;
        }
        match &mut request.phase {
            SavedGroupOpenPhase::WaitingBookList => {
                if self.book_op_pending.as_ref().is_some_and(|pending| {
                    matches!(
                        pending.intent,
                        crate::books::BookOpIntent::List { navigation: Some(id) }
                            if id == request.id
                    )
                }) {
                    if let Some(delay) = request.lease.completion_poll_delay() {
                        ctx.request_repaint_after(delay);
                    }
                } else if let Some(rows) = self.book_list_cache.as_ref() {
                    match book_action_target(rows, request.action, request.current_book.as_ref()) {
                        Ok(path) => {
                            if let Err(message) = self.start_saved_book_scan(&mut request, path) {
                                self.show_feedback_toast(message);
                                return;
                            }
                        }
                        Err(message) => {
                            self.show_feedback_toast(message);
                            return;
                        }
                    }
                } else {
                    self.show_feedback_toast("本棚一覧を読み込めませんでした".into());
                    return;
                }
            }
            SavedGroupOpenPhase::ScanningBook { receiver, .. } => match receiver.try_recv() {
                Ok(Ok(scan)) => {
                    let path = match std::mem::replace(
                        &mut request.phase,
                        SavedGroupOpenPhase::WaitingBookList,
                    ) {
                        SavedGroupOpenPhase::ScanningBook { path, .. } => path,
                        _ => unreachable!(),
                    };
                    request.lease.phase_progress(now, "ready");
                    request.phase = SavedGroupOpenPhase::ReadyBook { path, scan };
                }
                Ok(Err(error)) => {
                    self.show_feedback_toast(format!("本を読み込めませんでした: {error}"));
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.show_feedback_toast("本の読み込みが中断されました".into());
                    return;
                }
                Err(TryRecvError::Empty) => {
                    if let Some(delay) = request.lease.completion_poll_delay() {
                        ctx.request_repaint_after(delay);
                    }
                }
            },
            SavedGroupOpenPhase::WaitingCollectionCatalog { requested } => {
                match self.poll_collection_action_catalog(requested) {
                    Ok(Some((ids, target))) => match collection_action_target(
                        &ids,
                        if request.action == KeyAction::GridOpenCollectionTarget {
                            request.requested_collection_target
                        } else {
                            target
                        },
                        request.action,
                        request.current_collection,
                    ) {
                        Ok(id) => {
                            request.lease.phase_progress(now, "ready");
                            request.phase = SavedGroupOpenPhase::ReadyCollection { id };
                        }
                        Err(message) => {
                            self.show_feedback_toast(message);
                            return;
                        }
                    },
                    Ok(None) => {
                        if let Some(delay) = request.lease.completion_poll_delay() {
                            ctx.request_repaint_after(delay);
                        }
                    }
                    Err(message) => {
                        self.show_feedback_toast(message.into());
                        return;
                    }
                }
            }
            SavedGroupOpenPhase::ReadyBook { .. } | SavedGroupOpenPhase::ReadyCollection { .. } => {
            }
        }
        self.saved_group_open = Some(request);
    }

    pub(crate) fn saved_group_ready_nav(&self) -> Option<AddressBarNav> {
        if self.sidecar_restore_active() || self.saved_group_other_main_nav_pending() {
            return None;
        }
        self.saved_group_open.as_ref().and_then(|request| {
            if self.smart_folder_source_lease().as_ref() != Some(&request.source) {
                return None;
            }
            matches!(
                request.phase,
                SavedGroupOpenPhase::ReadyBook { .. } | SavedGroupOpenPhase::ReadyCollection { .. }
            )
            .then_some(AddressBarNav::SavedGroupReady(request.id))
        })
    }

    pub(crate) fn adopt_saved_group_ready(&mut self, id: u64) {
        let Some(mut request) = self.saved_group_open.take() else {
            return;
        };
        if request.id != id {
            self.saved_group_open = Some(request);
            return;
        }
        if self.sidecar_restore_active() {
            self.saved_group_open = Some(request);
            return;
        }
        if self.smart_folder_source_lease().as_ref() != Some(&request.source)
            || self.saved_group_other_main_nav_pending()
        {
            return;
        }
        match std::mem::replace(&mut request.phase, SavedGroupOpenPhase::WaitingBookList) {
            SavedGroupOpenPhase::ReadyBook { path, scan } => {
                request.lease.finish(Instant::now(), "adopted");
                self.bump_input_seq("grid-book-key", Some(&format!("{:?}", request.action)));
                if !self.load_folder_with_scan_owned(
                    path,
                    Some(scan),
                    super::OpenRequestOwner::Navigation,
                ) {
                    self.show_feedback_toast("本を開けませんでした".into());
                }
            }
            SavedGroupOpenPhase::ReadyCollection { id } => {
                let Ok((ids, target)) = self.collection_action_catalog() else {
                    request.lease.phase_progress(Instant::now(), "catalog");
                    request.phase =
                        SavedGroupOpenPhase::WaitingCollectionCatalog { requested: true };
                    self.saved_group_open = Some(request);
                    return;
                };
                let Ok(current_id) = collection_action_target(
                    &ids,
                    if request.action == KeyAction::GridOpenCollectionTarget {
                        request.requested_collection_target
                    } else {
                        target
                    },
                    request.action,
                    request.current_collection,
                ) else {
                    self.show_feedback_toast("コレクションを開けませんでした".into());
                    return;
                };
                if current_id != id {
                    crate::logger::log(format!(
                        "saved collection open retargeted after catalog update {:?} -> {:?}",
                        id, current_id
                    ));
                }
                request.lease.finish(Instant::now(), "adopted");
                self.bump_input_seq("grid-collection-key", Some(&format!("{current_id:?}")));
                self.open_collection_grid_from_navigation(current_id);
            }
            _ => self.saved_group_open = Some(request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SavedGroupOpenPhase, book_action_target, collection_action_target, ordered_target_index,
        saved_group_source_unavailable_message,
    };
    use crate::keymap::KeyAction;
    use std::time::{Duration, Instant};

    #[test]
    fn cycle_wraps_and_unknown_current_uses_the_directional_edge() {
        assert_eq!(ordered_target_index(0, None, true), None);
        assert_eq!(ordered_target_index(3, Some(2), true), Some(0));
        assert_eq!(ordered_target_index(3, Some(0), false), Some(2));
        assert_eq!(ordered_target_index(3, None, true), Some(0));
        assert_eq!(ordered_target_index(3, None, false), Some(2));
    }

    #[test]
    fn numbered_targets_follow_management_order_after_delete_and_book_rename() {
        let mut ids = (0..21)
            .map(|_| crate::collection_store::CollectionId::new())
            .collect::<Vec<_>>();
        assert_eq!(
            collection_action_target(&ids, None, KeyAction::GridOpenCollection1, None).unwrap(),
            ids[0]
        );
        assert_eq!(
            collection_action_target(&ids, None, KeyAction::GridOpenCollection20, None).unwrap(),
            ids[19]
        );
        let former_second = ids[1];
        let former_twenty_first = ids[20];
        ids.remove(0);
        assert_eq!(
            collection_action_target(&ids, None, KeyAction::GridOpenCollection1, None).unwrap(),
            former_second
        );
        assert_eq!(
            collection_action_target(&ids, None, KeyAction::GridOpenCollection20, None).unwrap(),
            former_twenty_first
        );

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("Alpha")).unwrap();
        std::fs::create_dir_all(root.join("Beta")).unwrap();
        let rows = crate::books::list_books(&root).unwrap();
        assert_eq!(
            book_action_target(&rows, KeyAction::GridOpenBook1, None)
                .unwrap()
                .file_name()
                .unwrap(),
            "Alpha"
        );
        std::fs::rename(root.join("Alpha"), root.join("Zulu")).unwrap();
        let rows = crate::books::list_books(&root).unwrap();
        assert_eq!(
            book_action_target(&rows, KeyAction::GridOpenBook1, None)
                .unwrap()
                .file_name()
                .unwrap(),
            "Beta"
        );
    }

    #[test]
    fn book_manager_refresh_retains_cached_rows_until_fresh_result_arrives() {
        let mut app = crate::app::setup_app_for_test();
        let old = crate::books::BookInfo {
            name: "Cached".into(),
            path: app.tmp.path().join("Cached"),
            page_count: 3,
        };
        app.book_list_cache = Some(vec![old]);
        let (sender, receiver) = std::sync::mpsc::channel();
        app.book_op_pending = Some(crate::books::BookOpPending {
            rx: receiver,
            intent: crate::books::BookOpIntent::List { navigation: None },
        });

        app.open_book_manager_from_action();

        assert!(app.show_book_manager);
        assert_eq!(app.book_list_cache.as_ref().unwrap()[0].name, "Cached");
        sender
            .send(Ok(crate::books::BookOpResult::List(vec![
                crate::books::BookInfo {
                    name: "Fresh".into(),
                    path: app.tmp.path().join("Fresh"),
                    page_count: 4,
                },
            ])))
            .unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert_eq!(app.book_list_cache.as_ref().unwrap()[0].name, "Fresh");
    }

    #[test]
    fn active_book_ui_entry_uses_the_key_owned_modal_transition() {
        let mut app = crate::app::setup_app_for_test();
        let root = app.tmp.path().join("books");
        std::fs::create_dir_all(root.join("Current")).unwrap();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "Current".into();

        app.open_active_book_from_action(&egui::Context::default());

        let request = app.saved_group_open.as_ref().unwrap();
        assert_eq!(request.action, KeyAction::GridOpenActiveBook);
        assert!(matches!(
            request.phase,
            SavedGroupOpenPhase::ScanningBook { .. }
        ));
        assert!(app.saved_group_open_modal_visible());
    }

    #[test]
    fn saved_group_escape_is_ime_aware_and_cancels_only_the_captured_request() {
        let mut app = crate::app::setup_app_for_test();
        let root = app.tmp.path().join("books");
        std::fs::create_dir_all(root.join("Current")).unwrap();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "Current".into();
        let ime_ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ime_ctx);
        app.open_active_book_from_action(&ime_ctx);
        let first_id = app.saved_group_open_request_id_for_test().unwrap();

        let _ = ime_ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("未確定".into())),
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: Some(egui::Key::Escape),
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                app.update_ime_state(ctx);
                app.render_saved_group_open_modal(ctx);
            },
        );
        assert_eq!(app.saved_group_open_request_id_for_test(), Some(first_id));
        assert_eq!(app.modal_dialog_block_reason(), Some("saved_group_open"));

        assert!(app.cancel_saved_group_open_request(first_id));
        let escape_ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&escape_ctx);
        app.open_active_book_from_action(&escape_ctx);
        let second_id = app.saved_group_open_request_id_for_test().unwrap();
        assert_ne!(first_id, second_id);
        assert!(!app.cancel_saved_group_open_request(first_id));
        let _ = escape_ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: Some(egui::Key::Escape),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.render_saved_group_open_modal(ctx),
        );
        assert!(app.saved_group_open.is_none());
        assert!(!app.saved_group_open_modal_visible());
    }

    #[test]
    fn long_running_saved_group_wait_keeps_modal_until_exact_request_is_cancelled() {
        let mut app = crate::app::setup_app_for_test();
        app.collection_ui.set_read_phase_for_test(true);
        let ctx = egui::Context::default();

        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenCollection1);
        let scope = crate::collection_store::CollectionReadScope::viewer(
            "saved_group",
            app.collection_grid_context_id().serial(),
            app.top_level_grid_view.generation(),
        );
        let request = app
            .saved_group_open
            .as_mut()
            .expect("collection open must wait for the shared catalog");
        let request_id = request.id;
        request.lease = crate::collection_store::CollectionReadLease::new(
            scope,
            Instant::now()
                .checked_sub(Duration::from_secs(24 * 60 * 60))
                .unwrap(),
            "catalog",
        );
        let lease_id = request.lease.request_id();

        app.poll_saved_group_open(&ctx);

        assert_eq!(app.saved_group_open_request_id_for_test(), Some(request_id));
        assert!(app.saved_group_open_modal_visible());
        assert!(app.saved_group_open.as_ref().is_some_and(|request| {
            request.lease.request_id() == lease_id
                && matches!(
                    request.phase,
                    SavedGroupOpenPhase::WaitingCollectionCatalog { .. }
                )
        }));
        assert!(matches!(
            app.collection_store_client_for_read(),
            Err(crate::collection_store::CollectionStoreError::Starting)
        ));
        assert!(app.cancel_saved_group_open_request(request_id));
        assert!(app.saved_group_open.is_none());
        assert!(!app.saved_group_open_modal_visible());
        app.poll_saved_group_open(&ctx);
        assert!(app.saved_group_open.is_none());
        assert_ne!(request_id, 0);
        assert_ne!(lease_id, 0);
    }

    #[test]
    fn saved_group_source_failure_explains_main_window_scope() {
        assert_eq!(
            saved_group_source_unavailable_message(false),
            "この操作はメインウィンドウでのみ使用できます"
        );
        assert_eq!(
            saved_group_source_unavailable_message(true),
            "メインの表示状態を確認できません"
        );
    }

    #[test]
    fn numbered_book_open_attaches_to_cold_list_and_adopts_once_from_full_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        let alpha = root.join("Alpha");
        let zulu = root.join("Zulu");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&zulu).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.pinned_books = vec!["Zulu".into()];
        app.book_list_cache = None;
        let ctx = egui::Context::default();
        let old_generation = app.top_level_grid_view.generation();
        let old_history = app.folder_nav_back_stack.len();
        assert!(
            app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenBook1)
                .is_none()
        );
        let id = app.saved_group_open.as_ref().unwrap().id;
        assert!(app.saved_group_open_modal_visible());
        assert!(matches!(
            app.book_op_pending.as_ref().unwrap().intent,
            crate::books::BookOpIntent::List {
                navigation: Some(owner)
            } if owner == id
        ));
        for _ in 0..100 {
            app.poll_book_op_pending(&ctx);
            app.poll_saved_group_open(&ctx);
            if matches!(
                app.saved_group_open.as_ref().map(|request| &request.phase),
                Some(SavedGroupOpenPhase::ReadyBook { .. })
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            app.saved_group_open.as_ref().map(|request| &request.phase),
            Some(SavedGroupOpenPhase::ReadyBook { path, .. }) if path == &alpha
        ));
        assert_eq!(app.top_level_grid_view.generation(), old_generation);
        assert_eq!(app.folder_nav_back_stack.len(), old_history);
        assert!(matches!(
            app.saved_group_ready_nav(),
            Some(crate::ui_main::AddressBarNav::SavedGroupReady(owner)) if owner == id
        ));
        app.adopt_saved_group_ready(id);
        assert!(app.saved_group_open.is_none());
        assert!(
            app.current_folder
                .as_ref()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &alpha))
        );
    }

    #[test]
    fn ready_book_waits_for_sidecar_modal_then_rechecks_source_before_adoption() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        let book = root.join("First");
        std::fs::create_dir_all(&book).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "First".into();
        let ctx = egui::Context::default();
        let before = app.current_folder.clone();
        let history = app.folder_nav_back_stack.len();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        let id = app.saved_group_open_request_id_for_test().unwrap();
        for _ in 0..100 {
            app.poll_saved_group_open(&ctx);
            if matches!(
                app.saved_group_open.as_ref().map(|request| &request.phase),
                Some(SavedGroupOpenPhase::ReadyBook { .. })
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.saved_group_ready_nav().is_some());

        app.activate_sidecar_restore_modal_for_test(temp.path().to_path_buf());
        assert!(!app.saved_group_open_modal_visible());
        assert!(app.saved_group_ready_nav().is_none());
        app.poll_saved_group_open(&ctx);
        app.adopt_saved_group_ready(id);
        assert_eq!(app.saved_group_open_request_id_for_test(), Some(id));
        assert_eq!(app.current_folder, before);
        assert_eq!(app.folder_nav_back_stack.len(), history);

        app.sidecar_restore = None;
        app.poll_saved_group_open(&ctx);
        assert!(app.saved_group_ready_nav().is_some());
        app.adopt_saved_group_ready(id);
        assert!(app.saved_group_open.is_none());
        assert!(
            app.current_folder
                .as_ref()
                .is_some_and(|path| crate::folder_tree::path_eq(path, &book))
        );
    }

    #[test]
    fn sidecar_terminal_drops_book_request_if_source_changed_during_restore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("First")).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "First".into();
        let ctx = egui::Context::default();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        let history = app.folder_nav_back_stack.len();
        app.activate_sidecar_restore_modal_for_test(temp.path().to_path_buf());
        app.top_level_grid_view
            .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::DriveList);
        app.poll_saved_group_open(&ctx);
        assert!(app.saved_group_open.is_some());
        app.sidecar_restore = None;
        app.poll_saved_group_open(&ctx);
        assert!(app.saved_group_open.is_none());
        assert!(app.saved_group_ready_nav().is_none());
        assert_eq!(app.folder_nav_back_stack.len(), history);
    }

    #[test]
    fn ready_book_yields_candidate_to_later_pending_main_navigation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("First")).unwrap();
        let later = temp.path().join("later");
        std::fs::create_dir_all(&later).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "First".into();
        let ctx = egui::Context::default();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        for _ in 0..100 {
            app.poll_saved_group_open(&ctx);
            if app.saved_group_ready_nav().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.saved_group_ready_nav().is_some());
        let history = app.folder_nav_back_stack.len();
        app.start_folder_pane_open(later);
        assert!(app.folder_pane_open_pending.is_some());
        assert!(app.saved_group_ready_nav().is_none());
        assert_eq!(app.folder_nav_back_stack.len(), history);
        app.retire_saved_group_open_if_replaced();
        assert!(app.saved_group_open.is_none());
        app.cancel_folder_pane_open();
        assert!(app.saved_group_ready_nav().is_none());
    }

    #[test]
    fn cancelled_cold_book_request_only_warms_cache_and_late_nav_cannot_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("First")).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.book_list_cache = None;
        let ctx = egui::Context::default();
        let before = app.current_folder.clone();
        let history = app.folder_nav_back_stack.len();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenBook1);
        let id = app.saved_group_open.as_ref().unwrap().id;
        assert!(app.cancel_saved_group_open_request(id));
        assert!(matches!(
            app.book_op_pending.as_ref().unwrap().intent,
            crate::books::BookOpIntent::List { navigation: None }
        ));
        for _ in 0..100 {
            app.poll_book_op_pending(&ctx);
            if app.book_op_pending.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        app.poll_saved_group_open(&ctx);
        assert!(app.book_list_cache.is_some());
        assert!(app.saved_group_ready_nav().is_none());
        assert_eq!(app.current_folder, before);
        assert_eq!(app.folder_nav_back_stack.len(), history);
    }

    #[test]
    fn synthetic_old_book_marker_is_not_cycle_current_and_late_pane_nav_retires_owner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        let alpha = root.join("Alpha");
        let zulu = root.join("Zulu");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&zulu).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.current_folder = Some(alpha.clone());
        app.top_level_grid_view
            .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::DriveList);
        assert!(app.book_action_current_folder().is_none());
        let rows = vec![
            crate::books::BookInfo {
                name: "Alpha".into(),
                path: alpha,
                page_count: 0,
            },
            crate::books::BookInfo {
                name: "Zulu".into(),
                path: zulu.clone(),
                page_count: 0,
            },
        ];
        assert_eq!(
            book_action_target(
                &rows,
                KeyAction::GridBookPrev,
                app.book_action_current_folder().as_ref()
            )
            .unwrap(),
            zulu,
        );
        app.book_list_cache = Some(rows);
        let ctx = egui::Context::default();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridBookPrev);
        assert!(app.saved_group_open_modal_visible());
        app.start_folder_pane_open(temp.path().to_path_buf());
        app.poll_saved_group_open(&ctx);
        assert!(app.saved_group_open.is_none());
        assert!(app.saved_group_ready_nav().is_none());
    }

    #[test]
    fn configured_book_and_manage_keys_reach_the_grid_dispatcher() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("First")).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.book_list_cache = None;
        app.keymap = crate::keymap::Keymap::from_ini_str(
            "[Grid]\nGridOpenBook1 = F13\nGridManageFavorites = F14\n",
        );
        let ctx = egui::Context::default();
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        ctx.begin_pass(egui::RawInput {
            events: vec![press(egui::Key::F13)],
            ..Default::default()
        });
        assert!(app.handle_keyboard(&ctx).is_none());
        assert!(app.saved_group_open_modal_visible());
        let _ = ctx.end_pass();
        app.cancel_saved_group_open_for_other_nav();

        ctx.begin_pass(egui::RawInput {
            events: vec![press(egui::Key::F14)],
            ..Default::default()
        });
        assert!(app.handle_keyboard(&ctx).is_none());
        assert!(app.show_favorites_editor);
        let _ = ctx.end_pass();

        app.show_favorites_editor = false;
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F14,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        assert!(app.handle_keyboard(&ctx).is_none());
        assert!(
            !app.show_favorites_editor,
            "repeat must be claimed without reopening"
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn later_smart_root_request_retires_unadopted_book_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("Current")).unwrap();
        let source = temp.path().join("smart-source");
        std::fs::create_dir_all(&source).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "Current".into();
        let ctx = egui::Context::default();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        assert!(app.saved_group_open_modal_visible());
        let mut definition = crate::settings::SmartFolderDefinition::new("Later request");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            source,
            true,
            Default::default(),
        ));
        let smart_id = definition.id;
        app.settings.smart_folders = vec![definition];
        app.open_smart_folder_staged(smart_id, false);
        assert!(app.smart_folder_transition.is_some());
        app.poll_saved_group_open(&ctx);
        assert!(!app.saved_group_open_modal_visible());
        assert!(app.saved_group_ready_nav().is_none());
    }

    #[test]
    fn remote_control_acquisition_retires_unadopted_book_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        std::fs::create_dir_all(root.join("Current")).unwrap();
        let mut app = crate::app::setup_app_for_test();
        app.settings.book_root = Some(root);
        app.settings.active_book_name = "Current".into();
        let ctx = egui::Context::default();
        app.apply_saved_group_key_action(&ctx, KeyAction::GridOpenActiveBook);
        assert!(app.saved_group_open_modal_visible());
        let handle = crate::remote_ipc::session::SessionHandle::new();
        let response = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "saved-group-test".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: Some("test".into()),
            },
        });
        assert_eq!(response.status, mimageviewer_ipc::SessionStatus::Active);
        assert!(handle.finish_acquire(handle.snapshot().generation));
        app.set_remote_session_handle(handle);
        assert!(app.remote_session_blocks_local_control());
        app.poll_saved_group_open(&ctx);
        assert!(!app.saved_group_open_modal_visible());
        assert!(app.saved_group_ready_nav().is_none());
    }
}
