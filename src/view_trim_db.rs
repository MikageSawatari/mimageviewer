//! 表示トリムの永続管理。
//!
//! `%APPDATA%/mimageviewer/view_trim.db` に、本ごとの適用モード / 本全体設定と、
//! ページごとの個別トリム設定を保存する。表示専用の設定であり、export crop や
//! 補正 / AI パイプラインの出力には影響しない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::view_trim::{ViewTrimBookState, ViewTrimPageOverride};

pub struct ViewTrimDb {
    conn: rusqlite::Connection,
}

impl ViewTrimDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS view_trim_books (
                book_key      TEXT PRIMARY KEY,
                state_json    TEXT NOT NULL,
                updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS view_trim_pages (
                page_path     TEXT PRIMARY KEY,
                override_json TEXT NOT NULL,
                updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("view_trim.db")
    }

    pub fn get_book_state(&self, book_path: &Path) -> Option<ViewTrimBookState> {
        let key = book_key(book_path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT state_json FROM view_trim_books WHERE book_key = ?1")
            .ok()?;
        let json: String = stmt.query_row([&key], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub fn set_book_state(
        &self,
        book_path: &Path,
        state: ViewTrimBookState,
    ) -> Result<(), rusqlite::Error> {
        let key = book_key(book_path);
        if state.is_removable() {
            self.conn
                .execute("DELETE FROM view_trim_books WHERE book_key = ?1", [&key])?;
            return Ok(());
        }
        let json = serde_json::to_string(&state).unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "INSERT INTO view_trim_books (book_key, state_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(book_key) DO UPDATE SET
                state_json = ?2,
                updated_at = unixepoch()",
            rusqlite::params![key, json],
        )?;
        Ok(())
    }

    pub fn set_page_override(
        &self,
        page_key: &str,
        page_override: ViewTrimPageOverride,
    ) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(&page_override).unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "INSERT INTO view_trim_pages (page_path, override_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                override_json = ?2,
                updated_at = unixepoch()",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    pub fn remove_page_override(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM view_trim_pages WHERE page_path = ?1",
            [page_key],
        )?;
        Ok(())
    }

    pub fn load_page_overrides_by_prefix(
        &self,
        prefix: &str,
    ) -> HashMap<String, ViewTrimPageOverride> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, override_json FROM view_trim_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let pattern = format!("{}%", crate::adjustment_db::escape_like_pattern(prefix));
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return map;
        };
        for (key, json) in rows.flatten() {
            if let Ok(page_override) = serde_json::from_str::<ViewTrimPageOverride>(&json) {
                map.insert(key, page_override);
            }
        }
        map
    }

    /// 複数フォルダを横断する一覧向けに、指定キーだけを一括読込する。
    pub fn load_page_overrides_many(
        &self,
        page_keys: &[&str],
    ) -> HashMap<String, ViewTrimPageOverride> {
        let mut map = HashMap::new();
        for chunk in page_keys.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT page_path, override_json FROM view_trim_pages
                 WHERE page_path IN ({placeholders})"
            );
            let Ok(mut stmt) = self.conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
            else {
                continue;
            };
            for (key, json) in rows.flatten() {
                if let Ok(page_override) = serde_json::from_str::<ViewTrimPageOverride>(&json) {
                    map.insert(key, page_override);
                }
            }
        }
        map
    }
}

fn book_key(path: &Path) -> String {
    crate::path_key::normalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_trim::{
        ViewTrimApplyMode, ViewTrimBookSettings, ViewTrimBookState, ViewTrimMargins,
        ViewTrimPageOverride,
    };

    fn open_temp_db() -> (tempfile::TempDir, ViewTrimDb) {
        let temp = tempfile::tempdir().unwrap();
        let db = ViewTrimDb::open_at(&temp.path().join("view_trim.db")).unwrap();
        (temp, db)
    }

    #[test]
    fn book_state_roundtrip_and_removal() {
        let (_temp, db) = open_temp_db();
        let path = Path::new(r"C:\Books\Vol1");
        assert!(db.get_book_state(path).is_none());

        let state = ViewTrimBookState {
            apply_mode: ViewTrimApplyMode::Book,
            book_settings: ViewTrimBookSettings {
                enabled: true,
                single: ViewTrimMargins {
                    left: 0.05,
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        db.set_book_state(path, state).unwrap();
        assert_eq!(db.get_book_state(path), Some(state));

        db.set_book_state(path, ViewTrimBookState::default())
            .unwrap();
        assert!(db.get_book_state(path).is_none());
    }

    #[test]
    fn page_overrides_load_by_escaped_prefix() {
        let (_temp, db) = open_temp_db();
        let keep_key = "c:/imgs/a_[one].png";
        let skip_key = "c:/imgs/a_xone].png";
        let page_override = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            top: 0.04,
            ..Default::default()
        });
        db.set_page_override(keep_key, page_override).unwrap();
        db.set_page_override(
            skip_key,
            ViewTrimPageOverride::from_margins(ViewTrimMargins {
                bottom: 0.07,
                ..Default::default()
            }),
        )
        .unwrap();

        let got = db.load_page_overrides_by_prefix("c:/imgs/a_[");
        assert_eq!(got.len(), 1);
        assert_eq!(got.get(keep_key), Some(&page_override));

        db.remove_page_override(keep_key).unwrap();
        assert!(db.load_page_overrides_by_prefix("c:/imgs/a_[").is_empty());
    }

    #[test]
    fn page_overrides_load_many_returns_only_requested_exact_keys() {
        let (_temp, db) = open_temp_db();
        let page_override = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.03,
            ..Default::default()
        });
        db.set_page_override("c:/a.jpg", page_override).unwrap();
        db.set_page_override("c:/b.jpg", page_override).unwrap();
        let loaded = db.load_page_overrides_many(&["c:/b.jpg", "c:/missing.jpg"]);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("c:/b.jpg"), Some(&page_override));
    }
}
