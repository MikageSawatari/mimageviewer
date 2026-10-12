//! Pure membership policy for rating publication and picker previews.
//!
//! The context owner validates write generations, source identity and non-rating
//! predicates before using a recipe. This module never revives a missing source.

use crate::app::ViewerPresentation;
use crate::grid_item::GridItem;
use std::collections::HashSet;

/// A successful picker preview may hold only a row that its rating would remove.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RatingPreviewRecipe {
    pub(crate) entries: Vec<RatingPreviewLeave>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RatingPreviewLeave {
    pub(crate) key: String,
    pub(crate) item: GridItem,
    pub(crate) before: u8,
    pub(crate) after: u8,
    pub(crate) write_generation: u64,
}

impl RatingPreviewRecipe {
    /// Uses real current stars for ordinary entrants. Only a validated preview
    /// leave may use its before value to retain an already adopted row.
    pub(crate) fn allows(&self, key: &str, current: u8, filter: &[bool; 6]) -> bool {
        filter.get(usize::from(current)).copied().unwrap_or(false)
            || self.entries.iter().any(|entry| {
                entry.key == key
                    && filter
                        .get(usize::from(entry.before))
                        .copied()
                        .unwrap_or(false)
            })
    }
}

/// Stable origin owned by a rating Undo entry; it does not own a viewer lifetime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatingOrigin {
    pub(crate) owner: u64,
    pub(crate) listing_identity: String,
    pub(crate) target: RatingOriginTarget,
}

/// A history entry owns its landing permission. Explicit grid navigation turns
/// Grid into ValueOnly without losing the durable rating edit or its context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RatingOriginTarget {
    ValueOnly,
    Grid(String),
    Viewer {
        key: String,
        presentation: ViewerPresentation,
    },
}

impl RatingOriginTarget {
    pub(crate) fn key(&self) -> Option<&String> {
        match self {
            Self::ValueOnly => None,
            Self::Grid(key) | Self::Viewer { key, .. } => Some(key),
        }
    }

    fn viewer_key(&self) -> Option<&String> {
        match self {
            Self::Viewer { key, .. } => Some(key),
            _ => None,
        }
    }
}

