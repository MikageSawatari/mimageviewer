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

/// `GridItem` から `FolderPinSource` を構築する。
///
/// `container` は item の親 (= 現在表示中のフォルダ / ZIP / PDF) の絶対パス。
/// item が container 自身を指している場合 (= ZipImage / PdfPage の zip_path /
/// pdf_path が container と一致) は `zip_rel` / `pdf_rel` を空文字にする。
///
/// 返り値:
/// - `Some(source)`: ピン留め可能な item
/// - `None`: ピン留め不可 (`ConvertibleArchive` / `SearchContainer` / `ZipSeparator`
///   や relative path が取れないケース)
pub fn source_from_grid_item(
    container: &Path,
    item: &crate::grid_item::GridItem,
) -> Option<FolderPinSource> {
    use crate::grid_item::GridItem;
    match item {
        GridItem::Image(p) => Some(FolderPinSource::File {
            rel: relative_path_string(container, p)?,
            kind: FileKind::Image,
        }),
        GridItem::Video(p) => Some(FolderPinSource::File {
            rel: relative_path_string(container, p)?,
            kind: FileKind::Video,
        }),
        GridItem::Folder(p) => Some(FolderPinSource::File {
            rel: relative_path_string(container, p)?,
            kind: FileKind::Folder,
        }),
        GridItem::ZipFile(p) => Some(FolderPinSource::File {
            rel: relative_path_string(container, p)?,
            kind: FileKind::ZipFile,
        }),
        GridItem::PdfFile(p) => Some(FolderPinSource::File {
            rel: relative_path_string(container, p)?,
            kind: FileKind::PdfFile,
        }),
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            // container == zip_path: ZIP を仮想フォルダとして開いた状態で
            // 中のエントリをピンする (zip_rel = "")。
            // それ以外: 通常はあり得ない (regular フォルダの items に ZipImage は
            // 入らない)。安全側で None。
            let zip_rel = if paths_equal(zip_path, container) {
                String::new()
            } else {
                return None;
            };
            if entry_name.is_empty() {
                return None;
            }
            Some(FolderPinSource::ZipEntry {
                zip_rel,
                entry: entry_name.clone(),
            })
        }
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => {
            let pdf_rel = if paths_equal(pdf_path, container) {
                String::new()
            } else {
                return None;
            };
            Some(FolderPinSource::PdfPage {
                pdf_rel,
                page: *page_num,
            })
        }
        // ConvertibleArchive: 7z/LZH は変換完了前に thumb 生成できないので
        // UI 側で disabled + tooltip 表示する (本関数は None を返すだけ)
        GridItem::ConvertibleArchive { .. } => None,
        // ピン対象として意味がないもの
        GridItem::SearchContainer { .. } | GridItem::ZipSeparator { .. } => None,
    }
}

/// container 相対の forward-slash 区切り文字列に正規化する。
/// container と target が同一パスのときは `None` (= 自分自身は pin できない)。
fn relative_path_string(container: &Path, target: &Path) -> Option<String> {
    let rel = target.strip_prefix(container).ok()?;
    let s = rel.to_string_lossy();
    if s.is_empty() {
        return None;
    }
    Some(s.replace('\\', "/"))
}

/// Windows パス比較 (大文字小文字 + slash/backslash 違いを吸収) で同一判定する。
fn paths_equal(a: &Path, b: &Path) -> bool {
    crate::path_key::normalize_keep_drive(a) == crate::path_key::normalize_keep_drive(b)
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
    ///
    /// 入力は重複除去してから 500 件ずつ chunked に IN クエリへ流す
    /// (SQLite の式ツリー上限・準備済みステートメントのバインド数上限の両方を
    /// 安全側で避けるため。`rating_db::get_many` と同じ規模感)。
    pub fn lookup_many<I, P>(&self, containers: I) -> HashMap<String, FolderPinSource>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut keys: Vec<String> = containers
            .into_iter()
            .map(|p| Self::container_key(p.as_ref()))
            .collect();
        if keys.is_empty() {
            return HashMap::new();
        }
        keys.sort_unstable();
        keys.dedup();

        let mut out = HashMap::new();
        for chunk in keys.chunks(500) {
            let placeholders = (0..chunk.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT container_key, source_kind, source_rel, source_entry, source_page \
                 FROM folder_thumb_pins WHERE container_key IN ({placeholders})"
            );
            let mut stmt = match self.conn.prepare(&sql) {
                Ok(s) => s,
                Err(e) => {
                    crate::logger::log(format!(
                        "folder_thumb_pins.lookup_many: prepare failed: {e}"
                    ));
                    continue;
                }
            };
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let key: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let rel: String = row.get(2)?;
                let entry: Option<String> = row.get(3)?;
                let page: Option<i64> = row.get(4)?;
                Ok((key, kind, rel, entry, page))
            });
            match rows {
                Ok(iter) => {
                    for r in iter.flatten() {
                        if let Some(src) = decode_row(&r.1, &r.2, r.3.as_deref(), r.4) {
                            out.insert(r.0, src);
                        }
                    }
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "folder_thumb_pins.lookup_many: query_map failed: {e}"
                    ));
                }
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

