//! 音楽ビデオのオフライン MP4 書き出し。
//!
//! この入口はブロッキング処理を行う。UI からは必ずワーカースレッド上で呼ぶこと。

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;

use musicvideo_core::{Assets, Project};

mod driver;
mod mux;

pub use driver::verify_exported_mp4;

/// H.264 エンコーダの選択。`Auto` は Remote 配信と同じ優先順を使う。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncoderPreference {
    #[default]
    Auto,
    Nvenc,
    Qsv,
    Amf,
    MediaFoundation,
    OpenH264,
}

impl FromStr for EncoderPreference {
    type Err = ExportError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "h264_nvenc" | "nvenc" => Ok(Self::Nvenc),
            "h264_qsv" | "qsv" => Ok(Self::Qsv),
            "h264_amf" | "amf" => Ok(Self::Amf),
            "h264_mf" | "mf" | "mediafoundation" => Ok(Self::MediaFoundation),
            "libopenh264" | "openh264" => Ok(Self::OpenH264),
            _ => Err(ExportError::InvalidRequest(format!(
                "不明な H.264 エンコーダです: {value}"
            ))),
        }
    }
}

pub struct MusicVideoExportRequest {
    pub project: Project,
    pub assets: Assets,
    pub audio_path: PathBuf,
    pub output_path: PathBuf,
    pub encoder: EncoderPreference,
    pub video_bitrate_bps: u32,
    /// 検証時だけ RGBA フレーム列の SHA-256 を計算する。
    pub compute_frame_hash: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportSummary {
    pub frames: u64,
    pub duration_secs: f64,
    pub encoder: String,
    pub render_mean_ms: f64,
    pub render_p95_ms: f64,
    pub convert_mean_ms: f64,
    pub encode_mean_ms: f64,
    pub audio_decode_secs: f64,
    pub aac_encode_secs: f64,
    pub mux_secs: f64,
    /// `av_write_trailer` による MP4 確定と `+faststart` の moov 移動。
    pub faststart_secs: f64,
    pub rename_secs: f64,
    /// `compute_frame_hash` が有効な検証時だけ発生する RGBA SHA-256 の計算。
    pub frame_hash_secs: f64,
    /// 上記区間とフレーム単位の 3 区間に含まれない初期化等。
    pub other_secs: f64,
    pub wall_secs: f64,
    pub realtime_ratio: f64,
    pub rgba_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mp4VideoVerification {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
    pub duration_secs: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mp4AudioVerification {
    pub codec: String,
    pub sample_rate: u32,
    pub samples_per_channel: u64,
    pub duration_secs: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mp4Verification {
    pub streams: usize,
    pub video: Mp4VideoVerification,
    pub audio: Mp4AudioVerification,
    pub duration_difference_secs: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    Cancelled,
    InvalidRequest(String),
    AudioDecode(String),
    Render(String),
    Encoder(String),
    Mux(String),
    Verification(String),
    Io(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("書き出しをキャンセルしました"),
            Self::InvalidRequest(message)
            | Self::AudioDecode(message)
            | Self::Render(message)
            | Self::Encoder(message)
            | Self::Mux(message)
            | Self::Verification(message)
            | Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ExportError {}

/// 音声を全尺デコードし、CFR の RGBA フレームを逐次 H.264/AAC MP4 へ書き出す。
pub fn export_music_video(
    request: MusicVideoExportRequest,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<ExportSummary, ExportError> {
    driver::export(request, progress, cancel)
}

fn io_error(operation: &str, path: &Path, error: std::io::Error) -> ExportError {
    ExportError::Io(format!("{operation} ({}): {error}", path.display()))
}
