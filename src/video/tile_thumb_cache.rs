//! 動画タイル モード のサムネイル WebP 永続キャッシュ (Phase 6.D-2)。
//!
//! `%APPDATA%/mimageviewer/video_tile_thumbs.db` に「(動画パス, 抽出間隔, タイル
//! サイズ, スロット番号) → WebP バイト列」を保存する。タイルモードを 2 度目以降
//! 開いたとき、ffmpeg seek + decode + swscale を省略して即座に表示できる。
//!
//! ## スキーマ
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS video_tile_thumbs (
//!     path        TEXT NOT NULL,
//!     interval_ms INTEGER NOT NULL,    -- 抽出間隔をミリ秒単位の整数キーで記録
//!     tile_w      INTEGER NOT NULL,
//!     tile_h      INTEGER NOT NULL,
//!     slot        INTEGER NOT NULL,    -- 0..N-1 (timestamp 配列のインデックス)
//!     webp        BLOB NOT NULL,
//!     video_mtime INTEGER NOT NULL,    -- 動画ファイルの mtime (UNIX 秒)、無効化判定用
//!     PRIMARY KEY (path, interval_ms, tile_w, slot)
//! );
//! CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
//!    ON video_tile_thumbs(path);
//! ```
//!
//! ## 無効化
//!
//! `lookup_webp` は与えられた `video_mtime` と DB 上の値を比較し、不一致なら
//! 「キャッシュ古い」と判定して `None` を返す + 該当行を削除する。動画ファイルが
//! 上書き / 再生成されたケースで古いサムネを引かないようにする。
//!
//! ## スレッドセーフティ
//!
//! `Connection` を `Mutex` で包み、複数スレッドから直列に呼べる。worker thread
//! と UI thread の双方が短いトランザクションで叩く想定。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct TileThumbCache {
    conn: Mutex<rusqlite::Connection>,
}

impl TileThumbCache {
    /// DB を開く (なければ作成)。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_tile_thumbs (
                path        TEXT NOT NULL,
                interval_ms INTEGER NOT NULL,
                tile_w      INTEGER NOT NULL,
                tile_h      INTEGER NOT NULL,
                slot        INTEGER NOT NULL,
                webp        BLOB NOT NULL,
                video_mtime INTEGER NOT NULL,
                PRIMARY KEY (path, interval_ms, tile_w, slot)
             );
             CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
                ON video_tile_thumbs(path);",
        )
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_tile_thumbs.db")
    }

    /// キャッシュ ヒットなら WebP バイト列を返す。`video_mtime` が DB と一致しない
    /// 行は古いとみなし削除して `None`。
    pub fn lookup_webp(
        &self,
        video_path: &Path,
        interval_ms: u32,
        tile_w: u32,
        slot: u32,
        video_mtime: i64,
    ) -> Option<Vec<u8>> {
        let key = normalize_path(video_path);
        let conn = self.conn.lock().ok()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT webp, video_mtime FROM video_tile_thumbs
                  WHERE path = ?1 AND interval_ms = ?2
                    AND tile_w = ?3 AND slot = ?4",
            )
            .ok()?;
        let row: Option<(Vec<u8>, i64)> = stmt
            .query_row(
                rusqlite::params![key, interval_ms, tile_w, slot],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
            )
            .ok();
        match row {
            Some((webp, mtime)) if mtime == video_mtime => Some(webp),
            Some(_) => {
                drop(stmt);
                // mtime 不一致 → 古いキャッシュなので削除して None
                let _ = conn.execute(
                    "DELETE FROM video_tile_thumbs WHERE path = ?1",
                    [&key],
                );
                None
            }
            None => None,
        }
    }

    /// 1 タイル分の WebP を保存。同 PRIMARY KEY なら ON CONFLICT で上書き。
    pub fn store_webp(
        &self,
        video_path: &Path,
        interval_ms: u32,
        tile_w: u32,
        tile_h: u32,
        slot: u32,
        webp: &[u8],
        video_mtime: i64,
    ) -> Result<(), rusqlite::Error> {
        let key = normalize_path(video_path);
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO video_tile_thumbs
                (path, interval_ms, tile_w, tile_h, slot, webp, video_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path, interval_ms, tile_w, slot) DO UPDATE SET
                tile_h = ?4, webp = ?6, video_mtime = ?7",
            rusqlite::params![key, interval_ms, tile_w, tile_h, slot, webp, video_mtime],
        )?;
        Ok(())
    }

    /// 動画 1 ファイル分のキャッシュを削除 (= 動画ファイル削除時 cleanup 用、
    /// Phase 6 では未配線)。
    #[allow(dead_code)]
    pub fn clear_for(&self, video_path: &Path) -> Result<(), rusqlite::Error> {
        let key = normalize_path(video_path);
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM video_tile_thumbs WHERE path = ?1",
            [&key],
        )?;
        Ok(())
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> TileThumbCache {
        let conn = Connection::open_in_memory().expect("memory db");
        TileThumbCache::init_schema(&conn).expect("schema");
        TileThumbCache {
            conn: Mutex::new(conn),
        }
    }

    #[test]
    fn lookup_missing_returns_none() {
        let db = open_in_memory();
        assert!(db
            .lookup_webp(Path::new("c:/none.mp4"), 1000, 320, 0, 12345)
            .is_none());
    }

    #[test]
    fn store_then_lookup_roundtrip() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        let webp = vec![0xDE, 0xAD, 0xBE, 0xEF];
        db.store_webp(p, 5000, 320, 180, 7, &webp, 99).unwrap();
        let got = db.lookup_webp(p, 5000, 320, 7, 99).unwrap();
        assert_eq!(got, webp);
    }

    #[test]
    fn lookup_drops_stale_mtime() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 5000, 320, 180, 0, &[1, 2, 3], 100).unwrap();
        // mtime 違い → None + 該当行削除
        assert!(db.lookup_webp(p, 5000, 320, 0, 999).is_none());
        // 削除されたので再度 100 で照会しても見つからない
        assert!(db.lookup_webp(p, 5000, 320, 0, 100).is_none());
    }

    #[test]
    fn store_overwrites_same_pk() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 5000, 320, 180, 0, &[1], 100).unwrap();
        db.store_webp(p, 5000, 320, 180, 0, &[2, 3], 100).unwrap();
        assert_eq!(db.lookup_webp(p, 5000, 320, 0, 100).unwrap(), vec![2, 3]);
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.store_webp(Path::new("C:\\V.MP4"), 5000, 320, 180, 0, &[9], 100)
            .unwrap();
        assert!(db.lookup_webp(Path::new("c:/v.mp4"), 5000, 320, 0, 100).is_some());
    }
}
