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
//! 外側 ZIP は位置指定 reader の `ZipArchive` template をプロセスで共有し、request ごとの clone
//! で読む。clone は独立した論理位置を持つため、同じ書庫の並列読みでも file position は競合しない。
//! template は `(path, mtime, len)` で検証する 8 書庫 LRU であり、別 context の移動では消さない。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
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

/// zip 2.4.2 constructs each central-directory record through `central_header_to_zip_file`, whose
/// `find_data_start` reads the matching local header and then restores the central-directory
/// position. Two tiny windows combine adjacent reads on both sides of that alternating access
/// without changing which headers or entries zip validates.
// 2,048-entry disposable ZIP comparison (2026-09-11): two 256-byte windows removed 58% of OS
// read calls while limiting read amplification to 3.38x; the second window removed 37.5% of the
// calls and bytes remaining with one window. Larger windows saved only another 8 percentage
// points of calls but raised amplification to 6.1x/11.5x/43.8x, a poor trade on UNC.
const POSITIONED_READ_WINDOW_BYTES: usize = 256;
const POSITIONED_READ_WINDOW_COUNT: usize = 2;

#[derive(Debug, Default)]
struct PositionedReadWindow {
    start: u64,
    bytes: Vec<u8>,
    last_used: u64,
}

#[derive(Debug)]
struct PositionedReadCache {
    window_bytes: usize,
    window_count: usize,
    windows: [PositionedReadWindow; POSITIONED_READ_WINDOW_COUNT],
    use_serial: u64,
    #[cfg(test)]
    buffer_allocations: u64,
}

impl PositionedReadCache {
    fn new(window_bytes: usize, window_count: usize) -> Self {
        Self {
            window_bytes,
            window_count: if window_bytes == 0 {
                0
            } else {
                window_count.clamp(1, POSITIONED_READ_WINDOW_COUNT)
            },
            windows: std::array::from_fn(|_| PositionedReadWindow::default()),
            use_serial: 0,
            #[cfg(test)]
            buffer_allocations: 0,
        }
    }

    fn copy_at(&mut self, position: u64, target: &mut [u8]) -> Option<usize> {
        let index = self.windows[..self.window_count]
            .iter()
            .position(|window| {
                position >= window.start
                    && !window.bytes.is_empty()
                    && position.saturating_sub(window.start) < window.bytes.len() as u64
            })?;
        let window = &mut self.windows[index];
        let offset = usize::try_from(position.saturating_sub(window.start)).ok()?;
        let count = target.len().min(window.bytes.len().saturating_sub(offset));
        target[..count].copy_from_slice(&window.bytes[offset..offset + count]);
        self.use_serial = self.use_serial.wrapping_add(1).max(1);
        window.last_used = self.use_serial;
        Some(count)
    }

    fn take_fill_buffer(&mut self) -> (usize, Vec<u8>) {
        let index = self.windows[..self.window_count]
            .iter()
            .position(|window| window.bytes.is_empty())
            .or_else(|| {
                self.windows[..self.window_count]
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, window)| window.last_used)
                    .map(|(index, _)| index)
            })
            .expect("a non-zero read window count has a fill slot");
        let window = &mut self.windows[index];
        window.start = 0;
        window.last_used = 0;
        let mut bytes = std::mem::take(&mut window.bytes);
        bytes.clear();
        if bytes.capacity() < self.window_bytes {
            #[cfg(test)]
            {
                self.buffer_allocations = self.buffer_allocations.saturating_add(1);
            }
            bytes.reserve_exact(self.window_bytes);
        }
        bytes.resize(self.window_bytes, 0);
        (index, bytes)
    }

    fn recycle_failed_fill(&mut self, index: usize, mut bytes: Vec<u8>) {
        bytes.clear();
        self.windows[index] = PositionedReadWindow {
            bytes,
            ..PositionedReadWindow::default()
        };
    }

    fn finish_fill(&mut self, index: usize, start: u64, bytes: Vec<u8>) {
        self.use_serial = self.use_serial.wrapping_add(1).max(1);
        self.windows[index] = PositionedReadWindow {
            start,
            bytes,
            last_used: self.use_serial,
        };
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct PositionedIoProbe {
    requested_bytes: AtomicU64,
    os_read_calls: AtomicU64,
    os_read_bytes: AtomicU64,
    cache_hits: AtomicU64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct PositionedIoSnapshot {
    requested_bytes: u64,
    os_read_calls: u64,
    os_read_bytes: u64,
    cache_hits: u64,
}

#[cfg(test)]
impl PositionedIoProbe {
    fn snapshot(&self) -> PositionedIoSnapshot {
        PositionedIoSnapshot {
            requested_bytes: self.requested_bytes.load(Ordering::Relaxed),
            os_read_calls: self.os_read_calls.load(Ordering::Relaxed),
            os_read_bytes: self.os_read_bytes.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
        }
    }
}

/// 1 つの `File` を共有しつつ、clone ごとに論理位置を持つ位置指定 reader。
#[derive(Debug)]
pub struct PositionedFileReader {
    file: Arc<File>,
    position: u64,
    cancel: Option<Arc<AtomicBool>>,
    clone_control: ReaderCloneControl,
    read_cache: PositionedReadCache,
    #[cfg(test)]
    io_probe: Option<Arc<PositionedIoProbe>>,
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
                read_cache: PositionedReadCache::new(
                    POSITIONED_READ_WINDOW_BYTES,
                    POSITIONED_READ_WINDOW_COUNT,
                ),
                #[cfg(test)]
                io_probe: None,
            },
            clone_control,
        )
    }

    #[cfg(test)]
    fn new_with_read_window(
        file: Arc<File>,
        cancel: Option<Arc<AtomicBool>>,
        window_bytes: usize,
        window_count: usize,
        io_probe: Arc<PositionedIoProbe>,
    ) -> (Self, ReaderCloneControl) {
        let (mut reader, clone_control) = Self::new(file, cancel);
        reader.read_cache = PositionedReadCache::new(window_bytes, window_count);
        reader.io_probe = Some(io_probe);
        (reader, clone_control)
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

    fn advance_position(&mut self, read: usize) -> std::io::Result<()> {
        self.position = self.position.checked_add(read as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ZIP reader position overflow",
            )
        })?;
        Ok(())
    }

    fn read_from_file(&self, buffer: &mut [u8], position: u64) -> std::io::Result<usize> {
        #[cfg(test)]
        if let Some(probe) = &self.io_probe {
            probe.os_read_calls.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(self.file.as_ref(), buffer, position)?;
        #[cfg(not(windows))]
        let read = std::os::unix::fs::FileExt::read_at(self.file.as_ref(), buffer, position)?;
        #[cfg(test)]
        if let Some(probe) = &self.io_probe {
            probe
                .os_read_bytes
                .fetch_add(read as u64, Ordering::Relaxed);
        }
        Ok(read)
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
            // A clone is a new request owner. It shares the immutable zip directory through
            // `ZipArchive`, but never shares mutable read-ahead state or cached bytes.
            read_cache: PositionedReadCache::new(
                self.read_cache.window_bytes,
                self.read_cache.window_count,
            ),
            #[cfg(test)]
            io_probe: self.io_probe.clone(),
        }
    }
}

