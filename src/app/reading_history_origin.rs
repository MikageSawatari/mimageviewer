//! Immutable list origins and Reading History's context-owned root exit.
use super::top_level_grid_view::{TopLevelGridOrigin, TopLevelGridRestore, TopLevelGridSurface};
use super::*;

/// A dispatch argument. Complete origins are never projected into an App-global restore slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MainListRestoreDestination {
    Native,
    Entry(FolderNavHistoryEntry),
    Origin(Box<TopLevelGridOrigin>),
}

impl App {
    /// The preceding native Rating view has left the visible row owner. Its immutable origin
    /// is already held by the admitted destination. A new Rating request remains independent.
    pub(crate) fn retire_rating_navigation_owner(&mut self) {
        self.rating_view_nav_stack.clear();
        self.rating_view_return_to = None;
        self.pending_rating_view_zipdir_open = None;
        if self
            .rating_view_pending
            .as_ref()
            .is_some_and(|pending| pending.navigation.is_none())
            && let Some(pending) = self.rating_view_pending.take()
        {
            pending.cancel();
        }
    }

    pub(super) fn smart_folder_restore_snapshot(
        &self,
        state: top_level_grid_view::SmartFolderViewState,
    ) -> SmartFolderRestore {
        SmartFolderRestore {
            state,
            saved_folder: self.smart_folder_saved_folder.clone(),
            return_to: self
                .top_level_grid_view
                .return_origin()
                .cloned()
                .map(Box::new),
        }
    }

    pub(super) fn bookmark_root_restore_snapshot(&self) -> BookmarkRootRestore {
        let selected_key = self
            .selected
            .and_then(|index| self.bookmark_browser_rows.get(index))
            .map(crate::bookmark_browser::BookmarkBrowserRow::stable_key);
        BookmarkRootRestore {
            grid: Some(BookmarkViewReturnGridState {
                row_keys: self
                    .bookmark_browser_rows
                    .iter()
                    .map(crate::bookmark_browser::BookmarkBrowserRow::stable_key)
                    .collect(),
                selected_key,
                opened_key: None,
                scroll_offset_y: self.grid_scroll.offset(),
            }),
        }
    }

    pub(super) fn restore_from_history_location(
        location: &FolderNavHistoryTarget,
    ) -> Option<TopLevelGridRestore> {
        Some(match location {
            FolderNavHistoryTarget::Path(_) => return None,
            FolderNavHistoryTarget::ReadingHistory(root) => {
                TopLevelGridRestore::ReadingHistory(root.clone())
            }
            FolderNavHistoryTarget::Rating(root) => TopLevelGridRestore::Rating(root.clone()),
            FolderNavHistoryTarget::RatingPhysical(restore) => {
                TopLevelGridRestore::RatingPhysical(restore.clone())
            }
            FolderNavHistoryTarget::Physical(restore) => {
                TopLevelGridRestore::Physical(restore.clone())
            }
            FolderNavHistoryTarget::Bookmarks(root) => TopLevelGridRestore::Bookmarks(root.clone()),
            FolderNavHistoryTarget::SmartFolder(restore) => {
                TopLevelGridRestore::SmartFolder(restore.clone())
            }
            FolderNavHistoryTarget::Collection(root) => {
                TopLevelGridRestore::Collection(root.clone())
            }
            FolderNavHistoryTarget::CollectionPhysical(restore) => {
                TopLevelGridRestore::CollectionPhysical(restore.clone())
            }
        })
    }

    pub(super) fn location_with_origin_payload(
        mut location: FolderNavHistoryTarget,
        origin: &TopLevelGridOrigin,
    ) -> FolderNavHistoryTarget {
        match (&mut location, &origin.restore) {
            (
                FolderNavHistoryTarget::SmartFolder(actual),
                TopLevelGridRestore::SmartFolder(saved),
            ) => {
                actual.saved_folder = saved.saved_folder.clone();
                actual.return_to = saved.return_to.clone();
            }
            (
                FolderNavHistoryTarget::Collection(actual),
                TopLevelGridRestore::Collection(saved),
            ) => actual.return_to = saved.return_to.clone(),
            (
                FolderNavHistoryTarget::CollectionPhysical(actual),
                TopLevelGridRestore::CollectionPhysical(saved),
            ) => actual.root.return_to = saved.root.return_to.clone(),
            (_, TopLevelGridRestore::Physical(saved)) => {
                return FolderNavHistoryTarget::Physical(saved.clone());
            }
            (_, TopLevelGridRestore::Bookmarks(saved)) => {
                return FolderNavHistoryTarget::Bookmarks(saved.clone());
            }
            _ => {}
        }
        location
    }

