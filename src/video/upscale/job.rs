use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;
use image::{DynamicImage, ImageBuffer, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::ai::{ModelKind, model_manager::ModelManager, runtime::AiRuntime};

use super::manifest::{
    JobManifest, ManifestOptions, ManifestOutput, ManifestSource, PlannedSegment, SegmentEntry,
    SegmentPlan, SegmentPlanState, SegmentPlanStrategy, SegmentState, TimeBase, save_json_atomic,
};
use super::paths::{
    MANIFEST_FILE_NAME, final_part_path_for, manifest_path_for, segment_file_name,
    segment_part_file_name, segment_part_path, segment_path, segments_dir_for, work_dir_for,
    worker_segment_part_path,
};
use super::sidecar::{
    EncodeInfo, OutputInfo, UpscaleInfo, VideoUpscaleSidecar, derived_sidecar_path_for,
    derived_video_path_for, output_within_mvp_limit, source_info_for,
};

const FFMPEG_EAGAIN: i32 = 11;
const SEGMENT_TARGET_SECONDS: f64 = 5.0;
const SEGMENT_MIN_SECONDS: f64 = 1.0;
const SEGMENT_MAX_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoUpscaleScale {
    X2,
    X4,
}

impl VideoUpscaleScale {
    pub fn factor(self) -> u32 {
        match self {
            Self::X2 => 2,
            Self::X4 => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::X2 => "2x",
            Self::X4 => "4x",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoUpscaleModelPreset {
    GeneralFast,
    Anime,
    Photo,
}

impl VideoUpscaleModelPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::GeneralFast => "汎用 (高速)",
            Self::Anime => "アニメ",
            Self::Photo => "写真",
        }
    }

    pub fn model_kind(self) -> ModelKind {
        match self {
            Self::GeneralFast => ModelKind::UpscaleRealEsrGeneralV3,
            Self::Anime => ModelKind::UpscaleRealEsrganAnime6B,
            Self::Photo => ModelKind::UpscaleRealEsrganX4Plus,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoUpscaleQuality {
    Q1,
    Q2,
    Q3,
    Q4,
    Q5,
}

impl VideoUpscaleQuality {
    pub fn level(self) -> u8 {
        match self {
            Self::Q1 => 1,
            Self::Q2 => 2,
            Self::Q3 => 3,
            Self::Q4 => 4,
            Self::Q5 => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Q1 => "1 最高品質",
            Self::Q2 => "2 高品質",
            Self::Q3 => "3 標準",
            Self::Q4 => "4 小さめ",
            Self::Q5 => "5 最小",
        }
    }

    pub fn crf(self) -> u8 {
        match self {
            Self::Q1 => 20,
            Self::Q2 => 24,
            Self::Q3 => 28,
            Self::Q4 => 32,
            Self::Q5 => 36,
        }
    }

    pub fn preset(self) -> u8 {
        match self {
            Self::Q1 => 7,
            Self::Q2 | Self::Q3 => 8,
            Self::Q4 | Self::Q5 => 9,
        }
    }

    pub fn pixel_format(self) -> ffmpeg::format::Pixel {
        match self {
            Self::Q1 | Self::Q2 => ffmpeg::format::Pixel::YUV420P10LE,
            Self::Q3 | Self::Q4 | Self::Q5 => ffmpeg::format::Pixel::YUV420P,
        }
    }

    pub fn pixel_format_name(self) -> &'static str {
        match self {
            Self::Q1 | Self::Q2 => "yuv420p10le",
            Self::Q3 | Self::Q4 | Self::Q5 => "yuv420p",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoUpscaleOptions {
    pub scale: VideoUpscaleScale,
    pub model: VideoUpscaleModelPreset,
    pub quality: VideoUpscaleQuality,
    pub overwrite: bool,
}

impl Default for VideoUpscaleOptions {
    fn default() -> Self {
        Self {
            scale: VideoUpscaleScale::X4,
            model: VideoUpscaleModelPreset::GeneralFast,
            quality: VideoUpscaleQuality::Q3,
            overwrite: false,
        }
    }
}

impl VideoUpscaleOptions {
    pub fn normalized_for_video_export(mut self) -> Self {
        self.model = VideoUpscaleModelPreset::GeneralFast;
        self
    }
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: i32,
    pub fps_den: i32,
    pub source_time_base: TimeBase,
    pub estimated_frames: Option<u64>,
    pub duration_secs: Option<f64>,
}

impl VideoInfo {
    pub fn output_size(&self, scale: VideoUpscaleScale) -> (u32, u32) {
        (
            self.width.saturating_mul(scale.factor()),
            self.height.saturating_mul(scale.factor()),
        )
    }

    pub fn output_allowed(&self, scale: VideoUpscaleScale) -> bool {
        let (w, h) = self.output_size(scale);
        output_within_mvp_limit(w, h)
    }
}

#[derive(Debug, Clone)]
pub struct VideoUpscalePreflight {
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub info: VideoInfo,
}

impl VideoUpscalePreflight {
    pub fn output_size(&self, scale: VideoUpscaleScale) -> (u32, u32) {
        self.info.output_size(scale)
    }

    pub fn estimate_encode_bytes(
        &self,
        scale: VideoUpscaleScale,
        quality: VideoUpscaleQuality,
    ) -> Option<u64> {
        let seconds = self.info.duration_secs?;
        let (w, h) = self.output_size(scale);
        let pixels = (w as f64 * h as f64).max(1.0);
        let q = match quality {
            VideoUpscaleQuality::Q1 => 1.9,
            VideoUpscaleQuality::Q2 => 1.45,
            VideoUpscaleQuality::Q3 => 1.0,
            VideoUpscaleQuality::Q4 => 0.68,
            VideoUpscaleQuality::Q5 => 0.45,
        };
        let bytes_per_min_at_4k_q3 = 150.0 * 1024.0 * 1024.0;
        let bytes = bytes_per_min_at_4k_q3 * (seconds / 60.0) * (pixels / (3840.0 * 2160.0)) * q;
        Some(bytes.max(1.0) as u64)
    }
}

#[derive(Debug)]
pub struct VideoUpscaleProgressShared {
    pub frames_done: AtomicU64,
    pub frames_total: AtomicU64,
    pub rate_base_frames: AtomicU64,
    pub elapsed_ms: AtomicU64,
}

impl VideoUpscaleProgressShared {
    pub fn new(total: Option<u64>) -> Self {
        Self {
            frames_done: AtomicU64::new(0),
            frames_total: AtomicU64::new(total.unwrap_or(0)),
            rate_base_frames: AtomicU64::new(0),
            elapsed_ms: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> (u64, u64, u64, Duration) {
        (
            self.frames_done.load(Ordering::Relaxed),
            self.frames_total.load(Ordering::Relaxed),
            self.rate_base_frames.load(Ordering::Relaxed),
            Duration::from_millis(self.elapsed_ms.load(Ordering::Relaxed)),
        )
    }
}

#[derive(Debug, Clone)]
pub struct VideoUpscaleJob {
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub info: VideoInfo,
    pub options: VideoUpscaleOptions,
    pub parallel_segments: Arc<AtomicU8>,
    pub pause: Arc<AtomicBool>,
    pub paused_idle: Arc<AtomicBool>,
}

impl VideoUpscaleJob {
    fn current_parallel_segments(&self) -> usize {
        1
    }

    fn pause_requested(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub enum VideoUpscaleMessage {
    PreflightDone(Result<VideoUpscalePreflight, String>),
    Finished(Result<PathBuf, String>),
}

pub fn preflight(source_path: &Path) -> Result<VideoUpscalePreflight, String> {
    crate::video::ffmpeg_loader::init()?;
    ffmpeg::init().map_err(|e| format!("FFmpeg init failed: {e}"))?;
    let info = probe_video_info(source_path)?;
    Ok(VideoUpscalePreflight {
        source_path: source_path.to_path_buf(),
        output_path: derived_video_path_for(source_path),
        sidecar_path: derived_sidecar_path_for(source_path),
        info,
    })
}

pub fn run_job(
    job: VideoUpscaleJob,
    runtime: Arc<AiRuntime>,
    model_manager: Arc<ModelManager>,
    cancel: Arc<AtomicBool>,
    progress: Arc<VideoUpscaleProgressShared>,
) -> Result<PathBuf, String> {
    crate::video::ffmpeg_loader::init()?;
    ffmpeg::init().map_err(|e| format!("FFmpeg init failed: {e}"))?;

    // T05: 前回 run が publish 中にクラッシュ (プロセス強制終了 / 電源断 / OS kill)
    // した場合の orphan state を掃除してから先に進む。残しておくと直後の
    // `output_path.exists() && !overwrite` ガードに引っかかって retry が
    // dead-end する。
    recover_interrupted_publish(&job);

    // T05: クラッシュが publish 完了 (step 3 commit 済み) と queue の mark_done の
    // 間で起きると、pair は healthy なまま task state は Running → 再起動時に
    // Queued へ復帰 → retry がここに到達する。pair が現 job と完全一致するなら
    // 「既に完了済み」として早期 Ok を返し、work_dir をクリーンアップする。
    //
    // overwrite=true の場合は短絡しない: ユーザーが明示的に「上書きしてやり直し」を
    // 選んだ意図を尊重する (出力が壊れている疑いがある等、ユーザーが healthy に見える
    // pair を信じない判断をしうる)。
    if !job.options.overwrite
        && let Some(()) = detect_existing_completed_pair(&job)
    {
        cleanup_work_dir_after_completion(&job);
        crate::logger::log(format!(
            "[VideoUpscale] healthy completed pair detected at run_job entry; treating as done: {}",
            job.output_path.display()
        ));
        return Ok(job.output_path);
    }

    if job.output_path.exists() && !job.options.overwrite {
        return Err(format!(
            "出力ファイルがすでに存在します: {}",
            job.output_path.display()
        ));
    }
    let model_kind = job.options.model.model_kind();
    let model_path = model_manager
        .model_path(model_kind)
        .ok_or_else(|| format!("AIモデルが見つかりません: {}", model_kind.as_str()))?;
    runtime
        .load_model(model_kind, &model_path)
        .map_err(|e| format!("AIモデルの読み込みに失敗しました: {e}"))?;

    let (out_w, out_h) = job.info.output_size(job.options.scale);
    if !output_within_mvp_limit(out_w, out_h) {
        return Err(format!(
            "出力解像度がMVP上限の8K UHDを超えます: {}x{}",
            out_w, out_h
        ));
    }

    // T40 (Codex P2 / 2026-05-16): 出力先のディスク容量を概算して preflight する。
    // 数時間走った末に ENOSPC で死ぬのを避け、UI 上で「容量不足」(= NoSpace) として
    // 即座に表示する。`estimate_required_bytes` は v0.9.0 時点で 1.25x / 1.5x マージン
    // 込み。`has_enough_space` が `Ok(None)` (= 非 Windows / unsupported) なら preflight
    // をスキップしてランタイム検出に頼る。
    {
        let estimate = super::disk::estimate_required_bytes(
            out_w,
            out_h,
            job.info.estimated_frames.unwrap_or(0),
            0.8, // bits-per-pixel-per-frame (= 約 0.1 byte/px、高ビットレート目安)
            false,
        );
        // 出力先の親 dir を query (= ドライブ容量を見る)。final output 側を優先
        let probe_path = job
            .output_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        match super::disk::has_enough_space(&probe_path, estimate.required_bytes) {
            Ok(Some(true)) | Ok(None) => {}
            Ok(Some(false)) => {
                return Err(format!(
                    "ディスク容量が不足しています: 必要 {} MB、ドライブ {} の空き容量が不足",
                    estimate.required_bytes / (1024 * 1024),
                    probe_path.display()
                ));
            }
            Err(e) => {
                crate::logger::log(format!(
                    "[VideoUpscale] disk preflight check failed (continuing): {e}"
                ));
            }
        }
    }

    let part_path = final_part_path_for(&job.source_path);
    let _ = fs::remove_file(&part_path);
    let result =
        run_segmented_video_only(&job, &part_path, runtime.clone(), &cancel, progress.clone());
    if result.is_err() || cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&part_path);
    }
    let finalize_result = result?;

    if cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&part_path);
        return Err("キャンセルされました".to_owned());
    }

    // T05: 最終公開を 3-phase atomic で行う (詳細は `publish_finalized_outputs` の
    // doc-comment 参照):
    //   (1) `save_json_atomic` で `<stem>.miv.json.staged` を書き出し
    //   (2) `fs::rename` で part_path → `<stem>.miv.mkv` を atomic 公開
    //   (3) `fs::rename` で staged → `<stem>.miv.json` を atomic commit
    // `std::fs::rename` は Windows でも Linux でも既存ファイルを atomic 置き換え
    // するので、overwrite モードでも race window を作らない。失敗時のロールバックも
    // ヘルパー内で完結している。
    //
    // 失敗時 publish_result.is_err() で part_path を削除するが、work_dir の Done
    // segment は残るので retry は encode 部分をスキップして publish からやり直せる。
    let publish_result =
        publish_finalized_outputs(&job, &part_path, &finalize_result, model_kind, out_w, out_h);
    if publish_result.is_err() {
        let _ = fs::remove_file(&part_path);
    }
    publish_result?;

    cleanup_work_dir_after_completion(&job);

    Ok(job.output_path)
}

/// T05: 既存の `.miv.mkv` + `.miv.json` ペアが現 `job` の意図と完全一致するなら
/// `Some(())` を返す (= 過去 run が publish 完了済みで、queue だけが mark_done を
/// 取りこぼした状態)。一致条件 (Codex P3 反映で encode 全フィールド追加):
/// - 本編が非空 (`metadata.len() > 0`)
/// - sidecar が schema/JSON 上正しい
/// - sidecar の source identity (file_name + size + head_tail_sha256) が現 source と一致
/// - sidecar の scale / model / output dims / output.path が現 job と一致
/// - sidecar の encode 全フィールド (container/codec/encoder/quality_level/crf/preset/
///   pixel_format) が現 job と一致
///
/// 一致しない場合は通常の `output_path.exists() && !overwrite` ガード経由で fall through。
/// 注: `miv.version` (アプリのバージョン) は意図的に比較しない — アプリ更新だけで
/// 完了済み出力を invalidate したくないため。
fn detect_existing_completed_pair(job: &VideoUpscaleJob) -> Option<()> {
    if !job.sidecar_path.exists() {
        return None;
    }
    // 本編が非空であることを確認 (0 byte の壊れたファイルを「完了済み」扱いしない)
    let video_len = std::fs::metadata(&job.output_path).ok()?.len();
    if video_len == 0 {
        return None;
    }
    let text = std::fs::read_to_string(&job.sidecar_path).ok()?;
    let sidecar: VideoUpscaleSidecar = serde_json::from_str(&text).ok()?;
    if !sidecar.is_valid_for_source(&job.source_path).ok()? {
        return None;
    }
    // upscale 側
    if sidecar.upscale.scale != job.options.scale.factor() {
        return None;
    }
    if sidecar.upscale.model != job.options.model.model_kind().as_str() {
        return None;
    }
    // encode 側 (現状ハードコードだが drift 検出のため全比較)
    if sidecar.encode.quality_level != job.options.quality.level() {
        return None;
    }
    if sidecar.encode.crf != job.options.quality.crf() {
        return None;
    }
    if sidecar.encode.preset != job.options.quality.preset() {
        return None;
    }
    if sidecar.encode.pixel_format != job.options.quality.pixel_format_name() {
        return None;
    }
    if sidecar.encode.container != "mkv" {
        return None;
    }
    if sidecar.encode.video_codec != "av1" {
        return None;
    }
    if sidecar.encode.encoder != "libsvtav1" {
        return None;
    }
    // output 側
    let (expected_w, expected_h) = job.info.output_size(job.options.scale);
    if sidecar.output.width != expected_w || sidecar.output.height != expected_h {
        return None;
    }
    let expected_filename = job.output_path.file_name()?.to_string_lossy().into_owned();
    if sidecar.output.path != expected_filename {
        return None;
    }
    Some(())
}

fn cleanup_work_dir_after_completion(job: &VideoUpscaleJob) {
    let work_dir = work_dir_for(&job.source_path);
    if work_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&work_dir) {
            crate::logger::log(format!(
                "[VideoUpscale] 作業フォルダの削除に失敗しました (継続): {} {e}",
                work_dir.display()
            ));
        }
    }
}

/// T05: `work_dir` が「現 `job` と一致する manifest を持ち、計画上の **全** segment が
/// 実ファイル付きで Done 状態である」かを判定する。`recover_interrupted_publish` が
/// orphan video を削除して良いかの安全条件 (Codex round 6 反映で強化)。
///
/// interrupted publish は必ず「全 segment 完了 → final concat/mux → step 2 video rename」
/// の後に発生する。つまり orphan video が観測されるなら計画上の全 segment が必ず
/// reusable 状態のはず。1 segment だけ Done な partial 状態の work_dir + どこかから
/// 紛れ込んだ無関係 .miv.mkv (例: ユーザーが手動配置) を「interrupted publish」と
/// 誤認して削除しないように、強い条件を要求する。
///
/// 四重チェック:
/// 1. manifest が読めて schema 一致
/// 2. manifest が現 job と意味的に一致 (`validate_manifest_matches_job` と同じ)
/// 3. plan が complete (= 全 segment が確定済み)
/// 4. **計画上の全 segment** が `segment_done_and_reusable` を通過 (= state が Done で
///    実ファイルサイズが manifest 記録と一致)
fn work_dir_has_reusable_segments_for(job: &VideoUpscaleJob) -> bool {
    let work_dir = work_dir_for(&job.source_path);
    if !work_dir.exists() {
        return false;
    }
    let manifest_path = work_dir.join(MANIFEST_FILE_NAME);
    let Ok(manifest) = JobManifest::load(&manifest_path) else {
        return false;
    };
    if !manifest.is_supported_schema() {
        return false;
    }
    if validate_manifest_matches_job(&manifest, job).is_err() {
        return false;
    }
    if !manifest.plan_is_complete() {
        return false;
    }
    let Some(plan) = manifest.plan.as_ref() else {
        return false;
    };
    if plan.segments.is_empty() {
        return false;
    }
    plan.segments
        .iter()
        .all(|planned| segment_done_and_reusable(&manifest, &work_dir, planned.index))
}

/// T05: 前回 run がクラッシュした際の orphan state を掃除する。3-phase publish の
/// 途中 (step 2 完了後・step 3 完了前) でプロセスが kill されると以下の orphan が
/// 残りうる:
/// - `<stem>.miv.json.staged`: 常に orphan (commit に失敗してそのまま残った)
/// - `<stem>.miv.mkv` (sidecar が無い): step 2 完了直後にクラッシュした場合の
///   新本編。元 pair が無かった初回出力ケースだけが該当する (overwrite モードでは
///   旧 sidecar が残っているはずなので mismatched pair として next retry が
///   handle する)
///
/// 後者は `work_dir` がまだあれば segment 再利用で速やかに再 publish できるので
/// 削除して clean state にする。`work_dir` が既にない場合 (ユーザーが手動掃除した
/// 等) は触らない — そのケースで本編を消すと数時間分の作業が消えるため、ユーザー
/// 判断に委ねる。
fn recover_interrupted_publish(job: &VideoUpscaleJob) {
    // .json.staged は常に orphan (publish 中の中間ファイルで、最終配置先ではない)
    let staged = job.sidecar_path.with_extension("json.staged");
    if staged.exists() {
        match fs::remove_file(&staged) {
            Ok(_) => crate::logger::log(format!(
                "[VideoUpscale] removed orphan staged sidecar from interrupted publish: {}",
                staged.display()
            )),
            Err(e) => crate::logger::log(format!(
                "[VideoUpscale] failed to remove orphan staged sidecar {}: {e}",
                staged.display()
            )),
        }
    }

    // 本編はあるが sidecar が無い → 初回出力 publish 中クラッシュの可能性。
    // 削除は **manifest が現 job と一致し、かつ再利用可能な Done segment
    // (実ファイルサイズが manifest 記録と一致するもの) が 1 つ以上ある** 場合のみ。
    // そうでなければ数時間分の encode を消すリスクがあるため、ログのみ出して
    // ユーザー判断に委ねる。
    if job.output_path.exists()
        && !job.sidecar_path.exists()
        && work_dir_has_reusable_segments_for(job)
    {
        match fs::remove_file(&job.output_path) {
            Ok(_) => crate::logger::log(format!(
                "[VideoUpscale] removed orphan video from interrupted publish (validated Done segments let next run re-publish quickly): {}",
                job.output_path.display()
            )),
            Err(e) => crate::logger::log(format!(
                "[VideoUpscale] failed to remove orphan video {}: {e}",
                job.output_path.display()
            )),
        }
    } else if job.output_path.exists() && !job.sidecar_path.exists() {
        crate::logger::log(format!(
            "[VideoUpscale] orphan video at {} without validated reusable segments; preserving for user inspection (manual sidecar regeneration or delete required to retry)",
            job.output_path.display()
        ));
    }
}

/// run_job の最終フェーズ: 新 sidecar を staging → 本編 rename (atomic publish) →
/// staged sidecar を commit (atomic replace)、の 3 段。`std::fs::rename` は Windows
/// でも Linux でも既存ファイルを atomic に置き換えるため、旧 pair を明示削除せずに
/// 上書き可能。旧 sidecar は step 3 完了まで自然に visible で居続けるので、
/// 仮にプロセスが途中でクラッシュしても sidecar 不可視窓は発生しない。
///
/// 旧設計 (v0.8 以前) は (1) 本編 rename → (2) 非 atomic な sidecar write の順で、
/// 中間でクラッシュすると本編が公開済み + sidecar 欠落の orphan ペアを作っていた
/// (overwrite=false で retry が `出力ファイルがすでに存在します` ガードで止まる
/// dead-end になる)。T05 で sidecar 先行 → atomic に変更したのが本関数。
///
/// 失敗モードとロールバック (Codex P2 反映):
/// - **staging 失敗**: 何も触らずに `Err`。
/// - **本編 rename 失敗**: staged を削除して `Err`。旧 pair は無傷。
/// - **sidecar commit 失敗**:
///   - 旧 pair が存在した場合 → 本編は新しくなったが旧 sidecar は自然に visible のまま
///     残るので「新本編 + 旧 sidecar」mismatched pair になる (sidecar dims が古い)。
///     次回再実行で全体が atomic に再構築されるので self-healing。
///   - 旧 pair が存在しなかった場合 (初回出力) → 新本編を `.part` に戻してロールバック。
///     これをしないと retry が `output_path.exists() && !overwrite` ガードで止まる
///     dead-end になる。
fn publish_finalized_outputs(
    job: &VideoUpscaleJob,
    part_path: &Path,
    finalize_result: &FinalizeResult,
    model_kind: ModelKind,
    out_w: u32,
    out_h: u32,
) -> Result<(), String> {
    let source = source_info_for(&job.source_path)
        .map_err(|e| format!("元動画の情報を取得できません: {e}"))?;
    let sidecar = VideoUpscaleSidecar::new(
        source,
        UpscaleInfo {
            scale: job.options.scale.factor(),
            model: model_kind.as_str().to_owned(),
        },
        EncodeInfo {
            container: "mkv".to_owned(),
            video_codec: "av1".to_owned(),
            encoder: "libsvtav1".to_owned(),
            quality_level: job.options.quality.level(),
            crf: job.options.quality.crf(),
            preset: job.options.quality.preset(),
            pixel_format: job.options.quality.pixel_format_name().to_owned(),
            audio: finalize_result.audio_sidecar_value.to_owned(),
        },
        OutputInfo {
            path: job
                .output_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| job.output_path.to_string_lossy().into_owned()),
            width: out_w,
            height: out_h,
        },
    );

    // Step 1: 新 sidecar を staging パスへ書き込む (atomic、まだユーザー可視の場所にはない)。
    let staged_sidecar_path = job.sidecar_path.with_extension("json.staged");
    save_json_atomic(&staged_sidecar_path, &sidecar)
        .map_err(|e| format!("sidecar JSONの保存に失敗しました: {e}"))?;

    // 旧本編の有無を覚えておく (step 3 失敗時に本編をロールバックするか判断するため)。
    // 旧本編なし → 新本編は孤立すると retry guard を dead-end させるので rollback。
    // 旧本編あり → overwrite=true 経由でしか到達できないので retry も overwrite=true で
    // 走り直しできるため rollback 不要 (新本編 + 旧 sidecar の mismatched pair で残す)。
    let had_old_video = job.output_path.exists();

    // Step 2: 本編を atomic rename で公開 (Windows/Linux とも MOVEFILE_REPLACE_EXISTING
    // 相当で既存を atomic 置換)。
    if let Err(e) = fs::rename(part_path, &job.output_path) {
        let _ = fs::remove_file(&staged_sidecar_path);
        return Err(format!("出力ファイルの確定に失敗しました: {e}"));
    }

    // Step 3: 新 sidecar を commit (staged → final、atomic replace)。
    if let Err(e) = fs::rename(&staged_sidecar_path, &job.sidecar_path) {
        let _ = fs::remove_file(&staged_sidecar_path);
        // 旧本編が無かった (初回出力 or 旧 sidecar のみ残った異常状態) なら新本編を
        // `.part` に戻してロールバック。これをしないと output_path に新本編が孤立し、
        // 次回 retry が `output_path.exists() && !overwrite` ガードに弾かれて dead-end する。
        // 旧本編があった場合は overwrite モード必須 (start of run_job のガード経由) なので
        // retry も overwrite=true で来るため dead-end しない → 新本編を残して
        // 旧 sidecar (atomic replace 未実施なのでまだ visible) との mismatched pair に。
        if !had_old_video {
            if let Err(rollback_err) = fs::rename(&job.output_path, part_path) {
                crate::logger::log(format!(
                    "[VideoUpscale] sidecar commit + video rollback failed: \
                     sidecar={e}, rollback={rollback_err}"
                ));
            }
        }
        return Err(format!("sidecar JSONのコミットに失敗しました: {e}"));
    }

    Ok(())
}

fn probe_video_info(path: &Path) -> Result<VideoInfo, String> {
    let ictx = ffmpeg::format::input(path).map_err(|e| format!("動画を開けません: {e}"))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "動画ストリームが見つかりません".to_owned())?;
    let params = stream.parameters();
    let width = params.width();
    let height = params.height();
    if width == 0 || height == 0 {
        return Err("動画の解像度を取得できません".to_owned());
    }

    let fps = sane_rate(stream.avg_frame_rate()).or_else(|| sane_rate(stream.rate()));
    let (fps_num, fps_den) = fps.unwrap_or((30, 1));
    let frames = if stream.frames() > 0 {
        Some(stream.frames() as u64)
    } else {
        None
    };
    let duration_secs = duration_seconds(stream.duration(), stream.time_base());
    let estimated_frames = frames.or_else(|| {
        duration_secs.map(|secs| (secs * fps_num as f64 / fps_den as f64).round().max(1.0) as u64)
    });

    Ok(VideoInfo {
        width,
        height,
        fps_num,
        fps_den,
        source_time_base: time_base_from_rational(stream.time_base()),
        estimated_frames,
        duration_secs,
    })
}

struct SegmentEncodeResult {
    frame_count: u64,
    total_pts_ticks: i64,
    output_time_base: TimeBase,
    source_start_pts: i64,
    source_last_pts: i64,
}

struct FinalizeResult {
    audio_sidecar_value: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyframePoint {
    frame: u64,
    pts: i64,
}

#[derive(Clone)]
enum SegmentProgressMode {
    Sequential {
        prior_done_frames: u64,
    },
    Parallel {
        committed_frames: Arc<AtomicU64>,
        in_flight_frames: Arc<Vec<AtomicU64>>,
        started: Instant,
        slot: usize,
    },
}

impl SegmentProgressMode {
    fn base_frames(&self) -> u64 {
        match self {
            Self::Sequential { prior_done_frames } => *prior_done_frames,
            Self::Parallel {
                committed_frames, ..
            } => committed_frames.load(Ordering::Relaxed),
        }
    }

    fn update(&self, progress: &VideoUpscaleProgressShared, segment_frames: u64) {
        match self {
            Self::Sequential { prior_done_frames } => {
                progress.frames_done.store(
                    prior_done_frames.saturating_add(segment_frames),
                    Ordering::Relaxed,
                );
            }
            Self::Parallel {
                committed_frames,
                in_flight_frames,
                started,
                slot,
            } => {
                if let Some(current) = in_flight_frames.get(*slot) {
                    current.store(segment_frames, Ordering::Relaxed);
                }
                progress.frames_done.store(
                    committed_frames
                        .load(Ordering::Relaxed)
                        .saturating_add(sum_atomic_u64(in_flight_frames.as_slice())),
                    Ordering::Relaxed,
                );
                progress
                    .elapsed_ms
                    .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
        }
    }
}

fn sum_atomic_u64(values: &[AtomicU64]) -> u64 {
    values
        .iter()
        .map(|value| value.load(Ordering::Relaxed))
        .sum()
}

#[derive(Clone, Copy)]
struct StreamCopyMap {
    input_index: usize,
    output_index: usize,
    input_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
}

struct SegmentVideoCursor {
    segments: Vec<SegmentEntry>,
    work_dir: PathBuf,
    output_stream_index: usize,
    output_time_base: ffmpeg::Rational,
    next_segment: usize,
    current: Option<SegmentVideoInput>,
    cumulative_offset: i64,
}

struct SegmentVideoInput {
    segment: SegmentEntry,
    input: ffmpeg::format::context::Input,
    input_stream_index: usize,
    input_time_base: ffmpeg::Rational,
}

impl SegmentVideoCursor {
    fn new(
        manifest: &JobManifest,
        work_dir: &Path,
        output_stream_index: usize,
        output_time_base: ffmpeg::Rational,
    ) -> Self {
        Self {
            segments: sorted_done_segments(manifest)
                .into_iter()
                .cloned()
                .collect(),
            work_dir: work_dir.to_path_buf(),
            output_stream_index,
            output_time_base,
            next_segment: 0,
            current: None,
            cumulative_offset: 0,
        }
    }

    fn next_packet(&mut self) -> Result<Option<ffmpeg::Packet>, String> {
        loop {
            if self.current.is_none() {
                let Some(segment) = self.segments.get(self.next_segment).cloned() else {
                    return Ok(None);
                };
                self.next_segment += 1;
                let path = self.work_dir.join(&segment.path);
                let input = ffmpeg::format::input(&path)
                    .map_err(|e| format!("failed to open segment: {e}"))?;
                let input_stream = input
                    .streams()
                    .best(ffmpeg::media::Type::Video)
                    .ok_or_else(|| "segment has no video stream".to_owned())?;
                self.current = Some(SegmentVideoInput {
                    segment,
                    input_stream_index: input_stream.index(),
                    input_time_base: input_stream.time_base(),
                    input,
                });
            }

            let current = self.current.as_mut().expect("current segment input exists");
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut current.input) {
                Ok(()) => {
                    if packet.stream() != current.input_stream_index {
                        continue;
                    }
                    packet.rescale_ts(current.input_time_base, self.output_time_base);
                    packet.set_pts(packet.pts().map(|pts| pts + self.cumulative_offset));
                    packet.set_dts(packet.dts().map(|dts| dts + self.cumulative_offset));
                    packet.set_stream(self.output_stream_index);
                    return Ok(Some(packet));
                }
                Err(ffmpeg::Error::Eof) => {
                    let segment = &current.segment;
                    self.cumulative_offset += rescale_ticks(
                        segment.output_total_pts_ticks,
                        segment.output_time_base,
                        time_base_from_rational(self.output_time_base),
                    );
                    self.current = None;
                }
                Err(e) => return Err(format!("failed to read segment packet: {e}")),
            }
        }
    }
}

fn run_segmented_video_only(
    job: &VideoUpscaleJob,
    final_part_path: &Path,
    runtime: Arc<AiRuntime>,
    cancel: &Arc<AtomicBool>,
    progress: Arc<VideoUpscaleProgressShared>,
) -> Result<FinalizeResult, String> {
    let work_dir = work_dir_for(&job.source_path);
    let segments_dir = segments_dir_for(&work_dir);
    fs::create_dir_all(&segments_dir)
        .map_err(|e| format!("segment作業フォルダを作成できません: {e}"))?;

    let manifest_path = manifest_path_for(&job.source_path);
    let (mut manifest, was_loaded_from_disk) = if manifest_path.exists() {
        let m = JobManifest::load(&manifest_path)
            .map_err(|e| format!("failed to load segment manifest: {e}"))?;
        (m, true)
    } else {
        (create_initial_manifest(job)?, false)
    };
    if !manifest.is_supported_schema() {
        return Err("unsupported segment manifest schema".to_owned());
    }
    // T04: 既存 manifest を再開する前に、現在の job (source / 出力サイズ / オプション)
    // と一致しているかを必ず検証する。同名で内容が異なるソースに差し替えた、もしくは
    // 解像度・品質・モデルを変えて retry した場合に、旧 segment と新 segment を
    // 無言で concat して破損動画を作るのを防ぐ。新規 manifest (just created from job)
    // は定義上一致するので validation はスキップして余分な hash 計算を避ける。
    if was_loaded_from_disk {
        validate_manifest_matches_job(&manifest, job)?;
        // T38: クラッシュ / process kill 後に Running 状態で残った segment を Pending
        // に戻し、`.part.<worker_id>` orphan ファイルも掃除する。再起動した本プロセスの
        // pid とは別の owner pid を「死亡」とみなす (= pid 再利用は started_ms で
        // 判別しない簡易判定。後発 process が同じ pid を引いた場合でも自分の管理下に
        // ない segment は Pending 化して再 spawn する方が安全)。
        let current_pid = std::process::id();
        let n_reset = manifest.reset_stale_running_segments(|pid, _started_ms| pid == current_pid);
        if n_reset > 0 {
            crate::logger::log(format!(
                "[VideoUpscale] reset {n_reset} stale Running segment(s) after process restart"
            ));
            for seg in manifest
                .segments
                .iter()
                .filter(|s| s.state == SegmentState::Pending)
            {
                cleanup_segment_parts(&work_dir, seg.index);
            }
            manifest
                .save_atomic(&manifest_path)
                .map_err(|e| format!("failed to save segment manifest: {e}"))?;
        }
    }
    ensure_plan(job, &mut manifest, cancel)?;
    manifest
        .save_atomic(&manifest_path)
        .map_err(|e| format!("failed to save segment manifest: {e}"))?;

    let planned_segments = manifest
        .plan
        .as_ref()
        .map(|plan| plan.segments.clone())
        .unwrap_or_default();
    if planned_segments.is_empty() {
        return Err("segment plan is empty".to_owned());
    }

    let completed_before = manifest
        .segments
        .iter()
        .filter(|segment| segment.state == SegmentState::Done)
        .map(|segment| segment.output_frame_count)
        .sum::<u64>();
    progress
        .frames_done
        .store(completed_before, Ordering::Relaxed);
    progress
        .rate_base_frames
        .store(completed_before, Ordering::Relaxed);
    progress.elapsed_ms.store(0, Ordering::Relaxed);

    if planned_segments.len() > 1 {
        run_segments_parallel(
            job,
            &manifest_path,
            &mut manifest,
            &planned_segments,
            &work_dir,
            runtime,
            cancel,
            progress.clone(),
            completed_before,
        )?;
        return finalize_segments(job, &manifest, &work_dir, final_part_path, cancel);
    }

    for planned in &planned_segments {
        if cancel.load(Ordering::Relaxed) {
            return Err("canceled".to_owned());
        }
        if segment_done_and_reusable(&manifest, &work_dir, planned.index) {
            continue;
        }

        let seg_part = segment_part_path(&work_dir, planned.index);
        let seg_final = segment_path(&work_dir, planned.index);
        let _ = fs::remove_file(&seg_part);
        let _ = fs::remove_file(&seg_final);

        let prior_done = manifest
            .segments
            .iter()
            .filter(|segment| segment.state == SegmentState::Done)
            .map(|segment| segment.output_frame_count)
            .sum::<u64>();
        let result = encode_video_segment(
            job,
            planned,
            &seg_part,
            runtime.as_ref(),
            cancel,
            progress.as_ref(),
            SegmentProgressMode::Sequential {
                prior_done_frames: prior_done,
            },
        );
        if result.is_err() {
            let _ = fs::remove_file(&seg_part);
        }
        let encoded = result?;
        if cancel.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&seg_part);
            return Err("キャンセルされました".to_owned());
        }
        // T39 (Codex P2 / 2026-05-16): 0 フレームのセグメントは Done として publish しない。
        // FFmpeg seek 後に対象範囲を超えるなどで encode 0 frame の "header だけ MKV" が
        // `validate_segment_file` (= len()>0 のみ) を通過し、永久 reuse される事故を防ぐ。
        if encoded.frame_count == 0 {
            let _ = fs::remove_file(&seg_part);
            return Err(format!(
                "segment {} produced 0 frames (FFmpeg seek likely landed past the segment range)",
                planned.index
            ));
        }
        validate_segment_file(&seg_part)?;

        let planned_count = planned
            .target_end_frame_exclusive
            .saturating_sub(planned.target_start_frame);
        if planned.target_end_frame_exclusive != u64::MAX && encoded.frame_count != planned_count {
            return Err(format!(
                "segment plan drift: planned {} frames, encoded {} frames",
                planned_count, encoded.frame_count
            ));
        }

        fs::rename(&seg_part, &seg_final)
            .map_err(|e| format!("failed to publish segment file: {e}"))?;
        let metadata = fs::metadata(&seg_final)
            .map_err(|e| format!("failed to read segment metadata: {e}"))?;
        upsert_segment_entry(
            &mut manifest,
            SegmentEntry {
                index: planned.index,
                path: PathBuf::from(format!(
                    "segments/{}",
                    segment_path_file_name(planned.index)
                )),
                state: SegmentState::Done,
                output_frame_start: planned.target_start_frame,
                output_frame_count: encoded.frame_count,
                output_total_pts_ticks: encoded.total_pts_ticks,
                output_time_base: encoded.output_time_base,
                source_start_pts: encoded.source_start_pts,
                source_last_pts: encoded.source_last_pts,
                size: metadata.len(),
                mtime_unix_ms: file_mtime_unix_ms(&metadata),
                worker_id: None,
                worker_pid: None,
                worker_started_unix_ms: None,
            },
        );
        manifest.progress.completed_frames = manifest
            .segments
            .iter()
            .filter(|segment| segment.state == SegmentState::Done)
            .map(|segment| segment.output_frame_count)
            .sum();
        manifest.progress.next_output_frame_index = manifest.progress.completed_frames;
        manifest
            .save_atomic(&manifest_path)
            .map_err(|e| format!("failed to save segment manifest: {e}"))?;
    }

    finalize_segments(job, &manifest, &work_dir, final_part_path, cancel)
}

struct ParallelSegmentMessage {
    slot: usize,
    planned: PlannedSegment,
    part_path: PathBuf,
    result: Result<SegmentEncodeResult, String>,
}

#[allow(clippy::too_many_arguments)]
fn run_segments_parallel(
    job: &VideoUpscaleJob,
    manifest_path: &Path,
    manifest: &mut JobManifest,
    planned_segments: &[PlannedSegment],
    work_dir: &Path,
    runtime: Arc<AiRuntime>,
    cancel: &Arc<AtomicBool>,
    progress: Arc<VideoUpscaleProgressShared>,
    completed_before: u64,
) -> Result<(), String> {
    let mut pending: std::collections::VecDeque<PlannedSegment> = planned_segments
        .iter()
        .filter(|planned| !segment_done_and_reusable(manifest, work_dir, planned.index))
        .cloned()
        .collect();
    if pending.is_empty() {
        return Ok(());
    }

    let worker_slots = 5_usize.min(pending.len().max(1));
    let committed_frames = Arc::new(AtomicU64::new(completed_before));
    let in_flight_frames = Arc::new(
        (0..worker_slots)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>(),
    );
    let (tx, rx) = mpsc::channel::<ParallelSegmentMessage>();
    let started = Instant::now();
    let mut active = 0_usize;
    let mut slot_active = vec![false; worker_slots];
    let mut first_error: Option<String> = None;
    job.paused_idle.store(false, Ordering::Relaxed);

    while active > 0 || (!pending.is_empty() && first_error.is_none()) {
        if active == 0 && job.pause_requested() && first_error.is_none() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            job.paused_idle.store(true, Ordering::Relaxed);
            progress
                .elapsed_ms
                .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        job.paused_idle.store(false, Ordering::Relaxed);

        let desired_workers = if job.pause_requested() {
            active
        } else {
            job.current_parallel_segments()
                .min(worker_slots)
                .min(active.saturating_add(pending.len()))
                .max(1)
        };
        while active < desired_workers
            && first_error.is_none()
            && !cancel.load(Ordering::Relaxed)
            && !job.pause_requested()
            && !pending.is_empty()
        {
            let planned = pending.pop_front().expect("pending segment exists");
            if segment_done_and_reusable(manifest, work_dir, planned.index) {
                continue;
            }
            let Some(slot) = slot_active.iter().position(|active| !*active) else {
                pending.push_front(planned);
                break;
            };
            slot_active[slot] = true;
            in_flight_frames[slot].store(0, Ordering::Relaxed);

            let worker_id = format!("{}-{slot}-{}", std::process::id(), planned.index);
            cleanup_segment_parts(work_dir, planned.index);
            let _ = fs::remove_file(segment_path(work_dir, planned.index));
            let part_path = worker_segment_part_path(work_dir, planned.index, &worker_id);

            upsert_segment_entry(
                manifest,
                SegmentEntry {
                    index: planned.index,
                    path: part_path
                        .strip_prefix(work_dir)
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| part_path.clone()),
                    state: SegmentState::Running,
                    output_frame_start: planned.target_start_frame,
                    output_frame_count: 0,
                    output_total_pts_ticks: 0,
                    output_time_base: TimeBase::new(job.info.fps_den, job.info.fps_num),
                    source_start_pts: planned.target_start_pts,
                    source_last_pts: planned.target_start_pts,
                    size: 0,
                    mtime_unix_ms: 0,
                    worker_id: Some(worker_id),
                    worker_pid: Some(std::process::id()),
                    worker_started_unix_ms: Some(super::manifest::now_unix_ms()),
                },
            );
            manifest
                .save_atomic(manifest_path)
                .map_err(|e| format!("failed to save segment manifest: {e}"))?;

            let worker_job = job.clone();
            let worker_runtime = runtime.clone();
            let worker_cancel = cancel.clone();
            let worker_progress = progress.clone();
            let worker_committed = committed_frames.clone();
            let worker_in_flight = in_flight_frames.clone();
            let worker_tx = tx.clone();
            let worker_planned = planned.clone();
            let worker_part = part_path.clone();
            thread::spawn(move || {
                let progress_mode = SegmentProgressMode::Parallel {
                    committed_frames: worker_committed,
                    in_flight_frames: worker_in_flight,
                    started,
                    slot,
                };
                let result = encode_video_segment(
                    &worker_job,
                    &worker_planned,
                    &worker_part,
                    worker_runtime.as_ref(),
                    &worker_cancel,
                    worker_progress.as_ref(),
                    progress_mode,
                )
                .and_then(|encoded| {
                    // T39: 0 フレームは Done に上げない (`validate_segment_file` は
                    // len()>0 だけ見るので header だけの空 MKV を弾けない)
                    if encoded.frame_count == 0 {
                        let _ = fs::remove_file(&worker_part);
                        return Err(format!(
                            "segment {} produced 0 frames (FFmpeg seek likely landed past the segment range)",
                            worker_planned.index
                        ));
                    }
                    let planned_count = worker_planned
                        .target_end_frame_exclusive
                        .saturating_sub(worker_planned.target_start_frame);
                    if worker_planned.target_end_frame_exclusive != u64::MAX
                        && encoded.frame_count != planned_count
                    {
                        return Err(format!(
                            "segment plan drift: planned {} frames, encoded {} frames",
                            planned_count, encoded.frame_count
                        ));
                    }
                    validate_segment_file(&worker_part)?;
                    Ok(encoded)
                });
                let _ = worker_tx.send(ParallelSegmentMessage {
                    slot,
                    planned: worker_planned,
                    part_path: worker_part,
                    result,
                });
            });
            active += 1;
        }

        if active == 0 {
            break;
        }

        let message = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                progress
                    .elapsed_ms
                    .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("segment worker channel closed".to_owned());
            }
        };
        active = active.saturating_sub(1);
        if let Some(slot) = slot_active.get_mut(message.slot) {
            *slot = false;
        }
        in_flight_frames[message.slot].store(0, Ordering::Relaxed);

        match message.result {
            Ok(encoded) if first_error.is_none() && !cancel.load(Ordering::Relaxed) => {
                let publish_result = (|| -> Result<(), String> {
                    let final_path = segment_path(work_dir, message.planned.index);
                    fs::rename(&message.part_path, &final_path)
                        .map_err(|e| format!("failed to publish segment file: {e}"))?;
                    let metadata = fs::metadata(&final_path)
                        .map_err(|e| format!("failed to read segment metadata: {e}"))?;
                    upsert_segment_entry(
                        manifest,
                        SegmentEntry {
                            index: message.planned.index,
                            path: PathBuf::from(format!(
                                "segments/{}",
                                segment_path_file_name(message.planned.index)
                            )),
                            state: SegmentState::Done,
                            output_frame_start: message.planned.target_start_frame,
                            output_frame_count: encoded.frame_count,
                            output_total_pts_ticks: encoded.total_pts_ticks,
                            output_time_base: encoded.output_time_base,
                            source_start_pts: encoded.source_start_pts,
                            source_last_pts: encoded.source_last_pts,
                            size: metadata.len(),
                            mtime_unix_ms: file_mtime_unix_ms(&metadata),
                            worker_id: None,
                            worker_pid: None,
                            worker_started_unix_ms: None,
                        },
                    );
                    let committed = committed_frames
                        .fetch_add(encoded.frame_count, Ordering::Relaxed)
                        .saturating_add(encoded.frame_count);
                    progress.frames_done.store(
                        committed.saturating_add(sum_atomic_u64(in_flight_frames.as_slice())),
                        Ordering::Relaxed,
                    );
                    progress
                        .elapsed_ms
                        .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    manifest.progress.completed_frames = manifest
                        .segments
                        .iter()
                        .filter(|segment| segment.state == SegmentState::Done)
                        .map(|segment| segment.output_frame_count)
                        .sum();
                    manifest.progress.next_output_frame_index = manifest.progress.completed_frames;
                    manifest
                        .save_atomic(manifest_path)
                        .map_err(|e| format!("failed to save segment manifest: {e}"))?;
                    Ok(())
                })();
                if let Err(err) = publish_result {
                    let _ = fs::remove_file(&message.part_path);
                    if first_error.is_none() {
                        first_error = Some(err);
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
            Ok(_) => {
                let _ = fs::remove_file(&message.part_path);
            }
            Err(err) => {
                let _ = fs::remove_file(&message.part_path);
                if first_error.is_none() {
                    first_error = Some(err);
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(first_error.unwrap_or_else(|| "canceled".to_owned()));
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(())
}

/// T04 (v0.9.0): 既存 manifest を再開する前に、`job` が表す現在の入力/出力/オプションと
/// 一致しているかを検証する。一致しなければ `Err` を返す。エラー文字列は
/// `ui_dialogs::video_upscale::failure_reason_from_error_text` が `StaleSource` /
/// `PlanDrift` に分類できるキーワード (`"stale source"` / `"segment plan drift"`) を含む。
///
/// 検証項目:
/// - **Source identity**: `file_name` / `size` / `head_tail_sha256` / `time_base` を現在の
///   ファイルと比較。`mtime_unix_ms` はクラウド同期ツールが本体を変えずに書き換える
///   ことがあるため意図的に**比較しない** (sidecar.rs と同じ方針)。
/// - **Output dims**: manifest 記録の width × height が現 `job.info.output_size(scale)`
///   と一致するか。
/// - **Options**: scale / model / quality_level / container / video_codec / encoder の
///   すべてを比較。container/video_codec/encoder は現状ハードコードだが、比較するだけ
///   なので将来の drift にも追従できる。
///
/// 失敗時、呼び出し側 (`run_segmented_video_only`) はそのまま `Err` を伝搬し、UI 側で
/// 失敗タスクとして表示される。ユーザーは Cancel (work_dir 削除) → 再登録で
/// 進められる。本関数は work_dir を自動削除しない (= false positive で数時間分の
/// 完了 segment を失う事故を避けるため、ユーザー明示判断を必須にする)。
fn validate_manifest_matches_job(
    manifest: &JobManifest,
    job: &VideoUpscaleJob,
) -> Result<(), String> {
    let current = source_info_for(&job.source_path)
        .map_err(|e| format!("stale source: failed to read source info: {e}"))?;
    if manifest.source.file_name != current.file_name {
        return Err(format!(
            "stale source: file name changed (manifest={}, current={})",
            manifest.source.file_name, current.file_name
        ));
    }
    if manifest.source.size != current.size {
        return Err(format!(
            "stale source: size changed (manifest={}B, current={}B)",
            manifest.source.size, current.size
        ));
    }
    if manifest.source.head_tail_sha256 != current.head_tail_sha256 {
        return Err("stale source: head/tail content hash mismatch".to_owned());
    }
    if manifest.source.time_base != job.info.source_time_base {
        return Err("segment plan drift: source time base changed".to_owned());
    }

    let (expected_w, expected_h) = job.info.output_size(job.options.scale);
    if manifest.output.width != expected_w || manifest.output.height != expected_h {
        return Err(format!(
            "segment plan drift: output dimensions changed (manifest={}x{}, current={}x{})",
            manifest.output.width, manifest.output.height, expected_w, expected_h
        ));
    }

    let expected_scale = job.options.scale.factor();
    if manifest.options.scale != expected_scale {
        return Err(format!(
            "segment plan drift: scale changed (manifest={}, current={})",
            manifest.options.scale, expected_scale
        ));
    }
    let expected_model = job.options.model.model_kind().as_str();
    if manifest.options.model != expected_model {
        return Err(format!(
            "segment plan drift: model changed (manifest={}, current={})",
            manifest.options.model, expected_model
        ));
    }
    let expected_quality = job.options.quality.level();
    if manifest.options.quality_level != expected_quality {
        return Err(format!(
            "segment plan drift: quality level changed (manifest={}, current={})",
            manifest.options.quality_level, expected_quality
        ));
    }
    if manifest.options.container != "mkv" {
        return Err(format!(
            "segment plan drift: container changed (manifest={}, current=mkv)",
            manifest.options.container
        ));
    }
    if manifest.options.video_codec != "av1" {
        return Err(format!(
            "segment plan drift: video codec changed (manifest={}, current=av1)",
            manifest.options.video_codec
        ));
    }
    if manifest.options.encoder != "libsvtav1" {
        return Err(format!(
            "segment plan drift: encoder changed (manifest={}, current=libsvtav1)",
            manifest.options.encoder
        ));
    }

    Ok(())
}

fn create_initial_manifest(job: &VideoUpscaleJob) -> Result<JobManifest, String> {
    let source = source_info_for(&job.source_path)
        .map_err(|e| format!("failed to read source info for manifest: {e}"))?;
    let (out_w, out_h) = job.info.output_size(job.options.scale);
    Ok(JobManifest::new(
        uuid::Uuid::new_v4(),
        ManifestSource {
            file_name: source.file_name,
            size: source.size,
            mtime_unix_ms: source.mtime_unix_ms,
            head_tail_sha256: source.head_tail_sha256,
            time_base: job.info.source_time_base,
        },
        ManifestOutput {
            final_path: job
                .output_path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("video.miv.mkv")),
            sidecar_path: job
                .sidecar_path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("video.miv.json")),
            width: out_w,
            height: out_h,
        },
        ManifestOptions {
            scale: job.options.scale.factor(),
            model: job.options.model.model_kind().as_str().to_owned(),
            quality_level: job.options.quality.level(),
            container: "mkv".to_owned(),
            video_codec: "av1".to_owned(),
            encoder: "libsvtav1".to_owned(),
        },
        job.info.estimated_frames.unwrap_or(0),
    ))
}

fn ensure_plan(
    job: &VideoUpscaleJob,
    manifest: &mut JobManifest,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    if manifest.plan_is_complete() {
        return Ok(());
    }
    if let Some(segments) = build_keyframe_snap_plan(job, cancel)? {
        manifest.plan = Some(SegmentPlan {
            strategy: SegmentPlanStrategy::SourceKeyframeSnap,
            state: SegmentPlanState::Complete,
            scan_progress_pts: segments.last().map(|segment| segment.target_end_pts),
            segments,
        });
        return Ok(());
    }
    let total_frames = job.info.estimated_frames.unwrap_or(0);
    let frames_per_segment = segment_frames_for_fps(job.info.fps_num, job.info.fps_den);
    let mut segments = Vec::new();
    if total_frames == 0 {
        segments.push(planned_segment_for_frames(job, 0, 0, u64::MAX));
    } else {
        let mut start = 0;
        let mut index = 0;
        while start < total_frames {
            let end = (start + frames_per_segment).min(total_frames);
            segments.push(planned_segment_for_frames(job, index, start, end));
            start = end;
            index += 1;
        }
    }
    manifest.plan = Some(SegmentPlan {
        strategy: SegmentPlanStrategy::TimeBased,
        state: SegmentPlanState::Complete,
        scan_progress_pts: None,
        segments,
    });
    Ok(())
}

fn build_keyframe_snap_plan(
    job: &VideoUpscaleJob,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<Vec<PlannedSegment>>, String> {
    let scan = match scan_source_keyframes(job, cancel) {
        Ok(scan) => scan,
        Err(err) => {
            crate::logger::log(format!(
                "[VideoUpscale] keyframe scan failed; using time-based plan: {err}"
            ));
            return Ok(None);
        }
    };
    if scan.total_frames == 0 {
        return Ok(None);
    }
    Ok(plan_segments_from_keyframes(
        job,
        scan.total_frames,
        &scan.keyframes,
    ))
}

struct KeyframeScan {
    total_frames: u64,
    keyframes: Vec<KeyframePoint>,
}

fn scan_source_keyframes(
    job: &VideoUpscaleJob,
    cancel: &Arc<AtomicBool>,
) -> Result<KeyframeScan, String> {
    let mut input = ffmpeg::format::input(&job.source_path)
        .map_err(|e| format!("failed to open source for keyframe scan: {e}"))?;
    let video_stream_index = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "source has no video stream for keyframe scan".to_owned())?
        .index();
    let mut frame = 0_u64;
    let mut keyframes = Vec::new();

    for packet_result in input.packets() {
        if frame % 1000 == 0 && cancel.load(Ordering::Relaxed) {
            return Err("canceled".to_owned());
        }
        let (stream, packet) =
            packet_result.map_err(|e| format!("failed to read source packet for plan: {e}"))?;
        if stream.index() != video_stream_index {
            continue;
        }
        // We only persist keyframe timestamps; for keyframes PTS and DTS are
        // equivalent for the seek origin we need, while DTS is a useful
        // fallback for containers with sparse PTS metadata.
        let pts = packet
            .pts()
            .or_else(|| packet.dts())
            .unwrap_or_else(|| frame_to_pts(job, frame));
        if packet.is_key() {
            keyframes.push(KeyframePoint { frame, pts });
        }
        frame = frame.saturating_add(1);
    }

    if keyframes.first().is_none_or(|keyframe| keyframe.frame != 0) {
        keyframes.insert(
            0,
            KeyframePoint {
                frame: 0,
                pts: frame_to_pts(job, 0),
            },
        );
    }
    keyframes.sort_by_key(|keyframe| keyframe.frame);
    keyframes.dedup_by_key(|keyframe| keyframe.frame);

    Ok(KeyframeScan {
        total_frames: frame,
        keyframes,
    })
}

fn plan_segments_from_keyframes(
    job: &VideoUpscaleJob,
    total_frames: u64,
    keyframes: &[KeyframePoint],
) -> Option<Vec<PlannedSegment>> {
    if total_frames == 0 || keyframes.len() < 2 {
        return None;
    }
    let target_frames = segment_frames_for_fps(job.info.fps_num, job.info.fps_den).max(1);
    let min_frames =
        segment_frames_for_seconds(job.info.fps_num, job.info.fps_den, SEGMENT_MIN_SECONDS);
    let max_frames =
        segment_frames_for_seconds(job.info.fps_num, job.info.fps_den, SEGMENT_MAX_SECONDS);

    let mut boundaries = Vec::new();
    boundaries.push(boundary_for_frame(job, 0, keyframes));
    let mut start = 0_u64;
    while start.saturating_add(target_frames) < total_frames {
        let ideal = start.saturating_add(target_frames);
        let min_end = start.saturating_add(min_frames).min(total_frames);
        let max_end = start.saturating_add(max_frames).min(total_frames);
        let Some(next) = nearest_keyframe_in_range(keyframes, ideal, min_end, max_end) else {
            return None;
        };
        if next.frame <= start || next.frame >= total_frames {
            return None;
        }
        if total_frames.saturating_sub(next.frame) < min_frames {
            break;
        }
        boundaries.push(SegmentBoundary {
            frame: next.frame,
            pts: next.pts,
            seek_frame: next.frame,
            seek_pts: next.pts,
        });
        start = next.frame;
    }
    boundaries.push(boundary_for_frame(job, total_frames, keyframes));

    boundaries.dedup_by_key(|boundary| boundary.frame);
    if boundaries.len() < 2 {
        return None;
    }

    Some(
        boundaries
            .windows(2)
            .enumerate()
            .map(|(index, pair)| PlannedSegment {
                index: index as u32,
                target_start_frame: pair[0].frame,
                target_end_frame_exclusive: pair[1].frame,
                target_start_pts: pair[0].pts,
                target_end_pts: pair[1].pts,
                seek_start_frame: pair[0].seek_frame,
                seek_start_pts: pair[0].seek_pts,
            })
            .collect(),
    )
}

#[derive(Clone, Copy)]
struct SegmentBoundary {
    frame: u64,
    pts: i64,
    seek_frame: u64,
    seek_pts: i64,
}

fn boundary_for_frame(
    job: &VideoUpscaleJob,
    frame: u64,
    keyframes: &[KeyframePoint],
) -> SegmentBoundary {
    let seek = previous_keyframe(keyframes, frame).unwrap_or(KeyframePoint {
        frame: 0,
        pts: frame_to_pts(job, 0),
    });
    let pts = keyframes
        .iter()
        .find(|keyframe| keyframe.frame == frame)
        .map(|keyframe| keyframe.pts)
        .unwrap_or_else(|| frame_to_pts(job, frame));
    SegmentBoundary {
        frame,
        pts,
        seek_frame: seek.frame,
        seek_pts: seek.pts,
    }
}

fn nearest_keyframe_in_range(
    keyframes: &[KeyframePoint],
    ideal: u64,
    min_frame: u64,
    max_frame: u64,
) -> Option<KeyframePoint> {
    keyframes
        .iter()
        .copied()
        .filter(|keyframe| keyframe.frame >= min_frame && keyframe.frame <= max_frame)
        .min_by_key(|keyframe| keyframe.frame.abs_diff(ideal))
}

fn previous_keyframe(keyframes: &[KeyframePoint], frame: u64) -> Option<KeyframePoint> {
    keyframes
        .iter()
        .copied()
        .take_while(|keyframe| keyframe.frame <= frame)
        .last()
}

fn planned_segment_for_frames(
    job: &VideoUpscaleJob,
    index: u32,
    start_frame: u64,
    end_frame: u64,
) -> PlannedSegment {
    PlannedSegment {
        index,
        target_start_frame: start_frame,
        target_end_frame_exclusive: end_frame,
        target_start_pts: frame_to_pts(job, start_frame),
        target_end_pts: if end_frame == u64::MAX {
            i64::MAX
        } else {
            frame_to_pts(job, end_frame)
        },
        seek_start_frame: 0,
        seek_start_pts: 0,
    }
}

fn segment_frames_for_fps(fps_num: i32, fps_den: i32) -> u64 {
    segment_frames_for_seconds(fps_num, fps_den, SEGMENT_TARGET_SECONDS)
}

fn segment_frames_for_seconds(fps_num: i32, fps_den: i32, seconds: f64) -> u64 {
    if fps_num <= 0 || fps_den <= 0 {
        return (30.0 * seconds).round().max(1.0) as u64;
    }
    ((fps_num as f64 / fps_den as f64) * seconds)
        .round()
        .clamp(1.0, 1200.0) as u64
}

fn frame_to_pts(job: &VideoUpscaleJob, frame: u64) -> i64 {
    if job.info.fps_num <= 0 || job.info.fps_den <= 0 {
        return frame as i64;
    }
    let seconds = frame as f64 * job.info.fps_den as f64 / job.info.fps_num as f64;
    (seconds * job.info.source_time_base.den as f64 / job.info.source_time_base.num as f64).round()
        as i64
}

fn segment_done_and_reusable(manifest: &JobManifest, work_dir: &Path, index: u32) -> bool {
    let Some(segment) = manifest
        .segments
        .iter()
        .find(|segment| segment.index == index && segment.state == SegmentState::Done)
    else {
        return false;
    };
    let path = segment_path(work_dir, index);
    path.metadata()
        .is_ok_and(|metadata| metadata.len() == segment.size && segment.size > 0)
}

fn cleanup_segment_parts(work_dir: &Path, index: u32) {
    let segments_dir = segments_dir_for(work_dir);
    let prefix = format!("{}.part", segment_file_name(index));
    let Ok(entries) = fs::read_dir(&segments_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == segment_part_file_name(index) || name.starts_with(&prefix) {
            let _ = fs::remove_file(path);
        }
    }
}

fn validate_segment_file(path: &Path) -> Result<(), String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("failed to read segment file metadata: {e}"))?;
    if metadata.len() == 0 {
        return Err("segment file is empty".to_owned());
    }
    let _ = ffmpeg::format::input(path)
        .map_err(|e| format!("failed to validate segment file with FFmpeg: {e}"))?;
    Ok(())
}

fn upsert_segment_entry(manifest: &mut JobManifest, entry: SegmentEntry) {
    if let Some(existing) = manifest
        .segments
        .iter_mut()
        .find(|segment| segment.index == entry.index)
    {
        *existing = entry;
    } else {
        manifest.segments.push(entry);
        manifest.segments.sort_by_key(|segment| segment.index);
    }
}

fn segment_path_file_name(index: u32) -> String {
    segment_file_name(index)
}

fn file_mtime_unix_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn time_base_from_rational(value: ffmpeg::Rational) -> TimeBase {
    TimeBase::new(value.numerator(), value.denominator())
}

fn rescale_ticks(value: i64, source: TimeBase, destination: TimeBase) -> i64 {
    if source.den == 0 || destination.num == 0 {
        return value;
    }
    let num = value as i128 * source.num as i128 * destination.den as i128;
    let den = source.den as i128 * destination.num as i128;
    (num / den) as i64
}

fn source_pts_to_av_time_base(pts: i64, source: TimeBase) -> i64 {
    rescale_ticks(pts, source, TimeBase::new(1, 1_000_000))
}

fn seek_input_to_source_pts(
    input: &mut ffmpeg::format::context::Input,
    video_stream_index: usize,
    source_time_base: TimeBase,
    source_pts: i64,
) -> Result<(), ffmpeg::Error> {
    let ret = unsafe {
        ffmpeg::ffi::av_seek_frame(
            input.as_mut_ptr(),
            video_stream_index as i32,
            source_pts,
            ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
        )
    };
    if ret >= 0 {
        Ok(())
    } else {
        let target = source_pts_to_av_time_base(source_pts, source_time_base);
        input.seek(target, target..)
    }
}

fn reset_codec_tag(mut stream: ffmpeg::format::stream::StreamMut<'_>) {
    let mut parameters = stream.parameters_mut();
    unsafe {
        (*parameters.as_mut_ptr()).codec_tag = 0;
    }
}

fn sorted_done_segments(manifest: &JobManifest) -> Vec<&SegmentEntry> {
    let mut done_segments: Vec<_> = manifest
        .segments
        .iter()
        .filter(|segment| segment.state == SegmentState::Done)
        .collect();
    done_segments.sort_by_key(|segment| segment.index);
    done_segments
}

fn finalize_segments(
    job: &VideoUpscaleJob,
    manifest: &JobManifest,
    work_dir: &Path,
    final_part_path: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<FinalizeResult, String> {
    let source_input = ffmpeg::format::input(&job.source_path)
        .map_err(|e| format!("音声確認のため元動画を開けません: {e}"))?;
    let has_audio = source_input
        .streams()
        .any(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio);
    drop(source_input);

    if has_audio {
        mux_segments_with_audio(job, manifest, work_dir, final_part_path, cancel)?;
        Ok(FinalizeResult {
            audio_sidecar_value: "copy",
        })
    } else {
        concat_video_segments(manifest, work_dir, final_part_path, cancel)?;
        Ok(FinalizeResult {
            audio_sidecar_value: "none",
        })
    }
}

fn concat_video_segments(
    manifest: &JobManifest,
    work_dir: &Path,
    final_part_path: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let done_segments = sorted_done_segments(manifest);
    if done_segments.is_empty() {
        return Err("no completed segments to concatenate".to_owned());
    }

    let first_path = work_dir.join(&done_segments[0].path);
    if done_segments.len() == 1 {
        fs::copy(&first_path, final_part_path)
            .map_err(|e| format!("failed to copy single segment to final output: {e}"))?;
        return Ok(());
    }

    let first_input = ffmpeg::format::input(&first_path)
        .map_err(|e| format!("failed to open first segment: {e}"))?;
    let first_stream = first_input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "first segment has no video stream".to_owned())?;
    let codec = ffmpeg::codec::encoder::find(first_stream.parameters().id())
        .ok_or_else(|| "could not find encoder metadata for segment remux".to_owned())?;
    let mut output = ffmpeg::format::output_as(final_part_path, "matroska")
        .map_err(|e| format!("failed to create final video output: {e}"))?;
    let output_stream_index;
    {
        let mut output_stream = output
            .add_stream(codec)
            .map_err(|e| format!("failed to add final video stream: {e}"))?;
        output_stream_index = output_stream.index();
        output_stream.set_parameters(first_stream.parameters());
        output_stream.set_time_base(first_stream.time_base());
        reset_codec_tag(output_stream);
    }
    drop(first_stream);
    drop(first_input);

    output
        .write_header()
        .map_err(|e| format!("failed to write final video header: {e}"))?;
    let output_time_base = output
        .stream(output_stream_index)
        .ok_or_else(|| "failed to read final video stream".to_owned())?
        .time_base();
    let mut cumulative_offset = 0_i64;
    let mut packet_count = 0_u64;

    // T41 (Codex P2 / 2026-05-16): 本ループは segment ごとに「直前の packet 末尾を
    // 起点に offset を加算」する CFR 結合 (= 入力 PTS を rescale → 累積 offset 加算)
    // のため、**VFR 動画は無言で CFR 変換される**。各 segment の plan 段階で
    // `output_total_pts_ticks = frame_count * (1/fps)` を使う設計のため、出力動画の
    // duration は「入力 frame 数 × 一定 fps」になる。元動画が VFR (= packet PTS の間隔が
    // 一定でない、コマ落ち / 可変 fps カメラ) でも出力 fps は plan の fps_num/fps_den で
    // 平準化される。
    //
    // 実害: (a) duration が元動画と微妙に異なる (b) plan 段階の fps が元 stream の
    // avg_frame_rate を使うので、可変 fps の動画では音声同期が drift する。
    //
    // 真の修正は「各 segment の packet PTS をそのまま採用 + segment 間のシームレス連結」で、
    // 実装には output_total_pts_ticks の計算を実 packet timing から導出する変更が要る。
    // v0.10 で対応予定。本 v0.9.0 では「VFR は CFR に flatten される」挙動を明示する
    // コメントで運用する (= ユーザー報告で気付けるように)。
    for segment in done_segments {
        check_finalize_cancel(cancel, packet_count)?;
        let path = work_dir.join(&segment.path);
        let mut input =
            ffmpeg::format::input(&path).map_err(|e| format!("failed to open segment: {e}"))?;
        let input_stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| "segment has no video stream".to_owned())?;
        let input_stream_index = input_stream.index();
        let input_time_base = input_stream.time_base();
        for packet_result in input.packets() {
            let (stream, mut packet) =
                packet_result.map_err(|e| format!("failed to read segment packet: {e}"))?;
            if stream.index() != input_stream_index {
                continue;
            }
            check_finalize_cancel(cancel, packet_count)?;
            packet_count = packet_count.saturating_add(1);
            packet.rescale_ts(input_time_base, output_time_base);
            packet.set_pts(packet.pts().map(|pts| pts + cumulative_offset));
            packet.set_dts(packet.dts().map(|dts| dts + cumulative_offset));
            packet.set_stream(output_stream_index);
            packet
                .write_interleaved(&mut output)
                .map_err(|e| format!("failed to write final video packet: {e}"))?;
        }
        cumulative_offset += rescale_ticks(
            segment.output_total_pts_ticks,
            segment.output_time_base,
            time_base_from_rational(output_time_base),
        );
    }
    output
        .write_trailer()
        .map_err(|e| format!("failed to write final video trailer: {e}"))?;
    Ok(())
}

