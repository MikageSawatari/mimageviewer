//! Native video window host ? pure lifecycle contract?
//!
//! Stage 1 ?? production ? HWND ?????????egui / Win32 / D3D ??
//! ?????? Stage ? request/event ???fake backend test?diagnostics schema
//! ???????????

use std::fmt;

use serde::{Deserialize, Serialize};

use super::NativeVideoPlacement;

pub(crate) const NATIVE_WINDOW_DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub(crate) struct $name(pub(crate) u64);
    };
}

opaque_id!(WindowRequestId);
opaque_id!(WindowEpoch);
opaque_id!(OpaqueWindowId);
opaque_id!(WindowGeneration);
opaque_id!(OpaqueThreadId);
opaque_id!(SourceGeneration);
opaque_id!(ProgressSequence);
opaque_id!(MonotonicMillis);

/// HWND ??????????????? opaque identity?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct OpaqueWindowHandle {
    pub(crate) id: OpaqueWindowId,
    pub(crate) generation: WindowGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowVisibility {
    Visible,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HostWindowTopology {
    PresenterOnly,
    PresenterAndHud,
}

/// presenter ? HUD ? paired ownership?HUD ?????? flag ?????
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "topology", rename_all = "snake_case")]
pub(crate) enum HostWindows {
    PresenterOnly {
        presenter: OpaqueWindowHandle,
    },
    PresenterAndHud {
        presenter: OpaqueWindowHandle,
        hud: OpaqueWindowHandle,
    },
}

impl HostWindows {
    fn topology(self) -> HostWindowTopology {
        match self {
            Self::PresenterOnly { .. } => HostWindowTopology::PresenterOnly,
            Self::PresenterAndHud { .. } => HostWindowTopology::PresenterAndHud,
        }
    }

