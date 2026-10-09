//! Host-owned chrome, file dialogs and decision presentation. No completion admission here.
use super::*;

pub(super) enum ChromeAction {
    OpenProject,
    SaveProject,
    SaveProjectAs,
    OpenAudio,
    OpenBackground,
    Export,
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
    can_play: bool,
    pub(super) playing: bool,
    pub(super) position_secs: f64,
    pub(super) duration_secs: f64,
}

impl ToolbarState {
    pub(super) fn from_app(app: &MusicVideoLabApp) -> Self {
        Self {
            can_open_project: app.export_pending.is_none() && app.lrc_save_pending.is_none(),
            can_save: app.project_save_pending.is_none(),
            can_open_material: app.export_pending.is_none(),
            can_export: app.audio.is_some()
                && app.assets.has_image(&ImageId::new(BACKGROUND_ID))
                && app.runtime_resources_ready()
                && app.export_pending.is_none(),
            can_play: app.audio_transport_ready(),
            playing: playback_snapshot_for_ui(
                app.position_secs,
                app.audio_transport_ready(),
                app.audio_player.as_ref().map(AudioPlayer::snapshot),
            )
            .playing,
            position_secs: app.position_secs,
            duration_secs: app.project.duration_secs,
        }
    }

    pub(super) fn draw(&self, ui: &mut egui::Ui, actions: &mut Vec<ChromeAction>) {
        for (enabled, label, action) in [
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
        let name = self
            .document
            .path()
            .and_then(|p| p.file_name())
            .map_or_else(|| "新規".to_owned(), |p| p.to_string_lossy().into_owned());
        let suffix = if self.document.is_dirty(&self.project) {
            "（未保存）"
        } else {
            ""
        };
        format!("{name}{suffix}")
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
                    ui.horizontal(|ui| {
                        let caption = self.project_caption();
                        let caption_width = (ui.available_width() * 0.18).min(190.0);
                        ui.add_sized([caption_width, 20.0], egui::Label::new(caption).truncate())
                            .on_hover_text(self.document.path().map_or_else(
                                || "新規プロジェクト".to_owned(),
                                |p| p.display().to_string(),
                            ));
                        let count = self.missing_count();
                        if count > 0 && ui.button(format!("未解決の素材 {count} 件")).clicked()
                        {
                            actions.push(ChromeAction::ShowMissing);
                        }
                        if matches!(
                            self.runtime_resources,
                            RuntimeResourceState::Restoring { .. }
                        ) && ui.button("素材復元中").clicked()
                        {
                            actions.push(ChromeAction::ShowMissing);
                        }
                        if self.pending_project_edit_in_progress()
                            && ui.button("素材読込をキャンセル").clicked()
                        {
                            actions.push(ChromeAction::CancelResources);
                        }
                        if let Some(pending) = &self.export_pending {
                            let completed = pending.progress.load(Ordering::Relaxed);
                            ui.add(
                                egui::ProgressBar::new(
                                    completed as f32 / pending.total_frames.max(1) as f32,
                                )
                                .desired_width(160.0)
                                .show_percentage()
                                .text(format!("{completed} / {} フレーム", pending.total_frames)),
                            );
                            if ui.button("書き出しをキャンセル").clicked() {
                                actions.push(ChromeAction::CancelExport);
                            }
                        }
                        ui.add_sized(
                            [ui.available_width().max(0.0), 20.0],
                            egui::Label::new(&self.status).truncate(),
                        )
                        .on_hover_text(&self.status);
                    });
                });
            });
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
            match action {
                ChromeAction::OpenProject => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("mIV 音楽ビデオ", &["json"])
                        .pick_file()
                    {
                        self.start_project_load(path, ctx);
                    }
                }
                ChromeAction::SaveProject | ChromeAction::SaveProjectAs => {
                    let path = if matches!(action, ChromeAction::SaveProject) {
                        self.document.path().map(PathBuf::from)
                    } else {
                        None
                    };
                    if let Some(path) = path.or_else(|| {
                        rfd::FileDialog::new()
                            .add_filter("mIV 音楽ビデオ", &["json"])
                            .set_file_name("project.mivmv.json")
                            .save_file()
                    }) {
                        self.start_project_save(ensure_project_extension(path), ctx);
                    }
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
                    if let Some(player) = &self.audio_player {
                        self.position_secs = player.toggle().position_secs;
                        reset_perf = true;
                        self.schedule_playback_preview();
                    }
                }
                ChromeAction::Seek(position) => self.seek_to(position),
                ChromeAction::ResetPerf => reset_perf = true,
                ChromeAction::ShowMissing => self.show_missing_resources = true,
                ChromeAction::ShowDiagnostics => self.show_diagnostics = true,
                ChromeAction::CancelExport => {
                    if let Some(pending) = &self.export_pending {
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
