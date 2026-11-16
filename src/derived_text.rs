//! 索引に入れる派生テキスト。ストア一覧・読取・反映待ちのSQLを共用する。
use std::path::{Path, PathBuf};

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::rename_key_migration::{STORES, StoreDescriptor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedTextKind {
    Caption,
    Transcript,
}

pub(crate) fn stores() -> impl Iterator<Item = &'static StoreDescriptor> {
    STORES.iter().filter(|store| store.derived_text.is_some())
}

impl DerivedTextKind {
    pub(crate) fn descriptor(self) -> &'static StoreDescriptor {
        stores()
            .find(|store| store.derived_text == Some(self))
            .expect("derived text store")
    }

    pub(crate) fn source(self) -> crate::fts_index::SourceKind {
        match self {
            Self::Caption => crate::fts_index::SourceKind::AiCaption,
            Self::Transcript => crate::fts_index::SourceKind::Transcript,
        }
    }

    fn removed_table(self) -> &'static str {
        match self {
            Self::Caption => "caption_removed",
            Self::Transcript => "transcript_removed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DerivedTextPaths(Vec<(DerivedTextKind, PathBuf)>);

impl DerivedTextPaths {
    pub fn at(data_dir: &Path) -> Self {
        Self(
            stores()
                .map(|store| (store.derived_text.unwrap(), data_dir.join(store.file)))
                .collect(),
        )
    }

    /// 既存の説明文テストの任意のDB名も、同じストア一覧で扱う。
    pub fn with_caption_path(path: PathBuf) -> Self {
        let mut paths = Self::at(path.parent().unwrap_or_else(|| Path::new(".")));
        paths
            .0
            .iter_mut()
            .find(|(kind, _)| *kind == DerivedTextKind::Caption)
            .unwrap()
            .1 = path;
        paths
    }

    pub fn path(&self, kind: DerivedTextKind) -> &Path {
        &self
            .0
            .iter()
            .find(|(candidate, _)| *candidate == kind)
            .expect("derived text path")
            .1
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (DerivedTextKind, &Path)> {
        self.0.iter().map(|(kind, path)| (*kind, path.as_path()))
    }
}

pub(crate) enum Reader {
    Caption(crate::ai_caption_db::CaptionDb),
    Transcript(crate::transcript_db::TranscriptDb),
}

impl Reader {
    pub(crate) fn open(kind: DerivedTextKind, path: &Path) -> rusqlite::Result<Option<Self>> {
        match kind {
            DerivedTextKind::Caption => crate::ai_caption_db::CaptionDb::open_read_only(path)
                .map(|db| db.map(Self::Caption)),
            DerivedTextKind::Transcript => crate::transcript_db::TranscriptDb::open_read_only(path)
                .map(|db| db.map(Self::Transcript)),
        }
    }

    pub(crate) fn text(&self, key: &str) -> rusqlite::Result<String> {
        match self {
            Self::Caption(db) => db.get(key).map(|row| {
                row.as_ref()
                    .map(crate::ingest_text::caption_search_text)
                    .unwrap_or_default()
            }),
            Self::Transcript(db) => db.get(key).map(|row| {
                row.as_ref()
                    .map(crate::ingest_text::transcript_search_text)
                    .unwrap_or_default()
            }),
        }
    }
}

/// schemaの作成・移行は各ストアの持ち主だけが行う。索引は既存DBだけを開く。
pub(crate) struct PendingStore {
    conn: Connection,
    kind: DerivedTextKind,
}

impl PendingStore {
    pub(crate) fn open(kind: DerivedTextKind, path: &Path) -> rusqlite::Result<Self> {
        let conn = match kind {
            DerivedTextKind::Caption => {
                crate::ai_caption_db::CaptionDb::open_existing_for_write(path)?.into_connection()
            }
            DerivedTextKind::Transcript => {
                crate::transcript_db::TranscriptDb::open_existing_for_write(path)?.into_connection()
            }
        };
        Ok(Self { conn, kind })
    }

    pub(crate) fn pending_in_ranges(
        &self,
        ranges: &[crate::metadata_ownership::PathRange],
        limit: usize,
    ) -> rusqlite::Result<Vec<(String, i64, bool)>> {
        pending_in_ranges(&self.conn, self.kind, ranges, limit)
    }

    pub(crate) fn mark_reflected(&self, changes: &[(String, i64, bool)]) -> rusqlite::Result<()> {
        mark_reflected(&self.conn, self.kind, changes)
    }

    pub(crate) fn cleanup_removed(
        &self,
        ranges: &[crate::metadata_ownership::PathRange],
    ) -> rusqlite::Result<usize> {
        cleanup_removed(&self.conn, self.kind, ranges)
    }
}

pub(crate) fn pending_in_ranges(
    conn: &Connection,
    kind: DerivedTextKind,
    ranges: &[crate::metadata_ownership::PathRange],
    limit: usize,
) -> rusqlite::Result<Vec<(String, i64, bool)>> {
    let mut pending = Vec::new();
    let sql = format!(
        "SELECT item_key, rev, removed FROM (
        SELECT item_key, rev, 0 AS removed FROM {} WHERE indexed=0
        UNION ALL SELECT item_key, rev, 1 AS removed FROM {}
        ) WHERE item_key >= ?1 COLLATE BINARY AND item_key < ?2 COLLATE BINARY
        ORDER BY item_key, rev, removed LIMIT ?3",
        kind.descriptor().table,
        kind.removed_table()
    );
    let mut statement = conn.prepare(&sql)?;
    for range in ranges {
        let remaining = limit.saturating_sub(pending.len());
        if remaining == 0 {
            break;
        }
        let rows = statement.query_map(
            params![
                range.start,
                range.end,
                i64::try_from(remaining).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        for row in rows {
            pending.push(row?);
        }
    }
    Ok(pending)
}

pub(crate) fn mark_reflected(
    conn: &Connection,
    kind: DerivedTextKind,
    changes: &[(String, i64, bool)],
) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    {
        let mut indexed = tx.prepare(&format!(
            "UPDATE {} SET indexed=1 WHERE item_key=?1 AND rev=?2",
            kind.descriptor().table
        ))?;
        let mut removed = tx.prepare(&format!(
            "DELETE FROM {} WHERE item_key=?1 AND rev=?2",
            kind.removed_table()
        ))?;
        for (key, rev, is_removed) in changes {
            if *is_removed {
                removed.execute(params![key, rev])?;
            } else {
                indexed.execute(params![key, rev])?;
            }
        }
    }
    tx.commit()
}

pub(crate) fn cleanup_removed(
    conn: &Connection,
    kind: DerivedTextKind,
    ranges: &[crate::metadata_ownership::PathRange],
) -> rusqlite::Result<usize> {
    if ranges.is_empty() {
        return conn.execute(&format!("DELETE FROM {}", kind.removed_table()), []);
    }
    let values = std::iter::repeat_n("(?, ?)", ranges.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("WITH roots(start, end) AS (VALUES {values}) DELETE FROM {} WHERE NOT EXISTS (
        SELECT 1 FROM roots WHERE item_key >= start COLLATE BINARY AND item_key < end COLLATE BINARY)", kind.removed_table());
    conn.execute(
        &sql,
        rusqlite::params_from_iter(ranges.iter().flat_map(|range| [&range.start, &range.end])),
    )
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
