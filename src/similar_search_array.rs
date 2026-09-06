//! 「別バージョン」検索用の不変 base / delta snapshot。
//!
//! SQLite が正本であり、このファイルと全メモリ構造は派生物である。公開済みの値は
//! 一切変更せず、更新は新しい [`SearchSnapshot`] の差し替えだけで公開する。

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use crate::dupe;
use crate::similar_db::{
    BaseSearchRow, ItemChangeBatch, ItemChangeOp, SimilarDb, current_hash_version,
};

const BASE_FILE: &str = "similar.base";
const LEGACY_SIDECAR_FILE: &str = "similar.compact";
const BASE_MAGIC: [u8; 8] = *b"MIVSIMB1";
const BASE_FORMAT_VERSION: u32 = 1;
const BASE_HEADER_LEN: usize = 96;
const BASE_RECORD_LEN: usize = 48;
const IO_RECORDS: usize = 16 * 1024;
const MAX_CHANGES_BEFORE_COMPACTION: usize = 65_536;
const MIN_CHANGES_BEFORE_COMPACTION: usize = 1_024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) type SearchRecord = BaseSearchRow;

#[derive(Debug)]
pub(crate) struct BaseArray {
    pub(crate) records: Box<[SearchRecord]>,
    pub(crate) store_id: [u8; 16],
    pub(crate) applied_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeltaEntry {
    pub(crate) item_id: u64,
    pub(crate) seq: u64,
    pub(crate) record: Option<SearchRecord>,
}

#[derive(Debug)]
pub(crate) struct SearchSnapshot {
    pub(crate) base: Arc<BaseArray>,
    pub(crate) delta: Arc<[DeltaEntry]>,
    pub(crate) superseded: Arc<[u64]>,
    pub(crate) applied_seq: u64,
}

impl SearchSnapshot {
    pub(crate) fn from_base(base: BaseArray) -> Self {
        let words = base.records.len().div_ceil(64);
        let applied_seq = base.applied_seq;
        Self {
            base: Arc::new(base),
            delta: Arc::from([]),
            superseded: vec![0; words].into(),
            applied_seq,
        }
    }

    pub(crate) fn base_record_is_superseded(&self, index: usize) -> bool {
        self.superseded
            .get(index / 64)
            .is_some_and(|word| word & (1u64 << (index % 64)) != 0)
    }

    pub(crate) fn record_count(&self) -> usize {
        self.base.records.len()
            + self
                .delta
                .iter()
                .filter(|entry| entry.record.is_some())
                .count()
    }

    pub(crate) fn should_compact(&self) -> bool {
        let adaptive = self.compaction_threshold();
        self.delta.len() >= adaptive
            || self.applied_seq.saturating_sub(self.base.applied_seq) >= adaptive as u64
    }

    pub(crate) fn compaction_threshold(&self) -> usize {
        (self.base.records.len() / 20)
            .clamp(MIN_CHANGES_BEFORE_COMPACTION, MAX_CHANGES_BEFORE_COMPACTION)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadSource {
    BaseFile,
    Sqlite,
}

pub(crate) struct LoadedSnapshot {
    pub(crate) snapshot: SearchSnapshot,
    pub(crate) source: LoadSource,
    pub(crate) rejected: Option<BaseRejectReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BaseRejectReason {
    Missing,
    Open(String),
    HeaderShort,
    Magic,
    FormatVersion,
    HeaderLength,
    HashVersion,
    ProxyVersion,
    RecordLength,
    StoreId,
    RecordCount,
    BodyShort,
    RecordOrder,
    Checksum,
}

impl fmt::Display for BaseRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "base file is missing"),
            Self::Open(error) => write!(f, "base file open failed: {error}"),
            Self::HeaderShort => write!(f, "base header is short"),
            Self::Magic => write!(f, "base magic mismatch"),
            Self::FormatVersion => write!(f, "base format version mismatch"),
            Self::HeaderLength => write!(f, "base header length mismatch"),
            Self::HashVersion => write!(f, "base hash_version mismatch"),
            Self::ProxyVersion => write!(f, "base PROXY_VERSION mismatch"),
            Self::RecordLength => write!(f, "base record length mismatch"),
            Self::StoreId => write!(f, "base store_id mismatch"),
            Self::RecordCount => write!(f, "base record_count or file length mismatch"),
            Self::BodyShort => write!(f, "base body is short"),
            Self::RecordOrder => write!(f, "base record order is invalid"),
            Self::Checksum => write!(f, "base checksum mismatch"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MissingHistory;

pub(crate) fn base_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BASE_FILE)
}

/// Step 4 の rowid ベース sidecar は新形式とは独立した派生物で、再利用しない。
pub(crate) fn retire_legacy_sidecar(data_dir: &Path) -> Result<bool, String> {
    let path = data_dir.join(LEGACY_SIDECAR_FILE);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("legacy similar sidecar removal failed: {error}")),
    }
}

