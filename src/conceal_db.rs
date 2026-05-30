//! 隠蔽加工マスクの永続管理。
//!
//! `%APPDATA%/mimageviewer/conceal.db` にマスク情報 (ビットマップ + Shape ベクタ) を
//! 保存する。実装は [`crate::mask_db`] のクローンで、テーブル名と Shape 採用以外は
//! 同形 (1bit/pixel + deflate 圧縮、スロット機構、PDF リスケール対応)。
//!
//! ## マスクスロット
//!
//! `__slot_1` / `__slot_2` を予約キーとしてスロット保存する。差分画像生成
//! (= 同一マスクを複数画像に適用) ワークフローを支援する。消しゴム ([`crate::mask_db`])
//! と同じ 2 スロット制。表示モードでは F9/F10 でスロット 1/2 を quick apply できる。
//!
//! ## DB に保存しないもの (グローバル設定)
//!
//! 隠蔽タイプ・タイル倍率・境界モード・不透明度・ぼかし半径・ぼかしモード・
//! 境界フェード等の**処理パラメータ**は [`crate::settings`] 側に保存する。
//! conceal_db はマスク (ビットマップ + ベクタ) のみを保持。これにより
//! 「同じマスクで異なるパラメータの結果を Ctrl+E で複数保存」が自然に成立する。

use std::path::PathBuf;

use crate::mask_db::{Shape, compress_mask, shapes_from_json, shapes_to_json};

/// マスク永続化 DB (隠蔽加工用)。
///
/// 内部は SQLite `conceal_entries` テーブル。スキーマは [`open_at`] 参照。
pub struct ConcealDb {
    conn: rusqlite::Connection,
}

impl ConcealDb {
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
            "CREATE TABLE IF NOT EXISTS conceal_entries (
                page_path   TEXT    PRIMARY KEY,
                bitmap_w    INTEGER NOT NULL,
                bitmap_h    INTEGER NOT NULL,
                bitmap_data BLOB    NOT NULL,
                shapes      TEXT
            )",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("conceal.db")
    }

    /// マスクとベクタ群をまとめて取得する。
    ///
    /// 画像サイズが保存時と異なる場合 (PDF 再レンダリング等) はビットマップを
    /// 最近傍法でリスケールし、Shape 群も比率で `scale_xy(sx, sy)` する。
    pub fn get_full(
        &self,
        key: &str,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<Shape>)> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT bitmap_data, bitmap_w, bitmap_h, shapes
                 FROM conceal_entries WHERE page_path = ?1",
            )
            .ok()?;
        let (blob, w, h, shapes_json): (Vec<u8>, usize, usize, Option<String>) = stmt
            .query_row([key], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .ok()?;

        let mut mask = decompress_mask(&blob, w, h)?;
        let mut shapes = shapes_json
            .as_deref()
            .map(shapes_from_json)
            .unwrap_or_default();

        if w != expected_w || h != expected_h {
            mask = rescale_mask(&mask, w, h, expected_w, expected_h);
            let sx = expected_w as f32 / w.max(1) as f32;
            let sy = expected_h as f32 / h.max(1) as f32;
            for s in &mut shapes {
                s.scale_xy(sx, sy);
            }
        }
        Some((mask, shapes))
    }

    /// マスク + ベクタを保存する。ビットマップが全 false でベクタも空なら削除する。
    pub fn set(
        &self,
        key: &str,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let bitmap_empty = !mask.iter().any(|&m| m);
        if bitmap_empty && shapes.is_empty() {
            return self.delete(key);
        }
        self.upsert(key, mask, shapes, w, h)
    }

    /// マスクを削除する。
    pub fn delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM conceal_entries WHERE page_path = ?1", [key])?;
        Ok(())
    }

    /// 名前付きスロットにマスクを保存する。`set` と異なりビットマップ全 false でも保存する
    /// (= 「ベクタだけのマスク」をスロットに保管できる)。
    pub fn set_slot(
        &self,
        slot: usize,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.upsert(&slot_key(slot), mask, shapes, w, h)
    }

    /// 名前付きスロットからマスクとベクタ群を取得する。
    pub fn get_slot_full(
        &self,
        slot: usize,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<Shape>)> {
        self.get_full(&slot_key(slot), expected_w, expected_h)
    }

    /// スロットの元のサイズ (width, height) を返す。存在しなければ None。
    pub fn slot_size(&self, slot: usize) -> Option<(usize, usize)> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT bitmap_w, bitmap_h FROM conceal_entries WHERE page_path = ?1")
            .ok()?;
        stmt.query_row([slot_key(slot)], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        })
        .ok()
    }

    /// スロットを削除する。
    pub fn delete_slot(&self, slot: usize) -> rusqlite::Result<()> {
        self.delete(&slot_key(slot))
    }

    /// 既に deflate 圧縮済みのビットマップ + JSON 済みベクタを直接保存する。
    /// サイドカー (mimageviewer.dat) からのインポート時に使用 (再圧縮を避ける)。
    pub fn set_raw(
        &self,
        key: &str,
        compressed: &[u8],
        shapes_json: Option<&str>,
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO conceal_entries (page_path, bitmap_w, bitmap_h, bitmap_data, shapes)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(page_path) DO UPDATE SET
                bitmap_w = ?2, bitmap_h = ?3, bitmap_data = ?4, shapes = ?5",
            rusqlite::params![key, w as i64, h as i64, compressed, shapes_json],
        )?;
        Ok(())
    }

    /// 指定プレフィックスで始まるパスを持つマスクエントリのキー集合を返す。
    /// フォルダ単位の「このフォルダ内でマスクを持つページ」列挙に使う (バッジ用)。
    /// スロットキー (`__slot_*`) は除外する。
    pub fn load_conceal_keys(&self, prefix: &str) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path FROM conceal_entries
             WHERE page_path LIKE ?1 ESCAPE '\\'
               AND page_path NOT LIKE '\\_\\_slot\\_%' ESCAPE '\\'",
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

    fn upsert(
        &self,
        key: &str,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let blob = compress_mask(mask);
        let shapes_json = shapes_to_json(shapes);
        self.set_raw(key, &blob, shapes_json.as_deref(), w, h)
    }
}

