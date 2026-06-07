//! ユーザー画像スタンプの再利用履歴。
//!
//! スタンプとして実際に配置した画像は `StampSource::Embedded` として注釈ドキュメント
//! (`comic.db` / sidecar) 側へ埋め込まれる。この DB はそれとは別に、ピッカーで再利用
//! しやすくするための小さな履歴ライブラリを `%APPDATA%/mimageviewer/comic_user_stamps.db`
//! に保持する。履歴から再選択した場合も、配置先の注釈には改めて `Embedded` をコピーする。

use std::path::{Path, PathBuf};

use base64::Engine as _;
use comic_core::StampSource;
use sha2::{Digest, Sha256};

pub const USER_STAMP_HISTORY_CAP: usize = 48;
pub const USER_STAMP_PICKER_LIMIT: usize = 24;
pub const SCHEMA_VERSION: i64 = 1;

pub struct ComicUserStampDb {
    conn: rusqlite::Connection,
}

impl ComicUserStampDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: rusqlite::Connection) -> Result<Self, rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS user_stamps (
                id              TEXT    PRIMARY KEY,
                name            TEXT    NOT NULL,
                png             BLOB    NOT NULL,
                created_at_ms   INTEGER NOT NULL,
                last_used_at_ms INTEGER NOT NULL,
                use_count       INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_user_stamps_last_used
                ON user_stamps(last_used_at_ms DESC, created_at_ms DESC);",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("comic_user_stamps.db")
    }

    /// `StampSource::Embedded` を履歴へ保存する。絵文字や旧 `File` 参照は対象外。
    pub fn upsert_embedded(&self, source: &StampSource) -> Result<Option<String>, rusqlite::Error> {
        self.upsert_embedded_with_cap(source, USER_STAMP_HISTORY_CAP)
    }

    fn upsert_embedded_with_cap(
        &self,
        source: &StampSource,
        cap: usize,
    ) -> Result<Option<String>, rusqlite::Error> {
        let StampSource::Embedded { name, data } = source else {
            return Ok(None);
        };
        let Ok(png) = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()) else {
            return Ok(None);
        };
        let id = stable_png_id(&png);
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO user_stamps
                (id, name, png, created_at_ms, last_used_at_ms, use_count)
             VALUES (?1, ?2, ?3, ?4, ?4, 1)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                png = excluded.png,
                last_used_at_ms = excluded.last_used_at_ms,
                use_count = user_stamps.use_count + 1",
            rusqlite::params![id, name, png, now],
        )?;
        self.prune_to(cap)?;
        Ok(Some(id))
    }

    fn prune_to(&self, cap: usize) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM user_stamps
             WHERE id IN (
                SELECT id FROM user_stamps
                ORDER BY last_used_at_ms DESC, created_at_ms DESC, id ASC
                LIMIT -1 OFFSET ?1
             )",
            [cap as i64],
        )?;
        Ok(())
    }

    /// 最近使った順でユーザー画像スタンプを返す。返り値はそのまま注釈へ埋め込める
    /// `StampSource::Embedded`。
    pub fn list_recent(&self, limit: usize) -> Vec<StampSource> {
        if limit == 0 {
            return Vec::new();
        }
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT name, png
             FROM user_stamps
             ORDER BY last_used_at_ms DESC, created_at_ms DESC, id ASC
             LIMIT ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([limit as i64], |row| {
            let name: String = row.get(0)?;
            let png: Vec<u8> = row.get(1)?;
            let data = base64::engine::general_purpose::STANDARD.encode(png);
            Ok(StampSource::Embedded {
                name,
                data: std::sync::Arc::<str>::from(data),
            })
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }
}

fn stable_png_id(png: &[u8]) -> String {
    let digest = Sha256::digest(png);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> ComicUserStampDb {
        ComicUserStampDb::from_connection(rusqlite::Connection::open_in_memory().unwrap())
            .expect("open in memory")
    }

    fn embedded(name: &str, png: &[u8]) -> StampSource {
        StampSource::Embedded {
            name: name.to_string(),
            data: std::sync::Arc::<str>::from(
                base64::engine::general_purpose::STANDARD.encode(png),
            ),
        }
    }

    fn decode(source: &StampSource) -> Vec<u8> {
        let StampSource::Embedded { data, .. } = source else {
            panic!("expected embedded stamp");
        };
        base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .unwrap()
    }

    #[test]
    fn upsert_and_list_embedded_roundtrip() {
        let db = db();
        let src = embedded("stamp-a", b"png-a");

        assert!(db.upsert_embedded(&src).unwrap().is_some());
        let list = db.list_recent(10);

        assert_eq!(list.len(), 1);
        assert_eq!(crate::comic_stamp::stamp_label(&list[0]), "stamp-a");
        assert_eq!(decode(&list[0]), b"png-a");
    }

    #[test]
    fn non_embedded_sources_are_ignored() {
        let db = db();

        assert_eq!(
            db.upsert_embedded(&StampSource::Emoji("1f600".to_string()))
                .unwrap(),
            None
        );
        assert!(db.list_recent(10).is_empty());
    }

    #[test]
    fn duplicate_png_is_deduplicated_and_renamed() {
        let db = db();

        let id1 = db
            .upsert_embedded(&embedded("old", b"same-png"))
            .unwrap()
            .unwrap();
        let id2 = db
            .upsert_embedded(&embedded("new", b"same-png"))
            .unwrap()
            .unwrap();

        assert_eq!(id1, id2);
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM user_stamps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            crate::comic_stamp::stamp_label(&db.list_recent(10)[0]),
            "new"
        );
    }

    #[test]
    fn cap_prunes_old_entries() {
        let db = db();

        db.upsert_embedded_with_cap(&embedded("a", b"a"), 2)
            .unwrap();
        db.upsert_embedded_with_cap(&embedded("b", b"b"), 2)
            .unwrap();
        db.upsert_embedded_with_cap(&embedded("c", b"c"), 2)
            .unwrap();

        let list = db.list_recent(10);
        assert_eq!(list.len(), 2);
    }
}
