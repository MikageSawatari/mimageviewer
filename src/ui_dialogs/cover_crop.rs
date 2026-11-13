//! Representative-only crop editor. Source selection and decoding form one
//! worker operation; the UI owns only a modal draft and a display texture.
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::app::App;
use crate::app::book_cover::{
    BookCover, BookCoverContainer, BookCoverEnv, BookCoverFailure, BookKind,
    resolve_logical_book_cover,
};
use crate::context_menu_model::ContextMenuActionState;
use crate::export_crop::{CropHandle, CropRect};
use crate::folder_thumb_pins::{CoverCrop, FolderPin, FolderPinSource};
use crate::grid_item::GridItem;
use crate::rotation_db::Rotation;
use eframe::egui;

const UNIT: usize = 1_000_000;
const OK_LABEL: &str = "  OK  ";
const CORNER_HIT_RADIUS: f32 = 9.0;
const EDGE_HIT_RADIUS: f32 = 6.0;
// The sides face the window resize edge: 9 pt handle hit radius + egui's
// 10 pt resize radius + 5 pt separation. Above/below are the title and footer,
// so 12 pt only needs to contain the 9 pt handles, preserving image height.
const CANVAS_PADDING: egui::Vec2 = egui::vec2(24.0, 12.0);
const WINDOW_MARGIN: f32 = 40.0;

fn window_margin(bounds: egui::Rect) -> egui::Vec2 {
    // Give compact viewports the former 16 pt inset, rather than spending
    // their editing height on the outer gap. Restore 40 pt gradually as the
    // viewport grows from 360 to 480 pt; still clamp for very tiny viewports.
    let roomy = ((bounds.height() - 360.0) / 120.0).clamp(0.0, 1.0);
    let margin = egui::lerp(16.0..=WINDOW_MARGIN, roomy);
    egui::vec2(
        margin.min(bounds.width().max(0.0) * 0.1),
        margin.min(bounds.height().max(0.0) * 0.1),
    )
}

pub(crate) struct CoverCropDialog {
    generation: u64,
    container: PathBuf,
    cancel: Arc<AtomicBool>,
    phase: Phase,
}

enum Phase {
    Loading(mpsc::Receiver<(u64, Result<PreparedCover, String>)>),
    Ready(CropDraft),
    Failed(String),
}

struct PreparedCover {
    source: FolderPinSource,
    rotation_key: Option<String>,
    crop: Option<CoverCrop>,
    auto_crop: Option<CoverCrop>,
    auto_enabled: bool,
    image: egui::ColorImage,
}

struct CropDraft {
    source: FolderPinSource,
    texture: egui::TextureHandle,
    rotation: Rotation,
    crop: CropRect,
    had_manual_crop: bool,
    auto_enabled: bool,
    drag: Option<CropDrag>,
    error: Option<String>,
}

struct CropDrag {
    origin: egui::Pos2,
    rect: CropRect,
    handle: Option<CropHandle>,
}

