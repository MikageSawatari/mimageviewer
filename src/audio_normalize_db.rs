//! 動画音量ノーマライズの per-file 測定値キャッシュ。
//!
//! `%APPDATA%/mimageviewer/audio_normalize.db` に integrated LUFS / true peak / 算出ゲインを
//! 音声 stream 単位で保存する。同じ動画を再オープンしたとき、グローバルノーマライズが
//! 有効ならスキャンを省略して即時適用するため。
//!
//! ## 主キー
//! 新規保存先 `audio_normalize_track` は
//! `(path_lower, file_size, mtime_ms, target_lufs_milli, stream_index)` の 5 列複合。
//! 旧 `audio_normalize` の 4 列主キーは維持し、既定トラックの互換読みだけに使う。
//! - `path_lower`: パス正規化 (大小文字統一 + スラッシュ統一、`adjustment_db::normalize_path` 流用)
//! - `file_size` + `mtime_ms`: 内容変化の検出 (mtime はミリ秒精度、同一秒更新の取りこぼし対策)
//! - `target_lufs_milli`: 整数 (例 -14000 = -14.000 LUFS)。float equality を避けるため整数化
//!
//! ## ON/OFF 状態は保存しない
//! 「グローバル ON/OFF」は `Settings::audio_normalize_enabled`、本 DB は測定値だけを持つ。
//! 動画再オープン時、Norm ON なら開いたトラックの lookup 完了まで pump が raw frame を保持する。
//! 未測定なら scan 条件を評価してから unity gain に解決する。

use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};

use crate::video::normalize_types::NormalizeResult;

/// 音量ノーマライズ測定値 DB。
pub struct AudioNormalizeDb {
    conn: rusqlite::Connection,
}

