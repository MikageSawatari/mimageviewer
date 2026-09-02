//! テキスト注釈 (comic) の永続管理。
//!
//! `%APPDATA%/mimageviewer/comic.db` に「1 画像 = 注釈ドキュメント JSON」を保存する。
//! キーは [`crate::app::App::page_path_key`] (Image / ZipImage / PdfPage)。実装は
//! [`crate::conceal_db`] / [`crate::mask_db`] と同形だが、ビットマップではなく
//! `Vec<AnnotationObject>` を serde_json で 1 行に直列化する点だけが異なる。
//!
//! 統合契約は docs/comic-integration-plan.md §6 (D4 保存方式)。中央 DB が正本で、
//! 「設定のバックアップ」ON 時のみフォルダの `mimageviewer.dat` にミラーする (§6.2、Inc 2b)。
//!
//! ## スキーマ版数 (§6.1)
//!
//! - テーブルに `PRAGMA user_version`（[`SCHEMA_VERSION`]）。将来のスキーマ移行用。
//! - JSON ドキュメント側に `doc_version`（[`DOC_VERSION`]）列。注釈フォーマットの版。
//! - no-row = 空注釈。壊れ JSON = 空 + ログ（クラッシュさせない）。
//!
//! 新規機能なので旧 mIV データからの移行は不要。ラボ `.comic.json` の取り込みは
//! 別途指示があるまで行わない（`doc_version` は将来用に予約）。

use std::path::PathBuf;

use comic_core::AnnotationObject;

/// テーブルスキーマの版 (`PRAGMA user_version`)。スキーマ自体を変えたら +1 して移行する。
pub const SCHEMA_VERSION: i64 = 1;
/// 注釈ドキュメント (JSON) フォーマットの版。`AnnotationObject` の構造を破壊的に
/// 変えたら +1 して読み替えを用意する。
pub const DOC_VERSION: u32 = 1;

/// 注釈永続化 DB (comic)。内部は SQLite `comic_entries` テーブル。
pub struct ComicDb {
    conn: rusqlite::Connection,
}

