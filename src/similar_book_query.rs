//! Viewer-owned scheduling for similar-book queries.
//!
//! The executor runs one job globally while each move-only client keeps an independent desired
//! book, completion, and at most one pending refresh.  The database reader and search index are
//! intentionally supplied by a worker-local runtime; UI clients never retain those resources.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ClientId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestId(u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RefreshId(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BookQueryRequest {
    request_id: RequestId,
    client_id: ClientId,
    hard_generation: u64,
    refresh_id: RefreshId,
    container_key: String,
}

impl BookQueryRequest {
    pub(crate) fn container_key(&self) -> &str {
        &self.container_key
    }

    #[cfg(test)]
    fn refresh_id(&self) -> u64 {
        self.refresh_id.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BookQueryTerminal<T> {
    Ready(T),
    Failed(String),
}

pub(crate) trait BookQueryRuntime<T>: Send + 'static {
    fn execute(
        &mut self,
        request: BookQueryRequest,
        cancel: Arc<AtomicBool>,
    ) -> BookQueryTerminal<T>;
}

type RuntimeFactory<T> =
    dyn Fn() -> Result<Box<dyn BookQueryRuntime<T>>, String> + Send + Sync + 'static;
type ThreadJob = Box<dyn FnOnce() + Send + 'static>;
type ThreadSpawner = dyn Fn(ThreadJob) -> Result<(), String> + Send + Sync + 'static;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BookQueryPoll<T> {
    Preparing,
    Ready(T),
    Failed(String),
    Withdrawn,
    ExecutorStopped,
}

#[derive(Clone, Debug)]
struct Completion<T> {
    request: BookQueryRequest,
    terminal: BookQueryTerminal<T>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientWork {
    Idle,
    Queued,
    Running(RequestId),
}

#[derive(Debug)]
struct ClientState<T> {
    container_key: Option<String>,
    hard_generation: u64,
    desired_refresh: RefreshId,
    work: ClientWork,
    completion: Option<Completion<T>>,
}

#[derive(Debug)]
struct Running {
    client_id: ClientId,
    request: BookQueryRequest,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WorkerLifecycle {
    Dormant,
    Starting,
    Live,
    Stopping,
    Stopped,
    Failed(String),
}

impl WorkerLifecycle {
    fn accepts_requests(&self) -> bool {
        matches!(self, Self::Dormant | Self::Starting | Self::Live)
    }
}

struct OwnerState<T> {
    clients: HashMap<ClientId, ClientState<T>>,
    fifo: VecDeque<ClientId>,
    /// The only state that grants execution authority.
    running: Option<Running>,
    worker: WorkerLifecycle,
    next_client_id: u64,
    next_request_id: u64,
    current_refresh: RefreshId,
}

impl<T> Default for OwnerState<T> {
    fn default() -> Self {
        Self {
            clients: HashMap::new(),
            fifo: VecDeque::new(),
            running: None,
            worker: WorkerLifecycle::Dormant,
            next_client_id: 1,
            next_request_id: 1,
            current_refresh: RefreshId(0),
        }
    }
}

struct ExecutorInner<T> {
    identity: u64,
    state: Mutex<OwnerState<T>>,
    wake: Condvar,
    runtime_factory: Arc<RuntimeFactory<T>>,
    thread_spawner: Arc<ThreadSpawner>,
}

static NEXT_EXECUTOR_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// Global one-worker owner. This handle is intentionally not cloneable; dropping the manager-side
/// owner signals shutdown but never joins the worker on the caller thread.
pub(crate) struct BookQueryExecutor<T> {
    inner: Arc<ExecutorInner<T>>,
}

enum ClientBinding<T: Clone + Send + 'static> {
    Unbound,
    Bound {
        executor: Weak<ExecutorInner<T>>,
        executor_identity: u64,
        client_id: ClientId,
    },
}

/// Per-viewer opaque client. It is move-only so two viewer bundles cannot accidentally share one
/// request identity.
pub(crate) struct BookQueryClient<T: Clone + Send + 'static> {
    binding: Mutex<ClientBinding<T>>,
}

impl<T: Clone + Send + 'static> Default for BookQueryClient<T> {
    fn default() -> Self {
        Self {
            binding: Mutex::new(ClientBinding::Unbound),
        }
    }
}

impl<T> BookQueryExecutor<T>
where
    T: Clone + Send + 'static,
{
    pub(crate) fn new(
        runtime_factory: impl Fn() -> Result<Box<dyn BookQueryRuntime<T>>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_spawner(runtime_factory, |job| {
            std::thread::Builder::new()
                .name("similar-book-query".to_owned())
                .spawn(job)
                .map(|_| ())
                .map_err(|error| format!("similar book query worker spawn failed: {error}"))
        })
    }

    fn with_spawner(
        runtime_factory: impl Fn() -> Result<Box<dyn BookQueryRuntime<T>>, String>
        + Send
        + Sync
        + 'static,
        thread_spawner: impl Fn(ThreadJob) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(ExecutorInner {
                identity: NEXT_EXECUTOR_IDENTITY.fetch_add(1, Ordering::Relaxed),
                state: Mutex::new(OwnerState::default()),
                wake: Condvar::new(),
                runtime_factory: Arc::new(runtime_factory),
                thread_spawner: Arc::new(thread_spawner),
            }),
        }
    }

    pub(crate) fn query(
        &self,
        client: &BookQueryClient<T>,
        container_key: &str,
    ) -> BookQueryPoll<T> {
        let client_id = client.bind_to(self);
        let poll = self.inner.query(client_id, container_key);
        self.inner.ensure_worker();
        self.inner.wake.notify_one();
        poll
    }

    pub(crate) fn withdraw(&self, client: &BookQueryClient<T>) {
        if let Some(client_id) = client.id_for(&self.inner) {
            self.inner.withdraw(client_id);
        }
    }

    /// Coalesces one environment notification. This id is owner-issued and deliberately unrelated
    /// to SQLite or array sequence numbers (page-order repair may change without either one).
    pub(crate) fn soft_refresh(&self) {
        self.inner.soft_refresh();
        self.inner.ensure_worker();
        self.inner.wake.notify_one();
    }

    /// A confirmed scope/store change invalidates results once. Snapshot X versus read-TX store Y
    /// is handled by the worker runtime and must not repeatedly call this method.
    pub(crate) fn hard_invalidate(&self) {
        self.inner.hard_invalidate();
        self.inner.ensure_worker();
        self.inner.wake.notify_one();
    }

    #[cfg(test)]
    fn wait_for_ready(&self, client: &BookQueryClient<T>, container_key: &str) -> T {
        let client_id = client
            .id_for(&self.inner)
            .expect("test client is not bound to this executor");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            let client = state.clients.get(&client_id).expect("test client vanished");
            if let Some(completion) = &client.completion
                && completion.request.hard_generation == client.hard_generation
                && completion.request.container_key == container_key
                && let BookQueryTerminal::Ready(value) = &completion.terminal
            {
                return value.clone();
            }
            let now = std::time::Instant::now();
            assert!(
                now < deadline,
                "timed out waiting for Ready({container_key})"
            );
            let (next, _) = self
                .inner
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
        }
    }
}

