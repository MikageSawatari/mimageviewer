use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

const CATALOG_VERSION: &str = "2";
pub const THUMB_LONG_SIDE: u32 = 512;

// -----------------------------------------------------------------------
// DB path helpers
// -----------------------------------------------------------------------

use crate::path_key;

/// `{cache_dir}/{xx}/{sha256}.db` の形式で DB ファイルパスを返す。
/// xx はハッシュ hex 先頭2文字（256サブフォルダに分散）。
pub fn db_path_for(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    let normalized = path_key::normalize(folder_path);
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    cache_dir.join(&hash[..2]).join(format!("{}.db", hash))
}

// -----------------------------------------------------------------------
// キャッシュエントリ
// -----------------------------------------------------------------------

#[derive(Clone)]
pub struct CacheEntry {
    pub mtime: i64,
    pub file_size: i64,
    pub jpeg_data: Vec<u8>,
    /// 元画像のピクセル寸法 (幅, 高さ)。
    /// 旧バージョンで保存されたエントリには NULL が入るため Option で表現する。
    pub source_dims: Option<(u32, u32)>,
}

/// 保存済みサムネのバイト列からヘッダのみで `(w, h)` を取り出す。
/// フォーマットは auto-detect (`with_guessed_format`)。これは旧バージョンが JPEG で
/// 保存していたエントリ ([`decode_thumb_to_color_image`] が "WebP or old JPEG" の
/// 両方を読んでいる) との互換性のため。フルデコードは走らない
/// (`ImageReader::into_dimensions` はチャンクヘッダだけを読む)。
pub fn decode_thumb_dims(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
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
                CacheEntry {
                    mtime,
                    file_size,
                    jpeg_data,
                    source_dims,
                },
            );
        }
        Ok(map)
    }

    /// 単一エントリのみ取り出す。`load_all` を呼ぶほどではないが特定 key だけ確認したい
    /// 場合用 (例: 仮想フォルダ進入時の親 catalog からの seed lookup)。
    pub fn load_one(&self, filename: &str) -> rusqlite::Result<Option<CacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT mtime, file_size, thumb_data, source_width, source_height \
             FROM thumbnails WHERE filename = ?1",
        )?;
        let mut iter = stmt.query_map(params![filename], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<u32>>(3)?,
                row.get::<_, Option<u32>>(4)?,
            ))
        })?;
        if let Some(item) = iter.next() {
            let (mtime, file_size, jpeg_data, src_w, src_h) = item?;
            let source_dims = match (src_w, src_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
                _ => None,
            };
            return Ok(Some(CacheEntry {
                mtime,
                file_size,
                jpeg_data,
                source_dims,
            }));
        }
        Ok(None)
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
            params![
                filename, mtime, file_size, width, height, jpeg_data, src_w, src_h
            ],
        )?;
        Ok(())
    }

    /// サムネバイト列のヘッダから `(w, h)` だけを取り出して `save` する薄いラッパ。
    /// `CacheEntry` には寸法フィールドが無いため、save 経由では `(w, h)` を呼び出し側で
    /// 用意する必要がある。仮想フォルダの seed / write-back のように「親 catalog から
    /// バイトをそのままミラーする」用途で繰り返し書きがちなので集約した。
    /// ヘッダのみ解析なのでフルデコードは走らない。
    ///
    /// 戻り値の `bool` は「実際に保存できたか」。`false` は「寸法を取り出せず保存を断念
    /// した」を意味する (= 壊れたバイト列)。呼び出し側はこれをもとに「cache_map にも
    /// 入れない」ことで、サムネ表示時に `Failed` 状態に陥るのを防げる。
    pub fn save_thumb_bytes(
        &self,
        filename: &str,
        mtime: i64,
        file_size: i64,
        source_dims: Option<(u32, u32)>,
        jpeg_data: &[u8],
    ) -> rusqlite::Result<bool> {
        let Some((w, h)) = decode_thumb_dims(jpeg_data) else {
            // 寸法が取れない (= 壊れたバイト列) なら保存を断念。SQLite スキーマ上
            // width/height は NOT NULL なので 0 を入れると整合性が壊れる。
            return Ok(false);
        };
        self.save(filename, mtime, file_size, w, h, source_dims, jpeg_data)?;
        Ok(true)
    }

    /// 単一エントリを `filename` キーで削除する。該当行が無くてもエラーにしない。
    ///
    /// 用途: フォルダ代表ピンが Video を指していたが対応する `video_pins` の WebP が
    /// 消えた / 空になった場合、`folderthumb:{dir}#pin:...` のキャッシュ行を明示的に
    /// 削除して worker を auto-pick fallback に落とすため (Codex Phase C P2 指摘)。
    pub fn delete_one(&self, filename: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM thumbnails WHERE filename = ?1",
            params![filename],
        )?;
        Ok(())
    }

    /// `existing` に含まれないファイル名の行を削除する（削除済みファイルの掃除）。
    pub fn delete_missing(&self, existing: &HashSet<String>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let db_names: Vec<String> = {
            let mut stmt = conn.prepare("SELECT filename FROM thumbnails")?;
            stmt.query_map([], |r| r.get(0))?.flatten().collect()
        };
        for name in db_names {
            if !existing.contains(&name) {
                conn.execute("DELETE FROM thumbnails WHERE filename = ?1", params![name])?;
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
    if let Err(e) = conn.execute("ALTER TABLE thumbnails ADD COLUMN source_width INTEGER", []) {
        // "duplicate column name" is expected if column already exists
        crate::logger::log(format!("catalog migration source_width: {e}"));
    }
    if let Err(e) = conn.execute(
        "ALTER TABLE thumbnails ADD COLUMN source_height INTEGER",
        [],
    ) {
        crate::logger::log(format!("catalog migration source_height: {e}"));
    }

    // バージョン不一致（スキーマ変更）の場合は全削除して再生成
    let version: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
            r.get(0)
        })
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
///
/// リサイズは SIMD 実装の `fast_image_resize` を Lanczos3 で使用する
/// (image crate のスカラー Lanczos3 より 3-5 倍速い)。
pub fn encode_thumb_webp(
    img: &image::DynamicImage,
    long_side: u32,
    quality: f32,
) -> Option<(Vec<u8>, u32, u32)> {
    let thumb = crate::fast_resize::resize_dynamic_fit(
        img,
        long_side,
        long_side,
        crate::fast_resize::Quality::Lanczos3,
    );
    let rgb = thumb.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), w, h);
    let webp_data = encoder.encode(quality.clamp(1.0, 100.0));
    Some((webp_data.to_vec(), w, h))
}

