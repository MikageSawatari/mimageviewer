//! Lock-free health observations for the native video window pump/render split.
//!
//! Pump/render writers only publish scalar latest values through atomics. The weak-reference
//! registry lock is used by the watchdog only; monitored threads never touch it after startup.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, PostThreadMessageW, WM_APP};

use super::NativeVideoPlacement;

pub(crate) const NATIVE_WINDOW_HEALTH_PING: u32 = WM_APP + 0x171;
pub(crate) const NATIVE_WINDOW_STALL_THRESHOLD: Duration = Duration::from_secs(5);
const NATIVE_WINDOW_LOG_RATE_LIMIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeRenderOperation {
    Attach = 1,
    AcquireSync = 2,
    FenceWait = 3,
    Present = 4,
    DCompCommit = 5,
    Detach = 6,
}

impl NativeRenderOperation {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Attach),
            2 => Some(Self::AcquireSync),
            3 => Some(Self::FenceWait),
            4 => Some(Self::Present),
            5 => Some(Self::DCompCommit),
            6 => Some(Self::Detach),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Attach => "Attach",
            Self::AcquireSync => "AcquireSync",
            Self::FenceWait => "FenceWait",
            Self::Present => "Present",
            Self::DCompCommit => "DCompCommit",
            Self::Detach => "Detach",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeWindowHealthClassification {
    Healthy = 0,
    PumpStall = 1,
    RenderStall = 2,
}

impl NativeWindowHealthClassification {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::PumpStall,
            2 => Self::RenderStall,
            _ => Self::Healthy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisibilityHealth {
    Unknown = 0,
    Visible = 1,
    Hidden = 2,
}

impl VisibilityHealth {
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Visible,
            2 => Self::Hidden,
            _ => Self::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Visible => "visible",
            Self::Hidden => "hidden",
        }
    }
}

fn placement_code(placement: NativeVideoPlacement) -> u8 {
    match placement {
        NativeVideoPlacement::MainWindowChild => 1,
        NativeVideoPlacement::FullscreenBorderless => 2,
        NativeVideoPlacement::DetachedViewerChild => 3,
        NativeVideoPlacement::DetachedWindow => 4,
    }
}

fn placement_label(code: u8) -> &'static str {
    match code {
        1 => "main-window-child",
        2 => "fullscreen-borderless",
        3 => "detached-viewer-child",
        4 => "detached-window",
        _ => "unknown",
    }
}

#[derive(Clone, Copy)]
struct ActiveRenderOperation {
    operation: NativeRenderOperation,
    epoch: u64,
    started_stamp: u64,
    sequence: u64,
}

pub(crate) struct NativeRenderOperationGuard<'a> {
    health: &'a NativeWindowHealth,
    operation: NativeRenderOperation,
    epoch: u64,
    sequence: u64,
    previous: Option<ActiveRenderOperation>,
}

impl Drop for NativeRenderOperationGuard<'_> {
    fn drop(&mut self) {
        self.health.complete_render_operation(
            self.operation,
            self.epoch,
            self.sequence,
            self.previous,
        );
    }
}

/// Per-native-output state. Every field written after construction is atomic.
pub(crate) struct NativeWindowHealth {
    started_at: Instant,
    pump_thread_id: AtomicU32,
    presenter_hwnd: AtomicU64,
    hud_hwnd: AtomicU64,
    window_epoch: AtomicU64,
    last_dispatch_sequence: AtomicU64,
    last_dispatch_stamp: AtomicU64,
    last_command_received_request: AtomicU64,
    last_command_received_stamp: AtomicU64,
    last_command_completed_request: AtomicU64,
    last_command_completed_stamp: AtomicU64,
    next_ping_sequence: AtomicU64,
    ping_sent_sequence: AtomicU64,
    ping_sent_generation: AtomicU64,
    ping_sent_stamp: AtomicU64,
    ping_ack_sequence: AtomicU64,
    ping_ack_generation: AtomicU64,
    ping_ack_stamp: AtomicU64,
    render_sequence: AtomicU64,
    render_active_operation: AtomicU8,
    render_active_epoch: AtomicU64,
    render_active_stamp: AtomicU64,
    render_active_sequence: AtomicU64,
    render_last_started_operation: AtomicU8,
    render_last_started_epoch: AtomicU64,
    render_last_started_stamp: AtomicU64,
    render_last_started_sequence: AtomicU64,
    render_last_completed_operation: AtomicU8,
    render_last_completed_epoch: AtomicU64,
    render_last_completed_stamp: AtomicU64,
    render_last_completed_sequence: AtomicU64,
    placement: AtomicU8,
    prior_placement: AtomicU8,
    placement_transition_stamp: AtomicU64,
    source_generation: AtomicU64,
    visibility: AtomicU8,
    cursor_hidden: AtomicBool,
    cursor_input_owned: AtomicBool,
    cursor_last_activity_stamp: AtomicU64,
    cursor_epoch: AtomicU64,
    owner_mismatch: AtomicBool,
    owner_mismatch_expected_thread: AtomicU32,
    owner_mismatch_actual_thread: AtomicU32,
    owner_mismatch_hwnd: AtomicU64,
    owner_mismatch_reported: AtomicBool,
    log_observed_state: AtomicU8,
    log_last_state: AtomicU8,
    log_last_emit_stamp: AtomicU64,
}

