//! 本のページブックマーク永続化と非同期アクセス。
//!
//! ページ番号は表示用 hint に限り、ジャンプ先の正本はコンテナ種別ごとに
//! [`PageIdentity`] で保持する。SQLite は [`BookBookmarkService`] の専用 worker
//! だけが触り、UI スレッドは request / event の送受信だけを行う。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, mpsc};

const MIGRATION_PHASE_PREPARED: &str = "prepared";
const MIGRATION_PHASE_APPLYING: &str = "applying";
const MIGRATION_PHASE_ROLLING_BACK: &str = "rolling_back";
const MIGRATION_PHASE_FILESYSTEM_COMMITTED: &str = "filesystem_committed";

/// A second App/service can exist transiently in tests and activation flows.
/// Startup recovery must not mistake an operation whose writer is alive in
/// this process for a crash remnant.
static ACTIVE_PATH_MIGRATION_JOBS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_path_migration_jobs() -> &'static Mutex<HashSet<String>> {
    ACTIVE_PATH_MIGRATION_JOBS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn register_active_path_migration(job_id: &str) {
    active_path_migration_jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(job_id.to_string());
}

fn unregister_active_path_migration(job_id: &str) {
    active_path_migration_jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(job_id);
}

fn path_migration_is_active(job_id: &str) -> bool {
    active_path_migration_jobs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(job_id)
}

#[derive(Debug)]
struct PathMigrationJournalEntry {
    job_id: String,
    mappings_json: String,
    operation_json: Option<String>,
    phase: String,
    next_step: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookContainerKind {
    CompiledBook,
    ImageFolder,
    Zip,
    Pdf,
    OtherArchive,
}

impl BookContainerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompiledBook => "compiled_book",
            Self::ImageFolder => "image_folder",
            Self::Zip => "zip",
            Self::Pdf => "pdf",
            Self::OtherArchive => "other_archive",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "compiled_book" => Some(Self::CompiledBook),
            "image_folder" => Some(Self::ImageFolder),
            "zip" => Some(Self::Zip),
            "pdf" => Some(Self::Pdf),
            "other_archive" => Some(Self::OtherArchive),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CompiledBook => "製本",
            Self::ImageFolder => "画像フォルダ",
            Self::Zip => "ZIP・CBZ",
            Self::Pdf => "PDF",
            Self::OtherArchive => "その他アーカイブ",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageIdentity {
    RelativePath(String),
    ArchiveEntry(String),
    PdfPage(u32),
}

impl PageIdentity {
    fn storage_parts(&self) -> (&'static str, String, String) {
        match self {
            Self::RelativePath(value) => {
                ("relative_path", value.clone(), normalize_page_path(value))
            }
            Self::ArchiveEntry(value) => {
                ("archive_entry", value.clone(), normalize_page_path(value))
            }
            Self::PdfPage(page) => ("pdf_page", page.to_string(), page.to_string()),
        }
    }