impl Read for PositionedFileReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancel()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        #[cfg(test)]
        if let Some(probe) = &self.io_probe {
            probe
                .requested_bytes
                .fetch_add(buffer.len() as u64, Ordering::Relaxed);
        }
        if let Some(read) = self.read_cache.copy_at(self.position, buffer) {
            // Cancellation was checked before consulting cached bytes. A cancelled request can
            // therefore never observe data merely because another read populated a window.
            #[cfg(test)]
            if let Some(probe) = &self.io_probe {
                probe.cache_hits.fetch_add(1, Ordering::Relaxed);
            }
            self.advance_position(read)?;
            return Ok(read);
        }
        let window_bytes = self.read_cache.window_bytes;
        if window_bytes == 0 || buffer.len() >= window_bytes {
            let read = self.read_from_file(buffer, self.position)?;
            self.advance_position(read)?;
            return Ok(read);
        }

        let start = self.position;
        let (slot, mut window) = self.read_cache.take_fill_buffer();
        let read = match self.read_from_file(&mut window, start) {
            Ok(read) => read,
            Err(error) => {
                // Keep the reusable allocation, but invalidate the selected slot: a failed fill
                // must not leave unrelated bytes addressable under its previous range.
                self.read_cache.recycle_failed_fill(slot, window);
                return Err(error);
            }
        };
        window.truncate(read);
        let copied = buffer.len().min(window.len());
        buffer[..copied].copy_from_slice(&window[..copied]);
        self.read_cache.finish_fill(slot, start, window);
        self.advance_position(copied)?;
        Ok(copied)
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

    #[cfg(feature = "dev-tools")]
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

    #[cfg(test)]
    fn slot_for_path_for_test(&self, path: &Path) -> Option<Arc<ArchiveTemplateSlot>> {
        let key = ArchiveCacheKey::from_path(path).ok()?;
        lock_unpoisoned(&self.inner)
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| Arc::clone(&entry.slot))
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
    nested_path: NestedCachePath,
    bytes: Arc<Vec<u8>>,
    last_used: Instant,
}

