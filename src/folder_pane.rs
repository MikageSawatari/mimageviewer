//! Left-side filesystem folder tree pane state.
//!
//! This pane is intentionally separate from `folder_tree`: the latter is the
//! Ctrl+Up/Down DFS navigator and treats ZIP/PDF files as virtual folders.  The
//! pane here shows only real filesystem directories.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::settings::SortOrder;

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

#[derive(Debug, Clone)]
pub(crate) struct FolderPaneNode {
    pub path: PathBuf,
    pub children: Vec<PathBuf>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl FolderPaneNode {
    fn placeholder(path: PathBuf) -> Self {
        Self {
            path,
            children: Vec::new(),
            loaded: false,
            loading: false,
            error: None,
        }
    }
}

pub(crate) struct FolderPaneScanPending {
    key: String,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Vec<PathBuf>, String>>,
}

impl Drop for FolderPaneScanPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

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
    pending: Vec<FolderPaneScanPending>,
    last_sort_order: SortOrder,
    show_hidden_files: bool,
    /// `refresh_drives` の throttle 用。ペイン表示中は `sync_to_active` から毎フレーム
    /// 呼ばれるので、`GetLogicalDrives` + 最大 26 回 `GetDriveTypeW` を間引く。
    last_drive_refresh: Option<std::time::Instant>,
}

impl Default for FolderPaneState {
    fn default() -> Self {
        let drives = crate::known_folders::available_drives();
        let selected_drive = drives.first().cloned();
        Self {
            has_focus: false,
            scroll_to_cursor: false,
            selected_drive,
            cursor_path: None,
            active_path: None,
            active_key: None,
            drives,
            nodes: HashMap::new(),
            user_expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            user_collapsed: HashSet::new(),
            pending: Vec::new(),
            last_sort_order: SortOrder::default(),
            show_hidden_files: false,
            last_drive_refresh: None,
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
        !self.pending.is_empty()
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
            if let Some(root) = self.selected_drive.clone() {
                let chain = ancestor_chain(&root, &active);
                for ancestor in chain.iter().take(chain.len().saturating_sub(1)) {
                    let key = key_for(ancestor);
                    self.auto_expanded.insert(key.clone());
                    self.user_collapsed.remove(&key);
                }
            }
            self.cursor_path = Some(active);
        }
        self.set_focus_tree();
    }

    pub(crate) fn set_focus_grid(&mut self) {
        self.has_focus = false;
    }

    /// トグルキー (キーボード T / ゲームパッド Y) でペインを閉じる時に、カーソルが
    /// 現在のアクティブフォルダと **別のフォルダ** を指していれば、その移動先パスを返す
    /// (= Enter 相当でそこへ移動して閉じる)。カーソルが動いていなければ `None`
    /// (= 単に閉じる)。
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
        // ペイン表示中は `sync_to_active` から毎フレーム呼ばれる。`available_drives()` は
        // `GetLogicalDrives` + 最大 26 回 `GetDriveTypeW` syscall + Vec alloc なので、
        // ~1.5s に throttle する (post-v1.3.0 backlog perf)。ドライブ抜き差しの反映は
        // 最大 ~1.5s 遅延するが許容範囲 (変化頻度が極小)。
        const DRIVE_REFRESH_THROTTLE: std::time::Duration = std::time::Duration::from_millis(1500);
        let now = std::time::Instant::now();
        if self
            .last_drive_refresh
            .is_some_and(|last| now.duration_since(last) < DRIVE_REFRESH_THROTTLE)
        {
            return;
        }
        self.last_drive_refresh = Some(now);

