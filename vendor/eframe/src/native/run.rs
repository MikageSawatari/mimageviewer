use std::time::{Duration, Instant};

use winit::{
    application::ApplicationHandler,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

use ahash::HashMap;

use super::winit_integration::{UserEvent, WinitApp};
use crate::{
    Result, epi,
    native::{
        event_loop_context,
        winit_integration::{
            EventResult, RenderTarget, RepaintNowReason, is_invisible_or_minimized,
        },
    },
};

/// Minimum interval between repaints for invisible windows.
///
/// On Windows, invisible windows don't receive `RedrawRequested` events,
/// so we throttle their repaints to avoid busy-looping while still
/// processing viewport commands like `Visible(true)`.
/// See <https://github.com/emilk/egui/issues/7776>.
const INVISIBLE_WINDOW_REPAINT_INTERVAL: Duration = Duration::from_millis(100);

fn throttle_invisible_repaint_time(when: Instant, now: Instant) -> Instant {
    when.max(now + INVISIBLE_WINDOW_REPAINT_INTERVAL)
}

#[cfg(test)]
fn throttle_existing_repaint<K>(repaint_times: &mut HashMap<K, Instant>, window_id: K, now: Instant)
where
    K: std::hash::Hash + Eq,
{
    repaint_times
        .entry(window_id)
        .and_modify(|time| *time = throttle_invisible_repaint_time(*time, now));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SchedulerTarget<W, V> {
    window_id: W,
    viewport_id: V,
    surface_generation: u64,
    size: [u32; 2],
}

impl From<RenderTarget> for SchedulerTarget<WindowId, egui::ViewportId> {
    fn from(target: RenderTarget) -> Self {
        Self {
            window_id: target.window_id,
            viewport_id: target.viewport_id,
            surface_generation: target.surface_generation,
            size: [target.size.width, target.size.height],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DamageReason {
    RedrawRequested,
    ScheduledRepaint,
    Bootstrap,
    AccessKit,
    InteractiveResize,
    #[cfg(test)]
    ReadinessWake,
}

impl From<RepaintNowReason> for DamageReason {
    fn from(reason: RepaintNowReason) -> Self {
        match reason {
            RepaintNowReason::Bootstrap => Self::Bootstrap,
            RepaintNowReason::AccessKit => Self::AccessKit,
            RepaintNowReason::InteractiveResize => Self::InteractiveResize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderPhase {
    MessageDispatch,
    Outer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirtyFrame<W, V> {
    target: SchedulerTarget<W, V>,
    reason: DamageReason,
    eligible_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowRenderState<W, V> {
    Dirty(DirtyFrame<W, V>),
    Painting {
        target: SchedulerTarget<W, V>,
        pending: Option<DirtyFrame<W, V>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PaintClaim<W, V> {
    target: SchedulerTarget<W, V>,
    reason: DamageReason,
    epoch: u64,
}

/// Painter-independent reducer for scheduled repaint times and per-window dirty state.
///
/// A Painting state is owned by one window only. Immediate viewport recursion remains inside
/// the parent's renderer call and never consults this scheduler, so it cannot be confused with
/// a second scheduler claim.
struct RenderScheduler<W, V> {
    windows_next_repaint_times: HashMap<W, Instant>,
    window_states: HashMap<W, WindowRenderState<W, V>>,
    outer_epoch: u64,
}

impl<W, V> Default for RenderScheduler<W, V> {
    fn default() -> Self {
        Self {
            windows_next_repaint_times: HashMap::default(),
            window_states: HashMap::default(),
            outer_epoch: 0,
        }
    }
}

impl<W, V> RenderScheduler<W, V>
where
    W: Copy + Eq + std::hash::Hash,
    V: Copy,
{
    fn schedule_repaint(
        &mut self,
        window_id: W,
        when: Instant,
        invisible_or_minimized: bool,
        now: Instant,
    ) {
        let when = if invisible_or_minimized {
            throttle_invisible_repaint_time(when, now)
        } else {
            when
        };
        self.windows_next_repaint_times
            .entry(window_id)
            .and_modify(|current| *current = (*current).min(when))
            .or_insert(when);
    }

    fn take_due_repaints(&mut self, now: Instant) -> Vec<W> {
        let mut due = Vec::new();
        self.windows_next_repaint_times
            .retain(|window_id, repaint_time| {
                if *repaint_time <= now {
                    due.push(*window_id);
                    false
                } else {
                    true
                }
            });
        due
    }

    fn next_repaint_time(&self) -> Option<Instant> {
        self.windows_next_repaint_times.values().min().copied()
    }

    fn record_damage(&mut self, target: SchedulerTarget<W, V>, reason: DamageReason) {
        use std::collections::hash_map::Entry;

        let window_id = target.window_id;
        match self.window_states.entry(window_id) {
            Entry::Vacant(entry) => {
                entry.insert(WindowRenderState::Dirty(DirtyFrame {
                    target,
                    reason,
                    eligible_epoch: self.outer_epoch,
                }));
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                WindowRenderState::Dirty(dirty) => {
                    dirty.target = target;
                    if reason == DamageReason::Bootstrap {
                        dirty.reason = reason;
                    }
                }
                WindowRenderState::Painting { pending, .. } => {
                    let next_epoch = self.outer_epoch.wrapping_add(1);
                    if let Some(dirty) = pending {
                        dirty.target = target;
                        if reason == DamageReason::Bootstrap {
                            dirty.reason = reason;
                        }
                        dirty.eligible_epoch = dirty.eligible_epoch.min(next_epoch);
                    } else {
                        *pending = Some(DirtyFrame {
                            target,
                            reason,
                            eligible_epoch: next_epoch,
                        });
                    }
                }
            },
        }
    }

    fn route_repaint_now(
        &mut self,
        target: SchedulerTarget<W, V>,
        reason: RepaintNowReason,
        phase: RenderPhase,
    ) -> Option<PaintClaim<W, V>> {
        if reason == RepaintNowReason::InteractiveResize && phase == RenderPhase::MessageDispatch {
            self.claim_inline(target, DamageReason::InteractiveResize)
        } else {
            self.record_damage(target, reason.into());
            None
        }
    }

    fn claim_inline(
        &mut self,
        target: SchedulerTarget<W, V>,
        reason: DamageReason,
    ) -> Option<PaintClaim<W, V>> {
        use std::collections::hash_map::Entry;

        match self.window_states.entry(target.window_id) {
            Entry::Vacant(entry) => {
                entry.insert(WindowRenderState::Painting {
                    target,
                    pending: None,
                });
                Some(PaintClaim {
                    target,
                    reason,
                    epoch: self.outer_epoch,
                })
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                WindowRenderState::Dirty(_) => {
                    entry.insert(WindowRenderState::Painting {
                        target,
                        pending: None,
                    });
                    Some(PaintClaim {
                        target,
                        reason,
                        epoch: self.outer_epoch,
                    })
                }
                WindowRenderState::Painting { pending, .. } => {
                    let next_epoch = self.outer_epoch.wrapping_add(1);
                    *pending = Some(DirtyFrame {
                        target,
                        reason,
                        eligible_epoch: next_epoch,
                    });
                    None
                }
            },
        }
    }

    fn begin_outer_phase(&mut self) -> u64 {
        self.outer_epoch = self.outer_epoch.wrapping_add(1);
        self.outer_epoch
    }

    fn claim_next_outer(&mut self, epoch: u64) -> Option<PaintClaim<W, V>> {
        let window_id = self.window_states.iter().find_map(|(window_id, state)| {
            matches!(
                state,
                WindowRenderState::Dirty(DirtyFrame { eligible_epoch, .. })
                    if *eligible_epoch <= epoch
            )
            .then_some(*window_id)
        })?;
        let state = self.window_states.remove(&window_id)?;
        let WindowRenderState::Dirty(dirty) = state else {
            return None;
        };
        self.window_states.insert(
            window_id,
            WindowRenderState::Painting {
                target: dirty.target,
                pending: None,
            },
        );
        Some(PaintClaim {
            target: dirty.target,
            reason: dirty.reason,
            epoch,
        })
    }

    fn finish_paint(&mut self, claim: PaintClaim<W, V>) {
        let state = self.window_states.remove(&claim.target.window_id);
        let Some(WindowRenderState::Painting { target, pending }) = state else {
            return;
        };
        debug_assert_eq!(target.surface_generation, claim.target.surface_generation);
        if let Some(pending) = pending {
            self.window_states
                .insert(claim.target.window_id, WindowRenderState::Dirty(pending));
        }
    }

    fn promote_immediate_child(
        &mut self,
        parent: SchedulerTarget<W, V>,
        invisible_or_minimized: bool,
        now: Instant,
    ) {
        self.schedule_repaint(parent.window_id, now, invisible_or_minimized, now);
    }

    fn close_window(&mut self, window_id: W) {
        self.windows_next_repaint_times.remove(&window_id);
        self.window_states.remove(&window_id);
    }

    fn clear(&mut self) {
        self.windows_next_repaint_times.clear();
        self.window_states.clear();
    }

    fn has_dirty_work(&self) -> bool {
        self.window_states
            .values()
            .any(|state| matches!(state, WindowRenderState::Dirty(_)))
    }
}

// ----------------------------------------------------------------------------
fn create_event_loop(native_options: &mut epi::NativeOptions) -> Result<EventLoop<UserEvent>> {
    #[cfg(target_os = "android")]
    use winit::platform::android::EventLoopBuilderExtAndroid as _;

    profiling::function_scope!();
    let mut builder = winit::event_loop::EventLoop::with_user_event();

    #[cfg(target_os = "android")]
    let mut builder =
        builder.with_android_app(native_options.android_app.take().ok_or_else(|| {
            crate::Error::AppCreation(Box::from(
                "`NativeOptions` is missing required `android_app`",
            ))
        })?);

    if let Some(hook) = std::mem::take(&mut native_options.event_loop_builder) {
        hook(&mut builder);
    }

    profiling::scope!("EventLoopBuilder::build");
    Ok(builder.build()?)
}

/// Access a thread-local event loop.
///
/// We reuse the event-loop so we can support closing and opening an eframe window
/// multiple times. This is just a limitation of winit.
#[cfg(not(target_os = "ios"))]
fn with_event_loop<R>(
    mut native_options: epi::NativeOptions,
    f: impl FnOnce(&mut EventLoop<UserEvent>, epi::NativeOptions) -> R,
) -> Result<R> {
    thread_local!(static EVENT_LOOP: std::cell::RefCell<Option<EventLoop<UserEvent>>> = const { std::cell::RefCell::new(None) });

    EVENT_LOOP.with(|event_loop| {
        // Since we want to reference NativeOptions when creating the EventLoop we can't
        // do that as part of the lazy thread local storage initialization and so we instead
        // create the event loop lazily here
        let mut event_loop_lock = event_loop.borrow_mut();
        let event_loop = if let Some(event_loop) = &mut *event_loop_lock {
            event_loop
        } else {
            event_loop_lock.insert(create_event_loop(&mut native_options)?)
        };
        Ok(f(event_loop, native_options))
    })
}

/// Wraps a [`WinitApp`] to implement [`ApplicationHandler`]. This handles redrawing, exit states, and
/// some events, but otherwise forwards events to the [`WinitApp`].
struct WinitAppWrapper<T: WinitApp> {
    render_scheduler: RenderScheduler<WindowId, egui::ViewportId>,
    winit_app: T,
    return_result: Result<(), crate::Error>,
    run_and_return: bool,
}

impl<T: WinitApp> WinitAppWrapper<T> {
    fn new(winit_app: T, run_and_return: bool) -> Self {
        Self {
            render_scheduler: RenderScheduler::default(),
            winit_app,
            return_result: Ok(()),
            run_and_return,
        }
    }

    fn window_is_invisible_or_minimized(&self, window_id: WindowId) -> bool {
        self.winit_app
            .window(window_id)
            .is_some_and(|window| is_invisible_or_minimized(&window))
    }

    fn schedule_repaint(&mut self, window_id: WindowId, when: Instant) {
        let hidden = self.window_is_invisible_or_minimized(window_id);
        self.render_scheduler
            .schedule_repaint(window_id, when, hidden, Instant::now());
    }

    fn handle_event_result(
        &mut self,
        event_loop: &ActiveEventLoop,
        event_result: Result<EventResult>,
        phase: RenderPhase,
        mut paint_origin: Option<WindowId>,
    ) {
        let mut event_result = event_result;
        loop {
            let mut exit = false;
            let mut save = false;
            let mut inline_claim = None;
            log::trace!("event_result: {event_result:?}");

            let combined_result = event_result.map(|event_result| match event_result {
                EventResult::Wait => event_result,
                EventResult::RepaintNow(window_id) => {
                    // Compatibility result from a legacy backend. Known bootstrap and resize
                    // producers are typed by their owning callback before reaching this point.
                    self.schedule_repaint(window_id, Instant::now());
                    event_result
                }
                EventResult::RepaintNowWithReason(request) => {
                    if let Some(target) = self.winit_app.render_target(request.window_id) {
                        let phase = if cfg!(target_os = "windows") {
                            phase
                        } else {
                            RenderPhase::Outer
                        };
                        inline_claim = self.render_scheduler.route_repaint_now(
                            target.into(),
                            request.reason,
                            phase,
                        );
                    } else {
                        self.render_scheduler.close_window(request.window_id);
                    }
                    event_result
                }
                EventResult::RepaintNext(window_id) => {
                    log::trace!("RepaintNext of {window_id:?}",);
                    let now = Instant::now();
                    if paint_origin.is_some_and(|origin| origin != window_id)
                        && let Some(parent) = self.winit_app.render_target(window_id)
                    {
                        let hidden = self.window_is_invisible_or_minimized(window_id);
                        self.render_scheduler
                            .promote_immediate_child(parent.into(), hidden, now);
                    } else {
                        self.schedule_repaint(window_id, now);
                    }
                    event_result
                }
                EventResult::RepaintAt(window_id, repaint_time) => {
                    self.schedule_repaint(window_id, repaint_time);
                    event_result
                }
                EventResult::Save => {
                    save = true;
                    event_result
                }
                EventResult::Exit => {
                    exit = true;
                    event_result
                }
                EventResult::CloseRequested => {
                    // The windows need to be dropped whilst the event loop is running to allow for proper cleanup.
                    self.winit_app.save_and_destroy();
                    self.render_scheduler.clear();
                    event_result
                }
            });

            if let Err(err) = combined_result {
                log::error!("Exiting because of error: {err}");
                exit = true;
                self.return_result = Err(err);
            }

            if let Some(claim) = inline_claim {
                log::trace!(
                    "inline resize paint begin: window={:?} viewport={:?} generation={} size={:?}",
                    claim.target.window_id,
                    claim.target.viewport_id,
                    claim.target.surface_generation,
                    claim.target.size
                );
                event_result = self
                    .winit_app
                    .run_ui_and_paint(event_loop, claim.target.window_id);
                self.render_scheduler.finish_paint(claim);
                log::trace!(
                    "inline resize paint end: window={:?} viewport={:?} generation={}",
                    claim.target.window_id,
                    claim.target.viewport_id,
                    claim.target.surface_generation
                );
                paint_origin = Some(claim.target.window_id);
                continue;
            }

            if save {
                log::debug!("Received an EventResult::Save - saving app state");
                self.winit_app.save();
            }

            if exit {
                if self.run_and_return {
                    log::debug!("Asking to exit event loop…");
                    event_loop.exit();
                } else {
                    log::debug!("Quitting - saving app state…");
                    self.winit_app.save_and_destroy();

                    log::debug!("Exiting with return code 0");

                    std::process::exit(0);
                }
            }

            break;
        }

        let requested_redraw = self.schedule_redraw_requests(event_loop);
        self.apply_control_flow(event_loop, requested_redraw);
    }

    fn schedule_redraw_requests(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        let now = Instant::now();
        let mut requested_redraw = false;
        for window_id in self.render_scheduler.take_due_repaints(now) {
            let Some(window) = self.winit_app.window(window_id) else {
                log::trace!("No window found for {window_id:?}");
                self.render_scheduler.close_window(window_id);
                continue;
            };
            if is_invisible_or_minimized(&window) {
                // Windows does not deliver RedrawRequested to hidden/minimized windows.
                // Only an already-requested due item becomes dirty here; no idle heartbeat
                // is manufactured.
                if let Some(target) = self.winit_app.render_target(window_id) {
                    self.render_scheduler
                        .record_damage(target.into(), DamageReason::ScheduledRepaint);
                }
            } else {
                log::trace!("request_redraw for {window_id:?}");
                window.request_redraw();
                requested_redraw = true;
            }
        }
        requested_redraw
    }

    fn drain_outer_paints(&mut self, event_loop: &ActiveEventLoop) {
        let epoch = self.render_scheduler.begin_outer_phase();
        while let Some(claim) = self.render_scheduler.claim_next_outer(epoch) {
            let Some(current_target) = self.winit_app.render_target(claim.target.window_id) else {
                self.render_scheduler.finish_paint(claim);
                self.render_scheduler.close_window(claim.target.window_id);
                continue;
            };
            let current_target: SchedulerTarget<WindowId, egui::ViewportId> = current_target.into();
            if current_target != claim.target {
                self.render_scheduler.finish_paint(claim);
                self.render_scheduler
                    .record_damage(current_target, claim.reason);
                continue;
            }

            log::trace!(
                "outer paint begin: window={:?} viewport={:?} generation={} size={:?} reason={:?}",
                claim.target.window_id,
                claim.target.viewport_id,
                claim.target.surface_generation,
                claim.target.size,
                claim.reason
            );
            let event_result = self
                .winit_app
                .run_ui_and_paint(event_loop, claim.target.window_id);
            self.render_scheduler.finish_paint(claim);
            log::trace!(
                "outer paint end: window={:?} viewport={:?} generation={}",
                claim.target.window_id,
                claim.target.viewport_id,
                claim.target.surface_generation
            );
            self.handle_event_result(
                event_loop,
                event_result,
                RenderPhase::Outer,
                Some(claim.target.window_id),
            );
        }
    }

    fn apply_control_flow(&self, event_loop: &ActiveEventLoop, requested_redraw: bool) {
        if requested_redraw || self.render_scheduler.has_dirty_work() {
            event_loop.set_control_flow(ControlFlow::Poll);
        } else if let Some(next_repaint_time) = self.render_scheduler.next_repaint_time() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(next_repaint_time));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestScheduler = RenderScheduler<u8, u8>;

    fn target(window_id: u8, viewport_id: u8, surface_generation: u64) -> SchedulerTarget<u8, u8> {
        SchedulerTarget {
            window_id,
            viewport_id,
            surface_generation,
            size: [640 + u32::from(window_id), 480],
        }
    }

    #[cfg(all(target_os = "windows", feature = "wgpu"))]
    #[allow(unsafe_code)]
    mod windows_process_test {
        use std::{
            io::{BufRead as _, BufReader, Read as _, Write as _},
            net::{TcpListener, TcpStream},
            process::{Child, Command, Stdio},
            sync::{
                atomic::{AtomicIsize, Ordering},
                mpsc,
            },
            thread,
            time::{Duration, Instant},
        };

        use raw_window_handle::RawWindowHandle;
        use windows_sys::Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::{RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow},
            System::Performance::QueryPerformanceCounter,
            UI::WindowsAndMessaging::{
                CallWindowProcW, DefWindowProcW, FindWindowW, GWLP_WNDPROC, PostMessageW,
                SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW,
                SetWindowLongPtrW, WM_APP, WM_CLOSE, WNDPROC,
            },
        };

        const TEST_DAMAGE_MESSAGE: u32 = WM_APP + 0x31A;
        const CHILD_ROLE_ENV: &str = "EFRAME_RENDER_PHASE_TEST_CHILD";
        const GATE_PORT_ENV: &str = "EFRAME_RENDER_PHASE_TEST_GATE_PORT";
        const ACK_PORT_ENV: &str = "EFRAME_RENDER_PHASE_TEST_ACK_PORT";
        const TITLE_ENV: &str = "EFRAME_RENDER_PHASE_TEST_TITLE";
        static PREVIOUS_WNDPROC: AtomicIsize = AtomicIsize::new(0);

        unsafe extern "system" fn test_wndproc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            if message == TEST_DAMAGE_MESSAGE {
                crate::native::wgpu_integration::render_phase_test_gate::arm();
                unsafe {
                    RedrawWindow(
                        hwnd,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        RDW_INVALIDATE | RDW_UPDATENOW,
                    );
                }
                return 0;
            }

            let previous = PREVIOUS_WNDPROC.load(Ordering::Acquire);
            if previous == 0 {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            let previous: WNDPROC = unsafe { std::mem::transmute(previous) };
            unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
        }

        fn performance_counter() -> i64 {
            let mut counter = 0;
            assert_ne!(unsafe { QueryPerformanceCounter(&mut counter) }, 0);
            counter
        }

        fn accept_until(
            listener: &TcpListener,
            deadline: Instant,
        ) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
            listener.set_nonblocking(true)?;
            loop {
                match listener.accept() {
                    Ok(connection) => return Ok(connection),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "timed out accepting test connection",
                            ));
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        fn find_window(title: &str, deadline: Instant) -> Option<isize> {
            let wide_title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            loop {
                let hwnd = unsafe { FindWindowW(std::ptr::null(), wide_title.as_ptr()) };
                if !hwnd.is_null() {
                    return Some(hwnd as isize);
                }
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }

        fn wait_for_child(
            child: &mut Child,
            deadline: Instant,
        ) -> Option<std::process::ExitStatus> {
            loop {
                if let Some(status) = child.try_wait().expect("query child process") {
                    return Some(status);
                }
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn run_child() {
            let title = std::env::var(TITLE_ENV).expect("child title");
            let native_options = crate::NativeOptions {
                renderer: crate::Renderer::Wgpu,
                run_and_return: true,
                event_loop_builder: Some(Box::new(|builder| {
                    use winit::platform::windows::EventLoopBuilderExtWindows as _;
                    builder.with_any_thread(true);
                })),
                viewport: egui::ViewportBuilder::default()
                    .with_title(title.clone())
                    .with_inner_size([320.0, 200.0]),
                ..Default::default()
            };

            struct TestApp;
            impl crate::App for TestApp {
                fn update(&mut self, _ctx: &egui::Context, _frame: &mut crate::Frame) {}
            }

            crate::run_native(
                &title,
                native_options,
                Box::new(|creation_context| {
                    let raw_handle = creation_context
                        .raw_window_handle
                        .as_ref()
                        .expect("test window handle");
                    let RawWindowHandle::Win32(handle) = raw_handle else {
                        panic!("Windows process test requires a Win32 window");
                    };
                    let hwnd = handle.hwnd.get() as HWND;
                    let previous = unsafe {
                        SetWindowLongPtrW(
                            hwnd,
                            GWLP_WNDPROC,
                            test_wndproc as *const () as usize as isize,
                        )
                    };
                    assert_ne!(previous, 0, "subclass test window");
                    PREVIOUS_WNDPROC.store(previous, Ordering::Release);
                    Ok(Box::new(TestApp))
                }),
            )
            .expect("run child eframe window");
        }

        #[test]
        fn synchronous_damage_returns_before_outer_paint_gate() {
            if std::env::var(CHILD_ROLE_ENV).as_deref() == Ok("1") {
                run_child();
                return;
            }

            let total_deadline = Instant::now() + Duration::from_secs(15);
            let gate_listener =
                TcpListener::bind(("127.0.0.1", 0)).expect("bind painter gate listener");
            let gate_port = gate_listener.local_addr().unwrap().port();
            let ack_listener =
                TcpListener::bind(("127.0.0.1", 0)).expect("bind sender acknowledgement listener");
            let ack_port = ack_listener.local_addr().unwrap().port();
            let title = format!(
                "eframe-render-phase-test-{}-{gate_port}",
                std::process::id()
            );

            let mut child = Command::new(std::env::current_exe().expect("current test executable"))
                .arg("--exact")
                .arg("native::run::tests::windows_process_test::synchronous_damage_returns_before_outer_paint_gate")
                .arg("--nocapture")
                .env(CHILD_ROLE_ENV, "1")
                .env(GATE_PORT_ENV, gate_port.to_string())
                .env(ACK_PORT_ENV, ack_port.to_string())
                .env(TITLE_ENV, &title)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn eframe render phase child");

            let (gate_stream, _) = match accept_until(&gate_listener, total_deadline) {
                Ok(connection) => connection,
                Err(error) => {
                    let status = child.try_wait().expect("query failed render phase child");
                    if status.is_none() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    let mut stderr_text = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut stderr_text);
                    }
                    panic!(
                        "child painter gate connection failed: {error}; status={status:?}; stderr={stderr_text}"
                    );
                }
            };
            gate_stream
                .set_nonblocking(false)
                .expect("restore blocking painter gate stream");
            gate_stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("set painter gate read timeout");
            let mut gate_release = gate_stream.try_clone().expect("clone painter gate stream");
            let mut gate_reader = BufReader::new(gate_stream);
            let mut ready = String::new();
            gate_reader
                .read_line(&mut ready)
                .expect("read bootstrap-ready marker");
            assert_eq!(ready.trim(), "READY");

            let hwnd = find_window(&title, total_deadline).expect("find child eframe HWND");
            let (sender_tx, sender_rx) = mpsc::channel();
            let sender = thread::spawn(move || {
                let mut message_result = 0_usize;
                let send_result = unsafe {
                    SendMessageTimeoutW(
                        hwnd as HWND,
                        TEST_DAMAGE_MESSAGE,
                        0,
                        0,
                        SMTO_BLOCK | SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                        500,
                        &mut message_result,
                    )
                };
                let returned_at = performance_counter();
                let ack_result =
                    accept_until(&ack_listener, Instant::now() + Duration::from_secs(3))
                        .and_then(|(mut stream, _)| stream.write_all(&[1_u8]));
                sender_tx
                    .send((send_result, returned_at, ack_result))
                    .expect("report SendMessageTimeout result");
            });

            let mut gate_line = String::new();
            let gate_read_result = gate_reader.read_line(&mut gate_line);
            let entered_at = gate_line
                .trim()
                .strip_prefix("GATE_ENTER ")
                .and_then(|value| value.parse::<i64>().ok());

            // Always release the deliberately blocked renderer before inspecting failures.
            let _ = gate_release.write_all(&[1_u8]);
            unsafe {
                PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0);
            }
            let sender_result = sender_rx.recv_timeout(Duration::from_secs(4));
            sender.join().expect("join SendMessageTimeout sender");

            let child_status = wait_for_child(&mut child, total_deadline);
            if child_status.is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let mut child_stderr = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut child_stderr);
            }

            assert!(
                gate_read_result.is_ok(),
                "outer paint gate was not reached: {gate_read_result:?}; child stderr: {child_stderr}"
            );
            let entered_at = entered_at.expect("parse outer paint gate timestamp");
            let (send_result, returned_at, ack_result) =
                sender_result.expect("SendMessageTimeout sender watchdog");
            assert_ne!(
                send_result, 0,
                "SendMessageTimeoutW timed out before message dispatch returned; child stderr: {child_stderr}"
            );
            assert!(
                ack_result.is_ok(),
                "release child acknowledgement: {ack_result:?}"
            );
            assert!(
                returned_at < entered_at,
                "SendMessageTimeoutW returned at {returned_at}, outer paint gate entered at {entered_at}"
            );
            assert!(
                child_status.is_some_and(|status| status.success()),
                "render phase child did not exit successfully: {child_status:?}; stderr: {child_stderr}"
            );
        }
    }

    #[test]
    fn redraw_requested_records_dirty_without_paint_claim() {
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(1, 11, 1), DamageReason::RedrawRequested);

        assert!(matches!(
            scheduler.window_states.get(&1),
            Some(WindowRenderState::Dirty(_))
        ));
    }

    #[test]
    fn same_window_damage_coalesces_to_latest_surface() {
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(1, 11, 1), DamageReason::RedrawRequested);
        scheduler.record_damage(target(1, 11, 2), DamageReason::ScheduledRepaint);

        assert_eq!(scheduler.window_states.len(), 1);
        let Some(WindowRenderState::Dirty(dirty)) = scheduler.window_states.get(&1) else {
            panic!("coalesced window must remain dirty");
        };
        assert_eq!(dirty.target.surface_generation, 2);
    }

    #[test]
    fn dirty_state_is_independent_per_window() {
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(1, 11, 1), DamageReason::RedrawRequested);
        scheduler.record_damage(target(2, 22, 7), DamageReason::RedrawRequested);

        assert_eq!(scheduler.window_states.len(), 2);
        scheduler.close_window(1);
        assert!(scheduler.window_states.contains_key(&2));
    }

    #[test]
    fn ordinary_work_can_only_be_claimed_by_outer_phase() {
        let mut scheduler = TestScheduler::default();
        let inline = scheduler.route_repaint_now(
            target(1, 11, 1),
            RepaintNowReason::AccessKit,
            RenderPhase::MessageDispatch,
        );

        assert!(inline.is_none());
        let epoch = scheduler.begin_outer_phase();
        assert!(scheduler.claim_next_outer(epoch).is_some());
    }

    #[test]
    fn repaint_requested_while_painting_waits_for_next_epoch() {
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(1, 11, 1), DamageReason::RedrawRequested);
        let epoch = scheduler.begin_outer_phase();
        let claim = scheduler
            .claim_next_outer(epoch)
            .expect("initial damage must be claimable");

        scheduler.record_damage(target(1, 11, 1), DamageReason::RedrawRequested);
        scheduler.finish_paint(claim);

        assert!(scheduler.claim_next_outer(epoch).is_none());
        let next_epoch = scheduler.begin_outer_phase();
        assert!(scheduler.claim_next_outer(next_epoch).is_some());
    }

    #[test]
    fn hidden_requested_work_is_throttled_to_100_ms() {
        let now = Instant::now();
        let mut scheduler = TestScheduler::default();
        scheduler.schedule_repaint(1, now, true, now);

        assert!(scheduler.take_due_repaints(now).is_empty());
        assert_eq!(
            scheduler.take_due_repaints(now + INVISIBLE_WINDOW_REPAINT_INTERVAL),
            vec![1]
        );
    }

    #[test]
    fn idle_hidden_scheduler_does_not_create_a_heartbeat() {
        let scheduler = TestScheduler::default();

        assert!(scheduler.next_repaint_time().is_none());
        assert!(!scheduler.has_dirty_work());
    }

    #[test]
    fn closing_window_discards_dirty_schedule_and_readiness_wake() {
        let now = Instant::now();
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(1, 11, 3), DamageReason::ReadinessWake);
        scheduler.schedule_repaint(1, now, false, now);

        scheduler.close_window(1);

        assert!(scheduler.window_states.is_empty());
        assert!(scheduler.windows_next_repaint_times.is_empty());
    }

    #[test]
    fn immediate_child_promotes_repaint_to_parent_window() {
        let now = Instant::now();
        let mut scheduler = TestScheduler::default();
        scheduler.record_damage(target(2, 22, 1), DamageReason::RedrawRequested);
        scheduler.close_window(2);

        scheduler.promote_immediate_child(target(1, 11, 4), false, now);

        assert_eq!(scheduler.take_due_repaints(now), vec![1]);
        assert!(!scheduler.window_states.contains_key(&2));
    }

    #[test]
    fn hidden_bootstrap_bypasses_requested_work_throttle() {
        let mut scheduler = TestScheduler::default();
        let inline = scheduler.route_repaint_now(
            target(1, 11, 1),
            RepaintNowReason::Bootstrap,
            RenderPhase::MessageDispatch,
        );

        assert!(inline.is_none());
        assert!(scheduler.next_repaint_time().is_none());
        let epoch = scheduler.begin_outer_phase();
        let claim = scheduler
            .claim_next_outer(epoch)
            .expect("bootstrap must drain in the first outer phase");
        assert_eq!(claim.reason, DamageReason::Bootstrap);
    }

    #[test]
    fn only_interactive_resize_reason_is_allowed_inline() {
        let mut bootstrap = TestScheduler::default();
        assert!(
            bootstrap
                .route_repaint_now(
                    target(1, 11, 1),
                    RepaintNowReason::Bootstrap,
                    RenderPhase::MessageDispatch,
                )
                .is_none()
        );

        let mut accesskit = TestScheduler::default();
        assert!(
            accesskit
                .route_repaint_now(
                    target(2, 22, 1),
                    RepaintNowReason::AccessKit,
                    RenderPhase::MessageDispatch,
                )
                .is_none()
        );

        let mut resize = TestScheduler::default();
        assert!(
            resize
                .route_repaint_now(
                    target(3, 33, 1),
                    RepaintNowReason::InteractiveResize,
                    RenderPhase::MessageDispatch,
                )
                .is_some()
        );

        let mut outer_resize = TestScheduler::default();
        assert!(
            outer_resize
                .route_repaint_now(
                    target(4, 44, 1),
                    RepaintNowReason::InteractiveResize,
                    RenderPhase::Outer,
                )
                .is_none()
        );
    }

    #[test]
    fn immediate_invisible_repaint_is_delayed_to_minimum_interval() {
        let now = Instant::now();
        let mut repaint_times = HashMap::default();
        repaint_times.insert(1_u8, now);

        throttle_existing_repaint(&mut repaint_times, 1, now);

        assert_eq!(
            repaint_times.get(&1),
            Some(&(now + INVISIBLE_WINDOW_REPAINT_INTERVAL))
        );
    }

    #[test]
    fn later_invisible_repaint_is_not_accelerated() {
        let now = Instant::now();
        let later = now + Duration::from_secs(5);
        let mut repaint_times = HashMap::default();
        repaint_times.insert(1_u8, later);

        throttle_existing_repaint(&mut repaint_times, 1, now);

        assert_eq!(repaint_times.get(&1), Some(&later));
    }

    #[test]
    fn idle_invisible_window_does_not_gain_a_heartbeat() {
        let now = Instant::now();
        let mut repaint_times = HashMap::<u8, Instant>::default();

        throttle_existing_repaint(&mut repaint_times, 1, now);

        assert!(repaint_times.is_empty());
    }
}

impl<T: WinitApp> ApplicationHandler<UserEvent> for WinitAppWrapper<T> {
    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        profiling::scope!("Event::Suspended");

        event_loop_context::with_event_loop_context(event_loop, move || {
            let event_result = self.winit_app.suspended(event_loop);
            self.handle_event_result(event_loop, event_result, RenderPhase::MessageDispatch, None);
        });
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        profiling::scope!("Event::Resumed");

        // Nb: Make sure this guard is dropped after this function returns.
        event_loop_context::with_event_loop_context(event_loop, move || {
            let event_result = self
                .winit_app
                .resumed(event_loop)
                .map(|result| result.with_repaint_now_reason(RepaintNowReason::Bootstrap));
            self.handle_event_result(event_loop, event_result, RenderPhase::MessageDispatch, None);
        });
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        // On Mac, Cmd-Q we get here and then `run_app_on_demand` doesn't return (despite its name),
        // so we need to save state now:
        log::debug!("Received Event::LoopExiting - saving app state…");
        event_loop_context::with_event_loop_context(event_loop, move || {
            self.winit_app.save_and_destroy();
        });
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        profiling::function_scope!(egui_winit::short_device_event_description(&event));

        // Nb: Make sure this guard is dropped after this function returns.
        event_loop_context::with_event_loop_context(event_loop, move || {
            let event_result = self.winit_app.device_event(event_loop, device_id, event);
            self.handle_event_result(event_loop, event_result, RenderPhase::MessageDispatch, None);
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        profiling::function_scope!(match &event {
            UserEvent::RequestRepaint { .. } => "UserEvent::RequestRepaint",
            #[cfg(feature = "accesskit")]
            UserEvent::AccessKitActionRequest(_) => "UserEvent::AccessKitActionRequest",
        });

        event_loop_context::with_event_loop_context(event_loop, move || {
            let event_result = match event {
                UserEvent::RequestRepaint {
                    when,
                    cumulative_pass_nr,
                    viewport_id,
                } => {
                    let current_pass_nr = self
                        .winit_app
                        .egui_ctx()
                        .map_or(0, |ctx| ctx.cumulative_pass_nr_for(viewport_id));
                    if current_pass_nr == cumulative_pass_nr
                        || current_pass_nr == cumulative_pass_nr + 1
                    {
                        log::trace!("UserEvent::RequestRepaint scheduling repaint at {when:?}");
                        if let Some(window_id) =
                            self.winit_app.window_id_from_viewport_id(viewport_id)
                        {
                            Ok(EventResult::RepaintAt(window_id, when))
                        } else {
                            Ok(EventResult::Wait)
                        }
                    } else {
                        log::trace!("Got outdated UserEvent::RequestRepaint");
                        Ok(EventResult::Wait) // old request - we've already repainted
                    }
                }
                #[cfg(feature = "accesskit")]
                UserEvent::AccessKitActionRequest(request) => {
                    self.winit_app.on_accesskit_event(request)
                }
            };
            self.handle_event_result(event_loop, event_result, RenderPhase::MessageDispatch, None);
        });
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if let winit::event::StartCause::ResumeTimeReached { .. } = cause {
            log::trace!("Woke up to check next_repaint_time");
        }

        let requested_redraw = self.schedule_redraw_requests(event_loop);
        self.apply_control_flow(event_loop, requested_redraw);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop_context::with_event_loop_context(event_loop, move || {
            // This is the first callback whose ownership is outside all message-dispatch
            // callbacks. Scheduling OS redraw is safe in either phase; only this phase may
            // drain ordinary dirty work into the renderer.
            let requested_before = self.schedule_redraw_requests(event_loop);
            self.drain_outer_paints(event_loop);
            let requested_after = self.schedule_redraw_requests(event_loop);
            self.apply_control_flow(event_loop, requested_before || requested_after);
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: winit::event::WindowEvent,
    ) {
        profiling::function_scope!(egui_winit::short_window_event_description(&event));

        // Nb: Make sure this guard is dropped after this function returns.
        event_loop_context::with_event_loop_context(event_loop, move || {
            let non_zero_resize = matches!(
                &event,
                winit::event::WindowEvent::Resized(size) if size.width != 0 && size.height != 0
            );
            let event_result = match event {
                winit::event::WindowEvent::RedrawRequested => {
                    if let Some(target) = self.winit_app.render_target(window_id) {
                        log::trace!(
                            "damage recorded: window={:?} viewport={:?} generation={} size={:?}",
                            target.window_id,
                            target.viewport_id,
                            target.surface_generation,
                            target.size
                        );
                        self.render_scheduler
                            .record_damage(target.into(), DamageReason::RedrawRequested);
                    } else {
                        self.render_scheduler.close_window(window_id);
                    }
                    Ok(EventResult::Wait)
                }
                _ => {
                    let result = self.winit_app.window_event(event_loop, window_id, event);
                    if non_zero_resize {
                        result.map(|result| {
                            result.with_repaint_now_reason(RepaintNowReason::InteractiveResize)
                        })
                    } else {
                        result
                    }
                }
            };

            self.handle_event_result(event_loop, event_result, RenderPhase::MessageDispatch, None);
        });
    }
}

#[cfg(not(target_os = "ios"))]
fn run_and_return(event_loop: &mut EventLoop<UserEvent>, winit_app: impl WinitApp) -> Result {
    use winit::platform::run_on_demand::EventLoopExtRunOnDemand as _;

    log::trace!("Entering the winit event loop (run_app_on_demand)…");

    let mut app = WinitAppWrapper::new(winit_app, true);
    event_loop.run_app_on_demand(&mut app)?;
    log::debug!("eframe window closed");
    app.return_result
}

fn run_and_exit(event_loop: EventLoop<UserEvent>, winit_app: impl WinitApp) -> Result {
    log::trace!("Entering the winit event loop (run_app)…");

    // When to repaint what window
    let mut app = WinitAppWrapper::new(winit_app, false);
    event_loop.run_app(&mut app)?;

    log::debug!("winit event loop unexpectedly returned");
    Ok(())
}

// ----------------------------------------------------------------------------

#[cfg(feature = "glow")]
pub fn run_glow(
    app_name: &str,
    mut native_options: epi::NativeOptions,
    app_creator: epi::AppCreator<'_>,
) -> Result {
    #![allow(clippy::needless_return_with_question_mark)] // False positive

    use super::glow_integration::GlowWinitApp;

    #[cfg(not(target_os = "ios"))]
    if native_options.run_and_return {
        return with_event_loop(native_options, |event_loop, native_options| {
            let glow_eframe = GlowWinitApp::new(event_loop, app_name, native_options, app_creator);
            run_and_return(event_loop, glow_eframe)
        })?;
    }

    let event_loop = create_event_loop(&mut native_options)?;
    let glow_eframe = GlowWinitApp::new(&event_loop, app_name, native_options, app_creator);
    run_and_exit(event_loop, glow_eframe)
}

#[cfg(feature = "glow")]
pub fn create_glow<'a>(
    app_name: &str,
    native_options: epi::NativeOptions,
    app_creator: epi::AppCreator<'a>,
    event_loop: &EventLoop<UserEvent>,
) -> impl ApplicationHandler<UserEvent> + 'a {
    use super::glow_integration::GlowWinitApp;

    let glow_eframe = GlowWinitApp::new(event_loop, app_name, native_options, app_creator);
    WinitAppWrapper::new(glow_eframe, true)
}

// ----------------------------------------------------------------------------

#[cfg(feature = "wgpu")]
pub fn run_wgpu(
    app_name: &str,
    mut native_options: epi::NativeOptions,
    app_creator: epi::AppCreator<'_>,
) -> Result {
    #![allow(clippy::needless_return_with_question_mark)] // False positive

    use super::wgpu_integration::WgpuWinitApp;

    #[cfg(not(target_os = "ios"))]
    if native_options.run_and_return {
        return with_event_loop(native_options, |event_loop, native_options| {
            let wgpu_eframe = WgpuWinitApp::new(event_loop, app_name, native_options, app_creator);
            run_and_return(event_loop, wgpu_eframe)
        })?;
    }

    let event_loop = create_event_loop(&mut native_options)?;
    let wgpu_eframe = WgpuWinitApp::new(&event_loop, app_name, native_options, app_creator);
    run_and_exit(event_loop, wgpu_eframe)
}

#[cfg(feature = "wgpu")]
pub fn create_wgpu<'a>(
    app_name: &str,
    native_options: epi::NativeOptions,
    app_creator: epi::AppCreator<'a>,
    event_loop: &EventLoop<UserEvent>,
) -> impl ApplicationHandler<UserEvent> + 'a {
    use super::wgpu_integration::WgpuWinitApp;

    let wgpu_eframe = WgpuWinitApp::new(event_loop, app_name, native_options, app_creator);
    WinitAppWrapper::new(wgpu_eframe, true)
}

// ----------------------------------------------------------------------------

/// A proxy to the eframe application that implements [`ApplicationHandler`].
///
/// This can be run directly on your own [`EventLoop`] by itself or with other
/// windows you manage outside of eframe.
pub struct EframeWinitApplication<'a> {
    wrapper: Box<dyn ApplicationHandler<UserEvent> + 'a>,
    control_flow: ControlFlow,
}

impl ApplicationHandler<UserEvent> for EframeWinitApplication<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.wrapper.resumed(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        self.wrapper.window_event(event_loop, window_id, event);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        self.wrapper.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.wrapper.user_event(event_loop, event);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        self.wrapper.device_event(event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.wrapper.about_to_wait(event_loop);
        self.control_flow = event_loop.control_flow();
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.wrapper.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.wrapper.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.wrapper.memory_warning(event_loop);
    }
}

impl<'a> EframeWinitApplication<'a> {
    pub(crate) fn new<T: ApplicationHandler<UserEvent> + 'a>(app: T) -> Self {
        Self {
            wrapper: Box::new(app),
            control_flow: ControlFlow::default(),
        }
    }

    /// Pump the `EventLoop` to check for and dispatch pending events to this application.
    ///
    /// Returns either the exit code for the application or the final state of the [`ControlFlow`]
    /// after all events have been dispatched in this iteration.
    ///
    /// This is useful when your [`EventLoop`] is not the main event loop for your application.
    /// See the `external_eventloop_async` example.
    #[cfg(not(target_os = "ios"))]
    pub fn pump_eframe_app(
        &mut self,
        event_loop: &mut EventLoop<UserEvent>,
        timeout: Option<std::time::Duration>,
    ) -> EframePumpStatus {
        use winit::platform::pump_events::{EventLoopExtPumpEvents as _, PumpStatus};

        match event_loop.pump_app_events(timeout, self) {
            PumpStatus::Continue => EframePumpStatus::Continue(self.control_flow),
            PumpStatus::Exit(code) => EframePumpStatus::Exit(code),
        }
    }
}

/// Either an exit code or a [`ControlFlow`] from the [`ActiveEventLoop`].
///
/// The result of [`EframeWinitApplication::pump_eframe_app`].
#[cfg(not(target_os = "ios"))]
pub enum EframePumpStatus {
    /// The final state of the [`ControlFlow`] after all events have been dispatched
    ///
    /// Callers should perform the action that is appropriate for the [`ControlFlow`] value.
    Continue(ControlFlow),

    /// The exit code for the application
    Exit(i32),
}