    fn from_storage(kind: &str, value: String) -> Option<Self> {
        match kind {
            "relative_path" => Some(Self::RelativePath(value)),
            "archive_entry" => Some(Self::ArchiveEntry(value)),
            "pdf_page" => value.parse::<u32>().ok().map(Self::PdfPage),
            _ => None,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::RelativePath(value) | Self::ArchiveEntry(value) => value
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(value)
                .to_string(),
            Self::PdfPage(page) => format!("{} ページ", page.saturating_add(1)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewBookBookmark {
    pub container_path: PathBuf,
    pub container_kind: BookContainerKind,
    pub page_identity: PageIdentity,
    pub page_index_hint: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookBookmark {
    pub id: i64,
    pub container_key: String,
    pub container_path: PathBuf,
    pub container_kind: BookContainerKind,
    pub page_identity: PageIdentity,
    pub page_index_hint: usize,
    pub created_at_ms: i64,
    /// ユーザーが任意で付けた名称。空文字は保存せず `None` として扱う。
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub enum BookBookmarkEvent {
    Added {
        request_id: u64,
        result: Result<(BookBookmark, bool), String>,
    },
    Removed {
        request_id: u64,
        result: Result<i64, String>,
    },
    TitleUpdated {
        request_id: u64,
        result: Result<(i64, Option<String>), String>,
    },
    ContainerListed {
        request_id: u64,
        container_key: String,
        result: Result<Vec<BookBookmark>, String>,
    },
    AllListed {
        request_id: u64,
        result: Result<Vec<BookBookmark>, String>,
    },
    PathsMigrated {
        request_id: u64,
        result: Result<usize, String>,
    },
}

enum BookBookmarkRequest {
    Add(u64, NewBookBookmark),
    Remove(u64, i64),
    SetTitle(u64, i64, String),
    ListContainer(u64, PathBuf),
    ListAll(u64),
    MigratePaths(u64, Vec<(PathBuf, PathBuf)>, Option<String>),
}

/// SQLite を専用スレッドに閉じ込める本ブックマークサービス。
pub struct BookBookmarkService {
    tx: Option<mpsc::Sender<BookBookmarkRequest>>,
    rx: mpsc::Receiver<BookBookmarkEvent>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BookBookmarkService {
    pub fn spawn() -> Option<Self> {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("book-bookmarks".to_string())
            .spawn(move || {
                let mut db = match BookBookmarkDb::open() {
                    Ok(db) => db,
                    Err(err) => {
                        let message = format!("book bookmarks: DB open failed: {err}");
                        crate::logger::log(&message);
                        // Service の生成後に DB open が失敗しても request を黙って失わない。
                        // UI は通常の event 経路で失敗を表示し、読み込み spinner も解除できる。
                        while let Ok(request) = request_rx.recv() {
                            let event = match request {
                                BookBookmarkRequest::Add(request_id, _) => {
                                    BookBookmarkEvent::Added {
                                        request_id,
                                        result: Err(message.clone()),
                                    }
                                }
                                BookBookmarkRequest::Remove(request_id, _) => {
                                    BookBookmarkEvent::Removed {
                                        request_id,
                                        result: Err(message.clone()),
                                    }
                                }
                                BookBookmarkRequest::SetTitle(request_id, _, _) => {
                                    BookBookmarkEvent::TitleUpdated {
                                        request_id,
                                        result: Err(message.clone()),
                                    }
                                }
                                BookBookmarkRequest::ListContainer(request_id, path) => {
                                    BookBookmarkEvent::ContainerListed {
                                        request_id,
                                        container_key: container_key(&path),
                                        result: Err(message.clone()),
                                    }
                                }
                                BookBookmarkRequest::ListAll(request_id) => {
                                    BookBookmarkEvent::AllListed {
                                        request_id,
                                        result: Err(message.clone()),
                                    }
                                }
                                BookBookmarkRequest::MigratePaths(request_id, _, _) => {
                                    BookBookmarkEvent::PathsMigrated {
                                        request_id,
                                        result: Err(message.clone()),
                                    }
                                }
                            };
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        return;
                    }
                };
                db.recover_path_migration_journal();
                while let Ok(request) = request_rx.recv() {
                    let event = match request {
                        BookBookmarkRequest::Add(request_id, entry) => BookBookmarkEvent::Added {
                            request_id,
                            result: db.add(&entry).map_err(|err| err.to_string()),
                        },
                        BookBookmarkRequest::Remove(request_id, id) => BookBookmarkEvent::Removed {
                            request_id,
                            result: db.remove(id).map(|_| id).map_err(|err| err.to_string()),
                        },
                        BookBookmarkRequest::SetTitle(request_id, id, title) => {
                            BookBookmarkEvent::TitleUpdated {
                                request_id,
                                result: db
                                    .set_title(id, Some(&title))
                                    .map(|title| (id, title))
                                    .map_err(|err| err.to_string()),
                            }
                        }
                        BookBookmarkRequest::ListContainer(request_id, path) => {
                            let container_key = container_key(&path);
                            BookBookmarkEvent::ContainerListed {
                                request_id,
                                container_key,
                                result: db.list_for_container(&path).map_err(|err| err.to_string()),
                            }
                        }
                        BookBookmarkRequest::ListAll(request_id) => BookBookmarkEvent::AllListed {
                            request_id,
                            result: db.list_all().map_err(|err| err.to_string()),
                        },
                        BookBookmarkRequest::MigratePaths(request_id, mappings, journal_id) => {
                            BookBookmarkEvent::PathsMigrated {
                                request_id,
                                result: db
                                    .migrate_paths_with_journal(&mappings, journal_id.as_deref())
                                    .map_err(|err| err.to_string()),
                            }
                        }
                    };
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            tx: Some(request_tx),
            rx: event_rx,
            handle: Some(handle),
        })
    }

    pub fn add(&self, request_id: u64, entry: NewBookBookmark) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::Add(request_id, entry));
        }
    }

    pub fn remove(&self, request_id: u64, id: i64) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::Remove(request_id, id));
        }
    }

    pub fn set_title(&self, request_id: u64, id: i64, title: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::SetTitle(request_id, id, title));
        }
    }

    pub fn list_for_container(&self, request_id: u64, path: PathBuf) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::ListContainer(request_id, path));
        }
    }

    pub fn list_all(&self, request_id: u64) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::ListAll(request_id));
        }
    }

    pub fn migrate_paths(&self, request_id: u64, mappings: Vec<(PathBuf, PathBuf)>) {
        self.migrate_paths_with_journal(request_id, mappings, None);
    }

    pub fn migrate_paths_with_journal(
        &self,
        request_id: u64,
        mappings: Vec<(PathBuf, PathBuf)>,
        journal_id: Option<String>,
    ) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::MigratePaths(
                request_id, mappings, journal_id,
            ));
        }
    }

    pub fn try_recv(&self) -> Result<BookBookmarkEvent, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for BookBookmarkService {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct BookBookmarkDb {
    conn: rusqlite::Connection,
}

impl BookBookmarkDb {
    fn open() -> Result<Self, rusqlite::Error> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::open_at(&path)
    }

    fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS book_bookmarks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                container_key   TEXT NOT NULL,
                container_path  TEXT NOT NULL,
                container_kind  TEXT NOT NULL,
                page_kind       TEXT NOT NULL,
                page_value      TEXT NOT NULL,
                page_key        TEXT NOT NULL,
                page_index_hint INTEGER NOT NULL DEFAULT 0,
                created_at_ms   INTEGER NOT NULL,
                title           TEXT,
                UNIQUE(container_key, page_kind, page_key)
             );
             CREATE INDEX IF NOT EXISTS idx_book_bookmarks_container
                ON book_bookmarks(container_key);
             CREATE INDEX IF NOT EXISTS idx_book_bookmarks_created
                ON book_bookmarks(created_at_ms DESC);
             CREATE TABLE IF NOT EXISTS book_bookmark_path_migrations (
                job_id          TEXT PRIMARY KEY,
                mappings_json   TEXT NOT NULL,
                operation_json  TEXT,
                phase           TEXT NOT NULL DEFAULT 'legacy_ambiguous',
                next_step       INTEGER NOT NULL DEFAULT 0,
                diagnostic      TEXT,
                created_at_ms   INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_book_bookmark_path_migrations_created
                ON book_bookmark_path_migrations(created_at_ms, job_id);",
        )?;
        ensure_column(conn, "book_bookmarks", "title", "TEXT")?;
        // Rows created by the pre-phase v2.7 development build cannot be
        // classified safely from path existence (swap/cycle are ambiguous).
        // Keep them as legacy_ambiguous for diagnosis instead of guessing.
        ensure_column(
            conn,
            "book_bookmark_path_migrations",
            "operation_json",
            "TEXT",
        )?;
        ensure_column(
            conn,
            "book_bookmark_path_migrations",
            "phase",
            "TEXT NOT NULL DEFAULT 'legacy_ambiguous'",
        )?;
        ensure_column(
            conn,
            "book_bookmark_path_migrations",
            "next_step",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(conn, "book_bookmark_path_migrations", "diagnostic", "TEXT")
    }

    fn add(&self, entry: &NewBookBookmark) -> Result<(BookBookmark, bool), rusqlite::Error> {
        let key = container_key(&entry.container_path);
        let (page_kind, page_value, page_key) = entry.page_identity.storage_parts();
        let created_at_ms = now_ms();
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO book_bookmarks
                (container_key, container_path, container_kind, page_kind, page_value,
                 page_key, page_index_hint, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                key,
                entry.container_path.to_string_lossy().as_ref(),
                entry.container_kind.as_str(),
                page_kind,
                page_value,
                page_key,
                entry.page_index_hint.min(i64::MAX as usize) as i64,
                created_at_ms,
            ],
        )? > 0;
        let bookmark = self.conn.query_row(
            "SELECT id, container_key, container_path, container_kind, page_kind,
                    page_value, page_index_hint, created_at_ms, title
               FROM book_bookmarks
              WHERE container_key = ?1 AND page_kind = ?2 AND page_key = ?3",
            rusqlite::params![key, page_kind, page_key],
            row_to_bookmark,
        )?;
        Ok((bookmark, inserted))
    }

    fn remove(&self, id: i64) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM book_bookmarks WHERE id = ?1", [id])?;
        Ok(())
    }

    fn set_title(&self, id: i64, title: Option<&str>) -> Result<Option<String>, rusqlite::Error> {
        let title = normalize_bookmark_title(title);
        self.conn.execute(
            "UPDATE book_bookmarks SET title = ?1 WHERE id = ?2",
            rusqlite::params![title.as_deref(), id],
        )?;
        Ok(title)
    }

    fn list_for_container(&self, path: &Path) -> Result<Vec<BookBookmark>, rusqlite::Error> {
        let key = container_key(path);
        self.list_query(
            "SELECT id, container_key, container_path, container_kind, page_kind,
                    page_value, page_index_hint, created_at_ms, title
               FROM book_bookmarks
              WHERE container_key = ?1
              ORDER BY page_index_hint ASC, created_at_ms ASC",
            Some(key),
        )
    }

    fn list_all(&self) -> Result<Vec<BookBookmark>, rusqlite::Error> {
        self.list_query(
            "SELECT id, container_key, container_path, container_kind, page_kind,
                    page_value, page_index_hint, created_at_ms, title
               FROM book_bookmarks
              ORDER BY created_at_ms DESC, id DESC",
            None,
        )
    }

    fn list_query(
        &self,
        sql: &str,
        key: Option<String>,
    ) -> Result<Vec<BookBookmark>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = match key {
            Some(key) => stmt.query_map([key], row_to_bookmark)?,
            None => stmt.query_map([], row_to_bookmark)?,
        };
        rows.collect()
    }

    /// 製本ページのリネーム・並べ替え・別の本への移動を、安定したページ identity へ反映する。
    /// 全対象行を一度 internal key へ退避してから確定するため、ページ同士の入れ替えでも
    /// UNIQUE(container_key, page_kind, page_key) の一時衝突を起こさない。
    fn migrate_paths(&mut self, mappings: &[(PathBuf, PathBuf)]) -> Result<usize, rusqlite::Error> {
        self.migrate_paths_with_journal(mappings, None)
    }

    /// `journal_id` がある場合、path migration と journal 消去を同じ SQLite transaction で
    /// commit する。DB busy / 一時失敗 / panic では transaction が完了せず journal が残るため、
    /// 次回起動の `recover_path_migration_journal` が同じ最終 mapping を冪等に再実行できる。
    fn migrate_paths_with_journal(
        &mut self,
        mappings: &[(PathBuf, PathBuf)],
        journal_id: Option<&str>,
    ) -> Result<usize, rusqlite::Error> {
        if mappings.is_empty() && journal_id.is_none() {
            return Ok(0);
        }
        // A journaled completion must use the durable mapping, not the UI
        // message copy. This keeps filesystem and bookmark identity coupled
        // even after a restart or a stale completion event.
        let journal_mappings = if let Some(journal_id) = journal_id {
            let (phase, mappings_json): (String, String) = self.conn.query_row(
                "SELECT phase, mappings_json
                   FROM book_bookmark_path_migrations
                  WHERE job_id = ?1",
                [journal_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if phase != MIGRATION_PHASE_FILESYSTEM_COMMITTED {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Some(
                serde_json::from_str::<Vec<(PathBuf, PathBuf)>>(&mappings_json).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
            )
        } else {
            None
        };
        let mappings = PathMappingIndex::new(journal_mappings.as_deref().unwrap_or(mappings));
        let targets: Vec<_> = self
            .list_all()?
            .into_iter()
            .filter_map(|bookmark| migration_target(&bookmark, &mappings))
            .collect();
        if targets.is_empty() && journal_id.is_none() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        for target in &targets {
            tx.execute(
                "UPDATE book_bookmarks SET container_key = ?1 WHERE id = ?2",
                rusqlite::params![
                    format!("miv-internal://bookmark-migration/{}", target.id),
                    target.id
                ],
            )?;
        }
        for target in &targets {
            let conflict = tx.query_row(
                "SELECT id FROM book_bookmarks
                  WHERE container_key = ?1 AND page_kind = ?2 AND page_key = ?3 AND id <> ?4
                  LIMIT 1",
                rusqlite::params![
                    target.container_key,
                    target.page_kind,
                    target.page_key,
                    target.id
                ],
                |row| row.get::<_, i64>(0),
            );
            match conflict {
                Ok(_) => {
                    // 改名後の identity に既存行がある場合は、共通 rename migration と同様に
                    // 新側を正本として旧ブックマークを捨てる。
                    tx.execute("DELETE FROM book_bookmarks WHERE id = ?1", [target.id])?;
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    tx.execute(
                        "UPDATE book_bookmarks
                            SET container_key = ?1, container_path = ?2,
                                page_kind = ?3, page_value = ?4, page_key = ?5,
                                page_index_hint = ?6
                          WHERE id = ?7",
                        rusqlite::params![
                            target.container_key,
                            target.container_path.to_string_lossy().as_ref(),
                            target.page_kind,
                            target.page_value,
                            target.page_key,
                            target.page_index_hint.min(i64::MAX as usize) as i64,
                            target.id,
                        ],
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(journal_id) = journal_id {
            tx.execute(
                "DELETE FROM book_bookmark_path_migrations WHERE job_id = ?1",
                [journal_id],
            )?;
        }
        tx.commit()?;
        Ok(targets.len())
    }

    fn recover_path_migration_journal(&mut self) {
        let pending = (|| -> Result<Vec<PathMigrationJournalEntry>, rusqlite::Error> {
            let mut stmt = self.conn.prepare(
                "SELECT job_id, mappings_json, operation_json, phase, next_step
                   FROM book_bookmark_path_migrations
                  ORDER BY created_at_ms ASC, job_id ASC",
            )?;
            stmt.query_map([], |row| {
                let next_step = row.get::<_, i64>(4)?.max(0) as usize;
                Ok(PathMigrationJournalEntry {
                    job_id: row.get(0)?,
                    mappings_json: row.get(1)?,
                    operation_json: row.get(2)?,
                    phase: row.get(3)?,
                    next_step,
                })
            })?
            .collect()
        })();
        let pending = match pending {
            Ok(pending) => pending,
            Err(error) => {
                crate::logger::log(format!(
                    "book bookmark migration journal read failed: {error}"
                ));
                return;
            }
        };
        for entry in pending {
            let job_id = entry.job_id.as_str();
            if path_migration_is_active(job_id) {
                crate::logger::log(format!(
                    "book bookmark migration recovery skipped active writer job={job_id}"
                ));
                continue;
            }
            match entry.phase.as_str() {
                MIGRATION_PHASE_PREPARED => {
                    // No filesystem decision was made. Prepared is the only
                    // phase that can be discarded without inspecting paths.
                    match self.conn.execute(
                        "DELETE FROM book_bookmark_path_migrations
                          WHERE job_id = ?1 AND phase = ?2",
                        rusqlite::params![job_id, MIGRATION_PHASE_PREPARED],
                    ) {
                        Ok(1) => crate::logger::log(format!(
                            "book bookmark migration prepared intent discarded job={job_id}"
                        )),
                        Ok(_) => crate::logger::log(format!(
                            "book bookmark migration prepared intent changed during recovery job={job_id}"
                        )),
                        Err(error) => crate::logger::log(format!(
                            "book bookmark migration prepared discard failed job={job_id}: {error}"
                        )),
                    }
                }
                MIGRATION_PHASE_APPLYING => {
                    let Some(plan) = self.parse_filesystem_plan(&entry) else {
                        continue;
                    };
                    let result = crate::book_fs_journal::execute_forward(
                        &plan,
                        entry.next_step,
                        |next_step| {
                            self.update_journal_progress(
                                job_id,
                                MIGRATION_PHASE_APPLYING,
                                next_step,
                            )
                        },
                    );
                    if let Err(error) = result {
                        self.keep_journal_diagnostic(job_id, &error.message);
                        continue;
                    }
                    if let Err(error) = self.transition_journal_phase(
                        job_id,
                        MIGRATION_PHASE_APPLYING,
                        MIGRATION_PHASE_FILESYSTEM_COMMITTED,
                        plan.len(),
                        None,
                    ) {
                        self.keep_journal_diagnostic(job_id, &error);
                        continue;
                    }
                    self.finish_recovered_journal(job_id, &entry.mappings_json);
                }
                MIGRATION_PHASE_ROLLING_BACK => {
                    let Some(plan) = self.parse_filesystem_plan(&entry) else {
                        continue;
                    };
                    let result = crate::book_fs_journal::execute_rollback(
                        &plan,
                        entry.next_step,
                        |next_step| {
                            self.update_journal_progress(
                                job_id,
                                MIGRATION_PHASE_ROLLING_BACK,
                                next_step,
                            )
                        },
                    );
                    match result {
                        Ok(()) => match self.conn.execute(
                            "DELETE FROM book_bookmark_path_migrations
                              WHERE job_id = ?1 AND phase = ?2 AND next_step = 0",
                            rusqlite::params![job_id, MIGRATION_PHASE_ROLLING_BACK],
                        ) {
                            Ok(1) => crate::logger::log(format!(
                                "book bookmark migration rollback recovered job={job_id}"
                            )),
                            Ok(_) => crate::logger::log(format!(
                                "book bookmark migration rollback completion changed job={job_id}"
                            )),
                            Err(error) => self.keep_journal_diagnostic(
                                job_id,
                                &format!("rollback journal discard failed: {error}"),
                            ),
                        },
                        Err(error) => self.keep_journal_diagnostic(job_id, &error.message),
                    }
                }
                MIGRATION_PHASE_FILESYSTEM_COMMITTED => {
                    self.finish_recovered_journal(job_id, &entry.mappings_json);
                }
                phase => {
                    // Pre-phase development rows are intrinsically ambiguous
                    // for swap/cycle operations. Preserve them for diagnosis.
                    self.keep_journal_diagnostic(
                        job_id,
                        &format!("unsupported or legacy migration phase: {phase}"),
                    );
                }
            }
        }
    }

    fn parse_filesystem_plan(
        &self,
        entry: &PathMigrationJournalEntry,
    ) -> Option<crate::book_fs_journal::BookFsOperationPlan> {
        let Some(operation_json) = entry.operation_json.as_deref() else {
            self.keep_journal_diagnostic(&entry.job_id, "filesystem operation plan is missing");
            return None;
        };
        match serde_json::from_str(operation_json) {
            Ok(plan) => Some(plan),
            Err(error) => {
                self.keep_journal_diagnostic(
                    &entry.job_id,
                    &format!("filesystem operation plan parse failed: {error}"),
                );
                None
            }
        }
    }

    fn update_journal_progress(
        &self,
        job_id: &str,
        phase: &str,
        next_step: usize,
    ) -> Result<(), String> {
        let changed = self
            .conn
            .execute(
                "UPDATE book_bookmark_path_migrations
                    SET next_step = ?1, diagnostic = NULL
                  WHERE job_id = ?2 AND phase = ?3",
                rusqlite::params![next_step.min(i64::MAX as usize) as i64, job_id, phase],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            Ok(())
        } else {
            Err(format!(
                "journal phase changed while recording progress: {phase}"
            ))
        }
    }

    fn transition_journal_phase(
        &self,
        job_id: &str,
        from: &str,
        to: &str,
        next_step: usize,
        diagnostic: Option<&str>,
    ) -> Result<(), String> {
        let changed = self
            .conn
            .execute(
                "UPDATE book_bookmark_path_migrations
                    SET phase = ?1, next_step = ?2, diagnostic = ?3
                  WHERE job_id = ?4 AND phase = ?5",
                rusqlite::params![
                    to,
                    next_step.min(i64::MAX as usize) as i64,
                    diagnostic,
                    job_id,
                    from
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            Ok(())
        } else {
            Err(format!("journal phase transition rejected: {from} -> {to}"))
        }
    }

    fn keep_journal_diagnostic(&self, job_id: &str, diagnostic: &str) {
        let _ = self.conn.execute(
            "UPDATE book_bookmark_path_migrations SET diagnostic = ?1 WHERE job_id = ?2",
            rusqlite::params![diagnostic, job_id],
        );
        crate::logger::log(format!(
            "book bookmark migration journal retained job={job_id}: {diagnostic}"
        ));
    }

    fn finish_recovered_journal(&mut self, job_id: &str, mappings_json: &str) {
        let mappings: Vec<(PathBuf, PathBuf)> = match serde_json::from_str(mappings_json) {
            Ok(mappings) => mappings,
            Err(error) => {
                self.keep_journal_diagnostic(
                    job_id,
                    &format!("bookmark mapping parse failed: {error}"),
                );
                return;
            }
        };
        match self.migrate_paths_with_journal(&mappings, Some(job_id)) {
            Ok(rows) => crate::logger::log(format!(
                "book bookmark migration journal recovered job={job_id} rows={rows}"
            )),
            Err(error) => self.keep_journal_diagnostic(
                job_id,
                &format!("bookmark migration retry failed: {error}"),
            ),
        }
    }
}

#[derive(Debug)]
struct BookmarkMigrationTarget {
    id: i64,
    container_key: String,
    container_path: PathBuf,
    page_kind: String,
    page_value: String,
    page_key: String,
    page_index_hint: usize,
}

fn migration_target(
    bookmark: &BookBookmark,
    mappings: &PathMappingIndex,
) -> Option<BookmarkMigrationTarget> {
    let (mut container_path, container_changed) = mappings.map_path(&bookmark.container_path);
    let mut page_identity = bookmark.page_identity.clone();
    let mut page_index_hint = bookmark.page_index_hint;

    if let PageIdentity::RelativePath(relative) = &bookmark.page_identity {
        let source_page = bookmark
            .container_path
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let (mapped_page, page_changed) = mappings.map_path(&source_page);
        if page_changed {
            if bookmark.container_kind == BookContainerKind::CompiledBook
                && !path_is_within(&mapped_page, &container_path)
                && let Some(parent) = mapped_page.parent()
            {
                container_path = parent.to_path_buf();
            }
            if let Some(new_relative) = relative_to(&mapped_page, &container_path) {
                page_identity =
                    PageIdentity::RelativePath(new_relative.to_string_lossy().replace('\\', "/"));
            }
            if bookmark.container_kind == BookContainerKind::CompiledBook {
                page_index_hint = compiled_page_index(&mapped_page).unwrap_or(page_index_hint);
            }
        }
    }

    let container_key = container_key(&container_path);
    let (page_kind, page_value, page_key) = page_identity.storage_parts();
    let raw_changed = container_path != bookmark.container_path || container_changed;
    let identity_changed = page_identity != bookmark.page_identity;
    let hint_changed = page_index_hint != bookmark.page_index_hint;
    (raw_changed || container_key != bookmark.container_key || identity_changed || hint_changed)
        .then_some(BookmarkMigrationTarget {
            id: bookmark.id,
            container_key,
            container_path,
            page_kind: page_kind.to_string(),
            page_value,
            page_key,
            page_index_hint,
        })
}

struct PathMappingIndex {
    /// 製本の全ページ並べ替えでも bookmark × mapping の全走査をしない。exact は O(1)、
    /// フォルダ rename は path の ancestor (通常は数段) だけを調べる。
    by_from: HashMap<String, (usize, PathBuf)>,
}

impl PathMappingIndex {
    fn new(mappings: &[(PathBuf, PathBuf)]) -> Self {
        let mut by_from = HashMap::with_capacity(mappings.len());
        for (from, to) in mappings {
            by_from
                .entry(crate::path_key::normalize_keep_drive(from))
                .or_insert_with(|| (from.components().count(), to.clone()));
        }
        Self { by_from }
    }

    fn map_path(&self, path: &Path) -> (PathBuf, bool) {
        for candidate in path.ancestors() {
            let key = crate::path_key::normalize_keep_drive(candidate);
            let Some((component_count, to)) = self.by_from.get(&key) else {
                continue;
            };
            if candidate == path {
                return (to.clone(), true);
            }
            let suffix = path
                .components()
                .skip(*component_count)
                .collect::<PathBuf>();
            return (to.join(suffix), true);
        }
        (path.to_path_buf(), false)
    }
}

fn path_is_within(path: &Path, container: &Path) -> bool {
    let path_key = crate::path_key::normalize_keep_drive(path);
    let container_key = crate::path_key::normalize_keep_drive(container);
    path_key.starts_with(&format!("{container_key}/"))
}

fn relative_to(path: &Path, container: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(container) {
        return Some(relative.to_path_buf());
    }
    path_is_within(path, container).then(|| {
        path.components()
            .skip(container.components().count())
            .collect()
    })
}

fn compiled_page_index(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    let prefix = name.split_once('_')?.0;
    (prefix.len() == 4)
        .then(|| prefix.parse::<usize>().ok()?.checked_sub(1))
        .flatten()
}

fn row_to_bookmark(row: &rusqlite::Row<'_>) -> Result<BookBookmark, rusqlite::Error> {
    let container_kind_value: String = row.get(3)?;
    let page_kind: String = row.get(4)?;
    let page_value: String = row.get(5)?;
    let container_kind = BookContainerKind::from_str(&container_kind_value).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(
            3,
            "container_kind".to_string(),
            rusqlite::types::Type::Text,
        )
    })?;
    let page_identity = PageIdentity::from_storage(&page_kind, page_value).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(4, "page_kind".to_string(), rusqlite::types::Type::Text)
    })?;
    let page_index_hint: i64 = row.get(6)?;
    Ok(BookBookmark {
        id: row.get(0)?,
        container_key: row.get(1)?,
        container_path: PathBuf::from(row.get::<_, String>(2)?),
        container_kind,
        page_identity,
        page_index_hint: page_index_hint.max(0) as usize,
        created_at_ms: row.get(7)?,
        title: row.get(8)?,
    })
}

fn ensure_column(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut found = false;
        for name in rows {
            if name? == column {
                found = true;
                break;
            }
        }
        found
    };
    if exists {
        return Ok(());
    }
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column") =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn normalize_bookmark_title(title: Option<&str>) -> Option<String> {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

pub fn db_path() -> PathBuf {
    crate::data_dir::get().join("book_bookmarks.db")
}

pub(crate) fn new_path_migration_job_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Persist the complete deterministic filesystem plan while it is still in the
/// no-op `prepared` phase. The caller must durably transition to `applying`
/// before executing the first step.
pub(crate) fn prepare_path_migration(
    job_id: &str,
    mappings: &[(PathBuf, PathBuf)],
    plan: &crate::book_fs_journal::BookFsOperationPlan,
) -> Result<Option<PathMigrationJournalWriter>, String> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let db = BookBookmarkDb::open_at(&path).map_err(|error| error.to_string())?;
    register_active_path_migration(job_id);
    match insert_prepared_path_migration(&db, job_id, mappings, plan) {
        Ok(true) => {}
        Ok(false) => {
            unregister_active_path_migration(job_id);
            return Ok(None);
        }
        Err(error) => {
            unregister_active_path_migration(job_id);
            return Err(error);
        }
    }
    Ok(Some(PathMigrationJournalWriter {
        db,
        job_id: job_id.to_string(),
    }))
}

#[cfg(test)]
fn prepare_path_migration_at(
    db_path: &Path,
    job_id: &str,
    mappings: &[(PathBuf, PathBuf)],
    plan: &crate::book_fs_journal::BookFsOperationPlan,
) -> Result<Option<String>, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let db = BookBookmarkDb::open_at(db_path).map_err(|error| error.to_string())?;
    if !insert_prepared_path_migration(&db, job_id, mappings, plan)? {
        return Ok(None);
    }
    Ok(Some(job_id.to_string()))
}

fn insert_prepared_path_migration(
    db: &BookBookmarkDb,
    job_id: &str,
    mappings: &[(PathBuf, PathBuf)],
    plan: &crate::book_fs_journal::BookFsOperationPlan,
) -> Result<bool, String> {
    let mappings = mappings
        .iter()
        .filter(|(from, to)| !crate::folder_tree::path_eq(from, to))
        .cloned()
        .collect::<Vec<_>>();
    if mappings.is_empty() && plan.is_empty() {
        return Ok(false);
    }
    let mappings_json = serde_json::to_string(&mappings).map_err(|error| error.to_string())?;
    let operation_json = serde_json::to_string(plan).map_err(|error| error.to_string())?;
    db.conn
        .execute(
            "INSERT INTO book_bookmark_path_migrations
                (job_id, mappings_json, operation_json, phase, next_step, diagnostic, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, 0, NULL, ?5)",
            rusqlite::params![
                job_id,
                mappings_json,
                operation_json,
                MIGRATION_PHASE_PREPARED,
                now_ms()
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(true)
}

pub(crate) struct PathMigrationJournalWriter {
    db: BookBookmarkDb,
    job_id: String,
}

impl PathMigrationJournalWriter {
    pub(crate) fn job_id(&self) -> &str {
        &self.job_id
    }

    pub(crate) fn begin(&self) -> Result<(), String> {
        self.db.transition_journal_phase(
            &self.job_id,
            MIGRATION_PHASE_PREPARED,
            MIGRATION_PHASE_APPLYING,
            0,
            None,
        )
    }

    pub(crate) fn record_progress(
        &self,
        rolling_back: bool,
        next_step: usize,
    ) -> Result<(), String> {
        self.db.update_journal_progress(
            &self.job_id,
            if rolling_back {
                MIGRATION_PHASE_ROLLING_BACK
            } else {
                MIGRATION_PHASE_APPLYING
            },
            next_step,
        )
    }

    pub(crate) fn begin_rollback(
        &self,
        affected_steps: usize,
        diagnostic: &str,
    ) -> Result<(), String> {
        self.db.transition_journal_phase(
            &self.job_id,
            MIGRATION_PHASE_APPLYING,
            MIGRATION_PHASE_ROLLING_BACK,
            affected_steps,
            Some(diagnostic),
        )
    }

    pub(crate) fn mark_filesystem_committed(&self, completed_steps: usize) -> Result<(), String> {
        self.db.transition_journal_phase(
            &self.job_id,
            MIGRATION_PHASE_APPLYING,
            MIGRATION_PHASE_FILESYSTEM_COMMITTED,
            completed_steps,
            None,
        )
    }

    pub(crate) fn discard_prepared(&self) -> Result<(), String> {
        let changed = self
            .db
            .conn
            .execute(
                "DELETE FROM book_bookmark_path_migrations
                  WHERE job_id = ?1 AND phase = ?2",
                rusqlite::params![self.job_id, MIGRATION_PHASE_PREPARED],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            Ok(())
        } else {
            Err("journal is no longer in prepared phase".to_string())
        }
    }

    pub(crate) fn discard_rolled_back(&self) -> Result<(), String> {
        let changed = self
            .db
            .conn
            .execute(
                "DELETE FROM book_bookmark_path_migrations
                  WHERE job_id = ?1 AND phase = ?2 AND next_step = 0",
                rusqlite::params![self.job_id, MIGRATION_PHASE_ROLLING_BACK],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            Ok(())
        } else {
            Err("rollback is not durably complete".to_string())
        }
    }

    pub(crate) fn into_job_id(self) -> String {
        self.job_id.clone()
    }
}

impl Drop for PathMigrationJournalWriter {
    fn drop(&mut self) {
        unregister_active_path_migration(&self.job_id);
    }
}

/// 指定先に本ブックマーク DB の現行 schema を用意する。
/// 明示メタ情報転送 worker とテストが、サービス専用接続を横取りせずに使うための入口。
pub fn ensure_schema_at(path: &Path) -> Result<(), rusqlite::Error> {
    drop(BookBookmarkDb::open_at(path)?);
    Ok(())
}

/// アプリ内の共通リネーム worker 用。コンテナ path と画像フォルダ内ページ identity の
/// 両方を同じ transaction で追従させる。削除時は missing 行を保持する仕様なので、
/// `rename_key_migration::STORES` の hard-purge 対象には加えない。
pub fn migrate_paths_at(
    db_path: &Path,
    mappings: &[(PathBuf, PathBuf)],
) -> Result<usize, rusqlite::Error> {
    BookBookmarkDb::open_at(db_path)?.migrate_paths(mappings)
}

/// 横断一覧 worker 用の全件読み出し。UI スレッドから直接呼ばないこと。
pub fn load_all_from_disk() -> Result<Vec<BookBookmark>, rusqlite::Error> {
    BookBookmarkDb::open()?.list_all()
}

/// 横断一覧の削除 worker 用。元コンテナやページは一切操作しない。
pub fn remove_from_disk(id: i64) -> Result<(), rusqlite::Error> {
    BookBookmarkDb::open()?.remove(id)
}

pub fn container_key(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

/// 画像フォルダ系ブックマークの相対ページを、実体の container 境界まで確認した結果。
///
/// 欠落ページはブックマークとして保持できるため `Missing` と `Unsafe` を区別する。
/// `Existing` は manifest 由来であることと検証済み trust root を保持する。後段の
/// loader は通常の path open へ戻さず、[`RelativePageProvenance::open_verified`] で
/// 開いた同一ハンドルの実体を再検証してから利用する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RelativePagePathResolution {
    Existing(RelativePageProvenance),
    Missing(PathBuf),
    Unsafe,
}

/// 信頼しない sidecar の relative page が由来とする container 境界。
///
/// `canonical_container` は materialize 時の trust root を固定する。ファイルを使う
/// ときは candidate path を検証してから開き直すのではなく、先に開いたハンドルの
/// final path がこの root 内にあることを確認し、そのハンドル自体から読む。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelativePageProvenance {
    container: PathBuf,
    relative: String,
    canonical_container: Option<PathBuf>,
}

impl RelativePageProvenance {
    /// 現在一覧に既にある item へのブックマークジャンプ用。UI スレッドでは lexical
    /// validation だけを行い、canonicalize / open は loader worker に委ねる。
    #[allow(dead_code)] // lib target does not compile the App consumers
    pub(crate) fn unresolved(container: &Path, relative: &str) -> Option<Self> {
        relative_page_candidate(container, relative)?;
        Some(Self {
            container: container.to_path_buf(),
            relative: relative.replace('\\', "/"),
            canonical_container: None,
        })
    }

    fn materialized(container: &Path, relative: &str, canonical_container: PathBuf) -> Self {
        Self {
            container: container.to_path_buf(),
            relative: relative.replace('\\', "/"),
            canonical_container: Some(canonical_container),
        }
    }

    pub(crate) fn candidate_path(&self) -> PathBuf {
        // construction 時に lexical validation 済み。
        self.container
            .join(self.relative.split('/').collect::<PathBuf>())
    }

    /// 同じ画像に対応する sidecar candidate へ trust root を引き継ぐ。
    #[allow(dead_code)] // lib target does not compile the App metadata consumer
    pub(crate) fn for_candidate(&self, candidate: &Path) -> Option<Self> {
        let relative = candidate.strip_prefix(&self.container).ok()?;
        let relative = relative.to_str()?.replace('\\', "/");
        relative_page_candidate(&self.container, &relative)?;
        Some(Self {
            container: self.container.clone(),
            relative,
            canonical_container: self.canonical_container.clone(),
        })
    }

    /// candidate を開き、開いた同一ハンドルの実体が trust root 内にある場合だけ返す。
    pub(crate) fn open_verified(&self) -> std::io::Result<VerifiedRelativePageFile> {
        let canonical_container = match &self.canonical_container {
            Some(path) => path.clone(),
            None => std::fs::canonicalize(&self.container)?,
        };
        let candidate = self.candidate_path();
        let file = std::fs::File::open(&candidate)?;
        let final_path = opened_file_final_path(&file)?;
        if !canonical_path_is_within(&final_path, &canonical_container)
            || crate::path_key::eq_keep_drive(&final_path, &canonical_container)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "relative bookmark page resolved outside its container",
            ));
        }
        Ok(VerifiedRelativePageFile { file })
    }

    pub(crate) fn read_verified(&self) -> std::io::Result<Vec<u8>> {
        self.open_verified()?.read_to_end()
    }
}

