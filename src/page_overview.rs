//! Pure viewer-owned page overview model. No images, I/O, input ownership or navigation effects.
//!
//! Metadata is prepared once with the items and shared independently of canonical reader order.
//! A projection validates/indexes that order once; opening, parking and drawing never clone or
//! scan all page identities. White companions annotate real pages and never have their own target.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, Vec2};

use crate::book_resume_db::PageIdentity;

/// Auxiliary display information supplied by the canonical composition/display owner.
/// This is an annotation, not a composition format, source address or selectable blank page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteCompanion {
    Composition,
    DisplayPadding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOverviewEntry {
    pub items_idx: usize,
    pub page: Arc<PageOverviewPage>,
}

/// Identity/name payload shared even when deleting another item changes the items index space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOverviewPage {
    pub identity: PageIdentity,
    pub label: Arc<str>,
    pub white_companion: Option<WhiteCompanion>,
}

/// Real-page metadata keyed by the actual items index, independent of reading order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PageOverviewMetadata {
    entries: HashMap<usize, PageOverviewEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOverviewProjectionError {
    DuplicateItem(usize),
    MissingItem(usize),
}

impl PageOverviewMetadata {
    /// Call at the existing items preparation boundary, not each time the overview is opened.
    pub fn new(
        entries: impl IntoIterator<Item = PageOverviewEntry>,
    ) -> Result<Self, PageOverviewProjectionError> {
        let mut indexed = HashMap::new();
        for entry in entries {
            let idx = entry.items_idx;
            if indexed.insert(idx, entry).is_some() {
                return Err(PageOverviewProjectionError::DuplicateItem(idx));
            }
        }
        Ok(Self { entries: indexed })
    }

    pub fn entry(&self, items_idx: usize) -> Option<&PageOverviewEntry> {
        self.entries.get(&items_idx)
    }

    pub fn entries(&self) -> impl Iterator<Item = &PageOverviewEntry> {
        self.entries.values()
    }

    /// Reindex only integer keys and Arc payloads using the existing owner's old->current map.
    /// Omitted items are removed; no source identity/path/label is copied or resolved here.
    pub fn remap_items(
        &self,
        mapping: impl IntoIterator<Item = (usize, usize)>,
    ) -> Result<Self, PageOverviewProjectionError> {
        let mut entries = HashMap::new();
        for (old_idx, items_idx) in mapping {
            let original = self
                .entry(old_idx)
                .ok_or(PageOverviewProjectionError::MissingItem(old_idx))?;
            let entry = PageOverviewEntry {
                items_idx,
                page: Arc::clone(&original.page),
            };
            if entries.insert(items_idx, entry).is_some() {
                return Err(PageOverviewProjectionError::DuplicateItem(items_idx));
            }
        }
        Ok(Self { entries })
    }
}

/// The supplied canonical visible real-page order, with no overview-specific filtering/sorting.
#[derive(Debug)]
pub struct PageOverviewProjection {
    metadata: Arc<PageOverviewMetadata>,
    reader_order: Arc<Vec<usize>>,
    ordinals: HashMap<usize, usize>,
}

impl PageOverviewProjection {
    /// Build/cache with the navigation projection. Only integer indices are copied here;
    /// metadata and the existing reader-order vector retain their shared allocations.
    pub fn new(
        metadata: Arc<PageOverviewMetadata>,
        reader_order: Arc<Vec<usize>>,
    ) -> Result<Self, PageOverviewProjectionError> {
        let mut ordinals = HashMap::with_capacity(reader_order.len());
        for (ordinal, &idx) in reader_order.iter().enumerate() {
            if metadata.entry(idx).is_none() {
                return Err(PageOverviewProjectionError::MissingItem(idx));
            }
            if ordinals.insert(idx, ordinal).is_some() {
                return Err(PageOverviewProjectionError::DuplicateItem(idx));
            }
        }
        Ok(Self {
            metadata,
            reader_order,
            ordinals,
        })
    }

    pub fn len(&self) -> usize {
        self.reader_order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reader_order.is_empty()
    }

    pub fn entry(&self, ordinal: usize) -> Option<&PageOverviewEntry> {
        self.reader_order
            .get(ordinal)
            .and_then(|&idx| self.metadata.entry(idx))
    }

    pub fn ordinal_for_item(&self, items_idx: usize) -> Option<usize> {
        self.ordinals.get(&items_idx).copied()
    }

    pub fn reader_order(&self) -> &Arc<Vec<usize>> {
        &self.reader_order
    }

    pub fn metadata(&self) -> &Arc<PageOverviewMetadata> {
        &self.metadata
    }
}

/// Context serial and incarnation are opaque caller-supplied identities, never HWND/rect guesses.
/// The caller must also validate the existing surface/viewport binding before model dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageOverviewStamp {
    pub context_serial: u64,
    pub items_generation: u64,
    pub reader_order_revision: u64,
    pub incarnation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageOverviewDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

/// Canonical scrolling is row-snapped. Pointer drag fractions belong to the renderer, not here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageOverviewScrollAnchor {
    pub row: usize,
}

/// All lengths are egui points in the drawing surface's coordinate system.
#[derive(Clone, Copy, Debug)]
pub struct PageOverviewLayout {
    viewport: Rect,
    page_count: usize,
    columns: usize,
    direction: PageOverviewDirection,
    cell_size: Vec2,
}

