use std::ffi::c_void;
use std::{fmt, mem, ptr, slice};

use ffmpeg::codec::packet::Mut as _;
use ffmpeg_the_third as ffmpeg;

use super::encoder::FrameRate;
use super::playlist::{SegmentLookup, SegmentRing, master_playlist};

const AVIO_BUFFER_SIZE: usize = 32 * 1024;
const AVERROR_ENOMEM: i32 = -12;
const VIDEO_STREAM_INDEX: usize = 0;
const AUDIO_STREAM_INDEX: usize = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fmp4SegmenterError(String);

impl Fmp4SegmenterError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn ffmpeg(operation: &str, code: i32) -> Self {
        Self(format!(
            "{operation} failed: {} ({code})",
            ffmpeg::Error::from(code)
        ))
    }
}

impl fmt::Display for Fmp4SegmenterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fmp4SegmenterError {}

struct MemoryWriteState {
    bytes: Vec<u8>,
    allocation_failed: bool,
}

unsafe extern "C" fn write_packet(opaque: *mut c_void, buffer: *const u8, size: i32) -> i32 {
    if opaque.is_null() || buffer.is_null() || size < 0 {
        return ffmpeg::ffi::AVERROR_INVALIDDATA;
    }
    let state = unsafe { &mut *(opaque as *mut MemoryWriteState) };
    let bytes = unsafe { slice::from_raw_parts(buffer, size as usize) };
    if state.bytes.try_reserve(bytes.len()).is_err() {
        state.allocation_failed = true;
        return AVERROR_ENOMEM;
    }
    state.bytes.extend_from_slice(bytes);
    size
}

struct MuxerResources {
    context: *mut ffmpeg::ffi::AVFormatContext,
    avio: *mut ffmpeg::ffi::AVIOContext,
    state: *mut MemoryWriteState,
    lifecycle: MuxerLifecycle,
}

/// `avio_alloc_context` may replace its buffer internally, so cleanup must free
/// the buffer currently stored in the AVIOContext rather than the pointer that
/// was originally passed to `avio_alloc_context`.
unsafe fn free_avio_buffer_then_context(avio: &mut *mut ffmpeg::ffi::AVIOContext) {
    if (*avio).is_null() {
        return;
    }
    unsafe {
        ffmpeg::ffi::av_freep(ptr::addr_of_mut!((**avio).buffer).cast());
        ffmpeg::ffi::avio_context_free(avio);
    }
}

unsafe impl Send for MuxerResources {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MuxerLifecycle {
    Allocated,
    HeaderWritten,
    TrailerWritten,
}

impl MuxerResources {
    fn allocate() -> Result<Self, Fmp4SegmenterError> {
        let mut context = ptr::null_mut();
        let result = unsafe {
            ffmpeg::ffi::avformat_alloc_output_context2(
                &mut context,
                ptr::null(),
                b"mp4\0".as_ptr().cast(),
                ptr::null(),
            )
        };
        if result < 0 || context.is_null() {
            if !context.is_null() {
                unsafe { ffmpeg::ffi::avformat_free_context(context) };
            }
            return Err(Fmp4SegmenterError::ffmpeg(
                "avformat_alloc_output_context2(mp4)",
                result,
            ));
        }

        let state = Box::into_raw(Box::new(MemoryWriteState {
            bytes: Vec::new(),
            allocation_failed: false,
        }));
        let buffer = unsafe { ffmpeg::ffi::av_malloc(AVIO_BUFFER_SIZE) as *mut u8 };
        if buffer.is_null() {
            unsafe {
                drop(Box::from_raw(state));
                ffmpeg::ffi::avformat_free_context(context);
            }
            return Err(Fmp4SegmenterError::new("av_malloc failed for output AVIO"));
        }
        let avio = unsafe {
            ffmpeg::ffi::avio_alloc_context(
                buffer,
                AVIO_BUFFER_SIZE as i32,
                1,
                state.cast(),
                None,
                Some(write_packet),
                None,
            )
        };
        if avio.is_null() {
            unsafe {
                ffmpeg::ffi::av_free(buffer.cast());
                drop(Box::from_raw(state));
                ffmpeg::ffi::avformat_free_context(context);
            }
            return Err(Fmp4SegmenterError::new(
                "avio_alloc_context failed for output",
            ));
        }
        unsafe {
            (*context).pb = avio;
            (*context).flags |= ffmpeg::ffi::AVFMT_FLAG_CUSTOM_IO;
        }
        Ok(Self {
            context,
            avio,
            state,
            lifecycle: MuxerLifecycle::Allocated,
        })
    }

    fn take_output(&mut self) -> Result<Vec<u8>, Fmp4SegmenterError> {
        unsafe { ffmpeg::ffi::avio_flush(self.avio) };
        let state = unsafe { &mut *self.state };
        if state.allocation_failed {
            return Err(Fmp4SegmenterError::new(
                "output AVIO callback could not allocate memory",
            ));
        }
        Ok(mem::take(&mut state.bytes))
    }

    fn write_trailer(&mut self) -> Result<(), Fmp4SegmenterError> {
        if self.lifecycle == MuxerLifecycle::HeaderWritten {
            let result = unsafe { ffmpeg::ffi::av_write_trailer(self.context) };
            unsafe { ffmpeg::ffi::avio_flush(self.avio) };
            self.lifecycle = MuxerLifecycle::TrailerWritten;
            if !self.state.is_null() {
                unsafe { (*self.state).bytes.clear() };
            }
            if result < 0 {
                return Err(Fmp4SegmenterError::ffmpeg("av_write_trailer", result));
            }
        }
        Ok(())
    }
}

impl Drop for MuxerResources {
    fn drop(&mut self) {
        let _ = self.write_trailer();
        unsafe {
            if !self.context.is_null() {
                ffmpeg::ffi::avformat_free_context(self.context);
                self.context = ptr::null_mut();
            }
            if !self.avio.is_null() {
                free_avio_buffer_then_context(&mut self.avio);
            }
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
                self.state = ptr::null_mut();
            }
        }
    }
}

/// AVCDecoderConfigurationRecord または Annex-B SPS から RFC 6381 の avc1.PPCCLL を作る。
pub(crate) fn avc1_codecs_from_extradata(extradata: &[u8]) -> Result<String, Fmp4SegmenterError> {
    let triplet = if extradata.len() >= 4 && extradata[0] == 1 {
        Some((extradata[1], extradata[2], extradata[3]))
    } else {
        h264_nals(extradata)
            .find(|nal| !nal.is_empty() && nal[0] & 0x1f == 7 && nal.len() >= 4)
            .map(|sps| (sps[1], sps[2], sps[3]))
    };
    let (profile_idc, constraint_flags, level_idc) = triplet.ok_or_else(|| {
        Fmp4SegmenterError::new("H.264 extradata does not contain a readable SPS")
    })?;
    Ok(format!(
        "avc1.{profile_idc:02x}{constraint_flags:02x}{level_idc:02x}"
    ))
}

/// MPEG-4 AudioSpecificConfig の実出力から RFC 6381 `mp4a.40.AOT` を作る。
pub(crate) fn mp4a_codecs_from_extradata(extradata: &[u8]) -> Result<String, Fmp4SegmenterError> {
    let first = *extradata
        .first()
        .ok_or_else(|| Fmp4SegmenterError::new("AAC encoder extradata is empty"))?;
    let mut audio_object_type = u16::from(first >> 3);
    if audio_object_type == 31 {
        let second = *extradata.get(1).ok_or_else(|| {
            Fmp4SegmenterError::new("extended AAC AudioSpecificConfig is truncated")
        })?;
        audio_object_type = 32 + u16::from(((first & 0x07) << 3) | (second >> 5));
    }
    if audio_object_type == 0 {
        return Err(Fmp4SegmenterError::new(
            "AAC AudioSpecificConfig has object type zero",
        ));
    }
    Ok(format!("mp4a.40.{audio_object_type}"))
}

fn packet_contains_idr(data: &[u8]) -> bool {
    h264_nals(data).any(|nal| !nal.is_empty() && nal[0] & 0x1f == 5)
}

