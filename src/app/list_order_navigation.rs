//! Frozen main-folder book order. Position is derived from the mounted logical source;
//! open requests and their asynchronous adoption never carry or update a cursor.
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
    pub(crate) entries: Arc<[ListOrderEntry]>,
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
    #[cfg(test)]
    pub(crate) fn list_order_pending_owners_for_test(&self) -> (bool, bool) {
        (
            self.folder_nav_pending.is_some(),
            self.folder_pane_open_pending.is_some(),
        )
    }

    pub(crate) fn capture_main_folder_list_order(&mut self, idx: usize) {
        if self.projected_viewer_context_id() != self.viewer_context_main()
            || !matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
            || self.is_snapshot_active()
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
            .filter(|entry| seen.insert(crate::path_key::normalize_keep_drive(&entry.path)))
            .collect::<Vec<_>>();
        self.list_order = Some(Arc::new(ListOrderSnapshot {
            owner: self.viewer_context_main(),
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
            || !matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
            || self.is_snapshot_active()
        {
            return None;
        }
        let current = self.effective_folder()?;
        let position = snapshot
            .entries
            .iter()
            .position(|entry| crate::folder_tree::path_eq(&entry.path, &current))
            .or_else(|| {
                snapshot.entries.iter().position(|entry| {
                    entry.kind == ListOrderEntryKind::Folder
                        && metadata_ops::path_in_subtree_ci(&current, &entry.path)
                })
            })?;
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
