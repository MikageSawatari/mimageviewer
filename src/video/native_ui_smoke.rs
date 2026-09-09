//! Test-script-only observation and driver for real native mouse input.
//!
//! The driver intentionally uses the production `SendInput -> WndProc -> event routes`
//! path. It does not make mouse moves lossless, suspend cursor polling, or inject directly
//! into either consumer. A step succeeds only when the same `dwExtraInfo` token is observed
//! by the actual target WndProc and reaches both the pump cursor reducer and the render input
//! tail for one still-current host/source/geometry snapshot.

#![cfg(all(windows, feature = "test-script"))]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, TryLockError, Weak};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
    MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClientRect, GetForegroundWindow, GetMessageExtraInfo,
    GetSystemMetrics, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::NativeVideoPlacement;
use super::native_presenter::NativeVideoInputRegion;
use super::native_touch::NativeVideoWindowSource;
use super::window_host_contract::{HostWindows, WindowEpoch};

const DISPOSABLE_DATA_MARKER: &str = "mimageviewer-disposable-smoke-v1;test-script=true";
const TARGET_WAIT_POLL: Duration = Duration::from_millis(20);
const TARGET_DIAGNOSTIC_RECORD_LIMIT: usize = 4;
const TARGET_DIAGNOSTIC_PAIR_LIMIT: usize = 8;
const WNDPROC_TRACE_RECORD_LIMIT: usize = 8;
const STEP_TOKEN_PREFIX: usize = 0x4d49_0000;
const STEP_TOKEN_SERIAL_MIN: u32 = 1;
const STEP_TOKEN_SERIAL_MAX: u32 = u16::MAX as u32;
const STEP_TOKEN_SERIAL_EXHAUSTED: u32 = STEP_TOKEN_SERIAL_MAX + 1;