impl Drop for CoverCropDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl CoverCropDialog {
    pub(crate) fn finish_worker_for_test(&self) {
        if let Phase::Loading(rx) = &self.phase {
            let _ = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("cover prepare completion");
            assert!(matches!(
                rx.recv_timeout(std::time::Duration::from_secs(10)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    Accept,
    Clear,
    Cancel,
}

impl App {
    pub(crate) fn cover_crop_dialog_open(&self) -> bool {
        self.cover_crop_dialog.is_some()
    }

    /// Returns an I/O-free logical owner and optional explicit page source.
    fn cover_crop_target(
        &self,
        idx: usize,
    ) -> Option<(BookCoverContainer, Option<FolderPinSource>)> {
        // The shared menu/start boundary owns these exclusions, so neither menu
        // backend nor direct command dispatch can enter an unsupported context.
        if self.items_are_global_search_view || !self.main_folder_history_available() {
            return None;
        }
        let item = self.items.get(idx)?;
        let target = match item {
            GridItem::Folder(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. } => (
                BookCoverContainer {
                    path: path.clone(),
                    kind: match item {
                        GridItem::Folder(_) => BookKind::Folder,
                        GridItem::ZipFile(_) => BookKind::Zip,
                        GridItem::PdfFile(_) => BookKind::Pdf,
                        GridItem::ConvertibleArchive { format, .. } => BookKind::Archive(*format),
                        _ => unreachable!(),
                    },
                    zip_dir_prefix: None,
                },
                None,
            ),
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } => {
                let path = self
                    .archive_source_override
                    .as_ref()
                    .filter(|_| {
                        self.current_folder
                            .as_ref()
                            .is_some_and(|p| crate::folder_tree::path_eq(p, zip_path))
                    })
                    .unwrap_or(zip_path)
                    .clone();
                let segments = dir_prefix
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let effective = self
                    .zip_nav
                    .as_ref()
                    .map(|nav| nav.tree.collapse_redundant(&segments))
                    .unwrap_or(segments);
                let prefix = format!("{}/", effective.join("/"));
                (
                    BookCoverContainer {
                        kind: BookKind::for_path(&path),
                        path,
                        zip_dir_prefix: Some(prefix),
                    },
                    None,
                )
            }
            GridItem::Image(path) if !crate::raw_format::is_raw_path(path) => {
                let path = self.current_folder.clone()?;
                (
                    BookCoverContainer {
                        path,
                        kind: BookKind::Folder,
                        zip_dir_prefix: None,
                    },
                    Some(self.folder_pin_selected_source(idx)?),
                )
            }
            GridItem::ZipImage { entry_name, .. }
                if !crate::raw_format::is_raw_path(Path::new(entry_name)) =>
            {
                let nav = self.zip_nav.as_ref()?;
                let path = self
                    .archive_source_override
                    .clone()
                    .unwrap_or_else(|| nav.tree.zip_path.clone());
                let prefix = (!nav.at_root()).then(|| format!("{}/", nav.current().join("/")));
                (
                    BookCoverContainer {
                        kind: BookKind::for_path(&path),
                        path,
                        zip_dir_prefix: prefix,
                    },
                    Some(self.folder_pin_selected_source(idx)?),
                )
            }
            GridItem::PdfPage { pdf_path, .. } => {
                let path = self
                    .current_folder
                    .as_ref()
                    .filter(|p| crate::folder_tree::path_eq(p, pdf_path))?
                    .clone();
                (
                    BookCoverContainer {
                        path,
                        kind: BookKind::Pdf,
                        zip_dir_prefix: None,
                    },
                    Some(self.folder_pin_selected_source(idx)?),
                )
            }
            _ => return None,
        };
        if crate::app::is_synthetic_view_path(&target.0.path)
            || crate::path_key::is_drive_or_share_root(&target.0.path)
            || self.archive_source_override.is_some() && self.zip_nav.is_none()
        {
            return None;
        }
        Some(target)
    }

    pub(crate) fn cover_crop_menu_state(&self, idx: usize) -> Option<ContextMenuActionState> {
        let (_, source) = self.cover_crop_target(idx)?;
        let unavailable = match self.items.get(idx)? {
            GridItem::ConvertibleArchive { path, .. } => !self
                .converted_archive_cache_paths
                .get(&crate::path_key::normalize_keep_drive(path))
                .is_some_and(crate::app::ConvertedArchiveSourceState::is_available),
            _ => false,
        };
        Some(ContextMenuActionState {
            label: if source.is_some() {
                "このページを代表にして範囲を指定…"
            } else {
                "代表サムネの範囲を指定…"
            }
            .into(),
            enabled: !unavailable,
            disabled_reason: unavailable
                .then(|| "アーカイブを ZIP に変換すると指定できます".into()),
        })
    }

    pub(crate) fn start_cover_crop_for_item(&mut self, ctx: &egui::Context, idx: usize) {
        if self.cover_crop_dialog.is_some()
            || self.main_chrome_pointer_blocked()
            || !self
                .cover_crop_menu_state(idx)
                .is_some_and(|state| state.enabled)
        {
            return;
        }
        let Some((container, explicit_source)) = self.cover_crop_target(idx) else {
            return;
        };
        self.cover_crop_generation = self.cover_crop_generation.wrapping_add(1);
        let generation = self.cover_crop_generation;
        let env = self.book_cover_env();
        let owner = container.pin_container();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let target_px = (ctx.content_rect().size().max_elem() * ctx.pixels_per_point())
            .ceil()
            .clamp(256.0, 2048.0) as u32;
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("cover-crop-prepare".into())
            .spawn(move || {
                let result =
                    prepare_cover(&container, explicit_source, &env, target_px, &worker_cancel);
                let _ = tx.send((generation, result));
                repaint.request_repaint();
            });
        let phase = match spawned {
            Ok(_) => Phase::Loading(rx),
            Err(error) => {
                crate::logger::log(format!("cover_crop: prepare worker spawn failed: {error}"));
                Phase::Failed(format!("画像の準備を開始できませんでした: {error}"))
            }
        };
        self.cover_crop_dialog = Some(CoverCropDialog {
            generation,
            container: owner,
            cancel,
            phase,
        });
    }

    pub(crate) fn show_cover_crop_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.cover_crop_dialog.take() else {
            return;
        };
        if let Phase::Loading(rx) = &dialog.phase {
            let result = match rx.try_recv() {
                Ok((generation, result)) if generation == dialog.generation => Some(result),
                Ok(_) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("画像の準備に失敗しました".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                dialog.phase = match result {
                    Ok(prepared) => {
                        let rotation = prepared
                            .rotation_key
                            .as_deref()
                            .map(|key| self.rotation_for_page_key(key))
                            .unwrap_or_default();
                        Phase::Ready(CropDraft {
                            source: prepared.source,
                            texture: ctx.load_texture(
                                format!("cover-crop-{}", dialog.generation),
                                prepared.image,
                                egui::TextureOptions::LINEAR,
                            ),
                            rotation,
                            crop: rotate_rect(
                                crop_rect(prepared.crop.or(prepared.auto_crop)),
                                rotation,
                            ),
                            had_manual_crop: prepared.crop.is_some(),
                            auto_enabled: prepared.auto_enabled,
                            drag: None,
                            error: None,
                        })
                    }
                    Err(message) => Phase::Failed(message),
                };
            }
        }
        let enter = self.dialog_enter_pressed(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let action = draw_window(ctx, &mut open, &mut dialog.phase, enter, escape);
        let close = !open
            || match action {
                Action::Cancel => true,
                Action::Accept | Action::Clear => {
                    if let Phase::Ready(draft) = &mut dialog.phase {
                        let crop = if action == Action::Clear {
                            Ok(None)
                        } else {
                            let source_rect = rotate_rect(draft.crop, inverse(draft.rotation));
                            CoverCrop::new([
                                source_rect.min_x.round() as u32,
                                source_rect.min_y.round() as u32,
                                source_rect.max_x.round() as u32,
                                source_rect.max_y.round() as u32,
                            ])
                        };
                        match crop {
                            Ok(crop) => {
                                if self.set_folder_thumb_pin_with_cover_crop(
                                    &dialog.container,
                                    draft.source.clone(),
                                    crop,
                                ) {
                                    self.show_feedback_toast(
                                        if action == Action::Clear {
                                            "代表サムネの範囲を解除しました"
                                        } else {
                                            "代表サムネの範囲を設定しました"
                                        }
                                        .into(),
                                    );
                                    true
                                } else {
                                    self.show_feedback_warning(
                                        if action == Action::Clear {
                                            "代表サムネの範囲を解除できませんでした"
                                        } else {
                                            "代表サムネの範囲を保存できませんでした"
                                        }
                                        .into(),
                                    );
                                    true
                                }
                            }
                            Err(_) => {
                                draft.error = Some(
                                    "幅・高さが画像の 1% 以上になるよう指定してください".into(),
                                );
                                false
                            }
                        }
                    } else {
                        false
                    }
                }
                Action::None => false,
            };
        if !close {
            self.cover_crop_dialog = Some(dialog);
        }
    }
}

fn prepare_cover(
    container: &BookCoverContainer,
    explicit_source: Option<FolderPinSource>,
    env: &BookCoverEnv,
    target_px: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<PreparedCover, String> {
    let started = std::time::Instant::now();
    let source = explicit_source.map(|source| {
        env.pin_db
            .as_ref()
            .and_then(|db| db.lookup(&container.pin_container()))
            .filter(|pin| pin.source == source)
            .unwrap_or_else(|| source.into())
    });
    let result = prepare_cover_inner(container, source, env, target_px, cancel);
    if let Err(error) = &result
        && !cancel.load(Ordering::Relaxed)
    {
        crate::logger::log(format!("cover_crop: preparation failed: {error}"));
    }
    if crate::perf::is_enabled() {
        crate::perf::event(
            "cover_crop",
            "prepare",
            None,
            0,
            &[
                (
                    "elapsed_ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
                ("success", serde_json::json!(result.is_ok())),
            ],
        );
    }
    result
}

fn prepare_cover_inner(
    container: &BookCoverContainer,
    source: Option<FolderPin>,
    env: &BookCoverEnv,
    target_px: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<PreparedCover, String> {
    use crate::thumb_loader::ReadLocation;
    let cover = match resolve_logical_book_cover(container, source, env) {
        BookCover::Readable(cover) => cover,
        BookCover::VideoFrame { .. } => return Err("動画は範囲を指定できません".into()),
        BookCover::Unreadable(reason) => {
            return Err(match reason {
                BookCoverFailure::Unconverted => "アーカイブを ZIP に変換してから指定してください",
                BookCoverFailure::Missing => "代表画像が見つかりませんでした",
                BookCoverFailure::ReadFailed => "代表画像を読み込めませんでした",
                BookCoverFailure::PasswordRequired => "PDF のパスワードが必要です",
            }
            .into());
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("キャンセルしました".into());
    }
    if !cover.persistent_source.supports_cover_crop() {
        return Err("この代表画像は範囲を指定できません".into());
    }
    let hint = match &cover.location {
        ReadLocation::File(path) => path.as_path(),
        ReadLocation::ArchiveEntry { entry, .. } => Path::new(entry),
        ReadLocation::PdfPage { .. } => Path::new("page.png"),
        _ => return Err("画像ではないため範囲を指定できません".into()),
    };
    if crate::raw_format::is_raw_path(hint) {
        return Err("RAW 画像は範囲を指定できません".into());
    }
    let image = match &cover.location {
        ReadLocation::PdfPage { pdf, page } => {
            use crate::pdf_loader::{CancelWaitPolicy, JobPriority, PdfRenderValidity};
            crate::pdf_loader::render_page_with_read_target(
                pdf,
                cover
                    .epub_read
                    .as_ref()
                    .ok_or("PDF を読み込めませんでした")?,
                *page,
                target_px,
                cover.pdf_password.as_deref(),
                Some(cancel.clone()),
                JobPriority::Normal,
                PdfRenderValidity::Unscoped,
                CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|error| error.to_string())?
            .image
        }
        _ => {
            let source = match &cover.location {
                ReadLocation::File(path) => crate::image_decode::Source::Path(path),
                ReadLocation::ArchiveEntry { entry, .. } => crate::image_decode::Source::Bytes {
                    bytes: cover
                        .preloaded_bytes
                        .as_deref()
                        .ok_or("書庫の画像を読み込めませんでした")?,
                    hint: entry,
                },
                _ => unreachable!(),
            };
            crate::image_decode::decode(
                source,
                crate::avif_decoder::Workload::Background,
                false,
                Some(cancel),
            )
            .map_err(|error| error.to_string())?
            .oriented(source)
        }
    };
    let auto_enabled = env.thumb_cover_auto_crop && container.kind != BookKind::Folder;
    let has_edit_preview = auto_enabled
        && cover.cover_crop.is_none()
        && cover
            .edit_preview_key
            .as_deref()
            .zip(env.edit_preview_db.as_deref())
            .is_some_and(|(key, db)| {
                match &cover.location {
                    ReadLocation::File(path) => std::fs::metadata(path).ok().and_then(|meta| {
                        db.load(
                            key,
                            crate::ui_helpers::mtime_secs(&meta),
                            meta.len() as i64,
                            target_px,
                        )
                    }),
                    ReadLocation::ArchiveEntry { archive, .. } => {
                        std::fs::metadata(archive).ok().and_then(|meta| {
                            db.load_for_container(
                                key,
                                crate::ui_helpers::mtime_secs(&meta),
                                meta.len() as i64,
                                target_px,
                            )
                        })
                    }
                    ReadLocation::PdfPage { .. } => db.load_for_container(
                        key,
                        cover.signature.mtime_ms.div_euclid(1000),
                        cover.signature.size as i64,
                        target_px,
                    ),
                    _ => None,
                }
                .is_some()
            });
    let auto_crop = if auto_enabled
        && cover.cover_crop.is_none()
        && !has_edit_preview
        && u64::from(image.width()) * 100 >= u64::from(image.height()) * 85
        && !cancel.load(Ordering::Relaxed)
    {
        let page_aspect = match &cover.location {
            ReadLocation::ArchiveEntry { archive, entry } => {
                crate::cover_auto_sample::sample_zip_page_aspect(
                    archive,
                    entry,
                    cover.zip_dir_prefix.as_deref(),
                )
            }
            ReadLocation::PdfPage { pdf, page } => cover.epub_read.as_ref().and_then(|read| {
                crate::pdf_loader::get_page_sizes_with_read_target(
                    pdf,
                    read,
                    cover.pdf_password.as_deref(),
                )
                .ok()
                .and_then(|sizes| {
                    crate::cover_auto_sample::page_aspect_from_sizes(&sizes, *page as usize)
                })
            }),
            _ => None,
        };
        crate::cover_auto_crop::cover_auto_crop(
            &crate::cover_auto_crop::analysis_raster(&image),
            (image.width(), image.height()),
            page_aspect,
            env.default_reading_direction,
        )
        .ok()
    } else {
        None
    };
    let image = if image.width().max(image.height()) > target_px {
        image.resize(target_px, target_px, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(PreparedCover {
        source: cover.persistent_source,
        rotation_key: cover.rotation_key,
        crop: cover.cover_crop,
        auto_crop,
        auto_enabled,
        image: egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ),
    })
}

fn crop_rect(crop: Option<CoverCrop>) -> CropRect {
    let [x0, y0, x1, y1] =
        crop.map(|crop| crop.values())
            .unwrap_or([0, 0, UNIT as u32, UNIT as u32]);
    CropRect {
        min_x: x0 as f32,
        min_y: y0 as f32,
        max_x: x1 as f32,
        max_y: y1 as f32,
    }
}

fn inverse(rotation: Rotation) -> Rotation {
    Rotation::from_degrees(-rotation.degrees())
}

fn rotate_point(point: egui::Pos2, rotation: Rotation) -> egui::Pos2 {
    match rotation {
        Rotation::None => point,
        Rotation::Cw90 => egui::pos2(1.0 - point.y, point.x),
        Rotation::Cw180 => egui::pos2(1.0 - point.x, 1.0 - point.y),
        Rotation::Cw270 => egui::pos2(point.y, 1.0 - point.x),
    }
}

fn rotate_rect(rect: CropRect, rotation: Rotation) -> CropRect {
    let unit = UNIT as f32;
    let a = rotate_point(egui::pos2(rect.min_x / unit, rect.min_y / unit), rotation);
    let b = rotate_point(egui::pos2(rect.max_x / unit, rect.max_y / unit), rotation);
    CropRect {
        min_x: a.x.min(b.x) * unit,
        min_y: a.y.min(b.y) * unit,
        max_x: a.x.max(b.x) * unit,
        max_y: a.y.max(b.y) * unit,
    }
}

fn draw_window(
    ctx: &egui::Context,
    open: &mut bool,
    phase: &mut Phase,
    enter: bool,
    escape: bool,
) -> Action {
    let bounds = ctx.content_rect();
    let safe = bounds.shrink2(window_margin(bounds));
    // Compensate for the padding increase (4 -> 24 pt per side) and the
    // scrollbar now needed for the explanations below the image. Retain the
    // former maximum image width using the actual scrollbar reservation.
    let scroll_width =
        super::non_overlapping_dialog_scroll_style(ctx.global_style().spacing.scroll)
            .allocated_width();
    let size = egui::vec2(safe.width().min(900.0 + scroll_width), safe.height());
    let mut action = if escape { Action::Cancel } else { Action::None };
    egui::Window::new("代表サムネの範囲").id(egui::Id::new("cover-crop-window"))
        .open(open).default_pos(safe.min)
        .default_size(size).min_size(egui::vec2(240.0,120.0).min(size))
        .max_size(size).constrain_to(safe).collapsible(false).resizable(true)
        .show(ctx, |parent| {
            let content_rect = parent.available_rect_before_wrap();
            let mut reserved = parent.new_child(
                egui::UiBuilder::new().max_rect(content_rect)
                    .layout(egui::Layout::bottom_up(egui::Align::Min)),
            );
            // Render the fixed footer first, bottom-up. Its actual widget
            // rectangles (including wrapped validation/auto text) consume the
            // height before the scroll body receives the remainder.
            if let Phase::Ready(draft)=phase
                && draft.had_manual_crop && draft.auto_enabled
            { reserved.small("手動の範囲を解除すると、自動の判定に戻ります。"); }
            if let Phase::Ready(draft)=phase
                && let Some(error)=&draft.error
            { reserved.colored_label(reserved.visuals().error_fg_color,error); }
            let row_height = [OK_LABEL, "キャンセル", "手動の範囲を解除"]
                .into_iter()
                .map(|label| egui::WidgetText::from(label)
                    .into_galley(&reserved, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button)
                    .size().y + 2.0 * reserved.spacing().button_padding.y)
                .fold(reserved.spacing().interact_size.y, f32::max);
            reserved.allocate_ui_with_layout(
                egui::vec2(reserved.available_width(), row_height),
                egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let ready = matches!(phase, Phase::Ready(_));
                if ui.add_enabled(ready, egui::Button::new(OK_LABEL)).clicked() || enter && ready { action = Action::Accept; }
                if ui.button("キャンセル").clicked() { action = Action::Cancel; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(ui.visuals().resize_corner_size);
                    let can_clear = matches!(phase, Phase::Ready(draft) if draft.had_manual_crop);
                    if ui.add_enabled(can_clear, egui::Button::new("手動の範囲を解除")).clicked() { action = Action::Clear; }
                });
            });
            // In top-down reading order: 8 pt, separator, 4 pt, action row.
            reserved.add_space(4.0);
            reserved.separator();
            reserved.add_space(8.0);
            let body_rect = reserved.available_rect_before_wrap();
            let mut body = reserved.new_child(egui::UiBuilder::new().max_rect(body_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)));
            let ui = &mut body;
            let body_height = body_rect.height().max(1.0);
            ui.spacing_mut().scroll = super::non_overlapping_dialog_scroll_style(ui.spacing().scroll);
            #[cfg(test)]
            let mut canvas_rect = None;
            let _body = egui::ScrollArea::vertical().auto_shrink([false,false]).max_height(body_height).show(ui, |ui| {
                match phase {
                    Phase::Loading(_) => { ui.spinner(); ui.label("代表画像を読み込んでいます…"); }
                    Phase::Failed(message) => {
                        ui.colored_label(ui.visuals().error_fg_color, message.as_str());
                        ui.label("本の中を開いて表紙のページで指定してください。");
                    }
                    Phase::Ready(draft) => {
                        let height = (body_height-4.0).max(48.0);
                        let _canvas = draw_canvas(ui,draft,height);
                        #[cfg(test)]
                        { canvas_rect = Some(_canvas); }
                        ui.weak("ドラッグで範囲を作成・移動できます。枠の辺と角で大きさを変更できます。");
                    }
                }
                ui.label("範囲は一覧の代表サムネだけに使います。画像そのものや閲覧時の表示は変わりません。");
                ui.label("範囲を指定すると、この画像が代表に固定されます。");
                ui.weak("このページの消しゴム・注釈・切り取りは代表サムネには反映されません (色調補正は反映されます)。");
            });
            #[cfg(test)]
            ctx.data_mut(|data| {
                let id=egui::Id::new("cover-crop-canvas-geometry");
                if let Some(rect)=canvas_rect { data.insert_temp(id,(rect,_body.inner_rect)); }
                else { data.remove::<(egui::Rect,egui::Rect)>(id); }
            });
            // The finite Window allocation owns the whole footer/body layout;
            // do not feed scroll-content height into the next frame's size.
            // Advance the original parent, not the scroll body's child.
            drop(body);
            parent.advance_cursor_after_rect(content_rect);
        });
    action
}

fn draw_canvas(ui: &mut egui::Ui, draft: &mut CropDraft, height: f32) -> egui::Rect {
    let mut size = draft.texture.size_vec2();
    if matches!(draft.rotation, Rotation::Cw90 | Rotation::Cw270) {
        size = egui::vec2(size.y, size.x);
    }
    let padding = 2.0 * CANVAS_PADDING;
    let scale = ((ui.available_width() - padding.x).max(1.0) / size.x)
        .min((height - padding.y).max(1.0) / size.y)
        .max(0.0001);
    let (canvas, response) = ui.allocate_exact_size(size * scale + padding, egui::Sense::drag());
    let rect = canvas.shrink2(CANVAS_PADDING);
    let mut mesh = egui::Mesh::with_texture(draft.texture.id());
    for (pos, uv) in [
        (rect.left_top(), egui::pos2(0., 0.)),
        (rect.right_top(), egui::pos2(1., 0.)),
        (rect.right_bottom(), egui::pos2(1., 1.)),
        (rect.left_bottom(), egui::pos2(0., 1.)),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: rotate_point(uv, inverse(draft.rotation)),
            color: egui::Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    ui.painter().add(egui::Shape::mesh(mesh));
    let crop = draft.crop.to_screen_rect(rect, UNIT, UNIT);
    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let origin = pointer - response.drag_delta();
        let handle = hit_handle(crop, origin).or_else(|| {
            (!draft.crop.is_full(UNIT, UNIT) && crop.contains(origin)).then_some(CropHandle::Body)
        });
        draft.drag = Some(CropDrag {
            origin,
            rect: draft.crop,
            handle,
        });
        draft.error = None;
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(drag) = &draft.drag
    {
        let unit = UNIT as f32;
        draft.crop = if let Some(handle) = drag.handle {
            drag.rect.dragged(
                handle,
                (pointer.x - drag.origin.x) / rect.width() * unit,
                (pointer.y - drag.origin.y) / rect.height() * unit,
                UNIT,
                UNIT,
                None,
            )
        } else {
            CropRect {
                min_x: (drag.origin.x - rect.left()) / rect.width() * unit,
                min_y: (drag.origin.y - rect.top()) / rect.height() * unit,
                max_x: (pointer.x - rect.left()) / rect.width() * unit,
                max_y: (pointer.y - rect.top()) / rect.height() * unit,
            }
            .sanitized(UNIT, UNIT)
        };
    }
    if response.drag_stopped() {
        draft.drag = None;
    }
    let crop = draft.crop.to_screen_rect(rect, UNIT, UNIT);
    let shade = egui::Color32::from_black_alpha(145);
    for outside in [
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), crop.top())),
        egui::Rect::from_min_max(egui::pos2(rect.left(), crop.bottom()), rect.max),
        egui::Rect::from_min_max(egui::pos2(rect.left(), crop.top()), crop.left_bottom()),
        egui::Rect::from_min_max(crop.right_top(), egui::pos2(rect.right(), crop.bottom())),
    ] {
        ui.painter().rect_filled(outside, 0.0, shade);
    }
    ui.painter().rect_stroke(
        crop,
        0.0,
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
        egui::StrokeKind::Inside,
    );
    for (point, _) in handles(crop) {
        ui.painter().rect_filled(
            egui::Rect::from_center_size(point, egui::vec2(7., 7.)),
            0.0,
            egui::Color32::WHITE,
        );
    }
    canvas
}

fn handles(rect: egui::Rect) -> [(egui::Pos2, CropHandle); 8] {
    [
        (rect.left_top(), CropHandle::NorthWest),
        (rect.center_top(), CropHandle::North),
        (rect.right_top(), CropHandle::NorthEast),
        (rect.right_center(), CropHandle::East),
        (rect.right_bottom(), CropHandle::SouthEast),
        (rect.center_bottom(), CropHandle::South),
        (rect.left_bottom(), CropHandle::SouthWest),
        (rect.left_center(), CropHandle::West),
    ]
}

fn hit_handle(rect: egui::Rect, point: egui::Pos2) -> Option<CropHandle> {
    handles(rect)
        .into_iter()
        .find(|(p, _)| p.distance(point) <= CORNER_HIT_RADIUS)
        .map(|(_, h)| h)
        .or_else(|| {
            if (point.x - rect.left()).abs() <= EDGE_HIT_RADIUS
                && (rect.top()..=rect.bottom()).contains(&point.y)
            {
                Some(CropHandle::West)
            } else if (point.x - rect.right()).abs() <= EDGE_HIT_RADIUS
                && (rect.top()..=rect.bottom()).contains(&point.y)
            {
                Some(CropHandle::East)
            } else if (point.y - rect.top()).abs() <= EDGE_HIT_RADIUS
                && (rect.left()..=rect.right()).contains(&point.x)
            {
                Some(CropHandle::North)
            } else if (point.y - rect.bottom()).abs() <= EDGE_HIT_RADIUS
                && (rect.left()..=rect.right()).contains(&point.x)
            {
                Some(CropHandle::South)
            } else {
                None
            }
        })
}

/// Pure fixture: generated pixels, production window/canvas, no App or worker.
#[doc(hidden)]
pub fn draw_cover_crop_snapshot_fixture(ui: &mut egui::Ui) {
    draw_cover_crop_snapshot(ui, false);
}

/// The automatic setting adds the explanation beside a manual range.
#[doc(hidden)]
pub fn draw_cover_auto_crop_snapshot_fixture(ui: &mut egui::Ui) {
    draw_cover_crop_snapshot(ui, true);
}

fn draw_cover_crop_snapshot(ui: &mut egui::Ui, auto_enabled: bool) {
    let size = [720, 280];
    let mut pixels = vec![egui::Color32::from_rgb(220, 210, 187); size[0] * size[1]];
    for y in 12..268 {
        for x in 10..710 {
            pixels[y * size[0] + x] = if x < 325 {
                egui::Color32::from_rgb(45 + (x % 60) as u8, 100 + (y % 65) as u8, 156)
            } else if x < 355 {
                egui::Color32::from_rgb(128, 85, 55)
            } else {
                egui::Color32::from_rgb(174, 156, 119)
            };
        }
    }
    let texture = ui.ctx().load_texture(
        "cover-crop-fixture",
        egui::ColorImage::new(size, pixels),
        egui::TextureOptions::LINEAR,
    );
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new("cover-crop-fixture-texture"), texture.clone())
    });
    let mut phase = Phase::Ready(CropDraft {
        source: FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "cover.png".into(),
        },
        texture,
        rotation: Rotation::None,
        crop: crop_rect(CoverCrop::new([15_000, 45_000, 450_000, 955_000]).unwrap()),
        had_manual_crop: true,
        auto_enabled,
        drag: None,
        error: None,
    });
    let _ = draw_window(ui.ctx(), &mut true, &mut phase, false, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    fn fake_prepared(crop: Option<CoverCrop>) -> PreparedCover {
        PreparedCover {
            source: FolderPinSource::File {
                rel: "child/cover.png".into(),
                kind: crate::folder_thumb_pins::FileKind::Image,
            },
            rotation_key: None,
            crop,
            auto_crop: None,
            auto_enabled: false,
            image: egui::ColorImage::new(
                [120, 80],
                vec![egui::Color32::from_rgb(55, 125, 180); 120 * 80],
            ),
        }
    }

    fn fake_loading_app() -> (
        crate::app::AppTestEnvForTest,
        mpsc::Sender<(u64, Result<PreparedCover, String>)>,
        PathBuf,
        CoverCrop,
    ) {
        let mut app = crate::app::setup_app_for_test();
        let root = app.tmp.path().join("crop-book");
        std::fs::create_dir_all(root.join("child")).unwrap();
        let db = Arc::new(
            crate::folder_thumb_pins::FolderThumbPinDb::open_at(
                &app.tmp.path().join("crop-pins.db"),
            )
            .unwrap(),
        );
        let crop = CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap().unwrap();
        db.set_with_cover_crop(
            &root.join("child"),
            &FolderPinSource::File {
                rel: "cover.png".into(),
                kind: crate::folder_thumb_pins::FileKind::Image,
            },
            Some(crop),
        )
        .unwrap();
        db.set(
            &root,
            &FolderPinSource::File {
                rel: "child".into(),
                kind: crate::folder_thumb_pins::FileKind::Folder,
            },
        )
        .unwrap();
        app.current_folder = Some(root.clone());
        app.folder_pin_map = db.lookup_many([&root, &root.join("child")]);
        app.folder_thumb_pin_db = Some(db);
        app.folder_thumb_pin_dirty.clear();
        app.cover_crop_generation = 7;
        let (tx, rx) = mpsc::channel();
        app.cover_crop_dialog = Some(CoverCropDialog {
            generation: 7,
            container: root.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            phase: Phase::Loading(rx),
        });
        (app, tx, root, crop)
    }

    pub(super) fn app_harness(
        app: crate::app::AppTestEnvForTest,
    ) -> Harness<'static, crate::app::AppTestEnvForTest> {
        crate::test_ui::builder()
            .with_size(egui::vec2(1093., 614.))
            .with_context_setup(|ctx| {
                crate::ui_fonts::configure_fonts(ctx);
                crate::ime_focus::install_ime_input_policy(ctx);
            })
            .build_ui_state(
                |ui, app: &mut crate::app::AppTestEnvForTest| app.show_cover_crop_dialog(ui.ctx()),
                app,
            )
    }

    fn child_pin(app: &App, root: &Path) -> FolderPin {
        app.folder_thumb_pin_db
            .as_ref()
            .unwrap()
            .lookup(&root.join("child"))
            .unwrap()
    }

    #[test]
    fn cover_crop_initial_range_prefers_manual_then_auto_then_full() {
        let manual = CoverCrop::new([100_000, 0, 500_000, 1_000_000]).unwrap();
        let automatic = CoverCrop::new([500_000, 0, 950_000, 1_000_000]).unwrap();
        for (manual_crop, auto_crop, expected) in [
            (manual, automatic, manual),
            (None, automatic, automatic),
            (None, None, None),
        ] {
            let (app, tx, _, _) = fake_loading_app();
            let mut prepared = fake_prepared(manual_crop);
            prepared.auto_crop = auto_crop;
            prepared.auto_enabled = true;
            tx.send((7, Ok(prepared))).ok().unwrap();
            let mut h = app_harness(app);
            h.run_steps(4);
            let Phase::Ready(draft) = &h.state().cover_crop_dialog.as_ref().unwrap().phase else {
                panic!("prepared crop draft");
            };
            let expected = crop_rect(expected);
            assert_eq!(
                [
                    draft.crop.min_x,
                    draft.crop.min_y,
                    draft.crop.max_x,
                    draft.crop.max_y
                ],
                [
                    expected.min_x,
                    expected.min_y,
                    expected.max_x,
                    expected.max_y
                ]
            );
            assert_eq!(draft.had_manual_crop, manual_crop.is_some());
            if manual_crop.is_none() {
                h.get_by_label("手動の範囲を解除").click();
                h.run_steps(2);
                assert!(
                    h.state().cover_crop_dialog_open(),
                    "automatic draft cannot clear a manual range"
                );
            } else {
                h.get_by_label("手動の範囲を解除すると、自動の判定に戻ります。");
            }
        }
    }

    #[test]
    fn cover_crop_accept_auto_draft_saves_manual_range() {
        let (app, tx, root, _) = fake_loading_app();
        let crop = CoverCrop::new([500_000, 0, 1_000_000, 1_000_000]).unwrap();
        let mut prepared = fake_prepared(None);
        prepared.auto_crop = crop;
        prepared.auto_enabled = true;
        tx.send((7, Ok(prepared))).ok().unwrap();
        let mut h = app_harness(app);
        h.run_steps(4);
        h.get_by_label(OK_LABEL).click();
        h.run_steps(2);
        assert_eq!(
            h.state()
                .folder_thumb_pin_db
                .as_ref()
                .unwrap()
                .lookup(&root)
                .unwrap()
                .cover_crop,
            crop
        );
    }

    #[test]
    fn cover_crop_modal_accept_and_clear_report_their_own_success_and_failure() {
        use crate::feedback::FeedbackKind;
        for (button, success, failure) in [
            (
                OK_LABEL,
                "代表サムネの範囲を設定しました",
                "代表サムネの範囲を保存できませんでした",
            ),
            (
                "手動の範囲を解除",
                "代表サムネの範囲を解除しました",
                "代表サムネの範囲を解除できませんでした",
            ),
        ] {
            for writable in [true, false] {
                let (app, tx, root, crop) = fake_loading_app();
                let mut h = app_harness(app);
                tx.send((7, Ok(fake_prepared(Some(crop))))).unwrap();
                h.run_steps(4);
                if !writable {
                    // Force the real writer to fail without touching external data.
                    h.state_mut().folder_thumb_pin_db = Some(Arc::new(
                        crate::folder_thumb_pins::FolderThumbPinDb::open_readonly(
                            &h.state().tmp.path().join("crop-pins.db"),
                        )
                        .unwrap(),
                    ));
                }
                h.state_mut().feedback_toasts_for_test.clear();
                h.get_by_label(button).click();
                h.run_steps(2);
                assert!(!h.state().cover_crop_dialog_open());
                let expected = if writable {
                    (success.to_owned(), FeedbackKind::Info)
                } else {
                    (failure.to_owned(), FeedbackKind::Warning)
                };
                assert_eq!(h.state().feedback_toasts_for_test, vec![expected]);
                let saved = h
                    .state()
                    .folder_thumb_pin_db
                    .as_ref()
                    .unwrap()
                    .lookup(&root)
                    .unwrap();
                if writable {
                    assert_eq!(saved.cover_crop, (button == OK_LABEL).then_some(crop));
                } else {
                    assert!(matches!(
                        saved.source,
                        FolderPinSource::File {
                            kind: crate::folder_thumb_pins::FileKind::Folder,
                            ..
                        }
                    ));
                }
            }
        }
    }

    #[test]
    fn cover_crop_modal_loading_ready_accept_publishes_source_and_crop() {
        let (app, tx, root, crop) = fake_loading_app();
        let original_child = child_pin(&app, &root);
        let mut h = app_harness(app);
        h.run_steps(3);
        assert!(h.state().common_modal_dialog_open());
        assert!(matches!(
            h.state().cover_crop_dialog.as_ref().unwrap().phase,
            Phase::Loading(_)
        ));
        assert!(tx.send((7, Ok(fake_prepared(Some(crop))))).is_ok());
        h.run_steps(3);
        assert!(matches!(
            h.state().cover_crop_dialog.as_ref().unwrap().phase,
            Phase::Ready(_)
        ));
        let changed = CoverCrop::new([25_000, 50_000, 475_000, 950_000]).unwrap();
        if let Phase::Ready(draft) = &mut h.state_mut().cover_crop_dialog.as_mut().unwrap().phase {
            draft.crop = crop_rect(changed);
        }
        h.get_by_label(OK_LABEL).click();
        h.run_steps(2);
        assert!(!h.state().cover_crop_dialog_open());
        let expected = FolderPin {
            source: fake_prepared(None).source,
            cover_crop: changed,
        };
        assert_eq!(
            h.state()
                .folder_thumb_pin_db
                .as_ref()
                .unwrap()
                .lookup(&root),
            Some(expected.clone())
        );
        assert_eq!(
            h.state()
                .folder_pin_map
                .get(&crate::path_key::normalize_keep_drive(&root)),
            Some(&expected)
        );
        assert!(h.state().folder_thumb_pin_dirty.contains(&root));
        assert_eq!(child_pin(h.state(), &root), original_child);
    }

    #[test]
    fn cover_crop_modal_clear_pins_same_leaf_at_parent_and_preserves_child_crop() {
        let (app, tx, root, crop) = fake_loading_app();
        let original_child = child_pin(&app, &root);
        let mut h = app_harness(app);
        assert!(tx.send((7, Ok(fake_prepared(Some(crop))))).is_ok());
        h.run_steps(4);
        h.get_by_label("手動の範囲を解除").click();
        h.run_steps(2);
        assert!(!h.state().cover_crop_dialog_open());
        let expected = FolderPin {
            source: fake_prepared(None).source,
            cover_crop: None,
        };
        assert_eq!(
            h.state()
                .folder_thumb_pin_db
                .as_ref()
                .unwrap()
                .lookup(&root),
            Some(expected.clone())
        );
        assert_eq!(
            h.state()
                .folder_pin_map
                .get(&crate::path_key::normalize_keep_drive(&root)),
            Some(&expected)
        );
        assert!(h.state().folder_thumb_pin_dirty.contains(&root));
        assert_eq!(child_pin(h.state(), &root), original_child);
    }

    #[test]
    fn cover_crop_invalid_accept_keeps_error_and_portrait_canvas_visible_at_100_and_200_percent() {
        for scale in [1.0, 2.0] {
            let (app, tx, root, crop) = fake_loading_app();
            let before = app.folder_thumb_pin_db.as_ref().unwrap().lookup(&root);
            let mut h = app_harness(app);
            crate::settings::apply_ui_scale_factor(&h.ctx, scale);
            h.set_size(egui::vec2(1093.0, 614.0) / scale);
            let mut prepared = fake_prepared(Some(crop));
            prepared.image = egui::ColorImage::new(
                [80, 160],
                vec![egui::Color32::from_rgb(55, 125, 180); 80 * 160],
            );
            assert!(tx.send((7, Ok(prepared))).is_ok());
            h.run_steps(6);
            if let Phase::Ready(draft) =
                &mut h.state_mut().cover_crop_dialog.as_mut().unwrap().phase
            {
                draft.crop = CropRect {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 5_000.0,
                    max_y: UNIT as f32,
                };
            }
            h.get_by_label(OK_LABEL).click();
            h.run_steps(6);
            assert!(h.state().cover_crop_dialog_open());
            let error = "幅・高さが画像の 1% 以上になるよう指定してください";
            let node = h.get_by_label(error);
            let viewport = h.ctx.content_rect();
            assert!((viewport.size() * scale - egui::vec2(1093.0, 614.0)).length() < 1.0);
            assert!(
                viewport.contains_rect(node.rect()),
                "{scale}: validation error outside viewport"
            );
            let id = unsafe {
                egui::Id::from_high_entropy_bits(
                    (u128::from(node.accesskit_node().id()) >> 64) as u64,
                )
            };
            let response = h.ctx.read_response(id).expect("validation error response");
            assert!(
                response.interact_rect.contains_rect(response.rect),
                "{scale}: validation error clipped"
            );
            let (canvas, body) = h
                .ctx
                .data(|data| {
                    data.get_temp::<(egui::Rect, egui::Rect)>(egui::Id::new(
                        "cover-crop-canvas-geometry",
                    ))
                })
                .unwrap();
            assert!(
                body.contains_rect(canvas),
                "{scale}: portrait image/frame clipped after failed accept: {canvas:?} in {body:?}"
            );
            assert!(viewport.contains_rect(canvas));
            for label in [OK_LABEL, "手動の範囲を解除", "キャンセル", "Close window"] {
                assert!(
                    viewport.contains_rect(h.get_by_label(label).rect()),
                    "{scale}: {label} outside viewport"
                );
            }
            assert_eq!(
                h.state()
                    .folder_thumb_pin_db
                    .as_ref()
                    .unwrap()
                    .lookup(&root),
                before
            );
            assert!(h.state().folder_thumb_pin_dirty.is_empty());
            h.get_by_label("キャンセル").click();
            h.run_steps(2);
            assert!(!h.state().cover_crop_dialog_open());
        }
    }

    #[test]
    fn cover_crop_modal_cancel_and_failed_close_leave_pin_rows_unchanged() {
        for failed in [false, true] {
            let (app, tx, root, crop) = fake_loading_app();
            let before = app.folder_thumb_pin_db.as_ref().unwrap().lookup(&root);
            let original_child = child_pin(&app, &root);
            let mut h = app_harness(app);
            assert!(
                tx.send((
                    7,
                    if failed {
                        Err("準備できませんでした".into())
                    } else {
                        Ok(fake_prepared(Some(crop)))
                    }
                ))
                .is_ok()
            );
            h.run_steps(4);
            if failed {
                assert!(matches!(
                    h.state().cover_crop_dialog.as_ref().unwrap().phase,
                    Phase::Failed(_)
                ));
                assert!(
                    h.query_by_label("本の中を開いて表紙のページで指定してください。")
                        .is_some()
                );
            }
            h.get_by_label("キャンセル").click();
            h.run_steps(2);
            assert!(!h.state().cover_crop_dialog_open());
            assert_eq!(
                h.state()
                    .folder_thumb_pin_db
                    .as_ref()
                    .unwrap()
                    .lookup(&root),
                before
            );
            assert!(h.state().folder_thumb_pin_dirty.is_empty());
            assert_eq!(child_pin(h.state(), &root), original_child);
        }
    }

    #[test]
    fn cover_crop_modal_rejects_wrong_generation_before_accepting_its_worker_result() {
        let (app, tx, _root, crop) = fake_loading_app();
        let mut h = app_harness(app);
        h.run_steps(3);
        assert!(tx.send((6, Err("stale generation".into()))).is_ok());
        h.step();
        assert!(matches!(
            h.state().cover_crop_dialog.as_ref().unwrap().phase,
            Phase::Loading(_)
        ));
        assert!(tx.send((7, Ok(fake_prepared(Some(crop))))).is_ok());
        h.run_steps(3);
        assert!(matches!(
            h.state().cover_crop_dialog.as_ref().unwrap().phase,
            Phase::Ready(_)
        ));
        h.get_by_label("キャンセル").click();
        h.run_steps(2);
        assert!(tx.send((7, Err("late completion".into()))).is_err());
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications through production dialog helpers"
    )]
    fn cover_crop_modal_enter_and_escape_respect_ime_composition() {
        for key in [egui::Key::Enter, egui::Key::Escape] {
            for composing in [false, true] {
                let (app, tx, root, crop) = fake_loading_app();
                let before = app.folder_thumb_pin_db.as_ref().unwrap().lookup(&root);
                let mut h = app_harness(app);
                assert!(tx.send((7, Ok(fake_prepared(Some(crop))))).is_ok());
                h.run_steps(4);
                if composing {
                    h.event(egui::Event::Ime(egui::ImeEvent::Enabled));
                    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "表紙".into(),
                        active_range_chars: None,
                    }));
                }
                h.event(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run_steps(2);
                if composing {
                    assert!(
                        h.state().cover_crop_dialog_open(),
                        "IME {key:?} must keep the editor open"
                    );
                    assert_eq!(
                        h.state()
                            .folder_thumb_pin_db
                            .as_ref()
                            .unwrap()
                            .lookup(&root),
                        before
                    );
                    h.event(egui::Event::Ime(egui::ImeEvent::Disabled));
                    h.event(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    h.event(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    h.run_steps(2);
                }
                assert!(
                    !h.state().cover_crop_dialog_open(),
                    "ordinary {key:?} closes the editor"
                );
                let after = h
                    .state()
                    .folder_thumb_pin_db
                    .as_ref()
                    .unwrap()
                    .lookup(&root);
                if key == egui::Key::Enter {
                    assert_eq!(
                        after,
                        Some(FolderPin {
                            source: fake_prepared(None).source,
                            cover_crop: Some(crop)
                        })
                    );
                } else {
                    assert_eq!(after, before);
                }
            }
        }
    }