fn h264_nals(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let annex_b = annex_b_nals(data);
    let length_prefixed = length_prefixed_nals(data);
    annex_b
        .or_else(|| length_prefixed)
        .unwrap_or_default()
        .into_iter()
}

fn annex_b_nals(data: &[u8]) -> Option<Vec<&[u8]>> {
    if !data.starts_with(&[0, 0, 1]) && !data.starts_with(&[0, 0, 0, 1]) {
        return None;
    }
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        let prefix_len = if data[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        starts.push((index, prefix_len));
        index += prefix_len;
    }
    if starts.is_empty() {
        return None;
    }
    Some(
        starts
            .iter()
            .enumerate()
            .filter_map(|(position, (start, prefix_len))| {
                let nal_start = start + prefix_len;
                let nal_end = starts
                    .get(position + 1)
                    .map_or(data.len(), |(next, _)| *next);
                (nal_start < nal_end).then_some(&data[nal_start..nal_end])
            })
            .collect(),
    )
}

fn length_prefixed_nals(data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut nals = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let length_bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        offset += 4;
        if length == 0 || offset.checked_add(length)? > data.len() {
            return None;
        }
        nals.push(&data[offset..offset + length]);
        offset += length;
    }
    (!nals.is_empty()).then_some(nals)
}

fn encoder_extradata(
    context: *const ffmpeg::ffi::AVCodecContext,
    stream_name: &str,
) -> Result<Vec<u8>, Fmp4SegmenterError> {
    if context.is_null() {
        return Err(Fmp4SegmenterError::new("encoder context is null"));
    }
    let context = unsafe { &*context };
    if context.extradata.is_null() || context.extradata_size <= 0 {
        return Err(Fmp4SegmenterError::new(format!(
            "{stream_name} encoder did not publish global extradata"
        )));
    }
    Ok(unsafe {
        slice::from_raw_parts(context.extradata, context.extradata_size as usize).to_vec()
    })
}

fn has_top_level_boxes(data: &[u8], required: &[[u8; 4]]) -> bool {
    let mut found = Vec::new();
    let mut offset = 0_usize;
    while offset + 8 <= data.len() {
        let size32 = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
        let (header_size, box_size) = match size32 {
            0 => (8, data.len() - offset),
            1 if offset + 16 <= data.len() => {
                let size64 = u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap());
                let Ok(size64) = usize::try_from(size64) else {
                    return false;
                };
                (16, size64)
            }
            _ => (8, size32),
        };
        if box_size < header_size || offset.saturating_add(box_size) > data.len() {
            return false;
        }
        found.push(kind);
        offset += box_size;
    }
    offset == data.len() && required.iter().all(|kind| found.contains(kind))
}

