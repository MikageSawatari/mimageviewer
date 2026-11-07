use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;
use music_core::PcmView;
use musicvideo_core::{
    AnalysisKey, AnalysisTimeline, Frame, FrameGrid, LayerKind, RenderReport, Smoothing,
    UnresolvedAsset, calibrate_modulators_view, evaluate, pcm_view, render_frame,
};
use rayon::prelude::*;
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
    EncoderPreference, ExportError, ExportProgress, ExportStage, ExportSummary,
    Mp4AudioVerification, Mp4Verification, Mp4VideoVerification, MusicVideoExportRequest, io_error,
};

const AUDIO_BITRATE_BPS: u32 = 192_000;
const AUDIO_CHUNK_SAMPLES_PER_CHANNEL: usize = 16_384;
const FFMPEG_EAGAIN: i32 = 11;
const CONVERTER_HALO_ROWS: u32 = 8;

struct ExportPlan {
    grid: FrameGrid,
    frames: u64,
    frame_rate: FrameRate,
    spectrum_bands: usize,
}

pub(super) fn export<I: Clone + Eq>(
    request: MusicVideoExportRequest<I>,
    progress: &dyn Fn(ExportProgress),
    cancel: &AtomicBool,
) -> Result<ExportSummary, ExportError> {
    let plan = validate_request(&request)?;
    let report_progress = |stage, completed_frames| {
        progress(ExportProgress {
            stage,
            completed_frames,
            total_frames: plan.frames,
        });
    };
    report_progress(ExportStage::Calibration, 0);
    check_cancel(cancel)?;
    let wall_started = Instant::now();
    let partial = PartialOutput::new(&request.output_path)?;
    let sample_rate = request.audio.sample_rate;
    let source_duration_secs = request.source_duration_secs;
    let view = pcm_view(
        &request.audio.stereo_pcm,
        sample_rate,
        request.project.duration_secs,
    )
    .map_err(|error| ExportError::InvalidRequest(error.to_string()))?;
    let calibration_started = Instant::now();
    let calibrations = calibrate_modulators_view(
        view,
        sample_rate,
        request.project.duration_secs,
        source_duration_secs,
        &request.project.modulators,
        &|| cancel.load(Ordering::Relaxed),
    )
    .map_err(|error| {
        if cancel.load(Ordering::Relaxed) {
            ExportError::Cancelled
        } else {
            ExportError::InvalidRequest(format!(
                "モジュレータの反応範囲を自動調整できません: {error}"
            ))
        }
    })?;
    let calibration_secs = calibration_started.elapsed().as_secs_f64();
    check_cancel(cancel)?;
    let key = AnalysisKey::new(
        request.pcm_id.clone(),
        sample_rate,
        view.logical_frames,
        plan.grid,
        plan.spectrum_bands,
        Smoothing::default(),
        &request.project.modulators,
        &calibrations,
        request.project.display_gain_db,
    )
    .map_err(|error| {
        ExportError::InvalidRequest(format!("スペクトラム解析を初期化できません: {error}"))
    })?;
    report_progress(ExportStage::AudioEncoding, 0);
    check_cancel(cancel)?;

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
    let mut audio = open_aac_encoder(sample_rate, AUDIO_BITRATE_BPS, 0, timeline)
        .map_err(|error| ExportError::Encoder(format!("AAC エンコーダを開けません: {error}")))?;
    let mux_started = Instant::now();
    let mut muxer = Mp4Muxer::new(
        partial.path(),
        &video,
        &audio,
        (plan.frame_rate.numerator, plan.frame_rate.denominator),
    )?;
    let mut mux_secs = mux_started.elapsed().as_secs_f64();

    let aac_encode_started = Instant::now();
    let mut audio_packets = VecDeque::from(encode_audio(&mut audio, view, sample_rate, cancel)?);
    let aac_encode_secs = aac_encode_started.elapsed().as_secs_f64();
    report_progress(ExportStage::Frames, 0);
    check_cancel(cancel)?;
    let mut spectrum = AnalysisTimeline::default();
    let mut rgba = Frame::new(request.project.output.width, request.project.output.height)
        .map_err(|error| ExportError::Render(format!("RGBA フレームを確保できません: {error}")))?;
    let mut converter = RgbaConverter::new(
        request.project.output.width,
        request.project.output.height,
        video.input_format,
    )?;
    let mut hasher = request.compute_frame_hash.then(Sha256::new);
    let mut frame_hashes = request.compute_per_frame_hashes.then(Vec::new);
    let mut render_ms = Vec::with_capacity(plan.frames as usize);
    let mut convert_total_secs = 0.0;
    let mut encode_total_secs = 0.0;
    let mut frame_hash_secs = 0.0;

    for frame_index in 0..plan.frames {
        check_cancel(cancel)?;
        let t_secs = plan.grid.time(frame_index);

        let render_started = Instant::now();
        let spectrum_frame = spectrum
            .frame(&key, view, frame_index, &|| cancel.load(Ordering::Relaxed))
            .map_err(|_| ExportError::Cancelled)?;
        let scene = evaluate(&request.project, t_secs, spectrum_frame);
        let report = render_frame(&scene, &request.assets, &mut rgba).map_err(|error| {
            ExportError::Render(format!(
                "フレーム {} の合成に失敗しました: {error}",
                frame_index + 1
            ))
        })?;
        ensure_resolved(&report, frame_index)?;
        render_ms.push(render_started.elapsed().as_secs_f64() * 1000.0);

        if let Some(hasher) = hasher.as_mut() {
            let hash_started = Instant::now();
            hasher.update(&rgba.rgba);
            frame_hash_secs += hash_started.elapsed().as_secs_f64();
        }
        if let Some(hashes) = frame_hashes.as_mut() {
            let hash_started = Instant::now();
            hashes.push(Sha256::digest(&rgba.rgba).into());
            frame_hash_secs += hash_started.elapsed().as_secs_f64();
        }

        let convert_started = Instant::now();
        let converted = converter.convert(&rgba)?;
        converted.set_pts(Some(frame_index as i64));
        convert_total_secs += convert_started.elapsed().as_secs_f64();

        let encode_started = Instant::now();
        video.encoder.send_frame(converted).map_err(|error| {
            ExportError::Encoder(format!("H.264 フレームを送れません: {error}"))
        })?;
        let packets = drain_h264_packets(&mut video.encoder)?;
        encode_total_secs += encode_started.elapsed().as_secs_f64();
        let mux_started = Instant::now();
        write_video_packets(&mut muxer, &mut audio_packets, packets)?;
        mux_secs += mux_started.elapsed().as_secs_f64();

        report_progress(ExportStage::Frames, frame_index + 1);
    }

    check_cancel(cancel)?;
    report_progress(ExportStage::Finalizing, plan.frames);
    check_cancel(cancel)?;
    let flush_started = Instant::now();
    video.encoder.send_eof().map_err(|error| {
        ExportError::Encoder(format!("H.264 エンコーダを終了できません: {error}"))
    })?;
    let final_video_packets = drain_h264_packets(&mut video.encoder)?;
    encode_total_secs += flush_started.elapsed().as_secs_f64();
    let mux_started = Instant::now();
    write_video_packets(&mut muxer, &mut audio_packets, final_video_packets)?;
    while let Some(packet) = audio_packets.pop_front() {
        muxer.write_audio(packet)?;
    }
    mux_secs += mux_started.elapsed().as_secs_f64();
    let faststart_started = Instant::now();
    muxer.finish()?;
    let faststart_secs = faststart_started.elapsed().as_secs_f64();
    check_cancel(cancel)?;
    let rename_started = Instant::now();
    partial.commit(&request.output_path, request.replace_existing, cancel)?;
    let rename_secs = rename_started.elapsed().as_secs_f64();
    let hash_finalize_started = Instant::now();
    let rgba_sha256 = hasher.map(|hasher| format!("{:x}", hasher.finalize()));
    if rgba_sha256.is_some() {
        frame_hash_secs += hash_finalize_started.elapsed().as_secs_f64();
    }

    let wall_secs = wall_started.elapsed().as_secs_f64();
    let frames = plan.frames as f64;
    let render_total_secs = render_ms.iter().sum::<f64>() / 1000.0;
    let accounted_secs = render_total_secs
        + convert_total_secs
        + encode_total_secs
        + calibration_secs
        + aac_encode_secs
        + mux_secs
        + faststart_secs
        + rename_secs
        + frame_hash_secs;
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
        calibration_secs,
        aac_encode_secs,
        mux_secs,
        faststart_secs,
        rename_secs,
        frame_hash_secs,
        other_secs: (wall_secs - accounted_secs).max(0.0),
        wall_secs,
        realtime_ratio: if wall_secs > 0.0 {
            duration_secs / wall_secs
        } else {
            0.0
        },
        rgba_sha256,
        rgba_frame_sha256: frame_hashes,
    })
}

