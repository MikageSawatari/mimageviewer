//! Host-owned chrome, file dialogs and decision presentation. No completion admission here.
use super::*;
use egui::emath::GuiRounding;

pub(super) enum ChromeAction {
    NewProject,
    SaveIntent,
    DiscardIntent,
    CancelIntent,
    StopExportAndClose,
    OpenProject,
    SaveProject,
    SaveProjectAs,
    OpenAudio,
    OpenBackground,
    Export,
    ExportMp4,
    ChooseMp4Output,
    TogglePlayback,
    Seek(f64),
    ResetPerf,
    CancelExport,
    CancelResources,
    ShowMissing,
    ShowDiagnostics,
    RetryRestore,
    ReplaceImage(ImageId),
    ApplyProject,
    DiscardProject,
    ApplyLrc,
    DiscardLrc,
}

pub(super) struct ToolbarState {
    can_open_project: bool,
    can_save: bool,
    can_open_material: bool,
    can_export: bool,
    mp4_disabled_reason: Option<&'static str>,
    can_play: bool,
    pub(super) playing: bool,
    pub(super) position_secs: f64,
    pub(super) duration_secs: f64,
}

impl ToolbarState {
    pub(super) fn from_app(app: &MusicVideoLabApp) -> Self {
        Self {
            can_open_project: app.export_state.pending().is_none()
                && app.lrc_save_pending.is_none(),
            can_save: app.project_save_pending.is_none(),
            can_open_material: app.export_state.pending().is_none(),
            can_export: app.audio.is_some()
                && app.assets.has_image(&ImageId::new(BACKGROUND_ID))
                && app.runtime_resources_ready()
                && app.export_state.pending().is_none(),
            mp4_disabled_reason: app.mp4_export_disabled_reason(),
            can_play: app.audio_transport_ready(),
            playing: app.playback_snapshot_for_ui().playing,
            position_secs: app.position_secs,
            duration_secs: app.project.duration_secs,
        }
    }

    pub(super) fn draw(&self, ui: &mut egui::Ui, actions: &mut Vec<ChromeAction>) {
        for (enabled, label, action) in [
            (self.can_open_project, "新規", ChromeAction::NewProject),
            (
                self.can_open_project,
                "プロジェクトを開く",
                ChromeAction::OpenProject,
            ),
            (
                self.can_save,
                "プロジェクトを保存",
                ChromeAction::SaveProject,
            ),
            (
                self.can_save,
                "名前を付けて保存",
                ChromeAction::SaveProjectAs,
            ),
            (
                self.can_open_material,
                "音声を開く",
                ChromeAction::OpenAudio,
            ),
            (
                self.can_open_material,
                "背景画像を開く",
                ChromeAction::OpenBackground,
            ),
            (self.can_export, "連番 PNG を書き出す", ChromeAction::Export),
        ] {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                actions.push(action);
            }
        }
        let response = ui.add_enabled(
            self.mp4_disabled_reason.is_none(),
            egui::Button::new("MP4 を書き出す"),
        );
        if let Some(reason) = self.mp4_disabled_reason {
            response.on_disabled_hover_text(reason);
        } else if response.clicked() {
            actions.push(ChromeAction::ExportMp4);
        }
        // Pause and play have different text widths; allocate the same button in both states.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let play_width = ["一時停止", "再生"]
            .into_iter()
            .map(|text| {
                ui.painter()
                    .layout_no_wrap(text.to_owned(), font.clone(), ui.visuals().text_color())
                    .size()
                    .x
            })
            .fold(ui.spacing().interact_size.x, f32::max)
            + 2.0 * ui.spacing().button_padding.x;
        if ui
            .add_enabled(
                self.can_play,
                egui::Button::new(if self.playing {
                    "一時停止"
                } else {
                    "再生"
                })
                .min_size(egui::vec2(play_width, 0.0)),
            )
            .clicked()
        {
            actions.push(ChromeAction::TogglePlayback);
        }
        ui.add_sized(
            [105.0, ui.spacing().interact_size.y],
            egui::Label::new(format!(
                "{} / {}",
                format_clock(self.position_secs),
                format_clock(self.duration_secs)
            ))
            .truncate(),
        );
        let mut position = self.position_secs;
        // Allocate the seek group independently of its current numeric value. Keep
        // Slider's direct numeric entry and label, including keyboard/drag editing.
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(260.0, ui.spacing().interact_size.y),
            egui::Sense::hover(),
        );
        let mut seek_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        seek_ui.set_clip_rect(ui.clip_rect().intersect(rect));
        if seek_ui
            .add(
                egui::Slider::new(&mut position, 0.0..=self.duration_secs.max(0.001))
                    .text("時刻 (秒)"),
            )
            .on_hover_text("時刻 (秒)")
            .changed()
        {
            actions.push(ChromeAction::Seek(position));
        }
        if ui.button("診断").clicked() {
            actions.push(ChromeAction::ShowDiagnostics);
        }
    }
}

