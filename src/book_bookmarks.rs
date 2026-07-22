//! 本のページブックマーク永続化と非同期アクセス。
//!
//! ページ番号は表示用 hint に限り、ジャンプ先の正本はコンテナ種別ごとに
//! [`PageIdentity`] で保持する。SQLite は [`BookBookmarkService`] の専用 worker
//! だけが触り、UI スレッドは request / event の送受信だけを行う。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

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
    MigratePaths(u64, Vec<(PathBuf, PathBuf)>),
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
                                BookBookmarkRequest::MigratePaths(request_id, _) => {
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
                        BookBookmarkRequest::MigratePaths(request_id, mappings) => {
                            BookBookmarkEvent::PathsMigrated {
                                request_id,
                                result: db.migrate_paths(&mappings).map_err(|err| err.to_string()),
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
        if let Some(tx) = &self.tx {
            let _ = tx.send(BookBookmarkRequest::MigratePaths(request_id, mappings));
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
                ON book_bookmarks(created_at_ms DESC);",
        )?;
        ensure_column(conn, "title", "TEXT")
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
        if mappings.is_empty() {
            return Ok(0);
        }
        let mappings = PathMappingIndex::new(mappings);
        let targets: Vec<_> = self
            .list_all()?
            .into_iter()
            .filter_map(|bookmark| migration_target(&bookmark, &mappings))
            .collect();
        if targets.is_empty() {
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
        tx.commit()?;
        Ok(targets.len())
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
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let exists = {
        let mut stmt = conn.prepare("PRAGMA table_info(book_bookmarks)")?;
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
        &format!("ALTER TABLE book_bookmarks ADD COLUMN {column} {definition}"),
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
}
