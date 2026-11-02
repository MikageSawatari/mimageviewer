//! Left-side filesystem folder tree pane state.
//!
//! This pane is intentionally separate from `folder_tree`: the latter is the
//! Ctrl+Up/Down DFS navigator and treats ZIP/PDF files as virtual folders.  The
//! pane here shows only real filesystem directories.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::settings::{FolderTreeSortOrder, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FolderPaneListingOptions {
    pub(crate) sort_order: FolderTreeSortOrder,
    pub(crate) show_hidden_files: bool,
}

impl FolderPaneListingOptions {
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            sort_order: settings.folder_tree_sort_order,
            show_hidden_files: settings.show_hidden_files,
        }
    }
}

impl Default for FolderPaneListingOptions {
    fn default() -> Self {
        Self {
            sort_order: FolderTreeSortOrder::default(),
            show_hidden_files: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPaneTreeKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FolderPaneCommand {
    Open(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderPaneRow {
    pub path: PathBuf,
    pub depth: usize,
    pub expanded: bool,
    pub loading: bool,
    pub has_children_or_unknown: bool,
    pub is_active: bool,
    pub is_cursor: bool,
    pub error: Option<String>,
}

/// Only a complete, successful enumeration can prove that a branch disappeared.
#[derive(Debug, Clone)]
struct CachedListing {
    children: Vec<PathBuf>,
    membership: HashSet<String>,
    options: FolderPaneListingOptions,
}

struct FolderPaneNode {
    path: PathBuf,
    cache: Option<CachedListing>,
    scan: NodeScan,
}

enum NodeScan {
    Unverified,
    Queued,
    Running(FolderPaneScanPending, FollowUp),
    Settled {
        options: FolderPaneListingOptions,
        error: Option<String>,
    },
}

#[derive(Clone, Copy)]
enum FollowUp {
    None,
    Recheck,
}

impl FolderPaneNode {
    fn placeholder(path: PathBuf) -> Self {
        Self {
            path,
            cache: None,
            scan: NodeScan::Unverified,
        }
    }

    fn children(&self) -> &[PathBuf] {
        self.cache
            .as_ref()
            .map_or(&[], |cache| cache.children.as_slice())
    }
}

struct FolderPaneScanPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<CachedListing, String>>,
    options: FolderPaneListingOptions,
}

impl Drop for FolderPaneScanPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // A completed listing may already be queued. Retire its payload off the UI thread.
        let rx = std::mem::replace(&mut self.rx, mpsc::channel().1);
        rayon::spawn(move || drop(rx));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaneParticipation {
    Suspended,
    Displayed,
}

#[cfg(test)]
type TestScanBackend = Arc<
    dyn Fn(&Path, FolderPaneListingOptions, &AtomicBool) -> Result<CachedListing, String>
        + Send
        + Sync,
>;

pub(crate) struct FolderPaneState {
    pub has_focus: bool,
    pub scroll_to_cursor: bool,
    pub selected_drive: Option<PathBuf>,
    cursor_path: Option<PathBuf>,
    active_path: Option<PathBuf>,
    active_key: Option<String>,
    drives: Vec<PathBuf>,
    nodes: HashMap<String, FolderPaneNode>,
    user_expanded: HashSet<String>,
    auto_expanded: HashSet<String>,
    user_collapsed: HashSet<String>,
    demand: HashSet<String>,
    participation: PaneParticipation,
    listing_options: FolderPaneListingOptions,
    last_drive_refresh: Option<std::time::Instant>,
    #[cfg(test)]
    scan_backend: Option<TestScanBackend>,
}

impl Default for FolderPaneState {
    fn default() -> Self {
        let drives = crate::known_folders::available_drives();
        Self {
            has_focus: false,
            scroll_to_cursor: false,
            selected_drive: drives.first().cloned(),
            drives,
            cursor_path: None,
            active_path: None,
            active_key: None,
            nodes: HashMap::new(),
            user_expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            user_collapsed: HashSet::new(),
            demand: HashSet::new(),
            participation: PaneParticipation::Suspended,
            listing_options: FolderPaneListingOptions::default(),
            last_drive_refresh: None,
            #[cfg(test)]
            scan_backend: None,
        }
    }
}

impl FolderPaneState {
    pub(crate) fn drives(&self) -> &[PathBuf] {
        &self.drives
    }
    pub(crate) fn active_path(&self) -> Option<&Path> {
        self.active_path.as_deref()
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.nodes
            .values()
            .any(|node| matches!(node.scan, NodeScan::Queued | NodeScan::Running(..)))
    }

    /// Only the actual main chrome drawing boundary resumes participation.
    pub(crate) fn resume_display(&mut self) {
        self.participation = PaneParticipation::Displayed;
    }

    pub(crate) fn suspend_display(&mut self) {
        if self.participation == PaneParticipation::Suspended {
            return;
        }
        self.participation = PaneParticipation::Suspended;
        self.cancel_pending();
        self.demand.clear();
    }

    pub(crate) fn set_focus_tree(&mut self) {
        self.has_focus = true;
        if self.cursor_path.is_none() {
            self.cursor_path = self
                .active_path
                .clone()
                .or_else(|| self.selected_drive.clone());
        }
        self.scroll_to_cursor = true;
    }

    pub(crate) fn set_focus_tree_at_active(&mut self) {
        if let Some(active) = self.active_path.clone() {
            if let Some(root) = self.selected_drive.as_ref() {
                for ancestor in ancestor_chain(root, &active) {
                    self.user_collapsed.remove(&key_for(&ancestor));
                }
            }
            self.reveal_active(false);
            self.cursor_path = Some(active);
        }
        self.set_focus_tree();
    }

    pub(crate) fn set_focus_grid(&mut self) {
        self.has_focus = false;
    }

    pub(crate) fn cursor_nav_target_if_moved(&self) -> Option<PathBuf> {
        let cursor = self.cursor_path.as_ref()?;
        if self
            .active_path
            .as_deref()
            .is_some_and(|active| crate::folder_tree::path_eq(active, cursor))
        {
            return None;
        }
        Some(cursor.clone())
    }

    pub(crate) fn refresh_drives(&mut self) {
        let now = std::time::Instant::now();
        if self
            .last_drive_refresh
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(1500))
        {
            return;
        }
        self.last_drive_refresh = Some(now);
        let drives = crate::known_folders::available_drives();
        if drives == self.drives {
            return;
        }
        self.drives = drives;
        if self.selected_drive.as_ref().is_some_and(|selected| {
            self.drives
                .iter()
                .any(|drive| crate::folder_tree::path_eq(drive, selected))
        }) {
            return;
        }
        self.selected_drive = self.drives.first().cloned();
    }

    pub(crate) fn select_drive(&mut self, drive: PathBuf) {
        self.has_focus = true;
        // An actual selection, including the selected drive, is an explicit recheck.
        // Invalidate all participants, so an unchanged root also refreshes its open branches.
        self.cancel_pending();
        self.demand.clear();
        self.selected_drive = Some(drive.clone());
        self.cursor_path = Some(drive.clone());
        self.scroll_to_cursor = true;
        self.ensure_node(drive);
        self.reconcile_demand();
    }

    pub(crate) fn reload_for_active(
        &mut self,
        active: Option<&Path>,
        options: FolderPaneListingOptions,
    ) {
        self.cancel_pending();
        let retired = std::mem::take(&mut self.nodes);
        rayon::spawn(move || drop(retired));
        self.demand.clear();
        self.user_expanded.clear();
        self.auto_expanded.clear();
        self.user_collapsed.clear();
        self.active_key = None;
        self.listing_options = options;
        self.sync_to_active(active, options);
        self.cursor_path = self
            .active_path
            .clone()
            .or_else(|| self.selected_drive.clone());
        self.scroll_to_cursor = true;
    }

    pub(crate) fn sync_to_active(
        &mut self,
        active: Option<&Path>,
        options: FolderPaneListingOptions,
    ) {
        self.refresh_drives();
        if self.listing_options != options {
            self.cancel_pending();
            self.listing_options = options;
        }
        let active = active.and_then(active_filesystem_folder);
        let key = active.as_ref().map(|path| key_for(path));
        let changed = self.active_key != key;
        if changed {
            self.auto_expanded.clear();
            self.user_collapsed.clear();
            self.active_key = key;
            if !self.has_focus || self.cursor_path.is_none() {
                self.cursor_path = active.clone();
                self.scroll_to_cursor = true;
            }
        }
        self.active_path = active;
        self.reveal_active(changed);
        if let Some(root) = self.selected_drive.clone() {
            self.ensure_node(root);
        }
        self.reconcile_demand();
        self.start_queued();
    }

    fn reveal_active(&mut self, changed: bool) {
        let Some(active) = self.active_path.clone() else {
            return;
        };
        let Some(root) = root_of_path(&active) else {
            return;
        };
        if self.has_focus
            && self
                .selected_drive
                .as_ref()
                .is_some_and(|drive| !crate::folder_tree::path_eq(drive, &root))
        {
            return;
        }
        self.selected_drive = Some(root.clone());
        self.ensure_node(root.clone());
        for pair in ancestor_chain(&root, &active).windows(2) {
            let parent_key = key_for(&pair[0]);
            self.auto_expanded.insert(parent_key.clone());
            if self.user_collapsed.contains(&parent_key) {
                break;
            }
            if let Some(cache) = self
                .nodes
                .get(&parent_key)
                .and_then(|node| node.cache.as_ref())
            {
                if !cache.membership.contains(&key_for(&pair[1])) {
                    // A NEW location may have been created after this parent's last scan.
                    // Retry this first contradiction once, never on same-location frame sync.
                    if changed {
                        self.request_recheck(&pair[0]);
                    }
                    break;
                }
                if !cache
                    .children
                    .iter()
                    .any(|child| crate::folder_tree::path_eq(child, &pair[1]))
                {
                    break;
                }
            }
            self.ensure_node(pair[1].clone());
        }
    }

    /// A hint after main's accepted physical-folder observation. Never creates a node,
    /// opens a folder, or touches any viewer-owned queue/watch/generation.
    pub(crate) fn observe_accepted_folder(&mut self, folder: &Path) {
        self.request_recheck(folder);
    }

    fn request_recheck(&mut self, path: &Path) {
        let key = key_for(path);
        let Some(node) = self.nodes.get_mut(&key) else {
            return;
        };
        match &mut node.scan {
            NodeScan::Running(_, follow_up) => *follow_up = FollowUp::Recheck,
            NodeScan::Queued => {}
            _ => {
                node.scan = if self.demand.contains(&key) {
                    NodeScan::Queued
                } else {
                    NodeScan::Unverified
                }
            }
        }
        self.trace("recheck", path);
    }

    fn trace(&self, action: &str, path: &Path) {
        if crate::perf::is_enabled() {
            crate::perf::event("folder_pane", action, Some(&path.to_string_lossy()), 0, &[]);
        }
    }

    fn reachable(&self, path: &Path) -> bool {
        let Some(root) = &self.selected_drive else {
            return false;
        };
        if !path_is_same_or_descendant(path, root) {
            return false;
        }
        for pair in ancestor_chain(root, path).windows(2) {
            let key = key_for(&pair[0]);
            if !self.is_expanded_key(&key) {
                return false;
            }
            if let Some(cache) = self.nodes.get(&key).and_then(|node| node.cache.as_ref()) {
                if !cache
                    .children
                    .iter()
                    .any(|child| crate::folder_tree::path_eq(child, &pair[1]))
                {
                    return false;
                }
            } else if !self.auto_expanded.contains(&key_for(&pair[1]))
                && self
                    .active_path
                    .as_deref()
                    .is_none_or(|active| !crate::folder_tree::path_eq(active, &pair[1]))
            {
                return false;
            }
        }
        true
    }

    fn reconcile_demand(&mut self) {
        if self.participation == PaneParticipation::Suspended {
            return;
        }
        let mut paths: Vec<_> = self
            .visible_rows()
            .into_iter()
            .filter(|row| row.expanded || row.depth == 0)
            .map(|row| row.path)
            .collect();
        // Unknown ancestors can still prefetch concurrently, within the existing depth bound.
        paths.extend(
            self.auto_expanded
                .iter()
                .filter_map(|key| self.nodes.get(key))
                .filter(|node| {
                    self.is_expanded_key(&key_for(&node.path)) && self.reachable(&node.path)
                })
                .map(|node| node.path.clone()),
        );
        let next: HashSet<_> = paths.iter().map(|path| key_for(path)).collect();
        for key in self.demand.difference(&next) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.scan = NodeScan::Unverified;
            }
        }
        for path in paths {
            let key = key_for(&path);
            self.ensure_node(path);
            let node = self.nodes.get_mut(&key).unwrap();
            if !self.demand.contains(&key)
                || matches!(node.scan, NodeScan::Unverified)
                || matches!(node.scan, NodeScan::Settled { options, .. } if options != self.listing_options)
            {
                node.scan = NodeScan::Queued;
            }
        }
        self.demand = next;
    }

    fn start_queued(&mut self) {
        const START_BUDGET: usize = 4;
        let mut keys: Vec<_> = self
            .demand
            .iter()
            .filter(|key| {
                self.nodes
                    .get(*key)
                    .is_some_and(|node| matches!(node.scan, NodeScan::Queued))
            })
            .cloned()
            .collect();
        keys.sort_by_key(|key| self.nodes[key].path.components().count());
        for key in keys.into_iter().take(START_BUDGET) {
            let path = self.nodes[&key].path.clone();
            let options = self.listing_options;
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancel_w = cancel.clone();
            let scan_path = path.clone();
            #[cfg(test)]
            let backend = self.scan_backend.clone();
            let spawned = std::thread::Builder::new()
                .name("folder-pane-scan".into())
                .spawn(move || {
                    #[cfg(test)]
                    let result = if let Some(backend) = backend {
                        backend(&scan_path, options, &cancel_w)
                    } else {
                        scan_complete_listing(&scan_path, options, Some(&cancel_w))
                            .map_err(|err| err.to_string())
                    };
                    #[cfg(not(test))]
                    let result = scan_complete_listing(&scan_path, options, Some(&cancel_w))
                        .map_err(|err| err.to_string());
                    if !cancel_w.load(Ordering::Relaxed) {
                        let _ = tx.send(result);
                    }
                });
            self.nodes.get_mut(&key).unwrap().scan = match spawned {
                Ok(_) => NodeScan::Running(
                    FolderPaneScanPending {
                        cancel,
                        rx,
                        options,
                    },
                    FollowUp::None,
                ),
                Err(err) => {
                    crate::logger::log(format!(
                        "folder pane scan spawn failed: {}: {err}",
                        path.display()
                    ));
                    NodeScan::Settled {
                        options,
                        error: Some(format!("列挙スレッドを開始できません: {err}")),
                    }
                }
            };
            self.trace("enqueue", &path);
        }
    }

    pub(crate) fn poll_pending(&mut self) -> bool {
        const APPLY_BUDGET: usize = 4;
        let mut keys: Vec<_> = self
            .demand
            .iter()
            .filter(|key| {
                self.nodes
                    .get(*key)
                    .is_some_and(|node| matches!(node.scan, NodeScan::Running(..)))
            })
            .cloned()
            .collect();
        keys.sort_by_key(|key| self.nodes[key].path.components().count());
        let mut applied = 0;
        let mut changed = false;
        for key in keys {
            if applied == APPLY_BUDGET {
                break;
            }
            let Some(node) = self.nodes.get_mut(&key) else {
                continue;
            };
            let NodeScan::Running(pending, follow_up) =
                std::mem::replace(&mut node.scan, NodeScan::Unverified)
            else {
                continue;
            };
            let result = match pending.rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    node.scan = NodeScan::Running(pending, follow_up);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("列挙に失敗しました".into()),
            };
            applied += 1;
            if matches!(follow_up, FollowUp::Recheck) {
                node.scan = NodeScan::Queued;
                rayon::spawn(move || drop(result));
                continue;
            }
            let path = node.path.clone();
            if pending.options != self.listing_options || !self.demand.contains(&key) {
                rayon::spawn(move || drop(result));
                continue;
            }
            let error = match result {
                Ok(cache) => {
                    debug_assert_eq!(cache.options, pending.options);
                    let previous = self.nodes.get_mut(&key).unwrap().cache.replace(cache);
                    rayon::spawn(move || drop(previous));
                    self.prune_missing_children(&path);
                    let children = self.nodes[&key].children().to_vec();
                    self.repair_cursor_after_successful_refresh(&path, &children);
                    None
                }
                Err(error) => {
                    crate::logger::log(format!(
                        "folder pane scan failed: {}: {error}",
                        path.display()
                    ));
                    Some(error)
                }
            };
            self.nodes.get_mut(&key).unwrap().scan = NodeScan::Settled {
                options: pending.options,
                error,
            };
            self.trace("settled", &path);
            self.reveal_active(false);
            self.reconcile_demand();
            changed = true;
        }
        self.start_queued();
        changed
    }

