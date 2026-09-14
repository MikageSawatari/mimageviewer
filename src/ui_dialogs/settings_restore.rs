//! 設定の復元ダイアログ (`設定` メニュー → `設定の復元…`)。
//!
//! `<data_dir>/settings.db` と世代バックアップ (`settings.db.bak1` ..
//! `settings.db.bak10`) を一覧化し、選択した世代の内容で本体を入れ替える。
//! 完全リセット (= 家族全削除) もここから実行する。どちらの操作も成功直後に
//! アプリを終了する (= 復元後の in-memory `Settings` 経由 save で `settings.db`
//! を踏み潰す事故を防ぐ)。
//!
//! バックエンドは [`crate::settings_restore`]、UI 状態は
//! [`SettingsRestoreState`]。

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;

use crate::app::App;
use crate::settings_restore::{
    self, BackupCompatibility, BackupSource, BackupSummary, ResetReport, RestoreReport,
};
use crate::ui_helpers::format_bytes;

const OPERATION_MODAL_SIZE: egui::Vec2 = egui::vec2(720.0, 540.0);
const OPERATION_MODAL_MIN_SIZE: egui::Vec2 = egui::vec2(560.0, 420.0);
const OPERATION_MODAL_VIEWPORT_MARGIN: egui::Vec2 = egui::vec2(32.0, 32.0);
const OPERATION_MODAL_HEADER_RESERVE: f32 = 58.0;
const OPERATION_MODAL_FOOTER_RESERVE: f32 = 42.0;

/// ダイアログの状態。Default で「未起動」。`App::open_settings_restore` で
/// 一覧をロードし、`show_settings_restore_dialog` で描画する。

pub(crate) struct SettingsRestoreState {
    pub(crate) backups: Vec<BackupSummary>,
    pub(crate) pending: Option<PendingAction>,
    pub(crate) result: Option<ActionResult>,
    tab: SettingsRestoreTab,
    operation_modal: Option<OperationModal>,
    operation_message: Option<(bool, String)>,
    operation_task_rx: Option<std::sync::mpsc::Receiver<OperationTaskResult>>,
    family_operation: SettingsFamilyOperationState,
}

impl Default for SettingsRestoreState {
    fn default() -> Self {
        Self {
            backups: Vec::new(),
            pending: None,
            result: None,
            tab: SettingsRestoreTab::Restore,
            operation_modal: None,
            operation_message: None,
            operation_task_rx: None,
            family_operation: SettingsFamilyOperationState::Idle,
        }
    }
}

enum SettingsFamilyOperationState {
    Idle,
    Running {
        action: PendingAction,
        task: settings_restore::SettingsFamilyOperationTask,
        deferred_close: Option<DeferredRootClose>,
    },
    RemoteUnavailable {
        retry: settings_restore::RemoteFavoritesRetry,
        error: String,
        retry_task: Option<UiWorkerTask<Result<(), String>>>,
        deferred_close: Option<DeferredRootClose>,
    },
}