    /// Successful native installs and their complete return payload publish in one adoption.
    pub(super) fn install_restored_origin_payload(&mut self, origin: &TopLevelGridOrigin) {
        match &origin.restore {
            TopLevelGridRestore::SmartFolder(saved) => {
                self.smart_folder_saved_folder = saved.saved_folder.clone();
                self.top_level_grid_view
                    .replace_return_origin(saved.return_to.as_deref().cloned());
            }
            TopLevelGridRestore::Collection(root) => self
                .top_level_grid_view
                .replace_return_origin(root.return_to.as_deref().cloned()),
            TopLevelGridRestore::CollectionPhysical(saved) => self
                .top_level_grid_view
                .replace_return_origin(saved.root.return_to.as_deref().cloned()),
            TopLevelGridRestore::Bookmarks(root) => {
                self.bookmark_view_state = root
                    .grid
                    .clone()
                    .map(|grid| BookmarkViewState::Restoring { grid })
            }
            TopLevelGridRestore::Physical(saved) => {
                self.reading_history_return_from = None;
                self.bookmark_view_state = None;
                match &saved.parent {
                    PhysicalParentReturn::ReadingHistory(parent) => {
                        self.reading_history_return_from = Some(parent.clone())
                    }
                    PhysicalParentReturn::Bookmark(parent) => {
                        self.bookmark_view_state = Some(parent.clone())
                    }
                }
            }
            TopLevelGridRestore::SubfolderExpansion(saved) => {
                self.subfolder_expansion_root = saved.root.clone();
                self.subfolder_expansion_roots = saved.roots.clone();
                self.subfolder_expansion_saved_folder = saved.saved_folder.clone();
            }
            _ => {}
        }
    }

    pub(crate) fn capture_top_level_origin(&self) -> Option<TopLevelGridOrigin> {
        if matches!(
            self.top_level_grid_view.surface(),
            TopLevelGridSurface::Search(_) | TopLevelGridSurface::Snapshot
        ) && let Some(origin) = self.top_level_grid_view.return_origin()
        {
            return Some(origin.clone());
        }
        let restore = self.current_top_level_restore_snapshot()?;
        Some(TopLevelGridOrigin {
            restore,
            route: self.facet_navigation.route().clone(),
        })
    }

    pub(crate) fn unavailable_reading_history_origin(&self) -> TopLevelGridOrigin {
        let restore = TopLevelGridRestore::ReadingHistory(ReadingHistoryRootRestore::unavailable());
        TopLevelGridOrigin {
            restore,
            route: FacetRoute::root(FacetScope::path(&reading_history_synthetic_path())),
        }
    }

    pub(super) fn capture_reading_history_root(&self) -> ReadingHistoryRootRestore {
        let origin = self.capture_top_level_origin();
        if !matches!(
            self.top_level_grid_view.surface(),
            TopLevelGridSurface::Snapshot
        ) && let Some(TopLevelGridOrigin {
            restore: TopLevelGridRestore::ReadingHistory(root),
            ..
        }) = &origin
        {
            return root.clone();
        }
        let exit = match origin {
            Some(origin)
                if self.global_search.active
                    || self.favsearch.active
                    || self.tag_view.active
                    || self.suppress_nav_record_for_search_restore =>
            {
                ReadingHistoryExit::CanonicalOrigin(Box::new(origin))
            }
            Some(origin) => ReadingHistoryExit::RecordedEntrance(Box::new(origin)),
            None => ReadingHistoryExit::DriveList,
        };
        ReadingHistoryRootRestore {
            visit: ReadingHistoryVisit {
                context: self.projected_viewer_context_id().serial(),
                surface_generation: self.top_level_grid_view.generation().wrapping_add(1),
                switch_sequence: self.quick_folder_switch_sequence,
            },
            exit,
        }
    }

