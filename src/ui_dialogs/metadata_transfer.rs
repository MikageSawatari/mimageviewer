//! `ファイル` メニューの明示メタ情報エクスポート / インポート。
//!
//! ダイアログが存在する間は [`App::common_modal_dialog_open`] が背面入力を止める。
//! ファイル列挙・JSON・SQLite はすべて worker で行い、UI スレッドは進捗だけを受け取る。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;

use crate::app::App;
use crate::metadata_transfer::{
    ExportSummary, ImportPreview, ImportRefreshDelta, ImportRefreshScope, ImportSummary,
    TransferError, TransferPhase, TransferProgress,
};

const WORKER_CHANNEL_CAPACITY: usize = 1;
const MAX_MESSAGES_PER_FRAME: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Export,
    Import,
}

enum Stage {
    ExportOptions,
    LoadingPreview,
    ImportConfirm(ImportPreview),
    WaitingForWriters,
    Running,
    Result(ResultState),
}

enum ResultState {
    Export(Result<ExportSummary, String>),
    Import(Result<ImportSummary, String>),
}

enum WorkerMessage {
    Progress(TransferProgress),
    ImportRefresh(ImportRefreshDelta),
    Preview(Result<ImportPreview, String>),
    Export {
        result: Result<ExportSummary, String>,
        view_trim_saved: bool,
    },
    Import {
        result: Result<ImportSummary, String>,
        resource_error: Option<String>,
        page_state_snapshot: Option<crate::metadata_transfer::ImportPageStateSnapshot>,
        view_trim_saved: bool,
    },
}

struct ImportWorkerResources {
    sidecars: HashMap<PathBuf, crate::sidecar::SidecarFile>,
    tags_db: Option<crate::tags_db::TagsDb>,
}

struct ProgressRelay {
    tx: mpsc::SyncSender<WorkerMessage>,
    last_phase: Option<TransferPhase>,
    last_sent: std::time::Instant,
}

impl ProgressRelay {
    fn new(tx: mpsc::SyncSender<WorkerMessage>) -> Self {
        Self {
            tx,
            last_phase: None,
            last_sent: std::time::Instant::now() - std::time::Duration::from_secs(1),
        }
    }

    fn send(&mut self, progress: TransferProgress) {
        let phase_changed = self.last_phase != Some(progress.phase);
        let completed = progress.total > 0 && progress.processed >= progress.total;
        if phase_changed
            || completed
            || self.last_sent.elapsed() >= std::time::Duration::from_millis(40)
        {
            self.last_phase = Some(progress.phase);
            self.last_sent = std::time::Instant::now();
            let _ = self.tx.send(WorkerMessage::Progress(progress));
        }
    }
}

pub(crate) struct MetadataTransferState {
    root: PathBuf,
    operation: Operation,
    recursive: bool,
    stage: Stage,
    progress: Option<TransferProgress>,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<WorkerMessage>>,
    handle: Option<std::thread::JoinHandle<()>>,
    import_resources: Option<Arc<Mutex<ImportWorkerResources>>>,
    pending_view_trim: Option<crate::ui_view_trim::PendingViewTrimTransfer>,
    import_refresh_scope: ImportRefreshScope,
    import_refresh_preparing: bool,
}

impl MetadataTransferState {
    fn export(root: PathBuf) -> Self {
        Self {
            root,
            operation: Operation::Export,
            recursive: false,
            stage: Stage::ExportOptions,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            handle: None,
            import_resources: None,
            pending_view_trim: None,
            import_refresh_scope: ImportRefreshScope::default(),
            import_refresh_preparing: false,
        }
    }

    fn import(root: PathBuf) -> Self {
        Self {
            root,
            operation: Operation::Import,
            recursive: false,
            stage: Stage::LoadingPreview,
            progress: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            handle: None,
            import_resources: None,
            pending_view_trim: None,
            import_refresh_scope: ImportRefreshScope::default(),
            import_refresh_preparing: false,
        }
    }
}

impl Drop for MetadataTransferState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // bounded channelでworkerがrefresh送信待ちの場合、receiverを先にdropして解除する。
        self.rx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

enum DialogAction {
    None,
    StartExport,
    StartImport,
    Cancel,
    Close,
}

