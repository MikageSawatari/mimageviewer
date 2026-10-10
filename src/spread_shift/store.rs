//! Worker-only SQLite primitives. Callers own the service or maintenance writer lease.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::SpreadShiftPageRef;

const SCHEMA_VERSION: i64 = 1;
const DB_FILE: &str = "spread_shift.db";

fn validate_storage_key(key: &str) -> Result<(), String> {
    let scope = crate::book_scope::BookScopeKey::new(key, "")?;
    if !Path::new(key).is_absolute() || scope.storage_key() != key || key.contains("::") {
        return Err("invalid spread shift scope key".into());
    }
    Ok(())
}

/// Create only a missing database. Existing incompatible or damaged data stays intact.
pub(crate) fn initialize_database_at(path: &Path) -> Result<(), String> {
    if path.try_exists().map_err(|error| error.to_string())? {
        return open_existing(path).map(|_| ());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "CREATE TABLE spread_shifts (
            path TEXT PRIMARY KEY,
            anchor_ref TEXT NOT NULL
         );
         PRAGMA user_version = 1;",
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

pub(crate) fn validate_database_schema(connection: &Connection) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported spread shift schema version: {version}"
        ));
    }
    let mut statement = connection
        .prepare("PRAGMA table_info(spread_shifts)")
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if columns.len() != 2
        || columns[0].0 != "path"
        || !columns[0].1.eq_ignore_ascii_case("TEXT")
        || columns[0].3 != 1
        || columns[1].0 != "anchor_ref"
        || !columns[1].1.eq_ignore_ascii_case("TEXT")
        || !columns[1].2
        || columns[1].3 != 0
    {
        return Err("invalid spread shift table schema".into());
    }
    Ok(())
}

pub(crate) fn load_rows(
    connection: &Connection,
) -> Result<BTreeMap<String, SpreadShiftPageRef>, String> {
    validate_database_schema(connection)?;
    let mut statement = connection
        .prepare("SELECT path, anchor_ref FROM spread_shifts ORDER BY path")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (key, wire) = row.map_err(|error| error.to_string())?;
        validate_storage_key(&key)?;
        let reference: SpreadShiftPageRef = serde_json::from_str(&wire)
            .map_err(|error| format!("invalid spread shift page reference: {error}"))?;
        reference.validate()?;
        result.insert(key, reference);
    }
    Ok(result)
}

pub(crate) fn open_existing(path: &Path) -> Result<Option<Connection>, String> {
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| error.to_string())?;
    validate_database_schema(&connection)?;
    // Invalid JSON is an unreadable store, never an empty saved state.
    load_rows(&connection)?;
    Ok(Some(connection))
}

pub(crate) fn set(
    connection: &mut Connection,
    key: &str,
    reference: &SpreadShiftPageRef,
) -> Result<bool, String> {
    validate_storage_key(key)?;
    reference.validate()?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    validate_database_schema(&transaction)?;
    let existing: Option<String> = transaction
        .query_row(
            "SELECT anchor_ref FROM spread_shifts WHERE path = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(wire) = existing {
        let old: SpreadShiftPageRef =
            serde_json::from_str(&wire).map_err(|error| error.to_string())?;
        old.validate()?;
        if &old == reference {
            return Ok(false);
        }
    }
    let wire = serde_json::to_string(reference).map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO spread_shifts(path, anchor_ref) VALUES (?1, ?2)
         ON CONFLICT(path) DO UPDATE SET anchor_ref = excluded.anchor_ref",
            rusqlite::params![key, wire],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(true)
}

