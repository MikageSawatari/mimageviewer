use super::*;

fn adjustment_fixture(data: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(data.join("adjustment.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE page_params(page_path TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE sidecar_sync(folder_key TEXT PRIMARY KEY);",
    )
    .unwrap();
    conn
}

fn adjustment(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM page_params WHERE page_path=?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .unwrap()
}

#[test]
fn section1886_cache_faces_move_adjustment_rating_spread_and_bookmark() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/old.rar");
    let new = Path::new("C:/section1886/new.rar");
    let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
    let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
    let old_key = crate::adjustment_db::zip_entry_key(&old_cache, "page.jpg");
    let new_key = crate::adjustment_db::zip_entry_key(&new_cache, "page.jpg");
    let adjustments = adjustment_fixture(temp.path());
    adjustments
        .execute(
            "INSERT INTO page_params VALUES (?1,'source edit')",
            [&old_key],
        )
        .unwrap();
    let rating = rusqlite::Connection::open(temp.path().join("rating.db")).unwrap();
    rating
        .execute_batch(
            "CREATE TABLE ratings(path TEXT PRIMARY KEY, stars INTEGER, source_path TEXT);",
        )
        .unwrap();
    rating
        .execute("INSERT INTO ratings VALUES (?1,4,'stale')", [&old_key])
        .unwrap();
    let spread = rusqlite::Connection::open(temp.path().join("spread.db")).unwrap();
    spread
        .execute_batch("CREATE TABLE spreads(path TEXT PRIMARY KEY, mode INTEGER);")
        .unwrap();
    spread
        .execute(
            "INSERT INTO spreads VALUES (?1,2)",
            [crate::path_key::normalize(&old_cache)],
        )
        .unwrap();
    let bookmark_db = temp.path().join("book_bookmarks.db");
    crate::book_bookmarks::ensure_schema_at(&bookmark_db).unwrap();
    let bookmarks = rusqlite::Connection::open(bookmark_db).unwrap();
    bookmarks.execute("INSERT INTO book_bookmarks
        (container_key,container_path,container_kind,page_kind,page_value,page_key,page_index_hint,created_at_ms)
        VALUES (?1,?2,'zip','archive_entry','page.jpg','page.jpg',0,1)",
        rusqlite::params![crate::book_bookmarks::container_key(&old_cache),old_cache.to_string_lossy()]).unwrap();
    let report = run_at(temp.path(), old, new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.converted_cache_pairs,
        vec![(old_cache.clone(), new_cache.clone())]
    );
    assert_eq!(
        adjustment(&adjustments, &new_key).as_deref(),
        Some("source edit")
    );
    assert!(adjustment(&adjustments, &old_key).is_none());
    let (path, stars, source): (String, i64, String) = rating
        .query_row("SELECT path,stars,source_path FROM ratings", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(
        (path, stars, source),
        (new_key, 4, crate::adjustment_db::normalize_path(&new_cache))
    );
    let spread_row: (String, i64) = spread
        .query_row("SELECT path,mode FROM spreads", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(spread_row, (crate::path_key::normalize(&new_cache), 2));
    let bookmark_row: (String, String, String) = bookmarks
        .query_row(
            "SELECT container_key,container_path,page_value FROM book_bookmarks",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        bookmark_row,
        (
            crate::book_bookmarks::container_key(&new_cache),
            new_cache.to_string_lossy().into_owned(),
            "page.jpg".into()
        )
    );
}

#[test]
fn section1886_bookmark_only_mutation_is_reported_without_generic_rows() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/old.lzh");
    let new = Path::new("C:/section1886/new.lzh");
    let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
    let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
    let db = temp.path().join("book_bookmarks.db");
    crate::book_bookmarks::ensure_schema_at(&db).unwrap();
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute("INSERT INTO book_bookmarks
        (container_key,container_path,container_kind,page_kind,page_value,page_key,page_index_hint,created_at_ms)
        VALUES (?1,?2,'zip','archive_entry','page.jpg','page.jpg',0,1)",
        rusqlite::params![crate::book_bookmarks::container_key(&old_cache),old_cache.to_string_lossy()]).unwrap();
    let report = run_at(temp.path(), old, new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.converted_cache_pairs, vec![(old_cache, new_cache)]);
}

#[test]
fn section1886_empty_cache_root_does_not_filter_fs_snapshot_or_scan_per_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir(&new).unwrap();
    for index in 0..320 {
        std::fs::write(new.join(format!("book{index}.rar")), b"fixture").unwrap();
    }
    let enumeration = collect_converted_archive_rels(&new);
    assert!(!enumeration.incomplete);
    assert_eq!(enumeration.converted_archive_rels.len(), 320);
    let adjustments = adjustment_fixture(&data);
    let unchanged = crate::adjustment_db::normalize_path(Path::new("C:/unrelated/page.jpg"));
    adjustments
        .execute(
            "INSERT INTO page_params VALUES (?1,'unrelated')",
            [&unchanged],
        )
        .unwrap();
    converted_archives::CACHE_ROOT_READS.with(|reads| reads.set(0));
    let report = run_at_with_converted_archives(
        &data,
        &old,
        &new,
        true,
        &enumeration.converted_archive_rels,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.rows, 0);
    assert!(report.converted_cache_pairs.is_empty());
    converted_archives::CACHE_ROOT_READS.with(|reads| {
        assert_eq!(
            reads.get(),
            2,
            "one cache-root range query for each adjustment descriptor"
        )
    });
    assert_eq!(
        adjustment(&adjustments, &unchanged).as_deref(),
        Some("unrelated")
    );
}