impl NativeWindowHealth {
    pub(crate) fn new_registered() -> Arc<Self> {
        let health = Arc::new(Self {
            started_at: Instant::now(),
            pump_thread_id: AtomicU32::new(0),
            presenter_hwnd: AtomicU64::new(0),
            hud_hwnd: AtomicU64::new(0),
            window_epoch: AtomicU64::new(0),
            last_dispatch_sequence: AtomicU64::new(0),
            last_dispatch_stamp: AtomicU64::new(0),
            last_command_received_request: AtomicU64::new(0),
            last_command_received_stamp: AtomicU64::new(0),
            last_command_completed_request: AtomicU64::new(0),
            last_command_completed_stamp: AtomicU64::new(0),
            next_ping_sequence: AtomicU64::new(1),
            ping_sent_sequence: AtomicU64::new(0),
            ping_sent_generation: AtomicU64::new(0),
            ping_sent_stamp: AtomicU64::new(0),
            ping_ack_sequence: AtomicU64::new(0),
            ping_ack_generation: AtomicU64::new(0),
            ping_ack_stamp: AtomicU64::new(0),
            render_sequence: AtomicU64::new(0),
            render_active_operation: AtomicU8::new(0),
            render_active_epoch: AtomicU64::new(0),
            render_active_stamp: AtomicU64::new(0),
            render_active_sequence: AtomicU64::new(0),
            render_last_started_operation: AtomicU8::new(0),
            render_last_started_epoch: AtomicU64::new(0),
            render_last_started_stamp: AtomicU64::new(0),
            render_last_started_sequence: AtomicU64::new(0),
            render_last_completed_operation: AtomicU8::new(0),
            render_last_completed_epoch: AtomicU64::new(0),
            render_last_completed_stamp: AtomicU64::new(0),
            render_last_completed_sequence: AtomicU64::new(0),
            placement: AtomicU8::new(0),
            prior_placement: AtomicU8::new(0),
            placement_transition_stamp: AtomicU64::new(0),
            source_generation: AtomicU64::new(0),
            visibility: AtomicU8::new(VisibilityHealth::Unknown as u8),
            cursor_hidden: AtomicBool::new(false),
            cursor_input_owned: AtomicBool::new(false),
            cursor_last_activity_stamp: AtomicU64::new(0),
            cursor_epoch: AtomicU64::new(0),
            owner_mismatch: AtomicBool::new(false),
            owner_mismatch_expected_thread: AtomicU32::new(0),
            owner_mismatch_actual_thread: AtomicU32::new(0),
            owner_mismatch_hwnd: AtomicU64::new(0),
            owner_mismatch_reported: AtomicBool::new(false),
            log_observed_state: AtomicU8::new(NativeWindowHealthClassification::Healthy as u8),
            log_last_state: AtomicU8::new(NativeWindowHealthClassification::Healthy as u8),
            log_last_emit_stamp: AtomicU64::new(0),
        });
        if let Ok(mut registry) = health_registry().lock() {
            registry.retain(|entry| entry.strong_count() != 0);
            registry.push(Arc::downgrade(&health));
        }
        health
    }

    fn now_stamp(&self) -> u64 {
        let elapsed = self
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX - 1)) as u64;
        elapsed + 1
    }

    fn stamp_for_instant(&self, instant: Option<Instant>) -> u64 {
        instant.map_or(0, |instant| {
            instant
                .checked_duration_since(self.started_at)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX - 1)) as u64
                + 1
        })
    }
}

impl NativeWindowHealth {
    pub(crate) fn record_pump_thread(&self, thread_id: u32) {
        self.pump_thread_id.store(thread_id, Ordering::Release);
    }

