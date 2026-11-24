//! Grid batch export owns its frozen options, output reservations and cancel token.
//! Decoding and saved-edit composition remain shared with books/materialization.

mod options;
mod paths;
pub use options::*;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Instant, SystemTime};

use crate::export_dialog::{ExportEvent, ExportFailure, ExportPending, ExportSuccess};
use crate::save_with_metadata::{SaveOptions, SrcFormat, ThumbnailNoticeKinds};
use paths::{OutputReservation, Target};

pub const TEMPLATE_PLACEHOLDER_HINT: &str =
    "<filename> / <dirname> / <num> / <num:N> / <ext> / <date> / <time> が使えます";

pub struct BatchExportItem {
    pub filename: String,
    pub dirname: String,
    pub source_dir: PathBuf,
    pub source_ext: String,
    pub source: crate::books::CompositeSource,
    pub edits: crate::books::BakedEditSnapshot,
    pub(crate) composition_check: Option<crate::book_composition::CompositionOutputCheck>,
}

pub(super) fn source_path(source: &crate::books::CompositeSource) -> &Path {
    match source {
        crate::books::CompositeSource::File { path } => path,
        crate::books::CompositeSource::ZipEntry { zip_path, .. } => zip_path,
        crate::books::CompositeSource::PdfPage { pdf_path, .. } => pdf_path,
    }
}

