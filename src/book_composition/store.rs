//! SQLite primitives used only by the composition writer or its maintenance lease.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{BookComposition, CompositionPageRef, PageDomain};
use crate::book_scope::BookScopeKey;

pub(crate) const DB_FILE: &str = "book_composition.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredComposition {
    pub(crate) domain: PageDomain,
    pub(crate) composition: BookComposition,
}

// The validation domain is persisted because an empty record has no page from
// which its kind or outer archive prefix can be recovered. It is never a key.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredDomain {
    ImageFolder,
    ArchiveLevel { prefix: String },
    Document,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowWire {
    domain: StoredDomain,
    composition: BookComposition,
}

impl From<&PageDomain> for StoredDomain {
    fn from(domain: &PageDomain) -> Self {
        match domain {
            PageDomain::ImageFolder => Self::ImageFolder,
            PageDomain::ArchiveLevel { prefix } => Self::ArchiveLevel {
                prefix: prefix.clone(),
            },
            PageDomain::Document => Self::Document,
        }
    }
}

impl From<StoredDomain> for PageDomain {
    fn from(domain: StoredDomain) -> Self {
        match domain {
            StoredDomain::ImageFolder => Self::ImageFolder,
            StoredDomain::ArchiveLevel { prefix } => Self::ArchiveLevel { prefix },
            StoredDomain::Document => Self::Document,
        }
    }
}

/// Persistence/request domains use the same prefix identity as BookScopeKey.
/// Archive member spelling in the composition remains an untouched byte-reader reference.
pub(crate) fn canonical_domain(domain: &PageDomain) -> Result<PageDomain, String> {
    domain.validate().map_err(|error| error.to_string())?;
    Ok(match domain {
        PageDomain::ArchiveLevel { prefix } => PageDomain::ArchiveLevel {
            prefix: crate::book_scope::normalize_level_prefix(prefix)?,
        },
        _ => domain.clone(),
    })
}

pub(crate) fn validate_scope_domain(
    scope: &BookScopeKey,
    domain: &PageDomain,
) -> Result<(), String> {
    scope.validate()?;
    domain.validate().map_err(|error| error.to_string())?;
    match domain {
        PageDomain::ArchiveLevel { prefix } => {
            if crate::book_scope::normalize_level_prefix(prefix)? != scope.level_prefix {
                return Err("composition domain does not match its archive scope".into());
            }
        }
        _ if !scope.level_prefix.is_empty() => {
            return Err("non-archive composition cannot have a level prefix".into());
        }
        _ => {}
    }
    Ok(())
}

fn validate_key_domain(key: &str, domain: &PageDomain) -> Result<(), String> {
    let key_scope = BookScopeKey::new(key, "")?;
    if !Path::new(key).is_absolute() || key_scope.storage_key() != key || key.contains("::") {
        return Err("invalid composition scope key".into());
    }
    let scope = match domain {
        PageDomain::ArchiveLevel { prefix } if !prefix.is_empty() => {
            let normalized = crate::book_scope::normalize_level_prefix(prefix)?;
            let suffix = format!("/{normalized}");
            let root = key
                .strip_suffix(&suffix)
                .ok_or("composition archive prefix does not match key")?;
            BookScopeKey::new(root, &normalized)?
        }
        _ => key_scope,
    };
    validate_scope_domain(&scope, domain)
}

pub(crate) fn encode_row(
    key: &str,
    domain: &PageDomain,
    composition: &BookComposition,
) -> Result<String, String> {
    let domain = canonical_domain(domain)?;
    validate_key_domain(key, &domain)?;
    composition
        .validate(&domain)
        .map_err(|error| error.to_string())?;
    if composition.revision > i64::MAX as u64 {
        return Err("composition revision exceeds SQLite integer range".into());
    }
    let wire = serde_json::to_string(&RowWire {
        domain: (&domain).into(),
        composition: composition.clone(),
    })
    .map_err(|error| error.to_string())?;
    if wire.len() > super::MAX_JSON_BYTES {
        return Err("composition row exceeds JSON byte limit".into());
    }
    Ok(wire)
}

