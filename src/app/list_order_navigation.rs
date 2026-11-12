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
                ListOrderScope::Folder => {
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
