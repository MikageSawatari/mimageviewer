use super::super::tests::Fixture;
use super::*;
use crate::book_composition::{
    BookComposition, BookCompositionProcessOwner, BookCompositionService, CompositionPageRef,
    PageDomain, ReadingDirection,
};
use crate::book_scope::BookScopeKey;
use std::io::{Cursor, Write};

fn settings(change: impl FnOnce(&mut Settings)) {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        change(&mut settings);
        db.save_full(&settings)
    })
    .unwrap();
}

fn context(fixture: &Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        (
            PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                thumbnails: false,
                gps: false,
                ai_captions: false,
            },
            0,
        ),
        Arc::new(AtomicBool::new(false)),
    )
}

fn list(
    fixture: &Fixture,
    path: &Path,
    prefix: Option<&str>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<FolderPage, AgentError> {
    let Payload::Folder(page) = fixture.handlers.list_folder(
        path.to_string_lossy().into_owned(),
        prefix.map(str::to_owned),
        None,
        None,
        None,
        limit,
        cursor,
        &context(fixture),
    )?
    else {
        panic!("expected folder page")
    };
    Ok(page)
}

fn list_filtered(
    fixture: &Fixture,
    path: &Path,
    filters: FolderFilters,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<FolderPage, AgentError> {
    let Payload::Folder(page) = fixture.handlers.list_folder(
        path.to_string_lossy().into_owned(),
        None,
        filters.min_stars,
        filters.max_stars,
        filters.kinds,
        limit,
        cursor,
        &context(fixture),
    )?
    else {
        panic!("folder page")
    };
    Ok(page)
}

fn info_rating(fixture: &Fixture, row: &FolderItem) -> u8 {
    super::super::item_info::read(
        &ItemRef {
            path: row.path.clone(),
            subresource: row.subresource.clone(),
        },
        context(fixture).granted.0,
        &AtomicBool::new(false),
    )
    .unwrap()
    .rating
}

#[test]
fn physical_ratings_tags_and_filters_match_info_without_writes() {
    let f = Fixture::new();
    let path = f._dir.path().join("photos");
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(path.join("child")).unwrap();
    for name in [
        "a.png", "b.png", "c.png", "d.mp4", "e.mp3", "f.pdf", "g.zip", "h.epub", "i.rar",
    ] {
        std::fs::write(path.join(name), b"fixture").unwrap();
    }
    let no_db = list(&f, &path, None, None, None).unwrap();
    assert!(
        rows(&no_db)
            .iter()
            .all(|row| row.rating == Some(0) && row.tags == Some(vec![]))
    );
    assert!(!crate::rating_db::RatingDb::db_path().exists());
    assert!(!crate::tags_db::TagsDb::db_path().exists());
    let db = crate::rating_db::RatingDb::open().unwrap();
    for (name, stars) in [
        ("child", 4),
        ("a.png", 2),
        ("b.png", 3),
        ("d.mp4", 2),
        ("e.mp3", 1),
        ("f.pdf", 4),
        ("g.zip", 5),
        ("h.epub", 3),
        ("i.rar", 2),
    ] {
        db.set(
            &crate::adjustment_db::normalize_path(&path.join(name)),
            stars,
        )
        .unwrap();
    }
    let mut tags = crate::tags_db::TagsDb::open().unwrap();
    let key = crate::tags_db::item_key_for_path(&path.join("a.png"));
    tags.set_item_tags(&key, ["cat", "photo"], "fixture")
        .unwrap();
    let before = persistent_files(f._dir.path());
    let page = list(&f, &path, None, None, None).unwrap();
    for row in rows(&page) {
        assert_eq!(row.rating, Some(info_rating(&f, row)), "{}", row.name);
    }
    let tagged = rows(&page)
        .into_iter()
        .find(|row| row.name == "a.png")
        .unwrap();
    assert_eq!(tagged.tags, Some(vec!["#cat".into(), "#photo".into()]));
    for (min, max, kinds, expected) in [
        (
            Some(3),
            None,
            None,
            vec!["child", "f.pdf", "g.zip", "h.epub", "b.png"],
        ),
        (None, Some(1), None, vec!["c.png", "e.mp3"]),
        (Some(2), Some(2), None, vec!["i.rar", "a.png", "d.mp4"]),
        (None, Some(0), None, vec!["c.png"]),
        (
            None,
            None,
            Some(vec![FolderKind::Image]),
            vec!["a.png", "b.png", "c.png"],
        ),
        (
            Some(2),
            Some(2),
            Some(vec![FolderKind::Image, FolderKind::Video]),
            vec!["a.png", "d.mp4"],
        ),
        (Some(5), None, Some(vec![FolderKind::Image]), vec![]),
    ] {
        let filtered = list_filtered(
            &f,
            &path,
            FolderFilters::new(min, max, kinds).unwrap(),
            None,
            None,
        )
        .unwrap();
        // Membership is independent of the grid's category placement.
        let mut names = rows(&filtered)
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>();
        let mut expected = expected;
        names.sort_unstable();
        expected.sort_unstable();
        assert_eq!(names, expected);
        assert!(!filtered.truncated);
        assert!(filtered.cursor.is_none());
    }
    assert_persistent_files_unchanged(f._dir.path(), &before);
}

#[test]
fn folder_filter_validation_and_unratable_rows() {
    let f = Fixture::new();
    for (min, max, kinds) in [
        (Some(6), None, None),
        (None, Some(6), None),
        (Some(3), Some(2), None),
        (None, None, Some(vec![])),
        (None, None, Some(vec![FolderKind::Image; 2])),
    ] {
        let error = f
            .handlers
            .list_folder(
                "invalid path".into(),
                None,
                min,
                max,
                kinds,
                None,
                None,
                &context(&f),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
    }
    let item = FolderItem {
        path: "C:/fixture".into(),
        subresource: None,
        name: "fixture".into(),
        kind: FolderKind::Folder,
        size: None,
        modified: None,
        rating: None,
        tags: Some(vec![]),
    };
    assert!(FolderFilters::default().matches(&item));
    assert!(
        !FolderFilters::new(Some(0), None, None)
            .unwrap()
            .matches(&item)
    );
    assert!(
        !FolderFilters::new(None, Some(5), None)
            .unwrap()
            .matches(&item)
    );
}

#[test]
fn filtered_cursor_tracks_base_position_across_rating_changes_and_normalizes_kinds() {
    let f = Fixture::new();
    let path = f._dir.path().join("paged-ratings");
    std::fs::create_dir(&path).unwrap();
    let db = crate::rating_db::RatingDb::open().unwrap();
    for (name, stars) in [
        ("a.png", 1),
        ("b.png", 2),
        ("c.png", 1),
        ("d.png", 2),
        ("e.png", 2),
        ("f.png", 1),
    ] {
        std::fs::write(path.join(name), b"image").unwrap();
        db.set(
            &crate::adjustment_db::normalize_path(&path.join(name)),
            stars,
        )
        .unwrap();
    }
    let filter = || {
        FolderFilters::new(
            Some(2),
            Some(2),
            Some(vec![FolderKind::Video, FolderKind::Image]),
        )
        .unwrap()
    };
    let first = list_filtered(&f, &path, filter(), Some(1), None).unwrap();
    assert_eq!(rows(&first)[0].name, "b.png");
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("a.png")),
        2,
    )
    .unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("b.png")),
        4,
    )
    .unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("c.png")),
        2,
    )
    .unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("d.png")),
        4,
    )
    .unwrap();
    let normalized = FolderFilters::new(
        Some(2),
        Some(2),
        Some(vec![FolderKind::Image, FolderKind::Video]),
    )
    .unwrap();
    let second = list_filtered(&f, &path, normalized, Some(1), first.cursor.clone()).unwrap();
    assert_eq!(rows(&second)[0].name, "c.png");
    let third = list_filtered(&f, &path, filter(), Some(1), second.cursor).unwrap();
    assert_eq!(rows(&third)[0].name, "e.png");
    assert!(!third.truncated);
    assert!(third.cursor.is_none());
    for changed in [
        FolderFilters::default(),
        FolderFilters::new(Some(1), Some(2), None).unwrap(),
        FolderFilters::new(Some(2), Some(2), None).unwrap(),
    ] {
        assert_eq!(
            list_filtered(&f, &path, changed, Some(1), first.cursor.clone())
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
    // No remaining matches is a successful empty final page.
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("c.png")),
        4,
    )
    .unwrap();
    db.set(
        &crate::adjustment_db::normalize_path(&path.join("e.png")),
        4,
    )
    .unwrap();
    let empty = list_filtered(&f, &path, filter(), Some(1), first.cursor).unwrap();
    assert!(empty.items.is_empty());
    assert!(!empty.truncated);
    assert!(empty.cursor.is_none());
}

