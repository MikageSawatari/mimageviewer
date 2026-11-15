//! Batch evaluation of ordinary logical books, without ground truth.
use super::*;
use crate::app::book_cover::{BookCover, BookCoverEnv, BookKind, resolve_book_cover};
use crate::thumb_loader::{AutoCoverInput, AutoCoverSkip, ReadLocation};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Options {
    pub binding: ReadingDirection,
    pub thumbs: bool,
    pub rotation_db: Option<PathBuf>,
    pub saved_rotation: Option<Rotation>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            binding: ReadingDirection::Rtl,
            thumbs: false,
            rotation_db: None,
            saved_rotation: None,
        }
    }
}

impl Options {
    pub(super) fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            binding: parse_binding(std::env::var("MIV_COVER_AUTO_EVAL_BINDING").ok().as_deref())?,
            thumbs: std::env::var("MIV_COVER_AUTO_EVAL_THUMBS").as_deref() == Ok("1"),
            // Only the explicitly invoked list tool reads this existing store,
            // read-only. Caches stay in the disposable TestDataDirGuard below.
            // Tests pass a temporary DB explicitly and never call from_env.
            rotation_db: Some(
                std::env::var_os("MIV_COVER_AUTO_EVAL_ROTATION_DB")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| crate::data_dir::flavor_default().join("rotation.db")),
            ),
            saved_rotation: env_rotation("MIV_COVER_AUTO_EVAL_SAVED_ROTATION")?,
        })
    }
}

fn parse_binding(value: Option<&str>) -> Result<ReadingDirection, &'static str> {
    match value {
        None | Some("rtl") => Ok(ReadingDirection::Rtl),
        Some("ltr") => Ok(ReadingDirection::Ltr),
        _ => Err("MIV_COVER_AUTO_EVAL_BINDING must be rtl or ltr"),
    }
}

fn saved_rotation(options: &Options, key: Option<&str>) -> Result<Rotation, String> {
    if let Some(angle) = options.saved_rotation {
        return Ok(angle);
    }
    let Some(path) = options.rotation_db.as_ref() else {
        return Ok(Rotation::None);
    };
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Rotation::None),
        Err(error) => return Err(format!("rotation DB metadata: {error}")),
        Ok(_) => (),
    }
    let db = crate::rotation_db::RotationDb::open_readonly_at(path)
        .map_err(|error| format!("rotation DB read: {error}"))?;
    crate::edit_source::read_caption_rotation(Some(&db), key)
        .map_err(|error| format!("rotation DB lookup: {error}"))
}

struct Row {
    json: serde_json::Value,
    outcome: Option<AngleOutcome>,
    native: Option<(u32, u32)>,
    aspect: Option<f64>,
    rotation: Rotation,
}

fn skip_reason(skip: AutoCoverSkip) -> &'static str {
    match skip {
        AutoCoverSkip::Manual { .. } => "manual",
        AutoCoverSkip::Off => "off",
        AutoCoverSkip::PinnedFolder => "pinned-folder",
        AutoCoverSkip::ChildRepresentative => "child-representative",
        AutoCoverSkip::FolderDisabled => "folder-disabled",
        AutoCoverSkip::DirectRar => "direct-rar",
        AutoCoverSkip::NestedZip => "nested-zip",
        AutoCoverSkip::Unsupported => "unsupported",
    }
}

fn kind(path: &Path, folders_only: bool) -> Result<(BookKind, &'static str), String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("open: {error}"))?;
    if metadata.is_dir() {
        return Ok((BookKind::Folder, "folder"));
    }
    if folders_only {
        return Err("folder-list entry is not a directory".into());
    }
    match BookKind::for_path(path) {
        BookKind::Zip => Ok((BookKind::Zip, "zip")),
        BookKind::Pdf => Ok((BookKind::Pdf, "pdf")),
        _ => Err("book-list entry must be a PDF, ZIP or image folder".into()),
    }
}

