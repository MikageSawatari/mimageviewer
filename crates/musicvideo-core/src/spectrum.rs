use std::fmt;

use music_core::{
    KeyboardAnimationState, MivSpectrumState, SpectrumAnalysis, SpectrumAnalyzer, SpectrumRange,
    SpectrumRangeRequest, SpectrumWeighting, apply_display_gain_db, prepare_keyboard_frame_into,
};
use serde::{Deserialize, Serialize};

use crate::model::{
    BandCalibration, BandRange, BandWeighting, Modulator, ModulatorId, ModulatorSource,
    ModulatorValue,
};
use crate::scene::SpectrumFrame;

pub const MAX_SPECTRUM_BANDS: usize = 128;
/// A larger discontinuity is a seek rather than sequential frame production.
pub const MAX_SEQUENTIAL_GAP_SECS: f64 = 0.25;
/// Automatic calibration samples the complete decoded track at a stable rate,
/// independent of preview and export frame rates.
pub const MODULATOR_CALIBRATION_HZ: f64 = 60.0;
const ONSET_REFERENCE_SECS: f32 = 1.0 / MODULATOR_CALIBRATION_HZ as f32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Smoothing {
    None,
    /// Legacy symmetric EMA. The factor is retained for project compatibility.
    Average {
        factor: f32,
    },
    /// Frame-rate independent asymmetric envelope follower.
    Envelope {
        attack_secs: f32,
        release_secs: f32,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandFeature {
    Energy,
    Onset,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulatorCalibrationRequest {
    pub id: ModulatorId,
    pub feature: BandFeature,
    pub range: BandRange,
    pub weighting: BandWeighting,
    pub gain_db: f32,
    pub low_percentile: f32,
    pub high_percentile: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulatorCalibration {
    pub request: ModulatorCalibrationRequest,
    pub floor: f32,
    pub ceiling: f32,
    pub sample_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulatorCalibrationError {
    InvalidSampleRate,
    InvalidRequest,
    Cancelled,
}

impl fmt::Display for ModulatorCalibrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => f.write_str("sample rate must be greater than zero"),
            Self::InvalidRequest => f.write_str("automatic calibration settings are invalid"),
            Self::Cancelled => f.write_str("automatic calibration was cancelled"),
        }
    }
}

impl std::error::Error for ModulatorCalibrationError {}

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
    miv_spectrum: MivSpectrumState,
    full_frame: SpectrumFrame,
    full_frame_valid: bool,
    full_frame_gain_bits: Option<u32>,
    modulators: Vec<ConfiguredModulator>,
    range_requests: Vec<SpectrumRangeRequest>,
    modulator_values: Vec<ModulatorValue>,
    previous_modulator_inputs: Vec<Option<f32>>,
    last_t: Option<f64>,
}

#[derive(Clone, PartialEq)]
struct ConfiguredModulator {
    id: crate::model::ModulatorId,
    source: ModulatorSource,
    floor: f32,
    ceiling: f32,
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
        if !valid_smoothing(smoothing) {
            return Err(SpectrumSourceError::InvalidSmoothingFactor);
        }
        Ok(Self {
            sample_rate,
            band_count,
            smoothing,
            analyzer: SpectrumAnalyzer::new(band_count),
            analysis: SpectrumAnalysis::default(),
            keyboard: KeyboardAnimationState::default(),
            miv_spectrum: MivSpectrumState::default(),
            full_frame: SpectrumFrame::default(),
            full_frame_valid: false,
            full_frame_gain_bits: None,
            modulators: Vec::new(),
            range_requests: Vec::new(),
            modulator_values: Vec::new(),
            previous_modulator_inputs: Vec::new(),
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
            self.full_frame.miv_spectrum.clone_from(
                self.miv_spectrum
                    .frame_at(&self.analysis.bands, self.last_t.unwrap_or(0.0)),
            );
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
        self.configure_modulators_with_calibrations(modulators, &[])
    }

    /// Replaces the project-level definitions and applies complete-track
    /// calibration results whose request keys still match the definitions.
    pub fn configure_modulators_with_calibrations(
        &mut self,
        modulators: &[Modulator],
        calibrations: &[ModulatorCalibration],
    ) -> Result<(), SpectrumSourceError> {
        if !modulators_are_valid(modulators) {
            return Err(SpectrumSourceError::InvalidModulator);
        }
        let configured = modulators
            .iter()
            .map(|modulator| {
                let (manual_floor, manual_ceiling) = source_manual_range(&modulator.source);
                let request = modulator_calibration_request(modulator);
                let calibrated = request.and_then(|request| {
                    calibrations
                        .iter()
                        .find(|calibration| calibration.request == request)
                });
                ConfiguredModulator {
                    id: modulator.id,
                    source: modulator.source.clone(),
                    floor: calibrated.map_or(manual_floor, |value| value.floor),
                    ceiling: calibrated.map_or(manual_ceiling, |value| value.ceiling),
                }
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
        let delta_secs = self.last_t.map_or(0.0, |last| (t - last).max(0.0) as f32);
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
                Smoothing::Envelope {
                    attack_secs,
                    release_secs,
                } => {
                    for (previous, current) in self.analysis.bands.iter_mut().zip(analyzed.bands) {
                        *previous = follow_envelope(
                            *previous,
                            current,
                            delta_secs,
                            attack_secs,
                            release_secs,
                        );
                    }
                }
            }
        }
        update_modulator_values(
            &self.modulators,
            &range_values,
            first_frame,
            delta_secs,
            &mut self.modulator_values,
            &mut self.previous_modulator_inputs,
        );
        self.last_t = Some(t);
        self.full_frame_valid = false;
    }

    pub fn reset(&mut self) {
        self.analyzer = SpectrumAnalyzer::new(self.band_count);
        self.analysis = SpectrumAnalysis::default();
        self.keyboard.clear();
        self.miv_spectrum.clear();
        self.full_frame_valid = false;
        self.full_frame_gain_bits = None;
        self.modulator_values.clear();
        self.previous_modulator_inputs.clear();
        self.last_t = None;
    }

    pub fn band_count(&self) -> usize {
        self.band_count
    }
}

pub fn modulator_calibration_requests(
    modulators: &[Modulator],
) -> Vec<ModulatorCalibrationRequest> {
    modulators
        .iter()
        .filter_map(modulator_calibration_request)
        .collect()
}

pub fn calibrate_modulators(
    pcm: &[f32],
    sample_rate: u32,
    modulators: &[Modulator],
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<ModulatorCalibration>, ModulatorCalibrationError> {
    if sample_rate == 0 {
        return Err(ModulatorCalibrationError::InvalidSampleRate);
    }
    if !modulators_are_valid(modulators) {
        return Err(ModulatorCalibrationError::InvalidRequest);
    }
    let requests = modulator_calibration_requests(modulators);
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let range_requests = requests
        .iter()
        .map(|request| spectrum_range_request(request.range, request.weighting))
        .collect::<Vec<_>>();
    let frame_count = pcm.len() / 2;
    let duration_secs = frame_count as f64 / sample_rate as f64;
    let sample_count = (duration_secs * MODULATOR_CALIBRATION_HZ).ceil() as usize;
    let mut distributions = vec![Vec::with_capacity(sample_count); requests.len()];
    let mut previous = vec![None; requests.len()];
    let mut analyzer = SpectrumAnalyzer::new(1);

    for index in 0..sample_count {
        if should_cancel() {
            return Err(ModulatorCalibrationError::Cancelled);
        }
        let t_secs = index as f64 / MODULATOR_CALIBRATION_HZ;
        let values = analyzer
            .analyze_with_ranges(pcm, sample_rate, t_secs, &range_requests)
            .range_values;
        for (((request, raw), previous), distribution) in requests
            .iter()
            .zip(values)
            .zip(previous.iter_mut())
            .zip(distributions.iter_mut())
        {
            let gained = apply_display_gain_db(raw, request.gain_db);
            let value = feature_input(request.feature, gained, *previous, ONSET_REFERENCE_SECS);
            *previous = Some(gained);
            distribution.push(value);
        }
    }

    Ok(requests
        .into_iter()
        .zip(distributions)
        .map(|(request, mut values)| {
            values.sort_by(f32::total_cmp);
            let floor = percentile(&values, request.low_percentile);
            let ceiling = percentile(&values, request.high_percentile);
            let (floor, ceiling) = non_empty_range(floor, ceiling);
            ModulatorCalibration {
                request,
                floor,
                ceiling,
                sample_count: values.len(),
            }
        })
        .collect())
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
                calibration,
                threshold,
                ..
            }
            | ModulatorSource::BandOnset {
                range,
                smoothing,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
                ..
            } => {
                valid_range(*range)
                    && valid_smoothing(*smoothing)
                    && gain_db.is_finite()
                    && floor.is_finite()
                    && ceiling.is_finite()
                    && threshold.is_finite()
                    && (0.0..1.0).contains(floor)
                    && (0.0..=1.0).contains(ceiling)
                    && ceiling > floor
                    && (0.0..=1.0).contains(threshold)
                    && valid_calibration(*calibration)
            }
        }
    })
}

