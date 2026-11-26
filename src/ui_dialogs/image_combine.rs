//! Grid-only combine session. UI captures snapshots within the batch 6ms budget.
use super::export_batch::ExportBatchTarget;
use crate::app::App;
use crate::image_combine::{
    worker::{self, Command, Event, RenderSpec},
    *,
};
use eframe::egui;
use std::cell::{Ref, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[cfg(test)]
mod image_combine_tests;
mod options_ui;
mod presets;
mod reopen;
mod resolver;

enum PreparationTarget {
    Selection(ExportBatchTarget),
    ReopenLookup(resolver::ReopenLookup),
    Reopen {
        source: CombineSourceRef,
        annotation_space: Option<[u32; 2]>,
        composition_identity: Option<crate::book_resume_db::PageIdentity>,
        mounted_idx: Option<usize>,
        prepared: Result<resolver::SourcePreparation, String>,
    },
}

struct ReopenedOutput {
    path: PathBuf,
    format: CombineFormat,
    replace: bool,
    has_edits: bool,
}

pub(crate) enum CombineSession {
    ReopenLoading(Box<reopen::ReopenRequest>),
    ReopenMissing(Box<reopen::ReopenLoaded>),
    Dialog(Box<CombineDialogState>),
    Writing { request: u64 },
}

/// App keeps one pointer; session, index and menu snapshots share this owner.
#[derive(Default)]
pub(crate) struct ImageCombineState {
    pub(crate) session: Option<CombineSession>,
    pub(crate) request_sequence: u64,
    pub(crate) index: crate::image_combine::store::CombineIndex,
    /// Snapshot owned by one open Tools menu; cleared when its body closes.
    pub(crate) tools_menu_owner: Option<super::context_menu::GridContextMenuOwner>,
    #[cfg(test)]
    pub(crate) eligibility_work: std::cell::Cell<CombineEligibilityWork>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CombineEligibilityWork {
    pub passes: usize,
    pub rows: usize,
    pub stack_lookups: usize,
    pub materializations: usize,
    pub page_keys: usize,
}

impl CombineSession {
    pub(crate) fn modal(&self) -> bool {
        !matches!(self, Self::Writing { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ItemStatus {
    Preparing,
    Loading,
    Loaded((u32, u32)),
    Failed(String),
}

#[derive(Clone, PartialEq, Eq)]
struct DialogItem {
    id: usize,
    spec: CombineItemSpec,
    name: String,
    status: ItemStatus,
}

struct Preview {
    texture: egui::TextureHandle,
    layout: Layout,
    ids: Vec<usize>,
    generation: u64,
    not_drawn: usize,
    kind: CombineLayout,
    direction: CombineDirection,
}

/// A draft's actual validation/render inputs, including text that has not parsed
/// into options yet. Compare by reference on paints; snapshot only on a change.
struct ValidationInputs {
    options: CombineOptions,
    items: Vec<DialogItem>,
    preparing: bool,
    split_text: String,
    custom_ratio_text: Option<String>,
}

impl ValidationInputs {
    fn matches(&self, state: &CombineDialogState) -> bool {
        self.options == state.options
            && self.items == state.items
            && self.preparing == !state.pending.is_empty()
            && self.split_text == state.split_text
            && self.custom_ratio_text == state.custom_ratio_text
    }

    fn capture(state: &CombineDialogState) -> Self {
        Self {
            options: state.options.clone(),
            items: state.items.clone(),
            preparing: !state.pending.is_empty(),
            split_text: state.split_text.clone(),
            custom_ratio_text: state.custom_ratio_text.clone(),
        }
    }
}

struct ValidatedDraft {
    inputs: ValidationInputs,
    revision: u64,
    result: Result<Layout, String>,
}

/// Only successfully submitted render commands belong here, never invalid drafts.
struct SubmittedPreview {
    revision: u64,
    spec: RenderSpec,
}

pub(crate) struct CombineDialogState {
    request: u64,
    options: CombineOptions,
    items: Vec<DialogItem>,
    pending: VecDeque<(usize, PreparationTarget)>,
    reopened: Option<ReopenedOutput>,
    /// Taken on transfer to Writing; Drop otherwise cancels the dialog worker.
    worker: Option<worker::Connection>,
    /// Stable image ID inside this dialog, independent of App's grid cursor.
    selected_item_id: Option<usize>,
    dragging: Option<usize>,
    preview: Option<Preview>,
    generation: u64,
    requested: Option<SubmittedPreview>,
    validation: RefCell<Option<ValidatedDraft>>,
    #[cfg(test)]
    layout_searches: std::cell::Cell<usize>,
    #[cfg(test)]
    spec_builds: std::cell::Cell<usize>,
    render_error: Option<String>,
    error: Option<String>,
    split_text: String,
    custom_ratio_text: Option<String>,
    folder_text: String,
    name: String,
    preset: presets::PresetUi,
}

impl Drop for CombineDialogState {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl CombineDialogState {
    fn replacement_path(&self) -> Option<&Path> {
        self.reopened
            .as_ref()
            .filter(|output| output.replace)
            .map(|output| output.path.as_path())
    }

    fn replacement_is_source(&self) -> bool {
        self.reopened.as_ref().is_some_and(|output| {
            let key = crate::adjustment_db::normalize_path(&output.path);
            self.items.iter().any(|item| {
                crate::adjustment_db::normalize_path(source_path(&item.spec.source)) == key
            })
        })
    }

    /// Preset recall and toggling replacement use the same fixed-target boundary.
    fn enforce_replacement_target(&mut self) {
        let Some(output) = self.reopened.as_ref().filter(|output| output.replace) else {
            return;
        };
        self.options.format = match (output.format, self.options.format) {
            (CombineFormat::Jpeg { .. }, CombineFormat::Jpeg { quality }) => {
                CombineFormat::Jpeg { quality }
            }
            (CombineFormat::Webp { lossless, .. }, CombineFormat::Webp { quality, .. }) => {
                CombineFormat::Webp { quality, lossless }
            }
            (format, _) => format,
        };
        self.options.destination =
            Destination::Folder(output.path.parent().unwrap_or(Path::new("")).to_path_buf());
        self.folder_text = output
            .path
            .parent()
            .unwrap_or(Path::new(""))
            .display()
            .to_string();
        self.name = output
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }

    fn spec(&self) -> RenderSpec {
        #[cfg(test)]
        self.spec_builds.set(self.spec_builds.get() + 1);
        RenderSpec {
            options: self.options.normalized(),
            items: self
                .items
                .iter()
                .map(|item| (item.id, item.spec.clone()))
                .collect(),
        }
    }

    fn ready(&self) -> bool {
        !self.items.is_empty()
            && self.pending.is_empty()
            && self
                .items
                .iter()
                .all(|item| matches!(item.status, ItemStatus::Loaded(_)))
    }

    fn validation(&self) -> Ref<'_, ValidatedDraft> {
        let mut cached = self.validation.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|draft| !draft.inputs.matches(self))
        {
            let revision = cached.as_ref().map_or(1, |draft| draft.revision + 1);
            *cached = Some(ValidatedDraft {
                inputs: ValidationInputs::capture(self),
                revision,
                result: self.compute_validation(),
            });
        }
        drop(cached);
        Ref::map(self.validation.borrow(), |cached| cached.as_ref().unwrap())
    }

    fn compute_validation(&self) -> Result<Layout, String> {
        if let Some(item) = self
            .items
            .iter()
            .find(|item| matches!(item.status, ItemStatus::Failed(_)))
        {
            return Err(format!("読めない画像を外してください: {}", item.name));
        }
        if !self.ready() {
            return Err("画像を読み込み中です".into());
        }
        if matches!(
            self.options.layout,
            CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns
        ) && matches!(self.options.split, Split::Counts(_))
        {
            parse_counts(&self.split_text, self.items.len())?;
        }
        if let Some(text) = &self.custom_ratio_text {
            parse_ratio(text)?;
        }
        let dims = self
            .items
            .iter()
            .filter_map(|item| {
                if let ItemStatus::Loaded(dims) = item.status {
                    Some(dims)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        #[cfg(test)]
        self.layout_searches.set(self.layout_searches.get() + 1);
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let result =
            layout(&self.options, &dims, self.options.labels.band_height()).map_err(|error| {
                match error {
                    LayoutError::CanvasPixelLimit
                    | LayoutError::WebpSideLimit
                    | LayoutError::JpegSideLimit => {
                        format!("{error}。サイズを小さくするか、形式を変えてください")
                    }
                    _ => error.to_string(),
                }
            });
        if let Some(started) = started {
            crate::perf::event(
                "image_combine/layout",
                "complete",
                None,
                self.generation,
                &[
                    ("items", serde_json::json!(dims.len())),
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        result
    }

    fn output_dir(&self) -> PathBuf {
        match &self.options.destination {
            Destination::FirstImageFolder => self
                .items
                .first()
                .and_then(|item| source_path(&item.spec.source).parent())
                .unwrap_or(Path::new(""))
                .to_path_buf(),
            Destination::Folder(path) => path.clone(),
        }
    }

    fn can_write(&self) -> bool {
        self.validation().result.is_ok()
            && self.render_error.is_none()
            && self
                .preview
                .as_ref()
                .is_some_and(|p| p.generation == self.generation)
            && self.output_validation().is_ok()
    }

    fn output_validation(&self) -> Result<(), String> {
        if self.replacement_path().is_some() && self.replacement_is_source() {
            return Err("元の結合画像が素材に含まれるため、置き換えできません".into());
        }
        if self.name.trim().is_empty() {
            return Err("出力の名前を入力してください".into());
        }
        if self.output_dir().as_os_str().is_empty() {
            return Err("保存先フォルダを指定してください".into());
        }
        Ok(())
    }

    fn invalidate_and_request(&mut self, ctx: &egui::Context) {
        let (revision, valid) = {
            let draft = self.validation();
            (draft.revision, draft.result.is_ok())
        };
        if !valid
            || self
                .requested
                .as_ref()
                .is_some_and(|sent| sent.revision == revision)
        {
            return;
        }
        let Some(worker) = &self.worker else {
            return;
        };
        let spec = self.spec();
        self.generation = self.generation.wrapping_add(1);
        self.render_error = None;
        #[cfg(test)]
        crate::ime_focus::typing_probe::job(
            "image_combine_label",
            self.typing_label(),
            crate::ime_focus::typing_probe::Outcome::Started,
        );
        if worker
            .tx
            .send(Command::Render {
                generation: self.generation,
                spec: spec.clone(),
            })
            .is_err()
        {
            self.render_error = Some("結合 worker が中断されました".into());
        } else {
            self.requested = Some(SubmittedPreview { revision, spec });
        }
        ctx.request_repaint();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        // Collect event values only; at most the matching generation can upload.
        let mut rendered = None;
        loop {
            match worker.rx.try_recv() {
                Ok(Event::Loaded { id, dims }) => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.status = ItemStatus::Loaded(dims);
                    }
                }
                Ok(Event::Failed { id, message }) => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                        item.status = ItemStatus::Failed(message);
                    }
                }
                Ok(Event::Rendered {
                    generation,
                    rgba,
                    layout,
                    not_drawn,
                }) if generation == self.generation => {
                    rendered = Some((generation, rgba, layout, not_drawn));
                }
                Ok(Event::RenderFailed {
                    generation,
                    message,
                }) if generation == self.generation => self.render_error = Some(message),
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.render_error = Some("結合 worker が中断されました".into());
                    break;
                }
            }
        }
        if let Some((generation, rgba, layout, not_drawn)) = rendered {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            self.preview = Some(Preview {
                texture: ctx.load_texture(
                    "image_combine_preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ),
                layout,
                ids: self
                    .requested
                    .as_ref()
                    .map(|sent| sent.spec.items.iter().map(|(id, _)| *id).collect())
                    .unwrap_or_default(),
                generation,
                not_drawn,
                kind: self
                    .requested
                    .as_ref()
                    .map_or(self.options.layout, |sent| sent.spec.options.layout),
                direction: self
                    .requested
                    .as_ref()
                    .map_or(self.options.direction, |sent| sent.spec.options.direction),
            });
            #[cfg(test)]
            crate::ime_focus::typing_probe::job(
                "image_combine_label",
                self.typing_label(),
                crate::ime_focus::typing_probe::Outcome::Delivered(self.items.len()),
            );
        }
    }

    fn move_item(&mut self, id: usize, before: Option<usize>) {
        if before == Some(id) {
            return;
        }
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return;
        };
        let item = self.items.remove(index);
        let destination = before
            .and_then(|id| self.items.iter().position(|item| item.id == id))
            .unwrap_or(self.items.len());
        self.items.insert(destination, item);
    }

    fn remove_failed(&mut self) {
        let ids = self
            .items
            .iter()
            .filter(|item| matches!(item.status, ItemStatus::Failed(_)))
            .map(|item| item.id)
            .collect::<Vec<_>>();
        self.items.retain(|item| !ids.contains(&item.id));
        if let Some(worker) = &self.worker {
            let _ = worker.tx.send(Command::Remove { ids });
        }
        if !self
            .items
            .iter()
            .any(|item| Some(item.id) == self.selected_item_id)
        {
            self.selected_item_id = self.items.first().map(|item| item.id);
        }
    }

    #[cfg(test)]
    fn typing_label(&self) -> &str {
        self.items
            .iter()
            .find(|item| Some(item.id) == self.selected_item_id)
            .and_then(|item| item.spec.label_override.as_deref())
            .unwrap_or("")
    }
}

fn source_path(source: &CombineSourceRef) -> &Path {
    match source {
        CombineSourceRef::File { path } => path,
        CombineSourceRef::ZipEntry { zip_path, .. } => zip_path,
        CombineSourceRef::PdfPage { pdf_path, .. } => pdf_path,
    }
}

fn source_name(source: &CombineSourceRef, stem: bool) -> String {
    let path = match source {
        CombineSourceRef::File { path } => path.as_path(),
        CombineSourceRef::ZipEntry { entry_name, .. } => Path::new(entry_name),
        CombineSourceRef::PdfPage { pdf_path, page_num } => {
            return format!(
                "{} p{}",
                pdf_path.file_stem().unwrap_or_default().to_string_lossy(),
                page_num + 1
            );
        }
    };
    if stem {
        path.file_stem()
    } else {
        path.file_name()
    }
    .unwrap_or_default()
    .to_string_lossy()
    .into_owned()
}

fn source_for_target(app: &App, target: &ExportBatchTarget) -> Option<CombineSourceRef> {
    use crate::grid_item::GridItem;
    match target {
        ExportBatchTarget::StackMember(path) => Some(CombineSourceRef::File { path: path.clone() }),
        ExportBatchTarget::Item { idx, .. } => match app.items.get(*idx)? {
            GridItem::Image(path) => Some(CombineSourceRef::File { path: path.clone() }),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => Some(CombineSourceRef::ZipEntry {
                zip_path: zip_path.clone(),
                entry_name: entry_name.clone(),
            }),
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => Some(CombineSourceRef::PdfPage {
                pdf_path: pdf_path.clone(),
                page_num: *page_num,
            }),
            _ => None,
        },
    }
}

fn parse_counts(text: &str, total: usize) -> Result<Vec<u32>, String> {
    let counts = text
        .split('-')
        .map(|s| {
            s.trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=100).contains(n))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("行または列の枚数を 1〜100 の数で指定してください (例 2-2)")?;
    if counts.iter().map(|n| *n as usize).sum::<usize>() != total {
        return Err("行または列の枚数が一致しません".into());
    }
    Ok(counts)
}

fn parse_ratio(text: &str) -> Result<(u32, u32), String> {
    text.split_once(':')
        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)))
        .filter(|(w, h)| (1..=100).contains(w) && (1..=100).contains(h))
        .ok_or_else(|| "比率は W:H (各 1〜100) で指定してください".into())
}

pub(crate) fn entry_disabled_reason(
    count: usize,
    session: bool,
    exporting: bool,
) -> Option<String> {
    if session {
        Some("結合の操作中です".into())
    } else if exporting {
        Some("エクスポート中です".into())
    } else if count == 0 {
        Some("選択の中に結合できる画像・ページがありません".into())
    } else if count > 100 {
        Some("結合できるのは 100 枚までです (101 枚以上選択中)".into())
    } else {
        None
    }
}

impl App {
    #[cfg(test)]
    pub(crate) fn prepare_image_combine_typing_for_test(&mut self) {
        if let Some(CombineSession::Dialog(state)) = &mut self.image_combine.session {
            if let Some(item) = state.items.first_mut() {
                item.spec.label_override = Some(String::new());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn image_combine_typing_state_for_test(&self) -> (&str, bool, usize) {
        if let Some(CombineSession::Dialog(state)) = &self.image_combine.session {
            (
                state.typing_label(),
                state
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.generation == state.generation),
                state.items.len(),
            )
        } else {
            ("", false, 0)
        }
    }
    pub(crate) fn combine_dialog_modal(&self) -> bool {
        self.image_combine
            .session
            .as_ref()
            .is_some_and(CombineSession::modal)
    }

    pub(crate) fn grid_combine_disabled_reason(&self) -> Option<String> {
        self.combine_disabled_reason_for_count(
            self.image_combine_target_count(self.selected, false),
        )
    }

    /// Admission only: no sorting, page keys, member paths or target queues.
    /// 101 represents every over-limit selection, not an exact cardinality.
    pub(crate) fn image_combine_target_count(
        &self,
        clicked: Option<usize>,
        background: bool,
    ) -> usize {
        #[cfg(test)]
        self.note_combine_work(|work| work.passes += 1);
        if background {
            return 0;
        }
        let mut count = 0usize;
        let single = clicked.filter(|_| self.checked.is_empty());
        for index in self.checked.iter().copied().chain(single) {
            #[cfg(test)]
            self.note_combine_work(|work| work.rows += 1);
            if !self.smart_folder_rule_qualifies_index(index) {
                continue;
            }
            count += match self.items.get(index) {
                Some(
                    crate::grid_item::GridItem::Image(_)
                    | crate::grid_item::GridItem::ZipImage { .. }
                    | crate::grid_item::GridItem::PdfPage { .. },
                ) => 1,
                Some(crate::grid_item::GridItem::Stack { key, .. }) => {
                    #[cfg(test)]
                    self.note_combine_work(|work| work.stack_lookups += 1);
                    self.stack_view
                        .as_ref()
                        .map_or(0, |view| view.image_count_for_group_key(key))
                }
                _ => 0,
            };
            if count > 100 {
                return 101;
            }
        }
        count
    }

    #[cfg(test)]
    pub(crate) fn note_combine_work(&self, update: impl FnOnce(&mut CombineEligibilityWork)) {
        let mut work = self.image_combine.eligibility_work.get();
        update(&mut work);
        self.image_combine.eligibility_work.set(work);
    }

    pub(crate) fn combine_tools_menu_disabled_reason(&mut self) -> Option<String> {
        let context_id = self.collection_grid_context_id();
        if self.image_combine.tools_menu_owner.is_none_or(|owner| {
            owner.context_id != context_id || owner.items_generation != self.items_generation
        }) {
            self.image_combine.tools_menu_owner = Some(super::context_menu::GridContextMenuOwner {
                index: self.selected.unwrap_or(usize::MAX),
                context_id,
                items_generation: self.items_generation,
                combine_target_count: self.image_combine_target_count(self.selected, false),
            });
        }
        self.combine_disabled_reason_for_count(
            self.image_combine
                .tools_menu_owner
                .unwrap()
                .combine_target_count,
        )
    }

    pub(crate) fn combine_disabled_reason_for_count(&self, count: usize) -> Option<String> {
        entry_disabled_reason(
            count,
            self.image_combine.session.is_some(),
            self.export_pending.is_some(),
        )
        .or_else(|| (!self.grid_item_input_allowed()).then(|| "一覧の準備中です".into()))
    }

    pub(crate) fn open_image_combine_dialog(&mut self, ctx: &egui::Context) {
        if let Some(reason) = self.grid_combine_disabled_reason() {
            self.show_feedback_warning(reason);
            return;
        }
        let (targets, _) = self.grid_batch_export_targets();
        self.open_image_combine_with_targets(ctx, targets);
    }

    pub(crate) fn image_combine_context_targets(
        &self,
        clicked: Option<usize>,
        is_folder_context: bool,
    ) -> VecDeque<ExportBatchTarget> {
        let indices = if is_folder_context {
            Vec::new()
        } else if self.checked.is_empty() {
            clicked.into_iter().collect()
        } else {
            self.grid_selection_indices()
        };
        self.grid_batch_export_targets_for_indices(indices).0
    }

    pub(crate) fn open_image_combine_context_dialog(
        &mut self,
        ctx: &egui::Context,
        clicked: Option<usize>,
        is_folder_context: bool,
    ) {
        if let Some(reason) = self.combine_disabled_reason_for_count(
            self.image_combine_target_count(clicked, is_folder_context),
        ) {
            self.show_feedback_warning(reason);
            return;
        }
        let targets = self.image_combine_context_targets(clicked, is_folder_context);
        self.open_image_combine_with_targets(ctx, targets);
    }

    fn open_image_combine_with_targets(
        &mut self,
        ctx: &egui::Context,
        targets: VecDeque<ExportBatchTarget>,
    ) {
        if let Some(reason) = self.combine_disabled_reason_for_count(targets.len()) {
            self.show_feedback_warning(reason);
            return;
        }
        let items = targets
            .iter()
            .enumerate()
            .map(|(id, target)| {
                let source =
                    source_for_target(self, target).ok_or("結合する画像を特定できません")?;
                Ok(DialogItem {
                    id,
                    name: source_name(&source, false),
                    spec: CombineItemSpec {
                        composition_identity: None,
                        annotation_space: None,
                        source,
                        label_override: None,
                    },
                    status: ItemStatus::Preparing,
                })
            })
            .collect::<Result<Vec<_>, &str>>();
        let items = match items {
            Ok(items) => items,
            Err(error) => {
                self.show_feedback_error(error.into());
                return;
            }
        };
        let worker = match worker::spawn(
            items.len(),
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
                return;
            }
        };
        self.image_combine.request_sequence = self.image_combine.request_sequence.wrapping_add(1);
        let options = self
            .settings
            .combine_options
            .clone()
            .unwrap_or_default()
            .normalized();
        let split_text = match &options.split {
            Split::Auto => String::new(),
            Split::Counts(counts) => counts
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("-"),
        };
        let folder_text = match &options.destination {
            Destination::Folder(path) => path.display().to_string(),
            _ => String::new(),
        };
        let name = format!("{}_結合", source_name(&items[0].spec.source, true));
        let custom_ratio_text = options_ui::custom_ratio_text(&options);
        self.image_combine.session = Some(CombineSession::Dialog(Box::new(CombineDialogState {
            request: self.image_combine.request_sequence,
            options,
            selected_item_id: items.first().map(|item| item.id),
            items,
            pending: targets
                .into_iter()
                .enumerate()
                .map(|(id, target)| (id, PreparationTarget::Selection(target)))
                .collect(),
            reopened: None,
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
        })));
        ctx.request_repaint();
    }

    /// Apply the shared grid shortcut gate before consuming any re-entry chord.
    pub(crate) fn reject_busy_image_combine_shortcut(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_none()
            && ctx.viewport_id() == egui::ViewportId::ROOT
            && (self.image_combine.session.is_some() || self.export_pending.is_some())
            && !self.shortcuts_blocked_by_text_input(ctx)
            && !self.ime_input_active(ctx)
            && !egui::Popup::is_any_open(ctx)
            && (self
                .keymap
                .consume_action(ctx, crate::keymap::KeyAction::GridCombineSelection)
                || self
                    .keymap
                    .consume_action(ctx, crate::keymap::KeyAction::GridCombineReopen))
        {
            let reason = if self.image_combine.session.is_some() {
                "結合の操作中です"
            } else {
                "エクスポート中です"
            };
            self.show_feedback_warning(reason.into());
        }
    }

    fn advance_image_combine_preparation(&mut self, state: &mut CombineDialogState) {
        let start = std::time::Instant::now();
        while let Some((id, target)) = state.pending.pop_front() {
            let target = match target {
                PreparationTarget::ReopenLookup(mut lookup) => {
                    let result = lookup.advance(self, || {
                        start.elapsed() < std::time::Duration::from_millis(6)
                    });
                    match result {
                        Ok(false) => {
                            state
                                .pending
                                .push_front((id, PreparationTarget::ReopenLookup(lookup)));
                            break;
                        }
                        result => {
                            for (_, target) in &mut state.pending {
                                if let PreparationTarget::Reopen {
                                    source,
                                    mounted_idx,
                                    prepared,
                                    ..
                                } = target
                                {
                                    match &result {
                                        Ok(_) => *mounted_idx = lookup.get(source),
                                        Err(error) => *prepared = Err(error.clone()),
                                    }
                                }
                            }
                            if start.elapsed() >= std::time::Duration::from_millis(6) {
                                break;
                            }
                            continue;
                        }
                    }
                }
                target => target,
            };
            let result = match target {
                PreparationTarget::ReopenLookup(_) => unreachable!(),
                PreparationTarget::Selection(target) => self
                    .batch_export_item_for_target(&target)
                    .and_then(|item| item.ok_or_else(|| "この対象は結合できません".into())),
                PreparationTarget::Reopen {
                    source,
                    annotation_space,
                    composition_identity,
                    mounted_idx,
                    prepared,
                } => prepared.and_then(|prepared| {
                    self.image_combine_item_for_source(
                        &source,
                        &prepared,
                        annotation_space,
                        composition_identity.as_ref(),
                        mounted_idx,
                    )
                }),
            };
            let status = match result {
                Ok(item) => match state.worker.as_ref().unwrap().tx.send(Command::Prepare {
                    id,
                    item: Box::new(item),
                }) {
                    Ok(()) => ItemStatus::Loading,
                    Err(_) => ItemStatus::Failed("結合 worker が中断されました".into()),
                },
                Err(error) => {
                    crate::logger::log(format!("image combine prepare {id}: {error}"));
                    ItemStatus::Failed(error)
                }
            };
            if let Some(item) = state.items.iter_mut().find(|item| item.id == id) {
                item.status = status;
            }
            if start.elapsed() >= std::time::Duration::from_millis(6) {
                break;
            }
        }
    }

    pub(crate) fn draw_image_combine_dialog(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.image_combine.index.poll(&crate::data_dir::get(), ctx) {
            self.show_feedback_warning(format!("結合の構成の一覧を読み込めません: {error}"));
        }
        if self.image_combine.session.is_some() {
            self.reject_busy_image_combine_shortcut(ctx);
        }
        let Some(session) = self.image_combine.session.take() else {
            return;
        };
        let session = match self.advance_image_combine_reopen(ctx, session) {
            Some(session) => session,
            None => return,
        };
        let CombineSession::Dialog(mut state) = session else {
            self.image_combine.session = Some(session);
            return;
        };
        self.advance_image_combine_preparation(&mut state);
        state.poll(ctx);
        state.invalidate_and_request(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let enter = self.dialog_enter_pressed(ctx);
        let action = draw_window_with_enter(ctx, &mut state, &self.settings.combine_presets, enter);
        if escape || action.cancel {
            return;
        }
        if let Some(action) = action.preset {
            self.apply_combine_preset(&mut state, action);
        }
        state.enforce_replacement_target();
        if action.pick_folder {
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(state.output_dir())
                .pick_folder()
            {
                state.folder_text = path.display().to_string();
                state.options.destination = Destination::Folder(path);
            }
        }
        state.invalidate_and_request(ctx);
        if action.write && state.can_write() {
            let spec = state.spec();
            match state.worker.as_ref().unwrap().write_with_target(
                spec,
                state.output_dir(),
                state.name.clone(),
                state.replacement_path().map(Path::to_path_buf),
                crate::data_dir::get(),
            ) {
                Ok(pending) => {
                    self.settings.combine_options = Some(state.options.clone());
                    if !self.settings.save_checked() {
                        self.show_feedback_error("結合の設定を保存できませんでした".into());
                    }
                    state.worker.take(); // Writing now owns cancellation through ExportPending.
                    self.export_pending = Some(pending);
                    self.image_combine.session = Some(CombineSession::Writing {
                        request: state.request,
                    });
                    return;
                }
                Err(error) => state.error = Some(error),
            }
        }
        if !state.ready() && state.render_error.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.image_combine.session = Some(CombineSession::Dialog(state));
    }

    /// Session take is the exactly-once latch, even when completed progress stays open.
    pub(crate) fn finish_combine_write(&mut self, ctx: &egui::Context) {
        if !matches!(
            self.image_combine.session,
            Some(CombineSession::Writing { .. })
        ) {
            return;
        }
        let session = self.image_combine.session.take();
        if let Some(CombineSession::Writing { request }) = session {
            crate::logger::log(format!("image combine finished request={request}"));
        }
        let path = self
            .export_pending
            .as_ref()
            .filter(|pending| pending.end == Some(crate::export_dialog::ExportEnd::Done))
            .and_then(|pending| pending.successes.first())
            .map(|success| success.path.clone());
        if let Some(path) = path {
            self.image_combine.index.mark_stale();
            self.refresh_image_combine_output(ctx, &path);
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            self.show_saved_output_feedback(format!("結合しました: {name}"));
            self.fs_feedback_toast_action =
                Some(crate::feedback::FeedbackAction::OpenInViewer(path));
            self.fs_feedback_toast_surface = None;
        }
    }
}

#[derive(Default)]
struct Action {
    cancel: bool,
    write: bool,
    pick_folder: bool,
    preset: Option<presets::PresetAction>,
    field_enter: bool,
}

fn trailing_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|label| {
            egui::WidgetText::from(*label)
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Button,
                )
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x
        })
        .sum()
}

fn text(ui: &mut egui::Ui, text: &mut String, salt: &str, width: f32) -> egui::Response {
    let mut output = crate::ime_focus::show_singleline(ui, text, None, |edit| {
        edit.id(egui::Id::unique(salt)).desired_width(width)
    });
    let label = match salt {
        "image_combine_label" => "ラベル",
        "image_combine_name" => "名前",
        "image_combine_folder" => "保存先フォルダ",
        "image_combine_split" => "行・列の枚数",
        "image_combine_ratio" => "指定の比率",
        "image_combine_preset_name" => "プリセット名",
        _ => salt,
    };
    output.response.response = output.response.response.accessible_name(label);
    crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, text);
    output.response.response
}

fn draw_window(
    ctx: &egui::Context,
    state: &mut CombineDialogState,
    presets: &[CombinePreset],
) -> Action {
    let enter = !crate::ime_focus::ime_input_active(ctx)
        && ctx.input(|input| input.key_pressed(egui::Key::Enter));
    draw_window_with_enter(ctx, state, presets, enter)
}

fn draw_window_with_enter(
    ctx: &egui::Context,
    state: &mut CombineDialogState,
    presets: &[CombinePreset],
    enter: bool,
) -> Action {
    state.enforce_replacement_target();
    let mut action = Action {
        field_enter: enter,
        ..Default::default()
    };
    let mut open = true;
    let screen = ctx.content_rect();
    let margin = egui::vec2(20.0, 20.0);
    let available_size = egui::vec2(
        (screen.width() - 56.0).max(1.0),
        (screen.height() - 2.0 * margin.y).max(1.0),
    );
    let size = egui::vec2(1000.0, 680.0).min(available_size);
    egui::Window::new(format!("画像を結合 ({} 枚)", state.items.len()))
        .id(egui::Id::unique("image_combine_dialog"))
        .collapsible(false)
        .resizable(true)
        .default_pos(screen.min + margin)
        .default_size(size)
        .min_size(egui::vec2(300.0, 220.0).min(available_size))
        .max_size(available_size)
        .open(&mut open)
        .show(ctx, |ui| {
            #[cfg(test)]
            let header_top = ui.cursor().top();
            presets::draw(ui, state, presets, &mut action);
            state.enforce_replacement_target();
            ui.separator();
            let content_rect = ui.available_rect_before_wrap();
            #[cfg(test)]
            let header_rect = egui::Rect::from_min_max(
                egui::pos2(content_rect.left(), header_top),
                content_rect.right_top(),
            );
            // Lay out the real fixed widgets first. Their wrapped text and
            // actual control heights consume space before the body gets any.
            let mut reserved = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("image_combine_fixed_footer")
                    .max_rect(content_rect)
                    .layout(egui::Layout::bottom_up(egui::Align::Min)),
            );
            draw_footer(&mut reserved, state, &mut action);
            let footer_rect = reserved.min_rect();
            // Use the widgets' actual boundary, not the rounded next-cursor
            // rectangle. Header/body/footer must share the same partition.
            let body_bottom =
                (footer_rect.top() - ui.spacing().item_spacing.y).max(content_rect.top());
            let body_rect = egui::Rect::from_min_max(
                content_rect.min,
                egui::pos2(content_rect.right(), body_bottom),
            );
            let mut body = reserved.new_child(
                egui::UiBuilder::new()
                    .id_salt("image_combine_body")
                    .max_rect(body_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            body.set_clip_rect(body_rect.intersect(ui.clip_rect()));
            let height = body_rect.height();
            body.allocate_ui_with_layout(
                egui::vec2(body.available_width(), height),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    let left_width = (ui.available_width() * 0.57).max(1.0);
                    let right_width =
                        (ui.available_width() - left_width - ui.spacing().item_spacing.x).max(1.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(left_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let controls = 2.0
                                * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y)
                                + super::startup_dialog_button_rows_height(
                                    ui,
                                    &[
                                        "ファイル名を入れる",
                                        "すべて番号に戻す",
                                        "逆順",
                                        "前へ",
                                        "後へ",
                                    ],
                                )
                                + if state
                                    .items
                                    .iter()
                                    .any(|item| matches!(item.status, ItemStatus::Failed(_)))
                                {
                                    80.0
                                } else {
                                    0.0
                                };
                            let controls = controls.min(height * 0.65);
                            draw_preview(
                                ui,
                                state,
                                (height - controls - ui.spacing().item_spacing.y).max(0.0),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("image_combine_item_controls")
                                .max_height(controls)
                                .min_scrolled_height(0.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| draw_item_controls(ui, state));
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(right_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.spacing_mut().scroll =
                                super::non_overlapping_dialog_scroll_style(ui.spacing().scroll);
                            let output = egui::ScrollArea::vertical()
                                .id_salt("image_combine_settings")
                                .max_height(height)
                                .min_scrolled_height(0.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| options_ui::draw(ui, state, &mut action));
                            #[cfg(test)]
                            ctx.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::unique("image_combine_scroll_layout"),
                                    crate::test_ui::scroll_layout::ScrollLayout::from_output(
                                        &output,
                                    ),
                                )
                            });
                            #[cfg(not(test))]
                            let _ = output;
                        },
                    );
                },
            );
            #[cfg(test)]
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique("image_combine_fixed_layout"),
                    [header_rect, body_rect, footer_rect],
                )
            });
            // Scroll content must not enlarge the next frame's finite Window.
            drop(body);
            ui.advance_cursor_after_rect(content_rect);
            // Popup pickers/combos consume wheel before the parent dialog.
            if egui::Popup::is_any_open(ctx) {
                crate::wheel_input::consume_popup_wheel_input(ctx);
            }
        });
    action.cancel |= !open;
    // Enter only confirms fields/popup widgets; it is never an export trigger.
    action
}

