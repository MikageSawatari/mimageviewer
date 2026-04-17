//! 消しゴムマスクの永続管理。
//!
//! `%APPDATA%/mimageviewer/mask.db` にマスク情報を保存する。
//! マスクは 1bit/pixel にパックし、deflate 圧縮して BLOB に格納する。
//!
//! 縦線/横線/直線はベクタオブジェクト (`LineObject`) として `vectors` 列に
//! JSON 文字列で保存する。囲み/筆はビットマップ側にラスタライズ済み。

use std::io::{Read, Write};
use std::path::PathBuf;

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

/// ベクタ線オブジェクトの種別。作成時のツールで決まる。
/// 作成時の挙動 (初期幾何) のみに影響し、保存後の編集では区別しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    #[serde(rename = "vert")]
    Vertical,
    #[serde(rename = "horiz")]
    Horizontal,
    #[serde(rename = "diag")]
    Diagonal,
}

/// 1 本のベクタ線オブジェクト。
///
/// `p0` → `p1` を結ぶ中心軸に沿った、厚さ `thickness` の矩形としてラスタライズする。
/// 縦/横線も内部的にはこの形式で保存し、rasterize 時に差異はない。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineObject {
    pub kind: LineKind,
    pub p0: (f32, f32),
    pub p1: (f32, f32),
    pub thickness: f32,
}

impl LineObject {
    /// オブジェクトの中心点 (回転基準)。
    pub fn center(&self) -> (f32, f32) {
        ((self.p0.0 + self.p1.0) * 0.5, (self.p0.1 + self.p1.1) * 0.5)
    }

    /// オブジェクトを (dx, dy) だけ平行移動する。
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.p0.0 += dx;
        self.p0.1 += dy;
        self.p1.0 += dx;
        self.p1.1 += dy;
    }

    /// 指定中心周りに `angle` [rad] 回転する。
    pub fn rotate_around(&mut self, cx: f32, cy: f32, angle: f32) {
        let (s, c) = angle.sin_cos();
        let rot = |p: (f32, f32)| -> (f32, f32) {
            let dx = p.0 - cx;
            let dy = p.1 - cy;
            (cx + dx * c - dy * s, cy + dx * s + dy * c)
        };
        self.p0 = rot(self.p0);
        self.p1 = rot(self.p1);
    }

    /// 4 隅の矩形コーナーを返す (ラスタライズ/ヒットテスト用)。
    /// `extra_thickness` は判定に少し余裕を持たせる用途。
    pub fn corners(&self, extra_thickness: f32) -> [(f32, f32); 4] {
        let dx = self.p1.0 - self.p0.0;
        let dy = self.p1.1 - self.p0.1;
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let nx = -dy / len;
        let ny = dx / len;
        let half = (self.thickness * 0.5 + extra_thickness).max(0.0);
        [
            (self.p0.0 + nx * half, self.p0.1 + ny * half),
            (self.p1.0 + nx * half, self.p1.1 + ny * half),
            (self.p1.0 - nx * half, self.p1.1 - ny * half),
            (self.p0.0 - nx * half, self.p0.1 - ny * half),
        ]
    }
}

/// ベクタ群を既存の 1bit マスク上にラスタライズする (in-place OR)。
pub fn rasterize_vectors_into(mask: &mut [bool], vectors: &[LineObject], w: usize, h: usize) {
    for v in vectors {
        let pts = v.corners(0.0);
        scanline_fill_polygon(mask, &pts, w, h, true);
    }
}

/// スキャンライン方式の多角形塗り。エラサーモードのビットマップ塗りと
/// ベクタラスタライズで共用する。`value=true` で塗り、`false` で消去。
pub fn scanline_fill_polygon(
    mask: &mut [bool],
    pts: &[(f32, f32)],
    w: usize,
    h: usize,
    value: bool,
) {
    if pts.len() < 3 { return; }
    let min_y = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0) as usize;
    let max_y = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max).min(h as f32) as usize;
    let n = pts.len();
    let mut intersections = Vec::with_capacity(8);
    for y in min_y..max_y {
        let scan_y = y as f32 + 0.5;
        intersections.clear();
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            if (y0 <= scan_y && y1 > scan_y) || (y1 <= scan_y && y0 > scan_y) {
                let t = (scan_y - y0) / (y1 - y0);
                intersections.push(x0 + t * (x1 - x0));
            }
        }
        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in intersections.chunks(2) {
            if pair.len() == 2 {
                let px0 = (pair[0].max(0.0) as usize).min(w);
                let px1 = (pair[1].max(0.0).ceil() as usize).min(w);
                for px in px0..px1 {
                    mask[y * w + px] = value;
                }
            }
        }
    }
}

/// ベクタ群を JSON 文字列にシリアライズする。空なら None。
pub fn vectors_to_json(vectors: &[LineObject]) -> Option<String> {
    if vectors.is_empty() {
        return None;
    }
    serde_json::to_string(vectors).ok()
}