fn mux_segments_with_audio(
    job: &VideoUpscaleJob,
    manifest: &JobManifest,
    work_dir: &Path,
    final_part_path: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let done_segments = sorted_done_segments(manifest);
    if done_segments.is_empty() {
        return Err("no completed segments to mux".to_owned());
    }

    let first_path = work_dir.join(&done_segments[0].path);
    let first_input = ffmpeg::format::input(&first_path)
        .map_err(|e| format!("failed to open first segment: {e}"))?;
    let first_stream = first_input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "first segment has no video stream".to_owned())?;
    let video_codec = first_stream.parameters().id();

    let mut source_input = ffmpeg::format::input(&job.source_path)
        .map_err(|e| format!("音声コピーのため元動画を開けません: {e}"))?;
    let audio_inputs: Vec<_> = source_input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .map(|stream| (stream.index(), stream.time_base()))
        .collect();
    if audio_inputs.is_empty() {
        // Defensive fallback: the source was reopened after the initial audio probe.
        return concat_video_segments(manifest, work_dir, final_part_path, cancel);
    }

    let mut output = ffmpeg::format::output_as(final_part_path, "matroska")
        .map_err(|e| format!("音声付き出力を作成できません: {e}"))?;
    let video_output_index;
    {
        let mut output_stream = output
            .add_stream(video_codec)
            .map_err(|e| format!("failed to add final video stream: {e}"))?;
        video_output_index = output_stream.index();
        output_stream.set_parameters(first_stream.parameters());
        output_stream.set_time_base(first_stream.time_base());
        reset_codec_tag(output_stream);
    }

    let mut audio_maps = Vec::new();
    for (input_index, input_time_base) in &audio_inputs {
        let input_stream = source_input
            .stream(*input_index)
            .ok_or_else(|| format!("音声ストリームを取得できません: {input_index}"))?;
        let mut output_stream = output
            .add_stream(input_stream.parameters().id())
            .map_err(|e| format!("音声出力ストリームの作成に失敗しました: {e}"))?;
        let output_index = output_stream.index();
        output_stream.set_parameters(input_stream.parameters());
        output_stream.set_time_base(*input_time_base);
        reset_codec_tag(output_stream);
        audio_maps.push(StreamCopyMap {
            input_index: *input_index,
            output_index,
            input_time_base: *input_time_base,
            output_time_base: *input_time_base,
        });
    }
    drop(first_stream);
    drop(first_input);

    output
        .write_header()
        .map_err(|e| format!("音声付き出力ヘッダの書き込みに失敗しました: {e}"))?;
    let video_output_time_base = output
        .stream(video_output_index)
        .ok_or_else(|| "failed to read final video stream".to_owned())?
        .time_base();
    for map in &mut audio_maps {
        map.output_time_base = output
            .stream(map.output_index)
            .ok_or_else(|| format!("音声出力ストリームを取得できません: {}", map.output_index))?
            .time_base();
    }

    write_interleaved_segment_video_and_audio(
        &mut output,
        manifest,
        work_dir,
        video_output_index,
        video_output_time_base,
        &mut source_input,
        &audio_maps,
        cancel,
    )?;
    output
        .write_trailer()
        .map_err(|e| format!("音声付き出力trailerの書き込みに失敗しました: {e}"))?;
    Ok(())
}