impl RatingOrigin {
    pub(crate) fn viewer_key(&self) -> Option<&String> {
        self.target.viewer_key()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RatingAdoptionIntent {
    Reconcile,
    RestoreOrigin(RatingOrigin),
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RatingNeighborDecision {
    Keep(String),
    Move(String),
    Empty,
}

/// Resolves neighbors without an available full canonical/Details order.
/// New entrants use the removed anchor's old ordinal, clamped to the new end.
pub(crate) fn reconcile_rating_neighbor(
    old_order: &[String],
    anchor: Option<&str>,
    current_order: &[String],
) -> RatingNeighborDecision {
    reconcile_rating_neighbor_with_basis(old_order, anchor, current_order, &[])
}

/// Resolves by identity: keep the anchor, then prefer next/previous old survivors.
/// When only entrants remain, their positions around the anchor in the full
/// canonical/Details basis determine next/previous. The caller chooses the
/// eligible domain; RTL does not reverse either order. Missing basis information
/// retains the ordinal/first fallback used by `reconcile_rating_neighbor`.
pub(crate) fn reconcile_rating_neighbor_with_basis(
    old_order: &[String],
    anchor: Option<&str>,
    current_order: &[String],
    basis_order: &[String],
) -> RatingNeighborDecision {
    if current_order.is_empty() {
        return RatingNeighborDecision::Empty;
    }
    let candidates: HashSet<&str> = current_order.iter().map(String::as_str).collect();
    if let Some(anchor) = anchor {
        if candidates.contains(anchor) {
            return RatingNeighborDecision::Keep(anchor.to_owned());
        }
        if let Some(position) = old_order.iter().position(|key| key == anchor) {
            let survivor = old_order[position + 1..]
                .iter()
                .chain(old_order[..position].iter().rev())
                .find(|key| candidates.contains(key.as_str()));
            if let Some(key) = survivor {
                return RatingNeighborDecision::Move(key.clone());
            }
        }
        if let Some(position) = basis_order.iter().position(|key| key == anchor) {
            let entrant = basis_order[position + 1..]
                .iter()
                .chain(basis_order[..position].iter().rev())
                .find(|key| candidates.contains(key.as_str()));
            if let Some(key) = entrant {
                return RatingNeighborDecision::Move(key.clone());
            }
        }
        if let Some(position) = old_order.iter().position(|key| key == anchor) {
            return RatingNeighborDecision::Move(
                current_order[position.min(current_order.len() - 1)].clone(),
            );
        }
    }
    RatingNeighborDecision::Move(current_order[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn recipe(key: &str, before: u8) -> RatingPreviewRecipe {
        RatingPreviewRecipe {
            entries: vec![RatingPreviewLeave {
                key: key.to_owned(),
                item: GridItem::Image(PathBuf::from(key)),
                before,
                after: 4,
                write_generation: 7,
            }],
        }
    }

    #[test]
    fn ordinary_membership_uses_each_current_rating() {
        let recipe = RatingPreviewRecipe::default();
        for stars in 0..=5 {
            let mut filter = [false; 6];
            filter[usize::from(stars)] = true;
            for current in 0..=5 {
                assert_eq!(recipe.allows("row", current, &filter), current == stars);
            }
        }
        assert!(!recipe.allows("row", 6, &[true; 6]));
    }

    #[test]
    fn preview_holds_only_matching_before_membership() {
        let preview = recipe("held", 3);
        let filter = [false, false, false, true, false, false];
        assert!(preview.allows("held", 4, &filter));
        assert!(preview.allows("new-hit", 3, &filter));
        assert!(!preview.allows("new-hit", 4, &filter));
        assert!(!preview.allows("held", 4, &[false; 6]));
        assert!(!recipe("held", 6).allows("held", 4, &filter));
    }

    #[test]
    fn ending_preview_removes_only_the_preview_leave() {
        let filter = [false, false, false, true, false, false];
        let preview = recipe("held", 3);
        let ordinary = RatingPreviewRecipe::default();
        assert!(preview.allows("held", 4, &filter));
        assert!(!ordinary.allows("held", 4, &filter));
        assert!(ordinary.allows("new-hit", 3, &filter));
        assert!(preview.allows("held", 3, &filter));
        assert!(ordinary.allows("held", 3, &filter));
    }

    #[test]
    fn multiple_preview_leaves_do_not_hold_unmatched_rows() {
        let mut preview = recipe("a", 0);
        preview.entries.extend(recipe("b", 3).entries);
        let filter = [true, false, false, true, false, false];
        assert!(preview.allows("a", 5, &filter));
        assert!(preview.allows("b", 4, &filter));
        assert!(!preview.allows("c", 4, &filter));
    }

    #[test]
    fn survivor_identity_is_kept_across_reorder() {
        assert_eq!(
            reconcile_rating_neighbor(&keys(&["a", "b", "c"]), Some("b"), &keys(&["c", "b"])),
            RatingNeighborDecision::Keep("b".into())
        );
    }

    #[test]
    fn next_old_survivor_wins_before_previous_and_new_entrants() {
        assert_eq!(
            reconcile_rating_neighbor(
                &keys(&["a", "b", "c", "d"]),
                Some("b"),
                &keys(&["new", "d", "a"]),
            ),
            RatingNeighborDecision::Move("d".into())
        );
    }

    #[test]
    fn previous_old_survivor_is_chosen_nearest_first() {
        assert_eq!(
            reconcile_rating_neighbor(
                &keys(&["a", "b", "c", "d"]),
                Some("d"),
                &keys(&["a", "new", "b"]),
            ),
            RatingNeighborDecision::Move("b".into())
        );
    }

    #[test]
    fn all_new_candidates_use_removed_anchor_insertion_position() {
        let old = keys(&["a", "b", "c"]);
        assert_eq!(
            reconcile_rating_neighbor(&old, Some("b"), &keys(&["x", "y", "z"])),
            RatingNeighborDecision::Move("y".into())
        );
        assert_eq!(
            reconcile_rating_neighbor(&old, Some("c"), &keys(&["x", "y"])),
            RatingNeighborDecision::Move("y".into())
        );
        assert_eq!(
            reconcile_rating_neighbor(&old, Some("a"), &keys(&["x"])),
            RatingNeighborDecision::Move("x".into())
        );
    }

    #[test]
    fn absent_anchor_uses_first_and_empty_domain_has_no_target() {
        for anchor in [None, Some("unknown")] {
            assert_eq!(
                reconcile_rating_neighbor(&keys(&["a", "b"]), anchor, &keys(&["x", "y"])),
                RatingNeighborDecision::Move("x".into())
            );
            assert_eq!(
                reconcile_rating_neighbor(&keys(&["a", "b"]), anchor, &[]),
                RatingNeighborDecision::Empty
            );
        }
        assert_eq!(
            reconcile_rating_neighbor(&keys(&["a"]), Some("a"), &[]),
            RatingNeighborDecision::Empty
        );
    }

    #[test]
    fn entrants_follow_full_basis_instead_of_old_visible_ordinal() {
        assert_eq!(
            reconcile_rating_neighbor_with_basis(
                &keys(&["anchor"]),
                Some("anchor"),
                &keys(&["previous", "next", "last"]),
                &keys(&["previous", "anchor", "next", "last"]),
            ),
            RatingNeighborDecision::Move("next".into())
        );
        assert_eq!(
            reconcile_rating_neighbor_with_basis(
                &keys(&["anchor"]),
                Some("anchor"),
                &keys(&["far", "near"]),
                &keys(&["far", "near", "anchor", "hidden"]),
            ),
            RatingNeighborDecision::Move("near".into())
        );
    }

    #[test]
    fn old_survivor_precedes_an_entrant_in_full_details_basis() {
        assert_eq!(
            reconcile_rating_neighbor_with_basis(
                &keys(&["previous", "anchor", "old-next"]),
                Some("anchor"),
                &keys(&["entrant", "old-next", "previous"]),
                &keys(&["previous", "anchor", "entrant", "old-next"]),
            ),
            RatingNeighborDecision::Move("old-next".into())
        );
    }

    #[test]
    fn basis_handles_unlisted_anchor_and_preserves_keep_empty_and_fallback() {
        let old = keys(&["a", "anchor", "b"]);
        let current = keys(&["x", "y"]);
        for basis in [keys(&[]), keys(&["x", "y"]), keys(&["anchor", "hidden"])] {
            assert_eq!(
                reconcile_rating_neighbor_with_basis(&old, Some("anchor"), &current, &basis),
                reconcile_rating_neighbor(&old, Some("anchor"), &current)
            );
        }
        assert_eq!(
            reconcile_rating_neighbor_with_basis(
                &[],
                Some("anchor"),
                &current,
                &keys(&["x", "anchor", "y"])
            ),
            RatingNeighborDecision::Move("y".into())
        );
        assert_eq!(
            reconcile_rating_neighbor_with_basis(
                &old,
                Some("anchor"),
                &keys(&["anchor", "x"]),
                &keys(&["x", "anchor"])
            ),
            RatingNeighborDecision::Keep("anchor".into())
        );
        assert_eq!(
            reconcile_rating_neighbor_with_basis(&old, Some("anchor"), &[], &old),
            RatingNeighborDecision::Empty
        );
    }
}
