//! 親コンテナ (フォルダ / ZIP / PDF) の代表サムネを手動で固定するためのピン DB。
//!
//! `%APPDATA%/mimageviewer/folder_thumb_pins.db` に「ユーザーが選んだ代表アイテム」
//! を保存する。代表サムネ生成の優先順位は次のとおり (Phase B 以降で配線):
//!
//! 1. 手動ピン (本 DB) — ★最優先
//! 2. 自動選定 (`resolve_folder_thumb_image`)
//! 3. フォルダ / ZIP / PDF アイコン fallback
//!
//! # スキーマ
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS folder_thumb_pins (
//!     container_key TEXT PRIMARY KEY,  -- 親フォルダ / ZIP / PDF のパス、normalize_keep_drive 済み
//!     source_kind   TEXT NOT NULL,     -- "image"/"video"/"folder"/"zipfile"/"pdffile"/"zipentry"/"pdfpage"
//!     source_rel    TEXT NOT NULL,     -- container 相対パス (zipentry/pdfpage で container 自身を指すときは空)
//!     source_entry  TEXT,              -- source_kind = "zipentry" のときの ZIP エントリ名
//!     source_page   INTEGER            -- source_kind = "pdfpage" のときのページ番号 (0-indexed)
//! );
//! ```
//!
//! # セキュリティ
//!
//! `source_rel` は **container 相対パス**として保存し、絶対パス・`..` セグメント・
//! Windows ドライブ修飾子 (`C:`) を書き込み時に拒否する。読み出し時にも同じ
//! 検査を二重で行い、DB が手書きで汚染されていても解決パスがコンテナ外に
//! 出ないようにする。

use rusqlite::{Connection, Result as SqlResult, params};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// ピン対象のうち通常ファイル / フォルダ系の種別。
/// (ZipImage / PdfPage は `FolderPinSource::ZipEntry` / `PdfPage` に分離している)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Image,
    Video,
    Folder,
    ZipFile,
    PdfFile,
}

impl FileKind {
    pub fn as_db_str(self) -> &'static str {
        match self {
            FileKind::Image => "image",
            FileKind::Video => "video",
            FileKind::Folder => "folder",
            FileKind::ZipFile => "zipfile",
            FileKind::PdfFile => "pdffile",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "image" => FileKind::Image,
            "video" => FileKind::Video,
            "folder" => FileKind::Folder,
            "zipfile" => FileKind::ZipFile,
            "pdffile" => FileKind::PdfFile,
            _ => return None,
        })
    }
}

/// container 内のどのアイテムを代表サムネに使うかを示すソース指定。
///
/// `rel` 系フィールドは **container 相対パス**。`..` / 絶対パス / ドライブ修飾を
/// 含まない (書き込み時 / 読み出し時の両方で検査)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderPinSource {
    /// container 直下 or サブディレクトリ内の通常ファイル / フォルダ。
    ///
    /// `rel` は container からの相対パス (forward-slash 正規化)。
    File { rel: String, kind: FileKind },
    /// ZIP 内の画像エントリ。
    ///
    /// `zip_rel` が空文字なら container 自身が対象 ZIP (= ZIP を内側から閲覧中に
    /// ピン留めしたケース)。非空なら container 直下 / サブの ZIP ファイル。
    ZipEntry { zip_rel: String, entry: String },
    /// PDF 内のページ。
    ///
    /// `pdf_rel` が空文字なら container 自身が対象 PDF (= PDF を内側から閲覧中に
    /// ピン留めしたケース)。非空なら container 直下 / サブの PDF ファイル。
    PdfPage { pdf_rel: String, page: u32 },
}

impl FolderPinSource {
    /// この source の DB 上の `source_kind` 文字列を返す。
    pub fn db_kind(&self) -> &'static str {
        match self {
            FolderPinSource::File { kind, .. } => kind.as_db_str(),
            FolderPinSource::ZipEntry { .. } => "zipentry",
            FolderPinSource::PdfPage { .. } => "pdfpage",
        }
    }

    /// container 相対パス本体 (DB 上の `source_rel`)。
    pub fn rel(&self) -> &str {
        match self {
            FolderPinSource::File { rel, .. } => rel,
            FolderPinSource::ZipEntry { zip_rel, .. } => zip_rel,
            FolderPinSource::PdfPage { pdf_rel, .. } => pdf_rel,
        }
    }
}

