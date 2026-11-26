//! One thread owns the non-clone edit snapshots from preview through writing.
use super::*;
use crate::export_batch::BatchExportItem;
use crate::export_dialog::{
    ExportEvent, ExportFailure, ExportOrigin, ExportPending, ExportSuccess,
};
use image::RgbaImage;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderSpec {
    pub options: CombineOptions,
    /// Stable initial IDs; ordering and overrides travel together.
    pub items: Vec<(usize, CombineItemSpec)>,
}

pub(crate) struct WriteSpec {
    pub render: RenderSpec,
    pub directory: PathBuf,
    pub name: String,
    pub replace: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub progress: mpsc::Sender<ExportEvent>,
}

pub(crate) enum Command {
    Prepare {
        id: usize,
        item: Box<BatchExportItem>,
    },
    Remove {
        ids: Vec<usize>,
    },
    Render {
        generation: u64,
        spec: RenderSpec,
    },
    Write(WriteSpec),
}

pub(crate) enum Event {
    Loaded {
        id: usize,
        dims: (u32, u32),
    },
    Failed {
        id: usize,
        message: String,
    },
    Rendered {
        generation: u64,
        rgba: RgbaImage,
        layout: Layout,
        not_drawn: usize,
    },
    RenderFailed {
        generation: u64,
        message: String,
    },
}

pub(crate) struct Connection {
    pub tx: mpsc::Sender<Command>,
    pub rx: mpsc::Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
}

impl Connection {
    /// Transfer progress ownership without recreating snapshots or spawning a thread.
    pub fn write(
        &self,
        spec: RenderSpec,
        directory: PathBuf,
        name: String,
    ) -> Result<ExportPending, String> {
        self.write_with_target(spec, directory, name, None, crate::data_dir::get())
    }

    pub fn write_with_target(
        &self,
        spec: RenderSpec,
        directory: PathBuf,
        name: String,
        replace: Option<PathBuf>,
        data_dir: PathBuf,
    ) -> Result<ExportPending, String> {
        let total = spec.items.len() + 1;
        let (progress, rx) = mpsc::channel();
        self.tx
            .send(Command::Write(WriteSpec {
                render: spec,
                directory,
                name,
                replace,
                data_dir,
                progress,
            }))
            .map_err(|_| "結合 worker が中断されました".to_string())?;
        Ok(ExportPending {
            origin: ExportOrigin::Combine,
            cancel: self.cancel.clone(),
            rx,
            total,
            done: 0,
            last_message: "準備中".into(),
            successes: Vec::new(),
            errors: Vec::new(),
            skipped: Vec::new(),
            thumbnail_notices: Vec::new(),
            end: None,
            cancel_requested: false,
        })
    }
}

pub(crate) fn spawn(
    count: usize,
    raw: Option<crate::raw::RawDecodeContext>,
    lease: crate::app::LocalAiActivityLease,
    ctx: egui::Context,
) -> Result<Connection, String> {
    let (tx, commands) = mpsc::channel();
    let (events, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("image-combine".into())
        .spawn(move || {
            let _lease = lease;
            let mut worker = Worker {
                items: HashMap::new(),
                load: Box::new(move |item, cancel| materialize(item, raw.as_ref(), cancel)),
                analyze_pdf: Box::new(|path, page, password, cancel| {
                    crate::pdf_loader::analyze_page_content_type(path, page, password, Some(cancel))
                        .map(|analysis| analysis.content_type)
                        .map_err(|error| format!("PDF ページを解析できません: {error}"))
                }),
                cancel: worker_cancel,
                #[cfg(test)]
                before_encode: None,
                #[cfg(test)]
                before_write: None,
                small_edge: (2048 / (count as f64).sqrt().ceil().max(1.0) as u32).clamp(256, 1024),
                fonts: None,
            };
            worker.run(commands, events, ctx);
        })
        .map_err(|e| format!("結合 worker を開始できません: {e}"))?;
    Ok(Connection { tx, rx, cancel })
}

struct LoadedItem {
    item: Box<BatchExportItem>,
    dims: (u32, u32),
    small: RgbaImage,
}

type Materializer =
    Box<dyn FnMut(&BatchExportItem, &Arc<AtomicBool>) -> Result<LoadedPixels, String> + Send>;

type PdfAnalyzer = Box<
    dyn FnMut(
            &Path,
            u32,
            Option<&str>,
            Arc<AtomicBool>,
        ) -> Result<crate::pdf_loader::PdfPageContentType, String>
        + Send,
>;

struct LoadedPixels {
    image: RgbaImage,
    decode_ms: f64,
    compose_ms: f64,
    #[cfg(test)]
    guard: Option<ImageCountGuard>,
}

#[cfg(test)]
struct ImageCountGuard(Arc<std::sync::atomic::AtomicUsize>);
#[cfg(test)]
impl Drop for ImageCountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Worker {
    items: HashMap<usize, LoadedItem>,
    load: Materializer,
    analyze_pdf: PdfAnalyzer,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    before_encode: Option<Box<dyn FnMut(&AtomicBool) + Send>>,
    #[cfg(test)]
    before_write: Option<Box<dyn FnMut(&AtomicBool) + Send>>,
    small_edge: u32,
    // Outer Option records that loading was attempted, including failure.
    fonts: Option<Option<comic_core::FontSet>>,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("キャンセルしました".into())
    } else {
        Ok(())
    }
}