/// `FolderPinSource` を実際の target ファイル情報に解決した結果。
/// `make_load_request` から `process_load_request` に渡す情報を組み立てる。
///
/// `mtime` / `file_size` は **target ファイル自身** の metadata。これを
/// `LoadRequest::mtime/file_size` にコピーすると、catalog の hit 判定が
/// 「target が変わったら自動で miss」になる (ピン書き換え / 解除でも同様)。
#[derive(Clone, Debug)]
pub struct ResolvedPinTarget {
    pub kind: ResolvedKind,
    pub abs_path: PathBuf,
    pub zip_entry: Option<String>,
    pub pdf_page: Option<u32>,
    pub mtime: i64,
    pub file_size: i64,
    /// pin の identity を表す compact 文字列。cache key suffix として
    /// 親キー (`folderthumb:{dirname}` 等) の後ろに `#pin:` で連結する。
    /// pin の付け替え / target ファイル変更で自動的に変わるので、古い
    /// pin の WebP を catch しない。
    pub source_id: String,
}

/// pin target の dispatch 種別。`LoadRequest::resolve_override` と 1 対 1 に対応。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedKind {
    /// 画像ファイル直接 decode
    Image,
    /// 動画ファイル (Phase B では fallback、Phase C で video worker 配線)
    Video,
    /// サブフォルダ。`resolve_folder_thumb_image` で代表画像を選び decode
    /// (recursion clip: そのサブフォルダの pin は引かない)
    Folder,
    /// ZIP ファイル直下の 1 枚目 (`read_first_image_bytes`)
    ZipFirstImage,
    /// PDF ファイルの page 0
    PdfFirstPage,
    /// 既知 ZIP entry を直接 decode
    ZipEntry,
    /// 既知 PDF page を render
    PdfPage,
}