#[test]
fn rating_order_changes_invalidate_filtered_cursors_in_both_directions() {
    let f = Fixture::new();
    let path = f._dir.path().join("ordered-ratings");
    std::fs::create_dir(&path).unwrap();
    let db = crate::rating_db::RatingDb::open().unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        std::fs::write(path.join(name), b"image").unwrap();
    }
    for order in [
        crate::settings::SortOrder::RatingAsc,
        crate::settings::SortOrder::RatingDesc,
    ] {
        settings(|s| s.sort_order = order);
        for (name, stars) in [("a.png", 2), ("b.png", 3), ("c.png", 4)] {
            db.set(
                &crate::adjustment_db::normalize_path(&path.join(name)),
                stars,
            )
            .unwrap();
        }
        let filter = || FolderFilters::new(Some(1), None, None).unwrap();
        let first = list_filtered(&f, &path, filter(), Some(1), None).unwrap();
        db.set(
            &crate::adjustment_db::normalize_path(&path.join("a.png")),
            5,
        )
        .unwrap();
        let error = list_filtered(&f, &path, filter(), Some(1), first.cursor).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert_eq!(
            error.message,
            "一覧が変わりました。最初から取り直してください"
        );
    }
}

fn cp932_zip(placeholder: &str, actual: &str, content: &[u8]) -> (Vec<u8>, String) {
    cp932_names(zip_bytes(&[(placeholder, content)]), placeholder, actual)
}

