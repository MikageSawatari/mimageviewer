//! Ingest Worker (docs/search-expansion-design.md §7.5, §5.6)。
//!
//! Walker が返した `to_ingest` / `to_delete` キューを受け取り、以下を行う:
//!
//! 1. **メタ抽出**: `ingest_text::build_all_text_for_file` で all_text_norm を作る
//! 2. **fts_meta.db に pending マーク** (§5.6.1 step 1)
//! 3. **Tantivy にバッファリング** (upsert_doc / delete_doc)
//! 4. **バッチ境界で Tantivy commit** (§5.6.1 step 3)
//! 5. **fts_meta.db を ok に遷移** (§5.6.1 step 4)
//!
//! ## バッチサイズ
//!
//! 100 件 or 5 秒のどちらか先に来た方でコミット。IndexWriter::commit() は fsync を伴うため
//! 1 件ずつ commit すると極端に遅くなる。設計ドキュメント §7.5 参照。
//!
//! ## I/O 優先度
//!
//! `GlobalIoSemaphore` で UI スレッドの読み取り (サムネロード等) と競合しないよう
//! Low 優先度で I/O する。HDD の場合は事実上シリアル化される。
//!
//! ## キャンセル
//!
//! `Arc<AtomicBool>` でいつでも中断可能。未コミット分は破棄され、次回起動時の
//! 差分走査で再度 `to_ingest` に入る (fts_meta.db の status=pending が残っている場合は
//! reconciliation 処理で再試行される)。
//!
//! ## v1 スコープ
//!
//! - FS 上の画像 / ZIP ファイル自体 / PDF ファイル自体の ingest
//! - ZIP 内エントリの展開 ingest は **本モジュールの責務外** (§7.7 の ZIP 専用 context で別途)
//! - PDFium document info の取り込みは §16 step 17 (別モジュール)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tantivy::IndexWriter;
use uuid::Uuid;

use crate::fts_index::{self, Container, FtsIndex, IndexDoc, IndexKind};
use crate::fts_meta::FtsMetaDb;
use crate::indexer_progress::ProgressReporter;
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::search_walker::{CandidateFile, CandidateKind};

/// バッチサイズ / タイムアウト (§7.5)
pub const BATCH_FLUSH_COUNT: usize = 100;
pub const BATCH_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Ingest 結果サマリ (進捗表示 / テスト向け)。
#[derive(Default, Debug, Clone)]
pub struct IngestStats {
    pub ingested_ok: usize,
    pub ingested_failed: usize,
    pub deleted: usize,
    pub skipped_unsupported: usize,
    pub cancelled: bool,
}

/// 単一お気に入りに対する ingest セッション。
///
/// 呼び出し側 (Indexer Supervisor) は以下の流れで使う:
/// ```ignore
/// let session = IngestSession::new(fav_id, fav_root, &meta_db, &index);
/// let writer = index.writer()?;
/// let stats = session.apply(scan_result, &writer, &io_sem, &cancel)?;
/// // writer drop で最終 commit
/// ```
///
/// `IndexWriter` はセッション跨ぎで使い回すこと (Tantivy の推奨)。
pub struct IngestSession<'a> {
    pub favorite_id: Uuid,
    pub favorite_root: PathBuf,
    pub meta_db: &'a FtsMetaDb,
    pub fts: &'a FtsIndex,
}

impl<'a> IngestSession<'a> {
    pub fn new(
        favorite_id: Uuid,
        favorite_root: PathBuf,
        meta_db: &'a FtsMetaDb,
        fts: &'a FtsIndex,
    ) -> Self {
        Self {
            favorite_id,
            favorite_root,
            meta_db,
            fts,
        }
    }