    fn prune_missing_children(&mut self, parent: &Path) {
        let membership = &self.nodes[&key_for(parent)]
            .cache
            .as_ref()
            .unwrap()
            .membership;
        let removed: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|(key, node)| {
                let child = node.path.ancestors().find(|candidate| {
                    candidate
                        .parent()
                        .is_some_and(|p| crate::folder_tree::path_eq(p, parent))
                })?;
                (!membership.contains(&key_for(child))).then_some(key.clone())
            })
            .collect();
        let mut retired = Vec::new();
        for key in removed {
            if let Some(mut node) = self.nodes.remove(&key) {
                node.scan = NodeScan::Unverified; // cancel/drop the private receiver before retirement
                retired.push(node);
            }
            self.user_expanded.remove(&key);
            self.auto_expanded.remove(&key);
            self.user_collapsed.remove(&key);
            self.demand.remove(&key);
        }
        if !retired.is_empty() {
            rayon::spawn(move || drop(retired));
        }
    }

    pub(crate) fn visible_rows(&self) -> Vec<FolderPaneRow> {
        let mut rows = Vec::new();
        if let Some(root) = &self.selected_drive {
            self.push_visible_rows(root, 0, &mut rows, &mut HashSet::new());
        }
        rows
    }

    pub(crate) fn set_cursor(&mut self, path: PathBuf) {
        self.cursor_path = Some(path);
        self.scroll_to_cursor = true;
    }

    pub(crate) fn handle_tree_key(&mut self, key: FolderPaneTreeKey) -> Option<FolderPaneCommand> {
        match key {
            FolderPaneTreeKey::Up => self.move_cursor(-1),
            FolderPaneTreeKey::Down => self.move_cursor(1),
            FolderPaneTreeKey::Left => self.collapse_cursor(),
            FolderPaneTreeKey::Right => self.expand_cursor(),
            FolderPaneTreeKey::Enter => {
                return self.cursor_path.clone().map(FolderPaneCommand::Open);
            }
        }
        None
    }

    fn move_cursor(&mut self, delta: isize) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .cursor_path
            .as_ref()
            .and_then(|cursor| {
                rows.iter()
                    .position(|row| crate::folder_tree::path_eq(&row.path, cursor))
            })
            .or_else(|| {
                self.active_path.as_ref().and_then(|active| {
                    rows.iter()
                        .position(|row| crate::folder_tree::path_eq(&row.path, active))
                })
            })
            .unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(rows.len() - 1)
        };
        self.set_cursor(rows[next].path.clone());
    }

    fn collapse_cursor(&mut self) {
        let Some(cursor) = self.cursor_path.clone() else {
            return;
        };
        let key = key_for(&cursor);
        if self.is_expanded_key(&key) {
            self.user_expanded.remove(&key);
            if self.auto_expanded.contains(&key) {
                self.user_collapsed.insert(key);
            }
            // The root is always demanded, but its next expansion must also recheck.
            if let Some(node) = self.nodes.get_mut(&key_for(&cursor)) {
                node.scan = NodeScan::Unverified;
            }
            self.reconcile_demand();
            self.scroll_to_cursor = true;
        } else if let Some(parent) = cursor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.set_cursor(parent.to_path_buf());
        }
    }

    fn expand_cursor(&mut self) {
        let Some(cursor) = self.cursor_path.clone() else {
            return;
        };
        let key = key_for(&cursor);
        if !self.is_expanded_key(&key) {
            self.user_expanded.insert(key.clone());
            self.user_collapsed.remove(&key);
            self.ensure_node(cursor.clone());
            self.request_recheck(&cursor);
            self.reconcile_demand();
            self.scroll_to_cursor = true;
        } else if let Some(child) = self
            .nodes
            .get(&key)
            .and_then(|node| node.children().first())
            .cloned()
        {
            self.set_cursor(child);
        }
    }

    fn push_visible_rows(
        &self,
        path: &Path,
        depth: usize,
        rows: &mut Vec<FolderPaneRow>,
        seen: &mut HashSet<String>,
    ) {
        if depth > 64 {
            return;
        }
        let key = key_for(path);
        if !seen.insert(key.clone()) {
            return;
        }
        let expanded = self.is_expanded_key(&key);
        let node = self.nodes.get(&key);
        let loading =
            node.is_some_and(|node| matches!(node.scan, NodeScan::Queued | NodeScan::Running(..)));
        let error = node.and_then(|node| match &node.scan {
            NodeScan::Settled { error, .. } => error.clone(),
            _ => None,
        });
        // No branch is watched: even a confirmed expanded empty must remain collapsible.
        let has_children_or_unknown = true;
        rows.push(FolderPaneRow {
            path: path.to_path_buf(),
            depth,
            expanded,
            loading,
            has_children_or_unknown,
            is_active: self
                .active_path
                .as_ref()
                .is_some_and(|active| crate::folder_tree::path_eq(active, path)),
            is_cursor: self
                .cursor_path
                .as_ref()
                .is_some_and(|cursor| crate::folder_tree::path_eq(cursor, path)),
            error,
        });
        if expanded && let Some(node) = node {
            for child in node.children() {
                self.push_visible_rows(child, depth + 1, rows, seen);
            }
        }
    }

    fn ensure_node(&mut self, path: PathBuf) {
        self.nodes
            .entry(key_for(&path))
            .or_insert_with(|| FolderPaneNode::placeholder(path));
    }
    fn is_expanded_key(&self, key: &str) -> bool {
        self.user_expanded.contains(key)
            || self.auto_expanded.contains(key) && !self.user_collapsed.contains(key)
    }
    fn cancel_pending(&mut self) {
        for node in self.nodes.values_mut() {
            node.scan = NodeScan::Unverified;
        }
    }

    fn repair_cursor_after_successful_refresh(&mut self, parent: &Path, children: &[PathBuf]) {
        let Some(cursor) = self.cursor_path.as_ref() else {
            return;
        };
        if crate::folder_tree::path_eq(cursor, parent)
            || !path_is_same_or_descendant(cursor, parent)
        {
            return;
        }
        if children
            .iter()
            .any(|child| path_is_same_or_descendant(cursor, child))
        {
            return;
        }
        let active_visible = self.active_path.as_ref().is_some_and(|active| {
            self.visible_rows()
                .iter()
                .any(|row| crate::folder_tree::path_eq(&row.path, active))
        });
        self.cursor_path = if active_visible {
            self.active_path.clone()
        } else {
            self.selected_drive.clone()
        };
        self.scroll_to_cursor = true;
    }
}

