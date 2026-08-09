//! RAR / 7z / LZH / (入れ子アーカイブ入り) ZIP を閲覧用 ZIP (STORE) に変換するコンバータ。
//!
//! mImageViewer は ZIP での閲覧に最適化されているため、RAR / 7z / LZH をクリックしたら
//! 中身の画像だけを抜き出して無圧縮 ZIP に変換しておき、以降は通常の ZIP として開く。
//!
//! - 対応: RAR (unrar), 7z (sevenz-rust2), LZH (delharc), ZIP (zip クレート、
//!   入れ子に非 ZIP アーカイブを含む場合のみ変換経路に乗る)。
//! - 出力: 常に STORE モード (中身は既に JPEG/PNG で圧縮済み、再圧縮無意味)。
//! - 対象: 画像エントリ (`folder_tree::is_recognized_image_ext`、Susie プラグイン対応拡張子も含む) のみ。非画像は破棄。
//! - **入れ子アーカイブは再帰展開** (v1.3.0): アーカイブ内の ZIP/CBZ/RAR/CBR/7z/CB7/LZH/LHA は
//!   一時ファイルへ取り出して中の画像を `"<アーカイブ名>/<内側パス>"` のフラットなエントリ名で
//!   出力 ZIP に書く (例: `"books/inner.rar/p01.jpg"`)。ネスト ZIP ツリー表示
//!   ([`crate::zip_tree`]) はエントリ名を `/` で split するので、入れ子アーカイブが
//!   そのまま「本」ノードになる。読み戻しは literal なフルネーム一致
//!   (`zip_loader::read_entry_bytes` の exact-name fallback) で解決される。
//!   深さ上限 [`MAX_NESTED_ARCHIVE_DEPTH`]。閲覧キャッシュ変換では壊れた / パスワード付き
//!   入れ子をログして skip する。明示 sibling / batch 変換では厳格に失敗させる。
//! - キャンセル: `Arc<AtomicBool>` を各エントリ境界でチェック。
//! - 進捗: `Fn(ConvertProgress)` コールバック。`files_total` は入れ子アーカイブを
//!   展開するたびに増える (事前スキャンでは入れ子の中身を数えないため)。
//!
//! キャッシュ管理は [`archive_cache`] 側の責務で、本モジュールは純粋な変換ロジックのみ。

use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::folder_tree::{is_recognized_image_ext, path_eq};

/// 入れ子アーカイブ展開の最大深さ。外側アーカイブ直下の入れ子が depth=1。
/// 異常な多重ネスト (アーカイブ爆弾) で変換が暴走しないための上限で、
/// 超えた入れ子はログして skip する。
pub const MAX_NESTED_ARCHIVE_DEPTH: u32 = 8;

/// 1 エントリ (画像 or 入れ子アーカイブ) の展開後サイズ上限。圧縮率の高い細工
/// エントリが GB 級に膨らむ「展開爆弾」で disk / RAM を枯渇させないためのガード。
/// 正規の高解像度画像・入れ子アーカイブでも到達しない余裕を持たせた固定値
/// (実行時の空き容量で変えない = deterministic over adaptive)。
const MAX_SINGLE_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB

/// 変換出力 (cache ZIP) の累積サイズ上限。入れ子再帰で多数のエントリが積み上がっても
/// 出力全体がこの値を超えたら中断する。
const MAX_CONVERT_OUTPUT_BYTES: u64 = 32 * 1024 * 1024 * 1024; // 32 GiB

/// `r` から最大 `limit` バイトまで `w` へストリーミングコピーする。`limit` を超える入力が
/// あれば [`ConvertError::TooLarge`] を返す (展開爆弾ガード)。`limit + 1` まで読んで超過を
/// 検出するので、`w` へ書かれるのは最大 `limit + 1` バイト (呼び出し側が abort / 破棄する)。
fn copy_capped<R: Read + ?Sized, W: Write + ?Sized>(
    r: &mut R,
    w: &mut W,
    limit: u64,
) -> Result<u64, ConvertError> {
    let mut limited = r.take(limit.saturating_add(1));
    let copied = std::io::copy(&mut limited, w).map_err(ConvertError::Io)?;
    if copied > limit {
        return Err(ConvertError::TooLarge);
    }
    Ok(copied)
}

/// 変換対応アーカイブ形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Rar,
    SevenZ,
    Lzh,
    /// ZIP/CBZ。通常はそのまま開くので `from_extension` には含めない (クリックで
    /// 変換ダイアログに誘導しない)。**入れ子に非 ZIP アーカイブを含む ZIP** を列挙時に
    /// 検出したときだけ、この形式で変換経路に乗せる (v1.3.0)。
    Zip,
}

impl ArchiveFormat {
    /// 拡張子から「クリック時に変換ダイアログへ誘導する」形式を判定する。
    /// 大文字小文字無視。対応外なら None。**ZIP/CBZ は含めない** (通常はネイティブの
    /// ZIP 閲覧経路で開くため。`GridItem::ConvertibleArchive` の分類にも使われるので、
    /// ここに zip を足すと ZIP がすべて変換扱いになってしまう)。
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "rar" | "cbr" => Some(Self::Rar),
            "7z" | "cb7" => Some(Self::SevenZ),
            "lzh" | "lha" => Some(Self::Lzh),
            _ => None,
        }
    }

    /// 拡張子から「入れ子アーカイブとして再帰展開する」形式を判定する (ZIP/CBZ を含む)。
    pub fn nested_from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "zip" | "cbz" => Some(Self::Zip),
            other => Self::from_extension(other),
        }
    }

    /// 一時ファイルに付ける拡張子 (sniffing がパス名に依存するリーダー対策)。
    fn temp_suffix(self) -> &'static str {
        match self {
            Self::Rar => ".rar",
            Self::SevenZ => ".7z",
            Self::Lzh => ".lzh",
            Self::Zip => ".zip",
        }
    }

    /// 形式のラベル (バッジ / ダイアログ表示用)。
    pub fn label(self) -> &'static str {
        match self {
            Self::Rar => "RAR",
            Self::SevenZ => "7z",
            Self::Lzh => "LZH",
            Self::Zip => "ZIP",
        }
    }
}

/// `path` が分割 RAR の先頭パート以外に見えるかを、ファイル名規則だけで判定する。
///
/// `unrar` の命名規則は通常の `Vol.2.rar` などにも一致するため、この結果を一覧除外や
/// 読込先変更の確定判定に使ってはならない。worker で RAR header の `volume_info()` を
/// 確認すべき候補を絞る用途にだけ使う。`.cbr` は単体 RAR として扱う。
pub fn looks_like_non_first_rar_part(path: &Path) -> bool {
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rar"))
    {
        return false;
    }
    let archive = unrar::Archive::new(path);
    archive.is_multipart() && !path_eq(path, &archive.first_part())
}

/// 事前スキャンで得られるアーカイブ内画像の概要。変換前の確認ダイアログ表示用。
#[derive(Debug, Clone, Copy)]
pub struct ArchiveImageSummary {
    /// 画像エントリの数 (非画像・ディレクトリを除く)。**入れ子アーカイブの中身は
    /// 含まない** (スキャンは入れ子を展開しない。変換時に追加で発見される)。
    pub image_count: u32,
    /// 画像エントリの非圧縮バイト総和 (変換後 ZIP サイズの目安。入れ子の中身は含まない)
    pub total_uncompressed_bytes: u64,
    /// 入れ子アーカイブ (ZIP/CBZ/RAR/CBR/7z/CB7/LZH/LHA) のエントリ数。
    /// 変換時に再帰展開され、中の画像が追加される (確認ダイアログの注記用)。
    pub nested_archive_count: u32,
}

/// 変換中の進捗情報。ダイアログへ `Fn(ConvertProgress)` で通知する。
#[derive(Debug, Clone, Copy)]
pub struct ConvertProgress {
    /// 書き込み完了した画像数
    pub files_done: u32,
    /// 予想される画像総数 (事前スキャン結果と一致させる)
    pub files_total: u32,
    /// 書き込んだバイト数 (ZIP ヘッダ等は含まない、本体のみの目安)
    pub bytes_written: u64,
}

/// ZIP 変換の publish / 完全性検証オプション。
///
/// 閲覧キャッシュなど従来経路は [`Self::default`] (上書き可・寛容変換・検証なし)。
/// ユーザーが元アーカイブを削除し得る明示 sibling / batch 変換だけ
/// `no_clobber: true, verify: true` にする。`verify` は変換中の厳格モードも兼ねる。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConvertOptions {
    pub no_clobber: bool,
    pub verify: bool,
}