impl AudioNormalizeDb {
    /// 既定のユーザー DB を開く (なければ作成)。`%APPDATA%/mimageviewer/audio_normalize.db`。
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 指定パスで DB を開く。テスト / 一時 DB 用。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS audio_normalize (
                path_lower         TEXT    NOT NULL,
                file_size          INTEGER NOT NULL,
                mtime_ms           INTEGER NOT NULL,
                target_lufs_milli  INTEGER NOT NULL,
                gain_db            REAL    NOT NULL,
                integrated_lufs    REAL    NOT NULL,
                true_peak_db       REAL    NOT NULL,
                scanned_at         INTEGER NOT NULL,
                PRIMARY KEY (path_lower, file_size, mtime_ms, target_lufs_milli)
            );
             CREATE TABLE IF NOT EXISTS audio_normalize_track (
                path_lower         TEXT    NOT NULL,
                file_size          INTEGER NOT NULL,
                mtime_ms           INTEGER NOT NULL,
                target_lufs_milli  INTEGER NOT NULL,
                stream_index       INTEGER NOT NULL,
                gain_db            REAL    NOT NULL,
                integrated_lufs    REAL    NOT NULL,
                true_peak_db       REAL    NOT NULL,
                scanned_at         INTEGER NOT NULL,
                PRIMARY KEY (path_lower, file_size, mtime_ms, target_lufs_milli, stream_index)
            )",
        )?;
        Ok(Self { conn })
    }

    /// Remote generation / App lookup workers use their own read-only connection.
    pub(crate) fn open_read_only_at(path: &Path) -> rusqlite::Result<Self> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        Ok(Self { conn })
    }

    pub(crate) fn db_path() -> PathBuf {
        crate::data_dir::get().join("audio_normalize.db")
    }

    /// 指定 stream の測定値を引く。旧版の行は open 時の既定 stream だけに適用する。
    pub fn lookup(
        &self,
        path: &Path,
        target_lufs_milli: i32,
        stream_index: usize,
        default_stream_index: Option<usize>,
    ) -> Option<NormalizeResult> {
        let (path_lower, file_size, mtime_ms) = file_key(path)?;
        self.lookup_key(
            &path_lower,
            file_size,
            mtime_ms,
            target_lufs_milli,
            stream_index,
            default_stream_index,
        )
        .ok()
        .flatten()
    }

    pub(crate) fn lookup_checked(
        &self,
        path: &Path,
        target_lufs_milli: i32,
        stream_index: usize,
        default_stream_index: Option<usize>,
    ) -> Result<Option<NormalizeResult>, String> {
        let (path_lower, file_size, mtime_ms) = file_key(path)
            .ok_or_else(|| format!("Norm source metadata unavailable: {}", path.display()))?;
        self.lookup_key(
            &path_lower,
            file_size,
            mtime_ms,
            target_lufs_milli,
            stream_index,
            default_stream_index,
        )
        .map_err(|error| error.to_string())
    }

    fn lookup_key(
        &self,
        path_lower: &str,
        file_size: u64,
        mtime_ms: u64,
        target_lufs_milli: i32,
        stream_index: usize,
        default_stream_index: Option<usize>,
    ) -> rusqlite::Result<Option<NormalizeResult>> {
        let read = |row: &rusqlite::Row<'_>| {
            Ok(NormalizeResult {
                gain_db: row.get::<_, f64>(0)? as f32,
                integrated_lufs: row.get::<_, f64>(1)? as f32,
                true_peak_db: row.get::<_, f64>(2)? as f32,
                target_lufs_milli,
            })
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT gain_db, integrated_lufs, true_peak_db FROM audio_normalize_track
             WHERE path_lower = ?1 AND file_size = ?2 AND mtime_ms = ?3
               AND target_lufs_milli = ?4 AND stream_index = ?5",
        )?;
        let found = stmt
            .query_row(
                rusqlite::params![
                    path_lower,
                    file_size as i64,
                    mtime_ms as i64,
                    target_lufs_milli,
                    stream_index as i64
                ],
                read,
            )
            .optional()?;
        if found.is_some() || default_stream_index != Some(stream_index) {
            return Ok(found);
        }
        // 旧版の best(Audio) が測った stream と open 時の既定 stream が同じという互換前提。
        self.conn
            .query_row(
                "SELECT gain_db, integrated_lufs, true_peak_db FROM audio_normalize
                 WHERE path_lower = ?1 AND file_size = ?2 AND mtime_ms = ?3
                   AND target_lufs_milli = ?4",
                rusqlite::params![
                    path_lower,
                    file_size as i64,
                    mtime_ms as i64,
                    target_lufs_milli
                ],
                read,
            )
            .optional()
    }

    /// 測定結果を保存 (既存があれば上書き)。
    pub fn upsert(
        &self,
        path: &Path,
        stream_index: usize,
        result: &NormalizeResult,
    ) -> Result<(), rusqlite::Error> {
        let Some((path_lower, file_size, mtime_ms)) = file_key(path) else {
            // ファイルが消えている等で metadata が取れない場合は単に保存しない (= エラーにしない)。
            return Ok(());
        };
        let scanned_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO audio_normalize_track
                (path_lower, file_size, mtime_ms, target_lufs_milli, stream_index,
                 gain_db, integrated_lufs, true_peak_db, scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (path_lower, file_size, mtime_ms, target_lufs_milli, stream_index)
             DO UPDATE SET
                gain_db = ?6,
                integrated_lufs = ?7,
                true_peak_db = ?8,
                scanned_at = ?9",
            rusqlite::params![
                path_lower,
                file_size as i64,
                mtime_ms as i64,
                result.target_lufs_milli,
                stream_index as i64,
                result.gain_db as f64,
                result.integrated_lufs as f64,
                result.true_peak_db as f64,
                scanned_at,
            ],
        )?;
        Ok(())
    }

    /// 全レコードを削除 (リセット用)。
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        Ok(self.conn.execute("DELETE FROM audio_normalize", [])?
            + self.conn.execute("DELETE FROM audio_normalize_track", [])?)
    }

    /// 登録件数 (UI 表示用)。
    pub fn count(&self) -> usize {
        self.conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM audio_normalize)
                        + (SELECT COUNT(*) FROM audio_normalize_track)",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
    }
}