    struct PureWindowState {
        phase: Option<Phase>,
        kind: u8,
        open: bool,
        action: Action,
        watched: Vec<egui::Id>,
        responses: Vec<egui::Response>,
        canvas: Option<(egui::Rect, egui::Rect)>,
    }

    #[test]
    fn cover_crop_pure_window_controls_are_visible_and_clickable_at_100_and_200_percent() {
        for scale in [1., 2.] {
            for kind in [0, 1, 2] {
                let labels: &[&str] = if kind == 1 {
                    &[OK_LABEL, "手動の範囲を解除", "キャンセル", "Close window"]
                } else {
                    &["キャンセル", "Close window"]
                };
                for label in labels {
                    let mut fonts_ready = false;
                    let mut h=crate::test_ui::builder().with_size(egui::vec2(1093.,614.)).build_ui_state(move |ui,state:&mut PureWindowState| {
                        let ctx=ui.ctx();
                        if !fonts_ready {
                            crate::ui_fonts::configure_fonts(ctx);
                            crate::settings::apply_ui_scale_factor(ctx,scale);
                            fonts_ready=true;
                            ctx.request_repaint();
                            return;
                        }
                        let phase=state.phase.get_or_insert_with(||match state.kind {
                            0=>Phase::Loading(mpsc::channel().1),
                            1=>{
                                let prepared=fake_prepared(CoverCrop::new([0,0,500_000,1_000_000]).unwrap());
                                Phase::Ready(CropDraft {source:prepared.source,texture:ctx.load_texture("pure-cover",prepared.image,egui::TextureOptions::LINEAR),rotation:Rotation::None,
                                    crop:crop_rect(prepared.crop),had_manual_crop:true,auto_enabled:true,drag:None,error:None})
                            }
                            _=>Phase::Failed("代表画像を読み込めませんでした。別のページで指定してください。".into()),
                        });
                        let action=draw_window(ctx,&mut state.open,phase,false,false);
                        if action!=Action::None {state.action=action;}
                        state.responses=state.watched.iter().filter_map(|id|ctx.read_response(*id)).collect();
                        state.canvas=ctx.data(|data|data.get_temp(egui::Id::new("cover-crop-canvas-geometry")));
                    },PureWindowState {phase:None,kind,open:true,action:Action::None,watched:vec![],responses:vec![],canvas:None});
                    h.run_steps(6);
                    let viewport = h.ctx.content_rect();
                    assert!((viewport.size() * scale - egui::vec2(1093., 614.)).length() < 1.);
                    if kind == 1 {
                        let (canvas, body) = h.state().canvas.expect("ready canvas geometry");
                        let ok = h.get_by_label(OK_LABEL).rect();
                        let cancel = h.get_by_label("キャンセル").rect();
                        let clear = h.get_by_label("手動の範囲を解除").rect();
                        assert!(ok.right() < cancel.left());
                        assert!(cancel.right() + 16.0 < clear.left());
                        assert!((ok.center().y - clear.center().y).abs() <= 1.0);
                        assert!(
                            body.bottom() < ok.top(),
                            "body must stop above fixed footer"
                        );
                        assert!(
                            body.contains_rect(canvas),
                            "{scale}: initial full image and crop handles {canvas:?} clipped by body {body:?}"
                        );
                        assert!(viewport.contains_rect(canvas));
                        assert!(
                            canvas.height() >= 48.0,
                            "{scale}: crop canvas too short to edit"
                        );
                        h.event(egui::Event::PointerMoved(body.center()));
                        h.event(egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -100000.0),
                            modifiers: egui::Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                        });
                        h.run_steps(40);
                        let note=h.get_by_label("このページの消しゴム・注釈・切り取りは代表サムネには反映されません (色調補正は反映されます)。").rect();
                        assert!(
                            body.contains_rect(note),
                            "{scale}: supplementary edit note is unreachable at scroll end: {note:?} in {body:?}"
                        );
                    }
                    for control in [OK_LABEL, "手動の範囲を解除", "キャンセル", "Close window"]
                    {
                        let node = h.get_by_label(control);
                        assert!(
                            viewport.contains_rect(node.rect()),
                            "{kind} {scale}: {control} outside viewport"
                        );
                    }
                    let node = h.get_by_label(label);
                    let id = unsafe {
                        egui::Id::from_high_entropy_bits(
                            (u128::from(node.accesskit_node().id()) >> 64) as u64,
                        )
                    };
                    h.state_mut().watched = vec![id];
                    h.run_steps(2);
                    let response = &h.state().responses[0];
                    assert!(
                        response.interact_rect.contains_rect(response.rect),
                        "{kind} {scale}: {label} clipped"
                    );
                    let center = response.rect.center();
                    h.hover_at(center);
                    h.run_steps(2);
                    assert!(
                        h.state().responses[0].hovered(),
                        "{kind} {scale}: {label} pointer unreachable"
                    );
                    h.get_by_label(label).click();
                    h.run_steps(2);
                    assert!(
                        match *label {
                            OK_LABEL => h.state().action == Action::Accept,
                            "手動の範囲を解除" => h.state().action == Action::Clear,
                            "キャンセル" => h.state().action == Action::Cancel,
                            _ => !h.state().open,
                        },
                        "{kind} {scale}: {label} click not handled"
                    );
                }
            }
        }
    }

    #[test]
    fn cover_crop_image_height_preserves_1822cd0ef_at_100_and_200_percent() {
        // Measured 1822cd0ef's draw_window/draw_canvas and 720x280 snapshot
        // fixture in an isolated test module, using the same app fonts, themes,
        // 1093x614 viewport, zoom and eight frames as tests/ui_snapshot.rs.
        // Read cover-crop-canvas-geometry; image = canvas.shrink(4.0), the
        // padding at that commit. Values are logical pt, including auto's note.
        let baselines = [
            (1.0, false, 333.875, 325.875),
            (1.0, true, 333.875, 325.875),
            (2.0, false, 101.0, 93.0),
            (2.0, true, 83.5, 75.5),
        ];
        for (scale, auto, old_canvas_height, old_image_height) in baselines {
            let mut fonts_ready = false;
            let mut h = crate::test_ui::builder()
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui(move |ui| {
                    crate::os_theme::apply_resolved(
                        ui.ctx(),
                        if scale == 1.0 {
                            crate::os_theme::ResolvedTheme::Light
                        } else {
                            crate::os_theme::ResolvedTheme::Dark
                        },
                    );
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ui.ctx());
                        crate::settings::apply_ui_scale_factor(ui.ctx(), scale);
                        fonts_ready = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    draw_cover_crop_snapshot(ui, auto);
                });
            h.run_steps(8);
            let (canvas, body) = h
                .ctx
                .data(|data| {
                    data.get_temp::<(egui::Rect, egui::Rect)>(egui::Id::new(
                        "cover-crop-canvas-geometry",
                    ))
                })
                .unwrap();
            let image = canvas.shrink2(CANVAS_PADDING);
            println!(
                "scale={scale} auto={auto}: canvas height={}, image height={}, width={}",
                canvas.height(),
                image.height(),
                image.width()
            );
            assert!(
                canvas.height() >= old_canvas_height,
                "{scale}/{auto}: canvas {canvas:?} shorter than {old_canvas_height}"
            );
            assert!(
                image.height() >= old_image_height,
                "{scale}/{auto}: image {image:?} shorter than {old_image_height}"
            );
            assert!(body.contains_rect(canvas));
            let viewport = h.ctx.content_rect();
            for label in [OK_LABEL, "キャンセル", "手動の範囲を解除", "Close window"] {
                assert!(viewport.contains_rect(h.get_by_label(label).rect()));
            }
            // The full-image hit regions must also fit, regardless of the
            // partial selection painted by this snapshot fixture.
            let window = h
                .ctx
                .memory(|memory| memory.area_rect(egui::Id::new("cover-crop-window")))
                .unwrap();
            let resize = h
                .ctx
                .global_style()
                .interaction
                .resize_grab_radius_corner
                .max(h.ctx.global_style().interaction.resize_grab_radius_side);
            for (point, _) in handles(image) {
                let hit =
                    egui::Rect::from_center_size(point, egui::Vec2::splat(2.0 * CORNER_HIT_RADIUS));
                assert!(canvas.contains_rect(hit));
                assert!(
                    hit.left() > window.left() + resize && hit.right() < window.right() - resize
                );
            }
        }
    }

    #[test]
    fn cover_crop_full_image_handles_clear_window_edges_and_drag_without_resizing() {
        for scale in [1.0, 2.0] {
            for corner in [false, true] {
                let (app, tx, _root, crop) = fake_loading_app();
                let mut h = app_harness(app);
                crate::settings::apply_ui_scale_factor(&h.ctx, scale);
                h.set_size(egui::vec2(1093.0, 614.0) / scale);
                tx.send((7, Ok(fake_prepared(Some(crop))))).unwrap();
                h.run_steps(6);
                if let Phase::Ready(draft) =
                    &mut h.state_mut().cover_crop_dialog.as_mut().unwrap().phase
                {
                    draft.crop = crop_rect(None);
                }
                h.run_steps(3);
                let (canvas, body) = h
                    .ctx
                    .data(|data| {
                        data.get_temp::<(egui::Rect, egui::Rect)>(egui::Id::new(
                            "cover-crop-canvas-geometry",
                        ))
                    })
                    .unwrap();
                let frame = canvas.shrink2(CANVAS_PADDING);
                let window = h
                    .ctx
                    .memory(|memory| memory.area_rect(egui::Id::new("cover-crop-window")))
                    .unwrap();
                let viewport = h.ctx.content_rect();
                let margin = window_margin(viewport);
                assert!(viewport.shrink2(margin).expand(1.0).contains_rect(window));
                let resize = h
                    .ctx
                    .global_style()
                    .interaction
                    .resize_grab_radius_corner
                    .max(h.ctx.global_style().interaction.resize_grab_radius_side);
                assert!(CANVAS_PADDING.x > CORNER_HIT_RADIUS + resize);
                assert!(CANVAS_PADDING.y > CORNER_HIT_RADIUS);
                assert!(body.contains_rect(canvas));
                for (point, _) in handles(frame) {
                    let hit = egui::Rect::from_center_size(
                        point,
                        egui::Vec2::splat(2.0 * CORNER_HIT_RADIUS),
                    );
                    assert!(
                        canvas.contains_rect(hit),
                        "{scale}: handle hit outside canvas"
                    );
                    assert!(
                        hit.left() > window.left() + resize
                            && hit.right() < window.right() - resize,
                        "{scale}: handle overlaps lateral window resize edge"
                    );
                }
                let (start, end, expected_handle) = if corner {
                    let start = frame.left_top() - egui::vec2(CORNER_HIT_RADIUS - 1.0, 0.0);
                    (start, start + egui::vec2(16.0, 16.0), CropHandle::NorthWest)
                } else {
                    let start = frame.right_center() + egui::vec2(EDGE_HIT_RADIUS - 1.0, 0.0);
                    (start, start - egui::vec2(20.0, 0.0), CropHandle::East)
                };
                assert_eq!(hit_handle(frame, start), Some(expected_handle));
                h.event(egui::Event::PointerMoved(start));
                h.run_steps(1);
                h.event(egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run_steps(1);
                h.event(egui::Event::PointerMoved(end));
                h.run_steps(1);
                h.event(egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run_steps(2);
                let after = h
                    .ctx
                    .memory(|memory| memory.area_rect(egui::Id::new("cover-crop-window")))
                    .unwrap();
                assert!(
                    (after.min - window.min).length() <= 1.0
                        && (after.size() - window.size()).length() <= 1.0,
                    "{scale}: crop drag moved/resized window {window:?} -> {after:?}"
                );
                let Phase::Ready(draft) = &h.state().cover_crop_dialog.as_ref().unwrap().phase
                else {
                    panic!("ready crop");
                };
                if corner {
                    assert!(draft.crop.min_x > 0.0 && draft.crop.min_y > 0.0);
                } else {
                    assert!(draft.crop.max_x < UNIT as f32);
                }
            }
        }
    }

    #[test]
    fn cover_crop_rotated_coordinates_round_trip() {
        let original = crop_rect(CoverCrop::new([123_000, 75_000, 840_000, 950_000]).unwrap());
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let back = rotate_rect(rotate_rect(original, rotation), inverse(rotation));
            for (actual, expected) in [back.min_x, back.min_y, back.max_x, back.max_y]
                .into_iter()
                .zip([
                    original.min_x,
                    original.min_y,
                    original.max_x,
                    original.max_y,
                ])
            {
                assert!((actual - expected).abs() < 0.2);
            }
        }
        let display = rotate_rect(original, Rotation::Cw90);
        assert!((display.min_x - 50_000.).abs() < 0.2);
        assert!((display.min_y - 123_000.).abs() < 0.2);
    }

    #[test]
    fn cover_crop_closed_dialog_retires_worker_result() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let dialog = CoverCropDialog {
            generation: 7,
            container: PathBuf::from("book.zip"),
            cancel: cancel.clone(),
            phase: Phase::Loading(rx),
        };
        drop(dialog);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(tx.send((7, Err("late result".into()))).is_err());
    }

    #[test]
    fn cover_crop_all_edges_and_body_have_distinct_hit_targets() {
        let rect = egui::Rect::from_min_max(egui::pos2(20., 30.), egui::pos2(220., 330.));
        for (point, handle) in handles(rect) {
            assert_eq!(hit_handle(rect, point), Some(handle));
        }
        assert_eq!(
            hit_handle(rect, egui::pos2(20., 100.)),
            Some(CropHandle::West)
        );
        assert_eq!(hit_handle(rect, rect.center()), None);
    }

    #[test]
    fn cover_crop_menu_admission_uses_logical_owners_without_source_io() {
        let mut app = crate::app::setup_app_for_test();
        app.current_folder = Some(crate::app::search_results_synthetic_path());
        app.items = vec![
            GridItem::ZipFile(PathBuf::from("C:/missing/book.zip")),
            GridItem::Image(PathBuf::from("C:/missing/page.png")),
            GridItem::Folder(crate::app::search_results_synthetic_path()),
        ];
        assert!(
            app.cover_crop_menu_state(0)
                .is_some_and(|state| state.enabled),
            "a real book tile in a synthetic list retains its own owner"
        );
        assert!(
            app.cover_crop_menu_state(1).is_none(),
            "a synthetic list is not a page's pin owner"
        );
        assert!(app.cover_crop_menu_state(2).is_none());
        assert!(app.cover_crop_menu_state(3).is_none());

        app.items_are_global_search_view = true;
        for idx in 0..app.items.len() {
            assert!(app.cover_crop_target(idx).is_none());
            assert!(app.cover_crop_menu_state(idx).is_none());
        }

        app.items_are_global_search_view = false;
        app.current_folder = Some(PathBuf::from("C:/missing"));
        app.items = vec![
            GridItem::Folder(PathBuf::from("C:/")),
            GridItem::Image(PathBuf::from("C:/missing/page.cr2")),
            GridItem::ConvertibleArchive {
                path: PathBuf::from("C:/missing/book.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
        ];
        assert!(app.cover_crop_menu_state(0).is_none());
        assert!(app.cover_crop_menu_state(1).is_none());
        assert!(
            app.cover_crop_menu_state(2)
                .is_some_and(|state| !state.enabled)
        );
        let archive_path = PathBuf::from("C:/missing/book.7z");
        app.converted_archive_cache_paths.insert(
            crate::path_key::normalize_keep_drive(&archive_path),
            crate::app::ConvertedArchiveSourceState::CachedZip {
                logical_source: archive_path,
                path: PathBuf::from("C:/missing/cache/book.zip"),
            },
        );
        assert!(
            app.cover_crop_menu_state(2)
                .is_some_and(|state| state.enabled),
            "worker-confirmed conversion availability needs no source I/O"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cover_crop_menu_excludes_drive_and_share_roots() {
        let mut app = crate::app::setup_app_for_test();
        for root in ["C:/", "//server/share/"] {
            app.items = vec![GridItem::Folder(PathBuf::from(root))];
            assert!(app.cover_crop_target(0).is_none(), "{root}");
            assert!(app.cover_crop_menu_state(0).is_none(), "{root}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn cover_crop_menu_excludes_detached_owner_and_preserves_main_admission() {
        let mut app = crate::app::setup_app_for_test();
        let item = GridItem::ZipFile(PathBuf::from("C:/missing/book.zip"));
        app.items = vec![item.clone()];
        assert!(app.cover_crop_menu_state(0).is_some());
        let detached = app.build_window_context_for_test(764_081, |owner| {
            owner.items = vec![item];
        });
        app.with_window_context_for_test(detached, |owner| {
            assert!(owner.cover_crop_target(0).is_none());
            assert!(owner.cover_crop_menu_state(0).is_none());
        });
        assert!(app.cover_crop_menu_state(0).is_some());
    }
}

#[cfg(test)]
#[path = "cover_crop_rotation_tests.rs"]
mod rotation_tests;