impl SettingsFamilyOperationState {
    fn blocks_close(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    fn defer_root_close(&mut self, requested: DeferredRootClose) -> bool {
        let deferred_close = match self {
            Self::Running { deferred_close, .. }
            | Self::RemoteUnavailable {
                retry_task: Some(_),
                deferred_close,
                ..
            } => deferred_close,
            Self::Idle
            | Self::RemoteUnavailable {
                retry_task: None, ..
            } => return false,
        };
        *deferred_close = Some(deferred_close.unwrap_or(requested).merge(requested));
        true
    }

    fn needs_hidden_root_wake(&self) -> bool {
        matches!(
            self,
            Self::Running {
                deferred_close: Some(_),
                ..
            } | Self::RemoteUnavailable {
                retry_task: Some(_),
                deferred_close: Some(_),
                ..
            }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeferredRootClose {
    WindowClose,
    ApplicationQuit,
}

impl DeferredRootClose {
    fn merge(self, newer: Self) -> Self {
        if matches!(self, Self::ApplicationQuit) || matches!(newer, Self::ApplicationQuit) {
            Self::ApplicationQuit
        } else {
            Self::WindowClose
        }
    }
}

fn settings_family_deferred_root_close(
    application_quit: bool,
    plain_close_hides_to_tray: bool,
) -> Option<DeferredRootClose> {
    if application_quit {
        Some(DeferredRootClose::ApplicationQuit)
    } else if plain_close_hides_to_tray {
        None
    } else {
        Some(DeferredRootClose::WindowClose)
    }
}

struct UiWorkerTask<T> {
    receiver: std::sync::mpsc::Receiver<T>,
    handle: Option<std::thread::JoinHandle<()>>,
}

enum UiWorkerPoll<T> {
    Running,
    Completed(T),
    Failed(String),
}

impl<T> UiWorkerTask<T> {
    fn poll_finished(&mut self) -> UiWorkerPoll<T> {
        let Some(handle) = self.handle.as_ref() else {
            return UiWorkerPoll::Failed("worker の所有権が失われました".to_owned());
        };
        if !handle.is_finished() {
            return UiWorkerPoll::Running;
        }
        let handle = self.handle.take().expect("checked above");
        if handle.join().is_err() {
            return UiWorkerPoll::Failed("worker が予期せず終了しました".to_owned());
        }
        match self.receiver.try_recv() {
            Ok(result) => UiWorkerPoll::Completed(result),
            Err(error) => {
                UiWorkerPoll::Failed(format!("worker の結果を確認できませんでした: {error}"))
            }
        }
    }

    fn finish_for_exit(mut self) -> Result<T, String> {
        let Some(handle) = self.handle.take() else {
            return Err("worker の所有権が失われました".to_owned());
        };
        handle
            .join()
            .map_err(|_| "worker が終了処理中に予期せず終了しました".to_owned())?;
        self.receiver
            .recv()
            .map_err(|error| format!("worker の終了結果を確認できませんでした: {error}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsRestoreTab {
    Restore,
    OperationCustomize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffCompareTarget {
    Standard,
    Current,
    Previous,
}

enum OperationTaskResult {
    LoadedRow {
        source: BackupSource,
        action: OperationRowAction,
        result: Result<crate::operation_customize_share::ParsedImport, String>,
    },
    ImportedFile {
        result: Result<crate::operation_customize_share::ParsedImport, String>,
        fallback_label: String,
    },
    LoadedPrevious {
        source: BackupSource,
        result: Result<
            (
                String,
                crate::operation_customize_share::OperationCustomizeBundle,
            ),
            String,
        >,
    },
    Exported {
        result: Result<String, String>,
    },
    PreparedApply {
        kind: OperationApplyKind,
        bundle: crate::operation_customize_share::OperationCustomizeBundle,
        result: Result<PathBuf, String>,
    },
}

#[derive(Clone)]
enum OperationApplyKind {
    Import { source_label: String },
    ResetDefaults,
}

fn operation_apply_success_message(kind: &OperationApplyKind, backup_name: &str) -> String {
    match kind {
        OperationApplyKind::Import { source_label } => format!(
            "{source_label} を取り込みました。取り込み前の設定は {backup_name} に退避しました。"
        ),
        OperationApplyKind::ResetDefaults => format!(
            "操作カスタマイズを初期値に戻しました。元の設定は {backup_name} に退避しました。"
        ),
    }
}

fn operation_apply_failure_message(kind: &OperationApplyKind, error: &str) -> String {
    match kind {
        OperationApplyKind::Import { .. } => {
            format!("取り込み前の自動退避に失敗したため、取り込みを中止しました: {error}")
        }
        OperationApplyKind::ResetDefaults => {
            format!("初期値へ戻す前の自動退避に失敗したため、リセットを中止しました: {error}")
        }
    }
}

enum OperationModal {
    Export {
        source_label: String,
        bundle: crate::operation_customize_share::OperationCustomizeBundle,
        label: String,
    },
    Diff {
        source_label: String,
        source: BackupSource,
        bundle: crate::operation_customize_share::OperationCustomizeBundle,
        current: crate::operation_customize_share::OperationCustomizeBundle,
        target: DiffCompareTarget,
        previous: Option<
            Result<
                (
                    String,
                    crate::operation_customize_share::OperationCustomizeBundle,
                ),
                String,
            >,
        >,
    },
    Import {
        source_label: String,
        bundle: crate::operation_customize_share::OperationCustomizeBundle,
        warnings: Vec<String>,
        ignored_items: usize,
    },
    ResetConfirm,
}

#[derive(Clone)]
pub(crate) enum PendingAction {
    Restore(BackupSource),
    FullReset,
}

pub(crate) enum ActionResult {
    RestoreOk {
        source: BackupSource,
        report: RestoreReport,
    },
    ResetOk {
        report: ResetReport,
    },
    /// 何もファイルを触っていない or 完全 rollback 済みの失敗。
    /// アプリは続行可。ダイアログを閉じればユーザーは普通に使える。
    /// 2026-05-17 Codex P2 対応で `Failed` を 2 つに分割。
    FailedRecoverable {
        action_label: String,
        error: String,
    },
    /// `set_global_db(None)` 後の失敗。SQLite ハンドルが落ちている + SAVE_SUPPRESSED
    /// が立ったままで、続けて操作すると挙動が崩れる。アプリを **強制終了** + 再起動を
    /// 促す。
    FailedTerminal {
        action_label: String,
        error: String,
    },
}

impl App {
    pub(crate) fn settings_family_deferred_close_needs_hidden_wake(&self) -> bool {
        self.settings_restore_state
            .family_operation
            .needs_hidden_root_wake()
    }

    /// Keep a destructive settings-family worker owned by the live App when a root-window close
    /// would otherwise end the process. A plain close handled by the existing tray-hide path
    /// remains a hide; process-exit close is replayed after the worker reaches terminal.
    pub(crate) fn defer_settings_family_root_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        let application_quit = self.sidecar_restore_root_lifecycle_close_requested();
        let plain_close_hides_to_tray = !application_quit
            && self.settings.minimize_to_tray_on_close
            && self.tray_controller.is_some();
        let Some(requested) =
            settings_family_deferred_root_close(application_quit, plain_close_hides_to_tray)
        else {
            return;
        };
        if self
            .settings_restore_state
            .family_operation
            .defer_root_close(requested)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint();
        }
    }

    /// Final process-exit fence for the destructive settings-family worker and its retry task.
    /// This is the only blocking App-side join; normal UI polls call `is_finished` first.
    pub(crate) fn resolve_settings_family_operation_for_exit(&mut self) {
        let state = std::mem::replace(
            &mut self.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Idle,
        );
        match state {
            SettingsFamilyOperationState::Running { action, task, .. } => {
                crate::logger::log(format!(
                    "[settings_restore] draining action worker for exit: {}",
                    describe_pending(&action)
                ));
                match task.finish_for_exit() {
                    Ok(completed) => crate::logger::log(format!(
                        "[settings_restore] exit drain completed: {} reader_release={:?} mutation={:?}",
                        describe_pending(&action),
                        completed.reader_release,
                        completed.mutation,
                    )),
                    Err(error) => crate::logger::log(format!(
                        "[settings_restore] exit drain failed: {}: {error}",
                        describe_pending(&action)
                    )),
                }
            }
            SettingsFamilyOperationState::RemoteUnavailable {
                retry_task: Some(task),
                ..
            } => match task.finish_for_exit() {
                Ok(Ok(())) => {
                    crate::logger::log("[settings_restore] remote reader retry drained for exit")
                }
                Ok(Err(error)) | Err(error) => crate::logger::log(format!(
                    "[settings_restore] remote reader retry exit drain failed: {error}"
                )),
            },
            SettingsFamilyOperationState::Idle
            | SettingsFamilyOperationState::RemoteUnavailable {
                retry_task: None, ..
            } => {}
        }
    }

    /// メニューから呼ぶ起動エントリ。
    pub(crate) fn open_settings_restore_dialog(&mut self) {
        let dir = crate::data_dir::get();
        let backups = settings_restore::list_backups(&dir);
        self.settings_restore_state = SettingsRestoreState {
            backups,
            ..SettingsRestoreState::default()
        };
        self.show_settings_restore = true;
    }

    pub(crate) fn show_settings_restore_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_restore {
            return;
        }

        self.poll_operation_share_task(ctx);
        self.poll_settings_family_operation(ctx);

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        // 確認ダイアログ / 結果ダイアログを出している間は escape を「メイン閉じる」
        // に使わない (= 子側のキャンセル扱いにしたい)。結果ダイアログは `egui::Modal`
        // で背景入力をブロックするので、親側の [x] も内部的にクリックされない
        // (2026-05-17 Codex P2 round 2 対応)。
        let confirm_open = self.settings_restore_state.pending.is_some();
        let result_open = self.settings_restore_state.result.is_some();
        let operation_modal_open = self.settings_restore_state.operation_modal.is_some();
        let family_operation_open = self.settings_restore_state.family_operation.blocks_close();

        egui::Window::new("設定の復元")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_width(860.0)
            .show(ctx, |ui| {
                draw_body(self, ui);
            });

        if (!open || (escape_pressed && !confirm_open && !result_open && !operation_modal_open))
            && !family_operation_open
        {
            self.show_settings_restore = false;
            self.settings_restore_state = SettingsRestoreState::default();
            if self.settings_boot_problem_source.is_some() {
                self.show_settings_boot_problem_notice = true;
            }
        }

        // 確認 / 完了の各ダイアログ。結果ダイアログは `egui::Modal` で背景全部を
        // ブロックするので、Terminal でも Recoverable でも背景 UI 操作は不可能。
        self.show_settings_restore_confirm_dialog(ctx);
        self.show_settings_family_running_dialog(ctx);
        self.show_settings_restore_result_dialog(ctx);
        self.show_operation_share_modal(ctx);
    }

    fn show_settings_restore_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.settings_restore_state.pending.clone() else {
            return;
        };
        let mut confirm_open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut cancel = false;
        let mut execute = false;

        let (title, body_top, body_warning, action_label) = match &pending {
            PendingAction::Restore(source) => (
                "設定を復元",
                format!("「{}」の内容で現在の設定を上書きします。", source.label()),
                "現在の設定一式は別名でバックアップしてから書き換えます。\n\
                 復元完了後、アプリを自動で終了します。次回起動時に内容が反映されます。",
                "復元して終了",
            ),
            PendingAction::FullReset => (
                "設定を完全リセット",
                "settings.db / bak1〜bak10 を含む設定ファイル一式を削除します。".to_string(),
                "現在の設定一式は別名でバックアップしてから削除します。\n\
                 完了後、アプリを自動で終了します。次回起動時は初期状態になります。",
                "リセットして終了",
            ),
        };

        egui::Window::new(title)
            .open(&mut confirm_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(body_top);
                ui.add_space(4.0);
                for line in body_warning.lines() {
                    ui.label(line);
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(action_label).clicked() {
                        execute = true;
                    }
                    if ui.button("キャンセル").clicked() || escape_pressed {
                        cancel = true;
                    }
                });
            });

        if !confirm_open {
            cancel = true;
        }

        if cancel {
            self.settings_restore_state.pending = None;
        } else if execute {
            self.execute_pending_settings_restore(pending, ctx);
        }
    }

    fn execute_pending_settings_restore(&mut self, pending: PendingAction, ctx: &egui::Context) {
        let dir = crate::data_dir::get();
        let action = match &pending {
            PendingAction::Restore(source) => {
                settings_restore::SettingsFamilyAction::Restore(source.clone())
            }
            PendingAction::FullReset => settings_restore::SettingsFamilyAction::FullReset,
        };
        self.settings_restore_state.pending = None;
        let repaint_ctx = ctx.clone();
        match settings_restore::start_settings_family_operation(
            dir,
            action,
            self.remote_settings_reader_control(),
            std::sync::Arc::new(move || repaint_ctx.request_repaint()),
        ) {
            Ok(task) => {
                crate::logger::log(format!(
                    "[settings_restore] action worker started: {}",
                    describe_pending(&pending)
                ));
                self.settings_restore_state.family_operation =
                    SettingsFamilyOperationState::Running {
                        action: pending,
                        task,
                        deferred_close: None,
                    };
            }
            Err(error) => {
                self.settings_restore_state.result = Some(ActionResult::FailedRecoverable {
                    action_label: describe_pending(&pending).to_owned(),
                    error,
                });
            }
        }
    }

    fn poll_settings_family_operation(&mut self, ctx: &egui::Context) {
        let mut deferred_close = None;
        let state = std::mem::replace(
            &mut self.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Idle,
        );
        self.settings_restore_state.family_operation = match state {
            SettingsFamilyOperationState::Idle => SettingsFamilyOperationState::Idle,
            SettingsFamilyOperationState::Running {
                action,
                mut task,
                deferred_close: close_intent,
            } => match task.poll_finished() {
                settings_restore::SettingsFamilyOperationPoll::Running => {
                    SettingsFamilyOperationState::Running {
                        action,
                        task,
                        deferred_close: close_intent,
                    }
                }
                settings_restore::SettingsFamilyOperationPoll::Completed(completed) => {
                    deferred_close = close_intent;
                    let action_label = describe_pending(&action).to_owned();
                    let result = match completed.outcome {
                        Ok(settings_restore::SettingsFamilySuccess::Restore { source, report }) => {
                            ActionResult::RestoreOk { source, report }
                        }
                        Ok(settings_restore::SettingsFamilySuccess::FullReset(report)) => {
                            ActionResult::ResetOk { report }
                        }
                        Err(failure) => failure_to_action_result(failure, action_label),
                    };
                    crate::logger::log(format!(
                        "[settings_restore] action executed: {} -> {} reader_release={:?} mutation={:?}",
                        describe_pending(&action),
                        describe_result(&result),
                        completed.reader_release,
                        completed.mutation,
                    ));
                    self.settings_restore_state.result = Some(result);
                    if let (Some(retry), Some(error)) =
                        (completed.remote_retry, completed.remote_resume_error)
                    {
                        SettingsFamilyOperationState::RemoteUnavailable {
                            retry,
                            error,
                            retry_task: None,
                            deferred_close: None,
                        }
                    } else {
                        SettingsFamilyOperationState::Idle
                    }
                }
                settings_restore::SettingsFamilyOperationPoll::WorkerFailed(error) => {
                    deferred_close = close_intent;
                    let action_label = describe_pending(&action).to_owned();
                    self.settings_restore_state.result = Some(ActionResult::FailedTerminal {
                        action_label,
                        error,
                    });
                    SettingsFamilyOperationState::Idle
                }
            },
            SettingsFamilyOperationState::RemoteUnavailable {
                retry,
                mut error,
                retry_task,
                deferred_close: close_intent,
            } => match retry_task {
                Some(mut task) => match task.poll_finished() {
                    UiWorkerPoll::Completed(Ok(())) => {
                        deferred_close = close_intent;
                        SettingsFamilyOperationState::Idle
                    }
                    UiWorkerPoll::Completed(Err(retry_error)) => {
                        deferred_close = close_intent;
                        error = retry_error;
                        SettingsFamilyOperationState::RemoteUnavailable {
                            retry,
                            error,
                            retry_task: None,
                            deferred_close: None,
                        }
                    }
                    UiWorkerPoll::Running => SettingsFamilyOperationState::RemoteUnavailable {
                        retry,
                        error,
                        retry_task: Some(task),
                        deferred_close: close_intent,
                    },
                    UiWorkerPoll::Failed(task_error) => {
                        deferred_close = close_intent;
                        error = task_error;
                        SettingsFamilyOperationState::RemoteUnavailable {
                            retry,
                            error,
                            retry_task: None,
                            deferred_close: None,
                        }
                    }
                },
                None => SettingsFamilyOperationState::RemoteUnavailable {
                    retry,
                    error,
                    retry_task: None,
                    deferred_close: close_intent,
                },
            },
        };
        if let Some(close) = deferred_close {
            match close {
                DeferredRootClose::WindowClose => self.request_main_window_close(ctx),
                DeferredRootClose::ApplicationQuit => self.request_application_quit(ctx),
            }
        }
    }

    fn show_settings_family_running_dialog(&mut self, ctx: &egui::Context) {
        let message = match &self.settings_restore_state.family_operation {
            SettingsFamilyOperationState::Running { .. } => Some("設定ファイルを準備しています…"),
            SettingsFamilyOperationState::RemoteUnavailable {
                retry_task: Some(_),
                ..
            } => Some("リモート設定readerを再接続しています…"),
            _ => None,
        };
        if let Some(message) = message {
            egui::Modal::new(egui::Id::new("settings_family_operation_modal")).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(message);
                });
            });
        }
    }

    fn start_remote_favorites_retry(&mut self, ctx: &egui::Context) {
        let SettingsFamilyOperationState::RemoteUnavailable {
            retry,
            error,
            retry_task: None,
            deferred_close: None,
        } = std::mem::replace(
            &mut self.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Idle,
        )
        else {
            return;
        };
        let worker_retry = retry.clone();
        let repaint_ctx = ctx.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("settings-favorites-resume".to_owned())
            .spawn(move || {
                let result = worker_retry.retry();
                let _ = tx.send(result);
                repaint_ctx.request_repaint();
            }) {
            Ok(handle) => {
                self.settings_restore_state.family_operation =
                    SettingsFamilyOperationState::RemoteUnavailable {
                        retry,
                        error,
                        retry_task: Some(UiWorkerTask {
                            receiver: rx,
                            handle: Some(handle),
                        }),
                        deferred_close: None,
                    };
            }
            Err(spawn_error) => {
                self.settings_restore_state.family_operation =
                    SettingsFamilyOperationState::RemoteUnavailable {
                        retry,
                        error: format!(
                            "リモート設定reader再接続 worker を開始できません: {spawn_error}"
                        ),
                        retry_task: None,
                        deferred_close: None,
                    };
            }
        }
    }

    fn show_settings_restore_result_dialog(&mut self, ctx: &egui::Context) {
        if self.settings_restore_state.result.is_none() {
            return;
        }
        if matches!(
            &self.settings_restore_state.family_operation,
            SettingsFamilyOperationState::RemoteUnavailable {
                retry_task: Some(_),
                ..
            }
        ) {
            return;
        }
        let mut closing = false;
        let mut retry_remote = false;
        let mut exit_for_remote = false;
        let remote_resume_error = match &self.settings_restore_state.family_operation {
            SettingsFamilyOperationState::RemoteUnavailable { error, .. } => Some(error.clone()),
            _ => None,
        };

        // self の借用を避けるため、必要情報を先に取り出してから描画する。
        let result_ref = self
            .settings_restore_state
            .result
            .as_ref()
            .expect("just checked is_some");
        let kind = result_kind(result_ref);
        let (title, lines) = build_result_body(result_ref);

        // 2026-05-17 Codex P2 round 2 対応: `egui::Modal` で背景 UI を完全にブロック
        // する。`egui::Window` だとモーダルでないので、ユーザーがダイアログを閉じずに
        // 背景の通常 UI を操作できてしまい、Terminal の「続行不可」分類が画面上は
        // 機能しなかった。Modal なら backdrop が背景クリックを全部吸う + 背景フォーカスを
        // 完全に奪う。
        let response =
            egui::Modal::new(egui::Id::new("settings_restore_result_modal")).show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.heading(title);
                ui.add_space(8.0);
                for line in lines {
                    if kind != ResultKind::Success {
                        ui.colored_label(egui::Color32::from_rgb(0xc0, 0x40, 0x40), line);
                    } else {
                        ui.label(line);
                    }
                }
                if let Some(error) = remote_resume_error.as_deref() {
                    ui.add_space(6.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(0xc0, 0x40, 0x40),
                        format!(
                            "リモートのお気に入り検索用readerを再接続できませんでした: {error}"
                        ),
                    );
                    ui.label(
                        "通常の設定アクセスは再開済みです。再試行するか、アプリを終了してください。",
                    );
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if remote_resume_error.is_some() {
                        if ui.button("リモート設定readerを再接続").clicked() {
                            retry_remote = true;
                        }
                        if ui.button("アプリを終了").clicked() {
                            exit_for_remote = true;
                        }
                    } else {
                        let button_label = match kind {
                            ResultKind::Success => "アプリを終了",
                            ResultKind::FailedRecoverable => "閉じる",
                            ResultKind::FailedTerminal => "アプリを終了して再起動を促す",
                        };
                        if ui.button(button_label).clicked() {
                            closing = true;
                        }
                    }
                });
            });

        // Recoverable のみ backdrop クリック / Esc を「閉じる」として受け付ける。
        // Terminal は backdrop / Esc も無効 (= ボタンクリックでしか抜けられない)。
        if kind == ResultKind::FailedRecoverable
            && remote_resume_error.is_none()
            && response.should_close()
        {
            closing = true;
        }

        if retry_remote {
            self.start_remote_favorites_retry(ctx);
            return;
        }
        if exit_for_remote {
            self.show_settings_restore = false;
            self.settings_restore_state = SettingsRestoreState::default();
            self.request_application_quit(ctx);
            return;
        }

        if closing {
            self.settings_restore_state.result = None;
            match kind {
                ResultKind::FailedRecoverable => {
                    // 状態は無傷、アプリ続行可。ダイアログを閉じるだけ。
                    self.show_settings_restore = false;
                    self.settings_restore_state = SettingsRestoreState::default();
                    if self.settings_boot_problem_source.is_some() {
                        self.show_settings_boot_problem_notice = true;
                    }
                }
                ResultKind::Success | ResultKind::FailedTerminal => {
                    // 成功 or Terminal failure: どちらもアプリを終了する。
                    // Terminal failure は handle 落ち + SAVE_SUPPRESSED で続行不可なので
                    // 強制的に終了させて次回起動時にクリーンに再 open させる。
                    // `shutdown_requested` を立てて tray 常駐の close 横取りを通す
                    // ([src/ui_main.rs] の「終了」ボタンと同じ方式)。
                    //
                    // 親ダイアログも閉じる: viewport close は 1-2 frame 遅延するので、
                    // その間に親ウィンドウが flicker するのを防ぐ。
                    self.show_settings_restore = false;
                    self.settings_restore_state = SettingsRestoreState::default();
                    self.request_application_quit(ctx);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    Success,
    FailedRecoverable,
    FailedTerminal,
}

fn result_kind(result: &ActionResult) -> ResultKind {
    match result {
        ActionResult::RestoreOk { .. } | ActionResult::ResetOk { .. } => ResultKind::Success,
        ActionResult::FailedRecoverable { .. } => ResultKind::FailedRecoverable,
        ActionResult::FailedTerminal { .. } => ResultKind::FailedTerminal,
    }
}

fn failure_to_action_result(
    failure: settings_restore::RestoreFailure,
    action_label: String,
) -> ActionResult {
    let error = failure.to_string();
    if failure.is_terminal() {
        ActionResult::FailedTerminal {
            action_label,
            error,
        }
    } else {
        ActionResult::FailedRecoverable {
            action_label,
            error,
        }
    }
}

fn describe_result(result: &ActionResult) -> &'static str {
    match result {
        ActionResult::RestoreOk { .. } => "ok(restore)",
        ActionResult::ResetOk { .. } => "ok(reset)",
        ActionResult::FailedRecoverable { .. } => "failed(recoverable)",
        ActionResult::FailedTerminal { .. } => "failed(terminal)",
    }
}

fn describe_pending(action: &PendingAction) -> String {
    match action {
        PendingAction::Restore(source) => format!("restore({})", source.filename()),
        PendingAction::FullReset => "full_reset".to_string(),
    }
}

fn build_result_body(result: &ActionResult) -> (&'static str, Vec<String>) {
    match result {
        ActionResult::RestoreOk { source, report } => {
            let mut lines = vec![format!(
                "「{}」の内容で設定を復元しました。",
                source.label()
            )];
            push_snapshot_lines(&mut lines, &report.snapshot_paths);
            lines.push(
                "「アプリを終了」を押すとアプリを閉じます。次回起動時に反映されます。".to_string(),
            );
            ("復元が完了しました", lines)
        }
        ActionResult::ResetOk { report } => {
            let mut lines = vec![format!(
                "設定ファイル一式 ({} 個) を削除しました。",
                report.deleted.len()
            )];
            push_snapshot_lines(&mut lines, &report.snapshot_paths);
            lines.push(
                "「アプリを終了」を押すとアプリを閉じます。次回起動時は初期状態になります。"
                    .to_string(),
            );
            ("リセットが完了しました", lines)
        }
        ActionResult::FailedRecoverable {
            action_label,
            error,
        } => {
            let lines = vec![
                format!("{action_label} に失敗しました。"),
                String::new(),
                error.clone(),
                String::new(),
                "設定ファイルは変更されていません。もう一度試すか、別の世代を選んでください。"
                    .to_string(),
            ];
            ("エラー", lines)
        }
        ActionResult::FailedTerminal {
            action_label,
            error,
        } => {
            let lines = vec![
                format!("{action_label} の途中で致命的なエラーが発生しました。"),
                String::new(),
                error.clone(),
                String::new(),
                "設定ファイルが不整合な状態になっている可能性があります。\
                 アプリを終了して再起動してください。"
                    .to_string(),
                "操作前の状態は `before-restore-*` という名前で残してあります。\
                 設定フォルダ内で確認できます。"
                    .to_string(),
            ];
            ("致命的なエラー", lines)
        }
    }
}

fn push_snapshot_lines(lines: &mut Vec<String>, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("操作前の状態は以下に退避しました (不要なら手動で削除できます):".to_string());
    for p in paths {
        // ファイル名だけ表示 (フルパスは長くて窓に収まらない)
        if let Some(name) = p.file_name() {
            lines.push(format!("  • {}", name.to_string_lossy()));
        } else {
            lines.push(format!("  • {}", p.display()));
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// 本体 (一覧 + ボタン)
// ──────────────────────────────────────────────────────────────────────

fn draw_body(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut app.settings_restore_state.tab,
            SettingsRestoreTab::Restore,
            "設定の復元",
        );
        if app.settings_boot_problem_source.is_none() {
            ui.selectable_value(
                &mut app.settings_restore_state.tab,
                SettingsRestoreTab::OperationCustomize,
                "操作カスタマイズ",
            );
        }
    });
    ui.separator();
    ui.add_space(6.0);
    match app.settings_restore_state.tab {
        SettingsRestoreTab::Restore => draw_restore_body(app, ui),
        SettingsRestoreTab::OperationCustomize => draw_operation_customize_body(app, ui),
    }
}

fn draw_restore_body(app: &mut App, ui: &mut egui::Ui) {
    ui.label(
        "現在の設定または過去のバックアップから設定を復元できます。\n\
         復元すると現在の設定は上書きされ、アプリは自動で終了します。",
    );
    ui.add_space(8.0);

    // 一覧テーブル。
    let now = SystemTime::now();
    egui::ScrollArea::both()
        .max_height(360.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("settings_restore_grid")
                .num_columns(9)
                .striped(true)
                .spacing(egui::vec2(12.0, 4.0))
                .show(ui, |ui| {
                    // ヘッダ
                    ui.strong("世代");
                    ui.strong("保存した版");
                    ui.strong("互換性");
                    ui.strong("日時");
                    ui.strong("サイズ");
                    ui.strong("お気に入り");
                    ui.strong("タグ");
                    ui.strong("動画再開");
                    ui.strong("操作");
                    ui.end_row();

                    for backup in &app.settings_restore_state.backups {
                        // 世代
                        ui.label(backup.source.label());
                        ui.label(
                            backup
                                .content_app_version
                                .as_deref()
                                .unwrap_or("不明"),
                        );
                        let boot_incompatible_current = matches!(
                            app.settings_boot_problem_source,
                            Some(crate::settings_db::BootSource::IncompatibleSettings)
                        )
                            && backup.source.is_current();
                        let newer = boot_incompatible_current
                            || backup.compatibility == BackupCompatibility::NewerVersion;
                        if newer {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xc0, 0x40, 0x40),
                                "利用不可（新しい版）",
                            );
                        } else {
                            match backup.compatibility {
                                BackupCompatibility::RestoreCandidate => {
                                    ui.label("復元候補");
                                }
                                BackupCompatibility::Unknown => {
                                    ui.weak("要確認");
                                }
                                BackupCompatibility::NewerVersion => unreachable!(),
                            }
                        }
                        // 日時 (= mtime の相対表現)
                        let when = backup
                            .mtime
                            .map(|t| format_relative_time(t, now))
                            .unwrap_or_else(|| "—".to_string());
                        ui.label(when);
                        // サイズ
                        ui.label(format_bytes(backup.size));
                        // 件数
                        ui.label(backup.favorites.to_string());
                        ui.label(backup.tags.to_string());
                        ui.label(backup.video_resume.to_string());
                        // 操作 (現在の設定行はボタンなし)
                        match &backup.source {
                            BackupSource::Current => {
                                ui.weak("(現在使用中)");
                            }
                            BackupSource::Bak(_) | BackupSource::PreUpgrade(_) => {
                                if ui
                                    .add_enabled(!newer, egui::Button::new("この時点に戻す…"))
                                    .on_disabled_hover_text(
                                        "このバックアップは現在のアプリより新しい版で保存されています。",
                                    )
                                    .clicked()
                                {
                                    app.settings_restore_state.pending =
                                        Some(PendingAction::Restore(backup.source.clone()));
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
        });

    if let Some(err) = first_partial_error(&app.settings_restore_state.backups) {
        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::from_rgb(0xc0, 0x40, 0x40),
            format!("一部の世代は読み取りに問題があります: {err}"),
        );
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("使えるバックアップが無い、または初期状態に戻したい場合:");
        if ui.button("設定を完全リセット…").clicked() {
            app.settings_restore_state.pending = Some(PendingAction::FullReset);
        }
    });
}

impl App {
    fn poll_operation_share_task(&mut self, ctx: &egui::Context) {
        let received = self
            .settings_restore_state
            .operation_task_rx
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "バックグラウンド処理が応答せず終了しました。".to_string(),
                )),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        let Some(received) = received else {
            if self.settings_restore_state.operation_task_rx.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        };
        self.settings_restore_state.operation_task_rx = None;
        let result = match received {
            Ok(result) => result,
            Err(error) => {
                self.settings_restore_state.operation_message = Some((true, error));
                return;
            }
        };
        match result {
            OperationTaskResult::LoadedRow {
                source,
                action,
                result,
            } => finish_loaded_row_action(self, source, action, result),
            OperationTaskResult::ImportedFile {
                result,
                fallback_label,
            } => match result {
                Ok(parsed) => {
                    let source_label = parsed.bundle.label.clone().unwrap_or(fallback_label);
                    self.settings_restore_state.operation_modal = Some(OperationModal::Import {
                        source_label,
                        bundle: parsed.bundle,
                        warnings: parsed.warnings,
                        ignored_items: parsed.ignored_items,
                    });
                    self.settings_restore_state.operation_message = None;
                }
                Err(error) => {
                    self.settings_restore_state.operation_message =
                        Some((true, format!("ファイルを読み込めませんでした: {error}")));
                }
            },
            OperationTaskResult::LoadedPrevious { source, result } => {
                if let Some(OperationModal::Diff {
                    source: modal_source,
                    previous,
                    ..
                }) = self.settings_restore_state.operation_modal.as_mut()
                    && *modal_source == source
                {
                    *previous = Some(result);
                }
            }
            OperationTaskResult::Exported { result } => match result {
                Ok(message) => {
                    self.settings_restore_state.operation_message = Some((false, message));
                }
                Err(error) => {
                    self.settings_restore_state.operation_message =
                        Some((true, format!("書き出しに失敗しました: {error}")));
                }
            },
            OperationTaskResult::PreparedApply {
                kind,
                bundle,
                result,
            } => match result {
                Ok(path) => {
                    self.apply_operation_customize_bundle(bundle);
                    let backup_name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let message = operation_apply_success_message(&kind, &backup_name);
                    self.settings_restore_state.operation_message = Some((false, message));
                }
                Err(error) => {
                    let message = operation_apply_failure_message(&kind, &error);
                    self.settings_restore_state.operation_message = Some((true, message));
                }
            },
        }
    }

    fn show_operation_share_modal(&mut self, ctx: &egui::Context) {
        let Some(mut modal) = self.settings_restore_state.operation_modal.take() else {
            return;
        };
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let mut keep_open = true;
        let mut apply_request = None;

        match &mut modal {
            OperationModal::Export {
                source_label,
                bundle,
                label,
            } => {
                let mut window_open = true;
                let mut save = false;
                egui::Window::new("操作カスタマイズを書き出す")
                    .open(&mut window_open)
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label(format!("書き出し元: {source_label}"));
                        ui.label("共有用の名前 (省略可):");
                        crate::ime_focus::add_singleline(ui, label, None, |edit| {
                            edit.hint_text("例: ブラウザー風")
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("保存...").clicked() || enter_pressed {
                                save = true;
                            }
                            if ui.button("キャンセル").clicked() || escape_pressed {
                                keep_open = false;
                            }
                        });
                    });
                if !window_open {
                    keep_open = false;
                }
                if save {
                    let export_bundle = bundle.clone().with_label(label.clone());
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("mIV operation customize", &["json"])
                        .set_file_name("operation-customize.mivkeys.json")
                        .save_file()
                    {
                        begin_operation_export(
                            self,
                            ensure_operation_customize_extension(path),
                            export_bundle,
                        );
                        keep_open = false;
                    }
                }
            }
            OperationModal::Diff {
                source_label,
                source,
                bundle,
                current,
                target,
                previous,
            } => {
                let mut window_open = true;
                let modal_size = operation_modal_size(ctx.available_rect().size());
                egui::Window::new("操作カスタマイズの差分")
                    .open(&mut window_open)
                    .collapsible(false)
                    .resizable(false)
                    .fixed_size(modal_size)
                    .show(ctx, |ui| {
                        ui.label(format!("比較する設定: {source_label}"));
                        ui.horizontal(|ui| {
                            ui.label("比較元:");
                            ui.selectable_value(target, DiffCompareTarget::Standard, "標準");
                            ui.add_enabled_ui(!source.is_current(), |ui| {
                                ui.selectable_value(target, DiffCompareTarget::Current, "現在");
                            });
                            ui.selectable_value(target, DiffCompareTarget::Previous, "前世代");
                        });

                        if *target == DiffCompareTarget::Previous && previous.is_none() {
                            if begin_previous_operation_load(self, source.clone()) {
                                *previous = Some(Err("前世代を読み込んでいます...".to_string()));
                            } else {
                                *previous = Some(Err(
                                    "前世代の読み込みを開始できませんでした。".to_string()
                                ));
                            }
                        }

                        let defaults =
                            crate::operation_customize_share::OperationCustomizeBundle::defaults();
                        let comparison = match *target {
                            DiffCompareTarget::Standard => Some(("標準".to_string(), &defaults)),
                            DiffCompareTarget::Current => Some(("現在".to_string(), &*current)),
                            DiffCompareTarget::Previous => match previous.as_ref() {
                                Some(Ok((label, previous_bundle))) => {
                                    Some((label.clone(), previous_bundle))
                                }
                                _ => None,
                            },
                        };
                        ui.add_space(6.0);
                        draw_operation_modal_scroll_body(
                            ui,
                            "operation_customize_diff_scroll",
                            modal_size.y,
                            |ui| {
                                if let Some((before_label, before)) = comparison {
                                    let operation_diff =
                                        crate::operation_customize_share::diff(before, bundle);
                                    ui.label(format!("{before_label} -> {source_label}"));
                                    draw_operation_diff(ui, &operation_diff);
                                } else if let Some(Err(error)) = previous.as_ref() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0xc0, 0x40, 0x40),
                                        error,
                                    );
                                }
                            },
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        if ui.button("閉じる").clicked() || escape_pressed {
                            keep_open = false;
                        }
                    });
                if !window_open {
                    keep_open = false;
                }
            }
            OperationModal::Import {
                source_label,
                bundle,
                warnings,
                ignored_items,
            } => {
                let current =
                    crate::operation_customize_share::OperationCustomizeBundle::from_settings(
                        &self.settings,
                    );
                let preview = crate::operation_customize_share::diff(&current, bundle);
                let mut window_open = true;
                let mut apply = false;
                let modal_size = operation_modal_size(ctx.available_rect().size());
                egui::Window::new("操作カスタマイズを取り込む")
                    .open(&mut window_open)
                    .collapsible(false)
                    .resizable(false)
                    .fixed_size(modal_size)
                    .show(ctx, |ui| {
                        ui.label(format!("取り込み元: {source_label}"));
                        ui.label("現在 -> 取り込み後の差分を確認してください。");
                        ui.add_space(6.0);
                        draw_operation_modal_scroll_body(
                            ui,
                            "operation_customize_import_scroll",
                            modal_size.y,
                            |ui| {
                                if *ignored_items > 0 {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0xc0, 0x70, 0x20),
                                        format!("無視した項目: {ignored_items} 件"),
                                    );
                                }
                                if !warnings.is_empty() {
                                    egui::CollapsingHeader::new(format!(
                                        "読み込み時の注意: {} 件",
                                        warnings.len()
                                    ))
                                    .show(ui, |ui| {
                                        for warning in warnings.iter() {
                                            ui.label(warning);
                                        }
                                    });
                                }
                                draw_operation_diff(ui, &preview);
                            },
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("取り込む").clicked() {
                                apply = true;
                            }
                            if ui.button("キャンセル").clicked() || escape_pressed {
                                keep_open = false;
                            }
                        });
                    });
                if !window_open {
                    keep_open = false;
                }
                if apply {
                    apply_request = Some((
                        OperationApplyKind::Import {
                            source_label: source_label.clone(),
                        },
                        bundle.clone(),
                    ));
                    keep_open = false;
                }
            }
            OperationModal::ResetConfirm => {
                let mut window_open = true;
                let mut reset = false;
                egui::Window::new("操作カスタマイズを初期値に戻す")
                    .open(&mut window_open)
                    .collapsible(false)
                    .resizable(false)
                    .default_pos(ctx.content_rect().min + egui::vec2(100.0, 80.0))
                    .show(ctx, |ui| {
                        ui.label(
                            "操作カスタマイズ (キー/リング/メニュー) を初期値に戻します。\nよろしいですか？",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("初期値に戻す").clicked() {
                                reset = true;
                            }
                            if ui.button("キャンセル").clicked() || escape_pressed {
                                keep_open = false;
                            }
                        });
                    });
                if !window_open {
                    keep_open = false;
                }
                if reset {
                    apply_request = Some((
                        OperationApplyKind::ResetDefaults,
                        crate::operation_customize_share::OperationCustomizeBundle::defaults()
                            .with_label("初期値"),
                    ));
                    keep_open = false;
                }
            }
        }

        if let Some((kind, bundle)) = apply_request {
            begin_operation_apply_backup(self, kind, bundle);
        }

        if keep_open {
            self.settings_restore_state.operation_modal = Some(modal);
        }
    }
}

