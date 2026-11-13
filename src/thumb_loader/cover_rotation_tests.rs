use super::*;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::RwLock;

type Memory = RwLock<HashMap<String, crate::catalog::CacheEntry>>;

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(12, 8)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn write_zip(path: &Path, entries: &[&str]) {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for name in entries {
        archive
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&png()).unwrap();
    }
    archive.finish().unwrap();
}

fn webp() -> Vec<u8> {
    crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(12, 8), 64, 80.0)
        .unwrap()
        .0
}

fn request(path: &Path, key: &str) -> LoadRequest {
    let metadata = std::fs::metadata(path).unwrap();
    LoadRequest {
        path: path.to_owned(),
        cache_key_override: Some(key.into()),
        mtime: crate::ui_helpers::mtime_secs(&metadata),
        file_size: if metadata.is_dir() {
            0
        } else {
            metadata.len() as i64
        },
        books_root: Some(path.parent().unwrap().join("unused-books")),
        ..Default::default()
    }
}

fn run(
    req: &mut LoadRequest,
    memory: &Memory,
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
    pins: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) -> Vec<ThumbMsg> {
    run_with_preview(req, memory, catalog, pins, None)
}

fn run_with_preview(
    req: &mut LoadRequest,
    memory: &Memory,
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
    pins: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
    preview: Option<&Arc<crate::edit_preview_cache::EditPreviewCacheDb>>,
) -> Vec<ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    let decision = CacheDecision {
        policy: crate::settings::CachePolicy::Off,
        threshold_ms: 0,
        size_threshold: 0,
        webp_always: true,
        zip_always: true,
        pdf_always: true,
        cache_book_cover_only: false,
    };
    process_load_request(
        req,
        memory,
        &tx,
        catalog,
        64,
        80,
        64,
        decision,
        &done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        pins,
        preview,
        None,
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    rx.try_iter().collect()
}

fn assert_cover(messages: &[ThumbMsg], expected: &CoverIdentity) {
    let pixels: Vec<_> = messages.iter().filter_map(ThumbMsg::pixels).collect();
    assert!(!pixels.is_empty(), "worker returned no pixels");
    for pixels in pixels {
        assert_eq!(&pixels.cover, expected);
    }
}

fn seed_legacy(
    catalog: &crate::catalog::CatalogDb,
    req: &LoadRequest,
    memory: &Memory,
) -> crate::catalog::CacheEntry {
    let key = cache_key_for_request(req).unwrap();
    catalog
        .save_thumb_bytes(
            &key,
            req.mtime,
            req.file_size,
            Some((12, 8)),
            &webp(),
            &CoverIdentity::Legacy,
        )
        .unwrap();
    let row = catalog.load_one(&key).unwrap().unwrap();
    memory
        .write()
        .unwrap()
        .insert(key.into_owned(), row.clone());
    row
}

#[test]
fn cover_identity_zip_first_and_resolved_pin_follow_the_decoded_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("book.zip");
    // Storage order deliberately differs from the ordinary page-list order.
    write_zip(&path, &["02.png", "01.png"]);
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&tmp.path().join("cache"), tmp.path()).unwrap());
    let mut first = request(&path, "zipthumb:book.zip");
    first.force_cache = true;
    let memory = Memory::default();
    let messages = run(&mut first, &memory, Some(&catalog), None);
    let expected = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&path, "02.png"));
    assert_cover(&messages, &expected);
    assert_eq!(
        catalog
            .load_one("zipthumb:book.zip")
            .unwrap()
            .unwrap()
            .cover,
        expected
    );

    let pins =
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&tmp.path().join("pins.db")).unwrap();
    let source = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "01.png".into(),
    };
    pins.set(&path, &source).unwrap();
    let target = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
        &path,
        &source,
        |path| pins.lookup(path),
        3,
    )
    .unwrap();
    let key = target.pinned_cache_key(
        "zipthumb:book.zip",
        crate::folder_thumb_pins::PinKeyMode::Exact,
        None,
    );
    let mut pinned = request(&target.abs_path, &key);
    pinned.zip_entry = target.zip_entry;
    pinned.force_cache = true;
    let messages = run(&mut pinned, &memory, Some(&catalog), Some(&pins));
    let expected = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&path, "01.png"));
    assert_cover(&messages, &expected);
    assert_eq!(catalog.load_one(&key).unwrap().unwrap().cover, expected);
}

