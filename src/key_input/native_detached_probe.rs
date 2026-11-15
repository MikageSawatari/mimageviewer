//! Development-only facade over the real native input allocation and dispatch.
//! No reducer, subclass, hit test, or gesture rule is copied into the probe.
use super::*;
use eframe::native_window_input::NativeWindowInputLease as _;

pub type TraceSink = Arc<dyn Fn(String) + Send + Sync>;

fn trace_sink() -> &'static Mutex<Option<TraceSink>> {
    static SINK: OnceLock<Mutex<Option<TraceSink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

fn emit(line: String) {
    let sink = trace_sink()
        .lock()
        .expect("probe trace lock poisoned")
        .clone();
    if let Some(sink) = sink {
        sink(line);
    }
}

fn snapshot(hwnd: u64) -> String {
    let guard = state().lock().expect("probe input lock poisoned");
    match guard.installed_hwnds.registration(hwnd) {
        Some(entry) => format!(
            "registration={:?} binding={:?}",
            entry.logical_registration, entry.native_detached
        ),
        None => "registration=None binding=Unregistered".into(),
    }
}

fn hit_payload(message: u32, wparam: usize) -> Option<u32> {
    match message {
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCLBUTTONUP | WM_NCMOUSEMOVE => {
            Some(wparam as u32)
        }
        WM_NCPOINTERDOWN | WM_NCPOINTERUP | WM_NCPOINTERUPDATE => {
            Some((wparam >> 16) as u32 & 0xffff)
        }
        _ => None,
    }
}

/// Records the actual owner before and after the shared production function.
/// Created before its state guard, so Drop observes state after unlocking it.
pub(super) struct MessageTrace {
    hwnd: u64,
    message: u32,
    phase: &'static str,
    active: bool,
}

impl MessageTrace {
    pub(super) fn begin(
        hwnd: u64,
        message: u32,
        parameters: Option<(usize, isize)>,
        phase: &'static str,
    ) -> Self {
        let active = trace_sink()
            .lock()
            .expect("probe trace lock poisoned")
            .is_some();
        if active {
            let parameters = match parameters {
                Some((wparam, lparam)) => format!(
                    "wParam=0x{wparam:x} lParam=0x{:x} hit={:?}",
                    lparam as usize,
                    hit_payload(message, wparam)
                ),
                None => "parameters=not-captured".into(),
            };
            emit(format!(
                "boundary={phase}.before hwnd=0x{hwnd:x} message=0x{message:04x} {parameters} {}",
                snapshot(hwnd)
            ));
        }
        Self {
            hwnd,
            message,
            phase,
            active,
        }
    }
}

impl Drop for MessageTrace {
    fn drop(&mut self) {
        if self.active {
            emit(format!(
                "boundary={}.after hwnd=0x{:x} message=0x{:04x} {}",
                self.phase,
                self.hwnd,
                self.message,
                snapshot(self.hwnd)
            ));
        }
    }
}

pub(super) fn release_hit(hwnd: u64, message: u32, hit: Option<u32>) {
    if trace_sink()
        .lock()
        .expect("probe trace lock poisoned")
        .is_some()
    {
        emit(format!(
            "boundary=release_hit hwnd=0x{hwnd:x} message=0x{message:04x} hit={hit:?}"
        ));
    }
}

/// Only call for HC_ACTION + PM_REMOVE on the UI thread's WH_GETMESSAGE hook.
pub fn observe_removed_message(hwnd: u64, message: u32, wparam: usize, lparam: isize) {
    observe_native_detached_message(hwnd, message, wparam, lparam);
}

/// The standalone Win32 owner installs and retires the exact production lease.
/// The headless Context owns only logical identity, not a native root or App.
pub struct ProbeSession {
    context: egui::Context,
    registration: egui::ViewportRegistration,
    lease: NativeKeyInputLease,
}

impl ProbeSession {
    pub fn attach(hwnd: u64, sink: TraceSink) -> Result<Self, String> {
        {
            let mut current = trace_sink()
                .lock()
                .map_err(|_| "probe trace lock poisoned")?;
            if current.is_some() {
                return Err("a probe session is already installed".into());
            }
            *current = Some(sink);
        }
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let viewport = egui::ViewportId::from_hash_of("native-detached-probe");
        context.show_viewport_deferred(viewport, Default::default(), |_, _| {});
        let registration = context
            .viewport_registration(viewport)
            .expect("probe logical registration");
        let target = NativeDetachedInputTarget {
            registration,
            kind: NativeDetachedInputTargetKind::Parked,
            availability: NativeTargetAvailability::Enabled,
            sink: Arc::new(|event| {
                let (outcome, reason) = match event.kind {
                    NativeDetachedInputEventKind::NonClient(outcome)
                    | NativeDetachedInputEventKind::NativeMediaTouch(outcome) => {
                        let reason = match outcome {
                            NativeNonClientOutcome::Cancelled(reason) => format!("{reason:?}"),
                            _ => "None".into(),
                        };
                        (format!("{outcome:?}"), reason)
                    }
                    other => (format!("{other:?}"), "None".into()),
                };
                emit(format!(
                    "boundary=outcome sequence={} hwnd=0x{:x} registration={:?} outcome={outcome} reason={reason}",
                    event.sequence, event.source.hwnd, event.source.registration
                ));
            }),
        };
        // This facade is exclusive to the standalone process. Never replace
        // another viewer's target list or adopt its native allocation.
        if !state()
            .lock()
            .map_err(|_| "probe input lock poisoned")?
            .native_detached_targets
            .is_empty()
        {
            *trace_sink()
                .lock()
                .map_err(|_| "probe trace lock poisoned")? = None;
            return Err("probe requires an empty native target registry".into());
        }
        set_native_detached_input_targets(vec![target]);
        match install_window_input(hwnd, viewport, Some(registration)) {
            Ok(Some(identity)) => Ok(Self {
                context,
                registration,
                lease: NativeKeyInputLease::Installed(identity),
            }),
            result => {
                retire_native_detached_input_target(registration);
                *trace_sink()
                    .lock()
                    .map_err(|_| "probe trace lock poisoned")? = None;
                Err(match result {
                    Err(error) => error,
                    Ok(None) => "probe HWND already has an input owner".into(),
                    Ok(Some(_)) => unreachable!(),
                })
            }
        }
    }
}

impl Drop for ProbeSession {
    fn drop(&mut self) {
        self.lease.retire();
        retire_native_detached_input_target(self.registration);
        let _ = self
            .context
            .retire_viewport(self.registration.viewport_id());
        *trace_sink().lock().expect("probe trace lock poisoned") = None;
    }
}
