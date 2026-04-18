//! ZIP ファイルを仮想フォルダとして扱うヘルパー (タスク 3 / v0.7.0)。
//!
//! ZIP 内の画像エントリを列挙し、必要に応じてエントリのバイト列を取り出す。
//! v0.7.0 からネスト ZIP (ZIP in ZIP) に対応。外側 ZIP のエントリに `.zip`
//! ファイルがあると再帰的に中身を列挙し、フラットに画像を並べる。
//!
//! # 内側 ZIP バイト列のキャッシュ
//!
//! ネスト ZIP の読み取り (`read_entry_bytes` を通じた個別エントリ取得) では、
//! 親 ZIP から同じ子 ZIP を何度も抽出するコストを避けるためバイト列をキャッシュする。
//! これは単なる「ファイル一覧」ではなく、**子 ZIP の圧縮バイト列そのもの**で、
//! 任意のエントリを読むたびに必要になる。
//!
//! - **容量上限**: 物理 RAM の 25%。ただし 4GB で頭打ち (安全弁)。搭載 RAM が
//!   32GB あれば 4GB、8GB なら 2GB 確保する。
//! - **ヒット率最大化**: 上限内では LRU eviction を行わない
//!   (すなわち、典型的な 200MB〜1GB 程度の漫画アーカイブは全て常駐する)。
//! - **ナビゲーション時クリア**: 別フォルダ/ZIP を開いたら `clear_all()` で全破棄し、
//!   外側 ZIP を切り替えても古いキャッシュが居残らないようにする。
//!
//! スレッドセーフのため、各呼び出しで ZIP を独立に開いている (共有ハンドルなし)。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

const ZIP_EXT: &str = "zip";

// ── 内側 ZIP バイト列キャッシュ ──────────────────────────────────

/// ネスト ZIP の展開済みバイト列を保持するキャッシュ。
///
/// 上限は起動時に `sys_memory::nested_zip_cache_budget()` (物理 RAM の 25%,
/// 最大 4GB) で決定する。上限を超過したときのみ LRU eviction を行う
/// (通常のユースケースでは evict は起きない)。
///
/// 外側 ZIP / PDF / フォルダを切り替えた際は `clear_all()` でまとめて破棄する。
/// これで「別アーカイブに移動したのに古い章のバイト列が居残る」を防ぐ。
struct NestedZipCache {
    inner: Mutex<NestedZipCacheInner>,
    max_bytes: usize,
}

struct NestedZipCacheInner {
    entries: Vec<NestedCacheEntry>,
    current_bytes: usize,
}

struct NestedCacheEntry {
    zip_path: PathBuf,
    nested_path: String,
    bytes: Arc<Vec<u8>>,
    last_used: Instant,
}

impl NestedZipCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(NestedZipCacheInner {
                entries: Vec::new(),
                current_bytes: 0,
            }),
            max_bytes,
        }
    }

    fn get(&self, zip_path: &Path, nested_path: &str) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.inner.lock().ok()?;
        for e in inner.entries.iter_mut() {
            if e.zip_path == zip_path && e.nested_path == nested_path {
                e.last_used = Instant::now();
                return Some(e.bytes.clone());
            }
        }
        None
    }

    fn insert(&self, zip_path: PathBuf, nested_path: String, bytes: Arc<Vec<u8>>) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if let Some(pos) = inner
            .entries
            .iter()
            .position(|e| e.zip_path == zip_path && e.nested_path == nested_path)
        {
            let removed = inner.entries.swap_remove(pos);
            inner.current_bytes = inner.current_bytes.saturating_sub(removed.bytes.len());
        }
        let add_size = bytes.len();
        if add_size > self.max_bytes {
            // 単一の内側 ZIP が予算を上回る場合はキャッシュしない (呼び出し側は都度展開)。
            // 典型的に 4GB を超える単一子 ZIP は想定外だが、安全弁として残す。
            return;
        }
        // 予算超過時のみ LRU eviction。通常ユースケース (200MB〜1GB) ではこのループは回らない。
        while inner.current_bytes + add_size > self.max_bytes && !inner.entries.is_empty() {
            let oldest_idx = inner
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .unwrap();
            let removed = inner.entries.swap_remove(oldest_idx);
            inner.current_bytes = inner.current_bytes.saturating_sub(removed.bytes.len());
        }
        inner.current_bytes += add_size;
        inner.entries.push(NestedCacheEntry {
            zip_path,
            nested_path,
            bytes,
            last_used: Instant::now(),
        });
    }

    fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
            inner.current_bytes = 0;
        }
    }
}

