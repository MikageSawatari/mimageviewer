//! 仮想スナップショットビュー共通の複数ルート再帰 walker。
//!
//! サブフォルダ展開とスマートフォルダで、cancel・深さ制限・reparse point loop 防止・
//! `GlobalIoSemaphore` / `ActivityGate` の規約を共有する。各ビュー固有の「1 ディレクトリを
//! 何として取り込むか」だけを callback に委ねる。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const SNAPSHOT_SORT_CHUNK_SIZE: usize = 16_384;
const SNAPSHOT_SORT_PROGRESS_INTERVAL: usize = 16_384;
const MAX_RECORDED_READ_DIR_FAILURES: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecursiveSnapshotWalkDiag {
    pub(crate) dirs_scanned: usize,
    pub(crate) read_dir_errors: usize,
    /// Directory reads that failed, with the OS error text captured at the I/O boundary.
    /// Keeping the path here lets aggregate views report which source was unavailable instead
    /// of reducing all failures to an opaque counter.
    pub(crate) read_dir_failures: Vec<(PathBuf, String)>,
    pub(crate) depth_limit_hits: usize,
    pub(crate) visited_skips: usize,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn walk_snapshot_roots<F, P>(
    roots: &[PathBuf],
    max_depth: u32,
    cancel: &AtomicBool,
    io_sem: Option<&crate::io_semaphore::GlobalIoSemaphore>,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    mut scan_directory: F,
    mut progress: P,
) -> RecursiveSnapshotWalkDiag
where
    F: FnMut(usize, &Path, std::fs::ReadDir, &AtomicBool) -> Vec<PathBuf>,
    P: FnMut(&RecursiveSnapshotWalkDiag, Option<&Path>),
{
    let mut diag = RecursiveSnapshotWalkDiag::default();
    let mut visited = HashSet::new();
    let mut stack: Vec<(PathBuf, u32, usize)> = roots
        .iter()
        .enumerate()
        .rev()
        .map(|(root_index, root)| (root.clone(), 0, root_index))
        .collect();

    while let Some((dir, depth, root_index)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if depth > max_depth {
            diag.depth_limit_hits += 1;
            continue;
        }
        if !crate::fs_entry::mark_directory_visited(&dir, &mut visited) {
            diag.visited_skips += 1;
            continue;
        }
        if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
            break;
        }
        let permit = match io_sem {
            Some(io_sem) => {
                io_sem.acquire_cancellable(crate::io_semaphore::IoPriority::Normal, cancel)
            }
            None => None,
        };
        if io_sem.is_some() && permit.is_none() {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                diag.read_dir_errors += 1;
                if diag.read_dir_failures.len() < MAX_RECORDED_READ_DIR_FAILURES {
                    diag.read_dir_failures
                        .push((dir.clone(), error.to_string()));
                }
                progress(&diag, Some(&dir));
                continue;
            }
        };
        let mut subdirs = scan_directory(root_index, &dir, entries, cancel);
        drop(permit);
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        diag.dirs_scanned += 1;
        progress(&diag, Some(&dir));
        subdirs.sort_by(|a, b| {
            a.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .cmp(
                    &b.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase(),
                )
        });
        for subdir in subdirs.into_iter().rev() {
            stack.push((subdir, depth + 1, root_index));
        }
    }

    progress(&diag, None);
    diag
}

/// 比較関数の途中で cancel 状態を変えず、chunk sort + merge の境界で応答する。
/// Rust sort の全順序契約を保ったまま巨大 snapshot の並べ替えを中止可能にする共通 helper。
pub(crate) fn cancelable_sorted_indices<F, P>(
    total: usize,
    cancel: &AtomicBool,
    compare: F,
    mut progress: P,
) -> Option<Vec<usize>>
where
    F: Fn(usize, usize) -> std::cmp::Ordering,
    P: FnMut(usize),
{
    if total == 0 {
        progress(0);
        return Some(Vec::new());
    }
    let chunk_size = SNAPSHOT_SORT_CHUNK_SIZE;
    let mut indices: Vec<usize> = (0..total).collect();
    for (chunk_index, chunk) in indices.chunks_mut(chunk_size).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        chunk.sort_unstable_by(|a, b| compare(*a, *b));
        progress(((chunk_index + 1) * chunk_size).min(total));
    }
    if total <= chunk_size {
        return Some(indices);
    }

    let mut scratch = vec![0usize; total];
    let mut source_in_indices = true;
    let mut run_width = chunk_size;
    while run_width < total {
        let (source, target) = if source_in_indices {
            (&indices[..], &mut scratch[..])
        } else {
            (&scratch[..], &mut indices[..])
        };
        let pair_width = run_width.saturating_mul(2).max(1);
        let mut written = 0usize;
        for start in (0..total).step_by(pair_width) {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let mid = start.saturating_add(run_width).min(total);
            let end = start.saturating_add(pair_width).min(total);
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
                if written.is_multiple_of(SNAPSHOT_SORT_PROGRESS_INTERVAL)
                    && cancel.load(Ordering::Relaxed)
                {
                    return None;
                }
            }
        }
        progress(total);
        source_in_indices = !source_in_indices;
        run_width = run_width.saturating_mul(2);
    }
    Some(if source_in_indices { indices } else { scratch })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walker_deduplicates_overlapping_roots_and_reports_progress() {
        let temp = tempfile::TempDir::new().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir_all(child.join("grandchild")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let diag = walk_snapshot_roots(
            &[child.clone(), temp.path().to_path_buf()],
            40,
            &cancel,
            None,
            None,
            |root_index, dir, entries, _| {
                seen.push((root_index, dir.to_path_buf()));
                entries
                    .flatten()
                    .filter_map(|entry| {
                        entry
                            .file_type()
                            .ok()
                            .filter(|kind| kind.is_dir())
                            .map(|_| entry.path())
                    })
                    .collect()
            },
            |_, _| {},
        );
        assert_eq!(diag.dirs_scanned, 3);
        assert!(diag.visited_skips >= 1);
        assert_eq!(seen[0], (0, child));
    }

    #[test]
    fn walker_observes_cancel_before_io() {
        let temp = tempfile::TempDir::new().unwrap();
        let cancel = AtomicBool::new(true);
        let diag = walk_snapshot_roots(
            &[temp.path().to_path_buf()],
            40,
            &cancel,
            None,
            None,
            |_, _, _, _| panic!("cancelled walk must not scan"),
            |_, _| {},
        );
        assert_eq!(diag.dirs_scanned, 0);
    }

    #[test]
    fn cancelable_sort_matches_standard_sort() {
        let values = [5, 1, 9, 3, 3, 7];
        let cancel = AtomicBool::new(false);
        let indices = cancelable_sorted_indices(
            values.len(),
            &cancel,
            |a, b| values[a].cmp(&values[b]).then_with(|| a.cmp(&b)),
            |_| {},
        )
        .unwrap();
        let sorted: Vec<_> = indices.into_iter().map(|index| values[index]).collect();
        assert_eq!(sorted, vec![1, 3, 3, 5, 7, 9]);
    }
}
