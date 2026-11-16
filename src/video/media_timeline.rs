//! The only boundary between FFmpeg's source clock and mIV media seconds.

use ffmpeg_the_third::{Rational, format::context::Input};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MediaTimeline {
    // AVFormatContext.start_time has AV_TIME_BASE (one microsecond) precision.
    // Retain the integer so integer seek locators never round-trip through f64.
    origin_av_time_base: i64,
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
        let origin_av_time_base =
            if Self::rebases_demuxer(name) && start != ffmpeg_the_third::ffi::AV_NOPTS_VALUE {
                start
            } else {
                0
            };
        Self {
            origin_av_time_base,
        }
    }

    /// Reconstruct the already-decided origin only for a released resume row.
    /// Playback and auxiliary workers must construct this from their own Input.
    pub(crate) fn for_resume_migration(origin_secs: f64) -> Self {
        debug_assert!(origin_secs.is_finite());
        Self {
            origin_av_time_base: (origin_secs * f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE))
                .round() as i64,
        }
    }

    pub(crate) fn origin_secs(self) -> f64 {
        self.origin_av_time_base as f64 / f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE)
    }

    pub(crate) fn raw_to_media_secs(self, raw_secs: f64) -> f64 {
        if self.origin_av_time_base == 0 {
            return raw_secs;
        }
        let media_secs = raw_secs - self.origin_secs();
        // Source ticks can be finer than the format origin's one-microsecond
        // precision. Only erase that rounding deficit, never real preroll.
        if media_secs > -1.0 / f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE) && media_secs < 0.0 {
            0.0
        } else {
            media_secs
        }
    }

    pub(crate) fn media_to_raw_secs(self, media_secs: f64) -> f64 {
        media_secs + self.origin_secs()
    }

    /// Convert an integer media locator without losing any AV_TIME_BASE ticks.
    pub(crate) fn media_av_time_base_to_raw(self, media_timestamp: i64) -> i64 {
        media_timestamp.saturating_add(self.origin_av_time_base)
    }

    pub(crate) fn media_to_av_time_base(self, media_secs: f64) -> i64 {
        (self.media_to_raw_secs(media_secs) * f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE)) as i64
    }

    pub(crate) fn media_to_stream_ticks(self, media_secs: f64, time_base: Rational) -> i64 {
        (self.media_to_raw_secs(media_secs) * f64::from(time_base.denominator())
            / f64::from(time_base.numerator())) as i64
    }

    /// 音声filterへ渡す整数PTSも同じ原点で変換する。原点0では丸めず恒等に保つ。
    pub(crate) fn raw_stream_ticks_to_media(self, raw: i64, time_base: Rational) -> Option<i64> {
        if self.origin_av_time_base == 0 {
            return Some(raw);
        }
        if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
            return None;
        }
        // AVFormatContextの整数原点をstreamのtickへ丸める。秒のf64往復はしない。
        let origin_ticks = unsafe {
            ffmpeg_the_third::ffi::av_rescale_q(
                self.origin_av_time_base,
                ffmpeg_the_third::ffi::AVRational {
                    num: 1,
                    den: ffmpeg_the_third::ffi::AV_TIME_BASE,
                },
                ffmpeg_the_third::ffi::AVRational {
                    num: time_base.numerator(),
                    den: time_base.denominator(),
                },
            )
        };
        raw.checked_sub(origin_ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_integer_audio_pts_share_media_origin_and_preserve_preroll() {
        let timeline = MediaTimeline::from_demuxer_start("mpegts", 36_001_378_667);
        let ticks = Rational(1, 90_000);
        assert_eq!(
            timeline.raw_stream_ticks_to_media(3_240_124_080, ticks),
            Some(0)
        );
        assert_eq!(
            timeline.raw_stream_ticks_to_media(3_240_124_080 + 360_000, ticks),
            Some(360_000)
        );
        assert_eq!(
            timeline.raw_stream_ticks_to_media(3_240_124_079, ticks),
            Some(-1)
        );
        assert_eq!(timeline.raw_stream_ticks_to_media(i64::MIN, ticks), None);
        for name in ["matroska,webm", "mov,mp4,m4a,3gp,3g2,mj2", "mp3"] {
            let identity = MediaTimeline::from_demuxer_start(name, 10_000_000);
            for raw in [i64::MIN, -1, 0, 208_000, i64::MAX] {
                assert_eq!(
                    identity.raw_stream_ticks_to_media(raw, Rational(1, 16_000)),
                    Some(raw)
                );
            }
        }
        let negative = MediaTimeline::from_demuxer_start("mpegts", -4_621_333);
        assert_eq!(negative.raw_stream_ticks_to_media(-415_920, ticks), Some(0));
    }

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

    #[test]
    fn origin_precision_zeroes_only_sub_microsecond_negative_rounding() {
        let timeline = MediaTimeline::from_demuxer_start("mpegts", 1);
        assert_eq!(timeline.raw_to_media_secs(0.5e-6), 0.0);
        assert_eq!(timeline.raw_to_media_secs(1e-6), 0.0);
        assert_eq!(timeline.raw_to_media_secs(0.0), -1e-6);
        assert_eq!(timeline.raw_to_media_secs(-1e-6), -2e-6);
        for (origin, first_pts) in [
            (36_001_378_667, 3_240_124_080_i64),
            (36_001_666_667, 3_240_150_000_i64),
        ] {
            let timeline = MediaTimeline::from_demuxer_start("mpegts", origin);
            assert_eq!(timeline.raw_to_media_secs(first_pts as f64 / 90_000.0), 0.0);
            assert!(timeline.raw_to_media_secs(first_pts as f64 / 90_000.0 - 1e-3) < -0.000999);
        }
        for timeline in [
            MediaTimeline::default(),
            MediaTimeline::from_demuxer_start("mpegts", 0),
            MediaTimeline::from_demuxer_start("matroska,webm", 10_000_000),
        ] {
            for raw in [-0.5e-6_f64, -0.0, 0.0, 10.0] {
                assert_eq!(timeline.raw_to_media_secs(raw).to_bits(), raw.to_bits());
            }
        }
    }

    #[test]
    fn integer_seek_locators_preserve_microseconds_and_identity() {
        for timeline in [
            MediaTimeline::default(),
            MediaTimeline::from_demuxer_start("mpegts", 0),
            MediaTimeline::from_demuxer_start("matroska,webm", 10_000_000),
        ] {
            for target in [i64::MIN, -2_003_500, -1, 0, 1, 2_003_500, i64::MAX] {
                assert_eq!(timeline.media_av_time_base_to_raw(target), target);
            }
        }
        let timeline = MediaTimeline::from_demuxer_start("mpegts", 36_001_666_667);
        assert_eq!(
            timeline.media_av_time_base_to_raw(2_003_500),
            36_003_670_167
        );
        let wrapped = MediaTimeline::from_demuxer_start("mpegts", -4_621_333);
        assert_eq!(wrapped.media_av_time_base_to_raw(4_000_000), -621_333);
    }
}