/// 変換失敗理由。
#[derive(Debug)]
pub enum ConvertError {
    Io(std::io::Error),
    /// ユーザーキャンセルによる中断
    Cancelled,
    /// この形式の reader へパスワードを渡せば再試行できる
    PasswordRequired,
    /// この形式の reader へ渡したパスワードが正しくなかったため再入力できる
    BadPassword,
    /// 暗号化を識別できたが、この形式の reader にはパスワード入力手段がない
    PasswordUnsupported,
    /// アーカイブ解析・展開時のエラー (ライブラリ依存のメッセージを文字列化)
    Archive(String),
    /// 画像エントリが 0 件だった
    NoImages,
    /// 展開後サイズが上限を超えた (展開爆弾ガード)。Cancelled / Io と同じく
    /// **入れ子でも skip せず伝播** させて変換全体を中断する。
    TooLarge,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O エラー: {e}"),
            Self::Cancelled => write!(f, "キャンセルされました"),
            Self::PasswordRequired => write!(f, "パスワードが必要です"),
            Self::BadPassword => write!(f, "パスワードが正しくありません"),
            Self::PasswordUnsupported => {
                write!(f, "この形式のパスワード付きアーカイブには対応していません")
            }
            Self::Archive(s) => write!(f, "アーカイブエラー: {s}"),
            Self::NoImages => write!(f, "画像ファイルが含まれていません"),
            Self::TooLarge => write!(f, "展開後サイズが上限を超えました"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<std::io::Error> for ConvertError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// エントリ名 (アーカイブ内相対パス) が画像拡張子か判定する。
///
/// `folder_tree::is_recognized_image_ext` を用いるので、ネイティブ対応拡張子に加え
/// ロード済み Susie プラグインの対応拡張子 (PI / MAG / Q0 等) も画像として扱う。
/// これによりフォルダ列挙・本表示での認識と RAR/7z/LZH 変換対象が一致する。
pub(crate) fn is_image_entry(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    // パス区切り後に '.' があることを確認 (ディレクトリパス中の '.' を誤検出しない)
    let last_sep = name
        .rfind(|c: char| c == '/' || c == '\\')
        .map_or(0, |i| i + 1);
    if dot < last_sep {
        return false;
    }
    let ext = name[dot + 1..].to_ascii_lowercase();
    is_recognized_image_ext(&ext)
}

/// エントリ名 (アーカイブ内相対パス) が再帰展開対象の入れ子アーカイブか判定する。
pub(crate) fn nested_archive_kind(name: &str) -> Option<ArchiveFormat> {
    let dot = name.rfind('.')?;
    let last_sep = name
        .rfind(|c: char| c == '/' || c == '\\')
        .map_or(0, |i| i + 1);
    if dot < last_sep {
        return None;
    }
    ArchiveFormat::nested_from_extension(&name[dot + 1..])
}

/// `zip_loader::should_ignore` と同じ除外規則 (macOS の `__MACOSX/` リソース等)。
/// 変換出力に書くエントリと通常 ZIP 閲覧で見えるエントリを一致させる
/// (変換キャッシュ側だけ macOS ゴミが「本」として現れる不整合を防ぐ)。
pub(crate) fn should_ignore_entry(name: &str) -> bool {
    name.contains("__MACOSX/") || name.starts_with('.')
}

/// エントリ名を ZIP 標準 (区切り '/') に正規化し、危険なパスを排除する。
/// - `\` → `/`
/// - 先頭 `/` を除去
/// - `..` を含むパスは拒否 (zip-slip 対策)
fn normalize_entry_name(raw: &str) -> Option<String> {
    let s = raw.replace('\\', "/");
    let s = s.trim_start_matches('/');
    if s.is_empty() || s.ends_with('/') {
        return None;
    }
    for comp in s.split('/') {
        if comp == ".." || comp == "." {
            return None;
        }
    }
    Some(s.to_string())
}

// ──────────────────────────────────────────────────────────────────────
// 事前スキャン (ダイアログ表示用)
// ──────────────────────────────────────────────────────────────────────

/// アーカイブ内の画像エントリを列挙して概要を返す。変換は行わない。
/// 確認ダイアログで「画像 N 枚、約 X MB」を表示するために使う。
pub fn scan_summary(
    path: &Path,
    format: ArchiveFormat,
) -> Result<ArchiveImageSummary, ConvertError> {
    scan_summary_with_password(path, format, None)
}

pub fn scan_summary_with_password(
    path: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let cancel = AtomicBool::new(false);
    scan_summary_with_password_cancelable(path, format, password, &cancel)
}

/// Cancellable variant used by the archive-open lifecycle. Archive libraries may perform a
/// bounded amount of work while opening headers, then every enumeration step checks `cancel`.
pub fn scan_summary_with_password_cancelable(
    path: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
    cancel: &AtomicBool,
) -> Result<ArchiveImageSummary, ConvertError> {
    check_scan_cancel(cancel)?;
    match format {
        ArchiveFormat::Rar => scan_summary_rar(path, password, cancel),
        ArchiveFormat::SevenZ => scan_summary_7z(path, cancel),
        ArchiveFormat::Lzh => scan_summary_lzh(path, cancel),
        ArchiveFormat::Zip => scan_summary_zip(path, cancel),
    }
}

fn check_scan_cancel(cancel: &AtomicBool) -> Result<(), ConvertError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConvertError::Cancelled)
    } else {
        Ok(())
    }
}

fn rar_error(e: unrar::error::UnrarError) -> ConvertError {
    match e.code {
        unrar::error::Code::MissingPassword => ConvertError::PasswordRequired,
        unrar::error::Code::BadPassword => ConvertError::BadPassword,
        _ => ConvertError::Archive(format!("RAR: {e}")),
    }
}

fn sevenz_error(error: sevenz_rust2::Error) -> ConvertError {
    match error {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
            ConvertError::PasswordUnsupported
        }
        error => ConvertError::Archive(error.to_string()),
    }
}

fn lzh_error(error: impl std::fmt::Display) -> ConvertError {
    // delharc exposes neither password input nor a typed encryption signal. In particular, do not
    // infer PasswordRequired from library error text: LZH failures are not retryable with a secret.
    ConvertError::Archive(error.to_string())
}

fn rar_archive<'a>(
    path: &'a Path,
    password: Option<&'a str>,
) -> Result<unrar::Archive<'a>, ConvertError> {
    match password {
        Some(password) => {
            if password.as_bytes().contains(&0) {
                return Err(ConvertError::BadPassword);
            }
            Ok(unrar::Archive::with_password(path, password))
        }
        None => Ok(unrar::Archive::new(path)),
    }
}

fn open_rar_listing(
    path: &Path,
    password: Option<&str>,
) -> Result<unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>, ConvertError> {
    let archive = rar_archive(path, password)?;
    let first_part = archive.first_part();
    let opened = archive.open_for_listing().map_err(rar_error)?;
    if opened.volume_info() != unrar::VolumeInfo::Subsequent {
        return Ok(opened);
    }
    drop(opened);
    rar_archive(&first_part, password)?
        .open_for_listing()
        .map_err(rar_error)
}

pub(crate) fn resolve_rar_source_path(
    path: &Path,
    password: Option<&str>,
) -> Result<std::path::PathBuf, ConvertError> {
    let archive = rar_archive(path, password)?;
    let first_part = archive.first_part();
    let opened = archive.open_for_listing().map_err(rar_error)?;
    let resolved = if opened.volume_info() == unrar::VolumeInfo::Subsequent {
        first_part
    } else {
        path.to_path_buf()
    };
    Ok(resolved)
}

fn open_rar_processing(
    path: &Path,
    password: Option<&str>,
) -> Result<unrar::OpenArchive<unrar::Process, unrar::CursorBeforeHeader>, ConvertError> {
    let resolved = resolve_rar_source_path(path, password)?;
    rar_archive(&resolved, password)?
        .open_for_processing()
        .map_err(rar_error)
}

fn scan_summary_rar(
    path: &Path,
    password: Option<&str>,
    cancel: &AtomicBool,
) -> Result<ArchiveImageSummary, ConvertError> {
    check_scan_cancel(cancel)?;
    let mut archive = open_rar_listing(path, password)?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    let mut nested = 0u32;
    loop {
        check_scan_cancel(cancel)?;
        let Some(entry) = archive.next() else {
            break;
        };
        let entry = entry.map_err(rar_error)?;
        let name = entry.filename.to_string_lossy();
        if !entry.is_file() || should_ignore_entry(&name.replace('\\', "/")) {
            continue;
        }
        if is_image_entry(&name) {
            count += 1;
            bytes = bytes.saturating_add(entry.unpacked_size);
        } else if nested_archive_kind(&name).is_some() {
            nested += 1;
        }
    }
    check_scan_cancel(cancel)?;
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
        nested_archive_count: nested,
    })
}

fn scan_summary_7z(path: &Path, cancel: &AtomicBool) -> Result<ArchiveImageSummary, ConvertError> {
    check_scan_cancel(cancel)?;
    let reader =
        sevenz_rust2::ArchiveReader::open(path, Default::default()).map_err(sevenz_error)?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    let mut nested = 0u32;
    for entry in &reader.archive().files {
        check_scan_cancel(cancel)?;
        if entry.is_directory || should_ignore_entry(&entry.name.replace('\\', "/")) {
            continue;
        }
        if is_image_entry(&entry.name) {
            count += 1;
            bytes = bytes.saturating_add(entry.size);
        } else if nested_archive_kind(&entry.name).is_some() {
            nested += 1;
        }
    }
    check_scan_cancel(cancel)?;
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
        nested_archive_count: nested,
    })
}

fn scan_summary_lzh(path: &Path, cancel: &AtomicBool) -> Result<ArchiveImageSummary, ConvertError> {
    // delharc 0.6.x has no password input and exposes no typed encryption error/flag. Keep every
    // LZH failure non-retryable (`Archive`); never infer a password prompt from error text.
    check_scan_cancel(cancel)?;
    let mut reader = delharc::parse_file(path).map_err(lzh_error)?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    let mut nested = 0u32;
    loop {
        check_scan_cancel(cancel)?;
        let header = reader.header();
        let pathname = header.parse_pathname();
        let name = pathname.to_string_lossy();
        if !header.is_directory() && !should_ignore_entry(&name.replace('\\', "/")) {
            if is_image_entry(&name) {
                count += 1;
                bytes = bytes.saturating_add(header.original_size);
            } else if nested_archive_kind(&name).is_some() {
                nested += 1;
            }
        }
        check_scan_cancel(cancel)?;
        if !reader.next_file().map_err(lzh_error)? {
            break;
        }
    }
    check_scan_cancel(cancel)?;
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
        nested_archive_count: nested,
    })
}

