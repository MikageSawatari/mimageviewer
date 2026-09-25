//! Internal rating-order comparison. Producers attach a key to each row before sorting.

use std::cmp::Ordering;

use crate::filename_sort::SortNameKey;

/// Transient order captured by a listing producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListingOrderRequest {
    Standard(crate::settings::SortOrder),
    Rating(RatingSortSpec),
}

impl ListingOrderRequest {
    pub(crate) fn standard_fallback(self) -> crate::settings::SortOrder {
        match self {
            Self::Standard(sort) => sort,
            Self::Rating(_) => crate::settings::SortOrder::FileName,
        }
    }
    pub(crate) fn from_settings(settings: &crate::settings::Settings) -> Self {
        #[cfg(test)]
        if let Some(spec) = TEST_RATING_ORDER.with(|order| order.get()) {
            return Self::Rating(spec);
        }
        Self::from_sort(settings.sort_order, settings.rating_sort_unrated_position)
    }

    pub(crate) fn from_sort(
        sort: crate::settings::SortOrder,
        unrated_position: RatingSortUnratedPosition,
    ) -> Self {
        match sort {
            crate::settings::SortOrder::RatingAsc => Self::Rating(RatingSortSpec {
                direction: RatingSortDirection::Asc,
                unrated_position,
            }),
            crate::settings::SortOrder::RatingDesc => Self::Rating(RatingSortSpec {
                direction: RatingSortDirection::Desc,
                unrated_position,
            }),
            sort => Self::Standard(sort),
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_RATING_ORDER: std::cell::Cell<Option<RatingSortSpec>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_test_rating_order<T>(spec: RatingSortSpec, f: impl FnOnce() -> T) -> T {
    TEST_RATING_ORDER.with(|order| {
        let previous = order.replace(Some(spec));
        struct Restore<'a>(
            &'a std::cell::Cell<Option<RatingSortSpec>>,
            Option<RatingSortSpec>,
        );
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(order, previous);
        f()
    })
}

/// Rating identity for a physical listing row; synthetic container keys need
/// their owning viewer and are deliberately outside this producer.
pub(crate) fn physical_item_key(item: &crate::grid_item::GridItem) -> Option<String> {
    use crate::grid_item::GridItem;
    if !item.accepts_rating() {
        return None;
    }
    match item {
        GridItem::Image(_) => crate::edit_source::page_key_for_grid_item(item),
        GridItem::Folder(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::Video(path)
        | GridItem::Audio(path) => Some(crate::adjustment_db::normalize_path(path)),
        GridItem::ConvertibleArchive { path, .. } => {
            Some(crate::adjustment_db::normalize_path(path))
        }
        _ => None,
    }
}

/// `Supported(Zero)` is an unrated item; `Unsupported` is outside the ratable domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RatingSortKey {
    Supported(RatingStars),
    Unsupported,
}

/// A supported key cannot carry an out-of-range star count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RatingStars {
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
}

impl RatingSortKey {
    pub(crate) fn supported(stars: u8) -> Option<Self> {
        let stars = match stars {
            0 => RatingStars::Zero,
            1 => RatingStars::One,
            2 => RatingStars::Two,
            3 => RatingStars::Three,
            4 => RatingStars::Four,
            5 => RatingStars::Five,
            _ => return None,
        };
        Some(Self::Supported(stars))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RatingSortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RatingSortUnratedPosition {
    #[default]
    BetweenThreeAndTwo,
    BelowAll,
}

/// Captured with a prepare request so later setting changes cannot alter its comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RatingSortSpec {
    pub(crate) direction: RatingSortDirection,
    pub(crate) unrated_position: RatingSortUnratedPosition,
}

impl RatingSortSpec {
    /// Compares only rows within the same folder/media category. The caller owns
    /// category order and keeps each rating key attached to its row.
    pub(crate) fn compare(
        self,
        rating_a: RatingSortKey,
        name_a: &SortNameKey,
        rating_b: RatingSortKey,
        name_b: &SortNameKey,
    ) -> Ordering {
        self.rank(rating_a)
            .cmp(&self.rank(rating_b))
            .then_with(|| name_a.compare_file_name(name_b))
    }