#[derive(Debug)]
pub enum FolderPinError {
    /// `rel` が空 (File variant のみ。ZipEntry/PdfPage の zip_rel/pdf_rel は空可)。
    EmptyRelPath { kind: &'static str },
    /// 絶対パス (`/foo` や `\foo`)。
    AbsolutePath(String),
    /// `..` セグメントを含む。
    ParentTraversal(String),
    /// Windows ドライブ修飾子 (`C:`, `C:foo` 等) を含む。
    DriveLetter(String),
    /// ZIP entry 名が空。
    EmptyZipEntry,
    /// SQLite エラー。
    Sql(rusqlite::Error),
}

impl std::fmt::Display for FolderPinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FolderPinError::EmptyRelPath { kind } => write!(
                f,
                "relative path is empty for {kind} (only zipentry / pdfpage with container-self target allow empty)"
            ),
            FolderPinError::AbsolutePath(p) => write!(f, "absolute path is not allowed: {p}"),
            FolderPinError::ParentTraversal(p) => {
                write!(f, "parent directory traversal (..) is not allowed: {p}")
            }
            FolderPinError::DriveLetter(p) => write!(f, "drive letter prefix is not allowed: {p}"),
            FolderPinError::EmptyZipEntry => write!(f, "zip entry name is empty"),
            FolderPinError::Sql(e) => write!(f, "sql error: {e}"),
        }
    }
}

impl std::error::Error for FolderPinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FolderPinError::Sql(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for FolderPinError {
    fn from(e: rusqlite::Error) -> Self {
        FolderPinError::Sql(e)
    }
}

/// ピン DB ハンドル。
pub struct FolderThumbPinDb {
    conn: Connection,
}

impl FolderThumbPinDb {
    /// DB を開く (なければ作成)。
    pub fn open() -> SqlResult<Self> {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("folder_thumb_pins.db")
    }

    fn init_schema(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folder_thumb_pins (
                container_key TEXT PRIMARY KEY,
                source_kind   TEXT NOT NULL,
                source_rel    TEXT NOT NULL,
                source_entry  TEXT,
                source_page   INTEGER
            )",
        )
    }

    fn container_key(container: &Path) -> String {
        crate::path_key::normalize_keep_drive(container)
    }

    /// 単一 container 用ピンを取得 (なければ `None`)。
    pub fn lookup(&self, container: &Path) -> Option<FolderPinSource> {
        let key = Self::container_key(container);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT source_kind, source_rel, source_entry, source_page \
                 FROM folder_thumb_pins WHERE container_key = ?1",
            )
            .ok()?;
        let row = stmt
            .query_row([&key], |row| {
                let kind: String = row.get(0)?;
                let rel: String = row.get(1)?;
                let entry: Option<String> = row.get(2)?;
                let page: Option<i64> = row.get(3)?;
                Ok((kind, rel, entry, page))
            })
            .ok()?;
        decode_row(&row.0, &row.1, row.2.as_deref(), row.3)
    }

    /// 複数 container 分のピンをまとめて取得する。`load_folder` で子セル分を
    /// N+1 を避けてバッチ取得するための API。
    ///
    /// 戻り値は **container パスの `normalize_keep_drive` キー → source** map。
    /// 呼び出し側は同じキーで lookup する想定。
    pub fn lookup_many<I, P>(&self, containers: I) -> HashMap<String, FolderPinSource>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let keys: Vec<String> = containers
            .into_iter()
            .map(|p| Self::container_key(p.as_ref()))
            .collect();
        if keys.is_empty() {
            return HashMap::new();
        }
        // SQLite IN リストを placeholder で組む (?1, ?2, ...)。
        // 親フォルダ 1 つ分の子セル数なら数百〜千程度を想定。SQLite の限界
        // (デフォルト SQLITE_MAX_VARIABLE_NUMBER = 32766) には十分収まる。
        let mut sql = String::from(
            "SELECT container_key, source_kind, source_rel, source_entry, source_page \
             FROM folder_thumb_pins WHERE container_key IN (",
        );
        for i in 0..keys.len() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
        }
        sql.push(')');

        let mut out = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return out;
        };
        let params: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
        let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let key: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let rel: String = row.get(2)?;
            let entry: Option<String> = row.get(3)?;
            let page: Option<i64> = row.get(4)?;
            Ok((key, kind, rel, entry, page))
        }) else {
            return out;
        };
        for r in rows.flatten() {
            if let Some(src) = decode_row(&r.1, &r.2, r.3.as_deref(), r.4) {
                out.insert(r.0, src);
            }
        }
        out
    }

    /// ピンを書き込む / 上書きする。
    pub fn set(&self, container: &Path, source: &FolderPinSource) -> Result<(), FolderPinError> {
        validate_source(source)?;
        let key = Self::container_key(container);
        let rel_norm = source.rel().replace('\\', "/");
        let (entry, page): (Option<&str>, Option<i64>) = match source {
            FolderPinSource::File { .. } => (None, None),
            FolderPinSource::ZipEntry { entry, .. } => (Some(entry.as_str()), None),
            FolderPinSource::PdfPage { page, .. } => (None, Some(*page as i64)),
        };
        self.conn.execute(
            "INSERT INTO folder_thumb_pins (container_key, source_kind, source_rel, source_entry, source_page) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(container_key) DO UPDATE SET \
                source_kind  = excluded.source_kind, \
                source_rel   = excluded.source_rel, \
                source_entry = excluded.source_entry, \
                source_page  = excluded.source_page",
            params![key, source.db_kind(), rel_norm, entry, page],
        )?;
        Ok(())
    }

    /// ピンを削除する (該当行が無くてもエラーにはならない)。
    pub fn remove(&self, container: &Path) -> Result<(), rusqlite::Error> {
        let key = Self::container_key(container);
        self.conn.execute(
            "DELETE FROM folder_thumb_pins WHERE container_key = ?1",
            [&key],
        )?;
        Ok(())
    }
}

