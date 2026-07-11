//! 起動時差分走査 (docs/search-expansion-design.md §7.4)。
//!
//! お気に入りルートを再帰的に walk し、現在の FS 状態と `fts_meta.db` の
//! 登録状態を 3-way diff して「ingest すべき path」「削除すべき path」を返す。
//!
//! ## 重要な制約 (CLAUDE.md §UI スレッド同期 I/O)
//!
//! - `entry.file_type()` を使う (`Path::is_dir()` / `is_file()` は `GetFileAttributes` syscall を
//!   per-entry で呼ぶため数百ファイルで 500-1000ms ブロックになる)
//! - UI スレッドから呼ばない。専用スレッドで実行する
//! - `GlobalIoSemaphore` で read_dir の同時実行を制御する (§7.5)
//! - キャンセルトークンで中断可能 (大量のお気に入り走査中に終了されても OK)
//!
//! ## 本モジュールのスコープ (§16 step 6)
//!
//! - FS walker + 3-way diff の計算のみ
//! - メタ抽出・Tantivy commit は **このモジュールの責務外**
//!   (Ingest Worker = §16 step 9 に切り出す)
//! - ZIP はアイテム索引 (Ctrl+G) の対象外なので候補に含めない
//!   (docs/search-container-item-redesign.md §3.2)。ZIP のコンテナ検索は Ctrl+S 専属。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

use crate::folder_tree;
use crate::fts_meta::FtsMetaDb;
use crate::indexer_progress::ProgressReporter;
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::search_index_db::normalize_path;

/// 1 候補ファイル (通常画像 / PDF / 動画)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateFile {
    /// 絶対パス (表示・I/O 用)
    pub abs_path: PathBuf,
    /// 正規化済み DB キー (`normalize_path` 済み)
    pub key: String,
    pub kind: CandidateKind,
    /// **表示用** mtime (画像本体)。Tantivy doc に入り Ctrl+G 一覧の日付ソートに使う。
    pub mtime: i64,
    /// **表示用** file_size (画像本体)。
    pub file_size: i64,
    /// **差分用** mtime。画像はサイドカーを織り込んだ `max(画像, サイドカー)`。
    /// fts_meta に保存され walker の 3-way diff で比較される。サイドカーの追加・編集を
    /// 「変化あり」として検出するため (docs/sidecar-metadata-ingest.md §14-3/§14-4)。
    pub diff_mtime: i64,
    /// **差分用** size。画像はサイドカーの size を加味した `画像 size + サイドカー size`
    /// (サイドカー無しは画像 size)。サイドカーの追加・削除を size 変化として検出するため。
    pub diff_size: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    /// ネイティブ対応画像 (Susie プラグイン拡張含む)
    Image,
    /// PDF (v1 は document info のみ ingest、本文は対象外)
    Pdf,
    /// 動画 (ファイル名 + mXD XMP + sidecar tags + container metadata)
    Video,
    /// 音声 (ファイル名のみ。ID3 等の埋め込みタグは対象外)
    Audio,
}

/// 3-way diff 結果。
#[derive(Debug, Default)]
pub struct ScanResult {
    /// FS にあり DB に無い、または mtime/size が変化したもの → ingest キュー対象
    pub to_ingest: Vec<CandidateFile>,
    /// DB にあり FS にないもの → tombstone (削除) 対象
    pub to_delete: Vec<String>,
    /// 差分なしの件数 (進捗表示用)
    pub unchanged: usize,
    /// 走査中に encountered した候補ファイル総数 (stats 用)
    pub total_scanned: usize,
    /// 診断統計 (Codex 6 回目 nice-to-have #2): インデックス管理ダイアログの
    /// トラブルシューティング表示で使える
    pub diag: ScanDiag,
}

