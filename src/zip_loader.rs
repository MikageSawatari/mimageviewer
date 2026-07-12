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
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
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

// ── CP932 デコード名 → entry index のキャッシュ ─────────────────────
//
// 非 UTF-8 名 ZIP (CP932) では zip crate の `by_name` (CP437/UTF-8 デコード名 keyed)
// が**必ず**ミスし、デコード名による線形走査へ落ちる。読み戻しはエントリ単位で
// アーカイブを開き直すため、素朴な走査だと 1 冊のページ送り/サムネ一括生成が
// O(N²) になる (2,000 ページで ~4M 回の SHIFT_JIS デコード)。
// 「正規化デコード名 → index」はアーカイブが変わらない限り不変なので、
// (path, len, mtime) keyed で 1 回だけ構築し、以後は O(1) で引く。
#[derive(Clone, PartialEq, Eq)]
struct DecodedNameCacheKey {
    path: PathBuf,
    len: u64,
    mtime: Option<std::time::SystemTime>,
}

const DECODED_NAME_CACHE_MAX_ARCHIVES: usize = 8;

static DECODED_NAME_INDEX_CACHE: LazyLock<
    Mutex<
        Vec<(
            DecodedNameCacheKey,
            Arc<std::collections::HashMap<String, usize>>,
        )>,
    >,
> = LazyLock::new(|| Mutex::new(Vec::new()));

fn decoded_name_cache_key(zip_path: &Path) -> Option<DecodedNameCacheKey> {
    let meta = std::fs::metadata(zip_path).ok()?;
    Some(DecodedNameCacheKey {
        path: zip_path.to_path_buf(),
        len: meta.len(),
        mtime: meta.modified().ok(),
    })
}

fn decoded_name_cache_get(
    key: &DecodedNameCacheKey,
) -> Option<Arc<std::collections::HashMap<String, usize>>> {
    let cache = DECODED_NAME_INDEX_CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, map)| Arc::clone(map))
}

fn decoded_name_cache_put(
    key: DecodedNameCacheKey,
    map: Arc<std::collections::HashMap<String, usize>>,
) {
    let Ok(mut cache) = DECODED_NAME_INDEX_CACHE.lock() else {
        return;
    };
    cache.retain(|(k, _)| k.path != key.path);
    if cache.len() >= DECODED_NAME_CACHE_MAX_ARCHIVES {
        cache.remove(0); // 最古を捨てる (典型は同時 1〜2 冊なので十分)
    }
    cache.push((key, map));
}

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

fn has_japanese_or_fullwidth_chars(s: &str) -> bool {
    s.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3040..=0x30ff | 0x3400..=0x9fff | 0xf900..=0xfaff | 0xff00..=0xffef
        )
    })
}

/// 非 UTF-8 フラグの ZIP エントリ名を Shift-JIS (CP932) として解釈する。
/// zip crate の既定は CP437 で、日本語名が mojibake になる。
/// **このデコードは ZIP 名を扱う全経路で共有すること** (列挙・読み戻し・
/// archive_converter の変換出力)。経路ごとに生 `entry.name()` を使うと、
/// 直接閲覧と変換キャッシュでエントリ名がずれて per-page キーが割れる。
pub(crate) fn decode_zip_entry_name(raw: &[u8], fallback_name: &str) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }

    let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(raw);
    if !had_errors && has_japanese_or_fullwidth_chars(&decoded) {
        return decoded.into_owned();
    }

    fallback_name.to_string()
}

pub(crate) fn zip_entry_name(entry: &zip::read::ZipFile<'_>) -> String {
    decode_zip_entry_name(entry.name_raw(), entry.name())
}