impl App {
    /// 明示メタ情報転送の対象。ZIP/PDF/合成ビューではなく、現在表示中の実フォルダだけ。
    pub(crate) fn metadata_transfer_target(&self) -> Option<PathBuf> {
        if self.archive_source_override.is_some()
            || self.items_are_drive_list
            || self.items_are_global_search_view
            || self.items_are_tag_view
            || self.items_are_reading_history_view
            || self.items_are_bookmark_view
            || self.items_are_rating_view
            || self.items_are_subfolder_expansion_view
            || self.current_smart_folder_id.is_some()
            || self.global_search.active
            || self.favsearch.active
            || self.tag_view.active
            || self.current_folder_last_mtime.is_none()
            || self.pdf_enumerate_pending.is_some()
            || self.zip_enumerate_pending.is_some()
            || self.show_pdf_password_dialog
            || self.pdf_password_pending_path.is_some()
        {
            return None;
        }
        self.current_folder.clone()
    }

    pub(crate) fn open_metadata_export_dialog(&mut self) {
        let Some(root) = self.metadata_transfer_target() else {
            self.show_feedback_toast("実フォルダを表示しているときに使用できます".to_string());
            return;
        };
        self.metadata_transfer = Some(MetadataTransferState::export(root));
    }

    pub(crate) fn open_metadata_import_dialog(&mut self) {
        let Some(root) = self.metadata_transfer_target() else {
            self.show_feedback_toast("実フォルダを表示しているときに使用できます".to_string());
            return;
        };
        let mut state = MetadataTransferState::import(root);
        start_preview_worker(&mut state);
        self.metadata_transfer = Some(state);
    }

    /// 転送 worker と同じ DB を書き換え得る既存処理が完全に着地したか。
    /// モーダルはユーザー入力を止めるが、開始前から走っていた worker までは止めないため、
    /// この drain 待ちを必ず通してから export/import を開始する。
    fn metadata_transfer_writers_busy(&self) -> bool {
        self.rename_migration_writers_busy()
            || self.tag_maintenance_rx.is_some()
            || self
                .tag_prewarm_pending
                .as_ref()
                .is_some_and(|pending| pending.is_busy())
            || self.tag_legacy_seed_pending.is_some()
            || self.tag_legacy_xmp_pending.is_some()
            || self.rename_migration_in_flight.is_some()
            || !self.rename_migration_queue.is_empty()
            || self.metadata_cleanup_pending.is_some()
            || self.delete_purge_retry_pending.is_some()
            || !self.book_bookmark_pending_requests.is_empty()
            || self.bookmark_delete_pending.is_some()
            || !self.drop_copy_pending.is_empty()
            || self.new_folder_pending.is_some()
            || self.rename_pending.is_some()
            || self.delete_pending.is_some()
            || self.book_op_pending.is_some()
            || self.capture_pending.is_some()
            || self.export_pending.is_some()
            || self.batch_convert.is_some()
    }