    pub(crate) fn record_window_handles(
        &self,
        epoch: u64,
        presenter_hwnd: u64,
        hud_hwnd: u64,
        visible: bool,
    ) {
        self.presenter_hwnd.store(presenter_hwnd, Ordering::Relaxed);
        self.hud_hwnd.store(hud_hwnd, Ordering::Relaxed);
        self.visibility.store(
            if visible {
                VisibilityHealth::Visible as u8
            } else {
                VisibilityHealth::Hidden as u8
            },
            Ordering::Relaxed,
        );
        self.window_epoch.store(epoch, Ordering::Release);
    }

    pub(crate) fn clear_window_handles_if_epoch(&self, epoch: u64) {
        if self.window_epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        self.presenter_hwnd.store(0, Ordering::Relaxed);
        self.hud_hwnd.store(0, Ordering::Relaxed);
        self.visibility
            .store(VisibilityHealth::Unknown as u8, Ordering::Relaxed);
        self.window_epoch.store(0, Ordering::Release);
    }

    pub(crate) fn clear_window_handles(&self) {
        self.presenter_hwnd.store(0, Ordering::Relaxed);
        self.hud_hwnd.store(0, Ordering::Relaxed);
        self.visibility
            .store(VisibilityHealth::Unknown as u8, Ordering::Relaxed);
        self.window_epoch.store(0, Ordering::Release);
    }

    pub(crate) fn record_window_published(
        &self,
        epoch: u64,
        presenter_hwnd: u64,
        hud_hwnd: u64,
        placement: NativeVideoPlacement,
        visible: bool,
    ) {
        self.record_window_handles(epoch, presenter_hwnd, hud_hwnd, visible);
        let next = placement_code(placement);
        let previous = self.placement.swap(next, Ordering::AcqRel);
        if previous != next {
            self.prior_placement.store(previous, Ordering::Relaxed);
            self.placement_transition_stamp
                .store(self.now_stamp(), Ordering::Release);
        }
    }

    pub(crate) fn record_visibility(&self, epoch: u64, visible: bool) {
        if self.window_epoch.load(Ordering::Acquire) == epoch {
            self.visibility.store(
                if visible {
                    VisibilityHealth::Visible as u8
                } else {
                    VisibilityHealth::Hidden as u8
                },
                Ordering::Release,
            );
        }
    }

    pub(crate) fn record_source_generation(&self, generation: u64) {
        self.source_generation.store(generation, Ordering::Release);
    }

    pub(crate) fn record_message_dispatched(&self) {
        self.last_dispatch_sequence.fetch_add(1, Ordering::Relaxed);
        self.last_dispatch_stamp
            .store(self.now_stamp(), Ordering::Release);
    }

    pub(crate) fn record_command_received(&self, request: u64) {
        self.last_command_received_request
            .store(request, Ordering::Relaxed);
        self.last_command_received_stamp
            .store(self.now_stamp(), Ordering::Release);
    }

    pub(crate) fn record_command_completed(&self, request: u64) {
        self.last_command_completed_request
            .store(request, Ordering::Relaxed);
        self.last_command_completed_stamp
            .store(self.now_stamp(), Ordering::Release);
    }

    pub(crate) fn acknowledge_pump_ping(&self, generation: u64, sequence: u64) {
        self.ping_ack_generation
            .store(generation, Ordering::Relaxed);
        self.ping_ack_stamp
            .store(self.now_stamp(), Ordering::Relaxed);
        self.ping_ack_sequence.store(sequence, Ordering::Release);
    }

