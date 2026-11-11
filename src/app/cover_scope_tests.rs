use super::*;
use crate::cover_source_scope::CoverSourceScope;
use crate::file_type_visibility::FileTypeVisibility;
use crate::folder_thumb_pins::{FileKind, FolderPinSource};
use crate::rotation_db::Rotation;
use std::collections::{BTreeMap, HashMap};

fn request(
    item: &GridItem,
    pins: &HashMap<String, FolderPinSource>,
    converted: &HashMap<String, ConvertedArchiveSourceState>,
    visibility: &FileTypeVisibility,
) -> LoadRequest {
    make_load_request_with_visibility(
        item,
        0,
        17,
        23,
        false,
        None,
        Some(crate::settings::SortOrder::Numeric),
        3,
        pins,
        converted,
        None,
        None,
        None,
        None,
        false,
        visibility,
    )
    .expect("fixture must assemble a thumbnail request")
}

fn auto_request(item: &GridItem) -> LoadRequest {
    request(
        item,
        &HashMap::new(),
        &HashMap::new(),
        &FileTypeVisibility::default(),
    )
}

fn assert_refresh_for(request: &LoadRequest, key: String, expected: bool) {
    let rotations = BTreeMap::from([(key.clone(), Rotation::Cw90)]);
    let mut prepared = request.clone();
    prepared.prepare_cover_refresh(&rotations);
    assert_eq!(
        prepared.refresh_legacy_cover, expected,
        "rotation key: {key}"
    );
    assert_eq!(
        prepared
            .cover_scope
            .as_ref()
            .is_some_and(|scope| scope.has_rotation(&rotations)),
        expected,
    );
}

fn converted_source(source: &Path, cache: &Path) -> HashMap<String, ConvertedArchiveSourceState> {
    HashMap::from([(
        crate::path_key::normalize_keep_drive(source),
        ConvertedArchiveSourceState::CachedZip {
            logical_source: source.to_path_buf(),
            path: cache.to_path_buf(),
        },
    )])
}

#[test]
fn automatic_folder_zip_pdf_and_epub_scopes_match_source_boundaries() {
    let root = Path::new("C:/scope-books");
    let folder = root.join("images");
    let req = auto_request(&GridItem::Folder(folder.clone()));
    assert_refresh_for(
        &req,
        crate::adjustment_db::normalize_path(&folder.join("nested/page.png")),
        true,
    );
    assert_refresh_for(
        &req,
        crate::adjustment_db::normalize_path(&root.join("images-other/page.png")),
        false,
    );
    for extension in ["zip", "pdf", "epub"] {
        let path = root.join(format!("book.{extension}"));
        let item = if extension == "zip" {
            GridItem::ZipFile(path.clone())
        } else {
            GridItem::PdfFile(path.clone())
        };
        let req = auto_request(&item);
        let page = if extension == "zip" {
            crate::adjustment_db::zip_entry_key(&path, "chapter/page.png")
        } else {
            crate::edit_source::page_key_for_pdf(&path, 4)
        };
        assert_refresh_for(&req, page, true);
        assert_refresh_for(&req, crate::adjustment_db::normalize_path(&path), false);
        assert_refresh_for(
            &req,
            crate::adjustment_db::zip_entry_key(&root.join(format!("other.{extension}")), "page_4"),
            false,
        );
    }
}

#[test]
fn converted_archive_scope_retains_original_and_effective_cache_paths() {
    let source = PathBuf::from("C:/scope-books/book.rar");
    let cache = PathBuf::from("C:/scope-cache/converted.zip");
    let req = request(
        &GridItem::ConvertibleArchive {
            path: source.clone(),
            format: crate::archive_converter::ArchiveFormat::Rar,
        },
        &HashMap::new(),
        &converted_source(&source, &cache),
        &FileTypeVisibility::default(),
    );
    assert_eq!(req.path, cache);
    for path in [&source, &cache] {
        assert_refresh_for(
            &req,
            crate::adjustment_db::zip_entry_key(path, "page.png"),
            true,
        );
    }
    assert_refresh_for(
        &req,
        crate::adjustment_db::zip_entry_key(Path::new("C:/scope-cache/other.zip"), "page.png"),
        false,
    );
}