    pub(crate) fn capture_reading_history_book_return(
        &self,
        container: &Path,
    ) -> Option<ReadingHistoryBookReturn> {
        let TopLevelGridSurface::ReadingHistory(root) = self.top_level_grid_view.surface() else {
            return None;
        };
        Some(ReadingHistoryBookReturn {
            container: container.to_path_buf(),
            return_to: Box::new(TopLevelGridOrigin {
                restore: TopLevelGridRestore::ReadingHistory(root.clone()),
                route: self.facet_navigation.route().clone(),
            }),
        })
    }

    pub(crate) fn reading_history_root_exit_nav(&self) -> Option<crate::ui_main::AddressBarNav> {
        (self.main_folder_history_available()
            && matches!(
                self.top_level_grid_view.surface(),
                TopLevelGridSurface::ReadingHistory(_)
            ))
        .then_some(crate::ui_main::AddressBarNav::ReadingHistoryRootExit)
    }

    pub(crate) fn reading_history_root_exit_target_label(&self) -> Option<String> {
        if !self.main_folder_history_available() {
            return None;
        }
        let TopLevelGridSurface::ReadingHistory(root) = self.top_level_grid_view.surface() else {
            return None;
        };
        Some(match &root.exit {
            ReadingHistoryExit::DriveList => "ドライブ一覧".to_owned(),
            ReadingHistoryExit::RecordedEntrance(origin)
            | ReadingHistoryExit::CanonicalOrigin(origin) => match &origin.restore {
                TopLevelGridRestore::ReadingHistory(_) => "閲覧履歴".to_owned(),
                TopLevelGridRestore::Rating(root) => format!("★{} 一覧", root.stars),
                TopLevelGridRestore::DriveList => "ドライブ一覧".to_owned(),
                TopLevelGridRestore::Bookmarks(_) => "ブックマーク一覧".to_owned(),
                TopLevelGridRestore::SubfolderExpansion(_) => "サブフォルダ展開".to_owned(),
                TopLevelGridRestore::Collection(_) => "コレクション".to_owned(),
                TopLevelGridRestore::SmartFolder(saved) => match &saved.state.position {
                    top_level_grid_view::SmartFolderPosition::Root => self
                        .settings
                        .smart_folders
                        .iter()
                        .find(|definition| definition.id == saved.definition_id)
                        .map(|definition| definition.name.clone())
                        .unwrap_or_else(|| "スマートフォルダ".to_owned()),
                    top_level_grid_view::SmartFolderPosition::Scoped { current, .. }
                    | top_level_grid_view::SmartFolderPosition::Container { current, .. } => {
                        current.display().to_string()
                    }
                },
                _ => origin
                    .restore
                    .legacy_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            },
        })
    }

    pub(crate) fn exit_reading_history_root(&mut self) {
        if !self.main_folder_history_available() {
            return;
        }
        let TopLevelGridSurface::ReadingHistory(root) = self.top_level_grid_view.surface() else {
            return;
        };
        let root = root.clone();
        if self.is_snapshot_active() {
            // End the fixed scope before ordinary navigation checks its destination.
            // Dismiss consumes the shared return slot; the still-visible root remains
            // owned here, and its captured exit is authoritative, not the dismiss result.
            let _ = self.dismiss_snapshot_with_origin();
            self.top_level_grid_view
                .replace_surface(TopLevelGridSurface::ReadingHistory(root.clone()));
        }
        let Some(current) = self.folder_nav_current_entry() else {
            return;
        };
        let (back, forward) = self.active_folder_history_vectors();
        let (plan, origin) = FolderHistoryPlan::reading_history_root_exit(
            current,
            back,
            forward,
            self.active_quick_folder_slot,
            &root,
        );
        let history = plan.map_or_else(
            || MainHistoryOperation::Direct(DirectNavigationPurpose::Navigation),
            |plan| MainHistoryOperation::Replay(Box::new(plan)),
        );
        let navigation =
            self.capture_main_list_navigation(history, self.copied_destination_source_proof());
        self.dispatch_main_list_restore_destination(
            MainListRestoreDestination::Origin(Box::new(origin)),
            navigation,
        );
    }

