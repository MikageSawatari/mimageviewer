//! One agent UI admission owner for Open, state reads, tags and ratings. Open
//! keeps the existing activation path; metadata targets are prepared on workers.
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use mimageviewer_agent_ipc::{AgentError, ErrorCode, OpenResult, OpenStatus};

use crate::remote_ipc::session::SessionHandle;
use crate::single_instance::ActivationOpenPath;
use crate::ui_request_dispatch::UiRequestDispatch;

const UI_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(test)]
type ValidationHook = dyn Fn(&std::path::Path) + Send + Sync;

struct InstalledUi {
    sender: mpsc::Sender<UiOpenRequest>,
    ctx: egui::Context,
}

pub(crate) struct OpenController {
    session: Option<SessionHandle>,
    installed: Mutex<Option<InstalledUi>>,
    active: AtomicBool,
    #[cfg(test)]
    enqueue_observer: Mutex<Option<mpsc::Sender<()>>>,
    #[cfg(test)]
    validation_hook: Mutex<Option<Arc<ValidationHook>>>,
    #[cfg(test)]
    activation_observer: Mutex<Option<mpsc::Sender<()>>>,
}

struct OpenPermit(Arc<OpenController>);

impl Drop for OpenPermit {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub(crate) enum UiOpenOutcome {
    Admitted,
    Read(mimageviewer_agent_ipc::ViewState),
    Settings(mimageviewer_agent_ipc::SettingsPage),
    Operations(mimageviewer_agent_ipc::OperationsPage),
    StackScriptAdmitted {
        enabled: bool,
    },
    OperationScriptAdmitted {
        snapshot: super::operation_script::AdmissionSnapshot,
        queued: Result<(), String>,
    },
    SoundChainAdmitted(crate::effetune::sound_service::SoundTicket),
    Written(Result<mimageviewer_agent_ipc::Payload, AgentError>),
    Refused(AgentError),
}

pub(crate) enum UiRequestKind {
    Open(ActivationOpenPath),
    GetViewState {
        default_books_root: std::path::PathBuf,
    },
    GetSettings {
        query: Option<String>,
        keys: Option<Vec<String>>,
        page: Option<u32>,
    },
    GetOperations(mimageviewer_agent_ipc::OperationsQuery),
    SetOperations {
        args: mimageviewer_agent_ipc::OperationEdits,
        client_name: String,
    },
    SetSettings {
        changes: serde_json::Map<String, serde_json::Value>,
        client_name: String,
    },
    SetFavorite(super::favorite::PreparedFavorite),
    StackScriptAdmission,
    OperationScript(crate::operation_script::AgentScriptRequest),
    SoundChain {
        request: crate::effetune::sound_service::SoundRequest,
        client_name: String,
        permit: Option<Weak<super::Permit>>,
    },
    TagWrite(super::metadata_write::PreparedTags),
    RatingWrite(super::metadata_write::PreparedRatings),
}

impl From<super::metadata_write::PreparedWrite> for UiRequestKind {
    fn from(write: super::metadata_write::PreparedWrite) -> Self {
        match write {
            super::metadata_write::PreparedWrite::Tags(write) => Self::TagWrite(write),
            super::metadata_write::PreparedWrite::Ratings(write) => Self::RatingWrite(write),
        }
    }
}

pub(crate) struct UiOpenRequest {
    pub(crate) kind: UiRequestKind,
    dispatch: Arc<UiRequestDispatch>,
    reply: mpsc::SyncSender<UiOpenOutcome>,
}

impl UiOpenRequest {
    pub(crate) fn try_claim(&self) -> bool {
        self.dispatch.try_claim()
    }

    pub(crate) fn complete(self, outcome: UiOpenOutcome) {
        let _ = self.reply.send(outcome);
    }

    /// Transfer the prepared request without cloning its parameter document
    /// on the UI thread. The handler still completes in this one UI turn.
    pub(crate) fn complete_with(self, handle: impl FnOnce(UiRequestKind) -> UiOpenOutcome) {
        let _ = self.reply.send(handle(self.kind));
    }
}

impl OpenController {
    pub(crate) fn new(session: Option<SessionHandle>) -> Arc<Self> {
        Arc::new(Self {
            session,
            installed: Mutex::new(None),
            active: AtomicBool::new(false),
            #[cfg(test)]
            enqueue_observer: Mutex::new(None),
            #[cfg(test)]
            validation_hook: Mutex::new(None),
            #[cfg(test)]
            activation_observer: Mutex::new(None),
        })
    }

