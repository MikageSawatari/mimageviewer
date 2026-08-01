//! mIV Remote 向けの動画・音声ストリーミング基盤。
//!
//! 増分 1〜2 では、再生経路から独立した encoder open とメモリ fMP4 セグメンタを
//! 所有する。tap、セッション、IPC は後続増分でこのモジュールへ追加する。

#![allow(dead_code)] // 後続増分で session/tap から接続する API を先に確定する。

pub(crate) mod audio_encoder;
pub(crate) mod encoder;
pub(crate) mod playlist;
pub(crate) mod quality;
pub(crate) mod segmenter;
pub(crate) mod timeline;

#[allow(unused_imports)]
pub(crate) use audio_encoder::{
    AacEncoderError, AacEncoderStats, OpenedAacEncoder, open_aac_encoder,
};

#[allow(unused_imports)]
pub(crate) use encoder::{
    EncoderPreference, FrameRate, H264EncoderOpenError, OpenedH264Encoder, open_h264_encoder,
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