#[test]
fn cover_identity_pdf_epub_locations_keep_the_logical_document_page_key() {
    let root = Path::new("C:/cover-identity");
    let books = root.join("books");
    for name in ["book.pdf", "book.epub"] {
        let path = root.join(name);
        for page in [0, 3] {
            assert_eq!(
                cover_identity_for_location(
                    &ReadLocation::PdfPage {
                        pdf: path.clone(),
                        page
                    },
                    Some(&books)
                ),
                CoverIdentity::Page(crate::edit_source::page_key_for_pdf(&path, page)),
            );
        }
    }
    assert_eq!(
        cover_identity_for_location(
            &ReadLocation::File(books.join("bound-book/page.png")),
            Some(&books)
        ),
        CoverIdentity::Unrotatable,
    );
}

#[test]
fn cover_identity_direct_rar_and_converted_zip_use_the_actual_archive_read() {
    let tmp = tempfile::tempdir().unwrap();
    let rar = tmp.path().join("original.rar");
    std::fs::write(&rar, crate::rar_loader::direct_read_test_fixture_bytes()).unwrap();
    let converted = tmp.path().join("converted.zip");
    write_zip(&converted, &["normalized/cover.png"]);
    for (index, (path, entry)) in [(&rar, "page.png"), (&converted, "normalized/cover.png")]
        .into_iter()
        .enumerate()
    {
        let key = format!("archivethumb:source.rar:{index}");
        let mut req = request(path, &key);
        req.resolve_override = Some(ResolveStrategy::ZipFirstImage);
        req.force_cache = true;
        let resolved = resolve_read_after_cache_miss(&req, None, None);
        assert!(
            matches!(&resolved.location, ReadLocation::ArchiveEntry { archive, entry: actual }
            if archive == path && actual == entry)
        );
        let expected = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(path, entry));
        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&tmp.path().join(format!("cache-{index}")), tmp.path())
                .unwrap(),
        );
        let messages = run(&mut req, &Memory::default(), Some(&catalog), None);
        assert_cover(&messages, &expected);
        assert_eq!(catalog.load_one(&key).unwrap().unwrap().cover, expected);
    }
}

#[test]
fn cover_identity_folder_direct_and_depth_use_the_winning_image() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let tmp = tempfile::tempdir().unwrap();
    for nested in [false, true] {
        let folder = tmp.path().join(format!("folder-{nested}"));
        let leaf = if nested {
            folder.join("child/deep")
        } else {
            folder.clone()
        };
        std::fs::create_dir_all(&leaf).unwrap();
        let page = leaf.join("cover.png");
        std::fs::write(&page, png()).unwrap();
        let key = format!("folderthumb:folder-{nested}");
        let mut req = request(&folder, &key);
        req.folder_thumb_sort = Some(crate::settings::SortOrder::Numeric);
        req.folder_thumb_depth = 3;
        req.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::AutoSelected);
        req.force_cache = true;
        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&tmp.path().join("cache"), tmp.path()).unwrap(),
        );
        let messages = run(&mut req, &Memory::default(), Some(&catalog), None);
        let expected = CoverIdentity::Page(crate::adjustment_db::normalize_path(&page));
        assert_cover(&messages, &expected);
        assert_eq!(catalog.load_one(&key).unwrap().unwrap().cover, expected);
    }
}

