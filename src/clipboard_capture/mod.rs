//! App-owned clipboard capture. Notifications carry immutable settings; viewer contexts
//! never own or reset this service. The native reader only copies bytes while open.
pub(crate) mod data;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod popup;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

pub(crate) fn default_destination() -> &'static std::path::Path {
    static DESTINATION: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DESTINATION.get_or_init(|| crate::capture::default_output_dir().join("clipboard"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureConfig {
    pub images: bool,
    pub html: bool,
    pub destination: PathBuf,
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
    pub reread: bool,
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
            && (request.snapshot.allows_images(request.sequence)
                || request.snapshot.allows_html(request.sequence))
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

pub(crate) enum CaptureEvent {
    StartupFailed(String),
    RevealSaved { generation: u64, path: PathBuf },
}

pub(crate) struct ClipboardCaptureService {
    snapshot: Option<Arc<CaptureSnapshot>>,
    generation: Arc<AtomicU64>,
    startup_error: Option<String>,
    event_rx: Option<mpsc::Receiver<CaptureEvent>>,
    #[cfg(windows)]
    runtime: Option<native::CaptureRuntime>,
}

impl Default for ClipboardCaptureService {
    fn default() -> Self {
        Self {
            snapshot: None,
            generation: Arc::new(AtomicU64::new(0)),
            startup_error: None,
            event_rx: None,
            #[cfg(windows)]
            runtime: None,
        }
    }
}

impl ClipboardCaptureService {
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
        slot: crate::tray::PlacementSlot,
    ) {
        let config = CaptureConfig {
            images: settings.clipboard_capture_image_enabled,
            html: settings.clipboard_capture_html_enabled,
            destination: settings
                .clipboard_capture_output_dir
                .clone()
                .unwrap_or_else(|| default_destination().to_path_buf()),
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
        if let Some(runtime) = &self.runtime {
            runtime.update_snapshot(snapshot);
        } else if (snapshot.config.images || snapshot.config.html) && self.startup_error.is_none() {
            let (tx, rx) = mpsc::channel();
            let foreground_ctx = ctx.clone();
            let foreground = Arc::new(move || {
                crate::window_activation::activate_main_window(
                    hwnd,
                    &slot,
                    &foreground_ctx,
                    "clipboard capture",
                );
            });
            let repaint_ctx = ctx.clone();
            let repaint: Arc<dyn Fn() + Send + Sync> =
                Arc::new(move || repaint_ctx.request_repaint());
            match native::CaptureRuntime::start(
                snapshot,
                self.generation.clone(),
                foreground,
                tx,
                repaint,
            ) {
                Ok(runtime) => {
                    self.runtime = Some(runtime);
                    self.event_rx = Some(rx);
                }
                Err(error) => self.record_startup_error(error),
            }
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
                CaptureEvent::RevealSaved { generation, path } => {
                    if generation == self.generation.load(Ordering::Acquire) {
                        reveal.push(path);
                    }
                }
            }
        }
        reveal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(images: bool, html: bool, sequence: u32) -> Arc<CaptureSnapshot> {
        Arc::new(CaptureSnapshot::updated(
            None,
            CaptureConfig {
                images,
                html,
                destination: PathBuf::from("capture"),
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
            reread: false,
        }
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
        let retry = ReadRequest {
            reread: true,
            sequence,
            ..r.clone()
        };
        assert!(Arc::ptr_eq(&retry.snapshot, &r.snapshot));
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
                destination: PathBuf::from("capture"),
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
                destination: PathBuf::from("other"),
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
