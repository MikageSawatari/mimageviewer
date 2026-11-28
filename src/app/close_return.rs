//! The existing navigation return slot outlives a terminal viewer.
use super::*;

#[cfg(all(test, windows))]
mod focus_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerCloseCause {
    User,
    Replaced,
    RemoteAcquire,
    AppExit,
    BrowserReplacement,
    DetachedToggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenCloseDestination {
    CurrentList,
    ChosenZipList,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenCloseRequestOutcome {
    ClosedNow,
    ReturnAccepted,
    DeferredTerminalClose,
    Ignored,
}

/// One accepted close owns both its continuation and its entry's optional Focus.
/// Dispatch consumes the latter in the same frame; a C5 wait never retains it.
#[derive(Debug, Default)]
pub(crate) struct FullscreenCloseContinuation {
    pub(crate) state: FullscreenCloseState,
    entry_focus: Option<egui::ViewportId>,
}

/// Consumed around one dispatch call. Never stored in App or a worker.
pub(super) struct FullscreenCloseDispatchFocus {
    target: egui::ViewportId,
    legacy_owners_before: LegacyCloseReturnOwners,
}

/// Identities of already accepted non-C5 workers, sampled only around dispatch.
/// A refused dispatch cannot borrow an unchanged worker's entry-frame Focus.
#[derive(Default)]
struct LegacyCloseReturnOwners {
    smart: Option<u64>,
    rating: Option<u64>,
    history: Option<u64>,
    classification: Option<u64>,
    subfolder: Vec<Arc<AtomicBool>>,
}

impl LegacyCloseReturnOwners {
    fn accepted_since(self, before: Self) -> bool {
        (self.smart.is_some() && self.smart != before.smart)
            || (self.rating.is_some() && self.rating != before.rating)
            || (self.history.is_some() && self.history != before.history)
            || (self.classification.is_some() && self.classification != before.classification)
            || self
                .subfolder
                .iter()
                .any(|owner| before.subfolder.iter().all(|old| !Arc::ptr_eq(owner, old)))
    }
}

#[derive(Debug, Default)]
pub(crate) enum FullscreenCloseState {
    #[default]
    None,
    Live,
    /// Rendering resources captured before true-close cleanup. No authority from the
    /// previous navigation sequence is transferred to this accepted close.
    LiveWithPresentation(crate::ui_fullscreen::FsCloseReturnPresentation),
    /// The navigation merge point has taken the return; its physical navigation
    /// captures these resources before admission retires the return slot.
    IssuedLive(crate::ui_fullscreen::FsCloseReturnPresentation),
    LiveZipListReturn(crate::zip_tree::ZipListReturnPlan),
    Terminal(Box<CloseReturn>),
    IssuedZipListReturn(archive_inner_book::ZipListReturnOwner),
}

impl FullscreenCloseContinuation {
    pub(super) fn is_live(&self) -> bool {
        matches!(
            self.state,
            FullscreenCloseState::Live
                | FullscreenCloseState::LiveWithPresentation(_)
                | FullscreenCloseState::LiveZipListReturn(_)
        )
    }
    pub(crate) fn is_pending(&self) -> bool {
        self.is_live() || matches!(self.state, FullscreenCloseState::Terminal(_))
    }
    pub(super) fn has_accepted_request(&self) -> bool {
        self.is_pending()
            || matches!(&self.state, FullscreenCloseState::IssuedZipListReturn(owner) if owner.has_outstanding_tickets())
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn request_live(&mut self) {
        if !self.has_accepted_request() {
            self.state = FullscreenCloseState::Live;
        }
    }
    pub(crate) fn request_live_with_presentation(
        &mut self,
        presentation: Option<crate::ui_fullscreen::FsCloseReturnPresentation>,
    ) {
        if !self.has_accepted_request() {
            self.state = presentation.map_or(
                FullscreenCloseState::Live,
                FullscreenCloseState::LiveWithPresentation,
            );
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
    /// Preserve the entry's existing target for the same-frame dispatch decision.
    /// This is an attribute of the accepted close, never a new App pending slot.
    pub(crate) fn route_fullscreen_close_entry_focus(
        &mut self,
        ctx: &egui::Context,
        outcome: FullscreenCloseRequestOutcome,
        sends_focus: bool,
        source: &'static str,
    ) {
        if !sends_focus {
            return;
        }
        match outcome {
            FullscreenCloseRequestOutcome::ClosedNow => {
                self.send_fullscreen_close_focus(ctx, ctx.viewport_id(), source);
            }
            FullscreenCloseRequestOutcome::ReturnAccepted => {
                self.pending_return_to_parent.entry_focus = Some(ctx.viewport_id());
            }
            FullscreenCloseRequestOutcome::DeferredTerminalClose
            | FullscreenCloseRequestOutcome::Ignored => {}
        }
    }

    pub(crate) fn take_pending_return_to_parent_nav_with_focus(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<crate::ui_main::AddressBarNav> {
        if !self.pending_return_to_parent.is_pending() {
            // A second seam poll must not consume an already-issued dispatch's Focus.
            return self.take_pending_return_to_parent_nav();
        }
        let target = self.pending_return_to_parent.entry_focus.take();
        let nav = self.take_pending_return_to_parent_nav();
        if nav.is_some() {
            // Admission can clear the continuation. The actual dispatcher takes
            // this attribute into a call-local owner before applying navigation.
            self.pending_return_to_parent.entry_focus = target;
        } else {
            // RetireNoDestination already applied its content close in take.
            self.finish_fullscreen_close_dispatch_focus(
                ctx,
                target.map(|target| FullscreenCloseDispatchFocus {
                    target,
                    legacy_owners_before: self.legacy_close_return_owners(),
                }),
            );
        }
        nav
    }

    pub(super) fn take_fullscreen_close_dispatch_focus(
        &mut self,
    ) -> Option<FullscreenCloseDispatchFocus> {
        let target = self.pending_return_to_parent.entry_focus.take()?;
        Some(FullscreenCloseDispatchFocus {
            target,
            legacy_owners_before: self.legacy_close_return_owners(),
        })
    }

    fn legacy_close_return_owners(&self) -> LegacyCloseReturnOwners {
        let current =
            |navigation: &MainListNavigation| self.main_list_navigation_is_current(navigation);
        LegacyCloseReturnOwners {
            smart: self.staged_smart_navigation_focus_owner(),
            rating: self
                .rating_view_pending
                .as_ref()
                .filter(|pending| {
                    !pending.cancel.load(Ordering::Acquire)
                        && pending.sequence == self.rating_view_request_sequence
                        && pending
                            .navigation
                            .as_ref()
                            .is_some_and(|nav| current(&nav.navigation))
                })
                .map(|pending| pending.sequence),
            history: self
                .top_level_grid_view
                .history_navigation_transition()
                .and_then(|owner| {
                    let (id, navigation) = match owner {
                        HistoryNavigationTransition::Physical(owner) => {
                            (owner.request_id, &owner.navigation)
                        }
                        HistoryNavigationTransition::Collection(owner) => {
                            (owner.request_id, &owner.navigation)
                        }
                    };
                    current(navigation).then_some(id)
                }),
            // Directory classifiers carry their completed scan into adoption;
            // they are legacy async returns, rather than a new C5 scan wait.
            classification: self
                .top_level_grid_view
                .open_path_classification()
                .filter(|owner| {
                    matches!(
                        owner.continuation.as_deref(),
                        Some(
                            ClassifiedOpenContinuation::Physical { .. }
                                | ClassifiedOpenContinuation::Collection { .. }
                                | ClassifiedOpenContinuation::Direct { .. }
                                | ClassifiedOpenContinuation::DirectNavigation { .. }
                                | ClassifiedOpenContinuation::DirectScan { .. }
                        )
                    ) && !owner.cancel.load(Ordering::Acquire)
                        && owner.navigation.as_ref().is_some_and(current)
                })
                .map(|owner| owner.request_id),
            subfolder: self.subfolder_close_return_focus_owners(),
        }
    }

    pub(super) fn finish_fullscreen_close_dispatch_focus(
        &mut self,
        ctx: &egui::Context,
        focus: Option<FullscreenCloseDispatchFocus>,
    ) {
        let Some(focus) = focus else {
            return;
        };
        if self.fs_close_return_navigation_wait_active() {
            // Only an actually started C5 wait hands authority to adoption.
            return;
        }
        let accepted_legacy_return = self
            .legacy_close_return_owners()
            .accepted_since(focus.legacy_owners_before);
        // Pre-C5 synthetic/history workers retain entry-frame Focus.
        // An open viewer after refusal/failure, with no new owner, never sends it.
        if !self.viewer_session_blocks_main_window() || accepted_legacy_return {
            self.send_fullscreen_close_focus(ctx, focus.target, "app::close_return_dispatch_focus");
        }
    }

    pub(crate) fn apply_fullscreen_close_nav_immediate_with_focus(
        &mut self,
        ctx: &egui::Context,
        nav: crate::ui_main::AddressBarNav,
    ) -> bool {
        let focus = self.take_fullscreen_close_dispatch_focus();
        let applied = self.apply_fullscreen_close_nav_immediate(nav);
        self.finish_fullscreen_close_dispatch_focus(ctx, focus);
        applied
    }

    fn send_fullscreen_close_focus(
        &self,
        ctx: &egui::Context,
        target: egui::ViewportId,
        source: &'static str,
    ) {
        ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
        #[cfg(windows)]
        self.observe_viewport_presentation_command(
            target,
            crate::presentation_observer::WindowAction::Focus,
            source,
            "viewport_command=Focus",
        );
        #[cfg(not(windows))]
        let _ = source;
    }

    /// Capture the closing surface before install changes presentation/viewport identity.
    /// Native presenter/HUD close belongs to its existing effect and HWND reclaim.
    #[cfg(windows)]
    fn main_close_return_focus_surface(&self, ctx: &egui::Context) -> Option<u64> {
        if !self.fullscreen_close_return_is_main_context()
            || self.current_viewer_session_is_detached_or_switching()
            || self.viewer_presentation != ViewerPresentation::Fullscreen
            || self.fullscreen_embedded_still_active()
            || self.native_video_fullscreen_active_for_main_backdrop()
            || self.fullscreen_idx.is_none()
        {
            return None;
        }
        let viewport = self.fullscreen_viewport_id();
        if viewport == egui::ViewportId::ROOT {
            return None;
        }
        let witness = eframe::native_window_input::native_window_allocation_witness(ctx, viewport)?;
        witness.is_alive().then_some(())?;
        witness
            .allocation()
            .windows_hwnd
            .filter(|hwnd| *hwnd != 0)
            .map(|hwnd| hwnd as u64)
    }

    /// A one-call result, not retained state. Query foreground immediately before
    /// sending, after the confirmed C5 install has closed the captured surface.
    #[cfg(windows)]
    pub(super) fn resume_direct_navigation_scan_with_focus(
        &mut self,
        ctx: &egui::Context,
        call: DirectNavigationContinuation,
        scan: ScannedDir,
    ) {
        let surface = self.main_close_return_focus_surface(ctx);
        let adopted_close = self.resume_direct_navigation_scan(call, scan);
        if adopted_close
            && let Some(surface) = surface
            && current_close_return_foreground_hwnd() == surface
        {
            self.send_fullscreen_close_focus(
                ctx,
                egui::ViewportId::ROOT,
                "app::close_return_adoption_focus",
            );
        }
    }

    /// Transfer only the issued close's immutable rendering payload into the normal
    /// physical navigation owner. Unconsumed payloads retire with the return slot.
    pub(super) fn take_fullscreen_close_return_presentation(
        &mut self,
    ) -> Option<crate::ui_fullscreen::FsCloseReturnPresentation> {
        if !matches!(
            self.pending_return_to_parent.state,
            FullscreenCloseState::IssuedLive(_)
        ) {
            return None;
        }
        let FullscreenCloseState::IssuedLive(presentation) =
            std::mem::take(&mut self.pending_return_to_parent.state)
        else {
            unreachable!()
        };
        Some(presentation)
    }

    /// The accepted None -> Some reader transition ends only this context's
    /// Zip return. Other navigation variants keep their existing contract.
    pub(super) fn retire_zip_close_return_for_reader_acceptance(&mut self) {
        let is_zip_return = match &self.pending_return_to_parent.state {
            FullscreenCloseState::LiveZipListReturn(_)
            | FullscreenCloseState::IssuedZipListReturn(_) => true,
            FullscreenCloseState::Terminal(request) => matches!(
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
        if matches!(&self.pending_return_to_parent.state,
            FullscreenCloseState::IssuedZipListReturn(owner) if owner.accepts(ticket))
        {
            let FullscreenCloseState::IssuedZipListReturn(owner) =
                std::mem::take(&mut self.pending_return_to_parent.state)
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
        self.pending_return_to_parent.state = FullscreenCloseState::IssuedZipListReturn(owner);
        crate::ui_main::AddressBarNav::ZipListReturn(request)
    }

    /// Rendering needs the close-return ownership predicate, not registry identities.
    /// Keep the same main-context check used by the close-return owner here.
    pub(crate) fn fullscreen_close_return_is_main_context(&self) -> bool {
        self.projected_viewer_context_id() == self.viewer_context_main()
    }

    /// A captured Terminal recipe still has to be current when its reader is
    /// torn down. Later delivery cannot repair a stale bookmark-preserve policy.
    pub(super) fn fullscreen_close_destination(&self) -> FullscreenCloseDestination {
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return FullscreenCloseDestination::CurrentList;
        }
        let plan = match &self.pending_return_to_parent.state {
            FullscreenCloseState::Terminal(request) => match &request.effect {
                CloseReturnEffect::Navigation {
                    nav: crate::ui_main::AddressBarNav::ZipListReturn(request),
                    ..
                } => request.prepared_plan(),
                _ => None,
            },
            FullscreenCloseState::IssuedZipListReturn(owner) if owner.has_outstanding_tickets() => {
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
                #[cfg(all(windows, feature = "test-script"))]
                folder_load_recorded: false,
                viewer_adoption: super::ViewerOpenAdoption::ContextContinuation,
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
                payload: None,
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
            if matches!(app.pending_return_to_parent.state, FullscreenCloseState::Terminal(_))
                || matches!(&app.pending_return_to_parent.state,
                    FullscreenCloseState::IssuedZipListReturn(owner) if owner.has_outstanding_tickets()) {
                return;
            }
            let parent = if matches!(
                app.pending_return_to_parent.state,
                FullscreenCloseState::LiveZipListReturn(_)
            ) {
                let FullscreenCloseState::LiveZipListReturn(plan) =
                    std::mem::take(&mut app.pending_return_to_parent.state)
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
            app.pending_return_to_parent.state =
                FullscreenCloseState::Terminal(Box::new(CloseReturn { origin, effect }));
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
        let _span = CloseNavSpan::new(CloseNavStage::ResolveReturn);
        if let Some(plan) = {
            let _span = CloseNavSpan::new(CloseNavStage::ResolveZipReturn);
            self.chosen_zip_list_return()
        } {
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
        if let Some(navigation) = {
            let _span = CloseNavSpan::new(CloseNavStage::ResolveCollectionReturn);
            self.collection_grid_parent_navigation()
        } {
            return Some(navigation);
        }
        let special = {
            let _span = CloseNavSpan::new(CloseNavStage::ResolveSpecialReturn);
            self.bookmark_view_back_nav()
                .or_else(|| self.reading_history_back_nav())
                .or_else(|| {
                    self.rating_view_parent_nav_available()
                        .then_some(crate::ui_main::AddressBarNav::RatingViewBack)
                })
                .or_else(|| self.subfolder_expansion_back_nav())
                .or_else(|| self.smart_folder_parent_nav_target())
        };
        if let Some(nav) = special {
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
        use FullscreenCloseState as State;
        // Input and post-render dispatch may both poll in one frame. A taken
        // return keeps its authority until execution or a new reader/admission.
        if matches!(
            self.pending_return_to_parent.state,
            State::IssuedZipListReturn(_) | State::IssuedLive(_)
        ) {
            return None;
        }
        let nav = match std::mem::take(&mut self.pending_return_to_parent.state) {
            State::None => None,
            State::LiveZipListReturn(plan) => Some(crate::ui_main::AddressBarNav::ZipListReturn(
                ZipListReturnRequest::PreparedLive(plan),
            )),
            state @ (State::Live | State::LiveWithPresentation(_)) => {
                if let Some((nav, selection)) = self.resolve_close_return_navigation() {
                    if selection.is_some() {
                        self.select_after_load =
                            selection.map(crate::app::AfterLoadSelection::Name);
                    }
                    if matches!(
                        &nav,
                        crate::ui_main::AddressBarNav::Direct(..)
                            | crate::ui_main::AddressBarNav::CollectionSource { .. }
                    ) && let State::LiveWithPresentation(presentation) = state
                    {
                        self.pending_return_to_parent.state = State::IssuedLive(presentation);
                    }
                    Some(nav)
                } else {
                    let _span = CloseNavSpan::new(CloseNavStage::RetireNoDestination);
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
                            self.select_after_load =
                                select_after_load.map(crate::app::AfterLoadSelection::Name);
                        }
                        Some(nav)
                    }
                }
            }
            State::IssuedZipListReturn(owner) => {
                self.pending_return_to_parent.state = State::IssuedZipListReturn(owner);
                None
            }
            State::IssuedLive(presentation) => {
                self.pending_return_to_parent.state = State::IssuedLive(presentation);
                None
            }
        }?;
        Some(self.issue_zip_list_return_navigation(nav))
    }
}

#[cfg(all(windows, not(test)))]
fn current_close_return_foreground_hwnd() -> u64 {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe { GetForegroundWindow().0 as usize as u64 }
}

#[cfg(all(windows, test))]
fn current_close_return_foreground_hwnd() -> u64 {
    crate::test_native_focus::foreground_hwnd()
}
