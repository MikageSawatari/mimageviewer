//! mIV の横長スペクトラムに固有の状態遷移と配置判断。
//!
//! egui や特定のラスタライザには依存せず、本体の音楽ビューと音楽ビデオ書き出しが
//! 同じフレーム状態・対数周波数軸・重ね描きプリミティブを使う。

use crate::{SPECTRUM_BAND_MAX_MIDI, SPECTRUM_BAND_MIN_MIDI};

pub const SPECTRUM_AXIS_MIN_HZ: f32 = 20.0;
pub const SPECTRUM_AXIS_MAX_HZ: f32 = 18_000.0;
pub const MIV_SPECTRUM_MAX_SEQUENTIAL_GAP_SECS: f64 = 0.25;

const SPECTRUM_ANALYSIS_MIN_HZ: f32 = 20.0;
const SPECTRUM_TRAIL_DECAY: f32 = 0.994;
const SPECTRUM_ONSET_DECAY: f32 = 0.86;
const SPECTRUM_ONSET_GAIN: f32 = 2.8;
const SPECTRUM_PREVIOUS_RETAIN: f32 = 0.25;
const SPECTRUM_CURRENT_MIX: f32 = 0.75;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpectrumVisualRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SpectrumVisualRect {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MivSpectrumBandFrame {
    pub value: f32,
    pub trail: f32,
    pub onset: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MivSpectrumFrame {
    pub bands: Vec<MivSpectrumBandFrame>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MivSpectrumState {
    trail: Vec<f32>,
    previous_bands: Vec<f32>,
    onsets: Vec<f32>,
    frame: MivSpectrumFrame,
    last_t: Option<f64>,
}

impl MivSpectrumState {
    pub fn clear(&mut self) {
        self.trail.clear();
        self.previous_bands.clear();
        self.onsets.clear();
        self.frame.bands.clear();
        self.last_t = None;
    }

    /// mIV と同じ残光・立ち上がり強調を、単調増加する音声時刻で 1 回だけ進める。
    ///
    /// 同じ時刻はキャッシュを返し、後退または 250ms を超える飛びはシークとして状態を
    /// リセットする。したがって結果は実時間や描画回数ではなく、入力と `t_secs` の列だけで決まる。
    pub fn frame_at(&mut self, bands: &[f32], t_secs: f64) -> &MivSpectrumFrame {
        let t = self.normalize_time(t_secs);
        if self.last_t == Some(t) {
            return &self.frame;
        }
        self.advance_at(bands, t);
        &self.frame
    }

    /// Adopt one newly completed live analysis, including revised input at the same time.
    ///
    /// The live result owner calls this once per accepted result; painting reads `frame()`.
    /// Time-based callers keep `frame_at()`'s repeated-time cache contract.
    pub fn adopt_analysis_at(&mut self, bands: &[f32], t_secs: f64) -> &MivSpectrumFrame {
        let t = self.normalize_time(t_secs);
        self.advance_at(bands, t);
        &self.frame
    }

    /// Advance exactly one output frame when a timeline owns sequencing and seeks.
    /// No time-based cache or gap reset applies, including at low frame rates.
    pub fn advance_frame(&mut self, bands: &[f32]) -> &MivSpectrumFrame {
        self.advance(bands);
        &self.frame
    }

    /// Read the accepted visual snapshot without advancing afterglow or onset state.
    pub fn frame(&self) -> &MivSpectrumFrame {
        &self.frame
    }

    fn normalize_time(&mut self, t_secs: f64) -> f64 {
        if t_secs.is_finite() {
            t_secs.max(0.0)
        } else {
            self.clear();
            0.0
        }
    }

    fn advance_at(&mut self, bands: &[f32], t: f64) {
        if self
            .last_t
            .is_some_and(|last| t < last || t - last > MIV_SPECTRUM_MAX_SEQUENTIAL_GAP_SECS)
        {
            self.clear();
        }
        self.advance(bands);
        self.last_t = Some(t);
    }

    fn advance(&mut self, bands: &[f32]) {
        let band_len = bands.len();
        if self.trail.len() != band_len {
            self.trail.clear();
            self.trail.resize(band_len, 0.0);
        }
        if self.previous_bands.len() != band_len {
            self.previous_bands.clear();
            self.previous_bands.extend_from_slice(bands);
        }
        if self.onsets.len() != band_len {
            self.onsets.clear();
            self.onsets.resize(band_len, 0.0);
        }
        if self.frame.bands.len() != band_len {
            self.frame
                .bands
                .resize(band_len, MivSpectrumBandFrame::default());
        }

        for (index, raw) in bands.iter().copied().enumerate() {
            let value = raw.clamp(0.0, 1.0);
            let rise = (value - self.previous_bands[index]).max(0.0);
            self.onsets[index] = (self.onsets[index] * SPECTRUM_ONSET_DECAY)
                .max((rise * SPECTRUM_ONSET_GAIN).clamp(0.0, 1.0));
            self.previous_bands[index] = self.previous_bands[index] * SPECTRUM_PREVIOUS_RETAIN
                + value * SPECTRUM_CURRENT_MIX;
            self.trail[index] = (self.trail[index] * SPECTRUM_TRAIL_DECAY).max(value);
            self.frame.bands[index] = MivSpectrumBandFrame {
                value,
                trail: self.trail[index],
                onset: self.onsets[index],
            };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MivSpectrumPass {
    Ghost,
    Trail,
    Current,
    Onset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MivSpectrumPrimitive {
    pub pass: MivSpectrumPass,
    pub band_index: usize,
    pub band_count: usize,
    pub rect: SpectrumVisualRect,
    pub corner_radius_px: f32,
    pub color_value: f32,
    /// `None` は `spectrum_color` またはカスタム Fill 本来の alpha を保つ。
    pub alpha_override: Option<u8>,
    pub brightness_scale: f32,
}

/// mIV の描画順 (バンドごとに ghost → trail → 現在値 → onset) でプリミティブを渡す。
pub fn visit_miv_spectrum_primitives(
    frame: &MivSpectrumFrame,
    plot: SpectrumVisualRect,
    visit: impl FnMut(MivSpectrumPrimitive),
) {
    visit_miv_spectrum_primitives_with_dimensions(
        frame,
        plot,
        MivSpectrumDimensions::default(),
        visit,
    );
}

/// Pixel dimensions independent of signal levels and the normalized plot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MivSpectrumDimensions {
    pub band_inset_px: f32,
    pub min_width_px: f32,
    pub corner_threshold_px: f32,
    pub corner_radius_px: f32,
    pub bottom_inset_px: f32,
    pub height_inset_px: f32,
    pub ghost_expand_px: f32,
    pub trail_expand_px: f32,
}

impl Default for MivSpectrumDimensions {
    fn default() -> Self {
        Self {
            band_inset_px: 0.25,
            min_width_px: 0.75,
            corner_threshold_px: 2.0,
            corner_radius_px: 1.0,
            bottom_inset_px: 2.0,
            height_inset_px: 3.0,
            ghost_expand_px: 0.45,
            trail_expand_px: 0.2,
        }
    }
}

pub fn visit_miv_spectrum_primitives_with_dimensions(
    frame: &MivSpectrumFrame,
    plot: SpectrumVisualRect,
    dimensions: MivSpectrumDimensions,
    mut visit: impl FnMut(MivSpectrumPrimitive),
) {
    let total = frame.bands.len();
    if total == 0 {
        return;
    }
    for (index, band) in frame.bands.iter().copied().enumerate() {
        let (band_low_hz, band_high_hz) = spectrum_band_hz_range(index, total);
        let x0 = (spectrum_axis_x(plot, band_low_hz) + dimensions.band_inset_px).max(plot.left);
        let x1 = (spectrum_axis_x(plot, band_high_hz) - dimensions.band_inset_px)
            .max(x0 + dimensions.min_width_px)
            .min(plot.right);
        let corner_radius_px = if x1 - x0 < dimensions.corner_threshold_px {
            0.0
        } else {
            dimensions.corner_radius_px
        };
        let base_y = plot.bottom - dimensions.bottom_inset_px;
        let height = plot.height() - dimensions.height_inset_px;
        let mut emit = |pass,
                        x_expand: f32,
                        height_ratio: f32,
                        color_value: f32,
                        alpha_override,
                        brightness_scale| {
            visit(MivSpectrumPrimitive {
                pass,
                band_index: index,
                band_count: total,
                rect: SpectrumVisualRect::new(
                    (x0 - x_expand).max(plot.left),
                    base_y - height * height_ratio,
                    (x1 + x_expand).min(plot.right),
                    base_y,
                ),
                corner_radius_px,
                color_value,
                alpha_override,
                brightness_scale,
            });
        };
        emit(
            MivSpectrumPass::Ghost,
            dimensions.ghost_expand_px,
            (band.trail * 0.72).max(0.012),
            band.trail,
            Some(48),
            1.0,
        );
        emit(
            MivSpectrumPass::Trail,
            dimensions.trail_expand_px,
            band.trail.max(0.015),
            band.trail,
            Some(100),
            1.0,
        );
        emit(
            MivSpectrumPass::Current,
            0.0,
            band.value.max(0.015),
            band.value,
            None,
            1.0,
        );
        let onset = band.onset.clamp(0.0, 1.0);
        if onset > 0.025 {
            emit(
                MivSpectrumPass::Onset,
                0.0,
                band.value.max(0.015),
                band.value.max(onset),
                Some((24.0 + onset * 68.0) as u8),
                1.18,
            );
        }
    }
}

pub fn spectrum_axis_x(rect: SpectrumVisualRect, hz: f32) -> f32 {
    let t = spectrum_axis_position(hz).clamp(0.0, 1.0);
    rect.left + t * rect.width()
}

pub fn spectrum_axis_x_unclamped(rect: SpectrumVisualRect, hz: f32) -> f32 {
    rect.left + spectrum_axis_position(hz) * rect.width()
}

pub fn spectrum_axis_hz(rect: SpectrumVisualRect, x: f32) -> f32 {
    let t = ((x - rect.left) / rect.width().max(1.0)).clamp(0.0, 1.0);
    2.0_f32.powf(
        SPECTRUM_AXIS_MIN_HZ.log2()
            + t * (SPECTRUM_AXIS_MAX_HZ.log2() - SPECTRUM_AXIS_MIN_HZ.log2()),
    )
}

pub fn spectrum_band_hz(index: usize, total: usize) -> f32 {
    spectrum_band_midi(index, total).map_or(SPECTRUM_ANALYSIS_MIN_HZ, midi_to_hz)
}

pub fn spectrum_band_hz_range(index: usize, total: usize) -> (f32, f32) {
    let Some(midi) = spectrum_band_midi(index, total) else {
        let half = 2.0_f32.powf(1.0 / 24.0);
        return (
            SPECTRUM_ANALYSIS_MIN_HZ / half,
            SPECTRUM_ANALYSIS_MIN_HZ * half,
        );
    };
    let center = midi_to_hz(midi);
    let half_step = 2.0_f32.powf(1.0 / 24.0);
    (center / half_step, center * half_step)
}

fn spectrum_axis_position(hz: f32) -> f32 {
    (hz.max(1.0).log2() - SPECTRUM_AXIS_MIN_HZ.log2())
        / (SPECTRUM_AXIS_MAX_HZ.log2() - SPECTRUM_AXIS_MIN_HZ.log2())
}

fn spectrum_band_midi(index: usize, total: usize) -> Option<u8> {
    if total == 0 || index >= total {
        return None;
    }
    let midi = SPECTRUM_BAND_MIN_MIDI as usize + index;
    (midi <= SPECTRUM_BAND_MAX_MIDI as usize).then_some(midi as u8)
}

pub fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_input_captures_shipped_state_transition_and_draw_plan() {
        let mut state = MivSpectrumState::default();
        assert_eq!(
            state.frame_at(&[0.2, 0.8, 0.4], 1.0).bands,
            [
                MivSpectrumBandFrame {
                    value: 0.2,
                    trail: 0.2,
                    onset: 0.0
                },
                MivSpectrumBandFrame {
                    value: 0.8,
                    trail: 0.8,
                    onset: 0.0
                },
                MivSpectrumBandFrame {
                    value: 0.4,
                    trail: 0.4,
                    onset: 0.0
                },
            ]
        );
        let second = state.frame_at(&[0.7, 0.3, 0.5], 1.0 + 1.0 / 60.0).clone();
        assert_eq!(
            second.bands[0],
            MivSpectrumBandFrame {
                value: 0.7,
                trail: 0.7,
                onset: 1.0
            }
        );
        assert_eq!(
            second.bands[1],
            MivSpectrumBandFrame {
                value: 0.3,
                trail: 0.795_200_05,
                onset: 0.0
            }
        );
        assert_eq!(
            second.bands[2],
            MivSpectrumBandFrame {
                value: 0.5,
                trail: 0.5,
                onset: 0.279_999_97
            }
        );

        let mut primitives = Vec::new();
        visit_miv_spectrum_primitives(
            &second,
            SpectrumVisualRect::new(18.0, 12.0, 1902.0, 240.0),
            |primitive| primitives.push(primitive),
        );
        assert_eq!(primitives.len(), 11);
        assert_eq!(primitives[0].pass, MivSpectrumPass::Ghost);
        assert_eq!(primitives[1].pass, MivSpectrumPass::Trail);
        assert_eq!(primitives[2].pass, MivSpectrumPass::Current);
        assert_eq!(primitives[3].pass, MivSpectrumPass::Onset);
        assert_eq!(primitives[0].alpha_override, Some(48));
        assert_eq!(primitives[1].alpha_override, Some(100));
        assert_eq!(primitives[3].alpha_override, Some(92));
        assert_eq!(primitives[3].brightness_scale, 1.18);
        assert!((primitives[2].rect.bottom - 238.0).abs() < f32::EPSILON);
    }

    #[test]
    fn explicit_frames_advance_without_time_cache_or_gap_resets() {
        let mut state = MivSpectrumState::default();
        let mut timed = MivSpectrumState::default();
        for (index, bands) in [[0.8, 0.1], [0.1, 0.7], [0.1, 0.7]].iter().enumerate() {
            assert_eq!(
                state.advance_frame(bands),
                timed.frame_at(bands, index as f64 / 60.0),
            );
        }
        assert_eq!(state.last_t, None);
        assert_eq!(
            state.frame().bands[0].trail,
            0.8 * SPECTRUM_TRAIL_DECAY * SPECTRUM_TRAIL_DECAY
        );
        assert_eq!(state.frame().bands[1].onset, SPECTRUM_ONSET_DECAY);
    }

    #[test]
    fn repeated_time_is_cached_and_seek_resets_state() {
        let mut state = MivSpectrumState::default();
        let first = state.frame_at(&[0.8], 1.0).clone();
        assert_eq!(state.frame_at(&[0.1], 1.0), &first);
        let after_seek = state.frame_at(&[0.1], 0.5).clone();
        let mut fresh = MivSpectrumState::default();
        assert_eq!(after_seek, *fresh.frame_at(&[0.1], 0.5));
    }

    #[test]
    fn live_same_time_adoption_updates_values_and_preserves_afterglow() {
        let mut state = MivSpectrumState::default();
        state.adopt_analysis_at(&[0.8, 0.1], 1.0);
        let revised = state.adopt_analysis_at(&[0.1, 0.7], 1.0).clone();
        assert_eq!(revised.bands[0].value, 0.1);
        assert_eq!(revised.bands[0].trail, 0.8 * SPECTRUM_TRAIL_DECAY);
        assert_eq!(revised.bands[1].value, 0.7);
        assert_eq!(revised.bands[1].onset, 1.0);
        assert_eq!(state.frame(), &revised);
        assert_eq!(state.frame_at(&[0.9, 0.9], 1.0), &revised);
        assert_eq!(state.frame(), &revised);
        let after_seek = state.adopt_analysis_at(&[0.2], 0.5).clone();
        let mut fresh = MivSpectrumState::default();
        assert_eq!(after_seek, *fresh.frame_at(&[0.2], 0.5));
        let after_gap = state.adopt_analysis_at(&[0.3], 2.0).clone();
        assert_eq!(after_gap, *fresh.frame_at(&[0.3], 2.0));
    }

    #[test]
    fn logarithmic_axis_and_band_ranges_are_stable() {
        let rect = SpectrumVisualRect::new(18.0, 0.0, 1902.0, 100.0);
        let a4 = spectrum_axis_x(rect, 440.0);
        assert!((a4 - 874.099_1).abs() < 0.001);
        assert!((spectrum_axis_hz(rect, a4) - 440.0).abs() < 0.001);
        let a4_index = 69 - SPECTRUM_BAND_MIN_MIDI as usize;
        let (low, high) = spectrum_band_hz_range(a4_index, 118);
        assert!((low - 427.474_06).abs() < 0.001);
        assert!((high - 452.892_97).abs() < 0.001);
    }
}
