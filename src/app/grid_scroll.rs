//! Context-owned canonical grid position and the two existing request lifetimes.
//!
//! A reveal can run before a winning edge request in the same layout. The independent
//! row anchor is consumed only by a reveal with a resolved selection, never by an edge
//! or a failed selection lookup. Native readback must not issue another position request.

use super::GridScrollIntent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GridScrollPlacement {
    #[default]
    KeepPosition,
    RevealSelection,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GridScrollRequest {
    #[default]
    Keep,
    Position {
        placement: GridScrollPlacement,
    },
    RevealSelection,
    Edge {
        edge: GridScrollIntent,
        placement: GridScrollPlacement,
    },
}

impl GridScrollRequest {
    fn placement(self) -> GridScrollPlacement {
        match self {
            Self::Position { placement } | Self::Edge { placement, .. } => placement,
            Self::RevealSelection => GridScrollPlacement::RevealSelection,
            Self::Keep => GridScrollPlacement::KeepPosition,
        }
    }

    pub(crate) fn edge(self) -> Option<GridScrollIntent> {
        match self {
            Self::Edge { edge, .. } => Some(edge),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowAnchor(u32);

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GridScrollState {
    offset_y: f32,
    request: GridScrollRequest,
    row_anchor: Option<RowAnchor>,
}

impl GridScrollState {
    pub(crate) fn offset(&self) -> f32 {
        self.offset_y
    }

    /// Explicit App movement/restoration, including restoration to the same value.
    /// An outstanding edge still wins; a prior reveal is composed with the position.
    pub(crate) fn set_position(&mut self, offset: f32) {
        self.offset_y = offset;
        self.request = match self.request {
            edge @ GridScrollRequest::Edge { .. } => edge,
            request => GridScrollRequest::Position {
                placement: request.placement(),
            },
        };
    }

    /// Adopt geometry/gesture/native readback without producing a next-layout request.
    pub(crate) fn adopt_offset(&mut self, offset: f32) {
        self.offset_y = offset;
    }

    pub(crate) fn reveal_requested(&self) -> bool {
        self.request.placement() == GridScrollPlacement::RevealSelection
    }

    pub(crate) fn set_reveal_requested(&mut self, reveal: bool) {
        let placement = if reveal {
            GridScrollPlacement::RevealSelection
        } else {
            GridScrollPlacement::KeepPosition
        };
        self.request = match self.request {
            GridScrollRequest::Position { .. } => GridScrollRequest::Position { placement },
            GridScrollRequest::Edge { edge, .. } => GridScrollRequest::Edge { edge, placement },
            _ if reveal => GridScrollRequest::RevealSelection,
            _ => GridScrollRequest::Keep,
        };
    }

    /// Preserve the renderer's existing early reveal claim. An edge is claimed later,
    /// after geometry, and a request produced during rendering belongs to the next layout.
    pub(crate) fn take_reveal(&mut self) -> bool {
        let reveal = self.reveal_requested();
        self.set_reveal_requested(false);
        reveal
    }

    pub(crate) fn request_edge(&mut self, edge: GridScrollIntent) {
        self.request = GridScrollRequest::Edge {
            edge,
            placement: self.request.placement(),
        };
    }

    pub(crate) fn pending_edge(&self) -> Option<GridScrollIntent> {
        self.request.edge()
    }

    pub(crate) fn clear_edge(&mut self) {
        if let GridScrollRequest::Edge { placement, .. } = self.request {
            self.request = GridScrollRequest::Position { placement };
        }
    }

    pub(crate) fn take_request(&mut self) -> GridScrollRequest {
        std::mem::take(&mut self.request)
    }

    /// The renderer already claimed this frame's reveal at entry. A reveal generated
    /// afterwards (for example by a header action) must survive the later edge claim.
    pub(crate) fn take_layout_request(&mut self) -> GridScrollRequest {
        let request = self.take_request();
        if request.placement() == GridScrollPlacement::RevealSelection {
            self.request = GridScrollRequest::RevealSelection;
        }
        request
    }

    pub(crate) fn row_anchor(&self) -> Option<u32> {
        self.row_anchor.map(|anchor| anchor.0)
    }
    pub(crate) fn set_row_anchor(&mut self, rows_above: Option<u32>) {
        self.row_anchor = rows_above.map(RowAnchor);
    }
    pub(crate) fn take_row_anchor(&mut self) -> Option<u32> {
        self.row_anchor.take().map(|anchor| anchor.0)
    }

    /// Existing forks copy grid position/reveal/edge, but never process-start cursor hints.
    /// Context moves and swaps transfer the whole owner and keep their unconsumed anchor.
    pub(crate) fn fork(&self) -> Self {
        Self {
            offset_y: self.offset_y,
            request: self.request,
            row_anchor: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_scroll_position_and_reveal_compose_in_both_orders() {
        for restore_first in [true, false] {
            let mut state = GridScrollState::default();
            if restore_first {
                state.set_position(240.0);
                state.set_reveal_requested(true);
            } else {
                state.set_reveal_requested(true);
                state.set_position(240.0);
            }
            assert_eq!(state.offset(), 240.0);
            assert_eq!(
                state.take_request(),
                GridScrollRequest::Position {
                    placement: GridScrollPlacement::RevealSelection,
                }
            );
            assert_eq!(state.take_request(), GridScrollRequest::Keep);
        }
    }

    #[test]
    fn grid_scroll_edge_wins_without_losing_real_reveal_or_dormant_anchor() {
        let mut state = GridScrollState::default();
        state.set_row_anchor(Some(3));
        state.request_edge(GridScrollIntent::Top);
        state.set_position(120.0);
        state.request_edge(GridScrollIntent::Bottom);
        assert!(!state.take_reveal());
        assert_eq!(state.take_request().edge(), Some(GridScrollIntent::Bottom));
        assert_eq!(state.row_anchor(), Some(3));
        state.request_edge(GridScrollIntent::Top);
        state.set_reveal_requested(true);
        assert!(state.take_reveal());
        // Only the resolved-selection reveal applies this placement, before the edge wins.
        assert_eq!(state.take_row_anchor(), Some(3));
        assert_eq!(state.take_request().edge(), Some(GridScrollIntent::Top));
        assert_eq!(state.row_anchor(), None);
    }

    #[test]
    fn grid_scroll_readback_and_late_request_have_separate_lifetimes() {
        let mut state = GridScrollState::default();
        state.set_position(100.0);
        assert!(matches!(
            state.take_request(),
            GridScrollRequest::Position { .. }
        ));
        state.request_edge(GridScrollIntent::Bottom);
        state.adopt_offset(120.0);
        assert_eq!(state.take_request().edge(), Some(GridScrollIntent::Bottom));
        state.adopt_offset(140.0);
        assert_eq!(state.take_request(), GridScrollRequest::Keep);
        // Restoring the same value is nevertheless an explicit position request.
        state.set_position(140.0);
        assert!(matches!(
            state.take_request(),
            GridScrollRequest::Position { .. }
        ));
    }

    #[test]
    fn grid_scroll_layout_claim_preserves_reveal_issued_after_early_claim() {
        let mut state = GridScrollState::default();
        assert!(!state.take_reveal());
        state.set_reveal_requested(true);
        state.request_edge(GridScrollIntent::Bottom);
        assert_eq!(
            state.take_layout_request().edge(),
            Some(GridScrollIntent::Bottom)
        );
        assert!(state.reveal_requested());
        assert_eq!(state.pending_edge(), None);
        assert!(state.take_reveal());
        assert_eq!(state.take_layout_request(), GridScrollRequest::Keep);
    }

    #[test]
    fn grid_scroll_list_reset_and_move_preserve_anchor_but_fork_does_not_inherit_it() {
        let mut state = GridScrollState::default();
        state.set_row_anchor(Some(2));
        state.request_edge(GridScrollIntent::Bottom);
        state.set_position(0.0);
        state.set_reveal_requested(false);
        state.clear_edge();
        assert_eq!(state.row_anchor(), Some(2));
        let fork = state.fork();
        assert_eq!(fork.offset(), state.offset());
        assert_eq!(fork.request, state.request);
        assert_eq!(fork.row_anchor(), None);
        let mut destination = GridScrollState::default();
        std::mem::swap(&mut destination, &mut state);
        assert_eq!(destination.row_anchor(), Some(2));
        assert_eq!(state.row_anchor(), None);
    }
}
