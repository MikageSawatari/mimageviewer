use super::{
    AcceptedContent, CaptureEvent, CaptureIntent, CaptureSnapshot, ReadDecision, ReadRequest,
    ReaderState, SelectionSnapshot, await_quiet_request, data, html, popup,
};
use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::*,
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{OleInitialize, OleUninitialize},
        },
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, w},
};

const SHUTDOWN: u32 = WM_APP + 0x32b;
const IMAGE_LIMIT: usize = 256 * 1024 * 1024;
const SMALL_LIMIT: usize = 64 * 1024;
struct Sta;
impl Sta {
    fn new() -> Result<Self, String> {
        unsafe { OleInitialize(None) }
            .map(|_| Self)
            .map_err(|e| format!("OLE STA: {e}"))
    }
}
impl Drop for Sta {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct Shared {
    snapshot: Mutex<Arc<CaptureSnapshot>>,
    generation: Arc<AtomicU64>,
    selection_open: Arc<AtomicBool>,
    html_epoch: Arc<AtomicU64>,
    latest: Mutex<Option<ReadRequest>>,
    serial: AtomicU64,
    stop: AtomicBool,
    hwnd: AtomicIsize,
    wake: mpsc::SyncSender<()>,
    events: mpsc::Sender<CaptureEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}
impl Shared {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        super::wake_save_destination_waiters();
        let _ = self.wake.try_send(());
    }
    fn fail(&self, error: String) {
        self.stop();
        crate::logger::log(format!("clipboard_capture: {error}"));
        let _ = self.events.send(CaptureEvent::StartupFailed(error));
        (self.repaint)();
    }
    fn take_latest(&self) -> Option<ReadRequest> {
        self.latest.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
    fn notify(&self, sequence: u32) {
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        // Read the cancellation generation before admission. If a dialog opens
        // across these loads, either admission is false or the old epoch retires
        // the request permanently, including after the dialog has closed.
        let html_epoch = self.html_epoch.load(Ordering::Acquire);
        let html_detection_allowed = !self.selection_open.load(Ordering::Acquire);
        let request = ReadRequest {
            serial: self.serial.fetch_add(1, Ordering::AcqRel) + 1,
            sequence,
            snapshot,
            notified_at: Instant::now(),
            reread: false,
            html_detection_allowed,
            html_epoch,
        };
        *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(request);
        let _ = self.wake.try_send(());
    }
    fn current(&self, request: &ReadRequest) -> bool {
        !self.stop.load(Ordering::Acquire)
            && self.generation.load(Ordering::Acquire) == request.snapshot.generation
    }
    fn still_latest(&self, request: &ReadRequest) -> bool {
        self.current(request) && self.serial.load(Ordering::Acquire) == request.serial
    }
    fn update_snapshot(&self, snapshot: Arc<CaptureSnapshot>) {
        *self.snapshot.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
        super::wake_save_destination_waiters();
        let _ = self.wake.try_send(());
    }
}
thread_local! { static LISTENER: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            LISTENER.with(|slot| {
                let state = slot.borrow().clone();
                if let Some(state) = state {
                    state.notify(unsafe { GetClipboardSequenceNumber() });
                }
            });
            LRESULT(0)
        }
        SHUTDOWN => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
fn listener(state: Arc<Shared>) {
    let run = || -> Result<(), String> {
        let _sta = Sta::new()?;
        let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            lpszClassName: w!("mIVClipboardCaptureListener"),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0
            && std::io::Error::last_os_error().raw_os_error() != Some(1410)
        {
            return Err(format!(
                "RegisterClassExW: {}",
                std::io::Error::last_os_error()
            ));
        }
        LISTENER.with(|s| *s.borrow_mut() = Some(state.clone()));
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                w!("mIV Clipboard Capture"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module.into()),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        state.hwnd.store(hwnd.0 as isize, Ordering::Release);
        if state.stop.load(Ordering::Acquire) {
            let _ = unsafe { DestroyWindow(hwnd) };
            return Ok(());
        }
        if let Err(e) = unsafe { AddClipboardFormatListener(hwnd) } {
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(format!("AddClipboardFormatListener: {e}"));
        }
        // Deliberately no initial read: enabling captures only subsequent copies.
        let mut msg = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if result.0 == -1 {
                return Err(format!("GetMessageW: {}", std::io::Error::last_os_error()));
            }
            if result.0 == 0 {
                break;
            }
            let _ = unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }
        let _ = unsafe { RemoveClipboardFormatListener(hwnd) };
        Ok(())
    };
    if let Err(e) = run() {
        state.fail(format!("listener: {e}"));
    }
    state.hwnd.store(0, Ordering::Release);
    LISTENER.with(|s| *s.borrow_mut() = None);
}

struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        if let Err(e) = unsafe { CloseClipboard() } {
            crate::logger::log(format!("clipboard_capture: CloseClipboard: {e}"));
        }
    }
}
fn copy_format(id: u32, limit: usize) -> Result<Vec<u8>, String> {
    let handle =
        unsafe { GetClipboardData(id) }.map_err(|e| format!("GetClipboardData({id}): {e}"))?;
    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    if size == 0 || size > limit {
        return Err(format!(
            "クリップボードのデータが空か、上限を超えています ({size} バイト)"
        ));
    }
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return Err(format!("GlobalLock: {}", std::io::Error::last_os_error()));
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
    let _ = unsafe { GlobalUnlock(global) };
    Ok(bytes)
}
fn format_id(entries: &[(u32, String)], name: &str) -> Option<u32> {
    entries
        .iter()
        .find(|(_, n)| data::format_name_eq(n, name))
        .map(|(id, _)| *id)
}

fn copy_named_format(
    entries: &[(u32, String)],
    name: &str,
    limit: usize,
    copy: impl FnOnce(u32, usize) -> Result<Vec<u8>, String>,
) -> Result<Option<Vec<u8>>, String> {
    format_id(entries, name)
        .map(|id| copy(id, limit))
        .transpose()
}
struct Observation {
    before: u32,
    after: u32,
    observed_at: Instant,
    data: Result<ObservedContent, String>,
}
enum ObservedContent {
    Skipped,
    Read(data::ClipboardFormats, data::RawClipboardData),
}

