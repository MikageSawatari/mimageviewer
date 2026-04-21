//! `fts_meta.db` — 全文メタ検索のファイル単位メタ管理 + post-filter 用原文保存。
//!
//! docs/search-expansion-design.md §5.3, §5.6 に準拠する。
//!
//! Tantivy インデックス (`fts_index/`) とは別 DB で、以下を担う:
//! - お気に入り単位の登録ファイル追跡 (差分検出の基準)
//! - `all_text_norm` の保存 (Ctrl+G post-filter での一括取得)
//! - `status=pending / ok / failed / tombstone` の二段整合性状態
//! - ingest 世代カウンタ (`index_generation`) — 将来のスナップショット用
//!
//! **スレッド安全性**: `Mutex<Connection>` で包む (既存 catalog.rs と同じパターン)。
//! 頻繁な UPSERT は Ingest Worker から呼ばれるため、ロックは短く保つ。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::search_index_db::normalize_path;

/// スキーマ変更時に bump することで、次回起動時に全再インデックスをトリガする定数。
pub const INDEX_VERSION: i64 = 1;

/// 1 ファイル/ZIP エントリに対応する fts_meta.db の行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMeta {
    /// 正規化済みパス (lowercase + `/`、ドライブレター保持、ZIP 内は `<zip>!<entry>`)
    pub path: String,
    pub favorite_id: Uuid,
    pub favorite_root: PathBuf,
    pub mtime: i64,
    pub file_size: i64,
    pub indexed_at: i64,
    pub index_version: i64,
    pub index_generation: i64,
    pub status: FileStatus,
    /// post-filter で使う正規化済み全文。`search_norm::normalize_for_match` 済み。
    pub all_text_norm: String,
}

/// ingest と delete の二段整合性プロトコル用 (§5.6)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum FileStatus {
    /// Tantivy にコミット済み、検索可能
    Ok = 0,
    /// ingest 開始済み、Tantivy へのコミット待ち / 進行中
    Pending = 1,
    /// ingest 失敗 (次回再試行)
    Failed = 2,
    /// 削除予定 (Tantivy からの delete を待つ間、post-filter で除外)
    Tombstone = 3,
}

impl FileStatus {
    fn from_i64(v: i64) -> Self {
        match v {
            0 => Self::Ok,
            1 => Self::Pending,
            2 => Self::Failed,
            3 => Self::Tombstone,
            _ => Self::Failed, // 不正値は failed 扱いで再試行
        }
    }
}

/// fts_meta.db への接続。
pub struct FtsMetaDb {
    conn: Mutex<Connection>,
}

impl FtsMetaDb {
    /// `%APPDATA%/mimageviewer/fts_meta.db` を開く (なければ作成)。
    pub fn open() -> rusqlite::Result<Self> {
        let db_path = crate::data_dir::get().join("fts_meta.db");
        Self::open_at(&db_path)
    }

    /// 任意パスに開く (テスト用)。
    pub fn open_at(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// status=pending で UPSERT。既存 row の generation を増やし、all_text_norm を更新する。
    /// Tantivy への add_document 前に呼ばれる (§5.6.1 ステップ 1)。
    pub fn mark_pending(
        &self,
        path: &str,
        favorite_id: Uuid,
        favorite_root: &Path,
        mtime: i64,
        file_size: i64,
        all_text_norm: &str,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = now_epoch();
        // generation は UPSERT 時に +1 する (旧 row があるなら)。CTE で現行値 +1 を計算。
        conn.execute(
            "INSERT INTO files (
                path, favorite_id, favorite_root, mtime, file_size,
                indexed_at, index_version, index_generation, status, all_text_norm
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)
             ON CONFLICT(path) DO UPDATE SET
                favorite_id = excluded.favorite_id,
                favorite_root = excluded.favorite_root,
                mtime = excluded.mtime,
                file_size = excluded.file_size,
                indexed_at = excluded.indexed_at,
                index_version = excluded.index_version,
                index_generation = files.index_generation + 1,
                status = 1,
                all_text_norm = excluded.all_text_norm",
            params![
                path,
                favorite_id.to_string(),
                favorite_root.to_string_lossy().into_owned(),
                mtime,
                file_size,
                now,
                INDEX_VERSION,
                FileStatus::Pending as i64,
                all_text_norm,
            ],
        )?;
        let gen_val: i64 = conn.query_row(
            "SELECT index_generation FROM files WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )?;
        Ok(gen_val)
    }

    /// Tantivy commit 後に呼ぶ。status=pending → ok に遷移 (§5.6.1 ステップ 4)。
    pub fn mark_ok(&self, paths: &[String]) -> rusqlite::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE files SET status = 0 WHERE path = ?1 AND status = 1")?;
            for p in paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// ingest 失敗を記録。次回再試行 (retry 抑制は上位レイヤーで管理)。
    pub fn mark_failed(&self, path: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE files SET status = 2 WHERE path = ?1",
            params![path],
        )?;
        Ok(())
    }

