//! AI モデルのダウンロード・検証・パス管理。
//!
//! モデルは `%APPDATA%/mimageviewer/models/` に保存される。
//! 初回使用時にバックグラウンドでダウンロードし、完了チェックする。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{AiError, ModelKind};

/// モデルのメタ情報（ダウンロード URL、ファイル名、サイズ）。
struct ModelInfo {
    filename: &'static str,
    url: &'static str,
    /// 概算サイズ（プログレスバー表示用）。実際はレスポンスの Content-Length を使う。
    approx_size_bytes: u64,
}

/// ダウンロード元のベース URL。
/// 開発中は HuggingFace の各リポジトリから直接取得する。
/// リリース時には GitHub Releases に複製して差し替える想定。
/// モデルのダウンロード URL。
/// GitHub Releases (models-v1) から配布。更新時は新しいタグ (models-v2 等) を作成し差し替える。
const MODEL_URLS: &[(ModelKind, &str, &str, u64)] = &[
    // (種類, ファイル名, ダウンロード URL, 概算サイズ)
    (ModelKind::ClassifierMobileNet,      "anime_classifier_mobilenetv3.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/anime_classifier_mobilenetv3.onnx",
        17_000_000),
    (ModelKind::UpscaleRealEsrganX4Plus,  "realesrgan_x4plus.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/realesrgan_x4plus.onnx",
        34_000_000),
    (ModelKind::UpscaleRealEsrganAnime6B, "realesrgan_x4plus_anime_6b.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/realesrgan_x4plus_anime_6b.onnx",
        18_000_000),
    // NOTE: waifu2x cunet は DirectML 非互換のため UI から除外。ダウンロード対象外。
    (ModelKind::UpscaleWaifu2xCunet,      "waifu2x_cunet_noise3_scale2x.onnx",
        "https://huggingface.co/deepghs/waifu2x_onnx/resolve/main/20250502/onnx_models/cunet/art/noise3_scale2x.onnx",
        6_000_000),
    (ModelKind::UpscaleRealEsrGeneralV3,  "realesr_general_x4v3.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/realesr_general_x4v3.onnx",
        5_000_000),
    (ModelKind::UpscaleRealCugan4x,       "realcugan_4x_conservative.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/realcugan_4x_conservative.onnx",
        6_000_000),
    (ModelKind::DenoiseRealplksr,         "dejpg_realplksr_otf.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/dejpg_realplksr_otf.onnx",
        28_000_000),
    (ModelKind::InpaintMiGan,             "migan.onnx",
        "https://github.com/MikageSawatari/mimageviewer/releases/download/models-v1/migan.onnx",
        30_000_000),
];

/// 各モデルのメタ情報を返す。
fn model_info(kind: ModelKind) -> ModelInfo {
    for &(k, filename, url, size) in MODEL_URLS {
        if k == kind {
            return ModelInfo {
                filename,
                url,
                approx_size_bytes: size,
            };
        }
    }
    // フォールバック（到達しないはず）
    ModelInfo {
        filename: "unknown.onnx",
        url: "",
        approx_size_bytes: 0,
    }
}

/// モデルのダウンロード状態。
#[derive(Debug, Clone)]
pub enum DownloadState {
    /// 未ダウンロード
    NotDownloaded,
    /// ダウンロード中
    Downloading {
        progress: Arc<AtomicU64>,
        total: Arc<AtomicU64>,
        /// ダウンロードスレッドからのエラーメッセージ
        error: Arc<Mutex<Option<String>>>,
        cancel: Arc<AtomicBool>,
    },
    /// ダウンロード完了・利用可能
    Ready(PathBuf),
    /// ダウンロード失敗
    Failed(String),
}

/// モデル管理マネージャ。
pub struct ModelManager {
    models_dir: PathBuf,
    states: Mutex<HashMap<ModelKind, DownloadState>>,
}

/// 全 ModelKind の一覧。
const ALL_MODELS: &[ModelKind] = &[
    ModelKind::ClassifierMobileNet,
    ModelKind::UpscaleRealEsrganX4Plus,
    ModelKind::UpscaleRealEsrganAnime6B,
    ModelKind::UpscaleWaifu2xCunet,
    ModelKind::UpscaleRealEsrGeneralV3,
    ModelKind::UpscaleRealCugan4x,
    ModelKind::DenoiseRealplksr,
    ModelKind::InpaintMiGan,
];

