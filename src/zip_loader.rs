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
//! 外側 ZIP は位置指定 reader の `ZipArchive` template を共有し、request ごとの clone で読む。
//! clone は独立した論理位置を持つため、同じ書庫の並列読みでも file position は競合しない。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

// ── 外側 ZIP の中央目次キャッシュ ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveCacheKey {
    path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    len: u64,
}

impl ArchiveCacheKey {
    fn from_path(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            mtime: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
}

fn interrupted_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "ZIP source read was cancelled",
    )
}

/// 取消を `Read` / `Seek` の実装から返すときの error。
///
/// **`ErrorKind::Interrupted` を使ってはならない。** `Read::read` の契約では
/// `Interrupted` は「中断した」ではなく **「やり直してよい」** を意味し、
/// `std::io::default_read_to_end` は `is_interrupted()` を見て `continue` する。
/// 実際にこれで 6 本すべての worker が `read_to_end` の中で永久にリトライし続け、
/// 実行枠を返さないまま CPU を焼いた (2026-09-04 の実機ハング、cdb で全スレッドの
/// スタックを採取して確定)。関数の戻り値としての `interrupted_error` は、
/// リトライループに載らないのでこれまでどおり使ってよい。
fn cancelled_read_error() -> std::io::Error {
    std::io::Error::other("ZIP source read was cancelled")
}

fn zip_error_to_io(error: impl ToString, cancel: Option<&AtomicBool>) -> std::io::Error {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        interrupted_error()
    } else {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
    }
}

/// `ZipArchive::clone` の次の reader clone に付ける request 固有 cancel。
/// template 自身には cancel を保持しない。
#[derive(Debug)]
enum ReaderCloneCancel {
    Disabled,
    Cancellable(Arc<AtomicBool>),
}

type ReaderCloneControl = Arc<Mutex<Option<ReaderCloneCancel>>>;

/// 1 つの `File` を共有しつつ、clone ごとに論理位置を持つ位置指定 reader。
#[derive(Debug)]
pub struct PositionedFileReader {
    file: Arc<File>,
    position: u64,
    cancel: Option<Arc<AtomicBool>>,
    clone_control: ReaderCloneControl,
}

impl PositionedFileReader {
    fn new(file: Arc<File>, cancel: Option<Arc<AtomicBool>>) -> (Self, ReaderCloneControl) {
        let clone_control = Arc::new(Mutex::new(None));
        (
            Self {
                file,
                position: 0,
                cancel,
                clone_control: Arc::clone(&clone_control),
            },
            clone_control,
        )
    }

    /// `Read` / `Seek` から返るので `cancelled_read_error` を使う (retry 契約を踏まない)。
    fn check_cancel(&self) -> std::io::Result<()> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            Err(cancelled_read_error())
        } else {
            Ok(())
        }
    }
}

impl Clone for PositionedFileReader {
    fn clone(&self) -> Self {
        let next_cancel = lock_unpoisoned(&self.clone_control).take();
        let cancel = match next_cancel {
            Some(ReaderCloneCancel::Disabled) => None,
            Some(ReaderCloneCancel::Cancellable(cancel)) => Some(cancel),
            None => self.cancel.clone(),
        };
        Self {
            file: Arc::clone(&self.file),
            position: self.position,
            cancel,
            clone_control: Arc::clone(&self.clone_control),
        }
    }
}

impl Read for PositionedFileReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancel()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        let read =
            std::os::windows::fs::FileExt::seek_read(self.file.as_ref(), buffer, self.position)?;
        #[cfg(not(windows))]
        let read = std::os::unix::fs::FileExt::read_at(self.file.as_ref(), buffer, self.position)?;
        self.position = self.position.checked_add(read as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ZIP reader position overflow",
            )
        })?;
        Ok(read)
    }
}

impl Seek for PositionedFileReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.check_cancel()?;
        let next = match position {
            SeekFrom::Start(position) => position as i128,
            SeekFrom::Current(offset) => self.position as i128 + offset as i128,
            SeekFrom::End(offset) => self.file.metadata()?.len() as i128 + offset as i128,
        };
        if !(0..=u64::MAX as i128).contains(&next) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid ZIP reader seek",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

