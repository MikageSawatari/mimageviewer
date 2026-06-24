//! サブフォルダ展開ビュー (snapshot flat view) の App グルーと走査ワーカー。
//!
//! 現在フォルダ以下の実ファイル画像/動画だけを、その時点のスナップショットとして
//! synthetic path に流し込む。ZIP/PDF/変換アーカイブの内部展開や watcher 追従は
//! 初期版では扱わない。

use super::*;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const MAX_SUBFOLDER_EXPANSION_DEPTH: u32 = 64;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(crate) struct SubfolderExpansionOptions {
    skip_image_if_video_exists: bool,
    skip_duplicate_images: bool,
    video_thumb_use_sidecar_image: bool,
    image_ext_priority: Vec<String>,
}

impl From<&crate::settings::Settings> for SubfolderExpansionOptions {
    fn from(settings: &crate::settings::Settings) -> Self {
        Self {
            skip_image_if_video_exists: settings.skip_image_if_video_exists,
            skip_duplicate_images: settings.skip_duplicate_images,
            video_thumb_use_sidecar_image: settings.video_thumb_use_sidecar_image,
            image_ext_priority: settings.image_ext_priority.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SubfolderExpansionDiag {
    pub(crate) dirs_scanned: usize,
    pub(crate) media_found: usize,
    pub(crate) read_dir_errors: usize,
    pub(crate) entry_errors: usize,
    pub(crate) file_type_errors: usize,
    pub(crate) metadata_errors: usize,
    pub(crate) depth_limit_hits: usize,
    pub(crate) visited_skips: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SubfolderExpansionProgress {
    pub(crate) dirs_scanned: usize,
    pub(crate) media_found: usize,
    pub(crate) current_dir: Option<PathBuf>,
}

impl SubfolderExpansionProgress {
    fn from_diag(diag: &SubfolderExpansionDiag, current_dir: Option<PathBuf>) -> Self {
        Self {
            dirs_scanned: diag.dirs_scanned,
            media_found: diag.media_found,
            current_dir,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SubfolderExpansionEntry {
    pub(crate) path: PathBuf,
    pub(crate) is_video: bool,
    pub(crate) mtime: i64,
    pub(crate) file_size: i64,
}

#[derive(Debug)]
pub(crate) struct SubfolderExpansionResult {
    pub(crate) root: PathBuf,
    pub(crate) entries: Vec<SubfolderExpansionEntry>,
    /// 動画パスの正規化キー -> 同名 sidecar 画像パス。
    ///
    /// 通常フォルダの動画 override は stem キーだが、サブ展開は複数フォルダが混在するため
    /// full-path キーにする。動画ワーカー側は full-path キーを優先して参照する。
    pub(crate) video_thumb_overrides: HashMap<String, PathBuf>,
    pub(crate) diag: SubfolderExpansionDiag,
}

#[derive(Clone, Debug)]
pub(crate) struct SubfolderExpansionSnapshot {
    pub(crate) root: PathBuf,
    pub(crate) entries: Vec<SubfolderExpansionEntry>,
    pub(crate) video_thumb_overrides: HashMap<String, PathBuf>,
    pub(crate) diag: SubfolderExpansionDiag,
}

pub(crate) enum SubfolderExpansionEvent {
    Progress(SubfolderExpansionProgress),
    Done(SubfolderExpansionResult),
    Cancelled,
}

pub(crate) struct SubfolderExpansionPending {
    pub(crate) root: PathBuf,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) rx: mpsc::Receiver<SubfolderExpansionEvent>,
}

impl SubfolderExpansionPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn_subfolder_expansion_worker(
    root: PathBuf,
    options: SubfolderExpansionOptions,
) -> Result<SubfolderExpansionPending, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let root_w = root.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("subfolder-expansion".into())
        .spawn(move || {
            let event = match scan_subfolder_expansion(root_w, options, Arc::clone(&cancel_w), &tx)
            {
                Some(_result) if cancel_w.load(Ordering::Relaxed) => {
                    SubfolderExpansionEvent::Cancelled
                }
                Some(result) => SubfolderExpansionEvent::Done(result),
                None => SubfolderExpansionEvent::Cancelled,
            };
            let _ = tx.send(event);
        })
        .map_err(|e| format!("サブ展開ワーカーを起動できませんでした: {e}"))?;

    Ok(SubfolderExpansionPending { root, cancel, rx })
}

fn scan_subfolder_expansion(
    root: PathBuf,
    options: SubfolderExpansionOptions,
    cancel: Arc<AtomicBool>,
    tx: &mpsc::Sender<SubfolderExpansionEvent>,
) -> Option<SubfolderExpansionResult> {
    let mut result = SubfolderExpansionResult {
        root: root.clone(),
        entries: Vec::new(),
        video_thumb_overrides: HashMap::new(),
        diag: SubfolderExpansionDiag::default(),
    };
    let mut visited = HashSet::new();
    let mut stack = vec![(root, 0_u32)];
    let mut last_progress = Instant::now();

    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if depth > MAX_SUBFOLDER_EXPANSION_DEPTH {
            result.diag.depth_limit_hits += 1;
            continue;
        }
        if !crate::fs_entry::mark_directory_visited(&dir, &mut visited) {
            result.diag.visited_skips += 1;
            continue;
        }

        let mut subdirs = Vec::new();
        scan_one_directory(&dir, &options, &cancel, &mut result, &mut subdirs);
        if cancel.load(Ordering::Relaxed) {
            return None;
        }

        result.diag.dirs_scanned += 1;
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            let _ = tx.send(SubfolderExpansionEvent::Progress(
                SubfolderExpansionProgress::from_diag(&result.diag, Some(dir.clone())),
            ));
            last_progress = Instant::now();
        }

        subdirs.sort_by(|a, b| path_name_for_sort(a).cmp(&path_name_for_sort(b)));
        for subdir in subdirs.into_iter().rev() {
            stack.push((subdir, depth + 1));
        }
    }

    let _ = tx.send(SubfolderExpansionEvent::Progress(
        SubfolderExpansionProgress::from_diag(&result.diag, None),
    ));
    Some(result)
}

fn scan_one_directory(
    dir: &Path,
    options: &SubfolderExpansionOptions,
    cancel: &AtomicBool,
    result: &mut SubfolderExpansionResult,
    subdirs: &mut Vec<PathBuf>,
) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            result.diag.read_dir_errors += 1;
            return;
        }
    };

