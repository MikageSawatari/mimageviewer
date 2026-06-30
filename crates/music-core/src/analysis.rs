use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};

use crate::beat::BeatGrid;

pub const SPECTRUM_NOTE_MIN_MIDI: u8 = 21;
pub const SPECTRUM_NOTE_MAX_MIDI: u8 = 108;
const SPECTRUM_MIN_HZ: f32 = 20.0;
const SPECTRUM_DISPLAY_MAX_HZ: f32 = 18_000.0;
const SPECTRUM_DB_FLOOR: f32 = -72.0;
const SPECTRUM_DB_CEIL: f32 = -8.0;
const SPECTRUM_DB_GAMMA: f32 = 1.65;
const SPECTRUM_BLEND_HALF_OCTAVES: f32 = 0.14;
const MULTI_RES_WINDOWS: [MultiResolutionWindowSpec; 5] = [
    MultiResolutionWindowSpec {
        size: 32_768,
        upper_hz: 90.0,
        refresh_secs: 0.025,
    },
    MultiResolutionWindowSpec {
        size: 16_384,
        upper_hz: 250.0,
        refresh_secs: 0.018,
    },
    MultiResolutionWindowSpec {
        size: 8_192,
        upper_hz: 1_000.0,
        refresh_secs: 0.010,
    },
    MultiResolutionWindowSpec {
        size: 4_096,
        upper_hz: 4_000.0,
        refresh_secs: 0.014,
    },
    MultiResolutionWindowSpec {
        size: 1_024,
        upper_hz: f32::INFINITY,
        refresh_secs: 0.005,
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
    refresh_secs: f64,
}

pub struct SpectrumAnalyzer {
    bands: usize,
    sample_rate: u32,
    frame_count: usize,
    windows: Vec<SpectrumFftWindow>,
}

struct SpectrumFftWindow {
    spec: MultiResolutionWindowSpec,
    fft: Arc<dyn Fft<f32>>,
    samples: Vec<Complex32>,
    powers: Vec<f32>,
    hann: Vec<f32>,
    hann_sum: f32,
    bin_hz: f32,
    last_center_frame: Option<usize>,
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
    /// 0..1 lightweight vocal-likelihood estimate for timeline coloring.
    pub vocal_score: f32,
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
    let mut vocal_raw = Vec::with_capacity(frame_count.div_ceil(frames_per_bin));

    let mut frame_start = 0usize;
    while frame_start < frame_count {
        let frame_end = (frame_start + frames_per_bin).min(frame_count);
        let mut peak = 0.0_f32;
        let mut square_sum = 0.0_f64;
        let mut low_sum = 0.0_f64;
        let mut mid_sum = 0.0_f64;
        let mut high_sum = 0.0_f64;
        let mut abs_sum = 0.0_f64;
        let mut crossings = 0usize;
        let mut prev_mono = 0.0_f32;
        let mut have_prev_mono = false;

        for frame_idx in frame_start..frame_end {
            let l = stereo_samples[frame_idx * 2];
            let r = stereo_samples[frame_idx * 2 + 1];
            let mono = ((l + r) * 0.5).clamp(-1.0, 1.0);
            let abs = mono.abs();
            peak = peak.max(abs);
            square_sum += (mono as f64) * (mono as f64);
            abs_sum += abs as f64;
            if have_prev_mono
                && mono.signum() != prev_mono.signum()
                && abs.max(prev_mono.abs()) > 0.004
            {
                crossings += 1;
            }
            prev_mono = mono;
            have_prev_mono = true;

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
        let band_energy = [
            (low_sum / total_band) as f32,
            (mid_sum / total_band) as f32,
            (high_sum / total_band) as f32,
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
        let crest = peak / rms.max(1.0e-6);
        let zero_cross_rate = crossings as f32 / (frame_end - frame_start).max(1) as f32;
        let mean_abs = (abs_sum / n) as f32;
        let vocal_candidate = vocal_candidate_score(
            loudness_db,
            band_energy,
            zero_cross_rate,
            crest,
            transient,
            mean_abs,
            rms,
        );
        vocal_raw.push(vocal_candidate);
        bins.push(WaveformBin {
            start_secs: frame_start as f64 / sample_rate as f64,
            duration_secs: (frame_end - frame_start) as f64 / sample_rate as f64,
            peak,
            rms,
            loudness_db,
            band_energy,
            transient,
            transient_band,
            vocal_score: 0.0,
        });
        prev_band_rms = [
            prev_band_rms[0] * 0.68 + band_rms[0] * 0.32,
            prev_band_rms[1] * 0.68 + band_rms[1] * 0.32,
            prev_band_rms[2] * 0.68 + band_rms[2] * 0.32,
        ];

        frame_start = frame_end;
    }
    apply_vocal_scores(&mut bins, &vocal_raw, config.bin_secs);

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
    let mut analyzer = SpectrumAnalyzer::new(bands);
    analyzer.analyze(stereo_samples, sample_rate, center_secs)
}

impl SpectrumAnalyzer {
    pub fn new(bands: usize) -> Self {
        Self {
            bands: bands.clamp(1, 128),
            sample_rate: 0,
            frame_count: 0,
            windows: Vec::new(),
        }
    }

    pub fn set_bands(&mut self, bands: usize) {
        self.bands = bands.clamp(1, 128);
    }

    pub fn analyze(
        &mut self,
        stereo_samples: &[f32],
        sample_rate: u32,
        center_secs: f64,
    ) -> SpectrumAnalysis {
        let bands = self.bands;
        let frame_count = stereo_samples.len() / 2;
        if sample_rate == 0 || frame_count == 0 {
            return SpectrumAnalysis {
                bands: vec![0.0; bands],
                notes: vec![0.0; (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize],
                note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            };
        }

        self.update_windows(stereo_samples, sample_rate, center_secs);
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
            let power = self.power_for_range(hz, low_hz, high_hz);
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
                let power = self.power_for_range(hz, low_hz.max(10.0), high_hz.min(max_hz));
                notes.push(power_to_display_value(power));
            }
        }

        SpectrumAnalysis {
            bands: values,
            notes,
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
        }
    }

    fn update_windows(&mut self, stereo_samples: &[f32], sample_rate: u32, center_secs: f64) {
        let frame_count = stereo_samples.len() / 2;
        let center_frame = if center_secs.is_finite() {
            (center_secs.max(0.0) * sample_rate.max(1) as f64).round() as usize
        } else {
            0
        }
        .min(frame_count.saturating_sub(1));
        self.ensure_windows(sample_rate, frame_count);

        for window in &mut self.windows {
            let refresh_frames = (window.spec.refresh_secs * sample_rate.max(1) as f64)
                .round()
                .max(1.0) as usize;
            let needs_update = window.last_center_frame.is_none_or(|last| {
                let distance = center_frame.abs_diff(last);
                distance >= refresh_frames || distance >= sample_rate.max(1) as usize / 4
            });
            if needs_update {
                fft_power_window_into(stereo_samples, center_frame, window);
            }
        }
    }

    fn ensure_windows(&mut self, sample_rate: u32, frame_count: usize) {
        if self.sample_rate == sample_rate
            && self.frame_count == frame_count
            && self.windows.len() == MULTI_RES_WINDOWS.len()
        {
            return;
        }

        self.sample_rate = sample_rate;
        self.frame_count = frame_count;
        self.windows.clear();
        let mut planner = FftPlanner::<f32>::new();
        for spec in MULTI_RES_WINDOWS {
            let size = spec.size.min(frame_count).max(1);
            let fft = planner.plan_fft_forward(size);
            let denom = (size.saturating_sub(1)).max(1) as f32;
            let mut hann = Vec::with_capacity(size);
            let mut hann_sum = 0.0_f32;
            for i in 0..size {
                let value = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / denom).cos();
                hann_sum += value;
                hann.push(value);
            }
            self.windows.push(SpectrumFftWindow {
                spec: MultiResolutionWindowSpec {
                    size,
                    upper_hz: spec.upper_hz,
                    refresh_secs: spec.refresh_secs,
                },
                fft,
                samples: vec![Complex32::new(0.0, 0.0); size],
                powers: vec![0.0; size / 2 + 1],
                hann,
                hann_sum,
                bin_hz: sample_rate.max(1) as f32 / size as f32,
                last_center_frame: None,
            });
        }
    }

    fn power_for_range(&self, center_hz: f32, low_hz: f32, high_hz: f32) -> f32 {
        let weights = self.window_weights(center_hz);
        weights
            .into_iter()
            .map(|(idx, weight)| {
                self.windows.get(idx).map_or(0.0, |window| {
                    weight * range_power_from_fft(&window.powers, window.bin_hz, low_hz, high_hz)
                })
            })
            .sum::<f32>()
            .max(0.0)
    }

    fn window_weights(&self, hz: f32) -> Vec<(usize, f32)> {
        let mut idx = 0;
        while idx + 1 < self.windows.len() && hz >= self.windows[idx].spec.upper_hz {
            idx += 1;
        }
        if idx > 0 {
            let boundary = self.windows[idx - 1].spec.upper_hz;
            if boundary.is_finite() {
                let distance = (hz.max(1.0).log2() - boundary.log2()) / SPECTRUM_BLEND_HALF_OCTAVES;
                if (-1.0..=1.0).contains(&distance) {
                    let upper = ((distance + 1.0) * 0.5).clamp(0.0, 1.0);
                    return vec![(idx - 1, 1.0 - upper), (idx, upper)];
                }
            }
        }
        if idx + 1 < self.windows.len() {
            let boundary = self.windows[idx].spec.upper_hz;
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

fn fft_power_window_into(
    stereo_samples: &[f32],
    center_frame: usize,
    window: &mut SpectrumFftWindow,
) {
    let frame_count = stereo_samples.len() / 2;
    let window_frames = window.samples.len();
    let max_start = frame_count.saturating_sub(window_frames);
    let start_frame = center_frame
        .saturating_sub(window_frames / 2)
        .min(max_start);
    for (i, sample) in window.samples.iter_mut().enumerate() {
        let frame_idx = start_frame + i;
        let l = stereo_samples[frame_idx * 2];
        let r = stereo_samples[frame_idx * 2 + 1];
        let mono = ((l + r) * 0.5).clamp(-1.0, 1.0);
        let hann = window.hann[i];
        sample.re = mono * hann;
        sample.im = 0.0;
    }
    window.fft.process(&mut window.samples);

    let half = window_frames / 2;
    let scale = window.hann_sum.max(1.0);
    for (idx, value) in window.samples.iter().take(half + 1).enumerate() {
        let mirror_scale = if idx == 0 || (window_frames % 2 == 0 && idx == half) {
            1.0
        } else {
            2.0
        };
        let amplitude = value.norm() * mirror_scale / scale;
        window.powers[idx] = amplitude * amplitude;
    }
    window.last_center_frame = Some(center_frame);
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

fn vocal_candidate_score(
    loudness_db: f32,
    band_energy: [f32; 3],
    zero_cross_rate: f32,
    crest: f32,
    transient: f32,
    mean_abs: f32,
    rms: f32,
) -> f32 {
    let loudness_gate = smoothstep(-48.0, -20.0, loudness_db);
    let mid_score = smoothstep(0.20, 0.58, band_energy[1]);
    let low_penalty = 1.0 - 0.65 * smoothstep(0.48, 0.78, band_energy[0]);
    let high_penalty = 1.0 - 0.65 * smoothstep(0.43, 0.72, band_energy[2]);
    let zcr_score = smoothstep(0.0035, 0.010, zero_cross_rate)
        * (1.0 - smoothstep(0.08, 0.16, zero_cross_rate));
    let abs_to_rms = mean_abs / rms.max(1.0e-6);
    let fullness =
        smoothstep(0.38, 0.64, abs_to_rms) * (1.0 - 0.35 * smoothstep(0.84, 0.96, abs_to_rms));
    let crest_penalty = 1.0 - 0.75 * smoothstep(5.5, 14.0, crest);
    let transient_penalty = 1.0 - 0.75 * smoothstep(0.28, 0.85, transient);

    let spectral = (mid_score * low_penalty * high_penalty).sqrt();
    let tonal = zcr_score * 0.64 + fullness * 0.36;
    (loudness_gate * spectral * tonal * crest_penalty * transient_penalty).clamp(0.0, 1.0)
}

fn apply_vocal_scores(bins: &mut [WaveformBin], raw: &[f32], bin_secs: f64) {
    if bins.is_empty() || raw.is_empty() {
        return;
    }
    let radius = (0.45 / bin_secs.max(0.01)).round().max(1.0) as usize;
    let mut smoothed = vec![0.0_f32; bins.len()];
    for i in 0..bins.len() {
        let start = i.saturating_sub(radius);
        let end = (i + radius + 1).min(raw.len());
        let mut score_sum = 0.0;
        let mut transient_sum = 0.0;
        let mut count = 0usize;
        for j in start..end {
            score_sum += raw[j];
            transient_sum += bins[j].transient;
            count += 1;
        }
        let count_f = count.max(1) as f32;
        let local_score = score_sum / count_f;
        let local_transient = transient_sum / count_f;
        let sustain_gate = smoothstep(0.10, 0.38, local_score);
        let transient_penalty = 1.0 - 0.55 * smoothstep(0.20, 0.75, local_transient);
        smoothed[i] =
            ((local_score * 0.78 + raw[i] * 0.22).powf(0.85) * sustain_gate * transient_penalty)
                .clamp(0.0, 1.0);
    }

    let mut state = 0.0_f32;
    for (bin, target) in bins.iter_mut().zip(smoothed) {
        if target > state {
            state = state * 0.78 + target * 0.22;
        } else {
            state = (state * 0.94).max(target * 0.55);
        }
        bin.vocal_score = state.clamp(0.0, 1.0);
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
    fn analysis_marks_sustained_midrange_as_vocal_candidate() {
        let mut samples = Vec::new();
        for i in 0..96_000 {
            let t = i as f32 / 48_000.0;
            let envelope = if t < 0.1 {
                t / 0.1
            } else if t > 1.9 {
                (2.0 - t) / 0.1
            } else {
                1.0
            }
            .clamp(0.0, 1.0);
            let x = envelope
                * 0.20
                * ((std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.55 * (std::f32::consts::TAU * 440.0 * t).sin()
                    + 0.35 * (std::f32::consts::TAU * 660.0 * t).sin()
                    + 0.18 * (std::f32::consts::TAU * 880.0 * t).sin());
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_vocal = analysis
            .bins
            .iter()
            .map(|b| b.vocal_score)
            .fold(0.0_f32, f32::max);

        assert!(max_vocal > 0.35, "max_vocal={max_vocal}");
    }

    #[test]
    fn analysis_keeps_percussive_bursts_low_vocal_score() {
        let mut samples = Vec::new();
        for i in 0..96_000 {
            let phase = i % 12_000;
            let x = if phase < 220 {
                let decay = 1.0 - phase as f32 / 220.0;
                0.9 * decay
            } else {
                0.0
            };
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_vocal = analysis
            .bins
            .iter()
            .map(|b| b.vocal_score)
            .fold(0.0_f32, f32::max);

        assert!(max_vocal < 0.18, "max_vocal={max_vocal}");
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
