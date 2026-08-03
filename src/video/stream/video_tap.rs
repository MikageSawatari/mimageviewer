use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;

use super::encoder::{
    EncoderPreference, FrameRate, H264EncoderKind, H264EncoderOpenError, H264InputFormat,
    OpenedH264Encoder, open_h264_encoder,
};
use super::quality::{
    OutputDimensions, OutputDimensionsError, QualityPreset, StreamOutputParameters,
};
use super::segmenter::{CfrTimelineFrameIndex, Fmp4Segmenter};
use super::timeline::{StreamTimeline, StreamTimelineError};

const FFMPEG_EAGAIN: i32 = 11;

/// Tap-owned decoder HW surfaces that can outlive one synchronous producer call.
///
/// This is a hard contract for increment 5's session/queue sizing: `attach` capacity applies only
/// to already-downloaded software frames, so increasing it cannot retain D3D11 decoder surfaces.
#[cfg(test)]
pub(crate) const VIDEO_TAP_MAX_QUEUED_DECODER_HW_SURFACES: usize = 0;

/// Decoder HW surfaces involved in one tap producer call. This is the source frame already owned by
/// `run_video_decode`; the tap never clones its AVHWFramesContext reference.
#[cfg(test)]
pub(crate) const VIDEO_TAP_MAX_SYNCHRONOUS_DECODER_HW_SURFACES: usize = 1;

/// A queue-safe software frame. Keeping this wrapper private makes it impossible to construct a
/// `TappedVideoFrame` containing a D3D11 decoder surface outside this module.
struct SoftwareTapFrame(Video);

impl SoftwareTapFrame {
    fn try_new(frame: Video) -> Result<Self, String> {
        let format = frame.format();
        if matches!(format, Pixel::D3D11 | Pixel::None) {
            return Err(format!(
                "video tap queue requires a software frame, got {format:?}"
            ));
        }
        Ok(Self(frame))
    }

    fn as_video(&self) -> &Video {
        &self.0
    }
}

/// Decoder output before the native-presenter GPU/CPU path split, made queue-safe.
///
/// `frame` is always software-backed. D3D11 input is downloaded synchronously before construction,
/// so a delayed streaming worker cannot retain a surface from the decoder's fixed-size pool.
pub(crate) struct TappedVideoFrame {
    frame: SoftwareTapFrame,
    pub(crate) source_pts_secs: f64,
    pub(crate) seek_serial: u64,
}

impl TappedVideoFrame {
    pub(crate) fn as_video(&self) -> &Video {
        self.frame.as_video()
    }

    #[cfg(test)]
    pub(crate) fn from_owned_software_frame(
        frame: Video,
        source_pts_secs: f64,
        seek_serial: u64,
    ) -> Result<Self, String> {
        Ok(Self {
            frame: SoftwareTapFrame::try_new(frame)?,
            source_pts_secs,
            seek_serial,
        })
    }
}

#[derive(Clone)]
pub(crate) struct VideoTapController {
    command_tx: Sender<VideoTapCommand>,
    next_owner_id: Arc<AtomicU64>,
}

/// The session owns this lease and receiver. An old lease cannot detach a newer owner.
pub(crate) struct VideoTapLease {
    owner_id: u64,
    command_tx: Sender<VideoTapCommand>,
    #[allow(dead_code)] // Increment 6 VideoStreamState will expose tap backpressure telemetry.
    dropped: Arc<AtomicU64>,
}

enum VideoTapCommand {
    Attach(ActiveVideoTap),
    Detach(u64),
}

struct ActiveVideoTap {
    owner_id: u64,
    payload_tx: Sender<TappedVideoFrame>,
    dropped: Arc<AtomicU64>,
}

/// Decoder-thread side: command polling, availability check, immediate HW readback or SW AVFrame
/// ref, and try_send. Stream-resolution swscale and encoding are absent from this type.
pub(crate) struct VideoTapProducer {
    command_rx: Receiver<VideoTapCommand>,
    active: Option<ActiveVideoTap>,
}

pub(crate) fn video_tap_channel() -> (VideoTapController, VideoTapProducer) {
    let (command_tx, command_rx) = unbounded();
    (
        VideoTapController {
            command_tx,
            next_owner_id: Arc::new(AtomicU64::new(1)),
        },
        VideoTapProducer {
            command_rx,
            active: None,
        },
    )
}