#[test]
fn section1886_adjustment_destination_wins_and_store_rolls_back_all_faces_on_error() {
    for fail in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let old = Path::new("C:/section1886/old.rar");
        let new = Path::new("C:/section1886/new.rar");
        let old_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), old);
        let new_cache = crate::archive_cache::cache_zip_path_for_data_dir(temp.path(), new);
        let old_key = crate::adjustment_db::zip_entry_key(&old_cache, "page.jpg");
        let new_key = crate::adjustment_db::zip_entry_key(&new_cache, "page.jpg");
        let conn = adjustment_fixture(temp.path());
        conn.execute("INSERT INTO page_params VALUES (?1,'old edit')", [&old_key])
            .unwrap();
        let old_logical = crate::adjustment_db::normalize_path(old);
        let new_logical = crate::adjustment_db::normalize_path(new);
        conn.execute(
            "INSERT INTO page_params VALUES (?1,'logical edit')",
            [&old_logical],
        )
        .unwrap();
        if fail {
            conn.execute_batch(&format!(
                "CREATE TRIGGER fail_cache BEFORE UPDATE OF page_path ON page_params
                WHEN NEW.page_path = '{}' BEGIN SELECT RAISE(ABORT,'cache failure'); END;",
                new_key.replace('\'', "''")
            ))
            .unwrap();
        } else {
            conn.execute(
                "INSERT INTO page_params VALUES (?1,'destination edit')",
                [&new_key],
            )
            .unwrap();
        }
        let report = run_at(temp.path(), old, new);
        if fail {
            assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
            assert!(report.converted_cache_pairs.is_empty());
            assert_eq!(adjustment(&conn, &old_key).as_deref(), Some("old edit"));
            assert_eq!(
                adjustment(&conn, &old_logical).as_deref(),
                Some("logical edit")
            );
            assert!(adjustment(&conn, &new_key).is_none());
            assert!(adjustment(&conn, &new_logical).is_none());
        } else {
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert_eq!(
                adjustment(&conn, &new_key).as_deref(),
                Some("destination edit")
            );
            assert!(adjustment(&conn, &old_key).is_none());
            assert_eq!(
                report.converted_cache_pairs.len(),
                1,
                "discarding a colliding source is a real change"
            );
        }
    }
}