#[derive(Clone, Copy)]
enum OperationRowAction {
    Diff,
    Export,
    Import,
}

fn draw_operation_customize_body(app: &mut App, ui: &mut egui::Ui) {
    ui.label(
        "キー割り当て、リング/マウス、メニュー構成をまとめて共有・比較できます。\n         操作カスタマイズだけを取り込み、再起動せずに反映します。",
    );
    ui.add_space(8.0);

    let backups = app.settings_restore_state.backups.clone();
    let now = SystemTime::now();
    egui::ScrollArea::vertical()
        .max_height(340.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("operation_customize_share_grid")
                .num_columns(5)
                .striped(true)
                .spacing(egui::vec2(10.0, 4.0))
                .show(ui, |ui| {
                    ui.strong("世代");
                    ui.strong("日時");
                    ui.strong("差分");
                    ui.strong("共有");
                    ui.strong("取り込み");
                    ui.end_row();

                    for backup in backups {
                        ui.label(backup.source.label());
                        let when = backup
                            .mtime
                            .map(|time| format_relative_time(time, now))
                            .unwrap_or_else(|| "-".to_string());
                        ui.label(when);
                        if ui.button("差分を見る...").clicked() {
                            begin_operation_row_action(
                                app,
                                backup.source.clone(),
                                OperationRowAction::Diff,
                            );
                        }
                        if ui.button("書き出す...").clicked() {
                            begin_operation_row_action(
                                app,
                                backup.source.clone(),
                                OperationRowAction::Export,
                            );
                        }
                        if backup.source.is_current() {
                            ui.weak("(現在使用中)");
                        } else if ui.button("取り込む...").clicked() {
                            begin_operation_row_action(
                                app,
                                backup.source.clone(),
                                OperationRowAction::Import,
                            );
                        }
                        ui.end_row();
                    }
                });
        });

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui.button("ファイルから読み込む...").clicked()
            && app.settings_restore_state.operation_task_rx.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("mIV operation customize", &["json"])
                .pick_file()
        {
            begin_file_import(app, path);
        }
        ui.weak("取り込みは現在の操作カスタマイズを置き換えます。");
    });

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("キー割り当て、リング/マウス、メニュー構成だけを初期値に戻す場合:");
        if ui
            .add_enabled(
                app.settings_restore_state.operation_task_rx.is_none(),
                egui::Button::new("操作カスタマイズを初期値に戻す..."),
            )
            .clicked()
        {
            app.settings_restore_state.operation_modal = Some(OperationModal::ResetConfirm);
        }
    });

    if let Some((is_error, message)) = &app.settings_restore_state.operation_message {
        ui.add_space(8.0);
        if *is_error {
            ui.colored_label(egui::Color32::from_rgb(0xc0, 0x40, 0x40), message);
        } else {
            ui.label(message);
        }
    }
}

