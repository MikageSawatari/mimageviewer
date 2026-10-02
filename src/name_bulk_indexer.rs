//! 名前索引 (Ctrl+S 検索用 `search_index_db`) のバックグラウンドバルクスキャナ。
//!
//! お気に入りで「名前」フル索引化フラグ (`auto_index_structure=true`) を ON にしたとき、
//! その favorite 配下を一度だけ再帰的に走査してフォルダ / ZIP / PDF / 動画名を一括登録する。
//!
//! - 閲覧時自動追記の経路は廃止された (`src/app.rs::load_folder` の "訪問時自動索引化は廃止"
//!   コメント参照)。現在は `NameIndexSupervisor` の起動時バルク + notify-rs 監視で
//!   全エントリを投入する。本 module はそのバルク部分の実装。
//! - メタ索引の supervisor とは独立に動く (別 DB、別スレッド、キャンセル独立)。
//! - Tantivy writer 制約は無いので複数 favorite を並列に走らせても問題ない。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::folder_tree::is_apple_double;
use crate::indexer_progress::ProgressReporter;
use crate::search_index_db::{IndexEntry, IndexKind, SearchIndexDb};

/// フル走査の read_dir 失敗を path ごとに 30 秒間抑制する logger。
struct ReadDirLogger {
    last_logged: HashMap<PathBuf, Instant>,
}

impl ReadDirLogger {
    fn new() -> Self {
        Self {
            last_logged: HashMap::new(),
        }
    }

    fn log(&mut self, p: &Path, e: &std::io::Error) {
        const COOLDOWN: Duration = Duration::from_secs(30);
        let now = Instant::now();
        let key = p.to_path_buf();
        let should_log = self
            .last_logged
            .get(&key)
            .is_none_or(|t| now.duration_since(*t) >= COOLDOWN);
        if should_log {
            crate::logger::log(format!(
                "name_bulk_indexer: read_dir failed for {}: {e}",
                p.display()
            ));
            self.last_logged.insert(key, now);
        }
    }
}

/// バルクスキャン 1 回分の進捗と結果サマリ (将来 UI 表示するなら拡張)。
#[derive(Debug, Default, Clone, Copy)]
pub struct BulkSummary {
    pub folders_visited: usize,
    pub entries_written: usize,
    /// 子集合が変化して実際に置換したフォルダ数 (空集合への置換も含む)。
    pub folders_written: usize,
    pub cancelled: bool,
    /// 走査中に `read_dir` / `file_type` / DB 更新のいずれかが失敗
    /// したか。`true` の場合は post-scan prune を skip する (不完全観測で正当な行を
    /// 消さないため — Codex P2 第 11 レビュー指摘)。
    pub had_error: bool,
}

