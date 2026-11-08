//! The existing navigation return slot outlives a terminal viewer.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerCloseCause {
    User,
    Replaced,
    RemoteAcquire,
    AppExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenCloseDestination {
    CurrentList,
    ChosenZipList,
}

#[derive(Debug, Default)]
pub(crate) enum FullscreenCloseContinuation {
    #[default]
    None,
    Live,
    Terminal(Box<CloseReturn>),
}

impl FullscreenCloseContinuation {
    pub(super) fn terminal_destination(&self) -> FullscreenCloseDestination {
        match self {
            Self::Terminal(request)
                if matches!(
                    &request.effect,
                    CloseReturnEffect::Navigation {
                        nav: crate::ui_main::AddressBarNav::ZipListReturn(_),
                        ..
                    }
                ) =>
            {
                FullscreenCloseDestination::ChosenZipList
            }
            _ => FullscreenCloseDestination::CurrentList,
        }
    }
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self, Self::Live | Self::Terminal(_))
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::None;
    }
    pub(crate) fn request_live(&mut self) {
        if !matches!(self, Self::Terminal(_)) {
            *self = Self::Live;
        }
    }
    pub(super) fn cancel_live_request(&mut self) {
        if matches!(self, Self::Live) {
            self.clear();
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CloseReturnOrigin {
    navigation: MainListNavigation,
}

#[derive(Debug)]
pub(crate) struct CloseReturn {
    origin: CloseReturnOrigin,
    effect: CloseReturnEffect,
}

#[derive(Debug)]
enum CloseReturnEffect {
    BookmarkNavigation {
        target: crate::bookmark_browser::BookmarkViewReturnTarget,
        grid: BookmarkViewReturnGridState,
    },
    Navigation {
        nav: crate::ui_main::AddressBarNav,
        select_after_load: Option<String>,
    },
}

impl App {
    /// Capture existing navigation identity without taking selection or adopting rows.
    pub(super) fn close_return_origin(&self) -> Option<CloseReturnOrigin> {
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
        })
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
        // Independent bundles have no main return authority. The shared viewport
        // close entry still captures main/linked returns before their teardown.
        if id != self.viewer_context_main() {
            return;
        }
        let _ = self.with_viewer_context_state(id, |app| {
            let Some(origin) = app.close_return_origin() else {
                return;
            };
            let parent = if app.chosen_zip_list_return().is_some()
                || app.bookmark_view_back_nav().is_some()
                || app.auto_open_for_current_container()
            {
                app.resolve_close_return_navigation()
            } else {
                None
            };
            let Some((nav, selection)) = parent else {
                return;
            };
            let effect = if matches!(&nav, crate::ui_main::AddressBarNav::ZipListReturn(_)) {
                CloseReturnEffect::Navigation {
                    nav,
                    select_after_load: selection,
                }
            } else if let Some(BookmarkViewState::Opening { target, grid }) =
                app.bookmark_view_state.as_ref()
            {
                let target = target.clone();
                let grid = grid.clone();
                app.bookmark_view_state = None;
                CloseReturnEffect::BookmarkNavigation { target, grid }
            } else {
                CloseReturnEffect::Navigation {
                    nav,
                    select_after_load: selection,
                }
            };
            app.pending_return_to_parent =
                FullscreenCloseContinuation::Terminal(Box::new(CloseReturn { origin, effect }));
        });
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
        if let Some(plan) = self.chosen_zip_list_return() {
            return Some((crate::ui_main::AddressBarNav::ZipListReturn(plan), None));
        }
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
            State::Live => {
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
                    CloseReturnEffect::BookmarkNavigation { target, grid } => {
                        self.bookmark_view_state =
                            Some(BookmarkViewState::Opening { target, grid });
                        Some(crate::ui_main::AddressBarNav::Bookmarks)
                    }
                    CloseReturnEffect::Navigation {
                        nav,
                        select_after_load,
                    } => {
                        if select_after_load.is_some() {
                            self.select_after_load = select_after_load;
                        }
                        Some(nav)
                    }
                }
            }
        }
    }
}