fn write_interleaved_segment_video_and_audio(
    output: &mut ffmpeg::format::context::Output,
    manifest: &JobManifest,
    work_dir: &Path,
    video_output_index: usize,
    video_output_time_base: ffmpeg::Rational,
    source_input: &mut ffmpeg::format::context::Input,
    audio_maps: &[StreamCopyMap],
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut video_cursor = SegmentVideoCursor::new(
        manifest,
        work_dir,
        video_output_index,
        video_output_time_base,
    );
    let mut next_video = video_cursor.next_packet()?;
    let mut next_audio = next_audio_packet(source_input, audio_maps)?;
    let mut packet_count = 0_u64;

    while next_video.is_some() || next_audio.is_some() {
        check_finalize_cancel(cancel, packet_count)?;
        packet_count = packet_count.saturating_add(1);
        let write_video = match (&next_video, &next_audio) {
            (Some(video), Some(audio)) => {
                let audio_map = audio_maps
                    .iter()
                    .find(|map| map.output_index == audio.stream())
                    .ok_or_else(|| "音声stream mapを取得できません".to_owned())?;
                packet_sort_key(video, video_output_time_base)
                    <= packet_sort_key(audio, audio_map.output_time_base)
            }
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };

        if write_video {
            if let Some(packet) = next_video.take() {
                packet
                    .write_interleaved(output)
                    .map_err(|e| format!("failed to write final video packet: {e}"))?;
            }
            next_video = video_cursor.next_packet()?;
        } else {
            if let Some(packet) = next_audio.take() {
                packet
                    .write_interleaved(output)
                    .map_err(|e| format!("音声packetの書き込みに失敗しました: {e}"))?;
            }
            next_audio = next_audio_packet(source_input, audio_maps)?;
        }
    }
    Ok(())
}

