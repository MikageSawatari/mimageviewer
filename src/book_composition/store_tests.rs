#![cfg(test)]

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
    invalid.page_order.push(invalid.page_order[0].clone());
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
        vec![CompositionPageRef::ArchiveEntry {
            entry_name: "Chapter/Page.JPG".into(),
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
            .page_order,
        composition.page_order
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
        vec![CompositionPageRef::ArchiveEntry {
            entry_name: "Chapter\\Sub\\Page.JPG".into(),
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
    assert_eq!(record.composition.page_order, composition.page_order);
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
        wire["composition"]["page_order"][0]["entry_name"],
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
        vec![CompositionPageRef::ArchiveEntry {
            entry_name: "Chapter/Sub/Page.JPG".into(),
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
    assert_eq!(record.composition.page_order, composition.page_order);
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
    connection.execute_batch("PRAGMA user_version=999").unwrap();
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
fn page_migration_is_simultaneous_updates_order_hidden_overrides_and_keeps_outside_move_missing() {
    let (temp, mut connection) = database();
    let key = "c:/books/folder";
    let page = |name: &str| CompositionPageRef::RelativePath { path: name.into() };
    let mut composition =
        BookComposition::new(ReadingDirection::Rtl, vec![page("A.JPG"), page("B.JPG")]);
    composition.cover_enabled = true;
    composition.hidden_pages = vec![page("A.JPG")];
    composition.page_overrides = vec![PageOverrideEntry {
        page: page("B.JPG"),
        directive: PageOverride::BlankOpposite {
            page_side: PageSide::Trailing,
        },
    }];
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
    assert!(row.composition.cover_enabled);
    assert_eq!(row.composition.hidden_pages, vec![page("B.JPG")]);
    assert_eq!(
        row.composition.page_order,
        vec![page("B.JPG"), page("A.JPG")]
    );
    assert_eq!(
        row.composition.page_override(&page("A.JPG")),
        Some(PageOverride::BlankOpposite {
            page_side: PageSide::Trailing
        })
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
                    vec![CompositionPageRef::ArchiveEntry {
                        entry_name: "chapter/Page.JPG".into(),
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

#[test]
fn old_unpublished_schema_is_preserved_and_service_rejects_writes_without_migration() {
    let (temp, mut connection) = database();
    let key = scope().storage_key();
    set(&mut connection, &key, &PageDomain::Document, &book());
    connection.execute_batch("PRAGMA user_version=1").unwrap();
    drop(connection);
    let before = std::fs::read(temp.path().join(DB_FILE)).unwrap();
    assert!(initialize_database_at(&temp.path().join(DB_FILE)).is_err());
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    assert!(
        prepare(&service, scope(), PageDomain::Document)
            .status
            .is_err()
    );
    let reply = service
        .save(scope(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(reply.status.is_err());
    assert!(!reply.changed);
    assert!(
        service
            .remove(scope(), PageDomain::Document)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_err()
    );
    owner.shutdown_and_join();
    assert_eq!(std::fs::read(temp.path().join(DB_FILE)).unwrap(), before);
}

#[test]
fn root_rename_source_priority_replaces_destination_family_and_absent_source_clears_it() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let old = scope();
    let destination = BookScopeKey::document("C:/books/new.pdf").unwrap();
    service
        .save(old.clone(), PageDomain::Document, book())
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let mut stale = book();
    stale.direction = ReadingDirection::Ltr;
    service
        .save(destination.clone(), PageDomain::Document, stale)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    service
        .rename_book_root_with_collision(
            &old.logical_root,
            &destination.logical_root,
            crate::rename_key_migration::PathMigrationCollision::SourceWins,
        )
        .unwrap();
    assert_eq!(
        prepare(&service, destination.clone(), PageDomain::Document)
            .status
            .unwrap()
            .unwrap()
            .composition
            .direction,
        book().direction
    );
    service
        .rename_book_root_with_collision(
            &old.logical_root,
            &destination.logical_root,
            crate::rename_key_migration::PathMigrationCollision::SourceWins,
        )
        .unwrap();
    assert!(
        prepare(&service, destination, PageDomain::Document)
            .status
            .unwrap()
            .is_none()
    );
    owner.shutdown_and_join();
}

#[test]
fn composition_busy_includes_admitted_mutations_until_their_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let reply = service.save(scope(), PageDomain::Document, book());
    assert!(service.is_busy());
    release.send(()).unwrap();
    assert!(
        reply
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .is_ok()
    );
    assert!(!service.is_busy());
    owner.shutdown_and_join();
}