pub(crate) fn load_or_rebuild(db: &SimilarDb, path: &Path) -> Result<LoadedSnapshot, String> {
    let store_id = db.search_store_id().map_err(db_error)?;
    match read_base(path, store_id) {
        Ok(base) => Ok(LoadedSnapshot {
            snapshot: SearchSnapshot::from_base(base),
            source: LoadSource::BaseFile,
            rejected: None,
        }),
        Err(reason) => {
            let rows = db
                .load_base_search_rows(current_hash_version())
                .map_err(db_error)?;
            let base = BaseArray {
                records: rows.records.into_boxed_slice(),
                store_id: rows.store_id,
                applied_seq: rows.applied_seq,
            };
            write_base(path, &base)?;
            db.prune_item_changes_through(base.applied_seq)
                .map_err(db_error)?;
            Ok(LoadedSnapshot {
                snapshot: SearchSnapshot::from_base(base),
                source: LoadSource::Sqlite,
                rejected: Some(reason),
            })
        }
    }
}

pub(crate) fn rebuild_from_sqlite(db: &SimilarDb, path: &Path) -> Result<SearchSnapshot, String> {
    let rows = db
        .load_base_search_rows(current_hash_version())
        .map_err(db_error)?;
    let base = BaseArray {
        records: rows.records.into_boxed_slice(),
        store_id: rows.store_id,
        applied_seq: rows.applied_seq,
    };
    write_base(path, &base)?;
    Ok(SearchSnapshot::from_base(base))
}

pub(crate) fn apply_change_batch(
    snapshot: &SearchSnapshot,
    batch: ItemChangeBatch,
) -> Result<Option<SearchSnapshot>, MissingHistory> {
    if batch.latest_seq == snapshot.applied_seq {
        return Ok(None);
    }
    if batch.latest_seq < snapshot.applied_seq {
        return Err(MissingHistory);
    }
    let expected_first = snapshot.applied_seq.checked_add(1).ok_or(MissingHistory)?;
    if batch.changes.first().map(|change| change.seq) != Some(expected_first) {
        return Err(MissingHistory);
    }
    let mut expected = expected_first;
    for change in &batch.changes {
        if change.seq != expected {
            return Err(MissingHistory);
        }
        expected = expected.checked_add(1).ok_or(MissingHistory)?;
    }
    if batch.changes.last().map(|change| change.seq) != Some(batch.latest_seq) {
        return Err(MissingHistory);
    }

    let mut delta = snapshot
        .delta
        .iter()
        .map(|entry| (entry.item_id, *entry))
        .collect::<BTreeMap<_, _>>();
    let mut superseded = snapshot.superseded.to_vec();
    for change in batch.changes {
        if let Ok(index) = snapshot
            .base
            .records
            .binary_search_by_key(&change.item_id, |record| record.item_id)
        {
            superseded[index / 64] |= 1u64 << (index % 64);
        }
        let record = match change.op {
            ItemChangeOp::Add | ItemChangeOp::Update => Some(SearchRecord {
                item_id: change.item_id,
                signature: change.signature.ok_or(MissingHistory)?,
                quality: change.quality.ok_or(MissingHistory)?,
                revision: change.revision.ok_or(MissingHistory)?,
            }),
            ItemChangeOp::Delete => None,
        };
        delta.insert(
            change.item_id,
            DeltaEntry {
                item_id: change.item_id,
                seq: change.seq,
                record,
            },
        );
    }
    Ok(Some(SearchSnapshot {
        base: Arc::clone(&snapshot.base),
        delta: delta.into_values().collect(),
        superseded: superseded.into(),
        applied_seq: batch.latest_seq,
    }))
}

