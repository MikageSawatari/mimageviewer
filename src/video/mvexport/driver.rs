use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;
use musicvideo_core::{
    Frame, LayerKind, RenderReport, Smoothing, SpectrumSource, UnresolvedAsset, evaluate,
    render_frame,
};
use sha2::{Digest, Sha256};

use crate::video::audio::ProcessedChunk;
use crate::video::stream::audio_encoder::{OpenedAacEncoder, open_aac_encoder};
use crate::video::stream::encoder::{
    EncoderPreference as StreamEncoderPreference, FrameRate, H264EncoderKind, H264InputFormat,
    open_h264_encoder,
};
use crate::video::stream::quality::{OutputDimensions, StreamOutputParameters};
use crate::video::stream::timeline::StreamTimeline;

use super::mux::Mp4Muxer;
use super::{
    EncoderPreference, ExportError, ExportSummary, Mp4AudioVerification, Mp4Verification,
    Mp4VideoVerification, MusicVideoExportRequest, io_error,
};

const AUDIO_BITRATE_BPS: u32 = 192_000;
const AUDIO_CHUNK_SAMPLES_PER_CHANNEL: usize = 16_384;
const FFMPEG_EAGAIN: i32 = 11;

struct ExportPlan {
    frames: u64,
    frame_rate: FrameRate,
    spectrum_bands: usize,
}

pub(super) fn export(
    request: MusicVideoExportRequest,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<ExportSummary, ExportError> {
    let plan = validate_request(&request)?;
    progress(0, plan.frames);
    check_cancel(cancel)?;
    let wall_started = Instant::now();

    let mut partial = PartialOutput::new(&request.output_path)?;
    let mut decoded =
        crate::audio_decode::decode_audio_file_to_stereo_f32(&request.audio_path, cancel).map_err(
            |error| {
                if cancel.load(Ordering::Relaxed) {
                    ExportError::Cancelled
                } else {
                    ExportError::AudioDecode(format!("音声をデコードできません: {error}"))
                }
            },
        )?;
    check_cancel(cancel)?;
    fit_audio_to_duration(
        &mut decoded.stereo_samples,
        decoded.info.sample_rate,
        request.project.duration_secs,
    )?;

    let output = StreamOutputParameters {
        dimensions: OutputDimensions {
            width: request.project.output.width,
            height: request.project.output.height,
        },
        video_bitrate_bps: request.video_bitrate_bps,
        audio_bitrate_bps: AUDIO_BITRATE_BPS,
    };
    let mut video = open_h264_encoder(
        stream_encoder_preference(request.encoder),
        output,
        plan.frame_rate,
    )
    .map_err(|error| ExportError::Encoder(format!("H.264 エンコーダを開けません: {error}")))?;
    let selected_encoder = video.kind.as_str().to_owned();
    let timeline = StreamTimeline::new(0.0)
        .map_err(|error| ExportError::Encoder(format!("音声タイムラインが不正です: {error}")))?;
    let mut audio = open_aac_encoder(decoded.info.sample_rate, AUDIO_BITRATE_BPS, 0, timeline)
        .map_err(|error| ExportError::Encoder(format!("AAC エンコーダを開けません: {error}")))?;
    let mut muxer = Mp4Muxer::new(
        partial.path(),
        &video,
        &audio,
        (plan.frame_rate.numerator, plan.frame_rate.denominator),
    )?;

    let mut audio_packets = VecDeque::from(encode_audio(
        &mut audio,
        &decoded.stereo_samples,
        decoded.info.sample_rate,
        cancel,
    )?);
    let mut spectrum = SpectrumSource::new(
        decoded.info.sample_rate,
        plan.spectrum_bands,
        Smoothing::default(),
    )
    .map_err(|error| {
        ExportError::InvalidRequest(format!("スペクトラム解析を初期化できません: {error}"))
    })?;
    let mut rgba = Frame::new(request.project.output.width, request.project.output.height)
        .map_err(|error| ExportError::Render(format!("RGBA フレームを確保できません: {error}")))?;
    let mut converter = RgbaConverter::new(
        request.project.output.width,
        request.project.output.height,
        video.input_format,
    )?;
    let mut hasher = request.compute_frame_hash.then(Sha256::new);
    let mut render_ms = Vec::with_capacity(plan.frames as usize);
    let mut convert_total_secs = 0.0;
    let mut encode_total_secs = 0.0;

    for frame_index in 0..plan.frames {
        check_cancel(cancel)?;
        let t_secs = frame_index as f64 * plan.frame_rate.denominator as f64
            / plan.frame_rate.numerator as f64;

        let render_started = Instant::now();
        let spectrum_frame = spectrum
            .full_frame_at(&decoded.stereo_samples, t_secs, 0.0)
            .clone();
        let scene = evaluate(&request.project, t_secs, &spectrum_frame);
        let report = render_frame(&scene, &request.assets, &mut rgba).map_err(|error| {
            ExportError::Render(format!(
                "フレーム {} の合成に失敗しました: {error}",
                frame_index + 1
            ))
        })?;
        ensure_resolved(&report, frame_index)?;
        render_ms.push(render_started.elapsed().as_secs_f64() * 1000.0);

        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&rgba.rgba);
        }

        let convert_started = Instant::now();
        let mut converted = converter.convert(&rgba)?;
        converted.set_pts(Some(frame_index as i64));
        convert_total_secs += convert_started.elapsed().as_secs_f64();

        let encode_started = Instant::now();
        video.encoder.send_frame(&converted).map_err(|error| {
            ExportError::Encoder(format!("H.264 フレームを送れません: {error}"))
        })?;
        let packets = drain_h264_packets(&mut video.encoder)?;
        encode_total_secs += encode_started.elapsed().as_secs_f64();
        write_video_packets(&mut muxer, &mut audio_packets, packets)?;

        progress(frame_index + 1, plan.frames);
    }

    check_cancel(cancel)?;
    let flush_started = Instant::now();
    video.encoder.send_eof().map_err(|error| {
        ExportError::Encoder(format!("H.264 エンコーダを終了できません: {error}"))
    })?;
    let final_video_packets = drain_h264_packets(&mut video.encoder)?;
    encode_total_secs += flush_started.elapsed().as_secs_f64();
    write_video_packets(&mut muxer, &mut audio_packets, final_video_packets)?;
    while let Some(packet) = audio_packets.pop_front() {
        muxer.write_audio(packet)?;
    }
    muxer.finish()?;
    check_cancel(cancel)?;
    partial.commit(&request.output_path)?;

    let wall_secs = wall_started.elapsed().as_secs_f64();
    let frames = plan.frames as f64;
    let duration_secs =
        frames * plan.frame_rate.denominator as f64 / plan.frame_rate.numerator as f64;
    Ok(ExportSummary {
        frames: plan.frames,
        duration_secs,
        encoder: selected_encoder,
        render_mean_ms: mean(&render_ms),
        render_p95_ms: percentile(&render_ms, 0.95),
        convert_mean_ms: convert_total_secs * 1000.0 / frames,
        encode_mean_ms: encode_total_secs * 1000.0 / frames,
        wall_secs,
        realtime_ratio: if wall_secs > 0.0 {
            duration_secs / wall_secs
        } else {
            0.0
        },
        rgba_sha256: hasher.map(|hasher| format!("{:x}", hasher.finalize())),
    })
}

