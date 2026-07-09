use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BeatTrackingStatus {
    NotRun,
    Estimated,
    LowConfidence,
    UserCorrected,
}

impl Default for BeatTrackingStatus {
    fn default() -> Self {
        Self::NotRun
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeatMarker {
    pub time_secs: f64,
    pub confidence: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarMarker {
    pub index: u32,
    pub time_secs: f64,
    pub beat_index: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BeatGrid {
    pub status: BeatTrackingStatus,
    pub bpm: Option<f32>,
    pub confidence: f32,
    pub time_signature_numerator: u8,
    pub beats: Vec<BeatMarker>,
    pub bars: Vec<BarMarker>,
}

impl BeatGrid {
    pub fn empty() -> Self {
        Self {
            status: BeatTrackingStatus::NotRun,
            time_signature_numerator: 4,
            ..Self::default()
        }
    }

    pub fn from_bpm(duration_secs: f64, bpm: f32, first_beat_secs: f64, confidence: f32) -> Self {
        if !duration_secs.is_finite()
            || duration_secs <= 0.0
            || !bpm.is_finite()
            || bpm <= 0.0
            || !first_beat_secs.is_finite()
        {
            return Self::empty();
        }

        let beat_period = 60.0 / bpm as f64;
        // 上限ガード (review-v2.3.0 hunt P2): BPM はビート検出以外にメタデータ / 外部入力
        // 由来の値も通り得る。異常な高 BPM だと (a) 数千万 beat の確保、(b) beat_period が
        // float 精度以下になって `t += beat_period` が進まない無限ループ、の両方が起きる。
        // 実用域 (〜300 BPM × 数時間) は 200_000 で余裕を持ってカバーする。
        const MAX_BEATS: usize = 200_000;
        let mut beats = Vec::new();
        let mut t = first_beat_secs.max(0.0);
        while t <= duration_secs + beat_period * 0.5 && beats.len() < MAX_BEATS {
            beats.push(BeatMarker {
                time_secs: t,
                confidence,
            });
            let next = t + beat_period;
            if next <= t {
                // beat_period が t の float 精度以下 (進まない): これ以上生成できない。
                break;
            }
            t = next;
        }

        let mut bars = Vec::new();
        for (beat_index, beat) in beats.iter().enumerate().step_by(4) {
            bars.push(BarMarker {
                index: bars.len() as u32,
                time_secs: beat.time_secs,
                beat_index: beat_index as u32,
            });
        }

        Self {
            status: if confidence >= 0.35 {
                BeatTrackingStatus::Estimated
            } else {
                BeatTrackingStatus::LowConfidence
            },
            bpm: Some(bpm),
            confidence,
            time_signature_numerator: 4,
            beats,
            bars,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_grid_builds_beats_and_bars() {
        let grid = BeatGrid::from_bpm(16.0, 120.0, 0.0, 0.8);
        assert_eq!(grid.bpm, Some(120.0));
        assert!(grid.beats.len() >= 32);
        assert_eq!(grid.bars[1].beat_index, 4);
        assert!((grid.bars[1].time_secs - 2.0).abs() < 1.0e-9);
    }

    #[test]
    fn bpm_grid_is_bounded_for_garbage_bpm() {
        // review-v2.3.0 hunt P2: 異常 BPM で巨大 allocation / 無限ループにならない。
        let grid = BeatGrid::from_bpm(60.0, 1_000_000.0, 0.0, 1.0);
        assert!(grid.beats.len() <= 200_000);
        // beat_period が float 精度以下でも終了する (進まなくなったら break)。
        let grid = BeatGrid::from_bpm(3600.0, f32::MAX, 0.0, 1.0);
        assert!(grid.beats.len() <= 200_000);
        // 通常域は従来どおり。
        let grid = BeatGrid::from_bpm(60.0, 120.0, 0.0, 0.8);
        assert!((grid.beats.len() as i64 - 121).abs() <= 1);
    }
}