fn evaluate_one(
    path: &Path,
    sequence: usize,
    folders_only: bool,
    output: &Path,
    options: &Options,
    config: FallbackConfig,
) -> Result<Row, String> {
    let (kind, label) = kind(path, folders_only)?;
    let env = BookCoverEnv {
        // Avoid Shell/default-root lookup. These inputs are ordinary image
        // folders, not compiled direct children of an implicit books root.
        books_root: Some(path.to_path_buf()),
        auto_cover_crop: Some(AutoCropIdentity {
            binding: options.binding,
            folders: true,
        }),
        ..Default::default()
    };
    let cover = match resolve_book_cover(path, kind, &env) {
        BookCover::Readable(cover) => cover,
        BookCover::Unreadable(reason) => return Err(format!("cover resolution: {reason:?}")),
        BookCover::VideoFrame { .. } => return Err("cover is a video frame".into()),
    };
    let saved = saved_rotation(options, cover.rotation_key.as_deref())?;
    let analysis = match &cover.location {
        ReadLocation::File(path) => crate::thumb_loader::decode_cover_auto_analysis_image(
            crate::image_decode::Source::Path(path),
            None,
            None,
        ),
        ReadLocation::ArchiveEntry { entry, .. } => {
            let bytes = cover
                .preloaded_bytes
                .as_deref()
                .ok_or("cover bytes missing")?;
            crate::thumb_loader::decode_cover_auto_analysis_image(
                crate::image_decode::Source::Bytes { bytes, hint: entry },
                None,
                None,
            )
        }
        ReadLocation::PdfPage { pdf, page } => crate::thumb_loader::decode_cover_auto_analysis_pdf(
            pdf,
            cover.epub_read.as_ref(),
            *page,
            cover.pdf_password.as_deref(),
            None,
            crate::pdf_loader::JobPriority::Normal,
            crate::pdf_loader::PdfRenderValidity::Unscoped,
            crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
        ),
        _ => return Err("cover has no readable raster".into()),
    }
    .map_err(|error| format!("cover decode: {error}"))?;
    let sample = match (&cover.location, &cover.auto_cover_crop) {
        (ReadLocation::File(leaf), Ok(auto)) => match &auto.input {
            AutoCoverInput::FolderPages(pages) => {
                crate::cover_auto_sample::sample_folder_pages(pages, leaf)
            }
            _ => Err(crate::cover_auto_sample::PageAspectDecline::TooFew),
        },
        (ReadLocation::ArchiveEntry { archive, entry }, Ok(_)) => {
            crate::cover_auto_sample::sample_zip_pages(
                archive,
                entry,
                cover.zip_dir_prefix.as_deref(),
            )
        }
        (ReadLocation::PdfPage { pdf, page }, Ok(_)) => {
            let read = cover.epub_read.as_ref().ok_or("PDF read lease missing")?;
            let sizes = crate::pdf_loader::get_page_sizes_with_read_target(
                pdf,
                read,
                cover.pdf_password.as_deref(),
            )
            .map_err(|error| format!("page sizes: {error}"))?;
            crate::cover_auto_sample::page_aspect_from_sizes_report(&sizes, *page as usize)
        }
        _ => Err(crate::cover_auto_sample::PageAspectDecline::TooFew),
    };
    let outcomes = cover.auto_cover_crop.as_ref().ok().map(|_| {
        evaluate_angles(
            &analysis.image,
            analysis.native_dims,
            sample.ok(),
            analysis.shot_exif,
            options.binding,
            config,
        )
    });
    let selected = outcomes.map(|angles| {
        angles[EVAL_ROTATIONS
            .iter()
            .position(|&angle| angle == saved)
            .unwrap()]
    });
    let current = selected
        .and_then(|outcome| outcome.original.ok())
        .map(|success| success.crop);
    let fallback = selected
        .filter(|outcome| outcome.fallback.is_some())
        .and_then(|outcome| outcome.final_result.ok());
    let diagnostic = selected
        .and_then(|outcome| outcome.fallback)
        .map(|value| value.json(config));
    let reason = match selected {
        Some(outcome) => outcome
            .final_result
            .map(|_| "cropped".to_owned())
            .unwrap_or_else(|reason| reason.as_str().into_owned()),
        None => skip_reason(*cover.auto_cover_crop.as_ref().unwrap_err()).to_owned(),
    };
    let filename = if options.thumbs {
        Some(thumbs::write(
            output,
            sequence,
            &analysis.image,
            saved,
            current,
            fallback,
        )?)
    } else {
        None
    };
    let ratio = layout_ratio(analysis.native_dims, sample.ok(), saved);
    let mut row = json!({
        "sequence": sequence, "index": sequence - 1, "kind": label,
        "reason": reason, "rotation": saved.degrees(),
        "R": ratio, "layout_ratio": ratio, "page_aspect": sample.ok(),
        "page_aspect_decline": sample.err().map(crate::cover_auto_sample::PageAspectDecline::as_str),
        "native_dims": analysis.native_dims,
        "decoded_dims": [analysis.image.width(), analysis.image.height()],
        "current_crop": current.map(|crop| thumbs::visible_crop(crop, saved)),
        "fallback_crop": fallback.map(|crop| thumbs::visible_crop(crop, saved)),
        "fallback_type": diagnostic.as_ref().and_then(|value|
            value.pointer("/layout/kind").or_else(|| value.get("mode"))),
        "image_filename": filename,
        "original_decline": selected.and_then(|outcome| outcome.original.err()).map(|reason| reason.as_str().into_owned()),
        "production_method": selected.and_then(|outcome| outcome.original.ok()).map(|success| success.method.as_str()),
        "production_layout": selected.and_then(|outcome| outcome.original.ok()).and_then(|success| success.layout()).map(AutoCropLayout::as_str),
        "iou": null,
    });
    if folders_only {
        // Keep the existing numeric-only folder report's source-coordinate
        // diagnostics. The new display-facing crops above are always 0..1.
        row["crop"] = json!(
            selected
                .and_then(|outcome| outcome.final_result.ok())
                .map(CoverCrop::values)
        );
        row["fallback"] = json!(diagnostic);
        row["all_crops"] = json!(outcomes.map(|angles| {
            angles.map(|outcome| outcome.final_result.ok().map(CoverCrop::values))
        }));
        row["all_fallbacks"] = json!(outcomes.map(|angles| {
            angles.map(|outcome| outcome.fallback.map(|diagnostic| diagnostic.json(config)))
        }));
    }
    if !folders_only || options.thumbs {
        row["path"] = json!(path);
    }
    Ok(Row {
        json: row,
        outcome: selected,
        native: Some(analysis.native_dims),
        aspect: sample.ok(),
        rotation: saved,
    })
}

