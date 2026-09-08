//! 「別バージョンの発見」用の独立 SQLite ストア。
//!
//! `item` / `container` は検索に公開済みの世代だけを持つ。再索引中のページは
//! staging 表へ書き、最後の transaction でだけ公開世代と入れ替える。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

const SCHEMA_VERSION: i64 = 3;
pub const HASH_ALGORITHM_VERSION: u32 = 1;

/// `page_index` がどの並べ方で振られているか。
///
/// 1 = 閲覧側と同じファイル名順。0 は書庫のエントリ順で振られた古い索引で、本単位の
/// 対応付けが崩れる。上げたときは [`SimilarDb::renumber_container_pages`] で振り直す。
/// 署名は変わらないので再デコードは要らない。
pub const PAGE_ORDER_VERSION: i64 = 1;

pub const fn current_hash_version() -> i64 {
    hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION)
}

pub const fn hash_version(algorithm_version: u32, proxy_version: u32) -> i64 {
    ((algorithm_version as i64) << 32) | proxy_version as i64
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Image = 0,
    ZipPage = 1,
    PdfPage = 2,
}

impl ItemKind {
    fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::Image),
            1 => Ok(Self::ZipPage),
            2 => Ok(Self::PdfPage),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    ImageFolder = 0,
    Zip = 1,
    Pdf = 2,
}