/// walker の診断統計。read_dir / file_type / metadata 失敗の件数を持つ。
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanDiag {
    /// std::fs::read_dir が失敗した回数 (典型例: アクセス拒否フォルダ)
    pub read_dir_errors: usize,
    /// DirEntry::file_type() が失敗した回数 (稀)
    pub file_type_errors: usize,
    /// DirEntry::metadata() が失敗した回数 (削除競合等)
    pub metadata_errors: usize,
    /// 最大深度 (MAX_DEPTH) に到達して打ち切ったディレクトリ数
    pub depth_limit_hits: usize,
}

/// 走査開始パラメータ。
pub struct ScanParams {
    pub favorite_id: Uuid,
    pub root: PathBuf,
    pub excluded_roots: Vec<PathBuf>,
    pub cancel: Arc<AtomicBool>,
    /// "今どこを walk してる" を UI に見せるためのレポーター。
    /// None なら通知しない (テスト等で便利)。
    pub progress: Option<ProgressReporter>,
}

/// 進捗通知 (UI への stream)。Walker は I/O-bound なので頻繁に通知しすぎないこと。
pub enum WalkerEvent {
    /// 定期的な進捗
    Progress {
        scanned: usize,
        current_dir: Option<PathBuf>,
    },
    /// 完了
    Done(ScanResult),
    /// エラー (フォルダが消えた等)
    Error(String),
}