    pub(crate) fn show_metadata_transfer_dialog(&mut self, ctx: &egui::Context) {
        if self.metadata_transfer.is_none() {
            return;
        }
        self.poll_metadata_transfer_worker();

        let writers_ready = self.metadata_transfer.as_ref().is_some_and(|state| {
            matches!(state.stage, Stage::WaitingForWriters)
                && !self.metadata_transfer_writers_busy()
        });
        if writers_ready
            && self.metadata_transfer.as_ref().is_some_and(|state| {
                state.operation == Operation::Import && !state.import_refresh_preparing
            })
        {
            self.begin_metadata_import_refresh_scope();
            if let Some(state) = self.metadata_transfer.as_mut() {
                state.import_refresh_preparing = true;
                state.import_refresh_scope = ImportRefreshScope::default();
            }
        }
        let import_refresh_ready = if writers_ready
            && self.metadata_transfer.as_ref().is_some_and(|state| {
                state.operation == Operation::Import && state.import_refresh_preparing
            }) {
            let mut scope = self
                .metadata_transfer
                .as_mut()
                .map(|state| std::mem::take(&mut state.import_refresh_scope))
                .unwrap_or_default();
            let ready = self.advance_metadata_import_refresh_scope(&mut scope);
            if let Some(state) = self.metadata_transfer.as_mut() {
                state.import_refresh_scope = scope;
            }
            ready
        } else {
            false
        };
        let should_start_worker = writers_ready
            && self.metadata_transfer.as_ref().is_some_and(|state| {
                state.operation == Operation::Export
                    || (state.operation == Operation::Import && import_refresh_ready)
            });
        if should_start_worker {
            let operation = self
                .metadata_transfer
                .as_ref()
                .map(|state| state.operation)
                .expect("state checked above");
            // writer の結果反映で view-trim が再び dirty になり得るため、実行直前の
            // UI 状態をメモリ snapshot する。SQLite write は transfer worker に渡す。
            let pending_view_trim = self.take_pending_view_trim_transfer();
            let mut restore_view_trim = None;
            let mut import_start_failed = false;
            match operation {
                Operation::Export => {
                    if let Some(state) = self.metadata_transfer.as_mut() {
                        state.pending_view_trim = pending_view_trim;
                        let batch = state
                            .pending_view_trim
                            .as_ref()
                            .map(|pending| pending.batch.clone());
                        if !start_export_worker(state, batch) {
                            restore_view_trim = state.pending_view_trim.take();
                        }
                    }
                }
                Operation::Import => {
                    // JSON serialize / temp write / rename and tags.db journal-mode
                    // handoff are part of the worker preparation stage.  Move their
                    // ownership instead of doing file I/O on the UI thread.
                    let refresh_scope = self
                        .metadata_transfer
                        .as_mut()
                        .map(|state| std::mem::take(&mut state.import_refresh_scope))
                        .unwrap_or_default();
                    let resources = ImportWorkerResources {
                        sidecars: std::mem::take(&mut self.sidecars),
                        tags_db: self.tags_db.take(),
                    };
                    if let Some(state) = self.metadata_transfer.as_mut() {
                        state.pending_view_trim = pending_view_trim;
                        let batch = state
                            .pending_view_trim
                            .as_ref()
                            .map(|pending| pending.batch.clone());
                        if !start_import_worker(state, resources, batch, refresh_scope) {
                            restore_view_trim = state.pending_view_trim.take();
                            import_start_failed = true;
                        }
                    }
                }
            }
            if let Some(pending) = restore_view_trim {
                self.restore_pending_view_trim_transfer(pending);
            }
            if import_start_failed {
                self.finish_metadata_transfer_import_refresh();
            }
        }
        // Thread creation failure reaches Result without a receiver message, so
        // return the moved DB/sidecar ownership in the same frame as well.
        self.restore_metadata_import_resources();

        let escape_pressed = self.dialog_escape_pressed(ctx);
        let action = {
            let state = self.metadata_transfer.as_mut().expect("checked above");
            draw_dialog(ctx, state, escape_pressed)
        };
        match action {
            DialogAction::None => {}
            DialogAction::StartExport => {
                if let Some(state) = self.metadata_transfer.as_mut() {
                    state.stage = Stage::WaitingForWriters;
                    state.progress = None;
                }
            }
            DialogAction::StartImport => {
                if let Some(state) = self.metadata_transfer.as_mut() {
                    state.stage = Stage::WaitingForWriters;
                    state.progress = None;
                }
            }
            DialogAction::Cancel => {
                let close_now = self.metadata_transfer.as_ref().is_some_and(|state| {
                    matches!(
                        state.stage,
                        Stage::ExportOptions
                            | Stage::ImportConfirm(_)
                            | Stage::WaitingForWriters
                            | Stage::Result(_)
                    )
                });
                if close_now {
                    self.metadata_transfer = None;
                } else if let Some(state) = self.metadata_transfer.as_ref() {
                    state.cancel.store(true, Ordering::Relaxed);
                }
            }
            DialogAction::Close => self.metadata_transfer = None,
        }

        if self.metadata_transfer.as_ref().is_some_and(|state| {
            matches!(
                state.stage,
                Stage::LoadingPreview | Stage::WaitingForWriters | Stage::Running
            )
        }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn poll_metadata_transfer_worker(&mut self) {
        let mut messages = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = self
            .metadata_transfer
            .as_ref()
            .and_then(|state| state.rx.as_ref())
        {
            for _ in 0..MAX_MESSAGES_PER_FRAME {
                match rx.try_recv() {
                    Ok(message) => {
                        let is_refresh = matches!(message, WorkerMessage::ImportRefresh(_));
                        messages.push(message);
                        if is_refresh {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let mut imported_refresh = None;
        let mut import_finished = false;
        let mut page_state_snapshot = None;
        let mut restore_view_trim = None;
        for message in messages {
            let Some(state) = self.metadata_transfer.as_mut() else {
                break;
            };
            match message {
                WorkerMessage::Progress(progress) => state.progress = Some(progress),
                WorkerMessage::ImportRefresh(refresh) => {
                    imported_refresh = Some(refresh);
                }
                WorkerMessage::Preview(result) => {
                    state.rx = None;
                    state.stage = match result {
                        Ok(preview) => Stage::ImportConfirm(preview),
                        Err(error) => Stage::Result(ResultState::Import(Err(error))),
                    };
                }
                WorkerMessage::Export {
                    result,
                    view_trim_saved,
                } => {
                    if view_trim_saved {
                        state.pending_view_trim = None;
                    } else {
                        restore_view_trim = state.pending_view_trim.take();
                    }
                    state.rx = None;
                    state.stage = Stage::Result(ResultState::Export(result));
                }
                WorkerMessage::Import {
                    mut result,
                    resource_error,
                    page_state_snapshot: snapshot,
                    view_trim_saved,
                } => {
                    page_state_snapshot = snapshot;
                    if view_trim_saved {
                        state.pending_view_trim = None;
                    } else {
                        restore_view_trim = state.pending_view_trim.take();
                    }
                    if let Some(error) = resource_error {
                        result = match result {
                            Ok(mut summary) => {
                                summary.incomplete_error = Some(
                                    summary
                                        .incomplete_error
                                        .map_or(error.clone(), |import_error| {
                                            format!("{import_error}\n{error}")
                                        }),
                                );
                                Ok(summary)
                            }
                            Err(import_error) => Err(format!("{import_error}\n{error}")),
                        };
                    }
                    state.rx = None;
                    state.stage = Stage::Result(ResultState::Import(result));
                    import_finished = true;
                }
            }
        }
        if disconnected
            && self.metadata_transfer.as_ref().is_some_and(|state| {
                state.rx.is_some() && matches!(state.stage, Stage::LoadingPreview | Stage::Running)
            })
            && let Some(state) = self.metadata_transfer.as_mut()
        {
            restore_view_trim = state.pending_view_trim.take();
            state.rx = None;
            state.stage = match state.operation {
                Operation::Export => Stage::Result(ResultState::Export(Err(
                    "worker が予期せず終了しました".to_string(),
                ))),
                Operation::Import => {
                    import_finished = true;
                    Stage::Result(ResultState::Import(Err(
                        "worker が予期せず終了しました".to_string()
                    )))
                }
            };
        }
        if let Some(pending) = restore_view_trim {
            self.restore_pending_view_trim_transfer(pending);
        }
        self.restore_metadata_import_resources();
        if let Some(refresh) = imported_refresh {
            self.refresh_after_metadata_transfer_import(refresh);
        }
        if let Some(snapshot) = page_state_snapshot {
            self.replace_metadata_import_page_state_snapshot(snapshot);
        }
        if import_finished {
            self.finish_metadata_transfer_import_refresh();
        }
    }

    fn restore_metadata_import_resources(&mut self) {
        let should_restore = self.metadata_transfer.as_ref().is_some_and(|state| {
            state.import_resources.is_some()
                && matches!(state.stage, Stage::Result(ResultState::Import(_)))
        });
        if !should_restore {
            return;
        }
        let Some(resources) = self
            .metadata_transfer
            .as_mut()
            .and_then(|state| state.import_resources.take())
        else {
            return;
        };
        let mut resources = match resources.lock() {
            Ok(resources) => resources,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.sidecars = std::mem::take(&mut resources.sidecars);
        self.tags_db = resources.tags_db.take();
    }
}

fn draw_dialog(
    ctx: &egui::Context,
    state: &mut MetadataTransferState,
    escape_pressed: bool,
) -> DialogAction {
    let mut action = DialogAction::None;
    egui::Modal::new(egui::Id::new("metadata_transfer_modal")).show(ctx, |ui| {
        ui.set_min_width(430.0);
        ui.heading(match state.operation {
            Operation::Export => "メタ情報をエクスポート",
            Operation::Import => "メタ情報をインポート",
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(state.root.display().to_string())
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(8.0);

        match &mut state.stage {
            Stage::ExportOptions => {
                ui.checkbox(&mut state.recursive, "サブフォルダも含める（再帰）");
                ui.add_space(6.0);
                ui.label(format!(
                    "同じフォルダの {} フォルダに保存します。",
                    crate::metadata_transfer::SIDECAR_FILENAME
                ));
                ui.small("既存のメタ情報は、完了時に新しい世代へ切り替えます。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("エクスポート").clicked() {
                        action = DialogAction::StartExport;
                    }
                    if ui.button("キャンセル").clicked() {
                        action = DialogAction::Cancel;
                    }
                });
                if escape_pressed {
                    action = DialogAction::Cancel;
                }
            }
            Stage::LoadingPreview => {
                ui.label("メタ情報を確認しています…");
                ui.spinner();
                if ui.button("キャンセル").clicked() || escape_pressed {
                    action = DialogAction::Cancel;
                }
            }
            Stage::ImportConfirm(preview) => {
                ui.label(if preview.recursive {
                    "範囲: サブフォルダを含む"
                } else {
                    "範囲: このフォルダ直下"
                });
                ui.label(format!("記載項目: {} 件", preview.entries));
                ui.label(format!("適用可能: {} 件", preview.existing_entries));
                if preview.missing_entries > 0 {
                    ui.label(format!(
                        "見つからないためスキップ: {} 件",
                        preview.missing_entries
                    ));
                }
                if preview.changed_files > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("サイズが異なるためスキップ: {} 件", preview.changed_files),
                    );
                }
                ui.add_space(6.0);
                ui.label("記載されたファイル単位で、付随メタ情報を上書きします。");
                ui.small(
                    "評価・タグ・ブックマーク・見開き・表示トリム・回転・ページ編集・代表サムネを含みます。",
                );
                ui.small("このメタ情報に記載がない項目の既存メタ情報は保持します。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("インポート").clicked() {
                        action = DialogAction::StartImport;
                    }
                    if ui.button("キャンセル").clicked() {
                        action = DialogAction::Cancel;
                    }
                });
                if escape_pressed {
                    action = DialogAction::Cancel;
                }
            }
            Stage::WaitingForWriters => {
                ui.label(if state.import_refresh_preparing {
                    "表示更新の準備をしています…"
                } else {
                    "実行中のメタ情報書き込みが完了するのを待っています…"
                });
                ui.spinner();
                ui.small("待機中も安全にキャンセルできます。");
                if ui.button("キャンセル").clicked() || escape_pressed {
                    action = DialogAction::Cancel;
                }
            }
            Stage::Running => {
                draw_progress(ui, state.progress.as_ref());
                let cancelling = state.cancel.load(Ordering::Relaxed);
                if cancelling {
                    ui.label("キャンセル処理中…");
                } else if ui.button("キャンセル").clicked() || escape_pressed {
                    action = DialogAction::Cancel;
                }
                if state.operation == Operation::Import {
                    ui.small("キャンセル時も、反映済みの項目はそのまま保持されます。");
                }
            }
            Stage::Result(result) => {
                draw_result(ui, result);
                ui.add_space(8.0);
                if ui.button("閉じる").clicked() || escape_pressed {
                    action = DialogAction::Close;
                }
            }
        }
    });
    action
}

fn draw_progress(ui: &mut egui::Ui, progress: Option<&TransferProgress>) {
    let Some(progress) = progress else {
        ui.spinner();
        ui.label("準備中…");
        draw_progress_path(ui, None);
        return;
    };
    let label = match progress.phase {
        TransferPhase::Scanning => "ファイルを列挙中",
        TransferPhase::ReadingMetadata => "メタ情報を読み取り中",
        TransferPhase::WritingSidecar => "メタ情報を書き込み中",
        TransferPhase::ReadingSidecar => "メタ情報を確認中",
        TransferPhase::Importing => "メタ情報を反映中",
    };
    ui.label(label);
    if progress.total > 0 {
        ui.label(format!("{} / {}", progress.processed, progress.total));
        ui.add(
            egui::ProgressBar::new(progress.processed as f32 / progress.total as f32)
                .show_percentage(),
        );
    } else {
        ui.spinner();
        ui.label(format!("{} 件", progress.processed));
    }
    draw_progress_path(ui, progress.current_path.as_deref());
}

const PROGRESS_PATH_ROWS: f32 = 3.0;

/// 現在処理中のpathが1〜3行へ折り返されても、後続buttonの位置を動かさない。
/// 3行を超える部分はclipし、hover時に全文を確認できるようにする。
fn draw_progress_path(ui: &mut egui::Ui, path: Option<&str>) {
    let row_height = ui.text_style_height(&egui::TextStyle::Small);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height * PROGRESS_PATH_ROWS),
        egui::Sense::hover(),
    );
    let Some(path) = path else {
        return;
    };
    let text_color = ui.visuals().weak_text_color();
    let mut path_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    path_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    path_ui.add(
        egui::Label::new(egui::RichText::new(path).small().color(text_color))
            .wrap()
            .selectable(false),
    );
    response.on_hover_text(path);
}

#[cfg(test)]
fn progress_widget_height(progress: TransferProgress) -> f32 {
    use std::sync::{Arc, Mutex};

    let height = Arc::new(Mutex::new(None));
    let captured_height = Arc::clone(&height);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(430.0, 240.0))
        .build(move |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let top = ui.cursor().top();
                draw_progress(ui, Some(&progress));
                *captured_height.lock().unwrap() = Some(ui.cursor().top() - top);
            });
        });
    harness.run();
    let result = height.lock().unwrap().unwrap();
    result
}