static NESTED_CACHE: LazyLock<NestedZipCache> =
    LazyLock::new(|| NestedZipCache::new(crate::sys_memory::nested_zip_cache_budget()));

/// 外側のフォルダ/ZIP/PDF を切り替えたときに呼ぶ。
/// 全エントリを破棄し、古い外側のキャッシュが居残らないようにする。
pub fn clear_nested_cache() {
    NESTED_CACHE.clear();
}

// ── 共通ヘルパー ────────────────────────────────────────────────

/// エントリ名を無視すべきか判定 (macOS メタデータ・ドットファイル)。
fn should_ignore(name: &str) -> bool {
    name.contains("__MACOSX/") || name.starts_with('.')
}

/// エントリ名から拡張子を小文字で取り出す。
/// ファイル名部分に '.' がない場合は None。
fn lowercase_ext(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    let base_start = name.rfind('/').map(|s| s + 1).unwrap_or(0);
    if dot < base_start {
        return None;
    }
    Some(name[dot + 1..].to_ascii_lowercase())
}

/// ZIP 内エントリが画像として扱える拡張子か判定する。
///
/// 通常フォルダ・7z/LZH 変換と同じ [`crate::folder_tree::is_recognized_image_ext`]
/// に委譲することで、ネイティブ (image クレート) ・WIC (HEIC / AVIF / JXL / TIFF /
/// RAW) ・ロード済み Susie プラグイン (PI / MAG / Q0 等) の対応拡張子が
/// すべて ZIP 内でも同じように認識される。
///
/// 以前はここに独自のハードコードリスト (jpg/jpeg/png/webp/bmp/gif) を持っていて、
/// ZIP 内の HEIC や MAG が本体では開けるのにサムネイル一覧に出てこないという
/// 不整合があった (v0.7.0 で修正)。
fn is_image_ext(ext_lower: &str) -> bool {
    crate::folder_tree::is_recognized_image_ext(ext_lower)
}

/// エントリ名に ".zip/" 境界があれば境界位置 (境界 '/' の絶対 byte 位置) を列挙。
/// 大文字小文字を区別しない。
fn find_nested_zip_boundaries(entry_name: &str) -> Vec<usize> {
    let lower = entry_name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut boundaries = Vec::new();
    let mut i = 0;
    while i + 5 <= bytes.len() {
        if &bytes[i..i + 5] == b".zip/" {
            boundaries.push(i + 4); // '/' の絶対位置
            i += 5;
        } else {
            i += 1;
        }
    }
    boundaries
}

/// エントリ名をネスト ZIP 境界で分割する。
/// 戻り値: 各セグメント。先頭 n-1 個が nested zip パス (末尾 ".zip")、最後が葉。
/// 境界がなければ長さ 1 の単一セグメントを返す。
fn split_nested_zip_path(entry_name: &str) -> Vec<&str> {
    let boundaries = find_nested_zip_boundaries(entry_name);
    if boundaries.is_empty() {
        return vec![entry_name];
    }
    let mut parts = Vec::with_capacity(boundaries.len() + 1);
    let mut start = 0;
    for b in boundaries {
        parts.push(&entry_name[start..b]);
        start = b + 1;
    }
    parts.push(&entry_name[start..]);
    parts
}

// ── 公開 API ────────────────────────────────────────────────────

/// ZIP エントリの情報 (画像のみ)
#[derive(Debug, Clone)]
pub struct ZipImageEntry {
    /// ZIP 内の相対パス (例: "work1/img01.jpg"、区切りは常に '/')
    /// ネスト ZIP 内の画像は "chapters/ch01.zip/page01.jpg" 形式。
    pub entry_name: String,
    /// 非圧縮サイズ (bytes)
    pub uncompressed_size: u64,
    /// エントリの最終更新時刻 (UNIX 秒)。取得できない場合は ZIP ファイル自身の mtime
    pub mtime: i64,
}

/// ZIP ファイル内の画像エントリをすべて列挙する。
///
/// 戻り値はディレクトリ構造を保持した相対パスの順序 (ZIP 内出現順)。
/// ネスト ZIP は再帰展開され、パスに親 ZIP 名が含まれる
/// (例: "outer/ch01.zip/page01.jpg")。
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
    enumerate_recursive(&mut archive, zip_path, "", zip_mtime, &mut out);
    Ok(out)
}

