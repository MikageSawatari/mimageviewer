use std::fmt;

use music_core::{
    KeyboardAnimationState, SpectrumAnalysis, SpectrumAnalyzer, SpectrumRange,
    SpectrumRangeRequest, SpectrumWeighting, apply_display_gain_db, prepare_keyboard_frame_into,
};
use serde::{Deserialize, Serialize};

use crate::model::{BandRange, BandWeighting, Modulator, ModulatorSource, ModulatorValue};
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
    InvalidModulator,
}

impl fmt::Display for SpectrumSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("sample rate must be greater than zero"),
            Self::InvalidBandCount => f.write_str("band count must be in 1..=128"),
            Self::InvalidSmoothingFactor => {
                f.write_str("smoothing factor must be finite and in 0.0..=1.0")
            }
            Self::InvalidModulator => f.write_str("band-energy modulator settings are invalid"),
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
    full_frame_gain_bits: Option<u32>,
    modulators: Vec<ConfiguredModulator>,
    range_requests: Vec<SpectrumRangeRequest>,
    modulator_values: Vec<ModulatorValue>,
    last_t: Option<f64>,
}

#[derive(Clone, PartialEq)]
struct ConfiguredModulator {
    id: crate::model::ModulatorId,
    source: ModulatorSource,
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
            full_frame_gain_bits: None,
            modulators: Vec::new(),
            range_requests: Vec::new(),
            modulator_values: Vec::new(),
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
        let gain_bits = norm_gain_db.to_bits();
        if self.full_frame_gain_bits != Some(gain_bits) {
            self.keyboard.clear();
            self.full_frame_valid = false;
        }
        if !self.full_frame_valid {
            self.full_frame.bands.clone_from(&self.analysis.bands);
            self.full_frame.notes.clone_from(&self.analysis.notes);
            self.full_frame.note_min_midi = self.analysis.note_min_midi;
            self.full_frame
                .modulators
                .clone_from(&self.modulator_values);
            prepare_keyboard_frame_into(
                &self.analysis.notes,
                self.analysis.note_min_midi,
                norm_gain_db,
                &mut self.keyboard,
                &mut self.full_frame.keyboard,
            );
            self.full_frame_valid = true;
            self.full_frame_gain_bits = Some(gain_bits);
        }
        &self.full_frame
    }

    /// Replaces the project-level modulator definitions. An actual change is a
    /// new deterministic analysis sequence, so all forward-only state resets.
    pub fn configure_modulators(
        &mut self,
        modulators: &[Modulator],
    ) -> Result<(), SpectrumSourceError> {
        if !modulators_are_valid(modulators) {
            return Err(SpectrumSourceError::InvalidModulator);
        }
        let configured = modulators
            .iter()
            .map(|modulator| ConfiguredModulator {
                id: modulator.id,
                source: modulator.source.clone(),
            })
            .collect::<Vec<_>>();
        if self.modulators != configured {
            self.modulators = configured;
            self.range_requests = self
                .modulators
                .iter()
                .map(modulator_range_request)
                .collect();
            self.reset();
        }
        Ok(())
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

        let analyzed =
            self.analyzer
                .analyze_with_ranges(pcm, self.sample_rate, t, &self.range_requests);
        let range_values = analyzed.range_values;
        let analyzed = analyzed.spectrum;
        let first_frame = self.last_t.is_none();
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
        update_modulator_values(
            &self.modulators,
            &range_values,
            first_frame,
            &mut self.modulator_values,
        );
        self.last_t = Some(t);
        self.full_frame_valid = false;
    }

    pub fn reset(&mut self) {
        self.analyzer = SpectrumAnalyzer::new(self.band_count);
        self.analysis = SpectrumAnalysis::default();
        self.keyboard.clear();
        self.full_frame_valid = false;
        self.full_frame_gain_bits = None;
        self.modulator_values.clear();
        self.last_t = None;
    }

    pub fn band_count(&self) -> usize {
        self.band_count
    }
}

fn modulators_are_valid(modulators: &[Modulator]) -> bool {
    let mut ids = std::collections::HashSet::with_capacity(modulators.len());
    modulators.iter().all(|modulator| {
        if !ids.insert(modulator.id) {
            return false;
        }
        match &modulator.source {
            ModulatorSource::BandEnergy {
                range,
                smoothing,
                gain_db,
                floor,
                ceiling,
                ..
            } => {
                valid_range(*range)
                    && valid_smoothing(*smoothing)
                    && gain_db.is_finite()
                    && floor.is_finite()
                    && ceiling.is_finite()
                    && (0.0..1.0).contains(floor)
                    && (0.0..=1.0).contains(ceiling)
                    && ceiling > floor
            }
        }
    })
}

fn valid_range(range: BandRange) -> bool {
    match range {
        BandRange::Hz { low_hz, high_hz } => {
            low_hz.is_finite() && high_hz.is_finite() && low_hz > 0.0 && high_hz > low_hz
        }
        BandRange::Semitone {
            low_midi,
            high_midi,
        } => low_midi <= high_midi,
    }
}

fn valid_smoothing(smoothing: Smoothing) -> bool {
    match smoothing {
        Smoothing::None => true,
        Smoothing::Average { factor } => factor.is_finite() && (0.0..=1.0).contains(&factor),
    }
}

