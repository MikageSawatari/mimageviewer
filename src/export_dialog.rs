//! Ctrl+E export dialog support.
//!
//! The UI side snapshots base pixels, mask, and selected presets. This module
//! composes conceal effects, encodes images, and writes files on a worker so
//! heavy CPU/I/O work never blocks egui.

use std::borrow::Cow;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ExportScale {
    #[default]
    Full,
    Half,
    Quarter,
    /// 長辺を指定 px 以下に縮小する (アップスケールはしない)。
    LongEdge(u32),
}

impl ExportScale {
    /// 固定倍率の選択肢 (ダイアログのラジオで列挙する)。長辺 px 指定は別 UI で扱う。
    pub const FIXED: [Self; 3] = [Self::Full, Self::Half, Self::Quarter];
    /// 長辺 px 指定モードの既定値・範囲。
    pub const DEFAULT_LONG_EDGE: u32 = 2048;
    pub const LONG_EDGE_MIN: u32 = 256;
    pub const LONG_EDGE_MAX: u32 = 16384;

    pub fn label(self) -> String {
        match self {
            Self::Full => "そのまま".to_string(),
            Self::Half => "1/2 サイズ".to_string(),
            Self::Quarter => "1/4 サイズ".to_string(),
            Self::LongEdge(px) => format!("長辺 {px}px 以下"),
        }
    }

    /// crop / 合成済みサイズを入力に、出力ピクセルサイズを返す。
    /// `LongEdge(n)` は長辺が n を超えるときだけ縮小し、アップスケールはしない。
    pub fn scaled_size(self, size: [usize; 2]) -> [usize; 2] {
        let w = size[0].max(1);
        let h = size[1].max(1);
        match self {
            Self::Full => [w, h],
            Self::Half => Self::scaled_by_factor(w, h, 0.5),
            Self::Quarter => Self::scaled_by_factor(w, h, 0.25),
            Self::LongEdge(px) => {
                let target = px.max(1) as usize;
                let long = w.max(h);
                if long <= target {
                    [w, h]
                } else {
                    Self::scaled_by_factor(w, h, target as f32 / long as f32)
                }
            }
        }
    }

    fn scaled_by_factor(w: usize, h: usize, factor: f32) -> [usize; 2] {
        [
            ((w as f32 * factor).round() as usize).max(1),
            ((h as f32 * factor).round() as usize).max(1),
        ]
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
    RenderedSpread,
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
    pub pixels: ExportPixels,
    pub scale: ExportScale,
    pub entries: Vec<ExportEntry>,
    pub include_metadata: bool,
}

#[derive(Clone)]
pub struct ExportPagePixels {
    pub base_pixels: Arc<egui::ColorImage>,
    pub conceal_mask: Option<Arc<Vec<bool>>>,
    pub crop: Option<crate::export_crop::CropRect>,
}

impl ExportPagePixels {
    pub fn render_size(&self) -> [usize; 2] {
        let [w, h] = self.base_pixels.size;
        if let Some(crop) = self.crop {
            let (_, _, crop_w, crop_h) = crop.pixel_bounds(w, h);
            [crop_w, crop_h]
        } else {
            [w.max(1), h.max(1)]
        }
    }
}

#[derive(Clone)]
pub enum ExportPixels {
    Single(ExportPagePixels),
    Spread {
        left: ExportPagePixels,
        right: ExportPagePixels,
    },
}

impl ExportPixels {
    pub fn has_conceal_mask(&self) -> bool {
        match self {
            Self::Single(page) => page.conceal_mask.is_some(),
            Self::Spread { left, right } => {
                left.conceal_mask.is_some() || right.conceal_mask.is_some()
            }
        }
    }