    /// Walker の結果を適用する。
    ///
    /// - `to_ingest` の各候補について、メタ抽出 → fts_meta.mark_pending → Tantivy upsert_doc
    /// - `to_delete` の各 path について、fts_meta.mark_tombstone → Tantivy delete_doc
    /// - バッチ境界 (BATCH_FLUSH_COUNT 件 or BATCH_FLUSH_INTERVAL) で
    ///   writer.commit() → fts_meta.mark_ok / purge_tombstone
    ///
    /// **重要 (writer lock 飢餓対策)**: writer は `Arc<Mutex<IndexWriter>>` 由来の Mutex 参照。
    /// 大規模 ingest で 1 回掴みっぱなしにすると、interactive な writer 利用者
    /// (`tag_write_worker` のタグ付与など) が分単位で starve する。flush 境界で必ず
    /// guard を drop → 短時間 sleep → 再取得して、待機中の他 lock 利用者に取り合いの
    /// 機会を与える ([docs/async-architecture.md §5.5](../docs/async-architecture.md))。
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        to_ingest: Vec<CandidateFile>,
        to_delete: Vec<String>,
        writer_mtx: &std::sync::Mutex<IndexWriter>,
        io_sem: &GlobalIoSemaphore,
        priority: IoPriority,
        cancel: &AtomicBool,
        progress: Option<&ProgressReporter>,
    ) -> tantivy::Result<IngestStats> {
        let mut stats = IngestStats::default();
        let fields = self.fts.fields();

        // pending 中のパス (commit 後に mark_ok 対象になる)
        let mut pending_paths: Vec<String> = Vec::new();
        // tombstone 中のパス (commit 後に purge する)
        let mut tombstone_paths: Vec<String> = Vec::new();
        let mut last_flush = Instant::now();
        let ingest_total = to_ingest.len();
        let delete_total = to_delete.len();

        // writer は Option で持ち、flush 後に take() で drop → 再取得で他 lock 利用者に
        // 機会を与える。`acquire_writer` ヘルパで lazy 取得する。
        let mut writer: Option<std::sync::MutexGuard<'_, IndexWriter>> =
            Some(writer_mtx.lock().unwrap());

        // === 1. 削除フェーズ ===
        // stats.deleted は **実際に tombstone 化 + Tantivy delete を push したもの** のみカウント
        // (Codex 6 回目指摘 nice-to-have #1)。mark_tombstone 失敗や cancel で処理されなかった分は数えない。
        for (i, path) in to_delete.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                p.set(format!("削除: {} ({}/{})", path, i + 1, delete_total));
            }
            // fts_meta: tombstone 化
            if let Err(e) = self.meta_db.mark_tombstone(&[path.clone()]) {
                crate::logger::log(format!("ingest: mark_tombstone failed for {path}: {e}"));
                continue;
            }
            let w = writer
                .as_deref_mut()
                .expect("writer guard alive between flushes");
            fts_index::delete_doc(w, fields, path);
            tombstone_paths.push(path.clone());
            stats.deleted += 1;

            if self.should_flush(&tombstone_paths, &pending_paths, last_flush) {
                self.flush(
                    writer.as_deref_mut().unwrap(),
                    &mut pending_paths,
                    &mut tombstone_paths,
                )?;
                yield_writer_lock(&mut writer, writer_mtx);
                last_flush = Instant::now();
            }
        }

        if stats.cancelled {
            // 残っている分を commit (stats.deleted は既に実処理カウント済み)
            self.flush(
                writer.as_deref_mut().unwrap(),
                &mut pending_paths,
                &mut tombstone_paths,
            )?;
            return Ok(stats);
        }

        // === 2. Ingest フェーズ ===
        for (i, cand) in to_ingest.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                let name = cand
                    .abs_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| cand.abs_path.display().to_string());
                p.set(format!("取込: {} ({}/{})", name, i + 1, ingest_total));
            }

            // Ingest 対象の種類で分岐。v1 スコープ: Image のみ詳細メタ抽出、
            // Zip / Pdf はファイル名のみ取り込み (ZIP 内は §7.7、PDF info は step 17)。
            let w = writer
                .as_deref()
                .expect("writer guard alive between flushes");
            match cand.kind {
                CandidateKind::Image => {
                    // I/O 同時実行制御
                    let _permit = io_sem.acquire(priority);
                    match self.ingest_image(&cand, w) {
                        Ok(()) => {
                            pending_paths.push(cand.key.clone());
                            stats.ingested_ok += 1;
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "ingest: image {:?} failed: {e}",
                                cand.abs_path
                            ));
                            let _ = self.meta_db.mark_failed(&cand.key);
                            stats.ingested_failed += 1;
                        }
                    }
                }
                CandidateKind::Zip => {
                    // v1: ZIP はファイル名のみ ingest (ZIP 内展開は v1.x で対応)
                    let _permit = io_sem.acquire(priority);
                    match self.ingest_name_only(&cand, w) {
                        Ok(()) => {
                            pending_paths.push(cand.key.clone());
                            stats.ingested_ok += 1;
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "ingest: container {:?} failed: {e}",
                                cand.abs_path
                            ));
                            let _ = self.meta_db.mark_failed(&cand.key);
                            stats.ingested_failed += 1;
                        }
                    }
                }
                CandidateKind::Pdf => {
                    // §16 step 17: PDF は document info (Title/Author/Subject/Keywords) を
                    // 取り込む。PDFium ワーカー (別プロセス) を使うのでメイン process の
                    // pdfium スレッド制約を気にしなくてよい。
                    let _permit = io_sem.acquire(priority);
                    match self.ingest_pdf(&cand, w) {
                        Ok(()) => {
                            pending_paths.push(cand.key.clone());
                            stats.ingested_ok += 1;
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "ingest: pdf {:?} failed: {e}",
                                cand.abs_path
                            ));
                            let _ = self.meta_db.mark_failed(&cand.key);
                            stats.ingested_failed += 1;
                        }
                    }
                }
            }

            if self.should_flush(&tombstone_paths, &pending_paths, last_flush) {
                self.flush(
                    writer.as_deref_mut().unwrap(),
                    &mut pending_paths,
                    &mut tombstone_paths,
                )?;
                yield_writer_lock(&mut writer, writer_mtx);
                last_flush = Instant::now();
            }
        }

        // 残りを flush (stats.deleted は削除フェーズで実処理カウント済み)
        self.flush(
            writer.as_deref_mut().unwrap(),
            &mut pending_paths,
            &mut tombstone_paths,
        )?;
        Ok(stats)
    }

    fn ingest_image(&self, cand: &CandidateFile, writer: &IndexWriter) -> Result<(), String> {
        let norms = crate::ingest_text::build_per_source_for_file(&cand.abs_path);
        self.commit_doc(writer, cand, Container::Fs, IndexKind::Image, norms)
    }

    /// PDF を ingest (§16 step 17)。ファイル名 + PDFium document info を検索対象にする。
    /// パスワード保護 PDF や破損 PDF は name_only にフォールバック。
    fn ingest_pdf(&self, cand: &CandidateFile, writer: &IndexWriter) -> Result<(), String> {
        let name = cand
            .abs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let info_text = match crate::pdf_loader::get_document_info(&cand.abs_path, None) {
            Ok(info) => info.as_search_text(),
            Err(e) => {
                crate::logger::log(format!(
                    "ingest_pdf: get_document_info failed (falling back to name-only): {e}"
                ));
                String::new()
            }
        };
        let norms = crate::ingest_text::build_per_source_for_pdf(&name, &info_text);
        // PDF は container="fs" 扱い (v1)
        self.commit_doc(writer, cand, Container::Fs, IndexKind::Pdf, norms)
    }

    /// ZIP / PDF の最小 ingest (ファイル名 + 基本メタのみ)。
    fn ingest_name_only(&self, cand: &CandidateFile, writer: &IndexWriter) -> Result<(), String> {
        let name = cand
            .abs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let norms = crate::ingest_text::build_per_source_name_only(&name);
        let (container, kind) = match cand.kind {
            CandidateKind::Zip => (Container::Zip, IndexKind::Zip),
            CandidateKind::Pdf => (Container::Fs, IndexKind::Pdf),
            // Image/Video が ここに来るのは本来通らないルートだが、
            // classifier を通さず name_only にフォールバックした診断目的のパス。
            _ => (Container::Fs, IndexKind::Image),
        };
        self.commit_doc(writer, cand, container, kind, norms)
    }

    /// mark_pending → IndexDoc 生成 → upsert_doc を 1 箇所に集約 (§19 simplify pass)。
    /// `norms` は move で受けて IndexDoc に埋め込むため、per-file で 5 x clone が発生しない。
    fn commit_doc(
        &self,
        writer: &IndexWriter,
        cand: &CandidateFile,
        container: Container,
        kind: IndexKind,
        norms: crate::ingest_text::PerSourceText,
    ) -> Result<(), String> {
        self.meta_db
            .mark_pending(
                &cand.key,
                self.favorite_id,
                &self.favorite_root,
                kind,
                cand.mtime,
                cand.file_size,
                &norms,
            )
            .map_err(|e| format!("mark_pending: {e}"))?;
        let doc = IndexDoc {
            path: cand.key.clone(),
            container,
            zip_entry: String::new(),
            favorite_id: self.favorite_id,
            kind,
            mtime: cand.mtime,
            file_size: cand.file_size,
            norms,
        };
        fts_index::upsert_doc(writer, self.fts.fields(), &doc)
            .map_err(|e| format!("upsert_doc: {e}"))?;
        Ok(())
    }

    fn should_flush(&self, tombstone: &[String], pending: &[String], last_flush: Instant) -> bool {
        let total = tombstone.len() + pending.len();
        total >= BATCH_FLUSH_COUNT || (total > 0 && last_flush.elapsed() >= BATCH_FLUSH_INTERVAL)
    }

    /// Tantivy commit → fts_meta を ok / purge に遷移 (§5.6.1 step 3-4 + §5.6.2 step 4)。
    fn flush(
        &self,
        writer: &mut IndexWriter,
        pending_paths: &mut Vec<String>,
        tombstone_paths: &mut Vec<String>,
    ) -> tantivy::Result<()> {
        if pending_paths.is_empty() && tombstone_paths.is_empty() {
            return Ok(());
        }
        writer.commit()?;
        // reader の reload は Ctrl+G クエリ時に `OnCommitWithDelay` で自動で行われる
        if !pending_paths.is_empty() {
            if let Err(e) = self.meta_db.mark_ok(pending_paths) {
                crate::logger::log(format!("ingest: mark_ok failed: {e}"));
            }
            pending_paths.clear();
        }
        if !tombstone_paths.is_empty() {
            if let Err(e) = self.meta_db.purge_tombstone(tombstone_paths) {
                crate::logger::log(format!("ingest: purge_tombstone failed: {e}"));
            }
            tombstone_paths.clear();
        }
        Ok(())
    }
}