    pub(crate) fn begin_render_operation(
        &self,
        operation: NativeRenderOperation,
        epoch: u64,
    ) -> NativeRenderOperationGuard<'_> {
        let previous = self.active_render_operation();
        let sequence = self.render_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let active = ActiveRenderOperation {
            operation,
            epoch,
            started_stamp: self.now_stamp(),
            sequence,
        };
        self.render_last_started_epoch
            .store(epoch, Ordering::Relaxed);
        self.render_last_started_stamp
            .store(active.started_stamp, Ordering::Relaxed);
        self.render_last_started_operation
            .store(operation as u8, Ordering::Relaxed);
        self.render_last_started_sequence
            .store(sequence, Ordering::Release);
        self.publish_active_render_operation(active);
        NativeRenderOperationGuard {
            health: self,
            operation,
            epoch,
            sequence,
            previous,
        }
    }

    fn active_render_operation(&self) -> Option<ActiveRenderOperation> {
        let operation =
            NativeRenderOperation::from_code(self.render_active_operation.load(Ordering::Acquire))?;
        Some(ActiveRenderOperation {
            operation,
            epoch: self.render_active_epoch.load(Ordering::Relaxed),
            started_stamp: self.render_active_stamp.load(Ordering::Relaxed),
            sequence: self.render_active_sequence.load(Ordering::Relaxed),
        })
    }

    fn publish_active_render_operation(&self, active: ActiveRenderOperation) {
        self.render_active_epoch
            .store(active.epoch, Ordering::Relaxed);
        self.render_active_stamp
            .store(active.started_stamp, Ordering::Relaxed);
        self.render_active_sequence
            .store(active.sequence, Ordering::Relaxed);
        self.render_active_operation
            .store(active.operation as u8, Ordering::Release);
    }

    fn complete_render_operation(
        &self,
        operation: NativeRenderOperation,
        epoch: u64,
        sequence: u64,
        previous: Option<ActiveRenderOperation>,
    ) {
        self.render_last_completed_epoch
            .store(epoch, Ordering::Relaxed);
        self.render_last_completed_stamp
            .store(self.now_stamp(), Ordering::Relaxed);
        self.render_last_completed_operation
            .store(operation as u8, Ordering::Relaxed);
        self.render_last_completed_sequence
            .store(sequence, Ordering::Release);
        if let Some(previous) = previous {
            self.publish_active_render_operation(previous);
        } else {
            self.render_active_operation.store(0, Ordering::Release);
        }
    }

    pub(crate) fn record_cursor_state(
        &self,
        epoch: u64,
        cursor_hidden: bool,
        cursor_input_owned: bool,
        cursor_last_activity: Option<Instant>,
    ) {
        self.cursor_hidden.store(cursor_hidden, Ordering::Relaxed);
        self.cursor_input_owned
            .store(cursor_input_owned, Ordering::Relaxed);
        self.cursor_last_activity_stamp.store(
            self.stamp_for_instant(cursor_last_activity),
            Ordering::Relaxed,
        );
        self.cursor_epoch.store(epoch, Ordering::Release);
    }

    pub(crate) fn record_owner_mismatch(
        &self,
        expected_thread: u32,
        actual_thread: u32,
        hwnd: u64,
    ) {
        self.owner_mismatch_expected_thread
            .store(expected_thread, Ordering::Relaxed);
        self.owner_mismatch_actual_thread
            .store(actual_thread, Ordering::Relaxed);
        self.owner_mismatch_hwnd.store(hwnd, Ordering::Relaxed);
        self.owner_mismatch.store(true, Ordering::Release);
    }

    fn post_ping_if_needed(&self) {
        let thread_id = self.pump_thread_id.load(Ordering::Acquire);
        let epoch = self.window_epoch.load(Ordering::Acquire);
        let presenter = self.presenter_hwnd.load(Ordering::Acquire);
        let hud = self.hud_hwnd.load(Ordering::Acquire);
        if thread_id == 0 || epoch == 0 || !window_is_alive(presenter, hud) {
            return;
        }

        let sent = self.ping_sent_sequence.load(Ordering::Acquire);
        let ack = self.ping_ack_sequence.load(Ordering::Acquire);
        let sent_generation = self.ping_sent_generation.load(Ordering::Acquire);
        if sent != ack && sent_generation == epoch {
            return;
        }

        let sequence = loop {
            let candidate = self.next_ping_sequence.fetch_add(1, Ordering::Relaxed) as u32;
            if candidate != 0 {
                break u64::from(candidate);
            }
        };
        let posted = unsafe {
            PostThreadMessageW(
                thread_id,
                NATIVE_WINDOW_HEALTH_PING,
                WPARAM(sequence as usize),
                LPARAM(epoch as isize),
            )
        }
        .is_ok();
        if posted {
            self.ping_sent_generation.store(epoch, Ordering::Relaxed);
            self.ping_sent_stamp
                .store(self.now_stamp(), Ordering::Relaxed);
            self.ping_sent_sequence.store(sequence, Ordering::Release);
        }
    }
}

