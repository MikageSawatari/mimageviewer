//! Bookshelf order storage. All filesystem entry points run on a book worker.
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::books::BookInfo;

pub const ORDER_FILE_NAME: &str = "mimageviewer.book-order.json";
pub const MAX_ORDER_BYTES: usize = 16 * 1024 * 1024;

struct OrderPerf(&'static str, Option<std::time::Instant>);

impl OrderPerf {
    fn new(kind: &'static str) -> Self {
        Self(
            kind,
            crate::perf::is_enabled().then(std::time::Instant::now),
        )
    }
}

impl Drop for OrderPerf {
    fn drop(&mut self) {
        if let Some(started) = self.1 {
            crate::perf::event(
                "book",
                self.0,
                None,
                0,
                &[(
                    "elapsed_ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                )],
            );
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BookOrderPolicy {
    #[default]
    Name,
    Manual(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileToken {
    Missing,
    Digest([u8; 32]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPresence {
    Missing,
    Existing,
}

/// Exact file contents and exact directory names; page edits do not invalidate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookOrderToken {
    file: FileToken,
    pub root_presence: RootPresence,
    pub directories: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum BookOrderReadState {
    Writable(BookOrderToken),
    ReadOnly(String),
}

#[derive(Clone, Debug)]
pub struct BookShelfSnapshot {
    pub root: PathBuf,
    pub root_presence: RootPresence,
    pub epoch: u64,
    pub revision: u64,
    pub rows: Vec<BookInfo>,
    pub policy: BookOrderPolicy,
    pub read: BookOrderReadState,
}

impl BookShelfSnapshot {
    pub fn token(&self) -> Option<&BookOrderToken> {
        match &self.read {
            BookOrderReadState::Writable(token) => Some(token),
            BookOrderReadState::ReadOnly(_) => None,
        }
    }
}

impl std::ops::Deref for BookShelfSnapshot {
    type Target = Vec<BookInfo>;
    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}

impl std::ops::DerefMut for BookShelfSnapshot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rows
    }
}

impl From<Vec<BookInfo>> for BookShelfSnapshot {
    fn from(rows: Vec<BookInfo>) -> Self {
        let root = rows
            .first()
            .and_then(|row| row.path.parent())
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let mut directories = rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
        directories.sort();
        Self {
            root,
            root_presence: RootPresence::Existing,
            epoch: 0,
            revision: 0,
            rows,
            policy: BookOrderPolicy::Name,
            read: BookOrderReadState::Writable(BookOrderToken {
                file: FileToken::Missing,
                root_presence: RootPresence::Existing,
                directories,
            }),
        }
    }
}

impl Default for BookShelfSnapshot {
    fn default() -> Self {
        Vec::new().into()
    }
}

#[derive(Clone, Debug)]
pub struct BookShelfCompletion {
    pub snapshot: Result<BookShelfSnapshot, String>,
    pub warning: Option<String>,
}

pub(crate) enum BookShelfMutation {
    Created(String),
    Renamed { old_name: String, new_name: String },
    Deleted(String),
    Unchanged,
}

/// The filesystem operation has succeeded. Never roll it back for order errors.
pub(crate) fn finish_mutation(
    root: &Path,
    before: Result<BookShelfSnapshot, String>,
    mutation: BookShelfMutation,
) -> BookShelfCompletion {
    let previous_presence = before.as_ref().ok().map(|snapshot| snapshot.root_presence);
    let attempted = (|| {
        let before = before?;
        let Some(mut token) = before.token().cloned() else {
            return Ok(None);
        };
        if !matches!(before.policy, BookOrderPolicy::Manual(_))
            || matches!(mutation, BookShelfMutation::Unchanged)
        {
            return Ok(None);
        }
        let mut names = before
            .rows
            .into_iter()
            .map(|row| row.name)
            .collect::<Vec<_>>();
        match mutation {
            BookShelfMutation::Created(name) => {
                if token
                    .directories
                    .iter()
                    .any(|entry| entry.to_lowercase() == name.to_lowercase())
                {
                    return Ok(None);
                }
                names.push(name.clone());
                token.directories.push(name);
                token.root_presence = RootPresence::Existing;
            }
            BookShelfMutation::Renamed { old_name, new_name } => {
                if let Some(name) = names.iter_mut().find(|name| **name == old_name) {
                    *name = new_name.clone();
                }
                if let Some(name) = token.directories.iter_mut().find(|name| **name == old_name) {
                    *name = new_name;
                }
            }
            BookShelfMutation::Deleted(name) => {
                names.retain(|entry| entry != &name);
                token.directories.retain(|entry| entry != &name);
            }
            BookShelfMutation::Unchanged => unreachable!(),
        }
        token.directories.sort();
        write_order(root, &token, &BookOrderPolicy::Manual(names))
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(None::<()>)
    })();
    let warning = attempted
        .err()
        .map(|error| format!("本の操作は完了しましたが、本の順序を保存できませんでした: {error}"));
    if let Some(warning) = &warning {
        crate::logger::log(format!("[book-order] {warning}"));
    }
    BookShelfCompletion {
        snapshot: read_snapshot_with_prior(root, previous_presence),
        warning,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookOrderWriteError {
    Conflict,
    Failed(String),
}

impl std::fmt::Display for BookOrderWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => f.write_str("本棚が変更されました。最新の内容を読み直します"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    mode: Mode,
    books: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Name,
    Manual,
}

pub fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_ascii_lowercase()
        .cmp(&b.to_ascii_lowercase())
        .then_with(|| a.cmp(b))
}

fn validate_names(names: &[String]) -> Result<(), String> {
    let mut keys = HashSet::with_capacity(names.len());
    for name in names {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\\', ':', '\0'])
            || Path::new(name).is_absolute()
        {
            return Err(format!("本の順序に不正な本名があります: {name:?}"));
        }
        if !keys.insert(name.to_lowercase()) {
            return Err(format!(
                "本の順序に重複または曖昧な本名があります: {name:?}"
            ));
        }
    }
    Ok(())
}

fn validate_policy(policy: &BookOrderPolicy) -> Result<(), String> {
    match policy {
        BookOrderPolicy::Name => Ok(()),
        BookOrderPolicy::Manual(names) => validate_names(names),
    }
}

/// Resolve exact names first, then the existing Windows path case comparison.
/// The strings from storage never become filesystem paths.
pub fn resolve_rows(
    policy: &BookOrderPolicy,
    mut rows: Vec<BookInfo>,
) -> Result<Vec<BookInfo>, String> {
    validate_policy(policy)?;
    let names = rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
    validate_names(&names)?;
    if rows
        .iter()
        .any(|row| row.path.file_name().and_then(|name| name.to_str()) != Some(row.name.as_str()))
    {
        return Err("本の名前を一意に読み取れません".into());
    }
    rows.sort_by(|a, b| compare_names(&a.name, &b.name));
    let BookOrderPolicy::Manual(saved) = policy else {
        return Ok(rows);
    };
    let mut slots: HashMap<String, BookInfo> = rows
        .into_iter()
        .map(|row| (row.name.to_lowercase(), row))
        .collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for name in saved {
        if let Some(row) = slots.remove(&name.to_lowercase()) {
            ordered.push(row);
        }
    }
    let mut unknown = slots.into_values().collect::<Vec<_>>();
    unknown.sort_by(|a, b| compare_names(&a.name, &b.name));
    ordered.extend(unknown);
    Ok(ordered)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn read_file(root: &Path) -> Result<(BookOrderPolicy, FileToken), String> {
    let path = root.join(ORDER_FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((BookOrderPolicy::Name, FileToken::Missing));
        }
        Err(error) => return Err(format!("本の順序を読み取れません: {error}")),
    };
    if !metadata.is_file() || is_reparse(&metadata) {
        return Err("本の順序の保存先が通常ファイルではありません".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| {
            file.take(MAX_ORDER_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("本の順序を読み取れません: {error}"))?;
    if bytes.len() > MAX_ORDER_BYTES {
        return Err("本の順序ファイルが16 MiBを超えています".into());
    }
    let doc: Document = serde_json::from_slice(&bytes)
        .map_err(|error| format!("本の順序ファイルを解釈できません: {error}"))?;
    if doc.version != 1 {
        return Err(format!(
            "本の順序ファイルの未対応バージョン: {}",
            doc.version
        ));
    }
    let policy = match doc.mode {
        Mode::Name if doc.books.is_empty() => BookOrderPolicy::Name,
        Mode::Name => return Err("名前順の本一覧が空ではありません".into()),
        Mode::Manual => BookOrderPolicy::Manual(doc.books),
    };
    validate_policy(&policy)?;
    Ok((policy, FileToken::Digest(Sha256::digest(&bytes).into())))
}

fn observe_root(root: &Path) -> Result<RootPresence, String> {
    match fs::symlink_metadata(root) {
        Ok(_) => Ok(RootPresence::Existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RootPresence::Missing),
        Err(error) => Err(format!(
            "本棚の状態を読み取れません: {}: {error}",
            root.display()
        )),
    }
}

fn directory_names(root: &Path) -> Result<(RootPresence, Vec<String>), String> {
    let root_presence = observe_root(root)?;
    if root_presence == RootPresence::Missing {
        return Ok((root_presence, Vec::new()));
    }
    let entries = fs::read_dir(root).map_err(|error| format!("本棚を読み取れません: {error}"))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("本棚の項目を読み取れません: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("本棚の項目種別を読み取れません: {error}"))?
            .is_dir()
        {
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "本の名前を一意に読み取れません")?,
            );
        }
    }
    names.sort();
    validate_names(&names)?;
    Ok((root_presence, names))
}

/// A bad order file still yields a complete name-ordered, protected snapshot.
/// Directory/page enumeration errors instead fail the entire read.
pub fn read_snapshot(root: &Path) -> Result<BookShelfSnapshot, String> {
    read_snapshot_with_prior(root, None)
}

pub fn read_snapshot_with_prior(
    root: &Path,
    previous_presence: Option<RootPresence>,
) -> Result<BookShelfSnapshot, String> {
    let _perf = OrderPerf::new("order_read");
    let root_presence = observe_root(root)?;
    if previous_presence == Some(RootPresence::Existing) && root_presence == RootPresence::Missing {
        return Err(format!(
            "以前読み込んだ本棚が見つかりません: {}",
            root.display()
        ));
    }
    let mut rows = crate::books::list_books_name_order(root, root_presence)?;
    rows.sort_by(|a, b| compare_names(&a.name, &b.name));
    let loaded = read_file(root).and_then(|(policy, file)| {
        let mut directories = rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
        directories.sort();
        let ordered = resolve_rows(&policy, rows.clone())?;
        Ok((
            policy,
            ordered,
            BookOrderToken {
                file,
                root_presence,
                directories,
            },
        ))
    });
    let (policy, rows, read) = match loaded {
        Ok((policy, rows, token)) => (policy, rows, BookOrderReadState::Writable(token)),
        Err(error) => {
            crate::logger::log(format!("[book-order] {error}"));
            (
                BookOrderPolicy::Name,
                rows,
                BookOrderReadState::ReadOnly(error),
            )
        }
    };
    Ok(BookShelfSnapshot {
        root: root.to_path_buf(),
        root_presence,
        epoch: 0,
        revision: 0,
        rows,
        policy,
        read,
    })
}

fn check_expected(root: &Path, expected: &BookOrderToken) -> Result<(), BookOrderWriteError> {
    let (_, file) = read_file(root).map_err(|_| BookOrderWriteError::Conflict)?;
    let (root_presence, directories) =
        directory_names(root).map_err(BookOrderWriteError::Failed)?;
    if root_presence != expected.root_presence
        || file != expected.file
        || directories != expected.directories
    {
        return Err(BookOrderWriteError::Conflict);
    }
    Ok(())
}

pub fn write_order(
    root: &Path,
    expected: &BookOrderToken,
    policy: &BookOrderPolicy,
) -> Result<BookOrderToken, BookOrderWriteError> {
    let _perf = OrderPerf::new("order_write");
    write_order_observed(root, expected, policy, |_| {})
}

fn write_order_observed(
    root: &Path,
    expected: &BookOrderToken,
    policy: &BookOrderPolicy,
    before_replace: impl FnOnce(&Path),
) -> Result<BookOrderToken, BookOrderWriteError> {
    validate_policy(policy).map_err(BookOrderWriteError::Failed)?;
    if let BookOrderPolicy::Manual(names) = policy {
        let mut names = names.clone();
        names.sort();
        if names != expected.directories {
            return Err(BookOrderWriteError::Failed(
                "保存する本の順序が本棚の全件と一致しません".into(),
            ));
        }
    }
    check_expected(root, expected)?;
    let (mode, books) = match policy {
        BookOrderPolicy::Name => (Mode::Name, Vec::new()),
        BookOrderPolicy::Manual(names) => (Mode::Manual, names.clone()),
    };
    let bytes = serde_json::to_vec(&Document {
        version: 1,
        mode,
        books,
    })
    .map_err(|error| BookOrderWriteError::Failed(error.to_string()))?;
    if bytes.len() > MAX_ORDER_BYTES {
        return Err(BookOrderWriteError::Failed(
            "本の順序ファイルが16 MiBを超えています".into(),
        ));
    }
    if expected.root_presence == RootPresence::Missing {
        fs::create_dir_all(root).map_err(|error| BookOrderWriteError::Failed(error.to_string()))?;
    }
    // Initial saving may create an uncreated shelf. Only this writer-owned
    // transition advances the pre-replace observation.
    let mut created_expected = expected.clone();
    created_expected.root_presence = RootPresence::Existing;
    let temp = root.join(format!(
        "mimageviewer.book-order.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| BookOrderWriteError::Failed(error.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| BookOrderWriteError::Failed(error.to_string()))?;
        drop(file);
        before_replace(&temp);
        check_expected(root, &created_expected)?;
        crate::archive_converter::replace_file_atomic(&temp, &root.join(ORDER_FILE_NAME), false)
            .map_err(|error| BookOrderWriteError::Failed(error.to_string()))?;
        Ok(BookOrderToken {
            file: FileToken::Digest(Sha256::digest(&bytes).into()),
            root_presence: RootPresence::Existing,
            directories: expected.directories.clone(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<BookInfo> {
        names
            .iter()
            .map(|name| BookInfo {
                name: (*name).into(),
                path: Path::new("books").join(name),
                page_count: 0,
            })
            .collect()
    }

    fn names(rows: &[BookInfo]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    fn folders(root: &Path, names: &[&str]) {
        for name in names {
            fs::create_dir_all(root.join(name)).unwrap();
        }
    }

    fn save(root: &Path, names: &[&str]) -> BookOrderToken {
        let snapshot = read_snapshot(root).unwrap();
        write_order(
            root,
            snapshot.token().unwrap(),
            &BookOrderPolicy::Manual(names.iter().map(|name| (*name).into()).collect()),
        )
        .unwrap()
    }

    #[test]
    fn legacy_name_comparator_is_unchanged() {
        let mut original = rows(&[
            "他", "ｍIV", "mIV", "mychara", "A", "a", "10", "2", " A", "A ",
        ]);
        original.sort_by(|a, b| compare_names(&a.name, &b.name));
        assert_eq!(
            names(&original),
            [
                " A", "10", "2", "A", "a", "A ", "mIV", "mychara", "他", "ｍIV"
            ]
        );
    }

    #[test]
    fn manual_missing_reappearing_and_unknown_group_are_resolved_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["C", "A"]);
        let token = save(root, &["C", "A"]);
        folders(root, &["D"]);
        assert_eq!(names(&read_snapshot(root).unwrap().rows), ["C", "A", "D"]);
        folders(root, &["B"]);
        assert_eq!(
            names(&read_snapshot(root).unwrap().rows),
            ["C", "A", "B", "D"]
        );
        fs::remove_dir(root.join("C")).unwrap();
        assert_eq!(names(&read_snapshot(root).unwrap().rows), ["A", "B", "D"]);
        folders(root, &["C"]);
        assert_eq!(
            names(&read_snapshot(root).unwrap().rows),
            ["C", "A", "B", "D"]
        );
        assert_eq!(read_file(root).unwrap().1, token.file);
    }

    #[test]
    fn unique_case_match_and_invalid_or_ambiguous_names() {
        assert_eq!(
            names(
                &resolve_rows(
                    &BookOrderPolicy::Manual(vec!["b".into()]),
                    rows(&["A", "B"])
                )
                .unwrap()
            ),
            ["B", "A"]
        );
        for invalid in [
            vec![""],
            vec!["."],
            vec![".."],
            vec!["C:/A"],
            vec!["a/b"],
            vec!["a\\b"],
            vec!["A", "a"],
            vec!["A", "A"],
        ] {
            assert!(
                resolve_rows(
                    &BookOrderPolicy::Manual(invalid.into_iter().map(str::to_string).collect()),
                    rows(&["A"])
                )
                .is_err()
            );
        }
        assert!(resolve_rows(&BookOrderPolicy::Name, rows(&["A", "a"])).is_err());
    }

    #[test]
    fn read_only_migration_empty_manual_and_name_have_distinct_meanings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("not-created");
        let initial = read_snapshot(&root).unwrap();
        assert_eq!(initial.policy, BookOrderPolicy::Name);
        assert_eq!(initial.root_presence, RootPresence::Missing);
        assert!(!root.exists());
        let token = write_order(
            &root,
            initial.token().unwrap(),
            &BookOrderPolicy::Manual(Vec::new()),
        )
        .unwrap();
        assert_eq!(token.root_presence, RootPresence::Existing);
        assert_eq!(
            read_snapshot(&root).unwrap().policy,
            BookOrderPolicy::Manual(Vec::new())
        );
        write_order(&root, &token, &BookOrderPolicy::Name).unwrap();
        assert_eq!(read_snapshot(&root).unwrap().policy, BookOrderPolicy::Name);
        assert!(root.join(ORDER_FILE_NAME).is_file());
    }

    #[test]
    fn cold_missing_root_is_empty_but_previously_existing_root_is_a_read_error() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        let cold = read_snapshot(&root).unwrap();
        assert_eq!(cold.root_presence, RootPresence::Missing);
        assert!(cold.rows.is_empty());
        folders(&root, &["A"]);
        let warm = read_snapshot_with_prior(&root, Some(cold.root_presence)).unwrap();
        assert_eq!(warm.root_presence, RootPresence::Existing);
        fs::remove_dir_all(&root).unwrap();
        assert!(read_snapshot_with_prior(&root, Some(warm.root_presence)).is_err());
        assert_eq!(
            read_snapshot(&root).unwrap().root_presence,
            RootPresence::Missing
        );
        assert!(!root.exists());
    }

    #[test]
    fn disappearing_existing_empty_root_conflicts_without_recreating_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        fs::create_dir(&root).unwrap();
        let before = read_snapshot(&root).unwrap();
        assert_eq!(before.root_presence, RootPresence::Existing);
        assert!(before.rows.is_empty());
        fs::remove_dir(&root).unwrap();
        assert_eq!(
            write_order(
                &root,
                before.token().unwrap(),
                &BookOrderPolicy::Manual(Vec::new())
            ),
            Err(BookOrderWriteError::Conflict)
        );
        assert!(!root.exists());
    }

    #[test]
    fn disappearing_root_is_checked_again_immediately_before_replace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("books");
        fs::create_dir(&root).unwrap();
        let before = read_snapshot(&root).unwrap();
        let result = write_order_observed(
            &root,
            before.token().unwrap(),
            &BookOrderPolicy::Name,
            |temp| {
                fs::remove_file(temp).unwrap();
                fs::remove_dir(&root).unwrap();
            },
        );
        assert_eq!(result, Err(BookOrderWriteError::Conflict));
        assert!(!root.exists());
    }

    #[test]
    fn unreadable_policy_is_name_order_and_never_overwritten_by_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["C", "A"]);
        for contents in [
            b"broken".as_slice(),
            br#"{"version":2,"mode":"manual","books":["C","A"]}"#,
            br#"{"version":1,"mode":"name","books":["A"]}"#,
        ] {
            fs::write(root.join(ORDER_FILE_NAME), contents).unwrap();
            let before = read_snapshot(root).unwrap();
            assert_eq!(names(&before.rows), ["A", "C"]);
            assert!(before.token().is_none());
            folders(root, &["B"]);
            let completion =
                finish_mutation(root, Ok(before), BookShelfMutation::Created("B".into()));
            assert!(completion.snapshot.unwrap().token().is_none());
            assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), contents);
            fs::remove_dir(root.join("B")).unwrap();
        }
    }

    #[test]
    fn reserved_directory_and_oversized_file_are_protected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join(ORDER_FILE_NAME)).unwrap();
        assert!(read_snapshot(root).unwrap().token().is_none());
        fs::remove_dir(root.join(ORDER_FILE_NAME)).unwrap();
        fs::write(root.join(ORDER_FILE_NAME), vec![b' '; MAX_ORDER_BYTES + 1]).unwrap();
        assert!(read_snapshot(root).unwrap().token().is_none());
    }

    #[test]
    fn read_accepts_exact_size_limit_and_rejects_limit_plus_one() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(ORDER_FILE_NAME);
        let mut bytes = br#"{"version":1,"mode":"name","books":[]}"#.to_vec();
        bytes.resize(MAX_ORDER_BYTES, b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(read_snapshot(temp.path()).unwrap().token().is_some());
        bytes.push(b' ');
        fs::write(&path, &bytes).unwrap();
        assert!(read_snapshot(temp.path()).unwrap().token().is_none());
    }

    #[test]
    fn external_file_or_directory_change_conflicts_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A", "B"]);
        let before = read_snapshot(root).unwrap();
        folders(root, &["C"]);
        assert_eq!(
            write_order(
                root,
                before.token().unwrap(),
                &BookOrderPolicy::Manual(vec!["B".into(), "A".into()])
            ),
            Err(BookOrderWriteError::Conflict)
        );
        assert!(!root.join(ORDER_FILE_NAME).exists());
        fs::remove_dir(root.join("C")).unwrap();
        fs::write(
            root.join(ORDER_FILE_NAME),
            br#"{"version":1,"mode":"name","books":[]}"#,
        )
        .unwrap();
        assert_eq!(
            write_order(root, before.token().unwrap(), &BookOrderPolicy::Name),
            Err(BookOrderWriteError::Conflict)
        );
    }

    #[test]
    fn replacement_rechecks_contents_and_keeps_external_original() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A", "B"]);
        let token = save(root, &["B", "A"]);
        let external = br#"{"version":1,"mode":"name","books":[]}"#;
        let result = write_order_observed(root, &token, &BookOrderPolicy::Name, |temp| {
            assert!(temp.is_file());
            fs::write(root.join(ORDER_FILE_NAME), external).unwrap();
        });
        assert_eq!(result, Err(BookOrderWriteError::Conflict));
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), external);
        assert_eq!(fs::read_dir(root).unwrap().count(), 3);
    }

    #[test]
    fn replacement_rechecks_directory_set_and_removes_only_its_temp() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A"]);
        let token = save(root, &["A"]);
        let original = fs::read(root.join(ORDER_FILE_NAME)).unwrap();
        let orphan = root.join("mimageviewer.book-order.old.tmp");
        fs::write(&orphan, b"ignore me").unwrap();
        assert_eq!(
            write_order_observed(root, &token, &BookOrderPolicy::Name, |_| folders(
                root,
                &["B"]
            )),
            Err(BookOrderWriteError::Conflict)
        );
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
        assert_eq!(fs::read(orphan).unwrap(), b"ignore me");
    }

    #[test]
    fn failed_atomic_replace_preserves_last_saved_file_and_initial_absence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A"]);
        let initial = read_snapshot(root).unwrap();
        let result = write_order_observed(
            root,
            initial.token().unwrap(),
            &BookOrderPolicy::Manual(vec!["A".into()]),
            |temp| fs::remove_file(temp).unwrap(),
        );
        assert!(matches!(result, Err(BookOrderWriteError::Failed(_))));
        assert!(!root.join(ORDER_FILE_NAME).exists());
        let token = save(root, &["A"]);
        let original = fs::read(root.join(ORDER_FILE_NAME)).unwrap();
        let result = write_order_observed(root, &token, &BookOrderPolicy::Name, |temp| {
            fs::remove_file(temp).unwrap()
        });
        assert!(matches!(result, Err(BookOrderWriteError::Failed(_))));
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
    }

    #[test]
    fn incomplete_root_enumeration_is_not_an_empty_success() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("not-a-directory");
        fs::write(&root, b"file").unwrap();
        assert!(read_snapshot(&root).is_err());
    }

    #[test]
    fn management_rename_of_unknown_book_preserves_resolved_position() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A", "C"]);
        save(root, &["C", "A"]);
        folders(root, &["B", "D"]);
        let before = read_snapshot(root);
        fs::rename(root.join("B"), root.join("Z")).unwrap();
        let completion = finish_mutation(
            root,
            before,
            BookShelfMutation::Renamed {
                old_name: "B".into(),
                new_name: "Z".into(),
            },
        );
        assert!(completion.warning.is_none());
        assert_eq!(
            names(&completion.snapshot.unwrap().rows),
            ["C", "A", "Z", "D"]
        );
    }

    #[test]
    fn lifecycle_manual_create_rename_delete_and_external_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A", "C"]);
        save(root, &["C", "A"]);
        let result = crate::books::create_book(root, "B").unwrap();
        let crate::books::BookOpResult::ShelfUpdated { shelf, .. } = result else {
            panic!("completion required")
        };
        assert!(shelf.warning.is_none());
        assert_eq!(names(&shelf.snapshot.unwrap().rows), ["C", "A", "B"]);
        let before = read_snapshot(root);
        fs::rename(root.join("C"), root.join("D")).unwrap();
        let completion = finish_mutation(
            root,
            before,
            BookShelfMutation::Renamed {
                old_name: "C".into(),
                new_name: "D".into(),
            },
        );
        assert!(completion.warning.is_none());
        assert_eq!(names(&completion.snapshot.unwrap().rows), ["D", "A", "B"]);
        let before = read_snapshot(root);
        fs::remove_dir(root.join("A")).unwrap();
        let completion = finish_mutation(root, before, BookShelfMutation::Deleted("A".into()));
        assert!(completion.warning.is_none());
        assert_eq!(names(&completion.snapshot.unwrap().rows), ["D", "B"]);
        let before = read_snapshot(root);
        let original = fs::read(root.join(ORDER_FILE_NAME)).unwrap();
        folders(root, &["E", "External"]);
        let completion = finish_mutation(root, before, BookShelfMutation::Created("E".into()));
        assert!(completion.warning.is_some());
        assert!(root.join("E").is_dir());
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
    }

    #[test]
    fn existing_append_and_name_lifecycle_never_create_or_rewrite_policy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A"]);
        let before = read_snapshot(root);
        folders(root, &["B"]);
        assert!(
            finish_mutation(root, before, BookShelfMutation::Created("B".into()))
                .warning
                .is_none()
        );
        assert!(!root.join(ORDER_FILE_NAME).exists());
        save(root, &["B", "A"]);
        let original = fs::read(root.join(ORDER_FILE_NAME)).unwrap();
        let before = read_snapshot(root);
        assert!(
            finish_mutation(root, before, BookShelfMutation::Created("a".into()))
                .warning
                .is_none()
        );
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
    }

    #[test]
    fn physical_root_move_and_full_copy_carry_order_book_only_copy_is_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        folders(&original, &["A", "C"]);
        save(&original, &["C", "A"]);
        let moved = temp.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        assert_eq!(names(&read_snapshot(&moved).unwrap().rows), ["C", "A"]);
        let copied = temp.path().join("copied");
        folders(&copied, &["A", "C"]);
        fs::copy(moved.join(ORDER_FILE_NAME), copied.join(ORDER_FILE_NAME)).unwrap();
        assert_eq!(names(&read_snapshot(&copied).unwrap().rows), ["C", "A"]);
        let single = temp.path().join("single");
        folders(&single, &["A"]);
        save(&single, &["A"]);
        fs::create_dir(single.join("C")).unwrap();
        assert_eq!(names(&read_snapshot(&single).unwrap().rows), ["A", "C"]);
        assert_eq!(names(&read_snapshot(&moved).unwrap().rows), ["C", "A"]);
    }

    #[test]
    fn missing_saved_name_is_dropped_only_at_next_successful_write() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        folders(root, &["A", "C"]);
        save(root, &["C", "A"]);
        fs::remove_dir(root.join("C")).unwrap();
        save(root, &["A"]);
        folders(root, &["C"]);
        assert_eq!(names(&read_snapshot(root).unwrap().rows), ["A", "C"]);
    }

    #[test]
    fn released_v4_reader_ignores_v5_order_and_temp_and_v5_reconciles_old_operations() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // v1.7.0's shipped layout: plain directories with numbered images.
        folders(root, &["A", "C"]);
        fs::write(root.join("A/0001_page.jpg"), b"a").unwrap();
        fs::write(root.join("C/0001_page.png"), b"c").unwrap();
        assert_eq!(names(&read_snapshot(root).unwrap().rows), ["A", "C"]);
        assert!(!root.join(ORDER_FILE_NAME).exists());
        save(root, &["C", "A"]);
        let original = fs::read(root.join(ORDER_FILE_NAME)).unwrap();
        fs::write(
            root.join("mimageviewer.book-order.orphan.tmp"),
            b"incomplete",
        )
        .unwrap();
        let old = super::v4_reader_fixture::list_books(root).unwrap();
        assert_eq!(names(&old), ["A", "C"]);
        assert_eq!(
            old.iter().map(|row| row.page_count).collect::<Vec<_>>(),
            [1, 1]
        );
        super::v4_reader_fixture::create_book(root, "B").unwrap();
        // The released filesystem effects do not know about the root order file.
        fs::write(root.join("B/0001_added.jpg"), b"b").unwrap();
        fs::rename(root.join("C"), root.join("D")).unwrap();
        fs::remove_dir_all(root.join("A")).unwrap();
        assert_eq!(
            names(&super::v4_reader_fixture::list_books(root).unwrap()),
            ["B", "D"]
        );
        assert_eq!(names(&read_snapshot(root).unwrap().rows), ["B", "D"]);
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
        folders(root, &["A", "C"]);
        assert_eq!(
            names(&read_snapshot(root).unwrap().rows),
            ["C", "A", "B", "D"]
        );
        assert_eq!(fs::read(root.join(ORDER_FILE_NAME)).unwrap(), original);
    }
}