    pub(crate) fn install(self: &Arc<Self>, ctx: egui::Context) -> mpsc::Receiver<UiOpenRequest> {
        let (sender, receiver) = mpsc::channel();
        *self.installed.lock().unwrap() = Some(InstalledUi { sender, ctx });
        receiver
    }

    #[cfg(test)]
    pub(crate) fn set_enqueue_observer(&self, observer: mpsc::Sender<()>) {
        *self.enqueue_observer.lock().unwrap() = Some(observer);
    }

    #[cfg(test)]
    pub(crate) fn set_validation_hook(&self, hook: Arc<ValidationHook>) {
        *self.validation_hook.lock().unwrap() = Some(hook);
    }

    pub(crate) fn execute(
        self: &Arc<Self>,
        path: String,
        cancel: &AtomicBool,
    ) -> Result<OpenResult, AgentError> {
        self.execute_with_dispatch(path, cancel, Arc::new(UiRequestDispatch::pending()))
    }

    pub(super) fn execute_with_dispatch(
        self: &Arc<Self>,
        path: String,
        cancel: &AtomicBool,
        dispatch: Arc<UiRequestDispatch>,
    ) -> Result<OpenResult, AgentError> {
        self.execute_with_dispatch_timeout(path, cancel, dispatch, UI_REPLY_TIMEOUT)
    }

