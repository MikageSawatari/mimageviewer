//! 名前索引 (Ctrl+S 検索用 `search_index_db`) のバックグラウンドバルクスキャナ。
//!
//! お気に入りで「名前」フル索引化フラグ (`auto_index_structure=true`) を ON にしたとき、
//! その favorite 配下を一度だけ再帰的に走査してフォルダ / ZIP / PDF 名を一括登録する。
//!
//! - 閲覧時追記 (`App::auto_index_current_folder`) はユーザーが開いたフォルダ直下のみを
//!   追記するので、未訪問のサブフォルダは索引に入らない。この module が埋める。
//! - メタ索引の supervisor とは独立に動く (別 DB、別スレッド、キャンセル独立)。
//! - Tantivy writer 制約は無いので複数 favorite を並列に走らせても問題ない。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::folder_tree::{is_apple_double, walk_dirs_recursive_with_progress};
use crate::indexer_progress::ProgressReporter;
use crate::search_index_db::{IndexEntry, IndexKind, SearchIndexDb};

/// バルクスキャン 1 回分の進捗と結果サマリ (将来 UI 表示するなら拡張)。
#[derive(Debug, Default, Clone, Copy)]
pub struct BulkSummary {
    pub folders_visited: usize,
    pub entries_written: usize,
    pub cancelled: bool,
}

/// favorite 1 つ分のバルクスキャンを同期実行する。通常は `std::thread::spawn` で包む。
///
/// - `fav_path`: お気に入りルート (絶対パス)
/// - `db`: 書き込み先 SQLite
/// - `cancel`: true になったら速やかに中断
///
/// 既存エントリとの衝突は `upsert_children` が同フォルダ配下を入れ替える挙動なので
/// 冪等に動作する (途中キャンセル後に再実行しても破綻しない)。
pub fn run_bulk_name_index(
    fav_path: &std::path::Path,
    db: &SearchIndexDb,
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
) -> BulkSummary {
    let mut summary = BulkSummary::default();

    if let Some(p) = progress {
        p.set(format!("スキャン開始: {}", fav_path.display()));
    }

    // Pass 1: サブフォルダ列挙
    let mut found: Vec<PathBuf> = Vec::new();
    walk_dirs_recursive_with_progress(fav_path, &mut found, cancel, &mut |cur| {
        if let Some(p) = progress {
            p.set(format!("フォルダ列挙: {}", cur.display()));
        }
    });
    if cancel.load(Ordering::Relaxed) {
        summary.cancelled = true;
        return summary;
    }
    summary.folders_visited = found.len();
    let total_folders = found.len();

    // Pass 2: 各フォルダ直下の Folder / ZipFile / PdfFile を集めて upsert
    for (i, folder) in found.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        if let Some(p) = progress {
            p.set(format!(
                "取込: {} ({}/{})",
                folder.display(),
                i + 1,
                total_folders
            ));
        }
        let mut children: Vec<IndexEntry> = Vec::new();
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if is_apple_double(&p) {
                continue;
            }
            // `entry.file_type()` は FindFirstFile の内部キャッシュから取るので追加 syscall なし
            // (Windows での per-entry metadata 取得は重いので避ける)
            let Ok(ft) = entry.file_type() else { continue };
            let Some(kind) = classify_name_index_kind(&p, &ft) else {
                continue;
            };
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            // mtime は 0 で良い (名前索引は新旧比較に使わない)
            children.push(IndexEntry {
                path: p,
                display_name: name,
                kind,
                mtime: 0,
            });
        }
        if children.is_empty() {
            continue;
        }
        // **Codex P2 race 対策**: upsert_children 直前にも cancel を確認する。
        // これで「UI が OFF に切り替えた → clear_for_favorite が走る → 直前の
        // in-flight upsert が race で書き戻す」窓を最小化する。
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        summary.entries_written += children.len();
        if let Err(e) = db.upsert_children(fav_path, folder, &children) {
            crate::logger::log(format!(
                "name_bulk_indexer: upsert_children failed for {}: {e}",
                folder.display()
            ));
        }
    }

    summary
}

/// DirEntry を名前索引の `IndexKind` に分類する共通ヘルパ。
/// index_creator / name_bulk_indexer から共有する (UI-responsiveness の観点で
/// `entry.file_type()` 経由で判定する経路に寄せる)。
pub fn classify_name_index_kind(
    path: &std::path::Path,
    file_type: &std::fs::FileType,
) -> Option<IndexKind> {
    if file_type.is_dir() {
        return Some(IndexKind::Folder);
    }
    if !file_type.is_file() {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str())?;
    if ext.eq_ignore_ascii_case("zip") {
        Some(IndexKind::ZipFile)
    } else if ext.eq_ignore_ascii_case("pdf") {
        Some(IndexKind::PdfFile)
    } else {
        None
    }
}

/// `std::thread::spawn` ラッパ。呼び出し側は `Arc<SearchIndexDb>` と `Arc<AtomicBool>` を
/// 渡して、スレッドハンドルを保持せずに投げ捨てる想定 (長期間走らないバルクなので)。
pub fn spawn_bulk(
    fav_path: PathBuf,
    db: Arc<SearchIndexDb>,
    cancel: Arc<AtomicBool>,
    progress: Option<ProgressReporter>,
) -> std::thread::JoinHandle<BulkSummary> {
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let summary = run_bulk_name_index(&fav_path, &db, &cancel, progress.as_ref());
        crate::logger::log(format!(
            "name_bulk_indexer: {} done in {} ms (folders={}, entries={}, cancelled={})",
            fav_path.display(),
            t0.elapsed().as_millis(),
            summary.folders_visited,
            summary.entries_written,
            summary.cancelled,
        ));
        if let Some(p) = progress {
            p.clear();
        }
        summary
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mkdir(p: &std::path::Path) {
        std::fs::create_dir_all(p).unwrap();
    }
    fn touch(p: &std::path::Path) {
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn bulk_collects_folders_zips_pdfs_and_ignores_other_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        let sub = root.join("sub");
        mkdir(&sub);
        touch(&root.join("a.zip"));
        touch(&root.join("b.pdf"));
        touch(&root.join("c.jpg")); // 画像は名前索引対象外
        touch(&sub.join("d.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, &cancel, None);

        // folders_visited = root + sub
        assert_eq!(summary.folders_visited, 2);
        // entries_written: root 直下 = sub(Folder) + a.zip + b.pdf = 3,
        //                  sub 直下  = d.zip = 1
        assert_eq!(summary.entries_written, 4);
        assert!(!summary.cancelled);

        // DB に登録されたエントリを count で確認 (root に 4 件入るはず)
        let count = db.count_for_favorite(&root).unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn bulk_respects_cancel_token() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("a"));
        mkdir(&root.join("b"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(true); // 最初から立てておく
        let summary = run_bulk_name_index(&root, &db, &cancel, None);
        assert!(summary.cancelled);
    }
}