fn validate_request(request: &MusicVideoExportRequest) -> Result<ExportPlan, ExportError> {
    let output = request.project.output;
    if output.width < 2 || output.height < 2 || output.width % 2 != 0 || output.height % 2 != 0 {
        return Err(ExportError::InvalidRequest(
            "H.264 の出力幅と高さは 2 以上の偶数にしてください".to_owned(),
        ));
    }
    if !output.fps.is_finite() || output.fps <= 0.0 {
        return Err(ExportError::InvalidRequest(
            "fps は 0 より大きい有限値にしてください".to_owned(),
        ));
    }
    if !request.project.duration_secs.is_finite() || request.project.duration_secs <= 0.0 {
        return Err(ExportError::InvalidRequest(
            "書き出し時間は 0 より大きい有限値にしてください".to_owned(),
        ));
    }
    if request.video_bitrate_bps == 0 {
        return Err(ExportError::InvalidRequest(
            "映像ビットレートは 1 以上にしてください".to_owned(),
        ));
    }
    if request.output_path.exists() {
        return Err(ExportError::InvalidRequest(format!(
            "出力先は既に存在します: {}",
            request.output_path.display()
        )));
    }
    if !has_mp4_extension(&request.output_path) {
        return Err(ExportError::InvalidRequest(
            "出力先の拡張子は .mp4 にしてください".to_owned(),
        ));
    }
    let parent = request
        .output_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.is_dir() {
        return Err(ExportError::InvalidRequest(format!(
            "出力先フォルダーがありません: {}",
            parent.display()
        )));
    }

    let frame_rate = rational_frame_rate(output.fps)?;
    let frame_count = (request.project.duration_secs * output.fps).round();
    if !frame_count.is_finite() || frame_count < 1.0 || frame_count > i64::MAX as f64 {
        return Err(ExportError::InvalidRequest(
            "書き出しフレーム数を表現できません".to_owned(),
        ));
    }
    let spectrum_bands = request
        .project
        .layers
        .iter()
        .filter_map(|layer| match &layer.kind {
            LayerKind::Spectrum(layer) => Some(layer.band_count),
            _ => None,
        })
        .max()
        .unwrap_or(1);

    Ok(ExportPlan {
        frames: frame_count as u64,
        frame_rate,
        spectrum_bands,
    })
}

