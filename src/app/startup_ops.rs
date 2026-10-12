use super::*;
use miv_startup::{Lane, Outcome, Stage, WatchHandle, WatchSlot, event_span};

/// Diagnostics for this existing resolver request, through UI acceptance/refusal.
/// This is not the lifetime of downstream async enumeration or conversion.
pub(super) struct InitialNavigationTrace {
    dispatch: Option<miv_startup::Span>,
    watch: Option<WatchHandle>,
}

impl InitialNavigationTrace {
    fn begin(boundary: &str) -> Self {
        let watch = miv_startup::first_dispatch_watch();
        if let Some(watch) = watch {
            watch.begin();
        }
        Self {
            dispatch: Some(
                event_span(Lane::Navigation, Stage::InitialTargetResolve, 0)
                    .watched_optional(watch)
                    .detail(boundary),
            ),
            watch,
        }
    }
    fn correlation(&self) -> u64 {
        self.dispatch.as_ref().map_or(0, miv_startup::Span::id)
    }
    fn finish(mut self, outcome: Outcome) {
        if let Some(dispatch) = self.dispatch.take() {
            dispatch.finish(outcome);
        }
        if let Some(watch) = self.watch.take() {
            watch.retire(outcome);
        }
    }
}

impl Drop for InitialNavigationTrace {
    fn drop(&mut self) {
        if let Some(dispatch) = self.dispatch.take() {
            dispatch.finish(Outcome::Cancelled);
        }
        if let Some(watch) = self.watch.take() {
            watch.retire(Outcome::Cancelled);
        }
    }
}

const STARTUP_OPEN_PATH_RESOLVE_TOAST_DELAY: std::time::Duration =
    std::time::Duration::from_millis(400);

#[must_use]
enum StartupOpenApplyOutcome {
    Opened,
    NotOpenable,
    Refused(FolderOpenRefusal),
    #[cfg(windows)]
    DetachedRefused(OpenAdmissionRefusal),
}