fn path_is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    path.ancestors()
        .any(|candidate| crate::folder_tree::path_eq(candidate, ancestor))
}

#[cfg(test)]
pub(crate) fn scan_real_subfolders(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<PathBuf>> {
    scan_complete_listing(
        path,
        FolderPaneListingOptions {
            sort_order,
            show_hidden_files,
        },
        cancel,
    )
    .map(|cache| cache.children)
}

fn scan_complete_listing(
    path: &Path,
    options: FolderPaneListingOptions,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<CachedListing> {
    let perf_start = crate::perf::is_enabled().then(std::time::Instant::now);
    let mut stats = FolderPaneScanStats::default();
    let result = scan_real_subfolders_inner(
        path,
        options.sort_order,
        options.show_hidden_files,
        cancel,
        &mut stats,
    );
    if let Some(start) = perf_start {
        emit_folder_pane_scan_perf(path, options.sort_order, start, &stats, &result);
    }
    result
}

#[derive(Default)]
struct FolderPaneScanStats {
    entries_seen: usize,
    dirs_returned: usize,
    entry_errors: usize,
    file_type_errors: usize,
    mtime_errors: usize,
    canceled: bool,
}

fn scan_real_subfolders_inner(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
    stats: &mut FolderPaneScanStats,
) -> std::io::Result<CachedListing> {
    let mut membership = HashSet::new();
    let mut dirs: Vec<(PathBuf, i64)> = Vec::new();
    let entries = std::fs::read_dir(path)?;
    let use_mtime = sort_order.uses_mtime();
    for entry in entries {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            stats.canceled = true;
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        stats.entries_seen += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                stats.entry_errors += 1;
                return Err(err);
            }
        };
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                stats.file_type_errors += 1;
                return Err(err);
            }
        };
        if !crate::fs_entry::try_classify_dir_entry(&entry, &file_type)?.is_directory() {
            continue;
        }
        membership.insert(key_for(&entry.path()));
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            continue;
        }
        let mtime = if use_mtime {
            match entry.metadata().and_then(|m| m.modified()) {
                Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as i64,
                    Err(_) => {
                        stats.mtime_errors += 1;
                        0
                    }
                },
                Err(_) => {
                    stats.mtime_errors += 1;
                    0
                }
            }
        } else {
            0
        };
        stats.dirs_returned += 1;
        dirs.push((entry.path(), mtime));
    }
    let mut keyed_dirs: Vec<_> = dirs
        .into_iter()
        .map(|(path, mtime)| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let key = sort_order.name_key(name);
            (path, mtime, key)
        })
        .collect();
    keyed_dirs
        .sort_by(|(_, a_mt, ak), (_, b_mt, bk)| sort_order.compare_name_keys(ak, *a_mt, bk, *b_mt));
    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
    }
    Ok(CachedListing {
        children: keyed_dirs.into_iter().map(|(path, _, _)| path).collect(),
        membership,
        options: FolderPaneListingOptions {
            sort_order,
            show_hidden_files,
        },
    })
}

