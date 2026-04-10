use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

const CATALOG_VERSION: &str = "2";
pub const THUMB_LONG_SIDE: u32 = 512;

// -----------------------------------------------------------------------
// DB path helpers
// -----------------------------------------------------------------------

/// ドライブ文字を除いて小文字化・スラッシュ統一したパス文字列を返す。
/// リムーバブルデバイスのドライブ文字変化に対応するため、ドライブ文字は含まない。
fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let no_drive = if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        &s[2..]
    } else {
        &s
    };
    no_drive.to_lowercase().replace('\\', "/")
}

/// `{cache_dir}/{xx}/{sha256}.db` の形式で DB ファイルパスを返す。
/// xx はハッシュ hex 先頭2文字（256サブフォルダに分散）。
pub fn db_path_for(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    let normalized = normalize_path(folder_path);
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    cache_dir.join(&hash[..2]).join(format!("{}.db", hash))
}

// -----------------------------------------------------------------------
// キャッシュエントリ
// -----------------------------------------------------------------------

pub struct CacheEntry {
    pub mtime: i64,
    pub file_size: i64,
    pub jpeg_data: Vec<u8>,
    /// 元画像のピクセル寸法 (幅, 高さ)。
    /// 旧バージョンで保存されたエントリには NULL が入るため Option で表現する。
    pub source_dims: Option<(u32, u32)>,
}

// -----------------------------------------------------------------------
// CatalogDb
// -----------------------------------------------------------------------

pub struct CatalogDb {
    conn: Mutex<Connection>,
}

impl CatalogDb {
    /// cache_dir 配下の適切な場所に DB を開く（なければ作成）。
    /// サブディレクトリも自動作成する。
    pub fn open(cache_dir: &Path, folder_path: &Path) -> rusqlite::Result<Self> {
        let db_path = db_path_for(cache_dir, folder_path);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// DB 内の全エントリを HashMap<filename, CacheEntry> として返す（一括 SELECT）。
    pub fn load_all(&self) -> rusqlite::Result<HashMap<String, CacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT filename, mtime, file_size, thumb_data, source_width, source_height \
             FROM thumbnails",
        )?;
        let mut map = HashMap::new();
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, Option<u32>>(5)?,
            ))
        })?;
        for item in iter.flatten() {
            let (filename, mtime, file_size, jpeg_data, src_w, src_h) = item;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            map.insert(
                filename,
                CacheEntry { mtime, file_size, jpeg_data, source_dims },
            );
        }
        Ok(map)
    }

    /// サムネイルを INSERT OR REPLACE で保存する。
    ///
    /// `width` / `height` はキャッシュされる WebP サムネイルの寸法、
    /// `source_dims` は元画像の寸法 (未取得なら None)。
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let src_w: Option<u32> = source_dims.map(|(w, _)| w);
        let src_h: Option<u32> = source_dims.map(|(_, h)| h);
        conn.execute(
            "INSERT OR REPLACE INTO thumbnails \
             (filename, mtime, file_size, width, height, thumb_data, source_width, source_height) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![filename, mtime, file_size, width, height, jpeg_data, src_w, src_h],
        )?;
        Ok(())
    }

    /// `existing` に含まれないファイル名の行を削除する（削除済みファイルの掃除）。
    pub fn delete_missing(&self, existing: &HashSet<String>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let db_names: Vec<String> = {
            let mut stmt = conn.prepare("SELECT filename FROM thumbnails")?;
            stmt.query_map([], |r| r.get(0))?
                .flatten()
                .collect()
        };
        for name in db_names {
            if !existing.contains(&name) {
                conn.execute(
                    "DELETE FROM thumbnails WHERE filename = ?1",
                    params![name],
                )?;
            }
        }
        Ok(())
    }
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS thumbnails (
             filename       TEXT    NOT NULL PRIMARY KEY,
             mtime          INTEGER NOT NULL,
             file_size      INTEGER NOT NULL,
             width          INTEGER NOT NULL,
             height         INTEGER NOT NULL,
             thumb_data     BLOB    NOT NULL,
             source_width   INTEGER,
             source_height  INTEGER
         );",
    )?;
    // 非破壊マイグレーション: 既存 DB で source_width/source_height が欠けていれば追加する。
    // 列が既にある場合 ALTER TABLE はエラーを返すので、結果は無視する。
    let _ = conn.execute(
        "ALTER TABLE thumbnails ADD COLUMN source_width INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE thumbnails ADD COLUMN source_height INTEGER",
        [],
    );

    // バージョン不一致（スキーマ変更）の場合は全削除して再生成
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'version'",
            [],
            |r| r.get(0),
        )
        .ok();
    if version.as_deref() != Some(CATALOG_VERSION) {
        conn.execute_batch("DELETE FROM thumbnails;")?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('version', ?1)",
            params![CATALOG_VERSION],
        )?;
    }
    Ok(())
}

