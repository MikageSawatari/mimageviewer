//! 文字起こしのコンテナ相対秒と、既存の再生秒の間だけを変換する。
//! decoder、再生時計、既存の seek やブックマークの原点は変更しない。

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TranscriptTimeMapping {
    playback_zero_secs: Option<f64>,
}

impl TranscriptTimeMapping {
    /// raw PTS 秒を使う再生経路。原点の非 finite 値は構築境界で未確定にする。
    /// 負のコンテナ原点は有効なので保持する。音声 stream の開始秒は渡さない。
    pub fn raw_pts(container_start_secs: f64) -> Self {
        Self {
            playback_zero_secs: container_start_secs
                .is_finite()
                .then_some(container_start_secs),
        }
    }

    /// 既にコンテナ原点を引いた経路。二重に原点を加算しない。
    pub fn container_relative() -> Self {
        Self {
            playback_zero_secs: Some(0.0),
        }
    }

    pub fn to_playback(self, transcript_secs: f64) -> Option<f64> {
        let result = transcript_secs + self.playback_zero_secs?;
        result.is_finite().then_some(result)
    }

    pub fn to_transcript(self, playback_secs: f64) -> Option<f64> {
        let result = playback_secs - self.playback_zero_secs?;
        result.is_finite().then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_pts_uses_container_origin_and_preserves_audio_delay() {
        let mapping = TranscriptTimeMapping::raw_pts(10.0);
        assert_eq!(mapping.to_playback(3.0), Some(13.0));
        assert_eq!(mapping.to_transcript(13.0), Some(3.0));
        assert_eq!(mapping.to_playback(0.0), Some(10.0));
    }

    #[test]
    fn container_relative_is_identity_without_double_origin_adjustment() {
        let mapping = TranscriptTimeMapping::container_relative();
        assert_eq!(mapping.to_playback(3.0), Some(3.0));
        assert_eq!(mapping.to_transcript(3.0), Some(3.0));
    }

    #[test]
    fn negative_origin_is_preserved_and_nonfinite_origin_is_unknown() {
        let mapping = TranscriptTimeMapping::raw_pts(-10.0);
        assert_eq!(mapping.to_playback(13.0), Some(3.0));
        assert_eq!(mapping.to_transcript(3.0), Some(13.0));
        for origin in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mapping = TranscriptTimeMapping::raw_pts(origin);
            assert_eq!(mapping, TranscriptTimeMapping::default());
            assert_eq!(mapping.to_playback(3.0), None);
            assert_eq!(mapping.to_transcript(3.0), None);
        }
    }

    #[test]
    fn unknown_origin_and_nonfinite_input_do_not_offer_a_seek_target() {
        let unknown = TranscriptTimeMapping::default();
        assert_eq!(unknown.to_playback(3.0), None);
        assert_eq!(unknown.to_transcript(13.0), None);
        let known = TranscriptTimeMapping::raw_pts(10.0);
        for input in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(known.to_playback(input), None);
            assert_eq!(known.to_transcript(input), None);
        }
    }
}
