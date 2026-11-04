//! Independent AI integration window and worker-only registration lifecycle.
use crate::agent_ipc::changes::ChangeRecord;
use crate::agent_registration::{self as backend, Action, AppTarget, RegistrationState};
use crate::app::App;
use crate::settings::AgentLevel;
use crate::settings::Settings;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PermissionSelection {
    level: AgentLevel,
    thumbnails: bool,
    gps: bool,
    ai_captions: bool,
}

impl From<&Settings> for PermissionSelection {
    fn from(s: &Settings) -> Self {
        Self {
            level: s.agent_level,
            thumbnails: s.agent_show_thumbnails,
            gps: s.agent_show_gps,
            ai_captions: s.agent_show_ai_captions,
        }
    }
}

enum DetectionState {
    Unchecked,
    Checking(
        mpsc::Receiver<Result<(backend::RegistrationEnvironment, backend::Detection), String>>,
    ),
    Ready(backend::RegistrationEnvironment, backend::Detection),
    Failed(String),
}

/// One owner from opening confirmation until the write result is adopted.
enum RegistrationOperation {
    Confirming {
        env: backend::RegistrationEnvironment,
        request: backend::WriteRequest,
        opened_frame: u64,
    },
    Writing {
        target: AppTarget,
        rx: mpsc::Receiver<Result<String, String>>,
    },
}

#[cfg(test)]
struct MockBackend {
    env: backend::RegistrationEnvironment,
    detection: backend::Detection,
    write_result: Result<String, String>,
    writes: usize,
}

pub(crate) struct AgentIntegrationState {
    detection: DetectionState,
    operation: Option<RegistrationOperation>,
    feedback: Option<(AppTarget, Result<String, String>)>,
    instructions: Option<AppTarget>,
    recent: Vec<(String, String, String)>,
    changes: Vec<Arc<ChangeRecord>>,
    #[cfg(test)]
    mock_backend: Option<MockBackend>,
    #[cfg(test)]
    dialog_scroll: Option<crate::test_ui::scroll_layout::ScrollLayout>,
    #[cfg(test)]
    pub(super) page_scroll: Option<crate::test_ui::scroll_layout::ScrollLayout>,
}

impl AgentIntegrationState {
    fn new() -> Self {
        Self {
            detection: DetectionState::Unchecked,
            operation: None,
            feedback: None,
            instructions: None,
            recent: Vec::new(),
            changes: Vec::new(),
            #[cfg(test)]
            mock_backend: None,
            #[cfg(test)]
            dialog_scroll: None,
            #[cfg(test)]
            page_scroll: None,
        }
    }

