//! One session-owned intent; only the end-of-frame handler may execute it.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DocumentIntent {
    Open(PathBuf),
    New,
    Close,
}

#[derive(Default)]
pub(super) enum DocumentIntentState {
    #[default]
    Idle,
    Pending {
        session: DocumentSessionId,
        intent: DocumentIntent,
        phase: IntentPhase,
    },
    CloseApproved,
}

pub(super) enum IntentPhase {
    Check,
    Confirm,
    Saving,
    ConfirmExport,
    StoppingExport,
}

impl MusicVideoLabApp {
    pub(super) fn choosing_project_save(&self) -> bool {
        matches!(
            self.project_save_pending,
            Some(ProjectSavePending::Choosing { .. })
        )
    }
    pub(super) fn intent_pending(&self) -> bool {
        matches!(self.document_intent, DocumentIntentState::Pending { .. })
    }

    pub(super) fn accept_document_intent(&mut self, intent: DocumentIntent) {
        if !self.editing_admitted() {
            return;
        }
        if matches!(self.document_intent, DocumentIntentState::CloseApproved) {
            return;
        }
        if let DocumentIntentState::Pending {
            intent: current, ..
        } = &mut self.document_intent
        {
            if intent == DocumentIntent::Close {
                *current = intent;
            }
            return;
        }
        // Remove the receiver too: a success already queued before cancellation cannot win.
        // Dropping it and any Ready payload uses the H2 cleanup worker, never a UI wait.
        match std::mem::take(&mut self.project_load) {
            ProjectLoadState::Loading(mut pending) => {
                pending.cancel.store(true, Ordering::Relaxed);
                pending.queued = None;
                if let Some(store) = &self.resource_store {
                    store.defer_drop(pending);
                }
            }
            ProjectLoadState::Ready(loaded) => self.defer_loaded_project(loaded),
            ProjectLoadState::Idle => {}
        }
        let phase = if intent == DocumentIntent::Close && self.export_state.pending().is_some() {
            IntentPhase::ConfirmExport
        } else {
            // Settings/output selection has no export worker to stop.
            if intent == DocumentIntent::Close && self.export_state.setup_open() {
                self.export_state = ExportState::Idle;
            }
            IntentPhase::Check
        };
        self.document_intent = DocumentIntentState::Pending {
            session: self.document.id(),
            intent,
            phase,
        };
    }