fn cp932_names(mut bytes: Vec<u8>, placeholder: &str, actual: &str) -> (Vec<u8>, String) {
    let encoded = encoding_rs::SHIFT_JIS.encode(actual).0;
    assert_eq!(encoded.len(), placeholder.len());
    let offsets = bytes
        .windows(placeholder.len())
        .enumerate()
        .filter_map(|(offset, value)| (value == placeholder.as_bytes()).then_some(offset))
        .collect::<Vec<_>>();
    for offset in offsets {
        bytes[offset..offset + placeholder.len()].copy_from_slice(&encoded);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let legacy = archive.by_index_raw(0).unwrap().name().replace('\\', "/");
    (bytes, legacy)
}

#[test]
fn zip_images_read_legacy_ratings_including_nested_ancestry_and_current_zero_wins() {
    let f = Fixture::new();
    let (_service, owner) = composition(&f);
    let (leaf, legacy_leaf) = cp932_zip("xxxx.png", "日本.png", b"image");
    let (nested, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &leaf);
    let db = crate::rating_db::RatingDb::open().unwrap();
    for (filename, bytes, current, legacy) in [
        ("direct.zip", leaf, "日本.png", legacy_leaf.clone()),
        (
            "nested.zip",
            nested,
            "日本.zip/日本.png",
            format!("{legacy_outer}/{legacy_leaf}"),
        ),
    ] {
        let path = f._dir.path().join(filename);
        std::fs::write(&path, bytes).unwrap();
        let old_key = crate::adjustment_db::zip_entry_key(&path, &legacy);
        let current_key = crate::adjustment_db::zip_entry_key(&path, current);
        db.set(&old_key, 5).unwrap();
        let before = persistent_files(f._dir.path());
        let page = list(&f, &path, None, None, None).unwrap();
        let row = rows(&page)[0];
        assert_eq!(row.subresource.as_deref(), Some(current));
        assert_eq!(row.rating, Some(5));
        assert_eq!(row.rating, Some(info_rating(&f, row)));
        assert_eq!(row.tags, None);
        assert_persistent_files_unchanged(f._dir.path(), &before);
        // Explicit zero is a stored row, not an absent current key.
        rusqlite::Connection::open(crate::rating_db::RatingDb::db_path())
            .unwrap()
            .execute(
                "INSERT INTO ratings (path, stars) VALUES (?1, 0)",
                [&current_key],
            )
            .unwrap();
        let before = persistent_files(f._dir.path());
        let page = list(&f, &path, None, None, None).unwrap();
        let row = rows(&page)[0];
        assert_eq!(row.rating, Some(0));
        assert_eq!(row.rating, Some(info_rating(&f, row)));
        assert_eq!(db.get(&old_key), 5);
        assert_persistent_files_unchanged(f._dir.path(), &before);
    }
    owner.shutdown_and_join();
}

#[test]
fn zip_directory_legacy_only_rating_is_a_documented_grid_limitation() {
    let f = Fixture::new();
    let (_service, owner) = composition(&f);
    // Two levels prevent the grid's redundant-root collapse from hiding the directory.
    let (bytes, legacy) = cp932_names(
        zip_bytes(&[("xxxx/page.png", b"image"), ("cover.png", b"image")]),
        "xxxx/page.png",
        "日本/page.png",
    );
    let path = f._dir.path().join("folders.zip");
    std::fs::write(&path, bytes).unwrap();
    let db = crate::rating_db::RatingDb::open().unwrap();
    let legacy_prefix = legacy.strip_suffix("page.png").unwrap();
    let old_key = super::super::metadata_write::zip_directory_rating_key(&path, legacy_prefix);
    let current_key = super::super::metadata_write::zip_directory_rating_key(&path, "日本/");
    db.set(&old_key, 5).unwrap();
    let before = persistent_files(f._dir.path());
    let page = list(&f, &path, None, None, None).unwrap();
    let row = rows(&page)
        .into_iter()
        .find(|row| row.kind == FolderKind::ZipFolder)
        .unwrap();
    assert_eq!(row.rating, Some(0));
    assert_eq!(info_rating(&f, row), 5);
    assert_persistent_files_unchanged(f._dir.path(), &before);
    rusqlite::Connection::open(crate::rating_db::RatingDb::db_path())
        .unwrap()
        .execute(
            "INSERT INTO ratings (path, stars) VALUES (?1, 0)",
            [&current_key],
        )
        .unwrap();
    let page = list(&f, &path, None, None, None).unwrap();
    let row = rows(&page)
        .into_iter()
        .find(|row| row.kind == FolderKind::ZipFolder)
        .unwrap();
    assert_eq!(row.rating, Some(info_rating(&f, row)));
    owner.shutdown_and_join();
}

#[test]
fn filtered_page_budget_and_tag_markers_keep_sparse_base_offsets() {
    let filters = FolderFilters::new(Some(2), Some(2), None).unwrap();
    let args = serde_json::json!(["fixture", filters]);
    let source = (0..220)
        .map(|index| FolderItem {
            path: format!("C:/photos/{index}.png"),
            subresource: None,
            name: index.to_string(),
            kind: FolderKind::Image,
            size: None,
            modified: None,
            rating: Some(if index % 3 == 1 { 2 } else { 0 }),
            tags: Some(vec![]),
        })
        .collect::<Vec<_>>();
    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = super::folder_page(
            source.clone(),
            false,
            Some(200),
            cursor.as_deref(),
            &args,
            0,
            &filters,
            |items| {
                // Only matches enter the single bulk tag read. Nonmatches must never
                // gain a too_long marker or consume page positions.
                assert!(items.iter().all(|item| item.rating == Some(2)));
                for item in items {
                    item.tags = Some(vec!["x".repeat(if item.name == "1" {
                        MAX_ITEM_CHARS
                    } else {
                        1500
                    })]);
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
        if pages == 0 {
            assert!(matches!(page.items[0], PageRow::TooLong(_)));
        }
        for row in &page.items {
            match row {
                PageRow::TooLong(_) => seen.push(1),
                PageRow::Value(row) => seen.push(row.name.parse::<usize>().unwrap()),
            }
        }
        pages += 1;
        if !page.truncated {
            assert!(page.cursor.is_none());
            break;
        }
        let next = page.cursor.as_deref().unwrap();
        let identity = source
            .iter()
            .map(|item| (physical_identity(Path::new(&item.path)), &item.subresource))
            .collect::<Vec<_>>();
        assert_eq!(
            folder_offset(
                Some(next),
                &wire_digest(&args).unwrap(),
                &wire_digest(&identity).unwrap(),
                source.len(),
                0
            )
            .unwrap(),
            seen.last().unwrap() + 1
        );
        cursor = page.cursor;
    }
    assert!(pages > 1);
    assert_eq!(
        seen,
        (0..220).filter(|index| index % 3 == 1).collect::<Vec<_>>()
    );
}

#[test]
fn physical_tag_overflow_returns_too_long_row_and_continues() {
    let f = Fixture::new();
    let path = f._dir.path().join("tag-overflow");
    std::fs::create_dir(&path).unwrap();
    for name in ["a.png", "b.png"] {
        std::fs::write(path.join(name), b"image").unwrap();
    }
    let mut db = crate::tags_db::TagsDb::open().unwrap();
    let tags = (0..300)
        .map(|index| format!("{index:03}{}", "x".repeat(50)))
        .collect::<Vec<_>>();
    db.set_item_tags(
        &crate::tags_db::item_key_for_path(&path.join("a.png")),
        &tags,
        "fixture",
    )
    .unwrap();
    let first = list(&f, &path, None, Some(1), None).unwrap();
    assert!(matches!(first.items[0], PageRow::TooLong(_)));
    let second = list(&f, &path, None, Some(1), first.cursor).unwrap();
    assert_eq!(rows(&second)[0].name, "b.png");
    assert_eq!(rows(&second)[0].tags, Some(vec![]));
    assert!(!second.truncated);
}

fn folder_page(
    items: Vec<FolderItem>,
    applied: bool,
    limit: Option<u32>,
    cursor: Option<&str>,
    args: &serde_json::Value,
    permission_epoch: u64,
) -> Result<FolderPage, AgentError> {
    super::folder_page(
        items,
        applied,
        limit,
        cursor,
        args,
        permission_epoch,
        &FolderFilters::default(),
        |_| Ok(()),
    )
}

fn rows(page: &FolderPage) -> Vec<&FolderItem> {
    page.items
        .iter()
        .map(|row| match row {
            PageRow::Value(item) => item,
            PageRow::TooLong(_) => panic!("small fixture row"),
        })
        .collect()
}

fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn composition(fixture: &Fixture) -> (BookCompositionService, BookCompositionProcessOwner) {
    let (service, owner) = BookCompositionService::spawn_at(crate::data_dir::get(), None).unwrap();
    // A FIFO read proves the initial snapshot has actually been published.
    service
        .prepare(
            BookScopeKey::document(fixture._dir.path().join("ready.pdf")).unwrap(),
            PageDomain::Document,
        )
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .status
        .unwrap();
    assert!(service.snapshot().status.is_ok());
    fixture
        .handlers
        .readers
        .install_book_composition(service.clone());
    (service, owner)
}

fn save(
    service: &BookCompositionService,
    scope: BookScopeKey,
    domain: PageDomain,
    record: BookComposition,
) {
    service
        .save(scope, domain, record)
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .status
        .unwrap();
}

fn seed_pdf(path: &Path, count: u32) -> crate::catalog::CatalogDb {
    std::fs::write(path, b"catalog-only PDF fixture").unwrap();
    let metadata = std::fs::metadata(path).unwrap();
    let db = crate::catalog::CatalogDb::open(
        &crate::catalog::default_cache_dir(),
        path.parent().unwrap(),
    )
    .unwrap();
    db.set_pdf_meta(
        path.file_name().unwrap().to_str().unwrap(),
        crate::ui_helpers::mtime_secs(&metadata),
        metadata.len() as i64,
        count,
        false,
    )
    .unwrap();
    db
}

fn persistent_files(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(persistent_files(&entry.path()));
        } else if !entry.file_name().to_string_lossy().ends_with("-shm") {
            files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
        }
    }
    files
}

fn assert_persistent_files_unchanged(
    root: &Path,
    before: &std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
) {
    let after = persistent_files(root);
    let paths = before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>();
    let changed = paths
        .into_iter()
        .filter_map(|path| {
            (before.get(path) != after.get(path)).then(|| {
                (
                    path,
                    before.get(path).map(Vec::len),
                    after.get(path).map(Vec::len),
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        changed.is_empty(),
        "persistent file bytes changed (path, before size, after size): {changed:?}"
    );
}

#[test]
fn physical_listing_matches_grid_visibility_duplicates_and_kinds_without_writes() {
    let f = Fixture::new();
    let folder = f._dir.path().join("library");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(folder.join("child")).unwrap();
    for name in [
        "image.jpg",
        "image.png",
        "movie.mp4",
        "movie.png",
        "song.mp3",
        "comic.zip",
        "comic.rar",
        "doc.pdf",
        "doc.epub",
        "other.epub",
        "notes.txt",
    ] {
        std::fs::write(folder.join(name), b"fixture").unwrap();
    }
    settings(|s| {
        s.skip_duplicate_images = true;
        s.image_ext_priority = vec!["png".into(), "jpg".into()];
        s.skip_image_if_video_exists = true;
        s.skip_archive_if_zip_exists = true;
        s.skip_epub_if_pdf_exists = true;
        s.file_type_visibility
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Audio);
    });
    let before = persistent_files(f._dir.path());
    let page = list(&f, &folder, None, None, None).unwrap();
    assert!(!page.book_composition_applied);
    let actual = rows(&page);
    assert_eq!(
        actual
            .iter()
            .map(|item| (item.name.as_str(), item.kind))
            .collect::<Vec<_>>(),
        vec![
            ("child", FolderKind::Folder),
            ("comic.zip", FolderKind::Zip),
            ("doc.pdf", FolderKind::Pdf),
            ("other.epub", FolderKind::Epub),
            ("image.png", FolderKind::Image),
            ("movie.mp4", FolderKind::Video),
        ]
    );
    assert!(actual.iter().all(|item| item.modified.is_some()));
    assert_eq!(actual[0].size, None);
    assert_persistent_files_unchanged(f._dir.path(), &before);

    // Visibility is applied before duplicate winners; the hidden PNG must not
    // remove the surviving JPG. Every call reloads the persisted settings.
    settings(|s| {
        s.file_type_visibility
            .excluded_extensions
            .insert("png".into());
    });
    let page = list(&f, &folder, None, None, None).unwrap();
    assert!(rows(&page).iter().any(|item| item.name == "image.jpg"));
    assert!(!rows(&page).iter().any(|item| item.name.ends_with(".png")));
}

#[test]
fn favorite_overlay_uses_deepest_stored_ancestor_and_grid_order() {
    let f = Fixture::new();
    let folder = f._dir.path().join("favorite");
    let child = folder.join("child");
    std::fs::create_dir_all(&child).unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        std::fs::write(child.join(name), b"image").unwrap();
    }
    let parent_favorite = crate::settings::FavoriteEntry::new("parent".into(), folder.clone());
    let child_favorite = crate::settings::FavoriteEntry::new("child".into(), child.clone());
    let db = crate::adjustment_db::AdjustmentDb::open().unwrap();
    let mut state = crate::settings::FavoriteViewState::from_settings(&Settings::default());
    state.sort_order = crate::settings::SortOrder::FileNameDesc;
    db.set_favorite_view_state(parent_favorite.id, &state)
        .unwrap();
    settings(|s| {
        s.remember_favorite_view_state = true;
        s.favorites = vec![parent_favorite, child_favorite.clone()];
    });
    let before = persistent_files(f._dir.path());
    let page = list(&f, &child, None, None, None).unwrap();
    assert_eq!(
        rows(&page)
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["c.png", "b.png", "a.png"]
    );
    assert_persistent_files_unchanged(f._dir.path(), &before);
    state.sort_order = crate::settings::SortOrder::FileName;
    db.set_favorite_view_state(child_favorite.id, &state)
        .unwrap();
    let page = list(&f, &child, None, None, None).unwrap();
    assert_eq!(rows(&page)[0].name, "a.png");
    settings(|s| {
        s.remember_favorite_view_state = false;
        s.sort_order = crate::settings::SortOrder::FileNameDesc;
    });
    assert_eq!(
        rows(&list(&f, &child, None, None, None).unwrap())[0].name,
        "c.png"
    );
}

#[test]
fn zip_composition_uses_raw_scope_before_display_collapse_and_keeps_child_placement() {
    let f = Fixture::new();
    let (service, owner) = composition(&f);
    let path = f._dir.path().join("raw.zip");
    std::fs::write(
        &path,
        zip_bytes(&[
            ("cover.jpg", b"image"),
            ("chapter/a.png", b"a"),
            ("chapter/b.png", b"b"),
        ]),
    )
    .unwrap();
    let scope = BookScopeKey::archive(&path, &[]).unwrap();
    let mut record = BookComposition::new(
        ReadingDirection::Rtl,
        vec![CompositionPageRef::ArchiveEntry {
            entry_name: "cover.jpg".into(),
        }],
    );
    record.hidden_pages = record.page_order.clone();
    save(
        &service,
        scope,
        PageDomain::ArchiveLevel { prefix: "".into() },
        record,
    );
    settings(|s| {
        s.file_type_visibility
            .excluded_extensions
            .insert("jpg".into());
    });
    let before = persistent_files(f._dir.path());
    let page = list(&f, &path, None, None, None).unwrap();
    assert!(page.book_composition_applied);
    // Display alone would collapse into chapter and show a/b. A root record
    // preserves the raw root and the child-container cell instead.
    assert_eq!(
        rows(&page)
            .iter()
            .map(|item| (item.subresource.as_deref(), item.kind))
            .collect::<Vec<_>>(),
        vec![(Some("chapter/"), FolderKind::ZipFolder)]
    );
    assert_eq!(rows(&page)[0].size, None);
    assert_persistent_files_unchanged(f._dir.path(), &before);

    let scope = BookScopeKey::archive(&path, &["chapter".into()]).unwrap();
    let mut record = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            CompositionPageRef::ArchiveEntry {
                entry_name: "chapter/b.png".into(),
            },
            CompositionPageRef::ArchiveEntry {
                entry_name: "chapter/a.png".into(),
            },
        ],
    );
    record.hidden_pages.push(record.page_order[1].clone());
    save(
        &service,
        scope,
        PageDomain::ArchiveLevel {
            prefix: "chapter".into(),
        },
        record,
    );
    let page = list(&f, &path, Some("chapter/"), None, None).unwrap();
    assert!(page.book_composition_applied);
    assert_eq!(
        rows(&page)
            .iter()
            .map(|item| item.subresource.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("chapter/b.png")]
    );
    owner.shutdown_and_join();
    // on_exit may close the service's admission before the agent worker finishes.
    assert_eq!(list(&f, &path, Some("chapter/"), None, None).unwrap(), page);
}

#[test]
fn nested_zip_root_collapse_and_explicit_inner_prefix_follow_grid() {
    let f = Fixture::new();
    let (_service, owner) = composition(&f);
    let inner = zip_bytes(&[("wrapper/a.png", b"a"), ("wrapper/b.png", b"b")]);
    let path = f._dir.path().join("outer.zip");
    std::fs::write(&path, zip_bytes(&[("inner.cbz", &inner)])).unwrap();
    let root = list(&f, &path, None, None, None).unwrap();
    assert_eq!(
        rows(&root)
            .iter()
            .map(|item| item.subresource.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("inner.cbz/wrapper/a.png"),
            Some("inner.cbz/wrapper/b.png")
        ]
    );
    assert_eq!(
        root,
        list(&f, &path, Some("inner.cbz/"), None, None).unwrap()
    );
    assert_eq!(
        root,
        list(&f, &path, Some("inner.cbz/wrapper/"), None, None).unwrap()
    );
    owner.shutdown_and_join();
}

#[test]
fn archive_composition_preserves_configured_grid_category_placement() {
    let f = Fixture::new();
    let (service, owner) = composition(&f);
    let path = f._dir.path().join("placement.zip");
    std::fs::write(
        &path,
        zip_bytes(&[("a.png", b"a"), ("b.png", b"b"), ("child/page.png", b"c")]),
    )
    .unwrap();
    use crate::settings::GridItemDisplayKind as Kind;
    settings(|settings| {
        settings.grid_display_order = crate::settings::GridDisplayOrder::from_rows([
            vec![Kind::Image],
            vec![Kind::Folder, Kind::Archive],
            vec![Kind::VideoAudio],
            vec![],
        ]);
    });
    save(
        &service,
        BookScopeKey::archive(&path, &[]).unwrap(),
        PageDomain::ArchiveLevel { prefix: "".into() },
        BookComposition::new(
            ReadingDirection::Rtl,
            ["b.png", "a.png"]
                .into_iter()
                .map(|entry_name| CompositionPageRef::ArchiveEntry {
                    entry_name: entry_name.into(),
                })
                .collect(),
        ),
    );
    let page = list(&f, &path, None, None, None).unwrap();
    assert_eq!(
        rows(&page)
            .iter()
            .map(|item| item.subresource.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("b.png"), Some("a.png"), Some("child/")]
    );
    assert!(page.book_composition_applied);
    owner.shutdown_and_join();
}

#[test]
fn cached_pdf_visibility_and_composition_are_applied_without_writing_catalog() {
    let f = Fixture::new();
    // Normal app startup initializes the shared settings connection and its
    // migration markers before the agent starts accepting requests.
    crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
    let (service, owner) = composition(&f);
    let path = f._dir.path().join("book.pdf");
    // Keep the fixture's writer alive so a read-only reader does not recreate
    // an empty SQLite WAL after the last writer's close removed the sidecars.
    let _catalog_writer = seed_pdf(&path, 4);
    let scope = BookScopeKey::document(&path).unwrap();
    let mut record = BookComposition::new(
        ReadingDirection::Rtl,
        [3, 1, 0, 2]
            .into_iter()
            .map(|page_num| CompositionPageRef::PdfPage { page_num })
            .collect(),
    );
    record
        .hidden_pages
        .push(CompositionPageRef::PdfPage { page_num: 1 });
    save(&service, scope, PageDomain::Document, record);
    let before = persistent_files(f._dir.path());
    let page = list(&f, &path, None, None, None).unwrap();
    assert!(page.book_composition_applied);
    assert_eq!(
        rows(&page)
            .iter()
            .map(|item| item.subresource.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("page_3"), Some("page_0"), Some("page_2")]
    );
    assert!(
        rows(&page)
            .iter()
            .all(|item| item.kind == FolderKind::PdfPage
                && item.size.is_none()
                && item.modified.is_none())
    );
    assert_persistent_files_unchanged(f._dir.path(), &before);
    settings(|s| {
        s.file_type_visibility
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Pdf);
    });
    let page = list(&f, &path, None, None, None).unwrap();
    assert!(page.items.is_empty());
    assert!(page.book_composition_applied);
    owner.shutdown_and_join();
}