impl<T> Drop for BookQueryExecutor<T> {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}

impl<T: Clone + Send + 'static> BookQueryClient<T> {
    fn bind_to(&self, executor: &BookQueryExecutor<T>) -> ClientId {
        let mut binding = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let ClientBinding::Bound {
            executor: current,
            executor_identity,
            client_id,
        } = &*binding
            && *executor_identity == executor.inner.identity
            && current
                .upgrade()
                .is_some_and(|inner| Arc::ptr_eq(&inner, &executor.inner))
        {
            return *client_id;
        }

        if let ClientBinding::Bound {
            executor: previous,
            executor_identity,
            client_id,
        } = std::mem::replace(&mut *binding, ClientBinding::Unbound)
            && let Some(previous) = previous.upgrade()
            && previous.identity == executor_identity
        {
            previous.drop_client(client_id);
        }

        let client_id = executor.inner.register_client();
        *binding = ClientBinding::Bound {
            executor: Arc::downgrade(&executor.inner),
            executor_identity: executor.inner.identity,
            client_id,
        };
        client_id
    }

    fn id_for(&self, executor: &Arc<ExecutorInner<T>>) -> Option<ClientId> {
        let binding = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &*binding {
            ClientBinding::Bound {
                executor: current,
                executor_identity,
                client_id,
            } if *executor_identity == executor.identity
                && current
                    .upgrade()
                    .is_some_and(|inner| Arc::ptr_eq(&inner, executor)) =>
            {
                Some(*client_id)
            }
            _ => None,
        }
    }

    pub(crate) fn withdraw(&self) {
        let binding = self
            .binding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let ClientBinding::Bound {
            executor,
            executor_identity,
            client_id,
        } = &*binding
            && let Some(executor) = executor.upgrade()
            && executor.identity == *executor_identity
        {
            executor.withdraw(*client_id);
        }
    }
}

impl<T: Clone + Send + 'static> Drop for BookQueryClient<T> {
    fn drop(&mut self) {
        let binding = self
            .binding
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        if let ClientBinding::Bound {
            executor,
            executor_identity,
            client_id,
        } = std::mem::replace(binding, ClientBinding::Unbound)
            && let Some(executor) = executor.upgrade()
            && executor.identity == executor_identity
        {
            executor.drop_client(client_id);
        }
    }
}

