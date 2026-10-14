use super::*;
use crate::mp4::{ExportSettings, export_snapshot, snapshot};
use mimageviewer::video::mvexport::{EncoderPreference, ExportError, ExportStage, ExportSummary};

impl MusicVideoLabApp {
    pub(super) fn choose_mp4_output(&mut self, settings: ExportSettings, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let worker_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-mp4-save-dialog".to_owned())
            .spawn(move || {
                // Keep native overwrite approval and its existence observation together.
                // Neither the native dialog nor filesystem metadata blocks the UI.
                let selected = rfd::FileDialog::new()
                    .add_filter("MP4 動画", &["mp4"])
                    .set_file_name("musicvideo.mp4")
                    .save_file()
                    .map(|path| {
                        let replace_existing = path.exists();
                        (path, replace_existing)
                    });
                let _ = tx.send(selected);
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => self.export_state = ExportState::Choosing { settings, rx },
            Err(error) => {
                self.status = format!("保存先ダイアログを開始できません: {error}");
                eprintln!("{}", self.status);
            }
        }
    }

    pub(super) fn poll_mp4_choice(&mut self, ctx: &egui::Context) {
        let ExportState::Choosing { rx, .. } = &self.export_state else {
            return;
        };
        match rx.try_recv() {
            Ok(choice) => {
                let ExportState::Choosing { settings, .. } = std::mem::take(&mut self.export_state)
                else {
                    return;
                };
                if let Some((path, replace_existing)) = choice {
                    self.begin_mp4_export(path, settings, replace_existing, ctx);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.export_state = ExportState::Idle;
                self.status = "保存先ダイアログが終了しました".to_owned();
                eprintln!("{}", self.status);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub(super) fn mp4_export_running(&self) -> bool {
        matches!(
            self.export_state.pending().map(|pending| &pending.kind),
            Some(ExportKind::Mp4 { .. })
        )
    }

    pub(super) fn mp4_export_disabled_reason(&self) -> Option<&'static str> {
        if self.export_state.pending().is_some() {
            Some("書き出しが進行中です")
        } else if !matches!(self.project_load, ProjectLoadState::Idle)
            || self.audio_pending.is_some()
            || self.image_pending.is_some()
            || self.lrc_load.is_active()
        {
            Some("プロジェクト・歌詞・素材の読込または読込確認が進行中です")
        } else if self.project_save_pending.is_some() || self.lrc_save_pending.is_some() {
            Some("プロジェクトまたは歌詞の保存が進行中です")
        } else if self.editor_state.has_pending_lyrics_edit() {
            Some("歌詞の行対応を確認してください")
        } else if !self.runtime_resources_ready()
            || self
                .project
                .images
                .iter()
                .any(|reference| !self.assets.has_image(&reference.id))
            || self.project.layers.iter().any(|layer| match &layer.kind {
                LayerKind::Image { image, .. } => !self.assets.has_image(image),
                LayerKind::Text(text) => self.assets.fonts.get(&text.font_key).is_none(),
                LayerKind::TimeText(text) => self.assets.fonts.get(&text.font_key).is_none(),
                LayerKind::Lyrics(lyrics) => self.assets.fonts.get(&lyrics.font_key).is_none(),
                _ => false,
            })
        {
            Some("すべての素材の読込が完了するまで書き出せません")
        } else if self.audio.is_none() || self.editor_state.resource_manifest().audio().is_none() {
            Some("先に音声を読み込んでください")
        } else {
            None
        }
    }

    pub(super) fn begin_mp4_export(
        &mut self,
        output_path: PathBuf,
        settings: ExportSettings,
        replace_existing: bool,
        ctx: &egui::Context,
    ) {
        if let Some(reason) = self.mp4_export_disabled_reason() {
            self.status = reason.to_owned();
            return;
        }
        let request = snapshot(
            &self.project,
            &self.assets,
            Arc::clone(self.audio.as_ref().unwrap()),
            self.editor_state
                .resource_manifest()
                .audio()
                .unwrap()
                .revision(),
            output_path.clone(),
            settings,
            replace_existing,
        );
        let total_frames = match frame_count(&self.project) {
            Ok(frames) => frames,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        self.suspend_preview_for_mp4();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let stage = Arc::new(AtomicUsize::new(0));
        let worker_cancel = Arc::clone(&cancel);
        let worker_progress = Arc::clone(&progress);
        let worker_stage = Arc::clone(&stage);
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("musicvideo-mp4-export".to_owned())
            .spawn(move || {
                let result = export_snapshot(
                    request,
                    &|update| {
                        worker_progress.store(update.completed_frames as usize, Ordering::Relaxed);
                        worker_stage.store(stage_index(update.stage), Ordering::Relaxed);
                        worker_ctx.request_repaint();
                    },
                    &worker_cancel,
                );
                let _ = tx.send(Ok(ExportCompletion::Mp4(result)));
                worker_ctx.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.export_state = ExportState::Running(ExportPending {
                    kind: ExportKind::Mp4 { stage, output_path },
                    cancel,
                    progress,
                    total_frames,
                    rx,
                });
                self.status = "MP4 を書き出しています".to_owned();
            }
            Err(error) => {
                self.status = format!("書き出しワーカーを開始できません: {error}");
                eprintln!("{}", self.status);
            }
        }
    }

    fn suspend_preview_for_mp4(&mut self) {
        if let Some(player) = &mut self.audio_player {
            player.pause();
        }
        // Invalidate both successful and error results before cancellation.
        // Keep the receiver until asynchronous terminal delivery/disconnect.
        self.mark_preview_dirty();
        if let Some(pending) = self.calibration_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
            self.calibration_key = None;
        }
    }

    pub(super) fn complete_mp4_export(&mut self, result: Result<ExportSummary, ExportError>) {
        let Some(pending) = self.export_state.take_worker() else {
            return;
        };
        let ExportKind::Mp4 { output_path, .. } = pending.kind else {
            return;
        };
        self.status = match result {
            Ok(summary) => format!(
                "MP4 を書き出しました ({} フレーム): {}",
                summary.frames,
                output_path.display()
            ),
            Err(ExportError::Cancelled) => "MP4 の書き出しを中止しました".to_owned(),
            Err(error) => {
                eprintln!("MP4 の書き出しに失敗しました: {error}");
                format!("MP4 の書き出しに失敗しました: {error}")
            }
        };
        self.preview_dirty = true;
    }

    pub(super) fn draw_mp4_dialogs(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<ChromeAction>,
    ) {
        if let ExportState::Settings(settings) = &mut self.export_state {
            let mut accept = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("musicvideo-mp4-settings")).show(ctx, |ui| {
                ui.heading("MP4 を書き出す");
                ui.label(format!(
                    "{} × {} / {} fps",
                    self.project.output.width, self.project.output.height, self.project.output.fps
                ));
                egui::ComboBox::from_id_salt("musicvideo-mp4-encoder")
                    .selected_text(encoder_label(settings.encoder))
                    .show_ui(ui, |ui| {
                        for encoder in [
                            EncoderPreference::Auto,
                            EncoderPreference::Nvenc,
                            EncoderPreference::Qsv,
                            EncoderPreference::Amf,
                            EncoderPreference::MediaFoundation,
                            EncoderPreference::OpenH264,
                        ] {
                            ui.selectable_value(
                                &mut settings.encoder,
                                encoder,
                                encoder_label(encoder),
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("映像ビットレート (bps)");
                    ui.add(
                        egui::DragValue::new(&mut settings.video_bitrate_bps)
                            .range(1..=u32::MAX)
                            .speed(100_000),
                    );
                });
                ui.horizontal(|ui| {
                    accept = ui.button("保存先を選ぶ").clicked();
                    cancel = ui.button("キャンセル").clicked();
                });
            });
            if accept {
                actions.push(ChromeAction::ChooseMp4Output);
            }
            if cancel {
                self.export_state = ExportState::Idle;
            }
        }
        if matches!(self.export_state, ExportState::Choosing { .. }) {
            egui::Modal::new(egui::Id::new("musicvideo-mp4-choosing")).show(ctx, |ui| {
                ui.label("MP4 の保存先を選んでください");
            });
        }
        if let Some(pending) = self.export_state.pending()
            && let ExportKind::Mp4 { stage, .. } = &pending.kind
        {
            egui::Modal::new(egui::Id::new("musicvideo-mp4-progress")).show(ctx, |ui| {
                ui.heading("MP4 を書き出しています");
                ui.label(stage_label(stage.load(Ordering::Relaxed)));
                let completed = pending.progress.load(Ordering::Relaxed);
                ui.add(
                    egui::ProgressBar::new(completed as f32 / pending.total_frames.max(1) as f32)
                        .desired_width(300.0)
                        .text(format!("{completed} / {} フレーム", pending.total_frames)),
                );
                if ui
                    .add_enabled(
                        !pending.cancel.load(Ordering::Relaxed),
                        egui::Button::new("中止"),
                    )
                    .clicked()
                {
                    actions.push(ChromeAction::CancelExport);
                }
                if pending.cancel.load(Ordering::Relaxed) {
                    ui.label("中止しています");
                }
            });
        }
    }
}

fn stage_index(stage: ExportStage) -> usize {
    match stage {
        ExportStage::Calibration => 0,
        ExportStage::AudioEncoding => 1,
        ExportStage::Frames => 2,
        ExportStage::Finalizing => 3,
    }
}

fn stage_label(stage: usize) -> &'static str {
    match stage {
        0 => "反応範囲の計算",
        1 => "音声の符号化",
        2 => "フレームの書き出し",
        _ => "仕上げ",
    }
}

fn encoder_label(encoder: EncoderPreference) -> &'static str {
    match encoder {
        EncoderPreference::Auto => "エンコーダ: 自動",
        EncoderPreference::Nvenc => "NVIDIA NVENC",
        EncoderPreference::Qsv => "Intel QSV",
        EncoderPreference::Amf => "AMD AMF",
        EncoderPreference::MediaFoundation => "Media Foundation",
        EncoderPreference::OpenH264 => "OpenH264",
    }
}