#[test]
fn missing_pdf_catalog_uses_anonymous_enumeration_and_does_not_create_it() {
    let f = Fixture::new();
    let (_service, owner) = composition(&f);
    let path = f._dir.path().join("uncached.pdf");
    std::fs::write(&path, b"PDF worker fixture").unwrap();
    let pool = crate::pdf_loader::TestPdfEnumeratePool::new(&path);
    let handlers = f.handlers.clone();
    let ctx = context(&f);
    let input = path.to_string_lossy().into_owned();
    let worker = std::thread::spawn(move || {
        handlers.list_folder(input, None, None, None, None, None, None, &ctx)
    });
    let job = pool.take();
    job.complete(3);
    let Payload::Folder(page) = worker.join().unwrap().unwrap() else {
        panic!("expected PDF rows")
    };
    assert_eq!(page.items.len(), 3);
    assert!(
        !crate::catalog::db_path_for(&crate::catalog::default_cache_dir(), path.parent().unwrap())
            .exists()
    );
    owner.shutdown_and_join();
}

#[test]
fn folder_cursor_detects_whole_identity_change_and_uses_actual_returned_count() {
    let f = Fixture::new();
    let path = f._dir.path().join("paged");
    std::fs::create_dir(&path).unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        std::fs::write(path.join(name), b"image").unwrap();
    }
    let first = list(&f, &path, None, Some(1), None).unwrap();
    assert!(first.truncated);
    let cursor = first.cursor.clone().unwrap();
    let second = list(&f, &path, None, Some(1), Some(cursor.clone())).unwrap();
    assert_eq!(rows(&second)[0].name, "b.png");
    std::fs::remove_file(path.join("c.png")).unwrap();
    // A change entirely past the current page still invalidates the cursor.
    let failure = list(&f, &path, None, Some(1), Some(cursor)).unwrap_err();
    assert_eq!(
        failure.message,
        "一覧が変わりました。最初から取り直してください"
    );
    assert_eq!(
        list(&f, &path, None, Some(0), None).unwrap_err().code,
        ErrorCode::BadRequest
    );
    for cursor in ["g0", "0", "", "00"] {
        assert_eq!(
            list(&f, &path, None, Some(1), Some(cursor.into()))
                .unwrap_err()
                .message,
            "最初からやり直してください"
        );
    }
}