fn footer_error(state: &CombineDialogState) -> Option<String> {
    state
        .error
        .clone()
        .or_else(|| state.render_error.clone())
        // Loading already has a status row. Treating it as an error adds a
        // temporary footer row and moves the label's click target on completion.
        .or_else(|| {
            (state.ready()
                || state
                    .items
                    .iter()
                    .any(|item| matches!(item.status, ItemStatus::Failed(_))))
            .then(|| state.validation().result.as_ref().err().cloned())
            .flatten()
        })
        .or_else(|| state.output_validation().err())
}

fn status_text(state: &CombineDialogState) -> String {
    let status = if let Ok(layout) = &state.validation().result {
        format!(
            "出力 {} × {} px・切り取り 最大 {:.0}%{}",
            layout.canvas_size.0,
            layout.canvas_size.1,
            layout.max_crop_percent,
            if layout.any_upscaled {
                "・拡大される画像があります"
            } else {
                ""
            }
        )
    } else {
        let loaded = state
            .items
            .iter()
            .filter(|item| matches!(item.status, ItemStatus::Loaded(_) | ItemStatus::Failed(_)))
            .count();
        format!("読み込み中 {loaded}/{}", state.items.len())
    };
    if let Some(path) = state.replacement_path() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        format!("{status}\n元の結合画像を置き換えます (元に戻せません): {name}")
    } else {
        status
    }
}

