//! 本 (フォルダ / ZIP / PDF) ごとの「最後に読んだページ」永続管理。
//!
//! `%APPDATA%/mimageviewer/book_resume.db` にコンテナパスごとの最後に表示した
//! ページ index を保存し、アプリ再起動を跨いで読書位置を復元する (動画の
//! `video_resume_positions` の画像本版)。
//!
//! キーは `path_key::normalize` (= ドライブ文字除去・小文字化・スラッシュ統一) で、
//! USB / 外付け HDD のドライブレター変化に追従する点を優先する `spread_db.rs` と
//! 同じ規則。`rotation_db.rs` 等の per-item DB はドライブ文字を保持する別規則なので
//! 混同しないこと (別ドライブの同名パスは同一キーに畳まれるトレードオフがある)。
//!
//! 従来の `items` index と optional な論理ページ identity を同じ transaction で保存する。
//! 新版の復元は同じ identity を照合し、種類除外後の同じ index の別画像を代用しない。
//! 旧版の raw-only 更新は同値でも trigger により identity を失効させる。
//! 同名の実 directory とネスト書庫の衝突は非対応 (2026-10-08 利用者決定)。
//!
//! ページ送りのたびに書き込みが走るため、**書き込みは [`BookResumeWriter`] の専用
//! スレッドへ逃がして UI スレッドの同期 SQLite I/O を避ける**。復元用位置読取も
//! 同じ FIFO と完了証明付き immutable snapshot を使い、先行記録を追い越さない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

use crate::path_key;

/// Logical page identity. Physical cache paths never identify converted documents.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageIdentity {
    Image {
        path: String,
    },
    Archive {
        container: String,
        entry_name: String,
    },
    Document {
        container: String,
        page_num: usize,
    },
}

impl PageIdentity {
    pub(crate) fn for_item(
        item: &crate::grid_item::GridItem,
        logical: Option<&Path>,
    ) -> Option<Self> {
        use crate::grid_item::GridItem;
        match item {
            GridItem::Image(path) => Some(Self::Image {
                path: path_key::normalize_keep_drive(path),
            }),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => Some(Self::Archive {
                container: path_key::normalize_keep_drive(logical.unwrap_or(zip_path)),
                entry_name: entry_name.replace('\\', "/"),
            }),
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => Some(Self::Document {
                container: path_key::normalize_keep_drive(logical.unwrap_or(pdf_path)),
                page_num: *page_num as usize,
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingPosition {
    pub page: usize,
    pub identity: Option<PageIdentity>,
}

/// HUDと同じ読み順で記録したanchorの1-based位置。内容変更後も次の記録まで保持する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadingMeterValue {
    pub ordinal: usize,
    pub total: usize,
}

impl ReadingMeterValue {
    pub fn new(ordinal: usize, total: usize) -> Option<Self> {
        (ordinal > 0 && ordinal <= total && i64::try_from(total).is_ok())
            .then_some(Self { ordinal, total })
    }

    pub fn fraction(self) -> f32 {
        self.ordinal as f32 / self.total as f32
    }
}

pub(crate) struct MeterSnapshot {
    pub values: HashMap<String, Option<ReadingMeterValue>>,
}

pub(crate) type MeterReadResult = Result<MeterSnapshot, String>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResumeWatermarks {
    pub admitted: u64,
    pub completed_through: u64,
}

/// An external transaction may only certify a read which began after completion
/// and saw no admission/completion change through the end of that transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResumeReadProof {
    pub cutoff: u64,
    pub completed_through: u64,
}

impl ResumeReadProof {
    pub(crate) fn for_transaction(
        cutoff: u64,
        before: ResumeWatermarks,
        after: ResumeWatermarks,
    ) -> Option<Self> {
        (before == after
            && before.admitted == before.completed_through
            && before.completed_through >= cutoff)
            .then_some(Self {
                cutoff,
                completed_through: before.completed_through,
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PositionReadReply {
    pub cutoff: u64,
    pub completed_through: u64,
    pub position: Option<ReadingPosition>,
    pub status: Result<(), String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PositionSnapshot {
    pub completed_through: u64,
    pub values: HashMap<String, ReadingPosition>,
    pub status: Result<(), String>,
}

impl PositionSnapshot {
    pub(crate) fn read_after(&self, resume_key: &str, cutoff: u64) -> Option<PositionReadReply> {
        (self.completed_through >= cutoff).then(|| PositionReadReply {
            cutoff,
            completed_through: self.completed_through,
            position: self.values.get(resume_key).cloned(),
            status: self.status.clone(),
        })
    }
}

#[derive(Default)]
struct WriterState {
    watermarks: ResumeWatermarks,
    snapshot: Option<Arc<PositionSnapshot>>,
}

/// Cloneable, I/O-free observations for row-preparation workers. Capture before
/// BEGIN/first SELECT and after the read transaction ends, then validate proof.
#[derive(Clone)]
pub(crate) struct ResumeWatermarkReader(Arc<Mutex<WriterState>>);

impl ResumeWatermarkReader {
    pub(crate) fn watermarks(&self) -> ResumeWatermarks {
        self.0.lock().unwrap().watermarks
    }

    fn observation(&self) -> Option<(ResumeWatermarks, Result<(), String>)> {
        let state = self.0.lock().unwrap();
        Some((state.watermarks, state.snapshot.as_ref()?.status.clone()))
    }
}

/// 読書位置 DB ハンドル
pub struct BookResumeDb {
    conn: rusqlite::Connection,
}

impl BookResumeDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 指定 DB を通常と同じスキーマ初期化経路で開く。
    pub(crate) fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut conn = rusqlite::Connection::open(path)?;
        // 読み (UI スレッド) と書き ([`BookResumeWriter`] スレッド) で 2 接続が同じ
        // ファイルを触るため、稀な競合で SQLITE_BUSY を即時エラーにせず待たせる。
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS book_resume (
                path TEXT PRIMARY KEY,
                page INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        // Released readers may update page without knowing the additive identity column.
        // Install the column and unconditional invalidation trigger atomically.
        let has_identity = conn
            .prepare("PRAGMA table_info(book_resume)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|column| column == "page_identity");
        let tx = conn.transaction()?;
        if !has_identity {
            tx.execute_batch("ALTER TABLE book_resume ADD COLUMN page_identity TEXT")?;
        }
        tx.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS book_resume_raw_invalidates_identity
            AFTER UPDATE OF page ON book_resume BEGIN
                UPDATE book_resume SET page_identity=NULL WHERE path=NEW.path;
            END",
        )?;
        tx.commit()?;
        Ok(Self { conn })
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("book_resume.db")
    }

    /// コンテナの最後に読んだページ index を取得。未登録なら None。
    pub fn get(&self, path: &Path) -> Option<usize> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT page FROM book_resume WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let v: i64 = row.get(0)?;
            Ok(v.max(0) as usize)
        })
        .ok()
    }

    pub fn get_position(&self, path: &Path) -> Option<ReadingPosition> {
        self.conn
            .query_row(
                "SELECT page,page_identity FROM book_resume WHERE path=?1",
                [normalize_path(path)],
                |row| {
                    let page: i64 = row.get(0)?;
                    let encoded: Option<String> = row.get(1)?;
                    Ok(ReadingPosition {
                        page: page.max(0) as usize,
                        identity: encoded.and_then(|value| serde_json::from_str(&value).ok()),
                    })
                },
            )
            .ok()
    }

    /// Exact released resume key; errors remain distinct from a missing row.
    pub(crate) fn read_position(
        &self,
        resume_key: &str,
    ) -> Result<Option<ReadingPosition>, String> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT page,page_identity FROM book_resume WHERE path=?1",
                [resume_key],
                |row| {
                    let page: i64 = row.get(0)?;
                    let encoded: Option<String> = row.get(1)?;
                    Ok((page, encoded))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|(page, encoded)| {
                Ok(ReadingPosition {
                    page: page.max(0) as usize,
                    identity: encoded
                        .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
                        .transpose()?,
                })
            })
            .transpose()
    }

    /// Row-preparation worker fast path. `None` means the external transaction
    /// could not be certified; enqueue ReadPositionAfter instead. This method
    /// owns BEGIN/end so the before/after observations bracket the actual read.
    /// Call only on a worker: opening/reading the DB is never UI work.
    pub(crate) fn read_position_proven(
        &self,
        resume_key: &str,
        cutoff: u64,
        reader: &ResumeWatermarkReader,
    ) -> Option<PositionReadReply> {
        let (before, writer_status) = reader.observation()?;
        ResumeReadProof::for_transaction(cutoff, before, before)?;
        let result = (|| {
            let tx = self
                .conn
                .unchecked_transaction()
                .map_err(|e| e.to_string())?;
            let position = self.read_position(resume_key)?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(position)
        })();
        let (after, _) = reader.observation()?;
        let proof = ResumeReadProof::for_transaction(cutoff, before, after)?;
        let (position, status) = match result {
            Ok(position) => (position, writer_status),
            Err(e) => {
                crate::logger::log(format!("book-resume: proven position read failed: {e}"));
                (None, Err(e))
            }
        };
        Some(PositionReadReply {
            cutoff: proof.cutoff,
            completed_through: proof.completed_through,
            position,
            status,
        })
    }

    fn read_positions(&self) -> Result<HashMap<String, ReadingPosition>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT path,page,page_identity FROM book_resume")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            let (key, page, encoded) = row.map_err(|e| e.to_string())?;
            Ok((
                key,
                ReadingPosition {
                    page: page.max(0) as usize,
                    identity: encoded
                        .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
                        .transpose()?,
                },
            ))
        })
        .collect()
    }

