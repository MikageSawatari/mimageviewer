//! Core data structures for the music-viewer lab.
//!
//! This crate deliberately has no GUI, audio-device, FFmpeg, or VST3 dependency.
//! The main application can later connect its existing decoder and `DspBridge`
//! to these small contracts without pulling lab-only code into the hot path.

pub mod analysis;
pub mod beat;
pub mod color;
pub mod effects;
pub mod keyboard;
pub mod playback;
pub mod timeline;

pub use analysis::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, SPECTRUM_BAND_COUNT, SPECTRUM_BAND_MAX_MIDI,
    SPECTRUM_BAND_MIN_MIDI, SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis,
    SpectrumAnalysisWithRanges, SpectrumAnalyzer, SpectrumRange, SpectrumRangeRequest,
    SpectrumWeighting, TIMELINE_ANALYSIS_VERSION, TimelineAnalysis, WaveformBin,
    analyze_stereo_timeline, analyze_stereo_timeline_cancellable,
    analyze_stereo_waveform_cancellable, apply_display_gain_db, resample_linear_stereo,
    spectrum_analysis_from_stereo_window, spectrum_bands_from_stereo_window,
};
pub use beat::{BarMarker, BeatGrid, BeatMarker, BeatTrackingStatus};
pub use color::{
    VisualizationColor, brighten_color, hsv_to_rgb, key_color, key_fill, lerp_color,
    spectrum_color, unlit_key_base, with_alpha,
};
pub use effects::{AudioProcessBlock, EffectChain, EffectChainStats, EffectError, NoopEffectChain};
pub use keyboard::{
    BassMarkerPlacement, KEYBOARD_NOTE_COUNT, KeyPlacement, KeyboardAnimationState, KeyboardFrame,
    KeyboardLayout, KeyboardLayoutCache, KeyboardLevels, KeyboardPoint, KeyboardRect,
    layout_bass_markers, layout_bass_markers_into, layout_keyboard, layout_keyboard_cached,
    prepare_keyboard_frame, prepare_keyboard_frame_into,
};
pub use playback::{PlaybackIntent, PlaybackSnapshot};
pub use timeline::{MediaVisualMode, MusicBookmark, MusicModeSource, MusicTimelineLayout};