    let mut media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();
    let mut entry_file_names_ci = HashSet::new();

    for entry_result in read_dir {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => {
                result.diag.entry_errors += 1;
                continue;
            }
        };
        entry_file_names_ci.insert(entry.file_name().to_string_lossy().to_lowercase());

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                result.diag.file_type_errors += 1;
                continue;
            }
        };
        let kind = crate::fs_entry::classify_dir_entry(&entry, &file_type);
        let path = entry.path();

        if kind.is_directory() {
            if !crate::video::upscale::paths::has_work_dir_suffix(&path) {
                subdirs.push(path);
            }
            continue;
        }
        if crate::folder_tree::is_apple_double(&path) || !kind.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext_lower = ext.to_ascii_lowercase();
        let is_video = if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
            false
        } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
            true
        } else {
            continue;
        };

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                result.diag.metadata_errors += 1;
                continue;
            }
        };
        media.push((
            path,
            is_video,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
        ));
    }

    super::folder_scan::filter_upscaled_video_pairs_fast(&mut media, &entry_file_names_ci);
    apply_duplicate_filters_to_media(&mut media, options, &mut result.video_thumb_overrides);
    result.diag.media_found += media.len();
    result
        .entries
        .extend(media.into_iter().map(|(path, is_video, mtime, file_size)| {
            SubfolderExpansionEntry {
                path,
                is_video,
                mtime,
                file_size,
            }
        }));
}