fn draw_footer(ui: &mut egui::Ui, state: &mut CombineDialogState, action: &mut Action) {
    enum Row {
        Separator,
        Name,
        Status,
        Warning,
        Destination,
        Buttons,
        Error,
    }
    let mut rows = [
        Row::Separator,
        Row::Name,
        Row::Status,
        Row::Warning,
        Row::Destination,
        Row::Buttons,
        Row::Error,
    ];
    // Preserve the same reading order in a bottom-anchored Window footer and
    // the top-down output-control fixture. Every widget is drawn only once.
    if ui.layout().main_dir() == egui::Direction::BottomUp {
        rows.reverse();
    }
    for row in rows {
        match row {
            Row::Separator => {
                ui.separator();
            }
            Row::Name => {
                ui.horizontal(|ui| {
                    ui.label("名前");
                    let replacing = state.replacement_path().is_some();
                    ui.add_enabled_ui(!replacing, |ui| {
                        text(
                            ui,
                            &mut state.name,
                            "image_combine_name",
                            (ui.available_width() - 8.0).max(1.0),
                        )
                    });
                });
            }
            Row::Status => {
                ui.add(egui::Label::new(status_text(state)).wrap());
            }
            Row::Warning => {
                if let Some(preview) = &state.preview {
                    if preview.not_drawn > 0 {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("収まらないラベル: {} 件", preview.not_drawn),
                        );
                    }
                }
            }
            Row::Destination => {
                let path = state.output_dir().display().to_string();
                ui.add(egui::Label::new(format!("保存先: {path}")).truncate())
                    .on_hover_text(path);
            }
            Row::Buttons => {
                ui.horizontal_wrapped(|ui| {
                    // Disable focused Enter activation of this one button.
                    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let response = ui.add_enabled(state.can_write(), egui::Button::new("書き出す"));
                    action.write |= response.clicked() && !enter;
                    action.cancel |= ui.button("キャンセル").clicked();
                });
            }
            Row::Error => {
                if let Some(error) = footer_error(state) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
    }
}

fn card_rect(card: Rect, rect: egui::Rect, canvas: (u32, u32)) -> egui::Rect {
    let scale = egui::vec2(
        rect.width() / canvas.0 as f32,
        rect.height() / canvas.1 as f32,
    );
    egui::Rect::from_min_max(
        rect.min + egui::vec2(card.x as f32 * scale.x, card.y as f32 * scale.y),
        rect.min
            + egui::vec2(
                card.right() as f32 * scale.x,
                card.bottom() as f32 * scale.y,
            ),
    )
}

fn draw_preview(ui: &mut egui::Ui, state: &mut CombineDialogState, height: f32) {
    let (area, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );
    ui.painter()
        .rect_filled(area, 0.0, ui.visuals().extreme_bg_color);
    let Some(preview) = &state.preview else {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "プレビューを準備中",
            egui::FontId::proportional(14.0),
            ui.visuals().weak_text_color(),
        );
        return;
    };
    let size = preview.texture.size_vec2();
    let fit = (area.width() / size.x).min(area.height() / size.y);
    let image_rect = egui::Rect::from_center_size(area.center(), size * fit);
    ui.painter().image(
        preview.texture.id(),
        image_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    let rects = preview
        .layout
        .cards
        .iter()
        .map(|card| card_rect(card.card, image_rect, preview.layout.canvas_size))
        .collect::<Vec<_>>();
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::unique("image_combine_preview_cards"),
            preview
                .ids
                .iter()
                .copied()
                .zip(rects.iter().copied())
                .collect::<Vec<_>>(),
        )
    });
    if let Some(index) = preview
        .ids
        .iter()
        .position(|id| Some(*id) == state.selected_item_id)
    {
        if let Some(rect) = rects.get(index) {
            ui.painter().rect_stroke(
                *rect,
                0.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
    }
    let hit = response
        .interact_pointer_pos()
        .and_then(|pos| rects.iter().position(|rect| rect.contains(pos)));
    if response.clicked() {
        if let Some(index) = hit {
            state.selected_item_id = Some(preview.ids[index]);
        }
    }
    if response.drag_started() {
        // The first drag frame can already be over a different card. Capture
        // the press owner, not that frame's current pointer target.
        if let Some(index) = ui
            .input(|input| input.pointer.press_origin())
            .and_then(|pos| rects.iter().position(|rect| rect.contains(pos)))
        {
            state.selected_item_id = Some(preview.ids[index]);
            state.dragging = state.selected_item_id;
        }
    }
    let mut insert = None;
    if state.dragging.is_some() {
        if let Some(index) = hit {
            let rect = rects[index];
            let vertical = matches!(
                preview.kind,
                CombineLayout::Column | CombineLayout::JustifiedColumns
            );
            let after = response.interact_pointer_pos().is_some_and(|pos| {
                if vertical {
                    pos.y > rect.center().y
                } else if preview.direction == CombineDirection::RightToLeft {
                    pos.x < rect.center().x
                } else {
                    pos.x > rect.center().x
                }
            });
            let slot = index + usize::from(after);
            insert = Some(preview.ids.get(slot).copied());
            let line = if vertical {
                let y = if after { rect.bottom() } else { rect.top() };
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)]
            } else {
                let right = after == (preview.direction == CombineDirection::LeftToRight);
                let x = if right { rect.right() } else { rect.left() };
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())]
            };
            ui.painter().line_segment(
                line,
                egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
            );
        }
    }
    if response.drag_stopped() {
        if let (Some(id), Some(before)) = (state.dragging.take(), insert) {
            state.move_item(id, before);
        }
    }
}

