use super::*;
use rusqlite::{Connection, OptionalExtension, params};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    trim: Connection,
    identity: Connection,
    adjustment: Connection,
    tags: Connection,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        let data = temp.path().join("data");
        fs::create_dir_all(&root).unwrap();
        drop(crate::view_trim_db::ViewTrimDb::open_at(&data.join("view_trim.db")).unwrap());
        drop(crate::adjustment_db::AdjustmentDb::open_at(&data.join("adjustment.db")).unwrap());
        drop(crate::tags_db::TagsDb::open_at(&data.join("tags.db")).unwrap());
        let identity =
            crate::content_identity::open_ledger_connection_at(&data.join("content_identity.db"))
                .unwrap();
        let trim = Connection::open(data.join("view_trim.db")).unwrap();
        let adjustment = Connection::open(data.join("adjustment.db")).unwrap();
        let tags = Connection::open(data.join("tags.db")).unwrap();
        Self {
            _temp: temp,
            root,
            data,
            trim,
            identity,
            adjustment,
            tags,
        }
    }

    fn page(&self, book: &str, name: &str, content: &[u8]) -> PathBuf {
        let folder = self.root.join(book);
        fs::create_dir_all(&folder).unwrap();
        let page = folder.join(name);
        fs::write(&page, content).unwrap();
        page
    }

    fn seed(&self, page: &Path, label: &str) {
        let key = crate::path_key::normalize_keep_drive(page);
        self.trim
            .execute(
                "INSERT INTO view_trim_pages (page_path, override_json) VALUES (?1, ?2)",
                params![key, label],
            )
            .unwrap();
        self.identity
            .execute(
                "INSERT INTO edit_origin
                    (file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at)
                 VALUES (?1, 1, ?2, ?2, 0, 'image', 0)",
                params![key, label],
            )
            .unwrap();
        self.adjustment
            .execute(
                "INSERT INTO page_params (page_path, params_json) VALUES (?1, ?2)",
                params![key, label],
            )
            .unwrap();
        self.tags
            .execute(
                "INSERT INTO item_tags (item_key, tag, tag_key, applied_at) VALUES (?1, ?2, ?2, 0)",
                params![key, label],
            )
            .unwrap();
        self.tags
            .execute(
                "INSERT INTO tag_item_state (item_key, decided_at, source) VALUES (?1, 0, 'test')",
                [&key],
            )
            .unwrap();
    }

    fn assert_page(&self, page: &Path, expected: Option<&str>) {
        let key = crate::path_key::normalize_keep_drive(page);
        for (db, sql) in [
            (
                &self.trim,
                "SELECT override_json FROM view_trim_pages WHERE page_path = ?1",
            ),
            (
                &self.identity,
                "SELECT head_hash FROM edit_origin WHERE file_key = ?1",
            ),
            (
                &self.adjustment,
                "SELECT params_json FROM page_params WHERE page_path = ?1",
            ),
            (&self.tags, "SELECT tag FROM item_tags WHERE item_key = ?1"),
        ] {
            let actual = db
                .query_row(sql, [&key], |row| row.get::<_, String>(0))
                .optional()
                .unwrap();
            assert_eq!(actual.as_deref(), expected, "{sql}: {key}");
        }
    }

    fn bookmark(&self, book: &str, relative: &str) -> Connection {
        let path = self.data.join("book_bookmarks.db");
        crate::book_bookmarks::ensure_schema_at(&path).unwrap();
        let db = Connection::open(path).unwrap();
        let folder = self.root.join(book);
        db.execute(
            "INSERT INTO book_bookmarks
                (container_key, container_path, container_kind, page_kind, page_value,
                 page_key, page_index_hint, created_at_ms)
             VALUES (?1, ?2, 'compiled_book', 'relative_path', ?3, ?3, 4, 0)",
            params![
                crate::path_key::normalize_keep_drive(&folder),
                folder.to_string_lossy(),
                relative,
            ],
        )
        .unwrap();
        db
    }
}

fn assert_metadata(summary: &BookStoreSummary) {
    assert!(summary.errors.is_empty(), "{:?}", summary.errors);
    assert!(summary.store_mutations.content_identity_index_stale());
}

