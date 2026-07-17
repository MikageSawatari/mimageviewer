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

const MAX_SUBFOLDER_EXPANSION_DEPTH: u32 = 40;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const PREPARE_CONFIRM_ITEM_THRESHOLD: usize = 100_000;
/// `sort_unstable_by` の比較関数を途中で変えずにキャンセルへ応答するためのソート単位。
const SUBFOLDER_SORT_CHUNK_SIZE: usize = 16_384;
/// キー生成 / マージ中に cancel と進捗を確認する間隔。
const SUBFOLDER_SORT_PROGRESS_INTERVAL: usize = 16_384;

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
    pub(crate) roots: Vec<PathBuf>,
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
    pub(crate) roots: Vec<PathBuf>,
    /// 再ソート準備 worker と UI が巨大な走査結果を複製せず共有する。
    pub(crate) entries: Arc<Vec<SubfolderExpansionEntry>>,
    pub(crate) video_thumb_overrides: HashMap<String, PathBuf>,
    pub(crate) diag: SubfolderExpansionDiag,
}

#[derive(Debug)]
pub(crate) struct SubfolderExpansionRestoreState {
    pub(crate) root: Option<PathBuf>,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) saved_folder: Option<PathBuf>,
    pub(crate) snapshot: Option<SubfolderExpansionSnapshot>,
    pub(crate) removed_paths: HashSet<String>,
}

pub(crate) enum SubfolderExpansionEvent {
    Progress(SubfolderExpansionProgress),
    Done(SubfolderExpansionResult),
    Cancelled,
}

pub(crate) struct SubfolderExpansionPending {
    pub(crate) root: PathBuf,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) rx: mpsc::Receiver<SubfolderExpansionEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubfolderExpansionPreparePhase {
    Sorting,
    Building,
    Ratings,
    Tags,
    Adjustments,
    VideoPins,
}

impl SubfolderExpansionPreparePhase {
    fn label(self) -> &'static str {
        match self {
            Self::Sorting => "並び順を計算中",
            Self::Building => "一覧を構築中",
            Self::Ratings => "レーティングを読み込み中",
            Self::Tags => "タグを読み込み中",
            Self::Adjustments => "補正レイヤーを確認中",
            Self::VideoPins => "動画サムネイルを確認中",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SubfolderExpansionPrepareProgress {
    pub(crate) phase: SubfolderExpansionPreparePhase,
    pub(crate) completed: usize,
    pub(crate) total: usize,
}

pub(crate) struct PreparedSubfolderMetadata {
    pub(crate) rating_cache: HashMap<usize, u8>,
    pub(crate) tags_cache: HashMap<String, Vec<String>>,
    pub(crate) local_adjust_pages: HashSet<usize>,
    pub(crate) video_pin_blobs: HashMap<PathBuf, Vec<u8>>,
    pub(crate) legacy_paths: Vec<PathBuf>,
}

#[derive(Clone)]
struct ReusedSubfolderMetadata {
    /// 並び替え前の idx に依存しないよう、パスキーで保持する。
    ratings_by_path: HashMap<String, u8>,
    tags_by_path: HashMap<String, Vec<String>>,
    local_adjust_paths: HashSet<String>,
}

pub(crate) struct PreparedSubfolderExpansion {
    pub(crate) snapshot: SubfolderExpansionSnapshot,
    pub(crate) show_toast: bool,
    pub(crate) items: Vec<GridItem>,
    pub(crate) image_metas: Vec<Option<(i64, i64)>>,
    pub(crate) video_items: Vec<(usize, PathBuf, u64)>,
    pub(crate) metadata: PreparedSubfolderMetadata,
}

pub(crate) enum SubfolderExpansionPrepareEvent {
    Progress(SubfolderExpansionPrepareProgress),
    Done(PreparedSubfolderExpansion),
    Cancelled,
    Error(String),
}

pub(crate) struct SubfolderExpansionInstallPending {
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) rx: mpsc::Receiver<SubfolderExpansionPrepareEvent>,
    pub(crate) progress: SubfolderExpansionPrepareProgress,
}

pub(crate) struct SubfolderExpansionConfirmPending {
    pub(crate) snapshot: SubfolderExpansionSnapshot,
    pub(crate) show_toast: bool,
}

impl SubfolderExpansionPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl SubfolderExpansionInstallPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn_subfolder_expansion_worker(
    root: PathBuf,
    roots: Vec<PathBuf>,
    options: SubfolderExpansionOptions,
) -> Result<SubfolderExpansionPending, String> {
    let roots = normalize_expansion_roots(&root, roots);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let root_w = root.clone();
    let roots_w = roots.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("subfolder-expansion".into())
        .spawn(move || {
            let event = match scan_subfolder_expansion(
                root_w,
                roots_w,
                options,
                Arc::clone(&cancel_w),
                &tx,
            ) {
                Some(_result) if cancel_w.load(Ordering::Relaxed) => {
                    SubfolderExpansionEvent::Cancelled
                }
                Some(result) => SubfolderExpansionEvent::Done(result),
                None => SubfolderExpansionEvent::Cancelled,
            };
            let _ = tx.send(event);
        })
        .map_err(|e| format!("サブ展開ワーカーを起動できませんでした: {e}"))?;

    Ok(SubfolderExpansionPending {
        root,
        roots,
        cancel,
        rx,
    })
}

fn scan_subfolder_expansion(
    root: PathBuf,
    roots: Vec<PathBuf>,
    options: SubfolderExpansionOptions,
    cancel: Arc<AtomicBool>,
    tx: &mpsc::Sender<SubfolderExpansionEvent>,
) -> Option<SubfolderExpansionResult> {
    let roots = normalize_expansion_roots(&root, roots);
    let mut result = SubfolderExpansionResult {
        root: root.clone(),
        roots: roots.clone(),
        entries: Vec::new(),
        video_thumb_overrides: HashMap::new(),
        diag: SubfolderExpansionDiag::default(),
    };
    let mut visited = HashSet::new();
    let mut stack: Vec<_> = roots
        .iter()
        .rev()
        .cloned()
        .map(|root| (root, 0_u32))
        .collect();
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

fn normalize_expansion_roots(root: &Path, roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let source = if roots.is_empty() {
        vec![root.to_path_buf()]
    } else {
        roots
    };
    let mut normalized: Vec<PathBuf> = Vec::new();
    for path in source {
        if normalized
            .iter()
            .any(|existing| crate::folder_tree::path_eq(existing, &path))
        {
            continue;
        }
        normalized.push(path);
    }
    if normalized.is_empty() {
        normalized.push(root.to_path_buf());
    }
    normalized
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

    let mut media: Vec<(PathBuf, super::folder_scan::ScanMediaKind, i64, i64)> = Vec::new();
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
        // サブ展開ビューは現状 画像 / 動画 のフラット一覧。音声 (GridItem::Audio) は
        // 通常フォルダ一覧にのみ出し、サブ展開ビューへの取り込みは後続 Inc とする。
        let kind = if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
            super::folder_scan::ScanMediaKind::Image
        } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
            super::folder_scan::ScanMediaKind::Video
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
            kind,
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
        ));
    }

    super::folder_scan::filter_upscaled_video_pairs_fast(&mut media, &entry_file_names_ci);
    apply_duplicate_filters_to_media(&mut media, options, &mut result.video_thumb_overrides);
    result.diag.media_found += media.len();
    result
        .entries
        .extend(
            media
                .into_iter()
                .map(|(path, kind, mtime, file_size)| SubfolderExpansionEntry {
                    path,
                    is_video: kind == super::folder_scan::ScanMediaKind::Video,
                    mtime,
                    file_size,
                }),
        );
}