fn materialize(
    item: &BatchExportItem,
    raw: Option<&crate::raw::RawDecodeContext>,
    cancel: &Arc<AtomicBool>,
) -> Result<LoadedPixels, String> {
    check_cancel(cancel)?;
    if let Some(check) = &item.composition_check {
        if !check.permits()? {
            return Err(crate::book_composition::COMPOSITION_OUTPUT_HIDDEN_NOTICE.into());
        }
    }
    let start = Instant::now();
    let pixels = crate::books::decode_composite_source_for_materialization(
        &item.source,
        4096,
        cancel.clone(),
        raw,
    )
    .map_err(|message| source_load_error(&item.source, message))?;
    let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let pixels =
        crate::books::compose_book_page_for_materialization(pixels, &item.edits, cancel.clone())?;
    check_cancel(cancel)?;
    let compose_ms = start.elapsed().as_secs_f64() * 1000.0;
    let rgba = RgbaImage::from_raw(
        pixels.size[0] as u32,
        pixels.size[1] as u32,
        crate::save_with_metadata::color_image_to_rgba(&pixels),
    )
    .ok_or("画像の寸法が無効です")?;
    Ok(LoadedPixels {
        image: rgba,
        decode_ms,
        compose_ms,
        #[cfg(test)]
        guard: None,
    })
}

fn source_load_error(source: &crate::books::CompositeSource, message: String) -> String {
    if matches!(
        source,
        crate::books::CompositeSource::PdfPage { password: None, .. }
    ) && (message.to_ascii_lowercase().contains("password") || message.contains("パスワード"))
    {
        "パスワードが必要です (一度 PDF を開いて入力してください)".to_string()
    } else {
        message
    }
}

impl Worker {
    /// Complete an absent page's annotation coordinate space once, before the
    /// snapshot is accepted. Preview and output then keep that same snapshot.
    fn prepare_pdf_comic_snapshot(&mut self, item: &mut BatchExportItem) -> Result<(), String> {
        if item.edits.comic.is_none() || item.edits.comic_source_dims.is_some() {
            return Ok(());
        }
        let crate::books::CompositeSource::PdfPage {
            pdf_path,
            page_num,
            password,
        } = &item.source
        else {
            return Ok(());
        };
        check_cancel(&self.cancel)?;
        let content_type = (self.analyze_pdf)(
            pdf_path,
            *page_num,
            password.as_deref(),
            self.cancel.clone(),
        )
        .map_err(|message| source_load_error(&item.source, message))?;
        check_cancel(&self.cancel)?;
        item.edits.comic_source_dims = crate::pdf_loader::canonical_pdf_raster_dims(content_type)
            .map(|[width, height]| [width as usize, height as usize]);
        if item.edits.comic_source_dims.is_none() {
            return Err("注釈の位置を再現できません。一覧から結合し直してください".into());
        }
        Ok(())
    }