fn apply_duplicate_filters_to_media(
    media: &mut Vec<(PathBuf, bool, i64, i64)>,
    options: &SubfolderExpansionOptions,
    video_thumb_overrides: &mut HashMap<String, PathBuf>,
) {
    if options.skip_image_if_video_exists {
        filter_video_image_duplicates_for_subfolder(
            media,
            options.video_thumb_use_sidecar_image,
            video_thumb_overrides,
        );
    }
    if options.skip_duplicate_images {
        filter_image_ext_duplicates(media, &options.image_ext_priority);
    }
}

fn filter_video_image_duplicates_for_subfolder(
    media: &mut Vec<(PathBuf, bool, i64, i64)>,
    use_sidecar: bool,
    video_thumb_overrides: &mut HashMap<String, PathBuf>,
) {
    let mut videos_by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (path, is_video, _, _) in media.iter() {
        if *is_video {
            videos_by_stem
                .entry(stem_lower_local(path))
                .or_default()
                .push(path.clone());
        }
    }
    if videos_by_stem.is_empty() {
        return;
    }

    if use_sidecar {
        for (path, is_video, _, _) in media.iter() {
            if *is_video {
                continue;
            }
            let stem = stem_lower_local(path);
            let Some(videos) = videos_by_stem.get(&stem) else {
                continue;
            };
            for video in videos {
                video_thumb_overrides
                    .insert(crate::path_key::normalize_keep_drive(video), path.clone());
            }
        }
    }

    media.retain(|(path, is_video, _, _)| {
        *is_video || !videos_by_stem.contains_key(&stem_lower_local(path))
    });
}

fn filter_image_ext_duplicates(media: &mut Vec<(PathBuf, bool, i64, i64)>, priority: &[String]) {
    let mut best: HashMap<String, (usize, usize)> = HashMap::new();
    for (i, (path, is_video, _, _)) in media.iter().enumerate() {
        if *is_video {
            continue;
        }
        let stem = stem_lower_local(path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let prio = priority
            .iter()
            .position(|candidate| candidate == &ext)
            .unwrap_or(usize::MAX);
        match best.get(&stem) {
            Some(&(existing_prio, _)) if prio >= existing_prio => {}
            _ => {
                best.insert(stem, (prio, i));
            }
        }
    }

    let mut stem_counts: HashMap<String, usize> = HashMap::new();
    for (path, is_video, _, _) in media.iter() {
        if !*is_video {
            *stem_counts.entry(stem_lower_local(path)).or_insert(0) += 1;
        }
    }
    let keep_indices: HashSet<usize> = best
        .iter()
        .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
        .map(|(_, &(_, idx))| idx)
        .collect();
    if keep_indices.is_empty() {
        return;
    }

    let mut i = 0;
    media.retain(|(path, is_video, _, _)| {
        let current_i = i;
        i += 1;
        if *is_video {
            return true;
        }
        let stem = stem_lower_local(path);
        if stem_counts.get(&stem).copied().unwrap_or(0) <= 1 {
            return true;
        }
        keep_indices.contains(&current_i)
    });
}

pub(crate) fn sort_entries_for_view(
    entries: Vec<SubfolderExpansionEntry>,
    sort: crate::settings::SortOrder,
    root: &Path,
) -> Vec<SubfolderExpansionEntry> {
    let mut keyed: Vec<_> = entries
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let name_key = sort.name_key(name);
            let parent_key = entry
                .path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
                .map(|p| p.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            (entry, name_key, parent_key)
        })
        .collect();
    keyed.sort_by(|(a, ak, ap), (b, bk, bp)| {
        sort.compare_name_keys(ak, a.mtime, bk, b.mtime)
            .then_with(|| ap.cmp(bp))
            .then_with(|| a.path.cmp(&b.path))
    });
    keyed.into_iter().map(|(entry, _, _)| entry).collect()
}

fn stem_lower_local(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default()
}

fn path_name_for_sort(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_lowercase)
        .unwrap_or_else(|| path.to_string_lossy().to_lowercase())
}

