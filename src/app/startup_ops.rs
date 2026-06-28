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

    pub(super) fn start_startup_open_path_resolve(
        &mut self,
        requested: PathBuf,
        source: StartupOpenPathSource,
        ctx: &egui::Context,
    ) {
        if let Some(pending) = self.startup_open_path_resolve_pending.take() {
            crate::logger::log(format!(
                "startup open: cancel pending resolve source={} requested={}",
                pending.source.perf_tag(),
                pending.requested.display()
            ));
            drop(pending);
        }

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let worker_requested = requested.clone();
        let repaint_ctx = ctx.clone();
        let spawn_result = std::thread::Builder::new()
            .name("startup-open-resolve".to_string())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let result = resolve_startup_open_path(worker_requested, source);
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
                    source,
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
                let result = resolve_startup_open_path(requested, source);
                self.finish_startup_open_path_resolve(source, result);
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
                let source = pending.source;
                drop(pending);
                self.finish_startup_open_path_resolve(source, result);
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
                let source = pending.source;
                crate::logger::log(format!(
                    "startup open: resolve worker disconnected source={} requested={}",
                    source.perf_tag(),
                    pending.requested.display()
                ));
                drop(pending);
                if matches!(source, StartupOpenPathSource::InitialStartup) {
                    self.open_default_startup_target();
                } else {
                    self.show_feedback_toast("パスの確認を完了できませんでした".to_string());
                }
                ctx.request_repaint();
            }
        }
    }

    fn finish_startup_open_path_resolve(
        &mut self,
        source: StartupOpenPathSource,
        result: StartupOpenPathResolveResult,
    ) {
        let requested_display = result.requested.display().to_string();
        if self.apply_startup_open_path_resolve_result(result) {
            return;
        }
        if matches!(source, StartupOpenPathSource::InitialStartup) {
            self.open_default_startup_target();
        } else {
            crate::logger::log(format!(
                "startup open: activation open failed for {requested_display}"
            ));
            self.show_feedback_toast("開けるパスが見つかりませんでした".to_string());
        }
    }

    fn apply_startup_open_path_resolve_result(
        &mut self,
        result: StartupOpenPathResolveResult,
    ) -> bool {
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
        let auto_fullscreen =
            startup_openable_should_auto_fullscreen(&self.settings, &openable, resolution.kind);
        let outcome =
            self.load_folder_or_convert_archive_with_auto_fullscreen(openable, auto_fullscreen);
        if matches!(outcome, FolderOpenOutcome::Ignored) {
            return false;
        }
        if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
            self.open_startup_file_if_visible(&result.requested);
        }
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
        let Some(idx) = self.items.iter().position(|item| match item {
            GridItem::Folder(path)
            | GridItem::Image(path)
            | GridItem::Video(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path) => crate::folder_tree::path_eq(path, requested),
            GridItem::ConvertibleArchive { path, .. } => {
                crate::folder_tree::path_eq(path, requested)
            }
            _ => false,
        }) else {
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
            self.open_fullscreen(idx);
        }
    }
}

pub(crate) fn startup_file_should_open_fullscreen(item: &GridItem) -> bool {
    matches!(item, GridItem::Image(_) | GridItem::Video(_))
}

pub(crate) fn startup_openable_should_auto_fullscreen(
    settings: &crate::settings::Settings,
    openable: &Path,
    kind: crate::folder_tree::OpenablePathKind,
) -> bool {
    if !settings.auto_fullscreen_zip_pdf {
        return false;
    }
    match kind {
        crate::folder_tree::OpenablePathKind::File => {
            crate::folder_tree::is_virtual_folder(openable)
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
) -> StartupOpenPathResolveResult {
    let t0 = std::time::Instant::now();
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
        elapsed_ms,
    }
}
