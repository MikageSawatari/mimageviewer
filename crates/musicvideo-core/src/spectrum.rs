use std::fmt;

use music_core::{
    KeyboardAnimationState, SpectrumAnalysis, SpectrumAnalyzer, prepare_keyboard_frame_into,
};
use serde::{Deserialize, Serialize};

use crate::scene::SpectrumFrame;

pub const MAX_SPECTRUM_BANDS: usize = 128;
/// A larger discontinuity is a seek rather than sequential frame production.
pub const MAX_SEQUENTIAL_GAP_SECS: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Smoothing {
    None,
    Average { factor: f32 },
}

impl Default for Smoothing {
    fn default() -> Self {
        Self::Average { factor: 0.7 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumSourceError {
    InvalidSampleRate,
    InvalidBandCount,
    InvalidSmoothingFactor,
}

impl fmt::Display for SpectrumSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("sample rate must be greater than zero"),
            Self::InvalidBandCount => f.write_str("band count must be in 1..=128"),
            Self::InvalidSmoothingFactor => {
                f.write_str("smoothing factor must be finite and in 0.0..=1.0")
            }
        }
    }
}

impl std::error::Error for SpectrumSourceError {}

/// Stateful, forward-only adapter around `music_core::SpectrumAnalyzer`.
pub struct SpectrumSource {
    sample_rate: u32,
    band_count: usize,
    smoothing: Smoothing,
    analyzer: SpectrumAnalyzer,
    analysis: SpectrumAnalysis,
    keyboard: KeyboardAnimationState,
    full_frame: SpectrumFrame,
    full_frame_valid: bool,
    last_t: Option<f64>,
}

impl SpectrumSource {
    pub fn new(
        sample_rate: u32,
        band_count: usize,
        smoothing: Smoothing,
    ) -> Result<Self, SpectrumSourceError> {
        if sample_rate == 0 {
            return Err(SpectrumSourceError::InvalidSampleRate);
        }
        if !(1..=MAX_SPECTRUM_BANDS).contains(&band_count) {
            return Err(SpectrumSourceError::InvalidBandCount);
        }
        if let Smoothing::Average { factor } = smoothing
            && (!factor.is_finite() || !(0.0..=1.0).contains(&factor))
        {
            return Err(SpectrumSourceError::InvalidSmoothingFactor);
        }
        Ok(Self {
            sample_rate,
            band_count,
            smoothing,
            analyzer: SpectrumAnalyzer::new(band_count),
            analysis: SpectrumAnalysis::default(),
            keyboard: KeyboardAnimationState::default(),
            full_frame: SpectrumFrame::default(),
            full_frame_valid: false,
            last_t: None,
        })
    }

    /// Produces a frame for monotonic, closely spaced `t_secs` values.
    /// Backward seeks, non-finite times, and gaps over 250 ms reset both the
    /// analyzer cache and temporal smoothing before the requested frame.
    pub fn frame_at(&mut self, pcm: &[f32], t_secs: f64) -> &[f32] {
        self.update_analysis(pcm, t_secs);
        &self.analysis.bands
    }

    /// Produces bands, notes, and the keyboard animation frame from one FFT analysis.
    /// Repeating the same time returns the cached result without advancing keyboard decay.
    pub fn full_frame_at(&mut self, pcm: &[f32], t_secs: f64, norm_gain_db: f32) -> &SpectrumFrame {
        self.update_analysis(pcm, t_secs);
        if !self.full_frame_valid {
            self.full_frame.bands.clone_from(&self.analysis.bands);
            self.full_frame.notes.clone_from(&self.analysis.notes);
            self.full_frame.note_min_midi = self.analysis.note_min_midi;
            prepare_keyboard_frame_into(
                &self.analysis.notes,
                self.analysis.note_min_midi,
                norm_gain_db,
                &mut self.keyboard,
                &mut self.full_frame.keyboard,
            );
            self.full_frame_valid = true;
        }
        &self.full_frame
    }

