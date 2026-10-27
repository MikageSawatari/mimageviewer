//! One media owner, one latest artwork job, and one latest immutable wire snapshot.
use super::HostCommand;
use std::{
    path::PathBuf,
    sync::mpsc,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

mod artwork;

/// Borrow the wire-visible prefix without examining the suffix after the limit.
/// The host accepts at most 4096 UTF-16 units, and NUL terminates its text.
pub(crate) fn bounded_text_prefix(text: &str) -> &str {
    let mut units = 0;
    for (start, ch) in text.char_indices() {
        if ch == '\0' || units + ch.len_utf16() > 4096 {
            return &text[..start];
        }
        units += ch.len_utf16();
        if units == 4096 {
            return &text[..start + ch.len_utf8()];
        }
    }
    text
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NowPlayingOwner {
    Local(u64),
    Remote { session: u64, registration: u64 },
}

#[derive(Clone)]
pub(crate) enum NowPlayingArtworkSource {
    None,
    Audio {
        sidecar: Option<PathBuf>,
        settings: Arc<crate::settings::Settings>,
        raw_executor: Arc<crate::raw::RawDevelopExecutor>,
        io_semaphore: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    },
    Video {
        pinned_webp: Option<Arc<[u8]>>,
        sidecar: Option<PathBuf>,
        settings: Arc<crate::settings::Settings>,
        raw_executor: Arc<crate::raw::RawDevelopExecutor>,
        io_semaphore: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    },
}

#[derive(Clone)]
pub(crate) struct NowPlayingInput {
    pub path: PathBuf,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub artwork: NowPlayingArtworkSource,
}
impl NowPlayingInput {
    fn bound_text(&mut self) {
        for text in [&mut self.title, &mut self.album, &mut self.artist]
            .into_iter()
            .flatten()
        {
            let prefix = bounded_text_prefix(text);
            if prefix.len() != text.len() {
                // Replacing also releases an oversized incoming allocation before ownership.
                *text = prefix.to_owned();
            }
        }
    }
    fn same(&self, rhs: &Self) -> bool {
        self.path == rhs.path
            && self.title == rhs.title
            && self.album == rhs.album
            && self.artist == rhs.artist
            && self.artwork.same(&rhs.artwork)
    }
}
fn same_source_settings(a: &crate::settings::Settings, b: &crate::settings::Settings) -> bool {
    a.video_thumb_use_sidecar_image == b.video_thumb_use_sidecar_image
        && a.skip_image_if_video_exists == b.skip_image_if_video_exists
        && a.show_hidden_files == b.show_hidden_files
        && a.archive_file_handling == b.archive_file_handling
        && a.epub_file_handling == b.epub_file_handling
        && a.skip_duplicate_images == b.skip_duplicate_images
        && a.image_ext_priority == b.image_ext_priority
        && a.susie_enabled == b.susie_enabled
}
impl NowPlayingArtworkSource {
    fn same(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (Self::None, Self::None) => true,
            (
                Self::Audio {
                    sidecar: a,
                    settings: sa,
                    ..
                },
                Self::Audio {
                    sidecar: b,
                    settings: sb,
                    ..
                },
            ) => a == b && same_source_settings(sa, sb),
            (
                Self::Video {
                    pinned_webp: a,
                    sidecar: pa,
                    settings: sa,
                    ..
                },
                Self::Video {
                    pinned_webp: b,
                    sidecar: pb,
                    settings: sb,
                    ..
                },
            ) => {
                pa == pb
                    && same_source_settings(sa, sb)
                    && match (a, b) {
                        (None, None) => true,
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        _ => false,
                    }
            }
            _ => false,
        }
    }
}

pub(crate) struct NowPlayingSnapshot {
    pub revision: u64,
    pub title: Arc<str>,
    pub album: Arc<str>,
    pub artist: Arc<str>,
    pub artwork: Arc<str>,
    pub artwork_mime_type: Arc<str>,
}
impl NowPlayingSnapshot {
    fn empty(revision: u64) -> Self {
        Self {
            revision,
            title: "".into(),
            album: "".into(),
            artist: "".into(),
            artwork: "".into(),
            artwork_mime_type: "".into(),
        }
    }
    fn text(revision: u64, input: &NowPlayingInput) -> Self {
        let fallback;
        let title = input
            .title
            .as_deref()
            .map(bounded_text_prefix)
            .filter(|s| !s.trim().is_empty());
        let title = match title {
            Some(title) => title,
            None => {
                // File stems normally borrow their UTF-8 storage too.
                fallback = input.path.file_stem().unwrap_or_default().to_string_lossy();
                bounded_text_prefix(&fallback)
            }
        };
        Self {
            revision,
            title: title.into(),
            album: bounded_text_prefix(input.album.as_deref().unwrap_or("")).into(),
            artist: bounded_text_prefix(input.artist.as_deref().unwrap_or("")).into(),
            ..Self::empty(revision)
        }
    }
    pub(crate) fn command(&self, slot_id: u64) -> crate::video::dsp::bridge::Cmd {
        crate::video::dsp::bridge::Cmd::SetNowPlaying {
            slot_id,
            revision: self.revision,
            title: Arc::clone(&self.title),
            album: Arc::clone(&self.album),
            artist: Arc::clone(&self.artist),
            artwork: Arc::clone(&self.artwork),
            artwork_mime_type: Arc::clone(&self.artwork_mime_type),
        }
    }
}

enum ArtworkState {
    Deferred,
    Pending(Arc<AtomicBool>),
    Absent,
    Ready,
}
enum OwnerState {
    Empty(Arc<NowPlayingSnapshot>),
    Media {
        owner: NowPlayingOwner,
        input: Arc<NowPlayingInput>,
        snapshot: Arc<NowPlayingSnapshot>,
        artwork: ArtworkState,
    },
}
impl OwnerState {
    fn snapshot(&self) -> Arc<NowPlayingSnapshot> {
        match self {
            Self::Empty(s) | Self::Media { snapshot: s, .. } => Arc::clone(s),
        }
    }
    fn cancel(&self) {
        if let Self::Media {
            artwork: ArtworkState::Pending(c),
            ..
        } = self
        {
            c.store(true, Ordering::Release);
        }
    }
}
struct ArtJob {
    revision: u64,
    input: Arc<NowPlayingInput>,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct JobQueue {
    pending: Option<ArtJob>,
    stopped: bool,
}
pub(super) struct Core {
    state: Mutex<OwnerState>,
    revision: AtomicU64,
    active: AtomicBool,
    queue: (Mutex<JobQueue>, Condvar),
    host_tx: mpsc::Sender<HostCommand>,
    refresh_pending: AtomicBool,
    warned: AtomicBool,
    warning: Mutex<Option<String>>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
}
impl Core {
    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub(super) fn snapshot(&self) -> Arc<NowPlayingSnapshot> {
        self.state.lock().unwrap().snapshot()
    }
    pub(super) fn refresh_consumed(&self) {
        self.refresh_pending.store(false, Ordering::Release);
    }
    fn refresh(&self) {
        if !self.refresh_pending.swap(true, Ordering::AcqRel) {
            let _ = self.host_tx.send(HostCommand::RefreshNowPlaying);
        }
    }
    pub(super) fn warn(&self, message: String) {
        crate::logger::log(format!("[EffeTune Now Playing] {message}"));
        if !self.warned.swap(true, Ordering::AcqRel) {
            *self.warning.lock().unwrap() = Some(message);
            super::wake_ui(&self.repaint);
        }
    }
    pub(super) fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub(super) fn is_current(&self, revision: u64) -> bool {
        self.state.lock().unwrap().snapshot().revision == revision
    }
    pub(super) fn start_artwork(&self) {
        let mut state = self.state.lock().unwrap();
        self.schedule(&mut state);
    }
    fn schedule(&self, state: &mut OwnerState) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        if let OwnerState::Media {
            input,
            snapshot,
            artwork,
            ..
        } = state
        {
            if !matches!(artwork, ArtworkState::Deferred) {
                return;
            }
            if matches!(input.artwork, NowPlayingArtworkSource::None) {
                *artwork = ArtworkState::Absent;
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let job = ArtJob {
                revision: snapshot.revision,
                input: Arc::clone(input),
                cancel: Arc::clone(&cancel),
            };
            *artwork = ArtworkState::Pending(cancel);
            self.queue.0.lock().unwrap().pending = Some(job);
            self.queue.1.notify_one();
        }
    }
    fn complete(&self, job: &ArtJob, result: Result<Option<artwork::EncodedArt>, String>) {
        if job.cancel.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let OwnerState::Media {
            snapshot, artwork, ..
        } = &mut *state
        else {
            return;
        };
        if snapshot.revision != job.revision || !self.active.load(Ordering::Acquire) {
            return;
        }
        match result {
            Ok(Some(art)) => {
                let warning = art.warning;
                let revision = self.next_revision();
                *snapshot = Arc::new(NowPlayingSnapshot {
                    revision,
                    title: Arc::clone(&snapshot.title),
                    album: Arc::clone(&snapshot.album),
                    artist: Arc::clone(&snapshot.artist),
                    artwork: art.base64,
                    artwork_mime_type: art.mime.into(),
                });
                *artwork = ArtworkState::Ready;
                drop(state);
                self.refresh();
                if let Some(error) = warning {
                    self.warn(format!("画像を読み込めませんでした: {error}"));
                }
            }
            Ok(None) => *artwork = ArtworkState::Absent,
            Err(error) => {
                *artwork = ArtworkState::Absent;
                drop(state);
                self.warn(format!("画像を読み込めませんでした: {error}"));
            }
        }
    }
}

pub(super) struct NowPlaying {
    pub(super) core: Arc<Core>,
    available: bool,
}
impl NowPlaying {
    pub(super) fn new(
        available: bool,
        host_tx: mpsc::Sender<HostCommand>,
        repaint: Arc<Mutex<Option<egui::Context>>>,
    ) -> Self {
        Self::new_with_prepare(available, host_tx, repaint, artwork::prepare)
    }
    fn new_with_prepare(
        available: bool,
        host_tx: mpsc::Sender<HostCommand>,
        repaint: Arc<Mutex<Option<egui::Context>>>,
        prepare: impl Fn(
            &NowPlayingInput,
            &Arc<AtomicBool>,
        ) -> Result<Option<artwork::EncodedArt>, String>
        + Send
        + 'static,
    ) -> Self {
        let core = Arc::new(Core {
            state: Mutex::new(OwnerState::Empty(Arc::new(NowPlayingSnapshot::empty(0)))),
            revision: AtomicU64::new(0),
            active: AtomicBool::new(false),
            queue: Default::default(),
            host_tx,
            refresh_pending: AtomicBool::new(false),
            warned: AtomicBool::new(false),
            warning: Mutex::new(None),
            repaint,
        });
        if available {
            let worker = Arc::clone(&core);
            std::thread::Builder::new()
                .name("effetune-now-playing-art".into())
                .spawn(move || {
                    loop {
                        let job = {
                            let mut queue = worker.queue.0.lock().unwrap();
                            while queue.pending.is_none() && !queue.stopped {
                                queue = worker.queue.1.wait(queue).unwrap();
                            }
                            if queue.stopped {
                                break;
                            }
                            queue.pending.take().unwrap()
                        };
                        if job.cancel.load(Ordering::Acquire) {
                            continue;
                        }
                        let result = prepare(&job.input, &job.cancel);
                        worker.complete(&job, result);
                    }
                })
                .expect("EffeTune artwork worker spawn");
        }
        Self { core, available }
    }
    pub(super) fn publish(&self, owner: NowPlayingOwner, mut input: NowPlayingInput) {
        if !self.available {
            return;
        }
        input.bound_text();
        let mut state = self.core.state.lock().unwrap();
        if matches!(&*state,OwnerState::Media {owner:existing,input:old,..} if *existing==owner && old.same(&input))
        {
            return;
        }
        state.cancel();
        let revision = self.core.next_revision();
        let snapshot = Arc::new(NowPlayingSnapshot::text(revision, &input));
        *state = OwnerState::Media {
            owner,
            input: Arc::new(input),
            snapshot,
            artwork: ArtworkState::Deferred,
        };
        drop(state);
        self.core.refresh();
    }
    pub(super) fn clear(&self, owner: NowPlayingOwner) {
        if !self.available {
            return;
        }
        let mut state = self.core.state.lock().unwrap();
        if !matches!(&*state,OwnerState::Media {owner:existing,..} if *existing==owner) {
            return;
        }
        state.cancel();
        *state = OwnerState::Empty(Arc::new(NowPlayingSnapshot::empty(
            self.core.next_revision(),
        )));
        self.core.queue.0.lock().unwrap().pending = None;
        drop(state);
        self.core.refresh();
    }
    pub(super) fn set_active(&self, active: bool) {
        if !self.available {
            return;
        }
        self.core.active.store(active, Ordering::Release);
        let state = self.core.state.lock().unwrap();
        if !active {
            state.cancel();
            self.core.queue.0.lock().unwrap().pending = None;
        }
    }
    pub(super) fn owner(&self) -> Option<NowPlayingOwner> {
        match &*self.core.state.lock().unwrap() {
            OwnerState::Media { owner, .. } => Some(*owner),
            _ => None,
        }
    }
    pub(super) fn metadata_matches(
        &self,
        owner: NowPlayingOwner,
        path: &std::path::Path,
        title: Option<&str>,
        album: Option<&str>,
        artist: Option<&str>,
    ) -> bool {
        let title = title.map(bounded_text_prefix);
        let album = album.map(bounded_text_prefix);
        let artist = artist.map(bounded_text_prefix);
        matches!(&*self.core.state.lock().unwrap(),OwnerState::Media {owner:existing,input,..} if *existing==owner && input.path==path && input.title.as_deref()==title && input.album.as_deref()==album && input.artist.as_deref()==artist)
    }
    pub(super) fn source_matches(
        &self,
        owner: NowPlayingOwner,
        sidecar: Option<&std::path::Path>,
        settings: &crate::settings::Settings,
    ) -> bool {
        let state = self.core.state.lock().unwrap();
        let OwnerState::Media {
            owner: existing,
            input,
            ..
        } = &*state
        else {
            return false;
        };
        if *existing != owner {
            return false;
        }
        match &input.artwork {
            NowPlayingArtworkSource::Audio {
                sidecar: old,
                settings: old_settings,
                ..
            }
            | NowPlayingArtworkSource::Video {
                sidecar: old,
                settings: old_settings,
                ..
            } => old.as_deref() == sidecar && same_source_settings(old_settings, settings),
            NowPlayingArtworkSource::None => sidecar.is_none(),
        }
    }
    pub(super) fn refresh_input(&self, owner: NowPlayingOwner, input: NowPlayingInput) {
        // Explicit source refresh is a new media revision even for an equal descriptor.
        if !self.available {
            return;
        }
        {
            let mut state = self.core.state.lock().unwrap();
            if !matches!(&*state,OwnerState::Media {owner:existing,..} if *existing==owner) {
                return;
            }
            state.cancel();
            *state = OwnerState::Empty(Arc::new(NowPlayingSnapshot::empty(
                self.core.next_revision(),
            )));
        }
        self.publish(owner, input);
    }
    pub(super) fn take_warning(&self) -> Option<String> {
        self.core.warning.lock().unwrap().take()
    }
}
impl Drop for NowPlaying {
    fn drop(&mut self) {
        self.core.state.lock().unwrap().cancel();
        let mut queue = self.core.queue.0.lock().unwrap();
        queue.stopped = true;
        queue.pending = None;
        self.core.queue.1.notify_one();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn input(path: &str) -> NowPlayingInput {
        NowPlayingInput {
            path: path.into(),
            title: None,
            album: None,
            artist: None,
            artwork: NowPlayingArtworkSource::None,
        }
    }
    fn service(available: bool) -> (NowPlaying, mpsc::Receiver<HostCommand>) {
        let (tx, rx) = mpsc::channel();
        (
            NowPlaying::new(available, tx, Arc::new(Mutex::new(None))),
            rx,
        )
    }
    #[test]
    fn now_playing_owner_reopen_and_old_context_clear_are_revision_scoped() {
        let (now, _rx) = service(true);
        let a = NowPlayingOwner::Local(1);
        let b = NowPlayingOwner::Local(2);
        now.publish(a, input("a.mp3"));
        let first = now.core.snapshot();
        assert_eq!(&*first.title, "a");
        now.publish(b, input("b.mp3"));
        let second = now.core.snapshot();
        now.clear(a);
        assert_eq!(now.core.snapshot().revision, second.revision);
        now.publish(a, input("a.mp3"));
        assert!(now.core.snapshot().revision > second.revision);
        now.clear(a);
        let clear = now.core.snapshot();
        assert!(
            clear.title.is_empty()
                && clear.album.is_empty()
                && clear.artist.is_empty()
                && clear.artwork.is_empty()
                && clear.artwork_mime_type.is_empty()
        );
        now.publish(a, input("a.mp3"));
        assert!(now.core.snapshot().revision > clear.revision);
        assert!(!now.core.is_current(first.revision));
    }
    #[test]
    fn now_playing_text_then_art_is_a_complete_new_snapshot_and_stale_art_is_discarded() {
        let (now, _rx) = service(true);
        now.set_active(true);
        let owner = NowPlayingOwner::Local(3);
        let mut source = input("a.mp3");
        source.title = Some("曲😀".into());
        source.album = Some("album".into());
        source.artist = Some("artist".into());
        now.publish(owner, source.clone());
        let text = now.core.snapshot();
        let job = ArtJob {
            revision: text.revision,
            input: Arc::new(source),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        now.core.complete(
            &job,
            Ok(Some(artwork::EncodedArt {
                base64: "YWJj".into(),
                mime: "image/png",
                warning: None,
            })),
        );
        let art = now.core.snapshot();
        assert!(art.revision > text.revision);
        assert_eq!(&*art.artwork, "YWJj");
        assert_eq!(&*art.title, "曲😀");
        assert_eq!(&*art.album, "album");
        assert_eq!(&*art.artist, "artist");
        now.publish(NowPlayingOwner::Local(4), input("b.mp3"));
        let latest = now.core.snapshot();
        now.core.complete(
            &job,
            Ok(Some(artwork::EncodedArt {
                base64: "old".into(),
                mime: "image/png",
                warning: None,
            })),
        );
        assert_eq!(now.core.snapshot().revision, latest.revision);
        assert!(now.core.snapshot().artwork.is_empty());
    }
    #[test]
    fn now_playing_portable_ignores_source_and_unavailable_defers_all_jobs_and_writes() {
        let mut candidate = input("missing.mp3");
        candidate.artwork = NowPlayingArtworkSource::Audio {
            sidecar: Some("missing.png".into()),
            settings: Arc::new(crate::settings::Settings::default()),
            raw_executor: Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
            io_semaphore: Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
        };
        let (portable, rx) = service(false);
        portable.publish(NowPlayingOwner::Local(1), candidate.clone());
        portable.set_active(true);
        assert!(portable.owner().is_none());
        assert!(rx.try_recv().is_err());
        assert!(portable.core.queue.0.lock().unwrap().pending.is_none());
        let (idle, _rx) = service(true);
        idle.publish(NowPlayingOwner::Local(1), candidate.clone());
        idle.core.start_artwork();
        assert_eq!(&*idle.core.snapshot().title, "missing");
        assert!(idle.core.queue.0.lock().unwrap().pending.is_none());
        let bridge = super::super::new_bridge();
        let (host, commands, _) = crate::video::dsp::bridge::Bridge::fake_host_for_test();
        bridge.install_fake_host_for_test(host);
        super::super::send_latest_now_playing(&idle.core, &Arc::downgrade(&bridge), &mut None);
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn now_playing_remote_registration_clear_and_no_art_termination_preserve_owner() {
        let (now, _rx) = service(true);
        now.set_active(true);
        let old = NowPlayingOwner::Remote {
            session: 8,
            registration: 1,
        };
        let current = NowPlayingOwner::Remote {
            session: 8,
            registration: 2,
        };
        now.publish(old, input("old.mp3"));
        now.publish(current, input("new.mp3"));
        let snapshot = now.core.snapshot();
        now.clear(old);
        assert_eq!(now.core.snapshot().revision, snapshot.revision);
        let job = ArtJob {
            revision: snapshot.revision,
            input: Arc::new(input("new.mp3")),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        now.core.complete(&job, Ok(None));
        assert_eq!(now.owner(), Some(current));
        assert_eq!(now.core.snapshot().revision, snapshot.revision);
        now.clear(current);
        assert!(now.owner().is_none());
    }
    #[test]
    fn now_playing_notifications_are_bounded_and_warning_is_once_without_disabling_audio() {
        let (now, rx) = service(true);
        for n in 0..100 {
            now.publish(NowPlayingOwner::Local(n), input("same.mp3"));
        }
        assert!(matches!(rx.try_recv(), Ok(HostCommand::RefreshNowPlaying)));
        assert!(rx.try_recv().is_err());
        now.core.warn("first".into());
        now.core.warn("second".into());
        assert_eq!(now.take_warning(), Some("first".into()));
        assert!(now.take_warning().is_none());
        assert_eq!(now.owner(), Some(NowPlayingOwner::Local(99)));
    }
    #[test]
    fn now_playing_art_worker_keeps_one_running_and_only_latest_pending_job() {
        let (host_tx, host_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        // Closure state is protected only by this worker; captured receiver gives deterministic admission.
        let prepare = move |input: &NowPlayingInput, _cancel: &Arc<AtomicBool>| {
            entered_tx
                .send((
                    input.path.clone(),
                    std::thread::current().name().unwrap().to_owned(),
                ))
                .unwrap();
            if input.path == PathBuf::from("a.mp3") {
                release_rx.recv().unwrap();
            }
            Ok(Some(artwork::EncodedArt {
                base64: "YWJj".into(),
                mime: "image/png",
                warning: None,
            }))
        };
        let now = NowPlaying::new_with_prepare(true, host_tx, Arc::new(Mutex::new(None)), prepare);
        now.set_active(true);
        let executor = Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap());
        let io = Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1));
        let source = |path: &str| {
            let mut input = input(path);
            input.artwork = NowPlayingArtworkSource::Audio {
                sidecar: None,
                settings: Arc::new(crate::settings::Settings::default()),
                raw_executor: Arc::clone(&executor),
                io_semaphore: Arc::clone(&io),
            };
            input
        };
        now.publish(NowPlayingOwner::Local(1), source("a.mp3"));
        now.core.start_artwork();
        let (path, thread) = entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(path, PathBuf::from("a.mp3"));
        assert_eq!(thread, "effetune-now-playing-art");
        now.publish(NowPlayingOwner::Local(2), source("b.mp3"));
        now.core.start_artwork();
        now.publish(NowPlayingOwner::Local(3), source("c.mp3"));
        now.core.start_artwork();
        assert_eq!(
            now.core
                .queue
                .0
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
                .input
                .path,
            PathBuf::from("c.mp3")
        );
        let _ = host_rx.try_iter().count();
        now.core.refresh_consumed();
        release_tx.send(()).unwrap();
        assert_eq!(
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap().0,
            PathBuf::from("c.mp3")
        );
        assert!(entered_rx.recv_timeout(Duration::from_millis(20)).is_err());
        // A fresh host wake proves completion without polling.
        assert!(matches!(
            host_rx.recv_timeout(Duration::from_secs(3)),
            Ok(HostCommand::RefreshNowPlaying)
        ));
        assert_eq!(&*now.core.snapshot().artwork, "YWJj");
        assert_eq!(now.owner(), Some(NowPlayingOwner::Local(3)));
    }
    #[test]
    #[cfg(not(feature = "portable"))]
    fn now_playing_host_control_resends_equal_and_empty_snapshots_on_actual_slot() {
        let mut controller = super::super::EffetuneController::new();
        controller.now_playing.set_active(true);
        let bridge = super::super::new_bridge();
        let (host, commands, _events) = crate::video::dsp::bridge::Bridge::fake_host_for_test();
        bridge.install_fake_host_for_test(Arc::clone(&host));
        bridge.set_fake_slot_id_for_test(17);
        let owner = NowPlayingOwner::Local(44);
        controller.publish_now_playing(owner, input("a.mp3"));
        let register = || {
            controller
                .host_tx
                .send(HostCommand::RegisterBridge(Arc::downgrade(&bridge)))
                .unwrap()
        };
        register();
        let first = commands.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(first["cmd"], "set_now_playing");
        assert_eq!(first["slot_id"], 17);
        assert_eq!(first["title"], "a");
        register();
        let second = commands.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(first, second);
        controller.clear_now_playing(owner);
        let clear = commands.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(clear["title"], "");
        assert_eq!(clear["artworkMimeType"], "");
        controller
            .host_tx
            .send(HostCommand::RegisterBridge(Arc::downgrade(&bridge)))
            .unwrap();
        assert_eq!(
            commands.recv_timeout(Duration::from_secs(3)).unwrap(),
            clear
        );
        assert!(
            host.fake_command_threads_for_test()
                .iter()
                .all(|name| name == "effetune-host-control")
        );
        assert!(
            host.recv_timeout(Duration::from_millis(20)).is_err(),
            "no metadata ACK enters event_rx"
        );
    }
    #[test]
    fn now_playing_ordinary_write_failure_keeps_effect_and_has_one_warning() {
        let (now, _rx) = service(true);
        now.set_active(true);
        now.publish(NowPlayingOwner::Local(1), input("a.mp3"));
        let bridge = super::super::new_bridge();
        let (host, commands, _) = crate::video::dsp::bridge::Bridge::fake_host_for_test();
        bridge.install_fake_host_for_test(host);
        bridge.mark_enabled_without_bridge_for_test();
        drop(commands);
        super::super::send_latest_now_playing(&now.core, &Arc::downgrade(&bridge), &mut None);
        assert!(bridge.is_enabled());
        assert!(
            now.take_warning()
                .unwrap()
                .contains("曲情報を送信できません")
        );
        super::super::send_latest_now_playing(&now.core, &Arc::downgrade(&bridge), &mut None);
        assert!(bridge.is_enabled());
        assert!(now.take_warning().is_none());
    }
    #[test]
    fn now_playing_text_utf16_boundary_nul_and_source_policy_revision() {
        let (now, _rx) = service(true);
        let owner = NowPlayingOwner::Local(1);
        let mut data = input("a.mp3");
        data.title = Some(format!("{}\u{1f600}tail", "a".repeat(4095)));
        data.album = Some("album\0private".into());
        now.publish(owner, data);
        let text = now.core.snapshot();
        assert_eq!(text.title.encode_utf16().count(), 4095);
        assert_eq!(&*text.album, "album");
        let mut data = input("a.mp3");
        let settings = crate::settings::Settings::default();
        data.artwork = NowPlayingArtworkSource::Audio {
            sidecar: Some("a.png".into()),
            settings: Arc::new(settings.clone()),
            raw_executor: Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
            io_semaphore: Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
        };
        now.publish(owner, data.clone());
        let first = now.core.snapshot().revision;
        assert!(now.source_matches(owner, Some(std::path::Path::new("a.png")), &settings));
        let mut changed = settings;
        changed.image_ext_priority.reverse();
        assert!(!now.source_matches(owner, Some(std::path::Path::new("a.png")), &changed));
        if let NowPlayingArtworkSource::Audio { settings, .. } = &mut data.artwork {
            *settings = Arc::new(changed);
        }
        now.publish(owner, data);
        assert!(now.core.snapshot().revision > first);
    }

    #[test]
    fn now_playing_borrowed_text_prefix_preserves_scalar_and_nul_boundaries() {
        for (text, expected_units) in [
            (String::new(), 0),
            ("\0private".into(), 0),
            ("album\0private".into(), 5),
            (format!("{}\u{1f600}suffix", "a".repeat(4094)), 4096),
            (format!("{}\u{1f600}suffix", "a".repeat(4095)), 4095),
            (format!("{}\u{1f600}suffix", "\u{20ac}".repeat(4096)), 4096),
        ] {
            let prefix = bounded_text_prefix(&text);
            assert_eq!(prefix.encode_utf16().count(), expected_units);
            assert!(!prefix.contains('\0'));
            assert_eq!(prefix.as_ptr(), text.as_ptr(), "must borrow input storage");
        }
    }

    #[test]
    fn now_playing_huge_suffix_changes_do_not_replace_owner_or_advance_revision() {
        let (now, _rx) = service(true);
        let owner = NowPlayingOwner::Local(7);
        let path = std::path::Path::new("a.mp3");
        let prefix = "\u{1f600}".repeat(2048);
        let huge = |suffix: &str| format!("{prefix}{}", suffix.repeat(1_000_000));
        let mut first = input("a.mp3");
        first.title = Some(huge("a"));
        first.album = Some(huge("b"));
        first.artist = Some("artist\0".to_owned() + "c".repeat(1_000_000).as_str());
        now.publish(owner, first);
        let snapshot = now.core.snapshot();
        assert_eq!(&*snapshot.title, prefix);
        assert_eq!(&*snapshot.album, prefix);
        assert_eq!(&*snapshot.artist, "artist");
        let stored = match &*now.core.state.lock().unwrap() {
            OwnerState::Media { input, .. } => Arc::clone(input),
            _ => panic!("missing owner"),
        };
        for field in [&stored.title, &stored.album, &stored.artist] {
            let text = field.as_ref().unwrap();
            assert!(text.encode_utf16().count() <= 4096);
            assert!(text.capacity() <= 4096 * 3, "oversized allocation retained");
        }

        let mut same_prefix = input("a.mp3");
        same_prefix.title = Some(huge("d"));
        same_prefix.album = Some(huge("e"));
        same_prefix.artist = Some("artist\0".to_owned() + "f".repeat(1_000_000).as_str());
        assert!(now.metadata_matches(
            owner,
            path,
            same_prefix.title.as_deref(),
            same_prefix.album.as_deref(),
            same_prefix.artist.as_deref()
        ));
        now.publish(owner, same_prefix);
        assert_eq!(now.core.snapshot().revision, snapshot.revision);
        assert!(
            matches!(&*now.core.state.lock().unwrap(), OwnerState::Media { input, .. } if Arc::ptr_eq(input, &stored))
        );

        let mut changed = (*stored).clone();
        changed.title.as_mut().unwrap().replace_range(..4, "x");
        assert!(!now.metadata_matches(
            owner,
            path,
            changed.title.as_deref(),
            changed.album.as_deref(),
            changed.artist.as_deref()
        ));
        now.publish(owner, changed);
        assert!(now.core.snapshot().revision > snapshot.revision);
        assert!(now.core.snapshot().title.starts_with('x'));
    }

    #[test]
    fn now_playing_snapshot_bounds_before_title_emptiness_and_fallback() {
        let mut source = input("fallback.mp3");
        source.title = Some(" ".repeat(4096) + "not-visible".repeat(100_000).as_str());
        source.album = Some("album\0".to_owned() + "hidden".repeat(100_000).as_str());
        source.artist = Some("\u{1f600}".repeat(300_000));
        let snapshot = NowPlayingSnapshot::text(1, &source);
        assert_eq!(&*snapshot.title, "fallback");
        assert_eq!(&*snapshot.album, "album");
        assert_eq!(snapshot.artist.encode_utf16().count(), 4096);

        source.title = Some("\0".to_owned() + "hidden".repeat(100_000).as_str());
        assert_eq!(&*NowPlayingSnapshot::text(2, &source).title, "fallback");
        source.title = None;
        source.path = format!("{}.mp3", "\u{1f600}".repeat(300_000)).into();
        assert_eq!(
            NowPlayingSnapshot::text(3, &source)
                .title
                .encode_utf16()
                .count(),
            4096
        );
    }
}
