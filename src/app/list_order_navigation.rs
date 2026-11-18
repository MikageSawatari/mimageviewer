//! Frozen book order. Position is derived from the mounted logical source, never a cursor.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListOrderEntryKind {
    Folder,
    Zip,
    Pdf,
    ConvertibleArchive,
}

#[derive(Clone, Debug)]
pub(crate) struct ListOrderEntry {
    pub(crate) path: PathBuf,
    pub(crate) kind: ListOrderEntryKind,
}

#[derive(Clone, Debug)]
pub(crate) struct ListOrderSnapshot {
    pub(crate) owner: ViewerContextId,
    pub(crate) scope: ListOrderScope,
    pub(crate) entries: Arc<[ListOrderEntry]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListOrderScope {
    Folder,
    Everything { run: u64 },
    Tag { run: u64 },
}

#[derive(Clone, Debug)]
pub(crate) enum TagViewNavAdmission {
    Descend,
    ListOrderRow { run: u64, row: PathBuf },
}

impl ListOrderEntry {
    fn from_item(item: &GridItem) -> Option<Self> {
        let (path, kind) = match item {
            GridItem::Folder(path) => (path, ListOrderEntryKind::Folder),
            GridItem::ZipFile(path) => (path, ListOrderEntryKind::Zip),
            GridItem::PdfFile(path) => (path, ListOrderEntryKind::Pdf),
            GridItem::ConvertibleArchive { path, .. } => {
                (path, ListOrderEntryKind::ConvertibleArchive)
            }
            _ => return None,
        };
        Some(Self {
            path: path.clone(),
            kind,
        })
    }
}

impl App {
    pub(crate) fn current_list_order_scope(&self) -> Option<ListOrderScope> {
        use crate::global_search_ui::SearchAccepted;
        if self.projected_viewer_context_id() != self.viewer_context_main()
            || self.is_snapshot_active()
        {
            return None;
        }
        match self.top_level_grid_view.surface() {
            top_level_grid_view::TopLevelGridSurface::Folder => Some(ListOrderScope::Folder),
            top_level_grid_view::TopLevelGridSurface::Search(
                top_level_grid_view::TopLevelSearchView::Tag,
            ) if self.tag_view.active => Some(ListOrderScope::Tag {
                run: self.tag_view.run,
            }),
            top_level_grid_view::TopLevelGridSurface::Search(
                top_level_grid_view::TopLevelSearchView::Global,
            ) if self.global_search.active => match &self.global_search.accepted {
                SearchAccepted::Everything { submitted, .. } => {
                    Some(ListOrderScope::Everything { run: submitted.run })
                }
                _ => None,
            },
            _ => None,
        }
    }
    #[cfg(test)]
    pub(crate) fn list_order_pending_owners_for_test(&self) -> (bool, bool) {
        (
            self.folder_nav_pending.is_some(),
            self.folder_pane_open_pending.is_some(),
        )
    }

    pub(crate) fn capture_main_folder_list_order(&mut self, idx: usize) {
        let Some(scope) = self.current_list_order_scope() else {
            return;
        };
        if matches!(scope, ListOrderScope::Everything { .. })
            && (!self.items_are_global_search_view
                || !matches!(
                    self.global_search.accepted,
                    crate::global_search_ui::SearchAccepted::Everything {
                        position: crate::global_search_ui::EverythingPosition::Root,
                        ..
                    }
                ))
        {
            return;
        }
        let Some(entry) = self.items.get(idx).and_then(ListOrderEntry::from_item) else {
            return;
        };
        if matches!(scope, ListOrderScope::Tag { .. })
            && (!self.items_are_tag_view || !self.tag_view.nav_stack.is_empty())
        {
            return;
        }
        if self
            .collection_grid_physical_load_owner(idx, &entry.path)
            .is_some()
            || self.rating_view_physical_load_owner(&entry.path).is_some()
        {
            return;
        }
        let mut seen = std::collections::HashSet::new();
        let entries = self
            .current_grid_order()
            .iter()
            .filter_map(|&index| self.items.get(index).and_then(ListOrderEntry::from_item))
            .filter(|entry| match scope {
                ListOrderScope::Folder | ListOrderScope::Tag { .. } => {
                    seen.insert(crate::path_key::normalize_keep_drive(&entry.path))
                }
                ListOrderScope::Everything { .. } => {
                    crate::path_key::physical_path_spelling(&entry.path)
                        .is_some_and(|key| seen.insert(key))
                }
            })
            .collect::<Vec<_>>();
        self.list_order = Some(Arc::new(ListOrderSnapshot {
            owner: self.viewer_context_main(),
            scope,
            entries: entries.into(),
        }));
    }

