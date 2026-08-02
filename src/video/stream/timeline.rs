use std::fmt;

pub(crate) const STREAM_TIMELINE_EPSILON_SECS: f64 = 1.0e-9;

/// 音声・映像が共有する source timeline → streaming session timeline の写像。
/// session start の source PTS を唯一の原点とし、両 stream を同じ 0 起点へ移す。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StreamTimeline {
    source_start_secs: f64,
}

impl StreamTimeline {
    pub(crate) fn new(source_start_secs: f64) -> Result<Self, StreamTimelineError> {
        if !source_start_secs.is_finite() {
            return Err(StreamTimelineError::NonFiniteSourceTime);
        }
        Ok(Self { source_start_secs })
    }

    pub(crate) fn source_start_secs(self) -> f64 {
        self.source_start_secs
    }

    pub(crate) fn relative_secs(self, source_pts_secs: f64) -> Result<f64, StreamTimelineError> {
        if !source_pts_secs.is_finite() {
            return Err(StreamTimelineError::NonFiniteSourceTime);
        }
        let relative = source_pts_secs - self.source_start_secs;
        if relative < -STREAM_TIMELINE_EPSILON_SECS {
            return Err(StreamTimelineError::BeforeSessionStart {
                source_pts_secs,
                source_start_secs: self.source_start_secs,
                delta_secs: relative,
            });
        }
        Ok(relative.max(0.0))
    }

    pub(crate) fn relative_ticks(
        self,
        source_pts_secs: f64,
        ticks_per_sec: u32,
    ) -> Result<i64, StreamTimelineError> {
        if ticks_per_sec == 0 {
            return Err(StreamTimelineError::ZeroTimescale);
        }
        let ticks = self.relative_secs(source_pts_secs)? * f64::from(ticks_per_sec);
        if ticks > i64::MAX as f64 {
            return Err(StreamTimelineError::TimestampOverflow);
        }
        Ok(ticks.round() as i64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum StreamTimelineError {
    NonFiniteSourceTime,
    BeforeSessionStart {
        source_pts_secs: f64,
        source_start_secs: f64,
        delta_secs: f64,
    },
    ZeroTimescale,
    TimestampOverflow,
}

impl fmt::Display for StreamTimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteSourceTime => f.write_str("source timestamp must be finite"),
            Self::BeforeSessionStart {
                source_pts_secs,
                source_start_secs,
                delta_secs,
            } => write!(
                f,
                "source timestamp precedes streaming session start: source_pts_secs={source_pts_secs:.9}, source_start_secs={source_start_secs:.9}, delta_secs={delta_secs:.9}"
            ),
            Self::ZeroTimescale => f.write_str("streaming timescale must be non-zero"),
            Self::TimestampOverflow => f.write_str("streaming timestamp exceeds i64 range"),
        }
    }
}

impl std::error::Error for StreamTimelineError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_and_video_share_one_zero_based_source_mapping() {
        let timeline = StreamTimeline::new(123.5).unwrap();
        assert_eq!(timeline.relative_secs(123.5), Ok(0.0));
        assert_eq!(timeline.relative_ticks(125.5, 48_000), Ok(96_000));
        assert_eq!(timeline.relative_ticks(125.5, 30), Ok(60));
    }

    #[test]
    fn timestamps_before_session_are_not_silently_clamped() {
        let timeline = StreamTimeline::new(10.0).unwrap();
        let error = timeline.relative_secs(9.5).unwrap_err();
        assert_eq!(
            error,
            StreamTimelineError::BeforeSessionStart {
                source_pts_secs: 9.5,
                source_start_secs: 10.0,
                delta_secs: -0.5,
            }
        );
        assert_eq!(
            error.to_string(),
            "source timestamp precedes streaming session start: source_pts_secs=9.500000000, source_start_secs=10.000000000, delta_secs=-0.500000000"
        );
        assert_eq!(timeline.relative_secs(10.0 - 5.0e-10), Ok(0.0));
    }
}