fn begin_operation_row_action(app: &mut App, source: BackupSource, action: OperationRowAction) {
    if app.settings_restore_state.operation_task_rx.is_some() {
        return;
    }
    let source_label = source.label();
    if source.is_current() {
        let result = normalize_operation_bundle(
            crate::operation_customize_share::OperationCustomizeBundle::from_settings(
                &app.settings,
            )
            .with_label(source_label),
        );
        finish_loaded_row_action(app, source, action, result);
        return;
    }

    let data_dir = crate::data_dir::get();
    let worker_source = source.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    match std::thread::Builder::new()
        .name("operation-customize-load".to_string())
        .spawn(move || {
            let result = settings_restore::load_operation_customize(&data_dir, &worker_source)
                .map_err(|error| error.to_string())
                .and_then(|bundle| {
                    normalize_operation_bundle(bundle.with_label(worker_source.label()))
                });
            let _ = tx.send(OperationTaskResult::LoadedRow {
                source: worker_source,
                action,
                result,
            });
        }) {
        Ok(_) => {
            app.settings_restore_state.operation_task_rx = Some(rx);
            app.settings_restore_state.operation_message =
                Some((false, "世代を読み込んでいます...".to_string()));
        }
        Err(error) => {
            app.settings_restore_state.operation_message =
                Some((true, format!("読み込み処理を開始できませんでした: {error}")));
        }
    }
}

