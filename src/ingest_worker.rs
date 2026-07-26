//! Ingest Worker (docs/archive/search-metadata/search-expansion-design.md §7.5, §5.6)。
//!
//! Walker が返した `to_ingest` / `to_delete` キューを受け取り、以下を行う:
//!
//! 1. **メタ抽出 + IndexDoc 構築**: `ingest_text::build_per_source_*` を呼んで
//!    `IndexDoc` を作る。SQLite には触れない。
//! 2. **Tantivy にバッファリング**: `IndexDoc` を `batch_upserts` に積み、削除候補は
//!    `batch_deletes` に積む。
//! 3. **バッチ境界で Tantivy commit + reader reload** (`writer.batch(commit=true,
//!    reload=true)`)
//! 4. **commit 成功後に SQLite を更新**: 投入済みの `(path, kind, mtime, size)` を
//!    `upsert_meta_ok` で `status=Ok` 直書き、削除済みの path を `delete_paths` で
//!    物理削除。
//!
//! 順序不変条件: SQLite の書き込みは Tantivy commit が成功したフレームのみ実施する
//! (Tantivy First)。途中でクラッシュした場合は次回起動時の reconciliation / walker
//! 3-way diff (FS / Tantivy / SQLite) で復旧する。
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
//! `Arc<AtomicBool>` でいつでも中断可能。Tantivy commit 前に中断された分は SQLite に
//! 反映していないので、次回起動時の walker 3-way diff で「FS にあるが SQLite に無い」
//! として再 `to_ingest` 候補に拾われる。
//!
//! ## v1 スコープ
//!
//! - FS 上の画像 / PDF ファイル / 動画ファイルの ingest
//! - ZIP はアイテム索引 (Ctrl+G) の対象外。walker が候補に含めないので本モジュールにも
//!   到達しない (docs/search-container-item-redesign.md §3.2)
//! - PDFium document info の取り込みは §16 step 17 (別モジュール)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// - `to_ingest` の各候補について、メタ抽出 → IndexDoc を `batch_upserts` に蓄積、
    ///   `(path, kind, mtime, size)` を `pending_ok_meta` に蓄積
    /// - `to_delete` の各 path について、Tantivy delete を `batch_deletes` に蓄積
    /// - sub-batch が `BATCH_FLUSH_COUNT` に達するか `BATCH_FLUSH_INTERVAL` 経過したら
    ///   `dispatcher.batch(upserts, deletes, commit_after=true)` で submit
    /// - **Tantivy commit + reader reload が成功したフレームでのみ** `fts_meta` を更新:
    ///   `pending_ok_meta` を `upsert_meta_ok` (status=Ok) で書き、`batch_deletes` を
    ///   `delete_paths` で物理削除
    /// - 各 sub-batch の境界で dispatcher が Interactive キュー (タグ書き込み等) を先に拾うため、
    ///   indexer の長時間 ingest 中もタグ操作は ~1 sub-batch (1〜2s) 以内に応答できる。
    ///
    /// Tantivy commit と SQLite 書き込みの間にクラッシュした場合は起動時 reconciliation
    /// (3-way diff: FS / Tantivy / SQLite) で復旧する。
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
        // ingest 候補のメタ情報。Tantivy commit が成功した後に upsert_meta_ok で
        // SQLite に書く (Tantivy First 順序)。クラッシュ時は SQLite に行が無いので
        // walker の 3-way diff で再 ingest 候補に拾われる。
        let mut pending_ok_meta: Vec<(String, IndexKind, i64, i64)> = Vec::new();
        let mut last_flush = Instant::now();
        let ingest_total = to_ingest.len();
        let delete_total = to_delete.len();

        // sub-batch を dispatcher に投げて Tantivy commit 後に SQLite を更新するヘルパ。
        // ingest: Tantivy commit → SQLite upsert_meta_ok (status=Ok)
        // delete: Tantivy commit → SQLite delete_paths
        // どちらも Tantivy First で書き、commit が失敗 / クラッシュした場合は次回起動の
        // walker / reconciliation で復旧する (Tantivy にあるが SQLite に無い doc も
        // 「FS にあるけど DB に無い」 / 「FS に無いし DB に無い」のいずれかとして
        // 検出可能)。
        let flush_batch = |batch_upserts: &mut Vec<IndexDoc>,
                           batch_deletes: &mut Vec<String>,
                           pending_ok_meta: &mut Vec<(String, IndexKind, i64, i64)>|
         -> tantivy::Result<bool> {
            if batch_upserts.is_empty() && batch_deletes.is_empty() {
                return Ok(true);
            }
            if cancel.load(Ordering::Relaxed) {
                batch_upserts.clear();
                batch_deletes.clear();
                pending_ok_meta.clear();
                return Ok(false);
            }
            let upserts = std::mem::take(batch_upserts);
            let deletes = std::mem::take(batch_deletes);
            let deletes_for_sqlite = deletes.clone();
            let ok_meta = std::mem::take(pending_ok_meta);
            let completed = writer.batch_cancellable(
                upserts,
                deletes,
                true,
                true,
                WriterPriority::Background,
                cancel,
            )?;
            if !completed {
                return Ok(false);
            }
            // ここに来た時点で Tantivy commit + reader reload が完了している。
            // SQLite 側を Tantivy に合わせて更新する。
            for (key, kind, mtime, file_size) in &ok_meta {
                if let Err(e) = self.meta_db.upsert_meta_ok(
                    key,
                    self.favorite_id,
                    &self.favorite_root,
                    *kind,
                    *mtime,
                    *file_size,
                ) {
                    crate::logger::log(format!("ingest: upsert_meta_ok({key}) failed: {e}"));
                }
            }
            if !deletes_for_sqlite.is_empty() {
                if let Err(e) = self.meta_db.delete_paths(&deletes_for_sqlite) {
                    crate::logger::log(format!("ingest: delete_paths failed: {e}"));
                }
            }
            Ok(true)
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
                if !flush_batch(&mut batch_upserts, &mut batch_deletes, &mut pending_ok_meta)? {
                    stats.cancelled = true;
                    break;
                }
                last_flush = Instant::now();
            }
        }

        if stats.cancelled {
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
            let Some(_permit) = io_sem.acquire_cancellable(priority, cancel) else {
                stats.cancelled = true;
                break;
            };
            let built = match cand.kind {
                CandidateKind::Image => self.build_doc_for_image(&cand),
                CandidateKind::Pdf => self.build_doc_for_pdf(&cand),
                CandidateKind::Video => self.build_doc_for_video(&cand),
                CandidateKind::Audio => self.build_doc_for_audio(&cand),
            };
            drop(_permit);
            match built {
                Ok(doc) => {
                    // fts_meta には **差分用** 署名 (画像 + サイドカーを織り込んだ diff_mtime/
                    // diff_size) を保存する。walker の 3-way diff がこれと比較してサイドカーの
                    // 追加/編集/削除を検出する。Tantivy doc 側 (`doc.mtime`) は画像本体の mtime の
                    // ままで、日付ソートはサイドカー編集時刻に引きずられない (docs §14-4)。
                    pending_ok_meta.push((
                        cand.key.clone(),
                        doc.kind,
                        cand.diff_mtime,
                        cand.diff_size,
                    ));
                    batch_upserts.push(doc);
                    stats.ingested_ok += 1;
                }
                Err(e) => {
                    crate::logger::log(format!("ingest: {:?} build failed: {e}", cand.abs_path));
                    // build 失敗時は Tantivy 投入も SQLite 書き込みも行わない。
                    // 次回起動の walker が「DB に無い」として再 ingest 候補に乗せる。
                    // 同じファイルが毎回失敗する場合は毎回再試行されるが、abusive な
                    // 失敗 (例: 壊れた画像) は実害が限定的なので retry 抑制は持たない。
                    stats.ingested_failed += 1;
                }
            }

            if self.batch_should_flush(batch_upserts.len(), batch_deletes.len(), last_flush) {
                if !flush_batch(&mut batch_upserts, &mut batch_deletes, &mut pending_ok_meta)? {
                    stats.cancelled = true;
                    break;
                }
                last_flush = Instant::now();
            }
        }

        if !stats.cancelled
            && !flush_batch(&mut batch_upserts, &mut batch_deletes, &mut pending_ok_meta)?
        {
            stats.cancelled = true;
        }
        if let Some(p) = progress {
            p.clear_count();
        }
        Ok(stats)
    }

    /// sub-batch 投入閾値判定 (旧 should_flush と同じセマンティクス、引数は件数のみ)。
    fn batch_should_flush(
        &self,
        upsert_count: usize,
        delete_count: usize,
        last_flush: Instant,
    ) -> bool {
        let total = upsert_count + delete_count;
        total >= BATCH_FLUSH_COUNT || (total > 0 && last_flush.elapsed() >= BATCH_FLUSH_INTERVAL)
    }

    /// 画像ファイルから IndexDoc を組み立てる。SQLite には触れない build phase。
    fn build_doc_for_image(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
        let norms = crate::ingest_text::build_per_source_for_file(&cand.abs_path);
        self.build_doc(cand, Container::Fs, IndexKind::Image, norms)
    }

    /// 動画ファイルから IndexDoc を組み立てる。ファイル名 / mXD XMP /
    /// sidecar tags / FFmpeg container metadata をソース別に保持する。
    fn build_doc_for_video(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
        let norms = crate::ingest_text::build_per_source_for_file(&cand.abs_path);
        self.build_doc(cand, Container::Fs, IndexKind::Video, norms)
    }

    /// 音声ファイルからファイル名だけの IndexDoc を組み立てる。
    /// ID3 等の埋め込みメタデータは将来スコープのため読み取らない。
    fn build_doc_for_audio(&self, cand: &CandidateFile) -> Result<IndexDoc, String> {
        let norms = crate::ingest_text::build_per_source_for_filename(&cand.abs_path);
        self.build_doc(cand, Container::Fs, IndexKind::Audio, norms)
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

    /// IndexDoc ビルド。SQLite には触れず Tantivy 投入用の構造体のみ返す。
    /// SQLite 側の status=Ok 書き込みは `flush_batch` 内で Tantivy commit 成功後に行う。
    /// `norms` は move で受けて IndexDoc に埋め込む — per-file で複製は発生しない。
    fn build_doc(
        &self,
        cand: &CandidateFile,
        container: Container,
        kind: IndexKind,
        norms: crate::ingest_text::PerSourceText,
    ) -> Result<IndexDoc, String> {
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
            // テストではサイドカー無し → 差分用署名は本体と同値。
            diff_mtime: mtime,
            diff_size: meta.len() as i64,
        }
    }

    #[test]
    fn ingest_empty_queues_is_noop() {
        let (_tmp, meta, fts) = setup();
        let session = IngestSession::new(Uuid::new_v4(), PathBuf::from("C:/x"), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
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
        // Tantivy commit 後に SQLite delete_paths が flush で走り、物理削除されている
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

    /// タグ刷新 (f64f6b4e〜) で通常 ingest は XMP `dc:subject` の mIV `#` タグを
    /// FTS 索引へ投影しなくなった (タグは tags.db 専有。`ingest_text.rs` の `tags`
    /// フィールドは移行時のみ参照し、通常 ingest では populate しない)。
    /// よってサイドカーに `#` タグを持つ動画を ingest しても、その動画自体は Video
    /// として索引されるが、`#` タグ文字列では FTS 検索にヒットしない。
    /// この経路が再び FTS へ漏れない (= 2 索引へ投影しない設計) ことの回帰ガード。
    #[test]
    fn video_file_with_sidecar_tags_ingests_but_tag_not_in_fts() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let mut cand = make_image_file(tmp.path(), "tagged_movie.mp4");
        cand.kind = CandidateKind::Video;
        std::fs::write(
            tmp.path().join("tagged_movie.mp4.xmp"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
           xmlns:dc="http://purl.org/dc/elements/1.1/">
    <rdf:Description>
      <dc:subject>
        <rdf:Bag>
          <rdf:li>#video_ingest_marker</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>"#,
        )
        .unwrap();
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
        assert_eq!(row.kind, IndexKind::Video);

        let favs = [fav];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["video_ingest_marker"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                ..Default::default()
            },
        )
        .unwrap();
        fts.reload_reader().unwrap();
        let searcher = fts.searcher();
        let hits = fts_index::search_page(&searcher, fts.fields(), &q, 0, 10).unwrap();
        // タグ刷新後: `#` タグは FTS へ投影されないので 0 件 (旧挙動は 1 件)。
        // タグの検索は tags.db / タグビュー側で行う。
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn audio_file_ingests_as_audio_with_filename_only() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        let mut cand = make_image_file(tmp.path(), "SearchableSong.MP3");
        cand.kind = CandidateKind::Audio;
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
        assert_eq!(meta.get(&key).unwrap().unwrap().kind, IndexKind::Audio);

        let favs = [fav];
        let kinds = [IndexKind::Audio];
        let q = fts_index::build_bigram_and_query(
            fts.fields(),
            &["searchablesong"],
            &crate::fts_index::QueryFilters {
                favorite_ids: Some(&favs),
                kinds: Some(&kinds),
                target: crate::fts_index::SearchTarget::Only(vec![
                    crate::fts_index::SourceKind::Filename,
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        fts.reload_reader().unwrap();
        let hits = fts_index::search_page(&fts.searcher(), fts.fields(), &q, 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, key);
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
            "ingest 完了直後 (commit + reader reload 後) の searcher は新 doc を見えるべき"
        );
    }

    #[test]
    fn cancel_mid_ingest_flushes_partial_and_returns_cancelled() {
        let (tmp, meta, fts) = setup();
        let fav = Uuid::new_v4();
        let session = IngestSession::new(fav, tmp.path().to_path_buf(), &meta, &fts);
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
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
        let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
            fts.writer().unwrap(),
            std::sync::Arc::clone(&fts),
        );
        let sem = GlobalIoSemaphore::new(2);
        let cancel = AtomicBool::new(false);

        // BATCH_FLUSH_COUNT を超える数を投入 (130 件 → 少なくとも 1 回は途中 flush がある)
        let mut cands = Vec::new();
        for i in 0..130 {
            cands.push(make_image_file(tmp.path(), &format!("b{:03}.jpg", i)));
        }
        let stats = session
            .apply(cands, vec![], &writer, &sem, IoPriority::Low, &cancel, None)
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
