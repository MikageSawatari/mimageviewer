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
enum Exit {
    None,
    Root,
    OwnerRetired,
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
    Selecting,
    Result {
        message: String,
        success: bool,
    },
}
pub(crate) struct Operation {
    owner: ViewerContextId,
    purpose: Purpose,
    completion: Completion,
    generation: u64,
    source: PathBuf,
    destination: PathBuf,
    control: Arc<zip::ConversionControl>,
    _lease: Option<crate::remote_ipc::session::LocalModalLease>,
    exit: Exit,
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
            generation: self.items_generation,
            source,
            destination,
            control: Default::default(),
            _lease: lease,
            exit: Exit::None,
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
    pub(crate) fn defer_image_folder_zip_root_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) {
            if let Some(op) = self.image_folder_zip_operation.as_mut() {
                op.exit = Exit::Root;
                op.cancel();
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
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
                            let _ = self.with_viewer_context(id, |app| {
                                app.restore_pending_view_trim_transfer(pending)
                            });
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
                        self.begin_metadata_import_terminal_refresh();
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
                            let _ = self.with_viewer_context(id, |app| {
                                app.restore_pending_view_trim_transfer(pending)
                            });
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
                if !self.advance_metadata_import_terminal_refresh(&op.source, false) {
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
                    let mut requests =
                        self.take_metadata_import_refresh_requests(changed, &Default::default());
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
                    self.apply_image_zip_source_chunk(chunk);
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
            Phase::Selecting => {
                // Consume our own already-admitted scan through the same navigation tail.
                // The operation is temporarily owned here, so only this continuation can open.
                if let Some(ready) = self.poll_folder_pane_open(ctx) {
                    if let Some(ready) = self.resolve_main_folder_open_ready(ctx, ready) {
                        self.open_direct_navigation_target(
                            ready.path,
                            Some(ready.scan),
                            super::OpenRequestOwner::Navigation,
                            ready.navigation,
                            ready.epub_restore,
                            ready.restore_intent,
                        );
                    }
                }

                let ready = self
                    .with_viewer_context_ref(op.owner, |viewer| {
                        viewer.items_generation() != op.generation
                            && viewer.current_folder() == op.source.parent()
                    })
                    .unwrap_or(false);
                if ready
                    && !self.folder_open_preparation_pending()
                    && !self.sidecar_restore_active()
                {
                    let _ = self.with_viewer_context(op.owner, |app| {
                        if let Some(idx) = app.visible_indices.iter().copied().find(|&idx| {
                            app.items
                                .get(idx)
                                .and_then(|item| item.container_path())
                                .is_some_and(|path| {
                                    crate::folder_tree::path_eq(path, &op.destination)
                                })
                        }) {
                            app.selected = Some(idx);
                            app.checked.clear();
                            app.scroll_to_selected = true;
                        }
                    });
                    op.success()
                } else if matches!(
                    self.viewer_context_residence(op.owner),
                    super::ContextResidence::Mounted | super::ContextResidence::AtRest
                ) {
                    Phase::Selecting
                } else {
                    op.success()
                }
            }
            phase => phase,
        };
        if matches!(op.phase, Phase::Result { .. }) && !matches!(op.exit, Exit::None) {
            self.resume_metadata_transfer_context_readers();
            if let Phase::Result { message, .. } = &op.phase {
                self.show_feedback_toast(message.clone());
            }
            if matches!(op.exit, Exit::Root) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                    let _ = self.with_viewer_context(id, |app| {
                        app.restore_pending_view_trim_transfer(pending)
                    });
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
                return Phase::Selecting;
            }
        }
        op.success()
    }
    fn apply_image_zip_source_chunk(&mut self, chunk: SourceChunk) {
        let _ = self.with_viewer_context(chunk.context, |app| {
            if app.items_generation != chunk.generation {
                return;
            }
            for (key, tags) in chunk.tags {
                app.set_tags_cache_entry(key, tags);
            }
            for page in chunk.pages {
                let idx = page.index;
                app.evict_thumbnail_for_reload(idx);
                app.bump_adjustment_generation(idx);
                app.bump_erase_mask_generation(idx);
                app.bump_local_adjust_generation(idx);
                app.adjustment_page_params.remove(&idx);
                if let Some(params) = page.adjustment {
                    app.adjustment_page_params.insert(idx, params);
                }
                app.local_adjust_pages.remove(&idx);
                if page.local {
                    app.local_adjust_pages.insert(idx);
                }
                app.local_adjust_page_layers.remove(&idx);
                app.local_adjust_selected_layers.remove(&idx);
                app.export_crop_pages.remove(&idx);
                app.export_crop_page_settings.remove(&idx);
                if let Some(crop) = page.crop {
                    app.export_crop_pages.insert(idx);
                    app.export_crop_page_settings.insert(idx, crop);
                }
                app.view_trim_page_overrides.remove(&idx);
                if let Some(trim) = page.trim {
                    app.view_trim_page_overrides.insert(idx, trim);
                }
                for (set, enabled) in [
                    (&mut app.mask_pages, page.mask),
                    (&mut app.conceal_pages, page.conceal),
                    (&mut app.comic_pages, page.comic),
                ] {
                    set.remove(&idx);
                    if enabled {
                        set.insert(idx);
                    }
                }
                app.rotation_cache.remove(&idx);
                if let Some(rotation) = page.rotation {
                    app.rotation_cache.insert(idx, rotation);
                }
            }
            if chunk.last {
                app.rebuild_visible_indices_preserving_facet_scope();
            }
        });
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
            | Phase::Selecting => ui::Phase::Adopting,
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
                    | Phase::Selecting
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
                app.items_generation += 1;
                app.image_folder_zip_operation.as_mut().unwrap().phase = Phase::Selecting;
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
        assert!(matches!(phase, Phase::Selecting));
        assert!(app.folder_open_preparation_pending());
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
