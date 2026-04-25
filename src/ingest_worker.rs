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
    /// - `to_ingest` の各候補について、メタ抽出 → fts_meta.mark_pending → upsert (sub-batch 蓄積)
    /// - `to_delete` の各 path について、fts_meta.mark_tombstone → delete (sub-batch 蓄積)
    /// - sub-batch が `BATCH_FLUSH_COUNT` に達するか `BATCH_FLUSH_INTERVAL` 経過したら
    ///   `dispatcher.batch(upserts, deletes, commit_after=true)` で submit する
    /// - 各 sub-batch の境界で dispatcher が Interactive キュー (タグ書き込み等) を先に拾うため、
    ///   indexer の長時間 ingest 中もタグ操作は ~1 sub-batch (1〜2s) 以内に応答できる。
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
        // Sub-batch アキュムレータ — 閾値到達でまとめて dispatcher に submit。
        let mut batch_upserts: Vec<IndexDoc> = Vec::with_capacity(BATCH_FLUSH_COUNT);
        let mut batch_deletes: Vec<String> = Vec::new();
        let mut pending_paths: Vec<String> = Vec::new(); // commit 後に mark_ok
        let mut tombstone_paths: Vec<String> = Vec::new(); // commit 後に purge
        let mut last_flush = Instant::now();
        let ingest_total = to_ingest.len();
        let delete_total = to_delete.len();

        // sub-batch を dispatcher に投げて mark_ok / purge_tombstone まで完了させるヘルパ。
        let flush_batch = |batch_upserts: &mut Vec<IndexDoc>,
                               batch_deletes: &mut Vec<String>,
                               pending_paths: &mut Vec<String>,
                               tombstone_paths: &mut Vec<String>|
         -> tantivy::Result<()> {
            if batch_upserts.is_empty() && batch_deletes.is_empty() {
                return Ok(());
            }
            let upserts = std::mem::take(batch_upserts);
            let deletes = std::mem::take(batch_deletes);
            writer.batch(
                upserts,
                deletes,
                true, // commit_after
                false, // reload_after_commit (Ctrl+G 時に OnCommitWithDelay で自動 reload)
                WriterPriority::Background,
            )?;
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
        };

        // === 1. 削除フェーズ ===
        for (i, path) in to_delete.iter().enumerate() {
            // UI 操作中なら ActivityGate 経由で待機。cancel が立てば即抜ける。
            if crate::activity_gate::wait_and_check_cancel(self.activity_gate, cancel) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                // カウントを先頭に置くとダイアログで truncate されても残る。
                // パスは絶対パスそのままにする (delete 経路は favorite 相対に直すと
                // tombstone されたキーと紐付けにくいので)。
                p.set(format!("削除 ({}/{}) {}", i + 1, delete_total, path));
            }
            if let Err(e) = self.meta_db.mark_tombstone(&[path.clone()]) {
                crate::logger::log(format!("ingest: mark_tombstone failed for {path}: {e}"));
                continue;
            }
            batch_deletes.push(path.clone());
            tombstone_paths.push(path.clone());
            stats.deleted += 1;

            if self.batch_should_flush(batch_upserts.len(), batch_deletes.len(), last_flush) {
                flush_batch(
                    &mut batch_upserts,
                    &mut batch_deletes,
                    &mut pending_paths,
                    &mut tombstone_paths,
                )?;
                last_flush = Instant::now();
            }
        }

        if stats.cancelled {
            flush_batch(
                &mut batch_upserts,
                &mut batch_deletes,
                &mut pending_paths,
                &mut tombstone_paths,
            )?;
            return Ok(stats);
        }

        // === 2. Ingest フェーズ ===
        for (i, cand) in to_ingest.into_iter().enumerate() {
            // XMP 読み 1 件分だけ進めて次で再チェック (gate + cancel 両対応)。
            if crate::activity_gate::wait_and_check_cancel(self.activity_gate, cancel) {
                stats.cancelled = true;
                break;
            }
            if let Some(p) = progress {
                // ファイル名だけだと同名別フォルダが判別できないので、favorite 相対パスで
                // フォルダごと見せる。カウントは先頭に置き、truncate されても残るようにする。
                let display = cand
                    .abs_path
                    .strip_prefix(&self.favorite_root)
                    .unwrap_or(&cand.abs_path)
                    .display()
                    .to_string();
                p.set(format!("取込 ({}/{}) {}", i + 1, ingest_total, display));
            }
            // メタ抽出と IndexDoc ビルドはここで実行 (writer に touch しない)。dispatcher 側は
            // upsert_doc を呼ぶだけなので、重い IO はこの thread で並列化されたまま。
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
                    pending_paths.push(cand.key.clone());
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
                flush_batch(
                    &mut batch_upserts,
                    &mut batch_deletes,
                    &mut pending_paths,
                    &mut tombstone_paths,
                )?;
                last_flush = Instant::now();
            }
        }

        // 残りを flush
        flush_batch(
            &mut batch_upserts,
            &mut batch_deletes,
            &mut pending_paths,
            &mut tombstone_paths,
        )?;
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

    /// mark_pending + IndexDoc ビルドを 1 箇所に集約 (旧 `commit_doc` の writer 抜き版)。
    /// `norms` は move で受けて IndexDoc に埋め込む — per-file で複製は発生しない。
    fn build_doc(
        &self,
        cand: &CandidateFile,
        container: Container,
        kind: IndexKind,
        norms: crate::ingest_text::PerSourceText,
    ) -> Result<IndexDoc, String> {
        self.meta_db
            .mark_pending(
                &cand.key,
                self.favorite_id,
                &self.favorite_root,
                kind,
                cand.mtime,
                cand.file_size,
            )
            .map_err(|e| format!("mark_pending: {e}"))?;
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
    fn delete_marks_tombstone_then_purges() {
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
        )
        .unwrap();
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
        writer
            .upsert(
                crate::fts_index::IndexDoc {
                    path: key.clone(),
                    container: crate::fts_index::Container::Fs,
                    zip_entry: String::new(),
                    favorite_id: fav,
                    kind: IndexKind::Image,
                    mtime: cand.mtime,
                    file_size: cand.file_size,
                    norms: norms.clone(),
                },
                crate::fts_writer_dispatcher::WriterPriority::Background,
            )
            .unwrap();
        writer
            .commit(false, crate::fts_writer_dispatcher::WriterPriority::Background)
            .unwrap();
        // !!! mark_ok を呼ばずに "クラッシュ" — pending のまま残留

        let row = meta.get(&key).unwrap().unwrap();
        assert_eq!(row.status, FileStatus::Pending);

        // Tantivy 側には commit 済 doc が残っている (post-filter で status=Pending を弾くのは
        // 検索ワーカー側 `global_search.rs` の責務)。
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