fn enumerate_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    zip_mtime: i64,
    out: &mut Vec<ZipImageEntry>,
) {
    let len = archive.len();
    for i in 0..len {
        let Ok(mut entry) = archive.by_index(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let raw_name = entry.name().to_string();
        let name = raw_name.replace('\\', "/");
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else { continue };
        let full_name = format!("{prefix}{name}");
        if is_image_ext(&ext) {
            out.push(ZipImageEntry {
                entry_name: full_name,
                uncompressed_size: entry.size(),
                mtime: zip_mtime,
            });
            continue;
        }
        if ext == ZIP_EXT {
            let size = entry.size();
            let cached = NESTED_CACHE.get(outer_zip_path, &full_name);
            let bytes = match cached {
                Some(b) => {
                    drop(entry);
                    b
                }
                None => {
                    let mut buf = Vec::with_capacity(size as usize);
                    if entry.read_to_end(&mut buf).is_err() {
                        continue;
                    }
                    drop(entry);
                    let arc = Arc::new(buf);
                    NESTED_CACHE.insert(
                        outer_zip_path.to_path_buf(),
                        full_name.clone(),
                        arc.clone(),
                    );
                    arc
                }
            };
            let cursor = Cursor::new(bytes.as_slice());
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else { continue };
            let new_prefix = format!("{full_name}/");
            enumerate_recursive(&mut inner, outer_zip_path, &new_prefix, zip_mtime, out);
        }
    }
}

/// ZIP ファイルの最初の画像エントリ名を返す。
/// フォルダ一覧でのサムネイル表示用 (1枚目のみ高速取得) と、
/// `folder_should_stop` での画像有無判定に使う。ネスト ZIP 内にしか画像がない
/// 場合も追跡して返す。
///
/// `cancel` が指定されていれば各エントリ検査前にチェックし、セット時は
/// `None` を返して早期離脱する (巨大な非画像 ZIP のスキャン中に Ctrl+↑↓
/// 連打がきたとき DFS をすぐ畳めるようにするため)。
pub fn first_image_entry(zip_path: &Path, cancel: Option<&AtomicBool>) -> Option<String> {
    let file = File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
    first_image_recursive(&mut archive, zip_path, "", cancel)
}

fn first_image_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    cancel: Option<&AtomicBool>,
) -> Option<String> {
    let len = archive.len();
    for i in 0..len {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return None;
        }
        let Ok(mut entry) = archive.by_index(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string().replace('\\', "/");
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else { continue };
        let full_name = format!("{prefix}{name}");
        if is_image_ext(&ext) {
            return Some(full_name);
        }
        if ext == ZIP_EXT {
            let size = entry.size();
            let cached = NESTED_CACHE.get(outer_zip_path, &full_name);
            let bytes = match cached {
                Some(b) => {
                    drop(entry);
                    b
                }
                None => {
                    let mut buf = Vec::with_capacity(size as usize);
                    if entry.read_to_end(&mut buf).is_err() {
                        continue;
                    }
                    drop(entry);
                    let arc = Arc::new(buf);
                    NESTED_CACHE.insert(
                        outer_zip_path.to_path_buf(),
                        full_name.clone(),
                        arc.clone(),
                    );
                    arc
                }
            };
            let cursor = Cursor::new(bytes.as_slice());
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else { continue };
            let new_prefix = format!("{full_name}/");
            if let Some(found) =
                first_image_recursive(&mut inner, outer_zip_path, &new_prefix, cancel)
            {
                return Some(found);
            }
        }
    }
    None
}

/// ZIP を 1 回だけ開き、最初の画像エントリを探してそのバイト列を読み取る。
///
/// ネットワークドライブでは ZIP の open が高コストなため、外側 ZIP については
/// 1 回 open を維持する。ネスト ZIP 展開時は内側を `Cursor` 経由で開くので
/// 追加の disk I/O は発生しない。
///
/// 戻り値: `Some((entry_name, bytes))` or `None` (画像エントリが無い場合)
pub fn read_first_image_bytes(zip_path: &Path) -> Option<(String, Vec<u8>)> {
    let file = File::open(zip_path).ok()?;
    let file_size = file.metadata().ok().map(|m| m.len()).unwrap_or(0);
    let t0 = std::time::Instant::now();
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
    let result = read_first_image_recursive(&mut archive, zip_path, "");
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if total_ms > 50.0 {
        if let Some((ref name, ref bytes)) = result {
            crate::logger::log(format!(
                "      [zip detail] zip_size={:.1}MB total={total_ms:.0}ms bytes={} {}  {}",
                file_size as f64 / (1024.0 * 1024.0),
                bytes.len(),
                name,
                zip_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?"),
            ));
        }
    }
    result
}

