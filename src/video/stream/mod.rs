//! mIV Remote 向けの動画・音声ストリーミング基盤。
//!
//! 増分 1 では、再生経路から独立した出力パラメータ計算と H.264 encoder の open だけを
//! 所有する。tap、セグメンタ、セッション、IPC は後続増分でこのモジュールへ追加する。

#![allow(dead_code)] // 後続増分で session/tap から接続する API を先に確定する。

pub(crate) mod encoder;
pub(crate) mod quality;

#[allow(unused_imports)]
pub(crate) use encoder::{
    EncoderPreference, FrameRate, H264EncoderOpenError, OpenedH264Encoder, open_h264_encoder,
};
#[allow(unused_imports)]
pub(crate) use quality::{
    OutputDimensions, OutputDimensionsError, QualityPreset, StreamOutputParameters,
    calculate_output_dimensions,
};
