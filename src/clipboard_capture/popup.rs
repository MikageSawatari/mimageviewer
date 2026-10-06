//! An ownerless, nonactivating notification window, owned by its own thread.
//! The mailbox and displayed content each retain only the latest notification.

use super::SelectionSnapshot;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DISPLAY_LIFETIME: Duration = Duration::from_secs(8);

#[derive(Clone, Debug)]
pub(crate) enum PopupContent {
    Html(Arc<SelectionSnapshot>),
    HtmlAwaitingResponse(Arc<SelectionSnapshot>),
    SelectionUnavailable,
    Saved(PathBuf),
    SavedAwaitingResponse(PathBuf),
    Failure(String),
    Abandoned,
    RevealUnavailable,
}

// Consume the button at the same ownership boundary that creates its event.
// Native dispatch cannot emit a second request while App's response is pending.
fn begin_reveal(
    shown: &mut Option<(u64, PopupContent)>,
    current_generation: u64,
) -> Option<crate::clipboard_capture::CaptureEvent> {
    let Some((generation, PopupContent::Saved(path))) = shown.as_ref() else {
        return None;
    };
    if *generation != current_generation {
        return None;
    }
    let generation = *generation;
    let path = path.clone();
    *shown = Some((
        generation,
        PopupContent::SavedAwaitingResponse(path.clone()),
    ));
    Some(crate::clipboard_capture::CaptureEvent::RevealSaved { generation, path })
}

fn begin_selection(
    shown: &mut Option<(u64, PopupContent)>,
    current_generation: u64,
    html_epoch: u64,
) -> Option<super::CaptureEvent> {
    let Some((generation, PopupContent::Html(snapshot))) = shown.as_ref() else {
        return None;
    };
    if *generation != current_generation || !snapshot.current(current_generation, html_epoch) {
        return None;
    }
    let generation = *generation;
    let snapshot = snapshot.clone();
    *shown = Some((
        generation,
        PopupContent::HtmlAwaitingResponse(snapshot.clone()),
    ));
    Some(super::CaptureEvent::OpenCaptureSelection(snapshot))
}

