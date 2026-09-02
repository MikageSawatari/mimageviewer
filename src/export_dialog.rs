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
    /// SNS 分割の枠。`Some` なら [`ExportPagePixels::crop`] より優先する。
    pub crop: Option<crate::export_crop::CropRect>,
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
    /// プリセット再合成が final AI を回している間、ローカル AI 利用中であることを
    /// remote 側の acquire barrier へ見せ続ける lease。AI を使わない export では `None`。
    /// lease は crate 内でしか作れないので、外から組み立てるときは常に `None`。
    pub local_ai_activity: Option<crate::app::LocalAiActivityLease>,
}

/// プリセット出力 (`_1`〜`_4`) を作り直すための「隠蔽適用前」スナップショット。
///
/// 表示は `raw → 消しゴム → 補正レイヤー → 隠蔽 → 色補正 → final AI → シャープ →
/// カラー化 → LUT → post filter → 注釈` の順で合成され、[`ExportPagePixels::base_pixels`]
/// はその最終結果 = **隠蔽が現在の設定で焼き込み済み**の絵である。プリセット出力は隠蔽の
/// パラメータだけが違う同じ絵なので、隠蔽の入力まで戻ってそこから先を同じ順で流し直す。
/// `base_pixels` へマスクを重ねるだけでは隠蔽が二重に掛かる (v1.1.0 の退行の原因)。
pub struct ConcealVariantSource {
    /// `raw → 消しゴム → 補正レイヤー` まで済んだ source 解像度のピクセル
    /// (= 表示側で隠蔽合成の入力になっているものと同じ)。
    pub base: Arc<egui::ColorImage>,
    /// `base` と同じ寸法へラスタライズ済みの合成マスク。
    pub mask: Arc<Vec<bool>>,
    pub params: crate::adjustment::AdjustParams,
    pub creative_lut: Option<(crate::creative_lut::SharedCreativeLut, f32)>,
    /// 表示の final composite が実際に final AI を通っているときだけ `Some`。
    /// 表示が AI 抜きなら (無効 / サイズ外 / 失敗) ここも `None` にして絵を揃える。
    pub ai: Option<ConcealVariantAi>,
    pub comic: Option<ConcealVariantComic>,
}

/// プリセット再合成で final AI を掛け直すための材料。モデル選択は worker 上で
/// [`crate::ai::final_pipeline::select_final_ai_models`] に委ね、UI スレッドで分類 (重い)
/// を走らせない。表示側が分類済みなら `category` で同じ答えを渡す。
pub struct ConcealVariantAi {
    pub runtime: Arc<crate::ai::runtime::AiRuntime>,
    pub manager: Arc<crate::ai::model_manager::ModelManager>,
    pub feature_mode: crate::settings::AiFeatureMode,
    pub upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    pub denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    pub category: Option<crate::ai::ImageCategory>,
    pub transparent_bg_mode: u8,
}

/// 注釈の焼き込み材料。製本の headless compositor と同じ
/// [`crate::books::bake_comic_annotations`] を使うためのスナップショット。
pub struct ConcealVariantComic {
    pub snapshot: crate::books::BookComicSnapshot,
    pub source_dims: Option<[usize; 2]>,
}

#[derive(Clone)]
pub struct ExportPagePixels {
    /// ダイアログを開いた瞬間の表示ピクセル (final composite + 注釈)。
    /// `_0` (現在の設定) はこれをそのまま書き出すので、画面と 1 pixel も違わない。
    pub base_pixels: Arc<egui::ColorImage>,
    /// プリセット出力用の再合成材料。`None` ならこのページはプリセット出力できない
    /// (= 隠蔽マスクが無い)。
    pub conceal_variant: Option<Arc<ConcealVariantSource>>,
    pub crop: Option<crate::export_crop::CropRect>,
    pub rotation: crate::rotation_db::Rotation,
}

impl ExportPagePixels {
    pub fn render_size(&self) -> [usize; 2] {
        self.render_size_with_crop(None)
    }