fn split_delayed_init_and_first_media(
    data: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Fmp4SegmenterError> {
    let mut init = Vec::new();
    let mut media = Vec::new();
    let mut offset = 0_usize;
    while offset + 8 <= data.len() {
        let size32 = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
        let box_size = match size32 {
            0 => data.len() - offset,
            1 if offset + 16 <= data.len() => {
                let size64 = u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap());
                usize::try_from(size64)
                    .map_err(|_| Fmp4SegmenterError::new("MP4 box size exceeds usize"))?
            }
            _ => size32,
        };
        if box_size < 8 || offset.saturating_add(box_size) > data.len() {
            return Err(Fmp4SegmenterError::new(
                "delayed MP4 output contains an invalid top-level box",
            ));
        }
        let target = if kind == *b"ftyp" || kind == *b"moov" {
            &mut init
        } else {
            &mut media
        };
        target.extend_from_slice(&data[offset..offset + box_size]);
        offset += box_size;
    }
    if offset != data.len()
        || !has_top_level_boxes(&init, &[*b"ftyp", *b"moov"])
        || !has_top_level_boxes(&media, &[*b"moof", *b"mdat"])
    {
        return Err(Fmp4SegmenterError::new(
            "delayed MP4 output did not contain complete init and media boxes",
        ));
    }
    Ok((init, media))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FragmentState {
    Empty,
    Writing {
        start_dts: i64,
        end_dts: i64,
    },
    AwaitingIdr {
        start_dts: i64,
        end_dts: i64,
        nominal_boundary_dts: i64,
    },
}

/// Zero-based position on the CFR source timeline.
///
/// A frame dropped before submission to the encoder leaves a gap in this index;
/// callers must not renumber the frames that were actually submitted. This keeps
/// forced-IDR requests aligned to the source timeline rather than encoder load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CfrTimelineFrameIndex(u64);

impl CfrTimelineFrameIndex {
    pub(crate) fn new(index: u64) -> Self {
        Self(index)
    }

    pub(crate) fn value(self) -> u64 {
        self.0
    }

    fn is_segment_boundary(self, keyint_frames: u32) -> bool {
        self.0 % u64::from(keyint_frames) == 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fmp4SegmenterStats {
    /// A nominal two-second boundary that had to be extended to a later IDR.
    pub(crate) delayed_idr_boundaries: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmenterLifecycle {
    Active,
    #[allow(dead_code)]
    // Only finite encoder fixtures finalize; live sessions stop by cancellation.
    Finished,
    Failed,
}

#[derive(Debug)]
enum InitSegmentState {
    /// `delay_moov` may emit an MP4 prefix before the first fragment is flushed.
    Pending {
        muxer_prefix: Vec<u8>,
    },
    Ready(Vec<u8>),
}

pub(crate) struct Fmp4Segmenter {
    muxer: MuxerResources,
    init_segment: InitSegmentState,
    codecs: String,
    bandwidth_bps: u64,
    width: u32,
    height: u32,
    video_source_time_base: ffmpeg::ffi::AVRational,
    video_stream_time_base: ffmpeg::ffi::AVRational,
    audio_source_time_base: ffmpeg::ffi::AVRational,
    audio_stream_time_base: ffmpeg::ffi::AVRational,
    keyint_frames: u32,
    fragment: FragmentState,
    last_video_pts: Option<i64>,
    last_video_dts: Option<i64>,
    last_audio_pts: Option<i64>,
    last_audio_dts: Option<i64>,
    last_audio_end_dts: Option<i64>,
    flushed_audio_before_dts: Option<i64>,
    stats: Fmp4SegmenterStats,
    ring: SegmentRing,
    lifecycle: SegmenterLifecycle,
}

unsafe impl Send for Fmp4Segmenter {}

impl Fmp4Segmenter {
    pub(crate) fn with_capacity(
        video_encoder: &ffmpeg::codec::encoder::video::Encoder,
        audio_encoder: &ffmpeg::codec::encoder::audio::Encoder,
        frame_rate: FrameRate,
        capacity: usize,
    ) -> Result<Self, Fmp4SegmenterError> {
        ffmpeg::init().map_err(|error| {
            Fmp4SegmenterError::new(format!("FFmpeg initialization failed: {error}"))
        })?;
        let ring = SegmentRing::new(capacity).map_err(Fmp4SegmenterError::new)?;
        let video_encoder_context = unsafe { video_encoder.as_ptr() };
        let audio_encoder_context = unsafe { audio_encoder.as_ptr() };
        let video_extradata = encoder_extradata(video_encoder_context, "H.264")?;
        let audio_extradata = encoder_extradata(audio_encoder_context, "AAC")?;
        let video_codecs = avc1_codecs_from_extradata(&video_extradata)?;
        let audio_codecs = mp4a_codecs_from_extradata(&audio_extradata)?;
        let codecs = format!("{video_codecs},{audio_codecs}");
        let (video_source_time_base, video_bitrate_bps, width, height) = unsafe {
            let context = &*video_encoder_context;
            (
                context.time_base,
                context.bit_rate.max(1) as u64,
                u32::try_from(context.width)
                    .map_err(|_| Fmp4SegmenterError::new("invalid encoder width"))?,
                u32::try_from(context.height)
                    .map_err(|_| Fmp4SegmenterError::new("invalid encoder height"))?,
            )
        };
        let (audio_source_time_base, audio_bitrate_bps) = unsafe {
            let context = &*audio_encoder_context;
            (context.time_base, context.bit_rate.max(1) as u64)
        };
        if width == 0
            || height == 0
            || video_source_time_base.num <= 0
            || video_source_time_base.den <= 0
            || audio_source_time_base.num <= 0
            || audio_source_time_base.den <= 0
        {
            return Err(Fmp4SegmenterError::new(
                "encoder dimensions or time bases are invalid",
            ));
        }
        let expected_time_base = ffmpeg::ffi::AVRational {
            num: frame_rate.denominator as i32,
            den: frame_rate.numerator as i32,
        };
        if unsafe { ffmpeg::ffi::av_cmp_q(video_source_time_base, expected_time_base) } != 0 {
            return Err(Fmp4SegmenterError::new(
                "encoder time base does not match the segment frame rate",
            ));
        }
        crate::logger::log(format!(
            "remote-stream fMP4 muxer: codecs={codecs} output={width}x{height} keyint_frames={}",
            frame_rate.keyint_frames()
        ));
        let bandwidth_bps = video_bitrate_bps.saturating_add(audio_bitrate_bps);

        let mut muxer = MuxerResources::allocate()?;
        let video_stream = unsafe { ffmpeg::ffi::avformat_new_stream(muxer.context, ptr::null()) };
        if video_stream.is_null() {
            return Err(Fmp4SegmenterError::new("video avformat_new_stream failed"));
        }
        let result = unsafe {
            ffmpeg::ffi::avcodec_parameters_from_context(
                (*video_stream).codecpar,
                video_encoder_context,
            )
        };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg(
                "avcodec_parameters_from_context(video)",
                result,
            ));
        }
        unsafe {
            (*video_stream).time_base = video_source_time_base;
            (*(*video_stream).codecpar).codec_tag = 0;
        }
        let audio_stream = unsafe { ffmpeg::ffi::avformat_new_stream(muxer.context, ptr::null()) };
        if audio_stream.is_null() {
            return Err(Fmp4SegmenterError::new("audio avformat_new_stream failed"));
        }
        let result = unsafe {
            ffmpeg::ffi::avcodec_parameters_from_context(
                (*audio_stream).codecpar,
                audio_encoder_context,
            )
        };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg(
                "avcodec_parameters_from_context(audio)",
                result,
            ));
        }
        unsafe {
            (*audio_stream).time_base = audio_source_time_base;
            (*(*audio_stream).codecpar).codec_tag = 0;
        }

        let mut options = ptr::null_mut();
        let option_result = unsafe {
            ffmpeg::ffi::av_dict_set(
                &mut options,
                b"movflags\0".as_ptr().cast(),
                // FFmpeg の movflag 名は default_base_moof。生成される tfhd の
                // default-base-is-moof flag（ISO BMFF 名）に対応する。
                b"frag_custom+delay_moov+default_base_moof+cmaf\0"
                    .as_ptr()
                    .cast(),
                0,
            )
        };
        if option_result < 0 {
            unsafe { ffmpeg::ffi::av_dict_free(&mut options) };
            return Err(Fmp4SegmenterError::ffmpeg(
                "av_dict_set(movflags)",
                option_result,
            ));
        }
        let result = unsafe { ffmpeg::ffi::avformat_write_header(muxer.context, &mut options) };
        unsafe { ffmpeg::ffi::av_dict_free(&mut options) };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg("avformat_write_header", result));
        }
        muxer.lifecycle = MuxerLifecycle::HeaderWritten;
        let video_stream_time_base = unsafe { (*video_stream).time_base };
        let audio_stream_time_base = unsafe { (*audio_stream).time_base };
        // AAC priming starts at DTS -1024. delay_moov lets the muxer build the edit list from
        // real first packets; init bytes are emitted with the first fragment and split there.
        let muxer_prefix = muxer.take_output()?;

        Ok(Self {
            muxer,
            init_segment: InitSegmentState::Pending { muxer_prefix },
            codecs,
            bandwidth_bps,
            width,
            height,
            video_source_time_base,
            video_stream_time_base,
            audio_source_time_base,
            audio_stream_time_base,
            keyint_frames: frame_rate.keyint_frames(),
            fragment: FragmentState::Empty,
            last_video_pts: None,
            last_video_dts: None,
            last_audio_pts: None,
            last_audio_dts: None,
            last_audio_end_dts: None,
            flushed_audio_before_dts: None,
            stats: Fmp4SegmenterStats::default(),
            ring,
            lifecycle: SegmenterLifecycle::Active,
        })
    }

    /// `delay_moov` のため、最初の media segment が確定するまでは `None`。
    pub(crate) fn init_segment(&self) -> Option<&[u8]> {
        match &self.init_segment {
            InitSegmentState::Pending { .. } => None,
            InitSegmentState::Ready(bytes) => Some(bytes),
        }
    }

    pub(crate) fn codecs(&self) -> &str {
        &self.codecs
    }

    /// init と最初の media segment が同時に公開可能になってから返す。
    pub(crate) fn master_playlist(&self) -> Option<String> {
        self.init_segment()
            .map(|_| master_playlist(&self.codecs, self.bandwidth_bps, self.width, self.height))
    }

    /// `EXT-X-MAP` が指す init を取得できる状態になってから返す。
    pub(crate) fn media_playlist(&self) -> Option<String> {
        self.init_segment().map(|_| self.ring.media_playlist())
    }

    pub(crate) fn segment(&self, sequence: u64) -> SegmentLookup<'_> {
        self.ring.get(sequence)
    }

    pub(crate) fn buffered_duration_secs(&self) -> f64 {
        self.ring.buffered_duration_secs()
    }

    pub(crate) fn effective_bitrate_bps(&self) -> u64 {
        self.ring.effective_bitrate_bps()
    }

    #[cfg(test)]
    pub(crate) fn media_sequence(&self) -> u64 {
        self.ring.media_sequence()
    }

    #[cfg(test)]
    pub(crate) fn next_sequence(&self) -> u64 {
        self.ring.next_sequence()
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> Fmp4SegmenterStats {
        self.stats
    }

    /// CFR source timeline 上の 2 秒境界だけを I frame 指定する。encoder 側の
    /// forced-IDR 設定と合わせ、各 fragment の先頭 packet を IDR にする。
    ///
    /// `timeline_frame_index` は submitted frame の連番ではない。tap や encoder が frame を
    /// 落としても timeline 上の位置は詰めず、次の 2 秒境界を同じ source 時刻に保つ。
    pub(crate) fn prepare_video_frame(
        &self,
        timeline_frame_index: CfrTimelineFrameIndex,
        frame: &mut ffmpeg::util::frame::video::Video,
    ) -> bool {
        let boundary = timeline_frame_index.is_segment_boundary(self.keyint_frames);
        frame.set_kind(if boundary {
            ffmpeg::picture::Type::I
        } else {
            ffmpeg::picture::Type::None
        });
        boundary
    }

    pub(crate) fn push_packet(
        &mut self,
        packet: &ffmpeg::Packet,
    ) -> Result<Option<u64>, Fmp4SegmenterError> {
        match self.lifecycle {
            SegmenterLifecycle::Active => {}
            SegmenterLifecycle::Finished => {
                return Err(Fmp4SegmenterError::new(
                    "cannot push a packet after segmenter finish",
                ));
            }
            SegmenterLifecycle::Failed => {
                return Err(Fmp4SegmenterError::new(
                    "cannot push a packet after a segmenter failure",
                ));
            }
        }
        let result = self.push_video_packet_inner(packet);
        if result.is_err() {
            self.lifecycle = SegmenterLifecycle::Failed;
        }
        result
    }

    pub(crate) fn push_audio_packet(
        &mut self,
        packet: &ffmpeg::Packet,
    ) -> Result<(), Fmp4SegmenterError> {
        match self.lifecycle {
            SegmenterLifecycle::Active => {}
            SegmenterLifecycle::Finished => {
                return Err(Fmp4SegmenterError::new(
                    "cannot push audio after segmenter finish",
                ));
            }
            SegmenterLifecycle::Failed => {
                return Err(Fmp4SegmenterError::new(
                    "cannot push audio after a segmenter failure",
                ));
            }
        }
        let result = self.push_audio_packet_inner(packet);
        if result.is_err() {
            self.lifecycle = SegmenterLifecycle::Failed;
        }
        result
    }

    fn push_audio_packet_inner(
        &mut self,
        packet: &ffmpeg::Packet,
    ) -> Result<(), Fmp4SegmenterError> {
        let pts = packet
            .pts()
            .ok_or_else(|| Fmp4SegmenterError::new("encoded audio packet has no PTS"))?;
        let dts = packet
            .dts()
            .ok_or_else(|| Fmp4SegmenterError::new("encoded audio packet has no DTS"))?;
        if let Some(last_dts) = self.last_audio_dts
            && dts <= last_dts
        {
            return Err(Fmp4SegmenterError::new(format!(
                "encoded audio DTS is not strictly increasing: {dts} <= {last_dts}"
            )));
        }
        if let Some(last_pts) = self.last_audio_pts
            && pts <= last_pts
        {
            return Err(Fmp4SegmenterError::new(format!(
                "encoded audio PTS is not strictly increasing: {pts} <= {last_pts}"
            )));
        }
        if let Some(floor) = self.flushed_audio_before_dts
            && dts < floor
        {
            return Err(Fmp4SegmenterError::new(format!(
                "audio packet DTS {dts} arrived after fragment containing timestamps before {floor} was flushed"
            )));
        }
        let duration = packet.duration().max(1);
        let mut mux_packet = packet.clone();
        mux_packet.set_stream(AUDIO_STREAM_INDEX);
        mux_packet.set_duration(duration);
        mux_packet.rescale_ts(
            ffmpeg::Rational::from(self.audio_source_time_base),
            ffmpeg::Rational::from(self.audio_stream_time_base),
        );
        let result = unsafe {
            ffmpeg::ffi::av_interleaved_write_frame(self.muxer.context, mux_packet.as_mut_ptr())
        };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg(
                "av_interleaved_write_frame(audio)",
                result,
            ));
        }
        self.last_audio_pts = Some(pts);
        self.last_audio_dts = Some(dts);
        self.last_audio_end_dts = Some(dts.saturating_add(duration));
        Ok(())
    }

    fn push_video_packet_inner(
        &mut self,
        packet: &ffmpeg::Packet,
    ) -> Result<Option<u64>, Fmp4SegmenterError> {
        let pts = packet
            .pts()
            .ok_or_else(|| Fmp4SegmenterError::new("encoded packet has no PTS"))?;
        let dts = packet
            .dts()
            .ok_or_else(|| Fmp4SegmenterError::new("encoded packet has no DTS"))?;
        if let Some(last_dts) = self.last_video_dts
            && dts <= last_dts
        {
            return Err(Fmp4SegmenterError::new(format!(
                "encoded packet DTS is not strictly increasing: {dts} <= {last_dts}"
            )));
        }
        if let Some(last_pts) = self.last_video_pts
            && pts <= last_pts
        {
            return Err(Fmp4SegmenterError::new(format!(
                "encoded packet PTS is not strictly increasing: {pts} <= {last_pts}"
            )));
        }
        let packet_is_idr = || {
            let data = packet
                .data()
                .ok_or_else(|| Fmp4SegmenterError::new("encoded packet has no payload"))?;
            Ok::<bool, Fmp4SegmenterError>(packet.is_key() && packet_contains_idr(data))
        };
        let mut completed = None;
        if let FragmentState::Writing { start_dts, end_dts } = self.fragment {
            let boundary_dts = start_dts.saturating_add(i64::from(self.keyint_frames));
            if dts >= boundary_dts {
                if packet_is_idr()? {
                    if dts > boundary_dts {
                        self.note_delayed_idr_boundary(boundary_dts, dts);
                    }
                    completed = self.flush_current_segment(Some(dts))?;
                } else {
                    self.note_delayed_idr_boundary(boundary_dts, dts);
                    self.fragment = FragmentState::AwaitingIdr {
                        start_dts,
                        end_dts,
                        nominal_boundary_dts: boundary_dts,
                    };
                }
            }
        }
        if matches!(self.fragment, FragmentState::AwaitingIdr { .. }) && packet_is_idr()? {
            completed = self.flush_current_segment(Some(dts))?;
        }
        if self.fragment == FragmentState::Empty && !packet_is_idr()? {
            return Err(Fmp4SegmenterError::new(format!(
                "segment boundary packet at PTS {pts} is not an IDR"
            )));
        }

        let packet_duration = packet.duration().max(1);
        let mut mux_packet = packet.clone();
        mux_packet.set_stream(VIDEO_STREAM_INDEX);
        mux_packet.set_duration(packet_duration);
        mux_packet.rescale_ts(
            ffmpeg::Rational::from(self.video_source_time_base),
            ffmpeg::Rational::from(self.video_stream_time_base),
        );
        let result = unsafe {
            ffmpeg::ffi::av_interleaved_write_frame(self.muxer.context, mux_packet.as_mut_ptr())
        };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg(
                "av_interleaved_write_frame",
                result,
            ));
        }

        self.last_video_pts = Some(pts);
        self.last_video_dts = Some(dts);
        self.fragment = match self.fragment {
            FragmentState::Empty => FragmentState::Writing {
                start_dts: dts,
                end_dts: dts.saturating_add(packet_duration),
            },
            FragmentState::Writing { start_dts, .. } => FragmentState::Writing {
                start_dts,
                end_dts: dts.saturating_add(packet_duration),
            },
            FragmentState::AwaitingIdr {
                start_dts,
                nominal_boundary_dts,
                ..
            } => FragmentState::AwaitingIdr {
                start_dts,
                end_dts: dts.saturating_add(packet_duration),
                nominal_boundary_dts,
            },
        };
        Ok(completed)
    }

    fn note_delayed_idr_boundary(&mut self, nominal_boundary_dts: i64, observed_dts: i64) {
        self.stats.delayed_idr_boundaries = self.stats.delayed_idr_boundaries.saturating_add(1);
        crate::logger::log(format!(
            "remote-stream fMP4 boundary delayed: nominal_dts={nominal_boundary_dts} observed_dts={observed_dts} delayed_idr_boundaries={}",
            self.stats.delayed_idr_boundaries
        ));
    }

    fn flush_current_segment(
        &mut self,
        boundary_dts: Option<i64>,
    ) -> Result<Option<u64>, Fmp4SegmenterError> {
        let (start_dts, last_packet_end_dts) = match self.fragment {
            FragmentState::Empty => return Ok(None),
            FragmentState::Writing { start_dts, end_dts }
            | FragmentState::AwaitingIdr {
                start_dts, end_dts, ..
            } => (start_dts, end_dts),
        };
        let end_dts = boundary_dts.unwrap_or(last_packet_end_dts);
        if end_dts <= start_dts {
            return Err(Fmp4SegmenterError::new(
                "media segment duration is not positive",
            ));
        }
        let audio_boundary_dts = boundary_dts.map(|video_dts| unsafe {
            ffmpeg::ffi::av_rescale_q(
                video_dts,
                self.video_source_time_base,
                self.audio_source_time_base,
            )
        });
        if let (Some(audio_boundary), Some(last_audio_end)) =
            (audio_boundary_dts, self.last_audio_end_dts)
            && last_audio_end < audio_boundary
        {
            return Err(Fmp4SegmenterError::new(format!(
                "audio packets only cover DTS {last_audio_end}, before video fragment boundary {audio_boundary}"
            )));
        }
        // av_interleaved_write_frame may retain packets while waiting for the other stream.
        // Drain that queue before frag_custom's explicit av_write_frame(NULL), otherwise the
        // just-submitted tail audio packets can miss the fragment being finalized here.
        let result =
            unsafe { ffmpeg::ffi::av_interleaved_write_frame(self.muxer.context, ptr::null_mut()) };
        if result < 0 {
            return Err(Fmp4SegmenterError::ffmpeg(
                "av_interleaved_write_frame(queue flush)",
                result,
            ));
        }
        // With delay_moov the first explicit flush publishes ftyp+moov; the following flush
        // publishes the buffered first moof+mdat. Later boundaries need one flush as before.
        let init_is_pending = matches!(self.init_segment, InitSegmentState::Pending { .. });
        let flush_count = if init_is_pending { 2 } else { 1 };
        for _ in 0..flush_count {
            let result =
                unsafe { ffmpeg::ffi::av_write_frame(self.muxer.context, ptr::null_mut()) };
            if result < 0 {
                return Err(Fmp4SegmenterError::ffmpeg(
                    "av_write_frame(fragment flush)",
                    result,
                ));
            }
        }
        let mut bytes = self.muxer.take_output()?;
        let mut completed_init = None;
        if let InitSegmentState::Pending { muxer_prefix } = &self.init_segment {
            if !muxer_prefix.is_empty() {
                let mut combined = Vec::with_capacity(muxer_prefix.len() + bytes.len());
                combined.extend_from_slice(muxer_prefix);
                combined.extend_from_slice(&bytes);
                bytes = combined;
            }
            let (init, media) = split_delayed_init_and_first_media(&bytes)?;
            completed_init = Some(init);
            bytes = media;
        }
        if !has_top_level_boxes(&bytes, &[*b"moof", *b"mdat"]) {
            return Err(Fmp4SegmenterError::new(
                "mp4 muxer fragment was not a moof+mdat media segment",
            ));
        }
        let duration_secs = (end_dts - start_dts) as f64
            * f64::from(self.video_source_time_base.num)
            / f64::from(self.video_source_time_base.den);
        let sequence = self
            .ring
            .push(duration_secs, bytes)
            .map_err(Fmp4SegmenterError::new)?;
        if let Some(init) = completed_init {
            self.init_segment = InitSegmentState::Ready(init);
        }
        if let Some(audio_boundary) = audio_boundary_dts {
            self.flushed_audio_before_dts = Some(audio_boundary);
        }
        self.fragment = FragmentState::Empty;
        Ok(Some(sequence))
    }

    /// session stop/test 終了時だけ、2 秒未満の末尾 fragment も確定して trailer を閉じる。
    pub(crate) fn finish(&mut self) -> Result<Option<u64>, Fmp4SegmenterError> {
        match self.lifecycle {
            SegmenterLifecycle::Finished => return Ok(None),
            SegmenterLifecycle::Failed => {
                return Err(Fmp4SegmenterError::new(
                    "cannot finish after a segmenter failure",
                ));
            }
            SegmenterLifecycle::Active => {}
        }
        let sequence = match self.flush_current_segment(None) {
            Ok(sequence) => sequence,
            Err(error) => {
                self.lifecycle = SegmenterLifecycle::Failed;
                return Err(error);
            }
        };
        if let Err(error) = self.muxer.write_trailer() {
            self.lifecycle = SegmenterLifecycle::Failed;
            return Err(error);
        }
        self.lifecycle = SegmenterLifecycle::Finished;
        Ok(sequence)
    }
}

