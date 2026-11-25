//! Request-scoped viewer opening policy, plus process-wide external-open notifications.
//! No App-global next-open flags.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

impl super::App {
    pub(crate) fn external_open_sink(
        &self,
        ctx: &egui::Context,
    ) -> crate::external_links::FailureSink {
        crate::external_links::FailureSink {
            tx: self.external_open_failure_tx.clone(),
            ctx: ctx.clone(),
        }
    }

    /// The photo/spread side is resolved by the drawing pass; no pending GPS
    /// state survives navigation. External-open notifications use the root poll.
    pub(crate) fn open_google_maps(&self, ctx: &egui::Context, url: String) {
        let _ = crate::external_links::spawn_open(
            crate::external_links::OpenTarget::Map(url),
            Some(crate::external_links::FailureSink {
                tx: self.external_open_failure_tx.clone(),
                ctx: ctx.clone(),
            }),
        );
    }

    pub(crate) fn poll_external_open_failures(&mut self) {
        while let Ok(failure) = self.external_open_failures.try_recv() {
            if failure.is_warning() {
                self.show_feedback_warning(failure.message().to_owned());
            } else {
                self.show_feedback_toast(failure.message().to_owned());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ViewerOpenIntent {
    #[default]
    Preferences,
    BrowserOverlay {
        placement: crate::detached_window_geometry::PhysicalWindowPlacement,
    },
}

impl ViewerOpenIntent {
    pub(crate) fn is_browser_overlay(self) -> bool {
        matches!(self, Self::BrowserOverlay { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewerOpenRequest {
    id: u64,
    pub(crate) intent: ViewerOpenIntent,
}

impl ViewerOpenRequest {
    pub(crate) fn new(intent: ViewerOpenIntent) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            intent,
        }
    }

    pub(crate) fn preferences() -> Self {
        Self::new(ViewerOpenIntent::Preferences)
    }

    pub(crate) fn id(self) -> u64 {
        self.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExplicitOpenPath {
    pub(crate) path: PathBuf,
    pub(crate) request: ViewerOpenRequest,
}

impl ExplicitOpenPath {
    pub(crate) fn new(path: PathBuf, request: ViewerOpenRequest) -> Self {
        Self {
            path: crate::path_key::normalize_input_path(&path),
            request,
        }
    }
}

impl std::ops::Deref for ExplicitOpenPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerOpenOrigin {
    MainNew,
    /// The old main reader has handed off; this request still owns new-main routing.
    MainPrepared,
    ContextContinuation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerOpenSelection {
    Restore,
    ExplicitFile,
}

/// The existing adoption value travels through list, sidecar and composition owners.
/// A request can change routing origin without losing its correlation or selection policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ViewerOpenAdoption {
    MainNew,
    #[default]
    ContextContinuation,
    Requested {
        request: ViewerOpenRequest,
        origin: ViewerOpenOrigin,
        selection: ViewerOpenSelection,
    },
}

impl ViewerOpenAdoption {
    pub(crate) fn requested(request: ViewerOpenRequest) -> Self {
        Self::Requested {
            request,
            origin: ViewerOpenOrigin::MainNew,
            selection: ViewerOpenSelection::Restore,
        }
    }

    pub(crate) fn is_main_new(self) -> bool {
        matches!(
            self,
            Self::MainNew
                | Self::Requested {
                    origin: ViewerOpenOrigin::MainNew | ViewerOpenOrigin::MainPrepared,
                    ..
                }
        )
    }

    pub(crate) fn needs_main_preparation(self) -> bool {
        matches!(
            self,
            Self::MainNew
                | Self::Requested {
                    origin: ViewerOpenOrigin::MainNew,
                    ..
                }
        )
    }

    /// Carry the completed handoff through hydration without a second old-reader close.
    pub(crate) fn after_main_preparation(self) -> Self {
        match self {
            Self::Requested {
                request,
                origin: ViewerOpenOrigin::MainNew,
                selection,
            } => Self::Requested {
                request,
                origin: ViewerOpenOrigin::MainPrepared,
                selection,
            },
            other => other,
        }
    }

    pub(crate) fn explicit_file(self) -> Self {
        let (request, origin) = match self {
            Self::Requested {
                request, origin, ..
            } => (request, origin),
            Self::MainNew => (ViewerOpenRequest::preferences(), ViewerOpenOrigin::MainNew),
            Self::ContextContinuation => (
                ViewerOpenRequest::preferences(),
                ViewerOpenOrigin::ContextContinuation,
            ),
        };
        Self::Requested {
            request,
            origin,
            selection: ViewerOpenSelection::ExplicitFile,
        }
    }

    pub(crate) fn context_continuation(self) -> Self {
        match self {
            Self::Requested {
                request, selection, ..
            } => Self::Requested {
                request,
                selection,
                origin: ViewerOpenOrigin::ContextContinuation,
            },
            _ => Self::ContextContinuation,
        }
    }

    pub(crate) fn owns_file_selection(self) -> bool {
        matches!(
            self,
            Self::Requested {
                selection: ViewerOpenSelection::ExplicitFile,
                ..
            }
        )
    }

    pub(crate) fn request(self) -> Option<ViewerOpenRequest> {
        match self {
            Self::Requested { request, .. } => Some(request),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_ai_root_poll_delivers_app_warnings_and_clipboard_result_toasts() {
        use crate::external_links::ExternalOpenFailure as Event;
        use crate::feedback::FeedbackKind;
        let mut app = crate::app::setup_app_for_test();
        for (event, kind) in [
            (Event::ClaudeApp, FeedbackKind::Warning),
            (Event::ChatGptApp, FeedbackKind::Warning),
            (Event::AiWebClipboardWritten, FeedbackKind::Info),
            (Event::AiWebClipboardSkipped, FeedbackKind::Info),
            (Event::AiWebClipboardFailed, FeedbackKind::Warning),
        ] {
            app.external_open_failure_tx.send(event).unwrap();
            app.poll_external_open_failures();
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, event.message());
            assert_eq!(app.feedback_toasts_for_test.last().unwrap().1, kind);
        }
    }

    #[test]
    fn external_open_map_failure_repaints_root_and_root_poll_warns() {
        use crate::external_links::{OpenTarget, TestOpenerGuard};
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = super::super::tests::phase_c_support::setup_app();
        let ctx = egui::Context::default();
        // Drain egui's initial repaint budget so the worker wake is observable.
        for _ in 0..3 {
            ctx.run_pass(Default::default(), |_| {})
                .drop_without_applying_deltas();
        }
        let (repaint_tx, repaint_rx) = mpsc::channel();
        ctx.set_request_repaint_callback(move |info| {
            let _ = repaint_tx.send(info.viewport_id);
        });
        let caller = std::thread::current().id();
        let _opener = TestOpenerGuard::install(move |target| {
            assert_ne!(caller, std::thread::current().id());
            let OpenTarget::Map(url) = target else {
                panic!("expected map")
            };
            Err(format!("failed URL: {url}"))
        });

        app.open_google_maps(&ctx, "https://www.google.com/maps/?q=35,139".into());
        assert_eq!(
            repaint_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            egui::ViewportId::ROOT
        );
        assert!(
            app.fs_feedback_toast.is_none(),
            "worker must not mutate App"
        );
        // Exercise the root worker phase, rather than draining the receiver in the test.
        app.render_other_worker_results(&ctx);
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some("Google マップを開けませんでした")
        );
        assert!(app.fs_feedback_toast_surface.is_none());
        assert_eq!(
            app.feedback_toasts_for_test.last().map(|toast| toast.1),
            Some(crate::feedback::FeedbackKind::Warning)
        );
        assert!(matches!(
            app.external_open_failures.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn external_open_general_failure_does_not_publish_notification() {
        use crate::external_links::{OpenTarget, TestOpenerGuard};
        use std::sync::mpsc;
        use std::time::Duration;

        let mut app = super::super::tests::phase_c_support::setup_app();
        let ctx = egui::Context::default();
        let (called_tx, called_rx) = mpsc::channel();
        let _opener = TestOpenerGuard::install(move |_| {
            called_tx.send(()).unwrap();
            Err("Shell rejected the target".into())
        });
        crate::ui_helpers::open_url("https://example.com");
        crate::ui_helpers::open_external_player(std::path::Path::new("movie.mp4"));
        crate::external_links::spawn_open(
            OpenTarget::Tweet("https://x.com/example/status/1".into()),
            None,
        )
        .unwrap();
        for _ in 0..3 {
            called_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        app.render_other_worker_results(&ctx);
        assert!(app.fs_feedback_toast.is_none());
        assert!(matches!(
            app.external_open_failures.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn external_open_invalid_map_request_warns_without_calling_shell() {
        let mut app = super::super::tests::phase_c_support::setup_app();
        let _opener =
            crate::external_links::TestOpenerGuard::install(|_| panic!("invalid map opened"));
        let ctx = egui::Context::default();
        app.open_google_maps(&ctx, "javascript:alert(1)".into());
        app.render_other_worker_results(&ctx);
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some("Google マップを開けませんでした")
        );
    }

    #[test]
    fn browser818_open_owner_same_path_has_distinct_identity_and_retains_intent() {
        let intent = ViewerOpenIntent::BrowserOverlay {
            placement: crate::detached_window_geometry::PhysicalWindowPlacement {
                outer: crate::detached_window_geometry::PhysicalRect {
                    left: -800,
                    top: 50,
                    right: 0,
                    bottom: 650,
                },
                maximized: false,
            },
        };
        let path = PathBuf::from("same.png");
        let first = ExplicitOpenPath::new(path.clone(), ViewerOpenRequest::new(intent));
        let second = ExplicitOpenPath::new(path, ViewerOpenRequest::new(intent));
        assert_eq!(first.path, second.path);
        assert_ne!(first.request, second.request);
        let adoption = ViewerOpenAdoption::requested(second.request);
        assert!(adoption.is_main_new());
        assert!(!adoption.owns_file_selection());
        let exact = adoption.explicit_file();
        assert_eq!(exact.request(), Some(second.request));
        assert!(exact.owns_file_selection());
        let routed = exact.context_continuation();
        assert!(!routed.is_main_new());
        assert!(routed.owns_file_selection());
        assert_eq!(routed.request(), Some(second.request));
        assert_eq!(routed.request().unwrap().intent, intent);
        assert_eq!(routed.explicit_file(), routed);
    }
}