/// favorite 1 つ分のバルクスキャンを同期実行する。通常は `std::thread::spawn` で包む。
///
/// - `fav_path`: お気に入りルート (絶対パス)
/// - `db`: 書き込み先 SQLite
/// - `cancel`: true になったら速やかに中断
///
/// 既存エントリとの衝突は `upsert_children` が同フォルダ配下を入れ替える挙動なので
/// 冪等に動作する (途中キャンセル後に再実行しても破綻しない)。
///
/// `activity_gate` を渡すと、各フォルダの処理前に UI 操作が静穏になるまで待機する
/// (2026-04 F: 操作中は bulk スキャンを一時停止)。
pub fn run_bulk_name_index(
    fav_path: &std::path::Path,
    db: &SearchIndexDb,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    excluded_roots: &[PathBuf],
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
) -> BulkSummary {
    run_bulk_name_index_with_reader(
        fav_path,
        db,
        activity_gate,
        excluded_roots,
        cancel,
        progress,
        &mut |path| std::fs::read_dir(path),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_bulk_name_index_with_reader<I>(
    fav_path: &Path,
    db: &SearchIndexDb,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    excluded_roots: &[PathBuf],
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
    read_dir: &mut impl FnMut(&Path) -> std::io::Result<I>,
) -> BulkSummary
where
    I: IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
{
    let mut summary = BulkSummary::default();

    if let Some(p) = progress {
        p.set(format!("スキャン開始: {}", fav_path.display()));
    }

    // post-scan prune の cutoff。`next_write_stamp` はプロセス全域で厳密単調増加 (AtomicI64)
    // なので、直後に走る upsert の stamp は常に `> scan_start_stamp`。
    // `<` 比較で「未観測 stale 行 / 今回 scan が触った新しい行」を race-free に分離できる。
    // (旧実装は秒精度で、同秒に連続スキャンが入ると stale 行が残留するバグがあった)
    let scan_start_stamp = crate::search_index_db::next_write_stamp();

    let total_folders = match db.previous_folder_count(fav_path) {
        Ok(count) => count,
        Err(e) => {
            crate::logger::log(format!("name_bulk_indexer: folder count failed: {e}"));
            None
        }
    };
    let mut read_dir_logger = ReadDirLogger::new();
    let mut had_error = false;
    // canonical key は循環検出専用。prune は DB と同じ列挙パスの key を使う。
    let mut cycle_keys = HashSet::new();
    let mut visited_parents = HashSet::new();
    let mut pending = vec![(fav_path.to_path_buf(), 0u32)];
    const MAX_WALK_DEPTH: u32 = 64;
    while let Some((folder, depth)) = pending.pop() {
        if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
            summary.cancelled = true;
            break;
        }
        if depth > MAX_WALK_DEPTH
            || crate::books::path_is_under_any(&folder, excluded_roots)
            || !crate::fs_entry::mark_directory_visited(&folder, &mut cycle_keys)
        {
            continue;
        }
        // 旧 DFS と同じく、存在しない root は完全な空走査として stale を消す。
        if depth == 0 && !folder.is_dir() {
            continue;
        }
        if let Some(p) = progress {
            let display = folder.strip_prefix(fav_path).unwrap_or(&folder).display();
            p.set(format!("フォルダ列挙 {}", display));
        }
        let entries = match read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) => {
                had_error = true;
                read_dir_logger.log(&folder, &e);
                continue;
            }
        };
        // 1 回の read_dir から索引対象と子フォルダを両方得る。
        let mut subfolders = Vec::new();
        let (children, had_entry_error) = collect_index_entries_with_cancel(
            entries,
            "name_bulk_indexer",
            activity_gate.map(|g| (g, cancel)),
            excluded_roots,
            Some(cancel),
            Some(&mut subfolders),
        );
        // 観測できた子の走査は続けるが、不完全な親を置換してはならない。
        pending.extend(subfolders.into_iter().rev().map(|path| (path, depth + 1)));
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        if had_entry_error {
            had_error = true;
            crate::logger::log(format!(
                "name_bulk_indexer: skipping upsert_children for {} (per-entry error: incomplete observation)",
                folder.display()
            ));
            continue;
        }
        let mut parent_key = crate::search_index_db::normalize_path(&folder);
        // DB の子 key から得る親と揃える (drive root と filesystem root は '/' を保持)。
        if parent_key != "/" && !parent_key.ends_with(":/") {
            parent_key.truncate(parent_key.trim_end_matches('/').len());
        }
        visited_parents.insert(parent_key);
        summary.folders_visited += 1;
        if let Some(p) = progress {
            let display = folder.strip_prefix(fav_path).unwrap_or(&folder).display();
            let current = summary.folders_visited as u64;
            if let Some(total) = total_folders {
                p.set_msg_and_count(
                    format!("取込 ({current}/{total}) {display}"),
                    current,
                    total,
                );
            } else {
                p.set_msg_and_count(format!("取込 ({current}) {display}"), current, 0);
            }
        }
        // 空になったフォルダも比較し、旧行があれば DELETE を実行する。
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        match db.upsert_children_if_changed(fav_path, &folder, &children) {
            Ok(true) => {
                summary.folders_written += 1;
                summary.entries_written += children.len();
            }
            Ok(false) => {}
            Err(e) => {
                had_error = true;
                crate::logger::log(format!(
                    "name_bulk_indexer: upsert_children failed for {}: {e}",
                    folder.display()
                ));
            }
        }
    }
    summary.cancelled |= cancel.load(Ordering::Relaxed);

    summary.had_error = had_error;

    if let Some(p) = progress {
        // バルク取込完了 — ETA カウントをクリアして UI から残り時間を消す。
        p.clear_count();
    }

    // stale 行を一掃 (cancel / per-entry エラー / read_dir 失敗 / upsert 失敗のいずれかで
    // 不完全観測の場合は skip — 観測できなかった正当な行を消さないため)。
    if !summary.cancelled && !summary.had_error {
        match db.prune_stale_for_favorite(fav_path, &visited_parents, scan_start_stamp) {
            Ok(0) => {}
            Ok(n) => crate::logger::log(format!(
                "name_bulk_indexer: pruned {n} stale rows under {}",
                fav_path.display()
            )),
            Err(e) => crate::logger::log(format!(
                "name_bulk_indexer: prune_stale_for_favorite failed for {}: {e}",
                fav_path.display()
            )),
        }
    } else if summary.had_error {
        crate::logger::log(format!(
            "name_bulk_indexer: skipping prune_stale_for_favorite for {} (incomplete scan: cancelled={}, had_error={})",
            fav_path.display(),
            summary.cancelled,
            summary.had_error
        ));
    }

    summary
}

