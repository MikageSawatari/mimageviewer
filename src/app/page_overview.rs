//! Connect the pure overview to its existing viewer owner and ordinary absolute navigation.
use super::*;
use crate::page_overview::{
    PageOverviewCursorMove, PageOverviewDirection, PageOverviewLayout, PageOverviewSelect,
    PageOverviewStamp, PageOverviewState, PageOverviewView,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverviewThumbnailVisibility {
    LoadedReady,
    LoadedAwaitingRotation,
    Pending,
    Evicted,
    Failed,
    NoArt,
    MissingState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OverviewThumbnailDemandTrace {
    stamp: Option<PageOverviewStamp>,
    pages: Vec<(
        usize,
        PriorityThumbnailAdmission,
        bool,
        bool,
        OverviewThumbnailVisibility,
    )>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PageOverviewCommand {
    Cancel {
        stamp: PageOverviewStamp,
    },
    Select(PageOverviewSelect),
    SetCursor {
        stamp: PageOverviewStamp,
        target: crate::page_overview::PageOverviewTarget,
    },
    Action {
        stamp: PageOverviewStamp,
        action: crate::keymap::KeyAction,
        press_count: usize,
    },
    ScrollRows {
        stamp: PageOverviewStamp,
        delta: isize,
    },
    ColumnsDelta {
        stamp: PageOverviewStamp,
        delta: i8,
    },
}

impl PageOverviewCommand {
    pub(crate) fn action_count_is_valid(
        action: crate::keymap::KeyAction,
        press_count: usize,
    ) -> bool {
        press_count > 0
            && (matches!(
                action,
                crate::keymap::KeyAction::FsCtrlNavPrev
                    | crate::keymap::KeyAction::FsCtrlNavNext
                    | crate::keymap::KeyAction::FsSiblingPrev
                    | crate::keymap::KeyAction::FsSiblingNext
            ) || press_count == 1)
    }
    pub(super) fn stamp(&self) -> PageOverviewStamp {
        match self {
            Self::Cancel { stamp }
            | Self::ScrollRows { stamp, .. }
            | Self::ColumnsDelta { stamp, .. }
            | Self::SetCursor { stamp, .. }
            | Self::Action { stamp, .. } => *stamp,
            Self::Select(selection) => selection.stamp,
        }
    }
}

impl App {
    /// Sparse normal-log diagnostics. The UI cache only suppresses identical observations;
    /// it neither owns demand nor influences admission, cancellation, or worker completion.
    pub(super) fn trace_page_overview_thumbnail_demand(
        &self,
        ctx: &egui::Context,
        admissions: Vec<(usize, PriorityThumbnailAdmission)>,
    ) {
        let trace = OverviewThumbnailDemandTrace {
            stamp: self.page_overview_view().map(|view| view.stamp),
            pages: admissions
                .into_iter()
                .map(|(idx, admission)| {
                    let visibility = match self.thumbnails.get(idx) {
                        Some(ThumbnailState::Loaded { .. })
                            if self.rotation_cache.contains_key(&idx) =>
                        {
                            OverviewThumbnailVisibility::LoadedReady
                        }
                        Some(ThumbnailState::Loaded { .. }) => {
                            OverviewThumbnailVisibility::LoadedAwaitingRotation
                        }
                        Some(ThumbnailState::Pending) => OverviewThumbnailVisibility::Pending,
                        Some(ThumbnailState::Evicted) => OverviewThumbnailVisibility::Evicted,
                        Some(ThumbnailState::Failed) => OverviewThumbnailVisibility::Failed,
                        Some(ThumbnailState::NoArt) => OverviewThumbnailVisibility::NoArt,
                        None => OverviewThumbnailVisibility::MissingState,
                    };
                    (
                        idx,
                        admission,
                        self.requested.contains_key(&idx),
                        self.keep_set.contains(&idx),
                        visibility,
                    )
                })
                .collect(),
        };
        let id = egui::Id::new((
            "overview-demand-trace",
            self.projected_viewer_context_id().serial(),
        ));
        let changed = ctx.data_mut(|data| {
            if data.get_temp::<OverviewThumbnailDemandTrace>(id).as_ref() == Some(&trace) {
                false
            } else {
                data.insert_temp(id, trace.clone());
                true
            }
        });
        if changed {
            let line = format!(
                "[page-overview-demand] stage=Admission context={} viewport={:?} items_generation={} stamp={:?} page_tuple=(idx,reason,requested,kept,visibility) pages={:?}",
                self.projected_viewer_context_id().serial(),
                ctx.viewport_id(),
                self.items_generation,
                trace.stamp,
                trace.pages
            );
            crate::logger::log(&line);
            #[cfg(test)]
            eprintln!("{line}");
        }
    }

    /// All entry points preserve the existing exclusive mode/picker owner.
    /// A refusal never commits a preview, drops a picker, or changes presentation.
    pub(crate) fn page_overview_unavailable_reason(&mut self) -> Option<&'static str> {
        if self.remote_session_blocks_local_control() {
            return Some("Remote の操作中はページ一覧を操作できません");
        }
        let Some(idx) = self.fullscreen_idx else {
            return Some("閲覧中のページがありません");
        };
        if matches!(
            self.items.get(idx),
            Some(GridItem::Video(_) | GridItem::Audio(_))
        ) {
            return Some("動画・音声ではページ一覧を開けません");
        }
        if self.ring_picker.is_some()
            || self.gamepad_favorite_picker.is_some()
            || self.gamepad_location_picker.is_some()
            || self.gamepad_video_marker_picker.is_some()
        {
            return Some("選択画面を閉じてからページ一覧を開いてください");
        }
        if self.is_overlay_edit_mode_active()
            || self.view_trim_mode
            || self.capture_selection.is_some()
        {
            return Some("編集・範囲選択を終了してからページ一覧を開いてください");
        }
        if self.analysis_mode
            || self.compare_view_mode != CompareViewMode::Off
            || self.compare_pin_pending.is_some()
            || self.compare_pin_load_pending.is_some()
            || self.panorama_intent.on
            || self.is_panorama_mode_active(idx)
        {
            return Some("分析・比較・パノラマ表示を終了してからページ一覧を開いてください");
        }
        if self.document_open_modal_admission_blocked()
            || self.any_dialog_open()
            || self.fs_context_menu_idx.is_some()
            || self.context_menu_idx.is_some()
        {
            return Some("開いている操作画面を閉じてからページ一覧を開いてください");
        }
        if !self.fullscreen_seek_overlay_allowed(idx, false)
            || !self
                .fullscreen_seek_info_cached(idx)
                .is_some_and(|info| info.has_page_strip_content())
        {
            return Some("現在の閲覧列にはページ一覧を表示できません");
        }
        None
    }

    pub(crate) fn page_overview_available(&mut self) -> bool {
        self.page_overview_unavailable_reason().is_none()
    }

    /// Actual active ownership is distinct from a temporary registry projection.
    /// The renderer still verifies its concrete surface/viewport binding before dispatch.
    pub(crate) fn page_overview_active_context_serial(&self) -> Option<u64> {
        self.fullscreen_idx?;
        let projected = self.projected_viewer_context_id();
        #[cfg(windows)]
        {
            if self.mounted_viewer_context_id() != Some(projected) {
                return None;
            }
            match self.viewer_presentation {
                ViewerPresentation::DetachedWindow => {
                    let window_id = self.active_detached_session?.window_id;
                    let (actual, residence) = self.locate_window_context(window_id)?;
                    if actual != projected
                        || residence != ContextResidence::Mounted
                        || self.detached_viewer_window_id() != Some(window_id)
                    {
                        return None;
                    }
                }
                ViewerPresentation::MainWindow | ViewerPresentation::Fullscreen => {
                    if self.active_detached_session.is_some()
                        || projected != self.viewer_context_main()
                        || self.detached_viewer_window_id().is_some()
                    {
                        return None;
                    }
                }
            }
        }
        #[cfg(not(windows))]
        if self.viewer_presentation == ViewerPresentation::DetachedWindow {
            return None;
        }
        Some(projected.serial())
    }

    fn page_overview_stamp(
        &self,
        projection: &Arc<crate::page_overview::PageOverviewProjection>,
    ) -> PageOverviewStamp {
        PageOverviewStamp {
            context_serial: self.projected_viewer_context_id().serial(),
            items_generation: self.items_generation,
            // The cached immutable projection is the reader-order revision. The open view
            // retains its allocation, so another live revision cannot reuse this address.
            reader_order_revision: Arc::as_ptr(projection) as usize as u64,
            incarnation: self.page_overview_incarnation,
        }
    }

    pub(crate) fn open_page_overview(
        &mut self,
        viewport: egui::Rect,
        direction: PageOverviewDirection,
    ) -> bool {
        if !self.page_overview_available() || self.page_overview_active_context_serial().is_none() {
            return false;
        }
        let Some(projection) = self.prepared_page_overview_projection() else {
            return false;
        };
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        self.page_overview_incarnation = self
            .page_overview_incarnation
            .checked_add(1)
            .expect("page overview incarnation exhausted");
        let stamp = self.page_overview_stamp(&projection);
        let Ok(opened) = PageOverviewState::open(
            projection,
            stamp,
            idx,
            PageOverviewLayout::DEFAULT_COLUMNS,
            viewport,
            direction,
        ) else {
            return false;
        };
        self.page_overview = opened;
        self.fs_seek_drag_active = false;
        if matches!(
            self.fs_seek_gesture,
            crate::ui_fullscreen::StillSeekGesture::Track(_)
                | crate::ui_fullscreen::StillSeekGesture::Strip { .. }
        ) {
            self.fs_seek_gesture = Default::default();
        }
        true
    }

    pub(crate) fn close_page_overview(&mut self) {
        if self.page_overview.session().is_none() {
            return;
        }
        self.page_overview.close();
        self.clear_still_seek_thumbnail_requests();
        if self.slideshow_playing {
            self.schedule_next_slideshow_from_now();
        }
    }

    pub(crate) fn close_page_overview_with_ctx(&mut self, ctx: &egui::Context) {
        self.close_page_overview();
        self.reconcile_foreground_thumbnail_keep_owners(Some(ctx));
    }

    pub(crate) fn page_overview_view(&self) -> Option<PageOverviewView> {
        self.page_overview
            .session()
            .map(|session| session.view().clone())
    }

    pub(crate) fn page_overview_action_stamp_is_current(
        &mut self,
        stamp: PageOverviewStamp,
    ) -> bool {
        self.prepared_page_overview_projection()
            .is_some_and(|projection| {
                self.page_overview_stamp(&projection) == stamp
                    && self
                        .page_overview
                        .session()
                        .is_some_and(|session| session.view().stamp == stamp)
            })
    }

    /// Reject an inactive Select before any admission, navigation or App-global side effect.
    /// Other commands remain context-local and can update a passive owner's overview.
    pub(crate) fn handle_page_overview_command(
        &mut self,
        ctx: &egui::Context,
        command: &PageOverviewCommand,
        viewport: egui::Rect,
        direction: PageOverviewDirection,
        active_context_serial: Option<u64>,
    ) -> bool {
        if self.remote_session_blocks_local_control() {
            return false;
        }
        if matches!(
            command,
            PageOverviewCommand::Select(_)
                | PageOverviewCommand::SetCursor { .. }
                | PageOverviewCommand::Action { .. }
        ) && (active_context_serial != Some(self.projected_viewer_context_id().serial())
            || self.page_overview_active_context_serial() != active_context_serial)
        {
            return false;
        }
        let Some(projection) = self.prepared_page_overview_projection() else {
            return false;
        };
        let current_stamp = self.page_overview_stamp(&projection);
        if !overview_command_matches(&self.page_overview, command, current_stamp) {
            return false;
        }
        match command {
            PageOverviewCommand::Cancel { .. } => self.close_page_overview_with_ctx(ctx),
            PageOverviewCommand::Select(selection) => {
                let Ok(target) = self.page_overview.validate_select(
                    selection,
                    current_stamp,
                    active_context_serial,
                ) else {
                    return false;
                };
                if !self.page_overview_available()
                    || self.fs_navigation_sequence_blocks_new_target()
                {
                    return false;
                }
                let Some(current_idx) = self.fullscreen_idx else {
                    return false;
                };
                self.land_still_page_navigation_target(
                    ctx,
                    current_idx,
                    target.items_idx,
                    crate::fs_page_load_scheduler::FsPageLoadContract::LatestSeek,
                    &mut None,
                );
                if self.fullscreen_idx != Some(target.items_idx) {
                    return false;
                }
                self.close_page_overview_with_ctx(ctx);
            }
            PageOverviewCommand::SetCursor { stamp, target } => {
                let selection = PageOverviewSelect {
                    stamp: *stamp,
                    target: target.clone(),
                };
                if self
                    .page_overview
                    .validate_select(&selection, current_stamp, active_context_serial)
                    .is_err()
                {
                    return false;
                }
                let session = self
                    .page_overview
                    .session_mut()
                    .expect("validated open session");
                let layout = session.view().layout(viewport, direction);
                if !session.set_cursor_target(target, &layout) {
                    return false;
                }
            }
            PageOverviewCommand::Action {
                action,
                press_count,
                ..
            } => {
                if !crate::keymap::page_overview_action_allowed(*action)
                    || !PageOverviewCommand::action_count_is_valid(*action, *press_count)
                    || !self.page_overview_executable_actions().contains(action)
                {
                    return false;
                }
                return self.execute_page_overview_key_action_count(ctx, action, *press_count);
            }
            PageOverviewCommand::ScrollRows { .. } | PageOverviewCommand::ColumnsDelta { .. } => {
                if !apply_overview_layout_command(
                    &mut self.page_overview,
                    command,
                    current_stamp,
                    viewport,
                    direction,
                ) {
                    return false;
                }
            }
        }
        ctx.request_repaint();
        true
    }

    pub(crate) fn move_page_overview_cursor(
        &mut self,
        movement: PageOverviewCursorMove,
        viewport: egui::Rect,
        direction: PageOverviewDirection,
    ) -> bool {
        let Some(projection) = self.prepared_page_overview_projection() else {
            return false;
        };
        let current_stamp = self.page_overview_stamp(&projection);
        let Some(session) = self.page_overview.session_mut() else {
            return false;
        };
        if session.view().stamp != current_stamp {
            return false;
        }
        let layout = session.view().layout(viewport, direction);
        session.move_cursor(movement, &layout);
        true
    }
}

fn overview_command_matches(
    state: &PageOverviewState,
    command: &PageOverviewCommand,
    current_stamp: PageOverviewStamp,
) -> bool {
    state.session().is_some_and(|session| {
        session.view().stamp == current_stamp && command.stamp() == current_stamp
    })
}

fn apply_overview_layout_command(
    state: &mut PageOverviewState,
    command: &PageOverviewCommand,
    current_stamp: PageOverviewStamp,
    viewport: egui::Rect,
    direction: PageOverviewDirection,
) -> bool {
    if !overview_command_matches(state, command, current_stamp) {
        return false;
    }
    let session = state.session_mut().expect("validated open session");
    match command {
        PageOverviewCommand::ScrollRows { delta, .. } => {
            let layout = session.view().layout(viewport, direction);
            session.scroll_rows(*delta, &layout);
        }
        PageOverviewCommand::ColumnsDelta { delta, .. } => {
            let columns = (i16::from(session.view().preferred_columns) + i16::from(*delta)).clamp(
                i16::from(PageOverviewLayout::MIN_COLUMNS),
                i16::from(PageOverviewLayout::MAX_COLUMNS),
            ) as u8;
            session.set_preferred_columns(columns, viewport, direction);
        }
        PageOverviewCommand::Cancel { .. }
        | PageOverviewCommand::Select(_)
        | PageOverviewCommand::SetCursor { .. }
        | PageOverviewCommand::Action { .. } => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_overview::{
        PageOverviewEntry, PageOverviewMetadata, PageOverviewPage, PageOverviewProjection,
    };

    fn fixture() -> (PageOverviewState, PageOverviewStamp, egui::Rect) {
        let rows: Vec<_> = (0..50)
            .map(|idx| GridItem::Image(format!("page/{idx}.jpg").into()))
            .collect();
        let metadata = Arc::new(
            PageOverviewMetadata::new(rows.iter().enumerate().map(|(idx, item)| {
                PageOverviewEntry {
                    items_idx: idx,
                    page: Arc::new(PageOverviewPage {
                        identity: crate::book_resume_db::PageIdentity::for_item(item, None)
                            .unwrap(),
                        label: Arc::from(item.name().as_ref()),
                        white_companion: None,
                    }),
                }
            }))
            .unwrap(),
        );
        let projection =
            Arc::new(PageOverviewProjection::new(metadata, Arc::new((0..50).collect())).unwrap());
        let stamp = PageOverviewStamp {
            context_serial: 1,
            items_generation: 2,
            reader_order_revision: 3,
            incarnation: 4,
        };
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 300.0));
        (
            PageOverviewState::open(projection, stamp, 0, 5, viewport, Default::default()).unwrap(),
            stamp,
            viewport,
        )
    }

    #[test]
    fn overview_layout_handler_is_stamped_and_never_changes_reading_target() {
        let (mut state, stamp, viewport) = fixture();
        let selection = state.session().unwrap().view().capture_select(0).unwrap();
        let stale = PageOverviewStamp {
            incarnation: stamp.incarnation + 1,
            ..stamp
        };
        let scroll = PageOverviewCommand::ScrollRows { stamp, delta: 3 };
        assert!(!apply_overview_layout_command(
            &mut state,
            &scroll,
            stale,
            viewport,
            Default::default()
        ));
        assert_eq!(state.session().unwrap().view().scroll_anchor.row, 0);
        assert!(apply_overview_layout_command(
            &mut state,
            &scroll,
            stamp,
            viewport,
            Default::default()
        ));
        assert_eq!(state.session().unwrap().view().scroll_anchor.row, 3);
        assert!(apply_overview_layout_command(
            &mut state,
            &PageOverviewCommand::ColumnsDelta {
                stamp,
                delta: i8::MAX,
            },
            stamp,
            viewport,
            Default::default()
        ));
        let view = state.session().unwrap().view();
        assert_eq!(view.preferred_columns, 20);
        assert_eq!(view.capture_select(view.cursor), Some(selection));
        assert!(!overview_command_matches(
            &state,
            &PageOverviewCommand::Cancel { stamp: stale },
            stamp
        ));
        assert!(overview_command_matches(
            &state,
            &PageOverviewCommand::Cancel { stamp },
            stamp
        ));
    }

    #[test]
    fn overview_selection_handler_preserves_open_when_active_owner_changed() {
        let (mut state, stamp, _) = fixture();
        let selection = state.session().unwrap().view().capture_select(40).unwrap();
        let landed = std::cell::Cell::new(false);
        assert_eq!(
            state.select(&selection, stamp, Some(99), |_| {
                landed.set(true);
                true
            }),
            Err(crate::page_overview::PageOverviewSelectRejection::InactiveOwner)
        );
        assert!(!landed.get());
        assert!(state.session().is_some());
        // Restoring the owner only restores the session, never replays the rejected command.
        assert_eq!(state.session().unwrap().view().cursor, 0);
        assert_eq!(
            state
                .validate_select(&selection, stamp, Some(1))
                .unwrap()
                .items_idx,
            40
        );
    }

    #[test]
    fn inactive_counted_overview_action_has_no_navigation_or_presentation_effects() {
        let mut app = crate::app::setup_app_for_test();
        let (state, stamp, viewport) = fixture();
        app.page_overview = state;
        app.fullscreen_idx = Some(0);
        app.selected = Some(9);
        let before = (
            app.fullscreen_idx,
            app.selected,
            app.items_generation,
            app.pending_folder_nav_steps,
            app.viewer_presentation,
            app.slideshow_playing,
        );
        let ctx = egui::Context::default();
        ctx.begin_pass(Default::default());
        for (action, press_count) in [
            (crate::keymap::KeyAction::FsCtrlNavNext, 3),
            (crate::keymap::KeyAction::FsToggleWindowMode, 1),
        ] {
            assert!(!app.handle_page_overview_command(
                &ctx,
                &PageOverviewCommand::Action {
                    stamp,
                    action,
                    press_count
                },
                viewport,
                Default::default(),
                Some(99)
            ));
        }
        assert_eq!(
            (
                app.fullscreen_idx,
                app.selected,
                app.items_generation,
                app.pending_folder_nav_steps,
                app.viewer_presentation,
                app.slideshow_playing
            ),
            before
        );
        assert!(app.page_overview.session().is_some());
        assert_eq!(app.page_overview.session().unwrap().view().cursor, 0);
        let output = ctx.end_pass();
        assert!(
            output
                .viewport_output
                .values()
                .all(|viewport| viewport.commands.is_empty())
        );
    }

    #[test]
    fn overview_counted_navigation_keeps_locked_dfs_accumulation_and_existing_cap() {
        for (action, mode) in [
            (
                crate::keymap::KeyAction::FsCtrlNavNext,
                FolderNavMode::Fullscreen,
            ),
            (
                crate::keymap::KeyAction::FsSiblingNext,
                FolderNavMode::SiblingFullscreen,
            ),
        ] {
            for (press_count, expected) in [(3, 3), (10, super::super::MAX_PENDING_NAV)] {
                let mut app = crate::app::setup_app_for_test();
                let current = app.tmp.path().to_path_buf();
                app.current_folder = Some(current.clone());
                app.items = vec![GridItem::Image(current.join("overview.jpg"))];
                app.visible_indices = vec![0];
                app.thumbnails = vec![ThumbnailState::Pending];
                app.image_metas = vec![None];
                app.fullscreen_idx = Some(0);
                app.page_identity_index = Arc::new(
                    super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                        &app.items,
                        None,
                        Default::default(),
                    ),
                );
                let viewport =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
                assert!(app.open_page_overview(viewport, Default::default()));
                let (_sender, rx) = std::sync::mpsc::channel();
                app.folder_nav_pending = Some(FolderNavPending {
                    cancel: Arc::new(AtomicBool::new(false)),
                    rx,
                    forward: true,
                    mode: mode.clone(),
                });
                app.pending_folder_nav_mode = mode.clone();
                app.pending_folder_nav_steps = 0;
                app.fs_nav_locked_gen = Some(app.items_generation);
                assert!(app.page_overview_executable_actions().contains(&action));
                assert!(app.execute_page_overview_key_action_count(
                    &egui::Context::default(),
                    &action,
                    press_count
                ));
                assert_eq!(app.pending_folder_nav_steps, expected);
                assert!(app.page_overview.session().is_none());
                assert!(app.folder_nav_pending.is_some());
                assert_eq!(app.fullscreen_idx, Some(0));
            }
        }
    }

    #[test]
    fn overview_unavailable_filtered_navigation_does_not_close_or_mutate_session() {
        let mut app = crate::app::setup_app_for_test();
        let current = app.tmp.path().to_path_buf();
        app.current_folder = Some(current.clone());
        app.items = vec![GridItem::Image(current.join("overview.jpg"))];
        app.visible_indices = vec![0];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        app.fullscreen_idx = Some(0);
        app.page_identity_index = Arc::new(
            super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                &app.items,
                None,
                Default::default(),
            ),
        );
        assert!(app.open_page_overview(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0)),
            Default::default()
        ));
        let before = app.page_overview_view().unwrap();
        assert_eq!(
            before.preferred_columns,
            PageOverviewLayout::DEFAULT_COLUMNS
        );
        app.show_search_bar = true;
        for action in [
            crate::keymap::KeyAction::FsCtrlNavPrev,
            crate::keymap::KeyAction::FsCtrlNavNext,
            crate::keymap::KeyAction::FsSiblingPrev,
            crate::keymap::KeyAction::FsSiblingNext,
        ] {
            assert!(!app.page_overview_executable_actions().contains(&action));
            assert!(!app.execute_page_overview_key_action_count(
                &egui::Context::default(),
                &action,
                3
            ));
            assert!(app.page_overview.session().is_some());
        }
        let after = app.page_overview_view().unwrap();
        assert_eq!(after.stamp, before.stamp);
        assert_eq!(after.cursor, before.cursor);
        assert_eq!(after.scroll_anchor, before.scroll_anchor);
        assert!(app.folder_nav_pending.is_none());
    }
}
