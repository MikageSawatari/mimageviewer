//! `ファイル` メニューの明示メタ情報エクスポート / インポート。
//!
//! ダイアログが存在する間は [`App::common_modal_dialog_open`] が背面入力を止める。
//! ファイル列挙・JSON・SQLite はすべて worker で行い、UI スレッドは進捗だけを受け取る。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;

use crate::app::App;
use crate::metadata_transfer::{
    ExportSummary, ImportPreview, ImportSummary, TransferError, TransferPhase, TransferProgress,
};

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
    Preview(Result<ImportPreview, String>),
    Export(Result<ExportSummary, String>),
    Import(Result<ImportSummary, String>),
}

struct ProgressRelay {
    tx: mpsc::Sender<WorkerMessage>,
    last_phase: Option<TransferPhase>,
    last_sent: std::time::Instant,
}

impl ProgressRelay {
    fn new(tx: mpsc::Sender<WorkerMessage>) -> Self {
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
        }
    }
}

impl Drop for MetadataTransferState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
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

        let should_start = self.metadata_transfer.as_ref().is_some_and(|state| {
            matches!(state.stage, Stage::WaitingForWriters)
                && !self.metadata_transfer_writers_busy()
        });
        if should_start {
            let operation = self
                .metadata_transfer
                .as_ref()
                .map(|state| state.operation)
                .expect("state checked above");
            // writer の結果反映で sidecar / view-trim が再び dirty になり得るため、
            // WaitingForWriters を抜ける実行直前にも確定する。
            self.persist_pending_view_trim_state();
            if operation == Operation::Import {
                self.flush_all_sidecars();
            }
            if let Some(state) = self.metadata_transfer.as_mut() {
                match operation {
                    Operation::Export => start_export_worker(state),
                    Operation::Import => start_import_worker(state),
                }
            }
        }

        let escape_pressed = self.dialog_escape_pressed(ctx);
        let action = {
            let state = self.metadata_transfer.as_mut().expect("checked above");
            draw_dialog(ctx, state, escape_pressed)
        };
        match action {
            DialogAction::None => {}
            DialogAction::StartExport => {
                // 表示トリムは操作中に debounce 保存されるため、worker が DB snapshot を
                // 読む前に現在の編集を確定する。モーダル中は以後の入力が発生しない。
                self.persist_pending_view_trim_state();
                if let Some(state) = self.metadata_transfer.as_mut() {
                    state.stage = Stage::WaitingForWriters;
                    state.progress = None;
                }
            }
            DialogAction::StartImport => {
                // import 後の再 hydrate が、未保存の旧 UI 状態を DB へ書き戻さないよう
                // 適用開始前に dirty state を排出する。
                self.persist_pending_view_trim_state();
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
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
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
            loop {
                match rx.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let mut imported_rating_keys = Vec::new();
        let mut imported_page_state_families = Vec::new();
        for message in messages {
            let Some(state) = self.metadata_transfer.as_mut() else {
                break;
            };
            match message {
                WorkerMessage::Progress(progress) => state.progress = Some(progress),
                WorkerMessage::Preview(result) => {
                    state.rx = None;
                    state.stage = match result {
                        Ok(preview) => Stage::ImportConfirm(preview),
                        Err(error) => Stage::Result(ResultState::Import(Err(error))),
                    };
                }
                WorkerMessage::Export(result) => {
                    state.rx = None;
                    state.stage = Stage::Result(ResultState::Export(result));
                }
                WorkerMessage::Import(mut result) => {
                    if let Ok(summary) = &mut result
                        && summary.applied_entries > 0
                    {
                        imported_rating_keys = std::mem::take(&mut summary.applied_rating_keys);
                        imported_page_state_families =
                            std::mem::take(&mut summary.applied_page_state_families);
                    }
                    state.rx = None;
                    state.stage = Stage::Result(ResultState::Import(result));
                }
            }
        }
        if disconnected
            && self.metadata_transfer.as_ref().is_some_and(|state| {
                state.rx.is_some() && matches!(state.stage, Stage::LoadingPreview | Stage::Running)
            })
            && let Some(state) = self.metadata_transfer.as_mut()
        {
            state.rx = None;
            state.stage = match state.operation {
                Operation::Export => Stage::Result(ResultState::Export(Err(
                    "worker が予期せず終了しました".to_string(),
                ))),
                Operation::Import => Stage::Result(ResultState::Import(Err(
                    "worker が予期せず終了しました".to_string(),
                ))),
            };
        }
        if !imported_rating_keys.is_empty() {
            self.refresh_after_metadata_transfer_import(
                &imported_rating_keys,
                &imported_page_state_families,
            );
        }
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
                    "同じフォルダの {} に保存します。",
                    crate::metadata_transfer::SIDECAR_FILENAME
                ));
                ui.small("既存ファイルは、エクスポートが完了した時点で置き換えます。");
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
                ui.label("メタ情報ファイルを確認しています…");
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
                ui.small("このファイルに記載がない項目の既存メタ情報は保持します。");
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
                ui.label("実行中のメタ情報書き込みが完了するのを待っています…");
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
        return;
    };
    let label = match progress.phase {
        TransferPhase::Scanning => "ファイルを列挙中",
        TransferPhase::ReadingMetadata => "メタ情報を読み取り中",
        TransferPhase::WritingSidecar => "メタ情報ファイルを書き込み中",
        TransferPhase::ReadingSidecar => "メタ情報ファイルを確認中",
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
    if let Some(path) = &progress.current_path {
        ui.label(
            egui::RichText::new(path)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
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
    let (tx, rx) = mpsc::channel();
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

fn start_export_worker(state: &mut MetadataTransferState) {
    let root = state.root.clone();
    let recursive = state.recursive;
    let data_dir = crate::data_dir::get();
    let cancel = Arc::clone(&state.cancel);
    cancel.store(false, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    state.rx = Some(rx);
    state.stage = Stage::Running;
    let spawn = std::thread::Builder::new()
        .name("metadata-export".to_string())
        .spawn(move || {
            let mut progress_relay = ProgressRelay::new(tx.clone());
            let result = crate::metadata_transfer::export_at(
                &data_dir,
                &root,
                recursive,
                &cancel,
                move |value| {
                    progress_relay.send(value);
                },
            )
            .map_err(|error| error.to_string());
            let _ = tx.send(WorkerMessage::Export(result));
        });
    match spawn {
        Ok(handle) => state.handle = Some(handle),
        Err(error) => {
            state.rx = None;
            state.stage = Stage::Result(ResultState::Export(Err(error.to_string())));
        }
    }
}

fn start_import_worker(state: &mut MetadataTransferState) {
    if let Some(preview_handle) = state.handle.take() {
        let _ = preview_handle.join();
    }
    let root = state.root.clone();
    let data_dir = crate::data_dir::get();
    let cancel = Arc::clone(&state.cancel);
    cancel.store(false, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    state.rx = Some(rx);
    state.stage = Stage::Running;
    let spawn = std::thread::Builder::new()
        .name("metadata-import".to_string())
        .spawn(move || {
            let mut progress_relay = ProgressRelay::new(tx.clone());
            let result =
                crate::metadata_transfer::import_at(&data_dir, &root, &cancel, move |value| {
                    progress_relay.send(value);
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(WorkerMessage::Import(result));
        });
    match spawn {
        Ok(handle) => state.handle = Some(handle),
        Err(error) => {
            state.rx = None;
            state.stage = Stage::Result(ResultState::Import(Err(error.to_string())));
        }
    }
}