/// お気に入りルートを走査して 3-way diff を計算する。
///
/// `io_sem` は read_dir 呼び出しの順番制御用。複数お気に入りを並列走査する場合は
/// 同じセマフォを共有することでグローバル I/O 同時実行数を制御できる。
/// `activity_gate` を渡すと、walker が各ディレクトリの `read_dir` 前にユーザー操作を待つ
/// (2026-04 F: **Codex P2 対応**、walker phase も操作中停止の対象にする)。
///
/// 呼び出し側は典型的に別スレッドで実行し、結果を mpsc で受け取る。
pub fn scan(
    params: ScanParams,
    db: &FtsMetaDb,
    io_sem: &GlobalIoSemaphore,
    priority: IoPriority,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) -> Result<ScanResult, String> {
    let ScanParams {
        favorite_id,
        root,
        excluded_roots,
        cancel,
        progress,
    } = params;

    // 1. FS を walk して候補を集める
    let mut fs_map = std::collections::HashMap::<String, CandidateFile>::new();
    let mut diag = ScanDiag::default();
    let mut visited = std::collections::HashSet::new();
    walk_dir_recursive(
        &root,
        io_sem,
        priority,
        activity_gate,
        &cancel,
        progress.as_ref(),
        &mut fs_map,
        &mut diag,
        0,
        &mut visited,
        &excluded_roots,
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    // 2. DB 側の登録一覧を取得
    let db_entries = db
        .list_favorite_files(favorite_id)
        .map_err(|e| format!("fts_meta list failed: {e}"))?;
    let db_map: std::collections::HashMap<String, (i64, i64)> = db_entries
        .into_iter()
        .map(|(p, m, s)| (p, (m, s)))
        .collect();

    // 3. 3-way diff
    let mut result = ScanResult::default();
    result.total_scanned = fs_map.len();
    result.diag = diag;

    for (key, cand) in &fs_map {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match db_map.get(key) {
            None => {
                // FS only → 新規 ingest
                result.to_ingest.push(cand.clone());
            }
            Some(&(db_mtime, db_size)) => {
                // 差分判定は **差分用** 署名 (画像 + サイドカーを織り込んだ値) で行う。
                // fts_meta には ingest_worker が diff_mtime/diff_size を保存している。
                if db_mtime == cand.diff_mtime && db_size == cand.diff_size {
                    result.unchanged += 1;
                } else {
                    // 変化あり (本体 or サイドカーの追加/編集/削除) → 再 ingest
                    result.to_ingest.push(cand.clone());
                }
            }
        }
    }
    for key in db_map.keys() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if !fs_map.contains_key(key) {
            result.to_delete.push(key.clone());
        }
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn walk_dir_recursive(
    dir: &Path,
    io_sem: &GlobalIoSemaphore,
    priority: IoPriority,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
    out: &mut std::collections::HashMap<String, CandidateFile>,
    diag: &mut ScanDiag,
    depth: u32,
    visited: &mut std::collections::HashSet<String>,
    excluded_roots: &[PathBuf],
) -> Result<(), String> {
    // 安全策: シンボリックループ対策 (深さ制限)。通常フォルダは 20 階層あれば十分
    const MAX_DEPTH: u32 = 40;
    if depth > MAX_DEPTH {
        diag.depth_limit_hits += 1;
        return Ok(());
    }
    if crate::books::path_is_under_any(dir, excluded_roots) {
        return Ok(());
    }
    if !crate::fs_entry::mark_directory_visited(dir, visited) {
        return Ok(());
    }

    // read_dir 1 回は通常 <10ms だが HDD/NAS で数百ディレクトリ連続すると操作中の
    // サムネ I/O と競合する。ディレクトリ単位で ActivityGate を待てば操作中は walk
    // が停止する。cancel check も兼ねる。
    if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
        return Ok(());
    }

    // このディレクトリに入る時点で UI に通知 (1 ディレクトリ 1 回なので mutex 競合は軽微)
    if let Some(p) = progress {
        p.set(format!("スキャン: {} ({} 件)", dir.display(), out.len()));
    }

    let Some(_permit) = io_sem.acquire_cancellable(priority, cancel) else {
        return Ok(());
    };
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => {
            diag.read_dir_errors += 1;
            return Ok(());
        }
    };
    // read_dir 中は permit を握ったまま全エントリを舐める
    let entries: Vec<_> = rd.flatten().collect();
    drop(_permit); // read_dir 完了後は permit を返し、子 walk 時に再取得

    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        // ★ file_type() は entry がキャッシュしているので syscall なし
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => {
                diag.file_type_errors += 1;
                continue;
            }
        };
        let path = entry.path();

        if folder_tree::is_apple_double(&path) {
            continue;
        }
        if crate::books::path_is_under_any(&path, excluded_roots) {
            continue;
        }

        let entry_kind = crate::fs_entry::classify_dir_entry(&entry, &file_type);
        if entry_kind.is_directory() {
            subdirs.push(path);
            continue;
        }
        if !entry_kind.is_file() {
            continue; // device 等はスキップ
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // ZIP はアイテム索引 (Ctrl+G) の対象外。候補に含めないことで、既存の ZIP doc は
        // 3-way diff で「FS になし + DB あり」と判定され to_delete に落ちて Tantivy から
        // 消える (docs/search-container-item-redesign.md §3.2)。
        // `is_recognized_image_ext` は Susie プラグインが申告した拡張子も画像扱いにする
        // ため、アーカイブ系 Susie プラグインが "zip" を申告しても確実に除外できるよう、
        // 拡張子分類より前に明示的に弾く (Codex P2)。
        if ext == "zip" {
            continue;
        }
        let kind = if ext == "pdf" {
            CandidateKind::Pdf
        } else if folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            CandidateKind::Video
        } else if folder_tree::is_audio_ext(&ext) {
            CandidateKind::Audio
        } else if folder_tree::is_recognized_image_ext(&ext) {
            CandidateKind::Image
        } else {
            continue;
        };

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                diag.metadata_errors += 1;
                continue;
            }
        };
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let file_size = metadata.len() as i64;

        // 差分用署名: 画像はサイドカー (同名 .json/.txt) の mtime/size を織り込む。
        // これでサイドカーの追加・編集・削除が 3-way diff で検出される (§14-3/§14-4)。
        // 検出は per-file の存在チェック (最大 4 回) + サイドカー 1 件の stat。
        let (diff_mtime, diff_size) = if kind == CandidateKind::Image {
            match crate::external_metadata::sidecar_signature(&path) {
                Some(sig) => (mtime.max(sig.mtime), file_size + sig.fingerprint),
                None => (mtime, file_size),
            }
        } else {
            (mtime, file_size)
        };

        let key = normalize_path(&path);
        out.insert(
            key.clone(),
            CandidateFile {
                abs_path: path,
                key,
                kind,
                mtime,
                file_size,
                diff_mtime,
                diff_size,
            },
        );
    }

    // 子ディレクトリを再帰 (permit は再帰先で取り直す)
    for sub in subdirs {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        walk_dir_recursive(
            &sub,
            io_sem,
            priority,
            activity_gate,
            cancel,
            progress,
            out,
            diag,
            depth + 1,
            visited,
            excluded_roots,
        )?;
    }
    Ok(())
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::IndexKind;
    use std::fs;
    use tempfile::TempDir;

    fn make_file(dir: &Path, name: &str, content: &[u8]) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn tmp_db() -> (TempDir, FtsMetaDb) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        (dir, db)
    }

    fn scan_sync(fav_id: Uuid, root: &Path, db: &FtsMetaDb) -> ScanResult {
        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        scan(
            ScanParams {
                favorite_id: fav_id,
                root: root.to_path_buf(),
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap()
    }

    #[test]
    fn empty_fs_empty_db_returns_zero() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("photos");
        fs::create_dir_all(&root).unwrap();
        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 0);
        assert!(r.to_ingest.is_empty());
        assert!(r.to_delete.is_empty());
    }

    #[test]
    fn new_files_go_to_ingest() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("p");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"xx");
        make_file(&root, "b.png", b"yy");
        make_file(&root, "ignore.txt", b"zz");
        // ZIP はアイテム索引の対象外なので候補にならない (§3.2)
        make_file(&root, "archive.zip", b"PK");
        make_file(&root, "doc.pdf", b"%PDF");
        make_file(&root, "clip.mp4", b"fake mp4");
        make_file(&root, "song.MP3", b"fake mp3");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(
            r.total_scanned, 5,
            "jpg+png+pdf+mp4+mp3 の 5 つ (zip/txt は除外)"
        );
        assert_eq!(r.to_ingest.len(), 5);
        assert_eq!(r.unchanged, 0);
        assert!(r.to_delete.is_empty());

        let kinds: Vec<_> = r.to_ingest.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&CandidateKind::Image));
        assert!(kinds.contains(&CandidateKind::Pdf));
        assert!(kinds.contains(&CandidateKind::Video));
        assert!(kinds.contains(&CandidateKind::Audio));
    }

    #[test]
    fn excluded_roots_are_not_scanned_and_stale_rows_are_deleted() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("p");
        let books_root = root.join("books");
        let book = books_root.join("名前なし");
        fs::create_dir_all(&book).unwrap();
        make_file(&root, "keep.jpg", b"ok");
        make_file(&book, "0001_page.png", b"compiled");

        let stale_key = normalize_path(&book.join("0001_page.png"));
        db.upsert_meta_ok(&stale_key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();

        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root: root.clone(),
                excluded_roots: vec![books_root],
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap();

        assert_eq!(r.total_scanned, 1, "除外 root 配下のページは候補にしない");
        assert_eq!(r.to_ingest.len(), 1);
        assert_eq!(r.to_ingest[0].abs_path, root.join("keep.jpg"));
        assert_eq!(
            r.to_delete,
            vec![stale_key],
            "除外 root 配下に残った旧行は削除候補に落とす"
        );
    }

    #[test]
    fn unchanged_files_not_re_ingested() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("u");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"hello");
        let abs = root.join("a.jpg");
        let metadata = abs.metadata().unwrap();
        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let size = metadata.len() as i64;
        let key = normalize_path(&abs);

        // DB に同じ mtime/size で登録済み
        db.upsert_meta_ok(&key, fav, &root, IndexKind::Image, mtime, size)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.unchanged, 1);
        assert!(r.to_ingest.is_empty());
        assert!(r.to_delete.is_empty());
    }

    #[test]
    fn modified_files_go_to_re_ingest() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("m");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"x");
        let abs = root.join("a.jpg");
        let key = normalize_path(&abs);
        // DB に "古い" mtime で登録
        db.upsert_meta_ok(&key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.unchanged, 0);
        assert_eq!(r.to_ingest.len(), 1, "mtime/size が変わったので再 ingest");
    }

    #[test]
    fn deleted_files_go_to_delete() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("d");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "survivor.jpg", b"s");

        let dead_key = normalize_path(&root.join("gone.jpg"));
        db.upsert_meta_ok(&dead_key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        let _ = dead_key.clone();
        let surv_key = normalize_path(&root.join("survivor.jpg"));
        let surv_meta = root.join("survivor.jpg").metadata().unwrap();
        let surv_mtime = surv_meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.upsert_meta_ok(
            &surv_key,
            fav,
            &root,
            IndexKind::Image,
            surv_mtime,
            surv_meta.len() as i64,
        )
        .unwrap();
        let _ = surv_key.clone();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.unchanged, 1);
        assert_eq!(r.to_delete, vec![dead_key]);
    }

    #[test]
    fn existing_zip_with_stale_db_row_goes_to_delete() {
        // 移行シナリオ: 旧版が索引した ZIP の fts_meta 行が残った状態で、ZIP ファイル
        // 自体は FS に存在し続ける。walker は ZIP を候補にしないので 3-way diff で
        // to_delete に落ち、ingest worker が Tantivy から掃除する (§3.2、Codex P3)。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("z");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "album.zip", b"PK");
        let zip_key = normalize_path(&root.join("album.zip"));
        // 旧版が入れた ZIP 行を seed する
        db.upsert_meta_ok(&zip_key, fav, &root, IndexKind::Zip, 1, 1)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 0, "ZIP は候補にならない");
        assert!(r.to_ingest.is_empty());
        assert_eq!(
            r.to_delete,
            vec![zip_key],
            "stale な ZIP 行が to_delete に落ちる"
        );
    }

    #[test]
    fn recursive_subdir_is_scanned() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("r");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        make_file(&root, "top.jpg", b"1");
        make_file(&root.join("sub"), "nested.jpg", b"2");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 2);
        assert_eq!(r.to_ingest.len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn directory_symlink_subtree_is_scanned_once_without_looping() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        make_file(&outside, "linked.jpg", b"1");
        if std::os::windows::fs::symlink_dir(&outside, root.join("link")).is_err() {
            return;
        }
        if std::os::windows::fs::symlink_dir(&root, root.join("loop")).is_err() {
            return;
        }

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.to_ingest.len(), 1);
        assert!(
            r.to_ingest[0].abs_path.ends_with("link\\linked.jpg")
                || r.to_ingest[0].abs_path.ends_with("link/linked.jpg"),
            "linked image should be indexed through the symlink path: {:?}",
            r.to_ingest[0].abs_path
        );
    }

    #[test]
    fn apple_double_files_are_ignored() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("a");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "photo.jpg", b"ok");
        make_file(&root, "._photo.jpg", b"metadata");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.to_ingest.len(), 1);
        assert!(r.to_ingest[0].abs_path.ends_with("photo.jpg"));
    }

    #[test]
    fn scope_respects_favorite_id() {
        // 別 favorite 配下の登録は本 scan の diff 対象にならない
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root_a = tmp.path().join("A");
        fs::create_dir_all(&root_a).unwrap();
        let key_b = normalize_path(&tmp.path().join("B/other.jpg"));
        // fav_b 所属の行を追加
        db.upsert_meta_ok(&key_b, fav_b, &tmp.path().join("B"), IndexKind::Image, 1, 1)
            .unwrap();

        // fav_a の scan 結果に fav_b は出てこない
        let r = scan_sync(fav_a, &root_a, &db);
        assert!(
            r.to_delete.is_empty(),
            "別 favorite の deleted は検出しない"
        );
    }

    #[test]
    fn diag_counters_increment_on_bad_dir() {
        // Codex 6 回目 nice-to-have #2: 診断 stats を出して原因調査を助ける
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        // 存在しないパスを root にする (read_dir がエラーを返すはず)
        let root = tmp.path().join("does_not_exist");
        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root,
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap();
        assert_eq!(r.total_scanned, 0);
        assert_eq!(
            r.diag.read_dir_errors, 1,
            "存在しないルートは read_dir エラーとしてカウントされるはず"
        );
    }

    #[test]
    fn cancel_stops_walk() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("c");
        fs::create_dir_all(&root).unwrap();
        for i in 0..50 {
            make_file(&root, &format!("f{}.jpg", i), b"x");
        }
        let cancel = Arc::new(AtomicBool::new(true)); // 最初から cancel
        let sem = GlobalIoSemaphore::new(2);
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root: root.clone(),
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        );
        // cancel 中は Err("cancelled") または 早期に空の ScanResult が返る
        assert!(r.is_err() || r.unwrap().total_scanned < 50);
    }

    #[test]
    fn sidecar_removal_re_ingests_image() {
        // サイドカーを後から削除したら、画像本体が変わっていなくても再 ingest 候補になる
        // (stale な sidecar_text をクリアするため。docs §14-3)。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("scrm");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"img");
        make_file(&root, "a.jpg.json", b"{\"k\":\"value\"}");

        // 1 回目: サイドカーありの差分署名で DB を seed する
        let r1 = scan_sync(fav, &root, &db);
        assert_eq!(r1.to_ingest.len(), 1);
        let cand = r1.to_ingest[0].clone();
        db.upsert_meta_ok(
            &cand.key,
            fav,
            &root,
            IndexKind::Image,
            cand.diff_mtime,
            cand.diff_size,
        )
        .unwrap();

        // 2 回目: 変化なし
        let r2 = scan_sync(fav, &root, &db);
        assert_eq!(r2.unchanged, 1, "サイドカー不変なら再 ingest しない");
        assert!(r2.to_ingest.is_empty());

        // サイドカー削除 → 3 回目で差分検出される
        fs::remove_file(root.join("a.jpg.json")).unwrap();
        let r3 = scan_sync(fav, &root, &db);
        assert_eq!(r3.to_ingest.len(), 1, "サイドカー削除で再 ingest される");
        assert_eq!(r3.unchanged, 0);
    }

    #[test]
    fn sidecar_priority_switch_same_size_re_ingests() {
        // Codex P3: `a.jpg.json` (優先1) 消失 → 同 size の `a.json` (優先3) に切替わったとき、
        // mtime/size が偶然一致しても差分署名の fingerprint がファイル名を含むので検出される。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("scsw");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"img");
        make_file(&root, "a.jpg.json", b"{\"x\":1}"); // 7 bytes

        // mtime も同一に固定する。これがないと、置換後 sidecar の mtime 差で
        // fingerprint 未導入の旧実装でもテストが通ってしまい、P3 修正を直接守れない。
        let sc_mtime = fs::metadata(root.join("a.jpg.json"))
            .unwrap()
            .modified()
            .unwrap();

        let r1 = scan_sync(fav, &root, &db);
        assert_eq!(r1.to_ingest.len(), 1);
        let cand = r1.to_ingest[0].clone();
        db.upsert_meta_ok(
            &cand.key,
            fav,
            &root,
            IndexKind::Image,
            cand.diff_mtime,
            cand.diff_size,
        )
        .unwrap();

        // full 形式を削除し、同じ 7 バイト・別名・**同一 mtime** の stem 形式に差し替える
        fs::remove_file(root.join("a.jpg.json")).unwrap();
        make_file(&root, "a.json", b"{\"y\":2}"); // 7 bytes
        std::fs::OpenOptions::new()
            .write(true)
            .open(root.join("a.json"))
            .unwrap()
            .set_modified(sc_mtime)
            .unwrap();

        let r2 = scan_sync(fav, &root, &db);
        assert_eq!(
            r2.to_ingest.len(),
            1,
            "サイドカーの優先順位切替 (同 size・同 mtime) でも fingerprint で再 ingest される"
        );
    }
}