    fn set_position(
        &mut self,
        path: &Path,
        page: usize,
        meter: Option<ReadingMeterValue>,
        identity: Option<&PageIdentity>,
        meter_ready: bool,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let key = normalize_path(path);
        let meter = meter.and_then(|v| ReadingMeterValue::new(v.ordinal, v.total));
        if meter_ready {
            tx.execute("INSERT INTO book_resume(path,page,page_ordinal,page_total)
                VALUES (?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET
                page=excluded.page,page_ordinal=excluded.page_ordinal,page_total=excluded.page_total",
                rusqlite::params![key, page as i64, meter.map(|v| v.ordinal as i64), meter.map(|v| v.total as i64)])?;
        } else {
            tx.execute(
                "INSERT INTO book_resume(path,page) VALUES (?1,?2)
                ON CONFLICT(path) DO UPDATE SET page=excluded.page",
                rusqlite::params![key, page as i64],
            )?;
        }
        let encoded = identity
            .map(|value| serde_json::to_string(value).expect("page identity serialization"));
        tx.execute(
            "UPDATE book_resume SET page_identity=?1 WHERE path=?2",
            rusqlite::params![encoded, key],
        )?;
        tx.commit()
    }

    /// 最後に読んだページ index を保存する。
    pub fn set(&self, path: &Path, page: usize) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        self.conn.execute(
            "INSERT INTO book_resume (path, page) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET page = ?2",
            rusqlite::params![key, page as i64],
        )?;
        Ok(())
    }

    /// 1 件削除 (リセット)
    pub fn remove(&self, path: &Path) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        self.conn
            .execute("DELETE FROM book_resume WHERE path = ?1", [&key])?;
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM book_resume", [])
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM book_resume", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// writer起動時だけ実行する。旧2列・既存行を保持し、失敗時は全ALTERをrollback。
    fn migrate_meter_columns(&mut self) -> Result<(), rusqlite::Error> {
        let columns = self
            .conn
            .prepare("PRAGMA table_info(book_resume)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self.conn.transaction()?;
        for column in ["page_ordinal", "page_total"] {
            if !columns.iter().any(|existing| existing == column) {
                tx.execute_batch(&format!(
                    "ALTER TABLE book_resume ADD COLUMN {column} INTEGER"
                ))?;
            }
        }
        tx.commit()
    }

    fn set_meter(
        &self,
        path: &Path,
        page: usize,
        value: Option<ReadingMeterValue>,
    ) -> Result<(), rusqlite::Error> {
        let value = value.and_then(|v| ReadingMeterValue::new(v.ordinal, v.total));
        self.conn.execute(
            "INSERT INTO book_resume (path,page,page_ordinal,page_total)
             VALUES (?1,?2,?3,?4) ON CONFLICT(path) DO UPDATE SET
             page=excluded.page,page_ordinal=excluded.page_ordinal,
             page_total=excluded.page_total",
            rusqlite::params![
                normalize_path(path),
                page as i64,
                value.map(|v| v.ordinal as i64),
                value.map(|v| v.total as i64)
            ],
        )?;
        Ok(())
    }

    fn read_meters(&self) -> Result<MeterSnapshot, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT path,page_ordinal,page_total FROM book_resume")?;
        let mut rows = stmt.query([])?;
        let mut snapshot = MeterSnapshot {
            values: HashMap::new(),
        };
        while let Some(row) = rows.next()? {
            let key = row.get(0)?;
            let mut value = None;
            let tuple = (row.get::<_, Option<i64>>(1), row.get::<_, Option<i64>>(2));
            if let (Ok(Some(ordinal)), Ok(Some(total))) = tuple
                && let (Ok(ordinal), Ok(total)) = (usize::try_from(ordinal), usize::try_from(total))
                && let Some(valid) = ReadingMeterValue::new(ordinal, total)
            {
                value = Some(valid);
            }
            snapshot.values.insert(key, value);
        }
        Ok(snapshot)
    }
}