#[test]
fn unpinned_zip_directory_scope_retains_prefix_for_original_and_cache() {
    let source = PathBuf::from("C:/scope-books/book.rar");
    let cache = PathBuf::from("C:/scope-cache/converted.zip");
    let req = request(
        &GridItem::ZipDir {
            zip_path: source.clone(),
            dir_prefix: "chapter/".into(),
            is_archive: false,
            representative: Some("chapter/first.png".into()),
        },
        &HashMap::new(),
        &converted_source(&source, &cache),
        &FileTypeVisibility::default(),
    );
    assert_eq!(req.path, cache);
    for path in [&source, &cache] {
        assert_refresh_for(
            &req,
            crate::adjustment_db::zip_entry_key(path, "chapter/nested/page.png"),
            true,
        );
        assert_refresh_for(
            &req,
            crate::adjustment_db::zip_entry_key(path, "chapter-other/page.png"),
            false,
        );
    }
}

#[test]
fn zip_directory_exact_pin_uses_selected_page_scope() {
    let path = PathBuf::from("C:/scope-books/book.zip");
    let item = GridItem::ZipDir {
        zip_path: path.clone(),
        dir_prefix: "chapter/".into(),
        is_archive: false,
        representative: Some("chapter/first.png".into()),
    };
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&book_container_key(&path, "chapter/")),
        FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "outside/pinned.png".into(),
        },
    )]);
    let req = request(
        &item,
        &pins,
        &HashMap::new(),
        &FileTypeVisibility::default(),
    );
    assert_eq!(req.zip_entry.as_deref(), Some("outside/pinned.png"));
    assert_refresh_for(
        &req,
        crate::adjustment_db::zip_entry_key(&path, "outside/pinned.png"),
        true,
    );
    assert_refresh_for(
        &req,
        crate::adjustment_db::zip_entry_key(&path, "outside/other.png"),
        false,
    );
    assert_refresh_for(
        &req,
        crate::adjustment_db::zip_entry_key(&path, "chapter/first.png"),
        false,
    );
}

#[test]
fn zip_pdf_and_epub_exact_pins_do_not_include_other_pages() {
    let temp = tempfile::tempdir().unwrap();
    for extension in ["zip", "pdf", "epub"] {
        let path = temp.path().join(format!("book.{extension}"));
        std::fs::write(&path, b"request assembly only").unwrap();
        let (item, source, selected, other) = if extension == "zip" {
            (
                GridItem::ZipFile(path.clone()),
                FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: "selected.png".into(),
                },
                crate::adjustment_db::zip_entry_key(&path, "selected.png"),
                crate::adjustment_db::zip_entry_key(&path, "other.png"),
            )
        } else {
            (
                GridItem::PdfFile(path.clone()),
                FolderPinSource::PdfPage {
                    pdf_rel: String::new(),
                    page: 2,
                },
                crate::edit_source::page_key_for_pdf(&path, 2),
                crate::edit_source::page_key_for_pdf(&path, 0),
            )
        };
        let pins = HashMap::from([(crate::path_key::normalize_keep_drive(&path), source)]);
        let req = request(
            &item,
            &pins,
            &HashMap::new(),
            &FileTypeVisibility::default(),
        );
        assert!(
            req.cache_key_override
                .as_deref()
                .unwrap()
                .contains(crate::thumb_loader::CACHE_KEY_PIN_SUFFIX)
        );
        assert_refresh_for(&req, selected, true);
        assert_refresh_for(&req, other, false);
    }
}

#[test]
fn folder_pin_of_converted_archive_covers_original_and_cache() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let source = folder.join("book.rar");
    let cache = temp.path().join("converted.zip");
    std::fs::write(&source, b"source metadata").unwrap();
    std::fs::write(&cache, b"cache metadata").unwrap();
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&folder),
        FolderPinSource::File {
            rel: "book.rar".into(),
            kind: FileKind::ConvertibleArchive,
        },
    )]);
    let req = request(
        &GridItem::Folder(folder.clone()),
        &pins,
        &converted_source(&source, &cache),
        &FileTypeVisibility::default(),
    );
    assert_eq!(req.path, cache);
    assert_eq!(
        req.resolve_override,
        Some(crate::thumb_loader::ResolveStrategy::ZipFirstImage)
    );
    for path in [&source, &cache] {
        assert_refresh_for(
            &req,
            crate::adjustment_db::zip_entry_key(path, "cover.png"),
            true,
        );
    }
    assert_refresh_for(
        &req,
        crate::adjustment_db::normalize_path(&folder.join("other.png")),
        false,
    );
}

