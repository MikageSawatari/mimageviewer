//! USER32 owner thread for native video presenter/HUD windows.
//!
//! Render receives only opaque composition targets and value snapshots. HWND creation,
//! mutation, dispatch, and destruction stay on this thread; neither direction blocks for ack.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};

use super::native_cursor::{
    CursorAutoHideReducer, CursorRoutingEvent, CursorRoutingEventKind, CursorRoutingState,
    reduce_cursor_routing_batch,
};
use super::native_window::{
    NativeCursorOwnershipEdge, NativeVideoTouchPhase, NativeVideoWindowEvent,
    NativeVideoWindowEventEnvelope, NativeVideoWindowSource, NativeWindowEventReceiver,
    NativeWindowEventRoute, native_window_event_route, post_typed_pump_quit,
};
use super::native_window_host::{
    NativeHudWindowRequest, NativeRenderTargetTransfer, NativeWindowHost, NativeWindowHostConfig,
    NativeWindowIntent, NativeWindowObservation,
};
use super::window_host_contract::{
    ClosingReason, HostWindowTopology, HostedWindow, WindowBackendOperation, WindowEpoch,
    WindowHostCommand, WindowHostEffect, WindowHostEvent, WindowHostFailure, WindowHostInput,
    WindowHostSpec, WindowHostState, WindowHostTransitionStatus, WindowRequestId, WindowVisibility,
    reduce_window_host,
};
use super::{
    NativeOutputEventSender, NativePresenterVisibility, NativeVideoOutputConfig,
    NativeVideoOutputEvent, NativeVideoPlacement, native_hud_overlay_enabled_for_placement,
    native_window_mode_for_placement, native_window_owner_for_placement,
};

const CONTROL_CAPACITY: usize = 64;
const WINDOW_EVENT_CAPACITY: usize = 256;
const PUMP_TICK: Duration = Duration::from_millis(4);
/// A pump iteration this long starves whatever the render thread is waiting for.
const PUMP_SLOW_ITERATION_MS: f64 = 20.0;
const CHILD_REFLOW_TICK: Duration = Duration::from_millis(16);
const OBSERVATION_TICK: Duration = Duration::from_millis(8);
const HUD_RAISE_RETRY_OFFSETS: [Duration; 3] = [
    Duration::from_millis(0),
    Duration::from_millis(16),
    Duration::from_millis(50),
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct PumpPlacementRequest {
    pub(crate) request: u64,
    pub(crate) epoch: u64,
    pub(crate) placement: NativeVideoPlacement,
    pub(crate) owner_hwnd: u64,
    pub(crate) rect: windows::Win32::Foundation::RECT,
    pub(crate) activate_on_show: bool,
    pub(crate) initially_visible: bool,
}

#[derive(Debug)]
enum PumpCommand {
    Open(PumpPlacementRequest),
    Switch(PumpPlacementRequest),
    TargetReady {
        request: u64,
        epoch: u64,
        topology: HostWindowTopology,
        startup_intents: Vec<NativeWindowIntent>,
        /// When the render thread handed this over. The pump reports the pickup
        /// delay so a busy pump is not mistaken for a slow publish (backlog 1.122).
        issued_at: Instant,
    },
    TargetCommit {
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    },
    TargetRetire {
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    },
    TargetAbort {
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    },
    TargetFailed {
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    },
    Visibility {
        epoch: u64,
        visible: bool,
    },
    Resize {
        epoch: u64,
        placement: NativeVideoPlacement,
        rect: windows::Win32::Foundation::RECT,
    },
    RaisePresenter {
        epoch: u64,
    },
    RaiseHud {
        epoch: u64,
    },
    SetZOrderRecoveryPermit {
        permitted: bool,
    },
    CursorActivity {
        epoch: u64,
    },
    RenderFault {
        request: u64,
        message: String,
    },
    Shutdown {
        request: u64,
    },
}

impl PumpCommand {
    fn request_id(&self) -> Option<u64> {
        match self {
            Self::Open(request) | Self::Switch(request) => Some(request.request),
            Self::TargetReady { request, .. }
            | Self::TargetCommit { request, .. }
            | Self::TargetRetire { request, .. }
            | Self::TargetAbort { request, .. }
            | Self::TargetFailed { request, .. }
            | Self::RenderFault { request, .. }
            | Self::Shutdown { request } => Some(*request),
            Self::Visibility { .. }
            | Self::Resize { .. }
            | Self::RaisePresenter { .. }
            | Self::RaiseHud { .. }
            | Self::SetZOrderRecoveryPermit { .. }
            | Self::CursorActivity { .. } => None,
        }
    }
}

pub(crate) enum PumpLifecycleEvent {
    Attach {
        request: u64,
        epoch: u64,
        targets: NativeRenderTargetTransfer,
        width: u32,
        height: u32,
        pixels_per_point: f32,
        observation: NativeWindowObservation,
        presenter_hwnd: u64,
    },
    Published {
        request: u64,
        epoch: u64,
    },
    Detached {
        epoch: u64,
    },
    VisibilityApplied {
        epoch: u64,
        visible: bool,
    },
    Resized {
        epoch: u64,
        width: u32,
        height: u32,
    },
    Fault {
        request: u64,
        message: String,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeWindowVisualUpdate {
    pub(crate) epoch: u64,
    pub(crate) window_intents: Vec<NativeWindowIntent>,
    pub(crate) hud_regions: Option<Vec<windows::Win32::Foundation::RECT>>,
    pub(crate) toast_active: bool,
    pub(crate) fullscreen_overlay_active: bool,
    pub(crate) debug_description: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PumpObservation {
    pub(crate) epoch: u64,
    pub(crate) value: NativeWindowObservation,
}

#[derive(Default)]
struct LatestPumpValues {
    visual: Mutex<Option<NativeWindowVisualUpdate>>,
    observation: Mutex<Option<PumpObservation>>,
}

pub(crate) struct NativeWindowPumpRenderClient {
    command_tx: Sender<PumpCommand>,
    lifecycle_rx: Receiver<PumpLifecycleEvent>,
    render_events: NativeWindowEventReceiver,
    latest: Arc<LatestPumpValues>,
    channel_fault: Arc<AtomicBool>,
}

impl NativeWindowPumpRenderClient {
    fn send_control(&self, command: PumpCommand) -> Result<(), String> {
        let result = self.command_tx.try_send(command);
        if result.is_err() {
            let message = "native window pump control queue unavailable".to_string();
            self.channel_fault.store(true, Ordering::Release);
            return Err(message);
        }
        Ok(())
    }

    pub(crate) fn open(&self, request: PumpPlacementRequest) -> Result<(), String> {
        self.send_control(PumpCommand::Open(request))
    }

    pub(crate) fn switch(&self, request: PumpPlacementRequest) -> Result<(), String> {
        self.send_control(PumpCommand::Switch(request))
    }

    pub(crate) fn target_ready(
        &self,
        request: u64,
        epoch: u64,
        topology: HostWindowTopology,
        startup_intents: Vec<NativeWindowIntent>,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::TargetReady {
            request,
            epoch,
            topology,
            startup_intents,
            issued_at: Instant::now(),
        })
    }

    pub(crate) fn target_commit(
        &self,
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::TargetCommit {
            request,
            epoch,
            transition_id,
        })
    }

    pub(crate) fn target_retire(
        &self,
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::TargetRetire {
            request,
            epoch,
            transition_id,
        })
    }

    pub(crate) fn target_abort(
        &self,
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::TargetAbort {
            request,
            epoch,
            transition_id,
        })
    }

    pub(crate) fn target_failed(
        &self,
        request: u64,
        epoch: u64,
        transition_id: Option<u64>,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::TargetFailed {
            request,
            epoch,
            transition_id,
        })
    }

    pub(crate) fn set_visibility(&self, epoch: u64, visible: bool) -> Result<(), String> {
        self.send_control(PumpCommand::Visibility { epoch, visible })
    }

    pub(crate) fn resize(
        &self,
        epoch: u64,
        placement: NativeVideoPlacement,
        rect: windows::Win32::Foundation::RECT,
    ) -> Result<(), String> {
        self.send_control(PumpCommand::Resize {
            epoch,
            placement,
            rect,
        })
    }

    pub(crate) fn raise_presenter(&self, epoch: u64) -> Result<(), String> {
        self.send_control(PumpCommand::RaisePresenter { epoch })
    }

    pub(crate) fn raise_hud(&self, epoch: u64) -> Result<(), String> {
        self.send_control(PumpCommand::RaiseHud { epoch })
    }

    pub(crate) fn set_z_order_recovery_permit(&self, permitted: bool) -> Result<(), String> {
        self.send_control(PumpCommand::SetZOrderRecoveryPermit { permitted })
    }

    pub(crate) fn mark_cursor_activity(&self, epoch: u64) -> Result<(), String> {
        self.send_control(PumpCommand::CursorActivity { epoch })
    }

    pub(crate) fn render_fault(&self, request: u64, message: String) {
        let _ = self.send_control(PumpCommand::RenderFault { request, message });
    }

    pub(crate) fn shutdown(&self, request: u64) {
        let _ = self.send_control(PumpCommand::Shutdown { request });
    }

    pub(crate) fn publish_visual(&self, update: NativeWindowVisualUpdate) {
        if let Ok(mut latest) = self.latest.visual.try_lock() {
            *latest = Some(update);
        }
    }

    pub(crate) fn take_observation(&self) -> Option<PumpObservation> {
        self.latest
            .observation
            .try_lock()
            .ok()
            .and_then(|mut latest| latest.take())
    }

    pub(crate) fn drain_window_events(&self) -> Vec<NativeVideoWindowEventEnvelope> {
        self.render_events.drain()
    }

    pub(crate) fn try_recv_lifecycle(&self) -> Result<PumpLifecycleEvent, TryRecvError> {
        self.lifecycle_rx.try_recv()
    }

    pub(crate) fn recv_lifecycle_timeout(
        &self,
        timeout: Duration,
    ) -> Result<PumpLifecycleEvent, crossbeam_channel::RecvTimeoutError> {
        self.lifecycle_rx.recv_timeout(timeout)
    }
}

