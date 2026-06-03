//! 補正レイヤーの最小永続管理。
//!
//! `%APPDATA%/mimageviewer/local_adjust.db` に、ページ単位の
//! `Vec<local_adjust_core::LocalAdjustmentLayer>` を JSON として保存する。
//! 初期統合では中央 DB を authoritative にし、サイドカーバックアップは後続で扱う。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use local_adjust_core::LocalAdjustmentLayer;

/// 補正レイヤー DB ハンドル。
pub struct LocalAdjustDb {
    conn: rusqlite::Connection,
}

impl LocalAdjustDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS local_adjust_pages (
                page_path   TEXT PRIMARY KEY,
                layers_json TEXT NOT NULL,
                updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("local_adjust.db")
    }

    /// ページの補正レイヤー配列を取得する。未登録または JSON 破損なら None。
    pub fn get_layers(&self, page_key: &str) -> Option<Vec<LocalAdjustmentLayer>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT layers_json FROM local_adjust_pages WHERE page_path = ?1")
            .ok()?;
        let json: String = stmt.query_row([page_key], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// ページの補正レイヤー配列を書き込む。空配列は削除として扱う。
    pub fn set_layers(
        &self,
        page_key: &str,
        layers: &[LocalAdjustmentLayer],
    ) -> Result<(), rusqlite::Error> {
        if layers.is_empty() {
            return self.remove_layers(page_key);
        }
        let json = serde_json::to_string(layers).unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            "INSERT INTO local_adjust_pages (page_path, layers_json, updated_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                layers_json = ?2,
                updated_at = unixepoch()",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    /// ページの補正レイヤーを削除する。
    pub fn remove_layers(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM local_adjust_pages WHERE page_path = ?1",
            [page_key],
        )?;
        Ok(())
    }

    /// 指定プレフィックスで始まるページキー集合を返す。
    ///
    /// フォルダロード時の hydrate や、サムネイル上のバッジ判定に使う。
    pub fn load_layer_keys(&self, prefix: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path FROM local_adjust_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return set;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for row in rows.flatten() {
            set.insert(row);
        }
        set
    }

    /// 指定プレフィックスで始まるページの補正レイヤー配列を一括取得する。
    pub fn load_layers_by_prefix(
        &self,
        prefix: &str,
    ) -> HashMap<String, Vec<LocalAdjustmentLayer>> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, layers_json FROM local_adjust_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return map;
        };
        for (key, json) in rows.flatten() {
            if let Ok(layers) = serde_json::from_str::<Vec<LocalAdjustmentLayer>>(&json) {
                if !layers.is_empty() {
                    map.insert(key, layers);
                }
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_adjust_core::{LocalEffect, LocalMask};

    fn open_temp_db() -> (tempfile::TempDir, LocalAdjustDb) {
        let temp = tempfile::tempdir().unwrap();
        let db = LocalAdjustDb::open_at(&temp.path().join("local_adjust.db")).unwrap();
        (temp, db)
    }

    fn sample_layer(name: &str) -> LocalAdjustmentLayer {
        LocalAdjustmentLayer::new(name, LocalMask::Full, LocalEffect::None)
    }

    #[test]
    fn round_trips_page_layers() {
        let (_temp, db) = open_temp_db();
        let key = "c:/imgs/a.png";
        let layers = vec![sample_layer("base"), sample_layer("finish")];

        db.set_layers(key, &layers).unwrap();

        assert_eq!(db.get_layers(key), Some(layers));
    }

    #[test]
    fn empty_layers_remove_page_entry() {
        let (_temp, db) = open_temp_db();
        let key = "c:/imgs/a.png";
        db.set_layers(key, &[sample_layer("base")]).unwrap();

        db.set_layers(key, &[]).unwrap();

        assert_eq!(db.get_layers(key), None);
    }

    #[test]
    fn load_layer_keys_filters_by_escaped_prefix() {
        let (_temp, db) = open_temp_db();
        db.set_layers("c:/imgs/100%/a.png", &[sample_layer("a")])
            .unwrap();
        db.set_layers("c:/imgs/100x/b.png", &[sample_layer("b")])
            .unwrap();

        let keys = db.load_layer_keys("c:/imgs/100%/");

        assert!(keys.contains("c:/imgs/100%/a.png"));
        assert!(!keys.contains("c:/imgs/100x/b.png"));
    }

    #[test]
    fn load_layers_by_prefix_returns_non_empty_layers() {
        let (_temp, db) = open_temp_db();
        let layers = vec![sample_layer("a")];
        db.set_layers("c:/imgs/a.png", &layers).unwrap();
        db.set_layers("c:/other/b.png", &[sample_layer("b")])
            .unwrap();

        let map = db.load_layers_by_prefix("c:/imgs/");

        assert_eq!(map.get("c:/imgs/a.png"), Some(&layers));
        assert!(!map.contains_key("c:/other/b.png"));
    }
}
