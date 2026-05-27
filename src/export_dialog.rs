//! Ctrl+E export dialog support.
//!
//! The UI side snapshots base pixels, mask, and selected presets. This module
//! composes conceal effects, encodes images, and writes files on a worker so
//! heavy CPU/I/O work never blocks egui.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use eframe::egui;

use crate::conceal::{ConcealPreset, ExportFallbackFormat};
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
    pub conceal_preset: Option<ConcealPreset>,
}

pub struct ExportRequest {
    pub source: ExportSource,
    pub original_format: SrcFormat,
    pub output_format: ExportFormat,
    pub output_dir: PathBuf,
    pub basename: String,
    pub base_pixels: Arc<egui::ColorImage>,
    pub conceal_mask: Option<Arc<Vec<bool>>>,
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
    pub has_conceal_mask: bool,
    /// ダイアログを開いた瞬間の base pixels と composite mask をスナップショット。
    /// 保存ボタンを押すまでに animation frame が進行したり AI upscale が完了したり
    /// しても、Ctrl+E を押した瞬間の image が export されるようにする
    /// (Codex review CONFIRMED)。
    pub base_pixels: Arc<egui::ColorImage>,
    pub conceal_mask: Option<Arc<Vec<bool>>>,
    /// 元の永続化済み batch selection。state.selection の force-clear 後でも、
    /// settings 保存時にこの「ユーザーが本当に意図した値」を温存する
    /// (Codex review CONFIRMED)。
    pub original_selection: [bool; 5],
    /// 元の永続化済み include_metadata。format 切替で UI が一時的に false に倒した
    /// 場合に、原状を保つために保持する (Codex review CONFIRMED)。
    pub original_include_metadata: bool,
    /// ダイアログを開いた瞬間にフォーカスを 1 度だけ basename へ寄せるためのラッチ。
    /// 毎フレーム request_focus すると他フィールドへフォーカスが移れない
    /// (Codex review CONFIRMED)。
    pub initial_focus_done: bool,
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
    // 元 WebP がアニメーション WebP かを検査するため、original が WebP のときは
    // 出力形式に関係なく source bytes を読み込む。出力 PNG/JPEG にしても黙って
    // 単一フレームを書き出してしまうのを防ぐ (Codex review CONFIRMED)。
    let needs_source_for_webp_check = request.original_format == SrcFormat::Webp;
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
        ExportSource::File { path } if needs_source_for_webp_check => {
            // File source は通常 source_path 経由で渡すが、アニメーション WebP の
            // 検出だけは bytes が要るのでここで読む。
            match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(_) => None,
            }
        }
        _ => None,
    };
    // 元 WebP がアニメーションなら、出力形式に関係なく全エントリを失敗にする。
    if request.original_format == SrcFormat::Webp
        && let Some(bytes) = source_bytes.as_deref()
        && crate::save_with_metadata::webp_is_animated(bytes)
    {
        let msg = "アニメーション WebP は対象外です".to_string();
        for entry in &request.entries {
            let _ = tx.send(ExportEvent::Failed(ExportFailure {
                label: entry.label.clone(),
                message: msg.clone(),
            }));
        }
        let _ = tx.send(ExportEvent::AllDone);
        return;
    }
    let source_path = match &request.source {
        ExportSource::File { path } if needs_source_bytes && source_bytes.is_none() => {
            Some(path.as_path())
        }
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
        let composed = match (&request.conceal_mask, &entry.conceal_preset) {
            (Some(mask), Some(preset)) => Some(compose_conceal_for_export(
                request.base_pixels.as_ref(),
                mask.as_ref(),
                preset,
            )),
            _ => None,
        };
        // 合成は CPU 重 (Mosaic/Blur で 4K だと数秒) なので、合成後 / encode 前にも
        // cancel を再チェックする。これでキャンセルが「encode 中の 1 ファイルだけは
        // 書き出されるが残りは抑止」ではなく、合成完了時点で確実に止まる
        // (Codex review CONFIRMED)。
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let pixels = composed
            .as_ref()
            .unwrap_or_else(|| request.base_pixels.as_ref());
        match save_image_with_metadata(
            pixels,
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

fn compose_conceal_for_export(
    base: &egui::ColorImage,
    mask: &[bool],
    params: &ConcealPreset,
) -> egui::ColorImage {
    match params.conceal_type {
        crate::conceal::ConcealType::Mosaic => {
            let long_edge = base.size[0].max(base.size[1]) as u32;
            let tile = crate::conceal::compute_tile_size(long_edge, params.mosaic_tile_mode);
            crate::conceal_compose::compose_mosaic(base, mask, tile, params.mosaic_boundary)
        }
        crate::conceal::ConcealType::WhiteFill => crate::conceal_compose::compose_solid_fill(
            base,
            mask,
            egui::Color32::WHITE,
            params.fill_opacity_percent,
            params.fill_edge,
        ),
        crate::conceal::ConcealType::BlackFill => crate::conceal_compose::compose_solid_fill(
            base,
            mask,
            egui::Color32::BLACK,
            params.fill_opacity_percent,
            params.fill_edge,
        ),
        crate::conceal::ConcealType::Blur => crate::conceal_compose::compose_blur(
            base,
            mask,
            params.blur_radius_px,
            params.blur_mode,
            params.blur_feather,
        ),
    }
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
            base_pixels: Arc::clone(&pixels),
            conceal_mask: None,
            entries: vec![
                ExportEntry {
                    label: "current".to_string(),
                    suffix: 0,
                    conceal_preset: None,
                },
                ExportEntry {
                    label: "preset1".to_string(),
                    suffix: 1,
                    conceal_preset: None,
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

    #[test]
    fn worker_composes_conceal_preset_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let pixels = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]));
        let mask = Arc::new(vec![true, false, false, false]);
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::PdfPage,
            original_format: SrcFormat::Other("pdf".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "masked".to_string(),
            base_pixels: pixels,
            conceal_mask: Some(mask),
            entries: vec![ExportEntry {
                label: "black".to_string(),
                suffix: 0,
                conceal_preset: Some(ConcealPreset {
                    conceal_type: crate::conceal::ConcealType::BlackFill,
                    ..Default::default()
                }),
            }],
            include_metadata: false,
        })
        .unwrap();

        loop {
            match pending
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            {
                ExportEvent::Failed(err) => panic!("unexpected export failure: {err:?}"),
                ExportEvent::AllDone => break,
                ExportEvent::Started { .. } | ExportEvent::Completed(_) => {}
                ExportEvent::Cancelled => panic!("unexpected cancel"),
            }
        }

        let out = image::open(temp.path().join("masked_0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [255, 255, 255, 255]);
    }
}