    fn run(
        &mut self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<Event>,
        ctx: egui::Context,
    ) {
        while let Ok(command) = commands.recv() {
            let mut latest = None;
            let mut next = Some(command);
            // Drain only queued requests, preserving Prepare/Remove/Write ordering.
            while let Some(command) = next {
                match command {
                    Command::Prepare { id, mut item } => {
                        let start = Instant::now();
                        let loaded = self
                            .prepare_pdf_comic_snapshot(&mut item)
                            .and_then(|()| (self.load)(&item, &self.cancel));
                        let event = match loaded {
                            Ok(pixels) => {
                                let image = &pixels.image;
                                let dims = image.dimensions();
                                let scale =
                                    (self.small_edge as f64 / dims.0.max(dims.1) as f64).min(1.0);
                                let small = crate::fast_resize::resize_rgba8_exact(
                                    &image,
                                    (dims.0 as f64 * scale).round().max(1.0) as u32,
                                    (dims.1 as f64 * scale).round().max(1.0) as u32,
                                    crate::fast_resize::Quality::Lanczos3,
                                );
                                self.items.insert(id, LoadedItem { item, dims, small });
                                Event::Loaded { id, dims }
                            }
                            Err(message) => Event::Failed { id, message },
                        };
                        if crate::perf::is_enabled() {
                            crate::perf::event(
                                "image_combine/preview_load",
                                "complete",
                                None,
                                id as u64,
                                &[(
                                    "ms",
                                    serde_json::json!(start.elapsed().as_secs_f64() * 1000.0),
                                )],
                            );
                        }
                        if events.send(event).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                    Command::Remove { ids } => {
                        for id in ids {
                            self.items.remove(&id);
                        }
                    }
                    Command::Render { generation, spec } => latest = Some((generation, spec)),
                    Command::Write(spec) => {
                        let progress = spec.progress.clone();
                        match self.write(&spec) {
                            Ok(Some(path)) => {
                                let recipe_error =
                                    save_recipe_stage_c_hook(&path, &spec, &self.items).err();
                                let _ = progress.send(ExportEvent::Completed(ExportSuccess {
                                    label: path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                    path,
                                }));
                                if let Some(message) = recipe_error {
                                    crate::logger::log(format!(
                                        "image combine recipe save failed: {message}"
                                    ));
                                    let _ = progress.send(ExportEvent::Failed(ExportFailure {
                                        label: "構成の保存".into(),
                                        message: format!("構成を保存できませんでした。やり直しは使えません: {message}"),
                                    }));
                                }
                                let _ = progress.send(ExportEvent::AllDone);
                            }
                            Ok(None) => {
                                let _ = progress.send(ExportEvent::Cancelled);
                            }
                            Err(message) => {
                                crate::logger::log(format!(
                                    "image combine write failed: {message}"
                                ));
                                let _ = progress.send(ExportEvent::Failed(ExportFailure {
                                    label: "画像の結合".into(),
                                    message,
                                }));
                                let _ = progress.send(ExportEvent::AllDone);
                            }
                        }
                        ctx.request_repaint();
                        return;
                    }
                }
                next = commands.try_recv().ok();
            }
            if self.cancel.load(Ordering::Relaxed) {
                continue;
            }
            if let Some((generation, spec)) = latest {
                let start = Instant::now();
                let event = match self.preview(&spec) {
                    Ok((rgba, layout, not_drawn)) => Event::Rendered {
                        generation,
                        rgba,
                        layout,
                        not_drawn,
                    },
                    Err(message) => Event::RenderFailed {
                        generation,
                        message,
                    },
                };
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "image_combine/preview_render",
                        "complete",
                        None,
                        generation,
                        &[(
                            "ms",
                            serde_json::json!(start.elapsed().as_secs_f64() * 1000.0),
                        )],
                    );
                }
                if events.send(event).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        }
    }