/// ファイル単位の DB キー (path_lower, file_size, mtime_ms) を取得する。
/// ファイルが存在しない / metadata 取得失敗で None。
fn file_key(path: &Path) -> Option<(String, u64, u64)> {
    let path_lower = crate::adjustment_db::normalize_path(path);
    let meta = std::fs::metadata(path).ok()?;
    let file_size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some((path_lower, file_size, mtime_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(target_milli: i32) -> NormalizeResult {
        NormalizeResult {
            gain_db: -5.14,
            integrated_lufs: -8.86,
            true_peak_db: -0.42,
            target_lufs_milli: target_milli,
        }
    }

    /// テスト用に temp 内 DB を開く (実ユーザー DB を触らない、Codex P3 反映)。
    fn temp_db() -> (AudioNormalizeDb, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = AudioNormalizeDb::open_at(&dir.path().join("test.db")).expect("open_at");
        (db, dir)
    }

    #[test]
    fn lookup_returns_none_for_missing_file() {
        let (db, _dir) = temp_db();
        let p = Path::new("C:/this/path/should/not/exist.mp4");
        assert!(db.lookup(p, -14000, 1, Some(1)).is_none());
    }

    #[test]
    fn upsert_silently_skips_missing_file() {
        let (db, _dir) = temp_db();
        let p = Path::new("C:/this/path/should/not/exist.mp4");
        // metadata 取得失敗で何もしない (= panic / Err しない)。
        assert!(db.upsert(p, 1, &sample_result(-14000)).is_ok());
    }

    #[test]
    fn upsert_lookup_roundtrip() {
        let (db, dir) = temp_db();
        let path = dir.path().join("dummy.mp4");
        std::fs::write(&path, b"dummy content").expect("write dummy");
        let result = sample_result(-14000);
        db.upsert(&path, 1, &result).expect("upsert");
        let loaded = db.lookup(&path, -14000, 1, Some(1)).expect("lookup");
        assert!((loaded.gain_db - result.gain_db).abs() < 1.0e-3);
        assert!((loaded.integrated_lufs - result.integrated_lufs).abs() < 1.0e-3);
        assert!((loaded.true_peak_db - result.true_peak_db).abs() < 1.0e-3);
        assert_eq!(loaded.target_lufs_milli, result.target_lufs_milli);
        // 異なる target なら別エントリ扱い (DB 主キーに含まれるため)
        assert!(db.lookup(&path, -16000, 1, Some(1)).is_none());
    }

    #[test]
    fn clear_all_removes_cached_measurements() {
        let (db, dir) = temp_db();
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        std::fs::write(&first, b"first").expect("write first");
        std::fs::write(&second, b"second").expect("write second");

        db.upsert(&first, 1, &sample_result(-14000))
            .expect("upsert first");
        db.upsert(&second, 1, &sample_result(-14000))
            .expect("upsert second");
        assert_eq!(db.count(), 2);

        assert_eq!(db.clear_all().expect("clear_all"), 2);
        assert_eq!(db.count(), 0);
        assert!(db.lookup(&first, -14000, 1, Some(1)).is_none());
        assert!(db.lookup(&second, -14000, 1, Some(1)).is_none());
    }

    #[test]
    fn track_rows_override_legacy_and_legacy_is_default_only() {
        let (db, dir) = temp_db();
        let path = dir.path().join("multi.mkv");
        std::fs::write(&path, b"two audio streams").unwrap();
        let (path_lower, size, mtime) = file_key(&path).unwrap();
        db.conn
            .execute(
                "INSERT INTO audio_normalize
             (path_lower, file_size, mtime_ms, target_lufs_milli, gain_db,
              integrated_lufs, true_peak_db, scanned_at)
             VALUES (?1, ?2, ?3, -14000, 3.0, -17.0, -2.0, 0)",
                rusqlite::params![path_lower, size as i64, mtime as i64],
            )
            .unwrap();
        assert_eq!(db.lookup(&path, -14000, 1, Some(1)).unwrap().gain_db, 3.0);
        assert!(db.lookup(&path, -14000, 2, Some(1)).is_none());

        let mut second = sample_result(-14000);
        second.gain_db = -8.0;
        db.upsert(&path, 2, &second).unwrap();
        assert_eq!(db.lookup(&path, -14000, 2, Some(1)).unwrap().gain_db, -8.0);
        // Even default-track writes go to the new table; the old row remains unchanged.
        db.upsert(&path, 1, &sample_result(-14000)).unwrap();
        assert_eq!(db.lookup(&path, -14000, 1, Some(1)).unwrap().gain_db, -5.14);
        assert_eq!(
            db.conn
                .query_row("SELECT gain_db FROM audio_normalize", [], |row| row
                    .get::<_, f64>(0))
                .unwrap(),
            3.0
        );
        assert_eq!(db.count(), 3);
        assert_eq!(db.clear_all().unwrap(), 3);
        assert_eq!(db.count(), 0);
    }
}