fn read_first_image_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
) -> Option<(String, Vec<u8>)> {
    let len = archive.len();
    for i in 0..len {
        let Ok(mut entry) = archive.by_index(i) else { continue };
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string().replace('\\', "/");
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else { continue };
        let full_name = format!("{prefix}{name}");
        if is_image_ext(&ext) {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            if entry.read_to_end(&mut bytes).is_err() {
                continue;
            }
            return Some((full_name, bytes));
        }
        if ext == ZIP_EXT {
            let size = entry.size();
            let cached = NESTED_CACHE.get(outer_zip_path, &full_name);
            let bytes = match cached {
                Some(b) => {
                    drop(entry);
                    b
                }
                None => {
                    let mut buf = Vec::with_capacity(size as usize);
                    if entry.read_to_end(&mut buf).is_err() {
                        continue;
                    }
                    drop(entry);
                    let arc = Arc::new(buf);
                    NESTED_CACHE.insert(
                        outer_zip_path.to_path_buf(),
                        full_name.clone(),
                        arc.clone(),
                    );
                    arc
                }
            };
            let cursor = Cursor::new(bytes.as_slice());
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else { continue };
            let new_prefix = format!("{full_name}/");
            if let Some(found) = read_first_image_recursive(&mut inner, outer_zip_path, &new_prefix)
            {
                return Some(found);
            }
        }
    }
    None
}

/// ZIP 内の特定エントリの生バイト列を取り出す。
///
/// `entry_name` がネスト ZIP パス (例: "chapters/ch01.zip/page01.jpg") の場合、
/// 途中の `.zip` ファイルを順に展開して読み取る。中間バイト列は LRU キャッシュに
/// 保持されるため、同じ内側 ZIP 内のエントリを連続で読む場合は再展開コストが
/// 発生しない。
pub fn read_entry_bytes(zip_path: &Path, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let parts = split_nested_zip_path(entry_name);
    if parts.len() == 1 {
        return read_entry_from_disk(zip_path, entry_name);
    }

    // 最深のキャッシュヒットを探す。キー = parts[0..level].join("/")
    let mut current_bytes: Option<Arc<Vec<u8>>> = None;
    let mut start_level: usize = 0;
    for level in (1..parts.len()).rev() {
        let key = parts[0..level].join("/");
        if let Some(b) = NESTED_CACHE.get(zip_path, &key) {
            current_bytes = Some(b);
            start_level = level;
            break;
        }
    }

    // start_level から葉 (parts.len() - 1) までを順に展開しながら読む。
    // キャッシュヒットしなかった場合、start_level = 0 で外側 ZIP から開始する。
    let mut level = start_level;
    while level < parts.len() - 1 {
        // parts[level] は内側 ZIP のエントリ。中身は別の ZIP バイト列。
        let next_bytes: Vec<u8> = match &current_bytes {
            Some(b) => read_entry_from_bytes(b, parts[level])?,
            None => read_entry_from_disk(zip_path, parts[level])?,
        };
        let arc = Arc::new(next_bytes);
        let key_so_far = parts[0..=level].join("/");
        NESTED_CACHE.insert(zip_path.to_path_buf(), key_so_far, arc.clone());
        current_bytes = Some(arc);
        level += 1;
    }

    // 葉の読み取り
    let leaf = parts[parts.len() - 1];
    match &current_bytes {
        Some(b) => read_entry_from_bytes(b, leaf),
        None => read_entry_from_disk(zip_path, leaf),
    }
}

fn read_entry_from_disk(zip_path: &Path, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    read_by_name(&mut archive, entry_name)
}

fn read_entry_from_bytes(bytes: &Arc<Vec<u8>>, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let cursor = Cursor::new(bytes.as_slice());
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    read_by_name(&mut archive, entry_name)
}