#[derive(Clone, Debug)]
struct HealthSnapshot {
    now_stamp: u64,
    pump_thread_id: u32,
    presenter_hwnd: u64,
    hud_hwnd: u64,
    hwnd_alive: bool,
    window_epoch: u64,
    dispatch_sequence: u64,
    dispatch_age_ms: Option<u64>,
    command_received: u64,
    command_received_age_ms: Option<u64>,
    command_completed: u64,
    command_completed_age_ms: Option<u64>,
    ping_sent: u64,
    ping_ack: u64,
    pump_has_acked: bool,
    ping_outstanding: bool,
    pump_ack_age_ms: Option<u64>,
    active_operation: Option<NativeRenderOperation>,
    active_operation_epoch: u64,
    operation_age_ms: Option<u64>,
    last_started_operation: Option<NativeRenderOperation>,
    last_started_epoch: u64,
    last_started_age_ms: Option<u64>,
    last_started_sequence: u64,
    last_completed_operation: Option<NativeRenderOperation>,
    last_completed_epoch: u64,
    last_completed_age_ms: Option<u64>,
    last_completed_sequence: u64,
    placement: u8,
    prior_placement: u8,
    placement_age_ms: Option<u64>,
    source_generation: u64,
    visibility: VisibilityHealth,
    cursor_hidden: bool,
    cursor_input_owned: bool,
    cursor_last_activity_age_ms: Option<u64>,
    cursor_epoch: u64,
}

impl NativeWindowHealth {
    fn snapshot(&self) -> HealthSnapshot {
        let now_stamp = self.now_stamp();
        let presenter_hwnd = self.presenter_hwnd.load(Ordering::Acquire);
        let hud_hwnd = self.hud_hwnd.load(Ordering::Acquire);
        let ping_sent = self.ping_sent_sequence.load(Ordering::Acquire);
        let ping_sent_generation = self.ping_sent_generation.load(Ordering::Relaxed);
        let ping_ack = self.ping_ack_sequence.load(Ordering::Acquire);
        let ping_ack_generation = self.ping_ack_generation.load(Ordering::Relaxed);
        let ping_outstanding = ping_sent != 0
            && (ping_sent != ping_ack || ping_sent_generation != ping_ack_generation);
        let pump_ack_age_ms = if ping_outstanding {
            age_ms(now_stamp, self.ping_sent_stamp.load(Ordering::Acquire))
        } else {
            age_ms(now_stamp, self.ping_ack_stamp.load(Ordering::Acquire))
        };
        let active_operation =
            NativeRenderOperation::from_code(self.render_active_operation.load(Ordering::Acquire));
        HealthSnapshot {
            now_stamp,
            pump_thread_id: self.pump_thread_id.load(Ordering::Acquire),
            presenter_hwnd,
            hud_hwnd,
            hwnd_alive: window_is_alive(presenter_hwnd, hud_hwnd),
            window_epoch: self.window_epoch.load(Ordering::Acquire),
            dispatch_sequence: self.last_dispatch_sequence.load(Ordering::Acquire),
            dispatch_age_ms: age_ms(now_stamp, self.last_dispatch_stamp.load(Ordering::Acquire)),
            command_received: self.last_command_received_request.load(Ordering::Acquire),
            command_received_age_ms: age_ms(
                now_stamp,
                self.last_command_received_stamp.load(Ordering::Acquire),
            ),
            command_completed: self.last_command_completed_request.load(Ordering::Acquire),
            command_completed_age_ms: age_ms(
                now_stamp,
                self.last_command_completed_stamp.load(Ordering::Acquire),
            ),
            ping_sent,
            ping_ack,
            pump_has_acked: ping_ack != 0,
            ping_outstanding,
            pump_ack_age_ms,
            active_operation,
            active_operation_epoch: self.render_active_epoch.load(Ordering::Relaxed),
            operation_age_ms: active_operation
                .and_then(|_| age_ms(now_stamp, self.render_active_stamp.load(Ordering::Relaxed))),
            last_started_operation: NativeRenderOperation::from_code(
                self.render_last_started_operation.load(Ordering::Acquire),
            ),
            last_started_epoch: self.render_last_started_epoch.load(Ordering::Relaxed),
            last_started_age_ms: age_ms(
                now_stamp,
                self.render_last_started_stamp.load(Ordering::Relaxed),
            ),
            last_started_sequence: self.render_last_started_sequence.load(Ordering::Acquire),
            last_completed_operation: NativeRenderOperation::from_code(
                self.render_last_completed_operation.load(Ordering::Acquire),
            ),
            last_completed_epoch: self.render_last_completed_epoch.load(Ordering::Relaxed),
            last_completed_age_ms: age_ms(
                now_stamp,
                self.render_last_completed_stamp.load(Ordering::Relaxed),
            ),
            last_completed_sequence: self.render_last_completed_sequence.load(Ordering::Acquire),
            placement: self.placement.load(Ordering::Acquire),
            prior_placement: self.prior_placement.load(Ordering::Acquire),
            placement_age_ms: age_ms(
                now_stamp,
                self.placement_transition_stamp.load(Ordering::Acquire),
            ),
            source_generation: self.source_generation.load(Ordering::Acquire),
            visibility: VisibilityHealth::from_code(self.visibility.load(Ordering::Acquire)),
            cursor_hidden: self.cursor_hidden.load(Ordering::Acquire),
            cursor_input_owned: self.cursor_input_owned.load(Ordering::Acquire),
            cursor_last_activity_age_ms: age_ms(
                now_stamp,
                self.cursor_last_activity_stamp.load(Ordering::Acquire),
            ),
            cursor_epoch: self.cursor_epoch.load(Ordering::Acquire),
        }
    }
}

