//! 画像補正プリセットの永続管理。
//!
//! `%APPDATA%/mimageviewer/adjustment.db` に保存する。
//! フォルダ/ZIP/PDF 単位の 4 プリセット + ページごとのプリセット割り当て。

use std::path::{Path, PathBuf};

use crate::adjustment::AdjustPresets;

/// 補正設定 DB ハンドル。
pub struct AdjustmentDb {
    conn: rusqlite::Connection,
}

impl AdjustmentDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS presets (
                container_path TEXT PRIMARY KEY,
                data TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS page_presets (
                page_path TEXT PRIMARY KEY,
                preset_idx INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("adjustment.db")
    }

    /// フォルダ/ZIP/PDF のプリセットを取得する。
    pub fn get_presets(&self, container: &Path) -> Option<AdjustPresets> {
        let key = normalize_path(container);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT data FROM presets WHERE container_path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let json: String = row.get(0)?;
            serde_json::from_str(&json).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
            })
        })
        .ok()
    }

    /// フォルダ/ZIP/PDF のプリセットを保存する。全デフォルトなら削除。
    pub fn set_presets(&self, container: &Path, presets: &AdjustPresets) -> Result<(), rusqlite::Error> {
        let key = normalize_path(container);
        if presets.is_all_default() {
            self.conn
                .execute("DELETE FROM presets WHERE container_path = ?1", [&key])?;
        } else {
            let json = serde_json::to_string(presets).unwrap_or_default();
            self.conn.execute(
                "INSERT INTO presets (container_path, data) VALUES (?1, ?2)
                 ON CONFLICT(container_path) DO UPDATE SET data = ?2",
                rusqlite::params![key, json],
            )?;
        }
        Ok(())
    }

    /// ページのプリセット番号を取得する。
    pub fn get_page_preset(&self, page_key: &str) -> Option<u8> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT preset_idx FROM page_presets WHERE page_path = ?1")
            .ok()?;
        stmt.query_row([page_key], |row| {
            let idx: i32 = row.get(0)?;
            Ok(idx as u8)
        })
        .ok()
    }

    /// ページのプリセット番号を設定する。未割り当て (None) なら削除。
    pub fn set_page_preset(&self, page_key: &str, preset_idx: Option<u8>) -> Result<(), rusqlite::Error> {
        match preset_idx {
            None => {
                self.conn
                    .execute("DELETE FROM page_presets WHERE page_path = ?1", [page_key])?;
            }
            Some(idx) => {
                self.conn.execute(
                    "INSERT INTO page_presets (page_path, preset_idx) VALUES (?1, ?2)
                     ON CONFLICT(page_path) DO UPDATE SET preset_idx = ?2",
                    rusqlite::params![page_key, idx as i32],
                )?;
            }
        }
        Ok(())
    }

    /// コンテナ配下の全ページプリセットを一括読み込みする。
    /// `prefix` はコンテナパスの正規化文字列。
    pub fn load_page_presets(&self, prefix: &str) -> std::collections::HashMap<String, u8> {
        let mut map = std::collections::HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, preset_idx FROM page_presets WHERE page_path LIKE ?1 ESCAPE '\\'"
        ) else {
            return map;
        };
        // LIKE 特殊文字 (%, _, [) をエスケープ
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            let path: String = row.get(0)?;
            let idx: i32 = row.get(1)?;
            Ok((path, idx as u8))
        }) else {
            return map;
        };
        for row in rows.flatten() {
            map.insert(row.0, row.1);
        }
        map
    }
}

/// パスを正規化 (小文字化 + バックスラッシュ→スラッシュ)。
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjustment::AdjustParams;

    #[test]
    fn db_presets_roundtrip() {
        let db = AdjustmentDb::open().unwrap();
        let folder = Path::new("C:/test/manga_folder");

        // 初期状態: 未登録
        assert!(db.get_presets(folder).is_none());

        // 設定
        let mut presets = AdjustPresets::default();
        presets.presets[0].brightness = 25.0;
        presets.names[0] = "テスト".to_string();
        db.set_presets(folder, &presets).unwrap();

        let loaded = db.get_presets(folder).unwrap();
        assert_eq!(loaded.presets[0].brightness, 25.0);
        assert_eq!(loaded.names[0], "テスト");

        // デフォルトに戻すと削除
        let default_presets = AdjustPresets::default();
        db.set_presets(folder, &default_presets).unwrap();
        assert!(db.get_presets(folder).is_none());
    }

    #[test]
    fn db_page_presets() {
        let db = AdjustmentDb::open().unwrap();
        let page = "c:/test/folder/page001.jpg";

        assert!(db.get_page_preset(page).is_none());

        db.set_page_preset(page, Some(2)).unwrap();
        assert_eq!(db.get_page_preset(page), Some(2));

        db.set_page_preset(page, None).unwrap();
        assert!(db.get_page_preset(page).is_none());
    }
}
