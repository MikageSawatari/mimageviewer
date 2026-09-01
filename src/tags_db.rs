//! mIV タグの中央カタログ DB。
//!
//! `%APPDATA%/mimageviewer/tags.db` を正本にし、タグ名は保存時に `#` を
//! 持たない。UI 表示だけ `#` を付ける。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetagReport {
    pub old_key: String,
    pub new_key: String,
    pub affected_items: usize,
    pub removed_conflicts: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyImportReport {
    pub skipped_already_imported: bool,
    pub scanned_docs: usize,
    pub imported_items: usize,
    pub inserted_tags: usize,
    pub skipped_decided_items: usize,
}

pub const LEGACY_TANTIVY_IMPORTED_META: &str = "legacy_tantivy_imported";

/// `tag_item_state.source` に入れる値。
pub mod source {
    pub const EDIT: &str = "edit";
    pub const TANTIVY_MIGRATION: &str = "tantivy_migration";
    pub const XMP_LEGACY: &str = "xmp_legacy";
    pub const SIDECAR: &str = "sidecar";
    pub const METADATA_IMPORT: &str = "metadata_import";
}

pub struct TagsDb {
    conn: Connection,
    path: PathBuf,
}

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

    /// §7.2 legacy seed 専用の原子的 union。`Ok(None)` = 決定済みで何もしなかった。
    ///
    /// 旧 XMP の読み取り (遅いファイル I/O) と DB 反映の間に通常編集
    /// (tag_write_worker、別コネクション) が割り込むと、読み取り時点の状態に基づく
    /// 全置換 (`set_item_tags`) は編集を巻き戻す (削除タグの復活 = §7.2 が禁じる事象)。
    /// よって seed は **同一トランザクション内で `tag_item_state` を再チェック**し、
    /// 決定済みなら何もしない。反映自体も INSERT OR IGNORE の union で、既存行を
    /// 削除しない。
    pub fn seed_legacy_tags_if_undecided<I, S>(
        &mut self,
        item_key: &str,
        tags: I,
    ) -> Result<Option<(Vec<String>, usize)>, rusqlite::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.rotate_backups_once();
        let now = now_unix_secs();
        let normalized = collapse_tags(tags, now);
        let tx = self.conn.transaction()?;
        let decided = tx
            .query_row(
                "SELECT 1 FROM tag_item_state WHERE item_key = ?1 LIMIT 1",
                [item_key],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if decided {
            return Ok(None); // tx は drop で rollback (変更なし)
        }
        let mut inserted = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO item_tags (item_key, tag, tag_key, applied_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for tag in &normalized {
                inserted +=
                    stmt.execute(params![item_key, tag.tag, tag.tag_key, tag.applied_at])?;
            }
        }
        upsert_item_state_tx(&tx, item_key, source::XMP_LEGACY, now)?;
        tx.commit()?;
        Ok(Some((self.display_tags_for_item(item_key), inserted)))
    }

    pub fn toggle_item_tag(
        &mut self,
        item_key: &str,
        tag_name: &str,
    ) -> Result<(TagToggleOutcome, Vec<String>, Vec<String>), rusqlite::Error> {
        self.rotate_backups_once();
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

    /// 指定キーのうち `tag_item_state` 行が存在するものを返す (500 件ずつの IN クエリ)。
    /// legacy seed worker の事前フィルタ用 — フォルダ再訪時に per-file の点クエリ
    /// (`has_item_state` × N) を発行しないための bulk 版。
    pub fn keys_with_item_state(&self, item_keys: &[String]) -> HashSet<String> {
        let mut out = HashSet::new();
        for chunk in item_keys.chunks(500) {
            let placeholders = (0..chunk.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT item_key FROM tag_item_state WHERE item_key IN ({placeholders})");
            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
            if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(0)
            }) {
                out.extend(rows.flatten());
            }
        }
        out
    }

    pub fn has_item_state(&self, item_key: &str) -> bool {
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
        let now = now_unix_secs();
        let tx = self.conn.transaction()?;
        upsert_item_state_tx(&tx, item_key, source, now)?;
        tx.commit()
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM tag_meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.rotate_backups_once();
        self.conn.execute(
            "INSERT INTO tag_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn sidecar_sync_get(&self, folder_key: &str) -> Option<i64> {
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
        self.conn.execute(
            "INSERT INTO tag_sidecar_sync (folder_key, sidecar_mtime) VALUES (?1, ?2)
             ON CONFLICT(folder_key) DO UPDATE SET sidecar_mtime = excluded.sidecar_mtime",
            params![folder_key, sidecar_mtime],
        )?;
        Ok(())
    }

    pub fn sidecar_sync_clear(&self, folder_key: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute(
                "DELETE FROM tag_sidecar_sync WHERE folder_key = ?1",
                [folder_key],
            )
            .map(|_| ())
    }

    pub fn import_legacy_tantivy_tags<I, K, T>(
        &mut self,
        docs: I,
    ) -> Result<LegacyImportReport, rusqlite::Error>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str>,
        T: AsRef<str>,
    {
        if self.meta(LEGACY_TANTIVY_IMPORTED_META).as_deref() == Some("1") {
            return Ok(LegacyImportReport {
                skipped_already_imported: true,
                ..LegacyImportReport::default()
            });
        }

        self.rotate_backups_once();
        let now = now_unix_secs();
        let tx = self.conn.transaction()?;
        let mut report = LegacyImportReport::default();
        for (item_key, tags_column) in docs {
            report.scanned_docs += 1;
            let item_key = item_key.as_ref().trim();
            if item_key.is_empty() {
                continue;
            }
            let already_decided = tx
                .query_row(
                    "SELECT 1 FROM tag_item_state WHERE item_key = ?1 LIMIT 1",
                    [item_key],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if already_decided {
                report.skipped_decided_items += 1;
                continue;
            }

            let legacy_tags = crate::ingest_text::parse_tags_column(tags_column.as_ref())
                .into_iter()
                .filter(|tag| tag.trim_start().starts_with('#'));
            let normalized = collapse_tags(legacy_tags, now);
            if normalized.is_empty() {
                continue;
            }
            {
                let mut stmt = tx.prepare(
                    "INSERT OR IGNORE INTO item_tags (item_key, tag, tag_key, applied_at)
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                for tag in &normalized {
                    let inserted =
                        stmt.execute(params![item_key, tag.tag, tag.tag_key, tag.applied_at])?;
                    report.inserted_tags += inserted;
                }
            }
            upsert_item_state_tx(&tx, item_key, source::TANTIVY_MIGRATION, now)?;
            report.imported_items += 1;
        }
        tx.execute(
            "INSERT INTO tag_meta (key, value) VALUES (?1, '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [LEGACY_TANTIVY_IMPORTED_META],
        )?;
        tx.commit()?;
        Ok(report)
    }

    pub fn tag_summaries(&self) -> Vec<TagSummary> {
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
        self.conn
            .query_row("SELECT 1 FROM item_tags LIMIT 1", [], |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
    }

    pub fn find_by_prefix(&self, prefix: &str, limit: usize) -> Vec<TagSummary> {
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
        let key = normalize_tag_key(prefix);
        if key.is_empty() || limit == 0 {
            return Vec::new();
        }
        let escaped = escape_like_pattern(&key);
        let pattern = format!("{escaped}%");
        let mut stmt = match self.conn.prepare(
            "SELECT DISTINCT item_key
             FROM item_tags
             WHERE tag_key LIKE ?1 ESCAPE '\\'
             ORDER BY item_key COLLATE NOCASE ASC
             LIMIT ?2",
        ) {
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
        let key = normalize_tag_key(tag);
        if key.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut stmt = match self.conn.prepare(
            "SELECT DISTINCT item_key
             FROM item_tags
             WHERE tag_key = ?1
             ORDER BY item_key COLLATE NOCASE ASC
             LIMIT ?2",
        ) {
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

    pub fn prune_items(&mut self, item_keys: &[String]) -> Result<usize, rusqlite::Error> {
        if item_keys.is_empty() {
            return Ok(0);
        }
        self.rotate_backups_once();
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

/// SQL LIKE エスケープは [`crate::adjustment_db::escape_like_pattern`] を共有する
/// (独自コピーを持つと将来のエスケープ修正が片方にしか入らない)。
pub use crate::adjustment_db::escape_like_pattern;

/// dc:subject 値のうち「mIV v1.0 が付けた legacy タグ」(= `#` 始まり) だけを抽出し、
/// 表示形 (`#` 付き) で返す。
///
/// 自動 seed (`tag_legacy_seed_worker`) と明示インポート (`tag_legacy_xmp_worker`) の
/// **両方がこの 1 実装を使うこと**。判定規則が分かれると、同じファイルから
/// 取り込まれるタグ集合が経路によって変わる再現困難な不整合になる。
pub fn miv_legacy_tags(subjects: Vec<String>) -> Vec<String> {
    collapse_tags(
        subjects
            .into_iter()
            .filter(|tag| tag.trim_start().starts_with('#')),
        0,
    )
    .into_iter()
    .map(|tag| format_display_tag(&tag.tag))
    .collect()
}

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

    /// 自動 seed と明示インポートが共有する legacy タグ判定の正本テスト
    /// (旧: 両 worker に同一テストが複製されていた)。
    #[test]
    fn miv_legacy_tags_filters_hash_tags_only() {
        let tags = miv_legacy_tags(vec![
            "#Cat".to_string(),
            "external".to_string(),
            " #ＤＯＧ ".to_string(),
            "#".to_string(),
        ]);
        assert_eq!(tags, vec!["#Cat".to_string(), "#DOG".to_string()]);
    }

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

    #[test]
    fn legacy_tantivy_import_copies_only_hash_tags_once() {
        let mut db = memory_db();
        let report = db
            .import_legacy_tantivy_tags([
                ("c:/a.jpg", "#原神 external #風景"),
                ("c:/b.jpg", "external"),
            ])
            .unwrap();
        assert_eq!(report.scanned_docs, 2);
        assert_eq!(report.imported_items, 1);
        assert_eq!(report.inserted_tags, 2);
        assert_eq!(report.skipped_decided_items, 0);
        assert_eq!(db.meta(LEGACY_TANTIVY_IMPORTED_META).as_deref(), Some("1"));

        let tags = db.display_tags_for_item("c:/a.jpg");
        assert!(tags.contains(&"#原神".to_string()));
        assert!(tags.contains(&"#風景".to_string()));
        assert!(db.display_tags_for_item("c:/b.jpg").is_empty());
        assert!(db.has_item_state("c:/a.jpg"));

        let skipped = db
            .import_legacy_tantivy_tags([("c:/c.jpg", "#未実行")])
            .unwrap();
        assert!(skipped.skipped_already_imported);
        assert!(db.display_tags_for_item("c:/c.jpg").is_empty());
    }

    #[test]
    fn legacy_tantivy_import_skips_decided_items() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["既存"], source::EDIT)
            .unwrap();
        let report = db
            .import_legacy_tantivy_tags([("c:/a.jpg", "#旧タグ")])
            .unwrap();
        assert_eq!(report.scanned_docs, 1);
        assert_eq!(report.imported_items, 0);
        assert_eq!(report.inserted_tags, 0);
        assert_eq!(report.skipped_decided_items, 1);
        assert_eq!(db.display_tags_for_item("c:/a.jpg"), vec!["#既存"]);
    }
}