        let drives = crate::known_folders::available_drives();
        if drives == self.drives {
            return;
        }
        self.drives = drives;
        if let Some(selected) = self.selected_drive.as_ref()
            && self
                .drives
                .iter()
                .any(|drive| crate::folder_tree::path_eq(drive, selected))
        {
            return;
        }
        self.selected_drive = self.drives.first().cloned();
    }

    pub(crate) fn select_drive(&mut self, drive: PathBuf, sort_order: SortOrder) {
        self.has_focus = true;
        if self
            .selected_drive
            .as_ref()
            .is_some_and(|current| crate::folder_tree::path_eq(current, &drive))
        {
            self.cursor_path = Some(drive.clone());
            self.scroll_to_cursor = true;
            self.ensure_node(drive.clone());
            self.ensure_scan(&drive, sort_order);
            return;
        }
        self.selected_drive = Some(drive.clone());
        self.cursor_path = Some(drive.clone());
        self.scroll_to_cursor = true;
        self.ensure_node(drive.clone());
        self.ensure_scan(&drive, sort_order);
    }

    pub(crate) fn reload_for_active(
        &mut self,
        active: Option<&Path>,
        sort_order: SortOrder,
        show_hidden_files: bool,
    ) {
        self.cancel_pending();
        self.nodes.clear();
        self.user_expanded.clear();
        self.auto_expanded.clear();
        self.user_collapsed.clear();
        self.active_key = None;
        self.sync_to_active(active, sort_order, show_hidden_files);
        self.cursor_path = self
            .active_path
            .clone()
            .or_else(|| self.selected_drive.clone());
        self.scroll_to_cursor = true;
    }

    pub(crate) fn sync_to_active(
        &mut self,
        active: Option<&Path>,
        sort_order: SortOrder,
        show_hidden_files: bool,
    ) {
        self.refresh_drives();
        let sort_changed = self.last_sort_order != sort_order;
        let visibility_changed = self.show_hidden_files != show_hidden_files;
        let listing_options_changed = sort_changed || visibility_changed;
        if listing_options_changed {
            // 列挙設定変更でツリーを作り直す。展開状態 (user_expanded / auto_expanded) も
            // クリアして「nodes は消えたが展開キーだけ残る」orphan (= 展開表示なのに子を
            // ロードできない行) を防ぐ。現在のフォルダまでの祖先チェーンは下で
            // auto_expanded に再構築されるので、現在地までの展開は維持される。
            self.cancel_pending();
            self.nodes.clear();
            self.user_expanded.clear();
            self.auto_expanded.clear();
            self.user_collapsed.clear();
            self.last_sort_order = sort_order;
            self.show_hidden_files = show_hidden_files;
        }

        let active_folder = active.and_then(active_filesystem_folder);
        let new_active_key = active_folder.as_ref().map(|p| key_for(p));
        let active_changed = self.active_key != new_active_key;
        if active_changed {
            self.auto_expanded.clear();
            self.user_collapsed.clear();
            self.active_key = new_active_key.clone();
            if !self.has_focus || self.cursor_path.is_none() {
                self.cursor_path = active_folder.clone();
                self.scroll_to_cursor = true;
            }
        }
        self.active_path = active_folder.clone();

        if let Some(active_folder) = active_folder {
            if let Some(root) = root_of_path(&active_folder) {
                let sync_selected_drive = !self.has_focus
                    || self
                        .selected_drive
                        .as_ref()
                        .is_none_or(|drive| crate::folder_tree::path_eq(drive, &root));
                if sync_selected_drive {
                    self.selected_drive = Some(root.clone());
                    self.ensure_node(root.clone());
                    let chain = ancestor_chain(&root, &active_folder);
                    for ancestor in chain.iter().take(chain.len().saturating_sub(1)) {
                        self.auto_expanded.insert(key_for(ancestor));
                        self.ensure_node(ancestor.clone());
                    }
                    self.ensure_node(active_folder);
                } else if let Some(drive) = self.selected_drive.clone() {
                    self.ensure_node(drive);
                }
            }
        } else if let Some(drive) = self.selected_drive.clone() {
            self.ensure_node(drive);
        }

        // 列挙設定変更直後は、作り直したツリーで現在のフォルダまでスクロールし直す
        // (= ESC でグリッド→ツリーへ抜けたときの `set_focus_tree_at_active` と同じ
        //  「現在地へ追従」挙動)。フォーカスは奪わない (グリッド操作中のソート変更で
        //  ツリーに focus が飛ばないように、scroll だけ要求する)。
        if listing_options_changed && let Some(active) = self.active_path.clone() {
            self.cursor_path = Some(active);
            self.scroll_to_cursor = true;
        }

        self.ensure_scans_for_expanded(sort_order);
    }

    pub(crate) fn poll_pending(&mut self) -> bool {
        let mut changed = false;
        let mut idx = 0;
        while idx < self.pending.len() {
            match self.pending[idx].rx.try_recv() {
                Ok(result) => {
                    let pending = self.pending.swap_remove(idx);
                    if let Some(node) = self.nodes.get_mut(&pending.key) {
                        node.loading = false;
                        node.loaded = result.is_ok();
                        match result {
                            Ok(children) => {
                                node.children = children;
                                node.error = None;
                            }
                            Err(err) => {
                                node.children.clear();
                                node.error = Some(err);
                            }
                        }
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    idx += 1;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let pending = self.pending.swap_remove(idx);
                    if let Some(node) = self.nodes.get_mut(&pending.key) {
                        node.loading = false;
                        node.loaded = false;
                        node.children.clear();
                        node.error = Some("列挙に失敗しました".to_string());
                    }
                    changed = true;
                }
            }
        }
        changed
    }

    pub(crate) fn visible_rows(&self) -> Vec<FolderPaneRow> {
        let Some(root) = self.selected_drive.as_ref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        self.push_visible_rows(root, 0, &mut rows, &mut seen);
        rows
    }

    pub(crate) fn set_cursor(&mut self, path: PathBuf) {
        self.cursor_path = Some(path);
        self.scroll_to_cursor = true;
    }

    pub(crate) fn handle_tree_key(
        &mut self,
        key: FolderPaneTreeKey,
        sort_order: SortOrder,
    ) -> Option<FolderPaneCommand> {
        match key {
            FolderPaneTreeKey::Up => {
                self.move_cursor(-1);
                None
            }
            FolderPaneTreeKey::Down => {
                self.move_cursor(1);
                None
            }
            FolderPaneTreeKey::Left => {
                self.collapse_cursor();
                None
            }
            FolderPaneTreeKey::Right => {
                self.expand_cursor(sort_order);
                None
            }
            FolderPaneTreeKey::Enter => self.cursor_path.clone().map(FolderPaneCommand::Open),
        }
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
            .unwrap_or_else(|| {
                self.active_path
                    .as_ref()
                    .and_then(|active| {
                        rows.iter()
                            .position(|row| crate::folder_tree::path_eq(&row.path, active))
                    })
                    .unwrap_or(0)
            });
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(rows.len() - 1)
        };
        self.cursor_path = Some(rows[next].path.clone());
        self.scroll_to_cursor = true;
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
            self.scroll_to_cursor = true;
            return;
        }
        if let Some(parent) = cursor.parent() {
            if !parent.as_os_str().is_empty() {
                self.cursor_path = Some(parent.to_path_buf());
                self.scroll_to_cursor = true;
            }
        }
    }

    fn expand_cursor(&mut self, sort_order: SortOrder) {
        let Some(cursor) = self.cursor_path.clone() else {
            return;
        };
        let key = key_for(&cursor);
        if !self.is_expanded_key(&key) {
            self.user_expanded.insert(key.clone());
            self.user_collapsed.remove(&key);
            self.ensure_node(cursor.clone());
            self.ensure_scan(&cursor, sort_order);
            self.scroll_to_cursor = true;
            return;
        }
        if let Some(first_child) = self
            .nodes
            .get(&key)
            .and_then(|node| node.children.first())
            .cloned()
        {
            self.cursor_path = Some(first_child);
            self.scroll_to_cursor = true;
        }
    }

    fn push_visible_rows(
        &self,
        path: &Path,
        depth: usize,
        rows: &mut Vec<FolderPaneRow>,
        seen: &mut HashSet<String>,
    ) {
        // junction / symlink ループや異常に深い階層で render が無限/過大に descend する
        // のを防ぐ防御的上限 (post-v1.3.0 backlog 堅牢化)。`seen` は正規化パス文字列なので
        // 別字句パスから同じ実体へ到達する reparse-point ループは捕捉できない (= 手動展開で
        // 無限に降りられる)。この上限で render 深度を頭打ちにし、上限超の行は描画しない
        // ので、それ以上の手動展開 (= クリック対象の行) も発生しない。通常のフォルダ階層は
        // この値に遠く及ばない。
        const MAX_TREE_DEPTH: usize = 64;
        if depth > MAX_TREE_DEPTH {
            return;
        }
        let key = key_for(path);
        if !seen.insert(key.clone()) {
            return;
        }
        let expanded = self.is_expanded_key(&key);
        let node = self.nodes.get(&key);
        let loading = node.is_some_and(|n| n.loading);
        let error = node.and_then(|n| n.error.clone());
        let has_children_or_unknown = node
            .map(|n| !n.loaded || !n.children.is_empty())
            .unwrap_or(true);
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
        if !expanded {
            return;
        }
        if let Some(node) = node {
            for child in &node.children {
                self.push_visible_rows(child, depth + 1, rows, seen);
            }
        }
    }

    fn ensure_scans_for_expanded(&mut self, sort_order: SortOrder) {
        let mut paths = Vec::new();
        for key in self
            .user_expanded
            .union(&self.auto_expanded)
            .cloned()
            .collect::<Vec<_>>()
        {
            if self.user_collapsed.contains(&key) {
                continue;
            }
            if let Some(path) = self.nodes.get(&key).map(|node| node.path.clone()) {
                paths.push(path);
            }
        }
        if let Some(root) = self.selected_drive.clone() {
            paths.push(root);
        }
        for path in paths {
            self.ensure_scan(&path, sort_order);
        }
    }

    fn ensure_node(&mut self, path: PathBuf) {
        let key = key_for(&path);
        self.nodes
            .entry(key)
            .or_insert_with(|| FolderPaneNode::placeholder(path));
    }

    fn ensure_scan(&mut self, path: &Path, sort_order: SortOrder) {
        let key = key_for(path);
        self.ensure_node(path.to_path_buf());
        let Some(node) = self.nodes.get_mut(&key) else {
            return;
        };
        if node.loaded || node.loading {
            return;
        }
        node.loading = true;
        node.error = None;

        let scan_path = path.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let show_hidden_files = self.show_hidden_files;
        let spawn_result = std::thread::Builder::new()
            .name("folder-pane-scan".to_string())
            .spawn(move || {
                let result = scan_real_subfolders(
                    &scan_path,
                    sort_order,
                    show_hidden_files,
                    Some(&cancel_w),
                )
                .map_err(|err| err.to_string());
                if !cancel_w.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                }
            });
        if let Err(err) = spawn_result {
            if let Some(node) = self.nodes.get_mut(&key) {
                node.loading = false;
                node.error = Some(format!("列挙スレッドを開始できません: {err}"));
            }
            return;
        }
        self.pending.push(FolderPaneScanPending { key, cancel, rx });
    }

    fn is_expanded_key(&self, key: &str) -> bool {
        self.user_expanded.contains(key)
            || (self.auto_expanded.contains(key) && !self.user_collapsed.contains(key))
    }

    fn cancel_pending(&mut self) {
        for pending in &self.pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.pending.clear();
    }
}