fn has_mp4_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
}

fn rational_frame_rate(fps: f64) -> Result<FrameRate, ExportError> {
    const SCALE: u64 = 1_000_000;
    let scaled = (fps * SCALE as f64).round();
    if !scaled.is_finite() || scaled < 1.0 || scaled > u64::MAX as f64 {
        return Err(ExportError::InvalidRequest(
            "fps を FFmpeg の有理数で表現できません".to_owned(),
        ));
    }
    let numerator = scaled as u64;
    let divisor = gcd(numerator, SCALE);
    let numerator = numerator / divisor;
    let denominator = SCALE / divisor;
    let numerator = u32::try_from(numerator).map_err(|_| {
        ExportError::InvalidRequest("fps の分子が FFmpeg の範囲を超えています".to_owned())
    })?;
    let denominator = u32::try_from(denominator).map_err(|_| {
        ExportError::InvalidRequest("fps の分母が FFmpeg の範囲を超えています".to_owned())
    })?;
    FrameRate::new(numerator, denominator).map_err(ExportError::InvalidRequest)
}

const fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn stream_encoder_preference(preference: EncoderPreference) -> StreamEncoderPreference {
    match preference {
        EncoderPreference::Auto => StreamEncoderPreference::Auto,
        EncoderPreference::Nvenc => StreamEncoderPreference::Encoder(H264EncoderKind::Nvenc),
        EncoderPreference::Qsv => StreamEncoderPreference::Encoder(H264EncoderKind::Qsv),
        EncoderPreference::Amf => StreamEncoderPreference::Encoder(H264EncoderKind::Amf),
        EncoderPreference::MediaFoundation => {
            StreamEncoderPreference::Encoder(H264EncoderKind::MediaFoundation)
        }
        EncoderPreference::OpenH264 => StreamEncoderPreference::Encoder(H264EncoderKind::OpenH264),
    }
}

fn fit_audio_to_duration(
    pcm: &mut Vec<f32>,
    sample_rate: u32,
    duration_secs: f64,
) -> Result<(), ExportError> {
    let samples_per_channel = (duration_secs * f64::from(sample_rate)).round();
    if !samples_per_channel.is_finite() || samples_per_channel < 1.0 {
        return Err(ExportError::InvalidRequest(
            "音声サンプル数を表現できません".to_owned(),
        ));
    }
    let target_len = (samples_per_channel as usize)
        .checked_mul(2)
        .ok_or_else(|| ExportError::InvalidRequest("音声サンプル数が大きすぎます".to_owned()))?;
    pcm.resize(target_len, 0.0);
    Ok(())
}

fn encode_audio(
    encoder: &mut OpenedAacEncoder,
    pcm: &[f32],
    sample_rate: u32,
    cancel: &AtomicBool,
) -> Result<Vec<ffmpeg::Packet>, ExportError> {
    let chunk_len = AUDIO_CHUNK_SAMPLES_PER_CHANNEL * 2;
    let mut packets = Vec::new();
    for (chunk_index, samples) in pcm.chunks(chunk_len).enumerate() {
        check_cancel(cancel)?;
        let samples_per_channel = samples.len() / 2;
        let first_sample = chunk_index * AUDIO_CHUNK_SAMPLES_PER_CHANNEL;
        let chunk = ProcessedChunk {
            samples: samples.to_vec(),
            audible_pts_secs: first_sample as f64 / f64::from(sample_rate),
            duration_secs: samples_per_channel as f64 / f64::from(sample_rate),
            source_secs_per_output_sec: 1.0,
            seek_serial: 0,
            pdc_latency_secs_at_process: 0.0,
        };
        packets.extend(encoder.push_chunk(chunk).map_err(|error| {
            ExportError::Encoder(format!("AAC 音声をエンコードできません: {error}"))
        })?);
    }
    packets.extend(encoder.finish().map_err(|error| {
        ExportError::Encoder(format!("AAC エンコーダを終了できません: {error}"))
    })?);
    Ok(packets)
}