/// DB 行から `FolderPinSource` を組み立てる。検査に通らない行は `None`。
fn decode_row(
    kind: &str,
    rel: &str,
    entry: Option<&str>,
    page: Option<i64>,
) -> Option<FolderPinSource> {
    let src = match kind {
        "zipentry" => {
            let entry = entry?.to_string();
            if entry.is_empty() {
                return None;
            }
            FolderPinSource::ZipEntry {
                zip_rel: rel.to_string(),
                entry,
            }
        }
        "pdfpage" => {
            let page = page?;
            if page < 0 || page > u32::MAX as i64 {
                return None;
            }
            FolderPinSource::PdfPage {
                pdf_rel: rel.to_string(),
                page: page as u32,
            }
        }
        other => {
            let file_kind = FileKind::from_db_str(other)?;
            FolderPinSource::File {
                rel: rel.to_string(),
                kind: file_kind,
            }
        }
    };
    validate_source(&src).ok()?;
    Some(src)
}

/// `FolderPinSource` を validate する (set 時 / lookup 時で共通)。
fn validate_source(source: &FolderPinSource) -> Result<(), FolderPinError> {
    match source {
        FolderPinSource::File { rel, kind } => {
            if rel.is_empty() {
                return Err(FolderPinError::EmptyRelPath {
                    kind: kind.as_db_str(),
                });
            }
            validate_rel(rel)
        }
        FolderPinSource::ZipEntry { zip_rel, entry } => {
            if entry.is_empty() {
                return Err(FolderPinError::EmptyZipEntry);
            }
            if zip_rel.is_empty() {
                // container 自身が ZIP のケースは許可
                Ok(())
            } else {
                validate_rel(zip_rel)
            }
        }
        FolderPinSource::PdfPage { pdf_rel, .. } => {
            if pdf_rel.is_empty() {
                Ok(())
            } else {
                validate_rel(pdf_rel)
            }
        }
    }
}

