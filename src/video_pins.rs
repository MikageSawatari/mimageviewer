//! 動画フレームのピン留め情報の永続管理 (Phase 5.3 で骨格、Phase 5.4.1 で完成)。
//!
//! `%APPDATA%/mimageviewer/video_pins.db` (= `data_dir`/video_pins.db) に
//! 「ユーザーが選んだ代表フレーム」を保存する。グリッドの動画サムネイル優先順位
//! のうち最上位 (= ユーザー意図) として参照される。
//!
//! # スキーマ
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS video_pins (
//!     path        TEXT PRIMARY KEY,        -- 動画ファイルの正規化パス
//!     pin_pts_secs REAL NOT NULL,          -- ピン位置 (秒)
//!     thumb_webp  BLOB                     -- 抽出済フレーム (WebP、表示用)
//! );
//! ```
//!
//! # 状態 (Phase 5.3 時点)
//!
//! `lookup_thumb` / `set_pin` の API シグネチャは決定し、内部実装はスタブ
//! (= 常に未登録扱い)。Phase 5.4.1 で:
//!
//! - フルスクリーン HUD のピンボタン / コンテキストメニューから `set_pin` を呼ぶ
//! - グリッドサムネ生成経路から `lookup_thumb` を呼ぶ
//!
//! という配線を行う。本ファイルを先行で導入することで、Phase 5.3 の優先順位
//! チェーン (pin > sidecar > shell) のコード上の意図を明示する。

use std::path::{Path, PathBuf};

/// ピン留め情報 1 件分。
#[derive(Clone, Debug)]
pub struct VideoPin {
    pub pin_pts_secs: f64,
    /// 抽出済みの WebP バイト列 (グリッドサムネにそのまま decode して使う)。
    pub thumb_webp: Vec<u8>,
}

/// 動画ピン DB ハンドル。Phase 5.3 時点ではスキーマだけ用意し、API は no-op。
pub struct VideoPinDb {
    #[allow(dead_code)]
    conn: rusqlite::Connection,
}

impl VideoPinDb {
    /// DB を開く (なければ作成)。スキーマだけ作成し、行は何も入らない。
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_pins (
                path TEXT PRIMARY KEY,
                pin_pts_secs REAL NOT NULL,
                thumb_webp BLOB
            )",
        )?;
        Ok(Self { conn })
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("video_pins.db")
    }

    /// pts のみ (= WebP BLOB は取り出さない) のフェッチ。
    /// 描画ループでパネルを毎フレーム再描画する状況 (~60fps) でも、数十 KB の
    /// WebP を毎回 Vec 化するコストを払わずに済む (simplify P1 指摘)。
    pub fn lookup_pts(&self, video_path: &Path) -> Option<f64> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT pin_pts_secs FROM video_pins WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| row.get::<_, f64>(0)).ok()
    }

    /// 動画パスに対応するピン情報を取得。Phase 5.3 ではスキーマ通りに読みに行くが、
    /// 行が無ければ `None` を返す (= 通常の運用)。Phase 5.4.1 で `set_pin` が
    /// 配線されるまで実質常に `None`。
    pub fn lookup(&self, video_path: &Path) -> Option<VideoPin> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT pin_pts_secs, thumb_webp FROM video_pins WHERE path = ?1",
            )
            .ok()?;
        stmt.query_row([&key], |row| {
            let pin_pts_secs: f64 = row.get(0)?;
            let thumb_webp: Vec<u8> = row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default();
            Ok(VideoPin {
                pin_pts_secs,
                thumb_webp,
            })
        })
        .ok()
    }

    /// ピンを書き込む (Phase 5.4.1 で UI から呼ばれる予定)。
    /// 既存ピンがあれば上書きする。`thumb_webp` が空なら BLOB は NULL になる。
    #[allow(dead_code)]
    pub fn set_pin(
        &self,
        video_path: &Path,
        pin_pts_secs: f64,
        thumb_webp: &[u8],
    ) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        let blob: Option<&[u8]> = if thumb_webp.is_empty() {
            None
        } else {
            Some(thumb_webp)
        };
        self.conn.execute(
            "INSERT INTO video_pins (path, pin_pts_secs, thumb_webp) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET pin_pts_secs = ?2, thumb_webp = ?3",
            rusqlite::params![key, pin_pts_secs, blob],
        )?;
        Ok(())
    }

    /// ピンを削除する (Phase 5.4.1 で UI から呼ばれる予定)。
    #[allow(dead_code)]
    pub fn remove(&self, video_path: &Path) -> Result<(), rusqlite::Error> {
        let key = crate::path_key::normalize_keep_drive(video_path);
        self.conn
            .execute("DELETE FROM video_pins WHERE path = ?1", [&key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// メモリ DB で API ラウンドトリップを検証。
    fn open_in_memory() -> VideoPinDb {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS video_pins (
                path TEXT PRIMARY KEY,
                pin_pts_secs REAL NOT NULL,
                thumb_webp BLOB
            )",
        )
        .expect("schema");
        VideoPinDb { conn }
    }

    #[test]
    fn lookup_missing_returns_none() {
        let db = open_in_memory();
        assert!(db.lookup(Path::new("C:/no/such.mp4")).is_none());
    }

    #[test]
    fn set_then_lookup_roundtrip() {
        let db = open_in_memory();
        let p = Path::new("C:/Videos/Movie.MP4");
        let blob = vec![0xDE, 0xAD, 0xBE, 0xEF];
        db.set_pin(p, 12.5, &blob).expect("set");
        let got = db.lookup(p).expect("present");
        assert!((got.pin_pts_secs - 12.5).abs() < 1e-9);
        assert_eq!(got.thumb_webp, blob);
    }

    #[test]
    fn set_overwrites_existing() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.set_pin(p, 1.0, &[1]).unwrap();
        db.set_pin(p, 2.0, &[2, 3]).unwrap();
        let got = db.lookup(p).unwrap();
        assert!((got.pin_pts_secs - 2.0).abs() < 1e-9);
        assert_eq!(got.thumb_webp, vec![2, 3]);
    }

    #[test]
    fn remove_deletes_row() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.set_pin(p, 5.0, &[]).unwrap();
        assert!(db.lookup(p).is_some());
        db.remove(p).unwrap();
        assert!(db.lookup(p).is_none());
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.set_pin(Path::new("C:\\Videos\\A.mp4"), 3.0, &[9]).unwrap();
        // 大文字小文字違い + スラッシュ違いでも同じレコードにヒットすること
        assert!(db.lookup(Path::new("c:/videos/a.mp4")).is_some());
    }
}
