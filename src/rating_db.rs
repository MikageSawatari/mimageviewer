//! 画像レーティング (★1〜★5) の永続管理。
//!
//! `%APPDATA%/mimageviewer/rating.db` に保存する。
//! 通常画像 / 動画 / ZIP 内画像 / PDF ページ / コンテナに対して
//! 0 (未評価) 〜 5 の星数を記録する。キーは `App::rating_path_key` が返す
//! 正規化キーを使う (`adjustment_db::normalize_path` と同じ規則で統一)。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;

/// ratings.kind に保存する GridItem 種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatingItemKind {
    Image,
    Video,
    Folder,
    ZipFile,
    PdfFile,
    ConvertibleArchive,
    ZipImage,
    PdfPage,
    ZipDir,
    Audio,
}

impl RatingItemKind {
    pub fn to_db(self) -> i64 {
        match self {
            Self::Image => 0,
            Self::Video => 1,
            Self::Folder => 2,
            Self::ZipFile => 3,
            Self::PdfFile => 4,
            Self::ConvertibleArchive => 5,
            Self::ZipImage => 6,
            Self::PdfPage => 7,
            Self::ZipDir => 8,
            // Audio は Inc 5 (音楽ビュー) で追加。既存 0..=8 の後ろに足すだけなので、
            // 既存行の判別子は不変 (未リリース機能だが後方互換で安全)。
            Self::Audio => 9,
        }
    }

    pub fn from_db(raw: i64) -> Option<Self> {
        match raw {
            0 => Some(Self::Image),
            1 => Some(Self::Video),
            2 => Some(Self::Folder),
            3 => Some(Self::ZipFile),
            4 => Some(Self::PdfFile),
            5 => Some(Self::ConvertibleArchive),
            6 => Some(Self::ZipImage),
            7 => Some(Self::PdfPage),
            8 => Some(Self::ZipDir),
            9 => Some(Self::Audio),
            _ => None,
        }
    }
}

/// ratings に保存する復元用メタデータ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatingMeta {
    pub kind: RatingItemKind,
    pub source_path: Option<String>,
    pub entry_name: Option<String>,
    pub page_num: Option<u32>,
    pub dir_prefix: Option<String>,
    pub archive_format: Option<String>,
    pub zipdir_is_archive: Option<bool>,
    pub zipdir_representative: Option<String>,
}

impl RatingMeta {
    pub fn new(kind: RatingItemKind) -> Self {
        Self {
            kind,
            source_path: None,
            entry_name: None,
            page_num: None,
            dir_prefix: None,
            archive_format: None,
            zipdir_is_archive: None,
            zipdir_representative: None,
        }
    }

    pub fn with_source_path(mut self, path: impl AsRef<Path>) -> Self {
        self.source_path = Some(path.as_ref().to_string_lossy().to_string());
        self
    }
}

/// レーティング一覧ビュー構築用の DB 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatingRow {
    pub key: String,
    pub stars: u8,
    pub rated_at_ms: Option<i64>,
    pub source_path: Option<String>,
    pub kind: Option<RatingItemKind>,
    pub entry_name: Option<String>,
    pub page_num: Option<u32>,
    pub dir_prefix: Option<String>,
    pub archive_format: Option<String>,
    pub zipdir_is_archive: Option<bool>,
    pub zipdir_representative: Option<String>,
}

/// レーティング DB ハンドル。
pub struct RatingDb {
    conn: rusqlite::Connection,
}

