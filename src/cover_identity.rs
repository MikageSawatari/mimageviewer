/// Identifies the page whose unrotated pixels are stored in a thumbnail.
/// Rotation is resolved by the consumer, independently of the cached raster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverIdentity {
    /// A thumbnail written before cover identities were recorded.
    Legacy,
    /// A current thumbnail whose source page could not be identified.
    Unknown,
    /// Material without page rotation, such as audio artwork or an icon.
    Unrotatable,
    /// The canonical page key used by the rotation store.
    Page(String),
}

impl CoverIdentity {
    pub(crate) fn to_sql(&self) -> Option<String> {
        match self {
            Self::Legacy => None,
            Self::Unknown => Some("u".to_owned()),
            Self::Unrotatable => Some("n".to_owned()),
            Self::Page(key) => Some(format!("p:{key}")),
        }
    }

    pub(crate) fn from_sql(value: Option<&str>) -> Self {
        match value {
            None => Self::Legacy,
            Some("u") => Self::Unknown,
            Some("n") => Self::Unrotatable,
            Some(value) => value
                .strip_prefix("p:")
                .map(|key| Self::Page(key.to_owned()))
                // Unknown encodings must not turn into perpetually refreshing legacy rows.
                .unwrap_or(Self::Unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_encoding_round_trips_every_cover_identity() {
        for (cover, encoded) in [
            (CoverIdentity::Legacy, None),
            (CoverIdentity::Unknown, Some("u")),
            (CoverIdentity::Unrotatable, Some("n")),
            (
                CoverIdentity::Page("c:/本.zip::001.png".into()),
                Some("p:c:/本.zip::001.png"),
            ),
        ] {
            assert_eq!(cover.to_sql().as_deref(), encoded);
            assert_eq!(CoverIdentity::from_sql(encoded), cover);
        }
    }

    #[test]
    fn unrecognized_sql_encoding_is_current_unknown_material() {
        assert_eq!(
            CoverIdentity::from_sql(Some("future:value")),
            CoverIdentity::Unknown
        );
    }
}