#[test]
fn section1886_fs_enumeration_has_stable_relative_paths_and_reports_failure() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("nested")).unwrap();
    for path in [
        "A.RAR",
        "nested/b.cb7",
        "nested/c.LHA",
        "d.CBZ",
        "normal.zip",
        "image.jpg",
        "not.rar.txt",
    ] {
        std::fs::write(temp.path().join(path), b"fixture").unwrap();
    }
    let result = collect_converted_archive_rels(temp.path());
    assert!(!result.incomplete);
    let expected = [
        "A.RAR",
        "d.CBZ",
        "nested/b.cb7",
        "nested/c.LHA",
        "normal.zip",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        result
            .converted_archive_rels
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected
    );
    let failed = collect_converted_archive_rels(&temp.path().join("missing"));
    assert!(failed.incomplete);
    assert!(failed.converted_archive_rels.is_empty());
    assert!(!temp.path().join("archive_cache.db").exists());
}

#[cfg(windows)]
#[test]
fn section1886_fs_enumeration_never_enters_junction_or_junction_root() {
    use std::os::windows::process::CommandExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(root.join("inside.rar"), b"fixture").unwrap();
    std::fs::write(outside.join("outside.rar"), b"fixture").unwrap();
    let link = root.join("junction");
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let enumerated = collect_converted_archive_rels(&root);
    let link_root = collect_converted_archive_rels(&link);
    // Remove only the verified fixture junction, before TempDir recursive cleanup.
    std::fs::remove_dir(&link).unwrap();
    assert!(!enumerated.incomplete);
    assert_eq!(
        enumerated.converted_archive_rels,
        vec![PathBuf::from("inside.rar")]
    );
    assert!(!link_root.incomplete);
    assert!(link_root.converted_archive_rels.is_empty());
    assert!(outside.join("outside.rar").exists());
}

#[cfg(unix)]
#[test]
fn section1886_fs_enumeration_never_enters_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("outside.rar"), b"fixture").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.join("outside.rar"), root.join("linked.rar")).unwrap();
    let enumeration = collect_converted_archive_rels(&root);
    assert!(!enumeration.incomplete);
    assert!(enumeration.converted_archive_rels.is_empty());
}

#[test]
fn section1886_pure_pairs_reject_invalid_journal_relative_paths() {
    let data = Path::new("C:/data");
    let old = Path::new("C:/before");
    let new = Path::new("C:/after");
    let rels = [
        "book.rar",
        "nested/book.7z",
        "../outside.rar",
        "C:/outside.rar",
        "",
        "image.jpg",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    let pairs = converted_archive_cache_pairs(data, old, new, true, &rels);
    assert_eq!(pairs.len(), 2);
    for relative in ["book.rar", "nested/book.7z"] {
        assert!(pairs.contains(&(
            crate::archive_cache::cache_zip_path_for_data_dir(data, &old.join(relative)),
            crate::archive_cache::cache_zip_path_for_data_dir(data, &new.join(relative))
        )));
    }
}

#[test]
fn section1886_journal_roundtrip_defaults_and_legacy_formats() {
    let temp = tempfile::tempdir().unwrap();
    let old = Path::new("C:/section1886/old");
    let new = Path::new("C:/section1886/new");
    let mut job = PathMigrationJob::shell_rename(old.into(), new.into(), true);
    job.converted_archive_rels = vec!["one.rar".into(), "nested/two.7z".into()];
    journal_save(temp.path(), std::slice::from_ref(&job)).unwrap();
    assert_eq!(journal_load(temp.path()).unwrap(), vec![job.clone()]);
    let mut old_json = serde_json::to_value(vec![job.clone()]).unwrap();
    old_json[0]
        .as_object_mut()
        .unwrap()
        .remove("converted_archive_rels");
    let old_jobs = parse_journal_bytes(&serde_json::to_vec(&old_json).unwrap()).unwrap();
    assert!(old_jobs[0].converted_archive_rels.is_empty());
    let tuples = serde_json::to_vec(&vec![(old.to_path_buf(), new.to_path_buf())]).unwrap();
    let tuples = parse_journal_bytes(&tuples).unwrap();
    assert!(tuples[0].converted_archive_rels.is_empty());
    assert!(tuples[0].generic_pending);
    job.generic_pending = false;
    journal_save(temp.path(), std::slice::from_ref(&job)).unwrap();
    let loaded = journal_load(temp.path()).unwrap().remove(0);
    assert!(!loaded.generic_pending);
    assert_eq!(loaded.mappings, job.mappings);
    assert_eq!(loaded.converted_archive_rels, job.converted_archive_rels);
}