pub(crate) struct NativeWindowPumpThread {
    pub(crate) render: NativeWindowPumpRenderClient,
    pub(crate) join: std::thread::JoinHandle<()>,
}

pub(crate) struct NativeWindowPumpSpawn {
    pub(crate) config: NativeVideoOutputConfig,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) hwnd_out: Arc<AtomicU64>,
    pub(crate) hud_hwnd_out: Arc<AtomicU64>,
    pub(crate) closed: Arc<AtomicBool>,
    pub(crate) presenter_visibility: NativePresenterVisibility,
    pub(crate) source_epoch: Arc<AtomicU64>,
    pub(crate) ui_event_tx: NativeOutputEventSender,
    pub(crate) init_error: Arc<Mutex<Option<String>>>,
    pub(crate) channel_fault: Arc<AtomicBool>,
    pub(crate) health: Arc<super::native_window_health::NativeWindowHealth>,
}

pub(crate) fn spawn_native_window_pump(
    spawn: NativeWindowPumpSpawn,
) -> Result<NativeWindowPumpThread, String> {
    let (command_tx, command_rx) = crossbeam_channel::bounded(CONTROL_CAPACITY);
    let (lifecycle_tx, lifecycle_rx) = crossbeam_channel::bounded(CONTROL_CAPACITY);
    let latest = Arc::new(LatestPumpValues::default());
    let (pump_route, pump_events) =
        native_window_event_route(WINDOW_EVENT_CAPACITY, Arc::clone(&spawn.channel_fault));
    let (render_route, render_events) =
        native_window_event_route(WINDOW_EVENT_CAPACITY, Arc::clone(&spawn.channel_fault));
    let client = NativeWindowPumpRenderClient {
        command_tx,
        lifecycle_rx,
        render_events,
        latest: Arc::clone(&latest),
        channel_fault: Arc::clone(&spawn.channel_fault),
    };
    let join = std::thread::Builder::new()
        .name("native-video-window-pump".into())
        .spawn(move || {
            let mut runtime = PumpRuntime::new(
                spawn,
                command_rx,
                lifecycle_tx,
                latest,
                pump_route,
                render_route,
                pump_events,
            );
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.run()));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => runtime.quarantine(err),
                Err(_) => runtime.quarantine("native window pump panicked".to_string()),
            }
        })
        .map_err(|err| format!("failed to spawn native video window pump: {err}"))?;
    Ok(NativeWindowPumpThread {
        render: client,
        join,
    })
}

struct PumpRuntime {
    config: NativeVideoOutputConfig,
    cancel: Arc<AtomicBool>,
    hwnd_out: Arc<AtomicU64>,
    hud_hwnd_out: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    presenter_visibility: NativePresenterVisibility,
    source_epoch: Arc<AtomicU64>,
    ui_event_tx: NativeOutputEventSender,
    init_error: Arc<Mutex<Option<String>>>,
    channel_fault: Arc<AtomicBool>,
    health: Arc<super::native_window_health::NativeWindowHealth>,
    command_rx: Receiver<PumpCommand>,
    lifecycle_tx: Sender<PumpLifecycleEvent>,
    latest: Arc<LatestPumpValues>,
    pump_route: NativeWindowEventRoute,
    render_route: NativeWindowEventRoute,
    pump_events: NativeWindowEventReceiver,
    state: WindowHostState,
    hosts: HashMap<WindowEpoch, NativeWindowHost>,
    requests: HashMap<WindowEpoch, PumpPlacementRequest>,
    hud_raise_deadlines: VecDeque<(WindowEpoch, Instant)>,
    z_order_recovery_permitted: bool,
    deferred_visibility: Option<WindowVisibility>,
    parent_sizes: HashMap<WindowEpoch, (u32, u32)>,
    last_child_reflow: Instant,
    last_observation: Instant,
    cursor_routing: CursorRoutingState,
    cursor: CursorAutoHideReducer,
    shutdown_request: u64,
    quitting: bool,
}