/// container と source から target を解決し、metadata を読み取って
/// `ResolvedPinTarget` を返す。
///
/// target が存在しない / stat できないときは `None` を返し、呼び出し側は
/// **無効なピン**として無視して auto-select に fall back する。
///
/// この関数は `std::fs::metadata` を 1 回呼ぶ (= cheap stat syscall)。
/// UI スレッドからの呼び出しを想定。
pub fn resolve_pin_target(container: &Path, source: &FolderPinSource) -> Option<ResolvedPinTarget> {
    let (abs_path, zip_entry, pdf_page, kind) = match source {
        FolderPinSource::File { rel, kind } => {
            let abs = container.join(rel);
            let (rk, page) = match kind {
                FileKind::Image => (ResolvedKind::Image, None),
                FileKind::Video => (ResolvedKind::Video, None),
                FileKind::Folder => (ResolvedKind::Folder, None),
                FileKind::ZipFile => (ResolvedKind::ZipFirstImage, None),
                FileKind::PdfFile => (ResolvedKind::PdfFirstPage, Some(0u32)),
            };
            (abs, None, page, rk)
        }
        FolderPinSource::ZipEntry { zip_rel, entry } => {
            let abs_zip = if zip_rel.is_empty() {
                container.to_path_buf()
            } else {
                container.join(zip_rel)
            };
            (abs_zip, Some(entry.clone()), None, ResolvedKind::ZipEntry)
        }
        FolderPinSource::PdfPage { pdf_rel, page } => {
            let abs_pdf = if pdf_rel.is_empty() {
                container.to_path_buf()
            } else {
                container.join(pdf_rel)
            };
            (abs_pdf, None, Some(*page), ResolvedKind::PdfPage)
        }
    };

    let meta = std::fs::metadata(&abs_path).ok()?;
    let mtime = crate::ui_helpers::mtime_secs(&meta);
    let file_size = meta.len() as i64;
    // 注: ZipEntry / PdfPage の場合、ここで取る mtime/file_size は **container 全体**
    // の値 (ZIP/PDF ファイル自身)。非ピン経路の ZipImage は entry の uncompressed size と
    // ZIP の mtime を使うのに対し、ピン経路は粒度が粗い (= container 全体が変わったとき
    // だけ再生成)。ZIP / PDF が部分書き換えされても mtime が動くので実害は限定的、
    // entry 単位の granularity が必要なケースが出てきたら entry metadata を別途
    // 取得する経路を追加する (Codex Phase B P3 指摘)。

    // source_id: cache key の suffix として使う。フォーマット例:
    //   "image|cover.jpg|-|-|1700000000|524288"
    //   "pdfpage|sub.pdf|-|42|1700000000|1048576"
    //   "zipentry|scans.zip|page-01.png|-|1700000000|2097152"
    // pin/unpin/target 変更で必ず変わるので、古い pin の WebP を取り違えない。
    let entry_part = match source {
        FolderPinSource::ZipEntry { entry, .. } => entry.as_str(),
        _ => "-",
    };
    let page_part = match source {
        FolderPinSource::PdfPage { page, .. } => page.to_string(),
        _ => "-".to_string(),
    };
    let rel_part = source.rel();
    let source_id = format!(
        "{kind}|{rel}|{entry}|{page}|{mtime}|{size}",
        kind = source.db_kind(),
        rel = rel_part,
        entry = entry_part,
        page = page_part,
        mtime = mtime,
        size = file_size,
    );

    Some(ResolvedPinTarget {
        kind,
        abs_path,
        zip_entry,
        pdf_page,
        mtime,
        file_size,
        source_id,
    })
}