fn check_finalize_cancel(cancel: &Arc<AtomicBool>, packet_count: u64) -> Result<(), String> {
    if packet_count % 1000 == 0 && cancel.load(Ordering::Relaxed) {
        return Err("キャンセルされました".to_owned());
    }
    Ok(())
}

fn next_audio_packet(
    source_input: &mut ffmpeg::format::context::Input,
    audio_maps: &[StreamCopyMap],
) -> Result<Option<ffmpeg::Packet>, String> {
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(source_input) {
            Ok(()) => {
                let Some(map) = audio_maps
                    .iter()
                    .find(|map| map.input_index == packet.stream())
                else {
                    continue;
                };
                packet.rescale_ts(map.input_time_base, map.output_time_base);
                packet.set_stream(map.output_index);
                return Ok(Some(packet));
            }
            Err(ffmpeg::Error::Eof) => return Ok(None),
            Err(e) => return Err(format!("音声packetの読み込みに失敗しました: {e}")),
        }
    }
}

fn packet_sort_key(packet: &ffmpeg::Packet, time_base: ffmpeg::Rational) -> i128 {
    let ts = packet.dts().or_else(|| packet.pts()).unwrap_or(0) as i128;
    if time_base.denominator() == 0 {
        return ts;
    }
    ts * time_base.numerator() as i128 * 1_000_000_000_i128 / time_base.denominator() as i128
}