impl PumpRuntime {
    fn new(
        spawn: NativeWindowPumpSpawn,
        command_rx: Receiver<PumpCommand>,
        lifecycle_tx: Sender<PumpLifecycleEvent>,
        latest: Arc<LatestPumpValues>,
        pump_route: NativeWindowEventRoute,
        render_route: NativeWindowEventRoute,
        pump_events: NativeWindowEventReceiver,
    ) -> Self {
        spawn
            .health
            .record_pump_thread(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() });
        let cursor = CursorAutoHideReducer::new(spawn.config.cursor_hide_delay_secs);
        Self {
            config: spawn.config,
            cancel: spawn.cancel,
            hwnd_out: spawn.hwnd_out,
            hud_hwnd_out: spawn.hud_hwnd_out,
            closed: spawn.closed,
            presenter_visibility: spawn.presenter_visibility,
            source_epoch: spawn.source_epoch,
            ui_event_tx: spawn.ui_event_tx,
            init_error: spawn.init_error,
            channel_fault: spawn.channel_fault,
            health: spawn.health,
            command_rx,
            lifecycle_tx,
            latest,
            pump_route,
            render_route,
            pump_events,
            state: WindowHostState::Empty,
            hosts: HashMap::new(),
            requests: HashMap::new(),
            hud_raise_deadlines: VecDeque::new(),
            z_order_recovery_permitted: true,
            deferred_visibility: None,
            parent_sizes: HashMap::new(),
            last_child_reflow: Instant::now(),
            last_observation: Instant::now(),
            cursor_routing: CursorRoutingState::Unknown,
            cursor,
            shutdown_request: 0,
            quitting: false,
        }
    }

    fn run(&mut self) -> Result<(), String> {
        while !self.quitting {
            if self.cancel.load(Ordering::Acquire) {
                self.begin_shutdown(self.shutdown_request.saturating_add(1));
            }
            if self.channel_fault.swap(false, Ordering::AcqRel) {
                self.quarantine("native video bounded event route overflow".to_string());
            }
            let iter_t0 = Instant::now();
            self.drain_commands()?;
            let commands_ms = iter_t0.elapsed().as_secs_f64() * 1000.0;
            // A typed Shutdown reducer posts the one final WM_QUIT only after every
            // owned HWND has been destroyed. Do not consume that expected quit as an
            // external fault on the way out of this iteration.
            if self.quitting {
                break;
            }
            let messages_t0 = Instant::now();
            if crate::video::native_window::pump_thread_messages_with_health(&self.health) {
                return Err("native window pump received an unexpected WM_QUIT".to_string());
            }
            let messages_ms = messages_t0.elapsed().as_secs_f64() * 1000.0;
            let events_t0 = Instant::now();
            self.drain_window_events()?;
            let events_ms = events_t0.elapsed().as_secs_f64() * 1000.0;
            if self.quitting {
                break;
            }
            let visual_t0 = Instant::now();
            self.apply_latest_visual();
            let visual_ms = visual_t0.elapsed().as_secs_f64() * 1000.0;
            let periodic_t0 = Instant::now();
            self.run_periodic_work();
            let periodic_ms = periodic_t0.elapsed().as_secs_f64() * 1000.0;
            // A pump iteration is ~4ms of sleep plus a little work. Anything an order
            // of magnitude above that delays every command the render thread is
            // waiting on, so name the phase that ate it (backlog 1.122).
            let iter_ms = iter_t0.elapsed().as_secs_f64() * 1000.0;
            if iter_ms >= PUMP_SLOW_ITERATION_MS {
                crate::logger::log(format!(
                    "[native-video] pump slow iteration total={iter_ms:.1}ms \
                     commands={commands_ms:.1} messages={messages_ms:.1} \
                     window_events={events_ms:.1} visual={visual_ms:.1} periodic={periodic_ms:.1}"
                ));
            }
            if !self.quitting {
                std::thread::sleep(PUMP_TICK);
            }
        }
        Ok(())
    }

    fn drain_commands(&mut self) -> Result<(), String> {
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => {
                    let request = command.request_id();
                    if let Some(request) = request {
                        self.health.record_command_received(request);
                    }
                    let result = self.handle_command(command);
                    if let Some(request) = request {
                        self.health.record_command_completed(request);
                    }
                    result?;
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    self.begin_shutdown(self.shutdown_request.saturating_add(1));
                    return Ok(());
                }
            }
        }
    }

    fn handle_command(&mut self, command: PumpCommand) -> Result<(), String> {
        match command {
            PumpCommand::Open(request) => {
                self.reset_cursor_for_transition(WindowEpoch(request.epoch));
                self.requests.insert(WindowEpoch(request.epoch), request);
                let spec = self.spec_for(request.placement);
                self.dispatch(WindowHostInput::Command(WindowHostCommand::Open {
                    request: WindowRequestId(request.request),
                    epoch: WindowEpoch(request.epoch),
                    spec,
                    visibility: visibility(request.initially_visible),
                }))?;
            }
            PumpCommand::Switch(request) => {
                let reset_epoch = active_epoch(self.state).unwrap_or(WindowEpoch(request.epoch));
                self.reset_cursor_for_transition(reset_epoch);
                self.requests.insert(WindowEpoch(request.epoch), request);
                let spec = self.spec_for(request.placement);
                self.dispatch(WindowHostInput::Command(
                    WindowHostCommand::SwitchPlacement {
                        request: WindowRequestId(request.request),
                        epoch: WindowEpoch(request.epoch),
                        spec,
                    },
                ))?;
            }
            PumpCommand::TargetReady {
                request,
                epoch,
                topology,
                startup_intents,
                issued_at,
            } => self.target_ready(request, epoch, topology, startup_intents, issued_at)?,
            PumpCommand::TargetCommit {
                request,
                epoch,
                transition_id,
            } => {
                self.dispatch_instrumented(
                    WindowHostInput::Event(WindowHostEvent::TargetCommit {
                        request: WindowRequestId(request),
                        epoch: WindowEpoch(epoch),
                    }),
                    transition_id.map(|id| (id, epoch, "Publish")),
                )?;
            }
            PumpCommand::TargetRetire {
                request,
                epoch,
                transition_id,
            } => {
                self.dispatch_instrumented(
                    WindowHostInput::Event(WindowHostEvent::TargetRetire {
                        request: WindowRequestId(request),
                        epoch: WindowEpoch(epoch),
                    }),
                    transition_id.map(|id| (id, epoch, "Destroy")),
                )?;
            }
            PumpCommand::TargetAbort {
                request,
                epoch,
                transition_id,
            } => {
                self.dispatch_instrumented(
                    WindowHostInput::Event(WindowHostEvent::TargetAbort {
                        request: WindowRequestId(request),
                        epoch: WindowEpoch(epoch),
                    }),
                    transition_id.map(|id| (id, epoch, "Destroy")),
                )?;
                self.restore_active_published_handles();
            }
            PumpCommand::TargetFailed {
                request,
                epoch,
                transition_id,
            } => {
                self.dispatch_instrumented(
                    WindowHostInput::Event(WindowHostEvent::TargetFailed {
                        request: WindowRequestId(request),
                        epoch: WindowEpoch(epoch),
                        failure: WindowHostFailure::Backend {
                            operation: WindowBackendOperation::AttachTarget,
                            code: -1,
                        },
                    }),
                    transition_id.map(|id| (id, epoch, "Destroy")),
                )?;
            }
            PumpCommand::Visibility { epoch, visible } => {
                if !visible {
                    self.reset_cursor_for_transition(WindowEpoch(epoch));
                }
                let requested_epoch = WindowEpoch(epoch);
                let requested_visibility = visibility(visible);
                if self.z_order_recovery_permitted {
                    self.dispatch(WindowHostInput::Command(WindowHostCommand::SetVisibility {
                        epoch: requested_epoch,
                        visibility: requested_visibility,
                    }))?;
                } else {
                    self.deferred_visibility = Some(requested_visibility);
                }
            }
            PumpCommand::Resize {
                epoch,
                placement,
                rect,
            } => {
                let key = WindowEpoch(epoch);
                if let Some(window) = self.hosts.get(&key) {
                    let (width, height) = window.resize_to_rect(placement, rect);
                    window.set_hud_geometry(rect.left, rect.top, width, height);
                    self.parent_sizes.insert(key, (width, height));
                    self.send_lifecycle(PumpLifecycleEvent::Resized {
                        epoch,
                        width,
                        height,
                    })?;
                }
            }
            PumpCommand::RaisePresenter { epoch } => {
                if self.z_order_recovery_permitted {
                    self.dispatch(WindowHostInput::Command(WindowHostCommand::Raise {
                        epoch: WindowEpoch(epoch),
                    }))?;
                }
            }
            PumpCommand::RaiseHud { epoch } => self.schedule_hud_raise(WindowEpoch(epoch)),
            PumpCommand::SetZOrderRecoveryPermit { permitted } => {
                self.z_order_recovery_permitted = permitted;
                if !permitted {
                    self.hud_raise_deadlines.clear();
                } else if let Some(requested_visibility) = self.deferred_visibility.take()
                    && let Some(epoch) = active_epoch(self.state)
                {
                    self.dispatch(WindowHostInput::Command(WindowHostCommand::SetVisibility {
                        epoch,
                        visibility: requested_visibility,
                    }))?;
                }
            }
            PumpCommand::CursorActivity { epoch } => {
                let key = WindowEpoch(epoch);
                if cursor_input_epoch(self.state) == Some(key) {
                    let icon = self.cursor.record_external_activity(Instant::now());
                    self.apply_cursor_icon(key, icon);
                    self.record_cursor_health(key);
                }
            }
            PumpCommand::RenderFault { request, message } => {
                self.record_fault(&message);
                let _ = self.send_lifecycle(PumpLifecycleEvent::Fault {
                    request,
                    message: message.clone(),
                });
                self.dispatch(WindowHostInput::Command(WindowHostCommand::Close {
                    request: WindowRequestId(request),
                    reason: ClosingReason::BackendFault,
                }))?;
            }
            PumpCommand::Shutdown { request } => self.begin_shutdown(request),
        }
        Ok(())
    }

    fn spec_for(&self, placement: NativeVideoPlacement) -> WindowHostSpec {
        WindowHostSpec {
            placement,
            topology: if native_hud_overlay_enabled_for_placement(&self.config, placement) {
                HostWindowTopology::PresenterAndHud
            } else {
                HostWindowTopology::PresenterOnly
            },
        }
    }

    fn dispatch(&mut self, input: WindowHostInput) -> Result<(), String> {
        self.dispatch_instrumented(input, None)
    }

    fn dispatch_instrumented(
        &mut self,
        input: WindowHostInput,
        transition_action: Option<(u64, u64, &'static str)>,
    ) -> Result<(), String> {
        let transition = reduce_window_host(self.state, input);
        if let WindowHostTransitionStatus::Rejected(error) = transition.status {
            return Err(format!(
                "native window host contract rejected transition: {error:?}"
            ));
        }
        self.state = transition.state;
        for effect in transition.effects {
            let action_host = transition_action.and_then(|(_, _, action)| match (action, effect) {
                ("Publish", WindowHostEffect::Publish { host, .. }) => Some(host),
                (
                    "Destroy",
                    WindowHostEffect::Destroy { host } | WindowHostEffect::DestroyOrphan { host },
                ) => Some(host),
                _ => None,
            });
            if let Some((transition_id, target_epoch, action)) = transition_action {
                if let Some(host) = action_host {
                    self.log_transition_native_action(
                        transition_id,
                        target_epoch,
                        action,
                        host.epoch.0,
                    );
                }
            }
            let target = transition_action
                .map(|(_, target_epoch, _)| self.presentation_probe_target(target_epoch));
            let action_hwnd = action_host
                .and_then(|host| self.hosts.get(&host.epoch))
                .map(|host| host.hwnd().0 as usize as u64)
                .unwrap_or(0);
            let _probe_scope =
                transition_action
                    .zip(target)
                    .map(|((transition_id, _, _), target)| {
                        crate::presentation_observer::TransitionScope::enter(transition_id, target)
                    });
            self.apply_effect(effect)?;
            if let (Some((transition_id, _, action)), Some(target), Some(_)) =
                (transition_action, target, action_host)
            {
                let probe_action = if action == "Publish" {
                    crate::presentation_observer::WindowAction::Publish
                } else {
                    crate::presentation_observer::WindowAction::Destroy
                };
                crate::presentation_observer::observe_for_transition(
                    transition_id,
                    target,
                    probe_action,
                    crate::presentation_observer::WindowRole::Presenter,
                    action_hwnd,
                    "native_window_pump",
                    format!("epoch={}", action_host.unwrap().epoch.0),
                );
            }
        }
        Ok(())
    }

    fn presentation_probe_target(
        &self,
        target_epoch: u64,
    ) -> crate::presentation_observer::TransitionTarget {
        self.requests
            .get(&WindowEpoch(target_epoch))
            .map(|request| match request.placement {
                NativeVideoPlacement::MainWindowChild => {
                    crate::presentation_observer::TransitionTarget::Main
                }
                NativeVideoPlacement::FullscreenBorderless => {
                    crate::presentation_observer::TransitionTarget::Fullscreen
                }
                NativeVideoPlacement::DetachedViewerChild
                | NativeVideoPlacement::DetachedWindow => {
                    crate::presentation_observer::TransitionTarget::Detached
                }
            })
            .unwrap_or(crate::presentation_observer::TransitionTarget::Unknown)
    }

    fn apply_effect(&mut self, effect: WindowHostEffect) -> Result<(), String> {
        match effect {
            WindowHostEffect::CreateHidden {
                request,
                epoch,
                spec,
            } => self.create_hidden(request, epoch, spec),
            WindowHostEffect::CancelCreate { epoch, .. } => {
                self.requests.remove(&epoch);
                Ok(())
            }
            WindowHostEffect::AttachTarget { host } => self.attach_target(host),
            WindowHostEffect::Publish { host, visibility } => self.publish_host(host, visibility),
            WindowHostEffect::ApplyVisibility { host, visibility } => {
                self.apply_visibility(host, visibility)
            }
            WindowHostEffect::ConfirmVisibility { host, visibility } => {
                self.confirm_visibility(host, visibility)
            }
            WindowHostEffect::Destroy { host } | WindowHostEffect::DestroyOrphan { host } => {
                self.destroy_host(host)
            }
            WindowHostEffect::DetachTarget { lease } => {
                self.send_lifecycle(PumpLifecycleEvent::Detached {
                    epoch: lease.epoch.0,
                })
            }
            WindowHostEffect::Raise { host } => {
                if let Some(window) = self.hosts.get(&host.epoch) {
                    let _ = window.raise_presenter_to_front();
                    crate::presentation_observer::observe(
                        crate::presentation_observer::WindowAction::Raise,
                        crate::presentation_observer::WindowRole::Presenter,
                        window.hwnd().0 as usize as u64,
                        "native_window_pump::Raise",
                        format!("epoch={}", host.epoch.0),
                    );
                }
                Ok(())
            }
            WindowHostEffect::ReportFailure { failure, .. } => {
                crate::logger::log(format!("native window host request failed: {failure:?}"));
                Ok(())
            }
            WindowHostEffect::HostLost { host } => {
                self.remove_lost_host(host);
                self.send_lifecycle(PumpLifecycleEvent::Detached {
                    epoch: host.epoch.0,
                })
            }
            WindowHostEffect::Closed { .. } => {
                self.finish_typed_shutdown();
                Ok(())
            }
        }
    }

    fn create_hidden(
        &mut self,
        request: WindowRequestId,
        epoch: WindowEpoch,
        spec: WindowHostSpec,
    ) -> Result<(), String> {
        let Some(placement) = self.requests.get(&epoch).copied() else {
            return Err(format!("missing placement request for epoch {}", epoch.0));
        };
        let width = (placement.rect.right - placement.rect.left).max(1) as u32;
        let height = (placement.rect.bottom - placement.rect.top).max(1) as u32;
        let presenter_sink = super::native_window::NativeVideoWindowEventSink::new(
            epoch.0,
            epoch.0,
            NativeVideoWindowSource::Presenter,
            self.pump_route.clone(),
            self.render_route.clone(),
        );
        let hud_sink = super::native_window::NativeVideoWindowEventSink::new(
            epoch.0,
            epoch.0,
            NativeVideoWindowSource::Hud,
            self.pump_route.clone(),
            self.render_route.clone(),
        );
        let create_t0 = Instant::now();
        let mut window = NativeWindowHost::create(NativeWindowHostConfig {
            window: super::native_window::NativeVideoWindowConfig {
                mode: native_window_mode_for_placement(placement.placement, placement.rect),
                owner_hwnd: native_window_owner_for_placement(
                    placement.owner_hwnd,
                    placement.placement,
                ),
                initially_visible: false,
                activate_on_show: placement.activate_on_show,
                close_on_escape: false,
                event_sink: Some(presenter_sink),
                generation: epoch.0,
            },
            hud: if spec.topology == HostWindowTopology::PresenterAndHud {
                NativeHudWindowRequest::Enabled { width, height }
            } else {
                NativeHudWindowRequest::Disabled
            },
            event_sink: hud_sink,
        })?;
        let create_ms = create_t0.elapsed().as_secs_f64() * 1000.0;
        window.set_editor_hwnds_snapshot(self.config.editor_hwnds_snapshot.clone());
        window.set_main_hwnd_for_raise_check(self.config.main_hwnd_for_raise);
        let actual_spec = WindowHostSpec {
            placement: spec.placement,
            topology: if window.has_hud() {
                HostWindowTopology::PresenterAndHud
            } else {
                HostWindowTopology::PresenterOnly
            },
        };
        let windows = window.contract_windows();
        self.health.record_window_handles(
            epoch.0,
            window.hwnd().0 as usize as u64,
            window.hud_hwnd(),
            false,
        );
        self.hosts.insert(epoch, window);
        self.parent_sizes.insert(epoch, (width, height));
        let dispatch_t0 = Instant::now();
        let result = self.dispatch(WindowHostInput::Event(WindowHostEvent::WindowCreated {
            request,
            epoch,
            spec: actual_spec,
            windows,
        }));
        crate::logger::log(format!(
            "[native-video] pump create_hidden epoch={} placement={} \
             create_window={create_ms:.1}ms dispatch_with_attach={:.1}",
            epoch.0,
            placement.placement.label(),
            dispatch_t0.elapsed().as_secs_f64() * 1000.0,
        ));
        result
    }

    fn attach_target(&mut self, host: HostedWindow) -> Result<(), String> {
        let Some(window) = self.hosts.get(&host.epoch) else {
            return Err(format!(
                "missing window host for attach epoch {}",
                host.epoch.0
            ));
        };
        let Some(placement) = self.requests.get(&host.epoch) else {
            return Err(format!(
                "missing placement for attach epoch {}",
                host.epoch.0
            ));
        };
        let width = (placement.rect.right - placement.rect.left).max(1) as u32;
        let height = (placement.rect.bottom - placement.rect.top).max(1) as u32;
        self.send_lifecycle(PumpLifecycleEvent::Attach {
            request: host.request.0,
            epoch: host.epoch.0,
            targets: window.render_target_transfer(),
            width,
            height,
            pixels_per_point: window.os_pixels_per_point(),
            observation: window.observe(),
            presenter_hwnd: window.hwnd().0 as usize as u64,
        })
    }

    fn target_ready(
        &mut self,
        request: u64,
        epoch: u64,
        topology: HostWindowTopology,
        startup_intents: Vec<NativeWindowIntent>,
        issued_at: Instant,
    ) -> Result<(), String> {
        let queue_ms = issued_at.elapsed().as_secs_f64() * 1000.0;
        let key = WindowEpoch(epoch);
        let Some(window) = self.hosts.remove(&key) else {
            return Ok(());
        };
        let topology_t0 = Instant::now();
        let window = window.retain_render_topology(topology);
        self.hosts.insert(key, window);
        let topology_ms = topology_t0.elapsed().as_secs_f64() * 1000.0;
        let intents_t0 = Instant::now();
        self.apply_window_intents(key, &startup_intents);
        let intents_ms = intents_t0.elapsed().as_secs_f64() * 1000.0;
        let dispatch_t0 = Instant::now();
        let result = self.dispatch(WindowHostInput::Event(WindowHostEvent::TargetReady {
            request: WindowRequestId(request),
            epoch: key,
        }));
        crate::logger::log(format!(
            "[native-video] pump target_ready request={request} epoch={epoch} \
             queue={queue_ms:.1}ms topology={topology_ms:.1} intents={intents_ms:.1} \
             dispatch_prepare={:.1} intents_len={}",
            dispatch_t0.elapsed().as_secs_f64() * 1000.0,
            startup_intents.len(),
        ));
        result
    }

    fn publish_host(
        &mut self,
        host: HostedWindow,
        visibility: WindowVisibility,
    ) -> Result<(), String> {
        self.reset_cursor_for_transition(host.epoch);
        let Some(window) = self.hosts.get(&host.epoch) else {
            return Err(format!(
                "missing window host for publish epoch {}",
                host.epoch.0
            ));
        };
        let placement = self
            .requests
            .get(&host.epoch)
            .copied()
            .ok_or_else(|| format!("missing placement for publish epoch {}", host.epoch.0))?;
        let show_t0 = Instant::now();
        match visibility {
            WindowVisibility::Visible => {
                let _ = window.show_for_placement(placement.activate_on_show, placement.placement);
                window.set_hud_window_visible(true);
                self.presenter_visibility.publish_hidden(false);
            }
            WindowVisibility::Hidden => {
                let _ = window.hide();
                self.presenter_visibility.publish_hidden(true);
            }
        }
        let show_ms = show_t0.elapsed().as_secs_f64() * 1000.0;
        let rest_t0 = Instant::now();
        self.hwnd_out
            .store(window.hwnd().0 as usize as u64, Ordering::Release);
        self.hud_hwnd_out
            .store(window.hud_hwnd(), Ordering::Release);
        self.health.record_window_published(
            host.epoch.0,
            window.hwnd().0 as usize as u64,
            window.hud_hwnd(),
            placement.placement,
            visibility == WindowVisibility::Visible,
        );
        self.validate_published_window_owner(window.hwnd().0 as usize as u64);
        if window.hud_hwnd() != 0 {
            self.validate_published_window_owner(window.hud_hwnd());
        }
        crate::logger::log(format!(
            "[native-video] pump publish_host epoch={} placement={} visible={} \
             show={show_ms:.1}ms rest={:.1}",
            host.epoch.0,
            placement.placement.label(),
            visibility == WindowVisibility::Visible,
            rest_t0.elapsed().as_secs_f64() * 1000.0,
        ));
        self.send_lifecycle(PumpLifecycleEvent::Published {
            request: host.request.0,
            epoch: host.epoch.0,
        })
    }

    fn apply_visibility(
        &mut self,
        host: HostedWindow,
        visibility: WindowVisibility,
    ) -> Result<(), String> {
        if let Some(window) = self.hosts.get(&host.epoch) {
            let placement = self.requests.get(&host.epoch).copied();
            match visibility {
                WindowVisibility::Visible => {
                    let placement = placement.ok_or_else(|| {
                        format!("missing placement for visibility epoch {}", host.epoch.0)
                    })?;
                    let _ =
                        window.show_for_placement(placement.activate_on_show, placement.placement);
                    window.set_hud_window_visible(true);
                }
                WindowVisibility::Hidden => {
                    let _ = window.hide();
                }
            }
        }
        self.publish_visibility_confirmation(host, visibility)
    }

    fn confirm_visibility(
        &mut self,
        host: HostedWindow,
        visibility: WindowVisibility,
    ) -> Result<(), String> {
        self.publish_visibility_confirmation(host, visibility)
    }

    fn publish_visibility_confirmation(
        &mut self,
        host: HostedWindow,
        visibility: WindowVisibility,
    ) -> Result<(), String> {
        // WindowHostState is authoritative. Re-publish its projection before the
        // acknowledgement so App and render observe the confirmed state even for
        // an idempotent request that performs no native show/hide operation.
        self.presenter_visibility
            .publish_hidden(visibility == WindowVisibility::Hidden);
        self.health
            .record_visibility(host.epoch.0, visibility == WindowVisibility::Visible);
        self.send_lifecycle(PumpLifecycleEvent::VisibilityApplied {
            epoch: host.epoch.0,
            visible: visibility == WindowVisibility::Visible,
        })
    }

    fn destroy_host(&mut self, host: HostedWindow) -> Result<(), String> {
        self.reset_cursor_for_transition(host.epoch);
        self.clear_published_if_matches(host.epoch);
        if let Some(mut window) = self.hosts.remove(&host.epoch) {
            window.destroy();
        }
        self.requests.remove(&host.epoch);
        self.parent_sizes.remove(&host.epoch);
        self.health.clear_window_handles_if_epoch(host.epoch.0);
        let result = self.dispatch(WindowHostInput::Event(WindowHostEvent::WindowDestroyed {
            lease: host.lease(),
        }));
        self.refresh_window_health();
        result
    }

    fn remove_lost_host(&mut self, host: HostedWindow) {
        self.reset_cursor_for_transition(host.epoch);
        self.clear_published_if_matches(host.epoch);
        self.hosts.remove(&host.epoch);
        self.requests.remove(&host.epoch);
        self.parent_sizes.remove(&host.epoch);
        self.health.clear_window_handles_if_epoch(host.epoch.0);
        self.refresh_window_health();
    }

    fn clear_published_if_matches(&self, epoch: WindowEpoch) {
        let Some(window) = self.hosts.get(&epoch) else {
            return;
        };
        let hwnd = window.hwnd().0 as usize as u64;
        let _ = self
            .hwnd_out
            .compare_exchange(hwnd, 0, Ordering::AcqRel, Ordering::Acquire);
        let hud = window.hud_hwnd();
        if hud != 0 {
            let _ = self
                .hud_hwnd_out
                .compare_exchange(hud, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }

    fn refresh_window_health(&self) {
        let (host, visible) = match self.state {
            WindowHostState::Visible { host, .. } => (host, true),
            WindowHostState::Hidden { host, .. } => (host, false),
            _ => return,
        };
        let Some(window) = self.hosts.get(&host.epoch) else {
            return;
        };
        self.health.record_window_handles(
            host.epoch.0,
            window.hwnd().0 as usize as u64,
            window.hud_hwnd(),
            visible,
        );
    }

    fn log_transition_native_action(
        &self,
        request: u64,
        target_epoch: u64,
        effect: &str,
        action_epoch: u64,
    ) {
        let target = self
            .requests
            .get(&WindowEpoch(target_epoch))
            .map(|request| match request.placement {
                NativeVideoPlacement::MainWindowChild => "MainWindow",
                NativeVideoPlacement::FullscreenBorderless => "Fullscreen",
                NativeVideoPlacement::DetachedViewerChild
                | NativeVideoPlacement::DetachedWindow => "DetachedWindow",
            })
            .unwrap_or("Unknown");
        let hwnd = self
            .hosts
            .get(&WindowEpoch(action_epoch))
            .map(|host| host.hwnd().0 as usize as u64)
            .unwrap_or(0);
        crate::logger::log(format!(
            "[presentation-transition] id={request} target={target} effect={effect} hwnd=0x{hwnd:x}"
        ));
    }

    fn restore_active_published_handles(&self) {
        let host = match self.state {
            WindowHostState::Visible { host, .. } | WindowHostState::Hidden { host, .. } => host,
            _ => return,
        };
        let Some(window) = self.hosts.get(&host.epoch) else {
            return;
        };
        self.hwnd_out
            .store(window.hwnd().0 as usize as u64, Ordering::Release);
        self.hud_hwnd_out
            .store(window.hud_hwnd(), Ordering::Release);
        self.refresh_window_health();
    }

    fn validate_published_window_owner(&self, hwnd_raw: u64) {
        let expected = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let actual = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                windows::Win32::Foundation::HWND(hwnd_raw as usize as *mut _),
                None,
            )
        };
        #[cfg(any(debug_assertions, test))]
        assert_eq!(
            actual, expected,
            "published native video HWND must belong to its pump thread"
        );
        if actual != expected {
            self.health
                .record_owner_mismatch(expected, actual, hwnd_raw);
        }
    }

    fn begin_shutdown(&mut self, request: u64) {
        if self.quitting || matches!(self.state, WindowHostState::Closed) {
            return;
        }
        if let Some(epoch) = active_epoch(self.state) {
            self.reset_cursor_for_transition(epoch);
        }
        self.shutdown_request = self.shutdown_request.max(request);
        if let Err(err) = self.dispatch(WindowHostInput::Command(WindowHostCommand::Shutdown {
            request: WindowRequestId(self.shutdown_request),
        })) {
            self.record_fault(&err);
            let hosts: Vec<_> = self.hosts.keys().copied().collect();
            for epoch in hosts {
                if let Some(mut window) = self.hosts.remove(&epoch) {
                    self.clear_published_handles(&window);
                    window.destroy();
                }
            }
            self.finish_typed_shutdown();
        }
    }

    fn finish_typed_shutdown(&mut self) {
        if self.quitting {
            return;
        }
        self.hwnd_out.store(0, Ordering::Release);
        self.hud_hwnd_out.store(0, Ordering::Release);
        self.health.clear_window_handles();
        self.presenter_visibility.publish_hidden(false);
        self.closed.store(true, Ordering::Release);
        let _ = self.send_lifecycle(PumpLifecycleEvent::Shutdown);
        self.quitting = true;
        post_typed_pump_quit();
    }

    fn clear_published_handles(&self, window: &NativeWindowHost) {
        let hwnd = window.hwnd().0 as usize as u64;
        let _ = self
            .hwnd_out
            .compare_exchange(hwnd, 0, Ordering::AcqRel, Ordering::Acquire);
        let hud = window.hud_hwnd();
        if hud != 0 {
            let _ = self
                .hud_hwnd_out
                .compare_exchange(hud, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }

    fn quarantine(&mut self, message: String) {
        self.record_fault(&message);
        if !self.quitting {
            self.begin_shutdown(self.shutdown_request.saturating_add(1));
        }
    }

    fn record_fault(&self, message: &str) {
        let rendered = format!("native video session quarantined: {message}");
        match self.init_error.try_lock() {
            Ok(mut slot) => {
                if slot.is_none() {
                    *slot = Some(rendered);
                }
                self.ui_event_tx.wake_ui();
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                let init_error = Arc::clone(&self.init_error);
                let ui_event_tx = self.ui_event_tx.clone();
                let _ = std::thread::Builder::new()
                    .name("native-video-quarantine-report".to_string())
                    .spawn(move || {
                        if let Ok(mut slot) = init_error.lock()
                            && slot.is_none()
                        {
                            *slot = Some(rendered);
                        }
                        ui_event_tx.wake_ui();
                    });
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {}
        }
    }

    fn send_lifecycle(&self, event: PumpLifecycleEvent) -> Result<(), String> {
        match self.lifecycle_tx.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.channel_fault.store(true, Ordering::Release);
                Err("native window lifecycle queue is full".to_string())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.channel_fault.store(true, Ordering::Release);
                Err("native window lifecycle queue disconnected".to_string())
            }
        }
    }

    fn drain_window_events(&mut self) -> Result<(), String> {
        let active_before_drain = cursor_input_epoch(self.state);
        let mut cursor_events = Vec::new();
        for envelope in self.pump_events.drain() {
            let epoch = WindowEpoch(envelope.epoch);
            if !self.hosts.contains_key(&epoch) {
                continue;
            }
            if let Some(event) = cursor_routing_event_for_epoch(active_before_drain, &envelope) {
                cursor_events.push(event);
            }
            match envelope.event {
                NativeVideoWindowEvent::CloseRequested { .. } => {
                    self.ui_event_tx.send(
                        self.source_epoch.load(Ordering::Acquire),
                        NativeVideoOutputEvent::Window(envelope.event.clone()),
                    );
                    let request = self.next_internal_request();
                    self.dispatch(WindowHostInput::Command(WindowHostCommand::Close {
                        request: WindowRequestId(request),
                        reason: ClosingReason::UserClose,
                    }))?;
                }
                NativeVideoWindowEvent::Destroyed => {
                    if let Some(host) = self.hosted_for_epoch(epoch) {
                        self.dispatch(WindowHostInput::Event(WindowHostEvent::WindowDestroyed {
                            lease: host.lease(),
                        }))?;
                        if matches!(self.state, WindowHostState::Empty) {
                            let request = self.next_internal_request();
                            self.dispatch(WindowHostInput::Command(WindowHostCommand::Close {
                                request: WindowRequestId(request),
                                reason: ClosingReason::HostDestroyed,
                            }))?;
                        }
                    }
                }
                NativeVideoWindowEvent::GeometryChanged { x, y, w, h, .. } => {
                    if let Some(window) = self.hosts.get(&epoch) {
                        window.set_hud_geometry(x, y, w, h);
                    }
                }
                NativeVideoWindowEvent::DpiChanged { suggested_rect, .. } => {
                    if let Some(window) = self.hosts.get(&epoch) {
                        let width = (suggested_rect.right - suggested_rect.left).max(1) as u32;
                        let height = (suggested_rect.bottom - suggested_rect.top).max(1) as u32;
                        window.set_hud_geometry(
                            suggested_rect.left,
                            suggested_rect.top,
                            width,
                            height,
                        );
                    }
                }
                NativeVideoWindowEvent::RequestRaiseHud => self.schedule_hud_raise(epoch),
                NativeVideoWindowEvent::RequestFocusClaim => {
                    if self.z_order_recovery_permitted
                        && let Some(window) = self.hosts.get(&epoch)
                    {
                        window.apply_render_intents(&[NativeWindowIntent::ClaimTextInputFocus]);
                    }
                }
                _ => {}
            }
        }
        if let Some(epoch) = active_before_drain
            && cursor_input_epoch(self.state) == Some(epoch)
            && self.hosts.contains_key(&epoch)
            && !cursor_events.is_empty()
        {
            let local_capture = self
                .hosts
                .get(&epoch)
                .and_then(NativeWindowHost::cursor_capture_source);
            let batch =
                reduce_cursor_routing_batch(self.cursor_routing, &cursor_events, local_capture);
            self.cursor_routing = batch.state;
            let icon = self.cursor.apply_input_batch(
                batch.state.input_ownership(),
                batch.activity,
                Instant::now(),
            );
            self.apply_cursor_icon(epoch, icon);
            self.record_cursor_health(epoch);
        }
        Ok(())
    }

    fn hosted_for_epoch(&self, epoch: WindowEpoch) -> Option<HostedWindow> {
        let request = self.requests.get(&epoch)?;
        let window = self.hosts.get(&epoch)?;
        Some(HostedWindow {
            request: WindowRequestId(request.request),
            epoch,
            spec: WindowHostSpec {
                placement: request.placement,
                topology: if window.has_hud() {
                    HostWindowTopology::PresenterAndHud
                } else {
                    HostWindowTopology::PresenterOnly
                },
            },
            windows: window.contract_windows(),
        })
    }

    fn next_internal_request(&mut self) -> u64 {
        let highest_external = self
            .requests
            .values()
            .map(|request| request.request)
            .max()
            .unwrap_or(0);
        self.shutdown_request = self
            .shutdown_request
            .max(highest_external)
            .saturating_add(1);
        self.shutdown_request
    }

    fn apply_latest_visual(&mut self) {
        let update = self
            .latest
            .visual
            .try_lock()
            .ok()
            .and_then(|mut latest| latest.take());
        let Some(update) = update else {
            return;
        };
        let epoch = WindowEpoch(update.epoch);
        if !self.hosts.contains_key(&epoch) {
            return;
        }
        self.apply_window_intents(epoch, &update.window_intents);
        let Some(window) = self.hosts.get_mut(&epoch) else {
            return;
        };
        if let Some(mut regions) = update.hud_regions {
            if update.fullscreen_overlay_active
                && window.has_hud()
                && !window.foreground_allows_hud_raise(false)
            {
                regions.clear();
            }
            window.apply_hud_regions(&regions, update.toast_active, update.debug_description);
        }
    }

    fn schedule_hud_raise(&mut self, epoch: WindowEpoch) {
        if !self.z_order_recovery_permitted {
            return;
        }
        let now = Instant::now();
        self.hud_raise_deadlines
            .retain(|(queued_epoch, _)| *queued_epoch != epoch);
        for offset in HUD_RAISE_RETRY_OFFSETS {
            self.hud_raise_deadlines.push_back((epoch, now + offset));
        }
    }

    fn run_periodic_work(&mut self) {
        let now = Instant::now();
        if let Some(epoch) = cursor_input_epoch(self.state) {
            let icon = self.cursor.tick(now);
            self.apply_cursor_icon(epoch, icon);
            self.record_cursor_health(epoch);
        }
        while self
            .hud_raise_deadlines
            .front()
            .is_some_and(|(_, deadline)| *deadline <= now)
        {
            if let Some((epoch, _)) = self.hud_raise_deadlines.pop_front()
                && self.z_order_recovery_permitted
                && let Some(window) = self.hosts.get(&epoch)
            {
                let _ = window.try_raise_hud_to_top();
            }
        }

        if now.duration_since(self.last_child_reflow) >= CHILD_REFLOW_TICK {
            self.last_child_reflow = now;
            let epochs: Vec<_> = self.hosts.keys().copied().collect();
            for epoch in epochs {
                let Some(request) = self.requests.get(&epoch).copied() else {
                    continue;
                };
                if !request.placement.is_child_window() {
                    continue;
                }
                let current = self.parent_sizes.get(&epoch).copied().unwrap_or((1, 1));
                if let Some(window) = self.hosts.get(&epoch) {
                    let next = window.reflow_child_to_parent_client(request.owner_hwnd, current);
                    self.parent_sizes.insert(epoch, next);
                }
            }
        }

        if now.duration_since(self.last_observation) >= OBSERVATION_TICK {
            self.last_observation = now;
            if let Some(epoch) = active_epoch(self.state)
                && let Some(value) = self.observe_window(epoch)
                && let Ok(mut latest) = self.latest.observation.try_lock()
            {
                *latest = Some(PumpObservation {
                    epoch: epoch.0,
                    value,
                });
            }
        }
    }

    fn apply_window_intents(&mut self, epoch: WindowEpoch, intents: &[NativeWindowIntent]) {
        for intent in intents {
            if let NativeWindowIntent::SetCursorPolicy {
                icon,
                auto_hide_allowed,
            } = *intent
            {
                let applied_before = self.cursor.applied_icon();
                let resolved =
                    self.cursor
                        .set_render_policy(icon, auto_hide_allowed, Instant::now());
                super::cursor_debug::log(format_args!(
                    "layer=pump event=set_render_policy epoch={} window={:?} icon={icon:?} auto_hide_allowed={auto_hide_allowed} applied_before={applied_before:?} returned={resolved:?} applied_after={:?}",
                    epoch.0,
                    self.cursor_routing.source(),
                    self.cursor.applied_icon(),
                ));
                self.apply_cursor_icon(epoch, resolved);
            }
        }
        if let Some(window) = self.hosts.get(&epoch) {
            window.apply_render_intents(intents);
        }
        self.record_cursor_health(epoch);
    }

    fn observe_window(&self, epoch: WindowEpoch) -> Option<NativeWindowObservation> {
        let mut observation = self.hosts.get(&epoch)?.observe();
        observation.cursor_input_owned = self.cursor.input_owned();
        observation.cursor_hidden = self.cursor.hidden();
        observation.cursor_last_activity = self.cursor.last_activity();
        Some(observation)
    }

    fn reset_cursor_for_transition(&mut self, epoch: WindowEpoch) {
        self.cursor_routing = CursorRoutingState::Unknown;
        let icon = self.cursor.reset_for_transition(Instant::now());
        debug_assert!(icon.is_none());
        self.record_cursor_health(epoch);
    }

    fn apply_cursor_icon(
        &self,
        epoch: WindowEpoch,
        icon: Option<super::native_window_host::NativeCursorIcon>,
    ) {
        let active_epoch = cursor_input_epoch(self.state);
        let input_owned = self.cursor.input_owned();
        let routing_ownership = self.cursor_routing.input_ownership();
        let window_source = self.cursor_routing.source();
        let applied_icon = self.cursor.applied_icon();
        let requested_icon = icon.or(applied_icon);
        if active_epoch != Some(epoch)
            || !input_owned
            || routing_ownership != super::native_cursor::CursorInputOwnership::Owned
        {
            super::cursor_debug::log(format_args!(
                "layer=pump event=apply_cursor_icon epoch={} window={window_source:?} hwnd=0x0 asked={requested_icon:?} reducer_output={icon:?} applied_icon={applied_icon:?} outcome=skipped_gate active_epoch={:?} input_owned={input_owned} routing_ownership={routing_ownership:?}",
                epoch.0,
                active_epoch.map(|active| active.0),
            ));
            return;
        }
        let Some(icon) = icon else {
            let outcome = if applied_icon.is_some() {
                "skipped_applied_icon_match"
            } else {
                "skipped_no_resolved_icon"
            };
            super::cursor_debug::log(format_args!(
                "layer=pump event=apply_cursor_icon epoch={} window={window_source:?} hwnd=0x0 asked={requested_icon:?} reducer_output=None applied_icon={applied_icon:?} outcome={outcome}",
                epoch.0,
            ));
            return;
        };
        let Some(window) = self.hosts.get(&epoch) else {
            super::cursor_debug::log(format_args!(
                "layer=pump event=apply_cursor_icon epoch={} window={window_source:?} hwnd=0x0 asked={requested_icon:?} reducer_output=Some({icon:?}) applied_icon={applied_icon:?} outcome=skipped_no_host",
                epoch.0,
            ));
            return;
        };
        let hwnd = match window_source {
            Some(NativeVideoWindowSource::Presenter) => window.hwnd().0 as usize as u64,
            Some(NativeVideoWindowSource::Hud) => window.hud_hwnd(),
            None => 0,
        };
        let invoked = window.apply_cursor_icon(icon);
        let outcome = if invoked {
            "set_cursor_invoked"
        } else {
            "skipped_load_cursor_failed"
        };
        super::cursor_debug::log(format_args!(
            "layer=pump event=apply_cursor_icon epoch={} window={window_source:?} hwnd=0x{hwnd:016X} asked={requested_icon:?} reducer_output=Some({icon:?}) applied_icon={applied_icon:?} outcome={outcome}",
            epoch.0,
        ));
    }

    fn record_cursor_health(&self, epoch: WindowEpoch) {
        if !self.hosts.contains_key(&epoch) {
            return;
        }
        self.health.record_cursor_state(
            epoch.0,
            self.cursor.hidden(),
            self.cursor.input_owned(),
            self.cursor.last_activity(),
        );
    }
}

fn cursor_routing_event_for_epoch(
    active_epoch: Option<WindowEpoch>,
    envelope: &NativeVideoWindowEventEnvelope,
) -> Option<CursorRoutingEvent> {
    if active_epoch != Some(WindowEpoch(envelope.epoch)) || envelope.generation != envelope.epoch {
        return None;
    }
    let kind = match &envelope.event {
        NativeVideoWindowEvent::MouseMove(mouse) => {
            CursorRoutingEventKind::Move([mouse.x, mouse.y])
        }
        NativeVideoWindowEvent::MouseButton(button) => CursorRoutingEventKind::Explicit {
            position: [button.x, button.y],
            establishes_target: button.down,
        },
        NativeVideoWindowEvent::MouseWheel(wheel) => CursorRoutingEventKind::Explicit {
            position: [wheel.x, wheel.y],
            // WM_MOUSEWHEEL is routed to the focus window, so it is genuine
            // activity but does not by itself prove cursor input ownership.
            establishes_target: false,
        },
        NativeVideoWindowEvent::Touch(touch) => match touch.phase {
            NativeVideoTouchPhase::Start => CursorRoutingEventKind::Explicit {
                position: [touch.x, touch.y],
                establishes_target: true,
            },
            NativeVideoTouchPhase::Move => CursorRoutingEventKind::Move([touch.x, touch.y]),
            NativeVideoTouchPhase::End => CursorRoutingEventKind::Explicit {
                position: [touch.x, touch.y],
                establishes_target: false,
            },
            NativeVideoTouchPhase::Cancel => CursorRoutingEventKind::CaptureLost,
        },
        NativeVideoWindowEvent::CursorOwnership(edge) => match edge {
            NativeCursorOwnershipEdge::Leave => CursorRoutingEventKind::Leave,
            NativeCursorOwnershipEdge::CaptureLost => CursorRoutingEventKind::CaptureLost,
            NativeCursorOwnershipEdge::TrackingFailed => CursorRoutingEventKind::TrackingFailed,
        },
        _ => return None,
    };
    Some(CursorRoutingEvent {
        sequence: envelope.sequence,
        source: envelope.source,
        kind,
    })
}

fn visibility(visible: bool) -> WindowVisibility {
    if visible {
        WindowVisibility::Visible
    } else {
        WindowVisibility::Hidden
    }
}

fn active_epoch(state: WindowHostState) -> Option<WindowEpoch> {
    match state {
        WindowHostState::Visible { host, .. } | WindowHostState::Hidden { host, .. } => {
            Some(host.epoch)
        }
        _ => None,
    }
}

fn cursor_input_epoch(state: WindowHostState) -> Option<WindowEpoch> {
    match state {
        WindowHostState::Visible { host, .. } => Some(host.epoch),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::NativeVideoInitialVisibility;
    use super::*;
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;

    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::w;

    const STALL_TEST: &str = "video::native_window_pump::tests::production_parent_destroy_remains_bounded_during_render_stall";
    const STALL_CHILD_ENV: &str = "MIV_STAGE4_PRODUCTION_STALL_CHILD";

    #[test]
    fn cursor_route_rejects_stale_epoch_and_generation() {
        let mut envelope = NativeVideoWindowEventEnvelope {
            sequence: 1,
            epoch: 8,
            generation: 8,
            source: NativeVideoWindowSource::Presenter,
            event: NativeVideoWindowEvent::MouseMove(
                super::super::native_window::NativeVideoMouseEvent {
                    x: 10,
                    y: 20,
                    shift: false,
                    ctrl: false,
                },
            ),
        };
        // Hidden/preparing/closing states expose no cursor input epoch, so
        // queued pre-transition mouse messages cannot reseed ownership.
        assert!(cursor_routing_event_for_epoch(None, &envelope).is_none());
        assert!(cursor_routing_event_for_epoch(Some(WindowEpoch(7)), &envelope).is_none());
        assert!(cursor_routing_event_for_epoch(Some(WindowEpoch(8)), &envelope).is_some());
        envelope.generation = 7;
        assert!(cursor_routing_event_for_epoch(Some(WindowEpoch(8)), &envelope).is_none());
    }

    #[test]
    fn cursor_ownership_path_has_no_synchronous_hit_testing_or_send_message() {
        let sources = [
            include_str!("native_cursor.rs"),
            include_str!("native_window.rs"),
            include_str!("native_window_host.rs"),
            include_str!("native_window_host/hud_window.rs"),
            include_str!("native_presenter/render_core.rs"),
            include_str!("native_window_pump.rs"),
        ];
        let window_from_point = ["Window", "FromPoint", "("].concat();
        let send_message_w = ["Send", "MessageW", "("].concat();
        let send_message_a = ["Send", "MessageA", "("].concat();
        for source in sources {
            assert!(!source.contains(&window_from_point));
            assert!(!source.contains(&send_message_w));
            assert!(!source.contains(&send_message_a));
        }
    }

    #[test]
    #[ignore = "requires Windows DComp hardware; runs in a watchdog subprocess"]
    fn production_parent_destroy_remains_bounded_during_render_stall() {
        if std::env::var_os(STALL_CHILD_ENV).is_some() {
            run_production_parent_destroy_child();
            return;
        }

        let executable = std::env::current_exe().expect("test executable");
        let mut child = Command::new(executable)
            .args(["--exact", STALL_TEST, "--ignored", "--nocapture"])
            .env(STALL_CHILD_ENV, "1")
            .spawn()
            .expect("spawn Stage 4 production stall subprocess");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().expect("poll stall subprocess") {
                assert!(status.success(), "stall subprocess failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("production parent-destroy stall subprocess exceeded 30s watchdog");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn run_production_parent_destroy_child() {
        let module = unsafe { GetModuleHandleW(None) }.expect("GetModuleHandleW");
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("mIV Stage4 parent"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
        }
        .expect("create disposable parent");

        let cancel = Arc::new(AtomicBool::new(false));
        let hwnd_out = Arc::new(AtomicU64::new(0));
        let hud_out = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let presenter_visibility =
            NativePresenterVisibility::new(NativeVideoInitialVisibility::Visible);
        let source_epoch = Arc::new(AtomicU64::new(0));
        let init_error = Arc::new(Mutex::new(None));
        let channel_fault = Arc::new(AtomicBool::new(false));
        let health = super::super::native_window_health::NativeWindowHealth::new_registered();
        let (ui_tx, _ui_rx) = super::super::native_output_event_bus(
            32,
            Arc::clone(&channel_fault),
            Arc::new(super::super::VideoUiWake::default()),
        );
        let config = NativeVideoOutputConfig {
            rect: windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: 320,
                bottom: 180,
            },
            owner_hwnd: parent.0 as usize as u64,
            fallback_file_name: "stall-test".to_string(),
            sync_interval: 0,
            perf_overlay_visible: false,
            initial_tile_overlay: false,
            vst3_available: false,
            checked: false,
            cursor_hide_delay_secs: 2.0,
            ui_scale: 1.0,
            text_contrast: crate::settings::TextContrast::Standard,
            ui_font: crate::settings::UiFontSettings::default(),
            video_grade: crate::creative_lut::VideoGradeSnapshot::default(),
            scale_filter: crate::settings::VideoScaleFilter::OsDefault,
            downscale_smoothing_percent: 0,
            anime4k_variant: None,
            anime4k_budget: crate::video::anime4k_policy::VideoAnime4kBudgetPreset::default(),
            editor_hwnds_snapshot: None,
            main_hwnd_for_raise: 0,
            hud_overlay_enabled: false,
            placement: NativeVideoPlacement::MainWindowChild,
            activate_on_show: false,
            initial_visibility: NativeVideoInitialVisibility::Visible,
            in_main_window: true,
            audio_only: false,
        };
        let pump = spawn_native_window_pump(NativeWindowPumpSpawn {
            config,
            cancel: Arc::clone(&cancel),
            hwnd_out: Arc::clone(&hwnd_out),
            hud_hwnd_out: hud_out,
            closed: Arc::clone(&closed),
            presenter_visibility,
            source_epoch,
            ui_event_tx: ui_tx,
            init_error: Arc::clone(&init_error),
            channel_fault,
            health: Arc::clone(&health),
        })
        .expect("spawn production window pump");
        let NativeWindowPumpThread {
            render,
            join: pump_join,
        } = pump;
        render
            .open(PumpPlacementRequest {
                request: 1,
                epoch: 1,
                placement: NativeVideoPlacement::MainWindowChild,
                owner_hwnd: parent.0 as usize as u64,
                rect: windows::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: 320,
                    bottom: 180,
                },
                activate_on_show: false,
                initially_visible: true,
            })
            .expect("open production child");

        let (stalled_tx, stalled_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let render_health = Arc::clone(&health);
        let render_join = thread::Builder::new()
            .name("native-video-render-stall-test".into())
            .spawn(move || {
                let _com = super::super::NativeComApartment::init().expect("render COM init");
                let attach = loop {
                    match render
                        .recv_lifecycle_timeout(Duration::from_secs(10))
                        .expect("production attach event")
                    {
                        PumpLifecycleEvent::Attach {
                            request,
                            epoch,
                            targets,
                            width,
                            height,
                            pixels_per_point,
                            observation,
                            ..
                        } if request == 1 && epoch == 1 => {
                            break (
                                targets.into_targets(),
                                width,
                                height,
                                pixels_per_point,
                                observation,
                            );
                        }
                        _ => continue,
                    }
                };
                let (core, topology, intents) =
                    crate::video::native_presenter::NativeRenderCore::new(
                        crate::video::native_presenter::NativeRenderConfig {
                            targets: attach.0,
                            width: attach.1,
                            height: attach.2,
                            os_pixels_per_point: attach.3,
                            initial_observation: attach.4,
                            test_overlay: false,
                            egui_overlay: false,
                            cursor_hide_delay_secs: 2.0,
                            ui_scale: 1.0,
                            text_contrast: crate::settings::TextContrast::Standard,
                            ui_font: crate::settings::UiFontSettings::default(),
                            scale_filter: crate::settings::VideoScaleFilter::OsDefault,
                            downscale_smoothing_percent: 0,
                            anime4k_variant: None,
                            anime4k_budget:
                                crate::video::anime4k_policy::VideoAnime4kBudgetPreset::default(),
                            anime4k_status:
                                crate::video::native_presenter::NativeVideoAnime4kStatus::Waiting,
                            health: render_health,
                            window_epoch: 1,
                        },
                    )
                    .expect("production render attach");
                render
                    .target_ready(1, 1, topology, intents)
                    .expect("target ready");
                loop {
                    if matches!(
                        render
                            .recv_lifecycle_timeout(Duration::from_secs(3))
                            .expect("publish event"),
                        PumpLifecycleEvent::Published {
                            request: 1,
                            epoch: 1
                        }
                    ) {
                        break;
                    }
                }
                stalled_tx.send(()).expect("announce render stall");
                release_rx.recv().expect("release render stall");
                drop(core);
            })
            .expect("spawn stalled production render");

        let stall_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match stalled_rx.try_recv() {
                Ok(()) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("production render disconnected before stall")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let _ = crate::video::native_window::pump_thread_messages();
            assert!(
                Instant::now() < stall_deadline,
                "render did not enter production stall"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let child_hwnd = HWND(hwnd_out.load(Ordering::Acquire) as usize as *mut _);
        assert_ne!(child_hwnd, HWND::default());
        let destroy_started = Instant::now();
        unsafe { DestroyWindow(parent) }.expect("destroy parent during render stall");
        let destroy_elapsed = destroy_started.elapsed();
        assert!(
            destroy_elapsed < Duration::from_secs(2),
            "parent destroy took {destroy_elapsed:?} while render stalled"
        );
        cancel.store(true, Ordering::Release);
        let (pump_done_tx, pump_done_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = pump_done_tx.send(pump_join.join());
        });
        pump_done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("production pump remained responsive")
            .expect("production pump join");
        assert!(closed.load(Ordering::Acquire));
        assert_eq!(
            init_error.lock().expect("pump init error lock").as_deref(),
            None,
            "typed shutdown must not quarantine the session's expected final WM_QUIT"
        );
        assert!(!unsafe { IsWindow(Some(parent)) }.as_bool());
        assert!(!unsafe { IsWindow(Some(child_hwnd)) }.as_bool());

        release_tx.send(()).expect("release stalled render");
        render_join.join().expect("render stall test join");
    }
}
