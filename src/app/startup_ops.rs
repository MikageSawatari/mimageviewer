use super::*;

const STARTUP_OPEN_PATH_RESOLVE_TOAST_DELAY: std::time::Duration =
    std::time::Duration::from_millis(400);

impl App {
    pub(crate) fn set_startup_open_path(&mut self, path: PathBuf) {
        self.startup_open_path = Some(path);
    }

    #[cfg(windows)]
    pub(super) fn poll_activation_open_paths(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = self.activation_open_path_rx.try_iter().collect();
        let Some(path) = paths.into_iter().last() else {
            return;
        };
        crate::logger::log(format!(
            "single_instance: resolving forwarded path in worker: {}",
            path.display()
        ));
        self.start_startup_open_path_resolve(path, StartupOpenPathSource::Activation, ctx);
        ctx.request_repaint();
    }

    pub(crate) fn start_startup_open_path_resolve(
        &mut self,
        requested: PathBuf,
        source: StartupOpenPathSource,
        ctx: &egui::Context,
    ) {
        let Some(owner) = self.startup_open_path_owner(source) else {
            crate::logger::log(format!(
                "startup open: reject ownerless resolve source={} requested={}",
                source.perf_tag(),
                requested.display()
            ));
            return;
        };
        // A forwarded activation is a new navigation request as soon as it is received. Do not
        // let an archive conversion owned by the currently opening bookmark win the race while
        // the activation path is still being resolved on its worker.
        if matches!(source, StartupOpenPathSource::Activation) {
            self.cancel_archive_convert_for_navigation("activation_navigation");
            if let Some(request_id) = self
                .bookmark_open_pending
                .as_ref()
                .map(crate::bookmark_browser::PendingBookmarkOpen::request_id)
            {
                self.cancel_bookmark_open_request(request_id, "activation_navigation");
            }
        }
        if let Some(pending) = self.startup_open_path_resolve_pending.take() {
            crate::logger::log(format!(
                "startup open: cancel pending resolve source={} requested={}",
                pending.owner.perf_tag(),
                pending.requested.display()
            ));
            let previous_owner = pending.owner.clone();
            drop(pending);
            self.finish_replaced_startup_open_owner(previous_owner, "resolver_replaced");
        }

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
        let spawn_result = std::thread::Builder::new()
            .name("startup-open-resolve".to_string())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let result =
                    resolve_startup_open_path(worker_requested, source, worker_bookmark.as_ref());
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
                repaint_ctx.request_repaint();
            });

        match spawn_result {
            Ok(_) => {
                self.startup_open_path_resolve_pending = Some(StartupOpenPathResolvePending {
                    requested,
                    owner,
                    cancel,
                    rx,
                    started_at: std::time::Instant::now(),
                    toast_shown: false,
                });
            }
            Err(e) => {
                crate::logger::log(format!(
                    "startup open: resolve worker spawn failed: {e}; running synchronously"
                ));
                let result = resolve_startup_open_path(requested, source, bookmark.as_ref());
                self.finish_startup_open_path_resolve(owner, result, ctx);
            }
        }
    }

    pub(super) fn poll_startup_open_path_resolve(&mut self, ctx: &egui::Context) {
        let recv = match self.startup_open_path_resolve_pending.as_ref() {
            Some(pending) => pending.rx.try_recv(),
            None => return,
        };

        match recv {
            Ok(result) => {
                let pending = self.startup_open_path_resolve_pending.take().unwrap();
                let owner = pending.owner.clone();
                drop(pending);
                self.finish_startup_open_path_resolve(owner, result, ctx);
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
                let pending = self.startup_open_path_resolve_pending.take().unwrap();
                let owner = pending.owner.clone();
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
                    }
                    self.show_feedback_toast("パスの確認を完了できませんでした".to_string());
                }
                ctx.request_repaint();
            }
        }
    }

    pub(super) fn finish_startup_open_path_resolve(
        &mut self,
        owner: StartupOpenPathOwner,
        result: StartupOpenPathResolveResult,
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
        let source = owner.source();
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
        if self.apply_startup_open_path_resolve_result(&owner, result, ctx) {
            return;
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

    fn startup_open_path_owner(
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
                    crate::bookmark_browser::BookmarkOpenRequestOwner { request_id, target },
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
        self.bookmark_open_pending
            .as_ref()
            .is_some_and(|pending| pending.request_id() == owner.request_id)
            && self.bookmark_view_target() == Some(&owner.target)
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

    /// A normal navigation supersedes an unresolved startup/activation/bookmark open. The
    /// resolver is removed before its receiver can be polled; bookmark-owned state is cleared
    /// only when its request ID is still current.
    pub(crate) fn cancel_unresolved_open_for_navigation(&mut self) {
        let Some(pending) = self.startup_open_path_resolve_pending.take() else {
            return;
        };
        crate::logger::log(format!(
            "startup open: navigation cancels resolve source={} requested={}",
            pending.owner.perf_tag(),
            pending.requested.display()
        ));
        let owner = pending.owner.clone();
        drop(pending);
        self.finish_replaced_startup_open_owner(owner, "normal_navigation");
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

    /// End every current lifecycle component belonging to exactly one bookmark request.
    /// A stale A cancellation is therefore unable to clear a newer B request.
    pub(crate) fn cancel_bookmark_open_request(
        &mut self,
        request_id: crate::bookmark_browser::BookmarkOpenRequestId,
        reason: &'static str,
    ) -> bool {
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
        if resolver_matches {
            drop(self.startup_open_path_resolve_pending.take());
        }
        let archive_matches = self.cancel_archive_convert_for_bookmark_request(request_id);
        let pending_matches = self
            .bookmark_open_pending
            .as_ref()
            .is_some_and(|pending| pending.request_id() == request_id);
        if !pending_matches {
            return resolver_matches || archive_matches;
        }
        crate::logger::log(format!(
            "[bookmark-open] finish request id={} reason={reason}",
            request_id.0
        ));
        self.bookmark_open_pending = None;
        self.clear_bookmark_view_return_state();
        true
    }

    fn apply_startup_open_path_resolve_result(
        &mut self,
        owner: &StartupOpenPathOwner,
        result: StartupOpenPathResolveResult,
        ctx: &egui::Context,
    ) -> bool {
        let source = owner.source();
        let Some(resolution) = result.resolved else {
            crate::logger::log(format!(
                "startup open: no openable path for {}",
                result.requested.display()
            ));
            return false;
        };

        let openable = resolution.path;
        crate::logger::log(format!(
            "startup open: requested={} resolved={} resolve_ms={:.1}",
            result.requested.display(),
            openable.display(),
            result.elapsed_ms
        ));

        let select_requested_file = resolution.requested_is_file
            && matches!(
                resolution.kind,
                crate::folder_tree::OpenablePathKind::Directory
            );
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
            return self.open_bookmark_media_in_detached_context(
                ctx,
                &result.requested,
                openable,
                select_requested_file,
            );
        }
        #[cfg(windows)]
        if matches!(source, StartupOpenPathSource::Bookmark)
            && self.settings.detached_viewer_open_images_in_window
            && let Some(opened) =
                self.open_bookmark_book_in_detached_context(ctx, openable.clone(), resolution.kind)
        {
            return opened;
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
            return false;
        }
        let auto_fullscreen = matches!(source, StartupOpenPathSource::Bookmark)
            || startup_openable_should_auto_fullscreen(&self.settings, &openable, resolution.kind);
        let outcome = self.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            openable,
            auto_fullscreen,
            owner.open_request_owner(),
        );
        if matches!(outcome, FolderOpenOutcome::Ignored) {
            return false;
        }
        if matches!(source, StartupOpenPathSource::Bookmark)
            && matches!(outcome, FolderOpenOutcome::Loaded)
            && let StartupOpenPathOwner::Bookmark(bookmark_owner) = owner
        {
            self.begin_bookmark_page_wait(bookmark_owner);
        }
        if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
            self.open_startup_file_if_visible(&result.requested);
        }
        true
    }

    /// Route a bookmark-backed book into the same independent context seam used by normal
    /// PDF/ZIP grid opens. `None` means the container still needs the archive-conversion flow;
    /// `Some` means this method owns the request and main must not load the container.
    #[cfg(windows)]
    pub(super) fn open_bookmark_book_in_detached_context(
        &mut self,
        ctx: &egui::Context,
        openable: PathBuf,
        kind: crate::folder_tree::OpenablePathKind,
    ) -> Option<bool> {
        let pending = self
            .bookmark_open_pending
            .as_ref()
            .and_then(crate::bookmark_browser::PendingBookmarkOpen::book)?;
        let target_matches = matches!(
            self.bookmark_view_state.as_ref(),
            Some(BookmarkViewState::Opening {
                target: crate::bookmark_browser::BookmarkViewReturnTarget::Book(path),
                ..
            }) if crate::path_key::eq_keep_drive(path, &pending.bookmark.container_path)
        );
        if !target_matches {
            return None;
        }

        let descriptor = match pending.bookmark.container_kind {
            crate::book_bookmarks::BookContainerKind::Pdf
                if matches!(kind, crate::folder_tree::OpenablePathKind::File) =>
            {
                ViewerContextDescriptor::Pdf {
                    path: openable,
                    page_num: None,
                }
            }
            crate::book_bookmarks::BookContainerKind::Zip
                if matches!(kind, crate::folder_tree::OpenablePathKind::File) =>
            {
                ViewerContextDescriptor::Zip {
                    path: openable,
                    entry_name: None,
                    archive_source_override: None,
                }
            }
            crate::book_bookmarks::BookContainerKind::CompiledBook
            | crate::book_bookmarks::BookContainerKind::ImageFolder
                if matches!(kind, crate::folder_tree::OpenablePathKind::Directory) =>
            {
                ViewerContextDescriptor::BookFolder { path: openable }
            }
            crate::book_bookmarks::BookContainerKind::OtherArchive => {
                let source = pending.bookmark.container_path.clone();
                let cached = self.try_archive_cache_lookup(&source)?;
                ViewerContextDescriptor::Zip {
                    path: cached,
                    entry_name: None,
                    archive_source_override: Some(source),
                }
            }
            _ => return None,
        };

        let pending = match self.bookmark_open_pending.take() {
            Some(crate::bookmark_browser::PendingBookmarkOpen::Book(pending)) => pending,
            other => {
                self.bookmark_open_pending = other;
                return None;
            }
        };
        let base_placement = self.active_detached_viewer_current_placement();
        let had_active_detached =
            self.active_detached_viewer_context.is_some() || self.viewer_session_is_detached();
        if !self.park_and_close_current_active_detached_viewer(ctx) {
            self.bookmark_open_pending =
                Some(crate::bookmark_browser::PendingBookmarkOpen::Book(pending));
            return Some(false);
        }
        let placement_seed = had_active_detached
            .then(|| self.offset_detached_image_window_placement(base_placement));
        Some(self.start_active_detached_book_context(
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
        let active_media = self.active_detached_viewer_context_contains_video();
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
            let Some(active) = self.active_detached_viewer_context.as_mut() else {
                self.bookmark_open_pending =
                    Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
                return false;
            };
            active.bundle.bookmark_open_pending =
                Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
            active.bundle.bookmark_view_state = Some(BookmarkViewState::Detached { target });
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

        let mut main_context = self.take_current_viewer_context_bundle();
        let context_serial = self.assign_next_detached_viewer_context_generation();
        self.reset_active_detached_viewport_runtime_for_new_window(
            context_serial,
            "bookmark_media_detached_context",
        );
        self.bookmark_open_pending =
            Some(crate::bookmark_browser::PendingBookmarkOpen::Media(pending));
        self.bookmark_view_state = Some(BookmarkViewState::Detached { target });

        let outcome = self.with_detached_viewer_main_history_suppressed(|app| {
            let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen(openable, true);
            if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
                app.open_startup_file_if_visible(requested);
            }
            outcome
        });
        let opened_target = matches!(outcome, FolderOpenOutcome::Loaded)
            && self
                .fullscreen_idx
                .and_then(|idx| self.items.get(idx))
                .is_some_and(|item| match item {
                    GridItem::Video(path) | GridItem::Audio(path) => {
                        crate::path_key::eq_keep_drive(path, requested)
                    }
                    _ => false,
                });

        if !opened_target {
            let session_window_id = self
                .active_detached_session
                .map(|session| session.window_id);
            let closing_window_id = session_window_id.or(self.detached_viewer_window_id);
            self.begin_active_detached_session_close("bookmark_media_detached_open_failed");
            self.finish_active_detached_session_close("bookmark_media_detached_open_failed");
            self.close_fullscreen();
            if session_window_id.is_none()
                && let Some(window_id) = closing_window_id
            {
                self.remove_detached_window_runtime(
                    window_id,
                    "bookmark_media_detached_open_failed",
                );
            }
            let _failed_context = self.take_current_viewer_context_bundle();
            self.swap_viewer_context_bundle(&mut main_context);
            return false;
        }

        let active_context = self.take_current_viewer_context_bundle();
        self.swap_viewer_context_bundle(&mut main_context);
        self.active_detached_viewer_context = Some(ActiveDetachedViewerContext {
            bundle: active_context,
        });
        crate::logger::log(format!(
            "[bookmark-open] detached media context started path={} main_bookmarks={}",
            requested.display(),
            self.items_are_bookmark_view
        ));
        ctx.request_repaint();
        true
    }

    pub(super) fn open_default_startup_target(&mut self) {
        if self.settings.startup_folder_mode == crate::settings::StartupFolderMode::ReadingHistory {
            self.enter_reading_history();
        } else if should_start_in_drive_list(&self.settings) {
            self.enter_drive_list(None);
        } else if let Some(folder) = crate::known_folders::startup_folder(
            self.settings.startup_folder_mode,
            self.settings.last_folder.as_deref(),
            self.settings.startup_folder_path.as_deref(),
        ) {
            // last_folder には変換アーカイブ (RAR/CBR/7z/LZH) の元パスが入りうるので、
            // load_folder ではなく load_folder_or_convert_archive を通す。キャッシュが
            // あれば open_archive_via_cache が元アーカイブを開き直し (current_folder は
            // キャッシュ ZIP だが address/override は元アーカイブ)、無ければ変換ダイアログを
            // 出す。通常フォルダ / ネイティブ ZIP/PDF は format=None なので load_folder に委譲され挙動不変。
            let _ = self.load_folder_or_convert_archive(folder);
        }
    }

    fn open_startup_file_if_visible(&mut self, requested: &Path) {
        let Some(idx) = startup_file_idx(&self.items, requested) else {
            crate::logger::log(format!(
                "startup open: requested file not found in loaded folder: {}",
                requested.display()
            ));
            return;
        };
        self.selected = Some(idx);
        self.scroll_to_selected = true;
        if startup_file_should_open_fullscreen(&self.items[idx]) {
            crate::logger::log(format!(
                "startup open: opening requested file in fullscreen: {}",
                requested.display()
            ));
            self.bump_input_seq_for_item("startup_open_file", idx);
            self.fs_open_intent_from_grid = true;
            self.open_fullscreen(idx, crate::app::HistoryTrigger::UserChosen);
        }
    }
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
        elapsed_ms,
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

        let result =
            resolve_startup_open_path(album, StartupOpenPathSource::Bookmark, Some(&bookmark));
        assert!(result.resolved.is_some());
        assert_eq!(result.bookmark_relative_page_openable, Some(false));
    }
}
