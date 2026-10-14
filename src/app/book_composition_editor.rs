//! Global admission/tickets, with the draft and presentation owned by one viewer.
//! Workers do source and store I/O; neither temporary mounts nor reload callbacks
//! can acquire a second editor or write a second shared reading position.
use super::book_composition_editor_prepare::{
    self as preparation, EditorSourceInput, PreparedEditorSource,
};
use super::book_composition_runtime::{ProjectionOrigin, ViewerBookComposition};
use super::{App, ViewerContextId};
use crate::book_composition::*;
use crate::book_resume_db::{PageIdentity, ReadingMeterValue, ReadingPosition};
use crate::book_scope::BookScopeKey;
use crate::ui_composition_editor as ui;
use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompositionEditorOwner {
    pub(crate) context_id: ViewerContextId,
    pub(crate) scope: BookScopeKey,
    pub(crate) request_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompositionEditorCloseReason {
    Cancel,
    OwnerClosed,
    RemoteAcquire,
    Shutdown,
}

pub(crate) struct EditorSession {
    input: EditorSourceInput,
    prepared: Result<PreparedEditorSource, String>,
    ui: ui::EditorUiState,
    result_worker: Option<preparation::EditorResultWorker>,
    error: Option<String>,
    saved: bool,
    pages: Vec<CompositionPageRef>,
    view: ui::EditorView,
    thumbnails: Option<super::book_composition_editor_thumbnails::EditorThumbnails>,
    textures: std::collections::HashMap<CompositionPageRef, egui::TextureHandle>,
    texture_order: std::collections::VecDeque<CompositionPageRef>,
    seed: Option<PageIdentity>,
    old_order: Vec<PageIdentity>,
}
impl std::fmt::Debug for EditorSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorSession")
            .field("input", &self.input)
            .field("prepared", &self.prepared)
            .field("error", &self.error)
            .field("saved", &self.saved)
            .finish()
    }
}
impl EditorSession {
    fn new(
        input: EditorSourceInput,
        prepared: Result<PreparedEditorSource, String>,
        saved: bool,
    ) -> Self {
        let mut pages = prepared
            .as_ref()
            .map(|prepared| {
                prepared
                    .raw_pages
                    .iter()
                    .map(|raw| raw.page.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut seen = pages
            .iter()
            .map(CompositionPageRef::matching_key)
            .collect::<std::collections::HashSet<_>>();
        if let Ok(prepared) = &prepared {
            for page in prepared.draft.record().pages() {
                if seen.insert(page.matching_key()) {
                    pages.push(page.clone());
                }
            }
        }
        let view = empty_editor_view(&input.scope, ui::EditorPhase::Blocked, None);
        let mut session = Self {
            input,
            prepared,
            ui: Default::default(),
            result_worker: None,
            error: None,
            saved,
            pages,
            view,
            thumbnails: None,
            textures: Default::default(),
            texture_order: Default::default(),
            seed: None,
            old_order: vec![],
        };
        session.refresh_view();
        session
    }
    fn refresh_view(&mut self) {
        self.view = editor_view(self);
    }
    fn poll_results(&mut self) {
        let Some(worker) = &self.result_worker else {
            return;
        };
        let Ok(prepared) = &mut self.prepared else {
            return;
        };
        let mut changed = false;
        while let Ok((generation, result)) = worker.receiver.try_recv() {
            if generation != prepared.draft.generation() {
                continue;
            }
            match result {
                Ok(result) => {
                    prepared.result = result;
                    self.error = None;
                }
                Err(error) => {
                    crate::logger::log(format!("composition editor result: {error}"));
                    self.error = Some(error);
                }
            }
            changed = true;
        }
        if changed {
            self.refresh_view();
        }
    }
}

#[derive(Debug, Default)]
pub(crate) enum CompositionEditorState {
    #[default]
    Closed,
    Preparing {
        input: Box<EditorSourceInput>,
        receiver: mpsc::Receiver<Result<Box<EditorSession>, String>>,
    },
    Editing(Box<EditorSession>),
    Saving(Box<EditorSession>),
    Reloading,
}

enum ResumeSeed {
    Page(PageIdentity),
    Existing(mpsc::Receiver<crate::book_resume_db::PositionReadReply>),
}
enum ResumeWriteIntent {
    Keep,
    Reanchor {
        owner: CompositionEditorOwner,
        resume_key: std::path::PathBuf,
        seed: ResumeSeed,
        old_order: Vec<PageIdentity>,
    },
}
struct ResumeCommit {
    key: std::path::PathBuf,
    position: ReadingPosition,
    meter: ReadingMeterValue,
}
struct SaveCompletion {
    status: Result<Option<ResumeCommit>, String>,
}
struct SentSaveTicket {
    owner: CompositionEditorOwner,
    receiver: mpsc::Receiver<SaveCompletion>,
}
#[derive(Default)]
pub(crate) struct CompositionEditorOperations {
    pub(crate) owner: Option<CompositionEditorOwner>,
    next_request: u64,
    tickets: Vec<SentSaveTicket>,
    exit: EditorExitRequest,
}
#[derive(Default)]
enum EditorExitRequest {
    #[default]
    None,
    RootClose,
}

impl App {
    pub(crate) fn composition_editor_modal_active(&self) -> bool {
        self.composition_editor_operations.owner.is_some()
    }
    pub(crate) fn composition_editor_writes_pending(&self) -> bool {
        !self.composition_editor_operations.tickets.is_empty()
    }
    pub(crate) fn composition_editor_acquire_ready(&self) -> bool {
        !self.composition_editor_modal_active() && !self.composition_editor_writes_pending()
    }
    pub(super) fn composition_editor_scope_write_pending(
        &self,
        scope: Option<&BookScopeKey>,
    ) -> bool {
        scope.is_some_and(|scope| {
            self.composition_editor_operations
                .tickets
                .iter()
                .any(|t| &t.owner.scope == scope)
        })
    }
    pub(crate) fn composition_editor_available(&self) -> bool {
        // Keep the existing Book entrance; unsupported image folders show only
        // guidance and never acquire an editor/save owner.
        if matches!(
            &self.projection_recipe,
            super::book_materialization::ProjectionRecipe::Book(
                super::book_materialization::BookScope::ImageFolder(_)
            )
        ) {
            return true;
        }
        self.composition_scope_domain().is_some()
            && self.book_composition_service.is_some()
            && !self.book_composition.is_pending()
            && !matches!(
                &self.book_composition,
                ViewerBookComposition::Accepted {
                    layout: CompositionProjectionOutcome::Unsupported { .. },
                    ..
                }
            )
    }
    fn editor_source_input(
        &mut self,
        scope: BookScopeKey,
        domain: PageDomain,
    ) -> EditorSourceInput {
        let source = match &self.book_composition {
            ViewerBookComposition::Accepted { source, .. } => source.clone(),
            _ => self.composition_source(),
        };
        let mut rules = self.accepted_spread_shift_rules();
        if !rules.mode.is_spread() {
            rules.mode = if self.settings.default_spread_mode.has_cover() {
                crate::settings::SpreadMode::LtrCover
            } else {
                crate::settings::SpreadMode::Ltr
            }
            .with_reading_direction(self.reading_direction);
        }
        let shift = match &self.spread_shift.state {
            super::spread_shift_owner::ViewerShift::Book(
                crate::spread_shift::SpreadShiftState::Adjusted { anchor_ref, .. },
            ) => Some(anchor_ref.clone()),
            _ => None,
        };
        let rotations = self
            .rotation_cache
            .iter()
            .filter_map(|(&index, &rotation)| {
                let identity = self.items.get(index).and_then(|item| {
                    PageIdentity::for_item(item, self.archive_source_override.as_deref())
                })?;
                Some((composition_page_from_identity(&scope, &identity)?, rotation))
            })
            .collect();
        EditorSourceInput {
            password: self.pdf_open_password(&scope.logical_root),
            scope,
            domain,
            source,
            policy: self.settings.file_type_visibility.clone(),
            placement: self.settings.grid_display_order.clone(),
            sort: super::BOOK_READING_PAGE_ORDER,
            rules,
            shift,
            rotations,
            data_dir: crate::data_dir::get(),
            raw_executor: self.raw_develop_executor.clone(),
            initial_binding: (!self.spread_mode.is_spread()).then_some(
                preparation::InitialEditorBinding {
                    default_mode: self.settings.default_spread_mode,
                    default_direction: self.settings.default_reading_direction,
                    follow_document_direction: self.settings.follow_document_reading_direction,
                },
            ),
        }
    }
    pub(crate) fn start_local_composition_editor(&mut self, ctx: &egui::Context) -> bool {
        if self.composition_editor_modal_active()
            || self
                .modal_dialog_block_reason()
                .is_some_and(|reason| reason != "grid_context_menu")
            || self.document_open_modal_admission_blocked()
            || !self.composition_editor_available()
        {
            return false;
        }
        let Some((scope, domain)) = self.composition_scope_domain() else {
            if matches!(
                &self.projection_recipe,
                super::book_materialization::ProjectionRecipe::Book(
                    super::book_materialization::BookScope::ImageFolder(_)
                )
            ) {
                if let super::book_materialization::ProjectionRecipe::Book(
                    super::book_materialization::BookScope::ImageFolder(path),
                ) = &self.projection_recipe
                {
                    return self.start_image_folder_zip(ctx, path.clone(), true);
                }
            }
            return false;
        };
        let input = self.editor_source_input(scope.clone(), domain);
        let old_order = self
            .items
            .iter()
            .filter_map(|item| {
                PageIdentity::for_item(item, self.archive_source_override.as_deref())
            })
            .collect::<Vec<_>>();
        let current_seed = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .and_then(|item| PageIdentity::for_item(item, self.archive_source_override.as_deref()));
        let existing_position = if current_seed.is_none() {
            self.book_resume_recording_folder().and_then(|key| {
                self.book_resume_writer.as_ref().map(|writer| {
                    writer.read_position_after(
                        &crate::path_key::normalize(key),
                        writer.watermarks().admitted,
                    )
                })
            })
        } else {
            None
        };
        self.composition_editor_operations.next_request += 1;
        let owner = CompositionEditorOwner {
            context_id: self.projected_viewer_context_id(),
            scope,
            request_id: self.composition_editor_operations.next_request,
        };
        let (tx, rx) = mpsc::channel();
        // Clone the potentially large rotation snapshot before the session lock.
        let preparing_input = Box::new(input.clone());
        let handle = self.remote_session_handle().cloned();
        let adopt = || {
            self.composition_editor_operations.owner = Some(owner.clone());
            self.composition_editor = CompositionEditorState::Preparing {
                input: preparing_input,
                receiver: rx,
            };
        };
        let accepted = if let Some(handle) = handle {
            handle.with_local_composition_admission(adopt).is_ok()
        } else {
            adopt();
            true
        };
        if !accepted {
            return false;
        }
        self.retire_editor_background_input(ctx);
        let service = self.book_composition_service.clone().unwrap();
        let wake = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("composition-editor-prepare".into())
            .spawn(move || {
                let result = service
                    .prepare(input.scope.clone(), input.domain.clone())
                    .recv()
                    .map_err(|_| "本の構成を読み込めませんでした".to_string())
                    .and_then(|reply| {
                        if reply.token != service.snapshot().token_for(&input.scope) {
                            return Err("本の構成が変更されています。読み直してください".into());
                        }
                        let saved = reply.status.map_err(|_| {
                            "構成を読み取れません。保存データを保持しています".to_string()
                        });
                        let saved_exists = saved.as_ref().is_ok_and(|record| record.is_some());
                        let prepared = saved.and_then(|record| {
                            preparation::prepare(&input, record.as_ref().map(|r| &r.composition))
                        });
                        let mut session = EditorSession::new(input, prepared, saved_exists);
                        session.old_order = old_order;
                        session.seed = current_seed.or_else(|| {
                            existing_position
                                .and_then(|receiver| receiver.recv().ok())
                                .filter(|reply| reply.status.is_ok())
                                .and_then(|reply| reply.position)
                                .and_then(|position| {
                                    position.identity.or_else(|| {
                                        session
                                            .prepared
                                            .as_ref()
                                            .ok()
                                            .and_then(|prepared| {
                                                prepared
                                                    .raw_pages
                                                    .iter()
                                                    .find(|raw| raw.raw_index == position.page)
                                            })
                                            .map(|raw| {
                                                raw.page.resume_identity(
                                                    &session.input.scope.logical_root,
                                                )
                                            })
                                    })
                                })
                        });
                        session.seed = session.seed.filter(|identity| {
                            composition_page_from_identity(&session.input.scope, identity)
                                .is_some_and(|page| {
                                    session.prepared.as_ref().is_ok_and(|prepared| {
                                        prepared.raw_pages.iter().any(|raw| {
                                            raw.page.matching_key() == page.matching_key()
                                        })
                                    })
                                })
                        });
                        if let Err(error) = &session.prepared {
                            crate::logger::log(format!("composition editor prepare: {error}"));
                        }
                        if let Ok(prepared) = &session.prepared {
                            session.result_worker = Some(preparation::EditorResultWorker::spawn(
                                prepared.result_source.clone(),
                                wake.clone(),
                            )?);
                            session.thumbnails = Some(
                                super::book_composition_editor_thumbnails::EditorThumbnails::spawn(
                                    session.input.clone(),
                                    prepared.rows.clone(),
                                    wake.clone(),
                                )?,
                            );
                        }
                        session.refresh_view();
                        Ok(Box::new(session))
                    });
                let _ = tx.send(result);
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
        if let Err(error) = spawn {
            crate::logger::log(format!("composition editor prepare: {error}"));
            self.close_local_composition_editor(CompositionEditorCloseReason::Cancel);
            self.show_feedback_toast("本の構成を準備できませんでした".into());
            return false;
        }
        ctx.request_repaint();
        true
    }
    fn retire_editor_background_input(&mut self, ctx: &egui::Context) {
        self.spread_popup_open = false;
        self.fit_popup_open = false;
        ctx.input_mut(|input| {
            input.raw_scroll_delta = egui::Vec2::ZERO;
            input.smooth_scroll_delta = egui::Vec2::ZERO;
            input.events.clear();
        });
    }
    pub(crate) fn close_composition_editor_for_context(&mut self, id: ViewerContextId) {
        if self
            .composition_editor_operations
            .owner
            .as_ref()
            .is_some_and(|owner| owner.context_id == id)
        {
            self.close_local_composition_editor(CompositionEditorCloseReason::OwnerClosed);
        }
    }
    pub(crate) fn close_local_composition_editor(&mut self, reason: CompositionEditorCloseReason) {
        let Some(owner) = self.composition_editor_operations.owner.take() else {
            return;
        };
        let _ = self.with_viewer_context(owner.context_id, |app| {
            app.composition_editor = CompositionEditorState::Closed;
        });
        if reason == CompositionEditorCloseReason::RemoteAcquire
            && !self.composition_editor_writes_pending()
        {
            self.show_feedback_toast("Remoteからの操作に切り替えたため、本の構成の編集を閉じました。未保存の変更は保存されません".into());
        }
    }
    pub(super) fn defer_composition_editor_root_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) {
            self.close_local_composition_editor(CompositionEditorCloseReason::Shutdown);
            if self.composition_editor_writes_pending() {
                self.composition_editor_operations.exit = EditorExitRequest::RootClose;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if !self.composition_editor_writes_pending()
            && matches!(
                self.composition_editor_operations.exit,
                EditorExitRequest::RootClose
            )
        {
            self.composition_editor_operations.exit = EditorExitRequest::None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    pub(super) fn poll_composition_editor_operations(&mut self, ctx: &egui::Context) {
        if self.remote_session_blocks_local_control() {
            self.close_local_composition_editor(CompositionEditorCloseReason::RemoteAcquire);
        }
        let tickets = std::mem::take(&mut self.composition_editor_operations.tickets);
        for ticket in tickets {
            let completion = match ticket.receiver.try_recv() {
                Ok(reply) => reply,
                Err(mpsc::TryRecvError::Empty) => {
                    self.composition_editor_operations.tickets.push(ticket);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => SaveCompletion {
                    status: Err("本の構成の保存処理が終了しました".into()),
                },
            };
            let still_owned =
                self.composition_editor_operations.owner.as_ref() == Some(&ticket.owner);
            match completion.status {
                Ok(resume) => {
                    // This is the sole receipt consumer, before service publications start reloads.
                    if let Some(resume) = resume {
                        let recorded = self.with_viewer_context(ticket.owner.context_id, |app| {
                            app.persist_book_resume_with_identity(
                                resume.key.clone(),
                                resume.position.page,
                                Some(resume.meter),
                                resume.position.identity.clone(),
                            );
                        });
                        // A sent receipt remains authoritative after its viewer closes.
                        if recorded.is_err()
                            && let Some(writer) = self.book_resume_writer.as_ref()
                            && !writer.record_identity(
                                &resume.key,
                                resume.position.page,
                                Some(resume.meter),
                                resume.position.identity,
                            )
                        {
                            crate::logger::log("composition editor: resume write unavailable");
                            self.show_feedback_toast("読書位置を保存できませんでした".into());
                        }
                    }
                    if still_owned {
                        let _ = self.with_viewer_context(ticket.owner.context_id, |app| {
                            app.composition_editor = CompositionEditorState::Reloading
                        });
                    } else {
                        self.show_feedback_toast(if self.remote_session_blocks_local_control() {
                            "本の構成を保存し、Remoteからの操作に切り替えました".into()
                        } else {
                            "本の構成を保存しました".into()
                        });
                    }
                }
                Err(error) => {
                    crate::logger::log(format!("composition editor save: {error}"));
                    self.show_feedback_toast(error.clone());
                    if still_owned {
                        let _ = self.with_viewer_context(ticket.owner.context_id, |app| {
                            let state = std::mem::take(&mut app.composition_editor);
                            app.composition_editor = match state {
                                CompositionEditorState::Saving(mut session) => {
                                    session.error = Some(error);
                                    session.refresh_view();
                                    CompositionEditorState::Editing(session)
                                }
                                state => state,
                            };
                        });
                    }
                }
            }
            ctx.request_repaint();
        }
        if let Some(owner) = self.composition_editor_operations.owner.clone() {
            let result = self.with_viewer_context(owner.context_id, |app| {
                let state = std::mem::take(&mut app.composition_editor);
                app.composition_editor = match state {
                    CompositionEditorState::Preparing { input, receiver } => {
                        match receiver.try_recv() {
                            Ok(Ok(session)) => CompositionEditorState::Editing(session),
                            Ok(Err(error)) => CompositionEditorState::Editing(Box::new(
                                EditorSession::new(*input, Err(error), false),
                            )),
                            Err(mpsc::TryRecvError::Empty) => {
                                CompositionEditorState::Preparing { input, receiver }
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                CompositionEditorState::Editing(Box::new(EditorSession::new(
                                    *input,
                                    Err("本のページを準備できませんでした".into()),
                                    false,
                                )))
                            }
                        }
                    }
                    CompositionEditorState::Reloading if !app.book_composition.is_pending() => {
                        // Start/poll the current publication even for main-only/parked editors.
                        app.poll_book_composition_read(ctx);
                        if app.book_composition.is_pending() {
                            CompositionEditorState::Reloading
                        } else {
                            CompositionEditorState::Closed
                        }
                    }
                    CompositionEditorState::Editing(mut session) => {
                        session.poll_results();
                        CompositionEditorState::Editing(session)
                    }
                    state => state,
                };
                matches!(app.composition_editor, CompositionEditorState::Closed)
            });
            if result != Ok(false) {
                self.composition_editor_operations.owner = None;
            }
        }
        if self.composition_editor_modal_active() || self.composition_editor_writes_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn capture_editor_resume_intent(&self, owner: &CompositionEditorOwner) -> ResumeWriteIntent {
        let Some(key) = self
            .book_resume_recording_folder()
            .map(std::path::Path::to_path_buf)
        else {
            return ResumeWriteIntent::Keep;
        };
        if self
            .composition_scope_domain()
            .as_ref()
            .map(|(scope, _)| scope)
            != Some(&owner.scope)
        {
            return ResumeWriteIntent::Keep;
        }
        let old_order = self
            .items
            .iter()
            .filter_map(|item| {
                PageIdentity::for_item(item, self.archive_source_override.as_deref())
            })
            .collect();
        let seed = if let Some(identity) = self
            .fullscreen_idx
            .and_then(|index| self.items.get(index))
            .and_then(|item| PageIdentity::for_item(item, self.archive_source_override.as_deref()))
        {
            ResumeSeed::Page(identity)
        } else if let CompositionEditorState::Editing(session) = &self.composition_editor {
            let Some(seed) = session.seed.clone() else {
                return ResumeWriteIntent::Keep;
            };
            ResumeSeed::Page(seed)
        } else {
            return ResumeWriteIntent::Keep;
        };
        ResumeWriteIntent::Reanchor {
            owner: owner.clone(),
            resume_key: key,
            seed,
            old_order,
        }
    }
    fn save_local_composition_editor(&mut self, ctx: &egui::Context, remove: bool) {
        if let CompositionEditorState::Editing(session) = &mut self.composition_editor {
            session.poll_results();
        }
        let Some(owner) = self.composition_editor_operations.owner.clone() else {
            return;
        };
        if owner.context_id != self.projected_viewer_context_id() {
            return;
        }
        if self.book_append_undo_in_progress() {
            self.admit_book_append_followup_mutation();
            return;
        }
        let intent = self.capture_editor_resume_intent(&owner);
        let state = std::mem::take(&mut self.composition_editor);
        let CompositionEditorState::Editing(session) = state else {
            self.composition_editor = state;
            return;
        };
        let Ok(prepared) = &session.prepared else {
            self.composition_editor = CompositionEditorState::Editing(session);
            return;
        };
        if !remove && prepared.result.generation != prepared.draft.generation() {
            self.composition_editor = CompositionEditorState::Editing(session);
            return;
        }
        let input = session.input.clone();
        let fingerprint = prepared.fingerprint.clone();
        let raw_pages = prepared.raw_pages.clone();
        let record = (!remove).then(|| prepared.draft.record().clone());
        let service = self.book_composition_service.clone().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let wake = ctx.clone();
        // Create the worker outside the lifecycle mutex. The critical section
        // below only registers a ticket and sends on this unbounded channel.
        let spawn = std::thread::Builder::new()
            .name("composition-editor-save".into())
            .spawn(move || {
                if request_rx.recv().is_err() {
                    return;
                }
                let result = (|| {
                    preparation::verify_source(&input, &fingerprint)?;
                    let rows = preparation::project_record(&input, record.as_ref())?;
                    let resume = resolve_editor_resume(&intent, &rows, &input, &raw_pages)?;
                    let reply = if let Some(record) = record {
                        service.save(input.scope.clone(), input.domain.clone(), record)
                    } else {
                        service.remove(input.scope.clone(), input.domain.clone())
                    };
                    reply
                        .recv()
                        .map_err(|_| "本の構成を保存できませんでした".to_string())?
                        .status?;
                    Ok(resume)
                })();
                let _ = reply_tx.send(SaveCompletion { status: result });
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
        if let Err(error) = spawn {
            self.composition_editor = CompositionEditorState::Editing(session);
            self.show_feedback_toast(format!("本の構成を保存できませんでした: {error}"));
            return;
        }
        let handle = self.remote_session_handle().cloned();
        let mut retained = Some(session);
        let adopt = || {
            // Session admission has succeeded. Invalidate before publishing a
            // sent ticket or letting the worker begin its source/store work.
            if !self.admit_book_append_followup_mutation() {
                return false;
            }
            let ticket = SentSaveTicket {
                owner: owner.clone(),
                receiver: reply_rx,
            };
            self.composition_editor_operations.tickets.push(ticket);
            if request_tx.send(()).is_err() {
                self.composition_editor_operations.tickets.pop();
                false
            } else {
                self.composition_editor = CompositionEditorState::Saving(retained.take().unwrap());
                true
            }
        };
        let admitted = if let Some(handle) = handle {
            handle
                .with_local_composition_admission(adopt)
                .unwrap_or(false)
        } else {
            adopt()
        };
        if !admitted {
            if let Some(mut session) = retained {
                session.error = Some("本の構成を保存できませんでした".into());
                session.refresh_view();
                self.composition_editor = CompositionEditorState::Editing(session);
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn show_local_composition_editor(&mut self, ctx: &egui::Context) {
        let Some(owner) = self.composition_editor_operations.owner.clone() else {
            return;
        };
        if owner.context_id != self.projected_viewer_context_id() {
            ctx.memory_mut(|memory| {
                memory.set_modal_layer(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("composition-editor-owner-notice"),
                ))
            });
            egui::Area::new(egui::Id::new("composition-editor-owner-notice"))
                .order(egui::Order::Foreground)
                .fixed_pos(ctx.content_rect().min + egui::vec2(12.0, 12.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label("別の窓で本の構成を編集中です");
                        ui.label("編集中の窓で保存またはキャンセルしてください");
                    });
                });
            return;
        }
        let marker = egui::Id::new(("composition-editor-drawn", owner.request_id));
        let frame = ctx.cumulative_frame_nr();
        if ctx.data(|data| data.get_temp::<u64>(marker)) == Some(frame) {
            return;
        }
        ctx.data_mut(|data| data.insert_temp(marker, frame));
        let keys = ui::DialogKeys {
            enter: self.dialog_enter_pressed(ctx),
            escape: self.dialog_escape_pressed(ctx),
        };
        let mut state = std::mem::take(&mut self.composition_editor);
        let actions = match &mut state {
            CompositionEditorState::Editing(session) => {
                session.poll_results();
                if let Some(thumbnails) = &mut session.thumbnails {
                    for (page, texture) in thumbnails.poll() {
                        let key = page.matching_key();
                        if !session.textures.contains_key(&key) {
                            session.texture_order.push_back(key.clone());
                        }
                        let id = session
                            .pages
                            .iter()
                            .position(|page| page.matching_key() == key);
                        if let Some(id) = id {
                            session.view.pages[id].thumbnail =
                                Some((texture.id(), texture.size_vec2()));
                        }
                        session.textures.insert(key, texture);
                        while session.textures.len() > 128 {
                            if let Some(old) = session.texture_order.pop_front() {
                                session.textures.remove(&old);
                                if let Some(id) = session
                                    .pages
                                    .iter()
                                    .position(|page| page.matching_key() == old)
                                {
                                    session.view.pages[id].thumbnail = None;
                                }
                            }
                        }
                    }
                }
                ui::show_window(
                    ctx,
                    egui::Id::new(("book-composition-editor", owner.request_id)),
                    &session.view,
                    &mut session.ui,
                    keys,
                )
            }
            phase => {
                let view = empty_editor_view(
                    &owner.scope,
                    match phase {
                        CompositionEditorState::Preparing { .. } => ui::EditorPhase::Preparing,
                        CompositionEditorState::Saving(_) => ui::EditorPhase::Saving,
                        _ => ui::EditorPhase::Reloading,
                    },
                    None,
                );
                ui::show_window(
                    ctx,
                    egui::Id::new(("book-composition-editor", owner.request_id)),
                    &view,
                    &mut ui::EditorUiState::default(),
                    keys,
                )
            }
        };
        self.composition_editor = state;
        for action in actions {
            self.apply_editor_ui_action(ctx, action);
        }
    }
    fn apply_editor_ui_action(&mut self, ctx: &egui::Context, action: ui::EditorUiAction) {
        if self.remote_session_blocks_local_control() {
            self.close_local_composition_editor(CompositionEditorCloseReason::RemoteAcquire);
            return;
        }
        use ui::EditorUiAction as A;
        match action {
            A::Cancel => self.close_local_composition_editor(CompositionEditorCloseReason::Cancel),
            A::Save => self.save_local_composition_editor(ctx, false),
            A::Remove => self.save_local_composition_editor(ctx, true),
            A::Reload => {
                self.close_local_composition_editor(CompositionEditorCloseReason::Cancel);
                self.start_local_composition_editor(ctx);
            }
            A::IgnoreComposition => {
                if let Some(owner) = self.composition_editor_operations.owner.clone() {
                    let source = match &self.composition_editor {
                        CompositionEditorState::Editing(session) => session.input.source.clone(),
                        _ => self.composition_source(),
                    };
                    self.close_local_composition_editor(CompositionEditorCloseReason::Cancel);
                    if let Some(service) = &self.book_composition_service {
                        let token = service.snapshot().token_for(&owner.scope);
                        self.begin_book_projection(
                            owner.scope,
                            token,
                            None,
                            ProjectionOrigin::IgnoreThisOpen { source },
                            ctx,
                        );
                    }
                }
            }
            A::RequestThumbnails(pages) => self.request_editor_thumbnails(&pages),
            action => {
                if self
                    .composition_editor_operations
                    .owner
                    .as_ref()
                    .is_none_or(|owner| owner.context_id != self.projected_viewer_context_id())
                {
                    return;
                }
                if self.book_append_undo_in_progress() {
                    self.admit_book_append_followup_mutation();
                    return;
                }
                let state = std::mem::take(&mut self.composition_editor);
                let CompositionEditorState::Editing(mut session) = state else {
                    self.composition_editor = state;
                    return;
                };
                let Ok(prepared) = &mut session.prepared else {
                    self.composition_editor = CompositionEditorState::Editing(session);
                    return;
                };
                let draft = &mut prepared.draft;
                let selected = session
                    .ui
                    .selected_pages
                    .first()
                    .and_then(|&id| session.pages.get(id))
                    .cloned()
                    .map(DraftSelection::Page);
                if !matches!(action, A::Undo | A::Redo) {
                    let _ = draft.select(selected);
                }
                let before = draft.generation();
                let result = match action {
                    A::Undo => {
                        draft.undo();
                        Ok(())
                    }
                    A::Redo => {
                        draft.redo();
                        Ok(())
                    }
                    action => editor_page_edits(draft, &session.pages, &action).and_then(|edits| {
                        draft
                            .apply_edits(edits)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    }),
                };
                if let Err(error) = result {
                    session.error = Some(format!("構成を変更できません: {error}"));
                } else {
                    session.error = None;
                }
                session.ui.selected_pages.clear();
                if let Some(DraftSelection::Page(page)) = draft.selection()
                    && let Some(id) = session
                        .pages
                        .iter()
                        .position(|p| p.matching_key() == page.matching_key())
                {
                    session.ui.selected_pages.push(id);
                }
                if before != draft.generation() {
                    self.admit_book_append_followup_mutation();
                }
                if before != draft.generation()
                    && let Some(worker) = &session.result_worker
                {
                    worker.request(draft.generation(), draft.record().clone());
                }
                session.refresh_view();
                self.composition_editor = CompositionEditorState::Editing(session);
            }
        }
    }
    fn request_editor_thumbnails(&mut self, pages: &[usize]) {
        let CompositionEditorState::Editing(session) = &mut self.composition_editor else {
            return;
        };
        let Some(thumbnails) = &mut session.thumbnails else {
            return;
        };
        let pages = pages
            .iter()
            .take(24)
            .filter_map(|&id| session.pages.get(id))
            .filter(|page| !session.textures.contains_key(&page.matching_key()))
            .cloned()
            .collect();
        thumbnails.request(pages);
    }
}

fn empty_editor_view(
    scope: &BookScopeKey,
    phase: ui::EditorPhase,
    error: Option<String>,
) -> ui::EditorView {
    ui::EditorView {
        book_name: scope
            .logical_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        saved: false,
        phase,
        direction: ReadingDirection::Ltr,
        cover_enabled: false,
        cover: None,
        pages: vec![],
        page_order: vec![],
        results_updating: false,
        units: vec![],
        can_undo: false,
        can_redo: false,
        can_save: false,
        can_remove: false,
        landing: "同じ本を開いている他の窓も移動することがあります".into(),
        error,
    }
}
fn editor_view(session: &EditorSession) -> ui::EditorView {
    let Ok(prepared) = &session.prepared else {
        return empty_editor_view(
            &session.input.scope,
            ui::EditorPhase::Blocked,
            session.prepared.as_ref().err().cloned(),
        );
    };
    let record = prepared.draft.record();
    let references = &session.pages;
    let ids = references
        .iter()
        .enumerate()
        .map(|(id, page)| (page.matching_key(), id))
        .collect::<std::collections::HashMap<_, _>>();
    let id = |page: &CompositionPageRef| ids[&page.matching_key()];
    let manifest = prepared
        .draft
        .manifest()
        .iter()
        .map(|m| (m.page.matching_key(), m))
        .collect::<std::collections::HashMap<_, _>>();
    let raw_indices = prepared
        .raw_pages
        .iter()
        .map(|raw| (raw.page.matching_key(), raw.raw_index))
        .collect::<std::collections::HashMap<_, _>>();
    let hidden = record
        .hidden_pages
        .iter()
        .map(CompositionPageRef::matching_key)
        .collect::<std::collections::HashSet<_>>();
    let overrides: std::collections::HashMap<_, _> = record
        .page_overrides
        .iter()
        .map(|entry| (entry.page.matching_key(), entry.directive))
        .collect();
    let pages = references
        .iter()
        .enumerate()
        .map(|(id, page)| {
            let entry = manifest.get(&page.matching_key());
            let physical_number = raw_indices
                .get(&page.matching_key())
                .map(|index| index + 1)
                .unwrap_or(0);
            let name = match page {
                CompositionPageRef::RelativePath { path } => path.clone(),
                CompositionPageRef::ArchiveEntry { entry_name } => entry_name.clone(),
                CompositionPageRef::PdfPage { page_num } => format!("{}ページ", page_num + 1),
            };
            ui::EditorPageView {
                id,
                name,
                physical_number,
                hidden: hidden.contains(&page.matching_key()),
                missing: entry.is_none(),
                excluded_by_policy: entry
                    .is_some_and(|m| m.availability == ManifestAvailability::ExcludedByPolicy),
                directive: overrides.get(&page.matching_key()).copied(),
                thumbnail: session
                    .textures
                    .get(&page.matching_key())
                    .map(|texture| (texture.id(), texture.size_vec2())),
            }
        })
        .collect();
    let visible = |page: &CompositionPageRef| {
        manifest
            .get(&page.matching_key())
            .is_some_and(|m| matches!(m.availability, ManifestAvailability::Present { .. }))
            && !hidden.contains(&page.matching_key())
    };
    let units = prepared
        .result
        .groups
        .iter()
        .filter(|group| match group {
            preparation::DerivedEditorGroup::Pair(first, second) => {
                visible(first) && visible(second)
            }
            preparation::DerivedEditorGroup::Single {
                page, companion, ..
            } => {
                visible(page)
                    && match companion {
                        preparation::DerivedCompanion::Cover(cover) => visible(cover),
                        preparation::DerivedCompanion::Vacant
                        | preparation::DerivedCompanion::Blank => true,
                    }
            }
        })
        .map(|group| match group {
            preparation::DerivedEditorGroup::Pair(first, second) => ui::EditorUnitView::Pair {
                first: id(first),
                second: id(second),
            },
            preparation::DerivedEditorGroup::Single {
                page,
                placement,
                companion,
            } => ui::EditorUnitView::Single {
                page: id(page),
                placement: *placement,
                companion: match companion {
                    preparation::DerivedCompanion::Vacant => ui::EditorCompanionView::Vacant,
                    preparation::DerivedCompanion::Blank => ui::EditorCompanionView::Blank,
                    preparation::DerivedCompanion::Cover(page) => {
                        ui::EditorCompanionView::Cover(id(page))
                    }
                },
            },
        })
        .collect();
    let results_updating = prepared.result.generation != prepared.draft.generation();
    ui::EditorView {
        book_name: session
            .input
            .scope
            .logical_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        saved: session.saved,
        phase: ui::EditorPhase::Editing,
        direction: record.direction,
        cover_enabled: record.cover_enabled,
        cover: prepared
            .result
            .cover
            .as_ref()
            .filter(|page| visible(page))
            .map(id),
        page_order: record.page_order.iter().map(id).collect(),
        results_updating,
        pages,
        units,
        can_undo: prepared.draft.can_undo(),
        can_redo: prepared.draft.can_redo(),
        can_save: !results_updating,
        can_remove: session.saved,
        landing: editor_landing_preview(session, record),
        error: session.error.clone(),
    }
}
fn page_label(page: &CompositionPageRef) -> String {
    match page {
        CompositionPageRef::RelativePath { path } => path.clone(),
        CompositionPageRef::ArchiveEntry { entry_name } => entry_name.clone(),
        CompositionPageRef::PdfPage { page_num } => format!("{}ページ", page_num + 1),
    }
}
fn editor_landing_preview(session: &EditorSession, record: &BookComposition) -> String {
    let suffix = "。同じ本を開いている他の窓も移動することがあります";
    let Some(seed) = &session.seed else {
        return format!("読書位置は変更しません{suffix}");
    };
    let Ok(prepared) = &session.prepared else {
        return format!("読書位置は変更しません{suffix}");
    };
    let Ok(resolved) =
        resolve_composition(record, &session.input.domain, prepared.draft.manifest())
    else {
        return format!("表示できるページがありません{suffix}");
    };
    let find = |identity: &PageIdentity| {
        composition_page_from_identity(&session.input.scope, identity)
            .filter(|page| matches!(resolved.page_status(page), PageStatus::Visible { .. }))
    };
    let target = find(seed)
        .or_else(|| {
            session
                .old_order
                .iter()
                .position(|old| old == seed)
                .and_then(|index| {
                    session.old_order[index + 1..]
                        .iter()
                        .find_map(find)
                        .or_else(|| session.old_order[..index].iter().rev().find_map(find))
                })
        })
        .or_else(|| resolved.visible_pages.first().map(|page| page.page.clone()));
    match target {
        Some(page) => format!("{}{suffix}", page_label(&page)),
        None => format!("表示できるページがありません{suffix}"),
    }
}

fn editor_page_edits(
    draft: &CompositionDraft,
    pages: &[CompositionPageRef],
    action: &ui::EditorUiAction,
) -> Result<Vec<CompositionEdit>, String> {
    use ui::EditorUiAction as A;
    let record = draft.record();
    let page = |id: usize| {
        pages
            .get(id)
            .and_then(|selected| {
                record
                    .pages()
                    .find(|page| page.matching_key() == selected.matching_key())
            })
            .cloned()
            .ok_or_else(|| "ページがありません".to_string())
    };
    let position = |page: &CompositionPageRef| {
        record
            .page_order
            .iter()
            .position(|p| p.matching_key() == page.matching_key())
            .ok_or_else(|| "ページがありません".to_string())
    };
    Ok(match *action {
        A::SetDirection(direction) => vec![CompositionEdit::SetDirection(direction)],
        A::SetCover(enabled) => vec![CompositionEdit::SetCover(enabled)],
        A::SetHidden { page: id, hidden } => vec![CompositionEdit::SetHidden {
            page: page(id)?,
            hidden,
        }],
        A::SetOverride {
            page: id,
            directive,
        } => vec![CompositionEdit::SetOverride {
            page: page(id)?,
            directive,
        }],
        A::MovePage { page: id, before } => {
            let moving = page(id)?;
            let source = position(&moving)?;
            let target = position(&page(before)?)?;
            vec![CompositionEdit::MovePage {
                page: moving,
                to: target.saturating_sub(usize::from(source < target)),
            }]
        }
        A::MovePageStep { page: id, forward } => {
            let moving = page(id)?;
            let from = position(&moving)?;
            vec![CompositionEdit::MovePage {
                page: moving,
                to: if forward {
                    (from + 1).min(record.page_order.len().saturating_sub(1))
                } else {
                    from.saturating_sub(1)
                },
            }]
        }
        _ => vec![],
    })
}

fn resolve_editor_resume(
    intent: &ResumeWriteIntent,
    rows: &super::book_materialization::MaterializedBookRows,
    input: &EditorSourceInput,
    raw_pages: &[CanonicalRawPage],
) -> Result<Option<ResumeCommit>, String> {
    let ResumeWriteIntent::Reanchor {
        owner,
        resume_key,
        seed,
        old_order,
    } = intent
    else {
        return Ok(None);
    };
    if owner.scope != input.scope {
        return Ok(None);
    }
    let seed = match seed {
        ResumeSeed::Page(identity) => Some(identity.clone()),
        ResumeSeed::Existing(receiver) => {
            let reply = receiver
                .recv()
                .map_err(|_| "読書位置を確認できませんでした".to_string())?;
            if reply.status.is_err() {
                return Ok(None);
            }
            reply.position.and_then(|position| {
                position.identity.or_else(|| {
                    raw_pages
                        .iter()
                        .find(|raw| raw.raw_index == position.page)
                        .map(|raw| raw.page.resume_identity(&owner.scope.logical_root))
                })
            })
        }
    };
    let Some(seed) = seed else {
        return Ok(None);
    };
    // An unrelated/missing legacy identity must never create a new position.
    let valid_seed = composition_page_from_identity(&owner.scope, &seed).is_some_and(|page| {
        raw_pages
            .iter()
            .any(|raw| raw.page.matching_key() == page.matching_key())
    });
    if !valid_seed {
        return Ok(None);
    }
    let index = find_editor_landing(&seed, old_order, rows);
    let Some(index) = index else {
        return Ok(None);
    };
    let (position, ordinal, total) = match &rows.composition {
        CompositionProjectionOutcome::Composed(projection) => (
            projection.position_for_projected(index),
            projection.reading_ordinal(index),
            projection.resolved.visible_pages.len(),
        ),
        _ => {
            let identity = rows
                .items
                .get(index)
                .and_then(|item| PageIdentity::for_item(item, None));
            let ordinal = rows.items[..index]
                .iter()
                .filter(|item| PageIdentity::for_item(item, None).is_some())
                .count();
            let total = rows
                .items
                .iter()
                .filter(|item| PageIdentity::for_item(item, None).is_some())
                .count();
            (
                identity.map(|identity| ReadingPosition {
                    page: index,
                    identity: Some(identity),
                }),
                Some(ordinal),
                total,
            )
        }
    };
    Ok(position.zip(ordinal).and_then(|(position, ordinal)| {
        ReadingMeterValue::new(ordinal, total).map(|meter| ResumeCommit {
            key: resume_key.clone(),
            position,
            meter,
        })
    }))
}
pub(super) fn find_editor_landing(
    seed: &PageIdentity,
    old_order: &[PageIdentity],
    rows: &super::book_materialization::MaterializedBookRows,
) -> Option<usize> {
    let find = |identity: &PageIdentity| match &rows.composition {
        CompositionProjectionOutcome::Composed(projection) => {
            match projection.resolve_position(&ReadingPosition {
                page: 0,
                identity: Some(identity.clone()),
            }) {
                PageStatus::Visible { item_index } => Some(item_index),
                _ => None,
            }
        }
        _ => rows
            .items
            .iter()
            .position(|item| PageIdentity::for_item(item, None).as_ref() == Some(identity)),
    };
    find(seed)
        .or_else(|| {
            old_order.iter().position(|p| p == seed).and_then(|pos| {
                old_order[pos + 1..]
                    .iter()
                    .find_map(find)
                    .or_else(|| old_order[..pos].iter().rev().find_map(find))
            })
        })
        .or_else(|| {
            rows.items
                .iter()
                .position(|item| PageIdentity::for_item(item, None).is_some())
        })
}

#[cfg(test)]
mod resume_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "book_composition_editor/metadata_transfer_tests.rs"]
mod metadata_transfer_tests;