#[cfg(test)]
thread_local! {
    static FORCE_STARTUP_OPEN_RESOLVE_SPAWN_FAILURE: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

#[cfg(test)]
pub(super) fn force_startup_open_resolve_spawn_failure_for_test(force: bool) {
    FORCE_STARTUP_OPEN_RESOLVE_SPAWN_FAILURE.with(|slot| slot.set(force));
}

#[derive(Clone)]
pub(super) struct DirectMediaPrepared {
    item: GridItem,
    metadata: Option<(i64, i64)>,
    rating: crate::rating_sort::RatingSortKey,
    rating_generation: u64,
    rotation: crate::rotation_db::Rotation,
    tags: HashMap<String, Vec<String>>,
}

impl App {
    pub(crate) fn set_startup_open_path(&mut self, path: PathBuf) {
        self.startup_open_path = Some(crate::path_key::normalize_input_path(&path));
    }

    pub(super) fn poll_deferred_startup_open_path(&mut self, ctx: &egui::Context) {
        if !self.initialized
            || self.remote_session_blocks_local_control()
            || self.startup_open_path_resolve_pending.is_some()
            || self.document_open_modal_admission_blocked()
        {
            return;
        }
        if let Some(path) = self.startup_open_path.take() {
            self.start_startup_open_path_resolve(path, StartupOpenPathSource::InitialStartup, ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn poll_activation_open_paths(&mut self, ctx: &egui::Context) {
        if self.remote_session_blocks_local_control() {
            self.reject_activation_open_paths_for_remote(ctx);
            return;
        }
        if self.composition_editor_modal_active() {
            let rejected = self.activation_open_path_rx.try_iter().count();
            if rejected != 0 {
                crate::logger::log(format!(
                    "composition editor: rejected {rejected} activation request(s)"
                ));
                self.show_feedback_toast("本の構成を編集中のため、別の本を開けません".into());
            }
            return;
        }
        // Keep the channel queued while a document dialog owns local admission. Draining it
        // after terminal naturally applies the existing last-activation-wins rule.
        if self.document_open_modal_admission_blocked() {
            return;
        }
        let current_acquisition_sequence = self
            .remote_session_handle()
            .map(|handle| handle.snapshot().acquisition_sequence);
        let requests: Vec<_> = self.activation_open_path_rx.try_iter().collect();
        let rejected = requests
            .iter()
            .filter(|request| request.rejected_for_remote(current_acquisition_sequence))
            .count();
        self.notify_rejected_remote_activation_paths(rejected, ctx);
        let paths: Vec<_> = requests
            .into_iter()
            .filter(|request| !request.rejected_for_remote(current_acquisition_sequence))
            .collect();
        if self.sidecar_restore_active() {
            if !paths.is_empty() {
                crate::logger::log(format!(
                    "single_instance: discarded {} activation open request(s) during sidecar restore",
                    paths.len()
                ));
            }
            return;
        }
        let Some(path) = paths.into_iter().max_by_key(|request| request.received_at) else {
            return;
        };
        let path = path.path;
        crate::logger::log(format!(
            "single_instance: resolving forwarded path in worker: {}",
            path.display()
        ));
        self.start_startup_open_path_resolve(path, StartupOpenPathSource::Activation, ctx);
        ctx.request_repaint();
    }

    #[cfg(windows)]
    pub(crate) fn reject_activation_open_paths_for_remote(&mut self, ctx: &egui::Context) {
        let current_acquisition_sequence = self
            .remote_session_handle()
            .map(|handle| handle.snapshot().acquisition_sequence);
        let requests: Vec<_> = self.activation_open_path_rx.try_iter().collect();
        let (rejected, deferred): (Vec<_>, Vec<_>) = requests
            .into_iter()
            .partition(|request| request.rejected_for_remote(current_acquisition_sequence));
        // The phase observed by poll_remote_session may be stale after a return. Put requests
        // received in that Local phase back for the ordinary activation poll; their original
        // receive time preserves the last-activation-wins ordering.
        for request in deferred {
            let _ = self.activation_open_path_tx.send(request);
        }
        self.notify_rejected_remote_activation_paths(rejected.len(), ctx);
    }

    #[cfg(windows)]
    fn notify_rejected_remote_activation_paths(&mut self, rejected: usize, ctx: &egui::Context) {
        if rejected != 0 {
            crate::logger::log(format!(
                "single_instance: rejected {rejected} activation open request(s) during Remote ownership"
            ));
            self.show_feedback_toast(
                "リモート閲覧中のため、二重起動で渡されたパスを開けません".to_string(),
            );
            ctx.request_repaint();
        }
    }

    pub(crate) fn start_startup_open_path_resolve(
        &mut self,
        requested: PathBuf,
        source: StartupOpenPathSource,
        ctx: &egui::Context,
    ) {
        self.start_startup_open_path_resolution_owned_with_navigation(requested, source, ctx, None);
    }

    pub(crate) fn start_startup_open_path_resolution_owned_with_navigation(
        &mut self,
        requested: PathBuf,
        source: StartupOpenPathSource,
        ctx: &egui::Context,
        navigation: Option<MainListNavigation>,
    ) {
        let requested = crate::path_key::normalize_input_path(&requested);
        if self.remote_session_blocks_local_control() {
            if matches!(source, StartupOpenPathSource::InitialStartup) {
                self.startup_open_path = Some(requested);
                return;
            }
            crate::logger::log(format!(
                "startup open: rejected resolve while Remote owns local control source={} requested={}",
                source.perf_tag(),
                requested.display()
            ));
            if matches!(source, StartupOpenPathSource::Activation) {
                self.show_feedback_toast(
                    "リモート閲覧中のため、二重起動で渡されたパスを開けません".to_string(),
                );
            }
            return;
        }
        if self.composition_editor_modal_active() {
            crate::logger::log("composition editor: rejected path open");
            self.show_feedback_toast("本の構成を編集中のため、別の本を開けません".into());
            return;
        }
        if self.document_open_modal_admission_blocked() {
            match source {
                StartupOpenPathSource::Activation => {
                    #[cfg(windows)]
                    let request = crate::single_instance::ActivationOpenPath::received(
                        requested,
                        self.remote_session_handle(),
                    );
                    #[cfg(windows)]
                    let _ = self.activation_open_path_tx.send(request);
                    #[cfg(not(windows))]
                    let _ = requested;
                }
                StartupOpenPathSource::InitialStartup => self.startup_open_path = Some(requested),
                StartupOpenPathSource::Bookmark => {}
            }
            return;
        }
        let Some(owner) = self.startup_open_path_owner(source) else {
            crate::logger::log(format!(
                "startup open: reject ownerless resolve source={} requested={}",
                source.perf_tag(),
                requested.display()
            ));
            return;
        };
        let navigation = navigation.or_else(|| match &owner {
            StartupOpenPathOwner::Bookmark(bookmark) if self.main_folder_history_available() => {
                Some(self.capture_main_list_navigation(
                    MainHistoryOperation::Direct(crate::app::DirectNavigationPurpose::Navigation),
                    MainListSourceProof::Bookmark(bookmark.clone()),
                ))
            }
            _ => None,
        });
        if navigation
            .as_ref()
            .is_some_and(|navigation| !self.main_list_navigation_is_current(navigation))
        {
            return;
        }
        // The suffix is known before the filesystem resolver runs. A directory ending in
        // `.epub` holds this provisional lease only until classification returns Directory.
        let requested_owner = match crate::pdf_loader::LeasedEpubPath::try_new(requested.clone()) {
            Ok(owner) => owner,
            Err(reason) => {
                match source {
                    StartupOpenPathSource::Activation => {
                        #[cfg(windows)]
                        let request = crate::single_instance::ActivationOpenPath::received(
                            requested,
                            self.remote_session_handle(),
                        );
                        #[cfg(windows)]
                        let _ = self.activation_open_path_tx.send(request);
                        #[cfg(not(windows))]
                        let _ = requested;
                    }
                    StartupOpenPathSource::InitialStartup => {
                        self.startup_open_path = Some(requested)
                    }
                    StartupOpenPathSource::Bookmark => {
                        self.show_open_admission_refusal(reason.into());
                    }
                }
                return;
            }
        };
        let defer_activation_supersede =
            matches!(source, StartupOpenPathSource::Activation) && self.is_snapshot_active();
        // A forwarded activation is a new navigation request as soon as it is received. Do not
        // let an archive conversion owned by the currently opening bookmark win the race while
        // the activation path is still being resolved on its worker. Snapshot lock is the one
        // exception: scope admission can still refuse the request, so its supersede is deferred
        // and completion-side consumers hold the prior owner's result instead.
        if matches!(source, StartupOpenPathSource::Activation) && !defer_activation_supersede {
            self.cancel_archive_convert_for_navigation("activation_navigation");
            if let Some(request_id) = self
                .bookmark_open_pending
                .as_ref()
                .map(crate::bookmark_browser::PendingBookmarkOpen::request_id)
            {
                self.cancel_bookmark_open_request(request_id, "activation_navigation");
            }
        }
        let mut held_resolve_for_activation_admission = None;
        if let Some(mut pending) = self.startup_open_path_resolve_pending.take() {
            if defer_activation_supersede {
                if matches!(pending.owner, StartupOpenPathOwner::Activation) {
                    crate::logger::log(format!(
                        "startup open: replace pending activation resolve requested={}",
                        pending.requested.display()
                    ));
                    self.defer_current_bookmark_open_timeout_by(pending.elapsed());
                    held_resolve_for_activation_admission =
                        pending.held_resolve_for_activation_admission.take();
                    drop(pending);
                } else {
                    crate::logger::log(format!(
                        "startup open: hold pending resolve for activation admission source={} requested={}",
                        pending.owner.perf_tag(),
                        pending.requested.display()
                    ));
                    if let Some(watch) = pending.diagnostic.as_ref().and_then(|trace| trace.watch) {
                        watch.suspend();
                    }
                    held_resolve_for_activation_admission = Some(Box::new(pending));
                }
            } else {
                crate::logger::log(format!(
                    "startup open: cancel pending resolve source={} requested={}",
                    pending.owner.perf_tag(),
                    pending.requested.display()
                ));
                self.finish_replaced_startup_open_pending(pending, "resolver_replaced");
            }
        }

        let diagnostic = matches!(source, StartupOpenPathSource::InitialStartup)
            .then(|| InitialNavigationTrace::begin("initial-resolver-dispatch-to-ui-admission"));
        let trace_correlation = diagnostic.as_ref().map(InitialNavigationTrace::correlation);
        let trace_watch = diagnostic.as_ref().and_then(|trace| trace.watch);
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let worker_requested = requested.clone();
        let bookmark = matches!(owner, StartupOpenPathOwner::Bookmark(_))
            .then(|| {
                self.bookmark_open_pending
                    .as_ref()
                    .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)
                    .map(|pending| pending.bookmark.clone())
            })
            .flatten();
        let worker_bookmark = bookmark.clone();
        let repaint_ctx = ctx.clone();
        #[cfg(test)]
        let test_epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let worker = move || {
            #[cfg(test)]
            let _test_epoch_scope =
                test_epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
            if cancel_w.load(Ordering::Relaxed) {
                return;
            }
            let resolve_span = trace_correlation.map(|id| {
                event_span(Lane::Navigation, Stage::InitialTargetResolve, id)
                    .watched_optional(trace_watch)
            });
            let result =
                resolve_startup_open_path(worker_requested, source, worker_bookmark.as_ref(), true);
            if let Some(resolve_span) = resolve_span {
                resolve_span.finish(if cancel_w.load(Ordering::Relaxed) {
                    Outcome::Cancelled
                } else if result.resolved.is_some() {
                    Outcome::Ok
                } else {
                    Outcome::Error
                });
            }
            if cancel_w.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(result);
            repaint_ctx.request_repaint();
        };
        #[cfg(test)]
        let force_spawn_failure =
            FORCE_STARTUP_OPEN_RESOLVE_SPAWN_FAILURE.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let force_spawn_failure = false;
        let spawn_result = if force_spawn_failure {
            Err(std::io::Error::other(
                "forced startup path resolver spawn failure",
            ))
        } else {
            std::thread::Builder::new()
                .name("startup-open-resolve".to_string())
                .spawn(worker)
        };

        match spawn_result {
            Ok(_) => {
                self.startup_open_path_resolve_pending = Some(StartupOpenPathResolvePending {
                    requested: requested_owner,
                    owner,
                    cancel,
                    rx,
                    started_at: std::time::Instant::now(),
                    toast_shown: false,
                    held_resolve_for_activation_admission,
                    diagnostic,
                    navigation,
                });
            }
            Err(e) => {
                crate::logger::log(format!(
                    "startup open: resolve worker spawn failed: {e}; running synchronously"
                ));
                let resolve_started_at = std::time::Instant::now();
                let resolve_span = trace_correlation.map(|id| {
                    event_span(Lane::Navigation, Stage::InitialTargetResolve, id)
                        .watched_optional(trace_watch)
                });
                let result = resolve_startup_open_path(requested, source, bookmark.as_ref(), false);
                if let Some(resolve_span) = resolve_span {
                    resolve_span.finish(if result.resolved.is_some() {
                        Outcome::Ok
                    } else {
                        Outcome::Error
                    });
                }
                let held_duration = resolve_started_at.elapsed();
                self.finish_startup_open_path_resolve_with_held(
                    owner,
                    result,
                    held_resolve_for_activation_admission,
                    held_duration,
                    Some(requested_owner),
                    diagnostic,
                    navigation,
                    ctx,
                );
            }
        }
    }

    pub(super) fn poll_startup_open_path_resolve(&mut self, ctx: &egui::Context) {
        if self.remote_session_blocks_local_control() {
            self.cancel_unresolved_open_for_remote_session();
            return;
        }
        // This request was accepted before the restore modal started. Keep the receiver and its
        // owner intact so the ordinary completion tail runs exactly once after restore terminal;
        // taking it here would let `load_folder` replace the restore target mid-transaction.
        if self.sidecar_restore_active() || self.document_open_modal_admission_blocked() {
            if let Some(watch) = self
                .startup_open_path_resolve_pending
                .as_ref()
                .and_then(|pending| pending.diagnostic.as_ref())
                .and_then(|trace| trace.watch)
            {
                watch.suspend();
            }
            return;
        }
        if let Some(watch) = self
            .startup_open_path_resolve_pending
            .as_ref()
            .and_then(|pending| pending.diagnostic.as_ref())
            .and_then(|trace| trace.watch)
        {
            watch.resume();
        }
        let recv = match self.startup_open_path_resolve_pending.as_ref() {
            Some(pending) => pending.rx.try_recv(),
            None => return,
        };

        match recv {
            Ok(result) => {
                let mut pending = self.startup_open_path_resolve_pending.take().unwrap();
                let owner = pending.owner.clone();
                let held = pending.held_resolve_for_activation_admission.take();
                let held_duration = pending.elapsed();
                let requested_owner = pending.requested.clone();
                let diagnostic = pending.diagnostic.take();
                let navigation = pending.navigation.take();
                drop(pending);
                self.finish_startup_open_path_resolve_with_held(
                    owner,
                    result,
                    held,
                    held_duration,
                    Some(requested_owner),
                    diagnostic,
                    navigation,
                    ctx,
                );
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => {
                let Some(pending) = self.startup_open_path_resolve_pending.as_mut() else {
                    return;
                };
                let elapsed = pending.elapsed();
                if !pending.toast_shown && elapsed >= STARTUP_OPEN_PATH_RESOLVE_TOAST_DELAY {
                    pending.toast_shown = true;
                    self.show_feedback_toast("パスを確認しています…".to_string());
                    ctx.request_repaint();
                } else if !pending.toast_shown {
                    let remaining = STARTUP_OPEN_PATH_RESOLVE_TOAST_DELAY
                        .saturating_sub(elapsed)
                        .max(std::time::Duration::from_millis(16));
                    ctx.request_repaint_after(remaining);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let mut pending = self.startup_open_path_resolve_pending.take().unwrap();
                let owner = pending.owner.clone();
                let held = pending.held_resolve_for_activation_admission.take();
                let held_duration = pending.elapsed();
                if let Some(diagnostic) = pending.diagnostic.take() {
                    diagnostic.finish(Outcome::Error);
                }
                crate::logger::log(format!(
                    "startup open: resolve worker disconnected source={} requested={}",
                    owner.perf_tag(),
                    pending.requested.display()
                ));
                drop(pending);
                let source = owner.source();
                if matches!(source, StartupOpenPathSource::InitialStartup) {
                    self.open_default_startup_target();
                } else {
                    if let StartupOpenPathOwner::Bookmark(owner) = owner {
                        self.cancel_bookmark_open_request(owner.request_id, "resolve_disconnected");
                    } else if matches!(source, StartupOpenPathSource::Activation) {
                        self.resume_activation_held_open(held, held_duration);
                    }
                    self.show_feedback_toast("パスの確認を完了できませんでした".to_string());
                }
                ctx.request_repaint();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn finish_startup_open_path_resolve(
        &mut self,
        owner: StartupOpenPathOwner,
        result: StartupOpenPathResolveResult,
        ctx: &egui::Context,
    ) {
        self.finish_startup_open_path_resolve_with_held(
            owner,
            result,
            None,
            std::time::Duration::ZERO,
            None,
            None,
            None,
            ctx,
        );
    }

    fn finish_startup_open_path_resolve_with_held(
        &mut self,
        owner: StartupOpenPathOwner,
        result: StartupOpenPathResolveResult,
        held: Option<Box<StartupOpenPathResolvePending>>,
        held_duration: std::time::Duration,
        requested_owner: Option<crate::pdf_loader::LeasedEpubPath>,
        diagnostic: Option<InitialNavigationTrace>,
        navigation: Option<MainListNavigation>,
        ctx: &egui::Context,
    ) {
        if !self.startup_open_path_owner_is_current(&owner) {
            crate::logger::log(format!(
                "startup open: discard stale completion source={} requested={}",
                owner.perf_tag(),
                result.requested.display()
            ));
            return;
        }
        #[cfg(windows)]
        let detached_route = match &owner {
            StartupOpenPathOwner::Bookmark(bookmark_owner) => bookmark_owner
                .detached_lease
                .map(|lease| (lease.window_id, bookmark_owner.request_id)),
            _ => None,
        };
        #[cfg(windows)]
        if let Some((window_id, request_id)) = detached_route
            && self.detached_viewer_window_id() != Some(window_id)
        {
            let routed = self.with_window_viewer_context(window_id, |app| {
                app.finish_startup_open_path_resolve_with_held(
                    owner,
                    result,
                    held,
                    held_duration,
                    requested_owner,
                    diagnostic,
                    navigation,
                    ctx,
                );
            });
            if let Err(error) = routed {
                crate::logger::log(format!(
                    "startup open: detached owner mount failed window_id={window_id} error={error:?}"
                ));
                self.cancel_bookmark_open_request(request_id, "detached_resolve_owner_missing");
            }
            return;
        }
        let source = owner.source();
        if let Some(message) = result
            .rar_volume_proof
            .as_ref()
            .and_then(crate::rar_loader::RarVolumeProof::rejection_message)
        {
            // A volume refusal has no admission authority over a held first-volume request.
            if matches!(source, StartupOpenPathSource::Activation) {
                self.resume_activation_held_open(held, held_duration);
            }
            if let StartupOpenPathOwner::Bookmark(bookmark_owner) = &owner {
                self.cancel_bookmark_open_request(
                    bookmark_owner.request_id,
                    "rar_subsequent_volume",
                );
            }
            if let Some(diagnostic) = diagnostic {
                diagnostic.finish(Outcome::Error);
            }
            if matches!(source, StartupOpenPathSource::InitialStartup) {
                self.open_default_startup_target();
            }
            self.show_feedback_toast(message);
            return;
        }
        if matches!(source, StartupOpenPathSource::Activation) {
            match result.resolved.as_ref() {
                None => self.resume_activation_held_open(held, held_duration),
                Some(resolution)
                    if !self.snapshot_scope_allows_open(
                        &resolution.path,
                        &owner.open_request_owner(),
                    ) =>
                {
                    crate::logger::log(format!(
                        "startup open: activation refused before supersede requested={} resolved={}",
                        result.requested.display(),
                        resolution.path.display()
                    ));
                    self.resume_activation_held_open(held, held_duration);
                    self.show_snapshot_out_of_scope_open_feedback();
                    return;
                }
                Some(_) => {}
            }
        }
        if let StartupOpenPathOwner::Bookmark(bookmark_owner) = &owner
            && result.bookmark_relative_page_openable == Some(false)
        {
            crate::logger::log(
                "[bookmark-open] relative page rejected by usage-time containment check",
            );
            self.cancel_bookmark_open_request(
                bookmark_owner.request_id,
                "relative_page_missing_or_unsafe",
            );
            self.show_feedback_toast(
                "ブックマーク先のページが見つかりません（記録は保持されます）".to_string(),
            );
            return;
        }
        let requested_display = result.requested.display().to_string();
        let correlation = diagnostic.as_ref().map(InitialNavigationTrace::correlation);
        let watch = diagnostic.as_ref().and_then(|trace| trace.watch);
        let adoption = correlation.map(|id| {
            event_span(Lane::Navigation, Stage::InitialTargetAdopt, id)
                .watched_optional(watch)
                .detail("resolver-result-ui-admission")
        });
        let outcome = self.apply_startup_open_path_resolve_result(
            &owner,
            result,
            ctx,
            correlation,
            watch,
            navigation,
        );
        let trace_outcome = if matches!(outcome, StartupOpenApplyOutcome::Opened) {
            Outcome::Ok
        } else {
            Outcome::Error
        };
        if let Some(adoption) = adoption {
            adoption.finish(trace_outcome);
        }
        if let Some(diagnostic) = diagnostic {
            diagnostic.finish(trace_outcome);
        }
        match outcome {
            StartupOpenApplyOutcome::Opened => return,
            StartupOpenApplyOutcome::Refused(reason) => {
                if let StartupOpenPathOwner::Bookmark(bookmark_owner) = &owner {
                    self.cancel_bookmark_open_request(bookmark_owner.request_id, "epub_ignored");
                }
                crate::logger::log(format!(
                    "startup open: refused source={} requested={} reason={reason:?}",
                    source.perf_tag(),
                    requested_display
                ));
                if matches!(source, StartupOpenPathSource::InitialStartup) {
                    self.open_default_startup_target();
                }
                return;
            }
            #[cfg(windows)]
            StartupOpenApplyOutcome::DetachedRefused(reason) => {
                if let StartupOpenPathOwner::Bookmark(bookmark_owner) = &owner {
                    self.cancel_bookmark_open_request(bookmark_owner.request_id, "epub_deleting");
                }
                self.show_open_admission_refusal(reason);
                return;
            }
            StartupOpenApplyOutcome::NotOpenable => {}
        }
        if matches!(source, StartupOpenPathSource::InitialStartup) {
            self.open_default_startup_target();
        } else {
            if let StartupOpenPathOwner::Bookmark(bookmark_owner) = owner {
                self.cancel_bookmark_open_request(bookmark_owner.request_id, "not_openable");
            }
            crate::logger::log(format!(
                "startup open: activation open failed for {requested_display}"
            ));
            self.show_feedback_toast("開けるパスが見つかりませんでした".to_string());
        }
    }

    /// An unresolved Activation is the typed completion gate for the request it may supersede.
    /// The common no-snapshot path still cancels immediately, so this predicate adds no delay
    /// there; it only keeps deferred snapshot owners from landing before admission is known.
    pub(crate) fn activation_open_resolve_holds_prior_completion(&self) -> bool {
        self.startup_open_path_resolve_pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.owner, StartupOpenPathOwner::Activation))
    }

    fn resume_activation_held_open(
        &mut self,
        held: Option<Box<StartupOpenPathResolvePending>>,
        held_duration: std::time::Duration,
    ) {
        self.defer_current_bookmark_open_timeout_by(held_duration);
        let Some(held) = held else {
            return;
        };
        if let Some(watch) = held.diagnostic.as_ref().and_then(|trace| trace.watch) {
            watch.resume();
        }
        crate::logger::log(format!(
            "startup open: resume held resolve after activation refusal source={} requested={}",
            held.owner.perf_tag(),
            held.requested.display()
        ));
        debug_assert!(self.startup_open_path_resolve_pending.is_none());
        self.startup_open_path_resolve_pending = Some(*held);
    }

    fn defer_current_bookmark_open_timeout_by(&mut self, duration: std::time::Duration) {
        let Some(pending) = self.bookmark_open_pending.as_mut() else {
            return;
        };
        match pending {
            crate::bookmark_browser::PendingBookmarkOpen::Media(pending) => {
                if let Some(resumed_at) = pending.started_at.checked_add(duration) {
                    pending.started_at = resumed_at;
                }
            }
            crate::bookmark_browser::PendingBookmarkOpen::Book(pending) => {
                if let Some(resumed_at) = pending.started_at.checked_add(duration) {
                    pending.started_at = resumed_at;
                }
                if let crate::bookmark_browser::PendingBookOpenStage::AwaitingPage {
                    started_at,
                    ..
                } = &mut pending.stage
                    && let Some(resumed_at) = started_at.checked_add(duration)
                {
                    *started_at = resumed_at;
                }
            }
        }
    }

    pub(super) fn startup_open_path_owner(
        &self,
        source: StartupOpenPathSource,
    ) -> Option<StartupOpenPathOwner> {
        match source {
            StartupOpenPathSource::InitialStartup => Some(StartupOpenPathOwner::InitialStartup),
            StartupOpenPathSource::Activation => Some(StartupOpenPathOwner::Activation),
            StartupOpenPathSource::Bookmark => {
                let request_id = self.bookmark_open_pending.as_ref()?.request_id();
                let target = self.bookmark_view_target()?.clone();
                Some(StartupOpenPathOwner::Bookmark(
                    crate::bookmark_browser::BookmarkOpenRequestOwner {
                        request_id,
                        target,
                        #[cfg(windows)]
                        detached_lease: self
                            .detached_viewer_window_id()
                            .map(Self::detached_session_lease),
                    },
                ))
            }
        }
    }

    fn startup_open_path_owner_is_current(&self, owner: &StartupOpenPathOwner) -> bool {
        let StartupOpenPathOwner::Bookmark(bookmark_owner) = owner else {
            return true;
        };
        self.bookmark_open_owner_is_current(bookmark_owner)
    }

    pub(crate) fn bookmark_open_owner_is_current(
        &self,
        owner: &crate::bookmark_browser::BookmarkOpenRequestOwner,
    ) -> bool {
        let projected_owner_matches = self
            .bookmark_open_pending
            .as_ref()
            .is_some_and(|pending| pending.request_id() == owner.request_id)
            && self.bookmark_view_target() == Some(&owner.target);
        if projected_owner_matches {
            return true;
        }
        #[cfg(windows)]
        {
            owner.detached_lease.is_some_and(|lease| {
                self.bookmark_open_request_seq == owner.request_id.0
                    && self.active_detached_window_id() == Some(lease.window_id)
                    && self.active_detached_session.is_some_and(|session| {
                        session.window_id == lease.window_id
                            && session.content_phase == DetachedSessionContentPhase::Preparing
                    })
                    && self.bookmark_view_target() == Some(&owner.target)
            })
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    pub(crate) fn begin_bookmark_page_wait(
        &mut self,
        owner: &crate::bookmark_browser::BookmarkOpenRequestOwner,
    ) -> bool {
        if !self.bookmark_open_owner_is_current(owner) {
            return false;
        }
        let Some(pending) = self
            .bookmark_open_pending
            .as_mut()
            .and_then(crate::bookmark_browser::PendingBookmarkOpen::book_mut)
        else {
            return false;
        };
        pending.begin_page_wait();
        true
    }

    fn finish_replaced_startup_open_owner(
        &mut self,
        owner: StartupOpenPathOwner,
        reason: &'static str,
    ) {
        if let StartupOpenPathOwner::Bookmark(bookmark_owner) = owner {
            self.cancel_bookmark_open_request(bookmark_owner.request_id, reason);
        }
    }

    fn finish_replaced_startup_open_pending(
        &mut self,
        mut pending: StartupOpenPathResolvePending,
        reason: &'static str,
    ) {
        let held = pending.held_resolve_for_activation_admission.take();
        let owner = pending.owner.clone();
        if let Some(diagnostic) = pending.diagnostic.take() {
            diagnostic.finish(if reason == "remote_session_acquired" {
                Outcome::SuspendedNotWatched
            } else {
                Outcome::Cancelled
            });
        }
        drop(pending);
        self.finish_replaced_startup_open_owner(owner, reason);
        if let Some(held) = held {
            self.finish_replaced_startup_open_pending(*held, reason);
        }
    }

    /// A normal navigation supersedes an unresolved startup/activation/bookmark open. The
    /// resolver is removed before its receiver can be polled; bookmark-owned state is cleared
    /// only when its request ID is still current.
    pub(crate) fn cancel_unresolved_open_for_navigation(&mut self) {
        self.cancel_unresolved_open("normal_navigation");
    }

    pub(crate) fn cancel_unresolved_open_for_remote_session(&mut self) {
        if let Some(watch) = miv_startup::watch_handle(WatchSlot::InitialDispatch) {
            watch.retire(Outcome::SuspendedNotWatched);
        }
        // The worker result is stale after Remote acquires, but the explicit command line
        // target must remain pending until Local owns the viewer again.
        let mut held = self.startup_open_path_resolve_pending.as_ref();
        while let Some(pending) = held {
            if matches!(pending.owner, StartupOpenPathOwner::InitialStartup) {
                self.startup_open_path
                    .get_or_insert_with(|| pending.requested.to_path_buf());
                break;
            }
            held = pending.held_resolve_for_activation_admission.as_deref();
        }
        self.cancel_unresolved_open("remote_session_acquired");
    }

    fn cancel_unresolved_open(&mut self, reason: &'static str) {
        let Some(pending) = self.startup_open_path_resolve_pending.take() else {
            return;
        };
        crate::logger::log(format!(
            "startup open: cancel resolve reason={reason} source={} requested={}",
            pending.owner.perf_tag(),
            pending.requested.display()
        ));
        self.finish_replaced_startup_open_pending(pending, reason);
    }

    /// A main surface switch owns only its mounted bookmark request, never a reader lease.
    pub(super) fn retire_main_bookmark_open_for_surface_switch(&mut self) {
        let request_id = match self
            .startup_open_path_resolve_pending
            .as_ref()
            .map(|pending| &pending.owner)
        {
            Some(StartupOpenPathOwner::Bookmark(owner)) => {
                #[cfg(windows)]
                if owner.detached_lease.is_some() {
                    return;
                }
                Some(owner.request_id)
            }
            // AwaitingPage and media waits belong to the already mounted viewer. A Current
            // reselect keeps that reader and its return target, just like sidecar hydration.
            _ => self
                .bookmark_open_pending
                .as_ref()
                .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)
                .filter(|pending| {
                    matches!(
                        pending.stage,
                        crate::bookmark_browser::PendingBookOpenStage::Resolving
                            | crate::bookmark_browser::PendingBookOpenStage::Qualifying(_)
                    )
                })
                .map(|pending| pending.request_id),
        };
        if let Some(request_id) = request_id {
            self.cancel_bookmark_open_request(request_id, "main_surface_switch");
        }
    }

    /// Once resolution has completed, page enumeration/player setup may still be pending. A
    /// navigation to another container supersedes that remainder of the request as well.
    pub(crate) fn cancel_conflicting_bookmark_open_for_navigation(&mut self, path: &Path) {
        let Some(request_id) = self
            .bookmark_open_pending
            .as_ref()
            .map(crate::bookmark_browser::PendingBookmarkOpen::request_id)
        else {
            return;
        };
        let conflicts = self
            .bookmark_view_target()
            .is_none_or(|target| !target.matches_loaded_container(path));
        if conflicts {
            self.cancel_bookmark_open_request(request_id, "conflicting_navigation");
        }
    }

    fn cancel_staged_bookmark_navigation(
        &mut self,
        bookmark_id: crate::bookmark_browser::BookmarkOpenRequestId,
        reason: &'static str,
    ) -> bool {
        let request_id = match self.top_level_grid_view.history_navigation_transition() {
            Some(HistoryNavigationTransition::Physical(request))
                if matches!(&request.intent, PhysicalHistoryIntent::Bookmark { owner, .. }
                    if owner.request_id == bookmark_id) =>
            {
                Some(request.request_id)
            }
            _ => None,
        };
        let Some(request_id) = request_id else {
            return false;
        };
        if self.staged_archive_conversion_is_current(request_id) {
            self.cancel_archive_convert_for_navigation(reason);
        }
        if self.pdf_password_request.as_ref().is_some_and(|request| {
            matches!(request.owner, PdfPasswordRequestOwner::StagedHistory(id)
                if id == request_id)
        }) {
            self.pdf_password_request = None;
            self.pdf_password_input.clear();
            self.pdf_password_error = None;
        }
        if matches!(self.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request)) if request.request_id == request_id)
        {
            self.replace_history_navigation_transition(None);
        }
        true
    }

    /// End every current lifecycle component belonging to exactly one bookmark request.
    /// A stale A cancellation is therefore unable to clear a newer B request.
    pub(crate) fn cancel_bookmark_open_request(
        &mut self,
        request_id: crate::bookmark_browser::BookmarkOpenRequestId,
        reason: &'static str,
    ) -> bool {
        #[cfg(windows)]
        let detached_lease = self
            .startup_open_path_resolve_pending
            .as_ref()
            .and_then(|pending| match &pending.owner {
                StartupOpenPathOwner::Bookmark(owner) if owner.request_id == request_id => {
                    owner.detached_lease
                }
                _ => pending
                    .held_resolve_for_activation_admission
                    .as_deref()
                    .and_then(|held| match &held.owner {
                        StartupOpenPathOwner::Bookmark(owner) if owner.request_id == request_id => {
                            owner.detached_lease
                        }
                        _ => None,
                    }),
            })
            .or_else(|| {
                self.archive_convert
                    .as_ref()
                    .and_then(|state| state.completion.bookmark_owner())
                    .filter(|owner| owner.request_id == request_id)
                    .and_then(|owner| owner.detached_lease)
            })
            .or_else(|| {
                (self.bookmark_open_request_seq == request_id.0)
                    .then_some(self.active_detached_session)
                    .flatten()
                    .filter(|session| {
                        session.content_phase == DetachedSessionContentPhase::Preparing
                    })
                    .map(|session| Self::detached_session_lease(session.window_id))
            });
        #[cfg(windows)]
        if let Some(lease) = detached_lease
            && self.detached_viewer_window_id() != Some(lease.window_id)
        {
            return self
                .with_window_viewer_context(lease.window_id, |app| {
                    app.cancel_bookmark_open_request(request_id, reason)
                })
                .unwrap_or(false);
        }
        let resolver_matches =
            self.startup_open_path_resolve_pending
                .as_ref()
                .is_some_and(|pending| {
                    matches!(
                        pending.owner,
                        StartupOpenPathOwner::Bookmark(
                            crate::bookmark_browser::BookmarkOpenRequestOwner {
                                request_id: current,
                                ..
                            }
                        ) if current == request_id
                    )
                });
        let held_resolver_matches = self
            .startup_open_path_resolve_pending
            .as_ref()
            .and_then(|pending| pending.held_resolve_for_activation_admission.as_deref())
            .is_some_and(|held| {
                matches!(
                    held.owner,
                    StartupOpenPathOwner::Bookmark(
                        crate::bookmark_browser::BookmarkOpenRequestOwner {
                            request_id: current,
                            ..
                        }
                    ) if current == request_id
                )
            });
        if resolver_matches {
            drop(self.startup_open_path_resolve_pending.take());
        } else if held_resolver_matches {
            drop(
                self.startup_open_path_resolve_pending
                    .as_mut()
                    .and_then(|pending| pending.held_resolve_for_activation_admission.take()),
            );
        }
        let staged_matches = self.cancel_staged_bookmark_navigation(request_id, reason);
        let archive_matches = self.cancel_archive_convert_for_bookmark_request(request_id);
        let pending_matches = self
            .bookmark_open_pending
            .as_ref()
            .is_some_and(|pending| pending.request_id() == request_id);
        let owned = resolver_matches
            || held_resolver_matches
            || staged_matches
            || archive_matches
            || pending_matches;
        if pending_matches {
            self.retire_book_identity_permission(request_id.0);
            crate::logger::log(format!(
                "[bookmark-open] finish request id={} reason={reason}",
                request_id.0
            ));
            self.bookmark_open_pending = None;
            #[cfg(windows)]
            let retain_detached_target_for_close = detached_lease.is_some_and(|lease| {
                self.detached_viewer_window_id() == Some(lease.window_id)
                    && matches!(
                        self.bookmark_view_state,
                        Some(BookmarkViewState::Detached { .. })
                    )
            });
            #[cfg(not(windows))]
            let retain_detached_target_for_close = false;
            if !retain_detached_target_for_close {
                self.clear_bookmark_view_return_state();
            }
        }
        #[cfg(windows)]
        if owned
            && detached_lease.is_some_and(|lease| {
                self.detached_viewer_window_id() == Some(lease.window_id)
                    && self.active_detached_session.is_some_and(|session| {
                        session.window_id == lease.window_id
                            && session.content_phase == DetachedSessionContentPhase::Preparing
                    })
            })
        {
            self.begin_active_detached_session_close(reason);
        }
        owned
    }

    fn apply_startup_open_path_resolve_result(
        &mut self,
        owner: &StartupOpenPathOwner,
        result: StartupOpenPathResolveResult,
        ctx: &egui::Context,
        trace_correlation: Option<u64>,
        trace_watch: Option<WatchHandle>,
        navigation: Option<MainListNavigation>,
    ) -> StartupOpenApplyOutcome {
        if navigation
            .as_ref()
            .is_some_and(|navigation| !self.main_list_navigation_is_current(navigation))
        {
            return StartupOpenApplyOutcome::NotOpenable;
        }
        let source = owner.source();
        let Some(resolution) = result.resolved else {
            crate::logger::log(format!(
                "startup open: no openable path for {}",
                result.requested.display()
            ));
            return StartupOpenApplyOutcome::NotOpenable;
        };

        let openable = resolution.path;
        if matches!(resolution.kind, crate::folder_tree::OpenablePathKind::File)
            && matches!(owner, StartupOpenPathOwner::Bookmark(_))
            && let Some(reason) = self.pdf_open_refusal(&openable)
        {
            self.show_pdf_open_refusal(reason);
            return StartupOpenApplyOutcome::Refused(reason);
        }
        if matches!(resolution.kind, crate::folder_tree::OpenablePathKind::File)
            && crate::rar_loader::is_rar_path(&openable)
            && result.rar_volume_proof.is_none()
        {
            // The resolver's UI-side spawn-failure fallback never probes a RAR header. Keep
            // the same request owner and let existing classification prove it asynchronously
            // before any cache or detached descriptor can adopt it.
            let auto_fullscreen = matches!(source, StartupOpenPathSource::Bookmark)
                || startup_openable_should_auto_fullscreen(
                    &self.settings,
                    &openable,
                    resolution.kind,
                );
            return match self
                .load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation(
                    openable,
                    auto_fullscreen,
                    owner.open_request_owner(),
                    None,
                    None,
                    super::StartupListIntent::container_open(auto_fullscreen),
                    navigation,
                ) {
                FolderOpenOutcome::Loaded
                | FolderOpenOutcome::Classifying
                | FolderOpenOutcome::ConversionDialogOpened => StartupOpenApplyOutcome::Opened,
                FolderOpenOutcome::Ignored => StartupOpenApplyOutcome::NotOpenable,
                FolderOpenOutcome::Refused(reason) => StartupOpenApplyOutcome::Refused(reason),
            };
        }
        crate::logger::log(format!(
            "startup open: requested={} resolved={} resolve_ms={:.1}",
            result.requested.display(),
            openable.display(),
            result.elapsed_ms
        ));

        self.capture_main_list_restore_cursor();
        let select_requested_file = resolution.requested_is_file
            && matches!(
                resolution.kind,
                crate::folder_tree::OpenablePathKind::Directory
            );
        #[cfg(windows)]
        if matches!(source, StartupOpenPathSource::Bookmark)
            && self.navigation_scope.is_detached_physical()
            && self.active_detached_session.is_some_and(|session| {
                session.content_phase == DetachedSessionContentPhase::Preparing
            })
        {
            if self
                .bookmark_open_pending
                .as_ref()
                .and_then(crate::bookmark_browser::PendingBookmarkOpen::media)
                .is_some()
            {
                let outcome = self.load_folder_or_convert_archive_with_auto_fullscreen_owned(
                    openable,
                    true,
                    owner.open_request_owner(),
                    super::StartupListIntent::PageContinuation,
                );
                if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
                    self.open_startup_file_if_visible(&result.requested);
                }
                return match outcome {
                    FolderOpenOutcome::Loaded
                    | FolderOpenOutcome::Classifying
                    | FolderOpenOutcome::ConversionDialogOpened => StartupOpenApplyOutcome::Opened,
                    FolderOpenOutcome::Ignored => StartupOpenApplyOutcome::NotOpenable,
                    FolderOpenOutcome::Refused(reason) => StartupOpenApplyOutcome::Refused(reason),
                };
            }
            match self.bookmark_detached_descriptor(
                &openable,
                resolution.kind,
                result.rar_volume_proof.as_ref(),
            ) {
                DetachedBuild::Built(descriptor) => {
                    return if self.continue_active_detached_book_context_from_descriptor(descriptor)
                    {
                        StartupOpenApplyOutcome::Opened
                    } else {
                        StartupOpenApplyOutcome::NotOpenable
                    };
                }
                DetachedBuild::Refused(reason) => {
                    return StartupOpenApplyOutcome::DetachedRefused(reason);
                }
                DetachedBuild::NotApplicable => {}
            }
            // OtherArchive without a cache remains in this same loading context.  The ordinary
            // owned loader below starts probe/conversion and carries the bookmark's window lease
            // through its completion policy.
        }
        #[cfg(windows)]
        if matches!(source, StartupOpenPathSource::Bookmark)
            && self.settings.effective_media_in_media_window()
            && self
                .bookmark_open_pending
                .as_ref()
                .and_then(crate::bookmark_browser::PendingBookmarkOpen::media)
                .is_some()
            && matches!(
                self.bookmark_view_state,
                Some(BookmarkViewState::Opening {
                    target: crate::bookmark_browser::BookmarkViewReturnTarget::Media(_),
                    ..
                })
            )
        {
            return if self.open_bookmark_media_in_detached_context(
                ctx,
                &result.requested,
                openable,
                select_requested_file,
            ) {
                StartupOpenApplyOutcome::Opened
            } else {
                StartupOpenApplyOutcome::NotOpenable
            };
        }
        #[cfg(windows)]
        if matches!(source, StartupOpenPathSource::Bookmark)
            && self.settings.detached_viewer_open_images_in_window
        {
            match self.open_bookmark_book_in_detached_context_classified(
                ctx,
                openable.clone(),
                resolution.kind,
                result.rar_volume_proof.as_ref(),
            ) {
                DetachedBuild::Built(true) => return StartupOpenApplyOutcome::Opened,
                DetachedBuild::Built(false) => return StartupOpenApplyOutcome::NotOpenable,
                DetachedBuild::Refused(reason) => {
                    return StartupOpenApplyOutcome::DetachedRefused(reason);
                }
                DetachedBuild::NotApplicable => {}
            }
        }
        #[cfg(windows)]
        if matches!(source, StartupOpenPathSource::Bookmark)
            && !self.settings.detached_viewer_open_images_in_window
            && self
                .bookmark_open_pending
                .as_ref()
                .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)
                .is_some()
            && !self.park_detached_media_before_fullfeature_bookmark_book_open(ctx)
        {
            return StartupOpenApplyOutcome::NotOpenable;
        }
        let auto_fullscreen = matches!(source, StartupOpenPathSource::Bookmark)
            || startup_openable_should_auto_fullscreen(&self.settings, &openable, resolution.kind);
        let scan = trace_correlation.map(|id| {
            event_span(Lane::Navigation, Stage::InitialTargetScan, id)
                .watched_optional(trace_watch)
                .detail("existing-loader-call-return")
        });
        let classified_kind = Some(match resolution.kind {
            crate::folder_tree::OpenablePathKind::Directory => OpenPathKind::Directory,
            crate::folder_tree::OpenablePathKind::File => OpenPathKind::File,
        });
        let outcome = self
            .load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation(
                openable,
                auto_fullscreen,
                owner.open_request_owner(),
                classified_kind,
                None,
                if select_requested_file {
                    super::StartupListIntent::PageContinuation
                } else {
                    super::StartupListIntent::container_open(auto_fullscreen)
                },
                navigation,
            );
        if let Some(scan) = scan {
            scan.finish(
                if matches!(
                    outcome,
                    FolderOpenOutcome::Ignored | FolderOpenOutcome::Refused(_)
                ) {
                    Outcome::Error
                } else {
                    Outcome::Ok
                },
            );
        }
        match outcome {
            FolderOpenOutcome::Ignored => return StartupOpenApplyOutcome::NotOpenable,
            FolderOpenOutcome::Refused(reason) => {
                return StartupOpenApplyOutcome::Refused(reason);
            }
            FolderOpenOutcome::Loaded
            | FolderOpenOutcome::Classifying
            | FolderOpenOutcome::ConversionDialogOpened => {}
        }
        if matches!(source, StartupOpenPathSource::Bookmark)
            && !self.main_folder_history_available()
            && matches!(outcome, FolderOpenOutcome::Loaded)
            && let StartupOpenPathOwner::Bookmark(bookmark_owner) = owner
        {
            self.begin_bookmark_page_wait(bookmark_owner);
        }
        if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
            if let Some(prepared) = result.direct_media
                && !self
                    .settings
                    .file_type_visibility
                    .allows_physical_file(&result.requested)
            {
                let action = MainListAfterHydration::DirectMedia(prepared);
                let action = match self.sidecar_restore.as_mut() {
                    Some(state) => state.defer_main_list_action(action).err(),
                    None => Some(action),
                };
                if let Some(action) = action {
                    self.finish_main_list_after_hydration(action);
                }
            } else {
                self.open_startup_file_if_visible(&result.requested);
            }
        }
        StartupOpenApplyOutcome::Opened
    }

    /// Route a bookmark-backed book into the same independent context seam used by normal
    /// PDF/ZIP grid opens. An inapplicable container still needs archive conversion;
    /// EPUB admission refusal is terminal and must not fall through to that route.
    #[cfg(windows)]
    fn bookmark_detached_descriptor(
        &mut self,
        openable: &Path,
        kind: crate::folder_tree::OpenablePathKind,
        rar_volume_proof: Option<&crate::rar_loader::RarVolumeProof>,
    ) -> DetachedBuild<ViewerContextDescriptor> {
        let Some(pending) = self
            .bookmark_open_pending
            .as_ref()
            .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)
        else {
            return DetachedBuild::NotApplicable;
        };
        match pending.bookmark.container_kind {
            crate::book_bookmarks::BookContainerKind::Pdf
                if matches!(kind, crate::folder_tree::OpenablePathKind::File) =>
            {
                Self::detached_pdf_descriptor(openable, None)
            }
            crate::book_bookmarks::BookContainerKind::Zip
                if matches!(kind, crate::folder_tree::OpenablePathKind::File) =>
            {
                DetachedBuild::Built(ViewerContextDescriptor::Zip {
                    path: openable.to_path_buf(),
                    entry_name: None,
                    archive_source_override: None,
                })
            }
            crate::book_bookmarks::BookContainerKind::CompiledBook
            | crate::book_bookmarks::BookContainerKind::ImageFolder
                if matches!(kind, crate::folder_tree::OpenablePathKind::Directory) =>
            {
                DetachedBuild::Built(ViewerContextDescriptor::BookFolder {
                    path: openable.to_path_buf(),
                })
            }
            crate::book_bookmarks::BookContainerKind::OtherArchive => {
                let source = pending.bookmark.container_path.clone();
                if crate::rar_loader::is_rar_path(&source)
                    && (rar_volume_proof.is_none()
                        || rar_volume_proof
                            .is_some_and(|proof| proof.rejection_message().is_some()))
                {
                    return DetachedBuild::NotApplicable;
                }
                let Some(cached) = self.try_archive_cache_lookup(&source) else {
                    return DetachedBuild::NotApplicable;
                };
                DetachedBuild::Built(ViewerContextDescriptor::Zip {
                    path: cached,
                    entry_name: None,
                    archive_source_override: Some(source),
                })
            }
            _ => DetachedBuild::NotApplicable,
        }
    }

    #[cfg(windows)]
    pub(super) fn open_bookmark_book_in_detached_context(
        &mut self,
        ctx: &egui::Context,
        openable: PathBuf,
        kind: crate::folder_tree::OpenablePathKind,
    ) -> DetachedBuild<bool> {
        self.open_bookmark_book_in_detached_context_classified(ctx, openable, kind, None)
    }

    #[cfg(windows)]
    fn open_bookmark_book_in_detached_context_classified(
        &mut self,
        ctx: &egui::Context,
        openable: PathBuf,
        kind: crate::folder_tree::OpenablePathKind,
        rar_volume_proof: Option<&crate::rar_loader::RarVolumeProof>,
    ) -> DetachedBuild<bool> {
        let Some(pending) = self
            .bookmark_open_pending
            .as_ref()
            .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)
        else {
            return DetachedBuild::NotApplicable;
        };
        let target_matches = matches!(
            self.bookmark_view_state.as_ref(),
            Some(BookmarkViewState::Opening {
                target: crate::bookmark_browser::BookmarkViewReturnTarget::Book(path),
                ..
            }) if crate::path_key::eq_keep_drive(path, &pending.bookmark.container_path)
        );
        if !target_matches {
            return DetachedBuild::NotApplicable;
        }

        let request_id = pending.request_id;
        let descriptor = match self.bookmark_detached_descriptor(&openable, kind, rar_volume_proof)
        {
            DetachedBuild::Built(descriptor) => descriptor,
            DetachedBuild::NotApplicable => return DetachedBuild::NotApplicable,
            DetachedBuild::Refused(reason) => return DetachedBuild::Refused(reason),
        };
        if let ViewerContextDescriptor::Pdf { path, .. } = &descriptor
            && let Some(reason) = self.pdf_open_refusal(path)
        {
            self.show_pdf_open_refusal(reason);
            self.cancel_bookmark_open_request(request_id, "epub_ignored");
            return DetachedBuild::Built(false);
        }

        let pending = match self.bookmark_open_pending.take() {
            Some(crate::bookmark_browser::PendingBookmarkOpen::Book(pending)) => pending,
            other => {
                self.bookmark_open_pending = other;
                return DetachedBuild::NotApplicable;
            }
        };
        let base_placement = self.active_detached_viewer_current_placement();
        let had_active_detached = self.active_detached_context_exists();
        if !self.park_and_close_current_active_detached_viewer(ctx) {
            self.bookmark_open_pending =
                Some(crate::bookmark_browser::PendingBookmarkOpen::Book(pending));
            return DetachedBuild::Built(false);
        }
        let placement_seed = had_active_detached
            .then(|| self.offset_detached_image_window_placement(base_placement));
        DetachedBuild::Built(self.start_active_detached_book_context(
            descriptor,
            ctx,
            placement_seed,
            Some(pending),
        ))
    }

    /// Full-feature book opens continue through the mounted/main context so editing and linked
    /// viewer semantics stay unchanged. If a detached media context currently owns the active
    /// session, park that media first through the standard `ParkedLive` handoff. Loading the book
    /// while the media context remains active lets both paths compete for the single mounted
    /// `active_detached_session`, which can re-show the media window and suppress the book open.
    #[cfg(windows)]
    pub(super) fn park_detached_media_before_fullfeature_bookmark_book_open(
        &mut self,
        ctx: &egui::Context,
    ) -> bool {
        let active_media = self.other_active_viewer_context_contains_video();
        let mounted_media =
            self.viewer_session_is_detached() && self.current_viewer_context_contains_video();
        if !active_media && !mounted_media {
            return true;
        }

        crate::logger::log(format!(
            "[bookmark-open] park detached media before full-feature book open active_context={active_media} mounted={mounted_media}"
        ));
        self.park_and_close_current_active_detached_viewer(ctx)
    }

    /// 「フル機能ウィンドウ + 動画・音声を別ウィンドウ」でブックマークを開く。
    ///
    /// ブックマーク一覧を載せた main context を実フォルダへ切り替えてから media window を
    /// 開くと、一覧とプレイヤーが同じ `ViewerContextBundle` を共有する。その状態で Esc を押すと
    /// 一覧復帰の folder load がプレイヤーを別 bundle へ promote し、1 回目は一覧復帰だけ、
    /// 2 回目で再生終了という二段 close になる。ここでは book detached context と同じ ownership
    /// seam で、実フォルダ・player・bookmark seek を最初から active detached bundle に載せる。
    /// main bundle はブックマーク一覧と復帰時の grid snapshot を保持したままにする。
    #[cfg(windows)]
    pub(super) fn open_bookmark_media_in_detached_context(
        &mut self,
        ctx: &egui::Context,
        requested: &Path,
        openable: PathBuf,
        select_requested_file: bool,
    ) -> bool {
        let Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending)) =
            self.bookmark_open_pending.take()
        else {
            return false;
        };
        let Some(target) = self.bookmark_view_target().cloned() else {
            self.bookmark_open_pending =
                Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
            return false;
        };

        // 同じ media window が既に生きている場合は、コンテキストを作り直さず seek だけ
        // 新しいブックマークへ差し替える。main 側の grid snapshot は今回のクリック時点の値。
        if self.raise_active_detached_media_for_grid_open(ctx, &pending.path) {
            let Some(id) = self.active_viewer_context_id() else {
                self.bookmark_open_pending =
                    Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
                return false;
            };
            let mut request = Some((pending, target));
            if let Err(error) = self.with_viewer_context(id, |app| {
                let (pending, target) = request.take().unwrap();
                app.bookmark_open_pending =
                    Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
                app.bookmark_view_state = Some(BookmarkViewState::Detached { target });
            }) {
                if let Some((pending, _)) = request {
                    self.bookmark_open_pending =
                        Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
                }
                crate::logger::log(format!(
                    "[bookmark-open] retarget mount failed id={id:?} error={error:?}"
                ));
                return false;
            }
            crate::logger::log(format!(
                "[bookmark-open] reuse detached media context path={}",
                requested.display()
            ));
            return true;
        }

        // 別の detached viewer があれば、通常の grid media open と同じ handoff seam で
        // park/close する。新しい boolean ownership state は作らず bundle 自体を所有境界にする。
        if !self.park_and_close_current_active_detached_viewer_for_media_handoff(ctx) {
            self.bookmark_open_pending =
                Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
            return false;
        }

        let source = if requested
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                crate::folder_tree::is_audio_ext(&extension.to_ascii_lowercase())
            }) {
            DetachedSource::Audio
        } else {
            DetachedSource::Video
        };
        let built = self.start_active_detached_loading_context(
            ctx,
            None,
            source,
            requested.to_path_buf(),
            |app, _window_id| {
                app.bookmark_open_pending =
                    Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
                app.bookmark_view_state = Some(BookmarkViewState::Detached { target });

                let outcome = app.with_detached_viewer_main_history_suppressed(|app| {
                    let outcome =
                        app.load_folder_or_convert_archive_with_auto_fullscreen(openable, true);
                    if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
                        app.open_startup_file_if_visible(requested);
                    }
                    outcome
                });
                let opened_target = matches!(outcome, FolderOpenOutcome::Loaded)
                    && app
                        .fullscreen_idx
                        .or_else(|| app.sidecar_restore_deferred_media_target(requested))
                        .and_then(|idx| app.items.get(idx))
                        .is_some_and(|item| match item {
                            GridItem::Video(path) | GridItem::Audio(path) => {
                                crate::path_key::eq_keep_drive(path, requested)
                            }
                            _ => false,
                        });

                if !opened_target {
                    app.close_fullscreen();
                    return DetachedLoadingContextInit::Abort("target_not_opened");
                }
                DetachedLoadingContextInit::Continue
            },
        );
        if built.is_none() {
            return false;
        }
        crate::logger::log(format!(
            "[bookmark-open] detached media context started path={} main_bookmarks={}",
            requested.display(),
            self.items_are_bookmark_view
        ));
        ctx.request_repaint();
        true
    }

    pub(crate) fn open_default_startup_target(&mut self) {
        let diagnostic = InitialNavigationTrace::begin("default-target-existing-loader-return");
        let correlation = diagnostic.correlation();
        let watch = diagnostic.watch;
        let adoption = event_span(Lane::Navigation, Stage::InitialTargetAdopt, correlation)
            .watched_optional(watch)
            .detail("default-target-call-return");
        let outcome = self.open_default_startup_target_observed(correlation, watch);
        adoption.finish(outcome);
        diagnostic.finish(outcome);
    }

    fn open_default_startup_target_observed(
        &mut self,
        correlation: u64,
        watch: Option<WatchHandle>,
    ) -> Outcome {
        if self.settings.startup_folder_mode == crate::settings::StartupFolderMode::Previous {
            let load = event_span(Lane::Navigation, Stage::InitialTargetScan, correlation)
                .watched_optional(watch)
                .detail("previous-list-call-return");
            self.open_previous_startup_list();
            load.finish(Outcome::Ok);
            return Outcome::Ok;
        }
        if self.settings.startup_folder_mode == crate::settings::StartupFolderMode::ReadingHistory {
            let load = event_span(Lane::Navigation, Stage::InitialTargetScan, correlation)
                .watched_optional(watch)
                .detail("reading-history-call-return");
            self.enter_reading_history();
            load.finish(Outcome::Ok);
        } else if should_start_in_drive_list(&self.settings) {
            let load = event_span(Lane::Navigation, Stage::InitialTargetScan, correlation)
                .watched_optional(watch)
                .detail("drive-list-call-return");
            self.enter_drive_list(None);
            load.finish(Outcome::Ok);
        } else {
            let resolve = event_span(Lane::Navigation, Stage::InitialTargetResolve, correlation)
                .watched_optional(watch)
                .detail("default-known-folder-resolution");
            let folder = crate::known_folders::startup_folder(
                self.settings.startup_folder_mode,
                self.settings.last_folder.as_deref(),
                self.settings.startup_folder_path.as_deref(),
            );
            resolve.finish(if folder.is_some() {
                Outcome::Ok
            } else {
                Outcome::Skipped
            });
            let Some(folder) = folder else {
                return Outcome::Skipped;
            };
            // last_folder には変換アーカイブ (RAR/CBR/7z/LZH) の元パスが入りうるので、
            // load_folder ではなく load_folder_or_convert_archive を通す。キャッシュが
            // あれば open_archive_via_cache が元アーカイブを開き直し (current_folder は
            // キャッシュ ZIP だが address/override は元アーカイブ)、無ければ変換ダイアログを
            // 出す。通常フォルダ / ネイティブ ZIP/PDF は format=None なので load_folder に委譲され挙動不変。
            //
            // 前回のカーソルは、開くのが**前回いた場所そのもの**のときだけ復元する。
            // 選択機構は増やさず、BS 戻りや親フォルダボタンが使っている `select_after_load`
            // にそのまま乗る (名前でケース無視照合 → フィルタで隠れていれば直近の可視 idx)。
            // 代入が無条件なのは、起動フォルダを開く時点で `select_after_load` に意見を
            // 持つ経路が他に無いため (BS 戻りも親フォルダボタンも、まだ 1 度も動いていない)。
            let previous_selection = self.select_after_load.clone();
            let previous_scroll = self.scroll_selected_to_rows_above;
            let hint = crate::known_folders::startup_cursor_hint(&self.settings, &folder);
            self.select_after_load = hint.as_ref().map(|(name, _)| name.clone());
            self.scroll_selected_to_rows_above = hint.and_then(|(_, rows)| rows);
            let load = event_span(Lane::Navigation, Stage::InitialTargetScan, correlation)
                .watched_optional(watch)
                .detail("default-existing-loader-call-return");
            let outcome = self.load_folder_or_convert_archive(folder);
            let refused = matches!(
                outcome,
                FolderOpenOutcome::Ignored | FolderOpenOutcome::Refused(_)
            );
            load.finish(if refused { Outcome::Error } else { Outcome::Ok });
            if refused {
                self.select_after_load = previous_selection;
                self.scroll_selected_to_rows_above = previous_scroll;
                return Outcome::Error;
            }
        }
        Outcome::Ok
    }

    /// Reuse the transient snapshot owner: the base grid remains projected, while the exact
    /// externally requested page lives in the reader beside the projected sibling order.
    pub(super) fn install_direct_media_reader(
        &mut self,
        mut prepared: DirectMediaPrepared,
    ) -> Option<crate::snapshot::SnapshotTarget> {
        let origin_path = self.current_folder.clone()?;
        let requested = match &prepared.item {
            GridItem::Image(path) | GridItem::Video(path) | GridItem::Audio(path) => path.clone(),
            _ => return None,
        };
        if startup_file_idx(&self.items, &requested).is_some() {
            return Some(crate::snapshot::SnapshotTarget::Fs(requested));
        }
        let return_origin = self.current_top_level_restore_snapshot().map(|restore| {
            top_level_grid_view::TopLevelGridOrigin {
                restore,
                route: self.facet_navigation.route().clone(),
            }
        });
        let policy = self.page_identity_index.policy.clone();
        let saved_page_identity_index = self.page_identity_index.clone();
        let saved_projection_recipe = self.projection_recipe.clone();
        let saved_viewing_state = self.capture_snapshot_viewing_state();
        let saved_items = self.items.clone();
        let saved_thumbnails = self.thumbnails.clone();
        let saved_image_metas = self.image_metas.clone();
        let saved_visible_indices = self.visible_indices.clone();
        let mut reader_order = self.current_reader_order().to_vec();
        let saved_scroll_offset_y = self.scroll_offset_y;
        let saved_selected = self.selected;
        let saved_zip_nav = self.zip_nav.clone();
        let mut reader_items = saved_items.clone();
        let mut reader_metas = saved_image_metas.clone();
        self.sync_current_context_rating_session_writes();
        if let Some(write) = crate::rating_sort::physical_item_key(&prepared.item)
            .and_then(|key| self.rating_session_writes.get(&key))
            .filter(|write| write.generation > prepared.rating_generation)
        {
            prepared.rating = crate::rating_sort::RatingSortKey::supported(write.stars)
                .expect("published ratings are validated");
        }
        let insert_at = direct_media_insertion_index(
            &saved_items,
            &saved_image_metas,
            &self.rating_cache,
            &self.settings,
            &prepared,
        );
        let details_sort = self.details_header_sort_active().then_some((
            self.settings.details_sort_key,
            self.settings.details_sort_ascending,
        ));
        let target_idx = reader_items.len();
        let reader_position = reader_order
            .iter()
            .position(|idx| *idx >= insert_at)
            .unwrap_or(reader_order.len());
        reader_items.push(prepared.item);
        reader_metas.push(prepared.metadata);
        let entries: Vec<_> = reader_items
            .iter()
            .filter_map(crate::snapshot::snapshot_entry_from_grid_item)
            .collect();
        let membership = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.key.clone(), index))
            .collect();
        let previous_ratings = self.rating_cache.clone();
        let previous_rotations = self.rotation_cache.clone();
        let target_rating = match prepared.rating {
            crate::rating_sort::RatingSortKey::Supported(stars) => Some(stars as u8),
            crate::rating_sort::RatingSortKey::Unsupported => None,
        };
        self.items = reader_items;
        // A direct audio/video exception makes the reader a mixed ordinary list. The saved
        // grid recipe is restored by the existing DirectMedia snapshot on return.
        if self.projection_recipe.extracted_surface().is_none()
            && matches!(
                self.items[target_idx],
                GridItem::Video(_) | GridItem::Audio(_)
            )
        {
            self.projection_recipe = super::book_materialization::ProjectionRecipe::OrdinaryList;
        }
        self.image_metas = reader_metas;
        if let Some(stars) = target_rating {
            self.rating_cache.insert(target_idx, stars);
        }
        self.tags_cache.extend(prepared.tags.clone());
        self.rotation_cache.insert(target_idx, prepared.rotation);
        reader_order.insert(reader_position, target_idx);
        self.thumbnails.push(ThumbnailState::Pending);
        self.bump_items_generation();
        self.invalidate_idx_state_and_queues();
        self.rating_cache = previous_ratings;
        self.rotation_cache = previous_rotations;
        if let Some(stars) = target_rating {
            self.rating_cache.insert(target_idx, stars);
        }
        self.tags_cache.extend(prepared.tags);
        self.rotation_cache.insert(target_idx, prepared.rotation);
        self.page_identity_index = Default::default();
        let generation_id = self.snapshot_next_generation_id;
        self.snapshot_next_generation_id = generation_id.saturating_add(1);
        self.snapshot = Some(crate::snapshot::SnapshotState {
            page_edit_source: crate::snapshot::SnapshotPageEditSource::PhysicalPrefix,
            items: entries,
            membership,
            origin: origin_path,
            filter_at_capture: self.current_filter_state(),
            source_label: crate::snapshot::SnapshotSourceLabel::DirectMedia {
                target: requested.clone(),
                policy,
                reader_order,
                details_sort,
            },
            generation_id,
            saved_items: saved_items.clone(),
            saved_page_identity_index: saved_page_identity_index.clone(),
            saved_projection_recipe: saved_projection_recipe.clone(),
            saved_viewing_state,
            saved_thumbnails: saved_thumbnails.clone(),
            saved_image_metas: saved_image_metas.clone(),
            saved_visible_indices,
            saved_scroll_offset_y,
            saved_selected,
            saved_zip_nav,
            list_view_items: saved_items,
            list_view_thumbnails: saved_thumbnails,
            list_view_image_metas: saved_image_metas,
            list_view_page_identity_index: saved_page_identity_index.clone(),
            list_view_projection_recipe: saved_projection_recipe,
            pre_snapshot_search_origin: None,
        });
        self.top_level_grid_view.begin_with_origin(
            top_level_grid_view::TopLevelGridSurface::Snapshot,
            return_origin,
        );
        self.rebuild_visible_indices();
        Some(crate::snapshot::SnapshotTarget::Fs(requested))
    }

    /// Finalize only the explicit page after the original combined hydration has
    /// installed its edit badges. Captured siblings keep their accepted permutation.
    pub(super) fn finalize_direct_media_reader_order(&mut self) {
        let Some((target, mut order, Some((key, ascending)))) =
            self.snapshot
                .as_ref()
                .and_then(|snapshot| match &snapshot.source_label {
                    crate::snapshot::SnapshotSourceLabel::DirectMedia {
                        target,
                        reader_order,
                        details_sort,
                        ..
                    } => Some((target.clone(), reader_order.clone(), *details_sort)),
                    _ => None,
                })
        else {
            return;
        };
        let Some(target_idx) = startup_file_idx(&self.items, &target) else {
            return;
        };
        order.retain(|idx| *idx != target_idx);
        for &idx in &order {
            if self
                .items
                .get(idx)
                .is_some_and(|item| crate::rating_sort::physical_item_key(item).is_some())
            {
                self.rating_cache.entry(idx).or_insert(0);
            }
        }
        let target_row = self.details_sort_row(target_idx, key);
        let position = order
            .iter()
            .position(|&idx| {
                let sibling = self.details_sort_row(idx, key);
                Self::compare_details_sort_rows(&target_row, &sibling, key, ascending).is_lt()
            })
            .unwrap_or(order.len());
        order.insert(position, target_idx);
        if let Some(snapshot) = self.snapshot.as_mut() {
            if let crate::snapshot::SnapshotSourceLabel::DirectMedia { reader_order, .. } =
                &mut snapshot.source_label
            {
                *reader_order = order;
            }
        }
        self.viewer_navigation_caches.invalidate();
    }

    pub(super) fn retire_direct_media_reader_for_source_adoption(&mut self) {
        if self.direct_media_reader_active() {
            self.snapshot = None;
            self.top_level_grid_view.take_return_origin();
            self.top_level_grid_view
                .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
        }
    }

    pub(super) fn restore_direct_media_reader_origin(&mut self) {
        if !self.snapshot.as_ref().is_some_and(|snapshot| {
            matches!(
                snapshot.source_label,
                crate::snapshot::SnapshotSourceLabel::DirectMedia { .. }
            )
        }) {
            return;
        }
        let detached_owner = self.navigation_scope == ViewerNavigationScope::DetachedPhysical;
        #[cfg(windows)]
        let detached_owner =
            detached_owner || self.projected_viewer_context_id() != self.viewer_context_main();
        if detached_owner {
            self.snapshot = None;
            self.top_level_grid_view.take_return_origin();
            self.top_level_grid_view
                .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
            return;
        }
        if let Some(origin) = self.dismiss_snapshot_with_origin() {
            self.pending_reload = false;
            self.restore_view_return_origin(origin);
        }
    }

    fn open_startup_file_if_visible(&mut self, requested: &Path) {
        if let Err(error) = self.open_loaded_file_fullscreen(requested, "startup_open_file") {
            crate::logger::log(format!("startup open: {error}"));
        }
    }

    /// Open a file through the same loaded-folder selection/fullscreen seam used by startup and
    /// activation requests. The caller owns path validation and any folder load needed first.
    pub(crate) fn open_loaded_file_fullscreen(
        &mut self,
        requested: &Path,
        input_reason: &'static str,
    ) -> Result<usize, String> {
        let Some(idx) = startup_file_idx(&self.items, requested) else {
            return Err(format!(
                "requested file was not found in the loaded folder: {}",
                requested.display()
            ));
        };
        if !startup_file_should_open_fullscreen(&self.items[idx]) {
            return Err(format!(
                "requested path is not playable media: {}",
                requested.display()
            ));
        }
        self.move_grid_cursor(Some(idx), crate::app::GridCursorIntent::Automatic);
        self.scroll_to_selected = true;
        crate::logger::log(format!(
            "{input_reason}: opening requested file through the loaded-folder viewer path: {}",
            requested.display()
        ));
        self.bump_input_seq_for_item(input_reason, idx);
        self.fs_open_intent_from_grid = true;
        self.open_fullscreen(idx, crate::app::HistoryTrigger::UserChosen);
        Ok(idx)
    }
}