struct RgbaConverter {
    scaler: ffmpeg::software::scaling::Context,
    input: Video,
    output_format: ffmpeg::format::Pixel,
    width: u32,
    height: u32,
}

impl RgbaConverter {
    fn new(width: u32, height: u32, format: H264InputFormat) -> Result<Self, ExportError> {
        let output_format = format.ffmpeg_pixel();
        let mut scaler = ffmpeg::software::scaling::Context::get(
            ffmpeg::format::Pixel::RGBA,
            width,
            height,
            output_format,
            width,
            height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )
        .map_err(|error| {
            ExportError::Encoder(format!("RGBA→YUV 変換を初期化できません: {error}"))
        })?;
        let coefficients = unsafe { ffmpeg::ffi::sws_getCoefficients(ffmpeg::ffi::SWS_CS_ITU709) };
        if coefficients.is_null() {
            return Err(ExportError::Encoder(
                "BT.709 の色変換係数を取得できません".to_owned(),
            ));
        }
        let result = unsafe {
            ffmpeg::ffi::sws_setColorspaceDetails(
                scaler.as_mut_ptr(),
                coefficients,
                1,
                coefficients,
                0,
                0,
                1 << 16,
                1 << 16,
            )
        };
        if result < 0 {
            return Err(ExportError::Encoder(format!(
                "BT.709 の色変換を設定できません: {}",
                ffmpeg::Error::from(result)
            )));
        }
        Ok(Self {
            scaler,
            input: Video::new(ffmpeg::format::Pixel::RGBA, width, height),
            output_format,
            width,
            height,
        })
    }

    fn convert(&mut self, frame: &Frame) -> Result<Video, ExportError> {
        let source_row_bytes = self.width as usize * 4;
        let stride = self.input.stride(0);
        for (source, destination) in frame
            .rgba
            .chunks_exact(source_row_bytes)
            .zip(self.input.data_mut(0).chunks_exact_mut(stride))
        {
            destination[..source_row_bytes].copy_from_slice(source);
        }
        let mut output = Video::new(self.output_format, self.width, self.height);
        self.scaler.run(&self.input, &mut output).map_err(|error| {
            ExportError::Encoder(format!("RGBA→YUV 変換に失敗しました: {error}"))
        })?;
        output.set_color_space(ffmpeg::color::Space::BT709);
        output.set_color_range(ffmpeg::color::Range::MPEG);
        output.set_color_primaries(ffmpeg::color::Primaries::BT709);
        output.set_color_transfer_characteristic(ffmpeg::color::TransferCharacteristic::BT709);
        Ok(output)
    }
}

fn drain_h264_packets(
    encoder: &mut ffmpeg::codec::encoder::video::Encoder,
) -> Result<Vec<ffmpeg::Packet>, ExportError> {
    let mut packets = Vec::new();
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => packets.push(packet),
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => return Ok(packets),
            Err(ffmpeg::Error::Eof) => return Ok(packets),
            Err(error) => {
                return Err(ExportError::Encoder(format!(
                    "H.264 パケットを受け取れません: {error}"
                )));
            }
        }
    }
}

fn write_video_packets(
    muxer: &mut Mp4Muxer,
    audio_packets: &mut VecDeque<ffmpeg::Packet>,
    video_packets: Vec<ffmpeg::Packet>,
) -> Result<(), ExportError> {
    for video in video_packets {
        loop {
            let audio_precedes = match audio_packets.front() {
                Some(audio) => muxer.audio_precedes_video(audio, &video)?,
                None => false,
            };
            if !audio_precedes {
                break;
            }
            muxer.write_audio(audio_packets.pop_front().unwrap())?;
        }
        muxer.write_video(video)?;
    }
    Ok(())
}

