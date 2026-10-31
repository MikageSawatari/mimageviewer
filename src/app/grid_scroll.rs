//! Context-owned canonical grid position and the two existing request lifetimes.
//!
//! A reveal can run before a winning edge request in the same layout. The independent
//! row anchor is consumed only by a reveal with a resolved selection, never by an edge
//! or a failed selection lookup. Native readback must not issue another position request.

use super::{GridScrollIntent, ViewerContextId};

/// Runtime input lifetime, deliberately independent of path/items generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GridScrollSessionId(u64);
impl Default for GridScrollSessionId {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Keep the caller's geometry and ScrollArea's minimum inner height identical.
pub(crate) const GRID_SCROLL_MIN_VIEWPORT_H: f32 = 64.0;

/// Last declared and drawn grid geometry, scoped with its canonical position.
/// Before the first paint, use the same navigation defaults as App historically used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridScrollGeometry {
    pub(crate) cols: usize,
    pub(crate) row_h: f32,
    pub(crate) viewport_h: f32,
    pub(crate) content_h: f32,
    pub(crate) max_offset: f32,
}

impl Default for GridScrollGeometry {
    fn default() -> Self {
        Self::new(4, 200.0, 600.0, 0.0)
    }
}

impl GridScrollGeometry {
    pub(crate) fn new(cols: usize, row_h: f32, viewport_h: f32, natural_h: f32) -> Self {
        let cols = cols.max(1);
        let row_h = row_h.max(1.0);
        let viewport_h = viewport_h.max(GRID_SCROLL_MIN_VIEWPORT_H);
        let (content_h, max_offset) = Self::snapped_extent(natural_h, viewport_h, row_h);
        Self {
            cols,
            row_h,
            viewport_h,
            content_h,
            max_offset,
        }
    }

    pub(crate) fn snapped_extent(natural_h: f32, viewport_h: f32, row_h: f32) -> (f32, f32) {
        let natural_h = natural_h.max(0.0);
        let viewport_h = viewport_h.max(0.0);
        if natural_h <= viewport_h {
            return (natural_h, 0.0);
        }
        let row_h = row_h.max(1.0);
        let max_offset = ((natural_h - viewport_h) / row_h).ceil() * row_h;
        (max_offset + viewport_h, max_offset)
    }
}

/// Proof that the owned row geometry was painted for this context/generation.
/// Geometry itself is stored once; the proof only adds the thumbnail width needed
/// for an explicit aspect conversion before the next paint.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum GridAspectLayout {
    #[default]
    UnlaidOut,
    Thumbnail {
        context_id: ViewerContextId,
        items_generation: u64,
        cell_width: f32,
    },
    Details,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedGridScroll {
    pub(crate) geometry: GridScrollGeometry,
    pub(crate) position: egui::scroll_area::ExternalScrollPosition,
}

impl ResolvedGridScroll {
    pub(crate) fn external(
        self,
        interaction_session: egui::Id,
        interaction_id: egui::Id,
    ) -> egui::scroll_area::ExternalVerticalScroll {
        egui::scroll_area::ExternalVerticalScroll {
            content_height: self.geometry.content_h,
            position: self.position,
            interaction_session,
            interaction_id,
        }
    }
}

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
    geometry: GridScrollGeometry,
    aspect_layout: GridAspectLayout,
    session: GridScrollSessionId,
}

impl GridScrollState {
    pub(crate) fn session(&self) -> GridScrollSessionId {
        self.session
    }

    /// End only input continuity. Position, placement requests and layout remain owned.
    pub(crate) fn renew_session(&mut self) {
        self.session = GridScrollSessionId::default();
    }

    pub(crate) fn scope(&self, context: ViewerContextId) -> egui::Id {
        egui::Id::unique(("miv_grid_scroll_session", context, self.session))
    }

    pub(crate) fn geometry(&self) -> GridScrollGeometry {
        self.geometry
    }

    pub(crate) fn aspect_layout(&self) -> GridAspectLayout {
        self.aspect_layout
    }

    #[cfg(test)]
    pub(crate) fn aspect_layout_mut_for_test(&mut self) -> &mut GridAspectLayout {
        &mut self.aspect_layout
    }