#[test]
fn cover_identity_child_cache_preserves_pixels_identity_and_resolves_legacy_pin() {
    let data = crate::data_dir::TestDataDirGuard::new();
    let folder = data.path().join("library");
    std::fs::create_dir_all(&folder).unwrap();
    let archive = folder.join("book.zip");
    write_zip(&archive, &["first.png", "pin.png"]);
    let cache_dir = crate::catalog::default_cache_dir();
    let children = crate::catalog::CatalogDb::open(&cache_dir, &folder).unwrap();
    let parent = Arc::new(crate::catalog::CatalogDb::open(&cache_dir, data.path()).unwrap());
    let metadata = std::fs::metadata(&archive).unwrap();
    let stamp = crate::catalog::file_stamp(&metadata);
    let page = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&archive, "first.png"));
    for (index, (child_identity, expected)) in [
        (CoverIdentity::Legacy, CoverIdentity::Unknown),
        (page.clone(), page),
    ]
    .into_iter()
    .enumerate()
    {
        children
            .save_thumb_bytes(
                "zipthumb:book.zip",
                stamp.0,
                stamp.1,
                Some((12, 8)),
                &webp(),
                &child_identity,
            )
            .unwrap();
        let key = format!("folderthumb:child-{index}");
        let mut req = request(&folder, &key);
        req.folder_thumb_depth = 3;
        req.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::AutoSelected);
        req.force_cache = true;
        assert!(matches!(
            resolve_read_after_cache_miss(&req, None, None).location,
            ReadLocation::ChildCached
        ));
        let messages = run(&mut req, &Memory::default(), Some(&parent), None);
        assert_cover(&messages, &expected);
        assert_eq!(parent.load_one(&key).unwrap().unwrap().cover, expected);
    }
    let pins =
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&data.path().join("pins.db")).unwrap();
    let source = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "pin.png".into(),
    };
    pins.set(&archive, &source).unwrap();
    let target = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
        &archive,
        &source,
        |path| pins.lookup(path),
        3,
    )
    .unwrap();
    let child_key = target.pinned_cache_key(
        "zipthumb:book.zip",
        crate::folder_thumb_pins::PinKeyMode::Exact,
        None,
    );
    children
        .save_thumb_bytes(
            &child_key,
            target.mtime,
            target.file_size,
            Some((12, 8)),
            &webp(),
            &CoverIdentity::Legacy,
        )
        .unwrap();
    let mut req = request(&folder, "folderthumb:pinned-child");
    req.folder_thumb_depth = 3;
    req.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::AutoSelected);
    req.force_cache = true;
    assert!(matches!(
        resolve_read_after_cache_miss(&req, Some(&pins), None).location,
        ReadLocation::ChildCached
    ));
    let messages = run(&mut req, &Memory::default(), Some(&parent), Some(&pins));
    let expected = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&archive, "pin.png"));
    assert_cover(&messages, &expected);
    assert_eq!(
        parent
            .load_one("folderthumb:pinned-child")
            .unwrap()
            .unwrap()
            .cover,
        expected
    );
}

#[test]
fn cover_identity_legacy_refresh_replaces_an_existing_row_with_cache_policy_off() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("book.zip");
    write_zip(&path, &["cover.png"]);
    let mut req = request(&path, "zipthumb:book.zip");
    req.refresh_legacy_cover = true;
    assert!(!req.force_cache);
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&tmp.path().join("cache"), tmp.path()).unwrap());
    let memory = Memory::default();
    seed_legacy(&catalog, &req, &memory);
    let before = catalog.revision_state();
    let messages = run(&mut req, &memory, Some(&catalog), None);
    let expected = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&path, "cover.png"));
    assert_cover(&messages, &expected);
    assert_eq!(
        catalog
            .load_one("zipthumb:book.zip")
            .unwrap()
            .unwrap()
            .cover,
        expected
    );
    let after = catalog.revision_state();
    assert_ne!(before, after);
    // A second request uses the rewritten row even though its source has gone offline.
    std::fs::remove_file(&path).unwrap();
    let messages = run(&mut req, &memory, Some(&catalog), None);
    assert_cover(&messages, &expected);
    assert!(
        messages
            .iter()
            .filter_map(ThumbMsg::pixels)
            .all(|pixels| pixels.origin == ThumbLoadOrigin::UpgradeableCache)
    );
    assert_eq!(catalog.revision_state(), after);
}

