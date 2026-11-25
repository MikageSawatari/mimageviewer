//! ROOT drop admission, snapshot publication and the shared Shell paste boundary.

use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, mpsc};

use eframe::egui;

use super::App;
use crate::main_drop_target::{DropGeometry, DropPlace, DropRequest, DropRow};
use crate::shell_file_ops::{ShellTransferRequest, ShellTransferResult};
use crate::shell_paste::{ShellPasteCompletion, ShellPasteError};
use crate::ui_dialogs::file_organize::{FileOrganizeRequest, FileOrganizeRunning};

pub(crate) const SHELL_PASTE_BUSY_MESSAGE: &str = "貼り付けが完了するまでお待ちください";
const SHELL_PASTE_START_FAILURE: &str = "貼り付けを開始できませんでした";
const SHELL_PASTE_COMPLETION_FAILURE: &str = "貼り付けを完了できませんでした";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellPasteOrigin {
    Tree,
    CtrlV,
    GridMenu,
}

impl ShellPasteOrigin {
    fn perf_tag(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::CtrlV => "ctrl_v",
            Self::GridMenu => "grid_menu",
        }
    }
}

pub(crate) struct ShellPasteRunning {
    pub(crate) rx: mpsc::Receiver<ShellPasteCompletion>,
    pub(crate) selection_id: Option<crate::post_operation_selection::PostOperationSelectionId>,
    pub(crate) origin: ShellPasteOrigin,
    pub(crate) started_at: std::time::Instant,
    pub(crate) perf_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellPasteOutcome {
    Invoked,
    Failed,
    StartFailed,
    Disconnected,
}

impl ShellPasteOutcome {
    fn perf_tag(self) -> &'static str {
        match self {
            Self::Invoked => "invoked",
            Self::Failed => "failed",
            Self::StartFailed => "start_failed",
            Self::Disconnected => "disconnected",
        }
    }
}