impl BatchExportItem {
    /// Pure path/string work only: safe while preparing on the UI thread.
    pub fn new(
        filename: String,
        dirname: String,
        source: crate::books::CompositeSource,
        edits: crate::books::BakedEditSnapshot,
    ) -> Self {
        let source_dir = source_path(&source)
            .parent()
            .unwrap_or(Path::new(""))
            .to_owned();
        let source_ext = match &source {
            crate::books::CompositeSource::File { path } => extension(path),
            crate::books::CompositeSource::ZipEntry { entry_name, .. } => {
                extension(Path::new(entry_name))
            }
            crate::books::CompositeSource::PdfPage { .. } => "pdf".into(),
        };
        Self {
            filename,
            dirname,
            source_dir,
            source_ext,
            source,
            edits,
            composition_check: None,
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub struct BatchExportRequest {
    pub options: BatchExportOptions,
    pub items: Vec<BatchExportItem>,
    pub raw: Option<crate::raw::RawDecodeContext>,
    /// Held for the entire worker lifetime, including cancellation and panic.
    pub local_ai_activity: crate::app::LocalAiActivityLease,
}

/// Expand only known tokens once; a source filename containing token text is literal.
pub fn expand_filename_template(
    template: &str,
    filename: &str,
    dirname: &str,
    index: usize,
) -> String {
    expand_template(template, filename, dirname, "", index, None)
}

fn expand_template(
    template: &str,
    filename: &str,
    dirname: &str,
    ext: &str,
    index: usize,
    datetime: Option<&(String, String)>,
) -> String {
    let mut rest = template;
    let mut output = String::with_capacity(template.len());
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let token = &rest[..=end];
        let replacement = match token {
            "<filename>" => Some(filename.to_owned()),
            "<dirname>" => Some(dirname.to_owned()),
            "<ext>" => Some(ext.to_owned()),
            "<num>" => Some(format!("{index:04}")),
            "<date>" => datetime.map(|v| v.0.clone()),
            "<time>" => datetime.map(|v| v.1.clone()),
            _ => token
                .strip_prefix("<num:")
                .and_then(|s| s.strip_suffix('>'))
                .filter(|s| s.len() == 1)
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| (1..=9).contains(n))
                .map(|width| format!("{index:0width$}")),
        };
        output.push_str(replacement.as_deref().unwrap_or(token));
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}

pub fn resolve_item_stem(template: &str, item: &BatchExportItem, index: usize) -> String {
    item_stem(template, item, index, None)
}

/// The UI never stats sources. Keep deferred date/time tokens visible in its example,
/// while applying the same filename sanitation to everything else.
pub(crate) fn preview_item_stem(template: &str, item: &BatchExportItem, index: usize) -> String {
    let expanded = expand_template(
        template,
        &item.filename,
        &item.dirname,
        &item.source_ext,
        index,
        None,
    );
    let mut marker = "__miv_export_deferred__".to_owned();
    while expanded.contains(&marker) {
        marker.push('_');
    }
    let date = format!("{marker}date");
    let time = format!("{marker}time");
    let text = expanded.replace("<date>", &date).replace("<time>", &time);
    crate::capture::basename_from_text(&text)
        .replace(&date, "<date>")
        .replace(&time, "<time>")
}

fn item_stem(
    template: &str,
    item: &BatchExportItem,
    index: usize,
    datetime: Option<&(String, String)>,
) -> String {
    crate::capture::basename_from_text(&expand_template(
        template,
        &item.filename,
        &item.dirname,
        &item.source_ext,
        index,
        datetime,
    ))
}

pub fn spawn_batch_export_worker(request: BatchExportRequest) -> Result<ExportPending, String> {
    let total = request.items.len();
    if total == 0 {
        return Err("エクスポートする画像がありません".into());
    }
    let BatchExportRequest {
        options,
        items,
        raw,
        local_ai_activity,
    } = request;
    let plan = BatchExportPlan {
        options,
        items,
        raw,
    };
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("ctrl-e-batch-export".into())
        .spawn(move || {
            let _local_ai_activity = local_ai_activity;
            run_batch_export(plan, worker_cancel, tx)
        })
        .map_err(|e| format!("一括エクスポート worker を開始できません: {e}"))?;
    Ok(ExportPending {
        cancel,
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

struct BatchExportPlan {
    options: BatchExportOptions,
    items: Vec<BatchExportItem>,
    raw: Option<crate::raw::RawDecodeContext>,
}

#[derive(Default)]
struct ItemTimings {
    decode: f64,
    compose: f64,
    resize: f64,
    encode: f64,
    write: f64,
    dimensions: [usize; 2],
}

enum ItemResult {
    Skipped(PathBuf),
    Written {
        path: PathBuf,
        kinds: ThumbnailNoticeKinds,
        replaced: bool,
    },
}

fn run_batch_export(
    request: BatchExportPlan,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ExportEvent>,
) {
    run_batch_export_with_decoder(request, cancel, tx, &mut |item, cancel, raw, keep_bytes| {
        if keep_bytes {
            crate::books::decode_composite_source_for_materialization_with_bytes(
                &item.source,
                4096,
                cancel,
                raw,
            )
        } else {
            crate::books::decode_composite_source_for_materialization(
                &item.source,
                4096,
                cancel,
                raw,
            )
            .map(|image| (image, None))
        }
    });
}

fn run_batch_export_with_decoder(
    request: BatchExportPlan,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ExportEvent>,
    decode: &mut impl FnMut(
        &BatchExportItem,
        Arc<AtomicBool>,
        Option<&crate::raw::RawDecodeContext>,
        bool,
    ) -> Result<(eframe::egui::ColorImage, Option<Vec<u8>>), String>,
) {
    let prepared = Instant::now();
    let base = paths::common_ancestor(&request.items);
    let mut outputs = OutputReservation::default();
    if request.options.overwrite == OverwritePolicy::Replace {
        if let Err(message) = outputs.protect_sources(&request.items) {
            for item in &request.items {
                log::error!("batch export {}: {message}", item.filename);
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: item.filename.clone(),
                    message: message.clone(),
                }));
            }
            let _ = tx.send(ExportEvent::AllDone);
            return;
        }
    }
    crate::perf::event(
        "export_batch/prepare",
        "complete",
        None,
        0,
        &[(
            "ms",
            serde_json::json!(prepared.elapsed().as_secs_f64() * 1000.0),
        )],
    );
    let mut resolver = crate::book_composition::CompositionOutputResolver::default();
    for (index, item) in request.items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let label = item.filename.clone();
        let _ = tx.send(ExportEvent::Started {
            label: label.clone(),
        });
        let mut timings = ItemTimings::default();
        let result = (|| {
            if let Some(check) = &item.composition_check {
                if !check.permits_with(&mut resolver)? {
                    return Err(crate::book_composition::COMPOSITION_OUTPUT_HIDDEN_NOTICE.into());
                }
            }
            let options = &request.options;
            let keep_file_time = options.can_keep_file_time() && options.keep_file_time;
            let needs_datetime =
                options.template.contains("<date>") || options.template.contains("<time>");
            let modified = if needs_datetime || keep_file_time {
                Some(
                    std::fs::metadata(source_path(&item.source))
                        .and_then(|m| m.modified())
                        .map_err(|e| format!("元の更新日時を取得できません: {e}"))?,
                )
            } else {
                None
            };
            let datetime = if needs_datetime {
                Some(local_datetime(modified.unwrap())?)
            } else {
                None
            };
            let stem = item_stem(&options.template, item, index + 1, datetime.as_ref());
            let directory = paths::output_directory(&options.destination, item, base.as_deref());
            let format = options.format.resolve(&item.source_ext);
            let target =
                outputs.reserve(&directory, &stem, format.extension(), options.overwrite)?;
            let (path, replace) = match target {
                Target::Write { path, replace } => (path, replace),
                Target::Skip(path) => return Ok(ItemResult::Skipped(path)),
            };
            let keep_metadata = options.keep_metadata
                && source_metadata_format(&item.source_ext) == Some(format.clone())
                && !matches!(item.source, crate::books::CompositeSource::PdfPage { .. });
            let start = Instant::now();
            let (pixels, bytes) = decode(
                item,
                Arc::clone(&cancel),
                request.raw.as_ref(),
                keep_metadata,
            )?;
            timings.decode = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let pixels = crate::books::compose_book_page_for_materialization(
                pixels,
                &item.edits,
                Arc::clone(&cancel),
            )?;
            timings.compose = start.elapsed().as_secs_f64() * 1000.0;
            let target_size = options.resize.scaled_size(pixels.size);
            timings.dimensions = target_size;
            validate_dimensions(&format, target_size)?;
            let start = Instant::now();
            let pixels = crate::export_dialog::resize_export_pixels(
                Cow::Owned(pixels),
                target_size,
                options.resize.filter.into(),
            )?;
            timings.resize = start.elapsed().as_secs_f64() * 1000.0;
            ensure_not_cancelled(&cancel)?;
            let save_options = SaveOptions {
                jpeg_quality: options.format.jpeg_quality.clamp(1, 100),
                webp_quality: options.format.webp_quality.clamp(1, 100) as f32,
                webp_lossless: options.format.webp_lossless,
                jpeg_matte: item.edits.jpeg_matte,
                include_metadata: keep_metadata,
                batch_metadata: true,
                thumbnail_notice_for_hidden_edits: item.edits.erase.is_some()
                    || item.edits.conceal.is_some()
                    || item.edits.export_crop.is_some()
                    || item.edits.comic.is_some()
                    || item.edits.local_adjust.is_some(),
                ..Default::default()
            };
            let src_path = if keep_metadata && bytes.is_none() {
                Some(source_path(&item.source))
            } else {
                None
            };
            let start = Instant::now();
            let encoded = crate::save_with_metadata::encode_image_with_metadata_report(
                pixels.as_ref(),
                src_path,
                bytes.as_deref(),
                format,
                &save_options,
            )
            .map_err(|e| e.to_string())?;
            timings.encode = start.elapsed().as_secs_f64() * 1000.0;
            ensure_not_cancelled(&cancel)?;
            let start = Instant::now();
            let previous_modified = if replace {
                Some(
                    std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .map_err(|e| format!("上書き前の更新日時を取得できません: {e}"))?,
                )
            } else {
                None
            };
            write_output(
                &path,
                &encoded.bytes,
                options.overwrite == OverwritePolicy::Replace,
            )?;
            outputs.record_written(&path)?;
            set_replacement_file_time(&path, previous_modified, SystemTime::now())?;
            if keep_file_time {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .and_then(|f| f.set_modified(modified.unwrap()))
                    .map_err(|e| format!("出力の更新日時を設定できません: {e}"))?;
            }
            timings.write = start.elapsed().as_secs_f64() * 1000.0;
            Ok(ItemResult::Written {
                path,
                kinds: encoded.thumbnail_notice,
                replaced: replace,
            })
        })();
        let outcome = match result {
            Ok(ItemResult::Skipped(path)) => {
                let _ = tx.send(ExportEvent::Skipped { label, path });
                "skipped"
            }
            Ok(ItemResult::Written {
                path,
                kinds,
                replaced,
            }) => {
                let _ = tx.send(ExportEvent::Completed(ExportSuccess {
                    label: label.clone(),
                    path: path.clone(),
                }));
                if !kinds.is_empty() {
                    log::warn!(
                        "batch export thumbnail metadata: {}: {:?}",
                        path.display(),
                        kinds
                    );
                    let _ = tx.send(ExportEvent::ThumbnailNotice { label, path, kinds });
                }
                if replaced { "replaced" } else { "written" }
            }
            Err(message) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(ExportEvent::Cancelled);
                    return;
                }
                log::error!("batch export {label}: {message}");
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
                "failed"
            }
        };
        crate::perf::event(
            "export_batch/item",
            "complete",
            None,
            index as u64,
            &[
                ("decode_ms", serde_json::json!(timings.decode)),
                ("compose_ms", serde_json::json!(timings.compose)),
                ("resize_ms", serde_json::json!(timings.resize)),
                ("watermark_ms", serde_json::json!(0)),
                ("encode_ms", serde_json::json!(timings.encode)),
                ("write_ms", serde_json::json!(timings.write)),
                ("size", serde_json::json!(timings.dimensions)),
                (
                    "format",
                    serde_json::json!(request.options.format.resolve(&item.source_ext).extension()),
                ),
                ("outcome", serde_json::json!(outcome)),
            ],
        );
    }
    let _ = tx.send(ExportEvent::AllDone);
}