impl ComicDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_readonly() -> Result<Self, rusqlite::Error> {
        Self::open_readonly_at(&Self::db_path())
    }

    pub fn open_readonly_at(path: &std::path::Path) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self { conn })
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    pub fn open_at(path: &std::path::Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS comic_entries (
                page_path   TEXT    PRIMARY KEY,
                doc_version INTEGER NOT NULL,
                doc_json    TEXT    NOT NULL
            )",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("comic.db")
    }

    /// 注釈ドキュメントを取得する。no-row / 壊れ JSON は `None`（= 空注釈扱い、
    /// クラッシュさせない。壊れ JSON はログに残す）。
    pub fn get(&self, key: &str) -> Option<Vec<AnnotationObject>> {
        let (_, json) = self.get_raw(key)?;
        match serde_json::from_str::<Vec<AnnotationObject>>(&json) {
            Ok(objs) => Some(objs),
            Err(e) => {
                crate::logger::log(format!("[comic] comic.db doc parse failed key={key}: {e}"));
                None
            }
        }
    }

    pub(crate) fn get_checked(&self, key: &str) -> Result<Option<Vec<AnnotationObject>>, String> {
        let json = self.get_json_checked(key)?;
        Ok(json.and_then(|json| match serde_json::from_str(&json) {
            Ok(objects) => Some(objects),
            Err(error) => {
                crate::logger::log(format!(
                    "[comic] comic.db doc parse failed key={key}: {error}"
                ));
                None
            }
        }))
    }

    /// 未選択の注釈を別操作の全置換で保持するときの strict read。破損行を空注釈と
    /// みなすと、別種類だけをリセットした操作が注釈まで削除してしまう。
    pub(crate) fn get_checked_strict(
        &self,
        key: &str,
    ) -> Result<Option<Vec<AnnotationObject>>, String> {
        use rusqlite::OptionalExtension as _;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT doc_version, doc_json FROM comic_entries WHERE page_path = ?1")
            .map_err(|error| error.to_string())?;
        let row: Option<(i64, String)> = stmt
            .query_row([key], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(|error| error.to_string())?;
        let Some((doc_version, json)) = row else {
            return Ok(None);
        };
        if doc_version != i64::from(DOC_VERSION) {
            return Err(format!(
                "comic.db の注釈形式が未対応です key={key}: version={doc_version}"
            ));
        }
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| format!("comic.db の注釈 JSON が壊れています key={key}: {error}"))
    }

    fn get_json_checked(&self, key: &str) -> Result<Option<String>, String> {
        use rusqlite::OptionalExtension as _;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT doc_json FROM comic_entries WHERE page_path = ?1")
            .map_err(|error| error.to_string())?;
        stmt.query_row([key], |row| row.get(0))
            .optional()
            .map_err(|error| error.to_string())
    }

    /// 生の `(doc_version, doc_json)` を取得する（サイドカー dual-write 用、パース回避）。
    pub fn get_raw(&self, key: &str) -> Option<(u32, String)> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT doc_version, doc_json FROM comic_entries WHERE page_path = ?1")
            .ok()?;
        stmt.query_row([key], |row| {
            Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
        })
        .ok()
    }

    /// 注釈ドキュメントを保存する。空（オブジェクト 0 個）なら削除する（no-row = 空注釈）。
    pub fn set(&self, key: &str, objects: &[AnnotationObject]) -> rusqlite::Result<()> {
        if objects.is_empty() {
            return self.delete(key);
        }
        let json = serde_json::to_string(objects).unwrap_or_else(|_| "[]".to_string());
        self.set_raw(key, DOC_VERSION, &json)
    }

    /// JSON 文字列を直接保存する（サイドカーからのインポート用、再シリアライズ回避）。
    pub fn set_raw(&self, key: &str, doc_version: u32, doc_json: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO comic_entries (page_path, doc_version, doc_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(page_path) DO UPDATE SET doc_version = ?2, doc_json = ?3",
            rusqlite::params![key, doc_version as i64, doc_json],
        )?;
        Ok(())
    }

    /// 注釈ドキュメントを削除する。
    pub fn delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM comic_entries WHERE page_path = ?1", [key])?;
        Ok(())
    }

    pub fn copy_entry_key(&self, from_key: &str, to_key: &str) -> rusqlite::Result<()> {
        if from_key == to_key {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO comic_entries (page_path, doc_version, doc_json)
             SELECT ?2, doc_version, doc_json FROM comic_entries WHERE page_path = ?1
             ON CONFLICT(page_path) DO UPDATE SET
                doc_version = excluded.doc_version,
                doc_json = excluded.doc_json",
            rusqlite::params![from_key, to_key],
        )?;
        Ok(())
    }

    pub fn move_entry_key(&self, from_key: &str, to_key: &str) -> rusqlite::Result<()> {
        if from_key == to_key {
            return Ok(());
        }
        self.copy_entry_key(from_key, to_key)?;
        self.delete(from_key)
    }

    /// 指定プレフィックスで始まるキー集合を返す。フォルダ単位の「このフォルダ内で注釈を
    /// 持つページ」列挙（グリッドのバッジ用）に使う。conceal の `load_conceal_keys` と同形。
    pub fn load_comic_keys(&self, prefix: &str) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path FROM comic_entries WHERE page_path LIKE ?1 ESCAPE '\\'",
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
        for r in rows.flatten() {
            set.insert(r);
        }
        set
    }

    /// 指定ページキーのうち、テキスト注釈を持つものだけを返す。
    pub fn load_existing_comic_keys(
        &self,
        page_keys: &[&str],
    ) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        for chunk in page_keys.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT page_path FROM comic_entries WHERE page_path IN ({placeholders})");
            let Ok(mut stmt) = self.conn.prepare(&sql) else {
                continue;
            };
            if let Ok(rows) = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
                    row.get::<_, String>(0)
                })
            {
                set.extend(rows.flatten());
            }
        }
        set
    }

    /// テキスト注釈を持つページキーを全件返す。
    ///
    /// スマートフィルタの親コンテナ判定用。`doc_json` は読まない。
    pub fn load_all_comic_keys(&self) -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        let Ok(mut stmt) = self
            .conn
            .prepare_cached("SELECT page_path FROM comic_entries")
        else {
            return set;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for row in rows.flatten() {
            set.insert(row);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use comic_core::{AnnotationObject, Orientation, Rgba, TextBlock};

    fn tmp_db() -> (ComicDb, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "mimageviewer_comic_db_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        (ComicDb::open_at(&p).expect("open"), p)
    }

    fn sample_objects() -> Vec<AnnotationObject> {
        let tb = TextBlock {
            text: "テスト注釈".to_string(),
            size_px: 48.0,
            color: Rgba::BLACK,
            orientation: Orientation::Vertical,
            ..TextBlock::default()
        };
        vec![
            AnnotationObject::new_text(1, (10.0, 20.0), tb.clone()),
            AnnotationObject::new_text(2, (100.0, 200.0), tb),
        ]
    }

    #[test]
    fn set_and_get_roundtrip() {
        let (db, p) = tmp_db();
        let objs = sample_objects();
        db.set("c:/foo/img.png", &objs).unwrap();
        let got = db.get("c:/foo/img.png").expect("get");
        assert_eq!(got, objs);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_existing_comic_keys_returns_only_requested_exact_keys() {
        let (db, p) = tmp_db();
        db.set_raw("c:/a.jpg", DOC_VERSION, "[]").unwrap();
        db.set_raw("c:/b.jpg", DOC_VERSION, "[]").unwrap();
        let loaded = db.load_existing_comic_keys(&["c:/b.jpg", "c:/missing.jpg"]);
        assert_eq!(
            loaded,
            std::collections::HashSet::from(["c:/b.jpg".to_string()])
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn empty_objects_deletes() {
        let (db, p) = tmp_db();
        db.set("k", &sample_objects()).unwrap();
        assert!(db.get("k").is_some());
        // 空注釈で set すると削除される (no-row = 空注釈)。
        db.set("k", &[]).unwrap();
        assert!(db.get("k").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn edit_bundle_bulk_comic_strict_reader_rejects_corrupt_json() {
        let (db, p) = tmp_db();
        db.set_raw("k", DOC_VERSION, "{ this is not valid json ]")
            .unwrap();
        // 壊れ JSON はクラッシュせず None (空注釈扱い)。
        assert!(db.get("k").is_none());
        // get_raw は生の文字列をそのまま返す。
        assert_eq!(
            db.get_raw("k").map(|(_, j)| j).as_deref(),
            Some("{ this is not valid json ]")
        );
        assert!(db.get_checked_strict("k").is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn edit_bundle_bulk_comic_strict_reader_rejects_unknown_document_version() {
        let (db, p) = tmp_db();
        db.set_raw("k", DOC_VERSION + 1, "[]").unwrap();
        assert_eq!(db.get("k"), Some(Vec::new()));
        assert!(db.get_checked_strict("k").is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_key_is_none() {
        let (db, p) = tmp_db();
        assert!(db.get("nope").is_none());
        assert!(db.get_raw("nope").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn delete_removes_entry() {
        let (db, p) = tmp_db();
        db.set("k", &sample_objects()).unwrap();
        db.delete("k").unwrap();
        assert!(db.get("k").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn set_raw_roundtrip_with_doc_version() {
        let (db, p) = tmp_db();
        let json = serde_json::to_string(&sample_objects()).unwrap();
        db.set_raw("k", 7, &json).unwrap();
        let (ver, got_json) = db.get_raw("k").expect("get_raw");
        assert_eq!(ver, 7);
        assert_eq!(got_json, json);
        // get() でパースして等価。
        assert_eq!(db.get("k").unwrap(), sample_objects());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_comic_keys_by_prefix() {
        let (db, p) = tmp_db();
        let objs = sample_objects();
        db.set("c:/foo/img1.png", &objs).unwrap();
        db.set("c:/foo/img2.png", &objs).unwrap();
        db.set("c:/other/img.png", &objs).unwrap();
        let keys = db.load_comic_keys("c:/foo/");
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("c:/foo/img1.png"));
        assert!(keys.contains("c:/foo/img2.png"));
        let _ = std::fs::remove_file(&p);
    }
}
