use rustfft::{FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};

use crate::beat::BeatGrid;

pub const SPECTRUM_NOTE_MIN_MIDI: u8 = 21;
pub const SPECTRUM_NOTE_MAX_MIDI: u8 = 108;
const SPECTRUM_MIN_HZ: f32 = 40.0;
const SPECTRUM_DISPLAY_MAX_HZ: f32 = 18_000.0;
const SPECTRUM_DB_FLOOR: f32 = -78.0;
const SPECTRUM_DB_CEIL: f32 = -12.0;
const SPECTRUM_DB_GAMMA: f32 = 1.35;
const SPECTRUM_BLEND_HALF_OCTAVES: f32 = 0.14;
const MULTI_RES_WINDOWS: [MultiResolutionWindowSpec; 5] = [
    MultiResolutionWindowSpec {
        size: 32_768,
        upper_hz: 90.0,
    },
    MultiResolutionWindowSpec {
        size: 16_384,
        upper_hz: 250.0,
    },
    MultiResolutionWindowSpec {
        size: 8_192,
        upper_hz: 1_000.0,
    },
    MultiResolutionWindowSpec {
        size: 4_096,
        upper_hz: 4_000.0,
    },
    MultiResolutionWindowSpec {
        size: 1_024,
        upper_hz: f32::INFINITY,
    },
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpectrumAnalysis {
    pub bands: Vec<f32>,
    pub notes: Vec<f32>,
    pub note_min_midi: u8,
}

#[derive(Clone, Copy, Debug)]
struct MultiResolutionWindowSpec {
    size: usize,
    upper_hz: f32,
}

struct MultiResolutionSpectrum {
    specs: Vec<MultiResolutionWindowSpec>,
    powers: Vec<Vec<f32>>,
    bin_hz: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecodedAudio {
    pub info: AudioStreamInfo,
    /// Interleaved stereo f32 samples in `[-1.0, 1.0]`.
    pub stereo_samples: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub bin_secs: f64,
    pub row_secs: f64,
    pub low_cut_hz: f32,
    pub mid_cut_hz: f32,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            bin_secs: 0.025,
            row_secs: 30.0,
            low_cut_hz: 250.0,
            mid_cut_hz: 2_500.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WaveformBin {
    pub start_secs: f64,
    pub duration_secs: f64,
    pub peak: f32,
    pub rms: f32,
    pub loudness_db: f32,
    /// Normalized low / mid / high energy. Used for DJ-style color mapping.
    pub band_energy: [f32; 3],
    /// 0..1 estimate of sudden energy change for percussive visual accents.
    pub transient: f32,
    /// Normalized low / mid / high contribution to the transient accent.
    pub transient_band: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineAnalysis {
    pub stream: AudioStreamInfo,
    pub config: AnalysisConfig,
    pub bins: Vec<WaveformBin>,
    pub beat_grid: BeatGrid,
}

pub fn analyze_stereo_timeline(
    stereo_samples: &[f32],
    sample_rate: u32,
    config: AnalysisConfig,
) -> TimelineAnalysis {
    let sample_rate = sample_rate.max(1);
    let frame_count = stereo_samples.len() / 2;
    let duration_secs = frame_count as f64 / sample_rate as f64;
    let frames_per_bin = (config.bin_secs.max(0.01) * sample_rate as f64)
        .round()
        .max(1.0) as usize;

    let mut bins = Vec::with_capacity(frame_count.div_ceil(frames_per_bin));
    let mut low_state = 0.0_f32;
    let mut mid_state = 0.0_f32;
    let mut prev_band_rms = [0.0_f32; 3];
    let low_alpha = one_pole_alpha(config.low_cut_hz, sample_rate);
    let mid_alpha = one_pole_alpha(config.mid_cut_hz, sample_rate);

    let mut frame_start = 0usize;
    while frame_start < frame_count {
        let frame_end = (frame_start + frames_per_bin).min(frame_count);
        let mut peak = 0.0_f32;
        let mut square_sum = 0.0_f64;
        let mut low_sum = 0.0_f64;
        let mut mid_sum = 0.0_f64;
        let mut high_sum = 0.0_f64;

        for frame_idx in frame_start..frame_end {
            let l = stereo_samples[frame_idx * 2];
            let r = stereo_samples[frame_idx * 2 + 1];
            let mono = ((l + r) * 0.5).clamp(-1.0, 1.0);
            let abs = mono.abs();
            peak = peak.max(abs);
            square_sum += (mono as f64) * (mono as f64);

            low_state += low_alpha * (mono - low_state);
            mid_state += mid_alpha * (mono - mid_state);
            let low = low_state;
            let mid = mid_state - low_state;
            let high = mono - mid_state;
            low_sum += (low as f64) * (low as f64);
            mid_sum += (mid as f64) * (mid as f64);
            high_sum += (high as f64) * (high as f64);
        }

        let n = (frame_end - frame_start).max(1) as f64;
        let rms = (square_sum / n).sqrt() as f32;
        let loudness_db = linear_to_db(rms);
        let total_band = (low_sum + mid_sum + high_sum).max(f64::EPSILON);
        let band_rms = [
            (low_sum / n).sqrt() as f32,
            (mid_sum / n).sqrt() as f32,
            (high_sum / n).sqrt() as f32,
        ];
        let transient_raw = [
            (band_rms[0] - prev_band_rms[0] - 0.006).max(0.0),
            (band_rms[1] - prev_band_rms[1] - 0.006).max(0.0),
            (band_rms[2] - prev_band_rms[2] - 0.006).max(0.0),
        ];
        let transient_total = transient_raw.iter().copied().sum::<f32>();
        let transient = (transient_total * 6.0).clamp(0.0, 1.0);
        let transient_band = if transient_total > 1.0e-8 {
            [
                transient_raw[0] / transient_total,
                transient_raw[1] / transient_total,
                transient_raw[2] / transient_total,
            ]
        } else {
            [0.0; 3]
        };
        bins.push(WaveformBin {
            start_secs: frame_start as f64 / sample_rate as f64,
            duration_secs: (frame_end - frame_start) as f64 / sample_rate as f64,
            peak,
            rms,
            loudness_db,
            band_energy: [
                (low_sum / total_band) as f32,
                (mid_sum / total_band) as f32,
                (high_sum / total_band) as f32,
            ],
            transient,
            transient_band,
        });
        prev_band_rms = [
            prev_band_rms[0] * 0.68 + band_rms[0] * 0.32,
            prev_band_rms[1] * 0.68 + band_rms[1] * 0.32,
            prev_band_rms[2] * 0.68 + band_rms[2] * 0.32,
        ];

        frame_start = frame_end;
    }

    let beat_grid = estimate_simple_beat_grid(&bins, duration_secs, config.bin_secs);

    TimelineAnalysis {
        stream: AudioStreamInfo {
            sample_rate,
            channels: 2,
            duration_secs,
        },
        config,
        bins,
        beat_grid,
    }
}

pub fn resample_linear_stereo(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if input.is_empty() || input_rate == 0 || output_rate == 0 || input_rate == output_rate {
        return input.to_vec();
    }
    let in_frames = input.len() / 2;
    if in_frames == 0 {
        return Vec::new();
    }
    let out_frames = ((in_frames as u64 * output_rate as u64).div_ceil(input_rate as u64)) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    let ratio = input_rate as f64 / output_rate as f64;
    for out_idx in 0..out_frames {
        let src_pos = out_idx as f64 * ratio;
        let i0 = src_pos.floor() as usize;
        let i1 = (i0 + 1).min(in_frames - 1);
        let frac = (src_pos - i0 as f64) as f32;
        for ch in 0..2 {
            let a = input[i0 * 2 + ch];
            let b = input[i1 * 2 + ch];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

pub fn spectrum_bands_from_stereo_window(
    stereo_samples: &[f32],
    sample_rate: u32,
    center_secs: f64,
    bands: usize,
) -> Vec<f32> {
    spectrum_analysis_from_stereo_window(stereo_samples, sample_rate, center_secs, bands).bands
}

pub fn spectrum_analysis_from_stereo_window(
    stereo_samples: &[f32],
    sample_rate: u32,
    center_secs: f64,
    bands: usize,
) -> SpectrumAnalysis {
    let bands = bands.clamp(1, 128);
    let frame_count = stereo_samples.len() / 2;
    if sample_rate == 0 || frame_count == 0 {
        return SpectrumAnalysis {
            bands: vec![0.0; bands],
            notes: vec![0.0; (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize],
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
        };
    }

    let spectrum =
        MultiResolutionSpectrum::from_stereo_window(stereo_samples, sample_rate, center_secs);
    let max_hz = spectrum_max_hz(sample_rate);
    let ratio = max_hz / SPECTRUM_MIN_HZ;
    let step = if bands <= 1 {
        2.0_f32.powf(1.0 / 12.0)
    } else {
        ratio.powf(1.0 / (bands - 1) as f32)
    };
    let edge_scale = step.sqrt();

    let mut values = Vec::with_capacity(bands);
    for band in 0..bands {
        let t = if bands == 1 {
            0.0
        } else {
            band as f32 / (bands - 1) as f32
        };
        let hz = SPECTRUM_MIN_HZ * ratio.powf(t);
        let low_hz = (hz / edge_scale).max(SPECTRUM_MIN_HZ * 0.5);
        let high_hz = (hz * edge_scale).min(max_hz);
        let power = spectrum.power_for_range(hz, low_hz, high_hz);
        values.push(power_to_display_value(power));
    }

    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    let mut notes = Vec::with_capacity(note_count);
    for midi in SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI {
        let hz = midi_to_hz(midi);
        let low_hz = hz / 2.0_f32.powf(1.0 / 24.0);
        let high_hz = hz * 2.0_f32.powf(1.0 / 24.0);
        if high_hz < SPECTRUM_MIN_HZ || low_hz > max_hz {
            notes.push(0.0);
        } else {
            let power = spectrum.power_for_range(hz, low_hz.max(10.0), high_hz.min(max_hz));
            notes.push(power_to_display_value(power));
        }
    }

    SpectrumAnalysis {
        bands: values,
        notes,
        note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
    }
}

impl MultiResolutionSpectrum {
    fn from_stereo_window(stereo_samples: &[f32], sample_rate: u32, center_secs: f64) -> Self {
        let frame_count = stereo_samples.len() / 2;
        let center_frame = if center_secs.is_finite() {
            (center_secs.max(0.0) * sample_rate.max(1) as f64).round() as usize
        } else {
            0
        }
        .min(frame_count.saturating_sub(1));
        let mut planner = FftPlanner::<f32>::new();
        let mut specs = Vec::with_capacity(MULTI_RES_WINDOWS.len());
        let mut powers = Vec::with_capacity(MULTI_RES_WINDOWS.len());
        let mut bin_hz = Vec::with_capacity(MULTI_RES_WINDOWS.len());
        for spec in MULTI_RES_WINDOWS {
            let size = spec.size.min(frame_count).max(1);
            let power = fft_power_window(stereo_samples, center_frame, size, &mut planner);
            specs.push(MultiResolutionWindowSpec {
                size,
                upper_hz: spec.upper_hz,
            });
            powers.push(power);
            bin_hz.push(sample_rate.max(1) as f32 / size as f32);
        }
        Self {
            specs,
            powers,
            bin_hz,
        }
    }

    fn power_for_range(&self, center_hz: f32, low_hz: f32, high_hz: f32) -> f32 {
        let weights = self.window_weights(center_hz);
        weights
            .into_iter()
            .map(|(idx, weight)| {
                weight * range_power_from_fft(&self.powers[idx], self.bin_hz[idx], low_hz, high_hz)
            })
            .sum::<f32>()
            .max(0.0)
    }

    fn window_weights(&self, hz: f32) -> Vec<(usize, f32)> {
        let mut idx = 0;
        while idx + 1 < self.specs.len() && hz >= self.specs[idx].upper_hz {
            idx += 1;
        }
        if idx > 0 {
            let boundary = self.specs[idx - 1].upper_hz;
            if boundary.is_finite() {
                let distance = (hz.max(1.0).log2() - boundary.log2()) / SPECTRUM_BLEND_HALF_OCTAVES;
                if (-1.0..=1.0).contains(&distance) {
                    let upper = ((distance + 1.0) * 0.5).clamp(0.0, 1.0);
                    return vec![(idx - 1, 1.0 - upper), (idx, upper)];
                }
            }
        }
        if idx + 1 < self.specs.len() {
            let boundary = self.specs[idx].upper_hz;
            if boundary.is_finite() {
                let distance = (hz.max(1.0).log2() - boundary.log2()) / SPECTRUM_BLEND_HALF_OCTAVES;
                if (-1.0..=1.0).contains(&distance) {
                    let upper = ((distance + 1.0) * 0.5).clamp(0.0, 1.0);
                    return vec![(idx, 1.0 - upper), (idx + 1, upper)];
                }
            }
        }
        vec![(idx, 1.0)]
    }
}

fn fft_power_window(
    stereo_samples: &[f32],
    center_frame: usize,
    window_frames: usize,
    planner: &mut FftPlanner<f32>,
) -> Vec<f32> {
    let frame_count = stereo_samples.len() / 2;
    let max_start = frame_count.saturating_sub(window_frames);
    let start_frame = center_frame
        .saturating_sub(window_frames / 2)
        .min(max_start);
    let denom = (window_frames.saturating_sub(1)).max(1) as f32;
    let mut hann_sum = 0.0_f32;
    let mut samples = vec![Complex32::new(0.0, 0.0); window_frames];
    for (i, sample) in samples.iter_mut().enumerate() {
        let frame_idx = start_frame + i;
        let l = stereo_samples[frame_idx * 2];
        let r = stereo_samples[frame_idx * 2 + 1];
        let mono = ((l + r) * 0.5).clamp(-1.0, 1.0);
        let hann = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / denom).cos();
        hann_sum += hann;
        sample.re = mono * hann;
    }
    let fft = planner.plan_fft_forward(window_frames);
    fft.process(&mut samples);

    let half = window_frames / 2;
    let mut powers = Vec::with_capacity(half + 1);
    let scale = hann_sum.max(1.0);
    for (idx, value) in samples.iter().take(half + 1).enumerate() {
        let mirror_scale = if idx == 0 || (window_frames % 2 == 0 && idx == half) {
            1.0
        } else {
            2.0
        };
        let amplitude = value.norm() * mirror_scale / scale;
        powers.push(amplitude * amplitude);
    }
    powers
}

fn range_power_from_fft(powers: &[f32], bin_hz: f32, low_hz: f32, high_hz: f32) -> f32 {
    if powers.is_empty() || bin_hz <= 0.0 || high_hz <= low_hz {
        return 0.0;
    }
    let start = ((low_hz / bin_hz).floor() as usize).max(1);
    let end = ((high_hz / bin_hz).ceil() as usize).min(powers.len().saturating_sub(1));
    if end < start {
        let idx = ((low_hz.max(0.0) / bin_hz).round() as usize).min(powers.len() - 1);
        return powers[idx];
    }
    let slice = &powers[start..=end];
    let sum = slice.iter().copied().sum::<f32>();
    let peak = slice.iter().copied().fold(0.0_f32, f32::max);
    let avg = sum / slice.len().max(1) as f32;
    avg.max(peak * 0.35)
}

fn power_to_display_value(power: f32) -> f32 {
    let db = 10.0 * power.max(1.0e-12).log10();
    ((db - SPECTRUM_DB_FLOOR) / (SPECTRUM_DB_CEIL - SPECTRUM_DB_FLOOR))
        .clamp(0.0, 1.0)
        .powf(SPECTRUM_DB_GAMMA)
}

fn spectrum_max_hz(sample_rate: u32) -> f32 {
    let nyquist = sample_rate.max(1) as f32 * 0.5;
    SPECTRUM_DISPLAY_MAX_HZ
        .min(nyquist * 0.92)
        .max(SPECTRUM_MIN_HZ * 1.2)
}

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

fn one_pole_alpha(cut_hz: f32, sample_rate: u32) -> f32 {
    let x = (-2.0 * std::f32::consts::PI * cut_hz.max(1.0) / sample_rate as f32).exp();
    (1.0 - x).clamp(0.0, 1.0)
}

fn linear_to_db(v: f32) -> f32 {
    if v <= 1.0e-9 {
        -120.0
    } else {
        (20.0 * v.log10()).clamp(-120.0, 12.0)
    }
}

fn estimate_simple_beat_grid(bins: &[WaveformBin], duration_secs: f64, bin_secs: f64) -> BeatGrid {
    if bins.len() < 32 || bin_secs <= 0.0 {
        return BeatGrid::empty();
    }
    let mut onset = vec![0.0_f32; bins.len()];
    for i in 1..bins.len() {
        let prev = bins[i - 1].band_energy[0] * 0.55
            + bins[i - 1].band_energy[1] * 0.35
            + bins[i - 1].rms * 0.10;
        let now =
            bins[i].band_energy[0] * 0.55 + bins[i].band_energy[1] * 0.35 + bins[i].rms * 0.10;
        onset[i] = (now - prev).max(0.0);
    }

    let mut best_bpm = 0.0_f32;
    let mut best_score = 0.0_f32;
    for bpm in 70..=180 {
        let lag = (60.0 / bpm as f64 / bin_secs).round() as usize;
        if lag < 2 || lag >= onset.len() / 2 {
            continue;
        }
        let mut score = 0.0_f32;
        for i in lag..onset.len() {
            score += onset[i] * onset[i - lag];
        }
        score /= (onset.len() - lag) as f32;
        if score > best_score {
            best_score = score;
            best_bpm = bpm as f32;
        }
    }

    if best_bpm <= 0.0 {
        return BeatGrid::empty();
    }

    let max_onset = onset.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
    let confidence = (best_score.sqrt() / max_onset).clamp(0.0, 1.0);
    let first_beat = onset
        .iter()
        .take(((60.0 / best_bpm as f64) / bin_secs).ceil() as usize * 2)
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(idx, _)| idx as f64 * bin_secs)
        .unwrap_or(0.0);

    BeatGrid::from_bpm(duration_secs, best_bpm, first_beat, confidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_stereo_shape() {
        let input = vec![0.0, 1.0, 0.5, 0.5, 1.0, 0.0];
        let out = resample_linear_stereo(&input, 3, 6);
        assert_eq!(out.len(), 12);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 1.0);
    }

    #[test]
    fn analysis_produces_bins() {
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let x = if i % 4800 < 120 { 0.8 } else { 0.0 };
            samples.extend([x, x]);
        }
        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        assert!(!analysis.bins.is_empty());
        assert!(analysis.bins.iter().any(|b| b.peak > 0.5));
    }

    #[test]
    fn analysis_marks_percussive_transients() {
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let x = if i % 12_000 < 180 { 0.9 } else { 0.0 };
            samples.extend([x, x]);
        }
        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        assert!(analysis.bins.iter().any(|b| b.transient > 0.25));
    }

    #[test]
    fn spectrum_window_returns_requested_band_count() {
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let phase = std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0;
            let x = phase.sin() * 0.5;
            samples.extend([x, x]);
        }

        let bands = spectrum_bands_from_stereo_window(&samples, 48_000, 0.5, 50);
        assert_eq!(bands.len(), 50);
        assert!(bands.iter().any(|v| *v > 0.5));
        assert!(bands.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn spectrum_analysis_reports_pitch_note_strength() {
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let phase = std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0;
            let x = phase.sin() * 0.6;
            samples.extend([x, x]);
        }

        let spectrum = spectrum_analysis_from_stereo_window(&samples, 48_000, 0.5, 108);
        assert_eq!(spectrum.bands.len(), 108);
        assert_eq!(
            spectrum.notes.len(),
            (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize
        );
        let a4 = (69 - spectrum.note_min_midi) as usize;
        assert!(spectrum.notes[a4] > 0.5);
    }
}
