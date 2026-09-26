//! EPUB conversion follows the archive dialog's scan/confirm/convert/error phases,
//! while keeping a separate completion owner: the published PDF is reopened by its EPUB path.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;

use crate::app::{
    App, FolderNavHistorySnapshot, OpenRequestOwner, PdfOpenFailure, PdfOpenFailureRoute,
};
use crate::epub_cache::PublishOutcome;
use crate::epub_convert::{
    self, CancelToken, ConvertProgress, EpubConvertError, EpubInspectSummary,
    PdfiumConvertedPdfVerifier,
};

/// Includes WebView2 startup and PDF verification on slower machines.
pub(crate) const EPUB_WORKER_TIMEOUT_SECS: u32 = 600;

pub(crate) enum EpubConvertPhase {
    Scanning,
    Confirm(EpubInspectSummary),
    Converting(Option<ConvertProgress>),
    Saving(Option<ConvertProgress>),
    Stale,
    Error(String),
    SaveError(String),
}

/// Abort restores the view left by this request. A later open already owns a superseded view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EpubConvertExit {
    Abort,
    Superseded,
}

/// View state to restore if a request ends before any successor owns the view.
pub(crate) struct EpubOpenRestore {
    pub(crate) logical: PathBuf,
    pub(crate) history: Option<FolderNavHistorySnapshot>,
}

enum EpubConvertMsg {
    InspectDone(Result<EpubInspectSummary, EpubConvertError>),
    Progress(ConvertProgress),
    ConvertDone(epub_convert::ConvertResult),
    SaveDone(Result<epub_convert::SavedPdf, EpubConvertError>),
}

pub(crate) struct EpubConvertState {
    pub(crate) src_path: PathBuf,
    pub(crate) owner: OpenRequestOwner,
    // Existing viewer-context surface identity and Smart staged-request identity. Item
    // refreshes do not supersede an open, but another top-level surface or staged Smart open does.
    pub(crate) surface_generation: u64,
    pub(crate) smart_transition_sequence: u64,
    pub(crate) open_restore: EpubOpenRestore,
    pub(crate) deferred_fullscreen: Option<crate::app::DeferredFsReopen>,
    pub(crate) phase: EpubConvertPhase,
    cancel: CancelToken,
    rx: Receiver<EpubConvertMsg>,
}

impl Drop for EpubConvertState {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
impl EpubConvertState {
    pub(crate) fn fake_published_sender_for_test(&mut self) -> impl FnOnce() + use<> {
        let (tx, rx) = mpsc::channel();
        self.rx = rx;
        self.phase = EpubConvertPhase::Converting(None);
        move || {
            let _ = tx.send(EpubConvertMsg::ConvertDone(Ok(PublishOutcome::Published)));
        }
    }