fn normalize_path(path: &Path) -> String {
    path_key::normalize(path)
}

/// 読書位置の書き込みを UI スレッドから外す background writer。自前の write 用
/// Connection を持つ専用スレッドへ `(path, page, meter)` を送って upsert する。ページ送りの
/// たびに走る書き込みで UI が引っかからないようにするのが目的 ([docs/ui-responsiveness.md]
/// の「UI スレッドの同期 I/O は worker 化」)。メーターの移行/全行読込/clearも同じFIFO。
/// Released raw/meter APIs remain available; new restores use the fenced read.
enum Command {
    FolderZip(
        PathBuf,
        PathBuf,
        Vec<String>,
        Option<Vec<String>>,
        crate::image_folder_zip_metadata::CopyPhase,
        mpsc::Sender<crate::image_folder_zip_metadata::CopyReply>,
    ),
    Record(
        PathBuf,
        usize,
        Option<ReadingMeterValue>,
        Option<PageIdentity>,
    ),
    Read(mpsc::Sender<MeterReadResult>),
    ReadPositionAfter(String, u64, mpsc::Sender<PositionReadReply>),
    Clear(mpsc::Sender<Result<usize, String>>),
    #[cfg(test)]
    Pause(mpsc::Sender<()>, mpsc::Receiver<()>),
}

struct QueuedCommand {
    position_fence_seq: Option<u64>,
    command: Command,
}

pub struct BookResumeWriter {
    /// `Option` なのは `Drop` で先に Sender を落として writer スレッドの recv ループを
    /// 終了させてから join するため。
    tx: Option<mpsc::Sender<QueuedCommand>>,
    handle: Option<std::thread::JoinHandle<()>>,
    pending: Arc<AtomicUsize>,
    /// This short lock is the admission order for every &self command, not a DB
    /// lock. No I/O, recv, or join runs while held.
    state: Arc<Mutex<WriterState>>,
}

impl BookResumeWriter {
    /// writer スレッドを spawn する。DB を開けなければ `None` (= 書き込みは no-op)。
    pub fn spawn() -> Option<Self> {
        Self::spawn_at(BookResumeDb::db_path())
    }

