//! mIV Remote 向けの動画・音声ストリーミング基盤。
//!
//! 増分 1〜4 では encoder、メモリ fMP4 セグメンタ、音声 tap、映像 tap と worker 側の
//! readback / scale / H.264 input を所有する。セッションと IPC は後続増分で追加する。

#![allow(dead_code)] // 後続増分で session/tap から接続する API を先に確定する。

pub(crate) mod audio_encoder;
pub(crate) mod encoder;
pub(crate) mod playlist;
pub(crate) mod quality;
pub(crate) mod segmenter;
pub(crate) mod timeline;
pub(crate) mod video_tap;

#[allow(unused_imports)]
pub(crate) use audio_encoder::{
    AacEncoderError, AacEncoderStats, OpenedAacEncoder, open_aac_encoder,
};

#[allow(unused_imports)]
pub(crate) use encoder::{
    EncoderPreference, FrameRate, H264EncoderOpenError, H264InputFormat, OpenedH264Encoder,
    open_h264_encoder,
};
#[allow(unused_imports)]
pub(crate) use quality::{
    OutputDimensions, OutputDimensionsError, QualityPreset, StreamOutputParameters,
    calculate_output_dimensions,
};
#[allow(unused_imports)]
pub(crate) use segmenter::{
    CfrTimelineFrameIndex, Fmp4Segmenter, Fmp4SegmenterError, Fmp4SegmenterStats,
};
#[allow(unused_imports)]
pub(crate) use timeline::{StreamTimeline, StreamTimelineError};
#[allow(unused_imports)]
pub(crate) use video_tap::{
    TappedVideoFrame, VIDEO_TAP_MAX_QUEUED_DECODER_HW_SURFACES,
    VIDEO_TAP_MAX_SYNCHRONOUS_DECODER_HW_SURFACES, VideoStreamEncoder, VideoStreamEncoderError,
    VideoStreamEncoderOpenError, VideoStreamEncoderStats, VideoTapController, VideoTapLease,
    cfr_timeline_frame_index, open_video_stream_encoder,
};