#[test]
fn nested_archive_pin_fallback_unions_folder_cover_and_propagates_refresh() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let cover = root.join("cover.png");
    std::fs::write(&cover, b"fallback metadata").unwrap();
    let archive = root.join("book.cbz");
    let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    inner
        .start_file("page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    inner.write_all(b"request scope does not decode").unwrap();
    let mut outer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    outer
        .start_file("inner.zip", zip::write::SimpleFileOptions::default())
        .unwrap();
    outer
        .write_all(&inner.finish().unwrap().into_inner())
        .unwrap();
    outer.finish().unwrap();
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&root),
        FolderPinSource::ZipEntry {
            zip_rel: "book.cbz".into(),
            entry: "inner.zip/page.png".into(),
        },
    )]);
    let visibility = FileTypeVisibility {
        excluded_extensions: ["zip".into()].into_iter().collect(),
        ..Default::default()
    };
    let mut req = request(
        &GridItem::Folder(root.clone()),
        &pins,
        &HashMap::new(),
        &visibility,
    );
    assert_eq!(req.zip_entry.as_deref(), Some("inner.zip/page.png"));
    let fallback = req
        .epub_pin_fallback
        .as_ref()
        .expect("nested exclusion needs a folder fallback");
    assert_eq!(fallback.path, root);
    assert_refresh_for(
        &req,
        crate::adjustment_db::zip_entry_key(&archive, "inner.zip/page.png"),
        true,
    );
    assert_refresh_for(&req, crate::adjustment_db::normalize_path(&cover), true);
    assert_refresh_for(
        &req,
        crate::adjustment_db::normalize_path(&temp.path().join("other/cover.png")),
        false,
    );
    req.prepare_cover_refresh(&BTreeMap::from([(
        crate::adjustment_db::normalize_path(&cover),
        Rotation::Cw90,
    )]));
    assert!(req.refresh_legacy_cover);
    assert!(req.epub_pin_fallback.as_ref().unwrap().refresh_legacy_cover);
}

#[test]
fn pending_archive_with_policy_fallback_has_no_refreshable_scope() {
    let source = PathBuf::from("C:/scope-books/book.rar");
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&source),
        FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "chapter/page.png".into(),
        },
    )]);
    let converted = HashMap::from([(
        crate::path_key::normalize_keep_drive(&source),
        ConvertedArchiveSourceState::Pending,
    )]);
    let visibility = FileTypeVisibility {
        excluded_extensions: ["zip".into()].into_iter().collect(),
        ..Default::default()
    };
    let mut req = request(
        &GridItem::ConvertibleArchive {
            path: source.clone(),
            format: crate::archive_converter::ArchiveFormat::Rar,
        },
        &pins,
        &converted,
        &visibility,
    );
    assert_eq!(req.source_policy, LoadSourcePolicy::CacheOnly);
    assert!(req.epub_pin_fallback.is_some());
    req.prepare_cover_refresh(&BTreeMap::from([(
        crate::adjustment_db::zip_entry_key(&source, "chapter/page.png"),
        Rotation::Cw90,
    )]));
    assert!(req.cover_scope.is_none());
    assert!(!req.refresh_legacy_cover);
    let fallback = req.epub_pin_fallback.as_ref().unwrap();
    assert!(fallback.cover_scope.is_none());
    assert!(!fallback.refresh_legacy_cover);
}

#[test]
fn requests_without_source_decode_drop_scope_before_admission() {
    let folder = PathBuf::from("C:/scope-books/folder");
    let rotations = BTreeMap::from([(
        crate::adjustment_db::normalize_path(&folder.join("cover.png")),
        Rotation::Cw90,
    )]);
    let base = auto_request(&GridItem::Folder(folder));
    let mut cache_only = base.clone();
    cache_only.source_policy = LoadSourcePolicy::CacheOnly;
    let mut memory_seed = base.clone();
    memory_seed.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::Seeded);
    assert!(memory_seed.uses_memory_pin_seed());
    let mut pinned_only = base;
    pinned_only.pinned_only = Some(crate::thumb_loader::PinnedOnlyRequest {
        cache_key_prefix: "drive-pin".into(),
        seed_proof: None,
        source: FolderPinSource::File {
            rel: "child".into(),
            kind: FileKind::Folder,
        },
    });
    for mut req in [cache_only, memory_seed, pinned_only] {
        assert!(req.cover_scope.as_ref().unwrap().has_rotation(&rotations));
        req.refresh_legacy_cover = true;
        req.prepare_cover_refresh(&rotations);
        assert_eq!(req.cover_scope, None::<CoverSourceScope>);
        assert!(!req.refresh_legacy_cover);
    }
}
