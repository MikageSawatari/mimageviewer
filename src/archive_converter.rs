//! RAR / 7z / LZH アーカイブを ZIP (STORE) に変換するコンバータ。
//!
//! mImageViewer は ZIP での閲覧に最適化されているため、RAR / 7z / LZH をクリックしたら
//! 中身の画像だけを抜き出して無圧縮 ZIP に変換しておき、以降は通常の ZIP として開く。
//!
//! - 対応: RAR (unrar), 7z (sevenz-rust2), LZH (delharc)。
//! - 出力: 常に STORE モード (中身は既に JPEG/PNG で圧縮済み、再圧縮無意味)。
//! - 対象: 画像エントリ (`folder_tree::is_recognized_image_ext`、Susie プラグイン対応拡張子も含む) のみ。非画像は破棄。
//! - キャンセル: `Arc<AtomicBool>` を各エントリ境界でチェック。
//! - 進捗: `Fn(ConvertProgress)` コールバック。
//!
//! キャッシュ管理は [`archive_cache`] 側の責務で、本モジュールは純粋な変換ロジックのみ。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::folder_tree::{is_recognized_image_ext, path_eq};

/// 変換対応アーカイブ形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Rar,
    SevenZ,
    Lzh,
}

impl ArchiveFormat {
    /// 拡張子から形式を判定する。大文字小文字無視。対応外なら None。
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "rar" | "cbr" => Some(Self::Rar),
            "7z" | "cb7" => Some(Self::SevenZ),
            "lzh" | "lha" => Some(Self::Lzh),
            _ => None,
        }
    }

    /// 形式のラベル (バッジ / ダイアログ表示用)。
    pub fn label(self) -> &'static str {
        match self {
            Self::Rar => "RAR",
            Self::SevenZ => "7z",
            Self::Lzh => "LZH",
        }
    }
}

/// `path` が分割 RAR の先頭パート以外を指しているかを、ファイル名規則だけで判定する。
///
/// 後続パート (`.part02.rar` 等) は単独で開く対象ではないため、フォルダ一覧では
/// 先頭パートだけを `ConvertibleArchive` として表示する。`.cbr` は単体 RAR として扱う。
pub fn is_non_first_rar_part(path: &Path) -> bool {
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
    /// 画像エントリの数 (非画像・ディレクトリを除く)
    pub image_count: u32,
    /// 画像エントリの非圧縮バイト総和 (変換後 ZIP サイズの目安)
    pub total_uncompressed_bytes: u64,
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

/// 変換失敗理由。
#[derive(Debug)]
pub enum ConvertError {
    Io(std::io::Error),
    /// ユーザーキャンセルによる中断
    Cancelled,
    /// パスワードが必要なアーカイブだった
    PasswordRequired,
    /// 入力済み / 保存済みパスワードが正しくなかった
    BadPassword,
    /// アーカイブ解析・展開時のエラー (ライブラリ依存のメッセージを文字列化)
    Archive(String),
    /// 画像エントリが 0 件だった
    NoImages,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O エラー: {e}"),
            Self::Cancelled => write!(f, "キャンセルされました"),
            Self::PasswordRequired => write!(f, "パスワードが必要です"),
            Self::BadPassword => write!(f, "パスワードが正しくありません"),
            Self::Archive(s) => write!(f, "アーカイブエラー: {s}"),
            Self::NoImages => write!(f, "画像ファイルが含まれていません"),
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
fn is_image_entry(name: &str) -> bool {
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
    match format {
        ArchiveFormat::Rar => scan_summary_rar(path, password),
        ArchiveFormat::SevenZ => scan_summary_7z(path),
        ArchiveFormat::Lzh => scan_summary_lzh(path),
    }
}

fn rar_error(e: unrar::error::UnrarError) -> ConvertError {
    match e.code {
        unrar::error::Code::MissingPassword => ConvertError::PasswordRequired,
        unrar::error::Code::BadPassword => ConvertError::BadPassword,
        _ => ConvertError::Archive(format!("RAR: {e}")),
    }
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

fn scan_summary_rar(
    path: &Path,
    password: Option<&str>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let mut archive = rar_archive(path, password)?
        .as_first_part()
        .open_for_listing()
        .map_err(rar_error)?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    for entry in archive.by_ref() {
        let entry = entry.map_err(rar_error)?;
        let name = entry.filename.to_string_lossy();
        if entry.is_file() && is_image_entry(&name) {
            count += 1;
            bytes = bytes.saturating_add(entry.unpacked_size);
        }
    }
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
    })
}

fn scan_summary_7z(path: &Path) -> Result<ArchiveImageSummary, ConvertError> {
    let reader = sevenz_rust2::ArchiveReader::open(path, Default::default())
        .map_err(|e| ConvertError::Archive(e.to_string()))?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    for entry in &reader.archive().files {
        if entry.is_directory {
            continue;
        }
        if !is_image_entry(&entry.name) {
            continue;
        }
        count += 1;
        bytes = bytes.saturating_add(entry.size);
    }
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
    })
}