#[test]
fn cover_identity_unreadable_legacy_refresh_keeps_the_existing_final_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("book.zip");
    write_zip(&path, &["cover.png"]);
    let mut req = request(&path, "zipthumb:book.zip");
    req.refresh_legacy_cover = true;
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&tmp.path().join("cache"), tmp.path()).unwrap());
    let memory = Memory::default();
    let old = seed_legacy(&catalog, &req, &memory);
    let before = catalog.revision_state();
    std::fs::remove_file(&path).unwrap();
    let messages = run(&mut req, &memory, Some(&catalog), None);
    assert_cover(&messages, &CoverIdentity::Legacy);
    assert!(
        messages
            .iter()
            .filter_map(ThumbMsg::pixels)
            .all(|pixels| pixels.origin == ThumbLoadOrigin::FinalCache)
    );
    let row = catalog.load_one("zipthumb:book.zip").unwrap().unwrap();
    assert_eq!(row.jpeg_data, old.jpeg_data);
    assert_eq!(row.cover, CoverIdentity::Legacy);
    assert_eq!(catalog.revision_state(), before);
    assert!(
        req.refresh_legacy_cover,
        "the result belongs to the refresh request"
    );
}

#[test]
fn cover_identity_synthetic_memory_refresh_off_does_not_create_a_parent_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let source_folder = tmp.path().join("library");
    std::fs::create_dir_all(&source_folder).unwrap();
    let source = source_folder.join("book.zip");
    write_zip(&source, &["cover.png"]);
    let cache = tmp.path().join("cache");
    let synthetic = Arc::new(
        crate::catalog::CatalogDb::open(&cache, &PathBuf::from("__search_results__")).unwrap(),
    );
    let mut req = request(&source, "zipthumb:synthetic-book.zip");
    req.refresh_legacy_cover = true;
    req.catalog_target = RequestCatalog::Parent {
        row: ParentRow {
            source: source.clone(),
            folder: source_folder.clone(),
            row_key: "zipthumb:book.zip".into(),
        },
        work: crate::catalog::CatalogWork::capture(&cache),
    };
    let memory = Memory::default();
    seed_legacy(&synthetic, &req, &memory);
    let parent_path = crate::catalog::db_path_for(&cache, &source_folder);
    assert!(!parent_path.exists());
    let messages = run(&mut req, &memory, Some(&synthetic), None);
    assert_cover(
        &messages,
        &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&source, "cover.png")),
    );
    assert!(
        !parent_path.exists(),
        "synthetic memory did not prove a parent row existed"
    );
    assert_eq!(
        synthetic
            .load_one("zipthumb:synthetic-book.zip")
            .unwrap()
            .unwrap()
            .cover,
        CoverIdentity::Legacy
    );
}

