use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::db::CollectionStoreDb;
use super::*;
use crate::settings::SortOrder;

fn registration(path: &str, kind: CollectionResolvedKind) -> CollectionRegistration {
    CollectionRegistration::from_trusted_path(Path::new(path), kind).unwrap()
}

fn open_db(temp: &tempfile::TempDir) -> CollectionStoreDb {
    CollectionStoreDb::open_at(&temp.path().join("collection.db")).unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct CollectionDbFingerprint {
    user_version: u32,
    catalog_revision: i64,
    collections: Vec<(String, String, String, String, String, i64, i64)>,
    entries: Vec<(String, String, String, String, String, String, i64)>,
}

fn collection_db_fingerprint(path: &Path) -> CollectionDbFingerprint {
    let conn =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let user_version = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let catalog_revision = conn
        .query_row(
            "SELECT catalog_revision FROM collection_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let collections = {
        let mut statement = conn
            .prepare(
                "SELECT id, name, order_mode, sort_order, shuffle_seed, revision, catalog_position
                 FROM collections ORDER BY catalog_position, id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let entries = {
        let mut statement = conn
            .prepare(
                "SELECT id, collection_id, source_namespace, source_path, normalized_path,
                        resolved_kind, manual_position
                 FROM collection_entries ORDER BY collection_id, manual_position, id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    CollectionDbFingerprint {
        user_version,
        catalog_revision,
        collections,
        entries,
    }
}

fn assert_first_mutation_backup<T>(
    setup: impl FnOnce(&mut CollectionStoreDb) -> T,
    mutate: impl FnOnce(&mut CollectionStoreDb, &T),
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let context = {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        let context = setup(&mut db);
        context
    };
    for generation in 1..=10 {
        let _ = std::fs::remove_file(temp.path().join(format!("collection.db.bak{generation}")));
    }
    let expected = collection_db_fingerprint(&path);
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    assert!(!temp.path().join("collection.db.bak1").exists());
    mutate(&mut db, &context);
    assert_eq!(
        collection_db_fingerprint(&temp.path().join("collection.db.bak1")),
        expected
    );
    assert!(!temp.path().join("collection.db.bak2").exists());
}

#[test]
fn source_key_collapses_legal_extended_drive_and_unc_spelling() {
    let plain = CollectionSourcePath::from_trusted(r"C:\Media\.\Books\..\A.JPG").unwrap();
    let extended = CollectionSourcePath::from_trusted(r"\\?\C:\Media\A.JPG\").unwrap();
    assert_eq!(plain.key(), extended.key());
    assert_eq!(plain.key().normalized_path(), "c:/media/a.jpg");

    let plain_unc = CollectionSourcePath::from_trusted(r"\\Server\Share\Books\.\A.zip").unwrap();
    let extended_unc =
        CollectionSourcePath::from_trusted(r"\\?\UNC\server\share\Books\A.ZIP").unwrap();
    assert_eq!(plain_unc.key(), extended_unc.key());
    assert_eq!(
        plain_unc.key().normalized_path(),
        "//server/share/books/a.zip"
    );
    let repeated = CollectionSourcePath::from_trusted(r"\\Server\\Share\\Books\\A.zip").unwrap();
    let repeated_extended =
        CollectionSourcePath::from_trusted(r"\\?\UNC\\server\\share\\Books\\A.zip").unwrap();
    assert_eq!(plain_unc.key(), repeated.key());
    assert_eq!(plain_unc.key(), repeated_extended.key());
}

#[test]
fn external_text_policy_rejects_device_verbatim_and_root_escape() {
    let base = Path::new(r"C:\Imports");
    for path in [
        r"\\?\C:\Media\a.jpg",
        r"\\.\PhysicalDrive0",
        r"https://example.invalid/a.jpg",
        r"C:\Media\*.jpg",
        r"%USERPROFILE%\a.jpg",
        r"C:\Media\NUL.txt",
        r"C:\Media\COM1",
        r"C:\Media\image.jpg:secret",
        "C:\\Media\\bad\u{0000}.jpg",
    ] {
        assert!(
            CollectionSourcePath::from_external_text(path, base).is_err(),
            "{path}"
        );
    }
    assert!(
        CollectionSourcePath::from_external_text(
            r"..\..\outside.jpg",
            Path::new(r"\\server\share"),
        )
        .is_err()
    );
    let relative = CollectionSourcePath::from_external_text(
        r"sub\.\old\..\image.png",
        Path::new(r"C:\Imports"),
    )
    .unwrap();
    assert_eq!(relative.path(), Path::new(r"C:\Imports\sub\image.png"));
}

#[test]
fn text_preview_is_pure_and_keeps_missing_paths_hash_and_first_duplicate() {
    let preview = parse_collection_text(
        "\u{feff}\"missing folder/a.jpg\"\r\n#literal.png\r\nsub/../#literal.png\r\n\r\nhttps://bad/x\r\n",
        Path::new(r"C:\NeverExists\list.txt"),
    )
    .unwrap();
    assert_eq!(preview.lines().len(), 4);
    assert_eq!(
        preview.lines()[0].status,
        CollectionImportLineStatus::Accepted
    );
    assert_eq!(
        preview.lines()[1].status,
        CollectionImportLineStatus::Accepted
    );
    assert_eq!(
        preview.lines()[2].status,
        CollectionImportLineStatus::Duplicate { first_line: 2 }
    );
    assert!(matches!(
        preview.lines()[3].status,
        CollectionImportLineStatus::Invalid { .. }
    ));
    assert_eq!(preview.accepted_count(), 2);
    assert_eq!(preview.invalid_count(), 1);
    let accepted: Vec<_> = preview
        .accepted_paths()
        .map(|(path, _)| path.to_path_buf())
        .collect();
    assert_eq!(accepted.len(), 2);
    assert!(accepted[0].to_string_lossy().contains("NeverExists"));

    let serialized = serialize_collection_paths(accepted.iter().map(PathBuf::as_path));
    assert!(serialized.starts_with("\u{feff}\"C:\\NeverExists\\missing folder\\a.jpg\""));
    assert!(serialized.ends_with("\r\n"));
    let reparsed =
        parse_collection_text(&serialized, Path::new(r"C:\Export\collection.txt")).unwrap();
    assert_eq!(
        reparsed
            .accepted_paths()
            .map(|(path, _)| path.to_path_buf())
            .collect::<Vec<_>>(),
        accepted
    );
}

#[test]
fn collection_sort_order_wire_names_round_trip_without_debug_spelling() {
    for sort in [
        SortOrder::FileName,
        SortOrder::FileNameDesc,
        SortOrder::Numeric,
        SortOrder::NumericDesc,
        SortOrder::DateAsc,
        SortOrder::DateDesc,
        SortOrder::SizeAsc,
        SortOrder::SizeDesc,
    ] {
        let wire = collection_sort_order_wire_name(sort);
        assert_eq!(collection_sort_order_from_wire_name(wire), Some(sort));
        assert_eq!(wire, wire.to_ascii_lowercase());
    }
    assert_eq!(collection_sort_order_from_wire_name("FileName"), None);
}

#[test]
fn text_preview_rejects_byte_and_nonempty_line_resource_limits_without_partial_preview() {
    let source = Path::new(r"C:\Imports\list.txt");
    let at_byte_limit = " ".repeat(MAX_COLLECTION_IMPORT_BYTES);
    assert!(
        parse_collection_text(&at_byte_limit, source)
            .unwrap()
            .lines()
            .is_empty()
    );
    let over_byte_limit = format!("{at_byte_limit} ");
    assert_eq!(
        parse_collection_text(&over_byte_limit, source),
        Err(CollectionImportLimitError::Bytes)
    );

    let at_line_limit = "C:\\Media\\same.jpg\r\n".repeat(MAX_COLLECTION_IMPORT_NONEMPTY_LINES);
    let preview = parse_collection_text(&at_line_limit, source).unwrap();
    assert_eq!(preview.lines().len(), MAX_COLLECTION_IMPORT_NONEMPTY_LINES);
    assert_eq!(
        preview.lines()[1].status,
        CollectionImportLineStatus::Duplicate { first_line: 1 }
    );
    assert_eq!(
        parse_collection_text(
            &format!("{at_line_limit}\r\nC:\\Media\\next.jpg\r\n"),
            source
        ),
        Err(CollectionImportLimitError::NonemptyLines)
    );
}

#[test]
fn add_batch_uses_authoritative_count_and_keeps_oversized_legacy_entries_editable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = open_db(&temp);
    let created = db.create_collection("Capacity").unwrap();
    let id = created.collection_id();
    // Seed old data directly to keep this boundary test independent of the new admission path.
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO collection_entries
             (id, collection_id, source_namespace, source_path, normalized_path,
              resolved_kind, manual_position, created_at_ms)
             VALUES (?1, ?2, 'filesystem_path', ?3, ?4, 'image', ?5, 0)",
            )
            .unwrap();
        for index in 0..MAX_COLLECTION_ENTRIES - 1 {
            insert
                .execute(rusqlite::params![
                    CollectionEntryId::new().to_string(),
                    id.to_string(),
                    format!(r"C:\seed\{index}.jpg"),
                    format!("c:/seed/{index}.jpg"),
                    index as i64,
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();

    let outcome = db
        .add_batch(
            id,
            created.revision(),
            vec![
                registration(r"C:\seed\0.jpg", CollectionResolvedKind::Image),
                registration(r"C:\new\first.jpg", CollectionResolvedKind::Image),
                registration(r"C:\new\second.jpg", CollectionResolvedKind::Image),
                registration(r"C:\new\second.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap();
    assert_eq!(outcome.added.len(), 1);
    assert_eq!(outcome.duplicates.len(), 2);
    assert_eq!(outcome.capacity_rejected.len(), 1);
    assert_eq!(outcome.snapshot.entries.len(), MAX_COLLECTION_ENTRIES);
    assert_eq!(outcome.snapshot.revision(), created.revision() + 1);

    let unchanged = db
        .add_batch(
            id,
            outcome.snapshot.revision(),
            vec![
                registration(r"C:\seed\0.jpg", CollectionResolvedKind::Image),
                registration(r"C:\new\first.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap();
    assert!(unchanged.added.is_empty());
    assert_eq!(unchanged.duplicates.len(), 2);
    assert!(unchanged.capacity_rejected.is_empty());
    assert_eq!(unchanged.snapshot.revision(), outcome.snapshot.revision());
    assert_eq!(
        unchanged.snapshot.catalog_revision,
        outcome.snapshot.catalog_revision
    );

    conn.execute(
        "INSERT INTO collection_entries
         (id, collection_id, source_namespace, source_path, normalized_path,
          resolved_kind, manual_position, created_at_ms)
         VALUES (?1, ?2, 'filesystem_path', 'C:\\legacy\\extra.jpg',
                 'c:/legacy/extra.jpg', 'image', ?3, 0)",
        rusqlite::params![
            CollectionEntryId::new().to_string(),
            id.to_string(),
            MAX_COLLECTION_ENTRIES as i64
        ],
    )
    .unwrap();
    let legacy = db.snapshot(id).unwrap();
    assert_eq!(legacy.entries.len(), MAX_COLLECTION_ENTRIES + 1);
    let rejected = db
        .add_batch(
            id,
            legacy.revision(),
            vec![registration(
                r"C:\new\third.jpg",
                CollectionResolvedKind::Image,
            )],
        )
        .unwrap();
    assert!(rejected.added.is_empty());
    assert_eq!(rejected.capacity_rejected.len(), 1);
    assert_eq!(rejected.snapshot.revision(), legacy.revision());

    let reversed = legacy
        .entries
        .iter()
        .rev()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    let reordered = db
        .reorder_manual(id, legacy.revision(), reversed.clone())
        .unwrap();
    assert_eq!(reordered.entries[0].id, reversed[0]);
    let removed = db
        .remove_entries(id, reordered.revision(), vec![reversed[0]])
        .unwrap();
    assert_eq!(removed.entries.len(), MAX_COLLECTION_ENTRIES);
}

#[test]
fn add_batch_sql_failure_rolls_back_earlier_insert_and_revision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = open_db(&temp);
    let created = db.create_collection("Atomic add").unwrap();
    Connection::open(path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_second BEFORE INSERT ON collection_entries
         WHEN NEW.normalized_path = 'c:/atomic/second.jpg'
         BEGIN SELECT RAISE(ABORT, 'forced insert failure'); END;",
        )
        .unwrap();
    assert!(
        db.add_batch(
            created.collection_id(),
            created.revision(),
            vec![
                registration(r"C:\atomic\first.jpg", CollectionResolvedKind::Image),
                registration(r"C:\atomic\second.jpg", CollectionResolvedKind::Image),
            ]
        )
        .is_err()
    );
    let after = db.snapshot(created.collection_id()).unwrap();
    assert!(after.entries.is_empty());
    assert_eq!(after.revision(), created.revision());
    assert_eq!(after.catalog_revision, created.catalog_revision);
}

#[test]
fn database_keeps_ids_manual_order_and_revisions_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (collection_id, entry_ids, final_revision, catalog_revision) = {
        let mut db = open_db(&temp);
        let created = db.create_collection("Audio").unwrap();
        assert_eq!(created.revision(), 1);
        let added = db
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    registration(r"C:\Media\B.mp3", CollectionResolvedKind::Audio),
                    registration(r"C:\Media\A.mp3", CollectionResolvedKind::Audio),
                    registration(r"c:/media/a.MP3", CollectionResolvedKind::Unresolved),
                ],
            )
            .unwrap();
        assert_eq!(added.added.len(), 2);
        assert_eq!(added.duplicates.len(), 1);
        assert_eq!(added.snapshot.revision(), 2);
        let ids: Vec<_> = added
            .snapshot
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect();

        let reordered = db
            .reorder_manual(created.collection_id(), 2, vec![ids[1], ids[0]])
            .unwrap();
        assert_eq!(reordered.revision(), 3);
        let standard = db
            .set_order(
                created.collection_id(),
                3,
                CollectionOrderMode::Standard,
                SortOrder::SizeDesc,
            )
            .unwrap();
        assert_eq!(standard.revision(), 4);
        assert_eq!(
            standard
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![ids[1], ids[0]]
        );
        assert_eq!(
            db.reorder_manual(created.collection_id(), 4, ids.clone()),
            Err(CollectionStoreError::ManualOrderInactive)
        );
        let standard = db
            .add_batch(
                created.collection_id(),
                4,
                vec![registration(
                    r"C:\Media\C.mp3",
                    CollectionResolvedKind::Audio,
                )],
            )
            .unwrap();
        let new_id = standard.added[0];
        let manual = db
            .set_order(
                created.collection_id(),
                5,
                CollectionOrderMode::Manual,
                SortOrder::SizeDesc,
            )
            .unwrap();
        assert_eq!(
            manual
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            vec![ids[1], ids[0], new_id]
        );
        (
            created.collection_id(),
            ids,
            manual.revision(),
            manual.catalog_revision,
        )
    };

    let db = open_db(&temp);
    let reopened = db.snapshot(collection_id).unwrap();
    assert_eq!(reopened.revision(), final_revision);
    assert_eq!(reopened.catalog_revision, catalog_revision);
    assert_eq!(reopened.entries.len(), 3);
    assert_eq!(reopened.entries[0].id, entry_ids[1]);
    assert_eq!(reopened.entries[1].id, entry_ids[0]);
    assert_eq!(reopened.definition.standard_sort, SortOrder::SizeDesc);
}

#[test]
fn v1_migration_preserves_every_collection_entry_and_revision_and_backs_up_original() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let (catalog_before, snapshots_before) = {
        let mut db = open_db(&temp);
        let first = db.create_collection("First").unwrap();
        let second = db.create_collection("Second").unwrap();
        let first = db
            .add_batch(
                first.collection_id(),
                first.revision(),
                vec![
                    registration(r"C:\v1\a.jpg", CollectionResolvedKind::Image),
                    registration(r"C:\v1\b.zip", CollectionResolvedKind::Zip),
                ],
            )
            .unwrap()
            .snapshot;
        let second = db
            .add_batch(
                second.collection_id(),
                second.revision(),
                vec![registration(r"C:\v1\c.mp3", CollectionResolvedKind::Audio)],
            )
            .unwrap()
            .snapshot;
        let second = db
            .set_order(
                second.collection_id(),
                second.revision(),
                CollectionOrderMode::Standard,
                SortOrder::DateDesc,
            )
            .unwrap();
        let catalog = db.catalog().unwrap();
        let snapshots: Vec<CollectionSnapshot> = [first.collection_id(), second.collection_id()]
            .into_iter()
            .map(|id| db.snapshot(id).unwrap())
            .collect();
        (catalog, snapshots)
    };
    for generation in 1..=10 {
        let _ = std::fs::remove_file(temp.path().join(format!("collection.db.bak{generation}")));
    }
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE collections DROP COLUMN shuffle_seed; PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let db = CollectionStoreDb::open_at(&path).unwrap();
    assert_eq!(db.catalog().unwrap(), catalog_before);
    for original in &snapshots_before {
        assert_eq!(db.snapshot(original.collection_id()).unwrap(), *original);
    }
    let backup = temp.path().join("collection.db.bak1");
    assert!(backup.is_file());
    let backup_conn =
        Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let backup_version: u32 = backup_conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(backup_version, 1);
    let backup_count: i64 = backup_conn
        .query_row("SELECT COUNT(*) FROM collection_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(backup_count, 3);
    drop(backup_conn);
    drop(db);
    let mut reopened = CollectionStoreDb::open_at(&path).unwrap();
    assert!(!temp.path().join("collection.db.bak2").exists());
    let first = reopened
        .snapshot(snapshots_before[0].collection_id())
        .unwrap();
    reopened
        .rename_collection(first.collection_id(), first.revision(), "First changed")
        .unwrap();
    assert!(
        temp.path().join("collection.db.bak2").exists(),
        "the next session's first real v2 mutation rotates the v1 generation once"
    );
    let prior = Connection::open_with_flags(
        temp.path().join("collection.db.bak2"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let prior_version: u32 = prior
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(prior_version, 1);
}

#[test]
fn new_database_and_read_only_reopen_do_not_rotate_but_first_v2_mutation_includes_committed_wal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    assert!(!temp.path().join("collection.db.bak1").exists());
    let created = db.create_collection("WAL member").unwrap();
    let mut wal_reader = Connection::open(&path).unwrap();
    let wal_guard = wal_reader.transaction().unwrap();
    let _: i64 = wal_guard
        .query_row("SELECT COUNT(*) FROM collection_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    db.add_batch(
        created.collection_id(),
        created.revision(),
        vec![registration(
            r"C:\wal\one.png",
            CollectionResolvedKind::Image,
        )],
    )
    .unwrap();
    assert!(
        path.with_file_name("collection.db-wal").is_file(),
        "a held reader keeps the committed entry in WAL for the backup boundary"
    );
    let armed_generation = temp.path().join("collection.db.bak1");
    let armed_backup = Connection::open_with_flags(
        &armed_generation,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let pre_add_count: i64 = armed_backup
        .query_row("SELECT COUNT(*) FROM collection_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(pre_add_count, 0);
    drop(armed_backup);
    drop(db);
    std::fs::remove_file(armed_generation).unwrap();
    let mut reopened = CollectionStoreDb::open_at(&path).unwrap();
    assert!(!temp.path().join("collection.db.bak1").exists());
    reopened
        .rename_collection(created.collection_id(), 2, "WAL member renamed")
        .unwrap();
    let backup = Connection::open_with_flags(
        temp.path().join("collection.db.bak1"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: i64 = backup
        .query_row("SELECT COUNT(*) FROM collection_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    drop(wal_guard);
    assert_eq!(
        reopened
            .snapshot(created.collection_id())
            .unwrap()
            .entries
            .len(),
        1
    );
    drop(backup);
    let first_generation = collection_db_fingerprint(&temp.path().join("collection.db.bak1"));
    let renamed = reopened.snapshot(created.collection_id()).unwrap();
    reopened
        .set_order(
            renamed.collection_id(),
            renamed.revision(),
            CollectionOrderMode::Standard,
            SortOrder::SizeAsc,
        )
        .unwrap();
    assert_eq!(
        collection_db_fingerprint(&temp.path().join("collection.db.bak1")),
        first_generation
    );
    assert!(!temp.path().join("collection.db.bak2").exists());
    drop(reopened);
    let mut reopened = CollectionStoreDb::open_at(&path).unwrap();
    assert!(!temp.path().join("collection.db.bak2").exists());
    let current = reopened.snapshot(created.collection_id()).unwrap();
    reopened
        .set_order(
            current.collection_id(),
            current.revision(),
            CollectionOrderMode::Standard,
            SortOrder::DateAsc,
        )
        .unwrap();
    assert!(temp.path().join("collection.db.bak2").is_file());
}

#[test]
fn actor_read_only_restarts_do_not_consume_generations_and_first_mutation_does() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let runtime = CollectionStoreRuntime::start_at(path.clone()).unwrap();
    wait_ready(&runtime);
    let created = runtime
        .client()
        .create_collection("Seed".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    runtime.shutdown_and_join();
    assert!(!temp.path().join("collection.db.bak1").exists());

    let runtime = CollectionStoreRuntime::start_at(path.clone()).unwrap();
    wait_ready(&runtime);
    assert!(!temp.path().join("collection.db.bak1").exists());
    runtime.shutdown_and_join();
    assert!(!temp.path().join("collection.db.bak1").exists());

    let runtime = CollectionStoreRuntime::start_at(path).unwrap();
    wait_ready(&runtime);
    runtime
        .client()
        .rename_collection(
            created.collection_id(),
            created.revision(),
            "First write after read-only restarts".into(),
        )
        .unwrap()
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(temp.path().join("collection.db.bak1").is_file());
    runtime.shutdown_and_join();
}

#[test]
fn empty_database_reopen_skips_empty_backup_then_preserves_first_useful_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    drop(CollectionStoreDb::open_at(&path).unwrap());

    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let created = db.create_collection("First useful state").unwrap();
    assert!(
        !temp.path().join("collection.db.bak1").exists(),
        "the first write after reopening a never-edited database must not preserve an empty schema"
    );
    db.delete_collection(created.collection_id(), created.revision())
        .unwrap();
    let backup = CollectionStoreDb::open_at(&temp.path().join("collection.db.bak1")).unwrap();
    assert_eq!(
        backup.catalog().unwrap().definitions[0].name,
        "First useful state"
    );
}

#[test]
fn invalid_or_future_v2_does_not_rotate_known_good_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let backup = temp.path().join("collection.db.bak1");
    std::fs::write(&backup, b"known good").unwrap();
    std::fs::write(&path, b"not sqlite").unwrap();
    assert!(CollectionStoreDb::open_at(&path).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
    assert!(!temp.path().join("collection.db.bak2").exists());

    std::fs::remove_file(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    assert!(matches!(
        CollectionStoreDb::open_at(&path),
        Err(CollectionStoreError::IncompatibleSchema(999))
    ));
    assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
    assert!(!temp.path().join("collection.db.bak2").exists());
}

#[test]
fn orphaned_v2_entry_does_not_displace_known_good_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    drop(CollectionStoreDb::open_at(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "INSERT INTO collection_entries
         (id, collection_id, source_namespace, source_path, normalized_path,
          resolved_kind, manual_position, created_at_ms)
         VALUES (?1, ?2, 'filesystem_path', ?3, ?4, 'image', 0, 0)",
        rusqlite::params![
            CollectionEntryId::new().to_string(),
            CollectionId::new().to_string(),
            r"C:\orphan\image.png",
            "c:/orphan/image.png",
        ],
    )
    .unwrap();
    drop(conn);
    let backup = temp.path().join("collection.db.bak1");
    std::fs::write(&backup, b"known good").unwrap();
    assert!(
        matches!(CollectionStoreDb::open_at(&path), Err(CollectionStoreError::Persistence(message)) if message.contains("foreign key"))
    );
    assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
    assert!(!temp.path().join("collection.db.bak2").exists());
}

#[test]
fn existing_v2_backup_snapshot_failure_is_nonfatal_without_rotating_chain() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let created = db.create_collection("Keep available").unwrap();
    drop(db);
    std::fs::write(temp.path().join("collection.db.bak1"), b"known good").unwrap();
    std::fs::create_dir(temp.path().join("collection.db.bak.tmp-snapshot")).unwrap();
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let renamed = db
        .rename_collection(
            created.collection_id(),
            created.revision(),
            "Still available",
        )
        .unwrap();
    assert_eq!(renamed.definition.name, "Still available");
    assert_eq!(
        std::fs::read(temp.path().join("collection.db.bak1")).unwrap(),
        b"known good"
    );
    assert!(!temp.path().join("collection.db.bak2").exists());
    std::fs::remove_dir(temp.path().join("collection.db.bak.tmp-snapshot")).unwrap();
    db.set_order(
        created.collection_id(),
        renamed.revision(),
        CollectionOrderMode::Standard,
        SortOrder::DateAsc,
    )
    .unwrap();
    assert!(
        !temp.path().join("collection.db.bak2").exists(),
        "a failed normal snapshot is attempted only once in the session"
    );
}

#[test]
fn every_real_mutation_variant_backs_up_the_exact_prechange_database() {
    assert_first_mutation_backup(
        |db| {
            db.create_collection("Existing").unwrap();
        },
        |db, ()| {
            db.create_collection("Created after backup").unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Before rename").unwrap();
            (snapshot.collection_id(), snapshot.revision())
        },
        |db, &(id, revision)| {
            db.rename_collection(id, revision, "After rename").unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Delete me").unwrap();
            (snapshot.collection_id(), snapshot.revision())
        },
        |db, &(id, revision)| {
            db.delete_collection(id, revision).unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Order").unwrap();
            (snapshot.collection_id(), snapshot.revision())
        },
        |db, &(id, revision)| {
            db.set_order(
                id,
                revision,
                CollectionOrderMode::Shuffle,
                SortOrder::FileName,
            )
            .unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Add").unwrap();
            (snapshot.collection_id(), snapshot.revision())
        },
        |db, &(id, revision)| {
            db.add_batch(
                id,
                revision,
                vec![registration(
                    r"C:\backup\added.jpg",
                    CollectionResolvedKind::Image,
                )],
            )
            .unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Remove").unwrap();
            let snapshot = db
                .add_batch(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    vec![registration(
                        r"C:\backup\removed.jpg",
                        CollectionResolvedKind::Image,
                    )],
                )
                .unwrap()
                .snapshot;
            (
                snapshot.collection_id(),
                snapshot.revision(),
                snapshot.entries[0].id,
            )
        },
        |db, &(id, revision, entry_id)| {
            db.remove_entries(id, revision, vec![entry_id]).unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Reorder").unwrap();
            let snapshot = db
                .add_batch(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    vec![
                        registration(r"C:\backup\a.jpg", CollectionResolvedKind::Image),
                        registration(r"C:\backup\b.jpg", CollectionResolvedKind::Image),
                    ],
                )
                .unwrap()
                .snapshot;
            let order: Vec<CollectionEntryId> = snapshot
                .entries
                .iter()
                .rev()
                .map(|entry| entry.id)
                .collect();
            (snapshot.collection_id(), snapshot.revision(), order)
        },
        |db, (id, revision, order)| {
            db.reorder_manual(*id, *revision, order.clone()).unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Relink").unwrap();
            let snapshot = db
                .add_batch(
                    snapshot.collection_id(),
                    snapshot.revision(),
                    vec![registration(
                        r"C:\backup\old.jpg",
                        CollectionResolvedKind::Image,
                    )],
                )
                .unwrap()
                .snapshot;
            (
                snapshot.collection_id(),
                snapshot.revision(),
                snapshot.entries[0].id,
            )
        },
        |db, &(id, revision, entry_id)| {
            db.relink(
                id,
                revision,
                entry_id,
                registration(r"D:\backup\new.jpg", CollectionResolvedKind::Image),
            )
            .unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Migrate").unwrap();
            db.add_batch(
                snapshot.collection_id(),
                snapshot.revision(),
                vec![registration(
                    r"C:\backup-old\page.jpg",
                    CollectionResolvedKind::Image,
                )],
            )
            .unwrap();
        },
        |db, ()| {
            db.migrate_sources(
                CollectionSourceMigration::from_trusted_paths(
                    r"C:\backup-old",
                    r"D:\backup-new",
                    CollectionSourceMigrationScope::Tree,
                )
                .unwrap(),
            )
            .unwrap();
        },
    );
    assert_first_mutation_backup(
        |db| {
            let snapshot = db.create_collection("Raw path migration").unwrap();
            db.add_batch(
                snapshot.collection_id(),
                snapshot.revision(),
                vec![registration(
                    r"C:\Case\Page.jpg",
                    CollectionResolvedKind::Image,
                )],
            )
            .unwrap();
        },
        |db, ()| {
            let outcome = db
                .migrate_sources(
                    CollectionSourceMigration::from_trusted_paths(
                        r"C:\Case\Page.jpg",
                        r"c:\CASE\PAGE.jpg",
                        CollectionSourceMigrationScope::Exact,
                    )
                    .unwrap(),
                )
                .unwrap();
            assert_eq!(outcome.updated_entries, 1);
        },
    );
}

#[test]
fn noops_and_rejections_leave_the_backup_chain_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let (id, revision, entries) = {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        let snapshot = db.create_collection("Stable").unwrap();
        let snapshot = db
            .add_batch(
                snapshot.collection_id(),
                snapshot.revision(),
                vec![
                    registration(r"C:\stable\a.jpg", CollectionResolvedKind::Image),
                    registration(r"C:\stable\b.jpg", CollectionResolvedKind::Image),
                ],
            )
            .unwrap()
            .snapshot;
        (
            snapshot.collection_id(),
            snapshot.revision(),
            snapshot
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
        )
    };
    let backup = temp.path().join("collection.db.bak1");
    std::fs::write(&backup, b"known good").unwrap();
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let assert_unchanged = || {
        assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
        assert!(!temp.path().join("collection.db.bak2").exists());
    };

    db.rename_collection(id, revision, "Stable").unwrap();
    assert_unchanged();
    db.set_order(
        id,
        revision,
        CollectionOrderMode::Manual,
        SortOrder::FileName,
    )
    .unwrap();
    assert_unchanged();
    let duplicate = db
        .add_batch(
            id,
            revision,
            vec![registration(
                r"C:\stable\a.jpg",
                CollectionResolvedKind::Image,
            )],
        )
        .unwrap();
    assert!(duplicate.added.is_empty());
    assert_unchanged();
    db.remove_entries(id, revision, vec![CollectionEntryId::new()])
        .unwrap();
    assert_unchanged();
    db.reorder_manual(id, revision, entries.clone()).unwrap();
    assert_unchanged();
    db.relink(
        id,
        revision,
        entries[0],
        registration(r"C:\stable\a.jpg", CollectionResolvedKind::Image),
    )
    .unwrap();
    assert_unchanged();
    let migration = db
        .migrate_sources(
            CollectionSourceMigration::from_trusted_paths(
                r"Z:\not-present",
                r"Y:\still-not-present",
                CollectionSourceMigrationScope::Tree,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(migration.updated_entries, 0);
    assert_unchanged();
    let identity_migration = db
        .migrate_sources(
            CollectionSourceMigration::from_trusted_paths(
                r"C:\stable\a.jpg",
                r"C:\stable\a.jpg",
                CollectionSourceMigrationScope::Exact,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(identity_migration.updated_entries, 0);
    assert_unchanged();

    assert!(matches!(
        db.rename_collection(id, revision + 1, "Conflict"),
        Err(CollectionStoreError::Conflict { .. })
    ));
    assert_unchanged();
    assert_eq!(
        db.reorder_manual(id, revision, vec![entries[0], entries[0]]),
        Err(CollectionStoreError::InvalidOrder)
    );
    assert_unchanged();
    assert!(matches!(
        db.relink(
            id,
            revision,
            entries[0],
            registration(r"C:\stable\b.jpg", CollectionResolvedKind::Image),
        ),
        Err(CollectionStoreError::DuplicateSource(_))
    ));
    assert_unchanged();
}

#[test]
fn capacity_only_add_is_a_noop_and_does_not_rotate_backups() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let created = {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        db.create_collection("Full").unwrap()
    };
    {
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO collection_entries
                     (id, collection_id, source_namespace, source_path, normalized_path,
                      resolved_kind, manual_position, created_at_ms)
                     VALUES (?1, ?2, 'filesystem_path', ?3, ?4, 'image', ?5, 0)",
                )
                .unwrap();
            for index in 0..MAX_COLLECTION_ENTRIES {
                insert
                    .execute(rusqlite::params![
                        CollectionEntryId::new().to_string(),
                        created.collection_id().to_string(),
                        format!(r"C:\full\{index}.jpg"),
                        format!("c:/full/{index}.jpg"),
                        index as i64,
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let backup = temp.path().join("collection.db.bak1");
    std::fs::write(&backup, b"known good").unwrap();
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let outcome = db
        .add_batch(
            created.collection_id(),
            created.revision(),
            vec![registration(
                r"C:\full\rejected.jpg",
                CollectionResolvedKind::Image,
            )],
        )
        .unwrap();
    assert!(outcome.added.is_empty());
    assert_eq!(outcome.capacity_rejected.len(), 1);
    assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
    assert!(!temp.path().join("collection.db.bak2").exists());
}

#[test]
fn rejected_and_failed_first_writes_do_not_arm_or_leak_mutation_outcome() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = CollectionStoreDb::open_at(&path).unwrap();

    assert_eq!(
        db.create_collection("   "),
        Err(CollectionStoreError::InvalidName)
    );
    assert!(!db.take_last_mutation_applied());
    assert!(!temp.path().join("collection.db.bak1").exists());

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_first_collection BEFORE INSERT ON collections
             BEGIN SELECT RAISE(FAIL, 'injected first-write failure'); END;",
        )
        .unwrap();
    }
    assert!(matches!(
        db.create_collection("Fails during apply"),
        Err(CollectionStoreError::Persistence(_))
    ));
    assert!(!db.take_last_mutation_applied());
    assert!(!temp.path().join("collection.db.bak1").exists());
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TRIGGER reject_first_collection;")
            .unwrap();
    }

    let created = db.create_collection("First success").unwrap();
    assert!(db.take_last_mutation_applied());
    assert!(!temp.path().join("collection.db.bak1").exists());
    assert_eq!(
        db.rename_collection(created.collection_id(), 99, "Rejected after success"),
        Err(CollectionStoreError::Conflict {
            expected: 99,
            actual: created.revision(),
        })
    );
    assert!(!db.take_last_mutation_applied());
    assert!(!temp.path().join("collection.db.bak1").exists());

    db.delete_collection(created.collection_id(), created.revision())
        .unwrap();
    assert!(temp.path().join("collection.db.bak1").is_file());
}

#[test]
fn external_change_between_backup_and_apply_is_rejected_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let created = {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        db.create_collection("Before").unwrap()
    };
    let mut db = CollectionStoreDb::open_at(&path).unwrap();
    let hook_path = path.clone();
    let id = created.collection_id();
    db.set_before_apply_hook(move || {
        let conn = Connection::open(hook_path).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute(
            "UPDATE collections SET name = 'External', revision = revision + 1 WHERE id = ?1",
            [id.to_string()],
        )
        .unwrap();
        tx.execute(
            "UPDATE collection_meta SET catalog_revision = catalog_revision + 1 WHERE singleton = 1",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
    });
    assert!(matches!(
        db.rename_collection(
            created.collection_id(),
            created.revision(),
            "Must not overwrite external",
        ),
        Err(CollectionStoreError::Conflict { .. })
    ));
    assert!(!db.take_last_mutation_applied());
    let current = db.snapshot(created.collection_id()).unwrap();
    assert_eq!(current.definition.name, "External");
    assert_eq!(current.revision(), created.revision() + 1);
    let backup = CollectionStoreDb::open_at(&temp.path().join("collection.db.bak1")).unwrap();
    assert_eq!(
        backup
            .snapshot(created.collection_id())
            .unwrap()
            .definition
            .name,
        "Before"
    );
}

#[test]
fn v1_backup_failure_remains_fail_closed_before_schema_write() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    drop(CollectionStoreDb::open_at(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE collections DROP COLUMN shuffle_seed; PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(conn);
    std::fs::write(temp.path().join("collection.db.bak1"), b"known good").unwrap();
    std::fs::create_dir(temp.path().join("collection.db.bak.tmp-snapshot")).unwrap();
    assert!(matches!(
        CollectionStoreDb::open_at(&path),
        Err(CollectionStoreError::Persistence(_))
    ));
    let conn = Connection::open(&path).unwrap();
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(
        std::fs::read(temp.path().join("collection.db.bak1")).unwrap(),
        b"known good"
    );
}

#[test]
fn v1_schema_write_failure_keeps_original_and_completed_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        db.create_collection("Preserved").unwrap();
    }
    for generation in 1..=10 {
        let _ = std::fs::remove_file(temp.path().join(format!("collection.db.bak{generation}")));
    }
    let conn = Connection::open(&path).unwrap();
    // validate_existing_v1 accepts this row shape, while ALTER ADD COLUMN must fail.
    conn.pragma_update(None, "user_version", 1).unwrap();
    drop(conn);

    assert!(matches!(
        CollectionStoreDb::open_at(&path),
        Err(CollectionStoreError::Persistence(_))
    ));
    let backup = temp.path().join("collection.db.bak1");
    for candidate in [&path, &backup] {
        let conn =
            Connection::open_with_flags(candidate, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM collections", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        assert_eq!(count, 1);
    }
}

#[test]
fn existing_unversioned_backup_failure_remains_fail_closed_before_schema_write() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        0
    );
    drop(conn);
    std::fs::write(temp.path().join("collection.db.bak1"), b"known good").unwrap();
    std::fs::create_dir(temp.path().join("collection.db.bak.tmp-snapshot")).unwrap();

    assert!(matches!(
        CollectionStoreDb::open_at(&path),
        Err(CollectionStoreError::Persistence(_))
    ));
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        0
    );
    let schema_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN
             ('collection_meta', 'collections', 'collection_entries')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(schema_rows, 0);
    assert_eq!(
        std::fs::read(temp.path().join("collection.db.bak1")).unwrap(),
        b"known good"
    );
}

#[test]
fn corrupt_v1_schema_does_not_rotate_a_known_good_generation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    drop(CollectionStoreDb::open_at(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE collections DROP COLUMN shuffle_seed; PRAGMA user_version = 1;",
    )
    .unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "INSERT INTO collection_entries
         (id, collection_id, source_namespace, source_path, normalized_path,
          resolved_kind, manual_position, created_at_ms)
         VALUES (?1, ?2, 'filesystem_path', ?3, ?4, 'image', 0, 0)",
        rusqlite::params![
            CollectionEntryId::new().to_string(),
            CollectionId::new().to_string(),
            r"C:\broken-v1\image.png",
            "c:/broken-v1/image.png",
        ],
    )
    .unwrap();
    drop(conn);
    let backup = temp.path().join("collection.db.bak1");
    std::fs::write(&backup, b"known good").unwrap();
    assert!(
        matches!(CollectionStoreDb::open_at(&path), Err(CollectionStoreError::Persistence(message)) if message.contains("foreign key"))
    );
    assert_eq!(std::fs::read(&backup).unwrap(), b"known good");
    assert!(!temp.path().join("collection.db.bak2").exists());
    let conn = Connection::open(&path).unwrap();
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn one_actor_export_request_freezes_catalog_and_all_member_revisions() {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    let temp = tempfile::tempdir().unwrap();
    let runtime = CollectionStoreRuntime::start_at(temp.path().join("collection.db")).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    let first = client
        .create_collection("First".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let second = client
        .create_collection("Second".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let progress = std::sync::Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let frozen = client
        .export_all_snapshot(cancel, std::sync::Arc::clone(&progress))
        .unwrap();
    let changed = client
        .rename_collection(first.collection_id(), first.revision(), "After".into())
        .unwrap();
    let bundle = frozen
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    changed
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert_eq!(
        bundle
            .catalog
            .definitions
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        vec!["First", "Second"]
    );
    assert_eq!(
        bundle
            .snapshots
            .iter()
            .map(|row| row.definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["First", "Second"]
    );
    assert!(
        bundle
            .snapshots
            .iter()
            .all(|row| row.catalog_revision == bundle.catalog.catalog_revision)
    );
    assert_eq!(bundle.snapshots[1].collection_id(), second.collection_id());
    assert_eq!(progress.0.load(std::sync::atomic::Ordering::Acquire), 2);

    let cancelled = std::sync::Arc::new(AtomicBool::new(true));
    let receiver = client
        .export_all_snapshot(
            cancelled,
            std::sync::Arc::new((AtomicUsize::new(0), AtomicUsize::new(0))),
        )
        .unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
        Err(CollectionStoreError::Cancelled)
    ));
    runtime.shutdown_and_join();
}

#[test]
fn shuffle_seed_roundtrips_full_u64_and_reselection_advances_revision_without_moving_manual_positions()
 {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let mut db = open_db(&temp);
    let created = db.create_collection("Shuffle").unwrap();
    let added = db
        .add_batch(
            created.collection_id(),
            created.revision(),
            vec![
                registration(r"C:\shuffle\a.jpg", CollectionResolvedKind::Image),
                registration(r"C:\shuffle\b.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap()
        .snapshot;
    let positions = added
        .entries
        .iter()
        .map(|entry| (entry.id, entry.manual_position))
        .collect::<Vec<_>>();
    let first = db
        .set_order(
            created.collection_id(),
            added.revision(),
            CollectionOrderMode::Shuffle,
            SortOrder::FileName,
        )
        .unwrap();
    let second = db
        .set_order(
            created.collection_id(),
            first.revision(),
            CollectionOrderMode::Shuffle,
            SortOrder::FileName,
        )
        .unwrap();
    assert_eq!(second.revision(), first.revision() + 1);
    assert_eq!(second.catalog_revision, first.catalog_revision + 1);
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| (entry.id, entry.manual_position))
            .collect::<Vec<_>>(),
        positions
    );
    drop(db);
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE collections SET shuffle_seed = 'ffffffffffffffff' WHERE id = ?1",
        [created.collection_id().to_string()],
    )
    .unwrap();
    drop(conn);
    let reopened = open_db(&temp).snapshot(created.collection_id()).unwrap();
    assert_eq!(reopened.definition.shuffle_seed, u64::MAX);
}

#[test]
fn collection_standard_sort_roundtrips_every_list_sort_variant() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let created = db.create_collection("Sort variants").unwrap();
    let collection_id = created.collection_id();
    let mut revision = created.revision();

    for &sort in SortOrder::all() {
        let updated = db
            .set_order(collection_id, revision, CollectionOrderMode::Standard, sort)
            .unwrap();
        revision = updated.revision();
        drop(db);

        db = open_db(&temp);
        let reopened = db.snapshot(collection_id).unwrap();
        assert_eq!(reopened.definition.standard_sort, sort, "{sort:?}");
        assert_eq!(reopened.revision(), revision);
    }
}

#[test]
fn database_mutation_lifecycle_preserves_ids_compacts_positions_and_cascades() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let (deleted_ids, surviving_ids) = {
        let mut db = CollectionStoreDb::open_at(&path).unwrap();
        let keep = db.create_collection("Keep").unwrap();
        let doomed = db.create_collection("Doomed").unwrap();
        let third = db.create_collection("Third").unwrap();
        let doomed = db
            .add_batch(
                doomed.collection_id(),
                doomed.revision(),
                vec![registration(
                    r"C:\gone\one.jpg",
                    CollectionResolvedKind::Image,
                )],
            )
            .unwrap()
            .snapshot;
        let deleted_collection = doomed.collection_id();
        let catalog = db
            .delete_collection(deleted_collection, doomed.revision())
            .unwrap();
        assert_eq!(
            catalog
                .definitions
                .iter()
                .map(|definition| definition.id)
                .collect::<Vec<_>>(),
            vec![keep.collection_id(), third.collection_id()]
        );
        let fourth = db.create_collection("Fourth").unwrap();
        assert_eq!(
            db.catalog()
                .unwrap()
                .definitions
                .iter()
                .map(|definition| definition.id)
                .collect::<Vec<_>>(),
            vec![
                keep.collection_id(),
                third.collection_id(),
                fourth.collection_id()
            ]
        );

        let keep = db
            .add_batch(
                keep.collection_id(),
                keep.revision(),
                vec![
                    registration(r"C:\keep\missing.mp4", CollectionResolvedKind::Unresolved),
                    registration(r"C:\keep\other.mp4", CollectionResolvedKind::Video),
                ],
            )
            .unwrap()
            .snapshot;
        let first_id = keep.entries[0].id;
        let second_id = keep.entries[1].id;
        assert_eq!(
            db.relink(
                keep.collection_id(),
                99,
                first_id,
                registration(r"C:\keep\missing.mp4", CollectionResolvedKind::Video),
            ),
            Err(CollectionStoreError::Conflict {
                expected: 99,
                actual: keep.revision()
            })
        );
        let relinked = db
            .relink(
                keep.collection_id(),
                keep.revision(),
                first_id,
                registration(r"C:\keep\missing.mp4", CollectionResolvedKind::Video),
            )
            .unwrap();
        assert_eq!(relinked.entries[0].id, first_id);
        assert_eq!(relinked.entries[0].manual_position, 0);
        assert_eq!(
            relinked.entries[0].resolved_kind,
            CollectionResolvedKind::Video
        );
        assert!(matches!(
            db.relink(
                relinked.collection_id(),
                relinked.revision(),
                first_id,
                registration(r"C:\keep\other.mp4", CollectionResolvedKind::Video),
            ),
            Err(CollectionStoreError::DuplicateSource(_))
        ));
        assert_eq!(
            db.reorder_manual(
                relinked.collection_id(),
                relinked.revision(),
                vec![first_id, first_id],
            ),
            Err(CollectionStoreError::InvalidOrder)
        );
        assert_eq!(
            db.snapshot(relinked.collection_id()).unwrap().revision(),
            relinked.revision()
        );
        let removed = db
            .remove_entries(
                relinked.collection_id(),
                relinked.revision(),
                vec![second_id, second_id],
            )
            .unwrap();
        assert_eq!(removed.entries.len(), 1);
        assert_eq!(removed.entries[0].id, first_id);
        assert_eq!(removed.entries[0].manual_position, 0);
        let catalog = db
            .delete_collection(removed.collection_id(), removed.revision())
            .unwrap();
        assert_eq!(
            catalog
                .definitions
                .iter()
                .map(|definition| definition.id)
                .collect::<Vec<_>>(),
            vec![third.collection_id(), fourth.collection_id()]
        );
        (
            vec![deleted_collection, removed.collection_id()],
            vec![third.collection_id(), fourth.collection_id()],
        )
    };

    let conn = Connection::open(&path).unwrap();
    for id in deleted_ids {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_entries WHERE collection_id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    let catalog = CollectionStoreDb::open_at(&path)
        .unwrap()
        .catalog()
        .unwrap();
    assert_eq!(
        catalog
            .definitions
            .iter()
            .map(|definition| definition.id)
            .collect::<Vec<_>>(),
        surviving_ids
    );
}

#[test]
fn no_op_and_conflict_do_not_advance_revision() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let created = db.create_collection("One").unwrap();
    let same = db
        .rename_collection(created.collection_id(), created.revision(), "One")
        .unwrap();
    assert_eq!(same.revision(), created.revision());
    assert_eq!(same.catalog_revision, created.catalog_revision);
    assert_eq!(
        db.rename_collection(created.collection_id(), 99, "Other"),
        Err(CollectionStoreError::Conflict {
            expected: 99,
            actual: 1
        })
    );
    assert_eq!(db.snapshot(created.collection_id()).unwrap(), created);
}

#[test]
fn migration_is_atomic_across_collections_and_preserves_entry_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let first = db.create_collection("First").unwrap();
    let second = db.create_collection("Second").unwrap();
    let first = db
        .add_batch(
            first.collection_id(),
            first.revision(),
            vec![registration(
                r"C:\Old\Album\A.jpg",
                CollectionResolvedKind::Image,
            )],
        )
        .unwrap()
        .snapshot;
    let second = db
        .add_batch(
            second.collection_id(),
            second.revision(),
            vec![
                registration(r"C:\Old\Album\B.jpg", CollectionResolvedKind::Image),
                registration(r"D:\New\Album\B.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap()
        .snapshot;
    let first_entry = first.entries[0].id;
    let second_entry = second.entries[0].id;
    let migration = CollectionSourceMigration::from_trusted_paths(
        r"C:\Old\Album",
        r"D:\New\Album",
        CollectionSourceMigrationScope::Tree,
    )
    .unwrap();
    assert!(matches!(
        db.migrate_sources(migration.clone()),
        Err(CollectionStoreError::DuplicateSource(_))
    ));
    assert_eq!(
        db.snapshot(first.collection_id()).unwrap().entries[0].source_path,
        PathBuf::from(r"C:\Old\Album\A.jpg")
    );
    assert_eq!(
        db.snapshot(second.collection_id()).unwrap().entries[0].source_path,
        PathBuf::from(r"C:\Old\Album\B.jpg")
    );

    let second = db
        .remove_entries(
            second.collection_id(),
            second.revision(),
            vec![second.entries[1].id],
        )
        .unwrap();
    let before_first_revision = first.revision();
    let before_second_revision = second.revision();
    let outcome = db.migrate_sources(migration).unwrap();
    assert_eq!(outcome.updated_entries, 2);
    assert_eq!(outcome.affected.len(), 2);
    let first_after = db.snapshot(first.collection_id()).unwrap();
    let second_after = db.snapshot(second.collection_id()).unwrap();
    assert_eq!(first_after.revision(), before_first_revision + 1);
    assert_eq!(second_after.revision(), before_second_revision + 1);
    assert_eq!(first_after.entries[0].id, first_entry);
    assert_eq!(second_after.entries[0].id, second_entry);
    assert_eq!(
        first_after.entries[0].source_path,
        PathBuf::from(r"D:\New\Album\A.jpg")
    );
    assert_eq!(
        second_after.entries[0].source_path,
        PathBuf::from(r"D:\New\Album\B.jpg")
    );
}

#[test]
fn migration_batch_swaps_sources_in_one_transaction_and_rolls_back_duplicates() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let created = db.create_collection("Swap").unwrap();
    let snapshot = db
        .add_batch(
            created.collection_id(),
            created.revision(),
            vec![
                registration(r"C:\Book\001.jpg", CollectionResolvedKind::Image),
                registration(r"C:\Book\002.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap()
        .snapshot;
    let ids = snapshot
        .entries
        .iter()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    let batch = CollectionSourceMigrationBatch::from_trusted_paths([
        (
            r"C:\Book\001.jpg",
            r"C:\Book\002.jpg",
            CollectionSourceMigrationScope::Exact,
        ),
        (
            r"C:\Book\002.jpg",
            r"C:\Book\001.jpg",
            CollectionSourceMigrationScope::Exact,
        ),
    ])
    .unwrap();
    let outcome = db.migrate_source_batch(batch).unwrap();
    assert_eq!(outcome.updated_entries, 2);
    let swapped = db.snapshot(created.collection_id()).unwrap();
    assert_eq!(swapped.entries[0].id, ids[0]);
    assert_eq!(
        swapped.entries[0].source_path,
        PathBuf::from(r"C:\Book\002.jpg")
    );
    assert_eq!(swapped.entries[1].id, ids[1]);
    assert_eq!(
        swapped.entries[1].source_path,
        PathBuf::from(r"C:\Book\001.jpg")
    );

    let duplicate = CollectionSourceMigrationBatch::from_trusted_paths([
        (
            r"C:\Book\001.jpg",
            r"C:\Book\same.jpg",
            CollectionSourceMigrationScope::Exact,
        ),
        (
            r"C:\Book\002.jpg",
            r"C:\Book\same.jpg",
            CollectionSourceMigrationScope::Exact,
        ),
    ])
    .unwrap();
    assert!(matches!(
        db.migrate_source_batch(duplicate),
        Err(CollectionStoreError::DuplicateSource(_))
    ));
    assert_eq!(db.snapshot(created.collection_id()).unwrap(), swapped);
}

#[test]
fn standard_order_uses_aligned_facts_while_manual_ignores_them() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let snapshot = db.create_collection("Order").unwrap();
    let snapshot = db
        .add_batch(
            snapshot.collection_id(),
            snapshot.revision(),
            vec![
                registration(r"C:\x\b.jpg", CollectionResolvedKind::Image),
                registration(r"C:\x\a.jpg", CollectionResolvedKind::Image),
            ],
        )
        .unwrap()
        .snapshot;
    let ids: Vec<_> = snapshot.entries.iter().map(|entry| entry.id).collect();
    assert_eq!(effective_collection_order(&snapshot, &[]).unwrap(), ids);
    let snapshot = db
        .set_order(
            snapshot.collection_id(),
            snapshot.revision(),
            CollectionOrderMode::Standard,
            SortOrder::SizeAsc,
        )
        .unwrap();
    let facts = vec![
        CollectionSortFacts {
            entry_id: ids[0],
            category_rank: 1,
            name: "b.jpg".into(),
            mtime: 0,
            file_size: None,
        },
        CollectionSortFacts {
            entry_id: ids[1],
            category_rank: 1,
            name: "a.jpg".into(),
            mtime: 0,
            file_size: Some(0),
        },
    ];
    assert_eq!(
        effective_collection_order(&snapshot, &facts).unwrap(),
        vec![ids[1], ids[0]]
    );
    assert_eq!(
        effective_collection_order(&snapshot, &facts[..1]),
        Err(CollectionStoreError::InvalidOrder)
    );
}

#[test]
fn shuffle_order_uses_only_seed_and_entry_identity_and_preserves_standard_fact_validation() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let created = db.create_collection("Shuffle order").unwrap();
    let sources = (0..12)
        .map(|index| {
            registration(
                &format!(r"C:\shuffle\{index:02}.jpg"),
                CollectionResolvedKind::Image,
            )
        })
        .collect();
    let manual = db
        .add_batch(created.collection_id(), created.revision(), sources)
        .unwrap()
        .snapshot;
    let mut shuffle = db
        .set_order(
            created.collection_id(),
            manual.revision(),
            CollectionOrderMode::Shuffle,
            SortOrder::FileName,
        )
        .unwrap();
    shuffle.definition.shuffle_seed = 0x0123456789abcdef;
    let first = effective_collection_order(&shuffle, &[]).unwrap();
    assert_eq!(first.len(), manual.entries.len());
    assert_eq!(
        first
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        first.len()
    );
    let mut permuted = shuffle.clone();
    permuted.entries =
        std::sync::Arc::from(shuffle.entries.iter().rev().cloned().collect::<Vec<_>>());
    assert_eq!(effective_collection_order(&permuted, &[]).unwrap(), first);
    permuted.definition.shuffle_seed = 0xfedcba9876543210;
    assert_ne!(effective_collection_order(&permuted, &[]).unwrap(), first);
    let fixed_ids = (1..=5)
        .map(|number| {
            CollectionEntryId::from_uuid(
                uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut fixed = shuffle.clone();
    fixed.definition.shuffle_seed = 0x0123456789abcdef;
    fixed.entries = std::sync::Arc::from(
        shuffle.entries[..5]
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let mut entry = entry.clone();
                entry.id = fixed_ids[index];
                entry
            })
            .collect::<Vec<_>>(),
    );
    // SHA-256(seed LE bytes || UUID raw bytes), digest ascending, UUID tie-break.
    assert_eq!(
        effective_collection_order(&fixed, &[]).unwrap(),
        vec![
            fixed_ids[1],
            fixed_ids[0],
            fixed_ids[4],
            fixed_ids[3],
            fixed_ids[2],
        ]
    );
    let mut standard = shuffle;
    standard.definition.order_mode = CollectionOrderMode::Standard;
    assert_eq!(
        effective_collection_order(&standard, &[]),
        Err(CollectionStoreError::InvalidOrder)
    );
}

#[test]
fn latest_next_uses_current_prepared_facts_and_id_source_head_anchor_policy() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = open_db(&temp);
    let snapshot = db.create_collection("Video").unwrap();
    let snapshot = db
        .add_batch(
            snapshot.collection_id(),
            snapshot.revision(),
            vec![
                registration(r"C:\v\a.mp4", CollectionResolvedKind::Unresolved),
                registration(r"C:\v\stale.mp4", CollectionResolvedKind::Video),
                registration(r"C:\v\b.mp4", CollectionResolvedKind::Video),
            ],
        )
        .unwrap()
        .snapshot;
    let order: Vec<_> = snapshot.entries.iter().map(|entry| entry.id).collect();
    let prepared_facts = vec![
        CollectionPreparedNavigationFact {
            entry_id: order[0],
            source_state: CollectionPreparedSourceState::Available(CollectionResolvedKind::Video),
        },
        CollectionPreparedNavigationFact {
            entry_id: order[1],
            source_state: CollectionPreparedSourceState::Unavailable,
        },
        CollectionPreparedNavigationFact {
            entry_id: order[2],
            source_state: CollectionPreparedSourceState::Available(CollectionResolvedKind::Video),
        },
    ];
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts,
            &CollectionCurrentEntry {
                entry_id: Some(order[0]),
                source_key: None,
            },
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        )
        .unwrap(),
        Some(order[2])
    );
    let old_key = snapshot.entries[0].source_key.clone();
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts,
            &CollectionCurrentEntry {
                entry_id: Some(CollectionEntryId::new()),
                source_key: Some(old_key),
            },
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        )
        .unwrap(),
        Some(order[2])
    );
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts,
            &CollectionCurrentEntry::default(),
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        )
        .unwrap(),
        Some(order[0])
    );
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts,
            &CollectionCurrentEntry {
                entry_id: Some(order[2]),
                source_key: None,
            },
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        )
        .unwrap(),
        None
    );
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts,
            &CollectionCurrentEntry {
                entry_id: Some(order[2]),
                source_key: None,
            },
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Loop,
        )
        .unwrap(),
        Some(order[0])
    );
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &order,
            &prepared_facts[..2],
            &CollectionCurrentEntry::default(),
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        ),
        Err(CollectionStoreError::InvalidOrder)
    );
    assert_eq!(
        resolve_latest_collection_next(
            &snapshot,
            &[order[0], order[0], order[2]],
            &prepared_facts,
            &CollectionCurrentEntry::default(),
            CollectionPlaybackKind::Video,
            CollectionTailBehavior::Stop,
        ),
        Err(CollectionStoreError::InvalidOrder)
    );
}

#[test]
fn runtime_fans_out_latest_revision_and_shutdown_closes_all_clone_admission() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("collection.db");
    let mut runtime = CollectionStoreRuntime::start_at(db_path.clone()).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    let clone = client.clone();
    let first_watch = client.subscribe().unwrap();
    let second_watch = clone.subscribe().unwrap();

    let first_created = client
        .create_collection("One".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let second_created = client
        .create_collection("Two".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let first = wait_notice(&first_watch, second_created.catalog_revision);
    let second = wait_notice(&second_watch, second_created.catalog_revision);
    assert_eq!(first, second);
    assert_eq!(first.collection_revisions.len(), 2);
    assert!(
        first
            .collection_revisions
            .contains(&(first_created.collection_id(), 1))
    );
    assert!(
        first
            .collection_revisions
            .contains(&(second_created.collection_id(), 1))
    );

    runtime.begin_shutdown();
    assert!(matches!(
        clone.create_collection("Late".into()),
        Err(CollectionStoreError::Unavailable)
    ));
    runtime.shutdown_and_join();
    let db = CollectionStoreDb::open_at(&db_path).unwrap();
    assert_eq!(db.catalog().unwrap().definitions.len(), 2);
}

#[test]
fn shutdown_rejects_queued_commands_without_mutating_them() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("collection.db");
    let mut runtime = CollectionStoreRuntime::start_at(db_path.clone()).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    let mut replies = Vec::new();
    for index in 0..COMMAND_TEST_REQUESTS {
        match client.create_collection(format!("Collection {index}")) {
            Ok(reply) => replies.push(reply),
            Err(CollectionStoreError::Busy) => break,
            other => panic!("unexpected enqueue result: {other:?}"),
        }
    }
    runtime.begin_shutdown();
    let mut committed = 0;
    for reply in replies {
        match reply.recv_timeout(Duration::from_secs(5)).unwrap() {
            Ok(_) => committed += 1,
            Err(CollectionStoreError::Unavailable) => {}
            other => panic!("unexpected command result: {other:?}"),
        }
    }
    assert!(matches!(
        client.create_collection("after linearization".into()),
        Err(CollectionStoreError::Unavailable)
    ));
    runtime.shutdown_and_join();
    let db = CollectionStoreDb::open_at(&db_path).unwrap();
    assert_eq!(db.catalog().unwrap().definitions.len(), committed);
}

#[test]
fn actor_prioritizes_shutdown_over_a_command_queued_behind_in_flight_work() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("collection.db");
    let mut runtime = CollectionStoreRuntime::start_at(db_path.clone()).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    let (barrier_reply, entered, release) = client.test_barrier().unwrap();
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let late = client.create_collection("must not commit".into()).unwrap();
    runtime.begin_shutdown();
    release.send(()).unwrap();
    barrier_reply
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert_eq!(
        late.recv_timeout(Duration::from_secs(3)).unwrap(),
        Err(CollectionStoreError::Unavailable)
    );
    runtime.shutdown_and_join();
    let db = CollectionStoreDb::open_at(&db_path).unwrap();
    assert!(db.catalog().unwrap().definitions.is_empty());
}

