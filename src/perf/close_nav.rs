//! §1.801: synchronous main fullscreen-return diagnostics, never a navigation owner.
use std::cell::Cell;
use std::time::Instant;

#[derive(Clone, Copy)]
struct Context {
    seq: u64,
    frame: u64,
}
thread_local! {
    static ACTIVE: Cell<Option<Context>> = const { Cell::new(None) };
}

macro_rules! stages {
    ($($name:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Stage { $($name),+ }
        impl Stage {
            fn name(self) -> &'static str {
                match self { $(Self::$name => stringify!($name)),+ }
            }
        }
    };
}
stages!(
    TakeReturn,
    ApplyReturn,
    Dispatch,
    DirectOpen,
    OpenAdmission,
    OpenClassified,
    NormalizePath,
    CaptureCursor,
    ModalAdmission,
    SnapshotAdmission,
    ClassifyPath,
    CaptureNavigation,
    SmartTarget,
    ClaimOwner,
    CancelArchive,
    CancelStartup,
    CancelBookmark,
    RetireBookOpen,
    RetireClassification,
    RetireColdDocument,
    RetireHistory,
    PathIsFile,
    LoadClaimed,
    ScanDirectory,
    OwnerCurrent,
    PrepareListing,
    BookRoot,
    FavoriteProjection,
    MaterializeListing,
    RatingRead,
    PrepareInstall,
    RemoteSnapshot,
    ResolveReturn,
    ResolveZipReturn,
    ResolveCollectionReturn,
    ResolveSpecialReturn,
    RetireNoDestination,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Complete,
    NoDestination,
    UnsupportedNavigation,
    ModalBlocked,
    SnapshotRefused,
    Classifying,
    AdmissionRefused,
    OwnerStale,
    ScanFailed,
    Ignored,
    Loaded,
    VisibilityRefused,
    SmartDispatched,
}

pub(crate) struct Trace(Option<Option<Context>>);
impl Trace {
    pub(crate) fn begin(seq: u64, frame: u64) -> Self {
        Self(super::is_enabled().then(|| ACTIVE.replace(Some(Context { seq, frame }))))
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            ACTIVE.set(previous);
        }
    }
}

pub(crate) struct Span {
    timing: Option<(Context, Instant)>,
    stage: Stage,
    outcome: Outcome,
}
impl Span {
    #[inline]
    pub(crate) fn new(stage: Stage) -> Self {
        Self {
            timing: ACTIVE.get().map(|context| (context, Instant::now())),
            stage,
            outcome: Outcome::Complete,
        }
    }
    pub(crate) fn outcome(&mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let Some((context, start)) = self.timing else {
            return;
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        #[cfg(test)]
        record(TestEvent::Stage(self.stage, self.outcome));
        super::event(
            "nav",
            "close_nav_stage",
            None,
            context.seq,
            &[
                ("stage", self.stage.name().into()),
                ("outcome", format!("{:?}", self.outcome).into()),
                ("ms", ms.into()),
                ("n", context.frame.into()),
            ],
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitSite {
    AiSessions,
    RemoteSessionState,
    RemoteWebConnections,
    DataDirOverride,
    EpubPins,
}
impl WaitSite {
    fn name(self) -> &'static str {
        match self {
            Self::AiSessions => "ai_runtime.sessions",
            Self::RemoteSessionState => "remote_session.inner",
            Self::RemoteWebConnections => "remote_session.remote_web_connections",
            Self::DataDirOverride => "data_dir.TEST_OVERRIDE",
            Self::EpubPins => "pdf_loader.epub_pin_coord",
        }
    }
}

/// Measure acquisition only. No try_lock, holder tracking lock, or changed poison policy.
pub(crate) struct Wait {
    start: Option<Instant>,
    site: WaitSite,
    operation: &'static str,
}
impl Wait {
    #[inline]
    pub(crate) fn for_close(site: WaitSite, operation: &'static str) -> Self {
        if ACTIVE.get().is_some() {
            Self::new(site, operation)
        } else {
            Self {
                start: None,
                site,
                operation,
            }
        }
    }
    #[inline]
    pub(crate) fn new(site: WaitSite, operation: &'static str) -> Self {
        #[cfg(test)]
        record(TestEvent::Attempt(site));
        Self {
            start: super::is_enabled().then(Instant::now),
            site,
            operation,
        }
    }
    pub(crate) fn acquired(self) {
        #[cfg(test)]
        record(TestEvent::Acquired(self.site));
        let Some(start) = self.start else { return };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if ms < 5.0 {
            return;
        }
        let context = ACTIVE.get();
        super::event(
            "sync",
            "lock_wait",
            None,
            context.map_or(0, |c| c.seq),
            &[
                ("lock", self.site.name().into()),
                ("operation", self.operation.into()),
                ("ms", ms.into()),
                ("close_navigation", context.is_some().into()),
                ("holder", "unknown".into()),
            ],
        );
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TestEvent {
    Stage(Stage, Outcome),
    Attempt(WaitSite),
    Acquired(WaitSite),
}
#[cfg(test)]
thread_local! {
    static TEST_EVENTS: std::cell::RefCell<Option<(Vec<TestEvent>, Option<std::sync::mpsc::Sender<TestEvent>>)>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn record(event: TestEvent) {
    TEST_EVENTS.with_borrow_mut(|slot| {
        if let Some((events, sender)) = slot {
            events.push(event);
            if let Some(sender) = sender {
                let _ = sender.send(event);
            }
        }
    });
}
#[cfg(test)]
pub(crate) struct TestCapture(Option<Context>);
#[cfg(test)]
impl TestCapture {
    pub(crate) fn begin(sender: Option<std::sync::mpsc::Sender<TestEvent>>) -> Self {
        TEST_EVENTS.with_borrow_mut(|slot| {
            assert!(slot.is_none());
            *slot = Some((Vec::new(), sender));
        });
        Self(ACTIVE.replace(Some(Context { seq: 801, frame: 1 })))
    }
    pub(crate) fn events(&self) -> Vec<TestEvent> {
        TEST_EVENTS.with_borrow(|slot| slot.as_ref().unwrap().0.clone())
    }
}
#[cfg(test)]
impl Drop for TestCapture {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
        TEST_EVENTS.with_borrow_mut(|slot| *slot = None);
    }
}
