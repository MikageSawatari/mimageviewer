use serde::{Deserialize, Serialize};

use crate::beat::BeatGrid;

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
            bin_secs: 0.10,
            row_secs: 60.0,
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
        });

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
}