/// `read_dir` の `ReadDir` イテレータから索引対象の `IndexEntry` 群を集める共通ヘルパ。
///
/// 戻り値の `bool` は「per-entry エラー (DirEntry::Err / file_type() 失敗) が 1 件以上
/// あったか」を示す。`true` の場合、呼び出し側は「不完全観測」として扱い、後続の
/// `prune_*` 系を **skip すべき** (Codex P2 第 11 レビュー指摘: `entries.flatten()` や
/// `let Ok(ft) = ... else { continue };` で silent skip すると、観測できなかった行が
/// stale 扱いで消える事故が起きる)。
///
/// `log_prefix` は per-entry エラー時のログ前缀 (例: `"name_bulk_indexer"` /
/// `"name_index Pass 2"`)。
///
/// なお `upsert_children` の DELETE は this 関数が返した `children` に含まれない
/// 直下行を消す best-effort 動作なので、`had_entry_error == true` のときに上層で
/// 不完全なフォルダでは upsert と post-scan prune の両方を skip する。
pub fn collect_index_entries(
    entries: std::fs::ReadDir,
    log_prefix: &str,
    yield_check: Option<(&crate::activity_gate::ActivityGate, &AtomicBool)>,
    excluded_roots: &[PathBuf],
) -> (Vec<IndexEntry>, bool) {
    collect_index_entries_with_cancel(entries, log_prefix, yield_check, excluded_roots, None, None)
}

fn collect_index_entries_with_cancel(
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
    log_prefix: &str,
    yield_check: Option<(&crate::activity_gate::ActivityGate, &AtomicBool)>,
    excluded_roots: &[PathBuf],
    cancel: Option<&AtomicBool>,
    mut subfolders: Option<&mut Vec<PathBuf>>,
) -> (Vec<IndexEntry>, bool) {
    // 数千件規模のフォルダで `file_type()` を per-entry に呼ぶと HDD 上で
    // 数百 ms-1s 単位の I/O 連続が発生し、その間に動画オープン等の高優先 I/O が
    // 競合する。64 件ごとに ActivityGate を待ち、ユーザー操作中は次のバッチに
    // 進まないようにする (= bump から最大 64 entry 分で indexer が停止)。
    const YIELD_EVERY_N: usize = 64;
    let mut children: Vec<IndexEntry> = Vec::new();
    let mut had_entry_error = false;
    let mut processed: usize = 0;
    for entry_result in entries {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            had_entry_error = true;
            break;
        }
        if processed > 0
            && processed % YIELD_EVERY_N == 0
            && let Some((gate, cancel)) = yield_check
        {
            gate.wait_until_idle(cancel);
            if cancel.load(Ordering::Relaxed) {
                // 観測不完全 → 呼び出し側で upsert を skip させる (Codex P2 第 12
                // レビュー以来の規約)。途中まで集めた children は破棄される。
                had_entry_error = true;
                break;
            }
        }
        processed += 1;
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                had_entry_error = true;
                crate::logger::log(format!("{log_prefix}: dir entry read failed: {e}"));
                continue;
            }
        };
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        let p = entry.path();
        if is_apple_double(&p) && subfolders.is_none() {
            continue;
        }
        if crate::books::path_is_under_any(&p, excluded_roots) {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                had_entry_error = true;
                crate::logger::log(format!(
                    "{log_prefix}: file_type failed for {}: {e}",
                    p.display()
                ));
                continue;
            }
        };
        let Some(kind) = classify_name_index_kind(&p, &entry, &ft) else {
            continue;
        };
        // 旧 DFS は ._* ディレクトリにも入るが、そのディレクトリ自身は索引行にしない。
        // 再帰先と索引対象を独立に集め、単一走査でも同じ範囲を保つ。
        if kind == IndexKind::Folder
            && let Some(subfolders) = subfolders.as_mut()
        {
            subfolders.push(p.clone());
        }
        if is_apple_double(&p) {
            continue;
        }
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        children.push(IndexEntry {
            path: p,
            display_name: name,
            kind,
            mtime: 0,
        });
    }
    (children, had_entry_error)
}