fn draw_item_controls(ui: &mut egui::Ui, state: &mut CombineDialogState) {
    if let Some(index) = state
        .items
        .iter()
        .position(|item| Some(item.id) == state.selected_item_id)
    {
        ui.add(egui::Label::new(format!("選んだ画像: {}", state.items[index].name)).truncate());
        ui.horizontal(|ui| {
            ui.label("ラベル");
            let mut label = state.items[index]
                .spec
                .label_override
                .clone()
                .unwrap_or_else(|| {
                    resolve_label(&state.items[index].spec, index, &state.options.labels)
                });
            let response = text(
                ui,
                &mut label,
                "image_combine_label",
                (ui.available_width() - trailing_width(ui, &["番号に戻す"])).max(1.0),
            );
            if response.changed() {
                state.items[index].spec.label_override = Some(label.chars().take(200).collect());
            }
            if ui.button("番号に戻す").clicked() {
                state.items[index].spec.label_override = None;
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("ファイル名を入れる").clicked() {
                for item in &mut state.items {
                    item.spec.label_override = Some(
                        source_name(&item.spec.source, true)
                            .chars()
                            .take(200)
                            .collect(),
                    );
                }
            }
            if ui.button("すべて番号に戻す").clicked() {
                for item in &mut state.items {
                    item.spec.label_override = None;
                }
            }
            if ui.button("逆順").clicked() {
                state.items.reverse();
            }
            if ui
                .add_enabled(index > 0, egui::Button::new("前へ"))
                .clicked()
            {
                state.items.swap(index, index - 1);
            }
            if ui
                .add_enabled(index + 1 < state.items.len(), egui::Button::new("後へ"))
                .clicked()
            {
                state.items.swap(index, index + 1);
            }
        });
    }
    if state
        .items
        .iter()
        .any(|item| matches!(item.status, ItemStatus::Failed(_)))
    {
        if ui.button("読めない画像を外す").clicked() {
            state.remove_failed();
        }
        egui::ScrollArea::vertical()
            .id_salt("image_combine_failures")
            .max_height(42.0)
            .show(ui, |ui| {
                for item in &state.items {
                    if let ItemStatus::Failed(message) = &item.status {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("{}: {message}", item.name),
                        );
                    }
                }
            });
    }
}

