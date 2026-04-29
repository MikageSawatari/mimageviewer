//! 動画タイル モード のサムネイル WebP 永続キャッシュ (Phase 6.D-2、Phase 8.C で
//! key を絶対 PTS 化)。
//!
//! `%APPDATA%/mimageviewer/video_tile_thumbs.db` に「(動画パス, タイル幅, 絶対 PTS
//! ms) → WebP バイト列」を保存する。タイルモードを 2 度目以降開いたとき、ffmpeg
//! seek + decode + swscale を省略して即座に表示できる。
//!
//! ## スキーマ (v2)
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS video_tile_thumbs (
//!     path         TEXT NOT NULL,
//!     tile_w       INTEGER NOT NULL,
//!     timestamp_ms INTEGER NOT NULL,    -- 絶対 PTS をミリ秒単位の整数キーで記録
//!     tile_h       INTEGER NOT NULL,
//!     webp         BLOB NOT NULL,
//!     video_mtime  INTEGER NOT NULL,
//!     PRIMARY KEY (path, tile_w, timestamp_ms)
//! );
//! CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
//!    ON video_tile_thumbs(path);
//! ```
//!
//! ## v1 → v2 マイグレーション
//!
//! 旧スキーマ (Phase 6.D-2) は `(interval_ms, slot)` をキーにしていたため、間隔
//! 5 秒 → 1 秒に切替えるとキャッシュが完全 miss になる問題があった。v2 では
//! 絶対 PTS をキーにし、interval が変わっても tile_w が同じなら共通 PTS のサムネを
//! 再利用できる。`init_schema` で旧テーブルを検出したら `interval_ms * slot` を
//! `timestamp_ms` に変換してマイグレートする。
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
        // 既存テーブルが v1 スキーマ (interval_ms + slot) なら v2 (timestamp_ms) に
        // マイグレートする。timestamp_ms = interval_ms * slot で算出可能 (両者とも
        // ミリ秒整数)。重複キー (path, tile_w, timestamp_ms) は INSERT OR IGNORE で
        // 1 件だけ残す (= v1 では同 path/tile_w で interval 違いの行が複数あった
        // ケース、内容は同じ pts を指すので 1 件あれば十分)。
        let is_v1 = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('video_tile_thumbs')
                 WHERE name = 'interval_ms' LIMIT 1",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if is_v1 {
            crate::logger::log(
                "video_tile_thumbs: migrating v1 (interval_ms+slot) → v2 (timestamp_ms)"
                    .to_string(),
            );
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE video_tile_thumbs_v2 (
                    path         TEXT NOT NULL,
                    tile_w       INTEGER NOT NULL,
                    timestamp_ms INTEGER NOT NULL,
                    tile_h       INTEGER NOT NULL,
                    webp         BLOB NOT NULL,
                    video_mtime  INTEGER NOT NULL,
                    PRIMARY KEY (path, tile_w, timestamp_ms)
                 );
                 INSERT OR IGNORE INTO video_tile_thumbs_v2
                    (path, tile_w, timestamp_ms, tile_h, webp, video_mtime)
                    SELECT path, tile_w, interval_ms * slot, tile_h, webp, video_mtime
                      FROM video_tile_thumbs;
                 DROP TABLE video_tile_thumbs;
                 ALTER TABLE video_tile_thumbs_v2 RENAME TO video_tile_thumbs;
                 CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
                    ON video_tile_thumbs(path);
                 COMMIT;",
            )?;
            return Ok(());
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_tile_thumbs (
                path         TEXT NOT NULL,
                tile_w       INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                tile_h       INTEGER NOT NULL,
                webp         BLOB NOT NULL,
                video_mtime  INTEGER NOT NULL,
                PRIMARY KEY (path, tile_w, timestamp_ms)
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
        tile_w: u32,
        timestamp_ms: i64,
        video_mtime: i64,
    ) -> Option<Vec<u8>> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self.conn.lock().ok()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT webp, video_mtime FROM video_tile_thumbs
                  WHERE path = ?1 AND tile_w = ?2 AND timestamp_ms = ?3",
            )
            .ok()?;
        let row: Option<(Vec<u8>, i64)> = stmt
            .query_row(
                rusqlite::params![key, tile_w, timestamp_ms],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
            )
            .ok();
        match row {
            Some((webp, mtime)) if mtime == video_mtime => Some(webp),
            Some(_) => {
                drop(stmt);
                // 古い mtime の行だけを削除する (同 path で別 mtime が混在しないよう)。
                let _ = conn.execute(
                    "DELETE FROM video_tile_thumbs WHERE path = ?1 AND video_mtime != ?2",
                    rusqlite::params![key, video_mtime],
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
        tile_w: u32,
        tile_h: u32,
        timestamp_ms: i64,
        webp: &[u8],
        video_mtime: i64,
    ) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO video_tile_thumbs
                (path, tile_w, timestamp_ms, tile_h, webp, video_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path, tile_w, timestamp_ms) DO UPDATE SET
                tile_h = ?4, webp = ?5, video_mtime = ?6",
            rusqlite::params![key, tile_w, timestamp_ms, tile_h, webp, video_mtime],
        )?;
        Ok(())
    }

    /// 動画 1 ファイル分のキャッシュを削除 (= 動画ファイル削除時 cleanup 用、未配線)。
    #[allow(dead_code)]
    pub fn clear_for(&self, video_path: &Path) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM video_tile_thumbs WHERE path = ?1",
            [&key],
        )?;
        Ok(())
    }
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
            .lookup_webp(Path::new("c:/none.mp4"), 320, 5000, 12345)
            .is_none());
    }

    #[test]
    fn store_then_lookup_roundtrip() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        let webp = vec![0xDE, 0xAD, 0xBE, 0xEF];
        db.store_webp(p, 320, 180, 5000, &webp, 99).unwrap();
        let got = db.lookup_webp(p, 320, 5000, 99).unwrap();
        assert_eq!(got, webp);
    }

    #[test]
    fn lookup_drops_stale_mtime() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 320, 180, 5000, &[1, 2, 3], 100).unwrap();
        // mtime 違い → None + 該当行削除
        assert!(db.lookup_webp(p, 320, 5000, 999).is_none());
        // 削除されたので再度 100 で照会しても見つからない
        assert!(db.lookup_webp(p, 320, 5000, 100).is_none());
    }

    #[test]
    fn store_overwrites_same_pk() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 320, 180, 5000, &[1], 100).unwrap();
        db.store_webp(p, 320, 180, 5000, &[2, 3], 100).unwrap();
        assert_eq!(db.lookup_webp(p, 320, 5000, 100).unwrap(), vec![2, 3]);
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.store_webp(Path::new("C:\\V.MP4"), 320, 180, 5000, &[9], 100)
            .unwrap();
        assert!(db
            .lookup_webp(Path::new("c:/v.mp4"), 320, 5000, 100)
            .is_some());
    }

    #[test]
    fn pts_keyed_lookup_reuses_across_intervals() {
        // Phase 8.C 動機ケース: 5 秒間隔で抽出した pts=5000ms のサムネが、
        // 1 秒間隔再描画時 (= pts=5000ms スロット) にヒットすることを確認。
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        // 5 秒間隔: pts=0ms, 5000ms, 10000ms, ...
        db.store_webp(p, 320, 180, 5000, &[5], 100).unwrap();
        db.store_webp(p, 320, 180, 10000, &[10], 100).unwrap();
        // 1 秒間隔再描画時: pts=5000ms, 10000ms はキャッシュヒット
        assert_eq!(db.lookup_webp(p, 320, 5000, 100).unwrap(), vec![5]);
        assert_eq!(db.lookup_webp(p, 320, 10000, 100).unwrap(), vec![10]);
        // 1 秒間隔の他スロット (pts=1000ms 等) は当然 miss
        assert!(db.lookup_webp(p, 320, 1000, 100).is_none());
    }

    #[test]
    fn migrate_v1_schema_to_v2() {
        // 旧 v1 スキーマに行を入れた後 init_schema が v2 にマイグレートする。
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(
            "CREATE TABLE video_tile_thumbs (
                path        TEXT NOT NULL,
                interval_ms INTEGER NOT NULL,
                tile_w      INTEGER NOT NULL,
                tile_h      INTEGER NOT NULL,
                slot        INTEGER NOT NULL,
                webp        BLOB NOT NULL,
                video_mtime INTEGER NOT NULL,
                PRIMARY KEY (path, interval_ms, tile_w, slot)
             );",
        )
        .unwrap();
        // 5 秒間隔の slot=2 (= pts 10000ms) を v1 で保存
        conn.execute(
            "INSERT INTO video_tile_thumbs
                (path, interval_ms, tile_w, tile_h, slot, webp, video_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["c:/v.mp4", 5000, 320, 180, 2, &[42u8] as &[u8], 100],
        )
        .unwrap();
        TileThumbCache::init_schema(&conn).expect("migrate");
        let db = TileThumbCache {
            conn: Mutex::new(conn),
        };
        // v2 lookup: pts=10000ms (= 5000 * 2) でヒットすること
        let got = db.lookup_webp(Path::new("c:/v.mp4"), 320, 10000, 100);
        assert_eq!(got.as_deref(), Some(&[42u8][..]));
    }
}
