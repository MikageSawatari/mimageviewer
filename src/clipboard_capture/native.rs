use super::{
    AcceptedContent, CaptureEvent, CaptureSnapshot, ReadDecision, ReadRequest, ReaderState, data,
    popup,
};
use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
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
    core::w,
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
    latest: Mutex<Option<ReadRequest>>,
    serial: AtomicU64,
    stop: AtomicBool,
    hwnd: AtomicIsize,
    wake: mpsc::SyncSender<()>,
    events: mpsc::Sender<CaptureEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}
impl Shared {
    fn fail(&self, error: String) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
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
        let request = ReadRequest {
            serial: self.serial.fetch_add(1, Ordering::AcqRel) + 1,
            sequence,
            snapshot,
            reread: false,
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
    data: Result<(data::ClipboardFormats, data::RawClipboardData), String>,
}
fn read(request: &ReadRequest) -> Result<Observation, String> {
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
        let history = get("CanIncludeInClipboardHistory", 256)?.and_then(|b| {
            b.get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) != 0)
        });
        let marker = get(data::ORIGIN_FORMAT_NAME, 256)
            .ok()
            .flatten()
            .is_some_and(|b| data::marker_is_ours(&b));
        let formats = data::ClipboardFormats {
            names: entries.iter().map(|(_, n)| n.clone()).collect(),
            history_allowed: history,
            own_marker: marker,
        };
        let mut raw = data::RawClipboardData::default();
        let kind = data::classify_automatic(&formats, None);
        if kind == data::ClipboardKind::Image && request.snapshot.allows_images(request.sequence) {
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
        Ok((formats, raw))
    })();
    drop(guard);
    let after = unsafe { GetClipboardSequenceNumber() };
    for error in origin_warnings {
        crate::logger::log(format!("clipboard_capture: origin omitted: {error}"));
    }
    Ok(Observation {
        before,
        after,
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
                match read(&request) {
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
                    request.sequence = sequence;
                    request.reread = true;
                    active = Some(request);
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
                        Ok((formats, raw)) => {
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
                            }
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
        events: mpsc::Sender<CaptureEvent>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let popup = Arc::new(popup::PopupRuntime::start(
            generation.clone(),
            events.clone(),
            repaint.clone(),
        )?);
        let (wake, wake_rx) = mpsc::sync_channel(1);
        let state = Arc::new(Shared {
            snapshot: Mutex::new(snapshot),
            generation,
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
                if let Ok(destination) = super::resolve_save_destination(&initial_destination) {
                    if let Err(error) = data::cleanup_part_files(&destination) {
                        crate::logger::log(format!(
                            "clipboard_capture: temporary cleanup: {error}"
                        ));
                    }
                }
                run_save_worker(
                    save_rx,
                    |job| save_state.current(&job.request),
                    |job| super::resolve_save_destination(&job.request.snapshot.config.destination),
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
                state.stop.store(true, Ordering::Release);
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
                state.stop.store(true, Ordering::Release);
                let _ = state.wake.try_send(());
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
        *self
            .state
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = snapshot;
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
    resolve: impl Fn(&J) -> Result<std::path::PathBuf, String>,
    save: impl Fn(&J, &std::path::Path) -> Result<T, String>,
    publish: impl Fn(&J, Result<T, String>),
) {
    while let Ok(job) = jobs.recv() {
        if !current(&job) {
            continue;
        }
        let destination = resolve(&job);
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
        self.state.stop.store(true, Ordering::Release);
        let _ = self.state.wake.try_send(());
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
                    Ok(PathBuf::from("resolved"))
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
                |_| Ok(PathBuf::from("original-destination")),
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
