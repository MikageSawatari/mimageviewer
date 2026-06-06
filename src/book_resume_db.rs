//! 本 (フォルダ / ZIP / PDF) ごとの「最後に読んだページ」永続管理。
//!
//! `%APPDATA%/mimageviewer/book_resume.db` にコンテナパスごとの最後に表示した
//! ページ index を保存する。`spread_db.rs` / `rotation_db.rs` と同パターンの
//! SQLite 永続化で、アプリ再起動を跨いで読書位置を復元する (動画の
//! `video_resume_positions` の画像本版)。
//!
//! 値は `items` 内の index。ZIP/PDF は列挙順が決定的なので index が安定する。
//! 通常フォルダはファイル追加削除で多少ずれるが、その場合は復元時に範囲・種別を
//! 検証して妥当でなければ先頭にフォールバックする (呼び出し側の責務)。

use std::path::{Path, PathBuf};

use crate::path_key;

/// 読書位置 DB ハンドル
pub struct BookResumeDb {
    conn: rusqlite::Connection,
}

impl BookResumeDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS book_resume (
                path TEXT PRIMARY KEY,
                page INTEGER NOT NULL DEFAULT 0
            )",
        )?;
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
}

fn normalize_path(path: &Path) -> String {
    path_key::normalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_resume_set_get_remove() {
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
