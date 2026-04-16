//! 見開き表示モードの永続管理。
//!
//! `%APPDATA%/mimageviewer/spread.db` にフォルダごとの見開きモードを保存する。
//! `rotation_db.rs` と同パターンの SQLite 永続化。

use std::path::{Path, PathBuf};

use crate::path_key;
use crate::settings::SpreadMode;

/// 見開き DB ハンドル
pub struct SpreadDb {
    conn: rusqlite::Connection,
}

impl SpreadDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS spreads (
                path TEXT PRIMARY KEY,
                mode INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        Ok(Self { conn })
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("spread.db")
    }

    /// フォルダの見開きモードを取得。未登録なら None。
    pub fn get(&self, path: &Path) -> Option<SpreadMode> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT mode FROM spreads WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let v: i32 = row.get(0)?;
            Ok(SpreadMode::from_int(v))
        })
        .ok()
    }

    /// 見開きモードを設定する。デフォルト値と同じ場合はレコードを削除する。
    pub fn set(
        &self,
        path: &Path,
        mode: SpreadMode,
        default: SpreadMode,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        if mode == default {
            self.conn
                .execute("DELETE FROM spreads WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO spreads (path, mode) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET mode = ?2",
                rusqlite::params![key, mode.to_int()],
            )?;
        }
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM spreads", [])
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM spreads", [], |row| row.get(0))
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
    fn spread_mode_roundtrip() {
        for mode in SpreadMode::all() {
            assert_eq!(SpreadMode::from_int(mode.to_int()), *mode);
        }
    }

    #[test]
    fn db_set_get_clear() {
        let db = SpreadDb::open().unwrap();
        let p = Path::new("C:/test/folder");
        let default = SpreadMode::Single;

        // 初期状態: 未登録
        assert!(db.get(p).is_none());

        // 設定
        db.set(p, SpreadMode::Ltr, default).unwrap();
        assert_eq!(db.get(p), Some(SpreadMode::Ltr));

        // 上書き
        db.set(p, SpreadMode::RtlCover, default).unwrap();
        assert_eq!(db.get(p), Some(SpreadMode::RtlCover));

        // デフォルト値で削除
        db.set(p, SpreadMode::Single, default).unwrap();
        assert!(db.get(p).is_none());
    }
}
