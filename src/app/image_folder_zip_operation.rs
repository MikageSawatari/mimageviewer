//! One process modal owns admission, resources, worker and terminal adoption.
use super::{App, ViewerContextId, metadata_import_refresh as refresh};
use crate::{image_folder_zip as zip, image_folder_zip_job as job, ui_image_folder_zip as ui};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

enum Purpose {
    FolderContext,
    CompositionGuide,
}
enum Completion {
    List { folder: Option<PathBuf> },
    Viewer,
}
#[derive(Debug)]
enum Exit {
    None,
    Root,
    #[cfg(windows)]
    TrayHide,
    OwnerRetired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitReason {
    Manifest,
    PriorSaves,
    SourceColumn,
    MetadataPreparation,
    Confirmation,
    ImageCopy,
    MetadataSaves,
    SourceKeys,
    SourceReads,
    SourceAdoption,
    ResumeRead,
    FavoriteCopy,
    ParentScan,
    ParentHydration,
    Terminal,
}
enum WorkerView {
    Preparing,
    Confirm(job::Summary),
    Copying(usize, usize, u64),
    Saving,
}
struct ColumnCapture {
    context: ViewerContextId,
    generation: u64,
    next: usize,
    names: Vec<String>,
}
struct SourceChunk {
    context: ViewerContextId,
    generation: u64,
    tags: Vec<(String, Vec<String>)>,
    pages: Vec<PageUpdate>,
    last: bool,
}
struct PageUpdate {
    index: usize,
    adjustment: Option<crate::adjustment::AdjustParams>,
    local: bool,
    crop: Option<crate::export_crop::CropSettings>,
    trim: Option<crate::view_trim::ViewTrimPageOverride>,
    mask: bool,
    conceal: bool,
    comic: bool,
    rotation: Option<crate::rotation_db::Rotation>,
}
struct SourceRefresh {
    snapshot: Option<crate::metadata_transfer::ImportPageStateSnapshot>,
    errors: Vec<String>,
    chunks: std::collections::VecDeque<SourceChunk>,
}
impl SourceRefresh {
    // Split on the worker; UI adoption never walks an entire book in one frame.
    fn prepare(result: refresh::RefreshResult) -> Self {
        let mut chunks = std::collections::VecDeque::new();
        for mut context in result.contexts {
            let mut tags = context
                .tags_cache
                .take()
                .unwrap_or_default()
                .into_iter()
                .peekable();
            let mut page = context.page_state.take();
            let mut indices = page
                .as_mut()
                .map(|p| std::mem::take(&mut p.thumbnail_reset_indices))
                .unwrap_or_default()
                .into_iter()
                .peekable();
            while tags.peek().is_some() || indices.peek().is_some() {
                let mut chunk = SourceChunk {
                    context: context.context_id,
                    generation: context.items_generation,
                    tags: vec![],
                    pages: vec![],
                    last: false,
                };
                for _ in 0..256 {
                    if let Some(tag) = tags.next() {
                        chunk.tags.push(tag);
                    } else if let Some(index) = indices.next() {
                        let p = page.as_mut().unwrap();
                        chunk.pages.push(PageUpdate {
                            index,
                            adjustment: p.adjustment_page_params.remove(&index),
                            local: p.local_adjust_pages.remove(&index),
                            crop: p.export_crop_page_settings.remove(&index),
                            trim: p.view_trim_page_overrides.remove(&index),
                            mask: p.mask_pages.remove(&index),
                            conceal: p.conceal_pages.remove(&index),
                            comic: p.comic_pages.remove(&index),
                            rotation: p.rotation_cache.remove(&index),
                        });
                    } else {
                        break;
                    }
                }
                chunk.last = tags.peek().is_none() && indices.peek().is_none();
                chunks.push_back(chunk);
            }
        }
        Self {
            snapshot: result.page_snapshot,
            errors: result.errors,
            chunks,
        }
    }
}
enum Phase {
    Preparing(mpsc::Receiver<Result<zip::ImageFolderManifest, String>>),
    Waiting(zip::ImageFolderManifest),
    Capture {
        manifest: zip::ImageFolderManifest,
        pending: std::collections::VecDeque<ColumnCapture>,
        columns: Vec<Arc<Vec<String>>>,
    },
    Worker {
        worker: job::Worker,
        view: WorkerView,
        trims: Vec<(
            ViewerContextId,
            crate::ui_view_trim::PendingViewTrimTransfer,
        )>,
    },
    RefreshIndex {
        result: Result<(), String>,
        keys: std::collections::HashSet<String>,
        edits: std::collections::HashSet<String>,
    },
    Refresh {
        result: Result<(), String>,
        receiver: mpsc::Receiver<Option<SourceRefresh>>,
    },
    Apply {
        result: Result<(), String>,
        refresh: SourceRefresh,
    },
    AwaitCaches {
        result: Result<(), String>,
    },
    ParentScan(super::FolderPaneOpenPending),
    ParentHydration(mpsc::Receiver<()>),
    Result {
        message: String,
        success: bool,
    },
}
pub(crate) struct Operation {
    owner: ViewerContextId,
    purpose: Purpose,
    completion: Completion,
    source: PathBuf,
    destination: PathBuf,
    control: Arc<zip::ConversionControl>,
    _lease: Option<crate::remote_ipc::session::LocalModalLease>,
    exit: Exit,
    last_wait: Option<WaitReason>,
    phase: Phase,
}
impl App {
    pub(crate) fn start_image_folder_zip(
        &mut self,
        ctx: &egui::Context,
        source: PathBuf,
        notice: bool,
    ) -> bool {
        if !self.grid_item_input_allowed()
            || self.image_folder_zip_operation.is_some()
            || self
                .modal_dialog_block_reason()
                .is_some_and(|reason| reason != "grid_context_menu")
            || self.document_open_modal_admission_blocked()
        {
            return false;
        }
        let lease = if let Some(handle) = self.remote_session_handle() {
            let Some(lease) = handle.try_local_modal_lease_with_notice(
                "PC で ZIP を作成中です。完了してから接続してください。",
            ) else {
                return false;
            };
            Some(lease)
        } else {
            None
        };
        let destination = match zip::destination_for(&source) {
            Ok(path) => path,
            Err(error) => {
                self.show_feedback_toast(error);
                return false;
            }
        };
        let owner = self.projected_viewer_context_id();
        let mut op = Operation {
            owner,
            completion: if owner == self.viewer_context_main() {
                Completion::List {
                    folder: self.current_folder.clone(),
                }
            } else {
                Completion::Viewer
            },
            purpose: if notice {
                Purpose::CompositionGuide
            } else {
                Purpose::FolderContext
            },
            source,
            destination,
            control: Default::default(),
            _lease: lease,
            exit: Exit::None,
            last_wait: None,
            phase: Phase::Result {
                message: String::new(),
                success: false,
            },
        };
        op.prepare(ctx, &self.settings);
        self.image_folder_zip_operation = Some(Box::new(op));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        true
    }
    pub(crate) fn cancel_image_folder_zip_for_context(&mut self, id: ViewerContextId) {
        if let Some(op) = self
            .image_folder_zip_operation
            .as_mut()
            .filter(|op| op.owner == id)
        {
            op.exit = Exit::OwnerRetired;
            op.cancel();
        }
    }
    #[cfg(test)]
    pub(crate) fn defer_image_folder_zip_root_close(&mut self, ctx: &egui::Context) {
        if ctx.input_for(egui::ViewportId::ROOT, |input| {
            input.viewport().close_requested()
        }) {
            let intent = if self.sidecar_restore_root_lifecycle_close_requested() {
                super::root_lifecycle::RootCloseIntent::ApplicationQuit
            } else {
                super::root_lifecycle::RootCloseIntent::WindowClose
            };
            self.accept_image_folder_zip_root_close(ctx, intent);
        }
    }

