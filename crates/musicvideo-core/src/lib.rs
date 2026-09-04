//! `musicvideo-core` — 音楽ビデオ書き出しの純ロジック（egui / FFmpeg / wgpu 非依存）。
//!
//! 正本: `docs/music-video-export-plan.md`。P1a のブリーフ: `docs/brief-musicvideo-p1a.md`。
//!
//! テキストは `comic-core`、スペクトラム解析は `music-core` を呼ぶ。ここで別のフォント
//! スタックや別の解析器を立てない。

pub mod anim;
pub mod assets;
pub mod model;
pub mod raster;
pub mod scene;
pub mod spectrum;
mod text;

pub use anim::{Interpolate, easing_amount, evaluate_track};
pub use assets::{AssetError, Assets, ImageAsset};
pub use model::{
    Animatable, Fill, Fit, ImageId, Interp, Keyframe, Layer, LayerId, LayerKind, NormRect, Output,
    ProgressBarLayer, Project, Rgba8, SpectrumLayer, TextAlign, TimeFormat, TimeTextLayer, Track,
    TrackError,
};
pub use raster::{Frame, PixelRect, RasterError, norm_rect_to_pixels, render_frame};
pub use scene::{Scene, SceneItem, SceneItemKind, SpectrumFrame, evaluate, format_time};
pub use spectrum::{
    MAX_SEQUENTIAL_GAP_SECS, MAX_SPECTRUM_BANDS, Smoothing, SpectrumSource, SpectrumSourceError,
};
