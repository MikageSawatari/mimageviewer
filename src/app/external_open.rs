//! Request-scoped presentation and selection policy. No App-global next-open flags.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
                    origin: ViewerOpenOrigin::MainNew,
                    ..
                }
        )
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