    fn start_detection(&mut self, ctx: &egui::Context) {
        if self.operation.is_some() || matches!(self.detection, DetectionState::Checking(_)) {
            return;
        }
        #[cfg(test)]
        assert!(
            self.mock_backend.is_some(),
            "headless AI registration detection requires an explicitly injected mock backend"
        );
        let (tx, rx) = mpsc::channel();
        self.detection = DetectionState::Checking(rx);
        #[cfg(test)]
        if let Some(mock) = &self.mock_backend {
            let _ = tx.send(Ok((mock.env.clone(), mock.detection.clone())));
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            return;
        }
        let repaint = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("agent-registration-detect".into())
            .spawn(move || {
                let result = backend::RegistrationEnvironment::capture().map(|env| {
                    let detection = backend::detect(&env);
                    (env, detection)
                });
                let _ = tx.send(result);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let DetectionState::Checking(rx) = &self.detection {
            match rx.try_recv() {
                Ok(Ok((env, result))) => self.detection = DetectionState::Ready(env, result),
                Ok(Err(error)) => self.detection = DetectionState::Failed(error),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.detection = DetectionState::Failed("確認に失敗しました".into())
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after_for(
                    std::time::Duration::from_millis(100),
                    egui::ViewportId::ROOT,
                ),
            }
        }
        if let Some(RegistrationOperation::Writing { target, rx }) = &self.operation {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("登録の処理に失敗しました".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after_for(
                        std::time::Duration::from_millis(100),
                        egui::ViewportId::ROOT,
                    );
                    None
                }
            };
            if let Some(result) = result {
                self.feedback = Some((*target, result));
                self.operation = None;
                self.start_detection(ctx);
            }
        }
    }

    fn confirm(&mut self, target: AppTarget, action: Action, opened_frame: u64) {
        if self.operation.is_some() {
            return;
        }
        let DetectionState::Ready(env, detection) = &self.detection else {
            return;
        };
        let Some(row) = detection.apps.iter().find(|row| row.target == target) else {
            return;
        };
        let request = match action {
            Action::Register => detection
                .command
                .as_ref()
                .ok()
                .and_then(|command| row.write_request(action, command)),
            Action::Remove => row.remove_request(),
        };
        if let Some(request) = request {
            self.operation = Some(RegistrationOperation::Confirming {
                env: env.clone(),
                request,
                opened_frame,
            });
        }
    }

    fn submit(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        assert!(
            self.mock_backend.is_some(),
            "headless AI registration writes require an explicitly injected mock backend"
        );
        let Some(RegistrationOperation::Confirming { env, request, .. }) = self.operation.take()
        else {
            return;
        };
        let target = request.target;
        let (tx, rx) = mpsc::channel();
        self.operation = Some(RegistrationOperation::Writing { target, rx });
        #[cfg(test)]
        if let Some(mock) = &mut self.mock_backend {
            mock.writes += 1;
            let _ = tx.send(mock.write_result.clone());
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            return;
        }
        let repaint = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("agent-registration-write".into())
            .spawn(move || {
                let result = backend::write(&env, &request);
                if let Err(error) = &result {
                    crate::logger::log(format!("[agent-registration] {error}"));
                }
                let _ = tx.send(result);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            });
    }
}

fn draw_permission(ui: &mut egui::Ui, settings: &mut PermissionSelection) -> egui::Response {
    let response = egui::ComboBox::from_id_salt("agent_level")
        .selected_text(level_label(settings.level))
        .show_ui(ui, |ui| {
            for level in [
                AgentLevel::Disabled,
                AgentLevel::ReadOnly,
                AgentLevel::ReadAndOpen,
                AgentLevel::Organize,
            ] {
                ui.selectable_value(&mut settings.level, level, level_label(level));
            }
        });
    ui.add_space(8.0);
    response.response
}

fn level_label(level: AgentLevel) -> &'static str {
    match level {
        AgentLevel::Disabled => "無効",
        AgentLevel::ReadOnly => "読み取りのみ",
        AgentLevel::ReadAndOpen => "読み取りと、mIV で開く",
        AgentLevel::Organize => "整理まで: タグ・★・コレクションの変更",
    }
}

fn draw_images(ui: &mut egui::Ui, settings: &mut PermissionSelection) {
    ui.checkbox(&mut settings.thumbnails, "縮小画像を AI に見せる");
    ui.checkbox(&mut settings.gps, "撮影位置 (GPS) を AI に見せる");
    ui.checkbox(&mut settings.ai_captions, "AI 説明文を AI に見せる");
    ui.label("ON にすると、AI アプリが AI 説明文 (説明文・画像内の文字・検索語) で検索でき、内容も受け取ります");
    ui.label("画像内の文字には、氏名・連絡先などが含まれる場合があります。");
    ui.label(
        "OFF でも、名前での検索結果から、本に AI 説明文があることを推測できる場合があります。",
    );
    ui.add_space(8.0);
}

#[derive(Debug)]
struct Exposure {
    shown: Vec<&'static str>,
    hidden: Vec<&'static str>,
    actions: Vec<&'static str>,
}

fn exposure(draft: PermissionSelection) -> Exposure {
    let mut result = Exposure {
        shown: Vec::new(),
        hidden: vec![
            "画像・動画・音声の元データ",
            "PDF / ZIP の本文そのもの (ページの画像や文章を直接取り出すこと)",
            "mIV の設定・PDF のパスワード・リモート接続の暗証番号",
        ],
        actions: Vec::new(),
    };
    if draft.level == AgentLevel::Disabled {
        result.shown.push("なし (AI 連携は無効です)");
        result
            .hidden
            .push("ファイルの情報・縮小画像・撮影位置・AI 説明文を含む、すべての情報");
        result.actions.push("なし");
        return result;
    }
    result.shown.extend([
        "ファイルの場所 (絶対パス)・名前・種類・大きさ・寸法・日時",
        "タグ・★・コレクション・本棚・閲覧履歴",
        "撮影日時・機種などの EXIF、AI 生成画像の生成情報 (PNG tEXt チャンク・EXIF UserComment)",
    ]);
    if draft.thumbnails {
        result.shown.push("縮小画像 (補正・編集は反映しない。隠す編集をした画像は渡さない。動画・書庫の代表は渡さない)");
    } else {
        result.hidden.push("縮小画像");
    }
    if draft.gps {
        result.shown.push("撮影位置 (GPS)");
    } else {
        result.hidden.push("撮影位置 (GPS)");
    }
    if draft.ai_captions {
        result.shown.push("AI 説明文 (説明文・画像内の文字・検索語。表紙に固定した本文のページや ZIP の中の画像から読み取った文字も含む)");
    } else {
        result.hidden.push("AI 説明文");
    }
    result.actions.push("情報の読み取り・検索");
    if draft.level >= AgentLevel::ReadAndOpen {
        result.actions.push("mIV の画面で開く (表示が切り替わる)");
    }
    if draft.level >= AgentLevel::Organize {
        result
            .actions
            .push("タグ・★・コレクションの変更 (確認なし。1 回 500 件まで)");
    }
    result
}