fn validate_request<I>(request: &MusicVideoExportRequest<I>) -> Result<ExportPlan, ExportError> {
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
    if !request.project.display_gain_db.is_finite() {
        return Err(ExportError::InvalidRequest(
            "表示ゲインは有限値にしてください".to_owned(),
        ));
    }
    if request.video_bitrate_bps == 0 {
        return Err(ExportError::InvalidRequest(
            "映像ビットレートは 1 以上にしてください".to_owned(),
        ));
    }
    if request.audio.sample_rate == 0
        || request.audio.stereo_pcm.is_empty()
        || request.audio.stereo_pcm.len() % 2 != 0
        || !request.source_duration_secs.is_finite()
        || request.source_duration_secs <= 0.0
    {
        return Err(ExportError::InvalidRequest(
            "音声スナップショットが不正です".to_owned(),
        ));
    }
    if !request.replace_existing && request.output_path.exists() {
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

    let grid = FrameGrid::new(output.fps, request.project.duration_secs)
        .map_err(|error| ExportError::InvalidRequest(error.to_string()))?;
    let frame_rate = FrameRate::new(grid.numerator(), grid.denominator())
        .map_err(ExportError::InvalidRequest)?;
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
        grid,
        frames: grid.frames(),
        frame_rate,
        spectrum_bands,
    })
}

