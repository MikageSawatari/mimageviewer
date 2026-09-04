//! Ctrl+E export dialog support.
//!
//! The UI side snapshots source identity, edit state, and selected presets.
//! This module decodes and composes the source, encodes images, and writes files
//! on a worker so heavy CPU/I/O work never blocks egui.

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

#[cfg(test)]
mod composite_tests {
    use super::*;

    #[test]
    fn sns_entry_plan_keeps_authoring_coordinates_for_the_worker() {
        let frames = vec![
            crate::export_crop::CropRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 277.0,
                max_y: 400.0,
            },
            crate::export_crop::CropRect {
                min_x: 277.0,
                min_y: 0.0,
                max_x: 555.0,
                max_y: 400.0,
            },
            crate::export_crop::CropRect {
                min_x: 555.0,
                min_y: 0.0,
                max_x: 832.0,
                max_y: 400.0,
            },
        ];
        let entries = plan_export_entries(
            &frames,
            Some([832, 400]),
            &[true; 5],
            true,
            &[None, None, None, None],
        )
        .unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.label.as_str(), entry.suffix))
                .collect::<Vec<_>>(),
            vec![("1 / 3", 1), ("2 / 3", 2), ("3 / 3", 3)]
        );
        assert_eq!(entries[1].crop_override, Some((frames[1], [832, 400])));
        assert!(entries.iter().all(|entry| entry.conceal_preset.is_none()));
    }

    #[test]
    fn normal_entry_plan_only_overrides_conceal_for_selected_presets() {
        let slots = [
            Some(ConcealPreset {
                name: "one".to_string(),
                ..Default::default()
            }),
            None,
            Some(ConcealPreset {
                name: "three".to_string(),
                ..Default::default()
            }),
            Some(ConcealPreset::default()),
        ];
        let entries =
            plan_export_entries(&[], None, &[true, true, true, false, true], true, &slots).unwrap();

        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.label.as_str(), entry.suffix))
                .collect::<Vec<_>>(),
            vec![
                ("現在の設定", 0),
                ("プリセット1: one", 1),
                ("プリセット4", 4),
            ]
        );
        assert!(entries[0].conceal_preset.is_none());
        assert!(
            entries[1..]
                .iter()
                .all(|entry| entry.conceal_preset.is_some())
        );
        assert!(entries.iter().all(|entry| entry.crop_override.is_none()));
    }

    #[test]
    fn session_basename_keeps_a_free_name_and_reserves_every_suffix() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_session_basename(temp.path(), "sample", "png", &[0, 1]).unwrap(),
            "sample"
        );
        std::fs::write(temp.path().join("sample_1.png"), b"occupied").unwrap();
        assert_eq!(
            resolve_session_basename(temp.path(), "sample", "png", &[0, 1]).unwrap(),
            "sample_0001"
        );
    }

    #[test]
    fn long_edge_scale_downscales_without_upscaling() {
        assert_eq!(
            ExportScale::LongEdge(1000).scaled_size([4000, 2000]),
            [1000, 500]
        );
        assert_eq!(
            ExportScale::LongEdge(4096).scaled_size([1920, 1080]),
            [1920, 1080]
        );
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
    /// SNS 分割の枠と、その枠を作成したときの合成画像サイズ。
    pub crop_override: Option<(crate::export_crop::CropRect, [usize; 2])>,
}

pub struct ExportRequest {
    pub source: ExportSource,
    pub original_format: SrcFormat,
    pub output_format: ExportFormat,
    pub output_dir: PathBuf,
    pub basename: String,
    pub composite: ExportComposite,
    pub scale: ExportScale,
    pub entries: Vec<ExportEntry>,
    pub include_metadata: bool,
    /// プリセット再合成が final AI を回している間、ローカル AI 利用中であることを
    /// remote 側の acquire barrier へ見せ続ける lease。AI を使わない export では `None`。
    /// lease は crate 内でしか作れないので、外から組み立てるときは常に `None`。
    pub local_ai_activity: Option<crate::app::LocalAiActivityLease>,
}

pub struct ExportPageComposite {
    /// worker がデコードする元ページ。
    pub source: crate::books::CompositeSource,
    /// 選択された焼き込み段までを再構成する編集スナップショット。
    pub edits: crate::books::BakedEditSnapshot,
    pub pdf_render_long_edge: u32,
    /// 回転・切り取りを適用する前の合成予測寸法。
    pub predicted_size: [usize; 2],
    pub has_conceal_mask: bool,
}

impl ExportPageComposite {
    pub fn render_size(&self) -> Result<[usize; 2], String> {
        self.render_size_with_crop(None)
    }