    pub(crate) fn dispatch_main_list_restore_destination(
        &mut self,
        destination: MainListRestoreDestination,
        mut navigation: MainListNavigation,
    ) -> bool {
        let origin = match destination {
            MainListRestoreDestination::Native => return false,
            MainListRestoreDestination::Entry(entry) => {
                return self.dispatch_folder_history_entry(entry, navigation);
            }
            MainListRestoreDestination::Origin(origin) => origin,
        };
        navigation.destination = MainListRestoreDestination::Origin(origin.clone());
        match &origin.restore {
            TopLevelGridRestore::SubfolderExpansion(state) => {
                self.restore_subfolder_expansion_navigation_with_state(state.clone(), navigation)
            }
            _ => {
                let Some(location) = FolderNavHistoryTarget::from_restore(&origin.restore) else {
                    return false;
                };
                self.dispatch_owned_folder_history_destination(
                    FolderNavHistoryEntry::new(location, origin.route),
                    navigation,
                )
            }
        }
    }
}

impl FolderHistoryPlan {
    /// Treat the root's own markers as cursor boundaries, retaining real intermediate visits.
    /// The ordinary one-step Back/Forward reducer deliberately has no special-case skip rule.
    pub(crate) fn reading_history_root_exit(
        current: FolderNavHistoryEntry,
        back: &[FolderNavHistoryEntry],
        forward: &[FolderNavHistoryEntry],
        quick_slot: Option<QuickFolderSlotId>,
        root: &ReadingHistoryRootRestore,
    ) -> (Option<Self>, TopLevelGridOrigin) {
        let origin = match &root.exit {
            ReadingHistoryExit::RecordedEntrance(origin)
            | ReadingHistoryExit::CanonicalOrigin(origin) => (**origin).clone(),
            ReadingHistoryExit::DriveList => TopLevelGridOrigin {
                restore: TopLevelGridRestore::DriveList,
                route: FacetRoute::root(FacetScope::path(&drive_list_synthetic_path())),
            },
        };
        let destination = FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::from_restore(&origin.restore)
                .expect("captured navigable origin"),
            origin.route.clone(),
        );
        // Only a recorded entrance marker licenses a grouped Back. Non-recorded origins and
        // truncated entrances use the existing Direct operation, without guessing a suffix.
        let ReadingHistoryExit::RecordedEntrance(_) = &root.exit else {
            return (None, origin);
        };
        let same_visit = |entry: &FolderNavHistoryEntry| {
            matches!(&entry.location,
            FolderNavHistoryTarget::ReadingHistory(marker) if marker.visit == root.visit)
        };
        // A later visit to the entrance (including an equivalent Physical target) belongs
        // to this visit's suffix, not to its entrance. The first own root marker bounds
        // that suffix; without one, the currently visible root is the boundary.
        let boundary = back.iter().position(&same_visit).unwrap_or(back.len());
        let Some(stop) = back[..boundary].iter().rposition(|entry| {
            App::folder_nav_targets_eq(&entry.location, &destination.location)
                && entry.route == destination.route
        }) else {
            return (None, origin);
        };
        let mut plan = Self {
            previous: Some(current.clone()),
            virtual_current: Some(current),
            original_back: back.to_vec(),
            original_forward: forward.to_vec(),
            virtual_back: back.to_vec(),
            virtual_forward: forward.to_vec(),
            quick_slot,
        };
        while plan.virtual_back.len() > stop {
            let target = plan.virtual_back.pop().unwrap();
            if plan.virtual_current.as_ref().is_some_and(&same_visit) && same_visit(&target) {
                continue; // retain the currently visible root's route, not a stale marker route
            }
            if let Some(current) = plan.virtual_current.take()
                && !App::folder_nav_targets_eq(&current.location, &target.location)
            {
                App::push_folder_nav_stack(&mut plan.virtual_forward, current);
            }
            plan.virtual_current = Some(target);
        }
        if let Some(current) = plan.virtual_current.take()
            && (!App::folder_nav_targets_eq(&current.location, &destination.location)
                || current.route != destination.route)
        {
            App::push_folder_nav_stack(&mut plan.virtual_forward, current);
        }
        plan.virtual_current = Some(destination);
        (Some(plan), origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str) -> FacetRoute {
        FacetRoute::root(FacetScope::path(&PathBuf::from(format!(
            r"C:\fixture\{name}"
        ))))
    }