fn has_mp4_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
}

pub(super) fn stream_encoder_preference(preference: EncoderPreference) -> StreamEncoderPreference {
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

/// The export has no track picker, so it takes the container's default audio stream,
/// which is what the decoder chose on its own before it started taking a stream index.
pub(super) fn default_audio_stream_index(path: &Path) -> Result<usize, ExportError> {
    let input = ffmpeg::format::input(&path.to_path_buf())
        .map_err(|error| ExportError::AudioDecode(format!("音声を開けません: {error}")))?;
    input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|stream| stream.index())
        .ok_or_else(|| ExportError::AudioDecode("音声ストリームが見つかりません".to_string()))
}

fn encode_audio(
    encoder: &mut OpenedAacEncoder,
    pcm: PcmView<'_>,
    sample_rate: u32,
    cancel: &AtomicBool,
) -> Result<Vec<ffmpeg::Packet>, ExportError> {
    let mut packets = Vec::new();
    for first_sample in (0..pcm.logical_frames).step_by(AUDIO_CHUNK_SAMPLES_PER_CHANNEL) {
        check_cancel(cancel)?;
        let chunk = materialize_audio_chunk(pcm, sample_rate, first_sample);
        packets.extend(encoder.push_chunk(chunk).map_err(|error| {
            ExportError::Encoder(format!("AAC 音声をエンコードできません: {error}"))
        })?);
    }
    packets.extend(encoder.finish().map_err(|error| {
        ExportError::Encoder(format!("AAC エンコーダを終了できません: {error}"))
    })?);
    Ok(packets)
}

/// Materializes at most one AAC input chunk, preserving source sample bits.
fn materialize_audio_chunk(
    pcm: PcmView<'_>,
    sample_rate: u32,
    first_sample: usize,
) -> ProcessedChunk {
    let frames = (pcm.logical_frames - first_sample).min(AUDIO_CHUNK_SAMPLES_PER_CHANNEL);
    let mut samples = vec![0.0; frames * 2];
    let available = (pcm.samples.len() / 2)
        .saturating_sub(first_sample)
        .min(frames);
    if available > 0 {
        samples[..available * 2]
            .copy_from_slice(&pcm.samples[first_sample * 2..(first_sample + available) * 2]);
    }
    ProcessedChunk {
        samples,
        audible_pts_secs: first_sample as f64 / f64::from(sample_rate),
        duration_secs: frames as f64 / f64::from(sample_rate),
        source_secs_per_output_sec: 1.0,
        seek_serial: 0,
        pdc_latency_secs_at_process: 0.0,
        effetune_generation: None,
    }
}

