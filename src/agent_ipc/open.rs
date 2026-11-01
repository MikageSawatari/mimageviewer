//! One agent UI admission owner for Open, tags and ratings. Open keeps the
//! existing activation path; metadata targets are prepared on workers.
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
    Written(Result<mimageviewer_agent_ipc::Payload, AgentError>),
    Refused(AgentError),
}

pub(crate) enum UiRequestKind {
    Open(ActivationOpenPath),
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
            super::metadata_write::prepare(command, ctx, client_name).map(UiRequestKind::from)
        })
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
        // Reuse the existing event/listener to restore a hidden or minimized
        // window. Headless unit tests must never signal a resident application.
        #[cfg(not(test))]
        crate::single_instance::signal_activate_existing();
        sender
            .send(UiOpenRequest {
                kind,
                dispatch: Arc::clone(&dispatch),
                reply,
            })
            .map_err(|_| not_ready())?;
        ctx.request_repaint();
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
        UiOpenOutcome::Written(_) => Err(open_failed()),
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