    pub fn render_size_with_crop(
        &self,
        entry_crop: Option<crate::export_crop::CropRect>,
    ) -> [usize; 2] {
        let [w, h] = self.base_pixels.size;
        let size = if let Some(crop) = entry_crop.or(self.crop) {
            let (_, _, crop_w, crop_h) = crop.pixel_bounds(w, h);
            [crop_w, crop_h]
        } else {
            [w.max(1), h.max(1)]
        };
        crate::capture::rotated_size(size, self.rotation)
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
    /// プリセット出力 (`_1`〜`_4`) を作れるか = 隠蔽マスクを持つページがあるか。
    pub fn has_conceal_mask(&self) -> bool {
        match self {
            Self::Single(page) => page.conceal_variant.is_some(),
            Self::Spread { left, right } => {
                left.conceal_variant.is_some() || right.conceal_variant.is_some()
            }
        }
    }

    /// プリセット再合成が final AI を回すか。worker が AI lease を握るかの判定に使う。
    pub(crate) fn conceal_variant_uses_ai(&self) -> bool {
        let uses_ai = |page: &ExportPagePixels| {
            page.conceal_variant
                .as_ref()
                .is_some_and(|v| v.ai.is_some())
        };
        match self {
            Self::Single(page) => uses_ai(page),
            Self::Spread { left, right } => uses_ai(left) || uses_ai(right),
        }
    }

    pub fn render_size(&self) -> [usize; 2] {
        self.render_size_with_crop(None)
    }

    pub fn render_size_with_crop(
        &self,
        entry_crop: Option<crate::export_crop::CropRect>,
    ) -> [usize; 2] {
        match self {
            Self::Single(page) => page.render_size_with_crop(entry_crop),
            Self::Spread { left, right } => {
                let [left_w, left_h] = left.render_size_with_crop(entry_crop);
                let [right_w, right_h] = right.render_size_with_crop(entry_crop);
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
    /// SNS 分割から開いたときの枠。空なら通常のエクスポート。
    pub sns_split_frames: Vec<crate::export_crop::CropRect>,
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

    pub fn render_size(&self) -> [usize; 2] {
        self.pixels
            .render_size_with_crop(self.sns_split_frames.first().copied())
    }
}

/// 書き出すバリエーションを決める。
///
/// `_0` (現在の設定) は [`ExportPagePixels::base_pixels`] = 表示スナップショットをそのまま
/// 出すので `conceal_preset` を持たない。隠蔽は既にそこへ焼き込まれており、preset を
/// 添えると二重適用になる。プリセット出力だけが隠蔽前 base から再合成する。
pub(crate) fn plan_export_entries(
    sns_split_frames: &[crate::export_crop::CropRect],
    selection: &[bool; 5],
    has_conceal_mask: bool,
    preset_slots: &crate::conceal::ConcealPresetSlots,
) -> Result<Vec<ExportEntry>, String> {
    if !sns_split_frames.is_empty() {
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
                crop: Some(crop),
            })
            .collect());
    }

    let mut entries = Vec::new();
    if selection[0] {
        entries.push(ExportEntry {
            label: "現在の設定".to_string(),
            suffix: 0,
            conceal_preset: None,
            crop: None,
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
            crop: None,
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
        let pixels = match render_export_pixels(
            &request.pixels,
            entry.conceal_preset.as_ref(),
            entry.crop,
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

pub(crate) fn render_export_pixels<'a>(
    pixels: &'a ExportPixels,
    preset: Option<&ConcealPreset>,
    entry_crop: Option<crate::export_crop::CropRect>,
    cancel: &Arc<AtomicBool>,
) -> Result<Cow<'a, egui::ColorImage>, ExportRenderError> {
    match pixels {
        ExportPixels::Single(page) => render_export_page_pixels(page, preset, entry_crop, cancel),
        ExportPixels::Spread { left, right } => {
            let left = render_export_page_pixels(left, preset, entry_crop, cancel)?;
            let right = render_export_page_pixels(right, preset, entry_crop, cancel)?;
            let combined =
                crate::capture::combine_spread_color_images(left.as_ref(), right.as_ref())?;
            Ok(Cow::Owned(combined))
        }
    }
}

fn render_export_page_pixels<'a>(
    page: &'a ExportPagePixels,
    preset: Option<&ConcealPreset>,
    entry_crop: Option<crate::export_crop::CropRect>,
    cancel: &Arc<AtomicBool>,
) -> Result<Cow<'a, egui::ColorImage>, ExportRenderError> {
    let rendered = match (&page.conceal_variant, preset) {
        // プリセット出力は隠蔽の入力まで戻って合成し直す。マスクが無いページ
        // (見開きの片側など) は preset が選ばれていても表示スナップショットのまま。
        (Some(variant), Some(preset)) => {
            Cow::Owned(compose_conceal_variant(variant, preset, cancel)?)
        }
        // `_0` と SNS 分割は表示スナップショットをそのまま出す。
        _ => Cow::Borrowed(page.base_pixels.as_ref()),
    };
    if let Some(crop) = entry_crop.or(page.crop) {
        let cropped = crate::export_crop::crop_color_image(rendered.as_ref(), crop)?;
        if page.rotation.is_none() {
            return Ok(Cow::Owned(cropped));
        }
        return Ok(Cow::Owned(crate::capture::rotate_color_image(
            &cropped,
            page.rotation,
        )));
    }
    if !page.rotation.is_none() {
        return Ok(Cow::Owned(crate::capture::rotate_color_image(
            rendered.as_ref(),
            page.rotation,
        )));
    }
    Ok(rendered)
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

/// プリセット 1 枚ぶんを、表示パイプラインの隠蔽以降と同じ順で組み立て直す。
///
/// 隠蔽 -> 色補正 / final AI -> シャープ -> カラー化 -> LUT -> post filter -> 注釈。
/// 段の順と適用条件は表示側 (docs/display-pipeline.md 3.0) と同一で、違うのは隠蔽の
/// パラメータだけ。crop と回転は呼び出し元が最終段で掛ける。
fn compose_conceal_variant(
    variant: &ConcealVariantSource,
    preset: &ConcealPreset,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, ExportRenderError> {
    let expected = variant.base.size[0]
        .checked_mul(variant.base.size[1])
        .ok_or_else(|| "隠蔽加工マスクのサイズが大きすぎます".to_string())?;
    if variant.mask.len() != expected {
        return Err(ExportRenderError::Failed(format!(
            "隠蔽加工マスクのサイズが一致しません: mask={}, expected={}",
            variant.mask.len(),
            expected
        )));
    }
    let concealed = crate::conceal_compose::compose_with_preset_cancel(
        &variant.base,
        &variant.mask,
        preset,
        cancel,
    )
    .ok_or(ExportRenderError::Cancelled)?;

    let params = &variant.params;
    let adjust_only = || (!params.is_color_identity()).then(|| params.clone());
    let concealed = Arc::new(concealed);
    let (source, used_upscale, adjust_before_effect) = match &variant.ai {
        Some(ai) => match crate::ai::final_pipeline::select_final_ai_models(
            &concealed,
            params,
            ai.feature_mode,
            ai.upscale_limit,
            ai.denoise_limit,
            ai.category,
        ) {
            Some(models) => {
                let request = crate::ai::final_pipeline::FinalAiExecutionRequest {
                    source: Arc::clone(&concealed),
                    // 隠蔽前 base は色補正前なので、表示の先読み経路と同じく AI 側で焼く。
                    adjust_before_ai: adjust_only(),
                    denoise_kind: models.denoise,
                    upscale_kind: models.upscale,
                    background_mode: ai.transparent_bg_mode,
                };
                match crate::ai::final_pipeline::execute_selected_final_ai(
                    &ai.runtime,
                    &ai.manager,
                    request,
                    cancel,
                    &crate::ai::final_pipeline::NoFinalAiProgress,
                ) {
                    Ok(output) => (Arc::new(output.image), output.used_upscale, None),
                    Err(crate::ai::final_pipeline::FinalAiExecutionError::Cancelled) => {
                        return Err(ExportRenderError::Cancelled);
                    }
                    // _0 は AI を通った絵なので、ここで AI 抜きへ落とすと寸法から別物に
                    // なる。黙って劣化させず、このエントリを失敗として報告する。
                    Err(crate::ai::final_pipeline::FinalAiExecutionError::Failed(error)) => {
                        return Err(ExportRenderError::Failed(format!(
                            "AI 処理に失敗しました: {error}"
                        )));
                    }
                }
            }
            None => (concealed, false, adjust_only()),
        },
        None => (concealed, false, adjust_only()),
    };

    let plan = crate::final_composite::FinalCompositePlan {
        adjust_before_effect,
        smart_sharpen: params.effective_smart_sharpen(used_upscale),
        colorize: params.colorize.clone(),
        colorize_applicable_override: None,
        creative_lut: variant.creative_lut.clone(),
        post_filter: params.post_filter,
    };
    let composed = match crate::final_composite::execute_final_composite(source, plan, cancel) {
        crate::final_composite::FinalCompositeResult::Ready { pixels, .. } => pixels,
        crate::final_composite::FinalCompositeResult::Cancelled => {
            return Err(ExportRenderError::Cancelled);
        }
    };

    Ok(match &variant.comic {
        Some(comic) => {
            match crate::books::bake_comic_annotations(
                &composed,
                &comic.snapshot,
                comic.source_dims,
                cancel,
            ) {
                Ok(image) => image,
                // 注釈の焼き込みは cancel でしか失敗しないが、将来別の失敗が増えても
                // 取り違えないよう、フラグを見てどちらかを決める。
                Err(message) => {
                    return Err(if cancel.load(Ordering::Relaxed) {
                        ExportRenderError::Cancelled
                    } else {
                        ExportRenderError::Failed(message)
                    });
                }
            }
        }
        None => Arc::try_unwrap(composed).unwrap_or_else(|shared| (*shared).clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 隠蔽マスクだけを持つ最小の再合成材料 (AI / LUT / 注釈 / 色補正なし)。
    /// この構成では `compose_conceal_variant` は「隠蔽を掛けるだけ」に縮退する。
    fn mask_only_variant(
        base: Arc<egui::ColorImage>,
        mask: Vec<bool>,
    ) -> Arc<ConcealVariantSource> {
        Arc::new(ConcealVariantSource {
            base,
            mask: Arc::new(mask),
            params: crate::adjustment::AdjustParams::default(),
            creative_lut: None,
            ai: None,
            comic: None,
        })
    }

    fn page(base_pixels: Arc<egui::ColorImage>) -> ExportPagePixels {
        ExportPagePixels {
            base_pixels,
            conceal_variant: None,
            crop: None,
            rotation: crate::rotation_db::Rotation::None,
        }
    }

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

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
    fn sns_split_entry_plan_uses_one_numbered_entry_per_frame_without_presets() {
        let frames = (0..4)
            .map(|index| crate::export_crop::CropRect {
                min_x: (index * 10) as f32,
                min_y: 0.0,
                max_x: (index * 10 + 8) as f32,
                max_y: 12.0,
            })
            .collect::<Vec<_>>();
        let preset = ConcealPreset {
            name: "batch preset".to_string(),
            ..Default::default()
        };
        let slots = [
            Some(preset.clone()),
            Some(preset.clone()),
            Some(preset.clone()),
            Some(preset.clone()),
        ];

        let entries = plan_export_entries(&frames, &[true; 5], true, &slots).unwrap();

        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries.iter().map(|entry| entry.suffix).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            vec!["1 / 4", "2 / 4", "3 / 4", "4 / 4"]
        );
        for (entry, frame) in entries.iter().zip(&frames) {
            assert!(entry.conceal_preset.is_none());
            assert_eq!(entry.crop, Some(*frame));
        }
    }

    #[test]
    fn normal_entry_plan_gives_only_preset_variants_a_conceal_override() {
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
            Some(ConcealPreset {
                name: String::new(),
                ..Default::default()
            }),
        ];

        let entries =
            plan_export_entries(&[], &[true, true, true, false, true], true, &slots).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.label.as_str(), entry.suffix))
                .collect::<Vec<_>>(),
            vec![
                ("現在の設定", 0),
                ("プリセット1: one", 1),
                ("プリセット4", 4)
            ]
        );
        assert!(entries.iter().all(|entry| entry.crop.is_none()));
        // `_0` は表示スナップショット (隠蔽は焼き込み済み) をそのまま出すので override
        // を持たない。preset を添えると隠蔽が二重に掛かる。
        assert!(entries[0].conceal_preset.is_none());
        assert!(
            entries[1..]
                .iter()
                .all(|entry| entry.conceal_preset.is_some())
        );
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
                conceal_variant: None,
                crop: None,
                rotation: crate::rotation_db::Rotation::None,
            }),
            scale: ExportScale::Full,
            entries: vec![
                ExportEntry {
                    label: "current".to_string(),
                    suffix: 0,
                    conceal_preset: None,
                    crop: None,
                },
                ExportEntry {
                    label: "preset1".to_string(),
                    suffix: 1,
                    conceal_preset: None,
                    crop: None,
                },
            ],
            include_metadata: false,
            local_ai_activity: None,
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
    fn worker_forwards_entry_crop_and_overrides_page_crop() {
        let temp = tempfile::tempdir().unwrap();
        let red = egui::Color32::from_rgb(255, 0, 0);
        let pixels = Arc::new(egui::ColorImage::new(
            [4, 1],
            vec![
                red,
                egui::Color32::from_rgb(0, 255, 0),
                egui::Color32::from_rgb(0, 0, 255),
                egui::Color32::WHITE,
            ],
        ));
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::PdfPage,
            original_format: SrcFormat::Other("pdf".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "entry_crop".to_string(),
            pixels: ExportPixels::Single(ExportPagePixels {
                base_pixels: pixels,
                conceal_variant: None,
                crop: Some(crate::export_crop::CropRect {
                    min_x: 1.0,
                    min_y: 0.0,
                    max_x: 3.0,
                    max_y: 1.0,
                }),
                rotation: crate::rotation_db::Rotation::None,
            }),
            scale: ExportScale::Full,
            entries: vec![ExportEntry {
                label: "first frame".to_string(),
                suffix: 1,
                conceal_preset: None,
                crop: Some(crate::export_crop::CropRect {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 1.0,
                    max_y: 1.0,
                }),
            }],
            include_metadata: false,
            local_ai_activity: None,
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

        let out = image::open(temp.path().join("entry_crop_1.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!([out.width(), out.height()], [1, 1]);
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    /// `_0` は表示スナップショットをそのまま、`_1` は隠蔽前 base から掛け直す。
    /// 2 つの入力を意図的に別の絵にして、どちらがどちらへ流れるかを固定する。
    #[test]
    fn worker_writes_display_snapshot_for_current_and_recomposes_preset_variants() {
        let temp = tempfile::tempdir().unwrap();
        let displayed = Arc::new(egui::ColorImage::new(
            [2, 2],
            vec![egui::Color32::from_rgb(7, 8, 9); 4],
        ));
        let pre_conceal = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]));
        let pending = spawn_export_worker(ExportRequest {
            source: ExportSource::PdfPage,
            original_format: SrcFormat::Other("pdf".to_string()),
            output_format: ExportFormat::Png,
            output_dir: temp.path().to_path_buf(),
            basename: "masked".to_string(),
            pixels: ExportPixels::Single(ExportPagePixels {
                base_pixels: displayed,
                conceal_variant: Some(mask_only_variant(
                    pre_conceal,
                    vec![true, false, false, false],
                )),
                crop: None,
                rotation: crate::rotation_db::Rotation::None,
            }),
            scale: ExportScale::Full,
            entries: vec![
                ExportEntry {
                    label: "current".to_string(),
                    suffix: 0,
                    conceal_preset: None,
                    crop: None,
                },
                ExportEntry {
                    label: "black".to_string(),
                    suffix: 1,
                    conceal_preset: Some(ConcealPreset {
                        conceal_type: crate::conceal::ConcealType::BlackFill,
                        ..Default::default()
                    }),
                    crop: None,
                },
            ],
            include_metadata: false,
            local_ai_activity: None,
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

        let current = image::open(temp.path().join("masked_0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(current.get_pixel(0, 0).0, [7, 8, 9, 255]);
        assert_eq!(current.get_pixel(1, 0).0, [7, 8, 9, 255]);

        let variant = image::open(temp.path().join("masked_1.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(variant.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(variant.get_pixel(1, 0).0, [255, 255, 255, 255]);
    }

    /// プリセット再合成は表示と同じ順 (隠蔽 -> 色補正) で流す。逆順だと黒塗りが
    /// そのまま残るので、明度を上げた黒塗り画素で順序を固定できる。
    #[test]
    fn conceal_variant_applies_tone_after_conceal_like_the_display_pipeline() {
        let base = Arc::new(egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2]));
        let mut variant = ConcealVariantSource {
            base,
            mask: Arc::new(vec![true, false]),
            params: crate::adjustment::AdjustParams::default(),
            creative_lut: None,
            ai: None,
            comic: None,
        };
        variant.params.brightness = 50.0;
        let preset = ConcealPreset {
            conceal_type: crate::conceal::ConcealType::BlackFill,
            ..Default::default()
        };

        let out = compose_conceal_variant(&variant, &preset, &no_cancel())
            .unwrap_or_else(|_| panic!("compose failed"));

        // 黒塗り後に明度が乗るので、マスク画素は純黒のままにならない。
        assert!(out.pixels[0].r() > 100, "got {:?}", out.pixels[0]);
        assert_eq!(out.pixels[1].r(), 255);
    }

    #[test]
    fn conceal_variant_rejects_a_mask_that_does_not_match_the_base() {
        let base = Arc::new(egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2]));
        let variant = ConcealVariantSource {
            base,
            mask: Arc::new(vec![true]),
            params: crate::adjustment::AdjustParams::default(),
            creative_lut: None,
            ai: None,
            comic: None,
        };

        let err = compose_conceal_variant(&variant, &ConcealPreset::default(), &no_cancel());

        assert!(matches!(err, Err(ExportRenderError::Failed(_))));
    }

    #[test]
    fn conceal_variant_reports_cancel_instead_of_failure() {
        let base = Arc::new(egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2]));
        let variant = ConcealVariantSource {
            base,
            mask: Arc::new(vec![true, false]),
            params: crate::adjustment::AdjustParams::default(),
            creative_lut: None,
            ai: None,
            comic: None,
        };
        let cancel = Arc::new(AtomicBool::new(true));

        let out = compose_conceal_variant(&variant, &ConcealPreset::default(), &cancel);

        assert!(matches!(out, Err(ExportRenderError::Cancelled)));
    }

    #[test]
    fn pages_without_a_mask_keep_the_display_snapshot_even_when_a_preset_is_selected() {
        let displayed = Arc::new(egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgb(1, 2, 3)],
        ));
        let page = page(displayed);
        let preset = ConcealPreset {
            conceal_type: crate::conceal::ConcealType::BlackFill,
            ..Default::default()
        };

        let out = render_export_page_pixels(&page, Some(&preset), None, &no_cancel())
            .unwrap_or_else(|_| panic!("render failed"));

        assert_eq!(out.pixels, vec![egui::Color32::from_rgb(1, 2, 3)]);
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
            conceal_variant: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 1.0,
            }),
            rotation: crate::rotation_db::Rotation::None,
        };

        let out = render_export_page_pixels(&page, None, None, &no_cancel()).unwrap();

        assert_eq!(out.size, [2, 1]);
        assert_eq!(out.pixels[0], egui::Color32::from_rgb(0, 255, 0));
        assert_eq!(out.pixels[1], egui::Color32::from_rgb(0, 0, 255));
    }

    #[test]
    fn render_export_page_pixels_prefers_entry_crop_over_page_crop() {
        let red = egui::Color32::from_rgb(255, 0, 0);
        let pixels = Arc::new(egui::ColorImage::new(
            [4, 1],
            vec![
                red,
                egui::Color32::from_rgb(0, 255, 0),
                egui::Color32::from_rgb(0, 0, 255),
                egui::Color32::WHITE,
            ],
        ));
        let page = ExportPagePixels {
            base_pixels: pixels,
            conceal_variant: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 1.0,
            }),
            rotation: crate::rotation_db::Rotation::None,
        };
        let entry_crop = crate::export_crop::CropRect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 1.0,
            max_y: 1.0,
        };

        let out = render_export_page_pixels(&page, None, Some(entry_crop), &no_cancel()).unwrap();

        assert_eq!(out.size, [1, 1]);
        assert_eq!(out.pixels, vec![red]);
        assert_eq!(page.render_size_with_crop(Some(entry_crop)), [1, 1]);
    }

    #[test]
    fn render_export_page_pixels_applies_rotation_after_crop() {
        let px = |v| egui::Color32::from_rgb(v, 0, 0);
        let pixels = Arc::new(egui::ColorImage::new(
            [3, 2],
            vec![px(1), px(2), px(3), px(4), px(5), px(6)],
        ));
        let page = ExportPagePixels {
            base_pixels: pixels,
            conceal_variant: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 2.0,
            }),
            rotation: crate::rotation_db::Rotation::Cw90,
        };

        let out = render_export_page_pixels(&page, None, None, &no_cancel()).unwrap();

        assert_eq!(out.size, [2, 2]);
        assert_eq!(out.pixels, vec![px(5), px(2), px(6), px(3)]);
        assert_eq!(page.render_size(), [2, 2]);
    }

    #[test]
    fn export_scale_dimensions_use_rendered_crop_spread_size() {
        let left = ExportPagePixels {
            base_pixels: Arc::new(egui::ColorImage::new(
                [5, 4],
                vec![egui::Color32::BLACK; 20],
            )),
            conceal_variant: None,
            crop: Some(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 1.0,
                max_x: 5.0,
                max_y: 4.0,
            }),
            rotation: crate::rotation_db::Rotation::None,
        };
        let right = ExportPagePixels {
            base_pixels: Arc::new(egui::ColorImage::new(
                [3, 5],
                vec![egui::Color32::WHITE; 15],
            )),
            conceal_variant: None,
            crop: None,
            rotation: crate::rotation_db::Rotation::None,
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
                    conceal_variant: None,
                    crop: None,
                    rotation: crate::rotation_db::Rotation::None,
                },
                right: ExportPagePixels {
                    base_pixels: right,
                    conceal_variant: None,
                    crop: None,
                    rotation: crate::rotation_db::Rotation::None,
                },
            },
            scale: ExportScale::Half,
            entries: vec![ExportEntry {
                label: "current".to_string(),
                suffix: 0,
                conceal_preset: None,
                crop: None,
            }],
            include_metadata: false,
            local_ai_activity: None,
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
                    base_pixels: Arc::clone(&left),
                    conceal_variant: Some(mask_only_variant(left, vec![true])),
                    crop: None,
                    rotation: crate::rotation_db::Rotation::None,
                },
                right: ExportPagePixels {
                    base_pixels: right,
                    conceal_variant: None,
                    crop: None,
                    rotation: crate::rotation_db::Rotation::None,
                },
            },
            scale: ExportScale::Full,
            entries: vec![ExportEntry {
                label: "black".to_string(),
                suffix: 1,
                conceal_preset: Some(ConcealPreset {
                    conceal_type: crate::conceal::ConcealType::BlackFill,
                    ..Default::default()
                }),
                crop: None,
            }],
            include_metadata: false,
            local_ai_activity: None,
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

        let out = image::open(temp.path().join("spread_1.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(out.dimensions(), (2, 1));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [10, 20, 30, 255]);
    }
}