/// The accepted siblings keep their exact prepared permutation; only the explicit initial
/// page is inserted. All target metadata and rating facts came from the resolver worker.
fn direct_media_insertion_index(
    items: &[GridItem],
    metadata: &[Option<(i64, i64)>],
    ratings: &HashMap<usize, u8>,
    settings: &crate::settings::Settings,
    target: &DirectMediaPrepared,
) -> usize {
    use crate::settings::GridItemDisplayKind as K;
    let row = |item: &GridItem| {
        settings.grid_display_order.row_for(match item {
            GridItem::Folder(_) => K::Folder,
            GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::ZipDir { .. } => K::Archive,
            GridItem::Video(_) | GridItem::Audio(_) => K::VideoAudio,
            _ => K::Image,
        })
    };
    let target_row = row(&target.item);
    let sort = crate::rating_sort::ListingOrderRequest::from_settings(settings);
    let target_name = sort.standard_fallback().name_key(&target.item.name());
    let listing_meta = |meta: Option<(i64, i64)>| match meta {
        Some((mtime, size)) => crate::settings::ListingSortMetadata::new(mtime, Some(size)),
        None => crate::settings::ListingSortMetadata::new(0, None),
    };
    items
        .iter()
        .enumerate()
        .position(|(index, item)| {
            let item_row = row(item);
            if item_row != target_row {
                return item_row > target_row;
            }
            let name = sort.standard_fallback().name_key(&item.name());
            let comparison = match sort {
                crate::rating_sort::ListingOrderRequest::Standard(sort) => sort
                    .compare_listing_keys(
                        &target_name,
                        listing_meta(target.metadata),
                        &name,
                        listing_meta(metadata.get(index).copied().flatten()),
                    ),
                crate::rating_sort::ListingOrderRequest::Rating(spec) => spec.compare(
                    target.rating,
                    &target_name,
                    crate::rating_sort::physical_item_key(item)
                        .map(|_| ratings.get(&index).copied().unwrap_or(0))
                        .and_then(crate::rating_sort::RatingSortKey::supported)
                        .unwrap_or(crate::rating_sort::RatingSortKey::Unsupported),
                    &name,
                ),
            };
            comparison.is_lt()
        })
        .unwrap_or(items.len())
}