fn emit_folder_pane_scan_perf(
    path: &Path,
    sort_order: FolderTreeSortOrder,
    start: std::time::Instant,
    stats: &FolderPaneScanStats,
    result: &std::io::Result<CachedListing>,
) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let use_mtime = sort_order.uses_mtime();
    let mut fields = vec![
        ("ms", serde_json::Value::from(ms)),
        ("entries_seen", serde_json::Value::from(stats.entries_seen)),
        (
            "dirs_returned",
            serde_json::Value::from(stats.dirs_returned),
        ),
        ("entry_errors", serde_json::Value::from(stats.entry_errors)),
        (
            "file_type_errors",
            serde_json::Value::from(stats.file_type_errors),
        ),
        ("mtime_errors", serde_json::Value::from(stats.mtime_errors)),
        ("canceled", serde_json::Value::from(stats.canceled)),
        ("use_mtime", serde_json::Value::from(use_mtime)),
        ("sort", serde_json::Value::from(format!("{sort_order:?}"))),
        ("ok", serde_json::Value::from(result.is_ok())),
    ];
    if let Err(err) = result {
        fields.push((
            "error_kind",
            serde_json::Value::from(format!("{:?}", err.kind())),
        ));
    }
    let key = path.to_string_lossy();
    crate::perf::event(
        "folder_pane",
        "scan_subfolders",
        Some(key.as_ref()),
        0,
        &fields,
    );
}

