//! App-owned clipboard capture. Notifications carry immutable settings; viewer contexts
//! never own or reset this service. The native reader only copies bytes while open.
pub(crate) mod data;
#[cfg(all(windows, feature = "test-script"))]
pub(crate) mod diagnostics;
pub(crate) mod fetch;
pub(crate) mod html;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod popup;

use std::path::PathBuf;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const PUBLISHER_QUIET_PERIOD: Duration = Duration::from_millis(300);

#[derive(Default)]
enum DestinationState {
    #[default]
    NotStarted,
    Resolving,
    Ready(Result<PathBuf, String>),
}

#[derive(Default)]
struct DestinationResolution {
    state: Mutex<DestinationState>,
    ready: Condvar,
}

impl DestinationResolution {
    fn resolved(&self) -> Option<PathBuf> {
        match &*self.state.lock().unwrap_or_else(|p| p.into_inner()) {
            DestinationState::Ready(Ok(path)) => Some(path.clone()),
            _ => None,
        }
    }

    fn start(
        self: &Arc<Self>,
        resolve: impl FnOnce() -> PathBuf + Send + 'static,
        repaint: impl FnOnce() + Send + 'static,
    ) {
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !matches!(*state, DestinationState::NotStarted) {
                return;
            }
            *state = DestinationState::Resolving;
        }
        let owner = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("clipboard-capture-destination".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(resolve))
                    .map_err(|_| "既定の保存先を確認できませんでした".to_owned());
                owner.publish(result);
                repaint();
            })
        {
            self.publish(Err(format!("spawn destination worker: {error}")));
        }
    }

    fn publish(&self, result: Result<PathBuf, String>) {
        if let Err(error) = &result {
            crate::logger::log(format!("clipboard_capture: destination: {error}"));
        }
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = DestinationState::Ready(result);
        self.ready.notify_all();
    }

    fn resolved_destination(&self, destination: &Option<PathBuf>) -> Option<PathBuf> {
        destination.clone().or_else(|| self.resolved())
    }

    fn resolve_save_destination(
        &self,
        destination: &Option<PathBuf>,
        current: impl Fn() -> bool,
    ) -> Option<Result<PathBuf, String>> {
        match destination {
            Some(path) => current().then(|| Ok(path.clone())),
            None => self.wait(current),
        }
    }

    // Only save workers wait here. A settings change or stop invalidates the
    // request and wakes it without waiting for the external Shell call.
    fn wait(&self, current: impl Fn() -> bool) -> Option<Result<PathBuf, String>> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if !current() {
                return None;
            }
            if let DestinationState::Ready(result) = &*state {
                return Some(result.clone());
            }
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    fn wake_waiters(&self) {
        // The invalidation happens before this lock. Holding the same mutex as
        // wait() prevents a wake from falling between its predicate and wait.
        let _state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.ready.notify_all();
    }
}

fn destination_resolution() -> &'static Arc<DestinationResolution> {
    static DESTINATION: OnceLock<Arc<DestinationResolution>> = OnceLock::new();
    DESTINATION.get_or_init(|| Arc::new(DestinationResolution::default()))
}

pub(crate) fn default_destination() -> Option<PathBuf> {
    destination_resolution().resolved()
}

pub(crate) fn start_default_destination_resolution(ctx: &egui::Context) {
    let ctx = ctx.clone();
    destination_resolution().start(
        || crate::capture::default_output_dir().join("clipboard"),
        move || ctx.request_repaint(),
    );
}

fn resolved_save_destination(destination: &Option<PathBuf>) -> Option<PathBuf> {
    destination_resolution().resolved_destination(destination)
}

fn resolve_save_destination(
    destination: &Option<PathBuf>,
    current: impl Fn() -> bool,
) -> Option<Result<PathBuf, String>> {
    destination_resolution().resolve_save_destination(destination, current)
}

fn wake_save_destination_waiters() {
    destination_resolution().wake_waiters();
}