const COMMAND_TEST_REQUESTS: usize = 256;

#[test]
fn startup_failure_is_typed_and_does_not_open_normal_data() {
    let temp = tempfile::tempdir().unwrap();
    let parent_file = temp.path().join("not-a-directory");
    std::fs::write(&parent_file, b"x").unwrap();
    let runtime = CollectionStoreRuntime::start_at(parent_file.join("collection.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(CollectionRuntimeEvent::Failed(_)) = runtime.try_recv_event() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "startup failure was not published"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        runtime.client().create_collection("No".into()),
        Err(CollectionStoreError::Unavailable)
    ));
}

#[test]
fn actor_panic_closes_admission_and_publishes_failure() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = CollectionStoreRuntime::start_at(temp.path().join("collection.db")).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    client.test_panic().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(CollectionRuntimeEvent::Failed(_)) = runtime.try_recv_event() {
            break;
        }
        assert!(Instant::now() < deadline, "actor panic was not published");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        client.create_collection("after panic".into()),
        Err(CollectionStoreError::Unavailable)
    ));
    runtime.shutdown_and_join();
}

#[test]
fn settings_full_reset_does_not_change_collection_family_or_runtime_snapshot() {
    let guard = crate::settings_db::DataDirOverrideGuard::new();
    let data_dir = guard.path();
    {
        let db = crate::settings_db::SettingsDb::create_new(data_dir).unwrap();
        db.save_full(&crate::settings::Settings::default()).unwrap();
    }
    let collection_path = data_dir.join("collection.db");
    let runtime = CollectionStoreRuntime::start_at(collection_path.clone()).unwrap();
    wait_ready(&runtime);
    let client = runtime.client();
    let created = client
        .create_collection("Survives settings reset".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let before_snapshot = client
        .load_collection(created.collection_id())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let before_files = collection_family_bytes(&collection_path);
    let backup = data_dir.join("collection.db.bak1");
    std::fs::write(&backup, b"collection backup survives reset").unwrap();

    let permit = crate::settings_db::quiesce_settings_family().unwrap();
    let reset = crate::settings_restore::full_reset_with_permit(data_dir, &permit).unwrap();
    permit.resume_local().unwrap();
    assert!(reset.deleted.iter().all(|path| {
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("collection.db"))
    }));
    assert_eq!(collection_family_bytes(&collection_path), before_files);
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        b"collection backup survives reset"
    );

    let after_snapshot = client
        .load_collection(created.collection_id())
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert_eq!(after_snapshot, before_snapshot);
    runtime.shutdown_and_join();
}

