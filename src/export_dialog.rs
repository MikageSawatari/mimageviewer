//! Ctrl+E export dialog support.
//!
//! The UI side prepares already-composited `ColorImage` entries on the main
//! thread, then this module writes them on a worker so metadata I/O and image
//! encoding never block egui.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use eframe::egui;

use crate::conceal::ExportFallbackFormat;
use crate::save_with_metadata::{SaveError, SaveOptions, SrcFormat, save_image_with_metadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg95,
    Png,
    Webp,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg95 => "JPEG 95",
            Self::Png => "PNG",
            Self::Webp => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg95 => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    pub fn to_src_format(self) -> SrcFormat {
        match self {
            Self::Jpeg95 => SrcFormat::Jpeg,
            Self::Png => SrcFormat::Png,
            Self::Webp => SrcFormat::Webp,
        }
    }

    pub fn from_source(src_format: &SrcFormat, fallback: ExportFallbackFormat) -> Self {
        match src_format {
            SrcFormat::Jpeg => Self::Jpeg95,
            SrcFormat::Png => Self::Png,
            SrcFormat::Webp => Self::Webp,
            SrcFormat::Other(_) => match fallback {
                ExportFallbackFormat::Jpeg95 => Self::Jpeg95,
                ExportFallbackFormat::Png => Self::Png,
            },
        }
    }

    pub fn fallback_format(self) -> Option<ExportFallbackFormat> {
        match self {
            Self::Jpeg95 => Some(ExportFallbackFormat::Jpeg95),
            Self::Png => Some(ExportFallbackFormat::Png),
            Self::Webp => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ExportSource {
    File {
        path: PathBuf,
    },
    ZipEntry {
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage,
}

#[derive(Clone)]
pub struct ExportEntry {
    pub label: String,
    pub suffix: u8,
    pub pixels: Arc<egui::ColorImage>,
}

pub struct ExportRequest {
    pub source: ExportSource,
    pub original_format: SrcFormat,
    pub output_format: ExportFormat,
    pub output_dir: PathBuf,
    pub basename: String,
    pub entries: Vec<ExportEntry>,
    pub include_metadata: bool,
}

#[derive(Clone)]
pub struct ExportDialogState {
    pub source: ExportSource,
    pub source_label: String,
    pub original_format: SrcFormat,
    pub output_format: ExportFormat,
    pub basename: String,
    pub output_dir_text: String,
    pub include_metadata: bool,
    pub selection: [bool; 5],
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ExportSuccess {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExportFailure {
    pub label: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum ExportEvent {
    Started { label: String },
    Completed(ExportSuccess),
    Failed(ExportFailure),
    Cancelled,
    AllDone,
}

pub struct ExportPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ExportEvent>,
    pub total: usize,
    pub done: usize,
    pub last_message: String,
    pub successes: Vec<ExportSuccess>,
    pub errors: Vec<ExportFailure>,
    pub finished: bool,
    pub cancel_requested: bool,
}

pub fn spawn_export_worker(request: ExportRequest) -> Result<ExportPending, String> {
    let total = request.entries.len();
    if total == 0 {
        return Err("エクスポートする項目がありません".to_string());
    }
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("ctrl-e-export".into())
        .spawn(move || run_export(request, worker_cancel, tx))
        .map_err(|e| format!("エクスポート worker を開始できません: {e}"))?;
    Ok(ExportPending {
        cancel,
        rx,
        total,
        done: 0,
        last_message: "準備中".to_string(),
        successes: Vec::new(),
        errors: Vec::new(),
        finished: false,
        cancel_requested: false,
    })
}

pub fn resolve_session_basename(
    output_dir: &Path,
    requested_basename: &str,
    extension: &str,
    suffixes: &[u8],
) -> Result<String, String> {
    let base = crate::capture::basename_from_text(requested_basename);
    if suffixes.is_empty() {
        return Err("エクスポートする項目がありません".to_string());
    }
    if session_targets_available(output_dir, &base, extension, suffixes) {
        return Ok(base);
    }
    for seq in 1..=9999 {
        let candidate = format!("{base}_{seq:04}");
        if session_targets_available(output_dir, &candidate, extension, suffixes) {
            return Ok(candidate);
        }
    }
    Err(format!(
        "同名ファイルが多すぎます: {}",
        output_dir.display()
    ))
}

pub fn target_path(output_dir: &Path, basename: &str, suffix: u8, extension: &str) -> PathBuf {
    output_dir.join(format!("{basename}_{suffix}.{extension}"))
}

fn session_targets_available(
    output_dir: &Path,
    basename: &str,
    extension: &str,
    suffixes: &[u8],
) -> bool {
    suffixes
        .iter()
        .all(|suffix| !target_path(output_dir, basename, *suffix, extension).exists())
}

fn run_export(request: ExportRequest, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ExportEvent>) {
    let output_src_format = request.output_format.to_src_format();
    let needs_source_for_webp_check =
        request.original_format == SrcFormat::Webp && output_src_format == SrcFormat::Webp;
    let needs_source_bytes = request.include_metadata || needs_source_for_webp_check;
    let source_bytes = match &request.source {
        ExportSource::ZipEntry {
            zip_path,
            entry_name,
        } if needs_source_bytes => {
            match crate::zip_loader::read_entry_bytes(zip_path, entry_name) {
                Ok(bytes) => Some(bytes),
                Err(err) => {
                    let msg = format!("ZIP エントリを読めません: {err}");
                    for entry in &request.entries {
                        let _ = tx.send(ExportEvent::Failed(ExportFailure {
                            label: entry.label.clone(),
                            message: msg.clone(),
                        }));
                    }
                    let _ = tx.send(ExportEvent::AllDone);
                    return;
                }
            }
        }
        _ => None,
    };
    let source_path = match &request.source {
        ExportSource::File { path } if needs_source_bytes => Some(path.as_path()),
        _ => None,
    };
    let include_metadata = request.include_metadata
        && request.original_format.supports_metadata_writeback()
        && request.original_format == output_src_format
        && (source_path.is_some() || source_bytes.is_some());
    let options = SaveOptions {
        jpeg_quality: 95,
        include_metadata,
        caller_applied_orientation: true,
        ..Default::default()
    };
    let extension = request.output_format.extension();

    for entry in request.entries {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let _ = tx.send(ExportEvent::Started {
            label: entry.label.clone(),
        });
        let path = target_path(
            &request.output_dir,
            &request.basename,
            entry.suffix,
            extension,
        );
        match save_image_with_metadata(
            entry.pixels.as_ref(),
            source_path,
            source_bytes.as_deref(),
            &path,
            output_src_format.clone(),
            &options,
        ) {
            Ok(()) => {
                let _ = tx.send(ExportEvent::Completed(ExportSuccess {
                    label: entry.label,
                    path,
                }));
            }
            Err(SaveError::IoError(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: entry.label,
                    message: format!("同名ファイルが既にあります: {}", path.display()),
                }));
            }
            Err(err) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: entry.label,
                    message: err.to_string(),
                }));
            }
        }
    }
    let _ = tx.send(ExportEvent::AllDone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_basename_uses_plain_base_when_free() {
        let temp = tempfile::tempdir().unwrap();
        let got = resolve_session_basename(temp.path(), "sample_edited", "jpg", &[0, 1]).unwrap();
        assert_eq!(got, "sample_edited");
    }

    #[test]
    fn session_basename_inserts_session_number_when_any_suffix_collides() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("sample_edited_1.jpg"), b"x").unwrap();
        let got = resolve_session_basename(temp.path(), "sample_edited", "jpg", &[0, 1]).unwrap();
        assert_eq!(got, "sample_edited_0001");
    }

    #[test]
    fn worker_writes_selected_entries_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let pixels = Arc::new(egui::ColorImage::new(
            [2, 2],
            vec![egui::Color32::from_rgb(32, 64, 96); 4],
        ));
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::PdfPage,
            original_format: SrcFormat::Other("pdf".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "out".to_string(),
            entries: vec![
                ExportEntry {
                    label: "current".to_string(),
                    suffix: 0,
                    pixels: Arc::clone(&pixels),
                },
                ExportEntry {
                    label: "preset1".to_string(),
                    suffix: 1,
                    pixels,
                },
            ],
            include_metadata: false,
        })
        .unwrap();

        let mut completed = 0;
        loop {
            match pending
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            {
                ExportEvent::Completed(_) => completed += 1,
                ExportEvent::Failed(err) => panic!("unexpected export failure: {err:?}"),
                ExportEvent::AllDone => break,
                ExportEvent::Started { .. } => {}
                ExportEvent::Cancelled => panic!("unexpected cancel"),
            }
        }
        assert_eq!(completed, 2);
        assert!(temp.path().join("out_0.png").exists());
        assert!(temp.path().join("out_1.png").exists());
    }
}
