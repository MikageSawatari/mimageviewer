#[cfg(any(test, feature = "dev-tools"))]
use super::raw_decoder::{
    RawBenchBrightness, RawMatchPreviewOutput, develop_bench, develop_match_preview,
};
use super::raw_decoder::{
    RawBrightness, RawCancellation, RawDevelopOutput, RawDevelopScale, RawError, RawOwnedSource,
    develop,
};
use image::DynamicImage;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

type RawResult = Result<DynamicImage, RawError>;
type Work = Box<dyn FnOnce(&RawCancellation, &AtomicU8) -> RawResult + Send + 'static>;
type ProductResult = Result<RawDevelopOutput, RawError>;
type ProductWork = Box<dyn FnOnce(&RawCancellation, &AtomicU8) -> ProductResult + Send + 'static>;
type ProductCompletion = Box<dyn FnOnce(ProductResult) + Send + 'static>;
#[cfg(any(test, feature = "dev-tools"))]
type MatchResult = Result<RawMatchPreviewOutput, RawError>;
#[cfg(any(test, feature = "dev-tools"))]
type MatchWork = Box<dyn FnOnce(&RawCancellation, &AtomicU8) -> MatchResult + Send + 'static>;

enum JobAction {
    Image {
        result: mpsc::Sender<RawResult>,
        work: Work,
    },
    Product {
        result: mpsc::Sender<ProductResult>,
        work: ProductWork,
    },
    ProductCallback {
        complete: ProductCompletion,
        work: ProductWork,
    },
    #[cfg(any(test, feature = "dev-tools"))]
    Match {
        result: mpsc::Sender<MatchResult>,
        work: MatchWork,
    },
}

enum Completion {
    Image(mpsc::Sender<RawResult>, RawResult),
    Product(mpsc::Sender<ProductResult>, ProductResult),
    ProductCallback(ProductCompletion, ProductResult),
    #[cfg(any(test, feature = "dev-tools"))]
    Match(mpsc::Sender<MatchResult>, MatchResult),
}

impl JobAction {
    fn cancel(self) {
        match self {
            Self::Image { result, .. } => {
                let _ = result.send(Err(RawError::Cancelled));
            }
            Self::Product { result, .. } => {
                let _ = result.send(Err(RawError::Cancelled));
            }
            Self::ProductCallback { complete, .. } => complete(Err(RawError::Cancelled)),
            #[cfg(any(test, feature = "dev-tools"))]
            Self::Match { result, .. } => {
                let _ = result.send(Err(RawError::Cancelled));
            }
        }
    }

    fn run(self, cancel: &RawCancellation, progress: &AtomicU8) -> Completion {
        match self {
            Self::Image { result, work } => Completion::Image(result, work(cancel, progress)),
            Self::Product { result, work } => Completion::Product(result, work(cancel, progress)),
            Self::ProductCallback { complete, work } => {
                Completion::ProductCallback(complete, work(cancel, progress))
            }
            #[cfg(any(test, feature = "dev-tools"))]
            Self::Match { result, work } => Completion::Match(result, work(cancel, progress)),
        }
    }
}

