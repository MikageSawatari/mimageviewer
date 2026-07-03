//! 音声解析結果 (`TimelineAnalysis`) の永続キャッシュ。
//!
//! `<data_dir>/audio_analysis.db` に「1 音声ファイル = 解析結果 1 行」を保存する。
//! キー = 実ファイルパス。キャッシュの有効性は `size` + `mtime` + `analysis_version` で
//! 判定し、いずれかがずれていれば stale とみなしてワーカーで再解析する
//! (docs/music-integration-plan.md §6, D8)。
//!
//! **保存形式 (v2, 2026-07-03)**: 波形 bin は表示専用データなので、**u16 量子化 + deflate**
//! したバイナリ BLOB で持つ。旧 v1 は解析結果を JSON TEXT で持っていたが、10ms bin ×
//! 長尺曲 (数時間のコンサート/メガミックス) で 1 曲 400MB 超・DB 全体 1.4GB に肥大したため
//! 置き換えた (684B/bin → 84B/bin + deflate ≒ 23x 削減)。量子化は 0..1 値を u16 (65535 段階)、
//! loudness を i16 (0.01dB)、pitch class を u8 にし、`start_secs` / `duration_secs` は
//! `sample_rate` + `bin_secs` + bin index から復元する (冗長なので保存しない)。`beat_grid` /
//! `stream` / `config` は小さいので JSON で正確に持つ。`TimelineAnalysis` は `BeatGrid` を
//! 内包するので 1 行で完結する。ブックマークは動画機構 (`video_bookmarks`) を再利用するので
//! この DB には含めない (D5.1)。
//!
//! 新規機能なので旧 mIV データからの移行は不要 (D11)。**未リリースなので v1→v2 の移行も
//! 不要**: 旧スキーマ DB を検出したらファイルごと削除して作り直す (旧 1.4GB を確実に解放)。
//! no-row / 壊れ BLOB / stale はすべて `None`（= 要再解析）で、クラッシュさせない。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use music_core::{
    AnalysisConfig, AudioStreamInfo, BeatGrid, TIMELINE_ANALYSIS_VERSION, TimelineAnalysis,
    WaveformBin,
};

/// テーブルスキーマの版 (`PRAGMA user_version`)。スキーマ / BLOB フォーマットを変えたら +1 する。
/// v2 = u16 量子化バイナリ BLOB (v1 = JSON TEXT からの置き換え、2026-07-03)。
pub const SCHEMA_VERSION: i64 = 2;

/// BLOB 先頭に置くフォーマット版 (deflate 解凍後の 1 バイト目)。
const BLOB_FORMAT_VERSION: u8 = 1;
/// 1 bin あたりの u16 量子化フィールド数 (start/duration/loudness/pitch class を除く)。
const BIN_U16_FIELDS: usize = 40;
/// 1 bin の固定バイト数: 40×u16 + i16(loudness) + u8×2(pitch class) = 84。
const BIN_BYTES: usize = BIN_U16_FIELDS * 2 + 2 + 2;

