//! External URL validation and fire-and-forget Shell opening helpers.
//!
//! Metadata can be user-provided, so only browser-safe HTTP(S) URLs are allowed
//! through this module. Shell calls run on short-lived STA workers; callers
//! never wait for Shell handlers and workers never borrow App state.

use std::path::PathBuf;
use std::sync::mpsc;

pub(crate) enum OpenTarget {
    Url(String),
    Player(PathBuf),
    Tweet(String),
    Map(String),
}

impl OpenTarget {
    fn kind(&self) -> &'static str {
        match self {
            Self::Url(_) => "url",
            Self::Player(_) => "player",
            Self::Tweet(_) => "tweet",
            Self::Map(_) => "map",
        }
    }

    fn normalize(self) -> Result<Self, String> {
        let normalize = |raw: String| {
            normalize_http_url(&raw).ok_or_else(|| "unsafe or unsupported URL".to_string())
        };
        Ok(match self {
            Self::Url(raw) => Self::Url(normalize(raw)?),
            Self::Tweet(raw) => Self::Tweet(normalize(raw)?),
            Self::Map(raw) => Self::Map(normalize(raw)?),
            target @ Self::Player(_) => target,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalOpenFailure {
    GoogleMaps,
}

impl ExternalOpenFailure {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::GoogleMaps => "Google マップを開けませんでした",
        }
    }
}

#[derive(Clone)]
pub(crate) struct FailureSink {
    pub(crate) tx: mpsc::Sender<ExternalOpenFailure>,
    pub(crate) ctx: egui::Context,
}

fn report_failure(kind: &str, error: &str, sink: Option<&FailureSink>) {
    // Opener errors may embed the target. GPS coordinates must never enter logs.
    if kind == "map" {
        crate::logger::log("external_open map failed");
    } else {
        crate::logger::log(format!("external_open {kind} failed: {error}"));
    }
    if let Some(sink) = sink {
        let _ = sink.tx.send(ExternalOpenFailure::GoogleMaps);
        sink.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

/// Validate on the caller, then submit once without waiting for Shell.
/// The Result only describes validation/spawn; Shell failures use the optional
/// process-wide notification sink. Order, cancellation and join are not promised.
pub(crate) fn spawn_open(
    target: OpenTarget,
    on_failure: Option<FailureSink>,
) -> Result<(), String> {
    let kind = target.kind();
    let target = target.normalize().inspect_err(|error| {
        report_failure(kind, error, on_failure.as_ref());
    })?;
    let opener = WorkerOpener::for_caller();
    #[cfg(test)]
    TEST_SPAWNS.with(|count| count.set(count.get() + 1));
    let worker_sink = on_failure.clone();
    let spawn_result = std::thread::Builder::new()
        .name(format!("external-open-{kind}"))
        .spawn(move || {
            let started = std::time::Instant::now();
            let result = opener.open(&target);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "ext",
                    "open_end",
                    None,
                    0,
                    &[
                        // `kind` is reserved for the event name by perf::event.
                        ("target_kind", kind.into()),
                        ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                        ("ok", result.is_ok().into()),
                    ],
                );
            }
            if let Err(error) = result {
                report_failure(kind, &error, worker_sink.as_ref());
            }
        });
    spawn_result.map(|_| ()).map_err(|error| {
        let error = format!("worker start failed: {error}");
        report_failure(kind, &error, on_failure.as_ref());
        error
    })
}

struct WorkerOpener {
    #[cfg(test)]
    opener: Option<TestOpener>,
}

impl WorkerOpener {
    fn for_caller() -> Self {
        Self {
            #[cfg(test)]
            opener: TEST_OPENER.with(|opener| opener.borrow().clone()),
        }
    }

    fn open(self, target: &OpenTarget) -> Result<(), String> {
        #[cfg(windows)]
        let _com = crate::open_with::initialize_com_sta()
            .map_err(|error| format!("COM initialization failed: {error}"))?;
        #[cfg(test)]
        if let Some(opener) = self.opener {
            return opener(target);
        }
        let result = match target {
            OpenTarget::Url(url) | OpenTarget::Tweet(url) | OpenTarget::Map(url) => {
                opener::open(url)
            }
            OpenTarget::Player(path) => opener::open(path),
        };
        result.map_err(|error| format!("open failed: {error}"))
    }
}

#[cfg(test)]
type TestOpener = std::sync::Arc<dyn Fn(&OpenTarget) -> Result<(), String> + Send + Sync>;

#[cfg(test)]
thread_local! {
    static TEST_OPENER: std::cell::RefCell<Option<TestOpener>> = const { std::cell::RefCell::new(None) };
    static TEST_SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The submitting test thread owns the override; each worker captures its copy.
/// Parallel tests never replace one another's opener.
#[cfg(test)]
pub(crate) struct TestOpenerGuard(Option<TestOpener>);

#[cfg(test)]
impl TestOpenerGuard {
    pub(crate) fn install(
        opener: impl Fn(&OpenTarget) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self(TEST_OPENER.with(|slot| slot.replace(Some(std::sync::Arc::new(opener)))))
    }
}

#[cfg(test)]
impl Drop for TestOpenerGuard {
    fn drop(&mut self) {
        TEST_OPENER.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

const MAX_EXTERNAL_URL_LEN: usize = 4096;

pub(crate) fn google_maps_url(gps: crate::exif_reader::GpsCoordinates) -> String {
    // Fixed precision is locale-independent. Normalize rounded negative zero too.
    let decimal = |value: f64| {
        let text = format!("{value:.7}");
        if text == "-0.0000000" {
            "0.0000000".to_string()
        } else {
            text
        }
    };
    format!(
        "https://www.google.com/maps/search/?api=1&query={}%2C{}",
        decimal(gps.latitude),
        decimal(gps.longitude),
    )
}

pub fn normalize_http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|c: char| c.is_whitespace() || c == '<' || c == '>');
    if trimmed.is_empty() || trimmed.len() > MAX_EXTERNAL_URL_LEN {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    let (scheme, rest) = trimmed.split_once(':')?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    if !rest.starts_with("//") || rest.len() <= 2 {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn is_http_url(raw: &str) -> bool {
    normalize_http_url(raw).is_some()
}

/// Submit a browser URL without waiting for Shell. Errors report validation or
/// worker startup only; a later Shell failure is logged on the worker.
pub fn open_url(raw: &str) -> Result<(), String> {
    spawn_open(OpenTarget::Url(raw.to_string()), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_open_url_returns_before_shell_and_opens_once_on_worker() {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;

        let caller = std::thread::current().id();
        let (called_tx, called_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let _opener = TestOpenerGuard::install(move |target| {
            worker_calls.fetch_add(1, Ordering::SeqCst);
            assert!(matches!(target, OpenTarget::Url(url) if url == "https://example.com/path"));
            called_tx.send(std::thread::current().id()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(())
        });

        let before = TEST_SPAWNS.with(|count| count.get());
        crate::ui_helpers::open_url(" <https://example.com/path> ");
        assert_eq!(TEST_SPAWNS.with(|count| count.get()), before + 1);
        // The worker is still inside the opener when the UI helper has returned.
        let worker = called_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(caller, worker);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release_tx.send(()).unwrap();
    }

    #[test]
    fn external_open_invalid_urls_never_spawn() {
        let _opener = TestOpenerGuard::install(|_| panic!("invalid URL reached opener"));
        let before = TEST_SPAWNS.with(|count| count.get());
        for raw in [
            "javascript:alert(1)",
            "file:///C:/test",
            "https://",
            "https://a/\ncalc",
        ] {
            assert!(open_url(raw).is_err());
            crate::ui_helpers::open_url(raw);
            assert!(spawn_open(OpenTarget::Tweet(raw.to_owned()), None).is_err());
            assert!(spawn_open(OpenTarget::Map(raw.to_owned()), None).is_err());
        }
        assert_eq!(TEST_SPAWNS.with(|count| count.get()), before);
    }

    #[test]
    fn external_open_player_and_tweet_use_the_same_worker_boundary() {
        let caller = std::thread::current().id();
        let (called_tx, called_rx) = mpsc::channel();
        let _opener = TestOpenerGuard::install(move |target| {
            assert_ne!(caller, std::thread::current().id());
            match target {
                OpenTarget::Player(path) => assert_eq!(path, &PathBuf::from("movie & still.mp4")),
                OpenTarget::Tweet(url) => assert_eq!(url, "https://x.com/example/status/1"),
                _ => panic!("unexpected target"),
            }
            called_tx.send(target.kind()).unwrap();
            Ok(())
        });
        crate::ui_helpers::open_external_player(std::path::Path::new("movie & still.mp4"));
        spawn_open(
            OpenTarget::Tweet("https://x.com/example/status/1".into()),
            None,
        )
        .unwrap();
        let mut kinds = [
            called_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            called_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
        ];
        kinds.sort_unstable();
        assert_eq!(kinds, ["player", "tweet"]);
    }

    #[test]
    fn google_maps_url_orders_and_formats_coordinates() {
        use crate::exif_reader::GpsCoordinates;
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: -35.123456789,
                longitude: -139.75
            }),
            "https://www.google.com/maps/search/?api=1&query=-35.1234568%2C-139.7500000"
        );
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: -0.000000001,
                longitude: -0.0
            }),
            "https://www.google.com/maps/search/?api=1&query=0.0000000%2C0.0000000"
        );
        assert_eq!(
            google_maps_url(GpsCoordinates {
                latitude: 90.0,
                longitude: 180.0
            }),
            "https://www.google.com/maps/search/?api=1&query=90.0000000%2C180.0000000"
        );
    }

    #[test]
    fn accepts_http_and_https_urls() {
        assert_eq!(
            normalize_http_url(" https://youtu.be/abc?x=1 ").as_deref(),
            Some("https://youtu.be/abc?x=1")
        );
        assert_eq!(
            normalize_http_url("HTTP://example.com/path").as_deref(),
            Some("HTTP://example.com/path")
        );
    }

    #[test]
    fn rejects_non_browser_or_control_urls() {
        assert!(normalize_http_url("javascript:alert(1)").is_none());
        assert!(normalize_http_url("file:///C:/Windows/win.ini").is_none());
        assert!(normalize_http_url("https://example.com/\ncalc").is_none());
        assert!(normalize_http_url("https://").is_none());
    }
}