fn read_by_name<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
) -> std::io::Result<Vec<u8>> {
    let mut entry = archive
        .by_name(entry_name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// 複数エントリをまとめて読むときのアーカイブハンドル型。
/// `zip` クレートの型を隠蔽するため `zip_loader` 外から名前で参照できるようにする。
///
/// このハンドルは**外側 ZIP のみ**を保持する。ネストパスを読む場合は
/// `read_entry_bytes` を使い、関数側でネスト境界を解釈させること。
pub type ZipArchiveHandle = zip::ZipArchive<BufReader<File>>;

/// ZIP を開いて `ZipArchiveHandle` を返す。
/// ネットワークドライブなど open が高コストな場合、同じハンドルから複数エントリを
/// 順に読めるようにするためのバッチ処理用入り口。
pub fn open_archive(zip_path: &Path) -> std::io::Result<ZipArchiveHandle> {
    let file = File::open(zip_path)?;
    zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// すでに開いた `ZipArchiveHandle` から 1 エントリの生バイト列を読む。
/// **ネストパスには対応しない** (`.zip/` を含む entry_name は `read_entry_bytes` を使うこと)。
pub fn read_entry_from_archive(
    archive: &mut ZipArchiveHandle,
    entry_name: &str,
) -> std::io::Result<Vec<u8>> {
    read_by_name(archive, entry_name)
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

    #[test]
    fn lowercase_ext_simple() {
        assert_eq!(lowercase_ext("img.JPG").as_deref(), Some("jpg"));
        assert_eq!(lowercase_ext("dir/a.PNG").as_deref(), Some("png"));
    }

    /// `is_image_ext` がネイティブ対応拡張子に加え WIC 対応拡張子 (HEIC/AVIF/JXL/
    /// TIFF/RAW) も ZIP 内で認識することを確認する回帰テスト。以前はハードコードの
    /// 6 種 (jpg/jpeg/png/webp/bmp/gif) だけを見ていて、ZIP 内の HEIC などが本体
    /// で開けるのにサムネイル一覧に出てこない不整合があった。
    ///
    /// Susie 対応拡張子 (PI / MAG 等) はテスト環境ではプール未初期化のため
    /// ここでは検証できないが、実行時は同じ `is_recognized_image_ext` を通るので
    /// ZIP でも認識される。
    #[test]
    fn is_image_ext_includes_native_and_wic_formats() {
        // ネイティブ (image クレート)
        assert!(is_image_ext("jpg"));
        assert!(is_image_ext("png"));
        assert!(is_image_ext("webp"));
        assert!(is_image_ext("bmp"));
        assert!(is_image_ext("gif"));
        // WIC
        assert!(is_image_ext("heic"));
        assert!(is_image_ext("avif"));
        assert!(is_image_ext("jxl"));
        assert!(is_image_ext("tiff"));
        assert!(is_image_ext("cr2"));
        assert!(is_image_ext("arw"));
        // 画像でないもの
        assert!(!is_image_ext("mp4"));
        assert!(!is_image_ext("txt"));
        assert!(!is_image_ext("zip"));
    }

    #[test]
    fn lowercase_ext_no_dot() {
        assert_eq!(lowercase_ext("nodotfile"), None);
    }

    #[test]
    fn lowercase_ext_dot_only_in_dir() {
        // "dir.with.dot/file" には拡張子はない
        assert_eq!(lowercase_ext("dir.with.dot/file"), None);
    }

    #[test]
    fn split_nested_flat() {
        let parts = split_nested_zip_path("work/img.jpg");
        assert_eq!(parts, vec!["work/img.jpg"]);
    }

    #[test]
    fn split_nested_one_level() {
        let parts = split_nested_zip_path("chapters/ch01.zip/page01.jpg");
        assert_eq!(parts, vec!["chapters/ch01.zip", "page01.jpg"]);
    }

    #[test]
    fn split_nested_two_levels() {
        let parts = split_nested_zip_path("a.zip/b.zip/img.png");
        assert_eq!(parts, vec!["a.zip", "b.zip", "img.png"]);
    }

    #[test]
    fn split_nested_case_insensitive() {
        let parts = split_nested_zip_path("CH01.ZIP/page.jpg");
        assert_eq!(parts, vec!["CH01.ZIP", "page.jpg"]);
    }

    #[test]
    fn split_nested_with_subdir_between() {
        let parts = split_nested_zip_path("pack.zip/sub/inner.zip/img.png");
        assert_eq!(parts, vec!["pack.zip", "sub/inner.zip", "img.png"]);
    }
}