fn source_metadata_format(ext: &str) -> Option<SrcFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "jfif" => Some(SrcFormat::Jpeg),
        "png" => Some(SrcFormat::Png),
        "webp" => Some(SrcFormat::Webp),
        _ => None,
    }
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("エクスポートをキャンセルしました".into())
    } else {
        Ok(())
    }
}

fn validate_dimensions(format: &SrcFormat, size: [usize; 2]) -> Result<(), String> {
    let max = match format {
        SrcFormat::Webp => 16383,
        SrcFormat::Jpeg => 65500,
        _ => usize::MAX,
    };
    if size.iter().any(|n| *n > max) {
        Err(format!(
            "この形式で書ける大きさを超えています ({}×{})",
            size[0], size[1]
        ))
    } else {
        Ok(())
    }
}

fn local_datetime(time: SystemTime) -> Result<(String, String), String> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx};
    let ticks = match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => 116_444_736_000_000_000i128 + d.as_nanos() as i128 / 100,
        Err(e) => 116_444_736_000_000_000i128 - e.duration().as_nanos() as i128 / 100,
    };
    let ticks = u64::try_from(ticks).map_err(|_| "元の更新日時を変換できません".to_string())?;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).map_err(|e| e.to_string())?;
        SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).map_err(|e| e.to_string())?;
    }
    Ok((
        format!("{:04}{:02}{:02}", local.wYear, local.wMonth, local.wDay),
        format!("{:02}{:02}{:02}", local.wHour, local.wMinute, local.wSecond),
    ))
}