    pub(crate) fn completed_for_test(
        src_path: PathBuf,
        owner: OpenRequestOwner,
        outcome: PublishOutcome,
        surface_generation: u64,
        smart_transition_sequence: u64,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        tx.send(EpubConvertMsg::ConvertDone(Ok(outcome))).unwrap();
        let logical = src_path.clone();
        Self {
            src_path,
            owner,
            surface_generation,
            smart_transition_sequence,
            open_restore: EpubOpenRestore {
                logical,
                history: None,
            },
            deferred_fullscreen: None,
            phase: EpubConvertPhase::Converting(None),
            cancel: CancelToken::new().unwrap(),
            rx,
        }
    }
}

pub(crate) fn error_message(error: &EpubConvertError) -> String {
    match error {
        EpubConvertError::Drm => "この EPUB は保護されているため変換できません".into(),
        EpubConvertError::Invalid => "不正な EPUB のため開けません".into(),
        EpubConvertError::WebView2Missing => "WebView2 Runtime が見つかりません".into(),
        EpubConvertError::WebView2Unsupported => {
            "WebView2 Runtime が古いか、必要な機能がありません".into()
        }
        EpubConvertError::RenderFailed
        | EpubConvertError::Protocol
        | EpubConvertError::InvalidPdf => "EPUB の変換に失敗しました".into(),
        EpubConvertError::Timeout => "EPUB の変換がタイムアウトしました".into(),
        EpubConvertError::SourceBusy => "EPUB ファイルが使用中です".into(),
        EpubConvertError::SourceChanged => {
            "変換中に EPUB が変更されました。もう一度お試しください".into()
        }
        EpubConvertError::ExistingPdf => "同じ名前の PDF が既にあります".into(),
        EpubConvertError::Unavailable(reason) => format!("EPUB 変換が無効です: {reason}"),
        EpubConvertError::Cancelled => "EPUB の変換を取り消しました".into(),
        EpubConvertError::Io(_) | EpubConvertError::Cache(_) => {
            "EPUB の変換に失敗しました。ファイルと保存先を確認してください".into()
        }
    }
}

fn progress_label(progress: &ConvertProgress) -> String {
    match progress.phase {
        epub_convert::Phase::Parse => "本を確認中",
        epub_convert::Phase::Extract => "内容を準備中",
        epub_convert::Phase::Init => "変換を準備中",
        epub_convert::Phase::Print => {
            return progress
                .pages
                .map(|pages| format!("ページを変換中 ({pages} ページ)"))
                .unwrap_or_else(|| "ページを変換中".into());
        }
        epub_convert::Phase::Merge => "ページをまとめています",
        epub_convert::Phase::Verify => "変換結果を確認中",
    }
    .into()
}

fn progress_fraction(progress: &ConvertProgress) -> f32 {
    if progress.total == 0 {
        0.0
    } else {
        (progress.done as f32 / progress.total as f32).clamp(0.0, 1.0)
    }
}

fn summary_direction(direction: &str) -> &'static str {
    match direction {
        "rtl" => "右開き",
        "ltr" => "左開き",
        _ => "指定なし",
    }
}

fn save_pdf_button(ui: &mut egui::Ui, summary: &EpubInspectSummary) -> egui::Response {
    ui.add_enabled(
        !summary.sibling_pdf_exists,
        egui::Button::new("PDF ファイルとして保存して開く"),
    )
}

fn start_inspect(state: &mut EpubConvertState) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let path = state.src_path.clone();
    let cancel = state.cancel.clone();
    std::thread::Builder::new()
        .name("epub-inspect".into())
        .spawn(move || {
            let result = epub_convert::inspect(&path, &cancel, EPUB_WORKER_TIMEOUT_SECS);
            let _ = tx.send(EpubConvertMsg::InspectDone(result));
        })
        .map_err(|_| "EPUB の確認を開始できませんでした".to_owned())?;
    state.rx = rx;
    state.phase = EpubConvertPhase::Scanning;
    Ok(())
}

fn start_convert(state: &mut EpubConvertState) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let path = state.src_path.clone();
    let cancel = state.cancel.clone();
    std::thread::Builder::new()
        .name("epub-convert".into())
        .spawn(move || {
            let (progress_tx, progress_rx) = mpsc::channel();
            let forward_tx = tx.clone();
            let forward = std::thread::spawn(move || {
                while let Ok(progress) = progress_rx.recv() {
                    if forward_tx.send(EpubConvertMsg::Progress(progress)).is_err() {
                        break;
                    }
                }
            });
            let result = match crate::pdf_loader::epub_conversion_guard() {
                Ok(guard) => epub_convert::convert(
                    guard,
                    &path,
                    &cancel,
                    &progress_tx,
                    EPUB_WORKER_TIMEOUT_SECS,
                    &PdfiumConvertedPdfVerifier,
                ),
                Err(crate::pdf_loader::PdfReadError::EpubUnavailable { reason }) => {
                    Err(EpubConvertError::Unavailable(reason.to_string()))
                }
                Err(error) => Err(EpubConvertError::Unavailable(error.to_string())),
            };
            drop(progress_tx);
            let _ = forward.join();
            let _ = tx.send(EpubConvertMsg::ConvertDone(result));
        })
        .map_err(|_| "EPUB の変換を開始できませんでした".to_owned())?;
    state.rx = rx;
    state.phase = EpubConvertPhase::Converting(None);
    Ok(())
}