/// メモリ上の入れ子 ZIP にも同じ I/O 境界で cancel を適用する。
struct CancellableReader<R> {
    inner: R,
    cancel: Option<Arc<AtomicBool>>,
}

impl<R> CancellableReader<R> {
    fn new(inner: R, cancel: Option<Arc<AtomicBool>>) -> Self {
        Self { inner, cancel }
    }

    /// `Read` / `Seek` から返るので `cancelled_read_error` を使う (retry 契約を踏まない)。
    fn check_cancel(&self) -> std::io::Result<()> {
        if is_cancelled(self.cancel.as_ref()) {
            Err(cancelled_read_error())
        } else {
            Ok(())
        }
    }
}

impl<R: Read> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancel()?;
        self.inner.read(buffer)
    }
}

impl<R: Seek> Seek for CancellableReader<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.check_cancel()?;
        self.inner.seek(position)
    }
}

type DiskZipArchive = zip::ZipArchive<PositionedFileReader>;

struct CachedArchiveTemplate {
    archive: DiskZipArchive,
    clone_control: ReaderCloneControl,
}

impl CachedArchiveTemplate {
    fn clone_for_request(
        &self,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<DiskZipArchive> {
        clone_archive_with_cancel(&self.archive, &self.clone_control, cancel.cloned())
    }
}

#[derive(Clone)]
struct CachedIoError {
    kind: std::io::ErrorKind,
    message: String,
    cancelled: bool,
}

impl CachedIoError {
    fn from_error(error: &std::io::Error, cancelled: bool) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
            cancelled,
        }
    }

    fn to_error(&self) -> std::io::Error {
        std::io::Error::new(self.kind, self.message.clone())
    }
}

enum ArchiveTemplateState {
    Loading,
    Ready(CachedArchiveTemplate),
    Failed(CachedIoError),
}

struct ArchiveTemplateSlot {
    state: Mutex<ArchiveTemplateState>,
    ready: Condvar,
}

struct ArchiveDirectoryCacheEntry {
    key: ArchiveCacheKey,
    slot: Arc<ArchiveTemplateSlot>,
    last_used: Instant,
}

struct ArchiveDirectoryCacheInner {
    entries: Vec<ArchiveDirectoryCacheEntry>,
}

/// 外側 ZIP の解析済み中央目次を、書庫数上限つきで所有する。
///
/// global lock は metadata key の照合と slot 取得だけに使う。初回解析は lock 外、同じ
/// key の二重解析は per-key `Condvar` で single-flight にする。ready 後は template の
/// clone の一瞬だけ slot lock を握り、エントリ I/O 中はどの cache lock も保持しない。
struct ArchiveDirectoryCache {
    inner: Mutex<ArchiveDirectoryCacheInner>,
    max_archives: usize,
    parse_count: AtomicUsize,
}

const ARCHIVE_DIRECTORY_CACHE_MAX_ARCHIVES: usize = 8;
const ARCHIVE_DIRECTORY_WAIT_POLL: Duration = Duration::from_millis(5);

impl ArchiveDirectoryCache {
    fn new(max_archives: usize) -> Self {
        Self {
            inner: Mutex::new(ArchiveDirectoryCacheInner {
                entries: Vec::new(),
            }),
            max_archives: max_archives.max(1),
            parse_count: AtomicUsize::new(0),
        }
    }