    pub fn render_size_with_crop(
        &self,
        crop_override: Option<(crate::export_crop::CropRect, [usize; 2])>,
    ) -> Result<[usize; 2], String> {
        crate::books::predicted_export_entry_size(self.predicted_size, &self.edits, crop_override)
    }
}

pub enum ExportComposite {
    Single(ExportPageComposite),
    Spread {
        left: ExportPageComposite,
        right: ExportPageComposite,
    },
}

impl ExportComposite {
    /// プリセット出力 (`_1`〜`_4`) を作れるか = 隠蔽マスクを持つページがあるか。
    pub fn has_conceal_mask(&self) -> bool {
        match self {
            Self::Single(page) => page.has_conceal_mask,
            Self::Spread { left, right } => left.has_conceal_mask || right.has_conceal_mask,
        }
    }

    pub(crate) fn includes_ai_stage(&self) -> bool {
        match self {
            Self::Single(page) => page.edits.stage.includes_ai(),
            Self::Spread { left, right } => {
                left.edits.stage.includes_ai() || right.edits.stage.includes_ai()
            }
        }
    }

    pub fn render_size(&self) -> Result<[usize; 2], String> {
        self.render_size_with_crop(None)
    }

    pub fn render_size_with_crop(
        &self,
        crop_override: Option<(crate::export_crop::CropRect, [usize; 2])>,
    ) -> Result<[usize; 2], String> {
        match self {
            Self::Single(page) => page.render_size_with_crop(crop_override),
            Self::Spread { left, right } => {
                let [left_w, left_h] = left.render_size_with_crop(crop_override)?;
                let [right_w, right_h] = right.render_size_with_crop(crop_override)?;
                Ok([left_w + right_w, left_h.max(right_h)])
            }
        }
    }
}

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
    /// SNS 分割から開いたときの枠。空なら通常のエクスポート。
    pub sns_split_frames: Vec<crate::export_crop::CropRect>,
    pub sns_split_source_size: Option<[usize; 2]>,
    /// worker が元画像から選択段までを合成するための source と編集スナップショット。
    pub composite: ExportComposite,
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

    pub fn render_size(&self) -> Result<[usize; 2], String> {
        let crop_override = self
            .sns_split_frames
            .first()
            .copied()
            .zip(self.sns_split_source_size);
        self.composite.render_size_with_crop(crop_override)
    }
}