/// JSON 文字列からベクタ群をデシリアライズする。失敗時は空。
pub fn vectors_from_json(s: &str) -> Vec<LineObject> {
    serde_json::from_str(s).unwrap_or_default()
}

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
                height     INTEGER NOT NULL,
                vectors    TEXT
            )",
        )?;
        // 既存 DB には vectors 列が無い可能性があるので ALTER で追加する。
        // 既に列があればエラーになるが無視する。
        let _ = conn.execute("ALTER TABLE masks ADD COLUMN vectors TEXT", []);
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("mask.db")
    }

    /// マスク (ビットマップのみ) を取得する。互換用。
    pub fn get(&self, key: &str, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        self.get_full(key, expected_w, expected_h).map(|(m, _)| m)
    }

    /// マスクとベクタ群をまとめて取得する。
    /// 画像サイズが保存時と異なる場合 (PDF 再レンダリング等) はビットマップをリスケールし、
    /// ベクタ座標も比率で伸縮する。
    pub fn get_full(
        &self,
        key: &str,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<LineObject>)> {
        let mut stmt = self.conn
            .prepare_cached("SELECT mask_data, width, height, vectors FROM masks WHERE path = ?1")
            .ok()?;
        let (blob, w, h, vectors_json): (Vec<u8>, usize, usize, Option<String>) =
            stmt.query_row([key], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, Option<String>>(3)?,
                ))
            }).ok()?;

        let mut mask = decompress_mask(&blob, w, h)?;
        let mut vectors = vectors_json
            .as_deref()
            .map(vectors_from_json)
            .unwrap_or_default();

        if w != expected_w || h != expected_h {
            mask = rescale_mask(&mask, w, h, expected_w, expected_h);
            let sx = expected_w as f32 / w.max(1) as f32;
            let sy = expected_h as f32 / h.max(1) as f32;
            for v in &mut vectors {
                v.p0.0 *= sx;
                v.p0.1 *= sy;
                v.p1.0 *= sx;
                v.p1.1 *= sy;
                // 幅は縦横スケールの平均で伸縮
                v.thickness *= (sx + sy) * 0.5;
            }
        }
        Some((mask, vectors))
    }

    /// マスク＋ベクタを保存する。ビットマップが全 false でベクタも空なら削除する。
    pub fn set(
        &self,
        key: &str,
        mask: &[bool],
        vectors: &[LineObject],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let bitmap_empty = !mask.iter().any(|&m| m);
        if bitmap_empty && vectors.is_empty() {
            return self.delete(key);
        }
        self.upsert_mask(key, mask, vectors, w, h)
    }

    /// マスクを削除する。
    pub fn delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM masks WHERE path = ?1", [key])?;
        Ok(())
    }

    /// 名前付きスロットにマスクを保存する。`set` と異なりビットマップ全 false でも保存する。
    pub fn set_slot(
        &self,
        slot: usize,
        mask: &[bool],
        vectors: &[LineObject],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.upsert_mask(&slot_key(slot), mask, vectors, w, h)
    }

    /// 既に 1bit/pixel + deflate 圧縮済みの生バイト列を直接保存する。
    /// サイドカー (mimageviewer.dat) からのインポート時に使用する (再圧縮を避ける)。
    pub fn set_raw(
        &self,
        key: &str,
        compressed: &[u8],
        vectors_json: Option<&str>,
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height, vectors)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4, vectors = ?5",
            rusqlite::params![key, compressed, w as i64, h as i64, vectors_json],
        )?;
        Ok(())
    }


    /// 名前付きスロットからマスク (ビットマップのみ) を取得する。互換用。
    pub fn get_slot(&self, slot: usize, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        self.get(&slot_key(slot), expected_w, expected_h)
    }

    /// 名前付きスロットからマスクとベクタ群を取得する。
    pub fn get_slot_full(
        &self,
        slot: usize,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<LineObject>)> {
        self.get_full(&slot_key(slot), expected_w, expected_h)
    }

    /// スロットの元のサイズ (width, height) を返す。存在しなければ None。
    /// 一括適用で元サイズのままデータを配る場合に使う。
    pub fn slot_size(&self, slot: usize) -> Option<(usize, usize)> {
        let mut stmt = self.conn
            .prepare_cached("SELECT width, height FROM masks WHERE path = ?1")
            .ok()?;
        stmt.query_row([slot_key(slot)], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        }).ok()
    }

    /// 指定プレフィックスで始まるパスを持つマスクエントリのキー集合を返す。
    /// フォルダ単位の「このフォルダ内でマスクを持つページ」列挙に使う。
    /// スロットキー (`__slot_*`) は除外する。
    pub fn load_mask_keys(&self, prefix: &str) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT path FROM masks WHERE path LIKE ?1 ESCAPE '\\' AND path NOT LIKE '\\_\\_slot\\_%' ESCAPE '\\'"
        ) else {
            return set;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for r in rows.flatten() {
            set.insert(r);
        }
        set
    }

    fn upsert_mask(
        &self,
        key: &str,
        mask: &[bool],
        vectors: &[LineObject],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let blob = compress_mask(mask);
        let vectors_json = vectors_to_json(vectors);
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height, vectors)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4, vectors = ?5",
            rusqlite::params![key, blob, w as i64, h as i64, vectors_json],
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

    #[test]
    fn vector_rasterize_and_serialize() {
        let v = LineObject {
            kind: LineKind::Diagonal,
            p0: (10.0, 10.0),
            p1: (90.0, 10.0),
            thickness: 4.0,
        };
        let json = vectors_to_json(&[v]).unwrap();
        let back = vectors_from_json(&json);
        assert_eq!(back.len(), 1);

        let mut mask = vec![false; 100 * 20];
        rasterize_vectors_into(&mut mask, &[v], 100, 20);
        // 中心軸 y=10, thickness=4 → y=8..12 の範囲で x=10..90 が塗られているはず
        assert!(mask[10 * 100 + 50]);
        assert!(!mask[0 * 100 + 50]);
    }

    #[test]
    fn empty_vectors_serialize_to_none() {
        assert!(vectors_to_json(&[]).is_none());
    }
}