pub(crate) fn compacted_base(snapshot: &SearchSnapshot) -> BaseArray {
    let mut records = Vec::with_capacity(snapshot.record_count());
    let mut base_index = 0usize;
    let mut delta_index = 0usize;
    while base_index < snapshot.base.records.len() || delta_index < snapshot.delta.len() {
        let base = snapshot.base.records.get(base_index);
        let delta = snapshot.delta.get(delta_index);
        match (base, delta) {
            (Some(base), Some(delta)) if base.item_id < delta.item_id => {
                if !snapshot.base_record_is_superseded(base_index) {
                    records.push(*base);
                }
                base_index += 1;
            }
            (Some(base), Some(delta)) if base.item_id == delta.item_id => {
                if let Some(record) = delta.record {
                    records.push(record);
                }
                base_index += 1;
                delta_index += 1;
            }
            (_, Some(delta)) => {
                if let Some(record) = delta.record {
                    records.push(record);
                }
                delta_index += 1;
            }
            (Some(base), None) => {
                if !snapshot.base_record_is_superseded(base_index) {
                    records.push(*base);
                }
                base_index += 1;
            }
            (None, None) => break,
        }
    }
    BaseArray {
        records: records.into_boxed_slice(),
        store_id: snapshot.base.store_id,
        applied_seq: snapshot.applied_seq,
    }
}

pub(crate) fn write_compacted_base(path: &Path, base: &BaseArray) -> Result<(), String> {
    write_base(path, base)
}

pub(crate) fn snapshot_on_new_base(
    current: &SearchSnapshot,
    base: Arc<BaseArray>,
) -> Result<SearchSnapshot, MissingHistory> {
    if current.base.store_id != base.store_id || current.base.applied_seq > base.applied_seq {
        // 別の復旧処理が、統合対象より新しい base を既に公開している。
        return Err(MissingHistory);
    }
    let retained = current
        .delta
        .iter()
        .filter(|entry| entry.seq > base.applied_seq)
        .copied()
        .collect::<Vec<_>>();
    let mut superseded = vec![0u64; base.records.len().div_ceil(64)];
    for entry in &retained {
        if let Ok(index) = base
            .records
            .binary_search_by_key(&entry.item_id, |record| record.item_id)
        {
            superseded[index / 64] |= 1u64 << (index % 64);
        }
    }
    if current.applied_seq < base.applied_seq {
        return Err(MissingHistory);
    }
    Ok(SearchSnapshot {
        base,
        delta: retained.into(),
        superseded: superseded.into(),
        applied_seq: current.applied_seq,
    })
}

#[derive(Clone, Copy)]
struct BaseHeader {
    hash_version: i64,
    proxy_version: u32,
    record_count: u64,
    applied_seq: u64,
    store_id: [u8; 16],
    body_sha256: [u8; 32],
}

