//! 画像レーティング (★1〜★5) の永続管理。
//!
//! `%APPDATA%/mimageviewer/rating.db` に保存する。
//! 通常画像 / ZIP 内画像 / PDF ページに対して 0 (未評価) 〜 5 の星数を記録する。
//! キーは `App::page_path_key` が返す正規化キーを使う
//! (`adjustment_db::normalize_path` と同じ規則で統一)。

use std::path::PathBuf;

/// レーティング DB ハンドル。
pub struct RatingDb {
    conn: rusqlite::Connection,
}

impl RatingDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ratings (
                path TEXT PRIMARY KEY,
                stars INTEGER NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("rating.db")
    }

    /// 指定キーのレーティングを取得。未登録なら 0。
    pub fn get(&self, key: &str) -> u8 {
        let mut stmt = match self
            .conn
            .prepare_cached("SELECT stars FROM ratings WHERE path = ?1")
        {
            Ok(s) => s,
            Err(_) => return 0,
        };
        stmt.query_row([key], |row| {
            let v: i32 = row.get(0)?;
            Ok(v.clamp(0, 5) as u8)
        })
        .unwrap_or(0)
    }

    /// レーティングを設定する。0 の場合はレコードを削除する。
    pub fn set(&self, key: &str, stars: u8) -> Result<(), rusqlite::Error> {
        let stars = stars.min(5);
        if stars == 0 {
            self.conn
                .execute("DELETE FROM ratings WHERE path = ?1", [key])?;
        } else {
            self.conn.execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET stars = ?2",
                rusqlite::params![key, stars as i32],
            )?;
        }
        Ok(())
    }

    /// 全レコードを削除 (リセット)。
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM ratings", [])
    }

    /// 登録件数。
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM ratings", [], |row| row.get(0))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear() {
        // 一時 DB 用に in-memory を使う
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)",
        )
        .unwrap();
        let db = RatingDb { conn };

        assert_eq!(db.get("a.jpg"), 0);
        db.set("a.jpg", 3).unwrap();
        assert_eq!(db.get("a.jpg"), 3);
        db.set("a.jpg", 5).unwrap();
        assert_eq!(db.get("a.jpg"), 5);
        db.set("a.jpg", 0).unwrap();
        assert_eq!(db.get("a.jpg"), 0);
    }

    #[test]
    fn clamp_to_5() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)",
        )
        .unwrap();
        let db = RatingDb { conn };
        db.set("x", 99).unwrap();
        assert_eq!(db.get("x"), 5);
    }
}