    pub fn render_size(&self) -> [usize; 2] {
        match self {
            Self::Single(page) => page.render_size(),
            Self::Spread { left, right } => {
                let [left_w, left_h] = left.render_size();
                let [right_w, right_h] = right.render_size();
                [left_w + right_w, left_h.max(right_h)]
            }
        }
    }
}

#[derive(Clone)]
pub struct ExportDialogState {
    pub source: ExportSource,
    pub source_label: String,
    pub original_format: SrcFormat,
    pub output_format: ExportFormat,
    pub scale: ExportScale,
    pub basename: String,
    pub output_dir_text: String,
    pub source_dir: PathBuf,
    pub include_metadata: bool,
    pub selection: [bool; 5],
    pub has_conceal_mask: bool,
    /// ダイアログを開いた瞬間の base pixels と composite mask をスナップショット。
    /// 保存ボタンを押すまでに animation frame が進行したり AI upscale が完了したり
    /// しても、Ctrl+E を押した瞬間の image が export されるようにする
    /// (Codex review CONFIRMED)。
    pub pixels: ExportPixels,
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

impl ExportDialogState {
    pub fn reset_output_dir_to_source_dir(&mut self) {
        self.output_dir_text = self.source_dir.display().to_string();
    }
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
            // read 失敗時に silent skip すると、出力 PNG/JPEG では `save_with_metadata`
            // 側の animation check も走らずアニメ WebP が単一フレームで書き出されて
            // しまう (Codex review P3)。ZIP 側と同じく全エントリ失敗にする。
            match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(err) => {
                    let msg = format!("アニメーション判定のため WebP を読めません: {err}");
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
    // 元 WebP がアニメーションなら、出力形式に関係なく全エントリを失敗にする。
    // ここに到達した時点で WebP 入力なら source_bytes は必ず Some であることが
    // 保証されている (read 失敗は上の File/ZIP 経路で全失敗 + return 済み)。
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
        // Ctrl+E の pixels はフルスクリーン表示用 base 由来で、通常ファイルも ZIP 内画像も
        // EXIF Orientation 適用済み。メタデータ転記時は Orientation を 1 に正規化して
        // 外部ビューアでの二重回転を避ける (v1.0.0 DI-2 follow-up)。
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
        let pixels = match render_export_pixels(&request.pixels, entry.conceal_preset.as_ref()) {
            Ok(pixels) => pixels,
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: entry.label,
                    message,
                }));
                continue;
            }
        };
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let pixels = match scale_export_pixels(pixels, request.scale) {
            Ok(pixels) => pixels,
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: entry.label,
                    message,
                }));
                continue;
            }
        };
        // 合成は CPU 重 (Mosaic/Blur で 4K だと数秒) なので、合成後 / encode 前にも
        // cancel を再チェックする。これでキャンセルが「encode 中の 1 ファイルだけは
        // 書き出されるが残りは抑止」ではなく、合成完了時点で確実に止まる
        // (Codex review CONFIRMED)。
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        match save_image_with_metadata(
            pixels.as_ref(),
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

fn render_export_pixels<'a>(
    pixels: &'a ExportPixels,
    preset: Option<&ConcealPreset>,
) -> Result<Cow<'a, egui::ColorImage>, String> {
    match pixels {
        ExportPixels::Single(page) => render_export_page_pixels(page, preset),
        ExportPixels::Spread { left, right } => {
            let left = render_export_page_pixels(left, preset)?;
            let right = render_export_page_pixels(right, preset)?;
            let combined =
                crate::capture::combine_spread_color_images(left.as_ref(), right.as_ref())?;
            Ok(Cow::Owned(combined))
        }
    }
}

fn render_export_page_pixels<'a>(
    page: &'a ExportPagePixels,
    preset: Option<&ConcealPreset>,
) -> Result<Cow<'a, egui::ColorImage>, String> {
    let rendered = match (&page.conceal_mask, preset) {
        (Some(mask), Some(preset)) => Cow::Owned(compose_conceal_for_export(
            page.base_pixels.as_ref(),
            mask.as_ref(),
            preset,
        )?),
        _ => Cow::Borrowed(page.base_pixels.as_ref()),
    };
    if let Some(crop) = page.crop {
        return Ok(Cow::Owned(crate::export_crop::crop_color_image(
            rendered.as_ref(),
            crop,
        )?));
    }
    Ok(rendered)
}

fn scale_export_pixels<'a>(
    pixels: Cow<'a, egui::ColorImage>,
    scale: ExportScale,
) -> Result<Cow<'a, egui::ColorImage>, String> {
    if scale == ExportScale::Full {
        return Ok(pixels);
    }
    let [w, h] = pixels.size;
    let [new_w, new_h] = scale.scaled_size([w, h]);
    if [new_w, new_h] == [w, h] {
        return Ok(pixels);
    }
    let src_w = u32::try_from(w).map_err(|_| "エクスポート画像の幅が大きすぎます".to_string())?;
    let src_h = u32::try_from(h).map_err(|_| "エクスポート画像の高さが大きすぎます".to_string())?;
    let dst_w =
        u32::try_from(new_w).map_err(|_| "エクスポート画像の幅が大きすぎます".to_string())?;
    let dst_h =
        u32::try_from(new_h).map_err(|_| "エクスポート画像の高さが大きすぎます".to_string())?;
    let rgba = crate::capture::color_image_to_rgba(pixels.as_ref());
    let src = image::RgbaImage::from_raw(src_w, src_h, rgba)
        .ok_or_else(|| "エクスポート画像の RGBA バッファが不正です".to_string())?;
    let resized = crate::fast_resize::resize_rgba8_exact(
        &src,
        dst_w,
        dst_h,
        crate::fast_resize::Quality::Lanczos3,
    );
    Ok(Cow::Owned(egui::ColorImage::from_rgba_unmultiplied(
        [new_w, new_h],
        resized.as_raw(),
    )))
}