#[test]
fn newer_schema_is_not_replaced_with_an_empty_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("collection.db");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let shm = PathBuf::from(format!("{}-shm", path.display()));
    assert!(!wal.exists());
    assert!(!shm.exists());
    assert!(matches!(
        CollectionStoreDb::open_at(&path),
        Err(CollectionStoreError::IncompatibleSchema(999))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!wal.exists());
    assert!(!shm.exists());
}

fn wait_ready(runtime: &CollectionStoreRuntime) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match runtime.try_recv_event() {
            Some(CollectionRuntimeEvent::Ready(_)) => return,
            Some(event) => panic!("collection runtime did not start: {event:?}"),
            None => {}
        }
        assert!(
            Instant::now() < deadline,
            "collection runtime startup timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn collection_family_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
    .into_iter()
    .filter_map(|candidate| {
        let bytes = std::fs::read(&candidate).ok()?;
        Some((candidate.file_name()?.to_string_lossy().into_owned(), bytes))
    })
    .collect()
}

fn wait_notice(
    watch: &CollectionRevisionWatch,
    expected_catalog_revision: u64,
) -> CollectionRevisionNotice {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(notice) = watch.take_latest()
            && notice.catalog_revision >= expected_catalog_revision
        {
            return notice;
        }
        assert!(Instant::now() < deadline, "revision watch timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
