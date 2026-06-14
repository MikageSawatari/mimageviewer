use super::*;

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
            "single_instance: opening forwarded path on UI thread: {}",
            path.display()
        ));
        let _ = self.open_startup_path(path);
        ctx.request_repaint();
    }

    pub(super) fn open_startup_path(&mut self, requested: PathBuf) -> bool {
        // perf 計装 (post-v1.3.0 backlog C-3): 転送パス activate 経路 (稼働中 UI スレッド)
        // で `resolve_openable_path` が is_file/is_dir + 親探索の FS stat を行う。遅い /
        // 切断ネットワークパスで stall しうるので、worker 化判断のため所要時間を計測する。
        // (本コミットは計装のみ。実際の worker 化は計測値を見て別途判断。)
        let resolve_t0 = crate::perf::is_enabled().then(std::time::Instant::now);
        let resolved = crate::folder_tree::resolve_openable_path(&requested);
        if let Some(t0) = resolve_t0 {
            crate::perf::event(
                "startup",
                "open_path_resolve",
                None,
                0,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("resolved", serde_json::Value::from(resolved.is_some())),
                ],
            );
        }
        let Some(openable) = resolved else {
            crate::logger::log(format!(
                "startup open: no openable path for {}",
                requested.display()
            ));
            return false;
        };

        crate::logger::log(format!(
            "startup open: requested={} resolved={}",
            requested.display(),
            openable.display()
        ));

        let select_requested_file = requested.is_file() && openable.is_dir();
        let auto_fullscreen = startup_openable_should_auto_fullscreen(&self.settings, &openable);
        let outcome =
            self.load_folder_or_convert_archive_with_auto_fullscreen(openable, auto_fullscreen);
        if matches!(outcome, FolderOpenOutcome::Ignored) {
            return false;
        }
        if select_requested_file && matches!(outcome, FolderOpenOutcome::Loaded) {
            self.open_startup_file_if_visible(&requested);
        }
        true
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
) -> bool {
    settings.auto_fullscreen_zip_pdf
        && openable.is_file()
        && (crate::folder_tree::is_virtual_folder(openable)
            || crate::folder_tree::is_convertible_archive_path(openable))
}
