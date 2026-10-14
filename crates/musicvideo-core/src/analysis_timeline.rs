//! Frame-indexed analysis. FFT caches are disposable; checkpoints contain only
//! the completed downstream state and never own FFT buffers.
use super::*;
use crate::FrameGrid;
use music_core::{PcmView, SpectrumRefreshMode};
use rayon::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
enum SmoothingKey {
    None,
    Average(u32),
    Envelope(u32, u32),
}
impl From<Smoothing> for SmoothingKey {
    fn from(s: Smoothing) -> Self {
        match s {
            Smoothing::None => Self::None,
            Smoothing::Average { factor } => Self::Average(factor.to_bits()),
            Smoothing::Envelope {
                attack_secs,
                release_secs,
            } => Self::Envelope(attack_secs.to_bits(), release_secs.to_bits()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RangeKey {
    Hz(u32, u32),
    Midi(u8, u8),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum CalibrationKey {
    Manual,
    Auto(u32, u32),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ModulatorKey {
    id: ModulatorId,
    onset: bool,
    range: RangeKey,
    smoothing: SmoothingKey,
    weighting: BandWeighting,
    gain: u32,
    manual_floor: u32,
    manual_ceiling: u32,
    calibration: CalibrationKey,
    threshold: u32,
    floor: u32,
    ceiling: u32,
}
impl ModulatorKey {
    fn new(m: &ConfiguredModulator) -> Self {
        let (onset, range, smoothing, weighting, gain_db, floor, ceiling, calibration, threshold) =
            match m.source {
                ModulatorSource::BandEnergy {
                    range,
                    smoothing,
                    weighting,
                    gain_db,
                    floor,
                    ceiling,
                    calibration,
                    threshold,
                } => (
                    false,
                    range,
                    smoothing,
                    weighting,
                    gain_db,
                    floor,
                    ceiling,
                    calibration,
                    threshold,
                ),
                ModulatorSource::BandOnset {
                    range,
                    smoothing,
                    weighting,
                    gain_db,
                    floor,
                    ceiling,
                    calibration,
                    threshold,
                } => (
                    true,
                    range,
                    smoothing,
                    weighting,
                    gain_db,
                    floor,
                    ceiling,
                    calibration,
                    threshold,
                ),
            };
        Self {
            id: m.id,
            onset,
            range: match range {
                BandRange::Hz { low_hz, high_hz } => {
                    RangeKey::Hz(low_hz.to_bits(), high_hz.to_bits())
                }
                BandRange::Semitone {
                    low_midi,
                    high_midi,
                } => RangeKey::Midi(low_midi, high_midi),
            },
            smoothing: smoothing.into(),
            weighting,
            gain: gain_db.to_bits(),
            manual_floor: floor.to_bits(),
            manual_ceiling: ceiling.to_bits(),
            calibration: match calibration {
                BandCalibration::Manual => CalibrationKey::Manual,
                BandCalibration::Auto {
                    low_percentile,
                    high_percentile,
                } => CalibrationKey::Auto(low_percentile.to_bits(), high_percentile.to_bits()),
            },
            threshold: threshold.to_bits(),
            floor: m.floor.to_bits(),
            ceiling: m.ceiling.to_bits(),
        }
    }
}

/// Typed identity of every analysis input. Floating point configuration compares
/// by bits; names and drawing-only project fields do not invalidate analysis.
#[derive(Clone)]
pub struct AnalysisKey<I> {
    pcm_id: I,
    sample_rate: u32,
    logical_frames: usize,
    grid: FrameGrid,
    band_count: usize,
    smoothing: Smoothing,
    smoothing_key: SmoothingKey,
    mode: SpectrumRefreshMode,
    modulators: Vec<ConfiguredModulator>,
    modulator_keys: Vec<ModulatorKey>,
    gain_bits: u32,
}
impl<I: PartialEq> PartialEq for AnalysisKey<I> {
    fn eq(&self, other: &Self) -> bool {
        self.pcm_id == other.pcm_id
            && self.sample_rate == other.sample_rate
            && self.logical_frames == other.logical_frames
            && self.grid == other.grid
            && self.band_count == other.band_count
            && self.mode == other.mode
            && self.smoothing_key == other.smoothing_key
            && self.modulator_keys == other.modulator_keys
            && self.gain_bits == other.gain_bits
    }
}
impl<I: Eq> Eq for AnalysisKey<I> {}
impl<I> AnalysisKey<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pcm_id: I,
        sample_rate: u32,
        logical_frames: usize,
        grid: FrameGrid,
        band_count: usize,
        smoothing: Smoothing,
        modulators: &[Modulator],
        calibrations: &[ModulatorCalibration],
        display_gain_db: f32,
    ) -> Result<Self, SpectrumSourceError> {
        // Reuse the existing validation and effective-calibration rules.
        let mut source = SpectrumSource::new(sample_rate, band_count, smoothing)?;
        source.configure_modulators_with_calibrations(modulators, calibrations)?;
        if !display_gain_db.is_finite() {
            return Err(SpectrumSourceError::InvalidModulator);
        }
        Ok(Self {
            pcm_id,
            sample_rate,
            logical_frames,
            grid,
            band_count,
            smoothing,
            smoothing_key: smoothing.into(),
            mode: SpectrumRefreshMode::FrameGrid,
            modulator_keys: source.modulators.iter().map(ModulatorKey::new).collect(),
            modulators: source.modulators,
            gain_bits: display_gain_db.to_bits(),
        })
    }
    pub fn grid(&self) -> FrameGrid {
        self.grid
    }
}

#[derive(Clone, Default)]
struct Downstream {
    analysis: SpectrumAnalysis,
    keyboard: KeyboardAnimationState,
    visual: MivSpectrumState,
    modulator_values: Vec<ModulatorValue>,
    previous_inputs: Vec<Option<f32>>,
    frame: SpectrumFrame,
}
#[derive(Clone)]
struct Completed {
    index: u64,
    state: Downstream,
}
#[derive(Clone, Default)]
enum Progress {
    #[default]
    Unprocessed,
    Completed(Completed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisTimelineError {
    Cancelled,
    InvalidFrame,
    InvalidPcmView,
}
impl fmt::Display for AnalysisTimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "analysis timeline: {self:?}")
    }
}
impl std::error::Error for AnalysisTimelineError {}

pub struct AnalysisTimeline<I> {
    key: Option<AnalysisKey<I>>,
    analyzer: SpectrumAnalyzer,
    progress: Progress,
    checkpoints: BTreeMap<u64, Completed>,
    ranges: Vec<SpectrumRangeRequest>,
}
impl<I> Default for AnalysisTimeline<I> {
    fn default() -> Self {
        Self {
            key: None,
            analyzer: SpectrumAnalyzer::with_refresh_mode(1, SpectrumRefreshMode::FrameGrid),
            progress: Progress::Unprocessed,
            checkpoints: BTreeMap::new(),
            ranges: Vec::new(),
        }
    }
}
impl<I: Clone + Eq> AnalysisTimeline<I> {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn key(&self) -> Option<&AnalysisKey<I>> {
        self.key.as_ref()
    }
    pub fn processed_frame(&self) -> Option<u64> {
        match &self.progress {
            Progress::Unprocessed => None,
            Progress::Completed(c) => Some(c.index),
        }
    }
    pub fn frame(
        &mut self,
        key: &AnalysisKey<I>,
        pcm: PcmView<'_>,
        n: u64,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<&SpectrumFrame, AnalysisTimelineError> {
        self.frame_impl(key, pcm, n, cancel, true)
    }
    fn frame_impl(
        &mut self,
        key: &AnalysisKey<I>,
        pcm: PcmView<'_>,
        n: u64,
        cancel: &(dyn Fn() -> bool + Sync),
        parallel: bool,
    ) -> Result<&SpectrumFrame, AnalysisTimelineError> {
        // Includes the cancellation check immediately following the host lock.
        if cancel() {
            return Err(AnalysisTimelineError::Cancelled);
        }
        if n >= key.grid.frames() {
            return Err(AnalysisTimelineError::InvalidFrame);
        }
        if pcm.logical_frames != key.logical_frames {
            return Err(AnalysisTimelineError::InvalidPcmView);
        }
        if self.key.as_ref() != Some(key) {
            self.progress = Progress::Unprocessed;
            self.checkpoints.clear();
            self.analyzer = SpectrumAnalyzer::with_refresh_mode(key.band_count, key.mode);
            self.ranges = key.modulators.iter().map(modulator_range_request).collect();
            self.key = Some(key.clone());
        }
        let current = self.processed_frame().filter(|&c| c <= n);
        let checkpoint = self.checkpoints.range(..=n).next_back().map(|(&c, _)| c);
        if checkpoint.is_some_and(|c| current.is_none_or(|current| c > current)) {
            self.progress = Progress::Completed(self.checkpoints[&checkpoint.unwrap()].clone());
        } else if current.is_none() {
            self.progress = Progress::Unprocessed;
        }
        let interval = key.grid.checkpoint_interval();
        while self.processed_frame() != Some(n) {
            if cancel() {
                return Err(AnalysisTimelineError::Cancelled);
            }
            let next = self.processed_frame().map_or(0, |c| c + 1);
            if parallel && n - next >= 2 * interval {
                // Only raw, history-free FFT work runs concurrently. The bounded
                // batch is discarded after ordered downstream steps; no raw cache.
                let batch_end = (next + interval * rayon::current_num_threads() as u64).min(n + 1);
                let starts = (next..batch_end)
                    .step_by(interval as usize)
                    .collect::<Vec<_>>();
                let bands = key.band_count;
                let rate = key.sample_rate;
                let grid = key.grid;
                let ranges = &self.ranges;
                let segments = starts
                    .into_par_iter()
                    .map(|start| {
                        if cancel() {
                            return (start, Vec::new());
                        }
                        let mut analyzer = SpectrumAnalyzer::with_refresh_mode(
                            bands,
                            SpectrumRefreshMode::FrameGrid,
                        );
                        let values = (start..(start + interval).min(batch_end))
                            .map(|index| {
                                analyzer.analyze_with_ranges_view(
                                    pcm,
                                    rate,
                                    grid.time(index),
                                    ranges,
                                )
                            })
                            .collect::<Vec<_>>();
                        (start, values)
                    })
                    .collect::<Vec<_>>();
                for (start, values) in segments {
                    if cancel() {
                        return Err(AnalysisTimelineError::Cancelled);
                    }
                    for (offset, raw) in values.into_iter().enumerate() {
                        let index = start + offset as u64;
                        self.step(key, pcm, index, Some(raw));
                        if (index + 1) % interval == 0 {
                            if let Progress::Completed(c) = &self.progress {
                                self.checkpoints.insert(index, c.clone());
                            }
                        }
                    }
                }
                continue;
            }
            // One exact step owns all temporal arithmetic, regardless of request order.
            self.step(key, pcm, next, None);
            if (next + 1) % interval == 0 {
                if let Progress::Completed(c) = &self.progress {
                    self.checkpoints.insert(next, c.clone());
                }
            }
        }
        match &self.progress {
            Progress::Completed(c) => Ok(&c.state.frame),
            Progress::Unprocessed => unreachable!(),
        }
    }
    fn step(
        &mut self,
        key: &AnalysisKey<I>,
        pcm: PcmView<'_>,
        n: u64,
        raw: Option<music_core::SpectrumAnalysisWithRanges>,
    ) {
        let t = key.grid.time(n);
        let delta = if n == 0 {
            0.0
        } else {
            (t - key.grid.time(n - 1)) as f32
        };
        let raw = raw.unwrap_or_else(|| {
            self.analyzer
                .analyze_with_ranges_view(pcm, key.sample_rate, t, &self.ranges)
        });
        let mut state = match std::mem::take(&mut self.progress) {
            Progress::Unprocessed => Downstream::default(),
            Progress::Completed(c) => c.state,
        };
        let analyzed = raw.spectrum;
        if n == 0 {
            state.analysis = analyzed;
        } else {
            state.analysis.notes = analyzed.notes;
            state.analysis.note_min_midi = analyzed.note_min_midi;
            match key.smoothing {
                Smoothing::None => state.analysis.bands = analyzed.bands,
                Smoothing::Average { factor } => {
                    for (previous, current) in state.analysis.bands.iter_mut().zip(analyzed.bands) {
                        *previous = factor * *previous + (1.0 - factor) * current;
                    }
                }
                Smoothing::Envelope {
                    attack_secs,
                    release_secs,
                } => {
                    for (previous, current) in state.analysis.bands.iter_mut().zip(analyzed.bands) {
                        *previous =
                            follow_envelope(*previous, current, delta, attack_secs, release_secs);
                    }
                }
            }
        }
        update_modulator_values(
            &key.modulators,
            &raw.range_values,
            n == 0,
            delta,
            &mut state.modulator_values,
            &mut state.previous_inputs,
        );
        state.frame.bands.clone_from(&state.analysis.bands);
        state.frame.notes.clone_from(&state.analysis.notes);
        state.frame.note_min_midi = state.analysis.note_min_midi;
        state.frame.modulators.clone_from(&state.modulator_values);
        state
            .frame
            .miv_spectrum
            .clone_from(state.visual.advance_frame(&state.analysis.bands));
        prepare_keyboard_frame_into(
            &state.analysis.notes,
            state.analysis.note_min_midi,
            f32::from_bits(key.gain_bits),
            &mut state.keyboard,
            &mut state.frame.keyboard,
        );
        self.progress = Progress::Completed(Completed { index: n, state });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{default_project, new_band_energy_modulator};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn samples() -> Vec<f32> {
        (0..24_000)
            .flat_map(|n| {
                let t = n as f32 / 8_000.0;
                let value = (std::f32::consts::TAU * (80.0 * t + 120.0 * t * t)).sin()
                    * (0.1 + 0.8 * (t * 7.0).sin().abs());
                [value, value]
            })
            .collect()
    }
    fn key(id: u64, grid: FrameGrid, gain: f32) -> AnalysisKey<u64> {
        let mut project = default_project(3.0);
        let mut energy = new_band_energy_modulator(&project);
        energy.id = ModulatorId(1);
        project.modulators.push(energy.clone());
        let ModulatorSource::BandOnset {
            range,
            smoothing,
            weighting,
            gain_db,
            floor,
            ceiling,
            calibration,
            threshold,
        } = energy.source
        else {
            unreachable!()
        };
        project.modulators.push(Modulator {
            id: ModulatorId(2),
            name: "energy".into(),
            source: ModulatorSource::BandEnergy {
                range,
                smoothing,
                weighting,
                gain_db,
                floor,
                ceiling,
                calibration,
                threshold,
            },
        });
        AnalysisKey::new(
            id,
            8_000,
            24_000,
            grid,
            32,
            Smoothing::default(),
            &project.modulators,
            &[],
            gain,
        )
        .unwrap()
    }
    fn bits(frame: &SpectrumFrame) -> Vec<u32> {
        let mut output = vec![
            frame.note_min_midi as u32,
            frame.keyboard.note_min_midi as u32,
        ];
        output.extend(frame.bands.iter().chain(&frame.notes).map(|v| v.to_bits()));
        for level in &frame.keyboard.levels {
            output.extend([
                level.center.to_bits(),
                level.even.to_bits(),
                level.odd.to_bits(),
            ]);
        }
        for band in &frame.miv_spectrum.bands {
            output.extend([
                band.value.to_bits(),
                band.trail.to_bits(),
                band.onset.to_bits(),
            ]);
        }
        for m in &frame.modulators {
            output.extend([m.id.0 as u32, (m.id.0 >> 32) as u32, m.value.to_bits()]);
        }
        output
    }
    fn reference(key: &AnalysisKey<u64>, pcm: PcmView<'_>) -> Vec<Vec<u32>> {
        let mut reference = AnalysisTimeline::default();
        (0..key.grid.frames())
            .map(|n| bits(reference.frame_impl(key, pcm, n, &|| false, false).unwrap()))
            .collect()
    }
    #[test]
    fn arbitrary_requests_and_parallel_batches_match_sequential_bits() {
        let pcm = samples();
        let view = PcmView::new(&pcm, 24_000);
        for fps in [60.0, 59.94, 2.0] {
            let grid = FrameGrid::new(fps, 3.0).unwrap();
            let key = key(1, grid, 0.0);
            let expected = reference(&key, view);
            let mut timeline = AnalysisTimeline::default();
            let end = grid.frames() - 1;
            let interval = grid.checkpoint_interval();
            let mut requests = vec![end, 0, interval - 1, interval - 1, interval, end, 1];
            let mut seed = 0x12345678_u64;
            for _ in 0..100 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                requests.push(seed % grid.frames());
            }
            for n in requests {
                assert_eq!(
                    bits(timeline.frame(&key, view, n, &|| false).unwrap()),
                    expected[n as usize],
                    "fps={fps}, n={n}"
                );
            }
        }
    }
    #[test]
    fn cancellation_retains_completed_state_and_resumes_exactly() {
        let pcm = samples();
        let view = PcmView::new(&pcm, 24_000);
        let grid = FrameGrid::new(60.0, 3.0).unwrap();
        let key = key(1, grid, 0.0);
        let expected = reference(&key, view);
        let mut timeline = AnalysisTimeline::default();
        timeline.frame(&key, view, 59, &|| false).unwrap();
        let calls = AtomicUsize::new(0);
        assert!(matches!(
            timeline.frame_impl(
                &key,
                view,
                120,
                &|| calls.fetch_add(1, Ordering::Relaxed) > 10,
                false
            ),
            Err(AnalysisTimelineError::Cancelled)
        ));
        let completed = timeline.processed_frame().unwrap();
        assert!((59..120).contains(&completed));
        assert_eq!(
            bits(timeline.frame(&key, view, completed, &|| false).unwrap()),
            expected[completed as usize]
        );
        assert_eq!(
            bits(timeline.frame(&key, view, 179, &|| false).unwrap()),
            expected[179]
        );
        let changed = key_with_gain(&key, 3.0);
        assert!(matches!(
            timeline.frame(&changed, view, 0, &|| true),
            Err(AnalysisTimelineError::Cancelled)
        ));
        assert!(timeline.key() == Some(&key));
        assert_eq!(timeline.processed_frame(), Some(179));
    }
    fn key_with_gain(k: &AnalysisKey<u64>, gain: f32) -> AnalysisKey<u64> {
        let mut k = k.clone();
        k.gain_bits = gain.to_bits();
        k
    }
    #[test]
    fn key_changes_invalidate_history_including_signed_zero_and_order() {
        let pcm = samples();
        let view = PcmView::new(&pcm, 24_000);
        let grid = FrameGrid::new(60.0, 3.0).unwrap();
        let original = key(1, grid, 0.0);
        let mut variants = vec![
            key_with_gain(&original, -0.0),
            key_with_gain(&original, 3.0),
            key(2, grid, 0.0),
        ];
        let mut k = original.clone();
        k.sample_rate = 16_000;
        variants.push(k);
        let mut k = original.clone();
        k.logical_frames = 16_000;
        variants.push(k);
        let mut k = original.clone();
        k.grid = FrameGrid::new(59.94, 3.0).unwrap();
        variants.push(k);
        let mut k = original.clone();
        k.band_count = 64;
        variants.push(k);
        let mut k = original.clone();
        k.smoothing = Smoothing::None;
        k.smoothing_key = Smoothing::None.into();
        variants.push(k);
        let mut k = original.clone();
        k.modulators.reverse();
        k.modulator_keys.reverse();
        variants.push(k);
        let mut k = original.clone();
        k.modulators[0].floor = 0.1;
        k.modulator_keys[0].floor = 0.1_f32.to_bits();
        variants.push(k);
        for variant in variants {
            assert!(variant != original);
            let mut timeline = AnalysisTimeline::default();
            timeline.frame(&original, view, 179, &|| false).unwrap();
            let changed_view = PcmView::new(&pcm, variant.logical_frames);
            let mut fresh = AnalysisTimeline::default();
            let expected = bits(fresh.frame(&variant, changed_view, 100, &|| false).unwrap());
            assert_eq!(
                bits(
                    timeline
                        .frame(&variant, changed_view, 100, &|| false)
                        .unwrap()
                ),
                expected
            );
            assert!(timeline.key() == Some(&variant));
            assert!(timeline.checkpoints.keys().all(|n| *n <= 100));
        }
    }
}