/// ZIP (入れ子に非 ZIP アーカイブを含むもの) の事前スキャン。トップレベルの
/// central directory だけを見る (入れ子は展開しない = 安価)。`image_count` は
/// 直下 (ネスト ZIP の外) の画像数、`nested_archive_count` は入れ子アーカイブ数。
fn scan_summary_zip(path: &Path, cancel: &AtomicBool) -> Result<ArchiveImageSummary, ConvertError> {
    check_scan_cancel(cancel)?;
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| ConvertError::Archive(e.to_string()))?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    let mut nested = 0u32;
    for i in 0..archive.len() {
        check_scan_cancel(cancel)?;
        // by_index_raw は伸長しない (central directory のメタ読みだけで安価)。
        let Ok(entry) = archive.by_index_raw(i) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if should_ignore_entry(&name) {
            continue;
        }
        if is_image_entry(&name) {
            count += 1;
            bytes = bytes.saturating_add(entry.size());
        } else if nested_archive_kind(&name).is_some() {
            // 入れ子の中身サイズは展開するまで分からないので、入れ子アーカイブ自体の
            // 非圧縮サイズを概算として足す (STORE 出力サイズの下限見積もり)。
            nested += 1;
            bytes = bytes.saturating_add(entry.size());
        }
    }
    check_scan_cancel(cancel)?;
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
        nested_archive_count: nested,
    })
}

// ──────────────────────────────────────────────────────────────────────
// 変換本体
// ──────────────────────────────────────────────────────────────────────

/// 変換を実行し、`dst` に STORE モードの ZIP を生成する。
///
/// - 既に `dst` が存在する場合は上書き。
/// - 失敗 / キャンセル時は既存の `dst` を保持し、途中生成物だけを削除する。
/// - `cancel` は各エントリ境界でチェックする。キャンセル検出時は `ConvertError::Cancelled`。
/// - `progress` が `Some` の場合、各ファイル処理完了後に呼ぶ。
pub fn convert_to_zip(
    src: &Path,
    dst: &Path,
    format: ArchiveFormat,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    convert_to_zip_with_password(
        src,
        dst,
        format,
        None,
        cancel,
        progress,
        ConvertOptions::default(),
    )
}

/// `options.no_clobber` が `true` のとき、publish 時に `dst` が既に存在すれば上書きせず
/// エラーで返す。`options.verify` が `true` のとき、入力展開を厳格化し、完成した tmp ZIP の
/// 全画像を publish 前に開き直して CRC 検証付きで読み切る。
pub fn convert_to_zip_with_password(
    src: &Path,
    dst: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
    options: ConvertOptions,
) -> Result<ArchiveImageSummary, ConvertError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 中間ファイルに書いて atomic rename。途中失敗時に壊れた zip が残らないようにする。
    let tmp_path = dst.with_extension("zip.part");
    let _ = std::fs::remove_file(&tmp_path);
    // 早期 return / panic のいずれでも .part を残さない RAII ガード。publish 成功後だけ
    // disarm する (成功時は tmp が消費済みなので掃除不要)。
    let mut tmp_guard = TmpCleanup::armed(&tmp_path);

    let summary = do_convert(
        src,
        &tmp_path,
        format,
        password,
        cancel,
        progress,
        options.verify,
    )?;

    if summary.image_count == 0 {
        return Err(ConvertError::NoImages);
    }

    if options.verify {
        verify_output_zip(&tmp_path, summary.image_count, cancel)?;
    }

    // publish 直前の最終キャンセルチェック。検証中 / 検証後にキャンセルされたのに
    // ZIP を公開してしまわないようにする (TmpCleanup が tmp を掃除する)。
    if cancel.load(Ordering::Relaxed) {
        return Err(ConvertError::Cancelled);
    }

    if let Err(e) = replace_file_atomic(&tmp_path, dst, options.no_clobber) {
        // no_clobber の非置換 move が「既存 dst」で失敗した場合は、汎用 Io ではなく
        // 意味の分かるメッセージで返す (publish 直前に同名 zip が現れたレース)。
        if options.no_clobber && dst.exists() {
            return Err(ConvertError::Archive(
                "同名の ZIP が既に存在するため上書きしませんでした".to_string(),
            ));
        }
        return Err(ConvertError::Io(e));
    }
    tmp_guard.disarm();
    Ok(summary)
}

fn verify_output_zip(
    tmp_path: &Path,
    expected_image_count: u32,
    cancel: &AtomicBool,
) -> Result<(), ConvertError> {
    let file = match std::fs::File::open(tmp_path) {
        Ok(file) => file,
        Err(e) => return Err(output_verification_error(format!("ZIP を開けません: {e}"))),
    };
    let mut archive = match zip::ZipArchive::new(std::io::BufReader::new(file)) {
        Ok(archive) => archive,
        Err(e) => {
            return Err(output_verification_error(format!(
                "ZIP を解析できません: {e}"
            )));
        }
    };

    let mut image_count = 0u32;
    for i in 0..archive.len() {
        // 大きな ZIP の再読み込みは時間がかかるので、各エントリ境界でキャンセルを見る。
        if cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        let mut entry = match archive.by_index(i) {
            Ok(entry) => entry,
            Err(e) => {
                return Err(output_verification_error(format!(
                    "エントリ {i} を開けません: {e}"
                )));
            }
        };
        if !entry.is_file() || !is_image_entry(entry.name()) {
            continue;
        }
        image_count = image_count.saturating_add(1);
        let name = entry.name().to_string();
        if let Err(e) = std::io::copy(&mut entry, &mut std::io::sink()) {
            return Err(output_verification_error(format!(
                "{name} を最後まで読み取れません: {e}"
            )));
        }
    }

    if image_count != expected_image_count {
        return Err(output_verification_error(format!(
            "画像数が一致しません (変換時 {expected_image_count} 件、検証時 {image_count} 件)"
        )));
    }
    Ok(())
}

fn output_verification_error(detail: impl std::fmt::Display) -> ConvertError {
    ConvertError::Archive(format!("変換結果の検証に失敗しました: {detail}"))
}

/// 変換中の中間ファイル (`*.zip.part`) を drop 時に削除する RAII ガード。
/// publish 成功後は [`Self::disarm`] で解除する。早期 return や panic の際に
/// 巨大な `.part` を残さないため (Codex P3)。
struct TmpCleanup<'a> {
    path: Option<&'a Path>,
}

impl<'a> TmpCleanup<'a> {
    fn armed(path: &'a Path) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TmpCleanup<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Atomically publish a completed sibling temp file over `dst`.
///
/// The temp path is always beside `dst`, so Windows `MoveFileExW` and POSIX `rename(2)`
/// can replace the directory entry without a delete gap. A failed replace leaves the old
/// destination intact.
/// `no_clobber` が `true` のときは既存 `dst` を上書きしない (MoveFileExW から
/// `MOVEFILE_REPLACE_EXISTING` を外し、dst が在れば失敗させる = atomic な no-clobber)。
#[cfg(windows)]
fn replace_file_atomic(tmp_path: &Path, dst: &Path, no_clobber: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let flags = if no_clobber {
        MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    };
    let tmp_wide: Vec<u16> = tmp_path.as_os_str().encode_wide().chain([0]).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain([0]).collect();
    unsafe { MoveFileExW(PCWSTR(tmp_wide.as_ptr()), PCWSTR(dst_wide.as_ptr()), flags) }
        .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn replace_file_atomic(tmp_path: &Path, dst: &Path, no_clobber: bool) -> std::io::Result<()> {
    if no_clobber {
        // atomic no-clobber: hard_link は dst が既存なら EEXIST で失敗する (POSIX link(2))。
        // link 成功時点で dst は変換結果を指す (publish 済み)。tmp の除去は best-effort に
        // する — remove 失敗で Err を返すと、呼び出し側が「既存 dst にブロックされた」と
        // 誤報告する (Codex P3)。呼び出し側の TmpCleanup ガードも残った tmp を掃除する。
        // (実行時経路は Windows のみ。このパスは主に非 Windows の cargo check 用。)
        std::fs::hard_link(tmp_path, dst)?;
        let _ = std::fs::remove_file(tmp_path);
        Ok(())
    } else {
        std::fs::rename(tmp_path, dst)
    }
}

fn do_convert(
    src: &Path,
    tmp_path: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
    strict: bool,
) -> Result<ArchiveImageSummary, ConvertError> {
    let out_file = std::fs::File::create(tmp_path)?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(out_file));

    let mut ctx = ConvertCtx {
        zw: &mut zw,
        cancel,
        progress,
        files_done: 0,
        files_total: 0,
        bytes_written: 0,
        seen_names: std::collections::HashSet::new(),
        strict,
    };
    match format {
        ArchiveFormat::Rar => expand_rar(&mut ctx, src, password, "", 0)?,
        ArchiveFormat::SevenZ => expand_7z(&mut ctx, src, "", 0)?,
        ArchiveFormat::Lzh => expand_lzh(&mut ctx, src, "", 0)?,
        ArchiveFormat::Zip => {
            let file = std::fs::File::open(src)?;
            let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
                .map_err(|e| ConvertError::Archive(e.to_string()))?;
            expand_zip(&mut ctx, &mut archive, "", 0)?;
        }
    }
    let summary = ArchiveImageSummary {
        image_count: ctx.files_done,
        total_uncompressed_bytes: ctx.bytes_written,
        nested_archive_count: 0,
    };
    drop(ctx);

    // finish が返す BufWriter を明示的に flush し、ハンドルを閉じてから publish 前検証で
    // 開き直す。BufWriter::drop の best-effort flush に出力完全性を委ねない。
    let mut out = zw.finish().map_err(zip_writer_error)?;
    out.flush()?;
    drop(out);
    Ok(summary)
}

fn store_options() -> zip::write::FileOptions<'static, ()> {
    zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true)
}

// ──────────────────────────────────────────────────────────────────────
// 再帰展開エンジン (v1.3.0: 入れ子アーカイブを展開しながら 1 本の ZIP に書く)
// ──────────────────────────────────────────────────────────────────────

/// 変換出力 (cache ZIP) への書き込み状態。入れ子展開の全レベルで共有する。
struct ConvertCtx<'a> {
    zw: &'a mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    cancel: &'a AtomicBool,
    progress: Option<&'a dyn Fn(ConvertProgress)>,
    files_done: u32,
    /// 進捗分母。各アーカイブ (外側 + 入れ子) を開くたびにその階層の直下画像数を
    /// 加算する。入れ子の中身は開くまで数えられないため、分母は変換中に成長する
    /// (進捗バーは後退せず、% が下がる形で伸びる)。
    files_total: u32,
    bytes_written: u64,
    /// 出力エントリ名の一意化 (DI-5)。全階層・全入れ子で共有する。
    seen_names: std::collections::HashSet<String>,
    /// 明示 sibling / batch 変換では、欠落につながる skip を許さず変換全体を失敗させる。
    strict: bool,
}

