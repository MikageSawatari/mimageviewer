//! The only boundary between FFmpeg's source clock and mIV media seconds.

use ffmpeg_the_third::{Rational, format::context::Input};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MediaTimeline {
    origin_secs: f64,
}

impl MediaTimeline {
    pub(crate) fn from_input(input: &Input) -> Self {
        let format = input.format();
        let name = format.name();
        // SAFETY: the format context is owned by the borrowed, open Input.
        let start = unsafe { (*input.as_ptr()).start_time };
        let timeline = Self::from_demuxer_start(name, start);
        if Self::rebases_demuxer(name) && start == ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
            crate::logger::log(format!(
                "video timeline: demuxer={name} missing start time; origin=0"
            ));
        }
        timeline
    }

    fn rebases_demuxer(name: &str) -> bool {
        // These demuxers estimate duration as end - start, rather than using a
        // container timeline with its own zero. AVFMT_TS_DISCONT is NOT this
        // policy: e.g. live_flv and ogg carry it too. Verify FFmpeg's duration
        // semantics before adding another demuxer here.
        matches!(name, "mpegts" | "mpegtsraw" | "mpeg")
    }

    fn from_demuxer_start(name: &str, start: i64) -> Self {
        let origin_secs =
            if Self::rebases_demuxer(name) && start != ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
                start as f64 / f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE)
            } else {
                0.0
            };
        Self { origin_secs }
    }

    /// Reconstruct the already-decided origin only for a released resume row.
    /// Playback and auxiliary workers must construct this from their own Input.
    pub(crate) fn for_resume_migration(origin_secs: f64) -> Self {
        debug_assert!(origin_secs.is_finite());
        Self { origin_secs }
    }

    pub(crate) fn origin_secs(self) -> f64 {
        self.origin_secs
    }

    pub(crate) fn raw_to_media_secs(self, raw_secs: f64) -> f64 {
        raw_secs - self.origin_secs
    }

    pub(crate) fn media_to_raw_secs(self, media_secs: f64) -> f64 {
        media_secs + self.origin_secs
    }

    pub(crate) fn media_to_av_time_base(self, media_secs: f64) -> i64 {
        (self.media_to_raw_secs(media_secs) * f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE)) as i64
    }

    pub(crate) fn media_to_stream_ticks(self, media_secs: f64, time_base: Rational) -> i64 {
        (self.media_to_raw_secs(media_secs) * f64::from(time_base.denominator())
            / f64::from(time_base.numerator())) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demuxer_policy_does_not_follow_discontinuity_flag_or_positive_start() {
        for name in ["mpegts", "mpegtsraw", "mpeg"] {
            assert_eq!(
                MediaTimeline::from_demuxer_start(name, 36_001_400_000).origin_secs(),
                36001.4
            );
            assert_eq!(
                MediaTimeline::from_demuxer_start(name, ffmpeg_the_third::ffi::AV_NOPTS_VALUE),
                MediaTimeline::default()
            );
            assert_eq!(
                MediaTimeline::from_demuxer_start(name, 0),
                MediaTimeline::default()
            );
        }
        for name in [
            "ogg",
            "matroska,webm",
            "mov,mp4,m4a,3gp,3g2,mj2",
            "mp3",
            "live_flv",
            "flv",
            "avi",
            "flac",
        ] {
            assert_eq!(
                MediaTimeline::from_demuxer_start(name, 10_000_000),
                MediaTimeline::default()
            );
        }
    }

    #[test]
    fn seconds_roundtrip_and_seek_tick_rounding() {
        let timeline = MediaTimeline::from_demuxer_start("mpegts", 36_001_400_000);
        for media in [-0.04, 0.0, 4.125, 11.96] {
            assert!(
                (timeline.raw_to_media_secs(timeline.media_to_raw_secs(media)) - media).abs()
                    < 1e-10
            );
        }
        assert_eq!(timeline.media_to_av_time_base(4.125), 36_005_525_000);
        assert_eq!(
            timeline.media_to_stream_ticks(4.125, Rational(1, 90_000)),
            3_240_497_250
        );
        assert_eq!(MediaTimeline::default().media_to_av_time_base(0.0000019), 1);
        assert_eq!(
            MediaTimeline::default().media_to_stream_ticks(1.2349, Rational(1, 1000)),
            1234
        );
    }
}