impl App {
    pub(crate) fn subfolder_expansion_available(&self) -> bool {
        self.current_folder_last_mtime.is_some()
            && self.current_folder.is_some()
            && self.zip_nav.is_none()
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && !self.items_are_reading_history_view
            && !self.items_are_rating_view
            && !self.items_are_drive_list
            && !self.global_search.active
            && !self.favsearch.active
            && !self.tag_view.active
            && !self.show_search_bar
            && self.search_filter.is_none()
            && self.search_pending.is_none()
            && !self.is_snapshot_active()
    }

    pub(crate) fn subfolder_expansion_on(&self) -> bool {
        self.items_are_subfolder_expansion_view
    }

    pub(crate) fn subfolder_expansion_pending_label(&self) -> Option<String> {
        let pending = self.subfolder_expansion_pending.as_ref()?;
        let progress = self.subfolder_expansion_progress.as_ref();
        Some(match progress {
            Some(progress) => {
                format!(
                    "サブ展開中 {}件 / {}フォルダ",
                    progress.media_found, progress.dirs_scanned
                )
            }
            None => format!("サブ展開中: {}", pending.root.display()),
        })
    }

    pub(crate) fn subfolder_expansion_pending_tooltip(&self) -> Option<String> {
        let pending = self.subfolder_expansion_pending.as_ref()?;
        let progress = self.subfolder_expansion_progress.as_ref();
        Some(match progress {
            Some(progress) => {
                let current = progress
                    .current_dir
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if current.is_empty() {
                    format!(
                        "サブフォルダを走査中\n{}件 / {}フォルダ\n中止ボタンでキャンセル",
                        progress.media_found, progress.dirs_scanned
                    )
                } else {
                    format!(
                        "サブフォルダを走査中\n{}件 / {}フォルダ\n現在: {current}\n中止ボタンでキャンセル",
                        progress.media_found, progress.dirs_scanned
                    )
                }
            }
            None => format!("サブ展開中: {}", pending.root.display()),
        })
    }

    pub(crate) fn toggle_subfolder_expansion_view(&mut self) {
        if self.subfolder_expansion_pending.is_some() {
            let should_return_to_root = self.items_are_subfolder_expansion_view
                || self.current_folder.as_ref().is_some_and(|cur| {
                    crate::folder_tree::path_eq(cur, &subfolder_expansion_synthetic_path())
                });
            let return_target = self
                .subfolder_expansion_saved_folder
                .clone()
                .or_else(|| self.subfolder_expansion_root.clone());
            self.cancel_subfolder_expansion_pending();
            self.clear_subfolder_expansion_view_state();
            if should_return_to_root {
                if let Some(target) = return_target {
                    self.load_folder(target);
                }
            } else if let Some(current) = self.current_folder.clone() {
                self.address = self
                    .book_address_label_for_path(&current)
                    .unwrap_or_else(|| current.to_string_lossy().to_string());
            }
            self.show_feedback_toast("サブ展開をキャンセルしました".into());
            return;
        }
        if self.items_are_subfolder_expansion_view {
            self.exit_subfolder_expansion_view();
            return;
        }
        let Some(root) = self.current_folder.clone() else {
            self.show_feedback_toast("サブ展開は通常フォルダでのみ使えます".into());
            return;
        };
        if !self.subfolder_expansion_available() {
            self.show_feedback_toast("サブ展開は通常フォルダでのみ使えます".into());
            return;
        }
        self.start_subfolder_expansion_scan(root);
    }

    pub(crate) fn start_subfolder_expansion_scan(&mut self, root: PathBuf) {
        self.cancel_subfolder_expansion_pending();
        self.cancel_pending_folder_nav();
        self.cancel_stack_script_pending();
        self.stack_mode_requested = false;
        self.stack_view = None;
        self.stack_showing_flat = false;
        self.subfolder_expansion_root = Some(root.clone());
        self.subfolder_expansion_saved_folder = Some(root.clone());
        self.subfolder_expansion_snapshot = None;
        self.subfolder_expansion_progress = Some(SubfolderExpansionProgress::default());
        self.subfolder_expansion_diag = None;
        self.address = format!("サブ展開中: {}", root.display());

        match spawn_subfolder_expansion_worker(
            root.clone(),
            SubfolderExpansionOptions::from(&self.settings),
        ) {
            Ok(pending) => {
                self.subfolder_expansion_pending = Some(pending);
            }
            Err(message) => {
                self.clear_subfolder_expansion_view_state();
                self.show_feedback_toast(message);
            }
        }
    }

