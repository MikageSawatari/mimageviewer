//! 動画音量ノーマライズの per-file 測定値キャッシュ。
//!
//! `%APPDATA%/mimageviewer/audio_normalize.db` に integrated LUFS / true peak / 算出ゲインを
//! ファイル単位で保存する。同じ動画を再オープンしたとき、グローバルノーマライズが
//! 有効ならスキャンを省略して即時適用するため。
//!
//! ## 主キー
//! `(path_lower, file_size, mtime_ms, target_lufs_milli)` の 4 列複合。
//! - `path_lower`: パス正規化 (大小文字統一 + スラッシュ統一、`adjustment_db::normalize_path` 流用)
//! - `file_size` + `mtime_ms`: 内容変化の検出 (mtime はミリ秒精度、同一秒更新の取りこぼし対策)
//! - `target_lufs_milli`: 整数 (例 -14000 = -14.000 LUFS)。float equality を避けるため整数化
//!
//! ## ON/OFF 状態は保存しない
//! 「グローバル ON/OFF」は `Settings::audio_normalize_enabled`、本 DB は測定値だけを持つ。
//! 動画再オープン時の動作は `Settings.audio_normalize_enabled && DB hit` でのみ自動適用、
//! それ以外は素通し (= UI ボタンが [OnUnmeasured] を出す)。

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
            )",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("audio_normalize.db")
    }

    /// 指定動画の測定値を引く。ファイルが存在しない / target が一致しない / 未測定なら None。
    pub fn lookup(&self, path: &Path, target_lufs_milli: i32) -> Option<NormalizeResult> {
        let (path_lower, file_size, mtime_ms) = file_key(path)?;
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT gain_db, integrated_lufs, true_peak_db
                   FROM audio_normalize
                  WHERE path_lower = ?1 AND file_size = ?2 AND mtime_ms = ?3
                    AND target_lufs_milli = ?4",
            )
            .ok()?;
        stmt.query_row(
            rusqlite::params![
                path_lower,
                file_size as i64,
                mtime_ms as i64,
                target_lufs_milli
            ],
            |row| {
                Ok(NormalizeResult {
                    gain_db: row.get::<_, f64>(0)? as f32,
                    integrated_lufs: row.get::<_, f64>(1)? as f32,
                    true_peak_db: row.get::<_, f64>(2)? as f32,
                    target_lufs_milli,
                })
            },
        )
        .ok()
    }

    /// 測定結果を保存 (既存があれば上書き)。
    pub fn upsert(&self, path: &Path, result: &NormalizeResult) -> Result<(), rusqlite::Error> {
        let Some((path_lower, file_size, mtime_ms)) = file_key(path) else {
            // ファイルが消えている等で metadata が取れない場合は単に保存しない (= エラーにしない)。
            return Ok(());
        };
        let scanned_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO audio_normalize
                (path_lower, file_size, mtime_ms, target_lufs_milli,
                 gain_db, integrated_lufs, true_peak_db, scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (path_lower, file_size, mtime_ms, target_lufs_milli)
             DO UPDATE SET
                gain_db = ?5,
                integrated_lufs = ?6,
                true_peak_db = ?7,
                scanned_at = ?8",
            rusqlite::params![
                path_lower,
                file_size as i64,
                mtime_ms as i64,
                result.target_lufs_milli,
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
        self.conn.execute("DELETE FROM audio_normalize", [])
    }

    /// 登録件数 (UI 表示用)。
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM audio_normalize", [], |row| row.get(0))
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
        assert!(db.lookup(p, -14000).is_none());
    }

    #[test]
    fn upsert_silently_skips_missing_file() {
        let (db, _dir) = temp_db();
        let p = Path::new("C:/this/path/should/not/exist.mp4");
        // metadata 取得失敗で何もしない (= panic / Err しない)。
        assert!(db.upsert(p, &sample_result(-14000)).is_ok());
    }

    #[test]
    fn upsert_lookup_roundtrip() {
        let (db, dir) = temp_db();
        let path = dir.path().join("dummy.mp4");
        std::fs::write(&path, b"dummy content").expect("write dummy");
        let result = sample_result(-14000);
        db.upsert(&path, &result).expect("upsert");
        let loaded = db.lookup(&path, -14000).expect("lookup");
        assert!((loaded.gain_db - result.gain_db).abs() < 1.0e-3);
        assert!((loaded.integrated_lufs - result.integrated_lufs).abs() < 1.0e-3);
        assert!((loaded.true_peak_db - result.true_peak_db).abs() < 1.0e-3);
        assert_eq!(loaded.target_lufs_milli, result.target_lufs_milli);
        // 異なる target なら別エントリ扱い (DB 主キーに含まれるため)
        assert!(db.lookup(&path, -16000).is_none());
    }

    #[test]
    fn clear_all_removes_cached_measurements() {
        let (db, dir) = temp_db();
        let first = dir.path().join("first.mp4");
        let second = dir.path().join("second.mp4");
        std::fs::write(&first, b"first").expect("write first");
        std::fs::write(&second, b"second").expect("write second");

        db.upsert(&first, &sample_result(-14000))
            .expect("upsert first");
        db.upsert(&second, &sample_result(-14000))
            .expect("upsert second");
        assert_eq!(db.count(), 2);

        assert_eq!(db.clear_all().expect("clear_all"), 2);
        assert_eq!(db.count(), 0);
        assert!(db.lookup(&first, -14000).is_none());
        assert!(db.lookup(&second, -14000).is_none());
    }
}