    fn rank(self, key: RatingSortKey) -> u8 {
        let desc_rank = match key {
            RatingSortKey::Unsupported => return 6,
            RatingSortKey::Supported(stars) => match (self.unrated_position, stars) {
                (_, RatingStars::Five) => 0,
                (_, RatingStars::Four) => 1,
                (_, RatingStars::Three) => 2,
                (RatingSortUnratedPosition::BetweenThreeAndTwo, RatingStars::Zero) => 3,
                (RatingSortUnratedPosition::BetweenThreeAndTwo, RatingStars::Two) => 4,
                (RatingSortUnratedPosition::BetweenThreeAndTwo, RatingStars::One) => 5,
                (RatingSortUnratedPosition::BelowAll, RatingStars::Two) => 3,
                (RatingSortUnratedPosition::BelowAll, RatingStars::One) => 4,
                (RatingSortUnratedPosition::BelowAll, RatingStars::Zero) => 5,
            },
        };
        match self.direction {
            RatingSortDirection::Desc => desc_rank,
            RatingSortDirection::Asc => 5 - desc_rank,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_sort_selection_captures_direction_and_unrated_position() {
        let mut settings = crate::settings::Settings::default();
        assert_eq!(
            ListingOrderRequest::from_settings(&settings),
            ListingOrderRequest::Standard(crate::settings::SortOrder::FileName)
        );
        settings.sort_order = crate::settings::SortOrder::RatingDesc;
        assert_eq!(
            ListingOrderRequest::from_settings(&settings),
            ListingOrderRequest::Rating(RatingSortSpec {
                direction: RatingSortDirection::Desc,
                unrated_position: RatingSortUnratedPosition::BetweenThreeAndTwo,
            })
        );
        settings.sort_order = crate::settings::SortOrder::RatingAsc;
        settings.rating_sort_unrated_position = RatingSortUnratedPosition::BelowAll;
        assert_eq!(
            ListingOrderRequest::from_settings(&settings),
            ListingOrderRequest::Rating(RatingSortSpec {
                direction: RatingSortDirection::Asc,
                unrated_position: RatingSortUnratedPosition::BelowAll,
            })
        );
    }

    #[test]
    fn rating_order_table_and_unsupported() {
        let cases = [
            (
                RatingSortUnratedPosition::BetweenThreeAndTwo,
                RatingSortDirection::Desc,
                [5, 4, 3, 0, 2, 1],
            ),
            (
                RatingSortUnratedPosition::BetweenThreeAndTwo,
                RatingSortDirection::Asc,
                [1, 2, 0, 3, 4, 5],
            ),
            (
                RatingSortUnratedPosition::BelowAll,
                RatingSortDirection::Desc,
                [5, 4, 3, 2, 1, 0],
            ),
            (
                RatingSortUnratedPosition::BelowAll,
                RatingSortDirection::Asc,
                [0, 1, 2, 3, 4, 5],
            ),
        ];
        let name = SortNameKey::file_name("same.jpg");
        for (unrated_position, direction, expected) in cases {
            let spec = RatingSortSpec {
                direction,
                unrated_position,
            };
            for (left_pos, &left) in expected.iter().enumerate() {
                for (right_pos, &right) in expected.iter().enumerate() {
                    assert_eq!(
                        spec.compare(
                            RatingSortKey::supported(left).unwrap(),
                            &name,
                            RatingSortKey::supported(right).unwrap(),
                            &name,
                        ),
                        left_pos.cmp(&right_pos),
                        "{unrated_position:?} {direction:?}: {left} vs {right}",
                    );
                }
                assert_eq!(
                    spec.compare(
                        RatingSortKey::supported(left).unwrap(),
                        &name,
                        RatingSortKey::Unsupported,
                        &name,
                    ),
                    Ordering::Less,
                );
            }
            assert_eq!(
                spec.compare(
                    RatingSortKey::Unsupported,
                    &name,
                    RatingSortKey::supported(0).unwrap(),
                    &name,
                ),
                Ordering::Greater,
            );
        }
        assert_eq!(
            RatingSortUnratedPosition::default(),
            RatingSortUnratedPosition::BetweenThreeAndTwo,
        );
    }

    #[test]
    fn equal_rating_and_unsupported_ties_use_file_name_ascending() {
        let alpha = SortNameKey::file_name("a.jpg");
        let zulu = SortNameKey::file_name("z.jpg");
        for unrated_position in [
            RatingSortUnratedPosition::BetweenThreeAndTwo,
            RatingSortUnratedPosition::BelowAll,
        ] {
            for direction in [RatingSortDirection::Asc, RatingSortDirection::Desc] {
                let spec = RatingSortSpec {
                    direction,
                    unrated_position,
                };
                for key in [
                    RatingSortKey::supported(0).unwrap(),
                    RatingSortKey::supported(4).unwrap(),
                    RatingSortKey::Unsupported,
                ] {
                    assert_eq!(spec.compare(key, &alpha, key, &zulu), Ordering::Less);
                    assert_eq!(spec.compare(key, &zulu, key, &alpha), Ordering::Greater);
                }
            }
        }
    }

    #[test]
    fn supported_key_rejects_out_of_range_stars() {
        for stars in 0..=5 {
            assert!(RatingSortKey::supported(stars).is_some());
        }
        assert_eq!(RatingSortKey::supported(6), None);
        assert_eq!(RatingSortKey::supported(u8::MAX), None);
    }
}