impl MusicVideoLabApp {
    pub(super) fn load_choice_open(&self) -> bool {
        matches!(self.project_load, ProjectLoadState::Ready(_))
            || matches!(self.lrc_load, LrcLoadState::Ready { .. })
    }

    pub(super) fn project_caption(&self) -> String {
        let name = self.project_name();
        let suffix = if self.document.is_dirty(&self.project) {
            "（未保存）"
        } else {
            ""
        };
        format!("{name}{suffix}")
    }

    fn project_name(&self) -> String {
        self.document
            .path()
            .and_then(|p| p.file_name())
            .map_or_else(|| "新規".to_owned(), |p| p.to_string_lossy().into_owned())
    }

    fn missing_count(&self) -> usize {
        unresolved_image_notices(&self.project, &self.preview_report, &self.image_errors).len()
            + self
                .preview_report
                .unresolved_assets
                .iter()
                .filter(|a| matches!(a, UnresolvedAsset::Font(_)))
                .count()
            + usize::from(self.audio_error.is_some())
            + usize::from(matches!(
                self.runtime_resources,
                RuntimeResourceState::Unresolved { .. }
            ))
    }

    pub(super) fn draw_status_bar(
        &mut self,
        ctx: &egui::Context,
        input_enabled: bool,
        actions: &mut Vec<ChromeAction>,
    ) {
        // Draw BEFORE timeline, so this is always the outermost (lowest) bottom panel.
        egui::TopBottomPanel::bottom("musicvideo-status")
            .exact_height(30.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                ui.add_enabled_ui(input_enabled, |ui| {
                    self.draw_status_row(ui, actions);
                });
            });
    }

    fn draw_status_row(&self, ui: &mut egui::Ui, actions: &mut Vec<ChromeAction>) {
        let row = ui.available_rect_before_wrap();
        let width = row.width().max(0.0);
        let height = row.height();
        let font = egui::TextStyle::Button.resolve(ui.style());
        let painter = ui.painter().clone();
        let color = ui.visuals().text_color();
        let padding = ui.spacing().button_padding.x * 2.0;
        let text_width = |text: &str| {
            painter
                .layout_no_wrap(text.to_owned(), font.clone(), color)
                .size()
                .x
        };
        let button_width = |text: &str| text_width(text) + padding;
        let dirty = self.document.is_dirty(&self.project);
        let cancel_export = self.export_state.pending().is_some();
        let cancel_resources = self.pending_project_edit_in_progress();
        let cancel_count = usize::from(cancel_export) + usize::from(cancel_resources);
        let mut gap = ui.spacing().item_spacing.x.min(width / 12.0);
        // Essential controls own space before any project name, progress or status text.
        // Compact labels keep the same direct cancellation actions at small widths.
        let full_cancel_width = if cancel_export {
            button_width("書き出しをキャンセル")
        } else {
            0.0
        } + if cancel_resources {
            button_width("素材読込をキャンセル")
        } else {
            0.0
        };
        let full_dirty_width = if dirty {
            text_width("（未保存）").ceil() + 2.0
        } else {
            0.0
        };
        let compact =
            full_cancel_width + full_dirty_width + button_width("詳細") + gap * 4.0 > width;
        let dirty_label = if compact { "*" } else { "（未保存）" };
        // Label placement rounds to pixels; include padding around the measured glyphs.
        let dirty_width = if dirty {
            text_width(dirty_label).ceil() + 2.0
        } else {
            0.0
        };
        let menu_label = if width < 160.0 { "..." } else { "詳細" };
        let menu_width = button_width(menu_label).min(width * 0.3);
        let minimum_cancel_width = if cancel_export {
            button_width("E")
        } else {
            0.0
        } + if cancel_resources {
            button_width("R")
        } else {
            0.0
        };
        let gap_count = cancel_count as f32 + 2.0;
        gap = gap
            .min(((width - dirty_width - menu_width - minimum_cancel_width) / gap_count).max(0.0));
        let cancel_budget = (width - dirty_width - menu_width - gap * gap_count).max(0.0);
        let cancel_slot = if cancel_count == 0 {
            0.0
        } else {
            cancel_budget / cancel_count as f32
        };
        let mut right = row.right();
        // Keep automatic widget IDs independent of conditional siblings in the parent.
        let mut draw_cancel = |salt, full: &str, short: &str, tiny: &str, action| {
            let label = if !compact {
                full
            } else if button_width(short) <= cancel_slot {
                short
            } else {
                tiny
            };
            let size = if compact {
                button_width(label).min(cancel_slot)
            } else {
                button_width(label)
            };
            let rect = egui::Rect::from_min_size(
                egui::pos2(right - size, row.top()),
                egui::vec2(size, height),
            )
            .round_ui();
            right = rect.left() - gap;
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .id(salt)
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.set_clip_rect(ui.clip_rect().intersect(rect));
            if child
                .add_sized(rect.size(), egui::Button::new(label).truncate())
                .on_hover_text(full)
                .clicked()
            {
                actions.push(action);
            }
        };
        if cancel_export {
            draw_cancel(
                "musicvideo-status-cancel-export",
                "書き出しをキャンセル",
                "書出取消",
                "E",
                ChromeAction::CancelExport,
            );
        }
        if cancel_resources {
            draw_cancel(
                "musicvideo-status-cancel-resources",
                "素材読込をキャンセル",
                "素材取消",
                "R",
                ChromeAction::CancelResources,
            );
        }

        let mut left = row.left();
        if dirty {
            let rect = egui::Rect::from_min_size(
                egui::pos2(left, row.top()),
                egui::vec2(dirty_width, height),
            );
            ui.put(rect, egui::Label::new(dirty_label).extend())
                .on_hover_text("未保存の変更があります");
            left = rect.right() + gap;
        }
        let menu_rect =
            egui::Rect::from_min_size(egui::pos2(left, row.top()), egui::vec2(menu_width, height));
        let mut menu_ui = ui.new_child(
            egui::UiBuilder::new()
                .id("musicvideo-status-details")
                .max_rect(menu_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        menu_ui.menu_button(menu_label, |ui| {
            ui.set_max_width((ui.ctx().content_rect().width() - 24.0).clamp(80.0, 500.0));
            ui.label(egui::RichText::new(self.project_caption()).strong())
                .on_hover_text(self.project_caption());
            ui.add(egui::Label::new(&self.status).truncate())
                .on_hover_text(&self.status);
            let count = self.missing_count();
            if count > 0 && ui.button(format!("未解決の素材 {count} 件")).clicked() {
                actions.push(ChromeAction::ShowMissing);
                ui.close();
            }
            if matches!(
                self.runtime_resources,
                RuntimeResourceState::Restoring { .. }
            ) && ui.button("素材復元中").clicked()
            {
                actions.push(ChromeAction::ShowMissing);
                ui.close();
            }
            self.draw_export_progress(ui);
        });
        left = menu_rect.right() + gap;
        let info_width = (right - left).max(0.0);
        if info_width > 0.0 {
            let rect = egui::Rect::from_min_size(
                egui::pos2(left, row.top()),
                egui::vec2(info_width, height),
            );
            let mut info = ui.new_child(
                egui::UiBuilder::new()
                    .id("musicvideo-status-info")
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            info.set_clip_rect(ui.clip_rect().intersect(rect));
            let name_width = (info_width * 0.25).min(190.0);
            info.add_sized(
                [name_width, 20.0],
                egui::Label::new(self.project_name()).truncate(),
            )
            .on_hover_text(self.document.path().map_or_else(
                || "新規プロジェクト".to_owned(),
                |p| p.display().to_string(),
            ));
            let count = self.missing_count();
            let missing_label = format!("未解決の素材 {count} 件");
            if count > 0
                && info.available_width() > button_width(&missing_label) + 90.0
                && info.button(missing_label).clicked()
            {
                actions.push(ChromeAction::ShowMissing);
            }
            if matches!(
                self.runtime_resources,
                RuntimeResourceState::Restoring { .. }
            ) && info.available_width() > button_width("素材復元中") + 90.0
                && info.button("素材復元中").clicked()
            {
                actions.push(ChromeAction::ShowMissing);
            }
            if info.available_width() > 260.0 {
                self.draw_export_progress(&mut info);
            }
            info.add_sized(
                [info.available_width().max(0.0), 20.0],
                egui::Label::new(&self.status).truncate(),
            )
            .on_hover_text(&self.status);
        }
    }

    fn draw_export_progress(&self, ui: &mut egui::Ui) {
        if let Some(pending) = self.export_state.pending() {
            let completed = pending.progress.load(Ordering::Relaxed);
            ui.add(
                egui::ProgressBar::new(completed as f32 / pending.total_frames.max(1) as f32)
                    .desired_width(160.0)
                    .show_percentage()
                    .text(format!("{completed} / {} フレーム", pending.total_frames)),
            );
        }
    }

    pub(super) fn draw_load_choice(&self, ctx: &egui::Context) -> Vec<ChromeAction> {
        let mut actions = Vec::new();
        // One decision at a time. Keep both Ready payloads; Project replacement already
        // invalidates LRC via the existing session boundary, whereas discard preserves it.
        if matches!(self.project_load, ProjectLoadState::Ready(_)) {
            egui::Modal::new(egui::Id::new("musicvideo-project-load-choice")).show(ctx, |ui| {
                ui.heading("プロジェクトの読込確認");
                ui.label("プロジェクト読込中に現在の編集内容が変わりました。");
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("読込結果へ切り替える（現在の変更を破棄）")
                        .clicked()
                    {
                        actions.push(ChromeAction::ApplyProject);
                    }
                    if ui.button("現在の編集を続ける").clicked() {
                        actions.push(ChromeAction::DiscardProject);
                    }
                });
            });
        } else if matches!(self.lrc_load, LrcLoadState::Ready { .. }) {
            egui::Modal::new(egui::Id::new("musicvideo-lrc-load-choice")).show(ctx, |ui| {
                ui.heading("LRC の読込確認");
                ui.label("LRC 読込中に対象の歌詞が変更されました。");
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("LRC の内容へ置換（現在の歌詞変更を破棄）")
                        .clicked()
                    {
                        actions.push(ChromeAction::ApplyLrc);
                    }
                    if ui.button("現在の歌詞を保持").clicked() {
                        actions.push(ChromeAction::DiscardLrc);
                    }
                });
            });
        }
        actions
    }

    pub(super) fn draw_tool_windows(
        &mut self,
        ctx: &egui::Context,
        input_enabled: bool,
        actions: &mut Vec<ChromeAction>,
    ) {
        if self.show_missing_resources {
            let mut open = true;
            egui::Window::new("未解決の素材")
                .open(&mut open)
                .default_width(560.0)
                .default_height(300.0)
                .show(ctx, |ui| {
                    ui.add_enabled_ui(input_enabled, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                match &self.runtime_resources {
                                    RuntimeResourceState::Restoring { .. } => {
                                        ui.label("履歴の素材を復元しています…");
                                    }
                                    RuntimeResourceState::Unresolved { error, .. } => {
                                        ui.colored_label(
                                            ui.visuals().error_fg_color,
                                            format!("履歴の素材が未解決です: {error}"),
                                        );
                                        if ui.button("素材の復元を再試行").clicked() {
                                            actions.push(ChromeAction::RetryRestore);
                                        }
                                    }
                                    RuntimeResourceState::Ready(_) => {}
                                }
                                let notices = unresolved_image_notices(
                                    &self.project,
                                    &self.preview_report,
                                    &self.image_errors,
                                );
                                if let Some(id) = draw_missing_images(ui, &notices) {
                                    actions.push(ChromeAction::ReplaceImage(id));
                                }
                                for asset in &self.preview_report.unresolved_assets {
                                    if let UnresolvedAsset::Font(key) = asset {
                                        ui.colored_label(
                                            ui.visuals().error_fg_color,
                                            format!("未解決素材: フォント {key}"),
                                        );
                                    }
                                }
                                if let Some(error) = &self.audio_error {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        format!("未解決の音声素材: {error}"),
                                    );
                                }
                                if self.missing_count() == 0
                                    && !matches!(
                                        self.runtime_resources,
                                        RuntimeResourceState::Restoring { .. }
                                    )
                                {
                                    ui.label("未解決の素材はありません");
                                }
                            });
                    });
                });
            self.show_missing_resources = open;
        }
        if self.show_diagnostics {
            self.draw_diagnostics(ctx, input_enabled, actions);
        }
    }

    pub(super) fn apply_chrome_actions(
        &mut self,
        ctx: &egui::Context,
        actions: Vec<ChromeAction>,
    ) -> bool {
        let mut reset_perf = false;
        for action in actions {
            if matches!(
                action,
                ChromeAction::SaveIntent
                    | ChromeAction::DiscardIntent
                    | ChromeAction::CancelIntent
                    | ChromeAction::StopExportAndClose
            ) {
                self.apply_intent_action(action, ctx);
                continue;
            }
            if self.intent_pending() || self.choosing_project_save() {
                continue;
            }
            if self.mp4_export_running() && !matches!(action, ChromeAction::CancelExport) {
                continue;
            }
            if self.export_state.setup_open()
                && !(matches!(self.export_state, ExportState::Settings(_))
                    && matches!(action, ChromeAction::ChooseMp4Output))
            {
                continue;
            }
            // A broken modal gate must fail headless tests before reaching a
            // native file dialog. The assertion is absent from product builds.
            #[cfg(test)]
            assert!(
                !self.mp4_export_running() || matches!(action, ChromeAction::CancelExport),
                "modal input reached dispatch"
            );
            match action {
                ChromeAction::NewProject => self.accept_document_intent(DocumentIntent::New),
                ChromeAction::SaveIntent
                | ChromeAction::DiscardIntent
                | ChromeAction::CancelIntent
                | ChromeAction::StopExportAndClose => unreachable!(),
                ChromeAction::ExportMp4 => {
                    if self.mp4_export_disabled_reason().is_none() {
                        self.export_state =
                            ExportState::Settings(crate::mp4::ExportSettings::default());
                    }
                }
                ChromeAction::ChooseMp4Output => {
                    if let ExportState::Settings(settings) = std::mem::take(&mut self.export_state)
                        && self.mp4_export_disabled_reason().is_none()
                    {
                        self.choose_mp4_output(settings, ctx);
                    }
                }
                ChromeAction::OpenProject => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("mIV 音楽ビデオ", &["json"])
                        .pick_file()
                    {
                        self.accept_document_intent(DocumentIntent::Open(path));
                    }
                }
                ChromeAction::SaveProject | ChromeAction::SaveProjectAs => {
                    self.request_project_save(matches!(action, ChromeAction::SaveProjectAs), ctx);
                }
                ChromeAction::OpenAudio => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(
                            "音声",
                            &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "alac"],
                        )
                        .pick_file()
                    {
                        self.start_audio_load(path, ctx);
                    }
                }
                ChromeAction::OpenBackground | ChromeAction::ReplaceImage(_) => {
                    let id = if let ChromeAction::ReplaceImage(id) = action {
                        id
                    } else {
                        ImageId::new(BACKGROUND_ID)
                    };
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("画像", &["png", "jpg", "jpeg", "webp", "bmp"])
                        .pick_file()
                    {
                        self.start_image_load(id, path, ctx);
                    }
                }
                ChromeAction::Export => {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.start_export(folder, ctx);
                    }
                }
                ChromeAction::TogglePlayback => {
                    if let Some(player) = &mut self.audio_player {
                        player.toggle();
                        reset_perf = true;
                        self.schedule_playback_preview();
                    }
                }
                ChromeAction::Seek(position) => self.seek_to(position),
                ChromeAction::ResetPerf => reset_perf = true,
                ChromeAction::ShowMissing => self.show_missing_resources = true,
                ChromeAction::ShowDiagnostics => self.show_diagnostics = true,
                ChromeAction::CancelExport => {
                    if let Some(pending) = self.export_state.pending() {
                        pending.cancel.store(true, Ordering::Relaxed);
                        self.status = "キャンセルを要求しました".to_owned();
                    }
                }
                ChromeAction::CancelResources => {
                    self.cancel_pending_resource_loads();
                    self.document.note_edit();
                    self.status = "素材の読込をキャンセルしました".to_owned();
                }
                ChromeAction::RetryRestore => self.retry_runtime_restore(ctx),
                ChromeAction::ApplyProject => {
                    if let ProjectLoadState::Ready(loaded) = std::mem::take(&mut self.project_load)
                    {
                        self.apply_loaded_project(loaded);
                    }
                }
                ChromeAction::DiscardProject => {
                    if let ProjectLoadState::Ready(loaded) = std::mem::take(&mut self.project_load)
                    {
                        self.defer_loaded_project(loaded);
                        self.status = "読込結果を破棄し、現在の編集を続けます".to_owned();
                    }
                }
                ChromeAction::ApplyLrc => {
                    if let LrcLoadState::Ready {
                        owner,
                        path,
                        report,
                    } = std::mem::take(&mut self.lrc_load)
                    {
                        self.apply_lrc_result(owner, path, report, ctx);
                    }
                }
                ChromeAction::DiscardLrc => {
                    if matches!(
                        std::mem::take(&mut self.lrc_load),
                        LrcLoadState::Ready { .. }
                    ) {
                        self.status = "LRC の読込結果を破棄し、現在の歌詞を保持しました".to_owned();
                    }
                }
            }
        }
        reset_perf
    }
    fn draw_diagnostics(
        &mut self,
        ctx: &egui::Context,
        input_enabled: bool,
        actions: &mut Vec<ChromeAction>,
    ) {
        let mut open = self.show_diagnostics;
        egui::Window::new("診断")
            .open(&mut open)
            .default_width(620.0)
            .default_height(300.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(input_enabled, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.label(format!("合成 {:.2} ms", self.preview_render_ms));
                            if ui.small_button("計測をリセット").clicked() {
                                actions.push(ChromeAction::ResetPerf);
                            }
                            if let Some(outcome) = &self.last_export {
                                match outcome {
                                    ExportOutcome::Completed { stats, .. } => {
                                        ui.label(format!(
                                            "解析: 平均 {:.3} ms / p95 {:.3} ms",
                                            stats.analysis.mean_ms, stats.analysis.p95_ms
                                        ));
                                        ui.label(format!(
                                            "合成: 平均 {:.3} ms / p95 {:.3} ms",
                                            stats.composition.mean_ms, stats.composition.p95_ms
                                        ));
                                        ui.label(format!(
                                            "合計: 平均 {:.3} ms / p95 {:.3} ms / 実時間比 {:.3}x",
                                            stats.total.mean_ms, stats.total.p95_ms, stats.realtime_ratio
                                        ));
                                    }
                                    ExportOutcome::Cancelled {
                                        completed_frames, ..
                                    } => {
                                        ui.label(format!("完了済み: {completed_frames} フレーム"));
                                    }
                                }
                            }

                            ui.small(
                                "シーク時はスペクトラムの履歴をリセットします。連続再生中は書き出しと同じ解析順です。",
                            );
                            if let Some(perf) = self.preview_perf.summary() {
                                ui.small(format!(
                                    "プレビュー実測 (直近 {}): {:.1} fps / 表示間隔 {:.2} ms",
                                    perf.samples, perf.fps, perf.display_interval_ms
                                ));
                                ui.small(format!(
                                    "worker: 解析 {:.2} / シーン評価+合成 {:.2} / RGBA→egui {:.2} / その他 {:.2} ms",
                                    perf.analysis_ms,
                                    perf.scene_composition_ms,
                                    perf.rgba_to_egui_ms,
                                    perf.worker_other_ms
                                ));
                                ui.small(format!(
                                    "UI: texture.set {:.2} / egui layout+draw {:.2} / 音声同期 {:.3} / その他 {:.2} ms",
                                    perf.texture_update_ms,
                                    perf.egui_layout_ms,
                                    perf.audio_sync_ms,
                                    perf.ui_other_ms
                                ));
                                ui.small(format!(
                                    "表示間隔−worker (eframe描画/GPU upload/待機、並行区間を含む): {:.2} ms",
                                    perf.outside_worker_ms
                                ));
                            }
                        });
                });
            });
        self.show_diagnostics = open;
    }
}