/// 書き出すバリエーションを決める。
///
/// `_0` (現在の設定) は保存済みの隠蔽設定を使うので `conceal_preset` を持たない。
/// プリセット出力だけが現在の隠蔽設定を置き換えて同じ worker 合成を行う。
pub(crate) fn plan_export_entries(
    sns_split_frames: &[crate::export_crop::CropRect],
    sns_split_source_size: Option<[usize; 2]>,
    selection: &[bool; 5],
    has_conceal_mask: bool,
    preset_slots: &crate::conceal::ConcealPresetSlots,
) -> Result<Vec<ExportEntry>, String> {
    if !sns_split_frames.is_empty() {
        let source_size = sns_split_source_size
            .ok_or_else(|| "SNS 分割枠の基準サイズがありません".to_string())?;
        let total = sns_split_frames.len();
        if total > usize::from(u8::MAX) {
            return Err("SNS 分割の枚数が多すぎます".to_string());
        }
        return Ok(sns_split_frames
            .iter()
            .copied()
            .enumerate()
            .map(|(index, crop)| ExportEntry {
                label: format!("{} / {total}", index + 1),
                suffix: (index + 1) as u8,
                conceal_preset: None,
                crop_override: Some((crop, source_size)),
            })
            .collect());
    }

    let mut entries = Vec::new();
    if selection[0] {
        entries.push(ExportEntry {
            label: "現在の設定".to_string(),
            suffix: 0,
            conceal_preset: None,
            crop_override: None,
        });
    }
    for (slot_idx, preset) in preset_slots.iter().enumerate() {
        if !selection[slot_idx + 1] || !has_conceal_mask {
            continue;
        }
        let Some(preset) = preset.clone() else {
            continue;
        };
        let label = if preset.name.trim().is_empty() {
            format!("プリセット{}", slot_idx + 1)
        } else {
            format!("プリセット{}: {}", slot_idx + 1, preset.name)
        };
        entries.push(ExportEntry {
            label,
            suffix: (slot_idx + 1) as u8,
            conceal_preset: Some(preset),
            crop_override: None,
        });
    }
    Ok(entries)
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
    // ローカル AI 利用中であることを worker 終端まで remote の acquire barrier へ見せる。
    let _local_ai_activity = request.local_ai_activity;
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
        // source compositor の decode は通常ファイルも ZIP 内画像も EXIF Orientation を
        // canonical 向きへ適用する。メタデータ転記時は Orientation を 1 に正規化して
        // 外部ビューアでの二重回転を避ける。
        caller_applied_orientation: true,
        ..Default::default()
    };
    let extension = request.output_format.extension();
    let decoded = match decode_export_composite(&request.composite, &cancel) {
        Ok(decoded) => decoded,
        Err(ExportRenderError::Cancelled) => {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        Err(ExportRenderError::Failed(message)) => {
            for entry in &request.entries {
                let _ = tx.send(ExportEvent::Failed(ExportFailure {
                    label: entry.label.clone(),
                    message: message.clone(),
                }));
            }
            let _ = tx.send(ExportEvent::AllDone);
            return;
        }
    };

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
        let pixels = match render_export_composite(
            &decoded,
            entry.conceal_preset.as_ref(),
            entry.crop_override,
            &cancel,
        ) {
            Ok(pixels) => pixels,
            // プリセット再合成は AI 推論を含み分単位になりうるので、合成の途中で
            // cancel が立ったら失敗ではなくキャンセルとして畳む。
            Err(ExportRenderError::Cancelled) => {
                let _ = tx.send(ExportEvent::Cancelled);
                return;
            }
            Err(ExportRenderError::Failed(message)) => {
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
        let pixels = match scale_export_pixels(Cow::Owned(pixels), request.scale) {
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

/// 1 エントリぶんの描画で起きうる終わり方。cancel を失敗として報告しないために分ける。
#[derive(Debug)]
pub(crate) enum ExportRenderError {
    Cancelled,
    Failed(String),
}

impl From<String> for ExportRenderError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl std::fmt::Display for ExportRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("書き出しをキャンセルしました"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

struct DecodedExportPage<'a> {
    page: &'a ExportPageComposite,
    image: egui::ColorImage,
}

/// decode 結果と、その結果を作ったページ要求を同じ variant に束ねる。
/// これにより Single / Spread の組み合わせ不一致を型で表現不能にする。
enum DecodedExportComposite<'a> {
    Single(DecodedExportPage<'a>),
    Spread {
        left: DecodedExportPage<'a>,
        right: DecodedExportPage<'a>,
    },
}

fn decode_export_page<'a>(
    page: &'a ExportPageComposite,
    cancel: &Arc<AtomicBool>,
) -> Result<DecodedExportPage<'a>, ExportRenderError> {
    let image = crate::books::decode_composite_source_for_materialization(
        &page.source,
        page.pdf_render_long_edge,
        Arc::clone(cancel),
    )
    .map_err(|message| {
        if cancel.load(Ordering::Relaxed) {
            ExportRenderError::Cancelled
        } else {
            ExportRenderError::Failed(message)
        }
    })?;
    Ok(DecodedExportPage { page, image })
}

fn decode_export_composite<'a>(
    composite: &'a ExportComposite,
    cancel: &Arc<AtomicBool>,
) -> Result<DecodedExportComposite<'a>, ExportRenderError> {
    match composite {
        ExportComposite::Single(page) => Ok(DecodedExportComposite::Single(decode_export_page(
            page, cancel,
        )?)),
        ExportComposite::Spread { left, right } => Ok(DecodedExportComposite::Spread {
            left: decode_export_page(left, cancel)?,
            right: decode_export_page(right, cancel)?,
        }),
    }
}

fn render_export_composite(
    decoded: &DecodedExportComposite<'_>,
    preset: Option<&ConcealPreset>,
    crop_override: Option<(crate::export_crop::CropRect, [usize; 2])>,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, ExportRenderError> {
    match decoded {
        DecodedExportComposite::Single(decoded) => {
            render_export_page(decoded.page, &decoded.image, preset, crop_override, cancel)
        }
        DecodedExportComposite::Spread { left, right } => {
            let left = render_export_page(left.page, &left.image, preset, crop_override, cancel)?;
            let right =
                render_export_page(right.page, &right.image, preset, crop_override, cancel)?;
            Ok(crate::capture::combine_spread_color_images(&left, &right)?)
        }
    }
}

fn render_export_page(
    page: &ExportPageComposite,
    decoded: &egui::ColorImage,
    preset: Option<&ConcealPreset>,
    crop_override: Option<(crate::export_crop::CropRect, [usize; 2])>,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, ExportRenderError> {
    crate::books::compose_export_entry(
        decoded.clone(),
        &page.edits,
        preset,
        crop_override,
        Arc::clone(cancel),
    )
    .map_err(|message| {
        if cancel.load(Ordering::Relaxed) {
            ExportRenderError::Cancelled
        } else {
            ExportRenderError::Failed(message)
        }
    })
}

/// 書き出し直前の縮小段。Ctrl+E の単ページ / 見開きと、一括エクスポート・製本が通る
/// `books::write_composited_page` が同じ 1 段を共有する。
pub(crate) fn scale_export_pixels<'a>(
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
