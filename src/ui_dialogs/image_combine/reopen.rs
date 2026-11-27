//! Recipe reads and existence checks belong to the request, never the UI thread.
use super::*;
use std::sync::{Arc, atomic::AtomicBool, mpsc};

pub(crate) struct ReopenRequest {
    request: u64,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<(u64, Result<Option<ReopenLoaded>, String>)>,
}

impl Drop for ReopenRequest {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct ReopenLoaded {
    request: u64,
    output: PathBuf,
    has_edits: bool,
    recipe: CombineRecipe,
    found: Vec<(CombineItemSpec, Result<resolver::SourcePreparation, String>)>,
    missing: Vec<CombineItemSpec>,
}

fn load_reopen(
    data_dir: &Path,
    output: PathBuf,
    has_edits: bool,
    request: u64,
    cancel: &AtomicBool,
) -> Result<Option<ReopenLoaded>, String> {
    let Some(recipe) = crate::image_combine::store::load_recipe(data_dir, &output)? else {
        return Ok(None);
    };
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for spec in &recipe.items {
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルしました".into());
        }
        if std::fs::metadata(source_path(&spec.source)).is_ok_and(|meta| meta.is_file()) {
            let prepared = resolver::read_source_preparation(data_dir, &spec.source);
            found.push((spec.clone(), prepared));
        } else {
            missing.push(spec.clone());
        }
    }
    Ok(Some(ReopenLoaded {
        request,
        output,
        has_edits,
        recipe,
        found,
        missing,
    }))
}

impl App {
    /// Check count before looking at any page keys or expanding stack members.
    fn image_combine_reopen_index(&self, clicked: Option<usize>, folder: bool) -> Option<usize> {
        let idx = if self.checked.is_empty() {
            if folder {
                return None;
            }
            clicked?
        } else if self.checked.len() == 1 {
            *self.checked.iter().next()?
        } else {
            return None;
        };
        matches!(
            self.items.get(idx),
            Some(crate::grid_item::GridItem::Image(_))
        )
        .then_some(idx)
    }

    pub(crate) fn image_combine_reopen_visible(
        &self,
        clicked: Option<usize>,
        folder: bool,
    ) -> bool {
        self.image_combine_reopen_index(clicked, folder)
            .and_then(|idx| self.items.get(idx))
            .is_some_and(|item| match item {
                crate::grid_item::GridItem::Image(path) => self
                    .image_combine
                    .index
                    .contains_or_unloaded(&crate::adjustment_db::normalize_path(path)),
                _ => false,
            })
    }

    pub(crate) fn image_combine_reopen_disabled_reason(
        &self,
        clicked: Option<usize>,
        folder: bool,
    ) -> Option<String> {
        if self.image_combine.session.is_some() {
            Some("結合の操作中です".into())
        } else if self.export_pending.is_some() {
            Some("エクスポート中です".into())
        } else if self.fullscreen_idx.is_some() {
            Some("一覧で画像を選んでください".into())
        } else if !self.grid_item_input_allowed() {
            Some("一覧の準備中です".into())
        } else if self.image_combine_reopen_index(clicked, folder).is_none() {
            Some("画像を 1 枚だけ選んでください".into())
        } else if !self.image_combine_reopen_visible(clicked, folder) {
            Some("この画像には結合の構成がありません".into())
        } else {
            None
        }
    }

