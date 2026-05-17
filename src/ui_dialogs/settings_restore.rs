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
use crate::settings_restore::{self, BackupSource, BackupSummary, ResetReport, RestoreReport};
use crate::ui_helpers::format_bytes;

/// ダイアログの状態。Default で「未起動」。`App::open_settings_restore` で
/// 一覧をロードし、`show_settings_restore_dialog` で描画する。
pub(crate) struct SettingsRestoreState {
    /// 一覧キャッシュ (= ダイアログ起動時に固定)。
    pub(crate) backups: Vec<BackupSummary>,
    /// ユーザーがボタンを押したアクション (= 確認待ち or 実行待ち)。
    pub(crate) pending: Option<PendingAction>,
    /// 実行結果 (= 成功なら snapshot 情報、失敗ならエラー文字列)。
    /// `Some` の状態でダイアログを閉じると `ViewportCommand::Close` でアプリ終了。
    pub(crate) result: Option<ActionResult>,
}

impl Default for SettingsRestoreState {
    fn default() -> Self {
        Self {
            backups: Vec::new(),
            pending: None,
            result: None,
        }
    }
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
    /// メニューから呼ぶ起動エントリ。
    pub(crate) fn open_settings_restore_dialog(&mut self) {
        let dir = crate::data_dir::get();
        let backups = settings_restore::list_backups(&dir);
        self.settings_restore_state = SettingsRestoreState {
            backups,
            pending: None,
            result: None,
        };
        self.show_settings_restore = true;
    }

    pub(crate) fn show_settings_restore_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_restore {
            return;
        }

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        // 確認ダイアログ / 結果ダイアログを出している間は escape を「メイン閉じる」
        // に使わない (= 子側のキャンセル扱いにしたい)。結果ダイアログは `egui::Modal`
        // で背景入力をブロックするので、親側の [x] も内部的にクリックされない
        // (2026-05-17 Codex P2 round 2 対応)。
        let confirm_open = self.settings_restore_state.pending.is_some();
        let result_open = self.settings_restore_state.result.is_some();

        egui::Window::new("設定の復元")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_width(640.0)
            .show(ctx, |ui| {
                draw_body(self, ui);
            });

        if !open || (escape_pressed && !confirm_open && !result_open) {
            self.show_settings_restore = false;
            self.settings_restore_state = SettingsRestoreState::default();
        }

        // 確認 / 完了の各ダイアログ。結果ダイアログは `egui::Modal` で背景全部を
        // ブロックするので、Terminal でも Recoverable でも背景 UI 操作は不可能。
        self.show_settings_restore_confirm_dialog(ctx);
        self.show_settings_restore_result_dialog(ctx);
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
            self.execute_pending_settings_restore(pending);
        }
    }

    fn execute_pending_settings_restore(&mut self, pending: PendingAction) {
        let dir = crate::data_dir::get();
        let result = match &pending {
            PendingAction::Restore(source) => match settings_restore::restore_from(&dir, source) {
                Ok(report) => ActionResult::RestoreOk {
                    source: source.clone(),
                    report,
                },
                Err(failure) => {
                    failure_to_action_result(failure, format!("「{}」からの復元", source.label()))
                }
            },
            PendingAction::FullReset => match settings_restore::full_reset(&dir) {
                Ok(report) => ActionResult::ResetOk { report },
                Err(failure) => failure_to_action_result(failure, "完全リセット".to_string()),
            },
        };
        crate::logger::log(format!(
            "[settings_restore] action executed: {} -> {}",
            describe_pending(&pending),
            describe_result(&result)
        ));
        self.settings_restore_state.pending = None;
        self.settings_restore_state.result = Some(result);
    }

    fn show_settings_restore_result_dialog(&mut self, ctx: &egui::Context) {
        if self.settings_restore_state.result.is_none() {
            return;
        }
        let mut closing = false;

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
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                let button_label = match kind {
                    ResultKind::Success => "アプリを終了",
                    ResultKind::FailedRecoverable => "閉じる",
                    ResultKind::FailedTerminal => "アプリを終了して再起動を促す",
                };
                ui.horizontal(|ui| {
                    if ui.button(button_label).clicked() {
                        closing = true;
                    }
                });
            });

        // Recoverable のみ backdrop クリック / Esc を「閉じる」として受け付ける。
        // Terminal は backdrop / Esc も無効 (= ボタンクリックでしか抜けられない)。
        if kind == ResultKind::FailedRecoverable && response.should_close() {
            closing = true;
        }

        if closing {
            self.settings_restore_state.result = None;
            match kind {
                ResultKind::FailedRecoverable => {
                    // 状態は無傷、アプリ続行可。ダイアログを閉じるだけ。
                    self.show_settings_restore = false;
                    self.settings_restore_state = SettingsRestoreState::default();
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
                    self.shutdown_requested
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
    ui.label(
        "現在の設定または過去のバックアップから設定を復元できます。\n\
         復元すると現在の設定は上書きされ、アプリは自動で終了します。",
    );
    ui.add_space(8.0);

    // 一覧テーブル。
    let now = SystemTime::now();
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            egui::Grid::new("settings_restore_grid")
                .num_columns(7)
                .striped(true)
                .spacing(egui::vec2(12.0, 4.0))
                .show(ui, |ui| {
                    // ヘッダ
                    ui.strong("世代");
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
                            BackupSource::Bak(_) => {
                                if ui.button("この時点に戻す…").clicked() {
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