    pub(crate) fn spawn_at(path: PathBuf) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<QueuedCommand>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);
        let state = Arc::new(Mutex::new(WriterState::default()));
        let worker_state = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name("book-resume-writer".into())
            .spawn(move || {
                let mut db = match BookResumeDb::open_at(&path) {
                    Ok(db) => db,
                    Err(e) => {
                        crate::logger::log(format!("book-resume writer: DB open failed: {e}"));
                        // Receiverを閉じると後続要求もDisconnectedとして完了する。
                        return;
                    }
                };
                let meter_ready = match db.migrate_meter_columns() {
                    Ok(()) => true,
                    Err(e) => {
                        crate::logger::log(format!(
                            "book-resume writer: meter migration failed: {e}"
                        ));
                        false
                    }
                };
                let (values, status) = match db.read_positions() {
                    Ok(values) => (values, Ok(())),
                    Err(e) => {
                        crate::logger::log(format!(
                            "book-resume writer: positions read failed: {e}"
                        ));
                        (HashMap::new(), Err(e))
                    }
                };
                worker_state.lock().unwrap().snapshot = Some(Arc::new(PositionSnapshot {
                    completed_through: 0,
                    values,
                    status,
                }));
                // tx が全て drop されるまで (= App 終了まで) 受信し続ける。channel に
                // 溜まっている分は Disconnected 前に drain されるので取りこぼしは無い。
                while let Ok(QueuedCommand {
                    position_fence_seq,
                    command,
                }) = rx.recv()
                {
                    match command {
                        Command::Record(path, page, value, identity) => {
                            let result =
                                db.set_position(&path, page, value, identity.as_ref(), meter_ready);
                            if let Err(e) = &result {
                                crate::logger::log(format!("book-resume writer: set failed: {e}"));
                            }
                            let seq =
                                position_fence_seq.expect("Record advances the position fence");
                            // Move the publication out briefly so COW of a held
                            // snapshot never blocks UI admission on a full-map clone.
                            let mut publication =
                                worker_state.lock().unwrap().snapshot.take().unwrap();
                            let snapshot = Arc::make_mut(&mut publication);
                            match result {
                                Ok(()) => {
                                    snapshot.values.insert(
                                        normalize_path(&path),
                                        ReadingPosition {
                                            page: (page as i64).max(0) as usize,
                                            identity,
                                        },
                                    );
                                }
                                Err(e) => snapshot.status = Err(e.to_string()),
                            }
                            snapshot.completed_through = seq;
                            let mut state = worker_state.lock().unwrap();
                            state.snapshot = Some(publication);
                            state.watermarks.completed_through = seq;
                        }
                        Command::FolderZip(source, destination, members, legacy, phase, reply) => {
                            let result = crate::image_folder_zip_metadata::clone_resume_on(
                                &db.conn,
                                &source,
                                &destination,
                                &members,
                                legacy.as_deref(),
                                phase,
                            );
                            let seq = position_fence_seq.expect("conversion read/write is fenced");
                            let values = db.read_positions();
                            {
                                let mut state = worker_state.lock().unwrap();
                                let mut snapshot = state.snapshot.take().unwrap();
                                let current = Arc::make_mut(&mut snapshot);
                                match values {
                                    Ok(values) => current.values = values,
                                    Err(error) => current.status = Err(error),
                                }
                                current.completed_through = seq;
                                state.snapshot = Some(snapshot);
                                state.watermarks.completed_through = seq;
                            }
                            let _ = reply.send(result);
                        }
                        Command::Read(tx) => {
                            let result = if meter_ready {
                                db.read_meters().map_err(|e| e.to_string())
                            } else {
                                Err("meter schema unavailable".into())
                            };
                            // Shared maintenance paths already reload the meter
                            // after changing resume rows. Publish positions through
                            // that same FIFO completion rather than retaining old keys.
                            let positions = db.read_positions();
                            let mut publication =
                                worker_state.lock().unwrap().snapshot.take().unwrap();
                            let snapshot = Arc::make_mut(&mut publication);
                            match positions {
                                Ok(values) => snapshot.values = values,
                                Err(e) => {
                                    crate::logger::log(format!(
                                        "book-resume writer: positions refresh failed: {e}"
                                    ));
                                    snapshot.status = Err(e);
                                }
                            }
                            let seq = position_fence_seq.expect("Read advances the position fence");
                            snapshot.completed_through = seq;
                            {
                                let mut state = worker_state.lock().unwrap();
                                state.snapshot = Some(publication);
                                state.watermarks.completed_through = seq;
                            }
                            let _ = tx.send(result);
                        }
                        Command::ReadPositionAfter(key, cutoff, tx) => {
                            let (completed_through, mut status) = {
                                let state = worker_state.lock().unwrap();
                                (
                                    state.watermarks.completed_through,
                                    state.snapshot.as_ref().unwrap().status.clone(),
                                )
                            };
                            let position = match db.read_position(&key) {
                                Ok(position) => position,
                                Err(e) => {
                                    crate::logger::log(format!(
                                        "book-resume writer: position read failed: {e}"
                                    ));
                                    status = Err(e);
                                    None
                                }
                            };
                            let _ = tx.send(PositionReadReply {
                                cutoff,
                                completed_through,
                                position,
                                status,
                            });
                        }
                        Command::Clear(tx) => {
                            let result = db.clear_all().map_err(|e| e.to_string());
                            if let Err(e) = &result {
                                crate::logger::log(format!(
                                    "book-resume writer: clear failed: {e}"
                                ));
                            }
                            {
                                let seq =
                                    position_fence_seq.expect("Clear advances the position fence");
                                let mut publication =
                                    worker_state.lock().unwrap().snapshot.take().unwrap();
                                let snapshot = Arc::make_mut(&mut publication);
                                match &result {
                                    Ok(_) => snapshot.values.clear(),
                                    Err(e) => snapshot.status = Err(e.clone()),
                                }
                                snapshot.completed_through = seq;
                                let mut state = worker_state.lock().unwrap();
                                state.snapshot = Some(publication);
                                state.watermarks.completed_through = seq;
                            }
                            let _ = tx.send(result);
                        }
                        #[cfg(test)]
                        Command::Pause(entered, resume) => {
                            let _ = entered.send(());
                            let _ = resume.recv();
                        }
                    }
                    worker_pending.fetch_sub(1, Ordering::Release);
                }
            });
        match spawned {
            Ok(handle) => Some(Self {
                tx: Some(tx),
                handle: Some(handle),
                pending,
                state,
            }),
            Err(e) => {
                crate::logger::log(format!("book-resume writer: thread spawn failed: {e}"));
                None
            }
        }
    }

    /// 読書位置を非同期で記録する (送るだけ・UI スレッドはブロックしない)。
    fn send(&self, command: Command) -> bool {
        let mut state = self.state.lock().unwrap();
        self.send_locked(command, &mut state)
    }

    fn send_locked(&self, command: Command, state: &mut WriterState) -> bool {
        // A maintenance reread changes the published position view, so it must
        // fence later receipts just like Record/Clear, even though it writes no DB row.
        let position_fence_seq = matches!(
            command,
            Command::Record(..) | Command::Read(..) | Command::Clear(..) | Command::FolderZip(..)
        )
        .then(|| {
            state
                .watermarks
                .admitted
                .checked_add(1)
                .expect("resume sequence exhausted")
        });
        self.pending.fetch_add(1, Ordering::AcqRel);
        if self.tx.as_ref().is_some_and(|tx| {
            tx.send(QueuedCommand {
                position_fence_seq,
                command,
            })
            .is_ok()
        }) {
            if let Some(seq) = position_fence_seq {
                state.watermarks.admitted = seq;
            }
            true
        } else {
            self.pending.fetch_sub(1, Ordering::Release);
            false
        }
    }

    pub(crate) fn watermarks(&self) -> ResumeWatermarks {
        self.state.lock().unwrap().watermarks
    }

    pub(crate) fn watermark_reader(&self) -> ResumeWatermarkReader {
        ResumeWatermarkReader(Arc::clone(&self.state))
    }

    /// Only offer a warm snapshot when it covers every admitted write and reload.
    /// A retained older Arc still cannot cover a later receipt's larger cutoff.
    pub(crate) fn position_snapshot(&self) -> Option<Arc<PositionSnapshot>> {
        let state = self.state.lock().unwrap();
        (state.watermarks.admitted == state.watermarks.completed_through)
            .then(|| state.snapshot.clone())
            .flatten()
    }

    pub(crate) fn read_position_after(
        &self,
        resume_key: &str,
        cutoff: u64,
    ) -> mpsc::Receiver<PositionReadReply> {
        let (tx, rx) = mpsc::channel();
        let mut state = self.state.lock().unwrap();
        if cutoff <= state.watermarks.admitted {
            if state.watermarks.admitted == state.watermarks.completed_through
                && let Some(reply) = state
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.read_after(resume_key, cutoff))
            {
                let _ = tx.send(reply);
                return rx;
            }
            if self.send_locked(
                Command::ReadPositionAfter(resume_key.into(), cutoff, tx.clone()),
                &mut state,
            ) {
                return rx;
            }
        }
        let _ = tx.send(PositionReadReply {
            cutoff,
            completed_through: state.watermarks.completed_through,
            position: None,
            status: Err("resume writer unavailable or cutoff was not admitted".into()),
        });
        rx
    }

    pub fn record(&self, path: &Path, page: usize, value: Option<ReadingMeterValue>) -> bool {
        self.record_identity(path, page, value, None)
    }

    pub fn record_identity(
        &self,
        path: &Path,
        page: usize,
        value: Option<ReadingMeterValue>,
        identity: Option<PageIdentity>,
    ) -> bool {
        self.send(Command::Record(path.to_path_buf(), page, value, identity))
    }

    pub(crate) fn read_all(&self) -> mpsc::Receiver<MeterReadResult> {
        let (tx, rx) = mpsc::channel();
        self.send(Command::Read(tx));
        rx
    }

    pub(crate) fn folder_zip_portal(
        &self,
        source: PathBuf,
        destination: PathBuf,
        sort: crate::settings::SortOrder,
        columns: Vec<Arc<Vec<String>>>,
    ) -> crate::image_folder_zip_metadata::CopyPortal {
        let tx = self.tx.clone();
        let state = Arc::clone(&self.state);
        let pending = Arc::clone(&self.pending);
        Arc::new(move |manifest, phase| {
            let members = crate::image_folder_zip_metadata::target_order(manifest, sort);
            let member_set: std::collections::HashSet<_> = members.iter().collect();
            let mut candidates = columns
                .iter()
                .filter(|column| column.iter().all(|name| member_set.contains(name)))
                .map(|column| column.as_ref().clone());
            let legacy = candidates
                .next()
                .filter(|first| candidates.all(|other| other == *first));
            let (reply, rx) = mpsc::channel();
            let mut state = state.lock().unwrap();
            let seq = state
                .watermarks
                .admitted
                .checked_add(1)
                .expect("resume sequence exhausted");
            pending.fetch_add(1, Ordering::AcqRel);
            let sent = tx.as_ref().is_some_and(|tx| {
                tx.send(QueuedCommand {
                    position_fence_seq: Some(seq),
                    command: Command::FolderZip(
                        source.clone(),
                        destination.clone(),
                        members,
                        legacy,
                        phase,
                        reply,
                    ),
                })
                .is_ok()
            });
            if sent {
                state.watermarks.admitted = seq;
            } else {
                pending.fetch_sub(1, Ordering::Release);
            }
            rx
        })
    }

    pub(crate) fn clear(&self) -> mpsc::Receiver<Result<usize, String>> {
        let (tx, rx) = mpsc::channel();
        self.send(Command::Clear(tx));
        rx
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            && self.pending.load(Ordering::Acquire) > 0
    }

    #[cfg(test)]
    pub(crate) fn pause_for_test(&self) -> mpsc::Sender<()> {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        assert!(self.send(Command::Pause(entered_tx, resume_rx)));
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        resume_tx
    }
}

