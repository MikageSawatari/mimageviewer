//! Bounded, normal-thread-only diagnostic publication. This is not a fatal
//! reader: producers attempt one lock, and activity ownership is atomic.
use super::{RUNNING, Shared, now_ms};
use crate::gpu_info::diagnostic_sampler::{AdapterProbe, GpuMemorySample, MemorySample};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const JOB_SLOTS: usize = 6;
pub(super) const PERIOD: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub utc_ms: u64,
    pub run_elapsed_ms: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticModel {
    #[default]
    Unknown,
    UpscaleRealEsrganX4Plus,
    UpscaleRealEsrganAnime6B,
    UpscaleRealEsrGeneralV3,
    UpscaleRealCugan4x,
    UpscaleNmkdSiax4x,
    DenoiseRealplksr,
    InpaintMiGan,
    SubjectMatte,
}
impl From<crate::ai::ModelKind> for DiagnosticModel {
    fn from(model: crate::ai::ModelKind) -> Self {
        use crate::ai::ModelKind;
        match model {
            ModelKind::UpscaleRealEsrganX4Plus => Self::UpscaleRealEsrganX4Plus,
            ModelKind::UpscaleRealEsrganAnime6B => Self::UpscaleRealEsrganAnime6B,
            ModelKind::UpscaleRealEsrGeneralV3 => Self::UpscaleRealEsrGeneralV3,
            ModelKind::UpscaleRealCugan4x => Self::UpscaleRealCugan4x,
            ModelKind::UpscaleNmkdSiax4x => Self::UpscaleNmkdSiax4x,
            ModelKind::DenoiseRealplksr => Self::DenoiseRealplksr,
            ModelKind::InpaintMiGan => Self::InpaintMiGan,
            ModelKind::SubjectMatte => Self::SubjectMatte,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticBackend {
    #[default]
    Unknown,
    DirectMl,
    TensorRt,
    Cpu,
}
impl From<crate::ai::AiBackend> for DiagnosticBackend {
    fn from(backend: crate::ai::AiBackend) -> Self {
        match backend {
            crate::ai::AiBackend::DirectMl => Self::DirectMl,
            crate::ai::AiBackend::TensorRt => Self::TensorRt,
            crate::ai::AiBackend::Cpu => Self::Cpu,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobMetadata {
    pub context_id: Option<u64>,
    pub items_generation: Option<u64>,
    pub job_id: u64,
    pub page: Option<u32>,
    pub total: Option<u32>,
    pub upscale_model: Option<DiagnosticModel>,
    pub denoise_model: Option<DiagnosticModel>,
    pub backend: DiagnosticBackend,
    pub source: Option<Dimensions>,
    pub source_bytes: Option<u64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum JobStage {
    #[default]
    Queued,
    ModelLoad,
    ScaleDetect,
    TileRun,
    Blend,
    ResultWaiting,
    Adopting,
    FinalEffectPending,
    FinalEffect,
    GpuRegistration,
    Completed,
    Discarded,
    Failed,
}
impl JobStage {
    const COUNT: usize = 13;
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Discarded | Self::Failed)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobUpdate {
    pub stage: JobStage,
    pub backend: Option<DiagnosticBackend>,
    pub model: Option<DiagnosticModel>,
    pub output: Option<Dimensions>,
    pub tile: Option<Dimensions>,
    pub tile_index: Option<u32>,
    pub tile_total: Option<u32>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySnapshot {
    pub context_id: u64,
    pub items_generation: u64,
    pub page: Option<u32>,
    pub total: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observed<T> {
    pub stamp: Stamp,
    pub value: T,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JobObservation {
    token: u64,
    pub metadata: JobMetadata,
    pub update: JobUpdate,
    pub stamp: Stamp,
    pub active: bool,
    pub cancel_requested: bool,
    pub cancel_stamp: Stamp,
}
#[derive(Clone)]
struct JobSlot {
    observation: JobObservation,
    cancel: Arc<AtomicBool>,
    activity: Weak<ActivityGroup>,
}
#[derive(Default)]
struct Mailbox {
    display: Option<Observed<DisplaySnapshot>>,
    jobs: [Option<JobSlot>; JOB_SLOTS],
}
pub(super) struct Publication {
    origin: Instant,
    active: AtomicU64,
    jobs: AtomicU64,
    next_token: AtomicU64,
    boundaries: AtomicU64,
    dropped: AtomicU64,
    stages: [AtomicU64; JobStage::COUNT],
    queued_source_bytes: AtomicU64,
    queued_unknown_sources: AtomicU64,
    // One immutable source, installed by the sole UI initializer. The writer
    // only uses get(), so mailbox contention cannot drop its registration.
    adapter: OnceLock<AdapterProbe>,
    mailbox: Mutex<Mailbox>,
}
impl Publication {
    pub(super) fn new() -> Self {
        Self {
            origin: Instant::now(),
            active: AtomicU64::new(0),
            jobs: AtomicU64::new(0),
            next_token: AtomicU64::new(1),
            boundaries: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            stages: std::array::from_fn(|_| AtomicU64::new(0)),
            queued_source_bytes: AtomicU64::new(0),
            queued_unknown_sources: AtomicU64::new(0),
            adapter: OnceLock::new(),
            mailbox: Mutex::new(Mailbox::default()),
        }
    }
    pub(super) fn stamp(&self) -> Stamp {
        Stamp {
            utc_ms: now_ms(),
            run_elapsed_ms: self.elapsed_ms(),
        }
    }
    pub(super) fn elapsed_ms(&self) -> u64 {
        self.origin
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    pub(super) fn active(&self) -> u64 {
        self.active.load(Ordering::Acquire)
    }
    pub(super) fn boundaries(&self) -> u64 {
        self.boundaries.load(Ordering::Acquire)
    }
    fn queue_source(&self, source: Option<u64>, add: bool) {
        let (counter, amount) = match source {
            Some(bytes) => (&self.queued_source_bytes, bytes),
            None => (&self.queued_unknown_sources, 1),
        };
        if add {
            counter.fetch_add(amount, Ordering::AcqRel);
        } else {
            counter.fetch_sub(amount, Ordering::AcqRel);
        }
    }
    // The writer alone may wait for this short metadata copy. No sampling,
    // serialization, or file operation runs under this guard.
    pub(super) fn snapshot(&self) -> PendingSnapshot {
        let mailbox = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
        PendingSnapshot {
            display: mailbox.display.clone(),
            jobs: mailbox.jobs.clone(),
            adapter: self.adapter.get().cloned(),
        }
    }
}
pub(super) struct PendingSnapshot {
    display: Option<Observed<DisplaySnapshot>>,
    jobs: [Option<JobSlot>; JOB_SLOTS],
    pub adapter: Option<AdapterProbe>,
}

#[derive(Clone)]
pub struct CheckpointPublisher(Arc<Shared>);
impl CheckpointPublisher {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        Self(shared)
    }
    #[cfg(test)]
    pub(crate) fn active_count_for_test(&self) -> u64 {
        self.0.checkpoint.active()
    }
    #[cfg(test)]
    pub(crate) fn display_for_test(&self) -> Option<DisplaySnapshot> {
        self.0
            .checkpoint
            .snapshot()
            .display
            .map(|display| display.value)
    }
    #[cfg(test)]
    pub(crate) fn queue_counts_for_test(&self) -> (u64, Option<u64>, u64) {
        let checkpoint = super::capture_checkpoint(&self.0, &mut |_| {
            let stamp = self.0.checkpoint.stamp();
            (MemorySample::default(), stamp, stamp)
        });
        (
            checkpoint.queued_jobs,
            checkpoint.queued_source_bytes,
            checkpoint.result_waiting_jobs,
        )
    }
    fn publish(&self, update: impl FnOnce(&mut Mailbox, Stamp)) {
        if self.0.state.load(Ordering::Acquire) != RUNNING {
            return;
        }
        // Exactly one nonwaiting attempt, also for the UI and drop paths.
        let Ok(mut mailbox) = self.0.checkpoint.mailbox.try_lock() else {
            self.0.checkpoint.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if self.0.state.load(Ordering::Acquire) == RUNNING {
            update(&mut mailbox, self.0.checkpoint.stamp());
        }
    }
    pub fn publish_display(&self, value: DisplaySnapshot) {
        self.publish(|mailbox, stamp| mailbox.display = Some(Observed { stamp, value }));
    }
    /// The sole startup UI initializer installs an existing adapter/device
    /// owner once. The writer never initializes this cell or waits on it.
    /// No query, mailbox lock, retry, or periodic wake is needed here.
    pub(crate) fn set_adapter(&self, adapter: AdapterProbe) {
        if self.0.state.load(Ordering::Acquire) == RUNNING {
            let _ = self.0.checkpoint.adapter.set(adapter);
        }
    }
    /// Clones share one real activity group, including after UI pending removal.
    pub fn activity(&self) -> ActivityLease {
        if self.0.checkpoint.active.fetch_add(1, Ordering::AcqRel) == 0 {
            self.boundary();
        }
        ActivityLease(Arc::new(ActivityGroup(self.clone())))
    }
    fn boundary(&self) {
        self.0.checkpoint.boundaries.fetch_add(1, Ordering::AcqRel);
        self.0.wake();
    }
    pub fn begin_job(&self, metadata: JobMetadata, cancel: Arc<AtomicBool>) -> JobLease {
        let token = self.0.checkpoint.next_token.fetch_add(1, Ordering::Relaxed);
        self.0.checkpoint.jobs.fetch_add(1, Ordering::AcqRel);
        self.0.checkpoint.stages[JobStage::Queued as usize].fetch_add(1, Ordering::AcqRel);
        self.0.checkpoint.queue_source(metadata.source_bytes, true);
        let activity = self.activity();
        let lease = JobLease(Arc::new(JobGroup {
            publisher: self.clone(),
            token,
            metadata,
            cancel,
            stage: AtomicU8::new(JobStage::Queued as u8),
            _activity: activity,
        }));
        lease.update(JobUpdate::default());
        lease
    }
}

#[derive(Clone)]
pub struct ActivityLease(Arc<ActivityGroup>);
struct ActivityGroup(CheckpointPublisher);
impl Drop for ActivityGroup {
    fn drop(&mut self) {
        if self.0.0.checkpoint.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.boundary();
        }
    }
}
#[derive(Clone)]
pub struct JobLease(Arc<JobGroup>);
struct JobGroup {
    publisher: CheckpointPublisher,
    token: u64,
    metadata: JobMetadata,
    cancel: Arc<AtomicBool>,
    stage: AtomicU8,
    _activity: ActivityLease,
}
impl JobLease {
    pub fn metadata(&self) -> JobMetadata {
        self.0.metadata
    }
    pub fn update(&self, update: JobUpdate) {
        let group = &self.0;
        if group.publisher.0.state.load(Ordering::Acquire) != RUNNING {
            return;
        }
        let previous = group.stage.swap(update.stage as u8, Ordering::AcqRel);
        if previous != update.stage as u8 {
            let publication = &group.publisher.0.checkpoint;
            publication.stages[update.stage as usize].fetch_add(1, Ordering::AcqRel);
            publication.stages[previous as usize].fetch_sub(1, Ordering::AcqRel);
            if previous == JobStage::Queued as u8 {
                publication.queue_source(group.metadata.source_bytes, false);
            }
            if update.stage == JobStage::Queued {
                publication.queue_source(group.metadata.source_bytes, true);
            }
        }
        group.publisher.publish(|mailbox, stamp| {
            let existing = mailbox.jobs.iter().position(|slot| {
                slot.as_ref()
                    .is_some_and(|slot| slot.observation.token == group.token)
            });
            let index = existing.or_else(|| {
                mailbox.jobs.iter().position(|slot| {
                    slot.as_ref()
                        .is_none_or(|slot| slot.activity.strong_count() == 0)
                })
            });
            if let Some(index) = index {
                mailbox.jobs[index] = Some(JobSlot {
                    observation: JobObservation {
                        token: group.token,
                        metadata: group.metadata,
                        update,
                        stamp,
                        active: true,
                        cancel_requested: false,
                        cancel_stamp: stamp,
                    },
                    cancel: group.cancel.clone(),
                    activity: Arc::downgrade(&group._activity.0),
                });
            }
        });
    }
}
impl Drop for JobGroup {
    fn drop(&mut self) {
        self.publisher.publish(|mailbox, stamp| {
            if let Some(slot) = mailbox
                .jobs
                .iter_mut()
                .flatten()
                .find(|slot| slot.observation.token == self.token)
            {
                slot.observation.active = false;
                if !slot.observation.update.stage.terminal() {
                    slot.observation.update.stage = JobStage::Discarded;
                }
                slot.observation.stamp = stamp;
            }
        });
        self.publisher
            .0
            .checkpoint
            .jobs
            .fetch_sub(1, Ordering::AcqRel);
        let stage = self.stage.load(Ordering::Acquire);
        self.publisher.0.checkpoint.stages[stage as usize].fetch_sub(1, Ordering::AcqRel);
        if stage == JobStage::Queued as u8 {
            self.publisher
                .0
                .checkpoint
                .queue_source(self.metadata.source_bytes, false);
        }
        // _activity's last drop publishes the final boundary after metadata.
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkpoint {
    pub stamp: Stamp,
    pub active_leases: u64,
    pub active_jobs: u64,
    pub omitted_jobs: u64,
    pub dropped_updates: u64,
    pub queued_jobs: u64,
    pub queued_source_bytes: Option<u64>,
    pub running_jobs: u64,
    pub result_waiting_jobs: u64,
    pub final_effect_jobs: u64,
    pub gpu_registration_jobs: u64,
    pub display: Option<Observed<DisplaySnapshot>>,
    pub jobs: Vec<JobObservation>,
    pub cpu_private_bytes: Observed<Option<u64>>,
    pub gpu: Observed<GpuMemorySample>,
    pub ai_adapter_correlation: crate::gpu_info::diagnostic_sampler::AiAdapterCorrelation,
}
impl PendingSnapshot {
    /// Called after dropping the mailbox guard, including cancel flag reads.
    pub(super) fn complete(
        self,
        publication: &Publication,
        sample: MemorySample,
        cpu_stamp: Stamp,
        gpu_stamp: Stamp,
    ) -> Checkpoint {
        let mut jobs: Vec<_> = self
            .jobs
            .into_iter()
            .flatten()
            .map(|slot| {
                let mut observation = slot.observation;
                observation.active = slot.activity.strong_count() != 0;
                observation.cancel_requested = slot.cancel.load(Ordering::Acquire);
                observation.cancel_stamp = publication.stamp();
                observation
            })
            .collect();
        // Latest activity is more useful than a complete page history.
        jobs.sort_by_key(|job| std::cmp::Reverse((job.active, job.stamp.run_elapsed_ms)));
        let active_jobs = publication.jobs.load(Ordering::Acquire);
        let represented = jobs.iter().filter(|job| job.active).count() as u64;
        let count = |stage: JobStage| publication.stages[stage as usize].load(Ordering::Acquire);
        Checkpoint {
            stamp: publication.stamp(),
            active_leases: publication.active(),
            active_jobs,
            omitted_jobs: active_jobs.saturating_sub(represented),
            dropped_updates: publication.dropped.load(Ordering::Relaxed),
            queued_jobs: count(JobStage::Queued),
            queued_source_bytes: (publication.queued_unknown_sources.load(Ordering::Acquire) == 0)
                .then(|| publication.queued_source_bytes.load(Ordering::Acquire)),
            running_jobs: [
                JobStage::ModelLoad,
                JobStage::ScaleDetect,
                JobStage::TileRun,
                JobStage::Blend,
            ]
            .into_iter()
            .map(count)
            .sum(),
            result_waiting_jobs: count(JobStage::ResultWaiting),
            final_effect_jobs: count(JobStage::FinalEffectPending) + count(JobStage::FinalEffect),
            gpu_registration_jobs: count(JobStage::GpuRegistration),
            display: self.display,
            jobs,
            cpu_private_bytes: Observed {
                stamp: cpu_stamp,
                value: sample.cpu_private_bytes,
            },
            gpu: Observed {
                stamp: gpu_stamp,
                value: sample.rendering,
            },
            ai_adapter_correlation: sample.ai_adapter_correlation,
        }
    }
}

impl Checkpoint {
    pub(super) fn valid(&self) -> bool {
        self.jobs.len() <= JOB_SLOTS
    }
}

/// Pure scheduling policy: idle has no periodic deadline. Boundary revisions
/// cannot disappear even if an entire activity starts and ends during I/O.
#[derive(Default)]
pub(super) struct Schedule {
    saved_boundary: u64,
    next_period_ms: Option<u64>,
}
impl Schedule {
    pub(super) fn due(&self, now_ms: u64, active: u64, boundary: u64) -> bool {
        boundary != self.saved_boundary
            || (active != 0 && self.next_period_ms.is_none_or(|next| now_ms >= next))
    }
    pub(super) fn saved(&mut self, now_ms: u64, active: u64, boundary: u64) {
        self.saved_boundary = boundary;
        self.next_period_ms =
            (active != 0).then(|| now_ms.saturating_add(PERIOD.as_millis() as u64));
    }
    pub(super) fn wait(&self, now_ms: u64, active: u64) -> Option<Duration> {
        (active != 0).then(|| {
            Duration::from_millis(self.next_period_ms.unwrap_or(now_ms).saturating_sub(now_ms))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_info::diagnostic_sampler::{DiagnosticSampler, GpuSampleStatus};
    use crate::previous_run_diagnostics::{RETURNED_OK, capture_checkpoint};

    fn publisher() -> CheckpointPublisher {
        CheckpointPublisher::new(Arc::new(Shared::new()))
    }
    fn capture(publisher: &CheckpointPublisher) -> Checkpoint {
        capture_checkpoint(&publisher.0, &mut |_| {
            (
                MemorySample::default(),
                publisher.0.checkpoint.stamp(),
                publisher.0.checkpoint.stamp(),
            )
        })
    }
    fn job(publisher: &CheckpointPublisher, id: u64) -> JobLease {
        publisher.begin_job(
            JobMetadata {
                context_id: Some(10 + id),
                items_generation: Some(id),
                job_id: id,
                source: Some(Dimensions {
                    width: id as u32,
                    height: id as u32 + 1,
                }),
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn adapter_registration_survives_busy_mailbox_and_reaches_later_samples() {
        let publisher = publisher();
        assert!(publisher.0.checkpoint.snapshot().adapter.is_none());
        let guard = publisher.0.checkpoint.mailbox.lock().unwrap();
        // Demonstrate real mailbox contention, without releasing it for install.
        publisher.publish_display(DisplaySnapshot::default());
        let installer_publisher = publisher.clone();
        let (installed_tx, installed_rx) = std::sync::mpsc::channel();
        let installer = std::thread::spawn(move || {
            installer_publisher.set_adapter(AdapterProbe::for_test());
            installed_tx.send(()).unwrap();
        });
        let installed = installed_rx.recv_timeout(Duration::from_secs(20));
        drop(guard); // Release even on failure, so a blocking regression can exit.
        installer.join().unwrap();
        assert!(installed.is_ok(), "adapter install waited for the mailbox");

        let sampler = std::thread::spawn(move || {
            let mut sampler = DiagnosticSampler::new();
            for _ in 0..2 {
                let checkpoint = capture_checkpoint(&publisher.0, &mut |adapter| {
                    sampler.sample_timed(adapter, || publisher.0.checkpoint.stamp())
                });
                // This fake Vulkan source needs no GPU/DXGI. It must reach the
                // real sampler, rather than staying NoRenderAdapter forever.
                assert_eq!(
                    checkpoint.gpu.value.status,
                    GpuSampleStatus::UnsupportedBackend
                );
                assert_eq!(checkpoint.gpu.value.adapter.unwrap().vendor_id, 100);
                assert_eq!(checkpoint.dropped_updates, 1);
            }
        });
        sampler.join().unwrap();
    }

    #[test]
    fn fake_clock_periodic_coalesces_and_idle_has_no_deadline() {
        let mut schedule = Schedule::default();
        assert!(!schedule.due(0, 0, 0));
        assert_eq!(schedule.wait(100_000, 0), None);
        assert!(schedule.due(100, 1, 1));
        schedule.saved(100, 1, 1);
        for elapsed in [101, 150, 300, 1099] {
            assert!(!schedule.due(elapsed, 1, 1));
        }
        assert_eq!(schedule.wait(1099, 1), Some(Duration::from_millis(1)));
        assert!(schedule.due(1100, 1, 1));
        schedule.saved(1100, 1, 1);
        assert!(schedule.due(1110, 0, 2));
        schedule.saved(1110, 0, 2);
        assert_eq!(schedule.wait(1_000_000, 0), None);
        assert!(!schedule.due(1_000_000, 0, 2));
        // Entire activity lived during the previous slow save.
        assert!(schedule.due(1_000_000, 0, 4));
    }

    #[test]
    fn overlapping_jobs_coalesce_until_periodic_deadline_then_save_final_idle() {
        let publisher = publisher();
        let baseline = publisher.activity();
        let boundary = publisher.0.checkpoint.boundaries();
        let mut schedule = Schedule::default();
        schedule.saved(0, publisher.0.checkpoint.active(), boundary);
        for elapsed in 1..1000 {
            let lease = job(&publisher, elapsed);
            let result_owner = lease.clone();
            lease.update(JobUpdate {
                stage: JobStage::ResultWaiting,
                ..Default::default()
            });
            drop(lease);
            assert_eq!(publisher.0.checkpoint.active(), 2);
            drop(result_owner);
            assert_eq!(publisher.0.checkpoint.active(), 1);
            assert_eq!(publisher.0.checkpoint.boundaries(), boundary);
            assert!(!schedule.due(elapsed, 1, boundary));
        }
        assert!(schedule.due(1000, 1, boundary));
        schedule.saved(1000, 1, boundary);
        drop(baseline);
        let idle_boundary = publisher.0.checkpoint.boundaries();
        assert_eq!(idle_boundary, boundary + 1);
        assert_eq!(publisher.0.checkpoint.active(), 0);
        assert!(schedule.due(1001, 0, idle_boundary));
        schedule.saved(1001, 0, idle_boundary);
        assert_eq!(schedule.wait(10_000, 0), None);
        assert!(!schedule.due(10_000, 0, idle_boundary));
    }

    #[test]
    fn whole_activity_during_save_preserves_both_global_boundaries() {
        let publisher = publisher();
        let mut schedule = Schedule::default();
        let saved_boundary = publisher.0.checkpoint.boundaries();
        schedule.saved(0, 0, saved_boundary);
        let original = publisher.activity();
        let clone = original.clone();
        let overlapping = publisher.activity();
        drop(original);
        drop(overlapping);
        assert_eq!(publisher.0.checkpoint.active(), 1);
        assert_eq!(publisher.0.checkpoint.boundaries(), saved_boundary + 1);
        drop(clone);
        let current_boundary = publisher.0.checkpoint.boundaries();
        assert_eq!(publisher.0.checkpoint.active(), 0);
        assert_eq!(current_boundary, saved_boundary + 2);
        assert!(schedule.due(250, 0, current_boundary));
        assert_eq!(schedule.wait(250, 0), None);
    }

    #[test]
    fn activity_is_exact_when_publication_is_busy_and_clones_share_a_group() {
        let publisher = publisher();
        let guard = publisher.0.checkpoint.mailbox.lock().unwrap();
        let lease = job(&publisher, 1);
        let result_waiting = lease.clone();
        assert_eq!(publisher.0.checkpoint.active(), 1);
        assert_eq!(publisher.0.checkpoint.jobs.load(Ordering::Acquire), 1);
        drop(lease); // UI pending removed; result/backlog still owns the job.
        assert_eq!(publisher.0.checkpoint.active(), 1);
        drop(guard);
        let snapshot = capture(&publisher);
        assert_eq!(snapshot.active_jobs, 1);
        assert_eq!(snapshot.omitted_jobs, 1);
        assert_eq!(snapshot.dropped_updates, 1);
        result_waiting.update(JobUpdate {
            stage: JobStage::ResultWaiting,
            ..Default::default()
        });
        let waiting = capture(&publisher);
        assert_eq!(waiting.jobs[0].update.stage, JobStage::ResultWaiting);
        drop(result_waiting);
        assert_eq!(publisher.0.checkpoint.active(), 0);
        let ended = capture(&publisher);
        assert_eq!(ended.active_jobs, 0);
        assert_eq!(ended.jobs[0].update.stage, JobStage::Discarded);
        assert!(!ended.jobs[0].active);
    }

    #[test]
    fn cancelled_work_remains_active_and_uses_its_own_observation_stamp() {
        let publisher = publisher();
        let cancel = Arc::new(AtomicBool::new(false));
        let lease = publisher.begin_job(JobMetadata::default(), cancel.clone());
        lease.update(JobUpdate {
            stage: JobStage::TileRun,
            ..Default::default()
        });
        let before = capture(&publisher);
        cancel.store(true, Ordering::Release);
        let cancelled = capture(&publisher);
        assert_eq!(cancelled.active_jobs, 1);
        assert!(cancelled.jobs[0].cancel_requested);
        assert_eq!(cancelled.jobs[0].stamp, before.jobs[0].stamp);
        assert!(
            cancelled.jobs[0].cancel_stamp.run_elapsed_ms >= cancelled.jobs[0].stamp.run_elapsed_ms
        );
        drop(lease);
    }

    #[test]
    fn queue_and_later_stage_counts_do_not_depend_on_metadata_delivery() {
        let publisher = publisher();
        let known = publisher.begin_job(
            JobMetadata {
                source_bytes: Some(128),
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        );
        let unknown = publisher.begin_job(JobMetadata::default(), Arc::new(AtomicBool::new(false)));
        let queued = capture(&publisher);
        assert_eq!(queued.queued_jobs, 2);
        assert_eq!(queued.queued_source_bytes, None);
        let guard = publisher.0.checkpoint.mailbox.lock().unwrap();
        unknown.update(JobUpdate {
            stage: JobStage::ModelLoad,
            ..Default::default()
        });
        drop(guard);
        let running = capture(&publisher);
        assert_eq!(running.queued_jobs, 1);
        assert_eq!(running.queued_source_bytes, Some(128));
        assert_eq!(running.running_jobs, 1);
        unknown.update(JobUpdate {
            stage: JobStage::ResultWaiting,
            ..Default::default()
        });
        assert_eq!(capture(&publisher).result_waiting_jobs, 1);
        unknown.update(JobUpdate {
            stage: JobStage::FinalEffectPending,
            ..Default::default()
        });
        assert_eq!(capture(&publisher).final_effect_jobs, 1);
        unknown.update(JobUpdate {
            stage: JobStage::GpuRegistration,
            ..Default::default()
        });
        assert_eq!(capture(&publisher).gpu_registration_jobs, 1);
        drop(known);
        drop(unknown);
        let ended = capture(&publisher);
        assert_eq!(ended.active_jobs, 0);
        assert_eq!(ended.queued_jobs, 0);
        assert_eq!(ended.queued_source_bytes, Some(0));
        assert_eq!(ended.gpu_registration_jobs, 0);
    }

    #[test]
    fn generic_activity_ends_only_on_last_clone_and_has_no_cache_owner() {
        let publisher = publisher();
        let original = publisher.activity();
        let moved = original.clone();
        drop(original);
        assert_eq!(publisher.0.checkpoint.active(), 1);
        let boundary = publisher.0.checkpoint.boundaries();
        drop(moved);
        assert_eq!(publisher.0.checkpoint.active(), 0);
        assert_eq!(publisher.0.checkpoint.boundaries(), boundary + 1);
        assert_eq!(capture(&publisher).active_jobs, 0);
    }

    #[test]
    fn busy_drop_cannot_leave_activity_or_a_metadata_slot_alive() {
        let publisher = publisher();
        let lease = job(&publisher, 1);
        let guard = publisher.0.checkpoint.mailbox.lock().unwrap();
        drop(lease);
        assert_eq!(publisher.0.checkpoint.active(), 0);
        drop(guard);
        let snapshot = capture(&publisher);
        assert!(!snapshot.jobs[0].active);
        assert_eq!(snapshot.active_jobs, 0);
        // The expired weak owner makes its slot reusable despite dropped metadata.
        let _new_job = job(&publisher, 2);
        assert!(
            capture(&publisher)
                .jobs
                .iter()
                .any(|job| job.metadata.job_id == 2)
        );
    }

    #[test]
    fn job_identity_stage_and_dimensions_are_one_record_and_bounded() {
        let publisher = publisher();
        let leases: Vec<_> = (1..=20).map(|id| job(&publisher, id)).collect();
        for (index, lease) in leases.iter().enumerate() {
            lease.update(JobUpdate {
                stage: JobStage::TileRun,
                output: Some(Dimensions {
                    width: index as u32 + 100,
                    height: 7,
                }),
                ..Default::default()
            });
        }
        let snapshot = capture(&publisher);
        assert_eq!(snapshot.jobs.len(), JOB_SLOTS);
        assert_eq!(snapshot.omitted_jobs, 14);
        assert_eq!(snapshot.active_jobs, 20);
        for job in &snapshot.jobs {
            assert_eq!(job.metadata.items_generation, Some(job.metadata.job_id));
            assert_eq!(
                job.metadata.source.unwrap().width as u64,
                job.metadata.job_id
            );
            assert_eq!(
                job.update.output.unwrap().width as u64,
                job.metadata.job_id + 99
            );
        }
        assert!(
            serde_json::to_vec(&snapshot).unwrap().len()
                < super::super::MAX_RECORD_BYTES as usize - 1024
        );
    }

    #[test]
    fn sampler_and_serialization_are_outside_the_mailbox_and_keep_source_stamps() {
        let publisher = publisher();
        let old = Stamp {
            utc_ms: 1_000,
            run_elapsed_ms: 4,
        };
        publisher.0.checkpoint.mailbox.lock().unwrap().display = Some(Observed {
            stamp: old,
            value: DisplaySnapshot {
                context_id: 2,
                ..Default::default()
            },
        });
        let mut calls = 0;
        let snapshot = capture_checkpoint(&publisher.0, &mut |adapter| {
            calls += 1;
            assert!(adapter.is_none());
            assert!(publisher.0.checkpoint.mailbox.try_lock().is_ok());
            (
                MemorySample {
                    cpu_private_bytes: Some(99),
                    ..Default::default()
                },
                Stamp {
                    utc_ms: 2_000,
                    run_elapsed_ms: 20,
                },
                Stamp {
                    utc_ms: 9_000,
                    run_elapsed_ms: 7_020,
                },
            )
        });
        assert_eq!(calls, 1);
        assert!(publisher.0.checkpoint.mailbox.try_lock().is_ok());
        assert_eq!(snapshot.display.unwrap().stamp, old);
        assert_eq!(snapshot.cpu_private_bytes.value, Some(99));
        assert_eq!(snapshot.cpu_private_bytes.stamp.utc_ms, 2_000);
        assert_eq!(snapshot.gpu.stamp.utc_ms, 9_000);
        assert!(serde_json::to_vec(&snapshot.gpu).is_ok());
        let missing = capture(&publisher);
        assert!(missing.cpu_private_bytes.value.is_none());
        assert!(missing.gpu.value.local.is_none());
    }

    #[test]
    fn terminal_rejects_late_publication_without_renewing_running_state() {
        let publisher = publisher();
        let lease = job(&publisher, 1);
        publisher.0.state.store(RETURNED_OK, Ordering::Release);
        lease.update(JobUpdate {
            stage: JobStage::TileRun,
            ..Default::default()
        });
        publisher.publish_display(DisplaySnapshot {
            context_id: 999,
            ..Default::default()
        });
        let snapshot = capture(&publisher);
        assert_eq!(snapshot.jobs[0].update.stage, JobStage::Queued);
        assert!(snapshot.display.is_none());
        assert_eq!(publisher.0.state.load(Ordering::Acquire), RETURNED_OK);
        drop(lease);
        assert_eq!(publisher.0.checkpoint.active(), 0);
        assert_eq!(publisher.0.state.load(Ordering::Acquire), RETURNED_OK);
    }
}