fn valid_calibration(calibration: BandCalibration) -> bool {
    match calibration {
        BandCalibration::Manual => true,
        BandCalibration::Auto {
            low_percentile,
            high_percentile,
        } => {
            low_percentile.is_finite()
                && high_percentile.is_finite()
                && (0.0..100.0).contains(&low_percentile)
                && (0.0..=100.0).contains(&high_percentile)
                && high_percentile > low_percentile
        }
    }
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
        Smoothing::Envelope {
            attack_secs,
            release_secs,
        } => {
            attack_secs.is_finite()
                && release_secs.is_finite()
                && attack_secs > 0.0
                && release_secs > 0.0
        }
    }
}

fn modulator_range_request(modulator: &ConfiguredModulator) -> SpectrumRangeRequest {
    let (range, weighting) = match modulator.source {
        ModulatorSource::BandEnergy {
            range, weighting, ..
        }
        | ModulatorSource::BandOnset {
            range, weighting, ..
        } => (range, weighting),
    };
    spectrum_range_request(range, weighting)
}

fn update_modulator_values(
    modulators: &[ConfiguredModulator],
    range_values: &[f32],
    first_frame: bool,
    delta_secs: f32,
    output: &mut Vec<ModulatorValue>,
    previous_inputs: &mut Vec<Option<f32>>,
) {
    if output.len() != modulators.len() {
        output.clear();
        output.extend(modulators.iter().map(|modulator| ModulatorValue {
            id: modulator.id,
            value: 0.0,
        }));
        previous_inputs.clear();
        previous_inputs.resize(modulators.len(), None);
    }
    for (((modulator, raw), state), previous_input) in modulators
        .iter()
        .zip(range_values.iter().copied())
        .zip(output.iter_mut())
        .zip(previous_inputs.iter_mut())
    {
        let (feature, smoothing, gain_db, threshold) = source_runtime_fields(&modulator.source);
        let gained = apply_display_gain_db(raw, gain_db);
        let input = feature_input(feature, gained, *previous_input, delta_secs);
        *previous_input = Some(gained);
        let normalized =
            ((input - modulator.floor) / (modulator.ceiling - modulator.floor)).clamp(0.0, 1.0);
        let current = if normalized < threshold {
            0.0
        } else {
            normalized
        };
        state.id = modulator.id;
        state.value = match smoothing {
            Smoothing::None => current,
            Smoothing::Average { factor } if !first_frame => {
                factor * state.value + (1.0 - factor) * current
            }
            Smoothing::Average { .. } => current,
            Smoothing::Envelope {
                attack_secs,
                release_secs,
            } if !first_frame => {
                follow_envelope(state.value, current, delta_secs, attack_secs, release_secs)
            }
            Smoothing::Envelope { .. } => current,
        };
    }
}