    /// 削除開始: status → tombstone (§5.6.2 ステップ 1)。
    /// post-filter 時の除外対象になる。Tantivy delete 後に `purge_tombstone` で完全削除。
    pub fn mark_tombstone(&self, paths: &[String]) -> rusqlite::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE files SET status = 3 WHERE path = ?1")?;
            for p in paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Tantivy からの delete が commit された後に呼ぶ (§5.6.2 ステップ 4)。
    /// tombstone 状態の行を物理削除する。
    pub fn purge_tombstone(&self, paths: &[String]) -> rusqlite::Result<usize> {
        if paths.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut deleted = 0;
        {
            let mut stmt = tx.prepare("DELETE FROM files WHERE path = ?1 AND status = 3")?;
            for p in paths {
                deleted += stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// post-filter 用: 指定 path 群の all_text_norm を一括取得 (§9.1 ステップ 5)。
    /// tombstone は含まない (検索結果から除外するため)。
    /// 戻り値は入力 path 順不同の `Vec<(path, all_text_norm)>`。
    pub fn lookup_all_text_norm(
        &self,
        paths: &[String],
    ) -> rusqlite::Result<Vec<(String, String)>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = (0..paths.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT path, all_text_norm FROM files \
             WHERE path IN ({}) AND status != 3",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::with_capacity(paths.len());
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 起動時差分走査 (§7.4) で使用。favorite_id スコープ内の (path, mtime, file_size) を返す。
    pub fn list_favorite_files(
        &self,
        favorite_id: Uuid,
    ) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, mtime, file_size FROM files \
             WHERE favorite_id = ?1 AND status != 3",
        )?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 起動時 reconciliation (§5.6.3) 用。status != 0 の行を全部取る。
    pub fn list_not_ok(&self) -> rusqlite::Result<Vec<FileMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, favorite_id, favorite_root, mtime, file_size,
                    indexed_at, index_version, index_generation, status, all_text_norm
             FROM files WHERE status != 0",
        )?;
        let rows = stmt.query_map([], row_to_filemeta)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// テスト / デバッグ用: 単一 path の行を取得。
    pub fn get(&self, path: &str) -> rusqlite::Result<Option<FileMeta>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT path, favorite_id, favorite_root, mtime, file_size,
                    indexed_at, index_version, index_generation, status, all_text_norm
             FROM files WHERE path = ?1",
            params![path],
            row_to_filemeta,
        )
        .optional()
    }

    /// favorite_id でフィルタしつつ総数と status 別の件数を返す。
    /// インデックス管理ダイアログの進捗表示 (§8.4) で使用。
    pub fn count_by_status(
        &self,
        favorite_id: Uuid,
    ) -> rusqlite::Result<StatusCounts> {
        let conn = self.conn.lock().unwrap();
        let mut counts = StatusCounts::default();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM files WHERE favorite_id = ?1 GROUP BY status",
        )?;
        let rows = stmt.query_map(params![favorite_id.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (s, c) = row?;
            match FileStatus::from_i64(s) {
                FileStatus::Ok => counts.ok = c as usize,
                FileStatus::Pending => counts.pending = c as usize,
                FileStatus::Failed => counts.failed = c as usize,
                FileStatus::Tombstone => counts.tombstone = c as usize,
            }
        }
        Ok(counts)
    }