impl Drop for Fmp4Segmenter {
    fn drop(&mut self) {
        if self.lifecycle == SegmenterLifecycle::Active {
            let _ = self.flush_current_segment(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::SeekFrom;

    use super::*;
    use crate::video::audio::ProcessedChunk;
    use crate::video::stream::audio_encoder::{OpenedAacEncoder, open_aac_encoder};
    use crate::video::stream::encoder::{
        AUDIO_PROFILE_ID, EncoderPreference, H264EncoderKind, H264InputFormat, open_h264_encoder,
    };
    use crate::video::stream::quality::{OutputDimensions, QualityPreset, StreamOutputParameters};
    use crate::video::stream::timeline::StreamTimeline;
    use crate::video::stream::video_tap::{TappedVideoFrame, open_video_stream_encoder};

    const AVERROR_EOF: i32 = -0x2046_4f45;
    const FFMPEG_EAGAIN: i32 = 11;

    #[test]
    fn codecs_are_parsed_from_avcc_and_annex_b_sps() {
        assert_eq!(
            avc1_codecs_from_extradata(&[1, 0x64, 0x00, 0x28]).unwrap(),
            "avc1.640028"
        );
        assert_eq!(
            avc1_codecs_from_extradata(&[
                0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xaa, 0, 0, 1, 0x68, 0xbb
            ])
            .unwrap(),
            "avc1.42c01f"
        );
        assert!(avc1_codecs_from_extradata(&[]).is_err());
        assert_eq!(
            mp4a_codecs_from_extradata(&[0x12, 0x10]).unwrap(),
            "mp4a.40.2"
        );
        assert!(mp4a_codecs_from_extradata(&[]).is_err());
    }

    fn open_test_audio(bitrate_bps: u32) -> OpenedAacEncoder {
        open_aac_encoder(48_000, bitrate_bps, 0, StreamTimeline::new(0.0).unwrap()).unwrap()
    }

    fn mp4_boxes(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut boxes = Vec::new();
        while data.len() >= 8 {
            let size32 = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
            let kind = data[4..8].try_into().unwrap();
            let (header_size, box_size) = match size32 {
                0 => (8, data.len()),
                1 if data.len() >= 16 => {
                    let size64 = u64::from_be_bytes(data[8..16].try_into().unwrap());
                    let Ok(size64) = usize::try_from(size64) else {
                        break;
                    };
                    (16, size64)
                }
                1 => break,
                size => (8, size),
            };
            if box_size < header_size || box_size > data.len() {
                break;
            }
            boxes.push((kind, &data[header_size..box_size]));
            data = &data[box_size..];
        }
        boxes
    }

    fn mp4_child(data: &[u8], wanted: [u8; 4]) -> Option<&[u8]> {
        mp4_boxes(data)
            .into_iter()
            .find_map(|(kind, body)| (kind == wanted).then_some(body))
    }

    fn assert_avc_init_visual_dimensions(init: &[u8], expected_width: u32, expected_height: u32) {
        let moov = mp4_child(init, *b"moov").expect("init has moov");
        let video_trak = mp4_boxes(moov)
            .into_iter()
            .filter_map(|(kind, body)| (kind == *b"trak").then_some(body))
            .find(|trak| {
                let Some(mdia) = mp4_child(trak, *b"mdia") else {
                    return false;
                };
                let Some(minf) = mp4_child(mdia, *b"minf") else {
                    return false;
                };
                let Some(stbl) = mp4_child(minf, *b"stbl") else {
                    return false;
                };
                let Some(stsd) = mp4_child(stbl, *b"stsd") else {
                    return false;
                };
                stsd.len() >= 8 && mp4_child(&stsd[8..], *b"avc1").is_some()
            })
            .expect("init has an AVC video track");

        let tkhd = mp4_child(video_trak, *b"tkhd").expect("video track has tkhd");
        assert!(tkhd.len() >= 8);
        let tkhd_width =
            u32::from_be_bytes(tkhd[tkhd.len() - 8..tkhd.len() - 4].try_into().unwrap());
        let tkhd_height = u32::from_be_bytes(tkhd[tkhd.len() - 4..].try_into().unwrap());
        assert_eq!(tkhd_width, expected_width << 16);
        assert_eq!(tkhd_height, expected_height << 16);

        let mdia = mp4_child(video_trak, *b"mdia").unwrap();
        let minf = mp4_child(mdia, *b"minf").unwrap();
        let stbl = mp4_child(minf, *b"stbl").unwrap();
        let stsd = mp4_child(stbl, *b"stsd").unwrap();
        let avc1 = mp4_child(&stsd[8..], *b"avc1").unwrap();
        // VisualSampleEntry fields before width/height occupy 24 bytes after the box header.
        assert!(avc1.len() >= 28);
        let sample_width = u16::from_be_bytes(avc1[24..26].try_into().unwrap());
        let sample_height = u16::from_be_bytes(avc1[26..28].try_into().unwrap());
        assert_eq!(u32::from(sample_width), expected_width);
        assert_eq!(u32::from(sample_height), expected_height);
    }

    fn openh264_init_for_dimensions(width: u32, height: u32) -> (Vec<u8>, String) {
        let frame_rate = FrameRate::new(30, 1).unwrap();
        let output = StreamOutputParameters {
            dimensions: OutputDimensions { width, height },
            video_bitrate_bps: 400_000,
            audio_bitrate_bps: 64_000,
        };
        let mut opened = open_h264_encoder(
            EncoderPreference::Encoder(H264EncoderKind::OpenH264),
            output,
            frame_rate,
        )
        .unwrap();
        let audio = open_test_audio(output.audio_bitrate_bps);
        let mut segmenter =
            Fmp4Segmenter::with_capacity(&opened.encoder, &audio.encoder, frame_rate, 2).unwrap();
        let mut completed = Vec::new();
        for index in 0..=frame_rate.keyint_frames() {
            let mut frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg::format::Pixel::YUV420P,
                width,
                height,
            );
            fill_yuv420p(&mut frame, index as u8);
            frame.set_pts(Some(i64::from(index)));
            segmenter.prepare_video_frame(CfrTimelineFrameIndex::new(u64::from(index)), &mut frame);
            opened.encoder.send_frame(&frame).unwrap();
            drain_encoder(&mut opened.encoder, &mut segmenter, &mut completed);
        }
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].0, 0);
        (
            segmenter
                .init_segment()
                .expect("first segment completed init")
                .to_vec(),
            segmenter
                .master_playlist()
                .expect("first segment completed master playlist"),
        )
    }