impl RatingDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(Self::db_path())
    }

    /// 指定パスの DB を開く。worker / test 用。
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Self::ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    /// 既存 DB を読み取り専用で開く (一覧ビュー worker 用)。`ensure_schema` を呼ばないので
    /// マイグレーション DDL を再実行しない。main 接続が起動時に移行済みである前提
    /// (= read-only の worker 接続が ALTER TABLE を再発行して main 接続と競合しない)。
    /// ファイルが無い / 開けない場合は呼び出し側が `open_at` へフォールバックする。
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Ok(Self { conn })
    }

    /// 起動時 migration 済みの既存 DB を worker から更新用に開く。
    /// delete worker が main 接続と並行して schema DDL を再発行しないための経路。
    pub fn open_existing_for_write(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { conn })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("rating.db")
    }

    fn ensure_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ratings (
                path TEXT PRIMARY KEY,
                stars INTEGER NOT NULL
            )",
        )?;
        Self::add_column_if_missing(conn, "rated_at_ms", "INTEGER")?;
        Self::add_column_if_missing(conn, "source_path", "TEXT")?;
        Self::add_column_if_missing(conn, "kind", "INTEGER")?;
        Self::add_column_if_missing(conn, "entry_name", "TEXT")?;
        Self::add_column_if_missing(conn, "page_num", "INTEGER")?;
        Self::add_column_if_missing(conn, "dir_prefix", "TEXT")?;
        Self::add_column_if_missing(conn, "archive_format", "TEXT")?;
        Self::add_column_if_missing(conn, "zipdir_is_archive", "INTEGER")?;
        Self::add_column_if_missing(conn, "zipdir_representative", "TEXT")?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_ratings_stars_rated_at
                ON ratings(stars, rated_at_ms DESC)",
        )?;
        Ok(())
    }

    fn add_column_if_missing(
        conn: &rusqlite::Connection,
        name: &str,
        decl: &str,
    ) -> Result<(), rusqlite::Error> {
        let exists = conn
            .prepare("PRAGMA table_info(ratings)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|col| col == name);
        if !exists {
            conn.execute(&format!("ALTER TABLE ratings ADD COLUMN {name} {decl}"), [])?;
        }
        Ok(())
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

    /// 複数キーをまとめて取得する。フォルダ読み込み直後のキャッシュプリウォーム用。
    /// 結果に含まれないキーは未登録 (=0) を意味する。
    pub fn get_many(&self, keys: &[String]) -> std::collections::HashMap<String, u8> {
        let mut out = std::collections::HashMap::new();
        if keys.is_empty() {
            return out;
        }
        // SQLite の式ツリー上限を避けるため 500 件ずつ分割
        for chunk in keys.chunks(500) {
            let placeholders = (0..chunk.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT path, stars FROM ratings WHERE path IN ({})",
                placeholders
            );
            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let path: String = row.get(0)?;
                let stars: i32 = row.get(1)?;
                Ok((path, stars.clamp(0, 5) as u8))
            });
            if let Ok(rows) = rows {
                for r in rows.flatten() {
                    out.insert(r.0, r.1);
                }
            }
        }
        out
    }

    /// 互換 API。ユーザー操作として扱い、非ゼロ値には rated_at_ms を入れる。
    pub fn set(&self, key: &str, stars: u8) -> Result<(), rusqlite::Error> {
        self.set_user_rating(key, stars, None)
    }

    /// ユーザーが明示的にレーティングを設定する。非ゼロ値には現在時刻を入れる。
    pub fn set_user_rating(
        &self,
        key: &str,
        stars: u8,
        meta: Option<&RatingMeta>,
    ) -> Result<(), rusqlite::Error> {
        self.set_with_timestamp(key, stars, meta, Some(now_ms()))
    }

    /// XMP hydration など外部由来の取り込み。非ゼロ値でも rated_at_ms は NULL。
    pub fn set_imported_rating(
        &self,
        key: &str,
        stars: u8,
        meta: Option<&RatingMeta>,
    ) -> Result<(), rusqlite::Error> {
        self.set_with_timestamp(key, stars, meta, None)
    }

    fn set_with_timestamp(
        &self,
        key: &str,
        stars: u8,
        meta: Option<&RatingMeta>,
        rated_at_ms: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        let stars = stars.min(5);
        if stars == 0 {
            self.conn
                .execute("DELETE FROM ratings WHERE path = ?1", [key])?;
            return Ok(());
        }

        let kind = meta.map(|m| m.kind.to_db());
        let source_path = meta.and_then(|m| m.source_path.as_deref());
        let entry_name = meta.and_then(|m| m.entry_name.as_deref());
        let page_num = meta.and_then(|m| m.page_num.map(i64::from));
        let dir_prefix = meta.and_then(|m| m.dir_prefix.as_deref());
        let archive_format = meta.and_then(|m| m.archive_format.as_deref());
        let zipdir_is_archive =
            meta.and_then(|m| m.zipdir_is_archive.map(|v| if v { 1_i64 } else { 0_i64 }));
        let zipdir_representative = meta.and_then(|m| m.zipdir_representative.as_deref());

        self.conn.execute(
            "INSERT INTO ratings (
                path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
                dir_prefix, archive_format, zipdir_is_archive, zipdir_representative
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(path) DO UPDATE SET
                stars = excluded.stars,
                rated_at_ms = excluded.rated_at_ms,
                source_path = excluded.source_path,
                kind = excluded.kind,
                entry_name = excluded.entry_name,
                page_num = excluded.page_num,
                dir_prefix = excluded.dir_prefix,
                archive_format = excluded.archive_format,
                zipdir_is_archive = excluded.zipdir_is_archive,
                zipdir_representative = excluded.zipdir_representative",
            rusqlite::params![
                key,
                stars as i64,
                rated_at_ms,
                source_path,
                kind,
                entry_name,
                page_num,
                dir_prefix,
                archive_format,
                zipdir_is_archive,
                zipdir_representative,
            ],
        )?;
        Ok(())
    }

    pub fn copy_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        let to_source_path = source_path_from_rating_key(to_key);
        self.conn.execute(
            "INSERT INTO ratings (
                path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
                dir_prefix, archive_format, zipdir_is_archive, zipdir_representative
            )
            SELECT
                ?2, stars, rated_at_ms, COALESCE(?3, source_path), kind, entry_name, page_num,
                dir_prefix, archive_format, zipdir_is_archive, zipdir_representative
            FROM ratings WHERE path = ?1
            ON CONFLICT(path) DO UPDATE SET
                stars = excluded.stars,
                rated_at_ms = excluded.rated_at_ms,
                source_path = excluded.source_path,
                kind = excluded.kind,
                entry_name = excluded.entry_name,
                page_num = excluded.page_num,
                dir_prefix = excluded.dir_prefix,
                archive_format = excluded.archive_format,
                zipdir_is_archive = excluded.zipdir_is_archive,
                zipdir_representative = excluded.zipdir_representative",
            rusqlite::params![from_key, to_key, to_source_path],
        )?;
        Ok(())
    }

    pub fn move_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.copy_entry_key(from_key, to_key)?;
        self.conn
            .execute("DELETE FROM ratings WHERE path = ?1", [from_key])?;
        Ok(())
    }

    /// 指定★の行を一覧用に取得する。
    pub fn list_by_stars(&self, stars: u8) -> Result<Vec<RatingRow>, rusqlite::Error> {
        let stars = stars.clamp(1, 5);
        let mut stmt = self.conn.prepare(
            "SELECT path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
                    dir_prefix, archive_format, zipdir_is_archive, zipdir_representative
             FROM ratings
             WHERE stars = ?1
             ORDER BY COALESCE(rated_at_ms, -1) DESC, path",
        )?;
        let rows = stmt.query_map([stars as i64], row_from_sql)?;
        rows.collect()
    }

    /// ★0〜★5 の件数。index 0 は未評価ではなく常に 0 (DB 行が無いため)。
    pub fn count_by_stars(&self) -> Result<[usize; 6], rusqlite::Error> {
        let mut out = [0usize; 6];
        let mut stmt = self
            .conn
            .prepare("SELECT stars, COUNT(*) FROM ratings GROUP BY stars")?;
        let rows = stmt.query_map([], |row| {
            let stars: i64 = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((stars, count))
        })?;
        for row in rows.flatten() {
            let (stars, count) = row;
            if (1..=5).contains(&stars) {
                out[stars as usize] = count.max(0) as usize;
            }
        }
        Ok(out)
    }

    /// 指定キーの 1 行を一覧復元用に取得する。
    pub fn row_for_key(&self, key: &str) -> Result<Option<RatingRow>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT path, stars, rated_at_ms, source_path, kind, entry_name, page_num,
                        dir_prefix, archive_format, zipdir_is_archive, zipdir_representative
                 FROM ratings
                 WHERE path = ?1",
                [key],
                row_from_sql,
            )
            .optional()
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