/// containment を確認した同一ハンドル。path を再 open する API は意図的に持たない。
pub(crate) struct VerifiedRelativePageFile {
    file: std::fs::File,
}

impl VerifiedRelativePageFile {
    pub(crate) fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.file.metadata()
    }

    pub(crate) fn read_to_end(mut self) -> std::io::Result<Vec<u8>> {
        self.file.rewind()?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(windows)]
fn opened_file_final_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let handle = HANDLE(file.as_raw_handle());
    let mut buffer = vec![0u16; 512];
    loop {
        let length = unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, VOLUME_NAME_DOS) };
        if length == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let length = length as usize;
        if length < buffer.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buffer[..length])));
        }
        buffer.resize(length.saturating_add(1), 0);
    }
}

#[cfg(unix)]
fn opened_file_final_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;

    let fd = file.as_raw_fd();
    for root in ["/proc/self/fd", "/dev/fd"] {
        let link = Path::new(root).join(fd.to_string());
        if let Ok(path) = std::fs::read_link(link) {
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "cannot resolve opened file descriptor path",
    ))
}

#[cfg(not(any(windows, unix)))]
fn opened_file_final_path(_file: &std::fs::File) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opened file path verification is unavailable on this platform",
    ))
}

/// 信頼できない相対ページ path を、leaf が欠落していても安全に解決する。
///
/// 完成 path の `canonicalize` だけでは、欠落 leaf の手前にある reparse point を
/// 見落とす。そこで最も近い既存 ancestor まで遡って canonicalize し、その実体が
/// canonical container 配下にあることを確認する。利用時にも同じ関数を呼び直すことで、
/// import 後に path 構造が置き換わったケースを通常の missing/invalid として扱える。
pub(crate) fn resolve_relative_page_path(
    container: &Path,
    relative: &str,
) -> RelativePagePathResolution {
    let Some(candidate) = relative_page_candidate(container, relative) else {
        return RelativePagePathResolution::Unsafe;
    };
    let canonical_container = match std::fs::canonicalize(container) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RelativePagePathResolution::Missing(candidate);
        }
        Err(_) => return RelativePagePathResolution::Unsafe,
    };

    let mut probe = candidate.as_path();
    let mut leaf_missing = false;
    loop {
        match std::fs::canonicalize(probe) {
            Ok(canonical_probe) => {
                if !canonical_path_is_within(&canonical_probe, &canonical_container) {
                    return RelativePagePathResolution::Unsafe;
                }
                if !leaf_missing
                    && crate::path_key::eq_keep_drive(&canonical_probe, &canonical_container)
                {
                    // ページ自体が container directory へ解決される値は画像ではない。
                    return RelativePagePathResolution::Unsafe;
                }
                return if leaf_missing {
                    RelativePagePathResolution::Missing(candidate)
                } else {
                    RelativePagePathResolution::Existing(RelativePageProvenance::materialized(
                        container,
                        relative,
                        canonical_container,
                    ))
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                leaf_missing = true;
                let Some(parent) = probe.parent() else {
                    return RelativePagePathResolution::Unsafe;
                };
                probe = parent;
            }
            Err(_) => return RelativePagePathResolution::Unsafe,
        }
    }
}

