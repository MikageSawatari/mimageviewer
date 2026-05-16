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
//!     thumb_webp  BLOB,                    -- 抽出済フレーム (WebP、表示用)
//!     thumb_pts_secs REAL                  -- 上記 WebP が抽出された pts (T37、`pin_pts_secs`
//!                                          --   と異なれば pending)
//! );
//! ```
//!
//! `thumb_pts_secs` は v0.9.0 で導入 (T37 / Codex R-VPIN-001)。`set_pin` で空 WebP が
//! 渡されたとき、旧 `thumb_webp` を保持しつつ NULL に落として「ピン位置と画像が不一致」
//! を機械可読化する。`pending_pin_thumb_refresh` (= `app/native_video.rs`) が thumb worker
//! 完了で `set_pin_thumb` を呼ぶと両者が一致する状態に戻る。リリース前のスキーマ追加
//! (= データ移行は不要)。
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
    /// `thumb_webp` が抽出された pts (T37 / Codex R-VPIN-001)。
    /// - `Some(x)`: WebP は pts=x の frame から抽出済。`x == pin_pts_secs` なら整合。
    /// - `None`: WebP の所属 pts 不明 (= 旧スキーマ行、または set_pin で webp 未指定の
    ///   pending 状態)。consumer 側は「整合性不明 / pending」として扱う。
    pub thumb_pts_secs: Option<f64>,
}