// Source fixture: v4.11.0 (2322f5a2c), src/books.rs. These function bodies
// are copied from the released tag, including its directory-only enumeration.
#[cfg(test)]
mod v4_reader_fixture {
    use crate::books::{BookInfo, BookOpResult, MAX_BOOK_PAGES, book_folder, normalize_book_name};
    use std::fs;
    use std::path::{Path, PathBuf};
    pub fn list_books(root: &Path) -> Result<Vec<BookInfo>, String> {
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        for entry in fs::read_dir(root)
            .map_err(|e| format!("本棚を読み取れません: {}: {e}", root.display()))?
        {
            let entry = entry.map_err(|e| format!("本棚の項目を読み取れません: {e}"))?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|e| format!("本棚の項目種別を読み取れません: {}: {e}", path.display()))?
                .is_dir()
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            rows.push(BookInfo {
                page_count: book_page_count(&path)?,
                path,
                name,
            });
        }
        rows.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(rows)
    }

    pub fn create_book(root: &Path, name: &str) -> Result<BookOpResult, String> {
        let name = normalize_book_name(name);
        let path = book_folder(root, &name);
        if path.exists() {
            return Err(format!("同名の本が既にあります: {name}"));
        }
        fs::create_dir_all(root)
            .map_err(|e| format!("本棚フォルダを作成できません: {}: {e}", root.display()))?;
        fs::create_dir(&path)
            .map_err(|e| format!("本を作成できません: {}: {e}", path.display()))?;
        Ok(BookOpResult::Created { name, path })
    }

    fn book_page_count(folder: &Path) -> Result<usize, String> {
        Ok(book_page_paths(folder)?.len())
    }

    fn book_page_paths(folder: &Path) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(folder)
            .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
        {
            let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if page_number_from_name(&name).is_some() && is_supported_book_image_path(&path) {
                paths.push(path);
            }
        }
        paths.sort_by_key(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(page_number_from_name)
                .unwrap_or(MAX_BOOK_PAGES + 1)
        });
        Ok(paths)
    }

    fn page_number_from_name(name: &str) -> Option<usize> {
        let bytes = name.as_bytes();
        if bytes.len() < 6 || bytes[4] != b'_' {
            return None;
        }
        let digits = &bytes[0..4];
        if !digits.iter().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value = std::str::from_utf8(digits).ok()?.parse::<usize>().ok()?;
        (1..=MAX_BOOK_PAGES).contains(&value).then_some(value)
    }

    fn is_supported_book_image_path(path: &Path) -> bool {
        if crate::raw_format::is_raw_path(path) {
            return true;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "bmp"
                | "gif"
                | "heic"
                | "heif"
                | "avif"
                | "jxl"
                | "tif"
                | "tiff"
        )
    }
}