impl NativeWindowHealth {
    fn update_log_gate(
        &self,
        classification: NativeWindowHealthClassification,
        now_stamp: u64,
    ) -> bool {
        let mut gate = NativeHealthLogGate {
            observed: NativeWindowHealthClassification::from_code(
                self.log_observed_state.load(Ordering::Acquire),
            ),
            last_logged: NativeWindowHealthClassification::from_code(
                self.log_last_state.load(Ordering::Acquire),
            ),
            last_emit_ms: decode_stamp(self.log_last_emit_stamp.load(Ordering::Acquire)),
        };
        let emit = gate.observe(
            now_stamp - 1,
            classification,
            NATIVE_WINDOW_LOG_RATE_LIMIT.as_millis() as u64,
        );
        self.log_observed_state
            .store(gate.observed as u8, Ordering::Release);
        self.log_last_state
            .store(gate.last_logged as u8, Ordering::Release);
        self.log_last_emit_stamp.store(
            gate.last_emit_ms.map_or(0, |millis| millis + 1),
            Ordering::Release,
        );
        emit
    }
}

impl NativeWindowHealth {
    fn take_owner_mismatch_message(&self, epoch: u64) -> Option<String> {
        if self.owner_mismatch.load(Ordering::Acquire) == false {
            return None;
        }
        if self
            .owner_mismatch_reported
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        Some(format!(
            "NATIVE VIDEO WINDOW OWNER MISMATCH: expected_pump_thread={} actual_window_thread={} hwnd={} epoch={epoch}",
            self.owner_mismatch_expected_thread.load(Ordering::Acquire),
            self.owner_mismatch_actual_thread.load(Ordering::Acquire),
            self.owner_mismatch_hwnd.load(Ordering::Acquire),
        ))
    }
}

impl NativeWindowHealth {
    fn watchdog_messages(&self) -> Vec<String> {
        self.post_ping_if_needed();
        let snapshot = self.snapshot();
        let mut messages = Vec::new();
        if let Some(message) = self.take_owner_mismatch_message(snapshot.window_epoch) {
            messages.push(message);
        }
        let classification = classify_native_window_health(
            snapshot.hwnd_alive,
            snapshot.pump_has_acked,
            snapshot
                .ping_outstanding
                .then(|| Duration::from_millis(snapshot.pump_ack_age_ms.unwrap_or_default())),
            snapshot.operation_age_ms.map(Duration::from_millis),
            NATIVE_WINDOW_STALL_THRESHOLD,
        );
        if self.update_log_gate(classification, snapshot.now_stamp) {
            let prefix = match classification {
                NativeWindowHealthClassification::PumpStall => "NATIVE VIDEO WINDOW PUMP STALL",
                NativeWindowHealthClassification::RenderStall => "NATIVE VIDEO RENDER STALL",
                NativeWindowHealthClassification::Healthy => return messages,
            };
            messages.push(format_snapshot(prefix, &snapshot));
        }
        messages
    }
}