impl BaseHeader {
    fn encode(self) -> [u8; BASE_HEADER_LEN] {
        let mut bytes = [0u8; BASE_HEADER_LEN];
        bytes[0..8].copy_from_slice(&BASE_MAGIC);
        bytes[8..12].copy_from_slice(&BASE_FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(BASE_HEADER_LEN as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.hash_version.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.proxy_version.to_le_bytes());
        bytes[28..32].copy_from_slice(&(BASE_RECORD_LEN as u32).to_le_bytes());
        bytes[32..40].copy_from_slice(&self.record_count.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.applied_seq.to_le_bytes());
        bytes[48..64].copy_from_slice(&self.store_id);
        bytes[64..96].copy_from_slice(&self.body_sha256);
        bytes
    }
}

fn read_base(path: &Path, expected_store_id: [u8; 16]) -> Result<BaseArray, BaseRejectReason> {
    let file = File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            BaseRejectReason::Missing
        } else {
            BaseRejectReason::Open(error.to_string())
        }
    })?;
    let file_len = file
        .metadata()
        .map_err(|error| BaseRejectReason::Open(error.to_string()))?
        .len();
    let mut reader = BufReader::new(file);
    let mut header = [0u8; BASE_HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_| BaseRejectReason::HeaderShort)?;
    if header[0..8] != BASE_MAGIC {
        return Err(BaseRejectReason::Magic);
    }
    if u32::from_le_bytes(header[8..12].try_into().unwrap()) != BASE_FORMAT_VERSION {
        return Err(BaseRejectReason::FormatVersion);
    }
    if u32::from_le_bytes(header[12..16].try_into().unwrap()) != BASE_HEADER_LEN as u32 {
        return Err(BaseRejectReason::HeaderLength);
    }
    if i64::from_le_bytes(header[16..24].try_into().unwrap()) != current_hash_version() {
        return Err(BaseRejectReason::HashVersion);
    }
    if u32::from_le_bytes(header[24..28].try_into().unwrap()) != dupe::PROXY_VERSION {
        return Err(BaseRejectReason::ProxyVersion);
    }
    if u32::from_le_bytes(header[28..32].try_into().unwrap()) != BASE_RECORD_LEN as u32 {
        return Err(BaseRejectReason::RecordLength);
    }
    let record_count = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let applied_seq = u64::from_le_bytes(header[40..48].try_into().unwrap());
    let mut store_id = [0u8; 16];
    store_id.copy_from_slice(&header[48..64]);
    if store_id != expected_store_id {
        return Err(BaseRejectReason::StoreId);
    }
    let expected_len = (BASE_HEADER_LEN as u64)
        .checked_add(
            record_count
                .checked_mul(BASE_RECORD_LEN as u64)
                .ok_or(BaseRejectReason::RecordCount)?,
        )
        .ok_or(BaseRejectReason::RecordCount)?;
    if expected_len != file_len {
        return Err(BaseRejectReason::RecordCount);
    }
    let count = usize::try_from(record_count).map_err(|_| BaseRejectReason::RecordCount)?;
    let mut records = Vec::with_capacity(count);
    let mut digest = Sha256::new();
    let mut buffer = Vec::with_capacity(IO_RECORDS * BASE_RECORD_LEN);
    let mut previous = 0u64;
    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(IO_RECORDS);
        buffer.resize(chunk * BASE_RECORD_LEN, 0);
        reader
            .read_exact(&mut buffer)
            .map_err(|_| BaseRejectReason::BodyShort)?;
        digest.update(&buffer);
        for bytes in buffer.chunks_exact(BASE_RECORD_LEN) {
            let item_id = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
            if item_id == 0 || item_id <= previous {
                return Err(BaseRejectReason::RecordOrder);
            }
            previous = item_id;
            let mut signature = [0u8; 32];
            signature.copy_from_slice(&bytes[8..40]);
            records.push(SearchRecord {
                item_id,
                signature,
                quality: bytes[40],
                revision: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
            });
        }
        remaining -= chunk;
    }
    let expected_checksum: [u8; 32] = header[64..96].try_into().unwrap();
    let actual_checksum: [u8; 32] = digest.finalize().into();
    if actual_checksum != expected_checksum {
        return Err(BaseRejectReason::Checksum);
    }
    Ok(BaseArray {
        records: records.into_boxed_slice(),
        store_id,
        applied_seq,
    })
}