fn source_runtime_fields(source: &ModulatorSource) -> (BandFeature, Smoothing, f32, f32) {
    match *source {
        ModulatorSource::BandEnergy {
            smoothing,
            gain_db,
            threshold,
            ..
        } => (BandFeature::Energy, smoothing, gain_db, threshold),
        ModulatorSource::BandOnset {
            smoothing,
            gain_db,
            threshold,
            ..
        } => (BandFeature::Onset, smoothing, gain_db, threshold),
    }
}

fn source_manual_range(source: &ModulatorSource) -> (f32, f32) {
    match *source {
        ModulatorSource::BandEnergy { floor, ceiling, .. }
        | ModulatorSource::BandOnset { floor, ceiling, .. } => (floor, ceiling),
    }
}

fn modulator_calibration_request(modulator: &Modulator) -> Option<ModulatorCalibrationRequest> {
    let (feature, range, weighting, gain_db, calibration) = match modulator.source {
        ModulatorSource::BandEnergy {
            range,
            weighting,
            gain_db,
            calibration,
            ..
        } => (BandFeature::Energy, range, weighting, gain_db, calibration),
        ModulatorSource::BandOnset {
            range,
            weighting,
            gain_db,
            calibration,
            ..
        } => (BandFeature::Onset, range, weighting, gain_db, calibration),
    };
    let BandCalibration::Auto {
        low_percentile,
        high_percentile,
    } = calibration
    else {
        return None;
    };
    Some(ModulatorCalibrationRequest {
        id: modulator.id,
        feature,
        range,
        weighting,
        gain_db,
        low_percentile,
        high_percentile,
    })
}