impl VideoPin {
    /// `thumb_webp` が `pin_pts_secs` と整合しているか。
    /// `false` の場合は thumb worker が新サムネを抽出中の pending 状態
    /// (= 古い画像を仮表示中、近く更新される)。grid 側はこれで「読み込み中」表示を分岐できる。
    #[allow(dead_code)]
    pub fn thumb_is_current(&self) -> bool {
        self.thumb_pts_secs
            .is_some_and(|t| (t - self.pin_pts_secs).abs() < 1e-3)
    }
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
                thumb_webp BLOB,
                thumb_pts_secs REAL
            )",
        )?;
        // T37: v0.9.0 リリース前の dev 環境 (= 上記 CREATE TABLE が `thumb_pts_secs`
        // 無しで先に作られた行があるかもしれない) に備えて `ALTER TABLE ADD COLUMN`
        // も冪等に流す。失敗 (= 既に存在) は無視。リリース後は CREATE 一発で済む。
        let _ = conn.execute("ALTER TABLE video_pins ADD COLUMN thumb_pts_secs REAL", []);
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
                "SELECT pin_pts_secs, thumb_webp, thumb_pts_secs FROM video_pins WHERE path = ?1",
            )
            .ok()?;
        stmt.query_row([&key], |row| {
            let pin_pts_secs: f64 = row.get(0)?;
            let thumb_webp: Vec<u8> = row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default();
            let thumb_pts_secs: Option<f64> = row.get::<_, Option<f64>>(2)?;
            Ok(VideoPin {
                pin_pts_secs,
                thumb_webp,
                thumb_pts_secs,
            })
        })
        .ok()
    }

    /// ピンを書き込む。既存ピンがあれば pts を新値で上書きする。
    ///
    /// `thumb_webp` の扱い:
    /// - **新規 path** (= ON CONFLICT に当たらない): 空なら BLOB は NULL のまま入る
    ///   (グリッド側は空サムネを除外して Shell API サムネ等にフォールバック)
    /// - **既存 path の上書き**: 引数が空 (NULL) なら既存 `thumb_webp` を**保持**、
    ///   非空なら新サムネに更新する。これは「常に上書きセット」UI 動作のもとで、
    ///   新位置の seek thumbnail が未生成のタイミングに上書きされても既存ピンの
    ///   グリッドサムネが消えないようにするため (`set_native_video_pin` から
    ///   `nearest_seek_thumbnail` が None を返す瞬間に呼ばれるケース)。
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
        // T37: WebP が新しく渡ってきた場合だけ thumb_pts_secs を pin_pts_secs と
        // 同期させる。空 WebP (= 旧サムネ温存) では thumb_pts_secs を NULL に落として
        // 「画像が新 pin_pts と不一致 (pending)」を機械可読化する。pending_pin_thumb_refresh
        // が後で完了 WebP を持って set_pin を呼べば再び整合状態へ戻る。
        let thumb_pts_for_set: Option<f64> = if blob.is_some() {
            Some(pin_pts_secs)
        } else {
            None
        };
        self.conn.execute(
            "INSERT INTO video_pins (path, pin_pts_secs, thumb_webp, thumb_pts_secs)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                pin_pts_secs = excluded.pin_pts_secs,
                thumb_webp = CASE
                    WHEN excluded.thumb_webp IS NOT NULL AND length(excluded.thumb_webp) > 0
                        THEN excluded.thumb_webp
                    ELSE video_pins.thumb_webp
                END,
                thumb_pts_secs = CASE
                    WHEN excluded.thumb_webp IS NOT NULL AND length(excluded.thumb_webp) > 0
                        THEN excluded.thumb_pts_secs
                    ELSE NULL
                END",
            rusqlite::params![key, pin_pts_secs, blob, thumb_pts_for_set],
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
                thumb_webp BLOB,
                thumb_pts_secs REAL
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

    /// 既存ピン (thumb あり) を空 thumb で上書きしたとき、pts は更新されるが
    /// thumb_webp は元の値が保持されること (UI の「常に上書きセット」動作で
    /// 新位置のサムネ未生成時に既存サムネを消さないため)。
    #[test]
    fn empty_thumb_preserves_existing() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        let original = vec![0xAA, 0xBB, 0xCC];
        db.set_pin(p, 1.0, &original).unwrap();
        // 空 thumb で上書き
        db.set_pin(p, 9.5, &[]).unwrap();
        let got = db.lookup(p).expect("present");
        // pts は新値、thumb は元のまま
        assert!((got.pin_pts_secs - 9.5).abs() < 1e-9);
        assert_eq!(got.thumb_webp, original);
    }

    /// 新規 path に空 thumb で set_pin したとき、行は入るが thumb_webp は
    /// NULL のままになること (lookup では空 Vec として返る)。
    #[test]
    fn empty_thumb_on_new_row_stores_null() {
        let db = open_in_memory();
        let p = Path::new("C:/new.mp4");
        db.set_pin(p, 3.0, &[]).unwrap();
        let got = db.lookup(p).expect("present");
        assert!((got.pin_pts_secs - 3.0).abs() < 1e-9);
        assert!(got.thumb_webp.is_empty());
    }

    #[test]
    fn case_and_separator_normalized() {
        let db = open_in_memory();
        db.set_pin(Path::new("C:\\Videos\\A.mp4"), 3.0, &[9])
            .unwrap();
        // 大文字小文字違い + スラッシュ違いでも同じレコードにヒットすること
        assert!(db.lookup(Path::new("c:/videos/a.mp4")).is_some());
    }

    /// T37: 新規 path で WebP 付きピン → thumb_pts_secs が pin_pts_secs と同じ値で記録される。
    #[test]
    fn t37_new_pin_with_webp_records_thumb_pts() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.set_pin(p, 12.5, &[1, 2, 3]).unwrap();
        let got = db.lookup(p).expect("present");
        assert_eq!(got.thumb_pts_secs, Some(12.5));
        assert!(got.thumb_is_current());
    }

    /// T37: 空 WebP で pts のみ更新したケース → thumb_pts_secs が NULL に落ち、
    /// thumb_is_current() が false (= pending) を返す。
    #[test]
    fn t37_empty_webp_clears_thumb_pts() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.set_pin(p, 1.0, &[1, 2, 3]).unwrap();
        // 新位置にピン留めしたがサムネがまだ準備中
        db.set_pin(p, 5.0, &[]).unwrap();
        let got = db.lookup(p).expect("present");
        assert!((got.pin_pts_secs - 5.0).abs() < 1e-9);
        // 旧 WebP は温存
        assert_eq!(got.thumb_webp, vec![1, 2, 3]);
        // thumb_pts_secs は NULL (= pending) → thumb_is_current は false
        assert_eq!(got.thumb_pts_secs, None);
        assert!(!got.thumb_is_current());
    }

    /// T37: pending 状態の後、worker が新 WebP を持ってきたら整合状態に戻る。
    #[test]
    fn t37_pending_resolves_when_webp_lands() {
        let db = open_in_memory();
        let p = Path::new("C:/v.mp4");
        db.set_pin(p, 1.0, &[0xAA]).unwrap();
        db.set_pin(p, 5.0, &[]).unwrap();
        // pending 中
        assert!(!db.lookup(p).unwrap().thumb_is_current());
        // worker 完了で新 WebP 入る
        db.set_pin(p, 5.0, &[0xBB]).unwrap();
        let got = db.lookup(p).unwrap();
        assert_eq!(got.thumb_pts_secs, Some(5.0));
        assert_eq!(got.thumb_webp, vec![0xBB]);
        assert!(got.thumb_is_current());
    }
}