    fn open_archive(
        &self,
        path: &Path,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<DiskZipArchive> {
        'lookup: loop {
            if is_cancelled(cancel) {
                return Err(interrupted_error());
            }
            // 取得のたびに metadata を取り、同じ path の古い identity を失効させる。
            let key = ArchiveCacheKey::from_path(path)?;
            let (slot, is_builder) = self.slot_for_key(key.clone());
            if is_builder {
                let built = self.build_archive(path, cancel);
                match built {
                    Ok((request_archive, template)) => {
                        *lock_unpoisoned(&slot.state) = ArchiveTemplateState::Ready(template);
                        slot.ready.notify_all();
                        return Ok(request_archive);
                    }
                    Err(error) => {
                        let failure = CachedIoError::from_error(&error, is_cancelled(cancel));
                        *lock_unpoisoned(&slot.state) = ArchiveTemplateState::Failed(failure);
                        slot.ready.notify_all();
                        self.remove_slot(&key, &slot);
                        return Err(error);
                    }
                }
            }

            let mut state = lock_unpoisoned(&slot.state);
            loop {
                match &*state {
                    ArchiveTemplateState::Ready(template) => {
                        return template.clone_for_request(cancel);
                    }
                    ArchiveTemplateState::Loading => {
                        if is_cancelled(cancel) {
                            return Err(interrupted_error());
                        }
                        state = match slot.ready.wait_timeout(state, ARCHIVE_DIRECTORY_WAIT_POLL) {
                            Ok((state, _)) => state,
                            Err(poisoned) => poisoned.into_inner().0,
                        };
                    }
                    ArchiveTemplateState::Failed(failure) => {
                        let retry = failure.cancelled && !is_cancelled(cancel);
                        let error = failure.to_error();
                        drop(state);
                        self.remove_slot(&key, &slot);
                        if retry {
                            continue 'lookup;
                        }
                        return Err(error);
                    }
                }
            }
        }
    }

    fn slot_for_key(&self, key: ArchiveCacheKey) -> (Arc<ArchiveTemplateSlot>, bool) {
        let mut inner = lock_unpoisoned(&self.inner);
        inner
            .entries
            .retain(|entry| entry.key.path != key.path || entry.key == key);
        if let Some(entry) = inner.entries.iter_mut().find(|entry| entry.key == key) {
            entry.last_used = Instant::now();
            return (Arc::clone(&entry.slot), false);
        }
        if inner.entries.len() >= self.max_archives {
            let oldest = inner
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(index, _)| index)
                .unwrap_or(0);
            inner.entries.swap_remove(oldest);
        }
        let slot = Arc::new(ArchiveTemplateSlot {
            state: Mutex::new(ArchiveTemplateState::Loading),
            ready: Condvar::new(),
        });
        inner.entries.push(ArchiveDirectoryCacheEntry {
            key,
            slot: Arc::clone(&slot),
            last_used: Instant::now(),
        });
        (slot, true)
    }

    fn build_archive(
        &self,
        path: &Path,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<(DiskZipArchive, CachedArchiveTemplate)> {
        if is_cancelled(cancel) {
            return Err(interrupted_error());
        }
        let file = Arc::new(File::open(path)?);
        let (reader, clone_control) = PositionedFileReader::new(file, cancel.cloned());
        self.parse_count.fetch_add(1, Ordering::Relaxed);
        let archive = zip::ZipArchive::new(reader)
            .map_err(|error| zip_error_to_io(error, cancel.map(Arc::as_ref)))?;
        // 保存する clone だけ cancel を外す。現在要求は元 archive を使い続ける。
        let template_archive = clone_archive_with_cancel(&archive, &clone_control, None)?;
        let template = CachedArchiveTemplate {
            archive: template_archive,
            clone_control,
        };
        Ok((archive, template))
    }

    fn remove_slot(&self, key: &ArchiveCacheKey, slot: &Arc<ArchiveTemplateSlot>) {
        lock_unpoisoned(&self.inner)
            .entries
            .retain(|entry| entry.key != *key || !Arc::ptr_eq(&entry.slot, slot));
    }

    fn clear(&self) {
        lock_unpoisoned(&self.inner).entries.clear();
    }

    #[cfg(test)]
    fn parse_count(&self) -> usize {
        self.parse_count.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        lock_unpoisoned(&self.inner).entries.len()
    }
}