#[test]
fn folder_page_character_budget_counts_composition_flag_and_advances_oversized_rows() {
    let args = serde_json::json!(["C:/fixture"]);
    let mut source = (0..250)
        .map(|index| FolderItem {
            rating: Some(0),
            tags: None,
            path: format!("C:/{}-{index}.png", "猫\\\"".repeat(300)),
            subresource: None,
            name: format!("{index}.png"),
            kind: FolderKind::Image,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    source[0].path = format!("C:/{}.png", "長".repeat(20_000));
    let first = folder_page(source.clone(), true, Some(u32::MAX), None, &args, 0).unwrap();
    assert!(matches!(first.items[0], PageRow::TooLong(_)));
    assert!(!first.items.is_empty());
    assert!(first.items.len() < 200);
    assert!(mcp_content_chars(&first).unwrap() <= MAX_PAGE_CHARS);
    assert!(first.book_composition_applied);
    let second = folder_page(
        source.clone(),
        true,
        None,
        first.cursor.as_deref(),
        &args,
        0,
    )
    .unwrap();
    let PageRow::Value(item) = &second.items[0] else {
        panic!("ordinary row")
    };
    assert_eq!(item.name, format!("{}.png", first.items.len()));
    let identity = source
        .iter()
        .map(|item| (physical_identity(Path::new(&item.path)), &item.subresource))
        .collect::<Vec<_>>();
    assert!(first.cursor.as_ref().unwrap().len() < 8192);
    assert_eq!(
        folder_offset(
            first.cursor.as_deref(),
            &wire_digest(&args).unwrap(),
            &wire_digest(&identity).unwrap(),
            source.len(),
            0
        )
        .unwrap(),
        first.items.len()
    );
    let tiny = (0..201)
        .map(|index| FolderItem {
            rating: Some(0),
            tags: None,
            path: format!("C:/{index}.png"),
            subresource: None,
            name: index.to_string(),
            kind: FolderKind::Image,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    let capped = folder_page(tiny.clone(), false, Some(200), None, &args, 0).unwrap();
    assert_eq!(folder_limit(Some(1000)).unwrap(), 200);
    assert_eq!(
        folder_page(tiny.clone(), false, Some(1000), None, &args, 0).unwrap(),
        capped
    );
    assert!(capped.items.len() <= 200);
    assert!(mcp_content_chars(&capped).unwrap() <= MAX_PAGE_CHARS);
    assert_eq!(
        folder_page(tiny, false, None, None, &args, 0)
            .unwrap()
            .items
            .len(),
        100
    );
}

#[test]
fn folder_cursor_version_three_is_rejected_and_epoch_precedes_listing_change() {
    let args = "argument-digest";
    let listing = "listing-digest";
    let cursor = folder_cursor(args, listing, 1, 7);
    assert_eq!(
        folder_offset(Some(&cursor), args, listing, 2, 7).unwrap(),
        1
    );
    assert_eq!(
        folder_offset(Some(&cursor), args, "changed-listing", 2, 8)
            .unwrap_err()
            .code,
        ErrorCode::PermissionChanged
    );
    let legacy = serde_json::json!({"version": 3, "tool": "list_folder", "args_digest": args, "listing_digest": listing, "returned": 1});
    let legacy = serde_json::to_vec(&legacy)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        folder_offset(Some(&legacy), args, listing, 2, 0)
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
}

#[test]
fn folder_cursor_continues_across_equivalent_physical_path_spellings() {
    let rows_for = |root: &str, separator: &str| {
        ["a.png", "b.png"]
            .into_iter()
            .map(|name| FolderItem {
                rating: Some(0),
                tags: None,
                path: format!("{root}{separator}{name}"),
                subresource: None,
                name: name.into(),
                kind: FolderKind::Image,
                size: None,
                modified: None,
            })
            .collect::<Vec<_>>()
    };
    let args = serde_json::json!([physical_identity(Path::new("C:/Books")), []]);
    let first = folder_page(rows_for("C:/Books", "/"), false, Some(1), None, &args, 0).unwrap();
    for (root, separator) in [(r"c:\books", r"\"), ("C:/BOOKS", "/")] {
        let alias_args = serde_json::json!([physical_identity(Path::new(root)), []]);
        assert_eq!(alias_args, args);
        let alias = rows_for(root, separator);
        let expected_path = alias[1].path.clone();
        let next = folder_page(
            alias,
            false,
            Some(1),
            first.cursor.as_deref(),
            &alias_args,
            0,
        )
        .unwrap();
        assert_eq!(rows(&next)[0].name, "b.png");
        // Only the cursor identity is normalized; emitted paths retain spelling.
        assert_eq!(rows(&next)[0].path, expected_path);
        assert!(!next.truncated);
        assert!(next.cursor.is_none());
    }
}

#[test]
fn folder_cursor_keeps_zip_subresource_identity_spelling_exact() {
    let source = ["pages/A.png", "pages/B.png"]
        .into_iter()
        .map(|member| FolderItem {
            rating: Some(0),
            tags: None,
            path: "C:/Books/book.zip".into(),
            subresource: Some(member.into()),
            name: member.into(),
            kind: FolderKind::ZipEntry,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    let args = serde_json::json!([physical_identity(Path::new("C:/Books/book.zip")), []]);
    let first = folder_page(source.clone(), false, Some(1), None, &args, 0).unwrap();
    let mut alias = source.clone();
    for item in &mut alias {
        item.path = r"c:\books\BOOK.ZIP".into();
    }
    let next = folder_page(alias, false, Some(1), first.cursor.as_deref(), &args, 0).unwrap();
    assert_eq!(rows(&next)[0].subresource.as_deref(), Some("pages/B.png"));
    for member in ["pages/a.png", r"pages\A.png"] {
        let mut changed = source.clone();
        changed[0].subresource = Some(member.into());
        assert_eq!(
            folder_page(changed, false, Some(1), first.cursor.as_deref(), &args, 0)
                .unwrap_err()
                .message,
            "一覧が変わりました。最初から取り直してください"
        );
    }
}

#[test]
fn missing_or_failed_composition_snapshot_is_not_automatic_and_unsupported_inputs_stay_read_only() {
    let f = Fixture::new();
    let path = f._dir.path().join("book.zip");
    std::fs::write(&path, zip_bytes(&[("page.png", b"image")])).unwrap();
    assert_eq!(
        list(&f, &path, None, None, None).unwrap_err().code,
        ErrorCode::NotReady
    );
    std::fs::write(
        f._dir.path().join("book_composition.db"),
        b"invalid SQLite fixture",
    )
    .unwrap();
    let (service, owner) = BookCompositionService::spawn_at(crate::data_dir::get(), None).unwrap();
    let result = service
        .prepare(
            BookScopeKey::document(f._dir.path().join("ready.pdf")).unwrap(),
            PageDomain::Document,
        )
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(result.status.is_err());
    f.handlers.readers.install_book_composition(service);
    assert_eq!(
        list(&f, &path, None, None, None).unwrap_err().code,
        ErrorCode::NotReady
    );
    owner.shutdown_and_join();
    for extension in ["rar", "7z", "epub"] {
        let path = f._dir.path().join(format!("unsupported.{extension}"));
        std::fs::write(&path, b"source").unwrap();
        assert_eq!(
            list(&f, &path, None, None, None).unwrap_err().message,
            "この形式の中は一覧できません。mIV で開いてください"
        );
    }
    for invalid in [
        "../", "a//", "/root/", "C:/foo/", "folder", "a/../b/", "a\0b/",
    ] {
        assert!(folder_prefix(Some(invalid)).is_err(), "{invalid:?}");
    }
}

#[test]
fn pre_cancelled_folder_scan_and_handler_never_publish_partial_rows_or_create_files() {
    let f = Fixture::new();
    let path = f._dir.path().join("empty");
    std::fs::create_dir(&path).unwrap();
    let cancel = AtomicBool::new(true);
    let error = crate::app::folder_scan::scan_directory_with_settings_cancel(
        &path,
        &Settings::default(),
        Some(&cancel),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    let ctx = context(&f);
    ctx.cancel.store(true, Ordering::Release);
    let before = persistent_files(f._dir.path());
    assert_eq!(
        f.handlers
            .list_folder(
                path.to_string_lossy().into_owned(),
                None,
                None,
                None,
                None,
                None,
                None,
                &ctx
            )
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_persistent_files_unchanged(f._dir.path(), &before);
}