    pub(crate) fn cancel_subfolder_expansion_pending(&mut self) {
        if let Some(pending) = self.subfolder_expansion_pending.take() {
            pending.cancel();
        }
        self.subfolder_expansion_progress = None;
    }

    pub(crate) fn clear_subfolder_expansion_view_state(&mut self) {
        self.items_are_subfolder_expansion_view = false;
        self.subfolder_expansion_root = None;
        self.subfolder_expansion_saved_folder = None;
        self.subfolder_expansion_snapshot = None;
        self.subfolder_expansion_progress = None;
        self.subfolder_expansion_diag = None;
    }

    pub(crate) fn exit_subfolder_expansion_view(&mut self) {
        let target = self
            .subfolder_expansion_saved_folder
            .clone()
            .or_else(|| self.subfolder_expansion_root.clone());
        self.clear_subfolder_expansion_view_state();
        if let Some(target) = target {
            self.load_folder(target);
        }
    }

    pub(crate) fn subfolder_expansion_back_nav(&self) -> Option<crate::ui_main::AddressBarNav> {
        if !self.items_are_subfolder_expansion_view {
            return None;
        }
        self.subfolder_expansion_saved_folder
            .clone()
            .or_else(|| self.subfolder_expansion_root.clone())
            .map(crate::ui_main::AddressBarNav::Direct)
    }

    pub(crate) fn poll_subfolder_expansion(&mut self, ctx: &egui::Context) {
        if self.subfolder_expansion_pending.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        loop {
            let event = {
                let Some(pending) = self.subfolder_expansion_pending.as_ref() else {
                    return;
                };
                pending.rx.try_recv()
            };
            match event {
                Ok(SubfolderExpansionEvent::Progress(progress)) => {
                    self.subfolder_expansion_progress = Some(progress);
                }
                Ok(SubfolderExpansionEvent::Done(result)) => {
                    let Some(pending) = self.subfolder_expansion_pending.take() else {
                        return;
                    };
                    if crate::folder_tree::path_eq(&pending.root, &result.root) {
                        self.apply_subfolder_expansion_result(result);
                    }
                    return;
                }
                Ok(SubfolderExpansionEvent::Cancelled) => {
                    self.subfolder_expansion_pending = None;
                    self.subfolder_expansion_progress = None;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.subfolder_expansion_pending = None;
                    self.subfolder_expansion_progress = None;
                    self.show_feedback_toast("サブ展開の走査が中断されました".into());
                    return;
                }
            }
        }
    }

    fn apply_subfolder_expansion_result(&mut self, result: SubfolderExpansionResult) {
        let SubfolderExpansionResult {
            root,
            entries,
            video_thumb_overrides,
            diag,
        } = result;
        let snapshot = SubfolderExpansionSnapshot {
            root,
            entries,
            video_thumb_overrides,
            diag,
        };
        self.install_subfolder_expansion_snapshot(snapshot, true);
    }

    pub(crate) fn reinstall_subfolder_expansion_snapshot(&mut self) -> bool {
        let Some(snapshot) = self.subfolder_expansion_snapshot.clone() else {
            return false;
        };
        self.install_subfolder_expansion_snapshot(snapshot, false);
        true
    }