impl ContainerKind {
    fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::ImageFolder),
            1 => Ok(Self::Zip),
            2 => Ok(Self::Pdf),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanState {
    Building = 0,
    Complete = 1,
    Failed = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredItem {
    pub item_key: String,
    pub kind: ItemKind,
    pub container_key: Option<String>,
    pub page_index: Option<u32>,
    pub mtime: i64,
    pub file_size: i64,
    pub hash_version: i64,
    pub pdq256: [u8; 32],
    pub quality: u8,
    pub width: u32,
    pub height: u32,
    pub format: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredContainer {
    pub container_key: String,
    pub kind: ContainerKind,
    pub page_count: Option<u32>,
    pub scan_state: ScanState,
    pub generation: u64,
    pub mtime: i64,
    pub file_size: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRow {
    pub item_id: u64,
    pub revision: u32,
    pub item: StoredItem,
}

#[derive(Debug)]
pub struct BaseSearchRows {
    pub store_id: [u8; 16],
    pub applied_seq: u64,
    pub records: Vec<BaseSearchRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseSearchRow {
    pub item_id: u64,
    pub signature: [u8; 32],
    pub quality: u8,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemChangeOp {
    Add,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemChange {
    pub seq: u64,
    pub item_id: u64,
    pub op: ItemChangeOp,
    pub revision: Option<u32>,
    pub signature: Option<[u8; 32]>,
    pub quality: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ItemChangeBatch {
    pub latest_seq: u64,
    pub first_available_seq: Option<u64>,
    pub changes: Vec<ItemChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateIdentity {
    pub item_id: u64,
    pub revision: u32,
    pub signature: [u8; 32],
}

#[derive(Debug)]
pub struct VerifiedCandidates {
    pub origin: SearchRow,
    pub candidates: Vec<SearchRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletedIndexStats {
    pub password_required_pdfs: u64,
    pub corrupt_containers: u64,
    pub zero_page_containers: u64,
    pub decode_failures: u64,
    pub io_failures: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredIndexSummary {
    pub completed_at_unix_secs: i64,
    pub registered_items: u64,
    pub stats: CompletedIndexStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Missing,
    Stale,
    Current,
}

pub struct SimilarDb {
    conn: Mutex<Connection>,
}

/// WAL への変換だけを直列化する。
///
/// `PRAGMA journal_mode=WAL` は排他ロックへ昇格するため、同じ fresh DB を複数接続が同時に
/// 開くと `busy_timeout` では待たず `SQLITE_BUSY` を返す。mode は DB に永続するので、まず
/// 読み取りだけで確認し、変換が必要な接続だけを process 内で直列化する。mutex を待つ前の
/// statement は `journal_mode_is_wal` の return で解放済みであり、mutex 内でも再確認する。
fn ensure_wal_journal(conn: &Connection) -> rusqlite::Result<()> {
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }

    static CONVERT: Mutex<()> = Mutex::new(());
    let _serialized = CONVERT.lock().unwrap_or_else(|error| error.into_inner());
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }

    match conn.execute_batch("PRAGMA journal_mode=WAL;") {
        Ok(()) => Ok(()),
        Err(error) if journal_mode_is_wal(conn).unwrap_or(false) => {
            // 別 process が先に変換を終えた場合も目的は達成済み。
            let _ = error;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn journal_mode_is_wal(conn: &Connection) -> rusqlite::Result<bool> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok(mode.eq_ignore_ascii_case("wal"))
}

impl SimilarDb {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut conn = Connection::open(path)?;
        // 既定の 5 秒では v1 からの一括移行を待ち切れない。実店 4,628,611 行の移行は 38.9 秒
        // かかり、その間に別 worker がこの店を開くと `init_schema` の DDL が書き込みロックを
        // 取れずに失敗する。開くのは常に worker なので、待つことで UI は止まらない。移行後の
        // 書き込みはどれも短いため、この時間が実際に使われるのは最初の一度だけになる。
        conn.busy_timeout(std::time::Duration::from_secs(180))?;
        // timeout の位置だけでは journal mode の昇格競合は待てない。変換要否を読んでから、
        // 必要な場合だけ上の専用 owner へ渡す。
        ensure_wal_journal(&conn)?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        init_schema(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        init_schema(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("similar.db")
    }

    pub fn db_path_at(data_dir: &Path) -> PathBuf {
        data_dir.join("similar.db")
    }

    /// 前回停止時に公開されなかった世代だけを掃除する。
    pub fn cleanup_incomplete(&self) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let staged = transaction.execute("DELETE FROM item_build", [])?;
        transaction.execute("DELETE FROM container_build", [])?;
        let unpublished = load_items_for_container_state(&transaction, ScanState::Building)?;
        for row in &unpublished {
            delete_item_with_change(&transaction, row)?;
        }
        let containers = transaction.execute(
            "DELETE FROM container WHERE scan_state = ?1",
            [ScanState::Building as i64],
        )?;
        transaction.commit()?;
        Ok(staged + containers + unpublished.len())
    }

    pub fn item_freshness(
        &self,
        item_key: &str,
        mtime: i64,
        file_size: i64,
        hash_version: i64,
    ) -> rusqlite::Result<Freshness> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let found = conn
            .query_row(
                "SELECT mtime, file_size, hash_version FROM item WHERE item_key = ?1",
                [item_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(match found {
            None => Freshness::Missing,
            Some((stored_mtime, stored_size, stored_version))
                if stored_mtime == mtime
                    && stored_size == file_size
                    && stored_version == hash_version =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
        })
    }

    pub fn container_freshness(
        &self,
        container_key: &str,
        mtime: i64,
        file_size: i64,
        page_count: u32,
        hash_version: i64,
    ) -> rusqlite::Result<Freshness> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let found = conn
            .query_row(
                "SELECT mtime, file_size, page_count, scan_state,
                        NOT EXISTS(
                          SELECT 1 FROM item
                          WHERE container_key = ?1 AND hash_version != ?2
                        )
                   FROM container WHERE container_key = ?1",
                params![container_key, hash_version],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?;
        Ok(match found {
            None => Freshness::Missing,
            Some((stored_mtime, stored_size, stored_pages, state, versions_current))
                if stored_mtime == mtime
                    && stored_size == file_size
                    && stored_pages == Some(i64::from(page_count))
                    && state == ScanState::Complete as i64
                    && versions_current =>
            {
                Freshness::Current
            }
            Some(_) => Freshness::Stale,
        })
    }

    /// 単独画像を差分 upsert する。戻り値は再計算した行なら true。
    pub fn upsert_loose_item(&self, item: &StoredItem) -> rusqlite::Result<bool> {
        debug_assert!(item.container_key.is_none());
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let existing = load_item_raw(&transaction, &item.item_key)?;
        if existing.as_ref().is_some_and(|existing| {
            existing.item.container_key.is_none()
                && existing.item.page_index == item.page_index
                && existing.item.mtime == item.mtime
                && existing.item.file_size == item.file_size
                && existing.item.hash_version == item.hash_version
        }) {
            return Ok(false);
        }
        publish_item(&transaction, existing.as_ref(), item)?;
        transaction.commit()?;
        Ok(true)
    }

    /// サムネイル元 buffer から得た署名を、公開索引とは別に再利用用として置く。
    pub fn put_prefill(&self, item: &StoredItem) -> rusqlite::Result<()> {
        self.put_prefill_if(item, || true).map(|_| ())
    }

    /// DB write ownershipを先に取得してから、呼び出し元の最新scope判定を実行する。
    ///
    /// predicateは短時間で完了し、外部lockのguardを返値より先に解放すること。これにより
    /// scope更新側はDBを待たず、scopeを外れた行は後続purgeより後に復活しない。
    pub(crate) fn put_prefill_if(
        &self,
        item: &StoredItem,
        should_insert: impl FnOnce() -> bool,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if !should_insert() {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO item_prefill
             (item_key, mtime, file_size, hash_version, pdq256, quality, width, height, format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(item_key) DO UPDATE SET
               mtime=excluded.mtime, file_size=excluded.file_size,
               hash_version=excluded.hash_version, pdq256=excluded.pdq256,
               quality=excluded.quality, width=excluded.width,
               height=excluded.height, format=excluded.format",
            params![
                item.item_key,
                item.mtime,
                item.file_size,
                item.hash_version,
                item.pdq256.as_slice(),
                i64::from(item.quality),
                i64::from(item.width),
                i64::from(item.height),
                item.format,
            ],
        )?;
        Ok(true)
    }

    pub fn load_prefill(
        &self,
        item_key: &str,
        mtime: i64,
        file_size: i64,
        hash_version: i64,
    ) -> rusqlite::Result<Option<StoredItem>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT pdq256, quality, width, height, format
             FROM item_prefill
             WHERE item_key = ?1 AND mtime = ?2 AND file_size = ?3 AND hash_version = ?4",
            params![item_key, mtime, file_size, hash_version],
            |row| {
                let bytes = row.get::<_, Vec<u8>>(0)?;
                let pdq256: [u8; 32] = bytes.try_into().map_err(|value: Vec<u8>| {
                    rusqlite::Error::FromSqlConversionFailure(
                        value.len(),
                        rusqlite::types::Type::Blob,
                        "pdq256 must contain 32 bytes".into(),
                    )
                })?;
                let quality = row.get::<_, i64>(1)?;
                Ok(StoredItem {
                    item_key: item_key.to_owned(),
                    kind: ItemKind::Image,
                    container_key: None,
                    page_index: None,
                    mtime,
                    file_size,
                    hash_version,
                    pdq256,
                    quality: u8::try_from(quality)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, quality))?,
                    width: i64_to_u32(row.get(2)?, 2)?,
                    height: i64_to_u32(row.get(3)?, 3)?,
                    format: row.get(4)?,
                })
            },
        )
        .optional()
    }

    /// 新しい非公開世代を開始する。既存の Complete 世代は変更しない。
    pub fn begin_container_build(
        &self,
        container_key: &str,
        kind: ContainerKind,
        page_count: u32,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<u64> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let active_generation = transaction
            .query_row(
                "SELECT generation FROM container WHERE container_key = ?1",
                [container_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let generation = active_generation.saturating_add(1).max(1);
        transaction.execute(
            "DELETE FROM item_build WHERE container_key = ?1",
            [container_key],
        )?;
        transaction.execute(
            "INSERT INTO container_build
             (container_key, kind, page_count, scan_state, generation, mtime, file_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(container_key) DO UPDATE SET
               kind=excluded.kind, page_count=excluded.page_count,
               scan_state=excluded.scan_state, generation=excluded.generation,
               mtime=excluded.mtime, file_size=excluded.file_size",
            params![
                container_key,
                kind as i64,
                i64::from(page_count),
                ScanState::Building as i64,
                generation,
                mtime,
                file_size,
            ],
        )?;
        // 初回だけは公開 table にも Building を記録する。再索引時は既存 Complete を保つ。
        transaction.execute(
            "INSERT OR IGNORE INTO container
             (container_key, kind, page_count, scan_state, generation, mtime, file_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                container_key,
                kind as i64,
                i64::from(page_count),
                ScanState::Building as i64,
                generation,
                mtime,
                file_size,
            ],
        )?;
        transaction.commit()?;
        u64::try_from(generation)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, generation))
    }

    pub fn stage_item(&self, generation: u64, item: &StoredItem) -> rusqlite::Result<()> {
        let Some(container_key) = item.container_key.as_deref() else {
            return Err(rusqlite::Error::InvalidParameterName(
                "container_key".to_owned(),
            ));
        };
        let generation_i64 = i64::try_from(generation).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("generation exceeds SQLite INTEGER".into())
        })?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO item_build
             (item_key, kind, container_key, page_index, mtime, file_size, hash_version,
              pdq256, quality, width, height, format, generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params_from_iter(item_params(item, Some(generation_i64))),
        )?;
        // 世代を取り違えた stage を黙って受理しない。
        let building = conn.query_row(
            "SELECT COUNT(*) FROM container_build
             WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?3",
            params![container_key, generation, ScanState::Building as i64],
            |row| row.get::<_, i64>(0),
        )?;
        if building != 1 {
            conn.execute(
                "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
                params![container_key, generation],
            )?;
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }

    /// staging 行数を確認し、公開世代を 1 transaction で置換する。
    pub fn complete_container(&self, container_key: &str, generation: u64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let expected = building_page_count(&transaction, container_key, generation)?;
        let actual = transaction.query_row(
            "SELECT COUNT(*) FROM item_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
            |row| row.get::<_, i64>(0),
        )?;
        if actual != expected {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let staged = load_staged_items(&transaction, container_key, generation)?;
        let staged_keys = staged
            .iter()
            .map(|item| item.item_key.as_str())
            .collect::<HashSet<_>>();
        let old_container_rows = load_items_for_container_raw(&transaction, container_key)?;
        for old in old_container_rows {
            if !staged_keys.contains(old.item.item_key.as_str()) {
                delete_item_with_change(&transaction, &old)?;
            }
        }
        for item in &staged {
            let existing = load_item_raw(&transaction, &item.item_key)?;
            publish_item(&transaction, existing.as_ref(), item)?;
        }
        transaction.execute(
            "INSERT INTO container
             (container_key, kind, page_count, scan_state, generation, mtime, file_size)
             SELECT container_key, kind, page_count, ?3, generation, mtime, file_size
             FROM container_build WHERE container_key = ?1 AND generation = ?2
             ON CONFLICT(container_key) DO UPDATE SET
               kind=excluded.kind, page_count=excluded.page_count,
               scan_state=excluded.scan_state, generation=excluded.generation,
               mtime=excluded.mtime, file_size=excluded.file_size",
            params![container_key, generation, ScanState::Complete as i64],
        )?;
        transaction.execute(
            "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.execute(
            "DELETE FROM container_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.commit()
    }

    /// 失敗した新規 container は Failed として残す。再索引なら旧 Complete を保つ。
    pub fn fail_container(&self, container_key: &str, generation: u64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        transaction.execute(
            "UPDATE container SET scan_state = ?3
             WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?4",
            params![
                container_key,
                generation,
                ScanState::Failed as i64,
                ScanState::Building as i64
            ],
        )?;
        transaction.execute(
            "DELETE FROM item_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.execute(
            "DELETE FROM container_build WHERE container_key = ?1 AND generation = ?2",
            params![container_key, generation],
        )?;
        transaction.commit()
    }

    /// 列挙前に失敗した container を型付きで記録する。既存 Complete は保持する。
    pub fn record_container_failure(
        &self,
        container_key: &str,
        kind: ContainerKind,
        mtime: i64,
        file_size: i64,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let current = conn
            .query_row(
                "SELECT scan_state, generation FROM container WHERE container_key = ?1",
                [container_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if current.is_some_and(|(state, _)| state == ScanState::Complete as i64) {
            return Ok(());
        }
        let generation = current.map_or(1, |(_, generation)| generation.saturating_add(1));
        conn.execute(
            "INSERT INTO container
             (container_key, kind, page_count, scan_state, generation, mtime, file_size)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)
             ON CONFLICT(container_key) DO UPDATE SET
               kind=excluded.kind, page_count=NULL, scan_state=excluded.scan_state,
               generation=excluded.generation, mtime=excluded.mtime,
               file_size=excluded.file_size",
            params![
                container_key,
                kind as i64,
                ScanState::Failed as i64,
                generation,
                mtime,
                file_size
            ],
        )?;
        Ok(())
    }

    pub fn load_search_rows(&self, hash_version: i64) -> rusqlite::Result<Vec<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = conn.prepare(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.hash_version = ?1
               AND (i.container_key IS NULL OR c.scan_state = ?2)
             ORDER BY i.item_id",
        )?;
        statement
            .query_map(
                params![hash_version, ScanState::Complete as i64],
                row_to_search_row,
            )?
            .collect()
    }

    /// SQLite の一つの read snapshot から、不変 base とその適用済み change seq を作る。
    pub fn load_base_search_rows(&self, hash_version: i64) -> rusqlite::Result<BaseSearchRows> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let store_id = search_store_id(&transaction)?;
        let applied_seq = latest_change_seq(&transaction)?;
        let count_i64 = transaction.query_row(
            "SELECT COUNT(*) FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.hash_version = ?1
               AND (i.container_key IS NULL OR c.scan_state = ?2)",
            params![hash_version, ScanState::Complete as i64],
            |row| row.get::<_, i64>(0),
        )?;
        let count = usize::try_from(count_i64)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count_i64))?;
        let mut records = Vec::with_capacity(count);
        let mut statement = transaction.prepare(
            "SELECT i.item_id, i.pdq256, i.quality, i.revision
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.hash_version = ?1
               AND (i.container_key IS NULL OR c.scan_state = ?2)
             ORDER BY i.item_id",
        )?;
        let mut rows = statement.query(params![hash_version, ScanState::Complete as i64])?;
        while let Some(row) = rows.next()? {
            let item_id = i64_to_u64(row.get(0)?, 0)?;
            let pdq = row.get_ref(1)?.as_blob()?;
            let signature: [u8; 32] = pdq.try_into().map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    pdq.len(),
                    rusqlite::types::Type::Blob,
                    "pdq256 must contain 32 bytes".into(),
                )
            })?;
            let quality_i64 = row.get::<_, i64>(2)?;
            records.push(BaseSearchRow {
                item_id,
                signature,
                quality: u8::try_from(quality_i64)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, quality_i64))?,
                revision: i64_to_u32(row.get(3)?, 3)?,
            });
        }
        drop(rows);
        drop(statement);
        transaction.commit()?;
        Ok(BaseSearchRows {
            store_id,
            applied_seq,
            records,
        })
    }

    pub fn search_store_id(&self) -> rusqlite::Result<[u8; 16]> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        search_store_id(&conn)
    }

    pub fn load_item_changes_after(&self, after_seq: u64) -> rusqlite::Result<ItemChangeBatch> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let latest_seq = latest_change_seq(&transaction)?;
        let first_available_seq = transaction
            .query_row("SELECT MIN(seq) FROM item_change", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?
            .map(|value| i64_to_u64(value, 0))
            .transpose()?;
        let after_i64 = i64::try_from(after_seq).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("change seq exceeds SQLite INTEGER".into())
        })?;
        let mut statement = transaction.prepare(
            "SELECT seq, item_id, op, revision, pdq256, quality
             FROM item_change WHERE seq > ?1 ORDER BY seq",
        )?;
        let changes = statement
            .query_map([after_i64], row_to_item_change)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(ItemChangeBatch {
            latest_seq,
            first_available_seq,
            changes,
        })
    }

    pub fn prune_item_changes_through(&self, through_seq: u64) -> rusqlite::Result<usize> {
        let through = i64::try_from(through_seq).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("change seq exceeds SQLite INTEGER".into())
        })?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("DELETE FROM item_change WHERE seq <= ?1", [through])
    }

    /// origin と全候補を同じ SQLite read snapshot で検証する。
    pub fn verify_item_candidates(
        &self,
        item_key: &str,
        hash_version: i64,
        candidates: &[CandidateIdentity],
    ) -> rusqlite::Result<Option<VerifiedCandidates>> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // 読み取り専用。ここで書くなら `write_transaction` に替えること。
        let transaction = conn.transaction()?;
        let Some(origin) = load_search_item_by_key(&transaction, item_key, hash_version)? else {
            transaction.commit()?;
            return Ok(None);
        };
        let mut verified = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let Some(row) = load_search_item_by_id(&transaction, candidate.item_id, hash_version)?
            else {
                continue;
            };
            if row.revision == candidate.revision && row.item.pdq256 == candidate.signature {
                verified.push(row);
            }
        }
        transaction.commit()?;
        Ok(Some(VerifiedCandidates {
            origin,
            candidates: verified,
        }))
    }

    /// 完走した索引ジョブの表示用集計を、公開済み行数と同じ transaction で記録する。
    /// キャンセル・失敗したジョブからは呼ばないため、「最終更新」は完走時だけ進む。
    pub fn record_completed_index(
        &self,
        hash_version: i64,
        completed_at_unix_secs: i64,
        stats: CompletedIndexStats,
    ) -> rusqlite::Result<StoredIndexSummary> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let registered_items = transaction.query_row(
            "SELECT COUNT(*) FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.hash_version = ?1
               AND (i.container_key IS NULL OR c.scan_state = ?2)",
            params![hash_version, ScanState::Complete as i64],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "INSERT INTO index_run
             (singleton, hash_version, completed_at_unix_secs, registered_items,
              password_required_pdfs, corrupt_containers, zero_page_containers,
              decode_failures, io_failures)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(singleton) DO UPDATE SET
               hash_version=excluded.hash_version,
               completed_at_unix_secs=excluded.completed_at_unix_secs,
               registered_items=excluded.registered_items,
               password_required_pdfs=excluded.password_required_pdfs,
               corrupt_containers=excluded.corrupt_containers,
               zero_page_containers=excluded.zero_page_containers,
               decode_failures=excluded.decode_failures,
               io_failures=excluded.io_failures",
            params![
                hash_version,
                completed_at_unix_secs,
                registered_items,
                i64::try_from(stats.password_required_pdfs).unwrap_or(i64::MAX),
                i64::try_from(stats.corrupt_containers).unwrap_or(i64::MAX),
                i64::try_from(stats.zero_page_containers).unwrap_or(i64::MAX),
                i64::try_from(stats.decode_failures).unwrap_or(i64::MAX),
                i64::try_from(stats.io_failures).unwrap_or(i64::MAX),
            ],
        )?;
        transaction.commit()?;
        Ok(StoredIndexSummary {
            completed_at_unix_secs,
            registered_items: u64::try_from(registered_items).unwrap_or(0),
            stats,
        })
    }

    pub fn load_index_summary(
        &self,
        hash_version: i64,
    ) -> rusqlite::Result<Option<StoredIndexSummary>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT completed_at_unix_secs, registered_items,
                    password_required_pdfs, corrupt_containers, zero_page_containers,
                    decode_failures, io_failures
             FROM index_run WHERE singleton = 1 AND hash_version = ?1",
            [hash_version],
            |row| {
                Ok(StoredIndexSummary {
                    completed_at_unix_secs: row.get(0)?,
                    registered_items: i64_to_u64(row.get(1)?, 1)?,
                    stats: CompletedIndexStats {
                        password_required_pdfs: i64_to_u64(row.get(2)?, 2)?,
                        corrupt_containers: i64_to_u64(row.get(3)?, 3)?,
                        zero_page_containers: i64_to_u64(row.get(4)?, 4)?,
                        decode_failures: i64_to_u64(row.get(5)?, 5)?,
                        io_failures: i64_to_u64(row.get(6)?, 6)?,
                    },
                })
            },
        )
        .optional()
    }

    pub fn load_item(
        &self,
        item_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Option<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.item_key = ?1 AND i.hash_version = ?2
               AND (i.container_key IS NULL OR c.scan_state = ?3)",
            params![item_key, hash_version, ScanState::Complete as i64],
            row_to_search_row,
        )
        .optional()
    }

    pub fn load_item_by_id(
        &self,
        item_id: u64,
        hash_version: i64,
    ) -> rusqlite::Result<Option<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let item_id = i64::try_from(item_id).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure("item_id exceeds SQLite INTEGER".into())
        })?;
        conn.query_row(
            "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
                    i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
                    i.width, i.height, i.format
             FROM item i
             LEFT JOIN container c ON c.container_key = i.container_key
             WHERE i.item_id = ?1 AND i.hash_version = ?2
               AND (i.container_key IS NULL OR c.scan_state = ?3)",
            params![item_id, hash_version, ScanState::Complete as i64],
            row_to_search_row,
        )
        .optional()
    }

    pub fn load_complete_containers(&self) -> rusqlite::Result<Vec<StoredContainer>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = conn.prepare(
            "SELECT container_key, kind, page_count, scan_state, generation, mtime, file_size
             FROM container WHERE scan_state = ?1 ORDER BY container_key",
        )?;
        statement
            .query_map([ScanState::Complete as i64], |row| {
                Ok(StoredContainer {
                    container_key: row.get(0)?,
                    kind: ContainerKind::from_i64(row.get(1)?)?,
                    page_count: row
                        .get::<_, Option<i64>>(2)?
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, -1))?,
                    scan_state: ScanState::Complete,
                    generation: i64_to_u64(row.get(4)?, 4)?,
                    mtime: row.get(5)?,
                    file_size: row.get(6)?,
                })
            })?
            .collect()
    }

    pub fn item_keys_for_container(&self, container_key: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement =
            conn.prepare("SELECT item_key FROM item WHERE container_key = ?1 ORDER BY page_index")?;
        statement
            .query_map([container_key], |row| row.get(0))?
            .collect()
    }

    pub fn page_order_version(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT page_order_version FROM search_content_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
    }

    /// 保存済みのページを、呼び出し側の並べ方で振り直す。
    ///
    /// 署名も mtime も触らないので**再デコードは起きない**。並べ方の判断はこの層では
    /// 行わず、比較関数を受け取る。返り値は振り直したコンテナの数。
    pub fn renumber_container_pages(
        &self,
        page_order_version: i64,
        order: impl Fn(&str, &str) -> std::cmp::Ordering,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let containers = {
            let mut statement = transaction.prepare(
                "SELECT container_key FROM container WHERE kind = ?1 AND scan_state = ?2",
            )?;
            statement
                .query_map(
                    params![ContainerKind::Zip as i64, ScanState::Complete as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut renumbered = 0usize;
        for container_key in containers {
            let mut pages = {
                let mut statement = transaction
                    .prepare("SELECT item_id, item_key FROM item WHERE container_key = ?1")?;
                statement
                    .query_map([&container_key], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let prefix_len = container_key.len() + 1;
            pages.sort_by(|left, right| {
                let left = left.1.get(prefix_len..).unwrap_or(&left.1);
                let right = right.1.get(prefix_len..).unwrap_or(&right.1);
                order(left, right)
            });
            for (page_index, (item_id, _)) in pages.iter().enumerate() {
                transaction.execute(
                    "UPDATE item SET page_index = ?1 WHERE item_id = ?2",
                    params![page_index as i64, item_id],
                )?;
            }
            renumbered += 1;
        }
        transaction.execute(
            "UPDATE search_content_state SET page_order_version = ?1 WHERE singleton = 1",
            [page_order_version],
        )?;
        transaction.commit()?;
        Ok(renumbered)
    }

    /// 公開済みコンテナ 1 冊分のページを、ページ順で読む。
    ///
    /// 本単位の照会はこれと `resolve_pages_by_item_id` の 2 つだけで identity を得る。
    /// 全行を読み込んだ在メモリ表は作らない (4.6M 行で 7.7 GB / 24 秒かかった)。
    pub fn load_book_pages(
        &self,
        container_key: &str,
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = conn.prepare(&format!(
            "{SEARCH_ROW_SELECT} JOIN container c ON c.container_key = i.container_key
             WHERE i.container_key = ?1 AND i.hash_version = ?2 AND c.scan_state = ?3
             ORDER BY i.page_index"
        ))?;
        statement
            .query_map(
                params![container_key, hash_version, ScanState::Complete as i64],
                row_to_search_row,
            )?
            .collect()
    }

    /// 配列が提案した item_id 群を、一つの読み取り snapshot で解決する。
    ///
    /// 単体画像の照会と同じ契約で、公開済み Complete 世代と `hash_version` の一致を SQL 側で
    /// 強制する。署名が配列と食い違う候補を捨てるのは呼び出し側の責任。
    pub fn resolve_pages_by_item_id(
        &self,
        item_ids: &[u64],
        hash_version: i64,
    ) -> rusqlite::Result<Vec<SearchRow>> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = conn.transaction()?;
        let mut rows = Vec::with_capacity(item_ids.len());
        for &item_id in item_ids {
            if let Some(row) = load_search_item_by_id(&transaction, item_id, hash_version)? {
                rows.push(row);
            }
        }
        transaction.commit()?;
        Ok(rows)
    }

    /// 完走した、有効な favorites 全体の snapshot に存在しなかった公開行を削除する。
    pub fn prune_except_seen(
        &self,
        seen_items: &HashSet<String>,
        seen_containers: &HashSet<String>,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let item_keys = {
            let mut statement = transaction.prepare("SELECT item_key FROM item")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let container_keys = {
            let mut statement = transaction.prepare("SELECT container_key FROM container")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut removed = 0;
        for key in item_keys {
            if !seen_items.contains(&key) {
                if let Some(row) = load_item_raw(&transaction, &key)? {
                    delete_item_with_change(&transaction, &row)?;
                    removed += 1;
                }
            }
        }
        for key in container_keys {
            if !seen_containers.contains(&key) {
                for row in load_items_for_container_raw(&transaction, &key)? {
                    delete_item_with_change(&transaction, &row)?;
                    removed += 1;
                }
                removed += transaction
                    .execute("DELETE FROM container WHERE container_key = ?1", [&key])?;
            }
        }
        transaction.commit()?;
        Ok(removed)
    }

    /// OFF になった favorite 配下の索引データを、次の全走査の完走を待たずに削除する。
    ///
    /// favorite は重なり得るため、`keep_roots` 配下でもあるキーは残す。公開済み世代、
    /// 構築途中の世代、prefill を同じ transaction で掃除し、公開件数の集計も追従させる。
    pub fn purge_roots_except(
        &self,
        purge_roots: &[String],
        keep_roots: &[String],
    ) -> rusqlite::Result<usize> {
        if purge_roots.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let transaction = write_transaction(&mut conn)?;
        let should_purge =
            |key: &str| key_is_under_any(key, purge_roots) && !key_is_under_any(key, keep_roots);

        let item_keys = query_string_column(&transaction, "SELECT item_key FROM item")?;
        let container_keys =
            query_string_column(&transaction, "SELECT container_key FROM container")?;
        let build_item_keys = query_string_column(&transaction, "SELECT item_key FROM item_build")?;
        let build_container_keys =
            query_string_column(&transaction, "SELECT container_key FROM container_build")?;
        let prefill_keys = query_string_column(&transaction, "SELECT item_key FROM item_prefill")?;

        let mut removed = 0;
        for key in item_keys.into_iter().filter(|key| should_purge(key)) {
            if let Some(row) = load_item_raw(&transaction, &key)? {
                delete_item_with_change(&transaction, &row)?;
                removed += 1;
            }
        }
        for key in container_keys.into_iter().filter(|key| should_purge(key)) {
            for row in load_items_for_container_raw(&transaction, &key)? {
                delete_item_with_change(&transaction, &row)?;
                removed += 1;
            }
            removed +=
                transaction.execute("DELETE FROM container WHERE container_key = ?1", [&key])?;
        }
        for key in build_item_keys.into_iter().filter(|key| should_purge(key)) {
            removed += transaction.execute("DELETE FROM item_build WHERE item_key = ?1", [&key])?;
        }
        for key in build_container_keys
            .into_iter()
            .filter(|key| should_purge(key))
        {
            removed +=
                transaction.execute("DELETE FROM item_build WHERE container_key = ?1", [&key])?;
            removed += transaction.execute(
                "DELETE FROM container_build WHERE container_key = ?1",
                [&key],
            )?;
        }
        for key in prefill_keys.into_iter().filter(|key| should_purge(key)) {
            removed +=
                transaction.execute("DELETE FROM item_prefill WHERE item_key = ?1", [&key])?;
        }

        transaction.execute(
            "UPDATE index_run
             SET registered_items = (
               SELECT COUNT(*) FROM item i
               LEFT JOIN container c ON c.container_key = i.container_key
               WHERE i.hash_version = index_run.hash_version
                 AND (i.container_key IS NULL OR c.scan_state = ?1)
             )
             WHERE singleton = 1",
            [ScanState::Complete as i64],
        )?;
        transaction.commit()?;
        Ok(removed)
    }

    #[cfg(test)]
    fn count_staged(&self) -> usize {
        self.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .query_row("SELECT COUNT(*) FROM item_build", [], |row| row.get(0))
            .unwrap()
    }
}

pub(crate) fn key_is_under_any(key: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        key == root
            || key
                .strip_prefix(root)
                .is_some_and(|suffix| root.ends_with('/') || suffix.starts_with('/'))
    })
}

fn query_string_column(transaction: &Transaction<'_>, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = transaction.prepare(sql)?;
    statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect()
}

fn building_page_count(
    transaction: &Transaction<'_>,
    container_key: &str,
    generation: u64,
) -> rusqlite::Result<i64> {
    transaction.query_row(
        "SELECT page_count FROM container_build
         WHERE container_key = ?1 AND generation = ?2 AND scan_state = ?3",
        params![container_key, generation, ScanState::Building as i64],
        |row| row.get(0),
    )
}

fn item_params(item: &StoredItem, generation: Option<i64>) -> Vec<rusqlite::types::Value> {
    let mut values = vec![
        item.item_key.clone().into(),
        (item.kind as i64).into(),
        item.container_key.clone().into(),
        item.page_index.map(i64::from).into(),
        item.mtime.into(),
        item.file_size.into(),
        item.hash_version.into(),
        item.pdq256.to_vec().into(),
        i64::from(item.quality).into(),
        i64::from(item.width).into(),
        i64::from(item.height).into(),
        item.format.into(),
    ];
    if let Some(generation) = generation {
        values.push(generation.into());
    }
    values
}

fn publish_item(
    conn: &Connection,
    existing: Option<&SearchRow>,
    item: &StoredItem,
) -> rusqlite::Result<SearchRow> {
    let (item_id, revision, op) = if let Some(existing) = existing {
        let revision = existing.revision.checked_add(1).ok_or_else(|| {
            rusqlite::Error::ToSqlConversionFailure("item revision exhausted".into())
        })?;
        conn.execute(
            "UPDATE item SET revision=?2, item_key=?3, kind=?4, container_key=?5,
               page_index=?6, mtime=?7, file_size=?8, hash_version=?9, pdq256=?10,
               quality=?11, width=?12, height=?13, format=?14 WHERE item_id=?1",
            params![
                i64::try_from(existing.item_id).unwrap_or(i64::MAX),
                i64::from(revision),
                item.item_key,
                item.kind as i64,
                item.container_key,
                item.page_index.map(i64::from),
                item.mtime,
                item.file_size,
                item.hash_version,
                item.pdq256.as_slice(),
                i64::from(item.quality),
                i64::from(item.width),
                i64::from(item.height),
                item.format,
            ],
        )?;
        (existing.item_id, revision, ItemChangeOp::Update)
    } else {
        conn.execute(
            "INSERT INTO item
             (revision, item_key, kind, container_key, page_index, mtime, file_size,
              hash_version, pdq256, quality, width, height, format)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params_from_iter(item_params(item, None)),
        )?;
        (
            i64_to_u64(conn.last_insert_rowid(), 0)?,
            1,
            ItemChangeOp::Add,
        )
    };
    insert_item_change(conn, item_id, op, Some(revision), Some(item))?;
    Ok(SearchRow {
        item_id,
        revision,
        item: item.clone(),
    })
}

fn delete_item_with_change(conn: &Connection, row: &SearchRow) -> rusqlite::Result<()> {
    insert_item_change(conn, row.item_id, ItemChangeOp::Delete, None, None)?;
    conn.execute(
        "DELETE FROM item WHERE item_id = ?1",
        [i64::try_from(row.item_id).unwrap_or(i64::MAX)],
    )?;
    Ok(())
}

fn insert_item_change(
    conn: &Connection,
    item_id: u64,
    op: ItemChangeOp,
    revision: Option<u32>,
    item: Option<&StoredItem>,
) -> rusqlite::Result<()> {
    let op = match op {
        ItemChangeOp::Add => 0,
        ItemChangeOp::Update => 1,
        ItemChangeOp::Delete => 2,
    };
    conn.execute(
        "INSERT INTO item_change (item_id, op, revision, pdq256, quality)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            i64::try_from(item_id).unwrap_or(i64::MAX),
            op,
            revision.map(i64::from),
            item.map(|item| item.pdq256.as_slice()),
            item.map(|item| i64::from(item.quality)),
        ],
    )?;
    Ok(())
}

fn row_to_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchRow> {
    let item_id = i64_to_u64(row.get(0)?, 0)?;
    let revision = i64_to_u32(row.get(1)?, 1)?;
    let pdq = row.get::<_, Vec<u8>>(9)?;
    let pdq256: [u8; 32] = pdq.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "pdq256 must contain 32 bytes".into(),
        )
    })?;
    let quality_i64 = row.get::<_, i64>(10)?;
    Ok(SearchRow {
        item_id,
        revision,
        item: StoredItem {
            item_key: row.get(2)?,
            kind: ItemKind::from_i64(row.get(3)?)?,
            container_key: row.get(4)?,
            page_index: row
                .get::<_, Option<i64>>(5)?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, -1))?,
            mtime: row.get(6)?,
            file_size: row.get(7)?,
            hash_version: row.get(8)?,
            pdq256,
            quality: u8::try_from(quality_i64)
                .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, quality_i64))?,
            width: i64_to_u32(row.get(11)?, 11)?,
            height: i64_to_u32(row.get(12)?, 12)?,
            format: row.get(13)?,
        },
    })
}