impl PageOverviewLayout {
    pub const MIN_CELL_WIDTH: f32 = 96.0;
    pub const MIN_CELL_HEIGHT: f32 = 120.0;
    pub const GAP: f32 = 8.0;

    pub fn new(
        page_count: usize,
        preferred_columns: u8,
        viewport: Rect,
        direction: PageOverviewDirection,
    ) -> Self {
        // Empty/invalid surfaces expose no cells, rather than creating non-finite rectangles.
        let viewport = if viewport.is_finite()
            && viewport.width().is_finite()
            && viewport.height().is_finite()
            && viewport.width() >= 0.0
            && viewport.height() >= 0.0
        {
            viewport
        } else {
            Rect::from_min_size(Pos2::ZERO, Vec2::ZERO)
        };
        let fits =
            ((viewport.width() + Self::GAP) / (Self::MIN_CELL_WIDTH + Self::GAP)).floor() as usize;
        let columns = usize::from(preferred_columns.clamp(1, 10)).min(fits.max(1));
        let width = ((viewport.width() - Self::GAP * (columns - 1) as f32) / columns as f32)
            .max(Self::MIN_CELL_WIDTH);
        Self {
            viewport,
            page_count,
            columns,
            direction,
            cell_size: egui::vec2(width, (width * 1.25).max(Self::MIN_CELL_HEIGHT)),
        }
    }

    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn cell_size(&self) -> Vec2 {
        self.cell_size
    }

    pub fn row_stride(&self) -> f32 {
        self.cell_size.y + Self::GAP
    }

    pub fn total_rows(&self) -> usize {
        self.page_count.div_ceil(self.columns)
    }

    pub fn content_height(&self) -> f32 {
        if self.page_count == 0 {
            0.0
        } else {
            self.total_rows() as f32 * self.row_stride() - Self::GAP
        }
    }

    /// At least one logical row for navigation, even when a tiny surface clips the cell.
    pub fn page_rows(&self) -> usize {
        (((self.viewport.height() + Self::GAP) / self.row_stride()).floor() as usize).max(1)
    }

    pub fn max_scroll_row(&self) -> usize {
        self.total_rows().saturating_sub(self.page_rows())
    }

    pub fn clamp_anchor(&self, anchor: PageOverviewScrollAnchor) -> PageOverviewScrollAnchor {
        PageOverviewScrollAnchor {
            row: anchor.row.min(self.max_scroll_row()),
        }
    }

    pub fn visible_rows(&self, anchor: PageOverviewScrollAnchor) -> Range<usize> {
        let top = self.clamp_anchor(anchor).row;
        if self.viewport.width() <= 0.0 || self.viewport.height() <= 0.0 || self.page_count == 0 {
            return top..top;
        }
        let count = (self.viewport.height() / self.row_stride()).ceil() as usize;
        top..top.saturating_add(count).min(self.total_rows())
    }

    /// Exactly the visible rows plus at most one row on each side, never a prefix to the cursor.
    pub fn overscan_rows(&self, anchor: PageOverviewScrollAnchor) -> Range<usize> {
        let visible = self.visible_rows(anchor);
        if visible.is_empty() {
            return visible;
        }
        visible.start.saturating_sub(1)..visible.end.saturating_add(1).min(self.total_rows())
    }

    pub fn visible_ordinals(&self, anchor: PageOverviewScrollAnchor) -> Range<usize> {
        self.ordinals_for_rows(self.visible_rows(anchor))
    }

    pub fn overscan_ordinals(&self, anchor: PageOverviewScrollAnchor) -> Range<usize> {
        self.ordinals_for_rows(self.overscan_rows(anchor))
    }

    fn ordinals_for_rows(&self, rows: Range<usize>) -> Range<usize> {
        rows.start.saturating_mul(self.columns).min(self.page_count)
            ..rows.end.saturating_mul(self.columns).min(self.page_count)
    }

    fn visual_column(&self, ordinal: usize) -> usize {
        let column = ordinal % self.columns;
        match self.direction {
            PageOverviewDirection::LeftToRight => column,
            PageOverviewDirection::RightToLeft => self.columns - 1 - column,
        }
    }

    /// RTL partial rows remain aligned to the right. Page numbers remain ordinal + 1.
    pub fn cell_rect(&self, ordinal: usize, anchor: PageOverviewScrollAnchor) -> Option<Rect> {
        if ordinal >= self.page_count {
            return None;
        }
        let top = self.clamp_anchor(anchor).row;
        let row_delta = (ordinal / self.columns) as f64 - top as f64;
        Some(Rect::from_min_size(
            self.viewport.min
                + egui::vec2(
                    self.visual_column(ordinal) as f32 * (self.cell_size.x + Self::GAP),
                    row_delta as f32 * self.row_stride(),
                ),
            self.cell_size,
        ))
    }