pub(super) struct RgbaConverter {
    bands: Vec<RgbaConverterBand>,
    output: Video,
    input_format: H264InputFormat,
    width: u32,
}

struct RgbaConverterBand {
    y0: u32,
    height: u32,
    source_y0: u32,
    source_height: u32,
    scaler: ffmpeg::software::scaling::Context,
    input: Video,
    output: Video,
    width: u32,
}

// SAFETY: each band exclusively owns its SwsContext and AVFrames. Rayon only receives a
// distinct mutable band for each job, so neither swscale state nor frame storage is accessed
// concurrently. FFmpeg scaling contexts are not thread-affine and may be moved between calls;
// they merely must not be used by two threads at the same time.
unsafe impl Send for RgbaConverterBand {}

impl RgbaConverter {
    pub(super) fn new(
        width: u32,
        height: u32,
        format: H264InputFormat,
    ) -> Result<Self, ExportError> {
        Self::with_parallelism(width, height, format, rayon::current_num_threads())
    }

    fn with_parallelism(
        width: u32,
        height: u32,
        format: H264InputFormat,
        requested_bands: usize,
    ) -> Result<Self, ExportError> {
        let output_format = format.ffmpeg_pixel();
        let bands = converter_band_ranges(height, requested_bands)
            .into_iter()
            .map(|(y0, band_height)| {
                RgbaConverterBand::new(width, height, y0, band_height, output_format)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            bands,
            output: Video::new(output_format, width, height),
            input_format: format,
            width,
        })
    }

    fn convert(&mut self, frame: &Frame) -> Result<&mut Video, ExportError> {
        self.convert_rgba(&frame.rgba)
    }

    pub(super) fn convert_rgba(&mut self, rgba: &[u8]) -> Result<&mut Video, ExportError> {
        self.bands
            .par_iter_mut()
            .try_for_each(|band| band.convert(rgba))
            .map_err(|error| {
                ExportError::Encoder(format!("RGBA→YUV 変換に失敗しました: {error}"))
            })?;

        make_frame_writable(&mut self.output)?;
        for band in &self.bands {
            copy_converted_band(&mut self.output, band, self.input_format, self.width);
        }
        set_bt709_metadata(&mut self.output);
        Ok(&mut self.output)
    }
}

fn make_frame_writable(frame: &mut Video) -> Result<(), ExportError> {
    // The encoder may retain an AVBufferRef after send_frame returns. Asking FFmpeg for a
    // writable frame here preserves that reference via copy-on-write before band stitching.
    let result = unsafe { ffmpeg::ffi::av_frame_make_writable(frame.as_mut_ptr()) };
    if result < 0 {
        return Err(ExportError::Encoder(format!(
            "YUV フレームを書き込み可能にできません: {}",
            ffmpeg::Error::from(result)
        )));
    }
    Ok(())
}

impl RgbaConverterBand {
    fn new(
        width: u32,
        full_height: u32,
        y0: u32,
        height: u32,
        output_format: ffmpeg::format::Pixel,
    ) -> Result<Self, ExportError> {
        let source_y0 = y0.saturating_sub(CONVERTER_HALO_ROWS);
        let source_y1 = y0
            .saturating_add(height)
            .saturating_add(CONVERTER_HALO_ROWS)
            .min(full_height);
        let source_height = source_y1 - source_y0;
        let scaler = create_bt709_scaler(width, source_height, output_format)?;
        Ok(Self {
            y0,
            height,
            source_y0,
            source_height,
            scaler,
            input: Video::new(ffmpeg::format::Pixel::RGBA, width, source_height),
            output: Video::new(output_format, width, source_height),
            width,
        })
    }