/// DB 行から `FolderPinSource` を組み立てる。検査に通らない行は `None` を返し、
/// **どの行をなぜ skip したか**を `logger` 経由でログに出す (= ユーザーが DB を
/// 手書きで触ったり、将来のスキーマ拡張で互換性が崩れた際に診断できるように)。
fn decode_row(
    kind: &str,
    rel: &str,
    entry: Option<&str>,
    page: Option<i64>,
) -> Option<FolderPinSource> {
    let src = match kind {
        "zipentry" => {
            let entry = match entry {
                Some(e) if !e.is_empty() => e.to_string(),
                _ => {
                    crate::logger::log(format!(
                        "folder_thumb_pins: skipping zipentry row with empty/NULL entry (rel={rel:?})"
                    ));
                    return None;
                }
            };
            FolderPinSource::ZipEntry {
                zip_rel: rel.to_string(),
                entry,
            }
        }
        "pdfpage" => {
            let page = match page {
                Some(p) if (0..=u32::MAX as i64).contains(&p) => p as u32,
                _ => {
                    crate::logger::log(format!(
                        "folder_thumb_pins: skipping pdfpage row with invalid page (rel={rel:?} page={page:?})"
                    ));
                    return None;
                }
            };
            FolderPinSource::PdfPage {
                pdf_rel: rel.to_string(),
                page,
            }
        }
        other => match FileKind::from_db_str(other) {
            Some(file_kind) => FolderPinSource::File {
                rel: rel.to_string(),
                kind: file_kind,
            },
            None => {
                crate::logger::log(format!(
                    "folder_thumb_pins: skipping row with unknown source_kind={other:?}"
                ));
                return None;
            }
        },
    };
    if let Err(e) = validate_source(&src) {
        crate::logger::log(format!(
            "folder_thumb_pins: skipping row that fails validation: {e}"
        ));
        return None;
    }
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

    /// path 検査: Codex P3 (2026-05-13) 指摘の Windows 特有のエッジケースを網羅。
    /// UNC, verbatim, device, rooted, drive-relative, trailing slash, CurDir 単体。
    #[test]
    fn set_rejects_unc_backslash() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: r"\\server\share\file.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_unc_forward_slash() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "//server/share/file.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_rooted_backslash() {
        // `\foo` は Windows では "rooted but not absolute"。Component::RootDir で弾く。
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: r"\foo.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_verbatim_prefix() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: r"\\?\C:\Windows\foo.exe".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_device_prefix() {
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: r"\\.\PhysicalDrive0".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::AbsolutePath(_)));
    }

    #[test]
    fn set_rejects_drive_relative_with_subpath() {
        // `C:foo` は drive-relative (= プロセスの C: のカレントからの相対)。
        // 危険なので拒否。
        let db = open_in_memory();
        let err = db
            .set(
                Path::new("C:/Albums/Trip"),
                &FolderPinSource::File {
                    rel: "C:foo.jpg".to_string(),
                    kind: FileKind::Image,
                },
            )
            .unwrap_err();
        assert!(matches!(err, FolderPinError::DriveLetter(_)));
    }

    #[test]
    fn set_allows_trailing_separator() {
        // `sub/dir/` は単に Normal + Normal なので OK (decode 側で trailing は無視される)。
        let db = open_in_memory();
        db.set(
            Path::new("C:/Albums/Trip"),
            &FolderPinSource::File {
                rel: "sub/dir/".to_string(),
                kind: FileKind::Folder,
            },
        )
        .unwrap();
        assert!(db.lookup(Path::new("C:/Albums/Trip")).is_some());
    }

    #[test]
    fn set_allows_dot_segment_in_middle() {
        // `./foo.jpg` = `foo.jpg`。CurDir は no-op で許可。
        let db = open_in_memory();
        db.set(
            Path::new("C:/Albums/Trip"),
            &FolderPinSource::File {
                rel: "./foo.jpg".to_string(),
                kind: FileKind::Image,
            },
        )
        .unwrap();
        assert!(db.lookup(Path::new("C:/Albums/Trip")).is_some());
    }

    /// 一時ディレクトリに実ファイルを作って resolve_pin_target が
    /// 絶対パス / metadata / source_id を返すことを確認する。
    #[test]
    fn resolve_pin_target_image_returns_target_metadata() {
        use std::io::Write;

        let tmp = std::env::temp_dir().join(format!("miv_pin_resolve_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let img = tmp.join("cover.jpg");
        let mut f = std::fs::File::create(&img).unwrap();
        f.write_all(b"fake-jpeg-bytes").unwrap();
        drop(f);

        let source = FolderPinSource::File {
            rel: "cover.jpg".to_string(),
            kind: FileKind::Image,
        };
        let resolved = resolve_pin_target(&tmp, &source).expect("target exists");
        assert_eq!(resolved.kind, ResolvedKind::Image);
        assert_eq!(resolved.abs_path, img);
        assert_eq!(resolved.zip_entry, None);
        assert_eq!(resolved.pdf_page, None);
        assert_eq!(resolved.file_size, b"fake-jpeg-bytes".len() as i64);
        assert!(resolved.source_id.starts_with("image|cover.jpg|-|-|"));
        // source_id に mtime と size が入っている
        assert!(
            resolved
                .source_id
                .ends_with(&format!("|{}", b"fake-jpeg-bytes".len()))
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_pin_target_missing_returns_none() {
        let tmp =
            std::env::temp_dir().join(format!("miv_pin_resolve_missing_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let source = FolderPinSource::File {
            rel: "nope.jpg".to_string(),
            kind: FileKind::Image,
        };
        assert!(resolve_pin_target(&tmp, &source).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_pin_target_pdfpage_carries_page_number() {
        use std::io::Write;
        let tmp =
            std::env::temp_dir().join(format!("miv_pin_resolve_pdfpage_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let pdf = tmp.join("doc.pdf");
        std::fs::File::create(&pdf)
            .unwrap()
            .write_all(b"%PDF-1.4 dummy")
            .unwrap();
        let source = FolderPinSource::PdfPage {
            pdf_rel: "doc.pdf".to_string(),
            page: 7,
        };
        let resolved = resolve_pin_target(&tmp, &source).unwrap();
        assert_eq!(resolved.kind, ResolvedKind::PdfPage);
        assert_eq!(resolved.pdf_page, Some(7));
        assert!(resolved.source_id.contains("|7|"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_pin_target_zipentry_in_container_uses_container_path() {
        use std::io::Write;
        let tmp =
            std::env::temp_dir().join(format!("miv_pin_resolve_zipentry_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let zip = tmp.join("scans.zip");
        std::fs::File::create(&zip)
            .unwrap()
            .write_all(b"PK\x03\x04 dummy")
            .unwrap();
        let source = FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "p01.png".to_string(),
        };
        // container 自身が ZIP のケース
        let resolved = resolve_pin_target(&zip, &source).unwrap();
        assert_eq!(resolved.kind, ResolvedKind::ZipEntry);
        assert_eq!(resolved.abs_path, zip);
        assert_eq!(resolved.zip_entry.as_deref(), Some("p01.png"));
        assert!(resolved.source_id.starts_with("zipentry||p01.png|-|"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn source_from_grid_item_image_in_folder() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Users\me\Photos\Vacation");
        let item = GridItem::Image(container.join("cover.jpg"));
        let src = source_from_grid_item(container, &item).expect("pinnable");
        match src {
            FolderPinSource::File { rel, kind } => {
                assert_eq!(rel, "cover.jpg");
                assert_eq!(kind, FileKind::Image);
            }
            _ => panic!("expected File source"),
        }
    }

    #[test]
    fn source_from_grid_item_subfolder_image_normalized_to_forward_slash() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Users\me\Photos");
        let item = GridItem::Image(container.join("Vacation").join("Day1").join("img.png"));
        let src = source_from_grid_item(container, &item).expect("pinnable");
        if let FolderPinSource::File { rel, kind } = src {
            assert_eq!(rel, "Vacation/Day1/img.png");
            assert_eq!(kind, FileKind::Image);
        } else {
            panic!("expected File source");
        }
    }

    #[test]
    fn source_from_grid_item_zipimage_inside_zip_uses_empty_rel() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Archives\book.zip");
        let item = GridItem::ZipImage {
            zip_path: container.to_path_buf(),
            entry_name: "scan/01.jpg".to_string(),
        };
        let src = source_from_grid_item(container, &item).expect("pinnable");
        match src {
            FolderPinSource::ZipEntry { zip_rel, entry } => {
                assert_eq!(zip_rel, "");
                assert_eq!(entry, "scan/01.jpg");
            }
            _ => panic!("expected ZipEntry source"),
        }
    }

    #[test]
    fn source_from_grid_item_zipimage_with_foreign_container_returns_none() {
        // 通常 path 上は発生しないが、container と zip_path が不一致なら None
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Photos");
        let item = GridItem::ZipImage {
            zip_path: PathBuf::from(r"C:\Archives\book.zip"),
            entry_name: "01.jpg".to_string(),
        };
        assert!(source_from_grid_item(container, &item).is_none());
    }

    #[test]
    fn source_from_grid_item_pdfpage_inside_pdf_uses_empty_rel() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Docs\paper.pdf");
        let item = GridItem::PdfPage {
            pdf_path: container.to_path_buf(),
            page_num: 7,
            content_type: None,
        };
        let src = source_from_grid_item(container, &item).expect("pinnable");
        match src {
            FolderPinSource::PdfPage { pdf_rel, page } => {
                assert_eq!(pdf_rel, "");
                assert_eq!(page, 7);
            }
            _ => panic!("expected PdfPage source"),
        }
    }

    #[test]
    fn source_from_grid_item_convertible_archive_returns_none() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Downloads");
        let item = GridItem::ConvertibleArchive {
            path: container.join("scan.7z"),
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        };
        assert!(source_from_grid_item(container, &item).is_none());
    }

    #[test]
    fn source_from_grid_item_search_container_returns_none() {
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Photos");
        let item = GridItem::SearchContainer {
            path: container.join("inner"),
            kind: crate::grid_item::SearchContainerKind::Folder,
            hit_count: 1,
            representative: None,
        };
        assert!(source_from_grid_item(container, &item).is_none());
    }

    #[test]
    fn source_from_grid_item_case_insensitive_path_match_for_zip() {
        // container と zip_path で大文字小文字違い → Windows 上は同一として扱い
        // zip_rel = "" になる (paths_equal が normalize_keep_drive 比較)
        use crate::grid_item::GridItem;
        let container = Path::new(r"C:\Archives\Book.ZIP");
        let item = GridItem::ZipImage {
            zip_path: PathBuf::from(r"C:\archives\book.zip"),
            entry_name: "01.jpg".to_string(),
        };
        let src = source_from_grid_item(container, &item).expect("pinnable");
        match src {
            FolderPinSource::ZipEntry { zip_rel, entry } => {
                assert_eq!(zip_rel, "");
                assert_eq!(entry, "01.jpg");
            }
            _ => panic!("expected ZipEntry source"),
        }
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
