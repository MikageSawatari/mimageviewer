//! 7z / LZH アーカイブを ZIP (STORE) に変換するコンバータ (v0.7.0)。
//!
//! mImageViewer は ZIP での閲覧に最適化されているため、7z / LZH をクリックしたら
//! 中身の画像だけを抜き出して無圧縮 ZIP に変換しておき、以降は通常の ZIP として開く。
//!
//! - 対応: 7z (sevenz-rust2), LZH (delharc)。RAR はライセンス都合で非対応。
//! - 出力: 常に STORE モード (中身は既に JPEG/PNG で圧縮済み、再圧縮無意味)。
//! - 対象: 画像エントリ (`folder_tree::SUPPORTED_EXTENSIONS`) のみ。非画像は破棄。
//! - キャンセル: `Arc<AtomicBool>` を各エントリ境界でチェック。
//! - 進捗: `Fn(ConvertProgress)` コールバック。
//!
//! キャッシュ管理は [`archive_cache`] 側の責務で、本モジュールは純粋な変換ロジックのみ。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::folder_tree::SUPPORTED_EXTENSIONS;

/// 変換対応アーカイブ形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    SevenZ,
    Lzh,
}

impl ArchiveFormat {
    /// 拡張子から形式を判定する。大文字小文字無視。対応外なら None。
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "7z" => Some(Self::SevenZ),
            "lzh" | "lha" => Some(Self::Lzh),
            _ => None,
        }
    }

    /// 形式のラベル (バッジ / ダイアログ表示用)。
    pub fn label(self) -> &'static str {
        match self {
            Self::SevenZ => "7z",
            Self::Lzh => "LZH",
        }
    }
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
fn is_image_entry(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else { return false };
    // パス区切り後に '.' があることを確認 (ディレクトリパス中の '.' を誤検出しない)
    let last_sep = name.rfind(|c: char| c == '/' || c == '\\').map_or(0, |i| i + 1);
    if dot < last_sep {
        return false;
    }
    let ext = name[dot + 1..].to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.iter().any(|e| *e == ext)
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
    match format {
        ArchiveFormat::SevenZ => scan_summary_7z(path),
        ArchiveFormat::Lzh => scan_summary_lzh(path),
    }
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
    let mut reader = delharc::parse_file(path)
        .map_err(|e| ConvertError::Archive(e.to_string()))?;
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
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 中間ファイルに書いて atomic rename。途中失敗時に壊れた zip が残らないようにする。
    let tmp_path = dst.with_extension("zip.part");
    let _ = std::fs::remove_file(&tmp_path);

    let summary = match do_convert(src, &tmp_path, format, cancel, progress) {
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
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    let out_file = std::fs::File::create(tmp_path)?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(out_file));

    let summary = match format {
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

    // sevenz-rust2 の for_each_entries は solid 圧縮でも正しく動く
    let opts = store_options();
    let iter_result = reader.for_each_entries(|entry, r| {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            return Ok(false);
        }
        if entry.is_directory || !is_image_entry(&entry.name) {
            // スキップ (for_each_entries は Read から読まなくても内部で進む)
            return Ok(true);
        }
        let Some(name) = normalize_entry_name(&entry.name) else {
            return Ok(true);
        };
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

fn convert_lzh(
    src: &Path,
    zw: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    cancel: &AtomicBool,
    progress: Option<&dyn Fn(ConvertProgress)>,
) -> Result<ArchiveImageSummary, ConvertError> {
    // LZH は事前にもう一度開いて総数を数える (ヘッダスキャンなので軽い)
    let files_total: u32 = {
        let mut r = delharc::parse_file(src)
            .map_err(|e| ConvertError::Archive(e.to_string()))?;
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

    let mut reader = delharc::parse_file(src)
        .map_err(|e| ConvertError::Archive(e.to_string()))?;
    let opts = store_options();
    let mut files_done: u32 = 0;
    let mut bytes_written: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ConvertError::Cancelled);
        }
        let header = reader.header();
        let pathname = header.parse_pathname();
        let raw_name = pathname.to_string_lossy().to_string();
        let should_copy = !header.is_directory()
            && is_image_entry(&raw_name)
            && reader.is_decoder_supported();
        if should_copy {
            if let Some(name) = normalize_entry_name(&raw_name) {
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
    fn format_from_extension() {
        assert_eq!(ArchiveFormat::from_extension("7z"), Some(ArchiveFormat::SevenZ));
        assert_eq!(ArchiveFormat::from_extension("7Z"), Some(ArchiveFormat::SevenZ));
        assert_eq!(ArchiveFormat::from_extension("lzh"), Some(ArchiveFormat::Lzh));
        assert_eq!(ArchiveFormat::from_extension("lha"), Some(ArchiveFormat::Lzh));
        assert_eq!(ArchiveFormat::from_extension("LHA"), Some(ArchiveFormat::Lzh));
        assert_eq!(ArchiveFormat::from_extension("zip"), None);
        assert_eq!(ArchiveFormat::from_extension("rar"), None);
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

    #[test]
    fn normalize_entry_name_zip_slip() {
        assert_eq!(normalize_entry_name("foo/bar.jpg"), Some("foo/bar.jpg".to_string()));
        assert_eq!(normalize_entry_name("foo\\bar.jpg"), Some("foo/bar.jpg".to_string()));
        assert_eq!(normalize_entry_name("/abs/path.jpg"), Some("abs/path.jpg".to_string()));
        assert_eq!(normalize_entry_name("../escape.jpg"), None);
        assert_eq!(normalize_entry_name("a/../b.jpg"), None);
        assert_eq!(normalize_entry_name(""), None);
        assert_eq!(normalize_entry_name("dir/"), None);
    }
}