fn write_base(path: &Path, base: &BaseArray) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "similar base has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("base directory create failed: {error}"))?;
    let temp = parent.join(format!(
        ".{BASE_FILE}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("base temp create failed: {error}"))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&[0; BASE_HEADER_LEN])
            .map_err(|error| format!("base header reserve failed: {error}"))?;
        let mut digest = Sha256::new();
        let mut buffer = Vec::with_capacity(IO_RECORDS * BASE_RECORD_LEN);
        for chunk in base.records.chunks(IO_RECORDS) {
            buffer.clear();
            for record in chunk {
                buffer.extend_from_slice(&encode_record(*record));
            }
            writer
                .write_all(&buffer)
                .map_err(|error| format!("base body write failed: {error}"))?;
            digest.update(&buffer);
        }
        let header = BaseHeader {
            hash_version: current_hash_version(),
            proxy_version: dupe::PROXY_VERSION,
            record_count: base.records.len() as u64,
            applied_seq: base.applied_seq,
            store_id: base.store_id,
            body_sha256: digest.finalize().into(),
        };
        writer
            .seek(SeekFrom::Start(0))
            .and_then(|_| writer.write_all(&header.encode()))
            .and_then(|_| writer.flush())
            .map_err(|error| format!("base header write failed: {error}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("base sync failed: {error}"))?;
        replace_file_atomic(&temp, path).map_err(|error| format!("base publish failed: {error}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn encode_record(record: SearchRecord) -> [u8; BASE_RECORD_LEN] {
    let mut bytes = [0u8; BASE_RECORD_LEN];
    bytes[0..8].copy_from_slice(&record.item_id.to_le_bytes());
    bytes[8..40].copy_from_slice(&record.signature);
    bytes[40] = record.quality;
    bytes[44..48].copy_from_slice(&record.revision.to_le_bytes());
    bytes
}

#[cfg(windows)]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;
    let temp = temp
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(temp.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(not(windows))]
fn replace_file_atomic(temp: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, destination)
}

fn db_error(error: rusqlite::Error) -> String {
    format!("similar.db: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(item_id: u64, marker: u8, revision: u32) -> SearchRecord {
        SearchRecord {
            item_id,
            signature: [marker; 32],
            quality: 50,
            revision,
        }
    }

    #[test]
    fn published_parts_are_replaced_without_mutation() {
        let base = BaseArray {
            records: vec![record(1, 1, 1)].into(),
            store_id: [2; 16],
            applied_seq: 1,
        };
        let first = Arc::new(SearchSnapshot::from_base(base));
        let batch = ItemChangeBatch {
            latest_seq: 2,
            first_available_seq: Some(2),
            changes: vec![crate::similar_db::ItemChange {
                seq: 2,
                item_id: 1,
                op: ItemChangeOp::Update,
                revision: Some(2),
                signature: Some([9; 32]),
                quality: Some(50),
            }],
        };
        let second = apply_change_batch(&first, batch).unwrap().unwrap();
        assert_eq!(first.base.records[0], record(1, 1, 1));
        assert!(first.delta.is_empty());
        assert!(!first.base_record_is_superseded(0));
        assert_eq!(second.delta[0].record.unwrap(), record(1, 9, 2));
        assert!(second.base_record_is_superseded(0));
    }

    #[test]
    fn delete_hides_base_record() {
        let base = BaseArray {
            records: vec![record(1, 1, 1)].into(),
            store_id: [2; 16],
            applied_seq: 1,
        };
        let snapshot = SearchSnapshot::from_base(base);
        let batch = ItemChangeBatch {
            latest_seq: 2,
            first_available_seq: Some(2),
            changes: vec![crate::similar_db::ItemChange {
                seq: 2,
                item_id: 1,
                op: ItemChangeOp::Delete,
                revision: None,
                signature: None,
                quality: None,
            }],
        };
        let updated = apply_change_batch(&snapshot, batch).unwrap().unwrap();
        assert!(updated.base_record_is_superseded(0));
        assert!(updated.delta[0].record.is_none());
        assert!(compacted_base(&updated).records.is_empty());
    }

    #[test]
    fn missing_history_is_detected() {
        let base = BaseArray {
            records: Vec::new().into_boxed_slice(),
            store_id: [2; 16],
            applied_seq: 3,
        };
        let snapshot = SearchSnapshot::from_base(base);
        let batch = ItemChangeBatch {
            latest_seq: 5,
            first_available_seq: Some(5),
            changes: vec![crate::similar_db::ItemChange {
                seq: 5,
                item_id: 1,
                op: ItemChangeOp::Add,
                revision: Some(1),
                signature: Some([1; 32]),
                quality: Some(1),
            }],
        };
        assert!(matches!(
            apply_change_batch(&snapshot, batch),
            Err(MissingHistory)
        ));
    }

    #[test]
    fn base_round_trips_and_stale_content_is_still_accepted() {
        let temp = tempfile::tempdir().unwrap();
        let path = base_path(temp.path());
        let base = BaseArray {
            records: vec![record(1, 3, 4), record(8, 9, 2)].into(),
            store_id: [7; 16],
            applied_seq: 12,
        };
        write_base(&path, &base).unwrap();

        let loaded = read_base(&path, [7; 16]).unwrap();
        assert_eq!(loaded.records.as_ref(), base.records.as_ref());
        assert_eq!(loaded.applied_seq, 12);
        assert_eq!(loaded.store_id, [7; 16]);
    }

    #[test]
    fn legacy_rowid_sidecar_is_discarded() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join(LEGACY_SIDECAR_FILE);
        std::fs::write(&legacy, b"old rowid format").unwrap();
        assert!(retire_legacy_sidecar(temp.path()).unwrap());
        assert!(!legacy.exists());
        assert!(!retire_legacy_sidecar(temp.path()).unwrap());
    }

    fn fallback_after_header_or_body_mutation(mutate: impl FnOnce(&mut [u8])) -> BaseRejectReason {
        let temp = tempfile::tempdir().unwrap();
        let path = base_path(temp.path());
        let db = SimilarDb::open_at(&SimilarDb::db_path_at(temp.path())).unwrap();
        let store_id = db.search_store_id().unwrap();
        let base = BaseArray {
            records: vec![record(1, 3, 4)].into(),
            store_id,
            applied_seq: 12,
        };
        write_base(&path, &base).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        mutate(&mut bytes);
        std::fs::write(&path, bytes).unwrap();
        let loaded = load_or_rebuild(&db, &path).unwrap();
        assert_eq!(loaded.source, LoadSource::Sqlite);
        assert!(loaded.snapshot.base.records.is_empty());
        loaded.rejected.unwrap()
    }

    #[test]
    fn base_rejection_reasons_are_distinct() {
        assert_eq!(
            fallback_after_header_or_body_mutation(|bytes| bytes[48] ^= 1),
            BaseRejectReason::StoreId
        );
        assert_eq!(
            fallback_after_header_or_body_mutation(|bytes| bytes[16] ^= 1),
            BaseRejectReason::HashVersion
        );
        assert_eq!(
            fallback_after_header_or_body_mutation(|bytes| bytes[24] ^= 1),
            BaseRejectReason::ProxyVersion
        );
        assert_eq!(
            fallback_after_header_or_body_mutation(|bytes| {
                bytes[32..40].copy_from_slice(&2u64.to_le_bytes());
            }),
            BaseRejectReason::RecordCount
        );
        assert_eq!(
            fallback_after_header_or_body_mutation(|bytes| bytes[BASE_HEADER_LEN + 8] ^= 1),
            BaseRejectReason::Checksum
        );
    }

    #[test]
    fn compaction_keeps_changes_that_arrived_after_its_cutoff() {
        let old_base = BaseArray {
            records: vec![record(1, 1, 1)].into(),
            store_id: [2; 16],
            applied_seq: 1,
        };
        let first = SearchSnapshot::from_base(old_base);
        let through_two = apply_change_batch(
            &first,
            ItemChangeBatch {
                latest_seq: 2,
                first_available_seq: Some(2),
                changes: vec![crate::similar_db::ItemChange {
                    seq: 2,
                    item_id: 2,
                    op: ItemChangeOp::Add,
                    revision: Some(1),
                    signature: Some([2; 32]),
                    quality: Some(50),
                }],
            },
        )
        .unwrap()
        .unwrap();
        let compacted_at_two = compacted_base(&through_two);
        let through_three = apply_change_batch(
            &through_two,
            ItemChangeBatch {
                latest_seq: 3,
                first_available_seq: Some(3),
                changes: vec![crate::similar_db::ItemChange {
                    seq: 3,
                    item_id: 3,
                    op: ItemChangeOp::Add,
                    revision: Some(1),
                    signature: Some([3; 32]),
                    quality: Some(50),
                }],
            },
        )
        .unwrap()
        .unwrap();

        let published = snapshot_on_new_base(&through_three, Arc::new(compacted_at_two)).unwrap();
        assert_eq!(published.base.applied_seq, 2);
        assert_eq!(published.applied_seq, 3);
        assert_eq!(published.delta.len(), 1);
        assert_eq!(published.delta[0].item_id, 3);
        assert_eq!(compacted_base(&published).records.len(), 3);
    }

    #[test]
    fn older_compaction_cannot_replace_a_newer_base() {
        let current = SearchSnapshot::from_base(BaseArray {
            records: vec![record(1, 1, 2)].into(),
            store_id: [2; 16],
            applied_seq: 8,
        });
        let stale = Arc::new(BaseArray {
            records: vec![record(1, 1, 1)].into(),
            store_id: [2; 16],
            applied_seq: 7,
        });
        assert!(matches!(
            snapshot_on_new_base(&current, stale),
            Err(MissingHistory)
        ));
    }
}