fn format_snapshot(prefix: &str, snapshot: &HealthSnapshot) -> String {
    format!(
        "{prefix}: pump_thread={} hwnd={} hud_hwnd={} hwnd_alive={} epoch={} \
         ping_seq={} ack_seq={} ack_age_ms={} dispatch_seq={} dispatch_age_ms={} \
         command_received={} command_received_age_ms={} command_completed={} \
         command_completed_age_ms={} render_operation={} render_operation_epoch={} \
         render_operation_age_ms={} render_last_started={} render_last_started_epoch={} \
         render_last_started_age_ms={} render_last_started_seq={} render_last_completed={} \
         render_last_completed_epoch={} render_last_completed_age_ms={} \
         render_last_completed_seq={} placement={} prior_placement={} placement_age_ms={} \
         source_generation={} visibility={} cursor_hidden={} cursor_input_owned={} \
         cursor_last_activity_age_ms={} cursor_epoch={} close_remains_possible={}",
        snapshot.pump_thread_id,
        snapshot.presenter_hwnd,
        snapshot.hud_hwnd,
        snapshot.hwnd_alive,
        snapshot.window_epoch,
        snapshot.ping_sent,
        snapshot.ping_ack,
        optional_age(snapshot.pump_ack_age_ms),
        snapshot.dispatch_sequence,
        optional_age(snapshot.dispatch_age_ms),
        snapshot.command_received,
        optional_age(snapshot.command_received_age_ms),
        snapshot.command_completed,
        optional_age(snapshot.command_completed_age_ms),
        optional_operation(snapshot.active_operation),
        snapshot.active_operation_epoch,
        optional_age(snapshot.operation_age_ms),
        optional_operation(snapshot.last_started_operation),
        snapshot.last_started_epoch,
        optional_age(snapshot.last_started_age_ms),
        snapshot.last_started_sequence,
        optional_operation(snapshot.last_completed_operation),
        snapshot.last_completed_epoch,
        optional_age(snapshot.last_completed_age_ms),
        snapshot.last_completed_sequence,
        placement_label(snapshot.placement),
        placement_label(snapshot.prior_placement),
        optional_age(snapshot.placement_age_ms),
        snapshot.source_generation,
        snapshot.visibility.label(),
        snapshot.cursor_hidden,
        snapshot.cursor_input_owned,
        optional_age(snapshot.cursor_last_activity_age_ms),
        snapshot.cursor_epoch,
        snapshot.pump_has_acked
            && snapshot
                .pump_ack_age_ms
                .is_some_and(|age| age < NATIVE_WINDOW_STALL_THRESHOLD.as_millis() as u64),
    )
}

fn optional_age(age: Option<u64>) -> String {
    age.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

fn optional_operation(operation: Option<NativeRenderOperation>) -> &'static str {
    operation.map_or("none", NativeRenderOperation::label)
}

fn window_is_alive(presenter_hwnd: u64, hud_hwnd: u64) -> bool {
    [presenter_hwnd, hud_hwnd]
        .into_iter()
        .filter(|hwnd| *hwnd != 0)
        .any(|hwnd| unsafe {
            IsWindow(Some(windows::Win32::Foundation::HWND(
                hwnd as usize as *mut _,
            )))
            .as_bool()
        })
}

fn age_ms(now_stamp: u64, then_stamp: u64) -> Option<u64> {
    (then_stamp != 0).then(|| now_stamp.saturating_sub(then_stamp))
}

fn decode_stamp(stamp: u64) -> Option<u64> {
    (stamp != 0).then(|| stamp - 1)
}

pub(crate) fn classify_native_window_health(
    hwnd_alive: bool,
    pump_has_acked: bool,
    outstanding_ping_age: Option<Duration>,
    render_operation_age: Option<Duration>,
    threshold: Duration,
) -> NativeWindowHealthClassification {
    if hwnd_alive == false {
        return NativeWindowHealthClassification::Healthy;
    }
    if outstanding_ping_age.is_some_and(|age| age >= threshold) {
        return NativeWindowHealthClassification::PumpStall;
    }
    if pump_has_acked
        && render_operation_age.is_some_and(|age| age >= threshold)
        && outstanding_ping_age.is_none_or(|age| age < threshold)
    {
        return NativeWindowHealthClassification::RenderStall;
    }
    NativeWindowHealthClassification::Healthy
}

#[derive(Clone, Copy, Debug)]
struct NativeHealthLogGate {
    observed: NativeWindowHealthClassification,
    last_logged: NativeWindowHealthClassification,
    last_emit_ms: Option<u64>,
}

impl NativeHealthLogGate {
    fn observe(
        &mut self,
        now_ms: u64,
        next: NativeWindowHealthClassification,
        rate_limit_ms: u64,
    ) -> bool {
        self.observed = next;
        if next == NativeWindowHealthClassification::Healthy {
            self.last_logged = NativeWindowHealthClassification::Healthy;
            return false;
        }
        if next == self.last_logged {
            return false;
        }
        if self
            .last_emit_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < rate_limit_ms)
        {
            return false;
        }
        self.last_logged = next;
        self.last_emit_ms = Some(now_ms);
        true
    }
}