const SEARCH_ROW_SELECT: &str =
    "SELECT i.item_id, i.revision, i.item_key, i.kind, i.container_key, i.page_index,
            i.mtime, i.file_size, i.hash_version, i.pdq256, i.quality,
            i.width, i.height, i.format FROM item i";

fn load_item_raw(conn: &Connection, item_key: &str) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!("{SEARCH_ROW_SELECT} WHERE i.item_key = ?1"),
        [item_key],
        row_to_search_row,
    )
    .optional()
}

fn load_search_item_by_key(
    conn: &Connection,
    item_key: &str,
    hash_version: i64,
) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!(
            "{SEARCH_ROW_SELECT} LEFT JOIN container c ON c.container_key=i.container_key
             WHERE i.item_key=?1 AND i.hash_version=?2
               AND (i.container_key IS NULL OR c.scan_state=?3)"
        ),
        params![item_key, hash_version, ScanState::Complete as i64],
        row_to_search_row,
    )
    .optional()
}

fn load_search_item_by_id(
    conn: &Connection,
    item_id: u64,
    hash_version: i64,
) -> rusqlite::Result<Option<SearchRow>> {
    conn.query_row(
        &format!(
            "{SEARCH_ROW_SELECT} LEFT JOIN container c ON c.container_key=i.container_key
             WHERE i.item_id=?1 AND i.hash_version=?2
               AND (i.container_key IS NULL OR c.scan_state=?3)"
        ),
        params![
            i64::try_from(item_id).unwrap_or(i64::MAX),
            hash_version,
            ScanState::Complete as i64
        ],
        row_to_search_row,
    )
    .optional()
}