    fn update_analysis(&mut self, pcm: &[f32], t_secs: f64) {
        let t = if t_secs.is_finite() {
            t_secs.max(0.0)
        } else {
            self.reset();
            0.0
        };

        if self.last_t == Some(t) {
            return;
        }
        if self
            .last_t
            .is_some_and(|last| t < last || t - last > MAX_SEQUENTIAL_GAP_SECS)
        {
            self.reset();
        }

        let analyzed = self.analyzer.analyze(pcm, self.sample_rate, t);
        if self.analysis.bands.len() != self.band_count || self.last_t.is_none() {
            self.analysis = analyzed;
        } else {
            self.analysis.notes = analyzed.notes;
            self.analysis.note_min_midi = analyzed.note_min_midi;
            match self.smoothing {
                Smoothing::None => self.analysis.bands = analyzed.bands,
                Smoothing::Average { factor } => {
                    for (previous, current) in self.analysis.bands.iter_mut().zip(analyzed.bands) {
                        *previous = factor * *previous + (1.0 - factor) * current;
                    }
                }
            }
        }
        self.last_t = Some(t);
        self.full_frame_valid = false;
    }

    pub fn reset(&mut self) {
        self.analyzer = SpectrumAnalyzer::new(self.band_count);
        self.analysis = SpectrumAnalysis::default();
        self.keyboard.clear();
        self.full_frame_valid = false;
        self.last_t = None;
    }

    pub fn band_count(&self) -> usize {
        self.band_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone() -> Vec<f32> {
        (0..96_000)
            .flat_map(|frame| {
                let t = frame as f32 / 48_000.0;
                let value = (std::f32::consts::TAU * 440.0 * t).sin() * 0.5;
                [value, value]
            })
            .collect()
    }

    #[test]
    fn identical_time_sequence_is_bit_exact_after_reset() {
        let pcm = tone();
        let times = [0.0, 1.0 / 60.0, 2.0 / 60.0, 3.0 / 60.0, 4.0 / 60.0];
        let mut source =
            SpectrumSource::new(48_000, 64, Smoothing::Average { factor: 0.7 }).unwrap();
        let first: Vec<Vec<f32>> = times
            .iter()
            .map(|time| source.frame_at(&pcm, *time).to_vec())
            .collect();
        source.reset();
        let second: Vec<Vec<f32>> = times
            .iter()
            .map(|time| source.frame_at(&pcm, *time).to_vec())
            .collect();
        assert_eq!(first, second);
    }

    #[test]
    fn backward_seek_discards_analyzer_and_smoothing_state() {
        let pcm = tone();
        let mut reused =
            SpectrumSource::new(48_000, 64, Smoothing::Average { factor: 0.7 }).unwrap();
        reused.frame_at(&pcm, 0.20);
        reused.frame_at(&pcm, 0.21);
        let after_seek = reused.frame_at(&pcm, 0.10).to_vec();

        let mut fresh =
            SpectrumSource::new(48_000, 64, Smoothing::Average { factor: 0.7 }).unwrap();
        let expected = fresh.frame_at(&pcm, 0.10).to_vec();
        assert_eq!(after_seek, expected);
    }

    #[test]
    fn rejects_band_counts_above_music_core_limit() {
        assert!(matches!(
            SpectrumSource::new(48_000, 129, Smoothing::default()),
            Err(SpectrumSourceError::InvalidBandCount)
        ));
    }

    #[test]
    fn full_frame_contains_bands_and_notes_from_the_same_analysis() {
        let pcm = tone();
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        let first = source.full_frame_at(&pcm, 0.5, 0.0).clone();
        assert_eq!(first.bands.len(), 64);
        assert_eq!(first.notes.len(), music_core::KEYBOARD_NOTE_COUNT);
        assert_eq!(first.note_min_midi, music_core::SPECTRUM_NOTE_MIN_MIDI);
        assert_eq!(first.keyboard.note_min_midi, first.note_min_midi);
        assert!(first.keyboard.levels.iter().any(|level| level.center > 0.0));

        let repeated = source.full_frame_at(&pcm, 0.5, 0.0).clone();
        assert_eq!(first, repeated);
        assert_eq!(source.frame_at(&pcm, 0.5), first.bands);
    }
}