fn clone_archive_with_cancel(
    archive: &DiskZipArchive,
    clone_control: &ReaderCloneControl,
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<DiskZipArchive> {
    let mut control = lock_unpoisoned(clone_control);
    if control.is_some() {
        return Err(std::io::Error::other(
            "ZIP archive clone control was already in use",
        ));
    }
    *control = Some(match cancel {
        Some(cancel) => ReaderCloneCancel::Cancellable(cancel),
        None => ReaderCloneCancel::Disabled,
    });
    drop(control);
    Ok(archive.clone())
}

static ARCHIVE_DIRECTORY_CACHE: LazyLock<ArchiveDirectoryCache> =
    LazyLock::new(|| ArchiveDirectoryCache::new(ARCHIVE_DIRECTORY_CACHE_MAX_ARCHIVES));

#[cfg(any(test, feature = "dev-tools"))]
pub fn archive_directory_parse_count() -> usize {
    ARCHIVE_DIRECTORY_CACHE.parse_count.load(Ordering::Relaxed)
}

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
const DECODED_NAME_CACHE_MAX_ARCHIVES: usize = 8;

static DECODED_NAME_INDEX_CACHE: LazyLock<
    Mutex<
        Vec<(
            ArchiveCacheKey,
            Arc<std::collections::HashMap<String, usize>>,
        )>,
    >,
> = LazyLock::new(|| Mutex::new(Vec::new()));

fn decoded_name_cache_key(zip_path: &Path) -> Option<ArchiveCacheKey> {
    ArchiveCacheKey::from_path(zip_path).ok()
}

fn decoded_name_cache_get(
    key: &ArchiveCacheKey,
) -> Option<Arc<std::collections::HashMap<String, usize>>> {
    let cache = DECODED_NAME_INDEX_CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, map)| Arc::clone(map))
}

fn decoded_name_cache_put(
    key: ArchiveCacheKey,
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
/// 内側 ZIP bytes と外側 ZIP の解析済み目次を破棄し、古い書庫を居残らせない。
/// 関数名は既存呼び出しとの互換のため維持する。
pub fn clear_nested_cache() {
    NESTED_CACHE.clear();
    ARCHIVE_DIRECTORY_CACHE.clear();
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
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, None)?;

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
    let file_size = std::fs::metadata(zip_path)
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);
    let t0 = std::time::Instant::now();
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, None).ok()?;
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
    read_entry_bytes_impl(zip_path, entry_name, None)
}

/// [`read_entry_bytes`] の cancel 対応版。外側・入れ子 ZIP の reader I/O 境界で停止する。
pub fn read_entry_bytes_cancellable(
    zip_path: &Path,
    entry_name: &str,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    read_entry_bytes_impl(zip_path, entry_name, Some(cancel))
}

fn read_entry_bytes_impl(
    zip_path: &Path,
    entry_name: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> std::io::Result<Vec<u8>> {
    if is_cancelled(cancel) {
        return Err(interrupted_error());
    }
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::read_entry_bytes(zip_path, entry_name);
    }
    let parts = split_nested_zip_path(entry_name);
    if parts.len() == 1 {
        return read_entry_from_disk(zip_path, entry_name, cancel);
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
        if let Ok(bytes) = read_entry_from_disk(zip_path, entry_name, cancel) {
            return Ok(bytes);
        }
        if is_cancelled(cancel) {
            return Err(interrupted_error());
        }
    }

    // start_level から葉 (parts.len() - 1) までを順に展開しながら読む。
    // キャッシュヒットしなかった場合、start_level = 0 で外側 ZIP から開始する。
    let mut level = start_level;
    while level < parts.len() - 1 {
        // parts[level] は内側 ZIP のエントリ。中身は別の ZIP バイト列。
        let next_bytes: Vec<u8> = match &current_bytes {
            Some(b) => read_entry_from_bytes(b, parts[level], cancel)?,
            None => read_entry_from_disk(zip_path, parts[level], cancel)?,
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
        Some(b) => read_entry_from_bytes(b, leaf, cancel),
        None => read_entry_from_disk(zip_path, leaf, cancel),
    }
}

fn read_entry_from_disk(
    zip_path: &Path,
    entry_name: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> std::io::Result<Vec<u8>> {
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, cancel)?;
    read_by_name(
        &mut archive,
        entry_name,
        decoded_name_cache_key(zip_path),
        cancel.map(Arc::as_ref),
    )
}

fn read_entry_from_bytes(
    bytes: &Arc<Vec<u8>>,
    entry_name: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> std::io::Result<Vec<u8>> {
    let reader = CancellableReader::new(Cursor::new(bytes.as_slice()), cancel.cloned());
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| zip_error_to_io(error, cancel.map(Arc::as_ref)))?;
    // メモリ上の子 ZIP は安定したキャッシュキーを持たないので index キャッシュなし
    // (走査は raw メタのみで解凍を伴わない)。
    read_by_name(&mut archive, entry_name, None, cancel.map(Arc::as_ref))
}

/// エントリ名から index を解く。**名前解決はここだけが持つ。**
///
/// 正確名 → 旧形式の `\` 区切り → 生メタから復号した名前、の順。日本語の書庫は
/// Shift-JIS の生名を持つことがあり、復号した名前では `index_for_name` が当たらない。
/// 部分読みを別経路で書いたときにこの段を丸ごと落とし、**その書庫では寸法が 1 件も
/// 取れなかった** (2026-08-26)。読み方が増えても、解決の順番は複製しないこと。
fn resolve_entry_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<ArchiveCacheKey>,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<usize> {
    if let Some(index) = archive.index_for_name(entry_name) {
        return Ok(index);
    }
    let legacy_name = entry_name.replace('/', "\\");
    if legacy_name != entry_name
        && let Some(index) = archive.index_for_name(&legacy_name)
    {
        return Ok(index);
    }
    decoded_name_index(archive, entry_name, cache_key, cancel)
}

fn read_by_name<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<ArchiveCacheKey>,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let index = resolve_entry_index(archive, entry_name, cache_key, cancel)?;
    read_by_index(archive, index, cancel)
}