pub(crate) fn decode_row(
    key: &str,
    revision: i64,
    schema_version: i64,
    wire: &str,
) -> Result<StoredComposition, String> {
    if wire.len() > super::MAX_JSON_BYTES
        || revision <= 0
        || schema_version != super::SCHEMA_VERSION as i64
    {
        return Err("invalid or unsupported composition row version/size".into());
    }
    let row: RowWire = serde_json::from_str(wire).map_err(|error| error.to_string())?;
    let domain = canonical_domain(&PageDomain::from(row.domain))?;
    validate_key_domain(key, &domain)?;
    row.composition
        .validate(&domain)
        .map_err(|error| error.to_string())?;
    if row.composition.revision != revision as u64
        || row.composition.schema_version != schema_version as u32
    {
        return Err("composition row version disagrees with JSON".into());
    }
    Ok(StoredComposition {
        domain,
        composition: row.composition,
    })
}

pub(crate) fn initialize_database_at(path: &Path) -> Result<(), String> {
    if path.try_exists().map_err(|error| error.to_string())? {
        return validate_backup(path);
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
            "CREATE TABLE book_compositions (
        book_key TEXT PRIMARY KEY, revision INTEGER NOT NULL,
        schema_version INTEGER NOT NULL, composition_json TEXT NOT NULL);
        PRAGMA user_version = 2;",
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

pub(crate) fn validate_database_schema(connection: &Connection) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version != 2 {
        return Err(format!("unsupported composition database schema {version}"));
    }
    let mut statement = connection
        .prepare("PRAGMA table_info(book_compositions)")
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
    let expected = [
        ("book_key", "TEXT", false, 1),
        ("revision", "INTEGER", true, 0),
        ("schema_version", "INTEGER", true, 0),
        ("composition_json", "TEXT", true, 0),
    ];
    if columns.len() != expected.len()
        || columns.iter().zip(expected).any(|(actual, expected)| {
            actual.0 != expected.0
                || !actual.1.eq_ignore_ascii_case(expected.1)
                || actual.2 != expected.2
                || actual.3 != expected.3
        })
    {
        return Err("invalid composition table schema".into());
    }
    Ok(())
}

pub(crate) fn load_rows(
    connection: &Connection,
) -> Result<BTreeMap<String, StoredComposition>, String> {
    validate_database_schema(connection)?;
    let mut statement = connection.prepare("SELECT book_key, revision, schema_version, composition_json FROM book_compositions ORDER BY book_key").map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (key, revision, version, wire) = row.map_err(|error| error.to_string())?;
        let record = decode_row(&key, revision, version, &wire)?;
        result.insert(key, record);
    }
    Ok(result)
}

pub(crate) fn validate_database_rows(connection: &Connection) -> Result<(), String> {
    load_rows(connection).map(|_| ())
}

pub(crate) fn validate_backup(path: &Path) -> Result<(), String> {
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    validate_database_rows(&connection)
}

pub(crate) fn open_existing(path: &Path) -> Result<Option<Connection>, String> {
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| error.to_string())?;
    validate_database_rows(&connection)?;
    Ok(Some(connection))
}

fn table_name(table: &str) -> Result<&str, String> {
    match table {
        "book_compositions" | "main.book_compositions" | "book_composition.book_compositions" => {
            Ok(table)
        }
        _ => Err("invalid composition SQL table".into()),
    }
}

