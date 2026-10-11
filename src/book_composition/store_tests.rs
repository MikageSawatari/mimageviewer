use super::store::*;
use super::*;
use crate::book_scope::BookScopeKey;
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

fn book() -> BookComposition {
    fixtures::four_page_book(ReadingDirection::Rtl)
}
fn scope() -> BookScopeKey {
    BookScopeKey::document("C:/books/book.pdf").unwrap()
}
fn database() -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    initialize_database_at(&temp.path().join(DB_FILE)).unwrap();
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    (temp, connection)
}
fn set(
    connection: &mut Connection,
    key: &str,
    domain: &PageDomain,
    composition: &BookComposition,
) -> bool {
    let tx = connection.transaction().unwrap();
    let changed = replace_row_in(&tx, "book_compositions", key, domain, composition).unwrap();
    tx.commit().unwrap();
    changed
}
fn prepare(
    service: &BookCompositionService,
    scope: BookScopeKey,
    domain: PageDomain,
) -> CompositionPrepareReply {
    service
        .prepare(scope, domain)
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
}

#[test]
fn transaction_roundtrip_revisions_and_invalid_replacement_preserve_old_record() {
    let (temp, mut connection) = database();
    let key = scope().storage_key();
    assert!(set(&mut connection, &key, &PageDomain::Document, &book()));
    let original = load_rows(&connection).unwrap();
    assert_eq!(original[&key].composition.revision, 1);
    let mut imported = book();
    imported.revision = 999;
    assert!(!set(
        &mut connection,
        &key,
        &PageDomain::Document,
        &imported
    ));
    imported.direction = ReadingDirection::Ltr;
    assert!(set(&mut connection, &key, &PageDomain::Document, &imported));
    assert_eq!(
        load_rows(&connection).unwrap()[&key].composition.revision,
        2
    );
    let before = load_rows(&connection).unwrap();
    let mut invalid = book();
    invalid.units.push(invalid.units[0].clone());
    let tx = connection.transaction().unwrap();
    assert!(
        replace_row_in(
            &tx,
            "book_compositions",
            &key,
            &PageDomain::Document,
            &invalid
        )
        .is_err()
    );
    drop(tx);
    assert_eq!(load_rows(&connection).unwrap(), before);
    drop(connection);
    let reopened = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert_eq!(load_rows(&reopened).unwrap(), before);
}

#[test]
fn attached_batch_savepoint_rollback_preserves_composition_and_existing_metadata() {
    let (temp, _) = database();
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE metadata(value INTEGER); INSERT INTO metadata VALUES(1)")
        .unwrap();
    connection
        .execute(
            "ATTACH DATABASE ?1 AS book_composition",
            [temp.path().join(DB_FILE).to_str().unwrap()],
        )
        .unwrap();
    connection.execute_batch("SAVEPOINT item").unwrap();
    assert!(
        replace_row_in(
            &connection,
            "book_composition.book_compositions",
            &scope().storage_key(),
            &PageDomain::Document,
            &book()
        )
        .unwrap()
    );
    connection
        .execute("UPDATE metadata SET value=2", [])
        .unwrap();
    connection
        .execute_batch("ROLLBACK TO item; RELEASE item")
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM book_composition.book_compositions",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM metadata", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        1
    );
}