    pub(super) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested())
            && !matches!(self.document_intent, DocumentIntentState::CloseApproved)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.accept_document_intent(DocumentIntent::Close);
        }
    }

    /// The single dirty decision. Run once after all frame input and completion admission.
    pub(super) fn finish_document_intent_frame(&mut self, ctx: &egui::Context) {
        let DocumentIntentState::Pending { session, phase, .. } = &self.document_intent else {
            return;
        };
        if !self.document.is_current(*session) {
            self.document_intent = DocumentIntentState::Idle;
            return;
        }
        if matches!(phase, IntentPhase::ConfirmExport)
            || (matches!(phase, IntentPhase::StoppingExport)
                && self.export_state.pending().is_some())
            || self.project_save_pending.is_some()
            || self.editor_state.has_pending_lyrics_edit()
        {
            return;
        }
        if matches!(phase, IntentPhase::Confirm) {
            return;
        }
        if self.document.is_dirty(&self.project) {
            if let DocumentIntentState::Pending { phase, .. } = &mut self.document_intent {
                *phase = IntentPhase::Confirm;
            }
            ctx.request_repaint();
        } else {
            self.execute_document_intent(ctx);
        }
    }

    fn execute_document_intent(&mut self, ctx: &egui::Context) {
        let DocumentIntentState::Pending {
            intent, session, ..
        } = std::mem::take(&mut self.document_intent)
        else {
            return;
        };
        if !self.document.is_current(session) {
            return;
        }
        match intent {
            DocumentIntent::Open(path) => self.start_project_load(path, ctx),
            DocumentIntent::New => {
                // Reuse precisely the Project replacement resource/audio/history boundary.
                self.apply_loaded_project(LoadedProject {
                    path: PathBuf::new(),
                    project: default_project(DEFAULT_DURATION_SECS),
                    audio: None,
                    waveform: None,
                    images: vec![],
                    resources: ResourceManifest::default(),
                    image_errors: HashMap::new(),
                    audio_error: None,
                });
                self.document = DocumentSession::default();
                self.status = "新しいプロジェクトを作成しました".to_owned();
            }
            DocumentIntent::Close => {
                self.document_intent = DocumentIntentState::CloseApproved;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(super) fn apply_intent_action(&mut self, action: ChromeAction, ctx: &egui::Context) {
        match action {
            ChromeAction::CancelIntent => self.document_intent = DocumentIntentState::Idle,
            ChromeAction::DiscardIntent => {
                if matches!(
                    self.document_intent,
                    DocumentIntentState::Pending {
                        phase: IntentPhase::Confirm,
                        ..
                    }
                ) {
                    self.execute_document_intent(ctx);
                }
            }
            ChromeAction::SaveIntent => {
                if let DocumentIntentState::Pending {
                    phase: phase @ IntentPhase::Confirm,
                    ..
                } = &mut self.document_intent
                {
                    *phase = IntentPhase::Saving;
                    self.request_project_save(false, ctx);
                }
            }
            ChromeAction::StopExportAndClose => {
                if let DocumentIntentState::Pending {
                    phase: phase @ IntentPhase::ConfirmExport,
                    ..
                } = &mut self.document_intent
                {
                    *phase = IntentPhase::StoppingExport;
                    if let Some(pending) = self.export_state.pending() {
                        pending.cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_document_intent(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<ChromeAction>,
    ) {
        if !self.intent_pending() {
            if self.choosing_project_save() {
                egui::Modal::new(egui::Id::new("musicvideo-project-save-choice")).show(ctx, |ui| {
                    ui.label("プロジェクトの保存先を選んでください");
                });
            }
            return;
        }
        if matches!(
            self.document_intent,
            DocumentIntentState::Pending {
                phase: IntentPhase::ConfirmExport | IntentPhase::StoppingExport,
                ..
            }
        ) {
            egui::Modal::new(egui::Id::new("musicvideo-close-export")).show(ctx, |ui| {
                if matches!(
                    self.document_intent,
                    DocumentIntentState::Pending {
                        phase: IntentPhase::ConfirmExport,
                        ..
                    }
                ) {
                    ui.heading("書き出しを中止して閉じますか");
                    ui.horizontal(|ui| {
                        if ui.button("中止して閉じる").clicked() {
                            actions.push(ChromeAction::StopExportAndClose);
                        }
                        if ui.button("キャンセル").clicked() {
                            actions.push(ChromeAction::CancelIntent);
                        }
                    });
                } else {
                    ui.label("書き出しの中止を待っています");
                }
            });
        } else if self.editor_state.has_pending_lyrics_edit() {
            let response = self
                .editor_state
                .pending_lyrics_confirmation(ctx, &mut self.project);
            self.handle_editor_response(response);
        } else {
            egui::Modal::new(egui::Id::new("musicvideo-document-intent")).show(ctx, |ui| {
                ui.heading("未保存の編集の確認");
                if self.project_save_pending.is_some() {
                    ui.label("プロジェクトの保存を待っています");
                } else if matches!(
                    self.document_intent,
                    DocumentIntentState::Pending {
                        phase: IntentPhase::Confirm,
                        ..
                    }
                ) {
                    ui.label("変更を保存しますか");
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            actions.push(ChromeAction::SaveIntent);
                        }
                        if ui.button("破棄").clicked() {
                            actions.push(ChromeAction::DiscardIntent);
                        }
                        if ui.button("キャンセル").clicked() {
                            actions.push(ChromeAction::CancelIntent);
                        }
                    });
                }
            });
        }
    }

    pub(super) fn request_project_save(&mut self, save_as: bool, ctx: &egui::Context) {
        if self.project_save_pending.is_some() {
            return;
        }
        if !save_as && let Some(path) = self.document.path().map(PathBuf::from) {
            self.start_project_save(path, ctx);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let worker_ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("musicvideo-project-save-dialog".to_owned())
            .spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("mIV 音楽ビデオ", &["json"])
                    .set_file_name("project.mivmv.json")
                    .save_file();
                let _ = tx.send(path);
                worker_ctx.request_repaint();
            }) {
            Ok(_) => {
                self.project_save_pending = Some(ProjectSavePending::Choosing {
                    session: self.document.id(),
                    rx,
                })
            }
            Err(error) => {
                self.status = format!("保存先ダイアログを開始できません: {error}");
                self.project_save_failed();
            }
        }
    }

    pub(super) fn poll_project_save_choice(&mut self, ctx: &egui::Context) {
        let Some(ProjectSavePending::Choosing { rx, .. }) = &self.project_save_pending else {
            return;
        };
        let result = rx.try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) {
            return;
        }
        let Some(ProjectSavePending::Choosing { session, .. }) = self.project_save_pending.take()
        else {
            unreachable!()
        };
        if !self.document.is_current(session) {
            return;
        }
        match result {
            Ok(Some(path)) => self.start_project_save(ensure_project_extension(path), ctx),
            Ok(None) => self.document_intent = DocumentIntentState::Idle,
            Err(_) => {
                self.status = "保存先ダイアログが終了しました".to_owned();
                self.project_save_failed();
            }
        }
    }

    pub(super) fn project_save_failed(&mut self) {
        self.log_status();
        self.document_intent = DocumentIntentState::Idle;
    }
}