    pub(crate) fn record_thumbnail_geometry(
        &mut self,
        geometry: GridScrollGeometry,
        context_id: ViewerContextId,
        items_generation: u64,
        cell_width: f32,
    ) {
        self.geometry = geometry;
        self.aspect_layout = GridAspectLayout::Thumbnail {
            context_id,
            items_generation,
            cell_width,
        };
    }

    pub(crate) fn record_details_geometry(&mut self, geometry: GridScrollGeometry) {
        self.geometry = geometry;
        self.aspect_layout = GridAspectLayout::Details;
    }

    /// Convert a proven thumbnail layout and its canonical anchor atomically. This
    /// is an explicit coordinate transform of the painted layout, never a prediction.
    pub(crate) fn reanchor_aspect(
        &mut self,
        context_id: ViewerContextId,
        items_generation: u64,
        cols: usize,
        height_ratio: f32,
        row_count: usize,
    ) -> Option<f32> {
        let GridAspectLayout::Thumbnail {
            context_id: owner,
            items_generation: generation,
            cell_width,
        } = self.aspect_layout
        else {
            return None;
        };
        if owner != context_id || generation != items_generation || self.geometry.cols != cols {
            return None;
        }
        let new_row_h = (cell_width * height_ratio).round().max(1.0);
        if (new_row_h - self.geometry.row_h).abs() >= 0.5 {
            let row = (self.offset_y / self.geometry.row_h).floor();
            self.geometry = GridScrollGeometry::new(
                cols,
                new_row_h,
                self.geometry.viewport_h,
                row_count as f32 * new_row_h,
            );
            self.set_position(row * new_row_h);
        }
        Some(new_row_h)
    }

    /// Native readback is rounded and clamped in the same current row coordinate
    /// system. It never queues another Set for the following frame.
    pub(crate) fn adopt_readback(&mut self, offset: f32) {
        self.offset_y = ((offset / self.geometry.row_h).round() * self.geometry.row_h)
            .clamp(0.0, self.geometry.max_offset);
    }

    /// Explicit setup for an existing final layout, including headless navigation fixtures.
    pub(crate) fn record_geometry(&mut self, geometry: GridScrollGeometry) {
        self.geometry = geometry;
    }

    /// Resolve exactly once with this frame's real layout. An early reveal is supplied
    /// separately so a new header-generated reveal stays queued for the following frame.
    pub(crate) fn resolve_layout(
        &mut self,
        geometry: GridScrollGeometry,
        reveal: bool,
        selected_position: Option<usize>,
        snap_to_row: bool,
    ) -> ResolvedGridScroll {
        let before = self.offset_y;
        self.record_geometry(geometry);
        if snap_to_row {
            self.offset_y = (self.offset_y / geometry.row_h).round() * geometry.row_h;
        }
        let anchor_applied = reveal && selected_position.is_some() && self.row_anchor.is_some();
        if reveal && let Some(position) = selected_position {
            self.apply_selection(position);
        }
        let request = self.take_layout_request();
        self.offset_y = match request.edge() {
            Some(GridScrollIntent::Top) => 0.0,
            Some(GridScrollIntent::Bottom) => geometry.max_offset,
            None => self.offset_y.clamp(0.0, geometry.max_offset),
        };
        let explicit = matches!(
            request,
            GridScrollRequest::Position { .. } | GridScrollRequest::Edge { .. }
        );
        let position = if explicit || anchor_applied || before != self.offset_y {
            egui::scroll_area::ExternalScrollPosition::Set(self.offset_y)
        } else {
            egui::scroll_area::ExternalScrollPosition::Sync(self.offset_y)
        };
        ResolvedGridScroll { geometry, position }
    }