/// エントリの先頭 `limit` バイトだけを読む。画像ヘッダから寸法を取るための限定 API。
fn read_prefix_by_name<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<ArchiveCacheKey>,
    limit: u64,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let index = resolve_entry_index(archive, entry_name, cache_key, cancel)?;
    let mut entry = archive
        .by_index(index)
        .map_err(|error| zip_error_to_io(error, cancel))?;
    let mut bytes = Vec::with_capacity(limit.min(entry.size()) as usize);
    entry.by_ref().take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn decoded_name_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    entry_name: &str,
    cache_key: Option<ArchiveCacheKey>,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<usize> {
    let wanted = entry_name.replace('\\', "/");
    if let Some(key) = cache_key {
        let map = match decoded_name_cache_get(&key) {
            Some(map) => map,
            None => {
                let map = Arc::new(build_decoded_name_index_map(archive, cancel)?);
                decoded_name_cache_put(key, Arc::clone(&map));
                map
            }
        };
        return map
            .get(&wanted)
            .copied()
            .ok_or_else(|| entry_not_found(entry_name));
    }
    // キャッシュキーなし (メモリ上の子 ZIP): raw メタの 1 パス走査で index を探す。
    for i in 0..archive.len() {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(interrupted_error());
        }
        let entry = archive
            .by_index_raw(i)
            .map_err(|error| zip_error_to_io(error, cancel))?;
        if normalized_zip_entry_name(&entry) == wanted {
            return Ok(i);
        }
    }
    Err(entry_not_found(entry_name))
}

fn build_decoded_name_index_map<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<std::collections::HashMap<String, usize>> {
    let mut map = std::collections::HashMap::with_capacity(archive.len());
    for i in 0..archive.len() {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(interrupted_error());
        }
        // by_index_raw は伸長準備をしない (central directory メタ読みのみで安価)。
        let entry = archive
            .by_index_raw(i)
            .map_err(|error| zip_error_to_io(error, cancel))?;
        map.insert(normalized_zip_entry_name(&entry), i);
    }
    Ok(map)
}

fn read_by_index<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut entry = archive
        .by_index(index)
        .map_err(|error| zip_error_to_io(error, cancel))?;
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

/// 複数エントリをまとめて読むときのアーカイブハンドル型。
/// `zip` クレートの型を隠蔽するため `zip_loader` 外から名前で参照できるようにする。
///
/// このハンドルは**外側 ZIP のみ**を保持する。ネストパスを読む場合は
/// `read_entry_bytes` を使い、関数側でネスト境界を解釈させること。
pub enum ZipArchiveHandle {
    Zip(DiskZipArchive),
    Rar(PathBuf),
}