fn encode_video_segment(
    job: &VideoUpscaleJob,
    planned: &PlannedSegment,
    part_path: &Path,
    runtime: &AiRuntime,
    cancel: &Arc<AtomicBool>,
    progress: &VideoUpscaleProgressShared,
    progress_mode: SegmentProgressMode,
) -> Result<SegmentEncodeResult, String> {
    let mut ictx =
        ffmpeg::format::input(&job.source_path).map_err(|e| format!("動画を開けません: {e}"))?;
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| "動画ストリームが見つかりません".to_owned())?;
    let video_stream_index = input_stream.index();
    let input_time_base = input_stream.time_base();
    let params = input_stream.parameters();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(params)
        .map_err(|e| format!("動画パラメータを読めません: {e}"))?
        .decoder()
        .video()
        .map_err(|e| format!("動画デコーダを開けません: {e}"))?;

    let src_format = decoder.format();
    let src_w = decoder.width();
    let src_h = decoder.height();
    let (out_w, out_h) = job.info.output_size(job.options.scale);
    let encoder_format = job.options.quality.pixel_format();
    let mut rgba_scaler = ffmpeg::software::scaling::Context::get(
        src_format,
        src_w,
        src_h,
        ffmpeg::format::Pixel::RGBA,
        src_w,
        src_h,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )
    .map_err(|e| format!("RGBA変換の初期化に失敗しました: {e}"))?;
    let mut encode_scaler = ffmpeg::software::scaling::Context::get(
        ffmpeg::format::Pixel::RGBA,
        out_w,
        out_h,
        encoder_format,
        out_w,
        out_h,
        ffmpeg::software::scaling::flag::Flags::BICUBIC,
    )
    .map_err(|e| format!("エンコード色変換の初期化に失敗しました: {e}"))?;

    let encoder_codec = ffmpeg::codec::encoder::find_by_name("libsvtav1")
        .ok_or_else(|| "libsvtav1 encoder が FFmpeg build に見つかりません".to_owned())?;
    let mut octx = ffmpeg::format::output_as(part_path, "matroska")
        .map_err(|e| format!("出力を作成できません: {e}"))?;
    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

    let stream_index;
    let stream_time_base = (job.info.fps_den, job.info.fps_num);
    let encoder_time_base = ffmpeg::Rational(job.info.fps_den, job.info.fps_num);
    let mut encoder = {
        let mut stream = octx
            .add_stream(encoder_codec)
            .map_err(|e| format!("出力ストリームの作成に失敗しました: {e}"))?;
        stream_index = stream.index();
        stream.set_time_base(stream_time_base);

        let mut enc = ffmpeg::codec::context::Context::new_with_codec(encoder_codec)
            .encoder()
            .video()
            .map_err(|e| format!("AV1エンコーダの作成に失敗しました: {e}"))?;
        enc.set_width(out_w);
        enc.set_height(out_h);
        enc.set_format(encoder_format);
        enc.set_time_base(stream_time_base);
        enc.set_frame_rate(Some((job.info.fps_num, job.info.fps_den)));
        enc.set_gop(gop_frames_for_fps(job.info.fps_num, job.info.fps_den));
        if global_header {
            enc.set_flags(ffmpeg::codec::flag::Flags::GLOBAL_HEADER);
        }
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("crf", &job.options.quality.crf().to_string());
        opts.set("preset", &job.options.quality.preset().to_string());
        opts.set("film-grain", "0");
        let opened = enc
            .open_as_with(encoder_codec, opts)
            .map_err(|e| format!("AV1エンコーダを開けません: {e}"))?;
        stream.copy_parameters_from_context(&opened);
        opened
    };
    octx.write_header()
        .map_err(|e| format!("出力ヘッダの書き込みに失敗しました: {e}"))?;
    let mux_stream_time_base = octx
        .stream(stream_index)
        .ok_or_else(|| "出力ストリームを取得できません".to_owned())?
        .time_base();

    let started = Instant::now();
    if matches!(progress_mode, SegmentProgressMode::Sequential { .. }) {
        progress
            .rate_base_frames
            .store(progress_mode.base_frames(), Ordering::Relaxed);
        progress.elapsed_ms.store(0, Ordering::Relaxed);
    }
    if planned.seek_start_frame > 0 || planned.seek_start_pts > 0 {
        seek_input_to_source_pts(
            &mut ictx,
            video_stream_index,
            job.info.source_time_base,
            planned.seek_start_pts,
        )
        .map_err(|e| format!("segment seek failed: {e}"))?;
    }
    let mut source_frame_index = planned.seek_start_frame;
    let mut segment_frame_index = 0_i64;
    let mut last_source_pts = None;
    let mut reached_segment_end = false;
    for packet_result in ictx.packets() {
        let (stream, packet) =
            packet_result.map_err(|e| format!("動画パケットの読み込みに失敗しました: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルされました".to_owned());
        }
        if stream.index() != video_stream_index {
            continue;
        }
        let mut packet = packet;
        packet.rescale_ts(input_time_base, decoder.time_base());
        loop {
            match decoder.send_packet(&packet) {
                Ok(()) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => {
                    reached_segment_end = receive_and_encode_frames(
                        &mut decoder,
                        &mut rgba_scaler,
                        &mut encode_scaler,
                        &mut encoder,
                        &mut octx,
                        stream_index,
                        encoder_time_base,
                        mux_stream_time_base,
                        job,
                        planned,
                        runtime,
                        cancel,
                        progress,
                        started,
                        &progress_mode,
                        &mut source_frame_index,
                        &mut segment_frame_index,
                        &mut last_source_pts,
                    )?;
                    if reached_segment_end {
                        break;
                    }
                }
                Err(e) => return Err(format!("動画パケットのデコード投入に失敗しました: {e}")),
            }
        }
        if reached_segment_end {
            break;
        }
        reached_segment_end = receive_and_encode_frames(
            &mut decoder,
            &mut rgba_scaler,
            &mut encode_scaler,
            &mut encoder,
            &mut octx,
            stream_index,
            encoder_time_base,
            mux_stream_time_base,
            job,
            planned,
            runtime,
            cancel,
            progress,
            started,
            &progress_mode,
            &mut source_frame_index,
            &mut segment_frame_index,
            &mut last_source_pts,
        )?;
        if reached_segment_end {
            break;
        }
    }
    if !reached_segment_end {
        decoder
            .send_eof()
            .map_err(|e| format!("デコーダのflushに失敗しました: {e}"))?;
        receive_and_encode_frames(
            &mut decoder,
            &mut rgba_scaler,
            &mut encode_scaler,
            &mut encoder,
            &mut octx,
            stream_index,
            encoder_time_base,
            mux_stream_time_base,
            job,
            planned,
            runtime,
            cancel,
            progress,
            started,
            &progress_mode,
            &mut source_frame_index,
            &mut segment_frame_index,
            &mut last_source_pts,
        )?;
    }

    encoder
        .send_eof()
        .map_err(|e| format!("エンコーダのflushに失敗しました: {e}"))?;
    drain_encoder(
        &mut encoder,
        &mut octx,
        stream_index,
        encoder_time_base,
        mux_stream_time_base,
    )?;
    octx.write_trailer()
        .map_err(|e| format!("出力trailerの書き込みに失敗しました: {e}"))?;
    Ok(SegmentEncodeResult {
        frame_count: segment_frame_index as u64,
        total_pts_ticks: rescale_ticks(
            segment_frame_index,
            TimeBase::new(
                encoder_time_base.numerator(),
                encoder_time_base.denominator(),
            ),
            time_base_from_rational(mux_stream_time_base),
        ),
        output_time_base: time_base_from_rational(mux_stream_time_base),
        source_start_pts: planned.target_start_pts,
        source_last_pts: last_source_pts
            .unwrap_or_else(|| planned.target_end_pts.saturating_sub(1)),
    })
}