impl Completion {
    fn send(self, cancelled: bool) {
        match self {
            Self::Image(sender, result) => {
                let _ = sender.send(if cancelled {
                    Err(RawError::Cancelled)
                } else {
                    result
                });
            }
            Self::Product(sender, result) => {
                let _ = sender.send(if cancelled {
                    Err(RawError::Cancelled)
                } else {
                    result
                });
            }
            Self::ProductCallback(complete, result) => {
                complete(if cancelled {
                    Err(RawError::Cancelled)
                } else {
                    result
                });
            }
            #[cfg(any(test, feature = "dev-tools"))]
            Self::Match(sender, result) => {
                let _ = sender.send(if cancelled {
                    Err(RawError::Cancelled)
                } else {
                    result
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPriority {
    High,
    Normal,
    Background,
}

impl RawPriority {
    fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Background => "background",
        }
    }
}

struct Job {
    id: u64,
    priority: RawPriority,
    queued_at: Instant,
    cancel: Arc<RawCancellation>,
    progress: Arc<AtomicU8>,
    action: JobAction,
}

struct Running {
    priority: RawPriority,
    cancel: Arc<RawCancellation>,
    cancel_at: Option<Instant>,
}

struct State {
    desired: usize,
    live: usize,
    next_id: u64,
    closed: bool,
    high: VecDeque<Job>,
    normal: VecDeque<Job>,
    background: VecDeque<Job>,
    running: HashMap<u64, Running>,
}

impl State {
    fn waiting(&self) -> usize {
        self.high.len() + self.normal.len() + self.background.len()
    }

    fn non_high(&self) -> usize {
        self.running
            .values()
            .filter(|job| job.priority != RawPriority::High)
            .count()
    }

    fn background_running(&self) -> usize {
        self.running
            .values()
            .filter(|job| job.priority == RawPriority::Background)
            .count()
    }

    fn queue(&mut self, priority: RawPriority) -> &mut VecDeque<Job> {
        match priority {
            RawPriority::High => &mut self.high,
            RawPriority::Normal => &mut self.normal,
            RawPriority::Background => &mut self.background,
        }
    }

    fn pick(&mut self) -> Option<Job> {
        if self.running.len() >= self.desired {
            return None;
        }
        if let Some(job) = self.high.pop_front() {
            return Some(job);
        }
        let non_high_limit = if self.desired == 1 {
            1
        } else {
            self.desired - 1
        };
        if self.non_high() >= non_high_limit {
            return None;
        }
        if let Some(job) = self.normal.pop_front() {
            return Some(job);
        }
        if self.background_running() == 0 {
            self.background.pop_front()
        } else {
            None
        }
    }

    fn remove_waiting(&mut self, id: u64) -> Option<Job> {
        for queue in [&mut self.high, &mut self.normal, &mut self.background] {
            if let Some(index) = queue.iter().position(|job| job.id == id) {
                return queue.remove(index);
            }
        }
        None
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

fn emit(
    kind: &'static str,
    id: u64,
    priority: RawPriority,
    waiting: usize,
    running: usize,
    elapsed: Option<Duration>,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut fields = vec![
        ("job_id", serde_json::Value::from(id)),
        ("priority", serde_json::Value::from(priority.label())),
        ("waiting", serde_json::Value::from(waiting)),
        ("running", serde_json::Value::from(running)),
    ];
    if let Some(elapsed) = elapsed {
        fields.push((
            "elapsed_ms",
            serde_json::Value::from(elapsed.as_secs_f64() * 1000.0),
        ));
    }
    crate::perf::event("raw", kind, None, id, &fields);
}

pub struct RawTicket {
    id: u64,
    shared: Weak<Shared>,
    cancel: Arc<RawCancellation>,
    progress: Arc<AtomicU8>,
}

impl RawTicket {
    pub fn progress(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.progress)
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let (waiting_job, priority, waiting, running) = {
            let mut state = shared.state.lock().unwrap();
            let waiting_job = state.remove_waiting(self.id);
            let priority = if let Some(job) = &waiting_job {
                job.priority
            } else if let Some(job) = state.running.get_mut(&self.id) {
                job.cancel_at.get_or_insert_with(Instant::now);
                job.priority
            } else {
                return;
            };
            (waiting_job, priority, state.waiting(), state.running.len())
        };
        shared.wake.notify_all();
        if let Some(job) = waiting_job {
            job.action.cancel();
            emit(
                "executor_cancel_waiting",
                self.id,
                priority,
                waiting,
                running,
                None,
            );
        } else {
            emit(
                "executor_cancelling",
                self.id,
                priority,
                waiting,
                running,
                None,
            );
        }
    }

    pub fn promote_to_high(&self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let (changed, waiting, running) = {
            let mut state = shared.state.lock().unwrap();
            let mut changed = false;
            if state.high.iter().any(|job| job.id == self.id) {
                return;
            }
            if let Some(mut job) = state.remove_waiting(self.id) {
                job.priority = RawPriority::High;
                changed = true;
                let position = state
                    .high
                    .iter()
                    .position(|queued| queued.id > job.id)
                    .unwrap_or(state.high.len());
                state.high.insert(position, job);
            } else if let Some(job) = state.running.get_mut(&self.id)
                && job.priority != RawPriority::High
            {
                job.priority = RawPriority::High;
                changed = true;
            }
            (changed, state.waiting(), state.running.len())
        };
        if changed {
            shared.wake.notify_all();
            emit(
                "executor_promote",
                self.id,
                RawPriority::High,
                waiting,
                running,
                None,
            );
        }
    }
}

pub struct RawDevelopExecutor {
    shared: Arc<Shared>,
}

impl RawDevelopExecutor {
    pub fn new(parallelism: usize) -> std::io::Result<Self> {
        assert!(
            (1..=10).contains(&parallelism),
            "RAW parallelism must be 1..=10"
        );
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                desired: 0,
                live: 0,
                next_id: 1,
                closed: false,
                high: VecDeque::new(),
                normal: VecDeque::new(),
                background: VecDeque::new(),
                running: HashMap::new(),
            }),
            wake: Condvar::new(),
        });
        let this = Self { shared };
        this.set_parallelism(parallelism)?;
        Ok(this)
    }

    pub fn set_parallelism(&self, parallelism: usize) -> std::io::Result<()> {
        assert!(
            (1..=10).contains(&parallelism),
            "RAW parallelism must be 1..=10"
        );
        let spawn_count = {
            let mut state = self.shared.state.lock().unwrap();
            if state.closed {
                return Err(std::io::Error::other("RAW executor closed"));
            }
            state.desired = parallelism;
            let count = parallelism.saturating_sub(state.live);
            state.live += count;
            count
        };
        for index in 0..spawn_count {
            let shared = Arc::clone(&self.shared);
            if let Err(error) = std::thread::Builder::new()
                .name("raw-develop".into())
                .stack_size(1024 * 1024)
                .spawn(move || worker(shared))
            {
                let mut state = self.shared.state.lock().unwrap();
                state.live -= spawn_count - index;
                self.shared.wake.notify_all();
                return Err(error);
            }
        }
        self.shared.wake.notify_all();
        Ok(())
    }

    /// Cancel queued and running work without joining LibRaw worker threads.
    pub fn shutdown(&self) {
        let (waiting, running) = {
            let mut state = self.shared.state.lock().unwrap();
            if state.closed {
                return;
            }
            state.closed = true;
            let mut waiting: Vec<_> = state.high.drain(..).collect();
            waiting.extend(state.normal.drain(..));
            waiting.extend(state.background.drain(..));
            let running: Vec<_> = state
                .running
                .values()
                .map(|job| Arc::clone(&job.cancel))
                .collect();
            (waiting, running)
        };
        for job in waiting {
            job.cancel.cancel();
            job.action.cancel();
        }
        for cancel in running {
            cancel.cancel();
        }
        self.shared.wake.notify_all();
    }

    pub fn submit(
        &self,
        source: RawOwnedSource,
        scale: RawDevelopScale,
        brightness: RawBrightness,
        priority: RawPriority,
        result: mpsc::Sender<ProductResult>,
    ) -> RawTicket {
        self.submit_with_cancel_flag(source, scale, brightness, priority, result, None)
    }

    /// Share the owning request's cancellation flag with the RAW job. A cancelled
    /// fullscreen ticket is observed by LibRaw's progress callback without a
    /// polling bridge or another waiting thread.
    pub fn submit_with_cancel_flag(
        &self,
        source: RawOwnedSource,
        scale: RawDevelopScale,
        brightness: RawBrightness,
        priority: RawPriority,
        result: mpsc::Sender<ProductResult>,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> RawTicket {
        self.submit_action(
            priority,
            JobAction::Product {
                result,
                work: Box::new(move |cancel, progress| {
                    develop(source.as_source(), scale, brightness, cancel, progress)
                }),
            },
            cancel_flag,
        )
    }

    /// Run only LibRaw inside the execution slot. The completion runs after the
    /// slot has been released, so callers may queue resize/cache work elsewhere.
    pub fn submit_with_completion(
        &self,
        source: RawOwnedSource,
        scale: RawDevelopScale,
        brightness: RawBrightness,
        priority: RawPriority,
        complete: impl FnOnce(ProductResult) + Send + 'static,
    ) -> Result<RawTicket, RawError> {
        let (ticket, rejected) = self.submit_action_inner(
            priority,
            JobAction::ProductCallback {
                complete: Box::new(complete),
                work: Box::new(move |cancel, progress| {
                    develop(source.as_source(), scale, brightness, cancel, progress)
                }),
            },
            None,
        );
        if rejected.is_some() {
            Err(RawError::Cancelled)
        } else {
            Ok(ticket)
        }
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub fn submit_bench(
        &self,
        source: RawOwnedSource,
        scale: RawDevelopScale,
        brightness: RawBenchBrightness,
        priority: RawPriority,
        result: mpsc::Sender<RawResult>,
    ) -> RawTicket {
        self.submit_work(
            priority,
            result,
            Box::new(move |cancel, progress| {
                develop_bench(source.as_source(), scale, brightness, cancel, progress)
            }),
        )
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub fn submit_match_preview(
        &self,
        source: RawOwnedSource,
        scale: RawDevelopScale,
        preview_median: Option<f64>,
        priority: RawPriority,
        result: mpsc::Sender<MatchResult>,
    ) -> RawTicket {
        self.submit_action(
            priority,
            JobAction::Match {
                result,
                work: Box::new(move |cancel, progress| {
                    develop_match_preview(
                        source.as_source(),
                        scale,
                        preview_median,
                        cancel,
                        progress,
                    )
                }),
            },
            None,
        )
    }

    #[cfg(any(test, feature = "dev-tools"))]
    fn submit_work(
        &self,
        priority: RawPriority,
        result: mpsc::Sender<RawResult>,
        work: Work,
    ) -> RawTicket {
        self.submit_action(priority, JobAction::Image { result, work }, None)
    }

    fn submit_action(
        &self,
        priority: RawPriority,
        action: JobAction,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> RawTicket {
        let (ticket, rejected) = self.submit_action_inner(priority, action, cancel_flag);
        if let Some(action) = rejected {
            action.cancel();
        }
        ticket
    }

    fn submit_action_inner(
        &self,
        priority: RawPriority,
        action: JobAction,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> (RawTicket, Option<JobAction>) {
        let cancel =
            Arc::new(cancel_flag.map_or_else(RawCancellation::new, RawCancellation::with_flag));
        let progress = Arc::new(AtomicU8::new(0));
        let mut action = Some(action);
        let (id, waiting, running, closed) = {
            let mut state = self.shared.state.lock().unwrap();
            let id = state.next_id;
            state.next_id += 1;
            let closed = state.closed;
            if !closed {
                state.queue(priority).push_back(Job {
                    id,
                    priority,
                    queued_at: Instant::now(),
                    cancel: Arc::clone(&cancel),
                    progress: Arc::clone(&progress),
                    action: action.take().unwrap(),
                });
            }
            (id, state.waiting(), state.running.len(), closed)
        };
        if !closed {
            self.shared.wake.notify_all();
            emit("executor_enqueue", id, priority, waiting, running, None);
        }
        (
            RawTicket {
                id,
                shared: Arc::downgrade(&self.shared),
                cancel,
                progress,
            },
            action,
        )
    }
}

impl Drop for RawDevelopExecutor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(
        executor: &RawDevelopExecutor,
        priority: RawPriority,
        name: &'static str,
        starts: mpsc::Sender<&'static str>,
    ) -> (RawTicket, mpsc::Sender<()>, mpsc::Receiver<RawResult>) {
        let (release, gate) = mpsc::channel();
        let (result, receive) = mpsc::channel();
        let ticket = executor.submit_work(
            priority,
            result,
            Box::new(move |_, _| {
                let _ = starts.send(name);
                let _ = gate.recv();
                Ok(DynamicImage::new_rgb8(1, 1))
            }),
        );
        (ticket, release, receive)
    }

    fn started(rx: &mpsc::Receiver<&'static str>) -> &'static str {
        rx.recv_timeout(Duration::from_secs(2))
            .expect("fake job should start")
    }

    fn still_waiting(rx: &mpsc::Receiver<&'static str>) {
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn reserves_high_slot_and_limits_background() {
        let executor = RawDevelopExecutor::new(3).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_, n1, _) = fake(&executor, RawPriority::Normal, "normal1", tx.clone());
        let (_, n2, _) = fake(&executor, RawPriority::Normal, "normal2", tx.clone());
        let mut running = [started(&rx), started(&rx)];
        running.sort_unstable();
        assert_eq!(running, ["normal1", "normal2"]);
        let (_, bg1, _) = fake(
            &executor,
            RawPriority::Background,
            "background1",
            tx.clone(),
        );
        let (_, bg2, _) = fake(
            &executor,
            RawPriority::Background,
            "background2",
            tx.clone(),
        );
        still_waiting(&rx);
        let (_, high, _) = fake(&executor, RawPriority::High, "high", tx);
        assert_eq!(started(&rx), "high");
        high.send(()).unwrap();
        still_waiting(&rx);
        n1.send(()).unwrap();
        assert_eq!(started(&rx), "background1");
        still_waiting(&rx);
        bg1.send(()).unwrap();
        assert_eq!(started(&rx), "background2");
        let _ = n2.send(());
        let _ = bg2.send(());
    }

    #[test]
    fn single_worker_orders_high_after_uncancellable_job() {
        let executor = RawDevelopExecutor::new(1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_, active, _) = fake(&executor, RawPriority::Normal, "active", tx.clone());
        assert_eq!(started(&rx), "active");
        let (_, normal, _) = fake(&executor, RawPriority::Normal, "normal", tx.clone());
        let (_, high, _) = fake(&executor, RawPriority::High, "high", tx);
        active.send(()).unwrap();
        assert_eq!(started(&rx), "high");
        high.send(()).unwrap();
        assert_eq!(started(&rx), "normal");
        normal.send(()).unwrap();
    }

    #[test]
    fn promoted_high_keeps_original_acceptance_order() {
        let executor = RawDevelopExecutor::new(1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_, active, _) = fake(&executor, RawPriority::Normal, "active", tx.clone());
        assert_eq!(started(&rx), "active");
        let (normal_a, normal_a_gate, _) =
            fake(&executor, RawPriority::Normal, "normal_a", tx.clone());
        let (_, high_b_gate, _) = fake(&executor, RawPriority::High, "high_b", tx);
        normal_a.promote_to_high();
        active.send(()).unwrap();
        assert_eq!(started(&rx), "normal_a");
        normal_a_gate.send(()).unwrap();
        assert_eq!(started(&rx), "high_b");
        high_b_gate.send(()).unwrap();
    }

    #[test]
    fn promotion_and_waiting_cancel() {
        let executor = RawDevelopExecutor::new(1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_, active, _) = fake(&executor, RawPriority::Normal, "active", tx.clone());
        assert_eq!(started(&rx), "active");
        let (_, normal, _) = fake(&executor, RawPriority::Normal, "normal", tx.clone());
        let (promoted, promoted_gate, _) =
            fake(&executor, RawPriority::Background, "promoted", tx.clone());
        let (cancelled, _, cancelled_result) = fake(&executor, RawPriority::High, "cancelled", tx);
        cancelled.cancel();
        assert!(matches!(
            cancelled_result.recv_timeout(Duration::from_secs(1)),
            Ok(Err(RawError::Cancelled))
        ));
        promoted.promote_to_high();
        active.send(()).unwrap();
        assert_eq!(started(&rx), "promoted");
        promoted_gate.send(()).unwrap();
        assert_eq!(started(&rx), "normal");
        normal.send(()).unwrap();
    }

    #[test]
    fn cancelling_running_job_keeps_slot_until_exit() {
        let executor = RawDevelopExecutor::new(1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (active_ticket, active, active_result) =
            fake(&executor, RawPriority::Normal, "active", tx.clone());
        assert_eq!(started(&rx), "active");
        active_ticket.cancel();
        let (_, high, _) = fake(&executor, RawPriority::High, "high", tx);
        still_waiting(&rx);
        active.send(()).unwrap();
        assert!(matches!(
            active_result.recv_timeout(Duration::from_secs(1)),
            Ok(Err(RawError::Cancelled))
        ));
        assert_eq!(started(&rx), "high");
        high.send(()).unwrap();
    }

    #[test]
    fn increases_and_decreases_worker_count_without_killing_jobs() {
        let executor = RawDevelopExecutor::new(1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_, first, first_result) = fake(&executor, RawPriority::High, "first", tx.clone());
        assert_eq!(started(&rx), "first");
        let (_, second, second_result) = fake(&executor, RawPriority::High, "second", tx.clone());
        still_waiting(&rx);
        executor.set_parallelism(2).unwrap();
        assert_eq!(started(&rx), "second");
        executor.set_parallelism(1).unwrap();
        let (_, third, _) = fake(&executor, RawPriority::High, "third", tx);
        first.send(()).unwrap();
        assert!(
            first_result
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_ok()
        );
        still_waiting(&rx);
        second.send(()).unwrap();
        assert!(
            second_result
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_ok()
        );
        assert_eq!(started(&rx), "third");
        third.send(()).unwrap();
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let (job, waiting, running) = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.closed || state.live > state.desired {
                    state.live -= 1;
                    shared.wake.notify_all();
                    return;
                }
                if let Some(job) = state.pick() {
                    let waiting = state.waiting();
                    state.running.insert(
                        job.id,
                        Running {
                            priority: job.priority,
                            cancel: Arc::clone(&job.cancel),
                            cancel_at: None,
                        },
                    );
                    break (job, waiting, state.running.len());
                }
                state = shared.wake.wait(state).unwrap();
            }
        };
        emit(
            "executor_start",
            job.id,
            job.priority,
            waiting,
            running,
            Some(job.queued_at.elapsed()),
        );
        let started = Instant::now();
        let completion = job.action.run(&job.cancel, &job.progress);
        let cancelled = job.cancel.flag.load(Ordering::Acquire);
        let (priority, cancel_at, waiting, running) = {
            let mut state = shared.state.lock().unwrap();
            let running_job = state.running.remove(&job.id).expect("running RAW job");
            let values = (
                running_job.priority,
                running_job.cancel_at,
                state.waiting(),
                state.running.len(),
            );
            shared.wake.notify_all();
            values
        };
        completion.send(cancelled);
        emit(
            "executor_finish",
            job.id,
            priority,
            waiting,
            running,
            Some(started.elapsed()),
        );
        if let Some(cancel_at) = cancel_at {
            emit(
                "executor_cancel_exit",
                job.id,
                priority,
                waiting,
                running,
                Some(cancel_at.elapsed()),
            );
        }
    }
}