fn preflight_format_ids() -> Result<Vec<(u32, &'static str)>, String> {
    static IDS: std::sync::OnceLock<Result<Vec<(u32, &'static str)>, String>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(register_preflight_formats).clone()
}

fn register_preflight_formats() -> Result<Vec<(u32, &'static str)>, String> {
    data::PREFLIGHT_FORMAT_NAMES
        .iter()
        .map(|&name| {
            let id = match name {
                "CF_DIB" => 8,
                "CF_DIBV5" => 17,
                "CF_HDROP" => 15,
                _ => {
                    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
                    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
                }
            };
            if id == 0 {
                Err(format!(
                    "RegisterClipboardFormatW({name}): {}",
                    std::io::Error::last_os_error()
                ))
            } else {
                Ok((id, name))
            }
        })
        .collect()
}

fn available_formats(formats: &[(u32, &'static str)]) -> data::ClipboardFormats {
    data::ClipboardFormats {
        names: formats
            .iter()
            .filter(|(id, _)| unsafe { IsClipboardFormatAvailable(*id) }.is_ok())
            .map(|(_, name)| (*name).to_owned())
            .collect(),
        ..Default::default()
    }
}

pub(super) fn manual_kind() -> Result<data::ClipboardKind, String> {
    Ok(data::classify_manual(&available_formats(
        &preflight_format_ids()?,
    )))
}

/// Explicit paste reads independently of monitor lifetime/settings. A bounded
/// OpenClipboard retry runs only on this worker; no sequence/hash is consulted.
pub(super) fn read_manual(kind: data::ClipboardKind) -> Result<data::RawClipboardData, String> {
    let _sta = Sta::new()?;
    for attempt in 0..=4 {
        match read_clipboard(ClipboardReadIntent::Manual(kind)) {
            Ok(observation) => {
                return match observation.data? {
                    ObservedContent::Read(_, raw) => Ok(raw),
                    ObservedContent::Skipped => Err("取り込める画像がありません".into()),
                };
            }
            Err(error) if attempt == 4 => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis([25, 50, 100, 250][attempt])),
        }
    }
    unreachable!()
}

// The rejected observation uses the same sequence fence as an opened read.
// It becomes Other only after acceptance, never after an unstable availability
// query or an obsolete generation. Candidates must pass the request/before/after
// fence too: a stale availability result must never reach OpenClipboard.
fn read_with_preflight(
    request: &ReadRequest,
    sequence: impl Fn() -> u32,
    now: impl Fn() -> Instant,
    available: impl FnOnce() -> data::ClipboardFormats,
    read: impl FnOnce() -> Result<Observation, String>,
) -> Result<Observation, String> {
    let before = sequence();
    let formats = available();
    let after = sequence();
    let observed_at = now();
    if request.sequence == before
        && before == after
        && data::should_open_automatic(
            &formats,
            request.snapshot.allows_images(request.sequence),
            request.allows_html(),
        )
    {
        read()
    } else {
        Ok(Observation {
            before,
            after,
            observed_at,
            data: Ok(ObservedContent::Skipped),
        })
    }
}

fn read(request: &ReadRequest, formats: &[(u32, &'static str)]) -> Result<Observation, String> {
    read_with_preflight(
        request,
        || unsafe { GetClipboardSequenceNumber() },
        Instant::now,
        || data::ClipboardFormats {
            names: formats
                .iter()
                .filter(|(id, _)| unsafe { IsClipboardFormatAvailable(*id) }.is_ok())
                .map(|(_, name)| (*name).to_owned())
                .collect(),
            ..Default::default()
        },
        || read_opened(request),
    )
}

fn read_opened(request: &ReadRequest) -> Result<Observation, String> {
    read_clipboard(ClipboardReadIntent::Automatic(request))
}

enum ClipboardReadIntent<'a> {
    Automatic(&'a ReadRequest),
    Manual(data::ClipboardKind),
}

// Pick the bounded payload once at the raw-byte ownership boundary. A manual
// request keeps its UI-selected route if the clipboard changes; it cannot
// fall back to Shell or acquire monitor-only exclusions.
fn requested_payload(
    formats: &data::ClipboardFormats,
    intent: &ClipboardReadIntent<'_>,
) -> data::ClipboardKind {
    match intent {
        ClipboardReadIntent::Manual(selected) => {
            if data::classify_manual(formats) == *selected {
                *selected
            } else {
                data::ClipboardKind::Other
            }
        }
        ClipboardReadIntent::Automatic(request) => {
            let classified = data::classify_automatic(formats, None);
            if classified == data::ClipboardKind::Image
                && request.snapshot.allows_images(request.sequence)
            {
                data::ClipboardKind::Image
            } else if classified == data::ClipboardKind::Other
                && data::classify_manual(formats) == data::ClipboardKind::Html
                && data::should_open_automatic(formats, false, request.allows_html())
            {
                data::ClipboardKind::Html
            } else {
                data::ClipboardKind::Other
            }
        }
    }
}

fn read_clipboard(intent: ClipboardReadIntent<'_>) -> Result<Observation, String> {
    unsafe { OpenClipboard(None) }.map_err(|e| format!("OpenClipboard: {e}"))?;
    let guard = ClipboardGuard;
    let before = unsafe { GetClipboardSequenceNumber() };
    let mut origin_warnings = Vec::new();
    let result = (|| {
        let mut entries = Vec::new();
        let mut id = 0;
        loop {
            id = unsafe { EnumClipboardFormats(id) };
            if id == 0 {
                break;
            }
            let name = match id {
                8 => "CF_DIB".to_owned(),
                17 => "CF_DIBV5".to_owned(),
                15 => "CF_HDROP".to_owned(),
                13 => "CF_UNICODETEXT".to_owned(),
                _ => {
                    let mut name = [0u16; 256];
                    let n = unsafe { GetClipboardFormatNameW(id, &mut name) };
                    if n > 0 {
                        String::from_utf16_lossy(&name[..n as usize])
                    } else {
                        format!("CF_{id}")
                    }
                }
            };
            entries.push((id, name));
        }
        let get = |name: &str, limit: usize| -> Result<Option<Vec<u8>>, String> {
            copy_named_format(&entries, name, limit, copy_format)
        };
        let history = if matches!(intent, ClipboardReadIntent::Automatic(_)) {
            get("CanIncludeInClipboardHistory", 256)?
        } else {
            None
        }
        .and_then(|b| {
            b.get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) != 0)
        });
        let marker = matches!(intent, ClipboardReadIntent::Automatic(_))
            && get(data::ORIGIN_FORMAT_NAME, 256)
                .ok()
                .flatten()
                .is_some_and(|b| data::marker_is_ours(&b));
        let formats = data::ClipboardFormats {
            names: entries.iter().map(|(_, n)| n.clone()).collect(),
            history_allowed: history,
            own_marker: marker,
        };
        let mut raw = data::RawClipboardData::default();
        let kind = requested_payload(&formats, &intent);
        if kind == data::ClipboardKind::Image {
            // Copy only bounded bytes. Interpretation, URL parsing and decoding follow CloseClipboard.
            for (name, target) in [
                ("PNG", &mut raw.png),
                ("CF_DIBV5", &mut raw.dib_v5),
                ("CF_DIB", &mut raw.dib),
            ] {
                if let Some(id) = format_id(&entries, name) {
                    *target = Some(copy_format(id, IMAGE_LIMIT)?);
                    break;
                }
            }
            for (name, target) in [
                ("UniformResourceLocatorW", &mut raw.uri_w),
                ("Chromium internal source URL", &mut raw.chromium_source),
            ] {
                match get(name, SMALL_LIMIT) {
                    Ok(bytes) => *target = bytes,
                    Err(error) => origin_warnings.push(error),
                }
            }
        }
        // SourceURL is parsed only after CloseClipboard. Image+HTML always
        // stays on the image route, even when image monitoring is disabled.
        if kind == data::ClipboardKind::Html {
            raw.html = get("HTML Format", data::MAX_HTML_BYTES)?;
        }
        Ok(ObservedContent::Read(formats, raw))
    })();
    drop(guard);
    let after = unsafe { GetClipboardSequenceNumber() };
    let observed_at = Instant::now();
    for error in origin_warnings {
        crate::logger::log(format!("clipboard_capture: origin omitted: {error}"));
    }
    Ok(Observation {
        before,
        after,
        observed_at,
        data: result,
    })
}

struct SaveJob {
    request: ReadRequest,
    image: data::CapturedImage,
}
fn reader(
    state: Arc<Shared>,
    wake: mpsc::Receiver<()>,
    save: mpsc::SyncSender<SaveJob>,
    popup: Arc<popup::PopupRuntime>,
) {
    let _sta = match Sta::new() {
        Ok(s) => s,
        Err(e) => {
            state.fail(format!("reader: {e}"));
            return;
        }
    };
    let formats = match preflight_format_ids() {
        Ok(formats) => formats,
        Err(error) => {
            state.fail(format!("reader: {error}"));
            return;
        }
    };
    let mut accepted = ReaderState::default();
    while !state.stop.load(Ordering::Acquire) {
        if wake.recv().is_err() {
            break;
        }
        let mut active = state.take_latest();
        while let Some(mut request) = active.take() {
            // A notification can arrive after selecting a sequence reread or
            // while decoding the preceding result. Always prefer its snapshot.
            if let Some(newer) = state.take_latest() {
                request = newer;
            }
            if !accepted.should_read(&request, state.generation.load(Ordering::Acquire))
                || !state.current(&request)
            {
                continue;
            }
            let Some(quiet) = await_quiet_request(
                request,
                &accepted,
                |request| state.current(request),
                || state.take_latest(),
                Instant::now,
                |remaining| match wake.recv_timeout(remaining) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => true,
                    Err(mpsc::RecvTimeoutError::Disconnected) => false,
                },
            ) else {
                continue;
            };
            request = quiet;
            let mut observation = None;
            let mut superseded = None;
            for attempt in 0..=4 {
                if let Some(newer) = state.take_latest() {
                    superseded = Some(newer);
                    break;
                }
                if !state.current(&request) {
                    break;
                }
                match read(&request, &formats) {
                    Ok(value) => {
                        observation = Some(value);
                        break;
                    }
                    Err(error) => {
                        if attempt == 4 {
                            crate::logger::log(format!("clipboard_capture: {error}"));
                            break;
                        }
                        let delay = [25, 50, 100, 250][attempt];
                        let _ = wake.recv_timeout(Duration::from_millis(delay));
                        if state
                            .latest
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .is_some()
                            || !state.current(&request)
                        {
                            break;
                        }
                    }
                }
            }
            let newer = state.take_latest().or(superseded);
            let Some(observation) = observation else {
                if newer.is_none() && state.still_latest(&request) {
                    popup.show(
                        request.snapshot.generation,
                        request.snapshot.dark,
                        popup::PopupContent::Abandoned,
                    );
                }
                active = newer;
                continue;
            };
            match accepted.decide(
                &request,
                observation.before,
                observation.after,
                state.generation.load(Ordering::Acquire),
                newer.is_some(),
            ) {
                ReadDecision::Discard => {
                    active = newer;
                }
                ReadDecision::PreferLatest => {
                    active = newer;
                }
                ReadDecision::Reread(sequence) => {
                    active = Some(request.reread_at(sequence, observation.observed_at));
                }
                ReadDecision::Abandon => {
                    crate::logger::log("clipboard_capture: sequence changed twice; copy skipped");
                    if state.still_latest(&request) {
                        popup.show(
                            request.snapshot.generation,
                            request.snapshot.dark,
                            popup::PopupContent::Abandoned,
                        );
                    }
                    active = newer;
                }
                ReadDecision::Accept => {
                    if !state.current(&request)
                        || !accepted.accept(&request, state.generation.load(Ordering::Acquire))
                    {
                        active = newer;
                        continue;
                    }
                    match observation.data {
                        Ok(ObservedContent::Read(formats, raw)) => {
                            if data::classify_automatic(&formats, None)
                                == data::ClipboardKind::Image
                            {
                                if request.snapshot.allows_images(request.sequence) {
                                    match data::decode_image(&raw) {
                                        Ok(image) if state.current(&request) => {
                                            if accepted.accept_content(
                                                &request,
                                                state.generation.load(Ordering::Acquire),
                                                AcceptedContent::Image(image.content_hash),
                                            ) {
                                                let _ = save.send(SaveJob {
                                                    request: request.clone(),
                                                    image,
                                                });
                                            }
                                        }
                                        Ok(_) => {}
                                        Err(error) => {
                                            accepted.accept_content(
                                                &request,
                                                state.generation.load(Ordering::Acquire),
                                                AcceptedContent::Other,
                                            );
                                            if state.still_latest(&request) {
                                                popup.show(
                                                    request.snapshot.generation,
                                                    request.snapshot.dark,
                                                    popup::PopupContent::Failure(error.clone()),
                                                );
                                            }
                                            crate::logger::log(format!(
                                                "clipboard_capture: decode: {error}"
                                            ));
                                        }
                                    }
                                }
                            } else {
                                accepted.accept_content(
                                    &request,
                                    state.generation.load(Ordering::Acquire),
                                    AcceptedContent::Other,
                                );
                                if !state.selection_open.load(Ordering::Acquire)
                                    && request.html_epoch
                                        == state.html_epoch.load(Ordering::Acquire)
                                    && request.allows_html()
                                    && let Some(bytes) = raw.html.as_deref()
                                    && let Ok(html) = html::parse_cf_html(bytes)
                                    && !html.candidates.is_empty()
                                    && state.still_latest(&request)
                                    && !state.selection_open.load(Ordering::Acquire)
                                    && request.html_epoch
                                        == state.html_epoch.load(Ordering::Acquire)
                                {
                                    let snapshot = SelectionSnapshot::new(
                                        html,
                                        CaptureIntent::Automatic {
                                            snapshot: request.snapshot.clone(),
                                        },
                                        request.html_epoch,
                                    );
                                    popup.show(
                                        request.snapshot.generation,
                                        request.snapshot.dark,
                                        popup::PopupContent::Html(snapshot),
                                    );
                                }
                            }
                        }
                        Ok(ObservedContent::Skipped) => {
                            accepted.accept_content(
                                &request,
                                state.generation.load(Ordering::Acquire),
                                AcceptedContent::Other,
                            );
                        }
                        Err(error) => {
                            accepted.accept_content(
                                &request,
                                state.generation.load(Ordering::Acquire),
                                AcceptedContent::Other,
                            );
                            crate::logger::log(format!("clipboard_capture: read: {error}"));
                            if state.still_latest(&request) {
                                popup.show(
                                    request.snapshot.generation,
                                    request.snapshot.dark,
                                    popup::PopupContent::Failure(error),
                                );
                            }
                        }
                    }
                    active = state.take_latest().or(newer);
                }
            }
        }
    }
}

pub(super) struct CaptureRuntime {
    state: Arc<Shared>,
    listener: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    saver: Option<JoinHandle<()>>,
    popup: Arc<popup::PopupRuntime>,
}
impl CaptureRuntime {
    pub(super) fn start(
        snapshot: Arc<CaptureSnapshot>,
        generation: Arc<AtomicU64>,
        selection_open: Arc<AtomicBool>,
        html_epoch: Arc<AtomicU64>,
        events: mpsc::Sender<CaptureEvent>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let popup = Arc::new(popup::PopupRuntime::start(
            generation.clone(),
            selection_open.clone(),
            html_epoch.clone(),
            events.clone(),
            repaint.clone(),
        )?);
        let (wake, wake_rx) = mpsc::sync_channel(1);
        let state = Arc::new(Shared {
            snapshot: Mutex::new(snapshot),
            generation,
            selection_open,
            html_epoch,
            latest: Mutex::new(None),
            serial: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            wake,
            events,
            repaint,
        });
        let (save_tx, save_rx) = mpsc::sync_channel::<SaveJob>(1);
        let save_state = state.clone();
        let save_popup = popup.clone();
        let saver = std::thread::Builder::new()
            .name("clipboard-capture-save".into())
            .spawn(move || {
                let initial_destination = save_state
                    .snapshot
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .config
                    .destination
                    .clone();
                // Cleanup is best effort once at startup. An unresolved default
                // must not hold the save queue behind a stalled Shell call.
                if let Some(destination) = super::resolved_save_destination(&initial_destination) {
                    if let Err(error) = data::cleanup_part_files(&destination) {
                        crate::logger::log(format!(
                            "clipboard_capture: temporary cleanup: {error}"
                        ));
                    }
                }
                run_save_worker(
                    save_rx,
                    |job| save_state.current(&job.request),
                    |job| {
                        super::resolve_save_destination(
                            &job.request.snapshot.config.destination,
                            || save_state.current(&job.request),
                        )
                    },
                    |job, destination| {
                        data::save_image(&job.image, destination, &data::CaptureTimestamp::now())
                    },
                    |job, result| {
                        let generation = job.request.snapshot.generation;
                        let dark = job.request.snapshot.dark;
                        match result {
                            Ok(saved) => {
                                if let Some(error) = saved.metadata_error {
                                    crate::logger::log(format!("clipboard_capture: MotW: {error}"));
                                }
                                save_popup.show(
                                    generation,
                                    dark,
                                    popup::PopupContent::Saved(saved.path),
                                );
                            }
                            Err(error) => {
                                crate::logger::log(format!("clipboard_capture: save: {error}"));
                                save_popup.show(
                                    generation,
                                    dark,
                                    popup::PopupContent::Failure(error),
                                );
                            }
                        }
                    },
                );
            })
            .map_err(|e| format!("spawn save worker: {e}"))?;
        let reader_state = state.clone();
        let reader_popup = popup.clone();
        let reader = match std::thread::Builder::new()
            .name("clipboard-capture-reader".into())
            .spawn(move || reader(reader_state, wake_rx, save_tx, reader_popup))
        {
            Ok(thread) => thread,
            Err(e) => {
                state.stop();
                return Err(format!("spawn reader: {e}"));
            }
        };
        let listener_state = state.clone();
        let listener = match std::thread::Builder::new()
            .name("clipboard-capture-listener".into())
            .spawn(move || listener(listener_state))
        {
            Ok(thread) => thread,
            Err(e) => {
                state.stop();
                return Err(format!("spawn listener: {e}"));
            }
        };
        Ok(Self {
            state,
            listener: Some(listener),
            reader: Some(reader),
            saver: Some(saver),
            popup,
        })
    }
    pub(super) fn update_snapshot(&self, snapshot: Arc<CaptureSnapshot>) {
        // The service has already advanced generation. Hide before exposing
        // new requests, so this invalidation cannot erase a new worker's Show.
        self.popup.hide();
        self.state.update_snapshot(snapshot);
    }

    pub(super) fn hide_popup(&self) {
        self.popup.hide();
    }

    pub(super) fn resolve_selection(
        &self,
        generation: u64,
        token: u64,
        response: popup::RevealResponse,
    ) {
        self.popup.resolve_selection(generation, token, response);
    }

    pub(super) fn resolve_reveal(
        &self,
        generation: u64,
        path: &std::path::Path,
        response: popup::RevealResponse,
    ) {
        self.popup.resolve_reveal(generation, path, response);
    }
}

// A generation check surrounds destination resolution. Once save() starts,
// settings changes cannot interrupt its file publication; popup has its own fence.
fn run_save_worker<J, T>(
    jobs: mpsc::Receiver<J>,
    current: impl Fn(&J) -> bool,
    resolve: impl Fn(&J) -> Option<Result<std::path::PathBuf, String>>,
    save: impl Fn(&J, &std::path::Path) -> Result<T, String>,
    publish: impl Fn(&J, Result<T, String>),
) {
    while let Ok(job) = jobs.recv() {
        if !current(&job) {
            continue;
        }
        let Some(destination) = resolve(&job) else {
            continue;
        };
        if !current(&job) {
            continue;
        }
        let result = destination.and_then(|destination| save(&job, &destination));
        publish(&job, result);
    }
}

fn finish_worker(thread: JoinHandle<()>, allow_wait: bool) {
    if allow_wait || thread.is_finished() {
        let _ = thread.join();
    } else {
        crate::logger::log("clipboard_capture: worker still in external call; detaching on exit");
    }
}
impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        self.state.stop();
        self.popup.hide();
        let hwnd = self.state.hwnd.load(Ordering::Acquire);
        let posted = hwnd != 0
            && unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), SHUTDOWN, WPARAM(0), LPARAM(0)) }
                .is_ok();
        if let Some(thread) = self.listener.take() {
            finish_worker(thread, posted);
        }
        for thread in [self.reader.take(), self.saver.take()]
            .into_iter()
            .flatten()
        {
            finish_worker(thread, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn read_request(sequence: u32) -> ReadRequest {
        ReadRequest {
            serial: 1,
            sequence,
            snapshot: Arc::new(CaptureSnapshot::updated(
                None,
                super::super::CaptureConfig {
                    images: true,
                    html: false,
                    destination: None,
                },
                10,
                false,
            )),
            notified_at: Instant::now(),
            reread: false,
            html_detection_allowed: true,
            html_epoch: 0,
        }
    }

    fn assert_publication_wait_is_cancelled(invalidate: impl FnOnce(&Shared, &ReadRequest)) {
        let request = read_request(11);
        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        let (events, _) = mpsc::channel();
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(request.snapshot.clone()),
            generation: Arc::new(AtomicU64::new(1)),
            selection_open: Arc::new(AtomicBool::new(false)),
            html_epoch: Arc::new(AtomicU64::new(0)),
            latest: Mutex::new(None),
            serial: AtomicU64::new(1),
            stop: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            wake: wake_tx,
            events,
            repaint: Arc::new(|| {}),
        });
        let waiting = shared.clone();
        let active = request.clone();
        let (entered, entry) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let clock = active.notified_at;
            let result = await_quiet_request(
                active,
                &ReaderState::default(),
                |request| waiting.current(request),
                || waiting.take_latest(),
                || clock,
                |remaining| {
                    assert_eq!(remaining, super::super::PUBLISHER_QUIET_PERIOD);
                    entered.send(()).unwrap();
                    // Freeze time and wait on the production wake channel:
                    // cancellation must wake us, not expire the quiet period.
                    wake_rx.recv().is_ok()
                },
            );
            finished.send(result).unwrap();
        });
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        invalidate(&shared, &request);
        let cancelled = completion.recv_timeout(Duration::from_secs(5));
        // Release a broken wait before reporting the failed assertion.
        let _ = shared.wake.try_send(());
        worker.join().unwrap();
        assert!(cancelled.unwrap().is_none());
    }

    #[test]
    fn publisher_wait_is_released_by_stop_without_advancing_the_clock() {
        assert_publication_wait_is_cancelled(|shared, _| shared.stop());
    }

    #[test]
    fn publisher_wait_is_released_by_settings_generation_without_advancing_the_clock() {
        assert_publication_wait_is_cancelled(|shared, request| {
            let snapshot = Arc::new(CaptureSnapshot::updated(
                Some(&request.snapshot),
                request.snapshot.config.clone(),
                11,
                false,
            ));
            shared
                .generation
                .store(snapshot.generation, Ordering::Release);
            shared.update_snapshot(snapshot);
        });
    }

    #[test]
    fn listener_captures_dialog_admission_without_disabling_automatic_images() {
        let mut request = read_request(11);
        Arc::make_mut(&mut request.snapshot).config.html = true;
        let (wake, _) = mpsc::sync_channel(1);
        let (events, _) = mpsc::channel();
        let state = Shared {
            snapshot: Mutex::new(request.snapshot),
            generation: Arc::new(AtomicU64::new(1)),
            selection_open: Arc::new(AtomicBool::new(true)),
            html_epoch: Arc::new(AtomicU64::new(1)),
            latest: Mutex::new(None),
            serial: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            hwnd: AtomicIsize::new(0),
            wake,
            events,
            repaint: Arc::new(|| {}),
        };
        state.notify(11);
        state.selection_open.store(false, Ordering::Release);
        let captured = state.take_latest().unwrap();
        assert_eq!(captured.html_epoch, 1);
        assert!(!captured.allows_html());
        assert!(captured.snapshot.allows_images(11));
        assert!(ReaderState::default().should_read(&captured, 1));
    }

    #[test]
    fn raw_payload_owner_preserves_manual_exclusion_bypass_and_automatic_image_precedence() {
        let mut request = read_request(11);
        Arc::make_mut(&mut request.snapshot).config.html = true;
        for names in [
            vec!["PNG", "ExcludeClipboardContentFromMonitorProcessing"],
            vec!["PNG", "Object Descriptor"],
            vec!["PNG", data::ORIGIN_FORMAT_NAME],
        ] {
            let formats = data::ClipboardFormats {
                names: names.into_iter().map(str::to_owned).collect(),
                own_marker: true,
                history_allowed: Some(false),
            };
            assert_eq!(
                requested_payload(&formats, &ClipboardReadIntent::Automatic(&request)),
                data::ClipboardKind::Other
            );
            assert_eq!(
                requested_payload(
                    &formats,
                    &ClipboardReadIntent::Manual(data::ClipboardKind::Image)
                ),
                data::ClipboardKind::Image
            );
        }
        let mixed = data::ClipboardFormats {
            names: vec!["PNG".into(), "HTML Format".into()],
            ..Default::default()
        };
        Arc::make_mut(&mut request.snapshot).config.images = false;
        assert_eq!(
            requested_payload(&mixed, &ClipboardReadIntent::Automatic(&request)),
            data::ClipboardKind::Other
        );
        assert_eq!(
            requested_payload(
                &mixed,
                &ClipboardReadIntent::Manual(data::ClipboardKind::Html)
            ),
            data::ClipboardKind::Other
        );
        let html = data::ClipboardFormats {
            names: vec!["HTML Format".into()],
            ..Default::default()
        };
        assert_eq!(
            requested_payload(&html, &ClipboardReadIntent::Automatic(&request)),
            data::ClipboardKind::Html
        );
        request.html_detection_allowed = false;
        assert_eq!(
            requested_payload(&html, &ClipboardReadIntent::Automatic(&request)),
            data::ClipboardKind::Other
        );
        assert_eq!(
            requested_payload(
                &html,
                &ClipboardReadIntent::Manual(data::ClipboardKind::Html)
            ),
            data::ClipboardKind::Html
        );
    }

    #[test]
    fn html_only_powerpoint_payload_is_rejected_automatically_and_accepted_manually() {
        let mut request = read_request(11);
        Arc::make_mut(&mut request.snapshot).config.html = true;
        let context = "<html><img src='/image.png'></html>";
        let header = |start: usize, end: usize| {
            format!(
                "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nSourceURL:https://example.com/page\r\n"
            )
        };
        let start = header(0, 0).len();
        let raw = format!("{}{context}", header(start, start + context.len())).into_bytes();
        let html = html::parse_cf_html(&raw).unwrap();
        assert_eq!(html.page_url, "https://example.com/page");
        assert_eq!(html.candidates, ["https://example.com/image.png"]);

        for office_format in [
            "PowerPoint 12.0 Internal Shapes",
            "pOwErPoInT 12.0 Internal Shapes",
        ] {
            let formats = data::ClipboardFormats {
                names: vec![office_format.into(), "HTML Format".into()],
                ..Default::default()
            };
            // The unknown prefix cannot be observed by the fixed preflight
            // queries, so the complete predicate must act after opening.
            assert!(data::should_open_automatic(&formats, false, true));
            assert_eq!(
                requested_payload(&formats, &ClipboardReadIntent::Automatic(&request)),
                data::ClipboardKind::Other
            );
            assert_eq!(
                data::classify_automatic(&formats, Some(&html.page_url)),
                data::ClipboardKind::Ignored
            );
            assert_eq!(
                requested_payload(
                    &formats,
                    &ClipboardReadIntent::Manual(data::ClipboardKind::Html)
                ),
                data::ClipboardKind::Html
            );
        }
    }

    #[test]
    fn preflight_exclusions_do_not_call_the_opened_reader() {
        let request = read_request(11);
        for excluded in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "Clipboard Viewer Ignore",
            data::ORIGIN_FORMAT_NAME,
            "CF_HDROP",
            "FileGroupDescriptorW",
            "Shell IDList Array",
            "Embed Source",
            "Object Descriptor",
            "XML Spreadsheet",
            "Art::GVML ClipFormat",
            "CF_UNICODETEXT",
        ] {
            let names = if excluded == "CF_UNICODETEXT" {
                vec![excluded.to_owned()]
            } else {
                vec![excluded.to_owned(), "PNG".to_owned()]
            };
            let observation = read_with_preflight(
                &request,
                || 11,
                Instant::now,
                || data::ClipboardFormats {
                    names,
                    ..Default::default()
                },
                || panic!("excluded clipboard must not be opened: {excluded}"),
            )
            .unwrap();
            assert_eq!((observation.before, observation.after), (11, 11));
            assert!(matches!(observation.data, Ok(ObservedContent::Skipped)));
        }
    }

    #[test]
    fn preflight_candidates_use_the_opened_observation_and_keep_post_open_exclusions() {
        for name in ["PNG", "CF_DIBV5", "CF_DIB", "HTML Format"] {
            let mut request = read_request(11);
            Arc::make_mut(&mut request.snapshot).config.html = true;
            let observation = read_with_preflight(
                &request,
                || 11,
                Instant::now,
                || data::ClipboardFormats {
                    names: vec![name.into()],
                    ..Default::default()
                },
                || {
                    Ok(Observation {
                        before: 12,
                        after: 13,
                        observed_at: Instant::now(),
                        data: Ok(ObservedContent::Read(
                            data::ClipboardFormats {
                                names: vec![name.into()],
                                history_allowed: Some(false),
                                own_marker: false,
                            },
                            data::RawClipboardData::default(),
                        )),
                    })
                },
            )
            .unwrap();
            assert_eq!((observation.before, observation.after), (12, 13));
            let Ok(ObservedContent::Read(formats, _)) = observation.data else {
                panic!("must read the candidate");
            };
            assert_eq!(
                data::classify_automatic(&formats, None),
                data::ClipboardKind::Ignored
            );
        }
    }

    #[test]
    fn preflight_sequence_mismatches_never_call_the_opened_reader() {
        let state = ReaderState::default();
        let request = read_request(11);
        for sequences in [[12, 12], [11, 12], [12, 11]] {
            let observed_at = request.notified_at + Duration::from_secs(1);
            let samples = std::cell::RefCell::new(sequences.into_iter());
            let observation = read_with_preflight(
                &request,
                || samples.borrow_mut().next().unwrap(),
                || observed_at,
                || data::ClipboardFormats {
                    names: vec!["PNG".into()],
                    ..Default::default()
                },
                || panic!("unstable image candidate must not open the clipboard"),
            )
            .unwrap();
            assert!(matches!(observation.data, Ok(ObservedContent::Skipped)));
            assert_eq!(observation.observed_at, observed_at);
            assert_eq!(
                state.decide(&request, observation.before, observation.after, 1, true),
                ReadDecision::PreferLatest
            );
            assert_eq!(
                state.decide(&request, observation.before, observation.after, 1, false),
                ReadDecision::Reread(sequences[1])
            );
            let retry = request
                .clone()
                .reread_at(sequences[1], observation.observed_at);
            assert!(Arc::ptr_eq(&retry.snapshot, &request.snapshot));
            assert_eq!(
                state.decide(&retry, sequences[1], sequences[1] + 1, 1, false),
                ReadDecision::Abandon
            );
        }
    }

    #[test]
    fn unnotified_sequence_reread_waits_from_observation_before_opening() {
        let request = read_request(11);
        let observed_at = request.notified_at + Duration::from_secs(1);
        let samples = std::cell::RefCell::new([11, 12].into_iter());
        let state = ReaderState::default();
        let observation = read_with_preflight(
            &request,
            || samples.borrow_mut().next().unwrap(),
            || observed_at,
            || data::ClipboardFormats {
                names: vec!["PNG".into()],
                ..Default::default()
            },
            || panic!("changed sequence must not open"),
        )
        .unwrap();
        let ReadDecision::Reread(sequence) =
            state.decide(&request, observation.before, observation.after, 1, false)
        else {
            panic!("must reread the newly observed sequence");
        };
        let retry = request.clone().reread_at(sequence, observation.observed_at);
        let clock = std::cell::Cell::new(observed_at);
        let mut waits = Vec::new();
        let quiet = await_quiet_request(
            retry,
            &state,
            |_| true,
            || None,
            || clock.get(),
            |remaining| {
                waits.push(remaining);
                clock.set(clock.get() + remaining);
                true
            },
        )
        .unwrap();
        assert_eq!(waits, vec![super::super::PUBLISHER_QUIET_PERIOD]);
        assert_eq!(quiet.notified_at, observed_at);
        assert!(Arc::ptr_eq(&quiet.snapshot, &request.snapshot));
        let result = read_with_preflight(
            &quiet,
            || 12,
            || clock.get(),
            || data::ClipboardFormats {
                names: vec!["PNG".into()],
                ..Default::default()
            },
            || {
                assert_eq!(
                    clock.get(),
                    observed_at + super::super::PUBLISHER_QUIET_PERIOD
                );
                Ok(Observation {
                    before: 12,
                    after: 12,
                    observed_at: clock.get(),
                    data: Ok(ObservedContent::Read(
                        data::ClipboardFormats::default(),
                        data::RawClipboardData::default(),
                    )),
                })
            },
        )
        .unwrap();
        assert!(matches!(result.data, Ok(ObservedContent::Read(..))));
        assert_eq!(
            state.decide(&quiet, result.before, result.after, 1, false),
            ReadDecision::Accept
        );
    }

    #[test]
    fn preflight_uses_each_content_kinds_setting_and_enable_baseline() {
        for (names, images, html, image_baseline, html_baseline, opens) in [
            (
                vec!["CF_UNICODETEXT", "HTML Format"],
                true,
                false,
                10,
                10,
                false,
            ),
            (vec!["HTML Format"], false, true, 10, 10, true),
            (vec!["HTML Format"], true, true, 10, 11, false),
            (vec!["PNG"], true, true, 11, 10, false),
            (vec!["PNG"], false, true, 10, 10, false),
            (vec!["PNG", "HTML Format"], false, true, 10, 10, false),
            (vec!["CF_DIBV5", "HTML Format"], false, true, 10, 10, false),
            (vec!["CF_DIB", "HTML Format"], false, true, 10, 10, false),
            (vec!["PNG", "HTML Format"], true, true, 11, 10, false),
            (vec!["PNG", "HTML Format"], true, true, 10, 11, true),
            (vec!["PNG", "HTML Format"], true, true, 11, 11, false),
        ] {
            let mut request = read_request(11);
            let snapshot = Arc::make_mut(&mut request.snapshot);
            snapshot.config.images = images;
            snapshot.config.html = html;
            snapshot.image_baseline = image_baseline;
            snapshot.html_baseline = html_baseline;
            let opened = std::cell::Cell::new(false);
            let observation = read_with_preflight(
                &request,
                || 11,
                Instant::now,
                || data::ClipboardFormats {
                    names: names.iter().map(|name| (*name).into()).collect(),
                    ..Default::default()
                },
                || {
                    assert!(opens, "disabled kind must not open: {names:?}");
                    opened.set(true);
                    Ok(Observation {
                        before: 11,
                        after: 11,
                        observed_at: Instant::now(),
                        data: Ok(ObservedContent::Read(
                            data::ClipboardFormats::default(),
                            data::RawClipboardData::default(),
                        )),
                    })
                },
            )
            .unwrap();
            assert_eq!(opened.get(), opens, "{names:?}");
            assert_eq!(
                matches!(observation.data, Ok(ObservedContent::Read(..))),
                opens
            );
        }
    }

    #[test]
    fn skipped_observation_keeps_sequence_and_hash_until_the_same_acceptance_fence_passes() {
        let mut state = ReaderState::default();
        let first = read_request(11);
        assert!(state.accept(&first, 1));
        assert!(state.accept_content(&first, 1, AcceptedContent::Image([1; 32])));
        let request = read_request(12);
        let sequences = std::cell::RefCell::new([12, 13].into_iter());
        let unstable = read_with_preflight(
            &request,
            || sequences.borrow_mut().next().unwrap(),
            Instant::now,
            data::ClipboardFormats::default,
            || panic!("text must not open the clipboard"),
        )
        .unwrap();
        assert_eq!(
            state.decide(&request, unstable.before, unstable.after, 1, false),
            ReadDecision::Reread(13)
        );
        assert_eq!(
            state.decide(&request, 12, 12, 2, false),
            ReadDecision::Discard
        );
        assert!(!state.accept(&request, 2));
        assert!(!state.accept_content(&request, 2, AcceptedContent::Other));
        assert_eq!(state.accepted_sequence, Some(11));
        assert_eq!(state.image_hash, Some([1; 32]));
        let stable = read_with_preflight(
            &request,
            || 12,
            Instant::now,
            data::ClipboardFormats::default,
            || panic!("text must not open the clipboard"),
        )
        .unwrap();
        assert!(matches!(stable.data, Ok(ObservedContent::Skipped)));
        assert_eq!(
            state.decide(&request, stable.before, stable.after, 1, false),
            ReadDecision::Accept
        );
        assert!(state.accept(&request, 1));
        assert!(!state.accept_content(&request, 1, AcceptedContent::Other));
        assert_eq!(state.accepted_sequence, Some(12));
        assert_eq!(state.image_hash, None);
        assert!(!state.should_read(&request, 1));
        let again = read_request(13);
        assert!(state.accept(&again, 1));
        assert!(state.accept_content(&again, 1, AcceptedContent::Image([1; 32])));
    }

    #[test]
    fn registered_payload_lookup_ignores_case_for_history_marker_and_preferred_image() {
        let entries = vec![
            (1, "cAnInClUdEiNcLiPbOaRdHiStOrY".into()),
            (2, "MIMAGEVIEWER clipboard ORIGIN V1".into()),
            (3, "pNg".into()),
            (4, "CF_DIBV5".into()),
            (5, "uNiFoRmReSoUrCeLoCaToRw".into()),
        ];
        let get = |name| {
            copy_named_format(&entries, name, 256, |id, limit| {
                assert_eq!(limit, 256);
                Ok(match id {
                    1 => 0u32.to_le_bytes().to_vec(),
                    2 => data::process_nonce().to_vec(),
                    _ => vec![id as u8],
                })
            })
            .unwrap()
            .unwrap()
        };
        assert_eq!(get("CanIncludeInClipboardHistory"), 0u32.to_le_bytes());
        assert!(data::marker_is_ours(&get(data::ORIGIN_FORMAT_NAME)));
        assert_eq!(format_id(&entries, "PNG"), Some(3));
        assert_eq!(get("UniformResourceLocatorW"), vec![5]);
        assert_eq!(format_id(&entries, "missing"), None);
    }

    #[test]
    fn queued_old_save_is_discarded_before_destination_resolution() {
        let (tx, rx) = mpsc::channel();
        tx.send(1u64).unwrap();
        drop(tx);
        run_save_worker::<u64, ()>(
            rx,
            |generation| *generation == 2,
            |_| panic!("obsolete job must not resolve destination"),
            |_, _| panic!("obsolete job must not start saving"),
            |_, _| panic!("obsolete job must not publish"),
        );
    }

    #[test]
    fn generation_changed_while_resolving_default_discards_save_before_it_starts() {
        let generation = Arc::new(AtomicU64::new(1));
        let current = generation.clone();
        let (tx, rx) = mpsc::channel();
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_save_worker::<u64, ()>(
                rx,
                |job| current.load(Ordering::Acquire) == *job,
                |_| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Some(Ok(PathBuf::from("resolved")))
                },
                |_, _| panic!("generation changed before save started"),
                |_, _| panic!("obsolete job must not publish"),
            );
        });
        tx.send(1).unwrap();
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.store(2, Ordering::Release);
        release.send(()).unwrap();
        drop(tx);
        worker.join().unwrap();
    }

    #[test]
    fn explicit_destination_save_progresses_while_old_default_shell_resolution_is_stalled() {
        let resolver = Arc::new(super::super::DestinationResolution::default());
        let (shell_entered, shell_entry) = mpsc::channel();
        let (shell_release, shell_released) = mpsc::channel();
        resolver.start(
            move || {
                shell_entered.send(()).unwrap();
                shell_released.recv().unwrap();
                PathBuf::from("late-default")
            },
            || {},
        );
        shell_entry.recv_timeout(Duration::from_secs(5)).unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        let current = generation.clone();
        let resolving = resolver.clone();
        let (tx, rx) = mpsc::sync_channel::<(u64, Option<PathBuf>)>(1);
        let (entered, entry) = mpsc::channel();
        let (completed, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            // The same startup boundary as the native saver: unresolved
            // cleanup is skipped before consuming the bounded save queue.
            assert_eq!(resolving.resolved_destination(&None), None);
            run_save_worker(
                rx,
                |job| current.load(Ordering::Acquire) == job.0,
                |job| {
                    resolving.resolve_save_destination(&job.1, || {
                        if job.1.is_none() {
                            entered.send(()).unwrap();
                        }
                        current.load(Ordering::Acquire) == job.0
                    })
                },
                |job, destination| {
                    assert_eq!(job.0, 2, "the old default job must not start saving");
                    Ok(destination.to_path_buf())
                },
                |job, result| completed.send((job.0, result.unwrap())).unwrap(),
            );
        });
        tx.send((1, None)).unwrap();
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.store(2, Ordering::Release);
        resolver.wake_waiters();
        tx.send((2, Some(PathBuf::from("explicit-local")))).unwrap();
        let saved = completion.recv_timeout(Duration::from_secs(5));
        assert_eq!(resolver.resolved(), None);
        // Release even if the save did not progress, avoiding a hung test join.
        shell_release.send(()).unwrap();
        assert_eq!(saved.unwrap(), (2, PathBuf::from("explicit-local")));
        drop(tx);
        worker.join().unwrap();
    }

    #[test]
    fn stop_releases_pending_save_without_waiting_for_default_shell_resolution() {
        let resolver = Arc::new(super::super::DestinationResolution::default());
        let (shell_entered, shell_entry) = mpsc::channel();
        let (shell_release, shell_released) = mpsc::channel();
        resolver.start(
            move || {
                shell_entered.send(()).unwrap();
                shell_released.recv().unwrap();
                PathBuf::from("late-default")
            },
            || {},
        );
        shell_entry.recv_timeout(Duration::from_secs(5)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let current = stop.clone();
        let resolving = resolver.clone();
        let (tx, rx) = mpsc::channel();
        let (entered, entry) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_save_worker::<(), ()>(
                rx,
                |_| !current.load(Ordering::Acquire),
                |_| {
                    resolving.resolve_save_destination(&None, || {
                        entered.send(()).unwrap();
                        !current.load(Ordering::Acquire)
                    })
                },
                |_, _| panic!("stopped job must not start saving"),
                |_, _| panic!("stopped job must not publish"),
            );
            finished.send(()).unwrap();
        });
        tx.send(()).unwrap();
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        stop.store(true, Ordering::Release);
        resolver.wake_waiters();
        drop(tx);
        let stopped = completion.recv_timeout(Duration::from_secs(5));
        assert_eq!(resolver.resolved(), None);
        shell_release.send(()).unwrap();
        assert!(
            stopped.is_ok(),
            "save worker still waits for the external Shell call"
        );
        worker.join().unwrap();
    }

    #[test]
    fn started_save_finishes_old_generation_after_setting_change() {
        let generation = Arc::new(AtomicU64::new(1));
        let current = generation.clone();
        let (tx, rx) = mpsc::channel();
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (completed, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_save_worker(
                rx,
                |job| current.load(Ordering::Acquire) == *job,
                |_| Some(Ok(PathBuf::from("original-destination"))),
                |_, destination| {
                    assert_eq!(destination, std::path::Path::new("original-destination"));
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Ok("file-published")
                },
                |job, result| {
                    completed
                        .send((*job, result.unwrap(), current.load(Ordering::Acquire)))
                        .unwrap();
                },
            );
        });
        tx.send(1).unwrap();
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.store(2, Ordering::Release);
        release.send(()).unwrap();
        assert_eq!(
            completion.recv_timeout(Duration::from_secs(5)).unwrap(),
            (1, "file-published", 2)
        );
        drop(tx);
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_detaches_unfinished_reader_and_saver_without_waiting() {
        for _ in 0..2 {
            let (entered, entry) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let (finished, completion) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                entered.send(()).unwrap();
                released.recv().unwrap();
                finished.send(()).unwrap();
            });
            entry.recv_timeout(Duration::from_secs(5)).unwrap();
            let (dropped, observed) = mpsc::channel();
            let shutdown = std::thread::spawn(move || {
                finish_worker(worker, false);
                dropped.send(()).unwrap();
            });
            let detached = observed.recv_timeout(Duration::from_secs(5));
            // Release even on failure, so the regression cannot leave a hung join.
            release.send(()).unwrap();
            assert!(detached.is_ok(), "shutdown waited for unfinished worker");
            completion.recv_timeout(Duration::from_secs(5)).unwrap();
            shutdown.join().unwrap();
        }
    }
}