fn resolve_selection(
    shown: &mut Option<(u64, PopupContent)>,
    generation: u64,
    token: u64,
    response: RevealResponse,
) -> bool {
    if !matches!(shown.as_ref(), Some((g, PopupContent::HtmlAwaitingResponse(snapshot))) if *g == generation && snapshot.token == token)
    {
        return false;
    }
    *shown = match response {
        RevealResponse::Accepted => None,
        RevealResponse::Unavailable => Some((generation, PopupContent::SelectionUnavailable)),
    };
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RevealResponse {
    Accepted,
    Unavailable,
}

// An App reply belongs to the saved notification that was clicked. A newer
// copy, even within the same settings generation, must retain its own content.
fn resolve_reveal(
    shown: &mut Option<(u64, PopupContent)>,
    generation: u64,
    path: &Path,
    response: RevealResponse,
) -> bool {
    if !matches!(shown.as_ref(), Some((g, PopupContent::SavedAwaitingResponse(p))) if *g == generation && p == path)
    {
        return false;
    }
    *shown = match response {
        RevealResponse::Accepted => None,
        RevealResponse::Unavailable => Some((generation, PopupContent::RevealUnavailable)),
    };
    true
}

impl PopupContent {
    fn current(&self, generation: u64, html_epoch: u64) -> bool {
        match self {
            Self::Html(snapshot) | Self::HtmlAwaitingResponse(snapshot) => {
                snapshot.current(generation, html_epoch)
            }
            _ => true,
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Html(snapshot) | Self::HtmlAwaitingResponse(snapshot) => {
                format!("このページの画像 {} 件", snapshot.html.candidates.len())
            }
            Self::SelectionUnavailable => "一覧画面で Ctrl+V を押すと開けます".into(),
            Self::Saved(_) | Self::SavedAwaitingResponse(_) => {
                "クリップボードの画像を保存しました".into()
            }
            Self::Failure(reason) => format!("保存できませんでした: {reason}"),
            Self::Abandoned => "取り込めませんでした。もう一度コピーしてください".into(),
            Self::RevealUnavailable => {
                "一覧画面に戻ると、場所▼の『クリップボード取り込み』から開けます".into()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifetime {
    Hidden,
    Counting(Instant),
    Hovered,
}

impl Lifetime {
    fn show(&mut self, now: Instant, hovered: bool) {
        *self = if hovered {
            Self::Hovered
        } else {
            Self::Counting(now + DISPLAY_LIFETIME)
        };
    }

    fn enter(&mut self) -> bool {
        if matches!(self, Self::Counting(_)) {
            *self = Self::Hovered;
            true
        } else {
            false
        }
    }

    fn leave(&mut self, now: Instant, pointer_inside: bool) -> bool {
        if *self == Self::Hovered && !pointer_inside {
            *self = Self::Counting(now + DISPLAY_LIFETIME);
            true
        } else {
            false
        }
    }

    fn expired(self, now: Instant) -> bool {
        matches!(self, Self::Counting(deadline) if now >= deadline)
    }
}

fn scaled(logical: i32, dpi: u32) -> i32 {
    ((i64::from(logical) * i64::from(dpi.max(96)) + 48) / 96) as i32
}

/// Clamp even undersized or negative-coordinate work areas without moving onto
/// the taskbar. The caller uses physical coordinates throughout.
fn bottom_right(work: [i32; 4], dpi: u32) -> [i32; 4] {
    let [left, top, right, bottom] = work;
    let width = scaled(420, dpi).min((right - left).max(1));
    let height = scaled(112, dpi).min((bottom - top).max(1));
    let margin = scaled(16, dpi);
    [
        (right - width - margin).max(left),
        (bottom - height - margin).max(top),
        width,
        height,
    ]
}

#[cfg(windows)]
mod native {
    use super::{
        Lifetime, PopupContent, RevealResponse, begin_reveal, begin_selection, bottom_right,
        resolve_reveal, resolve_selection, scaled,
    };
    use crate::clipboard_capture::CaptureEvent;
    #[cfg(feature = "test-script")]
    use std::sync::atomic::AtomicIsize;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, mpsc};
    use std::thread::JoinHandle;
    use std::time::Instant;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    const WAKE: u32 = WM_APP + 91;
    const HIDE_TIMER: usize = 1;
    const LIFETIME_MS: u32 = 8_000;

    enum Command {
        Show {
            generation: u64,
            dark: bool,
            content: PopupContent,
        },
        Hide,
        SelectionResult {
            generation: u64,
            token: u64,
            response: RevealResponse,
        },
        RevealResult {
            generation: u64,
            path: std::path::PathBuf,
            response: RevealResponse,
        },
    }

    struct Mailbox {
        #[cfg(feature = "test-script")]
        smoke_hwnd: AtomicIsize,
        latest: Mutex<Option<Command>>,
        current_generation: Arc<AtomicU64>,
        selection_open: Arc<AtomicBool>,
        html_epoch: Arc<AtomicU64>,
        thread_id: AtomicU32,
        stop: AtomicBool,
    }

    impl Mailbox {
        fn publish_show(&self, generation: u64, dark: bool, content: PopupContent) -> bool {
            let mut latest = self.latest.lock().unwrap_or_else(|p| p.into_inner());
            // Validate inside the same ownership boundary as replacement: an old
            // worker must not erase a pending Hide or a newer generation's Show.
            if generation != self.current_generation.load(Ordering::Acquire)
                || self.selection_open.load(Ordering::Acquire)
                || !content.current(generation, self.html_epoch.load(Ordering::Acquire))
            {
                return false;
            }
            *latest = Some(Command::Show {
                generation,
                dark,
                content,
            });
            #[cfg(feature = "test-script")]
            crate::clipboard_capture::diagnostics::popup_requested();
            true
        }

        fn wake(&self) {
            let id = self.thread_id.load(Ordering::Acquire);
            if id != 0 {
                // The thread publishes its id only after creating a message queue.
                let _ = unsafe { PostThreadMessageW(id, WAKE, WPARAM(0), LPARAM(0)) };
            }
        }

        fn publish_selection_result(
            &self,
            generation: u64,
            token: u64,
            response: RevealResponse,
        ) -> bool {
            let mut latest = self.latest.lock().unwrap_or_else(|p| p.into_inner());
            if generation != self.current_generation.load(Ordering::Acquire)
                || matches!(latest.as_ref(), Some(Command::Show { .. } | Command::Hide))
            {
                return false;
            }
            *latest = Some(Command::SelectionResult {
                generation,
                token,
                response,
            });
            true
        }

        fn publish_reveal_result(
            &self,
            generation: u64,
            path: &std::path::Path,
            response: RevealResponse,
        ) -> bool {
            let mut latest = self.latest.lock().unwrap_or_else(|p| p.into_inner());
            if generation != self.current_generation.load(Ordering::Acquire)
                || matches!(latest.as_ref(), Some(Command::Show { .. } | Command::Hide))
            {
                return false;
            }
            *latest = Some(Command::RevealResult {
                generation,
                path: path.to_path_buf(),
                response,
            });
            true
        }
    }

    pub(crate) struct PopupRuntime {
        mailbox: Arc<Mailbox>,
        thread: Option<JoinHandle<()>>,
    }

    impl PopupRuntime {
        #[cfg(feature = "test-script")]
        pub(crate) fn smoke_visible(&self) -> bool {
            let hwnd = self.mailbox.smoke_hwnd.load(Ordering::Acquire);
            hwnd != 0 && unsafe { IsWindowVisible(HWND(hwnd as *mut _)).as_bool() }
        }

        pub(crate) fn start(
            current_generation: Arc<AtomicU64>,
            selection_open: Arc<AtomicBool>,
            html_epoch: Arc<AtomicU64>,
            event_tx: mpsc::Sender<CaptureEvent>,
            repaint: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<Self, String> {
            let mailbox = Arc::new(Mailbox {
                #[cfg(feature = "test-script")]
                smoke_hwnd: AtomicIsize::new(0),
                latest: Mutex::new(None),
                current_generation: current_generation.clone(),
                selection_open: selection_open.clone(),
                html_epoch: html_epoch.clone(),
                thread_id: AtomicU32::new(0),
                stop: AtomicBool::new(false),
            });
            let worker_mailbox = mailbox.clone();
            let thread = std::thread::Builder::new()
                .name("clipboard-capture-popup".into())
                .spawn(move || {
                    let mut state = Box::new(WindowState {
                        current_generation,
                        selection_open,
                        html_epoch,
                        event_tx,
                        repaint,
                        shown: None,
                        dark: false,
                        dpi: 96,
                        font: HFONT::default(),
                        lifetime: Lifetime::Hidden,
                    });
                    if let Err(error) = run(&worker_mailbox, &mut state) {
                        crate::logger::log(format!("clipboard_capture popup: {error}"));
                        let _ = state.event_tx.send(CaptureEvent::StartupFailed(error));
                        (state.repaint)();
                    }
                    worker_mailbox.thread_id.store(0, Ordering::Release);
                    #[cfg(feature = "test-script")]
                    worker_mailbox.smoke_hwnd.store(0, Ordering::Release);
                })
                .map_err(|error| format!("clipboard capture popup thread: {error}"))?;
            Ok(Self {
                mailbox,
                thread: Some(thread),
            })
        }

        pub(crate) fn show(&self, generation: u64, dark: bool, content: PopupContent) {
            if self.mailbox.publish_show(generation, dark, content) {
                self.mailbox.wake();
            }
        }

        pub(crate) fn hide(&self) {
            *self
                .mailbox
                .latest
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some(Command::Hide);
            self.mailbox.wake();
        }

        pub(crate) fn resolve_selection(
            &self,
            generation: u64,
            token: u64,
            response: RevealResponse,
        ) {
            if self
                .mailbox
                .publish_selection_result(generation, token, response)
            {
                self.mailbox.wake();
            }
        }

        pub(crate) fn resolve_reveal(
            &self,
            generation: u64,
            path: &std::path::Path,
            response: RevealResponse,
        ) {
            if self
                .mailbox
                .publish_reveal_result(generation, path, response)
            {
                self.mailbox.wake();
            }
        }
    }

    impl Drop for PopupRuntime {
        fn drop(&mut self) {
            self.mailbox.stop.store(true, Ordering::Release);
            self.mailbox.wake();
            // UI teardown never waits for a thread that is inside native dispatch.
            if let Some(thread) = self.thread.take()
                && thread.is_finished()
            {
                let _ = thread.join();
            }
        }
    }

    struct WindowState {
        current_generation: Arc<AtomicU64>,
        selection_open: Arc<AtomicBool>,
        html_epoch: Arc<AtomicU64>,
        event_tx: mpsc::Sender<CaptureEvent>,
        repaint: Arc<dyn Fn() + Send + Sync>,
        shown: Option<(u64, PopupContent)>,
        dark: bool,
        dpi: u32,
        font: HFONT,
        lifetime: Lifetime,
    }

    impl Drop for WindowState {
        fn drop(&mut self) {
            if !self.font.0.is_null() {
                let _ = unsafe { DeleteObject(self.font.into()) };
            }
        }
    }

    fn register_class() -> Result<(), String> {
        static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
        REGISTERED
            .get_or_init(|| unsafe {
                let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: module.into(),
                    hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
                    lpszClassName: w!("mImageViewer.ClipboardCapturePopup"),
                    ..Default::default()
                };
                if RegisterClassW(&class) == 0 {
                    return Err(format!(
                        "RegisterClassW popup: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                Ok(())
            })
            .clone()
    }

    fn run(mailbox: &Mailbox, state: &mut Box<WindowState>) -> Result<(), String> {
        unsafe {
            let previous_dpi =
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            if previous_dpi.0.is_null() {
                return Err(format!(
                    "popup DPI awareness: {}",
                    std::io::Error::last_os_error()
                ));
            }
            register_class()?;
            // Creates the thread message queue before producers are allowed to post.
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            mailbox
                .thread_id
                .store(GetCurrentThreadId(), Ordering::Release);
            let state_ptr: *mut WindowState = &mut **state;
            let mut window = None;
            loop {
                if mailbox.stop.load(Ordering::Acquire) {
                    break;
                }
                let command = mailbox
                    .latest
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
                match command {
                    Some(Command::Show {
                        generation,
                        dark,
                        content,
                    }) if generation == (*state_ptr).current_generation.load(Ordering::Acquire)
                        && !mailbox.selection_open.load(Ordering::Acquire)
                        && content
                            .current(generation, mailbox.html_epoch.load(Ordering::Acquire)) =>
                    {
                        if window.is_none() {
                            let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
                            window = Some(
                                CreateWindowExW(
                                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                                    w!("mImageViewer.ClipboardCapturePopup"),
                                    w!("クリップボード取り込み"),
                                    WS_POPUP,
                                    0,
                                    0,
                                    1,
                                    1,
                                    None,
                                    None,
                                    Some(module.into()),
                                    Some(state_ptr.cast()),
                                )
                                .map_err(|e| format!("CreateWindowExW popup: {e}"))?,
                            );
                        }
                        #[cfg(feature = "test-script")]
                        mailbox
                            .smoke_hwnd
                            .store(window.unwrap().0 as isize, Ordering::Release);
                        show(window.unwrap(), state_ptr, generation, dark, content);
                    }
                    Some(Command::Hide) => {
                        if let Some(hwnd) = window {
                            hide(hwnd, state_ptr);
                        }
                    }
                    Some(Command::SelectionResult {
                        generation,
                        token,
                        response,
                    }) if generation == (*state_ptr).current_generation.load(Ordering::Acquire) => {
                        if resolve_selection(&mut (*state_ptr).shown, generation, token, response)
                            && let Some(hwnd) = window
                        {
                            match response {
                                RevealResponse::Accepted => hide(hwnd, state_ptr),
                                RevealResponse::Unavailable => {
                                    let dark = (*state_ptr).dark;
                                    show(
                                        hwnd,
                                        state_ptr,
                                        generation,
                                        dark,
                                        PopupContent::SelectionUnavailable,
                                    );
                                }
                            }
                        }
                    }
                    Some(Command::RevealResult {
                        generation,
                        path,
                        response,
                    }) if generation == (*state_ptr).current_generation.load(Ordering::Acquire) => {
                        if resolve_reveal(&mut (*state_ptr).shown, generation, &path, response)
                            && let Some(hwnd) = window
                        {
                            match response {
                                RevealResponse::Accepted => hide(hwnd, state_ptr),
                                RevealResponse::Unavailable => {
                                    let dark = (*state_ptr).dark;
                                    show(
                                        hwnd,
                                        state_ptr,
                                        generation,
                                        dark,
                                        PopupContent::RevealUnavailable,
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
                // The generation can change after a producer's checked enqueue
                // and before its Hide arrives. Draining always retires any old
                // displayed notification, including when the drained Show was
                // discarded as stale.
                let shown_stale =
                    (*state_ptr)
                        .shown
                        .as_ref()
                        .is_some_and(|(generation, content)| {
                            *generation != mailbox.current_generation.load(Ordering::Acquire)
                                || !content.current(
                                    *generation,
                                    mailbox.html_epoch.load(Ordering::Acquire),
                                )
                        });
                if (shown_stale || mailbox.selection_open.load(Ordering::Acquire))
                    && let Some(hwnd) = window
                {
                    hide(hwnd, state_ptr);
                }
                // A producer may have replaced the mailbox while show() dispatched
                // synchronous window messages. Its wake remains in this queue.
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 == -1 || !result.as_bool() {
                    break;
                }
                if message.message != WAKE {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if let Some(hwnd) = window {
                let _ = DestroyWindow(hwnd);
            }
            let _ = SetThreadDpiAwarenessContext(previous_dpi);
        }
        Ok(())
    }

    /// Native calls may synchronously reenter window_proc. No Rust reference to
    /// WindowState survives any window-position/show/dispatch operation.
    unsafe fn show(
        hwnd: HWND,
        state: *mut WindowState,
        generation: u64,
        dark: bool,
        content: PopupContent,
    ) {
        unsafe {
            (*state).shown = Some((generation, content));
            (*state).dark = dark;
            (*state).lifetime = Lifetime::Hidden;
            let _ = KillTimer(Some(hwnd), HIDE_TIMER);
            if let Some(work) = crate::monitor::foreground_work_area() {
                let area = [work.left, work.top, work.right, work.bottom];
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    work.left,
                    work.top,
                    1,
                    1,
                    SWP_NOACTIVATE,
                );
                rebuild_font(state, GetDpiForWindow(hwnd));
                let [x, y, width, height] = bottom_right(area, (*state).dpi);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
            } else {
                // GetMonitorInfo failure is a native initialization error; report
                // rather than placing a notification on an arbitrary screen.
                hide(hwnd, state);
                crate::logger::log("clipboard_capture popup: foreground work area unavailable");
                return;
            }
            if generation != (*state).current_generation.load(Ordering::Acquire) {
                hide(hwnd, state);
                return;
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            #[cfg(feature = "test-script")]
            crate::clipboard_capture::diagnostics::popup_shown();
            let _ = InvalidateRect(Some(hwnd), None, false);
            let mut cursor = POINT::default();
            let mut rect = RECT::default();
            let hovered = GetCursorPos(&mut cursor).is_ok()
                && GetWindowRect(hwnd, &mut rect).is_ok()
                && contains(rect, cursor.x, cursor.y);
            (*state).lifetime.show(Instant::now(), hovered);
            if hovered {
                if !track_leave(hwnd) {
                    // Without a leave notification the pause could never end.
                    (*state).lifetime.show(Instant::now(), false);
                    SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME_MS, None);
                }
            } else {
                SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME_MS, None);
            }
        }
    }

    unsafe fn hide(hwnd: HWND, state: *mut WindowState) {
        unsafe {
            (*state).lifetime = Lifetime::Hidden;
            (*state).shown = None;
            let _ = KillTimer(Some(hwnd), HIDE_TIMER);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    unsafe fn rebuild_font(state: *mut WindowState, dpi: u32) {
        unsafe {
            (*state).dpi = dpi.max(96);
            let new_font = CreateFontW(
                -scaled(14, (*state).dpi),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Yu Gothic UI"),
            );
            if !(*state).font.0.is_null() {
                let _ = DeleteObject((*state).font.into());
            }
            // A null font is rendered with the stock UI font, never deleted.
            (*state).font = new_font;
        }
    }

    fn contains(rect: RECT, x: i32, y: i32) -> bool {
        x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
    }

    fn buttons(width: i32, height: i32, dpi: u32, html: bool) -> (RECT, RECT) {
        let margin = scaled(12, dpi);
        let close = RECT {
            left: (width - scaled(36, dpi)).max(0),
            top: 0,
            right: width,
            bottom: scaled(32, dpi).min(height),
        };
        let open = RECT {
            left: margin,
            top: (height - scaled(42, dpi)).max(0),
            right: (margin + scaled(if html { 180 } else { 76 }, dpi)).min(width),
            bottom: (height - margin).max(0),
        };
        (open, close)
    }

    unsafe fn track_leave(hwnd: HWND) -> bool {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let registered = unsafe { TrackMouseEvent(&mut tracking) }.is_ok();
        if !registered {
            crate::logger::log("clipboard_capture popup: TrackMouseEvent failed");
        }
        registered
    }

    unsafe fn paint(hwnd: HWND, state: *const WindowState) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let dark = (*state).dark;
            let background = if dark {
                COLORREF(0x002b2b2b)
            } else {
                COLORREF(0x00fafafa)
            };
            let foreground = if dark {
                COLORREF(0x00f5f5f5)
            } else {
                COLORREF(0x00202020)
            };
            let button_color = if dark {
                COLORREF(0x00464646)
            } else {
                COLORREF(0x00e4e4e4)
            };
            let brush = CreateSolidBrush(background);
            FillRect(dc, &client, brush);
            let _ = DeleteObject(brush.into());
            let font = if (*state).font.0.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                (*state).font.into()
            };
            let old_font = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, foreground);
            let (mut open, mut close) = buttons(
                client.right,
                client.bottom,
                (*state).dpi,
                matches!(
                    (*state).shown.as_ref(),
                    Some((
                        _,
                        PopupContent::Html(_) | PopupContent::HtmlAwaitingResponse(_)
                    ))
                ),
            );
            if let Some((_, content)) = &(*state).shown {
                let mut text_rect = RECT {
                    left: scaled(16, (*state).dpi),
                    top: scaled(16, (*state).dpi),
                    right: close.left.max(0),
                    bottom: open.top.max(scaled(16, (*state).dpi)),
                };
                let mut text: Vec<u16> = content.text().encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut text,
                    &mut text_rect,
                    DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                if matches!(
                    content,
                    PopupContent::Saved(_)
                        | PopupContent::SavedAwaitingResponse(_)
                        | PopupContent::Html(_)
                        | PopupContent::HtmlAwaitingResponse(_)
                ) {
                    let brush = CreateSolidBrush(button_color);
                    FillRect(dc, &open, brush);
                    let _ = DeleteObject(brush.into());
                    if matches!(
                        content,
                        PopupContent::SavedAwaitingResponse(_)
                            | PopupContent::HtmlAwaitingResponse(_)
                    ) {
                        SetTextColor(
                            dc,
                            if dark {
                                COLORREF(0x00a0a0a0)
                            } else {
                                COLORREF(0x00808080)
                            },
                        );
                    }
                    let label = if matches!(
                        content,
                        PopupContent::Html(_) | PopupContent::HtmlAwaitingResponse(_)
                    ) {
                        "画像を選んで保存"
                    } else {
                        "開く"
                    };
                    let mut label: Vec<u16> = label.encode_utf16().collect();
                    DrawTextW(
                        dc,
                        &mut label,
                        &mut open,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    SetTextColor(dc, foreground);
                }
            }
            let mut label: Vec<u16> = "×".encode_utf16().collect();
            DrawTextW(
                dc,
                &mut label,
                &mut close,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(dc, old_font);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            if message == WM_NCCREATE {
                let create = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if state.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            match message {
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_ERASEBKGND => LRESULT(1),
                WM_PAINT => {
                    paint(hwnd, state);
                    LRESULT(0)
                }
                WM_TIMER if wparam.0 == HIDE_TIMER => {
                    // KillTimer cannot remove WM_TIMER already in the queue. The
                    // typed deadline keeps such a message from hiding new content
                    // or a popup whose hover pause has just ended.
                    if (*state).lifetime.expired(Instant::now()) {
                        hide(hwnd, state);
                    }
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    if (*state).lifetime.enter() {
                        let _ = KillTimer(Some(hwnd), HIDE_TIMER);
                        if !track_leave(hwnd) {
                            (*state).lifetime.show(Instant::now(), false);
                            SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME_MS, None);
                        }
                    }
                    LRESULT(0)
                }
                WM_MOUSELEAVE => {
                    let mut cursor = POINT::default();
                    let mut rect = RECT::default();
                    let pointer_inside = GetCursorPos(&mut cursor).is_ok()
                        && GetWindowRect(hwnd, &mut rect).is_ok()
                        && contains(rect, cursor.x, cursor.y);
                    if (*state).lifetime.leave(Instant::now(), pointer_inside) {
                        SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME_MS, None);
                    } else if (*state).lifetime == Lifetime::Hovered && !track_leave(hwnd) {
                        (*state).lifetime.show(Instant::now(), false);
                        SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME_MS, None);
                    }
                    LRESULT(0)
                }
                WM_DPICHANGED => {
                    let dpi = (wparam.0 & 0xffff) as u32;
                    rebuild_font(state, dpi);
                    if lparam.0 != 0 {
                        let rect = *(lparam.0 as *const RECT);
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_TOPMOST),
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOACTIVATE,
                        );
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let x = (lparam.0 & 0xffff) as u16 as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let (open, close) = buttons(
                        client.right,
                        client.bottom,
                        (*state).dpi,
                        matches!(
                            (*state).shown.as_ref(),
                            Some((
                                _,
                                PopupContent::Html(_) | PopupContent::HtmlAwaitingResponse(_)
                            ))
                        ),
                    );
                    if contains(close, x, y) {
                        hide(hwnd, state);
                    } else if contains(open, x, y) {
                        let generation = (*state).current_generation.load(Ordering::Acquire);
                        if !(*state).selection_open.load(Ordering::Acquire)
                            && let Some(event) = begin_selection(
                                &mut (*state).shown,
                                generation,
                                (*state).html_epoch.load(Ordering::Acquire),
                            )
                            .or_else(|| begin_reveal(&mut (*state).shown, generation))
                        {
                            let events = (*state).event_tx.clone();
                            let repaint = (*state).repaint.clone();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                            let _ = events.send(event);
                            repaint();
                        }
                    }
                    LRESULT(0)
                }
                WM_CLOSE => {
                    hide(hwnd, state);
                    LRESULT(0)
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn mailbox(generation: u64) -> Mailbox {
            Mailbox {
                #[cfg(feature = "test-script")]
                smoke_hwnd: AtomicIsize::new(0),
                latest: Mutex::new(None),
                current_generation: Arc::new(AtomicU64::new(generation)),
                selection_open: Arc::new(AtomicBool::new(false)),
                html_epoch: Arc::new(AtomicU64::new(0)),
                thread_id: AtomicU32::new(0),
                stop: AtomicBool::new(false),
            }
        }

        #[test]
        fn selection_open_discards_popup_publication_without_replacing_pending_hide() {
            let mailbox = mailbox(1);
            *mailbox.latest.lock().unwrap() = Some(Command::Hide);
            mailbox.selection_open.store(true, Ordering::Release);
            assert!(!mailbox.publish_show(
                1,
                false,
                PopupContent::Failure("saved while dialog open".into())
            ));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Hide)
            ));
        }

        #[test]
        fn selection_reply_retains_latest_notification_mailbox() {
            let mailbox = mailbox(1);
            assert!(mailbox.publish_selection_result(1, 10, RevealResponse::Unavailable));
            assert!(mailbox.publish_selection_result(1, 11, RevealResponse::Accepted));
            assert!(matches!(
                mailbox.latest.lock().unwrap().take(),
                Some(Command::SelectionResult {
                    token: 11,
                    response: RevealResponse::Accepted,
                    ..
                })
            ));
            assert!(mailbox.publish_show(1, false, PopupContent::Abandoned));
            assert!(!mailbox.publish_selection_result(1, 10, RevealResponse::Accepted));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Show { .. })
            ));
        }

        #[test]
        fn stale_show_cannot_replace_pending_hide() {
            let mailbox = mailbox(1);
            assert!(mailbox.publish_show(1, false, PopupContent::Abandoned));
            mailbox.current_generation.store(2, Ordering::Release);
            *mailbox.latest.lock().unwrap() = Some(Command::Hide);
            assert!(!mailbox.publish_show(1, true, PopupContent::Failure("old".into())));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Hide)
            ));
        }

        #[test]
        fn stale_show_cannot_replace_current_generation_show() {
            let mailbox = mailbox(2);
            assert!(mailbox.publish_show(2, false, PopupContent::Abandoned));
            assert!(!mailbox.publish_show(1, true, PopupContent::Failure("old".into())));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Show {
                    generation: 2,
                    dark: false,
                    content: PopupContent::Abandoned,
                })
            ));
        }

        #[test]
        fn latest_reveal_reply_replaces_an_earlier_reply_and_updates_only_clicked_copy() {
            let mailbox = mailbox(2);
            let a = std::path::PathBuf::from("capture/a.png");
            let b = std::path::PathBuf::from("capture/b.png");
            assert!(mailbox.publish_reveal_result(2, &a, RevealResponse::Accepted));
            assert!(mailbox.publish_reveal_result(2, &b, RevealResponse::Unavailable));
            let Some(Command::RevealResult {
                generation,
                path,
                response,
            }) = mailbox.latest.lock().unwrap().take()
            else {
                panic!("latest App reply missing");
            };
            assert_eq!(path, b);
            let mut shown = Some((2, PopupContent::Saved(b)));
            assert!(begin_reveal(&mut shown, 2).is_some());
            assert!(resolve_reveal(&mut shown, generation, &path, response));
            assert!(matches!(shown, Some((2, PopupContent::RevealUnavailable))));
        }

        #[test]
        fn reveal_reply_never_displaces_pending_show_hide_or_current_generation() {
            let mailbox = mailbox(2);
            let path = std::path::Path::new("capture/a.png");
            assert!(mailbox.publish_show(2, false, PopupContent::Abandoned));
            assert!(!mailbox.publish_reveal_result(2, path, RevealResponse::Accepted));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Show { .. })
            ));
            *mailbox.latest.lock().unwrap() = Some(Command::Hide);
            assert!(!mailbox.publish_reveal_result(2, path, RevealResponse::Unavailable));
            assert!(matches!(
                *mailbox.latest.lock().unwrap(),
                Some(Command::Hide)
            ));
            *mailbox.latest.lock().unwrap() = None;
            assert!(!mailbox.publish_reveal_result(1, path, RevealResponse::Unavailable));
            assert!(mailbox.latest.lock().unwrap().is_none());
        }
    }
}