#[cfg(test)]
thread_local! {
    static BOOKS_ROOT_RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MODIFIER_RESYNC_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SHELL_PASTE_END_FIELDS: std::cell::RefCell<Vec<serde_json::Value>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn record_modifier_resync_for_test() {
    MODIFIER_RESYNC_CALLS.with(|calls| calls.set(calls.get() + 1));
}

#[cfg(test)]
pub(crate) fn take_modifier_resync_calls_for_test() -> usize {
    MODIFIER_RESYNC_CALLS.with(|calls| calls.replace(0))
}

#[cfg(test)]
pub(super) fn record_books_root_resolution_for_test() {
    BOOKS_ROOT_RESOLUTIONS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(crate) fn take_books_root_resolutions_for_test() -> usize {
    BOOKS_ROOT_RESOLUTIONS.with(|count| count.replace(0))
}

/// A pass-local copy of what was drawn. Gesture retirement cannot erase presented geometry.
pub(crate) struct CapturedMainDropGeometry {
    geometry: DropGeometry,
    tree_layer: Option<egui::LayerId>,
    grid_layer: Option<egui::LayerId>,
}

#[derive(Clone)]
struct GridDropDisplay {
    pass: u64,
    layer: egui::LayerId,
    clip: egui::Rect,
    folders: Vec<DropRow>,
}

fn grid_drop_display_id() -> egui::Id {
    egui::Id::unique("main_drop_grid_display")
}

impl App {
    pub(crate) fn grid_drop_hover(&self, idx: usize) -> bool {
        matches!(self.items.get(idx), Some(crate::grid_item::GridItem::Folder(path))
            if self.main_drop_hover_matches(path, DropPlace::Grid))
    }

    pub(crate) fn main_drop_hover_matches(&self, path: &Path, place: DropPlace) -> bool {
        self.main_drop.borrow().hover.as_ref().is_some_and(|hover| {
            hover.place == place && crate::folder_tree::path_eq(path, &hover.path)
        })
    }

    pub(crate) fn record_main_drop_grid_geometry(
        &self,
        ui: &egui::Ui,
        clip: egui::Rect,
        folders: Vec<DropRow>,
    ) {
        let eligible = self.is_physical_folder_listing() && self.grid_item_input_allowed();
        let display = GridDropDisplay {
            pass: ui.ctx().cumulative_pass_nr(),
            layer: ui.layer_id(),
            clip: if eligible { clip } else { egui::Rect::NOTHING },
            folders: if eligible {
                folders
                    .into_iter()
                    .filter_map(|mut row| {
                        row.rect = row.rect.intersect(clip);
                        row.rect.is_positive().then_some(row)
                    })
                    .collect()
            } else {
                Vec::new()
            },
        };
        ui.ctx()
            .data_mut(|data| data.insert_temp(grid_drop_display_id(), display));
    }

    /// Read-only: publishing cursor eligibility must never invalidate an undo receipt.
    pub(crate) fn shell_transfer_admission(&self, ctx: &egui::Context) -> Result<(), String> {
        let reason = if self.shell_paste_request.is_some() {
            Some(SHELL_PASTE_BUSY_MESSAGE)
        } else if !matches!(self.file_organize_request, FileOrganizeRequest::Hidden) {
            Some("整理操作が完了するまでお待ちください")
        } else if self.remote_session_blocks_local_control() {
            Some("リモート接続中は整理操作を開始できません")
        } else if self.shutdown_requested.load(Ordering::SeqCst)
            || self
                .tray_controller
                .as_ref()
                .is_some_and(|tray| tray.is_quit_requested())
            || ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|viewport| viewport.close_requested())
            })
        {
            Some("終了が要求されたため整理操作を実行しませんでした")
        } else if self.main_hwnd.is_none_or(|hwnd| hwnd == 0) {
            Some("整理操作を開始できませんでした")
        } else if self.viewer_mutation_modal_active()
            || self.sidecar_restore_active()
            || self.common_modal_dialog_open()
        {
            Some("ダイアログを閉じてから操作してください")
        } else if self.folder_pane_disabled() {
            Some("固定表示中は整理操作を開始できません")
        } else {
            None
        };
        reason.map_or(Ok(()), |reason| Err(reason.to_owned()))
    }

    /// Operation-time only: the existing default root resolver must not run while drawing.
    fn shell_transfer_submission_admission_with(
        &self,
        ctx: &egui::Context,
        target: &Path,
        resolve_books_root: impl FnOnce(&Self) -> PathBuf,
    ) -> Result<(), String> {
        self.shell_transfer_admission(ctx)?;
        let books_root = resolve_books_root(self);
        if crate::books::path_is_under_or_equal(target, &books_root) {
            Err("本棚の中へは移動・コピーできません".to_owned())
        } else {
            Ok(())
        }
    }

    pub(crate) fn refresh_main_drop_admission(&self, ctx: &egui::Context) {
        let admission = self.shell_transfer_admission(ctx);
        self.main_drop.borrow_mut().admission = admission;
    }

    pub(crate) fn submit_shell_transfer(
        &mut self,
        ctx: &egui::Context,
        request: ShellTransferRequest,
    ) {
        self.submit_shell_transfer_with(
            ctx,
            request,
            Self::book_root_path,
            crate::shell_file_ops::transfer_items_async,
        );
    }

    pub(crate) fn submit_shell_transfer_with(
        &mut self,
        ctx: &egui::Context,
        request: ShellTransferRequest,
        resolve_books_root: impl FnOnce(&Self) -> PathBuf,
        start: impl FnOnce(
            Option<isize>,
            ShellTransferRequest,
            egui::Context,
        ) -> mpsc::Receiver<ShellTransferResult>,
    ) {
        if let Err(reason) = self.shell_transfer_submission_admission_with(
            ctx,
            &request.destination,
            resolve_books_root,
        ) {
            crate::logger::log(format!("shell_transfer: rejected: {reason}"));
            self.show_feedback_warning(reason);
            return;
        }
        if !self.admit_book_append_followup_mutation() {
            return;
        }
        let rx = start(self.main_hwnd, request.clone(), ctx.clone());
        self.file_organize_request =
            FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
        self.refresh_main_drop_admission(ctx);
    }

    pub(crate) fn consume_main_drops(&mut self, ctx: &egui::Context) {
        self.consume_main_drops_with(ctx, |app, paths| app.handle_external_file_drop(paths));
    }

    fn consume_main_drops_with(
        &mut self,
        ctx: &egui::Context,
        mut grid: impl FnMut(&mut Self, Vec<PathBuf>),
    ) {
        // End the RefCell borrow before handlers enter Shell message loops.
        let drops = std::mem::take(&mut self.main_drop.borrow_mut().drops);
        for request in drops {
            match request {
                DropRequest::GridFiles(paths) if self.sidecar_restore_active() => {
                    crate::logger::log(format!(
                        "file_drop: discarded {} path(s) during sidecar restore",
                        paths.len()
                    ));
                }
                DropRequest::GridFiles(paths) => grid(self, paths),
                DropRequest::FolderTransfer(request) => self.submit_shell_transfer(ctx, request),
                DropRequest::Failed(message) => {
                    crate::logger::log(format!("main_drop: {message}"));
                    self.show_feedback_warning(message);
                }
            }
        }
    }

    pub(crate) fn capture_main_drop_geometry(
        &self,
        ctx: &egui::Context,
    ) -> Option<CapturedMainDropGeometry> {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return None;
        }
        let metrics = self
            .main_hwnd
            .and_then(crate::main_drop_target::capture_window_metrics)?;
        Some(self.capture_main_drop_geometry_with_metrics(ctx, metrics))
    }

    /// Finalize only after later dialogs/areas have drawn, using the shape captured before retirement.
    pub(crate) fn publish_main_drop_geometry(
        &self,
        ctx: &egui::Context,
        captured: Option<CapturedMainDropGeometry>,
    ) {
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            return;
        }
        self.refresh_main_drop_admission(ctx);
        let Some(captured) = captured else { return };
        let geometry = finalize_main_drop_geometry(ctx, captured);
        crate::main_drop_target::publish_geometry(&self.main_drop, ctx, geometry);
    }

    fn capture_main_drop_geometry_with_metrics(
        &self,
        ctx: &egui::Context,
        metrics: crate::main_drop_target::WindowMetrics,
    ) -> CapturedMainDropGeometry {
        let mut geometry = DropGeometry {
            pane_rect: egui::Rect::NOTHING,
            tree_clip: egui::Rect::NOTHING,
            rows: Vec::new(),
            grid_clip: egui::Rect::NOTHING,
            grid_folders: Vec::new(),
            grid_blocked: Vec::new(),
            client_size_px: metrics.client_size_px,
            dpi: metrics.dpi,
            pixels_per_point: ctx.pixels_per_point(),
        };
        let mut tree_layer = None;
        let mut grid_layer = None;
        if let Some(display) = ctx
            .data(|data| data.get_temp::<GridDropDisplay>(grid_drop_display_id()))
            .filter(|display| display.pass == ctx.cumulative_pass_nr())
        {
            geometry.grid_clip = display.clip;
            geometry.grid_folders = display.folders;
            grid_layer = Some(display.layer);
        }
        if let Some(display) = self
            .folder_pane
            .label_display
            .as_ref()
            .filter(|display| display.pass == ctx.cumulative_pass_nr())
        {
            geometry.pane_rect = display.pane_rect;
            geometry.tree_clip = display.rect;
            tree_layer = Some(display.layer);
            geometry.rows = display
                .rows
                .iter()
                .filter_map(|row| {
                    let rect = row.row_rect.intersect(display.rect);
                    rect.is_positive().then(|| DropRow {
                        rect,
                        path: row.path.clone(),
                    })
                })
                .collect();
        }
        CapturedMainDropGeometry {
            geometry,
            tree_layer,
            grid_layer,
        }
    }

    #[cfg(test)]
    pub(crate) fn main_drop_geometry(
        &self,
        ctx: &egui::Context,
        metrics: crate::main_drop_target::WindowMetrics,
    ) -> DropGeometry {
        finalize_main_drop_geometry(
            ctx,
            self.capture_main_drop_geometry_with_metrics(ctx, metrics),
        )
    }

    pub(crate) fn paste_files_to_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.start_shell_paste(ctx, folder, ShellPasteOrigin::CtrlV);
    }

    pub(crate) fn paste_tree_files_to_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.start_shell_paste(ctx, folder, ShellPasteOrigin::Tree);
    }

    pub(crate) fn paste_grid_files_to_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.start_shell_paste(ctx, folder, ShellPasteOrigin::GridMenu);
    }

    pub(crate) fn start_shell_paste(
        &mut self,
        ctx: &egui::Context,
        folder: PathBuf,
        origin: ShellPasteOrigin,
    ) {
        self.start_shell_paste_with(ctx, folder, origin, Self::book_root_path);
    }

    pub(crate) fn start_shell_paste_with(
        &mut self,
        ctx: &egui::Context,
        folder: PathBuf,
        origin: ShellPasteOrigin,
        resolve_books_root: impl FnOnce(&Self) -> PathBuf,
    ) {
        // Reject read-only before mutation admission retires the book-append receipt.
        if self.shell_paste_request.is_some() {
            self.show_feedback_warning(SHELL_PASTE_BUSY_MESSAGE.to_owned());
            return;
        }
        if origin == ShellPasteOrigin::Tree
            && let Err(reason) =
                self.shell_transfer_submission_admission_with(ctx, &folder, resolve_books_root)
        {
            crate::logger::log(format!("shell_transfer: rejected: {reason}"));
            self.show_feedback_warning(reason);
            return;
        }
        let Some(hwnd) = self.main_hwnd else { return };
        if !self.admit_book_append_followup_mutation() {
            return;
        }
        let selection_id = (self.is_physical_folder_listing()
            && self
                .current_folder
                .as_deref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, &folder)))
        .then(|| self.request_post_operation_selection_for_added_items(folder.clone()));
        let started_at = std::time::Instant::now();
        let perf_seq = self.input_seq;
        if crate::perf::is_enabled() {
            crate::perf::event(
                "shell_paste",
                "submit",
                None,
                perf_seq,
                &[("origin", serde_json::Value::from(origin.perf_tag()))],
            );
        }
        match self.shell_paste_service.submit(hwnd, folder, ctx.clone()) {
            Ok(rx) => {
                self.shell_paste_request = Some(ShellPasteRunning {
                    rx,
                    selection_id,
                    origin,
                    started_at,
                    perf_seq,
                });
                self.refresh_main_drop_admission(ctx);
            }
            Err(error) => {
                crate::logger::log(format!(
                    "shell_paste: invocation submission failed: {error}"
                ));
                self.finish_shell_paste_invocation(
                    ctx,
                    selection_id,
                    origin,
                    started_at,
                    perf_seq,
                    ShellPasteOutcome::StartFailed,
                    Some(SHELL_PASTE_START_FAILURE),
                );
            }
        }
    }

    pub(crate) fn poll_shell_paste(&mut self, ctx: &egui::Context) {
        let Some(running) = self.shell_paste_request.take() else {
            return;
        };
        let completion = match running.rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                self.shell_paste_request = Some(running);
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                crate::logger::log("shell_paste: invocation worker disconnected");
                None
            }
            Ok(completion) => Some(completion),
        };
        let (outcome, warning) = match completion.map(|c| c.invocation) {
            None => (
                ShellPasteOutcome::Disconnected,
                Some(SHELL_PASTE_COMPLETION_FAILURE),
            ),
            Some(Ok(())) => (ShellPasteOutcome::Invoked, None),
            Some(Err(ShellPasteError::Start(error))) => {
                crate::logger::log(format!(
                    "shell_paste: invocation worker unavailable: {error}"
                ));
                (
                    ShellPasteOutcome::StartFailed,
                    Some(SHELL_PASTE_START_FAILURE),
                )
            }
            Some(Err(ShellPasteError::Invocation(error))) => {
                crate::logger::log(format!("shell_paste: invocation failed: {error}"));
                (ShellPasteOutcome::Failed, None)
            }
        };
        self.finish_shell_paste_invocation(
            ctx,
            running.selection_id,
            running.origin,
            running.started_at,
            running.perf_seq,
            outcome,
            warning,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_shell_paste_invocation(
        &mut self,
        ctx: &egui::Context,
        selection_id: Option<crate::post_operation_selection::PostOperationSelectionId>,
        origin: ShellPasteOrigin,
        started_at: std::time::Instant,
        perf_seq: u64,
        outcome: ShellPasteOutcome,
        warning: Option<&str>,
    ) {
        let retire_selection = match outcome {
            ShellPasteOutcome::Invoked => false,
            ShellPasteOutcome::Failed
            | ShellPasteOutcome::StartFailed
            | ShellPasteOutcome::Disconnected => true,
        };
        if retire_selection
            && let Some(id) = selection_id
            && self
                .post_operation_selection
                .as_ref()
                .is_some_and(|request| request.id() == id)
        {
            self.cancel_post_operation_selection("shell_paste_invocation_failed");
        }
        if let Some(warning) = warning {
            self.show_feedback_warning(warning.to_owned());
        }
        Self::resync_egui_modifiers_from_os(ctx);
        self.refresh_main_drop_admission(ctx);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        if cfg!(test) || crate::perf::is_enabled() {
            let fields = [
                ("origin", serde_json::Value::from(origin.perf_tag())),
                (
                    "ms",
                    serde_json::Value::from(started_at.elapsed().as_secs_f64() * 1000.0),
                ),
                ("outcome", serde_json::Value::from(outcome.perf_tag())),
            ];
            #[cfg(test)]
            SHELL_PASTE_END_FIELDS.with_borrow_mut(|events| {
                events.push(
                    fields
                        .iter()
                        .map(|(key, value)| ((*key).to_owned(), value.clone()))
                        .collect::<serde_json::Map<_, _>>()
                        .into(),
                );
            });
            crate::perf::event("shell_paste", "end", None, perf_seq, &fields);
        }
    }
}