fn load_items_for_container_raw(
    conn: &Connection,
    container_key: &str,
) -> rusqlite::Result<Vec<SearchRow>> {
    let mut statement = conn.prepare(&format!(
        "{SEARCH_ROW_SELECT} WHERE i.container_key=?1 ORDER BY i.item_id"
    ))?;
    statement
        .query_map([container_key], row_to_search_row)?
        .collect()
}

fn load_items_for_container_state(
    conn: &Connection,
    state: ScanState,
) -> rusqlite::Result<Vec<SearchRow>> {
    let mut statement = conn.prepare(&format!(
        "{SEARCH_ROW_SELECT} JOIN container c ON c.container_key=i.container_key
         WHERE c.scan_state=?1 ORDER BY i.item_id"
    ))?;
    statement
        .query_map([state as i64], row_to_search_row)?
        .collect()
}

fn load_staged_items(
    conn: &Connection,
    container_key: &str,
    generation: u64,
) -> rusqlite::Result<Vec<StoredItem>> {
    let mut statement = conn.prepare(
        "SELECT item_key, kind, container_key, page_index, mtime, file_size, hash_version,
                pdq256, quality, width, height, format
         FROM item_build WHERE container_key=?1 AND generation=?2 ORDER BY page_index",
    )?;
    statement
        .query_map(params![container_key, generation], row_to_stored_item)?
        .collect()
}