    #[test]
    fn fmp4_and_master_playlist_use_visual_dimensions_with_and_without_sps_cropping() {
        for (width, height) in [(640, 360), (640, 368)] {
            let (init, master) = openh264_init_for_dimensions(width, height);
            assert_avc_init_visual_dimensions(&init, width, height);
            assert!(master.contains(&format!("RESOLUTION={width}x{height}")));
        }
    }

    #[test]
    fn idr_detection_accepts_annex_b_and_avcc_packets() {
        assert!(packet_contains_idr(&[0, 0, 1, 0x65, 1, 2]));
        assert!(packet_contains_idr(&[0, 0, 0, 3, 0x65, 1, 2]));
        assert!(!packet_contains_idr(&[0, 0, 0, 2, 0x41, 1]));
    }

    struct MemoryReadState {
        bytes: Vec<u8>,
        position: usize,
    }

    unsafe extern "C" fn read_packet(opaque: *mut c_void, buffer: *mut u8, size: i32) -> i32 {
        let state = unsafe { &mut *(opaque as *mut MemoryReadState) };
        let remaining = state.bytes.len().saturating_sub(state.position);
        let read_len = remaining.min(size.max(0) as usize);
        if read_len == 0 {
            return AVERROR_EOF;
        }
        unsafe {
            ptr::copy_nonoverlapping(state.bytes.as_ptr().add(state.position), buffer, read_len);
        }
        state.position += read_len;
        read_len as i32
    }