#[cfg(test)]
fn test_progress(path: &str) -> TransferProgress {
    TransferProgress {
        phase: TransferPhase::ReadingMetadata,
        processed: 10,
        total: 100,
        current_path: Some(path.to_string()),
    }
}

fn draw_result(ui: &mut egui::Ui, result: &ResultState) {
    match result {
        ResultState::Export(Ok(summary)) => {
            ui.label("エクスポートが完了しました。");
            ui.label(format!(
                "{} 項目 / 評価 {} / タグ付き {} / 時刻ブックマーク {} / 本ブックマーク {} / ページ設定 {} / 本・フォルダ設定 {} / サムネピン {}",
                summary.entries,
                summary.ratings,
                summary.tagged_items,
                summary.timed_bookmarks,
                summary.book_bookmarks,
                summary.page_states,
                summary.container_states,
                summary.thumbnail_pins,
            ));
        }
        ResultState::Import(Ok(summary)) => {
            if summary.cancelled {
                ui.label("インポートをキャンセルしました。反映済みの項目は保持されています。");
            } else if summary.incomplete_error.is_some() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "途中でエラーが発生しました。反映済みの項目は保持されています。",
                );
            } else {
                ui.label("インポートが完了しました。");
            }
            ui.label(format!(
                "反映 {} / 見つからない {} / サイズ相違 {} / 失敗 {}（全 {} 項目）",
                summary.applied_entries,
                summary.skipped_missing,
                summary.skipped_changed,
                summary.failed_entries,
                summary.total_entries
            ));
            if let Some(error) = summary.incomplete_error.as_deref() {
                ui.label(error);
            }
        }
        ResultState::Export(Err(error)) | ResultState::Import(Err(error)) => {
            if error == &TransferError::Cancelled.to_string() {
                ui.label("キャンセルしました。");
            } else {
                ui.colored_label(ui.visuals().error_fg_color, "処理を完了できませんでした。");
                ui.label(error);
            }
        }
    }
}

