//! mIV タグの中央カタログ DB。
//!
//! `%APPDATA%/mimageviewer/tags.db` を正本にし、タグ名は保存時に `#` を
//! 持たない。UI 表示だけ `#` を付ける。

#[cfg(test)]
thread_local! { static READ_OPERATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn read_operations_for_test() -> usize {
    READ_OPERATIONS.with(std::cell::Cell::get)
}
#[cfg(test)]
fn count_read_for_test() {
    READ_OPERATIONS.with(|calls| calls.set(calls.get() + 1));
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, OptionalExtension, params, types::Value};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemTag {
    pub tag: String,
    pub tag_key: String,
    pub applied_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSummary {
    pub tag: String,
    pub tag_key: String,
    pub count: usize,
    pub last_applied_at: i64,
}

/// Query-scoped timestamp: unrelated tags on the item are excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagViewMatch {
    pub item_key: String,
    /// Unix seconds, the storage unit of `item_tags.applied_at`.
    pub applied_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagViewQueryOrder {
    Path,
    AppliedAtDesc,
    AppliedAtAsc,
}

const TAG_PATH_PREFIX_SQL: &str = "SELECT DISTINCT item_key
             FROM item_tags
             WHERE tag_key LIKE ?1 ESCAPE '\\'
             ORDER BY item_key COLLATE NOCASE ASC
             LIMIT ?2";
const TAG_PATH_EXACT_SQL: &str = "SELECT DISTINCT item_key
             FROM item_tags
             WHERE tag_key = ?1
             ORDER BY item_key COLLATE NOCASE ASC
             LIMIT ?2";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetagReport {
    pub old_key: String,
    pub new_key: String,
    pub affected_items: usize,
    pub removed_conflicts: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedSidecarTagItem {
    pub(crate) item_key: String,
    tags: Vec<ItemTag>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SidecarTagBatchReport {
    pub(crate) imported_items: usize,
    pub(crate) inserted_tags: usize,
    pub(crate) skipped_decided_items: usize,
    pub(crate) imported: Vec<(String, Vec<String>)>,
    pub(crate) transaction_committed: bool,
    pub(crate) sync_marker_recorded: bool,
    pub(crate) transaction_elapsed: std::time::Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidecarTagBatchOutcome {
    AlreadySynchronized,
    Applied(SidecarTagBatchReport),
    Cancelled,
}

/// `tag_item_state.source` に入れる値。
pub mod source {
    pub const EDIT: &str = "edit";
    pub const XMP_LEGACY: &str = "xmp_legacy";
    pub const SIDECAR: &str = "sidecar";
    pub const METADATA_IMPORT: &str = "metadata_import";
}

pub struct TagsDb {
    conn: Connection,
    path: PathBuf,
}

/// All tags.db writers, including worker and migration connections, publish here.
pub(crate) static TAG_WRITES: crate::page_edit_write_epoch::EditWriteEpoch =
    crate::page_edit_write_epoch::EditWriteEpoch::new();

impl TagsDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        Self::open_at(&path)
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::apply_pragmas(&conn)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// 起動時に schema 初期化済みの DB を一覧準備 worker から読み取り専用で開く。
    pub fn open_readonly(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("tags.db")
    }

    /// An initialized store's exact path for a worker-owned readonly connection.
    pub(crate) fn read_path(&self) -> &Path {
        &self.path
    }

    fn apply_pragmas(conn: &Connection) -> Result<(), rusqlite::Error> {
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS item_tags (
                item_key   TEXT    NOT NULL,
                tag        TEXT    NOT NULL,
                tag_key    TEXT    NOT NULL,
                applied_at INTEGER NOT NULL,
                PRIMARY KEY(item_key, tag_key)
             );
             CREATE INDEX IF NOT EXISTS idx_item_tags_tagkey
                ON item_tags(tag_key);

             CREATE TABLE IF NOT EXISTS tag_item_state (
                item_key   TEXT PRIMARY KEY,
                decided_at INTEGER NOT NULL,
                source     TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS tag_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS tag_sidecar_sync (
                folder_key    TEXT PRIMARY KEY,
                sidecar_mtime INTEGER NOT NULL
             );",
        )
    }

    pub fn backup_to(&self, target: &Path) -> Result<(), rusqlite::Error> {
        let target_str = target.to_string_lossy();
        let escaped = target_str.replace('\'', "''");
        let sql = format!("VACUUM INTO '{escaped}'");
        self.conn.execute_batch(&sql)
    }

    pub fn rotate_backups(&self) -> Result<(), rusqlite::Error> {
        let Some(data_dir) = self.path.parent() else {
            return Ok(());
        };
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        // 世代 rotate のアルゴリズム (T42 の snapshot-first 順序) は settings.db と
        // 共有の正本 (`db_backup`) を使う。
        crate::db_backup::rotate_generation_backups(
            data_dir,
            "tags.db",
            &|msg| crate::logger::log(format!("tags_db: {msg}")),
            &|tmp| self.backup_to(tmp).map_err(|e| e.to_string()),
        )
        .map_err(|msg| io_sqlite_error("rotate tags.db backups", std::io::Error::other(msg)))
    }

    fn rotate_backups_once(&self) {
        if !mark_backup_needed_for_path(&self.path) {
            return;
        }
        if let Err(e) = self.rotate_backups() {
            crate::logger::log(format!(
                "tags_db: rotate_backups failed (continuing with write): {e}"
            ));
        }
    }

    pub fn get_item_tags(&self, item_key: &str) -> Vec<ItemTag> {
        #[cfg(test)]
        count_read_for_test();
        let mut stmt = match self.conn.prepare_cached(
            "SELECT tag, tag_key, applied_at
             FROM item_tags
             WHERE item_key = ?1
             ORDER BY applied_at ASC, tag COLLATE NOCASE ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([item_key], |row| {
            Ok(ItemTag {
                tag: row.get(0)?,
                tag_key: row.get(1)?,
                applied_at: row.get(2)?,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub fn get_many_display_tags(&self, item_keys: &[String]) -> HashMap<String, Vec<String>> {
        #[cfg(test)]
        count_read_for_test();
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        if item_keys.is_empty() {
            return out;
        }
        for chunk in item_keys.chunks(500) {
            let placeholders = (0..chunk.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT item_key, tag
                 FROM item_tags
                 WHERE item_key IN ({})
                 ORDER BY item_key ASC, applied_at ASC, tag COLLATE NOCASE ASC",
                placeholders
            );
            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let item_key: String = row.get(0)?;
                let tag: String = row.get(1)?;
                Ok((item_key, tag))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    out.entry(row.0)
                        .or_default()
                        .push(format_display_tag(&row.1));
                }
            }
        }
        out
    }

    pub fn set_item_tags<I, S>(
        &mut self,
        item_key: &str,
        tags: I,
        source: &str,
    ) -> Result<Vec<String>, rusqlite::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [item_key])?;
        let normalized = collapse_tags(tags, now);
        {
            let mut stmt = tx.prepare(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for tag in &normalized {
                stmt.execute(params![item_key, tag.tag, tag.tag_key, tag.applied_at])?;
            }
        }
        upsert_item_state_tx(&tx, item_key, source, now)?;
        tx.commit()?;
        Ok(normalized
            .into_iter()
            .map(|t| format_display_tag(&t.tag))
            .collect())
    }

    pub fn union_item_tags<I, S>(
        &mut self,
        item_key: &str,
        tags: I,
        source: &str,
    ) -> Result<(Vec<String>, usize), rusqlite::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let normalized = collapse_tags(tags, now);
        let tx = self.conn.transaction()?;
        let mut inserted_tags = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for tag in &normalized {
                inserted_tags +=
                    stmt.execute(params![item_key, tag.tag, tag.tag_key, tag.applied_at])?;
            }
        }
        upsert_item_state_tx(&tx, item_key, source, now)?;
        tx.commit()?;
        Ok((self.display_tags_for_item(item_key), inserted_tags))
    }

    /// Import one sidecar snapshot in one WAL transaction.
    ///
    /// Both the durable decision row and legacy rows without a decision are
    /// authoritative.  The checks live in the same IMMEDIATE transaction as
    /// the inserts, so another connection cannot slip a user edit between the
    /// check and the write.  A disk sync marker, when eligible, is committed in
    /// this transaction too.  Pending writer sequence numbers are deliberately
    /// represented by `None` and are never persisted as disk markers.
    pub(crate) fn import_sidecar_tags_atomic(
        &mut self,
        folder_key: &str,
        sync_marker: Option<i64>,
        items: &[PreparedSidecarTagItem],
        cancel: &AtomicBool,
    ) -> Result<SidecarTagBatchOutcome, rusqlite::Error> {
        if let Some(marker) = sync_marker
            && self.sidecar_sync_get(folder_key) == Some(marker)
        {
            return Ok(SidecarTagBatchOutcome::AlreadySynchronized);
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(SidecarTagBatchOutcome::Cancelled);
        }

        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let transaction_started = std::time::Instant::now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(marker) = sync_marker {
            let current = tx
                .query_row(
                    "SELECT sidecar_mtime FROM tag_sidecar_sync WHERE folder_key = ?1",
                    [folder_key],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            if current == Some(marker) {
                return Ok(SidecarTagBatchOutcome::AlreadySynchronized);
            }
        }
        let mut report = SidecarTagBatchReport::default();
        for item in items {
            if cancel.load(Ordering::Relaxed) {
                return Ok(SidecarTagBatchOutcome::Cancelled);
            }
            let decided_or_existing = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM tag_item_state WHERE item_key = ?1
                 ) OR EXISTS(
                    SELECT 1 FROM item_tags WHERE item_key = ?1
                 )",
                [&item.item_key],
                |row| row.get::<_, bool>(0),
            )?;
            if decided_or_existing {
                report.skipped_decided_items += 1;
                continue;
            }

            let mut inserted = 0usize;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(item_key, tag_key) DO NOTHING",
                )?;
                for tag in &item.tags {
                    inserted +=
                        stmt.execute(params![item.item_key, tag.tag, tag.tag_key, tag.applied_at])?;
                }
            }
            upsert_item_state_tx(&tx, &item.item_key, source::SIDECAR, now_unix_secs())?;
            report.imported_items += 1;
            report.inserted_tags += inserted;
            report.imported.push((
                item.item_key.clone(),
                item.tags
                    .iter()
                    .map(|tag| format_display_tag(&tag.tag))
                    .collect(),
            ));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(SidecarTagBatchOutcome::Cancelled);
        }
        if let Some(marker) = sync_marker {
            tx.execute(
                "INSERT INTO tag_sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, ?2)
                 ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = excluded.sidecar_mtime",
                params![folder_key, marker],
            )?;
            report.sync_marker_recorded = true;
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(SidecarTagBatchOutcome::Cancelled);
        }
        tx.commit()?;
        report.transaction_committed = true;
        report.transaction_elapsed = transaction_started.elapsed();
        Ok(SidecarTagBatchOutcome::Applied(report))
    }

    pub fn toggle_item_tag(
        &mut self,
        item_key: &str,
        tag_name: &str,
    ) -> Result<(TagToggleOutcome, Vec<String>, Vec<String>), rusqlite::Error> {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let tag = normalize_tag_display_name(tag_name);
        let tag_key = normalize_tag_key(&tag);
        let before = self.display_tags_for_item(item_key);
        if !is_valid_tag_display_name(&tag) {
            return Ok((TagToggleOutcome::NoOp, before.clone(), before));
        }
        let tx = self.conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM item_tags WHERE item_key = ?1 AND tag_key = ?2",
            params![item_key, tag_key],
        )?;
        let outcome = if removed > 0 {
            TagToggleOutcome::Removed
        } else {
            tx.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(item_key, tag_key) DO UPDATE SET
                    tag = excluded.tag,
                    applied_at = excluded.applied_at",
                params![item_key, tag, tag_key, now],
            )?;
            TagToggleOutcome::Added
        };
        upsert_item_state_tx(&tx, item_key, source::EDIT, now)?;
        tx.commit()?;
        let after = self.display_tags_for_item(item_key);
        Ok((outcome, before, after))
    }

    pub fn clear_item_tags(
        &mut self,
        item_key: &str,
    ) -> Result<(bool, Vec<String>, Vec<String>), rusqlite::Error> {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let before = self.display_tags_for_item(item_key);
        let tx = self.conn.transaction()?;
        let changed = tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [item_key])? > 0;
        upsert_item_state_tx(&tx, item_key, source::EDIT, now)?;
        tx.commit()?;
        Ok((changed, before, Vec::new()))
    }

    pub fn copy_item_key(&mut self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [to_key])?;
        tx.execute("DELETE FROM tag_item_state WHERE item_key = ?1", [to_key])?;
        tx.execute(
            "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
             SELECT ?2, tag, tag_key, applied_at
             FROM item_tags
             WHERE item_key = ?1",
            params![from_key, to_key],
        )?;
        tx.execute(
            "INSERT INTO tag_item_state (item_key, decided_at, source)
             SELECT ?2, decided_at, source
             FROM tag_item_state
             WHERE item_key = ?1",
            params![from_key, to_key],
        )?;
        tx.commit()
    }

    pub fn move_item_key(&mut self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [to_key])?;
        tx.execute("DELETE FROM tag_item_state WHERE item_key = ?1", [to_key])?;
        tx.execute(
            "INSERT INTO item_tags (item_key, tag, tag_key, applied_at)
             SELECT ?2, tag, tag_key, applied_at
             FROM item_tags
             WHERE item_key = ?1",
            params![from_key, to_key],
        )?;
        tx.execute(
            "INSERT INTO tag_item_state (item_key, decided_at, source)
             SELECT ?2, decided_at, source
             FROM tag_item_state
             WHERE item_key = ?1",
            params![from_key, to_key],
        )?;
        tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [from_key])?;
        tx.execute("DELETE FROM tag_item_state WHERE item_key = ?1", [from_key])?;
        tx.commit()
    }

    pub fn retag_key(
        &mut self,
        old_key: &str,
        new_name: &str,
    ) -> Result<RetagReport, rusqlite::Error> {
        let old_key = normalize_tag_key(old_key);
        let new_display = normalize_tag_display_name(new_name);
        let new_key = normalize_tag_key(&new_display);
        if old_key.is_empty() || !is_valid_tag_display_name(&new_display) {
            return Ok(RetagReport::default());
        }

        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let tx = self.conn.transaction()?;
        let affected_items = item_keys_for_tag_key_tx(&tx, &old_key)?;
        let mut removed_conflicts = 0usize;

        if old_key == new_key {
            tx.execute(
                "UPDATE item_tags SET tag = ?1 WHERE tag_key = ?2",
                params![new_display, old_key],
            )?;
        } else {
            tx.execute(
                "UPDATE item_tags
                 SET tag = ?1, tag_key = ?2
                 WHERE tag_key = ?3
                   AND item_key NOT IN (
                       SELECT item_key FROM item_tags WHERE tag_key = ?2
                   )",
                params![new_display, new_key, old_key],
            )?;
            removed_conflicts = tx.execute(
                "DELETE FROM item_tags
                 WHERE tag_key = ?1
                   AND item_key IN (
                       SELECT item_key FROM item_tags WHERE tag_key = ?2
                   )",
                params![old_key, new_key],
            )?;
            tx.execute(
                "UPDATE item_tags SET tag = ?1 WHERE tag_key = ?2",
                params![new_display, new_key],
            )?;
        }

        for item_key in &affected_items {
            upsert_item_state_tx(&tx, item_key, source::EDIT, now)?;
        }
        tx.commit()?;

        Ok(RetagReport {
            old_key,
            new_key,
            affected_items: affected_items.len(),
            removed_conflicts,
        })
    }

    pub fn display_tags_for_item(&self, item_key: &str) -> Vec<String> {
        self.get_item_tags(item_key)
            .into_iter()
            .map(|t| format_display_tag(&t.tag))
            .collect()
    }

    pub fn has_item_state(&self, item_key: &str) -> bool {
        #[cfg(test)]
        count_read_for_test();
        self.conn
            .query_row(
                "SELECT 1 FROM tag_item_state WHERE item_key = ?1 LIMIT 1",
                [item_key],
                |_| Ok(()),
            )
            .is_ok()
    }

    pub fn upsert_item_state(
        &mut self,
        item_key: &str,
        source: &str,
    ) -> Result<(), rusqlite::Error> {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let now = now_unix_secs();
        let tx = self.conn.transaction()?;
        upsert_item_state_tx(&tx, item_key, source, now)?;
        tx.commit()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        self.conn.execute(
            "INSERT INTO tag_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn sidecar_sync_get(&self, folder_key: &str) -> Option<i64> {
        #[cfg(test)]
        count_read_for_test();
        self.conn
            .prepare_cached("SELECT sidecar_mtime FROM tag_sidecar_sync WHERE folder_key = ?1")
            .ok()?
            .query_row([folder_key], |row| row.get(0))
            .ok()
    }

    pub fn sidecar_sync_upsert(
        &self,
        folder_key: &str,
        sidecar_mtime: i64,
    ) -> Result<(), rusqlite::Error> {
        let _tag_write = TAG_WRITES.begin();
        self.conn.execute(
            "INSERT INTO tag_sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, ?2)
             ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = excluded.sidecar_mtime",
            params![folder_key, sidecar_mtime],
        )?;
        Ok(())
    }

    pub fn sidecar_sync_clear(&self, folder_key: &str) -> Result<(), rusqlite::Error> {
        let _tag_write = TAG_WRITES.begin();
        self.conn
            .execute(
                "DELETE FROM tag_sidecar_sync WHERE folder_key = ?1",
                [folder_key],
            )
            .map(|_| ())
    }

    pub fn tag_summaries(&self) -> Vec<TagSummary> {
        #[cfg(test)]
        count_read_for_test();
        let mut stmt = match self.conn.prepare(
            "SELECT tag, tag_key, COUNT(*) AS c, MAX(applied_at) AS last_at
             FROM item_tags
             GROUP BY tag_key
             ORDER BY LOWER(tag) ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            let count: i64 = row.get(2)?;
            Ok(TagSummary {
                tag: row.get(0)?,
                tag_key: row.get(1)?,
                count: count.max(0) as usize,
                last_applied_at: row.get(3)?,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    /// タグが 1 件でも存在するかを、集約せず先頭行だけで確認する。
    pub fn has_any_tags(&self) -> Result<bool, rusqlite::Error> {
        #[cfg(test)]
        count_read_for_test();
        self.conn
            .query_row("SELECT 1 FROM item_tags LIMIT 1", [], |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
    }

    pub fn find_by_prefix(&self, prefix: &str, limit: usize) -> Vec<TagSummary> {
        #[cfg(test)]
        count_read_for_test();
        let key = normalize_tag_key(prefix);
        let escaped = escape_like_pattern(&key);
        let pattern = format!("{escaped}%");
        let mut stmt = match self.conn.prepare(
            "SELECT tag, tag_key, COUNT(*) AS c, MAX(applied_at) AS last_at
             FROM item_tags
             WHERE tag_key LIKE ?1 ESCAPE '\\'
             GROUP BY tag_key
             ORDER BY last_at DESC, LOWER(tag) ASC
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(params![pattern, limit as i64], |row| {
            let count: i64 = row.get(2)?;
            Ok(TagSummary {
                tag: row.get(0)?,
                tag_key: row.get(1)?,
                count: count.max(0) as usize,
                last_applied_at: row.get(3)?,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub fn find_exact(&self, tag: &str) -> Option<TagSummary> {
        #[cfg(test)]
        count_read_for_test();
        let key = normalize_tag_key(tag);
        if key.is_empty() {
            return None;
        }
        self.conn
            .query_row(
                "SELECT tag, tag_key, COUNT(*) AS c, MAX(applied_at) AS last_at
                 FROM item_tags
                 WHERE tag_key = ?1
                 GROUP BY tag_key
                 LIMIT 1",
                [key],
                |row| {
                    let count: i64 = row.get(2)?;
                    Ok(TagSummary {
                        tag: row.get(0)?,
                        tag_key: row.get(1)?,
                        count: count.max(0) as usize,
                        last_applied_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn item_keys_by_tag_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        #[cfg(test)]
        count_read_for_test();
        let key = normalize_tag_key(prefix);
        if key.is_empty() || limit == 0 {
            return Vec::new();
        }
        let escaped = escape_like_pattern(&key);
        let pattern = format!("{escaped}%");
        let mut stmt = match self.conn.prepare(TAG_PATH_PREFIX_SQL) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(params![pattern, limit as i64], |row| row.get(0)) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub fn item_keys_by_tag_exact(&self, tag: &str, limit: usize) -> Vec<String> {
        #[cfg(test)]
        count_read_for_test();
        let key = normalize_tag_key(tag);
        if key.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut stmt = match self.conn.prepare(TAG_PATH_EXACT_SQL) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(params![key, limit as i64], |row| row.get(0)) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub fn item_keys_by_tag_terms_and(&self, terms: &[String], limit: usize) -> Vec<String> {
        #[cfg(test)]
        count_read_for_test();
        let mut matches = Vec::new();
        for term in terms {
            let key = normalize_tag_key(term);
            if key.is_empty() || matches.iter().any(|m: &TagQueryMatch| m.key == key) {
                continue;
            }
            if self.find_exact(&key).is_some() {
                matches.push(TagQueryMatch {
                    key,
                    kind: TagQueryMatchKind::Exact,
                });
            } else {
                matches.push(TagQueryMatch {
                    key,
                    kind: TagQueryMatchKind::Prefix,
                });
            }
        }
        if matches.is_empty() || limit == 0 {
            return Vec::new();
        }
        if matches.len() == 1 {
            return match matches[0].kind {
                TagQueryMatchKind::Exact => self.item_keys_by_tag_exact(&matches[0].key, limit),
                TagQueryMatchKind::Prefix => self.item_keys_by_tag_prefix(&matches[0].key, limit),
            };
        }

        matches.sort_by_key(|m| match m.kind {
            TagQueryMatchKind::Exact => 0,
            TagQueryMatchKind::Prefix => 1,
        });

        let (mut sql, mut values) = tag_candidate_sql(&matches);
        sql.push_str(" ORDER BY base.item_key COLLATE NOCASE ASC LIMIT ?");
        values.push(Value::Integer(limit as i64));

        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(rusqlite::params_from_iter(values), |row| row.get(0)) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub(crate) fn tag_view_matches_exact(
        &self,
        tag: &str,
        order: TagViewQueryOrder,
        limit: usize,
    ) -> Vec<TagViewMatch> {
        if order == TagViewQueryOrder::Path {
            let keys = self.item_keys_by_tag_exact(tag, limit);
            return self.tag_view_matches_for_keys(
                &[TagQueryMatch {
                    key: normalize_tag_key(tag),
                    kind: TagQueryMatchKind::Exact,
                }],
                &keys,
            );
        }
        self.tag_view_matches(
            &[TagQueryMatch {
                key: normalize_tag_key(tag),
                kind: TagQueryMatchKind::Exact,
            }],
            order,
            limit,
        )
    }

    pub(crate) fn tag_view_matches_prefix(
        &self,
        prefix: &str,
        order: TagViewQueryOrder,
        limit: usize,
    ) -> Vec<TagViewMatch> {
        if order == TagViewQueryOrder::Path {
            let keys = self.item_keys_by_tag_prefix(prefix, limit);
            return self.tag_view_matches_for_keys(
                &[TagQueryMatch {
                    key: normalize_tag_key(prefix),
                    kind: TagQueryMatchKind::Prefix,
                }],
                &keys,
            );
        }
        self.tag_view_matches(
            &[TagQueryMatch {
                key: normalize_tag_key(prefix),
                kind: TagQueryMatchKind::Prefix,
            }],
            order,
            limit,
        )
    }

    pub(crate) fn tag_view_matches_terms_and(
        &self,
        terms: &[String],
        order: TagViewQueryOrder,
        limit: usize,
    ) -> Vec<TagViewMatch> {
        let mut matches = Vec::new();
        for term in terms {
            let key = normalize_tag_key(term);
            if key.is_empty() || matches.iter().any(|m: &TagQueryMatch| m.key == key) {
                continue;
            }
            let kind = if self.find_exact(&key).is_some() {
                TagQueryMatchKind::Exact
            } else {
                TagQueryMatchKind::Prefix
            };
            matches.push(TagQueryMatch { key, kind });
        }
        if order == TagViewQueryOrder::Path {
            let keys = self.item_keys_by_tag_terms_and(terms, limit);
            return self.tag_view_matches_for_keys(&matches, &keys);
        }
        self.tag_view_matches(&matches, order, limit)
    }

    fn tag_view_matches_for_keys(
        &self,
        matches: &[TagQueryMatch],
        item_keys: &[String],
    ) -> Vec<TagViewMatch> {
        // The established path query owns candidate selection and its LIMIT.
        // Fetch timestamps only for those keys, using the item_key index.
        let mut timestamps = HashMap::new();
        for keys in item_keys.chunks(500) {
            let (sql, values) = tag_selected_timestamps_sql(matches, keys);
            let mut stmt = match self.conn.prepare(&sql) {
                Ok(stmt) => stmt,
                Err(_) => return Vec::new(),
            };
            let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            });
            let Ok(rows) = rows else {
                return Vec::new();
            };
            timestamps.extend(rows.flatten());
        }
        item_keys
            .iter()
            .filter_map(|key| {
                timestamps.remove(key).map(|applied_at| TagViewMatch {
                    item_key: key.clone(),
                    applied_at,
                })
            })
            .collect()
    }

    fn tag_view_matches(
        &self,
        matches: &[TagQueryMatch],
        order: TagViewQueryOrder,
        limit: usize,
    ) -> Vec<TagViewMatch> {
        #[cfg(test)]
        count_read_for_test();
        if matches.is_empty() || matches.iter().any(|m| m.key.is_empty()) || limit == 0 {
            return Vec::new();
        }

        let (sql, values) = tag_time_order_sql(matches, order, limit);
        let mut stmt = match self.conn.prepare(&sql) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(TagViewMatch {
                item_key: row.get(0)?,
                applied_at: row.get(1)?,
            })
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.flatten().collect()
    }

    pub fn prune_items(&mut self, item_keys: &[String]) -> Result<usize, rusqlite::Error> {
        if item_keys.is_empty() {
            return Ok(0);
        }
        self.rotate_backups_once();
        let _tag_write = TAG_WRITES.begin();
        let tx = self.conn.transaction()?;
        let mut removed_tags = 0usize;
        {
            let mut delete_tags = tx.prepare("DELETE FROM item_tags WHERE item_key = ?1")?;
            let mut delete_state = tx.prepare("DELETE FROM tag_item_state WHERE item_key = ?1")?;
            for key in item_keys {
                removed_tags += delete_tags.execute([key])?;
                delete_state.execute([key])?;
            }
        }
        tx.commit()?;
        Ok(removed_tags)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagToggleOutcome {
    Added,
    Removed,
    NoOp,
}

static TAG_BACKUP_DONE_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn mark_backup_needed_for_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    let registry = TAG_BACKUP_DONE_PATHS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut done) = registry.lock() else {
        crate::logger::log("tags_db: backup registry poisoned; skipping backup rotation");
        return false;
    };
    done.insert(path.to_path_buf())
}

fn io_sqlite_error(context: &str, err: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::CannotOpen,
            extended_code: 0,
        },
        Some(format!("{context}: {err}")),
    )
}

pub fn item_key_for_path(path: &Path) -> String {
    crate::adjustment_db::normalize_path(path)
}

pub fn normalize_tag_display_name(name: &str) -> String {
    name.trim()
        .trim_start_matches('#')
        .nfkc()
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn normalize_tag_key(name: &str) -> String {
    normalize_tag_display_name(name).to_lowercase()
}

pub fn tag_display_name_has_whitespace(name: &str) -> bool {
    name.chars().any(char::is_whitespace)
}

pub fn is_valid_tag_display_name(name: &str) -> bool {
    let display = normalize_tag_display_name(name);
    !display.is_empty()
        && display.chars().count() <= 64
        && !tag_display_name_has_whitespace(&display)
}

pub fn format_display_tag(tag: &str) -> String {
    let clean = normalize_tag_display_name(tag);
    if clean.is_empty() {
        String::new()
    } else {
        format!("#{clean}")
    }
}

#[derive(Clone, Debug)]
struct TagQueryMatch {
    key: String,
    kind: TagQueryMatchKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagQueryMatchKind {
    Exact,
    Prefix,
}

/// The caller puts exact terms first so candidate selection can use tag_key.
/// Keep this WHERE structure shared with the established path-order query.
fn tag_candidate_sql(matches: &[TagQueryMatch]) -> (String, Vec<Value>) {
    let mut sql = String::from("SELECT DISTINCT base.item_key FROM item_tags base WHERE ");
    let mut values = Vec::new();
    append_tag_match_sql(&mut sql, "base", &matches[0], &mut values);
    for (idx, tag_match) in matches.iter().enumerate().skip(1) {
        let alias = format!("t{idx}");
        sql.push_str(" AND EXISTS (SELECT 1 FROM item_tags ");
        sql.push_str(&alias);
        sql.push_str(" WHERE ");
        sql.push_str(&alias);
        sql.push_str(".item_key = base.item_key AND ");
        append_tag_match_sql(&mut sql, &alias, tag_match, &mut values);
        sql.push(')');
    }
    (sql, values)
}

fn append_matching_tags_sql(sql: &mut String, matches: &[TagQueryMatch], values: &mut Vec<Value>) {
    sql.push('(');
    for (idx, tag_match) in matches.iter().enumerate() {
        if idx > 0 {
            sql.push_str(" OR ");
        }
        append_tag_match_sql(sql, "matched", tag_match, values);
    }
    sql.push(')');
}

fn tag_selected_timestamps_sql(
    matches: &[TagQueryMatch],
    item_keys: &[String],
) -> (String, Vec<Value>) {
    let placeholders = vec!["(?)"; item_keys.len()].join(",");
    let mut sql = format!(
        "WITH selected(item_key) AS (VALUES {placeholders}) \
         SELECT selected.item_key, (SELECT MAX(matched.applied_at) FROM item_tags matched \
         WHERE matched.item_key = selected.item_key AND "
    );
    let mut values = item_keys.iter().cloned().map(Value::Text).collect();
    append_matching_tags_sql(&mut sql, matches, &mut values);
    sql.push_str(") FROM selected");
    (sql, values)
}

fn tag_time_order_sql(
    matches: &[TagQueryMatch],
    order: TagViewQueryOrder,
    limit: usize,
) -> (String, Vec<Value>) {
    let mut candidates = matches.to_vec();
    candidates.sort_by_key(|m| match m.kind {
        TagQueryMatchKind::Exact => 0,
        TagQueryMatchKind::Prefix => 1,
    });
    let (candidate_sql, candidate_values) = tag_candidate_sql(&candidates);
    // The OR belongs under an item_key lookup, never at the candidate root.
    // In particular, exact + prefix AND must start at the exact tag's index.
    // DISTINCT keeps the candidate subquery from flattening into matching rows.
    let mut sql = String::from(
        "SELECT candidate.item_key, (SELECT MAX(matched.applied_at) \
         FROM item_tags matched WHERE matched.item_key = candidate.item_key AND ",
    );
    let mut values = Vec::new();
    append_matching_tags_sql(&mut sql, matches, &mut values);
    sql.push_str(") AS applied_at FROM (");
    sql.push_str(&candidate_sql);
    values.extend(candidate_values);
    sql.push_str(") candidate ORDER BY applied_at ");
    sql.push_str(match order {
        TagViewQueryOrder::AppliedAtDesc => "DESC",
        TagViewQueryOrder::AppliedAtAsc => "ASC",
        TagViewQueryOrder::Path => unreachable!("path queries use existing key selection"),
    });
    // No candidate LIMIT: all timestamps must participate before the final cap.
    sql.push_str(", candidate.item_key ASC LIMIT ?");
    values.push(Value::Integer(limit as i64));
    (sql, values)
}

fn append_tag_match_sql(
    sql: &mut String,
    alias: &str,
    tag_match: &TagQueryMatch,
    values: &mut Vec<Value>,
) {
    sql.push_str(alias);
    match tag_match.kind {
        TagQueryMatchKind::Exact => {
            sql.push_str(".tag_key = ?");
            values.push(Value::Text(tag_match.key.clone()));
        }
        TagQueryMatchKind::Prefix => {
            sql.push_str(".tag_key LIKE ? ESCAPE '\\'");
            values.push(Value::Text(format!(
                "{}%",
                escape_like_pattern(&tag_match.key)
            )));
        }
    }
}

pub fn strip_display_hash(tag: &str) -> &str {
    tag.trim().trim_start_matches('#')
}

pub fn collapse_tags<I, S>(tags: I, fallback_applied_at: i64) -> Vec<ItemTag>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut by_key: BTreeMap<String, ItemTag> = BTreeMap::new();
    for tag in tags {
        let display = normalize_tag_display_name(tag.as_ref());
        let tag_key = normalize_tag_key(&display);
        if tag_key.is_empty()
            || display.chars().count() > 64
            || tag_display_name_has_whitespace(&display)
        {
            continue;
        }
        by_key.insert(
            tag_key.clone(),
            ItemTag {
                tag: display,
                tag_key,
                applied_at: fallback_applied_at,
            },
        );
    }
    by_key.into_values().collect()
}

pub(crate) fn prepare_sidecar_tag_item(
    item_key: String,
    tags: &[String],
) -> Result<Option<PreparedSidecarTagItem>, String> {
    if tags.is_empty() {
        return Ok(None);
    }
    let mut normalized = Vec::with_capacity(tags.len());
    for raw in tags {
        let display = normalize_tag_display_name(strip_display_hash(raw));
        let tag_key = normalize_tag_key(&display);
        if tag_key.is_empty()
            || display.chars().count() > 64
            || tag_display_name_has_whitespace(&display)
        {
            return Err(format!("invalid sidecar tag for {item_key}"));
        }
        normalized.push(display);
    }
    Ok(Some(PreparedSidecarTagItem {
        item_key,
        tags: collapse_tags(normalized, now_unix_secs()),
    }))
}

/// SQL LIKE エスケープは [`crate::adjustment_db::escape_like_pattern`] を共有する
/// (独自コピーを持つと将来のエスケープ修正が片方にしか入らない)。
pub use crate::adjustment_db::escape_like_pattern;

fn upsert_item_state_tx(
    tx: &rusqlite::Transaction<'_>,
    item_key: &str,
    source: &str,
    now: i64,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO tag_item_state (item_key, decided_at, source)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(item_key) DO UPDATE SET
            decided_at = excluded.decided_at,
            source = excluded.source",
        params![item_key, now, source],
    )?;
    Ok(())
}

fn item_keys_for_tag_key_tx(
    tx: &rusqlite::Transaction<'_>,
    tag_key: &str,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT DISTINCT item_key
         FROM item_tags
         WHERE tag_key = ?1
         ORDER BY item_key COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map([tag_key], |row| row.get(0))?;
    rows.collect()
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> TagsDb {
        let conn = Connection::open_in_memory().unwrap();
        TagsDb::apply_pragmas(&conn).unwrap();
        TagsDb::init_schema(&conn).unwrap();
        TagsDb {
            conn,
            path: PathBuf::new(),
        }
    }

    #[test]
    fn normalize_strips_hash_and_nfkc_lowercases_key() {
        assert_eq!(normalize_tag_display_name("  ##ＦＡＴＥ  "), "FATE");
        assert_eq!(normalize_tag_key("  #ＦＡＴＥ  "), "fate");
    }

    #[test]
    fn tag_validation_rejects_whitespace_inside_name() {
        assert!(is_valid_tag_display_name("原神"));
        assert!(!is_valid_tag_display_name("Blue Archive"));
        assert!(!is_valid_tag_display_name("Blue\tArchive"));
        assert!(!is_valid_tag_display_name("Blue　Archive"));
    }

    #[test]
    fn busy_timeout_pragma_is_applied() {
        let db = memory_db();
        let timeout_ms: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout_ms, 5000);
    }

    #[test]
    fn sidecar_sync_roundtrip() {
        let db = memory_db();
        assert_eq!(db.sidecar_sync_get("c:/pics"), None);
        db.sidecar_sync_upsert("c:/pics", 123).unwrap();
        assert_eq!(db.sidecar_sync_get("c:/pics"), Some(123));
        db.sidecar_sync_upsert("c:/pics", 456).unwrap();
        assert_eq!(db.sidecar_sync_get("c:/pics"), Some(456));
        db.sidecar_sync_clear("c:/pics").unwrap();
        assert_eq!(db.sidecar_sync_get("c:/pics"), None);
    }

    #[test]
    fn toggle_and_clear_roundtrip_display_tags() {
        let mut db = memory_db();
        let (outcome, before, after) = db.toggle_item_tag("c:/a.jpg", "#原神").unwrap();
        assert_eq!(outcome, TagToggleOutcome::Added);
        assert!(before.is_empty());
        assert_eq!(after, vec!["#原神"]);

        let (outcome, before, after) = db.toggle_item_tag("c:/a.jpg", "原神").unwrap();
        assert_eq!(outcome, TagToggleOutcome::Removed);
        assert_eq!(before, vec!["#原神"]);
        assert!(after.is_empty());

        let (changed, before, after) = db.clear_item_tags("c:/a.jpg").unwrap();
        assert!(!changed);
        assert!(before.is_empty());
        assert!(after.is_empty());
        assert!(db.has_item_state("c:/a.jpg"));
    }

    #[test]
    fn set_item_tags_collapses_equivalent_keys() {
        let mut db = memory_db();
        let after = db
            .set_item_tags("c:/a.jpg", ["#ＦＡＴＥ", "fate"], source::EDIT)
            .unwrap();
        assert_eq!(after, vec!["#fate"]);
    }

    #[test]
    fn set_and_toggle_reject_whitespace_tags() {
        let mut db = memory_db();
        let after = db
            .set_item_tags("c:/a.jpg", ["Blue Archive", "原神"], source::EDIT)
            .unwrap();
        assert_eq!(after, vec!["#原神"]);

        let (outcome, _, after) = db.toggle_item_tag("c:/a.jpg", "Blue Archive").unwrap();
        assert_eq!(outcome, TagToggleOutcome::NoOp);
        assert_eq!(after, vec!["#原神"]);
    }

    #[test]
    fn union_item_tags_preserves_existing_and_marks_state() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["#Cat"], source::EDIT)
            .unwrap();
        let (after, inserted) = db
            .union_item_tags("c:/a.jpg", ["#ＣＡＴ", "#Dog"], source::XMP_LEGACY)
            .unwrap();
        assert_eq!(inserted, 1);
        assert_eq!(after, vec!["#Cat".to_string(), "#Dog".to_string()]);
        assert!(db.has_item_state("c:/a.jpg"));
    }

    #[test]
    fn union_item_tags_empty_input_marks_state_without_deleting_tags() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["#Cat"], source::EDIT)
            .unwrap();
        let (after, inserted) = db
            .union_item_tags("c:/a.jpg", std::iter::empty::<&str>(), source::XMP_LEGACY)
            .unwrap();
        assert_eq!(inserted, 0);
        assert_eq!(after, vec!["#Cat".to_string()]);
        assert!(db.has_item_state("c:/a.jpg"));
    }

    #[test]
    fn prefix_search_escapes_like_wildcards() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["a_b", "a%b", "abc"], source::EDIT)
            .unwrap();
        let found = db.find_by_prefix("a_", 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tag, "a_b");
    }

    #[test]
    fn exact_search_returns_single_summary() {
        let mut db = memory_db();
        db.set_item_tags("c:/cat.jpg", ["cat"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/cat2.jpg", ["#ＣＡＴ"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/catnap.jpg", ["catnap"], source::EDIT)
            .unwrap();

        let found = db.find_exact("#cat").unwrap();
        assert_eq!(found.tag_key, "cat");
        assert_eq!(found.count, 2);
    }

    #[test]
    fn has_any_tags_checks_existence_without_building_summaries() {
        let mut db = memory_db();
        assert!(!db.has_any_tags().unwrap());

        db.set_item_tags("c:/cat.jpg", ["cat"], source::EDIT)
            .unwrap();

        assert!(db.has_any_tags().unwrap());
    }

    #[test]
    fn first_write_rotates_tags_db_backup_once_for_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tags.db");
        let mut db = TagsDb::open_at(&path).unwrap();

        db.set_item_tags("c:/a.jpg", ["alpha"], source::EDIT)
            .unwrap();
        assert!(dir.path().join("tags.db.bak1").exists());
        assert!(!dir.path().join("tags.db.bak2").exists());

        db.set_item_tags("c:/b.jpg", ["beta"], source::EDIT)
            .unwrap();
        assert!(!dir.path().join("tags.db.bak2").exists());

        let bak = TagsDb::open_at(&dir.path().join("tags.db.bak1")).unwrap();
        assert!(bak.display_tags_for_item("c:/a.jpg").is_empty());
    }

    #[test]
    fn item_keys_by_tag_prefix_escapes_like_wildcards() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["a_b"], source::EDIT).unwrap();
        db.set_item_tags("c:/b.jpg", ["abc"], source::EDIT).unwrap();
        db.set_item_tags("c:/c.jpg", ["a%b"], source::EDIT).unwrap();
        assert_eq!(
            db.item_keys_by_tag_prefix("a_", 10),
            vec!["c:/a.jpg".to_string()]
        );
    }

    #[test]
    fn item_keys_by_tag_exact_does_not_match_prefix_siblings() {
        let mut db = memory_db();
        db.set_item_tags("c:/cat.jpg", ["cat"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/catnap.jpg", ["catnap"], source::EDIT)
            .unwrap();
        assert_eq!(
            db.item_keys_by_tag_exact("#cat", 10),
            vec!["c:/cat.jpg".to_string()]
        );
    }

    #[test]
    fn item_keys_by_tag_terms_and_intersects_exact_tags() {
        let mut db = memory_db();
        db.set_item_tags("c:/cat-dog.jpg", ["cat", "dog"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/cat-only.jpg", ["cat"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/dog-only.jpg", ["dog"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/cat-dognap.jpg", ["cat", "dognap"], source::EDIT)
            .unwrap();

        assert_eq!(
            db.item_keys_by_tag_terms_and(&["#cat".to_string(), "#dog".to_string()], 10),
            vec!["c:/cat-dog.jpg".to_string()]
        );
    }

    #[test]
    fn item_keys_by_tag_terms_and_uses_prefix_when_no_exact_tag_exists() {
        let mut db = memory_db();
        db.set_item_tags("c:/cat-dog.jpg", ["cat", "dog"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/cat-dognap.jpg", ["cat", "dognap"], source::EDIT)
            .unwrap();
        db.set_item_tags("c:/cat-only.jpg", ["cat"], source::EDIT)
            .unwrap();

        assert_eq!(
            db.item_keys_by_tag_terms_and(&["#cat".to_string(), "#do".to_string()], 10),
            vec![
                "c:/cat-dog.jpg".to_string(),
                "c:/cat-dognap.jpg".to_string()
            ]
        );
    }

    #[test]
    fn tag_view_timestamps_include_only_exact_and_prefix_matching_rows() {
        let db = memory_db();
        for (item, tag, at) in [
            ("a", "cat", 10),
            ("a", "dog", 30),
            ("a", "dognap", 999),
            ("a", "unrelated", 9000),
            ("b", "cat", 20),
            ("b", "dog", 25),
            ("b", "unrelated", 5000),
            ("c", "catnap", 100),
            ("c", "catneko", 150),
            ("c", "unrelated", 9000),
            ("d", "dog", 40),
        ] {
            db.conn.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, ?2, ?2, ?3)",
                params![item, tag, at],
            ).unwrap();
        }
        let rows = |rows: Vec<TagViewMatch>| {
            rows.into_iter()
                .map(|row| (row.item_key, row.applied_at))
                .collect::<Vec<_>>()
        };
        let desc = TagViewQueryOrder::AppliedAtDesc;
        assert_eq!(
            rows(db.tag_view_matches_exact("#cat", desc, 20)),
            [("b".into(), 20), ("a".into(), 10)]
        );
        assert_eq!(
            rows(db.tag_view_matches_terms_and(&["cat".into(), "dog".into()], desc, 20)),
            [("a".into(), 30), ("b".into(), 25)]
        );
        assert_eq!(
            rows(db.tag_view_matches_terms_and(&["cat".into(), "do".into()], desc, 20)),
            [("a".into(), 999), ("b".into(), 25)]
        );
        assert_eq!(
            rows(db.tag_view_matches_prefix("ca", desc, 20)),
            [("c".into(), 150), ("b".into(), 20), ("a".into(), 10)]
        );
        assert_eq!(
            rows(db.tag_view_matches_prefix("ca", TagViewQueryOrder::AppliedAtAsc, 1)),
            [("a".into(), 10)]
        );
    }

    #[test]
    fn tag_view_timestamp_order_precedes_sql_limit_and_ties_use_path() {
        let mut db = memory_db();
        let tx = db.conn.transaction().unwrap();
        {
            let mut insert = tx.prepare(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, 'cat', 'cat', ?2)",
            ).unwrap();
            for idx in 0..10_002 {
                insert
                    .execute(params![format!("item-{idx:05}"), idx / 2])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        let rows = db.tag_view_matches_exact("cat", TagViewQueryOrder::AppliedAtDesc, 2);
        assert_eq!(
            rows.iter().map(|r| r.item_key.as_str()).collect::<Vec<_>>(),
            ["item-10000", "item-10001"]
        );
        assert_eq!(rows[0].applied_at, 5000);
        let rows = db.tag_view_matches_exact("cat", TagViewQueryOrder::AppliedAtAsc, 1);
        assert_eq!(rows[0].item_key, "item-00000");
        assert!(
            db.tag_view_matches_exact("cat", TagViewQueryOrder::AppliedAtDesc, 0)
                .is_empty()
        );
    }

    #[test]
    fn tag_view_query_plans_preserve_candidates_and_key_scoped_timestamps() {
        let db = memory_db();
        let explain = |sql: &str, values: Vec<Value>| {
            db.conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(rusqlite::params_from_iter(values), |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let exact = |key: &str| TagQueryMatch {
            key: key.into(),
            kind: TagQueryMatchKind::Exact,
        };
        let prefix = |key: &str| TagQueryMatch {
            key: key.into(),
            kind: TagQueryMatchKind::Prefix,
        };
        for matches in [
            vec![exact("cat")],
            vec![prefix("ca")],
            vec![exact("cat"), exact("dog")],
            // Input order must not make the prefix the candidate root.
            vec![prefix("do"), exact("cat")],
            vec![prefix("ca"), prefix("do")],
        ] {
            let mut candidates = matches.clone();
            candidates.sort_by_key(|m| match m.kind {
                TagQueryMatchKind::Exact => 0,
                TagQueryMatchKind::Prefix => 1,
            });
            let (path_sql, path_values, path_alias) = if candidates.len() == 1 {
                let (sql, key) = match candidates[0].kind {
                    TagQueryMatchKind::Exact => (TAG_PATH_EXACT_SQL, candidates[0].key.clone()),
                    TagQueryMatchKind::Prefix => (
                        TAG_PATH_PREFIX_SQL,
                        format!("{}%", escape_like_pattern(&candidates[0].key)),
                    ),
                };
                (
                    sql.to_string(),
                    vec![Value::Text(key), Value::Integer(2)],
                    "item_tags",
                )
            } else {
                let (mut sql, mut values) = tag_candidate_sql(&candidates);
                sql.push_str(" ORDER BY base.item_key COLLATE NOCASE ASC LIMIT ?");
                values.push(Value::Integer(2));
                (sql, values, "base")
            };
            let path_plan = explain(&path_sql, path_values);
            let path_access = path_plan
                .iter()
                .find_map(|line| {
                    line.strip_prefix(&format!("SEARCH {path_alias} "))
                        .or_else(|| line.strip_prefix(&format!("SCAN {path_alias} ")))
                })
                .unwrap();
            let has_exact = candidates[0].kind == TagQueryMatchKind::Exact;
            if has_exact {
                assert!(path_access.contains("idx_item_tags_tagkey (tag_key=?)"));
            }
            for order in [
                TagViewQueryOrder::AppliedAtDesc,
                TagViewQueryOrder::AppliedAtAsc,
            ] {
                let (sql, values) = tag_time_order_sql(&matches, order, 2);
                assert_eq!(sql.matches("LIMIT").count(), 1, "{sql}");
                assert!(sql.find("ORDER BY").unwrap() < sql.find("LIMIT").unwrap());
                let plan = explain(&sql, values);
                let candidate_access = plan
                    .iter()
                    .find_map(|line| {
                        line.strip_prefix("SEARCH base ")
                            .or_else(|| line.strip_prefix("SCAN base "))
                    })
                    .unwrap();
                assert_eq!(candidate_access, path_access, "{matches:?}: {plan:?}");
                assert!(
                    plan.iter().any(|line| {
                        line.starts_with("SEARCH matched ") && line.contains("(item_key=?")
                    }),
                    "{matches:?}: {plan:?}"
                );
                assert!(!plan.iter().any(|line| line.starts_with("SCAN matched ")));
                if has_exact {
                    assert!(!plan.iter().any(|line| line.starts_with("SCAN base ")));
                }
            }
            let (sql, values) = tag_selected_timestamps_sql(&matches, &["a".into(), "b".into()]);
            let plan = explain(&sql, values);
            assert!(
                plan.iter().any(|line| {
                    line.starts_with("SEARCH matched ") && line.contains("(item_key=?")
                }),
                "{matches:?}: {plan:?}"
            );
        }
    }

    #[test]
    fn tag_view_path_order_limits_existing_candidates_before_timestamp_lookup() {
        let db = memory_db();
        for (item, tag, at) in [
            ("a", "cat", 10),
            ("a", "dog", 20),
            ("a", "unrelated", 999),
            ("b", "cat", 200),
            ("b", "dog", 300),
        ] {
            db.conn.execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, ?2, ?2, ?3)",
                params![item, tag, at],
            ).unwrap();
        }
        let terms = ["cat".into(), "do".into()];
        for (rows, keys, at) in [
            (
                db.tag_view_matches_exact("cat", TagViewQueryOrder::Path, 1),
                db.item_keys_by_tag_exact("cat", 1),
                10,
            ),
            (
                db.tag_view_matches_prefix("ca", TagViewQueryOrder::Path, 1),
                db.item_keys_by_tag_prefix("ca", 1),
                10,
            ),
            (
                db.tag_view_matches_terms_and(&terms, TagViewQueryOrder::Path, 1),
                db.item_keys_by_tag_terms_and(&terms, 1),
                20,
            ),
        ] {
            assert_eq!(
                rows.iter().map(|row| &row.item_key).collect::<Vec<_>>(),
                keys.iter().collect::<Vec<_>>()
            );
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].item_key, "a");
            assert_eq!(rows[0].applied_at, at);
        }
        assert_eq!(
            db.tag_view_matches_terms_and(&terms, TagViewQueryOrder::AppliedAtDesc, 1)[0].item_key,
            "b"
        );
    }

    #[test]
    fn tag_view_timestamp_prefix_escapes_like_wildcards() {
        let mut db = memory_db();
        db.set_item_tags("literal", ["a_b"], source::EDIT).unwrap();
        db.set_item_tags("wildcard", ["axb"], source::EDIT).unwrap();
        let rows = db.tag_view_matches_prefix("a_", TagViewQueryOrder::AppliedAtDesc, 10);
        assert_eq!(
            rows.iter().map(|r| r.item_key.as_str()).collect::<Vec<_>>(),
            ["literal"]
        );
    }

    #[test]
    fn prune_items_removes_tags_and_decided_state() {
        let mut db = memory_db();
        db.set_item_tags("c:/gone.jpg", ["cat"], source::EDIT)
            .unwrap();
        assert!(db.has_item_state("c:/gone.jpg"));
        let removed = db.prune_items(&["c:/gone.jpg".to_string()]).unwrap();
        assert_eq!(removed, 1);
        assert!(db.display_tags_for_item("c:/gone.jpg").is_empty());
        assert!(!db.has_item_state("c:/gone.jpg"));
    }

    #[test]
    fn retag_key_updates_display_when_key_is_unchanged() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["ＦＡＴＥ"], source::EDIT)
            .unwrap();

        let report = db.retag_key("fate", "Fate").unwrap();

        assert_eq!(report.old_key, "fate");
        assert_eq!(report.new_key, "fate");
        assert_eq!(report.affected_items, 1);
        assert_eq!(db.display_tags_for_item("c:/a.jpg"), vec!["#Fate"]);
    }

    #[test]
    fn retag_key_merges_into_existing_tag_without_duplicates() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["cat"], source::EDIT).unwrap();
        db.set_item_tags("c:/b.jpg", ["cat", "dog"], source::EDIT)
            .unwrap();

        let report = db.retag_key("cat", "dog").unwrap();

        assert_eq!(report.old_key, "cat");
        assert_eq!(report.new_key, "dog");
        assert_eq!(report.affected_items, 2);
        assert_eq!(report.removed_conflicts, 1);
        assert_eq!(db.display_tags_for_item("c:/a.jpg"), vec!["#dog"]);
        assert_eq!(db.display_tags_for_item("c:/b.jpg"), vec!["#dog"]);
        assert!(db.item_keys_by_tag_exact("cat", 10).is_empty());
    }
}
