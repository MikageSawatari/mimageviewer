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
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use uuid::Uuid;

#[cfg(test)]
use crate::fts_index;
use crate::fts_index::{Container, FtsIndex, IndexDoc, IndexKind};
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
/// let session = IngestSession::new(fav_id, fav_root, &meta_db, &index)
///     .with_activity_gate(&activity_gate);
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
    /// UI 操作中は ingest を一時停止するためのゲート (2026-04)。
    /// `None` なら停止しない (単体テスト用)。
    pub activity_gate: Option<&'a crate::activity_gate::ActivityGate>,
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
            activity_gate: None,
        }
    }

    /// ActivityGate を設定する。ingest の各ファイル処理前にこのゲートで待つ。
    pub fn with_activity_gate(mut self, gate: &'a crate::activity_gate::ActivityGate) -> Self {
        self.activity_gate = Some(gate);
        self
    }

    /// Walker の結果を適用する。
    ///
    /// - `to_ingest` の各候補について、メタ抽出 → fts_meta.upsert_meta_ok → upsert (sub-batch 蓄積)
    /// - `to_delete` の各 path について、Tantivy delete を sub-batch 蓄積し、commit 後に
    ///   `fts_meta.delete_paths` で SQLite から物理削除
    /// - sub-batch が `BATCH_FLUSH_COUNT` に達するか `BATCH_FLUSH_INTERVAL` 経過したら
    ///   `dispatcher.batch(upserts, deletes, commit_after=true)` で submit する
    /// - 各 sub-batch の境界で dispatcher が Interactive キュー (タグ書き込み等) を先に拾うため、
    ///   indexer の長時間 ingest 中もタグ操作は ~1 sub-batch (1〜2s) 以内に応答できる。
    ///
    /// INDEX_VERSION=6 以降、Pending / Tombstone 中継状態は廃止: SQLite には ingest 完了
    /// 状態 (`status=Ok`) を直接書き、削除は commit 完了後に物理 DELETE する。Tantivy
    /// commit と SQLite 書き込みの間にクラッシュした場合は起動時 reconciliation (3-way diff)
    /// で復旧する。
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        to_ingest: Vec<CandidateFile>,
        to_delete: Vec<String>,
        writer: &crate::fts_writer_dispatcher::FtsWriterDispatcher,
        io_sem: &GlobalIoSemaphore,
        priority: IoPriority,
        cancel: &AtomicBool,
        progress: Option<&ProgressReporter>,
    ) -> tantivy::Result<IngestStats> {
        use crate::fts_writer_dispatcher::WriterPriority;

        let mut stats = IngestStats::default();
        let mut batch_upserts: Vec<IndexDoc> = Vec::with_capacity(BATCH_FLUSH_COUNT);
        let mut batch_deletes: Vec<String> = Vec::new();
        let mut last_flush = Instant::now();
        let ingest_total = to_ingest.len();
        let delete_total = to_delete.len();

        // sub-batch を dispatcher に投げて Tantivy commit 後に SQLite 物理削除まで完了させるヘルパ。
        let flush_batch = |batch_upserts: &mut Vec<IndexDoc>,
                               batch_deletes: &mut Vec<String>|
         -> tantivy::Result<()> {
            if batch_upserts.is_empty() && batch_deletes.is_empty() {
                return Ok(());
            }
            let upserts = std::mem::take(batch_upserts);
            let deletes = std::mem::take(batch_deletes);
            let deletes_for_sqlite = deletes.clone();
            writer.batch(
                upserts,
                deletes,
                true,  // commit_after
                true,  // reload_after_commit
                WriterPriority::Background,
            )?;
            if !deletes_for_sqlite.is_empty() {
                if let Err(e) = self.meta_db.delete_paths(&deletes_for_sqlite) {
                    crate::logger::log(format!("ingest: delete_paths failed: {e}"));
                }
            }
            Ok(())
        };

        // === 1. 削除フェーズ ===
        for (i, path) in to_delete.iter().enumerate() {
            // UI 操作中なら ActivityGate 経由で待機。cancel が立てば即抜ける。
            if crate::activity_gate::wait_and_check_cancel(self.activity_gate, cancel) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                p.set_msg_and_count(
                    format!("削除 ({}/{}) {}", i + 1, delete_total, path),
                    (i + 1) as u64,
                    delete_total as u64,
                );
            }
            batch_deletes.push(path.clone());
            stats.deleted += 1;

            if self.batch_should_flush(batch_upserts.len(), batch_deletes.len(), last_flush) {
                flush_batch(&mut batch_upserts, &mut batch_deletes)?;
                last_flush = Instant::now();
            }
        }

        if stats.cancelled {
            flush_batch(&mut batch_upserts, &mut batch_deletes)?;
            return Ok(stats);
        }

        // === 2. Ingest フェーズ ===
        for (i, cand) in to_ingest.into_iter().enumerate() {
            if crate::activity_gate::wait_and_check_cancel(self.activity_gate, cancel) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                let display = cand
                    .abs_path
                    .strip_prefix(&self.favorite_root)
                    .unwrap_or(&cand.abs_path)
                    .display()
                    .to_string();
                p.set_msg_and_count(
                    format!("取込 ({}/{}) {}", i + 1, ingest_total, display),
                    (i + 1) as u64,
                    ingest_total as u64,
                );
            }
            let _permit = io_sem.acquire(priority);
            let built = match cand.kind {
                CandidateKind::Image => self.build_doc_for_image(&cand),
                CandidateKind::Zip => self.build_doc_for_name_only(&cand),
                CandidateKind::Pdf => self.build_doc_for_pdf(&cand),
            };
            drop(_permit);
            match built {
                Ok(doc) => {
                    batch_upserts.push(doc);
                    stats.ingested_ok += 1;
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "ingest: {:?} build failed: {e}",
                        cand.abs_path
                    ));
                    let _ = self.meta_db.mark_failed(&cand.key);
                    stats.ingested_failed += 1;
                }
            }

            if self.batch_should_flush(batch_upserts.len(), batch_deletes.len(), last_flush) {
                flush_batch(&mut batch_upserts, &mut batch_deletes)?;
                last_flush = Instant::now();
            }
        }

        flush_batch(&mut batch_upserts, &mut batch_deletes)?;
        if let Some(p) = progress {
            p.clear_count();
        }
        Ok(stats)
    }

    /// sub-batch 投入閾値判定 (旧 should_flush と同じセマンティクス、引数は件数のみ)。
    fn batch_should_flush(&self, upsert_count: usize, delete_count: usize, last_flush: Instant) -> bool {
        let total = upsert_count + delete_count;
        total >= BATCH_FLUSH_COUNT || (total > 0 && last_flush.elapsed() >= BATCH_FLUSH_INTERVAL)
    }

    /// 画像ファイルから IndexDoc を組み立てる (mark_pending も同時に行う)。
    /// 旧 `ingest_image` を「writer に touch しない build フェーズ」として再構成したもの。
    fn build_doc_for_image(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
        let norms = crate::ingest_text::build_per_source_for_file(&cand.abs_path);
        self.build_doc(cand, Container::Fs, IndexKind::Image, norms)
    }

    /// PDF から IndexDoc を組み立てる (§16 step 17)。
    fn build_doc_for_pdf(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
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
                    "build_doc_for_pdf: get_document_info failed (falling back to name-only): {e}"
                ));
                String::new()
            }
        };
        let norms = crate::ingest_text::build_per_source_for_pdf(&name, &info_text);
        // PDF は container="fs" 扱い (v1)
        self.build_doc(cand, Container::Fs, IndexKind::Pdf, norms)
    }

    /// ZIP / PDF の最小 ingest (ファイル名 + 基本メタのみ)。
    fn build_doc_for_name_only(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
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
            _ => (Container::Fs, IndexKind::Image),
        };
        self.build_doc(cand, container, kind, norms)
    }

    /// upsert_meta_ok + IndexDoc ビルドを 1 箇所に集約。
    /// `norms` は move で受けて IndexDoc に埋め込む — per-file で複製は発生しない。
    fn build_doc(
        &self,
        cand: &CandidateFile,
        container: Container,
        kind: IndexKind,
        norms: crate::ingest_text::PerSourceText,
    ) -> Result<IndexDoc, String> {
        self.meta_db
            .upsert_meta_ok(
                &cand.key,
                self.favorite_id,
                &self.favorite_root,
                kind,
                cand.mtime,
                cand.file_size,
            )
            .map_err(|e| format!("upsert_meta_ok: {e}"))?;
        Ok(IndexDoc {
            path: cand.key.clone(),
            container,
            zip_entry: String::new(),
            favorite_id: self.favorite_id,
            kind,
            mtime: cand.mtime,
            file_size: cand.file_size,
            norms,
        })
    }
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

    fn setup() -> (TempDir, FtsMetaDb, std::sync::Arc<FtsIndex>) {
        let tmp = TempDir::new().unwrap();
        let meta = FtsMetaDb::open_at(&tmp.path().join("meta.db")).unwrap();
        let fts = std::sync::Arc::new(FtsIndex::open_at(&tmp.path().join("fts_index")).unwrap());
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
    fn delete_purges_immediately() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
        // ZIP ファイル名が Tantivy 側 (`name` STORED) でヒットすること
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["album"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        fts.reload_reader().unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert!(!hits.is_empty(), "Tantivy 側でヒットする");
    }

    /// Codex P1 回帰: ingest commit 後に reader が確実に reload 済みであること。
    /// post-filter は同じ Tantivy snapshot から STORED 原文を引くので、commit 後に
    /// 古い snapshot を読まされると偽陽性になる。`IngestSession::apply` 完了直後に
    /// reader が最新を見えていれば OK。
    #[test]
    fn ingest_commit_implies_reader_sees_latest_commit() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let cand = make_image_file(tmp.path(), "raceguard.jpg");
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
        // status=Ok になっていることを fts_meta 側で確認
        let row = meta.get(&key).unwrap().unwrap();
        assert_eq!(row.status, FileStatus::Ok);
        // 明示 reload を呼ばずに searcher を取る → reader は既に新 commit を見えているはず
        let searcher = fts.searcher();
        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["raceguard"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "ingest 完了直後 (mark_ok 後) の searcher は新 doc を見えるべき"
        );
    }

    #[test]
    fn cancel_mid_ingest_flushes_partial_and_returns_cancelled() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
    fn failed_residue_detected_by_reconciliation_hook() {
        // ingest 中に build_doc が失敗 (= mark_failed が呼ばれる) ケース。
        // list_not_ok_paths で Failed を回収できることを確認する。
        let (_tmp, meta, _fts) = setup();
        let fav = Uuid::new_v4();
        let key = "c:/never/exists.jpg".to_string();
        meta.upsert_meta_ok(
            &key,
            fav,
            std::path::Path::new("C:/"),
            IndexKind::Image,
            0,
            0,
        )
        .unwrap();
        meta.mark_failed(&key).unwrap();

        let not_ok = meta.list_not_ok_paths(fav).unwrap();
        assert_eq!(not_ok.len(), 1);
        assert_eq!(not_ok[0].0, key);
        assert_eq!(not_ok[0].1, FileStatus::Failed);
    }

    #[test]
    fn batch_flush_threshold_is_respected() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(fts.writer().unwrap(), std::sync::Arc::clone(&fts));
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