fn start_save(state: &mut EpubConvertState) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let path = state.src_path.clone();
    let cancel = state.cancel.clone();
    std::thread::Builder::new()
        .name("epub-save-pdf".into())
        .spawn(move || {
            let (progress_tx, progress_rx) = mpsc::channel();
            let forward_tx = tx.clone();
            let forward = std::thread::spawn(move || {
                while let Ok(progress) = progress_rx.recv() {
                    if forward_tx.send(EpubConvertMsg::Progress(progress)).is_err() {
                        break;
                    }
                }
            });
            let result = match crate::pdf_loader::epub_conversion_guard() {
                Ok(guard) => epub_convert::save_sibling(
                    guard,
                    &path,
                    &cancel,
                    &progress_tx,
                    EPUB_WORKER_TIMEOUT_SECS,
                    &PdfiumConvertedPdfVerifier,
                ),
                Err(error) => Err(EpubConvertError::Unavailable(error.to_string())),
            };
            drop(progress_tx);
            let _ = forward.join();
            let _ = tx.send(EpubConvertMsg::SaveDone(result));
        })
        .map_err(|_| "PDF の保存を開始できませんでした".to_owned())?;
    state.rx = rx;
    state.phase = EpubConvertPhase::Saving(None);
    Ok(())
}

impl App {
    fn restore_address_after_epub_open_aborted(&mut self, logical: &Path) {
        // A direct PDF-style open updates the address before asynchronous enumeration. On an
        // EPUB refusal/cancel the previous visible book is still installed.
        if self.address == logical.to_string_lossy() {
            self.address = self
                .effective_folder()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
    }

    /// Plain PDF failures remain with their established password and empty-book paths.
    pub(crate) fn route_pdf_open_failure(
        &mut self,
        owner: OpenRequestOwner,
        logical: &Path,
        failure: PdfOpenFailure,
    ) -> PdfOpenFailureRoute {
        match failure {
            PdfOpenFailure::NotConverted => {
                if self.settings.archive_file_handling_ignores_convertible() {
                    self.show_feedback_toast("設定により変換が必要な本を無視しています".into());
                    self.restore_address_after_epub_open_aborted(logical);
                    return PdfOpenFailureRoute::Handled;
                }
                let Ok(cancel) = CancelToken::new() else {
                    self.show_feedback_toast("EPUB の変換を開始できませんでした".into());
                    self.restore_address_after_epub_open_aborted(logical);
                    return PdfOpenFailureRoute::Handled;
                };
                let (_, rx) = mpsc::channel();
                let mut state = EpubConvertState {
                    src_path: logical.to_owned(),
                    owner,
                    surface_generation: self.top_level_grid_view.generation(),
                    smart_transition_sequence: self.smart_folder_transition_sequence,
                    open_restore: EpubOpenRestore {
                        logical: logical.to_owned(),
                        history: None,
                    },
                    deferred_fullscreen: None,
                    phase: EpubConvertPhase::Scanning,
                    cancel,
                    rx,
                };
                let started = if self.settings.archive_convert_suppresses_confirm() {
                    start_convert(&mut state)
                } else {
                    start_inspect(&mut state)
                };
                if let Err(message) = started {
                    state.phase = EpubConvertPhase::Error(message);
                }
                self.replace_epub_convert_state(state);
                PdfOpenFailureRoute::ConversionDialogOpened
            }
            PdfOpenFailure::EpubUnavailable(reason) => {
                self.show_feedback_toast(format!("EPUB 変換が無効です: {reason}"));
                self.restore_address_after_epub_open_aborted(logical);
                PdfOpenFailureRoute::Handled
            }
            PdfOpenFailure::PasswordRequired | PdfOpenFailure::Other(_) => {
                PdfOpenFailureRoute::Unhandled
            }
        }
    }

    pub(crate) fn epub_convert_dialog_visible(&self) -> bool {
        self.epub_convert.as_ref().is_some_and(|state| {
            !matches!(state.phase, EpubConvertPhase::Scanning)
                || !self.settings.archive_convert_suppresses_confirm()
        })
    }

    pub(crate) fn cancel_superseded_epub_convert(
        &mut self,
        _path: &Path,
        _owner: &OpenRequestOwner,
    ) {
        // Every accepted later open is a new request, including a reload of the same path.
        if self.epub_convert.is_some() {
            self.finish_epub_convert(EpubConvertExit::Superseded);
        }
    }

    pub(crate) fn restore_epub_open(&mut self, restore: EpubOpenRestore) {
        if let Some(snapshot) = restore.history {
            self.restore_folder_nav_history(snapshot);
        }
        self.restore_address_after_epub_open_aborted(&restore.logical);
    }

    pub(crate) fn finish_epub_convert(&mut self, exit: EpubConvertExit) -> Option<EpubOpenRestore> {
        let Some(mut state) = self.epub_convert.take() else {
            return None;
        };
        let had_deferred = state.deferred_fullscreen.take().is_some();
        let owner = state.owner.clone();
        let active_logical = state.src_path.clone();
        let restore = std::mem::replace(
            &mut state.open_restore,
            EpubOpenRestore {
                logical: state.src_path.clone(),
                history: None,
            },
        );
        drop(state);
        self.abort_smart_archive_open_for_owner(&owner);
        if had_deferred {
            if exit == EpubConvertExit::Abort {
                self.finish_visible_container_fs_nav_failed();
            }
            // Archive conversion uses this same terminal path. No replacement conversion or
            // pane scan explicitly takes over the old fullscreen navigation lock.
            self.release_fs_nav_lock();
        }
        match exit {
            EpubConvertExit::Abort => {
                // A replacement may have updated the address to its own EPUB path while
                // retaining the first request's rollback snapshot.
                self.restore_address_after_epub_open_aborted(&active_logical);
                self.restore_epub_open(restore);
                None
            }
            EpubConvertExit::Superseded => Some(restore),
        }
    }

    fn replace_epub_convert_state(&mut self, mut state: EpubConvertState) {
        if let Some(restore) = self.finish_epub_convert(EpubConvertExit::Superseded) {
            state.open_restore = restore;
        }
        self.epub_convert = Some(state);
    }

    fn finish_epub_conversion_open(
        &mut self,
        mut state: EpubConvertState,
        path: PathBuf,
        saved_sibling: bool,
        user_data_errors: Vec<String>,
    ) {
        let mut owner = state.owner.clone();
        if !self.epub_conversion_owner_is_current(&state) {
            self.epub_convert = Some(state);
            self.finish_epub_convert(EpubConvertExit::Superseded);
            return;
        }
        let restore = std::mem::replace(
            &mut state.open_restore,
            EpubOpenRestore {
                logical: state.src_path.clone(),
                history: None,
            },
        );
        let deferred = state.deferred_fullscreen.take();
        let source = state.src_path.clone();
        drop(state);
        if saved_sibling && let OpenRequestOwner::CollectionGridPhysical(collection) = &mut owner {
            collection.target_path = path.clone();
        }
        if !user_data_errors.is_empty() {
            self.show_feedback_toast(format!(
                "PDF は保存されましたが、一部の設定を引き継げませんでした: {}",
                user_data_errors.join("; ")
            ));
        }
        if matches!(
            &owner,
            OpenRequestOwner::MainGridArchive(intent)
                if matches!(intent.smart_folder_owner, crate::app::SmartGridArchiveOwner::Transition(_))
        ) {
            if saved_sibling {
                let _ = self.supply_smart_saved_epub_pdf(&source, &path, &owner);
            } else {
                let _ = self.supply_smart_epub_conversion(&path, &owner);
            }
            if deferred.is_some() {
                self.release_fs_nav_lock();
            }
            return;
        }
        let reopened = if matches!(owner, OpenRequestOwner::CollectionGridPhysical(_)) {
            self.load_folder_with_scan_owned(path.clone(), None, owner.clone())
        } else {
            self.load_pdf_as_folder_owned(path.clone(), owner.clone());
            true
        };
        if reopened
            && let Some(pending) = self.pdf_enumerate_pending.as_mut()
            && crate::folder_tree::path_eq(&pending.0, &path)
            && pending.3 == owner
        {
            pending.4 = restore.history;
            if deferred.is_some() {
                self.fs_nav_after_pdf_enumerate = deferred;
            }
        } else if deferred.is_some() {
            self.release_fs_nav_lock();
        }
    }

    pub(crate) fn show_epub_convert_dialog(&mut self, ctx: &egui::Context) {
        if self
            .epub_convert
            .as_ref()
            .is_some_and(|state| !self.epub_conversion_owner_is_current(state))
        {
            self.finish_epub_convert(EpubConvertExit::Superseded);
            return;
        }
        let Some(mut state) = self.epub_convert.take() else {
            return;
        };
        loop {
            match state.rx.try_recv() {
                Ok(EpubConvertMsg::InspectDone(Ok(summary))) => {
                    state.phase = EpubConvertPhase::Confirm(summary);
                }
                Ok(EpubConvertMsg::InspectDone(Err(error))) => {
                    state.phase = EpubConvertPhase::Error(error_message(&error));
                }
                Ok(EpubConvertMsg::Progress(progress)) => {
                    state.phase = match state.phase {
                        EpubConvertPhase::Saving(_) => EpubConvertPhase::Saving(Some(progress)),
                        _ => EpubConvertPhase::Converting(Some(progress)),
                    };
                }
                Ok(EpubConvertMsg::ConvertDone(Ok(PublishOutcome::Stale))) => {
                    state.phase = EpubConvertPhase::Stale;
                }
                Ok(EpubConvertMsg::ConvertDone(Ok(
                    PublishOutcome::Published | PublishOutcome::Adopted(_),
                ))) => {
                    let path = state.src_path.clone();
                    self.finish_epub_conversion_open(state, path, false, Vec::new());
                    return;
                }
                Ok(EpubConvertMsg::SaveDone(Ok(saved))) => {
                    self.finish_epub_conversion_open(
                        state,
                        saved.path,
                        true,
                        saved.user_data_errors,
                    );
                    return;
                }
                Ok(EpubConvertMsg::SaveDone(Err(error))) => {
                    state.phase = EpubConvertPhase::SaveError(format!(
                        "PDF を保存できませんでした: {}。『変換して開く』をお試しください",
                        error_message(&error)
                    ));
                }
                Ok(EpubConvertMsg::ConvertDone(Err(error))) => {
                    state.phase = EpubConvertPhase::Error(error_message(&error));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if matches!(
                        state.phase,
                        EpubConvertPhase::Scanning
                            | EpubConvertPhase::Converting(_)
                            | EpubConvertPhase::Saving(_)
                    ) {
                        state.phase = EpubConvertPhase::Error("EPUB の処理が中断されました".into());
                    }
                    break;
                }
            }
        }

        let mut close = false;
        let mut convert = false;
        let mut save = false;
        if !matches!(state.phase, EpubConvertPhase::Scanning)
            || !self.settings.archive_convert_suppresses_confirm()
        {
            let escape = self.dialog_escape_pressed(ctx);
            let enter = self.dialog_enter_pressed(ctx);
            let mut open = true;
            let name = state
                .src_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("EPUB");
            egui::Window::new("EPUB を PDF に変換")
                .id(egui::Id::new("epub_convert_dialog"))
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(400.0);
                    ui.label(name);
                    match &state.phase {
                        EpubConvertPhase::Scanning => { ui.label("本の内容を確認しています..."); }
                        EpubConvertPhase::Confirm(summary) => {
                            let layout = match summary.layout.as_str() {
                                "fixed" => "固定レイアウト",
                                "mixed" => "固定とリフローの混在",
                                _ => "リフロー",
                            };
                            let direction = summary_direction(&summary.direction);
                            ui.label(format!("{layout} / {direction} / 本文 {} 項目 / 保護なし", summary.spine_count));
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.button("変換して開く").clicked() || enter { convert = true; }
                                let save_button = save_pdf_button(ui, summary);
                                if save_button.clicked() { save = true; }
                                if ui.button("キャンセル").clicked() { close = true; }
                            });
                            if summary.sibling_pdf_exists {
                                ui.weak("同じ名前の PDF があるため保存できません");
                            }
                        }
                        EpubConvertPhase::Converting(progress) => {
                            if let Some(progress) = progress {
                                ui.add(egui::ProgressBar::new(progress_fraction(progress)).text(progress_label(progress)));
                            } else { ui.label("変換を準備しています..."); }
                            if ui.button("キャンセル").clicked() { close = true; }
                        }
                        EpubConvertPhase::Saving(progress) => {
                            if let Some(progress) = progress {
                                ui.add(egui::ProgressBar::new(progress_fraction(progress)).text(progress_label(progress)));
                            } else { ui.label("PDF の保存を準備しています..."); }
                            if ui.button("キャンセル").clicked() { close = true; }
                        }
                        EpubConvertPhase::Stale => {
                            ui.label("変換中に EPUB ファイルが変更されました。もう一度変換してください。");
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.button("再試行").clicked() { convert = true; }
                                if ui.button("閉じる").clicked() { close = true; }
                            });
                        }
                        EpubConvertPhase::Error(message) => {
                            ui.colored_label(ui.visuals().error_fg_color, message);
                            if ui.button("閉じる").clicked() { close = true; }
                        }
                        EpubConvertPhase::SaveError(message) => {
                            ui.colored_label(ui.visuals().error_fg_color, message);
                            ui.horizontal(|ui| {
                                if ui.button("変換して開く").clicked() { convert = true; }
                                if ui.button("閉じる").clicked() { close = true; }
                            });
                        }
                    }
                });
            close |= !open || escape;
        }
        if convert && let Err(message) = start_convert(&mut state) {
            state.phase = EpubConvertPhase::Error(message);
        }
        if save && let Err(message) = start_save(&mut state) {
            state.phase = EpubConvertPhase::SaveError(message);
        }
        if close {
            self.epub_convert = Some(state);
            self.finish_epub_convert(EpubConvertExit::Abort);
        } else {
            if matches!(
                state.phase,
                EpubConvertPhase::Scanning
                    | EpubConvertPhase::Converting(_)
                    | EpubConvertPhase::Saving(_)
            ) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            self.epub_convert = Some(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_state(
        phase: EpubConvertPhase,
    ) -> (EpubConvertState, mpsc::Sender<EpubConvertMsg>, CancelToken) {
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new().unwrap();
        (
            EpubConvertState {
                src_path: PathBuf::from("C:/books/book.epub"),
                owner: OpenRequestOwner::Navigation,
                surface_generation: 0,
                smart_transition_sequence: 0,
                open_restore: EpubOpenRestore {
                    logical: PathBuf::from("C:/books/book.epub"),
                    history: None,
                },
                deferred_fullscreen: None,
                phase,
                cancel: cancel.clone(),
                rx,
            },
            tx,
            cancel,
        )
    }

    #[test]
    fn fake_worker_drives_epub_dialog_progress_stale_and_inspect_error() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (state, tx, cancel) = fake_state(EpubConvertPhase::Converting(None));
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::Progress(ConvertProgress {
            phase: epub_convert::Phase::Print,
            done: 2,
            total: 4,
            pages: Some(12),
        }))
        .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(matches!(
            app.epub_convert.as_ref().map(|state| &state.phase),
            Some(EpubConvertPhase::Converting(Some(progress))) if progress.done == 2
        ));
        tx.send(EpubConvertMsg::ConvertDone(Ok(PublishOutcome::Stale)))
            .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(matches!(
            app.epub_convert.as_ref().map(|state| &state.phase),
            Some(EpubConvertPhase::Stale)
        ));
        app.epub_convert = None;
        assert!(cancel.is_cancelled());

        let (state, tx, _) = fake_state(EpubConvertPhase::Scanning);
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::InspectDone(Err(EpubConvertError::Drm)))
            .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(matches!(
            app.epub_convert.as_ref().map(|state| &state.phase),
            Some(EpubConvertPhase::Error(message)) if message.contains("保護")
        ));
    }

    #[test]
    fn epub_inspect_default_direction_dialog_says_unspecified() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (state, tx, _) = fake_state(EpubConvertPhase::Scanning);
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::InspectDone(Ok(EpubInspectSummary {
            layout: "fixed".into(),
            direction: "default".into(),
            spine_count: 7,
            sibling_pdf_exists: false,
        })))
        .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        let output = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        let mut text = String::new();
        let mut stack: Vec<&egui::epaint::Shape> =
            output.shapes.iter().map(|shape| &shape.shape).collect();
        while let Some(shape) = stack.pop() {
            match shape {
                egui::epaint::Shape::Text(shape) => text.push_str(shape.galley.text()),
                egui::epaint::Shape::Vec(shapes) => stack.extend(shapes.iter()),
                _ => {}
            }
        }
        assert!(text.contains("指定なし"), "{text}");
        assert!(!text.contains("左開き"), "{text}");
    }

    #[test]
    fn epub_convert_dialog_print_progress_shows_page_count() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (state, tx, _) = fake_state(EpubConvertPhase::Converting(None));
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::Progress(ConvertProgress {
            phase: epub_convert::Phase::Print,
            done: 2,
            total: 4,
            pages: Some(17),
        }))
        .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        let Some(EpubConvertPhase::Converting(Some(progress))) =
            app.epub_convert.as_ref().map(|state| &state.phase)
        else {
            panic!("progress was not adopted by the dialog");
        };
        assert_eq!(
            (progress.done, progress.total, progress.pages),
            (2, 4, Some(17))
        );
        assert_eq!(progress_fraction(progress), 0.5);
        let output = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        let mut text = String::new();
        let mut stack: Vec<&egui::epaint::Shape> =
            output.shapes.iter().map(|shape| &shape.shape).collect();
        while let Some(shape) = stack.pop() {
            match shape {
                egui::epaint::Shape::Text(shape) => text.push_str(shape.galley.text()),
                egui::epaint::Shape::Vec(shapes) => stack.extend(shapes.iter()),
                _ => {}
            }
        }
        assert!(text.contains("ページを変換中 (17 ページ)"), "{text}");
    }

    #[test]
    fn fake_worker_publication_reopens_logical_epub_with_same_owner() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (mut state, tx, _) = fake_state(EpubConvertPhase::Converting(None));
        let source = state.src_path.clone();
        let owner = state.owner.clone();
        state.deferred_fullscreen = Some(crate::app::DeferredFsReopen {
            history_trigger: crate::app::HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: crate::app::DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: false,
            preserve_after_password_prompt: false,
        });
        app.fs_nav_locked_gen = Some(7);
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::ConvertDone(Ok(PublishOutcome::Published)))
            .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(app.epub_convert.is_none());
        assert!(
            app.pdf_enumerate_pending
                .as_ref()
                .is_some_and(|pending| { pending.0 == source && pending.3 == owner })
        );
        assert!(app.fs_nav_after_pdf_enumerate.is_some());
        assert_eq!(app.fs_nav_locked_gen, Some(7));
    }

    #[test]
    fn sibling_save_result_opens_pdf_with_original_owner_and_fullscreen_handover() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (mut state, tx, _) = fake_state(EpubConvertPhase::Saving(None));
        let pdf = state.src_path.with_extension("pdf");
        let owner = state.owner.clone();
        state.deferred_fullscreen = Some(crate::app::DeferredFsReopen {
            history_trigger: crate::app::HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: crate::app::DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: false,
            preserve_after_password_prompt: false,
        });
        app.fs_nav_locked_gen = Some(7);
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::SaveDone(Ok(epub_convert::SavedPdf {
            path: pdf.clone(),
            reused_cache: false,
            user_data_errors: Vec::new(),
        })))
        .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(app.epub_convert.is_none());
        assert!(
            app.pdf_enumerate_pending
                .as_ref()
                .is_some_and(|pending| pending.0 == pdf && pending.3 == owner)
        );
        assert!(app.fs_nav_after_pdf_enumerate.is_some());
    }

    #[test]
    fn inspect_same_name_pdf_shows_disabled_save_reason() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (state, tx, _) = fake_state(EpubConvertPhase::Scanning);
        app.epub_convert = Some(state);
        let summary = EpubInspectSummary {
            layout: "fixed".into(),
            direction: "ltr".into(),
            spine_count: 2,
            sibling_pdf_exists: true,
        };
        tx.send(EpubConvertMsg::InspectDone(Ok(summary.clone())))
            .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        let output = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(
            matches!(app.epub_convert.as_ref().map(|state| &state.phase), Some(EpubConvertPhase::Confirm(summary)) if summary.sibling_pdf_exists)
        );
        let mut text = String::new();
        let mut stack: Vec<&egui::epaint::Shape> =
            output.shapes.iter().map(|shape| &shape.shape).collect();
        while let Some(shape) = stack.pop() {
            match shape {
                egui::epaint::Shape::Text(shape) => text.push_str(shape.galley.text()),
                egui::epaint::Shape::Vec(shapes) => stack.extend(shapes.iter()),
                _ => {}
            }
        }
        assert!(
            text.contains("同じ名前の PDF があるため保存できません"),
            "{text}"
        );
        let mut enabled = true;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                enabled = save_pdf_button(ui, &summary).enabled();
            });
        });
        assert!(!enabled);
        let mut available = summary;
        available.sibling_pdf_exists = false;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                enabled = save_pdf_button(ui, &available).enabled();
            });
        });
        assert!(enabled);
    }

    #[test]
    fn save_failure_explains_cache_fallback() {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let (state, tx, _) = fake_state(EpubConvertPhase::Saving(None));
        app.epub_convert = Some(state);
        tx.send(EpubConvertMsg::SaveDone(Err(EpubConvertError::Io(
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "read-only"),
        ))))
        .unwrap();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert!(
            matches!(app.epub_convert.as_ref().map(|state| &state.phase), Some(EpubConvertPhase::SaveError(message)) if message.contains("変換して開く"))
        );
    }

    #[test]
    fn epub_error_messages_cover_typed_failures() {
        let errors = [
            (EpubConvertError::Drm, "保護"),
            (EpubConvertError::Invalid, "不正な EPUB"),
            (EpubConvertError::WebView2Missing, "見つかりません"),
            (EpubConvertError::WebView2Unsupported, "古い"),
            (EpubConvertError::RenderFailed, "変換に失敗"),
            (EpubConvertError::Timeout, "タイムアウト"),
            (EpubConvertError::SourceBusy, "使用中"),
            (
                EpubConvertError::Unavailable("起動時の確認に失敗".into()),
                "無効",
            ),
            (EpubConvertError::Cancelled, "取り消し"),
            (EpubConvertError::Protocol, "変換に失敗"),
            (EpubConvertError::InvalidPdf, "変換に失敗"),
            (
                EpubConvertError::Io(std::io::Error::other("test")),
                "変換に失敗",
            ),
            (
                EpubConvertError::Cache(crate::epub_cache::CacheError::InvalidState("test")),
                "変換に失敗",
            ),
        ];
        for (error, expected) in errors {
            assert!(error_message(&error).contains(expected), "{error:?}");
        }
    }
}