    fn convert(&mut self, rgba: &[u8]) -> Result<(), ffmpeg::Error> {
        let source_row_bytes = self.width as usize * 4;
        let source_start = self.source_y0 as usize * source_row_bytes;
        let source_end = source_start + self.source_height as usize * source_row_bytes;
        let stride = self.input.stride(0);
        for (source, destination) in rgba[source_start..source_end]
            .chunks_exact(source_row_bytes)
            .zip(self.input.data_mut(0).chunks_exact_mut(stride))
        {
            destination[..source_row_bytes].copy_from_slice(source);
        }
        self.scaler.run(&self.input, &mut self.output)
    }
}

fn create_bt709_scaler(
    width: u32,
    height: u32,
    output_format: ffmpeg::format::Pixel,
) -> Result<ffmpeg::software::scaling::Context, ExportError> {
    let mut scaler = ffmpeg::software::scaling::Context::get(
        ffmpeg::format::Pixel::RGBA,
        width,
        height,
        output_format,
        width,
        height,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )
    .map_err(|error| ExportError::Encoder(format!("RGBA→YUV 変換を初期化できません: {error}")))?;
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
    Ok(scaler)
}

fn converter_band_ranges(height: u32, requested_bands: usize) -> Vec<(u32, u32)> {
    let chroma_rows = height as usize / 2;
    let band_count = requested_bands.max(1).min(chroma_rows.max(1));
    (0..band_count)
        .map(|index| {
            let chroma_y0 = index * chroma_rows / band_count;
            let chroma_y1 = (index + 1) * chroma_rows / band_count;
            ((chroma_y0 * 2) as u32, ((chroma_y1 - chroma_y0) * 2) as u32)
        })
        .collect()
}

fn copy_converted_band(
    destination: &mut Video,
    band: &RgbaConverterBand,
    format: H264InputFormat,
    width: u32,
) {
    copy_plane_rows(
        destination,
        0,
        band.y0 as usize,
        &band.output,
        (band.y0 - band.source_y0) as usize,
        width as usize,
        band.height as usize,
    );
    let chroma_y0 = band.y0 as usize / 2;
    let source_chroma_y0 = (band.y0 - band.source_y0) as usize / 2;
    let chroma_height = band.height as usize / 2;
    match format {
        H264InputFormat::Nv12 => copy_plane_rows(
            destination,
            1,
            chroma_y0,
            &band.output,
            source_chroma_y0,
            width as usize,
            chroma_height,
        ),
        H264InputFormat::Yuv420p => {
            let chroma_width = width as usize / 2;
            copy_plane_rows(
                destination,
                1,
                chroma_y0,
                &band.output,
                source_chroma_y0,
                chroma_width,
                chroma_height,
            );
            copy_plane_rows(
                destination,
                2,
                chroma_y0,
                &band.output,
                source_chroma_y0,
                chroma_width,
                chroma_height,
            );
        }
    }
}

fn copy_plane_rows(
    destination: &mut Video,
    plane: usize,
    destination_y0: usize,
    source: &Video,
    source_y0: usize,
    active_row_bytes: usize,
    rows: usize,
) {
    let destination_stride = destination.stride(plane);
    let source_stride = source.stride(plane);
    let source_data = source.data(plane);
    let destination_data = destination.data_mut(plane);
    for row in 0..rows {
        let source_offset = (source_y0 + row) * source_stride;
        let destination_offset = (destination_y0 + row) * destination_stride;
        destination_data[destination_offset..destination_offset + active_row_bytes]
            .copy_from_slice(&source_data[source_offset..source_offset + active_row_bytes]);
    }
}

fn set_bt709_metadata(output: &mut Video) {
    output.set_color_space(ffmpeg::color::Space::BT709);
    output.set_color_range(ffmpeg::color::Range::MPEG);
    output.set_color_primaries(ffmpeg::color::Primaries::BT709);
    output.set_color_transfer_characteristic(ffmpeg::color::TransferCharacteristic::BT709);
}

