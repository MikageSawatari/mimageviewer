//! 音声解析結果 (`TimelineAnalysis`) の永続キャッシュ。
//!
//! `<data_dir>/audio_analysis.db` に「1 音声ファイル = 解析結果 JSON」を保存する。
//! キー = 実ファイルパス。キャッシュの有効性は `size` + `mtime` + `analysis_version` で
//! 判定し、いずれかがずれていれば stale とみなしてワーカーで再解析する
//! (docs/music-integration-plan.md §6, D8)。
//!
//! `TimelineAnalysis` は `BeatGrid` を内包するので 1 行で完結する。ブックマークは動画機構
//! (`video_bookmarks`) を再利用するのでこの DB には含めない (D5.1)。
//!
//! 新規機能なので旧 mIV データからの移行は不要 (D11)。no-row / 壊れ JSON / stale は
//! すべて `None`（= 要再解析）で、クラッシュさせない。

use std::path::{Path, PathBuf};

use music_core::{TIMELINE_ANALYSIS_VERSION, TimelineAnalysis};

/// テーブルスキーマの版 (`PRAGMA user_version`)。スキーマ自体を変えたら +1 する。
pub const SCHEMA_VERSION: i64 = 1;

/// 音声解析結果の永続キャッシュ DB。内部は SQLite `audio_analysis` テーブル。
pub struct AudioAnalysisDb {
    conn: rusqlite::Connection,
}

impl AudioAnalysisDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト用。
    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS audio_analysis (
                path             TEXT    PRIMARY KEY,
                size             INTEGER NOT NULL,
                mtime            INTEGER NOT NULL,
                analysis_version INTEGER NOT NULL,
                doc_json         TEXT    NOT NULL
            )",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("audio_analysis.db")
    }

    /// キャッシュされた解析結果を返す。`size` / `mtime` / `analysis_version` が現在の
    /// ファイルと一致する行だけを有効とみなす。不一致・no-row・壊れ JSON は `None`
    /// (= 要再解析)。
    ///
    /// 呼び出し側は set/get で **同じソースの `size` / `mtime`** を渡すこと (通常は
    /// `folder_scan` と同じ秒精度 `mtime_secs` + ファイルサイズ)。mtime が秒精度なので、
    /// 「同一秒内に同一サイズで上書き」された場合は false hit しうるが、これは catalog /
    /// thumbnail など既存キャッシュと同水準の許容範囲 (Codex P3、恒久的にレアケース)。
    pub fn get(&self, path: &str, size: i64, mtime: i64) -> Option<TimelineAnalysis> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT doc_json FROM audio_analysis
                 WHERE path = ?1 AND size = ?2 AND mtime = ?3 AND analysis_version = ?4",
            )
            .ok()?;
        let json: String = stmt
            .query_row(
                rusqlite::params![path, size, mtime, TIMELINE_ANALYSIS_VERSION as i64],
                |row| row.get::<_, String>(0),
            )
            .ok()?;
        match serde_json::from_str::<TimelineAnalysis>(&json) {
            // 二重ガード: 列の analysis_version が一致しても、JSON 内 version も現行と
            // 一致することを確認する (直列化フォーマットの取り違え防止)。
            Ok(ta) if ta.analysis_version == TIMELINE_ANALYSIS_VERSION => Some(ta),
            Ok(_) => None,
            Err(e) => {
                crate::logger::log(format!(
                    "[audio] audio_analysis.db doc parse failed path={path}: {e}"
                ));
                None
            }
        }
    }

    /// 解析結果を保存する (upsert)。`size` / `mtime` は解析対象ファイルの実測値。
    pub fn set(
        &self,
        path: &str,
        size: i64,
        mtime: i64,
        analysis: &TimelineAnalysis,
    ) -> rusqlite::Result<()> {
        let json = serde_json::to_string(analysis)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        self.conn.execute(
            "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                size = ?2, mtime = ?3, analysis_version = ?4, doc_json = ?5",
            rusqlite::params![path, size, mtime, analysis.analysis_version as i64, &json],
        )?;
        Ok(())
    }

    /// 行を削除する。
    pub fn delete(&self, path: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM audio_analysis WHERE path = ?1", [path])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_core::{AnalysisConfig, analyze_stereo_timeline};

    fn tmp_db() -> (AudioAnalysisDb, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "mimageviewer_audio_analysis_db_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        (AudioAnalysisDb::open_at(&p).expect("open"), p)
    }

    /// 決定的な合成ステレオ PCM から解析結果を作る (FFmpeg 非依存)。
    fn sample_analysis() -> TimelineAnalysis {
        let sample_rate = 48_000;
        let frames = sample_rate; // 1 秒
        let mut pcm = Vec::with_capacity(frames as usize * 2);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            pcm.push(s); // L
            pcm.push(s); // R
        }
        analyze_stereo_timeline(&pcm, sample_rate, AnalysisConfig::default())
    }

    #[test]
    fn set_and_get_roundtrip() {
        let (db, p) = tmp_db();
        let ta = sample_analysis();
        db.set("c:/music/a.mp3", 1234, 5678, &ta).unwrap();
        let got = db.get("c:/music/a.mp3", 1234, 5678).expect("get");
        assert_eq!(got.analysis_version, ta.analysis_version);
        assert_eq!(got.bins.len(), ta.bins.len());
        assert_eq!(got.stream, ta.stream);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn stale_size_or_mtime_returns_none() {
        let (db, p) = tmp_db();
        let ta = sample_analysis();
        db.set("k", 100, 200, &ta).unwrap();
        // size / mtime いずれかがずれたら要再解析。
        assert!(db.get("k", 999, 200).is_none());
        assert!(db.get("k", 100, 999).is_none());
        // 一致すればヒット。
        assert!(db.get("k", 100, 200).is_some());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn version_gate_rejects_old_row() {
        let (db, p) = tmp_db();
        // analysis_version 列が古い行は無効。手書きで古い版を差し込む。
        let ta = sample_analysis();
        let json = serde_json::to_string(&ta).unwrap();
        db.conn
            .execute(
                "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["k", 1, 2, (TIMELINE_ANALYSIS_VERSION as i64) - 1, &json],
            )
            .unwrap();
        assert!(db.get("k", 1, 2).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_json_returns_none_not_panic() {
        let (db, p) = tmp_db();
        db.conn
            .execute(
                "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["k", 1, 2, TIMELINE_ANALYSIS_VERSION as i64, "{ not json ]"],
            )
            .unwrap();
        assert!(db.get("k", 1, 2).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_key_is_none() {
        let (db, p) = tmp_db();
        assert!(db.get("nope", 1, 2).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn delete_removes_entry() {
        let (db, p) = tmp_db();
        db.set("k", 1, 2, &sample_analysis()).unwrap();
        db.delete("k").unwrap();
        assert!(db.get("k", 1, 2).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn analyze_stereo_timeline_is_deterministic() {
        // 同じ PCM からは同じ解析結果 (bins 長・version) が得られる。デコーダ差で
        // 解析がぶれないことの土台 (docs/music-integration-plan.md Inc 2 決定性テスト)。
        let a = sample_analysis();
        let b = sample_analysis();
        assert_eq!(a.analysis_version, TIMELINE_ANALYSIS_VERSION);
        assert_eq!(a.bins.len(), b.bins.len());
        assert_eq!(a.stream, b.stream);
        for (x, y) in a.bins.iter().zip(b.bins.iter()) {
            assert_eq!(x.peak, y.peak);
            assert_eq!(x.rms, y.rms);
            assert_eq!(x.loudness_db, y.loudness_db);
        }
    }
}