#[cfg(windows)]
pub(crate) use native::PopupRuntime;

#[cfg(not(windows))]
pub(crate) struct PopupRuntime;

#[cfg(not(windows))]
impl PopupRuntime {
    pub(crate) fn start(
        _: std::sync::Arc<std::sync::atomic::AtomicU64>,
        _: std::sync::Arc<std::sync::atomic::AtomicBool>,
        _: std::sync::Arc<std::sync::atomic::AtomicU64>,
        _: std::sync::mpsc::Sender<crate::clipboard_capture::CaptureEvent>,
        _: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        Err("clipboard capture popup requires Windows".into())
    }
    pub(crate) fn show(&self, _: u64, _: bool, _: PopupContent) {}
    pub(crate) fn hide(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html_snapshot() -> Arc<SelectionSnapshot> {
        super::super::SelectionSnapshot::new(
            super::super::html::HtmlCapture {
                page_url: "https://example.com/page".into(),
                candidates: vec!["https://example.com/image.png".into()],
                omitted: 0,
            },
            super::super::CaptureIntent::Manual {
                destination: PathBuf::from("capture"),
            },
            0,
        )
    }

    #[test]
    fn html_click_emits_snapshot_once_and_acceptance_is_correlated_without_clipboard_read() {
        let snapshot = html_snapshot();
        let mut shown = Some((4, PopupContent::Html(snapshot.clone())));
        assert!(begin_selection(&mut shown, 3, 0).is_none());
        let event = begin_selection(&mut shown, 4, 0).unwrap();
        assert!(
            matches!(event, super::super::CaptureEvent::OpenCaptureSelection(ref captured) if Arc::ptr_eq(captured, &snapshot))
        );
        assert!(begin_selection(&mut shown, 4, 0).is_none());
        assert!(!resolve_selection(
            &mut shown,
            4,
            snapshot.token + 1,
            RevealResponse::Accepted
        ));
        assert!(resolve_selection(
            &mut shown,
            4,
            snapshot.token,
            RevealResponse::Accepted
        ));
        assert!(shown.is_none());
    }

    #[test]
    fn automatic_html_epoch_retires_content_and_clicks_permanently_after_dialog_close() {
        let config = super::super::CaptureConfig {
            images: false,
            html: true,
            destination: Some(PathBuf::from("capture")),
        };
        let settings = Arc::new(super::super::CaptureSnapshot::updated(
            None, config, 10, false,
        ));
        let snapshot = super::super::SelectionSnapshot::new(
            html_snapshot().html.clone(),
            super::super::CaptureIntent::Automatic { snapshot: settings },
            0,
        );
        let content = PopupContent::Html(snapshot);
        assert!(content.current(1, 0));
        assert!(!content.current(1, 1));
        let mut shown = Some((1, content));
        assert!(begin_selection(&mut shown, 1, 1).is_none());
        assert!(matches!(shown, Some((1, PopupContent::Html(_)))));
    }

    #[test]
    fn unavailable_html_replaces_only_clicked_token_with_ctrl_v_instruction() {
        let old = html_snapshot();
        let latest = html_snapshot();
        let mut shown = Some((4, PopupContent::Html(old.clone())));
        assert!(begin_selection(&mut shown, 4, 0).is_some());
        shown = Some((4, PopupContent::Html(latest.clone())));
        assert!(!resolve_selection(
            &mut shown,
            4,
            old.token,
            RevealResponse::Unavailable
        ));
        assert!(begin_selection(&mut shown, 4, 0).is_some());
        assert!(!resolve_selection(
            &mut shown,
            3,
            latest.token,
            RevealResponse::Unavailable
        ));
        assert!(resolve_selection(
            &mut shown,
            4,
            latest.token,
            RevealResponse::Unavailable
        ));
        assert!(matches!(
            shown,
            Some((4, PopupContent::SelectionUnavailable))
        ));
        assert_eq!(
            shown.unwrap().1.text(),
            "一覧画面で Ctrl+V を押すと開けます"
        );
    }

    #[test]
    fn repeated_clicks_emit_one_event_and_keep_the_accepted_response() {
        let path = PathBuf::from("capture/saved.png");
        let mut shown = Some((4, PopupContent::Saved(path.clone())));
        let (events, received) = std::sync::mpsc::channel();
        for _ in 0..3 {
            if let Some(event) = begin_reveal(&mut shown, 4) {
                events.send(event).unwrap();
            }
        }
        let events = received.try_iter().collect::<Vec<_>>();
        let [
            crate::clipboard_capture::CaptureEvent::RevealSaved {
                generation,
                path: clicked,
            },
        ] = events.as_slice()
        else {
            panic!("same notification emitted more than one event");
        };
        assert_eq!(*generation, 4);
        assert_eq!(clicked, &path);
        assert!(
            matches!(shown.as_ref(), Some((4, PopupContent::SavedAwaitingResponse(p))) if p == &path)
        );
        assert!(resolve_reveal(
            &mut shown,
            4,
            &path,
            RevealResponse::Accepted
        ));
        assert!(!resolve_reveal(
            &mut shown,
            4,
            &path,
            RevealResponse::Unavailable
        ));
        assert!(shown.is_none());
    }

    #[test]
    fn new_notification_can_be_clicked_while_the_previous_reply_is_pending() {
        let old = PathBuf::from("capture/old.png");
        let new = PathBuf::from("capture/new.png");
        let mut shown = Some((5, PopupContent::Saved(old.clone())));
        assert!(begin_reveal(&mut shown, 5).is_some());
        shown = Some((5, PopupContent::Saved(new.clone())));
        assert!(!resolve_reveal(
            &mut shown,
            5,
            &old,
            RevealResponse::Accepted
        ));
        assert!(begin_reveal(&mut shown, 5).is_some());
        assert!(begin_reveal(&mut shown, 5).is_none());
        assert!(!resolve_reveal(
            &mut shown,
            5,
            &old,
            RevealResponse::Unavailable
        ));
        assert!(resolve_reveal(
            &mut shown,
            5,
            &new,
            RevealResponse::Unavailable
        ));
        assert!(matches!(shown, Some((5, PopupContent::RevealUnavailable))));
    }

    #[test]
    fn stale_generation_or_closed_notification_cannot_emit_or_accept_a_reply() {
        let path = PathBuf::from("capture/saved.png");
        let mut shown = Some((4, PopupContent::Saved(path.clone())));
        assert!(begin_reveal(&mut shown, 5).is_none());
        assert!(matches!(shown.as_ref(), Some((4, PopupContent::Saved(_)))));
        assert!(begin_reveal(&mut shown, 4).is_some());
        shown = None; // Close while App's response is pending.
        assert!(begin_reveal(&mut shown, 4).is_none());
        assert!(!resolve_reveal(
            &mut shown,
            4,
            &path,
            RevealResponse::Accepted
        ));
        assert!(!resolve_reveal(
            &mut shown,
            4,
            &path,
            RevealResponse::Unavailable
        ));
        assert!(shown.is_none());
    }

    #[test]
    fn rejected_reveal_replaces_only_its_saved_notification_without_an_open_button() {
        let path = PathBuf::from("capture/saved.png");
        let mut shown = Some((4, PopupContent::Saved(path.clone())));
        assert!(begin_reveal(&mut shown, 4).is_some());
        assert!(resolve_reveal(
            &mut shown,
            4,
            &path,
            RevealResponse::Unavailable
        ));
        assert!(matches!(shown, Some((4, PopupContent::RevealUnavailable))));
        assert_eq!(
            shown.as_ref().unwrap().1.text(),
            "一覧画面に戻ると、場所▼の『クリップボード取り込み』から開けます"
        );
    }

    #[test]
    fn reveal_reply_does_not_replace_a_newer_copy_or_generation() {
        let old = PathBuf::from("capture/old.png");
        let new = PathBuf::from("capture/new.png");
        let mut shown = Some((5, PopupContent::Saved(new.clone())));
        assert!(!resolve_reveal(
            &mut shown,
            5,
            &new,
            RevealResponse::Accepted
        ));
        assert!(begin_reveal(&mut shown, 5).is_some());
        assert!(!resolve_reveal(
            &mut shown,
            5,
            &old,
            RevealResponse::Unavailable
        ));
        assert!(!resolve_reveal(
            &mut shown,
            4,
            &new,
            RevealResponse::Accepted
        ));
        assert!(
            matches!(shown.as_ref(), Some((5, PopupContent::SavedAwaitingResponse(p))) if *p == new)
        );
        assert!(resolve_reveal(
            &mut shown,
            5,
            &new,
            RevealResponse::Accepted
        ));
        assert!(shown.is_none());
    }

    #[test]
    fn physical_position_handles_negative_monitors_and_taskbars() {
        assert_eq!(bottom_right([-1920, 0, 0, 1040], 96), [-436, 912, 420, 112]);
        assert_eq!(
            bottom_right([0, 0, 2560, 1400], 144),
            [1906, 1208, 630, 168]
        );
        assert_eq!(bottom_right([0, 0, 200, 80], 192), [0, 0, 200, 80]);
    }

    #[test]
    fn dpi_scales_geometry_and_font_in_same_units() {
        assert_eq!(scaled(14, 96), 14);
        assert_eq!(scaled(14, 144), 21);
        assert_eq!(scaled(14, 192), 28);
        assert_eq!(scaled(14, 0), 14);
    }

    #[test]
    fn hover_pauses_and_leave_restarts_full_lifetime() {
        let mut state = Lifetime::Hidden;
        let now = Instant::now();
        assert!(!state.leave(now, false));
        state.show(now, false);
        assert!(!state.expired(now + Duration::from_secs(7)));
        assert!(state.enter());
        assert_eq!(state, Lifetime::Hovered);
        assert!(!state.enter());
        assert!(!state.expired(now + Duration::from_secs(60)));
        let leave_time = now + Duration::from_secs(60);
        assert!(state.leave(leave_time, false));
        assert_eq!(state, Lifetime::Counting(leave_time + DISPLAY_LIFETIME));
        assert!(!state.expired(leave_time + Duration::from_secs(7)));
        assert!(state.expired(leave_time + DISPLAY_LIFETIME));
        assert!(!state.leave(leave_time, false));
        state.show(now, true);
        assert_eq!(state, Lifetime::Hovered);
        state = Lifetime::Hidden;
        assert!(!state.enter());
        assert!(!state.leave(now, false));
    }

    #[test]
    fn stale_leave_does_not_expire_hovered_replacement() {
        let now = Instant::now();
        let mut state = Lifetime::Hovered;
        state.show(now, true);
        assert!(!state.leave(now + Duration::from_secs(1), true));
        assert_eq!(state, Lifetime::Hovered);
        assert!(!state.expired(now + Duration::from_secs(60)));
        assert!(state.leave(now + Duration::from_secs(61), false));
        assert!(state.expired(now + Duration::from_secs(69)));
    }
}
