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
    LiveZipListReturn(crate::zip_tree::ZipListReturnPlan),
    Terminal(Box<CloseReturn>),
    IssuedZipListReturn(archive_inner_book::ZipListReturnOwner),
}

impl FullscreenCloseContinuation {
    pub(super) fn is_live(&self) -> bool {
        matches!(self, Self::Live | Self::LiveZipListReturn(_))
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.is_live() || matches!(self, Self::Terminal(_))
    }
    pub(super) fn has_accepted_request(&self) -> bool {
        self.is_pending()
            || matches!(self, Self::IssuedZipListReturn(owner) if owner.has_outstanding_tickets())
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::None;
    }
    pub(crate) fn request_live(&mut self) {
        if !self.has_accepted_request() {
            *self = Self::Live;
        }
    }
    pub(super) fn cancel_live_request(&mut self) {
        if self.is_live() {
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
    /// The accepted None -> Some reader transition ends only this context's
    /// Zip return. Other navigation variants keep their existing contract.
    pub(super) fn retire_zip_close_return_for_reader_acceptance(&mut self) {
        let is_zip_return = match &self.pending_return_to_parent {
            FullscreenCloseContinuation::LiveZipListReturn(_)
            | FullscreenCloseContinuation::IssuedZipListReturn(_) => true,
            FullscreenCloseContinuation::Terminal(request) => matches!(
                request.effect,
                CloseReturnEffect::Navigation {
                    nav: crate::ui_main::AddressBarNav::ZipListReturn(_),
                    ..
                }
            ),
            _ => false,
        };
        if is_zip_return {
            self.pending_return_to_parent.clear();
            crate::logger::log(
                "archive list return retired: new reader accepted in owning context",
            );
        }
    }

    pub(super) fn consume_issued_zip_list_return(
        &mut self,
        ticket: &archive_inner_book::ZipListReturnTicket,
    ) -> Option<crate::zip_tree::ZipListReturnPlan> {
        if matches!(&self.pending_return_to_parent,
            FullscreenCloseContinuation::IssuedZipListReturn(owner) if owner.accepts(ticket))
        {
            let FullscreenCloseContinuation::IssuedZipListReturn(owner) =
                std::mem::take(&mut self.pending_return_to_parent)
            else {
                unreachable!()
            };
            Some(owner.into_plan())
        } else {
            None
        }
    }

    fn issue_zip_list_return_navigation(
        &mut self,
        nav: crate::ui_main::AddressBarNav,
    ) -> crate::ui_main::AddressBarNav {
        let crate::ui_main::AddressBarNav::ZipListReturn(request) = nav else {
            return nav;
        };
        let (owner, request) = match request {
            ZipListReturnRequest::PreparedLive(plan) => {
                let owner = archive_inner_book::ZipListReturnOwner::new(plan);
                let request = ZipListReturnRequest::Live {
                    ticket: owner.ticket(),
                };
                (owner, request)
            }
            ZipListReturnRequest::PreparedTerminal(plan) => {
                let owner = archive_inner_book::ZipListReturnOwner::new(plan);
                let request = ZipListReturnRequest::Terminal {
                    ticket: owner.ticket(),
                };
                (owner, request)
            }
            // Re-delivery must never renew an already-issued ticket.
            issued
            @ (ZipListReturnRequest::Live { .. } | ZipListReturnRequest::Terminal { .. }) => {
                return crate::ui_main::AddressBarNav::ZipListReturn(issued);
            }
        };
        self.pending_return_to_parent = FullscreenCloseContinuation::IssuedZipListReturn(owner);
        crate::ui_main::AddressBarNav::ZipListReturn(request)
    }

    /// A captured Terminal recipe still has to be current when its reader is
    /// torn down. Later delivery cannot repair a stale bookmark-preserve policy.
    pub(super) fn fullscreen_close_destination(&self) -> FullscreenCloseDestination {
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return FullscreenCloseDestination::CurrentList;
        }
        let plan = match &self.pending_return_to_parent {
            FullscreenCloseContinuation::Terminal(request) => match &request.effect {
                CloseReturnEffect::Navigation {
                    nav: crate::ui_main::AddressBarNav::ZipListReturn(request),
                    ..
                } => request.prepared_plan(),
                _ => None,
            },
            FullscreenCloseContinuation::IssuedZipListReturn(owner)
                if owner.has_outstanding_tickets() =>
            {
                Some(owner.plan())
            }
            _ => None,
        };
        if plan.is_some_and(|plan| self.zip_list_return_stale_reason(plan).is_none()) {
            FullscreenCloseDestination::ChosenZipList
        } else {
            FullscreenCloseDestination::CurrentList
        }
    }

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
            if matches!(app.pending_return_to_parent, FullscreenCloseContinuation::Terminal(_))
                || matches!(&app.pending_return_to_parent,
                    FullscreenCloseContinuation::IssuedZipListReturn(owner) if owner.has_outstanding_tickets()) {
                return;
            }
            let parent = if matches!(
                app.pending_return_to_parent,
                FullscreenCloseContinuation::LiveZipListReturn(_)
            ) {
                let FullscreenCloseContinuation::LiveZipListReturn(plan) =
                    std::mem::take(&mut app.pending_return_to_parent)
                else {
                    unreachable!()
                };
                Some((
                    crate::ui_main::AddressBarNav::ZipListReturn(
                        ZipListReturnRequest::PreparedTerminal(plan),
                    ),
                    None,
                ))
            } else if app.chosen_zip_list_return().is_some()
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
            let effect = if let crate::ui_main::AddressBarNav::ZipListReturn(request) = nav {
                CloseReturnEffect::Navigation {
                    nav: crate::ui_main::AddressBarNav::ZipListReturn(request.into_terminal()),
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
            return Some((
                crate::ui_main::AddressBarNav::ZipListReturn(ZipListReturnRequest::PreparedLive(
                    plan,
                )),
                None,
            ));
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
        // Input and post-render dispatch may both poll in one frame. A taken
        // return keeps its authority until execution or a new reader/admission.
        if matches!(self.pending_return_to_parent, State::IssuedZipListReturn(_)) {
            return None;
        }
        let nav = match std::mem::take(&mut self.pending_return_to_parent) {
            State::None => None,
            State::LiveZipListReturn(plan) => Some(crate::ui_main::AddressBarNav::ZipListReturn(
                ZipListReturnRequest::PreparedLive(plan),
            )),
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
            State::IssuedZipListReturn(owner) => {
                self.pending_return_to_parent = State::IssuedZipListReturn(owner);
                None
            }
        }?;
        Some(self.issue_zip_list_return_navigation(nav))
    }
}