fn draw_exposure(ui: &mut egui::Ui, settings: &PermissionSelection) {
    let summary = exposure(*settings);
    ui.columns(3, |columns| {
        for (column, (title, lines)) in columns.iter_mut().zip([
            ("AI に見せるもの", &summary.shown),
            ("見せないもの", &summary.hidden),
            ("AI に許す操作", &summary.actions),
        ]) {
            column.with_layout(egui::Layout::top_down(egui::Align::Min), |column| {
                column.label(egui::RichText::new(title).strong());
                for line in lines {
                    column.add(egui::Label::new(*line).wrap().selectable(true));
                    column.add_space(4.0);
                }
            });
        }
    });
    ui.add_space(6.0);
    ui.label("この情報は、お使いの AI アプリ (Claude / ChatGPT) の提供元へ送られます。各社のプライバシーポリシーに従って扱われます。mIV の開発者には送られません");
    ui.colored_label(
        ui.visuals().warn_fg_color,
        "許可の範囲の中の操作は、AI アプリの確認なしで実行されます。ここで選ぶ範囲が唯一の上限です",
    );
    ui.add_space(12.0);
}

pub(super) fn draw_recent(ui: &mut egui::Ui, state: &AgentIntegrationState) {
    ui.label(egui::RichText::new("AI アプリが名乗った名前です。mIV を閉じると消えます。").weak());
    if state.recent.is_empty() {
        ui.label("まだ使われていません");
    } else {
        let _ = draw_recent_table(ui, &state.recent);
    }
    ui.add_space(12.0);
}

pub(super) fn draw_changes(ui: &mut egui::Ui, state: &AgentIntegrationState) {
    draw_changes_content(ui, state, false);
}

fn draw_changes_content(ui: &mut egui::Ui, state: &AgentIntegrationState, expanded: bool) {
    ui.label(egui::RichText::new("mIV を閉じると消えます。最新 100 回分を表示します。").weak());
    if state.changes.is_empty() {
        ui.label("変更の記録はありません");
    }
    for record in &state.changes {
        let seconds = record
            .at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let heading = format!(
            "{}  {}  {} ({} 件)  {}",
            crate::app::format_details_timestamp(seconds, true),
            record.client_name,
            record.tool,
            record.counts.total(),
            record.summary
        );
        let id = ui.make_persistent_id(("agent_change", record.id));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, expanded)
            .show_header(ui, |ui| {
                ui.add(egui::Label::new(heading).wrap().selectable(true));
            })
            .body(|ui| {
                if let Some(target) = collection_target_text(record) {
                    ui.add(egui::Label::new(target).wrap().selectable(true));
                }
                ui.add(
                    egui::Label::new(format!(
                        "変更 {} 件 / 変更なし {} 件 / 対象外 {} 件 / 処理中 {} 件 / 失敗 {} 件",
                        record.counts.changed,
                        record.counts.unchanged,
                        record.counts.skipped,
                        record.counts.queued,
                        record.counts.failed
                    ))
                    .wrap()
                    .selectable(true),
                );
                if ui.small_button("この変更をコピー").clicked() {
                    ui.ctx().copy_text(change_record_text(record));
                }
                for (index, row) in record.rows.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.separator();
                        selectable_copy(ui, &change_row_copy_text(record, row));
                    });
                }
            });
    }
    ui.add_space(12.0);
}

