//! 消しゴムマスクの永続管理。
//!
//! `%APPDATA%/mimageviewer/mask.db` にマスク情報を保存する。
//! マスクは 1bit/pixel にパックし、deflate 圧縮して BLOB に格納する。

use std::io::{Read, Write};
use std::path::PathBuf;

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

/// マスク永続化 DB。
pub struct MaskDb {
    conn: rusqlite::Connection,
}

impl MaskDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    pub fn open_at(path: &std::path::Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS masks (
                path       TEXT PRIMARY KEY,
                mask_data  BLOB    NOT NULL,
                width      INTEGER NOT NULL,
                height     INTEGER NOT NULL
            )",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("mask.db")
    }

    /// マスクを取得する。未登録なら None。
    /// 画像サイズが保存時と異なる場合（PDF再レンダリング等）はリスケールする。
    pub fn get(&self, key: &str, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        let mut stmt = self.conn
            .prepare_cached("SELECT mask_data, width, height FROM masks WHERE path = ?1")
            .ok()?;
        let (blob, w, h): (Vec<u8>, usize, usize) = stmt.query_row([key], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
            ))
        }).ok()?;

        let mask = decompress_mask(&blob, w, h)?;

        if w == expected_w && h == expected_h {
            Some(mask)
        } else {
            // サイズが異なる → 最近傍法でリスケール
            Some(rescale_mask(&mask, w, h, expected_w, expected_h))
        }
    }

    /// マスクを保存する。全て false なら削除する。
    pub fn set(&self, key: &str, mask: &[bool], w: usize, h: usize) -> rusqlite::Result<()> {
        if !mask.iter().any(|&m| m) {
            return self.delete(key);
        }
        self.upsert_mask(key, mask, w, h)
    }

    /// マスクを削除する。
    pub fn delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM masks WHERE path = ?1", [key])?;
        Ok(())
    }

    /// 名前付きスロットにマスクを保存する。`set` と異なり全 false でも保存する。
    pub fn set_slot(&self, slot: usize, mask: &[bool], w: usize, h: usize) -> rusqlite::Result<()> {
        self.upsert_mask(&slot_key(slot), mask, w, h)
    }

    /// 既に 1bit/pixel + deflate 圧縮済みの生バイト列を直接保存する。
    /// サイドカー (mimageviewer.dat) からのインポート時に使用する (再圧縮を避ける)。
    pub fn set_raw(&self, key: &str, compressed: &[u8], w: usize, h: usize) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4",
            rusqlite::params![key, compressed, w as i64, h as i64],
        )?;
        Ok(())
    }


    /// 名前付きスロットからマスクを取得する。サイズが異なる場合は自動リスケール。
    pub fn get_slot(&self, slot: usize, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        self.get(&slot_key(slot), expected_w, expected_h)
    }

    fn upsert_mask(&self, key: &str, mask: &[bool], w: usize, h: usize) -> rusqlite::Result<()> {
        let blob = compress_mask(mask);
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4",
            rusqlite::params![key, blob, w as i64, h as i64],
        )?;
        Ok(())
    }
}

fn slot_key(slot: usize) -> String {
    format!("__slot_{}", slot)
}

/// マスク (Vec<bool>) を 1bit/pixel にパックし deflate 圧縮する。
pub fn compress_mask(mask: &[bool]) -> Vec<u8> {
    // 1bit/pixel にパック
    let byte_count = (mask.len() + 7) / 8;
    let mut packed = vec![0u8; byte_count];
    for (i, &m) in mask.iter().enumerate() {
        if m {
            packed[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    // deflate 圧縮
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&packed).unwrap_or_default();
    encoder.finish().unwrap_or_default()
}

/// deflate 展開して 1bit/pixel をアンパックする。
fn decompress_mask(blob: &[u8], w: usize, h: usize) -> Option<Vec<bool>> {
    let total = w * h;
    let byte_count = (total + 7) / 8;

    let mut decoder = DeflateDecoder::new(blob);
    let mut packed = Vec::new();
    decoder.read_to_end(&mut packed).ok()?;

    if packed.len() < byte_count {
        return None;
    }

    let mut mask = vec![false; total];
    for i in 0..total {
        if packed[i / 8] & (1 << (7 - (i % 8))) != 0 {
            mask[i] = true;
        }
    }
    Some(mask)
}

/// マスクを最近傍法でリスケールする。
fn rescale_mask(
    src: &[bool],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<bool> {
    let mut dst = vec![false; dst_w * dst_h];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;
    for dy in 0..dst_h {
        let sy = ((dy as f32 * y_ratio) as usize).min(src_h.saturating_sub(1));
        for dx in 0..dst_w {
            let sx = ((dx as f32 * x_ratio) as usize).min(src_w.saturating_sub(1));
            dst[dy * dst_w + dx] = src[sy * src_w + sx];
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_compress() {
        let mut mask = vec![false; 1000];
        mask[10] = true;
        mask[100] = true;
        mask[999] = true;

        let compressed = compress_mask(&mask);
        let decompressed = decompress_mask(&compressed, 100, 10).unwrap();
        assert_eq!(mask, decompressed);
    }

    #[test]
    fn empty_mask_compresses() {
        let mask = vec![false; 5000];
        let compressed = compress_mask(&mask);
        assert!(compressed.len() < 50, "empty mask should compress well: {} bytes", compressed.len());
    }

    #[test]
    fn full_mask_roundtrip() {
        let mask = vec![true; 512 * 512];
        let compressed = compress_mask(&mask);
        let decompressed = decompress_mask(&compressed, 512, 512).unwrap();
        assert_eq!(mask, decompressed);
    }
}