    /// Hit the layout drawn in this frame, then capture identity from the shared projection.
    /// Cell auxiliary areas have the same hit/target; gaps and empty last-row slots have no hit.
    pub fn hit_test(&self, point: Pos2, anchor: PageOverviewScrollAnchor) -> Option<usize> {
        if !self.viewport.contains(point) || !point.is_finite() {
            return None;
        }
        let offset = point - self.viewport.min;
        let visual_column = (offset.x / (self.cell_size.x + Self::GAP)).floor() as usize;
        let local_row = (offset.y / self.row_stride()).floor() as usize;
        if visual_column >= self.columns {
            return None;
        }
        let column = match self.direction {
            PageOverviewDirection::LeftToRight => visual_column,
            PageOverviewDirection::RightToLeft => self.columns - 1 - visual_column,
        };
        let row = self.clamp_anchor(anchor).row.saturating_add(local_row);
        let ordinal = row.saturating_mul(self.columns).saturating_add(column);
        self.cell_rect(ordinal, anchor)
            .filter(|rect| rect.contains(point))
            .map(|_| ordinal)
    }

    fn cursor_at_row(&self, cursor: usize, row: usize) -> usize {
        let start = row.saturating_mul(self.columns);
        start + (cursor % self.columns).min(self.page_count - start - 1)
    }

    fn moved_cursor(&self, cursor: usize, movement: PageOverviewCursorMove) -> usize {
        let row = cursor / self.columns;
        let column = cursor % self.columns;
        let row_end = ((row + 1) * self.columns).min(self.page_count) - 1;
        let advance = || (cursor + 1).min(row_end);
        let retreat = || cursor.saturating_sub(usize::from(column > 0));
        match movement {
            PageOverviewCursorMove::Left => match self.direction {
                PageOverviewDirection::LeftToRight => retreat(),
                PageOverviewDirection::RightToLeft => advance(),
            },
            PageOverviewCursorMove::Right => match self.direction {
                PageOverviewDirection::LeftToRight => advance(),
                PageOverviewDirection::RightToLeft => retreat(),
            },
            PageOverviewCursorMove::Up => self.cursor_at_row(cursor, row.saturating_sub(1)),
            PageOverviewCursorMove::Down => {
                self.cursor_at_row(cursor, (row + 1).min(self.total_rows() - 1))
            }
            PageOverviewCursorMove::PageUp => {
                self.cursor_at_row(cursor, row.saturating_sub(self.page_rows()))
            }
            PageOverviewCursorMove::PageDown => self.cursor_at_row(
                cursor,
                row.saturating_add(self.page_rows())
                    .min(self.total_rows() - 1),
            ),
            PageOverviewCursorMove::First => 0,
            PageOverviewCursorMove::Last => self.page_count - 1,
        }
    }