/// 0..1 値を u16 に量子化 (65535 段階、表示用途で目視不可の精度)。
fn q16(x: f32) -> u16 {
    (x.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// u16 量子化値を 0..1 に戻す。
fn dq16(q: u16) -> f32 {
    q as f32 / 65535.0
}

/// bins を除いた解析メタ (JSON で正確に持つ小さい部分)。
#[derive(serde::Serialize, serde::Deserialize)]
struct BlobMeta {
    analysis_version: u32,
    stream: AudioStreamInfo,
    config: AnalysisConfig,
    beat_grid: BeatGrid,
    bin_count: u32,
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    if enc.write_all(data).is_err() {
        return Vec::new();
    }
    enc.finish().unwrap_or_default()
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).ok()?;
    Some(out)
}

/// `TimelineAnalysis` を u16 量子化バイナリ + deflate した BLOB に符号化する。
fn encode_analysis(a: &TimelineAnalysis) -> Vec<u8> {
    let meta = BlobMeta {
        analysis_version: a.analysis_version,
        stream: a.stream,
        config: a.config,
        beat_grid: a.beat_grid.clone(),
        bin_count: a.bins.len() as u32,
    };
    let meta_json = serde_json::to_vec(&meta).unwrap_or_default();
    let mut buf = Vec::with_capacity(1 + 4 + meta_json.len() + a.bins.len() * BIN_BYTES);
    buf.push(BLOB_FORMAT_VERSION);
    buf.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
    buf.extend_from_slice(&meta_json);
    for b in &a.bins {
        // 順序は decode の g(k) と厳密に一致させる。
        let u16s = [
            b.peak,
            b.rms,
            b.band_energy[0],
            b.band_energy[1],
            b.band_energy[2],
            b.transient,
            b.transient_band[0],
            b.transient_band[1],
            b.transient_band[2],
            b.center_ratio,
            b.vocal_score,
            b.brightness,
            b.transient_density,
            b.novelty,
            b.bass_pitch_confidence,
            b.key_confidence,
        ];
        for v in u16s {
            buf.extend_from_slice(&q16(v).to_le_bytes());
        }
        for v in b.bass_chroma {
            buf.extend_from_slice(&q16(v).to_le_bytes());
        }
        for v in b.chroma {
            buf.extend_from_slice(&q16(v).to_le_bytes());
        }
        let ldb = (b.loudness_db.clamp(-120.0, 0.0) * 100.0).round() as i16;
        buf.extend_from_slice(&ldb.to_le_bytes());
        buf.push(b.bass_pitch_class);
        buf.push(b.key_pitch_class);
    }
    deflate(&buf)
}

/// `encode_analysis` の逆。壊れ / 版違いは `None`。
fn decode_analysis(blob: &[u8]) -> Option<TimelineAnalysis> {
    let raw = inflate(blob)?;
    if raw.first().copied()? != BLOB_FORMAT_VERSION {
        return None;
    }
    let mut p = 1usize;
    let meta_len = u32::from_le_bytes(raw.get(p..p + 4)?.try_into().ok()?) as usize;
    p += 4;
    let meta: BlobMeta = serde_json::from_slice(raw.get(p..p + meta_len)?).ok()?;
    p += meta_len;
    let n = meta.bin_count as usize;
    // start_secs / duration_secs を復元する (analyze_stereo_timeline と同じ frames_per_bin)。
    let sr = meta.stream.sample_rate.max(1) as f64;
    let fpb = (meta.config.bin_secs.max(0.01) * sr).round().max(1.0) as u64;
    let total_frames = (meta.stream.duration_secs * sr).round().max(0.0) as u64;
    let mut bins = Vec::with_capacity(n);
    for i in 0..n {
        let off = p + i * BIN_BYTES;
        let rec = raw.get(off..off + BIN_BYTES)?;
        let g = |k: usize| dq16(u16::from_le_bytes([rec[k * 2], rec[k * 2 + 1]]));
        let frame_start = i as u64 * fpb;
        let frame_end = ((i as u64 + 1) * fpb).min(total_frames.max(frame_start));
        let start_secs = frame_start as f64 / sr;
        let duration_secs = frame_end.saturating_sub(frame_start) as f64 / sr;
        let loud_off = BIN_U16_FIELDS * 2;
        let loudness_db = i16::from_le_bytes([rec[loud_off], rec[loud_off + 1]]) as f32 / 100.0;
        bins.push(WaveformBin {
            start_secs,
            duration_secs,
            peak: g(0),
            rms: g(1),
            loudness_db,
            band_energy: [g(2), g(3), g(4)],
            transient: g(5),
            transient_band: [g(6), g(7), g(8)],
            center_ratio: g(9),
            vocal_score: g(10),
            brightness: g(11),
            transient_density: g(12),
            novelty: g(13),
            bass_pitch_class: rec[loud_off + 2],
            bass_pitch_confidence: g(14),
            key_pitch_class: rec[loud_off + 3],
            key_confidence: g(15),
            bass_chroma: [
                g(16),
                g(17),
                g(18),
                g(19),
                g(20),
                g(21),
                g(22),
                g(23),
                g(24),
                g(25),
                g(26),
                g(27),
            ],
            chroma: [
                g(28),
                g(29),
                g(30),
                g(31),
                g(32),
                g(33),
                g(34),
                g(35),
                g(36),
                g(37),
                g(38),
                g(39),
            ],
        });
    }
    Some(TimelineAnalysis {
        analysis_version: meta.analysis_version,
        stream: meta.stream,
        config: meta.config,
        bins,
        beat_grid: meta.beat_grid,
    })
}

/// 音声解析結果の永続キャッシュ DB。内部は SQLite `audio_analysis` テーブル。
pub struct AudioAnalysisDb {
    conn: rusqlite::Connection,
}

/// 旧スキーマ (user_version != 現行) / 壊れ DB を検出する。ファイルが無ければ false。
fn needs_reset(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match rusqlite::Connection::open(path) {
        Ok(conn) => {
            let ver: i64 = conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap_or(0);
            ver != SCHEMA_VERSION
        }
        Err(_) => true,
    }
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
        if needs_reset(path) {
            // 未リリースにつき v1→v2 の移行は不要 (docs: 永続データ判断)。旧スキーマ
            // (巨大 JSON TEXT) はファイルごと削除して作り直し、旧 1.4GB を確実に解放する。
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(path.with_extension("db-wal"));
            let _ = std::fs::remove_file(path.with_extension("db-shm"));
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS audio_analysis (
                path             TEXT    PRIMARY KEY,
                size             INTEGER NOT NULL,
                mtime            INTEGER NOT NULL,
                analysis_version INTEGER NOT NULL,
                doc              BLOB    NOT NULL
            )",
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("audio_analysis.db")
    }

    /// キャッシュ DB ファイルのパス (環境設定のサイズ表示 / 削除 UI 用)。
    pub fn path() -> PathBuf {
        Self::db_path()
    }

    /// キャッシュされた解析結果を返す。`size` / `mtime` / `analysis_version` が現在の
    /// ファイルと一致する行だけを有効とみなす。不一致・no-row・壊れ BLOB は `None`
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
                "SELECT doc FROM audio_analysis
                 WHERE path = ?1 AND size = ?2 AND mtime = ?3 AND analysis_version = ?4",
            )
            .ok()?;
        let blob: Vec<u8> = stmt
            .query_row(
                rusqlite::params![path, size, mtime, TIMELINE_ANALYSIS_VERSION as i64],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok()?;
        match decode_analysis(&blob) {
            // 二重ガード: 列の analysis_version が一致しても、BLOB 内 version も現行と
            // 一致することを確認する (直列化フォーマットの取り違え防止)。
            Some(ta) if ta.analysis_version == TIMELINE_ANALYSIS_VERSION => Some(ta),
            Some(_) => None,
            None => {
                crate::logger::log(format!(
                    "[audio] audio_analysis.db doc decode failed path={path}"
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
        let blob = encode_analysis(analysis);
        self.conn.execute(
            "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                size = ?2, mtime = ?3, analysis_version = ?4, doc = ?5",
            rusqlite::params![path, size, mtime, analysis.analysis_version as i64, &blob],
        )?;
        Ok(())
    }

    /// 行を削除する。
    pub fn delete(&self, path: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM audio_analysis WHERE path = ?1", [path])?;
        Ok(())
    }

    /// キャッシュ DB ファイル (+ wal/shm) を削除する (環境設定の「オーディオ解析キャッシュの
    /// 削除」)。`DELETE + VACUUM` はファイル全体を書き換えて UI をブロックしうる (Codex P2) ため、
    /// ファイルごと unlink する: 1.4GB でも metadata 操作なので高速、かつ全容量を即解放する。
    /// SQLite ロックも介さないので `database is locked` は起きない (Codex P3)。次に音声を開くと
    /// `open_at` が空 DB を作り直す。背景に解析ワーカーが DB を開いている最中は Windows で削除に
    /// 失敗しうる (その場合 `Err`)。UI スレッドから呼んでよい。
    pub fn delete_cache_files() -> std::io::Result<()> {
        Self::delete_cache_files_at(&Self::db_path())
    }

    fn delete_cache_files_at(path: &Path) -> std::io::Result<()> {
        let mut result = Ok(());
        for f in [
            path.to_path_buf(),
            path.with_extension("db-wal"),
            path.with_extension("db-shm"),
        ] {
            if f.exists()
                && let Err(e) = std::fs::remove_file(&f)
            {
                result = Err(e);
            }
        }
        result
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

    /// u16 量子化のラウンドトリップ誤差が表示に十分小さいこと + start/duration の復元が正確なこと。
    #[test]
    fn quantized_roundtrip_within_tolerance() {
        let ta = sample_analysis();
        let blob = encode_analysis(&ta);
        let got = decode_analysis(&blob).expect("decode");
        assert_eq!(got.bins.len(), ta.bins.len());
        // 圧縮後の方が元 JSON より十分小さい (少なくとも縮んでいる)。
        let json_len = serde_json::to_vec(&ta).unwrap().len();
        assert!(
            blob.len() < json_len,
            "blob {} should be smaller than json {json_len}",
            blob.len()
        );
        for (a, b) in ta.bins.iter().zip(got.bins.iter()) {
            // 0..1 値は 1/65535 + 余裕。
            assert!((a.peak - b.peak).abs() <= 2.0 / 65535.0);
            assert!((a.rms - b.rms).abs() <= 2.0 / 65535.0);
            for k in 0..3 {
                assert!((a.band_energy[k] - b.band_energy[k]).abs() <= 2.0 / 65535.0);
            }
            for k in 0..12 {
                assert!((a.chroma[k] - b.chroma[k]).abs() <= 2.0 / 65535.0);
                assert!((a.bass_chroma[k] - b.bass_chroma[k]).abs() <= 2.0 / 65535.0);
            }
            // loudness は 0.01dB 精度。
            assert!((a.loudness_db - b.loudness_db).abs() <= 0.02);
            // pitch class は無損失。
            assert_eq!(a.bass_pitch_class, b.bass_pitch_class);
            assert_eq!(a.key_pitch_class, b.key_pitch_class);
            // start_secs / duration_secs は index から正確に復元。
            assert!((a.start_secs - b.start_secs).abs() <= 1.0e-9);
            assert!((a.duration_secs - b.duration_secs).abs() <= 1.0e-9);
        }
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
        let blob = encode_analysis(&ta);
        db.conn
            .execute(
                "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["k", 1, 2, (TIMELINE_ANALYSIS_VERSION as i64) - 1, &blob],
            )
            .unwrap();
        assert!(db.get("k", 1, 2).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_blob_returns_none_not_panic() {
        let (db, p) = tmp_db();
        db.conn
            .execute(
                "INSERT INTO audio_analysis (path, size, mtime, analysis_version, doc)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    "k",
                    1,
                    2,
                    TIMELINE_ANALYSIS_VERSION as i64,
                    &b"not a valid deflate blob"[..]
                ],
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
    fn delete_cache_files_removes_db() {
        let (db, p) = tmp_db();
        db.set("a", 1, 2, &sample_analysis()).unwrap();
        drop(db); // Windows: 開いている接続があると remove_file が失敗しうるので閉じる。
        assert!(p.exists());
        AudioAnalysisDb::delete_cache_files_at(&p).unwrap();
        assert!(!p.exists());
        // 削除後に開き直すと空 DB が作られ、旧行は無い。
        let db2 = AudioAnalysisDb::open_at(&p).unwrap();
        assert!(db2.get("a", 1, 2).is_none());
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