fn apply_duplicate_filters_to_media(
    media: &mut Vec<(PathBuf, super::folder_scan::ScanMediaKind, i64, i64)>,
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
    media: &mut Vec<(PathBuf, super::folder_scan::ScanMediaKind, i64, i64)>,
    use_sidecar: bool,
    video_thumb_overrides: &mut HashMap<String, PathBuf>,
) {
    use super::folder_scan::ScanMediaKind;
    let mut videos_by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (path, kind, _, _) in media.iter() {
        if *kind == ScanMediaKind::Video {
            videos_by_stem
                .entry(stem_lower_local(path))
                .or_default()
                .push(path.clone());
        }
    }
    if videos_by_stem.is_empty() {
        return;
    }

    // 動画とのサイドカー重複判定・除外は画像のみ対象 (音声は常に残す)。現状サブ展開の
    // media は Image / Video のみだが、将来 Audio を含めても安全なよう `!= Image` で判定。
    if use_sidecar {
        for (path, kind, _, _) in media.iter() {
            if *kind != ScanMediaKind::Image {
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

    media.retain(|(path, kind, _, _)| {
        *kind != ScanMediaKind::Image || !videos_by_stem.contains_key(&stem_lower_local(path))
    });
}

fn filter_image_ext_duplicates(
    media: &mut Vec<(PathBuf, super::folder_scan::ScanMediaKind, i64, i64)>,
    priority: &[String],
) {
    use super::folder_scan::ScanMediaKind;
    let mut best: HashMap<String, (usize, usize)> = HashMap::new();
    for (i, (path, kind, _, _)) in media.iter().enumerate() {
        if *kind != ScanMediaKind::Image {
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
    for (path, kind, _, _) in media.iter() {
        if *kind == ScanMediaKind::Image {
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
    media.retain(|(path, kind, _, _)| {
        let current_i = i;
        i += 1;
        if *kind != ScanMediaKind::Image {
            return true;
        }
        let stem = stem_lower_local(path);
        if stem_counts.get(&stem).copied().unwrap_or(0) <= 1 {
            return true;
        }
        keep_indices.contains(&current_i)
    });
}

struct SubfolderEntrySortKey {
    name: crate::filename_sort::SortNameKey,
    parent: crate::filename_sort::SortNameKey,
    row: usize,
}

fn compare_subfolder_entry_indices(
    ai: usize,
    bi: usize,
    entries: &[SubfolderExpansionEntry],
    keys: &[SubfolderEntrySortKey],
    sort: crate::settings::SortOrder,
    order: crate::settings::SubfolderExpansionOrder,
) -> std::cmp::Ordering {
    use crate::settings::SubfolderExpansionOrder;

    let a = &entries[ai];
    let b = &entries[bi];
    let ak = &keys[ai];
    let bk = &keys[bi];
    let within_folder = || {
        ak.row
            .cmp(&bk.row)
            .then_with(|| sort.compare_name_keys(&ak.name, a.mtime, &bk.name, b.mtime))
            .then_with(|| a.path.cmp(&b.path))
    };
    match order {
        SubfolderExpansionOrder::Flat => within_folder()
            .then_with(|| ak.parent.compare_file_name(&bk.parent))
            .then_with(|| a.path.cmp(&b.path)),
        SubfolderExpansionOrder::FolderGrouped => ak
            .parent
            .compare_file_name(&bk.parent)
            .then_with(within_folder)
            .then_with(|| a.path.cmp(&b.path)),
    }
}

fn scaled_subfolder_sort_progress(
    stage_start: usize,
    stage_span: usize,
    completed: usize,
    work_total: usize,
) -> usize {
    if work_total == 0 {
        return stage_start.saturating_add(stage_span);
    }
    let scaled = (completed.min(work_total) as u128 * stage_span as u128) / work_total as u128;
    stage_start.saturating_add(scaled as usize)
}

fn report_subfolder_sort_progress(progress: &mut Option<&mut dyn FnMut(usize)>, completed: usize) {
    if let Some(report) = progress.as_deref_mut() {
        report(completed);
    }
}

/// `source` 内の `run_width` ごとのソート済み run を 2 本ずつ `target` へマージする。
/// cancel は比較関数の外でだけ確認し、ソートの全順序契約を維持する。
fn merge_subfolder_sort_runs(
    source: &[usize],
    target: &mut [usize],
    run_width: usize,
    compare: &impl Fn(usize, usize) -> std::cmp::Ordering,
    cancel: Option<&AtomicBool>,
    on_progress: &mut impl FnMut(usize),
) -> bool {
    let len = source.len();
    let pair_width = run_width.saturating_mul(2).max(1);
    let mut written = 0usize;
    for start in (0..len).step_by(pair_width) {
        if cancel.is_some_and(cancelled) {
            return false;
        }
        let mid = start.saturating_add(run_width).min(len);
        let end = start.saturating_add(pair_width).min(len);
        let (mut left, mut right, mut out) = (start, mid, start);
        while left < mid || right < end {
            let take_left = right >= end
                || (left < mid
                    && compare(source[left], source[right]) != std::cmp::Ordering::Greater);
            target[out] = if take_left {
                let value = source[left];
                left += 1;
                value
            } else {
                let value = source[right];
                right += 1;
                value
            };
            out += 1;
            written += 1;
            if written.is_multiple_of(SUBFOLDER_SORT_PROGRESS_INTERVAL) {
                if cancel.is_some_and(cancelled) {
                    return false;
                }
                on_progress(written);
                if cancel.is_some_and(cancelled) {
                    return false;
                }
            }
        }
    }
    on_progress(written);
    !cancel.is_some_and(cancelled)
}

#[allow(clippy::too_many_arguments)]
fn sorted_entry_indices_for_view(
    entries: &[SubfolderExpansionEntry],
    sort: crate::settings::SortOrder,
    order: crate::settings::SubfolderExpansionOrder,
    display_order: &crate::settings::GridDisplayOrder,
    root: &Path,
    cancel: Option<&AtomicBool>,
    mut progress: Option<&mut dyn FnMut(usize)>,
    chunk_size: usize,
) -> Option<Vec<usize>> {
    use crate::settings::GridItemDisplayKind;

    let total = entries.len();
    if total == 0 {
        report_subfolder_sort_progress(&mut progress, 0);
        return Some(Vec::new());
    }
    let key_stage_end = total / 3;
    let chunk_stage_end = total.saturating_mul(2) / 3;

    let mut keys = Vec::with_capacity(total);
    for (idx, entry) in entries.iter().enumerate() {
        if idx.is_multiple_of(SUBFOLDER_SORT_PROGRESS_INTERVAL) {
            if cancel.is_some_and(cancelled) {
                return None;
            }
            report_subfolder_sort_progress(
                &mut progress,
                scaled_subfolder_sort_progress(0, key_stage_end, idx, total),
            );
        }
        let name = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let parent = entry
            .path
            .parent()
            .and_then(|parent| parent.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = if entry.is_video {
            GridItemDisplayKind::VideoAudio
        } else {
            GridItemDisplayKind::Image
        };
        keys.push(SubfolderEntrySortKey {
            name: sort.name_key(name),
            parent: crate::filename_sort::SortNameKey::file_name(&parent),
            row: display_order.row_for(kind),
        });
    }
    report_subfolder_sort_progress(&mut progress, key_stage_end);
    if cancel.is_some_and(cancelled) {
        return None;
    }

    let chunk_size = chunk_size.max(1);
    let chunk_count = total.div_ceil(chunk_size);
    let mut indices: Vec<usize> = (0..total).collect();
    let compare = |ai, bi| compare_subfolder_entry_indices(ai, bi, entries, &keys, sort, order);
    for (chunk_index, chunk) in indices.chunks_mut(chunk_size).enumerate() {
        if cancel.is_some_and(cancelled) {
            return None;
        }
        chunk.sort_unstable_by(|ai, bi| compare(*ai, *bi));
        let processed = ((chunk_index + 1) * chunk_size).min(total);
        let completed = if chunk_count == 1 {
            total
        } else {
            scaled_subfolder_sort_progress(
                key_stage_end,
                chunk_stage_end.saturating_sub(key_stage_end),
                processed,
                total,
            )
        };
        report_subfolder_sort_progress(&mut progress, completed);
        if cancel.is_some_and(cancelled) {
            return None;
        }
    }
    if chunk_count == 1 {
        return Some(indices);
    }

    let mut merge_levels = 0usize;
    let mut level_width = chunk_size;
    while level_width < total {
        merge_levels += 1;
        level_width = level_width.saturating_mul(2);
    }
    let merge_work_total = total.saturating_mul(merge_levels);
    let merge_stage_span = total.saturating_sub(chunk_stage_end);
    let mut merge_work_done = 0usize;
    let mut scratch = vec![0usize; total];
    let mut source_in_indices = true;
    let mut run_width = chunk_size;
    while run_width < total {
        let work_before_pass = merge_work_done;
        let mut report_merge = |pass_written: usize| {
            report_subfolder_sort_progress(
                &mut progress,
                scaled_subfolder_sort_progress(
                    chunk_stage_end,
                    merge_stage_span,
                    work_before_pass.saturating_add(pass_written),
                    merge_work_total,
                ),
            );
        };
        let completed = if source_in_indices {
            merge_subfolder_sort_runs(
                &indices,
                &mut scratch,
                run_width,
                &compare,
                cancel,
                &mut report_merge,
            )
        } else {
            merge_subfolder_sort_runs(
                &scratch,
                &mut indices,
                run_width,
                &compare,
                cancel,
                &mut report_merge,
            )
        };
        if !completed {
            return None;
        }
        merge_work_done = merge_work_done.saturating_add(total);
        source_in_indices = !source_in_indices;
        run_width = run_width.saturating_mul(2);
    }
    report_subfolder_sort_progress(&mut progress, total);
    if cancel.is_some_and(cancelled) {
        return None;
    }
    Some(if source_in_indices { indices } else { scratch })
}

#[cfg(test)]
pub(crate) fn sort_entries_for_view(
    entries: Vec<SubfolderExpansionEntry>,
    sort: crate::settings::SortOrder,
    root: &Path,
) -> Vec<SubfolderExpansionEntry> {
    let indices = sorted_entry_indices_for_view(
        &entries,
        sort,
        crate::settings::SubfolderExpansionOrder::Flat,
        &crate::settings::GridDisplayOrder::default(),
        root,
        None,
        None,
        SUBFOLDER_SORT_CHUNK_SIZE,
    )
    .expect("test sort is not cancelled");
    indices
        .into_iter()
        .map(|idx| entries[idx].clone())
        .collect()
}

#[derive(Clone)]
struct SubfolderExpansionPrepareOptions {
    sort: crate::settings::SortOrder,
    order: crate::settings::SubfolderExpansionOrder,
    display_order: crate::settings::GridDisplayOrder,
    load_ratings: bool,
    load_tags: bool,
    load_local_adjust: bool,
    load_video_pins: bool,
    removed_paths: HashSet<String>,
    /// 表示中のサブ展開を並び替える場合は、DB を再読込せず現在の
    /// sparse cache を新しい idx へ割り当て直す。
    reused_metadata: Option<ReusedSubfolderMetadata>,
}

fn prepare_progress(
    tx: &mpsc::Sender<SubfolderExpansionPrepareEvent>,
    phase: SubfolderExpansionPreparePhase,
    completed: usize,
    total: usize,
) {
    let _ = tx.send(SubfolderExpansionPrepareEvent::Progress(
        SubfolderExpansionPrepareProgress {
            phase,
            completed,
            total,
        },
    ));
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn prepare_subfolder_expansion(
    snapshot: SubfolderExpansionSnapshot,
    show_toast: bool,
    options: SubfolderExpansionPrepareOptions,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<SubfolderExpansionPrepareEvent>,
) -> Result<Option<PreparedSubfolderExpansion>, String> {
    let reused_metadata = options.reused_metadata.as_ref();
    let total = snapshot.entries.len();
    prepare_progress(tx, SubfolderExpansionPreparePhase::Sorting, 0, total);
    let mut report_sort_progress = |completed| {
        prepare_progress(
            tx,
            SubfolderExpansionPreparePhase::Sorting,
            completed,
            total,
        );
    };
    let Some(indices) = sorted_entry_indices_for_view(
        snapshot.entries.as_slice(),
        options.sort,
        options.order,
        &options.display_order,
        &snapshot.root,
        Some(cancel),
        Some(&mut report_sort_progress),
        SUBFOLDER_SORT_CHUNK_SIZE,
    ) else {
        return Ok(None);
    };

    prepare_progress(tx, SubfolderExpansionPreparePhase::Building, 0, total);
    let mut items = Vec::with_capacity(total);
    let mut image_metas = Vec::with_capacity(total);
    // DB 読み込みが必要な初回表示だけ、全件分の正規化キーを
    // 保持する。再ソート時はこの数百万件バッファ自体も不要にする。
    let mut key_by_idx = if reused_metadata.is_none() {
        Vec::with_capacity(total)
    } else {
        Vec::new()
    };
    let mut rating_cache = HashMap::new();
    let mut local_adjust_pages = HashSet::new();
    for (position, entry_idx) in indices.into_iter().enumerate() {
        if position % 16_384 == 0 {
            if cancelled(cancel) {
                return Ok(None);
            }
            prepare_progress(
                tx,
                SubfolderExpansionPreparePhase::Building,
                position,
                total,
            );
        }
        let entry = &snapshot.entries[entry_idx];
        let path_key = crate::adjustment_db::normalize_path(&entry.path);
        if options.removed_paths.contains(&path_key) {
            continue;
        }
        let display_idx = items.len();
        let item = if entry.is_video {
            GridItem::Video(entry.path.clone())
        } else {
            GridItem::Image(entry.path.clone())
        };
        items.push(item);
        image_metas.push(Some((entry.mtime, entry.file_size)));
        if let Some(reused) = reused_metadata {
            if let Some(stars) = reused.ratings_by_path.get(&path_key) {
                rating_cache.insert(display_idx, *stars);
            }
            if reused.local_adjust_paths.contains(&path_key) {
                local_adjust_pages.insert(display_idx);
            }
        } else {
            key_by_idx.push((display_idx, path_key));
        }
    }
    let video_items = crate::filename_stack_ui::stack_video_items(&items, &image_metas);

    if reused_metadata.is_none() && options.load_ratings {
        prepare_progress(tx, SubfolderExpansionPreparePhase::Ratings, 0, total);
        let db = crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
            .map_err(|e| format!("レーティング DB を読み込めませんでした: {e}"))?;
        for (chunk_no, chunk) in key_by_idx.chunks(10_000).enumerate() {
            if cancelled(cancel) {
                return Ok(None);
            }
            let keys = chunk.iter().map(|(_, key)| key.clone()).collect::<Vec<_>>();
            let loaded = db.get_many(&keys);
            for (idx, key) in chunk {
                if let Some(stars) = loaded.get(key).copied().filter(|stars| *stars > 0) {
                    rating_cache.insert(*idx, stars);
                }
            }
            prepare_progress(
                tx,
                SubfolderExpansionPreparePhase::Ratings,
                ((chunk_no + 1) * 10_000).min(total),
                total,
            );
        }
    }

    let mut tags_cache = reused_metadata
        .map(|reused| reused.tags_by_path.clone())
        .unwrap_or_default();
    if reused_metadata.is_none() && options.load_tags {
        prepare_progress(tx, SubfolderExpansionPreparePhase::Tags, 0, total);
        let db = crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path())
            .map_err(|e| format!("タグ DB を読み込めませんでした: {e}"))?;
        for (chunk_no, chunk) in key_by_idx.chunks(10_000).enumerate() {
            if cancelled(cancel) {
                return Ok(None);
            }
            let keys = chunk.iter().map(|(_, key)| key.clone()).collect::<Vec<_>>();
            for (key, tags) in db.get_many_display_tags(&keys) {
                if !tags.is_empty() {
                    tags_cache.insert(key, tags);
                }
            }
            prepare_progress(
                tx,
                SubfolderExpansionPreparePhase::Tags,
                ((chunk_no + 1) * 10_000).min(total),
                total,
            );
        }
    }

    if reused_metadata.is_none() && options.load_local_adjust {
        prepare_progress(tx, SubfolderExpansionPreparePhase::Adjustments, 0, total);
        let db = crate::local_adjust_db::LocalAdjustDb::open_readonly(
            &crate::local_adjust_db::LocalAdjustDb::db_path(),
        )
        .map_err(|e| format!("補正レイヤー DB を読み込めませんでした: {e}"))?;
        for (chunk_no, chunk) in key_by_idx.chunks(10_000).enumerate() {
            if cancelled(cancel) {
                return Ok(None);
            }
            let keys = chunk.iter().map(|(_, key)| key.clone()).collect::<Vec<_>>();
            let existing = db.load_existing_layer_keys(&keys);
            for (idx, key) in chunk {
                if existing.contains(key) {
                    local_adjust_pages.insert(*idx);
                }
            }
            prepare_progress(
                tx,
                SubfolderExpansionPreparePhase::Adjustments,
                ((chunk_no + 1) * 10_000).min(total),
                total,
            );
        }
    }

    let mut video_pin_blobs = HashMap::new();
    if options.load_video_pins && !video_items.is_empty() {
        prepare_progress(
            tx,
            SubfolderExpansionPreparePhase::VideoPins,
            0,
            video_items.len(),
        );
        let db =
            crate::video_pins::VideoPinDb::open_readonly(&crate::video_pins::VideoPinDb::db_path())
                .map_err(|e| format!("動画ピン DB を読み込めませんでした: {e}"))?;
        video_pin_blobs = db.lookup_webps_many(video_items.iter().map(|(_, path, _)| path));
    }
    if cancelled(cancel) {
        return Ok(None);
    }

    // 数百万件では正規化キー文字列だけでも大きい。legacy seed 用 path snapshot を
    // 作る前に DB lookup 用バッファを明示的に解放して peak memory を抑える。
    drop(key_by_idx);
    let legacy_paths = items
        .iter()
        .filter_map(|item| match item {
            GridItem::Image(path) | GridItem::Video(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    Ok(Some(PreparedSubfolderExpansion {
        snapshot,
        show_toast,
        items,
        image_metas,
        video_items,
        metadata: PreparedSubfolderMetadata {
            rating_cache,
            tags_cache,
            local_adjust_pages,
            video_pin_blobs,
            legacy_paths,
        },
    }))
}

fn spawn_subfolder_expansion_prepare(
    snapshot: SubfolderExpansionSnapshot,
    show_toast: bool,
    options: SubfolderExpansionPrepareOptions,
) -> Result<SubfolderExpansionInstallPending, String> {
    let total = snapshot.entries.len();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("subfolder-view-prepare".into())
        .spawn(move || {
            let event =
                match prepare_subfolder_expansion(snapshot, show_toast, options, &cancel_w, &tx) {
                    Ok(Some(prepared)) => SubfolderExpansionPrepareEvent::Done(prepared),
                    Ok(None) => SubfolderExpansionPrepareEvent::Cancelled,
                    Err(message) => SubfolderExpansionPrepareEvent::Error(message),
                };
            let _ = tx.send(event);
        })
        .map_err(|e| format!("サブ展開の表示準備を開始できませんでした: {e}"))?;
    Ok(SubfolderExpansionInstallPending {
        cancel,
        rx,
        progress: SubfolderExpansionPrepareProgress {
            phase: SubfolderExpansionPreparePhase::Sorting,
            completed: 0,
            total,
        },
    })
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
    pub(crate) fn grid_item_can_be_checked(&self, idx: usize) -> bool {
        let Some(item) = self.items.get(idx) else {
            return false;
        };
        item.is_checkable()
            || (self.subfolder_expansion_available() && matches!(item, GridItem::Folder(_)))
    }

    pub(crate) fn selected_subfolder_expansion_roots(&self) -> Vec<PathBuf> {
        if !self.subfolder_expansion_available() || self.checked.is_empty() {
            return Vec::new();
        }
        let mut indices: Vec<_> = self.checked.iter().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|idx| match self.items.get(idx) {
                Some(GridItem::Folder(path)) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn subfolder_expansion_action_tooltip(&self) -> String {
        let roots = self.selected_subfolder_expansion_roots();
        if roots.is_empty() {
            "現在のフォルダ以下の画像と動画をフラット表示\nフォルダを Space / Ctrl+クリックで選ぶと、選んだフォルダだけをまとめて展開できます".to_string()
        } else {
            format!(
                "チェックした {} 個のフォルダ以下の画像と動画をまとめてフラット表示",
                roots.len()
            )
        }
    }

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

    pub(crate) fn subfolder_expansion_busy(&self) -> bool {
        self.subfolder_expansion_pending.is_some()
            || self.subfolder_expansion_install_pending.is_some()
            || self.subfolder_expansion_confirm_pending.is_some()
    }

    pub(crate) fn subfolder_expansion_pending_tooltip(&self) -> Option<String> {
        if let Some(pending) = self.subfolder_expansion_install_pending.as_ref() {
            return Some(format!(
                "サブ展開の表示を準備中: {}\n{} / {}件\n中止ボタンでキャンセル",
                pending.progress.phase.label(),
                pending.progress.completed,
                pending.progress.total
            ));
        }
        if let Some(pending) = self.subfolder_expansion_confirm_pending.as_ref() {
            return Some(format!(
                "{}件の表示準備を開始するか確認待ちです",
                pending.snapshot.entries.len()
            ));
        }
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
            None => {
                if pending.roots.len() > 1 {
                    format!(
                        "サブ展開中: {}\n起点: {}フォルダ",
                        pending.root.display(),
                        pending.roots.len()
                    )
                } else {
                    format!("サブ展開中: {}", pending.root.display())
                }
            }
        })
    }

    pub(crate) fn toggle_subfolder_expansion_view(&mut self) {
        if self.subfolder_expansion_pending.is_some()
            || self.subfolder_expansion_install_pending.is_some()
            || self.subfolder_expansion_confirm_pending.is_some()
        {
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
        let selected_roots = self.selected_subfolder_expansion_roots();
        let roots = if selected_roots.is_empty() {
            vec![root.clone()]
        } else {
            selected_roots
        };
        self.start_subfolder_expansion_scan_roots(root, roots);
    }

    pub(crate) fn start_subfolder_expansion_scan_roots(
        &mut self,
        root: PathBuf,
        roots: Vec<PathBuf>,
    ) {
        let roots = normalize_expansion_roots(&root, roots);
        self.cancel_subfolder_expansion_pending();
        self.cancel_pending_folder_nav();
        self.cancel_stack_script_pending();
        self.stack_mode_requested = false;
        self.stack_view = None;
        self.stack_showing_flat = false;
        self.subfolder_expansion_root = Some(root.clone());
        self.subfolder_expansion_roots = roots.clone();
        self.subfolder_expansion_saved_folder = Some(root.clone());
        self.subfolder_expansion_snapshot = None;
        self.subfolder_expansion_removed_paths.clear();
        self.subfolder_expansion_install_pending = None;
        self.subfolder_expansion_confirm_pending = None;
        self.subfolder_expansion_progress = Some(SubfolderExpansionProgress::default());
        self.subfolder_expansion_diag = None;
        self.address = subfolder_expansion_view_label("サブ展開中", &root, &roots);

        match spawn_subfolder_expansion_worker(
            root.clone(),
            roots,
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
        if let Some(pending) = self.subfolder_expansion_install_pending.take() {
            pending.cancel();
        }
        self.subfolder_expansion_confirm_pending = None;
        self.subfolder_expansion_progress = None;
    }

    pub(crate) fn clear_subfolder_expansion_view_state(&mut self) {
        self.items_are_subfolder_expansion_view = false;
        self.subfolder_expansion_root = None;
        self.subfolder_expansion_roots.clear();
        self.subfolder_expansion_saved_folder = None;
        self.subfolder_expansion_snapshot = None;
        self.subfolder_expansion_removed_paths.clear();
        if let Some(pending) = self.subfolder_expansion_install_pending.take() {
            pending.cancel();
        }
        self.subfolder_expansion_confirm_pending = None;
        self.subfolder_expansion_progress = None;
        self.subfolder_expansion_diag = None;
    }

    pub(crate) fn restore_subfolder_expansion_view_state_after_items_install(&mut self) {
        let Some(snapshot) = self.subfolder_expansion_snapshot.clone() else {
            return;
        };
        let root = self
            .subfolder_expansion_root
            .clone()
            .unwrap_or_else(|| snapshot.root.clone());
        let roots = if self.subfolder_expansion_roots.is_empty() {
            snapshot.roots.clone()
        } else {
            self.subfolder_expansion_roots.clone()
        };
        self.items_are_subfolder_expansion_view = true;
        self.subfolder_expansion_root = Some(root.clone());
        self.subfolder_expansion_roots = roots.clone();
        self.subfolder_expansion_saved_folder
            .get_or_insert_with(|| root.clone());
        self.subfolder_expansion_progress = None;
        self.subfolder_expansion_install_pending = None;
        self.address = subfolder_expansion_view_label("サブ展開", &root, &roots);
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
        if self.poll_subfolder_expansion_install(ctx) {
            return;
        }
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
                    if crate::folder_tree::path_eq(&pending.root, &result.root)
                        && expansion_roots_eq(&pending.roots, &result.roots)
                    {
                        self.apply_subfolder_expansion_result(result, ctx);
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

    fn poll_subfolder_expansion_install(&mut self, ctx: &egui::Context) -> bool {
        if self.subfolder_expansion_install_pending.is_none() {
            return false;
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        loop {
            let event = {
                let Some(pending) = self.subfolder_expansion_install_pending.as_ref() else {
                    return true;
                };
                pending.rx.try_recv()
            };
            match event {
                Ok(SubfolderExpansionPrepareEvent::Progress(progress)) => {
                    if let Some(pending) = self.subfolder_expansion_install_pending.as_mut() {
                        pending.progress = progress;
                    }
                }
                Ok(SubfolderExpansionPrepareEvent::Done(prepared)) => {
                    self.subfolder_expansion_install_pending = None;
                    self.install_prepared_subfolder_expansion(prepared);
                    return true;
                }
                Ok(SubfolderExpansionPrepareEvent::Cancelled) => {
                    self.subfolder_expansion_install_pending = None;
                    self.subfolder_expansion_progress = None;
                    return true;
                }
                Ok(SubfolderExpansionPrepareEvent::Error(message)) => {
                    self.subfolder_expansion_install_pending = None;
                    self.subfolder_expansion_progress = None;
                    self.show_feedback_toast(message);
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.subfolder_expansion_install_pending = None;
                    self.subfolder_expansion_progress = None;
                    self.show_feedback_toast("サブ展開の表示準備が中断されました".into());
                    return true;
                }
            }
        }
    }

    fn apply_subfolder_expansion_result(
        &mut self,
        result: SubfolderExpansionResult,
        ctx: &egui::Context,
    ) {
        let SubfolderExpansionResult {
            root,
            roots,
            entries,
            video_thumb_overrides,
            diag,
        } = result;
        let snapshot = SubfolderExpansionSnapshot {
            root,
            roots,
            entries: Arc::new(entries),
            video_thumb_overrides,
            diag,
        };
        self.queue_or_install_subfolder_expansion_snapshot(snapshot, true, ctx);
    }

    fn queue_or_install_subfolder_expansion_snapshot(
        &mut self,
        snapshot: SubfolderExpansionSnapshot,
        show_toast: bool,
        ctx: &egui::Context,
    ) {
        let item_count = snapshot.entries.len();
        if item_count >= PREPARE_CONFIRM_ITEM_THRESHOLD {
            self.subfolder_expansion_progress =
                Some(SubfolderExpansionProgress::from_diag(&snapshot.diag, None));
            self.address =
                subfolder_expansion_view_label("サブ展開確認待ち", &snapshot.root, &snapshot.roots);
            self.subfolder_expansion_confirm_pending = Some(SubfolderExpansionConfirmPending {
                snapshot,
                show_toast,
            });
            ctx.request_repaint();
            return;
        }
        self.start_subfolder_expansion_prepare(snapshot, show_toast);
        ctx.request_repaint();
    }

    fn start_subfolder_expansion_prepare(
        &mut self,
        snapshot: SubfolderExpansionSnapshot,
        show_toast: bool,
    ) {
        self.start_subfolder_expansion_prepare_with_metadata(snapshot, show_toast, None);
    }

    fn start_subfolder_expansion_prepare_with_metadata(
        &mut self,
        snapshot: SubfolderExpansionSnapshot,
        show_toast: bool,
        reused_metadata: Option<ReusedSubfolderMetadata>,
    ) {
        if let Some(pending) = self.subfolder_expansion_install_pending.take() {
            pending.cancel();
        }
        self.subfolder_expansion_progress =
            Some(SubfolderExpansionProgress::from_diag(&snapshot.diag, None));
        self.address =
            subfolder_expansion_view_label("サブ展開準備中", &snapshot.root, &snapshot.roots);
        let options = SubfolderExpansionPrepareOptions {
            sort: self.book_sort_order_for_path(&snapshot.root),
            order: self.settings.subfolder_expansion_order,
            display_order: self.settings.grid_display_order.clone(),
            load_ratings: self.rating_db.is_some(),
            load_tags: self.tags_db.is_some(),
            load_local_adjust: self.local_adjust_db.is_some(),
            load_video_pins: self.video_pin_db.is_some(),
            removed_paths: self.subfolder_expansion_removed_paths.clone(),
            reused_metadata,
        };
        match spawn_subfolder_expansion_prepare(snapshot, show_toast, options) {
            Ok(pending) => self.subfolder_expansion_install_pending = Some(pending),
            Err(message) => {
                self.subfolder_expansion_progress = None;
                self.show_feedback_toast(message);
            }
        }
    }

    fn current_subfolder_metadata_for_resort(&self) -> Option<ReusedSubfolderMetadata> {
        if !self.items_are_subfolder_expansion_view {
            return None;
        }

        let ratings_by_path = self
            .rating_cache
            .iter()
            .filter_map(|(&idx, &stars)| {
                if stars == 0 {
                    return None;
                }
                let path = match self.items.get(idx)? {
                    GridItem::Image(path) | GridItem::Video(path) => path,
                    _ => return None,
                };
                Some((crate::adjustment_db::normalize_path(path), stars))
            })
            .collect();
        let tags_by_path = self
            .tags_cache
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(key, tags)| (key.clone(), tags.clone()))
            .collect();
        let local_adjust_paths = self
            .local_adjust_pages
            .iter()
            .filter_map(|&idx| match self.items.get(idx)? {
                GridItem::Image(path) | GridItem::Video(path) => {
                    Some(crate::adjustment_db::normalize_path(path))
                }
                _ => None,
            })
            .collect();

        Some(ReusedSubfolderMetadata {
            ratings_by_path,
            tags_by_path,
            local_adjust_paths,
        })
    }

    pub(crate) fn reinstall_subfolder_expansion_snapshot(&mut self) -> bool {
        let Some(snapshot) = self.subfolder_expansion_snapshot.clone() else {
            return false;
        };
        let reused_metadata = self.current_subfolder_metadata_for_resort();
        self.start_subfolder_expansion_prepare_with_metadata(snapshot, false, reused_metadata);
        true
    }

    pub(crate) fn remove_paths_from_subfolder_expansion_snapshot(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let removed: HashSet<String> = paths
            .iter()
            .map(|path| crate::path_key::normalize_keep_drive(path))
            .collect();
        if removed.is_empty() {
            return;
        }

        self.subfolder_expansion_removed_paths
            .extend(removed.iter().cloned());

        let mut active_changed = false;
        if let Some(snapshot) = self.subfolder_expansion_snapshot.as_mut() {
            // prepare worker が同じ Arc を保持中なら Arc::make_mut は数百万件を UI
            // スレッドで複製する。tombstone だけを残し、共有が無い時だけ compact する。
            if Arc::strong_count(&snapshot.entries) == 1 {
                active_changed = remove_paths_from_snapshot(snapshot, &removed);
            }
        }
        if active_changed {
            self.subfolder_expansion_diag = self
                .subfolder_expansion_snapshot
                .as_ref()
                .map(|s| s.diag.clone());
        }
        if let Some(confirm) = self.subfolder_expansion_confirm_pending.as_mut() {
            remove_paths_from_snapshot(&mut confirm.snapshot, &removed);
        }
        if self.subfolder_expansion_install_pending.is_some() {
            if let Some(pending) = self.subfolder_expansion_install_pending.take() {
                pending.cancel();
            }
            if let Some(snapshot) = self.subfolder_expansion_snapshot.clone() {
                self.start_subfolder_expansion_prepare(snapshot, false);
            }
        }
    }

    pub(crate) fn take_subfolder_expansion_restore_for_synthetic_path(
        &mut self,
        path: Option<&Path>,
    ) -> Option<SubfolderExpansionRestoreState> {
        let Some(path) = path else {
            return None;
        };
        if !crate::folder_tree::path_eq(path, &subfolder_expansion_synthetic_path()) {
            return None;
        }
        if self.subfolder_expansion_snapshot.is_none()
            && self.subfolder_expansion_root.is_none()
            && self.subfolder_expansion_saved_folder.is_none()
        {
            return None;
        }
        Some(SubfolderExpansionRestoreState {
            root: self.subfolder_expansion_root.clone(),
            roots: self.subfolder_expansion_roots.clone(),
            saved_folder: self.subfolder_expansion_saved_folder.clone(),
            snapshot: self.subfolder_expansion_snapshot.take(),
            removed_paths: std::mem::take(&mut self.subfolder_expansion_removed_paths),
        })
    }

    pub(crate) fn restore_subfolder_expansion_for_synthetic_path_with_state(
        &mut self,
        path: &Path,
        state: Option<SubfolderExpansionRestoreState>,
    ) -> bool {
        if !crate::folder_tree::path_eq(path, &subfolder_expansion_synthetic_path()) {
            return false;
        }
        if let Some(state) = state {
            self.subfolder_expansion_removed_paths = state.removed_paths;
            if let Some(snapshot) = state.snapshot {
                self.start_subfolder_expansion_prepare(snapshot, false);
                return true;
            }
            if let Some(root) = state.root.or(state.saved_folder) {
                let roots = if state.roots.is_empty() {
                    vec![root.clone()]
                } else {
                    state.roots
                };
                self.start_subfolder_expansion_scan_roots(root, roots);
                return true;
            }
        }
        self.restore_subfolder_expansion_for_synthetic_path(path)
    }

    pub(crate) fn restore_subfolder_expansion_for_synthetic_path(&mut self, path: &Path) -> bool {
        if !crate::folder_tree::path_eq(path, &subfolder_expansion_synthetic_path()) {
            return false;
        }
        if self.reinstall_subfolder_expansion_snapshot() {
            return true;
        }
        if let Some(root) = self
            .subfolder_expansion_root
            .clone()
            .or_else(|| self.subfolder_expansion_saved_folder.clone())
        {
            let roots = if self.subfolder_expansion_roots.is_empty() {
                vec![root.clone()]
            } else {
                self.subfolder_expansion_roots.clone()
            };
            self.start_subfolder_expansion_scan_roots(root, roots);
            return true;
        }
        self.show_feedback_toast("サブ展開ビューを復元できませんでした".into());
        true
    }

    fn install_prepared_subfolder_expansion(&mut self, prepared: PreparedSubfolderExpansion) {
        let install_t0 = Instant::now();
        let perf_on = crate::perf::is_enabled();
        let seq = self.input_seq;
        let PreparedSubfolderExpansion {
            snapshot,
            show_toast,
            items,
            image_metas,
            video_items,
            metadata,
        } = prepared;
        let entry_count = items.len();
        if perf_on {
            crate::perf::event(
                "subfolder",
                "install_begin",
                None,
                seq,
                &[
                    ("items", serde_json::Value::from(entry_count)),
                    ("roots", serde_json::Value::from(snapshot.roots.len())),
                    ("show_toast", serde_json::Value::from(show_toast)),
                ],
            );
        }
        let root = snapshot.root.clone();
        let roots = snapshot.roots.clone();
        let diag = snapshot.diag.clone();
        let video_thumb_overrides = snapshot.video_thumb_overrides.clone();
        self.video_thumb_overrides.clear();
        self.video_thumb_overrides.extend(video_thumb_overrides);

        let item_count = items.len();
        let start_loading_t0 = Instant::now();
        self.start_loading_subfolder_items(
            subfolder_expansion_synthetic_path(),
            items,
            image_metas,
            video_items,
            metadata,
        );
        if perf_on {
            crate::perf::event(
                "subfolder",
                "install_start_loading",
                None,
                seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(start_loading_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("items", serde_json::Value::from(item_count)),
                ],
            );
        }
        let state_t0 = Instant::now();
        self.items_are_subfolder_expansion_view = true;
        self.subfolder_expansion_root = Some(root.clone());
        self.subfolder_expansion_roots = roots.clone();
        self.subfolder_expansion_saved_folder = Some(root.clone());
        self.subfolder_expansion_snapshot = Some(snapshot);
        self.subfolder_expansion_progress = None;
        self.subfolder_expansion_diag = Some(diag.clone());
        self.address = subfolder_expansion_view_label("サブ展開", &root, &roots);
        if perf_on {
            crate::perf::event(
                "subfolder",
                "install_state",
                None,
                seq,
                &[(
                    "ms",
                    serde_json::Value::from(state_t0.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }
        if let Some(&idx) = self.visible_indices.first() {
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        }
        if perf_on {
            crate::perf::event(
                "subfolder",
                "install_end",
                None,
                seq,
                &[
                    (
                        "ms",
                        serde_json::Value::from(install_t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("items", serde_json::Value::from(item_count)),
                ],
            );
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
        } else if roots.len() > 1 {
            self.show_feedback_toast(format!(
                "サブ展開: {item_count}件 ({}フォルダ)",
                roots.len()
            ));
        } else {
            self.show_feedback_toast(format!("サブ展開: {item_count}件"));
        }
    }

    pub(crate) fn render_subfolder_expansion_install_overlay(&mut self, ctx: &egui::Context) {
        if self.subfolder_expansion_pending.is_some() {
            let progress = self
                .subfolder_expansion_progress
                .clone()
                .unwrap_or_default();
            let current_dir = progress
                .current_dir
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned());
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("subfolder_expansion_scan_modal")).show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.heading("サブフォルダを走査中...");
                });
                ui.add_space(6.0);
                ui.label(format!("画像・動画: {} 件", progress.media_found));
                ui.label(format!("確認済みフォルダ: {} 件", progress.dirs_scanned));
                if let Some(current_dir) = current_dir.as_deref() {
                    ui.label("現在のフォルダ:").on_hover_text(current_dir);
                    ui.add(
                        egui::Label::new(
                            std::path::Path::new(current_dir)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(current_dir),
                        )
                        .truncate(),
                    )
                    .on_hover_text(current_dir);
                }
                ui.add_space(8.0);
                if ui.button("中止").clicked() {
                    cancel = true;
                }
            });
            if cancel {
                self.cancel_subfolder_expansion_pending();
                if let Some(current) = self.current_folder.clone() {
                    self.address = self
                        .book_address_label_for_path(&current)
                        .unwrap_or_else(|| current.to_string_lossy().to_string());
                }
                self.show_feedback_toast("サブ展開をキャンセルしました".into());
            }
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }

        if let Some(confirm) = self.subfolder_expansion_confirm_pending.as_ref() {
            let item_count = confirm.snapshot.entries.len();
            let mut proceed = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("subfolder_expansion_confirm")).show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.heading("大量の項目をサブ展開");
                ui.add_space(8.0);
                ui.label(format!("{item_count} 件の画像・動画が見つかりました。"));
                ui.label("一覧の準備には時間がかかることがあります。");
                ui.label("準備中は「中止」以外の操作はできません。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("続ける").clicked() {
                        proceed = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
            if proceed {
                if let Some(confirm) = self.subfolder_expansion_confirm_pending.take() {
                    self.start_subfolder_expansion_prepare(confirm.snapshot, confirm.show_toast);
                }
            } else if cancel {
                self.subfolder_expansion_confirm_pending = None;
                self.subfolder_expansion_progress = None;
                if let Some(current) = self.current_folder.clone() {
                    self.address = self
                        .book_address_label_for_path(&current)
                        .unwrap_or_else(|| current.to_string_lossy().to_string());
                }
                self.show_feedback_toast("サブ展開をキャンセルしました".into());
            }
            return;
        }

        let Some(pending) = self.subfolder_expansion_install_pending.as_ref() else {
            return;
        };
        let progress = pending.progress.clone();
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("subfolder_expansion_install_modal")).show(ctx, |ui| {
            // 数百万件の進捗でも件数行が折り返されない幅を確保する。
            ui.set_min_width(460.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading("サブ展開の表示を準備中...");
            });
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(format!(
                    "{}: {} / {} 件",
                    progress.phase.label(),
                    progress.completed,
                    progress.total
                ))
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.add_space(8.0);
            if ui.button("中止").clicked() {
                cancel = true;
            }
        });
        if cancel {
            if let Some(pending) = self.subfolder_expansion_install_pending.take() {
                pending.cancel();
            }
            self.subfolder_expansion_progress = None;
            if self.items_are_subfolder_expansion_view {
                if let (Some(root), roots) = (
                    self.subfolder_expansion_root.clone(),
                    self.subfolder_expansion_roots.clone(),
                ) {
                    self.address = subfolder_expansion_view_label("サブ展開", &root, &roots);
                }
            } else if let Some(current) = self.current_folder.clone() {
                self.address = self
                    .book_address_label_for_path(&current)
                    .unwrap_or_else(|| current.to_string_lossy().to_string());
            }
            self.show_feedback_toast("サブ展開の表示準備を中止しました".into());
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    #[cfg(test)]
    pub(crate) fn finish_subfolder_expansion_prepare_for_test(&mut self) {
        loop {
            let event = {
                let Some(pending) = self.subfolder_expansion_install_pending.as_ref() else {
                    return;
                };
                pending.rx.recv_timeout(Duration::from_secs(5))
            };
            match event {
                Ok(SubfolderExpansionPrepareEvent::Progress(progress)) => {
                    if let Some(pending) = self.subfolder_expansion_install_pending.as_mut() {
                        pending.progress = progress;
                    }
                }
                Ok(SubfolderExpansionPrepareEvent::Done(prepared)) => {
                    self.subfolder_expansion_install_pending = None;
                    self.install_prepared_subfolder_expansion(prepared);
                    return;
                }
                Ok(SubfolderExpansionPrepareEvent::Cancelled) => {
                    self.subfolder_expansion_install_pending = None;
                    return;
                }
                Ok(SubfolderExpansionPrepareEvent::Error(message)) => panic!("{message}"),
                Err(error) => panic!("subfolder prepare did not finish: {error}"),
            }
        }
    }
}

fn subfolder_expansion_view_label(prefix: &str, root: &Path, roots: &[PathBuf]) -> String {
    if roots.len() > 1 {
        format!("{prefix}: {} ({}フォルダ)", root.display(), roots.len())
    } else {
        format!("{prefix}: {}", root.display())
    }
}

fn expansion_roots_eq(a: &[PathBuf], b: &[PathBuf]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(left, right)| crate::folder_tree::path_eq(left, right))
}

fn remove_paths_from_snapshot(
    snapshot: &mut SubfolderExpansionSnapshot,
    removed: &HashSet<String>,
) -> bool {
    let before = snapshot.entries.len();
    Arc::make_mut(&mut snapshot.entries)
        .retain(|entry| !removed.contains(&crate::path_key::normalize_keep_drive(&entry.path)));
    snapshot
        .video_thumb_overrides
        .retain(|video_key, image_path| {
            !removed.contains(video_key)
                && !removed.contains(&crate::path_key::normalize_keep_drive(image_path))
        });
    let changed = snapshot.entries.len() != before;
    if changed {
        snapshot.diag.media_found = snapshot.entries.len();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_path_is_registered_as_synthetic_view() {
        assert!(is_synthetic_view_path(&subfolder_expansion_synthetic_path()));
    }

    #[test]
    fn scan_uses_checked_roots_only_when_multiple_roots_are_supplied() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().join("root");
        let a = root.join("a");
        let b = root.join("b");
        let c = root.join("c");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(a.join("a.jpg"), b"a").unwrap();
        std::fs::write(b.join("b.png"), b"b").unwrap();
        std::fs::write(c.join("c.jpg"), b"c").unwrap();
        let (tx, _rx) = mpsc::channel();

        let result = scan_subfolder_expansion(
            root.clone(),
            vec![a.clone(), b.clone()],
            SubfolderExpansionOptions {
                skip_image_if_video_exists: false,
                skip_duplicate_images: false,
                video_thumb_use_sidecar_image: true,
                image_ext_priority: Vec::new(),
            },
            Arc::new(AtomicBool::new(false)),
            &tx,
        )
        .expect("scan should finish");

        let mut names: Vec<_> = result
            .entries
            .iter()
            .filter_map(|entry| entry.path.file_name()?.to_str().map(str::to_owned))
            .collect();
        names.sort();
        assert_eq!(result.root, root);
        assert_eq!(result.roots, vec![a, b]);
        assert_eq!(names, vec!["a.jpg", "b.png"]);
    }

    #[test]
    fn duplicate_filter_is_scoped_to_one_parent() {
        use crate::app::folder_scan::ScanMediaKind;
        let a = PathBuf::from(r"C:\root\a\same.jpg");
        let b = PathBuf::from(r"C:\root\b\same.png");
        let mut media = vec![
            (a.clone(), ScanMediaKind::Image, 1, 10),
            (b.clone(), ScanMediaKind::Image, 2, 20),
        ];
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

        let mut parent_a = vec![(a.clone(), ScanMediaKind::Image, 1, 10)];
        let mut parent_b = vec![(b.clone(), ScanMediaKind::Image, 2, 20)];
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

    #[test]
    fn folder_grouped_order_keeps_each_relative_folder_together() {
        let root = PathBuf::from(r"C:\root");
        let entries = vec![
            SubfolderExpansionEntry {
                path: root.join("b").join("1.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: root.join("a").join("2.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: root.join("a").join("1.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
        ];
        let indices = sorted_entry_indices_for_view(
            &entries,
            crate::settings::SortOrder::FileName,
            crate::settings::SubfolderExpansionOrder::FolderGrouped,
            &crate::settings::GridDisplayOrder::default(),
            &root,
            None,
            None,
            SUBFOLDER_SORT_CHUNK_SIZE,
        )
        .unwrap();
        let relative = indices
            .into_iter()
            .map(|idx| entries[idx].path.strip_prefix(&root).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(
            relative,
            vec![
                Path::new("a").join("1.png"),
                Path::new("a").join("2.png"),
                Path::new("b").join("1.png"),
            ]
        );
    }

    #[test]
    fn flat_order_preserves_legacy_cross_folder_name_sort() {
        let root = PathBuf::from(r"C:\root");
        let entries = vec![
            SubfolderExpansionEntry {
                path: root.join("a").join("2.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: root.join("b").join("1.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: root.join("a").join("1.png"),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
        ];
        let indices = sorted_entry_indices_for_view(
            &entries,
            crate::settings::SortOrder::FileName,
            crate::settings::SubfolderExpansionOrder::Flat,
            &crate::settings::GridDisplayOrder::default(),
            &root,
            None,
            None,
            SUBFOLDER_SORT_CHUNK_SIZE,
        )
        .unwrap();
        let relative = indices
            .into_iter()
            .map(|idx| entries[idx].path.strip_prefix(&root).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(
            relative,
            vec![
                Path::new("a").join("1.png"),
                Path::new("b").join("1.png"),
                Path::new("a").join("2.png"),
            ]
        );
    }

    #[test]
    fn prepare_excludes_removed_path_without_mutating_shared_snapshot() {
        let root = PathBuf::from(r"C:\root");
        let removed_path = root.join("a").join("removed.png");
        let kept_path = root.join("a").join("kept.png");
        let entries = Arc::new(vec![
            SubfolderExpansionEntry {
                path: removed_path.clone(),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: kept_path.clone(),
                is_video: false,
                mtime: 2,
                file_size: 2,
            },
        ]);
        let snapshot = SubfolderExpansionSnapshot {
            root: root.clone(),
            roots: vec![root],
            entries: Arc::clone(&entries),
            video_thumb_overrides: HashMap::new(),
            diag: SubfolderExpansionDiag::default(),
        };
        let options = SubfolderExpansionPrepareOptions {
            sort: crate::settings::SortOrder::FileName,
            order: crate::settings::SubfolderExpansionOrder::FolderGrouped,
            display_order: crate::settings::GridDisplayOrder::default(),
            load_ratings: false,
            load_tags: false,
            load_local_adjust: false,
            load_video_pins: false,
            removed_paths: HashSet::from([crate::adjustment_db::normalize_path(&removed_path)]),
            reused_metadata: None,
        };
        let (tx, _rx) = mpsc::channel();
        let prepared =
            prepare_subfolder_expansion(snapshot, false, options, &AtomicBool::new(false), &tx)
                .unwrap()
                .unwrap();

        assert_eq!(
            entries.len(),
            2,
            "shared scan snapshot is not copied/mutated"
        );
        assert_eq!(prepared.items.len(), 1);
        assert!(matches!(
            prepared.items.as_slice(),
            [GridItem::Image(path)] if path == &kept_path
        ));
    }

    #[test]
    fn resort_reuses_loaded_metadata_and_remaps_indexed_caches() {
        let root = PathBuf::from(r"C:\root");
        let a1 = root.join("a").join("1.png");
        let a2 = root.join("a").join("2.png");
        let b1 = root.join("b").join("1.png");
        let entries = Arc::new(vec![
            SubfolderExpansionEntry {
                path: b1.clone(),
                is_video: false,
                mtime: 1,
                file_size: 1,
            },
            SubfolderExpansionEntry {
                path: a2.clone(),
                is_video: false,
                mtime: 2,
                file_size: 2,
            },
            SubfolderExpansionEntry {
                path: a1.clone(),
                is_video: false,
                mtime: 3,
                file_size: 3,
            },
        ]);
        let a1_key = crate::adjustment_db::normalize_path(&a1);
        let a2_key = crate::adjustment_db::normalize_path(&a2);
        let b1_key = crate::adjustment_db::normalize_path(&b1);
        let snapshot = SubfolderExpansionSnapshot {
            root: root.clone(),
            roots: vec![root],
            entries,
            video_thumb_overrides: HashMap::new(),
            diag: SubfolderExpansionDiag::default(),
        };
        let options = SubfolderExpansionPrepareOptions {
            sort: crate::settings::SortOrder::FileName,
            order: crate::settings::SubfolderExpansionOrder::FolderGrouped,
            display_order: crate::settings::GridDisplayOrder::default(),
            // reuse が効いていれば DB を open せずに完了する。
            load_ratings: true,
            load_tags: true,
            load_local_adjust: true,
            load_video_pins: false,
            removed_paths: HashSet::new(),
            reused_metadata: Some(ReusedSubfolderMetadata {
                ratings_by_path: HashMap::from([(b1_key, 4)]),
                tags_by_path: HashMap::from([(a1_key.clone(), vec!["タグ".into()])]),
                local_adjust_paths: HashSet::from([a2_key]),
            }),
        };
        let (tx, rx) = mpsc::channel();
        let prepared =
            prepare_subfolder_expansion(snapshot, false, options, &AtomicBool::new(false), &tx)
                .unwrap()
                .unwrap();

        assert!(matches!(
            prepared.items.as_slice(),
            [GridItem::Image(first), GridItem::Image(second), GridItem::Image(third)]
                if first == &a1 && second == &a2 && third == &b1
        ));
        assert_eq!(prepared.metadata.rating_cache, HashMap::from([(2, 4)]));
        assert_eq!(
            prepared.metadata.tags_cache,
            HashMap::from([(a1_key, vec!["タグ".into()])])
        );
        assert_eq!(prepared.metadata.local_adjust_pages, HashSet::from([1]));
        let phases = rx
            .try_iter()
            .filter_map(|event| match event {
                SubfolderExpansionPrepareEvent::Progress(progress) => Some(progress.phase),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!phases.contains(&SubfolderExpansionPreparePhase::Ratings));
        assert!(!phases.contains(&SubfolderExpansionPreparePhase::Tags));
        assert!(!phases.contains(&SubfolderExpansionPreparePhase::Adjustments));
    }

    #[test]
    fn sort_stops_when_prepare_is_cancelled() {
        let root = PathBuf::from(r"C:\root");
        let entries = vec![SubfolderExpansionEntry {
            path: root.join("a").join("1.png"),
            is_video: false,
            mtime: 1,
            file_size: 1,
        }];
        let cancel = AtomicBool::new(true);
        assert!(
            sorted_entry_indices_for_view(
                &entries,
                crate::settings::SortOrder::FileName,
                crate::settings::SubfolderExpansionOrder::FolderGrouped,
                &crate::settings::GridDisplayOrder::default(),
                &root,
                Some(&cancel),
                None,
                SUBFOLDER_SORT_CHUNK_SIZE,
            )
            .is_none()
        );
    }

    fn chunked_sort_test_entries(root: &Path, count: usize) -> Vec<SubfolderExpansionEntry> {
        (0..count)
            .rev()
            .map(|i| SubfolderExpansionEntry {
                path: root
                    .join(format!("folder_{:02}", i % 11))
                    .join(format!("image_{:04}.png", (i * 37) % count)),
                is_video: i % 5 == 0,
                mtime: (i % 13) as i64,
                file_size: i as i64,
            })
            .collect()
    }

    fn sort_keys_for_test(
        entries: &[SubfolderExpansionEntry],
        sort: crate::settings::SortOrder,
        display_order: &crate::settings::GridDisplayOrder,
        root: &Path,
    ) -> Vec<SubfolderEntrySortKey> {
        use crate::settings::GridItemDisplayKind;

        entries
            .iter()
            .map(|entry| {
                let name = entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                let parent = entry
                    .path
                    .parent()
                    .and_then(|parent| parent.strip_prefix(root).ok())
                    .map(|parent| parent.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let kind = if entry.is_video {
                    GridItemDisplayKind::VideoAudio
                } else {
                    GridItemDisplayKind::Image
                };
                SubfolderEntrySortKey {
                    name: sort.name_key(name),
                    parent: crate::filename_sort::SortNameKey::file_name(&parent),
                    row: display_order.row_for(kind),
                }
            })
            .collect()
    }

    #[test]
    fn chunked_sort_matches_single_total_order_sort_and_reports_monotonic_progress() {
        let root = PathBuf::from(r"C:\root");
        let entries = chunked_sort_test_entries(&root, 257);
        let display_order = crate::settings::GridDisplayOrder::default();
        for (sort, order) in [
            (
                crate::settings::SortOrder::FileName,
                crate::settings::SubfolderExpansionOrder::Flat,
            ),
            (
                crate::settings::SortOrder::DateDesc,
                crate::settings::SubfolderExpansionOrder::FolderGrouped,
            ),
        ] {
            let mut progress = Vec::new();
            let mut report = |completed| progress.push(completed);
            let actual = sorted_entry_indices_for_view(
                &entries,
                sort,
                order,
                &display_order,
                &root,
                None,
                Some(&mut report),
                17,
            )
            .unwrap();

            let keys = sort_keys_for_test(&entries, sort, &display_order, &root);
            let mut expected: Vec<usize> = (0..entries.len()).collect();
            expected.sort_unstable_by(|ai, bi| {
                compare_subfolder_entry_indices(*ai, *bi, &entries, &keys, sort, order)
            });
            assert_eq!(actual, expected);
            assert_eq!(progress.last(), Some(&entries.len()));
            assert!(progress.windows(2).all(|window| window[0] <= window[1]));
        }
    }

    #[test]
    fn chunked_sort_cancels_between_chunks_without_changing_comparator() {
        let root = PathBuf::from(r"C:\root");
        let entries = chunked_sort_test_entries(&root, 96);
        let cancel = AtomicBool::new(false);
        let key_stage_end = entries.len() / 3;
        let mut progress = Vec::new();
        let mut report = |completed| {
            progress.push(completed);
            if completed > key_stage_end {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let result = sorted_entry_indices_for_view(
            &entries,
            crate::settings::SortOrder::FileName,
            crate::settings::SubfolderExpansionOrder::FolderGrouped,
            &crate::settings::GridDisplayOrder::default(),
            &root,
            Some(&cancel),
            Some(&mut report),
            8,
        );
        assert!(result.is_none());
        assert!(progress.iter().any(|completed| *completed > key_stage_end));
    }

    #[test]
    fn chunked_sort_cancels_during_merge_without_panicking() {
        let root = PathBuf::from(r"C:\root");
        let entries = chunked_sort_test_entries(&root, 96);
        let cancel = AtomicBool::new(false);
        let chunk_stage_end = entries.len() * 2 / 3;
        let mut progress = Vec::new();
        let mut report = |completed| {
            progress.push(completed);
            if completed > chunk_stage_end {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let result = sorted_entry_indices_for_view(
            &entries,
            crate::settings::SortOrder::FileName,
            crate::settings::SubfolderExpansionOrder::FolderGrouped,
            &crate::settings::GridDisplayOrder::default(),
            &root,
            Some(&cancel),
            Some(&mut report),
            8,
        );
        assert!(result.is_none());
        assert!(
            progress
                .iter()
                .any(|completed| *completed > chunk_stage_end)
        );
        assert!(progress.windows(2).all(|window| window[0] <= window[1]));
    }

    #[test]
    fn snapshot_removal_drops_deleted_entries_and_video_overrides() {
        let root = PathBuf::from(r"C:\root");
        let kept_video = root.join("kept.mp4");
        let removed_video = root.join("removed.mp4");
        let removed_image = root.join("removed.jpg");
        let kept_image = root.join("kept.jpg");
        let mut snapshot = SubfolderExpansionSnapshot {
            root: root.clone(),
            roots: vec![root],
            entries: Arc::new(vec![
                SubfolderExpansionEntry {
                    path: kept_video.clone(),
                    is_video: true,
                    mtime: 1,
                    file_size: 10,
                },
                SubfolderExpansionEntry {
                    path: removed_video.clone(),
                    is_video: true,
                    mtime: 2,
                    file_size: 20,
                },
                SubfolderExpansionEntry {
                    path: kept_image.clone(),
                    is_video: false,
                    mtime: 3,
                    file_size: 30,
                },
            ]),
            video_thumb_overrides: HashMap::from([
                (
                    crate::path_key::normalize_keep_drive(&kept_video),
                    kept_image.clone(),
                ),
                (
                    crate::path_key::normalize_keep_drive(&removed_video),
                    removed_image,
                ),
            ]),
            diag: SubfolderExpansionDiag {
                media_found: 3,
                ..Default::default()
            },
        };
        let removed = HashSet::from([crate::path_key::normalize_keep_drive(&removed_video)]);

        assert!(remove_paths_from_snapshot(&mut snapshot, &removed));
        assert_eq!(snapshot.entries.len(), 2);
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.path != removed_video)
        );
        assert_eq!(snapshot.video_thumb_overrides.len(), 1);
        assert!(
            snapshot
                .video_thumb_overrides
                .contains_key(&crate::path_key::normalize_keep_drive(&kept_video))
        );
        assert_eq!(snapshot.diag.media_found, 2);
    }
}