fn spectrum_range_request(range: BandRange, weighting: BandWeighting) -> SpectrumRangeRequest {
    SpectrumRangeRequest {
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
    }
}

fn feature_input(
    feature: BandFeature,
    current: f32,
    previous: Option<f32>,
    delta_secs: f32,
) -> f32 {
    match feature {
        BandFeature::Energy => current,
        BandFeature::Onset => previous.map_or(0.0, |previous| {
            let delta_secs = delta_secs.max(f32::EPSILON);
            ((current - previous).max(0.0) * ONSET_REFERENCE_SECS / delta_secs).clamp(0.0, 1.0)
        }),
    }
}

fn percentile(sorted: &[f32], percentile: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = percentile.clamp(0.0, 100.0) / 100.0 * (sorted.len() - 1) as f32;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    let amount = position - low as f32;
    sorted[low] * (1.0 - amount) + sorted[high] * amount
}

fn non_empty_range(floor: f32, ceiling: f32) -> (f32, f32) {
    const MIN_SPAN: f32 = 1.0e-4;
    let mut floor = floor.clamp(0.0, 1.0);
    let mut ceiling = ceiling.clamp(0.0, 1.0);
    if ceiling - floor < MIN_SPAN {
        let center = (floor + ceiling) * 0.5;
        floor = (center - MIN_SPAN * 0.5).clamp(0.0, 1.0 - MIN_SPAN);
        ceiling = floor + MIN_SPAN;
    }
    (floor, ceiling)
}

