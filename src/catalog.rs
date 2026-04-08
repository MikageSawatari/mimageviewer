use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

const CATALOG_VERSION: &str = "1";
const JPEG_QUALITY: u8 = 80;
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
            "SELECT filename, mtime, file_size, thumb_data FROM thumbnails",
        )?;
        let mut map = HashMap::new();
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        for item in iter.flatten() {
            let (filename, mtime, file_size, jpeg_data) = item;
            map.insert(filename, CacheEntry { mtime, file_size, jpeg_data });
        }
        Ok(map)
    }

    /// サムネイルを INSERT OR REPLACE で保存する。
    pub fn save(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        width: u32,
        height: u32,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO thumbnails \
             (filename, mtime, file_size, width, height, thumb_data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![filename, mtime, file_size, width, height, jpeg_data],
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
             filename   TEXT    NOT NULL PRIMARY KEY,
             mtime      INTEGER NOT NULL,
             file_size  INTEGER NOT NULL,
             width      INTEGER NOT NULL,
             height     INTEGER NOT NULL,
             thumb_data BLOB    NOT NULL
         );",
    )?;
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
// JPEG エンコード・デコードヘルパー
// -----------------------------------------------------------------------

/// 画像を THUMB_LONG_SIDE px にリサイズし、JPEG q=80 でエンコードする。
/// 戻り値: (jpeg_bytes, width, height)
pub fn encode_thumb_jpeg(img: &image::DynamicImage) -> Option<(Vec<u8>, u32, u32)> {
    use image::ImageEncoder;
    let thumb = img.thumbnail(THUMB_LONG_SIDE, THUMB_LONG_SIDE);
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY)
        .write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some((buf, w, h))
}

/// JPEG バイト列を egui::ColorImage にデコードする。
pub fn jpeg_to_color_image(data: &[u8]) -> Option<egui::ColorImage> {
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