fn startup_file_idx(items: &[GridItem], requested: &Path) -> Option<usize> {
    items.iter().position(|item| {
        let path = match item {
            GridItem::Folder(path)
            | GridItem::Image(path)
            | GridItem::Video(path)
            | GridItem::Audio(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. } => path,
            _ => return false,
        };
        crate::path_key::eq_keep_drive(path, requested)
    })
}

pub(crate) fn startup_file_should_open_fullscreen(item: &GridItem) -> bool {
    matches!(
        item,
        GridItem::Image(_) | GridItem::Video(_) | GridItem::Audio(_)
    )
}

pub(crate) fn startup_openable_should_auto_fullscreen(
    settings: &crate::settings::Settings,
    openable: &Path,
    kind: crate::folder_tree::OpenablePathKind,
) -> bool {
    if !settings.effective_auto_fullscreen_zip_pdf() {
        return false;
    }
    match kind {
        crate::folder_tree::OpenablePathKind::File => {
            if settings.epub_file_handling_ignores_epub()
                && openable
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
            {
                return false;
            }
            crate::folder_tree::is_open_as_container(openable)
                || (!settings.archive_file_handling_ignores_convertible()
                    && crate::folder_tree::is_convertible_archive_path(openable))
        }
        crate::folder_tree::OpenablePathKind::Directory => {
            settings.auto_fullscreen_image_folders_enabled()
        }
    }
}

