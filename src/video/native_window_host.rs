//! Native video HWND ownership boundary.
//!
//! `NativeWindowHost` owns the presenter and optional HUD HWNDs and is pinned to
//! the thread that created them.  Render code receives only `NativeRenderTargets`,
//! an opaque, non-owning target-binding lease with no USER32 lifecycle methods.

use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::thread::ThreadId;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionTarget};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
    ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, SetFocus, VK_LBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClientRect, GetCursorPos, GetGUIThreadInfo, GetWindowRect, IDC_ARROW,
    IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IDC_WAIT, IsChild,
    LoadCursorW, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor, SetWindowPos, WindowFromPoint,
};

use super::NativeVideoPlacement;
use super::native_window::{
    ForegroundClaimReport, NativeVideoWindow, NativeVideoWindowConfig, NativeVideoWindowEventSink,
};
use super::window_host_contract::{
    HostWindowTopology, HostWindows, OpaqueWindowHandle, OpaqueWindowId, WindowGeneration,
};

mod hud_window;

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

fn cursor_input_target_matches(
    presenter: u64,
    hud: u64,
    capture: u64,
    hit: u64,
    mut is_child: impl FnMut(u64, u64) -> bool,
) -> bool {
    let target = if capture != 0 { capture } else { hit };
    if target == 0 {
        return false;
    }
    target == presenter
        || (hud != 0 && target == hud)
        || is_child(presenter, target)
        || (hud != 0 && is_child(hud, target))
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
    editor_hwnds_snapshot: Option<Arc<RwLock<std::collections::HashSet<u64>>>>,
    main_hwnd_for_raise: u64,
    last_logged_region_hash: Option<u64>,
    last_region_hash: Option<u64>,
    last_regions_empty: bool,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl NativeWindowHost {
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
            editor_hwnds_snapshot: None,
            main_hwnd_for_raise: 0,
            last_logged_region_hash: None,
            last_region_hash: None,
            last_regions_empty: true,
            _not_send_or_sync: PhantomData,
        };
        debug_assert_eq!(this.render_targets().presenter().generation(), generation);
        Ok(this)
    }

    #[track_caller]
    fn assert_owner_thread(&self) {
        self.affinity.assert_current();
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
        let mut screen_point = POINT::default();
        let cursor_position_available = unsafe { GetCursorPos(&mut screen_point).is_ok() };
        let mut client_point = screen_point;
        let cursor_client_position = unsafe {
            if cursor_position_available && ScreenToClient(hwnd, &mut client_point).as_bool() {
                Some([client_point.x, client_point.y])
            } else {
                None
            }
        };
        let hud = self.hud_hwnd();
        let cursor_input_owned = unsafe {
            let local_capture = GetCapture();
            let mut gui = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            let foreground_capture = if GetGUIThreadInfo(0, &mut gui).is_ok() {
                gui.hwndCapture
            } else {
                HWND::default()
            };
            let capture = if !local_capture.0.is_null() {
                local_capture
            } else {
                foreground_capture
            };
            let hit = if cursor_position_available {
                WindowFromPoint(screen_point)
            } else {
                HWND::default()
            };
            cursor_input_target_matches(
                hwnd.0 as usize as u64,
                hud,
                capture.0 as usize as u64,
                hit.0 as usize as u64,
                |parent, child| {
                    IsChild(
                        HWND(parent as usize as *mut _),
                        HWND(child as usize as *mut _),
                    )
                    .as_bool()
                },
            )
        };
        NativeWindowObservation {
            cursor_client_position,
            cursor_input_owned,
            cursor_hidden: false,
            cursor_last_activity: None,
            focus: NativeFocusState {
                target_id: hwnd.0 as usize as u64,
                thread_focus_id: super::native_window::thread_focus_hwnd(),
                foreground_is_current_process:
                    super::native_window::foreground_belongs_to_current_process_strict(),
            },
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
            editor_hwnds_snapshot,
            main_hwnd_for_raise,
            last_logged_region_hash,
            last_region_hash,
            last_regions_empty,
            _not_send_or_sync,
        } = self;
        let windows = match (windows, topology) {
            (windows @ NativeOwnedWindows::PresenterOnly { .. }, _) => windows,
            (
                windows @ NativeOwnedWindows::PresenterAndHud { .. },
                HostWindowTopology::PresenterAndHud,
            ) => windows,
            (
                NativeOwnedWindows::PresenterAndHud { presenter, .. },
                HostWindowTopology::PresenterOnly,
            ) => NativeOwnedWindows::PresenterOnly { presenter },
        };
        Self {
            windows,
            affinity,
            editor_hwnds_snapshot,
            main_hwnd_for_raise,
            last_logged_region_hash,
            last_region_hash,
            last_regions_empty,
            _not_send_or_sync,
        }
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

    pub(crate) fn show_for_placement(
        &self,
        activate_on_show: bool,
        placement: NativeVideoPlacement,
    ) -> bool {
        self.assert_owner_thread();
        let shown = if activate_on_show {
            self.presenter().show_and_raise()
        } else {
            self.presenter().show_no_activate()
        };
        if super::native_child_should_set_focus(placement, activate_on_show) {
            unsafe {
                let _ = SetFocus(Some(self.presenter().hwnd()));
            }
        }
        if placement.is_main_window_child() {
            super::native_window::set_in_window_video_child(
                self.presenter().hwnd().0 as usize as u64,
            );
        } else {
            super::native_window::set_in_window_video_child(0);
        }
        shown
    }

    pub(crate) fn apply_render_intents(&self, intents: &[NativeWindowIntent]) {
        self.assert_owner_thread();
        for intent in intents {
            match *intent {
                NativeWindowIntent::ClaimTextInputFocus => {
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
                NativeWindowIntent::SetCursorPolicy { .. } => {}
            }
        }
    }

    pub(crate) fn apply_cursor_icon(&self, icon: NativeCursorIcon) {
        self.assert_owner_thread();
        match icon {
            NativeCursorIcon::Hidden => unsafe {
                SetCursor(None);
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
        }
    }

    pub(crate) fn set_hud_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        self.assert_owner_thread();
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            hud.set_geometry(x, y, width, height);
        }
    }

    pub(crate) fn set_editor_hwnds_snapshot(
        &mut self,
        snapshot: Option<Arc<RwLock<std::collections::HashSet<u64>>>>,
    ) {
        self.assert_owner_thread();
        self.editor_hwnds_snapshot = snapshot;
    }

    pub(crate) fn set_main_hwnd_for_raise_check(&mut self, main_hwnd: u64) {
        self.assert_owner_thread();
        self.main_hwnd_for_raise = main_hwnd;
    }

    pub(crate) fn foreground_allows_hud_raise(&self, require_editor_snapshot: bool) -> bool {
        self.assert_owner_thread();
        if !self.has_hud() {
            return false;
        }
        let editor_hwnds = match self.editor_hwnds_snapshot.as_ref() {
            Some(snapshot) => match snapshot.try_read() {
                Ok(guard) => guard.clone(),
                Err(_) => return false,
            },
            None if require_editor_snapshot => return false,
            None => std::collections::HashSet::new(),
        };
        crate::video::dsp::foreground_allows_hud_raise(
            self.hwnd().0 as usize as u64,
            self.hud_hwnd(),
            self.main_hwnd_for_raise,
            &editor_hwnds,
        )
    }

    pub(crate) fn try_raise_hud_to_top(&self) -> bool {
        self.assert_owner_thread();
        if !self.foreground_allows_hud_raise(true) {
            return false;
        }
        if let NativeOwnedWindows::PresenterAndHud { hud, .. } = &self.windows {
            hud.raise_to_top();
            return true;
        }
        false
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
        hud.apply_regions(regions);
        if self.last_region_hash != Some(new_hash) {
            crate::perf::event(
                "native_presenter",
                "hud_region_publish",
                None,
                0,
                &[
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
            let _ = SetWindowPos(
                self.hwnd(),
                None,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
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
            let _ = SetWindowPos(
                self.hwnd(),
                None,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
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
    fn cursor_input_owner_rejects_window_covering_presenter_rectangle() {
        let presenter = 10;
        let hud = 11;
        let covering_window = 20;
        assert!(!cursor_input_target_matches(
            presenter,
            hud,
            0,
            covering_window,
            |_, _| false,
        ));
        assert!(cursor_input_target_matches(
            presenter,
            hud,
            0,
            presenter,
            |_, _| false,
        ));
        assert!(cursor_input_target_matches(
            presenter,
            hud,
            hud,
            covering_window,
            |_, _| false,
        ));
    }

    #[test]
    fn cursor_input_owner_unknown_is_not_owned() {
        assert!(!cursor_input_target_matches(10, 11, 0, 0, |_, _| false));
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
