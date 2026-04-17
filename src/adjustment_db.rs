//! 画像補正のページ個別設定を永続管理する。
//!
//! `%APPDATA%/mimageviewer/adjustment.db` に `page_params` テーブルとして保存する。
//! 旧 (v0.6.0 開発版) の `presets` テーブル / preset_idx 方式は廃止。
//! 表示時の有効パラメータは `page_params.get(page) ?? settings.global_preset`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::adjustment::AdjustParams;

/// 補正設定 DB ハンドル。
pub struct AdjustmentDb {
    conn: rusqlite::Connection,
}

impl AdjustmentDb {
    /// DB を開く (なければ作成)。旧スキーマが残っていれば破棄して作り直す。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    /// 通常のランタイムパス (`%APPDATA%/mimageviewer/adjustment.db`) を使いたい場合は
    /// 引数なしの [`open`] を使うこと。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        // 未リリース機能なのでマイグレーションは行わず旧テーブルを破棄する。
        conn.execute_batch(
            "DROP TABLE IF EXISTS presets;
             DROP TABLE IF EXISTS page_presets;
             CREATE TABLE IF NOT EXISTS page_params (
                page_path TEXT PRIMARY KEY,
                params_json TEXT NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("adjustment.db")
    }

    /// ページのパラメータを取得する。未登録なら None。
    pub fn get_page_params(&self, page_key: &str) -> Option<AdjustParams> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT params_json FROM page_params WHERE page_path = ?1")
            .ok()?;
        let json: String = stmt.query_row([page_key], |row| row.get(0)).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// ページのパラメータを書き込む。
    ///
    /// 削除判定 (個別保存する意味があるか) は呼び出し側の責務。
    /// 旧バージョンでは `params.is_removable()` で内部的に削除に振り分けていたが、
    /// グローバルが非デフォルトのとき「個別で AI を OFF にする」上書きを保存したい
    /// ケースを取り逃すため、is_removable 判定を呼び出し側 (`App::set_page_params`) に
    /// 移した。明示的に削除したいときは `remove_page_params` を呼ぶこと。
    pub fn set_page_params(&self, page_key: &str, params: &AdjustParams) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(params).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)
             ON CONFLICT(page_path) DO UPDATE SET params_json = ?2",
            rusqlite::params![page_key, json],
        )?;
        Ok(())
    }

    /// ページのパラメータ個別設定を削除する。
    pub fn remove_page_params(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM page_params WHERE page_path = ?1", [page_key])?;
        Ok(())
    }

    /// 複数ページに同じパラメータを一括書込する (「全画像に適用」ボタン用)。
    ///
    /// 削除判定 (= グローバルと等価かどうか) は呼び出し側の責務。
    /// グローバルと等価な params を渡したい場合は `remove_page_params_bulk` を使う。
    pub fn set_page_params_bulk(
        &mut self,
        page_keys: &[String],
        params: &AdjustParams,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let json = serde_json::to_string(params).unwrap_or_default();
        let mut stmt = tx.prepare(
            "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)
             ON CONFLICT(page_path) DO UPDATE SET params_json = ?2",
        )?;
        for key in page_keys {
            stmt.execute(rusqlite::params![key, json])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// 複数ページの個別パラメータを一括削除する (「全画像から削除」ボタン用)。
    pub fn remove_page_params_bulk(
        &mut self,
        page_keys: &[String],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare("DELETE FROM page_params WHERE page_path = ?1")?;
        for key in page_keys {
            stmt.execute([key])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// コンテナ配下の全ページ個別パラメータを一括読込する。
    /// `prefix` はコンテナパスの正規化文字列。
    pub fn load_page_params(&self, prefix: &str) -> HashMap<String, AdjustParams> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, params_json FROM page_params WHERE page_path LIKE ?1 ESCAPE '\\'"
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
            let json: String = row.get(1)?;
            Ok((path, json))
        }) else {
            return map;
        };
        for row in rows.flatten() {
            if let Ok(params) = serde_json::from_str::<AdjustParams>(&row.1) {
                map.insert(row.0, params);
            }
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

    #[test]
    fn db_page_params_roundtrip() {
        let db = AdjustmentDb::open().unwrap();
        let page = "c:/test/folder/page001.jpg";
        // クリーンな状態を保証
        db.remove_page_params(page).unwrap();
        assert!(db.get_page_params(page).is_none());

        let mut params = AdjustParams::default();
        params.brightness = 30.0;
        db.set_page_params(page, &params).unwrap();
        let loaded = db.get_page_params(page).unwrap();
        assert_eq!(loaded.brightness, 30.0);

        // identity でも DB は保存する (削除判定は呼び出し側 App::set_page_params に移った)
        // 個別を「グローバルが AI ON のとき AI OFF として上書き」したいケースを保存できるように。
        db.set_page_params(page, &AdjustParams::default()).unwrap();
        let loaded_identity = db.get_page_params(page).unwrap();
        assert!(loaded_identity.is_identity());

        // 明示削除すれば消える
        db.remove_page_params(page).unwrap();
        assert!(db.get_page_params(page).is_none());
    }
}
