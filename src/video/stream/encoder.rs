use std::{fmt, str::FromStr};

use ffmpeg_the_third as ffmpeg;
use serde_json::Value;

use super::quality::StreamOutputParameters;

/// 正本 §4.4 の segment/GOP 長。後続セグメンタもこの値を共有する。
pub(crate) const SEGMENT_DURATION_SECS: u32 = 2;
pub(crate) const H264_PROFILE: &str = "high";
pub(crate) const H264_BIT_DEPTH: u8 = 8;
pub(crate) const VIDEO_COLOR_SPACE: &str = "bt709";
pub(crate) const AUDIO_ENCODER_NAME: &str = "aac";
pub(crate) const AUDIO_PROFILE: &str = "aac-lc";
pub(crate) const AUDIO_PROFILE_ID: i32 = ffmpeg::ffi::AV_PROFILE_AAC_LOW;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum H264EncoderKind {
    Nvenc,
    Qsv,
    Amf,
    MediaFoundation,
    OpenH264,
}

impl H264EncoderKind {
    pub(crate) const AUTO_ORDER: [Self; 5] = [
        Self::Nvenc,
        Self::Qsv,
        Self::Amf,
        Self::MediaFoundation,
        Self::OpenH264,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
            Self::MediaFoundation => "h264_mf",
            Self::OpenH264 => "libopenh264",
        }
    }
}

impl fmt::Display for H264EncoderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EncoderPreference {
    #[default]
    Auto,
    Encoder(H264EncoderKind),
}

