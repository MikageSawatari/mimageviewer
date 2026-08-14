//! Content identity for grid items, independent of their position in the list.
//!
//! An index says where an item sits; it says nothing about what the item is. Move to the next PDF
//! and index 0 is a different page, so every cache keyed by index alone can hand back the previous
//! document's content. `items_generation` already guards against that, and is sufficient *if* the
//! generation is bumped on every change that reassigns what an index means, and *if* nothing
//! stamps a newer generation onto an older result.
//!
//! Both of those held right up until they did not: on 2026-08-14 an AI upscale started while one
//! document was open landed after the switch to the next one and was presented as that document's
//! first page. Generation alone cannot catch it, because by the time the result arrives the
//! generation genuinely is the new one.
//!
//! So identity is tracked in parallel, and the two catch different failures:
//!
//! | check | catches |
//! |---|---|
//! | `items_generation` | the list was replaced |
//! | `ItemId` | the generation did not change but the item did, or a result adopted a key that is not its own |
//!
//! The cost is deliberately near zero so it can be applied redundantly rather than only where a
//! failure has already been proven: ids are interned once per item when a list is installed, and
//! every later comparison is a single `u64`. Keys stay `Copy`, nothing is allocated on a lookup,
//! and no string is hashed on a display path.
//!
//! Ids are interned by content, not by position, so the same page reopened later gets the same id.
//! That is what makes the check meaningful across a list change rather than a restatement of
//! `(generation, idx)`. A monotonic counter is used rather than a hash of the key: a hash can
//! collide, and a collision here would silently reinstate exactly the bug this exists to prevent.

use std::collections::HashMap;

/// Identifies *what* an item is, for as long as the process lives.
///
/// `NONE` is for slots that have no stable identity yet (an item that has not been interned, or a
/// key built for a page that is no longer in the list). It never compares equal to a real id, so
/// an entry carrying it fails the check rather than passing it by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub(crate) struct ItemId(u64);

impl ItemId {
    pub(crate) const NONE: Self = Self(0);

    pub(crate) const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_none() {
            f.write_str("none")
        } else {
            write!(f, "#{}", self.0)
        }
    }
}

/// Hands out one [`ItemId`] per distinct item key, stable for the process.
///
/// Interning happens when a list is installed - O(n) work inside an operation that is already
/// O(n) - so display paths only ever compare integers.
#[derive(Debug, Default)]
pub(crate) struct ItemIdInterner {
    ids: HashMap<String, ItemId>,
    next: u64,
}

impl ItemIdInterner {
    pub(crate) fn intern(&mut self, key: &str) -> ItemId {
        if let Some(id) = self.ids.get(key) {
            return *id;
        }
        self.next += 1;
        let id = ItemId(self.next);
        self.ids.insert(key.to_owned(), id);
        id
    }

    /// How many distinct items have been seen. Only for diagnostics.
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_keeps_its_identity_and_different_keys_never_share_one() {
        let mut interner = ItemIdInterner::default();
        let a = interner.intern("pdf::a.pdf#0");
        let b = interner.intern("pdf::b.pdf#0");
        assert_ne!(a, b, "two documents' first pages are not the same item");
        assert_eq!(
            a,
            interner.intern("pdf::a.pdf#0"),
            "identity is by content, so reopening a page must recover its id - otherwise the \
             check degenerates into a restatement of the list generation"
        );
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn the_absent_id_never_matches_a_real_one() {
        let mut interner = ItemIdInterner::default();
        let real = interner.intern("pdf::a.pdf#0");
        assert!(ItemId::NONE.is_none());
        assert!(!real.is_none());
        assert_ne!(
            ItemId::NONE,
            real,
            "an entry with no identity must fail the check rather than pass it by default"
        );
    }
}