fn finish_loaded_row_action(
    app: &mut App,
    source: BackupSource,
    action: OperationRowAction,
    result: Result<crate::operation_customize_share::ParsedImport, String>,
) {
    match result {
        Ok(parsed) => {
            let source_label = source.label();
            let bundle = parsed.bundle;
            let current = crate::operation_customize_share::OperationCustomizeBundle::from_settings(
                &app.settings,
            );
            app.settings_restore_state.operation_modal = Some(match action {
                OperationRowAction::Diff => OperationModal::Diff {
                    source_label,
                    source,
                    bundle,
                    current,
                    target: DiffCompareTarget::Standard,
                    previous: None,
                },
                OperationRowAction::Export => OperationModal::Export {
                    source_label,
                    bundle,
                    label: String::new(),
                },
                OperationRowAction::Import => OperationModal::Import {
                    source_label,
                    bundle,
                    warnings: parsed.warnings,
                    ignored_items: parsed.ignored_items,
                },
            });
            app.settings_restore_state.operation_message = None;
        }
        Err(error) => {
            app.settings_restore_state.operation_message =
                Some((true, format!("世代を読み込めませんでした: {error}")));
        }
    }
}

fn begin_file_import(app: &mut App, path: PathBuf) {
    if app.settings_restore_state.operation_task_rx.is_some() {
        return;
    }
    let fallback_label = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ファイル".to_string());
    let (tx, rx) = std::sync::mpsc::channel();
    match std::thread::Builder::new()
        .name("operation-customize-file-import".to_string())
        .spawn(move || {
            let result = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|json| {
                    crate::operation_customize_share::parse_json(&json)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(OperationTaskResult::ImportedFile {
                result,
                fallback_label,
            });
        }) {
        Ok(_) => {
            app.settings_restore_state.operation_task_rx = Some(rx);
            app.settings_restore_state.operation_message =
                Some((false, "ファイルを読み込んでいます...".to_string()));
        }
        Err(error) => {
            app.settings_restore_state.operation_message =
                Some((true, format!("読み込み処理を開始できませんでした: {error}")));
        }
    }
}

fn begin_operation_export(
    app: &mut App,
    path: PathBuf,
    bundle: crate::operation_customize_share::OperationCustomizeBundle,
) {
    if app.settings_restore_state.operation_task_rx.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    match std::thread::Builder::new()
        .name("operation-customize-export".to_string())
        .spawn(move || {
            let result = crate::operation_customize_share::to_json(&bundle)
                .map_err(|error| error.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|error| error.to_string()))
                .map(|()| {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    format!("{name} に書き出しました。")
                });
            let _ = tx.send(OperationTaskResult::Exported { result });
        }) {
        Ok(_) => {
            app.settings_restore_state.operation_task_rx = Some(rx);
            app.settings_restore_state.operation_message =
                Some((false, "ファイルへ書き出しています...".to_string()));
        }
        Err(error) => {
            app.settings_restore_state.operation_message =
                Some((true, format!("書き出し処理を開始できませんでした: {error}")));
        }
    }
}