#[test]
fn malformed_unknown_schema_domain_and_version_are_preserved() {
    for corruption in ["unknown_schema", "json", "row_revision", "domain"] {
        let (temp, mut connection) = database();
        let key = scope().storage_key();
        set(&mut connection, &key, &PageDomain::Document, &book());
        match corruption {
            "unknown_schema" => connection.execute_batch("PRAGMA user_version=999").unwrap(),
            "json" => {
                connection
                    .execute(
                        "UPDATE book_compositions SET composition_json='not-json'",
                        [],
                    )
                    .unwrap();
            }
            "row_revision" => {
                connection
                    .execute("UPDATE book_compositions SET revision=42", [])
                    .unwrap();
            }
            _ => {
                let wire = encode_row(
                    &key,
                    &PageDomain::Document,
                    &load_rows(&connection).unwrap()[&key].composition,
                )
                .unwrap()
                .replace("\"kind\":\"document\"", "\"kind\":\"image_folder\"");
                connection
                    .execute("UPDATE book_compositions SET composition_json=?1", [wire])
                    .unwrap();
            }
        }
        let before = std::fs::read(temp.path().join(DB_FILE)).unwrap();
        assert!(initialize_database_at(&temp.path().join(DB_FILE)).is_err());
        assert!(open_existing(&temp.path().join(DB_FILE)).is_err());
        assert_eq!(std::fs::read(temp.path().join(DB_FILE)).unwrap(), before);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM book_compositions", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn archive_domain_matches_scope_and_preserves_original_member_spelling() {
    let (_temp, mut connection) = database();
    let domain = PageDomain::ArchiveLevel {
        prefix: "Chapter".into(),
    };
    let scope = BookScopeKey::new("D:/Books/A.zip", "chapter").unwrap();
    let composition = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionUnit::Single {
            page: CompositionPageRef::ArchiveEntry {
                entry_name: "Chapter/Page.JPG".into(),
            },
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    );
    validate_scope_domain(&scope, &domain).unwrap();
    assert!(set(
        &mut connection,
        &scope.storage_key(),
        &domain,
        &composition
    ));
    assert_eq!(
        load_rows(&connection).unwrap()[&scope.storage_key()]
            .composition
            .units,
        composition.units
    );
    assert!(
        validate_scope_domain(
            &scope,
            &PageDomain::ArchiveLevel {
                prefix: "other".into()
            }
        )
        .is_err()
    );
    assert!(encode_row("d:/books/a.zip/other", &domain, &composition).is_err());
}

#[test]
fn service_archive_domain_case_and_slash_equivalence_preserves_refs_and_noop_token() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = BookScopeKey::new("C:/Books/Book.ZIP", "chapter/sub").unwrap();
    let raw_domain = PageDomain::ArchiveLevel {
        prefix: "Chapter\\Sub".into(),
    };
    let canonical = PageDomain::ArchiveLevel {
        prefix: "chapter/sub".into(),
    };
    let composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![CompositionUnit::Single {
            page: CompositionPageRef::ArchiveEntry {
                entry_name: "Chapter\\Sub\\Page.JPG".into(),
            },
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    );
    let saved = service
        .save(scope.clone(), raw_domain, composition.clone())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(saved.changed && saved.status.is_ok());
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![scope.storage_key()]
    );
    let prepared = prepare(&service, scope.clone(), canonical.clone());
    assert_eq!(prepared.domain, canonical);
    let record = prepared.status.unwrap().unwrap();
    assert_eq!(record.domain, canonical);
    assert_eq!(record.composition.units, composition.units);
    let unchanged = service
        .save(scope.clone(), canonical.clone(), composition)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(unchanged.status.is_ok() && !unchanged.changed);
    assert_eq!(unchanged.token, saved.token);
    assert!(service.drain_changed_scope_keys().is_empty());
    assert!(
        prepare(
            &service,
            scope.clone(),
            PageDomain::ArchiveLevel {
                prefix: "different/sub".into()
            }
        )
        .status
        .is_err()
    );
    assert!(
        service
            .remove(scope, PageDomain::Document)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_err()
    );
    owner.shutdown_and_join();
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    let wire: String = connection
        .query_row(
            "SELECT composition_json FROM book_compositions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let wire: serde_json::Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(wire["domain"]["prefix"], "chapter/sub");
    assert_eq!(
        wire["composition"]["units"][0]["page"]["entry_name"],
        "Chapter\\Sub\\Page.JPG"
    );
}

#[test]
fn raw_spelled_valid_stored_archive_domain_reads_equivalently_without_rewriting_row() {
    let (temp, mut connection) = database();
    let scope = BookScopeKey::new("C:/Books/Book.ZIP", "chapter/sub").unwrap();
    let canonical = PageDomain::ArchiveLevel {
        prefix: "chapter/sub".into(),
    };
    let composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![CompositionUnit::Single {
            page: CompositionPageRef::ArchiveEntry {
                entry_name: "Chapter/Sub/Page.JPG".into(),
            },
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    );
    set(
        &mut connection,
        &scope.storage_key(),
        &canonical,
        &composition,
    );
    let wire: String = connection
        .query_row(
            "SELECT composition_json FROM book_compositions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut wire: serde_json::Value = serde_json::from_str(&wire).unwrap();
    wire["domain"]["prefix"] = serde_json::Value::String("CHAPTER\\Sub".into());
    let raw_wire = serde_json::to_string(&wire).unwrap();
    connection
        .execute(
            "UPDATE book_compositions SET composition_json=?1",
            [&raw_wire],
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(temp.path().join(DB_FILE)).unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let prepared = prepare(&service, scope.clone(), canonical.clone());
    let token = prepared.token;
    let record = prepared.status.unwrap().unwrap();
    assert_eq!(record.domain, canonical);
    assert_eq!(record.composition.units, composition.units);
    let unchanged = service
        .save(scope.clone(), canonical, composition)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(unchanged.status.is_ok() && !unchanged.changed);
    assert_eq!(unchanged.token, token);
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
    assert_eq!(std::fs::read(temp.path().join(DB_FILE)).unwrap(), before);
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    let persisted: String = connection
        .query_row(
            "SELECT composition_json FROM book_compositions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, raw_wire);
}

#[test]
fn service_reads_noop_remove_and_unchanged_maintenance_never_write_or_notify() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let initial = prepare(&service, scope(), PageDomain::Document);
    assert_eq!(initial.status, Ok(None));
    assert!(
        !service
            .remove(scope(), PageDomain::Document)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .changed
    );
    drop(maintenance_at(temp.path()).unwrap());
    assert_eq!(
        prepare(&service, scope(), PageDomain::Document).token,
        initial.token
    );
    assert!(service.drain_changed_scope_keys().is_empty());
    assert!(!temp.path().join(DB_FILE).exists());
    assert_eq!(
        migrate_page_paths_at(temp.path(), &[("c:/a".into(), "c:/b".into())]).unwrap(),
        0
    );
    owner.shutdown_and_join();
}

#[test]
fn service_fifo_recreate_tokens_and_sibling_notifications_are_exact() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let sibling = BookScopeKey::document("D:/books/book.pdf").unwrap();
    let initial = prepare(&service, sibling.clone(), PageDomain::Document);
    let saved = service
        .save(scope(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(saved.changed);
    assert_eq!(saved.status, Ok(()));
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![scope().storage_key()]
    );
    let no_op = service
        .save(scope(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(!no_op.changed);
    assert_eq!(saved.token, no_op.token);
    let removed = service.remove(scope(), PageDomain::Document);
    let read_removed = service.prepare(scope(), PageDomain::Document);
    let recreated = service.save(scope(), PageDomain::Document, book());
    assert!(
        removed
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .changed
    );
    assert_eq!(
        read_removed
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status,
        Ok(None)
    );
    let recreated = recreated.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_ne!(saved.token, recreated.token);
    assert_eq!(
        prepare(&service, sibling, PageDomain::Document).token,
        initial.token
    );
    owner.shutdown_and_join();
}

#[test]
fn immutable_snapshots_share_unchanged_records_across_mutation_and_maintenance() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let sibling = BookScopeKey::document("D:/books/sibling.pdf").unwrap();
    for scope in [scope(), sibling.clone()] {
        service
            .save(scope, PageDomain::Document, book())
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    }
    let before = service.snapshot();
    let mut changed = book();
    changed.direction = ReadingDirection::Ltr;
    service
        .save(scope(), PageDomain::Document, changed)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let after = service.snapshot();
    assert!(std::sync::Arc::ptr_eq(
        &before.rows[&sibling.storage_key()],
        &after.rows[&sibling.storage_key()]
    ));
    assert!(!std::sync::Arc::ptr_eq(
        &before.rows[&scope().storage_key()],
        &after.rows[&scope().storage_key()]
    ));
    assert_eq!(
        before.rows[&scope().storage_key()].composition.direction,
        ReadingDirection::Rtl
    );
    assert_eq!(
        after.rows[&scope().storage_key()].composition.direction,
        ReadingDirection::Ltr
    );
    drop(maintenance_at(temp.path()).unwrap());
    let unchanged = service.snapshot();
    for scope in [scope(), sibling] {
        assert!(std::sync::Arc::ptr_eq(
            &after.rows[&scope.storage_key()],
            &unchanged.rows[&scope.storage_key()]
        ));
    }
    owner.shutdown_and_join();
}

#[test]
fn maintenance_defers_prepare_rejects_new_writes_and_refreshes_only_changed_scope() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let sibling = BookScopeKey::document("D:/books/other.pdf").unwrap();
    let initial = prepare(&service, sibling.clone(), PageDomain::Document);
    let lease = maintenance_at(temp.path()).unwrap();
    let read = service.prepare(scope(), PageDomain::Document);
    assert!(read.try_recv().is_err());
    assert!(
        service
            .save(scope(), PageDomain::Document, book())
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_err()
    );
    initialize_database_at(&temp.path().join(DB_FILE)).unwrap();
    let mut connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    set(
        &mut connection,
        &scope().storage_key(),
        &PageDomain::Document,
        &book(),
    );
    drop(connection);
    drop(lease);
    assert!(
        read.recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .unwrap()
            .is_some()
    );
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![scope().storage_key()]
    );
    assert_eq!(
        prepare(&service, sibling, PageDomain::Document).token,
        initial.token
    );
    owner.shutdown_and_join();
}

#[test]
fn unreadable_maintenance_preserves_old_rows_blocks_reads_and_changes_token() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let saved = service
        .save(scope(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    service.drain_changed_scope_keys();
    let lease = maintenance_at(temp.path()).unwrap();
    let connection = Connection::open(temp.path().join(DB_FILE)).unwrap();
    connection.execute_batch("PRAGMA user_version=2").unwrap();
    drop(connection);
    drop(lease);
    let read = prepare(&service, scope(), PageDomain::Document);
    assert!(read.status.is_err());
    assert_ne!(read.token, saved.token);
    assert_eq!(service.snapshot().rows.len(), 1);
    assert!(
        service
            .save(scope(), PageDomain::Document, book())
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_err()
    );
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
}

#[test]
fn database_save_failure_rolls_back_and_keeps_old_record_and_read_token() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let saved = service
        .save(scope(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    service.drain_changed_scope_keys();
    let lease = maintenance_at(temp.path()).unwrap();
    let connection = Connection::open(temp.path().join(DB_FILE)).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_composition_save BEFORE UPDATE ON book_compositions BEGIN SELECT RAISE(ABORT,'injected disk failure'); END").unwrap();
    drop(connection);
    drop(lease);
    let mut draft = book();
    draft.direction = ReadingDirection::Ltr;
    let failed = service
        .save(scope(), PageDomain::Document, draft)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(!failed.changed);
    assert!(failed.status.is_err());
    assert_eq!(failed.token, saved.token);
    let read = prepare(&service, scope(), PageDomain::Document);
    assert_eq!(read.token, saved.token);
    assert_eq!(
        read.status.unwrap().unwrap().composition.direction,
        ReadingDirection::Rtl
    );
    assert!(service.drain_changed_scope_keys().is_empty());
    assert!(!service.drain_errors().is_empty());
    owner.shutdown_and_join();
}

#[test]
fn concurrent_saves_single_writer_unique_scope_and_no_front_drop_join() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    assert!(BookCompositionService::spawn_at(temp.path().into(), None).is_err());
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let service = service.clone();
            std::thread::spawn(move || {
                let scope = BookScopeKey::document(format!("C:/books/{index}.pdf")).unwrap();
                service
                    .save(scope, PageDomain::Document, book())
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
            })
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap().changed);
    }
    assert_eq!(service.snapshot().rows.len(), 8);
    assert_eq!(service.drain_changed_scope_keys().len(), 8);
    drop(service.clone());
    owner.shutdown_and_join();
}

#[test]
fn startup_after_existing_lease_is_async_and_reads_refreshed_database() {
    let temp = tempfile::tempdir().unwrap();
    let lease = maintenance_at(temp.path()).unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let read = service.prepare(scope(), PageDomain::Document);
    assert!(read.try_recv().is_err());
    initialize_database_at(&temp.path().join(DB_FILE)).unwrap();
    let mut connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    set(
        &mut connection,
        &scope().storage_key(),
        &PageDomain::Document,
        &book(),
    );
    drop(connection);
    drop(lease);
    assert!(
        read.recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .unwrap()
            .is_some()
    );
    assert!(service.drain_changed_scope_keys().is_empty());
    owner.shutdown_and_join();
}

#[test]
fn restarted_service_epoch_changes_even_with_retained_closed_front_handle() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let before = prepare(&service, scope(), PageDomain::Document).token;
    owner.shutdown_and_join();
    let (restarted, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    assert_ne!(
        prepare(&restarted, scope(), PageDomain::Document).token,
        before
    );
    assert!(
        prepare(&service, scope(), PageDomain::Document)
            .status
            .is_err()
    );
    owner.shutdown_and_join();
}

#[test]
fn root_rename_destination_priority_page_purge_and_writer_fifo() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let old = scope();
    let destination = BookScopeKey::document("C:/books/new.pdf").unwrap();
    service
        .save(old.clone(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let mut kept = book();
    kept.direction = ReadingDirection::Ltr;
    service
        .save(destination.clone(), PageDomain::Document, kept.clone())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    service.drain_changed_scope_keys();
    service
        .rename_book_root(&old.logical_root, &destination.logical_root)
        .unwrap();
    assert_eq!(
        prepare(&service, old.clone(), PageDomain::Document).status,
        Ok(None)
    );
    assert_eq!(
        prepare(&service, destination.clone(), PageDomain::Document)
            .status
            .unwrap()
            .unwrap()
            .composition
            .direction,
        kept.direction
    );
    assert_eq!(service.drain_changed_scope_keys(), vec![old.storage_key()]);
    service
        .purge_book_root(Path::new("C:/books/new.pdf/page.png"))
        .unwrap();
    assert!(
        prepare(&service, destination.clone(), PageDomain::Document)
            .status
            .unwrap()
            .is_some()
    );
    assert!(service.drain_changed_scope_keys().is_empty());
    service.purge_book_root(&destination.logical_root).unwrap();
    assert_eq!(
        prepare(&service, destination.clone(), PageDomain::Document).status,
        Ok(None)
    );
    assert_eq!(
        service.drain_changed_scope_keys(),
        vec![destination.storage_key()]
    );
    owner.shutdown_and_join();
}

#[test]
fn page_migration_is_simultaneous_updates_cover_hidden_and_keeps_outside_move_missing() {
    let (temp, mut connection) = database();
    let key = "c:/books/folder";
    let page = |name: &str| CompositionPageRef::RelativePath { path: name.into() };
    let mut composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            CompositionUnit::Single {
                page: page("A.JPG"),
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            },
            CompositionUnit::Single {
                page: page("B.JPG"),
                placement: SinglePlacement::Leading,
                companion: SingleCompanion::FrontCoverSupplement {
                    page: page("A.JPG"),
                },
            },
        ],
    );
    composition.cover = Some(page("A.JPG"));
    composition.hidden_pages = vec![page("A.JPG")];
    set(&mut connection, key, &PageDomain::ImageFolder, &composition);
    drop(connection);
    assert_eq!(
        migrate_page_paths_at(
            temp.path(),
            &[
                (
                    "C:/Books/Folder/A.JPG".into(),
                    "C:/Books/Folder/B.JPG".into()
                ),
                (
                    "C:/Books/Folder/B.JPG".into(),
                    "C:/Books/Folder/A.JPG".into()
                )
            ]
        )
        .unwrap(),
        1
    );
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    let row = load_rows(&connection).unwrap().remove(key).unwrap();
    assert_eq!(row.composition.cover, Some(page("B.JPG")));
    assert_eq!(row.composition.hidden_pages, vec![page("B.JPG")]);
    assert_eq!(
        row.composition.units[0].pages().next(),
        Some(&page("B.JPG"))
    );
    assert!(
        matches!(&row.composition.units[1],CompositionUnit::Single{page:actual,companion:SingleCompanion::FrontCoverSupplement{page:cover},..} if *actual==page("A.JPG") && *cover==page("B.JPG"))
    );
    drop(connection);
    assert_eq!(
        migrate_page_paths_at(
            temp.path(),
            &[(
                "C:/Books/Folder/B.JPG".into(),
                "C:/Books/Other/Moved.JPG".into()
            )]
        )
        .unwrap(),
        0
    );
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert_eq!(load_rows(&connection).unwrap()[key], row);
}

#[test]
fn composition_only_save_observes_captured_pdf_epub_zip_and_converted_outer_roots() {
    let temp = tempfile::tempdir().unwrap();
    let recorder = crate::content_identity::ContentIdentityRecorder::spawn_at_for_test(
        temp.path().join("content_identity.db"),
    )
    .unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().into(), Some(recorder.producer())).unwrap();
    let mut expected = Vec::new();
    for extension in ["pdf", "epub", "zip", "rar"] {
        let root = temp.path().join(format!("book.{extension}"));
        std::fs::write(&root, format!("disposable {extension} source")).unwrap();
        let (scope, domain, composition) = if matches!(extension, "zip" | "rar") {
            (
                BookScopeKey::new(&root, "chapter").unwrap(),
                PageDomain::ArchiveLevel {
                    prefix: "chapter".into(),
                },
                BookComposition::new(
                    ReadingDirection::Rtl,
                    vec![CompositionUnit::Single {
                        page: CompositionPageRef::ArchiveEntry {
                            entry_name: "chapter/Page.JPG".into(),
                        },
                        placement: SinglePlacement::Center,
                        companion: SingleCompanion::Vacant,
                    }],
                ),
            )
        } else {
            (
                BookScopeKey::document(&root).unwrap(),
                PageDomain::Document,
                book(),
            )
        };
        expected.push(crate::path_key::normalize_keep_drive(&root));
        assert!(
            service
                .save(scope, domain, composition)
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .changed
        );
    }
    // A folder named .zip still cannot generate a container ledger observation.
    let folder = temp.path().join("folder.zip");
    std::fs::create_dir(&folder).unwrap();
    let folder_scope = BookScopeKey::folder(&folder).unwrap();
    assert!(
        service
            .save(
                folder_scope,
                PageDomain::ImageFolder,
                BookComposition::new(ReadingDirection::Ltr, Vec::new())
            )
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .changed
    );
    // Unrelated mount/read after submission cannot retarget the captured root.
    prepare(&service, scope(), PageDomain::Document);
    owner.shutdown_and_join();
    // Each committed recorder update is the explicit observation completion signal.
    let mut updates: Vec<_> = (0..expected.len())
        .map(|_| {
            recorder
                .recv_update_for_test(Duration::from_secs(30))
                .unwrap()
        })
        .collect();
    updates.sort_by(|left, right| left.file_key.cmp(&right.file_key));
    expected.sort();
    assert_eq!(
        updates
            .iter()
            .map(|entry| entry.file_key.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        updates
            .iter()
            .all(|entry| entry.last_edit_at > 0 && entry.has_restorable_content)
    );
}

fn folder_book() -> BookComposition {
    BookComposition::new(
        ReadingDirection::Rtl,
        vec![CompositionUnit::Single {
            page: CompositionPageRef::RelativePath {
                path: "page.jpg".into(),
            },
            placement: SinglePlacement::Center,
            companion: SingleCompanion::Vacant,
        }],
    )
}

fn write_folder_backup(folder: &Path, composition: &BookComposition, tag: &str) {
    std::fs::write(
        folder.join(crate::sidecar::SIDECAR_FILENAME),
        serde_json::to_vec(&serde_json::json!({
            "version": 2, "items": {"page.jpg": {"tags": [tag]}}, "folder_composition": composition,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn restore_folder_backup(service: &BookCompositionService, folder: &Path) -> CompositionWriteReply {
    let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
        crate::sidecar::SidecarFile::load_for_import(folder)
    else {
        panic!("strict loaded backup expected")
    };
    let (sidecar, source) = loaded.into_parts();
    service
        .restore_folder(BookScopeKey::folder(folder).unwrap(), sidecar, source)
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
}

#[test]
fn folder_off_removal_consumes_root_across_tag_rewrites_restart_and_copy() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    let scope = BookScopeKey::folder(&folder).unwrap();
    let composition = folder_book();
    write_folder_backup(&folder, &composition, "old tag");
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
    prepare(&service, scope.clone(), PageDomain::ImageFolder);
    assert!(restore_folder_backup(&service, &folder).changed);
    let original_marker = service
        .receipt(scope.clone())
        .recv()
        .unwrap()
        .unwrap()
        .unwrap()
        .disk_sync_marker;
    assert!(
        service
            .remove_folder(scope.clone(), false)
            .recv()
            .unwrap()
            .status
            .is_ok()
    );
    assert!(!restore_folder_backup(&service, &folder).changed);
    // A different family rewrites the whole disk token, preserving the consumed root.
    write_folder_backup(&folder, &composition, "new tag");
    assert!(!restore_folder_backup(&service, &folder).changed);
    let new_receipt = service
        .receipt(scope.clone())
        .recv()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_ne!(new_receipt.disk_sync_marker, original_marker);
    assert_eq!(
        prepare(&service, scope.clone(), PageDomain::ImageFolder).status,
        Ok(None)
    );
    owner.shutdown_and_join();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
    prepare(&service, scope.clone(), PageDomain::ImageFolder);
    assert!(!restore_folder_backup(&service, &folder).changed);
    let copied = temp.path().join("copied");
    std::fs::create_dir(&copied).unwrap();
    std::fs::copy(
        folder.join(crate::sidecar::SIDECAR_FILENAME),
        copied.join(crate::sidecar::SIDECAR_FILENAME),
    )
    .unwrap();
    assert!(restore_folder_backup(&service, &copied).changed);
    let mut changed_root = composition;
    changed_root.direction = ReadingDirection::Ltr;
    write_folder_backup(&folder, &changed_root, "new tag");
    assert!(restore_folder_backup(&service, &folder).changed);
    owner.shutdown_and_join();
}

#[test]
fn folder_restore_transaction_rechecks_consumption_and_preserves_existing_record() {
    let (temp, mut connection) = database();
    let folder = temp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    let key = BookScopeKey::folder(&folder).unwrap().storage_key();
    write_folder_backup(&folder, &folder_book(), "tag");
    let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
        crate::sidecar::SidecarFile::load_for_import(&folder)
    else {
        panic!()
    };
    let (sidecar, source) = loaded.into_parts();
    // Prepare preceded a later explicit remove; commit must use the current receipt.
    let digest = sidecar.folder_composition_digest();
    consume_folder_root_in(&connection, "book_compositions", &key, digest, Some(123)).unwrap();
    let tx = connection.transaction().unwrap();
    assert!(
        !super::folder_sidecar::restore_in(&tx, "book_compositions", &key, &sidecar, &source)
            .unwrap()
    );
    tx.commit().unwrap();
    assert!(load_rows(&connection).unwrap().is_empty());
    let mut local = folder_book();
    local.direction = ReadingDirection::Ltr;
    set(&mut connection, &key, &PageDomain::ImageFolder, &local);
    let before = load_rows(&connection).unwrap();
    let tx = connection.transaction().unwrap();
    assert!(
        !super::folder_sidecar::restore_in(&tx, "book_compositions", &key, &sidecar, &source)
            .unwrap()
    );
    tx.commit().unwrap();
    assert_eq!(load_rows(&connection).unwrap(), before);
}

#[test]
fn folder_pending_receipt_never_persists_process_sequence_and_rollback_is_atomic() {
    let (_temp, mut connection) = database();
    let key = "c:/books/folder";
    consume_folder_root_in(&connection, "book_compositions", key, [1; 32], Some(41)).unwrap();
    let tx = connection.transaction().unwrap();
    consume_folder_root_in(&tx, "book_compositions", key, [2; 32], None).unwrap();
    replace_row_in(
        &tx,
        "book_compositions",
        key,
        &PageDomain::ImageFolder,
        &folder_book(),
    )
    .unwrap();
    drop(tx);
    assert!(load_rows(&connection).unwrap().is_empty());
    assert_eq!(
        folder_receipt_in(&connection, "book_compositions", key)
            .unwrap()
            .unwrap(),
        FolderReceipt {
            disk_sync_marker: Some(41),
            consumed_root_digest: [1; 32]
        }
    );
    consume_folder_root_in(&connection, "book_compositions", key, [2; 32], None).unwrap();
    assert_eq!(
        folder_receipt_in(&connection, "book_compositions", key)
            .unwrap()
            .unwrap()
            .disk_sync_marker,
        Some(41)
    );
    let fresh = "c:/books/new";
    consume_folder_root_in(&connection, "book_compositions", fresh, [3; 32], None).unwrap();
    assert_eq!(
        folder_receipt_in(&connection, "book_compositions", fresh)
            .unwrap()
            .unwrap()
            .disk_sync_marker,
        None
    );
    purge_book_root(&mut connection, "c:/books").unwrap();
    assert!(
        folder_receipt_in(&connection, "book_compositions", key)
            .unwrap()
            .is_none()
    );
}

#[test]
fn folder_corrupt_source_prevents_off_removal_and_preserves_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    let scope = BookScopeKey::folder(&folder).unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
    prepare(&service, scope.clone(), PageDomain::ImageFolder);
    assert!(
        service
            .save_folder(scope.clone(), folder_book(), false)
            .recv()
            .unwrap()
            .changed
    );
    let path = folder.join(crate::sidecar::SIDECAR_FILENAME);
    std::fs::write(&path, b"broken backup").unwrap();
    assert!(
        service
            .remove_folder(scope.clone(), false)
            .recv()
            .unwrap()
            .status
            .is_err()
    );
    assert!(
        prepare(&service, scope, PageDomain::ImageFolder)
            .status
            .unwrap()
            .is_some()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"broken backup");
    owner.shutdown_and_join();
}

#[test]
fn folder_receipt_rename_requires_destination_disk_witness_and_copy_does_not_transfer() {
    let (temp, mut connection) = database();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&target).unwrap();
    let source_key = BookScopeKey::folder(&source).unwrap().storage_key();
    let target_key = BookScopeKey::folder(&target).unwrap().storage_key();
    let composition = folder_book();
    write_folder_backup(&source, &composition, "tag");
    let source_snapshot = super::folder_sidecar::prepare_source(&source).unwrap();
    source_snapshot
        .consume_in(&connection, "book_compositions", &source_key)
        .unwrap();
    let mut other = composition.clone();
    other.direction = ReadingDirection::Ltr;
    write_folder_backup(&target, &other, "tag");
    rename_book_root(&mut connection, &source_key, &target_key).unwrap();
    assert!(
        folder_receipt_in(&connection, "book_compositions", &source_key)
            .unwrap()
            .is_some()
    );
    assert!(
        folder_receipt_in(&connection, "book_compositions", &target_key)
            .unwrap()
            .is_none()
    );
    std::fs::copy(
        source.join(crate::sidecar::SIDECAR_FILENAME),
        target.join(crate::sidecar::SIDECAR_FILENAME),
    )
    .unwrap();
    rename_book_root(&mut connection, &source_key, &target_key).unwrap();
    assert!(
        folder_receipt_in(&connection, "book_compositions", &source_key)
            .unwrap()
            .is_none()
    );
    let receipt = folder_receipt_in(&connection, "book_compositions", &target_key)
        .unwrap()
        .unwrap();
    let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
        crate::sidecar::SidecarFile::load_for_import(&target)
    else {
        panic!()
    };
    let crate::sidecar::SidecarImportSource::Disk(token) = loaded.source() else {
        panic!()
    };
    assert_eq!(receipt.disk_sync_marker, Some(token.sync_marker()));
    assert!(load_rows(&connection).unwrap().is_empty());
}

#[test]
fn folder_backup_only_failure_keeps_committed_removal_and_consumes_retained_backup() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("book");
    let data = temp.path().join("data");
    std::fs::create_dir(&folder).unwrap();
    let scope = BookScopeKey::folder(&folder).unwrap();
    write_folder_backup(&folder, &folder_book(), "preserved");
    let (service, owner) = BookCompositionService::spawn_at(data, None).unwrap();
    prepare(&service, scope.clone(), PageDomain::ImageFolder);
    assert!(restore_folder_backup(&service, &folder).changed);
    service.drain_changed_scope_keys();
    let receipt = service
        .receipt(scope.clone())
        .recv()
        .unwrap()
        .unwrap()
        .unwrap();
    let sidecar_path = folder.join(crate::sidecar::SIDECAR_FILENAME);
    let retained_path = folder.join("retained-backup.dat");
    let original = std::fs::read(&sidecar_path).unwrap();
    let fault_path = sidecar_path.clone();
    let retained_for_fault = retained_path.clone();
    let result = service
        .remove_folder_with_before_mirror_for_test(scope.clone(), move || {
            // This executes after DB commit. A directory at the file path makes the
            // mirror source write-disabled while preserving the exact old bytes.
            std::fs::rename(&fault_path, retained_for_fault).unwrap();
            std::fs::create_dir(&fault_path).unwrap();
        })
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(result.changed && result.status.is_ok());
    assert_eq!(
        prepare(&service, scope.clone(), PageDomain::ImageFolder).status,
        Ok(None)
    );
    let warnings = service.drain_errors();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("フォルダのバックアップを更新できませんでした"));
    assert_eq!(
        service
            .receipt(scope.clone())
            .recv()
            .unwrap()
            .unwrap()
            .unwrap(),
        receipt
    );
    assert_eq!(std::fs::read(&retained_path).unwrap(), original);
    // Querying and idle processing did not retry or roll back the backup failure.
    assert!(sidecar_path.is_dir());
    std::fs::remove_dir(&sidecar_path).unwrap();
    std::fs::rename(retained_path, &sidecar_path).unwrap();
    assert!(!restore_folder_backup(&service, &folder).changed);
    assert_eq!(
        prepare(&service, scope, PageDomain::ImageFolder).status,
        Ok(None)
    );
    assert_eq!(std::fs::read(sidecar_path).unwrap(), original);
    owner.shutdown_and_join();
}

#[test]
fn folder_receipt_only_generic_purge_preserves_parent_on_page_delete_and_neighbor() {
    let (temp, connection) = database();
    let folder = temp.path().join("book");
    let sibling = temp.path().join("book-keep");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    let key = BookScopeKey::folder(&folder).unwrap().storage_key();
    let sibling_key = BookScopeKey::folder(&sibling).unwrap().storage_key();
    consume_folder_root_in(&connection, "book_compositions", &key, [1; 32], Some(21)).unwrap();
    consume_folder_root_in(
        &connection,
        "book_compositions",
        &sibling_key,
        [2; 32],
        Some(22),
    )
    .unwrap();
    assert!(load_rows(&connection).unwrap().is_empty());
    drop(connection);
    let page = folder.join("page.jpg");
    std::fs::write(&page, b"disposable page").unwrap();
    std::fs::remove_file(&page).unwrap();
    let report = crate::rename_key_migration::purge_removed_paths_at(temp.path(), &[page], &[]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert!(
        folder_receipt_in(&connection, "book_compositions", &key)
            .unwrap()
            .is_some()
    );
    drop(connection);
    std::fs::remove_dir(&folder).unwrap();
    let report = crate::rename_key_migration::purge_removed_scopes_at(
        temp.path(),
        &[crate::delete_worker::DeleteSourceScope::Tree(folder)],
        &[],
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert!(
        folder_receipt_in(&connection, "book_compositions", &key)
            .unwrap()
            .is_none()
    );
    assert!(
        folder_receipt_in(&connection, "book_compositions", &sibling_key)
            .unwrap()
            .is_some()
    );
}

#[test]
fn folder_auto_restore_preserves_unknown_and_corrupt_existing_database_bytes() {
    for corruption in ["row_schema", "row_json", "database_version"] {
        let (temp, mut connection) = database();
        let folder = temp.path().join("book");
        std::fs::create_dir(&folder).unwrap();
        let scope = BookScopeKey::folder(&folder).unwrap();
        set(
            &mut connection,
            &scope.storage_key(),
            &PageDomain::ImageFolder,
            &folder_book(),
        );
        match corruption {
            "row_schema" => connection
                .execute_batch("UPDATE book_compositions SET schema_version=999")
                .unwrap(),
            "row_json" => connection
                .execute_batch("UPDATE book_compositions SET composition_json='broken'")
                .unwrap(),
            _ => connection.execute_batch("PRAGMA user_version=999").unwrap(),
        }
        drop(connection);
        write_folder_backup(&folder, &folder_book(), "retained");
        let db_path = temp.path().join(DB_FILE);
        let original = std::fs::read(&db_path).unwrap();
        let backup_path = folder.join(crate::sidecar::SIDECAR_FILENAME);
        let original_backup = std::fs::read(&backup_path).unwrap();
        let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
        assert!(
            prepare(&service, scope, PageDomain::ImageFolder)
                .status
                .is_err()
        );
        let restored = restore_folder_backup(&service, &folder);
        assert!(!restored.changed && restored.status.is_err());
        assert!(service.snapshot().status.is_err());
        owner.shutdown_and_join();
        assert_eq!(std::fs::read(db_path).unwrap(), original);
        assert_eq!(std::fs::read(backup_path).unwrap(), original_backup);
    }
}

fn assert_folder_page_filesystem_migration(backup_enabled: bool) {
    let (temp, mut connection) = database();
    let folder = temp.path().join("book");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let old_page = folder.join("A.JPG");
    let renamed_page = folder.join("Renamed.JPG");
    let other_page = folder.join("B.JPG");
    std::fs::write(&old_page, b"disposable A page").unwrap();
    std::fs::write(&other_page, b"disposable B page").unwrap();
    let key = BookScopeKey::folder(&folder).unwrap().storage_key();
    let page = |name: &str| CompositionPageRef::RelativePath { path: name.into() };
    let mut composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            CompositionUnit::Single {
                page: page("A.JPG"),
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            },
            CompositionUnit::Single {
                page: page("B.JPG"),
                placement: SinglePlacement::Leading,
                companion: SingleCompanion::FrontCoverSupplement {
                    page: page("A.JPG"),
                },
            },
        ],
    );
    composition.cover = Some(page("A.JPG"));
    composition.hidden_pages = vec![page("A.JPG")];
    set(
        &mut connection,
        &key,
        &PageDomain::ImageFolder,
        &composition,
    );
    let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
    sidecar
        .set_folder_composition(Some(composition.clone()))
        .unwrap();
    sidecar.set_tags("A.JPG", &["preserved"]);
    let adjustment = crate::adjustment::AdjustParams {
        brightness: 12.0,
        ..Default::default()
    };
    sidecar.set_adjust("B.JPG", adjustment.clone());
    assert!(sidecar.flush_blocking());
    let backup_path = folder.join(crate::sidecar::SIDECAR_FILENAME);
    let original_backup = std::fs::read(&backup_path).unwrap();
    let original_digest = sidecar.folder_composition_digest();
    drop(connection);

    // A real same-parent rename must rewrite every saved occurrence once.
    std::fs::rename(&old_page, &renamed_page).unwrap();
    let (changed, warnings) = migrate_page_paths_with_backup_at(
        temp.path(),
        &[(old_page, renamed_page.clone())],
        backup_enabled,
    )
    .unwrap();
    assert_eq!(changed, 1);
    assert!(warnings.is_empty());
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    let renamed_record = row_in(&connection, "book_compositions", &key)
        .unwrap()
        .unwrap();
    assert_eq!(renamed_record.composition.cover, Some(page("Renamed.JPG")));
    assert_eq!(
        renamed_record.composition.hidden_pages,
        [page("Renamed.JPG")]
    );
    assert_eq!(
        renamed_record.composition.units[0].pages().next(),
        Some(&page("Renamed.JPG"))
    );
    assert!(
        matches!(&renamed_record.composition.units[1], CompositionUnit::Single { companion: SingleCompanion::FrontCoverSupplement { page: cover }, .. } if *cover == page("Renamed.JPG"))
    );
    let receipt = folder_receipt_in(&connection, "book_compositions", &key)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.consumed_root_digest, original_digest);
    assert!(receipt.disk_sync_marker.is_some());
    let loaded = crate::sidecar::SidecarFile::load(&folder);
    assert_eq!(
        loaded.items()["A.JPG"].tags.as_ref().unwrap(),
        &["#preserved"]
    );
    assert_eq!(
        loaded.items()["B.JPG"].adjust.as_ref().unwrap(),
        &adjustment
    );
    if backup_enabled {
        let root = loaded.folder_composition().unwrap();
        assert_eq!(
            root, &renamed_record.composition,
            "mirror uses the committed local revision"
        );
        assert_eq!(root.cover, Some(page("Renamed.JPG")));
        assert_eq!(root.hidden_pages, [page("Renamed.JPG")]);
    } else {
        assert_eq!(std::fs::read(&backup_path).unwrap(), original_backup);
        assert_eq!(loaded.folder_composition().unwrap(), &composition);
    }
    drop(connection);

    // An outside move keeps the stored identity; actual folder facts resolve Missing.
    let moved_page = outside.join("Moved.JPG");
    std::fs::rename(&other_page, &moved_page).unwrap();
    let (changed, warnings) =
        migrate_page_paths_with_backup_at(temp.path(), &[(other_page, moved_page)], backup_enabled)
            .unwrap();
    assert_eq!(changed, 0);
    assert!(warnings.is_empty());
    let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert_eq!(
        row_in(&connection, "book_compositions", &key)
            .unwrap()
            .unwrap(),
        renamed_record
    );
    let manifest = ["Renamed.JPG", "B.JPG"]
        .into_iter()
        .filter(|name| folder.join(name).is_file())
        .enumerate()
        .map(|(item_index, name)| ManifestPage {
            page: page(name),
            availability: ManifestAvailability::Present { item_index },
        })
        .collect::<Vec<_>>();
    let projection = resolve_composition(
        &renamed_record.composition,
        &PageDomain::ImageFolder,
        &manifest,
    )
    .unwrap();
    assert_eq!(projection.page_status(&page("B.JPG")), PageStatus::Missing);
    assert_eq!(
        projection.page_status(&page("Renamed.JPG")),
        PageStatus::HiddenByComposition
    );
    drop(connection);

    // Page deletion purges page metadata without erasing the parent composition.
    std::fs::remove_file(&renamed_page).unwrap();
    let report =
        crate::rename_key_migration::purge_removed_paths_at(temp.path(), &[renamed_page], &[]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let mut connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
    assert_eq!(
        row_in(&connection, "book_compositions", &key)
            .unwrap()
            .unwrap(),
        renamed_record
    );
    let projection =
        resolve_composition(&renamed_record.composition, &PageDomain::ImageFolder, &[]).unwrap();
    assert_eq!(
        projection.page_status(&page("Renamed.JPG")),
        PageStatus::Missing
    );
    if !backup_enabled {
        // Prove the rename's consumption itself survives row removal: no later
        // explicit remove consumer is used to create this receipt for the test.
        let transaction = connection.transaction().unwrap();
        assert!(remove_row_in(&transaction, "book_compositions", &key).unwrap());
        transaction.commit().unwrap();
        let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
            crate::sidecar::SidecarFile::load_for_import(&folder)
        else {
            panic!("retained original backup")
        };
        let (sidecar, source) = loaded.into_parts();
        let transaction = connection.transaction().unwrap();
        assert!(
            !super::folder_sidecar::restore_in(
                &transaction,
                "book_compositions",
                &key,
                &sidecar,
                &source
            )
            .unwrap()
        );
        transaction.commit().unwrap();
        assert!(
            row_in(&connection, "book_compositions", &key)
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(backup_path).unwrap(), original_backup);
    }
}

#[test]
fn folder_real_page_rename_backup_on_preserves_items_and_outside_move_is_missing() {
    assert_folder_page_filesystem_migration(true);
}

#[test]
fn folder_real_page_rename_backup_off_consumes_old_root_and_page_delete_keeps_folder() {
    assert_folder_page_filesystem_migration(false);
}