fn row_from_sql(row: &rusqlite::Row<'_>) -> Result<RatingRow, rusqlite::Error> {
    let stars: i32 = row.get(1)?;
    let kind_raw: Option<i64> = row.get(4)?;
    let page_num_raw: Option<i64> = row.get(6)?;
    let zipdir_is_archive_raw: Option<i64> = row.get(9)?;
    Ok(RatingRow {
        key: row.get(0)?,
        stars: stars.clamp(0, 5) as u8,
        rated_at_ms: row.get(2)?,
        source_path: row.get(3)?,
        kind: kind_raw.and_then(RatingItemKind::from_db),
        entry_name: row.get(5)?,
        page_num: page_num_raw.and_then(|v| u32::try_from(v).ok()),
        dir_prefix: row.get(7)?,
        archive_format: row.get(8)?,
        zipdir_is_archive: zipdir_is_archive_raw.map(|v| v != 0),
        zipdir_representative: row.get(10)?,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn source_path_from_rating_key(key: &str) -> Option<&str> {
    let source = key.split_once("::").map(|(left, _)| left).unwrap_or(key);
    (!source.is_empty()).then_some(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_schema(schema: &str) -> RatingDb {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(schema).unwrap();
        RatingDb::ensure_schema(&conn).unwrap();
        RatingDb { conn }
    }

    fn empty_db() -> RatingDb {
        db_with_schema("")
    }

    #[test]
    fn set_get_clear() {
        let db = empty_db();

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
        let db = empty_db();
        db.set("x", 99).unwrap();
        assert_eq!(db.get("x"), 5);
    }

    #[test]
    fn migrates_old_schema() {
        let db = db_with_schema(
            "CREATE TABLE ratings (
                path TEXT PRIMARY KEY,
                stars INTEGER NOT NULL
            );",
        );
        db.set("x", 4).unwrap();
        let row = db.row_for_key("x").unwrap().unwrap();
        assert_eq!(row.stars, 4);
        assert!(row.rated_at_ms.is_some());
    }

    #[test]
    fn user_rating_sets_time_and_meta() {
        let db = empty_db();
        let mut meta =
            RatingMeta::new(RatingItemKind::ZipImage).with_source_path(r"C:\Books\a.zip");
        meta.entry_name = Some("Dir/Page.JPG".to_string());

        db.set_user_rating("c:/books/a.zip::dir/page.jpg", 4, Some(&meta))
            .unwrap();

        let row = db
            .row_for_key("c:/books/a.zip::dir/page.jpg")
            .unwrap()
            .unwrap();
        assert_eq!(row.stars, 4);
        assert!(row.rated_at_ms.is_some());
        assert_eq!(row.kind, Some(RatingItemKind::ZipImage));
        assert_eq!(row.source_path.as_deref(), Some(r"C:\Books\a.zip"));
        assert_eq!(row.entry_name.as_deref(), Some("Dir/Page.JPG"));
    }

    #[test]
    fn imported_rating_leaves_time_null() {
        let db = empty_db();
        let meta = RatingMeta::new(RatingItemKind::Image).with_source_path(r"C:\img\a.jpg");

        db.set_imported_rating("c:/img/a.jpg", 3, Some(&meta))
            .unwrap();

        let row = db.row_for_key("c:/img/a.jpg").unwrap().unwrap();
        assert_eq!(row.stars, 3);
        assert_eq!(row.rated_at_ms, None);
        assert_eq!(row.kind, Some(RatingItemKind::Image));
    }

    #[test]
    fn list_counts_and_copy_move_carry_metadata() {
        let db = empty_db();
        let mut meta = RatingMeta::new(RatingItemKind::PdfPage).with_source_path(r"C:\Books\a.pdf");
        meta.page_num = Some(12);
        db.set_user_rating("c:/books/a.pdf::page_12", 5, Some(&meta))
            .unwrap();

        let counts = db.count_by_stars().unwrap();
        assert_eq!(counts[5], 1);
        assert_eq!(db.list_by_stars(5).unwrap().len(), 1);

        db.copy_entry_key("c:/books/a.pdf::page_12", "c:/books/b.pdf::page_12")
            .unwrap();
        let copied = db.row_for_key("c:/books/b.pdf::page_12").unwrap().unwrap();
        assert_eq!(copied.kind, Some(RatingItemKind::PdfPage));
        assert_eq!(copied.page_num, Some(12));
        assert_eq!(copied.source_path.as_deref(), Some("c:/books/b.pdf"));
        assert!(copied.rated_at_ms.is_some());

        db.move_entry_key("c:/books/b.pdf::page_12", "c:/books/c.pdf::page_12")
            .unwrap();
        assert!(db.row_for_key("c:/books/b.pdf::page_12").unwrap().is_none());
        let moved = db.row_for_key("c:/books/c.pdf::page_12").unwrap().unwrap();
        assert_eq!(moved.kind, Some(RatingItemKind::PdfPage));
        assert_eq!(moved.page_num, Some(12));
        assert_eq!(moved.source_path.as_deref(), Some("c:/books/c.pdf"));
    }
}