fn change_row_text(row: &crate::agent_ipc::changes::ChangeRow) -> String {
    use crate::agent_ipc::changes::{ChangeRowOutcome, ChangeRowResult};
    let failure;
    let (before, after, result) = match &row.result {
        ChangeRowResult::Pending => ("処理中", "処理中", "処理中"),
        ChangeRowResult::Done(result) => (
            row.before.as_str(),
            row.after.as_str(),
            match result {
                ChangeRowOutcome::Changed => "変更",
                ChangeRowOutcome::Unchanged => "変更なし",
                ChangeRowOutcome::Text(text) => text.as_str(),
            },
        ),
        ChangeRowResult::Failed(error) => {
            failure = format!("失敗: {error}");
            (row.before.as_str(), row.after.as_str(), failure.as_str())
        }
    };
    format!(
        "項目: {}\n前の値: {}\n後の値: {}\n結果: {}",
        row.item, before, after, result
    )
}

fn collection_target_text(record: &ChangeRecord) -> Option<String> {
    record.collection.as_ref().map(|target| {
        format!(
            "コレクション: {}\nコレクション ID: {}",
            target.name, target.id
        )
    })
}

fn change_row_copy_text(
    record: &ChangeRecord,
    row: &crate::agent_ipc::changes::ChangeRow,
) -> String {
    let row = change_row_text(row);
    match collection_target_text(record) {
        Some(target) => format!("{target}\n{row}"),
        None => row,
    }
}

fn change_record_text(record: &ChangeRecord) -> String {
    let mut text = format!(
        "{} / {} / {} 件 / {}",
        record.client_name,
        record.tool,
        record.counts.total(),
        record.summary
    );
    if let Some(target) = collection_target_text(record) {
        text.push('\n');
        text.push_str(&target);
    }
    for row in &record.rows {
        text.push_str("\n\n");
        text.push_str(&change_row_text(row));
    }
    text
}

/// Each row receives the full page width, and advances by its tallest natural
/// cell. Never derive column widths from a Grid cell's remaining width.
fn draw_recent_table(
    ui: &mut egui::Ui,
    recent: &[(String, String, String)],
) -> Vec<[egui::Rect; 3]> {
    ui.columns(3, |columns| {
        for (column, title) in columns.iter_mut().zip(["名前", "版", "最後に使われた時刻"])
        {
            column.strong(title);
        }
    });
    recent
        .iter()
        .map(|(name, version, time)| {
            ui.columns(3, |columns| {
                let mut cells = [egui::Rect::NOTHING; 3];
                for (index, (column, text)) in
                    columns.iter_mut().zip([name, version, time]).enumerate()
                {
                    cells[index] = column
                        .add(egui::Label::new(text).wrap().selectable(true))
                        .rect;
                }
                cells
            })
        })
        .collect()
}

fn selectable_copy(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap().selectable(true));
    if ui.small_button("コピー").clicked() {
        ui.ctx().copy_text(text.to_owned());
    }
}

fn draw_app(
    ui: &mut egui::Ui,
    settings: &PermissionSelection,
    state: &mut AgentIntegrationState,
    target: AppTarget,
) {
    ui.add_enabled_ui(state.operation.is_none(), |ui| {
    ui.push_id(target.name(), |ui| {
        let mut requested = None;
        match &state.detection {
            DetectionState::Unchecked | DetectionState::Checking(_) => { ui.label("確認中..."); }
            DetectionState::Failed(error) => { ui.colored_label(ui.visuals().error_fg_color, error); }
            DetectionState::Ready(_, detection) => {
                if let Some(row) = detection.apps.iter().find(|row| row.target == target) {
                    ui.label(match row.state {
                        RegistrationState::NotFound => "見つからない", RegistrationState::Unregistered => "未登録",
                        RegistrationState::Registered => "登録済み", RegistrationState::DifferentTarget => "登録先が違う",
                        RegistrationState::Unreadable => "設定ファイルを読めない", RegistrationState::Ambiguous => "自動登録できません",
                    });
                    if let Some(message) = &row.message { ui.colored_label(ui.visuals().warn_fg_color, message); }
                    if let Err(error) = &detection.command { ui.colored_label(ui.visuals().error_fg_color, error); }
                    let can_write = detection.command.is_ok() && row.can_write(Action::Register);
                    ui.horizontal_wrapped(|ui| {
                        let label = if row.state == RegistrationState::DifferentTarget { "登録し直す" } else { "登録する" };
                        if ui.add_enabled(can_write, egui::Button::new(label)).clicked() { requested = Some(Action::Register); }
                        if ui.add_enabled(row.can_remove(), egui::Button::new("登録を外す")).clicked() { requested = Some(Action::Remove); }
                    });
                }
            }
        }
        if settings.level == AgentLevel::Disabled { ui.label(egui::RichText::new("登録しても、許可の範囲を選ぶまで AI アプリからは使えません").weak()); }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!matches!(state.detection, DetectionState::Checking(_)), egui::Button::new("再確認")).clicked() { state.start_detection(ui.ctx()); }
            if ui.button("手順を見る").clicked() { state.instructions = if state.instructions == Some(target) { None } else { Some(target) }; }
            if target == AppTarget::ChatGpt && ui.button("設定を開く").clicked() {
                // The only non-http URL accepted here is this fixed settings entry.
                if let Err(error) = opener::open("codex://settings") { state.feedback = Some((target, Err(format!("設定を開けませんでした: {error}")))); }
            }
        });
        if let Some(action) = requested { state.confirm(target, action, ui.ctx().cumulative_frame_nr()); }
        if let Some((owner, result)) = &state.feedback {
            if *owner == target { match result { Ok(message) => { ui.label(message); }, Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error); } } }
        }
        if state.instructions == Some(target) {
            ui.label(match target {
                AppTarget::ClaudeDesktop => "Claude を終了 → 設定ファイルに追記 → Claude を起動。初回の確認画面で「常に許可」を選んでください。",
                AppTarget::ChatGpt => "Settings > MCP servers > Add server > STDIO で登録し、再起動してください。",
            });
            if let DetectionState::Ready(_, detection) = &state.detection {
                if let Some(row) = detection.apps.iter().find(|row| row.target == target) {
                    for path in &row.config_paths { selectable_copy(ui, &path.display().to_string()); }
                }
                if let Ok(command) = &detection.command { selectable_copy(ui, &command.snippet(target)); }
            }
        }
        ui.add_space(12.0);
    });
    });
}

