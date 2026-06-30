//! Core data structures for the music-viewer lab.
//!
//! This crate deliberately has no GUI, audio-device, FFmpeg, or VST3 dependency.
//! The main application can later connect its existing decoder and `DspBridge`
//! to these small contracts without pulling lab-only code into the hot path.

pub mod analysis;
pub mod beat;
pub mod effects;
pub mod playback;
pub mod timeline;

pub use analysis::{
    AnalysisConfig, AudioStreamInfo, DecodedAudio, SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI,
    SpectrumAnalysis, SpectrumAnalyzer, TimelineAnalysis, WaveformBin, analyze_stereo_timeline,
    resample_linear_stereo, spectrum_analysis_from_stereo_window,
    spectrum_bands_from_stereo_window,
};
pub use beat::{BarMarker, BeatGrid, BeatMarker, BeatTrackingStatus};
pub use effects::{AudioProcessBlock, EffectChain, EffectChainStats, EffectError, NoopEffectChain};
pub use playback::{PlaybackIntent, PlaybackSnapshot};
pub use timeline::{MediaVisualMode, MusicBookmark, MusicModeSource, MusicTimelineLayout};