fn ensure_resolved(report: &RenderReport, frame_index: u64) -> Result<(), ExportError> {
    if report.unresolved_assets.is_empty() {
        return Ok(());
    }
    let assets = report
        .unresolved_assets
        .iter()
        .map(|asset| match asset {
            UnresolvedAsset::Image(id) => format!("画像 {}", id.0),
            UnresolvedAsset::Font(key) => format!("フォント {key}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(ExportError::Render(format!(
        "フレーム {} に未解決の素材があります: {assets}",
        frame_index + 1
    )))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ExportError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ExportError::Cancelled)
    } else {
        Ok(())
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * quantile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

struct PartialOutput {
    path: PathBuf,
    committed: bool,
}

impl PartialOutput {
    fn new(destination: &Path) -> Result<Self, ExportError> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .ok_or_else(|| ExportError::InvalidRequest("出力ファイル名がありません".to_owned()))?
            .to_string_lossy();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for suffix in 0..1000_u32 {
            let path = parent.join(format!(
                ".{file_name}.mvexport-{}-{stamp}-{suffix}.part.mp4",
                std::process::id()
            ));
            if !path.exists() {
                return Ok(Self {
                    path,
                    committed: false,
                });
            }
        }
        Err(ExportError::Io(
            "一時 MP4 ファイル名を確保できません".to_owned(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self, destination: &Path) -> Result<(), ExportError> {
        if destination.exists() {
            return Err(ExportError::Io(format!(
                "書き出し中に出力先が作成されました: {}",
                destination.display()
            )));
        }
        std::fs::rename(&self.path, destination)
            .map_err(|error| io_error("MP4 を出力先へ移動できません", destination, error))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// 書き出した MP4 を avformat/avcodec で開き直し、実デコード単位で A/V 長を確認する。
pub fn verify_exported_mp4(path: &Path) -> Result<Mp4Verification, ExportError> {
    ffmpeg::init().map_err(|error| {
        ExportError::Verification(format!("MP4 検証用の初期化に失敗しました: {error}"))
    })?;
    let mut input = ffmpeg::format::input(path).map_err(|error| {
        ExportError::Verification(format!(
            "書き出した MP4 を開き直せません ({}): {error}",
            path.display()
        ))
    })?;
    let stream_count = input.streams().count();

    let (video_index, video_codec, fps, mut video_decoder) = {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| {
                ExportError::Verification("MP4 に映像ストリームがありません".to_owned())
            })?;
        let parameters = stream.parameters();
        let mut rate = stream.avg_frame_rate();
        if rate.numerator() <= 0 || rate.denominator() <= 0 {
            rate = stream.rate();
        }
        let fps = if rate.numerator() > 0 && rate.denominator() > 0 {
            f64::from(rate.numerator()) / f64::from(rate.denominator())
        } else {
            0.0
        };
        let codec = parameters.id().name().to_owned();
        let decoder = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|error| {
                ExportError::Verification(format!("映像デコーダーを作成できません: {error}"))
            })?
            .decoder()
            .video()
            .map_err(|error| {
                ExportError::Verification(format!("映像デコーダーを開けません: {error}"))
            })?;
        (stream.index(), codec, fps, decoder)
    };
    let (audio_index, audio_codec, mut audio_decoder) = {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| {
                ExportError::Verification("MP4 に音声ストリームがありません".to_owned())
            })?;
        let parameters = stream.parameters();
        let codec = parameters.id().name().to_owned();
        let decoder = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|error| {
                ExportError::Verification(format!("音声デコーダーを作成できません: {error}"))
            })?
            .decoder()
            .audio()
            .map_err(|error| {
                ExportError::Verification(format!("音声デコーダーを開けません: {error}"))
            })?;
        (stream.index(), codec, decoder)
    };

    let width = video_decoder.width();
    let height = video_decoder.height();
    let sample_rate = audio_decoder.rate();
    let mut video_frames = 0_u64;
    let mut audio_samples = 0_u64;
    let mut video_frame = ffmpeg::util::frame::video::Video::empty();
    let mut audio_frame = ffmpeg::util::frame::audio::Audio::empty();

    for packet_result in input.packets() {
        let (stream, packet) = packet_result.map_err(|error| {
            ExportError::Verification(format!("MP4 パケットを読み取れません: {error}"))
        })?;
        if stream.index() == video_index {
            video_decoder.send_packet(&packet).map_err(|error| {
                ExportError::Verification(format!("映像パケットをデコードできません: {error}"))
            })?;
            drain_video_decoder(&mut video_decoder, &mut video_frame, &mut video_frames)?;
        } else if stream.index() == audio_index {
            audio_decoder.send_packet(&packet).map_err(|error| {
                ExportError::Verification(format!("音声パケットをデコードできません: {error}"))
            })?;
            drain_audio_decoder(&mut audio_decoder, &mut audio_frame, &mut audio_samples)?;
        }
    }
    video_decoder.send_eof().map_err(|error| {
        ExportError::Verification(format!("映像デコーダーを終了できません: {error}"))
    })?;
    drain_video_decoder(&mut video_decoder, &mut video_frame, &mut video_frames)?;
    audio_decoder.send_eof().map_err(|error| {
        ExportError::Verification(format!("音声デコーダーを終了できません: {error}"))
    })?;
    drain_audio_decoder(&mut audio_decoder, &mut audio_frame, &mut audio_samples)?;

    if fps <= 0.0 || sample_rate == 0 {
        return Err(ExportError::Verification(
            "MP4 の fps または音声サンプルレートが不正です".to_owned(),
        ));
    }
    let video_duration_secs = video_frames as f64 / fps;
    let audio_duration_secs = audio_samples as f64 / f64::from(sample_rate);
    Ok(Mp4Verification {
        streams: stream_count,
        video: Mp4VideoVerification {
            codec: video_codec,
            width,
            height,
            fps,
            frames: video_frames,
            duration_secs: video_duration_secs,
        },
        audio: Mp4AudioVerification {
            codec: audio_codec,
            sample_rate,
            samples_per_channel: audio_samples,
            duration_secs: audio_duration_secs,
        },
        duration_difference_secs: (video_duration_secs - audio_duration_secs).abs(),
    })
}

fn drain_video_decoder(
    decoder: &mut ffmpeg::decoder::Video,
    frame: &mut ffmpeg::util::frame::video::Video,
    frames: &mut u64,
) -> Result<(), ExportError> {
    loop {
        match decoder.receive_frame(frame) {
            Ok(()) => *frames = frames.saturating_add(1),
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(error) => {
                return Err(ExportError::Verification(format!(
                    "映像フレームを受け取れません: {error}"
                )));
            }
        }
    }
}

fn drain_audio_decoder(
    decoder: &mut ffmpeg::decoder::Audio,
    frame: &mut ffmpeg::util::frame::audio::Audio,
    samples: &mut u64,
) -> Result<(), ExportError> {
    loop {
        match decoder.receive_frame(frame) {
            Ok(()) => *samples = samples.saturating_add(frame.samples() as u64),
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(error) => {
                return Err(ExportError::Verification(format!(
                    "音声フレームを受け取れません: {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_is_reduced_without_changing_common_values() {
        assert_eq!(
            rational_frame_rate(60.0).unwrap(),
            FrameRate::new(60, 1).unwrap()
        );
        assert_eq!(
            rational_frame_rate(59.94).unwrap(),
            FrameRate::new(2997, 50).unwrap()
        );
    }

    #[test]
    fn audio_is_truncated_or_zero_padded_to_project_duration() {
        let mut long = vec![1.0; 24];
        fit_audio_to_duration(&mut long, 4, 2.0).unwrap();
        assert_eq!(long, vec![1.0; 16]);

        let mut short = vec![1.0; 4];
        fit_audio_to_duration(&mut short, 4, 1.0).unwrap();
        assert_eq!(short.len(), 8);
        assert_eq!(&short[..4], &[1.0; 4]);
        assert_eq!(&short[4..], &[0.0; 4]);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[], 0.95), 0.0);
        assert_eq!(percentile(&[5.0, 1.0, 4.0, 2.0, 3.0], 0.8), 4.0);
        assert_eq!(percentile(&[5.0, 1.0, 4.0, 2.0, 3.0], 0.95), 5.0);
    }

    #[test]
    fn gcd_reduces_frame_rate_fraction() {
        assert_eq!(gcd(60_000_000, 1_000_000), 1_000_000);
        assert_eq!(gcd(59_940_000, 1_000_000), 20_000);
    }

    #[test]
    fn mp4_extension_is_ascii_case_insensitive() {
        assert!(has_mp4_extension(Path::new("video.mp4")));
        assert!(has_mp4_extension(Path::new("video.MP4")));
        assert!(!has_mp4_extension(Path::new("video.mov")));
    }

    #[test]
    fn uncommitted_partial_output_is_removed_on_drop() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let destination = std::env::temp_dir().join(format!(
            "miv-mvexport-partial-test-{}-{stamp}.mp4",
            std::process::id()
        ));
        let partial_path = {
            let partial = PartialOutput::new(&destination).unwrap();
            let path = partial.path().to_owned();
            std::fs::write(&path, b"partial").unwrap();
            assert!(path.exists());
            path
        };
        assert!(!partial_path.exists());
        assert!(!destination.exists());
    }
}
