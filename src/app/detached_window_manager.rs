use std::collections::{HashMap, HashSet};

use super::ActionSurface;
#[cfg(not(test))]
use super::App;
#[cfg(windows)]
use crate::ring_shortcut::RightDragCommand;

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetachedWindowHwndDiff {
    Created(u64),
    NoChange,
    Ambiguous(Vec<u64>),
}

#[cfg(windows)]
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedWindowState {
    Opening,
    Active,
    Parked,
    ParkedLive,
    Resuming,
    Closing,
}

/// Progress of a right-drag command recognized by a passive detached window.
/// The command remains owned by that window until its viewer bundle is mounted.
#[cfg(windows)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PendingRightDragCommand {
    Recognized { command: RightDragCommand },
    Activating { command: RightDragCommand },
    PendingExecution { command: RightDragCommand },
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DetachedActivationIntent {
    ActivateOnly,
    RightDrag(PendingRightDragCommand),
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetachedActivationDispatch {
    pub(crate) window_id: u64,
    pub(crate) command: Option<RightDragCommand>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MainFontAtlasResyncFrameSafety {
    pub(crate) placement_pending: bool,
    pub(crate) cloak_or_backdrop_active: bool,
    pub(crate) opening_count: usize,
    pub(crate) closing_count: usize,
}

#[cfg(windows)]
impl MainFontAtlasResyncFrameSafety {
    pub(crate) fn is_settled(self) -> bool {
        !self.placement_pending
            && !self.cloak_or_backdrop_active
            && self.opening_count == 0
            && self.closing_count == 0
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedActivationCloseHitTest {
    ImageBar,
    MusicChrome,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchTarget {
    pub(crate) window_id: u64,
    pub(crate) hwnd: u64,
    pub(crate) eligible: bool,
    pub(crate) close_hit_test: DetachedActivationCloseHitTest,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchTargetRect {
    pub(crate) window_id: u64,
    pub(crate) hwnd: u64,
    pub(crate) eligible: bool,
    pub(crate) close_hit_test: DetachedActivationCloseHitTest,
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchSample {
    pub(crate) left_button_down: bool,
    pub(crate) foreground_hwnd: u64,
    pub(crate) cursor_root_hwnd: u64,
    pub(crate) native_close_hit_hwnd: u64,
    pub(crate) cursor_pos: Option<(i32, i32)>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedActivationClickIntent {
    Activate,
    Close,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedActivationClickCandidate {
    pub(crate) window_id: u64,
    pub(crate) hwnd: u64,
    pub(crate) repair_hwnd: Option<u64>,
    pub(crate) start_screen_pos: Option<(i32, i32)>,
    pub(crate) drag_sensitive: bool,
    pub(crate) intent: DetachedActivationClickIntent,
    pub(crate) moved_too_far: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchState {
    pub(crate) left_button_was_down: bool,
    pub(crate) active_click: Option<DetachedActivationClickCandidate>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchDiagnostic {
    pub(crate) reason: &'static str,
    pub(crate) foreground_hwnd: u64,
    pub(crate) cursor_root_hwnd: u64,
    pub(crate) cursor_pos: Option<(i32, i32)>,
    pub(crate) target_window_id: Option<u64>,
    pub(crate) target_hwnd: Option<u64>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedActivationRequest {
    pub(crate) window_id: u64,
    pub(crate) repair_hwnd: Option<u64>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedCloseRequest {
    pub(crate) window_id: u64,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetachedActivationWatchStepResult {
    pub(crate) activation: Option<DetachedActivationRequest>,
    pub(crate) close: Option<DetachedCloseRequest>,
    pub(crate) diagnostic: Option<DetachedActivationWatchDiagnostic>,
}

#[cfg(all(windows, not(test)))]
struct DetachedActivationWatchCommand {
    targets: Vec<DetachedActivationWatchTarget>,
    main_hwnd: u64,
    claimed_hwnds: Vec<u64>,
    repaint_ctx: egui::Context,
}

#[cfg(windows)]
pub(crate) struct DetachedActivationWatcher {
    #[cfg(not(test))]
    command_tx: std::sync::mpsc::Sender<DetachedActivationWatchCommand>,
    activation_rx: std::sync::mpsc::Receiver<DetachedActivationRequest>,
    close_rx: std::sync::mpsc::Receiver<DetachedCloseRequest>,
    diagnostic_rx: std::sync::mpsc::Receiver<DetachedActivationWatchDiagnostic>,
}

#[cfg(windows)]
impl DetachedActivationWatcher {
    fn new() -> Self {
        let (activation_tx, activation_rx) = std::sync::mpsc::channel();
        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let (diagnostic_tx, diagnostic_rx) = std::sync::mpsc::channel();
        #[cfg(not(test))]
        {
            let (command_tx, command_rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("detached-activation-watch".to_string())
                .spawn(move || {
                    Self::watch_thread(command_rx, activation_tx, close_tx, diagnostic_tx);
                })
                .expect("failed to spawn detached activation watcher");
            Self {
                command_tx,
                activation_rx,
                close_rx,
                diagnostic_rx,
            }
        }
        #[cfg(test)]
        {
            let _ = (activation_tx, close_tx, diagnostic_tx);
            Self {
                activation_rx,
                close_rx,
                diagnostic_rx,
            }
        }
    }

    fn update_targets(
        &self,
        targets: Vec<DetachedActivationWatchTarget>,
        main_hwnd: u64,
        claimed_hwnds: Vec<u64>,
        ctx: &egui::Context,
    ) {
        #[cfg(not(test))]
        {
            let _ = self.command_tx.send(DetachedActivationWatchCommand {
                targets,
                main_hwnd,
                claimed_hwnds,
                repaint_ctx: ctx.clone(),
            });
        }
        #[cfg(test)]
        {
            let _ = (targets, main_hwnd, claimed_hwnds, ctx);
        }
    }

    fn drain_activation_requests(&self) -> Vec<DetachedActivationRequest> {
        let mut requests = Vec::new();
        loop {
            match self.activation_rx.try_recv() {
                Ok(request) => requests.push(request),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        requests
    }

    fn drain_close_requests(&self) -> Vec<DetachedCloseRequest> {
        let mut requests = Vec::new();
        loop {
            match self.close_rx.try_recv() {
                Ok(request) => requests.push(request),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        requests
    }

    fn drain_diagnostics(&self) -> Vec<DetachedActivationWatchDiagnostic> {
        let mut diagnostics = Vec::new();
        loop {
            match self.diagnostic_rx.try_recv() {
                Ok(diagnostic) => diagnostics.push(diagnostic),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        diagnostics
    }

    #[cfg(not(test))]
    fn watch_thread(
        command_rx: std::sync::mpsc::Receiver<DetachedActivationWatchCommand>,
        activation_tx: std::sync::mpsc::Sender<DetachedActivationRequest>,
        close_tx: std::sync::mpsc::Sender<DetachedCloseRequest>,
        diagnostic_tx: std::sync::mpsc::Sender<DetachedActivationWatchDiagnostic>,
    ) {
        let mut targets = Vec::new();
        let mut main_hwnd = 0;
        let mut claimed_hwnds = Vec::new();
        let mut repaint_ctx: Option<egui::Context> = None;
        let mut state = DetachedActivationWatchState::default();
        loop {
            if targets.is_empty() {
                match command_rx.recv() {
                    Ok(command) => {
                        targets = command.targets;
                        main_hwnd = command.main_hwnd;
                        claimed_hwnds = command.claimed_hwnds;
                        repaint_ctx = Some(command.repaint_ctx);
                        state = DetachedActivationWatchState::default();
                    }
                    Err(_) => break,
                }
                continue;
            }

            while let Ok(command) = command_rx.try_recv() {
                targets = command.targets;
                main_hwnd = command.main_hwnd;
                claimed_hwnds = command.claimed_hwnds;
                repaint_ctx = Some(command.repaint_ctx);
                if targets.is_empty() {
                    state = DetachedActivationWatchState::default();
                    break;
                }
            }
            if targets.is_empty() {
                continue;
            }

            let rects = Self::os_target_rects(&targets);
            let sample = Self::os_sample();
            let result = App::detached_activation_watch_step_result_with_context(
                &mut state,
                sample,
                &rects,
                main_hwnd,
                &claimed_hwnds,
            );
            if let Some(diagnostic) = result.diagnostic {
                let _ = diagnostic_tx.send(diagnostic);
            }
            if let Some(request) = result.activation {
                let _ = activation_tx.send(request);
                if let Some(ctx) = repaint_ctx.as_ref() {
                    ctx.request_repaint();
                }
            }
            if let Some(request) = result.close {
                let _ = close_tx.send(request);
                if let Some(ctx) = repaint_ctx.as_ref() {
                    ctx.request_repaint();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    #[cfg(not(test))]
    fn os_sample() -> DetachedActivationWatchSample {
        use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        use windows::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow, HTCLOSE, SMTO_ABORTIFHUNG,
            SendMessageTimeoutW, WM_NCHITTEST, WindowFromPoint,
        };

        // Mouse capture is outside the keyboard-only synthetic timeline, so
        // this physical button level intentionally bypasses key_input.
        let left_button_down =
            unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 };
        let foreground_hwnd = unsafe { GetForegroundWindow().0 as u64 };
        let mut point = POINT::default();
        let cursor_ok = unsafe { GetCursorPos(&mut point) }.is_ok();
        let cursor_pos = cursor_ok.then_some((point.x, point.y));
        let cursor_root_hwnd = if cursor_ok {
            unsafe {
                let hwnd = WindowFromPoint(point);
                if hwnd.0.is_null() {
                    0
                } else {
                    let root = GetAncestor(hwnd, GA_ROOT);
                    root.0 as u64
                }
            }
        } else {
            0
        };
        let native_close_hit_hwnd = if cursor_ok && cursor_root_hwnd != 0 {
            let packed = ((point.y as u32 & 0xffff) << 16) | (point.x as u32 & 0xffff);
            let mut hit_test_result = 0usize;
            let send_result = unsafe {
                SendMessageTimeoutW(
                    HWND(cursor_root_hwnd as *mut _),
                    WM_NCHITTEST,
                    WPARAM(0),
                    LPARAM(packed as isize),
                    SMTO_ABORTIFHUNG,
                    50,
                    Some(&mut hit_test_result),
                )
            };
            if send_result.0 != 0 && hit_test_result as u32 == HTCLOSE {
                cursor_root_hwnd
            } else {
                0
            }
        } else {
            0
        };
        DetachedActivationWatchSample {
            left_button_down,
            foreground_hwnd,
            cursor_root_hwnd,
            native_close_hit_hwnd,
            cursor_pos,
        }
    }

    #[cfg(not(test))]
    fn os_target_rects(
        targets: &[DetachedActivationWatchTarget],
    ) -> Vec<DetachedActivationWatchTargetRect> {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};

        targets
            .iter()
            .filter_map(|target| {
                let hwnd = HWND(target.hwnd as *mut _);
                unsafe {
                    if !IsWindow(Some(hwnd)).as_bool() {
                        return None;
                    }
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).is_err() {
                        return None;
                    }
                    Some(DetachedActivationWatchTargetRect {
                        window_id: target.window_id,
                        hwnd: target.hwnd,
                        eligible: target.eligible,
                        close_hit_test: target.close_hit_test,
                        left: rect.left,
                        top: rect.top,
                        right: rect.right,
                        bottom: rect.bottom,
                    })
                }
            })
            .collect()
    }
}

#[cfg(windows)]
#[derive(Debug, Clone)]
pub(crate) struct DetachedTrimBBoxEntry {
    pub(crate) item_key: String,
    pub(crate) bbox: Option<egui::Rect>,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
pub(crate) struct DetachedWindowRuntime {
    pub(crate) window_id: u64,
    pub(crate) state: DetachedWindowState,
    pub(crate) hwnd: u64,
    pub(crate) placement: Option<crate::settings::DetachedViewerWindowPlacement>,
    pub(crate) builder_placement_latch: Option<crate::settings::DetachedViewerWindowPlacement>,
    pub(crate) trim_bboxes: std::collections::HashMap<usize, DetachedTrimBBoxEntry>,
    pub(crate) linked: bool,
    pub(crate) activation_intent: Option<DetachedActivationIntent>,
}

#[cfg(windows)]
impl DetachedWindowRuntime {
    fn new(window_id: u64, linked: bool) -> Self {
        Self {
            window_id,
            state: DetachedWindowState::Opening,
            hwnd: 0,
            placement: None,
            builder_placement_latch: None,
            trim_bboxes: std::collections::HashMap::new(),
            linked,
            activation_intent: None,
        }
    }
}

/// Detached window lifecycle state owned independently from the rest of `App`.
///
/// `App` remains responsible for cross-cutting orchestration (settings persistence,
/// media presenter changes, and logging), while runtime state transitions, HWND
/// registration, activation-watch queues, and surface ownership live here.
pub(crate) struct DetachedWindowManager {
    runtimes: HashMap<u64, DetachedWindowRuntime>,
    activation_watcher: DetachedActivationWatcher,
    #[cfg(test)]
    live_hwnds_for_test: HashSet<u64>,
    last_input_surface: ActionSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RuntimeTransition {
    pub(super) window_id: u64,
    pub(super) from: DetachedWindowState,
    pub(super) to: DetachedWindowState,
    pub(super) hwnd: u64,
    pub(super) linked: bool,
}

impl DetachedWindowManager {
    pub(super) fn new() -> Self {
        Self {
            runtimes: HashMap::new(),
            activation_watcher: DetachedActivationWatcher::new(),
            #[cfg(test)]
            live_hwnds_for_test: HashSet::new(),
            last_input_surface: ActionSurface::MainWindow,
        }
    }

    fn entry_mut(&mut self, window_id: u64, default_linked: bool) -> &mut DetachedWindowRuntime {
        self.runtimes
            .entry(window_id)
            .or_insert_with(|| DetachedWindowRuntime::new(window_id, default_linked))
    }

    pub(super) fn runtime(&self, window_id: u64) -> Option<&DetachedWindowRuntime> {
        self.runtimes.get(&window_id)
    }

    pub(super) fn placement(
        &self,
        window_id: u64,
    ) -> Option<crate::settings::DetachedViewerWindowPlacement> {
        self.runtime(window_id)
            .and_then(|runtime| runtime.placement)
    }

    pub(super) fn set_placement(
        &mut self,
        window_id: u64,
        placement: crate::settings::DetachedViewerWindowPlacement,
        default_linked: bool,
    ) -> Option<crate::settings::DetachedViewerWindowPlacement> {
        let runtime = self.entry_mut(window_id, default_linked);
        let previous = runtime.placement;
        runtime.placement = Some(placement);
        previous
    }

    pub(super) fn trim_bbox(
        &self,
        window_id: u64,
        idx: usize,
        item_key: &str,
    ) -> Option<Option<egui::Rect>> {
        self.runtime(window_id)
            .and_then(|runtime| runtime.trim_bboxes.get(&idx))
            .filter(|entry| entry.item_key == item_key)
            .map(|entry| entry.bbox)
    }

    pub(super) fn set_trim_bbox(
        &mut self,
        window_id: u64,
        idx: usize,
        item_key: String,
        bbox: Option<egui::Rect>,
        default_linked: bool,
    ) {
        self.entry_mut(window_id, default_linked)
            .trim_bboxes
            .insert(idx, DetachedTrimBBoxEntry { item_key, bbox });
    }

    pub(super) fn builder_placement_latch(
        &self,
        window_id: u64,
    ) -> Option<crate::settings::DetachedViewerWindowPlacement> {
        self.runtime(window_id)
            .and_then(|runtime| runtime.builder_placement_latch)
    }

    pub(super) fn set_builder_placement_latch(
        &mut self,
        window_id: u64,
        placement: crate::settings::DetachedViewerWindowPlacement,
        default_linked: bool,
    ) -> Option<crate::settings::DetachedViewerWindowPlacement> {
        let runtime = self.entry_mut(window_id, default_linked);
        let previous = runtime.builder_placement_latch;
        runtime.builder_placement_latch = Some(placement);
        previous
    }

    pub(super) fn hwnd_raw(&self, window_id: u64) -> Option<u64> {
        self.runtime(window_id)
            .map(|runtime| runtime.hwnd)
            .filter(|hwnd| *hwnd != 0)
    }

    pub(super) fn hwnd_is_alive(&self, hwnd: u64) -> bool {
        #[cfg(test)]
        return self.live_hwnds_for_test.contains(&hwnd);
        #[cfg(not(test))]
        crate::video::native_window::is_window_alive(hwnd)
    }

    pub(super) fn hwnd_alive(&self, window_id: u64) -> Option<u64> {
        let hwnd = self.hwnd_raw(window_id)?;
        self.hwnd_is_alive(hwnd).then_some(hwnd)
    }

    pub(super) fn clear_hwnd(&mut self, window_id: u64) -> Option<u64> {
        let runtime = self.runtimes.get_mut(&window_id)?;
        let hwnd = std::mem::take(&mut runtime.hwnd);
        (hwnd != 0).then_some(hwnd)
    }

    pub(super) fn clear_hwnd_if_dead(&mut self, window_id: u64) -> Option<u64> {
        let hwnd = self.hwnd_raw(window_id)?;
        if self.hwnd_is_alive(hwnd) {
            None
        } else {
            self.clear_hwnd(window_id)
        }
    }

    pub(super) fn set_hwnd(
        &mut self,
        window_id: u64,
        hwnd: u64,
        default_linked: bool,
    ) -> Option<u64> {
        if hwnd == 0 {
            return self.clear_hwnd(window_id);
        }
        let runtime = self.entry_mut(window_id, default_linked);
        let previous = (runtime.hwnd != 0).then_some(runtime.hwnd);
        runtime.hwnd = hwnd;
        previous
    }

    pub(super) fn registered_hwnds(&self) -> HashSet<u64> {
        self.runtimes
            .values()
            .map(|runtime| runtime.hwnd)
            .filter(|hwnd| *hwnd != 0)
            .collect()
    }

    pub(super) fn hwnd_claimed_by_other(&self, window_id: u64, hwnd: u64) -> Option<u64> {
        self.runtimes.iter().find_map(|(&other_id, runtime)| {
            (other_id != window_id && runtime.hwnd == hwnd).then_some(other_id)
        })
    }

    pub(super) fn transition_state(
        &mut self,
        window_id: u64,
        new_state: DetachedWindowState,
        default_linked: bool,
    ) -> RuntimeTransition {
        let runtime = self.entry_mut(window_id, default_linked);
        let transition = RuntimeTransition {
            window_id: runtime.window_id,
            from: runtime.state,
            to: new_state,
            hwnd: runtime.hwnd,
            linked: runtime.linked,
        };
        runtime.state = new_state;
        transition
    }

    pub(super) fn state(&self, window_id: u64) -> Option<DetachedWindowState> {
        self.runtime(window_id).map(|runtime| runtime.state)
    }

    pub(super) fn set_linked(
        &mut self,
        window_id: u64,
        linked: bool,
        default_linked: bool,
    ) -> bool {
        let runtime = self.entry_mut(window_id, default_linked);
        let previous = runtime.linked;
        runtime.linked = linked;
        previous
    }

    pub(super) fn queue_deferred_activation(
        &mut self,
        window_id: u64,
        default_linked: bool,
    ) -> (bool, DetachedWindowState) {
        let runtime = self.entry_mut(window_id, default_linked);
        let was_pending = runtime.activation_intent.is_some();
        if runtime.activation_intent.is_none() {
            runtime.activation_intent = Some(DetachedActivationIntent::ActivateOnly);
        }
        (was_pending, runtime.state)
    }

    pub(super) fn queue_right_drag_command(
        &mut self,
        window_id: u64,
        command: RightDragCommand,
        default_linked: bool,
    ) -> (bool, DetachedWindowState) {
        let runtime = self.entry_mut(window_id, default_linked);
        let accepted = !matches!(
            runtime.activation_intent,
            Some(DetachedActivationIntent::RightDrag(_))
        );
        if accepted {
            runtime.activation_intent = Some(DetachedActivationIntent::RightDrag(
                PendingRightDragCommand::Recognized { command },
            ));
        }
        (accepted, runtime.state)
    }

    #[cfg(test)]
    pub(super) fn deferred_activation_pending(&self, window_id: u64) -> bool {
        self.runtime(window_id)
            .is_some_and(|runtime| runtime.activation_intent.is_some())
    }

    #[cfg(test)]
    pub(super) fn activation_intent(&self, window_id: u64) -> Option<&DetachedActivationIntent> {
        self.runtime(window_id)
            .and_then(|runtime| runtime.activation_intent.as_ref())
    }

    pub(super) fn take_pending_deferred_activation(
        &mut self,
    ) -> Option<DetachedActivationDispatch> {
        let mut ids = self
            .runtimes
            .iter()
            .filter(|(_, runtime)| {
                runtime.state != DetachedWindowState::Closing
                    && matches!(
                        runtime.activation_intent,
                        Some(DetachedActivationIntent::ActivateOnly)
                            | Some(DetachedActivationIntent::RightDrag(
                                PendingRightDragCommand::Recognized { .. }
                            ))
                    )
            })
            .map(|(&id, _)| id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        let id = ids.into_iter().next()?;
        let runtime = self.runtimes.get_mut(&id)?;
        let command = match runtime.activation_intent.take()? {
            DetachedActivationIntent::ActivateOnly => None,
            DetachedActivationIntent::RightDrag(PendingRightDragCommand::Recognized {
                command,
            }) => {
                runtime.activation_intent = Some(DetachedActivationIntent::RightDrag(
                    PendingRightDragCommand::Activating {
                        command: command.clone(),
                    },
                ));
                Some(command)
            }
            other => {
                runtime.activation_intent = Some(other);
                return None;
            }
        };
        Some(DetachedActivationDispatch {
            window_id: id,
            command,
        })
    }

    pub(super) fn mark_right_drag_pending_execution(
        &mut self,
        window_id: u64,
        command: RightDragCommand,
    ) -> bool {
        let Some(runtime) = self.runtimes.get_mut(&window_id) else {
            return false;
        };
        let Some(pending) = runtime.activation_intent.take() else {
            return false;
        };
        match pending {
            DetachedActivationIntent::RightDrag(PendingRightDragCommand::Activating {
                command: activating,
            }) if activating == command => {
                runtime.activation_intent = Some(DetachedActivationIntent::RightDrag(
                    PendingRightDragCommand::PendingExecution { command },
                ));
                true
            }
            other => {
                runtime.activation_intent = Some(other);
                false
            }
        }
    }

    pub(super) fn restore_activation_intent(
        &mut self,
        window_id: u64,
        intent: DetachedActivationIntent,
        default_linked: bool,
    ) {
        self.entry_mut(window_id, default_linked).activation_intent = Some(intent);
    }

    pub(super) fn take_pending_right_drag_execution(
        &mut self,
        window_id: u64,
    ) -> Option<RightDragCommand> {
        let runtime = self.runtimes.get_mut(&window_id)?;
        let pending = runtime.activation_intent.take()?;
        match pending {
            DetachedActivationIntent::RightDrag(PendingRightDragCommand::PendingExecution {
                command,
            }) => Some(command),
            other => {
                runtime.activation_intent = Some(other);
                None
            }
        }
    }

    pub(super) fn pending_right_drag_execution_window_ids(&self) -> Vec<u64> {
        self.runtimes
            .iter()
            .filter_map(|(&id, runtime)| {
                matches!(
                    runtime.activation_intent,
                    Some(DetachedActivationIntent::RightDrag(
                        PendingRightDragCommand::PendingExecution { .. }
                    ))
                )
                .then_some(id)
            })
            .collect()
    }

    pub(super) fn discard_right_drag_command(
        &mut self,
        window_id: u64,
    ) -> Option<PendingRightDragCommand> {
        let runtime = self.runtimes.get_mut(&window_id)?;
        let pending = runtime.activation_intent.take()?;
        match pending {
            DetachedActivationIntent::RightDrag(command) => Some(command),
            other => {
                runtime.activation_intent = Some(other);
                None
            }
        }
    }

    pub(super) fn remove(&mut self, window_id: u64) -> Option<DetachedWindowRuntime> {
        self.runtimes.remove(&window_id)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.runtimes.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.runtimes.len()
    }

    pub(super) fn ids(&self) -> Vec<u64> {
        self.runtimes.keys().copied().collect()
    }

    pub(super) fn state_count(&self, state: DetachedWindowState) -> usize {
        self.runtimes
            .values()
            .filter(|runtime| runtime.state == state)
            .count()
    }

    pub(super) fn claimed_hwnds(&self) -> Vec<u64> {
        self.runtimes
            .values()
            .map(|runtime| runtime.hwnd)
            .filter(|hwnd| *hwnd != 0)
            .collect()
    }

    pub(super) fn update_activation_watch_targets(
        &self,
        targets: Vec<DetachedActivationWatchTarget>,
        main_hwnd: u64,
        ctx: &egui::Context,
    ) {
        self.activation_watcher
            .update_targets(targets, main_hwnd, self.claimed_hwnds(), ctx);
    }

    pub(super) fn drain_watch_diagnostics(&self) -> Vec<DetachedActivationWatchDiagnostic> {
        self.activation_watcher.drain_diagnostics()
    }

    pub(super) fn drain_close_requests(&self) -> Vec<DetachedCloseRequest> {
        self.activation_watcher.drain_close_requests()
    }

    pub(super) fn drain_activation_requests(&self) -> Vec<DetachedActivationRequest> {
        self.activation_watcher.drain_activation_requests()
    }

    pub(super) fn note_input_surface(&mut self, surface: ActionSurface) {
        self.last_input_surface = surface;
    }

    pub(super) fn resolve_input_surface(
        &self,
        detached_viewer_active: bool,
        main_hwnd: Option<u64>,
        foreground_app_hwnd: Option<u64>,
        viewer_available: bool,
    ) -> ActionSurface {
        if !detached_viewer_active || !viewer_available {
            return if viewer_available {
                ActionSurface::Viewer
            } else {
                ActionSurface::MainWindow
            };
        }

        match foreground_app_hwnd {
            Some(foreground) if main_hwnd == Some(foreground) => ActionSurface::MainWindow,
            Some(_) => ActionSurface::Viewer,
            None => self.last_input_surface,
        }
    }

    #[cfg(test)]
    pub(super) fn set_live_hwnds_for_test<I>(&mut self, hwnds: I)
    where
        I: IntoIterator<Item = u64>,
    {
        self.live_hwnds_for_test = hwnds.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_click_command() -> RightDragCommand {
        RightDragCommand::ViewerShortRightClick {
            context: crate::ring_shortcut::RightDragContext::ImageFullscreen,
            pos: egui::pos2(12.0, 34.0),
        }
    }

    #[test]
    fn activate_only_intent_keeps_plain_deferred_activation_behavior() {
        let mut manager = DetachedWindowManager::new();
        assert_eq!(manager.queue_deferred_activation(7, false).0, false);
        assert!(matches!(
            manager.activation_intent(7),
            Some(DetachedActivationIntent::ActivateOnly)
        ));

        let dispatch = manager
            .take_pending_deferred_activation()
            .expect("plain activation dispatch");
        assert_eq!(dispatch.window_id, 7);
        assert!(dispatch.command.is_none());
        assert!(manager.activation_intent(7).is_none());
    }

    #[test]
    fn right_drag_intent_enforces_recognized_activating_pending_execution_order() {
        let mut manager = DetachedWindowManager::new();
        let command = short_click_command();
        assert!(
            manager
                .queue_right_drag_command(9, command.clone(), false)
                .0
        );
        assert!(matches!(
            manager.activation_intent(9),
            Some(DetachedActivationIntent::RightDrag(
                PendingRightDragCommand::Recognized { .. }
            ))
        ));

        let dispatch = manager
            .take_pending_deferred_activation()
            .expect("recognized dispatch");
        assert_eq!(dispatch.command, Some(command.clone()));
        assert!(matches!(
            manager.activation_intent(9),
            Some(DetachedActivationIntent::RightDrag(
                PendingRightDragCommand::Activating { .. }
            ))
        ));
        assert!(manager.mark_right_drag_pending_execution(9, command.clone()));
        assert!(matches!(
            manager.activation_intent(9),
            Some(DetachedActivationIntent::RightDrag(
                PendingRightDragCommand::PendingExecution { .. }
            ))
        ));
        assert_eq!(manager.take_pending_right_drag_execution(9), Some(command));
        assert!(manager.activation_intent(9).is_none());
    }

    #[test]
    fn detached_input_surface_prefers_foreground_then_last_touched() {
        let mut manager = DetachedWindowManager::new();
        assert_eq!(
            manager.resolve_input_surface(true, Some(10), Some(10), true),
            ActionSurface::MainWindow
        );
        assert_eq!(
            manager.resolve_input_surface(true, Some(10), Some(20), true),
            ActionSurface::Viewer
        );

        manager.note_input_surface(ActionSurface::Viewer);
        assert_eq!(
            manager.resolve_input_surface(true, Some(10), None, true),
            ActionSurface::Viewer
        );
    }

    #[test]
    fn single_surface_ignores_stale_last_touched_value() {
        let mut manager = DetachedWindowManager::new();
        manager.note_input_surface(ActionSurface::Viewer);
        assert_eq!(
            manager.resolve_input_surface(false, Some(10), Some(10), false),
            ActionSurface::MainWindow
        );
        assert_eq!(
            manager.resolve_input_surface(false, Some(10), Some(10), true),
            ActionSurface::Viewer
        );
    }
}