    fn presenter(self) -> OpaqueWindowHandle {
        match self {
            Self::PresenterOnly { presenter } | Self::PresenterAndHud { presenter, .. } => {
                presenter
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WindowHostSpec {
    pub(crate) placement: NativeVideoPlacement,
    pub(crate) topology: HostWindowTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HostedWindow {
    pub(crate) request: WindowRequestId,
    pub(crate) epoch: WindowEpoch,
    pub(crate) spec: WindowHostSpec,
    pub(crate) windows: HostWindows,
}

impl HostedWindow {
    pub(crate) fn lease(self) -> WindowLease {
        WindowLease {
            request: self.request,
            epoch: self.epoch,
            presenter: self.windows.presenter(),
        }
    }
}

/// render ????? opaque target identity?USER32 mutation capability ??????
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct WindowLease {
    pub(crate) request: WindowRequestId,
    pub(crate) epoch: WindowEpoch,
    pub(crate) presenter: OpaqueWindowHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum StagingWindow {
    Requested {
        epoch: WindowEpoch,
        spec: WindowHostSpec,
        visibility: WindowVisibility,
    },
    Created {
        host: HostedWindow,
        visibility: WindowVisibility,
    },
}

impl StagingWindow {
    fn epoch(self) -> WindowEpoch {
        match self {
            Self::Requested { epoch, .. } => epoch,
            Self::Created { host, .. } => host.epoch,
        }
    }

    fn visibility(self) -> WindowVisibility {
        match self {
            Self::Requested { visibility, .. } | Self::Created { visibility, .. } => visibility,
        }
    }

    fn with_visibility(self, visibility: WindowVisibility) -> Self {
        match self {
            Self::Requested { epoch, spec, .. } => Self::Requested {
                epoch,
                spec,
                visibility,
            },
            Self::Created { host, .. } => Self::Created { host, visibility },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum PriorHost {
    NoPrior,
    Prior {
        host: HostedWindow,
        visibility: WindowVisibility,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ClosingHosts {
    NoHosts,
    One {
        host: HostedWindow,
    },
    Two {
        first: HostedWindow,
        second: HostedWindow,
    },
}

impl ClosingHosts {
    fn from_created(created: &[HostedWindow]) -> Self {
        match created {
            [] => Self::NoHosts,
            [host] => Self::One { host: *host },
            [first, second] => Self::Two {
                first: *first,
                second: *second,
            },
            _ => unreachable!("at most active + staging windows"),
        }
    }

    fn remove(self, lease: WindowLease) -> ClosingRemoval {
        match self {
            Self::NoHosts => ClosingRemoval::Stale(Self::NoHosts),
            Self::One { host } if host.lease() == lease => ClosingRemoval::Complete,
            Self::One { host } => ClosingRemoval::Stale(Self::One { host }),
            Self::Two { first, second } if first.lease() == lease => {
                ClosingRemoval::Remaining(Self::One { host: second })
            }
            Self::Two { first, second } if second.lease() == lease => {
                ClosingRemoval::Remaining(Self::One { host: first })
            }
            Self::Two { first, second } => ClosingRemoval::Stale(Self::Two { first, second }),
        }
    }
}

enum ClosingRemoval {
    Stale(ClosingHosts),
    Remaining(ClosingHosts),
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClosingReason {
    UserClose,
    HostDestroyed,
    SourceEnded,
    Shutdown,
    BackendFault,
}

/// Window host lifecycle ???? state owner?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum WindowHostState {
    Empty,
    Preparing {
        request: WindowRequestId,
        staging: StagingWindow,
        prior: PriorHost,
    },
    Visible {
        request: WindowRequestId,
        host: HostedWindow,
    },
    Hidden {
        request: WindowRequestId,
        host: HostedWindow,
    },
    Switching {
        request: WindowRequestId,
        old: HostedWindow,
        staging: StagingWindow,
        visibility: WindowVisibility,
    },
    Closing {
        request: WindowRequestId,
        hosts: ClosingHosts,
        reason: ClosingReason,
    },
    Closed,
}

impl WindowHostState {
    fn request(self) -> Option<WindowRequestId> {
        match self {
            Self::Preparing { request, .. }
            | Self::Visible { request, .. }
            | Self::Hidden { request, .. }
            | Self::Switching { request, .. }
            | Self::Closing { request, .. } => Some(request),
            Self::Empty | Self::Closed => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum WindowHostCommand {
    Open {
        request: WindowRequestId,
        epoch: WindowEpoch,
        spec: WindowHostSpec,
        visibility: WindowVisibility,
    },
    SwitchPlacement {
        request: WindowRequestId,
        epoch: WindowEpoch,
        spec: WindowHostSpec,
    },
    SetVisibility {
        epoch: WindowEpoch,
        visibility: WindowVisibility,
    },
    Close {
        request: WindowRequestId,
        reason: ClosingReason,
    },
    Raise {
        epoch: WindowEpoch,
    },
    Shutdown {
        request: WindowRequestId,
    },
}

impl WindowHostCommand {
    fn kind(self) -> WindowHostCommandKind {
        match self {
            Self::Open { .. } => WindowHostCommandKind::Open,
            Self::SwitchPlacement { .. } => WindowHostCommandKind::SwitchPlacement,
            Self::SetVisibility { .. } => WindowHostCommandKind::SetVisibility,
            Self::Close { .. } => WindowHostCommandKind::Close,
            Self::Raise { .. } => WindowHostCommandKind::Raise,
            Self::Shutdown { .. } => WindowHostCommandKind::Shutdown,
        }
    }

    fn request(self) -> Option<WindowRequestId> {
        match self {
            Self::Open { request, .. }
            | Self::SwitchPlacement { request, .. }
            | Self::Close { request, .. }
            | Self::Shutdown { request } => Some(request),
            Self::SetVisibility { .. } | Self::Raise { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowHostCommandKind {
    Open,
    SwitchPlacement,
    SetVisibility,
    Close,
    Raise,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowHostStateKind {
    Empty,
    Preparing,
    Visible,
    Hidden,
    Switching,
    Closing,
    Closed,
}

impl From<WindowHostState> for WindowHostStateKind {
    fn from(state: WindowHostState) -> Self {
        match state {
            WindowHostState::Empty => Self::Empty,
            WindowHostState::Preparing { .. } => Self::Preparing,
            WindowHostState::Visible { .. } => Self::Visible,
            WindowHostState::Hidden { .. } => Self::Hidden,
            WindowHostState::Switching { .. } => Self::Switching,
            WindowHostState::Closing { .. } => Self::Closing,
            WindowHostState::Closed => Self::Closed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowBackendOperation {
    CreateWindow,
    PumpMessage,
    ResizeWindow,
    AttachTarget,
    PrimeTarget,
    DetachTarget,
    DestroyWindow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowHostChannel {
    AppToPump,
    PumpToRender,
    RenderToPump,
    PumpToApp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowHostInvariant {
    WindowCreatedHidden,
    TargetReadyBeforePublish,
    OldWindowKeptUntilReplacementReady,
    CloseDoesNotWaitForRender,
    PresenterAndHudShareEpoch,
    StaleEpochCannotPublish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "failure", rename_all = "snake_case")]
pub(crate) enum WindowHostFailure {
    Backend {
        operation: WindowBackendOperation,
        code: i64,
    },
    ChannelDisconnected {
        channel: WindowHostChannel,
    },
    ContractViolation {
        invariant: WindowHostInvariant,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum WindowHostEvent {
    WindowCreated {
        request: WindowRequestId,
        epoch: WindowEpoch,
        spec: WindowHostSpec,
        windows: HostWindows,
    },
    WindowCreateFailed {
        request: WindowRequestId,
        epoch: WindowEpoch,
        failure: WindowHostFailure,
    },
    TargetReady {
        request: WindowRequestId,
        epoch: WindowEpoch,
    },
    TargetFailed {
        request: WindowRequestId,
        epoch: WindowEpoch,
        failure: WindowHostFailure,
    },
    WindowDestroyed {
        lease: WindowLease,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub(crate) enum WindowHostEffect {
    CreateHidden {
        request: WindowRequestId,
        epoch: WindowEpoch,
        spec: WindowHostSpec,
    },
    CancelCreate {
        request: WindowRequestId,
        epoch: WindowEpoch,
    },
    AttachTarget {
        host: HostedWindow,
    },
    Publish {
        host: HostedWindow,
        visibility: WindowVisibility,
    },
    ApplyVisibility {
        host: HostedWindow,
        visibility: WindowVisibility,
    },
    Destroy {
        host: HostedWindow,
    },
    DestroyOrphan {
        host: HostedWindow,
    },
    DetachTarget {
        lease: WindowLease,
    },
    Raise {
        host: HostedWindow,
    },
    ReportFailure {
        request: WindowRequestId,
        epoch: WindowEpoch,
        failure: WindowHostFailure,
    },
    HostLost {
        host: HostedWindow,
    },
    Closed {
        request: WindowRequestId,
        reason: ClosingReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub(crate) enum WindowHostContractError {
    CommandNotAllowed {
        command: WindowHostCommandKind,
        state: WindowHostStateKind,
    },
    NonIncreasingEpoch {
        current: WindowEpoch,
        proposed: WindowEpoch,
    },
    CreatedWindowMismatch {
        expected: WindowHostSpec,
        actual: WindowHostSpec,
    },
    CreatedTopologyMismatch {
        expected: HostWindowTopology,
        actual: HostWindowTopology,
    },
    TargetReadyBeforeWindowCreated {
        request: WindowRequestId,
        epoch: WindowEpoch,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub(crate) enum WindowHostTransitionStatus {
    Applied,
    IgnoredIdempotent,
    IgnoredStale,
    Rejected(WindowHostContractError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowHostTransition {
    pub(crate) state: WindowHostState,
    pub(crate) effects: Vec<WindowHostEffect>,
    pub(crate) status: WindowHostTransitionStatus,
}

impl WindowHostTransition {
    fn new(
        state: WindowHostState,
        effects: Vec<WindowHostEffect>,
        status: WindowHostTransitionStatus,
    ) -> Self {
        Self {
            state,
            effects,
            status,
        }
    }

    fn applied(state: WindowHostState, effects: Vec<WindowHostEffect>) -> Self {
        Self::new(state, effects, WindowHostTransitionStatus::Applied)
    }

    fn ignored(state: WindowHostState, status: WindowHostTransitionStatus) -> Self {
        Self::new(state, Vec::new(), status)
    }

    fn rejected(state: WindowHostState, error: WindowHostContractError) -> Self {
        Self::new(
            state,
            Vec::new(),
            WindowHostTransitionStatus::Rejected(error),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowHostInput {
    Command(WindowHostCommand),
    Event(WindowHostEvent),
}

pub(crate) fn reduce_window_host(
    state: WindowHostState,
    input: WindowHostInput,
) -> WindowHostTransition {
    match input {
        WindowHostInput::Command(command) => reduce_command(state, command),
        WindowHostInput::Event(event) => reduce_event(state, event),
    }
}

fn reduce_command(state: WindowHostState, command: WindowHostCommand) -> WindowHostTransition {
    if let Some(request) = command.request()
        && let Some(current) = state.request()
    {
        if request < current {
            return WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale);
        }
        if request == current {
            return WindowHostTransition::ignored(
                state,
                WindowHostTransitionStatus::IgnoredIdempotent,
            );
        }
    }

    match command {
        WindowHostCommand::Open {
            request,
            epoch,
            spec,
            visibility,
        } => match state {
            WindowHostState::Empty => WindowHostTransition::applied(
                WindowHostState::Preparing {
                    request,
                    staging: StagingWindow::Requested {
                        epoch,
                        spec,
                        visibility,
                    },
                    prior: PriorHost::NoPrior,
                },
                vec![WindowHostEffect::CreateHidden {
                    request,
                    epoch,
                    spec,
                }],
            ),
            _ => command_not_allowed(state, command.kind()),
        },
        WindowHostCommand::SwitchPlacement {
            request,
            epoch,
            spec,
        } => {
            let (old, visibility) = match state {
                WindowHostState::Visible { host, .. } => (host, WindowVisibility::Visible),
                WindowHostState::Hidden { host, .. } => (host, WindowVisibility::Hidden),
                _ => return command_not_allowed(state, command.kind()),
            };
            if epoch <= old.epoch {
                return WindowHostTransition::rejected(
                    state,
                    WindowHostContractError::NonIncreasingEpoch {
                        current: old.epoch,
                        proposed: epoch,
                    },
                );
            }
            WindowHostTransition::applied(
                WindowHostState::Switching {
                    request,
                    old,
                    staging: StagingWindow::Requested {
                        epoch,
                        spec,
                        visibility,
                    },
                    visibility,
                },
                vec![WindowHostEffect::CreateHidden {
                    request,
                    epoch,
                    spec,
                }],
            )
        }
        WindowHostCommand::SetVisibility { epoch, visibility } => {
            set_visibility(state, epoch, visibility, command.kind())
        }
        WindowHostCommand::Close { request, reason } => begin_close(state, request, reason),
        WindowHostCommand::Raise { epoch } => match state {
            WindowHostState::Visible { host, .. } if host.epoch == epoch => {
                WindowHostTransition::applied(state, vec![WindowHostEffect::Raise { host }])
            }
            WindowHostState::Hidden { host, .. } if host.epoch == epoch => {
                WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredIdempotent)
            }
            WindowHostState::Visible { .. } | WindowHostState::Hidden { .. } => {
                WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale)
            }
            _ => command_not_allowed(state, command.kind()),
        },
        WindowHostCommand::Shutdown { request } => {
            begin_close(state, request, ClosingReason::Shutdown)
        }
    }
}

fn command_not_allowed(
    state: WindowHostState,
    command: WindowHostCommandKind,
) -> WindowHostTransition {
    WindowHostTransition::rejected(
        state,
        WindowHostContractError::CommandNotAllowed {
            command,
            state: state.into(),
        },
    )
}

fn set_visibility(
    state: WindowHostState,
    epoch: WindowEpoch,
    visibility: WindowVisibility,
    command: WindowHostCommandKind,
) -> WindowHostTransition {
    match state {
        WindowHostState::Preparing {
            request,
            staging,
            prior,
        } if staging.epoch() == epoch => {
            if staging.visibility() == visibility {
                return WindowHostTransition::ignored(
                    state,
                    WindowHostTransitionStatus::IgnoredIdempotent,
                );
            }
            WindowHostTransition::applied(
                WindowHostState::Preparing {
                    request,
                    staging: staging.with_visibility(visibility),
                    prior,
                },
                Vec::new(),
            )
        }
        WindowHostState::Switching {
            request,
            old,
            staging,
            visibility: old_visibility,
        } if staging.epoch() == epoch => {
            if old_visibility == visibility {
                return WindowHostTransition::ignored(
                    state,
                    WindowHostTransitionStatus::IgnoredIdempotent,
                );
            }
            WindowHostTransition::applied(
                WindowHostState::Switching {
                    request,
                    old,
                    staging: staging.with_visibility(visibility),
                    visibility,
                },
                vec![WindowHostEffect::ApplyVisibility {
                    host: old,
                    visibility,
                }],
            )
        }
        WindowHostState::Visible { request, host } if host.epoch == epoch => {
            if visibility == WindowVisibility::Visible {
                return WindowHostTransition::ignored(
                    state,
                    WindowHostTransitionStatus::IgnoredIdempotent,
                );
            }
            WindowHostTransition::applied(
                WindowHostState::Hidden { request, host },
                vec![WindowHostEffect::ApplyVisibility { host, visibility }],
            )
        }
        WindowHostState::Hidden { request, host } if host.epoch == epoch => {
            if visibility == WindowVisibility::Hidden {
                return WindowHostTransition::ignored(
                    state,
                    WindowHostTransitionStatus::IgnoredIdempotent,
                );
            }
            WindowHostTransition::applied(
                WindowHostState::Visible { request, host },
                vec![WindowHostEffect::ApplyVisibility { host, visibility }],
            )
        }
        WindowHostState::Preparing { .. }
        | WindowHostState::Switching { .. }
        | WindowHostState::Visible { .. }
        | WindowHostState::Hidden { .. } => {
            WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale)
        }
        _ => command_not_allowed(state, command),
    }
}

fn begin_close(
    state: WindowHostState,
    request: WindowRequestId,
    reason: ClosingReason,
) -> WindowHostTransition {
    if matches!(
        state,
        WindowHostState::Closed | WindowHostState::Closing { .. }
    ) {
        return WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredIdempotent);
    }

    let mut created = Vec::with_capacity(2);
    let mut effects = Vec::new();
    match state {
        WindowHostState::Empty => {}
        WindowHostState::Preparing {
            request: staging_request,
            staging,
            prior,
        } => {
            collect_staging_for_close(staging, &mut created, &mut effects, staging_request);
            if let PriorHost::Prior { host, .. } = prior {
                created.push(host);
            }
        }
        WindowHostState::Visible { host, .. } | WindowHostState::Hidden { host, .. } => {
            created.push(host);
        }
        WindowHostState::Switching {
            request: staging_request,
            old,
            staging,
            ..
        } => {
            created.push(old);
            collect_staging_for_close(staging, &mut created, &mut effects, staging_request);
        }
        WindowHostState::Closing { .. } | WindowHostState::Closed => unreachable!(),
    }

    let hosts = ClosingHosts::from_created(&created);
    for host in created {
        // Destroy は render detach/ack より先に発行する。どちらも非同期 effect。
        effects.push(WindowHostEffect::Destroy { host });
        effects.push(WindowHostEffect::DetachTarget {
            lease: host.lease(),
        });
    }
    if matches!(hosts, ClosingHosts::NoHosts) {
        effects.push(WindowHostEffect::Closed { request, reason });
        WindowHostTransition::applied(WindowHostState::Closed, effects)
    } else {
        WindowHostTransition::applied(
            WindowHostState::Closing {
                request,
                hosts,
                reason,
            },
            effects,
        )
    }
}

fn collect_staging_for_close(
    staging: StagingWindow,
    created: &mut Vec<HostedWindow>,
    effects: &mut Vec<WindowHostEffect>,
    close_request: WindowRequestId,
) {
    match staging {
        StagingWindow::Requested { epoch, .. } => {
            effects.push(WindowHostEffect::CancelCreate {
                request: close_request,
                epoch,
            });
        }
        StagingWindow::Created { host, .. } => created.push(host),
    }
}

fn reduce_event(state: WindowHostState, event: WindowHostEvent) -> WindowHostTransition {
    match event {
        WindowHostEvent::WindowCreated {
            request,
            epoch,
            spec,
            windows,
        } => window_created(state, request, epoch, spec, windows),
        WindowHostEvent::WindowCreateFailed {
            request,
            epoch,
            failure,
        } => window_create_failed(state, request, epoch, failure),
        WindowHostEvent::TargetReady { request, epoch } => target_ready(state, request, epoch),
        WindowHostEvent::TargetFailed {
            request,
            epoch,
            failure,
        } => target_failed(state, request, epoch, failure),
        WindowHostEvent::WindowDestroyed { lease } => window_destroyed(state, lease),
    }
}

fn window_created(
    state: WindowHostState,
    request: WindowRequestId,
    epoch: WindowEpoch,
    spec: WindowHostSpec,
    windows: HostWindows,
) -> WindowHostTransition {
    let event_host = HostedWindow {
        request,
        epoch,
        spec,
        windows,
    };
    let (state_request, staging) = match state {
        WindowHostState::Preparing {
            request, staging, ..
        }
        | WindowHostState::Switching {
            request, staging, ..
        } => (request, staging),
        _ => {
            return WindowHostTransition::new(
                state,
                vec![WindowHostEffect::DestroyOrphan { host: event_host }],
                WindowHostTransitionStatus::IgnoredStale,
            );
        }
    };

    match staging {
        StagingWindow::Requested {
            epoch: expected_epoch,
            spec: expected_spec,
            visibility,
        } if state_request == request && expected_epoch == epoch => {
            if expected_spec != spec {
                return WindowHostTransition::new(
                    state,
                    vec![WindowHostEffect::DestroyOrphan { host: event_host }],
                    WindowHostTransitionStatus::Rejected(
                        WindowHostContractError::CreatedWindowMismatch {
                            expected: expected_spec,
                            actual: spec,
                        },
                    ),
                );
            }
            if spec.topology != windows.topology() {
                return WindowHostTransition::new(
                    state,
                    vec![WindowHostEffect::DestroyOrphan { host: event_host }],
                    WindowHostTransitionStatus::Rejected(
                        WindowHostContractError::CreatedTopologyMismatch {
                            expected: spec.topology,
                            actual: windows.topology(),
                        },
                    ),
                );
            }
            let created = StagingWindow::Created {
                host: event_host,
                visibility,
            };
            let next = match state {
                WindowHostState::Preparing { request, prior, .. } => WindowHostState::Preparing {
                    request,
                    staging: created,
                    prior,
                },
                WindowHostState::Switching {
                    request,
                    old,
                    visibility,
                    ..
                } => WindowHostState::Switching {
                    request,
                    old,
                    staging: created,
                    visibility,
                },
                _ => unreachable!(),
            };
            WindowHostTransition::applied(
                next,
                vec![WindowHostEffect::AttachTarget { host: event_host }],
            )
        }
        StagingWindow::Created { host, .. } if host == event_host => {
            WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredIdempotent)
        }
        _ => WindowHostTransition::new(
            state,
            vec![WindowHostEffect::DestroyOrphan { host: event_host }],
            WindowHostTransitionStatus::IgnoredStale,
        ),
    }
}

fn target_ready(
    state: WindowHostState,
    request: WindowRequestId,
    epoch: WindowEpoch,
) -> WindowHostTransition {
    match state {
        WindowHostState::Preparing {
            request: expected_request,
            staging: StagingWindow::Created { host, visibility },
            prior,
        } if expected_request == request && host.epoch == epoch => {
            let mut effects = vec![WindowHostEffect::Publish { host, visibility }];
            if let PriorHost::Prior {
                host: prior_host, ..
            } = prior
            {
                effects.push(WindowHostEffect::Destroy { host: prior_host });
                effects.push(WindowHostEffect::DetachTarget {
                    lease: prior_host.lease(),
                });
            }
            WindowHostTransition::applied(active_state(request, host, visibility), effects)
        }
        WindowHostState::Switching {
            request: expected_request,
            old,
            staging: StagingWindow::Created { host, visibility },
            ..
        } if expected_request == request && host.epoch == epoch => WindowHostTransition::applied(
            active_state(request, host, visibility),
            vec![
                WindowHostEffect::Publish { host, visibility },
                WindowHostEffect::Destroy { host: old },
                WindowHostEffect::DetachTarget { lease: old.lease() },
            ],
        ),
        WindowHostState::Preparing {
            request: expected_request,
            staging: StagingWindow::Requested {
                epoch: expected, ..
            },
            ..
        }
        | WindowHostState::Switching {
            request: expected_request,
            staging: StagingWindow::Requested {
                epoch: expected, ..
            },
            ..
        } if expected_request == request && expected == epoch => WindowHostTransition::rejected(
            state,
            WindowHostContractError::TargetReadyBeforeWindowCreated { request, epoch },
        ),
        _ => WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale),
    }
}

fn window_create_failed(
    state: WindowHostState,
    request: WindowRequestId,
    epoch: WindowEpoch,
    failure: WindowHostFailure,
) -> WindowHostTransition {
    match state {
        WindowHostState::Preparing {
            request: expected,
            staging: StagingWindow::Requested { epoch: current, .. },
            prior,
        } if expected == request && current == epoch => {
            recover_preparing_failure(request, epoch, prior, failure, Vec::new())
        }
        WindowHostState::Switching {
            request: expected,
            old,
            staging: StagingWindow::Requested { epoch: current, .. },
            visibility,
        } if expected == request && current == epoch => WindowHostTransition::applied(
            active_state(old.request, old, visibility),
            vec![WindowHostEffect::ReportFailure {
                request,
                epoch,
                failure,
            }],
        ),
        _ => WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale),
    }
}

fn target_failed(
    state: WindowHostState,
    request: WindowRequestId,
    epoch: WindowEpoch,
    failure: WindowHostFailure,
) -> WindowHostTransition {
    match state {
        WindowHostState::Preparing {
            request: expected,
            staging,
            prior,
        } if expected == request && staging.epoch() == epoch => {
            let mut cleanup = Vec::new();
            if let StagingWindow::Created { host, .. } = staging {
                cleanup.push(WindowHostEffect::Destroy { host });
                cleanup.push(WindowHostEffect::DetachTarget {
                    lease: host.lease(),
                });
            }
            recover_preparing_failure(request, epoch, prior, failure, cleanup)
        }
        WindowHostState::Switching {
            request: expected,
            old,
            staging,
            visibility,
        } if expected == request && staging.epoch() == epoch => {
            let mut effects = Vec::new();
            if let StagingWindow::Created { host, .. } = staging {
                effects.push(WindowHostEffect::Destroy { host });
                effects.push(WindowHostEffect::DetachTarget {
                    lease: host.lease(),
                });
            }
            effects.push(WindowHostEffect::ReportFailure {
                request,
                epoch,
                failure,
            });
            WindowHostTransition::applied(active_state(old.request, old, visibility), effects)
        }
        _ => WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale),
    }
}

fn recover_preparing_failure(
    request: WindowRequestId,
    epoch: WindowEpoch,
    prior: PriorHost,
    failure: WindowHostFailure,
    mut effects: Vec<WindowHostEffect>,
) -> WindowHostTransition {
    effects.push(WindowHostEffect::ReportFailure {
        request,
        epoch,
        failure,
    });
    let state = match prior {
        PriorHost::NoPrior => WindowHostState::Empty,
        PriorHost::Prior { host, visibility } => active_state(host.request, host, visibility),
    };
    WindowHostTransition::applied(state, effects)
}

fn window_destroyed(state: WindowHostState, lease: WindowLease) -> WindowHostTransition {
    match state {
        WindowHostState::Closing {
            request,
            hosts,
            reason,
        } => match hosts.remove(lease) {
            ClosingRemoval::Stale(hosts) => WindowHostTransition::ignored(
                WindowHostState::Closing {
                    request,
                    hosts,
                    reason,
                },
                WindowHostTransitionStatus::IgnoredStale,
            ),
            ClosingRemoval::Remaining(hosts) => WindowHostTransition::applied(
                WindowHostState::Closing {
                    request,
                    hosts,
                    reason,
                },
                Vec::new(),
            ),
            ClosingRemoval::Complete => WindowHostTransition::applied(
                WindowHostState::Closed,
                vec![WindowHostEffect::Closed { request, reason }],
            ),
        },
        WindowHostState::Visible { host, .. } | WindowHostState::Hidden { host, .. }
            if host.lease() == lease =>
        {
            WindowHostTransition::applied(
                WindowHostState::Empty,
                vec![WindowHostEffect::HostLost { host }],
            )
        }
        WindowHostState::Preparing {
            staging: StagingWindow::Created { host, .. },
            prior,
            ..
        } if host.lease() == lease => {
            let next = match prior {
                PriorHost::NoPrior => WindowHostState::Empty,
                PriorHost::Prior { host, visibility } => {
                    active_state(host.request, host, visibility)
                }
            };
            WindowHostTransition::applied(next, vec![WindowHostEffect::HostLost { host }])
        }
        WindowHostState::Preparing {
            request,
            staging,
            prior: PriorHost::Prior { host, .. },
        } if host.lease() == lease => WindowHostTransition::applied(
            WindowHostState::Preparing {
                request,
                staging,
                prior: PriorHost::NoPrior,
            },
            vec![WindowHostEffect::HostLost { host }],
        ),
        WindowHostState::Switching {
            old,
            staging: StagingWindow::Created { host, .. },
            visibility,
            ..
        } if host.lease() == lease => WindowHostTransition::applied(
            active_state(old.request, old, visibility),
            vec![WindowHostEffect::HostLost { host }],
        ),
        WindowHostState::Switching {
            request,
            old,
            staging,
            ..
        } if old.lease() == lease => WindowHostTransition::applied(
            WindowHostState::Preparing {
                request,
                staging,
                prior: PriorHost::NoPrior,
            },
            vec![WindowHostEffect::HostLost { host: old }],
        ),
        _ => WindowHostTransition::ignored(state, WindowHostTransitionStatus::IgnoredStale),
    }
}

fn active_state(
    request: WindowRequestId,
    host: HostedWindow,
    visibility: WindowVisibility,
) -> WindowHostState {
    match visibility {
        WindowVisibility::Visible => WindowHostState::Visible { request, host },
        WindowVisibility::Hidden => WindowHostState::Hidden { request, host },
    }
}

/// Stage 4 以降の health log が同じ schema で pump/render を相関できるようにする。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeWindowDiagnostics {
    pub(crate) schema_version: u32,
    pub(crate) sampled_at: MonotonicMillis,
    pub(crate) source_generation: SourceGeneration,
    pub(crate) host: WindowHostState,
    pub(crate) pump: PumpDiagnostics,
    pub(crate) render: RenderDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum DiagnosticThread {
    NotStarted,
    Running { thread_id: OpaqueThreadId },
    Stopped { thread_id: OpaqueThreadId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ProgressStamp<T> {
    NotObserved,
    Observed { value: T, at: MonotonicMillis },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PumpCommandProgress {
    pub(crate) request: WindowRequestId,
    pub(crate) epoch: WindowEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PumpDiagnostics {
    pub(crate) thread: DiagnosticThread,
    pub(crate) message_dispatch: ProgressStamp<ProgressSequence>,
    pub(crate) command_received: ProgressStamp<PumpCommandProgress>,
    pub(crate) command_completed: ProgressStamp<PumpCommandProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenderOperation {
    Attach,
    AcquireSync,
    FenceWait,
    Present,
    DcompCommit,
    Detach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RenderOperationProgress {
    pub(crate) operation: RenderOperation,
    pub(crate) request: WindowRequestId,
    pub(crate) epoch: WindowEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RenderDiagnostics {
    pub(crate) thread: DiagnosticThread,
    pub(crate) operation_started: ProgressStamp<RenderOperationProgress>,
    pub(crate) operation_completed: ProgressStamp<RenderOperationProgress>,
}

/// Stage 3 Windows harness で各待機に同じ timeout 語彙を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowsHarnessPhase {
    PumpThreadStart,
    WindowCreate,
    TargetAttach,
    FirstPresent,
    RenderStall,
    PumpPing,
    WindowResize,
    WindowClose,
    ParentDestroy,
    TargetDetach,
    RenderThreadStop,
    ThreadJoin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WindowsHarnessTimeout {
    pub(crate) phase: WindowsHarnessPhase,
    pub(crate) limit_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowsHarnessInvariant {
    PumpMustProgressWhileRenderStalled,
    CloseMustNotWaitForRender,
    ParentDestroyMustComplete,
    WindowOwnerMustMatchPumpThread,
    TestProcessMustExitWithinDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub(crate) enum WindowsHarnessError {
    Timeout {
        timeout: WindowsHarnessTimeout,
    },
    Backend {
        operation: WindowBackendOperation,
        code: i64,
    },
    ChannelDisconnected {
        channel: WindowHostChannel,
    },
    InvariantViolation {
        invariant: WindowsHarnessInvariant,
    },
    UnexpectedEvent {
        expected: WindowsHarnessPhase,
        actual: WindowsHarnessPhase,
    },
}

impl fmt::Display for WindowsHarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { timeout } => write!(
                f,
                "timeout during {:?} after {} ms",
                timeout.phase, timeout.limit_millis
            ),
            Self::Backend { operation, code } => {
                write!(f, "backend {:?} failed with code {}", operation, code)
            }
            Self::ChannelDisconnected { channel } => {
                write!(f, "channel {:?} disconnected", channel)
            }
            Self::InvariantViolation { invariant } => {
                write!(f, "harness invariant {:?} was violated", invariant)
            }
            Self::UnexpectedEvent { expected, actual } => {
                write!(f, "expected {:?}, observed {:?}", expected, actual)
            }
        }
    }
}

impl std::error::Error for WindowsHarnessError {}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeRenderMode {
        ReadyImmediately,
        Stalled,
    }

    struct FakeWindowBackend {
        render_mode: FakeRenderMode,
        next_window_id: u64,
        next_generation: u64,
        events: VecDeque<WindowHostEvent>,
        effects: Vec<WindowHostEffect>,
    }

    impl FakeWindowBackend {
        fn new(render_mode: FakeRenderMode) -> Self {
            Self {
                render_mode,
                next_window_id: 100,
                next_generation: 1,
                events: VecDeque::new(),
                effects: Vec::new(),
            }
        }

        fn dispatch(&mut self, state: WindowHostState, input: WindowHostInput) -> WindowHostState {
            let transition = reduce_window_host(state, input);
            self.apply_effects(&transition.effects);
            transition.state
        }

        fn dispatch_and_drain(
            &mut self,
            state: WindowHostState,
            input: WindowHostInput,
        ) -> WindowHostState {
            let mut state = self.dispatch(state, input);
            while let Some(event) = self.events.pop_front() {
                state = self.dispatch(state, WindowHostInput::Event(event));
            }
            state
        }

        fn apply_effects(&mut self, effects: &[WindowHostEffect]) {
            for effect in effects {
                self.effects.push(*effect);
                match *effect {
                    WindowHostEffect::CreateHidden {
                        request,
                        epoch,
                        spec,
                    } => {
                        let presenter = self.new_window_handle();
                        let windows = match spec.topology {
                            HostWindowTopology::PresenterOnly => {
                                HostWindows::PresenterOnly { presenter }
                            }
                            HostWindowTopology::PresenterAndHud => HostWindows::PresenterAndHud {
                                presenter,
                                hud: self.new_window_handle(),
                            },
                        };
                        self.events.push_back(WindowHostEvent::WindowCreated {
                            request,
                            epoch,
                            spec,
                            windows,
                        });
                    }
                    WindowHostEffect::AttachTarget { host }
                        if self.render_mode == FakeRenderMode::ReadyImmediately =>
                    {
                        self.events.push_back(WindowHostEvent::TargetReady {
                            request: host.request,
                            epoch: host.epoch,
                        });
                    }
                    WindowHostEffect::Destroy { host }
                    | WindowHostEffect::DestroyOrphan { host } => {
                        self.events.push_back(WindowHostEvent::WindowDestroyed {
                            lease: host.lease(),
                        });
                    }
                    _ => {}
                }
            }
        }

        fn new_window_handle(&mut self) -> OpaqueWindowHandle {
            let handle = OpaqueWindowHandle {
                id: OpaqueWindowId(self.next_window_id),
                generation: WindowGeneration(self.next_generation),
            };
            self.next_window_id += 1;
            self.next_generation += 1;
            handle
        }
    }

    fn spec(placement: NativeVideoPlacement, topology: HostWindowTopology) -> WindowHostSpec {
        WindowHostSpec {
            placement,
            topology,
        }
    }

    fn presenter(
        request: u64,
        epoch: u64,
        raw_id: u64,
        generation: u64,
        placement: NativeVideoPlacement,
    ) -> HostedWindow {
        HostedWindow {
            request: WindowRequestId(request),
            epoch: WindowEpoch(epoch),
            spec: spec(placement, HostWindowTopology::PresenterOnly),
            windows: HostWindows::PresenterOnly {
                presenter: OpaqueWindowHandle {
                    id: OpaqueWindowId(raw_id),
                    generation: WindowGeneration(generation),
                },
            },
        }
    }

    fn open_visible(request: u64, epoch: u64) -> WindowHostCommand {
        WindowHostCommand::Open {
            request: WindowRequestId(request),
            epoch: WindowEpoch(epoch),
            spec: spec(
                NativeVideoPlacement::FullscreenBorderless,
                HostWindowTopology::PresenterAndHud,
            ),
            visibility: WindowVisibility::Visible,
        }
    }

    #[test]
    fn fake_backend_does_not_publish_before_target_ready() {
        let mut backend = FakeWindowBackend::new(FakeRenderMode::Stalled);
        let state = backend.dispatch_and_drain(
            WindowHostState::Empty,
            WindowHostInput::Command(open_visible(1, 10)),
        );

        assert!(matches!(
            state,
            WindowHostState::Preparing {
                staging: StagingWindow::Created { .. },
                ..
            }
        ));
        assert!(
            !backend
                .effects
                .iter()
                .any(|effect| matches!(effect, WindowHostEffect::Publish { .. }))
        );

        let state = backend.dispatch_and_drain(
            state,
            WindowHostInput::Event(WindowHostEvent::TargetReady {
                request: WindowRequestId(1),
                epoch: WindowEpoch(10),
            }),
        );
        assert!(matches!(state, WindowHostState::Visible { .. }));
        assert_eq!(
            backend
                .effects
                .iter()
                .filter(|effect| matches!(effect, WindowHostEffect::Publish { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn fake_backend_switch_stall_keeps_old_window_and_close_needs_no_render_ack() {
        let mut backend = FakeWindowBackend::new(FakeRenderMode::ReadyImmediately);
        let state = backend.dispatch_and_drain(
            WindowHostState::Empty,
            WindowHostInput::Command(open_visible(1, 10)),
        );
        let old = match state {
            WindowHostState::Visible { host, .. } => host,
            other => panic!("expected visible host, got {other:?}"),
        };

        backend.render_mode = FakeRenderMode::Stalled;
        let effect_start = backend.effects.len();
        let state = backend.dispatch_and_drain(
            state,
            WindowHostInput::Command(WindowHostCommand::SwitchPlacement {
                request: WindowRequestId(2),
                epoch: WindowEpoch(11),
                spec: spec(
                    NativeVideoPlacement::DetachedViewerChild,
                    HostWindowTopology::PresenterOnly,
                ),
            }),
        );
        assert!(matches!(
            state,
            WindowHostState::Switching {
                old: current_old,
                staging: StagingWindow::Created { .. },
                ..
            } if current_old == old
        ));
        assert!(
            !backend.effects[effect_start..]
                .iter()
                .any(|effect| matches!(effect, WindowHostEffect::Destroy { host } if *host == old))
        );

        let state = backend.dispatch_and_drain(
            state,
            WindowHostInput::Command(WindowHostCommand::Close {
                request: WindowRequestId(3),
                reason: ClosingReason::UserClose,
            }),
        );
        assert_eq!(state, WindowHostState::Closed);
        assert!(backend.effects.iter().any(|effect| matches!(
            effect,
            WindowHostEffect::Closed {
                request: WindowRequestId(3),
                reason: ClosingReason::UserClose
            }
        )));
    }

    #[test]
    fn stale_epoch_property_rejects_ready_even_when_raw_hwnd_is_reused() {
        let old = presenter(1, 10, 0x1234, 40, NativeVideoPlacement::MainWindowChild);
        let switch = reduce_window_host(
            WindowHostState::Visible {
                request: WindowRequestId(1),
                host: old,
            },
            WindowHostInput::Command(WindowHostCommand::SwitchPlacement {
                request: WindowRequestId(2),
                epoch: WindowEpoch(11),
                spec: spec(
                    NativeVideoPlacement::DetachedViewerChild,
                    HostWindowTopology::PresenterOnly,
                ),
            }),
        );
        let created = reduce_window_host(
            switch.state,
            WindowHostInput::Event(WindowHostEvent::WindowCreated {
                request: WindowRequestId(2),
                epoch: WindowEpoch(11),
                spec: spec(
                    NativeVideoPlacement::DetachedViewerChild,
                    HostWindowTopology::PresenterOnly,
                ),
                windows: HostWindows::PresenterOnly {
                    presenter: OpaqueWindowHandle {
                        id: OpaqueWindowId(0x1234),
                        generation: WindowGeneration(41),
                    },
                },
            }),
        );

        let stale = reduce_window_host(
            created.state,
            WindowHostInput::Event(WindowHostEvent::TargetReady {
                request: WindowRequestId(1),
                epoch: WindowEpoch(10),
            }),
        );
        assert_eq!(stale.status, WindowHostTransitionStatus::IgnoredStale);
        assert!(
            !stale
                .effects
                .iter()
                .any(|effect| matches!(effect, WindowHostEffect::Publish { .. }))
        );
        assert!(matches!(
            stale.state,
            WindowHostState::Switching { old: current, .. } if current == old
        ));

        let ready = reduce_window_host(
            stale.state,
            WindowHostInput::Event(WindowHostEvent::TargetReady {
                request: WindowRequestId(2),
                epoch: WindowEpoch(11),
            }),
        );
        let current = match ready.state {
            WindowHostState::Visible { host, .. } => host,
            other => panic!("expected visible replacement, got {other:?}"),
        };
        assert_eq!(current.windows.presenter().id, old.windows.presenter().id);
        assert_ne!(
            current.windows.presenter().generation,
            old.windows.presenter().generation
        );
    }

    #[test]
    fn close_property_reaches_closed_from_every_nonterminal_state_without_target_ready() {
        let host_a = presenter(1, 10, 100, 10, NativeVideoPlacement::MainWindowChild);
        let host_b = presenter(2, 11, 101, 11, NativeVideoPlacement::DetachedViewerChild);
        let requested = StagingWindow::Requested {
            epoch: WindowEpoch(11),
            spec: host_b.spec,
            visibility: WindowVisibility::Visible,
        };
        let created = StagingWindow::Created {
            host: host_b,
            visibility: WindowVisibility::Visible,
        };
        let states = [
            WindowHostState::Empty,
            WindowHostState::Preparing {
                request: WindowRequestId(2),
                staging: requested,
                prior: PriorHost::NoPrior,
            },
            WindowHostState::Preparing {
                request: WindowRequestId(2),
                staging: created,
                prior: PriorHost::Prior {
                    host: host_a,
                    visibility: WindowVisibility::Visible,
                },
            },
            WindowHostState::Visible {
                request: WindowRequestId(1),
                host: host_a,
            },
            WindowHostState::Hidden {
                request: WindowRequestId(1),
                host: host_a,
            },
            WindowHostState::Switching {
                request: WindowRequestId(2),
                old: host_a,
                staging: requested,
                visibility: WindowVisibility::Visible,
            },
            WindowHostState::Switching {
                request: WindowRequestId(2),
                old: host_a,
                staging: created,
                visibility: WindowVisibility::Visible,
            },
        ];

        for initial in states {
            let mut backend = FakeWindowBackend::new(FakeRenderMode::Stalled);
            let closed = backend.dispatch_and_drain(
                initial,
                WindowHostInput::Command(WindowHostCommand::Shutdown {
                    request: WindowRequestId(100),
                }),
            );
            assert_eq!(closed, WindowHostState::Closed, "initial={initial:?}");
        }
    }

    #[test]
    fn idempotency_property_duplicate_request_does_not_repeat_effects() {
        let first = reduce_window_host(
            WindowHostState::Empty,
            WindowHostInput::Command(open_visible(1, 10)),
        );
        let duplicate =
            reduce_window_host(first.state, WindowHostInput::Command(open_visible(1, 10)));
        assert_eq!(
            duplicate.status,
            WindowHostTransitionStatus::IgnoredIdempotent
        );
        assert!(duplicate.effects.is_empty());
        assert_eq!(duplicate.state, first.state);
    }

    #[test]
    fn close_cancels_staging_with_the_original_request_identity() {
        let transition = reduce_window_host(
            WindowHostState::Preparing {
                request: WindowRequestId(7),
                staging: StagingWindow::Requested {
                    epoch: WindowEpoch(20),
                    spec: spec(
                        NativeVideoPlacement::MainWindowChild,
                        HostWindowTopology::PresenterOnly,
                    ),
                    visibility: WindowVisibility::Visible,
                },
                prior: PriorHost::NoPrior,
            },
            WindowHostInput::Command(WindowHostCommand::Close {
                request: WindowRequestId(8),
                reason: ClosingReason::UserClose,
            }),
        );
        assert!(
            transition
                .effects
                .contains(&WindowHostEffect::CancelCreate {
                    request: WindowRequestId(7),
                    epoch: WindowEpoch(20),
                })
        );
        assert!(transition.effects.contains(&WindowHostEffect::Closed {
            request: WindowRequestId(8),
            reason: ClosingReason::UserClose,
        }));
    }
    #[test]
    fn diagnostics_schema_serializes_thread_hwnd_generation_and_progress() {
        let old = presenter(7, 41, 0x1000, 70, NativeVideoPlacement::MainWindowChild);
        let snapshot = NativeWindowDiagnostics {
            schema_version: NATIVE_WINDOW_DIAGNOSTICS_SCHEMA_VERSION,
            sampled_at: MonotonicMillis(9000),
            source_generation: SourceGeneration(12),
            host: WindowHostState::Switching {
                request: WindowRequestId(8),
                old,
                staging: StagingWindow::Requested {
                    epoch: WindowEpoch(42),
                    spec: spec(
                        NativeVideoPlacement::FullscreenBorderless,
                        HostWindowTopology::PresenterAndHud,
                    ),
                    visibility: WindowVisibility::Visible,
                },
                visibility: WindowVisibility::Visible,
            },
            pump: PumpDiagnostics {
                thread: DiagnosticThread::Running {
                    thread_id: OpaqueThreadId(101),
                },
                message_dispatch: ProgressStamp::Observed {
                    value: ProgressSequence(55),
                    at: MonotonicMillis(8990),
                },
                command_received: ProgressStamp::Observed {
                    value: PumpCommandProgress {
                        request: WindowRequestId(8),
                        epoch: WindowEpoch(42),
                    },
                    at: MonotonicMillis(8995),
                },
                command_completed: ProgressStamp::Observed {
                    value: PumpCommandProgress {
                        request: WindowRequestId(7),
                        epoch: WindowEpoch(41),
                    },
                    at: MonotonicMillis(8980),
                },
            },
            render: RenderDiagnostics {
                thread: DiagnosticThread::Running {
                    thread_id: OpaqueThreadId(202),
                },
                operation_started: ProgressStamp::Observed {
                    value: RenderOperationProgress {
                        operation: RenderOperation::Attach,
                        request: WindowRequestId(8),
                        epoch: WindowEpoch(42),
                    },
                    at: MonotonicMillis(8996),
                },
                operation_completed: ProgressStamp::Observed {
                    value: RenderOperationProgress {
                        operation: RenderOperation::Present,
                        request: WindowRequestId(7),
                        epoch: WindowEpoch(41),
                    },
                    at: MonotonicMillis(8985),
                },
            },
        };

        let value = serde_json::to_value(&snapshot).expect("serialize diagnostics");
        assert_eq!(value["schema_version"], serde_json::json!(1));
        assert_eq!(value["pump"]["thread"]["thread_id"], serde_json::json!(101));
        assert_eq!(
            value["render"]["operation_started"]["value"]["operation"],
            serde_json::json!("attach")
        );
        assert_eq!(
            value["host"]["old"]["windows"]["presenter"]["generation"],
            serde_json::json!(70)
        );
        assert_eq!(value["host"]["staging"]["epoch"], serde_json::json!(42));
        assert_eq!(
            value["host"]["staging"]["spec"]["placement"],
            serde_json::json!("fullscreen_borderless")
        );

        let decoded: NativeWindowDiagnostics =
            serde_json::from_value(value).expect("deserialize diagnostics");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn windows_harness_timeout_and_error_vocabulary_is_stable() {
        let error = WindowsHarnessError::Timeout {
            timeout: WindowsHarnessTimeout {
                phase: WindowsHarnessPhase::PumpPing,
                limit_millis: 2_000,
            },
        };
        assert_eq!(error.to_string(), "timeout during PumpPing after 2000 ms");
        assert_eq!(
            serde_json::to_value(error).expect("serialize harness error"),
            serde_json::json!({
                "error": "timeout",
                "timeout": {
                    "phase": "pump_ping",
                    "limit_millis": 2000
                }
            })
        );
    }
}
