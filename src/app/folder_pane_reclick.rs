//! Admission for the tree's same-location refresh. No request owner lives here:
//! physical preflight owns the list and FolderPaneState owns tree reconciliation.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReclickNavigationOwner {
    PhysicalOrCollection,
    Classification,
    Pane,
    PaneReady,
    FolderTraversal,
    Rating,
    SmartFolder,
    Collection,
    Subfolder,
    Bookmark,
    Startup,
    Archive,
    Document,
    SavedGroup,
    CloseReturn,
    Tag,
    BookOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FolderPaneReclickRejection {
    OtherContext,
    NotPhysicalListing,
    DifferentFolder,
    ConvertedOrExtracted,
    ReaderOpen,
    FlatProjection,
    StackPreparation,
    EpubBatch,
    Modal,
    PendingNavigation(ReclickNavigationOwner),
    HigherPriorityNavigation,
    GridNavigation,
    RequestUnavailable,
    WorkerAdmissionFailed,
}

fn ordinary_refresh(intent: &StartupListIntent) -> bool {
    matches!(
        intent,
        StartupListIntent::RefreshList {
            automatic_observation: false,
            book_append_refresh: None,
            ..
        }
    )
}

fn ordinary_navigation(intent: &PhysicalHistoryIntent) -> bool {
    // §14.7: an idle RatingPhysical list may start its own Rating refresh, but
    // that owner must finish before another reclick. Only Navigation is replaceable.
    matches!(
        intent,
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false,
        }
    )
}

fn same_location(navigation: Option<&MainListNavigation>) -> bool {
    navigation.is_some_and(|nav| matches!(nav.history, MainHistoryOperation::SameLocation))
}

fn replaceable_order_refresh(
    path: &Path,
    request_path: &Path,
    purpose: &FolderOpenScanPurpose,
    navigation: Option<&MainListNavigation>,
) -> bool {
    crate::folder_tree::path_eq(path, request_path)
        && matches!(purpose, FolderOpenScanPurpose::CurrentViewOrderRefresh { reload_owner, .. }
            if matches!(reload_owner.as_ref(), OpenRequestOwner::Navigation))
        && same_location(navigation)
}

fn classification_is_refresh(request: &OpenPathClassification, path: &Path) -> bool {
    if !crate::folder_tree::path_eq(request.path.as_path(), path)
        || !same_location(request.navigation.as_ref())
    {
        return false;
    }
    match request.continuation.as_deref() {
        Some(ClassifiedOpenContinuation::Physical {
            intent,
            dfs_continuation,
            restore_intent,
            ..
        }) => {
            ordinary_navigation(intent)
                && dfs_continuation.is_none()
                && ordinary_refresh(restore_intent)
        }
        Some(ClassifiedOpenContinuation::Direct {
            auto_fullscreen: false,
            owner: OpenRequestOwner::Navigation,
            restore_intent,
        })
        | Some(ClassifiedOpenContinuation::DirectNavigation {
            owner: OpenRequestOwner::Navigation,
            restore_intent,
            ..
        })
        | Some(ClassifiedOpenContinuation::DirectScan {
            owner: OpenRequestOwner::Navigation,
            restore_intent,
            ..
        }) => ordinary_refresh(restore_intent),
        _ => false,
    }
}