#[test]
fn structural_worker_reorder_swaps_same_named_images_with_trim_and_identity() {
    let fixture = Fixture::new();
    let a = fixture.page("source", "0001_same.jpg", b"a");
    let b = fixture.page("source", "0002_same.jpg", b"b");
    fixture.seed(&a, "a");
    fixture.seed(&b, "b");
    let result = flush_reorder_at(
        &fixture.data,
        fixture.root.join("source"),
        vec![b.clone(), a.clone()],
    )
    .unwrap();
    let BookOpResult::Reordered { metadata, .. } = result else {
        panic!("expected reorder");
    };
    assert_metadata(&metadata);
    assert_eq!(fs::read(&a).unwrap(), b"b");
    assert_eq!(fs::read(&b).unwrap(), b"a");
    fixture.assert_page(&a, Some("b"));
    fixture.assert_page(&b, Some("a"));
}

#[test]
fn structural_worker_move_compacts_same_named_sibling_without_stealing_metadata() {
    let fixture = Fixture::new();
    let a = fixture.page("source", "0001_same.jpg", b"a");
    let b = fixture.page("source", "0002_same.jpg", b"b");
    fixture.seed(&a, "a");
    fixture.seed(&b, "b");
    fs::create_dir_all(fixture.root.join("target")).unwrap();
    let dest = fixture.root.join("target/0001_same.jpg");
    fixture.seed(&dest, "stale");
    let result = transfer_pages_between_books_at(
        &fixture.data,
        fixture.root.clone(),
        fixture.root.join("source"),
        vec![a.clone(), b.clone()],
        vec![a.clone()],
        "target".into(),
        BookTransferKind::Move,
    )
    .unwrap();
    let BookOpResult::Transfer(summary) = result else {
        panic!("expected transfer");
    };
    assert_metadata(&summary.metadata);
    assert_eq!(fs::read(&a).unwrap(), b"b");
    assert_eq!(fs::read(&dest).unwrap(), b"a");
    fixture.assert_page(&a, Some("b"));
    fixture.assert_page(&b, None);
    fixture.assert_page(&dest, Some("a"));
}

