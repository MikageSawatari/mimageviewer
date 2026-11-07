//! Frozen 3D-photo export requests and one worker per modal operation.
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::depth_parallax::{DepthParallaxSettings, MotionKind, motion_offset};
use crate::depth_parallax_wgpu::{DepthFrameRenderer, DepthRenderParams, DepthRenderResources};
use crate::video::mvexport::{
    EncoderPreference, PartialOutput, VideoOnlyMp4Options, VideoOnlyMp4Writer,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DepthExportFormat {
    #[default]
    Mp4,
    Gif,
    Apng,
}
impl DepthExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
            Self::Apng => "png",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Gif => "GIF",
            Self::Apng => "APNG",
        }
    }
    pub fn fps(self) -> u32 {
        if self == Self::Mp4 { 30 } else { 15 }
    }
}

#[derive(Clone, Debug)]
pub struct DepthExportOptions {
    pub format: DepthExportFormat,
    pub long_edge: u32,
    pub motion: MotionKind,
    pub loops: u32,
}
impl Default for DepthExportOptions {
    fn default() -> Self {
        Self {
            format: DepthExportFormat::Mp4,
            long_edge: 1080,
            motion: MotionKind::Horizontal,
            loops: 4,
        }
    }
}
impl DepthExportOptions {
    pub fn dimensions(&self, source: [u32; 2], uv: egui::Rect, quarter_turns: u32) -> [u32; 2] {
        let mut dimensions = [
            source[0] as f32 * uv.width(),
            source[1] as f32 * uv.height(),
        ];
        if quarter_turns % 2 == 1 {
            dimensions.swap(0, 1);
        }
        let scale = (self.long_edge as f32 / dimensions[0].max(dimensions[1])).min(1.);
        dimensions.map(|d| {
            let size = (d * scale).round().max(1.) as u32;
            if self.format == DepthExportFormat::Mp4 {
                (size / 2 * 2).max(2)
            } else {
                size
            }
        })
    }
    pub fn cycle_frames(&self, period: f32) -> u32 {
        (period * self.format.fps() as f32).round().max(1.) as u32
    }
    pub fn total_frames(&self, period: f32) -> u32 {
        self.cycle_frames(period)
            * if self.format == DepthExportFormat::Mp4 {
                self.loops.clamp(1, 10)
            } else {
                1
            }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DepthExportUnderlay {
    Solid([u8; 3]),
    /// Cell width/height in output pixels, scaled from the shown image rectangle.
    Checker {
        cell_size: [f32; 2],
    },
}

pub(crate) struct DepthExportRequest {
    pub resource: Arc<DepthRenderResources>,
    pub settings: DepthParallaxSettings,
    pub uv: egui::Rect,
    pub quarter_turns: u32,
    pub source_size: [u32; 2],
    pub underlay: DepthExportUnderlay,
    pub output: PathBuf,
    pub options: DepthExportOptions,
}
pub(crate) enum DepthExportEvent {
    Progress { completed: u32, total: u32 },
    Finished(Result<PathBuf, String>),
    Cancelled,
}
pub(crate) struct DepthExportJob {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<DepthExportEvent>,
}
impl DepthExportJob {
    #[cfg(test)]
    pub(crate) fn controlled_for_test() -> (Self, mpsc::Sender<DepthExportEvent>, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        (
            Self {
                cancel: cancel.clone(),
                rx,
            },
            tx,
            cancel,
        )
    }
    pub(crate) fn start(
        request: DepthExportRequest,
        device: wgpu::Device,
        queue: wgpu::Queue,
        ctx: egui::Context,
        viewport: egui::ViewportId,
    ) -> Result<Self, String> {
        let output = request.output.clone();
        Self::spawn(ctx, viewport, output, move |cancel, progress| {
            let size =
                request
                    .options
                    .dimensions(request.source_size, request.uv, request.quarter_turns);
            validate(&request)?;
            check_cancel(cancel)?;
            let renderer =
                DepthFrameRenderer::new(device, queue, &request.resource, size, request.underlay);
            encode_frames(
                &request.output,
                &request.options,
                request.settings.period_secs,
                size,
                cancel,
                |index, cycle| {
                    renderer.render(DepthRenderParams::from_settings(
                        &request.settings,
                        request.resource.depth.median,
                        motion_offset(
                            request.options.motion,
                            request.settings.strength,
                            (index % cycle) as f32 / cycle as f32,
                        ),
                        request.uv,
                        request.quarter_turns,
                        size[0] as f32 / size[1] as f32,
                    ))
                },
                progress,
            )
        })
    }
    fn spawn(
        ctx: egui::Context,
        viewport: egui::ViewportId,
        output: PathBuf,
        run: impl FnOnce(&AtomicBool, &dyn Fn(u32, u32)) -> Result<(), ExportFailure> + Send + 'static,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("depth-export".into())
            .spawn(move || {
                let result = run(&worker_cancel, &|completed, total| {
                    let _ = tx.send(DepthExportEvent::Progress { completed, total });
                    ctx.request_repaint_of(viewport);
                });
                let event = match result {
                    Err(ExportFailure::Cancelled) => DepthExportEvent::Cancelled,
                    Err(ExportFailure::Failed(error)) => {
                        crate::logger::log(format!("[DepthExport] {error}"));
                        DepthExportEvent::Finished(Err(error))
                    }
                    Ok(()) => DepthExportEvent::Finished(Ok(output)),
                };
                let _ = tx.send(event);
                ctx.request_repaint_of(viewport);
            })
            .map_err(|error| format!("書き出しを開始できません: {error}"))?;
        Ok(Self { cancel, rx })
    }
    pub(crate) fn try_recv(&self) -> Result<DepthExportEvent, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for DepthExportJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Debug)]
enum ExportFailure {
    Cancelled,
    Failed(String),
}
impl From<String> for ExportFailure {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
impl From<crate::video::mvexport::ExportError> for ExportFailure {
    fn from(error: crate::video::mvexport::ExportError) -> Self {
        match error {
            crate::video::mvexport::ExportError::Cancelled => Self::Cancelled,
            error => Self::Failed(error.to_string()),
        }
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), ExportFailure> {
    if cancel.load(Ordering::Relaxed) {
        Err(ExportFailure::Cancelled)
    } else {
        Ok(())
    }
}
fn validate(request: &DepthExportRequest) -> Result<(), ExportFailure> {
    let s = &request.settings;
    if request.source_size.contains(&0)
        || !request.uv.is_positive()
        || !request.uv.is_finite()
        || !s.period_secs.is_finite()
        || !(1.0..=10.0).contains(&s.period_secs)
        || request.options.long_edge == 0
        || matches!(
            request.options.motion,
            MotionKind::Mouse | MotionKind::Unknown
        )
    {
        return Err("3D 写真の書き出し設定が不正です".to_owned().into());
    }
    if let DepthExportUnderlay::Checker { cell_size } = request.underlay
        && cell_size
            .into_iter()
            .any(|cell| !cell.is_finite() || cell <= 0.)
    {
        return Err("市松の大きさが不正です".to_owned().into());
    }
    Ok(())
}

/// Cumulative quantization avoids truncating every GIF frame to 60ms.
fn delay_ticks(index: u32, frames: u32, period: f32, ticks_per_second: u32) -> u16 {
    let total = (period as f64 * ticks_per_second as f64).round() as u64;
    let at = |i: u32| (u64::from(i) * total + u64::from(frames) / 2) / u64::from(frames);
    (at(index + 1) - at(index)) as u16
}

fn encode_frames(
    output: &std::path::Path,
    options: &DepthExportOptions,
    period: f32,
    size: [u32; 2],
    cancel: &AtomicBool,
    mut render: impl FnMut(u32, u32) -> Result<Vec<u8>, String>,
    progress: impl Fn(u32, u32),
) -> Result<(), ExportFailure> {
    let frames = options.cycle_frames(period);
    let total = options.total_frames(period);
    check_cancel(cancel)?;
    progress(0, total);
    match options.format {
        DepthExportFormat::Mp4 => {
            let mut writer = VideoOnlyMp4Writer::new(
                output,
                VideoOnlyMp4Options {
                    width: size[0],
                    height: size[1],
                    fps: options.format.fps(),
                    video_bitrate_bps: 12_000_000,
                    encoder: EncoderPreference::Auto,
                    replace_existing: true,
                },
            )?;
            for index in 0..total {
                check_cancel(cancel)?;
                let rgba = render(index, frames)?;
                check_cancel(cancel)?;
                writer.write_rgba(&rgba, cancel)?;
                progress(index + 1, total);
            }
            writer.finish(cancel)?;
        }
        DepthExportFormat::Gif | DepthExportFormat::Apng => {
            let partial = PartialOutput::new(output)?;
            let file = std::fs::File::create(partial.path()).map_err(|e| e.to_string())?;
            let mut buffer = std::io::BufWriter::new(file);
            if options.format == DepthExportFormat::Gif {
                let mut writer = image::codecs::gif::GifEncoder::new_with_speed(&mut buffer, 10);
                writer
                    .set_repeat(image::codecs::gif::Repeat::Infinite)
                    .map_err(|e| e.to_string())?;
                for index in 0..frames {
                    check_cancel(cancel)?;
                    let rgba = render(index, frames)?;
                    check_cancel(cancel)?;
                    let image = image::RgbaImage::from_raw(size[0], size[1], rgba)
                        .ok_or("フレームの寸法が不正です".to_owned())?;
                    writer
                        .encode_frame(image::Frame::from_parts(
                            image,
                            0,
                            0,
                            image::Delay::from_numer_denom_ms(
                                u32::from(delay_ticks(index, frames, period, 100)) * 10,
                                1,
                            ),
                        ))
                        .map_err(|e| e.to_string())?;
                    progress(index + 1, total);
                }
                drop(writer);
            } else {
                let mut encoder = png::Encoder::new(&mut buffer, size[0], size[1]);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.set_animated(frames, 0).map_err(|e| e.to_string())?;
                let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
                for index in 0..frames {
                    check_cancel(cancel)?;
                    let rgba = render(index, frames)?;
                    check_cancel(cancel)?;
                    writer
                        .set_frame_delay(delay_ticks(index, frames, period, 1000), 1000)
                        .map_err(|e| e.to_string())?;
                    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
                    progress(index + 1, total);
                }
                writer.finish().map_err(|e| e.to_string())?;
            }
            // Publish only after all buffered writes, including the trailer, succeeded.
            std::io::Write::flush(&mut buffer).map_err(|e| e.to_string())?;
            drop(buffer);
            check_cancel(cancel)?;
            partial.commit(output, true, cancel)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
