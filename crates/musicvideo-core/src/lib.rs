//! `musicvideo-core` — 音楽ビデオ書き出しの純ロジック（egui / FFmpeg / wgpu 非依存）。
//!
//! 正本: `docs/music-video-export-plan.md`。各 increment のブリーフは `docs/brief-musicvideo-*.md`。
//!
//! テキストは `comic-core`、スペクトラム解析は `music-core` を呼ぶ。ここで別のフォント
//! スタックや別の解析器を立てない。

pub mod anim;
pub mod assets;
pub mod lyrics;
pub mod model;
pub mod preset;
pub mod raster;
pub mod scene;
pub mod spectrum;
mod text;

#[cfg(test)]
mod selection_tests;

pub use anim::{Interpolate, easing_amount, evaluate_track};
pub use assets::{
    AssetError, Assets, ImageAsset, MAX_SCALED_IMAGE_CACHE_ENTRIES, ScaledImageCacheStats,
};
pub use lyrics::{
    ActiveLyric, LrcEncoding, LrcParseError, LrcParseReport, LrcWriteReport, active_lyric_at,
    parse_lrc, write_lrc,
};
pub use model::{
    Animatable, AudioRef, BandCalibration, BandRange, BandWeighting, Fill, Fit, Glow, GradientStop,
    GradientStops, GradientStopsError, ImageId, ImageRef, Interp, Keyframe, Layer, LayerId,
    LayerKind, LyricLine, LyricsLayer, ModulationBinding, ModulationCurve, ModulationTarget,
    Modulator, ModulatorId, ModulatorSource, ModulatorValue, MusicColorMode, NormRect, Output,
    PitchKeyboardLayer, ProgressBarLayer, Project, Rgba8, SpectrumDirection, SpectrumLayer,
    SpectrumShape, StrokeStyle, TextAlign, TextBackgroundStyle, TextColor, TextEchoStyle,
    TextLayer, TextShadowStyle, TextStyle, TimeFormat, TimeTextLayer, Track, TrackError,
};
pub use preset::{
    BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE, BAND_ENERGY_CALIBRATION_LOW_PERCENTILE,
    DEFAULT_BACKGROUND_IMAGE_ID, DEFAULT_FONT_KEY, DEFAULT_OUTPUT, DEFAULT_SPECTRUM_BANDS,
    KICK_ATTACK_SECS, KICK_CALIBRATION_HIGH_PERCENTILE, KICK_CALIBRATION_LOW_PERCENTILE,
    KICK_RELEASE_SECS, KICK_THRESHOLD, MIV_SPECTRUM_BANDS, PROJECT_VERSION, default_project,
    new_band_energy_modulator, new_image_layer, new_linear_spectrum_layer, new_lyrics_layer,
    new_miv_spectrum_keyboard_layers, new_miv_spectrum_layer, new_pitch_keyboard_layer,
    new_progress_bar_layer, new_radial_spectrum_layer, new_solid_layer, new_text_layer,
    new_time_text_layer,
};
pub use raster::{
    Frame, PixelRect, RasterError, RenderReport, UnresolvedAsset, norm_rect_to_pixels, render_frame,
};
pub use scene::{
    LayerBounds, Scene, SceneItem, SceneItemKind, SpectrumFrame, evaluate, evaluate_layer_bounds,
    format_time,
};
pub use spectrum::{
    BandFeature, MAX_SEQUENTIAL_GAP_SECS, MAX_SPECTRUM_BANDS, MODULATOR_CALIBRATION_HZ,
    ModulatorCalibration, ModulatorCalibrationError, ModulatorCalibrationRequest, Smoothing,
    SpectrumSource, SpectrumSourceError, calibrate_modulators, modulator_calibration_requests,
};

pub use text::{MAX_TEXT_CACHE_BYTES, MAX_TEXT_RASTER_BYTES, TextCacheStats};