fn normalized_zip_entry_name(entry: &zip::read::ZipFile<'_>) -> String {
    zip_entry_name(entry).replace('\\', "/")
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
/// 通常フォルダ・RAR/7z/LZH 変換と同じ [`crate::folder_tree::is_recognized_image_ext`]
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

/// エントリ名に ".zip/" / ".cbz/" 境界があれば境界位置 (境界 '/' の絶対 byte 位置) を列挙。
/// 大文字小文字を区別しない。CBZ は実体が ZIP なので、列挙 (`enumerate_image_entries`)
/// 側でネスト .cbz も再帰展開する。それと一致させ、読み戻し (`read_entry_bytes`) でも
/// .cbz 境界で分割できるようにする (両者がずれると「列挙されるが読めない」不整合になる)。
/// `.zip/` と `.cbz/` はどちらも 5 byte で対称。
fn find_nested_zip_boundaries(entry_name: &str) -> Vec<usize> {
    let lower = entry_name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut boundaries = Vec::new();
    let mut i = 0;
    while i + 5 <= bytes.len() {
        let seg = &bytes[i..i + 5];
        if seg == b".zip/" || seg == b".cbz/" {
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

/// `enumerate_image_entries_detailed` の結果 (エントリ + 付帯情報)。
#[derive(Debug)]
pub struct ZipEnumeration {
    pub entries: Vec<ZipImageEntry>,
    /// ZIP 内 (ネスト ZIP 内含む) に、変換対応の**非 ZIP** アーカイブ
    /// (RAR/CBR/7z/CB7/LZH/LHA) のファイルエントリが存在したか。それらの中身は
    /// この列挙には**含まれない** (ZIP ネイティブ経路では読めないため)。true の場合、
    /// 呼び出し側が「入れ子を展開した閲覧用キャッシュへの変換」を提案する (v1.3.0)。
    pub has_foreign_archives: bool,
    /// CP932 デコード対応 (v1.4.0) で **v1.3.x までと entry_name が変わったエントリ**の
    /// `(旧名, 新名)` ペア。旧名 = zip crate の既定 (CP437) デコード結果で、リリース済みの
    /// per-page DB キー (★/補正/注釈等) はこの旧名から導出されている。呼び出し側は
    /// このペアで旧キー → 新キーの一度きり移行を行う (`zip_key_migration`)。
    /// UTF-8 名 ZIP では常に空。
    pub legacy_renames: Vec<(String, String)>,
}

/// ZIP ファイル内の画像エントリをすべて列挙する。
///
/// 戻り値はディレクトリ構造を保持した相対パスの順序 (ZIP 内出現順)。
/// ネスト ZIP は再帰展開され、パスに親 ZIP 名が含まれる
/// (例: "outer/ch01.zip/page01.jpg")。
/// 呼び出し側でサブディレクトリグループ化とソートを行う。
pub fn enumerate_image_entries(zip_path: &Path) -> std::io::Result<Vec<ZipImageEntry>> {
    enumerate_image_entries_detailed(zip_path).map(|d| d.entries)
}

/// `enumerate_image_entries` + 付帯情報 (非 ZIP アーカイブの有無)。
pub fn enumerate_image_entries_detailed(zip_path: &Path) -> std::io::Result<ZipEnumeration> {
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::enumerate_image_entries_detailed(zip_path);
    }
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    // ZIP 自身の mtime をフォールバックに使う
    let zip_mtime = std::fs::metadata(zip_path)
        .ok()
        .map_or(0, |m| crate::ui_helpers::mtime_secs(&m));

    let mut out: Vec<ZipImageEntry> = Vec::new();
    let mut has_foreign = false;
    let mut legacy_renames: Vec<(String, String)> = Vec::new();
    enumerate_recursive(
        &mut archive,
        zip_path,
        "",
        "",
        zip_mtime,
        &mut out,
        &mut has_foreign,
        &mut legacy_renames,
    );
    Ok(ZipEnumeration {
        entries: out,
        has_foreign_archives: has_foreign,
        legacy_renames,
    })
}

#[allow(clippy::too_many_arguments)]
fn enumerate_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    legacy_prefix: &str,
    zip_mtime: i64,
    out: &mut Vec<ZipImageEntry>,
    has_foreign: &mut bool,
    legacy_renames: &mut Vec<(String, String)>,
) {
    let len = archive.len();
    for i in 0..len {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        let name = normalized_zip_entry_name(&entry);
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else {
            continue;
        };
        let full_name = format!("{prefix}{name}");
        // v1.3.x までの entry_name (zip crate の CP437 デコード)。CP932 名 ZIP では
        // 新デコード名と異なり、その差分がリリース済み per-page キーの移行対象になる。
        let legacy_name = entry.name().replace('\\', "/");
        let legacy_full = format!("{legacy_prefix}{legacy_name}");
        if is_image_ext(&ext) {
            if legacy_full != full_name {
                legacy_renames.push((legacy_full, full_name.clone()));
            }
            out.push(ZipImageEntry {
                entry_name: full_name,
                uncompressed_size: entry.size(),
                mtime: zip_mtime,
            });
            continue;
        }
        // RAR/7z/LZH 等の非 ZIP アーカイブはこの経路では読めない (スキップされる)。
        // 検出だけして呼び出し側に伝え、「展開キャッシュへの変換」の提案につなげる (v1.3.0)。
        if crate::archive_converter::ArchiveFormat::from_extension(&ext).is_some() {
            *has_foreign = true;
            continue;
        }
        if crate::folder_tree::is_zip_extension(&ext) {
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
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else {
                continue;
            };
            let new_prefix = format!("{full_name}/");
            let new_legacy_prefix = format!("{legacy_full}/");
            enumerate_recursive(
                &mut inner,
                outer_zip_path,
                &new_prefix,
                &new_legacy_prefix,
                zip_mtime,
                out,
                has_foreign,
                legacy_renames,
            );
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
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::first_image_entry(zip_path, cancel);
    }
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
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        let name = normalized_zip_entry_name(&entry);
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else {
            continue;
        };
        let full_name = format!("{prefix}{name}");
        if is_image_ext(&ext) {
            return Some(full_name);
        }
        if crate::folder_tree::is_zip_extension(&ext) {
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
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else {
                continue;
            };
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
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::read_first_image_bytes(zip_path);
    }
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
                zip_path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
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
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if !entry.is_file() {
            continue;
        }
        let name = normalized_zip_entry_name(&entry);
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else {
            continue;
        };
        let full_name = format!("{prefix}{name}");
        if is_image_ext(&ext) {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            if entry.read_to_end(&mut bytes).is_err() {
                continue;
            }
            return Some((full_name, bytes));
        }
        if crate::folder_tree::is_zip_extension(&ext) {
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
            let Ok(mut inner) = zip::ZipArchive::new(cursor) else {
                continue;
            };
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
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::read_entry_bytes(zip_path, entry_name);
    }
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

    // 変換キャッシュ ZIP (入れ子アーカイブ展開済み、v1.3.0) は "inner.zip/p01.jpg" の
    // ような **literal なフラットエントリ** を持つ (エントリ名自体に ".zip/" 区切りが
    // 含まれる)。ネスト境界として分割解決する前に、まずフルネーム一致を直接試す:
    // - 変換キャッシュ: 常にここで解決される (ネスト展開コストなし)。
    // - 実ネスト ZIP: フルネームのエントリは存在しないので 1 回 miss して従来経路へ。
    //   miss を払うのは NESTED_CACHE が冷えている初回だけ (ヒット後はこの分岐に来ない)。
    // - 病的ケース: 同一 ZIP が実エントリ "book.zip" と literal "book.zip/p.jpg" を
    //   **両方**持つ場合、cold 読みは literal 側を返す (列挙も両方を別エントリとして
    //   挙げており identity は元々曖昧。どちらかを決定的に選ぶ仕様とする、Codex P3)。
    if current_bytes.is_none() {
        if let Ok(bytes) = read_entry_from_disk(zip_path, entry_name) {
            return Ok(bytes);
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
    read_by_name(&mut archive, entry_name, decoded_name_cache_key(zip_path))
}

fn read_entry_from_bytes(bytes: &Arc<Vec<u8>>, entry_name: &str) -> std::io::Result<Vec<u8>> {
    let cursor = Cursor::new(bytes.as_slice());
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    // メモリ上の子 ZIP は安定したキャッシュキーを持たないので index キャッシュなし
    // (走査は raw メタのみで解凍を伴わない)。
    read_by_name(&mut archive, entry_name, None)
}

fn read_by_name<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<DecodedNameCacheKey>,
) -> std::io::Result<Vec<u8>> {
    match read_by_exact_name(archive, entry_name) {
        Ok(bytes) => Ok(bytes),
        Err(first_err) if first_err.kind() == std::io::ErrorKind::NotFound => {
            let legacy_name = entry_name.replace('/', "\\");
            if legacy_name != entry_name
                && let Ok(bytes) = read_by_exact_name(archive, &legacy_name)
            {
                return Ok(bytes);
            }
            read_by_decoded_name(archive, entry_name, cache_key).map_err(|_| first_err)
        }
        Err(err) => Err(err),
    }
}

fn read_by_decoded_name<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<DecodedNameCacheKey>,
) -> std::io::Result<Vec<u8>> {
    let wanted = entry_name.replace('\\', "/");
    if let Some(key) = cache_key {
        let map = match decoded_name_cache_get(&key) {
            Some(map) => map,
            None => {
                let map = Arc::new(build_decoded_name_index_map(archive)?);
                decoded_name_cache_put(key, Arc::clone(&map));
                map
            }
        };
        return match map.get(&wanted) {
            Some(&i) => read_by_index(archive, i),
            None => Err(entry_not_found(entry_name)),
        };
    }
    // キャッシュキーなし (メモリ上の子 ZIP): raw メタの 1 パス走査で index を探す。
    let mut found = None;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        if normalized_zip_entry_name(&entry) == wanted {
            found = Some(i);
            break;
        }
    }
    match found {
        Some(i) => read_by_index(archive, i),
        None => Err(entry_not_found(entry_name)),
    }
}

fn build_decoded_name_index_map<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> std::io::Result<std::collections::HashMap<String, usize>> {
    let mut map = std::collections::HashMap::with_capacity(archive.len());
    for i in 0..archive.len() {
        // by_index_raw は伸長準備をしない (central directory メタ読みのみで安価)。
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        map.insert(normalized_zip_entry_name(&entry), i);
    }
    Ok(map)
}

fn read_by_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
) -> std::io::Result<Vec<u8>> {
    let mut entry = archive
        .by_index(index)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn entry_not_found(entry_name: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("ZIP entry not found: {entry_name}"),
    )
}

fn read_by_exact_name<R: Read + Seek>(
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
pub enum ZipArchiveHandle {
    Zip(zip::ZipArchive<BufReader<File>>),
    Rar(PathBuf),
}

/// ZIP を開いて `ZipArchiveHandle` を返す。
/// ネットワークドライブなど open が高コストな場合、同じハンドルから複数エントリを
/// 順に読めるようにするためのバッチ処理用入り口。
pub fn open_archive(zip_path: &Path) -> std::io::Result<ZipArchiveHandle> {
    if crate::rar_loader::is_rar_path(zip_path) {
        return Ok(ZipArchiveHandle::Rar(zip_path.to_path_buf()));
    }
    let file = File::open(zip_path)?;
    zip::ZipArchive::new(BufReader::new(file))
        .map(ZipArchiveHandle::Zip)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// すでに開いた `ZipArchiveHandle` から 1 エントリの生バイト列を読む。
/// **ネストパスには対応しない** (`.zip/` を含む entry_name は `read_entry_bytes` を使うこと)。
pub fn read_entry_from_archive(
    archive: &mut ZipArchiveHandle,
    entry_name: &str,
) -> std::io::Result<Vec<u8>> {
    match archive {
        ZipArchiveHandle::Zip(archive) => {
            // ZIP バッチハンドルはパスを保持しないため index キャッシュなし。
            read_by_name(archive, entry_name, None)
        }
        ZipArchiveHandle::Rar(path) => crate::rar_loader::read_entry_bytes(path, entry_name),
    }
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

    #[test]
    fn split_nested_cbz_boundary() {
        // CBZ は実体が ZIP。列挙側がネスト .cbz を再帰するので、読み戻し側も .cbz/ で
        // 分割できないと「列挙されるが読めない」不整合になる (Codex P1 回帰防止)。
        let parts = split_nested_zip_path("chapters/ch01.cbz/page01.jpg");
        assert_eq!(parts, vec!["chapters/ch01.cbz", "page01.jpg"]);
    }

    #[test]
    fn split_nested_mixed_zip_and_cbz() {
        assert_eq!(
            split_nested_zip_path("a.cbz/b.zip/img.png"),
            vec!["a.cbz", "b.zip", "img.png"]
        );
        assert_eq!(
            split_nested_zip_path("a.zip/b.cbz/img.png"),
            vec!["a.zip", "b.cbz", "img.png"]
        );
    }

    // ── v1.3.0: 変換キャッシュ (入れ子展開) との整合 ─────────────────

    /// テスト用 ZIP をディスクに作る。entries = (エントリ名, 中身)。
    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            zw.start_file(*name, opts).unwrap();
            use std::io::Write as _;
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                if crc & 1 == 1 {
                    crc = (crc >> 1) ^ 0xedb8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        !crc
    }

    fn push_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    /// UTF-8 flag を立てず、任意の raw filename bytes を持つ STORE ZIP を作る。
    fn write_raw_name_store_zip(path: &Path, raw_name: &[u8], data: &[u8]) {
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_len = raw_name.len() as u16;
        let mut out = Vec::new();

        let local_offset = out.len() as u32;
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0); // general purpose bit flag: no UTF-8 flag
        push_u16(&mut out, 0); // stored
        push_u16(&mut out, 0); // mod time
        push_u16(&mut out, 0); // mod date
        push_u32(&mut out, crc);
        push_u32(&mut out, size);
        push_u32(&mut out, size);
        push_u16(&mut out, name_len);
        push_u16(&mut out, 0); // extra len
        out.extend_from_slice(raw_name);
        out.extend_from_slice(data);

        let central_offset = out.len() as u32;
        push_u32(&mut out, 0x0201_4b50);
        push_u16(&mut out, 20); // version made by
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, crc);
        push_u32(&mut out, size);
        push_u32(&mut out, size);
        push_u16(&mut out, name_len);
        push_u16(&mut out, 0); // extra len
        push_u16(&mut out, 0); // comment len
        push_u16(&mut out, 0); // disk start
        push_u16(&mut out, 0); // internal attrs
        push_u32(&mut out, 0); // external attrs
        push_u32(&mut out, local_offset);
        out.extend_from_slice(raw_name);
        let central_size = out.len() as u32 - central_offset;

        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 1);
        push_u16(&mut out, 1);
        push_u32(&mut out, central_size);
        push_u32(&mut out, central_offset);
        push_u16(&mut out, 0);

        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn cp932_zip_entry_names_are_decoded_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("cp932.zip");
        let display_name = "小さな天使のおしごとは_特典用/小さな天使のおしごとは_特典用_001.jpg";
        let (raw_name, _, had_errors) = encoding_rs::SHIFT_JIS.encode(display_name);
        assert!(!had_errors);
        write_raw_name_store_zip(&zip_path, raw_name.as_ref(), b"IMAGE");

        let entries = enumerate_image_entries(&zip_path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_name, display_name);
        assert_eq!(
            first_image_entry(&zip_path, None).as_deref(),
            Some(display_name)
        );
        let (first_name, first_bytes) = read_first_image_bytes(&zip_path).unwrap();
        assert_eq!(first_name, display_name);
        assert_eq!(first_bytes, b"IMAGE");
        assert_eq!(read_entry_bytes(&zip_path, display_name).unwrap(), b"IMAGE");
        // 2 回目は decoded-name → index キャッシュ経由 (O(1)) でも同じ結果になること。
        assert_eq!(read_entry_bytes(&zip_path, display_name).unwrap(), b"IMAGE");
        // キャッシュ構築後の不在名は NotFound (誤 index を引かない)。
        assert!(read_entry_bytes(&zip_path, "不在/missing.jpg").is_err());

        let mut archive = open_archive(&zip_path).unwrap();
        assert_eq!(
            read_entry_from_archive(&mut archive, display_name).unwrap(),
            b"IMAGE"
        );
    }

    /// 変換キャッシュ ZIP は "inner.zip/p.jpg" のような literal なフラットエントリを
    /// 持つ (入れ子アーカイブ展開の出力)。".zip/" 境界の分割解決より先にフルネーム
    /// 一致で読めること (exact-name fallback)。
    #[test]
    fn read_entry_bytes_resolves_literal_flat_cache_entries() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("cache_like.zip");
        write_test_zip(
            &zip_path,
            &[
                ("inner.zip/p1.jpg", b"ZIPSEG"),
                ("books/inner.rar/p2.jpg", b"RARSEG"),
                ("plain.jpg", b"PLAIN"),
            ],
        );
        // ".zip/" を含む literal エントリ (旧実装ではネスト解決を試みて NotFound だった)。
        assert_eq!(
            read_entry_bytes(&zip_path, "inner.zip/p1.jpg").unwrap(),
            b"ZIPSEG"
        );
        // ".rar/" セグメントは元々分割対象外 → 直接読み (回帰確認)。
        assert_eq!(
            read_entry_bytes(&zip_path, "books/inner.rar/p2.jpg").unwrap(),
            b"RARSEG"
        );
        assert_eq!(read_entry_bytes(&zip_path, "plain.jpg").unwrap(), b"PLAIN");
    }

    /// 実ネスト ZIP (本物の .zip エントリ) は従来どおり境界分割で読める
    /// (exact-name fallback が先に走っても、フルネームのエントリは存在しないので
    /// miss して従来経路に落ちる)。
    #[test]
    fn read_entry_bytes_still_resolves_real_nested_zip() {
        let dir = tempfile::tempdir().unwrap();
        // 内側 ZIP バイト列を作る
        let inner_path = dir.path().join("inner_src.zip");
        write_test_zip(&inner_path, &[("page01.jpg", b"NESTED")]);
        let inner_bytes = std::fs::read(&inner_path).unwrap();
        let zip_path = dir.path().join("outer.zip");
        write_test_zip(&zip_path, &[("ch01.zip", &inner_bytes)]);
        assert_eq!(
            read_entry_bytes(&zip_path, "ch01.zip/page01.jpg").unwrap(),
            b"NESTED"
        );
    }

    /// 非 ZIP アーカイブ (RAR/7z/LZH) のファイルエントリ検出フラグ (v1.3.0)。
    /// ネスト ZIP の中にあっても検出される。
    #[test]
    fn enumerate_detects_foreign_archives() {
        let dir = tempfile::tempdir().unwrap();

        // 直下に rar (中身はゴミで OK、拡張子判定のみ)
        let z1 = dir.path().join("with_rar.zip");
        write_test_zip(&z1, &[("a.jpg", b"A"), ("b.rar", b"junk")]);
        let d1 = enumerate_image_entries_detailed(&z1).unwrap();
        assert!(d1.has_foreign_archives);
        assert_eq!(d1.entries.len(), 1);

        // ネスト ZIP の中に 7z
        let inner_path = dir.path().join("inner_src.zip");
        write_test_zip(&inner_path, &[("c.jpg", b"C"), ("deep.7z", b"junk7z")]);
        let inner_bytes = std::fs::read(&inner_path).unwrap();
        let z2 = dir.path().join("with_nested_7z.zip");
        write_test_zip(&z2, &[("inner.zip", &inner_bytes)]);
        let d2 = enumerate_image_entries_detailed(&z2).unwrap();
        assert!(d2.has_foreign_archives);
        assert_eq!(d2.entries.len(), 1); // inner.zip/c.jpg

        // 純 ZIP (フラグ無し)
        let z3 = dir.path().join("plain.zip");
        write_test_zip(&z3, &[("d.jpg", b"D")]);
        let d3 = enumerate_image_entries_detailed(&z3).unwrap();
        assert!(!d3.has_foreign_archives);
    }
}