static NEXT_OUTPUT_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PUBLISHER_NONCE: AtomicU64 = AtomicU64::new(1);
static NEXT_OVERLAY_OWNER_NONCE: AtomicU64 = AtomicU64::new(1);
static NEXT_NAMED_TARGET_TOKEN: AtomicU64 = AtomicU64::new(1);
static NEXT_INVENTORY_COMMIT_SERIAL: AtomicU64 = AtomicU64::new(1);
static NEXT_STEP_TOKEN_SERIAL: AtomicU32 = AtomicU32::new(STEP_TOKEN_SERIAL_MIN);
static ACTIVE_STEP_TOKEN: AtomicUsize = AtomicUsize::new(0);
// Process-lifetime count. It is intentionally not presented as belonging to any one step.
static WNDPROC_TRACE_UNAVAILABLE_TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeUiSmokeMessageEntry {
    active_token: usize,
    extra_info: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WndProcTraceRecord {
    entry_active_token: usize,
    match_active_token: usize,
    receiver_hwnd: u64,
    event_x: i32,
    event_y: i32,
    entry_extra_info: usize,
    match_extra_info: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WndProcTraceState {
    owner_token: usize,
    len: usize,
    overflow: usize,
    records: [Option<WndProcTraceRecord>; WNDPROC_TRACE_RECORD_LIMIT],
}

impl Default for WndProcTraceState {
    fn default() -> Self {
        Self {
            owner_token: 0,
            len: 0,
            overflow: 0,
            records: [None; WNDPROC_TRACE_RECORD_LIMIT],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WndProcTraceSnapshot {
    Available {
        len: usize,
        overflow: usize,
        records: [Option<WndProcTraceRecord>; WNDPROC_TRACE_RECORD_LIMIT],
    },
    Inactive,
    OwnerMismatch {
        owner_token: usize,
    },
    Busy,
    Poisoned,
}

fn wndproc_trace() -> &'static Mutex<WndProcTraceState> {
    static TRACE: OnceLock<Mutex<WndProcTraceState>> = OnceLock::new();
    TRACE.get_or_init(|| Mutex::new(WndProcTraceState::default()))
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct NativeUiSmokeOutputId(u64);

pub(crate) fn allocate_output_id() -> NativeUiSmokeOutputId {
    NativeUiSmokeOutputId(NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug)]
pub(crate) struct NativeUiSmokeOverlayOwner {
    nonce: u64,
}

pub(crate) fn allocate_overlay_owner() -> Arc<NativeUiSmokeOverlayOwner> {
    Arc::new(NativeUiSmokeOverlayOwner {
        nonce: NEXT_OVERLAY_OWNER_NONCE.fetch_add(1, Ordering::Relaxed),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeUiSmokeTargetArea {
    rect_points: egui::Rect,
    pixels_per_point: f32,
    client_width: u32,
    client_height: u32,
}

impl NativeUiSmokeTargetArea {
    pub(crate) fn new(
        rect_points: egui::Rect,
        pixels_per_point: f32,
        client_width: u32,
        client_height: u32,
    ) -> Self {
        Self {
            rect_points,
            pixels_per_point,
            client_width,
            client_height,
        }
    }

    fn valid(self) -> bool {
        let rect = self.rect_points;
        rect.is_finite()
            && rect.is_positive()
            && self.pixels_per_point.is_finite()
            && self.pixels_per_point > 0.0
            && self.client_width > 1
            && self.client_height > 1
    }

    fn client_point(self, normalized: [f32; 2]) -> Result<POINT, String> {
        if !self.valid() {
            return Err("native mouse target area is not usable".to_string());
        }
        if !normalized
            .into_iter()
            .all(|value| value.is_finite() && value > 0.0 && value < 1.0)
        {
            return Err("native mouse normalized coordinates must be between zero and one".into());
        }
        let point = egui::pos2(
            self.rect_points.min.x + self.rect_points.width() * normalized[0],
            self.rect_points.min.y + self.rect_points.height() * normalized[1],
        );
        let x = (point.x * self.pixels_per_point).round() as i32;
        let y = (point.y * self.pixels_per_point).round() as i32;
        if x < 0 || y < 0 || x >= self.client_width as i32 || y >= self.client_height as i32 {
            return Err(format!(
                "native mouse target ({x},{y}) falls outside the prepared {}x{} client",
                self.client_width, self.client_height
            ));
        }
        Ok(POINT { x, y })
    }

    fn contains_client_point(self, point: [i32; 2]) -> bool {
        if !self.valid() {
            return false;
        }
        let x = point[0] as f32 / self.pixels_per_point;
        let y = point[1] as f32 / self.pixels_per_point;
        self.rect_points.contains(egui::pos2(x, y))
            && point[0] >= 0
            && point[1] >= 0
            && point[0] < self.client_width as i32
            && point[1] < self.client_height as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeUiSmokePanoramaClassification {
    Unknown,
    Panorama,
    NonPanorama,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeUiSmokeControlObservation {
    pub(crate) rect: egui::Rect,
    pub(crate) interact_rect: egui::Rect,
    pub(crate) clip_rect: egui::Rect,
    pub(crate) layer_id: egui::LayerId,
    pub(crate) sense: egui::Sense,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeUiSmokeLogicalInventory {
    owner: Weak<NativeUiSmokeOverlayOwner>,
    owner_nonce: u64,
    top_hover_activation: Option<NativeUiSmokeTargetArea>,
    native_top_panorama: Option<NativeUiSmokeControlObservation>,
    panorama_classification: NativeUiSmokePanoramaClassification,
    panorama_pose_present: bool,
    video_zoom_present: bool,
    named_control_allowed: bool,
    pixels_per_point: f32,
    client_width: u32,
    client_height: u32,
}

impl NativeUiSmokeLogicalInventory {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        owner: &Arc<NativeUiSmokeOverlayOwner>,
        top_hover_activation: Option<NativeUiSmokeTargetArea>,
        native_top_panorama: Option<NativeUiSmokeControlObservation>,
        panorama_classification: NativeUiSmokePanoramaClassification,
        panorama_pose_present: bool,
        video_zoom_present: bool,
        named_control_allowed: bool,
        pixels_per_point: f32,
        client_width: u32,
        client_height: u32,
    ) -> Self {
        Self {
            owner: Arc::downgrade(owner),
            owner_nonce: owner.nonce,
            top_hover_activation,
            native_top_panorama,
            panorama_classification,
            panorama_pose_present,
            video_zoom_present,
            named_control_allowed,
            pixels_per_point,
            client_width,
            client_height,
        }
    }

    #[cfg(test)]
    pub(crate) fn top_hover_min_y(&self) -> Option<f32> {
        self.top_hover_activation.map(|area| area.rect_points.min.y)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NativeUiSmokeCommittedNamedControl {
    token: Option<u64>,
    observation: NativeUiSmokeControlObservation,
    classification: NativeUiSmokePanoramaClassification,
    panorama_pose_present: bool,
    video_zoom_present: bool,
    area: NativeUiSmokeTargetArea,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeUiSmokeCommittedInventory {
    owner: Weak<NativeUiSmokeOverlayOwner>,
    owner_nonce: u64,
    commit_serial: u64,
    target_version: u64,
    top_hover_activation: Option<NativeUiSmokeTargetArea>,
    native_top_panorama: Option<NativeUiSmokeCommittedNamedControl>,
}

impl NativeUiSmokeCommittedInventory {
    pub(crate) fn commit(previous: Option<&Self>, logical: NativeUiSmokeLogicalInventory) -> Self {
        let named = logical.native_top_panorama.map(|observation| {
            let area_rect = observation
                .rect
                .intersect(observation.interact_rect)
                .intersect(observation.clip_rect);
            let area = NativeUiSmokeTargetArea::new(
                area_rect,
                logical.pixels_per_point,
                logical.client_width,
                logical.client_height,
            );
            let eligible = logical.named_control_allowed
                && observation.enabled
                && observation.sense.senses_click()
                && area.valid()
                && area.client_point([0.5, 0.5]).is_ok();
            let previous_named = previous
                .filter(|previous| previous.owner_nonce == logical.owner_nonce)
                .and_then(|previous| previous.native_top_panorama.as_ref());
            let token = if eligible {
                previous_named
                    .filter(|previous| {
                        previous.token.is_some()
                            && previous.observation == observation
                            && previous.classification == logical.panorama_classification
                            && previous.panorama_pose_present == logical.panorama_pose_present
                            && previous.video_zoom_present == logical.video_zoom_present
                    })
                    .and_then(|previous| previous.token)
                    .or_else(|| Some(NEXT_NAMED_TARGET_TOKEN.fetch_add(1, Ordering::Relaxed)))
            } else {
                None
            };
            NativeUiSmokeCommittedNamedControl {
                token,
                observation,
                classification: logical.panorama_classification,
                panorama_pose_present: logical.panorama_pose_present,
                video_zoom_present: logical.video_zoom_present,
                area,
            }
        });
        let same_targets = previous.is_some_and(|previous| {
            previous.owner_nonce == logical.owner_nonce
                && previous.top_hover_activation == logical.top_hover_activation
                && previous.native_top_panorama.as_ref().map(|named| {
                    (
                        named.token,
                        named.observation,
                        named.classification,
                        named.panorama_pose_present,
                        named.video_zoom_present,
                        named.area,
                    )
                }) == named.as_ref().map(|named| {
                    (
                        named.token,
                        named.observation,
                        named.classification,
                        named.panorama_pose_present,
                        named.video_zoom_present,
                        named.area,
                    )
                })
        });
        Self {
            owner: logical.owner,
            owner_nonce: logical.owner_nonce,
            commit_serial: NEXT_INVENTORY_COMMIT_SERIAL.fetch_add(1, Ordering::Relaxed),
            target_version: previous.map_or(1, |previous| {
                if same_targets {
                    previous.target_version
                } else {
                    previous.target_version.saturating_add(1)
                }
            }),
            top_hover_activation: logical.top_hover_activation,
            native_top_panorama: named,
        }
    }

    fn owner_is_live(&self) -> bool {
        self.owner.upgrade().is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeUiSmokeCanvasGeometry {
    pub(crate) region: NativeVideoInputRegion,
    pub(crate) pixels_per_point: f32,
    pub(crate) client_width: u32,
    pub(crate) client_height: u32,
}

impl NativeUiSmokeCanvasGeometry {
    fn valid(self) -> bool {
        let values = [
            self.region.origin_points[0],
            self.region.origin_points[1],
            self.region.size_points[0],
            self.region.size_points[1],
            self.pixels_per_point,
        ];
        values.into_iter().all(f32::is_finite)
            && self.pixels_per_point > 0.0
            && self.region.size_points[0] > 1.0
            && self.region.size_points[1] > 1.0
            && self.client_width > 1
            && self.client_height > 1
    }

    fn client_point(self, normalized: [f32; 2]) -> Result<POINT, String> {
        if !self.valid() {
            return Err("native mouse target geometry is not usable".to_string());
        }
        if !normalized
            .into_iter()
            .all(|value| value.is_finite() && value > 0.0 && value < 1.0)
        {
            return Err("native mouse normalized coordinates must be between zero and one".into());
        }
        let x_points = self.region.origin_points[0] + self.region.size_points[0] * normalized[0];
        let y_points = self.region.origin_points[1] + self.region.size_points[1] * normalized[1];
        let x = (x_points * self.pixels_per_point).round() as i32;
        let y = (y_points * self.pixels_per_point).round() as i32;
        if x < 0 || y < 0 || x >= self.client_width as i32 || y >= self.client_height as i32 {
            return Err(format!(
                "native mouse target ({x},{y}) falls outside the prepared {}x{} client",
                self.client_width, self.client_height
            ));
        }
        Ok(POINT { x, y })
    }

    #[cfg(test)]
    fn contains_client_point(self, point: [i32; 2]) -> bool {
        if !self.valid() {
            return false;
        }
        let ppp = self.pixels_per_point;
        let min_x = self.region.origin_points[0] * ppp;
        let min_y = self.region.origin_points[1] * ppp;
        let max_x = (self.region.origin_points[0] + self.region.size_points[0]) * ppp;
        let max_y = (self.region.origin_points[1] + self.region.size_points[1]) * ppp;
        let x = point[0] as f32;
        let y = point[1] as f32;
        x >= min_x
            && y >= min_y
            && x < max_x
            && y < max_y
            && point[0] >= 0
            && point[1] >= 0
            && point[0] < self.client_width as i32
            && point[1] < self.client_height as i32
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct PublisherKey {
    output: NativeUiSmokeOutputId,
    nonce: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowIdentity {
    hwnd: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostWindowSet {
    PresenterOnly {
        presenter: WindowIdentity,
    },
    PresenterAndHud {
        presenter: WindowIdentity,
        hud: WindowIdentity,
    },
}

impl HostWindowSet {
    fn from_contract(windows: HostWindows) -> Self {
        match windows {
            HostWindows::PresenterOnly { presenter } => Self::PresenterOnly {
                presenter: WindowIdentity {
                    hwnd: presenter.id.0,
                    generation: presenter.generation.0,
                },
            },
            HostWindows::PresenterAndHud { presenter, hud } => Self::PresenterAndHud {
                presenter: WindowIdentity {
                    hwnd: presenter.id.0,
                    generation: presenter.generation.0,
                },
                hud: WindowIdentity {
                    hwnd: hud.id.0,
                    generation: hud.generation.0,
                },
            },
        }
    }

    fn presenter(self) -> WindowIdentity {
        match self {
            Self::PresenterOnly { presenter } | Self::PresenterAndHud { presenter, .. } => {
                presenter
            }
        }
    }

    fn presenter_only(self) -> bool {
        matches!(self, Self::PresenterOnly { .. })
    }
}

#[derive(Clone, Debug)]
struct HostSnapshot {
    publisher: PublisherKey,
    request: u64,
    epoch: u64,
    placement: NativeVideoPlacement,
    owner_hwnd: u64,
    windows: HostWindowSet,
}

#[derive(Clone)]
struct RenderSnapshot {
    publisher: PublisherKey,
    requested_source_epoch: Weak<AtomicU64>,
    actual_source_epoch: u64,
    generation: u64,
    placement: NativeVideoPlacement,
    owner_hwnd: u64,
    presenter_hwnd: u64,
    geometry: NativeUiSmokeCanvasGeometry,
    geometry_version: u64,
    inventory: NativeUiSmokeCommittedInventory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparedTargetKind {
    Canvas,
    TopHoverActivation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparedTargetValidationPhase {
    BeforeInput,
    ReceiptCompletion,
}

#[derive(Clone)]
struct PreparedTarget {
    host: HostSnapshot,
    render: RenderSnapshot,
    kind: PreparedTargetKind,
    area: NativeUiSmokeTargetArea,
    client_point: POINT,
}

pub(crate) struct NativeUiSmokePreparedMove {
    target: PreparedTarget,
    normalized: [f32; 2],
    coordinate_tolerance: [i32; 2],
}

impl PreparedTarget {
    fn still_matches(&self, state: &BrokerState, phase: PreparedTargetValidationPhase) -> bool {
        let Some(host) = state.hosts.get(&self.host.publisher) else {
            return false;
        };
        let Some(render) = state.renders.get(&self.render.publisher) else {
            return false;
        };
        let identity_matches = host.request == self.host.request
            && host.epoch == self.host.epoch
            && host.placement == self.host.placement
            && host.owner_hwnd == self.host.owner_hwnd
            && host.windows == self.host.windows
            && render.actual_source_epoch == self.render.actual_source_epoch
            && render.generation == self.render.generation
            && render.placement == self.render.placement
            && render.owner_hwnd == self.render.owner_hwnd
            && render.presenter_hwnd == self.render.presenter_hwnd
            && render.inventory.owner_nonce == self.render.inventory.owner_nonce
            && render.inventory.owner_is_live()
            && render
                .requested_source_epoch
                .upgrade()
                .is_some_and(|requested| {
                    requested.load(Ordering::Acquire) == self.render.actual_source_epoch
                });
        if !identity_matches {
            return false;
        }
        match self.kind {
            PreparedTargetKind::Canvas => {
                render.geometry == self.render.geometry
                    && render.geometry_version == self.render.geometry_version
            }
            PreparedTargetKind::TopHoverActivation => {
                // The prepared state proves that this operation starts from a genuinely hidden
                // top bar. The move is expected to publish a newer inventory containing the
                // button before its render receipt is recorded, so that transition is valid only
                // after input while the owner/source/host and activation area stay unchanged.
                self.render.inventory.native_top_panorama.is_none()
                    && render.inventory.top_hover_activation == Some(self.area)
                    && match phase {
                        PreparedTargetValidationPhase::BeforeInput => {
                            render.inventory.target_version == self.render.inventory.target_version
                                && render.inventory.native_top_panorama.is_none()
                        }
                        PreparedTargetValidationPhase::ReceiptCompletion => {
                            render.inventory.target_version > self.render.inventory.target_version
                                && render.inventory.native_top_panorama.is_some()
                        }
                    }
            }
        }
    }

    fn ready_for_initial_prepare(&self) -> bool {
        match self.kind {
            PreparedTargetKind::Canvas => true,
            PreparedTargetKind::TopHoverActivation => {
                self.render.inventory.native_top_panorama.is_none()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeUiSmokeMessageMetadata {
    pub(crate) token: usize,
    pub(crate) receiver_hwnd: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeUiSmokePumpReceipt {
    pub(crate) output: NativeUiSmokeOutputId,
    pub(crate) epoch: u64,
    pub(crate) placement: NativeVideoPlacement,
    pub(crate) owner_hwnd: u64,
    pub(crate) windows: HostWindows,
    pub(crate) source: NativeVideoWindowSource,
    pub(crate) event_x: i32,
    pub(crate) event_y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeUiSmokeRenderReceipt {
    pub(crate) output: NativeUiSmokeOutputId,
    pub(crate) actual_source_epoch: u64,
    pub(crate) generation: u64,
    pub(crate) placement: NativeVideoPlacement,
    pub(crate) owner_hwnd: u64,
    pub(crate) presenter_hwnd: u64,
    pub(crate) geometry: NativeUiSmokeCanvasGeometry,
    pub(crate) geometry_version: u64,
    pub(crate) raw_forwarded: bool,
    pub(crate) command_count: usize,
    pub(crate) source: NativeVideoWindowSource,
    pub(crate) event_x: i32,
    pub(crate) event_y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeUiSmokeMoveReceipt {
    pub(crate) token: u64,
    pub(crate) owner_hwnd: u64,
    pub(crate) presenter_hwnd: u64,
    pub(crate) source_epoch: u64,
    pub(crate) generation: u64,
    pub(crate) requested_client_x: i32,
    pub(crate) requested_client_y: i32,
    pub(crate) actual_client_x: i32,
    pub(crate) actual_client_y: i32,
    host_publisher: PublisherKey,
    render_publisher: PublisherKey,
    overlay_owner_nonce: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeUiSmokeNamedControlReceipt {
    pub(crate) token: u64,
    pub(crate) owner_hwnd: u64,
    pub(crate) presenter_hwnd: u64,
    pub(crate) source_epoch: u64,
    pub(crate) generation: u64,
    pub(crate) client_x: i32,
    pub(crate) client_y: i32,
    pub(crate) pixels_per_point: f32,
    pub(crate) rect: egui::Rect,
    pub(crate) interact_rect: egui::Rect,
    pub(crate) clip_rect: egui::Rect,
    pub(crate) layer_id: egui::LayerId,
    pub(crate) senses_click: bool,
    pub(crate) enabled: bool,
    pub(crate) classification: NativeUiSmokePanoramaClassification,
    pub(crate) panorama_pose_present: bool,
    pub(crate) video_zoom_present: bool,
}

struct PendingStep {
    token: usize,
    target: PreparedTarget,
    pump_actual_point: Option<[i32; 2]>,
    render_actual_point: Option<[i32; 2]>,
    coordinate_tolerance: [i32; 2],
    failure: Option<String>,
}

#[derive(Default)]
struct BrokerState {
    hosts: HashMap<PublisherKey, HostSnapshot>,
    renders: HashMap<PublisherKey, RenderSnapshot>,
    pending: Option<PendingStep>,
}

struct Broker {
    state: Mutex<BrokerState>,
    changed: Condvar,
    faulted: AtomicBool,
}

fn broker() -> &'static Broker {
    static BROKER: OnceLock<Broker> = OnceLock::new();
    BROKER.get_or_init(Broker::new)
}

impl Broker {
    fn new() -> Self {
        Self {
            state: Mutex::new(BrokerState::default()),
            changed: Condvar::new(),
            faulted: AtomicBool::new(false),
        }
    }
}

fn lock_broker_state(broker: &Broker) -> Result<MutexGuard<'_, BrokerState>, String> {
    if broker.faulted.load(Ordering::Acquire) {
        return Err("native mouse diagnostic broker is faulted".into());
    }
    broker.state.lock().map_err(|_| {
        broker.faulted.store(true, Ordering::Release);
        "native mouse diagnostic broker state is poisoned".to_string()
    })
}

pub(crate) struct NativeUiSmokeHostPublisher {
    key: PublisherKey,
}

impl NativeUiSmokeHostPublisher {
    pub(crate) fn new(
        output: NativeUiSmokeOutputId,
        request: u64,
        epoch: WindowEpoch,
        placement: NativeVideoPlacement,
        owner_hwnd: u64,
        windows: HostWindows,
    ) -> Result<Self, String> {
        let key = PublisherKey {
            output,
            nonce: NEXT_PUBLISHER_NONCE.fetch_add(1, Ordering::Relaxed),
        };
        let snapshot = HostSnapshot {
            publisher: key,
            request,
            epoch: epoch.0,
            placement,
            owner_hwnd,
            windows: HostWindowSet::from_contract(windows),
        };
        let broker = broker();
        let mut state = lock_broker_state(broker)?;
        state.hosts.insert(key, snapshot);
        broker.changed.notify_all();
        Ok(Self { key })
    }
}

impl Drop for NativeUiSmokeHostPublisher {
    fn drop(&mut self) {
        let broker = broker();
        let _ = retire_host_publisher_in(broker, self.key);
        broker.changed.notify_all();
    }
}

fn retire_host_publisher_in(broker: &Broker, key: PublisherKey) -> Result<(), String> {
    let mut state = lock_broker_state(broker)?;
    state.hosts.remove(&key);
    fail_pending_if(
        &mut state,
        |step| step.target.host.publisher == key,
        "native mouse host publisher retired during the step",
    );
    Ok(())
}

pub(crate) struct NativeUiSmokeRenderPublisher {
    key: PublisherKey,
    requested_source_epoch: Weak<AtomicU64>,
    invalidated_commit_floor: AtomicU64,
}

impl NativeUiSmokeRenderPublisher {
    pub(crate) fn new(
        output: NativeUiSmokeOutputId,
        requested_source_epoch: &Arc<AtomicU64>,
    ) -> Self {
        Self {
            key: PublisherKey {
                output,
                nonce: NEXT_PUBLISHER_NONCE.fetch_add(1, Ordering::Relaxed),
            },
            requested_source_epoch: Arc::downgrade(requested_source_epoch),
            invalidated_commit_floor: AtomicU64::new(0),
        }
    }

    pub(crate) fn publish(
        &self,
        actual_source_epoch: u64,
        generation: u64,
        placement: NativeVideoPlacement,
        owner_hwnd: u64,
        presenter_hwnd: u64,
        geometry: NativeUiSmokeCanvasGeometry,
        inventory: NativeUiSmokeCommittedInventory,
    ) -> Result<u64, String> {
        if !inventory.owner_is_live() {
            return Err("native mouse overlay observation owner already retired".to_string());
        }
        let broker = broker();
        let mut state = lock_broker_state(broker)?;
        if inventory.commit_serial <= self.invalidated_commit_floor.load(Ordering::Acquire) {
            return Err(
                "native mouse overlay observation predates the last source invalidation"
                    .to_string(),
            );
        }
        let geometry_version = state
            .renders
            .get(&self.key)
            .map(|previous| {
                if previous.geometry == geometry {
                    previous.geometry_version
                } else {
                    previous.geometry_version.saturating_add(1)
                }
            })
            .unwrap_or(1);
        state.renders.insert(
            self.key,
            RenderSnapshot {
                publisher: self.key,
                requested_source_epoch: self.requested_source_epoch.clone(),
                actual_source_epoch,
                generation,
                placement,
                owner_hwnd,
                presenter_hwnd,
                geometry,
                geometry_version,
                inventory,
            },
        );
        broker.changed.notify_all();
        Ok(geometry_version)
    }

    pub(crate) fn invalidate(&self, reason: &str) -> Result<(), String> {
        let broker = broker();
        let mut state = lock_broker_state(broker)?;
        if let Some(retired) = state.renders.remove(&self.key) {
            self.invalidated_commit_floor
                .fetch_max(retired.inventory.commit_serial, Ordering::AcqRel);
        }
        fail_pending_if(
            &mut state,
            |step| step.target.render.publisher == self.key,
            reason,
        );
        broker.changed.notify_all();
        Ok(())
    }

    pub(crate) fn output_id(&self) -> NativeUiSmokeOutputId {
        self.key.output
    }
}

impl Drop for NativeUiSmokeRenderPublisher {
    fn drop(&mut self) {
        let broker = broker();
        let _ = retire_render_publisher_in(broker, self.key);
        broker.changed.notify_all();
    }
}

fn retire_render_publisher_in(broker: &Broker, key: PublisherKey) -> Result<(), String> {
    let mut state = lock_broker_state(broker)?;
    state.renders.remove(&key);
    fail_pending_if(
        &mut state,
        |step| step.target.render.publisher == key,
        "native mouse render publisher retired during the step",
    );
    Ok(())
}

fn fail_pending_if(
    state: &mut BrokerState,
    predicate: impl FnOnce(&PendingStep) -> bool,
    message: &str,
) {
    if let Some(step) = state.pending.as_mut()
        && predicate(step)
        && step.failure.is_none()
    {
        step.failure = Some(message.to_string());
    }
}

fn receipt_point_matches(
    step: &PendingStep,
    source: NativeVideoWindowSource,
    point: [i32; 2],
) -> bool {
    if source != NativeVideoWindowSource::Presenter
        || !step.target.area.contains_client_point(point)
    {
        return false;
    }
    let requested = [step.target.client_point.x, step.target.client_point.y];
    point[0].abs_diff(requested[0]) <= step.coordinate_tolerance[0] as u32
        && point[1].abs_diff(requested[1]) <= step.coordinate_tolerance[1] as u32
}

pub(crate) fn capture_message_entry() -> Option<NativeUiSmokeMessageEntry> {
    let active_token = ACTIVE_STEP_TOKEN.load(Ordering::Acquire);
    (active_token != 0).then(|| NativeUiSmokeMessageEntry {
        active_token,
        extra_info: unsafe { GetMessageExtraInfo().0 as usize },
    })
}

pub(crate) fn message_metadata(
    receiver_hwnd: HWND,
    event: &super::native_window::NativeVideoWindowEvent,
    entry: Option<NativeUiSmokeMessageEntry>,
) -> Option<NativeUiSmokeMessageMetadata> {
    // Keep this second read at the existing match point. The entry read only diagnoses whether
    // earlier WndProc work changed what GetMessageExtraInfo exposes; it does not authorize input.
    let active = ACTIVE_STEP_TOKEN.load(Ordering::Acquire);
    if active == 0 {
        return None;
    }
    let observed = unsafe { GetMessageExtraInfo().0 as usize };
    if let (Some(entry), super::native_window::NativeVideoWindowEvent::MouseMove(mouse)) =
        (entry, event)
    {
        record_wndproc_trace(
            entry,
            active,
            observed,
            hwnd_value(receiver_hwnd),
            mouse.x,
            mouse.y,
        );
    }
    message_metadata_from_values(active, observed, hwnd_value(receiver_hwnd))
}

fn message_metadata_from_values(
    active: usize,
    observed: usize,
    receiver_hwnd: u64,
) -> Option<NativeUiSmokeMessageMetadata> {
    (active != 0 && observed == active).then_some(NativeUiSmokeMessageMetadata {
        token: observed,
        receiver_hwnd,
    })
}

fn begin_wndproc_trace(token: usize) {
    begin_wndproc_trace_in(wndproc_trace(), &WNDPROC_TRACE_UNAVAILABLE_TOTAL, token);
}

fn begin_wndproc_trace_in(
    trace: &Mutex<WndProcTraceState>,
    unavailable_total: &AtomicUsize,
    token: usize,
) {
    match trace.try_lock() {
        Ok(mut state) => {
            *state = WndProcTraceState {
                owner_token: token,
                ..WndProcTraceState::default()
            };
        }
        Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
            unavailable_total.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn record_wndproc_trace(
    entry: NativeUiSmokeMessageEntry,
    match_active_token: usize,
    match_extra_info: usize,
    receiver_hwnd: u64,
    event_x: i32,
    event_y: i32,
) {
    if entry.active_token == 0 || entry.active_token != match_active_token {
        return;
    }
    record_wndproc_trace_in(
        wndproc_trace(),
        &ACTIVE_STEP_TOKEN,
        &WNDPROC_TRACE_UNAVAILABLE_TOTAL,
        entry,
        match_active_token,
        match_extra_info,
        receiver_hwnd,
        event_x,
        event_y,
    );
}

#[allow(clippy::too_many_arguments)]
fn record_wndproc_trace_in(
    trace: &Mutex<WndProcTraceState>,
    active_token: &AtomicUsize,
    unavailable_total: &AtomicUsize,
    entry: NativeUiSmokeMessageEntry,
    match_active_token: usize,
    match_extra_info: usize,
    receiver_hwnd: u64,
    event_x: i32,
    event_y: i32,
) {
    if entry.active_token == 0 || entry.active_token != match_active_token {
        return;
    }
    let mut state = match trace.try_lock() {
        Ok(state) => state,
        Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
            unavailable_total.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    if active_token.load(Ordering::Acquire) != entry.active_token {
        return;
    }
    if state.owner_token != entry.active_token {
        return;
    }
    if state.len == WNDPROC_TRACE_RECORD_LIMIT {
        state.overflow = state.overflow.saturating_add(1);
        return;
    }
    let index = state.len;
    state.records[index] = Some(WndProcTraceRecord {
        entry_active_token: entry.active_token,
        match_active_token,
        receiver_hwnd,
        event_x,
        event_y,
        entry_extra_info: entry.extra_info,
        match_extra_info,
    });
    state.len += 1;
}

pub(crate) fn record_pump_receipt(
    metadata: NativeUiSmokeMessageMetadata,
    receipt: NativeUiSmokePumpReceipt,
) {
    record_pump_receipt_in(broker(), metadata, receipt);
}

fn record_pump_receipt_in(
    broker: &Broker,
    metadata: NativeUiSmokeMessageMetadata,
    receipt: NativeUiSmokePumpReceipt,
) {
    let Ok(mut state) = lock_broker_state(broker) else {
        broker.changed.notify_all();
        return;
    };
    let Some(step) = state.pending.as_mut() else {
        return;
    };
    if step.token != metadata.token {
        return;
    }
    let target = &step.target;
    let actual_windows = HostWindowSet::from_contract(receipt.windows);
    let mismatch = metadata.receiver_hwnd != target.render.presenter_hwnd
        || receipt.output != target.host.publisher.output
        || receipt.epoch != target.host.epoch
        || receipt.placement != target.host.placement
        || receipt.owner_hwnd != target.host.owner_hwnd
        || actual_windows != target.host.windows
        || !receipt_point_matches(step, receipt.source, [receipt.event_x, receipt.event_y])
        || step
            .render_actual_point
            .is_some_and(|point| point != [receipt.event_x, receipt.event_y]);
    if mismatch {
        if step.failure.is_none() {
            step.failure =
                Some("native mouse pump receipt did not match the prepared host or point".into());
        }
    } else {
        step.pump_actual_point = Some([receipt.event_x, receipt.event_y]);
    }
    broker.changed.notify_all();
}

pub(crate) fn record_render_receipt(
    metadata: NativeUiSmokeMessageMetadata,
    receipt: NativeUiSmokeRenderReceipt,
) {
    record_render_receipt_in(broker(), metadata, receipt);
}

fn record_render_receipt_in(
    broker: &Broker,
    metadata: NativeUiSmokeMessageMetadata,
    receipt: NativeUiSmokeRenderReceipt,
) {
    let Ok(mut state) = lock_broker_state(broker) else {
        broker.changed.notify_all();
        return;
    };
    let Some(step) = state.pending.as_mut() else {
        return;
    };
    if step.token != metadata.token {
        return;
    }
    let target = &step.target;
    let requested_matches =
        target
            .render
            .requested_source_epoch
            .upgrade()
            .is_some_and(|requested| {
                requested.load(Ordering::Acquire) == target.render.actual_source_epoch
            });
    let target_geometry_matches = match target.kind {
        PreparedTargetKind::Canvas => {
            receipt.geometry == target.render.geometry
                && receipt.geometry_version == target.render.geometry_version
        }
        PreparedTargetKind::TopHoverActivation => true,
    };
    let mismatch = metadata.receiver_hwnd != target.render.presenter_hwnd
        || receipt.output != target.render.publisher.output
        || receipt.actual_source_epoch != target.render.actual_source_epoch
        || receipt.generation != target.render.generation
        || receipt.placement != target.render.placement
        || receipt.owner_hwnd != target.render.owner_hwnd
        || receipt.presenter_hwnd != target.render.presenter_hwnd
        || !target_geometry_matches
        || !requested_matches
        || !receipt_point_matches(step, receipt.source, [receipt.event_x, receipt.event_y])
        || step
            .pump_actual_point
            .is_some_and(|point| point != [receipt.event_x, receipt.event_y]);
    if mismatch {
        if step.failure.is_none() {
            step.failure = Some(
                "native mouse render receipt did not match the prepared target or point".into(),
            );
        }
    } else {
        step.render_actual_point = Some([receipt.event_x, receipt.event_y]);
    }
    broker.changed.notify_all();
}

pub(crate) fn record_render_failure(
    metadata: NativeUiSmokeMessageMetadata,
    message: impl Into<String>,
) {
    record_render_failure_in(broker(), metadata, message.into());
}

fn record_render_failure_in(
    broker: &Broker,
    metadata: NativeUiSmokeMessageMetadata,
    message: String,
) {
    let Ok(mut state) = lock_broker_state(broker) else {
        broker.changed.notify_all();
        return;
    };
    if let Some(step) = state.pending.as_mut()
        && step.token == metadata.token
        && step.failure.is_none()
    {
        step.failure = Some(message);
    }
    broker.changed.notify_all();
}

pub(crate) fn prepare_real_mouse_in_canvas(
    owner_hwnd: u64,
    normalized: [f32; 2],
    deadline: Instant,
    mut validate_owner_and_interrupt: impl FnMut() -> Result<(), String>,
) -> Result<NativeUiSmokePreparedMove, String> {
    validate_disposable_runtime()?;
    require_unexpired_deadline(deadline, "preparing native mouse input")?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "prepare_before_target_wait",
    )?;
    let broker = broker();
    let target = wait_for_target(
        broker,
        owner_hwnd,
        normalized,
        PreparedTargetKind::Canvas,
        deadline,
        &mut validate_owner_and_interrupt,
    )?;
    validate_os_target(&target)?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "prepare_after_os_target_validation",
    )?;
    require_unexpired_deadline(deadline, "returning the prepared native mouse target")?;
    Ok(NativeUiSmokePreparedMove {
        target,
        normalized,
        coordinate_tolerance: virtual_desktop_coordinate_tolerance()?,
    })
}

pub(crate) fn prepare_real_mouse_in_top_hover_activation(
    owner_hwnd: u64,
    deadline: Instant,
    mut validate_owner_and_interrupt: impl FnMut() -> Result<(), String>,
) -> Result<NativeUiSmokePreparedMove, String> {
    validate_disposable_runtime()?;
    require_unexpired_deadline(deadline, "preparing native top-hover input")?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "prepare_top_hover_before_target_wait",
    )?;
    let normalized = [0.5, 0.5];
    let target = wait_for_target(
        broker(),
        owner_hwnd,
        normalized,
        PreparedTargetKind::TopHoverActivation,
        deadline,
        &mut validate_owner_and_interrupt,
    )?;
    validate_os_target(&target)?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "prepare_top_hover_after_os_target_validation",
    )?;
    require_unexpired_deadline(deadline, "returning the prepared native top-hover target")?;
    Ok(NativeUiSmokePreparedMove {
        target,
        normalized,
        coordinate_tolerance: virtual_desktop_coordinate_tolerance()?,
    })
}

pub(crate) fn send_prepared_real_mouse_move(
    prepared: NativeUiSmokePreparedMove,
    deadline: Instant,
    mut validate_owner_and_interrupt: impl FnMut() -> Result<(), String>,
) -> Result<NativeUiSmokeMoveReceipt, String> {
    validate_disposable_runtime()?;
    require_unexpired_deadline(deadline, "sending native mouse input")?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "send_before_prepared_target_validation",
    )?;
    let broker = broker();
    validate_prepared_target(
        broker,
        &prepared,
        PreparedTargetValidationPhase::BeforeInput,
    )?;
    validate_os_target(&prepared.target)?;
    validate_owner_for_phase(
        &mut validate_owner_and_interrupt,
        "send_after_os_target_validation",
    )?;

    let token = begin_active_step_token_in(&NEXT_STEP_TOKEN_SERIAL, &ACTIVE_STEP_TOKEN)?;
    let _active = ActiveStepToken(token);
    begin_wndproc_trace(token);
    {
        let mut state = lock_broker_state(broker)?;
        if state.pending.is_some() {
            return Err("another native mouse diagnostic receipt is pending".into());
        }
        if !prepared
            .target
            .still_matches(&state, PreparedTargetValidationPhase::BeforeInput)
            || !prepared_target_is_unique(&state, &prepared)?
        {
            return Err("native mouse target changed before SendInput".into());
        }
        state.pending = Some(PendingStep {
            token,
            target: prepared.target.clone(),
            pump_actual_point: None,
            render_actual_point: None,
            coordinate_tolerance: prepared.coordinate_tolerance,
            failure: None,
        });
    }

    run_pending_step(broker, token, || {
        validate_owner_for_phase(
            &mut validate_owner_and_interrupt,
            "send_pending_before_target_validation",
        )?;
        validate_prepared_target(
            broker,
            &prepared,
            PreparedTargetValidationPhase::BeforeInput,
        )?;
        validate_os_target(&prepared.target)?;
        validate_owner_for_phase(
            &mut validate_owner_and_interrupt,
            "send_pending_immediately_before_send_input",
        )?;
        validate_prepared_target(
            broker,
            &prepared,
            PreparedTargetValidationPhase::BeforeInput,
        )?;
        validate_os_target(&prepared.target)?;
        require_unexpired_deadline(deadline, "calling SendInput")?;
        send_absolute_mouse_move(&prepared.target, token)?;
        let actual_point = wait_for_receipts(
            broker,
            token,
            &prepared,
            deadline,
            &mut validate_owner_and_interrupt,
        )?;
        Ok(NativeUiSmokeMoveReceipt {
            token: token as u64,
            owner_hwnd: prepared.target.host.owner_hwnd,
            presenter_hwnd: prepared.target.render.presenter_hwnd,
            source_epoch: prepared.target.render.actual_source_epoch,
            generation: prepared.target.render.generation,
            requested_client_x: prepared.target.client_point.x,
            requested_client_y: prepared.target.client_point.y,
            actual_client_x: actual_point[0],
            actual_client_y: actual_point[1],
            host_publisher: prepared.target.host.publisher,
            render_publisher: prepared.target.render.publisher,
            overlay_owner_nonce: prepared.target.render.inventory.owner_nonce,
        })
    })
}

pub(crate) fn wait_for_native_top_panorama_after_move(
    move_receipt: &NativeUiSmokeMoveReceipt,
    deadline: Instant,
    mut validate_owner_and_interrupt: impl FnMut() -> Result<(), String>,
) -> Result<NativeUiSmokeNamedControlReceipt, String> {
    validate_disposable_runtime()?;
    require_unexpired_deadline(deadline, "waiting for native_top_panorama")?;
    loop {
        validate_owner_for_phase(
            &mut validate_owner_and_interrupt,
            "named_target_before_snapshot",
        )?;
        let broker = broker();
        let state = lock_broker_state(broker)?;
        if let Some(named) = named_control_after_move(&state, move_receipt)? {
            return Ok(named);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(
                "timed out waiting for enabled native_top_panorama on the prepared owner/source/host"
                    .to_string(),
            );
        }
        let wait = TARGET_WAIT_POLL.min(deadline.saturating_duration_since(now));
        let waited = broker.changed.wait_timeout(state, wait);
        match waited {
            Ok((state, _)) => drop(state),
            Err(_) => {
                broker.faulted.store(true, Ordering::Release);
                return Err("native mouse diagnostic broker state is poisoned".into());
            }
        }
        validate_owner_for_phase(
            &mut validate_owner_and_interrupt,
            "named_target_after_broker_wait",
        )?;
    }
}

fn named_control_after_move(
    state: &BrokerState,
    move_receipt: &NativeUiSmokeMoveReceipt,
) -> Result<Option<NativeUiSmokeNamedControlReceipt>, String> {
    let host = state
        .hosts
        .get(&move_receipt.host_publisher)
        .ok_or_else(|| {
            "native mouse host retired while waiting for native_top_panorama".to_string()
        })?;
    let render = state
        .renders
        .get(&move_receipt.render_publisher)
        .ok_or_else(|| {
            "native mouse render target retired while waiting for native_top_panorama".to_string()
        })?;
    let requested_source = render
        .requested_source_epoch
        .upgrade()
        .map(|source| source.load(Ordering::Acquire));
    let identity_matches = host.owner_hwnd == move_receipt.owner_hwnd
        && host.placement == NativeVideoPlacement::DetachedViewerChild
        && host.windows.presenter_only()
        && host.windows.presenter().hwnd == move_receipt.presenter_hwnd
        && render.publisher.output == host.publisher.output
        && render.actual_source_epoch == move_receipt.source_epoch
        && render.generation == move_receipt.generation
        && render.placement == host.placement
        && render.owner_hwnd == move_receipt.owner_hwnd
        && render.presenter_hwnd == move_receipt.presenter_hwnd
        && requested_source == Some(move_receipt.source_epoch)
        && render.inventory.owner_nonce == move_receipt.overlay_owner_nonce
        && render.inventory.owner_is_live();
    if !identity_matches {
        return Err(
            "native mouse owner, source, host, or overlay changed while waiting for native_top_panorama"
                .to_string(),
        );
    }
    let Some(named) = render.inventory.native_top_panorama.as_ref() else {
        return Ok(None);
    };
    let Some(token) = named.token else {
        return Ok(None);
    };
    let point = named.area.client_point([0.5, 0.5])?;
    Ok(Some(NativeUiSmokeNamedControlReceipt {
        token,
        owner_hwnd: render.owner_hwnd,
        presenter_hwnd: render.presenter_hwnd,
        source_epoch: render.actual_source_epoch,
        generation: render.generation,
        client_x: point.x,
        client_y: point.y,
        pixels_per_point: named.area.pixels_per_point,
        rect: named.observation.rect,
        interact_rect: named.observation.interact_rect,
        clip_rect: named.observation.clip_rect,
        layer_id: named.observation.layer_id,
        senses_click: named.observation.sense.senses_click(),
        enabled: named.observation.enabled,
        classification: named.classification,
        panorama_pose_present: named.panorama_pose_present,
        video_zoom_present: named.video_zoom_present,
    }))
}

fn require_unexpired_deadline(deadline: Instant, phase: &str) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err(format!("native mouse deadline expired while {phase}"))
    } else {
        Ok(())
    }
}

fn validate_owner_for_phase(
    validate_owner_and_interrupt: &mut impl FnMut() -> Result<(), String>,
    phase: &str,
) -> Result<(), String> {
    validate_owner_and_interrupt()
        .map_err(|error| format!("{error}; owner_validation_phase={phase}"))
}

fn run_pending_step<T>(
    broker: &Broker,
    token: usize,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let result = operation().map_err(|error| {
        format!(
            "{error}; wndproc_trace={}",
            format_wndproc_trace(
                snapshot_wndproc_trace(token),
                WNDPROC_TRACE_UNAVAILABLE_TOTAL.load(Ordering::Relaxed),
            )
        )
    });
    match lock_broker_state(broker) {
        Ok(mut state) => {
            if state
                .pending
                .as_ref()
                .is_some_and(|step| step.token == token)
            {
                state.pending = None;
            }
        }
        Err(error) => {
            if result.is_ok() {
                return Err(error);
            }
        }
    }
    broker.changed.notify_all();
    result
}

fn wait_for_target(
    broker: &Broker,
    owner_hwnd: u64,
    normalized: [f32; 2],
    kind: PreparedTargetKind,
    deadline: Instant,
    validate_owner_and_interrupt: &mut impl FnMut() -> Result<(), String>,
) -> Result<PreparedTarget, String> {
    let mut scan_count = 0_u64;
    let mut owner_validation_attempts = 0_u64;
    let mut owner_validation_completed = 0_u64;
    loop {
        let (candidate, candidate_count) = {
            let state = lock_broker_state(broker)?;
            let candidate_result = match kind {
                PreparedTargetKind::Canvas => coherent_targets(&state, owner_hwnd, normalized),
                PreparedTargetKind::TopHoverActivation => {
                    coherent_top_hover_targets(&state, owner_hwnd)
                }
            };
            let mut candidates = candidate_result.map_err(|error| {
                format!(
                    "{error}; phase=coherent_target_scan; scan_count={}; last_scan_candidate_count=unknown; owner_validation_attempts={owner_validation_attempts}; owner_validation_completed={owner_validation_completed}; failure_snapshot_current={}",
                    scan_count.saturating_add(1),
                    target_wait_diagnostic_from_state(&state, owner_hwnd, normalized)
                )
            })?;
            scan_count = scan_count.saturating_add(1);
            match candidates.len() {
                0 => (None, 0),
                1 => {
                    let candidate = candidates.remove(0);
                    (
                        candidate.ready_for_initial_prepare().then_some(candidate),
                        1,
                    )
                }
                count => {
                    return Err(format!(
                        "native mouse target is ambiguous: {count} live presenters use owner 0x{owner_hwnd:x}; phase=coherent_target_scan; scan_count={scan_count}; last_scan_candidate_count={count}; owner_validation_attempts={owner_validation_attempts}; owner_validation_completed={owner_validation_completed}; failure_snapshot_current={}",
                        target_wait_diagnostic_from_state(&state, owner_hwnd, normalized)
                    ));
                }
            }
        };
        owner_validation_attempts = owner_validation_attempts.saturating_add(1);
        if let Err(error) = validate_owner_and_interrupt() {
            return Err(target_wait_failure(
                broker,
                owner_hwnd,
                normalized,
                "after_candidate_scan",
                scan_count,
                candidate_count,
                owner_validation_attempts,
                owner_validation_completed,
                error,
            ));
        }
        owner_validation_completed = owner_validation_completed.saturating_add(1);
        if let Err(error) =
            require_unexpired_deadline(deadline, "waiting for the native mouse target")
        {
            return Err(target_wait_failure(
                broker,
                owner_hwnd,
                normalized,
                "after_owner_validation",
                scan_count,
                candidate_count,
                owner_validation_attempts,
                owner_validation_completed,
                error,
            ));
        }
        if let Some(candidate) = candidate {
            return Ok(candidate);
        }
        let now = Instant::now();
        let wait = TARGET_WAIT_POLL.min(deadline.saturating_duration_since(now));
        let state = lock_broker_state(broker)?;
        let waited = broker.changed.wait_timeout(state, wait);
        match waited {
            Ok((state, _)) => drop(state),
            Err(_) => {
                broker.faulted.store(true, Ordering::Release);
                return Err("native mouse diagnostic broker state is poisoned".into());
            }
        }
        owner_validation_attempts = owner_validation_attempts.saturating_add(1);
        if let Err(error) = validate_owner_and_interrupt() {
            return Err(target_wait_failure(
                broker,
                owner_hwnd,
                normalized,
                "after_broker_wait",
                scan_count,
                candidate_count,
                owner_validation_attempts,
                owner_validation_completed,
                error,
            ));
        }
        owner_validation_completed = owner_validation_completed.saturating_add(1);
    }
}

fn target_wait_failure(
    broker: &Broker,
    owner_hwnd: u64,
    normalized: [f32; 2],
    phase: &str,
    scan_count: u64,
    last_candidate_count: usize,
    owner_validation_attempts: u64,
    owner_validation_completed: u64,
    error: String,
) -> String {
    match lock_broker_state(broker) {
        Ok(state) => format!(
            "{error}; phase={phase}; scan_count={scan_count}; last_scan_candidate_count={last_candidate_count}; owner_validation_attempts={owner_validation_attempts}; owner_validation_completed={owner_validation_completed}; failure_snapshot_current={}",
            target_wait_diagnostic_from_state(&state, owner_hwnd, normalized)
        ),
        Err(diagnostic_error) => format!(
            "{error}; phase={phase}; scan_count={scan_count}; last_scan_candidate_count={last_candidate_count}; owner_validation_attempts={owner_validation_attempts}; owner_validation_completed={owner_validation_completed}; failure_snapshot_current=unavailable({diagnostic_error})"
        ),
    }
}

struct TargetPairEvaluation {
    host_owner: bool,
    host_placement: bool,
    presenter_only: bool,
    output: bool,
    generation: bool,
    placement: bool,
    owner: bool,
    presenter_hwnd: bool,
    presenter_generation: bool,
    requested: RequestedSourceState,
    source: Option<bool>,
    geometry: Option<bool>,
    point: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestedSourceState {
    NotEvaluated,
    Expired,
    Value(u64),
}

impl std::fmt::Display for RequestedSourceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEvaluated => formatter.write_str("not_evaluated"),
            Self::Expired => formatter.write_str("expired"),
            Self::Value(value) => write!(formatter, "value({value})"),
        }
    }
}

impl TargetPairEvaluation {
    fn host_eligible(&self) -> bool {
        self.host_owner && self.host_placement && self.presenter_only
    }

    fn render_matches_host(&self) -> bool {
        self.output
            && self.generation
            && self.placement
            && self.owner
            && self.presenter_hwnd
            && self.presenter_generation
    }

    fn matches_before_point(&self) -> bool {
        self.host_eligible()
            && self.render_matches_host()
            && self.source == Some(true)
            && self.geometry == Some(true)
    }

    fn coherent(&self) -> bool {
        self.matches_before_point() && self.point == Some(true)
    }
}

fn evaluate_target_pair(
    host: &HostSnapshot,
    render: &RenderSnapshot,
    owner_hwnd: u64,
    normalized: [f32; 2],
) -> TargetPairEvaluation {
    let presenter = host.windows.presenter();
    let mut evaluation = TargetPairEvaluation {
        host_owner: host.owner_hwnd == owner_hwnd,
        host_placement: host.placement == NativeVideoPlacement::DetachedViewerChild,
        presenter_only: host.windows.presenter_only(),
        output: render.publisher.output == host.publisher.output,
        generation: render.generation == host.epoch,
        placement: render.placement == host.placement,
        owner: render.owner_hwnd == host.owner_hwnd,
        presenter_hwnd: render.presenter_hwnd == presenter.hwnd,
        presenter_generation: presenter.generation == host.epoch,
        requested: RequestedSourceState::NotEvaluated,
        source: None,
        geometry: None,
        point: None,
    };
    if evaluation.host_eligible() && evaluation.render_matches_host() {
        let requested = render
            .requested_source_epoch
            .upgrade()
            .map(|value| value.load(Ordering::Acquire));
        let geometry = render.geometry.valid() && render.inventory.owner_is_live();
        evaluation.requested =
            requested.map_or(RequestedSourceState::Expired, RequestedSourceState::Value);
        evaluation.source = Some(requested == Some(render.actual_source_epoch));
        evaluation.geometry = Some(geometry);
        evaluation.point = Some(geometry && render.geometry.client_point(normalized).is_ok());
    }
    evaluation
}

fn target_wait_diagnostic_from_state(
    state: &BrokerState,
    owner_hwnd: u64,
    normalized: [f32; 2],
) -> String {
    let mut hosts: Vec<_> = state
        .hosts
        .values()
        .filter(|host| host.owner_hwnd == owner_hwnd)
        .take(TARGET_DIAGNOSTIC_RECORD_LIMIT)
        .collect();
    if hosts.len() < TARGET_DIAGNOSTIC_RECORD_LIMIT {
        for host in state.hosts.values() {
            if hosts.len() >= TARGET_DIAGNOSTIC_RECORD_LIMIT {
                break;
            }
            if !hosts
                .iter()
                .any(|sampled| sampled.publisher == host.publisher)
            {
                hosts.push(host);
            }
        }
    }
    let mut renders: Vec<_> = state
        .renders
        .values()
        .filter(|render| render.owner_hwnd == owner_hwnd)
        .take(TARGET_DIAGNOSTIC_RECORD_LIMIT)
        .collect();
    if renders.len() < TARGET_DIAGNOSTIC_RECORD_LIMIT {
        for render in state.renders.values() {
            if renders.len() >= TARGET_DIAGNOSTIC_RECORD_LIMIT {
                break;
            }
            if !renders
                .iter()
                .any(|sampled| sampled.publisher == render.publisher)
            {
                renders.push(render);
            }
        }
    }

    let pair_space = state.hosts.len().saturating_mul(state.renders.len());
    let mut pair_details = Vec::new();
    for host in &hosts {
        for render in &renders {
            if pair_details.len() >= TARGET_DIAGNOSTIC_PAIR_LIMIT {
                break;
            }
            let evaluation = evaluate_target_pair(host, render, owner_hwnd, normalized);
            pair_details.push(format!(
                "pair[h={}:{},r={}:{}]={{host_owner={},host_placement={},presenter_only={},output={},generation={},placement={},owner={},presenter_hwnd={},presenter_generation={},requested_state={},actual={},source={:?},geometry={:?},point={:?},coherent={}}}",
                host.publisher.output.0,
                host.publisher.nonce,
                render.publisher.output.0,
                render.publisher.nonce,
                evaluation.host_owner,
                evaluation.host_placement,
                evaluation.presenter_only,
                evaluation.output,
                evaluation.generation,
                evaluation.placement,
                evaluation.owner,
                evaluation.presenter_hwnd,
                evaluation.presenter_generation,
                evaluation.requested,
                render.actual_source_epoch,
                evaluation.source,
                evaluation.geometry,
                evaluation.point,
                evaluation.coherent(),
            ));
        }
    }

    let mut diagnostic = format!(
        "target_snapshot={{owner=0x{owner_hwnd:x},normalized=({:.3},{:.3}),hosts={},renders={},pair_space={pair_space},pairs_sampled={}",
        normalized[0],
        normalized[1],
        state.hosts.len(),
        state.renders.len(),
        pair_details.len(),
    );
    for (index, host) in hosts
        .iter()
        .take(TARGET_DIAGNOSTIC_RECORD_LIMIT)
        .enumerate()
    {
        let presenter = host.windows.presenter();
        let _ = write!(
            diagnostic,
            ",host[{index}]={{output={},nonce={},request={},epoch={},placement={},owner=0x{:x},presenter=0x{:x},presenter_generation={},presenter_only={}}}",
            host.publisher.output.0,
            host.publisher.nonce,
            host.request,
            host.epoch,
            host.placement.label(),
            host.owner_hwnd,
            presenter.hwnd,
            presenter.generation,
            host.windows.presenter_only(),
        );
    }
    for (index, render) in renders
        .iter()
        .take(TARGET_DIAGNOSTIC_RECORD_LIMIT)
        .enumerate()
    {
        let requested = render
            .requested_source_epoch
            .upgrade()
            .map(|value| value.load(Ordering::Acquire));
        let _ = write!(
            diagnostic,
            ",render[{index}]={{output={},nonce={},actual={},requested_state={},generation={},placement={},owner=0x{:x},presenter=0x{:x},geometry_version={},region=({:.1},{:.1},{:.1},{:.1}),ppp={:.3},client={}x{},geometry_valid={}}}",
            render.publisher.output.0,
            render.publisher.nonce,
            render.actual_source_epoch,
            requested.map_or_else(|| "expired".to_string(), |value| format!("value({value})")),
            render.generation,
            render.placement.label(),
            render.owner_hwnd,
            render.presenter_hwnd,
            render.geometry_version,
            render.geometry.region.origin_points[0],
            render.geometry.region.origin_points[1],
            render.geometry.region.size_points[0],
            render.geometry.region.size_points[1],
            render.geometry.pixels_per_point,
            render.geometry.client_width,
            render.geometry.client_height,
            render.geometry.valid(),
        );
    }
    if state.hosts.len() > hosts.len() {
        let _ = write!(
            diagnostic,
            ",hosts_omitted={}",
            state.hosts.len() - hosts.len()
        );
    }
    if state.renders.len() > renders.len() {
        let _ = write!(
            diagnostic,
            ",renders_omitted={}",
            state.renders.len() - renders.len()
        );
    }
    for detail in &pair_details {
        diagnostic.push(',');
        diagnostic.push_str(detail);
    }
    if pair_space > pair_details.len() {
        let _ = write!(
            diagnostic,
            ",pairs_omitted={}",
            pair_space - pair_details.len()
        );
    }
    diagnostic.push('}');
    diagnostic
}

fn coherent_targets(
    state: &BrokerState,
    owner_hwnd: u64,
    normalized: [f32; 2],
) -> Result<Vec<PreparedTarget>, String> {
    let mut candidates = Vec::new();
    for host in state.hosts.values().filter(|host| {
        host.owner_hwnd == owner_hwnd
            && host.placement == NativeVideoPlacement::DetachedViewerChild
            && host.windows.presenter_only()
    }) {
        for render in state.renders.values() {
            let evaluation = evaluate_target_pair(host, render, owner_hwnd, normalized);
            if !evaluation.matches_before_point() {
                continue;
            }
            candidates.push(PreparedTarget {
                host: host.clone(),
                render: render.clone(),
                kind: PreparedTargetKind::Canvas,
                area: NativeUiSmokeTargetArea::new(
                    egui::Rect::from_min_size(
                        egui::pos2(
                            render.geometry.region.origin_points[0],
                            render.geometry.region.origin_points[1],
                        ),
                        egui::vec2(
                            render.geometry.region.size_points[0],
                            render.geometry.region.size_points[1],
                        ),
                    ),
                    render.geometry.pixels_per_point,
                    render.geometry.client_width,
                    render.geometry.client_height,
                ),
                client_point: render.geometry.client_point(normalized)?,
            });
        }
    }
    Ok(candidates)
}

fn coherent_top_hover_targets(
    state: &BrokerState,
    owner_hwnd: u64,
) -> Result<Vec<PreparedTarget>, String> {
    let normalized = [0.5, 0.5];
    let mut candidates = Vec::new();
    for host in state.hosts.values().filter(|host| {
        host.owner_hwnd == owner_hwnd
            && host.placement == NativeVideoPlacement::DetachedViewerChild
            && host.windows.presenter_only()
    }) {
        for render in state.renders.values() {
            let evaluation = evaluate_target_pair(host, render, owner_hwnd, normalized);
            if !evaluation.host_eligible() || !evaluation.render_matches_host() {
                continue;
            }
            let requested = render
                .requested_source_epoch
                .upgrade()
                .map(|value| value.load(Ordering::Acquire));
            if requested != Some(render.actual_source_epoch) || !render.inventory.owner_is_live() {
                continue;
            }
            let Some(area) = render.inventory.top_hover_activation else {
                continue;
            };
            candidates.push(PreparedTarget {
                host: host.clone(),
                render: render.clone(),
                kind: PreparedTargetKind::TopHoverActivation,
                area,
                client_point: area.client_point(normalized)?,
            });
        }
    }
    Ok(candidates)
}

fn validate_prepared_target(
    broker: &Broker,
    prepared: &NativeUiSmokePreparedMove,
    phase: PreparedTargetValidationPhase,
) -> Result<(), String> {
    let state = lock_broker_state(broker)?;
    if !prepared.target.still_matches(&state, phase)
        || !prepared_target_is_unique(&state, prepared)?
    {
        return Err("native mouse prepared target is no longer the unique current target".into());
    }
    Ok(())
}

fn prepared_target_is_unique(
    state: &BrokerState,
    prepared: &NativeUiSmokePreparedMove,
) -> Result<bool, String> {
    let candidates = match prepared.target.kind {
        PreparedTargetKind::Canvas => {
            coherent_targets(state, prepared.target.host.owner_hwnd, prepared.normalized)?
        }
        PreparedTargetKind::TopHoverActivation => {
            coherent_top_hover_targets(state, prepared.target.host.owner_hwnd)?
        }
    };
    Ok(matches!(candidates.as_slice(), [candidate]
        if candidate.host.publisher == prepared.target.host.publisher
            && candidate.render.publisher == prepared.target.render.publisher))
}

fn wait_for_receipts(
    broker: &Broker,
    token: usize,
    prepared: &NativeUiSmokePreparedMove,
    deadline: Instant,
    validate_owner_and_interrupt: &mut impl FnMut() -> Result<(), String>,
) -> Result<[i32; 2], String> {
    loop {
        let complete = {
            let state = lock_broker_state(broker)?;
            completed_actual_point(&state, token, prepared)?.is_some()
        };
        if let Err(error) = validate_owner_for_phase(
            validate_owner_and_interrupt,
            "receipt_wait_after_state_scan",
        ) {
            return Err(receipt_wait_failure(broker, token, error));
        }
        if complete {
            require_unexpired_deadline(deadline, "validating native mouse receipts")?;
            validate_os_target(&prepared.target)?;
            validate_prepared_target(
                broker,
                prepared,
                PreparedTargetValidationPhase::ReceiptCompletion,
            )?;
            if let Err(error) = validate_owner_for_phase(
                validate_owner_and_interrupt,
                "receipt_completion_after_target_validation",
            ) {
                return Err(receipt_wait_failure(broker, token, error));
            }
            let state = lock_broker_state(broker)?;
            let point = completed_actual_point(&state, token, prepared)?.ok_or_else(|| {
                "native mouse receipt completion was revoked during final validation".to_string()
            })?;
            drop(state);
            require_unexpired_deadline(deadline, "returning native mouse receipts")?;
            return Ok(point);
        }
        let now = Instant::now();
        if now >= deadline {
            let state = lock_broker_state(broker)?;
            let Some(step) = state.pending.as_ref() else {
                return Err("native mouse receipt state disappeared".into());
            };
            return Err(format!(
                "native mouse receipt timed out (WndProc token observed only where available; pump={}, render={})",
                step.pump_actual_point.is_some(),
                step.render_actual_point.is_some()
            ));
        }
        let wait = TARGET_WAIT_POLL.min(deadline.saturating_duration_since(now));
        let state = lock_broker_state(broker)?;
        let waited = broker.changed.wait_timeout(state, wait);
        match waited {
            Ok((state, _)) => drop(state),
            Err(_) => {
                broker.faulted.store(true, Ordering::Release);
                return Err("native mouse diagnostic broker state is poisoned".into());
            }
        }
    }
}

fn receipt_wait_failure(broker: &Broker, expected_token: usize, error: String) -> String {
    match lock_broker_state(broker) {
        Ok(state) => match state.pending.as_ref() {
            Some(step) => format!(
                "{error}; receipt_snapshot={{expected_token=0x{expected_token:x},pending_token=0x{:x},requested=({},{}),tolerance=({},{}),pump_actual={},render_actual={},failure={}}}",
                step.token,
                step.target.client_point.x,
                step.target.client_point.y,
                step.coordinate_tolerance[0],
                step.coordinate_tolerance[1],
                format_receipt_point(step.pump_actual_point),
                format_receipt_point(step.render_actual_point),
                step.failure
                    .as_deref()
                    .map(bounded_diagnostic_text)
                    .unwrap_or_else(|| "none".to_string()),
            ),
            None => format!(
                "{error}; receipt_snapshot={{expected_token=0x{expected_token:x},pending=none}}"
            ),
        },
        Err(diagnostic_error) => format!(
            "{error}; receipt_snapshot={{expected_token=0x{expected_token:x},unavailable={}}}",
            bounded_diagnostic_text(&diagnostic_error)
        ),
    }
}

fn format_receipt_point(point: Option<[i32; 2]>) -> String {
    point.map_or_else(
        || "none".to_string(),
        |point| format!("({},{})", point[0], point[1]),
    )
}

fn bounded_diagnostic_text(text: &str) -> String {
    const LIMIT: usize = 240;
    let mut chars = text.chars();
    let bounded: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{bounded}...")
    } else {
        bounded
    }
}

fn snapshot_wndproc_trace(expected_token: usize) -> WndProcTraceSnapshot {
    snapshot_wndproc_trace_in(wndproc_trace(), expected_token, || {
        ACTIVE_STEP_TOKEN.load(Ordering::Acquire)
    })
}

fn snapshot_wndproc_trace_in(
    trace: &Mutex<WndProcTraceState>,
    expected_token: usize,
    active_token: impl Fn() -> usize,
) -> WndProcTraceSnapshot {
    if active_token() != expected_token {
        return WndProcTraceSnapshot::Inactive;
    }
    let state = match trace.try_lock() {
        Ok(state) => state,
        Err(TryLockError::WouldBlock) => return WndProcTraceSnapshot::Busy,
        Err(TryLockError::Poisoned(_)) => return WndProcTraceSnapshot::Poisoned,
    };
    if state.owner_token != expected_token {
        return WndProcTraceSnapshot::OwnerMismatch {
            owner_token: state.owner_token,
        };
    }
    let snapshot = WndProcTraceSnapshot::Available {
        len: state.len,
        overflow: state.overflow,
        records: state.records,
    };
    if active_token() != expected_token {
        WndProcTraceSnapshot::Inactive
    } else {
        snapshot
    }
}

fn format_wndproc_trace(
    snapshot: WndProcTraceSnapshot,
    process_unavailable_total: usize,
) -> String {
    match snapshot {
        WndProcTraceSnapshot::Available {
            len,
            overflow,
            records,
        } => {
            let mut text = format!(
                "available,count={len},overflow={overflow},process_unavailable_total={process_unavailable_total},records=["
            );
            for (index, record) in records.into_iter().take(len).flatten().enumerate() {
                if index > 0 {
                    text.push('|');
                }
                let matched = record.match_active_token != 0
                    && record.match_extra_info == record.match_active_token;
                let _ = write!(
                    text,
                    "kind=mouse_move,entry_active=0x{:x},match_active=0x{:x},hwnd=0x{:x},client=({},{}),entry_extra=0x{:x},match_extra=0x{:x},matched={matched}",
                    record.entry_active_token,
                    record.match_active_token,
                    record.receiver_hwnd,
                    record.event_x,
                    record.event_y,
                    record.entry_extra_info,
                    record.match_extra_info,
                );
            }
            text.push(']');
            text
        }
        WndProcTraceSnapshot::Inactive => format!(
            "unavailable(reason=active-token-changed,process_unavailable_total={process_unavailable_total})"
        ),
        WndProcTraceSnapshot::OwnerMismatch { owner_token } => {
            format!(
                "unavailable(reason=owner-mismatch,owner=0x{owner_token:x},process_unavailable_total={process_unavailable_total})"
            )
        }
        WndProcTraceSnapshot::Busy => format!(
            "unavailable(reason=busy,process_unavailable_total={process_unavailable_total})"
        ),
        WndProcTraceSnapshot::Poisoned => format!(
            "unavailable(reason=poisoned,process_unavailable_total={process_unavailable_total})"
        ),
    }
}

fn completed_actual_point(
    state: &BrokerState,
    token: usize,
    prepared: &NativeUiSmokePreparedMove,
) -> Result<Option<[i32; 2]>, String> {
    let Some(step) = state.pending.as_ref() else {
        return Err("native mouse receipt state disappeared".into());
    };
    if step.token != token {
        return Err("native mouse receipt token was replaced".into());
    }
    if let Some(failure) = step.failure.as_ref() {
        return Err(failure.clone());
    }
    let (Some(pump), Some(render)) = (step.pump_actual_point, step.render_actual_point) else {
        return Ok(None);
    };
    if pump != render {
        return Err("native mouse pump and render receipts disagree on actual point".into());
    }
    if !prepared
        .target
        .still_matches(state, PreparedTargetValidationPhase::ReceiptCompletion)
        || !prepared_target_is_unique(state, prepared)?
    {
        return Err("native mouse target changed while completing receipts".into());
    }
    Ok(Some(pump))
}

fn validate_disposable_runtime() -> Result<(), String> {
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let expected_exe = manifest_root
        .join("target")
        .join("portable-smoke")
        .join("mimageviewer.exe");
    let expected_data = expected_exe
        .parent()
        .expect("portable smoke exe has a parent")
        .join("data");
    let actual_exe = std::env::current_exe()
        .map_err(|error| format!("could not inspect current executable: {error}"))?;
    let actual_data = crate::data_dir::get();
    require_same_canonical_path(&actual_exe, &expected_exe, "executable")?;
    require_same_canonical_path(&actual_data, &expected_data, "data directory")?;
    let marker_path = expected_data.join(".disposable-smoke-data");
    let marker = std::fs::read_to_string(&marker_path).map_err(|error| {
        format!(
            "could not read disposable smoke marker {}: {error}",
            marker_path.display()
        )
    })?;
    if marker.trim_end_matches(&['\r', '\n'][..]) != DISPOSABLE_DATA_MARKER {
        return Err("disposable smoke data marker does not authorize test-script input".into());
    }
    Ok(())
}

fn require_same_canonical_path(actual: &Path, expected: &Path, label: &str) -> Result<(), String> {
    let actual = std::fs::canonicalize(actual).map_err(|error| {
        format!(
            "could not canonicalize {label} {}: {error}",
            actual.display()
        )
    })?;
    let expected = std::fs::canonicalize(expected).map_err(|error| {
        format!(
            "could not canonicalize expected {label} {}: {error}",
            expected.display()
        )
    })?;
    if actual != expected {
        return Err(format!(
            "native input is restricted to {} (actual {})",
            expected.display(),
            actual.display()
        ));
    }
    Ok(())
}

fn validate_os_target(target: &PreparedTarget) -> Result<(), String> {
    let presenter = hwnd_from_value(target.render.presenter_hwnd);
    let owner = hwnd_from_value(target.host.owner_hwnd);
    if !unsafe { IsWindow(Some(presenter)).as_bool() }
        || !unsafe { IsWindowVisible(presenter).as_bool() }
    {
        return Err("native mouse presenter HWND is not a visible live window".into());
    }
    if !unsafe { IsWindow(Some(owner)).as_bool() } || !unsafe { IsWindowVisible(owner).as_bool() } {
        return Err("native mouse detached owner HWND is not a visible live window".into());
    }
    let process_id = unsafe { GetCurrentProcessId() };
    let mut presenter_process = 0;
    let mut owner_process = 0;
    unsafe {
        GetWindowThreadProcessId(presenter, Some(&mut presenter_process));
        GetWindowThreadProcessId(owner, Some(&mut owner_process));
    }
    if presenter_process != process_id || owner_process != process_id {
        return Err("native mouse target does not belong to the disposable process".into());
    }
    if unsafe { GetAncestor(presenter, GA_ROOT) } != owner {
        return Err(
            "native mouse presenter is no longer hosted by the selected detached window".into(),
        );
    }
    if unsafe { GetForegroundWindow() } != owner {
        return Err("selected detached window is not the foreground window".into());
    }
    let _dpi = ThreadDpiContext::enter()?;
    let mut rect = RECT::default();
    if unsafe { GetClientRect(presenter, &mut rect) }.is_err() {
        return Err("GetClientRect failed for native mouse presenter".into());
    }
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    if (width, height)
        != (
            target.render.geometry.client_width,
            target.render.geometry.client_height,
        )
    {
        return Err(format!(
            "native mouse client changed from {}x{} to {width}x{height}",
            target.render.geometry.client_width, target.render.geometry.client_height
        ));
    }
    Ok(())
}

fn send_absolute_mouse_move(target: &PreparedTarget, token: usize) -> Result<(), String> {
    let _dpi = ThreadDpiContext::enter()?;
    let presenter = hwnd_from_value(target.render.presenter_hwnd);
    let mut screen = target.client_point;
    if !unsafe { ClientToScreen(presenter, &mut screen).as_bool() } {
        return Err("ClientToScreen failed for native mouse target".into());
    }
    let (dx, dy) = virtual_desktop_absolute(screen)?;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: token,
            },
        },
    };
    let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if inserted != 1 {
        return Err(format!(
            "SendInput inserted {inserted} of 1 native mouse events"
        ));
    }
    Ok(())
}

fn virtual_desktop_absolute(screen: POINT) -> Result<(i32, i32), String> {
    let (left, top, width, height) = virtual_desktop_metrics()?;
    if screen.x < left
        || screen.y < top
        || screen.x >= left.saturating_add(width)
        || screen.y >= top.saturating_add(height)
    {
        return Err(format!(
            "native mouse screen target ({},{}) is outside virtual desktop ({left},{top}) {width}x{height}",
            screen.x, screen.y
        ));
    }
    let scale = |value: i32, origin: i32, extent: i32| -> i32 {
        let numerator = i64::from(value - origin) * 65_535;
        ((numerator + i64::from(extent - 1) / 2) / i64::from(extent - 1)) as i32
    };
    Ok((scale(screen.x, left, width), scale(screen.y, top, height)))
}

fn virtual_desktop_metrics() -> Result<(i32, i32, i32, i32), String> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 1 || height <= 1 {
        return Err("virtual desktop extent is not usable".into());
    }
    Ok((left, top, width, height))
}

fn virtual_desktop_coordinate_tolerance() -> Result<[i32; 2], String> {
    let (_, _, width, height) = virtual_desktop_metrics()?;
    let tolerance = |extent: i32| 1 + (extent - 1) / 131_070;
    Ok([tolerance(width), tolerance(height)])
}

fn allocate_step_token_in(next_serial: &AtomicU32) -> Result<usize, String> {
    let serial = next_serial
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |serial| {
            if (STEP_TOKEN_SERIAL_MIN..STEP_TOKEN_SERIAL_MAX).contains(&serial) {
                Some(serial + 1)
            } else if serial == STEP_TOKEN_SERIAL_MAX {
                Some(STEP_TOKEN_SERIAL_EXHAUSTED)
            } else {
                None
            }
        })
        .map_err(|_| {
            "native mouse diagnostic step token space is exhausted for this process".to_string()
        })?;
    Ok(STEP_TOKEN_PREFIX | serial as usize)
}

fn begin_active_step_token_in(
    next_serial: &AtomicU32,
    active_token: &AtomicUsize,
) -> Result<usize, String> {
    let token = allocate_step_token_in(next_serial)?;
    active_token
        .compare_exchange(0, token, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "another native mouse diagnostic step is already active".to_string())?;
    Ok(token)
}

struct ActiveStepToken(usize);

impl Drop for ActiveStepToken {
    fn drop(&mut self) {
        let _ = ACTIVE_STEP_TOKEN.compare_exchange(self.0, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

struct ThreadDpiContext(DPI_AWARENESS_CONTEXT);

impl ThreadDpiContext {
    fn enter() -> Result<Self, String> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            Err("SetThreadDpiAwarenessContext failed for native mouse driver".into())
        } else {
            Ok(Self(previous))
        }
    }
}

impl Drop for ThreadDpiContext {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

fn hwnd_from_value(value: u64) -> HWND {
    HWND(value as usize as *mut c_void)
}

fn hwnd_value(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::window_host_contract::{
        OpaqueWindowHandle, OpaqueWindowId, WindowGeneration,
    };

    fn presenter_only(hwnd: u64, generation: u64) -> HostWindows {
        HostWindows::PresenterOnly {
            presenter: OpaqueWindowHandle {
                id: OpaqueWindowId(hwnd),
                generation: WindowGeneration(generation),
            },
        }
    }

    fn geometry() -> NativeUiSmokeCanvasGeometry {
        NativeUiSmokeCanvasGeometry {
            region: NativeVideoInputRegion {
                origin_points: [10.0, 20.0],
                size_points: [100.0, 60.0],
            },
            pixels_per_point: 2.0,
            client_width: 400,
            client_height: 240,
        }
    }

    fn isolated_state() -> BrokerState {
        BrokerState::default()
    }

    fn test_inventory(owner: &Arc<NativeUiSmokeOverlayOwner>) -> NativeUiSmokeCommittedInventory {
        NativeUiSmokeCommittedInventory::commit(
            None,
            NativeUiSmokeLogicalInventory::new(
                owner,
                Some(NativeUiSmokeTargetArea::new(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 36.0)),
                    2.0,
                    400,
                    240,
                )),
                None,
                NativeUiSmokePanoramaClassification::Unknown,
                false,
                false,
                false,
                2.0,
                400,
                240,
            ),
        )
    }

    fn test_panorama_observation(enabled: bool) -> NativeUiSmokeControlObservation {
        NativeUiSmokeControlObservation {
            rect: egui::Rect::from_min_size(egui::pos2(280.0, 13.0), egui::vec2(28.0, 28.0)),
            interact_rect: egui::Rect::from_min_size(
                egui::pos2(280.0, 13.0),
                egui::vec2(28.0, 28.0),
            ),
            clip_rect: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(400.0, 64.0)),
            layer_id: egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("native_video_top_bar"),
            ),
            sense: if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
            enabled,
        }
    }

    fn test_fixture_panorama_observation(enabled: bool) -> NativeUiSmokeControlObservation {
        NativeUiSmokeControlObservation {
            rect: egui::Rect::from_min_size(egui::pos2(120.0, 4.0), egui::vec2(28.0, 28.0)),
            interact_rect: egui::Rect::from_min_size(
                egui::pos2(120.0, 4.0),
                egui::vec2(28.0, 28.0),
            ),
            clip_rect: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 36.0)),
            layer_id: egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("native_video_top_bar"),
            ),
            sense: if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
            enabled,
        }
    }

    fn test_fixture_panorama_logical(
        owner: &Arc<NativeUiSmokeOverlayOwner>,
        top_hover_activation: NativeUiSmokeTargetArea,
        observation: Option<NativeUiSmokeControlObservation>,
    ) -> NativeUiSmokeLogicalInventory {
        NativeUiSmokeLogicalInventory::new(
            owner,
            Some(top_hover_activation),
            observation,
            NativeUiSmokePanoramaClassification::NonPanorama,
            false,
            false,
            true,
            top_hover_activation.pixels_per_point,
            top_hover_activation.client_width,
            top_hover_activation.client_height,
        )
    }

    fn test_panorama_logical(
        owner: &Arc<NativeUiSmokeOverlayOwner>,
        observation: Option<NativeUiSmokeControlObservation>,
        classification: NativeUiSmokePanoramaClassification,
        allowed: bool,
    ) -> NativeUiSmokeLogicalInventory {
        NativeUiSmokeLogicalInventory::new(
            owner,
            Some(NativeUiSmokeTargetArea::new(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 36.0)),
                1.5,
                600,
                360,
            )),
            observation,
            classification,
            false,
            false,
            allowed,
            1.5,
            600,
            360,
        )
    }

    #[test]
    fn top_hover_target_is_independent_of_letterboxed_canvas_at_non_unit_dpi() {
        let canvas = geometry();
        let canvas_point = canvas.client_point([0.5, 0.5]).unwrap();
        let top = NativeUiSmokeTargetArea::new(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 36.0)),
            2.0,
            400,
            240,
        );
        let top_point = top.client_point([0.5, 0.5]).unwrap();
        assert_eq!([top_point.x, top_point.y], [200, 36]);
        assert!(top.contains_client_point([top_point.x, top_point.y]));
        assert!(!canvas.contains_client_point([top_point.x, top_point.y]));
        assert!(canvas.contains_client_point([canvas_point.x, canvas_point.y]));
    }

    #[test]
    fn top_hover_receipts_complete_outside_the_letterboxed_canvas() {
        let mut fixture = ReceiptFixture::new();
        let area = fixture
            .prepared
            .target
            .render
            .inventory
            .top_hover_activation
            .expect("top hover area");
        let point = area.client_point([0.5, 0.5]).unwrap();
        assert!(
            !fixture
                .prepared
                .target
                .render
                .geometry
                .contains_client_point([point.x, point.y])
        );
        fixture.prepared.target.kind = PreparedTargetKind::TopHoverActivation;
        fixture.prepared.target.area = area;
        fixture.prepared.target.client_point = point;
        fixture.pump.event_x = point.x;
        fixture.pump.event_y = point.y;
        fixture.render.event_x = point.x;
        fixture.render.event_y = point.y;

        fixture.begin();
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            let render = state
                .renders
                .get_mut(&fixture.prepared.target.render.publisher)
                .unwrap();
            render.inventory = NativeUiSmokeCommittedInventory::commit(
                Some(&render.inventory),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    area,
                    Some(test_fixture_panorama_observation(true)),
                ),
            );
        }
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        assert_eq!(fixture.completion().unwrap(), Some([point.x, point.y]));
        validate_prepared_target(
            &fixture.broker,
            &fixture.prepared,
            PreparedTargetValidationPhase::ReceiptCompletion,
        )
        .unwrap();
    }

    #[test]
    fn top_hover_hidden_to_shown_completes_receipts_and_observes_the_named_target() {
        let mut fixture = ReceiptFixture::new();
        let target = {
            let state = lock_broker_state(&fixture.broker).unwrap();
            coherent_top_hover_targets(&state, 0x100)
                .unwrap()
                .pop()
                .expect("hidden top-hover target")
        };
        assert!(target.ready_for_initial_prepare());
        assert!(target.render.inventory.native_top_panorama.is_none());
        let baseline_version = target.render.inventory.target_version;
        let area = target.area;
        let point = target.client_point;
        fixture.prepared = NativeUiSmokePreparedMove {
            target,
            normalized: [0.5, 0.5],
            coordinate_tolerance: [1, 1],
        };
        fixture.pump.event_x = point.x;
        fixture.pump.event_y = point.y;
        fixture.render.event_x = point.x;
        fixture.render.event_y = point.y;
        fixture.begin();

        // Match the production event tail: a successful present publishes the newly visible
        // inventory before the tagged render receipt is recorded.
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            let render = state
                .renders
                .get_mut(&fixture.prepared.target.render.publisher)
                .unwrap();
            render.inventory = NativeUiSmokeCommittedInventory::commit(
                Some(&render.inventory),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    area,
                    Some(test_fixture_panorama_observation(true)),
                ),
            );
            assert!(render.inventory.target_version > baseline_version);
        }
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        assert_eq!(fixture.completion().unwrap(), Some([point.x, point.y]));
        validate_prepared_target(
            &fixture.broker,
            &fixture.prepared,
            PreparedTargetValidationPhase::ReceiptCompletion,
        )
        .unwrap();

        let move_receipt = NativeUiSmokeMoveReceipt {
            token: fixture.metadata.token as u64,
            owner_hwnd: fixture.prepared.target.host.owner_hwnd,
            presenter_hwnd: fixture.prepared.target.render.presenter_hwnd,
            source_epoch: fixture.prepared.target.render.actual_source_epoch,
            generation: fixture.prepared.target.render.generation,
            requested_client_x: point.x,
            requested_client_y: point.y,
            actual_client_x: point.x,
            actual_client_y: point.y,
            host_publisher: fixture.prepared.target.host.publisher,
            render_publisher: fixture.prepared.target.render.publisher,
            overlay_owner_nonce: fixture.prepared.target.render.inventory.owner_nonce,
        };
        let state = lock_broker_state(&fixture.broker).unwrap();
        let named = named_control_after_move(&state, &move_receipt)
            .unwrap()
            .expect("enabled native_top_panorama after hover");
        assert!(named.enabled);
        assert!(named.senses_click);
        assert_eq!(named.owner_hwnd, move_receipt.owner_hwnd);
        assert_eq!(named.presenter_hwnd, move_receipt.presenter_hwnd);
        assert_eq!(named.source_epoch, move_receipt.source_epoch);
        assert_eq!(named.generation, move_receipt.generation);
    }

    #[test]
    fn top_hover_initial_prepare_waits_for_the_response_to_be_absent() {
        let fixture = ReceiptFixture::new();
        let area = fixture
            .prepared
            .target
            .render
            .inventory
            .top_hover_activation
            .unwrap();
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            let render = state
                .renders
                .get_mut(&fixture.prepared.target.render.publisher)
                .unwrap();
            render.inventory = NativeUiSmokeCommittedInventory::commit(
                Some(&render.inventory),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    area,
                    Some(test_fixture_panorama_observation(false)),
                ),
            );
            let candidates = coherent_top_hover_targets(&state, 0x100).unwrap();
            assert_eq!(candidates.len(), 1);
            assert!(!candidates[0].ready_for_initial_prepare());
        }
        let error = match wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::TopHoverActivation,
            Instant::now() + Duration::from_millis(5),
            &mut || Ok(()),
        ) {
            Ok(_) => panic!("a visible disabled Response is not a hidden hover baseline"),
            Err(error) => error,
        };
        assert!(error.contains("deadline expired"));
        assert!(error.contains("last_scan_candidate_count=1"));

        let second_output = NativeUiSmokeOutputId(194);
        let second_host = PublisherKey {
            output: second_output,
            nonce: 195,
        };
        let second_render = PublisherKey {
            output: second_output,
            nonce: 196,
        };
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            state.hosts.insert(
                second_host,
                HostSnapshot {
                    publisher: second_host,
                    request: 5,
                    epoch: 6,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    windows: HostWindowSet::from_contract(presenter_only(0x300, 6)),
                },
            );
            let hidden = test_inventory(&fixture._ui_smoke_owner);
            let second_area = hidden.top_hover_activation.unwrap();
            let visible = NativeUiSmokeCommittedInventory::commit(
                Some(&hidden),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    second_area,
                    Some(test_fixture_panorama_observation(false)),
                ),
            );
            state.renders.insert(
                second_render,
                RenderSnapshot {
                    publisher: second_render,
                    requested_source_epoch: Arc::downgrade(&fixture.requested),
                    actual_source_epoch: 7,
                    generation: 6,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    presenter_hwnd: 0x300,
                    geometry: geometry(),
                    geometry_version: 1,
                    inventory: visible,
                },
            );
        }
        let error = match wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::TopHoverActivation,
            Instant::now() + Duration::from_secs(1),
            &mut || panic!("ambiguous candidates must fail before owner validation"),
        ) {
            Ok(_) => panic!("multiple visible candidates must remain ambiguous"),
            Err(error) => error,
        };
        assert!(error.contains("native mouse target is ambiguous: 2"));
    }

    #[test]
    fn top_hover_before_input_rejects_a_show_hide_round_trip_to_the_same_area() {
        let mut fixture = ReceiptFixture::new();
        let target = {
            let state = lock_broker_state(&fixture.broker).unwrap();
            coherent_top_hover_targets(&state, 0x100)
                .unwrap()
                .pop()
                .unwrap()
        };
        let baseline_version = target.render.inventory.target_version;
        let area = target.area;
        fixture.prepared = NativeUiSmokePreparedMove {
            target,
            normalized: [0.5, 0.5],
            coordinate_tolerance: [1, 1],
        };
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            let render = state
                .renders
                .get_mut(&fixture.prepared.target.render.publisher)
                .unwrap();
            let shown = NativeUiSmokeCommittedInventory::commit(
                Some(&render.inventory),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    area,
                    Some(test_fixture_panorama_observation(true)),
                ),
            );
            render.inventory = NativeUiSmokeCommittedInventory::commit(
                Some(&shown),
                test_fixture_panorama_logical(&fixture._ui_smoke_owner, area, None),
            );
            assert!(render.inventory.native_top_panorama.is_none());
            assert!(render.inventory.target_version > baseline_version);
        }
        let error = validate_prepared_target(
            &fixture.broker,
            &fixture.prepared,
            PreparedTargetValidationPhase::BeforeInput,
        )
        .expect_err("an inventory round trip before SendInput must invalidate the preparation");
        assert!(error.contains("no longer the unique current target"));
    }

    #[test]
    fn top_hover_receipt_completion_rejects_a_hidden_inventory_without_advance() {
        let mut fixture = ReceiptFixture::new();
        let target = {
            let state = lock_broker_state(&fixture.broker).unwrap();
            coherent_top_hover_targets(&state, 0x100)
                .unwrap()
                .pop()
                .unwrap()
        };
        let point = target.client_point;
        fixture.prepared = NativeUiSmokePreparedMove {
            target,
            normalized: [0.5, 0.5],
            coordinate_tolerance: [1, 1],
        };
        fixture.pump.event_x = point.x;
        fixture.pump.event_y = point.y;
        fixture.render.event_x = point.x;
        fixture.render.event_y = point.y;
        fixture.begin();
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        let error = fixture
            .completion()
            .expect_err("a later unrelated tick must not complete this tagged hover receipt");
        assert!(error.contains("target changed while completing receipts"));
        assert!(
            validate_prepared_target(
                &fixture.broker,
                &fixture.prepared,
                PreparedTargetValidationPhase::ReceiptCompletion,
            )
            .is_err()
        );
    }

    #[test]
    fn canvas_completion_ignores_named_chrome_version_changes() {
        let fixture = ReceiptFixture::new();
        let area = fixture
            .prepared
            .target
            .render
            .inventory
            .top_hover_activation
            .unwrap();
        fixture.begin();
        {
            let mut state = lock_broker_state(&fixture.broker).unwrap();
            let render = state
                .renders
                .get_mut(&fixture.prepared.target.render.publisher)
                .unwrap();
            let baseline_version = render.inventory.target_version;
            render.inventory = NativeUiSmokeCommittedInventory::commit(
                Some(&render.inventory),
                test_fixture_panorama_logical(
                    &fixture._ui_smoke_owner,
                    area,
                    Some(test_fixture_panorama_observation(true)),
                ),
            );
            assert!(render.inventory.target_version > baseline_version);
        }
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        assert_eq!(
            fixture.completion().unwrap(),
            Some([fixture.pump.event_x, fixture.pump.event_y])
        );
        validate_prepared_target(
            &fixture.broker,
            &fixture.prepared,
            PreparedTargetValidationPhase::ReceiptCompletion,
        )
        .unwrap();
    }

    #[test]
    fn named_target_waits_for_enabled_click_response_and_rotates_after_hide() {
        let owner = allocate_overlay_owner();
        let disabled = NativeUiSmokeCommittedInventory::commit(
            None,
            test_panorama_logical(
                &owner,
                Some(test_panorama_observation(false)),
                NativeUiSmokePanoramaClassification::Unknown,
                true,
            ),
        );
        assert_eq!(
            disabled
                .native_top_panorama
                .as_ref()
                .and_then(|named| named.token),
            None
        );

        let enabled = NativeUiSmokeCommittedInventory::commit(
            Some(&disabled),
            test_panorama_logical(
                &owner,
                Some(test_panorama_observation(true)),
                NativeUiSmokePanoramaClassification::NonPanorama,
                true,
            ),
        );
        let first_token = enabled
            .native_top_panorama
            .as_ref()
            .and_then(|named| named.token)
            .unwrap();
        let named_area = enabled
            .native_top_panorama
            .as_ref()
            .expect("enabled named target")
            .area;
        let named_point = named_area.client_point([0.5, 0.5]).unwrap();
        assert_eq!([named_point.x, named_point.y], [441, 41]);
        assert!(named_area.contains_client_point([named_point.x, named_point.y]));
        let playback_tick = NativeUiSmokeCommittedInventory::commit(
            Some(&enabled),
            test_panorama_logical(
                &owner,
                Some(test_panorama_observation(true)),
                NativeUiSmokePanoramaClassification::NonPanorama,
                true,
            ),
        );
        assert_eq!(
            playback_tick
                .native_top_panorama
                .as_ref()
                .and_then(|named| named.token),
            Some(first_token)
        );
        assert_eq!(playback_tick.target_version, enabled.target_version);
        assert!(playback_tick.commit_serial > enabled.commit_serial);

        let hidden = NativeUiSmokeCommittedInventory::commit(
            Some(&playback_tick),
            test_panorama_logical(
                &owner,
                None,
                NativeUiSmokePanoramaClassification::NonPanorama,
                false,
            ),
        );
        let reappeared = NativeUiSmokeCommittedInventory::commit(
            Some(&hidden),
            test_panorama_logical(
                &owner,
                Some(test_panorama_observation(true)),
                NativeUiSmokePanoramaClassification::NonPanorama,
                true,
            ),
        );
        let second_token = reappeared
            .native_top_panorama
            .as_ref()
            .and_then(|named| named.token)
            .unwrap();
        assert_ne!(first_token, second_token);
        assert!(reappeared.target_version > playback_tick.target_version);
    }

    #[test]
    fn disabled_or_blocked_named_control_never_gets_a_target_token() {
        let owner = allocate_overlay_owner();
        for (enabled, allowed, classification) in [
            (false, true, NativeUiSmokePanoramaClassification::Unknown),
            (true, false, NativeUiSmokePanoramaClassification::Panorama),
            (
                true,
                false,
                NativeUiSmokePanoramaClassification::NonPanorama,
            ),
        ] {
            let committed = NativeUiSmokeCommittedInventory::commit(
                None,
                test_panorama_logical(
                    &owner,
                    Some(test_panorama_observation(enabled)),
                    classification,
                    allowed,
                ),
            );
            assert_eq!(
                committed
                    .native_top_panorama
                    .as_ref()
                    .and_then(|named| named.token),
                None
            );
        }
    }

    #[test]
    fn overlay_attempts_have_distinct_weak_owners_and_retired_ones_reject() {
        let failed_hud_owner = allocate_overlay_owner();
        let failed_inventory = test_inventory(&failed_hud_owner);
        let fallback_owner = allocate_overlay_owner();
        let fallback_inventory = test_inventory(&fallback_owner);
        assert_ne!(failed_inventory.owner_nonce, fallback_inventory.owner_nonce);
        drop(failed_hud_owner);
        assert!(!failed_inventory.owner_is_live());
        assert!(fallback_inventory.owner_is_live());
        let requested = Arc::new(AtomicU64::new(1));
        let publisher = NativeUiSmokeRenderPublisher::new(allocate_output_id(), &requested);
        assert!(
            publisher
                .publish(
                    1,
                    1,
                    NativeVideoPlacement::DetachedViewerChild,
                    0x100,
                    0x200,
                    geometry(),
                    failed_inventory,
                )
                .unwrap_err()
                .contains("owner already retired")
        );
    }

    #[test]
    fn source_switch_invalidation_removes_the_old_committed_join() {
        let requested = Arc::new(AtomicU64::new(3));
        let publisher = NativeUiSmokeRenderPublisher::new(allocate_output_id(), &requested);
        let owner = allocate_overlay_owner();
        let old_inventory = test_inventory(&owner);
        publisher
            .publish(
                3,
                9,
                NativeVideoPlacement::DetachedViewerChild,
                0x100,
                0x200,
                geometry(),
                old_inventory.clone(),
            )
            .unwrap();
        assert!(
            lock_broker_state(broker())
                .unwrap()
                .renders
                .contains_key(&publisher.key)
        );
        publisher.invalidate("source switched").unwrap();
        requested.store(4, Ordering::Release);
        assert!(
            !lock_broker_state(broker())
                .unwrap()
                .renders
                .contains_key(&publisher.key)
        );
        assert!(
            publisher
                .publish(
                    4,
                    9,
                    NativeVideoPlacement::DetachedViewerChild,
                    0x100,
                    0x200,
                    geometry(),
                    old_inventory,
                )
                .unwrap_err()
                .contains("predates the last source invalidation")
        );
        publisher
            .publish(
                4,
                9,
                NativeVideoPlacement::DetachedViewerChild,
                0x100,
                0x200,
                geometry(),
                test_inventory(&owner),
            )
            .unwrap();
    }

    struct ReceiptFixture {
        broker: Broker,
        requested: Arc<AtomicU64>,
        _ui_smoke_owner: Arc<NativeUiSmokeOverlayOwner>,
        prepared: NativeUiSmokePreparedMove,
        metadata: NativeUiSmokeMessageMetadata,
        pump: NativeUiSmokePumpReceipt,
        render: NativeUiSmokeRenderReceipt,
    }

    impl ReceiptFixture {
        fn new() -> Self {
            Self::with_source_epoch(7)
        }

        fn with_source_epoch(source_epoch: u64) -> Self {
            let broker = Broker::new();
            let requested = Arc::new(AtomicU64::new(source_epoch));
            let output = NativeUiSmokeOutputId(91);
            let host_key = PublisherKey { output, nonce: 92 };
            let render_key = PublisherKey { output, nonce: 93 };
            let windows = presenter_only(0x200, 4);
            let ui_smoke_owner = allocate_overlay_owner();
            {
                let mut state = lock_broker_state(&broker).unwrap();
                state.hosts.insert(
                    host_key,
                    HostSnapshot {
                        publisher: host_key,
                        request: 3,
                        epoch: 4,
                        placement: NativeVideoPlacement::DetachedViewerChild,
                        owner_hwnd: 0x100,
                        windows: HostWindowSet::from_contract(windows),
                    },
                );
                state.renders.insert(
                    render_key,
                    RenderSnapshot {
                        publisher: render_key,
                        requested_source_epoch: Arc::downgrade(&requested),
                        actual_source_epoch: source_epoch,
                        generation: 4,
                        placement: NativeVideoPlacement::DetachedViewerChild,
                        owner_hwnd: 0x100,
                        presenter_hwnd: 0x200,
                        geometry: geometry(),
                        geometry_version: 1,
                        inventory: test_inventory(&ui_smoke_owner),
                    },
                );
            }
            let target = {
                let state = lock_broker_state(&broker).unwrap();
                coherent_targets(&state, 0x100, [0.5, 0.5])
                    .unwrap()
                    .pop()
                    .unwrap()
            };
            let point = [target.client_point.x, target.client_point.y];
            Self {
                broker,
                requested,
                _ui_smoke_owner: ui_smoke_owner,
                prepared: NativeUiSmokePreparedMove {
                    target,
                    normalized: [0.5, 0.5],
                    coordinate_tolerance: [1, 1],
                },
                metadata: NativeUiSmokeMessageMetadata {
                    token: 0x1234,
                    receiver_hwnd: 0x200,
                },
                pump: NativeUiSmokePumpReceipt {
                    output,
                    epoch: 4,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    windows,
                    source: NativeVideoWindowSource::Presenter,
                    event_x: point[0],
                    event_y: point[1],
                },
                render: NativeUiSmokeRenderReceipt {
                    output,
                    actual_source_epoch: source_epoch,
                    generation: 4,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    presenter_hwnd: 0x200,
                    geometry: geometry(),
                    geometry_version: 1,
                    raw_forwarded: true,
                    command_count: 0,
                    source: NativeVideoWindowSource::Presenter,
                    event_x: point[0],
                    event_y: point[1],
                },
            }
        }

        fn begin(&self) {
            let mut state = lock_broker_state(&self.broker).unwrap();
            state.pending = Some(PendingStep {
                token: self.metadata.token,
                target: self.prepared.target.clone(),
                pump_actual_point: None,
                render_actual_point: None,
                coordinate_tolerance: self.prepared.coordinate_tolerance,
                failure: None,
            });
        }

        fn completion(&self) -> Result<Option<[i32; 2]>, String> {
            let state = lock_broker_state(&self.broker).unwrap();
            completed_actual_point(&state, self.metadata.token, &self.prepared)
        }
    }

    #[test]
    fn geometry_point_uses_the_published_pixels_per_point_once() {
        let point = geometry().client_point([0.25, 0.5]).unwrap();
        assert_eq!((point.x, point.y), (70, 100));
    }

    #[test]
    fn coherent_target_accepts_initial_zero_and_rejects_one_sided_advance() {
        let requested = Arc::new(AtomicU64::new(0));
        let ui_smoke_owner = allocate_overlay_owner();
        let output = NativeUiSmokeOutputId(1);
        let host_key = PublisherKey { output, nonce: 1 };
        let render_key = PublisherKey { output, nonce: 2 };
        let mut state = isolated_state();
        state.hosts.insert(
            host_key,
            HostSnapshot {
                publisher: host_key,
                request: 1,
                epoch: 4,
                placement: NativeVideoPlacement::DetachedViewerChild,
                owner_hwnd: 0x100,
                windows: HostWindowSet::from_contract(presenter_only(0x200, 4)),
            },
        );
        state.renders.insert(
            render_key,
            RenderSnapshot {
                publisher: render_key,
                requested_source_epoch: Arc::downgrade(&requested),
                actual_source_epoch: 0,
                generation: 4,
                placement: NativeVideoPlacement::DetachedViewerChild,
                owner_hwnd: 0x100,
                presenter_hwnd: 0x200,
                geometry: geometry(),
                geometry_version: 1,
                inventory: test_inventory(&ui_smoke_owner),
            },
        );
        assert_eq!(
            coherent_targets(&state, 0x100, [0.5, 0.5]).unwrap().len(),
            1
        );
        requested.store(1, Ordering::Release);
        assert!(
            coherent_targets(&state, 0x100, [0.5, 0.5])
                .unwrap()
                .is_empty()
        );
        requested.store(0, Ordering::Release);
        state
            .renders
            .get_mut(&render_key)
            .unwrap()
            .actual_source_epoch = 1;
        assert!(
            coherent_targets(&state, 0x100, [0.5, 0.5])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn target_wait_failure_keeps_phase_counts_and_pair_predicates() {
        let fixture = ReceiptFixture::with_source_epoch(0);
        fixture.requested.store(1, Ordering::Release);
        let error = match wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::Canvas,
            Instant::now() + Duration::from_secs(1),
            &mut || Err("fresh owner validation deadline".to_string()),
        ) {
            Ok(_) => panic!("a failed owner barrier must not return a target"),
            Err(error) => error,
        };
        assert!(error.contains("phase=after_candidate_scan"));
        assert!(error.contains("scan_count=1; last_scan_candidate_count=0"));
        assert!(error.contains("owner_validation_attempts=1; owner_validation_completed=0"));
        assert!(error.contains("requested_state=value(1),actual=0,source=Some(false)"));
        assert!(error.contains("geometry=Some(true),point=Some(true),coherent=false"));

        fixture.requested.store(0, Ordering::Release);
        let error = match wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::Canvas,
            Instant::now() + Duration::from_secs(1),
            &mut || Err("fresh owner validation deadline".to_string()),
        ) {
            Ok(_) => panic!("a failed owner barrier must not return a target"),
            Err(error) => error,
        };
        assert!(error.contains("scan_count=1; last_scan_candidate_count=1"));
        assert!(error.contains("requested_state=value(0),actual=0,source=Some(true)"));
        assert!(error.contains("geometry=Some(true),point=Some(true),coherent=true"));
    }

    #[test]
    fn target_wait_diagnostic_distinguishes_unchecked_and_expired_requested_source() {
        let fixture = ReceiptFixture::new();
        let mut state = lock_broker_state(&fixture.broker).unwrap();
        let host_output = state.hosts.values().next().unwrap().publisher.output;
        let render = state.renders.values_mut().next().unwrap();
        render.publisher.output = NativeUiSmokeOutputId(host_output.0 + 1);
        render.requested_source_epoch = Weak::new();

        let diagnostic = target_wait_diagnostic_from_state(&state, 0x100, [0.5, 0.5]);
        assert!(diagnostic.contains("requested_state=not_evaluated"));
        assert!(diagnostic.contains("source=None,geometry=None,point=None"));

        state.renders.values_mut().next().unwrap().publisher.output = host_output;
        let diagnostic = target_wait_diagnostic_from_state(&state, 0x100, [0.5, 0.5]);
        assert!(diagnostic.contains("requested_state=expired"));
        assert!(diagnostic.contains("source=Some(false),geometry=Some(true),point=Some(true)"));
    }

    #[test]
    fn owner_validation_error_keeps_the_static_callsite_phase() {
        let error = validate_owner_for_phase(
            &mut || Err("fresh owner validation failed".to_string()),
            "send_pending_immediately_before_send_input",
        )
        .expect_err("owner validation failure must remain terminal");
        assert!(error.contains("fresh owner validation failed"));
        assert!(
            error.contains("owner_validation_phase=send_pending_immediately_before_send_input")
        );
    }

    #[test]
    fn target_wait_diagnostic_bounds_host_render_and_pair_details() {
        let requested = Arc::new(AtomicU64::new(7));
        let ui_smoke_owner = allocate_overlay_owner();
        let mut state = isolated_state();
        for index in 0..6_u64 {
            let output = NativeUiSmokeOutputId(index + 1);
            let generation = index + 10;
            let presenter_hwnd = 0x200 + index;
            let host_key = PublisherKey {
                output,
                nonce: index + 20,
            };
            let render_key = PublisherKey {
                output,
                nonce: index + 30,
            };
            state.hosts.insert(
                host_key,
                HostSnapshot {
                    publisher: host_key,
                    request: index + 40,
                    epoch: generation,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    windows: HostWindowSet::from_contract(presenter_only(
                        presenter_hwnd,
                        generation,
                    )),
                },
            );
            state.renders.insert(
                render_key,
                RenderSnapshot {
                    publisher: render_key,
                    requested_source_epoch: Arc::downgrade(&requested),
                    actual_source_epoch: 7,
                    generation,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    presenter_hwnd,
                    geometry: geometry(),
                    geometry_version: 1,
                    inventory: test_inventory(&ui_smoke_owner),
                },
            );
        }

        let diagnostic = target_wait_diagnostic_from_state(&state, 0x100, [0.5, 0.5]);
        assert!(diagnostic.contains("hosts=6,renders=6,pair_space=36,pairs_sampled=8"));
        assert_eq!(diagnostic.matches("host[").count(), 4);
        assert_eq!(diagnostic.matches("render[").count(), 4);
        assert_eq!(diagnostic.matches("pair[h=").count(), 8);
        assert!(diagnostic.contains("hosts_omitted=2"));
        assert!(diagnostic.contains("renders_omitted=2"));
        assert!(diagnostic.contains("pairs_omitted=28"));
    }

    #[test]
    fn old_publisher_does_not_replace_a_new_live_candidate() {
        let requested = Arc::new(AtomicU64::new(7));
        let ui_smoke_owner = allocate_overlay_owner();
        let output_old = NativeUiSmokeOutputId(1);
        let output_new = NativeUiSmokeOutputId(2);
        let mut state = isolated_state();
        for (output, base) in [(output_old, 10), (output_new, 20)] {
            let host_key = PublisherKey {
                output,
                nonce: base,
            };
            let render_key = PublisherKey {
                output,
                nonce: base + 1,
            };
            state.hosts.insert(
                host_key,
                HostSnapshot {
                    publisher: host_key,
                    request: base,
                    epoch: 4,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    windows: HostWindowSet::from_contract(presenter_only(0x200 + base, 4)),
                },
            );
            state.renders.insert(
                render_key,
                RenderSnapshot {
                    publisher: render_key,
                    requested_source_epoch: Arc::downgrade(&requested),
                    actual_source_epoch: 7,
                    generation: 4,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    presenter_hwnd: 0x200 + base,
                    geometry: geometry(),
                    geometry_version: 1,
                    inventory: test_inventory(&ui_smoke_owner),
                },
            );
        }
        assert_eq!(
            coherent_targets(&state, 0x100, [0.5, 0.5]).unwrap().len(),
            2,
            "two live outputs for one parent must be rejected as ambiguous"
        );
        state.hosts.retain(|key, _| key.output != output_old);
        assert_eq!(
            coherent_targets(&state, 0x100, [0.5, 0.5]).unwrap().len(),
            1,
            "a late old render publication cannot replace the new output"
        );
    }

    #[test]
    fn same_geometry_keeps_version_while_a_resize_advances_it() {
        let requested = Arc::new(AtomicU64::new(3));
        let ui_smoke_owner = allocate_overlay_owner();
        let inventory = test_inventory(&ui_smoke_owner);
        let publisher = NativeUiSmokeRenderPublisher::new(
            NativeUiSmokeOutputId(NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed)),
            &requested,
        );
        let first = publisher
            .publish(
                3,
                9,
                NativeVideoPlacement::DetachedViewerChild,
                1,
                2,
                geometry(),
                inventory.clone(),
            )
            .unwrap();
        let same = publisher
            .publish(
                3,
                9,
                NativeVideoPlacement::DetachedViewerChild,
                1,
                2,
                geometry(),
                inventory.clone(),
            )
            .unwrap();
        let mut resized = geometry();
        resized.client_width += 1;
        let changed = publisher
            .publish(
                3,
                9,
                NativeVideoPlacement::DetachedViewerChild,
                1,
                2,
                resized,
                inventory,
            )
            .unwrap();
        assert_eq!(first, same);
        assert_eq!(changed, same + 1);
    }

    #[test]
    fn both_receipt_orders_require_the_second_route_before_completion() {
        for source_epoch in [0, 7] {
            for render_first in [false, true] {
                let fixture = ReceiptFixture::with_source_epoch(source_epoch);
                fixture.begin();
                if render_first {
                    record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
                } else {
                    record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
                }
                assert_eq!(fixture.completion().unwrap(), None);

                if render_first {
                    record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
                } else {
                    record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
                }
                assert_eq!(
                    fixture.completion().unwrap(),
                    Some([fixture.pump.event_x, fixture.pump.event_y])
                );
            }
        }
    }

    #[test]
    fn initial_zero_receipts_reject_a_requested_source_advance_mid_step() {
        for render_first in [false, true] {
            let fixture = ReceiptFixture::with_source_epoch(0);
            fixture.begin();
            if render_first {
                record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
            } else {
                record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
            }
            fixture.requested.store(1, Ordering::Release);
            if render_first {
                record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
            } else {
                record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
            }
            assert!(
                fixture.completion().is_err(),
                "a one-sided source advance must invalidate zero-epoch completion"
            );
        }
    }

    #[test]
    fn completion_rejects_source_geometry_or_overlay_owner_change_after_the_first_receipt() {
        for changed in ["requested", "actual", "geometry", "overlay_owner"] {
            let fixture = ReceiptFixture::new();
            let replacement_owner = allocate_overlay_owner();
            fixture.begin();
            record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
            match changed {
                "requested" => fixture.requested.store(8, Ordering::Release),
                "actual" => {
                    let mut state = lock_broker_state(&fixture.broker).unwrap();
                    state
                        .renders
                        .get_mut(&fixture.prepared.target.render.publisher)
                        .unwrap()
                        .actual_source_epoch = 8;
                }
                "geometry" => {
                    let mut state = lock_broker_state(&fixture.broker).unwrap();
                    let render = state
                        .renders
                        .get_mut(&fixture.prepared.target.render.publisher)
                        .unwrap();
                    render.geometry.client_width += 1;
                    render.geometry_version += 1;
                }
                "overlay_owner" => {
                    let mut state = lock_broker_state(&fixture.broker).unwrap();
                    state
                        .renders
                        .get_mut(&fixture.prepared.target.render.publisher)
                        .unwrap()
                        .inventory = test_inventory(&replacement_owner);
                }
                _ => unreachable!(),
            }
            record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
            assert!(
                fixture.completion().unwrap_err().contains("target changed"),
                "change kind {changed} must invalidate final completion"
            );
        }
    }

    #[test]
    fn publisher_retirement_is_scoped_and_new_same_owner_candidate_is_ambiguous() {
        let current = ReceiptFixture::new();
        current.begin();
        retire_host_publisher_in(&current.broker, current.prepared.target.host.publisher).unwrap();
        assert!(
            current
                .completion()
                .unwrap_err()
                .contains("publisher retired")
        );

        let unrelated = ReceiptFixture::new();
        unrelated.begin();
        let unrelated_key = PublisherKey {
            output: NativeUiSmokeOutputId(700),
            nonce: 701,
        };
        {
            let mut state = lock_broker_state(&unrelated.broker).unwrap();
            state.hosts.insert(
                unrelated_key,
                HostSnapshot {
                    publisher: unrelated_key,
                    request: 9,
                    epoch: 10,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x900,
                    windows: HostWindowSet::from_contract(presenter_only(0xa00, 10)),
                },
            );
        }
        retire_host_publisher_in(&unrelated.broker, unrelated_key).unwrap();
        record_pump_receipt_in(&unrelated.broker, unrelated.metadata, unrelated.pump);
        record_render_receipt_in(&unrelated.broker, unrelated.metadata, unrelated.render);
        assert!(unrelated.completion().unwrap().is_some());

        let ambiguous = ReceiptFixture::new();
        ambiguous.begin();
        let output = NativeUiSmokeOutputId(800);
        let host_key = PublisherKey { output, nonce: 801 };
        let render_key = PublisherKey { output, nonce: 802 };
        {
            let mut state = lock_broker_state(&ambiguous.broker).unwrap();
            state.hosts.insert(
                host_key,
                HostSnapshot {
                    publisher: host_key,
                    request: 11,
                    epoch: 12,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    windows: HostWindowSet::from_contract(presenter_only(0xb00, 12)),
                },
            );
            state.renders.insert(
                render_key,
                RenderSnapshot {
                    publisher: render_key,
                    requested_source_epoch: Arc::downgrade(&ambiguous.requested),
                    actual_source_epoch: 7,
                    generation: 12,
                    placement: NativeVideoPlacement::DetachedViewerChild,
                    owner_hwnd: 0x100,
                    presenter_hwnd: 0xb00,
                    geometry: geometry(),
                    geometry_version: 1,
                    inventory: test_inventory(&ambiguous._ui_smoke_owner),
                },
            );
        }
        record_pump_receipt_in(&ambiguous.broker, ambiguous.metadata, ambiguous.pump);
        record_render_receipt_in(&ambiguous.broker, ambiguous.metadata, ambiguous.render);
        assert!(
            ambiguous
                .completion()
                .unwrap_err()
                .contains("target changed")
        );
    }

    #[test]
    fn receipt_identity_failure_is_sticky_when_a_matching_receipt_arrives_later() {
        let fixture = ReceiptFixture::new();
        fixture.begin();
        let mut wrong_metadata = fixture.metadata;
        wrong_metadata.receiver_hwnd += 1;
        record_pump_receipt_in(&fixture.broker, wrong_metadata, fixture.pump);
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        assert!(fixture.completion().unwrap_err().contains("pump receipt"));

        record_render_failure_in(
            &fixture.broker,
            fixture.metadata,
            "later render error must not replace the first failure".into(),
        );
        assert!(fixture.completion().unwrap_err().contains("pump receipt"));
    }

    #[test]
    fn render_failure_remains_terminal_after_both_matching_receipts() {
        let fixture = ReceiptFixture::new();
        fixture.begin();
        record_render_failure_in(
            &fixture.broker,
            fixture.metadata,
            "overlay handler rejected the tagged batch".into(),
        );
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        assert!(
            fixture
                .completion()
                .unwrap_err()
                .contains("overlay handler rejected")
        );
    }

    #[test]
    fn actual_point_and_presenter_source_are_required_on_both_routes() {
        let wrong_point = ReceiptFixture::new();
        wrong_point.begin();
        let mut pump = wrong_point.pump;
        pump.event_x += 20;
        record_pump_receipt_in(&wrong_point.broker, wrong_point.metadata, pump);
        assert!(
            wrong_point
                .completion()
                .unwrap_err()
                .contains("pump receipt")
        );

        let wrong_source = ReceiptFixture::new();
        wrong_source.begin();
        let mut render = wrong_source.render;
        render.source = NativeVideoWindowSource::Hud;
        record_render_receipt_in(&wrong_source.broker, wrong_source.metadata, render);
        assert!(
            wrong_source
                .completion()
                .unwrap_err()
                .contains("render receipt")
        );

        let disagreement = ReceiptFixture::new();
        disagreement.begin();
        record_pump_receipt_in(
            &disagreement.broker,
            disagreement.metadata,
            disagreement.pump,
        );
        let mut render = disagreement.render;
        render.event_x += 1;
        record_render_receipt_in(&disagreement.broker, disagreement.metadata, render);
        assert!(
            disagreement
                .completion()
                .unwrap_err()
                .contains("render receipt")
        );
    }

    #[test]
    fn pending_operation_runs_without_the_broker_lock_and_cleans_up_on_error() {
        let fixture = ReceiptFixture::new();
        let mut validation_calls = 0;
        let target = wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::Canvas,
            Instant::now() + Duration::from_millis(20),
            &mut || {
                let guard = fixture
                    .broker
                    .state
                    .try_lock()
                    .map_err(|_| "owner validation ran under the broker lock".to_string())?;
                drop(guard);
                validation_calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            target.host.publisher,
            fixture.prepared.target.host.publisher
        );
        assert!(validation_calls > 0);

        fixture.begin();
        let result: Result<(), String> =
            run_pending_step(&fixture.broker, fixture.metadata.token, || {
                let guard = fixture
                    .broker
                    .state
                    .try_lock()
                    .map_err(|_| "owner validation ran under the broker lock".to_string())?;
                drop(guard);
                Err("owner validation interrupted the step".into())
            });
        assert!(result.unwrap_err().contains("interrupted"));
        assert!(
            lock_broker_state(&fixture.broker)
                .unwrap()
                .pending
                .is_none()
        );
    }

    #[test]
    fn ready_target_cannot_be_returned_after_the_absolute_deadline() {
        let fixture = ReceiptFixture::new();
        let result = wait_for_target(
            &fixture.broker,
            0x100,
            [0.5, 0.5],
            PreparedTargetKind::Canvas,
            Instant::now() + Duration::from_millis(1),
            &mut || {
                std::thread::sleep(Duration::from_millis(5));
                Ok(())
            },
        );
        let error = match result {
            Ok(_) => panic!("a ready candidate must still respect the shared deadline"),
            Err(error) => error,
        };
        assert!(error.contains("deadline expired"));
    }

    #[test]
    fn complete_receipts_cannot_be_returned_after_the_absolute_deadline() {
        let fixture = ReceiptFixture::new();
        fixture.begin();
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        record_render_receipt_in(&fixture.broker, fixture.metadata, fixture.render);
        let error = wait_for_receipts(
            &fixture.broker,
            fixture.metadata.token,
            &fixture.prepared,
            Instant::now() + Duration::from_millis(1),
            &mut || {
                std::thread::sleep(Duration::from_millis(5));
                Ok(())
            },
        )
        .expect_err("complete receipts must still respect the shared deadline");
        assert!(error.contains("deadline expired"));
    }

    #[test]
    fn receipt_owner_failure_keeps_pending_route_evidence_before_cleanup() {
        let fixture = ReceiptFixture::new();
        fixture.begin();
        record_pump_receipt_in(&fixture.broker, fixture.metadata, fixture.pump);
        let error = wait_for_receipts(
            &fixture.broker,
            fixture.metadata.token,
            &fixture.prepared,
            Instant::now() + Duration::from_secs(1),
            &mut || {
                let mut state = lock_broker_state(&fixture.broker)?;
                state.pending.as_mut().unwrap().failure =
                    Some("render failed while owner callback ran".to_string());
                Err("fresh owner validation failed".to_string())
            },
        )
        .expect_err("owner failure must stop receipt completion");
        assert!(error.contains("owner_validation_phase=receipt_wait_after_state_scan"));
        assert!(error.contains("expected_token=0x1234,pending_token=0x1234"));
        assert!(error.contains("requested=(120,100),tolerance=(1,1)"));
        assert!(error.contains("pump_actual=(120,100),render_actual=none"));
        assert!(error.contains("failure=render failed while owner callback ran"));
        assert!(
            lock_broker_state(&fixture.broker)
                .unwrap()
                .pending
                .is_some()
        );
    }

    #[test]
    fn step_token_allocator_stays_in_the_32_bit_prefix_range_and_exhausts_permanently() {
        let first_serial = AtomicU32::new(STEP_TOKEN_SERIAL_MIN);
        let first = allocate_step_token_in(&first_serial).unwrap();
        assert_eq!(first, STEP_TOKEN_PREFIX | 1);
        assert_eq!(first_serial.load(Ordering::Relaxed), 2);
        assert_eq!(first & !(STEP_TOKEN_SERIAL_MAX as usize), STEP_TOKEN_PREFIX);
        assert!(u32::try_from(first).is_ok());

        let last_serial = AtomicU32::new(STEP_TOKEN_SERIAL_MAX);
        let last = allocate_step_token_in(&last_serial).unwrap();
        assert_eq!(last, STEP_TOKEN_PREFIX | STEP_TOKEN_SERIAL_MAX as usize);
        assert_eq!(
            last_serial.load(Ordering::Relaxed),
            STEP_TOKEN_SERIAL_EXHAUSTED
        );
        let first_error = allocate_step_token_in(&last_serial).unwrap_err();
        let second_error = allocate_step_token_in(&last_serial).unwrap_err();
        assert!(first_error.contains("exhausted"));
        assert_eq!(second_error, first_error);
        assert_eq!(
            last_serial.load(Ordering::Relaxed),
            STEP_TOKEN_SERIAL_EXHAUSTED
        );

        let invalid_zero = AtomicU32::new(0);
        assert!(allocate_step_token_in(&invalid_zero).is_err());
        assert_eq!(invalid_zero.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn concurrent_step_token_allocation_is_unique_and_in_range() {
        const THREADS: usize = 8;
        const TOKENS_PER_THREAD: usize = 128;
        let next_serial = Arc::new(AtomicU32::new(STEP_TOKEN_SERIAL_MIN));
        let handles = (0..THREADS)
            .map(|_| {
                let next_serial = Arc::clone(&next_serial);
                std::thread::spawn(move || {
                    (0..TOKENS_PER_THREAD)
                        .map(|_| allocate_step_token_in(&next_serial).unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let mut tokens = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(tokens.len(), THREADS * TOKENS_PER_THREAD);
        assert!(tokens.iter().all(|token| {
            *token & !(STEP_TOKEN_SERIAL_MAX as usize) == STEP_TOKEN_PREFIX
                && (*token & STEP_TOKEN_SERIAL_MAX as usize) != 0
                && u32::try_from(*token).is_ok()
        }));
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), THREADS * TOKENS_PER_THREAD);
    }

    #[test]
    fn failed_active_step_cas_burns_the_allocated_serial() {
        let next_serial = AtomicU32::new(STEP_TOKEN_SERIAL_MIN);
        let occupied = STEP_TOKEN_PREFIX | 99;
        let active = AtomicUsize::new(occupied);

        let error = begin_active_step_token_in(&next_serial, &active).unwrap_err();
        assert!(error.contains("already active"));
        assert_eq!(active.load(Ordering::Acquire), occupied);
        assert_eq!(next_serial.load(Ordering::Relaxed), 2);

        active.store(0, Ordering::Release);
        let next = begin_active_step_token_in(&next_serial, &active).unwrap();
        assert_eq!(next, STEP_TOKEN_PREFIX | 2);
        assert_ne!(next, STEP_TOKEN_PREFIX | 1);
        assert_eq!(active.load(Ordering::Acquire), next);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn truncated_low_32_bits_do_not_match_an_old_large_token() {
        let old_large_token = 0x4d49_5653_0000_0001usize;
        let truncated_extra_info = old_large_token as u32 as usize;
        assert_eq!(truncated_extra_info, 1);
        assert!(
            message_metadata_from_values(old_large_token, truncated_extra_info, 0x200).is_none()
        );
    }

    #[test]
    fn an_old_packet_cannot_complete_the_next_step() {
        let mut fixture = ReceiptFixture::new();
        let old_token = STEP_TOKEN_PREFIX | 1;
        let current_token = STEP_TOKEN_PREFIX | 2;
        fixture.metadata.token = current_token;
        fixture.begin();

        let old_metadata = NativeUiSmokeMessageMetadata {
            token: old_token,
            receiver_hwnd: fixture.metadata.receiver_hwnd,
        };
        record_pump_receipt_in(&fixture.broker, old_metadata, fixture.pump);
        record_render_receipt_in(&fixture.broker, old_metadata, fixture.render);

        let state = lock_broker_state(&fixture.broker).unwrap();
        let pending = state.pending.as_ref().unwrap();
        assert_eq!(pending.token, current_token);
        assert_eq!(pending.pump_actual_point, None);
        assert_eq!(pending.render_actual_point, None);
        assert_eq!(pending.failure, None);
        drop(state);
        assert_eq!(fixture.completion().unwrap(), None);
        assert!(message_metadata_from_values(current_token, old_token, 0x200).is_none());
    }

    #[test]
    fn wndproc_trace_preserves_entry_and_match_extra_info_without_weakening_the_token_match() {
        let token = STEP_TOKEN_PREFIX | 1;
        let trace = Mutex::new(WndProcTraceState::default());
        let active = AtomicUsize::new(token);
        let unavailable = AtomicUsize::new(0);
        begin_wndproc_trace_in(&trace, &unavailable, token);
        record_wndproc_trace_in(
            &trace,
            &active,
            &unavailable,
            NativeUiSmokeMessageEntry {
                active_token: token,
                extra_info: token,
            },
            token,
            1,
            0x200,
            504,
            432,
        );

        let snapshot = snapshot_wndproc_trace_in(&trace, token, || active.load(Ordering::Acquire));
        let WndProcTraceSnapshot::Available {
            len,
            overflow,
            records,
        } = snapshot
        else {
            panic!("trace was not available: {snapshot:?}");
        };
        assert_eq!(len, 1);
        assert_eq!(overflow, 0);
        assert_eq!(
            records[0],
            Some(WndProcTraceRecord {
                entry_active_token: token,
                match_active_token: token,
                receiver_hwnd: 0x200,
                event_x: 504,
                event_y: 432,
                entry_extra_info: token,
                match_extra_info: 1,
            })
        );
        assert_eq!(unavailable.load(Ordering::Relaxed), 0);
        assert!(message_metadata_from_values(token, token, 0x200).is_some());
        assert!(message_metadata_from_values(token, 1, 0x200).is_none());
        assert!(message_metadata_from_values(0, 0, 0x200).is_none());
    }

    #[test]
    fn wndproc_trace_is_bounded_and_old_steps_are_not_visible_after_active_owner_changes() {
        let token = 0x1234;
        let trace = Mutex::new(WndProcTraceState::default());
        let active = AtomicUsize::new(token);
        let unavailable = AtomicUsize::new(0);
        begin_wndproc_trace_in(&trace, &unavailable, token);
        for x in 0..(WNDPROC_TRACE_RECORD_LIMIT + 2) {
            record_wndproc_trace_in(
                &trace,
                &active,
                &unavailable,
                NativeUiSmokeMessageEntry {
                    active_token: token,
                    extra_info: token,
                },
                token,
                token,
                0x200,
                x as i32,
                20,
            );
        }
        let snapshot = snapshot_wndproc_trace_in(&trace, token, || active.load(Ordering::Acquire));
        assert!(matches!(
            snapshot,
            WndProcTraceSnapshot::Available {
                len: WNDPROC_TRACE_RECORD_LIMIT,
                overflow: 2,
                ..
            }
        ));

        active.store(0, Ordering::Release);
        assert_eq!(
            snapshot_wndproc_trace_in(&trace, token, || active.load(Ordering::Acquire)),
            WndProcTraceSnapshot::Inactive
        );

        let next_token = 0x1235;
        active.store(next_token, Ordering::Release);
        begin_wndproc_trace_in(&trace, &unavailable, next_token);
        record_wndproc_trace_in(
            &trace,
            &active,
            &unavailable,
            NativeUiSmokeMessageEntry {
                active_token: token,
                extra_info: token,
            },
            token,
            token,
            0x200,
            99,
            20,
        );
        assert!(matches!(
            snapshot_wndproc_trace_in(&trace, next_token, || active.load(Ordering::Acquire)),
            WndProcTraceSnapshot::Available {
                len: 0,
                overflow: 0,
                ..
            }
        ));
    }

    #[test]
    fn wndproc_trace_reports_nonblocking_contention_and_poison_as_unavailable() {
        let token = 0x1234;
        let trace = Mutex::new(WndProcTraceState {
            owner_token: token,
            ..WndProcTraceState::default()
        });
        let active = AtomicUsize::new(token);
        let unavailable = AtomicUsize::new(0);
        let guard = trace.lock().unwrap();
        assert_eq!(
            snapshot_wndproc_trace_in(&trace, token, || active.load(Ordering::Acquire)),
            WndProcTraceSnapshot::Busy
        );
        begin_wndproc_trace_in(&trace, &unavailable, token + 1);
        record_wndproc_trace_in(
            &trace,
            &active,
            &unavailable,
            NativeUiSmokeMessageEntry {
                active_token: token,
                extra_info: token,
            },
            token,
            token,
            0x200,
            1,
            2,
        );
        assert_eq!(unavailable.load(Ordering::Relaxed), 2);
        drop(guard);

        let poisoned = Arc::new(Mutex::new(WndProcTraceState::default()));
        let poisoner = Arc::clone(&poisoned);
        assert!(
            std::thread::spawn(move || {
                let _guard = poisoner.lock().unwrap();
                panic!("poison local wndproc trace");
            })
            .join()
            .is_err()
        );
        assert_eq!(
            snapshot_wndproc_trace_in(&poisoned, token, || token),
            WndProcTraceSnapshot::Poisoned
        );
    }

    #[test]
    fn a_poisoned_local_broker_faults_only_that_broker() {
        let poisoned = Arc::new(Broker::new());
        let poisoner = Arc::clone(&poisoned);
        assert!(
            std::thread::spawn(move || {
                let _guard = poisoner.state.lock().unwrap();
                panic!("poison local native UI smoke broker");
            })
            .join()
            .is_err()
        );
        let poison_error = match lock_broker_state(&poisoned) {
            Ok(_) => panic!("poisoned broker lock unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(poison_error.contains("poisoned"));
        assert!(poisoned.faulted.load(Ordering::Acquire));

        let healthy = Broker::new();
        assert!(lock_broker_state(&healthy).is_ok());
    }
}
