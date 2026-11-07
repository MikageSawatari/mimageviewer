//! Worker-owned CFR H.264 export, without an audio track.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::video::stream::encoder::{FrameRate, OpenedH264Encoder, open_offline_h264_encoder};
use crate::video::stream::quality::{OutputDimensions, StreamOutputParameters};

use super::driver::{RgbaConverter, check_cancel, drain_h264_packets, stream_encoder_preference};
use super::mux::Mp4Muxer;
use super::{EncoderPreference, ExportError, PartialOutput};

#[derive(Clone, Copy, Debug)]
pub(crate) struct VideoOnlyMp4Options {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
    pub(crate) video_bitrate_bps: u32,
    pub(crate) encoder: EncoderPreference,
    pub(crate) replace_existing: bool,
}

/// Blocking API: construct, write and finish on the export worker only.
/// The caller supplies opaque, tightly packed RGBA8 frames in display gamma space.
/// Dropping an unfinished writer removes its temporary file and leaves the destination intact.
pub(crate) struct VideoOnlyMp4Writer {
    video: OpenedH264Encoder,
    converter: RgbaConverter,
    // Close FFmpeg's output handle before PartialOutput deletes the file on Windows.
    muxer: Mp4Muxer,
    partial: PartialOutput,
    destination: PathBuf,
    replace_existing: bool,
    rgba_bytes: usize,
    frames: i64,
}

impl VideoOnlyMp4Writer {
    pub(crate) fn new(path: &Path, options: VideoOnlyMp4Options) -> Result<Self, ExportError> {
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        {
            return Err(ExportError::InvalidRequest(
                "MP4 の保存先を指定してください".to_owned(),
            ));
        }
        let rgba_bytes = validate_options(options)?;
        let frame_rate = FrameRate::new(options.fps, 1).map_err(ExportError::InvalidRequest)?;
        let partial = PartialOutput::new(path)?;
        let video = open_offline_h264_encoder(
            stream_encoder_preference(options.encoder),
            StreamOutputParameters {
                dimensions: OutputDimensions {
                    width: options.width,
                    height: options.height,
                },
                video_bitrate_bps: options.video_bitrate_bps,
                audio_bitrate_bps: 0,
            },
            frame_rate,
        )
        .map_err(|error| ExportError::Encoder(format!("H.264 エンコーダを開けません: {error}")))?;
        let converter = RgbaConverter::new(options.width, options.height, video.input_format)?;
        let muxer = Mp4Muxer::new_video_only(partial.path(), &video, (options.fps, 1))?;
        Ok(Self {
            video,
            converter,
            muxer,
            partial,
            destination: path.to_owned(),
            replace_existing: options.replace_existing,
            rgba_bytes,
            frames: 0,
        })
    }

    pub(crate) fn write_rgba(
        &mut self,
        rgba: &[u8],
        cancel: &AtomicBool,
    ) -> Result<(), ExportError> {
        check_cancel(cancel)?;
        if rgba.len() != self.rgba_bytes {
            return Err(ExportError::InvalidRequest(
                "RGBA フレームの寸法が一致しません".to_owned(),
            ));
        }
        let next = self.frames.checked_add(1).ok_or_else(|| {
            ExportError::InvalidRequest("MP4 のフレーム数が上限を超えました".to_owned())
        })?;
        let converted = self.converter.convert_rgba(rgba)?;
        converted.set_pts(Some(self.frames));
        self.video.encoder.send_frame(converted).map_err(|error| {
            ExportError::Encoder(format!("H.264 フレームを送れません: {error}"))
        })?;
        for packet in drain_h264_packets(&mut self.video.encoder)? {
            self.muxer.write_video(packet)?;
        }
        self.frames = next;
        Ok(())
    }

    pub(crate) fn finish(mut self, cancel: &AtomicBool) -> Result<(), ExportError> {
        check_cancel(cancel)?;
        if self.frames == 0 {
            return Err(ExportError::InvalidRequest(
                "MP4 に映像フレームがありません".to_owned(),
            ));
        }
        self.video.encoder.send_eof().map_err(|error| {
            ExportError::Encoder(format!("H.264 エンコーダを終了できません: {error}"))
        })?;
        for packet in drain_h264_packets(&mut self.video.encoder)? {
            self.muxer.write_video(packet)?;
        }
        self.muxer.finish()?;
        self.partial
            .commit(&self.destination, self.replace_existing, cancel)
    }
}

fn validate_options(options: VideoOnlyMp4Options) -> Result<usize, ExportError> {
    if options.width < 2
        || options.height < 2
        || options.width % 2 != 0
        || options.height % 2 != 0
        || options.width > i32::MAX as u32
        || options.height > i32::MAX as u32
        || options.video_bitrate_bps == 0
    {
        return Err(ExportError::InvalidRequest(
            "MP4 の寸法またはビットレートが不正です".to_owned(),
        ));
    }
    FrameRate::new(options.fps, 1).map_err(ExportError::InvalidRequest)?;
    (options.width as usize)
        .checked_mul(options.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| ExportError::InvalidRequest("MP4 のフレームが大きすぎます".to_owned()))
}

#[cfg(test)]
#[path = "video_only_tests.rs"]
mod tests;
