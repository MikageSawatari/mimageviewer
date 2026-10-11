use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The sole mutation capability for one viewer payload. Moving it preserves
/// identity; cloning a ticket never grants invalidation authority.
#[derive(Debug)]
pub struct PdfRenderEpochOwner {
    state: Arc<AtomicU64>,
}

impl Default for PdfRenderEpochOwner {
    fn default() -> Self {
        Self::fresh()
    }
}

impl PdfRenderEpochOwner {
    pub fn fresh() -> Self {
        Self {
            state: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn snapshot(&self) -> PdfRenderEpochTicket {
        PdfRenderEpochTicket {
            state: Arc::clone(&self.state),
            generation: self.state.load(Ordering::Acquire),
        }
    }

    pub fn advance(&mut self) -> PdfRenderEpochTicket {
        let generation = self.state.load(Ordering::Relaxed);
        if generation != 0 {
            // Zero is terminal, and wrapping must never revive an old ticket.
            self.state
                .store(generation.checked_add(1).unwrap_or(0), Ordering::Release);
            super::wake_render_epoch_maintenance();
        }
        self.snapshot()
    }

    pub fn retire(&mut self) {
        if self.state.swap(0, Ordering::AcqRel) != 0 {
            super::wake_render_epoch_maintenance();
        }
    }
}

impl Drop for PdfRenderEpochOwner {
    fn drop(&mut self) {
        self.retire();
    }
}

#[derive(Debug, Clone)]
pub struct PdfRenderEpochTicket {
    state: Arc<AtomicU64>,
    generation: u64,
}

impl PdfRenderEpochTicket {
    pub fn is_current(&self) -> bool {
        self.generation != 0 && self.state.load(Ordering::Acquire) == self.generation
    }

    pub fn is_stale(&self) -> bool {
        !self.is_current()
    }

    /// Identity comparison also works on stale tickets for tracking/dedup.
    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state) && self.generation == other.generation
    }
}

#[derive(Debug, Default, Clone)]
pub enum PdfRenderValidity {
    Viewer(PdfRenderEpochTicket),
    #[default]
    Unscoped,
}

impl PdfRenderValidity {
    pub fn is_stale(&self) -> bool {
        match self {
            Self::Viewer(ticket) => ticket.is_stale(),
            Self::Unscoped => false,
        }
    }

    pub fn is_current(&self) -> bool {
        !self.is_stale()
    }

    pub fn same_request(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Viewer(a), Self::Viewer(b)) => a.same_request(b),
            (Self::Unscoped, Self::Unscoped) => true,
            _ => false,
        }
    }

    pub(super) fn matches_current(&self, target: &Self) -> bool {
        self.same_request(target) && self.is_current() && target.is_current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_with_same_generation_are_independent() {
        let mut a = PdfRenderEpochOwner::fresh();
        let b = PdfRenderEpochOwner::default();
        let old = a.snapshot();
        let sibling = b.snapshot();
        assert!(!old.same_request(&sibling));
        let current = a.advance();
        assert!(old.is_stale());
        assert!(current.is_current());
        assert!(sibling.is_current());
        a.retire();
        assert!(current.is_stale());
        assert!(a.advance().is_stale());
        assert!(sibling.is_current());
    }

    #[test]
    fn ticket_clones_do_not_retire_owner_but_owner_drop_does() {
        let owner = PdfRenderEpochOwner::fresh();
        let ticket = owner.snapshot();
        let clone = ticket.clone();
        assert!(ticket.same_request(&clone));
        drop(clone);
        assert!(ticket.is_current());
        drop(owner);
        assert!(ticket.is_stale());
        assert!(PdfRenderValidity::default().is_current());
    }
}