impl VideoTapController {
    pub(crate) fn disconnected() -> Self {
        let (controller, producer) = video_tap_channel();
        drop(producer);
        controller
    }

    #[cfg(test)]
    pub(crate) fn connected_without_frames_for_test() -> Self {
        let (controller, producer) = video_tap_channel();
        std::mem::forget(producer);
        controller
    }

    pub(crate) fn attach(
        &self,
        software_frame_capacity: usize,
    ) -> Result<(VideoTapLease, Receiver<TappedVideoFrame>), &'static str> {
        if software_frame_capacity == 0 {
            return Err("video tap capacity must be non-zero");
        }
        let owner_id = self.next_owner_id.fetch_add(1, Ordering::Relaxed);
        // TappedVideoFrame can only contain SoftwareTapFrame. Queue capacity therefore has no
        // relationship to the decoder's fixed D3D11 AVHWFramesContext pool.
        let (payload_tx, payload_rx) = bounded(software_frame_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        self.command_tx
            .send(VideoTapCommand::Attach(ActiveVideoTap {
                owner_id,
                payload_tx,
                dropped: Arc::clone(&dropped),
            }))
            .map_err(|_| "video decoder is no longer running")?;
        let lease = VideoTapLease {
            owner_id,
            command_tx: self.command_tx.clone(),
            dropped,
        };
        Ok((lease, payload_rx))
    }
}

impl VideoTapLease {
    #[allow(dead_code)] // Increment 6 VideoStreamState will expose tap backpressure telemetry.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for VideoTapLease {
    fn drop(&mut self) {
        let _ = self
            .command_tx
            .try_send(VideoTapCommand::Detach(self.owner_id));
    }
}

impl VideoTapProducer {
    fn refresh(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                VideoTapCommand::Attach(tap) => self.active = Some(tap),
                VideoTapCommand::Detach(owner_id)
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|tap| tap.owner_id == owner_id) =>
                {
                    self.active = None;
                }
                VideoTapCommand::Detach(_) => {}
            }
        }
    }

    /// Called at the one common decoder-output branch point. This never waits for the tap.
    pub(crate) fn try_publish(&mut self, frame: &Video, source_pts_secs: f64, seek_serial: u64) {
        self.try_publish_with(source_pts_secs, seek_serial, || {
            prepare_queue_frame_with(
                frame.format(),
                || download_hw_frame(frame),
                || clone_avframe_ref(frame),
            )
        });
    }

    fn try_publish_with<F>(&mut self, source_pts_secs: f64, seek_serial: u64, prepare_frame: F)
    where
        F: FnOnce() -> Result<SoftwareTapFrame, String>,
    {
        self.refresh();
        let Some(tap) = self.active.as_ref() else {
            return;
        };
        // With one producer, a full queue can only become less full. Avoid even the shallow
        // AVFrame allocation/ref on this drop path.
        if tap.payload_tx.is_full() {
            tap.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let frame = match prepare_frame() {
            Ok(frame) => frame,
            Err(error) => {
                tap.dropped.fetch_add(1, Ordering::Relaxed);
                crate::logger::log(format!(
                    "remote-stream video tap frame prepare failed: {error}"
                ));
                return;
            }
        };
        let payload = TappedVideoFrame {
            frame,
            source_pts_secs,
            seek_serial,
        };
        match tap.payload_tx.try_send(payload) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                tap.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                tap.dropped.fetch_add(1, Ordering::Relaxed);
                self.active = None;
            }
        }
    }
}

fn prepare_queue_frame_with<DownloadHw, CloneSoftware>(
    source_format: Pixel,
    download_hw: DownloadHw,
    clone_software: CloneSoftware,
) -> Result<SoftwareTapFrame, String>
where
    DownloadHw: FnOnce() -> Result<Video, String>,
    CloneSoftware: FnOnce() -> Result<Video, String>,
{
    let frame = match source_format {
        // Never clone an AVHWFramesContext-backed frame: download while the decoder owns its one
        // current source reference, then enqueue only the independent software buffer.
        Pixel::D3D11 => download_hw()?,
        Pixel::None => return Err("video tap source has no pixel format".to_owned()),
        _ => clone_software()?,
    };
    SoftwareTapFrame::try_new(frame)
}

