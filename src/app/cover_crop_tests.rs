use super::*;
use crate::folder_thumb_pins::{CoverCrop, FileKind, FolderPin, FolderPinSource, PinKeyMode};

fn thumbnail_bytes() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::WebP)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn cover_crop_released_leaf_key_strings_are_golden() {
    let container = Path::new(r"C:\books");
    let cases = [
        (
            FolderPinSource::File {
                rel: "cover.jpg".into(),
                kind: FileKind::Image,
            },
            "folderthumb:book",
            "folderthumb:book#pin:image|cover.jpg|-|-|17|23",
        ),
        (
            FolderPinSource::ZipEntry {
                zip_rel: "book.zip".into(),
                entry: "01.jpg".into(),
            },
            "zipthumb:book.zip",
            "zipthumb:book.zip#pin:zipentry|book.zip|01.jpg|-|17|23",
        ),
        (
            FolderPinSource::PdfPage {
                pdf_rel: "book.pdf".into(),
                page: 7,
            },
            "pdfthumb:book.pdf",
            "pdfthumb:book.pdf#pin:pdfpage|book.pdf|-|7|17|23",
        ),
    ];
    for (source, base, expected) in cases {
        let pin: FolderPin = source.into();
        let target = crate::folder_thumb_pins::resolve_pin_target_cascaded_via_with_metadata(
            container,
            &pin,
            |_| None,
            |_| Some((17, 23)),
            0,
        )
        .unwrap();
        assert_eq!(
            target.pinned_cache_key(base, PinKeyMode::Exact, None),
            expected
        );
        assert_eq!(
            target.pinned_cache_key(base, PinKeyMode::WithoutCrop, None),
            expected
        );
    }
}

#[test]
fn cover_crop_zipdir_request_preserves_golden_key_and_assigns_crop_atomically() {
    let zip = PathBuf::from(r"C:\books\book.zip");
    let item = GridItem::ZipDir {
        zip_path: zip.clone(),
        dir_prefix: "book/".into(),
        is_archive: false,
        representative: Some("book/01.jpg".into()),
    };
    let source = FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "book/cover.jpg".into(),
    };
    let crop = CoverCrop::new([100_000, 50_000, 500_000, 950_000])
        .unwrap()
        .unwrap();
    for crop in [None, Some(crop)] {
        let mut pins = HashMap::new();
        pins.insert(
            crate::path_key::normalize_keep_drive(&book_container_key(&zip, "book/")),
            FolderPin {
                source: source.clone(),
                cover_crop: crop,
            },
        );
        let request = make_load_request_with_visibility(
            &item,
            0,
            17,
            23,
            false,
            None,
            Some(crate::settings::SortOrder::Numeric),
            3,
            &pins,
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &Default::default(),
        )
        .unwrap();
        let expected = if crop.is_some() {
            "zipdir:book/#pin:book/cover.jpg|crop:100000,50000,500000,950000"
        } else {
            "zipdir:book/#pin:book/cover.jpg"
        };
        assert_eq!(request.cache_key_override.as_deref(), Some(expected));
        assert_eq!(request.cover_crop, crop);
        assert_eq!(request.edit_preview_key.is_none(), crop.is_some());
        assert!(request.pinned_page_adjustment_key.is_some());
    }
}

#[test]
fn cover_crop_drive_catalog_prefix_skips_newer_cropped_and_auto_rows() {
    let temporary = tempfile::tempdir().unwrap();
    let db = crate::catalog::CatalogDb::open(temporary.path(), Path::new(r"C:\books")).unwrap();
    let prefix = "folderthumb:book#pin:image|cover.jpg|-|-|";
    for (suffix, stamp) in [
        ("17|23", 17),
        ("crop:0,0,500000,1000000|30|23", 30),
        ("40|23|autocrop:v1:rtl", 40),
    ] {
        db.save_thumb_bytes(
            &format!("{prefix}{suffix}"),
            stamp,
            23,
            Some((1, 1)),
            &thumbnail_bytes(),
            &crate::cover_identity::CoverIdentity::Unknown,
        )
        .unwrap();
    }
    let (key, _) = db
        .load_latest_uncropped_with_prefix_matching(prefix, None)
        .unwrap()
        .unwrap();
    assert_eq!(key, format!("{prefix}17|23"));
    assert!(
        db.load_latest_uncropped_with_prefix_matching(prefix, Some((30, 23)))
            .unwrap()
            .is_none()
    );
}