impl App {
    pub(crate) fn open_agent_integration_dialog(&mut self) {
        if self.agent_integration_dialog.is_none() {
            self.agent_integration_dialog = Some(AgentIntegrationState::new());
        }
    }

    pub(crate) fn agent_registration_dialog_open(&self) -> bool {
        self.agent_integration_dialog
            .as_ref()
            .is_some_and(|state| state.operation.is_some())
    }

    pub(crate) fn show_agent_integration_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.agent_integration_dialog.take() else {
            return;
        };
        state.poll(ctx);
        if let Some(changes) = &self.agent_changes {
            let shared = changes.controller.records();
            if let Ok(records) = shared.try_lock() {
                state.changes = records.entries().to_vec();
            }
        }
        if matches!(state.detection, DetectionState::Unchecked) {
            state.start_detection(ctx);
        }
        if let Some(recent) = self
            .agent_recent_apps
            .as_ref()
            .and_then(|apps| apps.try_lock().ok())
        {
            state.recent = recent
                .entries()
                .iter()
                .map(|entry| {
                    let seconds = entry
                        .last_called
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    (
                        entry.client_name.clone(),
                        entry.client_version.clone(),
                        crate::app::format_details_timestamp(seconds, true),
                    )
                })
                .collect();
        }
        let enter = self.dialog_enter_pressed(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let ime_active = self.ime_input_active(ctx);
        let mut selected = PermissionSelection::from(&self.settings);
        let open = draw_window(ctx, &mut selected, &mut state, escape, false);
        if selected != PermissionSelection::from(&self.settings) {
            self.commit_agent_permissions(selected);
        }
        draw_registration_dialog(ctx, &mut state, enter, escape, ime_active);
        if open || state.operation.is_some() {
            self.agent_integration_dialog = Some(state);
        }
    }

    fn commit_agent_permissions(&mut self, permission: PermissionSelection) {
        // save_checked is the same DB transaction/permission epoch boundary as
        // Preferences OK. Adopt only after success, so failure restores the UI.
        let mut candidate = self.settings.clone();
        candidate.agent_level = permission.level;
        candidate.agent_show_thumbnails = permission.thumbnails;
        candidate.agent_show_gps = permission.gps;
        candidate.agent_show_ai_captions = permission.ai_captions;
        if candidate.save_checked() {
            self.settings = candidate;
        } else {
            crate::logger::log("agent integration: permission settings save failed");
            self.show_feedback_warning(
                "AI 連携の設定を保存できませんでした。保存済みの値に戻しました。".into(),
            );
        }
    }
}

fn panel_scroll_style() -> egui::style::ScrollStyle {
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 10.0;
    scroll.bar_inner_margin = 8.0;
    scroll.bar_outer_margin = 2.0;
    scroll.foreground_color = true;
    scroll
}