fn download_hw_frame(frame: &Video) -> Result<Video, String> {
    let mut sw_holder = None;
    crate::video::swscale_helpers::prepare_frame_for_swscale(frame, &mut sw_holder)?;
    sw_holder.ok_or_else(|| "video tap D3D11 readback did not produce a software frame".to_owned())
}

fn clone_avframe_ref(frame: &Video) -> Result<Video, String> {
    debug_assert_ne!(frame.format(), Pixel::D3D11);
    // SAFETY: this is called only for software frames. av_frame_clone returns a new AVFrame
    // referencing the source buffers; Video::wrap owns it and Drop calls av_frame_free.
    let cloned = unsafe { ffmpeg::ffi::av_frame_clone(frame.as_ptr()) };
    if cloned.is_null() {
        return Err("av_frame_clone returned null".to_owned());
    }
    Ok(unsafe { Video::wrap(cloned) })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VideoStreamEncoderStats {
    pub(crate) tapped_frames: u64,
    pub(crate) stale_seek_frames: u64,
    pub(crate) duplicate_or_retrograde_cfr_slots: u64,
    pub(crate) submitted_frames: u64,
    pub(crate) encoded_packets: u64,
}

#[derive(Debug)]
pub(crate) enum VideoStreamEncoderOpenError {
    OutputDimensions(OutputDimensionsError),
    Encoder(H264EncoderOpenError),
}

impl fmt::Display for VideoStreamEncoderOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputDimensions(error) => error.fmt(f),
            Self::Encoder(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for VideoStreamEncoderOpenError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VideoStreamEncoderError(String);

impl VideoStreamEncoderError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for VideoStreamEncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VideoStreamEncoderError {}

impl From<StreamTimelineError> for VideoStreamEncoderError {
    fn from(error: StreamTimelineError) -> Self {
        Self::new(error.to_string())
    }
}

struct VideoFrameScaler {
    context: Option<ScaleContext>,
    input_key: Option<(Pixel, u32, u32)>,
    output_format: Pixel,
    output_dimensions: OutputDimensions,
}

impl VideoFrameScaler {
    fn new(input_format: H264InputFormat, output_dimensions: OutputDimensions) -> Self {
        Self {
            context: None,
            input_key: None,
            output_format: input_format.ffmpeg_pixel(),
            output_dimensions,
        }
    }

    fn scale(&mut self, input: &Video) -> Result<Video, VideoStreamEncoderError> {
        // SoftwareTapFrame already excludes D3D11. Keep the shared guard here as defense in depth
        // before handing the format to swscale, whose invalid-format failure is process-fatal.
        let mut sw_holder = None;
        let input = crate::video::swscale_helpers::prepare_frame_for_swscale(input, &mut sw_holder)
            .map_err(VideoStreamEncoderError::new)?;
        let input_key = (input.format(), input.width(), input.height());
        if matches!(input_key.0, Pixel::D3D11 | Pixel::None) || input_key.1 == 0 || input_key.2 == 0
        {
            return Err(VideoStreamEncoderError::new(format!(
                "invalid video tap swscale input: {:?} {}x{}",
                input_key.0, input_key.1, input_key.2
            )));
        }
        if self.context.is_none() || self.input_key != Some(input_key) {
            self.context = Some(
                ScaleContext::get(
                    input_key.0,
                    input_key.1,
                    input_key.2,
                    self.output_format,
                    self.output_dimensions.width,
                    self.output_dimensions.height,
                    ScaleFlags::BILINEAR,
                )
                .map_err(|error| {
                    VideoStreamEncoderError::new(format!(
                        "remote video swscale init failed: {error}"
                    ))
                })?,
            );
            self.input_key = Some(input_key);
        }
        let mut output = Video::new(
            self.output_format,
            self.output_dimensions.width,
            self.output_dimensions.height,
        );
        self.context
            .as_mut()
            .expect("initialized above")
            .run(input, &mut output)
            .map_err(|error| {
                VideoStreamEncoderError::new(format!("remote video swscale failed: {error}"))
            })?;
        Ok(output)
    }
}

/// Worker-side decoder-frame -> encoder component. The future session worker owns this together
/// with the shared A/V timeline and mux coordinator; the decoder owns only `VideoTapProducer`.
pub(crate) struct VideoStreamEncoder {
    output: StreamOutputParameters,
    frame_rate: FrameRate,
    timeline: StreamTimeline,
    expected_seek_serial: u64,
    opened: OpenedH264Encoder,
    scaler: VideoFrameScaler,
    last_submitted_index: Option<CfrTimelineFrameIndex>,
    stats: VideoStreamEncoderStats,
    finished: bool,
}

pub(crate) fn open_video_stream_encoder(
    preference: EncoderPreference,
    preset: QualityPreset,
    source_width: u32,
    source_height: u32,
    frame_rate: FrameRate,
    expected_seek_serial: u64,
    timeline: StreamTimeline,
) -> Result<VideoStreamEncoder, VideoStreamEncoderOpenError> {
    let output = preset
        .output_parameters(source_width, source_height)
        .map_err(VideoStreamEncoderOpenError::OutputDimensions)?;
    let opened = open_h264_encoder(preference, output, frame_rate)
        .map_err(VideoStreamEncoderOpenError::Encoder)?;
    let scaler = VideoFrameScaler::new(opened.input_format, output.dimensions);
    Ok(VideoStreamEncoder {
        output,
        frame_rate,
        timeline,
        expected_seek_serial,
        opened,
        scaler,
        last_submitted_index: None,
        stats: VideoStreamEncoderStats::default(),
        finished: false,
    })
}

impl VideoStreamEncoder {
    pub(crate) fn encoder_kind(&self) -> H264EncoderKind {
        self.opened.kind
    }

    pub(crate) fn encoder(&self) -> &ffmpeg::codec::encoder::video::Encoder {
        &self.opened.encoder
    }

    pub(crate) fn input_format(&self) -> H264InputFormat {
        self.opened.input_format
    }

    pub(crate) fn output_parameters(&self) -> StreamOutputParameters {
        self.output
    }

    pub(crate) fn effective_video_bitrate_bps(&self) -> u64 {
        self.opened.effective_video_bitrate_bps
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> VideoStreamEncoderStats {
        self.stats
    }

    /// Scale to the selected encoder's input format, retain the source-derived CFR slot, and ask
    /// the segmenter whether this slot is a forced-IDR boundary. Increment 5 will interleave the
    /// returned packets with AAC packets in its session worker.
    pub(crate) fn encode_frame(
        &mut self,
        tapped: TappedVideoFrame,
        segmenter: &Fmp4Segmenter,
    ) -> Result<Vec<ffmpeg::Packet>, VideoStreamEncoderError> {
        if self.finished {
            return Err(VideoStreamEncoderError::new(
                "cannot push video after encoder finish",
            ));
        }
        self.stats.tapped_frames = self.stats.tapped_frames.saturating_add(1);
        if tapped.seek_serial != self.expected_seek_serial {
            self.stats.stale_seek_frames = self.stats.stale_seek_frames.saturating_add(1);
            return Ok(Vec::new());
        }
        let index =
            cfr_timeline_frame_index(self.timeline, tapped.source_pts_secs, self.frame_rate)?;
        if self
            .last_submitted_index
            .is_some_and(|last| index.value() <= last.value())
        {
            self.stats.duplicate_or_retrograde_cfr_slots = self
                .stats
                .duplicate_or_retrograde_cfr_slots
                .saturating_add(1);
            return Ok(Vec::new());
        }
        let mut frame = self.scaler.scale(tapped.frame.as_video())?;
        let pts = i64::try_from(index.value())
            .map_err(|_| VideoStreamEncoderError::new("video CFR timestamp exceeds i64 range"))?;
        frame.set_pts(Some(pts));
        segmenter.prepare_video_frame(index, &mut frame);
        self.opened
            .encoder
            .send_frame(&frame)
            .map_err(|error| VideoStreamEncoderError::new(format!("H.264 send_frame: {error}")))?;
        self.last_submitted_index = Some(index);
        self.stats.submitted_frames = self.stats.submitted_frames.saturating_add(1);
        let packets = drain_h264_packets(&mut self.opened.encoder)?;
        self.stats.encoded_packets = self
            .stats
            .encoded_packets
            .saturating_add(packets.len() as u64);
        Ok(packets)
    }

    #[cfg(test)]
    pub(crate) fn finish(&mut self) -> Result<Vec<ffmpeg::Packet>, VideoStreamEncoderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.opened
            .encoder
            .send_eof()
            .map_err(|error| VideoStreamEncoderError::new(format!("H.264 send_eof: {error}")))?;
        let packets = drain_h264_packets(&mut self.opened.encoder)?;
        self.stats.encoded_packets = self
            .stats
            .encoded_packets
            .saturating_add(packets.len() as u64);
        self.finished = true;
        Ok(packets)
    }
}

/// Map source PTS to the nearest configured CFR slot. VFR frames that land on a used slot are
/// coalesced by `VideoStreamEncoder`; an absent/dropped frame leaves a hole instead of renumbering.
pub(crate) fn cfr_timeline_frame_index(
    timeline: StreamTimeline,
    source_pts_secs: f64,
    frame_rate: FrameRate,
) -> Result<CfrTimelineFrameIndex, VideoStreamEncoderError> {
    let relative_secs = timeline.relative_secs(source_pts_secs)?;
    let scaled =
        relative_secs * f64::from(frame_rate.numerator) / f64::from(frame_rate.denominator);
    if !scaled.is_finite() || scaled > i64::MAX as f64 {
        return Err(VideoStreamEncoderError::new(
            "video CFR timestamp exceeds i64 range",
        ));
    }
    Ok(CfrTimelineFrameIndex::new(scaled.round() as u64))
}

fn drain_h264_packets(
    encoder: &mut ffmpeg::codec::encoder::video::Encoder,
) -> Result<Vec<ffmpeg::Packet>, VideoStreamEncoderError> {
    let mut packets = Vec::new();
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => packets.push(packet),
            Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => return Ok(packets),
            Err(ffmpeg::Error::Eof) => return Ok(packets),
            Err(error) => {
                return Err(VideoStreamEncoderError::new(format!(
                    "H.264 receive_packet: {error}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn software_tap_frame() -> SoftwareTapFrame {
        SoftwareTapFrame::try_new(Video::new(Pixel::YUV420P, 2, 2)).unwrap()
    }

    struct FakeDecoderHwSurfaceRef<'a> {
        active: &'a Cell<usize>,
        max_active: &'a Cell<usize>,
    }

    impl<'a> FakeDecoderHwSurfaceRef<'a> {
        fn new(active: &'a Cell<usize>, max_active: &'a Cell<usize>) -> Self {
            let current = active.get() + 1;
            active.set(current);
            max_active.set(max_active.get().max(current));
            Self { active, max_active }
        }

        fn observe(&self) {
            self.max_active
                .set(self.max_active.get().max(self.active.get()));
        }
    }

    impl Drop for FakeDecoderHwSurfaceRef<'_> {
        fn drop(&mut self) {
            self.active.set(self.active.get() - 1);
        }
    }

    #[test]
    fn disconnected_tap_keeps_decoder_branch_allocation_and_conversion_free() {
        let (_controller, mut producer) = video_tap_channel();
        let expensive_work_calls = Cell::new(0_u32);
        producer.try_publish_with(3.25, 7, || {
            expensive_work_calls.set(expensive_work_calls.get() + 1);
            Ok(software_tap_frame())
        });
        assert_eq!(expensive_work_calls.get(), 0);
    }

    #[test]
    fn full_tap_never_blocks_decoder_and_counts_without_cloning() {
        let (controller, mut producer) = video_tap_channel();
        let (lease, rx) = controller.attach(1).unwrap();
        let clone_calls = Cell::new(0_u32);
        producer.try_publish_with(0.0, 0, || {
            clone_calls.set(clone_calls.get() + 1);
            Ok(software_tap_frame())
        });
        assert_eq!(rx.len(), 1);

        producer.try_publish_with(1.0 / 30.0, 0, || {
            clone_calls.set(clone_calls.get() + 1);
            Ok(software_tap_frame())
        });
        assert_eq!(clone_calls.get(), 1, "full drop must not clone the AVFrame");
        assert_eq!(lease.dropped(), 1);
    }

    #[test]
    fn stale_lease_drop_does_not_detach_replacement_owner() {
        let (controller, mut producer) = video_tap_channel();
        let (old_lease, old_rx) = controller.attach(1).unwrap();
        producer.try_publish_with(0.0, 0, || Ok(software_tap_frame()));
        assert_eq!(old_rx.len(), 1);

        let (_new_lease, new_rx) = controller.attach(1).unwrap();
        drop(old_lease);
        producer.try_publish_with(1.0 / 30.0, 0, || Ok(software_tap_frame()));
        assert_eq!(new_rx.len(), 1);
    }

    #[test]
    fn dropping_video_tap_lease_restores_disconnected_decoder_path() {
        let (controller, mut producer) = video_tap_channel();
        let (lease, rx) = controller.attach(1).unwrap();
        producer.try_publish_with(0.0, 0, || Ok(software_tap_frame()));
        assert_eq!(rx.len(), 1);
        drop(lease);

        let expensive_work_calls = Cell::new(0_u32);
        producer.try_publish_with(1.0 / 30.0, 0, || {
            expensive_work_calls.set(expensive_work_calls.get() + 1);
            Ok(software_tap_frame())
        });
        assert_eq!(expensive_work_calls.get(), 0);
    }

    #[test]
    fn stalled_worker_queue_retains_no_decoder_hw_surface_refs() {
        const SOFTWARE_QUEUE_CAPACITY: usize = 8;

        assert_eq!(VIDEO_TAP_MAX_QUEUED_DECODER_HW_SURFACES, 0);
        assert_eq!(VIDEO_TAP_MAX_SYNCHRONOUS_DECODER_HW_SURFACES, 1);

        let (controller, mut producer) = video_tap_channel();
        let (_lease, rx) = controller.attach(SOFTWARE_QUEUE_CAPACITY).unwrap();
        let active_hw_refs = Cell::new(0_usize);
        let max_active_hw_refs = Cell::new(0_usize);

        // Do not receive from `rx`: this models a worker stalled behind scale/encode.
        for index in 0..SOFTWARE_QUEUE_CAPACITY {
            let source = FakeDecoderHwSurfaceRef::new(&active_hw_refs, &max_active_hw_refs);
            producer.try_publish_with(index as f64 / 30.0, 0, || {
                source.observe();
                prepare_queue_frame_with(
                    Pixel::D3D11,
                    || Ok(Video::new(Pixel::YUV420P, 2, 2)),
                    || panic!("D3D11 tap input must be downloaded, never cloned"),
                )
            });
            drop(source);
            assert_eq!(
                active_hw_refs.get(),
                VIDEO_TAP_MAX_QUEUED_DECODER_HW_SURFACES,
                "queued tap payload retained decoder HW ref at depth {}",
                rx.len()
            );
        }

        assert_eq!(rx.len(), SOFTWARE_QUEUE_CAPACITY);
        assert_eq!(max_active_hw_refs.get(), 1);
        assert_eq!(active_hw_refs.get(), 0);
    }

    #[test]
    fn source_pts_selects_cfr_slots_without_renumbering_dropped_frames() {
        let timeline = StreamTimeline::new(10.0).unwrap();
        let rate = FrameRate::new(30, 1).unwrap();
        let observed = [10.0, 10.0 + 1.0 / 30.0, 10.0 + 3.0 / 30.0].map(|pts| {
            cfr_timeline_frame_index(timeline, pts, rate)
                .unwrap()
                .value()
        });
        assert_eq!(observed, [0, 1, 3]);
    }

    #[test]
    fn vfr_pts_are_rounded_to_nearest_cfr_slot() {
        let timeline = StreamTimeline::new(2.0).unwrap();
        let rate = FrameRate::new(30_000, 1_001).unwrap();
        let slots = [2.0, 2.010, 2.040, 2.100].map(|pts| {
            cfr_timeline_frame_index(timeline, pts, rate)
                .unwrap()
                .value()
        });
        assert_eq!(slots, [0, 0, 1, 3]);
    }
}