#[derive(Clone, PartialEq, Eq)]
enum NestedCachePath {
    Named(String),
    RemoteIndices {
        archive_key: ArchiveCacheKey,
        indices: Vec<usize>,
        display_name: String,
    },
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
        self.get_key(zip_path, &NestedCachePath::Named(nested_path.to_owned()))
    }

    fn get_remote(
        &self,
        archive_key: &ArchiveCacheKey,
        indices: &[usize],
        display_name: &str,
    ) -> Option<Arc<Vec<u8>>> {
        self.get_key(
            &archive_key.path,
            &NestedCachePath::RemoteIndices {
                archive_key: archive_key.clone(),
                indices: indices.to_vec(),
                display_name: display_name.to_owned(),
            },
        )
    }

    fn get_key(&self, zip_path: &Path, nested_path: &NestedCachePath) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.inner.lock().ok()?;
        for e in inner.entries.iter_mut() {
            if e.zip_path == zip_path && &e.nested_path == nested_path {
                e.last_used = Instant::now();
                return Some(e.bytes.clone());
            }
        }
        None
    }

    fn insert(&self, zip_path: PathBuf, nested_path: String, bytes: Arc<Vec<u8>>) {
        self.insert_key(zip_path, NestedCachePath::Named(nested_path), bytes);
    }

    fn insert_remote(
        &self,
        archive_key: ArchiveCacheKey,
        indices: Vec<usize>,
        display_name: String,
        bytes: Arc<Vec<u8>>,
    ) {
        self.insert_key(
            archive_key.path.clone(),
            NestedCachePath::RemoteIndices {
                archive_key,
                indices,
                display_name,
            },
            bytes,
        );
    }

    fn insert_key(&self, zip_path: PathBuf, nested_path: NestedCachePath, bytes: Arc<Vec<u8>>) {
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

/// 外側のフォルダ/ZIP/PDF を切り替えたとき、内側 ZIP bytes だけを破棄する。
///
/// 解析済み目次はプロセス共有であり、別 context の移動では失効させない。
/// `ArchiveCacheKey` の path + mtime + len が変われば次の lookup で自動的に外れ、
/// 変わらない書庫は 8 書庫上限の LRU 内で再利用する。
pub fn clear_nested_cache() {
    NESTED_CACHE.clear();
}

/// ベンチマークで cold miss を再現するためだけに、共有目次 cache を明示的に破棄する。
/// 通常の navigation / reload から呼んではならない。
#[cfg(feature = "dev-tools")]
pub fn clear_archive_directory_cache_for_benchmark() {
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
    enumerate_image_entries_detailed_with_cancel(zip_path, None)
}

pub fn enumerate_image_entries_detailed_with_cancel(
    zip_path: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> std::io::Result<ZipEnumeration> {
    enumerate_image_entries_detailed_with_cancels(zip_path, cancel, None)
}

pub fn enumerate_image_entries_detailed_with_cancels(
    zip_path: &Path,
    cancel: Option<&AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<ZipEnumeration> {
    if crate::rar_loader::is_rar_path(zip_path) {
        return crate::rar_loader::enumerate_image_entries_detailed_with_cancel(zip_path, cancel);
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
        cancel,
        stop,
    )?;
    Ok(ZipEnumeration {
        entries: out,
        has_foreign_archives: has_foreign,
        legacy_renames,
    })
}

#[cfg(test)]
pub(crate) fn nested_cache_contains(zip_path: &Path, nested_name: &str) -> bool {
    if NESTED_CACHE.get(zip_path, nested_name).is_some() {
        return true;
    }
    let Ok(inner) = NESTED_CACHE.inner.lock() else {
        return false;
    };
    inner.entries.iter().any(|entry| {
        entry.zip_path == zip_path
            && matches!(&entry.nested_path, NestedCachePath::RemoteIndices { display_name, .. } if display_name == nested_name)
    })
}

#[cfg(test)]
static RAW_PAYLOAD_READS: LazyLock<Mutex<std::collections::HashMap<(PathBuf, String), usize>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Count attempts to read an image RAW leaf, excluding the enclosing nested ZIP bytes.
#[cfg(test)]
pub(crate) fn raw_payload_read_count(zip_path: &Path, entry_name: &str) -> usize {
    lock_unpoisoned(&RAW_PAYLOAD_READS)
        .get(&(zip_path.to_path_buf(), entry_name.to_owned()))
        .copied()
        .unwrap_or(0)
}

#[cfg(test)]
fn note_raw_payload_read(zip_path: &Path, entry_name: &str) {
    if crate::raw_format::is_raw_path(Path::new(entry_name)) {
        *lock_unpoisoned(&RAW_PAYLOAD_READS)
            .entry((zip_path.to_path_buf(), entry_name.to_owned()))
            .or_default() += 1;
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum TestZipUnreadableMetadata {
    Encrypted,
    UnsupportedCompression,
}

/// Mark a synthetic ZIP entry unreadable without damaging its central directory.
#[cfg(test)]
pub(crate) fn mark_zip_entry_unreadable_for_test(
    bytes: &mut [u8],
    entry_name: &str,
    failure: TestZipUnreadableMetadata,
) {
    let offset = bytes
        .windows(4)
        .enumerate()
        .find_map(|(offset, magic)| {
            if magic != b"PK\x01\x02" || offset + 46 > bytes.len() {
                return None;
            }
            let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
            (bytes.get(offset + 46..offset + 46 + name_len) == Some(entry_name.as_bytes()))
                .then_some(offset)
        })
        .expect("fixture has the named central-directory entry");
    match failure {
        TestZipUnreadableMetadata::Encrypted => bytes[offset + 8] |= 1,
        // Bzip2 is valid ZIP metadata but unsupported by this build.
        TestZipUnreadableMetadata::UnsupportedCompression => {
            bytes[offset + 10..offset + 12].copy_from_slice(&12_u16.to_le_bytes());
        }
    }
}

/// Physical location of one archive entry. ZIP indices include each nested
/// container followed by the leaf index, so duplicate decoded names stay distinct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RemoteArchiveCandidateCursor {
    Zip(Vec<usize>),
    Rar(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteImageCandidate {
    pub entry_name: String,
    pub cursor: RemoteArchiveCandidateCursor,
}

#[cfg(test)]
static REMOTE_CANDIDATE_PAYLOAD_READS: LazyLock<
    Mutex<std::collections::HashMap<(PathBuf, RemoteArchiveCandidateCursor), usize>>,
> = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
pub(crate) fn candidate_payload_read_count(
    zip_path: &Path,
    cursor: &RemoteArchiveCandidateCursor,
) -> usize {
    lock_unpoisoned(&REMOTE_CANDIDATE_PAYLOAD_READS)
        .get(&(zip_path.to_path_buf(), cursor.clone()))
        .copied()
        .unwrap_or(0)
}

fn remote_readable_zip_metadata(entry: &zip::read::ZipFile<'_>) -> bool {
    !entry.encrypted()
        && matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        )
}

/// Return the next Remote archive image in reading order using directory metadata.
/// Image payloads are never read here. A nested ZIP container may be expanded via
/// the bounded cache, and is visited only when it precedes the requested candidate.
/// Pass the last unreadable candidate's cursor as `after` to continue in the
/// same request. Decoded names are display labels and are never used as cursors.
pub fn next_remote_image_candidate(
    zip_path: &Path,
    directory_prefix: Option<&str>,
    after: Option<&RemoteArchiveCandidateCursor>,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<RemoteImageCandidate>> {
    if cancelled_by(Some(cancel), stop) {
        return Err(interrupted_error());
    }
    if crate::rar_loader::is_rar_path(zip_path) {
        let entries = crate::rar_loader::enumerate_image_entries_detailed_with_cancel(
            zip_path,
            Some(cancel),
        )?
        .entries;
        let candidates: Vec<_> = entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| RemoteImageCandidate {
                entry_name: entry.entry_name,
                cursor: RemoteArchiveCandidateCursor::Rar(index),
            })
            .collect();
        if let Some(prefix) = directory_prefix {
            return Ok(next_after_cursor(
                ordered_remote_directory_candidates(candidates, prefix),
                after,
            ));
        }
        return Ok(next_after_cursor(candidates, after));
    }
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, Some(cancel))?;
    if let Some(prefix) = directory_prefix {
        return next_remote_directory_candidate(
            &mut archive,
            zip_path,
            prefix,
            after,
            cancel,
            stop,
        );
    }
    let mut passed_after = after.is_none();
    find_next_remote_file_candidate(
        &mut archive,
        zip_path,
        "",
        &[],
        after,
        &mut passed_after,
        cancel,
        stop,
    )
}

fn remote_name_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let sort = crate::app::BOOK_READING_PAGE_ORDER;
    sort.compare_name_keys(&sort.name_key(a), 0, &sort.name_key(b), 0)
}

fn next_after_cursor(
    candidates: impl IntoIterator<Item = RemoteImageCandidate>,
    after: Option<&RemoteArchiveCandidateCursor>,
) -> Option<RemoteImageCandidate> {
    let mut passed_after = after.is_none();
    for candidate in candidates {
        if passed_after {
            return Some(candidate);
        }
        if Some(&candidate.cursor) == after {
            passed_after = true;
        }
    }
    None
}

fn remote_metadata_indices<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Vec<(usize, String, bool)>> {
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        if cancelled_by(Some(cancel), stop) {
            return Err(interrupted_error());
        }
        let Ok(entry) = archive.by_index_raw(index) else {
            continue;
        };
        if !entry.is_file() || !remote_readable_zip_metadata(&entry) {
            continue;
        }
        let name = normalized_zip_entry_name(&entry);
        if should_ignore(&name) {
            continue;
        }
        let Some(ext) = lowercase_ext(&name) else {
            continue;
        };
        let image = is_image_ext(&ext);
        if image || crate::folder_tree::is_zip_extension(&ext) {
            entries.push((index, name, image));
        }
    }
    Ok(entries)
}

fn remote_nested_bytes<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    outer_zip_path: &Path,
    full_name: &str,
    index_chain: &[usize],
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<Arc<Vec<u8>>>> {
    let archive_key = ArchiveCacheKey::from_path(outer_zip_path)?;
    if let Some(bytes) = NESTED_CACHE.get_remote(&archive_key, index_chain, full_name) {
        return Ok(Some(bytes));
    }
    let Ok(mut entry) = archive.by_index(index) else {
        return Ok(None);
    };
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    if read_to_end_with_cancel(&mut entry, &mut bytes, Some(cancel), stop).is_err() {
        if cancelled_by(Some(cancel), stop) {
            return Err(interrupted_error());
        }
        return Ok(None);
    }
    if cancelled_by(Some(cancel), stop) {
        return Err(interrupted_error());
    }
    let bytes = Arc::new(bytes);
    NESTED_CACHE.insert_remote(
        archive_key,
        index_chain.to_vec(),
        full_name.to_owned(),
        bytes.clone(),
    );
    Ok(Some(bytes))
}

#[allow(clippy::too_many_arguments)]
fn find_next_remote_file_candidate<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    parent_indices: &[usize],
    after: Option<&RemoteArchiveCandidateCursor>,
    passed_after: &mut bool,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<RemoteImageCandidate>> {
    for (index, name, is_image) in remote_metadata_indices(archive, cancel, stop)? {
        if cancelled_by(Some(cancel), stop) {
            return Err(interrupted_error());
        }
        let full_name = format!("{prefix}{name}");
        let mut indices = parent_indices.to_vec();
        indices.push(index);
        if is_image {
            let candidate = RemoteImageCandidate {
                entry_name: full_name,
                cursor: RemoteArchiveCandidateCursor::Zip(indices),
            };
            if *passed_after {
                return Ok(Some(candidate));
            }
            if Some(&candidate.cursor) == after {
                *passed_after = true;
            }
            continue;
        }
        let Some(bytes) = remote_nested_bytes(
            archive,
            index,
            outer_zip_path,
            &full_name,
            &indices,
            cancel,
            stop,
        )?
        else {
            continue;
        };
        let Ok(mut inner) = zip::ZipArchive::new(Cursor::new(bytes.as_slice())) else {
            continue;
        };
        if let Some(found) = find_next_remote_file_candidate(
            &mut inner,
            outer_zip_path,
            &format!("{full_name}/"),
            &indices,
            after,
            passed_after,
            cancel,
            stop,
        )? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn next_remote_directory_candidate<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    zip_path: &Path,
    prefix: &str,
    after: Option<&RemoteArchiveCandidateCursor>,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<RemoteImageCandidate>> {
    let directory = prefix.trim_end_matches('/');
    let mut direct: Vec<_> = remote_metadata_indices(archive, cancel, stop)?
        .into_iter()
        .filter_map(|(index, name, image)| {
            (image && name.rsplit_once('/').map_or("", |(parent, _)| parent) == directory)
                .then_some(RemoteImageCandidate {
                    entry_name: name,
                    cursor: RemoteArchiveCandidateCursor::Zip(vec![index]),
                })
        })
        .collect();
    sort_remote_direct_images(&mut direct);
    if (after.is_none()
        || after.is_some_and(|cursor| direct.iter().any(|entry| &entry.cursor == cursor)))
        && let Some(candidate) = next_after_cursor(direct, after)
    {
        return Ok(Some(candidate));
    }
    // No direct image remains. Collect only eligible central-directory entries,
    // expanding nested ZIP containers without preparing an image decompressor.
    let mut entries = Vec::new();
    collect_remote_metadata_candidates(archive, zip_path, "", &[], &mut entries, cancel, stop)?;
    Ok(next_after_cursor(
        ordered_remote_directory_candidates(entries, prefix),
        after,
    ))
}

fn collect_remote_metadata_candidates<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    parent_indices: &[usize],
    out: &mut Vec<RemoteImageCandidate>,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<()> {
    for (index, name, is_image) in remote_metadata_indices(archive, cancel, stop)? {
        if cancelled_by(Some(cancel), stop) {
            return Err(interrupted_error());
        }
        let full_name = format!("{prefix}{name}");
        let mut indices = parent_indices.to_vec();
        indices.push(index);
        if is_image {
            out.push(RemoteImageCandidate {
                entry_name: full_name,
                cursor: RemoteArchiveCandidateCursor::Zip(indices),
            });
            continue;
        }
        let Some(bytes) = remote_nested_bytes(
            archive,
            index,
            outer_zip_path,
            &full_name,
            &indices,
            cancel,
            stop,
        )?
        else {
            continue;
        };
        let Ok(mut inner) = zip::ZipArchive::new(Cursor::new(bytes.as_slice())) else {
            continue;
        };
        collect_remote_metadata_candidates(
            &mut inner,
            outer_zip_path,
            &format!("{full_name}/"),
            &indices,
            out,
            cancel,
            stop,
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct RemoteCandidateNode {
    images: Vec<RemoteImageCandidate>,
    dirs: std::collections::BTreeMap<String, RemoteCandidateNode>,
}

fn sort_remote_direct_images(images: &mut [RemoteImageCandidate]) {
    images.sort_by(|a, b| {
        remote_name_compare(entry_basename(&a.entry_name), entry_basename(&b.entry_name))
    });
}

fn ordered_remote_directory_candidates(
    entries: Vec<RemoteImageCandidate>,
    directory_prefix: &str,
) -> Vec<RemoteImageCandidate> {
    let mut root = RemoteCandidateNode::default();
    for entry in entries {
        let directory = entry_dir(&entry.entry_name).to_owned();
        let mut node = &mut root;
        for segment in directory.split('/').filter(|segment| !segment.is_empty()) {
            node = node.dirs.entry(segment.to_owned()).or_default();
        }
        node.images.push(entry);
    }
    let mut node = &root;
    for segment in directory_prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        let Some(child) = node.dirs.get(segment) else {
            return Vec::new();
        };
        node = child;
    }
    let mut ordered = Vec::new();
    collect_remote_tree_order(node, &mut ordered);
    ordered
}

fn collect_remote_tree_order(node: &RemoteCandidateNode, out: &mut Vec<RemoteImageCandidate>) {
    let mut images = node.images.clone();
    sort_remote_direct_images(&mut images);
    out.extend(images);
    let mut dirs: Vec<_> = node.dirs.iter().collect();
    dirs.sort_by(|a, b| remote_name_compare(a.0, b.0));
    for (_, child) in dirs {
        collect_remote_tree_order(child, out);
    }
}

/// Resolve an explicit ZIP entry through the normal exact/legacy/decoded-name
/// lookup, retaining each physical index for later payload reads and RAW identity.
pub fn resolve_remote_image_candidate(
    zip_path: &Path,
    entry_name: &str,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<RemoteImageCandidate> {
    if is_cancelled(Some(cancel)) {
        return Err(interrupted_error());
    }
    if crate::rar_loader::is_rar_path(zip_path) {
        let entries = crate::rar_loader::enumerate_image_entries_detailed_with_cancel(
            zip_path,
            Some(cancel),
        )?
        .entries;
        return entries
            .into_iter()
            .enumerate()
            .find(|(_, entry)| entry.entry_name == entry_name)
            .map(|(index, entry)| RemoteImageCandidate {
                entry_name: entry.entry_name,
                cursor: RemoteArchiveCandidateCursor::Rar(index),
            })
            .ok_or_else(|| entry_not_found(entry_name));
    }
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, Some(cancel))?;
    let parts = split_nested_zip_path(entry_name);
    match resolve_entry_index(
        &mut archive,
        entry_name,
        decoded_name_cache_key(zip_path),
        Some(cancel),
    ) {
        Ok(index) => {
            let entry = archive
                .by_index_raw(index)
                .map_err(|error| zip_error_to_io(error, Some(cancel)))?;
            if entry.is_file() && remote_readable_zip_metadata(&entry) {
                return Ok(RemoteImageCandidate {
                    entry_name: normalized_zip_entry_name(&entry),
                    cursor: RemoteArchiveCandidateCursor::Zip(vec![index]),
                });
            }
            if parts.len() < 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "ZIP entry is not readable",
                ));
            }
            // The named loader falls through after a literal read failure.
            // Metadata already proves this literal cannot be read; resolve
            // the nested path without reading either image payload here.
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if parts.len() < 2 {
        return Err(entry_not_found(entry_name));
    }
    let mut indices = Vec::new();
    resolve_remote_nested_candidate(&mut archive, zip_path, &parts, "", &mut indices, cancel)
}

fn resolve_remote_nested_candidate<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    parts: &[&str],
    prefix: &str,
    indices: &mut Vec<usize>,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<RemoteImageCandidate> {
    if is_cancelled(Some(cancel)) {
        return Err(interrupted_error());
    }
    let index = resolve_entry_index(archive, parts[0], None, Some(cancel))?;
    let entry = archive
        .by_index_raw(index)
        .map_err(|error| zip_error_to_io(error, Some(cancel)))?;
    if !entry.is_file() || !remote_readable_zip_metadata(&entry) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ZIP entry is not readable",
        ));
    }
    let full_name = format!("{prefix}{}", normalized_zip_entry_name(&entry));
    indices.push(index);
    drop(entry);
    if parts.len() == 1 {
        return Ok(RemoteImageCandidate {
            entry_name: full_name,
            cursor: RemoteArchiveCandidateCursor::Zip(indices.clone()),
        });
    }
    let bytes = remote_nested_bytes(
        archive,
        index,
        outer_zip_path,
        &full_name,
        indices,
        cancel,
        None,
    )?
    .ok_or_else(|| std::io::Error::other("nested ZIP entry is unreadable"))?;
    let reader = CancellableReader::new(Cursor::new(bytes.as_slice()), Some(cancel.clone()));
    let mut inner =
        zip::ZipArchive::new(reader).map_err(|error| zip_error_to_io(error, Some(cancel)))?;
    resolve_remote_nested_candidate(
        &mut inner,
        outer_zip_path,
        &parts[1..],
        &format!("{full_name}/"),
        indices,
        cancel,
    )
}

/// Read exactly the physical leaf selected by the metadata cursor. Nested ZIP
/// cache lookups use the index chain, so equal decoded names never alias.
pub fn read_remote_image_candidate_bytes_cancellable(
    zip_path: &Path,
    candidate: &RemoteImageCandidate,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    if is_cancelled(Some(cancel)) {
        return Err(interrupted_error());
    }
    match &candidate.cursor {
        RemoteArchiveCandidateCursor::Rar(_) => {
            #[cfg(test)]
            note_remote_candidate_payload_read(zip_path, candidate);
            crate::rar_loader::read_entry_bytes(zip_path, &candidate.entry_name)
        }
        RemoteArchiveCandidateCursor::Zip(indices) if !indices.is_empty() => {
            let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, Some(cancel))?;
            read_remote_zip_candidate_recursive(
                &mut archive,
                zip_path,
                &candidate.entry_name,
                indices,
                &[],
                "",
                cancel,
            )
        }
        RemoteArchiveCandidateCursor::Zip(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty ZIP entry cursor",
        )),
    }
}