fn begin_operation_apply_backup(
    app: &mut App,
    kind: OperationApplyKind,
    bundle: crate::operation_customize_share::OperationCustomizeBundle,
) {
    if app.settings_restore_state.operation_task_rx.is_some() {
        return;
    }
    let backup_label = match &kind {
        OperationApplyKind::Import { .. } => "取り込み前",
        OperationApplyKind::ResetDefaults => "初期値へ戻す前",
    };
    let current =
        crate::operation_customize_share::OperationCustomizeBundle::from_settings(&app.settings)
            .with_label(backup_label);
    let data_dir = crate::data_dir::get();
    let worker_bundle = bundle.clone();
    let worker_kind = kind.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    match std::thread::Builder::new()
        .name("operation-customize-backup".to_string())
        .spawn(move || {
            let result =
                settings_restore::backup_operation_customize_before_import(&data_dir, &current)
                    .map_err(|error| error.to_string());
            let _ = tx.send(OperationTaskResult::PreparedApply {
                kind: worker_kind,
                bundle: worker_bundle,
                result,
            });
        }) {
        Ok(_) => {
            app.settings_restore_state.operation_task_rx = Some(rx);
            let message = match &kind {
                OperationApplyKind::Import { .. } => "取り込み前の設定を退避しています...",
                OperationApplyKind::ResetDefaults => "初期値へ戻す前の設定を退避しています...",
            };
            app.settings_restore_state.operation_message = Some((false, message.to_string()));
        }
        Err(error) => {
            let action = match &kind {
                OperationApplyKind::Import { .. } => "取り込み",
                OperationApplyKind::ResetDefaults => "リセット",
            };
            app.settings_restore_state.operation_message = Some((
                true,
                format!("自動退避処理を開始できなかったため、{action}を中止しました: {error}"),
            ));
        }
    }
}