impl ConvertCtx<'_> {
    fn check_cancel(&self) -> Result<(), ConvertError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        Ok(())
    }

    fn emit_progress(&self) {
        if let Some(cb) = self.progress {
            cb(ConvertProgress {
                files_done: self.files_done,
                files_total: self.files_total.max(self.files_done),
                bytes_written: self.bytes_written,
            });
        }
    }

    /// この階層で書く予定の画像数を進捗分母へ加算する。
    fn add_expected(&mut self, n: u32) {
        if n > 0 {
            self.files_total = self.files_total.saturating_add(n);
            self.emit_progress();
        }
    }

    /// 画像 1 枚に許す copy 上限 = min(残り総予算, 単一エントリ上限)。これを超える入力は
    /// 展開爆弾とみなして [`ConvertError::TooLarge`] にする。
    fn image_copy_limit(&self) -> u64 {
        MAX_CONVERT_OUTPUT_BYTES
            .saturating_sub(self.bytes_written)
            .min(MAX_SINGLE_ENTRY_BYTES)
    }

    /// 画像 1 枚を STORE で書く (ストリーミング copy)。`name` は prefix 込みの
    /// 正規化済みフルパス。copy 途中で失敗したら書きかけエントリを `abort_file` で
    /// 取り除いてから伝播する (上位の入れ子 skip ガードが握っても出力 ZIP が
    /// エントリ途中の壊れた状態にならないように)。
    fn write_image_reader<R: Read + ?Sized>(
        &mut self,
        name: String,
        r: &mut R,
    ) -> Result<(), ConvertError> {
        let limit = self.image_copy_limit();
        let name = dedup_entry_name(name, &mut self.seen_names);
        self.zw
            .start_file(&name, store_options())
            .map_err(zip_writer_error)?;
        // copy_capped が Io / TooLarge いずれを返しても abort_file で書きかけエントリを除去。
        let copied = match copy_capped(r, &mut *self.zw, limit) {
            Ok(n) => n,
            Err(e) => {
                let _ = self.zw.abort_file();
                return Err(e);
            }
        };
        self.finish_image(copied);
        Ok(())
    }

    /// 画像 1 枚を STORE で書く (メモリ上のバイト列。unrar はストリーミング API が無い)。
    fn write_image_bytes(&mut self, name: String, data: &[u8]) -> Result<(), ConvertError> {
        if data.len() as u64 > self.image_copy_limit() {
            return Err(ConvertError::TooLarge);
        }
        let name = dedup_entry_name(name, &mut self.seen_names);
        self.zw
            .start_file(&name, store_options())
            .map_err(zip_writer_error)?;
        if let Err(e) = self.zw.write_all(data) {
            let _ = self.zw.abort_file();
            return Err(ConvertError::Io(e));
        }
        self.finish_image(data.len() as u64);
        Ok(())
    }

    fn finish_image(&mut self, copied: u64) {
        self.bytes_written = self.bytes_written.saturating_add(copied);
        // bytes_written と同様 saturating で統一 (到達不能だが >42 億エントリで wrap/panic
        // しないように)。
        self.files_done = self.files_done.saturating_add(1);
        self.emit_progress();
    }
}

/// 出力 ZIP writer のエラーを ConvertError へ写像する。**Io は Io のまま保つ**こと:
/// `expand_nested_guarded` は Archive 系 (入れ子アーカイブ自体の解析失敗) だけを
/// 握って skip するので、出力側のディスク/ストリーム障害を Archive に丸めると
/// 「エントリの欠けた cache ZIP が成功扱いで残る」事故になる (Codex P2)。
fn zip_writer_error(e: zip::result::ZipError) -> ConvertError {
    match e {
        zip::result::ZipError::Io(io) => ConvertError::Io(io),
        other => ConvertError::Archive(other.to_string()),
    }
}

/// 入れ子アーカイブのバイト列を一時ファイルへ書き出す (unrar / sevenz / delharc は
/// ファイルパス前提のため)。戻り値の Drop で自動削除される。
fn write_reader_to_temp<R: Read + ?Sized>(
    r: &mut R,
    kind: ArchiveFormat,
) -> Result<tempfile::NamedTempFile, ConvertError> {
    let mut tmp = tempfile::Builder::new()
        .prefix("miv-nested-")
        .suffix(kind.temp_suffix())
        .tempfile()?;
    // 入れ子アーカイブ 1 個の展開後サイズも上限でガード (展開爆弾の中間 temp が
    // disk を埋めるのを防ぐ)。tmp は Drop で削除されるので途中失敗も後始末される。
    copy_capped(r, tmp.as_file_mut(), MAX_SINGLE_ENTRY_BYTES)?;
    tmp.as_file_mut().flush()?;
    Ok(tmp)
}

/// メモリ上の入れ子アーカイブを一時ファイル化する (RAR 経路用)。
fn write_bytes_to_temp(
    data: &[u8],
    kind: ArchiveFormat,
) -> Result<tempfile::NamedTempFile, ConvertError> {
    if data.len() as u64 > MAX_SINGLE_ENTRY_BYTES {
        return Err(ConvertError::TooLarge);
    }
    let mut tmp = tempfile::Builder::new()
        .prefix("miv-nested-")
        .suffix(kind.temp_suffix())
        .tempfile()?;
    tmp.as_file_mut().write_all(data)?;
    tmp.as_file_mut().flush()?;
    Ok(tmp)
}

/// 入れ子アーカイブ 1 個を再帰展開する。壊れた / パスワード付きの入れ子は
/// **ログして skip** し、変換全体は続行する (アーカイブ解析系エラーのみ握る)。
/// Cancelled と Io (出力側の書き込み失敗・ディスク満杯等) は伝播する。
fn expand_nested_guarded(
    ctx: &mut ConvertCtx<'_>,
    kind: ArchiveFormat,
    nested_path: &Path,
    prefix: &str,
    depth: u32,
) -> Result<(), ConvertError> {
    let result = match kind {
        ArchiveFormat::Zip => std::fs::File::open(nested_path)
            .map_err(ConvertError::from)
            .and_then(|file| {
                zip::ZipArchive::new(std::io::BufReader::new(file))
                    .map_err(|e| ConvertError::Archive(e.to_string()))
            })
            .and_then(|mut archive| expand_zip(ctx, &mut archive, prefix, depth)),
        ArchiveFormat::Rar => expand_rar(ctx, nested_path, None, prefix, depth),
        ArchiveFormat::SevenZ => expand_7z(ctx, nested_path, prefix, depth),
        ArchiveFormat::Lzh => expand_lzh(ctx, nested_path, prefix, depth),
    };
    match result {
        Ok(()) => Ok(()),
        // 入れ子側の password error をそのまま返すと、単一変換 UI が外側アーカイブの
        // password prompt と誤認して再試行を繰り返す。入れ子の読取失敗として確定させる。
        Err(ConvertError::PasswordUnsupported) if ctx.strict => Err(ConvertError::Archive(
            format!("入れ子アーカイブ {prefix} はパスワード付き形式に対応していません"),
        )),
        Err(ConvertError::PasswordRequired) if ctx.strict => Err(ConvertError::Archive(format!(
            "入れ子アーカイブ {prefix} を開くにはパスワードが必要です"
        ))),
        Err(ConvertError::BadPassword) if ctx.strict => Err(ConvertError::Archive(format!(
            "入れ子アーカイブ {prefix} のパスワードが正しくありません"
        ))),
        // 明示 sibling / batch 変換は、入れ子の一部欠落を成功扱いにしない。
        Err(e) if ctx.strict => Err(e),
        // TooLarge は展開爆弾なので skip せず伝播 (Cancelled / Io と同じ扱い)。
        Err(e @ (ConvertError::Cancelled | ConvertError::Io(_) | ConvertError::TooLarge)) => Err(e),
        Err(e) => {
            crate::logger::log(format!(
                "archive_converter: 入れ子アーカイブ {prefix} の展開に失敗、skip: {e}"
            ));
            Ok(())
        }
    }
}

/// ZIP/CBZ の中身を出力へ展開する。`prefix` は出力エントリ名の前置
/// ("" または "books/inner.zip/" 形式、末尾 '/')。`depth` は現在の入れ子深さ (外側=0)。
fn expand_zip<R: Read + Seek>(
    ctx: &mut ConvertCtx<'_>,
    archive: &mut zip::ZipArchive<R>,
    prefix: &str,
    depth: u32,
) -> Result<(), ConvertError> {
    // 進捗分母: この階層の直下画像数 (central directory のみで安価)。
    let expected = archive
        .file_names()
        .filter(|raw| {
            let name = raw.replace('\\', "/");
            !name.ends_with('/') && !should_ignore_entry(&name) && is_image_entry(&name)
        })
        .count() as u32;
    ctx.add_expected(expected);

    for i in 0..archive.len() {
        ctx.check_cancel()?;
        let mut entry = match archive.by_index(i) {
            Ok(entry) => entry,
            Err(e) if ctx.strict => {
                return Err(ConvertError::Archive(format!(
                    "ZIP エントリ {i} を読み取れません: {e}"
                )));
            }
            Err(_) => continue,
        };
        if !entry.is_file() {
            continue;
        }
        // CP932 名は zip_loader と同じデコーダで解釈する。生 entry.name() (CP437
        // mojibake) のまま出力 ZIP に書くと、直接閲覧 (zip_loader 経由) と変換
        // キャッシュでエントリ名が割れ、キャッシュに mojibake 名が永続化する。
        let raw_name = crate::zip_loader::zip_entry_name(&entry);
        let normalized = raw_name.replace('\\', "/");
        if should_ignore_entry(&normalized) {
            continue;
        }
        let Some(name) = normalize_entry_name(&raw_name) else {
            if ctx.strict
                && (is_image_entry(&normalized) || nested_archive_kind(&normalized).is_some())
            {
                return Err(ConvertError::Archive(format!(
                    "安全な出力名に変換できないエントリです: {raw_name}"
                )));
            }
            continue;
        };
        if is_image_entry(&name) {
            ctx.write_image_reader(format!("{prefix}{name}"), &mut entry)?;
        } else if let Some(kind) = nested_archive_kind(&name) {
            if depth + 1 > MAX_NESTED_ARCHIVE_DEPTH {
                if ctx.strict {
                    return Err(ConvertError::Archive(format!(
                        "入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} を超えました: {prefix}{name}"
                    )));
                }
                crate::logger::log(format!(
                    "archive_converter: 入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} 超過、skip: {prefix}{name}"
                ));
                continue;
            }
            let tmp = write_reader_to_temp(&mut entry, kind)?;
            drop(entry);
            expand_nested_guarded(
                ctx,
                kind,
                tmp.path(),
                &format!("{prefix}{name}/"),
                depth + 1,
            )?;
        }
    }
    Ok(())
}