pub(crate) fn active_filesystem_folder(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    // 字句判定のみ (filesystem syscall なし)。`sync_to_active` はペイン表示中 **毎フレーム**
    // これを呼ぶので、ここで `Path::is_dir()` / `is_file()` を叩くと切断ネットワークドライブ等で
    // 毎フレーム GetFileAttributes が SMB タイムアウトし UI が固まる
    // (docs/ui-responsiveness.md §4 の禁止事項)。
    //
    // 入力は `App::effective_folder()`、すなわち実ディレクトリ (current_folder) か
    // ZIP/PDF/変換アーカイブを仮想フォルダ source として開いたファイルパスのいずれか。
    // 後者は拡張子で判定して親ディレクトリを返し、それ以外はディレクトリとして扱う。
    if crate::folder_tree::is_open_as_container(path)
        || crate::folder_tree::is_convertible_archive_path(path)
    {
        return path.parent().map(Path::to_path_buf);
    }
    Some(path.to_path_buf())
}

pub(crate) fn folder_label(path: &Path) -> String {
    if is_root_like(path) {
        return path.display().to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn drive_label(path: &Path) -> String {
    let display = path.display().to_string();
    if display.len() >= 2 && display.as_bytes().get(1) == Some(&b':') {
        display[..2].to_string()
    } else {
        display
    }
}

fn key_for(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

fn is_root_like(path: &Path) -> bool {
    path.parent().is_none()
        || path
            .parent()
            .is_some_and(|parent| parent.as_os_str().is_empty())
}

fn root_of_path(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let letter = (bytes[0] as char).to_ascii_uppercase();
        return Some(PathBuf::from(format!("{letter}:\\")));
    }
    if raw.starts_with('/') {
        return Some(PathBuf::from("/"));
    }
    path.ancestors().last().map(Path::to_path_buf)
}

fn ancestor_chain(root: &Path, path: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = Some(path);
    while let Some(cur) = current {
        chain.push(cur.to_path_buf());
        if crate::folder_tree::path_eq(cur, root) {
            break;
        }
        current = cur.parent();
    }
    chain.reverse();
    if chain
        .first()
        .is_none_or(|first| !crate::folder_tree::path_eq(first, root))
    {
        chain.insert(0, root.to_path_buf());
    }
    chain
}

#[cfg(test)]
#[path = "folder_pane/refresh_tests.rs"]
mod refresh_tests;

#[cfg(test)]
impl FolderPaneState {
    pub(crate) fn test_tree(root: &Path, children: Vec<PathBuf>) -> Self {
        let mut state = Self::default();
        state.selected_drive = Some(root.to_path_buf());
        state.has_focus = true;
        state.last_drive_refresh = Some(std::time::Instant::now());
        state.resume_display();
        state.test_seed_listing(root, children, true);
        state
    }

    pub(crate) fn test_set_drive_choices(&mut self, drives: Vec<PathBuf>) {
        self.drives = drives;
    }

    pub(crate) fn test_seed_listing(
        &mut self,
        path: &Path,
        children: Vec<PathBuf>,
        expanded: bool,
    ) {
        let key = key_for(path);
        let cache = CachedListing {
            membership: children.iter().map(|child| key_for(child)).collect(),
            children,
            options: self.listing_options,
        };
        self.nodes.insert(
            key.clone(),
            FolderPaneNode {
                path: path.to_path_buf(),
                cache: Some(cache),
                scan: NodeScan::Settled {
                    options: self.listing_options,
                    error: None,
                },
            },
        );
        if expanded {
            self.user_expanded.insert(key.clone());
        }
        if expanded
            || self
                .selected_drive
                .as_deref()
                .is_some_and(|root| crate::folder_tree::path_eq(root, path))
        {
            self.demand.insert(key);
        }
    }

    pub(crate) fn test_children(&self, path: &Path) -> Vec<PathBuf> {
        self.nodes
            .get(&key_for(path))
            .map_or(Vec::new(), |node| node.children().to_vec())
    }

    pub(crate) fn test_unverified(&self, path: &Path) -> bool {
        self.nodes
            .get(&key_for(path))
            .is_some_and(|node| !matches!(node.scan, NodeScan::Settled { .. }))
    }

    pub(crate) fn test_settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while self.has_pending() {
            self.poll_pending();
            assert!(
                std::time::Instant::now() < deadline,
                "pane scans did not settle"
            );
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn active_virtual_folder_maps_to_parent() {
        assert_eq!(
            active_filesystem_folder(Path::new(r"C:\books\vol.zip")),
            Some(p(r"C:\books"))
        );
        assert_eq!(
            active_filesystem_folder(Path::new(r"D:\docs\scan.pdf")),
            Some(p(r"D:\docs"))
        );
    }

    #[test]
    fn scan_real_subfolders_excludes_files_and_virtual_containers() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::write(tmp.path().join("book.zip"), b"not a real tree folder").unwrap();
        std::fs::write(tmp.path().join("doc.pdf"), b"pdf").unwrap();
        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, false, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["a", "b"]);
    }

    #[test]
    fn scan_real_subfolders_excludes_portable_metadata_bundle_even_when_showing_hidden() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("visible")).unwrap();
        std::fs::create_dir(
            tmp.path()
                .join(crate::fs_entry::PORTABLE_METADATA_BUNDLE_DIRNAME),
        )
        .unwrap();
        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, true, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["visible"]);
    }

    #[cfg(windows)]
    #[test]
    fn scan_real_subfolders_includes_windows_directory_symlink() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let target = tmp.path().join("target");
        let link = tmp.path().join("link");
        std::fs::create_dir(&target).unwrap();
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return;
        }

        let dirs =
            scan_real_subfolders(tmp.path(), FolderTreeSortOrder::NameAsc, false, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["link", "target"]);
    }
}
