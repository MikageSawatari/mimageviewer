use std::collections::HashSet;

use crate::collection_store::CollectionEntryId;

/// Replace only slots occupied by the original visible IDs. Both visible orders must be
/// duplicate-free and contain exactly the same subset of the full registration order.
pub(super) fn merge_visible_order(
    full: &[CollectionEntryId],
    visible: &HashSet<CollectionEntryId>,
    reordered: &[CollectionEntryId],
) -> Option<Vec<CollectionEntryId>> {
    let full_set = full.iter().copied().collect::<HashSet<_>>();
    let reordered_set = reordered.iter().copied().collect::<HashSet<_>>();
    if full_set.len() != full.len()
        || !visible.is_subset(&full_set)
        || reordered_set.len() != reordered.len()
        || &reordered_set != visible
    {
        return None;
    }
    let mut replacements = reordered.iter().copied();
    Some(
        full.iter()
            .map(|id| {
                if visible.contains(id) {
                    replacements.next().unwrap()
                } else {
                    *id
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_hidden_slots_and_all_ids() {
        let full = (0..5).map(|_| CollectionEntryId::new()).collect::<Vec<_>>();
        let visible = HashSet::from([full[0], full[2], full[4]]);
        assert_eq!(
            merge_visible_order(&full, &visible, &[full[4], full[0], full[2]]),
            Some(vec![full[4], full[1], full[0], full[3], full[2]])
        );
        assert_eq!(
            merge_visible_order(&full, &HashSet::new(), &[]),
            Some(full.clone())
        );
    }

    #[test]
    fn merge_rejects_duplicate_missing_and_foreign_ids() {
        let full = vec![CollectionEntryId::new(), CollectionEntryId::new()];
        let visible = HashSet::from([full[0], full[1]]);
        assert!(merge_visible_order(&full, &visible, &[full[0], full[0]]).is_none());
        assert!(merge_visible_order(&full, &visible, &[full[0]]).is_none());
        assert!(
            merge_visible_order(&full, &visible, &[full[0], CollectionEntryId::new()]).is_none()
        );
        assert!(merge_visible_order(&[full[0], full[0]], &visible, &full).is_none());
    }
}