/// DirEntry を名前索引の `IndexKind` に分類する共通ヘルパ。
/// index_creator / name_bulk_indexer から共有する (UI-responsiveness の観点で
/// `entry.file_type()` 経由で判定する経路に寄せる)。
pub fn classify_name_index_kind(
    path: &std::path::Path,
    entry: &std::fs::DirEntry,
    file_type: &std::fs::FileType,
) -> Option<IndexKind> {
    let kind = crate::fs_entry::classify_dir_entry(entry, file_type);
    if kind.is_directory() {
        return Some(IndexKind::Folder);
    }
    if !kind.is_file() {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str())?;
    // 動画はコンテナではないので、コンテナ索引 (Ctrl+S) の対象から外す
    // (docs/search-container-item-redesign.md §4.2)。動画はアイテム索引 (Ctrl+G)
    // で扱う。`IndexKind::VideoFile` variant 自体は stale 行の読み取り用に残すが、
    // 書き込み経路 (= ここ) では生成しない。
    // 拡張子の綴りは仮想フォルダ判定と共有する。ここで `zip` を直接書いていた頃は
    // `.cbz` が索引に入らず、他の全経路がネイティブ ZIP として開けるコンテナなのに
    // Ctrl+S だけヒットしなかった (docs/item-kind-capability-matrix.md §6-12)。
    let ext = ext.to_ascii_lowercase();
    if crate::folder_tree::is_zip_extension(&ext) {
        Some(IndexKind::ZipFile)
    } else if crate::folder_tree::is_paged_document_path(path) {
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
        let summary = run_bulk_name_index(&fav_path, &db, None, &[], &cancel, progress.as_ref());
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
        touch(&root.join("book.epub"));
        touch(&root.join("c.jpg")); // 画像は名前索引対象外
        touch(&sub.join("d.zip"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);

        // folders_visited = root + sub
        assert_eq!(summary.folders_visited, 2);
        // entries_written: root 直下 = sub(Folder) + a.zip + b.pdf + book.epub = 4,
        //                  sub 直下  = d.zip = 1
        assert_eq!(summary.entries_written, 5);
        assert!(!summary.cancelled);

        // DB に登録されたエントリを count で確認 (計 5 件)
        let count = db.count_for_favorite(&root).unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn bulk_index_ignores_sibling_output_temp_during_save() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root);
        touch(&root.join("book.pdf"));
        let mut cache = crate::epub_cache::EpubCache::open_at(&tmp.path().join("data")).unwrap();
        let reserved = cache
            .reserve_sibling_output(&root.join("new-book.pdf"))
            .unwrap();
        let _held_temp = reserved.create_file().unwrap();
        let token = "a".repeat(64);
        touch(&root.join(format!(".miv-part-{token}.pdf")));
        touch(&root.join(format!(".miv-part-{token}.tmp")));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        assert_eq!(summary.entries_written, 1);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 1);
    }

    /// `.cbz` は他の全経路でネイティブ ZIP として開けるコンテナなのに、名前索引の
    /// 分類だけが拡張子 `zip` の厳密一致だったため Ctrl+S に出てこなかった
    /// (docs/item-kind-capability-matrix.md §6-12)。大文字混じりも同じに扱う。
    #[test]
    fn bulk_indexes_cbz_like_any_other_zip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root);
        touch(&root.join("a.cbz"));
        touch(&root.join("b.CBZ"));
        touch(&root.join("c.zip"));
        touch(&root.join("d.PDF"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);

        assert_eq!(summary.entries_written, 4);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 4);
    }

    #[test]
    fn bulk_excludes_configured_roots() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        let books_root = root.join("books");
        let book = books_root.join("名前なし");
        mkdir(&book);
        touch(&root.join("keep.zip"));
        touch(&book.join("0001_page.png"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[books_root], &cancel, None);

        assert_eq!(summary.folders_visited, 1);
        assert_eq!(summary.entries_written, 1);
        let roots = vec![root.clone()];
        assert_eq!(
            db.search("books", &roots, None, crate::search_query::MatchMode::And)
                .unwrap()
                .len(),
            0,
            "除外 root 自体を Folder 行として入れない"
        );
        assert_eq!(
            db.search("keep", &roots, None, crate::search_query::MatchMode::And)
                .unwrap()
                .len(),
            1
        );
    }

    /// B2 (Codex P2 レビュー反映): depth 3 のサブツリーが正しく再帰投入されることを
    /// supervisor を経由せずに bulk 本体だけで固定する。
    /// 既存 `bulk_collects_folders_zips_pdfs_and_ignores_other_files` は depth 2 までしか
    /// カバーしていなかった。
    #[test]
    fn bulk_indexes_zip_pdf_at_depth_three() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        // root/a/b/c/leaf.zip と leaf.pdf。leaf.mp4 はコンテナ索引対象外 (§4.2)。
        let c = root.join("a").join("b").join("c");
        mkdir(&c);
        touch(&c.join("leaf.zip"));
        touch(&c.join("leaf.pdf"));
        touch(&c.join("leaf.mp4"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);

        // folders_visited = root + a + b + c = 4
        assert_eq!(summary.folders_visited, 4);
        // entries_written: root直下=a(Folder)=1, a直下=b(Folder)=1, b直下=c(Folder)=1,
        //                  c直下=leaf.zip + leaf.pdf = 2 (mp4 は除外) → 合計 5
        assert_eq!(summary.entries_written, 5);
        assert!(!summary.cancelled);

        // 深い場所の ZIP / PDF は検索ヒット (動画は除外)
        let leaf = db
            .search(
                "leaf",
                &[root.clone()],
                None,
                crate::search_query::MatchMode::And,
            )
            .unwrap();
        assert_eq!(
            leaf.len(),
            2,
            "leaf.zip + leaf.pdf が 2 件ヒット (leaf.mp4 は除外)"
        );
        let kinds: Vec<IndexKind> = leaf.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&IndexKind::ZipFile));
        assert!(kinds.contains(&IndexKind::PdfFile));
    }

    #[test]
    fn bulk_respects_cancel_token() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("a"));
        mkdir(&root.join("b"));

        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(true); // 最初から立てておく
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        assert!(summary.cancelled);
    }

    #[test]
    fn bulk_second_scan_skips_all_writes_and_reads_each_folder_once() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("sub/empty"));
        touch(&root.join("sub/book.zip"));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let first = run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        assert_eq!(first.folders_written, 2);
        let mut reads = HashMap::new();
        let second =
            run_bulk_name_index_with_reader(&root, &db, None, &[], &cancel, None, &mut |path| {
                *reads.entry(path.to_path_buf()).or_insert(0) += 1;
                std::fs::read_dir(path)
            });
        assert_eq!(second.folders_visited, 3);
        assert_eq!(second.folders_written, 0);
        assert_eq!(second.entries_written, 0);
        assert!(!second.had_error);
        assert_eq!(reads.len(), 3);
        assert!(reads.values().all(|count| *count == 1));
        assert_eq!(db.count_for_favorite(&root).unwrap(), 3);
    }

    #[test]
    fn bulk_preserves_traversal_into_apple_double_named_directories() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("._folder"));
        touch(&root.join("._folder/book.zip"));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        assert_eq!(summary.folders_visited, 2);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 1);
        assert_eq!(
            run_bulk_name_index(&root, &db, None, &[], &cancel, None).folders_written,
            0
        );
    }

    #[test]
    fn bulk_excluded_root_and_missing_root_prune_old_rows() {
        for excluded in [true, false] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().join("fav");
            mkdir(&root.join("sub"));
            touch(&root.join("sub/book.pdf"));
            let db = SearchIndexDb::open_in_memory().unwrap();
            let cancel = AtomicBool::new(false);
            run_bulk_name_index(&root, &db, None, &[], &cancel, None);
            let exclusions = if excluded {
                vec![tmp.path().to_path_buf()]
            } else {
                std::fs::remove_dir_all(&root).unwrap();
                vec![]
            };
            let summary = run_bulk_name_index(&root, &db, None, &exclusions, &cancel, None);
            assert_eq!(summary.folders_visited, 0);
            assert!(!summary.had_error);
            assert_eq!(db.count_for_favorite(&root).unwrap(), 0);
        }
    }

    #[test]
    fn bulk_empty_folder_and_deleted_deep_subtree_remove_stale_rows() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root.join("keep"));
        mkdir(&root.join("gone/a/b"));
        touch(&root.join("keep/book.zip"));
        touch(&root.join("gone/a/b/book.pdf"));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        std::fs::remove_file(root.join("keep/book.zip")).unwrap();
        std::fs::remove_dir_all(root.join("gone")).unwrap();
        let summary = run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        assert_eq!(summary.folders_written, 2);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 1);
    }

    #[test]
    fn bulk_incomplete_listing_skips_folder_replace_and_prune() {
        for per_entry_error in [false, true] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().join("fav");
            mkdir(&root.join("sub"));
            touch(&root.join("sub/book.zip"));
            let db = SearchIndexDb::open_in_memory().unwrap();
            let cancel = AtomicBool::new(false);
            run_bulk_name_index(&root, &db, None, &[], &cancel, None);
            let summary = run_bulk_name_index_with_reader(
                &root,
                &db,
                None,
                &[],
                &cancel,
                None,
                &mut |path| {
                    if path == root.join("sub") {
                        let error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
                        if per_entry_error {
                            Ok(vec![Err(error)])
                        } else {
                            Err(error)
                        }
                    } else {
                        Ok(std::fs::read_dir(path)?.collect::<Vec<_>>())
                    }
                },
            );
            assert!(summary.had_error);
            assert_eq!(summary.folders_written, 0);
            assert_eq!(db.count_for_favorite(&root).unwrap(), 2);
        }
    }

    #[test]
    fn bulk_cancel_during_listing_preserves_existing_children() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root);
        touch(&root.join("book.zip"));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        let summary =
            run_bulk_name_index_with_reader(&root, &db, None, &[], &cancel, None, &mut |path| {
                cancel.store(true, Ordering::Relaxed);
                std::fs::read_dir(path)
            });
        assert!(summary.cancelled);
        assert_eq!(summary.folders_written, 0);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 1);
    }

    #[test]
    fn bulk_prune_keeps_rows_written_after_scan_start() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("fav");
        mkdir(&root);
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let concurrent_parent = root.join("not_observed");
        let summary =
            run_bulk_name_index_with_reader(&root, &db, None, &[], &cancel, None, &mut |path| {
                db.upsert_children(
                    &root,
                    &concurrent_parent,
                    &[IndexEntry {
                        path: concurrent_parent.join("new.zip"),
                        display_name: "new.zip".into(),
                        kind: IndexKind::ZipFile,
                        mtime: 0,
                    }],
                )
                .unwrap();
                std::fs::read_dir(path)
            });
        assert!(!summary.had_error);
        assert_eq!(db.count_for_favorite(&root).unwrap(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn bulk_ancestor_case_change_updates_display_paths() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("MixedCase");
        mkdir(&root.join("sub"));
        touch(&root.join("sub/book.zip"));
        let db = SearchIndexDb::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        run_bulk_name_index(&root, &db, None, &[], &cancel, None);
        let lower_root = tmp.path().join("mixedcase");
        let second = run_bulk_name_index(&lower_root, &db, None, &[], &cancel, None);
        assert_eq!(second.folders_written, 2);
        let hits = db
            .search("book", &[root], None, crate::search_query::MatchMode::And)
            .unwrap();
        assert_eq!(hits[0].path, lower_root.join("sub/book.zip"));
    }
}