impl FromStr for EncoderPreference {
    type Err = EncoderPreferenceParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        H264EncoderKind::AUTO_ORDER
            .into_iter()
            .find(|encoder| value.eq_ignore_ascii_case(encoder.as_str()))
            .map(Self::Encoder)
            .ok_or_else(|| EncoderPreferenceParseError(value.to_owned()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EncoderPreferenceParseError(String);

impl fmt::Display for EncoderPreferenceParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown H.264 encoder preference: {}", self.0)
    }
}

impl std::error::Error for EncoderPreferenceParseError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameRate {
    pub(crate) numerator: u32,
    pub(crate) denominator: u32,
}

impl FrameRate {
    pub(crate) fn new(numerator: u32, denominator: u32) -> Result<Self, String> {
        if numerator == 0 || denominator == 0 {
            return Err("frame rate numerator and denominator must be non-zero".to_owned());
        }
        if numerator > i32::MAX as u32 || denominator > i32::MAX as u32 {
            return Err("frame rate components exceed FFmpeg rational range".to_owned());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub(crate) fn keyint_frames(self) -> u32 {
        let numerator = u64::from(self.numerator) * u64::from(SEGMENT_DURATION_SECS);
        let rounded = (numerator + u64::from(self.denominator) / 2) / u64::from(self.denominator);
        rounded.clamp(1, u64::from(u32::MAX)) as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum H264InputFormat {
    Nv12,
    Yuv420p,
}

impl H264InputFormat {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Nv12 => "nv12",
            Self::Yuv420p => "yuv420p",
        }
    }

    const fn ffmpeg_pixel(self) -> ffmpeg::format::Pixel {
        match self {
            Self::Nv12 => ffmpeg::format::Pixel::NV12,
            Self::Yuv420p => ffmpeg::format::Pixel::YUV420P,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EncoderAttemptFailureKind {
    NotFound,
    OpenFailed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EncoderAttemptFailure {
    pub(crate) encoder: H264EncoderKind,
    pub(crate) kind: EncoderAttemptFailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum H264EncoderOpenErrorCode {
    BackendInitializationFailed,
    NoAutoCandidatesFound,
    AutoCandidatesExhausted,
    ExplicitEncoderNotFound,
    ExplicitEncoderOpenFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum H264EncoderOpenError {
    BackendInitializationFailed {
        reason: String,
    },
    NoAutoCandidatesFound {
        attempts: Vec<EncoderAttemptFailure>,
    },
    AutoCandidatesExhausted {
        attempts: Vec<EncoderAttemptFailure>,
    },
    ExplicitEncoderNotFound {
        encoder: H264EncoderKind,
    },
    ExplicitEncoderOpenFailed {
        encoder: H264EncoderKind,
        reason: String,
    },
}

impl H264EncoderOpenError {
    pub(crate) const fn code(&self) -> H264EncoderOpenErrorCode {
        match self {
            Self::BackendInitializationFailed { .. } => {
                H264EncoderOpenErrorCode::BackendInitializationFailed
            }
            Self::NoAutoCandidatesFound { .. } => H264EncoderOpenErrorCode::NoAutoCandidatesFound,
            Self::AutoCandidatesExhausted { .. } => {
                H264EncoderOpenErrorCode::AutoCandidatesExhausted
            }
            Self::ExplicitEncoderNotFound { .. } => {
                H264EncoderOpenErrorCode::ExplicitEncoderNotFound
            }
            Self::ExplicitEncoderOpenFailed { .. } => {
                H264EncoderOpenErrorCode::ExplicitEncoderOpenFailed
            }
        }
    }
}

impl fmt::Display for H264EncoderOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendInitializationFailed { reason } => {
                write!(f, "FFmpeg initialization failed: {reason}")
            }
            Self::NoAutoCandidatesFound { .. } => {
                f.write_str("no configured H.264 encoders were found")
            }
            Self::AutoCandidatesExhausted { attempts } => {
                write!(f, "all H.264 encoders failed ({} attempts)", attempts.len())
            }
            Self::ExplicitEncoderNotFound { encoder } => {
                write!(f, "explicit H.264 encoder was not found: {encoder}")
            }
            Self::ExplicitEncoderOpenFailed { encoder, reason } => write!(
                f,
                "explicit H.264 encoder {encoder} failed to open: {reason}"
            ),
        }
    }
}

impl std::error::Error for H264EncoderOpenError {}

pub(crate) struct BackendOpened<T> {
    value: T,
    input_format: H264InputFormat,
    effective_video_bitrate_bps: u64,
}

pub(crate) enum BackendOpenAttempt<T> {
    Opened(BackendOpened<T>),
    NotFound,
    OpenFailed(String),
}

/// `find_by_name` と実 open を一候補単位に抽象化する。production は FFmpeg、test は
/// scripted backend を注入するため、階段の状態遷移は GPU/DLL 非依存で検証できる。
pub(crate) trait H264EncoderBackend {
    type Opened;

    fn try_open(
        &mut self,
        encoder: H264EncoderKind,
        output: StreamOutputParameters,
        frame_rate: FrameRate,
    ) -> BackendOpenAttempt<Self::Opened>;
}

#[derive(Debug)]
pub(crate) struct SelectedH264Encoder<T> {
    pub(crate) kind: H264EncoderKind,
    pub(crate) value: T,
    pub(crate) input_format: H264InputFormat,
    pub(crate) effective_video_bitrate_bps: u64,
}

pub(crate) fn select_h264_encoder<B: H264EncoderBackend>(
    preference: EncoderPreference,
    output: StreamOutputParameters,
    frame_rate: FrameRate,
    backend: &mut B,
) -> Result<SelectedH264Encoder<B::Opened>, H264EncoderOpenError> {
    if let EncoderPreference::Encoder(encoder) = preference {
        return match backend.try_open(encoder, output, frame_rate) {
            BackendOpenAttempt::Opened(opened) => Ok(SelectedH264Encoder {
                kind: encoder,
                value: opened.value,
                input_format: opened.input_format,
                effective_video_bitrate_bps: opened.effective_video_bitrate_bps,
            }),
            BackendOpenAttempt::NotFound => {
                Err(H264EncoderOpenError::ExplicitEncoderNotFound { encoder })
            }
            BackendOpenAttempt::OpenFailed(reason) => {
                Err(H264EncoderOpenError::ExplicitEncoderOpenFailed { encoder, reason })
            }
        };
    }

    let mut attempts = Vec::with_capacity(H264EncoderKind::AUTO_ORDER.len());
    for encoder in H264EncoderKind::AUTO_ORDER {
        match backend.try_open(encoder, output, frame_rate) {
            BackendOpenAttempt::Opened(opened) => {
                return Ok(SelectedH264Encoder {
                    kind: encoder,
                    value: opened.value,
                    input_format: opened.input_format,
                    effective_video_bitrate_bps: opened.effective_video_bitrate_bps,
                });
            }
            BackendOpenAttempt::NotFound => attempts.push(EncoderAttemptFailure {
                encoder,
                kind: EncoderAttemptFailureKind::NotFound,
            }),
            BackendOpenAttempt::OpenFailed(reason) => attempts.push(EncoderAttemptFailure {
                encoder,
                kind: EncoderAttemptFailureKind::OpenFailed { reason },
            }),
        }
    }

    if attempts
        .iter()
        .all(|attempt| matches!(attempt.kind, EncoderAttemptFailureKind::NotFound))
    {
        Err(H264EncoderOpenError::NoAutoCandidatesFound { attempts })
    } else {
        Err(H264EncoderOpenError::AutoCandidatesExhausted { attempts })
    }
}

pub(crate) struct OpenedH264Encoder {
    pub(crate) kind: H264EncoderKind,
    pub(crate) encoder: ffmpeg::codec::encoder::video::Encoder,
    pub(crate) input_format: H264InputFormat,
    pub(crate) effective_video_bitrate_bps: u64,
    pub(crate) keyint_frames: u32,
}

pub(crate) fn open_h264_encoder(
    preference: EncoderPreference,
    output: StreamOutputParameters,
    frame_rate: FrameRate,
) -> Result<OpenedH264Encoder, H264EncoderOpenError> {
    ffmpeg::init().map_err(|error| H264EncoderOpenError::BackendInitializationFailed {
        reason: error.to_string(),
    })?;
    let selected = select_h264_encoder(
        preference,
        output,
        frame_rate,
        &mut FfmpegH264EncoderBackend,
    )?;
    let keyint_frames = frame_rate.keyint_frames();
    crate::logger::log(format!(
        "remote-stream encoder selected: encoder={} video_bitrate_bps={} audio_bitrate_bps={} output={}x{} input_format={} profile={} bit_depth={} color_space={} keyint_frames={}",
        selected.kind,
        selected.effective_video_bitrate_bps,
        output.audio_bitrate_bps,
        output.dimensions.width,
        output.dimensions.height,
        selected.input_format.as_str(),
        H264_PROFILE,
        H264_BIT_DEPTH,
        VIDEO_COLOR_SPACE,
        keyint_frames,
    ));
    crate::perf::event(
        "remote_stream",
        "encoder_selected",
        None,
        0,
        &[
            ("encoder", Value::from(selected.kind.as_str())),
            (
                "video_bitrate_bps",
                Value::from(selected.effective_video_bitrate_bps),
            ),
            ("audio_bitrate_bps", Value::from(output.audio_bitrate_bps)),
            ("width", Value::from(output.dimensions.width)),
            ("height", Value::from(output.dimensions.height)),
            ("keyint_frames", Value::from(keyint_frames)),
        ],
    );
    Ok(OpenedH264Encoder {
        kind: selected.kind,
        encoder: selected.value,
        input_format: selected.input_format,
        effective_video_bitrate_bps: selected.effective_video_bitrate_bps,
        keyint_frames,
    })
}

struct FfmpegH264EncoderBackend;

impl H264EncoderBackend for FfmpegH264EncoderBackend {
    type Opened = ffmpeg::codec::encoder::video::Encoder;

    fn try_open(
        &mut self,
        encoder: H264EncoderKind,
        output: StreamOutputParameters,
        frame_rate: FrameRate,
    ) -> BackendOpenAttempt<Self::Opened> {
        let Some(codec) = ffmpeg::codec::encoder::find_by_name(encoder.as_str()) else {
            crate::logger::log(format!(
                "remote-stream encoder probe: encoder={encoder} result=not_found"
            ));
            return BackendOpenAttempt::NotFound;
        };
        let Some(input_format) = choose_input_format(codec, encoder) else {
            let reason = "encoder has no supported 8-bit 4:2:0 input format".to_owned();
            crate::logger::log(format!(
                "remote-stream encoder probe: encoder={encoder} result=open_failed reason={reason}"
            ));
            return BackendOpenAttempt::OpenFailed(reason);
        };

        match open_ffmpeg_encoder(codec, encoder, input_format, output, frame_rate) {
            Ok(opened) => {
                let effective_video_bitrate_bps =
                    unsafe { (*opened.as_ptr()).bit_rate.max(0) as u64 };
                BackendOpenAttempt::Opened(BackendOpened {
                    value: opened,
                    input_format,
                    effective_video_bitrate_bps,
                })
            }
            Err(error) => {
                let reason = error.to_string();
                crate::logger::log(format!(
                    "remote-stream encoder probe: encoder={encoder} result=open_failed reason={reason}"
                ));
                BackendOpenAttempt::OpenFailed(reason)
            }
        }
    }
}

fn choose_input_format(codec: ffmpeg::Codec, encoder: H264EncoderKind) -> Option<H264InputFormat> {
    let codec = codec.video()?;
    let supported = codec.formats().map(|formats| formats.collect::<Vec<_>>());
    let preference = if encoder == H264EncoderKind::OpenH264 {
        [H264InputFormat::Yuv420p, H264InputFormat::Nv12]
    } else {
        [H264InputFormat::Nv12, H264InputFormat::Yuv420p]
    };
    preference.into_iter().find(|format| {
        supported
            .as_ref()
            .is_none_or(|formats| formats.contains(&format.ffmpeg_pixel()))
    })
}

fn open_ffmpeg_encoder(
    codec: ffmpeg::Codec,
    encoder: H264EncoderKind,
    input_format: H264InputFormat,
    output: StreamOutputParameters,
    frame_rate: FrameRate,
) -> Result<ffmpeg::codec::encoder::video::Encoder, ffmpeg::Error> {
    let mut context = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    context.set_width(output.dimensions.width);
    context.set_height(output.dimensions.height);
    context.set_format(input_format.ffmpeg_pixel());
    context.set_bit_rate(output.video_bitrate_bps as usize);
    context.set_time_base((frame_rate.denominator as i32, frame_rate.numerator as i32));
    context.set_frame_rate(Some((
        frame_rate.numerator as i32,
        frame_rate.denominator as i32,
    )));
    let keyint_frames = frame_rate.keyint_frames();

    // GOP は 2 秒セグメントと同じ長さに固定し、最小 keyint も同値にして scene change
    // で短縮させない。後続セグメンタは各 2 秒境界の frame を明示的に key frame 指定する。
    // `forced-idr` 対応 encoder では、その指定を I-frame ではなく IDR にする。これにより
    // 各 CMAF segment が必ず単独デコード可能な IDR から始まる、という前提を固定する。
    context.set_gop(keyint_frames);
    context.set_colorspace(ffmpeg::color::Space::BT709);
    context.set_color_range(ffmpeg::color::Range::MPEG);
    unsafe {
        let raw = context.as_mut_ptr();
        (*raw).keyint_min = keyint_frames as i32;
        (*raw).profile = ffmpeg::ffi::AV_PROFILE_H264_HIGH;
        (*raw).bits_per_raw_sample = i32::from(H264_BIT_DEPTH);
        (*raw).color_primaries = ffmpeg::ffi::AVColorPrimaries::AVCOL_PRI_BT709;
        (*raw).color_trc = ffmpeg::ffi::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
    }

    let mut options = ffmpeg::Dictionary::new();
    match encoder {
        H264EncoderKind::Nvenc => {
            options.set("profile", H264_PROFILE);
            options.set("no-scenecut", "1");
            options.set("forced-idr", "1");
        }
        H264EncoderKind::Qsv => options.set("forced_idr", "1"),
        H264EncoderKind::Amf => {
            options.set("profile", H264_PROFILE);
            options.set("gops_per_idr", "1");
            options.set("header_insertion_mode", "gop");
        }
        H264EncoderKind::OpenH264 => {
            // OpenH264 warns that bitrate mode cannot enforce the target unless frame skipping
            // is enabled. The fallback exists for constrained links, so prefer the configured
            // bitrate over preserving every source frame under encoder overload.
            options.set("allow_skip_frames", "1");
        }
        H264EncoderKind::MediaFoundation => {}
    }
    context.open_as_with(codec, options)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::video::stream::quality::{OutputDimensions, QualityPreset};

    #[derive(Clone, Debug)]
    enum ScriptedAttempt {
        Opened(u8),
        NotFound,
        Failed(&'static str),
    }

    struct ScriptedBackend {
        attempts: HashMap<H264EncoderKind, ScriptedAttempt>,
        calls: Vec<H264EncoderKind>,
    }

    impl ScriptedBackend {
        fn new(entries: impl IntoIterator<Item = (H264EncoderKind, ScriptedAttempt)>) -> Self {
            Self {
                attempts: entries.into_iter().collect(),
                calls: Vec::new(),
            }
        }
    }

    impl H264EncoderBackend for ScriptedBackend {
        type Opened = u8;

        fn try_open(
            &mut self,
            encoder: H264EncoderKind,
            _output: StreamOutputParameters,
            _frame_rate: FrameRate,
        ) -> BackendOpenAttempt<Self::Opened> {
            self.calls.push(encoder);
            match self
                .attempts
                .remove(&encoder)
                .unwrap_or(ScriptedAttempt::NotFound)
            {
                ScriptedAttempt::Opened(value) => BackendOpenAttempt::Opened(BackendOpened {
                    value,
                    input_format: H264InputFormat::Nv12,
                    effective_video_bitrate_bps: 1_500_000,
                }),
                ScriptedAttempt::NotFound => BackendOpenAttempt::NotFound,
                ScriptedAttempt::Failed(reason) => {
                    BackendOpenAttempt::OpenFailed(reason.to_owned())
                }
            }
        }
    }

    fn output() -> StreamOutputParameters {
        StreamOutputParameters {
            dimensions: OutputDimensions {
                width: 1_280,
                height: 720,
            },
            video_bitrate_bps: 1_500_000,
            audio_bitrate_bps: 128_000,
        }
    }

    fn fps_30() -> FrameRate {
        FrameRate::new(30, 1).unwrap()
    }

    #[test]
    fn auto_fallback_stops_at_first_success() {
        let mut backend = ScriptedBackend::new([
            (H264EncoderKind::Nvenc, ScriptedAttempt::Failed("no device")),
            (H264EncoderKind::Qsv, ScriptedAttempt::NotFound),
            (H264EncoderKind::Amf, ScriptedAttempt::Opened(7)),
            (H264EncoderKind::MediaFoundation, ScriptedAttempt::Opened(8)),
        ]);
        let selected =
            select_h264_encoder(EncoderPreference::Auto, output(), fps_30(), &mut backend).unwrap();
        assert_eq!(selected.kind, H264EncoderKind::Amf);
        assert_eq!(selected.value, 7);
        assert_eq!(
            backend.calls,
            vec![
                H264EncoderKind::Nvenc,
                H264EncoderKind::Qsv,
                H264EncoderKind::Amf,
            ]
        );
    }

    #[test]
    fn auto_reports_no_candidates_when_none_are_found() {
        let mut backend = ScriptedBackend::new([]);
        let error = select_h264_encoder(EncoderPreference::Auto, output(), fps_30(), &mut backend)
            .unwrap_err();
        assert_eq!(
            error.code(),
            H264EncoderOpenErrorCode::NoAutoCandidatesFound
        );
        assert!(matches!(error,
            H264EncoderOpenError::NoAutoCandidatesFound { ref attempts }
                if attempts.len() == H264EncoderKind::AUTO_ORDER.len()));
        assert_eq!(backend.calls, H264EncoderKind::AUTO_ORDER);
    }

    #[test]
    fn auto_reports_every_failure_when_ladder_is_exhausted() {
        let entries = H264EncoderKind::AUTO_ORDER
            .map(|encoder| (encoder, ScriptedAttempt::Failed("open rejected")));
        let mut backend = ScriptedBackend::new(entries);
        let error = select_h264_encoder(EncoderPreference::Auto, output(), fps_30(), &mut backend)
            .unwrap_err();
        assert_eq!(
            error.code(),
            H264EncoderOpenErrorCode::AutoCandidatesExhausted
        );
        assert!(matches!(error,
            H264EncoderOpenError::AutoCandidatesExhausted { ref attempts }
                if attempts.len() == H264EncoderKind::AUTO_ORDER.len()));
    }

    #[test]
    fn explicit_open_failure_never_falls_back() {
        let mut backend = ScriptedBackend::new([
            (
                H264EncoderKind::Qsv,
                ScriptedAttempt::Failed("driver rejected"),
            ),
            (H264EncoderKind::Amf, ScriptedAttempt::Opened(9)),
        ]);
        let error = select_h264_encoder(
            EncoderPreference::Encoder(H264EncoderKind::Qsv),
            output(),
            fps_30(),
            &mut backend,
        )
        .unwrap_err();
        assert_eq!(
            error,
            H264EncoderOpenError::ExplicitEncoderOpenFailed {
                encoder: H264EncoderKind::Qsv,
                reason: "driver rejected".to_owned(),
            }
        );
        assert_eq!(backend.calls, vec![H264EncoderKind::Qsv]);
    }

    #[test]
    fn explicit_missing_encoder_never_falls_back() {
        let mut backend = ScriptedBackend::new([
            (H264EncoderKind::Qsv, ScriptedAttempt::NotFound),
            (H264EncoderKind::Amf, ScriptedAttempt::Opened(9)),
        ]);
        let error = select_h264_encoder(
            EncoderPreference::Encoder(H264EncoderKind::Qsv),
            output(),
            fps_30(),
            &mut backend,
        )
        .unwrap_err();
        assert_eq!(
            error,
            H264EncoderOpenError::ExplicitEncoderNotFound {
                encoder: H264EncoderKind::Qsv,
            }
        );
        assert_eq!(backend.calls, vec![H264EncoderKind::Qsv]);
    }

    #[test]
    fn preference_parser_accepts_canonical_values() {
        assert_eq!("Auto".parse(), Ok(EncoderPreference::Auto));
        for encoder in H264EncoderKind::AUTO_ORDER {
            assert_eq!(
                encoder.as_str().parse(),
                Ok(EncoderPreference::Encoder(encoder))
            );
        }
        assert!("h264_unknown".parse::<EncoderPreference>().is_err());
    }

    #[test]
    fn keyint_matches_one_two_second_gop() {
        assert_eq!(FrameRate::new(24, 1).unwrap().keyint_frames(), 48);
        assert_eq!(FrameRate::new(30, 1).unwrap().keyint_frames(), 60);
        assert_eq!(FrameRate::new(60, 1).unwrap().keyint_frames(), 120);
        assert_eq!(FrameRate::new(30_000, 1_001).unwrap().keyint_frames(), 60);
        assert!(FrameRate::new(0, 1).is_err());
    }

    #[test]
    fn codec_contract_is_h264_high_8bit_bt709_and_aac_lc() {
        assert_eq!(H264_PROFILE, "high");
        assert_eq!(H264_BIT_DEPTH, 8);
        assert_eq!(VIDEO_COLOR_SPACE, "bt709");
        assert_eq!(AUDIO_ENCODER_NAME, "aac");
        assert_eq!(AUDIO_PROFILE, "aac-lc");
        assert_eq!(AUDIO_PROFILE_ID, ffmpeg::ffi::AV_PROFILE_AAC_LOW);
    }

    /// 明示実行用の開発機 probe。通常の test suite は hardware/DLL に依存しない。
    ///
    /// `cargo test -p mimageviewer --lib probe_local_h264_encoder_open_results -- --ignored --nocapture`
    #[test]
    #[ignore = "requires local FFmpeg DLLs and encoder hardware"]
    fn probe_local_h264_encoder_open_results() {
        ffmpeg::init().expect("FFmpeg init");
        let output = QualityPreset::Standard
            .output_parameters(1_280, 720)
            .unwrap();
        for encoder in H264EncoderKind::AUTO_ORDER {
            match FfmpegH264EncoderBackend.try_open(encoder, output, fps_30()) {
                BackendOpenAttempt::Opened(opened) => unsafe {
                    let context = &*opened.value.as_ptr();
                    println!(
                        "{encoder}: OPENED (format={}, bitrate={}, profile={}, bit_depth={}, color={:?}/{:?}/{:?}, gop={}/{})",
                        opened.input_format.as_str(),
                        opened.effective_video_bitrate_bps,
                        context.profile,
                        context.bits_per_raw_sample,
                        context.color_primaries,
                        context.color_trc,
                        context.colorspace,
                        context.gop_size,
                        context.keyint_min,
                    );
                },
                BackendOpenAttempt::NotFound => println!("{encoder}: NOT_FOUND"),
                BackendOpenAttempt::OpenFailed(reason) => {
                    println!("{encoder}: OPEN_FAILED ({reason})")
                }
            }
        }
    }
}
