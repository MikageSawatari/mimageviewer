//! Native video HWND ownership boundary.
//!
//! `NativeWindowHost` owns the presenter and optional HUD HWNDs and is pinned to
//! the thread that created them.  Render code receives only `NativeRenderTargets`,
//! an opaque, non-owning target-binding lease with no USER32 lifecycle methods.

use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::ThreadId;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionTarget};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
    ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorPos, GetWindowRect, IDC_ARROW, IDC_HAND, IDC_IBEAM, IDC_NO,
    IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IDC_WAIT, LoadCursorW, SWP_NOACTIVATE, SWP_NOZORDER,
    SetCursor,
};

use super::NativeVideoPlacement;
use super::native_diagnostics::{DiagnosticOutputId, DiagnosticSample, NativeDiagnostics};
use super::native_window::{
    ForegroundClaimReport, NativeVideoWindow, NativeVideoWindowConfig, NativeVideoWindowEventSink,
    NativeVideoWindowSource,
};
use super::window_host_contract::{
    HostWindowTopology, HostWindows, OpaqueWindowHandle, OpaqueWindowId, WindowGeneration,
};

mod hud_window;

fn diagnostic_rect(rect: RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

pub(crate) fn hash_hud_regions(regions: &[RECT]) -> u64 {
    hud_window::hash_regions_for_debug(regions)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NativeHudWindowRequest {
    Disabled,
    Enabled { width: u32, height: u32 },
}

pub(crate) struct NativeWindowHostConfig {
    pub(crate) window: NativeVideoWindowConfig,
    pub(crate) hud: NativeHudWindowRequest,
    pub(crate) event_sink: NativeVideoWindowEventSink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeRenderTargetRole {
    Presenter,
    Hud,
}

/// Opaque, non-owning target-binding lease for the render core.
///
/// The raw HWND and its owner are private.  This type intentionally has no
/// show/hide/move/destroy/z-order methods; those operations remain on
/// `NativeWindowHost` and therefore on its creation thread.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeRenderTarget {
    hwnd: HWND,
    generation: u64,
    role: NativeRenderTargetRole,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NativeRenderTargets {
    PresenterOnly {
        presenter: NativeRenderTarget,
    },
    PresenterAndHud {
        presenter: NativeRenderTarget,
        hud: NativeRenderTarget,
    },
}

#[derive(Debug)]
pub(crate) struct NativeRenderTargetTransfer {
    presenter: (u64, u64),
    hud: Option<(u64, u64)>,
}

impl NativeRenderTargetTransfer {
    pub(crate) fn into_targets(self) -> NativeRenderTargets {
        let presenter = NativeRenderTarget::new(
            HWND(self.presenter.0 as usize as *mut _),
            self.presenter.1,
            NativeRenderTargetRole::Presenter,
        );
        if let Some((raw, generation)) = self.hud {
            NativeRenderTargets::PresenterAndHud {
                presenter,
                hud: NativeRenderTarget::new(
                    HWND(raw as usize as *mut _),
                    generation,
                    NativeRenderTargetRole::Hud,
                ),
            }
        } else {
            NativeRenderTargets::PresenterOnly { presenter }
        }
    }
}

impl NativeRenderTargets {
    pub(crate) fn presenter(self) -> NativeRenderTarget {
        let presenter = match self {
            Self::PresenterOnly { presenter } | Self::PresenterAndHud { presenter, .. } => {
                presenter
            }
        };
        debug_assert_eq!(presenter.role, NativeRenderTargetRole::Presenter);
        presenter
    }

    pub(crate) fn hud(self) -> Option<NativeRenderTarget> {
        let hud = match self {
            Self::PresenterOnly { .. } => None,
            Self::PresenterAndHud { hud, .. } => Some(hud),
        };
        debug_assert!(hud.is_none_or(|target| target.role == NativeRenderTargetRole::Hud));
        hud
    }

    pub(crate) fn topology(self) -> HostWindowTopology {
        match self {
            Self::PresenterOnly { .. } => HostWindowTopology::PresenterOnly,
            Self::PresenterAndHud { .. } => HostWindowTopology::PresenterAndHud,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCursorIcon {
    Hidden,
    Arrow,
    Hand,
    Text,
    ResizeHorizontal,
    ResizeVertical,
    Move,
    NotAllowed,
    Wait,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum NativeWindowIntent {
    ClaimTextInputFocus,
    UpdateImeCursorArea {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    SetCursorPolicy {
        icon: NativeCursorIcon,
        auto_hide_allowed: bool,
    },
}

fn native_ime_window_forms(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> (COMPOSITIONFORM, CANDIDATEFORM) {
    let area = RECT {
        left: x,
        top: y,
        right: x + width.max(1),
        bottom: y + height.max(1),
    };
    let candidate_form = CANDIDATEFORM {
        dwIndex: 0,
        dwStyle: windows::Win32::UI::Input::Ime::CFS_EXCLUDE,
        ptCurrentPos: POINT { x, y },
        rcArea: area,
    };
    let composition_form = COMPOSITIONFORM {
        dwStyle: windows::Win32::UI::Input::Ime::CFS_POINT,
        ptCurrentPos: POINT { x, y: area.bottom },
        rcArea: area,
    };
    (composition_form, candidate_form)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeFocusState {
    pub(crate) target_id: u64,
    pub(crate) thread_focus_id: u64,
    pub(crate) foreground_is_current_process: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeWindowObservation {
    pub(crate) cursor_client_position: Option<[i32; 2]>,
    pub(crate) cursor_input_owned: bool,
    pub(crate) cursor_hidden: bool,
    pub(crate) cursor_last_activity: Option<std::time::Instant>,
    pub(crate) focus: NativeFocusState,
    pub(crate) global_lbutton_down: bool,
    pub(crate) has_hud: bool,
    pub(crate) hud_has_capture: bool,
}

impl Default for NativeWindowObservation {
    fn default() -> Self {
        Self {
            cursor_client_position: None,
            cursor_input_owned: false,
            cursor_hidden: false,
            cursor_last_activity: None,
            focus: NativeFocusState {
                target_id: 0,
                thread_focus_id: 0,
                foreground_is_current_process: false,
            },
            global_lbutton_down: false,
            has_hud: false,
            hud_has_capture: false,
        }
    }
}

impl NativeRenderTarget {
    fn new(hwnd: HWND, generation: u64, role: NativeRenderTargetRole) -> Self {
        Self {
            hwnd,
            generation,
            role,
        }
    }

    pub(crate) fn generation(self) -> u64 {
        self.generation
    }

    pub(crate) fn create_dcomp_target(
        self,
        device: &IDCompositionDevice,
    ) -> windows::core::Result<IDCompositionTarget> {
        unsafe { device.CreateTargetForHwnd(self.hwnd, true) }
    }
}

#[derive(Clone, Copy, Debug)]
struct WindowThreadAffinity {
    owner: ThreadId,
}

impl WindowThreadAffinity {
    fn current() -> Self {
        Self {
            owner: std::thread::current().id(),
        }
    }

    #[track_caller]
    fn assert_current(self) {
        assert_eq!(
            std::thread::current().id(),
            self.owner,
            "native video HWND operation ran on a non-owner thread"
        );
    }
}

enum NativeOwnedWindows {
    PresenterOnly {
        presenter: NativeVideoWindow,
    },
    PresenterAndHud {
        presenter: NativeVideoWindow,
        hud: hud_window::HudOverlayWindow,
        regions: Arc<std::sync::Mutex<hud_window::HudInteractiveRegions>>,
    },
}

/// Creation-thread-bound owner for the presenter and HUD HWND topology.
pub(crate) struct NativeWindowHost {
    windows: NativeOwnedWindows,
    affinity: WindowThreadAffinity,
    editor_ui_snapshot: Option<super::dsp::SharedEditorUiSnapshot>,
    main_hwnd_for_raise: u64,
    last_logged_region_hash: Option<u64>,
    last_region_hash: Option<u64>,
    last_regions_empty: bool,
    diagnostics: NativeDiagnostics,
    #[cfg(test)]
    focus_claim_probe: Option<Arc<std::sync::atomic::AtomicUsize>>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl NativeWindowHost {
    /// Production HWNDs without either ShowWindow path. Used only by pump
    /// handler regressions; no DComp targets or application profile are opened.
    #[cfg(test)]
    pub(crate) fn create_hidden_for_geometry_test(
        epoch: u64,
        presenter_sink: NativeVideoWindowEventSink,
        hud_sink: NativeVideoWindowEventSink,
        with_hud: bool,
    ) -> Self {
        let rect = RECT {
            left: -2000,
            top: -2000,
            right: -1680,
            bottom: -1820,
        };
        let presenter = NativeVideoWindow::create(NativeVideoWindowConfig {
            mode: super::native_window::NativeVideoWindowMode::Borderless { rect },
            owner_hwnd: 0,
            initially_visible: false,
            activate_on_show: false,
            close_on_escape: false,
            event_sink: Some(presenter_sink),
            generation: epoch,
        })
        .expect("hidden production presenter");
        let windows = if with_hud {
            let regions = Arc::new(std::sync::Mutex::new(
                hud_window::HudInteractiveRegions::default(),
            ));
            let hud = hud_window::HudOverlayWindow::create_hidden_for_test(
                hud_window::HudOverlayConfig {
                    owner_hwnd: presenter.hwnd(),
                    x: rect.left,
                    y: rect.top,
                    width: 320,
                    height: 180,
                    event_sink: hud_sink,
                    regions: Arc::clone(&regions),
                },
            )
            .expect("hidden production HUD");
            NativeOwnedWindows::PresenterAndHud {
                presenter,
                hud,
                regions,
            }
        } else {
            NativeOwnedWindows::PresenterOnly { presenter }
        };
        Self {
            windows,
            affinity: WindowThreadAffinity::current(),
            editor_ui_snapshot: None,
            main_hwnd_for_raise: 0,
            last_logged_region_hash: None,
            last_region_hash: None,
            last_regions_empty: true,
            diagnostics: NativeDiagnostics::new(
                DiagnosticOutputId::default(),
                epoch,
                "hud",
                "pump",
            ),
            focus_claim_probe: None,
            _not_send_or_sync: PhantomData,
        }
    }

    #[cfg(test)]
    pub(crate) fn hud_geometry_attempts_for_test(&self) -> usize {
        match &self.windows {
            NativeOwnedWindows::PresenterAndHud { hud, .. } => hud.geometry_probe_for_test(),
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn hud_position_notifications_for_test(&self) -> usize {
        match &self.windows {
            NativeOwnedWindows::PresenterAndHud { hud, .. } => {
                hud.position_notifications_for_test()
            }
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_hud_geometry_failure_for_test(&self, query: bool, apply: bool) {
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            hud.inject_geometry_failure_for_test(query, apply);
        }
    }

    pub(crate) fn create(config: NativeWindowHostConfig) -> Result<Self, String> {
        let affinity = WindowThreadAffinity::current();
        let generation = config.window.generation;
        let presenter = NativeVideoWindow::create(config.window)?;
        let windows = match config.hud {
            NativeHudWindowRequest::Disabled => NativeOwnedWindows::PresenterOnly { presenter },
            NativeHudWindowRequest::Enabled { width, height } => {
                let regions = Arc::new(std::sync::Mutex::new(
                    hud_window::HudInteractiveRegions::default(),
                ));
                let presenter_hwnd = presenter.hwnd();
                let (x, y) = unsafe {
                    let mut rect = RECT::default();
                    if GetWindowRect(presenter_hwnd, &mut rect).is_ok() {
                        (rect.left, rect.top)
                    } else {
                        (0, 0)
                    }
                };
                let hud_config = hud_window::HudOverlayConfig {
                    owner_hwnd: presenter_hwnd,
                    x,
                    y,
                    width,
                    height,
                    event_sink: config.event_sink,
                    regions: Arc::clone(&regions),
                };
                match hud_window::HudOverlayWindow::create(hud_config) {
                    Ok(hud) => NativeOwnedWindows::PresenterAndHud {
                        presenter,
                        hud,
                        regions,
                    },
                    Err(error) => {
                        crate::logger::log(format!(
                            "native-presenter: HUD overlay HWND creation failed, fallback: {error}"
                        ));
                        NativeOwnedWindows::PresenterOnly { presenter }
                    }
                }
            }
        };
        let this = Self {
            windows,
            affinity,
            editor_ui_snapshot: None,
            main_hwnd_for_raise: 0,
            last_logged_region_hash: None,
            last_region_hash: None,
            last_regions_empty: true,
            diagnostics: NativeDiagnostics::new(
                DiagnosticOutputId::default(),
                generation,
                "hud",
                "pump",
            ),
            #[cfg(test)]
            focus_claim_probe: None,
            _not_send_or_sync: PhantomData,
        };
        debug_assert_eq!(this.render_targets().presenter().generation(), generation);
        Ok(this)
    }

    #[track_caller]
    fn assert_owner_thread(&self) {
        self.affinity.assert_current();
    }

    pub(crate) fn bind_diagnostics(&mut self, output: DiagnosticOutputId, epoch: u64) {
        self.diagnostics = NativeDiagnostics::new(output, epoch, "hud", "pump");
        self.diagnostics.snapshot("host", || {
            let sample = DiagnosticSample::new()
                .u64("presenter_hwnd", self.hwnd().0 as usize as u64)
                .u64("hud_hwnd", self.hud_hwnd())
                .bool("has_hud", self.has_hud())
                .text(
                    "topology",
                    if self.has_hud() {
                        "presenter_and_hud"
                    } else {
                        "presenter_only"
                    },
                )
                .bool(
                    "presenter_visible",
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.hwnd())
                    }
                    .as_bool(),
                );
            if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
                sample
                    .bool(
                        "hud_visible",
                        unsafe {
                            windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hud.hwnd())
                        }
                        .as_bool(),
                    )
                    .bool(
                        "initial_region_applied",
                        hud.applied_regions_hash().is_some(),
                    )
                    .u64(
                        "initial_region_hash",
                        hud.applied_regions_hash().unwrap_or(0),
                    )
            } else {
                sample
            }
        });
        self.sample_diagnostic_windows();
    }

    pub(crate) fn diagnostics(&self) -> &NativeDiagnostics {
        &self.diagnostics
    }

    pub(crate) fn sample_diagnostic_windows(&self) {
        self.sample_diagnostic_windows_with_hud_rect(None);
    }

    // Outer None requests a fresh periodic/create observation; Some(None)
    // preserves a known query failure without querying again for diagnostics.
    fn sample_diagnostic_windows_with_hud_rect(&self, hud_rect: Option<Option<RECT>>) {
        if !self.diagnostics.is_enabled() {
            return;
        }
        for (role, hwnd) in [
            ("presenter_window", self.hwnd()),
            ("hud_window", HWND(self.hud_hwnd() as *mut _)),
        ] {
            if hwnd.0.is_null() {
                self.diagnostics.snapshot(role, || {
                    DiagnosticSample::new()
                        .u64("hwnd", 0)
                        .bool("present", false)
                        .bool("visible", false)
                });
                continue;
            }
            self.diagnostics.snapshot(role, || unsafe {
                use windows::Win32::Graphics::Gdi::{
                    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromWindow,
                };
                let mut window = RECT::default();
                let mut client = RECT::default();
                let window_ok = if role == "hud_window" {
                    match hud_rect {
                        Some(Some(rect)) => {
                            window = rect;
                            true
                        }
                        Some(None) => false,
                        None => GetWindowRect(hwnd, &mut window).is_ok(),
                    }
                } else {
                    GetWindowRect(hwnd, &mut window).is_ok()
                };
                let client_ok = GetClientRect(hwnd, &mut client).is_ok();
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFOEXW::default();
                info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                let monitor_ok = GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool();
                DiagnosticSample::new()
                    .u64("hwnd", hwnd.0 as usize as u64)
                    .bool("present", true)
                    .bool(
                        "visible",
                        windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool(),
                    )
                    .bool("window_query_ok", window_ok)
                    .rect("window_rect", diagnostic_rect(window))
                    .bool("client_query_ok", client_ok)
                    .rect("client_rect", diagnostic_rect(client))
                    .u64("dpi", u64::from(GetDpiForWindow(hwnd)))
                    .u64("monitor", monitor.0 as usize as u64)
                    .bool("monitor_query_ok", monitor_ok)
                    .rect("monitor_rect", diagnostic_rect(info.monitorInfo.rcMonitor))
                    .rect("work_rect", diagnostic_rect(info.monitorInfo.rcWork))
                    .wide("monitor_device", info.szDevice)
            });
        }
    }

    fn presenter(&self) -> &NativeVideoWindow {
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { presenter }
            | NativeOwnedWindows::PresenterAndHud { presenter, .. } => presenter,
        }
    }

    fn presenter_mut(&mut self) -> &mut NativeVideoWindow {
        match &mut self.windows {
            NativeOwnedWindows::PresenterOnly { presenter }
            | NativeOwnedWindows::PresenterAndHud { presenter, .. } => presenter,
        }
    }

    pub(crate) fn render_targets(&self) -> NativeRenderTargets {
        self.assert_owner_thread();
        let presenter = NativeRenderTarget::new(
            self.presenter().hwnd(),
            self.presenter().generation(),
            NativeRenderTargetRole::Presenter,
        );
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { .. } => {
                NativeRenderTargets::PresenterOnly { presenter }
            }
            NativeOwnedWindows::PresenterAndHud { hud, .. } => {
                NativeRenderTargets::PresenterAndHud {
                    presenter,
                    hud: NativeRenderTarget::new(
                        hud.hwnd(),
                        self.presenter().generation(),
                        NativeRenderTargetRole::Hud,
                    ),
                }
            }
        }
    }

    pub(crate) fn render_target_transfer(&self) -> NativeRenderTargetTransfer {
        self.assert_owner_thread();
        let targets = self.render_targets();
        let presenter = targets.presenter();
        let hud = targets.hud();
        NativeRenderTargetTransfer {
            presenter: (presenter.hwnd.0 as usize as u64, presenter.generation),
            hud: hud.map(|target| (target.hwnd.0 as usize as u64, target.generation)),
        }
    }

    pub(crate) fn contract_windows(&self) -> HostWindows {
        self.assert_owner_thread();
        let presenter = OpaqueWindowHandle {
            id: OpaqueWindowId(self.presenter().hwnd().0 as usize as u64),
            generation: WindowGeneration(self.presenter().generation()),
        };
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { .. } => HostWindows::PresenterOnly { presenter },
            NativeOwnedWindows::PresenterAndHud { hud, .. } => HostWindows::PresenterAndHud {
                presenter,
                hud: OpaqueWindowHandle {
                    id: OpaqueWindowId(hud.hwnd().0 as usize as u64),
                    generation: WindowGeneration(self.presenter().generation()),
                },
            },
        }
    }

    pub(crate) fn os_pixels_per_point(&self) -> f32 {
        self.assert_owner_thread();
        let dpi = unsafe { GetDpiForWindow(self.presenter().hwnd()) };
        let value = dpi as f32 / 96.0;
        if value.is_finite() && value > 0.0 {
            value
        } else {
            1.0
        }
    }

    pub(crate) fn observe(&self) -> NativeWindowObservation {
        self.assert_owner_thread();
        let hwnd = self.presenter().hwnd();
        let mut point = POINT::default();
        let cursor_client_position = unsafe {
            if GetCursorPos(&mut point).is_ok() && ScreenToClient(hwnd, &mut point).as_bool() {
                Some([point.x, point.y])
            } else {
                None
            }
        };
        NativeWindowObservation {
            cursor_client_position,
            cursor_input_owned: false,
            cursor_hidden: false,
            cursor_last_activity: None,
            focus: NativeFocusState {
                target_id: hwnd.0 as usize as u64,
                thread_focus_id: super::native_window::thread_focus_hwnd(),
                foreground_is_current_process:
                    super::native_window::foreground_belongs_to_current_process_strict(),
            },
            // Mouse capture is outside the keyboard-only synthetic timeline.
            global_lbutton_down: unsafe {
                (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0
            },
            has_hud: self.has_hud(),
            hud_has_capture: self.hud_has_capture(),
        }
    }

    pub(crate) fn retain_render_topology(self, topology: HostWindowTopology) -> Self {
        self.assert_owner_thread();
        let Self {
            windows,
            affinity,
            editor_ui_snapshot,
            main_hwnd_for_raise,
            last_logged_region_hash,
            last_region_hash,
            last_regions_empty,
            diagnostics,
            #[cfg(test)]
            focus_claim_probe,
            _not_send_or_sync,
        } = self;
        let windows = match (windows, topology) {
            (windows @ NativeOwnedWindows::PresenterOnly { .. }, _) => windows,
            (
                windows @ NativeOwnedWindows::PresenterAndHud { .. },
                HostWindowTopology::PresenterAndHud,
            ) => windows,
            (
                NativeOwnedWindows::PresenterAndHud { presenter, hud, .. },
                HostWindowTopology::PresenterOnly,
            ) => {
                diagnostics.event("hud_window_retired", || {
                    DiagnosticSample::new()
                        .u64("hwnd", hud.hwnd().0 as usize as u64)
                        .text("reason", "topology_removed")
                });
                NativeOwnedWindows::PresenterOnly { presenter }
            }
        };
        let this = Self {
            windows,
            affinity,
            editor_ui_snapshot,
            main_hwnd_for_raise,
            last_logged_region_hash,
            last_region_hash,
            last_regions_empty,
            diagnostics,
            #[cfg(test)]
            focus_claim_probe,
            _not_send_or_sync,
        };
        this.diagnostics.snapshot("host", || {
            DiagnosticSample::new()
                .u64("presenter_hwnd", this.hwnd().0 as usize as u64)
                .u64("hud_hwnd", this.hud_hwnd())
                .bool("has_hud", this.has_hud())
                .text(
                    "topology",
                    if this.has_hud() {
                        "presenter_and_hud"
                    } else {
                        "presenter_only"
                    },
                )
        });
        this.sample_diagnostic_windows();
        this
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.assert_owner_thread();
        self.presenter().hwnd()
    }

    pub(crate) fn hud_hwnd(&self) -> u64 {
        self.assert_owner_thread();
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { .. } => 0,
            NativeOwnedWindows::PresenterAndHud { hud, .. } => hud.hwnd().0 as usize as u64,
        }
    }

    pub(crate) fn has_hud(&self) -> bool {
        self.assert_owner_thread();
        matches!(self.windows, NativeOwnedWindows::PresenterAndHud { .. })
    }

    pub(crate) fn hud_has_capture(&self) -> bool {
        self.assert_owner_thread();
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { .. } => false,
            NativeOwnedWindows::PresenterAndHud { hud, .. } => unsafe {
                GetCapture() == hud.hwnd()
            },
        }
    }

    pub(crate) fn cursor_capture_source(&self) -> Option<NativeVideoWindowSource> {
        self.assert_owner_thread();
        let capture = unsafe { GetCapture() };
        if capture == self.presenter().hwnd() {
            return Some(NativeVideoWindowSource::Presenter);
        }
        match &self.windows {
            NativeOwnedWindows::PresenterOnly { .. } => None,
            NativeOwnedWindows::PresenterAndHud { hud, .. } if capture == hud.hwnd() => {
                Some(NativeVideoWindowSource::Hud)
            }
            NativeOwnedWindows::PresenterAndHud { .. } => None,
        }
    }

    pub(crate) fn show_for_placement(
        &self,
        activate_on_show: bool,
        placement: NativeVideoPlacement,
    ) -> bool {
        self.assert_owner_thread();
        let show_t0 = std::time::Instant::now();
        let shown = if activate_on_show {
            self.presenter().show_and_raise()
        } else {
            self.presenter().show_no_activate()
        };
        let show_ms = show_t0.elapsed().as_secs_f64() * 1000.0;
        let focus_t0 = std::time::Instant::now();
        if super::native_child_should_set_focus(placement, activate_on_show) {
            unsafe {
                let _ = crate::presentation_observer::set_focus(
                    Some(self.presenter().hwnd()),
                    crate::presentation_observer::WindowRole::Presenter,
                    "NativeWindowHost::show_for_placement",
                );
            }
        }
        let focus_ms = focus_t0.elapsed().as_secs_f64() * 1000.0;
        if placement.is_main_window_child() {
            super::native_window::set_in_window_video_child(
                self.presenter().hwnd().0 as usize as u64,
            );
        } else {
            super::native_window::set_in_window_video_child(0);
        }
        crate::logger::log(format!(
            "[native-video] show_for_placement placement={} activate={activate_on_show} \
             show={show_ms:.1}ms set_focus={focus_ms:.1}",
            placement.label(),
        ));
        shown
    }

    pub(crate) fn apply_render_intents(&self, intents: &[NativeWindowIntent]) {
        self.assert_owner_thread();
        for intent in intents {
            match *intent {
                NativeWindowIntent::ClaimTextInputFocus => {
                    #[cfg(test)]
                    if let Some(probe) = &self.focus_claim_probe {
                        probe.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                    let focus_state = self.observe().focus;
                    let foreground_hwnd = super::native_window::foreground_hwnd();
                    let report =
                        super::native_window::claim_foreground(self.hwnd().0 as usize as u64);
                    let post_thread_focus_hwnd = super::native_window::thread_focus_hwnd();
                    crate::perf::event(
                        "native_presenter",
                        "text_input_focus_claim",
                        None,
                        0,
                        &[
                            (
                                "target_hwnd",
                                serde_json::Value::from(focus_state.target_id),
                            ),
                            ("foreground_hwnd", serde_json::Value::from(foreground_hwnd)),
                            (
                                "thread_focus_hwnd",
                                serde_json::Value::from(focus_state.thread_focus_id),
                            ),
                            (
                                "post_foreground_hwnd",
                                serde_json::Value::from(report.post_foreground_hwnd),
                            ),
                            (
                                "post_thread_focus_hwnd",
                                serde_json::Value::from(post_thread_focus_hwnd),
                            ),
                            (
                                "attach_thread_input_ok",
                                serde_json::Value::from(report.attach_thread_input_ok),
                            ),
                            (
                                "set_foreground_ok",
                                serde_json::Value::from(report.set_foreground_ok),
                            ),
                            (
                                "set_active_ok",
                                serde_json::Value::from(report.set_active_ok),
                            ),
                            ("set_focus_ok", serde_json::Value::from(report.set_focus_ok)),
                        ],
                    );
                }
                NativeWindowIntent::UpdateImeCursorArea {
                    x,
                    y,
                    width,
                    height,
                } => {
                    self.update_ime_cursor_area(x, y, width, height);
                }
                NativeWindowIntent::SetCursorPolicy { .. } => {}
            }
        }
    }

    fn update_ime_cursor_area(&self, x: i32, y: i32, width: i32, height: i32) {
        let (composition_form, candidate_form) = native_ime_window_forms(x, y, width, height);
        unsafe {
            let hwnd = self.hwnd();
            let himc = ImmGetContext(hwnd);
            if !himc.0.is_null() {
                let _ = ImmSetCompositionWindow(himc, &composition_form);
                let _ = ImmSetCandidateWindow(himc, &candidate_form);
                let _ = ImmReleaseContext(hwnd, himc);
            }
        }
    }

    /// Returns whether this call reached the Win32 `SetCursor` API. A non-hidden
    /// icon can be skipped only when its shared system cursor fails to load.
    pub(crate) fn apply_cursor_icon(&self, icon: NativeCursorIcon) -> bool {
        self.assert_owner_thread();
        match icon {
            NativeCursorIcon::Hidden => unsafe {
                SetCursor(None);
                true
            },
            icon => {
                let cursor_id = match icon {
                    NativeCursorIcon::Hidden => unreachable!(),
                    NativeCursorIcon::Hand => IDC_HAND,
                    NativeCursorIcon::Text => IDC_IBEAM,
                    NativeCursorIcon::ResizeHorizontal => IDC_SIZEWE,
                    NativeCursorIcon::ResizeVertical => IDC_SIZENS,
                    NativeCursorIcon::Move => IDC_SIZEALL,
                    NativeCursorIcon::NotAllowed => IDC_NO,
                    NativeCursorIcon::Wait => IDC_WAIT,
                    NativeCursorIcon::Arrow => IDC_ARROW,
                };
                if let Ok(cursor) = unsafe { LoadCursorW(None, cursor_id) } {
                    unsafe {
                        SetCursor(Some(cursor));
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn raise_presenter_to_front(&self) -> (bool, ForegroundClaimReport) {
        self.assert_owner_thread();
        let hwnd = self.hwnd().0 as usize as u64;
        (
            super::native_window::bring_to_front(hwnd),
            super::native_window::claim_foreground(hwnd),
        )
    }

    pub(crate) fn hide(&self) -> bool {
        self.assert_owner_thread();
        let hidden = self.presenter().hide();
        self.set_hud_window_visible(false);
        hidden
    }

    pub(crate) fn destroy(&mut self) {
        self.assert_owner_thread();
        self.presenter_mut().destroy();
    }

    pub(crate) fn set_hud_window_visible(&self, visible: bool) {
        self.assert_owner_thread();
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            hud.set_visible(visible);
            self.diagnostics.count("hud_hwnd_visibility_requested", 1);
            self.diagnostics.snapshot("hwnd_visibility", || {
                DiagnosticSample::new().bool("requested", visible).bool(
                    "actual",
                    unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hud.hwnd()) }
                        .as_bool(),
                )
            });
        }
    }

    pub(crate) fn set_hud_geometry(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        origin: &'static str,
    ) {
        self.assert_owner_thread();
        self.diagnostics.count(origin, 1);
        let mut sampled_hud_rect = None;
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            let result = hud.set_geometry(x, y, width, height);
            self.diagnostics.count("geometry_requested", 1);
            self.diagnostics.count(result.outcome, 1);
            self.diagnostics.count(
                "geometry_query_failed",
                u64::from(result.before.is_none() && result.outcome != "no_hwnd"),
            );
            self.diagnostics.count(
                "geometry_apply_attempt",
                u64::from(matches!(result.outcome, "applied" | "apply_failed")),
            );
            if self.diagnostics.is_enabled() {
                let mut after = RECT::default();
                let after_ok = if matches!(result.outcome, "applied" | "apply_failed") {
                    unsafe { GetWindowRect(hud.hwnd(), &mut after) }.is_ok()
                } else if let Some(before) = result.before {
                    after = before;
                    true
                } else {
                    false
                };
                self.diagnostics.count(
                    "geometry_actual_changed",
                    u64::from(after_ok && result.before.is_some_and(|before| before != after)),
                );
                self.diagnostics.count(
                    "geometry_actual_unchanged",
                    u64::from(after_ok && result.before == Some(after)),
                );
                self.diagnostics.count(
                    "geometry_actual_unknown",
                    u64::from(!after_ok || result.before.is_none()),
                );
                sampled_hud_rect = Some(after_ok.then_some(after));
                self.diagnostics.event("geometry", || {
                    DiagnosticSample::new()
                        .text("origin", origin)
                        .text("outcome", result.outcome)
                        .rect("requested", diagnostic_rect(result.requested))
                        .bool("before_known", result.before.is_some())
                        .rect("before", diagnostic_rect(result.before.unwrap_or_default()))
                        .bool("after_known", after_ok)
                        .rect("after", diagnostic_rect(after))
                });
            }
        } else {
            self.diagnostics.count("geometry_no_hud", 1);
        }
        self.sample_diagnostic_windows_with_hud_rect(sampled_hud_rect);
    }

    pub(crate) fn set_editor_ui_snapshot(
        &mut self,
        snapshot: Option<super::dsp::SharedEditorUiSnapshot>,
    ) {
        self.assert_owner_thread();
        self.editor_ui_snapshot = snapshot;
    }

    pub(crate) fn set_main_hwnd_for_raise_check(&mut self, main_hwnd: u64) {
        self.assert_owner_thread();
        self.main_hwnd_for_raise = main_hwnd;
    }

    pub(crate) fn foreground_allows_hud_raise(&self, require_editor_snapshot: bool) -> bool {
        self.hud_raise_foreground_decision(require_editor_snapshot)
            .allowed
    }

    fn hud_raise_foreground_decision(
        &self,
        require_editor_snapshot: bool,
    ) -> super::dsp::HudRaiseForegroundDecision {
        self.assert_owner_thread();
        if !self.has_hud() {
            return super::dsp::HudRaiseForegroundDecision {
                allowed: false,
                foreground_hwnd: 0,
                classification: "no_hud",
            };
        }
        let editor_snapshot = match self.editor_ui_snapshot.as_ref() {
            Some(snapshot) => super::dsp::read_editor_ui_snapshot(snapshot),
            None if require_editor_snapshot => {
                return super::dsp::HudRaiseForegroundDecision {
                    allowed: false,
                    foreground_hwnd: 0,
                    classification: "no_editor_snapshot",
                };
            }
            None => Arc::new(super::dsp::EditorUiSnapshot::default()),
        };
        crate::video::dsp::hud_raise_foreground_decision(
            self.hwnd().0 as usize as u64,
            self.hud_hwnd(),
            self.main_hwnd_for_raise,
            &editor_snapshot.hwnds,
        )
    }

    /// Classify the foreground captured by the HUD wndproc at button-down,
    /// before a queued focus request can change it. The snapshot is copied
    /// without holding a lock across USER32 calls.
    pub(crate) fn should_skip_hud_focus_claim(&self, hwnd_at_down: u64) -> bool {
        self.assert_owner_thread();
        let Some(snapshot) = self.editor_ui_snapshot.as_ref() else {
            return false;
        };
        let editor_snapshot = super::dsp::read_editor_ui_snapshot(snapshot);
        !crate::video::native_window::hud_down_should_claim_presenter_focus(
            crate::video::native_window::ui_group_for_hwnd(
                hwnd_at_down,
                &editor_snapshot.hwnd_pids,
                &std::collections::HashSet::new(),
            ),
        )
    }

    #[cfg(test)]
    pub(crate) fn install_focus_claim_probe_for_test(
        &mut self,
    ) -> Arc<std::sync::atomic::AtomicUsize> {
        let probe = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.focus_claim_probe = Some(Arc::clone(&probe));
        probe
    }

    pub(crate) fn try_raise_hud_to_top(&self) -> &'static str {
        self.assert_owner_thread();
        let decision = self.hud_raise_foreground_decision(true);
        self.diagnostics.snapshot("raise_foreground", || {
            DiagnosticSample::new()
                .bool("allowed", decision.allowed)
                .u64("foreground_hwnd", decision.foreground_hwnd)
                .text("classification", decision.classification)
        });
        if !decision.allowed {
            return match decision.classification {
                "no_hud" => "raise_no_hud",
                "no_editor_snapshot" => "raise_no_snapshot",
                _ => "raise_foreground_denied",
            };
        }
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            return if hud.raise_to_top() {
                "raise_applied"
            } else {
                "raise_apply_failed"
            };
        }
        "raise_no_hud"
    }

    pub(crate) fn apply_hud_regions(
        &mut self,
        regions: &[RECT],
        toast_active: bool,
        debug_description: Option<String>,
    ) {
        self.assert_owner_thread();
        let NativeOwnedWindows::PresenterAndHud {
            hud,
            regions: shared_regions,
            ..
        } = &mut self.windows
        else {
            return;
        };
        let new_hash = hud_window::hash_regions_for_debug(regions);
        if self.last_logged_region_hash != Some(new_hash) {
            if let Some(description) = debug_description {
                self.last_logged_region_hash = Some(new_hash);
                crate::logger::log(description);
            }
        }
        if let Ok(mut guard) = shared_regions.lock() {
            guard.regions = regions.to_vec();
        }
        let before = hud.applied_regions_hash();
        let outcome = hud.apply_regions(regions);
        self.diagnostics.count("region_published", 1);
        self.diagnostics.count(outcome, 1);
        self.diagnostics.count(
            "region_apply_attempt",
            u64::from(matches!(outcome, "region_applied" | "region_apply_failed")),
        );
        self.diagnostics.snapshot("region_applied", || {
            DiagnosticSample::new()
                .u64("published_hash", new_hash)
                .u64("count", regions.len() as u64)
                .bool("empty", regions.is_empty())
                .text("outcome", outcome)
                .bool("before_hash_known", before.is_some())
                .u64("before_hash", before.unwrap_or(0))
                .bool("applied_hash_known", hud.applied_regions_hash().is_some())
                .u64("applied_hash", hud.applied_regions_hash().unwrap_or(0))
        });
        if self.last_region_hash != Some(new_hash) {
            if crate::perf::is_enabled() {
                let (output_id, window_epoch) = self.diagnostics.correlation_key();
                crate::perf::event(
                    "native_presenter",
                    "hud_region_publish",
                    None,
                    0,
                    &[
                        ("output_id", serde_json::Value::from(output_id)),
                        ("window_epoch", serde_json::Value::from(window_epoch)),
                        ("region_hash", serde_json::Value::from(new_hash)),
                        (
                            "region_count",
                            serde_json::Value::from(regions.len() as i64),
                        ),
                        ("regions_empty", serde_json::Value::from(regions.is_empty())),
                        (
                            "was_empty",
                            serde_json::Value::from(self.last_regions_empty),
                        ),
                        ("toast_active", serde_json::Value::from(toast_active)),
                    ],
                );
            }
            self.last_region_hash = Some(new_hash);
            self.last_regions_empty = regions.is_empty();
        }
    }

    pub(crate) fn resize_to_rect(&self, placement: NativeVideoPlacement, rect: RECT) -> (u32, u32) {
        self.assert_owner_thread();
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;
        let (x, y) = if placement.is_child_window() {
            (0, 0)
        } else {
            (rect.left, rect.top)
        };
        log_detached_native_set_window_pos(
            "resize_existing_native_window_to_rect",
            self.hwnd(),
            x,
            y,
            width as i32,
            height as i32,
            format!(
                "placement={placement:?} rect=({},{} {}x{})",
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top
            ),
        );
        unsafe {
            let _ = crate::presentation_observer::set_window_pos(
                self.hwnd(),
                None,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeWindowHost::resize_existing_native_window_to_rect",
            );
        }
        (width, height)
    }

    pub(crate) fn reflow_child_to_parent_client(
        &self,
        parent_hwnd_raw: u64,
        current: (u32, u32),
    ) -> (u32, u32) {
        self.assert_owner_thread();
        if parent_hwnd_raw == 0 {
            return current;
        }
        let parent = HWND(parent_hwnd_raw as *mut _);
        let mut rect = RECT::default();
        if unsafe { GetClientRect(parent, &mut rect) }.is_err() {
            return current;
        }
        let width = (rect.right - rect.left).max(0) as u32;
        let height = (rect.bottom - rect.top).max(0) as u32;
        if width == 0 || height == 0 || (width, height) == current {
            return current;
        }
        log_detached_native_set_window_pos(
            "reflow_child_to_parent_client",
            self.hwnd(),
            0,
            0,
            width as i32,
            height as i32,
            format!(
                "parent=0x{parent_hwnd_raw:x} current={}x{}",
                current.0, current.1
            ),
        );
        unsafe {
            let _ = crate::presentation_observer::set_window_pos(
                self.hwnd(),
                None,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
                crate::presentation_observer::WindowRole::Presenter,
                "NativeWindowHost::reflow_child_to_parent_client",
            );
        }
        (width, height)
    }
}

pub(super) fn hud_debug_enabled() -> bool {
    std::env::var_os("MIV_HUD_DEBUG").is_some()
}

fn detached_window_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MIV_DETACHED_WINDOW_DEBUG").is_some())
}

fn log_detached_native_set_window_pos(
    source: &'static str,
    hwnd: HWND,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    detail: impl AsRef<str>,
) {
    if !detached_window_debug_enabled() {
        return;
    }
    crate::logger::log(format!(
        "[detached-window-debug] placement_trace source={source} \
         event=native_set_window_pos hwnd=0x{:x} pos=({x},{y}) size={width}x{height} {}",
        hwnd.0 as usize,
        detail.as_ref()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ime_forms_keep_cursor_exclusion_and_composition_below_the_cursor() {
        let (composition, candidate) = native_ime_window_forms(-3, 9, 4, 18);
        assert_eq!(
            composition.dwStyle,
            windows::Win32::UI::Input::Ime::CFS_POINT
        );
        assert_eq!(
            (composition.ptCurrentPos.x, composition.ptCurrentPos.y),
            (-3, 27)
        );
        assert_eq!(candidate.dwIndex, 0);
        assert_eq!(
            candidate.dwStyle,
            windows::Win32::UI::Input::Ime::CFS_EXCLUDE
        );
        assert_eq!(
            (candidate.ptCurrentPos.x, candidate.ptCurrentPos.y),
            (-3, 9)
        );
        for area in [composition.rcArea, candidate.rcArea] {
            assert_eq!(
                (area.left, area.top, area.right, area.bottom),
                (-3, 9, 1, 27)
            );
        }
    }

    #[test]
    fn native_ime_forms_keep_the_existing_one_physical_pixel_minimum() {
        let (composition, candidate) = native_ime_window_forms(10, 20, 0, -2);
        assert_eq!(
            (composition.ptCurrentPos.x, composition.ptCurrentPos.y),
            (10, 21)
        );
        for area in [composition.rcArea, candidate.rcArea] {
            assert_eq!(
                (area.left, area.top, area.right, area.bottom),
                (10, 20, 11, 21)
            );
        }
    }

    macro_rules! assert_not_impl_any {
        ($ty:ty: $trait:path) => {
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn marker() {}
                }
                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
                impl<T: ?Sized + $trait> AmbiguousIfImpl<u8> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::marker;
            };
        };
    }

    assert_not_impl_any!(NativeWindowHost: Send);
    assert_not_impl_any!(NativeWindowHost: Sync);
    assert_not_impl_any!(NativeVideoWindow: Send);
    assert_not_impl_any!(NativeVideoWindow: Sync);
    assert_not_impl_any!(hud_window::HudOverlayWindow: Send);
    assert_not_impl_any!(hud_window::HudOverlayWindow: Sync);
    assert_not_impl_any!(NativeRenderTargets: Send);
    assert_not_impl_any!(NativeRenderTargets: Sync);

    #[test]
    fn opaque_render_target_transfer_is_send_without_exposing_window_capabilities() {
        fn assert_send<T: Send>() {}
        assert_send::<NativeRenderTargetTransfer>();
    }

    #[test]
    fn window_thread_affinity_rejects_non_owner_thread() {
        let affinity = WindowThreadAffinity::current();
        let panicked = std::thread::spawn(move || {
            std::panic::catch_unwind(|| affinity.assert_current()).is_err()
        })
        .join()
        .expect("thread-affinity test thread must join");
        assert!(panicked);
    }

    #[test]
    fn render_module_has_no_user32_mutation_capability() {
        let source = include_str!("native_presenter/render_core.rs");
        for forbidden in [
            "ShowWindow(",
            "SetWindowPos(",
            "DestroyWindow(",
            "SetFocus(",
            "ImmSetCompositionWindow(",
            "ImmSetCandidateWindow(",
            "LoadCursorW(",
            "GetCursorPos(",
            "GetFocus(",
            "GetForegroundWindow(",
            "GetAsyncKeyState(",
            "GetCapture(",
            "claim_foreground(",
            "NativeVideoWindow::create",
            "HudOverlayWindow",
            "NativeWindowHost",
        ] {
            assert!(
                !source.contains(forbidden),
                "render core regained forbidden window capability: {forbidden}"
            );
        }
    }
}