pub(crate) fn clear(connection: &mut Connection, key: &str) -> Result<bool, String> {
    validate_storage_key(key)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    validate_database_schema(&transaction)?;
    // Do not overwrite/delete a malformed existing reference as if it were saved state.
    if let Some(wire) = transaction
        .query_row(
            "SELECT anchor_ref FROM spread_shifts WHERE path = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
    {
        let reference: SpreadShiftPageRef =
            serde_json::from_str(&wire).map_err(|error| error.to_string())?;
        reference.validate()?;
    }
    let changed = transaction
        .execute("DELETE FROM spread_shifts WHERE path = ?1", [key])
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

/// Explicit metadata deletion, separate from settings reset or history/cache clearing.
pub(crate) fn clear_all(connection: &mut Connection) -> Result<usize, String> {
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    load_rows(&transaction)?;
    let changed = transaction
        .execute("DELETE FROM spread_shifts", [])
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}

/// Completed book-folder operations retain payload spelling and affect only that root family.
/// The caller is the single shift worker, with its existing writable connection.
pub(crate) fn rename_book_root(
    connection: &mut Connection,
    source: &str,
    target: &str,
) -> Result<usize, String> {
    validate_storage_key(source)?;
    validate_storage_key(target)?;
    if source == target {
        return Ok(0);
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let rows = load_rows(&transaction)?;
    let prefix = format!("{}/", source.trim_end_matches('/'));
    let mut moved = 0;
    for (key, reference) in rows {
        let destination = if key == source {
            target.to_owned()
        } else if let Some(suffix) = key.strip_prefix(&prefix) {
            format!("{}/{suffix}", target.trim_end_matches('/'))
        } else {
            continue;
        };
        validate_storage_key(&destination)?;
        let wire = serde_json::to_string(&reference).map_err(|error| error.to_string())?;
        // Follow the existing destination-priority rule for persisted metadata.
        transaction
            .execute(
                "INSERT OR IGNORE INTO spread_shifts(path, anchor_ref) VALUES (?1, ?2)",
                rusqlite::params![destination, wire],
            )
            .map_err(|error| error.to_string())?;
        moved += transaction
            .execute("DELETE FROM spread_shifts WHERE path = ?1", [key])
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(moved)
}

/// A deleted book root clears its scopes; deleting a page does not match its parent scope.
pub(crate) fn purge_book_root(connection: &mut Connection, root: &str) -> Result<usize, String> {
    validate_storage_key(root)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let rows = load_rows(&transaction)?;
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let mut removed = 0;
    for key in rows
        .keys()
        .filter(|key| key.as_str() == root || key.starts_with(&prefix))
    {
        removed += transaction
            .execute("DELETE FROM spread_shifts WHERE path = ?1", [key])
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(removed)
}

struct PathMappingIndex {
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

    fn map_relative(&self, scope: &str, relative: &str) -> Option<String> {
        let root = Path::new(scope);
        let original = root.join(relative.replace(['/', '\\'], std::path::MAIN_SEPARATOR_STR));
        let scope_prefix = format!("{}/", scope.trim_end_matches('/'));
        for candidate in original.ancestors() {
            let candidate_key = crate::path_key::normalize_keep_drive(candidate);
            let Some((component_count, destination)) = self.by_from.get(&candidate_key) else {
                continue;
            };
            // Root/ancestor moves are handled by generic key migration. A relative payload
            // does not change, and moving a page to a sibling book leaves the old anchor Missing.
            if !candidate_key.starts_with(&scope_prefix) {
                return None;
            }
            let suffix = original
                .components()
                .skip(*component_count)
                .collect::<PathBuf>();
            let mapped = destination.join(suffix);
            if !crate::path_key::normalize_keep_drive(&mapped).starts_with(&scope_prefix) {
                return None;
            }
            return Some(
                mapped
                    .components()
                    .skip(root.components().count())
                    .collect::<PathBuf>()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
        None
    }
}

/// Apply original-to-final maps simultaneously; no newly updated reference is mapped twice.
pub(crate) fn migrate_page_paths_at(
    data_dir: &Path,
    mappings: &[(PathBuf, PathBuf)],
) -> Result<usize, String> {
    if mappings.is_empty() {
        return Ok(0);
    }
    let path = data_dir.join(DB_FILE);
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(0);
    }
    let mut connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| error.to_string())?;
    let present: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'spread_shifts')",
        [], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    if !present {
        return Ok(0);
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let original_rows = load_rows(&transaction)?;
    let mappings = PathMappingIndex::new(mappings);
    let mut changed = 0;
    for (key, reference) in original_rows {
        let SpreadShiftPageRef::RelativePath(relative) = &reference else {
            continue;
        };
        let Some(mapped) = mappings.map_relative(&key, relative) else {
            continue;
        };
        let replacement = SpreadShiftPageRef::RelativePath(mapped);
        replacement.validate()?;
        if replacement == reference {
            continue;
        }
        let wire = serde_json::to_string(&replacement).map_err(|error| error.to_string())?;
        changed += transaction
            .execute(
                "UPDATE spread_shifts SET anchor_ref = ?1 WHERE path = ?2",
                rusqlite::params![wire, key],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_rename_rejects_oversized_child_scope_without_changing_original_rows() {
        let (_temp, mut connection) = database();
        let parent = format!(
            "c:/books/{}",
            "chapter/".repeat((crate::book_scope::MAX_BOOK_REFERENCE_BYTES - 256) / 8)
        );
        let parent = parent.trim_end_matches('/');
        let source = format!("{parent}/old");
        let target = format!("{parent}/{}", "n".repeat(200));
        let child = format!("{source}/{}", "x".repeat(100));
        validate_storage_key(&target).unwrap();
        let anchor = SpreadShiftPageRef::RelativePath("Page.JPG".into());
        set(&mut connection, &source, &anchor).unwrap();
        set(&mut connection, &child, &anchor).unwrap();
        let original = load_rows(&connection).unwrap();
        assert!(rename_book_root(&mut connection, &source, &target).is_err());
        assert_eq!(load_rows(&connection).unwrap(), original);
    }

    #[test]
    fn completed_book_root_operations_preserve_payload_and_destination_priority() {
        let (_temp, mut connection) = database();
        let source = "c:/books/old%_book";
        let target = "c:/books/new_book";
        let old = SpreadShiftPageRef::RelativePath("NewChapter/Page.JPG".into());
        let kept = SpreadShiftPageRef::RelativePath("Keep.JPG".into());
        for key in [
            source.to_owned(),
            format!("{source}/chapter"),
            format!("{source}-other"),
        ] {
            set(&mut connection, &key, &old).unwrap();
        }
        set(&mut connection, target, &kept).unwrap();
        assert_eq!(
            rename_book_root(&mut connection, source, target).unwrap(),
            2
        );
        let rows = load_rows(&connection).unwrap();
        assert!(!rows.contains_key(source));
        assert!(!rows.contains_key(&format!("{source}/chapter")));
        assert_eq!(rows[target], kept);
        assert_eq!(rows[&format!("{target}/chapter")], old);
        assert_eq!(rows[&format!("{source}-other")], old);
        assert_eq!(
            purge_book_root(
                &mut connection,
                &format!("{target}/Page.JPG").to_lowercase()
            )
            .unwrap(),
            0
        );
        assert_eq!(purge_book_root(&mut connection, target).unwrap(), 2);
        assert_eq!(load_rows(&connection).unwrap().len(), 1);
    }

    fn database() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_FILE);
        initialize_database_at(&path).unwrap();
        let connection = open_existing(&path).unwrap().unwrap();
        (temp, connection)
    }

    #[test]
    fn creation_actual_changes_and_reopen_preserve_references() {
        let (temp, mut connection) = database();
        let key = "c:/books/book.zip/chapter";
        let reference = SpreadShiftPageRef::ArchiveEntry("chapter/inner.zip/pages/001.jpg".into());
        assert!(set(&mut connection, key, &reference).unwrap());
        assert!(!set(&mut connection, key, &reference).unwrap());
        drop(connection);
        initialize_database_at(&temp.path().join(DB_FILE)).unwrap();
        let mut connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
        assert_eq!(load_rows(&connection).unwrap().get(key), Some(&reference));
        assert!(clear(&mut connection, key).unwrap());
        assert!(!clear(&mut connection, key).unwrap());
    }

    #[test]
    fn explicit_clear_all_counts_committed_rows_and_rejects_invalid_original() {
        let (_temp, mut connection) = database();
        for key in ["c:/books/a.zip", "c:/books/a.zip/chapter"] {
            set(
                &mut connection,
                key,
                &SpreadShiftPageRef::ArchiveEntry("chapter/page.jpg".into()),
            )
            .unwrap();
        }
        assert_eq!(clear_all(&mut connection).unwrap(), 2);
        assert_eq!(clear_all(&mut connection).unwrap(), 0);
        connection
            .execute(
                "INSERT INTO spread_shifts VALUES ('c:/books/a.zip', 'not-json')",
                [],
            )
            .unwrap();
        assert!(clear_all(&mut connection).is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM spread_shifts", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn absent_database_is_not_created_by_reads_or_path_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(DB_FILE);
        assert!(open_existing(&path).unwrap().is_none());
        assert_eq!(
            migrate_page_paths_at(temp.path(), &[("c:/a.jpg".into(), "c:/b.jpg".into())]).unwrap(),
            0
        );
        assert!(!path.exists());
    }

    #[test]
    fn incompatible_schema_and_invalid_json_are_preserved() {
        for schema in [
            "CREATE TABLE spread_shifts(path TEXT PRIMARY KEY, anchor_ref TEXT NOT NULL); PRAGMA user_version=2;",
            "CREATE TABLE spread_shifts(path TEXT PRIMARY KEY, wrong TEXT NOT NULL); PRAGMA user_version=1;",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(DB_FILE);
            Connection::open(&path)
                .unwrap()
                .execute_batch(schema)
                .unwrap();
            let before = std::fs::read(&path).unwrap();
            assert!(initialize_database_at(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let (temp, mut connection) = database();
        connection
            .execute(
                "INSERT INTO spread_shifts VALUES ('c:/books/book', 'not-json')",
                [],
            )
            .unwrap();
        assert!(load_rows(&connection).is_err());
        assert!(
            set(
                &mut connection,
                "c:/books/book",
                &SpreadShiftPageRef::RelativePath("001.jpg".into())
            )
            .is_err()
        );
        assert!(clear(&mut connection, "c:/books/book").is_err());
        drop(connection);
        assert!(open_existing(&temp.path().join(DB_FILE)).is_err());
        let connection = Connection::open(temp.path().join(DB_FILE)).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT anchor_ref FROM spread_shifts", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "not-json"
        );
    }

    #[test]
    fn folder_reorder_maps_once_and_outside_move_leaves_missing_anchor() {
        let (temp, mut connection) = database();
        let key = "c:/books/compiled";
        set(
            &mut connection,
            key,
            &SpreadShiftPageRef::RelativePath("001.jpg".into()),
        )
        .unwrap();
        drop(connection);
        assert_eq!(
            migrate_page_paths_at(
                temp.path(),
                &[
                    (
                        "C:/books/compiled/001.jpg".into(),
                        "C:/books/compiled/002.jpg".into()
                    ),
                    (
                        "C:/books/compiled/002.jpg".into(),
                        "C:/books/compiled/001.jpg".into()
                    ),
                ]
            )
            .unwrap(),
            1
        );
        assert_eq!(
            migrate_page_paths_at(
                temp.path(),
                &[(
                    "C:/books/compiled/002.jpg".into(),
                    "C:/books/other/002.jpg".into()
                ),]
            )
            .unwrap(),
            0
        );
        let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
        assert_eq!(
            load_rows(&connection).unwrap()[key],
            SpreadShiftPageRef::RelativePath("002.jpg".into())
        );
    }

    #[test]
    fn root_moves_leave_payload_and_inner_directory_moves_preserve_case_and_slashes() {
        let (temp, mut connection) = database();
        let key = "c:/books/folder";
        set(
            &mut connection,
            key,
            &SpreadShiftPageRef::RelativePath("Chapter\\Page.JPG".into()),
        )
        .unwrap();
        drop(connection);
        assert_eq!(
            migrate_page_paths_at(
                temp.path(),
                &[("C:/books/folder".into(), "D:/books/folder".into()),]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            migrate_page_paths_at(
                temp.path(),
                &[(
                    "C:/BOOKS/FOLDER/chapter".into(),
                    "C:/books/folder/NewChapter".into()
                ),]
            )
            .unwrap(),
            1
        );
        let connection = open_existing(&temp.path().join(DB_FILE)).unwrap().unwrap();
        assert_eq!(
            load_rows(&connection).unwrap()[key],
            SpreadShiftPageRef::RelativePath("NewChapter/Page.JPG".into())
        );
    }
}