#[allow(clippy::too_many_arguments)]
fn receive_and_encode_frames(
    decoder: &mut ffmpeg::decoder::Video,
    rgba_scaler: &mut ffmpeg::software::scaling::Context,
    encode_scaler: &mut ffmpeg::software::scaling::Context,
    encoder: &mut ffmpeg::encoder::Video,
    octx: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    mux_stream_time_base: ffmpeg::Rational,
    job: &VideoUpscaleJob,
    planned: &PlannedSegment,
    runtime: &AiRuntime,
    cancel: &Arc<AtomicBool>,
    progress: &VideoUpscaleProgressShared,
    started: Instant,
    progress_mode: &SegmentProgressMode,
    source_frame_index: &mut u64,
    segment_frame_index: &mut i64,
    last_source_pts: &mut Option<i64>,
) -> Result<bool, String> {
    let model_kind = job.options.model.model_kind();
    loop {
        let mut decoded = ffmpeg::util::frame::video::Video::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(format!("動画フレームのデコードに失敗しました: {e}")),
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルされました".to_owned());
        }
        let current_source_frame = *source_frame_index;
        *source_frame_index = source_frame_index.saturating_add(1);
        if current_source_frame < planned.target_start_frame {
            continue;
        }
        if current_source_frame >= planned.target_end_frame_exclusive {
            return Ok(true);
        }
        let source_pts = decoded
            .pts()
            .unwrap_or_else(|| frame_to_pts(job, current_source_frame));
        let mut rgba = ffmpeg::util::frame::video::Video::new(
            ffmpeg::format::Pixel::RGBA,
            decoded.width(),
            decoded.height(),
        );
        rgba_scaler
            .run(&decoded, &mut rgba)
            .map_err(|e| format!("RGBA変換に失敗しました: {e}"))?;
        let input_image = dynamic_image_from_rgba_frame(&rgba)?;
        let upscaled = crate::ai::upscale::upscale(runtime, model_kind, &input_image, cancel)
            .map_err(|e| format!("AIアップスケールに失敗しました: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルされました".to_owned());
        }
        let out_rgba = output_rgba_frame(upscaled, job.options.scale)?;
        let mut yuv = ffmpeg::util::frame::video::Video::new(
            job.options.quality.pixel_format(),
            out_rgba.width(),
            out_rgba.height(),
        );
        encode_scaler
            .run(&out_rgba, &mut yuv)
            .map_err(|e| format!("エンコード色変換に失敗しました: {e}"))?;
        yuv.set_pts(Some(*segment_frame_index));
        encoder
            .send_frame(&yuv)
            .map_err(|e| format!("エンコーダへのフレーム投入に失敗しました: {e}"))?;
        drain_encoder(
            encoder,
            octx,
            stream_index,
            encoder_time_base,
            mux_stream_time_base,
        )?;

        *last_source_pts = Some(source_pts);
        *segment_frame_index += 1;
        progress_mode.update(progress, *segment_frame_index as u64);
        if matches!(progress_mode, SegmentProgressMode::Sequential { .. }) {
            progress
                .elapsed_ms
                .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    }
    Ok(false)
}

fn drain_encoder(
    encoder: &mut ffmpeg::encoder::Video,
    octx: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    source_time_base: ffmpeg::Rational,
    destination_time_base: ffmpeg::Rational,
) -> Result<(), String> {
    loop {
        let mut encoded = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut encoded) {
            Ok(()) => {
                encoded.set_stream(stream_index);
                encoded.rescale_ts(source_time_base, destination_time_base);
                encoded
                    .write_interleaved(octx)
                    .map_err(|e| format!("エンコード済みパケットの書き込みに失敗しました: {e}"))?;
            }
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(e) => return Err(format!("エンコード済みパケットの取得に失敗しました: {e}")),
        }
    }
    Ok(())
}

fn dynamic_image_from_rgba_frame(
    frame: &ffmpeg::util::frame::video::Video,
) -> Result<DynamicImage, String> {
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride(0);
    let src = frame.data(0);
    let row_len = width as usize * 4;
    let mut bytes = vec![0_u8; row_len * height as usize];
    for y in 0..height as usize {
        let src_start = y * stride;
        let dst_start = y * row_len;
        bytes[dst_start..dst_start + row_len].copy_from_slice(&src[src_start..src_start + row_len]);
    }
    let img = RgbaImage::from_raw(width, height, bytes)
        .ok_or_else(|| "RGBAフレームの画像化に失敗しました".to_owned())?;
    Ok(DynamicImage::ImageRgba8(img))
}