/// スロットキー (`__slot_1` / `__slot_2`) を生成する。
pub fn slot_key(slot: usize) -> String {
    format!("__slot_{}", slot)
}

/// deflate 展開して 1bit/pixel をアンパックする。
/// `mask_db::decompress_mask` と同じロジック (private なので複製)。
fn decompress_mask(blob: &[u8], w: usize, h: usize) -> Option<Vec<bool>> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    let total = w * h;
    let byte_count = total.div_ceil(8);

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

/// マスクを最近傍法でリスケールする (`mask_db::rescale_mask` と同じ)。
fn rescale_mask(src: &[bool], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<bool> {
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
    use crate::mask_db::{LineKind, Shape, ShapeOp};

    fn tmp_db() -> (ConcealDb, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "mimageviewer_conceal_db_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        (ConcealDb::open_at(&p).expect("open"), p)
    }

    #[test]
    fn slot_key_format() {
        assert_eq!(slot_key(1), "__slot_1");
        assert_eq!(slot_key(2), "__slot_2");
    }

    #[test]
    fn set_and_get_roundtrip() {
        let (db, p) = tmp_db();
        let w = 50;
        let h = 50;
        let mut mask = vec![false; w * h];
        mask[10 * w + 10] = true;
        mask[40 * w + 40] = true;
        let shapes = vec![
            Shape::Rect {
                op: ShapeOp::Add,
                center: (25.0, 25.0),
                half_w: 5.0,
                half_h: 3.0,
                rotation_rad: 0.0,
            },
            Shape::Ellipse {
                op: ShapeOp::Add,
                center: (30.0, 15.0),
                rx: 4.0,
                ry: 2.0,
                rotation_rad: 0.0,
            },
        ];
        db.set("test/image.png", &mask, &shapes, w, h).unwrap();
        let (got_mask, got_shapes) = db.get_full("test/image.png", w, h).expect("get");
        assert_eq!(got_mask, mask);
        assert_eq!(got_shapes, shapes);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn empty_mask_and_empty_shapes_deletes() {
        let (db, p) = tmp_db();
        let mask = vec![false; 100];
        db.set("k", &mask, &[], 10, 10).unwrap();
        // set 後でも全 false / 空 shapes なら DB に何も入らない
        assert!(db.get_full("k", 10, 10).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn empty_mask_with_shapes_persists() {
        let (db, p) = tmp_db();
        let mask = vec![false; 100];
        let shapes = vec![Shape::Line {
            op: ShapeOp::Add,
            kind: LineKind::Diagonal,
            p0: (0.0, 0.0),
            p1: (10.0, 10.0),
            thickness: 1.0,
        }];
        db.set("k", &mask, &shapes, 10, 10).unwrap();
        let (got_mask, got_shapes) = db.get_full("k", 10, 10).expect("get");
        assert_eq!(got_mask.len(), 100);
        assert!(!got_mask.iter().any(|&b| b));
        assert_eq!(got_shapes, shapes);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn slot_roundtrip() {
        let (db, p) = tmp_db();
        let mask = vec![true; 64];
        let shapes = vec![Shape::Rect {
            op: ShapeOp::Add,
            center: (4.0, 4.0),
            half_w: 2.0,
            half_h: 2.0,
            rotation_rad: 0.0,
        }];
        db.set_slot(1, &mask, &shapes, 8, 8).unwrap();
        let got = db.get_slot_full(1, 8, 8).expect("slot1");
        assert_eq!(got.0, mask);
        assert_eq!(got.1, shapes);
        assert_eq!(db.slot_size(1), Some((8, 8)));
        // slot 2 は空のはず
        assert!(db.get_slot_full(2, 8, 8).is_none());
        // delete_slot で消える
        db.delete_slot(1).unwrap();
        assert!(db.get_slot_full(1, 8, 8).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rescale_on_size_change() {
        let (db, p) = tmp_db();
        let w = 100;
        let h = 100;
        let mut mask = vec![false; w * h];
        mask[50 * w + 50] = true;
        let shapes = vec![Shape::Rect {
            op: ShapeOp::Add,
            center: (50.0, 50.0),
            half_w: 10.0,
            half_h: 5.0,
            rotation_rad: 0.0,
        }];
        db.set("k", &mask, &shapes, w, h).unwrap();
        // 倍サイズで取得 (isotropic 2x)
        let (got_mask, got_shapes) = db.get_full("k", 200, 200).expect("get");
        assert_eq!(got_mask.len(), 200 * 200);
        match got_shapes[0] {
            Shape::Rect {
                center,
                half_w,
                half_h,
                ..
            } => {
                assert_eq!(center, (100.0, 100.0));
                assert_eq!(half_w, 20.0);
                assert_eq!(half_h, 10.0);
            }
            _ => panic!("rect expected"),
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_conceal_keys_excludes_slots() {
        let (db, p) = tmp_db();
        let w = 4;
        let h = 4;
        let mask = vec![true; 16];
        db.set("c:/foo/img1.png", &mask, &[], w, h).unwrap();
        db.set("c:/foo/img2.png", &mask, &[], w, h).unwrap();
        db.set("c:/other/img.png", &mask, &[], w, h).unwrap();
        db.set_slot(1, &mask, &[], w, h).unwrap();
        let keys = db.load_conceal_keys("c:/foo/");
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("c:/foo/img1.png"));
        assert!(keys.contains("c:/foo/img2.png"));
        // スロットは除外
        assert!(!keys.iter().any(|k| k.starts_with("__slot_")));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn set_raw_with_explicit_json() {
        let (db, p) = tmp_db();
        let mask = vec![true; 16];
        let blob = compress_mask(&mask);
        // タグ付き Shape JSON を直接渡す
        let json =
            r#"[{"type":"line","kind":"diag","p0":[0.0,0.0],"p1":[3.0,3.0],"thickness":1.0}]"#;
        db.set_raw("k", &blob, Some(json), 4, 4).unwrap();
        let (got_mask, got_shapes) = db.get_full("k", 4, 4).expect("get");
        assert_eq!(got_mask, mask);
        assert_eq!(got_shapes.len(), 1);
        match got_shapes[0] {
            Shape::Line {
                kind: LineKind::Diagonal,
                ..
            } => {}
            _ => panic!("expected line"),
        }
        let _ = std::fs::remove_file(&p);
    }
}