    fn folder(name: &str) -> FolderNavHistoryEntry {
        FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::Path(PathBuf::from(format!(r"C:\fixture\{name}"))),
            route(name),
        )
    }

    fn origin(name: &str) -> TopLevelGridOrigin {
        let entry = folder(name);
        let FolderNavHistoryTarget::Path(path) = entry.location else {
            unreachable!()
        };
        TopLevelGridOrigin {
            restore: TopLevelGridRestore::Folder(path),
            route: entry.route,
        }
    }

    fn make_root(id: u64, exit: ReadingHistoryExit) -> ReadingHistoryRootRestore {
        ReadingHistoryRootRestore {
            visit: ReadingHistoryVisit {
                context: 7,
                surface_generation: id,
                switch_sequence: 3,
            },
            exit,
        }
    }

    fn history(root: &ReadingHistoryRootRestore, route_name: &str) -> FolderNavHistoryEntry {
        FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::ReadingHistory(root.clone()),
            route(route_name),
        )
    }

    fn entry(origin: &TopLevelGridOrigin) -> FolderNavHistoryEntry {
        FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::from_restore(&origin.restore).unwrap(),
            origin.route.clone(),
        )
    }

    fn assert_plan(
        root: &ReadingHistoryRootRestore,
        current: FolderNavHistoryEntry,
        back: Vec<FolderNavHistoryEntry>,
        forward: Vec<FolderNavHistoryEntry>,
        expected_back: Vec<FolderNavHistoryEntry>,
        expected_forward: Vec<FolderNavHistoryEntry>,
    ) {
        let (plan, destination) = FolderHistoryPlan::reading_history_root_exit(
            current.clone(),
            &back,
            &forward,
            None,
            root,
        );
        let plan = plan.expect("recorded entrance marker must license one grouped Back");
        assert_eq!(plan.previous, Some(current));
        assert_eq!(plan.original_back, back);
        assert_eq!(plan.original_forward, forward);
        assert_eq!(plan.virtual_back, expected_back);
        assert_eq!(plan.virtual_forward, expected_forward);
        assert_eq!(plan.virtual_current, Some(entry(&destination)));
    }

    #[test]
    fn section1473_root_exit_plan_preserves_back_order_forward_and_intermediate_visits() {
        let o = origin("O");
        let root = make_root(
            10,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let h = history(&root, "visible-H");
        let x = folder("X");
        let b = folder("B");
        let c = folder("C");
        let f = folder("existing-forward");
        for (back, forward, result_forward) in [
            (vec![x.clone(), entry(&o)], vec![], vec![h.clone()]),
            (
                vec![x.clone(), entry(&o), h.clone()],
                vec![],
                vec![h.clone()],
            ),
            (
                vec![x.clone(), entry(&o)],
                vec![b.clone()],
                vec![b.clone(), h.clone()],
            ),
            (
                vec![x.clone(), entry(&o), h.clone(), b.clone(), c.clone()],
                vec![],
                vec![h.clone(), c.clone(), b.clone(), h.clone()],
            ),
            (
                vec![x.clone(), entry(&o), h.clone(), b.clone()],
                vec![f.clone()],
                vec![f.clone(), h.clone(), b.clone(), h.clone()],
            ),
        ] {
            assert_plan(
                &root,
                h.clone(),
                back,
                forward,
                vec![x.clone()],
                result_forward,
            );
        }
    }

    #[test]
    fn section1473_root_exit_plan_excludes_equal_revisits_after_first_own_marker() {
        let o = origin("O");
        let root = make_root(
            16,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let h = history(&root, "H");
        let entrance = entry(&o);
        for suffix in [
            vec![h.clone(), entrance.clone()],
            vec![h.clone(), entrance.clone(), h.clone(), entrance.clone()],
        ] {
            let mut back = vec![folder("X"), entrance.clone()];
            back.extend(suffix.clone());
            let mut forward = vec![folder("F"), h.clone()];
            forward.extend(suffix.into_iter().rev());
            assert_plan(
                &root,
                h.clone(),
                back,
                vec![folder("F")],
                vec![folder("X")],
                forward,
            );
        }
        // An equal revisit cannot replace an entrance that fell out of the bounded stack.
        let back = vec![folder("X"), h.clone(), entrance];
        let forward = vec![folder("F")];
        let (plan, actual) =
            FolderHistoryPlan::reading_history_root_exit(h, &back, &forward, None, &root);
        assert!(plan.is_none());
        assert_eq!(actual, o);
        assert_eq!(back, vec![folder("X"), history(&root, "H"), entry(&o)]);
        assert_eq!(forward, vec![folder("F")]);
    }

    #[test]
    fn section1473_root_exit_plan_without_own_marker_keeps_entrance_destination() {
        let o = origin("O");
        let root = make_root(
            17,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let h = history(&root, "H");
        for back in [
            vec![folder("X"), entry(&o)],
            vec![folder("X"), entry(&o), folder("Y"), entry(&o)],
        ] {
            let forward = vec![folder("F")];
            let (_plan, actual) = FolderHistoryPlan::reading_history_root_exit(
                h.clone(),
                &back,
                &forward,
                None,
                &root,
            );
            // Both markers may have expired while an equal revisit remains. The accepted
            // tradeoff preserves the entrance destination, without fixing stack ordering.
            assert_eq!(actual, o);
        }
    }

    #[test]
    fn section1473_root_exit_plan_skips_only_own_adjacent_markers_and_keeps_visible_route() {
        let o = origin("O");
        let root = make_root(
            11,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let h = history(&root, "visible-route");
        let stale = history(&root, "stale-marker-route");
        let stale2 = history(&root, "another-marker-route");
        let old_root = make_root(
            9,
            ReadingHistoryExit::CanonicalOrigin(Box::new(origin("old-origin"))),
        );
        let old_h = history(&old_root, "same-surface-other-visit");
        assert_plan(
            &root,
            h.clone(),
            vec![folder("X"), entry(&o), stale.clone(), stale2],
            vec![folder("F")],
            vec![folder("X")],
            vec![folder("F"), h.clone()],
        );
        assert_plan(
            &root,
            h.clone(),
            vec![folder("X"), entry(&o), old_h.clone(), stale],
            vec![],
            vec![folder("X")],
            vec![h, old_h],
        );
    }

    #[test]
    fn section1473_root_exit_plan_matches_entrance_route_and_dispatches_complete_saved_payload() {
        let o = origin("O");
        let h1 = make_root(1, ReadingHistoryExit::RecordedEntrance(Box::new(o)));
        let saved = TopLevelGridOrigin {
            restore: TopLevelGridRestore::Rating(RatingRootRestore {
                stars: 3,
                return_to: Some(Box::new(TopLevelGridOrigin {
                    restore: TopLevelGridRestore::ReadingHistory(h1),
                    route: route("H1-route"),
                })),
            }),
            route: route("exact-rating-entrance"),
        };
        let h2 = make_root(
            2,
            ReadingHistoryExit::RecordedEntrance(Box::new(saved.clone())),
        );
        let current = history(&h2, "H2-route");
        let marker = FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::Rating(RatingRootRestore {
                stars: 3,
                return_to: None,
            }),
            saved.route.clone(),
        );
        let wrong_route =
            FolderNavHistoryEntry::new(marker.location.clone(), route("different-rating-entrance"));
        let (plan, actual) = FolderHistoryPlan::reading_history_root_exit(
            current.clone(),
            &[folder("X"), marker, wrong_route],
            &[],
            None,
            &h2,
        );
        let plan = plan.unwrap();
        assert_eq!(actual, saved);
        assert_eq!(plan.virtual_current, Some(entry(&saved)));
        assert_eq!(plan.virtual_back, vec![folder("X")]);
        assert_eq!(plan.virtual_forward, vec![current]);
        let (missing, absent) = FolderHistoryPlan::reading_history_root_exit(
            history(&h2, "H2-route"),
            &[FolderNavHistoryEntry::new(
                entry(&saved).location,
                route("wrong-route-only"),
            )],
            &[],
            None,
            &h2,
        );
        assert!(missing.is_none());
        assert_eq!(absent, saved);
    }

    #[test]
    fn section1473_root_exit_plan_uses_direct_for_canonical_missing_and_truncated_entrances() {
        let o = origin("O");
        let recorded = make_root(
            12,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let canonical = make_root(13, ReadingHistoryExit::CanonicalOrigin(Box::new(o.clone())));
        let none = make_root(14, ReadingHistoryExit::DriveList);
        let truncated = (0..MAX_FOLDER_NAV_STACK)
            .map(|n| folder(&format!("remaining-{n}")))
            .collect::<Vec<_>>();
        for (root, back) in [
            (&recorded, vec![]),
            (&recorded, truncated),
            (
                &recorded,
                vec![history(&recorded, "own-marker-but-no-entrance")],
            ),
            (
                &canonical,
                vec![folder("X"), entry(&o), history(&canonical, "own-marker")],
            ),
            (&none, vec![]),
        ] {
            let original_back = back.clone();
            let forward = vec![folder("existing-forward")];
            let (plan, target) = FolderHistoryPlan::reading_history_root_exit(
                history(root, "visible"),
                &back,
                &forward,
                None,
                root,
            );
            assert!(
                plan.is_none(),
                "a missing recorded entrance must not reconstruct the stacks"
            );
            assert_eq!(back, original_back);
            if matches!(root.exit, ReadingHistoryExit::DriveList) {
                assert!(matches!(target.restore, TopLevelGridRestore::DriveList));
            } else {
                assert_eq!(target, o);
            }
        }
    }

    #[test]
    fn section1473_root_exit_plan_keeps_each_quick_slot_baseline_separate() {
        let o = origin("O");
        let root = make_root(
            15,
            ReadingHistoryExit::RecordedEntrance(Box::new(o.clone())),
        );
        let current = history(&root, "visible");
        let normal = vec![folder("normal"), entry(&o)];
        let a = vec![folder("slot-A"), entry(&o), current.clone()];
        let b = vec![
            folder("slot-B"),
            entry(&o),
            current.clone(),
            folder("B-book"),
        ];
        for (slot, back) in [
            (None, &normal),
            (Some(QuickFolderSlotId::A), &a),
            (Some(QuickFolderSlotId::B), &b),
        ] {
            let (plan, _) = FolderHistoryPlan::reading_history_root_exit(
                current.clone(),
                back,
                &[],
                slot,
                &root,
            );
            let plan = plan.unwrap();
            assert_eq!(plan.quick_slot, slot);
            assert_eq!(&plan.original_back, back);
            assert_eq!(plan.virtual_back, vec![back[0].clone()]);
        }
        assert_eq!(normal, vec![folder("normal"), entry(&o)]);
        assert_eq!(a, vec![folder("slot-A"), entry(&o), current.clone()]);
        assert_eq!(
            b,
            vec![folder("slot-B"), entry(&o), current, folder("B-book")]
        );
    }

    fn settle_navigation(app: &mut App) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            app.settle_open_path_classification_for_test();
            app.poll_sidecar_restore(&ctx);
            app.poll_collection_history_transition(&ctx);
            app.poll_smart_folder(&ctx);
            if app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
                && !app.smart_folder_busy()
                && !app.sidecar_restore_active()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "navigation did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn section1473_rapid_history_rebinds_typed_and_plain_physical_destinations() {
        for typed_middle in [true, false] {
            let mut app = crate::app::setup_app_for_test();
            app.active_quick_folder_slot = None;
            let source = app.tmp.path().join("source");
            let middle = app.tmp.path().join("middle");
            let oldest = app.tmp.path().join("oldest");
            for path in [&source, &middle, &oldest] {
                std::fs::create_dir_all(path).unwrap();
            }
            app.current_folder = Some(source.clone());
            app.adopt_main_facet_route(FacetRoute::root(FacetScope::path(&source)));
            let source_entry = app.folder_nav_current_entry().unwrap();
            let make_target = |path: &Path, typed: bool| {
                let location = if typed {
                    FolderNavHistoryTarget::Physical(PhysicalGridRestore {
                        visible_path: path.to_path_buf(),
                        parent: PhysicalParentReturn::ReadingHistory(
                            ReadingHistoryBookReturn::without_entrance(path.to_path_buf()),
                        ),
                    })
                } else {
                    FolderNavHistoryTarget::Path(path.to_path_buf())
                };
                FolderNavHistoryEntry::new(location, FacetRoute::root(FacetScope::path(path)))
            };
            let middle_entry = make_target(&middle, typed_middle);
            let oldest_entry = make_target(&oldest, !typed_middle);
            app.folder_nav_back_stack = vec![oldest_entry.clone(), middle_entry.clone()];
            let before = app.folder_nav_history_snapshot();
            app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
            app.settle_open_path_classification_for_test();
            app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
            app.settle_open_path_classification_for_test();
            assert_eq!(app.current_folder.as_ref(), Some(&source));
            assert_eq!(app.folder_nav_back_stack, before.back_stack);
            assert_eq!(app.folder_nav_forward_stack, before.forward_stack);
            settle_navigation(&mut app);
            assert_eq!(app.current_folder.as_ref(), Some(&oldest));
            assert_eq!(app.facet_navigation.route(), &oldest_entry.route);
            assert_eq!(
                app.reading_history_return_from,
                (!typed_middle).then(|| ReadingHistoryBookReturn::without_entrance(oldest.clone()))
            );
            assert!(app.folder_nav_back_stack.is_empty());
            assert_eq!(
                app.folder_nav_forward_stack,
                vec![source_entry, middle_entry]
            );
        }
    }

    #[test]
    fn section1473_rapid_smart_history_rebinds_destination_before_cross_kind_handoff() {
        let mut app = crate::app::setup_app_for_test();
        app.active_quick_folder_slot = None;
        let source = app.tmp.path().join("source");
        let oldest = app.tmp.path().join("oldest");
        let smart_source = app.tmp.path().join("smart-source");
        for path in [&source, &oldest, &smart_source] {
            std::fs::create_dir_all(path).unwrap();
        }
        app.current_folder = Some(source.clone());
        app.adopt_main_facet_route(FacetRoute::root(FacetScope::path(&source)));
        let source_entry = app.folder_nav_current_entry().unwrap();
        let mut definition = crate::settings::SmartFolderDefinition::new("rapid history");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            smart_source,
            true,
            Default::default(),
        ));
        let saved = SmartFolderRestore {
            state: top_level_grid_view::SmartFolderViewState::root(definition.id, vec![]),
            saved_folder: Some(PathBuf::from(r"C:\fixture\smart-saved-folder")),
            return_to: Some(Box::new(origin("smart-return-origin"))),
        };
        app.settings.smart_folders = vec![definition];
        let smart_entry = FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::SmartFolder(saved),
            route("saved-smart-route"),
        );
        let oldest_entry = FolderNavHistoryEntry::new(
            FolderNavHistoryTarget::Path(oldest.clone()),
            FacetRoute::root(FacetScope::path(&oldest)),
        );
        app.folder_nav_back_stack = vec![oldest_entry.clone(), smart_entry.clone()];
        let before = app.folder_nav_history_snapshot();
        app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        assert!(app.staged_smart_history_navigation().is_some());
        app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        assert_eq!(app.current_folder.as_ref(), Some(&source));
        assert_eq!(app.folder_nav_back_stack, before.back_stack);
        settle_navigation(&mut app);
        assert_eq!(app.current_folder.as_ref(), Some(&oldest));
        assert_eq!(app.facet_navigation.route(), &oldest_entry.route);
        assert!(app.smart_folder_saved_folder.is_none());
        assert!(app.top_level_grid_view.return_origin().is_none());
        assert_eq!(
            app.folder_nav_forward_stack,
            vec![source_entry, smart_entry]
        );
    }

    #[test]
    fn section1473_direct_smart_entry_captures_source_route_before_adoption() {
        let mut app = crate::app::setup_app_for_test();
        app.active_quick_folder_slot = None;
        let source = app.tmp.path().join("origin");
        let smart_source = app.tmp.path().join("smart-source");
        for path in [&source, &smart_source] {
            std::fs::create_dir_all(path).unwrap();
        }
        app.current_folder = Some(source.clone());
        let source_route = FacetRoute::root(FacetScope::path(&source));
        app.adopt_main_facet_route(source_route.clone());
        let mut definition = crate::settings::SmartFolderDefinition::new("capture origin");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            smart_source,
            true,
            Default::default(),
        ));
        let id = definition.id;
        app.settings.smart_folders = vec![definition];
        assert!(app.begin_smart_folder_staged(id, false).is_some());
        settle_navigation(&mut app);
        assert_ne!(app.facet_navigation.route(), &source_route);
        let restored_origin = app.top_level_grid_view.return_origin().unwrap();
        assert_eq!(restored_origin.route, source_route);
        assert_eq!(restored_origin.restore, TopLevelGridRestore::Folder(source));
    }
}
