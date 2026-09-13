//! Process-wide observation of the Windows file clipboard for cut-item rendering.
//!
//! Viewer contexts only read the resulting immutable path set. OS notifications,
//! OLE reads, local clipboard publication, and paste-completion callbacks converge
//! on the reducer in this module.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

pub(crate) const CUT_CONTENT_OPACITY: f32 = 0.5;

const CUT_OWNER_MAGIC: [u8; 8] = *b"mIVcut01";
const CUT_OWNER_PAYLOAD_LEN: usize = 32;
const CLIPBOARD_RETRY_DELAYS_MS: [u64; 4] = [25, 50, 100, 250];

#[derive(Debug, Default)]
struct ClipboardRetrySchedule {
    failure_index: usize,
}

impl ClipboardRetrySchedule {
    fn reset(&mut self) {
        self.failure_index = 0;
    }

    fn next_delay_ms(&mut self) -> u64 {
        let delay =
            CLIPBOARD_RETRY_DELAYS_MS[self.failure_index.min(CLIPBOARD_RETRY_DELAYS_MS.len() - 1)];
        self.failure_index = (self.failure_index + 1).min(CLIPBOARD_RETRY_DELAYS_MS.len() - 1);
        delay
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClipboardObjectIdentity {
    process_nonce: [u8; 16],
    token: u64,
}

impl ClipboardObjectIdentity {
    fn encode(self) -> [u8; CUT_OWNER_PAYLOAD_LEN] {
        let mut bytes = [0_u8; CUT_OWNER_PAYLOAD_LEN];
        bytes[..8].copy_from_slice(&CUT_OWNER_MAGIC);
        bytes[8..24].copy_from_slice(&self.process_nonce);
        bytes[24..].copy_from_slice(&self.token.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < CUT_OWNER_PAYLOAD_LEN || bytes[..8] != CUT_OWNER_MAGIC {
            return None;
        }
        let mut process_nonce = [0_u8; 16];
        process_nonce.copy_from_slice(&bytes[8..24]);
        let mut token = [0_u8; 8];
        token.copy_from_slice(&bytes[24..]);
        Some(Self {
            process_nonce,
            token: u64::from_le_bytes(token),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardTransferSignal {
    PerformedMove,
    PerformedOptimizedMove,
    PasteSucceededMove,
    LogicalMove,
    Other,
}

impl ClipboardTransferSignal {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::PerformedOptimizedMove | Self::PasteSucceededMove | Self::LogicalMove
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferProgress {
    Offered,
    AwaitingPasteSucceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClipboardReadRequest {
    request_serial: u64,
    os_sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardReadObservation {
    NotCut,
    Cut {
        identity: Option<ClipboardObjectIdentity>,
        paths: Arc<HashSet<CutPathKey>>,
        awaiting_paste_succeeded: bool,
    },
    Terminal {
        identity: Option<ClipboardObjectIdentity>,
    },
}

#[derive(Debug)]
enum CutClipboardEvent {
    BackendStartup {
        generation: u64,
        component: ClipboardBackendComponent,
        result: Result<(), String>,
    },
    ClipboardChanged(ClipboardReadRequest),
    ReadCompleted {
        request: ClipboardReadRequest,
        observation: ClipboardReadObservation,
    },
    LocalCompletion {
        identity: ClipboardObjectIdentity,
        signal: ClipboardTransferSignal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardBackendComponent {
    Reader,
    Listener,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardStartupTransition {
    Pending,
    Running,
    Disabled(String),
}

#[derive(Debug)]
struct ClipboardStartupReadiness {
    generation: u64,
    reader_ready: bool,
    listener_ready: bool,
}

impl ClipboardStartupReadiness {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            reader_ready: false,
            listener_ready: false,
        }
    }

    fn observe(
        &mut self,
        generation: u64,
        component: ClipboardBackendComponent,
        result: Result<(), String>,
    ) -> ClipboardStartupTransition {
        if generation != self.generation {
            return ClipboardStartupTransition::Pending;
        }
        if let Err(error) = result {
            return ClipboardStartupTransition::Disabled(error);
        }
        match component {
            ClipboardBackendComponent::Reader => self.reader_ready = true,
            ClipboardBackendComponent::Listener => self.listener_ready = true,
        }
        if self.reader_ready && self.listener_ready {
            ClipboardStartupTransition::Running
        } else {
            ClipboardStartupTransition::Pending
        }
    }
}

#[derive(Clone)]
pub(crate) struct CutClipboardEventSink {
    tx: mpsc::Sender<CutClipboardEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl CutClipboardEventSink {
    fn publish(&self, event: CutClipboardEvent) {
        if self.tx.send(event).is_ok() {
            (self.repaint)();
        }
    }

    pub(crate) fn publish_local_completion(
        &self,
        identity: ClipboardObjectIdentity,
        signal: ClipboardTransferSignal,
    ) {
        self.publish(CutClipboardEvent::LocalCompletion { identity, signal });
    }
}

#[derive(Clone)]
pub(crate) struct LocalCutDataObject {
    identity: ClipboardObjectIdentity,
    sink: CutClipboardEventSink,
    #[cfg(test)]
    completion_observations: Option<Arc<std::sync::Mutex<Vec<ClipboardTransferSignal>>>>,
}

impl LocalCutDataObject {
    pub(crate) fn identity_payload(&self) -> [u8; CUT_OWNER_PAYLOAD_LEN] {
        self.identity.encode()
    }

    pub(crate) fn publish_completion(&self, signal: ClipboardTransferSignal) {
        #[cfg(test)]
        if let Some(observations) = self.completion_observations.as_ref() {
            observations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(signal);
        }
        self.sink.publish_local_completion(self.identity, signal);
    }

    #[cfg(test)]
    pub(crate) fn inert_for_test(process_nonce: [u8; 16], token: u64) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self {
            identity: ClipboardObjectIdentity {
                process_nonce,
                token,
            },
            sink: CutClipboardEventSink {
                tx,
                repaint: Arc::new(|| {}),
            },
            completion_observations: Some(Arc::new(std::sync::Mutex::new(Vec::new()))),
        }
    }

    #[cfg(test)]
    pub(crate) fn completion_observations_for_test(
        &self,
    ) -> Arc<std::sync::Mutex<Vec<ClipboardTransferSignal>>> {
        self.completion_observations
            .as_ref()
            .expect("test data object has an observation sink")
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalClipboardWriteIntent {
    Copy,
    Cut,
}

pub(crate) struct LocalClipboardWrite {
    identity: ClipboardObjectIdentity,
    intent: LocalClipboardWriteIntent,
    notification_floor: u64,
    paths: Arc<HashSet<CutPathKey>>,
    data_object: Option<LocalCutDataObject>,
}

impl LocalClipboardWrite {
    pub(crate) fn cut_data_object(&self) -> Option<LocalCutDataObject> {
        self.data_object.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CutPathKey(String);

impl CutPathKey {
    fn from_path(path: &Path) -> Self {
        Self(normalize_windows_path_for_clipboard(path))
    }
}

fn normalize_windows_path_for_clipboard(path: &Path) -> String {
    normalize_windows_path_text(&path.to_string_lossy())
}

fn normalize_windows_path_text(path: &str) -> String {
    let mut normalized = path.replace('/', "\\");
    let folded = normalized.to_lowercase();
    if folded.starts_with("\\\\?\\unc\\") {
        normalized = format!("\\\\{}", &normalized[8..]);
    } else if folded.starts_with("\\\\?\\") {
        normalized = normalized[4..].to_string();
    }
    normalized = normalized.to_lowercase();

    let root_len = windows_root_len(&normalized);
    while normalized.len() > root_len && normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized
}

fn windows_root_len(path: &str) -> usize {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'\\' {
        return 3;
    }
    if path.starts_with("\\\\") {
        let mut separators = path.match_indices('\\').map(|(index, _)| index);
        let _ = separators.next();
        let _ = separators.next();
        let _ = separators.next();
        if let Some(after_share) = separators.next() {
            return after_share + 1;
        }
        return path.len();
    }
    0
}

#[derive(Debug, Clone)]
struct CutSnapshot {
    identity: Option<ClipboardObjectIdentity>,
    os_sequence: Option<u32>,
    paths: Arc<HashSet<CutPathKey>>,
    transfer: TransferProgress,
}

#[derive(Debug, Clone)]
enum CutDisplayState {
    Empty,
    Pending {
        request: ClipboardReadRequest,
        retained_local: Option<CutSnapshot>,
    },
    Cut(CutSnapshot),
}

impl Default for CutDisplayState {
    fn default() -> Self {
        Self::Empty
    }
}

impl CutDisplayState {
    fn snapshot(&self) -> Option<&CutSnapshot> {
        match self {
            Self::Cut(snapshot) => Some(snapshot),
            Self::Pending {
                retained_local: Some(snapshot),
                ..
            } => Some(snapshot),
            Self::Empty | Self::Pending { .. } => None,
        }
    }

    fn snapshot_mut(&mut self) -> Option<&mut CutSnapshot> {
        match self {
            Self::Cut(snapshot) => Some(snapshot),
            Self::Pending {
                retained_local: Some(snapshot),
                ..
            } => Some(snapshot),
            Self::Empty | Self::Pending { .. } => None,
        }
    }
}

struct CutClipboardReducer {
    process_nonce: [u8; 16],
    next_local_token: u64,
    latest_request_serial: u64,
    committed_local_cut_tokens: HashSet<u64>,
    retired_local_cut_tokens: HashSet<u64>,
    display: CutDisplayState,
}

impl CutClipboardReducer {
    fn new(process_nonce: [u8; 16]) -> Self {
        Self {
            process_nonce,
            next_local_token: 1,
            latest_request_serial: 0,
            committed_local_cut_tokens: HashSet::new(),
            retired_local_cut_tokens: HashSet::new(),
            display: CutDisplayState::Empty,
        }
    }

    fn reserve_local_write(
        &mut self,
        intent: LocalClipboardWriteIntent,
        notification_floor: u64,
        paths: &[PathBuf],
        sink: Option<CutClipboardEventSink>,
    ) -> LocalClipboardWrite {
        let token = self.next_local_token;
        self.next_local_token = self.next_local_token.saturating_add(1).max(1);
        let identity = ClipboardObjectIdentity {
            process_nonce: self.process_nonce,
            token,
        };
        let data_object = matches!(intent, LocalClipboardWriteIntent::Cut)
            .then(|| {
                sink.map(|sink| LocalCutDataObject {
                    identity,
                    sink,
                    #[cfg(test)]
                    completion_observations: None,
                })
            })
            .flatten();
        LocalClipboardWrite {
            identity,
            intent,
            notification_floor,
            paths: Arc::new(
                paths
                    .iter()
                    .map(|path| CutPathKey::from_path(path))
                    .collect(),
            ),
            data_object,
        }
    }

    fn commit_local_write(&mut self, write: LocalClipboardWrite, os_sequence: u32) {
        self.latest_request_serial = self.latest_request_serial.max(write.notification_floor);
        self.display = match write.intent {
            LocalClipboardWriteIntent::Copy => CutDisplayState::Empty,
            LocalClipboardWriteIntent::Cut => {
                self.committed_local_cut_tokens.insert(write.identity.token);
                CutDisplayState::Cut(CutSnapshot {
                    identity: Some(write.identity),
                    os_sequence: (os_sequence != 0).then_some(os_sequence),
                    paths: write.paths,
                    transfer: TransferProgress::Offered,
                })
            }
        };
    }

    fn observe_clipboard_change(&mut self, request: ClipboardReadRequest) {
        if request.request_serial <= self.latest_request_serial {
            return;
        }
        self.latest_request_serial = request.request_serial;
        let retained_local = self.display.snapshot().and_then(|snapshot| {
            let same_local_sequence = request.os_sequence != 0
                && snapshot.identity.is_some()
                && snapshot.os_sequence == Some(request.os_sequence);
            same_local_sequence.then(|| snapshot.clone())
        });
        self.display = CutDisplayState::Pending {
            request,
            retained_local,
        };
    }

    fn apply_read(&mut self, request: ClipboardReadRequest, observation: ClipboardReadObservation) {
        if request.request_serial != self.latest_request_serial {
            return;
        }
        if !matches!(
            self.display,
            CutDisplayState::Pending { request: current, .. } if current == request
        ) {
            return;
        }
        match observation {
            ClipboardReadObservation::NotCut => self.display = CutDisplayState::Empty,
            ClipboardReadObservation::Terminal { identity } => {
                if let Some(identity) = identity.and_then(|id| self.known_local_identity(id)) {
                    self.retire_local_identity(identity);
                }
                self.display = CutDisplayState::Empty;
            }
            ClipboardReadObservation::Cut {
                identity,
                paths,
                awaiting_paste_succeeded,
            } => {
                let identity = identity.and_then(|identity| self.known_local_identity(identity));
                if identity.is_some_and(|identity| self.identity_is_retired(identity)) {
                    self.display = CutDisplayState::Empty;
                    return;
                }
                self.display = CutDisplayState::Cut(CutSnapshot {
                    identity,
                    os_sequence: (request.os_sequence != 0).then_some(request.os_sequence),
                    paths,
                    transfer: if awaiting_paste_succeeded {
                        TransferProgress::AwaitingPasteSucceeded
                    } else {
                        TransferProgress::Offered
                    },
                });
            }
        }
    }

    fn apply_local_completion(
        &mut self,
        identity: ClipboardObjectIdentity,
        signal: ClipboardTransferSignal,
    ) {
        if self.known_local_identity(identity).is_none() {
            return;
        }
        if signal.is_terminal() {
            self.retire_local_identity(identity);
            let clears_current = self
                .display
                .snapshot()
                .is_some_and(|snapshot| snapshot.identity == Some(identity));
            if clears_current {
                self.display = match &self.display {
                    CutDisplayState::Pending { request, .. } => CutDisplayState::Pending {
                        request: *request,
                        retained_local: None,
                    },
                    CutDisplayState::Cut(_) | CutDisplayState::Empty => CutDisplayState::Empty,
                };
            }
            return;
        }
        if signal == ClipboardTransferSignal::PerformedMove
            && let Some(snapshot) = self.display.snapshot_mut()
            && snapshot.identity == Some(identity)
        {
            snapshot.transfer = TransferProgress::AwaitingPasteSucceeded;
        }
    }

    fn retire_local_identity(&mut self, identity: ClipboardObjectIdentity) {
        if self.known_local_identity(identity).is_some() {
            self.committed_local_cut_tokens.remove(&identity.token);
            self.retired_local_cut_tokens.insert(identity.token);
        }
    }

    fn identity_is_retired(&self, identity: ClipboardObjectIdentity) -> bool {
        identity.process_nonce == self.process_nonce
            && self.retired_local_cut_tokens.contains(&identity.token)
    }

    fn known_local_identity(
        &self,
        identity: ClipboardObjectIdentity,
    ) -> Option<ClipboardObjectIdentity> {
        (identity.process_nonce == self.process_nonce
            && (self.committed_local_cut_tokens.contains(&identity.token)
                || self.retired_local_cut_tokens.contains(&identity.token)))
        .then_some(identity)
    }

    fn contains(&self, path: &Path) -> bool {
        self.display
            .snapshot()
            .is_some_and(|snapshot| snapshot.paths.contains(&CutPathKey::from_path(path)))
    }
}

enum CutClipboardBackend {
    /// Headless/default construction. Tests may inject typed events, but local OS writes are inert.
    Inert,
    Disabled,
    #[cfg(windows)]
    Starting {
        readiness: ClipboardStartupReadiness,
        deferred: Vec<CutClipboardEvent>,
        sink: CutClipboardEventSink,
        runtime: windows_impl::CutClipboardRuntime,
    },
    #[cfg(windows)]
    Running {
        sink: CutClipboardEventSink,
        runtime: windows_impl::CutClipboardRuntime,
    },
}

impl CutClipboardBackend {
    fn accepts_local_tracking(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::Running { .. })
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

pub(crate) struct CutClipboardObserver {
    reducer: CutClipboardReducer,
    event_rx: mpsc::Receiver<CutClipboardEvent>,
    event_tx: mpsc::Sender<CutClipboardEvent>,
    request_counter: Arc<std::sync::atomic::AtomicU64>,
    next_backend_generation: u64,
    backend: CutClipboardBackend,
}

impl Default for CutClipboardObserver {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let process_nonce = *uuid::Uuid::new_v4().as_bytes();
        Self {
            reducer: CutClipboardReducer::new(process_nonce),
            event_rx,
            event_tx,
            request_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            next_backend_generation: 1,
            backend: CutClipboardBackend::Inert,
        }
    }
}

impl CutClipboardObserver {
    pub(crate) fn install_production(
        &mut self,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            if matches!(
                self.backend,
                CutClipboardBackend::Starting { .. } | CutClipboardBackend::Running { .. }
            ) {
                return Ok(());
            }
            if matches!(self.backend, CutClipboardBackend::Disabled) {
                return Err("cut clipboard observation was disabled after startup failure".into());
            }
            let sink = CutClipboardEventSink {
                tx: self.event_tx.clone(),
                repaint: Arc::new(repaint),
            };
            let generation = self.next_backend_generation;
            self.next_backend_generation = self.next_backend_generation.saturating_add(1).max(1);
            match windows_impl::CutClipboardRuntime::start(
                generation,
                sink.clone(),
                self.request_counter.clone(),
            ) {
                Ok(runtime) => {
                    self.backend = CutClipboardBackend::Starting {
                        readiness: ClipboardStartupReadiness::new(generation),
                        deferred: Vec::new(),
                        sink,
                        runtime,
                    };
                    Ok(())
                }
                Err(error) => {
                    self.reducer.display = CutDisplayState::Empty;
                    self.backend = CutClipboardBackend::Disabled;
                    Err(error)
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = repaint;
            Ok(())
        }
    }

    pub(crate) fn poll(&mut self) {
        if matches!(self.backend, CutClipboardBackend::Disabled) {
            while self.event_rx.try_recv().is_ok() {}
            self.reducer.display = CutDisplayState::Empty;
            return;
        }
        while let Ok(event) = self.event_rx.try_recv() {
            if let CutClipboardEvent::BackendStartup {
                generation,
                component,
                result,
            } = event
            {
                self.apply_backend_startup(generation, component, result);
                continue;
            }
            #[cfg(windows)]
            if let CutClipboardBackend::Starting { deferred, .. } = &mut self.backend {
                deferred.push(event);
                continue;
            }
            if matches!(self.backend, CutClipboardBackend::Disabled) {
                continue;
            }
            self.apply_observation_event(event);
        }
    }

    fn apply_observation_event(&mut self, event: CutClipboardEvent) {
        match event {
            CutClipboardEvent::BackendStartup { .. } => {}
            CutClipboardEvent::ClipboardChanged(request) => {
                self.reducer.observe_clipboard_change(request);
            }
            CutClipboardEvent::ReadCompleted {
                request,
                observation,
            } => self.reducer.apply_read(request, observation),
            CutClipboardEvent::LocalCompletion { identity, signal } => {
                self.reducer.apply_local_completion(identity, signal);
            }
        }
    }

    fn apply_backend_startup(
        &mut self,
        generation: u64,
        component: ClipboardBackendComponent,
        result: Result<(), String>,
    ) {
        #[cfg(windows)]
        {
            let transition = match &mut self.backend {
                CutClipboardBackend::Starting { readiness, .. } => {
                    readiness.observe(generation, component, result)
                }
                CutClipboardBackend::Inert
                | CutClipboardBackend::Disabled
                | CutClipboardBackend::Running { .. } => return,
            };
            match transition {
                ClipboardStartupTransition::Pending => {}
                ClipboardStartupTransition::Running => {
                    let previous =
                        std::mem::replace(&mut self.backend, CutClipboardBackend::Disabled);
                    if let CutClipboardBackend::Starting {
                        deferred,
                        sink,
                        runtime,
                        ..
                    } = previous
                    {
                        self.backend = CutClipboardBackend::Running { sink, runtime };
                        for event in deferred {
                            self.apply_observation_event(event);
                        }
                    }
                }
                ClipboardStartupTransition::Disabled(error) => {
                    crate::logger::log(format!(
                        "cut_clipboard: observer startup failed asynchronously: {error}"
                    ));
                    let previous =
                        std::mem::replace(&mut self.backend, CutClipboardBackend::Disabled);
                    if let CutClipboardBackend::Starting { mut runtime, .. } = previous {
                        runtime.shutdown_without_join();
                    }
                    self.reducer.display = CutDisplayState::Empty;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (generation, component, result);
        }
    }

    pub(crate) fn begin_local_write(
        &mut self,
        intent: LocalClipboardWriteIntent,
        paths: &[PathBuf],
    ) -> LocalClipboardWrite {
        let floor = self
            .request_counter
            .load(std::sync::atomic::Ordering::Acquire);
        let sink = match &self.backend {
            #[cfg(windows)]
            CutClipboardBackend::Running { sink, .. } => Some(sink.clone()),
            #[cfg(windows)]
            CutClipboardBackend::Starting { .. } => None,
            CutClipboardBackend::Inert | CutClipboardBackend::Disabled => None,
        };
        self.reducer.reserve_local_write(intent, floor, paths, sink)
    }

    pub(crate) fn commit_local_write(&mut self, write: LocalClipboardWrite, os_sequence: u32) {
        if self.backend.accepts_local_tracking() {
            self.reducer.commit_local_write(write, os_sequence);
        }
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.reducer.contains(path)
    }

    pub(crate) fn shutdown(&mut self) {
        #[cfg(windows)]
        match std::mem::replace(&mut self.backend, CutClipboardBackend::Disabled) {
            CutClipboardBackend::Starting { mut runtime, .. } => runtime.shutdown_without_join(),
            CutClipboardBackend::Running { mut runtime, .. } => runtime.shutdown(),
            CutClipboardBackend::Inert | CutClipboardBackend::Disabled => {}
        }
        #[cfg(not(windows))]
        {
            self.backend = CutClipboardBackend::Disabled;
        }
        self.reducer.display = CutDisplayState::Empty;
    }
}

impl Drop for CutClipboardObserver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(windows)]
pub(crate) mod windows_impl {
    use super::*;
    use std::ffi::OsString;
    use std::mem::ManuallyDrop;
    use std::os::windows::ffi::OsStringExt;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use windows::Win32::Foundation::{
        DV_E_CLIPFORMAT, DV_E_DVASPECT, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, E_OUTOFMEMORY,
        HWND, LPARAM, LRESULT, S_FALSE, S_OK, WPARAM,
    };
    use windows::Win32::System::Com::{
        DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows::Win32::System::DataExchange::{
        AddClipboardFormatListener, GetClipboardSequenceNumber, RegisterClipboardFormatW,
        RemoveClipboardFormatListener,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::{
        CF_HDROP, OleGetClipboard, OleInitialize, OleUninitialize, ReleaseStgMedium,
    };
    use windows::Win32::UI::Shell::{
        CFSTR_LOGICALPERFORMEDDROPEFFECT, CFSTR_PASTESUCCEEDED, CFSTR_PERFORMEDDROPEFFECT,
        CFSTR_PREFERREDDROPEFFECT, DragQueryFileW, HDROP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WM_DESTROY, WNDCLASSEXW,
    };
    use windows::core::{HRESULT, PCWSTR, w};

    const LISTENER_CLASS: PCWSTR = w!("mIVCutClipboardListener");
    const LISTENER_TITLE: PCWSTR = w!("mIV Cut Clipboard Listener");
    const CUT_OWNER_FORMAT: PCWSTR = w!("mImageViewer Cut Owner v1");
    const WM_MIV_CLIPBOARD_SHUTDOWN: u32 = WM_APP + 0x31a;
    const DROP_EFFECT_NONE: u32 = 0;
    const DROP_EFFECT_MOVE: u32 = 2;

    struct OleStaGuard;

    impl OleStaGuard {
        fn initialize(owner: &str) -> Result<Self, String> {
            unsafe { OleInitialize(None) }
                .map(|_| Self)
                .map_err(|error| format!("{owner} OLE STA init failed: {error}"))
        }
    }

    impl Drop for OleStaGuard {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct ClipboardFormats {
        preferred: u16,
        performed: u16,
        logical_performed: u16,
        paste_succeeded: u16,
        owner: u16,
    }

    fn register_format(name: PCWSTR, label: &str) -> Result<u16, String> {
        let raw = unsafe { RegisterClipboardFormatW(name) };
        if raw == 0 {
            return Err(format!("RegisterClipboardFormatW({label}) failed"));
        }
        u16::try_from(raw).map_err(|_| format!("clipboard format ID out of range: {raw}"))
    }

    fn clipboard_formats() -> Result<ClipboardFormats, String> {
        static FORMATS: OnceLock<Result<ClipboardFormats, String>> = OnceLock::new();
        FORMATS
            .get_or_init(|| {
                Ok(ClipboardFormats {
                    preferred: register_format(CFSTR_PREFERREDDROPEFFECT, "PreferredDropEffect")?,
                    performed: register_format(CFSTR_PERFORMEDDROPEFFECT, "PerformedDropEffect")?,
                    logical_performed: register_format(
                        CFSTR_LOGICALPERFORMEDDROPEFFECT,
                        "LogicalPerformedDropEffect",
                    )?,
                    paste_succeeded: register_format(CFSTR_PASTESUCCEEDED, "PasteSucceeded")?,
                    owner: register_format(CUT_OWNER_FORMAT, "mImageViewer Cut Owner")?,
                })
            })
            .clone()
    }

    pub(crate) fn cut_owner_format_id() -> Result<u16, String> {
        clipboard_formats().map(|formats| formats.owner)
    }

    pub(crate) fn completion_format_signal(cf_format: u16, effect: u32) -> ClipboardTransferSignal {
        let Ok(formats) = clipboard_formats() else {
            return ClipboardTransferSignal::Other;
        };
        if cf_format == formats.performed {
            return match effect {
                DROP_EFFECT_NONE => ClipboardTransferSignal::PerformedOptimizedMove,
                DROP_EFFECT_MOVE => ClipboardTransferSignal::PerformedMove,
                _ => ClipboardTransferSignal::Other,
            };
        }
        if cf_format == formats.paste_succeeded && effect == DROP_EFFECT_MOVE {
            return ClipboardTransferSignal::PasteSucceededMove;
        }
        if cf_format == formats.logical_performed && effect == DROP_EFFECT_MOVE {
            return ClipboardTransferSignal::LogicalMove;
        }
        ClipboardTransferSignal::Other
    }

    pub(crate) fn bytes_medium(bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
            .map_err(|_| windows::core::Error::from_hresult(E_OUTOFMEMORY))?;
        let mut medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: ManuallyDrop::new(None),
        };
        let locked = unsafe { GlobalLock(hglobal) };
        if locked.is_null() {
            unsafe { ReleaseStgMedium(&mut medium) };
            return Err(windows::core::Error::from_hresult(E_OUTOFMEMORY));
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), locked.cast::<u8>(), bytes.len()) };
        let _ = unsafe { GlobalUnlock(hglobal) };
        Ok(medium)
    }

    pub(crate) fn read_effect_medium(medium: *const STGMEDIUM) -> Option<u32> {
        let medium = unsafe { medium.as_ref() }?;
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            return None;
        }
        let hglobal = unsafe { medium.u.hGlobal };
        if unsafe { GlobalSize(hglobal) } < std::mem::size_of::<u32>() {
            return None;
        }
        let locked = unsafe { GlobalLock(hglobal) };
        if locked.is_null() {
            return None;
        }
        let value = unsafe { locked.cast::<u32>().read_unaligned() };
        let _ = unsafe { GlobalUnlock(hglobal) };
        Some(value)
    }

    struct LatestRequestMailbox {
        latest: Mutex<Option<ClipboardReadRequest>>,
        wake_tx: mpsc::SyncSender<()>,
    }

    impl LatestRequestMailbox {
        #[cfg(test)]
        fn replace_and_wake(&self, request: ClipboardReadRequest) {
            *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(request);
            let _ = self.wake_tx.try_send(());
        }

        fn take_latest(&self) -> Option<ClipboardReadRequest> {
            self.latest.lock().unwrap_or_else(|p| p.into_inner()).take()
        }
    }

    struct ListenerState {
        generation: u64,
        ready_components: AtomicU8,
        initial_request_published: AtomicBool,
        counter: Arc<AtomicU64>,
        mailbox: Arc<LatestRequestMailbox>,
        sink: CutClipboardEventSink,
    }

    static LISTENER_STATE: Mutex<Option<Arc<ListenerState>>> = Mutex::new(None);

    fn publish_startup_failure(
        state: &ListenerState,
        component: ClipboardBackendComponent,
        error: String,
    ) {
        state.sink.publish(CutClipboardEvent::BackendStartup {
            generation: state.generation,
            component,
            result: Err(error),
        });
    }

    fn publish_startup_ready(state: &ListenerState, component: ClipboardBackendComponent) {
        // Publish readiness before the initial request. The UI keeps any clipboard events that
        // race in Starting and replays them only after both component-ready events are observed.
        state.sink.publish(CutClipboardEvent::BackendStartup {
            generation: state.generation,
            component,
            result: Ok(()),
        });
        if startup_barrier_requests_initial_read(
            &state.ready_components,
            &state.initial_request_published,
            component,
        ) {
            publish_read_request(state, unsafe { GetClipboardSequenceNumber() });
        }
    }

    fn startup_barrier_requests_initial_read(
        ready_components: &AtomicU8,
        initial_request_published: &AtomicBool,
        component: ClipboardBackendComponent,
    ) -> bool {
        let bit = match component {
            ClipboardBackendComponent::Reader => 0b01,
            ClipboardBackendComponent::Listener => 0b10,
        };
        let ready = ready_components.fetch_or(bit, Ordering::AcqRel) | bit;
        if ready == 0b11
            && initial_request_published
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            true
        } else {
            false
        }
    }

    fn publish_read_request(state: &ListenerState, os_sequence: u32) {
        let request = ClipboardReadRequest {
            request_serial: state.counter.fetch_add(1, Ordering::AcqRel) + 1,
            os_sequence,
        };
        *state
            .mailbox
            .latest
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(request);
        state
            .sink
            .publish(CutClipboardEvent::ClipboardChanged(request));
        let _ = state.mailbox.wake_tx.try_send(());
    }

    unsafe extern "system" fn listener_wnd_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CLIPBOARDUPDATE => {
                let state = LISTENER_STATE
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(state) = state {
                    publish_read_request(&state, unsafe { GetClipboardSequenceNumber() });
                }
                LRESULT(0)
            }
            WM_MIV_CLIPBOARD_SHUTDOWN => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn register_listener_class() -> Result<(), String> {
        static REGISTER: OnceLock<Result<(), String>> = OnceLock::new();
        REGISTER
            .get_or_init(|| unsafe {
                let module = GetModuleHandleW(None)
                    .map_err(|error| format!("GetModuleHandleW clipboard listener: {error}"))?;
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(listener_wnd_proc),
                    hInstance: module.into(),
                    lpszClassName: LISTENER_CLASS,
                    ..Default::default()
                };
                if RegisterClassExW(&class) == 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(1410) {
                        return Err(format!("RegisterClassExW clipboard listener: {error}"));
                    }
                }
                Ok(())
            })
            .clone()
    }

    fn run_listener(state: Arc<ListenerState>, stop: Arc<AtomicBool>, hwnd_slot: Arc<AtomicIsize>) {
        let _ole = match OleStaGuard::initialize("cut clipboard listener") {
            Ok(guard) => guard,
            Err(error) => {
                publish_startup_failure(&state, ClipboardBackendComponent::Listener, error.clone());
                crate::logger::log(format!("cut_clipboard: listener unavailable: {error}"));
                return;
            }
        };
        if let Err(error) = register_listener_class() {
            publish_startup_failure(&state, ClipboardBackendComponent::Listener, error.clone());
            crate::logger::log(format!("cut_clipboard: listener unavailable: {error}"));
            return;
        }
        {
            let mut slot = LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_some() {
                let error = "duplicate cut clipboard listener refused".to_string();
                publish_startup_failure(&state, ClipboardBackendComponent::Listener, error.clone());
                crate::logger::log(format!("cut_clipboard: {error}"));
                return;
            }
            *slot = Some(state.clone());
        }
        let window = unsafe {
            let module = match GetModuleHandleW(None) {
                Ok(module) => module,
                Err(error) => {
                    let error = format!("clipboard listener module unavailable: {error}");
                    publish_startup_failure(
                        &state,
                        ClipboardBackendComponent::Listener,
                        error.clone(),
                    );
                    crate::logger::log(format!("cut_clipboard: {error}"));
                    *LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    return;
                }
            };
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                LISTENER_CLASS,
                LISTENER_TITLE,
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
        };
        let hwnd = match window {
            Ok(hwnd) => hwnd,
            Err(error) => {
                let error = format!("clipboard listener window failed: {error}");
                publish_startup_failure(&state, ClipboardBackendComponent::Listener, error.clone());
                crate::logger::log(format!("cut_clipboard: {error}"));
                *LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
                return;
            }
        };
        hwnd_slot.store(hwnd.0 as isize, Ordering::Release);
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { DestroyWindow(hwnd) };
            hwnd_slot.store(0, Ordering::Release);
            *LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
            return;
        }
        if let Err(error) = unsafe { AddClipboardFormatListener(hwnd) } {
            let error = format!("clipboard listener registration failed: {error}");
            publish_startup_failure(&state, ClipboardBackendComponent::Listener, error.clone());
            crate::logger::log(format!("cut_clipboard: {error}"));
            let _ = unsafe { DestroyWindow(hwnd) };
            hwnd_slot.store(0, Ordering::Release);
            *LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
            return;
        }
        publish_startup_ready(&state, ClipboardBackendComponent::Listener);
        unsafe {
            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 == -1 || !result.as_bool() {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = RemoveClipboardFormatListener(hwnd);
        }
        hwnd_slot.store(0, Ordering::Release);
        *LISTENER_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    struct MediumGuard(STGMEDIUM);

    impl Drop for MediumGuard {
        fn drop(&mut self) {
            unsafe { ReleaseStgMedium(&mut self.0) };
        }
    }

    fn format_request(cf_format: u16) -> FORMATETC {
        FORMATETC {
            cfFormat: cf_format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum QueryFormatAvailability {
        Available,
        Absent,
    }

    fn classify_query_get_data(
        result: HRESULT,
        cf_format: u16,
    ) -> Result<QueryFormatAvailability, String> {
        if result == S_OK {
            return Ok(QueryFormatAvailability::Available);
        }
        if result == S_FALSE
            || matches!(
                result,
                DV_E_CLIPFORMAT | DV_E_FORMATETC | DV_E_TYMED | DV_E_DVASPECT | DV_E_LINDEX
            )
        {
            return Ok(QueryFormatAvailability::Absent);
        }
        Err(format!(
            "IDataObject::QueryGetData({cf_format}) failed: {result:?}"
        ))
    }

    fn read_owner_identity(
        data: &IDataObject,
        cf_format: u16,
    ) -> Result<Option<ClipboardObjectIdentity>, String> {
        let format = format_request(cf_format);
        if classify_query_get_data(unsafe { data.QueryGetData(&format) }, cf_format)?
            == QueryFormatAvailability::Absent
        {
            return Ok(None);
        }
        let medium = unsafe { data.GetData(&format) }
            .map_err(|error| format!("IDataObject::GetData({cf_format}) failed: {error}"))?;
        let guard = MediumGuard(medium);
        if guard.0.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err(format!("clipboard format {cf_format} returned non-HGLOBAL"));
        }
        let hglobal = unsafe { guard.0.u.hGlobal };
        let size = unsafe { GlobalSize(hglobal) };
        if size < CUT_OWNER_PAYLOAD_LEN {
            return Err(format!("clipboard owner format {cf_format} is truncated"));
        }
        let locked = unsafe { GlobalLock(hglobal) };
        if locked.is_null() {
            return Err(format!("GlobalLock clipboard format {cf_format} failed"));
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(locked.cast::<u8>(), CUT_OWNER_PAYLOAD_LEN) };
        let identity = ClipboardObjectIdentity::decode(bytes);
        let _ = unsafe { GlobalUnlock(hglobal) };
        Ok(identity)
    }

    fn read_effect(data: &IDataObject, cf_format: u16) -> Result<Option<u32>, String> {
        let format = format_request(cf_format);
        if classify_query_get_data(unsafe { data.QueryGetData(&format) }, cf_format)?
            == QueryFormatAvailability::Absent
        {
            return Ok(None);
        }
        let medium = unsafe { data.GetData(&format) }
            .map_err(|error| format!("IDataObject::GetData({cf_format}) failed: {error}"))?;
        let guard = MediumGuard(medium);
        if guard.0.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err(format!(
                "clipboard effect format {cf_format} returned non-HGLOBAL"
            ));
        }
        let hglobal = unsafe { guard.0.u.hGlobal };
        if unsafe { GlobalSize(hglobal) } < std::mem::size_of::<u32>() {
            return Err(format!("clipboard effect format {cf_format} is truncated"));
        }
        let locked = unsafe { GlobalLock(hglobal) };
        if locked.is_null() {
            return Err(format!("GlobalLock clipboard effect {cf_format} failed"));
        }
        let value = unsafe { locked.cast::<u32>().read_unaligned() };
        let _ = unsafe { GlobalUnlock(hglobal) };
        Ok(Some(value))
    }

    fn read_hdrop_paths(data: &IDataObject) -> Result<Option<Vec<PathBuf>>, String> {
        let format = format_request(CF_HDROP.0);
        if classify_query_get_data(unsafe { data.QueryGetData(&format) }, CF_HDROP.0)?
            == QueryFormatAvailability::Absent
        {
            return Ok(None);
        }
        let medium = unsafe { data.GetData(&format) }
            .map_err(|error| format!("IDataObject::GetData(CF_HDROP) failed: {error}"))?;
        let guard = MediumGuard(medium);
        if guard.0.tymed != TYMED_HGLOBAL.0 as u32 {
            return Err("CF_HDROP returned non-HGLOBAL".to_string());
        }
        let hglobal = unsafe { guard.0.u.hGlobal };
        let drop = HDROP(hglobal.0);
        let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
        let mut paths = Vec::new();
        for index in 0..count {
            let len = unsafe { DragQueryFileW(drop, index, None) };
            if len == 0 {
                continue;
            }
            let mut wide = vec![0_u16; len as usize + 1];
            let copied = unsafe { DragQueryFileW(drop, index, Some(&mut wide)) } as usize;
            if copied == 0 || copied > len as usize {
                continue;
            }
            paths.push(PathBuf::from(OsString::from_wide(&wide[..copied])));
        }
        Ok(Some(paths))
    }

    enum StableRead {
        Observation(ClipboardReadObservation),
        SequenceChanged(u32),
    }

    fn read_data_object(
        data: &IDataObject,
        formats: ClipboardFormats,
    ) -> Result<ClipboardReadObservation, String> {
        let identity = read_owner_identity(&data, formats.owner)?;
        let logical = read_effect(&data, formats.logical_performed)?;
        let paste = read_effect(&data, formats.paste_succeeded)?;
        let performed = read_effect(&data, formats.performed)?;
        let preferred = read_effect(&data, formats.preferred)?;

        if logical == Some(DROP_EFFECT_MOVE)
            || paste == Some(DROP_EFFECT_MOVE)
            || performed == Some(DROP_EFFECT_NONE)
        {
            Ok(ClipboardReadObservation::Terminal { identity })
        } else if preferred == Some(DROP_EFFECT_MOVE) {
            match read_hdrop_paths(&data)? {
                Some(paths) if !paths.is_empty() => Ok(ClipboardReadObservation::Cut {
                    identity,
                    paths: Arc::new(
                        paths
                            .iter()
                            .map(|path| CutPathKey::from_path(path))
                            .collect(),
                    ),
                    awaiting_paste_succeeded: performed == Some(DROP_EFFECT_MOVE),
                }),
                _ => Ok(ClipboardReadObservation::NotCut),
            }
        } else {
            Ok(ClipboardReadObservation::NotCut)
        }
    }

    #[cfg(test)]
    pub(crate) fn external_cut_data_object_contains_for_test(
        data: &IDataObject,
        path: &Path,
    ) -> Result<bool, String> {
        match read_data_object(data, clipboard_formats()?)? {
            ClipboardReadObservation::Cut { paths, .. } => {
                Ok(paths.contains(&CutPathKey::from_path(path)))
            }
            ClipboardReadObservation::NotCut | ClipboardReadObservation::Terminal { .. } => {
                Ok(false)
            }
        }
    }

    fn read_clipboard(request: ClipboardReadRequest) -> Result<StableRead, String> {
        let before = unsafe { GetClipboardSequenceNumber() };
        let data = unsafe { OleGetClipboard() }
            .map_err(|error| format!("OleGetClipboard failed: {error}"))?;
        let observation = read_data_object(&data, clipboard_formats()?)?;
        let after = unsafe { GetClipboardSequenceNumber() };
        if before != after || (request.os_sequence != 0 && request.os_sequence != after) {
            Ok(StableRead::SequenceChanged(after))
        } else {
            Ok(StableRead::Observation(observation))
        }
    }

    fn run_reader(state: Arc<ListenerState>, wake_rx: mpsc::Receiver<()>, stop: Arc<AtomicBool>) {
        let _ole = match OleStaGuard::initialize("cut clipboard reader") {
            Ok(guard) => guard,
            Err(error) => {
                publish_startup_failure(&state, ClipboardBackendComponent::Reader, error.clone());
                crate::logger::log(format!("cut_clipboard: reader unavailable: {error}"));
                return;
            }
        };
        if let Err(error) = clipboard_formats() {
            publish_startup_failure(&state, ClipboardBackendComponent::Reader, error.clone());
            crate::logger::log(format!("cut_clipboard: reader unavailable: {error}"));
            return;
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        publish_startup_ready(&state, ClipboardBackendComponent::Reader);
        let mut active = None;
        let mut retry = ClipboardRetrySchedule::default();
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            if active.is_none() {
                if wake_rx.recv().is_err() {
                    break;
                }
                active = state.mailbox.take_latest();
                retry.reset();
            } else if let Some(latest) = state.mailbox.take_latest() {
                active = Some(latest);
                retry.reset();
            }
            let Some(request) = active else {
                continue;
            };
            match read_clipboard(request) {
                Ok(StableRead::Observation(observation)) => {
                    state.sink.publish(CutClipboardEvent::ReadCompleted {
                        request,
                        observation,
                    });
                    active = None;
                    retry.reset();
                }
                Ok(StableRead::SequenceChanged(sequence)) => {
                    publish_read_request(&state, sequence);
                    active = None;
                    retry.reset();
                }
                Err(error) => {
                    if retry.failure_index == 0 {
                        crate::logger::log(format!(
                            "cut_clipboard: clipboard read pending retry: {error}"
                        ));
                    }
                    let delay = Duration::from_millis(retry.next_delay_ms());
                    match wake_rx.recv_timeout(delay) {
                        Ok(()) => {
                            if let Some(latest) = state.mailbox.take_latest() {
                                active = Some(latest);
                                retry.reset();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }
        }
    }

    pub(crate) struct CutClipboardRuntime {
        stop: Arc<AtomicBool>,
        listener_hwnd: Arc<AtomicIsize>,
        wake_tx: mpsc::SyncSender<()>,
        listener: Option<JoinHandle<()>>,
        reader: Option<JoinHandle<()>>,
    }

    impl CutClipboardRuntime {
        pub(crate) fn start(
            generation: u64,
            sink: CutClipboardEventSink,
            counter: Arc<AtomicU64>,
        ) -> Result<Self, String> {
            let stop = Arc::new(AtomicBool::new(false));
            let listener_hwnd = Arc::new(AtomicIsize::new(0));
            let (wake_tx, wake_rx) = mpsc::sync_channel(1);
            let mailbox = Arc::new(LatestRequestMailbox {
                latest: Mutex::new(None),
                wake_tx: wake_tx.clone(),
            });
            let state = Arc::new(ListenerState {
                generation,
                ready_components: AtomicU8::new(0),
                initial_request_published: AtomicBool::new(false),
                counter,
                mailbox,
                sink,
            });
            let reader = {
                let state = state.clone();
                let stop = stop.clone();
                std::thread::Builder::new()
                    .name("cut-clipboard-reader".to_string())
                    .spawn(move || run_reader(state, wake_rx, stop))
                    .map_err(|error| format!("spawn cut clipboard reader: {error}"))?
            };
            let listener = {
                let listener_stop = stop.clone();
                let hwnd_slot = listener_hwnd.clone();
                match std::thread::Builder::new()
                    .name("cut-clipboard-listener".to_string())
                    .spawn(move || run_listener(state, listener_stop, hwnd_slot))
                {
                    Ok(listener) => listener,
                    Err(error) => {
                        stop.store(true, Ordering::Release);
                        let _ = wake_tx.try_send(());
                        if reader.is_finished() {
                            let _ = reader.join();
                        } else {
                            drop(reader);
                        }
                        return Err(format!("spawn cut clipboard listener: {error}"));
                    }
                }
            };
            Ok(Self {
                stop,
                listener_hwnd,
                wake_tx,
                listener: Some(listener),
                reader: Some(reader),
            })
        }

        /// Stops a runtime that has not reached Running without ever waiting on UI/App teardown.
        pub(crate) fn shutdown_without_join(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = self.wake_tx.try_send(());
            let raw = self.listener_hwnd.load(Ordering::Acquire);
            if raw != 0 {
                let hwnd = HWND(raw as *mut core::ffi::c_void);
                let _ = unsafe {
                    PostMessageW(Some(hwnd), WM_MIV_CLIPBOARD_SHUTDOWN, WPARAM(0), LPARAM(0))
                };
            }
            for (owner, handle) in [
                ("listener", self.listener.take()),
                ("reader", self.reader.take()),
            ] {
                if let Some(handle) = handle {
                    if handle.is_finished() {
                        let _ = handle.join();
                    } else {
                        crate::logger::log(format!(
                            "cut_clipboard: {owner} startup still in progress; detaching on exit"
                        ));
                        drop(handle);
                    }
                }
            }
        }

        pub(crate) fn shutdown(&mut self) {
            if self.stop.swap(true, Ordering::AcqRel) {
                return;
            }
            let _ = self.wake_tx.try_send(());
            let raw = self.listener_hwnd.load(Ordering::Acquire);
            let listener_woken = if raw != 0 {
                let hwnd = HWND(raw as *mut core::ffi::c_void);
                unsafe { PostMessageW(Some(hwnd), WM_MIV_CLIPBOARD_SHUTDOWN, WPARAM(0), LPARAM(0)) }
                    .is_ok()
            } else {
                false
            };
            if let Some(listener) = self.listener.take() {
                if listener.is_finished() || listener_woken {
                    let _ = listener.join();
                } else {
                    crate::logger::log(
                        "cut_clipboard: listener shutdown wake failed; detaching on exit"
                            .to_string(),
                    );
                    drop(listener);
                }
            }
            if let Some(reader) = self.reader.take() {
                if reader.is_finished() {
                    let _ = reader.join();
                } else {
                    crate::logger::log(
                        "cut_clipboard: reader still inside external COM call; detaching on exit"
                            .to_string(),
                    );
                    drop(reader);
                }
            }
        }
    }

    impl Drop for CutClipboardRuntime {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    #[cfg(test)]
    mod windows_tests {
        use super::*;

        #[test]
        fn completion_formats_keep_move_pending_and_accept_optimized_or_confirmed_move() {
            let formats = clipboard_formats().expect("registered formats");
            assert_eq!(
                completion_format_signal(formats.performed, DROP_EFFECT_MOVE),
                ClipboardTransferSignal::PerformedMove
            );
            assert_eq!(
                completion_format_signal(formats.performed, DROP_EFFECT_NONE),
                ClipboardTransferSignal::PerformedOptimizedMove
            );
            assert_eq!(
                completion_format_signal(formats.paste_succeeded, DROP_EFFECT_MOVE),
                ClipboardTransferSignal::PasteSucceededMove
            );
            assert_eq!(
                completion_format_signal(formats.logical_performed, DROP_EFFECT_MOVE),
                ClipboardTransferSignal::LogicalMove
            );
            assert_eq!(
                completion_format_signal(formats.performed, 1),
                ClipboardTransferSignal::Other
            );
        }

        #[test]
        fn full_wake_channel_still_retains_latest_request() {
            let (wake_tx, wake_rx) = mpsc::sync_channel(1);
            let mailbox = LatestRequestMailbox {
                latest: Mutex::new(None),
                wake_tx,
            };
            let first = ClipboardReadRequest {
                request_serial: 1,
                os_sequence: 11,
            };
            let latest = ClipboardReadRequest {
                request_serial: 2,
                os_sequence: 12,
            };
            mailbox.replace_and_wake(first);
            mailbox.replace_and_wake(latest);
            wake_rx.recv().expect("one coalesced wake");
            assert_eq!(mailbox.take_latest(), Some(latest));
        }

        #[test]
        fn query_get_data_only_treats_known_format_mismatches_as_absent() {
            use windows::Win32::Foundation::{RPC_E_CALL_REJECTED, RPC_E_SERVERCALL_RETRYLATER};

            assert_eq!(
                classify_query_get_data(S_OK, 1),
                Ok(QueryFormatAvailability::Available)
            );
            assert_eq!(
                classify_query_get_data(S_FALSE, 1),
                Ok(QueryFormatAvailability::Absent),
                "Shell IDataObject implementations may return S_FALSE for an unsupported format"
            );
            for absent in [
                DV_E_CLIPFORMAT,
                DV_E_FORMATETC,
                DV_E_TYMED,
                DV_E_DVASPECT,
                DV_E_LINDEX,
            ] {
                assert_eq!(
                    classify_query_get_data(absent, 1),
                    Ok(QueryFormatAvailability::Absent)
                );
            }
            assert!(classify_query_get_data(RPC_E_CALL_REJECTED, 1).is_err());
            assert!(classify_query_get_data(RPC_E_SERVERCALL_RETRYLATER, 1).is_err());
        }

        #[test]
        fn startup_barrier_requests_one_initial_read_after_both_components_are_ready() {
            let ready = AtomicU8::new(0);
            let initial = AtomicBool::new(false);
            assert!(!startup_barrier_requests_initial_read(
                &ready,
                &initial,
                ClipboardBackendComponent::Listener,
            ));
            assert!(!startup_barrier_requests_initial_read(
                &ready,
                &initial,
                ClipboardBackendComponent::Listener,
            ));
            assert!(startup_barrier_requests_initial_read(
                &ready,
                &initial,
                ClipboardBackendComponent::Reader,
            ));
            assert!(!startup_barrier_requests_initial_read(
                &ready,
                &initial,
                ClipboardBackendComponent::Reader,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: [u8; 16] = [7; 16];

    fn request(serial: u64, sequence: u32) -> ClipboardReadRequest {
        ClipboardReadRequest {
            request_serial: serial,
            os_sequence: sequence,
        }
    }

    fn local_write(
        reducer: &mut CutClipboardReducer,
        intent: LocalClipboardWriteIntent,
        floor: u64,
        paths: &[&str],
    ) -> LocalClipboardWrite {
        reducer.reserve_local_write(
            intent,
            floor,
            &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
            None,
        )
    }

    fn cut_paths(paths: &[&str]) -> Arc<HashSet<CutPathKey>> {
        Arc::new(
            paths
                .iter()
                .map(|path| CutPathKey::from_path(Path::new(path)))
                .collect(),
        )
    }

    #[test]
    fn lexical_path_keys_fold_case_separators_and_verbatim_prefixes_without_io() {
        assert_eq!(
            normalize_windows_path_text(r"\\?\C:\Photo\A.JPG\\"),
            normalize_windows_path_text(r"c:/photo/a.jpg")
        );
        assert_eq!(
            normalize_windows_path_text(r"\\?\UNC\Server\Share\A.JPG"),
            normalize_windows_path_text(r"\\server\share\a.jpg")
        );
        assert_ne!(
            normalize_windows_path_text(r"C:\Photo\A.JPG"),
            normalize_windows_path_text(r"D:\Photo\A.JPG")
        );
        assert_ne!(
            normalize_windows_path_text(r"\\server\share\a.jpg"),
            normalize_windows_path_text(r"\\server\other\a.jpg")
        );
    }

    #[test]
    fn owner_identity_payload_roundtrips_and_rejects_unrelated_bytes() {
        let identity = ClipboardObjectIdentity {
            process_nonce: NONCE,
            token: 42,
        };
        assert_eq!(
            ClipboardObjectIdentity::decode(&identity.encode()),
            Some(identity)
        );
        let mut wrong = identity.encode();
        wrong[0] ^= 0xff;
        assert_eq!(ClipboardObjectIdentity::decode(&wrong), None);
    }

    #[test]
    fn local_cut_is_immediate_and_copy_or_failed_write_preserves_correct_state() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let cut = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\a.jpg", r"C:\folder"],
        );
        reducer.commit_local_write(cut, 10);
        assert!(reducer.contains(Path::new(r"c:/A.JPG")));
        assert!(reducer.contains(Path::new(r"C:\folder")));

        // Reserving and then dropping a failed write does not alter the current cut.
        let failed = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Copy,
            0,
            &[r"C:\b.jpg"],
        );
        drop(failed);
        assert!(reducer.contains(Path::new(r"C:\a.jpg")));

        let copy = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Copy,
            0,
            &[r"C:\b.jpg"],
        );
        reducer.commit_local_write(copy, 11);
        assert!(!reducer.contains(Path::new(r"C:\a.jpg")));
    }

    #[test]
    fn different_sequence_invalidates_immediately_while_same_local_sequence_may_retain() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let cut = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\a.jpg"],
        );
        reducer.commit_local_write(cut, 10);
        reducer.observe_clipboard_change(request(1, 10));
        assert!(reducer.contains(Path::new(r"C:\a.jpg")));
        reducer.observe_clipboard_change(request(2, 11));
        assert!(!reducer.contains(Path::new(r"C:\a.jpg")));
    }

    #[test]
    fn latest_read_wins_and_old_queued_notification_before_publish_is_rejected() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let cut = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            5,
            &[r"C:\new.jpg"],
        );
        reducer.commit_local_write(cut, 20);
        reducer.observe_clipboard_change(request(4, 19));
        assert!(reducer.contains(Path::new(r"C:\new.jpg")));
        reducer.observe_clipboard_change(request(6, 21));
        reducer.observe_clipboard_change(request(7, 22));
        reducer.apply_read(
            request(6, 21),
            ClipboardReadObservation::Cut {
                identity: None,
                paths: cut_paths(&[r"C:\stale.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(!reducer.contains(Path::new(r"C:\stale.jpg")));
        reducer.apply_read(
            request(7, 22),
            ClipboardReadObservation::Cut {
                identity: None,
                paths: cut_paths(&[r"C:\external.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(reducer.contains(Path::new(r"C:\external.jpg")));
    }

    #[test]
    fn performed_move_waits_but_terminal_signals_clear_matching_local_cut_only() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let first = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\first.jpg"],
        );
        let first_identity = first.identity;
        reducer.commit_local_write(first, 30);
        reducer.apply_local_completion(first_identity, ClipboardTransferSignal::PerformedMove);
        assert!(reducer.contains(Path::new(r"C:\first.jpg")));
        assert_eq!(
            reducer.display.snapshot().map(|s| s.transfer),
            Some(TransferProgress::AwaitingPasteSucceeded)
        );

        let second = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\second.jpg"],
        );
        let second_identity = second.identity;
        reducer.commit_local_write(second, 31);
        reducer.apply_local_completion(first_identity, ClipboardTransferSignal::PasteSucceededMove);
        assert!(reducer.contains(Path::new(r"C:\second.jpg")));
        reducer.apply_local_completion(
            second_identity,
            ClipboardTransferSignal::PerformedOptimizedMove,
        );
        assert!(!reducer.contains(Path::new(r"C:\second.jpg")));
    }

    #[test]
    fn terminal_tombstone_prevents_same_object_read_from_resurrecting_cut() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let cut = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\a.jpg"],
        );
        let identity = cut.identity;
        reducer.commit_local_write(cut, 40);
        reducer.apply_local_completion(identity, ClipboardTransferSignal::LogicalMove);
        reducer.observe_clipboard_change(request(1, 41));
        reducer.apply_read(
            request(1, 41),
            ClipboardReadObservation::Cut {
                identity: Some(identity),
                paths: cut_paths(&[r"C:\a.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(!reducer.contains(Path::new(r"C:\a.jpg")));
    }

    #[test]
    fn external_overwrite_after_local_publish_is_not_cleared_by_old_local_callback() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let local = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\local.jpg"],
        );
        let local_identity = local.identity;
        reducer.commit_local_write(local, 50);

        reducer.observe_clipboard_change(request(1, 51));
        assert!(!reducer.contains(Path::new(r"C:\local.jpg")));
        reducer.apply_read(
            request(1, 51),
            ClipboardReadObservation::Cut {
                identity: None,
                paths: cut_paths(&[r"C:\external.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(reducer.contains(Path::new(r"C:\external.jpg")));

        reducer.apply_local_completion(
            local_identity,
            ClipboardTransferSignal::PerformedOptimizedMove,
        );
        assert!(reducer.contains(Path::new(r"C:\external.jpg")));
        assert!(!reducer.contains(Path::new(r"C:\local.jpg")));
    }

    #[test]
    fn callback_then_same_object_notification_stays_terminal_but_new_token_is_allowed() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let first = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\first.jpg"],
        );
        let first_identity = first.identity;
        reducer.commit_local_write(first, 60);
        reducer.apply_local_completion(first_identity, ClipboardTransferSignal::LogicalMove);

        reducer.observe_clipboard_change(request(1, 61));
        reducer.apply_read(
            request(1, 61),
            ClipboardReadObservation::Cut {
                identity: Some(first_identity),
                paths: cut_paths(&[r"C:\first.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(!reducer.contains(Path::new(r"C:\first.jpg")));

        let second = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            1,
            &[r"C:\second.jpg"],
        );
        reducer.commit_local_write(second, 62);
        assert!(reducer.contains(Path::new(r"C:\second.jpg")));
    }

    #[test]
    fn terminal_tombstones_are_exact_and_do_not_suppress_older_history_objects() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let first = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\first.jpg"],
        );
        let first_identity = first.identity;
        reducer.commit_local_write(first, 70);

        let second = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\second.jpg"],
        );
        let second_identity = second.identity;
        reducer.commit_local_write(second, 71);
        reducer.apply_local_completion(second_identity, ClipboardTransferSignal::LogicalMove);

        reducer.observe_clipboard_change(request(1, 72));
        reducer.apply_read(
            request(1, 72),
            ClipboardReadObservation::Cut {
                identity: Some(first_identity),
                paths: cut_paths(&[r"C:\first.jpg"]),
                awaiting_paste_succeeded: false,
            },
        );
        assert!(reducer.contains(Path::new(r"C:\first.jpg")));
        assert!(!reducer.contains(Path::new(r"C:\second.jpg")));
    }

    #[test]
    fn unissued_future_identity_cannot_retire_a_later_legitimate_cut() {
        let mut reducer = CutClipboardReducer::new(NONCE);
        let first = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\first.jpg"],
        );
        reducer.commit_local_write(first, 80);
        let future = ClipboardObjectIdentity {
            process_nonce: NONCE,
            token: 2,
        };
        reducer.apply_local_completion(future, ClipboardTransferSignal::LogicalMove);

        let second = local_write(
            &mut reducer,
            LocalClipboardWriteIntent::Cut,
            0,
            &[r"C:\second.jpg"],
        );
        assert_eq!(second.identity, future);
        reducer.commit_local_write(second, 81);
        assert!(reducer.contains(Path::new(r"C:\second.jpg")));
    }

    #[test]
    fn retry_schedule_is_capped_and_resets_for_a_new_request() {
        let mut retry = ClipboardRetrySchedule::default();
        assert_eq!(
            (0..6).map(|_| retry.next_delay_ms()).collect::<Vec<_>>(),
            vec![25, 50, 100, 250, 250, 250]
        );
        retry.reset();
        assert_eq!(retry.next_delay_ms(), 25);
    }

    #[test]
    fn startup_readiness_requires_both_exact_generation_components_and_fails_typed() {
        let mut readiness = ClipboardStartupReadiness::new(7);
        assert_eq!(
            readiness.observe(
                6,
                ClipboardBackendComponent::Listener,
                Err("stale listener".to_string()),
            ),
            ClipboardStartupTransition::Pending,
            "a late startup result from an older backend generation is inert"
        );
        assert_eq!(
            readiness.observe(7, ClipboardBackendComponent::Reader, Ok(())),
            ClipboardStartupTransition::Pending
        );
        assert_eq!(
            readiness.observe(7, ClipboardBackendComponent::Listener, Ok(())),
            ClipboardStartupTransition::Running
        );

        let mut failed = ClipboardStartupReadiness::new(8);
        assert_eq!(
            failed.observe(
                8,
                ClipboardBackendComponent::Reader,
                Err("reader init failed".to_string()),
            ),
            ClipboardStartupTransition::Disabled("reader init failed".to_string())
        );
    }

    #[test]
    fn default_observer_is_inert_and_accepts_only_injected_typed_events() {
        let mut observer = CutClipboardObserver::default();
        let write = observer.begin_local_write(
            LocalClipboardWriteIntent::Cut,
            &[PathBuf::from(r"C:\local.jpg")],
        );
        assert!(
            write.cut_data_object().is_none(),
            "headless default has no OS wrapper"
        );
        observer.commit_local_write(write, 70);
        assert!(
            !observer.contains(Path::new(r"C:\local.jpg")),
            "headless/default construction never pretends an OS Cut was observed"
        );

        let external = request(1, 71);
        observer
            .event_tx
            .send(CutClipboardEvent::ClipboardChanged(external))
            .unwrap();
        observer
            .event_tx
            .send(CutClipboardEvent::ReadCompleted {
                request: external,
                observation: ClipboardReadObservation::Cut {
                    identity: None,
                    paths: cut_paths(&[r"C:\external.jpg"]),
                    awaiting_paste_succeeded: false,
                },
            })
            .unwrap();
        observer.poll();
        assert!(observer.contains(Path::new(r"C:\external.jpg")));
        assert!(!observer.contains(Path::new(r"C:\local.jpg")));
        observer.shutdown();
        observer.shutdown();
    }

    #[test]
    fn disabled_observer_ignores_late_events_and_local_commits() {
        let mut observer = CutClipboardObserver::default();
        observer.backend = CutClipboardBackend::Disabled;
        let external = request(1, 90);
        observer
            .event_tx
            .send(CutClipboardEvent::ClipboardChanged(external))
            .unwrap();
        observer
            .event_tx
            .send(CutClipboardEvent::ReadCompleted {
                request: external,
                observation: ClipboardReadObservation::Cut {
                    identity: None,
                    paths: cut_paths(&[r"C:\external.jpg"]),
                    awaiting_paste_succeeded: false,
                },
            })
            .unwrap();
        let write = observer.begin_local_write(
            LocalClipboardWriteIntent::Cut,
            &[PathBuf::from(r"C:\local.jpg")],
        );
        observer.commit_local_write(write, 91);
        observer.poll();
        assert!(!observer.contains(Path::new(r"C:\external.jpg")));
        assert!(!observer.contains(Path::new(r"C:\local.jpg")));
    }
}