#[test]
fn cover_identity_drive_seed_same_bytes_with_changed_identity_sends_terminal_pixels() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    let data = crate::data_dir::TestDataDirGuard::new();
    let root = data.path().join("library");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let page = child.join("page.png");
    std::fs::write(&page, png()).unwrap();
    let pins =
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&data.path().join("pins.db")).unwrap();
    let source = FolderPinSource::File {
        rel: "page.png".into(),
        kind: FileKind::Image,
    };
    pins.set(&child, &source).unwrap();
    let target = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
        &child,
        &source,
        |path| pins.lookup(path),
        3,
    )
    .unwrap();
    let base = folder_thumb_cache_key_for_path(
        &child,
        false,
        crate::settings::SortOrder::Numeric,
        3,
        crate::catalog::FolderThumbProvenance::Seeded,
    )
    .unwrap();
    let key = target.pinned_cache_key(&base, crate::folder_thumb_pins::PinKeyMode::Exact, None);
    let catalog = Arc::new(
        crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &root).unwrap(),
    );
    let expected = CoverIdentity::Page(crate::adjustment_db::normalize_path(&page));
    catalog
        .save_thumb_bytes(
            &key,
            target.mtime,
            target.file_size,
            Some((12, 8)),
            &webp(),
            &expected,
        )
        .unwrap();
    let current = catalog.load_one(&key).unwrap().unwrap();
    for same_identity in [false, true] {
        let mut seed = current.clone();
        if !same_identity {
            seed.cover = CoverIdentity::Legacy;
        }
        let memory = Memory::from(HashMap::from([("drive-preview:old".into(), seed)]));
        let mut req = request(&root, "drive-result");
        req.source_policy = LoadSourcePolicy::CacheOnly;
        req.folder_thumb_sort = Some(crate::settings::SortOrder::Numeric);
        req.folder_thumb_depth = 3;
        req.pinned_only = Some(PinnedOnlyRequest {
            cache_key_prefix: "drive-preview:".into(),
            seed_proof: None,
            source: FolderPinSource::File {
                rel: "child".into(),
                kind: FileKind::Folder,
            },
        });
        let messages = run(&mut req, &memory, Some(&catalog), Some(&pins));
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::ProvisionalPixels(_)
        ));
        if same_identity {
            assert!(messages[1].is_finalized());
        } else {
            assert_eq!(messages[0].pixels().unwrap().cover, CoverIdentity::Legacy);
            assert!(matches!(messages[1].payload, ThumbMsgPayload::Pixels(_)));
            assert_eq!(messages[1].pixels().unwrap().cover, expected);
            assert_eq!(
                messages[0].pixels().unwrap().image.pixels,
                messages[1].pixels().unwrap().image.pixels
            );
        }
    }
}

#[test]
fn cover_identity_edit_preview_manual_representative_and_compiled_page_use_their_source() {
    let data = crate::data_dir::TestDataDirGuard::new();
    let archive = data.path().join("book.zip");
    write_zip(&archive, &["page.png"]);
    let books = data.path().join("custom-books");
    let book = books.join("bound-book");
    std::fs::create_dir_all(&book).unwrap();
    let page = book.join("page.png");
    std::fs::write(&page, png()).unwrap();
    let previews = crate::edit_preview_cache::EditPreviewCacheService::open().unwrap();
    let db = previews.db();
    for compiled_page in [false, true] {
        let path = if compiled_page { &page } else { &archive };
        let mut req = request(path, "folderthumb:manual-representative");
        req.books_root = Some(books.clone());
        req.zip_entry = (!compiled_page).then(|| "page.png".into());
        req.edit_preview_validate_container = !compiled_page;
        let key = if compiled_page {
            crate::adjustment_db::normalize_path(&page)
        } else {
            crate::adjustment_db::zip_entry_key(&archive, "page.png")
        };
        req.edit_preview_key = Some(key.clone());
        // Virtual page source_size differs from container size, so only container validation
        // can accept the manual representative's saved edited raster.
        let source_size = if compiled_page {
            req.file_size
        } else {
            req.file_size + 100
        };
        previews.save(
            key.clone(),
            req.mtime,
            source_size,
            (!compiled_page).then(|| archive.clone()),
            crate::page_dims::CanonicalPageDimensions::new(12, 8),
            None,
            Arc::new(egui::ColorImage::filled(
                [12, 8],
                egui::Color32::from_rgb(40, 80, 160),
            )),
            None,
            None,
            crate::edit_preview_cache::DEFAULT_MAX_BYTES,
            None,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match previews.try_recv() {
                Ok(crate::edit_preview_cache::EditPreviewEvent::Saved { item_key }) => {
                    assert_eq!(item_key, key);
                    break;
                }
                Ok(_) => panic!("unexpected preview event"),
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "preview save did not finish"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("preview service disconnected"),
            }
        }
        let messages = run_with_preview(&mut req, &Memory::default(), None, None, Some(&db));
        let expected = if compiled_page {
            CoverIdentity::Unrotatable
        } else {
            CoverIdentity::Page(key)
        };
        assert_cover(&messages, &expected);
        assert_eq!(messages.len(), 1);
        let pixels = messages[0].pixels().unwrap();
        assert!(pixels.from_edit_preview);
        assert_eq!(
            pixels.origin,
            ThumbLoadOrigin::EditPreviewCache { epoch: db.epoch() }
        );
        assert_eq!(pixels.image.size, [12, 8]);
    }
}