    /// テスト用: ファイル名キー (normalize_path(&PathBuf) 結果) の key 生成ヘルパー。
    pub fn path_key(p: &Path) -> String {
        normalize_path(p)
    }
}

fn row_to_filemeta(row: &rusqlite::Row) -> rusqlite::Result<FileMeta> {
    let uuid_str: String = row.get(1)?;
    let favorite_id = Uuid::parse_str(&uuid_str)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    let favorite_root: String = row.get(2)?;
    let status_i: i64 = row.get(8)?;
    Ok(FileMeta {
        path: row.get(0)?,
        favorite_id,
        favorite_root: PathBuf::from(favorite_root),
        mtime: row.get(3)?,
        file_size: row.get(4)?,
        indexed_at: row.get(5)?,
        index_version: row.get(6)?,
        index_generation: row.get(7)?,
        status: FileStatus::from_i64(status_i),
        all_text_norm: row.get(9)?,
    })
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
            path              TEXT PRIMARY KEY,
            favorite_id       TEXT NOT NULL,
            favorite_root     TEXT NOT NULL,
            mtime             INTEGER NOT NULL,
            file_size         INTEGER NOT NULL,
            indexed_at        INTEGER NOT NULL,
            index_version     INTEGER NOT NULL,
            index_generation  INTEGER NOT NULL,
            status            INTEGER NOT NULL,
            all_text_norm     TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS idx_files_fav       ON files(favorite_id);
         CREATE INDEX IF NOT EXISTS idx_files_fav_mtime ON files(favorite_id, mtime);
         CREATE INDEX IF NOT EXISTS idx_files_status    ON files(status) WHERE status != 0;",
    )?;
    Ok(())
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// favorite 単位の status 別カウント (§8.4 の UI 表示で使用)。
#[derive(Default, Debug, Clone, Copy)]
pub struct StatusCounts {
    pub ok: usize,
    pub pending: usize,
    pub failed: usize,
    pub tombstone: usize,
}

impl StatusCounts {
    pub fn total(self) -> usize {
        self.ok + self.pending + self.failed + self.tombstone
    }
    /// インデックスされた有効件数 (検索結果に含まれうる)
    pub fn indexed(self) -> usize {
        self.ok
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_db() -> (TempDir, FtsMetaDb) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        (dir, db)
    }

    #[test]
    fn create_and_list_empty() {
        let (_tmp, db) = tmp_db();
        let id = Uuid::new_v4();
        assert!(db.list_favorite_files(id).unwrap().is_empty());
        assert!(db.list_not_ok().unwrap().is_empty());
    }

    #[test]
    fn pending_then_ok_roundtrip() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/photos");
        let gen1 = db
            .mark_pending("c:/photos/a.jpg", fav, &root, 100, 2048, "text a")
            .unwrap();
        assert_eq!(gen1, 1, "初回 ingest の generation は 1");

        // 状態: pending
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
        assert_eq!(got.all_text_norm, "text a");
        assert_eq!(got.index_generation, 1);
        assert_eq!(got.favorite_id, fav);

