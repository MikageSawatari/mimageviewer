//! The existing navigation return slot outlives a terminal viewer.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerCloseCause {
    User,
    Replaced,
    RemoteAcquire,
    AppExit,
}

#[derive(Debug, Default)]
pub(crate) enum FullscreenCloseContinuation {
    #[default]
    None,
    /// An existing viewer's navigation origin, without a pending UI request.
    Viewing(Box<CloseReturnOrigin>),
    Live(Option<Box<CloseReturnOrigin>>),
    Terminal(Box<CloseReturn>),
}

impl FullscreenCloseContinuation {
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self, Self::Live(_) | Self::Terminal(_))
    }
    pub(super) fn preserves_live_bookmark_grid(&self) -> bool {
        matches!(
            self,
            Self::Terminal(request)
                if matches!(request.effect, CloseReturnEffect::BookmarkRefresh { .. })
        )
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::None;
    }
    pub(crate) fn request_live(&mut self) {
        *self = match std::mem::take(self) {
            Self::Viewing(origin) | Self::Live(Some(origin)) => Self::Live(Some(origin)),
            terminal @ Self::Terminal(_) => terminal,
            _ => Self::Live(None),
        };
    }
    pub(super) fn cancel_live_request(&mut self) {
        *self = match std::mem::take(self) {
            Self::Live(origin) => origin.map_or(Self::None, Self::Viewing),
            state => state,
        };
    }
    fn origin(&self) -> Option<&CloseReturnOrigin> {
        match self {
            Self::Viewing(origin) | Self::Live(Some(origin)) => Some(origin),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CloseReturnOrigin {
    navigation: MainListNavigation,
    bookmark_request_id: crate::bookmark_browser::BookmarkOpenRequestId,
    bookmark_target: Option<crate::bookmark_browser::BookmarkViewReturnTarget>,
}

#[derive(Debug)]
pub(crate) struct CloseReturn {
    origin: CloseReturnOrigin,
    effect: CloseReturnEffect,
}

#[derive(Debug)]
enum CloseReturnEffect {
    BookmarkRefresh {
        target: crate::bookmark_browser::BookmarkViewReturnTarget,
        grid: BookmarkViewReturnGridState,
    },
    BookmarkNavigation {
        target: crate::bookmark_browser::BookmarkViewReturnTarget,
        grid: BookmarkViewReturnGridState,
    },
    Navigation {
        nav: crate::ui_main::AddressBarNav,
        select_after_load: Option<String>,
        media: Option<PathBuf>,
    },
}

impl App {
    /// Capture existing navigation identity without taking selection or adopting rows.
    pub(super) fn close_return_origin(&self) -> Option<CloseReturnOrigin> {
        if let Some(origin) = self.pending_return_to_parent.origin() {
            return Some(origin.clone());
        }
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return None;
        }
        Some(CloseReturnOrigin {
            navigation: MainListNavigation {
                history_before: Box::new(self.folder_nav_history_snapshot()),
                source_context: self.projected_viewer_context_id(),
                source_surface_generation: self.top_level_grid_view.generation(),
                source_items_generation: self.items_generation,
                source_slot: self.active_quick_folder_slot,
                source_slot_switch_sequence: self.quick_folder_switch_sequence,
                source_location: self.folder_nav_current_entry(),
                source_proof: self.copied_destination_source_proof(),
                history: MainHistoryOperation::Direct(DirectNavigationPurpose::Navigation),
                destination: super::MainListRestoreDestination::Native,
                selection_after_load: None,
            },
            bookmark_request_id: crate::bookmark_browser::BookmarkOpenRequestId(
                self.bookmark_open_request_seq,
            ),
            bookmark_target: self
                .bookmark_view_state
                .as_ref()
                .and_then(BookmarkViewState::target)
                .cloned(),
        })
    }

    pub(super) fn install_close_return_origin(&mut self, origin: Option<CloseReturnOrigin>) {
        self.pending_return_to_parent = origin
            .map_or(FullscreenCloseContinuation::None, |origin| {
                FullscreenCloseContinuation::Viewing(Box::new(origin))
            });
    }

    /// Admission owns invalidation, including identical-path replacement. Row refreshes
    /// and selection/scroll changes never call this boundary.
    pub(super) fn retire_close_returns_for_main_admission(&mut self) {
        let main = self.viewer_context_main();
        if self.projected_viewer_context_id() != main {
            return;
        }
        self.retire_close_return_owners(ViewerCloseCause::Replaced);
    }

    pub(super) fn retire_close_return_owners(&mut self, cause: ViewerCloseCause) {
        debug_assert_ne!(cause, ViewerCloseCause::User);
        #[cfg(windows)]
        for id in self.viewer_context_ids() {
            let _ = self.with_viewer_context_state(id, |app| app.pending_return_to_parent.clear());
        }
        #[cfg(not(windows))]
        self.pending_return_to_parent.clear();
    }

    /// Produce an owned terminal recipe before the viewer's teardown mutates its state.
    #[cfg(windows)]
    pub(crate) fn transfer_projected_terminal_close_return(&mut self, cause: ViewerCloseCause) {
        self.transfer_terminal_close_return(self.projected_viewer_context_id(), cause);
    }

    #[cfg(windows)]
    pub(super) fn transfer_terminal_close_return(
        &mut self,
        id: ViewerContextId,
        cause: ViewerCloseCause,
    ) {
        if cause != ViewerCloseCause::User {
            return;
        }
        let main = self.viewer_context_main();
        let captured = self
            .with_viewer_context_state(id, |app| {
                let origin = app.close_return_origin()?;
                let mut summary = app.with_viewer_context_ref(id, ClosedBookmarkSummary::read)?;
                // An initial media resolver can terminate before it has installed an
                // item. Its retained Detached target still belongs to this exact
                // Preparing session; there is no displayed media path to compare yet.
                // Ready viewers continue to use the actual-media comparison in read.
                if let Some(session) = app.active_detached_session
                    && session.content_phase == DetachedSessionContentPhase::Preparing
                    && app.detached_viewer_window_id() == Some(session.window_id)
                    && app
                        .locate_window_context(session.window_id)
                        .is_some_and(|(owner, _)| owner == id)
                    && summary.selected_media_path.is_none()
                    && let Some(BookmarkViewState::Detached { target }) =
                        app.bookmark_view_state.as_ref()
                {
                    summary.bookmark_target = Some(target.clone());
                }
                let standalone = id == main;
                let bookmark = app.bookmark_view_state.clone();
                let parent = if app.bookmark_view_back_nav().is_some()
                    || app.auto_open_for_current_container()
                {
                    app.resolve_close_return_navigation()
                } else {
                    None
                };
                Some((origin, summary, bookmark, parent, standalone))
            })
            .ok()
            .flatten();
        let Some((origin, summary, bookmark, parent, standalone)) = captured else {
            return;
        };
        let _ = self.with_viewer_context_state(main, |app| {
            if origin.navigation.source_context != main
                || origin.navigation.source_slot_switch_sequence != app.quick_folder_switch_sequence
            {
                return;
            }
            let effect = if let Some(BookmarkViewState::Opening { target, grid }) =
                bookmark.as_ref().filter(|_| standalone)
            {
                if origin.bookmark_request_id
                    != crate::bookmark_browser::BookmarkOpenRequestId(app.bookmark_open_request_seq)
                {
                    return;
                }
                if parent.is_some() {
                    let target = target.clone();
                    let grid = grid.clone();
                    app.bookmark_view_state = None;
                    CloseReturnEffect::BookmarkNavigation { target, grid }
                } else {
                    return;
                }
            } else if origin.bookmark_target.is_some() {
                if origin.bookmark_request_id
                    != crate::bookmark_browser::BookmarkOpenRequestId(app.bookmark_open_request_seq)
                {
                    return;
                }
                let Some(effect) = app.bookmark_close_return_effect(&summary) else {
                    return;
                };
                effect
            } else if let Some((nav, selection)) = parent {
                CloseReturnEffect::Navigation {
                    nav,
                    select_after_load: selection,
                    media: None,
                }
            } else {
                return;
            };
            app.pending_return_to_parent =
                FullscreenCloseContinuation::Terminal(Box::new(CloseReturn { origin, effect }));
        });
    }

    #[cfg(windows)]
    fn bookmark_close_return_effect(
        &mut self,
        summary: &ClosedBookmarkSummary,
    ) -> Option<CloseReturnEffect> {
        let BookmarkViewState::Opening { target, grid } = self.bookmark_view_state.as_ref()? else {
            return None;
        };
        if !self.items_are_bookmark_view {
            return None;
        }
        let target = target.clone();
        let mut grid = grid.clone();
        let effect = if matches!(
            target,
            crate::bookmark_browser::BookmarkViewReturnTarget::Book(_)
        ) || summary.bookmark_target.as_ref() == Some(&target)
        {
            grid.selected_key = self
                .selected
                .and_then(|idx| self.bookmark_browser_rows.get(idx))
                .map(crate::bookmark_browser::BookmarkBrowserRow::stable_key);
            grid.scroll_offset_y = self.grid_scroll.offset();
            CloseReturnEffect::BookmarkRefresh { target, grid }
        } else {
            CloseReturnEffect::Navigation {
                nav: crate::ui_main::AddressBarNav::Direct(
                    summary.destination.clone()?,
                    StartupListIntent::ExplicitList,
                ),
                select_after_load: None,
                media: summary.selected_media_path.clone(),
            }
        };
        self.bookmark_view_state = None;
        Some(effect)
    }

    #[cfg(all(windows, test))]
    pub(super) fn reconcile_closed_bookmark_detached_context(
        &mut self,
        summary: &ClosedBookmarkSummary,
    ) {
        let Some(origin) = self.close_return_origin() else {
            return;
        };
        let Some(effect) = self.bookmark_close_return_effect(summary) else {
            return;
        };
        self.pending_return_to_parent =
            FullscreenCloseContinuation::Terminal(Box::new(CloseReturn { origin, effect }));
        if let Some(nav) = self.take_close_return_navigation() {
            self.apply_fullscreen_close_nav_immediate(nav);
        }
    }

    /// The accepted Search row owns the physical kind; the accepted book recipe
    /// owns the currently viewed book. A filename extension cannot prove either.
    pub(super) fn everything_book_close_return_available(&self) -> bool {
        use crate::global_search_ui::{EverythingPosition, SearchAccepted};
        use book_materialization::{BookScope, ProjectionRecipe};

        if !self.global_search.active || !self.settings.effective_auto_fullscreen_zip_pdf() {
            return false;
        }
        let SearchAccepted::Everything {
            dataset,
            position: EverythingPosition::PhysicalChild { logical_source, .. },
            ..
        } = &self.global_search.accepted
        else {
            return false;
        };
        if !self
            .effective_folder()
            .is_some_and(|current| crate::folder_tree::path_eq(&current, logical_source))
        {
            return false;
        }
        let ProjectionRecipe::Book(scope) = &self.projection_recipe else {
            return false;
        };
        let root = match scope {
            BookScope::Archive { logical_root, .. } => logical_root,
            BookScope::Document(root)
            | BookScope::ImageFolder(root)
            | BookScope::CompiledFolder(root) => root,
        };
        if !crate::folder_tree::path_eq(root, logical_source) {
            return false;
        }
        let Some(key) = crate::path_key::physical_path_spelling(logical_source) else {
            return false;
        };
        let Some(row) = dataset.rows.iter().find(|row| row.physical_key == key) else {
            return false;
        };
        match &row.item {
            GridItem::ZipFile(_) | GridItem::ConvertibleArchive { .. } => {
                matches!(scope, BookScope::Archive { .. })
            }
            GridItem::PdfFile(_) => matches!(scope, BookScope::Document(_)),
            // Extension-named directories can have an Archive/Document recipe
            // (known classification issue 1.418). Their accepted Folder kind
            // still requires the opt-in image-folder book policy.
            GridItem::Folder(_) => {
                self.settings.auto_fullscreen_image_folders_enabled()
                    && self.items_are_image_only_folder_pages()
            }
            _ => false,
        }
    }

    /// Resolve selection intent as a value, without loading/adopting a list.
    pub(super) fn resolve_close_return_navigation(
        &self,
    ) -> Option<(crate::ui_main::AddressBarNav, Option<String>)> {
        if self.everything_book_close_return_available() {
            // The accepted Search owner holds the logical source and full-path anchor,
            // including converted books. Never infer this return from a cache's parent.
            return Some((crate::ui_main::AddressBarNav::EverythingResults, None));
        }
        if let Some(navigation) = self.collection_grid_parent_navigation() {
            return Some(navigation);
        }
        if let Some(nav) = self
            .bookmark_view_back_nav()
            .or_else(|| self.reading_history_back_nav())
            .or_else(|| {
                self.rating_view_parent_nav_available()
                    .then_some(crate::ui_main::AddressBarNav::RatingViewBack)
            })
            .or_else(|| self.subfolder_expansion_back_nav())
            .or_else(|| self.smart_folder_parent_nav_target())
        {
            return Some((nav, None));
        }
        if self.items_are_smart_folder_view {
            return None;
        }
        let current = self.effective_folder()?;
        Some((
            crate::ui_main::AddressBarNav::Direct(
                current.parent()?.to_path_buf(),
                StartupListIntent::ExplicitList,
            ),
            current
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned),
        ))
    }

    pub(super) fn take_close_return_navigation(&mut self) -> Option<crate::ui_main::AddressBarNav> {
        use FullscreenCloseContinuation as State;
        match std::mem::take(&mut self.pending_return_to_parent) {
            State::None => None,
            source @ State::Viewing(_) => {
                self.pending_return_to_parent = source;
                None
            }
            State::Live(_) => {
                if let Some((nav, selection)) = self.resolve_close_return_navigation() {
                    if selection.is_some() {
                        self.select_after_load = selection;
                    }
                    Some(nav)
                } else {
                    self.change_main_context_for_visible_grid(false);
                    None
                }
            }
            State::Terminal(request) => {
                debug_assert_eq!(
                    self.projected_viewer_context_id(),
                    request.origin.navigation.source_context
                );
                match request.effect {
                    CloseReturnEffect::BookmarkRefresh { target, mut grid } => {
                        // A user can keep selecting/scrolling while logic-only returns wait.
                        // A completed read-model may also have replaced the live main rows
                        // before this consumer. Restore that adopted grid, not its predecessor.
                        grid.row_keys = self
                            .bookmark_browser_rows
                            .iter()
                            .map(crate::bookmark_browser::BookmarkBrowserRow::stable_key)
                            .collect();
                        grid.selected_key = self
                            .selected
                            .and_then(|idx| self.bookmark_browser_rows.get(idx))
                            .map(crate::bookmark_browser::BookmarkBrowserRow::stable_key);
                        grid.scroll_offset_y = self.grid_scroll.offset();
                        let _ = target;
                        self.bookmark_view_state = Some(BookmarkViewState::Restoring { grid });
                        self.refresh_bookmark_browser();
                        None
                    }
                    CloseReturnEffect::BookmarkNavigation { target, grid } => {
                        self.bookmark_view_state =
                            Some(BookmarkViewState::Opening { target, grid });
                        Some(crate::ui_main::AddressBarNav::Bookmarks)
                    }
                    CloseReturnEffect::Navigation {
                        nav,
                        select_after_load,
                        media,
                    } => {
                        if let Some(path) = media {
                            self.select_after_load = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .map(str::to_owned);
                        } else if select_after_load.is_some() {
                            self.select_after_load = select_after_load;
                        }
                        Some(nav)
                    }
                }
            }
        }
    }
}