fn row_to_stored_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredItem> {
    let pdq = row.get::<_, Vec<u8>>(7)?;
    let pdq256: [u8; 32] = pdq.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "pdq256 must contain 32 bytes".into(),
        )
    })?;
    let quality = row.get::<_, i64>(8)?;
    Ok(StoredItem {
        item_key: row.get(0)?,
        kind: ItemKind::from_i64(row.get(1)?)?,
        container_key: row.get(2)?,
        page_index: row
            .get::<_, Option<i64>>(3)?
            .map(u32::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, -1))?,
        mtime: row.get(4)?,
        file_size: row.get(5)?,
        hash_version: row.get(6)?,
        pdq256,
        quality: u8::try_from(quality)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(8, quality))?,
        width: i64_to_u32(row.get(9)?, 9)?,
        height: i64_to_u32(row.get(10)?, 10)?,
        format: row.get(11)?,
    })
}

fn row_to_item_change(row: &rusqlite::Row<'_>) -> rusqlite::Result<ItemChange> {
    let op_value = row.get::<_, i64>(2)?;
    let op = match op_value {
        0 => ItemChangeOp::Add,
        1 => ItemChangeOp::Update,
        2 => ItemChangeOp::Delete,
        _ => return Err(rusqlite::Error::IntegralValueOutOfRange(2, op_value)),
    };
    let signature = row
        .get::<_, Option<Vec<u8>>>(4)?
        .map(|bytes| {
            let len = bytes.len();
            bytes.try_into().map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    len,
                    rusqlite::types::Type::Blob,
                    "pdq256 must contain 32 bytes".into(),
                )
            })
        })
        .transpose()?;
    let quality_value = row.get::<_, Option<i64>>(5)?;
    Ok(ItemChange {
        seq: i64_to_u64(row.get(0)?, 0)?,
        item_id: i64_to_u64(row.get(1)?, 1)?,
        op,
        revision: row
            .get::<_, Option<i64>>(3)?
            .map(|v| i64_to_u32(v, 3))
            .transpose()?,
        signature,
        quality: quality_value
            .map(|v| u8::try_from(v).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, v)))
            .transpose()?,
    })
}

fn i64_to_u32(value: i64, column: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn i64_to_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn search_store_id(conn: &Connection) -> rusqlite::Result<[u8; 16]> {
    let store_id = conn.query_row(
        "SELECT store_id FROM search_content_state WHERE singleton = 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let store_id: [u8; 16] = store_id.try_into().map_err(|value: Vec<u8>| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Blob,
            "search content store_id must contain 16 bytes".into(),
        )
    })?;
    Ok(store_id)
}

fn latest_change_seq(conn: &Connection) -> rusqlite::Result<u64> {
    let seq = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'item_change'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    i64_to_u64(seq, 0)
}

/// Step 5 より前の店かどうかを、世代番号ではなく**形**で見る。
///
/// 旧 `init_schema` は `PRAGMA user_version` を一度も書いておらず、実店を読むと 0 だった。
/// 新規ファイルも 0 なので番号では区別できない。v1 にしかない形 ——`item` はあるが `item_id`
/// を持たない—— で判定し、`search_content_state` の同居も要求する。片方しかない店は v1 として
/// 読まず、未知の世代と同じく作り直す。
fn is_v1_layout(conn: &Connection) -> rusqlite::Result<bool> {
    let table_exists = |name: &str| -> rusqlite::Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )
    };
    if !table_exists("item")? || !table_exists("search_content_state")? {
        return Ok(false);
    }
    let has_item_id: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('item') WHERE name = 'item_id')",
        [],
        |row| row.get(0),
    )?;
    Ok(!has_item_id)
}

/// v1 の店を v2 へ移す。**署名は作り直さない。**
///
/// `dupe` は Step 5 で一行も変わっていないので、保存された PDQ 署名と `quality` の意味は
/// v1 と同一である。v1 と v2 の違いは `item` が `item_id` と `revision` を得たことと、
/// `search_content_state` が使わなくなった `generation` を落としたことだけで、他のテーブルは
/// 定義が一致する。値が同じものを再デコードする理由はない (実店で約 11.6 時間かかる)。
///
/// 旧表をここで退避し、共通の CREATE が新しい形を作った後に [`copy_v1_rows_into_v2`] が
/// 中身を移す。`ALTER TABLE ... RENAME` は索引を連れて行かないため、同名で作り直せるよう
/// 旧索引を先に落とす。
fn move_v1_tables_aside(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE item RENAME TO item_v1;
         DROP INDEX IF EXISTS item_container_idx;
         ALTER TABLE search_content_state RENAME TO search_content_state_v1;",
    )
}

/// 退避した v1 の行を新しい表へ移し、旧表を捨てる。
///
/// `item_id` は AUTOINCREMENT が採番し、`revision` は 1 から始める。`item_change` は空のまま
/// なので、最初の配列構築が SQLite から base を作り直す。`store_id` は引き継ぐ。
fn copy_v1_rows_into_v2(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "INSERT INTO item (item_key, revision, kind, container_key, page_index, mtime,
                           file_size, hash_version, pdq256, quality, width, height, format)
           SELECT item_key, 1, kind, container_key, page_index, mtime,
                  file_size, hash_version, pdq256, quality, width, height, format
           FROM item_v1;
         INSERT INTO search_content_state (singleton, store_id, page_order_version)
           SELECT singleton, store_id, 0 FROM search_content_state_v1;
         DROP TABLE item_v1;
         DROP TABLE search_content_state_v1;",
    )
}