    fn plan(&mut self, spec: &RenderSpec) -> Result<(Layout, LabelLayout), String> {
        let dims = spec
            .items
            .iter()
            .map(|(id, _)| {
                self.items
                    .get(id)
                    .map(|item| item.dims)
                    .ok_or_else(|| "読めない画像を外してください".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let layout = layout(&spec.options, &dims, spec.options.labels.band_height())
            .map_err(|e| e.to_string())?;
        let fonts = self
            .fonts
            .get_or_insert_with(crate::comic_overlay::load_comic_fonts);
        if spec.options.labels.placement != LabelPlacement::None && fonts.is_none() {
            return Err("ラベルのフォントを読めません".into());
        }
        let labels = if let Some(fonts) = fonts {
            label_layout(
                &layout,
                &spec
                    .items
                    .iter()
                    .map(|(_, item)| item.clone())
                    .collect::<Vec<_>>(),
                &spec.options,
                fonts,
            )
        } else {
            LabelLayout {
                labels: vec![None; dims.len()],
                not_drawn: 0,
            }
        };
        Ok((layout, labels))
    }

    fn preview(&mut self, spec: &RenderSpec) -> Result<(RgbaImage, Layout, usize), String> {
        let (layout, labels) = self.plan(spec)?;
        let scale = (1600.0 / layout.canvas_size.0.max(layout.canvas_size.1) as f64).min(1.0);
        let mut canvas = create_canvas(&layout, &spec.options, scale).map_err(|e| e.to_string())?;
        for (index, (id, _)) in spec.items.iter().enumerate() {
            check_cancel(&self.cancel)?;
            draw_card(
                &mut canvas,
                &layout.cards[index],
                &self.items[id].small,
                &spec.options,
                scale,
            )
            .map_err(|e| e.to_string())?;
            if let (Some(label), Some(Some(fonts))) = (&labels.labels[index], &self.fonts) {
                draw_label(
                    &mut canvas,
                    &layout.cards[index],
                    label,
                    &spec.options,
                    scale,
                    fonts,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Ok((canvas, layout, labels.not_drawn))
    }

    fn encode(&mut self, spec: &WriteSpec) -> Result<Vec<u8>, String> {
        let (layout, labels) = self.plan(&spec.render)?;
        let options = &spec.render.options;
        check_cancel(&self.cancel)?;
        let mut canvas = create_canvas(&layout, options, 1.0).map_err(|e| e.to_string())?;
        for (index, (id, _)) in spec.render.items.iter().enumerate() {
            check_cancel(&self.cancel)?;
            let loaded = &self.items[id];
            let pixels = (self.load)(&loaded.item, &self.cancel)?;
            let image = &pixels.image;
            let (decode_ms, compose_ms) = (pixels.decode_ms, pixels.compose_ms);
            if image.dimensions() != loaded.dims {
                return Err("元の画像が変わりました。開き直してください".into());
            }
            // draw_card owns resize and releases its temporary image before returning.
            let (resize_ms, draw_ms) = super::render::draw_card_timed(
                &mut canvas,
                &layout.cards[index],
                image,
                options,
                1.0,
            )
            .map_err(|e| e.to_string())?;
            drop(pixels);
            if let (Some(label), Some(Some(fonts))) = (&labels.labels[index], &self.fonts) {
                draw_label(
                    &mut canvas,
                    &layout.cards[index],
                    label,
                    options,
                    1.0,
                    fonts,
                )
                .map_err(|e| e.to_string())?;
            }
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "image_combine/item",
                    "complete",
                    None,
                    *id as u64,
                    &[
                        ("decode_ms", serde_json::json!(decode_ms)),
                        ("compose_ms", serde_json::json!(compose_ms)),
                        ("resize_ms", serde_json::json!(resize_ms)),
                        ("draw_ms", serde_json::json!(draw_ms)),
                    ],
                );
            }
            let _ = spec.progress.send(ExportEvent::Step {
                label: loaded.item.filename.clone(),
            });
        }
        #[cfg(test)]
        if let Some(hook) = &mut self.before_encode {
            hook(&self.cancel);
        }
        check_cancel(&self.cancel)?; // Before encoding.
        let start = Instant::now();
        let (format, extension, save_options) = encoding_options(options.format);
        let encoded = crate::save_with_metadata::encode_rgba_with_metadata(
            canvas.as_raw(),
            canvas.width(),
            canvas.height(),
            None,
            None,
            format,
            &save_options,
        )
        .map_err(|e| e.to_string())?;
        if crate::perf::is_enabled() {
            crate::perf::event(
                "image_combine/encode",
                "complete",
                None,
                0,
                &[
                    ("width", serde_json::json!(canvas.width())),
                    ("height", serde_json::json!(canvas.height())),
                    ("format", serde_json::json!(extension)),
                    (
                        "ms",
                        serde_json::json!(start.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        drop(canvas);
        Ok(encoded.bytes)
    }

    fn write(&mut self, spec: &WriteSpec) -> Result<Option<PathBuf>, String> {
        // Establish the protected identities before materializing any source.
        if let Some(path) = &spec.replace {
            protect_replace_sources(path, &spec.render)?;
        }
        let encoded = match self.encode(spec) {
            Ok(bytes) => bytes,
            Err(_) if self.cancel.load(Ordering::Relaxed) => return Ok(None),
            Err(message) => return Err(message),
        };
        #[cfg(test)]
        if let Some(hook) = &mut self.before_write {
            hook(&self.cancel);
        }
        let (_, extension, _) = encoding_options(spec.render.options.format);
        if let Some(path) = &spec.replace {
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let previous = match std::fs::metadata(path) {
                Ok(metadata) => Some(metadata.modified().map_err(|e| e.to_string())?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            // Final cancellation boundary: once the temporary file starts, finish the write.
            if self.cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            crate::export_batch::write_output(path, &encoded, true)?;
            crate::export_batch::set_replacement_file_time(
                path,
                previous,
                std::time::SystemTime::now(),
            )?;
            return Ok(Some(path.clone()));
        }
        write_new(
            &spec.directory,
            &spec.name,
            extension,
            &encoded,
            &self.cancel,
        )
    }
}

fn encoding_options(
    format: CombineFormat,
) -> (
    crate::save_with_metadata::SrcFormat,
    &'static str,
    crate::save_with_metadata::SaveOptions,
) {
    use crate::save_with_metadata::{SaveOptions, SrcFormat};
    let mut options = SaveOptions {
        include_metadata: false,
        ..Default::default()
    };
    let (format, extension) = match format {
        CombineFormat::Png => (SrcFormat::Png, "png"),
        CombineFormat::Jpeg { quality } => {
            options.jpeg_quality = quality;
            (SrcFormat::Jpeg, "jpg")
        }
        CombineFormat::Webp { quality, lossless } => {
            options.webp_quality = quality as f32;
            options.webp_lossless = lossless;
            (SrcFormat::Webp, "webp")
        }
    };
    (format, extension, options)
}

fn write_new(
    directory: &Path,
    name: &str,
    extension: &str,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>, String> {
    write_new_with(directory, name, extension, cancel, |file| {
        file.write_all(bytes)
    })
}

fn write_new_with(
    directory: &Path,
    name: &str,
    extension: &str,
    cancel: &AtomicBool,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<Option<PathBuf>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let name = crate::capture::basename_from_text(name);
    for seq in 0..=9999 {
        let stem = if seq == 0 {
            name.clone()
        } else {
            format!("{name}_{seq}")
        };
        let path = directory.join(format!("{stem}.{extension}"));
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        } // No cancellation after create_new succeeds.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                write(&mut file).map_err(|e| e.to_string())?;
                return Ok(Some(path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("同名ファイルが多すぎます".into())
}

fn save_recipe_stage_c_hook(
    path: &Path,
    spec: &WriteSpec,
    snapshots: &HashMap<usize, LoadedItem>,
) -> Result<(), String> {
    // The worker's accepted snapshots are authoritative. UI specs only own the
    // current order and labels; their recorded canvas may predate this write.
    let items = spec
        .render
        .items
        .iter()
        .map(|(id, spec)| {
            let loaded = snapshots.get(id).ok_or("構成の元画像を特定できません")?;
            let mut spec = spec.clone();
            spec.annotation_space = loaded
                .item
                .edits
                .comic_source_dims
                .map(|[width, height]| {
                    Ok::<_, String>([
                        u32::try_from(width).map_err(|_| "注釈の基準寸法が大きすぎます")?,
                        u32::try_from(height).map_err(|_| "注釈の基準寸法が大きすぎます")?,
                    ])
                })
                .transpose()?;
            Ok(spec)
        })
        .collect::<Result<Vec<_>, String>>()?;
    super::store::save_recipe(
        &spec.data_dir,
        path,
        &CombineRecipe {
            v: 1,
            options: spec.render.options.clone(),
            format: spec.render.options.format,
            items,
        },
    )
}

fn protect_replace_sources(output: &Path, spec: &RenderSpec) -> Result<(), String> {
    let output_key = crate::adjustment_db::normalize_path(
        &std::path::absolute(output).map_err(|e| e.to_string())?,
    );
    let canonical_output = match output.canonicalize() {
        Ok(path) => Some(crate::adjustment_db::normalize_path(&path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("保存先を確認できません: {error}")),
    };
    for (_, item) in &spec.items {
        let source = super::store::source_path(&item.source);
        let absolute = std::path::absolute(source).map_err(|e| e.to_string())?;
        let canonical = source
            .canonicalize()
            .map_err(|e| format!("元ファイルを確認できません: {}: {e}", source.display()))?;
        if output_key == crate::adjustment_db::normalize_path(&absolute)
            || canonical_output.as_deref()
                == Some(crate::adjustment_db::normalize_path(&canonical).as_str())
        {
            return Err("元ファイルは上書きしません".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod image_combine_tests;