fn follow_envelope(
    previous: f32,
    current: f32,
    delta_secs: f32,
    attack_secs: f32,
    release_secs: f32,
) -> f32 {
    let time_constant = if current > previous {
        attack_secs
    } else {
        release_secs
    };
    let retention = (-delta_secs.max(0.0) / time_constant).exp();
    retention * previous + (1.0 - retention) * current
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
        assert_eq!(first.miv_spectrum.bands.len(), 64);
        assert!(
            first
                .miv_spectrum
                .bands
                .iter()
                .zip(&first.bands)
                .all(|(visual, band)| visual.value.to_bits() == band.clamp(0.0, 1.0).to_bits())
        );
        assert_eq!(first.note_min_midi, music_core::SPECTRUM_NOTE_MIN_MIDI);
        assert_eq!(first.keyboard.note_min_midi, first.note_min_midi);
        assert!(first.keyboard.levels.iter().any(|level| level.center > 0.0));

        let repeated = source.full_frame_at(&pcm, 0.5, 0.0).clone();
        assert_eq!(first, repeated);
        assert_eq!(source.frame_at(&pcm, 0.5), first.bands);
    }

    #[test]
    fn miv_visual_sequence_is_bit_exact_after_reset() {
        let pcm = tone();
        let times = [0.0, 1.0 / 60.0, 2.0 / 60.0, 3.0 / 60.0];
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        let first = times
            .iter()
            .map(|time| source.full_frame_at(&pcm, *time, 0.0).miv_spectrum.clone())
            .collect::<Vec<_>>();
        source.reset();
        let second = times
            .iter()
            .map(|time| source.full_frame_at(&pcm, *time, 0.0).miv_spectrum.clone())
            .collect::<Vec<_>>();
        assert_eq!(first, second);
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
                calibration: BandCalibration::Manual,
                threshold: 0.0,
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
                calibration: BandCalibration::Manual,
                threshold: 0.0,
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

    #[test]
    fn envelope_time_constants_are_frame_rate_independent() {
        fn follow_for(
            fps: usize,
            duration_secs: f32,
            start: f32,
            target: f32,
            attack_secs: f32,
            release_secs: f32,
        ) -> f32 {
            let mut value = start;
            let steps = (duration_secs * fps as f32).round() as usize;
            for _ in 0..steps {
                value = follow_envelope(value, target, 1.0 / fps as f32, attack_secs, release_secs);
            }
            value
        }

        let rise_30 = follow_for(30, 0.2, 0.0, 1.0, 0.05, 0.2);
        let rise_60 = follow_for(60, 0.2, 0.0, 1.0, 0.05, 0.2);
        let rise_120 = follow_for(120, 0.2, 0.0, 1.0, 0.05, 0.2);
        let release_30 = follow_for(30, 0.2, 1.0, 0.0, 0.05, 0.2);
        let release_60 = follow_for(60, 0.2, 1.0, 0.0, 0.05, 0.2);
        let release_120 = follow_for(120, 0.2, 1.0, 0.0, 0.05, 0.2);

        assert!((rise_30 - rise_60).abs() < 1.0e-6);
        assert!((rise_60 - rise_120).abs() < 1.0e-6);
        assert!((release_30 - release_60).abs() < 1.0e-6);
        assert!((release_60 - release_120).abs() < 1.0e-6);
        assert!((rise_60 - (1.0 - (-4.0_f32).exp())).abs() < 1.0e-6);
        assert!((release_60 - (-1.0_f32).exp()).abs() < 1.0e-6);
    }

    #[test]
    fn threshold_gates_input_before_the_envelope() {
        let modulator = ConfiguredModulator {
            id: ModulatorId(1),
            source: ModulatorSource::BandEnergy {
                range: BandRange::Hz {
                    low_hz: 40.0,
                    high_hz: 120.0,
                },
                smoothing: Smoothing::Envelope {
                    attack_secs: 0.015,
                    release_secs: 0.18,
                },
                weighting: BandWeighting::Raw,
                gain_db: 0.0,
                floor: 0.0,
                ceiling: 1.0,
                calibration: BandCalibration::Manual,
                threshold: 0.1,
            },
            floor: 0.0,
            ceiling: 1.0,
        };
        let mut output = Vec::new();
        let mut previous = Vec::new();
        update_modulator_values(
            &[modulator.clone()],
            &[0.09],
            true,
            0.0,
            &mut output,
            &mut previous,
        );
        assert_eq!(output[0].value, 0.0);
        update_modulator_values(
            &[modulator],
            &[1.0],
            false,
            1.0 / 60.0,
            &mut output,
            &mut previous,
        );
        assert!(output[0].value > 0.6);
    }

    #[test]
    fn onset_responds_to_a_rise_and_not_to_a_sustained_level() {
        let modulator = ConfiguredModulator {
            id: ModulatorId(1),
            source: ModulatorSource::BandOnset {
                range: BandRange::Hz {
                    low_hz: 40.0,
                    high_hz: 120.0,
                },
                smoothing: Smoothing::None,
                weighting: BandWeighting::Raw,
                gain_db: 0.0,
                floor: 0.0,
                ceiling: 1.0,
                calibration: BandCalibration::Manual,
                threshold: 0.0,
            },
            floor: 0.0,
            ceiling: 1.0,
        };
        let mut output = Vec::new();
        let mut previous = Vec::new();
        update_modulator_values(
            &[modulator.clone()],
            &[0.2],
            true,
            0.0,
            &mut output,
            &mut previous,
        );
        assert_eq!(output[0].value, 0.0);
        update_modulator_values(
            &[modulator.clone()],
            &[0.8],
            false,
            1.0 / 60.0,
            &mut output,
            &mut previous,
        );
        assert!((output[0].value - 0.6).abs() < 1.0e-6);
        update_modulator_values(
            &[modulator],
            &[0.8],
            false,
            1.0 / 60.0,
            &mut output,
            &mut previous,
        );
        assert_eq!(output[0].value, 0.0);
    }

    #[test]
    fn onset_rate_is_frame_rate_independent_for_the_same_ramp() {
        let at_60 = feature_input(BandFeature::Onset, 0.3, Some(0.2), 1.0 / 60.0);
        let at_30 = feature_input(BandFeature::Onset, 0.4, Some(0.2), 1.0 / 30.0);
        assert!((at_60 - at_30).abs() < 1.0e-6);
    }

    #[test]
    fn complete_track_calibration_is_deterministic() {
        let sample_rate = 48_000;
        let pcm = (0..sample_rate * 2)
            .flat_map(|frame| {
                let t = frame as f32 / sample_rate as f32;
                let amplitude = if (t * 4.0).fract() < 0.12 { 0.8 } else { 0.2 };
                let value = (std::f32::consts::TAU * 80.0 * t).sin() * amplitude;
                [value, value]
            })
            .collect::<Vec<_>>();
        let modulator = crate::new_band_energy_modulator(&crate::default_project(2.0));
        let first =
            calibrate_modulators(&pcm, sample_rate, std::slice::from_ref(&modulator), &|| {
                false
            })
            .unwrap();
        let second = calibrate_modulators(&pcm, sample_rate, &[modulator], &|| false).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert!(first[0].ceiling > first[0].floor);
        assert_eq!(first[0].sample_count, 120);
    }

    #[test]
    fn only_an_exact_calibration_request_overrides_the_saved_manual_range() {
        let project = crate::default_project(2.0);
        let modulator = crate::new_band_energy_modulator(&project);
        let request = modulator_calibration_requests(std::slice::from_ref(&modulator))[0];
        let calibration = ModulatorCalibration {
            request,
            floor: 0.25,
            ceiling: 0.5,
            sample_count: 120,
        };
        let mut source = SpectrumSource::new(48_000, 64, Smoothing::None).unwrap();
        source
            .configure_modulators_with_calibrations(
                std::slice::from_ref(&modulator),
                std::slice::from_ref(&calibration),
            )
            .unwrap();
        assert_eq!(source.modulators[0].floor, 0.25);
        assert_eq!(source.modulators[0].ceiling, 0.5);

        let mut stale = calibration;
        stale.request.gain_db += 1.0;
        source
            .configure_modulators_with_calibrations(&[modulator], &[stale])
            .unwrap();
        assert_eq!(source.modulators[0].floor, 0.03);
        assert_eq!(source.modulators[0].ceiling, 0.8);
    }

    #[test]
    fn complete_track_calibration_honors_cancellation() {
        let project = crate::default_project(2.0);
        let modulator = crate::new_band_energy_modulator(&project);
        assert_eq!(
            calibrate_modulators(&tone(), 48_000, &[modulator], &|| true),
            Err(ModulatorCalibrationError::Cancelled)
        );
    }
}