fn normalize_operation_bundle(
    bundle: crate::operation_customize_share::OperationCustomizeBundle,
) -> Result<crate::operation_customize_share::ParsedImport, String> {
    crate::operation_customize_share::to_json(&bundle)
        .map_err(|error| error.to_string())
        .and_then(|json| {
            crate::operation_customize_share::parse_json(&json).map_err(|error| error.to_string())
        })
}

fn begin_previous_operation_load(app: &mut App, source: BackupSource) -> bool {
    if app.settings_restore_state.operation_task_rx.is_some() {
        return false;
    }
    let backups = app.settings_restore_state.backups.clone();
    let worker_source = source.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("operation-customize-previous".to_string())
        .spawn(move || {
            let result = load_previous_operation_bundle(&backups, &worker_source);
            let _ = tx.send(OperationTaskResult::LoadedPrevious {
                source: worker_source,
                result,
            });
        });
    if spawned.is_err() {
        return false;
    }
    app.settings_restore_state.operation_task_rx = Some(rx);
    true
}

fn load_previous_operation_bundle(
    backups: &[BackupSummary],
    source: &BackupSource,
) -> Result<
    (
        String,
        crate::operation_customize_share::OperationCustomizeBundle,
    ),
    String,
> {
    let previous = match source {
        BackupSource::Current => BackupSource::Bak(1),
        BackupSource::Bak(number) if *number < 10 => BackupSource::Bak(number + 1),
        BackupSource::Bak(_) => return Err("前世代はありません。".to_string()),
        BackupSource::PreUpgrade(_) => return Err("前世代はありません。".to_string()),
    };
    if !backups.iter().any(|backup| backup.source == previous) {
        return Err("前世代はありません。".to_string());
    }
    let label = previous.label();
    settings_restore::load_operation_customize(&crate::data_dir::get(), &previous)
        .map(|bundle| (label, bundle))
        .map_err(|error| format!("前世代を読み込めませんでした: {error}"))
}

fn draw_operation_modal_scroll_body(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    modal_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let body_height = operation_modal_body_height(modal_height);
    let body_size = egui::vec2(ui.available_width(), body_height);
    ui.allocate_ui_with_layout(body_size, egui::Layout::top_down(egui::Align::Min), |ui| {
        egui::ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink([false, false])
            .max_height(body_height)
            .show(ui, add_contents);
    });
}

fn operation_modal_body_height(modal_height: f32) -> f32 {
    (modal_height - OPERATION_MODAL_HEADER_RESERVE - OPERATION_MODAL_FOOTER_RESERVE).max(80.0)
}

fn operation_modal_size(viewport_size: egui::Vec2) -> egui::Vec2 {
    let available = viewport_size - OPERATION_MODAL_VIEWPORT_MARGIN;
    egui::vec2(
        available
            .x
            .clamp(OPERATION_MODAL_MIN_SIZE.x, OPERATION_MODAL_SIZE.x),
        available
            .y
            .clamp(OPERATION_MODAL_MIN_SIZE.y, OPERATION_MODAL_SIZE.y),
    )
}

fn draw_operation_diff(
    ui: &mut egui::Ui,
    operation_diff: &crate::operation_customize_share::OperationDiff,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(format!(
            "キー割り当て: {} 件",
            operation_diff.key_changes.len()
        ));
        ui.separator();
        ui.label(format!(
            "リング/マウス: {}",
            if operation_diff.ring_change_count == 0 {
                "変更なし".to_string()
            } else {
                format!("変更 {} 件", operation_diff.ring_change_count)
            }
        ));
        ui.separator();
        ui.label(format!(
            "メニュー構成: {}",
            if operation_diff.menu_change_count == 0 {
                "変更なし".to_string()
            } else {
                format!("変更 {} 件", operation_diff.menu_change_count)
            }
        ));
    });
    if operation_diff.is_empty() {
        ui.weak("差分はありません。");
        return;
    }

    egui::Grid::new("operation_customize_diff_grid")
        .num_columns(3)
        .striped(true)
        .spacing(egui::vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.strong("コマンド");
            ui.strong("変更前");
            ui.strong("変更後");
            ui.end_row();
            for change in &operation_diff.key_changes {
                ui.label(format!(
                    "{} / {}",
                    change.action.context().description(),
                    change.action.description()
                ));
                ui.label(format_chords(&change.before));
                ui.label(format_chords(&change.after));
                ui.end_row();
            }
        });
}

