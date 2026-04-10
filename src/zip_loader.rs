//! ZIP ファイルを仮想フォルダとして扱うヘルパー (タスク 3)。
//!
//! ZIP 内の画像エントリを列挙し、必要に応じてエントリのバイト列を取り出す。
//! スレッドセーフのため、各呼び出しで ZIP を独立に開いている (共有ハンドルなし)。

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// サムネイル生成対象とする画像拡張子 (`app.rs` の `SUPPORTED_EXTENSIONS` と同じ)
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

/// ZIP エントリの情報 (画像のみ)
#[derive(Debug, Clone)]
pub struct ZipImageEntry {
    /// ZIP 内の相対パス (例: "work1/img01.jpg"、区切りは常に '/')
    pub entry_name: String,
    /// 非圧縮サイズ (bytes)
    pub uncompressed_size: u64,
    /// エントリの最終更新時刻 (UNIX 秒)。取得できない場合は ZIP ファイル自身の mtime
    pub mtime: i64,
}

/// ZIP ファイル内の画像エントリをすべて列挙する。
///
/// 戻り値はディレクトリ構造を保持した相対パスの順序 (ZIP 内出現順)。
/// 呼び出し側でサブディレクトリグループ化とソートを行う。
pub fn enumerate_image_entries(zip_path: &Path) -> std::io::Result<Vec<ZipImageEntry>> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    // ZIP 自身の mtime をフォールバックに使う
    let zip_mtime = std::fs::metadata(zip_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut out: Vec<ZipImageEntry> = Vec::new();
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        // 隠しファイルや macOS のメタデータを除外
        if name.contains("__MACOSX/") || name.starts_with('.') {
            continue;
        }
        let Some(dot) = name.rfind('.') else { continue };
        let ext = name[dot + 1..].to_ascii_lowercase();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        // 区切りを '/' に正規化
        let normalized = name.replace('\\', "/");
        out.push(ZipImageEntry {
            entry_name: normalized,
            uncompressed_size: entry.size(),
            // zip crate は DOS 時刻をそのまま返すため変換が複雑。
            // ここでは ZIP ファイル自身の mtime をフォールバックとして使う。
            // キャッシュ整合性は "エントリ名 + uncompressed_size + zip 自身の mtime" で十分。
            mtime: zip_mtime,
        });
    }
    Ok(out)
}

/// ZIP 内の特定エントリの生バイト列を取り出す。
///
/// 呼び出しごとに ZIP を開き直すため、多数のエントリを連続で読むと
/// オーバーヘッドが発生する (エントリあたり数 ms)。現状のサムネイル
/// 生成レート (1 スレッドあたり 50-100 枚/秒) であれば許容範囲。
pub fn read_entry_bytes(zip_path: &Path, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// ZIP 内エントリ名からサブディレクトリ名 (親ディレクトリ) を取り出す。
/// ルート直下のエントリは空文字列を返す。
pub fn entry_dir(entry_name: &str) -> &str {
    match entry_name.rfind('/') {
        Some(pos) => &entry_name[..pos],
        None => "",
    }
}

/// ZIP 内エントリ名からファイル名だけを取り出す。
pub fn entry_basename(entry_name: &str) -> &str {
    match entry_name.rfind('/') {
        Some(pos) => &entry_name[pos + 1..],
        None => entry_name,
    }
}