fn scan_summary_lzh(path: &Path) -> Result<ArchiveImageSummary, ConvertError> {
    let mut reader = delharc::parse_file(path).map_err(|e| ConvertError::Archive(e.to_string()))?;
    let mut count = 0u32;
    let mut bytes = 0u64;
    loop {
        let header = reader.header();
        let pathname = header.parse_pathname();
        let name = pathname.to_string_lossy();
        if !header.is_directory() && is_image_entry(&name) {
            count += 1;
            bytes = bytes.saturating_add(header.original_size);
        }
        if !reader
            .next_file()
            .map_err(|e| ConvertError::Archive(e.to_string()))?
        {
            break;
        }
    }
    Ok(ArchiveImageSummary {
        image_count: count,
        total_uncompressed_bytes: bytes,
    })
}

// ──────────────────────────────────────────────────────────────────────
// 変換本体
// ──────────────────────────────────────────────────────────────────────

/// 変換を実行し、`dst` に STORE モードの ZIP を生成する。
///
/// - 既に `dst` が存在する場合は上書き。
/// - 失敗 / キャンセル時は `dst` を削除してクリーンにする (途中生成物を残さない)。
/// - `cancel` は各エントリ境界でチェックする。キャンセル検出時は `ConvertError::Cancelled`。
/// - `progress` が `Some` の場合、各ファイル処理完了後に呼ぶ。
pub fn convert_to_zip(
    src: &Path,
    dst: &Path,
    format: ArchiveFormat,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    convert_to_zip_with_password(src, dst, format, None, cancel, progress)
}

pub fn convert_to_zip_with_password(
    src: &Path,
    dst: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 中間ファイルに書いて atomic rename。途中失敗時に壊れた zip が残らないようにする。
    let tmp_path = dst.with_extension("zip.part");
    let _ = std::fs::remove_file(&tmp_path);

    let summary = match do_convert(src, &tmp_path, format, password, cancel, progress) {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if summary.image_count == 0 {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ConvertError::NoImages);
    }

    // 既存の dst があれば置き換え
    if dst.exists() {
        let _ = std::fs::remove_file(dst);
    }
    std::fs::rename(&tmp_path, dst)?;
    Ok(summary)
}

fn do_convert(
    src: &Path,
    tmp_path: &Path,
    format: ArchiveFormat,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let out_file = std::fs::File::create(tmp_path)?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(out_file));

    let summary = match format {
        ArchiveFormat::Rar => convert_rar(src, password, &mut zw, cancel, progress)?,
        ArchiveFormat::SevenZ => convert_7z(src, &mut zw, cancel, progress)?,
        ArchiveFormat::Lzh => convert_lzh(src, &mut zw, cancel, progress)?,
    };

    zw.finish()
        .map_err(|e| ConvertError::Archive(e.to_string()))?;
    Ok(summary)
}

fn store_options() -> zip::write::FileOptions<'static, ()> {
    zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true)
}

fn convert_rar(
    src: &Path,
    password: Option<&str>,
    zw: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let files_total = scan_summary_rar(src, password)?.image_count;
    let mut archive = rar_archive(src, password)?
        .as_first_part()
        .open_for_processing()
        .map_err(rar_error)?;
    let opts = store_options();
    let mut files_done: u32 = 0;
    let mut bytes_written: u64 = 0;
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        let Some(header) = archive.read_header().map_err(rar_error)? else {
            break;
        };
        let entry = header.entry();
        let raw_name = entry.filename.to_string_lossy().to_string();
        if !entry.is_file() || !is_image_entry(&raw_name) {
            archive = header.skip().map_err(rar_error)?;
            continue;
        }
        let Some(name) = normalize_entry_name(&raw_name) else {
            archive = header.skip().map_err(rar_error)?;
            continue;
        };
        let name = dedup_entry_name(name, &mut seen_names);
        let (data, next_archive) = header.read().map_err(rar_error)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        zw.start_file(&name, opts)
            .map_err(|e| ConvertError::Archive(e.to_string()))?;
        zw.write_all(&data)?;
        bytes_written = bytes_written.saturating_add(data.len() as u64);
        files_done += 1;
        if let Some(cb) = progress {
            cb(ConvertProgress {
                files_done,
                files_total,
                bytes_written,
            });
        }
        archive = next_archive;
    }

    Ok(ArchiveImageSummary {
        image_count: files_done,
        total_uncompressed_bytes: bytes_written,
    })
}