fn draw_body(
    ui: &mut egui::Ui,
    settings: &mut PermissionSelection,
    state: &mut AgentIntegrationState,
    expanded: bool,
) {
    let label = ui.label(egui::RichText::new("許可の範囲").strong());
    draw_permission(ui, settings).labelled_by(label.id);
    ui.label(egui::RichText::new("縮小画像・撮影位置・AI 説明文").strong());
    draw_images(ui, settings);
    ui.label(egui::RichText::new("AI に見せる範囲").strong());
    draw_exposure(ui, settings);
    ui.label(egui::RichText::new("最近使った AI アプリ").strong());
    draw_recent(ui, state);
    ui.label(egui::RichText::new("AI による変更").strong());
    if expanded {
        draw_changes_content(ui, state, true);
    } else {
        draw_changes(ui, state);
    }
    for target in [AppTarget::ClaudeDesktop, AppTarget::ChatGpt] {
        ui.label(egui::RichText::new(target.name()).strong());
        draw_app(ui, settings, state, target);
    }
}

fn draw_window(
    ctx: &egui::Context,
    settings: &mut PermissionSelection,
    state: &mut AgentIntegrationState,
    escape: bool,
    expanded: bool,
) -> bool {
    let blocked = state.operation.is_some();
    if blocked {
        egui::Popup::close_all(ctx);
    }
    let mut open = true;
    let screen = ctx.content_rect();
    egui::Window::new("AI 連携")
        .id(egui::Id::new("agent_integration_dialog"))
        .drag_area(egui::WindowDrag::Anywhere)
        .default_pos(screen.min + egui::vec2(60.0, 40.0))
        .default_width(860.0)
        .default_height((screen.height() - 100.0).max(100.0))
        .max_size((screen.size() - egui::vec2(80.0, 80.0)).max(egui::vec2(100.0, 100.0)))
        .resizable(true)
        .collapsible(false)
        .enabled(!blocked)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.spacing_mut().scroll = panel_scroll_style();
            let output = egui::ScrollArea::vertical()
                .id_salt("agent_integration_body")
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Always,
                    ..Default::default()
                })
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    draw_body(ui, settings, state, expanded);
                });
            #[cfg(test)]
            {
                state.page_scroll = Some(crate::test_ui::scroll_layout::ScrollLayout::from_output(
                    &output,
                ));
            }
            #[cfg(not(test))]
            let _ = output;
        });
    blocked || state.operation.is_some() || (open && !escape)
}

fn draw_registration_dialog(
    ctx: &egui::Context,
    state: &mut AgentIntegrationState,
    enter: bool,
    escape: bool,
    ime_active: bool,
) {
    let Some(operation) = &state.operation else {
        return;
    };
    egui::Popup::close_all(ctx);
    let writing = matches!(operation, RegistrationOperation::Writing { .. });
    // A sizing/layout pass can show the new window again within the opening
    // frame. Its initiating key must never double as confirmation/cancellation.
    let keyboard_allowed = matches!(operation,
        RegistrationOperation::Confirming { opened_frame, .. }
        if *opened_frame != ctx.cumulative_frame_nr());
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    #[cfg(test)]
    let mut measured_scroll = None;
    let screen = ctx.content_rect();
    egui::Window::new("AI アプリへの登録の確認")
        .id(egui::Id::new("agent_registration_confirmation"))
        .order(egui::Order::Foreground)
        .default_pos(screen.min + egui::vec2(24.0, 24.0))
        .default_width((screen.width() - 80.0).clamp(160.0, 540.0))
        .default_height((screen.height() - 100.0).max(100.0))
        .max_size((screen.size() - egui::vec2(48.0, 48.0)).max(egui::vec2(100.0, 100.0)))
        .collapsible(false)
        .resizable(true)
        .enabled(!writing)
        .open(&mut open)
        .show(ctx, |ui| {
            let footer =
                super::startup_dialog_footer_height(ui, &["書き換える", "キャンセル"], 0.0);
            // Use the viewport, never this resizable Window's previous content height.
            // Reserving the measured footer prevents a body/footer sizing feedback loop.
            let frame = egui::Frame::window(ui.style()).total_margin();
            let body_height =
                (screen.bottom() - 24.0 - frame.bottom - ui.cursor().top() - footer).max(1.0);
            ui.spacing_mut().scroll =
                super::non_overlapping_dialog_scroll_style(panel_scroll_style());
            let output = egui::ScrollArea::vertical()
                .id_salt("agent_registration_body")
                .max_height(body_height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match operation {
                        RegistrationOperation::Confirming { request, .. } => {
                            ui.label(format!(
                                "{} の設定を書き換えます。アプリを終了してから進めてください。",
                                request.target.name()
                            ));
                            selectable_copy(ui, &request.path.display().to_string());
                            if request.action == Action::Remove {
                                ui.label(
                                    "mimageviewer の登録項目を消します。他の項目は変更しません。",
                                );
                            }
                            if !request.preview.removed.is_empty() {
                                ui.label("次の内容を削除します。");
                                selectable_copy(ui, &request.preview.removed);
                            }
                            if !request.preview.added.is_empty() {
                                ui.label("次の内容を登録します。");
                                selectable_copy(ui, &request.preview.added);
                            }
                            if request.preview.removed.is_empty()
                                && request.preview.added.is_empty()
                            {
                                ui.label("登録内容の変更はありません。");
                            }
                            ui.label("書き換え前の内容は .mimageviewer.bak に残します。");
                        }
                        RegistrationOperation::Writing { .. } => {
                            ui.label("設定を書き換えています...");
                        }
                    }
                });
            #[cfg(test)]
            {
                measured_scroll = Some(crate::test_ui::scroll_layout::ScrollLayout::from_output(
                    &output,
                ));
            }
            #[cfg(not(test))]
            let _ = output;
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                let confirm = ui.button("書き換える");
                if registration_action_requested(&confirm, enter, keyboard_allowed, ime_active) {
                    submit = true;
                }
                let cancel_button = ui.button("キャンセル");
                if registration_action_requested(
                    &cancel_button,
                    escape,
                    keyboard_allowed,
                    ime_active,
                ) {
                    cancel = true;
                }
            });
        });
    #[cfg(test)]
    {
        state.dialog_scroll = measured_scroll;
    }
    if !writing && (cancel || !open) {
        state.operation = None;
    } else if submit {
        state.submit(ctx);
    }
}