impl<T> ExecutorInner<T>
where
    T: Clone + Send + 'static,
{
    fn register_client(&self) -> ClientId {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let client_id = ClientId(state.next_client_id);
        state.next_client_id = state.next_client_id.wrapping_add(1).max(1);
        let refresh = state.current_refresh;
        state.clients.insert(
            client_id,
            ClientState {
                container_key: None,
                hard_generation: 0,
                desired_refresh: refresh,
                work: ClientWork::Idle,
                completion: None,
            },
        );
        client_id
    }

    fn query(self: &Arc<Self>, client_id: ClientId, container_key: &str) -> BookQueryPoll<T> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match &state.worker {
            WorkerLifecycle::Failed(error) => return BookQueryPoll::Failed(error.clone()),
            WorkerLifecycle::Stopping | WorkerLifecycle::Stopped => {
                return BookQueryPoll::ExecutorStopped;
            }
            WorkerLifecycle::Dormant | WorkerLifecycle::Starting | WorkerLifecycle::Live => {}
        }
        let Some(client) = state.clients.get(&client_id) else {
            return BookQueryPoll::Withdrawn;
        };
        let origin_changed = client.container_key.as_deref() != Some(container_key);
        if origin_changed {
            Self::remove_queued_locked(&mut state, client_id);
            if let Some(running) = state
                .running
                .as_ref()
                .filter(|running| running.client_id == client_id)
            {
                running.cancel.store(true, Ordering::Release);
            }
            let refresh = state.current_refresh;
            let client = state.clients.get_mut(&client_id).unwrap();
            client.container_key = Some(container_key.to_owned());
            client.hard_generation = client.hard_generation.wrapping_add(1);
            client.desired_refresh = refresh;
            client.work = ClientWork::Idle;
            client.completion = None;
        }

        let client = state.clients.get(&client_id).unwrap();
        let visible = client.completion.as_ref().and_then(|completion| {
            (completion.request.hard_generation == client.hard_generation
                && completion.request.container_key == container_key)
                .then(|| match &completion.terminal {
                    BookQueryTerminal::Ready(value) => BookQueryPoll::Ready(value.clone()),
                    BookQueryTerminal::Failed(error) => BookQueryPoll::Failed(error.clone()),
                })
        });
        if visible.is_none() && matches!(client.work, ClientWork::Idle) {
            Self::enqueue_locked(&mut state, client_id);
        }
        visible.unwrap_or(BookQueryPoll::Preparing)
    }

    fn soft_refresh(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.worker.accepts_requests() {
            return;
        }
        state.current_refresh = RefreshId(state.current_refresh.0.saturating_add(1));
        let refresh = state.current_refresh;
        let client_ids = state.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            let Some(client) = state.clients.get_mut(&client_id) else {
                continue;
            };
            if client.container_key.is_none() {
                continue;
            }
            client.desired_refresh = refresh;
            if matches!(client.work, ClientWork::Idle) {
                Self::enqueue_locked(&mut state, client_id);
            }
            // Queued refreshes remain in place; repeated notifications update `desired_refresh`
            // only. Running work is not cancelled and publishes once before its refresh is queued.
        }
    }

    fn hard_invalidate(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.worker.accepts_requests() {
            return;
        }
        if let Some(running) = &state.running {
            running.cancel.store(true, Ordering::Release);
        }
        state.fifo.clear();
        let refresh = state.current_refresh;
        let client_ids = state.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            let client = state.clients.get_mut(&client_id).unwrap();
            client.hard_generation = client.hard_generation.wrapping_add(1);
            client.desired_refresh = refresh;
            client.work = ClientWork::Idle;
            client.completion = None;
            if client.container_key.is_some() {
                Self::enqueue_locked(&mut state, client_id);
            }
        }
    }

    fn withdraw(&self, client_id: ClientId) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        Self::withdraw_locked(&mut state, client_id, false);
        self.wake.notify_all();
    }

    fn drop_client(&self, client_id: ClientId) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        Self::withdraw_locked(&mut state, client_id, true);
        self.wake.notify_one();
    }

    fn withdraw_locked(state: &mut OwnerState<T>, client_id: ClientId, remove: bool) {
        Self::remove_queued_locked(state, client_id);
        if let Some(running) = state
            .running
            .as_ref()
            .filter(|running| running.client_id == client_id)
        {
            running.cancel.store(true, Ordering::Release);
        }
        if remove {
            state.clients.remove(&client_id);
        } else if let Some(client) = state.clients.get_mut(&client_id) {
            client.container_key = None;
            client.hard_generation = client.hard_generation.wrapping_add(1);
            client.work = ClientWork::Idle;
            client.completion = None;
        }
    }

    fn enqueue_locked(state: &mut OwnerState<T>, client_id: ClientId) {
        let Some(client) = state.clients.get_mut(&client_id) else {
            return;
        };
        if matches!(client.work, ClientWork::Idle) {
            client.work = ClientWork::Queued;
            state.fifo.push_back(client_id);
        }
    }

    fn remove_queued_locked(state: &mut OwnerState<T>, client_id: ClientId) {
        state.fifo.retain(|queued| *queued != client_id);
        if let Some(client) = state.clients.get_mut(&client_id)
            && matches!(client.work, ClientWork::Queued)
        {
            client.work = ClientWork::Idle;
        }
    }

    fn take_next_locked(state: &mut OwnerState<T>) -> Option<Running> {
        if state.running.is_some() {
            return None;
        }
        while let Some(client_id) = state.fifo.pop_front() {
            let Some(client) = state.clients.get_mut(&client_id) else {
                continue;
            };
            if !matches!(client.work, ClientWork::Queued) {
                continue;
            }
            let Some(container_key) = client.container_key.clone() else {
                client.work = ClientWork::Idle;
                continue;
            };
            let request_id = RequestId(state.next_request_id);
            state.next_request_id = state.next_request_id.wrapping_add(1).max(1);
            let request = BookQueryRequest {
                request_id,
                client_id,
                hard_generation: client.hard_generation,
                refresh_id: client.desired_refresh,
                container_key,
            };
            let running = Running {
                client_id,
                request: request.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            };
            client.work = ClientWork::Running(request_id);
            state.running = Some(Running {
                client_id,
                request,
                cancel: Arc::clone(&running.cancel),
            });
            return Some(running);
        }
        None
    }

    fn finish(&self, running: &Running, terminal: BookQueryTerminal<T>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(active) = state.running.as_ref() else {
            return;
        };
        if active.client_id != running.client_id
            || active.request.request_id != running.request.request_id
        {
            return;
        }
        state.running = None;

        let mut enqueue_refresh = false;
        if let Some(client) = state.clients.get_mut(&running.client_id) {
            let same_request = client.container_key.as_deref()
                == Some(running.request.container_key.as_str())
                && client.hard_generation == running.request.hard_generation
                && matches!(client.work, ClientWork::Running(id) if id == running.request.request_id);
            if same_request {
                client.work = ClientWork::Idle;
                if !running.cancel.load(Ordering::Acquire) {
                    client.completion = Some(Completion {
                        request: running.request.clone(),
                        terminal,
                    });
                }
                enqueue_refresh = client.desired_refresh > running.request.refresh_id;
            }
        }
        if enqueue_refresh {
            Self::enqueue_locked(&mut state, running.client_id);
        }
        self.wake.notify_one();
    }

    fn ensure_worker(self: &Arc<Self>) {
        let should_spawn = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if matches!(state.worker, WorkerLifecycle::Dormant) && !state.fifo.is_empty() {
                state.worker = WorkerLifecycle::Starting;
                true
            } else {
                false
            }
        };
        if !should_spawn {
            return;
        }

        let inner = Arc::clone(self);
        let job: ThreadJob = Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(|| inner.worker_main()));
            match result {
                Ok(()) => inner.mark_worker_stopped(),
                Err(_) => inner.fail_worker("similar book query worker panicked".to_owned()),
            }
        });
        if let Err(error) = (self.thread_spawner)(job) {
            self.fail_worker(error);
        }
    }

    fn worker_main(self: &Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            match state.worker {
                WorkerLifecycle::Starting => state.worker = WorkerLifecycle::Live,
                WorkerLifecycle::Stopping => return,
                _ => return,
            }
        }

        let mut runtime = match self.create_runtime_if_live() {
            Some(Ok(runtime)) => runtime,
            Some(Err(error)) => {
                self.fail_worker(format!("similar book query runtime init failed: {error}"));
                return;
            }
            None => return,
        };

        loop {
            let running = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                loop {
                    match state.worker {
                        WorkerLifecycle::Live => {
                            if let Some(running) = Self::take_next_locked(&mut state) {
                                break running;
                            }
                            state = self
                                .wake
                                .wait(state)
                                .unwrap_or_else(|error| error.into_inner());
                        }
                        WorkerLifecycle::Stopping
                        | WorkerLifecycle::Stopped
                        | WorkerLifecycle::Failed(_) => return,
                        WorkerLifecycle::Dormant | WorkerLifecycle::Starting => return,
                    }
                }
            };

            let terminal = catch_unwind(AssertUnwindSafe(|| {
                runtime.execute(running.request.clone(), Arc::clone(&running.cancel))
            }));
            match terminal {
                Ok(terminal) => self.finish(&running, terminal),
                Err(_) => {
                    self.finish(
                        &running,
                        BookQueryTerminal::Failed("similar book query job panicked".to_owned()),
                    );
                    drop(runtime);
                    runtime = match self.create_runtime_if_live() {
                        Some(Ok(runtime)) => runtime,
                        Some(Err(error)) => {
                            self.fail_worker(format!(
                                "similar book query runtime restart failed: {error}"
                            ));
                            return;
                        }
                        None => return,
                    };
                }
            }
        }
    }

    /// Approves both initial creation and panic recovery from the lifecycle owner while holding
    /// its lock, then calls user/runtime code only after releasing that lock. If shutdown wins
    /// before approval no runtime is created; if it wins after approval the worker drops the new
    /// runtime without executing another request.
    fn create_runtime_if_live(&self) -> Option<Result<Box<dyn BookQueryRuntime<T>>, String>> {
        let approved = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            matches!(state.worker, WorkerLifecycle::Live)
        };
        approved.then(|| (self.runtime_factory)())
    }

    fn fail_worker(&self, error: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if matches!(
            state.worker,
            WorkerLifecycle::Stopping | WorkerLifecycle::Stopped
        ) {
            state.worker = WorkerLifecycle::Stopped;
            state.running = None;
            state.fifo.clear();
            self.wake.notify_all();
            return;
        }
        if let Some(running) = &state.running {
            running.cancel.store(true, Ordering::Release);
        }
        state.running = None;
        state.fifo.clear();
        for client in state.clients.values_mut() {
            client.work = ClientWork::Idle;
        }
        state.worker = WorkerLifecycle::Failed(error);
        self.wake.notify_all();
    }

    fn mark_worker_stopped(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(state.worker, WorkerLifecycle::Failed(_)) {
            state.worker = WorkerLifecycle::Stopped;
        }
        state.running = None;
        state.fifo.clear();
        for client in state.clients.values_mut() {
            client.work = ClientWork::Idle;
        }
        self.wake.notify_all();
    }
}