#[test]
fn structural_worker_copy_reads_reordered_source_and_clears_noncopy_stores() {
    let fixture = Fixture::new();
    let a = fixture.page("source", "0001_same.jpg", b"a");
    let b = fixture.page("source", "0002_same.jpg", b"b");
    fixture.seed(&a, "a");
    fixture.seed(&b, "b");
    fs::create_dir_all(fixture.root.join("target")).unwrap();
    let dest = fixture.root.join("target/0001_same.jpg");
    fixture.seed(&dest, "stale");
    let result = transfer_pages_between_books_at(
        &fixture.data,
        fixture.root.clone(),
        fixture.root.join("source"),
        vec![b.clone(), a.clone()],
        vec![b.clone()],
        "target".into(),
        BookTransferKind::Copy,
    )
    .unwrap();
    let BookOpResult::Transfer(summary) = result else {
        panic!("expected transfer");
    };
    assert_metadata(&summary.metadata);
    fixture.assert_page(&a, Some("b"));
    fixture.assert_page(&b, Some("a"));
    assert_eq!(fs::read(&dest).unwrap(), b"b");
    let key = crate::path_key::normalize_keep_drive(&dest);
    let copied: String = fixture
        .adjustment
        .query_row(
            "SELECT params_json FROM page_params WHERE page_path = ?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(copied, "b");
    let tags: String = fixture
        .tags
        .query_row(
            "SELECT tag FROM item_tags WHERE item_key = ?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tags, "b");
    assert_eq!(
        fixture
            .trim
            .query_row(
                "SELECT COUNT(*) FROM view_trim_pages WHERE page_path = ?1",
                [&key],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .identity
            .query_row(
                "SELECT COUNT(*) FROM edit_origin WHERE file_key = ?1",
                [&key],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn structural_worker_rename_moves_folder_rows_and_journals_missing_bookmarks() {
    let fixture = Fixture::new();
    let source = fixture.page("source", "0005_same.jpg", b"a");
    fixture.seed(&source, "a");
    let bookmarks = fixture.bookmark("source", "0002_missing.jpg");
    fixture
        .trim
        .execute(
            "INSERT INTO view_trim_books (book_key, state_json) VALUES (?1, 'book-state')",
            [crate::path_key::normalize(&fixture.root.join("source"))],
        )
        .unwrap();
    let result = rename_book_at(&fixture.data, &fixture.root, "source", "target").unwrap();
    let BookOpResult::Renamed {
        metadata,
        edit_moves,
        bookmark_migration_journal_id,
        ..
    } = result
    else {
        panic!("expected rename");
    };
    assert_metadata(&metadata);
    fixture.assert_page(&source, None);
    fixture.assert_page(&fixture.root.join("target/0005_same.jpg"), Some("a"));
    let state: String = fixture
        .trim
        .query_row(
            "SELECT state_json FROM view_trim_books WHERE book_key = ?1",
            [crate::path_key::normalize(&fixture.root.join("target"))],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "book-state");
    let json: String = bookmarks.query_row(
        "SELECT mappings_json FROM book_bookmark_path_migrations WHERE job_id = ?1 AND phase = 'filesystem_committed'",
        [bookmark_migration_journal_id.unwrap()], |row| row.get(0),
    ).unwrap();
    let mappings: Vec<(PathBuf, PathBuf)> = serde_json::from_str(&json).unwrap();
    assert_eq!(
        mappings,
        vec![(fixture.root.join("source"), fixture.root.join("target"))]
    );
    assert_eq!(edit_moves.len(), 1);
    crate::book_bookmarks::migrate_paths_at(&fixture.data.join("book_bookmarks.db"), &mappings)
        .unwrap();
    let (container, relative, hint): (String, String, usize) = bookmarks
        .query_row(
            "SELECT container_key, page_value, page_index_hint FROM book_bookmarks",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        container,
        crate::path_key::normalize_keep_drive(&fixture.root.join("target"))
    );
    assert_eq!(relative, "0002_missing.jpg");
    assert_eq!(hint, 4);
}

#[test]
fn structural_worker_delete_purges_metadata_but_preserves_missing_bookmark_on_recreation() {
    let fixture = Fixture::new();
    let page = fixture.page("source", "0001_same.jpg", b"old");
    let unrelated = fixture.page("other", "0001_same.jpg", b"other");
    fixture.seed(&page, "old");
    fixture.seed(&unrelated, "other");
    let bookmarks = fixture.bookmark("source", "0001_same.jpg");
    let result = delete_book_at(&fixture.data, &fixture.root, "source").unwrap();
    let BookOpResult::Deleted { metadata, path, .. } = result else {
        panic!("expected delete");
    };
    assert_metadata(&metadata);
    assert_eq!(path, fixture.root.join("source"));
    assert!(!path.exists());
    fixture.assert_page(&page, None);
    fixture.assert_page(&unrelated, Some("other"));
    let remaining: usize = bookmarks
        .query_row("SELECT COUNT(*) FROM book_bookmarks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 1);
    create_book(&fixture.root, "source").unwrap();
    fixture.page("source", "0001_same.jpg", b"new");
    fixture.assert_page(&page, None);
}

#[test]
fn structural_worker_reports_store_error_after_successful_filesystem_commit() {
    let fixture = Fixture::new();
    let a = fixture.page("source", "0001_same.jpg", b"a");
    let b = fixture.page("source", "0002_same.jpg", b"b");
    fixture.seed(&a, "a");
    fixture.seed(&b, "b");
    fixture
        .trim
        .execute_batch("DROP TABLE view_trim_pages")
        .unwrap();
    let result = flush_reorder_at(
        &fixture.data,
        fixture.root.join("source"),
        vec![b.clone(), a.clone()],
    )
    .unwrap();
    let BookOpResult::Reordered { metadata, .. } = result else {
        panic!("expected reorder");
    };
    assert!(!metadata.errors.is_empty());
    assert!(
        metadata
            .errors
            .iter()
            .any(|error| error.contains("view_trim"))
    );
    assert_eq!(fs::read(&a).unwrap(), b"b");
    let key = crate::path_key::normalize_keep_drive(&a);
    let hash: String = fixture
        .identity
        .query_row(
            "SELECT head_hash FROM edit_origin WHERE file_key = ?1",
            [key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hash, "b");
}