/// キャッシュされたサムネイル (WebP あるいは旧 JPEG) を egui::ColorImage にデコードする。
/// `image::load_from_memory` が自動でフォーマット判定するため両対応。
pub fn decode_thumb_to_color_image(data: &[u8]) -> Option<egui::ColorImage> {
    let (w, h, rgba) = decode_thumb_to_rgba(data)?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// `image::load_from_memory` でデコードして RGBA8 + (w, h) を返す。
/// `decode_thumb_to_color_image` と動画タイル サムネ cache の WebP 復元で共用。
pub fn decode_thumb_to_rgba(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some((w, h, rgba.into_raw()))
}

/// キャッシュディレクトリのデフォルト位置（DATA_DIR\cache）
pub fn default_cache_dir() -> PathBuf {
    crate::data_dir::get().join("cache")
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
    let Ok(top) = std::fs::read_dir(cache_dir) else {
        return;
    };
    for entry in top.flatten() {
        // per-entry GetFileAttributes syscall を避けるため file_type を 1 回取る
        // (docs/ui-responsiveness.md §4)。キャッシュ全走査は数千フォルダ規模になるので効く。
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let sub = entry.path();
        let Ok(sub_entries) = std::fs::read_dir(&sub) else {
            continue;
        };
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
        db.save(
            "test.jpg",
            1000,
            2048,
            256,
            192,
            Some((4000, 3000)),
            b"fake_webp",
        )
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
        db.save("remove.jpg", 200, 600, 128, 96, None, b"b")
            .unwrap();
        db.save("also_remove.jpg", 300, 700, 128, 96, None, b"c")
            .unwrap();

        let existing: HashSet<String> = ["keep.jpg".to_string()].into_iter().collect();
        db.delete_missing(&existing).unwrap();

        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("keep.jpg"));
    }

    #[test]
    fn catalog_delete_one_removes_only_target() {
        let db = open_in_memory();
        db.save("a.jpg", 1, 10, 8, 8, None, b"a").unwrap();
        db.save("b.jpg", 1, 10, 8, 8, None, b"b").unwrap();
        db.delete_one("a.jpg").unwrap();
        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("b.jpg"));
        assert!(!map.contains_key("a.jpg"));
        // 二度目の delete (存在しないキー) もエラーにしない
        db.delete_one("a.jpg").unwrap();
        db.delete_one("never_existed.jpg").unwrap();
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

    /// `collect_db_paths` が `cache_dir/<sub>/*.db` を網羅すること。
    /// `cache_dir/file.db` (top-level) は subdir でないので **無視**、
    /// 非 .db ファイル / 余計なフォルダの中の非 .db も無視。
    /// docs/ui-responsiveness.md §4 (file_type 経由) との整合を機能面から保証する。
    #[test]
    fn collect_db_paths_enumerates_only_subdir_db_files() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        // sub1: foo.db + readme.txt
        let sub1 = cache_dir.join("sub1");
        std::fs::create_dir_all(&sub1).unwrap();
        std::fs::write(sub1.join("foo.db"), b"x").unwrap();
        std::fs::write(sub1.join("readme.txt"), b"x").unwrap();
        // sub2: bar.db
        let sub2 = cache_dir.join("sub2");
        std::fs::create_dir_all(&sub2).unwrap();
        std::fs::write(sub2.join("bar.db"), b"x").unwrap();
        // top-level の loose db (subdir に居ない) は拾わない
        std::fs::write(cache_dir.join("loose.db"), b"x").unwrap();
        // 空サブフォルダは無害
        std::fs::create_dir_all(cache_dir.join("empty_sub")).unwrap();

        let mut found: Vec<String> = Vec::new();
        super::collect_db_paths(&cache_dir, &mut |p, _meta| {
            found.push(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
            );
        });
        found.sort();
        assert_eq!(
            found,
            vec!["bar.db".to_string(), "foo.db".to_string()],
            "subdir 配下の .db のみ列挙、top-level loose.db は無視"
        );
    }

    /// `collect_db_paths` は cache_dir 自体が存在しない場合に panic せず、
    /// 単にコールバックを呼ばずに return する (`std::fs::read_dir` Err 時の規約)。
    #[test]
    fn collect_db_paths_handles_missing_cache_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let nonexistent = temp.path().join("does_not_exist");
        let mut count = 0usize;
        super::collect_db_paths(&nonexistent, &mut |_, _| count += 1);
        assert_eq!(count, 0, "missing cache_dir なら空列挙");
    }

    /// 大量サブフォルダ (200 件) でも全 .db ファイルを取りこぼさず列挙する。
    /// 実時間 assert は flaky になるので、件数だけ厳密に確認 (file_type 経路で
    /// per-entry syscall が発生していないことの間接担保)。
    #[test]
    fn collect_db_paths_handles_many_subfolders() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        for i in 0..200 {
            let sub = cache_dir.join(format!("s{i:03}"));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("a.db"), b"x").unwrap();
        }
        let mut count = 0usize;
        super::collect_db_paths(&cache_dir, &mut |_, _| count += 1);
        assert_eq!(count, 200, "200 件全部列挙");
    }

    /// 0.8.2 で `decode_thumb_dims` を WebP 固定から `with_guessed_format()` auto-detect
    /// に変更した回帰ガード。ここで JPEG が読めなくなると、旧バージョンが JPEG で書いた
    /// 親 catalog エントリから seed/writeback できなくなる (= 仮想フォルダの初回 thumb
    /// が永続的に失われる)。
    #[test]
    fn decode_thumb_dims_reads_webp_jpeg_and_rejects_garbage() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(8, 6, |x, y| {
            image::Rgb([(x * 30) as u8, (y * 40) as u8, 200])
        }));

        // WebP (現行フォーマット): 寸法を返す
        let (webp_bytes, _, _) = encode_thumb_webp(&img, 8, 75.0).expect("webp encode ok");
        assert_eq!(decode_thumb_dims(&webp_bytes), Some((8, 6)));

        // JPEG (旧バージョンが書いていた形式): 寸法を返す
        let mut jpeg_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut jpeg_bytes),
            image::ImageFormat::Jpeg,
        )
        .expect("jpeg encode");
        assert_eq!(decode_thumb_dims(&jpeg_bytes), Some((8, 6)));

        // 破損データ: None。空バイト列・テキスト・WebP magic だけ・短縮 JPEG いずれも reject。
        assert_eq!(decode_thumb_dims(&[]), None);
        assert_eq!(decode_thumb_dims(b"NOT-AN-IMAGE-AT-ALL"), None);
        // RIFF/WEBP magic の手前 12 バイトだけ (本体なし)
        assert_eq!(
            decode_thumb_dims(b"RIFF\x00\x00\x00\x00WEBP"),
            None,
            "header だけで本体なし → None"
        );
        // JPEG SOI のみ (SOF0 まで届かない)
        assert_eq!(decode_thumb_dims(b"\xFF\xD8\xFF\xE0"), None);
    }
}