/// RAR エントリの処理アクション (header の move 都合で先に分類してから消費する)。
enum RarEntryAction {
    Skip,
    Image(String),
    Nested(String, ArchiveFormat),
}

/// RAR/CBR の中身を出力へ展開する。
fn expand_rar(
    ctx: &mut ConvertCtx<'_>,
    src: &Path,
    password: Option<&str>,
    prefix: &str,
    depth: u32,
) -> Result<(), ConvertError> {
    // 進捗分母: listing は伸長しないので安価。
    {
        let mut listing = open_rar_listing(src, password)?;
        let mut expected = 0u32;
        for entry in listing.by_ref() {
            let entry = entry.map_err(rar_error)?;
            let name = entry.filename.to_string_lossy().replace('\\', "/");
            if entry.is_file() && !should_ignore_entry(&name) && is_image_entry(&name) {
                expected += 1;
            }
        }
        ctx.add_expected(expected);
    }

    let mut archive = open_rar_processing(src, password)?;
    loop {
        ctx.check_cancel()?;
        let Some(header) = archive.read_header().map_err(rar_error)? else {
            break;
        };
        let entry = header.entry();
        let raw_name = entry.filename.to_string_lossy().to_string();
        // 宣言サイズで RAM ガード。unrar はストリーミング API が無く header.read() が
        // entry 全体を Vec へ読むので、巨大宣言サイズは read 前に弾く (展開爆弾の
        // RAM 枯渇防止)。実データが宣言より大きい嘘ヘッダは read 後の write 側上限が拾う。
        let unpacked_size = entry.unpacked_size;
        let normalized = raw_name.replace('\\', "/");
        let action = if !entry.is_file() || should_ignore_entry(&normalized) {
            RarEntryAction::Skip
        } else if is_image_entry(&normalized) {
            match normalize_entry_name(&raw_name) {
                Some(name) => RarEntryAction::Image(name),
                None if ctx.strict => {
                    return Err(ConvertError::Archive(format!(
                        "安全な出力名に変換できない RAR エントリです: {raw_name}"
                    )));
                }
                None => RarEntryAction::Skip,
            }
        } else if let Some(kind) = nested_archive_kind(&normalized) {
            match normalize_entry_name(&raw_name) {
                Some(name) if depth + 1 <= MAX_NESTED_ARCHIVE_DEPTH => {
                    RarEntryAction::Nested(name, kind)
                }
                Some(name) if ctx.strict => {
                    return Err(ConvertError::Archive(format!(
                        "入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} を超えました: {prefix}{name}"
                    )));
                }
                Some(name) => {
                    crate::logger::log(format!(
                        "archive_converter: 入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} 超過、skip: {prefix}{name}"
                    ));
                    RarEntryAction::Skip
                }
                None if ctx.strict => {
                    return Err(ConvertError::Archive(format!(
                        "安全な出力名に変換できない RAR エントリです: {raw_name}"
                    )));
                }
                None => RarEntryAction::Skip,
            }
        } else {
            RarEntryAction::Skip
        };
        archive = match action {
            RarEntryAction::Skip => header.skip().map_err(rar_error)?,
            RarEntryAction::Image(name) => {
                if unpacked_size > MAX_SINGLE_ENTRY_BYTES {
                    return Err(ConvertError::TooLarge);
                }
                let (data, next) = header.read().map_err(rar_error)?;
                ctx.check_cancel()?;
                ctx.write_image_bytes(format!("{prefix}{name}"), &data)?;
                next
            }
            RarEntryAction::Nested(name, kind) => {
                if unpacked_size > MAX_SINGLE_ENTRY_BYTES {
                    return Err(ConvertError::TooLarge);
                }
                // unrar にストリーミング API が無いため一旦メモリへ読み、一時ファイル化
                // して再帰する (メモリピーク = 入れ子アーカイブ 1 個分)。
                let (data, next) = header.read().map_err(rar_error)?;
                ctx.check_cancel()?;
                let tmp = write_bytes_to_temp(&data, kind)?;
                drop(data);
                expand_nested_guarded(
                    ctx,
                    kind,
                    tmp.path(),
                    &format!("{prefix}{name}/"),
                    depth + 1,
                )?;
                next
            }
        };
    }
    Ok(())
}

/// 7z/CB7 の中身を出力へ展開する。
fn expand_7z(
    ctx: &mut ConvertCtx<'_>,
    src: &Path,
    prefix: &str,
    depth: u32,
) -> Result<(), ConvertError> {
    let mut reader =
        sevenz_rust2::ArchiveReader::open(src, Default::default()).map_err(sevenz_error)?;

    // 進捗分母: この階層の直下画像数 (アーカイブメタの走査のみで安価)。
    let expected = reader
        .archive()
        .files
        .iter()
        .filter(|e| {
            !e.is_directory
                && !should_ignore_entry(&e.name.replace('\\', "/"))
                && is_image_entry(&e.name)
        })
        .count() as u32;
    ctx.add_expected(expected);

    // for_each_entries の closure は io::Result しか返せないので、ConvertError
    // (Cancelled / 出力 Io / 入れ子再帰の伝播分) はスロットへ退避して終了後に取り出す。
    let mut deferred: Option<ConvertError> = None;
    let iter_result = reader.for_each_entries(|entry, r| {
        if ctx.cancel.load(Ordering::Relaxed) {
            deferred = Some(ConvertError::Cancelled);
            return Ok(false);
        }
        let normalized = entry.name.replace('\\', "/");
        if entry.is_directory || should_ignore_entry(&normalized) {
            // solid 7z (7-Zip 既定) は block 内の各ファイルが同一の逐次 stream を共有するため、
            // skip するエントリも読み捨てて stream を進めないと、後続エントリが前の残バイトを
            // 読んで画像が壊れる (v1.0.0 データ整合性レビュー DI-4)。directory は空 reader
            // なので 0 バイト copy で無害。
            std::io::copy(r, &mut std::io::sink())?;
            return Ok(true);
        }
        if is_image_entry(&normalized) {
            let Some(name) = normalize_entry_name(&entry.name) else {
                if ctx.strict {
                    deferred = Some(ConvertError::Archive(format!(
                        "安全な出力名に変換できない 7z エントリです: {}",
                        entry.name
                    )));
                    return Ok(false);
                }
                // 正規化不能な画像エントリも drain してから skip (同上、stream 整合のため)。
                std::io::copy(r, &mut std::io::sink())?;
                return Ok(true);
            };
            if let Err(e) = ctx.write_image_reader(format!("{prefix}{name}"), r) {
                deferred = Some(e);
                return Ok(false);
            }
            return Ok(true);
        }
        if let Some(kind) = nested_archive_kind(&normalized) {
            let Some(name) = normalize_entry_name(&entry.name) else {
                if ctx.strict {
                    deferred = Some(ConvertError::Archive(format!(
                        "安全な出力名に変換できない 7z エントリです: {}",
                        entry.name
                    )));
                    return Ok(false);
                }
                std::io::copy(r, &mut std::io::sink())?;
                return Ok(true);
            };
            if depth + 1 > MAX_NESTED_ARCHIVE_DEPTH {
                if ctx.strict {
                    deferred = Some(ConvertError::Archive(format!(
                        "入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} を超えました: {prefix}{name}"
                    )));
                    return Ok(false);
                }
                crate::logger::log(format!(
                    "archive_converter: 入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} 超過、skip: {prefix}{name}"
                ));
                std::io::copy(r, &mut std::io::sink())?;
                return Ok(true);
            }
            // 一時ファイルへ取り出し (= stream は完全 drain される) → 再帰展開。
            let tmp = match write_reader_to_temp(r, kind) {
                Ok(t) => t,
                Err(e) => {
                    deferred = Some(e);
                    return Ok(false);
                }
            };
            if let Err(e) =
                expand_nested_guarded(ctx, kind, tmp.path(), &format!("{prefix}{name}/"), depth + 1)
            {
                deferred = Some(e);
                return Ok(false);
            }
            return Ok(true);
        }
        std::io::copy(r, &mut std::io::sink())?;
        Ok(true)
    });
    // deferred (= ネスト write の Io / Cancelled) を **先に** 返す。`for_each_entries` が
    // stop/cleanup で Err を返すと、後で map_err すると Archive 扱いに化けて、せっかくの
    // Io / Cancelled が握り潰され、切り詰めた cache が「成功」として record され得る
    // (expand_nested_guarded は Io / Cancelled 以外を skip するため)。
    if let Some(e) = deferred {
        return Err(e);
    }
    iter_result.map_err(sevenz_error)?;
    Ok(())
}