#[cfg(test)]
fn note_remote_candidate_payload_read(zip_path: &Path, candidate: &RemoteImageCandidate) {
    *lock_unpoisoned(&REMOTE_CANDIDATE_PAYLOAD_READS)
        .entry((zip_path.to_path_buf(), candidate.cursor.clone()))
        .or_default() += 1;
    note_raw_payload_read(zip_path, &candidate.entry_name);
}

#[allow(clippy::too_many_arguments)]
fn read_remote_zip_candidate_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    candidate_name: &str,
    indices: &[usize],
    parent_indices: &[usize],
    prefix: &str,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    if is_cancelled(Some(cancel)) {
        return Err(interrupted_error());
    }
    let index = indices[0];
    if indices.len() == 1 {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| zip_error_to_io(error, Some(cancel)))?;
        #[cfg(test)]
        note_remote_candidate_payload_read(
            outer_zip_path,
            &RemoteImageCandidate {
                entry_name: candidate_name.to_owned(),
                cursor: RemoteArchiveCandidateCursor::Zip(
                    parent_indices.iter().copied().chain([index]).collect(),
                ),
            },
        );
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        read_to_end_with_cancel(&mut entry, &mut bytes, Some(cancel), None)?;
        return Ok(bytes);
    }
    let entry = archive
        .by_index_raw(index)
        .map_err(|error| zip_error_to_io(error, Some(cancel)))?;
    let full_name = format!("{prefix}{}", normalized_zip_entry_name(&entry));
    drop(entry);
    let mut chain = parent_indices.to_vec();
    chain.push(index);
    let bytes = remote_nested_bytes(
        archive,
        index,
        outer_zip_path,
        &full_name,
        &chain,
        cancel,
        None,
    )?
    .ok_or_else(|| std::io::Error::other("nested ZIP entry is unreadable"))?;
    let reader = CancellableReader::new(Cursor::new(bytes.as_slice()), Some(cancel.clone()));
    let mut inner =
        zip::ZipArchive::new(reader).map_err(|error| zip_error_to_io(error, Some(cancel)))?;
    read_remote_zip_candidate_recursive(
        &mut inner,
        outer_zip_path,
        candidate_name,
        &indices[1..],
        &chain,
        &format!("{full_name}/"),
        cancel,
    )
}