    pub(crate) fn open_image_combine_reopen_context_dialog(
        &mut self,
        ctx: &egui::Context,
        clicked: Option<usize>,
        folder: bool,
    ) {
        if let Some(reason) = self.image_combine_reopen_disabled_reason(clicked, folder) {
            self.show_feedback_warning(reason);
            return;
        }
        let idx = self.image_combine_reopen_index(clicked, folder).unwrap();
        let crate::grid_item::GridItem::Image(output) = &self.items[idx] else {
            return;
        };
        let output = output.clone();
        let key = crate::adjustment_db::normalize_path(&output);
        let has_edits = self.adjustment_page_params.contains_key(&idx)
            || self.adjusted_page_keys.contains(&key)
            || self.mask_page_keys.contains(&key)
            || self.conceal_page_keys.contains(&key)
            || self.local_adjust_page_keys.contains(&key)
            || self.export_crop_page_keys.contains(&key)
            || self.export_crop_page_settings.contains_key(&idx)
            || self.comic_page_keys.contains(&key);
        self.image_combine.request_sequence = self.image_combine.request_sequence.wrapping_add(1);
        let request = self.image_combine.request_sequence;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let data_dir = crate::data_dir::get();
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("image-combine-reopen".into())
            .spawn(move || {
                let result = load_reopen(&data_dir, output, has_edits, request, &worker_cancel);
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send((request, result));
                    wake.request_repaint();
                }
            }) {
            Ok(_) => {
                self.image_combine.session =
                    Some(CombineSession::ReopenLoading(Box::new(ReopenRequest {
                        request,
                        cancel,
                        rx,
                    })))
            }
            Err(error) => self.show_feedback_error(format!("結合の構成を読み込めません: {error}")),
        }
        ctx.request_repaint();
    }

    pub(super) fn advance_image_combine_reopen(
        &mut self,
        ctx: &egui::Context,
        session: CombineSession,
    ) -> Option<CombineSession> {
        match session {
            CombineSession::ReopenLoading(pending) => {
                let mut open = true;
                let mut cancel = false;
                egui::Window::new("結合の構成を読み込み中")
                    .id(egui::Id::unique("image_combine_reopen_loading"))
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                    .show(ctx, |ui| {
                        ui.label("構成と元の画像を確認しています");
                        cancel = ui.button("キャンセル").clicked();
                    });
                if cancel || !open || self.dialog_escape_pressed(ctx) {
                    return None;
                }
                match pending.rx.try_recv() {
                    Ok((request, result))
                        if request == pending.request
                            && request == self.image_combine.request_sequence =>
                    {
                        match result {
                            Ok(None) => {
                                self.image_combine.index.mark_stale();
                                self.show_feedback_warning(
                                    "この画像には結合の構成がありません".into(),
                                );
                                None
                            }
                            Ok(Some(loaded)) => self.adopt_image_combine_reopen(ctx, loaded),
                            Err(error) => {
                                crate::logger::log(format!("image combine reopen: {error}"));
                                self.show_feedback_error(format!(
                                    "結合の構成を読み込めません: {error}"
                                ));
                                None
                            }
                        }
                    }
                    Ok(_) => None,
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        Some(CombineSession::ReopenLoading(pending))
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        crate::logger::log("image combine reopen worker disconnected");
                        self.show_feedback_error("結合の構成の読み込みが中断されました".into());
                        None
                    }
                }
            }
            CombineSession::ReopenMissing(loaded) => {
                let mut open = true;
                let mut accept = false;
                let mut cancel = false;
                egui::Window::new("見つからない元の画像")
                    .id(egui::Id::unique("image_combine_reopen_missing"))
                    .open(&mut open).collapsible(false).resizable(true)
                    .default_pos(ctx.content_rect().min + egui::vec2(20.0, 20.0))
                    .default_width(600.0_f32.min((ctx.content_rect().width() - 56.0).max(1.0)))
                    .max_width((ctx.content_rect().width() - 56.0).max(1.0))
                    .show(ctx, |ui| {
                        ui.label("次の画像は見つかりません。元の画像を移動・改名している場合は、元の場所へ戻してください。");
                        egui::ScrollArea::vertical().id_salt("image_combine_missing_sources")
                            .max_height((ctx.content_rect().height() - 230.0).max(40.0))
                            .auto_shrink([false, true]).show(ui, |ui| {
                                for item in &loaded.missing {
                                    ui.label(format!("{} ({})", source_name(&item.source, false), source_path(&item.source).display()));
                                }
                            });
                        ui.separator();
                        ui.horizontal_wrapped(|ui| {
                            accept = ui.button("見つからない画像を外して開く").clicked();
                            cancel = ui.button("キャンセル").clicked();
                        });
                    });
                if !open || cancel || self.dialog_escape_pressed(ctx) {
                    None
                } else if accept {
                    self.image_combine_dialog_from_recipe(ctx, *loaded)
                } else {
                    Some(CombineSession::ReopenMissing(loaded))
                }
            }
            session => Some(session),
        }
    }

    fn adopt_image_combine_reopen(
        &mut self,
        ctx: &egui::Context,
        loaded: ReopenLoaded,
    ) -> Option<CombineSession> {
        if loaded.found.is_empty() {
            self.show_feedback_warning(
                "元の画像がすべて見つからないため、結合をやり直せません".into(),
            );
            None
        } else if !loaded.missing.is_empty() {
            Some(CombineSession::ReopenMissing(Box::new(loaded)))
        } else {
            self.image_combine_dialog_from_recipe(ctx, loaded)
        }
    }

    fn image_combine_dialog_from_recipe(
        &mut self,
        ctx: &egui::Context,
        loaded: ReopenLoaded,
    ) -> Option<CombineSession> {
        let worker = match worker::spawn(
            loaded.found.len(),
            Some(crate::raw::RawDecodeContext::new(
                self.raw_develop_executor.clone(),
                self.settings.raw_brightness,
            )),
            self.local_ai_activity_lease(),
            ctx.clone(),
        ) {
            Ok(worker) => worker,
            Err(error) => {
                self.show_feedback_error(error);
                return None;
            }
        };
        let mut options = loaded.recipe.options.normalized();
        options.format = loaded.recipe.format;
        options.destination = Destination::Folder(
            loaded
                .output
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf(),
        );
        let split_text = match &options.split {
            Split::Auto => String::new(),
            Split::Counts(counts) => counts
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("-"),
        };
        let folder_text = loaded
            .output
            .parent()
            .unwrap_or(Path::new(""))
            .display()
            .to_string();
        let name = loaded
            .output
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let custom_ratio_text = options_ui::custom_ratio_text(&options);
        let mut pending = VecDeque::new();
        let lookup = resolver::ReopenLookup::new(
            self,
            loaded
                .found
                .iter()
                .map(|(spec, _)| resolver::source_page_key(&spec.source)),
        );
        let items = loaded
            .found
            .into_iter()
            .enumerate()
            .map(|(id, (spec, prepared))| {
                pending.push_back((
                    id,
                    PreparationTarget::Reopen {
                        source: spec.source.clone(),
                        annotation_space: spec.annotation_space,
                        composition_identity: spec.composition_identity.clone(),
                        mounted_idx: None,
                        prepared,
                    },
                ));
                DialogItem {
                    id,
                    name: source_name(&spec.source, false),
                    spec,
                    status: ItemStatus::Preparing,
                }
            })
            .collect::<Vec<_>>();
        if !self.items.is_empty() {
            pending.push_front((0, PreparationTarget::ReopenLookup(lookup)));
        }
        // A recipe can refer to a previous output. Keep export available with replacement OFF.
        let key = crate::adjustment_db::normalize_path(&loaded.output);
        let replace = !items.iter().any(|item| {
            crate::adjustment_db::normalize_path(source_path(&item.spec.source)) == key
        });
        Some(CombineSession::Dialog(Box::new(CombineDialogState {
            request: loaded.request,
            options,
            selected_item_id: items.first().map(|item| item.id),
            items,
            pending,
            reopened: Some(ReopenedOutput {
                path: loaded.output,
                format: loaded.recipe.format,
                replace,
                has_edits: loaded.has_edits,
            }),
            worker: Some(worker),
            dragging: None,
            preview: None,
            generation: 0,
            requested: None,
            validation: RefCell::new(None),
            #[cfg(test)]
            layout_searches: Default::default(),
            #[cfg(test)]
            spec_builds: Default::default(),
            render_error: None,
            error: None,
            split_text,
            custom_ratio_text,
            folder_text,
            name,
            preset: Default::default(),
        })))
    }
}

#[cfg(test)]
#[path = "image_combine_reopen_tests.rs"]
mod image_combine_reopen_tests;