fn health_registry() -> &'static Mutex<Vec<Weak<NativeWindowHealth>>> {
    static REGISTRY: OnceLock<Mutex<Vec<Weak<NativeWindowHealth>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

fn live_health_entries() -> Vec<Arc<NativeWindowHealth>> {
    let Ok(mut registry) = health_registry().lock() else {
        return Vec::new();
    };
    let mut live = Vec::with_capacity(registry.len());
    registry.retain(|entry| {
        if let Some(health) = entry.upgrade() {
            live.push(health);
            true
        } else {
            false
        }
    });
    live
}

pub(crate) fn poll_native_window_watchdogs() -> Vec<String> {
    live_health_entries()
        .into_iter()
        .flat_map(|health| health.watchdog_messages())
        .collect()
}

pub(crate) fn ui_hang_native_window_context() -> String {
    let entries = live_health_entries();
    if entries.is_empty() {
        return "native_window_health=inactive".to_string();
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, health)| {
            let snapshot = health.snapshot();
            format!(
                "native_window[{index}]:pump_ack_age_ms={},render_operation={},render_operation_age_ms={},epoch={}",
                optional_age(snapshot.pump_ack_age_ms),
                optional_operation(snapshot.active_operation),
                optional_age(snapshot.operation_age_ms),
                snapshot.window_epoch,
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn native_window_health_writer_is_atomic_send_sync_state() {
        assert_send_sync::<NativeWindowHealth>();
        let health = NativeWindowHealth::new_registered();
        health.record_message_dispatched();
        health.record_command_received(7);
        health.record_command_completed(7);
        let operation = health.begin_render_operation(NativeRenderOperation::Present, 3);
        drop(operation);
        assert_eq!(
            health
                .render_last_completed_sequence
                .load(Ordering::Acquire),
            1
        );
    }

    #[test]
    fn watchdog_classification_prioritizes_pump_then_render() {
        let threshold = Duration::from_secs(5);
        assert_eq!(
            classify_native_window_health(
                true,
                true,
                Some(Duration::from_secs(6)),
                Some(Duration::from_secs(8)),
                threshold,
            ),
            NativeWindowHealthClassification::PumpStall
        );
        assert_eq!(
            classify_native_window_health(
                true,
                true,
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(6)),
                threshold,
            ),
            NativeWindowHealthClassification::RenderStall
        );
        assert_eq!(
            classify_native_window_health(
                true,
                false,
                None,
                Some(Duration::from_secs(6)),
                threshold,
            ),
            NativeWindowHealthClassification::Healthy
        );
        assert_eq!(
            classify_native_window_health(
                false,
                true,
                Some(Duration::from_secs(9)),
                Some(Duration::from_secs(9)),
                threshold,
            ),
            NativeWindowHealthClassification::Healthy
        );
    }

    #[test]
    fn health_log_gate_emits_edges_without_same_state_flood() {
        let mut gate = NativeHealthLogGate {
            observed: NativeWindowHealthClassification::Healthy,
            last_logged: NativeWindowHealthClassification::Healthy,
            last_emit_ms: None,
        };
        assert!(gate.observe(1_000, NativeWindowHealthClassification::PumpStall, 10_000));
        assert!(!gate.observe(2_000, NativeWindowHealthClassification::PumpStall, 10_000));
        assert!(!gate.observe(30_000, NativeWindowHealthClassification::PumpStall, 10_000));
        assert!(!gate.observe(31_000, NativeWindowHealthClassification::Healthy, 10_000));
        assert!(gate.observe(
            32_000,
            NativeWindowHealthClassification::RenderStall,
            10_000
        ));
        assert!(!gate.observe(
            33_000,
            NativeWindowHealthClassification::RenderStall,
            10_000
        ));
    }

    #[test]
    fn nested_render_operation_restores_outer_operation() {
        let health = NativeWindowHealth::new_registered();
        let attach = health.begin_render_operation(NativeRenderOperation::Attach, 4);
        let present = health.begin_render_operation(NativeRenderOperation::Present, 4);
        assert_eq!(
            health
                .active_render_operation()
                .map(|active| active.operation),
            Some(NativeRenderOperation::Present)
        );
        drop(present);
        assert_eq!(
            health
                .active_render_operation()
                .map(|active| active.operation),
            Some(NativeRenderOperation::Attach)
        );
        drop(attach);
        assert!(health.active_render_operation().is_none());
    }
}