fn format_chords(chords: &[String]) -> String {
    if chords.is_empty() {
        "(なし)".to_string()
    } else {
        chords.join(" / ")
    }
}

fn ensure_operation_customize_extension(path: PathBuf) -> PathBuf {
    if path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".mivkeys.json")
    {
        path
    } else {
        let mut filename = path.into_os_string();
        filename.push(".mivkeys.json");
        PathBuf::from(filename)
    }
}

fn first_partial_error(backups: &[BackupSummary]) -> Option<String> {
    backups
        .iter()
        .find_map(|b| b.partial_error.clone().map(|e| (b.source.label(), e)))
        .map(|(label, e)| format!("{label}: {e}"))
}

fn format_relative_time(mtime: SystemTime, now: SystemTime) -> String {
    let Ok(dur) = now.duration_since(mtime) else {
        return "未来".to_string();
    };
    let secs = dur.as_secs();
    if secs < 60 {
        "たった今".to_string()
    } else if secs < 3600 {
        format!("{} 分前", secs / 60)
    } else if secs < 86_400 {
        format!("{} 時間前", secs / 3600)
    } else if secs < 86_400 * 7 {
        format!("{} 日前", secs / 86_400)
    } else if secs < 86_400 * 30 {
        format!("{} 週間前", secs / (86_400 * 7))
    } else {
        format!("{} か月前", secs / (86_400 * 30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyBindingOverride;
    use egui_kittest::Harness;

    #[test]
    fn settings_family_close_policy_preserves_tray_hide_and_latches_process_exit() {
        assert_eq!(
            settings_family_deferred_root_close(false, true),
            None,
            "plain close remains owned by the existing tray-hide path"
        );
        assert_eq!(
            settings_family_deferred_root_close(false, false),
            Some(DeferredRootClose::WindowClose)
        );
        assert_eq!(
            settings_family_deferred_root_close(true, true),
            Some(DeferredRootClose::ApplicationQuit),
            "explicit/tray quit takes precedence over tray hide"
        );
        assert_eq!(
            DeferredRootClose::WindowClose.merge(DeferredRootClose::ApplicationQuit),
            DeferredRootClose::ApplicationQuit,
            "a later explicit quit upgrades an already deferred plain close"
        );
    }

    #[test]
    fn hidden_explicit_quit_keeps_the_wake_until_the_exact_task_reissues_close() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.minimize_to_tray_on_close = false;
        app.window_visible = false;
        app.shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = settings_restore::SettingsFamilyOperationTask::waiting_for_release_for_test(
            release_rx,
            std::sync::Arc::clone(&worker_finished),
        );
        app.settings_restore_state.family_operation = SettingsFamilyOperationState::Running {
            action: PendingAction::Restore(BackupSource::Bak(1)),
            task,
            deferred_close: None,
        };
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);

        let blocked = ctx.run(raw, |ctx| app.defer_settings_family_root_close(ctx));
        assert!(
            blocked
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| viewport
                    .commands
                    .contains(&egui::ViewportCommand::CancelClose))
        );
        assert!(matches!(
            app.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Running {
                deferred_close: Some(DeferredRootClose::ApplicationQuit),
                ..
            }
        ));
        assert!(app.tray_resident_media_updates_needed());
        let app_source = include_str!("../app.rs");
        let update = app_source
            .find("fn update(&mut self, ctx: &egui::Context")
            .expect("eframe update wrapper");
        let update_source = &app_source[update..];
        assert!(
            update_source
                .find("defer_settings_family_root_close(ctx)")
                .expect("close latch")
                < update_source
                    .find("self.update_frame(ctx, frame)")
                    .expect("tray wake publication pass"),
            "the hidden-root close latch must precede update_frame's tray wake publication"
        );

        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                app.poll_settings_family_operation(ctx)
            });
            if output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| viewport.commands.contains(&egui::ViewportCommand::Close))
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(matches!(
            app.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Idle
        ));
        assert!(worker_finished.load(std::sync::atomic::Ordering::Acquire));
        assert!(!app.tray_resident_media_updates_needed());
    }

    #[test]
    fn exit_fence_joins_the_mutation_worker_before_settings_persistence() {
        let mut app = crate::app::setup_app_for_test();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = settings_restore::SettingsFamilyOperationTask::waiting_for_release_for_test(
            release_rx,
            std::sync::Arc::clone(&worker_finished),
        );
        app.settings_restore_state.family_operation = SettingsFamilyOperationState::Running {
            action: PendingAction::Restore(BackupSource::Bak(1)),
            task,
            deferred_close: None,
        };
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            release_tx.send(()).unwrap();
        });

        app.resolve_settings_family_operation_for_exit();

        release.join().unwrap();
        assert!(worker_finished.load(std::sync::atomic::Ordering::Acquire));
        assert!(matches!(
            app.settings_restore_state.family_operation,
            SettingsFamilyOperationState::Idle
        ));
        let app_source = include_str!("../app.rs");
        let on_exit = app_source
            .find("fn on_exit(&mut self")
            .expect("eframe on_exit owner");
        let exit_source = &app_source[on_exit..];
        assert!(
            exit_source
                .find("self.resolve_settings_family_operation_for_exit()")
                .expect("settings-family exit fence")
                < exit_source
                    .find("self.on_exit_inner()")
                    .expect("settings persistence boundary")
        );
    }

    #[test]
    fn operation_modal_size_is_clamped_to_the_viewport() {
        assert_eq!(
            operation_modal_size(egui::vec2(1_200.0, 900.0)),
            OPERATION_MODAL_SIZE
        );
        assert_eq!(
            operation_modal_size(egui::vec2(640.0, 580.0)),
            egui::vec2(608.0, 540.0)
        );
        assert_eq!(
            operation_modal_size(egui::vec2(500.0, 400.0)),
            OPERATION_MODAL_MIN_SIZE
        );
    }

    #[test]
    fn operation_modal_body_height_is_derived_only_from_the_fixed_modal_height() {
        assert_eq!(operation_modal_body_height(OPERATION_MODAL_SIZE.y), 440.0);
        assert_eq!(
            operation_modal_body_height(OPERATION_MODAL_MIN_SIZE.y),
            320.0
        );
    }

    #[test]
    fn operation_customize_diff_preview_snapshot() {
        let defaults = crate::operation_customize_share::OperationCustomizeBundle::defaults();
        let mut imported = defaults.clone();
        imported.keymap.overrides = vec![
            KeyBindingOverride {
                action: "GridPin".to_string(),
                chords: vec!["Ctrl+P".to_string()],
            },
            KeyBindingOverride {
                action: "FsSlideshow".to_string(),
                chords: Vec::new(),
            },
        ];
        imported.ring_shortcuts.mouse_ring_help_visible = false;
        imported.menu_layout.hidden_commands = vec!["SettingsOperationCustomize".to_string()];
        let operation_diff = crate::operation_customize_share::diff(&defaults, &imported);

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(760.0, 420.0))
            .build(move |ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                if !fonts_ready {
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("操作カスタマイズの差分");
                    ui.horizontal(|ui| {
                        let _ = ui.selectable_label(false, "標準");
                        let _ = ui.selectable_label(true, "現在");
                        let _ = ui.selectable_label(false, "前世代");
                    });
                    ui.separator();
                    draw_operation_diff(ui, &operation_diff);
                });
            });
        harness.run();
        harness.snapshot("settings_restore_operation_diff_preview");
    }
}