    pub(super) fn accept_image_folder_zip_root_close(
        &mut self,
        ctx: &egui::Context,
        intent: super::root_lifecycle::RootCloseIntent,
    ) -> bool {
        use super::root_lifecycle::RootCloseIntent;
        let Some(op) = self.image_folder_zip_operation.as_mut() else {
            return false;
        };
        op.exit = Exit::Root;
        #[cfg(windows)]
        if matches!(intent, RootCloseIntent::TrayHide)
            || (matches!(intent, RootCloseIntent::WindowClose)
                && self.settings.minimize_to_tray_on_close
                && self.tray_controller.is_some())
        {
            op.exit = Exit::TrayHide;
        }
        #[cfg(not(windows))]
        let _ = intent;
        op.cancel();
        crate::logger::log(format!(
            "folder_zip_close owner={:?} wait={:?} exit={:?}",
            op.owner, op.last_wait, op.exit,
        ));
        let hydration_owner = matches!(op.phase, Phase::ParentHydration(_)).then_some(op.owner);
        if let Some(owner) = hydration_owner {
            self.cancel_image_zip_hydration_for_exit(owner);
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
        true
    }

    /// Only an existing cancellation/exit owner may run through the non-drawing service.
    pub(super) fn service_image_folder_zip_exit(&mut self, ctx: &egui::Context) {
        if self
            .image_folder_zip_operation
            .as_ref()
            .is_some_and(|op| !matches!(op.exit, Exit::None))
        {
            self.poll_image_folder_zip(ctx);
        }
    }

    pub(crate) fn poll_image_folder_zip(&mut self, ctx: &egui::Context) {
        let Some(mut op) = self.image_folder_zip_operation.take() else {
            return;
        };
        let phase = std::mem::replace(
            &mut op.phase,
            Phase::Result {
                message: String::new(),
                success: false,
            },
        );
        op.phase = match phase {
            Phase::Preparing(receiver) => match receiver.try_recv() {
                Ok(Ok(manifest)) => Phase::Waiting(manifest),
                Ok(Err(error)) => op.failed(error),
                Err(mpsc::TryRecvError::Empty) => Phase::Preparing(receiver),
                Err(_) => op.failed("変換の準備処理が終了しました。".into()),
            },
            Phase::Waiting(manifest) => {
                if op.control.is_cancelled() {
                    op.failed("変換を取り消しました。".into())
                } else if self.image_zip_prior_owners_busy() {
                    Phase::Waiting(manifest)
                } else {
                    self.flush_favorite_view_writes_for_exit();
                    match self.metadata_transfer_writers_busy(true) {
                        Ok(true) => Phase::Waiting(manifest),
                        Err(error) => op.failed(error),
                        Ok(false)
                            if self
                                .favorite_view_store_writer
                                .as_ref()
                                .is_some_and(|w| w.is_busy()) =>
                        {
                            Phase::Waiting(manifest)
                        }
                        Ok(false) => {
                            let pending =
                                self.viewer_context_ids()
                                    .into_iter()
                                    .filter_map(|id| {
                                        self.with_viewer_context_ref(id, |v| {
                                            (v.current_folder() == Some(op.source.as_path()))
                                                .then_some(ColumnCapture {
                                                    context: id,
                                                    generation: v.items_generation(),
                                                    next: 0,
                                                    names: vec![],
                                                })
                                        })
                                        .flatten()
                                    })
                                    .collect();
                            Phase::Capture {
                                manifest,
                                pending,
                                columns: vec![],
                            }
                        }
                    }
                }
            }
            Phase::Capture {
                manifest,
                mut pending,
                mut columns,
            } => {
                if op.control.is_cancelled() {
                    self.resume_metadata_transfer_context_readers();
                    op.failed("変換を取り消しました。".into())
                } else {
                    let mut budget = 2048;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(3);
                    while budget > 0 && std::time::Instant::now() < deadline {
                        let Some(mut capture) = pending.pop_front() else {
                            break;
                        };
                        let progress = self
                            .with_viewer_context_ref(capture.context, |viewer| {
                                if viewer.items_generation() != capture.generation {
                                    return None;
                                }
                                while capture.next < viewer.items().len()
                                    && budget > 0
                                    && std::time::Instant::now() < deadline
                                {
                                    let super::GridItem::Image(path) =
                                        &viewer.items()[capture.next]
                                    else {
                                        return None;
                                    };
                                    let name = path.strip_prefix(&op.source).ok()?.to_str()?;
                                    capture.names.push(name.to_owned());
                                    capture.next += 1;
                                    budget -= 1;
                                }
                                Some(capture.next == viewer.items().len())
                            })
                            .flatten();
                        match progress {
                            Some(true) => columns.push(Arc::new(capture.names)),
                            Some(false) => pending.push_front(capture),
                            None => {}
                        }
                    }
                    if pending.is_empty() {
                        match self.start_image_zip_worker(ctx, &op, manifest, columns) {
                            Ok(phase) => phase,
                            Err(error) => {
                                self.resume_metadata_transfer_context_readers();
                                op.failed(error)
                            }
                        }
                    } else {
                        Phase::Capture {
                            manifest,
                            pending,
                            columns,
                        }
                    }
                }
            }
            Phase::Worker {
                worker,
                mut view,
                trims,
            } => {
                match worker.events.try_recv() {
                    Ok(job::Event::Confirm(summary)) => {
                        view = WorkerView::Confirm(summary);
                        Phase::Worker {
                            worker,
                            view,
                            trims,
                        }
                    }
                    Ok(job::Event::Progress { images, bytes }) => {
                        let total = match &view {
                            WorkerView::Copying(_, total, _) => *total,
                            _ => images.max(1),
                        };
                        view = WorkerView::Copying(images, total, bytes);
                        Phase::Worker {
                            worker,
                            view,
                            trims,
                        }
                    }
                    Ok(job::Event::Committing) => Phase::Worker {
                        worker,
                        view: WorkerView::Saving,
                        trims,
                    },
                    Ok(job::Event::Finished { result, .. }) => {
                        // Terminal is sent strictly after the worker unlocks these resources.
                        let mut resources =
                            worker.resources.lock().unwrap_or_else(|e| e.into_inner());
                        self.sidecars = std::mem::take(&mut resources.sidecars);
                        self.tags_db = resources.tags.take();
                        for (id, pending) in trims.into_iter().skip(resources.saved_trims) {
                            let restore =
                                |app: &mut App| app.restore_pending_view_trim_transfer(pending);
                            let _ = if matches!(op.exit, Exit::None) {
                                self.with_viewer_context(id, restore)
                            } else {
                                self.with_viewer_context_state(id, restore)
                            };
                        }
                        let keys = std::mem::take(&mut resources.source_keys);
                        let edits = std::mem::take(&mut resources.source_edits);
                        drop(resources);
                        if let Ok(success) = &result {
                            for favorite in &success.favorites {
                                if !self.settings.favorites.iter().any(|f| f.id == favorite.id) {
                                    self.settings.favorites.push(favorite.clone());
                                }
                            }
                            for (id, params) in &success.favorite_params {
                                self.adjustment_favorite_params.insert(*id, params.clone());
                            }
                            self.reload_book_resume_meters();
                            self.invalidate_rating_counts_cache();
                            self.notify_bookmarks_changed();
                            self.invalidate_tag_apply_suggestions();
                        }
                        if matches!(op.exit, Exit::None) {
                            self.begin_image_zip_terminal_refresh();
                        } else {
                            self.begin_image_zip_terminal_refresh_state();
                        }
                        Phase::RefreshIndex {
                            result: result.map(|_| ()),
                            keys,
                            edits,
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => Phase::Worker {
                        worker,
                        view,
                        trims,
                    },
                    Err(_) => {
                        let mut resources =
                            worker.resources.lock().unwrap_or_else(|e| e.into_inner());
                        self.sidecars = std::mem::take(&mut resources.sidecars);
                        self.tags_db = resources.tags.take();
                        for (id, pending) in trims.into_iter().skip(resources.saved_trims) {
                            let restore =
                                |app: &mut App| app.restore_pending_view_trim_transfer(pending);
                            let _ = if matches!(op.exit, Exit::None) {
                                self.with_viewer_context(id, restore)
                            } else {
                                self.with_viewer_context_state(id, restore)
                            };
                        }
                        self.resume_metadata_transfer_context_readers();
                        op.failed(
                            "ZIP の保存処理が終了しました。元のフォルダは残っています。".into(),
                        )
                    }
                }
            }
            Phase::RefreshIndex {
                result,
                keys,
                edits,
            } => {
                let complete = if matches!(op.exit, Exit::None) {
                    self.advance_image_zip_terminal_refresh(&keys)
                } else {
                    self.advance_image_zip_terminal_refresh_state(&keys)
                };
                if !complete {
                    Phase::RefreshIndex {
                        result,
                        keys,
                        edits,
                    }
                } else {
                    let changed = crate::metadata_transfer::ImportChangedSections {
                        tags: true,
                        page_state: true,
                        ..Default::default()
                    };
                    let mut requests = if matches!(op.exit, Exit::None) {
                        self.take_metadata_import_refresh_requests(changed, &Default::default())
                    } else {
                        self.take_metadata_import_refresh_requests_state(
                            changed,
                            &Default::default(),
                        )
                    };
                    for request in &mut requests {
                        request.items.retain(|item| {
                            keys.contains(&item.key)
                                || item
                                    .alternate_page_key
                                    .as_ref()
                                    .is_some_and(|k| keys.contains(k))
                        });
                        for item in &mut request.items {
                            item.rating = false;
                            item.page = edits
                                .contains(item.alternate_page_key.as_ref().unwrap_or(&item.key));
                        }
                        request.pin_materialization = Default::default();
                        request.current_rating_key = None;
                        request.spread_container_path = None;
                        request.spread_container_fallback = None;
                        request.shift_read = None;
                    }
                    requests.retain(|r| !r.items.is_empty());
                    let (tx, receiver) = mpsc::channel();
                    let data_dir = self.rename_migration_data_dir();
                    let wake = ctx.clone();
                    match std::thread::Builder::new()
                        .name("folder-zip-refresh".into())
                        .spawn(move || {
                            let response = refresh::run(
                                data_dir,
                                requests,
                                changed,
                                &std::sync::atomic::AtomicBool::new(false),
                            );
                            let _ = tx.send(response.map(SourceRefresh::prepare));
                            wake.request_repaint_of(egui::ViewportId::ROOT);
                        }) {
                        Ok(_) => Phase::Refresh { result, receiver },
                        Err(error) => {
                            self.resume_metadata_transfer_context_readers();
                            op.failed(error.to_string())
                        }
                    }
                }
            }
            Phase::Refresh { result, receiver } => match receiver.try_recv() {
                Ok(Some(refresh)) => Phase::Apply { result, refresh },
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.resume_metadata_transfer_context_readers();
                    op.failed("表示の更新を完了できませんでした。".into())
                }
                Err(mpsc::TryRecvError::Empty) => Phase::Refresh { result, receiver },
            },
            Phase::Apply {
                mut result,
                mut refresh,
            } => {
                if let Some(snapshot) = refresh.snapshot.take() {
                    self.replace_metadata_import_page_state_snapshot(snapshot);
                }
                if !refresh.errors.is_empty() {
                    for error in refresh.errors.drain(..) {
                        crate::logger::log(format!("folder ZIP refresh: {error}"));
                    }
                    result = Err("ZIP の処理後に表示を更新できませんでした。元のフォルダを開き直してください。".into());
                }
                // One bounded chunk per frame, including context mounting.
                if let Some(chunk) = refresh.chunks.pop_front() {
                    if matches!(op.exit, Exit::None) {
                        self.apply_image_zip_source_chunk(chunk);
                    } else {
                        let last = chunk.last;
                        let _ = self.with_viewer_context_state(chunk.context, |app| {
                            if app.apply_image_zip_source_chunk_current(chunk) && last {
                                app.mark_metadata_import_display_reconciliation();
                            }
                        });
                    }
                }
                if refresh.chunks.is_empty() {
                    Phase::AwaitCaches { result }
                } else {
                    Phase::Apply { result, refresh }
                }
            }
            Phase::AwaitCaches { result } => {
                self.poll_book_resume_meters(ctx);
                if self.book_resume_meters.read_pending()
                    || self
                        .favorite_view_store_writer
                        .as_ref()
                        .is_some_and(|w| w.is_busy())
                {
                    Phase::AwaitCaches { result }
                } else {
                    self.resume_metadata_transfer_context_readers();
                    self.finish_image_zip_adoption(ctx, &op, result)
                }
            }
            Phase::ParentScan(mut pending) => {
                if !matches!(op.exit, Exit::None) {
                    pending.cancel_with_diagnostic("folder_zip_close");
                    op.success()
                } else {
                    match pending.rx.try_recv() {
                        Ok(Ok(scan)) => self.adopt_image_zip_parent_scan(&op, pending, scan),
                        Ok(Err(error)) => op.failed(format!("親一覧の読取: {error}")),
                        Err(mpsc::TryRecvError::Empty) => Phase::ParentScan(pending),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            op.failed("親のフォルダを読み取れませんでした。".into())
                        }
                    }
                }
            }
            Phase::ParentHydration(completed) => match completed.try_recv() {
                Ok(()) => op.success(),
                Err(mpsc::TryRecvError::Empty) => Phase::ParentHydration(completed),
                Err(mpsc::TryRecvError::Disconnected) if !matches!(op.exit, Exit::None) => {
                    if self.image_zip_discarded_hydration_exit_pending(op.owner) {
                        Phase::ParentHydration(completed)
                    } else {
                        op.success()
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => op.failed(
                    "親の一覧の更新を完了できませんでした。作成先を開き直してください。".into(),
                ),
            },
            phase => phase,
        };
        op.log_wait(self);
        if matches!(op.phase, Phase::Result { .. }) && !matches!(op.exit, Exit::None) {
            self.resume_metadata_transfer_context_readers();
            if let Phase::Result { message, .. } = &op.phase {
                self.show_feedback_toast(message.clone());
            }
            if matches!(op.exit, Exit::Root) {
                self.resume_root_lifecycle_close(
                    ctx,
                    super::root_lifecycle::RootCloseIntent::WindowClose,
                );
            }
            #[cfg(windows)]
            if matches!(op.exit, Exit::TrayHide) {
                self.resume_root_lifecycle_close(
                    ctx,
                    super::root_lifecycle::RootCloseIntent::TrayHide,
                );
            }
        } else {
            if !matches!(op.phase, Phase::Result { .. }) {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            self.image_folder_zip_operation = Some(op);
        }
    }
    fn image_zip_prior_owners_busy(&mut self) -> bool {
        if !self.local_adjust_write_pending.is_empty() {
            return true;
        }
        let ids = self.viewer_context_ids();
        ids.into_iter().any(|id| {
            self.with_viewer_context(id, |app| {
                app.folder_open_preparation_pending()
                    || app.sidecar_restore_active()
                    || app
                        .erase_inpaint_pending
                        .keys()
                        .any(|key| key.kind == crate::ui_erase::EraseInpaintKind::Commit)
                    || !app.comic_bake_pending.is_empty()
            })
            .unwrap_or(false)
        })
    }
    fn image_zip_target_sort(
        &mut self,
        op: &Operation,
    ) -> Result<crate::settings::SortOrder, String> {
        self.with_viewer_context(op.owner, |app| {
            // Capture and flush the owning viewer's current overlay, not the root's unrelated
            // favorite. Ancestor inheritance uses the existing transition resolver.
            app.flush_favorite_view_writes_for_exit();
            app.favorite_view_state_for_path(&op.source).sort_order
        })
        .map_err(|error| format!("folder ZIP source viewer unavailable: {error:?}"))
    }
    fn start_image_zip_worker(
        &mut self,
        ctx: &egui::Context,
        op: &Operation,
        manifest: zip::ImageFolderManifest,
        columns: Vec<Arc<Vec<String>>>,
    ) -> Result<Phase, String> {
        // Preparation has accepted a valid manifest. The metadata worker also
        // supplements source data before confirmation, so admission belongs
        // before its resource/queue handoff rather than the Convert button.
        if !self.admit_book_append_followup_mutation() {
            return Err("本への追加を取り消しています".into());
        }
        let mut trims = Vec::new();
        for id in self.viewer_context_ids() {
            if let Ok(Some(pending)) =
                self.with_viewer_context(id, |app| app.take_pending_view_trim_transfer())
            {
                trims.push((id, pending));
            }
        }
        let target_sort = self.image_zip_target_sort(op)?;
        let mut portals = Vec::new();
        if let Some(writer) = &self.book_resume_writer {
            portals.push(writer.folder_zip_portal(
                op.source.clone(),
                op.destination.clone(),
                target_sort,
                columns,
            ));
        }
        if let Some(writer) = &self.reading_history_writer {
            portals.push(writer.folder_zip_portal(op.source.clone(), op.destination.clone()));
        }
        if let Some(writer) = &self.book_bookmark_service {
            portals.push(writer.folder_zip_portal(
                op.source.clone(),
                op.destination.clone(),
                target_sort,
            ));
        }
        let mut favorite_pairs = Vec::new();
        for source in self
            .settings
            .favorites
            .iter()
            .filter(|f| crate::folder_tree::path_eq(&f.path, &op.source))
        {
            let mut target = source.clone();
            target.id = uuid::Uuid::new_v4();
            target.path = op.destination.clone();
            target.name = format!("{} (ZIP)", source.name);
            if let Some(existing) = self
                .settings
                .favorites
                .iter()
                .find(|f| crate::folder_tree::path_eq(&f.path, &op.destination))
            {
                target.id = existing.id;
            }
            favorite_pairs.push((source.clone(), target));
        }
        if let Some(writer) = &self.favorite_view_store_writer {
            portals.push(
                writer
                    .folder_zip_portal(favorite_pairs.iter().map(|(a, b)| (a.id, b.id)).collect()),
            );
        }
        let input = job::Inputs {
            data_dir: self.rename_migration_data_dir(),
            manifest,
            settings: Box::new(self.settings.clone()),
            session_ratings: self
                .rating_session_writes
                .iter()
                .map(|(key, value)| (key.clone(), value.stars))
                .collect(),
            portals,
            favorite_pairs,
            trims: trims.iter().map(|(_, p)| p.batch.clone()).collect(),
            observer: self
                .content_identity_recorder
                .as_ref()
                .map(|owner| owner.producer()),
        };
        let resources = Arc::new(Mutex::new(job::Resources::new(
            std::mem::take(&mut self.sidecars),
            self.tags_db.take(),
        )));
        match job::spawn(
            input,
            Arc::clone(&resources),
            Arc::clone(&op.control),
            ctx.clone(),
        ) {
            Ok(worker) => Ok(Phase::Worker {
                worker,
                view: WorkerView::Preparing,
                trims,
            }),
            Err(error) => {
                let mut resources = resources.lock().unwrap_or_else(|e| e.into_inner());
                self.sidecars = std::mem::take(&mut resources.sidecars);
                self.tags_db = resources.tags.take();
                for (id, pending) in trims {
                    let restore = |app: &mut App| app.restore_pending_view_trim_transfer(pending);
                    let _ = if matches!(op.exit, Exit::None) {
                        self.with_viewer_context(id, restore)
                    } else {
                        self.with_viewer_context_state(id, restore)
                    };
                }
                Err(error)
            }
        }
    }
    fn finish_image_zip_adoption(
        &mut self,
        _ctx: &egui::Context,
        op: &Operation,
        result: Result<(), String>,
    ) -> Phase {
        if let Err(error) = result {
            return op.failed(error);
        }
        if let Completion::List { folder } = &op.completion
            && matches!(op.exit, Exit::None)
        {
            let valid = self
                .with_viewer_context_ref(op.owner, |v| v.current_folder() == folder.as_deref())
                .unwrap_or(false);
            if valid {
                let _ = self.with_viewer_context(op.owner, |app| {
                    app.close_fullscreen();
                    app.start_folder_pane_open(op.source.parent().unwrap().to_owned());
                });
                // Move the accepted scan owner, including its navigation proof, into this
                // operation. The normal navigation tail never sees or consumes its receiver.
                return match self.folder_pane_open_pending.take() {
                    Some(pending) => Phase::ParentScan(pending),
                    None => op.failed("親のフォルダの読取を開始できませんでした。".into()),
                };
            }
        }
        op.success()
    }

    fn adopt_image_zip_parent_scan(
        &mut self,
        op: &Operation,
        pending: super::FolderPaneOpenPending,
        scan: super::ScannedDir,
    ) -> Phase {
        use super::*;
        if op.owner != self.projected_viewer_context_id() {
            return op.failed("一覧の表示先が変更されました。作成先を開き直してください。".into());
        }
        let Some(mut navigation) = pending.navigation else {
            return op.failed("親一覧の移動先を確認できませんでした。".into());
        };
        navigation.history =
            MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
                selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
            });
        let path = pending.path;
        let destination = self.main_list_requested_destination(
            &navigation,
            FolderNavHistoryTarget::Path(path.clone()),
            None,
        );
        let Some(adoption) = self.prepare_main_list_adoption(navigation, destination) else {
            return op.failed("元の一覧が変更されました。作成先を開き直してください。".into());
        };
        // A successful directory scan is the classification proof. Suffixes such as .epub
        // do not create a second classifier whose poll would be blocked by this modal.
        let now = std::time::Instant::now();
        let prepared = self.prepare_scanned_folder_listing_with_cache_keys(
            path.clone(),
            scan,
            FolderListingMetrics {
                seq: self.input_seq,
                started: now,
                scan_started: now,
                pre_scanned: true,
                path_display: String::new(),
            },
            StartupListIntent::ExplicitList,
            use_full_path_cache_keys_for_folder(&path),
        );
        let token = match self.prepare_visible_install(&path, &prepared.items, None) {
            Ok(token) => token,
            Err(reason) => return op.failed(format!("親一覧の採用: {reason:?}")),
        };
        let (completed, receipt) = mpsc::channel();
        self.adopt_main_list_navigation_with_completion(
            adoption,
            move |app| {
                app.install_admitted_physical_surface(&path, &OpenRequestOwner::Navigation, None);
                app.install_prepared_scanned_folder_listing(
                    prepared,
                    VisibleInstallAuthority::Ordinary,
                    token,
                )
            },
            Some(MainListAfterHydration::ImageFolderZip {
                destination: op.destination.clone(),
                completed,
            }),
        );
        Phase::ParentHydration(receipt)
    }
    fn apply_image_zip_source_chunk(&mut self, chunk: SourceChunk) {
        let last = chunk.last;
        let _ = self.with_viewer_context(chunk.context, |app| {
            if app.apply_image_zip_source_chunk_current(chunk) && last {
                app.rebuild_visible_indices_preserving_facet_scope();
            }
        });
    }

    /// Metadata ownership updates are independent of visible membership adoption.
    fn apply_image_zip_source_chunk_current(&mut self, chunk: SourceChunk) -> bool {
        if self.items_generation != chunk.generation {
            return false;
        }
        for (key, tags) in chunk.tags {
            self.set_tags_cache_entry(key, tags);
        }
        for page in chunk.pages {
            let idx = page.index;
            self.evict_thumbnail_for_reload(idx);
            self.bump_adjustment_generation(idx);
            self.bump_erase_mask_generation(idx);
            self.bump_local_adjust_generation(idx);
            self.adjustment_page_params.remove(&idx);
            if let Some(params) = page.adjustment {
                self.adjustment_page_params.insert(idx, params);
            }
            self.local_adjust_pages.remove(&idx);
            if page.local {
                self.local_adjust_pages.insert(idx);
            }
            self.local_adjust_page_layers.remove(&idx);
            self.local_adjust_selected_layers.remove(&idx);
            self.export_crop_pages.remove(&idx);
            self.export_crop_page_settings.remove(&idx);
            if let Some(crop) = page.crop {
                self.export_crop_pages.insert(idx);
                self.export_crop_page_settings.insert(idx, crop);
            }
            self.view_trim_page_overrides.remove(&idx);
            if let Some(trim) = page.trim {
                self.view_trim_page_overrides.insert(idx, trim);
            }
            for (set, enabled) in [
                (&mut self.mask_pages, page.mask),
                (&mut self.conceal_pages, page.conceal),
                (&mut self.comic_pages, page.comic),
            ] {
                set.remove(&idx);
                if enabled {
                    set.insert(idx);
                }
            }
            self.rotation_cache.remove(&idx);
            if let Some(rotation) = page.rotation {
                self.rotation_cache.insert(idx, rotation);
            }
        }
        true
    }
    pub(crate) fn show_image_folder_zip(&mut self, ctx: &egui::Context) {
        let Some(op) = self.image_folder_zip_operation.as_ref() else {
            return;
        };
        if op.owner != self.projected_viewer_context_id()
            && matches!(
                self.viewer_context_residence(op.owner),
                super::ContextResidence::Mounted | super::ContextResidence::AtRest
            )
        {
            ui::block_background(ctx);
            return;
        }
        let phase = match &op.phase {
            Phase::Preparing(_) => ui::Phase::Preparing,
            Phase::Waiting(_) | Phase::Capture { .. } => ui::Phase::Waiting,
            Phase::Worker { view, .. } => match view {
                WorkerView::Preparing => ui::Phase::Preparing,
                WorkerView::Confirm(s) => match op.purpose {
                    Purpose::FolderContext => ui::Phase::Confirm(s),
                    Purpose::CompositionGuide => ui::Phase::GuideConfirm(s),
                },
                WorkerView::Copying(images, total, bytes) => ui::Phase::Copying {
                    images: *images,
                    total: *total,
                    bytes: *bytes,
                },
                WorkerView::Saving => ui::Phase::Saving,
            },
            Phase::RefreshIndex { .. }
            | Phase::Refresh { .. }
            | Phase::Apply { .. }
            | Phase::AwaitCaches { .. }
            | Phase::ParentScan(..)
            | Phase::ParentHydration(..) => ui::Phase::Adopting,
            Phase::Result { message, success } => ui::Phase::Result {
                message,
                success: *success,
            },
        };
        let can_cancel = op.control.can_cancel()
            && !matches!(
                op.phase,
                Phase::RefreshIndex { .. }
                    | Phase::Refresh { .. }
                    | Phase::Apply { .. }
                    | Phase::AwaitCaches { .. }
                    | Phase::ParentScan(..)
                    | Phase::ParentHydration(..)
            );
        let draw = || {
            ui::draw(
                ctx,
                &op.source,
                &op.destination,
                phase,
                can_cancel,
                self.dialog_escape_pressed(ctx),
            )
        };
        let action = if self.fullscreen_idx.is_some() {
            crate::os_theme::with_dark_context_style(ctx, draw)
        } else {
            draw()
        };
        let Some(action) = action else { return };
        let Some(mut op) = self.image_folder_zip_operation.take() else {
            return;
        };
        match action {
            ui::Action::Close => {
                self.resume_metadata_transfer_context_readers();
                return;
            }
            ui::Action::Cancel => op.cancel(),
            ui::Action::Convert => match &mut op.phase {
                Phase::Worker {
                    worker,
                    view: WorkerView::Confirm(summary),
                    ..
                } => {
                    let total = summary.images;
                    let _ = worker.commands.send(job::Command::Convert);
                    if let Phase::Worker { view, .. } = &mut op.phase {
                        *view = WorkerView::Copying(0, total, 0);
                    }
                }
                _ => {}
            },
        }
        self.image_folder_zip_operation = Some(op);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}
impl Operation {
    fn log_wait(&mut self, app: &App) {
        let reason = match &self.phase {
            Phase::Preparing(..) => WaitReason::Manifest,
            Phase::Waiting(..) => WaitReason::PriorSaves,
            Phase::Capture { .. } => WaitReason::SourceColumn,
            Phase::Worker { view, .. } => match view {
                WorkerView::Preparing => WaitReason::MetadataPreparation,
                WorkerView::Confirm(..) => WaitReason::Confirmation,
                WorkerView::Copying(..) => WaitReason::ImageCopy,
                WorkerView::Saving => WaitReason::MetadataSaves,
            },
            Phase::RefreshIndex { .. } => WaitReason::SourceKeys,
            Phase::Refresh { .. } => WaitReason::SourceReads,
            Phase::Apply { .. } => WaitReason::SourceAdoption,
            Phase::AwaitCaches { .. } if app.book_resume_meters.read_pending() => {
                WaitReason::ResumeRead
            }
            Phase::AwaitCaches { .. } => WaitReason::FavoriteCopy,
            Phase::ParentScan(..) => WaitReason::ParentScan,
            Phase::ParentHydration(..) => WaitReason::ParentHydration,
            Phase::Result { .. } => WaitReason::Terminal,
        };
        if self.last_wait != Some(reason) {
            crate::logger::log(format!(
                "folder_zip_adoption owner={:?} wait={reason:?} exit={:?} destination={}",
                self.owner,
                self.exit,
                self.destination.display(),
            ));
            self.last_wait = Some(reason);
        }
    }
    fn prepare(&mut self, ctx: &egui::Context, settings: &crate::settings::Settings) {
        let wake = ctx.clone();
        self.phase = match zip::spawn_prepare(
            self.source.clone(),
            Some(crate::books::settings_books_root(settings)),
            Arc::clone(&self.control),
            move || wake.request_repaint_of(egui::ViewportId::ROOT),
        ) {
            Ok(receiver) => Phase::Preparing(receiver),
            Err(error) => self.failed(error),
        };
    }
    fn cancel(&mut self) {
        if self.control.cancel() {
            if let Phase::Worker { worker, .. } = &self.phase {
                let _ = worker.commands.send(job::Command::Cancel);
            }
        }
    }
    fn failed(&self, error: String) -> Phase {
        crate::logger::log(format!("folder ZIP {}: {error}", self.source.display()));
        let message = if self.control.is_cancelled() {
            "変換を取り消しました。元のフォルダは残っています。".into()
        } else if error.chars().next().is_some_and(|c| !c.is_ascii()) {
            error
        } else {
            "ZIP を作成できませんでした。元のフォルダは残っています。詳しくはログを確認してください。".into()
        };
        Phase::Result {
            message: format!(
                "{message}\n\u{4f5c}\u{6210}\u{5148}: {}",
                self.destination.display()
            ),
            success: false,
        }
    }
    fn success(&self) -> Phase {
        Phase::Result {
            message: format!(
                "ZIP を作成しました。元のフォルダは残っています。\n{}",
                self.destination.display()
            ),
            success: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::setup_app_for_test;
    use crate::grid_item::GridItem;
    use crate::same_name_folder_book::SameNameFolderBookPolicy as SameNameFolderBook;

    #[test]
    fn section1373_folder_zip_admits_metadata_before_confirmation_and_rejects_undo_busy() {
        for busy in [false, true] {
            let mut app = setup_app_for_test();
            let source = app.tmp.path().join("conversion-source");
            std::fs::create_dir_all(&source).unwrap();
            image::RgbaImage::new(8, 16)
                .save(source.join("page.png"))
                .unwrap();
            let root = app.tmp.path().join("append-books");
            let folder = root.join("book");
            std::fs::create_dir_all(&folder).unwrap();
            let mut builder =
                crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "book".into())
                    .unwrap();
            let page = folder.join("0001_page.jpg");
            std::fs::write(&page, b"generated page").unwrap();
            builder.record(&page).unwrap();
            let receipt = crate::book_append_undo::BookAppendUndoReceipt {
                files: builder.finish().unwrap(),
                operation_id: 1373,
                source_context: app.projected_viewer_context_id(),
                accepted_revision: app.book_append_undo_revision,
            };
            app.book_append_undo_receipt = Some(receipt.clone());
            let ctx = egui::Context::default();
            let revision = app.book_append_undo_revision;
            assert!(app.start_image_folder_zip(&ctx, source.clone(), false));
            assert_eq!(
                app.book_append_undo_revision, revision,
                "manifest preparation is readonly"
            );
            let mut op = app.image_folder_zip_operation.take().unwrap();
            let manifest = zip::prepare_manifest(&source, None, &op.control).unwrap();
            if busy {
                let (_tx, rx) = mpsc::channel();
                app.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    intent: crate::books::BookOpIntent::UndoAppend { receipt },
                });
                let had_tags = app.tags_db.is_some();
                let sidecars = app.sidecars.len();
                assert!(
                    app.start_image_zip_worker(&ctx, &op, manifest, vec![])
                        .is_err()
                );
                assert_eq!(app.book_append_undo_revision, revision);
                assert!(app.book_append_undo_receipt.is_some());
                assert_eq!(app.tags_db.is_some(), had_tags);
                assert_eq!(app.sidecars.len(), sidecars);
            } else {
                op.phase = app
                    .start_image_zip_worker(&ctx, &op, manifest, vec![])
                    .unwrap();
                assert!(app.book_append_undo_revision > revision);
                assert!(app.book_append_undo_receipt.is_none());
                let Phase::Worker { worker, .. } = &mut op.phase else {
                    panic!("accepted worker");
                };
                assert!(matches!(
                    worker
                        .events
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap(),
                    job::Event::Confirm(_)
                ));
                // Confirm has not been accepted: metadata admission already
                // invalidated the receipt, and cancellation cannot revive it.
                op.control.cancel();
                worker.commands.send(job::Command::Cancel).unwrap();
                assert!(matches!(
                    worker
                        .events
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap(),
                    job::Event::Finished { .. }
                ));
                assert!(app.book_append_undo_receipt.is_none());
            }
        }
    }

    fn pump_until(app: &mut App, ctx: &egui::Context, ready: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready(app) {
            app.poll_sidecar_restore(ctx);
            app.poll_favorite_view_writes(ctx);
            app.poll_image_folder_zip(ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "ZIP adoption did not reach its terminal receipt"
            );
            std::thread::yield_now();
        }
    }

    fn finish_actual_worker(app: &mut App, ctx: &egui::Context, source: PathBuf, cancel: bool) {
        assert!(app.start_image_folder_zip(ctx, source.clone(), false));
        let mut op = app.image_folder_zip_operation.take().unwrap();
        let manifest = zip::prepare_manifest(&source, None, &op.control).unwrap();
        op.phase = app
            .start_image_zip_worker(ctx, &op, manifest, vec![])
            .unwrap();
        let Phase::Worker { worker, .. } = &mut op.phase else {
            unreachable!()
        };
        let summary = match worker
            .events
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
        {
            job::Event::Confirm(summary) => summary,
            job::Event::Finished { result, .. } => panic!("preparation failed: {result:?}"),
            _ => panic!("unexpected event before confirmation"),
        };
        assert!(summary.images > 0);
        if cancel {
            op.control.cancel();
            worker.commands.send(job::Command::Cancel).unwrap();
        } else {
            worker.commands.send(job::Command::Convert).unwrap();
        }
        let terminal = loop {
            let event = worker
                .events
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap();
            if let job::Event::Finished { result, .. } = &event {
                assert_eq!(result.is_ok(), !cancel, "{result:?}");
                break event;
            }
        };
        // Fix the receive order using the real worker's terminal payload/resources. No
        // synthetic generation, Selecting phase, sleep or operation retry is involved.
        let (sent, receive) = mpsc::channel();
        sent.send(terminal).unwrap();
        worker.events = receive;
        app.image_folder_zip_operation = Some(op);
    }

    #[test]
    fn image_folder_zip_real_parent_scan_has_one_owner_with_watcher_and_suffix_directory() {
        for suffix in [
            "library",
            "写真.epub",
            "library.7z",
            "library.pdf",
            "library.zip",
        ] {
            for watcher_first in [true, false] {
                let mut app = setup_app_for_test();
                let parent = app.tmp.path().join(suffix);
                let source = parent.join("comicA");
                std::fs::create_dir_all(&source).unwrap();
                std::fs::create_dir(parent.join("comicB")).unwrap();
                std::fs::write(parent.join("comicB.zip"), b"neighbor").unwrap();
                for i in 0..10 {
                    std::fs::write(source.join(format!("p{i:02}.jpg")), b"image bytes").unwrap();
                }
                app.current_folder = Some(parent.clone());
                app.current_folder_last_mtime =
                    Some(std::fs::metadata(&parent).unwrap().modified().unwrap());
                app.items = vec![
                    GridItem::Folder(source.clone()),
                    GridItem::ZipFile(parent.join("comicB.zip")),
                ];
                app.visible_indices = vec![0, 1];
                let ctx = egui::Context::default();
                app.ensure_current_folder_watch(&ctx);
                assert!(app.current_folder_watch.is_some());
                finish_actual_worker(&mut app, &ctx, source.clone(), false);
                // Use an actual notify completion and advance its existing debounce boundary
                // explicitly; the regression does not wait a guessed number of milliseconds.
                let event = loop {
                    let event = app
                        .current_folder_watch
                        .as_ref()
                        .unwrap()
                        .rx
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap();
                    if event
                        .1
                        .as_ref()
                        .is_ok_and(App::current_folder_watch_event_relevant)
                    {
                        break event;
                    }
                };
                let (sent, receive) = crossbeam_channel::unbounded();
                sent.send(event).unwrap();
                app.current_folder_watch.as_mut().unwrap().rx = receive;
                app.poll_current_folder_watch(&ctx);
                app.current_folder_watch.as_mut().unwrap().debounce_until =
                    Some(std::time::Instant::now());
                app.poll_current_folder_watch(&ctx);
                // The same request used by the watcher: hold its completed scan until the
                // conversion's accepted parent scan is ready, then test both consumer orders.
                let pending = app.external_rescan_pending.as_mut().unwrap();
                let result = pending
                    .rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
                let (sent, receive) = mpsc::channel();
                sent.send(result).unwrap();
                pending.rx = receive;
                pump_until(&mut app, &ctx, |app| {
                    matches!(
                        app.image_folder_zip_operation.as_ref().unwrap().phase,
                        Phase::ParentScan(..)
                    )
                });
                let op = app.image_folder_zip_operation.as_mut().unwrap();
                let Phase::ParentScan(pending) = &mut op.phase else {
                    unreachable!()
                };
                let scan = pending
                    .rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
                let (sent, receive) = mpsc::channel();
                sent.send(scan).unwrap();
                pending.rx = receive;
                // The ordinary frame's poll runs first and cannot steal the reply.
                assert!(app.poll_folder_pane_open(&ctx).is_none());
                if watcher_first {
                    app.poll_external_rescan(&ctx);
                }
                app.poll_image_folder_zip(&ctx);
                assert!(matches!(
                    app.image_folder_zip_operation.as_ref().unwrap().phase,
                    Phase::ParentHydration(..)
                ));
                assert!(app.top_level_grid_view.open_path_classification().is_none());
                if !watcher_first {
                    app.poll_external_rescan(&ctx);
                }
                pump_until(&mut app, &ctx, |app| {
                    matches!(
                        app.image_folder_zip_operation.as_ref().unwrap().phase,
                        Phase::Result { .. }
                    )
                });
                assert!(matches!(
                    app.image_folder_zip_operation.as_ref().unwrap().phase,
                    Phase::Result { success: true, .. }
                ));
                let destination = source.with_extension("zip");
                assert_eq!(
                    app.items[app.selected.unwrap()].container_path(),
                    Some(destination.as_path())
                );
                assert!(
                    !app.items
                        .iter()
                        .any(|row| matches!(row, GridItem::Folder(path) if path == &source))
                );
                assert!(source.exists());
                assert_eq!(
                    app.image_folder_zip_operation.as_ref().unwrap().last_wait,
                    Some(WaitReason::Terminal)
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn image_folder_zip_dat_cancel_refreshes_aggregate_and_matching_sibling_only() {
        let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("book");
        std::fs::create_dir(&source).unwrap();
        let page = source.join("a.jpg");
        let other = app.tmp.path().join("other.jpg");
        std::fs::write(&page, b"image").unwrap();
        std::fs::write(&other, b"image").unwrap();
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 0.32;
        app.current_folder = None;
        app.items = vec![
            GridItem::Image(page.clone()),
            GridItem::Image(other.clone()),
        ];
        app.visible_indices = vec![0, 1];
        app.fullscreen_idx = Some(0);
        let sibling = app.fork_materialized_still_context(&source, 0);
        app.with_window_context_for_test(sibling, |viewer| {
            viewer.current_folder = None;
        });
        let unrelated = app.fork_materialized_still_context(&source, 1);
        app.with_window_context_for_test(unrelated, |viewer| {
            viewer.current_folder = None;
            viewer.items = vec![GridItem::Image(other.clone())];
            viewer.mask_pages.insert(0);
        });
        app.fullscreen_idx = None;
        let key = crate::path_key::normalize_keep_drive(&page);
        let mut dat = crate::sidecar::SidecarFile::load(&source);
        dat.set_tags("a.jpg", ["fromDAT"]);
        dat.set_adjust("a.jpg", params.clone());
        assert!(dat.flush_blocking());
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "source-before-DAT",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            Default::default(),
        );
        let cached = crate::grid_item::ThumbnailState::Loaded {
            tex: texture,
            origin: crate::thumb_loader::ThumbLoadOrigin::FinalCache,
            from_edit_preview: true,
            rendered_at_px: 1,
            source_dims: Some((1, 1)),
            layout_dims: None,
        };
        app.thumbnails = vec![cached.clone(), cached.clone()];
        app.with_window_context_for_test(sibling, |viewer| {
            viewer.thumbnails = vec![cached.clone()];
        });
        app.with_window_context_for_test(unrelated, |viewer| {
            viewer.thumbnails = vec![cached.clone()];
        });
        finish_actual_worker(&mut app, &ctx, source.clone(), true);
        let db =
            crate::tags_db::TagsDb::open_readonly(&app.rename_migration_data_dir().join("tags.db"))
                .unwrap();
        assert_eq!(
            db.get_many_display_tags(std::slice::from_ref(&key))
                .get(&key)
                .unwrap(),
            &["#fromDAT"]
        );
        pump_until(&mut app, &ctx, |app| {
            matches!(
                app.image_folder_zip_operation.as_ref().unwrap().phase,
                Phase::Result { .. }
            )
        });
        assert_eq!(app.tags_cache.get(&key).unwrap(), &["#fromDAT"]);
        assert_eq!(app.adjustment_page_params.get(&0), Some(&params));
        assert!(matches!(
            app.thumbnails[0],
            crate::grid_item::ThumbnailState::Evicted
        ));
        assert!(app.thumbnails[1].has_pixels());
        app.with_window_context_for_test(sibling, |viewer| {
            assert_eq!(viewer.tags_cache.get(&key).unwrap(), &["#fromDAT"]);
            assert_eq!(viewer.adjustment_page_params.get(&0), Some(&params));
            assert!(matches!(
                viewer.thumbnails[0],
                crate::grid_item::ThumbnailState::Evicted
            ));
        });
        app.with_window_context_for_test(unrelated, |viewer| {
            assert!(viewer.mask_pages.contains(&0));
            assert!(viewer.tags_cache.get(&key).is_none());
            assert!(viewer.adjustment_page_params.is_empty());
            assert!(viewer.thumbnails[0].has_pixels());
        });
        assert!(!source.with_extension("zip").exists());
        app.image_folder_zip_operation = None;
        app.retire_context(sibling, "zip_refresh_test", |_| ())
            .unwrap();
        app.retire_context(unrelated, "zip_refresh_test", |_| ())
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn egui037_continuation_root_lifecycle_image_folder_zip_close_finishes_accepted_save_then_exits_or_hides_without_parent_wait()
     {
        for tray in [false, true] {
            for stage in 0..4 {
                let committed = stage != 0;
                let mut app = setup_app_for_test();
                let source = app.tmp.path().join("book");
                std::fs::create_dir(&source).unwrap();
                std::fs::write(source.join("a.jpg"), b"image").unwrap();
                app.current_folder = Some(app.tmp.path().to_owned());
                app.settings.minimize_to_tray_on_close = tray;
                if tray {
                    app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
                }
                let ctx = egui::Context::default();
                if committed {
                    finish_actual_worker(&mut app, &ctx, source.clone(), false);
                } else {
                    assert!(app.start_image_folder_zip(&ctx, source.clone(), false));
                    let mut op = app.image_folder_zip_operation.take().unwrap();
                    let manifest = zip::prepare_manifest(&source, None, &op.control).unwrap();
                    op.phase = app
                        .start_image_zip_worker(&ctx, &op, manifest, vec![])
                        .unwrap();
                    let Phase::Worker { worker, .. } = &op.phase else {
                        unreachable!()
                    };
                    assert!(matches!(
                        worker
                            .events
                            .recv_timeout(std::time::Duration::from_secs(30))
                            .unwrap(),
                        job::Event::Confirm(..)
                    ));
                    app.image_folder_zip_operation = Some(op);
                }
                if stage >= 2 {
                    pump_until(&mut app, &ctx, |app| {
                        matches!(
                            app.image_folder_zip_operation.as_ref().unwrap().phase,
                            Phase::ParentScan(..)
                        )
                    });
                }
                if stage == 3 {
                    let op = app.image_folder_zip_operation.as_mut().unwrap();
                    let Phase::ParentScan(pending) = &mut op.phase else {
                        unreachable!()
                    };
                    let scan = pending
                        .rx
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .unwrap();
                    let (sent, receive) = mpsc::channel();
                    sent.send(scan).unwrap();
                    pending.rx = receive;
                    app.poll_image_folder_zip(&ctx);
                    assert!(matches!(
                        app.image_folder_zip_operation.as_ref().unwrap().phase,
                        Phase::ParentHydration(..)
                    ));
                }
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
                ctx.begin_pass(input);
                let lifecycle = app.accept_root_lifecycle_before_frame(&ctx);
                app.service_root_lifecycle_tray_checkpoint(&ctx, lifecycle);
                assert_eq!(
                    matches!(
                        app.image_folder_zip_operation.as_ref().unwrap().exit,
                        Exit::TrayHide
                    ),
                    tray
                );
                assert!(app.window_visible);
                let requested = ctx.end_pass();
                assert!(
                    requested.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .contains(&egui::ViewportCommand::CancelClose)
                );
                ctx.begin_pass(egui::RawInput::default());
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while app.image_folder_zip_operation.is_some() {
                    app.service_root_lifecycle(&ctx);
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
                let terminal = ctx.end_pass();
                assert_eq!(
                    terminal.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .contains(&egui::ViewportCommand::Close),
                    !tray
                );
                assert_eq!(app.window_visible, !tray);
                assert!(app.folder_pane_open_pending.is_none());
                assert_eq!(source.with_extension("zip").exists(), committed);
            }
        }
    }

    #[test]
    fn egui037_continuation_root_lifecycle_zip_hydration_waits_for_discarded_terminal() {
        let mut app = setup_app_for_test();
        app.settings.minimize_to_tray_on_close = false;
        let source = app.tmp.path().join("hydrating-book");
        std::fs::create_dir(&source).unwrap();
        app.current_folder = Some(source.clone());
        let ctx = egui::Context::default();
        assert!(app.start_image_folder_zip(&ctx, source.clone(), false));
        let destination = app
            .image_folder_zip_operation
            .as_ref()
            .unwrap()
            .destination
            .clone();
        let owner = app.image_folder_zip_operation.as_ref().unwrap().owner;
        let (completed, receipt) = mpsc::channel();
        app.image_folder_zip_operation.as_mut().unwrap().phase = Phase::ParentHydration(receipt);
        let hydration_messages = app.activate_live_sidecar_restore_for_test(source);
        app.set_live_sidecar_first_display_for_test(
            crate::app::sidecar_restore::FirstDisplayAction::None,
            Some(super::super::MainListAfterHydration::ImageFolderZip {
                destination,
                completed,
            }),
        );
        let selected = app.selected;
        let mut raw = egui::RawInput::default();
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        let blocked = ctx.run_pass(raw, |ctx| {
            let lifecycle = app.accept_root_lifecycle_before_frame(ctx);
            app.service_root_lifecycle_tray_checkpoint(ctx, lifecycle);
            assert!(app.image_zip_discarded_hydration_exit_pending(owner));
            app.service_image_folder_zip_exit(ctx);
            assert!(
                app.image_folder_zip_operation.is_some(),
                "ZIP exit waits for its discarded hydration owner"
            );
        });
        assert!(
            blocked.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(
            !blocked.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(matches!(
            hydration_messages.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let terminal = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.service_discarded_sidecar_restore(ctx);
            assert!(!app.image_zip_discarded_hydration_exit_pending(owner));
            app.service_root_lifecycle(ctx);
        });
        assert!(app.image_folder_zip_operation.is_none());
        assert!(
            terminal.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        assert_eq!(
            app.selected, selected,
            "cancelled hydration cannot select the ZIP destination"
        );
        let again = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.service_root_lifecycle(ctx)
        });
        assert!(
            !again
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close))
        );
    }

    #[test]
    fn image_folder_zip_actual_favorite_copy_names_only_destination() {
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("book");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a.jpg"), b"image").unwrap();
        let favorite = crate::settings::FavoriteEntry::new("私の本".into(), source.clone());
        let old_id = favorite.id;
        app.settings.favorites.push(favorite.clone());
        crate::settings_db::with_db_result(|db| db.save_full(&app.settings)).unwrap();
        let ctx = egui::Context::default();
        finish_actual_worker(&mut app, &ctx, source.clone(), false);
        app.poll_image_folder_zip(&ctx);
        let original = app
            .settings
            .favorites
            .iter()
            .find(|f| f.id == old_id)
            .unwrap();
        assert_eq!(original.name, "私の本");
        assert_eq!(original.path, source);
        let copied = app
            .settings
            .favorites
            .iter()
            .find(|f| f.path == source.with_extension("zip"))
            .unwrap();
        assert_eq!(copied.name, "私の本 (ZIP)");
        assert_ne!(copied.id, old_id);
    }

    #[test]
    fn image_folder_zip_main_selects_only_visible_destination_for_all_policies() {
        for policy in [
            SameNameFolderBook::PreferBook,
            SameNameFolderBook::PreferFolder,
            SameNameFolderBook::ShowBoth,
        ] {
            for filtered in [0, 1, 2] {
                let mut app = setup_app_for_test();
                let source = app.tmp.path().join("photos");
                let parent = app.tmp.path().to_owned();
                app.current_folder = Some(source.clone());
                app.settings.same_name_folder_book = policy;
                let ctx = egui::Context::default();
                assert!(app.start_image_folder_zip(&ctx, source.clone(), true));
                let destination = zip::destination_for(&source).unwrap();
                app.current_folder = Some(parent);
                app.items = vec![
                    GridItem::Folder(source),
                    GridItem::ZipFile(destination.clone()),
                ];
                app.visible_indices = if filtered == 2 {
                    vec![]
                } else if filtered != 0 || policy == SameNameFolderBook::PreferFolder {
                    vec![0]
                } else {
                    vec![1]
                };
                app.selected = app.visible_indices.first().copied();
                let (completed, receipt) = mpsc::channel();
                app.finish_main_list_after_hydration(
                    super::super::MainListAfterHydration::ImageFolderZip {
                        destination,
                        completed,
                    },
                );
                app.image_folder_zip_operation.as_mut().unwrap().phase =
                    Phase::ParentHydration(receipt);
                app.poll_image_folder_zip(&ctx);
                assert_eq!(
                    app.selected,
                    if filtered == 2 {
                        None
                    } else {
                        Some(
                            if filtered != 0 || policy == SameNameFolderBook::PreferFolder {
                                0
                            } else {
                                1
                            },
                        )
                    }
                );
                assert_eq!(app.settings.same_name_folder_book, policy);
                assert!(matches!(
                    app.image_folder_zip_operation.as_ref().unwrap().phase,
                    Phase::Result { success: true, .. }
                ));
            }
        }
    }

    #[test]
    fn image_folder_zip_target_order_uses_source_ancestor_view_instead_of_an_unrelated_root() {
        let mut app = setup_app_for_test();
        let parent = app.tmp.path().join("library");
        let source = parent.join("photos");
        let unrelated = app.tmp.path().join("other");
        let favorite = crate::settings::FavoriteEntry::new("library".into(), parent);
        app.settings.favorites.push(favorite.clone());
        app.settings.remember_favorite_view_state = true;
        let mut state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        state.sort_order = crate::settings::SortOrder::NumericDesc;
        app.favorite_view_states.insert(favorite.id, state);
        app.settings.sort_order = crate::settings::SortOrder::DateAsc;
        app.current_folder = Some(unrelated);
        assert!(app.start_image_folder_zip(&egui::Context::default(), source, false));
        let op = app.image_folder_zip_operation.take().unwrap();
        assert_eq!(
            app.image_zip_target_sort(&op).unwrap(),
            crate::settings::SortOrder::NumericDesc
        );
        assert_eq!(app.settings.sort_order, crate::settings::SortOrder::DateAsc);
    }

    #[test]
    fn image_folder_zip_aggregate_origin_returns_through_the_existing_parent_list_owner() {
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("photos");
        std::fs::create_dir(&source).unwrap();
        app.current_folder = None;
        app.items = vec![GridItem::Folder(source.clone())];
        let ctx = egui::Context::default();
        assert!(app.start_image_folder_zip(&ctx, source, true));
        let op = app.image_folder_zip_operation.take().unwrap();
        let phase = app.finish_image_zip_adoption(&ctx, &op, Ok(()));
        assert!(matches!(phase, Phase::ParentScan(_)));
        assert!(!app.folder_open_preparation_pending());
    }

    #[test]
    fn image_folder_zip_modal_blocks_inputs_and_notice_escape_preserves_reader() {
        use egui_kittest::kittest::Queryable;
        for composing in [false, true] {
            let mut app = setup_app_for_test();
            let source = app.tmp.path().join("photos");
            app.current_folder = Some(source.clone());
            app.items = vec![GridItem::Image(source.join("a.jpg"))];
            app.fullscreen_idx = None;
            app.selected = Some(0);
            app.fs_zoom = 1.25;
            let ctx = egui::Context::default();
            assert!(app.start_image_folder_zip(&ctx, source.clone(), true));
            assert!(!app.grid_item_input_allowed());
            assert!(app.document_open_modal_admission_blocked());
            assert!(!app.start_image_folder_zip(&ctx, source.clone(), true));
            let (commands, requests) = mpsc::channel();
            let (_events_tx, events) = mpsc::channel();
            app.image_folder_zip_operation.as_mut().unwrap().phase = Phase::Worker {
                worker: job::Worker {
                    commands,
                    events,
                    resources: Arc::new(Mutex::new(job::Resources::new(Default::default(), None))),
                },
                view: WorkerView::Confirm(job::Summary {
                    images: 1,
                    bytes: 100,
                    rows: 0,
                    auxiliary: 0,
                }),
                trims: vec![],
            };
            let held = std::rc::Rc::new(std::cell::RefCell::new(app));
            let render = held.clone();
            let mut first = true;
            let mut h = crate::test_ui::builder()
                .with_size(egui::vec2(1093.0, 614.0))
                .build(move |ctx| {
                    if first {
                        crate::ui_fonts::configure_fonts(ctx);
                        crate::ime_focus::install_ime_input_policy(ctx);
                        first = false;
                    }
                    let mut app = render.borrow_mut();
                    app.update_ime_state(ctx);
                    let _ = app.handle_keyboard(ctx);
                    app.show_image_folder_zip(ctx);
                    app.process_scroll(ctx);
                });
            h.run();
            let _ = h.get_by_label("ZIP \u{306b}\u{5909}\u{63db}");
            if composing {
                h.input_mut().events.extend([
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("input".into())),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 120.0),
                        modifiers: egui::Modifiers::CTRL,
                    },
                ]);
                h.key_press(egui::Key::Escape);
                h.step();
                assert!(
                    !held
                        .borrow()
                        .image_folder_zip_operation
                        .as_ref()
                        .unwrap()
                        .control
                        .is_cancelled()
                );
                assert_eq!(held.borrow().selected, Some(0));
                assert_eq!(held.borrow().scroll_offset_y, 0.0);
                // Pointer cancellation is explicit even while the IME owns Escape.
                h.get_by_label("\u{53d6}\u{6d88}").click();
                h.run();
            } else {
                h.get_by_label("ZIP \u{306b}\u{5909}\u{63db}").click();
                h.run();
                assert!(matches!(requests.try_recv(), Ok(job::Command::Convert)));
                assert!(matches!(
                    &held
                        .borrow()
                        .image_folder_zip_operation
                        .as_ref()
                        .unwrap()
                        .phase,
                    Phase::Worker {
                        view: WorkerView::Copying(0, 1, 0),
                        ..
                    }
                ));
                h.key_press(egui::Key::Escape);
                h.step();
            }
            let app = held.borrow();
            assert!(
                app.image_folder_zip_operation
                    .as_ref()
                    .unwrap()
                    .control
                    .is_cancelled()
            );
            assert_eq!(app.current_folder, Some(source));
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.fs_zoom, 1.25);
            assert_eq!(app.items.len(), 1);
        }
    }

    #[test]
    fn image_folder_zip_copied_favorite_is_adopted_by_the_actual_app_consumer() {
        use crate::favorite_view_state::{FavoriteViewStoreCommand, FavoriteViewStoreWriter};
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("photos");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a.jpg"), b"image").unwrap();
        let manifest = zip::prepare_manifest(&source, None, &Default::default()).unwrap();
        let old = uuid::Uuid::new_v4();
        let new = uuid::Uuid::new_v4();
        let mut state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        state.grid_cols = 7;
        state.default_spread_mode = crate::settings::SpreadMode::RtlCover;
        let writer =
            FavoriteViewStoreWriter::spawn(app.tmp.path().join("favorite-copy.db")).unwrap();
        writer
            .submit(FavoriteViewStoreCommand::Set {
                id: old,
                state: state.clone(),
            })
            .unwrap();
        let portal = writer.folder_zip_portal(vec![(old, new)]);
        portal(
            &manifest,
            crate::image_folder_zip_metadata::CopyPhase::Apply,
        )
        .recv_timeout(std::time::Duration::from_secs(15))
        .unwrap()
        .unwrap();
        // CopyReply is sent after Copied publication. No polling delay or retry.
        app.favorite_view_store_writer = Some(writer);
        let mut newer = state.clone();
        newer.grid_cols = 9;
        app.favorite_view_states.insert(old, newer.clone());
        app.poll_favorite_view_writes(&egui::Context::default());
        assert_eq!(app.favorite_view_states.get(&new), Some(&state));
        assert_eq!(app.favorite_view_states.get(&old), Some(&newer));
    }

    #[test]
    fn image_folder_zip_finished_payload_adopts_favorite_defaults_before_first_zip_use() {
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("photos");
        let target = zip::destination_for(&source).unwrap();
        let favorite = crate::settings::FavoriteEntry::new("photos".into(), target.clone());
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 0.2;
        let ctx = egui::Context::default();
        assert!(app.start_image_folder_zip(&ctx, source.clone(), false));
        let resources = job::Resources::new(std::mem::take(&mut app.sidecars), app.tags_db.take());
        let (tx, events) = mpsc::channel();
        let (commands, _requests) = mpsc::channel();
        tx.send(job::Event::Finished {
            source,
            destination: target.clone(),
            result: Ok(job::Success {
                favorites: vec![favorite.clone()],
                favorite_params: vec![(favorite.id, params.clone())],
            }),
        })
        .unwrap();
        app.image_folder_zip_operation.as_mut().unwrap().phase = Phase::Worker {
            worker: job::Worker {
                commands,
                events,
                resources: Arc::new(Mutex::new(resources)),
            },
            view: WorkerView::Saving,
            trims: vec![],
        };
        app.poll_image_folder_zip(&ctx);
        assert_eq!(
            app.adjustment_favorite_params.get(&favorite.id),
            Some(&params)
        );
        assert_eq!(
            crate::final_composite::active_favorite_default_id_for_path(
                &target,
                &app.settings.favorites,
                None,
                |id| app.adjustment_favorite_params.contains_key(&id)
            ),
            Some(favorite.id)
        );
    }

    #[test]
    fn image_folder_zip_partial_refresh_clears_only_affected_page_and_ignores_stale_generation() {
        let mut app = setup_app_for_test();
        let index = 0;
        app.mask_pages.extend([0, 1]);
        app.comic_pages.extend([0, 1]);
        let chunk = |generation| SourceChunk {
            context: app.viewer_context_main(),
            generation,
            tags: vec![],
            pages: vec![PageUpdate {
                index,
                adjustment: None,
                local: false,
                crop: None,
                trim: None,
                mask: false,
                conceal: false,
                comic: false,
                rotation: None,
            }],
            last: false,
        };
        let stale = chunk(app.items_generation.wrapping_add(1));
        let current = chunk(app.items_generation);
        app.apply_image_zip_source_chunk(stale);
        assert!(app.mask_pages.contains(&0));
        app.apply_image_zip_source_chunk(current);
        assert!(!app.mask_pages.contains(&0));
        assert!(!app.comic_pages.contains(&0));
        assert!(app.mask_pages.contains(&1));
        assert!(app.comic_pages.contains(&1));
    }

    #[cfg(windows)]
    #[test]
    fn egui037_continuation_context_state_zip_exit_defers_membership_to_own_ui() {
        use crate::app::context_state_tests::Fixture;
        let Fixture {
            mut env,
            main,
            other,
            old_stamp,
            ..
        } = Fixture::new();
        let (generation, key) = env
            .with_viewer_context_state(other, |app| {
                (app.items_generation, app.rating_path_key(0).unwrap())
            })
            .unwrap();
        let source = env.tmp.path().join("source");
        env.image_folder_zip_operation = Some(Box::new(Operation {
            owner: other,
            purpose: Purpose::FolderContext,
            completion: Completion::Viewer,
            destination: source.with_extension("zip"),
            source,
            control: Default::default(),
            _lease: None,
            exit: Exit::OwnerRetired,
            last_wait: None,
            phase: Phase::Apply {
                result: Ok(()),
                refresh: SourceRefresh {
                    snapshot: None,
                    errors: vec![],
                    chunks: [SourceChunk {
                        context: other,
                        generation,
                        tags: vec![(key, vec!["saved-tag".into()])],
                        pages: vec![],
                        last: true,
                    }]
                    .into(),
                },
            },
        }));
        App::start_rating_boundary_trace_for_test();
        env.service_image_folder_zip_exit(&egui::Context::default());
        assert!(App::take_rating_boundary_trace_for_test().is_empty());
        App::stop_rating_boundary_trace_for_test();
        Fixture::assert_pending(&env, main, 4, old_stamp);
        env.with_viewer_context_state(other, |app| {
            Fixture::assert_pending(app, other, 7, old_stamp);
            assert!(matches!(app.metadata_import_refresh_index,
                Some(super::super::MetadataImportRefreshState::DisplayReconciliation { items_generation }) if items_generation == generation));
        }).unwrap();
        env.image_folder_zip_operation = None;
        env.with_viewer_context(other, |app| {
            assert!(app.metadata_import_refresh_index.is_none());
            assert!(app.visible_indices.is_empty());
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn image_folder_zip_f12_adopts_terminal_without_switching_source_or_sibling() {
        let mut app = setup_app_for_test();
        let source = app.tmp.path().join("photos");
        app.current_folder = Some(source.clone());
        app.items = vec![GridItem::Image(source.join("a.jpg"))];
        app.fullscreen_idx = Some(0);
        app.fs_zoom = 1.37;
        let detached = app.fork_materialized_still_context(&source, 0);
        let main_fullscreen = app.fullscreen_idx;
        let main_items = app.items.clone();

        let ctx = egui::Context::default();
        app.with_window_context_for_test(detached, |a| {
            assert!(a.start_image_folder_zip(&ctx, source.clone(), true));
        });
        app.image_folder_zip_operation.as_mut().unwrap().phase =
            Phase::AwaitCaches { result: Ok(()) };
        app.book_resume_writer
            .as_ref()
            .unwrap()
            .read_all()
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap()
            .unwrap();
        app.poll_image_folder_zip(&ctx);
        assert!(matches!(
            app.image_folder_zip_operation.as_ref().unwrap().phase,
            Phase::Result { success: true, .. }
        ));
        assert_eq!(app.current_folder, Some(source.clone()));
        assert_eq!(app.fullscreen_idx, main_fullscreen);
        assert_eq!(app.items, main_items);
        app.with_window_context_for_test(detached, |a| {
            assert_eq!(a.current_folder, Some(source.clone()));
            assert_eq!(a.fullscreen_idx, Some(0));
            assert_eq!(a.items.len(), 1);
        });
        app.image_folder_zip_operation = None;
        app.retire_context(detached, "zip_test", |_| ()).unwrap();
    }
}