#[test]
fn cover_identity_epub_generated_first_and_pinned_outer_rows_keep_the_logical_page() {
    let fixture = crate::epub_cache::reconverted_for_worker_test();
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Arc::new(crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap());
    for (page, key) in [
        (0, "pdfthumb:worker-book.epub"),
        (3, "folderthumb:manual#pin:epub-page3"),
    ] {
        let mut req = request(&fixture.source, key);
        req.pdf_page = Some(page);
        req.pdf_stamp_policy = PdfStampPolicy::ResolveInWorker;
        req.source_policy = LoadSourcePolicy::SourceOnly;
        req.force_cache = true;
        let read = crate::pdf_loader::resolve_read_target(&fixture.source).unwrap();
        assert_ne!(read.read_path.as_path(), fixture.source.as_path());
        let stamped = stamp_resolved_pdf_request(&req, &read).unwrap();
        let row_key = cache_key_for_request(&stamped).unwrap();
        let memory = Memory::default();
        let messages = with_test_pdf_render_result(
            crate::pdf_loader::RenderResult {
                image: image::DynamicImage::new_rgb8(12, 8),
                content_type: crate::pdf_loader::PdfPageContentType::Vector,
                page_count: 5,
                page_size_points: crate::pdf_loader::PdfPageSizePoints {
                    width: 120.0,
                    height: 80.0,
                },
            },
            || run(&mut req, &memory, Some(&catalog), None),
        );
        let expected =
            CoverIdentity::Page(crate::edit_source::page_key_for_pdf(&fixture.source, page));
        assert_cover(&messages, &expected);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        let row = catalog.load_one(&row_key).unwrap().unwrap();
        assert_eq!(row.cover, expected);
        assert_eq!(
            (row.mtime, row.file_size),
            (
                fixture.current.generation_id,
                fixture.current.pdf_size as i64
            )
        );
        assert_eq!(memory.read().unwrap()[row_key.as_ref()].cover, expected);
        assert_ne!(
            row.cover,
            CoverIdentity::Page(crate::edit_source::page_key_for_pdf(
                read.read_path.as_path(),
                page
            ))
        );
    }
}

#[test]
fn cover_identity_bulk_pdf_helper_persists_the_rendered_page() {
    // Existing in-process PDF test backend covers the helper's direct render_page call,
    // rather than the common worker's render-result injection hook.
    let tmp = tempfile::tempdir().unwrap();
    let pdf = tmp.path().join("bulk.pdf");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] >>",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Root 1 0 R /Size 4 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(&pdf, &bytes).unwrap();
    let _backend = crate::pdf_loader::RemotePdfTestBackend::for_path(&pdf);
    let catalog = crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap();
    assert!(
        build_and_save_one_pdf(
            &pdf,
            None,
            0,
            None,
            &catalog,
            17,
            bytes.len() as i64,
            64,
            80
        )
        .is_some()
    );
    let row = catalog
        .load_one(&crate::grid_item::pdf_page_cache_key(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.cover,
        CoverIdentity::Page(crate::edit_source::page_key_for_pdf(&pdf, 0))
    );
}