/// バッチ flush 直後、tantivy writer の guard を一旦 drop して、待機中の他 lock 利用者
/// (interactive な `tag_write_worker` など) に取り合いの機会を与える。短い sleep を挟むのは
/// std::sync::Mutex がプラットフォームによって unfair で、drop 直後に同スレッドが
/// 再取得してしまうケースを防ぐため (Windows では特に顕著)。再取得して `writer` を埋め直す。
fn yield_writer_lock<'a>(
    writer: &mut Option<std::sync::MutexGuard<'a, IndexWriter>>,
    writer_mtx: &'a std::sync::Mutex<IndexWriter>,
) {
    *writer = None; // drop the guard → release the mutex
    // ~5ms あれば tag_write_worker の 1 ファイル分 (read + write_xmp + upsert) が
    // 滑り込める。50 batch x 5ms = 250ms の追加 overhead は initial scan 数十秒に対し誤差。
    std::thread::sleep(std::time::Duration::from_millis(5));
    *writer = Some(writer_mtx.lock().unwrap());
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_meta::FileStatus;
    use crate::search_index_db::normalize_path;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn setup() -> (TempDir, FtsMetaDb, FtsIndex) {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap();
        let fts = FtsIndex::open_at(&tmp.path().join("fts_index")).unwrap();
        (tmp, meta, fts)
    }

    fn make_image_file(dir: &Path, name: &str) -> CandidateFile {
        let abs = dir.join(name);
        fs::write(&abs, b"jpeg-like content").unwrap();
        let meta = abs.metadata().unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        CandidateFile {
            abs_path: abs.clone(),
            key: normalize_path(&abs),
            kind: CandidateKind::Image,
            mtime,
            file_size: meta.len() as i64,
        }
    }

    #[test]
    fn ingest_empty_queues_is_noop() {
        let (_tmp, meta, fts) = setup();
        let session = IngestSession::new(Uuid::new_v4(), PathBuf::from("C:/x"), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);
        let stats = session
            .apply(
                vec![],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert_eq!(stats.ingested_ok, 0);
        assert_eq!(stats.deleted, 0);
    }

    #[test]
    fn ingest_image_marks_ok_and_searchable() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let root = tmp.path().to_path_buf();
        let session = IngestSession::new(fav, root.clone(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let cand = make_image_file(tmp.path(), "sunset_夕焼け.jpg");
        let key = cand.key.clone();
        let stats = session
            .apply(
                vec![cand],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert_eq!(stats.ingested_ok, 1);
        assert_eq!(stats.ingested_failed, 0);

        // fts_meta 側が ok に遷移している
        let row = meta.get(&key).unwrap().unwrap();
        assert_eq!(row.status, FileStatus::Ok);
        assert_eq!(row.index_generation, 1);
        assert_eq!(row.favorite_id, fav);

        // Tantivy 側で検索できる
        fts.reload_reader().unwrap();
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["夕焼け"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, key);
    }

    #[test]
    fn delete_marks_tombstone_then_purges() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let cand = make_image_file(tmp.path(), "a.jpg");
        let key = cand.key.clone();
        session
            .apply(
                vec![cand],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        fts.reload_reader().unwrap();
        assert!(meta.get(&key).unwrap().is_some());

        // 削除
        let stats = session
            .apply(
                vec![],
                vec![key.clone()],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert_eq!(stats.deleted, 1);
        // 物理削除されている (tombstone → purge が flush で走った)
        assert!(meta.get(&key).unwrap().is_none());

        // Tantivy 側からも消えている
        fts.reload_reader().unwrap();
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["a.jpg"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn zip_file_ingested_as_container() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let mut cand = make_image_file(tmp.path(), "album.zip");
        cand.kind = CandidateKind::Zip;
        let key = cand.key.clone();
        let stats = session
            .apply(
                vec![cand],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert_eq!(stats.ingested_ok, 1);
        let row = meta.get(&key).unwrap().unwrap();
        assert_eq!(row.status, FileStatus::Ok);
        // ZIP ファイル名が検索対象に入っている (name_norm に格納される)
        assert!(row.norms.name.contains("album.zip"));
    }

    #[test]
    fn cancel_mid_ingest_flushes_partial_and_returns_cancelled() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(true); // 最初から cancel

        let cand = make_image_file(tmp.path(), "skip.jpg");
        let stats = session
            .apply(
                vec![cand],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert!(stats.cancelled);
        assert_eq!(stats.ingested_ok, 0);
    }

    #[test]
    fn re_ingest_existing_path_bumps_generation() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let cand = make_image_file(tmp.path(), "a.jpg");
        let key = cand.key.clone();
        session
            .apply(
                vec![cand.clone()],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();

        let gen1 = meta.get(&key).unwrap().unwrap().index_generation;

        // 再 ingest (例: ファイル更新シミュレーション)
        session
            .apply(
                vec![cand],
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        let gen2 = meta.get(&key).unwrap().unwrap().index_generation;
        assert!(gen2 > gen1, "generation が増える");
    }

    #[test]
    fn pending_residue_detected_by_reconciliation_hook() {
        // Codex 6 回目テスト推奨: Tantivy commit 後 mark_ok が失敗 (または呼ばれず
        // クラッシュ) した場合の残留状態。list_not_ok_paths で pending を回収できること。
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let root = tmp.path().to_path_buf();

        // mark_pending のみを手動で呼び、Tantivy upsert + commit も行うが
        // mark_ok は意図的に呼ばない (クラッシュシミュレーション)
        let cand = make_image_file(tmp.path(), "crash.jpg");
        let key = cand.key.clone();
        let norms = crate::ingest_text::build_per_source_for_file(&cand.abs_path);
        meta.mark_pending(
            &key,
            fav,
            &root,
            IndexKind::Image,
            cand.mtime,
            cand.file_size,
            &norms,
        )
        .unwrap();
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        {
            let mut w = writer.lock().unwrap();
            fts_index::upsert_doc(
                &w,
                fts.fields(),
                &crate::fts_index::IndexDoc {
                    path: key.clone(),
                    container: crate::fts_index::Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav,
                    kind: IndexKind::Image,
                    mtime: cand.mtime,
                    file_size: cand.file_size,
                    norms: norms.clone(),
                },
            )
            .unwrap();
            w.commit().unwrap();
        }
        // !!! mark_ok を呼ばずに "クラッシュ" — pending のまま残留

        // 状態: fts_meta は pending、Tantivy は ok 済み (新テキスト)
        let row = meta.get(&key).unwrap().unwrap();
        assert_eq!(row.status, FileStatus::Pending);

        // Ctrl+G 検索では pending は結果に現れない (lookup_norms_for_target が status=0 のみ返す)
        fts.reload_reader().unwrap();
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["crash.jpg"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1, "Tantivy には残っている");
        let lookup = meta
            .lookup_norms_for_target(&[key.clone()], &crate::fts_index::SearchTarget::All)
            .unwrap();
        assert!(
            lookup.is_empty(),
            "post-filter では pending は除外 → 検索結果に出ない"
        );

        // reconciliation hook で検出できる (次回起動時の再 ingest 対象)
        let not_ok = meta.list_not_ok_paths(fav).unwrap();
        assert_eq!(not_ok.len(), 1);
        assert_eq!(not_ok[0].0, key);
        assert_eq!(not_ok[0].1, FileStatus::Pending);
    }

    #[test]
    fn batch_flush_threshold_is_respected() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = std::sync::Mutex::new(fts.writer().unwrap());
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        // BATCH_FLUSH_COUNT を超える数を投入 (130 件 → 少なくとも 1 回は途中 flush がある)
        let mut cands = Vec::new();
        for i in 0..130 {
            cands.push(make_image_file(tmp.path(), &format!("b{:03}.jpg", i)));
        }
        let stats = session
            .apply(
                cands,
                vec![],
                &writer,
                &sem,
                IoPriority::Low,
                &cancel,
                None,
            )
            .unwrap();
        assert_eq!(stats.ingested_ok, 130);
        // 全 ok になっている
        fts.reload_reader().unwrap();
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["b000"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert!(!hits.is_empty());
    }
}