fn resolve_startup_open_path(
    requested: PathBuf,
    source: StartupOpenPathSource,
    bookmark: Option<&crate::book_bookmarks::BookBookmark>,
    probe_rar: bool,
) -> StartupOpenPathResolveResult {
    let t0 = std::time::Instant::now();
    let bookmark_relative_page_openable = bookmark.and_then(|bookmark| {
        let crate::book_bookmarks::PageIdentity::RelativePath(relative) = &bookmark.page_identity
        else {
            return None;
        };
        Some(matches!(
            crate::book_bookmarks::resolve_relative_page_path(&bookmark.container_path, relative),
            crate::book_bookmarks::RelativePagePathResolution::Existing(_)
        ))
    });
    let resolved = crate::folder_tree::resolve_openable_path_detailed(&requested);
    let direct_media = (!matches!(source, StartupOpenPathSource::Bookmark))
        .then(|| {
            resolved
                .as_ref()
                .filter(|resolution| resolution.requested_is_file)
        })
        .flatten()
        .and_then(|_| {
            use crate::file_type_visibility::FileTypeCategory as C;
            let extension = requested.extension()?.to_str()?;
            let item = match crate::file_type_visibility::category_for_extension(extension)? {
                C::Image | C::Raw => GridItem::Image(requested.clone()),
                C::Video => GridItem::Video(requested.clone()),
                C::Audio => GridItem::Audio(requested.clone()),
                _ => return None,
            };
            let metadata = std::fs::metadata(&requested).ok().map(|metadata| {
                (
                    crate::ui_helpers::mtime_secs(&metadata),
                    metadata.len() as i64,
                )
            });
            let rating_generation = crate::rating_db::RATING_WRITES.sample().completed_writes;
            let rating = crate::rating_sort::physical_item_key(&item)
                .and_then(|key| {
                    crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
                        .ok()
                        .and_then(|db| db.get_many_complete(&[key.clone()], rating_generation).ok())
                        .map(|facts| facts.key_for_requested(&key))
                })
                .unwrap_or(crate::rating_sort::RatingSortKey::Unsupported);
            let tags = crate::rating_sort::physical_item_key(&item)
                .map(|key| {
                    crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path())
                        .map(|db| db.get_many_display_tags(&[key]))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let rotation = crate::rotation_db::RotationDb::open_readonly()
                .ok()
                .and_then(|db| db.get(&requested))
                .unwrap_or(crate::rotation_db::Rotation::None);
            Some(DirectMediaPrepared {
                rotation,
                item,
                metadata,
                rating,
                rating_generation,
                tags,
            })
        });
    let rar_volume_proof = probe_rar
        .then(|| {
            resolved.as_ref().and_then(|resolution| {
                (matches!(resolution.kind, crate::folder_tree::OpenablePathKind::File)
                    && crate::rar_loader::is_rar_path(&resolution.path))
                .then(|| crate::rar_loader::volume_proof(&resolution.path).ok())
                .flatten()
            })
        })
        .flatten();
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if crate::perf::is_enabled() {
        crate::perf::event(
            "startup",
            "open_path_resolve",
            None,
            0,
            &[
                ("ms", serde_json::Value::from(elapsed_ms)),
                ("resolved", serde_json::Value::from(resolved.is_some())),
                ("source", serde_json::Value::from(source.perf_tag())),
            ],
        );
    }
    StartupOpenPathResolveResult {
        requested,
        resolved,
        bookmark_relative_page_openable,
        rar_volume_proof,
        elapsed_ms,
        direct_media,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(unix)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[test]
    fn startup_file_lookup_matches_normalized_db_paths_for_video_and_audio() {
        let items = vec![
            GridItem::Video(PathBuf::from(r"C:\Media\Movie.MP4")),
            GridItem::Audio(PathBuf::from(r"C:\Media\Track.FLAC")),
        ];

        assert_eq!(
            startup_file_idx(&items, Path::new("c:/media/movie.mp4")),
            Some(0)
        );
        assert_eq!(
            startup_file_idx(&items, Path::new("c:/media/track.flac")),
            Some(1)
        );
    }

    #[test]
    fn excluded_external_media_startup_and_activation_keep_projected_parent_and_siblings() {
        for source in [
            StartupOpenPathSource::InitialStartup,
            StartupOpenPathSource::Activation,
        ] {
            let mut app = setup_app_for_test();
            app.settings.detached_viewer_open_images_in_window = false;
            let parent = app.tmp.path().join("direct-media-parent");
            std::fs::create_dir_all(&parent).unwrap();
            let requested = parent.join("2.png");
            let hidden_sibling = parent.join("4.png");
            let visible_siblings = [parent.join("1.jpg"), parent.join("3.jpg")];
            for path in [
                &requested,
                &hidden_sibling,
                &visible_siblings[0],
                &visible_siblings[1],
            ] {
                image::RgbImage::from_pixel(2, 2, image::Rgb([30, 40, 50]))
                    .save(path)
                    .unwrap();
            }
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("png".into());
            let result = resolve_startup_open_path(requested.clone(), source, None, true);
            assert!(result.direct_media.is_some());
            let ctx = egui::Context::default();
            let owner = match source {
                StartupOpenPathSource::InitialStartup => StartupOpenPathOwner::InitialStartup,
                StartupOpenPathSource::Activation => StartupOpenPathOwner::Activation,
                StartupOpenPathSource::Bookmark => unreachable!(),
            };
            app.finish_startup_open_path_resolve(owner, result, &ctx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while app.sidecar_restore_active() {
                assert!(std::time::Instant::now() < deadline);
                app.poll_sidecar_restore(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let index = app.fullscreen_idx.expect("exact explicit target must open");
            assert!(matches!(&app.items[index], GridItem::Image(path) if path == &requested));
            assert!(
                app.items
                    .iter()
                    .all(|item| !matches!(item, GridItem::Image(path) if path == &hidden_sibling))
            );
            assert_eq!(app.visible_indices.len(), 2);
            assert!(!app.visible_indices.contains(&index));
            assert!(
                !app.is_snapshot_active(),
                "external transient reader never takes the fixed-snapshot lock"
            );
            assert!(app.snapshot_count().is_none());
            assert_eq!(app.current_reader_order().len(), 3);
            assert_eq!(app.get_still_image_indices().len(), 3);
            let next = crate::ui_helpers::adjacent_navigable_idx(
                &app.items,
                app.current_reader_order(),
                index,
                1,
            )
            .unwrap();
            let previous = crate::ui_helpers::adjacent_navigable_idx(
                &app.items,
                app.current_reader_order(),
                index,
                -1,
            )
            .unwrap();
            assert!(
                matches!(&app.items[next], GridItem::Image(path) if path == &visible_siblings[1])
            );
            assert!(
                matches!(&app.items[previous], GridItem::Image(path) if path == &visible_siblings[0])
            );
            assert_eq!(app.facet_candidate_indices(FacetField::Rating).len(), 2);
            assert!(app.snapshot_scope_allows_open(
                &parent.with_extension("other"),
                &OpenRequestOwner::Navigation
            ));

            let snapshot = app
                .snapshot
                .as_ref()
                .expect("transient reader has one typed owner");
            assert_eq!(snapshot.saved_items.len(), 2);
            assert!(
                snapshot
                    .saved_items
                    .iter()
                    .all(|item| !matches!(item, GridItem::Image(path) if path == &requested))
            );
            assert_eq!(
                app.items
                    .iter()
                    .filter(|item| startup_file_should_open_fullscreen(item))
                    .count(),
                3
            );
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("jpg".into());
            app.close_fullscreen();
            while app
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
                || app.sidecar_restore_active()
            {
                assert!(std::time::Instant::now() < deadline);
                app.poll_collection_history_transition(&ctx);
                app.poll_sidecar_restore(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(app.snapshot.is_none());
            assert!(app.fullscreen_idx.is_none());
            assert!(
                app.items.is_empty(),
                "close restores parent under latest policy"
            );
            assert_eq!(app.current_folder.as_ref(), Some(&parent));
        }
    }

    #[test]
    fn external_media_prepare_is_worker_validated_and_bookmark_never_gets_exemption() {
        // The normal App fixture owns the data-dir lease and its TestEpochScope.
        let _app = setup_app_for_test();
        let temp = tempfile::tempdir().unwrap();
        for extension in ["png", "cr2", "mp4", "flac"] {
            let path = temp.path().join(format!("target.{extension}"));
            std::fs::write(&path, b"worker validation fixture").unwrap();
            for source in [
                StartupOpenPathSource::InitialStartup,
                StartupOpenPathSource::Activation,
            ] {
                let result = resolve_startup_open_path(path.clone(), source, None, true);
                assert!(result.direct_media.is_some(), "{extension}");
            }
            assert!(
                resolve_startup_open_path(path, StartupOpenPathSource::Bookmark, None, true)
                    .direct_media
                    .is_none()
            );
        }
        assert!(
            resolve_startup_open_path(
                temp.path().join("missing.png"),
                StartupOpenPathSource::InitialStartup,
                None,
                true,
            )
            .direct_media
            .is_none()
        );
    }

    #[test]
    fn direct_media_insertion_preserves_rating_order_and_all_media_navigation() {
        let mut settings = crate::settings::Settings::default();
        settings.sort_order = crate::settings::SortOrder::RatingDesc;
        let siblings = vec![
            GridItem::Image("z-first.jpg".into()),
            GridItem::Image("a-last.jpg".into()),
        ];
        let ratings = HashMap::from([(0, 5), (1, 1)]);
        let prepared = DirectMediaPrepared {
            item: GridItem::Image("m-hidden.png".into()),
            metadata: Some((1, 1)),
            rating: crate::rating_sort::RatingSortKey::supported(3).unwrap(),
            rating_generation: 0,
            rotation: crate::rotation_db::Rotation::None,
            tags: Default::default(),
        };
        assert_eq!(
            direct_media_insertion_index(&siblings, &[None, None], &ratings, &settings, &prepared),
            1
        );
        assert_eq!(siblings[0].name(), "z-first.jpg");
        assert_eq!(siblings[1].name(), "a-last.jpg");
        let unrated = DirectMediaPrepared {
            item: GridItem::Image("m-unrated.png".into()),
            metadata: None,
            rating: crate::rating_sort::RatingSortKey::supported(0).unwrap(),
            rating_generation: 0,
            rotation: crate::rotation_db::Rotation::None,
            tags: Default::default(),
        };
        assert_eq!(
            direct_media_insertion_index(
                &[
                    GridItem::Image("a.jpg".into()),
                    GridItem::Image("z.jpg".into())
                ],
                &[None, None],
                &HashMap::new(),
                &settings,
                &unrated,
            ),
            1,
            "ratable cache absence retains supported zero and the name tie-break"
        );
        settings.sort_order = crate::settings::SortOrder::FileName;
        for target in [
            GridItem::Image("2.cr2".into()),
            GridItem::Video("2.mp4".into()),
            GridItem::Audio("2.flac".into()),
        ] {
            let neighbors = match target {
                GridItem::Image(_) => vec![
                    GridItem::Image("1.jpg".into()),
                    GridItem::Image("3.jpg".into()),
                ],
                _ => vec![
                    GridItem::Video("1.mp4".into()),
                    GridItem::Audio("3.flac".into()),
                ],
            };
            let prepared = DirectMediaPrepared {
                item: target,
                metadata: None,
                rating: crate::rating_sort::RatingSortKey::Unsupported,
                rating_generation: 0,
                rotation: crate::rotation_db::Rotation::None,
                tags: Default::default(),
            };
            assert_eq!(
                direct_media_insertion_index(
                    &neighbors,
                    &[None, None],
                    &Default::default(),
                    &settings,
                    &prepared
                ),
                1
            );
            let mut reader = neighbors.clone();
            reader.insert(1, prepared.item);
            assert_eq!(
                crate::ui_helpers::adjacent_navigable_idx(&reader, &[0, 1, 2], 1, 1),
                Some(2)
            );
            assert_eq!(
                crate::ui_helpers::adjacent_navigable_idx(&reader, &[0, 1, 2], 1, -1),
                Some(0)
            );
        }
        let mut app = setup_app_for_test();
        app.items = vec![GridItem::Audio("track.flac".into())];
        app.fullscreen_idx = Some(0);
        assert_eq!(
            app.snapshot_current_fullscreen_path(),
            Some(PathBuf::from("track.flac"))
        );
    }

    #[test]
    fn direct_media_activation_can_supersede_other_folder_and_fixed_snapshot_keeps_lock() {
        let mut app = setup_app_for_test();
        app.settings.detached_viewer_open_images_in_window = false;
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        let ctx = egui::Context::default();
        let mut last_parent = None;
        for name in ["first", "second"] {
            let parent = app.tmp.path().join(name);
            std::fs::create_dir_all(&parent).unwrap();
            let target = parent.join("2.png");
            image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]))
                .save(&target)
                .unwrap();
            image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]))
                .save(parent.join("1.jpg"))
                .unwrap();
            let result = resolve_startup_open_path(
                target.clone(),
                StartupOpenPathSource::Activation,
                None,
                true,
            );
            app.finish_startup_open_path_resolve(StartupOpenPathOwner::Activation, result, &ctx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while app.sidecar_restore_active() {
                assert!(std::time::Instant::now() < deadline);
                app.poll_sidecar_restore(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert_eq!(app.current_folder.as_ref(), Some(&parent));
            let opened = app
                .fullscreen_idx
                .expect("superseding external target opens");
            assert!(matches!(&app.items[opened], GridItem::Image(path) if path == &target));
            last_parent = Some(parent);
        }
        app.close_fullscreen();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app
            .top_level_grid_view
            .history_navigation_transition()
            .is_some()
            || app.sidecar_restore_active()
        {
            assert!(std::time::Instant::now() < deadline);
            app.poll_collection_history_transition(&ctx);
            app.poll_sidecar_restore(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(app.current_folder, last_parent);
        assert_eq!(app.items.len(), 1);
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        assert!(!app.snapshot_scope_allows_open(
            &app.tmp.path().join("outside"),
            &OpenRequestOwner::Navigation
        ));
    }

    fn direct_reader_fixture() -> crate::app::AppTestEnvForTest {
        let mut app = setup_app_for_test();
        let parent = app.tmp.path().join("direct-owner-fixture");
        std::fs::create_dir_all(&parent).unwrap();
        app.current_folder = Some(parent.clone());
        app.items = vec![
            GridItem::Image(parent.join("a.jpg")),
            GridItem::Image(parent.join("z.jpg")),
        ];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.image_metas = vec![Some((1, 20)), Some((2, 30))];
        app.visible_indices = vec![0, 1];
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        app.page_identity_index = Arc::new(
            super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                &app.items,
                None,
                app.settings.file_type_visibility.clone(),
            ),
        );
        app
    }

    fn direct_fixture_target(app: &App, name: &str) -> DirectMediaPrepared {
        DirectMediaPrepared {
            item: GridItem::Image(app.current_folder.as_ref().unwrap().join(name)),
            metadata: Some((3, 25)),
            rating: crate::rating_sort::RatingSortKey::supported(3).unwrap(),
            rating_generation: 0,
            rotation: crate::rotation_db::Rotation::None,
            tags: Default::default(),
        }
    }

    #[test]
    fn visibility_direct_media_details_order_keeps_nonmonotonic_siblings_and_facet_basis() {
        let mut app = direct_reader_fixture();
        app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        app.settings.details_sort_ascending = false;
        app.details_order = vec![1, 0];
        let target = direct_fixture_target(&app, "m.png");
        app.install_direct_media_reader(target).unwrap();
        app.finalize_direct_media_reader_order();
        assert_eq!(app.current_reader_order(), &[1, 2, 0]);
        assert_eq!(app.smart_folder_rule_total(), 2);
        assert_eq!(app.visible_indices, vec![0, 1]);
        drop(app);

        let mut app = direct_reader_fixture();
        app.rating_cache = HashMap::from([(0, 5)]);
        app.settings.rating_filter = [false, false, false, false, false, true];
        app.rebuild_visible_indices();
        assert_eq!(app.visible_indices, vec![0]);
        let target = direct_fixture_target(&app, "m.png");
        app.install_direct_media_reader(target).unwrap();
        assert_eq!(app.current_reader_order(), &[0, 2]);
        assert_eq!(app.visible_indices, vec![0]);
        assert_eq!(
            app.facet_candidate_indices(super::super::FacetField::Rating),
            vec![0, 1]
        );
    }

    #[test]
    fn visibility_direct_media_refused_and_admitted_pending_opens_keep_reader_until_adoption() {
        let mut app = direct_reader_fixture();
        let target = direct_fixture_target(&app, "m.png");
        app.install_direct_media_reader(target).unwrap();
        let generation = app.items_generation;
        let stale = OpenRequestOwner::Bookmark(crate::bookmark_browser::BookmarkOpenRequestOwner {
            request_id: crate::bookmark_browser::BookmarkOpenRequestId(80001),
            target: crate::bookmark_browser::BookmarkViewReturnTarget::Book(PathBuf::from(
                "stale.zip",
            )),
            #[cfg(windows)]
            detached_lease: None,
        });
        assert!(!app.claim_open_request_owner(Path::new("stale.zip"), &stale));
        assert!(app.direct_media_reader_active());
        assert!(
            app.claim_open_request_owner(Path::new("pending.zip"), &OpenRequestOwner::Navigation)
        );
        assert!(app.direct_media_reader_active());
        assert_eq!(app.current_reader_order(), &[0, 2, 1]);
        assert_eq!(app.items_generation, generation);
    }

    #[test]
    #[cfg(windows)]
    fn visibility_direct_media_other_destination_admission_keeps_main_owner() {
        let mut app = direct_reader_fixture();
        let target = direct_fixture_target(&app, "m.png");
        app.install_direct_media_reader(target).unwrap();
        app.detached_grid_archive_open_request_seq = 77;
        let owner = OpenRequestOwner::DetachedGridArchive(DetachedGridArchiveOpenRequestOwner {
            request_id: 77,
            lease: DetachedSessionLease { window_id: 80002 },
            source_path: PathBuf::from("other-context.zip"),
            collection_restore: None,
        });
        assert!(app.claim_open_request_owner(Path::new("other-context.zip"), &owner));
        assert!(app.direct_media_reader_active());
        assert_eq!(app.current_reader_order(), &[0, 2, 1]);
    }

    #[test]
    fn visibility_direct_media_empty_fixed_snapshot_refusal_keeps_exact_reader_owner() {
        let mut app = direct_reader_fixture();
        let target = direct_fixture_target(&app, "m.png");
        app.install_direct_media_reader(target).unwrap();
        app.fullscreen_idx = Some(2);
        app.visible_indices.clear();
        let index = app.page_identity_index.clone();
        let generation = app.items_generation;
        let owner_generation = app.snapshot.as_ref().unwrap().generation_id;
        let order = app.current_reader_order().to_vec();
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        assert!(app.direct_media_reader_active());
        assert!(!app.is_snapshot_active());
        assert_eq!(app.fullscreen_idx, Some(2));
        assert_eq!(app.items.len(), 3);
        assert_eq!(app.items_generation, generation);
        assert_eq!(
            app.snapshot.as_ref().unwrap().generation_id,
            owner_generation
        );
        assert_eq!(app.current_reader_order(), order);
        assert!(Arc::ptr_eq(&app.page_identity_index, &index));
        assert!(app.folder_pane_open_pending.is_none());
        assert!(!app.sidecar_restore_active());
    }

    #[test]
    fn visibility_direct_media_composes_with_fixed_snapshot_without_hidden_basis_or_return_reload()
    {
        for initial_target_open in [true, false] {
            let mut app = direct_reader_fixture();
            let basis_index = app.page_identity_index.clone();
            let target = direct_fixture_target(&app, "m.png");
            app.install_direct_media_reader(target).unwrap();
            app.visible_indices = vec![1];
            app.fullscreen_idx = Some(if initial_target_open { 2 } else { 1 });
            let parent = app.current_folder.clone();
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            assert!(app.is_snapshot_active());
            assert!(!app.direct_media_reader_active());
            assert_eq!(app.items.len(), 1);
            assert_eq!(app.snapshot.as_ref().unwrap().saved_items.len(), 2);
            assert!(Arc::ptr_eq(
                &app.snapshot.as_ref().unwrap().saved_page_identity_index,
                &basis_index
            ));
            assert_eq!(
                app.fullscreen_idx,
                if initial_target_open { None } else { Some(0) }
            );
            assert_eq!(app.current_folder, parent);
            assert!(app.folder_pane_open_pending.is_none());
            assert!(app.folder_nav_pending.is_none());
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while app.sidecar_restore_active() {
                assert!(std::time::Instant::now() < deadline);
                app.poll_sidecar_restore(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let identity =
                crate::book_resume_db::PageIdentity::for_item(&app.items[0], None).unwrap();
            let position = crate::book_resume_db::ReadingPosition {
                page: 1,
                identity: Some(identity),
            };
            assert_eq!(
                app.page_identity_index.resolve(&position),
                super::super::file_type_resume::PositionResolution::Restored(0)
            );
            app.deactivate_snapshot();
            assert_eq!(app.items.len(), 2);
            assert!(app.items.iter().all(|item| !item.name().ends_with(".png")));
            assert!(Arc::ptr_eq(&app.page_identity_index, &basis_index));
            assert_eq!(
                app.page_identity_index.resolve(&position),
                super::super::file_type_resume::PositionResolution::Restored(1)
            );
            let hidden = crate::book_resume_db::PageIdentity::for_item(
                &GridItem::Image(app.current_folder.as_ref().unwrap().join("m.png")),
                None,
            )
            .unwrap();
            assert_eq!(
                app.page_identity_index
                    .resolve(&crate::book_resume_db::ReadingPosition {
                        page: 2,
                        identity: Some(hidden)
                    }),
                super::super::file_type_resume::PositionResolution::HiddenIdentity
            );
            assert!(!app.sidecar_restore_active());
            assert!(app.folder_pane_open_pending.is_none());
            assert!(app.folder_nav_pending.is_none());
            assert_eq!(
                app.fullscreen_idx,
                if initial_target_open { None } else { Some(1) }
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn visibility_direct_video_audio_pdf_adoption_preserves_actual_parked_reader_owner() {
        for audio in [false, true] {
            let mut app = direct_reader_fixture();
            let mut target = direct_fixture_target(&app, if audio { "m.flac" } else { "m.mp4" });
            let path = match target.item {
                GridItem::Image(path) => path,
                _ => unreachable!(),
            };
            target.item = if audio {
                GridItem::Audio(path)
            } else {
                GridItem::Video(path)
            };
            app.install_direct_media_reader(target).unwrap();
            app.page_identity_index = Arc::new(
                super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &app.items,
                    None,
                    app.settings.file_type_visibility.clone(),
                ),
            );
            app.fullscreen_idx = Some(2);
            let accepted = app.page_identity_index.clone();
            let order = app.current_reader_order().to_vec();
            let window = app.ensure_detached_viewer_window_id();
            app.become_mounted_independent_detached_viewer(window, 2);
            assert!(app.should_promote_active_detached_video_for_main_context_change());
            let pdf = crate::pdf_loader::LeasedEpubPath::try_new(app.tmp.path().join("next.pdf"))
                .unwrap();
            app.prepare_pdf_visible_adoption(&pdf);
            assert!(!app.direct_media_reader_active());
            assert!(
                app.items.is_empty(),
                "adopted main source starts in the established fresh context"
            );
            let main = app.projected_viewer_context_id();
            let main_generation = app.items_generation;
            app.with_window_viewer_context(window, |parked| {
                assert!(parked.direct_media_reader_active());
                assert_eq!(parked.current_reader_order(), order);
                assert!(Arc::ptr_eq(&parked.page_identity_index, &accepted));
                parked.restore_direct_media_reader_origin();
                assert!(!parked.direct_media_reader_active());
                assert!(
                    !parked.sidecar_restore_active(),
                    "owning detached close consumes the origin without rescanning"
                );
            })
            .unwrap();
            assert_eq!(app.projected_viewer_context_id(), main);
            assert_eq!(app.items_generation, main_generation);
            assert!(app.items.is_empty());
        }
    }

    #[test]
    #[cfg(windows)]
    fn visibility_direct_media_successful_fork_moves_owner_and_preserves_main_basis() {
        for live in [false, true] {
            let mut app = direct_reader_fixture();
            let basis_index = app.page_identity_index.clone();
            let parent = app.current_folder.clone().unwrap();
            let target = direct_fixture_target(&app, "m.png");
            app.install_direct_media_reader(target).unwrap();
            app.page_identity_index = Arc::new(
                super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &app.items,
                    None,
                    app.settings.file_type_visibility.clone(),
                ),
            );
            app.fullscreen_idx = Some(2);
            let generation = app.items_generation;
            let mut detached = if live {
                app.split_current_context_preserving_main_grid()
            } else {
                app.split_materialized_physical_context_for_detached_scope(&parent, 2, generation)
            };
            assert!(!app.direct_media_reader_active());
            assert_eq!(app.items.len(), 2);
            assert_eq!(app.visible_indices, vec![0, 1]);
            assert!(Arc::ptr_eq(&app.page_identity_index, &basis_index));
            let main_generation = app.items_generation;
            app.swap_viewer_context_bundle(&mut detached);
            assert!(matches!(
                app.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Snapshot
            ));
            app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
            assert!(app.direct_media_reader_active());
            assert_eq!(app.current_reader_order(), &[0, 2, 1]);
            app.restore_direct_media_reader_origin();
            assert!(!app.direct_media_reader_active());
            app.swap_viewer_context_bundle(&mut detached);
            assert_eq!(app.items.len(), 2);
            assert_eq!(app.items_generation, main_generation);
            assert!(Arc::ptr_eq(&app.page_identity_index, &basis_index));
        }
    }

    fn arm_section1339_bookmark(
        app: &mut App,
        id: u64,
        path: &Path,
    ) -> crate::bookmark_browser::BookmarkOpenRequestOwner {
        let request_id = crate::bookmark_browser::BookmarkOpenRequestId(id);
        let target = crate::bookmark_browser::BookmarkViewReturnTarget::Book(path.to_path_buf());
        app.bookmark_view_state = Some(BookmarkViewState::Opening {
            target: target.clone(),
            grid: BookmarkViewReturnGridState {
                row_keys: vec![(0, 1)],
                selected_key: Some((0, 1)),
                opened_key: (0, 1),
                scroll_offset_y: 0.0,
            },
        });
        app.bookmark_open_pending = Some(crate::bookmark_browser::PendingBookmarkOpen::Book(
            crate::bookmark_browser::PendingBookOpen {
                request_id,
                bookmark_source: crate::pdf_loader::LeasedEpubPath::try_new(path.to_path_buf())
                    .unwrap(),
                bookmark: crate::book_bookmarks::BookBookmark {
                    id: 1,
                    container_key: crate::book_bookmarks::container_key(path),
                    container_path: path.to_path_buf(),
                    container_kind: crate::book_bookmarks::BookContainerKind::Zip,
                    page_identity: crate::book_bookmarks::PageIdentity::ArchiveEntry(
                        "1.jpg".into(),
                    ),
                    page_index_hint: 0,
                    created_at_ms: 1,
                    title: None,
                },
                relative_page_provenance: None,
                started_at: std::time::Instant::now(),
                stage: crate::bookmark_browser::PendingBookOpenStage::Resolving,
            },
        ));
        crate::bookmark_browser::BookmarkOpenRequestOwner {
            request_id,
            target,
            #[cfg(windows)]
            detached_lease: None,
        }
    }

    fn write_section1339_bookmark_zip(path: &Path) {
        use std::io::Write;
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        archive
            .start_file("1.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"image metadata fixture").unwrap();
        archive.finish().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn detached_bookmark_cache_only_rejects_later_rar_with_valid_legacy_cache() {
        let mut app = setup_app_for_test();
        app.settings.detached_viewer_open_images_in_window = true;
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
        let source = app.tmp.path().join("bookmark.part2.rar");
        std::fs::copy(fixture.join("real-split-control.part2.rar"), &source).unwrap();
        std::fs::copy(
            fixture.join("real-split-control.part1.rar"),
            app.tmp.path().join("bookmark.part1.rar"),
        )
        .unwrap();
        let cached = app.tmp.path().join("bookmark-cache.zip");
        write_section1339_bookmark_zip(&cached);
        let metadata = std::fs::metadata(&source).unwrap();
        app.archive_cache_db
            .as_ref()
            .unwrap()
            .record(
                &source,
                crate::ui_helpers::mtime_secs(&metadata),
                metadata.len() as i64,
                crate::archive_converter::ArchiveFormat::Rar,
                &cached,
                std::fs::metadata(&cached).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
        assert_eq!(app.try_archive_cache_lookup(&source), Some(cached));
        arm_section1339_bookmark(&mut app, 404, &source);
        app.bookmark_open_pending
            .as_mut()
            .unwrap()
            .book_mut()
            .unwrap()
            .bookmark
            .container_kind = crate::book_bookmarks::BookContainerKind::OtherArchive;
        let origin = app.current_folder.clone();
        let generation = app.items_generation;
        let ctx = egui::Context::default();
        app.start_startup_open_path_resolve(source, StartupOpenPathSource::Bookmark, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.startup_open_path_resolve_pending.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "bookmark resolver timed out"
            );
            app.poll_startup_open_path_resolve(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(app.current_folder, origin);
        assert_eq!(app.items_generation, generation);
        assert!(app.active_viewer_context_id().is_none());
        assert!(app.detached_window_manager.is_empty());
        assert!(app.zip_enumerate_pending.is_none());
        assert!(app.archive_convert.is_none());
        assert!(app.fs_feedback_toast.as_ref().unwrap().0.contains("part1"));
    }

    #[test]
    fn section1339_quick_reselect_retires_unadopted_bookmark_but_keeps_adopted_hydration() {
        for adopted in [false, true] {
            let mut app = setup_app_for_test();
            let path = app.tmp.path().join("reader.zip");
            write_section1339_bookmark_zip(&path);
            app.current_folder = Some(path.clone());
            app.set_quick_folder_slot_target(super::super::QuickFolderSlotId::A, path.clone());
            let owner = arm_section1339_bookmark(&mut app, 303, &path);
            if adopted {
                assert!(app.begin_bookmark_page_wait(&owner));
            }
            assert_eq!(
                app.activate_quick_folder_slot(super::super::QuickFolderSlotId::A),
                super::super::QuickFolderSwitchTarget::Current
            );
            assert_eq!(
                app.bookmark_open_pending.is_some(),
                adopted,
                "same visible reader retains post-adoption hydration and its return target"
            );
            assert_eq!(app.bookmark_view_target().is_some(), adopted);
        }
    }

    #[test]
    fn section1339_bookmark_resolver_moves_original_navigation_through_row_refresh() {
        let mut app = setup_app_for_test();
        app.active_quick_folder_slot = None;
        app.settings.detached_viewer_open_images_in_window = false;
        let origin = app.tmp.path().join("origin");
        let target = app.tmp.path().join("book.zip");
        std::fs::create_dir(&origin).unwrap();
        write_section1339_bookmark_zip(&target);
        app.current_folder = Some(origin.clone());
        let owner = arm_section1339_bookmark(&mut app, 101, &target);
        let navigation = app.capture_main_list_navigation(
            MainHistoryOperation::Direct(crate::app::DirectNavigationPurpose::Navigation),
            MainListSourceProof::Bookmark(owner),
        );
        let original_route = navigation.source_location.as_ref().unwrap().route.clone();
        let ctx = egui::Context::default();
        app.start_startup_open_path_resolution_owned_with_navigation(
            target.clone(),
            StartupOpenPathSource::Bookmark,
            &ctx,
            Some(navigation),
        );
        assert!(
            app.startup_open_path_resolve_pending
                .as_ref()
                .unwrap()
                .navigation
                .is_some()
        );
        app.items_generation += 1;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.startup_open_path_resolve_pending.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "bookmark resolver did not complete"
            );
            app.poll_startup_open_path_resolve(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let Some(HistoryNavigationTransition::Physical(request)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("bookmark ZIP must retain the staged physical request");
        };
        assert!(
            matches!(&request.intent, PhysicalHistoryIntent::Bookmark { owner, .. } if owner.request_id.0 == 101)
        );
        assert_eq!(
            request.navigation.source_location.as_ref().unwrap().route,
            original_route
        );
        assert_eq!(
            request
                .navigation
                .source_location
                .as_ref()
                .unwrap()
                .location,
            FolderNavHistoryTarget::Path(origin)
        );
        assert!(app.physical_history_source_is_current(request));
        assert!(app.folder_nav_back_stack.is_empty());
        assert!(matches!(
            app.bookmark_open_pending
                .as_ref()
                .unwrap()
                .book()
                .unwrap()
                .stage,
            crate::bookmark_browser::PendingBookOpenStage::Resolving
        ));
    }

    #[test]
    fn section1339_stale_bookmark_cancel_cannot_remove_new_staged_owner() {
        let mut app = setup_app_for_test();
        app.active_quick_folder_slot = None;
        let target = app.tmp.path().join("book.zip");
        write_section1339_bookmark_zip(&target);
        let owner = arm_section1339_bookmark(&mut app, 202, &target);
        let navigation = app.capture_main_list_navigation(
            MainHistoryOperation::Direct(crate::app::DirectNavigationPurpose::Navigation),
            MainListSourceProof::Bookmark(owner.clone()),
        );
        assert!(app.start_physical_history_transition_classified(
            PhysicalHistoryIntent::Bookmark {
                owner,
                auto_fullscreen: true
            },
            target,
            None,
            Some(OpenPathKind::File),
            StartupListIntent::ExplicitList,
            navigation,
            None,
        ));
        assert!(!app.cancel_bookmark_open_request(
            crate::bookmark_browser::BookmarkOpenRequestId(201),
            "stale_test"
        ));
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(), Some(HistoryNavigationTransition::Physical(request)) if matches!(&request.intent, PhysicalHistoryIntent::Bookmark {owner, ..} if owner.request_id.0 == 202))
        );
        assert!(app.cancel_bookmark_open_request(
            crate::bookmark_browser::BookmarkOpenRequestId(202),
            "current_test"
        ));
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        assert!(app.bookmark_open_pending.is_none());
    }

    #[test]
    fn bookmark_open_resolver_rechecks_relative_page_containment() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("future.jpg"), b"outside").unwrap();
        if !create_dir_link(&outside, &album.join("link")) {
            return;
        }
        let bookmark = crate::book_bookmarks::BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&album),
            container_path: album.clone(),
            container_kind: crate::book_bookmarks::BookContainerKind::ImageFolder,
            page_identity: crate::book_bookmarks::PageIdentity::RelativePath(
                "link/future.jpg".to_string(),
            ),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        };

        let result = resolve_startup_open_path(
            album,
            StartupOpenPathSource::Bookmark,
            Some(&bookmark),
            true,
        );
        assert!(result.resolved.is_some());
        assert_eq!(result.bookmark_relative_page_openable, Some(false));
    }
}
