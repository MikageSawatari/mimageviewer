//! 画像回転情報の永続管理。
//!
//! `%APPDATA%/mimageviewer/rotation.db` に回転角度を保存する。
//! 元ファイルは変更しない（非破壊）。

use std::path::{Path, PathBuf};

/// 回転角度 (時計回り)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    None,       // 0°
    Cw90,       // 90°
    Cw180,      // 180°
    Cw270,      // 270° (= 反時計回り 90°)
}

impl Rotation {
    /// 角度値 (0, 90, 180, 270) から生成
    pub fn from_degrees(deg: i32) -> Self {
        match deg.rem_euclid(360) {
            90 => Self::Cw90,
            180 => Self::Cw180,
            270 => Self::Cw270,
            _ => Self::None,
        }
    }

    /// 角度値を返す
    pub fn degrees(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// 時計回りに 90° 加算
    pub fn rotate_cw(self) -> Self {
        match self {
            Self::None => Self::Cw90,
            Self::Cw90 => Self::Cw180,
            Self::Cw180 => Self::Cw270,
            Self::Cw270 => Self::None,
        }
    }

    /// 反時計回りに 90° 加算
    pub fn rotate_ccw(self) -> Self {
        match self {
            Self::None => Self::Cw270,
            Self::Cw90 => Self::None,
            Self::Cw180 => Self::Cw90,
            Self::Cw270 => Self::Cw180,
        }
    }

    pub fn is_none(self) -> bool {
        matches!(self, Self::None)
    }
}

/// 回転 DB ハンドル
pub struct RotationDb {
    conn: rusqlite::Connection,
}

impl RotationDb {
    /// DB を開く (なければ作成)
    pub fn open() -> Result<Self, rusqlite::Error> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS rotations (
                path TEXT PRIMARY KEY,
                angle INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        Ok(Self { conn })
    }

    /// DB ファイルのパス
    fn db_path() -> PathBuf {
        crate::data_dir::get().join("rotation.db")
    }

    /// 画像の回転角度を取得。未登録なら None。
    pub fn get(&self, path: &Path) -> Option<Rotation> {
        let key = normalize_path(path);
        let mut stmt = self
            .conn
            .prepare_cached("SELECT angle FROM rotations WHERE path = ?1")
            .ok()?;
        stmt.query_row([&key], |row| {
            let deg: i32 = row.get(0)?;
            Ok(Rotation::from_degrees(deg))
        })
        .ok()
    }

    /// 回転角度を設定する。None (0°) の場合はレコードを削除する。
    pub fn set(&self, path: &Path, rotation: Rotation) -> Result<(), rusqlite::Error> {
        let key = normalize_path(path);
        if rotation.is_none() {
            self.conn
                .execute("DELETE FROM rotations WHERE path = ?1", [&key])?;
        } else {
            self.conn.execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET angle = ?2",
                rusqlite::params![key, rotation.degrees()],
            )?;
        }
        Ok(())
    }

    /// 全レコードを削除 (リセット)
    pub fn clear_all(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM rotations", [])
    }

    /// 登録件数
    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM rotations", [], |row| row.get(0))
            .unwrap_or(0)
    }
}

/// パスを正規化 (小文字化 + バックスラッシュ→スラッシュ)
fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_degrees_roundtrip() {
        for &deg in &[0, 90, 180, 270] {
            assert_eq!(Rotation::from_degrees(deg).degrees(), deg);
        }
    }

    #[test]
    fn rotation_cw_cycle() {
        let mut r = Rotation::None;
        r = r.rotate_cw();
        assert_eq!(r, Rotation::Cw90);
        r = r.rotate_cw();
        assert_eq!(r, Rotation::Cw180);
        r = r.rotate_cw();
        assert_eq!(r, Rotation::Cw270);
        r = r.rotate_cw();
        assert_eq!(r, Rotation::None);
    }

    #[test]
    fn rotation_ccw_cycle() {
        let mut r = Rotation::None;
        r = r.rotate_ccw();
        assert_eq!(r, Rotation::Cw270);
        r = r.rotate_ccw();
        assert_eq!(r, Rotation::Cw180);
        r = r.rotate_ccw();
        assert_eq!(r, Rotation::Cw90);
        r = r.rotate_ccw();
        assert_eq!(r, Rotation::None);
    }

    #[test]
    fn db_set_get_clear() {
        let db = RotationDb::open().unwrap();
        let p = Path::new("C:/test/image.jpg");

        // 初期状態: 未登録
        assert!(db.get(p).is_none());

        // 設定
        db.set(p, Rotation::Cw90).unwrap();
        assert_eq!(db.get(p), Some(Rotation::Cw90));

        // 上書き
        db.set(p, Rotation::Cw180).unwrap();
        assert_eq!(db.get(p), Some(Rotation::Cw180));

        // None で削除
        db.set(p, Rotation::None).unwrap();
        assert!(db.get(p).is_none());
    }
}
