//! 動画タイル モードのサムネイル WebP と、動画シークストリップの粗い全尺波形列の
//! 永続キャッシュ (Phase 6.D-2、Phase 8.C でサムネイル key を絶対 PTS 化、D32 で
//! 粗い波形列を追加)。
//!
//! `%APPDATA%/mimageviewer/video_tile_thumbs.db` にサムネイルと粗い波形 chunk を保存する。
//! タイルモードや波形ストリップを 2 度目以降開いたとき、再度の decode を省略できる。
//!
//! ## スキーマ (v3)
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS video_tile_thumbs (
//!     path         TEXT NOT NULL,
//!     tile_w       INTEGER NOT NULL,
//!     timestamp_ms INTEGER NOT NULL,    -- 絶対 PTS をミリ秒単位の整数キーで記録
//!     tile_h       INTEGER NOT NULL,
//!     webp         BLOB NOT NULL,
//!     video_mtime  INTEGER NOT NULL,
//!     render_version INTEGER NOT NULL,
//!     PRIMARY KEY (path, tile_w, timestamp_ms)
//! );
//! CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
//!    ON video_tile_thumbs(path);
//!
//! CREATE TABLE IF NOT EXISTS video_resume_thumbs (
//!     path         TEXT NOT NULL PRIMARY KEY,
//!     tile_w       INTEGER NOT NULL,
//!     timestamp_ms INTEGER NOT NULL,
//!     tile_h       INTEGER NOT NULL,
//!     webp         BLOB NOT NULL,
//!     video_mtime  INTEGER NOT NULL,
//!     render_version INTEGER NOT NULL
//! );
//!
//! CREATE TABLE IF NOT EXISTS video_wave_chunks (
//!     path            TEXT NOT NULL,
//!     bin_secs_millis INTEGER NOT NULL,
//!     chunk_index     INTEGER NOT NULL,
//!     bins            BLOB NOT NULL,
//!     bin_count       INTEGER NOT NULL,
//!     video_mtime     INTEGER NOT NULL,
//!     video_size      INTEGER NOT NULL,
//!     format_version  INTEGER NOT NULL,
//!     PRIMARY KEY (path, bin_secs_millis, chunk_index)
//! );
//! CREATE INDEX IF NOT EXISTS idx_video_wave_chunks_path
//!    ON video_wave_chunks(path);
//! ```
//! `video_resume_thumbs` はホイール動画ナビゲーション中の静止画プレビュー用で、
//! 動画 1 本につき最新 resume 位置の 1 行だけを upsert する。`tile_w` は
//! 品質判定用の列で、主キーには含めない。同じ動画で resume 位置や抽出幅が
//! 変わっても、行が増殖せず上書きされるようにするため。
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

/// v2 rows contain encoded-orientation pixels. v3 materializes display matrix
/// orientation, so old rows must miss and be regenerated.
const THUMB_RENDER_VERSION: i64 = 2;

/// Increment when the seven-byte `QuantizedWaveformBin` byte layout changes.
const WAVE_COLUMN_FORMAT_VERSION: i64 = 1;

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
                    render_version INTEGER NOT NULL DEFAULT 1,
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
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS video_resume_thumbs (
                    path         TEXT NOT NULL,
                    tile_w       INTEGER NOT NULL,
                    timestamp_ms INTEGER NOT NULL,
                    tile_h       INTEGER NOT NULL,
                    webp         BLOB NOT NULL,
                    video_mtime  INTEGER NOT NULL,
                    render_version INTEGER NOT NULL DEFAULT 1,
                    PRIMARY KEY (path)
                 );
                 CREATE INDEX IF NOT EXISTS idx_video_resume_thumbs_path
                    ON video_resume_thumbs(path);",
            )?;
            Self::ensure_render_version_columns(conn)?;
            return Self::init_wave_schema(conn);
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_tile_thumbs (
                path         TEXT NOT NULL,
                tile_w       INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                tile_h       INTEGER NOT NULL,
                webp         BLOB NOT NULL,
                video_mtime  INTEGER NOT NULL,
                render_version INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (path, tile_w, timestamp_ms)
             );
             CREATE INDEX IF NOT EXISTS idx_video_tile_thumbs_path
                ON video_tile_thumbs(path);",
        )
        .and_then(|_| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS video_resume_thumbs (
                    path         TEXT NOT NULL,
                    tile_w       INTEGER NOT NULL,
                    timestamp_ms INTEGER NOT NULL,
                    tile_h       INTEGER NOT NULL,
                    webp         BLOB NOT NULL,
                    video_mtime  INTEGER NOT NULL,
                    render_version INTEGER NOT NULL DEFAULT 1,
                    PRIMARY KEY (path)
                 );
                 CREATE INDEX IF NOT EXISTS idx_video_resume_thumbs_path
                    ON video_resume_thumbs(path);",
            )
        })
        .and_then(|_| Self::ensure_render_version_columns(conn))
        .and_then(|_| Self::init_wave_schema(conn))
    }

    fn init_wave_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_wave_chunks (
                path            TEXT NOT NULL,
                bin_secs_millis INTEGER NOT NULL,
                chunk_index     INTEGER NOT NULL,
                bins            BLOB NOT NULL,
                bin_count       INTEGER NOT NULL,
                video_mtime     INTEGER NOT NULL,
                video_size      INTEGER NOT NULL,
                format_version  INTEGER NOT NULL,
                PRIMARY KEY (path, bin_secs_millis, chunk_index)
             );
             CREATE INDEX IF NOT EXISTS idx_video_wave_chunks_path
                ON video_wave_chunks(path);",
        )
    }

    fn ensure_render_version_columns(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        for table in ["video_tile_thumbs", "video_resume_thumbs"] {
            let sql = format!(
                "SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'render_version' LIMIT 1"
            );
            let exists = conn.query_row(&sql, [], |_| Ok(true)).unwrap_or(false);
            if !exists {
                conn.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN render_version INTEGER NOT NULL DEFAULT 1;"
                ))?;
            }
        }
        Ok(())
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_tile_thumbs.db")
    }

    /// キャッシュ ヒットなら WebP バイト列を返す。`video_mtime` が DB と一致しない
    /// 行は古いとみなし削除して `None`。
    ///
    /// `min_tile_w` 未満の幅で保存された行は無視する。抽出幅が
    /// `settings::VIDEO_TILE_EXTRACT_WIDTH` に固定される前は列数ごとに抽出幅が変動
    /// していたため、旧い狭い行 (10/16/20 列モード由来) がそのまま残っている。
    /// それを現在の固定幅モードで引くと拡大描画でぼやけるので、要求幅を満たさない
    /// 行は miss 扱いにして再抽出させる。`min_tile_w` 以上の行が複数あれば最大幅を
    /// 採用する (= 縮小描画の方がシャープ)。
    pub fn lookup_webp(
        &self,
        video_path: &Path,
        timestamp_ms: i64,
        video_mtime: i64,
        min_tile_w: u32,
    ) -> Option<Vec<u8>> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self.conn.lock().ok()?;
        Self::lookup_with_conn(&conn, &key, timestamp_ms, video_mtime, min_tile_w)
    }

    /// 複数の timestamp を 1 度の Mutex 取得で照会するバッチ版。タイル worker の
    /// 起動時 (~100 スロット) で per-slot lock を回避するため。
    /// 戻り値は入力順 (= スロット順) と一致する。
    pub fn lookup_webp_batch(
        &self,
        video_path: &Path,
        timestamps_ms: &[i64],
        video_mtime: i64,
        min_tile_w: u32,
    ) -> Vec<Option<Vec<u8>>> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let Ok(conn) = self.conn.lock() else {
            return vec![None; timestamps_ms.len()];
        };
        timestamps_ms
            .iter()
            .map(|&ts| Self::lookup_with_conn(&conn, &key, ts, video_mtime, min_tile_w))
            .collect()
    }

    /// Resume プレビュー用の「動画 1 本につき最新 1 枚」キャッシュを取得する。
    /// 戻り値は `(timestamp_ms, webp)`。呼び出し側が現在の resume 位置と timestamp を
    /// 照合し、ずれていれば black fallback にする。
    pub fn lookup_resume_webp(
        &self,
        video_path: &Path,
        video_mtime: i64,
        min_tile_w: u32,
    ) -> Option<(i64, Vec<u8>)> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self.conn.lock().ok()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT timestamp_ms, webp, video_mtime, tile_w FROM video_resume_thumbs
                  WHERE path = ?1 AND render_version = ?2
                  LIMIT 1",
            )
            .ok()?;
        let row: Option<(i64, Vec<u8>, i64, i64)> = stmt
            .query_row(rusqlite::params![&key, THUMB_RENDER_VERSION], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .ok();
        match row {
            Some((timestamp_ms, webp, mtime, tile_w))
                if mtime == video_mtime && tile_w >= i64::from(min_tile_w) =>
            {
                Some((timestamp_ms, webp))
            }
            Some((_timestamp_ms, _webp, mtime, tile_w)) => {
                drop(stmt);
                let _ = if mtime != video_mtime {
                    conn.execute(
                        "DELETE FROM video_resume_thumbs WHERE path = ?1 AND video_mtime != ?2",
                        rusqlite::params![key, video_mtime],
                    )
                } else if tile_w < i64::from(min_tile_w) {
                    conn.execute(
                        "DELETE FROM video_resume_thumbs WHERE path = ?1 AND tile_w < ?2",
                        rusqlite::params![key, min_tile_w],
                    )
                } else {
                    Ok(0)
                };
                None
            }
            None => None,
        }
    }

    fn lookup_with_conn(
        conn: &rusqlite::Connection,
        key: &str,
        timestamp_ms: i64,
        video_mtime: i64,
        min_tile_w: u32,
    ) -> Option<Vec<u8>> {
        // 同 (path, timestamp_ms) で複数 tile_w 行があり得るため、`min_tile_w` 以上で
        // 最大幅の 1 件を取る。要求幅以上なら描画時に egui が縮小スケールするだけで
        // シャープに出るが、要求幅未満の行を使うと拡大描画でぼやけるため除外する。
        let mut stmt = conn
            .prepare_cached(
                "SELECT webp, video_mtime FROM video_tile_thumbs
                  WHERE path = ?1 AND timestamp_ms = ?2 AND tile_w >= ?3
                    AND render_version = ?4
                  ORDER BY tile_w DESC LIMIT 1",
            )
            .ok()?;
        let row: Option<(Vec<u8>, i64)> = stmt
            .query_row(
                rusqlite::params![key, timestamp_ms, min_tile_w, THUMB_RENDER_VERSION],
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

    /// 粗い全尺波形列のうち、動画 identity と scale が一致する全 chunk を返す。
    /// 一致しない同一 path の行は古い列としてその場で削除する。
    pub fn lookup_wave_chunks(
        &self,
        video_path: &Path,
        video_mtime: i64,
        video_size: i64,
        bin_secs_millis: i64,
        bin_count: i64,
    ) -> Vec<(i64, Vec<u8>)> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };

        // 同じ path に古い動画 identity / scale / format の chunk を残すと、動画の
        // 差し替えや scale 変更のたびに DB が増え続ける。cache lookup は best-effort
        // なので削除失敗は miss/hit 判定を止めず、下の厳密一致 SELECT は必ず保つ。
        let _ = conn.execute(
            "DELETE FROM video_wave_chunks
             WHERE path = ?1 AND (
                bin_secs_millis != ?2 OR bin_count != ?3 OR
                video_mtime != ?4 OR video_size != ?5 OR format_version != ?6
             )",
            rusqlite::params![
                &key,
                bin_secs_millis,
                bin_count,
                video_mtime,
                video_size,
                WAVE_COLUMN_FORMAT_VERSION
            ],
        );

        let mut stmt = match conn.prepare_cached(
            "SELECT chunk_index, bins FROM video_wave_chunks
             WHERE path = ?1 AND bin_secs_millis = ?2 AND bin_count = ?3
               AND video_mtime = ?4 AND video_size = ?5 AND format_version = ?6
             ORDER BY chunk_index",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(
            rusqlite::params![
                &key,
                bin_secs_millis,
                bin_count,
                video_mtime,
                video_size,
                WAVE_COLUMN_FORMAT_VERSION
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        ) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.collect::<Result<Vec<_>, _>>().unwrap_or_default()
    }

    /// 1 タイル分の WebP を保存。同 PRIMARY KEY なら ON CONFLICT で上書き。
    /// 引数順は `lookup_webp` の prefix `(path, tile_w, timestamp_ms, video_mtime)`
    /// に揃え、payload (`tile_h`, `webp`) を末尾に置く。
    pub fn store_webp(
        &self,
        video_path: &Path,
        tile_w: u32,
        timestamp_ms: i64,
        video_mtime: i64,
        tile_h: u32,
        webp: &[u8],
    ) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO video_tile_thumbs
                (path, tile_w, timestamp_ms, tile_h, webp, video_mtime, render_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path, tile_w, timestamp_ms) DO UPDATE SET
                tile_h = ?4, webp = ?5, video_mtime = ?6, render_version = ?7",
            rusqlite::params![
                key,
                tile_w,
                timestamp_ms,
                tile_h,
                webp,
                video_mtime,
                THUMB_RENDER_VERSION
            ],
        )?;
        Ok(())
    }

    /// Resume プレビュー用の WebP を保存する。同じ `path` は常に上書きされる
    /// ため、再生を続けても動画 1 本あたり最新 1 行に保たれる。
    pub fn store_resume_webp(
        &self,
        video_path: &Path,
        tile_w: u32,
        timestamp_ms: i64,
        video_mtime: i64,
        tile_h: u32,
        webp: &[u8],
    ) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO video_resume_thumbs
                (path, tile_w, timestamp_ms, tile_h, webp, video_mtime, render_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                tile_w = ?2, timestamp_ms = ?3, tile_h = ?4, webp = ?5,
                video_mtime = ?6, render_version = ?7",
            rusqlite::params![
                key,
                tile_w,
                timestamp_ms,
                tile_h,
                webp,
                video_mtime,
                THUMB_RENDER_VERSION
            ],
        )?;
        Ok(())
    }

    /// 粗い全尺波形列の 1 chunk を保存する。同じ列・chunk は上書きする。
    pub fn store_wave_chunk(
        &self,
        video_path: &Path,
        video_mtime: i64,
        video_size: i64,
        bin_secs_millis: i64,
        bin_count: i64,
        chunk_index: i64,
        bins: &[u8],
    ) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO video_wave_chunks
                (path, bin_secs_millis, chunk_index, bins, bin_count,
                 video_mtime, video_size, format_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path, bin_secs_millis, chunk_index) DO UPDATE SET
                bins = ?4, bin_count = ?5, video_mtime = ?6, video_size = ?7,
                format_version = ?8",
            rusqlite::params![
                key,
                bin_secs_millis,
                chunk_index,
                bins,
                bin_count,
                video_mtime,
                video_size,
                WAVE_COLUMN_FORMAT_VERSION
            ],
        )?;
        Ok(())
    }

    /// 動画 1 ファイル分のキャッシュを削除 (= 動画ファイル削除時 cleanup 用、未配線)。
    #[allow(dead_code)]
    pub fn clear_for(&self, video_path: &Path) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM video_tile_thumbs WHERE path = ?1", [&key])?;
        conn.execute("DELETE FROM video_resume_thumbs WHERE path = ?1", [&key])?;
        conn.execute("DELETE FROM video_wave_chunks WHERE path = ?1", [&key])?;
        Ok(())
    }

    /// すべての行を削除し、空きページを `VACUUM` で実体解放する。
    /// 削除した行数を返す。「サムネイルキャッシュ管理」の「すべて削除」と同期し、
    /// ユーザーの体感としてディスク容量が実際に減るようにする (Codex P2)。
    /// `VACUUM` は worker thread 内で走るので UI スレッドはブロックしない。
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let removed = conn.execute("DELETE FROM video_tile_thumbs", [])?
            + conn.execute("DELETE FROM video_resume_thumbs", [])?
            + conn.execute("DELETE FROM video_wave_chunks", [])?;
        // VACUUM は autocommit 外で実行 (DELETE は execute() の単発なのでこの時点で
        // 既に commit 済み)。VACUUM 失敗は致命的ではないので log だけ残して継続。
        if let Err(e) = conn.execute_batch("VACUUM") {
            crate::logger::log(format!("TileThumbCache::clear_all: VACUUM failed: {e}"));
        }
        Ok(removed)
    }

    /// 指定フォルダ配下の動画パスに紐づく行を削除する (= 「現在のフォルダのキャッシュ
    /// を削除」と同期)。再帰的にサブフォルダも含む。削除した行数を返す。
    /// 削除した行が 1 つ以上ある場合は続けて `VACUUM` で実体解放する (Codex P2)。
    ///
    /// `folder` は `normalize_keep_drive` で正規化したうえで末尾 `/` を付与し、
    /// `substr(path, 1, length(?1)) = ?1` で前方一致削除する。`%` `_` を含む path も
    /// 安全 (= wildcard 評価を経ない)。
    pub fn clear_for_folder(&self, folder: &Path) -> Result<usize, rusqlite::Error> {
        let mut prefix = crate::path_key::normalize_keep_drive(folder);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let removed = conn.execute(
            "DELETE FROM video_tile_thumbs \
             WHERE substr(path, 1, length(?1)) = ?1",
            rusqlite::params![prefix],
        )? + conn.execute(
            "DELETE FROM video_resume_thumbs \
             WHERE substr(path, 1, length(?1)) = ?1",
            rusqlite::params![&prefix],
        )? + conn.execute(
            "DELETE FROM video_wave_chunks \
             WHERE substr(path, 1, length(?1)) = ?1",
            rusqlite::params![prefix],
        )?;
        if removed > 0 {
            if let Err(e) = conn.execute_batch("VACUUM") {
                crate::logger::log(format!(
                    "TileThumbCache::clear_for_folder: VACUUM failed: {e}"
                ));
            }
        }
        Ok(removed)
    }

    /// スキーマだけ本物の、ディスクに触らない DB。**テスト専用。**
    ///
    /// `init_schema` を通すので、テーブルを足したときに呼び出し側のテストが
    /// 勝手に古い形の DB を組み立てて通ってしまうことがない。
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> TileThumbCache {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        Self::init_schema(&conn).expect("schema");
        TileThumbCache {
            conn: Mutex::new(conn),
        }
    }

    /// DB 本体 + WAL + SHM の合計バイト数 (キャッシュ管理ダイアログの表示用)。
    /// 取得失敗時は 0 を返す (= 表示でだけ使うので失敗を panic にしない)。
    pub fn db_size_bytes() -> u64 {
        let db = Self::db_path();
        let wal = db.with_extension("db-wal");
        let shm = db.with_extension("db-shm");
        let one = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        one(&db) + one(&wal) + one(&shm)
    }

    /// open 失敗時の fallback 用に、DB / WAL / SHM のファイルを物理削除する。
    /// 削除に成功したファイル数を返す (0〜3)。`open()` が走らない経路でも呼べる
    /// よう静的メソッドにしている (Codex P2)。`TileThumbCache` インスタンスを
    /// 持っているなら通常は `clear_all()` を使う。
    ///
    /// ## 注意
    /// 同時に `Connection` を握っているインスタンスが存在するときに呼ぶと SQLite が
    /// 一貫性を失う。**Arc が dropped されているか、そもそも open に失敗していて
    /// インスタンスが存在しないとき限定**で使う。
    pub fn erase_db_files() -> usize {
        let db = Self::db_path();
        let wal = db.with_extension("db-wal");
        let shm = db.with_extension("db-shm");
        let mut removed = 0usize;
        for p in [&db, &wal, &shm] {
            if std::fs::remove_file(p).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> TileThumbCache {
        TileThumbCache::open_in_memory()
    }

    fn wave_row_count(db: &TileThumbCache) -> i64 {
        db.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM video_wave_chunks", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn lookup_missing_returns_none() {
        let db = open_in_memory();
        assert!(
            db.lookup_webp(Path::new("c:/none.mp4"), 5000, 12345, 320)
                .is_none()
        );
    }

    #[test]
    fn store_then_lookup_roundtrip() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        let webp = vec![0xDE, 0xAD, 0xBE, 0xEF];
        db.store_webp(p, 320, 5000, 99, 180, &webp).unwrap();
        let got = db.lookup_webp(p, 5000, 99, 320).unwrap();
        assert_eq!(got, webp);
    }

    #[test]
    fn wave_store_then_lookup_roundtrip() {
        let db = open_in_memory();
        let stored_path = Path::new(r"C:\Videos\V.MP4");
        db.store_wave_chunk(stored_path, 100, 4_096, 100, 1_200, 1, &[7, 8, 9])
            .unwrap();
        db.store_wave_chunk(stored_path, 100, 4_096, 100, 1_200, 0, &[1, 2, 3])
            .unwrap();

        let got = db.lookup_wave_chunks(Path::new("c:/videos/v.mp4"), 100, 4_096, 100, 1_200);
        assert_eq!(got, vec![(0, vec![1, 2, 3]), (1, vec![7, 8, 9])]);
    }

    #[test]
    fn lookup_drops_stale_mtime() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 320, 5000, 100, 180, &[1, 2, 3]).unwrap();
        // mtime 違い → None + 該当行削除
        assert!(db.lookup_webp(p, 5000, 999, 320).is_none());
        // 削除されたので再度 100 で照会しても見つからない
        assert!(db.lookup_webp(p, 5000, 100, 320).is_none());
    }

    #[test]
    fn wave_lookup_mtime_mismatch_deletes_stale_rows() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_wave_chunk(p, 100, 4_096, 100, 600, 0, &[1, 2, 3])
            .unwrap();

        assert!(db.lookup_wave_chunks(p, 999, 4_096, 100, 600).is_empty());
        assert!(
            db.lookup_wave_chunks(p, 100, 4_096, 100, 600).is_empty(),
            "the stale row must be deleted, not merely filtered"
        );
        assert_eq!(wave_row_count(&db), 0);
    }

    #[test]
    fn wave_lookup_size_mismatch_misses_and_deletes() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_wave_chunk(p, 100, 4_096, 100, 600, 0, &[1, 2, 3])
            .unwrap();

        assert!(db.lookup_wave_chunks(p, 100, 8_192, 100, 600).is_empty());
        assert_eq!(wave_row_count(&db), 0);
    }

    #[test]
    fn wave_lookup_bin_secs_mismatch_misses_and_deletes() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_wave_chunk(p, 100, 4_096, 100, 600, 0, &[1, 2, 3])
            .unwrap();

        assert!(db.lookup_wave_chunks(p, 100, 4_096, 200, 600).is_empty());
        assert_eq!(wave_row_count(&db), 0);
    }

    #[test]
    fn wave_lookup_bin_count_mismatch_misses_and_deletes() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_wave_chunk(p, 100, 4_096, 100, 600, 0, &[1, 2, 3])
            .unwrap();

        assert!(db.lookup_wave_chunks(p, 100, 4_096, 100, 601).is_empty());
        assert_eq!(wave_row_count(&db), 0);
    }

    #[test]
    fn wave_lookup_format_version_mismatch_misses_and_deletes() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_wave_chunk(p, 100, 4_096, 100, 600, 0, &[1, 2, 3])
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            let changed = conn
                .execute(
                    "UPDATE video_wave_chunks SET format_version = ?1",
                    [WAVE_COLUMN_FORMAT_VERSION + 1],
                )
                .unwrap();
            assert_eq!(changed, 1);
        }

        assert!(db.lookup_wave_chunks(p, 100, 4_096, 100, 600).is_empty());
        assert_eq!(wave_row_count(&db), 0);
    }

    #[test]
    fn store_overwrites_same_pk() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 320, 5000, 100, 180, &[1]).unwrap();
        db.store_webp(p, 320, 5000, 100, 180, &[2, 3]).unwrap();
        assert_eq!(db.lookup_webp(p, 5000, 100, 320).unwrap(), vec![2, 3]);
    }

    #[test]
    fn resume_store_keeps_one_row_per_path() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_resume_webp(p, 1280, 5000, 100, 720, &[1]).unwrap();
        db.store_resume_webp(p, 1280, 9000, 100, 720, &[2, 3])
            .unwrap();
        let (timestamp_ms, webp) = db.lookup_resume_webp(p, 100, 1280).unwrap();
        assert_eq!(timestamp_ms, 9000);
        assert_eq!(webp, vec![2, 3]);
        let row_count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM video_resume_thumbs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 1);
    }

    #[test]
    fn resume_lookup_rejects_narrow_or_stale_rows() {
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_resume_webp(p, 640, 5000, 100, 360, &[1]).unwrap();
        assert!(db.lookup_resume_webp(p, 100, 1280).is_none());
        let row_count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM video_resume_thumbs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0);

        db.store_resume_webp(p, 1280, 5000, 100, 720, &[1]).unwrap();
        assert!(db.lookup_resume_webp(p, 999, 640).is_none());
        let row_count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM video_resume_thumbs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 0);

        assert!(db.lookup_resume_webp(p, 100, 640).is_none());
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.store_webp(Path::new("C:\\V.MP4"), 320, 5000, 100, 180, &[9])
            .unwrap();
        assert!(
            db.lookup_webp(Path::new("c:/v.mp4"), 5000, 100, 320)
                .is_some()
        );
    }

    #[test]
    fn pts_keyed_lookup_reuses_across_intervals() {
        // Phase 8.C 動機ケース: 5 秒間隔で抽出した pts=5000ms のサムネが、
        // 1 秒間隔再描画時 (= pts=5000ms スロット) にヒットすることを確認。
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        db.store_webp(p, 320, 5000, 100, 180, &[5]).unwrap();
        db.store_webp(p, 320, 10000, 100, 180, &[10]).unwrap();
        assert_eq!(db.lookup_webp(p, 5000, 100, 320).unwrap(), vec![5]);
        assert_eq!(db.lookup_webp(p, 10000, 100, 320).unwrap(), vec![10]);
        assert!(db.lookup_webp(p, 1000, 100, 320).is_none());
    }

    #[test]
    fn lookup_rejects_rows_narrower_than_min() {
        // 抽出幅固定化 (VIDEO_TILE_EXTRACT_WIDTH) 後の回帰防止: 旧い狭い tile_w 行を
        // 現在の固定抽出幅で引くと拡大描画でぼやけるため、min_tile_w 未満の行は
        // miss 扱いにして再抽出させる。
        let db = open_in_memory();
        let p = Path::new("c:/v.mp4");
        // 旧 10/16/20 列モード相当の狭いサムネ
        db.store_webp(p, 200, 5000, 100, 112, &[7]).unwrap();
        // 現在の固定抽出幅 (640) で要求 → 狭すぎるので miss
        assert!(db.lookup_webp(p, 5000, 100, 640).is_none());
        // 要求幅が保存幅以下なら従来どおりヒット
        assert_eq!(db.lookup_webp(p, 5000, 100, 200).unwrap(), vec![7]);
        assert_eq!(db.lookup_webp(p, 5000, 100, 100).unwrap(), vec![7]);
        // 固定抽出幅で抽出し直した行が入れば、要求幅以上の中から最大幅を採用する
        db.store_webp(p, 640, 5000, 100, 360, &[42]).unwrap();
        assert_eq!(db.lookup_webp(p, 5000, 100, 640).unwrap(), vec![42]);
    }

    #[test]
    fn clear_for_removes_wave_chunks_for_only_that_path() {
        let db = open_in_memory();
        let target = Path::new(r"C:\Movies\Target.mp4");
        let other = Path::new("c:/movies/other.mp4");
        db.store_wave_chunk(target, 100, 4_096, 100, 600, 0, &[1])
            .unwrap();
        db.store_wave_chunk(other, 100, 4_096, 100, 600, 0, &[2])
            .unwrap();

        db.clear_for(Path::new("c:/movies/target.mp4")).unwrap();

        assert!(
            db.lookup_wave_chunks(target, 100, 4_096, 100, 600)
                .is_empty()
        );
        assert_eq!(
            db.lookup_wave_chunks(other, 100, 4_096, 100, 600),
            vec![(0, vec![2])]
        );
    }

    #[test]
    fn clear_all_counts_and_removes_wave_chunks() {
        let db = open_in_memory();
        let p = Path::new("c:/wave.mp4");
        db.store_webp(p, 320, 5_000, 100, 180, &[9]).unwrap();
        db.store_wave_chunk(p, 100, 4_096, 100, 1_200, 0, &[1])
            .unwrap();
        db.store_wave_chunk(p, 100, 4_096, 100, 1_200, 1, &[2])
            .unwrap();

        let removed = db.clear_all().unwrap();
        assert_eq!(removed, 3);
        assert!(db.lookup_webp(p, 5_000, 100, 320).is_none());
        assert!(db.lookup_wave_chunks(p, 100, 4_096, 100, 1_200).is_empty());

        db.store_wave_chunk(p, 100, 4_096, 100, 1_200, 0, &[3])
            .unwrap();
        assert_eq!(
            db.lookup_wave_chunks(p, 100, 4_096, 100, 1_200),
            vec![(0, vec![3])]
        );
    }

    #[test]
    fn clear_all_runs_vacuum_shrinks_file() {
        // P2 (Codex) regression: `DELETE` だけだと SQLite はフリーページを再利用するだけで
        // ファイルサイズが縮まないので、ユーザーが「全削除」しても `video_tile_thumbs.db`
        // のディスク使用量が残る。`clear_all()` は VACUUM を行うべき。
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("video_tile_thumbs.db");
        let conn = Connection::open(&db_path).expect("open file db");
        TileThumbCache::init_schema(&conn).expect("schema");
        let db = TileThumbCache {
            conn: Mutex::new(conn),
        };
        // 200 行 × 2KB BLOB を投入してファイルを成長させる
        for i in 0..200u32 {
            let blob: Vec<u8> = (0..2048u32).map(|j| ((i * 31 + j) % 256) as u8).collect();
            db.store_webp(
                Path::new(&format!("c:/grow/v_{i}.mp4")),
                320,
                5000,
                100,
                180,
                &blob,
            )
            .expect("store");
        }
        // チェックポイント無しでも file は伸びている (rollback journal mode)
        let size_before = std::fs::metadata(&db_path).expect("meta").len();
        assert!(
            size_before > 200_000,
            "DB should have grown above 200KB after seeding (got {} bytes)",
            size_before
        );

        let removed = db.clear_all().expect("clear_all");
        assert_eq!(removed, 200);

        let size_after = std::fs::metadata(&db_path).expect("meta after").len();
        assert!(
            size_after < size_before / 2,
            "VACUUM should have shrunk the DB: before={} bytes, after={} bytes",
            size_before,
            size_after
        );
    }

    #[test]
    fn clear_for_folder_runs_vacuum_when_rows_deleted() {
        // VACUUM が走るのは「1 行以上削除した」場合のみ (= 0 行で VACUUM すると
        // 無意味な I/O が走る)。本テストは「削除した場合に size が縮む」ことで
        // VACUUM パスが踏まれたことを間接的に検証する。
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("video_tile_thumbs.db");
        let conn = Connection::open(&db_path).expect("open file db");
        TileThumbCache::init_schema(&conn).expect("schema");
        let db = TileThumbCache {
            conn: Mutex::new(conn),
        };
        for i in 0..150u32 {
            let blob: Vec<u8> = (0..2048u32).map(|j| ((i * 17 + j) % 256) as u8).collect();
            db.store_webp(
                Path::new(&format!("c:/movies/v_{i}.mp4")),
                320,
                5000,
                100,
                180,
                &blob,
            )
            .expect("store");
        }
        let size_before = std::fs::metadata(&db_path).expect("meta").len();
        assert!(size_before > 150_000);

        // 全部 c:/movies 配下なので 150 行消える
        let removed = db
            .clear_for_folder(Path::new("c:/movies"))
            .expect("clear_for_folder");
        assert_eq!(removed, 150);

        let size_after = std::fs::metadata(&db_path).expect("meta after").len();
        assert!(
            size_after < size_before / 2,
            "VACUUM should have shrunk: before={}, after={}",
            size_before,
            size_after
        );
    }

    #[test]
    fn clear_for_folder_no_rows_does_not_error() {
        // rows = 0 のとき VACUUM は skip される。ここでは error 無しで完走することを
        // 確認する (内部実装が if removed > 0 で gating している前提)。
        let db = open_in_memory();
        db.store_webp(Path::new("c:/other/x.mp4"), 320, 5000, 100, 180, &[1])
            .unwrap();
        let removed = db.clear_for_folder(Path::new("c:/empty_folder")).unwrap();
        assert_eq!(removed, 0);
        // 他フォルダの行は残っている
        assert!(
            db.lookup_webp(Path::new("c:/other/x.mp4"), 5000, 100, 320)
                .is_some()
        );
    }

    #[test]
    fn clear_all_wipes_table_keeps_schema() {
        let db = open_in_memory();
        let p1 = Path::new("c:/v1.mp4");
        let p2 = Path::new("d:/dir/v2.mp4");
        db.store_webp(p1, 320, 5000, 100, 180, &[1]).unwrap();
        db.store_webp(p2, 320, 5000, 100, 180, &[2]).unwrap();
        // 2 行ある状態から clear_all → 2 件削除
        let removed = db.clear_all().unwrap();
        assert_eq!(removed, 2);
        // 削除後も schema は残っているので新規 store/lookup が動く
        db.store_webp(p1, 320, 5000, 100, 180, &[3]).unwrap();
        assert_eq!(db.lookup_webp(p1, 5000, 100, 320).unwrap(), vec![3]);
        // p2 は消えたまま
        assert!(db.lookup_webp(p2, 5000, 100, 320).is_none());
    }

    #[test]
    fn clear_for_folder_removes_wave_chunks_with_normalized_prefix() {
        let db = open_in_memory();
        let target = Path::new(r"C:\Movies\Sub\A.mp4");
        let similar = Path::new("c:/movies_backup/b.mp4");
        db.store_wave_chunk(target, 100, 4_096, 100, 1_200, 0, &[1])
            .unwrap();
        db.store_wave_chunk(target, 100, 4_096, 100, 1_200, 1, &[2])
            .unwrap();
        db.store_wave_chunk(similar, 100, 4_096, 100, 1_200, 0, &[3])
            .unwrap();

        let removed = db.clear_for_folder(Path::new("c:/MOVIES")).unwrap();
        assert_eq!(removed, 2);
        assert!(
            db.lookup_wave_chunks(target, 100, 4_096, 100, 1_200)
                .is_empty()
        );
        assert_eq!(
            db.lookup_wave_chunks(similar, 100, 4_096, 100, 1_200),
            vec![(0, vec![3])]
        );
    }

    #[test]
    fn clear_for_folder_recursive_prefix() {
        let db = open_in_memory();
        // 対象フォルダ配下: c:/movies/  に 2 動画 (直下 + サブフォルダ)
        db.store_webp(Path::new("c:/movies/a.mp4"), 320, 5000, 100, 180, &[1])
            .unwrap();
        db.store_webp(Path::new("c:/movies/sub/b.mp4"), 320, 5000, 100, 180, &[2])
            .unwrap();
        // 別フォルダ
        db.store_webp(Path::new("c:/other/c.mp4"), 320, 5000, 100, 180, &[3])
            .unwrap();
        // 紛らわしい類似名 (movies に prefix 一致しないことを確認)
        db.store_webp(
            Path::new("c:/movies_backup/d.mp4"),
            320,
            5000,
            100,
            180,
            &[4],
        )
        .unwrap();

        let removed = db.clear_for_folder(Path::new("c:/movies")).unwrap();
        assert_eq!(removed, 2, "movies 配下 2 件のみ削除されるべき");

        // 配下 2 件は消えた
        assert!(
            db.lookup_webp(Path::new("c:/movies/a.mp4"), 5000, 100, 320)
                .is_none()
        );
        assert!(
            db.lookup_webp(Path::new("c:/movies/sub/b.mp4"), 5000, 100, 320)
                .is_none()
        );
        // 別フォルダと類似名は残っている
        assert!(
            db.lookup_webp(Path::new("c:/other/c.mp4"), 5000, 100, 320)
                .is_some()
        );
        assert!(
            db.lookup_webp(Path::new("c:/movies_backup/d.mp4"), 5000, 100, 320)
                .is_some()
        );
    }

    #[test]
    fn clear_for_folder_handles_trailing_slash_and_case() {
        let db = open_in_memory();
        db.store_webp(Path::new("C:\\Movies\\A.mp4"), 320, 5000, 100, 180, &[1])
            .unwrap();
        // 末尾スラッシュあり + 大文字 + バックスラッシュ混じり でも 1 件消える
        let removed = db.clear_for_folder(Path::new("C:\\Movies\\")).unwrap();
        assert_eq!(removed, 1);
    }

    #[test]
    fn clear_for_folder_with_wildcard_chars_safe() {
        let db = open_in_memory();
        // `%` や `_` を含む path も誤削除しない (= LIKE を使っていないことの保証)。
        db.store_webp(
            Path::new("c:/movies/foo%bar.mp4"),
            320,
            5000,
            100,
            180,
            &[1],
        )
        .unwrap();
        db.store_webp(
            Path::new("c:/movies/foo_baz.mp4"),
            320,
            5000,
            100,
            180,
            &[2],
        )
        .unwrap();
        db.store_webp(Path::new("c:/other/x.mp4"), 320, 5000, 100, 180, &[3])
            .unwrap();

        let removed = db.clear_for_folder(Path::new("c:/movies")).unwrap();
        assert_eq!(removed, 2);
        assert!(
            db.lookup_webp(Path::new("c:/other/x.mp4"), 5000, 100, 320)
                .is_some()
        );
    }

    #[test]
    fn migrate_v1_schema_marks_existing_rows_as_stale() {
        // 旧スキーマの画像は回転未反映なので、移行後の lookup から除外する。
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
        let got = db.lookup_webp(Path::new("c:/v.mp4"), 10000, 100, 320);
        assert!(got.is_none());

        db.store_webp(Path::new("c:/v.mp4"), 320, 10000, 100, 180, &[43])
            .unwrap();
        let got = db.lookup_webp(Path::new("c:/v.mp4"), 10000, 100, 320);
        assert_eq!(got.as_deref(), Some(&[43u8][..]));

        // v1 migration の早期 return 経路でも D32 の新規 table が初期化される。
        db.store_wave_chunk(Path::new("c:/v.mp4"), 100, 4_096, 100, 600, 0, &[7])
            .unwrap();
        assert_eq!(
            db.lookup_wave_chunks(Path::new("c:/v.mp4"), 100, 4_096, 100, 600),
            vec![(0, vec![7])]
        );
    }
}