impl App {
    /// Read-only, both before polling pane navigation and against its local ready.
    /// A failed/retired B request still wins over a reclick admitted in that pass.
    /// Passive observations are not navigation; their existing owner/generation
    /// checks govern adoption. Only Navigation refresh forms may be replaced (§14.7).
    pub(super) fn folder_pane_reclick_admission(
        &self,
        path: &Path,
        ready: Option<&FolderPaneOpenReady>,
    ) -> Result<(), FolderPaneReclickRejection> {
        use FolderPaneReclickRejection as Reject;
        use ReclickNavigationOwner as Owner;
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return Err(Reject::OtherContext);
        }
        if !self.is_physical_folder_listing() {
            return Err(Reject::NotPhysicalListing);
        }
        if !self
            .current_folder
            .as_ref()
            .is_some_and(|folder| crate::folder_tree::path_eq(folder, path))
        {
            return Err(Reject::DifferentFolder);
        }
        if self.archive_source_override.is_some() || self.extracted_result_surface().is_some() {
            return Err(Reject::ConvertedOrExtracted);
        }
        if self.projected_viewer_session_open_for(SourceSwapOwnership::MountedContext)
            || self.pending_auto_fs_open
            || self.fs_nav_after_pdf_enumerate.is_some()
            || self.archive_convert_deferred_fullscreen_active()
        {
            return Err(Reject::ReaderOpen);
        }
        if self.stack_showing_flat {
            return Err(Reject::FlatProjection);
        }
        // Includes Grouping/Extracting and switching BACK to the aggregate.
        // open_pending() covers only the later fullscreen open of a flat reader.
        if self.stack_script_pending.is_some() {
            return Err(Reject::StackPreparation);
        }
        if self.epub_batch_convert.is_some() {
            return Err(Reject::EpubBatch);
        }
        if self.main_chrome_pointer_blocked() {
            return Err(Reject::Modal);
        }
        let pending = |owner| Err(Reject::PendingNavigation(owner));
        if let Some(transition) = self.top_level_grid_view.history_navigation_transition() {
            let refresh = matches!(transition, HistoryNavigationTransition::Physical(request)
                if crate::folder_tree::path_eq(&request.path, path)
                    && same_location(Some(&request.navigation))
                    && ordinary_refresh(&request.restore_intent)
                    && ordinary_navigation(&request.intent)
                    && request.dfs_continuation.is_none());
            if !refresh {
                return pending(Owner::PhysicalOrCollection);
            }
        }
        if let Some(request) = self.top_level_grid_view.open_path_classification()
            && request.context == self.viewer_context_main()
            && !classification_is_refresh(request, path)
        {
            return pending(Owner::Classification);
        }
        if let Some(request) = self.folder_pane_open_pending.as_ref()
            && !matches!(
                request.purpose,
                FolderOpenScanPurpose::DetachedFolder | FolderOpenScanPurpose::DetachedImage { .. }
            )
            && !replaceable_order_refresh(
                path,
                &request.path,
                &request.purpose,
                request.navigation.as_ref(),
            )
        {
            return pending(Owner::Pane);
        }
        if let Some(ready) = ready
            && !matches!(
                ready.purpose,
                FolderOpenScanPurpose::DetachedFolder | FolderOpenScanPurpose::DetachedImage { .. }
            )
            && !replaceable_order_refresh(
                path,
                &ready.path,
                &ready.purpose,
                ready.navigation.as_ref(),
            )
        {
            return pending(Owner::PaneReady);
        }
        if self.folder_nav_pending.is_some() {
            return pending(Owner::FolderTraversal);
        }
        if self.rating_view_pending.is_some() || self.pending_rating_view_zipdir_open.is_some() {
            return pending(Owner::Rating);
        }
        if self.smart_folder_transition.is_some()
            || self.smart_folder_pending.is_some()
            || self.smart_folder_prepare_pending.is_some()
            || self.smart_folder_confirm_pending.is_some()
        {
            return pending(Owner::SmartFolder);
        }
        if self.top_level_grid_view.collection_navigation_pending() {
            return pending(Owner::Collection);
        }
        if self.subfolder_expansion_pending.is_some()
            || self.subfolder_expansion_install_pending.is_some()
            || self.subfolder_expansion_confirm_pending.is_some()
        {
            return pending(Owner::Subfolder);
        }
        if self.bookmark_open_pending.is_some()
            || matches!(
                self.bookmark_view_state,
                Some(BookmarkViewState::Opening { .. })
            )
        {
            return pending(Owner::Bookmark);
        }
        if self.main_unresolved_open_pending() {
            return pending(Owner::Startup);
        }
        if self.archive_convert.is_some() {
            return pending(Owner::Archive);
        }
        if self.pdf_enumerate_pending.is_some()
            || self.zip_enumerate_pending.is_some()
            || self.pdf_password_request.is_some()
            || self.epub_convert.is_some()
        {
            return pending(Owner::Document);
        }
        if self.saved_group_open.is_some() {
            return pending(Owner::SavedGroup);
        }
        if self.pending_return_to_parent.has_accepted_request() {
            return pending(Owner::CloseReturn);
        }
        if self.tag_view_pending.is_some() {
            return pending(Owner::Tag);
        }
        if self.book_deferred_open_wait_active() || self.book_composition_permission.open_pending()
        {
            return pending(Owner::BookOpen);
        }
        Ok(())
    }

    /// Called only after adjudication. Never use the mtime-dependent reload
    /// dispatcher here: its missing-mtime branch performs a synchronous scan.
    /// The common preflight's existing document-policy DB reads remain unchanged
    /// per §14.7; directory enumeration and duration facts stay on its worker.
    pub(super) fn start_folder_pane_reclick_refresh(
        &mut self,
        path: &Path,
    ) -> Result<(), FolderPaneReclickRejection> {
        let owner = self.current_folder_reload_owner(path);
        let (Some(intent), Some(navigation)) = (
            Self::physical_open_intent(&owner, false),
            self.capture_physical_open_navigation(&owner, path),
        ) else {
            return Err(FolderPaneReclickRejection::RequestUnavailable);
        };
        let restore_intent = self.current_list_refresh_intent();
        if !self.start_physical_history_transition_classified_at_stage(
            intent,
            path.to_path_buf(),
            None,
            Some(OpenPathKind::Directory),
            restore_intent,
            navigation,
            None,
            MainListRequestStage::Admission,
        ) {
            return Err(FolderPaneReclickRejection::WorkerAdmissionFailed);
        }
        self.folder_pane.recheck_displayed();
        Ok(())
    }

    pub(super) fn trace_folder_pane_reclick(
        &self,
        path: &Path,
        result: Result<(), FolderPaneReclickRejection>,
        started: std::time::Instant,
    ) {
        match result {
            Ok(()) => crate::logger::log(format!(
                "folder pane reclick accepted path={}",
                path.display()
            )),
            Err(reason) => crate::logger::log(format!(
                "folder pane reclick refused: {reason:?} path={}",
                path.display()
            )),
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "folder_pane",
                "reclick",
                Some(&path.to_string_lossy()),
                0,
                &[
                    (
                        "result",
                        serde_json::json!(match result {
                            Ok(()) => "accepted".to_owned(),
                            Err(reason) => format!("{reason:?}"),
                        }),
                    ),
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
    }
}