pub(crate) fn scan_real_subfolders(
    path: &Path,
    sort_order: SortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<PathBuf>> {
    let perf_start = crate::perf::is_enabled().then(std::time::Instant::now);
    let mut stats = FolderPaneScanStats::default();
    let result =
        scan_real_subfolders_inner(path, sort_order, show_hidden_files, cancel, &mut stats);
    if let Some(start) = perf_start {
        emit_folder_pane_scan_perf(path, sort_order, start, &stats, &result);
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
    sort_order: SortOrder,
    show_hidden_files: bool,
    cancel: Option<&AtomicBool>,
    stats: &mut FolderPaneScanStats,
) -> std::io::Result<Vec<PathBuf>> {
    let mut dirs: Vec<(PathBuf, i64)> = Vec::new();
    let entries = std::fs::read_dir(path)?;
    let use_mtime = matches!(sort_order, SortOrder::DateAsc | SortOrder::DateDesc);
    for entry in entries {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            stats.canceled = true;
            break;
        }
        stats.entries_seen += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                stats.entry_errors += 1;
                continue;
            }
        };
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                stats.file_type_errors += 1;
                continue;
            }
        };
        if !crate::fs_entry::classify_dir_entry(&entry, &file_type).is_directory() {
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
    Ok(keyed_dirs.into_iter().map(|(path, _, _)| path).collect())
}