    fn install_subfolder_expansion_snapshot(
        &mut self,
        snapshot: SubfolderExpansionSnapshot,
        show_toast: bool,
    ) {
        let root = snapshot.root.clone();
        let diag = snapshot.diag.clone();
        let video_thumb_overrides = snapshot.video_thumb_overrides.clone();
        let sorted = sort_entries_for_view(
            snapshot.entries.clone(),
            self.book_sort_order_for_path(&root),
            &root,
        );
        let mut items = Vec::with_capacity(sorted.len());
        let mut image_metas = Vec::with_capacity(sorted.len());
        let mut video_items = Vec::new();

        for entry in sorted {
            let idx = items.len();
            let mtime = entry.mtime;
            let file_size = entry.file_size;
            if entry.is_video {
                items.push(GridItem::Video(entry.path.clone()));
                video_items.push((idx, entry.path, file_size.max(0) as u64));
            } else {
                items.push(GridItem::Image(entry.path));
            }
            image_metas.push(Some((mtime, file_size)));
        }

        self.video_thumb_overrides.clear();
        self.video_thumb_overrides.extend(video_thumb_overrides);
        let existing_keys: HashSet<String> = items
            .iter()
            .zip(image_metas.iter())
            .flat_map(|(item, meta)| {
                folder_thumb_existing_keys_for(
                    item,
                    *meta,
                    &HashMap::new(),
                    self.folder_thumb_pin_db.as_deref(),
                    Some(self.settings.folder_thumb_sort),
                    self.settings.folder_thumb_depth,
                    true,
                )
            })
            .collect();

        let item_count = items.len();
        self.start_loading_items(
            subfolder_expansion_synthetic_path(),
            items,
            image_metas,
            existing_keys,
            video_items,
            None,
        );
        self.items_are_subfolder_expansion_view = true;
        self.subfolder_expansion_root = Some(root.clone());
        self.subfolder_expansion_saved_folder = Some(root.clone());
        self.subfolder_expansion_snapshot = Some(snapshot);
        self.subfolder_expansion_progress = None;
        self.subfolder_expansion_diag = Some(diag.clone());
        self.address = format!("サブ展開: {}", root.display());
        self.rebuild_visible_indices();
        if let Some(&idx) = self.visible_indices.first() {
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        }

        let skipped = diag.read_dir_errors
            + diag.entry_errors
            + diag.file_type_errors
            + diag.metadata_errors
            + diag.depth_limit_hits;
        if !show_toast {
            return;
        }
        if skipped > 0 {
            self.show_feedback_toast(format!(
                "サブ展開: {item_count}件 (読めなかった項目 {skipped}件)"
            ));
        } else {
            self.show_feedback_toast(format!("サブ展開: {item_count}件"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_path_is_registered_as_synthetic_view() {
        assert!(is_synthetic_view_path(&subfolder_expansion_synthetic_path()));
    }

    #[test]
    fn duplicate_filter_is_scoped_to_one_parent() {
        let a = PathBuf::from(r"C:\root\a\same.jpg");
        let b = PathBuf::from(r"C:\root\b\same.png");
        let mut media = vec![(a.clone(), false, 1, 10), (b.clone(), false, 2, 20)];
        let options = SubfolderExpansionOptions {
            skip_image_if_video_exists: false,
            skip_duplicate_images: true,
            video_thumb_use_sidecar_image: true,
            image_ext_priority: vec!["jpg".into(), "png".into()],
        };
        let mut overrides = HashMap::new();
        apply_duplicate_filters_to_media(&mut media, &options, &mut overrides);

        assert_eq!(media.len(), 1);
        assert_eq!(media[0].0, a);

        let mut parent_a = vec![(a.clone(), false, 1, 10)];
        let mut parent_b = vec![(b.clone(), false, 2, 20)];
        apply_duplicate_filters_to_media(&mut parent_a, &options, &mut overrides);
        apply_duplicate_filters_to_media(&mut parent_b, &options, &mut overrides);
        assert_eq!(parent_a[0].0, a);
        assert_eq!(parent_b[0].0, b);
    }

    #[test]
    fn sort_entries_tiebreaks_same_name_by_relative_parent() {
        let root = PathBuf::from(r"C:\root");
        let entries = vec![
            SubfolderExpansionEntry {
                path: root.join("b").join("same.jpg"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: root.join("a").join("same.jpg"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
        ];
        let sorted = sort_entries_for_view(entries, crate::settings::SortOrder::FileName, &root);
        assert!(sorted[0].path.ends_with(Path::new("a").join("same.jpg")));
        assert!(sorted[1].path.ends_with(Path::new("b").join("same.jpg")));
    }
}