impl Drop for BookResumeWriter {
    fn drop(&mut self) {
        // Sender を先に落とすと writer の `rx.recv()` がキュー分を drain し切ってから
        // `Err` を返してループを抜ける。その後 join して、終了時に積んでいた書き込みが
        // ディスクへ反映されるのを待つ (デタッチのままだとプロセス終了で取りこぼす)。
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_zip_fixture() -> (
        tempfile::TempDir,
        crate::image_folder_zip::ImageFolderManifest,
    ) {
        let files = tempfile::tempdir().unwrap();
        let source = files.path().join("book");
        std::fs::create_dir(&source).unwrap();
        for name in ["p1.jpg", "p2.jpg", "p10.jpg"] {
            std::fs::write(source.join(name), b"image bytes").unwrap();
        }
        let manifest = crate::image_folder_zip::prepare_manifest(
            &source,
            None,
            &crate::image_folder_zip::ConversionControl::default(),
        )
        .unwrap();
        (files, manifest)
    }

    #[test]
    fn folder_zip_resume_fifo_remaps_identity_hint_and_meter_for_real_consumer() {
        use crate::app::file_type_resume::{PositionResolution, PreparedPageIdentityIndex};
        use crate::image_folder_zip_metadata::CopyPhase;
        let (files, manifest) = folder_zip_fixture();
        let writer = BookResumeWriter::spawn_at(files.path().join("resume.db")).unwrap();
        let portal = writer.folder_zip_portal(
            manifest.source.clone(),
            manifest.destination.clone(),
            crate::settings::SortOrder::NumericDesc,
            Vec::new(),
        );
        let source_identity = PageIdentity::Image {
            path: path_key::normalize_keep_drive(&manifest.source.join("p1.jpg")),
        };
        let sibling = files.path().join("sibling.zip");
        assert!(writer.record(&sibling, 7, ReadingMeterValue::new(8, 10)));
        let release = writer.pause_for_test();
        assert!(writer.record_identity(
            &manifest.source,
            0,
            ReadingMeterValue::new(1, 3),
            Some(source_identity.clone()),
        ));
        let checked = portal(&manifest, CopyPhase::Check);
        let copied = portal(&manifest, CopyPhase::Apply);
        let retained_source = writer.read_position_after(
            &normalize_path(&manifest.source),
            writer.watermarks().admitted,
        );
        assert!(writer.record(&manifest.source, 99, None));
        assert!(matches!(copied.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            checked.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(1)
        );
        assert_eq!(copied.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(1));
        assert_eq!(
            retained_source
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .position,
            Some(ReadingPosition {
                page: 0,
                identity: Some(source_identity)
            })
        );
        let cutoff = writer.watermarks().admitted;
        let target = position_reply(&writer, &manifest.destination, cutoff)
            .position
            .unwrap();
        assert_eq!(target.page, 2);
        assert_eq!(
            target.identity,
            Some(PageIdentity::Archive {
                container: path_key::normalize_keep_drive(&manifest.destination),
                entry_name: "p1.jpg".into(),
            })
        );
        let rows: Vec<_> = ["p10.jpg", "p2.jpg", "p1.jpg"]
            .into_iter()
            .map(|name| crate::grid_item::GridItem::ZipImage {
                zip_path: manifest.destination.clone(),
                entry_name: name.into(),
            })
            .collect();
        let consumer = PreparedPageIdentityIndex::prepare(&rows, None, Default::default());
        assert_eq!(consumer.resolve(&target), PositionResolution::Restored(2));
        let meters = writer
            .read_all()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(
            meters.values[&normalize_path(&manifest.destination)],
            ReadingMeterValue::new(3, 3)
        );
        assert_eq!(
            meters.values[&normalize_path(&sibling)],
            ReadingMeterValue::new(8, 10)
        );
        assert_eq!(
            position_reply(&writer, &manifest.source, cutoff)
                .position
                .unwrap(),
            ReadingPosition {
                page: 99,
                identity: None
            }
        );
        assert_eq!(
            position_reply(&writer, &sibling, cutoff)
                .position
                .unwrap()
                .page,
            7
        );
    }

    #[test]
    fn folder_zip_resume_legacy_identity_requires_unambiguous_captured_column() {
        use crate::image_folder_zip_metadata::CopyPhase;
        let (files, manifest) = folder_zip_fixture();
        let valid = Arc::new(vec!["p1.jpg".into(), "p2.jpg".into(), "p10.jpg".into()]);
        let conflicting = Arc::new(vec!["p10.jpg".into(), "p2.jpg".into(), "p1.jpg".into()]);
        for (case, columns, page, expected_identity, expected_page) in [
            ("none", Vec::new(), 0, None, 0),
            (
                "proven",
                vec![Arc::clone(&valid), Arc::clone(&valid)],
                0,
                Some("p1.jpg"),
                2,
            ),
            (
                "ambiguous",
                vec![Arc::clone(&valid), conflicting],
                0,
                None,
                0,
            ),
            ("out_of_range", vec![valid], 99, None, 99),
        ] {
            let writer =
                BookResumeWriter::spawn_at(files.path().join(format!("{case}.db"))).unwrap();
            assert!(writer.record(&manifest.source, page, ReadingMeterValue::new(1, 3)));
            let portal = writer.folder_zip_portal(
                manifest.source.clone(),
                manifest.destination.clone(),
                crate::settings::SortOrder::NumericDesc,
                columns,
            );
            assert_eq!(
                portal(&manifest, CopyPhase::Apply)
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap(),
                Ok(1)
            );
            let cutoff = writer.watermarks().admitted;
            let target = position_reply(&writer, &manifest.destination, cutoff)
                .position
                .unwrap();
            assert_eq!(target.page, expected_page, "{case}");
            assert_eq!(
                target.identity,
                expected_identity.map(|name| PageIdentity::Archive {
                    container: path_key::normalize_keep_drive(&manifest.destination),
                    entry_name: name.into(),
                }),
                "{case}"
            );
            assert_eq!(
                position_reply(&writer, &manifest.source, cutoff)
                    .position
                    .unwrap(),
                ReadingPosition {
                    page,
                    identity: None
                },
                "{case}"
            );
        }
    }

    fn position_reply(writer: &BookResumeWriter, book: &Path, cutoff: u64) -> PositionReadReply {
        writer
            .read_position_after(&normalize_path(book), cutoff)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }

    #[test]
    fn position_read_waits_for_pending_record_and_freezes_fifo_result() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = BookResumeWriter::spawn_at(tmp.path().join("resume.db")).unwrap();
        let book = Path::new("C:/books/a.zip");
        assert!(writer.record(book, 1, None));
        assert_eq!(position_reply(&writer, book, 1).position.unwrap().page, 1);
        let old = writer.position_snapshot().unwrap();
        let release = writer.pause_for_test();
        let identity = PageIdentity::Archive {
            container: path_key::normalize_keep_drive(book),
            entry_name: "page9.png".into(),
        };
        assert!(writer.record_identity(book, 9, None, Some(identity.clone())));
        let cutoff = writer.watermarks().admitted;
        assert_eq!(cutoff, 2);
        assert_eq!(writer.watermarks().completed_through, 1);
        assert!(writer.position_snapshot().is_none());
        assert!(old.read_after(&normalize_path(book), cutoff).is_none());
        let read = writer.read_position_after(&normalize_path(book), cutoff);
        assert!(matches!(read.try_recv(), Err(mpsc::TryRecvError::Empty)));
        // A later sibling action cannot replace the already enqueued read.
        assert!(writer.record(book, 14, None));
        release.send(()).unwrap();
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10)).unwrap(),
            PositionReadReply {
                cutoff: 2,
                completed_through: 2,
                position: Some(ReadingPosition {
                    page: 9,
                    identity: Some(identity)
                }),
                status: Ok(()),
            }
        );
        assert_eq!(position_reply(&writer, book, 3).position.unwrap().page, 14);
        let snapshot = writer.position_snapshot().unwrap();
        assert_eq!(snapshot.completed_through, 3);
        assert_eq!(snapshot.values[&normalize_path(book)].page, 14);
        assert_eq!(old.values[&normalize_path(book)].page, 1);
    }

    #[test]
    fn position_read_waits_for_pending_clear_and_external_read_cannot_bypass_it() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("resume.db");
        let writer = BookResumeWriter::spawn_at(db_path.clone()).unwrap();
        let book = Path::new("C:/books/a.zip");
        assert!(writer.record(book, 7, None));
        assert!(position_reply(&writer, book, 1).status.is_ok());
        let db = BookResumeDb::open_at(&db_path).unwrap();
        let observer = writer.watermark_reader();
        assert_eq!(
            db.read_position_proven(&normalize_path(book), 1, &observer)
                .unwrap()
                .position
                .unwrap()
                .page,
            7
        );
        let old = writer.position_snapshot().unwrap();
        let release = writer.pause_for_test();
        let cleared = writer.clear();
        assert_eq!(writer.watermarks().admitted, 2);
        assert!(writer.position_snapshot().is_none());
        assert!(old.read_after(&normalize_path(book), 2).is_none());
        assert!(
            db.read_position_proven(&normalize_path(book), 2, &observer)
                .is_none()
        );
        let read = writer.read_position_after(&normalize_path(book), 2);
        assert!(matches!(read.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            cleared.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(1)
        );
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10)).unwrap(),
            PositionReadReply {
                cutoff: 2,
                completed_through: 2,
                position: None,
                status: Ok(())
            }
        );
    }

    #[test]
    fn external_position_read_requires_stable_completed_watermarks() {
        let stable = ResumeWatermarks {
            admitted: 4,
            completed_through: 4,
        };
        assert!(ResumeReadProof::for_transaction(4, stable, stable).is_some());
        assert!(ResumeReadProof::for_transaction(5, stable, stable).is_none());
        let pending = ResumeWatermarks {
            admitted: 4,
            completed_through: 3,
        };
        // Unchanging admissions do not prove an outstanding Record is complete.
        assert!(ResumeReadProof::for_transaction(3, pending, pending).is_none());
        assert!(ResumeReadProof::for_transaction(4, pending, stable).is_none());
        let newer = ResumeWatermarks {
            admitted: 5,
            completed_through: 5,
        };
        assert!(ResumeReadProof::for_transaction(4, stable, newer).is_none());
    }

    #[test]
    fn admission_only_counts_successful_send() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let writer = BookResumeWriter {
            tx: Some(tx),
            handle: None,
            pending: Arc::new(AtomicUsize::new(0)),
            state: Arc::new(Mutex::new(WriterState::default())),
        };
        assert!(!writer.record(Path::new("C:/book.zip"), 2, None));
        assert_eq!(writer.watermarks(), ResumeWatermarks::default());
        assert_eq!(writer.pending.load(Ordering::Acquire), 0);
        assert!(matches!(
            writer.read_all().try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(writer.watermarks(), ResumeWatermarks::default());
        assert_eq!(writer.pending.load(Ordering::Acquire), 0);
        let reply = position_reply(&writer, Path::new("C:/book.zip"), 0);
        assert!(reply.status.is_err());
    }

    #[test]
    fn normal_drop_drains_record_clear_record() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("resume.db");
        let writer = BookResumeWriter::spawn_at(db_path.clone()).unwrap();
        let book = Path::new("C:/books/a.zip");
        let release = writer.pause_for_test();
        assert!(writer.record(book, 2, None));
        let clear = writer.clear();
        assert!(writer.record(book, 18, None));
        release.send(()).unwrap();
        drop(writer);
        assert_eq!(clear.recv().unwrap(), Ok(1));
        assert_eq!(BookResumeDb::open_at(&db_path).unwrap().get(book), Some(18));
    }

    #[test]
    fn failed_record_is_terminal_but_position_reply_is_not_successful_absence() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("resume.db");
        let writer = BookResumeWriter::spawn_at(db_path.clone()).unwrap();
        let book = Path::new("C:/books/a.zip");
        let release = writer.pause_for_test();
        let db = BookResumeDb::open_at(&db_path).unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_record BEFORE INSERT ON book_resume BEGIN SELECT RAISE(FAIL,'test write failure'); END").unwrap();
        assert!(writer.record(book, 2, None));
        let read = writer.read_position_after(&normalize_path(book), 1);
        release.send(()).unwrap();
        let reply = read.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply.completed_through, 1);
        assert!(reply.position.is_none());
        assert!(reply.status.unwrap_err().contains("test write failure"));
        let external = db
            .read_position_proven(&normalize_path(book), 1, &writer.watermark_reader())
            .unwrap();
        assert!(external.status.is_err());
    }

    #[test]
    fn page_identity_new_old_new_and_same_raw_invalidation() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("resume.db");
        let book = Path::new("C:/book.zip");
        let identity = PageIdentity::Archive {
            container: path_key::normalize_keep_drive(book),
            entry_name: "2.png".into(),
        };
        let meter = ReadingMeterValue::new(2, 3);
        let mut db = BookResumeDb::open_at(&db_path).unwrap();
        db.migrate_meter_columns().unwrap();
        db.set_position(book, 1, meter, Some(&identity), true)
            .unwrap();
        assert_eq!(
            db.get_position(book).unwrap().identity,
            Some(identity.clone())
        );
        // Exact released page-only SQL, running against the same DB after a downgrade.
        let old = rusqlite::Connection::open(&db_path).unwrap();
        for page in [2_i64, 1, 1] {
            old.execute(
                "INSERT INTO book_resume (path, page) VALUES (?1, ?2)
                ON CONFLICT(path) DO UPDATE SET page = ?2",
                rusqlite::params![normalize_path(book), page],
            )
            .unwrap();
            let reopened = BookResumeDb::open_at(&db_path).unwrap();
            assert_eq!(
                reopened.get_position(book).unwrap(),
                ReadingPosition {
                    page: page as usize,
                    identity: None
                }
            );
            assert_eq!(
                reopened
                    .read_meters()
                    .unwrap()
                    .values
                    .get(&normalize_path(book)),
                Some(&meter)
            );
            db.set_position(book, 1, meter, Some(&identity), true)
                .unwrap();
            assert_eq!(
                db.get_position(book).unwrap().identity,
                Some(identity.clone())
            );
        }
        db.set(book, 1).unwrap();
        assert!(db.get_position(book).unwrap().identity.is_none());
    }

    #[test]
    fn old_resume_schema_adds_identity_without_changing_raw() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.db");
        let old = rusqlite::Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE book_resume(path TEXT PRIMARY KEY,page INTEGER NOT NULL DEFAULT 0);
            INSERT INTO book_resume(path,page) VALUES ('/book.zip',1)",
        )
        .unwrap();
        drop(old);
        let db = BookResumeDb::open_at(&path).unwrap();
        assert_eq!(
            db.get_position(Path::new("C:/book.zip")).unwrap(),
            ReadingPosition {
                page: 1,
                identity: None
            }
        );
    }

    #[test]
    fn shared_writer_admission_orders_record_and_clear_across_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = Arc::new(BookResumeWriter::spawn_at(tmp.path().join("resume.db")).unwrap());
        let release = writer.pause_for_test();
        let (recorded_tx, recorded_rx) = mpsc::channel();
        let record_writer = Arc::clone(&writer);
        let record = std::thread::spawn(move || {
            assert!(record_writer.record(Path::new("C:/book.zip"), 2, None));
            recorded_tx.send(()).unwrap();
        });
        let clear_writer = Arc::clone(&writer);
        let clear = std::thread::spawn(move || {
            recorded_rx.recv().unwrap();
            clear_writer.clear()
        });
        record.join().unwrap();
        let clear_reply = clear.join().unwrap();
        assert_eq!(
            writer.watermarks(),
            ResumeWatermarks {
                admitted: 2,
                completed_through: 0
            }
        );
        let read = writer.read_position_after(&normalize_path(Path::new("C:/book.zip")), 2);
        release.send(()).unwrap();
        assert_eq!(
            clear_reply.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(1)
        );
        let reply = read.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply.completed_through, 2);
        assert_eq!(reply.position, None);
        assert_eq!(reply.status, Ok(()));
    }

    #[test]
    fn existing_meter_reload_refreshes_externally_changed_positions() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("resume.db");
        let writer = BookResumeWriter::spawn_at(db_path.clone()).unwrap();
        let book = Path::new("C:/book.zip");
        assert!(writer.record(book, 2, None));
        assert!(position_reply(&writer, book, 1).status.is_ok());
        let db = BookResumeDb::open_at(&db_path).unwrap();
        db.set(book, 17).unwrap();
        writer
            .read_all()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(position_reply(&writer, book, 1).position.unwrap().page, 17);
        db.remove(book).unwrap();
        writer
            .read_all()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert!(position_reply(&writer, book, 1).position.is_none());
    }

    #[test]
    fn pending_reload_fences_warm_retained_snapshot_and_external_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("resume.db");
        let writer = BookResumeWriter::spawn_at(db_path.clone()).unwrap();
        let book = Path::new("C:/book.zip");
        let key = normalize_path(book);
        assert!(writer.record(book, 2, None));
        assert_eq!(position_reply(&writer, book, 1).position.unwrap().page, 2);
        let before_reload = writer.position_snapshot().unwrap();
        let db = BookResumeDb::open_at(&db_path).unwrap();
        let observer = writer.watermark_reader();
        let release = writer.pause_for_test();
        // Existing maintenance updates rows externally, then accepts this reread.
        db.set(book, 17).unwrap();
        let reloaded = writer.read_all();
        let cutoff = writer.watermarks().admitted;
        assert_eq!(cutoff, 2);
        assert_eq!(writer.watermarks().completed_through, 1);
        assert_eq!(db.read_position(&key).unwrap().unwrap().page, 17);
        assert!(writer.position_snapshot().is_none());
        assert!(before_reload.read_after(&key, cutoff).is_none());
        // Even an external transaction which could already see the changed DB
        // cannot prove completion of the accepted, still unfinished reread.
        assert!(db.read_position_proven(&key, cutoff, &observer).is_none());
        assert!(db.read_position_proven(&key, 1, &observer).is_none());
        let restored = writer.read_position_after(&key, cutoff);
        assert!(matches!(
            restored.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(matches!(
            reloaded.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let meter = reloaded
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(meter.values[&key], None);
        let reply = restored.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(reply.cutoff, cutoff);
        assert_eq!(reply.completed_through, cutoff);
        assert_eq!(reply.position.unwrap().page, 17);
        assert_eq!(reply.status, Ok(()));
        let after_reload = writer.position_snapshot().unwrap();
        assert_eq!(after_reload.completed_through, cutoff);
        assert_eq!(after_reload.values[&key].page, 17);
        assert_eq!(before_reload.values[&key].page, 2);
        assert!(before_reload.read_after(&key, cutoff).is_none());
        let proven = db.read_position_proven(&key, cutoff, &observer).unwrap();
        assert_eq!(proven.completed_through, cutoff);
        assert_eq!(proven.position.unwrap().page, 17);
        assert_eq!(proven.status, Ok(()));
    }

    #[test]
    fn book_resume_meter_migration_preserves_legacy_and_nulls() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("resume.db");
        let mut db = BookResumeDb::open_at(&path).unwrap();
        let book = Path::new("C:/books/a.zip");
        db.set(book, 7).unwrap();
        db.migrate_meter_columns().unwrap();
        db.migrate_meter_columns().unwrap();
        let columns = db
            .conn
            .prepare("PRAGMA table_info(book_resume)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "path",
                "page",
                "page_identity",
                "page_ordinal",
                "page_total"
            ]
        );
        assert_eq!(db.get(book), Some(7));
        assert_eq!(db.get_position(book).unwrap().identity, None);
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            None
        );
        let meter = ReadingMeterValue::new(2, 4).unwrap();
        db.set_meter(book, 8, Some(meter)).unwrap();
        // 出荷済み2列版のupsert/SELECTをそのまま実行できる。新列が残るのは合意済み。
        db.set(book, 9).unwrap();
        assert_eq!(db.get(book), Some(9));
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            Some(meter)
        );
        db.set_meter(book, 10, None).unwrap();
        assert_eq!(
            db.read_meters().unwrap().values[&normalize_path(book)],
            None
        );
    }

    #[test]
    fn book_resume_meter_invalid_columns_are_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = BookResumeDb::open_at(&tmp.path().join("resume.db")).unwrap();
        db.migrate_meter_columns().unwrap();
        for (idx, (ordinal, total)) in [(0, 5), (1, 0), (6, 5), (-1, 5)].into_iter().enumerate() {
            db.conn
                .execute(
                    "INSERT INTO book_resume(path,page,page_ordinal,page_total) VALUES (?1,0,?2,?3)",
                    rusqlite::params![format!("bad{idx}"), ordinal, total],
                )
                .unwrap();
        }
        let snapshot = db.read_meters().unwrap();
        assert_eq!(snapshot.values.len(), 4);
        assert!(snapshot.values.values().all(Option::is_none));
        assert!(ReadingMeterValue::new(0, 1).is_none());
        assert!(ReadingMeterValue::new(1, 0).is_none());
        assert!(ReadingMeterValue::new(2, 1).is_none());
        assert_eq!(ReadingMeterValue::new(1, 1).unwrap().fraction(), 1.0);
    }

    #[test]
    fn book_resume_meter_failed_alter_rolls_back_and_raw_record_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = BookResumeDb::open_at(&tmp.path().join("resume.db")).unwrap();
        // SQLite既定列上限直前: 1列追加成功後に2列目が失敗する決定的なrollback検証。
        let columns = (0..1997)
            .map(|idx| format!(",legacy_{idx} INTEGER"))
            .collect::<String>();
        db.conn.execute_batch(&format!("DROP TABLE book_resume; CREATE TABLE book_resume(path TEXT PRIMARY KEY,page INTEGER NOT NULL DEFAULT 0{columns})")).unwrap();
        assert!(db.migrate_meter_columns().is_err());
        let columns: usize = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('book_resume') WHERE name='page_ordinal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 0);
        let book = Path::new("C:/books/a.zip");
        db.set(book, 12).unwrap();
        assert_eq!(db.get(book), Some(12));
    }

    #[test]
    fn book_resume_meter_worker_record_read_clear_fifo() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = BookResumeWriter::spawn_at(tmp.path().join("resume.db")).unwrap();
        let release = writer.pause_for_test();
        let book = Path::new("C:/books/a.zip");
        let value = ReadingMeterValue::new(3, 10).unwrap();
        assert!(writer.record(book, 9, Some(value)));
        assert!(writer.is_busy());
        let read = writer.read_all();
        assert!(matches!(read.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(
            read.recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .values[&normalize_path(book)],
            Some(value)
        );
        assert_eq!(
            writer
                .clear()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
            1
        );
        assert!(
            writer
                .read_all()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .values
                .is_empty()
        );
        drop(writer);
    }

    #[test]
    fn book_resume_set_get_remove() {
        // data_dir の test override を自分で張る。張らないと、たまたま生きている他
        // テストのガードへ相乗りして同じ DB を共有する。同型の相乗りは rotation_db で
        // 実際に CannotOpen を起こした (2026-09-01、リリースの test gate)。こちらは
        // open() が親ディレクトリを作り直すぶん落ちにくいだけで、分離できていない点は同じ。
        let _guard = crate::data_dir::TestDataDirGuard::new();
        let db = BookResumeDb::open().unwrap();
        let p = Path::new("C:/manga/book.zip");

        // テスト分離: 既存レコードを消してから
        db.remove(p).unwrap();
        assert!(db.get(p).is_none());

        db.set(p, 42).unwrap();
        assert_eq!(db.get(p), Some(42));

        // 上書き
        db.set(p, 7).unwrap();
        assert_eq!(db.get(p), Some(7));

        // 削除
        db.remove(p).unwrap();
        assert!(db.get(p).is_none());
    }
}
