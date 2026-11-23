//! Drive inputs own one main-list request from admission through classified open.
//! Only the short-lived resolver worker probes paths. Rendering/acceptance use memory.

use super::*;
use crate::folder_tree::{OpenablePathKind, OpenablePathResolution};
use crate::ui_main::AddressBarNav;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DriveJumpRootOrigin {
    DriveLetter(String),
    CurrentLocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DriveJumpRequest {
    LastLocation {
        drive_key: String,
        remembered: Option<PathBuf>,
    },
    Root {
        path: PathBuf,
        origin: DriveJumpRootOrigin,
    },
}

impl DriveJumpRequest {
    fn kind(&self) -> &'static str {
        match self {
            Self::LastLocation { .. } => "last_location",
            Self::Root {
                origin: DriveJumpRootOrigin::DriveLetter(_),
                ..
            } => "drive_root",
            Self::Root {
                origin: DriveJumpRootOrigin::CurrentLocation,
                ..
            } => "current_root",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DriveJumpResolution {
    pub(super) target: Option<OpenablePathResolution>,
    pub(super) warnings: Vec<String>,
    pub(super) outcome: &'static str,
}

/// Stateless resolution policy, run only on a worker. A nearest existing parent on
/// the requested drive is a normal target, without a warning. Containers retain
/// their file classification; an archive-looking directory remains a directory.
pub(super) fn resolve_drive_jump(request: &DriveJumpRequest) -> DriveJumpResolution {
    resolve_drive_jump_with(request, crate::folder_tree::resolve_openable_path_detailed)
}

fn resolve_drive_jump_with(
    request: &DriveJumpRequest,
    mut resolve: impl FnMut(&Path) -> Option<OpenablePathResolution>,
) -> DriveJumpResolution {
    let mut warnings = Vec::new();
    let (target, fallback) = match request {
        DriveJumpRequest::LastLocation {
            drive_key,
            remembered,
        } => {
            if let Some(target) = remembered.as_ref().and_then(|path| resolve(path))
                && drive_current_key_for_path(&target.path).as_ref() == Some(drive_key)
            {
                return DriveJumpResolution {
                    target: Some(target),
                    warnings,
                    outcome: "target",
                };
            }
            if let Some(path) = remembered {
                warnings.push(format!(
                    "{} の最後の場所が見つかりません。ルートへ移動します: {}",
                    drive_key,
                    path.display()
                ));
            }
            let root = drive_root_path_for_letter(drive_key.chars().next().unwrap_or('\0'));
            let target = root.as_deref().and_then(&mut resolve);
            if target.is_none()
                && let Some(root) = root
            {
                warnings.push(format!("ドライブが見つかりません: {}", root.display()));
            }
            (target, remembered.is_some())
        }
        DriveJumpRequest::Root { path, origin } => {
            let target = resolve(path);
            if target.is_none() {
                warnings.push(match origin {
                    DriveJumpRootOrigin::DriveLetter(_) => {
                        format!("ドライブが見つかりません: {}", path.display())
                    }
                    DriveJumpRootOrigin::CurrentLocation => {
                        format!("ルートディレクトリが見つかりません: {}", path.display())
                    }
                });
            }
            (target, false)
        }
    };
    let outcome = if target.is_none() {
        "missing"
    } else if fallback {
        "fallback"
    } else {
        "target"
    };
    DriveJumpResolution {
        target,
        warnings,
        outcome,
    }
}

enum DriveJumpPhase {
    Resolving(mpsc::Receiver<DriveJumpResolution>),
    Ready(DriveJumpResolution),
}

/// Existing viewer-to-root navigation transport, before main admission. No
/// worker or main-list ticket belongs to the outgoing viewer payload.
#[derive(Debug)]
pub(crate) struct DriveJumpInput {
    request: DriveJumpRequest,
    source: &'static str,
    ctx: egui::Context,
}

pub(super) struct DriveJumpPending {
    pub(super) request: DriveJumpRequest,
    pub(super) navigation: MainListNavigation,
    pub(super) cancel: Arc<AtomicBool>,
    phase: DriveJumpPhase,
    started_at: Instant,
    perf_seq: u64,
}

impl Drop for DriveJumpPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
pub(super) type TestDriveJumpResolver =
    Arc<dyn Fn(&DriveJumpRequest) -> DriveJumpResolution + Send + Sync>;

fn trace_resolution(kind: &str, outcome: &str, elapsed: Duration, seq: u64) {
    crate::perf::event(
        "nav",
        "drive_jump_resolve",
        Some(kind),
        seq,
        &[
            ("ms", serde_json::json!(elapsed.as_secs_f64() * 1000.0)),
            ("request_kind", serde_json::json!(kind)),
            ("outcome", serde_json::json!(outcome)),
        ],
    );
}

impl App {
    pub(crate) fn apply_switch_drive_letter(
        &mut self,
        ctx: &egui::Context,
        letter: char,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        let drive_key = drive_current_key_for_letter(letter)?;
        let request = DriveJumpRequest::LastLocation {
            drive_key,
            remembered: self.active_drive_current_dir(letter),
        };
        self.start_drive_jump(ctx, request, source)
    }

    pub(crate) fn apply_ring_drive_letter(
        &mut self,
        ctx: &egui::Context,
        letter: char,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        let path = drive_root_path_for_letter(letter)?;
        let origin = DriveJumpRootOrigin::DriveLetter(drive_current_key_for_letter(letter)?);
        self.start_drive_jump(ctx, DriveJumpRequest::Root { path, origin }, source)
    }

    pub(crate) fn apply_current_drive_root(
        &mut self,
        ctx: &egui::Context,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        // Preserve the snapshot refusal even when the current location has no root.
        if self.location_jump_admission_blocked() {
            return None;
        }
        let Some(path) = self.current_location_root() else {
            self.show_feedback_warning("現在位置のルートディレクトリが見つかりません".into());
            return None;
        };
        self.start_drive_jump(
            ctx,
            DriveJumpRequest::Root {
                path,
                origin: DriveJumpRootOrigin::CurrentLocation,
            },
            source,
        )
    }

    fn start_drive_jump(
        &mut self,
        ctx: &egui::Context,
        request: DriveJumpRequest,
        source: &'static str,
    ) -> Option<AddressBarNav> {
        if !self.prepare_drive_jump_input(ctx) {
            return None;
        }
        // Main and separate viewers use the same candidate transport. No owner
        // is admitted until this input wins the existing root navigation arbiter.
        Some(AddressBarNav::DriveJump(DriveJumpInput {
            request,
            source,
            ctx: ctx.clone(),
        }))
    }

    fn prepare_drive_jump_input(&mut self, ctx: &egui::Context) -> bool {
        // Refusal and completed close precede candidate creation. Losing a
        // candidate must never retire a previously admitted main-list request.
        if self.location_jump_admission_blocked() || self.remote_session_blocks_local_control() {
            return false;
        }
        if self.projected_viewer_session_open() {
            let close_failed = self.close_drive_jump_viewer(ctx);
            if close_failed || self.projected_viewer_session_open() {
                self.show_feedback_warning(
                    "全画面を閉じられなかったため移動できませんでした".into(),
                );
                return false;
            }
        }
        true
    }

    pub(super) fn dispatch_drive_jump_input(&mut self, input: DriveJumpInput) {
        let DriveJumpInput {
            request,
            source,
            ctx,
        } = input;
        // Recheck the winning input on main: separate-viewer candidates were
        // prepared with the outgoing payload, before returning to this owner.
        if self.prepare_drive_jump_input(&ctx) {
            self.admit_drive_jump(&ctx, request, source);
        }
    }

    fn admit_drive_jump(
        &mut self,
        ctx: &egui::Context,
        request: DriveJumpRequest,
        source: &'static str,
    ) {
        if self.location_jump_admission_blocked() || self.remote_session_blocks_local_control() {
            return;
        }
        let history = MainHistoryOperation::Direct(
            if self.global_search.active || self.favsearch.active || self.tag_view.active {
                DirectNavigationPurpose::ExitTemporaryView {
                    selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
                }
            } else {
                DirectNavigationPurpose::Navigation
            },
        );
        let navigation =
            self.capture_main_list_navigation(history, self.copied_destination_source_proof());
        self.retire_replaced_main_list_requests(Some(MainListRequestOwner::DriveJump));
        // Retaining the owner only retains the handoff, never a previous request.
        self.retire_drive_jump();
        self.cancel_saved_group_open_for_other_nav();
        self.bump_input_seq(source, Some(&format!("drive_jump={request:?}")));
        self.spawn_drive_jump(ctx, request, navigation);
    }

    fn close_drive_jump_viewer(&mut self, ctx: &egui::Context) -> bool {
        #[cfg(test)]
        if let Some(backend) = self.drive_jump_close_backend {
            return backend(self, ctx);
        }
        #[cfg(windows)]
        {
            match self.close_fullscreen_to_completion(ctx) {
                Ok(()) => false,
                Err(error) => {
                    crate::logger::log(format!("[drive-jump] fullscreen close failed: {error:?}"));
                    true
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            self.close_fullscreen();
            false
        }
    }

    fn spawn_drive_jump(
        &mut self,
        ctx: &egui::Context,
        request: DriveJumpRequest,
        navigation: MainListNavigation,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = cancel.clone();
        let request_w = request.clone();
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        let started_at = Instant::now();
        let perf_seq = self.input_seq;
        #[cfg(test)]
        let resolver = self.drive_jump_resolver.clone();
        let spawned = std::thread::Builder::new()
            .name("drive-jump-resolve".into())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    trace_resolution(request_w.kind(), "stale", started_at.elapsed(), perf_seq);
                    return;
                }
                #[cfg(test)]
                let resolution = resolver.as_ref().map_or_else(
                    || resolve_drive_jump(&request_w),
                    |resolver| resolver(&request_w),
                );
                #[cfg(not(test))]
                let resolution = resolve_drive_jump(&request_w);
                let resolved_outcome = resolution.outcome;
                let delivered = !cancel_w.load(Ordering::Relaxed) && tx.send(resolution).is_ok();
                let outcome = if delivered && !cancel_w.load(Ordering::Relaxed) {
                    resolved_outcome
                } else {
                    "stale"
                };
                trace_resolution(request_w.kind(), outcome, started_at.elapsed(), perf_seq);
                if delivered {
                    wake.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.drive_jump_pending = Some(DriveJumpPending {
                    request,
                    navigation,
                    cancel,
                    phase: DriveJumpPhase::Resolving(rx),
                    started_at,
                    perf_seq,
                });
                // Reuse the existing delayed direct-navigation indicator.
                ctx.request_repaint_after(Duration::from_millis(500));
            }
            Err(error) => {
                crate::logger::log(format!("[drive-jump] worker start failed: {error}"));
                self.show_feedback_warning("ドライブへの移動を開始できませんでした".into());
            }
        }
    }

    pub(super) fn retire_drive_jump(&mut self) {
        self.drive_jump_pending = None;
    }

    /// Successful later admissions use this boundary. Continuations keep their
    /// original owner; detached admissions must not retire the main request.
    pub(super) fn retire_direct_location_requests_for_admission(&mut self) {
        self.retire_drive_jump();
        if self.direct_navigation_scan_pending() {
            self.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
        }
    }

    pub(crate) fn drive_jump_pending(&self) -> bool {
        self.main_folder_history_available() && self.drive_jump_pending.is_some()
    }
    pub(crate) fn drive_jump_elapsed(&self) -> Option<Duration> {
        self.drive_jump_pending().then(|| {
            self.drive_jump_pending
                .as_ref()
                .unwrap()
                .started_at
                .elapsed()
        })
    }

    /// Root pump runs before fullscreen early returns. Ready remains part of the
    /// same owner until the normal input-priority arbiter consumes it.
    pub(super) fn poll_drive_jump(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.drive_jump_pending.as_ref() else {
            return;
        };
        if self.projected_viewer_session_open()
            || self.remote_session_blocks_local_control()
            || !self.main_list_navigation_is_current(&pending.navigation)
        {
            trace_resolution(
                pending.request.kind(),
                "stale",
                pending.started_at.elapsed(),
                pending.perf_seq,
            );
            self.retire_drive_jump();
            return;
        }
        let result = match &pending.phase {
            DriveJumpPhase::Ready(_) => return,
            DriveJumpPhase::Resolving(rx) => rx.try_recv(),
        };
        match result {
            Ok(resolution) => {
                self.drive_jump_pending.as_mut().unwrap().phase = DriveJumpPhase::Ready(resolution);
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.retire_drive_jump();
                crate::logger::log("[drive-jump] resolver disconnected");
                self.show_feedback_warning("ドライブへの移動を解決できませんでした".into());
            }
        }
    }

    pub(super) fn drive_jump_ready(&self) -> bool {
        self.drive_jump_pending
            .as_ref()
            .is_some_and(|p| matches!(p.phase, DriveJumpPhase::Ready(_)))
    }

    pub(super) fn adopt_ready_drive_jump(&mut self) {
        if !self.drive_jump_ready() {
            return;
        }
        let pending = self.drive_jump_pending.take().unwrap();
        // Recheck after search widgets and same-frame inputs, before handoff.
        if self.projected_viewer_session_open()
            || self.remote_session_blocks_local_control()
            || !self.main_list_navigation_is_current(&pending.navigation)
        {
            trace_resolution(
                pending.request.kind(),
                "stale",
                pending.started_at.elapsed(),
                pending.perf_seq,
            );
            return;
        }
        let DriveJumpPhase::Ready(resolution) = &pending.phase else {
            unreachable!()
        };
        for warning in &resolution.warnings {
            self.show_feedback_warning(warning.clone());
        }
        let Some(target) = &resolution.target else {
            return;
        };
        let kind = match target.kind {
            OpenablePathKind::Directory => OpenPathKind::Directory,
            OpenablePathKind::File => OpenPathKind::File,
        };
        let navigation = pending.navigation.clone();
        if matches!(
            &navigation.history,
            MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView { .. })
        ) {
            self.begin_context_jump_to_folder_classified(
                target.path.clone(),
                kind,
                navigation,
                MainListRequestStage::Continuation,
            );
        } else {
            self.open_direct_navigation_target_classified(
                target.path.clone(),
                None,
                OpenRequestOwner::Navigation,
                Some(navigation),
                None,
                Some(kind),
                StartupListIntent::ExplicitList,
                MainListRequestStage::Continuation,
                DirectoryScanRoute::Worker,
            );
        }
    }

    pub(crate) fn toolbar_drive_keys(&self) -> Vec<String> {
        self.quick_folder_workspaces
            .iter()
            .flat_map(|workspace| workspace.drive_current_dirs.keys())
            .filter(|key| {
                key.len() == 2
                    && key.as_bytes()[1] == b':'
                    && key.as_bytes()[0].is_ascii_uppercase()
            })
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn current_toolbar_drive_key(&self) -> Option<String> {
        if self.global_search.active
            || self.favsearch.active
            || self.tag_view.active
            || self.items_are_global_search_view
            || self.items_are_tag_view
            || self.items_are_rating_view
            || self.items_are_drive_list
            || self.is_snapshot_active()
            || !matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
        {
            return None;
        }
        drive_current_key_for_path(&self.effective_folder()?)
    }

    pub(crate) fn toolbar_drive_tooltip(&self, key: &str) -> String {
        self.active_quick_folder_workspace()
            .and_then(|workspace| workspace.drive_current_dirs.get(key))
            .map_or_else(
                || "記録なし (ルートを開きます)".into(),
                |path| path.display().to_string(),
            )
    }

    pub(crate) fn remove_toolbar_drive(&mut self, key: &str) {
        if self.current_toolbar_drive_key().as_deref() == Some(key) {
            return;
        }
        for workspace in &mut self.quick_folder_workspaces {
            workspace.drive_current_dirs.remove(key);
        }
        self.sync_quick_folder_settings();
        self.settings.save();
    }
}

#[cfg(test)]
pub(super) fn finish_drive_jump_for_test(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.drive_jump_pending() {
        app.poll_drive_jump(ctx);
        app.post_render_main_navigation(ctx, update_phases::MainFrameNavigation::default());
        assert!(Instant::now() < deadline, "drive resolver did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(all(test, windows))]
mod resolution_tests {
    use super::*;

    fn last_location(path: Option<PathBuf>, key: &str) -> DriveJumpRequest {
        DriveJumpRequest::LastLocation {
            drive_key: key.into(),
            remembered: path,
        }
    }

    fn directory(path: &Path) -> OpenablePathResolution {
        OpenablePathResolution {
            path: path.to_path_buf(),
            kind: OpenablePathKind::Directory,
            requested_is_file: false,
        }
    }

    #[test]
    fn drive_jump_resolver_keeps_existing_directory_and_nearest_parent_without_warning() {
        let temp = tempfile::tempdir().unwrap();
        let key = drive_current_key_for_path(temp.path()).unwrap();
        let nested = temp.path().join("parent");
        std::fs::create_dir(&nested).unwrap();
        for path in [&nested, &nested.join("deleted").join("child")] {
            let resolved = resolve_drive_jump(&last_location(Some(path.to_owned()), &key));
            assert_eq!(resolved.target.unwrap(), directory(&nested));
            assert!(resolved.warnings.is_empty());
            assert_eq!(resolved.outcome, "target");
        }
    }

    #[test]
    fn drive_jump_resolver_preserves_container_kinds_and_deleted_container_parent() {
        let temp = tempfile::tempdir().unwrap();
        let key = drive_current_key_for_path(temp.path()).unwrap();
        for name in ["book.zip", "book.pdf", "book.cbr", "book.epub"] {
            let path = temp.path().join(name);
            std::fs::write(&path, b"fixture").unwrap();
            let resolved = resolve_drive_jump(&last_location(Some(path.clone()), &key));
            let target = resolved.target.unwrap();
            assert_eq!(target.path, path);
            assert_eq!(target.kind, OpenablePathKind::File);
            assert!(resolved.warnings.is_empty());
            std::fs::remove_file(&path).unwrap();
            let removed = resolve_drive_jump(&last_location(Some(path), &key));
            assert_eq!(removed.target.unwrap(), directory(temp.path()));
            assert!(removed.warnings.is_empty());
        }
        let path = temp.path().join("directory.pdf");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            resolve_drive_jump(&last_location(Some(path.clone()), &key))
                .target
                .unwrap(),
            directory(&path)
        );
    }

    #[test]
    fn drive_jump_resolver_missing_record_opens_root_without_warning() {
        let request = last_location(None, "D:");
        let mut calls = Vec::new();
        let result = resolve_drive_jump_with(&request, |path| {
            calls.push(path.to_path_buf());
            Some(directory(path))
        });
        assert_eq!(calls, [PathBuf::from(r"D:\")]);
        assert_eq!(result.target.unwrap().path, PathBuf::from(r"D:\"));
        assert!(result.warnings.is_empty());
        assert_eq!(result.outcome, "target");
    }

    #[test]
    fn drive_jump_resolver_inaccessible_or_wrong_drive_record_falls_back_once() {
        for other_drive in [false, true] {
            let remembered = PathBuf::from(r"D:\missing\book.zip");
            let request = last_location(Some(remembered.clone()), "D:");
            let mut calls = Vec::new();
            let result = resolve_drive_jump_with(&request, |path| {
                calls.push(path.to_path_buf());
                if path == remembered.as_path() {
                    other_drive.then(|| directory(Path::new(r"E:\elsewhere")))
                } else {
                    Some(directory(path))
                }
            });
            assert_eq!(calls, [remembered, PathBuf::from(r"D:\")]);
            assert_eq!(result.target.unwrap().path, PathBuf::from(r"D:\"));
            assert_eq!(
                result.warnings,
                [r"D: の最後の場所が見つかりません。ルートへ移動します: D:\missing\book.zip"]
            );
            assert_eq!(result.outcome, "fallback");
        }
    }

    #[test]
    fn drive_jump_resolver_missing_root_preserves_warning_text_and_order() {
        let request = last_location(Some(PathBuf::from(r"D:\missing")), "D:");
        let result = resolve_drive_jump_with(&request, |_| None);
        assert_eq!(result.outcome, "missing");
        assert!(result.target.is_none());
        assert_eq!(
            result.warnings,
            [
                r"D: の最後の場所が見つかりません。ルートへ移動します: D:\missing",
                r"ドライブが見つかりません: D:\",
            ]
        );
        for (origin, warning) in [
            (
                DriveJumpRootOrigin::DriveLetter("D:".into()),
                r"ドライブが見つかりません: D:\",
            ),
            (
                DriveJumpRootOrigin::CurrentLocation,
                r"ルートディレクトリが見つかりません: D:\",
            ),
        ] {
            let result = resolve_drive_jump_with(
                &DriveJumpRequest::Root {
                    path: PathBuf::from(r"D:\"),
                    origin,
                },
                |_| None,
            );
            assert!(result.target.is_none());
            assert_eq!(result.warnings, [warning]);
            assert_eq!(result.outcome, "missing");
        }
    }

    #[test]
    fn drive_jump_resolver_current_root_keeps_unc_and_verbatim_unc_path() {
        for path in [r"\\server\share\", r"\\?\UNC\server\share\"] {
            let path = PathBuf::from(path);
            let result = resolve_drive_jump_with(
                &DriveJumpRequest::Root {
                    path: path.clone(),
                    origin: DriveJumpRootOrigin::CurrentLocation,
                },
                |requested| {
                    assert_eq!(requested, path);
                    Some(directory(requested))
                },
            );
            assert_eq!(result.target.unwrap().path, path);
            assert!(result.warnings.is_empty());
        }
    }
}
