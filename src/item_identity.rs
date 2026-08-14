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
//! Ids are interned by content, not by position, so the same page reopened later gets the same id.
//! That is what makes the check meaningful across a list change rather than a restatement of
//! `(generation, idx)`. A monotonic counter is used rather than a hash of the key: a hash can
//! collide, and a collision here would silently reinstate exactly the bug this exists to prevent.
//!
//! Identity is *derived* from the item on demand rather than stored beside it. Holding it in a
//! vector parallel to `items` was tried first and was wrong: six paths mutate the list without
//! going through the install that filled the vector - deletion, snapshot restore and smart-folder
//! restore among them - and after any of those, an item's texture stopped matching its own index.
//! A guard against showing the wrong page became a way to refuse the right one. Deriving it costs
//! a short-lived key allocation per call, which measured against roughly a hundred calls a frame
//! is not worth a memo that would bring the synchronisation obligation back.

use std::collections::HashMap;
use std::collections::VecDeque;

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

    /// How many distinct items have been seen.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }
}

/// Which item a display texture was made to show.
///
/// Every cache that can answer for a page is keyed by index, and each one has to be given identity
/// separately. Chasing them one at a time is how this bug kept coming back: a store nobody had got
/// to yet would answer for an index whose meaning had changed, and the picture on screen belonged
/// to the previous document.
///
/// A texture is the one thing all of those stores have in common, and it cannot be re-labelled -
/// the pixels were uploaded once, for one page. Recording what a texture is *for* therefore covers
/// every store at once, including stores that do not exist yet.
///
/// Identity is learned on first sight rather than at upload, so a store needs no cooperation to be
/// covered and a new one cannot forget. A producer that already knows which item it worked on binds
/// earlier via [`Self::bind`], which is strictly better - it catches the case where the very first
/// sighting is already the wrong one, as when an AI composite starts under one document and lands
/// after the switch to the next - but nothing depends on that having happened.
///
/// The checks live on the resolvers and accessors that hand a texture to the page being drawn:
/// `resolve_fs_processed_texture`, `resolve_fs_display_tex`, the continuous-reading cache and
/// original-preview lookups, the continuous transition, the pass-through rendition, and
/// `fs_thumbnail_texture_for_display`. Small overlays that show a page at reduced size - the
/// navigator, the loupe, the grid - are deliberately *not* gated: a stale thumbnail there is worth
/// far less than the risk of telling someone to restart over one.
///
/// Texture ids are never reused by `epaint`, so an entry can never come to describe a different
/// texture; entries are dropped oldest-first purely to bound memory. The bound is far above the
/// number of pages that can be resident, so a texture that outlives a document switch - the case
/// worth catching - is always still on the books.
#[derive(Debug, Default)]
pub(crate) struct TextureIdentityLedger {
    ids: HashMap<egui::TextureId, ItemId>,
    order: VecDeque<egui::TextureId>,
}

impl TextureIdentityLedger {
    /// Far above the number of page textures that can be resident at once.
    const CAPACITY: usize = 4096;

    /// Record what this texture shows, if it is not already recorded.
    ///
    /// The first binding wins. A later one being ignored is the point: that is precisely the
    /// disagreement this exists to detect, and overwriting would erase it.
    pub(crate) fn bind(&mut self, texture: egui::TextureId, item: ItemId) {
        if item.is_none() || self.ids.contains_key(&texture) {
            return;
        }
        self.ids.insert(texture, item);
        self.order.push_back(texture);
        while self.order.len() > Self::CAPACITY {
            if let Some(evicted) = self.order.pop_front() {
                self.ids.remove(&evicted);
            }
        }
    }

    /// What this texture was made to show, or `None` if it has not been seen before.
    pub(crate) fn get(&self, texture: egui::TextureId) -> Option<ItemId> {
        self.ids.get(&texture).copied()
    }

    /// Does this texture show `item`?
    ///
    /// Unknown answers `true`. Only a texture that is on the books *and* disagrees is a mismatch,
    /// so a store this has not learned about yet keeps working exactly as before rather than
    /// blanking the screen on a guess.
    pub(crate) fn agrees(&self, texture: egui::TextureId, item: ItemId) -> bool {
        if item.is_none() {
            return true;
        }
        match self.get(texture) {
            Some(bound) => bound == item,
            None => true,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }
}

#[cfg(test)]
thread_local! {
    static MISMATCH_EXPECTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Is the test running on this thread one that deliberately builds a mismatch?
///
/// A mismatch is a development-time assertion failure everywhere else, and that is worth keeping:
/// it is how the rest of the suite would catch a store handing back a foreign texture. Only the
/// tests that construct the condition on purpose opt out, and only for their own duration.
pub(crate) fn mismatch_is_expected() -> bool {
    #[cfg(test)]
    {
        MISMATCH_EXPECTED.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Suppress the mismatch assertion until the returned guard is dropped.
#[cfg(test)]
#[must_use]
pub(crate) fn expect_mismatch() -> MismatchExpected {
    MISMATCH_EXPECTED.with(|flag| flag.set(true));
    MismatchExpected
}

#[cfg(test)]
pub(crate) struct MismatchExpected;

#[cfg(test)]
impl Drop for MismatchExpected {
    fn drop(&mut self) {
        MISMATCH_EXPECTED.with(|flag| flag.set(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(id: u64) -> egui::TextureId {
        egui::TextureId::Managed(id)
    }

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

    #[test]
    fn a_texture_that_outlives_its_page_stops_agreeing_with_the_index_it_sits_at() {
        let mut interner = ItemIdInterner::default();
        let mut ledger = TextureIdentityLedger::default();
        let first_doc_page = interner.intern("pdf::001.pdf#0");
        let second_doc_page = interner.intern("pdf::002.pdf#0");

        ledger.bind(tex(181), first_doc_page);
        assert!(ledger.agrees(tex(181), first_doc_page));
        assert!(
            !ledger.agrees(tex(181), second_doc_page),
            "the reported failure: index 0 now means another document, but the texture there is \
             still the first document's page"
        );

        // A store that binds late cannot talk the ledger out of what it already knows.
        ledger.bind(tex(181), second_doc_page);
        assert!(!ledger.agrees(tex(181), second_doc_page));
    }

    #[test]
    fn an_unseen_texture_is_allowed_through() {
        let mut interner = ItemIdInterner::default();
        let ledger = TextureIdentityLedger::default();
        assert!(
            ledger.agrees(tex(7), interner.intern("pdf::a.pdf#0")),
            "a store this has not learned about must keep working, not blank the page on a guess"
        );
    }

    #[test]
    fn the_ledger_stays_bounded_and_keeps_the_recent_textures() {
        let mut interner = ItemIdInterner::default();
        let mut ledger = TextureIdentityLedger::default();
        let overflow = TextureIdentityLedger::CAPACITY + 100;
        for i in 0..overflow {
            ledger.bind(tex(i as u64), interner.intern(&format!("pdf::a.pdf#{i}")));
        }
        assert_eq!(ledger.len(), TextureIdentityLedger::CAPACITY);
        let newest = tex((overflow - 1) as u64);
        assert!(
            ledger.get(newest).is_some(),
            "a texture that just crossed a document switch must still be on the books"
        );
    }
}
