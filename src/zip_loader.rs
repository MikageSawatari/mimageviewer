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
        .map_or(0, |m| crate::ui_helpers::mtime_secs(&m));

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

/// ZIP ファイルの最初の画像エントリ名を返す。
/// フォルダ一覧でのサムネイル表示用 (1枚目のみ高速取得)。
pub fn first_image_entry(zip_path: &Path) -> Option<String> {
    let file = File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        if name.contains("__MACOSX/") || name.starts_with('.') {
            continue;
        }
        let Some(dot) = name.rfind('.') else { continue };
        let ext = name[dot + 1..].to_ascii_lowercase();
        if IMAGE_EXTS.contains(&ext.as_str()) {
            return Some(name.replace('\\', "/"));
        }
    }
    None
}

/// ZIP を 1 回だけ開き、最初の画像エントリを探してそのバイト列を読み取る。
///
/// `first_image_entry` + `read_entry_bytes` を 1 回の open で行う最適化版。
/// ネットワークドライブでは ZIP の open (セントラルディレクトリ読み取り) が
/// 高コストなため、2 回 → 1 回に削減することで体感速度が大幅に改善する。
///
/// 戻り値: `Some((entry_name, bytes))` or `None` (画像エントリが無い場合)
pub fn read_first_image_bytes(zip_path: &Path) -> Option<(String, Vec<u8>)> {
    let file = File::open(zip_path).ok()?;
    let file_size = file.metadata().ok().map(|m| m.len()).unwrap_or(0);
    // BufReader はデフォルト 8KB が最適。ZipArchive::new() はファイル末尾から
    // 逆方向にシークするため、大きなバッファはむしろ有害 (seek 後に不要な大量 read)。
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
    let entry_count = archive.len();

    let t0 = std::time::Instant::now();

    // まず最初の画像エントリのインデックスと名前を探す
    let mut found: Option<(usize, String)> = None;
    for i in 0..entry_count {
        let Ok(entry) = archive.by_index_raw(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        if name.contains("__MACOSX/") || name.starts_with('.') {
            continue;
        }
        let Some(dot) = name.rfind('.') else { continue };
        let ext = name[dot + 1..].to_ascii_lowercase();
        if IMAGE_EXTS.contains(&ext.as_str()) {
            found = Some((i, name.replace('\\', "/")));
            break;
        }
    }

    let scan_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let (idx, entry_name) = found?;
    // 同じ archive からエントリを展開して読み取る
    let mut entry = archive.by_index(idx).ok()?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).ok()?;

    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if total_ms > 50.0 {
        crate::logger::log(format!(
            "      [zip detail] entries={entry_count} zip_size={:.1}MB scan={scan_ms:.0}ms read={:.0}ms total={total_ms:.0}ms  {}",
            file_size as f64 / (1024.0 * 1024.0),
            total_ms - scan_ms,
            zip_path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        ));
    }

    Some((entry_name, bytes))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_dir_root_is_empty() {
        assert_eq!(entry_dir("img.jpg"), "");
        assert_eq!(entry_dir("file.png"), "");
    }

    #[test]
    fn entry_dir_one_level() {
        assert_eq!(entry_dir("work1/img.jpg"), "work1");
        assert_eq!(entry_dir("a/b.png"), "a");
    }

    #[test]
    fn entry_dir_nested() {
        assert_eq!(entry_dir("a/b/c.jpg"), "a/b");
        assert_eq!(entry_dir("dir/sub/img.png"), "dir/sub");
    }

    #[test]
    fn entry_basename_root() {
        assert_eq!(entry_basename("img.jpg"), "img.jpg");
    }

    #[test]
    fn entry_basename_one_level() {
        assert_eq!(entry_basename("work1/img.jpg"), "img.jpg");
    }

    #[test]
    fn entry_basename_nested() {
        assert_eq!(entry_basename("a/b/c.png"), "c.png");
    }

    #[test]
    fn entry_basename_empty_after_slash() {
        // 通常起こらないが防御
        assert_eq!(entry_basename("dir/"), "");
    }
}