/// ZIP を開いて `ZipArchiveHandle` を返す。
/// ネットワークドライブなど open が高コストな場合、同じハンドルから複数エントリを
/// 順に読めるようにするためのバッチ処理用入り口。
pub fn open_archive(zip_path: &Path) -> std::io::Result<ZipArchiveHandle> {
    if crate::rar_loader::is_rar_path(zip_path) {
        return Ok(ZipArchiveHandle::Rar(zip_path.to_path_buf()));
    }
    ARCHIVE_DIRECTORY_CACHE
        .open_archive(zip_path, None)
        .map(ZipArchiveHandle::Zip)
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
            read_by_name(archive, entry_name, None, None)
        }
        ZipArchiveHandle::Rar(path) => crate::rar_loader::read_entry_bytes(path, entry_name),
    }
}

/// すでに開いた `ZipArchiveHandle` から 1 エントリの**先頭だけ**を読む。
///
/// 画像ヘッダから寸法を取るための限定 API。全体を展開すると、寸法を知るためだけに
/// 1 冊分の画像を伸長することになる (見開きの単独表示と横長分割は、ページが横長かを
/// 知る必要がある)。
///
/// **ネストパスには対応しない** (`.zip/` を含む entry_name は `read_entry_bytes` を使う)。
/// 解決できなければ `NotFound` を返すので、呼び出し側が従来経路へ落とせる。
pub fn read_entry_prefix_from_archive(
    archive: &mut ZipArchiveHandle,
    entry_name: &str,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    match archive {
        // 名前解決は `read_entry_from_archive` と同じ段を通す。
        ZipArchiveHandle::Zip(archive) => {
            read_prefix_by_name(archive, entry_name, None, limit, None)
        }
        // RAR は部分読みの経路を持たない。全体を読んで先頭だけ使う。
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

    /// **先頭だけ読む経路も、全体を読む経路と同じ名前解決を通る。**
    ///
    /// 日本語の書庫は Shift-JIS の生名を持つことがあり、列挙側が返すのは復号後の名前。
    /// 部分読みを別経路で書いて `by_name` だけに頼ったとき、この種の書庫では 1 件も
    /// 解決できず、寸法が全く取れなかった (2026-08-26)。
    #[test]
    fn a_prefix_read_finds_an_entry_whose_stored_name_is_not_utf8() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("cp932.zip");
        // "あ.txt" を CP932 で格納する。UTF-8 フラグは立てない。
        let raw_name = vec![0x82u8, 0xA0, b'.', b't', b'x', b't'];
        let stored_name = unsafe { String::from_utf8_unchecked(raw_name.clone()) };
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file(stored_name, options).unwrap();
            writer.write_all(b"0123456789abcdef").unwrap();
            writer.finish().unwrap();
        }

        // 列挙側が返す名前 (= 復号後) で引く。生名とは別物。
        let mut archive = open_archive(&zip_path).unwrap();
        let listed = match &mut archive {
            ZipArchiveHandle::Zip(archive) => {
                normalized_zip_entry_name(&archive.by_index(0).unwrap())
            }
            ZipArchiveHandle::Rar(_) => unreachable!(),
        };

        let whole = read_entry_from_archive(&mut archive, &listed).unwrap();
        assert_eq!(whole, b"0123456789abcdef");

        let head = read_entry_prefix_from_archive(&mut archive, &listed, 4).unwrap();
        assert_eq!(head, b"0123");
    }

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

    fn read_with_directory_cache(
        cache: &ArchiveDirectoryCache,
        path: &Path,
        entry_name: &str,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<Vec<u8>> {
        let mut archive = cache.open_archive(path, cancel)?;
        read_by_name(&mut archive, entry_name, None, cancel.map(Arc::as_ref))
    }

    /// A cancelled read must not report `Interrupted`.
    ///
    /// `Read::read` defines `Interrupted` as "nothing was read, try again", so returning it
    /// for cancellation tells every std adapter to retry forever. This test used to assert
    /// the opposite and so pinned the defect in place: on 2026-09-04 all six permits ended
    /// up spinning inside `read_to_end`, never returning and never releasing their slot.
    #[test]
    fn cancelled_reader_error_is_not_the_retry_signal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader.bin");
        std::fs::write(&path, b"abcdef").unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let (mut reader, _) =
            PositionedFileReader::new(Arc::new(File::open(path).unwrap()), Some(cancel));

        let mut byte = [0_u8; 1];
        let read_error = reader.read(&mut byte).unwrap_err();
        assert_ne!(read_error.kind(), std::io::ErrorKind::Interrupted);
        let seek_error = reader.seek(SeekFrom::Start(0)).unwrap_err();
        assert_ne!(seek_error.kind(), std::io::ErrorKind::Interrupted);
    }

    /// Drive the real consumer, not just `read` on its own.
    ///
    /// The reader is only ever used through the zip crate, which reads entries with
    /// `read_to_end`. Testing `read` directly cannot see a retry loop, which is why the
    /// hang reached a real machine. A regression fails this by timing out rather than
    /// hanging the suite.
    #[test]
    fn a_cancelled_read_to_end_returns_instead_of_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader.bin");
        std::fs::write(&path, vec![7_u8; 64 * 1024]).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let (mut reader, _) =
            PositionedFileReader::new(Arc::new(File::open(path).unwrap()), Some(cancel));

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = tx.send(reader.read_to_end(&mut sink).is_err());
        });
        let errored = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a cancelled read_to_end must return rather than retry forever");
        assert!(errored, "the cancelled read must surface as an error");
    }

    /// Cancelling after the archive is open stops the read and leaves the archive intact.
    ///
    /// This covers the classification, not the retry loop: the cancelled seek inside
    /// `by_index` fails before `read_to_end` is reached, so this test passes either way.
    /// The retry loop is covered by `a_cancelled_read_to_end_returns_instead_of_retrying`
    /// and `cancelled_reader_error_is_not_the_retry_signal`, both of which fail if the
    /// cancellation error goes back to `Interrupted`.
    #[test]
    fn cancelling_during_an_entry_read_stops_and_is_not_reported_as_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("midread.zip");
        write_test_zip(&path, &[("page.jpg", &vec![9_u8; 4 * 1024 * 1024])]);
        let cache = ArchiveDirectoryCache::new(2);
        // Warm the directory so the cancellation lands in the entry read, not the parse.
        read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let mut archive = cache.open_archive(&path, Some(&cancel)).unwrap();
        cancel.store(true, Ordering::Relaxed);

        let (tx, rx) = std::sync::mpsc::channel();
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let result = read_by_name(&mut archive, "page.jpg", None, Some(flag.as_ref()));
            let _ = tx.send(result.is_err());
        });
        let errored = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a mid-read cancellation must return rather than retry forever");
        assert!(errored);

        // The archive is intact: with no cancellation the same entry still reads.
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None)
                .unwrap()
                .len(),
            4 * 1024 * 1024
        );
    }

    #[test]
    fn directory_cache_reuses_one_central_directory_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reuse.zip");
        write_test_zip(&path, &[("page.jpg", b"PAGE")]);
        let cache = ArchiveDirectoryCache::new(2);

        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"PAGE"
        );
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"PAGE"
        );
        assert_eq!(cache.parse_count(), 1);
    }

    #[test]
    fn directory_cache_is_bounded_by_archive_count() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ArchiveDirectoryCache::new(2);
        for index in 0..3 {
            let path = dir.path().join(format!("book-{index}.zip"));
            write_test_zip(&path, &[("page.jpg", b"PAGE")]);
            assert_eq!(
                read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
                b"PAGE"
            );
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.parse_count(), 3);
    }

    #[test]
    fn directory_cache_invalidates_when_size_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("size.zip");
        let cache = ArchiveDirectoryCache::new(2);
        write_test_zip(&path, &[("page.jpg", b"OLD")]);
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"OLD"
        );

        write_test_zip(&path, &[("page.jpg", b"NEW-LONGER")]);
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"NEW-LONGER"
        );
        assert_eq!(cache.parse_count(), 2);
    }

    #[test]
    fn directory_cache_invalidates_when_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mtime.zip");
        let cache = ArchiveDirectoryCache::new(2);
        write_test_zip(&path, &[("page.jpg", b"OLD")]);
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"OLD"
        );
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let mut changed = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            write_test_zip(&path, &[("page.jpg", b"NEW")]);
            if std::fs::metadata(&path).unwrap().modified().unwrap() != original_mtime {
                changed = true;
                break;
            }
        }
        assert!(changed, "test filesystem did not advance mtime");
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"NEW"
        );
        assert_eq!(cache.parse_count(), 2);
    }

    #[test]
    fn request_cancel_does_not_poison_the_cached_template() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.zip");
        write_test_zip(&path, &[("page.jpg", b"PAGE")]);
        let cache = ArchiveDirectoryCache::new(2);
        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"PAGE"
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let mut cancelled_archive = cache.open_archive(&path, Some(&cancel)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let error = read_by_name(
            &mut cancelled_archive,
            "page.jpg",
            None,
            Some(cancel.as_ref()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);

        assert_eq!(
            read_with_directory_cache(&cache, &path, "page.jpg", None).unwrap(),
            b"PAGE"
        );
        assert_eq!(cache.parse_count(), 1);
    }

    #[test]
    fn parallel_reads_share_the_directory_and_keep_positions_independent() {
        const WORKERS: usize = 8;
        const ENTRIES: [(&str, &[u8]); WORKERS] = [
            ("p0.bin", b"zero"),
            ("p1.bin", b"one-one"),
            ("p2.bin", b"two-two-two"),
            ("p3.bin", b"three"),
            ("p4.bin", b"four-four"),
            ("p5.bin", b"five-five-five"),
            ("p6.bin", b"six"),
            ("p7.bin", b"seven-seven"),
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parallel.zip");
        write_test_zip(&path, &ENTRIES);
        let cache = Arc::new(ArchiveDirectoryCache::new(2));
        let barrier = Arc::new(std::sync::Barrier::new(WORKERS));

        let workers: Vec<_> = ENTRIES
            .iter()
            .map(|(name, expected)| {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                let path = path.clone();
                let name = (*name).to_owned();
                let expected = expected.to_vec();
                std::thread::spawn(move || {
                    barrier.wait();
                    let actual = read_with_directory_cache(&cache, &path, &name, None).unwrap();
                    assert_eq!(actual, expected);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(cache.parse_count(), 1);
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

    #[test]
    fn read_entry_bytes_keeps_the_direct_rar_dispatch() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression/○×△□ Vol.2.rar");
        let entries = crate::rar_loader::enumerate_image_entries_detailed(&path).unwrap();
        let entry_name = &entries.entries[0].entry_name;
        let expected = crate::rar_loader::read_entry_bytes(&path, entry_name).unwrap();
        let actual = read_entry_bytes(&path, entry_name).unwrap();
        assert_eq!(actual, expected);
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

    #[test]
    fn enumerate_detects_foreign_archives_deep_inside_nested_zip_tree() {
        let dir = tempfile::tempdir().unwrap();

        // outer.zip
        //   shelf/vol01.zip
        //     pages/p01.jpg
        //     extras/raw.rar      (中身はゴミでよい。検出は拡張子ベース)
        //     extras/deep.7z
        //   shelf/vol02.zip
        //     pages/p02.jpg
        let vol01_path = dir.path().join("vol01_src.zip");
        write_test_zip(
            &vol01_path,
            &[
                ("pages/p01.jpg", b"P1"),
                ("extras/raw.rar", b"not a rar"),
                ("extras/deep.7z", b"not a 7z"),
            ],
        );
        let vol02_path = dir.path().join("vol02_src.zip");
        write_test_zip(&vol02_path, &[("pages/p02.jpg", b"P2")]);
        let vol01_bytes = std::fs::read(&vol01_path).unwrap();
        let vol02_bytes = std::fs::read(&vol02_path).unwrap();
        let outer = dir.path().join("mixed_tree.zip");
        write_test_zip(
            &outer,
            &[
                ("shelf/vol01.zip", &vol01_bytes),
                ("shelf/vol02.zip", &vol02_bytes),
                ("cover.jpg", b"COVER"),
            ],
        );

        let d = enumerate_image_entries_detailed(&outer).unwrap();
        assert!(
            d.has_foreign_archives,
            "ネスト ZIP のさらに下にある RAR/7z でも変換提案フラグを立てる"
        );
        let names: Vec<_> = d.entries.iter().map(|e| e.entry_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "shelf/vol01.zip/pages/p01.jpg",
                "shelf/vol02.zip/pages/p02.jpg",
                "cover.jpg",
            ]
        );
    }
}
