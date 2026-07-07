use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};

use crate::beat::BeatGrid;

pub const SPECTRUM_NOTE_MIN_MIDI: u8 = 21;
pub const SPECTRUM_NOTE_MAX_MIDI: u8 = 108;
pub const SPECTRUM_NOTE_BANDS: usize =
    (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
pub const SPECTRUM_BAND_MIN_MIDI: u8 = 16; // E0, the first semitone band above 20Hz.
pub const SPECTRUM_BAND_MAX_MIDI: u8 = 133; // C#10, whose upper edge covers 18kHz.
pub const SPECTRUM_BAND_COUNT: usize =
    (SPECTRUM_BAND_MAX_MIDI - SPECTRUM_BAND_MIN_MIDI + 1) as usize;
pub const TIMELINE_ANALYSIS_VERSION: u32 = 1;
const SPECTRUM_MIN_HZ: f32 = 20.0;
const SPECTRUM_DISPLAY_MAX_HZ: f32 = 18_000.0;
const SPECTRUM_DB_FLOOR: f32 = -72.0;
const SPECTRUM_DB_CEIL: f32 = -8.0;
const SPECTRUM_DB_GAMMA: f32 = 1.65;
const SPECTRUM_BLEND_HALF_OCTAVES: f32 = 0.14;
const SPECTRUM_LOW_SECONDARY_WINDOW_IDX: usize = 1;
const SPECTRUM_LOW_SECONDARY_FULL_HZ: f32 = 72.0;
const SPECTRUM_LOW_SECONDARY_FADE_END_HZ: f32 = 90.0;
const SPECTRUM_LOW_SECONDARY_MAX_MIX: f32 = 0.45;
const SPECTRUM_LOW_DISPLAY_ATTENUATION_DB: f32 = 5.5;
const SPECTRUM_SUB_DISPLAY_ATTENUATION_DB: f32 = 1.0;
const SPECTRUM_PRESENCE_DISPLAY_BOOST_DB: f32 = 1.25;
const VOCAL_PERIODICITY_TARGET_HZ: u32 = 8_000;
const TIMELINE_CHROMA_CLASSES: usize = 12;
const TIMELINE_CHROMA_WINDOW_SIZE: usize = 8_192;
const TIMELINE_CHROMA_HOP_SECS: f64 = 0.10;
const TIMELINE_BASS_MIN_HZ: f32 = 35.0;
const TIMELINE_BASS_MAX_HZ: f32 = 260.0;
const TIMELINE_CHROMA_MIN_HZ: f32 = 55.0;
const TIMELINE_CHROMA_MAX_HZ: f32 = 2_800.0;
const MULTI_RES_WINDOWS: [MultiResolutionWindowSpec; 5] = [
    MultiResolutionWindowSpec {
        size: 32_768,
        upper_hz: 90.0,
        refresh_secs: 0.025,
    },
    MultiResolutionWindowSpec {
        size: 16_384,
        upper_hz: 1_000.0,
        refresh_secs: 0.018,
    },
    MultiResolutionWindowSpec {
        size: 8_192,
        upper_hz: 2_000.0,
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
    /// 左チャンネルの peak / RMS。DJ タイムラインの上半分に描く (L/R ステレオ波形)。
    /// 既存 mono `peak` / `rms` は他メトリクス (band / chroma 等) が使い続けるので残す。
    #[serde(default)]
    pub peak_l: f32,
    #[serde(default)]
    pub rms_l: f32,
    /// 右チャンネルの peak / RMS。下半分に描く。
    #[serde(default)]
    pub peak_r: f32,
    #[serde(default)]
    pub rms_r: f32,
    pub loudness_db: f32,
    /// Normalized low / mid / high energy. Used for DJ-style color mapping.
    pub band_energy: [f32; 3],
    /// 0..1 estimate of sudden energy change for percussive visual accents.
    pub transient: f32,
    /// Normalized low / mid / high contribution to the transient accent.
    pub transient_band: [f32; 3],
    /// 0..1 stereo center dominance. Lead vocals are often mixed near center.
    pub center_ratio: f32,
    /// 0..1 lightweight vocal-likelihood estimate for timeline coloring.
    pub vocal_score: f32,
    /// 0..1 high-band presence normalized for section-change hints.
    #[serde(default)]
    pub brightness: f32,
    /// 0..1 moving density of short energy rises.
    #[serde(default)]
    pub transient_density: f32,
    /// 0..1 local feature change against neighboring time windows.
    #[serde(default)]
    pub novelty: f32,
    /// 0..11 strongest low-frequency pitch class, C=0.
    #[serde(default)]
    pub bass_pitch_class: u8,
    /// 0..1 confidence of the low-frequency pitch class.
    #[serde(default)]
    pub bass_pitch_confidence: f32,
    /// 0..11 strongest broadband chroma pitch class, C=0.
    #[serde(default)]
    pub key_pitch_class: u8,
    /// 0..1 confidence of the broadband chroma pitch class.
    #[serde(default)]
    pub key_confidence: f32,
    /// Low-frequency pitch-class energy, normalized to max=1.
    #[serde(default)]
    pub bass_chroma: [f32; TIMELINE_CHROMA_CLASSES],
    /// Broadband pitch-class energy, normalized to max=1.
    #[serde(default)]
    pub chroma: [f32; TIMELINE_CHROMA_CLASSES],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineAnalysis {
    #[serde(default)]
    pub analysis_version: u32,
    pub stream: AudioStreamInfo,
    pub config: AnalysisConfig,
    pub bins: Vec<WaveformBin>,
    pub beat_grid: BeatGrid,
}

impl Default for TimelineAnalysis {
    fn default() -> Self {
        Self {
            analysis_version: TIMELINE_ANALYSIS_VERSION,
            stream: AudioStreamInfo::default(),
            config: AnalysisConfig::default(),
            bins: Vec::new(),
            beat_grid: BeatGrid::empty(),
        }
    }
}

impl TimelineAnalysis {
    pub fn new(
        stream: AudioStreamInfo,
        config: AnalysisConfig,
        bins: Vec<WaveformBin>,
        beat_grid: BeatGrid,
    ) -> Self {
        Self {
            analysis_version: TIMELINE_ANALYSIS_VERSION,
            stream,
            config,
            bins,
            beat_grid,
        }
    }

    pub fn is_current_version(&self) -> bool {
        self.analysis_version == TIMELINE_ANALYSIS_VERSION
    }
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
    let vocal_period_step = (sample_rate / VOCAL_PERIODICITY_TARGET_HZ).max(1) as usize;
    let vocal_period_rate = sample_rate as f32 / vocal_period_step as f32;

    let mut frame_start = 0usize;
    while frame_start < frame_count {
        let frame_end = (frame_start + frames_per_bin).min(frame_count);
        let mut peak = 0.0_f32;
        let mut square_sum = 0.0_f64;
        // 左右チャンネル別の peak / RMS (L/R ステレオ波形表示用)。mono とは別に集計する。
        let mut peak_l = 0.0_f32;
        let mut peak_r = 0.0_f32;
        let mut square_sum_l = 0.0_f64;
        let mut square_sum_r = 0.0_f64;
        let mut side_square_sum = 0.0_f64;
        let mut low_sum = 0.0_f64;
        let mut mid_sum = 0.0_f64;
        let mut high_sum = 0.0_f64;
        let mut abs_sum = 0.0_f64;
        let mut crossings = 0usize;
        let mut prev_mono = 0.0_f32;
        let mut have_prev_mono = false;
        let mut periodicity_samples =
            Vec::with_capacity((frame_end - frame_start).div_ceil(vocal_period_step).max(1));

        for frame_idx in frame_start..frame_end {
            let l = stereo_samples[frame_idx * 2];
            let r = stereo_samples[frame_idx * 2 + 1];
            let mono = ((l + r) * 0.5).clamp(-1.0, 1.0);
            let side = ((l - r) * 0.5).clamp(-1.0, 1.0);
            if (frame_idx - frame_start) % vocal_period_step == 0 {
                periodicity_samples.push(mono);
            }
            let abs = mono.abs();
            peak = peak.max(abs);
            square_sum += (mono as f64) * (mono as f64);
            let lc = l.clamp(-1.0, 1.0);
            let rc = r.clamp(-1.0, 1.0);
            peak_l = peak_l.max(lc.abs());
            peak_r = peak_r.max(rc.abs());
            square_sum_l += (lc as f64) * (lc as f64);
            square_sum_r += (rc as f64) * (rc as f64);
            side_square_sum += (side as f64) * (side as f64);
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
        let rms_l = (square_sum_l / n).sqrt() as f32;
        let rms_r = (square_sum_r / n).sqrt() as f32;
        let side_rms = (side_square_sum / n).sqrt() as f32;
        let center_ratio = (rms / (rms + side_rms + 1.0e-6)).clamp(0.0, 1.0);
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
        let periodicity = voiced_periodicity_score(&periodicity_samples, vocal_period_rate);
        let vocal_candidate = vocal_candidate_score(
            loudness_db,
            band_energy,
            zero_cross_rate,
            crest,
            transient,
            mean_abs,
            rms,
            periodicity,
            center_ratio,
        );
        vocal_raw.push(vocal_candidate);
        bins.push(WaveformBin {
            start_secs: frame_start as f64 / sample_rate as f64,
            duration_secs: (frame_end - frame_start) as f64 / sample_rate as f64,
            peak,
            rms,
            peak_l,
            rms_l,
            peak_r,
            rms_r,
            loudness_db,
            band_energy,
            transient,
            transient_band,
            center_ratio,
            vocal_score: 0.0,
            brightness: 0.0,
            transient_density: 0.0,
            novelty: 0.0,
            bass_pitch_class: 0,
            bass_pitch_confidence: 0.0,
            key_pitch_class: 0,
            key_confidence: 0.0,
            bass_chroma: [0.0; TIMELINE_CHROMA_CLASSES],
            chroma: [0.0; TIMELINE_CHROMA_CLASSES],
        });
        prev_band_rms = [
            prev_band_rms[0] * 0.68 + band_rms[0] * 0.32,
            prev_band_rms[1] * 0.68 + band_rms[1] * 0.32,
            prev_band_rms[2] * 0.68 + band_rms[2] * 0.32,
        ];

        frame_start = frame_end;
    }
    apply_vocal_scores(&mut bins, &vocal_raw, config.bin_secs);
    apply_timeline_summary_metrics(&mut bins, config.bin_secs);
    apply_chroma_metrics(&mut bins, stereo_samples, sample_rate, config.bin_secs);
    apply_novelty_scores(&mut bins, config.bin_secs);

    let beat_grid = estimate_simple_beat_grid(&bins, duration_secs, config.bin_secs);

    TimelineAnalysis::new(
        AudioStreamInfo {
            sample_rate,
            channels: 2,
            duration_secs,
        },
        config,
        bins,
        beat_grid,
    )
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
                notes: vec![0.0; SPECTRUM_NOTE_BANDS],
                note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            };
        }

        self.update_windows(stereo_samples, sample_rate, center_secs);
        let max_hz = spectrum_max_hz(sample_rate);

        let mut values = Vec::with_capacity(bands);
        for band in 0..bands {
            if let Some(midi) = spectrum_band_midi(band, bands) {
                let hz = midi_to_hz(midi);
                let (low_hz, high_hz) = midi_note_hz_range(midi);
                if high_hz < SPECTRUM_MIN_HZ || low_hz > max_hz {
                    values.push(0.0);
                } else {
                    let power = self.power_for_range(hz, low_hz.max(10.0), high_hz.min(max_hz));
                    values.push(power_to_display_value(
                        power,
                        spectrum_display_weight_db(hz),
                    ));
                }
            } else {
                values.push(0.0);
            }
        }

        let mut notes = Vec::with_capacity(SPECTRUM_NOTE_BANDS);
        for midi in SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI {
            let hz = midi_to_hz(midi);
            let (low_hz, high_hz) = midi_note_hz_range(midi);
            if high_hz < SPECTRUM_MIN_HZ || low_hz > max_hz {
                notes.push(0.0);
            } else {
                let power = self.power_for_range(hz, low_hz.max(10.0), high_hz.min(max_hz));
                // Layer ① 知覚補正: 鍵盤 notes にも棒グラフ同等の等ラウドネス dB カーブを掛け、
                // 低域偏重を解消してボーカル帯を見えやすくする (棒グラフだけ重み付けだった非対称を是正)。
                notes.push(power_to_display_value(
                    power,
                    spectrum_display_weight_db(hz),
                ));
            }
        }

        SpectrumAnalysis {
            bands: values,
            notes,
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
        }
    }

    /// Analyze a short sliding sample window whose contents can change even when
    /// its length and local center position stay the same.
    pub fn analyze_moving_window(
        &mut self,
        stereo_samples: &[f32],
        sample_rate: u32,
        center_secs: f64,
    ) -> SpectrumAnalysis {
        self.invalidate_cached_windows();
        self.analyze(stereo_samples, sample_rate, center_secs)
    }

    fn invalidate_cached_windows(&mut self) {
        for window in &mut self.windows {
            window.last_center_frame = None;
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
        let base = weights
            .into_iter()
            .map(|(idx, weight)| {
                self.windows.get(idx).map_or(0.0, |window| {
                    weight * range_power_from_fft(&window.powers, window.bin_hz, low_hz, high_hz)
                })
            })
            .sum::<f32>()
            .max(0.0);

        let secondary_mix = low_secondary_mix(center_hz);
        if secondary_mix <= 0.0 {
            return base;
        }

        let secondary = self
            .windows
            .get(SPECTRUM_LOW_SECONDARY_WINDOW_IDX)
            .map(|window| range_power_from_fft(&window.powers, window.bin_hz, low_hz, high_hz))
            .unwrap_or(base);
        (base * (1.0 - secondary_mix) + secondary * secondary_mix).max(0.0)
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

fn low_secondary_mix(hz: f32) -> f32 {
    if hz <= SPECTRUM_LOW_SECONDARY_FULL_HZ {
        SPECTRUM_LOW_SECONDARY_MAX_MIX
    } else if hz >= SPECTRUM_LOW_SECONDARY_FADE_END_HZ {
        0.0
    } else {
        let t = (hz - SPECTRUM_LOW_SECONDARY_FULL_HZ)
            / (SPECTRUM_LOW_SECONDARY_FADE_END_HZ - SPECTRUM_LOW_SECONDARY_FULL_HZ);
        SPECTRUM_LOW_SECONDARY_MAX_MIX * (1.0 - t.clamp(0.0, 1.0))
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
    let start = ((low_hz / bin_hz).ceil() as usize).max(1);
    let end = ((high_hz / bin_hz).floor() as usize).min(powers.len().saturating_sub(1));
    if end < start {
        let center_hz = (low_hz + high_hz) * 0.5;
        let idx = ((center_hz.max(0.0) / bin_hz).round() as usize).min(powers.len() - 1);
        return powers[idx];
    }
    let slice = &powers[start..=end];
    let sum = slice.iter().copied().sum::<f32>();
    let peak = slice.iter().copied().fold(0.0_f32, f32::max);
    let avg = sum / slice.len().max(1) as f32;
    avg.max(peak * 0.35)
}

fn power_to_display_value(power: f32, display_gain_db: f32) -> f32 {
    let db = 10.0 * power.max(1.0e-12).log10() + display_gain_db;
    ((db - SPECTRUM_DB_FLOOR) / (SPECTRUM_DB_CEIL - SPECTRUM_DB_FLOOR))
        .clamp(0.0, 1.0)
        .powf(SPECTRUM_DB_GAMMA)
}

fn spectrum_display_weight_db(hz: f32) -> f32 {
    let low_attenuation =
        -SPECTRUM_LOW_DISPLAY_ATTENUATION_DB * (1.0 - smoothstep_log_hz(35.0, 130.0, hz));
    let sub_attenuation =
        -SPECTRUM_SUB_DISPLAY_ATTENUATION_DB * (1.0 - smoothstep_log_hz(20.0, 35.0, hz));
    let presence_boost = SPECTRUM_PRESENCE_DISPLAY_BOOST_DB
        * smoothstep_log_hz(450.0, 1_600.0, hz)
        * (1.0 - smoothstep_log_hz(4_200.0, 8_500.0, hz));
    let air_rolloff = -1.25 * smoothstep_log_hz(12_000.0, 18_000.0, hz);
    low_attenuation + sub_attenuation + presence_boost + air_rolloff
}

fn smoothstep_log_hz(edge0: f32, edge1: f32, hz: f32) -> f32 {
    smoothstep(
        edge0.max(1.0).log2(),
        edge1.max(1.0).log2(),
        hz.max(1.0).log2(),
    )
}

fn spectrum_max_hz(sample_rate: u32) -> f32 {
    let nyquist = sample_rate.max(1) as f32 * 0.5;
    SPECTRUM_DISPLAY_MAX_HZ
        .min(nyquist * 0.92)
        .max(SPECTRUM_MIN_HZ * 1.2)
}

fn spectrum_band_midi(index: usize, total: usize) -> Option<u8> {
    if total == 0 || index >= total {
        return None;
    }
    let midi = SPECTRUM_BAND_MIN_MIDI as usize + index;
    (midi <= SPECTRUM_BAND_MAX_MIDI as usize).then_some(midi as u8)
}

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

fn midi_note_hz_range(midi: u8) -> (f32, f32) {
    let hz = midi_to_hz(midi);
    let half_step = 2.0_f32.powf(1.0 / 24.0);
    (hz / half_step, hz * half_step)
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
    periodicity: f32,
    center_ratio: f32,
) -> f32 {
    let loudness_gate = smoothstep(-50.0, -20.0, loudness_db);
    let mid_score = smoothstep(0.14, 0.50, band_energy[1]);
    let low_penalty = 1.0 - 0.55 * smoothstep(0.55, 0.84, band_energy[0]);
    let high_penalty = 1.0 - 0.48 * smoothstep(0.56, 0.86, band_energy[2]);
    let zcr_score = smoothstep(0.0025, 0.0090, zero_cross_rate)
        * (1.0 - smoothstep(0.08, 0.16, zero_cross_rate));
    let abs_to_rms = mean_abs / rms.max(1.0e-6);
    let fullness =
        smoothstep(0.38, 0.64, abs_to_rms) * (1.0 - 0.35 * smoothstep(0.84, 0.96, abs_to_rms));
    let crest_penalty = 1.0 - 0.55 * smoothstep(7.0, 16.0, crest);
    let transient_penalty = 1.0 - 0.55 * smoothstep(0.36, 0.92, transient);
    let periodicity_gate = 0.34 + 0.66 * smoothstep(0.12, 0.52, periodicity);
    let center_gate = 0.48 + 0.52 * smoothstep(0.50, 0.82, center_ratio);

    let spectral = (mid_score * low_penalty * high_penalty).sqrt();
    let tonal = zcr_score * 0.64 + fullness * 0.36;
    (loudness_gate
        * spectral
        * tonal
        * periodicity_gate
        * center_gate
        * crest_penalty
        * transient_penalty)
        .clamp(0.0, 1.0)
}

fn voiced_periodicity_score(samples: &[f32], sample_rate: f32) -> f32 {
    if samples.len() < 48 || sample_rate <= 0.0 {
        return 0.0;
    }
    let mean = samples.iter().copied().sum::<f32>() / samples.len() as f32;
    let energy = samples
        .iter()
        .map(|sample| {
            let centered = *sample - mean;
            centered * centered
        })
        .sum::<f32>();
    if energy <= 1.0e-8 {
        return 0.0;
    }

    let min_lag = (sample_rate / 360.0).round().max(2.0) as usize;
    let max_lag = (sample_rate / 80.0).round().max(min_lag as f32 + 1.0) as usize;
    let max_lag = max_lag.min(samples.len().saturating_sub(8));
    if max_lag <= min_lag {
        return 0.0;
    }

    let mut best = 0.0_f32;
    for lag in min_lag..=max_lag {
        let mut corr = 0.0;
        let mut a_energy = 0.0;
        let mut b_energy = 0.0;
        for i in lag..samples.len() {
            let a = samples[i] - mean;
            let b = samples[i - lag] - mean;
            corr += a * b;
            a_energy += a * a;
            b_energy += b * b;
        }
        let norm = (a_energy * b_energy).sqrt();
        if norm > 1.0e-8 {
            best = best.max((corr / norm).max(0.0));
        }
    }

    best.clamp(0.0, 1.0)
}

fn vocal_phrase_hint_score(bin: &WaveformBin) -> f32 {
    let loudness = smoothstep(-48.0, -18.0, bin.loudness_db);
    let mid = smoothstep(0.18, 0.52, bin.band_energy[1]);
    let air = smoothstep(0.22, 0.64, bin.band_energy[2]);
    let voice_band = (mid * 0.86 + air * 0.20).min(1.0);
    let low_penalty = 1.0 - 0.68 * smoothstep(0.62, 0.90, bin.band_energy[0]);
    let high_penalty = 1.0 - 0.48 * smoothstep(0.74, 0.96, bin.band_energy[2]);
    let transient_penalty = 1.0 - 0.48 * smoothstep(0.38, 0.90, bin.transient);
    let center_gate = smoothstep(0.46, 0.82, bin.center_ratio);
    (loudness * voice_band * low_penalty * high_penalty * transient_penalty * center_gate)
        .clamp(0.0, 1.0)
}

fn apply_vocal_scores(bins: &mut [WaveformBin], raw: &[f32], bin_secs: f64) {
    if bins.is_empty() || raw.is_empty() {
        return;
    }
    let radius = (1.10 / bin_secs.max(0.01)).round().max(1.0) as usize;
    let phrase_radius = (1.85 / bin_secs.max(0.01)).round().max(radius as f64) as usize;
    let phrase_hint: Vec<f32> = bins.iter().map(vocal_phrase_hint_score).collect();
    let mut smoothed = vec![0.0_f32; bins.len()];
    for i in 0..bins.len() {
        let start = i.saturating_sub(radius);
        let end = (i + radius + 1).min(raw.len());
        let mut score_sum = 0.0;
        let mut transient_sum = 0.0;
        let mut active_count = 0usize;
        let mut strong_count = 0usize;
        let mut count = 0usize;
        for j in start..end {
            score_sum += raw[j];
            transient_sum += bins[j].transient;
            if raw[j] > 0.16 {
                active_count += 1;
            }
            if raw[j] > 0.34 {
                strong_count += 1;
            }
            count += 1;
        }
        let count_f = count.max(1) as f32;
        let local_score = score_sum / count_f;
        let local_transient = transient_sum / count_f;
        let active_ratio = active_count as f32 / count_f;
        let strong_ratio = strong_count as f32 / count_f;
        let sustain_gate = smoothstep(0.24, 0.62, active_ratio)
            * (0.45 + 0.55 * smoothstep(0.05, 0.28, strong_ratio));
        let score_gate = smoothstep(0.17, 0.42, local_score);
        let transient_penalty = 1.0 - 0.62 * smoothstep(0.20, 0.68, local_transient);
        let raw_score = (local_score.powf(0.85) * sustain_gate * score_gate * transient_penalty)
            .clamp(0.0, 1.0);

        let phrase_start = i.saturating_sub(phrase_radius);
        let phrase_end = (i + phrase_radius + 1).min(phrase_hint.len());
        let mut phrase_sum = 0.0;
        let mut phrase_active = 0usize;
        let mut phrase_strong = 0usize;
        let mut phrase_count = 0usize;
        for score in &phrase_hint[phrase_start..phrase_end] {
            phrase_sum += *score;
            if *score > 0.16 {
                phrase_active += 1;
            }
            if *score > 0.34 {
                phrase_strong += 1;
            }
            phrase_count += 1;
        }
        let phrase_count_f = phrase_count.max(1) as f32;
        let local_phrase = phrase_sum / phrase_count_f;
        let phrase_active_ratio = phrase_active as f32 / phrase_count_f;
        let phrase_strong_ratio = phrase_strong as f32 / phrase_count_f;
        let raw_support = smoothstep(0.018, 0.11, local_score)
            * (0.48 + 0.52 * smoothstep(0.03, 0.18, active_ratio));
        let phrase_sustain = smoothstep(0.32, 0.72, phrase_active_ratio)
            * (0.28 + 0.72 * smoothstep(0.05, 0.26, phrase_strong_ratio))
            * (0.35 + 0.65 * raw_support);
        let phrase_score =
            (local_phrase.powf(0.88) * phrase_sustain * transient_penalty).clamp(0.0, 1.0);

        smoothed[i] = raw_score.max(phrase_score * 0.56).clamp(0.0, 1.0);
    }

    keep_only_vocal_like_segments(&mut smoothed, bin_secs);

    let mut state = 0.0_f32;
    for (bin, target) in bins.iter_mut().zip(smoothed) {
        if target > state {
            state = state * 0.82 + target * 0.18;
        } else {
            let release = if target < 0.05 { 0.94 } else { 0.965 };
            state = (state * release).max(target * 0.55);
        }
        bin.vocal_score = state.clamp(0.0, 1.0);
    }
}

fn keep_only_vocal_like_segments(scores: &mut [f32], bin_secs: f64) {
    if scores.is_empty() {
        return;
    }
    let min_segment_bins = (1.80 / bin_secs.max(0.01)).round().max(1.0) as usize;
    let bridge_bins = (0.85 / bin_secs.max(0.01)).round().max(1.0) as usize;

    let mut i = 0usize;
    while i < scores.len() {
        if scores[i] > 0.12 {
            i += 1;
            continue;
        }
        let gap_start = i;
        while i < scores.len() && scores[i] <= 0.12 {
            i += 1;
        }
        let gap_end = i;
        if gap_start > 0 && gap_end < scores.len() && gap_end - gap_start <= bridge_bins {
            let bridge_score =
                (scores[gap_start - 1].min(scores[gap_end]) * 0.72).clamp(0.22, 0.42);
            for score in &mut scores[gap_start..gap_end] {
                *score = bridge_score;
            }
        }
    }

    let mut start = None;
    for idx in 0..=scores.len() {
        let active = idx < scores.len() && scores[idx] > 0.12;
        match (start, active) {
            (None, true) => start = Some(idx),
            (Some(segment_start), false) => {
                let segment_end = idx;
                let segment_len = segment_end - segment_start;
                let peak = scores[segment_start..segment_end]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max);
                if segment_len < min_segment_bins || peak < 0.22 {
                    for score in &mut scores[segment_start..segment_end] {
                        *score = 0.0;
                    }
                }
                start = None;
            }
            _ => {}
        }
    }

    for score in scores {
        *score = smoothstep(0.18, 0.55, *score);
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn normalize_range(value: f32, low: f32, high: f32) -> f32 {
    if high <= low {
        return if value >= high { 1.0 } else { 0.0 };
    }
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn apply_timeline_summary_metrics(bins: &mut [WaveformBin], bin_secs: f64) {
    if bins.is_empty() {
        return;
    }
    let density_radius = (0.70 / bin_secs.max(0.01)).round().max(1.0) as usize;
    let brightness_radius = (1.20 / bin_secs.max(0.01)).round().max(1.0) as usize;
    let high_mid: Vec<f32> = bins
        .iter()
        .map(|bin| {
            let high = bin.band_energy[2].max(0.0);
            let mid = bin.band_energy[1].max(0.0);
            high / (mid + high + 1.0e-6)
        })
        .collect();

    for i in 0..bins.len() {
        let density_start = i.saturating_sub(density_radius);
        let density_end = (i + density_radius + 1).min(bins.len());
        let mut transient_sum = 0.0;
        let mut transient_active = 0usize;
        for bin in &bins[density_start..density_end] {
            transient_sum += bin.transient;
            if bin.transient > 0.10 {
                transient_active += 1;
            }
        }
        let density_count = (density_end - density_start).max(1) as f32;
        let transient_mean = transient_sum / density_count;
        let transient_ratio = transient_active as f32 / density_count;
        bins[i].transient_density = (smoothstep(0.025, 0.18, transient_mean) * 0.68
            + smoothstep(0.04, 0.20, transient_ratio) * 0.42)
            .clamp(0.0, 1.0);

        let bright_start = i.saturating_sub(brightness_radius);
        let bright_end = (i + brightness_radius + 1).min(bins.len());
        let local_mean = high_mid[bright_start..bright_end]
            .iter()
            .copied()
            .sum::<f32>()
            / (bright_end - bright_start).max(1) as f32;
        let absolute = normalize_range(high_mid[i], 0.035, 0.34);
        let lift = smoothstep(0.018, 0.13, high_mid[i] - local_mean);
        bins[i].brightness = (absolute * 0.74 + lift * 0.38).clamp(0.0, 1.0);
    }
}

fn apply_chroma_metrics(
    bins: &mut [WaveformBin],
    stereo_samples: &[f32],
    sample_rate: u32,
    bin_secs: f64,
) {
    let frame_count = stereo_samples.len() / 2;
    if bins.is_empty() || sample_rate == 0 || frame_count == 0 {
        return;
    }
    let fft_size = TIMELINE_CHROMA_WINDOW_SIZE.min(frame_count).max(64);
    let hop_frames = (TIMELINE_CHROMA_HOP_SECS * sample_rate as f64)
        .round()
        .max(1.0) as usize;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let denom = (fft_size.saturating_sub(1)).max(1) as f32;
    let mut hann = Vec::with_capacity(fft_size);
    let mut hann_sum = 0.0_f32;
    for i in 0..fft_size {
        let value = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / denom).cos();
        hann_sum += value;
        hann.push(value);
    }
    let mut samples = vec![Complex32::new(0.0, 0.0); fft_size];
    let mut frames = Vec::new();
    let mut center_frame = 0usize;
    while center_frame < frame_count {
        frames.push(analyze_chroma_frame(
            stereo_samples,
            center_frame,
            sample_rate,
            &fft,
            &hann,
            hann_sum,
            &mut samples,
        ));
        center_frame = center_frame.saturating_add(hop_frames);
    }
    if frames.is_empty() {
        return;
    }

    for bin in &mut *bins {
        let center_secs = bin.start_secs + bin.duration_secs * 0.5;
        let frame_idx = (center_secs / TIMELINE_CHROMA_HOP_SECS).round().max(0.0) as usize;
        let frame = &frames[frame_idx.min(frames.len() - 1)];
        bin.bass_chroma = frame.bass_chroma;
        bin.chroma = frame.chroma;
        bin.bass_pitch_class = frame.bass_pitch_class;
        let bass_loudness_gate = normalize_range(bin.rms, 0.004, 0.035);
        bin.bass_pitch_confidence = (frame.bass_confidence
            * normalize_range(bin.band_energy[0], 0.05, 0.45)
            * bass_loudness_gate)
            .clamp(0.0, 1.0);
        bin.key_pitch_class = frame.key_pitch_class;
        bin.key_confidence =
            (frame.key_confidence * normalize_range(bin.rms, 0.006, 0.06)).clamp(0.0, 1.0);
    }

    let smooth_radius = (0.42 / bin_secs.max(0.01)).round().max(1.0) as usize;
    smooth_pitch_confidence(bins, smooth_radius);
}

#[derive(Clone, Copy, Debug)]
struct TimelineChromaFrame {
    bass_chroma: [f32; TIMELINE_CHROMA_CLASSES],
    chroma: [f32; TIMELINE_CHROMA_CLASSES],
    bass_pitch_class: u8,
    bass_confidence: f32,
    key_pitch_class: u8,
    key_confidence: f32,
}

fn analyze_chroma_frame(
    stereo_samples: &[f32],
    center_frame: usize,
    sample_rate: u32,
    fft: &Arc<dyn Fft<f32>>,
    hann: &[f32],
    hann_sum: f32,
    samples: &mut [Complex32],
) -> TimelineChromaFrame {
    let frame_count = stereo_samples.len() / 2;
    let window_frames = samples.len();
    let max_start = frame_count.saturating_sub(window_frames);
    let start_frame = center_frame
        .saturating_sub(window_frames / 2)
        .min(max_start);
    for (i, sample) in samples.iter_mut().enumerate() {
        let frame_idx = start_frame + i;
        let mono = if frame_idx < frame_count {
            ((stereo_samples[frame_idx * 2] + stereo_samples[frame_idx * 2 + 1]) * 0.5)
                .clamp(-1.0, 1.0)
        } else {
            0.0
        };
        sample.re = mono * hann.get(i).copied().unwrap_or(0.0);
        sample.im = 0.0;
    }
    fft.process(samples);

    let half = window_frames / 2;
    let bin_hz = sample_rate.max(1) as f32 / window_frames as f32;
    let scale = hann_sum.max(1.0);
    let mut bass_chroma = [0.0_f32; TIMELINE_CHROMA_CLASSES];
    let mut chroma = [0.0_f32; TIMELINE_CHROMA_CLASSES];
    for idx in 1..=half {
        let hz = idx as f32 * bin_hz;
        if hz > TIMELINE_CHROMA_MAX_HZ {
            break;
        }
        let mirror_scale = if idx == half && window_frames % 2 == 0 {
            1.0
        } else {
            2.0
        };
        let amp = samples[idx].norm() * mirror_scale / scale;
        let compressed = amp.max(0.0).powf(0.62);
        if hz >= TIMELINE_BASS_MIN_HZ && hz <= TIMELINE_BASS_MAX_HZ {
            let pc = pitch_class_for_hz(hz);
            let focus = 1.0 - smoothstep(210.0, TIMELINE_BASS_MAX_HZ, hz) * 0.35;
            bass_chroma[pc] += compressed * focus;
        }
        if hz >= TIMELINE_CHROMA_MIN_HZ {
            let pc = pitch_class_for_hz(hz);
            let mid_focus = (1.0 - smoothstep(1_800.0, TIMELINE_CHROMA_MAX_HZ, hz) * 0.45)
                * (0.75 + smoothstep(80.0, 180.0, hz) * 0.25);
            chroma[pc] += compressed * mid_focus;
        }
    }

    let (bass_chroma, bass_pitch_class, bass_confidence) = normalize_chroma(bass_chroma);
    let (chroma, key_pitch_class, key_confidence) = normalize_chroma(chroma);
    TimelineChromaFrame {
        bass_chroma,
        chroma,
        bass_pitch_class,
        bass_confidence,
        key_pitch_class,
        key_confidence,
    }
}

fn pitch_class_for_hz(hz: f32) -> usize {
    let midi = (69.0 + 12.0 * (hz.max(1.0) / 440.0).log2()).round() as i32;
    midi.rem_euclid(TIMELINE_CHROMA_CLASSES as i32) as usize
}

fn normalize_chroma(
    values: [f32; TIMELINE_CHROMA_CLASSES],
) -> ([f32; TIMELINE_CHROMA_CLASSES], u8, f32) {
    let mut peak_idx = 0usize;
    let mut peak = 0.0_f32;
    let mut second = 0.0_f32;
    let mut sum = 0.0_f32;
    for (idx, value) in values.iter().copied().enumerate() {
        sum += value;
        if value > peak {
            second = peak;
            peak = value;
            peak_idx = idx;
        } else if value > second {
            second = value;
        }
    }
    if peak <= 1.0e-8 || sum <= 1.0e-8 {
        return ([0.0; TIMELINE_CHROMA_CLASSES], 0, 0.0);
    }
    let mut normalized = values;
    for value in &mut normalized {
        *value = (*value / peak).clamp(0.0, 1.0);
    }
    let peakiness = ((peak - second) / peak.max(1.0e-8)).clamp(0.0, 1.0);
    let concentration =
        (peak / (sum / TIMELINE_CHROMA_CLASSES as f32 + 1.0e-8)).clamp(0.0, 3.0) / 3.0;
    let confidence = (peakiness * 0.72 + concentration * 0.28).clamp(0.0, 1.0);
    (normalized, peak_idx as u8, confidence)
}

fn smooth_pitch_confidence(bins: &mut [WaveformBin], radius: usize) {
    if bins.is_empty() || radius == 0 {
        return;
    }
    let original: Vec<(u8, f32, u8, f32)> = bins
        .iter()
        .map(|bin| {
            (
                bin.bass_pitch_class,
                bin.bass_pitch_confidence,
                bin.key_pitch_class,
                bin.key_confidence,
            )
        })
        .collect();
    for i in 0..bins.len() {
        let start = i.saturating_sub(radius);
        let end = (i + radius + 1).min(bins.len());
        let mut bass_votes = [0.0_f32; TIMELINE_CHROMA_CLASSES];
        let mut key_votes = [0.0_f32; TIMELINE_CHROMA_CLASSES];
        for &(bass_pc, bass_conf, key_pc, key_conf) in &original[start..end] {
            bass_votes[bass_pc as usize % TIMELINE_CHROMA_CLASSES] += bass_conf;
            key_votes[key_pc as usize % TIMELINE_CHROMA_CLASSES] += key_conf;
        }
        let (bass_pc, bass_conf) = best_vote(&bass_votes);
        let (key_pc, key_conf) = best_vote(&key_votes);
        let bass_loudness_gate = normalize_range(bins[i].rms, 0.004, 0.035);
        let key_loudness_gate = normalize_range(bins[i].rms, 0.006, 0.06);
        bins[i].bass_pitch_class = bass_pc;
        bins[i].bass_pitch_confidence = bins[i]
            .bass_pitch_confidence
            .max(bass_conf * 0.55 * bass_loudness_gate);
        bins[i].key_pitch_class = key_pc;
        bins[i].key_confidence = bins[i]
            .key_confidence
            .max(key_conf * 0.50 * key_loudness_gate);
    }
}

fn best_vote(votes: &[f32; TIMELINE_CHROMA_CLASSES]) -> (u8, f32) {
    let mut best_idx = 0usize;
    let mut best = 0.0_f32;
    let mut sum = 0.0_f32;
    for (idx, value) in votes.iter().copied().enumerate() {
        sum += value;
        if value > best {
            best = value;
            best_idx = idx;
        }
    }
    let confidence = if sum > 1.0e-6 { best / sum } else { 0.0 };
    (best_idx as u8, confidence.clamp(0.0, 1.0))
}

fn apply_novelty_scores(bins: &mut [WaveformBin], bin_secs: f64) {
    if bins.len() < 4 {
        return;
    }
    let radius = (1.20 / bin_secs.max(0.01)).round().max(2.0) as usize;
    let mut raw = vec![0.0_f32; bins.len()];
    for i in 0..bins.len() {
        let prev_start = i.saturating_sub(radius);
        let prev_end = i;
        let next_start = i;
        let next_end = (i + radius).min(bins.len());
        if prev_end <= prev_start || next_end <= next_start {
            continue;
        }
        let prev = average_feature_window(&bins[prev_start..prev_end]);
        let next = average_feature_window(&bins[next_start..next_end]);
        let chroma_delta = l1_delta(&prev.chroma, &next.chroma) * 0.20;
        let bass_delta = l1_delta(&prev.bass_chroma, &next.bass_chroma) * 0.12;
        let scalar_delta = (prev.loudness - next.loudness).abs() * 0.60
            + (prev.bass - next.bass).abs() * 0.42
            + (prev.brightness - next.brightness).abs() * 0.72
            + (prev.transient_density - next.transient_density).abs() * 0.70
            + (prev.vocal - next.vocal).abs() * 0.38;
        raw[i] = smoothstep(0.12, 0.56, scalar_delta + chroma_delta + bass_delta);
    }
    let smooth_radius = (0.12 / bin_secs.max(0.01)).round().max(1.0) as usize;
    for i in 0..bins.len() {
        let start = i.saturating_sub(smooth_radius);
        let end = (i + smooth_radius + 1).min(bins.len());
        let peak = raw[start..end].iter().copied().fold(0.0_f32, f32::max);
        bins[i].novelty = peak.clamp(0.0, 1.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct TimelineFeatureAverage {
    loudness: f32,
    bass: f32,
    brightness: f32,
    transient_density: f32,
    vocal: f32,
    bass_chroma: [f32; TIMELINE_CHROMA_CLASSES],
    chroma: [f32; TIMELINE_CHROMA_CLASSES],
}

fn average_feature_window(window: &[WaveformBin]) -> TimelineFeatureAverage {
    let count = window.len().max(1) as f32;
    let mut avg = TimelineFeatureAverage {
        loudness: 0.0,
        bass: 0.0,
        brightness: 0.0,
        transient_density: 0.0,
        vocal: 0.0,
        bass_chroma: [0.0; TIMELINE_CHROMA_CLASSES],
        chroma: [0.0; TIMELINE_CHROMA_CLASSES],
    };
    for bin in window {
        avg.loudness += normalize_range(bin.loudness_db, -52.0, 0.0);
        avg.bass += bin.band_energy[0].clamp(0.0, 1.0);
        avg.brightness += bin.brightness;
        avg.transient_density += bin.transient_density;
        avg.vocal += bin.vocal_score;
        for pc in 0..TIMELINE_CHROMA_CLASSES {
            avg.bass_chroma[pc] += bin.bass_chroma[pc] * bin.bass_pitch_confidence;
            avg.chroma[pc] += bin.chroma[pc] * bin.key_confidence;
        }
    }
    avg.loudness /= count;
    avg.bass /= count;
    avg.brightness /= count;
    avg.transient_density /= count;
    avg.vocal /= count;
    for pc in 0..TIMELINE_CHROMA_CLASSES {
        avg.bass_chroma[pc] /= count;
        avg.chroma[pc] /= count;
    }
    avg
}

fn l1_delta(a: &[f32; TIMELINE_CHROMA_CLASSES], b: &[f32; TIMELINE_CHROMA_CLASSES]) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f32>() / TIMELINE_CHROMA_CLASSES as f32
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
        assert_eq!(analysis.analysis_version, TIMELINE_ANALYSIS_VERSION);
        assert!(analysis.is_current_version());
        assert!(!analysis.bins.is_empty());
        assert!(analysis.bins.iter().any(|b| b.peak > 0.5));
    }

    #[test]
    fn analysis_separates_left_right_channels() {
        // 左に強い信号、右は無音。per-channel の peak/rms が分離され、mono はその平均になる。
        let mut samples = Vec::new();
        for _ in 0..48_000 {
            samples.extend([0.8_f32, 0.0_f32]); // L=0.8, R=0.0
        }
        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let bin = analysis
            .bins
            .iter()
            .find(|b| b.peak_l > 0.1)
            .expect("a bin with left-channel energy");
        assert!(bin.peak_l > 0.7, "peak_l={}", bin.peak_l);
        assert!(bin.peak_r < 0.01, "peak_r={}", bin.peak_r);
        assert!(bin.rms_l > 0.7, "rms_l={}", bin.rms_l);
        assert!(bin.rms_r < 0.01, "rms_r={}", bin.rms_r);
        // mono は (L+R)/2 = 0.4 相当。
        assert!(bin.peak > 0.3 && bin.peak < 0.5, "mono peak={}", bin.peak);
    }

    #[test]
    fn timeline_analysis_default_uses_current_version() {
        let analysis = TimelineAnalysis::default();

        assert_eq!(analysis.analysis_version, TIMELINE_ANALYSIS_VERSION);
        assert!(analysis.is_current_version());
        assert_eq!(analysis.beat_grid.time_signature_numerator, 4);
    }

    #[test]
    fn timeline_analysis_detects_stale_version() {
        let analysis = TimelineAnalysis {
            analysis_version: TIMELINE_ANALYSIS_VERSION.saturating_sub(1),
            ..TimelineAnalysis::default()
        };

        assert!(!analysis.is_current_version());
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
    fn analysis_summarizes_transient_density() {
        let mut samples = Vec::new();
        for i in 0..96_000 {
            let phase = i % 6_000;
            let x = if phase < 140 {
                let decay = 1.0 - phase as f32 / 140.0;
                0.8 * decay
            } else {
                0.0
            };
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_density = analysis
            .bins
            .iter()
            .map(|b| b.transient_density)
            .fold(0.0_f32, f32::max);

        assert!(max_density > 0.18, "max_density={max_density}");
    }

    #[test]
    fn analysis_estimates_bass_pitch_class() {
        let mut samples = Vec::new();
        for i in 0..96_000 {
            let t = i as f32 / 48_000.0;
            let x = (std::f32::consts::TAU * 110.0 * t).sin() * 0.45;
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let best = analysis
            .bins
            .iter()
            .max_by(|a, b| a.bass_pitch_confidence.total_cmp(&b.bass_pitch_confidence))
            .expect("bin");

        assert_eq!(best.bass_pitch_class, 9);
        assert!(
            best.bass_pitch_confidence > 0.15,
            "confidence={}",
            best.bass_pitch_confidence
        );
    }

    #[test]
    fn analysis_gates_bass_pitch_in_near_silence() {
        let mut samples = Vec::new();
        for i in 0..96_000 {
            let t = i as f32 / 48_000.0;
            let x = (std::f32::consts::TAU * 110.0 * t).sin() * 0.00008;
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_confidence = analysis
            .bins
            .iter()
            .map(|b| b.bass_pitch_confidence)
            .fold(0.0_f32, f32::max);

        assert!(max_confidence < 0.03, "max_confidence={max_confidence}");
    }

    #[test]
    fn analysis_marks_local_feature_changes() {
        let mut samples = Vec::new();
        for i in 0..192_000 {
            let t = i as f32 / 48_000.0;
            let hz = if t < 2.0 { 110.0 } else { 660.0 };
            let x = (std::f32::consts::TAU * hz * t).sin() * 0.36;
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_novelty = analysis
            .bins
            .iter()
            .map(|b| b.novelty)
            .fold(0.0_f32, f32::max);

        assert!(max_novelty > 0.16, "max_novelty={max_novelty}");
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
    fn analysis_bridges_short_vocal_gaps() {
        let mut samples = Vec::new();
        for i in 0..144_000 {
            let t = i as f32 / 48_000.0;
            let active = (0.25..1.35).contains(&t) || (1.75..2.85).contains(&t);
            let envelope = if active { 1.0 } else { 0.0 };
            let x = envelope
                * 0.20
                * ((std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.55 * (std::f32::consts::TAU * 440.0 * t).sin()
                    + 0.35 * (std::f32::consts::TAU * 660.0 * t).sin()
                    + 0.18 * (std::f32::consts::TAU * 880.0 * t).sin());
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(
            &samples,
            48_000,
            AnalysisConfig {
                bin_secs: 0.025,
                ..AnalysisConfig::default()
            },
        );
        let gap_score = analysis
            .bins
            .iter()
            .filter(|bin| bin.start_secs >= 1.40 && bin.start_secs <= 1.65)
            .map(|bin| bin.vocal_score)
            .fold(0.0_f32, f32::max);

        assert!(gap_score > 0.08, "gap_score={gap_score}");
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
    fn analysis_keeps_noise_low_vocal_score() {
        let mut samples = Vec::new();
        let mut state = 0x1234_5678_u32;
        for _ in 0..96_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((state >> 8) as f32 / 16_777_215.0) * 2.0 - 1.0;
            let x = white * 0.16;
            samples.extend([x, x]);
        }

        let analysis = analyze_stereo_timeline(&samples, 48_000, AnalysisConfig::default());
        let max_vocal = analysis
            .bins
            .iter()
            .map(|b| b.vocal_score)
            .fold(0.0_f32, f32::max);

        assert!(max_vocal < 0.12, "max_vocal={max_vocal}");
    }

    #[test]
    fn spectrum_window_returns_requested_band_count() {
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let phase = std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0;
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

        let spectrum =
            spectrum_analysis_from_stereo_window(&samples, 48_000, 0.5, SPECTRUM_BAND_COUNT);
        assert_eq!(spectrum.bands.len(), SPECTRUM_BAND_COUNT);
        assert_eq!(spectrum.notes.len(), SPECTRUM_NOTE_BANDS);
        let a4 = (69 - spectrum.note_min_midi) as usize;
        assert!(spectrum.notes[a4] > 0.5);
    }

    #[test]
    fn spectrum_bands_are_centered_on_midi_semitones() {
        fn tone(midi: u8) -> Vec<f32> {
            let hz = midi_to_hz(midi);
            let mut samples = Vec::new();
            for i in 0..96_000 {
                let phase = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
                let x = phase.sin() * 0.6;
                samples.extend([x, x]);
            }
            samples
        }

        for midi in [60_u8, 69, 81] {
            let spectrum =
                spectrum_analysis_from_stereo_window(&tone(midi), 48_000, 1.0, SPECTRUM_BAND_COUNT);
            let band = (midi - SPECTRUM_BAND_MIN_MIDI) as usize;
            let center = spectrum.bands[band];
            let left = spectrum.bands[band - 1];
            let right = spectrum.bands[band + 1];
            assert!(
                center > 0.5,
                "midi {midi} center band should be visible, got {center}"
            );
            assert!(
                center > left * 2.5,
                "midi {midi} center {center} should dominate left {left}"
            );
            assert!(
                center > right * 2.5,
                "midi {midi} center {center} should dominate right {right}"
            );
        }
    }

    #[test]
    fn spectrum_moving_window_recomputes_when_samples_change_at_same_center() {
        fn tone(hz: f32) -> Vec<f32> {
            let mut samples = Vec::new();
            for i in 0..96_000 {
                let phase = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
                let x = phase.sin() * 0.6;
                samples.extend([x, x]);
            }
            samples
        }

        let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BAND_COUNT);
        let a4_tone = analyzer.analyze_moving_window(&tone(440.0), 48_000, 1.0);
        let a5_tone = analyzer.analyze_moving_window(&tone(880.0), 48_000, 1.0);
        let a4 = (69 - a4_tone.note_min_midi) as usize;
        let a5 = (81 - a4_tone.note_min_midi) as usize;

        assert!(
            a4_tone.notes[a4] > a4_tone.notes[a5] * 1.4,
            "A4 note should dominate for 440 Hz"
        );
        assert!(
            a5_tone.notes[a5] > a5_tone.notes[a4] * 1.4,
            "A5 note should dominate after moving window content changes"
        );
    }

    #[test]
    fn spectrum_low_bass_secondary_window_mix_fades_out_at_boundary() {
        assert_eq!(low_secondary_mix(32.0), SPECTRUM_LOW_SECONDARY_MAX_MIX);
        assert!(low_secondary_mix(80.0) > 0.0);
        assert_eq!(low_secondary_mix(90.0), 0.0);
    }

    #[test]
    fn spectrum_display_weight_reduces_sub_bass_and_lifts_presence() {
        assert!(spectrum_display_weight_db(40.0) < -4.0);
        assert!(spectrum_display_weight_db(100.0) < 0.0);
        assert!(spectrum_display_weight_db(2_000.0) > 0.8);
    }
}