fn convert_7z(
    src: &Path,
    zw: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let mut reader = sevenz_rust2::ArchiveReader::open(src, Default::default())
        .map_err(|e| ConvertError::Archive(e.to_string()))?;

    // 事前に総画像数を数える (進捗表示用)
    let files_total: u32 = reader
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory && is_image_entry(&e.name))
        .count() as u32;

    let mut files_done: u32 = 0;
    let mut bytes_written: u64 = 0;
    let mut cancelled = false;
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    let opts = store_options();
    let iter_result = reader.for_each_entries(|entry, r| {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            return Ok(false);
        }
        if entry.is_directory || !is_image_entry(&entry.name) {
            // solid 7z (7-Zip 既定) は block 内の各ファイルが同一の逐次 stream を共有するため、
            // skip するエントリも読み捨てて stream を進めないと、後続エントリが前の残バイトを
            // 読んで画像が壊れる (v1.0.0 データ整合性レビュー DI-4)。directory は空 reader
            // なので 0 バイト copy で無害。
            std::io::copy(r, &mut std::io::sink())?;
            return Ok(true);
        }
        let Some(name) = normalize_entry_name(&entry.name) else {
            // 正規化不能な画像エントリも drain してから skip (同上、stream 整合のため)。
            std::io::copy(r, &mut std::io::sink())?;
            return Ok(true);
        };
        let name = dedup_entry_name(name, &mut seen_names);
        zw.start_file(&name, opts)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let copied = std::io::copy(r, zw)?;
        bytes_written = bytes_written.saturating_add(copied);
        files_done += 1;
        if let Some(cb) = progress {
            cb(ConvertProgress {
                files_done,
                files_total,
                bytes_written,
            });
        }
        Ok(true)
    });
    iter_result.map_err(|e| ConvertError::Archive(e.to_string()))?;

    if cancelled {
        return Err(ConvertError::Cancelled);
    }
    Ok(ArchiveImageSummary {
        image_count: files_done,
        total_uncompressed_bytes: bytes_written,
    })
}

/// 既に書き込んだ正規化名と衝突したら拡張子の前に " (N)" を挿入して一意化する。
/// normalize_entry_name は `\`→`/`・先頭 `/` 除去を行うため、元が異なる 2 エントリが同名に
/// なり得る。同名で複数 start_file すると read 時 by_name が 1 つしか返せず片方が不可視に
/// なるので、両方を個別に名前解決できるようにする (v1.0.0 データ整合性レビュー DI-5)。
fn dedup_entry_name(name: String, seen: &mut std::collections::HashSet<String>) -> String {
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

fn convert_lzh(
    src: &Path,
    zw: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    // LZH は事前にもう一度開いて総数を数える (ヘッダスキャンなので軽い)
    let files_total: u32 = {
        let mut r = delharc::parse_file(src).map_err(|e| ConvertError::Archive(e.to_string()))?;
        let mut total = 0u32;
        loop {
            let header = r.header();
            let pathname = header.parse_pathname();
            let name = pathname.to_string_lossy();
            if !header.is_directory() && is_image_entry(&name) {
                total += 1;
            }
            if !r
                .next_file()
                .map_err(|e| ConvertError::Archive(e.to_string()))?
            {
                break;
            }
        }
        total
    };

    let mut reader = delharc::parse_file(src).map_err(|e| ConvertError::Archive(e.to_string()))?;
    let opts = store_options();
    let mut files_done: u32 = 0;
    let mut bytes_written: u64 = 0;
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        let header = reader.header();
        let pathname = header.parse_pathname();
        let raw_name = pathname.to_string_lossy().to_string();
        let should_copy =
            !header.is_directory() && is_image_entry(&raw_name) && reader.is_decoder_supported();
        if should_copy {
            if let Some(name) = normalize_entry_name(&raw_name) {
                let name = dedup_entry_name(name, &mut seen_names);
                zw.start_file(&name, opts)
                    .map_err(|e| ConvertError::Archive(e.to_string()))?;
                let copied = std::io::copy(&mut reader, zw)?;
                bytes_written = bytes_written.saturating_add(copied);
                files_done += 1;
                if let Some(cb) = progress {
                    cb(ConvertProgress {
                        files_done,
                        files_total,
                        bytes_written,
                    });
                }
                // CRC 検証は失敗しても致命的ではない (ファイルは既に書き込み済み)
                // ログに残すだけに留める
                if let Err(e) = reader.crc_check() {
                    crate::logger::log(format!(
                        "archive_converter: LZH CRC mismatch for {raw_name}: {e}"
                    ));
                }
            }
        }
        if !reader
            .next_file()
            .map_err(|e| ConvertError::Archive(e.to_string()))?
        {
            break;
        }
    }

    Ok(ArchiveImageSummary {
        image_count: files_done,
        total_uncompressed_bytes: bytes_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rar_non_first_part_detection() {
        assert!(!is_non_first_rar_part(Path::new(r"C:\books\a.rar")));
        assert!(!is_non_first_rar_part(Path::new(r"C:\books\a.cbr")));
        assert!(!is_non_first_rar_part(Path::new(r"C:\books\a.part01.rar")));
        assert!(is_non_first_rar_part(Path::new(r"C:\books\a.part02.rar")));
        assert!(!is_non_first_rar_part(Path::new(r"C:\books\a.r00")));
        assert!(!is_non_first_rar_part(Path::new(r"C:\books\a.r01")));
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