    #[cfg(test)]
    fn execute_with_timeout(
        self: &Arc<Self>,
        path: String,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<OpenResult, AgentError> {
        self.execute_with_dispatch_timeout(
            path,
            cancel,
            Arc::new(UiRequestDispatch::pending()),
            timeout,
        )
    }

    fn execute_with_dispatch_timeout(
        self: &Arc<Self>,
        path: String,
        cancel: &AtomicBool,
        dispatch: Arc<UiRequestDispatch>,
        timeout: Duration,
    ) -> Result<OpenResult, AgentError> {
        let outcome = self.dispatch_request(cancel, dispatch.clone(), timeout, || {
            let path = super::input_path::validate(&path)?;
            #[cfg(test)]
            {
                let hook = self.validation_hook.lock().unwrap().clone();
                if let Some(hook) = hook {
                    hook(path);
                }
            }
            // Worker-only I/O. Do not let the ordinary resolver walk up to a parent
            // when the requested path is absent, or disclose OS errors and paths.
            if !path.try_exists().map_err(|_| open_failed())? {
                return Err(super::error(
                    ErrorCode::BadRequest,
                    "指定されたファイルまたはフォルダがありません",
                ));
            }
            let activation = ActivationOpenPath::received(path.to_owned(), self.session.as_ref());
            if activation.remote_owned_at_receive {
                return Err(super::error(
                    ErrorCode::Busy,
                    "mIV Remote の操作中は開けません",
                ));
            }
            Ok(UiRequestKind::Open(activation))
        })?;
        outcome_result(outcome)
    }

    pub(super) fn execute_write(
        self: &Arc<Self>,
        command: mimageviewer_agent_ipc::Command,
        ctx: &super::ReadContext,
        client_name: String,
        dispatch: Arc<UiRequestDispatch>,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.dispatch_request(&ctx.cancel, dispatch, UI_REPLY_TIMEOUT, || match command {
            mimageviewer_agent_ipc::Command::SetOperations(args) => {
                args.validate()
                    .map_err(|message| AgentError::new(ErrorCode::BadRequest, message))?;
                ctx.checkpoint()?;
                Ok(UiRequestKind::SetOperations { args, client_name })
            }
            mimageviewer_agent_ipc::Command::SetSettings { changes } => {
                if changes.is_empty() || changes.len() > mimageviewer_agent_ipc::MAX_SETTINGS_KEYS {
                    return Err(super::error(
                        mimageviewer_agent_ipc::ErrorCode::BadRequest,
                        "設定は 1～32 項目で指定してください",
                    ));
                }
                crate::settings_transfer::validate_stage1_paths(&changes).map_err(|message| {
                    super::error(mimageviewer_agent_ipc::ErrorCode::BadRequest, &message)
                })?;
                ctx.checkpoint()?;
                Ok(UiRequestKind::SetSettings {
                    changes,
                    client_name,
                })
            }
            mimageviewer_agent_ipc::Command::SetFavorite {
                path,
                name,
                item_index,
                container_index,
            } => {
                super::favorite::prepare(path, name, item_index, container_index, client_name, ctx)
                    .map(UiRequestKind::SetFavorite)
            }
            other => {
                super::metadata_write::prepare(other, ctx, client_name).map(UiRequestKind::from)
            }
        })
    }

    pub(super) fn execute_operations_read(
        self: &Arc<Self>,
        args: mimageviewer_agent_ipc::OperationsQuery,
        ctx: &super::ReadContext,
    ) -> Result<mimageviewer_agent_ipc::OperationsPage, AgentError> {
        args.validate()
            .map_err(|message| AgentError::new(ErrorCode::BadRequest, message))?;
        ctx.checkpoint()?;
        match self.dispatch_request(
            &ctx.cancel,
            Arc::new(UiRequestDispatch::pending()),
            UI_REPLY_TIMEOUT,
            || Ok(UiRequestKind::GetOperations(args)),
        )? {
            UiOpenOutcome::Operations(page) => Ok(page),
            UiOpenOutcome::Refused(error) => Err(error),
            _ => Err(super::internal()),
        }
    }

    pub(super) fn execute_settings_read(
        self: &Arc<Self>,
        query: Option<String>,
        keys: Option<Vec<String>>,
        page: Option<u32>,
        ctx: &super::ReadContext,
    ) -> Result<mimageviewer_agent_ipc::SettingsPage, AgentError> {
        ctx.checkpoint()?;
        let result = self.dispatch_request(
            &ctx.cancel,
            Arc::new(UiRequestDispatch::pending()),
            UI_REPLY_TIMEOUT,
            || Ok(UiRequestKind::GetSettings { query, keys, page }),
        )?;
        match result {
            UiOpenOutcome::Settings(page) => Ok(page),
            UiOpenOutcome::Refused(error) => Err(error),
            _ => Err(super::internal()),
        }
    }

    pub(super) fn execute_stack_script(
        self: &Arc<Self>,
        source: Option<String>,
        part: Option<u32>,
        expected_sha256: Option<String>,
        ctx: &super::ReadContext,
        client_name: String,
        dispatch: Arc<UiRequestDispatch>,
        changes: &super::changes::ChangeController,
    ) -> Result<UiOpenOutcome, AgentError> {
        super::stack_script::validate_arguments(
            source.as_deref(),
            part,
            expected_sha256.as_deref(),
        )?;
        ctx.checkpoint()?;
        let admission = self.stack_script_admission(&ctx.cancel, dispatch)?;
        match admission {
            UiOpenOutcome::StackScriptAdmitted { enabled } => {
                // The claimed request owns its result. Only this agent worker
                // waits for file work; the UI has already returned admission.
                Ok(UiOpenOutcome::Written(super::stack_script::execute(
                    source,
                    part,
                    expected_sha256,
                    enabled,
                    &client_name,
                    changes,
                )))
            }
            UiOpenOutcome::Refused(error) => Ok(UiOpenOutcome::Refused(error)),
            _ => Err(super::internal()),
        }
    }

    pub(super) fn execute_operation_script(
        self: &Arc<Self>,
        source: Option<String>,
        part: Option<u32>,
        expected_sha256: Option<String>,
        ctx: &super::ReadContext,
        client_name: String,
        dispatch: Arc<UiRequestDispatch>,
        changes: &super::changes::ChangeController,
    ) -> Result<UiOpenOutcome, AgentError> {
        super::operation_script::validate_arguments(
            source.as_deref(),
            part,
            expected_sha256.as_deref(),
        )?;
        let writing = source.is_some();
        if writing {
            super::operation_script::check_write_budget()?;
        }
        ctx.checkpoint()?;
        let (reply, receive) = mpsc::channel();
        let job = crate::operation_script::AgentScriptRequest {
            source,
            expected_sha256: expected_sha256.clone(),
            reply,
        };
        let admission = self.dispatch_request(&ctx.cancel, dispatch, UI_REPLY_TIMEOUT, || {
            Ok(UiRequestKind::OperationScript(job))
        })?;
        match admission {
            UiOpenOutcome::OperationScriptAdmitted { snapshot, queued } => {
                // Only this worker waits. Claimed requests retain their eventual
                // result even if the outer deadline or cancellation has passed.
                let outcome = queued.and_then(|()| {
                    receive.recv().map_err(|_| {
                        "操作のスクリプトのworkerが結果を返さず終了しました".to_owned()
                    })?
                });
                let result = if writing {
                    super::operation_script::complete_write(
                        outcome.and_then(|value| match value {
                            crate::operation_script::AgentScriptOutcome::Write(value) => Ok(value),
                            _ => Err("操作のスクリプトの書き込み結果を確認できません".into()),
                        }),
                        &snapshot,
                        &client_name,
                        changes,
                    )
                } else {
                    match outcome {
                        Ok(crate::operation_script::AgentScriptOutcome::Read(document)) => {
                            super::operation_script::read_result(
                                &document,
                                &snapshot,
                                part,
                                expected_sha256.as_deref(),
                            )
                            .map(mimageviewer_agent_ipc::Payload::OperationScript)
                        }
                        _ => {
                            crate::logger::log("agent_ipc: tool=operation_script read_failed");
                            Err(super::error(
                                ErrorCode::Internal,
                                "操作のスクリプトを読み込めませんでした",
                            ))
                        }
                    }
                };
                Ok(UiOpenOutcome::Written(result))
            }
            UiOpenOutcome::Refused(error) => Ok(UiOpenOutcome::Refused(error)),
            _ => Err(super::internal()),
        }
    }

    pub(super) fn execute_sound_chain(
        self: &Arc<Self>,
        args: mimageviewer_agent_ipc::SoundChainRequest,
        ctx: &super::ReadContext,
        client_name: String,
        dispatch: Arc<UiRequestDispatch>,
        permit: Arc<super::Permit>,
    ) -> Result<UiOpenOutcome, AgentError> {
        let request = super::sound_chain::prepare_request(args)?;
        ctx.checkpoint()?;
        // Keep the strong owner on this worker while the UI decides. Pending
        // queue entries must not retain a slot after cancellation. If claim
        // wins the shared CAS, dispatch_request waits for the UI reply, keeping
        // this owner alive until the UI upgrades it and submits the job.
        let admission = self.dispatch_request(&ctx.cancel, dispatch, UI_REPLY_TIMEOUT, || {
            Ok(UiRequestKind::SoundChain {
                request,
                client_name,
                permit: Some(Arc::downgrade(&permit)),
            })
        })?;
        match admission {
            UiOpenOutcome::SoundChainAdmitted(ticket) => {
                // UI has already submitted the accepted request to the FIFO.
                // Only this worker waits, within the parent tool's deadline.
                let result = super::sound_chain::wire_result(
                    ticket.with_wait_budget(ctx.remaining()).wait(),
                )?;
                Ok(UiOpenOutcome::Written(Ok(
                    mimageviewer_agent_ipc::Payload::SoundChain(result),
                )))
            }
            UiOpenOutcome::Refused(error) => Ok(UiOpenOutcome::Refused(error)),
            _ => Err(super::internal()),
        }
    }

    fn stack_script_admission(
        self: &Arc<Self>,
        cancel: &AtomicBool,
        dispatch: Arc<UiRequestDispatch>,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.dispatch_request(cancel, dispatch, UI_REPLY_TIMEOUT, || {
            Ok(UiRequestKind::StackScriptAdmission)
        })
    }

    #[cfg(test)]
    pub(crate) fn stack_script_admission_for_test(
        self: &Arc<Self>,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.stack_script_admission(
            &AtomicBool::new(false),
            Arc::new(UiRequestDispatch::pending()),
        )
    }

    pub(super) fn execute_view_state(
        self: &Arc<Self>,
        ctx: &super::ReadContext,
    ) -> Result<mimageviewer_agent_ipc::ViewState, AgentError> {
        ctx.checkpoint()?;
        self.execute_view_state_read(&ctx.cancel)
    }

    fn execute_view_state_read(
        self: &Arc<Self>,
        cancel: &AtomicBool,
    ) -> Result<mimageviewer_agent_ipc::ViewState, AgentError> {
        let outcome = self.dispatch_request(
            cancel,
            Arc::new(UiRequestDispatch::pending()),
            UI_REPLY_TIMEOUT,
            // Known-folder resolution may call the shell. Keep it on this
            // worker; the UI chooses its live explicit book_root if present.
            || {
                Ok(UiRequestKind::GetViewState {
                    default_books_root: crate::books::default_books_root(),
                })
            },
        )?;
        match outcome {
            UiOpenOutcome::Read(state) => Ok(state),
            UiOpenOutcome::Refused(error) => Err(error),
            _ => Err(super::internal()),
        }
    }

    #[cfg(test)]
    pub(crate) fn execute_view_state_for_test(
        self: &Arc<Self>,
    ) -> Result<mimageviewer_agent_ipc::ViewState, AgentError> {
        self.execute_view_state_read(&AtomicBool::new(false))
    }

    #[cfg(test)]
    pub(crate) fn execute_prepared_for_test(
        self: &Arc<Self>,
        prepared: super::metadata_write::PreparedWrite,
        cancel: &AtomicBool,
        dispatch: Arc<UiRequestDispatch>,
        timeout: Duration,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.dispatch_request(cancel, dispatch, timeout, || Ok(prepared.into()))
    }

    #[cfg(test)]
    pub(crate) fn execute_settings_for_test(
        self: &Arc<Self>,
        kind: UiRequestKind,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.dispatch_request(
            &AtomicBool::new(false),
            Arc::new(UiRequestDispatch::pending()),
            UI_REPLY_TIMEOUT,
            || Ok(kind),
        )
    }

    #[cfg(test)]
    pub(crate) fn execute_favorite_for_test(
        self: &Arc<Self>,
        favorite: super::favorite::PreparedFavorite,
    ) -> Result<UiOpenOutcome, AgentError> {
        self.dispatch_request(
            &AtomicBool::new(false),
            Arc::new(UiRequestDispatch::pending()),
            UI_REPLY_TIMEOUT,
            || Ok(UiRequestKind::SetFavorite(favorite)),
        )
    }

    fn dispatch_request(
        self: &Arc<Self>,
        cancel: &AtomicBool,
        dispatch: Arc<UiRequestDispatch>,
        timeout: Duration,
        prepare: impl FnOnce() -> Result<UiRequestKind, AgentError>,
    ) -> Result<UiOpenOutcome, AgentError> {
        if self
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(super::error(
                ErrorCode::Busy,
                "ほかの要求で開く処理を受け付けています",
            ));
        }
        let _permit = OpenPermit(Arc::clone(self));
        let (sender, ctx) = {
            let installed = self.installed.lock().unwrap();
            let ui = installed.as_ref().ok_or_else(not_ready)?;
            (ui.sender.clone(), ui.ctx.clone())
        };
        if cancel.load(Ordering::Acquire) || dispatch.is_cancelled() {
            return Err(cancelled());
        }
        let kind = prepare()?;
        if cancel.load(Ordering::Acquire) || dispatch.is_cancelled() {
            return Err(cancelled());
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        // Only Open restores/foregrounds the window. Reads and metadata requests
        // wake the UI through repaint, including minimized/tray-hidden windows.
        if matches!(&kind, UiRequestKind::Open(_)) {
            #[cfg(not(test))]
            crate::single_instance::signal_activate_existing();
            // Headless tests probe the same routing without signaling a resident app.
            #[cfg(test)]
            if let Some(observer) = &*self.activation_observer.lock().unwrap() {
                let _ = observer.send(());
            }
        }
        sender
            .send(UiOpenRequest {
                kind,
                dispatch: Arc::clone(&dispatch),
                reply,
            })
            .map_err(|_| not_ready())?;
        // App owns this queue in ROOT, even while a deferred child pass is current.
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        #[cfg(test)]
        if let Some(observer) = self.enqueue_observer.lock().unwrap().as_ref() {
            let _ = observer.send(());
        }
        let deadline = Instant::now() + timeout;
        loop {
            if dispatch.is_cancelled() {
                return Err(cancelled());
            }
            let refusal = if cancel.load(Ordering::Acquire) {
                Some(cancelled())
            } else if Instant::now() >= deadline {
                Some(super::error(ErrorCode::Timeout, "mIV が応答しません"))
            } else {
                None
            };
            if let Some(refusal) = refusal {
                if dispatch.cancel_pending() {
                    return Err(refusal);
                }
                if dispatch.is_cancelled() {
                    return Err(cancelled());
                }
                // The UI owns the acceptance decision now. Cancellation and
                // timeouts may not replace its single, synchronous reply.
                return receiver.recv().map_err(|_| open_failed());
            }
            match receiver.recv_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(CANCEL_CHECK_INTERVAL),
            ) {
                Ok(reply) => return Ok(reply),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(open_failed()),
            }
        }
    }
}

fn outcome_result(reply: UiOpenOutcome) -> Result<OpenResult, AgentError> {
    match reply {
        UiOpenOutcome::Admitted => Ok(OpenResult {
            status: OpenStatus::Accepted,
            message: "mIV で開く処理を始めました".to_owned(),
        }),
        UiOpenOutcome::Refused(error) => Err(error),
        UiOpenOutcome::Written(_)
        | UiOpenOutcome::Read(_)
        | UiOpenOutcome::Operations(_)
        | UiOpenOutcome::Settings(_)
        | UiOpenOutcome::StackScriptAdmitted { .. }
        | UiOpenOutcome::OperationScriptAdmitted { .. }
        | UiOpenOutcome::SoundChainAdmitted(_) => Err(open_failed()),
    }
}

fn not_ready() -> AgentError {
    super::error(ErrorCode::NotReady, "mIV の起動処理中です")
}

fn cancelled() -> AgentError {
    super::error(ErrorCode::Cancelled, "開く要求を取り消しました")
}

fn open_failed() -> AgentError {
    super::error(ErrorCode::Internal, "開く要求の処理に失敗しました")
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;
