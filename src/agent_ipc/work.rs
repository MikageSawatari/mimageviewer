use super::*;

#[derive(Clone)]
pub(super) struct ReadContext {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) permission: PermissionSnapshot,
    progress: Arc<Mutex<Option<Payload>>>,
    deadline: Instant,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}
impl ReadContext {
    pub(super) fn new(
        handlers: &Handlers,
        permission: PermissionSnapshot,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let clock = handlers.clock();
        Self {
            cancel,
            permission,
            progress: Arc::new(Mutex::new(None)),
            deadline: clock() + Duration::from_secs(20),
            clock,
        }
    }
    pub(super) fn checkpoint(&self) -> Result<(), AgentError> {
        if self.cancel.load(Ordering::Acquire) {
            Err(cancelled())
        } else {
            Ok(())
        }
    }
    pub(super) fn expired(&self) -> bool {
        (self.clock)() >= self.deadline
    }
    pub(super) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since((self.clock)())
    }
    pub(super) fn publish(&self, payload: Payload) {
        *self.progress.lock().unwrap() = Some(payload);
    }
    fn deadline_result(&self) -> Result<Payload, AgentError> {
        self.progress.lock().unwrap().clone().ok_or_else(timeout)
    }
}
pub(super) fn timeout() -> AgentError {
    error(
        ErrorCode::Timeout,
        "時間切れ。もう一度試すか、件数を減らしてください",
    )
}

#[cfg(test)]
pub(super) struct ReadHooks {
    pub(super) now: Arc<dyn Fn() -> Instant + Send + Sync>,
    pub(super) execute:
        Box<dyn Fn(Command, &ReadContext) -> Result<Payload, AgentError> + Send + Sync>,
}

impl Handlers {
    fn clock(&self) -> Arc<dyn Fn() -> Instant + Send + Sync> {
        #[cfg(test)]
        {
            if let Some(hooks) = &self.read_hooks {
                return hooks.now.clone();
            }
            if let Some(hooks) = &self.search_hooks {
                let hooks = hooks.clone();
                return Arc::new(move || (hooks.now)());
            }
        }
        Arc::new(Instant::now)
    }
}

pub(super) fn respond(
    connection: Arc<Connection>,
    command: Command,
    granted: PermissionGrant,
    permit: Permit,
    ctx: ReadContext,
    replies: crossbeam_channel::Sender<Reply>,
    started: Instant,
    id: u64,
    tool: &'static str,
) {
    let worker_context = ctx.clone();
    let handlers = connection.handlers.clone();
    let (done, result) = crossbeam_channel::bounded(1);
    let worker = std::thread::Builder::new()
        .name("agent-read-worker".into())
        .spawn(move || {
            #[cfg(test)]
            if let Some(hooks) = &handlers.read_hooks {
                let _ = done.send((hooks.execute)(command, &worker_context));
                return;
            }
            let (value, nested) = handlers.execute(command, &worker_context);
            // Some synchronous search/PDF work cannot stop midway. Its caller must
            // remain alive until completion, even after a response deadline.
            if let Some(nested) = nested {
                let _ = nested.join();
            }
            let _ = done.send(value);
        });
    let Ok(worker) = worker else {
        drop(permit);
        let result = Err(internal());
        record(tool, started, &result);
        let _ = replies.send(Reply {
            response: Response { id, result },
            policy: ReplyPolicy::Read(granted),
        });
        return;
    };
    let mut completed = false;
    let value = loop {
        if ctx.cancel.load(Ordering::Acquire) {
            break Err(cancelled());
        }
        if (ctx.clock)() >= ctx.deadline {
            ctx.cancel.store(true, Ordering::Release);
            break ctx.deadline_result();
        }
        match result.recv_timeout(
            ctx.deadline
                .saturating_duration_since((ctx.clock)())
                .min(Duration::from_millis(25)),
        ) {
            Ok(value) => {
                completed = true;
                if (ctx.clock)() >= ctx.deadline {
                    ctx.cancel.store(true, Ordering::Release);
                    break ctx.deadline_result();
                }
                break value;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => {
                completed = true;
                break Err(internal());
            }
        }
    };
    if matches!(value, Ok(Payload::Hello { .. })) {
        connection.hello.store(true, Ordering::Release);
    }
    record(tool, started, &value);
    let reply = Reply {
        response: Response { id, result: value },
        policy: ReplyPolicy::Read(granted),
    };
    if completed {
        let _ = worker.join();
        drop(permit);
        let _ = replies.send(reply);
    } else {
        let _ = replies.send(reply);
        let _ = worker.join();
        drop(permit);
    }
}