fn emit_folder_pane_scan_perf(
    path: &Path,
    sort_order: SortOrder,
    start: std::time::Instant,
    stats: &FolderPaneScanStats,
    result: &std::io::Result<Vec<PathBuf>>,
) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let use_mtime = matches!(sort_order, SortOrder::DateAsc | SortOrder::DateDesc);
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
    fn sync_to_active_expands_minimum_ancestor_chain() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b\c")), SortOrder::FileName, false);
        let root_key = key_for(Path::new(r"C:\"));
        let a_key = key_for(Path::new(r"C:\a"));
        let b_key = key_for(Path::new(r"C:\a\b"));
        let c_key = key_for(Path::new(r"C:\a\b\c"));
        assert!(state.auto_expanded.contains(&root_key));
        assert!(state.auto_expanded.contains(&a_key));
        assert!(state.auto_expanded.contains(&b_key));
        assert!(!state.auto_expanded.contains(&c_key));
        assert_eq!(state.active_path(), Some(Path::new(r"C:\a\b\c")));
    }

    #[test]
    fn auto_branch_is_replaced_but_user_expansion_persists() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, false);
        state.user_expanded.insert(key_for(Path::new(r"C:\manual")));
        state.ensure_node(p(r"C:\manual"));
        state.sync_to_active(Some(Path::new(r"C:\x\y")), SortOrder::FileName, false);
        assert!(!state.auto_expanded.contains(&key_for(Path::new(r"C:\a"))));
        assert!(state.auto_expanded.contains(&key_for(Path::new(r"C:\x"))));
        assert!(
            state
                .user_expanded
                .contains(&key_for(Path::new(r"C:\manual")))
        );
    }

    #[test]
    fn cursor_nav_target_only_when_cursor_moved_off_active() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, false);
        // 開いた直後はカーソル = アクティブなので移動先なし (= 単に閉じる)。
        assert_eq!(state.cursor_nav_target_if_moved(), None);
        // カーソルを別フォルダへ動かすと、その移動先を返す (= Enter 相当で移動)。
        state.cursor_path = Some(p(r"C:\a\c"));
        assert_eq!(state.cursor_nav_target_if_moved(), Some(p(r"C:\a\c")));
    }

    #[test]
    fn sort_change_resets_expansion_and_scrolls_to_active() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, false);
        // ユーザーが現在地と無関係な枝を手動展開している状態を作る。
        state.user_expanded.insert(key_for(Path::new(r"C:\manual")));
        state.ensure_node(p(r"C:\manual"));
        state.scroll_to_cursor = false;

        // ソート順を変更すると作り直しが走る (active は同じ C:\a\b)。
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::DateDesc, false);

        // 手動展開は捨てられ orphan 行を残さない。
        assert!(
            !state
                .user_expanded
                .contains(&key_for(Path::new(r"C:\manual")))
        );
        // 現在地までの祖先チェーンは再構築される。
        assert!(state.auto_expanded.contains(&key_for(Path::new(r"C:\a"))));
        // そして現在のフォルダへスクロールし直す (= 現在地を見失わない)。
        assert!(state.scroll_to_cursor);
        assert_eq!(state.cursor_path.as_deref(), Some(Path::new(r"C:\a\b")));
    }

    #[test]
    fn collapse_auto_expanded_branch_hides_it_until_active_changes() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, false);
        state.cursor_path = Some(p(r"C:\a"));
        state.collapse_cursor();
        assert!(state.user_collapsed.contains(&key_for(Path::new(r"C:\a"))));
        assert!(!state.is_expanded_key(&key_for(Path::new(r"C:\a"))));
        state.sync_to_active(Some(Path::new(r"C:\a\c")), SortOrder::FileName, false);
        assert!(!state.user_collapsed.contains(&key_for(Path::new(r"C:\a"))));
        assert!(state.is_expanded_key(&key_for(Path::new(r"C:\a"))));
    }

    #[test]
    fn hidden_visibility_change_rebuilds_tree_cache() {
        let mut state = FolderPaneState::default();
        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, false);
        state.user_expanded.insert(key_for(Path::new(r"C:\manual")));
        state.ensure_node(p(r"C:\manual"));
        state.scroll_to_cursor = false;

        state.sync_to_active(Some(Path::new(r"C:\a\b")), SortOrder::FileName, true);

        assert!(state.show_hidden_files);
        assert!(
            !state
                .user_expanded
                .contains(&key_for(Path::new(r"C:\manual")))
        );
        assert!(state.auto_expanded.contains(&key_for(Path::new(r"C:\a"))));
        assert!(state.scroll_to_cursor);
    }

    #[test]
    fn keyboard_moves_visible_rows_and_enter_opens_cursor() {
        let mut state = FolderPaneState::default();
        let root = p(r"C:\");
        let a = p(r"C:\a");
        let b = p(r"C:\b");
        state.selected_drive = Some(root.clone());
        state.nodes.insert(
            key_for(&root),
            FolderPaneNode {
                path: root.clone(),
                children: vec![a.clone(), b.clone()],
                loaded: true,
                loading: false,
                error: None,
            },
        );
        state
            .nodes
            .insert(key_for(&a), FolderPaneNode::placeholder(a.clone()));
        state
            .nodes
            .insert(key_for(&b), FolderPaneNode::placeholder(b.clone()));
        state.user_expanded.insert(key_for(&root));
        state.cursor_path = Some(root);
        state.handle_tree_key(FolderPaneTreeKey::Down, SortOrder::FileName);
        assert_eq!(state.cursor_path.as_deref(), Some(a.as_path()));
        let command = state.handle_tree_key(FolderPaneTreeKey::Enter, SortOrder::FileName);
        assert_eq!(command, Some(FolderPaneCommand::Open(a)));
    }

    #[test]
    fn scan_real_subfolders_excludes_files_and_virtual_containers() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::write(tmp.path().join("book.zip"), b"not a real tree folder").unwrap();
        std::fs::write(tmp.path().join("doc.pdf"), b"pdf").unwrap();
        let dirs = scan_real_subfolders(tmp.path(), SortOrder::FileName, false, None).unwrap();
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
        let dirs = scan_real_subfolders(tmp.path(), SortOrder::FileName, true, None).unwrap();
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

        let dirs = scan_real_subfolders(tmp.path(), SortOrder::FileName, false, None).unwrap();
        let labels: Vec<_> = dirs.iter().map(|path| folder_label(path)).collect();
        assert_eq!(labels, vec!["link", "target"]);
    }
}
