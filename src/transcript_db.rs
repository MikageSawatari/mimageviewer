//! ローカル文字起こしの永続化。ファイルと SQLite の操作は worker 上だけで行う。
//! rollback journal を保ち、メタ情報取り込みの複数 DB commit に参加する。

pub use crate::ai::transcript_postprocess::RemovedCounts;
pub use crate::ai_caption_db::FileSignature;
use crate::ai_caption_db::{commit_unless_cancelled, system_time_ms};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// 保存形そのものを表す。時刻は再生時刻のミリ秒。
pub type TranscriptSegment = (u64, u64, String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptResult {
    Ok {
        segments: Vec<TranscriptSegment>,
        removed_counts: RemovedCounts,
        duration_ms: u64,
        audio_stream: usize,
    },
    Failed,
}
#[derive(Clone, Debug)]
pub struct SaveRequest {
    pub item_path: PathBuf,
    pub signature: FileSignature,
    pub model_id: String,
    pub config_version: i64,
    pub lang: String,
    pub result: TranscriptResult,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Written,
    KeptExistingOk,
    Discarded,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptStatus {
    Ok,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptRow {
    pub item_key: String,
    pub signature: FileSignature,
    pub audio_stream: Option<usize>,
    pub model_id: String,
    pub config_version: i64,
    pub lang: String,
    pub duration_ms: u64,
    pub segments: Vec<TranscriptSegment>,
    pub removed_counts: RemovedCounts,
    pub status: TranscriptStatus,
    pub created_at: i64,
    pub rev: i64,
    pub indexed: bool,
}
pub struct TranscriptDb {
    conn: Connection,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptDeletionCandidates {
    pub keys: Vec<String>,
    pub ok: usize,
    pub failed: usize,
}
impl TranscriptDb {
    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("transcript.db")
    }
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(Self::db_path())
    }
    pub fn open_at(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Self::ensure_schema(&conn)?;
        Ok(Self { conn })
    }
    /// Worker-only reads. Missing or not-yet-initialized stores are unpublished;
    /// never initializes the directory, schema, journal mode, or database file.
    pub fn open_read_only(path: impl AsRef<Path>) -> rusqlite::Result<Option<Self>> {
        let path = path.as_ref();
        let conn = match Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(conn) => conn,
            Err(error) => {
                if std::fs::metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // schema と版の公開は同じ commit。export と同じ境界で未公開だけを除く。
        if crate::ai_caption_db::schema_is_published(&conn)? {
            Ok(Some(Self { conn }))
        } else {
            Ok(None)
        }
    }

    /// Existing, already-initialized database; never creates a missing store.
    pub fn open_existing_for_write(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        if conn.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))? != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(Self { conn })
    }

    pub(crate) fn into_connection(self) -> Connection {
        self.conn
    }

    fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 0 && version != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS transcripts (
            item_key TEXT PRIMARY KEY NOT NULL,
            mtime INTEGER NOT NULL, size INTEGER NOT NULL,
            audio_stream INTEGER, model_id TEXT NOT NULL, config_version INTEGER NOT NULL,
            lang TEXT NOT NULL, duration_ms INTEGER NOT NULL,
            segments TEXT NOT NULL, removed_counts TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('ok','failed')),
            created_at INTEGER NOT NULL, rev INTEGER NOT NULL DEFAULT 0, indexed INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS transcript_seq(n INTEGER NOT NULL);
        INSERT INTO transcript_seq SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM transcript_seq);
        CREATE TABLE IF NOT EXISTS transcript_removed(item_key TEXT NOT NULL, rev INTEGER NOT NULL, PRIMARY KEY(item_key,rev));
        CREATE TRIGGER IF NOT EXISTS transcripts_ins AFTER INSERT ON transcripts BEGIN
            UPDATE transcript_seq SET n=n+1;
            UPDATE transcripts SET rev=(SELECT n FROM transcript_seq), indexed=0 WHERE item_key=NEW.item_key;
        END;
        CREATE TRIGGER IF NOT EXISTS transcripts_upd AFTER UPDATE OF
            item_key,mtime,size,audio_stream,model_id,config_version,lang,duration_ms,segments,removed_counts,status,created_at
            ON transcripts BEGIN
            UPDATE transcript_seq SET n=n+1;
            UPDATE transcripts SET rev=(SELECT n FROM transcript_seq), indexed=0 WHERE item_key=NEW.item_key;
            INSERT OR IGNORE INTO transcript_removed SELECT OLD.item_key,n FROM transcript_seq WHERE OLD.item_key<>NEW.item_key;
        END;
        CREATE TRIGGER IF NOT EXISTS transcripts_del AFTER DELETE ON transcripts BEGIN
            UPDATE transcript_seq SET n=n+1;
            INSERT OR IGNORE INTO transcript_removed SELECT OLD.item_key,n FROM transcript_seq;
        END;
        PRAGMA user_version=1;")?;
        tx.commit()
    }
    /// 説明文と同じ descriptor の正規化を使用する。
    pub fn normalize_path(path: &Path) -> String {
        crate::ai_caption_db::CaptionDb::normalize_path(path)
    }
    pub fn get(&self, item_key: &str) -> rusqlite::Result<Option<TranscriptRow>> {
        self.conn.query_row("SELECT item_key,mtime,size,audio_stream,model_id,config_version,lang,duration_ms,segments,removed_counts,status,created_at,rev,indexed FROM transcripts WHERE item_key=?1",
            [Self::normalize_path(Path::new(item_key))], |row| {
                let status: String = row.get(10)?;
                Ok(TranscriptRow {
                    item_key: row.get(0)?, signature: FileSignature {mtime_ms: row.get(1)?, size: row.get(2)?},
                    audio_stream: row.get::<_, Option<i64>>(3)?.map(|n| usize::try_from(n).map_err(|_| rusqlite::Error::InvalidQuery)).transpose()?,
                    model_id: row.get(4)?, config_version: row.get(5)?, lang: row.get(6)?, duration_ms: row.get(7)?,
                    segments: json_column(row, 8)?, removed_counts: json_column(row, 9)?,
                    status: match status.as_str() { "ok" => TranscriptStatus::Ok, "failed" => TranscriptStatus::Failed, _ => return Err(rusqlite::Error::InvalidQuery) },
                    created_at: row.get(11)?, rev: row.get(12)?, indexed: row.get(13)?,
                })
            }).optional()
    }
    /// 既存を飛ばす判定は成功行の有無だけ。古い成功行も飛ばし、failed は飛ばさない。
    pub fn has_ok(&self, item_key: &str) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcripts WHERE item_key=?1 AND status='ok')",
            [Self::normalize_path(Path::new(item_key))],
            |row| row.get(0),
        )
    }
    pub fn save(&self, request: &SaveRequest) -> rusqlite::Result<SaveOutcome> {
        self.save_inner(request, None)
            .map(|value| value.expect("uncancellable save"))
    }
    pub(crate) fn save_unless_cancelled(
        &self,
        request: &SaveRequest,
        cancel: &Arc<AtomicBool>,
    ) -> rusqlite::Result<Option<SaveOutcome>> {
        self.save_inner(request, Some(cancel))
    }
    fn save_inner(
        &self,
        request: &SaveRequest,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> rusqlite::Result<Option<SaveOutcome>> {
        let cancelled = || cancel.is_some_and(|flag| flag.load(Ordering::Acquire));
        let (status, segments, removed_counts, duration_ms, audio_stream) = match &request.result {
            TranscriptResult::Ok {
                segments,
                removed_counts,
                duration_ms,
                audio_stream,
            } => {
                validate_transcript(segments, *duration_ms)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                (
                    "ok",
                    serde_json::to_string(segments).expect("tuple serialize"),
                    serde_json::to_string(removed_counts).expect("counts serialize"),
                    *duration_ms,
                    Some(i64::try_from(*audio_stream).map_err(|_| rusqlite::Error::InvalidQuery)?),
                )
            }
            TranscriptResult::Failed => (
                "failed",
                "[]".into(),
                serde_json::to_string(&RemovedCounts::default()).expect("counts serialize"),
                0,
                None,
            ),
        };
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if cancelled() {
            tx.rollback()?;
            return Ok(None);
        }
        // 改名の付け替えと同じ writer lock の中で入力を再確認する。
        let signature = std::fs::metadata(&request.item_path)
            .ok()
            .as_ref()
            .and_then(FileSignature::from_metadata);
        if signature != Some(request.signature) {
            tx.rollback()?;
            return Ok(Some(SaveOutcome::Discarded));
        }
        if cancelled() {
            tx.rollback()?;
            return Ok(None);
        }
        let changed = tx.execute("INSERT INTO transcripts(item_key,mtime,size,audio_stream,model_id,config_version,lang,duration_ms,segments,removed_counts,status,created_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
            ON CONFLICT(item_key) DO UPDATE SET mtime=excluded.mtime,size=excluded.size,audio_stream=excluded.audio_stream,
                model_id=excluded.model_id,config_version=excluded.config_version,lang=excluded.lang,duration_ms=excluded.duration_ms,
                segments=excluded.segments,removed_counts=excluded.removed_counts,status=excluded.status,created_at=excluded.created_at
            WHERE excluded.status='ok' OR transcripts.status<>'ok'",
            params![Self::normalize_path(&request.item_path),request.signature.mtime_ms,request.signature.size,audio_stream,request.model_id,
                request.config_version,request.lang,duration_ms,segments,removed_counts,status,system_time_ms(SystemTime::now()).unwrap_or(0)])?;
        if let Some(cancel) = cancel {
            if !commit_unless_cancelled(&self.conn, tx, cancel, BUSY_TIMEOUT)? {
                return Ok(None);
            }
        } else {
            tx.commit()?;
        }
        // commit 後の取消では Written を消さない。完了採用が失効を必ず通知する。
        Ok(Some(if changed == 0 {
            SaveOutcome::KeptExistingOk
        } else {
            SaveOutcome::Written
        }))
    }
    /// Worker-only lookup. Keys are already normalized; never normalize them twice.
    pub(crate) fn count_existing(
        &self,
        keys: &[String],
    ) -> rusqlite::Result<TranscriptDeletionCandidates> {
        let tx = self.conn.unchecked_transaction()?;
        let mut candidates = TranscriptDeletionCandidates::default();
        {
            let mut query = tx.prepare("SELECT status FROM transcripts WHERE item_key = ?1")?;
            for key in keys {
                let status = query
                    .query_row([key], |row| row.get::<_, String>(0))
                    .optional()?;
                if let Some(status) = status {
                    if status == "ok" {
                        candidates.ok += 1;
                    } else {
                        candidates.failed += 1;
                    }
                    candidates.keys.push(key.clone());
                }
            }
        }
        tx.commit()?;
        Ok(candidates)
    }

    /// Worker-only bulk deletion of normalized keys. Existing triggers own indexing.
    pub(crate) fn delete_keys(&self, keys: &[String]) -> rusqlite::Result<usize> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let mut deleted = 0;
        {
            let mut delete = tx.prepare("DELETE FROM transcripts WHERE item_key = ?1")?;
            for key in keys {
                deleted += delete.execute([key])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// Worker-only destination initialization/purge. The trigger records removal.
    pub fn delete(&self, item_key: &str) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM transcripts WHERE item_key = ?1",
            [Self::normalize_path(Path::new(item_key))],
        )
    }
}

fn json_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<T> {
    let value: String = row.get(column)?;
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
/// transport と同じ時刻順を確認する。ゼロ長と重なりは正当な区間として許す。
pub(crate) fn validate_transcript(
    segments: &[TranscriptSegment],
    duration_ms: u64,
) -> Result<(), &'static str> {
    if duration_ms > i64::MAX as u64 {
        return Err("文字起こしの長さが不正です");
    }
    let mut previous = (0, 0);
    for (start, end, text) in segments {
        if start > end || *start < previous.0 || *end < previous.1 || text.trim().is_empty() {
            return Err("文字起こしの区間が不正です");
        }
        previous = (*start, *end);
    }
    Ok(())
}
#[cfg(test)]
mod tests;