fn output_rgba_frame(
    image: egui::ColorImage,
    scale: VideoUpscaleScale,
) -> Result<ffmpeg::util::frame::video::Video, String> {
    let width = image.size[0] as u32;
    let height = image.size[1] as u32;
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for c in &image.pixels {
        rgba.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
    }
    let img = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "AI出力の画像化に失敗しました".to_owned())?;
    let img = if scale == VideoUpscaleScale::X2 {
        let down_w = (width / 2).max(1);
        let down_h = (height / 2).max(1);
        image::imageops::resize(&img, down_w, down_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    rgba_image_to_frame(&img)
}

fn rgba_image_to_frame(
    img: &ImageBuffer<image::Rgba<u8>, Vec<u8>>,
) -> Result<ffmpeg::util::frame::video::Video, String> {
    let width = img.width();
    let height = img.height();
    let mut frame =
        ffmpeg::util::frame::video::Video::new(ffmpeg::format::Pixel::RGBA, width, height);
    let stride = frame.stride(0);
    let dst = frame.data_mut(0);
    let row_len = width as usize * 4;
    for y in 0..height as usize {
        let src_start = y * row_len;
        let dst_start = y * stride;
        dst[dst_start..dst_start + row_len]
            .copy_from_slice(&img.as_raw()[src_start..src_start + row_len]);
    }
    Ok(frame)
}

fn sane_rate(rate: ffmpeg::Rational) -> Option<(i32, i32)> {
    if rate.numerator() > 0 && rate.denominator() > 0 {
        Some((rate.numerator(), rate.denominator()))
    } else {
        None
    }
}

fn duration_seconds(duration: i64, time_base: ffmpeg::Rational) -> Option<f64> {
    if duration <= 0 || time_base.numerator() <= 0 || time_base.denominator() <= 0 {
        return None;
    }
    Some(duration as f64 * time_base.numerator() as f64 / time_base.denominator() as f64)
}

fn gop_frames_for_fps(fps_num: i32, fps_den: i32) -> u32 {
    if fps_num <= 0 || fps_den <= 0 {
        return 60;
    }
    let fps = (fps_num as f64 / fps_den as f64).round();
    fps.max(1.0) as u32 * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(estimated_frames: Option<u64>) -> VideoUpscaleJob {
        VideoUpscaleJob {
            source_path: PathBuf::from("clip.mp4"),
            output_path: PathBuf::from("clip.miv.mkv"),
            sidecar_path: PathBuf::from("clip.miv.json"),
            info: VideoInfo {
                width: 320,
                height: 240,
                fps_num: 24,
                fps_den: 1,
                source_time_base: TimeBase::new(1, 24_000),
                estimated_frames,
                duration_secs: estimated_frames.map(|frames| frames as f64 / 24.0),
            },
            options: VideoUpscaleOptions {
                scale: VideoUpscaleScale::X2,
                model: VideoUpscaleModelPreset::GeneralFast,
                quality: VideoUpscaleQuality::Q3,
                overwrite: false,
            },
            parallel_segments: Arc::new(AtomicU8::new(1)),
            pause: Arc::new(AtomicBool::new(false)),
            paused_idle: Arc::new(AtomicBool::new(false)),
        }
    }

    fn test_manifest(estimated_frames: u64) -> JobManifest {
        JobManifest::new(
            uuid::Uuid::new_v4(),
            ManifestSource {
                file_name: "clip.mp4".to_owned(),
                size: 1234,
                mtime_unix_ms: 0,
                head_tail_sha256: "hash".to_owned(),
                time_base: TimeBase::new(1, 24_000),
            },
            ManifestOutput {
                final_path: PathBuf::from("clip.miv.mkv"),
                sidecar_path: PathBuf::from("clip.miv.json"),
                width: 640,
                height: 480,
            },
            ManifestOptions {
                scale: 2,
                model: "realesr-general-x4v3".to_owned(),
                quality_level: 3,
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
            },
            estimated_frames,
        )
    }

    #[test]
    fn quality_levels_map_to_expected_encoder_settings() {
        assert_eq!(VideoUpscaleQuality::Q1.crf(), 20);
        assert_eq!(VideoUpscaleQuality::Q1.pixel_format_name(), "yuv420p10le");
        assert_eq!(VideoUpscaleQuality::Q3.crf(), 28);
        assert_eq!(VideoUpscaleQuality::Q3.pixel_format_name(), "yuv420p");
        assert_eq!(VideoUpscaleQuality::Q5.preset(), 9);
    }

    #[test]
    fn preflight_output_size_uses_selected_scale() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            fps_num: 30,
            fps_den: 1,
            source_time_base: TimeBase::new(1, 30),
            estimated_frames: Some(30),
            duration_secs: Some(1.0),
        };
        assert_eq!(info.output_size(VideoUpscaleScale::X2), (3840, 2160));
        assert_eq!(info.output_size(VideoUpscaleScale::X4), (7680, 4320));
        assert!(info.output_allowed(VideoUpscaleScale::X4));
    }

    #[test]
    fn gop_frames_handles_fractional_ntsc_rates() {
        assert_eq!(gop_frames_for_fps(30, 1), 60);
        assert_eq!(gop_frames_for_fps(30000, 1001), 60);
        assert_eq!(gop_frames_for_fps(24000, 1001), 48);
        assert_eq!(gop_frames_for_fps(60000, 1001), 120);
    }

    #[test]
    fn segment_frames_for_fps_targets_about_five_seconds() {
        assert_eq!(segment_frames_for_fps(24, 1), 120);
        assert_eq!(segment_frames_for_fps(60, 1), 300);
        assert_eq!(segment_frames_for_fps(24000, 1001), 120);
        assert_eq!(segment_frames_for_fps(0, 1), 150);
    }

    #[test]
    fn job_current_parallel_segments_clamps_atomic_value() {
        let job = test_job(Some(100));
        assert_eq!(job.current_parallel_segments(), 1);
        job.parallel_segments.store(0, Ordering::Relaxed);
        assert_eq!(job.current_parallel_segments(), 1);
        job.parallel_segments.store(9, Ordering::Relaxed);
        assert_eq!(job.current_parallel_segments(), 1);
        job.parallel_segments.store(3, Ordering::Relaxed);
        assert_eq!(job.current_parallel_segments(), 1);
    }

    #[test]
    fn frame_to_pts_uses_source_time_base() {
        let job = test_job(Some(240));
        assert_eq!(frame_to_pts(&job, 0), 0);
        assert_eq!(frame_to_pts(&job, 24), 24_000);
        assert_eq!(frame_to_pts(&job, 120), 120_000);
    }

    #[test]
    fn rescale_ticks_converts_between_time_bases() {
        assert_eq!(
            rescale_ticks(120, TimeBase::new(1, 24), TimeBase::new(1, 1000)),
            5000
        );
        assert_eq!(
            rescale_ticks(5000, TimeBase::new(1, 1000), TimeBase::new(1, 25)),
            125
        );
    }

    #[test]
    fn packet_sort_key_compares_different_time_bases() {
        let mut video = ffmpeg::Packet::empty();
        video.set_dts(Some(120));
        let mut audio = ffmpeg::Packet::empty();
        audio.set_dts(Some(48000));

        assert_eq!(
            packet_sort_key(&video, ffmpeg::Rational(1, 24)),
            5_000_000_000
        );
        assert_eq!(
            packet_sort_key(&audio, ffmpeg::Rational(1, 48000)),
            1_000_000_000
        );
        assert!(
            packet_sort_key(&audio, ffmpeg::Rational(1, 48000))
                < packet_sort_key(&video, ffmpeg::Rational(1, 24))
        );
    }

    #[test]
    fn packet_sort_key_uses_dts_then_pts_then_zero() {
        let mut with_dts = ffmpeg::Packet::empty();
        with_dts.set_pts(Some(100));
        with_dts.set_dts(Some(90));
        assert_eq!(
            packet_sort_key(&with_dts, ffmpeg::Rational(1, 1000)),
            90_000_000
        );

        let mut pts_only = ffmpeg::Packet::empty();
        pts_only.set_pts(Some(100));
        assert_eq!(
            packet_sort_key(&pts_only, ffmpeg::Rational(1, 1000)),
            100_000_000
        );

        let no_ts = ffmpeg::Packet::empty();
        assert_eq!(packet_sort_key(&no_ts, ffmpeg::Rational(1, 1000)), 0);
    }

    #[test]
    fn finalize_cancel_check_is_throttled() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(check_finalize_cancel(&cancel, 0).is_ok());
        cancel.store(true, Ordering::Relaxed);
        assert!(check_finalize_cancel(&cancel, 999).is_ok());
        assert!(check_finalize_cancel(&cancel, 1000).is_err());
    }

    #[test]
    fn planned_segment_for_unknown_total_uses_open_ended_pts() {
        let job = test_job(None);
        let planned = planned_segment_for_frames(&job, 0, 0, u64::MAX);
        assert_eq!(planned.target_start_frame, 0);
        assert_eq!(planned.target_end_frame_exclusive, u64::MAX);
        assert_eq!(planned.target_start_pts, 0);
        assert_eq!(planned.target_end_pts, i64::MAX);
    }

    #[test]
    fn ensure_plan_creates_single_open_segment_when_total_unknown() {
        let job = test_job(None);
        let mut manifest = test_manifest(0);
        let cancel = Arc::new(AtomicBool::new(false));
        ensure_plan(&job, &mut manifest, &cancel).unwrap();
        let plan = manifest.plan.expect("plan");
        assert_eq!(plan.state, SegmentPlanState::Complete);
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].target_end_frame_exclusive, u64::MAX);
    }

    #[test]
    fn ensure_plan_partitions_total_frames_with_remainder() {
        let job = test_job(Some(250));
        let mut manifest = test_manifest(250);
        let cancel = Arc::new(AtomicBool::new(false));
        ensure_plan(&job, &mut manifest, &cancel).unwrap();
        let plan = manifest.plan.expect("plan");
        let ranges: Vec<_> = plan
            .segments
            .iter()
            .map(|segment| {
                (
                    segment.index,
                    segment.target_start_frame,
                    segment.target_end_frame_exclusive,
                )
            })
            .collect();
        assert_eq!(ranges, vec![(0, 0, 120), (1, 120, 240), (2, 240, 250)]);
    }

    #[test]
    fn keyframe_plan_snaps_boundaries_and_records_seek_origin() {
        let job = test_job(Some(260));
        let keyframes = vec![
            KeyframePoint { frame: 0, pts: 0 },
            KeyframePoint {
                frame: 121,
                pts: frame_to_pts(&job, 121),
            },
            KeyframePoint {
                frame: 240,
                pts: frame_to_pts(&job, 240),
            },
        ];
        let plan = plan_segments_from_keyframes(&job, 260, &keyframes).expect("keyframe plan");
        let ranges: Vec<_> = plan
            .iter()
            .map(|segment| {
                (
                    segment.target_start_frame,
                    segment.target_end_frame_exclusive,
                    segment.seek_start_frame,
                )
            })
            .collect();
        assert_eq!(ranges, vec![(0, 121, 0), (121, 260, 121)]);
        assert_eq!(plan[1].seek_start_pts, frame_to_pts(&job, 121));
    }

    #[test]
    fn keyframe_plan_returns_none_when_keyframes_are_too_sparse() {
        let job = test_job(Some(500));
        let keyframes = vec![
            KeyframePoint { frame: 0, pts: 0 },
            KeyframePoint {
                frame: 400,
                pts: frame_to_pts(&job, 400),
            },
        ];
        assert!(plan_segments_from_keyframes(&job, 500, &keyframes).is_none());
    }

    #[test]
    fn keyframe_plan_returns_none_when_only_first_keyframe_exists() {
        let job = test_job(Some(500));
        let keyframes = vec![KeyframePoint { frame: 0, pts: 0 }];
        assert!(plan_segments_from_keyframes(&job, 500, &keyframes).is_none());
    }

    #[test]
    fn ensure_plan_keeps_existing_complete_plan() {
        let job = test_job(Some(250));
        let mut manifest = test_manifest(250);
        manifest.plan = Some(SegmentPlan {
            strategy: SegmentPlanStrategy::FrameBased,
            state: SegmentPlanState::Complete,
            scan_progress_pts: Some(42),
            segments: vec![planned_segment_for_frames(&job, 9, 0, 10)],
        });
        let cancel = Arc::new(AtomicBool::new(false));
        ensure_plan(&job, &mut manifest, &cancel).unwrap();
        let plan = manifest.plan.expect("plan");
        assert_eq!(plan.strategy, SegmentPlanStrategy::FrameBased);
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].index, 9);
    }

    #[test]
    fn segment_done_and_reusable_requires_matching_done_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("clip.mp4");
        let work_dir = work_dir_for(&source);
        fs::create_dir_all(segments_dir_for(&work_dir)).unwrap();
        let segment_path = segment_path(&work_dir, 0);
        fs::write(&segment_path, b"abc").unwrap();

        let mut manifest = test_manifest(120);
        manifest.segments.push(SegmentEntry {
            index: 0,
            path: PathBuf::from("segments/000000.mkv"),
            state: SegmentState::Done,
            output_frame_start: 0,
            output_frame_count: 120,
            output_total_pts_ticks: 5000,
            output_time_base: TimeBase::new(1, 1000),
            source_start_pts: 0,
            source_last_pts: 119,
            size: 3,
            mtime_unix_ms: 0,
            worker_id: None,
            worker_pid: None,
            worker_started_unix_ms: None,
        });

        assert!(segment_done_and_reusable(&manifest, &work_dir, 0));

        manifest.segments[0].size = 4;
        assert!(!segment_done_and_reusable(&manifest, &work_dir, 0));

        manifest.segments[0].size = 3;
        manifest.segments[0].state = SegmentState::Failed;
        assert!(!segment_done_and_reusable(&manifest, &work_dir, 0));
    }

    /// T04: 現 source の `source_info` を読み取り、`test_job` と整合する manifest と
    /// `VideoUpscaleJob` のペアを作る。テスト本体はこのペアの片方を改変して
    /// `validate_manifest_matches_job` の失敗経路を確認する。
    fn matched_job_and_manifest(temp: &tempfile::TempDir) -> (VideoUpscaleJob, JobManifest) {
        let source = temp.path().join("clip.mp4");
        fs::write(&source, b"some video content for validate testing").unwrap();
        let source_info = source_info_for(&source).unwrap();

        let mut job = test_job(Some(120));
        job.source_path = source;
        job.output_path = temp.path().join("clip.miv.mkv");
        job.sidecar_path = temp.path().join("clip.miv.json");

        let (out_w, out_h) = job.info.output_size(job.options.scale);

        let manifest = JobManifest::new(
            uuid::Uuid::new_v4(),
            ManifestSource {
                file_name: source_info.file_name,
                size: source_info.size,
                mtime_unix_ms: source_info.mtime_unix_ms,
                head_tail_sha256: source_info.head_tail_sha256,
                time_base: job.info.source_time_base,
            },
            ManifestOutput {
                final_path: PathBuf::from("clip.miv.mkv"),
                sidecar_path: PathBuf::from("clip.miv.json"),
                width: out_w,
                height: out_h,
            },
            ManifestOptions {
                scale: job.options.scale.factor(),
                model: job.options.model.model_kind().as_str().to_owned(),
                quality_level: job.options.quality.level(),
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
            },
            120,
        );

        (job, manifest)
    }

    #[test]
    fn validate_passes_for_matching_job_and_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, manifest) = matched_job_and_manifest(&temp);
        assert!(validate_manifest_matches_job(&manifest, &job).is_ok());
    }

    #[test]
    fn validate_ignores_mtime_drift() {
        // sync tools rewrite mtime; head_tail_sha256 / size catch real content change.
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.source.mtime_unix_ms = 0;
        assert!(validate_manifest_matches_job(&manifest, &job).is_ok());
        manifest.source.mtime_unix_ms = u64::MAX;
        assert!(validate_manifest_matches_job(&manifest, &job).is_ok());
    }

    #[test]
    fn validate_detects_stale_source_on_size_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.source.size += 1;
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("stale source"), "{err}");
        assert!(err.contains("size changed"), "{err}");
    }

    #[test]
    fn validate_detects_stale_source_on_content_hash_change() {
        // Source overwritten with different content of same size (file_name + size match).
        let temp = tempfile::TempDir::new().unwrap();
        let (job, manifest) = matched_job_and_manifest(&temp);
        fs::write(&job.source_path, b"some video XXXXXXX for validate testing").unwrap();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("stale source"), "{err}");
        assert!(err.contains("hash mismatch"), "{err}");
    }

    #[test]
    fn validate_detects_stale_source_on_file_name_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.source.file_name = "renamed.mp4".to_owned();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("stale source"), "{err}");
        assert!(err.contains("file name changed"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_time_base_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.source.time_base = TimeBase::new(1, 30_000);
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("time base"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_output_dim_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.output.width += 1;
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("dimensions"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_scale_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.scale = 4;
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("scale"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_model_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.model = "different_model".to_owned();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("model"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_quality_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.quality_level = 1;
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("quality"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_encoder_field_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.encoder = "x265".to_owned();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("encoder"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_container_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.container = "mp4".to_owned();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("container"), "{err}");
    }

    #[test]
    fn validate_detects_plan_drift_on_video_codec_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);
        manifest.options.video_codec = "h264".to_owned();
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("segment plan drift"), "{err}");
        assert!(err.contains("video codec"), "{err}");
    }

    /// 各検証エラー文字列が `ui_dialogs::video_upscale::failure_reason_from_error_text`
    /// の分類器で正しく `StaleSource` / `PlanDrift` に振り分けられることを確認する。
    /// 分類器のロジックそのもの (lower.contains の連鎖) はそちらのテストでカバーされる
    /// 想定だが、validate 側が出すキーワードがそこに通る前提を毎リリースで確認する。
    #[test]
    fn validate_errors_contain_classifier_keywords() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, mut manifest) = matched_job_and_manifest(&temp);

        manifest.source.size += 1;
        let stale_err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(stale_err.to_lowercase().contains("stale"));

        manifest.source.size -= 1;
        manifest.options.scale = 4;
        let drift_err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        let lower = drift_err.to_lowercase();
        assert!(lower.contains("plan_drift") || lower.contains("segment plan drift"));
    }

    #[test]
    fn validate_detects_stale_source_when_file_missing() {
        let temp = tempfile::TempDir::new().unwrap();
        let (mut job, manifest) = matched_job_and_manifest(&temp);
        job.source_path = temp.path().join("does_not_exist.mp4");
        let err = validate_manifest_matches_job(&manifest, &job).unwrap_err();
        assert!(err.contains("stale source"), "{err}");
        assert!(err.contains("failed to read source info"), "{err}");
    }

    /// T05: publish_finalized_outputs の成功・ロールバック挙動。
    fn test_finalize_result() -> FinalizeResult {
        FinalizeResult {
            audio_sidecar_value: "copy",
        }
    }

    #[test]
    fn publish_creates_pair_when_none_exists() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        let part_path = temp.path().join("clip.miv.mkv.part");
        fs::write(&part_path, b"new video bytes").unwrap();

        assert!(!job.output_path.exists());
        assert!(!job.sidecar_path.exists());

        publish_finalized_outputs(
            &job,
            &part_path,
            &test_finalize_result(),
            ModelKind::UpscaleRealEsrGeneralV3,
            640,
            480,
        )
        .unwrap();

        assert!(job.output_path.exists(), "video should be published");
        assert!(job.sidecar_path.exists(), "sidecar should be published");
        assert!(!part_path.exists(), "part_path consumed by rename");
        // staged を残さない
        assert!(!job.sidecar_path.with_extension("json.staged").exists());
    }

    #[test]
    fn publish_overwrites_existing_pair_atomically() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        // 旧 pair をあらかじめ用意
        fs::write(&job.output_path, b"OLD video bytes").unwrap();
        fs::write(&job.sidecar_path, b"{\"schema\":99,\"old\":true}").unwrap();
        let part_path = temp.path().join("clip.miv.mkv.part");
        fs::write(&part_path, b"NEW video bytes").unwrap();

        publish_finalized_outputs(
            &job,
            &part_path,
            &test_finalize_result(),
            ModelKind::UpscaleRealEsrGeneralV3,
            640,
            480,
        )
        .unwrap();

        let video = fs::read(&job.output_path).unwrap();
        assert_eq!(video, b"NEW video bytes");
        let sidecar_text = fs::read_to_string(&job.sidecar_path).unwrap();
        assert!(
            sidecar_text.contains("\"schema\""),
            "new sidecar should be JSON: {sidecar_text}"
        );
        assert!(
            !sidecar_text.contains("\"old\""),
            "old sidecar should be replaced: {sidecar_text}"
        );
        assert!(!part_path.exists());
        assert!(!job.sidecar_path.with_extension("json.staged").exists());
    }

    #[test]
    fn publish_returns_err_and_preserves_old_pair_when_video_rename_fails() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        // 旧 pair をあらかじめ用意
        fs::write(&job.output_path, b"OLD video bytes").unwrap();
        let old_sidecar_text = "{\"schema\":99,\"old\":true}";
        fs::write(&job.sidecar_path, old_sidecar_text).unwrap();
        // part_path を意図的に作らない → fs::rename が NotFound で失敗する
        let part_path = temp.path().join("clip.miv.mkv.part");
        assert!(!part_path.exists());

        let err = publish_finalized_outputs(
            &job,
            &part_path,
            &test_finalize_result(),
            ModelKind::UpscaleRealEsrGeneralV3,
            640,
            480,
        )
        .unwrap_err();
        assert!(err.contains("出力ファイルの確定に失敗"), "{err}");

        // step 2 失敗 + 旧 pair 無傷: 旧本編・旧 sidecar は触っていない
        let video = fs::read(&job.output_path).unwrap();
        assert_eq!(video, b"OLD video bytes", "old video must be intact");
        let sidecar = fs::read_to_string(&job.sidecar_path).unwrap();
        assert_eq!(sidecar, old_sidecar_text, "old sidecar must be intact");
        // staged は cleanup されている
        assert!(!job.sidecar_path.with_extension("json.staged").exists());
    }

    #[test]
    fn recover_clears_orphan_staged_sidecar() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        let staged = job.sidecar_path.with_extension("json.staged");
        fs::write(&staged, b"{\"orphan\":true}").unwrap();
        assert!(staged.exists());

        recover_interrupted_publish(&job);
        assert!(
            !staged.exists(),
            "staged orphan should be removed at run_job start"
        );
    }

    /// `work_dir` に「現 job と一致する manifest + plan complete + 全 planned segment が
    /// 実ファイル付き Done」な状態を構築する。`recover_interrupted_publish` が orphan
    /// video を削除して安全な条件 (manifest match + plan complete + all reusable) を満たす。
    fn populate_work_dir_with_reusable_done_segment(job: &VideoUpscaleJob) {
        let work_dir = work_dir_for(&job.source_path);
        fs::create_dir_all(segments_dir_for(&work_dir)).unwrap();
        let segment_file = segment_path(&work_dir, 0);
        let segment_bytes = b"segment bytes";
        fs::write(&segment_file, segment_bytes).unwrap();
        let source = source_info_for(&job.source_path).unwrap();
        let (out_w, out_h) = job.info.output_size(job.options.scale);
        let mut manifest = JobManifest::new(
            uuid::Uuid::new_v4(),
            ManifestSource {
                file_name: source.file_name,
                size: source.size,
                mtime_unix_ms: source.mtime_unix_ms,
                head_tail_sha256: source.head_tail_sha256,
                time_base: job.info.source_time_base,
            },
            ManifestOutput {
                final_path: PathBuf::from("clip.miv.mkv"),
                sidecar_path: PathBuf::from("clip.miv.json"),
                width: out_w,
                height: out_h,
            },
            ManifestOptions {
                scale: job.options.scale.factor(),
                model: job.options.model.model_kind().as_str().to_owned(),
                quality_level: job.options.quality.level(),
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
            },
            120,
        );
        manifest.plan = Some(SegmentPlan {
            strategy: SegmentPlanStrategy::FrameBased,
            state: SegmentPlanState::Complete,
            scan_progress_pts: None,
            segments: vec![PlannedSegment {
                index: 0,
                target_start_frame: 0,
                target_end_frame_exclusive: 120,
                target_start_pts: 0,
                target_end_pts: 120,
                seek_start_frame: 0,
                seek_start_pts: 0,
            }],
        });
        manifest.segments.push(SegmentEntry {
            index: 0,
            path: PathBuf::from("segments/000000.mkv"),
            state: SegmentState::Done,
            output_frame_start: 0,
            output_frame_count: 120,
            output_total_pts_ticks: 5000,
            output_time_base: TimeBase::new(1, 1000),
            source_start_pts: 0,
            source_last_pts: 119,
            size: segment_bytes.len() as u64,
            mtime_unix_ms: 0,
            worker_id: None,
            worker_pid: None,
            worker_started_unix_ms: None,
        });
        manifest
            .save_atomic(&work_dir.join(MANIFEST_FILE_NAME))
            .unwrap();
    }

    #[test]
    fn recover_clears_first_time_orphan_video_when_work_dir_has_reusable_segments() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"orphan video bytes").unwrap();
        assert!(!job.sidecar_path.exists());
        populate_work_dir_with_reusable_done_segment(&job);

        recover_interrupted_publish(&job);
        assert!(
            !job.output_path.exists(),
            "orphan video should be removed when manifest matches + reusable Done segment exists"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_only_subset_of_planned_segments_done() {
        // Codex round 6: plan に 2 segments、片方だけ Done な状態。
        // interrupted publish なら全 segments 完了済みのはず → 片方だけ Done な状態は
        // ユーザーが encoding 中に kill した状態であり、orphan video は無関係な
        // 紛れ込み (= 削除してはいけない)。
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan").unwrap();

        // 上の helper をベースにして 2 segment plan に書き換える
        let work_dir = work_dir_for(&job.source_path);
        populate_work_dir_with_reusable_done_segment(&job);
        let manifest_path = work_dir.join(MANIFEST_FILE_NAME);
        let mut manifest = JobManifest::load(&manifest_path).unwrap();
        // plan に 2 つ目を追加
        if let Some(plan) = manifest.plan.as_mut() {
            plan.segments.push(PlannedSegment {
                index: 1,
                target_start_frame: 120,
                target_end_frame_exclusive: 240,
                target_start_pts: 120,
                target_end_pts: 240,
                seek_start_frame: 120,
                seek_start_pts: 120,
            });
        }
        manifest.save_atomic(&manifest_path).unwrap();

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan must be preserved when not all planned segments are reusable"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_segment_file_size_mismatches_manifest() {
        // P2 (Codex round 5): Done だが実ファイルサイズが manifest 記録と不一致な
        // segment は再利用できない → orphan を削除しない。
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan").unwrap();
        populate_work_dir_with_reusable_done_segment(&job);
        // segment ファイルを切り詰めて manifest 記録のサイズと不一致にする
        let segment_file = segment_path(&work_dir_for(&job.source_path), 0);
        fs::write(&segment_file, b"truncated").unwrap();

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan video must be preserved when segment file size diverges from manifest"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_manifest_does_not_match_job() {
        // P2 (Codex round 5): manifest が現 job と source/options が一致しない場合は
        // 再利用できない → orphan を削除しない。
        let temp = tempfile::TempDir::new().unwrap();
        let (mut job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan").unwrap();
        populate_work_dir_with_reusable_done_segment(&job);
        // populate 後に scale を変更 → manifest と job が drift
        job.options.scale = VideoUpscaleScale::X4;

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan video must be preserved when manifest doesn't match current job"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_no_work_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan video").unwrap();
        let work_dir = work_dir_for(&job.source_path);
        assert!(!work_dir.exists());

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan video must be preserved when work_dir is missing"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_work_dir_is_empty() {
        // P2 (Codex round 4): work_dir.exists() だけでは「Done segment あり」を
        // 保証しない。空 work_dir では orphan を削除しない。
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan video").unwrap();
        let work_dir = work_dir_for(&job.source_path);
        fs::create_dir_all(&work_dir).unwrap();
        // manifest も segment も無い

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan video must be preserved when work_dir has no reusable segments"
        );
    }

    #[test]
    fn detect_existing_pair_rejects_zero_byte_video() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"").unwrap();
        write_matching_sidecar(&job);
        assert!(
            detect_existing_completed_pair(&job).is_none(),
            "0-byte video must not be treated as completed"
        );
    }

    #[test]
    fn recover_preserves_orphan_video_when_manifest_has_no_done_segments() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"valuable orphan video").unwrap();
        let work_dir = work_dir_for(&job.source_path);
        fs::create_dir_all(segments_dir_for(&work_dir)).unwrap();
        let mut manifest = test_manifest(120);
        manifest.segments.push(SegmentEntry {
            index: 0,
            path: PathBuf::from("segments/000000.mkv"),
            state: SegmentState::Pending, // not Done
            output_frame_start: 0,
            output_frame_count: 0,
            output_total_pts_ticks: 0,
            output_time_base: TimeBase::new(1, 1000),
            source_start_pts: 0,
            source_last_pts: 0,
            size: 0,
            mtime_unix_ms: 0,
            worker_id: None,
            worker_pid: None,
            worker_started_unix_ms: None,
        });
        manifest
            .save_atomic(&work_dir.join(MANIFEST_FILE_NAME))
            .unwrap();

        recover_interrupted_publish(&job);
        assert!(
            job.output_path.exists(),
            "orphan video must be preserved when no Done segments exist"
        );
    }

    #[test]
    fn recover_does_not_touch_healthy_pair() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video").unwrap();
        fs::write(&job.sidecar_path, b"{\"healthy\":true}").unwrap();

        recover_interrupted_publish(&job);
        assert!(job.output_path.exists());
        assert!(job.sidecar_path.exists());
    }

    /// T05: pair が現 job と一致する場合の `detect_existing_completed_pair` を helper で構築。
    /// 一致条件を 1 つだけ変えることで個別フィールドの不一致テストを書きやすくする。
    fn write_matching_sidecar(job: &VideoUpscaleJob) {
        let source = source_info_for(&job.source_path).unwrap();
        let (out_w, out_h) = job.info.output_size(job.options.scale);
        let sidecar = VideoUpscaleSidecar::new(
            source,
            UpscaleInfo {
                scale: job.options.scale.factor(),
                model: job.options.model.model_kind().as_str().to_owned(),
            },
            EncodeInfo {
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
                quality_level: job.options.quality.level(),
                crf: job.options.quality.crf(),
                preset: job.options.quality.preset(),
                pixel_format: job.options.quality.pixel_format_name().to_owned(),
                audio: "copy".to_owned(),
            },
            OutputInfo {
                path: job
                    .output_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap(),
                width: out_w,
                height: out_h,
            },
        );
        save_json_atomic(&job.sidecar_path, &sidecar).unwrap();
    }

    #[test]
    fn detect_existing_pair_recognizes_matching_pair() {
        // P2-3: crash 後 + queue mark_done 前のシナリオ。pair が現 job と一致するなら
        // 既完了として早期 Ok する。
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video bytes").unwrap();
        write_matching_sidecar(&job);

        assert!(detect_existing_completed_pair(&job).is_some());
    }

    #[test]
    fn detect_existing_pair_rejects_when_only_one_side_present() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video").unwrap();
        // sidecar 無し
        assert!(detect_existing_completed_pair(&job).is_none());

        // 逆: sidecar あり、video 無し
        fs::remove_file(&job.output_path).unwrap();
        write_matching_sidecar(&job);
        assert!(detect_existing_completed_pair(&job).is_none());
    }

    #[test]
    fn detect_existing_pair_rejects_stale_sidecar_source() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video").unwrap();
        write_matching_sidecar(&job);
        // sidecar 書き込み後にソースを差し替え → head_tail_sha256 不一致
        fs::write(
            &job.source_path,
            b"different source content for validate testing",
        )
        .unwrap();
        assert!(detect_existing_completed_pair(&job).is_none());
    }

    #[test]
    fn detect_existing_pair_rejects_on_scale_drift() {
        let temp = tempfile::TempDir::new().unwrap();
        let (mut job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video").unwrap();
        write_matching_sidecar(&job);
        // sidecar 書き込み後に job 側の scale を変える
        job.options.scale = VideoUpscaleScale::X4;
        assert!(detect_existing_completed_pair(&job).is_none());
    }

    #[test]
    fn detect_existing_pair_rejects_corrupt_sidecar_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        fs::write(&job.output_path, b"video").unwrap();
        fs::write(&job.sidecar_path, b"{ corrupt JSON").unwrap();
        assert!(detect_existing_completed_pair(&job).is_none());
    }

    /// T05 P2 修正の核: 初回出力 (旧 pair なし) で本編 rename が失敗するケース。
    /// staged sidecar が削除されること + 新本編が孤立して output_path に残らない
    /// ことを確認する (= 次回 retry の overwrite=false ガードに弾かれない)。
    #[test]
    fn publish_does_not_leak_video_or_staged_on_first_time_video_rename_failure() {
        let temp = tempfile::TempDir::new().unwrap();
        let (job, _) = matched_job_and_manifest(&temp);
        // 旧 pair を**作らない** (= 初回出力シミュレーション)
        assert!(!job.output_path.exists());
        assert!(!job.sidecar_path.exists());
        // part_path も作らない → step 2 (fs::rename) が NotFound で失敗
        let part_path = temp.path().join("clip.miv.mkv.part");

        let err = publish_finalized_outputs(
            &job,
            &part_path,
            &test_finalize_result(),
            ModelKind::UpscaleRealEsrGeneralV3,
            640,
            480,
        )
        .unwrap_err();
        assert!(err.contains("出力ファイルの確定に失敗"), "{err}");
        assert!(
            !job.output_path.exists(),
            "no new video should be left behind"
        );
        assert!(
            !job.sidecar_path.exists(),
            "no new sidecar should be left behind"
        );
        assert!(!job.sidecar_path.with_extension("json.staged").exists());
    }
}