        // ok に遷移
        db.mark_ok(&["c:/photos/a.jpg".to_string()]).unwrap();
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Ok);

        // 同じ path で再 ingest → generation += 1
        let gen2 = db
            .mark_pending("c:/photos/a.jpg", fav, &root, 200, 2100, "text a updated")
            .unwrap();
        assert_eq!(gen2, 2);
        let got = db.get("c:/photos/a.jpg").unwrap().unwrap();
        assert_eq!(got.status, FileStatus::Pending);
        assert_eq!(got.all_text_norm, "text a updated");
    }

    #[test]
    fn tombstone_hides_from_lookup() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/p");
        db.mark_pending("c:/p/1.jpg", fav, &root, 1, 10, "one").unwrap();
        db.mark_pending("c:/p/2.jpg", fav, &root, 2, 20, "two").unwrap();
        db.mark_ok(&["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()])
            .unwrap();

        // lookup で両方返る
        let rows = db
            .lookup_all_text_norm(&["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()])
            .unwrap();
        assert_eq!(rows.len(), 2);

        // tombstone 後は除外される
        db.mark_tombstone(&["c:/p/1.jpg".to_string()]).unwrap();
        let rows = db
            .lookup_all_text_norm(&["c:/p/1.jpg".to_string(), "c:/p/2.jpg".to_string()])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "c:/p/2.jpg");

        // purge で物理削除
        let deleted = db.purge_tombstone(&["c:/p/1.jpg".to_string()]).unwrap();
        assert_eq!(deleted, 1);
        assert!(db.get("c:/p/1.jpg").unwrap().is_none());
    }

    #[test]
    fn list_favorite_files_scoped() {
        let (_tmp, db) = tmp_db();
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let root_a = PathBuf::from("C:/a");
        let root_b = PathBuf::from("C:/b");
        db.mark_pending("c:/a/1.jpg", fav_a, &root_a, 1, 1, "").unwrap();
        db.mark_pending("c:/a/2.jpg", fav_a, &root_a, 2, 2, "").unwrap();
        db.mark_pending("c:/b/1.jpg", fav_b, &root_b, 3, 3, "").unwrap();

        let a = db.list_favorite_files(fav_a).unwrap();
        assert_eq!(a.len(), 2);
        let b = db.list_favorite_files(fav_b).unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn list_not_ok_returns_pending_and_failed() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/x");
        db.mark_pending("c:/x/1.jpg", fav, &root, 1, 1, "").unwrap();
        db.mark_pending("c:/x/2.jpg", fav, &root, 2, 2, "").unwrap();
        db.mark_pending("c:/x/3.jpg", fav, &root, 3, 3, "").unwrap();

        db.mark_ok(&["c:/x/1.jpg".to_string()]).unwrap();
        db.mark_failed("c:/x/2.jpg").unwrap();
        // 3 は pending のまま

        let not_ok = db.list_not_ok().unwrap();
        assert_eq!(not_ok.len(), 2);
        let statuses: Vec<_> = not_ok.iter().map(|m| m.status).collect();
        assert!(statuses.contains(&FileStatus::Pending));
        assert!(statuses.contains(&FileStatus::Failed));
    }

    #[test]
    fn count_by_status_groups() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/y");
        for i in 0..5 {
            db.mark_pending(
                &format!("c:/y/{}.jpg", i),
                fav,
                &root,
                i,
                1,
                "",
            )
            .unwrap();
        }
        db.mark_ok(&["c:/y/0.jpg".to_string(), "c:/y/1.jpg".to_string()])
            .unwrap();
        db.mark_failed("c:/y/2.jpg").unwrap();
        db.mark_tombstone(&["c:/y/3.jpg".to_string()]).unwrap();
        // 4 は pending のまま

        let c = db.count_by_status(fav).unwrap();
        assert_eq!(c.ok, 2);
        assert_eq!(c.pending, 1);
        assert_eq!(c.failed, 1);
        assert_eq!(c.tombstone, 1);
        assert_eq!(c.total(), 5);
        assert_eq!(c.indexed(), 2);
    }

    #[test]
    fn lookup_returns_only_matching_paths() {
        let (_tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = PathBuf::from("C:/z");
        db.mark_pending("c:/z/a.jpg", fav, &root, 1, 1, "alpha").unwrap();
        db.mark_pending("c:/z/b.jpg", fav, &root, 2, 2, "beta").unwrap();
        db.mark_ok(&["c:/z/a.jpg".to_string(), "c:/z/b.jpg".to_string()])
            .unwrap();

        // 存在しない path を含むクエリでも、存在するものだけ返る
        let rows = db
            .lookup_all_text_norm(&[
                "c:/z/a.jpg".to_string(),
                "c:/z/missing.jpg".to_string(),
            ])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "c:/z/a.jpg");
        assert_eq!(rows[0].1, "alpha");
    }
}