fn registration_action_requested(
    response: &egui::Response,
    dialog_key: bool,
    keyboard_allowed: bool,
    ime_active: bool,
) -> bool {
    let focus_allows_action = response
        .ctx
        .memory(|memory| memory.focused().is_none_or(|id| id == response.id));
    // As in first_setup_should_confirm, preserve pointer/accessibility clicks
    // while rejecting egui's synthesized Enter/Space click during composition.
    let ime_keyboard_click = ime_active
        && response.ctx.input(|input| {
            input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
        });
    response.clicked_by(egui::PointerButton::Primary)
        || (keyboard_allowed && response.clicked() && !ime_keyboard_click)
        || (keyboard_allowed && focus_allows_action && dialog_key)
}

/// Fixed confirmation preview; no environment capture, filesystem access or workers.
#[doc(hidden)]
pub fn draw_agent_registration_snapshot_fixture(ui: &mut egui::Ui) {
    let mut state = AgentIntegrationState::new();
    state.operation = Some(RegistrationOperation::Confirming {
        env: backend::RegistrationEnvironment {
            local_app_data: None, app_data: None, user_profile: None,
            codex_home: None, launcher_exe: None,
            current_exe: r"C:\mImageViewer\mimageviewer-core.exe".into(),
            data_dir: r"C:\mImageViewer\data".into(),
            working_dir: r"C:\mImageViewer".into(),
        },
        request: backend::WriteRequest {
            target: AppTarget::ClaudeDesktop,
            path: r"C:\Users\User\AppData\Roaming\Claude\claude_desktop_config.json".into(),
            action: Action::Register,
            command: backend::RegistrationCommand { command: String::new(), args: Vec::new() },
            preview: backend::RegistrationPreview {
                removed: String::new(),
                added: "{\n  \"mcpServers\": {\n    \"mimageviewer\": {\n      \"command\": \"C:/mImageViewer/mimageviewer-mcp.exe\"\n    }\n  }\n}".into(),
            },
        },
        opened_frame: ui.ctx().cumulative_frame_nr(),
    });
    draw_registration_dialog(ui.ctx(), &mut state, false, false, false);
}