    pub fn cursor_after_move(
        &self,
        cursor: usize,
        movement: PageOverviewCursorMove,
    ) -> Option<usize> {
        (cursor < self.page_count).then(|| self.moved_cursor(cursor, movement))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOverviewCursorMove {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOverviewTarget {
    pub ordinal: usize,
    pub items_idx: usize,
    pub identity: PageIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOverviewSelect {
    pub stamp: PageOverviewStamp,
    pub target: PageOverviewTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOverviewOpenError {
    EmptyProjection,
    CurrentPageMissing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOverviewSelectRejection {
    Closed,
    StaleStamp,
    InactiveOwner,
    StaleTarget,
    NavigationRejected,
}

/// The single context-owned state. Intentionally not Clone: a fork starts Closed.
#[derive(Debug, Default)]
pub enum PageOverviewState {
    #[default]
    Closed,
    Open(PageOverviewSession),
}

#[derive(Debug)]
pub struct PageOverviewSession {
    view: PageOverviewView,
}

/// O(1) read-only projection for passive drawing. No pointer gesture ownership is retained;
/// the renderer must end its gesture when parking the existing owner.
#[derive(Clone, Debug)]
pub struct PageOverviewView {
    pub projection: Arc<PageOverviewProjection>,
    pub stamp: PageOverviewStamp,
    pub cursor: usize,
    pub scroll_anchor: PageOverviewScrollAnchor,
    pub preferred_columns: u8,
}

impl PageOverviewView {
    pub fn layout(&self, viewport: Rect, direction: PageOverviewDirection) -> PageOverviewLayout {
        PageOverviewLayout::new(
            self.projection.len(),
            self.preferred_columns,
            viewport,
            direction,
        )
    }

    pub fn capture_select(&self, ordinal: usize) -> Option<PageOverviewSelect> {
        let entry = self.projection.entry(ordinal)?;
        Some(PageOverviewSelect {
            stamp: self.stamp,
            target: PageOverviewTarget {
                ordinal,
                items_idx: entry.items_idx,
                identity: entry.page.identity.clone(),
            },
        })
    }

    /// Derive a geometric projection without moving the canonical session or changing its stamp.
    /// Active reflow and read-only passive drawing preserve the same cursor-relative row.
    pub fn reflowed(&self, previous: &PageOverviewLayout, next: &PageOverviewLayout) -> Self {
        assert_eq!(previous.page_count, self.projection.len());
        assert_eq!(next.page_count, self.projection.len());
        let relative_row = (self.cursor / previous.columns)
            .saturating_sub(previous.clamp_anchor(self.scroll_anchor).row)
            .min(next.page_rows() - 1);
        let mut view = self.clone();
        view.scroll_anchor = next.clamp_anchor(PageOverviewScrollAnchor {
            row: (self.cursor / next.columns).saturating_sub(relative_row),
        });
        view
    }
}

impl PageOverviewState {
    /// The caller supplies an already-prepared projection and a fresh incarnation.
    pub fn open(
        projection: Arc<PageOverviewProjection>,
        stamp: PageOverviewStamp,
        current_items_idx: usize,
        preferred_columns: u8,
        viewport: Rect,
        direction: PageOverviewDirection,
    ) -> Result<Self, PageOverviewOpenError> {
        if projection.is_empty() {
            return Err(PageOverviewOpenError::EmptyProjection);
        }
        let cursor = projection
            .ordinal_for_item(current_items_idx)
            .ok_or(PageOverviewOpenError::CurrentPageMissing)?;
        let mut view = PageOverviewView {
            projection,
            stamp,
            cursor,
            scroll_anchor: PageOverviewScrollAnchor::default(),
            preferred_columns: preferred_columns.clamp(1, 10),
        };
        let layout = view.layout(viewport, direction);
        view.scroll_anchor = layout.clamp_anchor(PageOverviewScrollAnchor {
            row: cursor / layout.columns,
        });
        Ok(Self::Open(PageOverviewSession { view }))
    }

    pub fn session(&self) -> Option<&PageOverviewSession> {
        match self {
            Self::Closed => None,
            Self::Open(session) => Some(session),
        }
    }

    pub fn session_mut(&mut self) -> Option<&mut PageOverviewSession> {
        match self {
            Self::Closed => None,
            Self::Open(session) => Some(session),
        }
    }

    pub fn close(&mut self) {
        *self = Self::Closed;
    }

    /// Forking never duplicates an overview session or any request/gesture owner.
    pub fn fork_closed(&self) -> Self {
        Self::Closed
    }

    /// Active eligibility is supplied from the actual current owner, not a scoped mount or focus.
    /// This performs no state changes, activation, navigation or thumbnail effects.
    pub fn validate_select(
        &self,
        selection: &PageOverviewSelect,
        current_stamp: PageOverviewStamp,
        active_context_serial: Option<u64>,
    ) -> Result<PageOverviewTarget, PageOverviewSelectRejection> {
        let session = self.session().ok_or(PageOverviewSelectRejection::Closed)?;
        if selection.stamp != session.view.stamp || current_stamp != session.view.stamp {
            return Err(PageOverviewSelectRejection::StaleStamp);
        }
        if active_context_serial != Some(session.view.stamp.context_serial) {
            return Err(PageOverviewSelectRejection::InactiveOwner);
        }
        let entry = session
            .view
            .projection
            .entry(selection.target.ordinal)
            .ok_or(PageOverviewSelectRejection::StaleTarget)?;
        if selection.target.items_idx != entry.items_idx
            || selection.target.identity != entry.page.identity
        {
            return Err(PageOverviewSelectRejection::StaleTarget);
        }
        Ok(selection.target.clone())
    }

    /// Invoke the existing landing gate exactly once for a validated active target. Close only
    /// after acceptance; inactive/stale/rejected commands leave the entire session unchanged.
    pub fn select(
        &mut self,
        selection: &PageOverviewSelect,
        current_stamp: PageOverviewStamp,
        active_context_serial: Option<u64>,
        accept_navigation: impl FnOnce(&PageOverviewTarget) -> bool,
    ) -> Result<PageOverviewTarget, PageOverviewSelectRejection> {
        let target = self.validate_select(selection, current_stamp, active_context_serial)?;
        if !accept_navigation(&target) {
            return Err(PageOverviewSelectRejection::NavigationRejected);
        }
        self.close();
        Ok(target)
    }
}

impl PageOverviewSession {
    /// Parking retains the same state; copying this view only increments the metadata Arc.
    pub fn view(&self) -> &PageOverviewView {
        &self.view
    }

    fn check_layout(&self, layout: &PageOverviewLayout) {
        assert_eq!(layout.page_count, self.view.projection.len());
    }

    pub fn move_cursor(&mut self, movement: PageOverviewCursorMove, layout: &PageOverviewLayout) {
        self.check_layout(layout);
        self.view.cursor = layout.moved_cursor(self.view.cursor, movement);
        self.reveal_cursor(layout);
    }

    pub fn set_cursor_target(
        &mut self,
        target: &PageOverviewTarget,
        layout: &PageOverviewLayout,
    ) -> bool {
        self.check_layout(layout);
        let Some(entry) = self.view.projection.entry(target.ordinal) else {
            return false;
        };
        if entry.items_idx != target.items_idx || entry.page.identity != target.identity {
            return false;
        }
        self.view.cursor = target.ordinal;
        self.reveal_cursor(layout);
        true
    }

    fn reveal_cursor(&mut self, layout: &PageOverviewLayout) {
        let row = self.view.cursor / layout.columns;
        let top = self.view.scroll_anchor.row;
        let desired = if row < top {
            row
        } else if row >= top.saturating_add(layout.page_rows()) {
            row.saturating_sub(layout.page_rows() - 1)
        } else {
            top
        };
        self.view.scroll_anchor = layout.clamp_anchor(PageOverviewScrollAnchor { row: desired });
    }

    /// Scrolling does not change the overview cursor or the viewer's reading position.
    pub fn scroll_rows(&mut self, delta: isize, layout: &PageOverviewLayout) {
        self.check_layout(layout);
        self.view.scroll_anchor = layout.clamp_anchor(PageOverviewScrollAnchor {
            row: layout
                .clamp_anchor(self.view.scroll_anchor)
                .row
                .saturating_add_signed(delta),
        });
    }

    /// Preserve cursor identity and its relative visible row across resize/DPI/column changes.
    /// If scrolling had hidden the cursor, reflow brings that same cursor back into view.
    pub fn reflow(&mut self, previous: &PageOverviewLayout, next: &PageOverviewLayout) {
        self.view = self.view.reflowed(previous, next);
    }

    /// Preferred columns remain a session value even if a narrow surface reduces effective columns.
    pub fn set_preferred_columns(
        &mut self,
        preferred_columns: u8,
        viewport: Rect,
        direction: PageOverviewDirection,
    ) -> PageOverviewLayout {
        let previous = self.view.layout(viewport, direction);
        self.view.preferred_columns = preferred_columns.clamp(1, 10);
        let next = self.view.layout(viewport, direction);
        self.reflow(&previous, &next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn entry(idx: usize) -> PageOverviewEntry {
        PageOverviewEntry {
            items_idx: idx,
            page: Arc::new(PageOverviewPage {
                identity: PageIdentity::Image {
                    path: format!("g:/book/{idx}.png"),
                },
                label: Arc::from(format!("page {idx}")),
                white_companion: None,
            }),
        }
    }

    fn projection(count: usize) -> Arc<PageOverviewProjection> {
        Arc::new(
            PageOverviewProjection::new(
                Arc::new(PageOverviewMetadata::new((0..count).map(entry)).unwrap()),
                Arc::new((0..count).collect()),
            )
            .unwrap(),
        )
    }

    fn stamp() -> PageOverviewStamp {
        PageOverviewStamp {
            context_serial: 11,
            items_generation: 22,
            reader_order_revision: 33,
            incarnation: 44,
        }
    }

    fn viewport() -> Rect {
        Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(408.0, 376.0))
    }

    fn opened(count: usize, idx: usize, direction: PageOverviewDirection) -> PageOverviewState {
        PageOverviewState::open(projection(count), stamp(), idx, 4, viewport(), direction).unwrap()
    }

    fn layout(state: &PageOverviewState, direction: PageOverviewDirection) -> PageOverviewLayout {
        state
            .session()
            .unwrap()
            .view()
            .layout(viewport(), direction)
    }

    #[test]
    fn canonical_projection_shares_metadata_and_order_without_sorting_or_filtering() {
        let metadata = Arc::new(PageOverviewMetadata::new([entry(7), entry(1), entry(9)]).unwrap());
        let order = Arc::new(vec![9, 7]);
        let projection =
            Arc::new(PageOverviewProjection::new(metadata.clone(), order.clone()).unwrap());
        assert!(Arc::ptr_eq(projection.metadata(), &metadata));
        assert!(Arc::ptr_eq(projection.reader_order(), &order));
        assert!(std::ptr::eq(
            projection.entry(0).unwrap(),
            metadata.entry(9).unwrap()
        ));
        let state = PageOverviewState::open(
            projection.clone(),
            stamp(),
            7,
            4,
            viewport(),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            &state.session().unwrap().view().projection,
            &projection
        ));
        assert_eq!(state.session().unwrap().view().cursor, 1);
        assert_eq!(projection.ordinal_for_item(1), None);
        assert_eq!(projection.entry(0).unwrap().items_idx, 9);
    }

    #[test]
    fn invalid_projections_are_rejected_at_preparation_boundary() {
        assert!(matches!(
            PageOverviewMetadata::new([entry(1), entry(1)]),
            Err(PageOverviewProjectionError::DuplicateItem(1))
        ));
        let metadata = Arc::new(PageOverviewMetadata::new([entry(1)]).unwrap());
        assert!(matches!(
            PageOverviewProjection::new(metadata.clone(), Arc::new(vec![2])),
            Err(PageOverviewProjectionError::MissingItem(2))
        ));
        assert!(matches!(
            PageOverviewProjection::new(metadata, Arc::new(vec![1, 1])),
            Err(PageOverviewProjectionError::DuplicateItem(1))
        ));
    }

    #[test]
    fn metadata_remap_shares_identity_payload_and_lands_at_current_item_index() {
        let original = Arc::new(PageOverviewMetadata::new([entry(1), entry(8)]).unwrap());
        let remapped = Arc::new(original.remap_items([(8, 6)]).unwrap());
        assert!(Arc::ptr_eq(
            &original.entry(8).unwrap().page,
            &remapped.entry(6).unwrap().page
        ));
        assert_eq!(remapped.entry(6).unwrap().items_idx, 6);
        assert!(remapped.entry(1).is_none());
        let projection =
            Arc::new(PageOverviewProjection::new(remapped, Arc::new(vec![6])).unwrap());
        let state = PageOverviewState::open(
            projection,
            stamp(),
            6,
            4,
            viewport(),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap();
        let command = state.session().unwrap().view().capture_select(0).unwrap();
        assert_eq!(command.target.items_idx, 6);
        assert_eq!(
            command.target.identity,
            original.entry(8).unwrap().page.identity
        );
        assert!(state.validate_select(&command, stamp(), Some(11)).is_ok());
        assert!(matches!(
            original.remap_items([(9, 0)]),
            Err(PageOverviewProjectionError::MissingItem(9))
        ));
        assert!(matches!(
            original.remap_items([(1, 0), (8, 0)]),
            Err(PageOverviewProjectionError::DuplicateItem(0))
        ));
    }

    #[test]
    fn empty_and_missing_current_page_do_not_choose_an_unrelated_page() {
        assert!(matches!(
            PageOverviewState::open(
                projection(0),
                stamp(),
                0,
                4,
                viewport(),
                PageOverviewDirection::LeftToRight
            ),
            Err(PageOverviewOpenError::EmptyProjection)
        ));
        assert!(matches!(
            PageOverviewState::open(
                projection(1),
                stamp(),
                2,
                4,
                viewport(),
                PageOverviewDirection::LeftToRight
            ),
            Err(PageOverviewOpenError::CurrentPageMissing)
        ));
        let state = opened(1, 0, PageOverviewDirection::LeftToRight);
        let grid = layout(&state, PageOverviewDirection::LeftToRight);
        assert_eq!(
            grid.visible_ordinals(state.session().unwrap().view().scroll_anchor),
            0..1
        );
    }

    #[test]
    fn large_book_end_has_only_visible_cells_and_one_row_overscan() {
        for count in [1, 317, 50_000] {
            let mut state = opened(count, count - 1, PageOverviewDirection::LeftToRight);
            let grid = layout(&state, PageOverviewDirection::LeftToRight);
            let view = state.session().unwrap().view();
            let visible = grid.visible_ordinals(view.scroll_anchor);
            let overscan = grid.overscan_ordinals(view.scroll_anchor);
            assert!(visible.contains(&(count - 1)));
            assert!(visible.len() <= 3 * grid.columns());
            assert!(overscan.len() <= 5 * grid.columns());
            assert_eq!(overscan.end, count);
            let last = grid.cell_rect(count - 1, view.scroll_anchor).unwrap();
            assert_eq!(
                grid.hit_test(last.center(), view.scroll_anchor),
                Some(count - 1)
            );
            state.session_mut().unwrap().scroll_rows(isize::MIN, &grid);
            assert_eq!(state.session().unwrap().view().scroll_anchor.row, 0);
            assert_eq!(state.session().unwrap().view().cursor, count - 1);
        }
    }

    #[test]
    fn minimum_dimensions_and_preferred_column_bounds_survive_small_surfaces() {
        let tiny = Rect::from_min_size(Pos2::ZERO, egui::vec2(1.0, 1.0));
        let grid = PageOverviewLayout::new(50_000, 255, tiny, PageOverviewDirection::RightToLeft);
        assert_eq!(grid.columns(), 1);
        assert!(grid.cell_size().x >= PageOverviewLayout::MIN_CELL_WIDTH);
        assert!(grid.cell_size().y >= PageOverviewLayout::MIN_CELL_HEIGHT);
        assert_eq!(
            grid.visible_ordinals(PageOverviewScrollAnchor::default()),
            0..1
        );
        let wide = Rect::from_min_size(Pos2::ZERO, egui::vec2(2000.0, 500.0));
        assert_eq!(
            PageOverviewLayout::new(100, 255, wide, PageOverviewDirection::LeftToRight).columns(),
            10
        );
        assert_eq!(
            PageOverviewLayout::new(100, 0, wide, PageOverviewDirection::LeftToRight).columns(),
            1
        );
        let zero =
            PageOverviewLayout::new(100, 4, Rect::NOTHING, PageOverviewDirection::LeftToRight);
        assert!(
            zero.overscan_ordinals(PageOverviewScrollAnchor::default())
                .is_empty()
        );
    }

    #[test]
    fn horizontal_arrows_follow_visual_geometry_without_wrapping() {
        for direction in [
            PageOverviewDirection::LeftToRight,
            PageOverviewDirection::RightToLeft,
        ] {
            let mut state = opened(10, 1, direction);
            let grid = layout(&state, direction);
            let old_x = grid
                .cell_rect(1, PageOverviewScrollAnchor::default())
                .unwrap()
                .center()
                .x;
            state
                .session_mut()
                .unwrap()
                .move_cursor(PageOverviewCursorMove::Left, &grid);
            let left = state.session().unwrap().view().cursor;
            assert!(
                grid.cell_rect(left, PageOverviewScrollAnchor::default())
                    .unwrap()
                    .center()
                    .x
                    < old_x
            );
            state
                .session_mut()
                .unwrap()
                .move_cursor(PageOverviewCursorMove::Right, &grid);
            assert_eq!(state.session().unwrap().view().cursor, 1);
            let edge = if direction == PageOverviewDirection::LeftToRight {
                0
            } else {
                3
            };
            let mut at_edge = opened(10, edge, direction);
            at_edge
                .session_mut()
                .unwrap()
                .move_cursor(PageOverviewCursorMove::Left, &grid);
            assert_eq!(at_edge.session().unwrap().view().cursor, edge);
        }
    }

    #[test]
    fn vertical_page_home_end_navigation_clamps_partial_rows_and_scrolls() {
        for direction in [
            PageOverviewDirection::LeftToRight,
            PageOverviewDirection::RightToLeft,
        ] {
            let mut state = opened(18, 7, direction);
            let grid = layout(&state, direction);
            let session = state.session_mut().unwrap();
            session.move_cursor(PageOverviewCursorMove::Down, &grid);
            assert_eq!(session.view().cursor, 11);
            session.move_cursor(PageOverviewCursorMove::PageDown, &grid);
            assert_eq!(session.view().cursor, 17);
            assert!(
                grid.visible_ordinals(session.view().scroll_anchor)
                    .contains(&17)
            );
            session.move_cursor(PageOverviewCursorMove::PageUp, &grid);
            assert_eq!(session.view().cursor, 5);
            session.move_cursor(PageOverviewCursorMove::Up, &grid);
            assert_eq!(session.view().cursor, 1);
            session.move_cursor(PageOverviewCursorMove::First, &grid);
            assert_eq!(session.view().cursor, 0);
            assert_eq!(session.view().scroll_anchor.row, 0);
            session.move_cursor(PageOverviewCursorMove::Last, &grid);
            assert_eq!(session.view().cursor, 17);
        }
    }

    #[test]
    fn rtl_partial_row_hit_uses_drawn_slot_and_rejects_empty_slots_and_gaps() {
        let state = opened(6, 0, PageOverviewDirection::RightToLeft);
        let grid = layout(&state, PageOverviewDirection::RightToLeft);
        let anchor = PageOverviewScrollAnchor::default();
        assert!(
            grid.cell_rect(0, anchor).unwrap().left() > grid.cell_rect(1, anchor).unwrap().left()
        );
        assert_eq!(
            grid.hit_test(grid.cell_rect(4, anchor).unwrap().center(), anchor),
            Some(4)
        );
        assert_eq!(
            grid.hit_test(grid.cell_rect(5, anchor).unwrap().center(), anchor),
            Some(5)
        );
        assert_eq!(grid.hit_test(egui::pos2(20.0, 160.0), anchor), None);
        let left = grid.cell_rect(3, anchor).unwrap();
        assert_eq!(
            grid.hit_test(egui::pos2(left.right() + 4.0, left.center().y), anchor),
            None
        );
        assert_eq!(
            grid.hit_test(egui::pos2(left.center().x, left.bottom() + 4.0), anchor),
            None
        );
    }

    #[test]
    fn resize_and_columns_keep_cursor_identity_visible_and_preferences_session_local() {
        let mut state = opened(200, 43, PageOverviewDirection::LeftToRight);
        let old = layout(&state, PageOverviewDirection::LeftToRight);
        let target = state.session().unwrap().view().capture_select(43).unwrap();
        let session = state.session_mut().unwrap();
        session.scroll_rows(-1, &old);
        let narrow = Rect::from_min_size(Pos2::ZERO, egui::vec2(210.0, 250.0));
        let next = session
            .view()
            .layout(narrow, PageOverviewDirection::LeftToRight);
        session.reflow(&old, &next);
        assert_eq!(next.columns(), 2);
        assert_eq!(session.view().cursor, 43);
        assert!(
            next.visible_ordinals(session.view().scroll_anchor)
                .contains(&43)
        );
        assert_eq!(session.view().preferred_columns, 4);
        let changed = session.set_preferred_columns(10, narrow, PageOverviewDirection::LeftToRight);
        assert_eq!(changed.columns(), 2);
        assert_eq!(session.view().preferred_columns, 10);
        assert_eq!(
            session
                .view()
                .capture_select(session.view().cursor)
                .unwrap(),
            target
        );
        let expanded = session
            .view()
            .layout(viewport(), PageOverviewDirection::LeftToRight);
        session.reflow(&changed, &expanded);
        assert_eq!(expanded.columns(), 4);
        assert!(
            expanded
                .visible_ordinals(session.view().scroll_anchor)
                .contains(&43)
        );
    }

    #[test]
    fn parking_shares_all_metadata_and_newly_exposed_cells_have_real_identity() {
        let state = opened(40, 0, PageOverviewDirection::RightToLeft);
        let parked = state.session().unwrap().view().clone();
        assert!(Arc::ptr_eq(
            &parked.projection,
            &state.session().unwrap().view().projection
        ));
        let larger = parked.layout(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(408.0, 760.0)),
            PageOverviewDirection::RightToLeft,
        );
        let cell = larger.cell_rect(20, parked.scroll_anchor).unwrap();
        assert_eq!(
            larger.hit_test(cell.center(), parked.scroll_anchor),
            Some(20)
        );
        assert_eq!(parked.capture_select(20).unwrap().target.items_idx, 20);
        assert!(matches!(state.fork_closed(), PageOverviewState::Closed));
        assert!(matches!(state, PageOverviewState::Open(_)));
    }

    #[test]
    fn inactive_select_does_not_call_landing_close_move_or_queue_replay() {
        let mut state = opened(50, 12, PageOverviewDirection::LeftToRight);
        let before = state.session().unwrap().view().clone();
        let command = before.capture_select(37).unwrap();
        let called = Cell::new(0);
        assert_eq!(
            state.select(&command, stamp(), Some(99), |_| {
                called.set(called.get() + 1);
                true
            }),
            Err(PageOverviewSelectRejection::InactiveOwner)
        );
        assert_eq!(called.get(), 0);
        let after = state.session().unwrap().view();
        assert_eq!(after.cursor, before.cursor);
        assert_eq!(after.scroll_anchor, before.scroll_anchor);
        assert_eq!(after.stamp, before.stamp);
        assert!(Arc::ptr_eq(&after.projection, &before.projection));
        // Returning to this owner has no stored Select to replay: a new explicit command is needed.
        assert!(matches!(state, PageOverviewState::Open(_)));
        assert_eq!(
            state.validate_select(&command, stamp(), None),
            Err(PageOverviewSelectRejection::InactiveOwner)
        );
    }

    #[test]
    fn each_stamp_component_and_current_revision_are_checked() {
        let mut state = opened(4, 0, PageOverviewDirection::LeftToRight);
        let command = state.session().unwrap().view().capture_select(2).unwrap();
        for stale in [
            PageOverviewStamp {
                context_serial: 99,
                ..stamp()
            },
            PageOverviewStamp {
                items_generation: 99,
                ..stamp()
            },
            PageOverviewStamp {
                reader_order_revision: 99,
                ..stamp()
            },
            PageOverviewStamp {
                incarnation: 99,
                ..stamp()
            },
        ] {
            let old_command = PageOverviewSelect {
                stamp: stale,
                ..command.clone()
            };
            assert_eq!(
                state.validate_select(&old_command, stamp(), Some(11)),
                Err(PageOverviewSelectRejection::StaleStamp)
            );
            assert_eq!(
                state.validate_select(&command, stale, Some(11)),
                Err(PageOverviewSelectRejection::StaleStamp)
            );
        }
        state.close();
        assert_eq!(
            state.validate_select(&command, stamp(), Some(11)),
            Err(PageOverviewSelectRejection::Closed)
        );
        let reopened = PageOverviewState::open(
            projection(4),
            PageOverviewStamp {
                incarnation: 45,
                ..stamp()
            },
            0,
            4,
            viewport(),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap();
        assert_eq!(
            reopened.validate_select(
                &command,
                PageOverviewStamp {
                    incarnation: 45,
                    ..stamp()
                },
                Some(11)
            ),
            Err(PageOverviewSelectRejection::StaleStamp)
        );
    }

    #[test]
    fn target_checks_ordinal_real_index_and_stable_source_identity() {
        let state = opened(4, 0, PageOverviewDirection::LeftToRight);
        let command = state.session().unwrap().view().capture_select(2).unwrap();
        let mut wrong_idx = command.clone();
        wrong_idx.target.items_idx = 3;
        let mut wrong_identity = command.clone();
        wrong_identity.target.identity = entry(3).page.identity.clone();
        let mut wrong_ordinal = command.clone();
        wrong_ordinal.target.ordinal = 3;
        let mut out_of_range = command;
        out_of_range.target.ordinal = 99;
        for stale in [wrong_idx, wrong_identity, wrong_ordinal, out_of_range] {
            assert_eq!(
                state.validate_select(&stale, stamp(), Some(11)),
                Err(PageOverviewSelectRejection::StaleTarget)
            );
        }
    }

    #[test]
    fn rejected_landing_keeps_open_and_accepted_landing_closes_once() {
        let mut state = opened(20, 3, PageOverviewDirection::LeftToRight);
        let command = state.session().unwrap().view().capture_select(19).unwrap();
        assert_eq!(
            state.select(&command, stamp(), Some(11), |_| false),
            Err(PageOverviewSelectRejection::NavigationRejected)
        );
        assert_eq!(state.session().unwrap().view().cursor, 3);
        let calls = Cell::new(0);
        let accepted = state
            .select(&command, stamp(), Some(11), |target| {
                calls.set(calls.get() + 1);
                assert_eq!(target.items_idx, 19);
                true
            })
            .unwrap();
        assert_eq!(accepted, command.target);
        assert_eq!(calls.get(), 1);
        assert!(matches!(state, PageOverviewState::Closed));
        assert_eq!(
            state.select(&command, stamp(), Some(11), |_| {
                calls.set(calls.get() + 1);
                true
            }),
            Err(PageOverviewSelectRejection::Closed)
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn white_companion_is_only_real_page_annotation_and_same_cell_target() {
        let mut real = entry(6);
        let page = Arc::make_mut(&mut real.page);
        page.white_companion = Some(WhiteCompanion::Composition);
        page.identity = PageIdentity::Document {
            container: "g:/book/source.epub".into(),
            page_num: 8,
        };
        let projection = Arc::new(
            PageOverviewProjection::new(
                Arc::new(PageOverviewMetadata::new([real.clone()]).unwrap()),
                Arc::new(vec![6]),
            )
            .unwrap(),
        );
        let state = PageOverviewState::open(
            projection,
            stamp(),
            6,
            4,
            viewport(),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap();
        let view = state.session().unwrap().view();
        assert_eq!(view.projection.len(), 1);
        assert_eq!(
            view.projection.entry(0).unwrap().page.white_companion,
            Some(WhiteCompanion::Composition)
        );
        assert!(view.capture_select(1).is_none());
        let grid = view.layout(viewport(), PageOverviewDirection::LeftToRight);
        let cell = grid.cell_rect(0, view.scroll_anchor).unwrap();
        let hit = grid
            .hit_test(
                cell.right_bottom() - egui::vec2(1.0, 1.0),
                view.scroll_anchor,
            )
            .unwrap();
        let command = view.capture_select(hit).unwrap();
        assert_eq!(command.target.items_idx, 6);
        assert_eq!(command.target.identity, real.page.identity);
        assert!(state.validate_select(&command, stamp(), Some(11)).is_ok());
    }
}