fn compose_conceal_for_export(
    base: &egui::ColorImage,
    mask: &[bool],
    params: &ConcealPreset,
) -> Result<egui::ColorImage, String> {
    let expected = base.size[0]
        .checked_mul(base.size[1])
        .ok_or_else(|| "隠蔽加工マスクのサイズが大きすぎます".to_string())?;
    if mask.len() != expected {
        return Err(format!(
            "隠蔽加工マスクのサイズが一致しません: mask={}, expected={}",
            mask.len(),
            expected
        ));
    }
    Ok(crate::conceal_compose::compose_with_preset(
        base, mask, params,
    ))
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
            pixels: ExportPixels::Single(ExportPagePixels {
                base_pixels: Arc::clone(&pixels),
                conceal_mask: None,
                crop: None,
            }),
            scale: ExportScale::Full,
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
            pixels: ExportPixels::Single(ExportPagePixels {
                base_pixels: pixels,
                conceal_mask: Some(mask),
                crop: None,
            }),
            scale: ExportScale::Full,
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

    #[test]
    fn render_export_page_pixels_applies_crop_last() {
        let pixels = Arc::new(egui::ColorImage::new(
            [3, 1],
            vec![
                egui::Color32::from_rgb(255, 0, 0),
                egui::Color32::from_rgb(0, 255, 0),
                egui::Color32::from_rgb(0, 0, 255),
            ],
        ));
        let page = ExportPagePixels {
            base_pixels: pixels,
            conceal_mask: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 1.0,
            }),
        };

        let out = render_export_page_pixels(&page, None).unwrap();

        assert_eq!(out.size, [2, 1]);
        assert_eq!(out.pixels[0], egui::Color32::from_rgb(0, 255, 0));
        assert_eq!(out.pixels[1], egui::Color32::from_rgb(0, 0, 255));
    }

    #[test]
    fn export_scale_dimensions_use_rendered_crop_spread_size() {
        let left = ExportPagePixels {
            base_pixels: Arc::new(egui::ColorImage::new(
                [5, 4],
                vec![egui::Color32::BLACK; 20],
            )),
            conceal_mask: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 1.0,
                max_x: 5.0,
                max_y: 4.0,
            }),
        };
        let right = ExportPagePixels {
            base_pixels: Arc::new(egui::ColorImage::new(
                [3, 5],
                vec![egui::Color32::WHITE; 15],
            )),
            conceal_mask: None,
            crop: None,
        };
        let pixels = ExportPixels::Spread { left, right };

        assert_eq!(pixels.render_size(), [7, 5]);
        assert_eq!(ExportScale::Half.scaled_size(pixels.render_size()), [4, 3]);
        assert_eq!(
            ExportScale::Quarter.scaled_size(pixels.render_size()),
            [2, 1]
        );
    }

    #[test]
    fn export_scale_long_edge_downscales_only_when_larger() {
        // 長辺が target を超えるときは長辺=target に合わせて等比縮小。
        assert_eq!(
            ExportScale::LongEdge(1000).scaled_size([4000, 2000]),
            [1000, 500]
        );
        assert_eq!(
            ExportScale::LongEdge(1000).scaled_size([2000, 4000]),
            [500, 1000]
        );
        // 長辺が target 以下なら原寸のまま (アップスケールしない)。
        assert_eq!(
            ExportScale::LongEdge(4096).scaled_size([1920, 1080]),
            [1920, 1080]
        );
        // 長辺がちょうど target なら原寸。
        assert_eq!(
            ExportScale::LongEdge(2048).scaled_size([2048, 1024]),
            [2048, 1024]
        );
    }

    #[test]
    fn worker_exports_half_scale_after_spread_render() {
        let temp = tempfile::tempdir().unwrap();
        let left = Arc::new(egui::ColorImage::new(
            [4, 2],
            vec![egui::Color32::from_rgb(200, 0, 0); 8],
        ));
        let right = Arc::new(egui::ColorImage::new(
            [2, 2],
            vec![egui::Color32::from_rgb(0, 0, 200); 4],
        ));
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::RenderedSpread,
            original_format: SrcFormat::Other("spread".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "half_spread".to_string(),
            pixels: ExportPixels::Spread {
                left: ExportPagePixels {
                    base_pixels: left,
                    conceal_mask: None,
                    crop: None,
                },
                right: ExportPagePixels {
                    base_pixels: right,
                    conceal_mask: None,
                    crop: None,
                },
            },
            scale: ExportScale::Half,
            entries: vec![ExportEntry {
                label: "current".to_string(),
                suffix: 0,
                conceal_preset: None,
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

        let out = image::open(temp.path().join("half_spread_0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(out.dimensions(), (3, 1));
    }

    #[test]
    fn worker_exports_spread_pixels_with_per_page_conceal() {
        let temp = tempfile::tempdir().unwrap();
        let left = Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]));
        let right = Arc::new(egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgb(10, 20, 30)],
        ));
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::RenderedSpread,
            original_format: SrcFormat::Other("spread".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "spread".to_string(),
            pixels: ExportPixels::Spread {
                left: ExportPagePixels {
                    base_pixels: left,
                    conceal_mask: Some(Arc::new(vec![true])),
                    crop: None,
                },
                right: ExportPagePixels {
                    base_pixels: right,
                    conceal_mask: None,
                    crop: None,
                },
            },
            scale: ExportScale::Full,
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

        let out = image::open(temp.path().join("spread_0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(out.dimensions(), (2, 1));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [10, 20, 30, 255]);
    }
}