impl<T> ExecutorInner<T> {
    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(running) = &state.running {
            running.cancel.store(true, Ordering::Release);
        }
        state.fifo.clear();
        for client in state.clients.values_mut() {
            if matches!(client.work, ClientWork::Queued) {
                client.work = ClientWork::Idle;
            }
        }
        state.worker = match state.worker {
            WorkerLifecycle::Dormant => WorkerLifecycle::Stopped,
            WorkerLifecycle::Starting | WorkerLifecycle::Live => WorkerLifecycle::Stopping,
            WorkerLifecycle::Stopping => WorkerLifecycle::Stopping,
            WorkerLifecycle::Stopped => WorkerLifecycle::Stopped,
            WorkerLifecycle::Failed(ref error) => WorkerLifecycle::Failed(error.clone()),
        };
        self.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    #[derive(Debug)]
    enum Command {
        Complete(i32),
        Fail(&'static str),
        Panic,
    }

    #[derive(Debug)]
    enum Event {
        Started {
            key: String,
            refresh: u64,
            cancel: Arc<AtomicBool>,
        },
        RuntimeDropped(u64),
    }

    struct MockRuntime {
        id: u64,
        commands: Arc<Mutex<Receiver<Command>>>,
        events: Sender<Event>,
    }

    impl Drop for MockRuntime {
        fn drop(&mut self) {
            let _ = self.events.send(Event::RuntimeDropped(self.id));
        }
    }

    impl BookQueryRuntime<i32> for MockRuntime {
        fn execute(
            &mut self,
            request: BookQueryRequest,
            cancel: Arc<AtomicBool>,
        ) -> BookQueryTerminal<i32> {
            self.events
                .send(Event::Started {
                    key: request.container_key().to_owned(),
                    refresh: request.refresh_id(),
                    cancel,
                })
                .unwrap();
            let command = self
                .commands
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .recv()
                .unwrap();
            match command {
                Command::Complete(value) => BookQueryTerminal::Ready(value),
                Command::Fail(error) => BookQueryTerminal::Failed(error.to_owned()),
                Command::Panic => panic!("mock job panic"),
            }
        }
    }

    struct CountingRuntime {
        executes: Arc<AtomicU64>,
        drops: Arc<AtomicU64>,
    }