pub(super) fn evaluate(
    list: &Path,
    output: &Path,
    folders_only: bool,
    config: FallbackConfig,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(list)?;
    // Windows PowerShell 5.1's Set-Content -Encoding UTF8 emits this BOM.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let paths: Vec<PathBuf> = serde_json::from_slice(bytes)?;
    if paths.is_empty() || paths.iter().any(|path| !path.is_absolute()) {
        return Err("evaluation list must contain nonempty absolute paths".into());
    }
    let output = target_output(output.to_path_buf())?;
    let _worker_data = crate::data_dir::TestDataDirGuard::new();
    // Use the same PDF IPC operations in-process: the lib-test executable
    // cannot act as an application's child worker. Hold one registration per
    // unique path for the entire batch, including duplicate input books.
    let _pdf_backends: Vec<_> = paths
        .iter()
        .filter(|path| BookKind::for_path(path) == BookKind::Pdf)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|path| crate::pdf_loader::RemotePdfTestBackend::for_path(path))
        .collect();
    let cursor = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel();
    // Match App's heavy-I/O thumbnail lane cap (1 on <=4 CPUs, otherwise 2).
    let lanes = if std::thread::available_parallelism().map_or(1, usize::from) <= 4 {
        1
    } else {
        2
    };
    std::thread::scope(|scope| {
        for _ in 0..lanes.min(paths.len()) {
            let (tx, cursor, completed, paths, output) =
                (tx.clone(), &cursor, &completed, &paths, &output);
            scope.spawn(move || {
                loop {
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result =
                        evaluate_one(path, index + 1, folders_only, output, options, config);
                    let row = result.unwrap_or_else(|error| {
                        // Preserve FOLDER_LIST's numeric-only privacy contract
                        // unless the operator explicitly opted into thumbnails.
                        let error = if folders_only && !options.thumbs {
                            error
                                .split(':')
                                .next()
                                .unwrap_or("evaluation failed")
                                .to_owned()
                        } else {
                            error
                        };
                        let kind = if folders_only {
                            "folder"
                        } else {
                            match BookKind::for_path(path) {
                                BookKind::Pdf => "pdf",
                                BookKind::Zip => "zip",
                                _ => "folder",
                            }
                        };
                        let mut json = json!({"sequence": index+1, "index": index, "kind": kind,
                        "reason": "error", "error": error, "rotation": null, "R": null,
                        "current_crop": null, "fallback_crop": null, "fallback_type": null,
                        "image_filename": null, "iou": null});
                        if !folders_only || options.thumbs {
                            json["path"] = json!(path);
                        }
                        Row {
                            json,
                            outcome: None,
                            native: None,
                            aspect: None,
                            rotation: Rotation::None,
                        }
                    });
                    if tx.send((index, row)).is_err() {
                        break;
                    }
                    let count = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if count.is_multiple_of(100) {
                        println!("cover auto list evaluation: {count}/{}", paths.len());
                    }
                }
            });
        }
    });
    drop(tx);
    let mut ordered: Vec<_> = rx.into_iter().collect();
    ordered.sort_unstable_by_key(|(index, _)| *index);
    let (mut cropped, mut declined, mut failed) = (0, 0, 0);
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut stats = FallbackStats::default();
    let mut ratios = RatioDistribution::default();
    let mut rows = Vec::new();
    for (_, row) in ordered {
        let reason = row.json["reason"].as_str().unwrap();
        match reason {
            "cropped" => cropped += 1,
            "error" => failed += 1,
            _ => {
                declined += 1;
                *reasons.entry(reason.to_owned()).or_default() += 1;
            }
        }
        if let Some(outcome) = row.outcome {
            stats.record(outcome, None, false);
        }
        if let Some(native) = row.native {
            ratios.record(native, row.aspect, row.rotation);
        }
        rows.push(row.json);
    }
    let summary = json!({"algorithm": "autocrop:v3f", "source": if folders_only {"folder-list"} else {"book-list"},
        "binding": if options.binding == ReadingDirection::Rtl { "rtl" } else { "ltr" },
        "total": paths.len(), "cropped": cropped, "declined": declined, "failed": failed,
        "decline_reasons": reasons, "fallback_mode": config.name(), "fallback_richer": config.includes_richer(),
        "fallback_cropped": stats.cropped, "fallback_good": null, "fallback_wrong": null,
        "fallback_original_declines": stats.reasons,
        "fallback_inward_fraction_distribution": stats.distribution(),
        "fallback_by_layout": stats.layout_summary(false),
        "fallback_layout_ratio_distribution": stats.layout_ratio_distribution(),
        "ratio_distribution": ratios.json(), "rows": rows});
    std::fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!(
        "cover auto list evaluation: total={} cropped={cropped} declined={declined} failed={failed}",
        paths.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests;