/// 書き込みを行う transaction。**DEFERRED を使わない。**
///
/// WAL では、読み取りで始まった transaction が後から書こうとしたときに別の接続が書いていると、
/// SQLite は待たずに SQLITE_BUSY を返す。`busy_timeout` はこの昇格を待てないので、
/// 「database is locked」がそのまま呼び出し側の失敗になる。索引 worker と配列更新 worker は
/// どちらもこの店へ書くため、書く側は最初から書き込みロックを取って順番に待つ。
fn write_transaction(conn: &mut Connection) -> rusqlite::Result<Transaction<'_>> {
    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
}

fn stored_schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
}

fn init_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    // 定常状態では書き込みロックを一切取らない。索引 worker・配列更新 worker・パネル照会・
    // 集計はそれぞれ別の接続でこの店を開くので、開くこと自体が writer になると互いに競合する。
    if stored_schema_version(conn)? == SCHEMA_VERSION {
        return Ok(());
    }
    // 作成と移行は writer なので、読んでから書きへ上げない。WAL の deferred transaction は
    // 読み取り後に別の接続が書いていると SQLITE_BUSY を即返し、これは busy_timeout では
    // 待てない (「database is locked」で開けなくなる)。最初から IMMEDIATE を取る。
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let user_version = stored_schema_version(&transaction)?;
    if user_version == SCHEMA_VERSION {
        // ロックを待っている間に、別の接続が作成か移行を終えていた。
        return transaction.commit();
    }
    // 中断しても v1 のまま残るか v2 へ移り切るかのどちらかになるよう、退避・作成・移送・
    // user_version の更新を一つの transaction に入れる。
    let migrating_v1 = user_version == 0 && is_v1_layout(&transaction)?;
    if migrating_v1 {
        move_v1_tables_aside(&transaction)?;
    } else if user_version == 2 {
        // v2 の行はそのまま使える。`page_index` の並べ方だけが分からないので 0 を記録し、
        // 起動時の振り直しに任せる。署名は変わらないので再索引は起きない。
        transaction.execute_batch(
            "ALTER TABLE search_content_state
               ADD COLUMN page_order_version INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 3;",
        )?;
    } else if user_version != SCHEMA_VERSION {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS item_change;
             DROP TABLE IF EXISTS item_build;
             DROP TABLE IF EXISTS item_prefill;
             DROP TABLE IF EXISTS container_build;
             DROP TABLE IF EXISTS item;
             DROP TABLE IF EXISTS container;
             DROP TABLE IF EXISTS index_run;
             DROP TABLE IF EXISTS search_content_state;",
        )?;
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS item (
           item_id INTEGER PRIMARY KEY AUTOINCREMENT,
           item_key TEXT NOT NULL UNIQUE,
           revision INTEGER NOT NULL CHECK(revision > 0),
           kind INTEGER NOT NULL,
           container_key TEXT,
           page_index INTEGER,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS item_container_idx ON item(container_key, page_index);
         CREATE TABLE IF NOT EXISTS item_change (
           seq INTEGER PRIMARY KEY AUTOINCREMENT,
           item_id INTEGER NOT NULL,
           op INTEGER NOT NULL CHECK(op IN (0, 1, 2)),
           revision INTEGER,
           pdq256 BLOB CHECK(pdq256 IS NULL OR length(pdq256) = 32),
           quality INTEGER,
           CHECK((op = 2 AND revision IS NULL AND pdq256 IS NULL AND quality IS NULL)
              OR (op IN (0, 1) AND revision IS NOT NULL AND pdq256 IS NOT NULL AND quality IS NOT NULL))
         );
         CREATE TABLE IF NOT EXISTS container (
           container_key TEXT PRIMARY KEY,
           kind INTEGER NOT NULL,
           page_count INTEGER,
           scan_state INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS container_build (
           container_key TEXT PRIMARY KEY,
           kind INTEGER NOT NULL,
           page_count INTEGER NOT NULL,
           scan_state INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS item_build (
           item_key TEXT NOT NULL,
           kind INTEGER NOT NULL,
           container_key TEXT NOT NULL,
           page_index INTEGER NOT NULL,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL,
           generation INTEGER NOT NULL,
           PRIMARY KEY(container_key, generation, item_key)
         );
         CREATE TABLE IF NOT EXISTS item_prefill (
           item_key TEXT PRIMARY KEY,
           mtime INTEGER NOT NULL,
           file_size INTEGER NOT NULL,
           hash_version INTEGER NOT NULL,
           pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
           quality INTEGER NOT NULL,
           width INTEGER NOT NULL,
           height INTEGER NOT NULL,
           format INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS index_run (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           hash_version INTEGER NOT NULL,
           completed_at_unix_secs INTEGER NOT NULL,
           registered_items INTEGER NOT NULL,
           password_required_pdfs INTEGER NOT NULL,
           corrupt_containers INTEGER NOT NULL,
           zero_page_containers INTEGER NOT NULL,
           decode_failures INTEGER NOT NULL,
           io_failures INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS search_content_state (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           store_id BLOB NOT NULL CHECK(length(store_id) = 16),
           page_order_version INTEGER NOT NULL
         );
         PRAGMA user_version = 3;",
    )?;
    if migrating_v1 {
        copy_v1_rows_into_v2(&transaction)?;
    }
    let store_id = *uuid::Uuid::new_v4().as_bytes();
    transaction.execute(
        "INSERT OR IGNORE INTO search_content_state (singleton, store_id, page_order_version)
         VALUES (1, ?1, ?2)",
        params![store_id.as_slice(), PAGE_ORDER_VERSION],
    )?;
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, container: Option<&str>, page: Option<u32>, marker: u8) -> StoredItem {
        StoredItem {
            item_key: key.to_owned(),
            kind: ItemKind::Image,
            container_key: container.map(str::to_owned),
            page_index: page,
            mtime: 10,
            file_size: 20,
            hash_version: current_hash_version(),
            pdq256: [marker; 32],
            quality: 50,
            width: 100,
            height: 80,
            format: 1,
        }
    }

    /// Step 5 より前の店を作る。移行が「同じ値を運べたか」を問えるように、DDL は当時のまま
    /// 書き写す。ここを現在の定義から生成すると、移行が壊れても気付けない。
    ///
    /// `PRAGMA user_version` を**設定しない**のが要点。旧 `init_schema` は書いておらず、
    /// 実店 (4,628,611 行) を読んでも 0 だった。世代番号で移行を選ぶと、この店は新規ファイル
    /// と区別されずに捨てられる。
    fn create_v1_store(path: &std::path::Path, store_id: [u8; 16]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE item (
               item_key TEXT PRIMARY KEY,
               kind INTEGER NOT NULL,
               container_key TEXT,
               page_index INTEGER,
               mtime INTEGER NOT NULL,
               file_size INTEGER NOT NULL,
               hash_version INTEGER NOT NULL,
               pdq256 BLOB NOT NULL CHECK(length(pdq256) = 32),
               quality INTEGER NOT NULL,
               width INTEGER NOT NULL,
               height INTEGER NOT NULL,
               format INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS item_container_idx ON item(container_key, page_index);
             CREATE TABLE container (
               container_key TEXT PRIMARY KEY,
               kind INTEGER NOT NULL,
               page_count INTEGER,
               scan_state INTEGER NOT NULL,
               generation INTEGER NOT NULL,
               mtime INTEGER NOT NULL,
               file_size INTEGER NOT NULL
             );
             CREATE TABLE search_content_state (
               singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
               store_id BLOB NOT NULL CHECK(length(store_id) = 16),
               generation INTEGER NOT NULL CHECK(generation >= 0)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO search_content_state (singleton, store_id, generation) VALUES (1, ?1, 7)",
            [store_id.as_slice()],
        )
        .unwrap();
        let mut insert = conn
            .prepare(
                "INSERT INTO item (item_key, kind, container_key, page_index, mtime, file_size,
                                   hash_version, pdq256, quality, width, height, format)
                 VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .unwrap();
        for (key, marker, quality) in [("a", 0x11u8, 40i64), ("b", 0x22, 60)] {
            insert
                .execute(params![
                    key,
                    ItemKind::Image as i64,
                    10i64,
                    20i64,
                    current_hash_version(),
                    [marker; 32].as_slice(),
                    quality,
                    100i64,
                    80i64,
                    1i64
                ])
                .unwrap();
        }
    }

    #[test]
    fn v1_store_keeps_its_signatures_instead_of_rehashing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let store_id = [0x5au8; 16];
        create_v1_store(&path, store_id);

        let db = SimilarDb::open_at(&path).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();

        // 再デコードしていたら署名は同じでも「移行できた」ことにならないので、行が残って
        // いること自体を先に確かめる。索引ジョブはこのテストでは一度も走っていない。
        assert_eq!(base.records.len(), 2);
        let signatures = base
            .records
            .iter()
            .map(|record| record.signature[0])
            .collect::<Vec<_>>();
        assert_eq!(signatures, vec![0x11, 0x22]);
        assert_eq!(
            base.records.iter().map(|r| r.quality).collect::<Vec<_>>(),
            vec![40, 60]
        );
        // item_id は採番され、重複しない。revision は 1 から始まる。
        assert_eq!(
            base.records.iter().map(|r| r.item_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(base.records.iter().all(|record| record.revision == 1));
        assert_eq!(base.store_id, store_id);
        assert_eq!(base.applied_seq, 0);

        let key_a = db.load_item("a", current_hash_version()).unwrap().unwrap();
        assert_eq!(key_a.item.pdq256, [0x11; 32]);
        assert_eq!(key_a.item.width, 100);
    }

    /// WAL 化済みの v1 store で既存 writer が移行を塞いでいる間は待機し、解放後に
    /// 署名と行を保ったまま現行 schema へ移行する。
    #[test]
    fn v1_migration_waits_for_an_existing_wal_writer_and_preserves_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let store_id = [0x6bu8; 16];
        create_v1_store(&path, store_id);

        // WAL 変換と schema migration の待機を分ける。ここで WAL へ変換しておけば、
        // opener は journal-mode writer ではなく既存 IMMEDIATE writer の解放を待つ。
        let wal = Connection::open(&path).unwrap();
        wal.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        drop(wal);

        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let opener_path = path.clone();
        let opener = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = SimilarDb::open_at(&opener_path)
                .and_then(|db| db.load_base_search_rows(current_hash_version()));
            result_tx.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        let early = result_rx.recv_timeout(std::time::Duration::from_millis(100));

        blocker.execute_batch("ROLLBACK;").unwrap();
        let (finished_while_held, terminal) = match early {
            Ok(result) => (true, Ok(result)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (
                false,
                result_rx.recv_timeout(std::time::Duration::from_secs(5)),
            ),
            Err(error @ std::sync::mpsc::RecvTimeoutError::Disconnected) => (false, Err(error)),
        };
        let joined = opener.join();

        assert!(joined.is_ok(), "v1 migration opener panicked");
        assert!(
            !finished_while_held,
            "v1 migration completed while its IMMEDIATE writer was still held"
        );
        let base = terminal
            .expect("v1 migration opener did not return a terminal result after writer release")
            .unwrap();

        assert_eq!(base.records.len(), 2);
        assert_eq!(
            base.records
                .iter()
                .map(|record| record.signature[0])
                .collect::<Vec<_>>(),
            vec![0x11, 0x22]
        );
        assert_eq!(base.store_id, store_id);
        assert_eq!(base.applied_seq, 0);
    }

    /// v2 の店を開き直したときに作り直されないこと。ここが逆になると、起動のたびに索引が
    /// 消える。`is_v1_layout` は形で判定するので、番号の一致だけに頼らず両方を確かめる。
    #[test]
    fn reopening_a_v2_store_keeps_its_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        {
            let db = SimilarDb::open_at(&path).unwrap();
            db.upsert_loose_item(&item("kept", None, None, 9)).unwrap();
        }
        let db = SimilarDb::open_at(&path).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        assert_eq!(base.records.len(), 1);
        assert_eq!(base.records[0].signature, [9; 32]);
    }

    /// 一括移行を待てる時間が実際に設定されていること。既定の 5 秒に戻ると、移行中に別 worker
    /// が開いた瞬間に「database is locked」で落ちる。
    #[test]
    fn a_bulk_migration_can_be_waited_out_by_another_opener() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let db = SimilarDb::open_at(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        let timeout_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert!(
            timeout_ms >= 120_000,
            "busy_timeout {timeout_ms} ms is shorter than a measured 38.9 s migration"
        );
    }

    /// fresh DB の初回 WAL 変換へ複数 worker が同時に入っても、open 自体を失敗させない。
    ///
    /// 既存の並列回帰は先に一度 open していたため、全接続が既に WAL を読む定常経路しか覆わず、
    /// index worker と memory loader の初回競合を検出できなかった。
    #[test]
    fn opening_fresh_stores_from_many_workers_at_once_never_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        for round in 0..8 {
            let path = tmp.path().join(format!("fresh-{round}")).join("similar.db");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles = (0..8)
                .map(|_| {
                    let path = path.clone();
                    let barrier = std::sync::Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        SimilarDb::open_at(&path).map(|_| ())
                    })
                })
                .collect::<Vec<_>>();
            let errors = handles
                .into_iter()
                .filter_map(|handle| handle.join().unwrap().err())
                .collect::<Vec<_>>();
            assert!(errors.is_empty(), "round {round} failed: {errors:?}");
        }
    }

    /// 既に WAL の store は active writer がいても journal mode の変換を試みない。
    #[test]
    fn opening_an_existing_wal_store_does_not_contend_with_its_writer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        let owner = SimilarDb::open_at(&path).unwrap();
        let owner_conn = owner.conn.lock().unwrap_or_else(|error| error.into_inner());
        owner_conn.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let reopened = SimilarDb::open_at(&path);

        owner_conn.execute_batch("ROLLBACK;").unwrap();
        assert!(
            reopened.is_ok(),
            "an existing WAL store open tried to take the writer lock: {:?}",
            reopened.err()
        );
    }

    /// 同じ店を複数の worker が同時に開いても失敗しないこと。
    ///
    /// 索引 worker・配列更新 worker・パネル照会・集計はそれぞれ別の接続でこの店を開く。
    /// どれか一つでも開けないと、その経路は結果を返せずに諦める。
    #[test]
    fn opening_the_same_store_from_many_workers_at_once_never_fails() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        SimilarDb::open_at(&path).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            let failures = std::sync::Arc::clone(&failures);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..20 {
                    match SimilarDb::open_at(&path) {
                        Ok(db) => {
                            db.upsert_loose_item(&item("k", None, None, 1)).unwrap();
                        }
                        Err(error) => {
                            eprintln!("open failed: {error}");
                            failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(failures.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    /// 実店の複製に対して移行を通す手動確認。`MIV_SIMILAR_V1_COPY` に **複製の** パスを渡す。
    /// 元の店を指さないこと。移行はその場でファイルを書き換える。
    #[test]
    #[ignore = "manual migration check against a caller-supplied copy of a real v1 store"]
    fn migrate_a_real_v1_store_copy() {
        let path = std::path::PathBuf::from(
            std::env::var("MIV_SIMILAR_V1_COPY").expect("MIV_SIMILAR_V1_COPY"),
        );
        let before =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let expected_rows: i64 = before
            .query_row("SELECT COUNT(*) FROM item", [], |row| row.get(0))
            .unwrap();
        let sample: (String, Vec<u8>, i64) = before
            .query_row(
                "SELECT item_key, pdq256, quality FROM item ORDER BY item_key LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        drop(before);

        let started = std::time::Instant::now();
        let db = SimilarDb::open_at(&path).unwrap();
        let elapsed = started.elapsed();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        println!(
            "migrated {expected_rows} rows in {:.1}s, {} searchable",
            elapsed.as_secs_f64(),
            base.records.len()
        );

        let migrated = db
            .load_item(&sample.0, current_hash_version())
            .unwrap()
            .unwrap();
        assert_eq!(migrated.item.pdq256.as_slice(), sample.1.as_slice());
        assert_eq!(migrated.item.quality as i64, sample.2);
        assert_eq!(migrated.revision, 1);
    }

    /// 読めない世代は移行せず作り直す。v1 の移行を足したことで、この経路が消えていないこと。
    #[test]
    fn an_unknown_schema_generation_is_still_rebuilt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("similar.db");
        create_v1_store(&path, [0x5a; 16]);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        drop(conn);

        let db = SimilarDb::open_at(&path).unwrap();
        let base = db.load_base_search_rows(current_hash_version()).unwrap();
        assert!(base.records.is_empty());
    }

    #[test]
    fn building_generation_is_not_searchable_and_cleanup_removes_it() {
        let db = SimilarDb::open_in_memory().unwrap();
        let generation = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 1, 0)
            .unwrap();
        db.stage_item(generation, &item("a", Some("book"), Some(0), 1))
            .unwrap();

        assert!(
            db.load_search_rows(current_hash_version())
                .unwrap()
                .is_empty()
        );
        assert!(
            db.load_base_search_rows(current_hash_version())
                .unwrap()
                .records
                .is_empty()
        );
        assert_eq!(db.count_staged(), 1);
        db.cleanup_incomplete().unwrap();
        assert_eq!(db.count_staged(), 0);
        assert!(
            db.load_search_rows(current_hash_version())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn item_change_tracks_only_published_changes() {
        let db = SimilarDb::open_in_memory().unwrap();
        let initial_store = db.search_store_id().unwrap();
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);

        let generation = db
            .begin_container_build("book", ContainerKind::ImageFolder, 1, 1, 0)
            .unwrap();
        db.stage_item(generation, &item("page", Some("book"), Some(0), 1))
            .unwrap();
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);

        db.complete_container("book", generation).unwrap();
        let after_publish = db.load_item_changes_after(0).unwrap();
        assert_eq!(db.search_store_id().unwrap(), initial_store);
        assert_eq!(after_publish.changes.len(), 1);
        assert_eq!(after_publish.changes[0].op, ItemChangeOp::Add);

        let loose = item("loose", None, None, 2);
        assert!(db.upsert_loose_item(&loose).unwrap());
        let after_loose = db.load_item_changes_after(0).unwrap();
        assert_eq!(after_loose.latest_seq, after_publish.latest_seq + 1);
        assert!(!db.upsert_loose_item(&loose).unwrap());
        assert_eq!(db.load_item_changes_after(0).unwrap(), after_loose);

        let seen_items = HashSet::from(["loose".to_owned()]);
        let seen_containers = HashSet::new();
        assert!(db.prune_except_seen(&seen_items, &seen_containers).unwrap() > 0);
        let after_prune = db.load_item_changes_after(0).unwrap();
        assert!(after_prune.latest_seq > after_loose.latest_seq);
        assert!(
            after_prune
                .changes
                .iter()
                .any(|change| change.op == ItemChangeOp::Delete)
        );
    }

    #[test]
    fn item_and_change_log_roll_back_together() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_item_change
                 BEFORE INSERT ON item_change
                 BEGIN
                   SELECT RAISE(ABORT, 'test item_change failure');
                 END;",
            )
            .unwrap();

        assert!(
            db.upsert_loose_item(&item("never-published", None, None, 1))
                .is_err()
        );
        assert!(
            db.load_item("never-published", current_hash_version())
                .unwrap()
                .is_none()
        );
        assert_eq!(db.load_item_changes_after(0).unwrap().latest_seq, 0);
    }

    #[test]
    fn each_store_gets_a_distinct_content_identity() {
        let first = SimilarDb::open_in_memory()
            .unwrap()
            .search_store_id()
            .unwrap();
        let second = SimilarDb::open_in_memory()
            .unwrap()
            .search_store_id()
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn generation_swap_is_atomic_and_cancel_preserves_complete_generation() {
        let db = SimilarDb::open_in_memory().unwrap();
        let first = db
            .begin_container_build("book", ContainerKind::ImageFolder, 1, 1, 0)
            .unwrap();
        db.stage_item(first, &item("old", Some("book"), Some(0), 1))
            .unwrap();
        db.complete_container("book", first).unwrap();
        let compact = db.load_base_search_rows(current_hash_version()).unwrap();
        assert_eq!(compact.records.len(), 1);
        let old_item_id = compact.records[0].item_id;
        assert_eq!(
            db.load_item_by_id(old_item_id, current_hash_version())
                .unwrap()
                .unwrap()
                .item
                .item_key,
            "old"
        );

        let second = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 2, 0)
            .unwrap();
        db.stage_item(second, &item("new-a", Some("book"), Some(0), 2))
            .unwrap();
        let during = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(during.len(), 1);
        assert_eq!(during[0].item.item_key, "old");

        // cancel/cleanup しても公開済み世代はそのまま。
        db.cleanup_incomplete().unwrap();
        assert_eq!(db.load_search_rows(current_hash_version()).unwrap(), during);

        let second = db
            .begin_container_build("book", ContainerKind::ImageFolder, 2, 2, 0)
            .unwrap();
        db.stage_item(second, &item("new-a", Some("book"), Some(0), 2))
            .unwrap();
        db.stage_item(second, &item("new-b", Some("book"), Some(1), 3))
            .unwrap();
        db.complete_container("book", second).unwrap();
        let after = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(after.len(), 2);
        assert!(
            after
                .iter()
                .all(|row| row.item.item_key.starts_with("new-"))
        );
        assert_eq!(db.load_complete_containers().unwrap()[0].generation, 2);
    }

    #[test]
    fn hash_and_proxy_versions_invalidate_rows() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("a", None, None, 1)).unwrap();
        assert_eq!(
            db.item_freshness("a", 10, 20, current_hash_version())
                .unwrap(),
            Freshness::Current
        );
        assert_eq!(
            db.item_freshness("a", 10, 20, current_hash_version() + 1)
                .unwrap(),
            Freshness::Stale
        );
        assert!(
            db.load_search_rows(current_hash_version() + 1)
                .unwrap()
                .is_empty()
        );
        assert_ne!(
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION),
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION + 1)
        );
        assert_ne!(
            hash_version(HASH_ALGORITHM_VERSION, crate::dupe::PROXY_VERSION),
            hash_version(HASH_ALGORITHM_VERSION + 1, crate::dupe::PROXY_VERSION)
        );
    }

    #[test]
    fn only_changed_item_is_recomputed() {
        let db = SimilarDb::open_in_memory().unwrap();
        let first = item("a", None, None, 1);
        assert!(db.upsert_loose_item(&first).unwrap());
        assert!(!db.upsert_loose_item(&first).unwrap());

        let mut changed = first.clone();
        changed.file_size += 1;
        assert!(db.upsert_loose_item(&changed).unwrap());
        assert_eq!(
            db.load_search_rows(current_hash_version()).unwrap().len(),
            1
        );
        let changes = db.load_item_changes_after(0).unwrap();
        assert_eq!(changes.changes.len(), 2);
        assert_eq!(changes.changes[0].item_id, changes.changes[1].item_id);
        assert_eq!(changes.changes[1].revision, Some(2));
    }

    #[test]
    fn deleted_item_id_is_never_reused_for_the_same_key() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("same", None, None, 1)).unwrap();
        let first_id = db
            .load_item("same", current_hash_version())
            .unwrap()
            .unwrap()
            .item_id;
        assert_eq!(
            db.prune_except_seen(&HashSet::new(), &HashSet::new())
                .unwrap(),
            1
        );
        db.upsert_loose_item(&item("same", None, None, 2)).unwrap();
        let second_id = db
            .load_item("same", current_hash_version())
            .unwrap()
            .unwrap()
            .item_id;
        assert!(second_id > first_id);
        let changes = db.load_item_changes_after(0).unwrap();
        assert_eq!(
            changes.changes.iter().map(|c| c.op).collect::<Vec<_>>(),
            vec![ItemChangeOp::Add, ItemChangeOp::Delete, ItemChangeOp::Add]
        );
    }

    #[test]
    fn completed_index_summary_counts_only_searchable_rows_and_round_trips_failures() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("loose", None, None, 1)).unwrap();
        let complete = db
            .begin_container_build("complete", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            complete,
            &item("complete-page", Some("complete"), Some(0), 2),
        )
        .unwrap();
        db.complete_container("complete", complete).unwrap();
        let building = db
            .begin_container_build("building", ContainerKind::Pdf, 1, 1, 10)
            .unwrap();
        db.stage_item(
            building,
            &item("building-page", Some("building"), Some(0), 3),
        )
        .unwrap();

        let stats = CompletedIndexStats {
            password_required_pdfs: 1,
            corrupt_containers: 2,
            zero_page_containers: 3,
            decode_failures: 4,
            io_failures: 5,
        };
        let stored = db
            .record_completed_index(current_hash_version(), 1234, stats)
            .unwrap();
        assert_eq!(stored.registered_items, 2);
        assert_eq!(
            db.load_index_summary(current_hash_version()).unwrap(),
            Some(stored)
        );
        assert_eq!(
            db.load_index_summary(current_hash_version() + 1).unwrap(),
            None
        );
    }

    #[test]
    fn completed_scope_snapshot_prunes_rows_from_disabled_roots() {
        let db = SimilarDb::open_in_memory().unwrap();
        db.upsert_loose_item(&item("c:/enabled/a.jpg", None, None, 1))
            .unwrap();
        db.upsert_loose_item(&item("c:/disabled/b.jpg", None, None, 2))
            .unwrap();

        let seen_items = HashSet::from(["c:/enabled/a.jpg".to_owned()]);
        assert_eq!(
            db.prune_except_seen(&seen_items, &HashSet::new()).unwrap(),
            1
        );
        let rows = db.load_search_rows(current_hash_version()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.item_key, "c:/enabled/a.jpg");
    }

    #[test]
    fn disabled_favorite_purge_preserves_enabled_overlap_and_prefix_sibling() {
        let db = SimilarDb::open_in_memory().unwrap();
        for (key, marker) in [
            ("c:/library/drop/a.jpg", 1),
            ("c:/library/keep/a.jpg", 2),
            ("c:/library-old/a.jpg", 3),
        ] {
            db.upsert_loose_item(&item(key, None, None, marker))
                .unwrap();
        }
        let drop_book = db
            .begin_container_build("c:/library/drop/book.zip", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            drop_book,
            &item(
                "c:/library/drop/book.zip\u{1f}page.jpg",
                Some("c:/library/drop/book.zip"),
                Some(0),
                4,
            ),
        )
        .unwrap();
        db.complete_container("c:/library/drop/book.zip", drop_book)
            .unwrap();
        let building = db
            .begin_container_build("c:/library/drop/building.zip", ContainerKind::Zip, 1, 1, 10)
            .unwrap();
        db.stage_item(
            building,
            &item(
                "c:/library/drop/building.zip\u{1f}page.jpg",
                Some("c:/library/drop/building.zip"),
                Some(0),
                5,
            ),
        )
        .unwrap();
        let prefill = item("c:/library/drop/prefill.jpg", None, None, 6);
        db.put_prefill(&prefill).unwrap();
        db.record_completed_index(current_hash_version(), 1234, CompletedIndexStats::default())
            .unwrap();

        let removed = db
            .purge_roots_except(&["c:/library".to_owned()], &["c:/library/keep".to_owned()])
            .unwrap();
        // loose + complete page/container + Building page/public/build row + prefill。
        assert_eq!(removed, 7);

        let keys = db
            .load_search_rows(current_hash_version())
            .unwrap()
            .into_iter()
            .map(|row| row.item.item_key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "c:/library/keep/a.jpg".to_owned(),
                "c:/library-old/a.jpg".to_owned(),
            ]
        );
        assert!(db.load_complete_containers().unwrap().is_empty());
        assert_eq!(db.count_staged(), 0);
        assert!(
            db.load_prefill(
                &prefill.item_key,
                prefill.mtime,
                prefill.file_size,
                prefill.hash_version,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            db.load_index_summary(current_hash_version())
                .unwrap()
                .unwrap()
                .registered_items,
            2
        );
    }
}
