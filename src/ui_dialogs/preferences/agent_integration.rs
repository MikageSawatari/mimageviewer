//! AI integration view and its draft-scoped, worker-only registration lifecycle.
use super::*;
use crate::agent_registration::{self as backend, Action, AppTarget, RegistrationState};
use crate::settings::AgentLevel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PermissionDraft {
    level: AgentLevel,
    thumbnails: bool,
    gps: bool,
}

impl From<&Settings> for PermissionDraft {
    fn from(s: &Settings) -> Self {
        Self {
            level: s.agent_level,
            thumbnails: s.agent_show_thumbnails,
            gps: s.agent_show_gps,
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

pub(super) struct AgentIntegrationState {
    applied: PermissionDraft,
    detection: DetectionState,
    operation: Option<RegistrationOperation>,
    feedback: Option<(AppTarget, Result<String, String>)>,
    instructions: Option<AppTarget>,
    recent: Vec<(String, String, String)>,
    #[cfg(test)]
    mock_backend: Option<MockBackend>,
    #[cfg(test)]
    dialog_scroll: Option<crate::test_ui::scroll_layout::ScrollLayout>,
    #[cfg(test)]
    pub(super) page_scroll: Option<crate::test_ui::scroll_layout::ScrollLayout>,
}

impl AgentIntegrationState {
    pub(super) fn new(settings: &Settings) -> Self {
        Self {
            applied: settings.into(),
            detection: DetectionState::Unchecked,
            operation: None,
            feedback: None,
            instructions: None,
            recent: Vec::new(),
            #[cfg(test)]
            mock_backend: None,
            #[cfg(test)]
            dialog_scroll: None,
            #[cfg(test)]
            page_scroll: None,
        }
    }

    fn draft_changed(&self, settings: &Settings) -> bool {
        self.applied != settings.into()
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
            ctx.request_repaint();
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
                repaint.request_repaint();
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
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
            }
        }
        if let Some(RegistrationOperation::Writing { target, rx }) = &self.operation {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("登録の処理に失敗しました".into()))
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
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

    fn confirm(
        &mut self,
        settings: &Settings,
        target: AppTarget,
        action: Action,
        opened_frame: u64,
    ) {
        if self.draft_changed(settings) || self.operation.is_some() {
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
            ctx.request_repaint();
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
                repaint.request_repaint();
            });
    }
}

pub(super) fn draw_permission(ui: &mut egui::Ui, settings: &mut Settings) -> egui::Response {
    let response = egui::ComboBox::from_id_salt("agent_level")
        .selected_text(level_label(settings.agent_level))
        .show_ui(ui, |ui| {
            for level in [
                AgentLevel::Disabled,
                AgentLevel::ReadOnly,
                AgentLevel::ReadAndOpen,
            ] {
                ui.selectable_value(&mut settings.agent_level, level, level_label(level));
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
    }
}

pub(super) fn draw_images(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.checkbox(
        &mut settings.agent_show_thumbnails,
        "縮小画像を AI に見せる",
    );
    ui.checkbox(
        &mut settings.agent_show_gps,
        "撮影位置 (GPS) を AI に見せる",
    );
    ui.add_space(8.0);
}

#[derive(Debug)]
struct Exposure {
    shown: Vec<&'static str>,
    hidden: Vec<&'static str>,
    actions: Vec<&'static str>,
}

fn exposure(draft: PermissionDraft) -> Exposure {
    let mut result = Exposure {
        shown: Vec::new(),
        hidden: vec![
            "画像・動画・音声の元データ",
            "PDF / ZIP の中身の本文",
            "mIV の設定・PDF のパスワード・リモート接続の暗証番号",
        ],
        actions: Vec::new(),
    };
    if draft.level == AgentLevel::Disabled {
        result.shown.push("なし (AI 連携は無効です)");
        result
            .hidden
            .push("ファイルの情報・縮小画像・撮影位置を含む、すべての情報");
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
    result.actions.push("情報の読み取り・検索");
    if draft.level == AgentLevel::ReadAndOpen {
        result.actions.push("mIV の画面で開く (表示が切り替わる)");
    }
    result
}

pub(super) fn draw_exposure(ui: &mut egui::Ui, settings: &Settings, state: &AgentIntegrationState) {
    if state.draft_changed(settings) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "編集中の設定です。OK で適用されます",
        );
    }
    let summary = exposure(settings.into());
    ui.columns(3, |columns| {
        for (column, (title, lines)) in columns.iter_mut().zip([
            ("AI に見せるもの", &summary.shown),
            ("見せないもの", &summary.hidden),
            ("AI に許す操作", &summary.actions),
        ]) {
            column.label(egui::RichText::new(title).strong());
            for line in lines {
                column.add(egui::Label::new(*line).wrap().selectable(true));
                column.add_space(4.0);
            }
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

pub(super) fn draw_app(
    ui: &mut egui::Ui,
    settings: &Settings,
    state: &mut AgentIntegrationState,
    target: AppTarget,
) {
    ui.add_enabled_ui(state.operation.is_none(), |ui| {
    ui.push_id(target.name(), |ui| {
        ui.label(egui::RichText::new("登録・解除はすぐに実行されます。環境設定の OK / キャンセルとは無関係です。").weak());
        let dirty = state.draft_changed(settings);
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
                    let can_write = !dirty && detection.command.as_ref().ok().is_some_and(|command| row.write_request(Action::Register, command).is_some());
                    ui.horizontal_wrapped(|ui| {
                        let label = if row.state == RegistrationState::DifferentTarget { "登録し直す" } else { "登録する" };
                        if ui.add_enabled(can_write, egui::Button::new(label)).clicked() { requested = Some(Action::Register); }
                        if ui.add_enabled(!dirty && row.remove_request().is_some(), egui::Button::new("登録を外す")).clicked() { requested = Some(Action::Remove); }
                    });
                }
            }
        }
        if dirty { ui.colored_label(ui.visuals().warn_fg_color, "OK で設定を確定してから登録してください"); }
        if state.applied.level == AgentLevel::Disabled { ui.label(egui::RichText::new("登録しても、許可の範囲を選ぶまで AI アプリからは使えません").weak()); }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!matches!(state.detection, DetectionState::Checking(_)), egui::Button::new("再確認")).clicked() { state.start_detection(ui.ctx()); }
            if ui.button("手順を見る").clicked() { state.instructions = if state.instructions == Some(target) { None } else { Some(target) }; }
            if target == AppTarget::ChatGpt && ui.button("設定を開く").clicked() {
                // The only non-http URL accepted here is this fixed settings entry.
                if let Err(error) = opener::open("codex://settings") { state.feedback = Some((target, Err(format!("設定を開けませんでした: {error}")))); }
            }
        });
        if let Some(action) = requested { state.confirm(settings, target, action, ui.ctx().cumulative_frame_nr()); }
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
    pub(crate) fn agent_registration_dialog_open(&self) -> bool {
        self.show_preferences
            && self
                .pref_state
                .as_ref()
                .is_some_and(|state| state.agent_integration.operation.is_some())
    }

    pub(super) fn prepare_agent_integration(&mut self, ctx: &egui::Context) {
        let Some(state) = &mut self.pref_state else {
            return;
        };
        state.agent_integration.applied = (&self.settings).into();
        state.agent_integration.poll(ctx);
        if state.selected == PreferencesPage::AgentIntegration {
            if matches!(state.agent_integration.detection, DetectionState::Unchecked) {
                state.agent_integration.start_detection(ctx);
            }
            state.agent_integration.recent = self
                .agent_recent_apps
                .as_ref()
                .and_then(|apps| apps.try_lock().ok())
                .map(|apps| {
                    apps.entries()
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
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    pub(super) fn show_agent_registration_dialog(&mut self, ctx: &egui::Context) {
        let enter = self.dialog_enter_pressed(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let ime_active = self.ime_input_active(ctx);
        let Some(state) = &mut self.pref_state else {
            return;
        };
        draw_registration_dialog(ctx, &mut state.agent_integration, enter, escape, ime_active);
    }
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
            let button_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            let body_height = (ui.available_height() - button_height - 12.0).max(24.0);
            ui.spacing_mut().scroll = super::pref_panel_scroll_style();
            let output = egui::ScrollArea::vertical()
                .id_salt("agent_registration_body")
                .max_height(body_height)
                .auto_shrink([false, false])
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
                            } else {
                                ui.label("次の内容を登録します。");
                                selectable_copy(ui, &request.command.snippet(request.target));
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

/// No environment capture, filesystem access, process enumeration, or workers.
pub(super) fn snapshot(ui: &mut egui::Ui, level: u8) {
    let mut settings = Settings::default();
    settings.agent_level = match level {
        0 => AgentLevel::Disabled,
        1 => AgentLevel::ReadOnly,
        _ => AgentLevel::ReadAndOpen,
    };
    settings.agent_show_thumbnails = level == 2;
    settings.agent_show_gps = level == 2;
    let mut state = super::PreferencesState::from_settings(
        &settings,
        crate::external_tool::LaunchTarget::None,
        None,
        crate::ai::trt_worker_lifecycle::TrtWorkerLifecycleOwner::new().snapshot(),
        false,
        0,
        0,
        0,
    );
    state.selected = PreferencesPage::AgentIntegration;
    state.agent_integration.detection = DetectionState::Ready(
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
            command: Ok(backend::RegistrationCommand {
                command: r"C:\mImageViewer\mimageviewer-mcp.exe".into(),
                args: vec!["--data-dir".into(), r"C:\mImageViewer\data".into()],
            }),
            apps: vec![
                backend::AppRegistration {
                    target: AppTarget::ClaudeDesktop,
                    state: RegistrationState::Registered,
                    config_paths: vec![PathBuf::from(
                        r"C:\Users\User\AppData\Roaming\Claude\claude_desktop_config.json",
                    )],
                    message: None,
                },
                backend::AppRegistration {
                    target: AppTarget::ChatGpt,
                    state: RegistrationState::Unregistered,
                    config_paths: vec![PathBuf::from(r"C:\Users\User\.codex\config.toml")],
                    message: None,
                },
            ],
        },
    );
    state.agent_integration.recent.push((
        "Claude Desktop".into(),
        "1.0".into(),
        "2026-11-01 12:34:56".into(),
    ));
    super::draw_page(ui, &mut state, false);
}

#[cfg(test)]
mod tests;