// -----------------------------------------------------------------------
// WebP エンコード・デコードヘルパー
// -----------------------------------------------------------------------

/// 画像を `long_side` px にリサイズし、ロッシー WebP でエンコードする。
/// `quality` は 0.0–100.0 (JPEG の quality と同等の意味)。
/// 戻り値: (webp_bytes, width, height)
pub fn encode_thumb_webp(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    let thumb = img.resize(long_side, long_side, image::imageops::FilterType::Lanczos3);
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
    let webp_data = encoder.encode(quality.clamp(1.0, 100.0));
    Some((webp_data.to_vec(), w, h))
}

/// キャッシュされたサムネイル (WebP あるいは旧 JPEG) を egui::ColorImage にデコードする。
/// `image::load_from_memory` が自動でフォーマット判定するため両対応。
pub fn decode_thumb_to_color_image(data: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(data).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// キャッシュディレクトリのデフォルト位置（%APPDATA%\mimageviewer\cache）
pub fn default_cache_dir() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("mimageviewer").join("cache")
}

// -----------------------------------------------------------------------
// キャッシュ管理ユーティリティ
// -----------------------------------------------------------------------

/// cache_dir 配下の .db ファイル数と合計バイト数を返す。
pub fn cache_stats(cache_dir: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut total_bytes = 0u64;
    collect_db_files(cache_dir, &mut |meta| {
        count += 1;
        total_bytes += meta.len();
    });
    (count, total_bytes)
}

/// cache_dir 配下で最終更新時刻が `days` 日以上前の .db ファイルを削除する。
/// 削除したファイル数を返す。
pub fn delete_old_cache(cache_dir: &Path, days: u64) -> usize {
    let now = std::time::SystemTime::now();
    let threshold = std::time::Duration::from_secs(days * 24 * 3600);
    let mut deleted = 0usize;
    collect_db_paths(cache_dir, &mut |path, meta| {
        let age = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .unwrap_or(std::time::Duration::ZERO);
        if age >= threshold {
            if std::fs::remove_file(path).is_ok() {
                deleted += 1;
            }
        }
    });
    deleted
}

/// cache_dir 配下の .db ファイルをすべて削除する。
/// 削除したファイル数を返す。
pub fn delete_all_cache(cache_dir: &Path) -> usize {
    let mut deleted = 0usize;
    collect_db_paths(cache_dir, &mut |path, _| {
        if std::fs::remove_file(path).is_ok() {
            deleted += 1;
        }
    });
    deleted
}

/// cache_dir 配下の .db ファイルのパスとメタデータを列挙してコールバックを呼ぶ。
fn collect_db_paths(cache_dir: &Path, cb: &mut impl FnMut(&Path, std::fs::Metadata)) {
    let Ok(top) = std::fs::read_dir(cache_dir) else { return };
    for entry in top.flatten() {
        let sub = entry.path();
        if !sub.is_dir() { continue; }
        let Ok(sub_entries) = std::fs::read_dir(&sub) else { continue };
        for file in sub_entries.flatten() {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()) == Some("db") {
                if let Ok(meta) = file.metadata() {
                    cb(&p, meta);
                }
            }
        }
    }
}

/// collect_db_paths の統計専用バリアント（パス不要）。
fn collect_db_files(cache_dir: &Path, cb: &mut impl FnMut(std::fs::Metadata)) {
    collect_db_paths(cache_dir, &mut |_, meta| cb(meta));
}