/// Conservative RAW possibility check for early cache lookups that precede
/// representative selection. Actual Remote prefetch resolves its representative.
/// A nested archive is opaque here, and an unreadable image may expose later RAW.
pub fn prefetch_may_select_raw_without_extraction(
    zip_path: &Path,
    directory_prefix: Option<&str>,
) -> std::io::Result<bool> {
    if crate::rar_loader::is_rar_path(zip_path) {
        let entries = crate::rar_loader::enumerate_image_entries_detailed(zip_path)?.entries;
        return Ok(entries.iter().any(|entry| {
            crate::raw_format::is_raw_path(Path::new(&entry.entry_name))
                && directory_prefix.is_none_or(|prefix| entry.entry_name.starts_with(prefix))
        }));
    }
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, None)?;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
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
        if crate::folder_tree::is_zip_extension(&ext) {
            let nested_prefix = format!("{name}/");
            if directory_prefix
                .is_none_or(|prefix| name.starts_with(prefix) || prefix.starts_with(&nested_prefix))
            {
                return Ok(true);
            }
        } else if crate::raw_format::is_raw_path(Path::new(&name))
            && directory_prefix.is_none_or(|prefix| name.starts_with(prefix))
        {
            return Ok(true);
        }
    }
    Ok(false)
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
    cancel: Option<&std::sync::atomic::AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<()> {
    let len = archive.len();
    for i in 0..len {
        if cancelled_by(cancel, stop) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "ZIP enumeration cancelled",
            ));
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
                    if read_to_end_with_cancel(&mut entry, &mut buf, cancel, stop).is_err() {
                        if cancelled_by(cancel, stop) {
                            return Err(interrupted_error());
                        }
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
                cancel,
                stop,
            )?;
        }
    }
    Ok(())
}

fn cancelled_by(cancel: Option<&AtomicBool>, stop: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|token| token.load(Ordering::Relaxed))
        || stop.is_some_and(|token| token.load(Ordering::Relaxed))
}