fn fixture_state(ctx: &egui::Context, band: bool) -> CombineDialogState {
    fixture_state_with_fonts(ctx, band, None)
}

fn fixture_state_with_fonts(
    ctx: &egui::Context,
    band: bool,
    fonts: Option<&comic_core::FontSet>,
) -> CombineDialogState {
    let mut options = CombineOptions::default();
    if band {
        options.labels.placement = LabelPlacement::Band;
    }
    options.output_size = OutputSize::Width(1600);
    let items = (0..4)
        .map(|id| DialogItem {
            id,
            spec: CombineItemSpec {
                composition_identity: None,
                annotation_space: None,
                source: CombineSourceRef::File {
                    path: PathBuf::from(format!("C:/Pictures/商品{}.png", id + 1)),
                },
                label_override: band.then(|| format!("商品{}", id + 1)),
            },
            name: format!("商品{}.png", id + 1),
            status: ItemStatus::Loaded((600, 800)),
        })
        .collect::<Vec<_>>();
    let layout = layout(&options, &vec![(600, 800); 4], options.labels.band_height()).unwrap();
    let sources = [
        [128, 169, 175, 255],
        [192, 160, 143, 255],
        [161, 174, 145, 255],
        [159, 153, 181, 255],
    ]
    .map(|color| image::RgbaImage::from_pixel(60, 80, image::Rgba(color)));
    let labels = fonts
        .map(|fonts| {
            label_layout(
                &layout,
                &items
                    .iter()
                    .map(|item| item.spec.clone())
                    .collect::<Vec<_>>(),
                &options,
                fonts,
            )
        })
        .unwrap_or(LabelLayout {
            labels: vec![None; 4],
            not_drawn: 0,
        });
    let fallback_fonts = comic_core::FontSet::new();
    let rgba = render_all(
        &layout,
        &sources,
        &options,
        &labels,
        0.3,
        fonts.unwrap_or(&fallback_fonts),
    )
    .unwrap();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    CombineDialogState {
        request: 0,
        options,
        items,
        pending: VecDeque::new(),
        reopened: None,
        worker: None,
        selected_item_id: Some(2),
        dragging: None,
        preview: Some(Preview {
            texture: ctx.load_texture("image_combine_fixture", image, Default::default()),
            layout,
            ids: vec![0, 1, 2, 3],
            generation: 0,
            not_drawn: 0,
            kind: CombineLayout::JustifiedRows,
            direction: CombineDirection::LeftToRight,
        }),
        generation: 0,
        requested: None,
        validation: RefCell::new(None),
        #[cfg(test)]
        layout_searches: Default::default(),
        #[cfg(test)]
        spec_builds: Default::default(),
        render_error: None,
        error: None,
        split_text: String::new(),
        custom_ratio_text: None,
        folder_text: String::new(),
        name: "商品1_結合".into(),
        preset: Default::default(),
    }
}