/// container 相対パスとして安全か検査する。
fn validate_rel(rel: &str) -> Result<(), FolderPinError> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(FolderPinError::AbsolutePath(rel.to_string()));
    }
    // Windows ドライブ修飾子 (`C:`, `C:foo`) 検出。
    // Path::is_absolute は `C:relative` (root なし) を絶対と扱わないので
    // 文字列ベースで弾く。
    if rel.len() >= 2 {
        let bytes = rel.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Err(FolderPinError::DriveLetter(rel.to_string()));
        }
    }
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                return Err(FolderPinError::ParentTraversal(rel.to_string()));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(FolderPinError::AbsolutePath(rel.to_string()));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory() -> FolderThumbPinDb {
        let conn = Connection::open_in_memory().expect("memory db");
        FolderThumbPinDb::init_schema(&conn).expect("schema");
        FolderThumbPinDb { conn }
    }

    #[test]
    fn lookup_missing_returns_none() {
        let db = open_in_memory();
        assert!(db.lookup(Path::new("C:/no/such/folder")).is_none());
    }

    #[test]
    fn roundtrip_image_file() {
        let db = open_in_memory();
        let container = Path::new("C:/Albums/Trip");
        let src = FolderPinSource::File {
            rel: "cover.jpg".to_string(),
            kind: FileKind::Image,
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn roundtrip_zipentry_in_subzip() {
        let db = open_in_memory();
        let container = Path::new("C:/Albums/Trip");
        let src = FolderPinSource::ZipEntry {
            zip_rel: "scans.zip".to_string(),
            entry: "page-01.png".to_string(),
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn roundtrip_zipentry_in_container_itself() {
        // container 自身が ZIP で、その中の entry を pin したケース
        let db = open_in_memory();
        let container = Path::new("C:/Albums/Trip.zip");
        let src = FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "page-05.png".to_string(),
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn roundtrip_pdfpage_in_subpdf() {
        let db = open_in_memory();
        let container = Path::new("C:/Docs/Magazines");
        let src = FolderPinSource::PdfPage {
            pdf_rel: "issue42.pdf".to_string(),
            page: 7,
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn roundtrip_pdfpage_in_container_itself() {
        let db = open_in_memory();
        let container = Path::new("C:/Docs/issue42.pdf");
        let src = FolderPinSource::PdfPage {
            pdf_rel: String::new(),
            page: 3,
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn roundtrip_folder_source() {
        let db = open_in_memory();
        let container = Path::new("C:/Albums");
        let src = FolderPinSource::File {
            rel: "2024/Trip".to_string(),
            kind: FileKind::Folder,
        };
        db.set(container, &src).unwrap();
        assert_eq!(db.lookup(container), Some(src));
    }

    #[test]
    fn set_overwrites_existing() {
        let db = open_in_memory();
        let container = Path::new("C:/Albums/Trip");
        db.set(
            container,
            &FolderPinSource::File {
                rel: "first.jpg".to_string(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        let new = FolderPinSource::File {
            rel: "better.jpg".to_string(),
            kind: FileKind::Image,
        };
        db.set(container, &new).unwrap();
        assert_eq!(db.lookup(container), Some(new));
    }

    #[test]
    fn remove_deletes_row() {
        let db = open_in_memory();
        let container = Path::new("C:/Albums/Trip");
        db.set(
            container,
            &FolderPinSource::File {
                rel: "cover.jpg".to_string(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        assert!(db.lookup(container).is_some());
        db.remove(container).unwrap();
        assert!(db.lookup(container).is_none());
    }

    #[test]
    fn remove_missing_is_noop() {
        let db = open_in_memory();
        db.remove(Path::new("C:/no/such/folder")).unwrap();
    }

    #[test]
    fn container_key_case_and_separator_normalized() {
        let db = open_in_memory();
        let src = FolderPinSource::File {
            rel: "cover.jpg".to_string(),
            kind: FileKind::Image,
        };
        db.set(Path::new(r"C:\Albums\Trip"), &src).unwrap();
        // 大文字小文字違い + フォワードスラッシュ違いでもヒットする
        assert!(db.lookup(Path::new("c:/albums/trip")).is_some());
        assert!(db.lookup(Path::new(r"c:\Albums/TRIP")).is_some());
    }

    #[test]
    fn set_rejects_empty_rel_for_file() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: String::new(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::EmptyRelPath { .. }));
    }

    #[test]
    fn set_rejects_absolute_path() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "/etc/passwd".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_drive_letter() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "C:/Windows/System32/cmd.exe".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        // 先頭がドライブ修飾子なので AbsolutePath か DriveLetter のどちらかでヒット
        assert!(matches!(
            err,
            FolderPinError::AbsolutePath(_) | FolderPinError::DriveLetter(_)
        ));
    }

    #[test]
    fn set_rejects_bare_drive_letter() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "C:".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::DriveLetter(_)));
    }

    #[test]
    fn set_rejects_parent_traversal_leading() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "../up.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::ParentTraversal(_)));
    }

    #[test]
    fn set_rejects_parent_traversal_middle() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "sub/../../up.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::ParentTraversal(_)));
    }

    #[test]
    fn set_rejects_empty_zip_entry() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::ZipEntry {
                    zip_rel: "scans.zip".to_string(),
                    entry: String::new(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::EmptyZipEntry));
    }

    #[test]
    fn set_normalizes_backslashes_in_rel() {
        let db = open_in_memory();
        db.set(
            Path::new("C:/Albums/Trip"),
            &FolderPinSource::File {
                rel: r"sub\dir\img.jpg".to_string(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        let got = db.lookup(Path::new("C:/Albums/Trip")).unwrap();
        // forward-slash に正規化されている
        assert_eq!(
            got,
            FolderPinSource::File {
                rel: "sub/dir/img.jpg".to_string(),
                kind: FileKind::Image,
            }
        );
    }

    #[test]
    fn lookup_rejects_corrupt_rel_in_db() {
        // DB が手書きで書き換えられたケースを模擬: 直接 INSERT で `../` を入れる。
        // lookup 側は decode_row で validate_source を再走するので None を返す。
        let db = open_in_memory();
        db.conn
            .execute(
                "INSERT INTO folder_thumb_pins (container_key, source_kind, source_rel, source_entry, source_page) \
                 VALUES (?1, ?2, ?3, NULL, NULL)",
                params!["c:/albums/trip", "image", "../up.jpg"],
            )
            .unwrap();
        assert!(db.lookup(Path::new("C:/Albums/Trip")).is_none());
    }

    #[test]
    fn lookup_many_returns_only_existing() {
        let db = open_in_memory();
        let a = Path::new("C:/Albums/A");
        let b = Path::new("C:/Albums/B");
        let c = Path::new("C:/Albums/C");
        db.set(
            a,
            &FolderPinSource::File {
                rel: "a.jpg".into(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        db.set(
            c,
            &FolderPinSource::File {
                rel: "c.jpg".into(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        let got = db.lookup_many([a, b, c]);
        assert_eq!(got.len(), 2);
        assert!(got.contains_key("c:/albums/a"));
        assert!(got.contains_key("c:/albums/c"));
        assert!(!got.contains_key("c:/albums/b"));
    }

    #[test]
    fn lookup_many_empty_input_returns_empty() {
        let db = open_in_memory();
        let empty: [&Path; 0] = [];
        assert!(db.lookup_many(empty).is_empty());
    }

    #[test]
    fn file_kind_db_str_roundtrip() {
        for k in [
            FileKind::Image,
            FileKind::Video,
            FileKind::Folder,
            FileKind::ZipFile,
            FileKind::PdfFile,
        ] {
            assert_eq!(FileKind::from_db_str(k.as_db_str()), Some(k));
        }
        assert_eq!(FileKind::from_db_str("unknown"), None);
    }
}