fn modulator_range_request(modulator: &ConfiguredModulator) -> SpectrumRangeRequest {
    match modulator.source {
        ModulatorSource::BandEnergy {
            range, weighting, ..
        } => SpectrumRangeRequest {
            range: match range {
                BandRange::Hz { low_hz, high_hz } => SpectrumRange::Hz { low_hz, high_hz },
                BandRange::Semitone {
                    low_midi,
                    high_midi,
                } => SpectrumRange::Midi {
                    low_midi,
                    high_midi,
                },
            },
            weighting: match weighting {
                BandWeighting::Raw => SpectrumWeighting::None,
                BandWeighting::MivDisplay => SpectrumWeighting::Display,
            },
        },
    }
}

fn update_modulator_values(
    modulators: &[ConfiguredModulator],
    range_values: &[f32],
    first_frame: bool,
    output: &mut Vec<ModulatorValue>,
) {
    if output.len() != modulators.len() {
        output.clear();
        output.extend(modulators.iter().map(|modulator| ModulatorValue {
            id: modulator.id,
            value: 0.0,
        }));
    }
    for ((modulator, raw), state) in modulators
        .iter()
        .zip(range_values.iter().copied())
        .zip(output.iter_mut())
    {
        let ModulatorSource::BandEnergy {
            smoothing,
            gain_db,
            floor,
            ceiling,
            ..
        } = modulator.source;
        let gained = apply_display_gain_db(raw, gain_db);
        let current = ((gained - floor) / (ceiling - floor)).clamp(0.0, 1.0);
        state.id = modulator.id;
        state.value = match smoothing {
            Smoothing::None => current,
            Smoothing::Average { factor } if !first_frame => {
                factor * state.value + (1.0 - factor) * current
            }
            Smoothing::Average { .. } => current,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BandRange, BandWeighting, Modulator, ModulatorId, ModulatorSource};

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

    #[test]
    fn changing_display_gain_at_the_same_time_rebuilds_the_keyboard_frame() {
        let pcm = tone();
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        let without_gain = source.full_frame_at(&pcm, 0.5, 0.0).keyboard.clone();
        let with_gain = source.full_frame_at(&pcm, 0.5, 12.0).keyboard.clone();
        assert_ne!(without_gain, with_gain);

        source.reset();
        let fresh_with_gain = source.full_frame_at(&pcm, 0.5, 12.0).keyboard.clone();
        assert_eq!(with_gain, fresh_with_gain);
    }

    fn a4_modulator(smoothing: Smoothing) -> Modulator {
        Modulator {
            id: ModulatorId(7),
            name: "A4".to_owned(),
            source: ModulatorSource::BandEnergy {
                range: BandRange::Semitone {
                    low_midi: 69,
                    high_midi: 69,
                },
                smoothing,
                weighting: BandWeighting::Raw,
                gain_db: 0.0,
                floor: 0.0,
                ceiling: 1.0,
            },
        }
    }

    #[test]
    fn modulator_sequence_is_bit_exact_after_reset() {
        let pcm = tone();
        let times = [0.0, 1.0 / 60.0, 2.0 / 60.0, 3.0 / 60.0];
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        source
            .configure_modulators(&[a4_modulator(Smoothing::Average { factor: 0.7 })])
            .unwrap();
        let first = times
            .iter()
            .map(|time| source.full_frame_at(&pcm, *time, 0.0).modulators.clone())
            .collect::<Vec<_>>();
        source.reset();
        let second = times
            .iter()
            .map(|time| source.full_frame_at(&pcm, *time, 0.0).modulators.clone())
            .collect::<Vec<_>>();
        assert_eq!(first, second);
        assert!(first[0][0].value > 0.5);
    }

    #[test]
    fn backward_seek_resets_modulator_smoothing_to_the_fresh_value() {
        let pcm = tone();
        let modulator = a4_modulator(Smoothing::Average { factor: 0.7 });
        let mut reused = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        reused
            .configure_modulators(std::slice::from_ref(&modulator))
            .unwrap();
        reused.full_frame_at(&pcm, 0.2, 0.0);
        let after_seek = reused.full_frame_at(&pcm, 0.1, 0.0).modulators.clone();

        let mut fresh = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        fresh.configure_modulators(&[modulator]).unwrap();
        let expected = fresh.full_frame_at(&pcm, 0.1, 0.0).modulators.clone();
        assert_eq!(after_seek, expected);
    }

    #[test]
    fn invalid_or_duplicate_modulators_are_rejected() {
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        let invalid = Modulator {
            source: ModulatorSource::BandEnergy {
                range: BandRange::Semitone {
                    low_midi: 69,
                    high_midi: 69,
                },
                smoothing: Smoothing::None,
                weighting: BandWeighting::Raw,
                gain_db: 0.0,
                floor: 0.8,
                ceiling: 0.2,
            },
            ..a4_modulator(Smoothing::None)
        };
        assert_eq!(
            source.configure_modulators(&[invalid]),
            Err(SpectrumSourceError::InvalidModulator)
        );
        let duplicate = a4_modulator(Smoothing::None);
        assert_eq!(
            source.configure_modulators(&[duplicate.clone(), duplicate]),
            Err(SpectrumSourceError::InvalidModulator)
        );
    }
}