fn set_replacement_file_time(
    path: &Path,
    previous: Option<SystemTime>,
    written: SystemTime,
) -> Result<(), String> {
    let Some(previous) = previous else {
        return Ok(());
    };
    // Cache stamps use whole seconds. Advance to the next boundary, rather
    // than adding a second to the old fractional timestamp. Handle pre-epoch
    // timestamps with the same floor operation.
    let fractional_nanos = match previous.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.subsec_nanos(),
        Err(error) => (1_000_000_000 - error.duration().subsec_nanos()) % 1_000_000_000,
    };
    let next_second = previous
        .checked_add(std::time::Duration::from_nanos(u64::from(
            1_000_000_000 - fractional_nanos,
        )))
        .ok_or("出力の更新日時を計算できません")?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(written.max(next_second)))
        .map_err(|e| format!("出力の更新日時を設定できません: {e}"))
}

fn write_output(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    use std::io::Write;
    if replace {
        let parent = path.parent().ok_or("保存先フォルダがありません")?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        // tempfile's Windows persist calls SetFileAttributesW / MoveFileExW
        // directly. Canonicalize the parent so both native paths retain the
        // verbatim prefix, including when the destination does not exist yet.
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        let destination = parent.join(path.file_name().ok_or("保存先のファイル名がありません")?);
        let mut tmp = tempfile::Builder::new()
            .prefix(".miv-tmp-")
            .tempfile_in(&parent)
            .map_err(|e| e.to_string())?;
        tmp.write_all(bytes)
            .and_then(|_| tmp.flush())
            .map_err(|e| e.to_string())?;
        tmp.persist(&destination).map_err(|e| e.to_string())?;
    } else {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