fn relative_page_candidate(container: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.chars().count() > 32_768 || relative.contains('\0') {
        return None;
    }
    let normalized = relative.replace('\\', "/");
    let has_drive_prefix = normalized.as_bytes().get(1) == Some(&b':')
        && normalized
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic);
    if normalized.starts_with('/')
        || has_drive_prefix
        || normalized
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    let relative_path = normalized.split('/').collect::<PathBuf>();
    Some(container.join(relative_path))
}

fn canonical_path_is_within(path: &Path, container: &Path) -> bool {
    let path_key = crate::path_key::normalize_keep_drive(path);
    let container_key = crate::path_key::normalize_keep_drive(container);
    if path_key == container_key {
        return true;
    }
    let prefix = if container_key.ends_with('/') {
        container_key
    } else {
        format!("{container_key}/")
    };
    path_key.starts_with(&prefix)
}

fn normalize_page_path(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory() -> BookBookmarkDb {
        let conn = rusqlite::Connection::open_in_memory().expect("memory DB");
        BookBookmarkDb::init_schema(&conn).expect("schema");
        BookBookmarkDb { conn }
    }

    #[cfg(windows)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(unix)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[test]
    fn relative_page_candidate_rejects_escape_syntax() {
        let container = Path::new("C:/Books/album");
        for unsafe_path in [
            "",
            "/absolute.jpg",
            r"C:\absolute.jpg",
            r"\\server\share\outside.jpg",
            "../outside.jpg",
            "chapter/../../outside.jpg",
            "chapter//page.jpg",
            "chapter/./page.jpg",
        ] {
            assert!(
                relative_page_candidate(container, unsafe_path).is_none(),
                "accepted {unsafe_path:?}"
            );
        }
        let candidate = relative_page_candidate(container, r"chapter\page.jpg").unwrap();
        assert_eq!(
            crate::path_key::normalize_keep_drive(&candidate),
            "c:/books/album/chapter/page.jpg"
        );
    }

    #[test]
    fn canonical_containment_uses_component_boundary() {
        let container = Path::new("C:/Books/album");
        assert!(canonical_path_is_within(container, container));
        assert!(canonical_path_is_within(
            Path::new("C:/Books/album/chapter/page.jpg"),
            container
        ));
        assert!(!canonical_path_is_within(
            Path::new("C:/Books/album-other/page.jpg"),
            container
        ));
    }

    #[test]
    fn relative_page_resolution_allows_safe_missing_and_existing_pages() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        std::fs::create_dir_all(album.join("chapter")).unwrap();

        assert!(matches!(
            resolve_relative_page_path(&album, "chapter/future.jpg"),
            RelativePagePathResolution::Missing(path)
                if path == album.join("chapter").join("future.jpg")
        ));

        std::fs::write(album.join("chapter/page.jpg"), b"page").unwrap();
        assert!(matches!(
            resolve_relative_page_path(&album, "chapter/page.jpg"),
            RelativePagePathResolution::Existing(provenance)
                if provenance.candidate_path() == album.join("chapter").join("page.jpg")
        ));
        let RelativePagePathResolution::Existing(provenance) =
            resolve_relative_page_path(&album, "chapter/page.jpg")
        else {
            panic!("existing page provenance");
        };
        assert_eq!(provenance.read_verified().unwrap(), b"page");
    }

    #[test]
    fn relative_page_resolution_rejects_missing_leaf_below_external_link() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        if !create_dir_link(&outside, &album.join("link")) {
            // Windows Developer Mode / symlink privilege がない環境では作成不能。
            return;
        }

        assert_eq!(
            resolve_relative_page_path(&album, "link/future.jpg"),
            RelativePagePathResolution::Unsafe
        );
    }

    #[test]
    fn relative_page_resolution_allows_internal_link() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let inside = album.join("inside");
        std::fs::create_dir_all(&inside).unwrap();
        if !create_dir_link(&inside, &album.join("link")) {
            return;
        }

        assert!(matches!(
            resolve_relative_page_path(&album, "link/future.jpg"),
            RelativePagePathResolution::Missing(_)
        ));

        std::fs::write(inside.join("page.jpg"), b"inside").unwrap();
        let RelativePagePathResolution::Existing(provenance) =
            resolve_relative_page_path(&album, "link/page.jpg")
        else {
            panic!("internal link page should materialize");
        };
        assert_eq!(provenance.read_verified().unwrap(), b"inside");
    }

    #[test]
    fn verified_relative_page_read_rejects_swap_after_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let chapter = album.join("chapter");
        let parked = album.join("chapter-safe");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&chapter).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(chapter.join("page.jpg"), b"inside").unwrap();
        std::fs::write(outside.join("page.jpg"), b"outside-secret").unwrap();

        let RelativePagePathResolution::Existing(provenance) =
            resolve_relative_page_path(&album, "chapter/page.jpg")
        else {
            panic!("safe page should materialize");
        };
        std::fs::rename(&chapter, &parked).unwrap();
        if !create_dir_link(&outside, &chapter) {
            return;
        }

        let result = provenance.read_verified();
        assert!(result.is_err(), "swapped external target must be rejected");
        assert_ne!(result.ok().as_deref(), Some(b"outside-secret".as_slice()));
    }

    #[test]
    fn image_folder_identity_is_filename_based_and_deduplicated() {
        let db = open_in_memory();
        let first = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/One"),
            container_kind: BookContainerKind::ImageFolder,
            page_identity: PageIdentity::RelativePath("Chapter/001.JPG".to_string()),
            page_index_hint: 7,
        };
        let (saved, inserted) = db.add(&first).expect("first add");
        assert!(inserted);
        assert_eq!(saved.page_index_hint, 7);

        let duplicate = NewBookBookmark {
            container_path: PathBuf::from("c:\\books\\one"),
            container_kind: BookContainerKind::ImageFolder,
            page_identity: PageIdentity::RelativePath("chapter\\001.jpg".to_string()),
            page_index_hint: 99,
        };
        let (same, inserted) = db.add(&duplicate).expect("duplicate add");
        assert!(!inserted);
        assert_eq!(same.id, saved.id);
        assert_eq!(db.list_all().unwrap().len(), 1);
    }

    #[test]
    fn archive_entry_and_pdf_page_round_trip() {
        let db = open_in_memory();
        let archive = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/a.cbz"),
            container_kind: BookContainerKind::Zip,
            page_identity: PageIdentity::ArchiveEntry("第1話/010.png".to_string()),
            page_index_hint: 9,
        };
        let pdf = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/a.pdf"),
            container_kind: BookContainerKind::Pdf,
            page_identity: PageIdentity::PdfPage(42),
            page_index_hint: 42,
        };
        db.add(&archive).unwrap();
        db.add(&pdf).unwrap();
        let archive_rows = db.list_for_container(&archive.container_path).unwrap();
        assert_eq!(archive_rows[0].page_identity, archive.page_identity);
        let pdf_rows = db.list_for_container(&pdf.container_path).unwrap();
        assert_eq!(pdf_rows[0].page_identity, pdf.page_identity);
    }

    #[test]
    fn remove_deletes_only_bookmark_row() {
        let db = open_in_memory();
        let entry = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/a.pdf"),
            container_kind: BookContainerKind::Pdf,
            page_identity: PageIdentity::PdfPage(1),
            page_index_hint: 1,
        };
        let (saved, _) = db.add(&entry).unwrap();
        db.remove(saved.id).unwrap();
        assert!(db.list_all().unwrap().is_empty());
    }

    #[test]
    fn title_is_trimmed_persisted_and_can_be_cleared() {
        let db = open_in_memory();
        let entry = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/a.pdf"),
            container_kind: BookContainerKind::Pdf,
            page_identity: PageIdentity::PdfPage(2),
            page_index_hint: 2,
        };
        let (saved, _) = db.add(&entry).unwrap();

        assert_eq!(
            db.set_title(saved.id, Some("  重要なページ  ")).unwrap(),
            Some("重要なページ".to_string())
        );
        assert_eq!(
            db.list_all().unwrap()[0].title.as_deref(),
            Some("重要なページ")
        );

        assert_eq!(db.set_title(saved.id, Some("   ")).unwrap(), None);
        assert_eq!(db.list_all().unwrap()[0].title, None);
    }

    #[test]
    fn path_migration_tracks_container_and_relative_page_renames() {
        let mut db = open_in_memory();
        let old_container = PathBuf::from("C:/Books/Old");
        let new_container = PathBuf::from("D:/Library/New");
        let (saved, _) = db
            .add(&NewBookBookmark {
                container_path: old_container.clone(),
                container_kind: BookContainerKind::ImageFolder,
                page_identity: PageIdentity::RelativePath("chapter/001.jpg".to_string()),
                page_index_hint: 4,
            })
            .unwrap();
        db.set_title(saved.id, Some("keep me")).unwrap();

        assert_eq!(
            db.migrate_paths(&[(old_container.clone(), new_container.clone())])
                .unwrap(),
            1
        );
        let renamed_page = new_container.join("chapter").join("cover.jpg");
        assert_eq!(
            db.migrate_paths(&[(new_container.join("chapter").join("001.jpg"), renamed_page,)])
                .unwrap(),
            1
        );

        let row = db.list_all().unwrap().pop().expect("bookmark");
        assert_eq!(row.container_path, new_container);
        assert_eq!(
            row.page_identity,
            PageIdentity::RelativePath("chapter/cover.jpg".to_string())
        );
        assert_eq!(row.title.as_deref(), Some("keep me"));
    }

    #[test]
    fn path_migration_tracks_compiled_book_reorder_without_losing_titles() {
        let mut db = open_in_memory();
        let book = PathBuf::from("C:/Books/story");
        for (name, title) in [("0001_alpha.jpg", "alpha"), ("0002_beta.jpg", "beta")] {
            let (saved, _) = db
                .add(&NewBookBookmark {
                    container_path: book.clone(),
                    container_kind: BookContainerKind::CompiledBook,
                    page_identity: PageIdentity::RelativePath(name.to_string()),
                    page_index_hint: if name.starts_with("0001") { 0 } else { 1 },
                })
                .unwrap();
            db.set_title(saved.id, Some(title)).unwrap();
        }

        let mappings = [
            (book.join("0001_alpha.jpg"), book.join("0002_alpha.jpg")),
            (book.join("0002_beta.jpg"), book.join("0001_beta.jpg")),
        ];
        assert_eq!(db.migrate_paths(&mappings).unwrap(), 2);

        let rows = db.list_all().unwrap();
        let alpha = rows
            .iter()
            .find(|row| row.title.as_deref() == Some("alpha"))
            .unwrap();
        assert_eq!(
            alpha.page_identity,
            PageIdentity::RelativePath("0002_alpha.jpg".to_string())
        );
        assert_eq!(alpha.page_index_hint, 1);
        let beta = rows
            .iter()
            .find(|row| row.title.as_deref() == Some("beta"))
            .unwrap();
        assert_eq!(
            beta.page_identity,
            PageIdentity::RelativePath("0001_beta.jpg".to_string())
        );
        assert_eq!(beta.page_index_hint, 0);
    }

    #[test]
    fn path_migration_moves_compiled_page_to_its_new_book() {
        let mut db = open_in_memory();
        let source = PathBuf::from("C:/Books/source");
        let target = PathBuf::from("C:/Books/target");
        let source_page = source.join("0002_scene.jpg");
        let target_page = target.join("0003_scene.jpg");
        db.add(&NewBookBookmark {
            container_path: source,
            container_kind: BookContainerKind::CompiledBook,
            page_identity: PageIdentity::RelativePath("0002_scene.jpg".to_string()),
            page_index_hint: 1,
        })
        .unwrap();

        assert_eq!(db.migrate_paths(&[(source_page, target_page)]).unwrap(), 1);
        let row = db.list_all().unwrap().pop().expect("bookmark");
        assert_eq!(row.container_path, target);
        assert_eq!(
            row.page_identity,
            PageIdentity::RelativePath("0003_scene.jpg".to_string())
        );
        assert_eq!(row.page_index_hint, 2);
    }

    fn journal_count(conn: &rusqlite::Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM book_bookmark_path_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn single_rename_plan(from: &Path, to: &Path) -> crate::book_fs_journal::BookFsOperationPlan {
        crate::book_fs_journal::BookFsOperationPlan::new(vec![
            crate::book_fs_journal::BookFsStep::Rename {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            },
        ])
    }

    fn add_compiled_bookmark(db: &BookBookmarkDb, book: &Path, name: &str, title: &str) {
        let (saved, _) = db
            .add(&NewBookBookmark {
                container_path: book.to_path_buf(),
                container_kind: BookContainerKind::CompiledBook,
                page_identity: PageIdentity::RelativePath(name.to_string()),
                page_index_hint: 0,
            })
            .unwrap();
        db.set_title(saved.id, Some(title)).unwrap();
    }

    #[test]
    fn prepared_journal_without_filesystem_change_is_discarded_as_noop() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("book_bookmarks.db");
        let book = temp.path().join("story");
        std::fs::create_dir(&book).unwrap();
        let old_page = book.join("0001_old.jpg");
        let new_page = book.join("0001_new.jpg");
        std::fs::write(&old_page, b"page").unwrap();

        let db = BookBookmarkDb::open_at(&db_path).unwrap();
        add_compiled_bookmark(&db, &book, "0001_old.jpg", "stay old");
        drop(db);

        let mappings = vec![(old_page.clone(), new_page.clone())];
        let plan = single_rename_plan(&old_page, &new_page);
        let job_id = "prepared-only";
        prepare_path_migration_at(&db_path, job_id, &mappings, &plan)
            .unwrap()
            .expect("journal id");

        let mut restarted = BookBookmarkDb::open_at(&db_path).unwrap();
        assert_eq!(journal_count(&restarted.conn), 1);
        restarted.recover_path_migration_journal();
        let row = restarted.list_all().unwrap().pop().unwrap();
        assert_eq!(
            row.page_identity,
            PageIdentity::RelativePath("0001_old.jpg".to_string())
        );
        assert!(old_page.exists());
        assert!(!new_page.exists());
        assert_eq!(journal_count(&restarted.conn), 0);
    }

    #[test]
    fn recovery_skips_a_journal_with_a_live_in_process_writer() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("book_bookmarks.db");
        let from = temp.path().join("from.jpg");
        let to = temp.path().join("to.jpg");
        std::fs::write(&from, b"page").unwrap();
        let mappings = vec![(from.clone(), to.clone())];
        let plan = single_rename_plan(&from, &to);
        let job_id = "live-writer";
        prepare_path_migration_at(&db_path, job_id, &mappings, &plan).unwrap();
        register_active_path_migration(job_id);

        let mut db = BookBookmarkDb::open_at(&db_path).unwrap();
        db.recover_path_migration_journal();
        assert_eq!(journal_count(&db.conn), 1);
        assert!(from.exists());
        assert!(!to.exists());

        unregister_active_path_migration(job_id);
        db.recover_path_migration_journal();
        assert_eq!(journal_count(&db.conn), 0);
    }

    #[test]
    fn applying_move_recovers_filesystem_then_bookmark_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("book_bookmarks.db");
        let book = temp.path().join("story");
        std::fs::create_dir(&book).unwrap();
        let old_page = book.join("0001_old.jpg");
        let new_page = book.join("0001_new.jpg");
        std::fs::write(&old_page, b"page").unwrap();
        let db = BookBookmarkDb::open_at(&db_path).unwrap();
        add_compiled_bookmark(&db, &book, "0001_old.jpg", "recover me");
        drop(db);

        let mappings = vec![(old_page.clone(), new_page.clone())];
        let plan = single_rename_plan(&old_page, &new_page);
        let job_id = "one-move";
        prepare_path_migration_at(&db_path, job_id, &mappings, &plan).unwrap();
        let journal = BookBookmarkDb::open_at(&db_path).unwrap();
        journal
            .transition_journal_phase(
                job_id,
                MIGRATION_PHASE_PREPARED,
                MIGRATION_PHASE_APPLYING,
                0,
                None,
            )
            .unwrap();
        crate::book_fs_journal::execute_forward(&plan, 0, |next_step| {
            journal.update_journal_progress(job_id, MIGRATION_PHASE_APPLYING, next_step)
        })
        .unwrap();
        drop(journal); // crash after filesystem, before phase/DB commit

        let mut restarted = BookBookmarkDb::open_at(&db_path).unwrap();
        restarted.recover_path_migration_journal();
        let row = restarted.list_all().unwrap().pop().unwrap();
        assert_eq!(
            row.page_identity,
            PageIdentity::RelativePath("0001_new.jpg".to_string())
        );
        assert!(!old_page.exists());
        assert!(new_page.exists());
        assert_eq!(journal_count(&restarted.conn), 0);

        restarted.recover_path_migration_journal();
        assert_eq!(restarted.list_all().unwrap(), vec![row]);
        assert_eq!(journal_count(&restarted.conn), 0);
    }

    fn assert_cycle_recovers_from_every_step(names: &[&str], destinations: &[usize]) {
        for crash_after in 0..=(names.len() * 2) {
            let temp = tempfile::tempdir().unwrap();
            let db_path = temp.path().join("book_bookmarks.db");
            let book = temp.path().join("book");
            std::fs::create_dir(&book).unwrap();
            let paths = names.iter().map(|name| book.join(name)).collect::<Vec<_>>();
            for (name, path) in names.iter().zip(&paths) {
                std::fs::write(path, name.as_bytes()).unwrap();
            }
            let db = BookBookmarkDb::open_at(&db_path).unwrap();
            for name in names {
                add_compiled_bookmark(&db, &book, name, name);
            }
            drop(db);

            let mappings = paths
                .iter()
                .enumerate()
                .map(|(idx, path)| (path.clone(), paths[destinations[idx]].clone()))
                .collect::<Vec<_>>();
            let temp_paths = (0..names.len())
                .map(|idx| book.join(format!(".journal-temp-{idx}")))
                .collect::<Vec<_>>();
            let mut steps = paths
                .iter()
                .zip(&temp_paths)
                .map(|(from, to)| crate::book_fs_journal::BookFsStep::Rename {
                    from: from.clone(),
                    to: to.clone(),
                })
                .collect::<Vec<_>>();
            steps.extend(temp_paths.iter().enumerate().map(|(idx, from)| {
                crate::book_fs_journal::BookFsStep::Rename {
                    from: from.clone(),
                    to: paths[destinations[idx]].clone(),
                }
            }));
            let plan = crate::book_fs_journal::BookFsOperationPlan::new(steps);
            let job_id = format!("cycle-{crash_after}");
            prepare_path_migration_at(&db_path, &job_id, &mappings, &plan).unwrap();
            let journal = BookBookmarkDb::open_at(&db_path).unwrap();
            journal
                .transition_journal_phase(
                    &job_id,
                    MIGRATION_PHASE_PREPARED,
                    MIGRATION_PHASE_APPLYING,
                    0,
                    None,
                )
                .unwrap();
            let prefix = crate::book_fs_journal::BookFsOperationPlan::new(
                plan.steps[..crash_after].to_vec(),
            );
            crate::book_fs_journal::execute_forward(&prefix, 0, |next_step| {
                journal.update_journal_progress(&job_id, MIGRATION_PHASE_APPLYING, next_step)
            })
            .unwrap();
            drop(journal);

            let mut restarted = BookBookmarkDb::open_at(&db_path).unwrap();
            restarted.recover_path_migration_journal();
            let rows = restarted.list_all().unwrap();
            for (idx, name) in names.iter().enumerate() {
                let destination = &paths[destinations[idx]];
                assert_eq!(
                    std::fs::read(destination).unwrap(),
                    name.as_bytes(),
                    "crash_after={crash_after}"
                );
                let row = rows
                    .iter()
                    .find(|row| row.title.as_deref() == Some(*name))
                    .unwrap();
                assert_eq!(
                    row.page_identity,
                    PageIdentity::RelativePath(
                        destination
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string()
                    ),
                    "crash_after={crash_after}"
                );
            }
            assert!(temp_paths.iter().all(|path| !path.exists()));
            assert_eq!(journal_count(&restarted.conn), 0);
            let rows_before_second_recovery = rows.clone();
            restarted.recover_path_migration_journal();
            assert_eq!(restarted.list_all().unwrap(), rows_before_second_recovery);
        }
    }

    #[test]
    fn swap_and_three_page_cycle_recover_from_every_step_boundary() {
        assert_cycle_recovers_from_every_step(&["a.jpg", "b.jpg"], &[1, 0]);
        assert_cycle_recovers_from_every_step(&["a.jpg", "b.jpg", "c.jpg"], &[1, 2, 0]);
    }

    #[test]
    fn failed_rollback_is_retained_and_later_recovery_converges() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("book_bookmarks.db");
        let book = temp.path().join("story");
        std::fs::create_dir(&book).unwrap();
        let old_page = book.join("0001_old.jpg");
        let new_page = book.join("0001_new.jpg");
        std::fs::write(&new_page, b"operation-page").unwrap();
        std::fs::write(&old_page, b"rollback-conflict").unwrap();
        let db = BookBookmarkDb::open_at(&db_path).unwrap();
        add_compiled_bookmark(&db, &book, "0001_old.jpg", "old identity");
        drop(db);
        let mappings = vec![(old_page.clone(), new_page.clone())];
        let plan = single_rename_plan(&old_page, &new_page);
        let job_id = "rollback-retry";
        prepare_path_migration_at(&db_path, job_id, &mappings, &plan).unwrap();
        let mut retrying = BookBookmarkDb::open_at(&db_path).unwrap();
        retrying
            .transition_journal_phase(
                job_id,
                MIGRATION_PHASE_PREPARED,
                MIGRATION_PHASE_APPLYING,
                0,
                None,
            )
            .unwrap();
        retrying
            .transition_journal_phase(
                job_id,
                MIGRATION_PHASE_APPLYING,
                MIGRATION_PHASE_ROLLING_BACK,
                1,
                Some("injected rename rollback failure"),
            )
            .unwrap();
        retrying.recover_path_migration_journal();
        assert_eq!(journal_count(&retrying.conn), 1);
        assert_eq!(
            retrying.list_all().unwrap()[0].page_identity,
            PageIdentity::RelativePath("0001_old.jpg".to_string())
        );

        // Resolve the injected conflict; the same durable rollback plan now
        // restores the original path and is safe to delete.
        std::fs::remove_file(&old_page).unwrap();
        retrying.recover_path_migration_journal();
        assert_eq!(journal_count(&retrying.conn), 0);
        assert!(old_page.exists());
        assert!(!new_page.exists());
        assert_eq!(
            retrying.list_all().unwrap()[0].page_identity,
            PageIdentity::RelativePath("0001_old.jpg".to_string())
        );
    }

    #[test]
    fn old_schema_is_migrated_with_nullable_title() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory DB");
        conn.execute_batch(
            "CREATE TABLE book_bookmarks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                container_key   TEXT NOT NULL,
                container_path  TEXT NOT NULL,
                container_kind  TEXT NOT NULL,
                page_kind       TEXT NOT NULL,
                page_value      TEXT NOT NULL,
                page_key        TEXT NOT NULL,
                page_index_hint INTEGER NOT NULL DEFAULT 0,
                created_at_ms   INTEGER NOT NULL,
                UNIQUE(container_key, page_kind, page_key)
             );",
        )
        .unwrap();

        BookBookmarkDb::init_schema(&conn).expect("migrate schema");
        let mut stmt = conn.prepare("PRAGMA table_info(book_bookmarks)").unwrap();
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "title"));
        drop(stmt);
        assert_eq!(journal_count(&conn), 0);

        let db = BookBookmarkDb { conn };
        let entry = NewBookBookmark {
            container_path: PathBuf::from("C:/Books/legacy.cbz"),
            container_kind: BookContainerKind::Zip,
            page_identity: PageIdentity::ArchiveEntry("001.jpg".to_string()),
            page_index_hint: 0,
        };
        let (saved, _) = db.add(&entry).unwrap();
        assert_eq!(saved.title, None);
        db.set_title(saved.id, Some("表紙")).unwrap();
        assert_eq!(db.list_all().unwrap()[0].title.as_deref(), Some("表紙"));
    }

    #[test]
    fn pre_phase_journal_schema_is_preserved_as_legacy_ambiguous() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_bookmark_path_migrations (
                job_id TEXT PRIMARY KEY,
                mappings_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
             );
             INSERT INTO book_bookmark_path_migrations
                (job_id, mappings_json, created_at_ms)
             VALUES ('legacy', '[]', 1);",
        )
        .unwrap();
        BookBookmarkDb::init_schema(&conn).unwrap();
        let row: (Option<String>, String, i64, Option<String>) = conn
            .query_row(
                "SELECT operation_json, phase, next_step, diagnostic
                   FROM book_bookmark_path_migrations WHERE job_id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (None, "legacy_ambiguous".to_string(), 0, None));

        let mut db = BookBookmarkDb { conn };
        db.recover_path_migration_journal();
        assert_eq!(journal_count(&db.conn), 1);
        let diagnostic: String = db
            .conn
            .query_row(
                "SELECT diagnostic FROM book_bookmark_path_migrations WHERE job_id = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(diagnostic.contains("legacy migration phase"));
    }
}