    pub(crate) fn list_order_position(&self) -> Option<(Arc<ListOrderSnapshot>, usize)> {
        let snapshot = self.list_order.as_ref()?;
        if snapshot.owner != self.viewer_context_main()
            || self.projected_viewer_context_id() != self.viewer_context_main()
            || !matches!(
                self.viewer_presentation,
                ViewerPresentation::MainWindow | ViewerPresentation::Fullscreen
            )
            || self.navigation_scope.is_detached_physical()
            || Some(snapshot.scope) != self.current_list_order_scope()
        {
            return None;
        }
        let current = self.effective_folder()?;
        let position = match snapshot.scope {
            ListOrderScope::Tag { .. } => {
                let root = self.tag_view.nav_stack.first()?;
                snapshot.entries.iter().position(|entry| {
                    crate::folder_tree::path_eq(&entry.path, root)
                        && (crate::folder_tree::path_eq(&current, root)
                            || (entry.kind == ListOrderEntryKind::Folder
                                && metadata_ops::path_in_subtree_ci(&current, root)))
                })?
            }
            ListOrderScope::Folder => snapshot
                .entries
                .iter()
                .position(|entry| crate::folder_tree::path_eq(&entry.path, &current))
                .or_else(|| {
                    snapshot.entries.iter().position(|entry| {
                        entry.kind == ListOrderEntryKind::Folder
                            && metadata_ops::path_in_subtree_ci(&current, &entry.path)
                    })
                })?,
            ListOrderScope::Everything { .. } => {
                use crate::global_search_ui::{EverythingPosition, SearchAccepted};
                let SearchAccepted::Everything {
                    position: EverythingPosition::PhysicalChild { logical_source, .. },
                    ..
                } = &self.global_search.accepted
                else {
                    return None;
                };
                let key = crate::path_key::physical_path_spelling(logical_source)?;
                snapshot.entries.iter().position(|entry| {
                    crate::path_key::physical_path_spelling(&entry.path).as_ref() == Some(&key)
                        && (crate::path_key::physical_path_spelling(&current).as_ref()
                            == Some(&key)
                            || (entry.kind == ListOrderEntryKind::Folder
                                && crate::path_key::physical_path_is_strict_descendant(
                                    &current,
                                    &entry.path,
                                )))
                })?
            }
        };
        Some((Arc::clone(snapshot), position))
    }

    pub(crate) fn tag_view_nav_admission(
        &self,
        mode: &FolderNavMode,
        path: &Path,
    ) -> Option<TagViewNavAdmission> {
        self.tag_view.active.then(|| match mode {
            FolderNavMode::ListOrder { snapshot, .. }
                if matches!(snapshot.scope, ListOrderScope::Tag { .. }) =>
            {
                let ListOrderScope::Tag { run } = snapshot.scope else {
                    unreachable!()
                };
                TagViewNavAdmission::ListOrderRow {
                    run,
                    row: path.to_path_buf(),
                }
            }
            _ => TagViewNavAdmission::Descend,
        })
    }

    /// A traversal owns admission until its destination listing finishes hydration.
    /// DirectNavigation scans have no DFS continuation and can be superseded normally.
    pub(crate) fn main_list_folder_navigation_adoption_pending(&self) -> bool {
        matches!(self.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request)) if request.dfs_continuation.is_some())
            || matches!(self.top_level_grid_view.open_path_classification(),
                Some(request) if matches!(request.continuation.as_deref(),
                    Some(ClassifiedOpenContinuation::Physical { dfs_continuation: Some(_), .. })))
            || self.sidecar_restore.as_ref().is_some_and(|state| {
                state.folder_navigation_adoption_pending(self.viewer_context_main())
            })
    }

    pub(crate) fn route_tag_view_grid_book_navigation(
        &mut self,
        _ctx: &egui::Context,
        forward: bool,
    ) -> bool {
        if !self.tag_view.active {
            return false;
        }
        if self.tag_view.nav_stack.is_empty() {
            self.cancel_pending_folder_nav();
            return true;
        }
        if self.folder_nav_pending.is_some() || self.main_list_folder_navigation_adoption_pending()
        {
            crate::logger::log(
                "[grid-nav] refused list-order request while folder navigation awaits adoption",
            );
            return true;
        }
        let Some((snapshot, from_index)) = self.list_order_position() else {
            self.show_feedback_warning("一覧から開いた本で使えます".into());
            return true;
        };
        let current = self
            .effective_folder()
            .expect("list-order position has a folder");
        self.start_folder_nav(
            current,
            forward,
            FolderNavMode::ListOrder {
                snapshot,
                from_index,
                fullscreen: false,
            },
        );
        true
    }
}

/// Worker-only destination selection. Reuse Ctrl traversal's stop predicate, memo and
/// archive policy, then let spawn_folder_nav perform its ordinary directory pre-scan.
pub(super) fn navigate_list_order(
    snapshot: &ListOrderSnapshot,
    from_index: usize,
    forward: bool,
    opts: crate::folder_tree::FolderTreeOptions,
    cancel: &AtomicBool,
    cache: Option<&crate::archive_cache::ArchiveCacheDb>,
    memo: Option<&crate::folder_tree::FolderNavDecisionMemo>,
) -> Option<FolderNavOutcome> {
    let mut index = from_index;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        index = if forward {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        let entry = snapshot.entries.get(index)?;
        if crate::folder_tree::folder_should_stop_with_memo(
            &entry.path,
            Some(cancel),
            opts.clone(),
            cache,
            memo,
        ) {
            return Some(FolderNavOutcome {
                path: entry.path.clone(),
                hit_image_folder: true,
            });
        }
    }
}

/// One selection request: an ordinary filename or an exact result row, never both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AfterLoadSelection {
    Name(String),
    ExactPath(PathBuf),
}
impl From<String> for AfterLoadSelection {
    fn from(name: String) -> Self {
        Self::Name(name)
    }
}
impl From<&str> for AfterLoadSelection {
    fn from(name: &str) -> Self {
        Self::Name(name.into())
    }
}
impl AfterLoadSelection {
    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            Self::ExactPath(_) => None,
        }
    }
}
