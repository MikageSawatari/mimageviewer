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
    );
    assert_eq!(preview.lines.len(), 4);
    assert_eq!(
        preview.lines[0].status,
        CollectionImportLineStatus::Accepted
    );
    assert_eq!(
        preview.lines[1].status,
        CollectionImportLineStatus::Accepted
    );
    assert_eq!(
        preview.lines[2].status,
        CollectionImportLineStatus::Duplicate { first_line: 2 }
    );
    assert!(matches!(
        preview.lines[3].status,
        CollectionImportLineStatus::Invalid { .. }
    ));
    let accepted: Vec<_> = preview
        .accepted_paths()
        .map(|(path, _)| path.to_path_buf())
        .collect();
    assert_eq!(accepted.len(), 2);
    assert!(accepted[0].to_string_lossy().contains("NeverExists"));

    let serialized = serialize_collection_paths(accepted.iter().map(PathBuf::as_path));
    assert!(serialized.starts_with(r#""C:\NeverExists\missing folder\a.jpg""#));
    assert!(serialized.ends_with("\r\n"));
    let reparsed = parse_collection_text(&serialized, Path::new(r"C:\Export\collection.txt"));
    assert_eq!(
        reparsed
            .accepted_paths()
            .map(|(path, _)| path.to_path_buf())
            .collect::<Vec<_>>(),
        accepted
    );
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
