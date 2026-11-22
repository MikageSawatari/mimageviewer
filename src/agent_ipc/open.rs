//! One agent UI admission owner for Open, state reads, tags and ratings. Open
//! keeps the existing activation path; metadata targets are prepared on workers.
use std::sync::{
    Arc, Mutex,
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
    SetSettings {
        changes: serde_json::Map<String, serde_json::Value>,
        client_name: String,
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
        self.dispatch_request(&ctx.cancel, dispatch, UI_REPLY_TIMEOUT, || {
            if let mimageviewer_agent_ipc::Command::SetSettings { changes } = command {
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
            } else {
                super::metadata_write::prepare(command, ctx, client_name).map(UiRequestKind::from)
            }
        })
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
        UiOpenOutcome::Written(_) | UiOpenOutcome::Read(_) | UiOpenOutcome::Settings(_) => {
            Err(open_failed())
        }
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