/// 既に書き込んだ正規化名と衝突したら拡張子の前に " (N)" を挿入して一意化する。
/// normalize_entry_name は `\`→`/`・先頭 `/` 除去を行うため、元が異なる 2 エントリが同名に
/// なり得る。同名で複数 start_file すると read 時 by_name が 1 つしか返せず片方が不可視に
/// なるので、両方を個別に名前解決できるようにする (v1.0.0 データ整合性レビュー DI-5)。
pub(crate) fn dedup_entry_name(
    name: String,
    seen: &mut std::collections::HashSet<String>,
) -> String {
    if seen.insert(name.clone()) {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) => (&name[..i], &name[i..]), // ext は先頭の '.' を含む
        None => (name.as_str(), ""),
    };
    let mut n = 2;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// LZH/LHA の中身を出力へ展開する。
fn expand_lzh(
    ctx: &mut ConvertCtx<'_>,
    src: &Path,
    prefix: &str,
    depth: u32,
) -> Result<(), ConvertError> {
    // 進捗分母: ヘッダスキャンのみで安価。
    {
        let mut r = delharc::parse_file(src).map_err(lzh_error)?;
        let mut expected = 0u32;
        loop {
            let header = r.header();
            let name = header.parse_pathname().to_string_lossy().replace('\\', "/");
            if !header.is_directory() && !should_ignore_entry(&name) && is_image_entry(&name) {
                expected += 1;
            }
            if !r.next_file().map_err(lzh_error)? {
                break;
            }
        }
        ctx.add_expected(expected);
    }

    let mut reader = delharc::parse_file(src).map_err(lzh_error)?;
    loop {
        ctx.check_cancel()?;
        // header の借用は raw_name 抽出までで終える (この後 reader を Read で使う)。
        let (raw_name, is_dir) = {
            let header = reader.header();
            (
                header.parse_pathname().to_string_lossy().to_string(),
                header.is_directory(),
            )
        };
        let normalized = raw_name.replace('\\', "/");
        if !is_dir && !should_ignore_entry(&normalized) {
            let is_image = is_image_entry(&normalized);
            let nested_kind = nested_archive_kind(&normalized);
            if !reader.is_decoder_supported() {
                if ctx.strict && (is_image || nested_kind.is_some()) {
                    return Err(ConvertError::Archive(format!(
                        "LZH エントリの圧縮方式に対応していません: {raw_name}"
                    )));
                }
            } else if is_image {
                let Some(name) = normalize_entry_name(&raw_name) else {
                    if ctx.strict {
                        return Err(ConvertError::Archive(format!(
                            "安全な出力名に変換できない LZH エントリです: {raw_name}"
                        )));
                    }
                    if !reader.next_file().map_err(lzh_error)? {
                        break;
                    }
                    continue;
                };
                ctx.write_image_reader(format!("{prefix}{name}"), &mut reader)?;
                if let Err(e) = reader.crc_check() {
                    if ctx.strict {
                        return Err(ConvertError::Archive(format!(
                            "LZH エントリの CRC 検証に失敗しました: {raw_name}: {e}"
                        )));
                    }
                    crate::logger::log(format!(
                        "archive_converter: LZH CRC mismatch for {raw_name}: {e}"
                    ));
                }
            } else if let Some(kind) = nested_kind {
                let Some(name) = normalize_entry_name(&raw_name) else {
                    if ctx.strict {
                        return Err(ConvertError::Archive(format!(
                            "安全な出力名に変換できない LZH エントリです: {raw_name}"
                        )));
                    }
                    if !reader.next_file().map_err(lzh_error)? {
                        break;
                    }
                    continue;
                };
                if depth + 1 > MAX_NESTED_ARCHIVE_DEPTH {
                    if ctx.strict {
                        return Err(ConvertError::Archive(format!(
                            "入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} を超えました: {prefix}{name}"
                        )));
                    }
                    crate::logger::log(format!(
                        "archive_converter: 入れ子深さ上限 {MAX_NESTED_ARCHIVE_DEPTH} 超過、skip: {prefix}{name}"
                    ));
                } else {
                    let tmp = write_reader_to_temp(&mut reader, kind)?;
                    if let Err(e) = reader.crc_check() {
                        if ctx.strict {
                            return Err(ConvertError::Archive(format!(
                                "LZH エントリの CRC 検証に失敗しました: {raw_name}: {e}"
                            )));
                        }
                        crate::logger::log(format!(
                            "archive_converter: LZH CRC mismatch for {raw_name}: {e}"
                        ));
                    }
                    expand_nested_guarded(
                        ctx,
                        kind,
                        tmp.path(),
                        &format!("{prefix}{name}/"),
                        depth + 1,
                    )?;
                }
            }
        }
        if !reader.next_file().map_err(lzh_error)? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_summary_scan_short_circuits_every_supported_format() {
        let cancel = AtomicBool::new(true);
        for format in [
            ArchiveFormat::Rar,
            ArchiveFormat::SevenZ,
            ArchiveFormat::Lzh,
            ArchiveFormat::Zip,
        ] {
            assert!(matches!(
                scan_summary_with_password_cancelable(
                    Path::new("cancel-before-archive-open"),
                    format,
                    None,
                    &cancel,
                ),
                Err(ConvertError::Cancelled)
            ));
        }
    }

    #[test]
    fn copy_capped_passes_input_within_limit() {
        let src = vec![0u8; 100];
        let mut out = Vec::new();
        let copied = copy_capped(&mut src.as_slice(), &mut out, 100).unwrap();
        assert_eq!(copied, 100);
        assert_eq!(out.len(), 100);
    }

    #[test]
    fn copy_capped_rejects_input_over_limit() {
        let src = vec![0u8; 101];
        let mut out = Vec::new();
        let err = copy_capped(&mut src.as_slice(), &mut out, 100).unwrap_err();
        assert!(matches!(err, ConvertError::TooLarge));
        // limit+1 までしか読まないので出力は最大 limit+1 バイトで止まる (爆弾でも有界)。
        assert!(out.len() as u64 <= 101);
    }

    #[test]
    fn copy_capped_zero_limit_rejects_nonempty() {
        let src = vec![0u8; 1];
        let mut out = Vec::new();
        assert!(matches!(
            copy_capped(&mut src.as_slice(), &mut out, 0),
            Err(ConvertError::TooLarge)
        ));
        // 空入力は 0 バイトで OK。
        let empty: Vec<u8> = Vec::new();
        let mut out2 = Vec::new();
        assert_eq!(copy_capped(&mut empty.as_slice(), &mut out2, 0).unwrap(), 0);
    }

    // ── 入れ子アーカイブ再帰展開 (v1.3.0) のテストヘルパー ──────────────

    /// メモリ上に STORE ZIP を作る。entries = (エントリ名, 中身バイト)。
    fn build_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                zw.start_file(*name, opts).unwrap();
                zw.write_all(data).unwrap();
            }
            zw.finish().unwrap();
        }
        buf.into_inner()
    }

    /// 7z を一時生成して中身バイトを返す (sevenz-rust2 の writer、default features
    /// に compress が含まれるのでテストから書ける)。
    fn build_7z_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.7z");
        let mut w = sevenz_rust2::ArchiveWriter::create(&path).unwrap();
        for (name, data) in entries {
            w.push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(std::io::Cursor::new(data.to_vec())),
            )
            .unwrap();
        }
        w.finish().unwrap();
        std::fs::read(&path).unwrap()
    }

    /// `src_bytes` を `format` として変換し、出力 ZIP の (エントリ名, 中身) を返す。
    fn run_convert_bytes(
        src_bytes: &[u8],
        src_file_name: &str,
        format: ArchiveFormat,
    ) -> Vec<(String, Vec<u8>)> {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(src_file_name);
        std::fs::write(&src, src_bytes).unwrap();
        let dst = dir.path().join("out.zip");
        let cancel = AtomicBool::new(false);
        convert_to_zip(&src, &dst, format, &cancel, None).unwrap();
        let f = std::fs::File::open(&dst).unwrap();
        let mut ar = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
        let mut out = Vec::new();
        for i in 0..ar.len() {
            let mut e = ar.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut b = Vec::new();
            std::io::Read::read_to_end(&mut e, &mut b).unwrap();
            out.push((name, b));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn names(out: &[(String, Vec<u8>)]) -> Vec<&str> {
        out.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[test]
    fn zip_convert_expands_nested_zip_with_flat_names() {
        // ZIP > {cover.jpg, books/inner.zip > {p1.jpg, sub/p2.png}, skip.txt}
        let inner = build_zip_bytes(&[("p1.jpg", b"P1"), ("sub/p2.png", b"P2")]);
        let outer = build_zip_bytes(&[
            ("cover.jpg", b"COVER"),
            ("books/inner.zip", &inner),
            ("skip.txt", b"not image"),
        ]);
        let out = run_convert_bytes(&outer, "src.zip", ArchiveFormat::Zip);
        assert_eq!(
            names(&out),
            vec![
                "books/inner.zip/p1.jpg",
                "books/inner.zip/sub/p2.png",
                "cover.jpg"
            ]
        );
        // 中身バイトのラウンドトリップ (展開で壊れていない)。
        let p1 = out
            .iter()
            .find(|(n, _)| n == "books/inner.zip/p1.jpg")
            .unwrap();
        assert_eq!(p1.1, b"P1");
    }

    #[test]
    fn zip_convert_expands_double_nested_zip() {
        // ZIP > inner.zip > inner2.cbz > p.jpg → "inner.zip/inner2.cbz/p.jpg"
        let inner2 = build_zip_bytes(&[("p.jpg", b"DEEP")]);
        let inner = build_zip_bytes(&[("inner2.cbz", &inner2)]);
        let outer = build_zip_bytes(&[("inner.zip", &inner)]);
        let out = run_convert_bytes(&outer, "src.zip", ArchiveFormat::Zip);
        assert_eq!(names(&out), vec!["inner.zip/inner2.cbz/p.jpg"]);
        assert_eq!(out[0].1, b"DEEP");
    }

    #[test]
    fn zip_convert_depth_cap_skips_too_deep() {
        // MAX_NESTED_ARCHIVE_DEPTH を超える多重ネストは skip される (浅い画像は残る)。
        let mut current = build_zip_bytes(&[("bottom.jpg", b"BOTTOM")]);
        for i in 0..(MAX_NESTED_ARCHIVE_DEPTH + 2) {
            let name = format!("n{i}.zip");
            current = build_zip_bytes(&[(name.as_str(), &current), ("shallow.jpg", b"S")]);
        }
        let out = run_convert_bytes(&current, "src.zip", ArchiveFormat::Zip);
        let names = names(&out);
        // 最も浅い shallow.jpg は必ず残る。
        assert!(names.contains(&"shallow.jpg"), "{names:?}");
        // 一番深い bottom.jpg は深さ上限で落ちる。
        assert!(
            !names.iter().any(|n| n.ends_with("bottom.jpg")),
            "{names:?}"
        );
    }

    #[test]
    fn zip_convert_expands_nested_7z() {
        // ZIP > books/in.7z > {a.png, art/b.jpg} → フラットパスで展開 (非 ZIP 入れ子)。
        let seven = build_7z_bytes(&[("a.png", b"A7"), ("art/b.jpg", b"B7")]);
        let outer = build_zip_bytes(&[("books/in.7z", &seven), ("c.jpg", b"C")]);
        let out = run_convert_bytes(&outer, "src.zip", ArchiveFormat::Zip);
        assert_eq!(
            names(&out),
            vec!["books/in.7z/a.png", "books/in.7z/art/b.jpg", "c.jpg"]
        );
        let a = out.iter().find(|(n, _)| n == "books/in.7z/a.png").unwrap();
        assert_eq!(a.1, b"A7");
    }

    #[test]
    fn sevenz_convert_expands_nested_zip() {
        // 7z > {d.jpg, x.zip > img.png} → RAR/7z を外側にした入れ子も展開される。
        let nested_zip = build_zip_bytes(&[("img.png", b"NZ")]);
        let seven = build_7z_bytes(&[("d.jpg", b"D"), ("x.zip", &nested_zip)]);
        let out = run_convert_bytes(&seven, "src.7z", ArchiveFormat::SevenZ);
        assert_eq!(names(&out), vec!["d.jpg", "x.zip/img.png"]);
    }

    #[test]
    fn zip_convert_skips_corrupt_nested_archive() {
        // 壊れた入れ子 (rar 拡張子だが中身ゴミ) はログ + skip、変換全体は成功する。
        let outer =
            build_zip_bytes(&[("ok.jpg", b"OK"), ("bad.rar", b"this is not a rar archive")]);
        let out = run_convert_bytes(&outer, "src.zip", ArchiveFormat::Zip);
        assert_eq!(names(&out), vec!["ok.jpg"]);
    }

    #[test]
    fn strict_convert_rejects_corrupt_nested_archive_and_cleans_tmp() {
        let outer =
            build_zip_bytes(&[("ok.jpg", b"OK"), ("bad.rar", b"this is not a rar archive")]);
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.zip");
        let dst = dir.path().join("out.zip");
        let tmp = dst.with_extension("zip.part");
        std::fs::write(&src, outer).unwrap();

        let result = convert_to_zip_with_password(
            &src,
            &dst,
            ArchiveFormat::Zip,
            None,
            &AtomicBool::new(false),
            None,
            ConvertOptions {
                no_clobber: true,
                verify: true,
            },
        );

        assert!(matches!(result, Err(ConvertError::Archive(_))));
        assert!(!dst.exists(), "失敗した変換結果を publish しないこと");
        assert!(!tmp.exists(), "TmpCleanup が中間 ZIP を削除すること");
    }

    #[test]
    fn zip_convert_ignores_macosx_entries() {
        // __MACOSX/ 配下と dot 始まりは通常 ZIP 閲覧 (zip_loader::should_ignore) と
        // 同様に除外する。
        let outer = build_zip_bytes(&[
            ("a.jpg", b"A"),
            ("__MACOSX/a.jpg", b"GARBAGE"),
            (".hidden.jpg", b"H"),
        ]);
        let out = run_convert_bytes(&outer, "src.zip", ArchiveFormat::Zip);
        assert_eq!(names(&out), vec!["a.jpg"]);
    }

    #[test]
    fn scan_summary_zip_counts_images_and_nested() {
        let inner = build_zip_bytes(&[("p1.jpg", b"P1")]);
        let outer = build_zip_bytes(&[
            ("cover.jpg", b"COVER"),
            ("inner.zip", &inner),
            ("foo.rar", b"junk"),
            ("note.txt", b"x"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.zip");
        std::fs::write(&src, &outer).unwrap();
        let s = scan_summary(&src, ArchiveFormat::Zip).unwrap();
        // 直下画像 1 (cover)、入れ子アーカイブ 2 (inner.zip + foo.rar)。
        assert_eq!(s.image_count, 1);
        assert_eq!(s.nested_archive_count, 2);
    }

    /// dist/ziptest のサンプル (scripts/make_nested_archive_test.py、要 WinRAR) が
    /// 存在するときだけ実 RAR で再帰展開を検証する。RAR の作成は WinRAR 専用機能で
    /// テストから生成できないため、サンプル無し環境では no-op で pass する。
    fn ziptest_sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("dist/ziptest")
            .join(name);
        p.exists().then_some(p)
    }

    fn convert_sample(src: &Path, format: ArchiveFormat) -> (ArchiveImageSummary, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("out.zip");
        let cancel = AtomicBool::new(false);
        let summary = convert_to_zip(src, &dst, format, &cancel, None).unwrap();
        let f = std::fs::File::open(&dst).unwrap();
        let mut ar = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
        let names = (0..ar.len())
            .map(|i| ar.by_index(i).unwrap().name().to_string())
            .collect();
        (summary, names)
    }

    #[test]
    fn real_rar_in_zip_expands_if_sample_present() {
        // ZIP > inner.rar (3 頁) + front.png
        let Some(src) = ziptest_sample("rar_in_zip.zip") else {
            return;
        };
        let (summary, names) = convert_sample(&src, ArchiveFormat::Zip);
        assert_eq!(summary.image_count, 4, "{names:?}");
        assert!(names.iter().any(|n| n == "front.png"), "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("inner.rar/")),
            "{names:?}"
        );
    }

    #[test]
    fn real_zip_in_rar_expands_if_sample_present() {
        // RAR > 直下 2 頁 + inner.zip (3 頁)
        let Some(src) = ziptest_sample("zip_in_rar.rar") else {
            return;
        };
        let (summary, names) = convert_sample(&src, ArchiveFormat::Rar);
        assert_eq!(summary.image_count, 5, "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("inner.zip/")),
            "{names:?}"
        );
    }

    #[test]
    fn real_rar_in_rar_expands_if_sample_present() {
        // RAR > 直下 2 頁 + inner.rar (3 頁)
        let Some(src) = ziptest_sample("nested_rar_test.rar") else {
            return;
        };
        let (summary, names) = convert_sample(&src, ArchiveFormat::Rar);
        assert_eq!(summary.image_count, 5, "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("inner.rar/")),
            "{names:?}"
        );
    }

    #[test]
    fn nested_archive_kind_detects_archive_exts() {
        assert_eq!(nested_archive_kind("a/b.zip"), Some(ArchiveFormat::Zip));
        assert_eq!(nested_archive_kind("b.CBZ"), Some(ArchiveFormat::Zip));
        assert_eq!(nested_archive_kind("c.rar"), Some(ArchiveFormat::Rar));
        assert_eq!(nested_archive_kind("d.cb7"), Some(ArchiveFormat::SevenZ));
        assert_eq!(nested_archive_kind("e.lha"), Some(ArchiveFormat::Lzh));
        assert_eq!(nested_archive_kind("f.jpg"), None);
        assert_eq!(nested_archive_kind("dir.zip/f"), None); // 拡張子はファイル名部分のみ
    }

    #[test]
    fn dedup_entry_name_disambiguates_collisions() {
        // DI-5: 正規化で同名衝突したエントリを一意化し、全画像が by_name 解決可能になること。
        let mut seen = std::collections::HashSet::new();
        assert_eq!(
            dedup_entry_name("a/b.jpg".to_string(), &mut seen),
            "a/b.jpg"
        );
        assert_eq!(
            dedup_entry_name("a/b.jpg".to_string(), &mut seen),
            "a/b (2).jpg"
        );
        assert_eq!(
            dedup_entry_name("a/b.jpg".to_string(), &mut seen),
            "a/b (3).jpg"
        );
        // 拡張子なしでも壊れない
        assert_eq!(dedup_entry_name("x".to_string(), &mut seen), "x");
        assert_eq!(dedup_entry_name("x".to_string(), &mut seen), "x (2)");
    }

    #[test]
    fn atomic_replace_overwrites_destination_without_a_delete_gap() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("book.zip");
        let tmp = dir.path().join("book.zip.part");
        std::fs::write(&dst, b"old zip").unwrap();
        std::fs::write(&tmp, b"new zip").unwrap();

        replace_file_atomic(&tmp, &dst, false).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), b"new zip");
        assert!(!tmp.exists());
    }

    #[test]
    fn no_clobber_replace_refuses_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("book.zip");
        let tmp = dir.path().join("book.zip.part");
        std::fs::write(&dst, b"user owned zip").unwrap();
        std::fs::write(&tmp, b"new zip").unwrap();

        // no_clobber = true なので既存 dst を上書きしない。
        assert!(replace_file_atomic(&tmp, &dst, true).is_err());
        assert_eq!(std::fs::read(&dst).unwrap(), b"user owned zip");
    }

    #[test]
    fn verify_output_zip_accepts_complete_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("complete.zip.part");
        let bytes = build_zip_bytes(&[("a.jpg", b"A"), ("dir/b.png", b"BBBB")]);
        std::fs::write(&path, bytes).unwrap();

        verify_output_zip(&path, 2, &AtomicBool::new(false)).unwrap();
    }

    #[test]
    fn verify_output_zip_aborts_on_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.zip.part");
        let bytes = build_zip_bytes(&[("a.jpg", b"A"), ("dir/b.png", b"BBBB")]);
        std::fs::write(&path, bytes).unwrap();

        // 事前にキャンセルを立てておくと、公開前検証は Cancelled で抜ける。
        let error = verify_output_zip(&path, 2, &AtomicBool::new(true)).unwrap_err();
        assert!(matches!(error, ConvertError::Cancelled));
    }

    #[test]
    fn verify_output_zip_rejects_image_count_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("count-mismatch.zip.part");
        let bytes = build_zip_bytes(&[("a.jpg", b"A")]);
        std::fs::write(&path, bytes).unwrap();

        let error = verify_output_zip(&path, 2, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(
            error,
            ConvertError::Archive(message)
                if message.starts_with("変換結果の検証に失敗しました:")
        ));
    }

    #[test]
    fn verify_output_zip_rejects_truncated_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.zip.part");
        let mut bytes = build_zip_bytes(&[("a.jpg", b"complete image bytes")]);
        bytes.truncate(bytes.len().saturating_sub(12));
        std::fs::write(&path, bytes).unwrap();

        let error = verify_output_zip(&path, 1, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(
            error,
            ConvertError::Archive(message)
                if message.starts_with("変換結果の検証に失敗しました:")
        ));
    }

    #[test]
    fn failed_atomic_replace_preserves_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("book.zip");
        let missing_tmp = dir.path().join("missing.zip.part");
        std::fs::write(&dst, b"user owned zip").unwrap();

        assert!(replace_file_atomic(&missing_tmp, &dst, false).is_err());
        assert_eq!(std::fs::read(&dst).unwrap(), b"user owned zip");
    }

    #[test]
    fn format_from_extension() {
        assert_eq!(
            ArchiveFormat::from_extension("7z"),
            Some(ArchiveFormat::SevenZ)
        );
        assert_eq!(
            ArchiveFormat::from_extension("7Z"),
            Some(ArchiveFormat::SevenZ)
        );
        assert_eq!(
            ArchiveFormat::from_extension("lzh"),
            Some(ArchiveFormat::Lzh)
        );
        assert_eq!(
            ArchiveFormat::from_extension("lha"),
            Some(ArchiveFormat::Lzh)
        );
        assert_eq!(
            ArchiveFormat::from_extension("LHA"),
            Some(ArchiveFormat::Lzh)
        );
        assert_eq!(
            ArchiveFormat::from_extension("rar"),
            Some(ArchiveFormat::Rar)
        );
        assert_eq!(
            ArchiveFormat::from_extension("CBR"),
            Some(ArchiveFormat::Rar)
        );
        // CB7 は 7z と同じ変換経路。
        assert_eq!(
            ArchiveFormat::from_extension("cb7"),
            Some(ArchiveFormat::SevenZ)
        );
        assert_eq!(
            ArchiveFormat::from_extension("CB7"),
            Some(ArchiveFormat::SevenZ)
        );
        // CBZ / ZIP はネイティブ ZIP 扱いなので変換フォーマットではない (folder_tree 側で判定)。
        assert_eq!(ArchiveFormat::from_extension("zip"), None);
        assert_eq!(ArchiveFormat::from_extension("cbz"), None);
    }

    #[test]
    fn rar_non_first_part_name_candidate_detection() {
        assert!(!looks_like_non_first_rar_part(Path::new(r"C:\books\a.rar")));
        assert!(!looks_like_non_first_rar_part(Path::new(r"C:\books\a.cbr")));
        assert!(!looks_like_non_first_rar_part(Path::new(
            r"C:\books\a.part01.rar"
        )));
        assert!(looks_like_non_first_rar_part(Path::new(
            r"C:\books\a.part02.rar"
        )));
        assert!(!looks_like_non_first_rar_part(Path::new(r"C:\books\a.r00")));
        assert!(!looks_like_non_first_rar_part(Path::new(r"C:\books\a.r01")));
    }

    #[test]
    fn rar_converter_resolves_only_header_confirmed_subsequent_volume() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression");
        let ambiguous_single = root.join("○×△□ Vol.2.rar");
        assert_eq!(
            resolve_rar_source_path(&ambiguous_single, None).unwrap(),
            ambiguous_single
        );

        let split_root = root.join("real-split-control");
        let first = split_root.join("real-split-control.part1.rar");
        let second = split_root.join("real-split-control.part2.rar");
        assert_eq!(resolve_rar_source_path(&second, None).unwrap(), first);

        let summary = scan_summary(&second, ArchiveFormat::Rar).unwrap();
        assert!(summary.image_count > 0);
        let (converted, names) = convert_sample(&second, ArchiveFormat::Rar);
        assert_eq!(converted.image_count, summary.image_count);
        assert!(!names.is_empty());
    }

    #[test]
    fn rar_scan_opens_real_archive() {
        use base64::Engine;

        // Tiny RAR fixture with one non-image VERSION file. This exercises the UnRAR link/open/list
        // path without relying on an external rar.exe in the test environment.
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                "UmFyIRoHAM+QcwAADQAAAAAAAAAPDHQggCcAFQAAAAsAAAADRfN9xqSKB0cdMwcApIEAAFZFUlNJT04MAI/sikXMI8hICINi/l/dXFOI8HLEPXsAQAcA",
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version.rar");
        std::fs::write(&path, bytes).unwrap();

        let summary = scan_summary(&path, ArchiveFormat::Rar).unwrap();
        assert_eq!(summary.image_count, 0);
        assert_eq!(summary.total_uncompressed_bytes, 0);
    }

    #[test]
    fn non_rar_encryption_errors_are_never_password_retryable() {
        assert!(matches!(
            sevenz_error(sevenz_rust2::Error::PasswordRequired),
            ConvertError::PasswordUnsupported
        ));
        assert!(matches!(
            sevenz_error(sevenz_rust2::Error::MaybeBadPassword(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad password"
            ))),
            ConvertError::PasswordUnsupported
        ));
        assert!(matches!(
            lzh_error("password required"),
            ConvertError::Archive(_)
        ));
    }

    #[test]
    fn rar_scan_reports_password_required_for_encrypted_headers() {
        use base64::Engine;

        // RAR fixture from the unrar crate with encrypted headers. Listing without a password
        // fails before file names can be inspected, which is the path the UI uses to show the
        // password dialog before conversion.
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                "UmFyIRoHAQCb9TwzIQQAAAEPYGk2Ogo76RuVVujwyW9w3llU+IqF7eqFu5Ud8T9UQbRH0zt9uVUIq2EF/ThXvjKvKRfFlWD68jfLv5pwASApgwfOR0umx/aDmUllP0GHXFAFKr4s5tAmqjpfd60BOlJkcidJknKA8KiGTaNRm9lWAX7Cp11fp1dL98JHERp9rfM9fdVNC7ytSELuv0teRu/FAfMmq88Vd/XW7wMxQzanvMOjbWTvxRV+6cSjO6mJp+1Xfn1RUpfz5ud4WbbyBYFbFpMFScJuBHRi3jnun4FgYDt4MNKdGmrMnsigq6nxhgcd0VGiurfJA18hQcq+Rcc+jfgaAJA9cmaV8SZm/dxd8HlyjB27WXMJ+cVjXJonDDfH8LH414+wNUr2AlDwm+YbqZOJWXUh4JX7yoyLWatDOu+Ng7+qXBM0emk1YsEdFRuoAMOKc0mzztW6Iw+H9UDaazy7WGYYChWGbk1X",
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted_headers.rar");
        std::fs::write(&path, bytes).unwrap();

        let result = scan_summary_with_password(&path, ArchiveFormat::Rar, None);
        assert!(matches!(result, Err(ConvertError::PasswordRequired)));

        let summary = scan_summary_with_password(&path, ArchiveFormat::Rar, Some("password"))
            .expect("correct RAR password should list encrypted headers");
        assert_eq!(summary.image_count, 0);
    }

    #[test]
    fn is_image_entry_common_cases() {
        assert!(is_image_entry("foo.jpg"));
        assert!(is_image_entry("dir/sub/pic.PNG"));
        assert!(is_image_entry("work\\page01.webp"));
        assert!(!is_image_entry("readme.txt"));
        assert!(!is_image_entry("movie.mp4"));
        assert!(!is_image_entry(".hidden"));
        assert!(!is_image_entry("no_extension"));
    }

    /// 判定が `folder_tree::is_recognized_image_ext` に委譲されていることを確認する。
    /// これにより起動時にロードされた Susie プラグイン対応拡張子 (PI / MAG / Q0 等) も
    /// RAR/7z/LZH 変換の対象になる。ユニットテスト環境では Susie プール未初期化なので
    /// 具体的なレトロ拡張子のテストは実機シナリオで確認する。
    #[test]
    fn is_image_entry_matches_recognized_ext_predicate() {
        use crate::folder_tree::is_recognized_image_ext;
        // 代表例で委譲関係を確認 (入力はパス末尾の拡張子小文字のみ)
        for ext in ["jpg", "png", "webp", "gif", "bmp", "txt", "mp4", "rar"] {
            let entry = format!("file.{}", ext);
            assert_eq!(
                is_image_entry(&entry),
                is_recognized_image_ext(ext),
                "is_image_entry/is_recognized_image_ext mismatch for .{ext}",
            );
        }
    }

    #[test]
    fn normalize_entry_name_zip_slip() {
        assert_eq!(
            normalize_entry_name("foo/bar.jpg"),
            Some("foo/bar.jpg".to_string())
        );
        assert_eq!(
            normalize_entry_name("foo\\bar.jpg"),
            Some("foo/bar.jpg".to_string())
        );
        assert_eq!(
            normalize_entry_name("/abs/path.jpg"),
            Some("abs/path.jpg".to_string())
        );
        assert_eq!(normalize_entry_name("../escape.jpg"), None);
        assert_eq!(normalize_entry_name("a/../b.jpg"), None);
        assert_eq!(normalize_entry_name(""), None);
        assert_eq!(normalize_entry_name("dir/"), None);
    }
}