fn finalize_main_drop_geometry(
    ctx: &egui::Context,
    mut captured: CapturedMainDropGeometry,
) -> DropGeometry {
    if let Some(layer) = captured.tree_layer {
        let front = front_layer_rects(ctx, layer);
        captured
            .geometry
            .rows
            .retain(|row| !front.iter().any(|front| front.intersects(row.rect)));
    }
    if let Some(layer) = captured.grid_layer {
        let front = front_layer_rects(ctx, layer);
        let blocked = &mut captured.geometry.grid_blocked;
        captured.geometry.grid_folders.retain(|row| {
            if front.iter().any(|front| front.intersects(row.rect)) {
                blocked.push(row.rect);
                false
            } else {
                true
            }
        });
    }
    captured.geometry
}

/// Entire intersecting rows are removed; a few sampled points cannot prove visibility.
fn front_layer_rects(ctx: &egui::Context, tree: egui::LayerId) -> Vec<egui::Rect> {
    let layers = ctx.memory(|memory| {
        let ordered: Vec<_> = memory.layer_ids().collect();
        let tree_index = ordered.iter().position(|layer| *layer == tree);
        ordered
            .iter()
            .enumerate()
            .filter(|(index, layer)| {
                memory.areas().is_visible(layer)
                    && (layer.order > tree.order
                        || (layer.order == tree.order
                            && tree_index.is_some_and(|tree| *index > tree)))
            })
            .filter_map(|(_, layer)| memory.area_rect(layer.id).map(|rect| (*layer, rect)))
            .collect::<Vec<_>>()
    });
    layers
        .into_iter()
        // Areas keeps last-pass visibility as well. Only layers painted in this pass occlude.
        .filter(|(layer, _)| {
            ctx.graphics(|graphics| {
                graphics.get(*layer).is_some_and(|list| {
                    list.all_entries()
                        .any(|entry| !matches!(&entry.shape, egui::Shape::Noop))
                })
            })
        })
        .map(|(layer, rect)| {
            ctx.layer_transform_to_global(layer)
                .map_or(rect, |transform| transform * rect)
        })
        .collect()
}

pub(crate) fn clipboard_has_drop_files() -> bool {
    #[cfg(windows)]
    unsafe {
        windows::Win32::System::DataExchange::IsClipboardFormatAvailable(
            windows::Win32::System::Ole::CF_HDROP.0 as u32,
        )
        .is_ok()
    }
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
#[path = "main_drop_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shell_paste_tests.rs"]
mod shell_paste_tests;