    unsafe extern "C" fn seek(opaque: *mut c_void, offset: i64, whence: i32) -> i64 {
        let state = unsafe { &mut *(opaque as *mut MemoryReadState) };
        if whence & ffmpeg::ffi::AVSEEK_SIZE != 0 {
            return state.bytes.len() as i64;
        }
        let base = match whence & 0x3 {
            0 => SeekFrom::Start(offset.max(0) as u64),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return -1,
        };
        let position = match base {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(value) => state.position as i128 + i128::from(value),
            SeekFrom::End(value) => state.bytes.len() as i128 + i128::from(value),
        };
        if position < 0 || position > state.bytes.len() as i128 {
            return -1;
        }
        state.position = position as usize;
        state.position as i64
    }

    #[derive(Debug)]
    struct PacketProbe {
        pts_secs: f64,
        dts_secs: f64,
        key: bool,
        idr: bool,
    }

    struct SegmentProbe {
        video_codecs: String,
        audio_codecs: String,
        audio_sample_rate: i32,
        audio_profile: i32,
        packets: Vec<PacketProbe>,
        audio_packet_pts_secs: Vec<f64>,
    }

    fn read_with_ffmpeg(init: &[u8], media: &[u8]) -> SegmentProbe {
        let mut bytes = Vec::with_capacity(init.len() + media.len());
        bytes.extend_from_slice(init);
        bytes.extend_from_slice(media);
        let state = Box::into_raw(Box::new(MemoryReadState { bytes, position: 0 }));
        let buffer = unsafe { ffmpeg::ffi::av_malloc(AVIO_BUFFER_SIZE) as *mut u8 };
        assert!(!buffer.is_null());
        let mut avio = unsafe {
            ffmpeg::ffi::avio_alloc_context(
                buffer,
                AVIO_BUFFER_SIZE as i32,
                0,
                state.cast(),
                Some(read_packet),
                None,
                Some(seek),
            )
        };
        assert!(!avio.is_null());
        let context = unsafe { ffmpeg::ffi::avformat_alloc_context() };
        assert!(!context.is_null());
        unsafe {
            (*context).pb = avio;
            (*context).flags |= ffmpeg::ffi::AVFMT_FLAG_CUSTOM_IO;
        }
        let mut context_ref = context;
        let result = unsafe {
            ffmpeg::ffi::avformat_open_input(
                &mut context_ref,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(
            result,
            0,
            "avformat_open_input: {}",
            ffmpeg::Error::from(result)
        );
        let result =
            unsafe { ffmpeg::ffi::avformat_find_stream_info(context_ref, ptr::null_mut()) };
        assert!(
            result >= 0,
            "avformat_find_stream_info: {}",
            ffmpeg::Error::from(result)
        );
        assert_eq!(unsafe { (*context_ref).nb_streams }, 2);
        let video_stream = unsafe { *(*context_ref).streams.add(VIDEO_STREAM_INDEX) };
        let audio_stream = unsafe { *(*context_ref).streams.add(AUDIO_STREAM_INDEX) };
        let video_parameters = unsafe { &*(*video_stream).codecpar };
        assert_eq!(
            ffmpeg::codec::Id::from(video_parameters.codec_id),
            ffmpeg::codec::Id::H264
        );
        let video_extradata = unsafe {
            slice::from_raw_parts(
                video_parameters.extradata,
                video_parameters.extradata_size as usize,
            )
        };
        let video_codecs = avc1_codecs_from_extradata(video_extradata).unwrap();
        let audio_parameters = unsafe { &*(*audio_stream).codecpar };
        assert_eq!(
            ffmpeg::codec::Id::from(audio_parameters.codec_id),
            ffmpeg::codec::Id::AAC
        );
        let audio_extradata = unsafe {
            slice::from_raw_parts(
                audio_parameters.extradata,
                audio_parameters.extradata_size as usize,
            )
        };
        let audio_codecs = mp4a_codecs_from_extradata(audio_extradata).unwrap();
        let audio_sample_rate = audio_parameters.sample_rate;
        let audio_profile = audio_parameters.profile;
        let video_time_base = unsafe { (*video_stream).time_base };
        let audio_time_base = unsafe { (*audio_stream).time_base };

        let mut packets = Vec::new();
        let mut audio_packet_pts_secs = Vec::new();
        loop {
            let mut packet = ffmpeg::Packet::empty();
            let result = unsafe { ffmpeg::ffi::av_read_frame(context_ref, packet.as_mut_ptr()) };
            if result < 0 {
                assert_eq!(ffmpeg::Error::from(result), ffmpeg::Error::Eof);
                break;
            }
            if packet.stream() == VIDEO_STREAM_INDEX {
                let seconds_per_tick =
                    f64::from(video_time_base.num) / f64::from(video_time_base.den);
                packets.push(PacketProbe {
                    pts_secs: packet.pts().unwrap() as f64 * seconds_per_tick,
                    dts_secs: packet.dts().unwrap() as f64 * seconds_per_tick,
                    key: packet.is_key(),
                    idr: packet.data().is_some_and(packet_contains_idr),
                });
            } else if packet.stream() == AUDIO_STREAM_INDEX {
                let seconds_per_tick =
                    f64::from(audio_time_base.num) / f64::from(audio_time_base.den);
                audio_packet_pts_secs.push(packet.pts().unwrap() as f64 * seconds_per_tick);
            }
        }
        unsafe {
            ffmpeg::ffi::avformat_close_input(&mut context_ref);
            free_avio_buffer_then_context(&mut avio);
            drop(Box::from_raw(state));
        }
        SegmentProbe {
            video_codecs,
            audio_codecs,
            audio_sample_rate,
            audio_profile,
            packets,
            audio_packet_pts_secs,
        }
    }

    fn fill_yuv420p(frame: &mut ffmpeg::util::frame::video::Video, index: u8) {
        for plane in 0..3 {
            let width = frame.plane_width(plane) as usize;
            let height = frame.plane_height(plane) as usize;
            let stride = frame.stride(plane);
            let value = if plane == 0 {
                32_u8.saturating_add(index % 160)
            } else {
                128
            };
            let data = frame.data_mut(plane);
            for row in 0..height {
                data[row * stride..row * stride + width].fill(value);
            }
        }
    }

    fn drain_encoder_filter(
        encoder: &mut ffmpeg::codec::encoder::video::Encoder,
        segmenter: &mut Fmp4Segmenter,
        completed: &mut Vec<(u64, Vec<u8>)>,
        should_drop: &mut impl FnMut(&ffmpeg::Packet) -> bool,
    ) {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    if should_drop(&packet) {
                        continue;
                    }
                    if let Some(sequence) = segmenter.push_packet(&packet).unwrap() {
                        let SegmentLookup::Found(segment) = segmenter.segment(sequence) else {
                            panic!("just-completed segment is absent");
                        };
                        completed.push((sequence, segment.bytes.clone()));
                    }
                }
                Err(ffmpeg::Error::Other { errno }) if errno == FFMPEG_EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => panic!("receive_packet failed: {error}"),
            }
        }
    }

    fn drain_encoder(
        encoder: &mut ffmpeg::codec::encoder::video::Encoder,
        segmenter: &mut Fmp4Segmenter,
        completed: &mut Vec<(u64, Vec<u8>)>,
    ) {
        drain_encoder_filter(encoder, segmenter, completed, &mut |_| false);
    }

    #[test]
    fn missing_boundary_idr_extends_segment_to_next_idr_with_actual_duration() {
        let frame_rate = FrameRate::new(30, 1).unwrap();
        let output = StreamOutputParameters {
            dimensions: OutputDimensions {
                width: 320,
                height: 180,
            },
            video_bitrate_bps: 400_000,
            audio_bitrate_bps: 64_000,
        };
        let mut opened = open_h264_encoder(
            EncoderPreference::Encoder(H264EncoderKind::OpenH264),
            output,
            frame_rate,
        )
        .unwrap();
        let audio = open_test_audio(output.audio_bitrate_bps);
        let mut segmenter =
            Fmp4Segmenter::with_capacity(&opened.encoder, &audio.encoder, frame_rate, 4).unwrap();
        let missing_boundary_dts = i64::from(frame_rate.keyint_frames());
        let mut dropped_boundary_idr = false;
        let mut drop_first_boundary_idr = |packet: &ffmpeg::Packet| {
            if packet.dts() != Some(missing_boundary_dts) {
                return false;
            }
            assert!(packet.is_key());
            assert!(packet.data().is_some_and(packet_contains_idr));
            dropped_boundary_idr = true;
            true
        };

        let mut completed = Vec::new();
        let frame_count = frame_rate.keyint_frames() * 3;
        for index in 0..frame_count {
            let mut frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg::format::Pixel::YUV420P,
                output.dimensions.width,
                output.dimensions.height,
            );
            fill_yuv420p(&mut frame, index as u8);
            frame.set_pts(Some(i64::from(index)));
            segmenter.prepare_video_frame(CfrTimelineFrameIndex::new(u64::from(index)), &mut frame);
            opened.encoder.send_frame(&frame).unwrap();
            drain_encoder_filter(
                &mut opened.encoder,
                &mut segmenter,
                &mut completed,
                &mut drop_first_boundary_idr,
            );
        }
        opened.encoder.send_eof().unwrap();
        drain_encoder_filter(
            &mut opened.encoder,
            &mut segmenter,
            &mut completed,
            &mut drop_first_boundary_idr,
        );
        assert!(dropped_boundary_idr);

        let final_sequence = segmenter.finish().unwrap().unwrap();
        let SegmentLookup::Found(final_segment) = segmenter.segment(final_sequence) else {
            panic!("final segment is absent");
        };
        completed.push((final_sequence, final_segment.bytes.clone()));
        assert_eq!(completed.len(), 2);
        assert_eq!(segmenter.stats().delayed_idr_boundaries, 1);

        let SegmentLookup::Found(extended) = segmenter.segment(0) else {
            panic!("extended segment is absent");
        };
        assert!((extended.duration_secs - 4.0).abs() < 1e-6);
        let playlist = segmenter
            .media_playlist()
            .expect("playlist is ready with the completed init segment");
        assert!(playlist.contains("#EXT-X-TARGETDURATION:4\n"));
        assert!(playlist.contains("#EXTINF:4.000000,\n0.m4s\n"));

        let SegmentLookup::Found(after_gap) = segmenter.segment(1) else {
            panic!("segment after the missing boundary is absent");
        };
        let probe = read_with_ffmpeg(
            segmenter
                .init_segment()
                .expect("init is ready with the first media segment"),
            &after_gap.bytes,
        );
        assert!(probe.packets[0].key);
        assert!(probe.packets[0].idr);
        assert!((probe.packets[0].dts_secs - 4.0).abs() < 1e-6);
    }

    #[test]
    fn openh264_fmp4_segments_round_trip_in_process() {
        let frame_rate = FrameRate::new(30, 1).unwrap();
        let output = StreamOutputParameters {
            dimensions: OutputDimensions {
                width: 320,
                height: 180,
            },
            video_bitrate_bps: 400_000,
            audio_bitrate_bps: 64_000,
        };
        let mut opened = open_h264_encoder(
            EncoderPreference::Encoder(H264EncoderKind::OpenH264),
            output,
            frame_rate,
        )
        .unwrap();
        assert_eq!(opened.input_format, H264InputFormat::Yuv420p);
        let audio = open_test_audio(output.audio_bitrate_bps);
        let mut segmenter =
            Fmp4Segmenter::with_capacity(&opened.encoder, &audio.encoder, frame_rate, 2).unwrap();
        assert!(segmenter.codecs().starts_with("avc1.42c0"));
        assert!(segmenter.codecs().ends_with(",mp4a.40.2"));
        assert!(segmenter.init_segment().is_none());
        assert!(segmenter.master_playlist().is_none());
        assert!(segmenter.media_playlist().is_none());

        let mut completed = Vec::new();
        let frame_count = frame_rate.keyint_frames() * 3;
        for index in 0..frame_count {
            let mut frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg::format::Pixel::YUV420P,
                output.dimensions.width,
                output.dimensions.height,
            );
            fill_yuv420p(&mut frame, index as u8);
            frame.set_pts(Some(i64::from(index)));
            assert_eq!(
                segmenter
                    .prepare_video_frame(CfrTimelineFrameIndex::new(u64::from(index)), &mut frame),
                index % frame_rate.keyint_frames() == 0
            );
            opened.encoder.send_frame(&frame).unwrap();
            drain_encoder(&mut opened.encoder, &mut segmenter, &mut completed);
        }
        opened.encoder.send_eof().unwrap();
        drain_encoder(&mut opened.encoder, &mut segmenter, &mut completed);
        let final_sequence = segmenter.finish().unwrap().unwrap();
        let SegmentLookup::Found(final_segment) = segmenter.segment(final_sequence) else {
            panic!("final segment is absent");
        };
        completed.push((final_sequence, final_segment.bytes.clone()));
        assert_eq!(completed.len(), 3);
        assert_eq!(segmenter.media_sequence(), 1);
        assert_eq!(segmenter.next_sequence(), 3);
        assert_eq!(segmenter.segment(0), SegmentLookup::Gone);
        assert!(
            segmenter
                .media_playlist()
                .expect("media playlist is ready after the first segment")
                .contains("#EXT-X-MEDIA-SEQUENCE:1\n")
        );
        let master = segmenter
            .master_playlist()
            .expect("master playlist is ready after the first segment");
        assert!(master.contains("CODECS="));
        assert!(master.contains(segmenter.codecs()));
        let init = segmenter
            .init_segment()
            .expect("init is ready after the first media segment");
        assert!(has_top_level_boxes(init, &[*b"ftyp", *b"moov"]));

        let mut all_packets = Vec::new();
        for (expected_sequence, (sequence, bytes)) in completed.iter().enumerate() {
            assert_eq!(*sequence, expected_sequence as u64);
            assert!(has_top_level_boxes(bytes, &[*b"moof", *b"mdat"]));
            let probe = read_with_ffmpeg(init, bytes);
            assert!(segmenter.codecs().starts_with(&probe.video_codecs));
            assert_eq!(probe.audio_codecs, "mp4a.40.2");
            assert_eq!(probe.packets.len(), frame_rate.keyint_frames() as usize);
            assert!(probe.packets[0].key);
            assert!(probe.packets[0].idr);
            let expected_start = expected_sequence as f64 * 2.0;
            assert!((probe.packets[0].dts_secs - expected_start).abs() < 1e-6);
            all_packets.extend(probe.packets);
        }

        for pair in all_packets.windows(2) {
            assert!(pair[1].dts_secs > pair[0].dts_secs);
            assert!(pair[1].pts_secs > pair[0].pts_secs);
            assert!((pair[1].dts_secs - pair[0].dts_secs - 1.0 / 30.0).abs() < 1e-6);
        }
        println!("libopenh264 CMAF CODECS={}", segmenter.codecs());
    }

    #[test]
    fn tapped_video_and_audio_round_trip_in_process_through_the_same_ffmpeg() {
        let frame_rate = FrameRate::new(30, 1).unwrap();
        let timeline = StreamTimeline::new(0.0).unwrap();
        let mut video = open_video_stream_encoder(
            EncoderPreference::Encoder(H264EncoderKind::OpenH264),
            QualityPreset::Minimum,
            960,
            540,
            frame_rate,
            0,
            timeline,
        )
        .unwrap();
        let output = video.output_parameters();
        assert_eq!(video.input_format(), H264InputFormat::Yuv420p);
        assert_eq!(output.dimensions.width, 640);
        assert_eq!(output.dimensions.height, 360);
        let mut audio = open_aac_encoder(48_000, output.audio_bitrate_bps, 0, timeline).unwrap();
        let mut segmenter =
            Fmp4Segmenter::with_capacity(video.encoder(), &audio.encoder, frame_rate, 4).unwrap();

        let mut video_packets = Vec::new();
        let frame_count = frame_rate.keyint_frames() * 2;
        for index in 0..frame_count {
            let mut frame =
                ffmpeg::util::frame::video::Video::new(ffmpeg::format::Pixel::YUV420P, 960, 540);
            fill_yuv420p(&mut frame, index as u8);
            video_packets.extend(
                video
                    .encode_frame(
                        TappedVideoFrame::from_owned_software_frame(
                            frame,
                            f64::from(index) / 30.0,
                            0,
                        )
                        .unwrap(),
                        &segmenter,
                    )
                    .unwrap(),
            );
        }
        video_packets.extend(video.finish().unwrap());
        assert_eq!(video.stats().submitted_frames, u64::from(frame_count));

        let audio_sample_rate = audio.input_sample_rate();
        let total_audio_samples = 4 * audio_sample_rate as usize;
        let chunk_pattern = [333_usize, 777, 2_048, 511, 1_025];
        let mut audio_packets = Vec::new();
        let mut cursor = 0_usize;
        let mut pattern_index = 0_usize;
        while cursor < total_audio_samples {
            let count = chunk_pattern[pattern_index % chunk_pattern.len()]
                .min(total_audio_samples - cursor);
            pattern_index += 1;
            let mut samples = Vec::with_capacity(count * 2);
            for sample_index in cursor..cursor + count {
                let phase =
                    sample_index as f32 * 440.0 * std::f32::consts::TAU / audio_sample_rate as f32;
                let sample = phase.sin() * 0.1;
                samples.extend_from_slice(&[sample, sample]);
            }
            audio_packets.extend(
                audio
                    .push_chunk(ProcessedChunk {
                        samples,
                        audible_pts_secs: cursor as f64 / f64::from(audio_sample_rate),
                        duration_secs: count as f64 / f64::from(audio_sample_rate),
                        source_secs_per_output_sec: 1.0,
                        seek_serial: 0,
                        pdc_latency_secs_at_process: 0.0,
                    })
                    .unwrap(),
            );
            cursor += count;
        }
        audio_packets.extend(audio.finish().unwrap());
        let expected_audio_pts = audio_packets
            .iter()
            .map(|packet| packet.pts().unwrap() as f64 / f64::from(audio.output_sample_rate()))
            .collect::<Vec<_>>();
        struct TimedPacket {
            dts_secs: f64,
            audio: bool,
            packet: ffmpeg::Packet,
        }
        let mut interleaved = video_packets
            .into_iter()
            .map(|packet| TimedPacket {
                dts_secs: packet.dts().unwrap() as f64 / 30.0,
                audio: false,
                packet,
            })
            .chain(audio_packets.into_iter().map(|packet| TimedPacket {
                dts_secs: packet.dts().unwrap() as f64 / f64::from(audio.output_sample_rate()),
                audio: true,
                packet,
            }))
            .collect::<Vec<_>>();
        interleaved.sort_by(|left, right| {
            left.dts_secs
                .partial_cmp(&right.dts_secs)
                .unwrap()
                .then_with(|| right.audio.cmp(&left.audio))
        });
        let mut completed = Vec::new();
        for timed in interleaved {
            if timed.audio {
                segmenter.push_audio_packet(&timed.packet).unwrap();
            } else if let Some(sequence) = segmenter.push_packet(&timed.packet).unwrap() {
                completed.push(sequence);
            }
        }
        completed.push(segmenter.finish().unwrap().unwrap());
        assert_eq!(completed, vec![0, 1]);
        let mut observed_audio_pts = Vec::new();
        let mut first_video_dts_secs = None;
        for sequence in completed {
            let SegmentLookup::Found(segment) = segmenter.segment(sequence) else {
                panic!("completed segment {sequence} is absent");
            };
            let probe = read_with_ffmpeg(
                segmenter
                    .init_segment()
                    .expect("init is ready with completed media segments"),
                &segment.bytes,
            );
            assert!(!probe.packets.is_empty());
            assert!(!probe.audio_packet_pts_secs.is_empty());
            assert_eq!(probe.audio_codecs, "mp4a.40.2");
            assert_eq!(probe.audio_sample_rate, audio.output_sample_rate() as i32);
            assert_eq!(probe.audio_profile, AUDIO_PROFILE_ID);
            if first_video_dts_secs.is_none() {
                first_video_dts_secs = probe.packets.first().map(|packet| packet.dts_secs);
            }
            observed_audio_pts.extend(probe.audio_packet_pts_secs);
        }
        assert_eq!(observed_audio_pts.len(), expected_audio_pts.len());
        let timestamp_shift = observed_audio_pts[0] - expected_audio_pts[0];
        for (observed, expected) in observed_audio_pts.iter().zip(expected_audio_pts) {
            assert!((observed - expected - timestamp_shift).abs() < 1.0e-9);
        }
        assert!(timestamp_shift.abs() < 1.0e-9);
        assert!((observed_audio_pts[1] - first_video_dts_secs.unwrap()).abs() < 1.0 / 30.0);
        assert!(
            segmenter
                .master_playlist()
                .expect("master playlist is ready with completed media segments")
                .contains("mp4a.40.2")
        );
    }
}
