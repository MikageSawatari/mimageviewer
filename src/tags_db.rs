//! mIV タグの中央カタログ DB。
//!
//! `%APPDATA%/mimageviewer/tags.db` を正本にし、タグ名は保存時に `#` を
//! 持たない。UI 表示だけ `#` を付ける。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
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
}

pub struct TagsDb {
    conn: Connection,
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
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("tags.db")
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
             );",
        )
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

    pub fn toggle_item_tag(
        &mut self,
        item_key: &str,
        tag_name: &str,
    ) -> Result<(TagToggleOutcome, Vec<String>, Vec<String>), rusqlite::Error> {
        let now = now_unix_secs();
        let tag = normalize_tag_display_name(tag_name);
        let tag_key = normalize_tag_key(&tag);
        let before = self.display_tags_for_item(item_key);
        if tag_key.is_empty() {
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
        let now = now_unix_secs();
        let before = self.display_tags_for_item(item_key);
        let tx = self.conn.transaction()?;
        let changed = tx.execute("DELETE FROM item_tags WHERE item_key = ?1", [item_key])? > 0;
        upsert_item_state_tx(&tx, item_key, source::EDIT, now)?;
        tx.commit()?;
        Ok((changed, before, Vec::new()))
    }

    pub fn display_tags_for_item(&self, item_key: &str) -> Vec<String> {
        self.get_item_tags(item_key)
            .into_iter()
            .map(|t| format_display_tag(&t.tag))
            .collect()
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
        self.conn.execute(
            "INSERT INTO tag_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
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

    pub fn prune_items(&mut self, item_keys: &[String]) -> Result<usize, rusqlite::Error> {
        if item_keys.is_empty() {
            return Ok(0);
        }
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

pub fn format_display_tag(tag: &str) -> String {
    let clean = normalize_tag_display_name(tag);
    if clean.is_empty() {
        String::new()
    } else {
        format!("#{clean}")
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
        if tag_key.is_empty() || display.chars().count() > 64 {
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

pub fn escape_like_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
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
        TagsDb::init_schema(&conn).unwrap();
        TagsDb { conn }
    }

    #[test]
    fn normalize_strips_hash_and_nfkc_lowercases_key() {
        assert_eq!(normalize_tag_display_name("  ##ＦＡＴＥ  "), "FATE");
        assert_eq!(normalize_tag_key("  #ＦＡＴＥ  "), "fate");
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
    fn prefix_search_escapes_like_wildcards() {
        let mut db = memory_db();
        db.set_item_tags("c:/a.jpg", ["a_b", "a%b", "abc"], source::EDIT)
            .unwrap();
        let found = db.find_by_prefix("a_", 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tag, "a_b");
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