fn read_to_end_with_cancel(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    cancel: Option<&AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<()> {
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if cancelled_by(cancel, stop) {
            return Err(interrupted_error());
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(read) => output.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
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
    read_first_image_bytes_impl(zip_path, None, None)
        .ok()
        .flatten()
}

/// Remote page representative selection. The selected entry is the same one
/// the legacy first-image loader would return, but nested extraction and image
/// reads stop at the same cancellation boundaries as entry reads.
pub fn read_first_image_bytes_cancellable(
    zip_path: &Path,
    cancel: &Arc<AtomicBool>,
) -> std::io::Result<Option<(String, Vec<u8>)>> {
    read_first_image_bytes_cancellable_with_stop(zip_path, cancel, None)
}

pub fn read_first_image_bytes_cancellable_with_stop(
    zip_path: &Path,
    cancel: &Arc<AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<(String, Vec<u8>)>> {
    read_first_image_bytes_impl(zip_path, Some(cancel), stop)
}

fn read_first_image_bytes_impl(
    zip_path: &Path,
    cancel: Option<&Arc<AtomicBool>>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<(String, Vec<u8>)>> {
    if cancelled_by(cancel.map(Arc::as_ref), stop) {
        return Err(interrupted_error());
    }
    if crate::rar_loader::is_rar_path(zip_path) {
        let result = crate::rar_loader::read_first_image_bytes(zip_path);
        return if cancelled_by(cancel.map(Arc::as_ref), stop) {
            Err(interrupted_error())
        } else {
            Ok(result)
        };
    }
    let file_size = std::fs::metadata(zip_path)
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);
    let t0 = std::time::Instant::now();
    let mut archive = ARCHIVE_DIRECTORY_CACHE.open_archive(zip_path, cancel)?;
    let result =
        read_first_image_recursive(&mut archive, zip_path, "", cancel.map(Arc::as_ref), stop)?;
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if total_ms > 50.0
        && let Some((ref name, ref bytes)) = result
    {
        crate::logger::log(format!(
            "      [zip detail] zip_size={:.1}MB total={total_ms:.0}ms bytes={} {}  {}",
            file_size as f64 / (1024.0 * 1024.0),
            bytes.len(),
            name,
            zip_path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        ));
    }
    Ok(result)
}

fn read_first_image_recursive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    outer_zip_path: &Path,
    prefix: &str,
    cancel: Option<&AtomicBool>,
    stop: Option<&AtomicBool>,
) -> std::io::Result<Option<(String, Vec<u8>)>> {
    let len = archive.len();
    for i in 0..len {
        if cancelled_by(cancel, stop) {
            return Err(interrupted_error());
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
            #[cfg(test)]
            note_raw_payload_read(outer_zip_path, &full_name);
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            if read_to_end_with_cancel(&mut entry, &mut bytes, cancel, stop).is_err() {
                if cancelled_by(cancel, stop) {
                    return Err(interrupted_error());
                }
                continue;
            }
            return Ok(Some((full_name, bytes)));
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
                    if read_to_end_with_cancel(&mut entry, &mut buf, cancel, stop).is_err() {
                        if cancelled_by(cancel, stop) {
                            return Err(interrupted_error());
                        }
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
                read_first_image_recursive(&mut inner, outer_zip_path, &new_prefix, cancel, stop)?
            {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
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
    #[cfg(test)]
    note_raw_payload_read(zip_path, entry_name);
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

    struct CancelAfterRead<R> {
        inner: R,
        cancel: Arc<AtomicBool>,
        armed: Arc<AtomicBool>,
    }

    impl<R: Read> Read for CancelAfterRead<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let read = self.inner.read(buf)?;
            if read > 0 && self.armed.load(Ordering::Relaxed) {
                self.cancel.store(true, Ordering::Release);
            }
            Ok(read)
        }
    }

    impl<R: Seek> Seek for CancelAfterRead<R> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn cancelled_nested_selection_and_enumeration_stop_before_cache_publication() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cancel-nested.zip");
        let inner_bytes = {
            let cursor = Cursor::new(Vec::new());
            let mut writer = zip::ZipWriter::new(cursor);
            writer
                .start_file("leaf.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&vec![7; 256 * 1024]).unwrap();
            writer.finish().unwrap().into_inner()
        };
        let outer_bytes = {
            let cursor = Cursor::new(Vec::new());
            let mut writer = zip::ZipWriter::new(cursor);
            writer
                .start_file(
                    "inner.zip",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(&inner_bytes).unwrap();
            writer.finish().unwrap().into_inner()
        };
        std::fs::write(&path, &outer_bytes).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let armed = Arc::new(AtomicBool::new(false));
        let reader = CancelAfterRead {
            inner: Cursor::new(outer_bytes.clone()),
            cancel: Arc::clone(&cancel),
            armed: Arc::clone(&armed),
        };
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        armed.store(true, Ordering::Release);
        let result = read_first_image_recursive(&mut archive, &path, "", Some(&cancel), None);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!nested_cache_contains(&path, "inner.zip"));

        cancel.store(false, Ordering::Release);
        let reader = CancelAfterRead {
            inner: Cursor::new(outer_bytes),
            cancel: Arc::clone(&cancel),
            armed,
        };
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let mut entries = Vec::new();
        let mut has_foreign = false;
        let mut legacy_renames = Vec::new();
        let result = enumerate_recursive(
            &mut archive,
            &path,
            "",
            "",
            0,
            &mut entries,
            &mut has_foreign,
            &mut legacy_renames,
            Some(&cancel),
            None,
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!nested_cache_contains(&path, "inner.zip"));
    }

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

    fn measure_zip_directory_reads(
        path: &Path,
        window_bytes: usize,
        window_count: usize,
    ) -> (
        Vec<(String, bool, u64)>,
        PositionedIoSnapshot,
        PositionedIoSnapshot,
        Duration,
    ) {
        let probe = Arc::new(PositionedIoProbe::default());
        let (reader, _) = PositionedFileReader::new_with_read_window(
            Arc::new(File::open(path).unwrap()),
            None,
            window_bytes,
            window_count,
            Arc::clone(&probe),
        );
        let started = Instant::now();
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let after_open = probe.snapshot();
        let entries = (0..archive.len())
            .map(|index| {
                let entry = archive.by_index(index).unwrap();
                (entry.name().to_owned(), entry.is_file(), entry.size())
            })
            .collect();
        (entries, after_open, probe.snapshot(), started.elapsed())
    }

    /// Drive zip 2.4's real central-directory parser against a disposable on-disk archive.
    /// The timing is diagnostic only; deterministic assertions use calls, bytes, and metadata.
    #[test]
    fn positioned_read_windows_reduce_zip_parser_reads_without_changing_entries() {
        const ENTRIES: usize = 2_048;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("directory-read-count.zip");
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let payload = vec![0x5a_u8; 4 * 1024];
        for index in 0..ENTRIES {
            use std::io::Write as _;
            writer
                .start_file(format!("chapter/{index:06}.jpg"), options)
                .unwrap();
            writer.write_all(&payload).unwrap();
        }
        writer.finish().unwrap();

        let (baseline_entries, baseline_open, baseline, baseline_elapsed) =
            measure_zip_directory_reads(&path, 0, 0);
        assert_eq!(baseline_entries.len(), ENTRIES);
        let mut selected = None;
        let mut one_window = None;
        for (window_bytes, window_count) in [(256, 1), (256, 2), (512, 2), (1_024, 2), (4_096, 2)] {
            let (entries, after_open, measured, elapsed) =
                measure_zip_directory_reads(&path, window_bytes, window_count);
            assert_eq!(entries, baseline_entries);
            eprintln!(
                "zip_positioned_reader entries={ENTRIES} window_bytes={window_bytes} window_count={window_count} elapsed_ms={:.3} requested_bytes={} open_calls={} open_bytes={} entry_calls={} entry_bytes={} os_read_calls={} os_read_bytes={} cache_hits={} baseline_elapsed_ms={:.3} baseline_open_calls={} baseline_open_bytes={} baseline_calls={} baseline_bytes={}",
                elapsed.as_secs_f64() * 1000.0,
                measured.requested_bytes,
                after_open.os_read_calls,
                after_open.os_read_bytes,
                measured
                    .os_read_calls
                    .saturating_sub(after_open.os_read_calls),
                measured
                    .os_read_bytes
                    .saturating_sub(after_open.os_read_bytes),
                measured.os_read_calls,
                measured.os_read_bytes,
                measured.cache_hits,
                baseline_elapsed.as_secs_f64() * 1000.0,
                baseline_open.os_read_calls,
                baseline_open.os_read_bytes,
                baseline.os_read_calls,
                baseline.os_read_bytes,
            );
            if window_bytes == POSITIONED_READ_WINDOW_BYTES && window_count == 1 {
                one_window = Some(measured);
            }
            if window_bytes == POSITIONED_READ_WINDOW_BYTES
                && window_count == POSITIONED_READ_WINDOW_COUNT
            {
                selected = Some(measured);
            }
        }
        let selected = selected.expect("production read window must be one measured candidate");
        let one_window = one_window.expect("one-window comparison must be measured");
        assert!(
            selected.os_read_calls * 2 < baseline.os_read_calls,
            "the bounded production window must remove most tiny parser reads: baseline={} selected={}",
            baseline.os_read_calls,
            selected.os_read_calls,
        );
        assert!(
            selected.os_read_bytes <= baseline.os_read_bytes.saturating_mul(4),
            "read-ahead must remain bounded on network and rotating media: baseline={} selected={}",
            baseline.os_read_bytes,
            selected.os_read_bytes,
        );
        assert!(
            selected.os_read_calls.saturating_mul(4) <= one_window.os_read_calls.saturating_mul(3),
            "a second window must remove a material share of OS calls: one={} two={}",
            one_window.os_read_calls,
            selected.os_read_calls,
        );
    }

    #[test]
    fn positioned_read_window_keeps_seek_eof_large_read_and_clone_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader-window.bin");
        let expected = (0..2_500).map(|value| value as u8).collect::<Vec<_>>();
        std::fs::write(&path, &expected).unwrap();
        let probe = Arc::new(PositionedIoProbe::default());
        let (mut reader, _) = PositionedFileReader::new_with_read_window(
            Arc::new(File::open(&path).unwrap()),
            None,
            512,
            2,
            Arc::clone(&probe),
        );

        let mut prefix = [0_u8; 40];
        reader.read_exact(&mut prefix).unwrap();
        assert_eq!(prefix.as_slice(), &expected[..40]);
        let calls_after_fill = probe.snapshot().os_read_calls;
        reader.seek(SeekFrom::Start(16)).unwrap();
        let mut cached = [0_u8; 32];
        reader.read_exact(&mut cached).unwrap();
        assert_eq!(cached.as_slice(), &expected[16..48]);
        assert_eq!(probe.snapshot().os_read_calls, calls_after_fill);

        let mut clone = reader.clone();
        assert!(
            clone
                .read_cache
                .windows
                .iter()
                .all(|window| window.bytes.capacity() == 0),
            "cloning a reader must not allocate or copy window storage"
        );
        clone.seek(SeekFrom::Start(16)).unwrap();
        clone.read_exact(&mut cached).unwrap();
        assert_eq!(cached.as_slice(), &expected[16..48]);
        assert_eq!(
            probe.snapshot().os_read_calls,
            calls_after_fill + 1,
            "a request clone must start with an empty mutable cache"
        );

        reader.seek(SeekFrom::Start(2_400)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, expected[2_400..]);
        let mut eof = [0_u8; 1];
        assert_eq!(reader.read(&mut eof).unwrap(), 0);

        let direct_probe = Arc::new(PositionedIoProbe::default());
        let (mut direct_reader, _) = PositionedFileReader::new_with_read_window(
            Arc::new(File::open(&path).unwrap()),
            None,
            512,
            2,
            Arc::clone(&direct_probe),
        );
        let mut large = [0_u8; 1_024];
        direct_reader.read_exact(&mut large).unwrap();
        assert_eq!(large.as_slice(), &expected[..1_024]);
        assert_eq!(
            direct_probe.snapshot().os_read_calls,
            1,
            "large reads bypass the read-ahead copy"
        );
        assert_eq!(direct_reader.read_cache.buffer_allocations, 0);
    }

    #[test]
    fn positioned_read_window_handles_partial_hits_and_two_window_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader-window-eviction.bin");
        let expected = (0..256).map(|value| value as u8).collect::<Vec<_>>();
        std::fs::write(&path, &expected).unwrap();
        let probe = Arc::new(PositionedIoProbe::default());
        let (mut reader, _) = PositionedFileReader::new_with_read_window(
            Arc::new(File::open(path).unwrap()),
            None,
            16,
            2,
            Arc::clone(&probe),
        );

        let mut first = [0_u8; 4];
        reader.read_exact(&mut first).unwrap();
        reader.seek(SeekFrom::Start(64)).unwrap();
        reader.read_exact(&mut first).unwrap();
        assert_eq!(probe.snapshot().os_read_calls, 2);

        // Refresh the first window, then a third fill must evict the older second window.
        reader.seek(SeekFrom::Start(0)).unwrap();
        reader.read_exact(&mut first).unwrap();
        reader.seek(SeekFrom::Start(128)).unwrap();
        reader.read_exact(&mut first).unwrap();
        assert_eq!(probe.snapshot().os_read_calls, 3);
        reader.seek(SeekFrom::Start(64)).unwrap();
        reader.read_exact(&mut first).unwrap();
        assert_eq!(probe.snapshot().os_read_calls, 4);

        // A read crossing a cached window boundary may return the cached prefix first; ReadExact
        // must then continue at the exact logical position without a duplicate or missing byte.
        reader.seek(SeekFrom::Start(76)).unwrap();
        let mut crossing = [0_u8; 12];
        reader.read_exact(&mut crossing).unwrap();
        assert_eq!(crossing.as_slice(), &expected[76..88]);
        assert_eq!(
            reader.read_cache.buffer_allocations, 2,
            "repeated misses must reuse the two fixed slot buffers"
        );
    }

    #[test]
    fn positioned_read_window_checks_cancel_before_returning_cached_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reader-window-cancel.bin");
        std::fs::write(&path, b"cached bytes must stay behind cancellation").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let probe = Arc::new(PositionedIoProbe::default());
        let (mut reader, _) = PositionedFileReader::new_with_read_window(
            Arc::new(File::open(path).unwrap()),
            Some(Arc::clone(&cancel)),
            512,
            2,
            probe,
        );
        let mut first = [0_u8; 6];
        reader.read_exact(&mut first).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        cancel.store(true, Ordering::Release);
        let error = reader.read(&mut first).unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::Interrupted);
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
    fn clearing_nested_bytes_keeps_the_shared_archive_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unrelated-folder-move.zip");
        let nested_name = "chapter.zip";
        write_test_zip(&path, &[("page.jpg", b"PAGE")]);

        assert_eq!(
            read_with_directory_cache(&ARCHIVE_DIRECTORY_CACHE, &path, "page.jpg", None,).unwrap(),
            b"PAGE"
        );
        let warmed_slot = ARCHIVE_DIRECTORY_CACHE
            .slot_for_path_for_test(&path)
            .expect("the parsed directory must be cached");
        NESTED_CACHE.insert(
            path.clone(),
            nested_name.to_owned(),
            Arc::new(vec![1, 2, 3]),
        );

        clear_nested_cache();

        assert!(NESTED_CACHE.get(&path, nested_name).is_none());
        let retained_slot = ARCHIVE_DIRECTORY_CACHE
            .slot_for_path_for_test(&path)
            .expect("an unrelated context move must retain the ready directory template");
        assert!(Arc::ptr_eq(&retained_slot, &warmed_slot));
        assert_eq!(
            read_with_directory_cache(&ARCHIVE_DIRECTORY_CACHE, &path, "page.jpg", None,).unwrap(),
            b"PAGE"
        );
        let reused_slot = ARCHIVE_DIRECTORY_CACHE
            .slot_for_path_for_test(&path)
            .expect("the next read must retain the ready directory template");
        assert!(
            Arc::ptr_eq(&reused_slot, &warmed_slot),
            "the next read must reuse the same parsed directory"
        );
    }

    #[test]
    fn shared_directory_cache_reparses_when_the_archive_identity_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changed-shared.zip");
        write_test_zip(&path, &[("page.jpg", b"OLD")]);
        assert_eq!(
            read_with_directory_cache(&ARCHIVE_DIRECTORY_CACHE, &path, "page.jpg", None,).unwrap(),
            b"OLD"
        );
        let old_slot = ARCHIVE_DIRECTORY_CACHE
            .slot_for_path_for_test(&path)
            .expect("the first archive identity must be cached");

        write_test_zip(&path, &[("page.jpg", b"NEW-LONGER")]);

        assert_eq!(
            read_with_directory_cache(&ARCHIVE_DIRECTORY_CACHE, &path, "page.jpg", None,).unwrap(),
            b"NEW-LONGER"
        );
        let new_slot = ARCHIVE_DIRECTORY_CACHE
            .slot_for_path_for_test(&path)
            .expect("the changed archive identity must be cached");
        assert!(
            !Arc::ptr_eq(&new_slot, &old_slot),
            "a size or mtime change must create and parse a new directory template"
        );
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

    #[test]
    fn remote_file_candidates_follow_central_index_order_without_reading_raw_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner-source.zip");
        write_test_zip(&inner, &[("page.jpg", b"INNER")]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let outer = dir.path().join("outer.zip");
        // The root File loader uses central-directory index order. A later
        // nested ZIP is not expanded while the first outer image is selected.
        write_test_zip(
            &outer,
            &[
                ("z.jpg", b"LAST"),
                ("later.zip", &inner_bytes),
                ("a.dng", b"RAW"),
                ("b.jpg", b"SECOND"),
            ],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let first = next_remote_image_candidate(&outer, None, None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(first.entry_name, "z.jpg");
        assert_eq!(first.cursor, RemoteArchiveCandidateCursor::Zip(vec![0]));
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &first, &cancel).unwrap(),
            b"LAST"
        );
        assert_eq!(raw_payload_read_count(&outer, "a.dng"), 0);
        assert!(!nested_cache_contains(&outer, "later.zip"));
        let nested = next_remote_image_candidate(&outer, None, Some(&first.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(nested.entry_name, "later.zip/page.jpg");
        assert_eq!(nested.cursor, RemoteArchiveCandidateCursor::Zip(vec![1, 0]));
        assert!(nested_cache_contains(&outer, "later.zip"));
        let raw = next_remote_image_candidate(&outer, None, Some(&nested.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(raw.entry_name, "a.dng");
        assert_eq!(raw.cursor, RemoteArchiveCandidateCursor::Zip(vec![2]));
        assert_eq!(raw_payload_read_count(&outer, "a.dng"), 0);
        let last = next_remote_image_candidate(&outer, None, Some(&raw.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(last.entry_name, "b.jpg");
        assert_eq!(
            next_remote_image_candidate(&outer, None, Some(&last.cursor), &cancel, None).unwrap(),
            None
        );
    }

    #[test]
    fn remote_directory_candidates_defer_nested_expansion_until_direct_images_fail() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner-source.zip");
        write_test_zip(&inner, &[("page.cr2", b"RAW")]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let outer = dir.path().join("outer.zip");
        write_test_zip(
            &outer,
            &[
                ("book/inner.zip", &inner_bytes),
                ("book/b.jpg", b"SECOND"),
                ("book/a.jpg", b"FIRST"),
            ],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let first = next_remote_image_candidate(&outer, Some("book/"), None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(first.entry_name, "book/a.jpg");
        assert!(!nested_cache_contains(&outer, "book/inner.zip"));
        let second =
            next_remote_image_candidate(&outer, Some("book/"), Some(&first.cursor), &cancel, None)
                .unwrap()
                .unwrap();
        assert_eq!(second.entry_name, "book/b.jpg");
        assert!(!nested_cache_contains(&outer, "book/inner.zip"));
        let nested =
            next_remote_image_candidate(&outer, Some("book/"), Some(&second.cursor), &cancel, None)
                .unwrap()
                .unwrap();
        assert_eq!(nested.entry_name, "book/inner.zip/page.cr2");
        assert_eq!(raw_payload_read_count(&outer, "book/inner.zip/page.cr2"), 0);
    }

    #[test]
    fn remote_candidate_cursor_advances_across_duplicate_normalized_names() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("duplicate-names.zip");
        write_test_zip(
            &outer,
            &[
                ("a\\p.jpg", b"BADPAYLOAD"),
                ("a/p.jpg", b"GOODPAYLOAD"),
                ("z.jpg", b"LASTPAYLOAD"),
            ],
        );
        let mut bytes = std::fs::read(&outer).unwrap();
        let offset = bytes
            .windows(b"BADPAYLOAD".len())
            .position(|window| window == b"BADPAYLOAD")
            .unwrap();
        bytes[offset] ^= 1; // keep the central CRC, so only the payload read fails
        std::fs::write(&outer, bytes).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let first = next_remote_image_candidate(&outer, None, None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(first.entry_name, "a/p.jpg");
        assert_eq!(first.cursor, RemoteArchiveCandidateCursor::Zip(vec![0]));
        assert!(read_remote_image_candidate_bytes_cancellable(&outer, &first, &cancel).is_err());
        let second = next_remote_image_candidate(&outer, None, Some(&first.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(second.entry_name, "a/p.jpg");
        assert_eq!(second.cursor, RemoteArchiveCandidateCursor::Zip(vec![1]));
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &second, &cancel).unwrap(),
            b"GOODPAYLOAD"
        );
        let third = next_remote_image_candidate(&outer, None, Some(&second.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(third.entry_name, "z.jpg");
        assert_eq!(third.cursor, RemoteArchiveCandidateCursor::Zip(vec![2]));
        assert_eq!(candidate_payload_read_count(&outer, &first.cursor), 1);
        assert_eq!(candidate_payload_read_count(&outer, &second.cursor), 1);
        assert_eq!(candidate_payload_read_count(&outer, &third.cursor), 0);
    }

    #[test]
    fn remote_nested_cache_distinguishes_duplicate_normalized_container_names() {
        let dir = tempfile::tempdir().unwrap();
        let first_inner = dir.path().join("first.zip");
        let second_inner = dir.path().join("second.zip");
        write_test_zip(&first_inner, &[("page.jpg", b"FIRST")]);
        write_test_zip(&second_inner, &[("page.jpg", b"OTHER")]);
        let first_bytes = std::fs::read(&first_inner).unwrap();
        let second_bytes = std::fs::read(&second_inner).unwrap();
        let outer = dir.path().join("outer.zip");
        write_test_zip(
            &outer,
            &[
                ("a\\inner.zip", &first_bytes),
                ("a/inner.zip", &second_bytes),
            ],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let first = next_remote_image_candidate(&outer, None, None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(first.entry_name, "a/inner.zip/page.jpg");
        assert_eq!(first.cursor, RemoteArchiveCandidateCursor::Zip(vec![0, 0]));
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &first, &cancel).unwrap(),
            b"FIRST"
        );
        let second = next_remote_image_candidate(&outer, None, Some(&first.cursor), &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(second.entry_name, first.entry_name);
        assert_eq!(second.cursor, RemoteArchiveCandidateCursor::Zip(vec![1, 0]));
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &second, &cancel).unwrap(),
            b"OTHER"
        );
        assert_eq!(candidate_payload_read_count(&outer, &first.cursor), 1);
        assert_eq!(candidate_payload_read_count(&outer, &second.cursor), 1);
    }

    #[test]
    fn remote_nested_cache_revalidates_precise_outer_mtime_at_same_size() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner.zip");
        let outer = dir.path().join("outer.zip");
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let write_version = |contents: &[u8], modified: std::time::SystemTime| {
            write_test_zip(&inner, &[("page.jpg", contents)]);
            let inner_bytes = std::fs::read(&inner).unwrap();
            write_test_zip(&outer, &[("nested.zip", &inner_bytes)]);
            File::options()
                .write(true)
                .open(&outer)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        };
        write_version(b"FIRST", base);
        let first_meta = std::fs::metadata(&outer).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let first = next_remote_image_candidate(&outer, None, None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &first, &cancel).unwrap(),
            b"FIRST"
        );
        write_version(b"OTHER", base + std::time::Duration::from_millis(1));
        let second_meta = std::fs::metadata(&outer).unwrap();
        assert_eq!(first_meta.len(), second_meta.len());
        assert_ne!(
            first_meta.modified().unwrap(),
            second_meta.modified().unwrap()
        );
        assert_eq!(
            crate::ui_helpers::mtime_secs(&first_meta),
            crate::ui_helpers::mtime_secs(&second_meta)
        );
        let second = next_remote_image_candidate(&outer, None, None, &cancel, None)
            .unwrap()
            .unwrap();
        assert_eq!(second.cursor, first.cursor);
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&outer, &second, &cancel).unwrap(),
            b"OTHER"
        );
    }

    #[test]
    fn remote_explicit_candidate_keeps_literal_and_nested_entry_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner.zip");
        write_test_zip(&inner, &[("page.jpg", b"NESTED")]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let literal = dir.path().join("literal.zip");
        write_test_zip(
            &literal,
            &[
                ("inner.zip/page.jpg", b"LITERAL"),
                ("inner.zip", &inner_bytes),
            ],
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let selected =
            resolve_remote_image_candidate(&literal, "inner.zip/page.jpg", &cancel).unwrap();
        assert_eq!(selected.cursor, RemoteArchiveCandidateCursor::Zip(vec![0]));
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&literal, &selected, &cancel).unwrap(),
            b"LITERAL"
        );
        let nested_only = dir.path().join("nested-only.zip");
        write_test_zip(&nested_only, &[("inner.zip", &inner_bytes)]);
        let selected =
            resolve_remote_image_candidate(&nested_only, "inner.zip/page.jpg", &cancel).unwrap();
        assert_eq!(
            selected.cursor,
            RemoteArchiveCandidateCursor::Zip(vec![0, 0])
        );
        assert_eq!(
            read_remote_image_candidate_bytes_cancellable(&nested_only, &selected, &cancel)
                .unwrap(),
            b"NESTED"
        );
    }

    #[test]
    fn remote_explicit_candidate_falls_through_metadata_unreadable_literal_without_leaf_reads() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        for failure in [
            TestZipUnreadableMetadata::Encrypted,
            TestZipUnreadableMetadata::UnsupportedCompression,
        ] {
            for leaf in ["page.jpg", "page.cr2"] {
                let inner = dir.path().join("inner.zip");
                write_test_zip(&inner, &[(leaf, b"NESTED")]);
                let inner_bytes = std::fs::read(&inner).unwrap();
                let outer = dir.path().join(format!("{failure:?}-{leaf}.zip"));
                let entry_name = format!("inner.zip/{leaf}");
                write_test_zip(
                    &outer,
                    &[(&entry_name, b"LITERAL"), ("inner.zip", &inner_bytes)],
                );
                let mut bytes = std::fs::read(&outer).unwrap();
                mark_zip_entry_unreadable_for_test(&mut bytes, &entry_name, failure);
                std::fs::write(&outer, bytes).unwrap();
                assert_eq!(read_entry_bytes(&outer, &entry_name).unwrap(), b"NESTED");
                let raw_reads_before = raw_payload_read_count(&outer, &entry_name);
                let selected =
                    resolve_remote_image_candidate(&outer, &entry_name, &cancel).unwrap();
                assert_eq!(
                    raw_payload_read_count(&outer, &entry_name),
                    raw_reads_before
                );
                assert_eq!(
                    selected.cursor,
                    RemoteArchiveCandidateCursor::Zip(vec![1, 0])
                );
                assert_eq!(candidate_payload_read_count(&outer, &selected.cursor), 0);
                assert_eq!(
                    candidate_payload_read_count(
                        &outer,
                        &RemoteArchiveCandidateCursor::Zip(vec![0])
                    ),
                    0
                );
                assert_eq!(
                    read_remote_image_candidate_bytes_cancellable(&outer, &selected, &cancel)
                        .unwrap(),
                    b"NESTED"
                );
            }
        }
    }

    #[test]
    fn remote_metadata_excludes_encrypted_and_unsupported_compression_entries() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("eligibility.zip");
        write_test_zip(
            &outer,
            &[
                ("a.cr2", b"ENCRYPTED"),
                ("b.cr2", b"UNSUPPORTED"),
                ("c.jpg", b"USABLE"),
            ],
        );
        let mut bytes = std::fs::read(&outer).unwrap();
        let mut offset = 0;
        let mut patched = 0;
        while offset + 46 <= bytes.len() {
            if bytes[offset..offset + 4] != *b"PK\x01\x02" {
                offset += 1;
                continue;
            }
            let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]) as usize;
            if offset + 46 + name_len > bytes.len() {
                break;
            }
            match &bytes[offset + 46..offset + 46 + name_len] {
                b"a.cr2" => {
                    bytes[offset + 8] |= 1;
                    patched += 1;
                }
                b"b.cr2" => {
                    // Bzip2 is a valid ZIP method, but this build enables only
                    // Stored/Deflated. Method 99 requires AES extra metadata
                    // and would make this fixture an invalid archive instead.
                    bytes[offset + 10..offset + 12].copy_from_slice(&12_u16.to_le_bytes());
                    patched += 1;
                }
                _ => {}
            }
            offset += 46 + name_len + extra_len + comment_len;
        }
        assert_eq!(patched, 2);
        std::fs::write(&outer, bytes).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            next_remote_image_candidate(&outer, None, None, &cancel, None)
                .unwrap()
                .unwrap()
                .entry_name,
            "c.jpg"
        );
        assert_eq!(raw_payload_read_count(&outer, "a.cr2"), 0);
        assert_eq!(raw_payload_read_count(&outer, "b.cr2"), 0);
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
