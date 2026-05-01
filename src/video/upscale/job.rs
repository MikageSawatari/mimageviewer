use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;
use image::{DynamicImage, ImageBuffer, RgbaImage};

use crate::ai::{ModelKind, model_manager::ModelManager, runtime::AiRuntime};

use super::sidecar::{
    EncodeInfo, OutputInfo, UpscaleInfo, VideoUpscaleSidecar, derived_sidecar_path_for,
    derived_video_path_for, output_within_mvp_limit, source_info_for,
};

const FFMPEG_EAGAIN: i32 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: i32,
    pub fps_den: i32,
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
    pub elapsed_ms: AtomicU64,
}

impl VideoUpscaleProgressShared {
    pub fn new(total: Option<u64>) -> Self {
        Self {
            frames_done: AtomicU64::new(0),
            frames_total: AtomicU64::new(total.unwrap_or(0)),
            elapsed_ms: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> (u64, u64, Duration) {
        (
            self.frames_done.load(Ordering::Relaxed),
            self.frames_total.load(Ordering::Relaxed),
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

    let part_path = job.output_path.with_file_name(format!(
        "{}.part",
        job.output_path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let _ = fs::remove_file(&part_path);

    let result = encode_video_only(&job, &part_path, &runtime, &cancel, &progress);
    if result.is_err() || cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&part_path);
    }
    result?;

    if cancel.load(Ordering::Relaxed) {
        return Err("キャンセルされました".to_owned());
    }

    if job.output_path.exists() {
        fs::remove_file(&job.output_path)
            .map_err(|e| format!("既存の出力ファイルを削除できません: {e}"))?;
    }
    fs::rename(&part_path, &job.output_path)
        .map_err(|e| format!("出力ファイルの確定に失敗しました: {e}"))?;

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
            audio: "none".to_owned(),
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
    let json = serde_json::to_string_pretty(&sidecar)
        .map_err(|e| format!("sidecar JSONの作成に失敗しました: {e}"))?;
    fs::write(&job.sidecar_path, json)
        .map_err(|e| format!("sidecar JSONの保存に失敗しました: {e}"))?;

    Ok(job.output_path)
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
        estimated_frames,
        duration_secs,
    })
}

fn encode_video_only(
    job: &VideoUpscaleJob,
    part_path: &Path,
    runtime: &AiRuntime,
    cancel: &Arc<AtomicBool>,
    progress: &VideoUpscaleProgressShared,
) -> Result<(), String> {
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
    let mut frame_index = 0_i64;
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
                        runtime,
                        cancel,
                        progress,
                        started,
                        &mut frame_index,
                    )?;
                }
                Err(e) => return Err(format!("動画パケットのデコード投入に失敗しました: {e}")),
            }
        }
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
            runtime,
            cancel,
            progress,
            started,
            &mut frame_index,
        )?;
    }
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
        runtime,
        cancel,
        progress,
        started,
        &mut frame_index,
    )?;

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
    Ok(())
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
    runtime: &AiRuntime,
    cancel: &Arc<AtomicBool>,
    progress: &VideoUpscaleProgressShared,
    started: Instant,
    frame_index: &mut i64,
) -> Result<(), String> {
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
        yuv.set_pts(Some(*frame_index));
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

        *frame_index += 1;
        progress
            .frames_done
            .store(*frame_index as u64, Ordering::Relaxed);
        progress
            .elapsed_ms
            .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    Ok(())
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
}