#[test]
fn cover_crop_completed_rows_survive_existing_key_cleanup_for_folder_zip_pdf() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("cover.jpg"), b"image").unwrap();
    let zip = root.join("book.zip");
    let pdf = root.join("book.pdf");
    std::fs::write(&zip, b"zip identity").unwrap();
    std::fs::write(&pdf, b"pdf identity").unwrap();
    let crop = CoverCrop::new([0, 0, 400_000, 1_000_000]).unwrap();
    for (item, container, source) in [
        (
            GridItem::Folder(folder.clone()),
            folder,
            FolderPinSource::File {
                rel: "cover.jpg".into(),
                kind: FileKind::Image,
            },
        ),
        (
            GridItem::ZipFile(zip.clone()),
            zip,
            FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "cover.jpg".into(),
            },
        ),
        (
            GridItem::PdfFile(pdf.clone()),
            pdf,
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            },
        ),
    ] {
        let pins = HashMap::from([(
            crate::path_key::normalize_keep_drive(&container),
            FolderPin {
                source,
                cover_crop: crop,
            },
        )]);
        let request = make_load_request_with_visibility(
            &item,
            0,
            17,
            23,
            false,
            None,
            Some(crate::settings::SortOrder::Numeric),
            3,
            &pins,
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &Default::default(),
        )
        .unwrap();
        let key = crate::thumb_loader::cache_key_for_request(&request)
            .unwrap()
            .into_owned();
        let existing = folder_thumb_existing_keys_for_with_visibility(
            &item,
            Some((17, 23)),
            &pins,
            None,
            Some(crate::settings::SortOrder::Numeric),
            3,
            false,
            &Default::default(),
        )
        .into_iter()
        .collect();
        let catalog = crate::catalog::CatalogDb::open(&root.join("cache"), root).unwrap();
        catalog
            .save_thumb_bytes(
                &key,
                request.mtime,
                request.file_size,
                Some((4, 10)),
                &thumbnail_bytes(),
                &crate::cover_identity::CoverIdentity::Unknown,
            )
            .unwrap();
        catalog.delete_missing(&existing).unwrap();
        assert!(
            catalog.load_one(&key).unwrap().is_some(),
            "completed crop key lost: {key}"
        );
    }
}

#[test]
fn cover_crop_epub_completed_key_retention_matches_existing_1742_limit() {
    // §1.742 predates cover crop: the current keep set has the pending pin key,
    // while the worker appends the EPUB generation. Do not fix it in §1.764.
    let temporary = tempfile::tempdir().unwrap();
    let epub = temporary.path().join("book.epub");
    std::fs::write(&epub, b"epub identity").unwrap();
    for cover_crop in [None, CoverCrop::new([0, 0, 400_000, 1_000_000]).unwrap()] {
        let item = GridItem::PdfFile(epub.clone());
        let pins = HashMap::from([(
            crate::path_key::normalize_keep_drive(&epub),
            FolderPin {
                source: FolderPinSource::PdfPage {
                    pdf_rel: String::new(),
                    page: 1,
                },
                cover_crop,
            },
        )]);
        let pending = make_load_request_with_visibility(
            &item,
            0,
            17,
            23,
            false,
            None,
            None,
            3,
            &pins,
            &HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            &Default::default(),
        )
        .unwrap();
        let read = crate::pdf_loader::generation_target_for_test(&epub, 91, 200);
        let completed = crate::thumb_loader::stamp_resolved_pdf_request(&pending, &read).unwrap();
        let key = crate::thumb_loader::cache_key_for_request(&completed)
            .unwrap()
            .into_owned();
        let existing = folder_thumb_existing_keys_for_with_visibility(
            &item,
            Some((17, 23)),
            &pins,
            None,
            None,
            3,
            false,
            &Default::default(),
        )
        .into_iter()
        .collect();
        let catalog =
            crate::catalog::CatalogDb::open(&temporary.path().join("cache"), temporary.path())
                .unwrap();
        catalog
            .save_thumb_bytes(
                &key,
                91,
                200,
                Some((4, 10)),
                &thumbnail_bytes(),
                &crate::cover_identity::CoverIdentity::Unknown,
            )
            .unwrap();
        assert!(catalog.load_one(&key).unwrap().is_some());
        catalog.delete_missing(&existing).unwrap();
        assert!(
            catalog.load_one(&key).unwrap().is_none(),
            "§1.742 boundary changed; update this documented limitation"
        );
    }
}