/// No environment capture, filesystem access, process enumeration, or workers.
pub fn draw_agent_integration_snapshot_fixture(ui: &mut egui::Ui, level: u8) {
    let mut settings = PermissionSelection {
        level: AgentLevel::Disabled,
        thumbnails: false,
        gps: false,
        ai_captions: false,
    };
    settings.level = match level {
        0 => AgentLevel::Disabled,
        1 => AgentLevel::ReadOnly,
        2 => AgentLevel::ReadAndOpen,
        _ => AgentLevel::Organize,
    };
    settings.thumbnails = level >= 2;
    settings.gps = level >= 2;
    settings.ai_captions = level >= 2;
    let command = backend::RegistrationCommand {
        command: r"C:\mImageViewer\mimageviewer-mcp.exe".into(),
        args: vec!["--data-dir".into(), r"C:\mImageViewer\data".into()],
    };
    let mut state = AgentIntegrationState::new();
    state.detection = DetectionState::Ready(
        backend::RegistrationEnvironment {
            local_app_data: None,
            app_data: None,
            user_profile: None,
            codex_home: None,
            launcher_exe: None,
            current_exe: PathBuf::from(r"C:\mImageViewer\mimageviewer-core.exe"),
            data_dir: PathBuf::from(r"C:\mImageViewer\data"),
            working_dir: PathBuf::from(r"C:\mImageViewer"),
        },
        backend::Detection {
            command: Ok(command.clone()),
            apps: vec![
                backend::AppRegistration {
                    target: AppTarget::ClaudeDesktop,
                    state: RegistrationState::Registered,
                    config_paths: vec![PathBuf::from(
                        r"C:\Users\User\AppData\Roaming\Claude\claude_desktop_config.json",
                    )],
                    message: None,
                    previews: backend::RegistrationPreviews::from_config(
                        AppTarget::ClaudeDesktop,
                        &command.snippet(AppTarget::ClaudeDesktop),
                        Some(&command),
                    )
                    .unwrap(),
                },
                backend::AppRegistration {
                    target: AppTarget::ChatGpt,
                    state: RegistrationState::Unregistered,
                    config_paths: vec![PathBuf::from(r"C:\Users\User\.codex\config.toml")],
                    message: None,
                    previews: backend::RegistrationPreviews::from_config(
                        AppTarget::ChatGpt,
                        "",
                        Some(&command),
                    )
                    .unwrap(),
                },
            ],
        },
    );
    state.recent.push((
        "Claude Desktop".into(),
        "1.0".into(),
        "2026-11-01 12:34:56".into(),
    ));
    if level >= 4 {
        let controller = crate::agent_ipc::changes::ChangeController::new();
        controller.publish(crate::agent_ipc::changes::ChangeRecord {
            id: Default::default(),
            at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_501_696),
            client_name: "Claude Desktop".into(),
            tool: "add_to_collection".into(),
            collection: Some(crate::agent_ipc::changes::CollectionTarget {
                id: "collection-travel".into(),
                name: "旅行の写真".into(),
            }),
            counts: crate::agent_ipc::changes::ChangeCounts {
                changed: 2,
                unchanged: 1,
                skipped: 1,
                ..Default::default()
            },
            summary: "コレクション「旅行の写真」に追加".into(),
            rows: vec![crate::agent_ipc::changes::ChangeRow {
                item: r"C:\写真\旅行\海辺の写真.jpg".into(),
                before: "未登録".into(),
                after: "旅行の写真".into(),
                result: crate::agent_ipc::changes::ChangeRowResult::Done(
                    crate::agent_ipc::changes::ChangeRowOutcome::Text("変更しました".into()),
                ),
            }],
            notice: String::new(),
        });
        use crate::agent_ipc::changes::{ChangeRow, ChangeRowOutcome, ChangeRowResult};
        let id = controller.records().lock().unwrap().entries()[0].id;
        controller.update(id, |record| {
            record.counts = crate::agent_ipc::changes::ChangeCounts {
                changed: 1,
                unchanged: 1,
                queued: 1,
                failed: 1,
                ..Default::default()
            };
            record.rows = [
                ("海辺.jpg", ChangeRowResult::Done(ChangeRowOutcome::Changed)),
                ("山.jpg", ChangeRowResult::Done(ChangeRowOutcome::Unchanged)),
                ("空.jpg", ChangeRowResult::Pending),
                (
                    "川.jpg",
                    ChangeRowResult::Failed("保存できませんでした".into()),
                ),
            ]
            .into_iter()
            .map(|(name, result)| ChangeRow {
                item: format!(r"C:\写真\旅行\{name}"),
                before: "未登録".into(),
                after: "旅行の写真".into(),
                result,
            })
            .collect();
        });
        state.changes = controller.records().lock().unwrap().entries().to_vec();
    }
    draw_window(ui.ctx(), &mut settings, &mut state, false, level >= 4);
}

#[cfg(test)]
mod tests;