impl ModelManager {
    /// 新しい ModelManager を作成する。
    ///
    /// モデル探索パス:
    /// 1. `%APPDATA%/mimageviewer/models/` (ダウンロード先)
    /// 2. `exe と同階層の models/` (開発時にリポジトリ内のモデルを使う)
    pub fn new() -> Self {
        let models_dir = crate::data_dir::get().join("models");
        let mut states = HashMap::new();

        // exe と同じディレクトリの models/ も探索する（開発用）
        let dev_models_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("models")))
            .or_else(|| {
                // cargo run 時はカレントディレクトリに models/ がある
                std::env::current_dir().ok().map(|d| d.join("models"))
            });

        for &kind in ALL_MODELS {
            let info = model_info(kind);

            // 優先: %APPDATA% → 開発用ディレクトリ
            let mut candidates = vec![models_dir.join(info.filename)];
            if let Some(ref dev) = dev_models_dir {
                candidates.push(dev.join(info.filename));
            }

            let found = candidates.into_iter().find(|p| {
                p.exists() && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
            });

            if let Some(path) = found {
                states.insert(kind, DownloadState::Ready(path));
            } else {
                states.insert(kind, DownloadState::NotDownloaded);
            }
        }

        ModelManager {
            models_dir,
            states: Mutex::new(states),
        }
    }

    /// モデルファイルのパスを返す（存在する場合のみ）。
    pub fn model_path(&self, kind: ModelKind) -> Option<PathBuf> {
        let states = self.states.lock().unwrap();
        match states.get(&kind) {
            Some(DownloadState::Ready(p)) => Some(p.clone()),
            _ => None,
        }
    }

    /// モデルが利用可能か確認する。
    #[allow(dead_code)]
    pub fn is_ready(&self, kind: ModelKind) -> bool {
        matches!(
            self.states.lock().unwrap().get(&kind),
            Some(DownloadState::Ready(_))
        )
    }

    /// モデルのダウンロード状態を返す。
    pub fn download_state(&self, kind: ModelKind) -> DownloadState {
        self.states
            .lock()
            .unwrap()
            .get(&kind)
            .cloned()
            .unwrap_or(DownloadState::NotDownloaded)
    }

    /// 指定モデル群のうち、未ダウンロードのものを返す。
    pub fn missing_models(&self, models: &[ModelKind]) -> Vec<ModelKind> {
        let states = self.states.lock().unwrap();
        models.iter()
            .filter(|&&kind| !matches!(states.get(&kind), Some(DownloadState::Ready(_))))
            .copied()
            .collect()
    }

    /// アップスケールに必要なモデル一覧のうち、未ダウンロードのものを返す。
    /// 分類器 (ClassifierMobileNet) も含む。
    pub fn missing_upscale_models(&self) -> Vec<ModelKind> {
        let mut all: Vec<ModelKind> = vec![ModelKind::ClassifierMobileNet];
        all.extend_from_slice(ModelKind::upscale_models());
        self.missing_models(&all)
    }

    /// ノイズ除去に必要なモデル一覧のうち、未ダウンロードのものを返す。
    pub fn missing_denoise_models(&self) -> Vec<ModelKind> {
        self.missing_models(ModelKind::denoise_models())
    }

    /// 見開き補完に必要なモデル一覧のうち、未ダウンロードのものを返す。
    pub fn missing_inpaint_models(&self) -> Vec<ModelKind> {
        self.missing_models(&[ModelKind::InpaintMiGan])
    }

    /// 指定されたモデル群のうち未ダウンロードのものをダウンロード開始する。
    pub fn start_download_models(&self, models: &[ModelKind]) {
        for &kind in models {
            if matches!(self.download_state(kind), DownloadState::NotDownloaded | DownloadState::Failed(_)) {
                self.start_download(kind);
            }
        }
    }

    /// 指定モデルのダウンロードサイズ（バイト）を返す。
    pub fn model_size(kind: ModelKind) -> u64 {
        model_info(kind).approx_size_bytes
    }

    /// 複数モデルの合計ダウンロードサイズ。
    #[allow(dead_code)]
    pub fn total_download_size(models: &[ModelKind]) -> u64 {
        models.iter().map(|&k| Self::model_size(k)).sum()
    }

    /// バックグラウンドでモデルをダウンロードする。
    pub fn start_download(
        &self,
        kind: ModelKind,
    ) -> (Arc<AtomicU64>, Arc<AtomicBool>) {
        let progress = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let info = model_info(kind);
        total.store(info.approx_size_bytes, Ordering::Relaxed);

        // 状態を Downloading に更新
        {
            let mut states = self.states.lock().unwrap();
            states.insert(
                kind,
                DownloadState::Downloading {
                    progress: progress.clone(),
                    total: total.clone(),
                    cancel: cancel.clone(),
                    error: error.clone(),
                },
            );
        }

        let models_dir = self.models_dir.clone();
        let progress_clone = progress.clone();
        let total_clone = total.clone();
        let cancel_clone = cancel.clone();
        let error_clone = error.clone();
        let url = info.url.to_string();
        let filename = info.filename.to_string();

        // NOTE: ダウンロード完了時の状態更新は poll_downloads() で行う
        std::thread::spawn(move || {
            let result = download_file(
                &url,
                &models_dir,
                &filename,
                &progress_clone,
                &total_clone,
                &cancel_clone,
            );
            match result {
                Ok(path) => {
                    crate::logger::log(format!(
                        "[AI] Model {} downloaded to {}",
                        filename,
                        path.display()
                    ));
                }
                Err(e) => {
                    let msg = format!("{e}");
                    crate::logger::log(format!("[AI] Download failed for {}: {}", filename, msg));
                    *error_clone.lock().unwrap() = Some(msg);
                }
            }
        });

        (progress, cancel)
    }

    /// ダウンロード状態をポーリングして更新する。
    /// ダウンロード完了時に Ready / Failed に遷移させる。
    /// ダウンロード中のモデルがあるか（poll_downloads のガード用）。
    pub fn has_active_downloads(&self) -> bool {
        self.states.lock().unwrap().values().any(|s| matches!(s, DownloadState::Downloading { .. }))
    }

    pub fn poll_downloads(&self) {
        let mut states = self.states.lock().unwrap();
        let kinds: Vec<ModelKind> = states.keys().copied().collect();

        for kind in kinds {
            let should_update = match states.get(&kind) {
                Some(DownloadState::Downloading { cancel, progress, total, error }) => {
                    if cancel.load(Ordering::Relaxed) {
                        Some(DownloadState::Failed("キャンセルされました".to_string()))
                    } else if let Some(err_msg) = error.lock().unwrap().take() {
                        // ダウンロードスレッドがエラーを報告した
                        Some(DownloadState::Failed(err_msg))
                    } else {
                        let info = model_info(kind);
                        let path = self.models_dir.join(info.filename);
                        // .tmp ファイルが消えて本体ファイルがあればダウンロード完了
                        let tmp = self.models_dir.join(format!("{}.tmp", info.filename));
                        if path.exists() && !tmp.exists() {
                            match std::fs::metadata(&path) {
                                Ok(meta) if meta.len() > 0 => {
                                    Some(DownloadState::Ready(path))
                                }
                                _ => None,
                            }
                        } else {
                            // progress == total (実際のサイズ) でも完了チェック
                            let p = progress.load(Ordering::Relaxed);
                            let t = total.load(Ordering::Relaxed);
                            if t > 0 && p >= t && path.exists() {
                                Some(DownloadState::Ready(path))
                            } else {
                                None
                            }
                        }
                    }
                }
                _ => None,
            };

            if let Some(new_state) = should_update {
                states.insert(kind, new_state);
            }
        }
    }

    /// 未ダウンロードのモデルを全てダウンロード開始する。
    pub fn start_download_all_missing(&self) {
        let missing: Vec<ModelKind> = {
            let states = self.states.lock().unwrap();
            states.iter()
                .filter(|(_, s)| matches!(s, DownloadState::NotDownloaded | DownloadState::Failed(_)))
                .map(|(&k, _)| k)
                .collect()
        };

        for kind in missing {
            self.start_download(kind);
        }
    }
}

