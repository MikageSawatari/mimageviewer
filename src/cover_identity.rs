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
    pub(crate) fn to_sql(&self, catalog_drive: Option<char>) -> Option<String> {
        match self {
            Self::Legacy => None,
            Self::Unknown => Some("u".to_owned()),
            Self::Unrotatable => Some("n".to_owned()),
            Self::Page(key) => {
                // Only the catalog's own drive can follow its drive-letter changes.
                // External sources (for example a converted ZIP cache) stay absolute.
                let bytes = key.as_bytes();
                if bytes.len() >= 2
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && catalog_drive
                        .is_some_and(|drive| char::from(bytes[0]).eq_ignore_ascii_case(&drive))
                {
                    Some(format!("q:{}", &key[2..]))
                } else {
                    Some(format!("p:{key}"))
                }
            }
        }
    }

    pub(crate) fn from_sql(value: Option<&str>, drive: Option<char>) -> Self {
        match value {
            None => Self::Legacy,
            Some("u") => Self::Unknown,
            Some("n") => Self::Unrotatable,
            Some(value) if value.starts_with("q:") => drive
                .map(|drive| Self::Page(format!("{drive}:{}", &value[2..])))
                // This is current material even when its drive context is unavailable.
                .unwrap_or(Self::Unknown),
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
                Some("q:/本.zip::001.png"),
            ),
        ] {
            assert_eq!(cover.to_sql(Some('c')).as_deref(), encoded);
            assert_eq!(CoverIdentity::from_sql(encoded, Some('c')), cover);
        }
    }

    #[test]
    fn catalog_drive_encoding_rebases_only_matching_drive_pages() {
        let cover = CoverIdentity::Page("d:/本.zip::nested/001.png".into());
        let encoded = cover.to_sql(Some('D'));
        assert_eq!(encoded.as_deref(), Some("q:/本.zip::nested/001.png"));
        assert_eq!(
            CoverIdentity::from_sql(encoded.as_deref(), Some('e')),
            CoverIdentity::Page("e:/本.zip::nested/001.png".into())
        );
        assert_eq!(
            CoverIdentity::from_sql(encoded.as_deref(), None),
            CoverIdentity::Unknown
        );
        assert_eq!(
            cover,
            CoverIdentity::Page("d:/本.zip::nested/001.png".into())
        );
        for drive in [None, Some('c')] {
            let encoded = cover.to_sql(drive);
            assert_eq!(encoded.as_deref(), Some("p:d:/本.zip::nested/001.png"));
            assert_eq!(
                CoverIdentity::from_sql(encoded.as_deref(), Some('e')),
                cover
            );
            assert_eq!(CoverIdentity::from_sql(encoded.as_deref(), None), cover);
        }
        for key in [
            "//server/share/本.zip::001.png",
            "/books/book.pdf::page_0000",
        ] {
            let cover = CoverIdentity::Page(key.into());
            let encoded = cover.to_sql(Some('d'));
            assert_eq!(encoded, Some(format!("p:{key}")));
            assert_eq!(
                CoverIdentity::from_sql(encoded.as_deref(), Some('e')),
                cover
            );
            assert_eq!(CoverIdentity::from_sql(encoded.as_deref(), None), cover);
        }
    }

    #[test]
    fn unrecognized_sql_encoding_is_current_unknown_material() {
        assert_eq!(
            CoverIdentity::from_sql(Some("future:value"), None),
            CoverIdentity::Unknown
        );
    }
}