fn start_preview_worker(state: &mut MetadataTransferState) {
    let root = state.root.clone();
    let cancel = Arc::clone(&state.cancel);
    let (tx, rx) = mpsc::sync_channel(WORKER_CHANNEL_CAPACITY);
    state.rx = Some(rx);
    state.stage = Stage::LoadingPreview;
    let spawn = std::thread::Builder::new()
        .name("metadata-import-preview".to_string())
        .spawn(move || {
            let mut progress_relay = ProgressRelay::new(tx.clone());
            let result =
                crate::metadata_transfer::inspect_import_at(&root, &cancel, move |value| {
                    progress_relay.send(value);
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(WorkerMessage::Preview(result));
        });
    match spawn {
        Ok(handle) => state.handle = Some(handle),
        Err(error) => {
            state.rx = None;
            state.stage = Stage::Result(ResultState::Import(Err(error.to_string())));
        }
    }
}

fn start_export_worker(
    state: &mut MetadataTransferState,
    view_trim_batch: Option<crate::view_trim_db::ViewTrimWriteBatch>,
) -> bool {
    let root = state.root.clone();
    let recursive = state.recursive;
    let data_dir = crate::data_dir::get();
    let cancel = Arc::clone(&state.cancel);
    cancel.store(false, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel(WORKER_CHANNEL_CAPACITY);
    state.rx = Some(rx);
    state.stage = Stage::Running;
    let spawn = std::thread::Builder::new()
        .name("metadata-export".to_string())
        .spawn(move || {
            let mut progress_relay = ProgressRelay::new(tx.clone());
            let preparation = flush_view_trim_batch(&data_dir, view_trim_batch.as_ref(), "export");
            let view_trim_saved = preparation.is_ok();
            let result = preparation.and_then(|()| {
                crate::metadata_transfer::export_at(
                    &data_dir,
                    &root,
                    recursive,
                    &cancel,
                    move |value| {
                        progress_relay.send(value);
                    },
                )
                .map_err(|error| error.to_string())
            });
            let _ = tx.send(WorkerMessage::Export {
                result,
                view_trim_saved,
            });
        });
    match spawn {
        Ok(handle) => {
            state.handle = Some(handle);
            true
        }
        Err(error) => {
            state.rx = None;
            state.stage = Stage::Result(ResultState::Export(Err(error.to_string())));
            false
        }
    }
}

fn start_import_worker(
    state: &mut MetadataTransferState,
    resources: ImportWorkerResources,
    view_trim_batch: Option<crate::view_trim_db::ViewTrimWriteBatch>,
    refresh_scope: crate::metadata_transfer::ImportRefreshScope,
) -> bool {
    if let Some(preview_handle) = state.handle.take() {
        let _ = preview_handle.join();
    }
    let root = state.root.clone();
    let data_dir = crate::data_dir::get();
    let cancel = Arc::clone(&state.cancel);
    cancel.store(false, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel(WORKER_CHANNEL_CAPACITY);
    state.rx = Some(rx);
    state.stage = Stage::Running;
    let resources = Arc::new(Mutex::new(resources));
    state.import_resources = Some(Arc::clone(&resources));
    let spawn = std::thread::Builder::new()
        .name("metadata-import".to_string())
        .spawn(move || {
            // tags.db is normally held open in WAL mode by App.  Drop that idle
            // connection on this worker before metadata_transfer switches it to
            // DELETE journal mode for a crash-atomic attached transaction.
            let tags_db = {
                let mut resources = match resources.lock() {
                    Ok(resources) => resources,
                    Err(poisoned) => poisoned.into_inner(),
                };
                resources.tags_db.take()
            };
            drop(tags_db);

            let mut progress_relay = ProgressRelay::new(tx.clone());
            let view_trim_result =
                flush_view_trim_batch(&data_dir, view_trim_batch.as_ref(), "import");
            let view_trim_saved = view_trim_result.is_ok();
            let flush_result = view_trim_result.and_then(|()| flush_import_sidecars(&resources));
            let refresh_tx = tx.clone();
            let result = match flush_result {
                Ok(()) => crate::metadata_transfer::import_at_with_refresh_scope(
                    &data_dir,
                    &root,
                    &cancel,
                    Some(&refresh_scope),
                    move |value| {
                        progress_relay.send(value);
                    },
                    move |refresh| {
                        refresh_tx
                            .send(WorkerMessage::ImportRefresh(refresh))
                            .map_err(|_| TransferError::Cancelled)
                    },
                )
                .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };

            // Reopening restores tags.db to its normal WAL mode before the UI
            // regains ownership.  Report a reopen failure even when DB import
            // itself succeeded; the caller still extracts the applied-key delta.
            let (tags_db, mut resource_error) =
                match crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db")) {
                    Ok(tags_db) => (Some(tags_db), None),
                    Err(error) => (None, Some(format!("タグDBを再開できませんでした: {error}"))),
                };
            {
                let mut resources = match resources.lock() {
                    Ok(resources) => resources,
                    Err(poisoned) => poisoned.into_inner(),
                };
                resources.tags_db = tags_db;
            }
            let page_state_snapshot =
                match crate::metadata_transfer::load_import_page_state_snapshot(&data_dir) {
                    Ok(snapshot) => Some(snapshot),
                    Err(error) => {
                        let error = format!("編集状態索引を再構築できませんでした: {error}");
                        crate::logger::log(format!("metadata import: {error}"));
                        resource_error = Some(
                            resource_error
                                .map_or(error.clone(), |existing| format!("{existing}\n{error}")),
                        );
                        None
                    }
                };
            let _ = tx.send(WorkerMessage::Import {
                result,
                resource_error,
                page_state_snapshot,
                view_trim_saved,
            });
        });
    match spawn {
        Ok(handle) => {
            state.handle = Some(handle);
            true
        }
        Err(error) => {
            state.rx = None;
            state.stage = Stage::Result(ResultState::Import(Err(error.to_string())));
            false
        }
    }
}

fn flush_view_trim_batch(
    data_dir: &std::path::Path,
    batch: Option<&crate::view_trim_db::ViewTrimWriteBatch>,
    operation: &str,
) -> Result<(), String> {
    let Some(batch) = batch else {
        return Ok(());
    };
    let mut db = crate::view_trim_db::ViewTrimDb::open_at(&data_dir.join("view_trim.db")).map_err(
        |error| format!("表示トリムDBを開けないため、{operation}を開始しませんでした: {error}"),
    )?;
    db.apply_write_batch(batch).map_err(|error| {
        format!("未保存の表示トリムを保存できないため、{operation}を開始しませんでした: {error}")
    })
}

fn flush_import_sidecars(resources: &Arc<Mutex<ImportWorkerResources>>) -> Result<(), String> {
    let mut resources = match resources.lock() {
        Ok(resources) => resources,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut failed = Vec::new();
    for sidecar in resources.sidecars.values_mut() {
        if !sidecar.flush() {
            failed.push(sidecar.folder().display().to_string());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        const MAX_REPORTED: usize = 3;
        let omitted = failed.len().saturating_sub(MAX_REPORTED);
        let mut detail = failed
            .iter()
            .take(MAX_REPORTED)
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        if omitted > 0 {
            detail.push_str(&format!("、ほか {omitted} フォルダ"));
        }
        Err(format!(
            "既存の自動バックアップを保存できなかったため、importを開始しませんでした: {detail}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_preparation_flushes_dirty_sidecars_off_thread() {
        let temp = tempfile::TempDir::new().unwrap();
        let folder = temp.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
        sidecar.set_adjust("page.jpg", crate::adjustment::AdjustParams::default());
        let resources = Arc::new(Mutex::new(ImportWorkerResources {
            sidecars: HashMap::from([(folder.clone(), sidecar)]),
            tags_db: None,
        }));
        let worker_resources = Arc::clone(&resources);
        std::thread::Builder::new()
            .name("metadata-sidecar-flush-test".to_string())
            .spawn(move || flush_import_sidecars(&worker_resources))
            .unwrap()
            .join()
            .unwrap()
            .unwrap();

        let resources = resources.lock().unwrap();
        assert!(!resources.sidecars[&folder].is_dirty());
        assert!(folder.join(crate::sidecar::SIDECAR_FILENAME).is_file());
    }

    #[test]
    fn import_preparation_reports_sidecar_failure_before_db_work() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing_folder = temp.path().join("missing");
        let mut sidecar = crate::sidecar::SidecarFile::new(missing_folder.clone());
        sidecar.set_adjust("page.jpg", crate::adjustment::AdjustParams::default());
        let resources = Arc::new(Mutex::new(ImportWorkerResources {
            sidecars: HashMap::from([(missing_folder, sidecar)]),
            tags_db: None,
        }));

        let error = flush_import_sidecars(&resources).unwrap_err();
        assert!(error.contains("importを開始しませんでした"));
    }

    #[test]
    fn transfer_preparation_flushes_pending_view_trim_on_worker() {
        let temp = tempfile::TempDir::new().unwrap();
        let book = temp.path().join("book");
        let state = crate::view_trim::ViewTrimBookState {
            apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
            book_settings: crate::view_trim::ViewTrimBookSettings {
                enabled: true,
                ..Default::default()
            },
        };
        let batch = crate::view_trim_db::ViewTrimWriteBatch {
            book: Some((book.clone(), state)),
            pages: Vec::new(),
        };
        let data_dir = temp.path().to_path_buf();
        std::thread::Builder::new()
            .name("metadata-view-trim-flush-test".to_string())
            .spawn(move || flush_view_trim_batch(&data_dir, Some(&batch), "test"))
            .unwrap()
            .join()
            .unwrap()
            .unwrap();

        let db =
            crate::view_trim_db::ViewTrimDb::open_at(&temp.path().join("view_trim.db")).unwrap();
        assert_eq!(db.get_book_state(&book), Some(state));
    }

    #[test]
    fn progress_height_does_not_change_when_current_path_wraps() {
        let short = progress_widget_height(test_progress("C:/images/page.jpg"));
        let long = progress_widget_height(test_progress(
            "C:/very-long-folder-name/another-very-long-folder-name/\
             third-very-long-folder-name/fourth-very-long-folder-name/page.jpg",
        ));

        assert!(
            (short - long).abs() < f32::EPSILON,
            "progress widget height changed: short={short}, long={long}"
        );
    }
}