/// Production window fixture without App, file IO or a worker.
#[doc(hidden)]
pub fn draw_image_combine_snapshot_fixture(ctx: &egui::Context, band: bool) {
    let id = egui::Id::unique(("image_combine_fixture", band));
    let state = ctx
        .data_mut(|data| data.get_temp::<std::sync::Arc<std::sync::Mutex<CombineDialogState>>>(id));
    let state = state.unwrap_or_else(|| {
        // Reuse the harness's already loaded font bytes; fixture rendering does no IO.
        let font_bytes = ctx.fonts(|fonts| {
            let definitions = fonts.definitions();
            definitions
                .families
                .get(&egui::FontFamily::Proportional)
                .and_then(|names| {
                    names
                        .iter()
                        .find_map(|name| definitions.font_data.get(name))
                })
                .map(|font| font.bytes().to_vec())
        });
        let fonts = font_bytes
            .and_then(|bytes| {
                comic_core::LoadedFont::from_bytes(crate::comic_overlay::COMIC_FONT_KEY, bytes).ok()
            })
            .map(|font| {
                let mut fonts = comic_core::FontSet::new();
                fonts.insert(font);
                fonts
            });
        let state = std::sync::Arc::new(std::sync::Mutex::new(fixture_state_with_fonts(
            ctx,
            band,
            fonts.as_ref(),
        )));
        ctx.data_mut(|data| data.insert_temp(id, state.clone()));
        state
    });
    draw_window(ctx, &mut state.lock().unwrap(), &[]);
}

/// Production reopened dialog fixture without disk reads or a running worker.
#[doc(hidden)]
pub fn draw_image_combine_reopen_snapshot_fixture(ctx: &egui::Context) {
    let id = egui::Id::unique("image_combine_reopen_fixture");
    let state = ctx
        .data_mut(|data| data.get_temp::<std::sync::Arc<std::sync::Mutex<CombineDialogState>>>(id));
    let state = state.unwrap_or_else(|| {
        let mut state = fixture_state(ctx, false);
        state.reopened = Some(ReopenedOutput {
            path: PathBuf::from("C:/Pictures/商品1_結合.jpg"),
            format: CombineFormat::Jpeg { quality: 95 },
            replace: true,
            has_edits: true,
        });
        let state = std::sync::Arc::new(std::sync::Mutex::new(state));
        ctx.data_mut(|data| data.insert_temp(id, state.clone()));
        state
    });
    draw_window(ctx, &mut state.lock().unwrap(), &[]);
}
