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
        let mut beats = Vec::new();
        let mut t = first_beat_secs.max(0.0);
        while t <= duration_secs + beat_period * 0.5 {
            beats.push(BeatMarker {
                time_secs: t,
                confidence,
            });
            t += beat_period;
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
}