// -----------------------------------------------------------------------
// テスト
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    /// テスト用: in-memory SQLite で CatalogDb を作成する。
    fn open_in_memory() -> CatalogDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .unwrap();
        init_schema(&conn).unwrap();
        CatalogDb {
            conn: Mutex::new(conn),
        }
    }

    // -- normalize_path --

    #[test]
    fn normalize_path_removes_drive_letter() {
        let p = Path::new(r"C:\Users\foo");
        assert_eq!(normalize_path(p), "/users/foo");
    }

    #[test]
    fn normalize_path_no_drive() {
        let p = Path::new(r"\already\unix");
        assert_eq!(normalize_path(p), "/already/unix");
    }

    #[test]
    fn normalize_path_backslash_to_slash() {
        let p = Path::new(r"D:\a\b\c");
        let result = normalize_path(p);
        assert!(!result.contains('\\'), "should not contain backslash: {result}");
        assert!(result.contains("/a/b/c"));
    }

    #[test]
    fn normalize_path_lowercase() {
        let p = Path::new(r"E:\MyFolder\SubDir");
        let result = normalize_path(p);
        assert_eq!(result, "/myfolder/subdir");
    }

    // -- db_path_for --

    #[test]
    fn db_path_for_deterministic() {
        let cache = Path::new(r"C:\cache");
        let folder = Path::new(r"D:\photos\2024");
        let a = db_path_for(cache, folder);
        let b = db_path_for(cache, folder);
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_different_paths() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"D:\photos\2024"));
        let b = db_path_for(cache, Path::new(r"D:\photos\2025"));
        assert_ne!(a, b);
    }

    #[test]
    fn db_path_for_case_insensitive() {
        let cache = Path::new(r"C:\cache");
        let a = db_path_for(cache, Path::new(r"C:\Photos\Vacation"));
        let b = db_path_for(cache, Path::new(r"D:\photos\vacation"));
        // ドライブ文字は除去され、小文字化されるので同じパスになるはず
        assert_eq!(a, b);
    }

    #[test]
    fn db_path_for_structure() {
        let cache = Path::new(r"C:\cache");
        let result = db_path_for(cache, Path::new(r"D:\test"));
        let result_str = result.to_string_lossy();
        // {cache_dir}/{xx}/{hash}.db の形式
        assert!(result_str.starts_with(r"C:\cache\"));
        assert!(result_str.ends_with(".db"));
        // xx サブディレクトリが2文字の hex
        let relative = result.strip_prefix(cache).unwrap();
        let components: Vec<_> = relative.components().collect();
        assert_eq!(components.len(), 2); // xx/ と hash.db
    }

    // -- CatalogDb schema --

    #[test]
    fn catalog_open_and_schema() {
        let db = open_in_memory();
        let conn = db.conn.lock().unwrap();
        // meta テーブルにバージョンが記録されているか
        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, CATALOG_VERSION);
    }

    // -- CatalogDb CRUD --

    #[test]
    fn catalog_save_and_load_all() {
        let db = open_in_memory();
        db.save("test.jpg", 1000, 2048, 256, 192, Some((4000, 3000)), b"fake_webp")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        let entry = &map["test.jpg"];
        assert_eq!(entry.mtime, 1000);
        assert_eq!(entry.file_size, 2048);
        assert_eq!(entry.jpeg_data, b"fake_webp");
        assert_eq!(entry.source_dims, Some((4000, 3000)));
    }

    #[test]
    fn catalog_save_overwrites() {
        let db = open_in_memory();
        db.save("img.jpg", 100, 500, 128, 96, None, b"data1")
            .unwrap();
        db.save("img.jpg", 200, 600, 128, 96, None, b"data2")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["img.jpg"].mtime, 200);
        assert_eq!(map["img.jpg"].jpeg_data, b"data2");
    }

    #[test]
    fn catalog_source_dims_none() {
        let db = open_in_memory();
        db.save("no_dims.jpg", 100, 500, 128, 96, None, b"data")
            .unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map["no_dims.jpg"].source_dims, None);
    }

    #[test]
    fn catalog_delete_missing() {
        let db = open_in_memory();
        db.save("keep.jpg", 100, 500, 128, 96, None, b"a").unwrap();
        db.save("remove.jpg", 200, 600, 128, 96, None, b"b").unwrap();
        db.save("also_remove.jpg", 300, 700, 128, 96, None, b"c").unwrap();

        let existing: HashSet<String> = ["keep.jpg".to_string()].into_iter().collect();
        db.delete_missing(&existing).unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("keep.jpg"));
    }

    #[test]
    fn catalog_version_mismatch_clears() {
        // 1) DB を作成してデータを保存
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO thumbnails (filename, mtime, file_size, width, height, thumb_data) \
             VALUES ('old.jpg', 1, 1, 1, 1, X'00')",
            [],
        )
        .unwrap();
        // データが存在することを確認
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // 2) バージョンを不正な値に書き換え
        conn.execute(
            "UPDATE meta SET value = 'old_version' WHERE key = 'version'",
            [],
        )
        .unwrap();

        // 3) init_schema を再度呼ぶとバージョン不一致で全削除されるはず
        init_schema(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    // -- WebP encode/decode --

    #[test]
    fn encode_thumb_webp_basic() {
        // 小さな 4x4 テスト画像を生成
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(4, 4, |x, y| {
            image::Rgb([(x * 60) as u8, (y * 60) as u8, 128])
        }));
        let result = encode_thumb_webp(&img, 4, 75.0);
        assert!(result.is_some());
        let (data, w, h) = result.unwrap();
        assert!(!data.is_empty());
        assert!(w <= 4 && h <= 4);
    }
}