fn row_in(
    connection: &Connection,
    table: &str,
    key: &str,
) -> Result<Option<StoredComposition>, String> {
    let table = table_name(table)?;
    let raw = connection
        .query_row(
            &format!(
                "SELECT revision,schema_version,composition_json FROM {table} WHERE book_key=?1"
            ),
            [key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    raw.map(|(revision, version, wire)| decode_row(key, revision, version, &wire))
        .transpose()
}

/// The caller owns a transaction/SAVEPOINT; imported revisions never become local revisions.
pub(crate) fn replace_row_in(
    connection: &Connection,
    table: &str,
    key: &str,
    domain: &PageDomain,
    composition: &BookComposition,
) -> Result<bool, String> {
    let table = table_name(table)?;
    let domain = canonical_domain(domain)?;
    validate_key_domain(key, &domain)?;
    let mut replacement = composition.clone();
    replacement.revision = 0;
    replacement
        .validate(&domain)
        .map_err(|error| error.to_string())?;
    let previous = row_in(connection, table, key)?;
    if let Some(previous) = &previous {
        let mut comparable = previous.composition.clone();
        comparable.revision = 0;
        if previous.domain == domain && comparable == replacement {
            return Ok(false);
        }
    }
    replacement.revision = previous.as_ref().map_or(Ok(1), |previous| {
        previous
            .composition
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
            .ok_or("composition revision exhausted".to_owned())
    })?;
    let wire = encode_row(key, &domain, &replacement)?;
    connection.execute(&format!("INSERT INTO {table}(book_key,revision,schema_version,composition_json) VALUES (?1,?2,?3,?4) ON CONFLICT(book_key) DO UPDATE SET revision=excluded.revision,schema_version=excluded.schema_version,composition_json=excluded.composition_json"), rusqlite::params![key,replacement.revision as i64,replacement.schema_version as i64,wire]).map_err(|error| error.to_string())?;
    Ok(true)
}

pub(crate) fn remove_row_in(
    connection: &Connection,
    table: &str,
    key: &str,
) -> Result<bool, String> {
    let table = table_name(table)?;
    if row_in(connection, table, key)?.is_none() {
        return Ok(false);
    }
    connection
        .execute(&format!("DELETE FROM {table} WHERE book_key=?1"), [key])
        .map(|count| count > 0)
        .map_err(|error| error.to_string())
}

pub(crate) fn rename_book_root(
    connection: &mut Connection,
    source: &str,
    target: &str,
) -> Result<usize, String> {
    let source_scope = BookScopeKey::new(source, "")?;
    let target_scope = BookScopeKey::new(target, "")?;
    if source_scope.storage_key() != source
        || target_scope.storage_key() != target
        || !Path::new(source).is_absolute()
        || !Path::new(target).is_absolute()
    {
        return Err("invalid composition root mapping".into());
    }
    if source == target {
        return Ok(0);
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let original = load_rows(&transaction)?;
    let prefix = format!("{}/", source.trim_end_matches('/'));
    let mut changed = 0;
    for (key, row) in original {
        let destination = if key == source {
            target.to_owned()
        } else if let Some(suffix) = key.strip_prefix(&prefix) {
            format!("{}/{suffix}", target.trim_end_matches('/'))
        } else {
            continue;
        };
        let wire = encode_row(&destination, &row.domain, &row.composition)?;
        transaction.execute("INSERT OR IGNORE INTO book_compositions(book_key,revision,schema_version,composition_json) VALUES (?1,?2,?3,?4)",rusqlite::params![destination,row.composition.revision as i64,row.composition.schema_version as i64,wire]).map_err(|error|error.to_string())?;
        changed += transaction
            .execute("DELETE FROM book_compositions WHERE book_key=?1", [key])
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}

pub(crate) fn purge_book_root(connection: &mut Connection, root: &str) -> Result<usize, String> {
    let scope = BookScopeKey::new(root, "")?;
    if scope.storage_key() != root || !Path::new(root).is_absolute() {
        return Err("invalid composition purge root".into());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let original = load_rows(&transaction)?;
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let mut changed = 0;
    for key in original
        .keys()
        .filter(|key| key.as_str() == root || key.starts_with(&prefix))
    {
        changed += transaction
            .execute("DELETE FROM book_compositions WHERE book_key=?1", [key])
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}

/// Folder refs change only for moves within that same direct folder. Archive refs
/// and document numbers are unaffected by filesystem cache/root migration.
pub(crate) fn migrate_page_paths_at(
    data_dir: &Path,
    mappings: &[(PathBuf, PathBuf)],
) -> Result<usize, String> {
    if mappings.is_empty() {
        return Ok(0);
    }
    let Some(mut connection) = open_existing(&data_dir.join(DB_FILE))? else {
        return Ok(0);
    };
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let original = load_rows(&transaction)?;
    let mut changed = 0;
    for (key, row) in original {
        if row.domain != PageDomain::ImageFolder {
            continue;
        }
        let mut composition = row.composition;
        let mut rewrite = |page: &mut CompositionPageRef| {
            let CompositionPageRef::RelativePath { path } = page else {
                return;
            };
            let original = Path::new(&key).join(path.replace('\\', "/"));
            let original_key = crate::path_key::normalize_keep_drive(&original);
            if let Some((_, destination)) = mappings
                .iter()
                .find(|(source, _)| crate::path_key::normalize_keep_drive(source) == original_key)
                && destination
                    .parent()
                    .is_some_and(|parent| crate::path_key::normalize_keep_drive(parent) == key)
                && let Some(name) = destination.file_name().and_then(|name| name.to_str())
            {
                *path = name.to_owned();
            }
        };
        for page in &mut composition.page_order {
            rewrite(page);
        }
        for entry in &mut composition.page_overrides {
            rewrite(&mut entry.page);
        }
        for page in &mut composition.hidden_pages {
            rewrite(page);
        }
        changed += usize::from(replace_row_in(
            &transaction,
            "book_compositions",
            &key,
            &row.domain,
            &composition,
        )?);
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(changed)
}
