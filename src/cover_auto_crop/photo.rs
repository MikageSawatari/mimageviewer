//! Shooting metadata supplied by the existing analysis decode (G1 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhotoLikeReason {
    ShotExif,
}

impl PhotoLikeReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ShotExif => "shot-exif",
        }
    }
}

pub(crate) fn photo_like_reason(shot_exif: bool) -> Option<PhotoLikeReason> {
    shot_exif.then_some(PhotoLikeReason::ShotExif)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_auto_photo_only_shooting_metadata_is_evidence() {
        assert_eq!(photo_like_reason(true), Some(PhotoLikeReason::ShotExif));
        assert_eq!(photo_like_reason(false), None);
    }
}
