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
        PermissionSnapshot {
            level: AgentLevel::ReadOnly,
            thumbnails: false,
            gps: false,
        },
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
        limit,
        cursor,
        &context(fixture),
    )?
    else {
        panic!("expected folder page")
    };
    Ok(page)
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
    let worker = std::thread::spawn(move || handlers.list_folder(input, None, None, None, &ctx));
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
            path: format!("C:/{}-{index}.png", "猫\\\"".repeat(300)),
            subresource: None,
            name: format!("{index}.png"),
            kind: FolderKind::Image,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    source[0].path = format!("C:/{}.png", "長".repeat(20_000));
    let first = folder_page(source.clone(), true, Some(u32::MAX), None, &args).unwrap();
    assert!(matches!(first.items[0], PageRow::TooLong(_)));
    assert!(!first.items.is_empty());
    assert!(first.items.len() < 200);
    assert!(mcp_content_chars(&first).unwrap() <= MAX_PAGE_CHARS);
    assert!(first.book_composition_applied);
    let second = folder_page(source.clone(), true, None, first.cursor.as_deref(), &args).unwrap();
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
            source.len()
        )
        .unwrap(),
        first.items.len()
    );
    let tiny = (0..201)
        .map(|index| FolderItem {
            path: format!("C:/{index}.png"),
            subresource: None,
            name: index.to_string(),
            kind: FolderKind::Image,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    let capped = folder_page(tiny.clone(), false, Some(200), None, &args).unwrap();
    assert_eq!(folder_limit(Some(1000)).unwrap(), 200);
    assert_eq!(
        folder_page(tiny.clone(), false, Some(1000), None, &args).unwrap(),
        capped
    );
    assert!(capped.items.len() <= 200);
    assert!(mcp_content_chars(&capped).unwrap() <= MAX_PAGE_CHARS);
    assert_eq!(
        folder_page(tiny, false, None, None, &args)
            .unwrap()
            .items
            .len(),
        100
    );
}

#[test]
fn folder_cursor_continues_across_equivalent_physical_path_spellings() {
    let rows_for = |root: &str, separator: &str| {
        ["a.png", "b.png"]
            .into_iter()
            .map(|name| FolderItem {
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
    let first = folder_page(rows_for("C:/Books", "/"), false, Some(1), None, &args).unwrap();
    for (root, separator) in [(r"c:\books", r"\"), ("C:/BOOKS", "/")] {
        let alias_args = serde_json::json!([physical_identity(Path::new(root)), []]);
        assert_eq!(alias_args, args);
        let alias = rows_for(root, separator);
        let expected_path = alias[1].path.clone();
        let next =
            folder_page(alias, false, Some(1), first.cursor.as_deref(), &alias_args).unwrap();
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
            path: "C:/Books/book.zip".into(),
            subresource: Some(member.into()),
            name: member.into(),
            kind: FolderKind::ZipEntry,
            size: None,
            modified: None,
        })
        .collect::<Vec<_>>();
    let args = serde_json::json!([physical_identity(Path::new("C:/Books/book.zip")), []]);
    let first = folder_page(source.clone(), false, Some(1), None, &args).unwrap();
    let mut alias = source.clone();
    for item in &mut alias {
        item.path = r"c:\books\BOOK.ZIP".into();
    }
    let next = folder_page(alias, false, Some(1), first.cursor.as_deref(), &args).unwrap();
    assert_eq!(rows(&next)[0].subresource.as_deref(), Some("pages/B.png"));
    for member in ["pages/a.png", r"pages\A.png"] {
        let mut changed = source.clone();
        changed[0].subresource = Some(member.into());
        assert_eq!(
            folder_page(changed, false, Some(1), first.cursor.as_deref(), &args)
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
            .list_folder(path.to_string_lossy().into_owned(), None, None, None, &ctx)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_persistent_files_unchanged(f._dir.path(), &before);
}