/// ureq を使ってファイルをダウンロードする。
/// ストリーミングでチャンクごとに書き込み、進捗を報告する。
fn download_file(
    url: &str,
    dir: &Path,
    filename: &str,
    progress: &AtomicU64,
    total: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<PathBuf, AiError> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(filename);
    let tmp = dir.join(format!("{filename}.tmp"));

    crate::logger::log(format!("[AI] Downloading {} from {}", filename, url));

    // HTTP GET（リダイレクト自動追従）
    let response = ureq::get(url)
        .call()
        .map_err(|e| AiError::DownloadFailed(format!("HTTP request failed: {e}")))?;

    // Content-Length があれば total を更新
    if let Some(content_length) = response.headers().get("content-length") {
        if let Ok(len_str) = content_length.to_str() {
            if let Ok(len) = len_str.parse::<u64>() {
                total.store(len, Ordering::Relaxed);
            }
        }
    }

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&tmp)?;
    let mut buf = [0u8; 65536]; // 64KB chunks
    let mut downloaded: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(AiError::Cancelled);
        }

        let n = reader.read(&mut buf)
            .map_err(|e| AiError::DownloadFailed(format!("Read error: {e}")))?;
        if n == 0 {
            break; // EOF
        }

        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;
        progress.store(downloaded, Ordering::Relaxed);
    }

    std::io::Write::flush(&mut file)?;
    drop(file);

    // tmp → 最終ファイルにリネーム（上書き対応）
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }
    std::fs::rename(&tmp, &dest)?;

    crate::logger::log(format!(
        "[AI] Download complete: {} ({} bytes)",
        filename, downloaded
    ));

    Ok(dest)
}