pub(super) fn drain_h264_packets(
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
            UnresolvedAsset::TextPreparationBudget {
                font_key,
                required_bytes,
                budget_bytes,
            } => format!(
                "文字の描画準備が上限を超えました（フォント {font_key}、見積もり量 {required_bytes} / 上限 {budget_bytes} バイト）。文字のサイズ・文字数・装飾を減らしてください"
            ),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(ExportError::Render(format!(
        "フレーム {} に未解決の素材があります: {assets}",
        frame_index + 1
    )))
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), ExportError> {
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

pub(crate) struct PartialOutput {
    path: tempfile::TempPath,
}

impl PartialOutput {
    pub(crate) fn new(destination: &Path) -> Result<Self, ExportError> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let extension = destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp");
        let temporary = tempfile::Builder::new()
            .prefix(".mvexport-")
            .suffix(&format!(".part.{extension}"))
            .tempfile_in(parent)
            .map_err(|error| io_error("一時ファイルを作成できません", parent, error))?;
        // Release the reservation handle before FFmpeg opens it on Windows.
        Ok(Self {
            path: temporary.into_temp_path(),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn commit(
        self,
        destination: &Path,
        replace_existing: bool,
        cancel: &AtomicBool,
    ) -> Result<(), ExportError> {
        check_cancel(cancel)?;
        let result = if replace_existing {
            self.path.persist(destination)
        } else {
            self.path.persist_noclobber(destination)
        };
        result
            .map_err(|error| io_error("ファイルを出力先へ公開できません", destination, error.error))
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
#[path = "snapshot_tests.rs"]
mod snapshot_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn serial_reference_convert(frame: &Frame, format: H264InputFormat) -> Video {
        let mut scaler =
            create_bt709_scaler(frame.width, frame.height, format.ffmpeg_pixel()).unwrap();
        let mut input = Video::new(ffmpeg::format::Pixel::RGBA, frame.width, frame.height);
        let source_row_bytes = frame.width as usize * 4;
        let stride = input.stride(0);
        for (source, destination) in frame
            .rgba
            .chunks_exact(source_row_bytes)
            .zip(input.data_mut(0).chunks_exact_mut(stride))
        {
            destination[..source_row_bytes].copy_from_slice(source);
        }
        let mut output = Video::new(format.ffmpeg_pixel(), frame.width, frame.height);
        scaler.run(&input, &mut output).unwrap();
        set_bt709_metadata(&mut output);
        output
    }

    fn assert_active_video_eq(
        expected: &Video,
        actual: &Video,
        format: H264InputFormat,
        width: usize,
        height: usize,
    ) {
        let planes: &[(usize, usize, usize)] = match format {
            H264InputFormat::Nv12 => &[(0, width, height), (1, width, height / 2)],
            H264InputFormat::Yuv420p => &[
                (0, width, height),
                (1, width / 2, height / 2),
                (2, width / 2, height / 2),
            ],
        };
        for &(plane, active_row_bytes, rows) in planes {
            let expected_stride = expected.stride(plane);
            let actual_stride = actual.stride(plane);
            for row in 0..rows {
                assert_eq!(
                    &expected.data(plane)
                        [row * expected_stride..row * expected_stride + active_row_bytes],
                    &actual.data(plane)
                        [row * actual_stride..row * actual_stride + active_row_bytes],
                    "format={format:?}, plane={plane}, row={row}"
                );
            }
        }
        assert_eq!(actual.color_space(), expected.color_space());
        assert_eq!(actual.color_range(), expected.color_range());
        assert_eq!(actual.color_primaries(), expected.color_primaries());
        assert_eq!(
            actual.color_transfer_characteristic(),
            expected.color_transfer_characteristic()
        );
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[], 0.95), 0.0);
        assert_eq!(percentile(&[5.0, 1.0, 4.0, 2.0, 3.0], 0.8), 4.0);
        assert_eq!(percentile(&[5.0, 1.0, 4.0, 2.0, 3.0], 0.95), 5.0);
    }

    #[test]
    fn mp4_extension_is_ascii_case_insensitive() {
        assert!(has_mp4_extension(Path::new("video.mp4")));
        assert!(has_mp4_extension(Path::new("video.MP4")));
        assert!(!has_mp4_extension(Path::new("video.mov")));
    }

    #[test]
    fn d17_eleven_text_budget_is_a_fatal_frame_error_like_missing_assets() {
        assert!(ensure_resolved(&RenderReport::default(), 7).is_ok());
        let report = RenderReport {
            unresolved_assets: vec![UnresolvedAsset::TextPreparationBudget {
                font_key: "budget-font".into(),
                required_bytes: 123_456_789,
                budget_bytes: 67_108_864,
            }],
        };
        let error = ensure_resolved(&report, 7).unwrap_err();
        let ExportError::Render(message) = error else {
            panic!("a text budget failure must stop frame rendering");
        };
        for expected in [
            "フレーム 8",
            "文字の描画準備が上限を超えました",
            "budget-font",
            "見積もり量 123456789 / 上限 67108864 バイト",
            "文字のサイズ・文字数・装飾を減らしてください",
        ] {
            assert!(message.contains(expected), "missing {expected}: {message}");
        }
        assert!(!message.contains("未解決素材: フォント"));
    }

    #[test]
    fn converter_bands_cover_each_even_row_exactly_once() {
        let ranges = converter_band_ranges(362, 7);
        assert_eq!(ranges.len(), 7);
        let mut next_y = 0;
        for (y0, height) in ranges {
            assert_eq!(y0, next_y);
            assert_eq!(y0 % 2, 0);
            assert_eq!(height % 2, 0);
            assert!(height > 0);
            next_y += height;
        }
        assert_eq!(next_y, 362);
    }

    #[test]
    fn parallel_rgba_conversion_matches_single_swscale_context_bit_for_bit() {
        for (width, height, bands) in [(642, 362, 7), (1920, 1080, rayon::current_num_threads())] {
            let mut frame = Frame::new(width, height).unwrap();
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let offset = (y * width as usize + x) * 4;
                    frame.rgba[offset] = (x.wrapping_mul(13) ^ y.wrapping_mul(7)) as u8;
                    frame.rgba[offset + 1] =
                        (x.wrapping_mul(3).wrapping_add(y.wrapping_mul(11))) as u8;
                    frame.rgba[offset + 2] =
                        (x.wrapping_mul(17).wrapping_add(y.wrapping_mul(5))) as u8;
                    frame.rgba[offset + 3] = (x ^ y) as u8;
                }
            }

            for format in [H264InputFormat::Nv12, H264InputFormat::Yuv420p] {
                let expected = serial_reference_convert(&frame, format);
                let mut converter =
                    RgbaConverter::with_parallelism(width, height, format, bands).unwrap();
                let actual = converter.convert(&frame).unwrap();
                assert_active_video_eq(&expected, &actual, format, width as usize, height as usize);
            }
        }
    }

    #[test]
    fn reused_output_preserves_encoder_held_buffer_references() {
        let (width, height) = (642, 362);
        let mut first = Frame::new(width, height).unwrap();
        let mut second = Frame::new(width, height).unwrap();
        for (index, pixel) in first.rgba.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[
                index.wrapping_mul(7) as u8,
                index.wrapping_mul(13) as u8,
                index.wrapping_mul(29) as u8,
                255,
            ]);
        }
        for (source, destination) in first
            .rgba
            .chunks_exact(4)
            .zip(second.rgba.chunks_exact_mut(4))
        {
            destination.copy_from_slice(&[255 - source[0], source[2], source[1], 255]);
        }

        let format = H264InputFormat::Nv12;
        let expected_first = serial_reference_convert(&first, format);
        let expected_second = serial_reference_convert(&second, format);
        let mut converter = RgbaConverter::with_parallelism(width, height, format, 7).unwrap();
        let retained = {
            let converted = converter.convert(&first).unwrap();
            let mut reference = unsafe { ffmpeg::Frame::empty() };
            let result =
                unsafe { ffmpeg::ffi::av_frame_ref(reference.as_mut_ptr(), converted.as_ptr()) };
            assert_eq!(result, 0);
            Video::from(reference)
        };

        let actual_second = converter.convert(&second).unwrap();
        assert_active_video_eq(
            &expected_second,
            actual_second,
            format,
            width as usize,
            height as usize,
        );
        assert_active_video_eq(
            &expected_first,
            &retained,
            format,
            width as usize,
            height as usize,
        );
    }

    #[test]
    fn uncommitted_partial_output_is_removed_on_drop() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