    impl Drop for CountingRuntime {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::AcqRel);
        }
    }

    impl BookQueryRuntime<i32> for CountingRuntime {
        fn execute(
            &mut self,
            _request: BookQueryRequest,
            _cancel: Arc<AtomicBool>,
        ) -> BookQueryTerminal<i32> {
            self.executes.fetch_add(1, Ordering::AcqRel);
            BookQueryTerminal::Ready(1)
        }
    }

    fn executor_with_done_spawner(
        runtime_factory: impl Fn() -> Result<Box<dyn BookQueryRuntime<i32>>, String>
        + Send
        + Sync
        + 'static,
    ) -> (BookQueryExecutor<i32>, Receiver<()>) {
        let (done_tx, done_rx) = channel();
        let executor = BookQueryExecutor::with_spawner(runtime_factory, move |job| {
            let done = done_tx.clone();
            std::thread::Builder::new()
                .name("similar-book-query-test".to_owned())
                .spawn(move || {
                    job();
                    let _ = done.send(());
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        (executor, done_rx)
    }

    struct Harness {
        executor: Option<BookQueryExecutor<i32>>,
        commands: Sender<Command>,
        events: Receiver<Event>,
        next_runtime_id: Arc<AtomicU64>,
    }

    impl Harness {
        fn new() -> Self {
            let (command_tx, command_rx) = channel();
            let commands = Arc::new(Mutex::new(command_rx));
            let (event_tx, event_rx) = channel();
            let next_runtime_id = Arc::new(AtomicU64::new(1));
            let executor = BookQueryExecutor::new({
                let commands = Arc::clone(&commands);
                let events = event_tx;
                let ids = Arc::clone(&next_runtime_id);
                move || {
                    Ok(Box::new(MockRuntime {
                        id: ids.fetch_add(1, Ordering::Relaxed),
                        commands: Arc::clone(&commands),
                        events: events.clone(),
                    }))
                }
            });
            Self {
                executor: Some(executor),
                commands: command_tx,
                events: event_rx,
                next_runtime_id,
            }
        }

        fn executor(&self) -> &BookQueryExecutor<i32> {
            self.executor.as_ref().unwrap()
        }

        fn started(&self) -> (String, u64, Arc<AtomicBool>) {
            match self.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::Started {
                    key,
                    refresh,
                    cancel,
                } => (key, refresh, cancel),
                Event::RuntimeDropped(id) => panic!("runtime {id} dropped before a job started"),
            }
        }

        fn shutdown(mut self, expected_runtime_id: u64) {
            drop(self.executor.take());
            match self.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::RuntimeDropped(id) => assert_eq!(id, expected_runtime_id),
                event => panic!("unexpected shutdown event: {event:?}"),
            }
        }
    }

    #[test]
    fn soft_refresh_publishes_current_then_runs_fifo_and_coalesces_in_place() {
        let harness = Harness::new();
        let a = BookQueryClient::default();
        let b = BookQueryClient::default();
        let c = BookQueryClient::default();
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        assert_eq!(harness.started().0, "a");
        assert_eq!(harness.executor().query(&b, "b"), BookQueryPoll::Preparing);

        harness.executor().soft_refresh();
        harness.commands.send(Command::Complete(10)).unwrap();

        assert_eq!(harness.started().0, "b");
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Ready(10));
        assert_eq!(harness.executor().query(&c, "c"), BookQueryPoll::Preparing);
        // A's refresh is already queued ahead of C. Updating that queued refresh in place must
        // neither duplicate it nor move it behind C.
        harness.executor().soft_refresh();
        harness.executor().soft_refresh();
        harness.commands.send(Command::Complete(20)).unwrap();

        let (key, refresh, _) = harness.started();
        assert_eq!((key.as_str(), refresh), ("a", 3));
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Ready(10));
        harness.commands.send(Command::Complete(11)).unwrap();
        assert_eq!(harness.started().0, "c");
        harness.commands.send(Command::Complete(30)).unwrap();
        assert_eq!(harness.started().0, "b");
        harness.commands.send(Command::Complete(21)).unwrap();
        assert_eq!(harness.executor().wait_for_ready(&a, "a"), 11);
        harness.shutdown(1);
    }

    #[test]
    fn repeated_poll_does_not_reorder_a_waiting_client() {
        let harness = Harness::new();
        let a = BookQueryClient::default();
        let b = BookQueryClient::default();
        let c = BookQueryClient::default();
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        assert_eq!(harness.started().0, "a");
        for _ in 0..4 {
            assert_eq!(harness.executor().query(&b, "b"), BookQueryPoll::Preparing);
        }
        assert_eq!(harness.executor().query(&c, "c"), BookQueryPoll::Preparing);
        harness.commands.send(Command::Complete(1)).unwrap();
        assert_eq!(harness.started().0, "b");
        harness.commands.send(Command::Complete(2)).unwrap();
        assert_eq!(harness.started().0, "c");
        harness.commands.send(Command::Complete(3)).unwrap();
        harness.shutdown(1);
    }

    #[test]
    fn hard_a_to_b_to_a_rejects_the_first_a_completion_and_preserves_other_client() {
        let harness = Harness::new();
        let a = BookQueryClient::default();
        let other = BookQueryClient::default();
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        let (_, _, old_cancel) = harness.started();
        assert_eq!(
            harness.executor().query(&other, "other"),
            BookQueryPoll::Preparing
        );
        assert_eq!(harness.executor().query(&a, "b"), BookQueryPoll::Preparing);
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        assert!(old_cancel.load(Ordering::Acquire));

        harness.commands.send(Command::Complete(99)).unwrap();
        assert_eq!(harness.started().0, "other");
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        harness.commands.send(Command::Complete(7)).unwrap();
        assert_eq!(harness.started().0, "a");
        harness.commands.send(Command::Complete(1)).unwrap();
        harness.shutdown(1);
    }

    #[test]
    fn two_clients_for_the_same_book_keep_independent_requests_and_results() {
        let harness = Harness::new();
        let first = BookQueryClient::default();
        let second = BookQueryClient::default();
        assert_eq!(
            harness.executor().query(&first, "shared"),
            BookQueryPoll::Preparing
        );
        assert_eq!(harness.started().0, "shared");
        assert_eq!(
            harness.executor().query(&second, "shared"),
            BookQueryPoll::Preparing
        );

        harness.commands.send(Command::Complete(10)).unwrap();
        assert_eq!(harness.started().0, "shared");
        assert_eq!(
            harness.executor().query(&first, "shared"),
            BookQueryPoll::Ready(10)
        );
        assert_eq!(
            harness.executor().query(&second, "shared"),
            BookQueryPoll::Preparing
        );
        harness.commands.send(Command::Complete(20)).unwrap();
        assert_eq!(harness.executor().wait_for_ready(&second, "shared"), 20);
        assert_eq!(
            harness.executor().query(&first, "shared"),
            BookQueryPoll::Ready(10)
        );
        harness.shutdown(1);
    }

    #[test]
    fn explicit_withdraw_cancels_only_its_client_and_preserves_a_sibling() {
        let harness = Harness::new();
        let withdrawn = BookQueryClient::default();
        let sibling = BookQueryClient::default();
        assert_eq!(
            harness.executor().query(&withdrawn, "withdrawn"),
            BookQueryPoll::Preparing
        );
        let (_, _, cancel) = harness.started();
        assert_eq!(
            harness.executor().query(&sibling, "sibling"),
            BookQueryPoll::Preparing
        );

        harness.executor().withdraw(&withdrawn);
        assert!(cancel.load(Ordering::Acquire));
        harness.commands.send(Command::Complete(1)).unwrap();
        assert_eq!(harness.started().0, "sibling");
        harness.commands.send(Command::Complete(2)).unwrap();
        assert_eq!(harness.executor().wait_for_ready(&sibling, "sibling"), 2);
        assert_eq!(
            harness.executor().query(&withdrawn, "withdrawn"),
            BookQueryPoll::Preparing
        );
        assert_eq!(harness.started().0, "withdrawn");
        harness.commands.send(Command::Complete(3)).unwrap();
        harness.shutdown(1);
    }

    #[test]
    fn dropping_one_client_removes_only_its_registration() {
        let harness = Harness::new();
        let sibling = BookQueryClient::default();
        let dropped = BookQueryClient::default();
        assert_eq!(
            harness.executor().query(&sibling, "sibling"),
            BookQueryPoll::Preparing
        );
        assert_eq!(harness.started().0, "sibling");
        assert_eq!(
            harness.executor().query(&dropped, "dropped"),
            BookQueryPoll::Preparing
        );
        let dropped_id = dropped.id_for(&harness.executor().inner).unwrap();
        drop(dropped);
        {
            let state = harness
                .executor()
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert!(!state.clients.contains_key(&dropped_id));
            assert!(!state.fifo.contains(&dropped_id));
        }

        harness.commands.send(Command::Complete(4)).unwrap();
        assert_eq!(harness.executor().wait_for_ready(&sibling, "sibling"), 4);
        harness.shutdown(1);
    }

    #[test]
    fn global_hard_invalidation_retires_ready_and_cancels_running_work() {
        let harness = Harness::new();
        let ready = BookQueryClient::default();
        let running = BookQueryClient::default();
        assert_eq!(
            harness.executor().query(&ready, "ready"),
            BookQueryPoll::Preparing
        );
        assert_eq!(harness.started().0, "ready");
        harness.commands.send(Command::Complete(1)).unwrap();
        assert_eq!(harness.executor().wait_for_ready(&ready, "ready"), 1);
        assert_eq!(
            harness.executor().query(&running, "running"),
            BookQueryPoll::Preparing
        );
        let (_, _, old_cancel) = harness.started();

        harness.executor().hard_invalidate();
        assert!(old_cancel.load(Ordering::Acquire));
        assert_eq!(
            harness.executor().query(&ready, "ready"),
            BookQueryPoll::Preparing
        );
        assert_eq!(
            harness.executor().query(&running, "running"),
            BookQueryPoll::Preparing
        );
        harness.commands.send(Command::Complete(99)).unwrap();

        let mut refreshed = Vec::new();
        let mut running_value = None;
        for value in [10, 20] {
            let (key, _, _) = harness.started();
            refreshed.push(key.clone());
            if key == "running" {
                running_value = Some(value);
            }
            harness.commands.send(Command::Complete(value)).unwrap();
        }
        refreshed.sort();
        assert_eq!(refreshed, ["ready", "running"]);
        assert_eq!(
            harness.executor().wait_for_ready(&running, "running"),
            running_value.unwrap()
        );
        harness.shutdown(1);
    }

    #[test]
    fn failure_dispatches_the_next_client_without_ui_poll() {
        let harness = Harness::new();
        let a = BookQueryClient::default();
        let b = BookQueryClient::default();
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        assert_eq!(harness.started().0, "a");
        assert_eq!(harness.executor().query(&b, "b"), BookQueryPoll::Preparing);
        harness.commands.send(Command::Fail("failed-a")).unwrap();
        assert_eq!(harness.started().0, "b");
        assert_eq!(
            harness.executor().query(&a, "a"),
            BookQueryPoll::Failed("failed-a".to_owned())
        );
        harness.commands.send(Command::Complete(2)).unwrap();
        harness.shutdown(1);
    }

    #[test]
    fn panicked_job_replaces_its_runtime_and_dispatches_the_next_client() {
        let harness = Harness::new();
        let a = BookQueryClient::default();
        let b = BookQueryClient::default();
        assert_eq!(harness.executor().query(&a, "a"), BookQueryPoll::Preparing);
        assert_eq!(harness.started().0, "a");
        assert_eq!(harness.executor().query(&b, "b"), BookQueryPoll::Preparing);
        harness.commands.send(Command::Panic).unwrap();

        match harness.events.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::RuntimeDropped(1) => {}
            event => panic!("unexpected event after panic: {event:?}"),
        }
        assert_eq!(harness.started().0, "b");
        assert_eq!(
            harness.executor().query(&a, "a"),
            BookQueryPoll::Failed("similar book query job panicked".to_owned())
        );
        harness.commands.send(Command::Complete(2)).unwrap();
        harness.shutdown(2);
    }

    #[test]
    fn rebinding_a_client_withdraws_its_old_executor_registration() {
        let first = Harness::new();
        let second = Harness::new();
        let client = BookQueryClient::default();
        assert_eq!(
            first.executor().query(&client, "old"),
            BookQueryPoll::Preparing
        );
        let (_, _, old_cancel) = first.started();
        assert_eq!(
            second.executor().query(&client, "new"),
            BookQueryPoll::Preparing
        );
        assert!(old_cancel.load(Ordering::Acquire));
        assert_eq!(second.started().0, "new");
        first.commands.send(Command::Complete(1)).unwrap();
        second.commands.send(Command::Complete(2)).unwrap();
        first.shutdown(1);
        second.shutdown(1);
    }

    #[test]
    fn spawn_failure_is_terminal_instead_of_permanent_preparing() {
        let executor: BookQueryExecutor<i32> = BookQueryExecutor::with_spawner(
            || panic!("runtime must not be created when spawning fails"),
            |_| Err("injected spawn failure".to_owned()),
        );
        let client = BookQueryClient::default();
        assert_eq!(executor.query(&client, "a"), BookQueryPoll::Preparing);
        assert_eq!(
            executor.query(&client, "a"),
            BookQueryPoll::Failed("injected spawn failure".to_owned())
        );
    }

    #[test]
    fn runtime_init_error_and_panic_are_terminal_instead_of_permanent_preparing() {
        for (panic_during_init, expected) in [
            (
                false,
                "similar book query runtime init failed: injected init failure",
            ),
            (true, "similar book query worker panicked"),
        ] {
            let (executor, done) = executor_with_done_spawner(move || {
                if panic_during_init {
                    panic!("injected init panic");
                }
                Err("injected init failure".to_owned())
            });
            let client = BookQueryClient::default();
            assert_eq!(executor.query(&client, "a"), BookQueryPoll::Preparing);
            done.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(
                executor.query(&client, "a"),
                BookQueryPoll::Failed(expected.to_owned())
            );
        }
    }

    fn assert_runtime_restart_failure(panic_during_restart: bool, expected: &str) {
        let (command_tx, command_rx) = channel();
        let commands = Arc::new(Mutex::new(command_rx));
        let (event_tx, event_rx) = channel();
        let factory_calls = Arc::new(AtomicU64::new(0));
        let (executor, done) = executor_with_done_spawner({
            let commands = Arc::clone(&commands);
            let events = event_tx;
            let factory_calls = Arc::clone(&factory_calls);
            move || {
                let call = factory_calls.fetch_add(1, Ordering::AcqRel);
                if call == 0 {
                    return Ok(Box::new(MockRuntime {
                        id: 1,
                        commands: Arc::clone(&commands),
                        events: events.clone(),
                    }) as Box<dyn BookQueryRuntime<i32>>);
                }
                if panic_during_restart {
                    panic!("injected restart panic");
                }
                Err("injected restart failure".to_owned())
            }
        });
        let first = BookQueryClient::default();
        let next = BookQueryClient::default();
        assert_eq!(executor.query(&first, "first"), BookQueryPoll::Preparing);
        match event_rx.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::Started { key, .. } => assert_eq!(key, "first"),
            event => panic!("unexpected first-runtime event: {event:?}"),
        }
        assert_eq!(executor.query(&next, "next"), BookQueryPoll::Preparing);
        command_tx.send(Command::Panic).unwrap();
        match event_rx.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::RuntimeDropped(1) => {}
            event => panic!("unexpected restart event: {event:?}"),
        }
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(
            executor.query(&next, "next"),
            BookQueryPoll::Failed(expected.to_owned())
        );
        assert_eq!(factory_calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn runtime_restart_error_and_panic_are_terminal_instead_of_leaving_the_queue_stuck() {
        assert_runtime_restart_failure(
            false,
            "similar book query runtime restart failed: injected restart failure",
        );
        assert_runtime_restart_failure(true, "similar book query worker panicked");
    }

    #[test]
    fn dropping_before_a_delayed_worker_entry_never_creates_the_runtime() {
        let factory_calls = Arc::new(AtomicU64::new(0));
        let pending_job = Arc::new(Mutex::new(None::<ThreadJob>));
        let executor: BookQueryExecutor<i32> = BookQueryExecutor::with_spawner(
            {
                let factory_calls = Arc::clone(&factory_calls);
                move || {
                    factory_calls.fetch_add(1, Ordering::AcqRel);
                    Ok(Box::new(CountingRuntime {
                        executes: Arc::new(AtomicU64::new(0)),
                        drops: Arc::new(AtomicU64::new(0)),
                    }))
                }
            },
            {
                let pending_job = Arc::downgrade(&pending_job);
                move |job| {
                    *pending_job
                        .upgrade()
                        .ok_or_else(|| "delayed job slot was dropped".to_owned())?
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(job);
                    Ok(())
                }
            },
        );
        let client = BookQueryClient::default();
        assert_eq!(executor.query(&client, "a"), BookQueryPoll::Preparing);
        drop(executor);
        pending_job
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .unwrap()();
        assert_eq!(factory_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn stopping_while_the_initial_factory_is_blocked_drops_runtime_without_executing() {
        let (factory_started_tx, factory_started_rx) = channel();
        let (release_factory_tx, release_factory_rx) = channel();
        let release_factory = Arc::new(Mutex::new(release_factory_rx));
        let executes = Arc::new(AtomicU64::new(0));
        let drops = Arc::new(AtomicU64::new(0));
        let (executor, done) = executor_with_done_spawner({
            let release_factory = Arc::clone(&release_factory);
            let executes = Arc::clone(&executes);
            let drops = Arc::clone(&drops);
            move || {
                factory_started_tx.send(()).unwrap();
                release_factory
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .recv()
                    .unwrap();
                Ok(Box::new(CountingRuntime {
                    executes: Arc::clone(&executes),
                    drops: Arc::clone(&drops),
                }))
            }
        });
        let client = BookQueryClient::default();
        assert_eq!(executor.query(&client, "a"), BookQueryPoll::Preparing);
        factory_started_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let (drop_entered_tx, drop_entered_rx) = channel();
        let (drop_returned_tx, drop_returned_rx) = channel();
        let dropper = std::thread::spawn(move || {
            drop_entered_tx.send(()).unwrap();
            drop(executor);
            drop_returned_tx.send(()).unwrap();
        });
        drop_entered_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let returned_before_release = drop_returned_rx.recv_timeout(Duration::from_secs(3));
        release_factory_tx.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        dropper.join().unwrap();
        assert!(
            returned_before_release.is_ok(),
            "executor Drop blocked on the worker/factory: {returned_before_release:?}"
        );
        assert_eq!(executes.load(Ordering::Acquire), 0);
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn stopping_while_panicked_runtime_drop_is_blocked_skips_restart_factory() {
        struct PanicRuntime {
            drop_started: Sender<()>,
            release_drop: Arc<Mutex<Receiver<()>>>,
            drops: Arc<AtomicU64>,
        }

        impl Drop for PanicRuntime {
            fn drop(&mut self) {
                self.drop_started.send(()).unwrap();
                self.release_drop
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .recv()
                    .unwrap();
                self.drops.fetch_add(1, Ordering::AcqRel);
            }
        }

        impl BookQueryRuntime<i32> for PanicRuntime {
            fn execute(
                &mut self,
                _request: BookQueryRequest,
                _cancel: Arc<AtomicBool>,
            ) -> BookQueryTerminal<i32> {
                panic!("injected job panic before blocking Drop");
            }
        }

        let (drop_started_tx, drop_started_rx) = channel();
        let (release_drop_tx, release_drop_rx) = channel();
        let release_drop = Arc::new(Mutex::new(release_drop_rx));
        let factory_calls = Arc::new(AtomicU64::new(0));
        let drops = Arc::new(AtomicU64::new(0));
        let (executor, done) = executor_with_done_spawner({
            let release_drop = Arc::clone(&release_drop);
            let factory_calls = Arc::clone(&factory_calls);
            let drops = Arc::clone(&drops);
            move || {
                let call = factory_calls.fetch_add(1, Ordering::AcqRel);
                assert_eq!(call, 0, "restart factory ran after shutdown");
                Ok(Box::new(PanicRuntime {
                    drop_started: drop_started_tx.clone(),
                    release_drop: Arc::clone(&release_drop),
                    drops: Arc::clone(&drops),
                }))
            }
        });
        let client = BookQueryClient::default();
        assert_eq!(executor.query(&client, "a"), BookQueryPoll::Preparing);
        drop_started_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let (drop_entered_tx, drop_entered_rx) = channel();
        let (drop_returned_tx, drop_returned_rx) = channel();
        let dropper = std::thread::spawn(move || {
            drop_entered_tx.send(()).unwrap();
            drop(executor);
            drop_returned_tx.send(()).unwrap();
        });
        drop_entered_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let returned_before_release = drop_returned_rx.recv_timeout(Duration::from_secs(3));
        release_drop_tx.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        dropper.join().unwrap();
        assert!(
            returned_before_release.is_ok(),
            "executor Drop blocked on panicked runtime Drop: {returned_before_release:?}"
        );
        assert_eq!(factory_calls.load(Ordering::Acquire), 1);
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn executor_drop_never_joins_and_worker_resources_drop_after_job_terminal() {
        let mut harness = Harness::new();
        let client = BookQueryClient::default();
        assert_eq!(
            harness.executor().query(&client, "a"),
            BookQueryPoll::Preparing
        );
        let (_, _, cancel) = harness.started();

        let executor = harness.executor.take().unwrap();
        let (drop_entered_tx, drop_entered_rx) = channel();
        let (drop_returned_tx, drop_returned_rx) = channel();
        let dropper = std::thread::spawn(move || {
            drop_entered_tx.send(()).unwrap();
            drop(executor);
            drop_returned_tx.send(()).unwrap();
        });
        drop_entered_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let returned_before_release = drop_returned_rx.recv_timeout(Duration::from_secs(3));
        // The mock runtime is still blocked, proving owner Drop did not join the worker.
        assert!(matches!(
            harness.events.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        harness.commands.send(Command::Complete(1)).unwrap();
        match harness.events.recv_timeout(Duration::from_secs(3)).unwrap() {
            Event::RuntimeDropped(1) => {}
            event => panic!("unexpected shutdown event: {event:?}"),
        }
        dropper.join().unwrap();
        assert!(
            returned_before_release.is_ok(),
            "executor Drop joined the blocked worker: {returned_before_release:?}"
        );
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(harness.next_runtime_id.load(Ordering::Relaxed), 2);
    }
}