fn synchronize_runtime<T>(
    runtime: &mut Option<T>,
    enabled: bool,
    start: impl FnOnce() -> Result<T, String>,
    update: impl FnOnce(&T),
) -> Result<(), String> {
    if let Some(runtime) = runtime {
        update(runtime);
    } else if enabled {
        *runtime = Some(start()?);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureConfig {
    pub images: bool,
    pub html: bool,
    pub destination: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureSnapshot {
    pub config: CaptureConfig,
    pub generation: u64,
    pub image_baseline: u32,
    pub html_baseline: u32,
    pub image_enable_count: u64,
    pub dark: bool,
}

impl CaptureSnapshot {
    fn allows_images(&self, sequence: u32) -> bool {
        self.config.images && sequence != self.image_baseline
    }

    fn allows_html(&self, sequence: u32) -> bool {
        self.config.html && sequence != self.html_baseline
    }

    fn updated(previous: Option<&Self>, config: CaptureConfig, sequence: u32, dark: bool) -> Self {
        Self {
            generation: previous.map_or(1, |p| p.generation + 1),
            image_baseline: if config.images && previous.is_none_or(|p| !p.config.images) {
                sequence
            } else {
                previous.map_or(sequence, |p| p.image_baseline)
            },
            html_baseline: if config.html && previous.is_none_or(|p| !p.config.html) {
                sequence
            } else {
                previous.map_or(sequence, |p| p.html_baseline)
            },
            image_enable_count: previous.map_or(0, |p| p.image_enable_count)
                + u64::from(config.images && previous.is_none_or(|p| !p.config.images)),
            config,
            dark,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReadRequest {
    pub serial: u64,
    pub sequence: u32,
    pub snapshot: Arc<CaptureSnapshot>,
    pub notified_at: Instant,
    pub reread: bool,
    // Admission is captured by the listener. A copy made while the modal is
    // open must not become eligible merely because it closes before reading.
    pub html_detection_allowed: bool,
    // Opening a selection retires pre-existing HTML work through reader,
    // popup and App event drain; automatic image saves keep their own lifetime.
    pub html_epoch: u64,
}

impl ReadRequest {
    fn allows_html(&self) -> bool {
        self.html_detection_allowed && self.snapshot.allows_html(self.sequence)
    }

    fn reread_at(mut self, sequence: u32, observed_at: Instant) -> Self {
        self.sequence = sequence;
        self.notified_at = observed_at;
        self.reread = true;
        self
    }
}

// Waiting belongs to the reader. The listener only replaces its latest slot
// and wakes the channel; neither notification delivery nor the UI waits here.
fn await_quiet_request(
    mut request: ReadRequest,
    accepted: &ReaderState,
    current: impl Fn(&ReadRequest) -> bool,
    mut take_latest: impl FnMut() -> Option<ReadRequest>,
    now: impl Fn() -> Instant,
    mut wait: impl FnMut(Duration) -> bool,
) -> Option<ReadRequest> {
    loop {
        if let Some(newer) = take_latest() {
            request = newer;
        }
        if !current(&request) || !accepted.should_read(&request, request.snapshot.generation) {
            return None;
        }
        let remaining = PUBLISHER_QUIET_PERIOD
            .saturating_sub(now().saturating_duration_since(request.notified_at));
        if remaining.is_zero() {
            return Some(request);
        }
        if !wait(remaining) {
            return None;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReadDecision {
    Accept,
    Discard,
    PreferLatest,
    Reread(u32),
    Abandon,
}

enum AcceptedContent {
    Image([u8; 32]),
    Other,
}

#[derive(Default)]
struct ReaderState {
    accepted_sequence: Option<u32>,
    image_hash: Option<[u8; 32]>,
    image_enable_count: u64,
}

impl ReaderState {
    fn should_read(&self, request: &ReadRequest, generation: u64) -> bool {
        request.snapshot.generation == generation
            && (request.snapshot.allows_images(request.sequence) || request.allows_html())
            && self.accepted_sequence != Some(request.sequence)
    }

    fn decide(
        &self,
        request: &ReadRequest,
        before: u32,
        after: u32,
        generation: u64,
        newer: bool,
    ) -> ReadDecision {
        if !self.should_read(request, generation) {
            return ReadDecision::Discard;
        }
        if request.sequence == before && before == after {
            return ReadDecision::Accept;
        }
        if newer {
            ReadDecision::PreferLatest
        } else if !request.reread {
            ReadDecision::Reread(after)
        } else {
            ReadDecision::Abandon
        }
    }

    fn accept(&mut self, request: &ReadRequest, generation: u64) -> bool {
        if !self.should_read(request, generation) {
            return false;
        }
        self.accepted_sequence = Some(request.sequence);
        if self.image_enable_count != request.snapshot.image_enable_count {
            self.image_hash = None;
            self.image_enable_count = request.snapshot.image_enable_count;
        }
        true
    }

    // Decode may finish after a setting change. Only the accepted request's
    // current generation may publish content or alter consecutive-image dedup.
    fn accept_content(
        &mut self,
        request: &ReadRequest,
        generation: u64,
        content: AcceptedContent,
    ) -> bool {
        if request.snapshot.generation != generation
            || self.accepted_sequence != Some(request.sequence)
        {
            return false;
        }
        match content {
            AcceptedContent::Image(hash) => {
                if !request.snapshot.allows_images(request.sequence) {
                    return false;
                }
                let new = self.image_hash != Some(hash);
                self.image_hash = Some(hash);
                new
            }
            AcceptedContent::Other => {
                self.image_hash = None;
                false
            }
        }
    }
}

/// The explicit source owns the destination and eligibility rules. Manual work
/// never consults monitor generations, settings, sequence dedup or exclusions.
#[derive(Clone, Debug)]
pub(crate) enum CaptureIntent {
    Automatic { snapshot: Arc<CaptureSnapshot> },
    Manual { destination: PathBuf },
}

#[derive(Clone, Debug)]
pub(crate) struct SelectionSnapshot {
    pub token: u64,
    pub html: html::HtmlCapture,
    pub intent: CaptureIntent,
    pub timestamp: data::CaptureTimestamp,
    pub html_epoch: u64,
}

impl SelectionSnapshot {
    fn new(html: html::HtmlCapture, intent: CaptureIntent, html_epoch: u64) -> Arc<Self> {
        static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self {
            token: NEXT_TOKEN.fetch_add(1, Ordering::Relaxed),
            html,
            intent,
            timestamp: data::CaptureTimestamp::now(),
            html_epoch,
        })
    }

    fn current(&self, generation: u64, html_epoch: u64) -> bool {
        match &self.intent {
            CaptureIntent::Automatic { snapshot } => {
                snapshot.generation == generation && self.html_epoch == html_epoch
            }
            CaptureIntent::Manual { .. } => true,
        }
    }
}

#[derive(Debug)]
pub(crate) enum SelectionEvent {
    OpenCaptureSelection(Arc<SelectionSnapshot>),
    ManualSaved {
        path: PathBuf,
        metadata_error: Option<String>,
    },
    ManualFailed(String),
}

fn capture_manual_content(
    raw: data::RawClipboardData,
    kind: data::ClipboardKind,
    destination: PathBuf,
) -> Result<SelectionEvent, String> {
    match kind {
        data::ClipboardKind::Image => {
            let image = data::decode_image(&raw)?;
            let saved =
                data::save_image_in_folder(&image, &destination, &data::CaptureTimestamp::now())?;
            Ok(SelectionEvent::ManualSaved {
                path: saved.path,
                metadata_error: saved.metadata_error,
            })
        }
        data::ClipboardKind::Html => {
            let bytes = raw.html.as_deref().unwrap_or_default();
            if data::html_source_url(bytes).is_none() {
                return Err("取り込める画像がありません".into());
            }
            let html = html::parse_cf_html(bytes)?;
            if html.candidates.is_empty() {
                return Err("取り込める画像がありません".into());
            }
            Ok(SelectionEvent::OpenCaptureSelection(
                SelectionSnapshot::new(html, CaptureIntent::Manual { destination }, 0),
            ))
        }
        _ => Err("取り込める画像がありません".into()),
    }
}

#[cfg(windows)]
pub(crate) fn manual_kind() -> Result<data::ClipboardKind, String> {
    native::manual_kind()
}

#[cfg(not(windows))]
pub(crate) fn manual_kind() -> Result<data::ClipboardKind, String> {
    Ok(data::ClipboardKind::Other)
}

pub(crate) enum CaptureEvent {
    StartupFailed(String),
    OpenCaptureSelection(Arc<SelectionSnapshot>),
    RevealSaved { generation: u64, path: PathBuf },
}

pub(crate) struct ClipboardCaptureService {
    snapshot: Option<Arc<CaptureSnapshot>>,
    generation: Arc<AtomicU64>,
    startup_error: Option<String>,
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
    selection_open: Arc<AtomicBool>,
    html_epoch: Arc<AtomicU64>,
    manual_busy: Arc<AtomicBool>,
    manual_tx: mpsc::Sender<SelectionEvent>,
    manual_rx: mpsc::Receiver<SelectionEvent>,
    selection_events: Vec<SelectionEvent>,
    #[cfg(windows)]
    runtime: Option<native::CaptureRuntime>,
}

impl Default for ClipboardCaptureService {
    fn default() -> Self {
        let (manual_tx, manual_rx) = mpsc::channel();
        Self {
            snapshot: None,
            generation: Arc::new(AtomicU64::new(0)),
            startup_error: None,
            event_rx: None,
            selection_open: Arc::new(AtomicBool::new(false)),
            html_epoch: Arc::new(AtomicU64::new(0)),
            manual_busy: Arc::new(AtomicBool::new(false)),
            manual_tx,
            manual_rx,
            selection_events: Vec::new(),
            #[cfg(windows)]
            runtime: None,
        }
    }
}

impl ClipboardCaptureService {
    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn smoke_monitor_ready(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.smoke_monitor_ready())
    }

    #[cfg(all(windows, feature = "test-script"))]
    pub(crate) fn smoke_popup_visible(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.smoke_popup_visible())
    }

    pub(crate) fn startup_failed(&self) -> bool {
        self.startup_error.is_some()
    }

    #[cfg(windows)]
    pub(crate) fn synchronize(
        &mut self,
        settings: &crate::settings::Settings,
        dark: bool,
        ctx: &egui::Context,
        hwnd: isize,
    ) {
        let config = CaptureConfig {
            images: settings.clipboard_capture_image_enabled,
            html: settings.clipboard_capture_html_enabled,
            destination: settings.clipboard_capture_output_dir.clone(),
        };
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.config == config && s.dark == dark)
        {
            return;
        }
        let sequence =
            unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let snapshot = Arc::new(CaptureSnapshot::updated(
            self.snapshot.as_deref(),
            config,
            sequence,
            dark,
        ));
        self.generation
            .store(snapshot.generation, Ordering::Release);
        self.snapshot = Some(snapshot.clone());
        let (tx, rx) = mpsc::channel();
        let repaint_ctx = ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            repaint_ctx.request_repaint();
            let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut _);
            if !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) }.as_bool()
            {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(hwnd),
                        windows::Win32::UI::WindowsAndMessaging::WM_PAINT,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    )
                };
            }
        });
        let mut started = false;
        let result = synchronize_runtime(
            &mut self.runtime,
            (snapshot.config.images || snapshot.config.html) && self.startup_error.is_none(),
            || {
                let runtime = native::CaptureRuntime::start(
                    snapshot.clone(),
                    self.generation.clone(),
                    self.selection_open.clone(),
                    self.html_epoch.clone(),
                    tx,
                    repaint,
                )?;
                started = true;
                Ok(runtime)
            },
            |runtime| runtime.update_snapshot(snapshot.clone()),
        );
        if started {
            self.event_rx = Some(rx);
        }
        if let Err(error) = result {
            self.record_startup_error(error);
        }
    }

    fn record_startup_error(&mut self, error: String) {
        crate::logger::log(format!("clipboard_capture: startup failed: {error}"));
        self.startup_error = Some(error);
        #[cfg(windows)]
        if let Some(runtime) = self.runtime.take() {
            drop(runtime);
        }
    }

    pub(crate) fn poll(&mut self) -> Vec<PathBuf> {
        let events: Vec<_> = self
            .event_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        let mut reveal = Vec::new();
        for event in events {
            match event {
                CaptureEvent::StartupFailed(error) => self.record_startup_error(error),
                CaptureEvent::OpenCaptureSelection(snapshot) => {
                    if snapshot.current(
                        self.generation.load(Ordering::Acquire),
                        self.html_epoch.load(Ordering::Acquire),
                    ) && !self.selection_open.load(Ordering::Acquire)
                    {
                        self.selection_events
                            .push(SelectionEvent::OpenCaptureSelection(snapshot));
                    }
                }
                CaptureEvent::RevealSaved { generation, path } => {
                    if generation == self.generation.load(Ordering::Acquire) {
                        reveal.push(path);
                    }
                }
            }
        }
        reveal
    }

    pub(crate) fn poll_selection(&mut self) -> Vec<SelectionEvent> {
        self.selection_events.extend(self.manual_rx.try_iter());
        let generation = self.generation.load(Ordering::Acquire);
        std::mem::take(&mut self.selection_events)
            .into_iter()
            .filter(|event| match event {
                SelectionEvent::OpenCaptureSelection(snapshot) => {
                    snapshot.current(generation, self.html_epoch.load(Ordering::Acquire))
                        && !self.selection_open.load(Ordering::Acquire)
                }
                _ => true,
            })
            .collect()
    }

    pub(crate) fn set_selection_open(&mut self, open: bool) {
        let was_open = self.selection_open.swap(open, Ordering::AcqRel);
        if open && !was_open {
            self.html_epoch.fetch_add(1, Ordering::AcqRel);
        }
        if open {
            self.selection_events
                .retain(|event| !matches!(event, SelectionEvent::OpenCaptureSelection(_)));
            #[cfg(windows)]
            if let Some(runtime) = &self.runtime {
                runtime.hide_popup();
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn resolve_selection(&self, token: u64, admitted: bool) {
        if let Some(runtime) = &self.runtime {
            runtime.resolve_selection(
                self.generation.load(Ordering::Acquire),
                token,
                if admitted {
                    popup::RevealResponse::Accepted
                } else {
                    popup::RevealResponse::Unavailable
                },
            );
        }
    }

    #[cfg(windows)]
    pub(crate) fn manual_capture(
        &self,
        kind: data::ClipboardKind,
        destination: PathBuf,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if !matches!(kind, data::ClipboardKind::Image | data::ClipboardKind::Html) {
            return Err("取り込める画像がありません".into());
        }
        if self.manual_busy.swap(true, Ordering::AcqRel) {
            return Err("クリップボードの取り込み中です".into());
        }
        let busy = self.manual_busy.clone();
        let events = self.manual_tx.clone();
        let ctx = ctx.clone();
        let result = std::thread::Builder::new()
            .name("clipboard-capture-manual".into())
            .spawn(move || {
                let result = native::read_manual(kind)
                    .and_then(|raw| capture_manual_content(raw, kind, destination));
                let event = result.unwrap_or_else(SelectionEvent::ManualFailed);
                let _ = events.send(event);
                busy.store(false, Ordering::Release);
                ctx.request_repaint();
            });
        if let Err(error) = result {
            self.manual_busy.store(false, Ordering::Release);
            return Err(format!("取り込みを開始できませんでした: {error}"));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub(crate) fn manual_capture(
        &self,
        _: data::ClipboardKind,
        _: PathBuf,
        _: &egui::Context,
    ) -> Result<(), String> {
        Err("この環境ではクリップボードの取り込みは利用できません".into())
    }

    #[cfg(not(windows))]
    pub(crate) fn resolve_selection(&self, _: u64, _: bool) {}

    #[cfg(test)]
    pub(crate) fn inject_selection_event_for_test(&mut self, event: SelectionEvent) {
        self.manual_tx.send(event).unwrap();
    }

    #[cfg(test)]
    pub(crate) fn inject_reveal_for_test(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.event_rx = Some(rx);
        tx.send(CaptureEvent::RevealSaved {
            generation: self.generation.load(Ordering::Acquire),
            path,
        })
        .unwrap();
    }

    #[cfg(windows)]
    pub(crate) fn resolve_reveal(&self, path: &std::path::Path, admitted: bool) {
        if let Some(runtime) = &self.runtime {
            runtime.resolve_reveal(
                self.generation.load(Ordering::Acquire),
                path,
                if admitted {
                    popup::RevealResponse::Accepted
                } else {
                    popup::RevealResponse::Unavailable
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn html_selection(intent: CaptureIntent, epoch: u64) -> Arc<SelectionSnapshot> {
        SelectionSnapshot::new(
            html::HtmlCapture {
                page_url: "https://example.com/page".into(),
                candidates: vec!["https://example.com/a.png".into()],
                omitted: 0,
            },
            intent,
            epoch,
        )
    }

    #[test]
    fn selection_open_invalidates_automatic_html_across_close_but_manual_ignores_monitor_generation()
     {
        let mut service = ClipboardCaptureService::default();
        service.generation.store(1, Ordering::Release);
        let automatic = html_selection(
            CaptureIntent::Automatic {
                snapshot: snapshot(false, true, 10),
            },
            0,
        );
        let manual = html_selection(
            CaptureIntent::Manual {
                destination: PathBuf::from("current-folder"),
            },
            0,
        );
        service
            .selection_events
            .push(SelectionEvent::OpenCaptureSelection(automatic.clone()));
        service.set_selection_open(true);
        assert!(service.poll_selection().is_empty());
        service.set_selection_open(false);
        service
            .selection_events
            .push(SelectionEvent::OpenCaptureSelection(automatic));
        service
            .manual_tx
            .send(SelectionEvent::OpenCaptureSelection(manual.clone()))
            .unwrap();
        service.generation.store(99, Ordering::Release);
        let events = service.poll_selection();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], SelectionEvent::OpenCaptureSelection(snapshot) if snapshot.token == manual.token)
        );
    }

    #[test]
    fn html_notifications_during_selection_are_not_eligible_after_close() {
        let mut request = request(11);
        request.snapshot = snapshot(false, true, 10);
        request.html_detection_allowed = false;
        assert!(!ReaderState::default().should_read(&request, 1));
        let reread = request.reread_at(12, Instant::now());
        assert!(!reread.allows_html());
        // The same request still permits automatic image saves.
        let mut image = reread;
        image.snapshot = snapshot(true, true, 10);
        assert!(ReaderState::default().should_read(&image, 1));
    }

    #[test]
    fn manual_images_save_repeatedly_directly_in_current_folder_without_monitor() {
        let folder = tempfile::tempdir().unwrap();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let png = bytes.into_inner();
        let mut paths = Vec::new();
        for _ in 0..2 {
            let event = capture_manual_content(
                data::RawClipboardData {
                    png: Some(png.clone()),
                    ..Default::default()
                },
                data::ClipboardKind::Image,
                folder.path().to_path_buf(),
            )
            .unwrap();
            let SelectionEvent::ManualSaved { path, .. } = event else {
                panic!("manual image did not save");
            };
            assert_eq!(path.parent(), Some(folder.path()));
            assert!(path.exists());
            paths.push(path);
        }
        assert_ne!(paths[0], paths[1]);
    }

    fn manual_html_fixture(fragment: &str, source: &str) -> Vec<u8> {
        let context = format!("<html><!--StartFragment-->{fragment}<!--EndFragment--></html>");
        let header = |start: usize, end: usize| {
            format!(
                "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:-1\r\nEndFragment:-1\r\nSourceURL:{source}\r\n"
            )
        };
        let start = header(0, 0).len();
        format!("{}{context}", header(start, start + context.len())).into_bytes()
    }

    #[test]
    fn manual_html_without_source_or_candidates_returns_no_image_and_never_a_shell_event() {
        for html in [
            b"<html><img src='https://example.com/a.png'></html>".to_vec(),
            manual_html_fixture("<p>text</p>", "https://example.com/page"),
        ] {
            let result = capture_manual_content(
                data::RawClipboardData {
                    html: Some(html),
                    ..Default::default()
                },
                data::ClipboardKind::Html,
                PathBuf::from("current-folder"),
            );
            assert!(matches!(result, Err(ref message) if message == "取り込める画像がありません"));
        }
    }

    #[test]
    fn manual_html_keeps_current_destination_and_never_consults_monitor_config() {
        let destination = PathBuf::from("current-folder");
        let result = capture_manual_content(
            data::RawClipboardData {
                html: Some(manual_html_fixture(
                    "<img src='/a.png'>",
                    "https://example.com/page",
                )),
                ..Default::default()
            },
            data::ClipboardKind::Html,
            destination.clone(),
        )
        .unwrap();
        let SelectionEvent::OpenCaptureSelection(snapshot) = result else {
            panic!("manual html did not select");
        };
        assert!(
            matches!(&snapshot.intent, CaptureIntent::Manual { destination: captured } if captured == &destination)
        );
        assert_eq!(snapshot.html.candidates, ["https://example.com/a.png"]);
        assert!(snapshot.current(999, 999));
    }

    #[test]
    fn default_destination_is_resolved_once_off_caller_and_shared_with_save_waiters() {
        let resolver = Arc::new(DestinationResolution::default());
        let caller = std::thread::current().id();
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (wake, woken) = mpsc::channel();
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        resolver.start(
            move || {
                assert_ne!(std::thread::current().id(), caller);
                counted.fetch_add(1, Ordering::AcqRel);
                entered.send(()).unwrap();
                released.recv().unwrap();
                PathBuf::from("shared-default")
            },
            move || {
                wake.send(()).unwrap();
            },
        );
        entry.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(resolver.resolved(), None);
        resolver.start(|| panic!("second resolver must not start"), || {});
        let save_resolver = resolver.clone();
        let (waiting, waiter_started) = mpsc::channel();
        let (result, results) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            waiting.send(()).unwrap();
            result.send(save_resolver.wait(|| true)).unwrap();
        });
        waiter_started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(results.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            results
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .unwrap(),
            PathBuf::from("shared-default")
        );
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(resolver.resolved(), Some(PathBuf::from("shared-default")));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        resolver.start(|| panic!("completed resolver must not restart"), || {});
        waiter.join().unwrap();
    }

    #[test]
    fn startup_cleanup_uses_only_an_available_destination_without_waiting() {
        let resolver = DestinationResolution::default();
        *resolver.state.lock().unwrap() = DestinationState::Resolving;
        assert_eq!(resolver.resolved_destination(&None), None);
        let explicit = Some(PathBuf::from("explicit-local"));
        assert_eq!(resolver.resolved_destination(&explicit), explicit);
        resolver.publish(Ok(PathBuf::from("resolved-default")));
        assert_eq!(
            resolver.resolved_destination(&None),
            Some(PathBuf::from("resolved-default"))
        );
        assert_eq!(resolver.resolved_destination(&explicit), explicit);
    }

    #[test]
    fn destination_invalidation_wake_cannot_be_lost_between_predicate_and_wait() {
        let resolver = Arc::new(DestinationResolution::default());
        let generation = Arc::new(AtomicU64::new(1));
        let (checked, check) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let (result, results) = mpsc::channel();
        let waiting = resolver.clone();
        let current = generation.clone();
        let waiter = std::thread::spawn(move || {
            let calls = AtomicU64::new(0);
            let resolved = waiting.wait(|| {
                let valid = current.load(Ordering::Acquire) == 1;
                if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                    // The wait mutex is held across this first predicate. Force
                    // invalidation to happen before Condvar::wait releases it.
                    checked.send(()).unwrap();
                    resumed.recv().unwrap();
                }
                valid
            });
            result.send(resolved).unwrap();
        });
        check.recv_timeout(Duration::from_secs(5)).unwrap();
        let (invalidated, invalidation) = mpsc::channel();
        let waking = resolver.clone();
        let notifier = std::thread::spawn(move || {
            generation.store(2, Ordering::Release);
            invalidated.send(()).unwrap();
            waking.wake_waiters();
        });
        invalidation.recv_timeout(Duration::from_secs(5)).unwrap();
        resume.send(()).unwrap();
        let canceled = results.recv_timeout(Duration::from_secs(5));
        // Unblock a broken implementation before reporting a regression.
        resolver.publish(Ok(PathBuf::from("late-default")));
        assert_eq!(canceled.unwrap(), None);
        waiter.join().unwrap();
        notifier.join().unwrap();
    }

    #[test]
    fn capture_runtime_starts_on_first_enable_and_survives_off_then_on() {
        struct FakeRuntime {
            tx: mpsc::Sender<bool>,
        }
        let mut runtime = None;
        synchronize_runtime::<FakeRuntime>(
            &mut runtime,
            false,
            || panic!("both OFF"),
            |_| panic!("no runtime yet"),
        )
        .unwrap();
        assert!(runtime.is_none());
        let (tx, rx) = mpsc::channel();
        let (seen, observed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            while let Ok(enabled) = rx.recv() {
                seen.send(enabled).unwrap();
            }
        });
        synchronize_runtime(
            &mut runtime,
            true,
            || Ok(FakeRuntime { tx }),
            |_| panic!("new runtime"),
        )
        .unwrap();
        let first_owner = runtime.as_ref().unwrap() as *const FakeRuntime;
        for enabled in [false, true] {
            synchronize_runtime(
                &mut runtime,
                enabled,
                || panic!("must retain runtime"),
                |runtime| {
                    runtime.tx.send(enabled).unwrap();
                },
            )
            .unwrap();
            assert_eq!(runtime.as_ref().unwrap() as *const FakeRuntime, first_owner);
            assert_eq!(
                observed.recv_timeout(Duration::from_secs(5)).unwrap(),
                enabled
            );
        }
        drop(runtime);
        worker.join().unwrap();
    }
    fn snapshot(images: bool, html: bool, sequence: u32) -> Arc<CaptureSnapshot> {
        Arc::new(CaptureSnapshot::updated(
            None,
            CaptureConfig {
                images,
                html,
                destination: Some(PathBuf::from("capture")),
            },
            sequence,
            false,
        ))
    }
    fn request(sequence: u32) -> ReadRequest {
        ReadRequest {
            serial: 1,
            sequence,
            snapshot: snapshot(true, false, 10),
            notified_at: Instant::now(),
            reread: false,
            html_detection_allowed: true,
            html_epoch: 0,
        }
    }

    #[test]
    fn publisher_wait_merges_notifications_and_waits_from_the_latest_notification() {
        use std::cell::{Cell, RefCell};
        let start = Instant::now();
        let clock = Cell::new(start);
        let latest = RefCell::new(None);
        let mut waits = Vec::new();
        let first = ReadRequest {
            notified_at: start,
            ..request(11)
        };
        let mut final_request = None;
        let quiet = await_quiet_request(
            first,
            &ReaderState::default(),
            |_| true,
            || latest.borrow_mut().take(),
            || clock.get(),
            |remaining| {
                waits.push(remaining);
                if waits.len() == 1 {
                    // The reader wakes after multiple publications. Their
                    // individual snapshots/timestamps belong to the slot.
                    *latest.borrow_mut() = Some(ReadRequest {
                        serial: 2,
                        notified_at: start + Duration::from_millis(25),
                        ..request(12)
                    });
                    let last = ReadRequest {
                        serial: 3,
                        notified_at: start + Duration::from_millis(156),
                        ..request(13)
                    };
                    final_request = Some(last.clone());
                    *latest.borrow_mut() = Some(last);
                    clock.set(start + Duration::from_millis(156));
                } else {
                    clock.set(clock.get() + remaining);
                }
                true
            },
        )
        .unwrap();
        let last = final_request.unwrap();
        assert_eq!(quiet.sequence, 13);
        assert_eq!(quiet.serial, 3);
        assert!(Arc::ptr_eq(&quiet.snapshot, &last.snapshot));
        assert_eq!(quiet.notified_at, last.notified_at);
        assert_eq!(clock.get(), start + Duration::from_millis(456));
        assert_eq!(waits, vec![PUBLISHER_QUIET_PERIOD; 2]);
    }

    #[test]
    fn publisher_wait_without_a_new_notification_does_not_restart_the_deadline() {
        use std::cell::Cell;
        let start = Instant::now();
        let clock = Cell::new(start);
        let mut waits = Vec::new();
        let quiet = await_quiet_request(
            ReadRequest {
                notified_at: start,
                ..request(11)
            },
            &ReaderState::default(),
            |_| true,
            || None,
            || clock.get(),
            |remaining| {
                waits.push(remaining);
                clock.set(
                    clock.get()
                        + if waits.len() == 1 {
                            Duration::from_millis(100)
                        } else {
                            remaining
                        },
                );
                true
            },
        )
        .unwrap();
        assert_eq!(quiet.sequence, 11);
        assert_eq!(
            waits,
            vec![Duration::from_millis(300), Duration::from_millis(200)]
        );
        assert_eq!(clock.get(), start + PUBLISHER_QUIET_PERIOD);
    }

    #[test]
    fn publisher_wait_does_not_add_another_delay_for_an_already_quiet_sequence_reread() {
        let start = Instant::now();
        let r = request(11).reread_at(12, start);
        let quiet = await_quiet_request(
            r.clone(),
            &ReaderState::default(),
            |_| true,
            || None,
            || start + Duration::from_millis(400),
            |_| panic!("the observed sequence has already been quiet for 300 ms"),
        )
        .unwrap();
        assert!(quiet.reread);
        assert!(Arc::ptr_eq(&quiet.snapshot, &r.snapshot));
        assert_eq!(quiet.notified_at, r.notified_at);
    }

    #[test]
    fn publisher_wait_replaces_an_unnotified_reread_with_the_latest_notification() {
        let original = request(11);
        let observed_at = original.notified_at + Duration::from_secs(1);
        let retry = original.clone().reread_at(12, observed_at);
        let notification = ReadRequest {
            serial: 2,
            notified_at: observed_at + Duration::from_millis(25),
            snapshot: snapshot(true, true, 10),
            ..request(13)
        };
        let latest = std::cell::RefCell::new(None);
        let clock = std::cell::Cell::new(observed_at);
        let mut waits = Vec::new();
        let quiet = await_quiet_request(
            retry,
            &ReaderState::default(),
            |_| true,
            || latest.borrow_mut().take(),
            || clock.get(),
            |remaining| {
                waits.push(remaining);
                if waits.len() == 1 {
                    clock.set(notification.notified_at);
                    *latest.borrow_mut() = Some(notification.clone());
                } else {
                    clock.set(clock.get() + remaining);
                }
                true
            },
        )
        .unwrap();
        assert_eq!(waits, vec![PUBLISHER_QUIET_PERIOD; 2]);
        assert_eq!(quiet.sequence, 13);
        assert_eq!(quiet.serial, 2);
        assert!(!quiet.reread);
        assert!(Arc::ptr_eq(&quiet.snapshot, &notification.snapshot));
        assert!(!Arc::ptr_eq(&quiet.snapshot, &original.snapshot));
        assert_eq!(
            clock.get(),
            notification.notified_at + PUBLISHER_QUIET_PERIOD
        );
    }

    #[test]
    fn publisher_wait_discards_disabled_baseline_duplicate_and_obsolete_requests_before_waiting() {
        let mut accepted = ReaderState::default();
        assert!(accepted.accept(&request(11), 1));
        let requests = [
            request(11),
            request(10),
            ReadRequest {
                snapshot: snapshot(false, false, 10),
                ..request(12)
            },
        ];
        for r in requests {
            assert!(
                await_quiet_request(
                    r,
                    &accepted,
                    |_| true,
                    || None,
                    Instant::now,
                    |_| panic!("discarded request must not wait")
                )
                .is_none()
            );
        }
        assert!(
            await_quiet_request(
                request(12),
                &accepted,
                |_| false,
                || None,
                Instant::now,
                |_| panic!("obsolete request must not wait")
            )
            .is_none()
        );
        assert_eq!(accepted.accepted_sequence, Some(11));
    }

    #[test]
    fn publisher_wait_rechecks_the_merged_requests_settings_before_reading() {
        let start = Instant::now();
        let latest = std::cell::RefCell::new(None);
        let mut waits = 0;
        assert!(
            await_quiet_request(
                ReadRequest {
                    notified_at: start,
                    ..request(11)
                },
                &ReaderState::default(),
                |_| true,
                || latest.borrow_mut().take(),
                || start,
                |_| {
                    waits += 1;
                    *latest.borrow_mut() = Some(ReadRequest {
                        snapshot: snapshot(false, false, 10),
                        ..request(12)
                    });
                    true
                },
            )
            .is_none()
        );
        assert_eq!(waits, 1);
    }
    #[test]
    fn mismatches_prefer_latest_then_allow_only_one_snapshot_preserving_retry() {
        let state = ReaderState::default();
        let r = request(11);
        assert_eq!(state.decide(&r, 11, 11, 1, false), ReadDecision::Accept);
        assert_eq!(
            state.decide(&r, 11, 12, 1, true),
            ReadDecision::PreferLatest
        );
        let ReadDecision::Reread(sequence) = state.decide(&r, 12, 12, 1, false) else {
            panic!("one reread required");
        };
        let observed_at = r.notified_at + Duration::from_secs(1);
        let retry = r.clone().reread_at(sequence, observed_at);
        assert!(Arc::ptr_eq(&retry.snapshot, &r.snapshot));
        assert_eq!(retry.serial, r.serial);
        assert_eq!(retry.notified_at, observed_at);
        assert_eq!(
            state.decide(&retry, 12, 13, 1, false),
            ReadDecision::Abandon
        );
        assert_eq!(
            state.decide(&retry, 12, 13, 1, true),
            ReadDecision::PreferLatest
        );
        assert_eq!(state.decide(&retry, 12, 12, 1, false), ReadDecision::Accept);
        assert_eq!(state.accepted_sequence, None);
        assert_eq!(state.image_hash, None);
    }
    #[test]
    fn duplicate_notifications_and_both_disabled_are_discarded() {
        let mut state = ReaderState::default();
        let r = request(11);
        assert!(state.accept(&r, 1));
        assert!(!state.should_read(&r, 1));
        assert_eq!(
            state.decide(
                &ReadRequest {
                    serial: 2,
                    ..r.clone()
                },
                11,
                11,
                1,
                false
            ),
            ReadDecision::Discard
        );
        let off = ReadRequest {
            snapshot: snapshot(false, false, 0),
            sequence: 12,
            ..r
        };
        assert!(!state.should_read(&off, 1));
        assert!(!state.accept(&off, 1));
        assert_eq!(state.accepted_sequence, Some(11));
    }
    #[test]
    fn each_enable_boundary_preserves_the_other_baseline() {
        let first = snapshot(true, false, 40);
        let second = CaptureSnapshot::updated(
            Some(&first),
            CaptureConfig {
                html: true,
                ..first.config.clone()
            },
            41,
            false,
        );
        assert_eq!((second.image_baseline, second.html_baseline), (40, 41));
        assert_eq!(second.image_enable_count, 1);
        let off = CaptureSnapshot::updated(
            Some(&second),
            CaptureConfig {
                images: false,
                ..second.config.clone()
            },
            42,
            false,
        );
        let on = CaptureSnapshot::updated(Some(&off), first.config.clone(), 43, false);
        assert_eq!(on.image_baseline, 43);
        assert_eq!(on.image_enable_count, 2);
    }
    #[test]
    fn startup_and_each_check_enable_exclude_existing_content_including_rereads() {
        let state = ReaderState::default();
        let initial = ReadRequest {
            sequence: 40,
            snapshot: snapshot(true, false, 40),
            ..request(11)
        };
        assert_eq!(
            state.decide(&initial, 40, 40, 1, false),
            ReadDecision::Discard
        );
        let enabled_html = Arc::new(CaptureSnapshot::updated(
            Some(&initial.snapshot),
            CaptureConfig {
                html: true,
                ..initial.snapshot.config.clone()
            },
            41,
            false,
        ));
        assert!(enabled_html.allows_images(41));
        assert!(!enabled_html.allows_html(41));
        assert!(enabled_html.allows_html(42));
        let enabled_image = Arc::new(CaptureSnapshot::updated(
            Some(&snapshot(false, true, 40)),
            CaptureConfig {
                images: true,
                html: true,
                destination: Some(PathBuf::from("capture")),
            },
            41,
            false,
        ));
        assert!(!enabled_image.allows_images(41));
        assert!(enabled_image.allows_html(41));
        let baseline_retry = ReadRequest {
            sequence: 40,
            reread: true,
            ..initial
        };
        assert!(!state.should_read(&baseline_retry, 1));
        // Sequence wrap is equality based, with no numeric ordering assumption.
        let wrapped = ReadRequest {
            sequence: 0,
            snapshot: snapshot(true, false, u32::MAX),
            ..request(11)
        };
        assert!(state.should_read(&wrapped, 1));
    }
    fn accept_content(
        state: &mut ReaderState,
        request: &ReadRequest,
        content: AcceptedContent,
    ) -> bool {
        let generation = request.snapshot.generation;
        assert_eq!(
            state.decide(
                request,
                request.sequence,
                request.sequence,
                generation,
                false
            ),
            ReadDecision::Accept
        );
        assert!(state.accept(request, generation));
        state.accept_content(request, generation, content)
    }
    #[test]
    fn consecutive_images_deduplicate_but_observed_text_or_rejected_image_breaks_the_run() {
        let mut state = ReaderState::default();
        let first = request(11);
        assert!(accept_content(
            &mut state,
            &first,
            AcceptedContent::Image([1; 32])
        ));
        let duplicate = ReadRequest {
            sequence: 12,
            ..first.clone()
        };
        assert!(!accept_content(
            &mut state,
            &duplicate,
            AcceptedContent::Image([1; 32])
        ));
        let text = ReadRequest {
            sequence: 13,
            ..first.clone()
        };
        assert!(!accept_content(&mut state, &text, AcceptedContent::Other));
        let after_text = ReadRequest {
            sequence: 14,
            ..first.clone()
        };
        assert!(accept_content(
            &mut state,
            &after_text,
            AcceptedContent::Image([1; 32])
        ));
        // The native decoder routes a rejected image through the same typed completion.
        let rejected = ReadRequest {
            sequence: 15,
            ..first.clone()
        };
        assert!(!accept_content(
            &mut state,
            &rejected,
            AcceptedContent::Other
        ));
        let after_rejection = ReadRequest {
            sequence: 16,
            ..first
        };
        assert!(accept_content(
            &mut state,
            &after_rejection,
            AcceptedContent::Image([1; 32])
        ));
    }
    #[test]
    fn stale_observations_and_late_completions_never_change_sequence_or_hash() {
        let mut state = ReaderState::default();
        let first = request(11);
        assert!(accept_content(
            &mut state,
            &first,
            AcceptedContent::Image([1; 32])
        ));
        let stale = ReadRequest {
            sequence: 12,
            ..first.clone()
        };
        assert_eq!(
            state.decide(&stale, 12, 12, 2, false),
            ReadDecision::Discard
        );
        assert!(!state.accept(&stale, 2));
        assert!(!state.accept_content(&first, 2, AcceptedContent::Image([2; 32])));
        assert!(!state.accept_content(&first, 2, AcceptedContent::Other));
        assert_eq!(state.accepted_sequence, Some(11));
        assert_eq!(state.image_hash, Some([1; 32]));
        assert_eq!(
            state.decide(&stale, 12, 13, 1, false),
            ReadDecision::Reread(13)
        );
        assert_eq!(state.accepted_sequence, Some(11));
        assert_eq!(state.image_hash, Some([1; 32]));
        let current_snapshot = Arc::new(CaptureSnapshot::updated(
            Some(&first.snapshot),
            first.snapshot.config.clone(),
            12,
            true,
        ));
        let current = ReadRequest {
            sequence: 13,
            snapshot: current_snapshot,
            ..first.clone()
        };
        assert!(accept_content(
            &mut state,
            &current,
            AcceptedContent::Image([2; 32])
        ));
        assert!(!state.accept_content(&first, current.snapshot.generation, AcceptedContent::Other));
        assert_eq!(state.accepted_sequence, Some(13));
        assert_eq!(state.image_hash, Some([2; 32]));
    }
    #[test]
    fn enable_epoch_clears_hash_even_when_off_notifications_were_coalesced() {
        let mut state = ReaderState::default();
        let first = request(11);
        assert!(accept_content(
            &mut state,
            &first,
            AcceptedContent::Image([1; 32])
        ));
        let off = CaptureSnapshot::updated(
            Some(&first.snapshot),
            CaptureConfig {
                images: false,
                ..first.snapshot.config.clone()
            },
            12,
            false,
        );
        let on = Arc::new(CaptureSnapshot::updated(
            Some(&off),
            first.snapshot.config.clone(),
            13,
            false,
        ));
        let next = ReadRequest {
            sequence: 14,
            snapshot: on,
            ..first
        };
        assert!(accept_content(
            &mut state,
            &next,
            AcceptedContent::Image([1; 32])
        ));
    }
    #[test]
    fn destination_and_theme_changes_preserve_the_image_enable_epoch() {
        let mut state = ReaderState::default();
        let first = request(11);
        assert!(accept_content(
            &mut state,
            &first,
            AcceptedContent::Image([1; 32])
        ));
        let changed = Arc::new(CaptureSnapshot::updated(
            Some(&first.snapshot),
            CaptureConfig {
                destination: Some(PathBuf::from("other")),
                ..first.snapshot.config.clone()
            },
            12,
            true,
        ));
        let next = ReadRequest {
            sequence: 13,
            snapshot: changed,
            ..first
        };
        assert!(!accept_content(
            &mut state,
            &next,
            AcceptedContent::Image([1; 32])
        ));
    }
    #[test]
    fn queued_saved_clicks_are_filtered_by_generation_at_app_delivery() {
        let mut service = ClipboardCaptureService::default();
        let (tx, rx) = mpsc::channel();
        service.event_rx = Some(rx);
        service.generation.store(2, Ordering::Release);
        tx.send(CaptureEvent::RevealSaved {
            generation: 1,
            path: PathBuf::from("old"),
        })
        .unwrap();
        tx.send(CaptureEvent::RevealSaved {
            generation: 2,
            path: PathBuf::from("current"),
        })
        .unwrap();
        assert_eq!(service.poll(), vec![PathBuf::from("current")]);
        assert!(service.poll().is_empty());
    }
}