    /// Only a reveal with a resolved selection consumes the independent row anchor.
    pub(crate) fn apply_selection(&mut self, selected_position: usize) {
        let geometry = self.geometry;
        let row = selected_position / geometry.cols;
        if let Some(rows_above) = self.take_row_anchor() {
            self.offset_y = row.saturating_sub(rows_above as usize) as f32 * geometry.row_h;
            return;
        }
        let row_top = row as f32 * geometry.row_h;
        let row_bottom = row_top + geometry.row_h;
        if row_top < self.offset_y {
            self.offset_y = row_top;
        } else if row_bottom > self.offset_y + geometry.viewport_h {
            self.offset_y = ((row_bottom - geometry.viewport_h).max(0.0) / geometry.row_h).ceil()
                * geometry.row_h;
        }
    }

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
            geometry: self.geometry,
            aspect_layout: GridAspectLayout::UnlaidOut,
            session: GridScrollSessionId::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_scroll_session_renewal_preserves_position_requests_anchor_and_geometry() {
        let mut state = GridScrollState::default();
        state.set_position(280.0);
        state.set_row_anchor(Some(3));
        state.set_reveal_requested(true);
        state.request_edge(GridScrollIntent::Bottom);
        state.record_geometry(GridScrollGeometry::new(1, 28.0, 280.0, 1120.0));
        let before = state.clone();
        state.renew_session();
        assert_ne!(state.session(), before.session());
        assert_eq!(state.offset_y, before.offset_y);
        assert_eq!(state.request, before.request);
        assert_eq!(state.row_anchor, before.row_anchor);
        assert_eq!(state.geometry, before.geometry);
        assert_eq!(state.aspect_layout, before.aspect_layout);
        let fork = state.fork();
        assert_ne!(fork.session(), state.session());
        assert_eq!(fork.offset(), state.offset());
        let second_fork = state.fork();
        assert_ne!(fork.session(), second_fork.session());
        let mut at_rest = GridScrollState::default();
        let session = state.session();
        std::mem::swap(&mut state, &mut at_rest);
        assert_eq!(
            at_rest.session(),
            session,
            "a transfer cannot mint an input lifetime"
        );
    }

    #[test]
    fn grid_scroll_session_same_path_ab_without_paint_is_always_fresh() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("same-path-ab");
        app.current_folder = Some(folder.clone());
        app.items_are_drive_list = false;
        app.set_quick_folder_slot_target(crate::app::QuickFolderSlotId::A, folder.clone());
        app.set_quick_folder_slot_target(crate::app::QuickFolderSlotId::B, folder);
        app.active_quick_folder_slot = Some(crate::app::QuickFolderSlotId::A);
        app.grid_scroll.set_position(280.0);
        app.grid_scroll.set_row_anchor(Some(2));
        app.grid_scroll.request_edge(GridScrollIntent::Bottom);
        let first = app.grid_scroll.session();
        let generation = app.items_generation;
        assert!(matches!(
            app.activate_quick_folder_slot(crate::app::QuickFolderSlotId::B),
            crate::app::QuickFolderSwitchTarget::Current
        ));
        let second = app.grid_scroll.session();
        assert_ne!(second, first);
        assert!(matches!(
            app.activate_quick_folder_slot(crate::app::QuickFolderSlotId::A),
            crate::app::QuickFolderSwitchTarget::Current
        ));
        assert_ne!(app.grid_scroll.session(), first);
        assert_ne!(app.grid_scroll.session(), second);
        assert_eq!(app.grid_scroll.offset(), 280.0);
        assert_eq!(app.grid_scroll.row_anchor(), Some(2));
        assert_eq!(
            app.grid_scroll.pending_edge(),
            Some(GridScrollIntent::Bottom)
        );
        assert_eq!(app.items_generation, generation);
    }

    #[test]
    fn grid_scroll_session_adopted_location_changes_but_refresh_and_capture_do_not() {
        let mut app = crate::app::setup_app_for_test();
        let first = app.tmp.path().join("first-list");
        let second = app.tmp.path().join("second-list");
        app.current_folder = Some(first.clone());
        app.items_are_drive_list = false;
        let location = crate::app::FolderNavHistoryTarget::Path(first.clone());
        app.adopt_main_facet_route(crate::app::FacetRoute::root(
            crate::app::App::facet_scope_for_location(&location),
        ));
        let initial = app.grid_scroll.session();
        let request = app.capture_main_list_navigation(
            crate::app::MainHistoryOperation::SameLocation,
            crate::app::MainListSourceProof::Row,
        );
        let destination = app.main_list_requested_destination(
            &request,
            crate::app::FolderNavHistoryTarget::Path(second.clone()),
            None,
        );
        drop((request, destination));
        assert_eq!(
            app.grid_scroll.session(),
            initial,
            "an abandoned request is not a visible adoption"
        );
        for (path, operation, changes) in [
            (
                first.clone(),
                crate::app::MainHistoryOperation::SameLocation,
                false,
            ),
            (
                second,
                crate::app::MainHistoryOperation::Direct(
                    crate::app::DirectNavigationPurpose::Navigation,
                ),
                true,
            ),
            (
                first,
                crate::app::MainHistoryOperation::Direct(
                    crate::app::DirectNavigationPurpose::Navigation,
                ),
                true,
            ),
        ] {
            let before = app.grid_scroll.session();
            let navigation =
                app.capture_main_list_navigation(operation, crate::app::MainListSourceProof::Row);
            let destination = app.main_list_requested_destination(
                &navigation,
                crate::app::FolderNavHistoryTarget::Path(path.clone()),
                None,
            );
            let adoption = app
                .prepare_main_list_adoption(navigation, destination)
                .unwrap();
            assert!(app.adopt_main_list_navigation(adoption, |owner| {
                owner.current_folder = Some(path);
                owner.bump_items_generation();
                None
            }));
            assert_eq!(app.grid_scroll.session() != before, changes);
        }
    }

    #[test]
    fn grid_scroll_readback_rounds_then_clamps_to_current_row_boundary_without_set() {
        let mut state = GridScrollState::default();
        let geometry = GridScrollGeometry::new(1, 28.0, 280.0, 840.0);
        state.record_geometry(geometry);
        for (raw, expected) in [(-40.0, 0.0), (43.0, 56.0), (576.0, 560.0), (900.0, 560.0)] {
            state.adopt_readback(raw);
            assert_eq!(state.offset(), expected);
            assert_eq!(state.offset() % geometry.row_h, 0.0);
            assert!(state.offset() <= geometry.max_offset);
            assert_eq!(state.take_request(), GridScrollRequest::Keep);
        }
        assert_eq!(
            state.resolve_layout(geometry, false, None, false).position,
            egui::scroll_area::ExternalScrollPosition::Sync(560.0)
        );
    }

    #[test]
    fn grid_scroll_aspect_proof_updates_geometry_and_offset_together_only_for_own_rows() {
        let context = ViewerContextId::for_test(1);
        let geometry = GridScrollGeometry::new(4, 100.0, 400.0, 2000.0);
        let mut state = GridScrollState::default();
        state.record_thumbnail_geometry(geometry, context, 7, 100.0);
        state.adopt_offset(800.0);
        for (owner, generation, cols) in [
            (ViewerContextId::for_test(2), 7, 4),
            (context, 8, 4),
            (context, 7, 5),
        ] {
            let before = state.clone();
            assert_eq!(
                state.reanchor_aspect(owner, generation, cols, 1.5, 20),
                None
            );
            assert_eq!(state, before);
        }
        assert_eq!(state.reanchor_aspect(context, 7, 4, 1.5, 20), Some(150.0));
        assert_eq!(state.offset(), 1200.0);
        assert_eq!(
            state.geometry(),
            GridScrollGeometry::new(4, 150.0, 400.0, 3000.0)
        );
        assert_eq!(state.geometry().max_offset, 2700.0);
        let converted = state.clone();
        state.reanchor_aspect(context, 7, 4, 1.5, 20);
        assert_eq!(
            state, converted,
            "same-frame second transform must be a no-op"
        );
        let mut fork = state.fork();
        let before = fork.clone();
        assert_eq!(fork.reanchor_aspect(context, 7, 4, 2.0, 20), None);
        assert_eq!(
            fork, before,
            "fresh fork cannot claim the source's painted proof"
        );
        state.record_details_geometry(GridScrollGeometry::new(1, 28.0, 400.0, 560.0));
        let before = state.clone();
        assert_eq!(state.reanchor_aspect(context, 7, 4, 2.0, 20), None);
        assert_eq!(state, before);
    }

    #[test]
    fn grid_scroll_aspect_fixup_preserves_predraw_cursor_and_f5_hint() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("aspect-cursor");
        app.current_folder = Some(folder.clone());
        app.settings.last_folder = Some(folder.clone());
        app.items_are_drive_list = false;
        app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
        app.settings.grid_cols = 4;
        app.items = (0..80)
            .map(|index| crate::grid_item::GridItem::Image(folder.join(format!("{index:02}.png"))))
            .collect();
        app.visible_indices = (0..80).collect();
        app.selected = Some(36);
        let context = app.projected_viewer_context_id();
        let generation = app.items_generation;
        app.grid_scroll.record_thumbnail_geometry(
            GridScrollGeometry::new(4, 100.0, 400.0, 2000.0),
            context,
            generation,
            100.0,
        );
        app.last_cell_size = 100.0;
        app.grid_scroll.set_position(800.0);
        assert_eq!(app.cursor_to_persist(), Some(("36.png".into(), 1)));
        app.fixup_scroll_for_aspect_change(crate::settings::ThumbAspect::Portrait2x3);
        assert_eq!(app.grid_scroll.offset(), 1200.0);
        assert_eq!(app.grid_scroll.geometry().row_h, 150.0);
        // No paint occurred: exit/departure and F5 must already share the transformed coordinates.
        assert_eq!(app.cursor_to_persist(), Some(("36.png".into(), 1)));
        let crate::app::StartupListIntent::RefreshList { cursor, .. } =
            app.current_list_refresh_intent()
        else {
            panic!("F5 must capture the physical list cursor");
        };
        assert_eq!(
            cursor.unwrap(),
            crate::settings::ListCursorHint {
                name: "36.png".into(),
                rows_above: Some(1),
            }
        );
    }

    #[test]
    fn grid_scroll_geometry_clamp_and_snap_are_explicit_even_after_becoming_valid() {
        let mut state = GridScrollState::default();
        state.adopt_offset(840.0);
        let geometry = GridScrollGeometry::new(1, 28.0, 280.0, 840.0);
        let resolved = state.resolve_layout(geometry, false, None, false);
        assert_eq!(geometry.max_offset, 560.0);
        assert_eq!(
            resolved.position,
            egui::scroll_area::ExternalScrollPosition::Set(560.0)
        );
        assert_eq!(state.offset(), 560.0);
        assert_eq!(
            state.resolve_layout(geometry, false, None, false).position,
            egui::scroll_area::ExternalScrollPosition::Sync(560.0)
        );
        state.adopt_offset(545.0);
        assert_eq!(
            state.resolve_layout(geometry, false, None, true).position,
            egui::scroll_area::ExternalScrollPosition::Set(532.0)
        );
        state.adopt_offset(532.0);
        assert_eq!(
            state.resolve_layout(geometry, false, None, false).position,
            egui::scroll_area::ExternalScrollPosition::Sync(532.0)
        );
    }

    #[test]
    fn grid_scroll_resolver_combines_restore_reveal_edge_and_deferred_header_reveal() {
        let mut state = GridScrollState::default();
        let geometry = GridScrollGeometry::new(1, 28.0, 280.0, 2800.0);
        state.set_position(560.0);
        state.set_reveal_requested(true);
        let reveal = state.take_reveal();
        state.set_reveal_requested(true); // A later header action, for the next frame.
        state.set_row_anchor(Some(2));
        state.request_edge(GridScrollIntent::Bottom);
        let resolved = state.resolve_layout(geometry, reveal, Some(30), false);
        assert_eq!(
            resolved.position,
            egui::scroll_area::ExternalScrollPosition::Set(2520.0)
        );
        assert_eq!(state.row_anchor(), None);
        assert!(state.reveal_requested());
        assert_eq!(state.pending_edge(), None);
        assert!(state.take_reveal());
    }

    #[test]
    fn grid_scroll_geometry_records_actual_columns_and_enforces_shared_minimum() {
        let mut state = GridScrollState::default();
        let thumbnail = GridScrollGeometry::new(10, 100.0, 300.0, 2000.0);
        state.resolve_layout(thumbnail, false, None, false);
        let details = GridScrollGeometry::new(1, 28.0, 20.0, 560.0);
        state.resolve_layout(details, false, None, false);
        assert_eq!(state.geometry().cols, 1);
        assert_eq!(state.geometry().row_h, 28.0);
        assert_eq!(state.geometry().viewport_h, GRID_SCROLL_MIN_VIEWPORT_H);
        assert_eq!(
            state.geometry().content_h - state.geometry().viewport_h,
            details.max_offset
        );
        assert_eq!(state.fork().geometry(), details);
    }

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
