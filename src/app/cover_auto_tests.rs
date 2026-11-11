use super::*;
use crate::cover_auto_crop::AutoCropIdentity;
use crate::folder_thumb_pins::{CoverCrop, FileKind, FolderPin, FolderPinSource};
use crate::settings::ReadingDirection;

fn request(
    item: &GridItem,
    pins: &HashMap<String, FolderPin>,
    auto: Option<AutoCropIdentity>,
) -> LoadRequest {
    make_load_request_with_auto_crop(
        item,
        0,
        17,
        23,
        false,
        None,
        None,
        3,
        pins,
        &HashMap::new(),
        None,
        None,
        None,
        None,
        false,
        &Default::default(),
        auto,
    )
    .unwrap()
}

fn saved_bytes() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut out, image::ImageFormat::WebP)
        .unwrap();
    out.into_inner()
}

#[test]
fn cover_auto_request_roles_binding_and_off_keys() {
    let zip = PathBuf::from("book.zip");
    let items = [
        GridItem::ZipFile(zip.clone()),
        GridItem::PdfFile("book.pdf".into()),
        GridItem::PdfFile("book.epub".into()),
        GridItem::ZipDir {
            zip_path: zip,
            dir_prefix: "book/".into(),
            is_archive: false,
            representative: Some("book/page.jpg".into()),
        },
        GridItem::Folder("folder".into()),
        GridItem::Folder("scans.cr3".into()),
        GridItem::Folder("movie.mp4".into()),
        GridItem::Image("page.jpg".into()),
    ];
    let pins = HashMap::new();
    for item in items {
        let legacy = make_load_request_with_visibility(
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
        assert_eq!(
            crate::thumb_loader::cache_key_for_request(&request(&item, &pins, None)),
            crate::thumb_loader::cache_key_for_request(&legacy)
        );
        for direction in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            let auto = AutoCropIdentity::new(direction);
            let req = request(&item, &pins, Some(auto));
            let eligible = !matches!(item, GridItem::Image(_));
            assert_eq!(req.auto_cover_crop, eligible.then_some(auto));
            assert_eq!(
                crate::thumb_loader::cache_key_for_request(&req)
                    .unwrap()
                    .contains(auto.suffix()),
                eligible
            );
            if let Some(row) = &req.parent_row {
                assert_eq!(row.row_key.contains(auto.suffix()), eligible);
            }
        }
    }
}

#[test]
fn cover_auto_converted_archive_request_keeps_logical_key_and_read_path() {
    let source = PathBuf::from("book.7z");
    let load = PathBuf::from("converted/book.zip");
    let item = GridItem::ConvertibleArchive {
        path: source.clone(),
        format: crate::archive_converter::ArchiveFormat::SevenZ,
    };
    let states = HashMap::from([(
        crate::path_key::normalize_keep_drive(&source),
        ConvertedArchiveSourceState::CachedZip {
            logical_source: source,
            path: load.clone(),
        },
    )]);
    let build = |auto| {
        make_load_request_with_auto_crop(
            &item,
            0,
            17,
            23,
            false,
            None,
            None,
            3,
            &HashMap::new(),
            &states,
            None,
            None,
            None,
            None,
            false,
            &Default::default(),
            auto,
        )
        .unwrap()
    };
    let off = build(None);
    let auto = AutoCropIdentity::new(ReadingDirection::Rtl);
    let on = build(Some(auto));
    assert_eq!(on.path, load);
    assert_eq!(on.auto_cover_crop, Some(auto));
    assert_eq!(
        crate::thumb_loader::cache_key_for_request(&on).unwrap(),
        crate::thumb_loader::representative_cache_key_for_policy_with_auto(
            &crate::thumb_loader::cache_key_for_request(&off).unwrap(),
            &Default::default(),
            Some(auto)
        )
    );
}

#[test]
fn cover_auto_known_raw_media_and_manual_ranges_keep_uncropped_namespace() {
    let zip = PathBuf::from("book.zip");
    let auto = Some(AutoCropIdentity::new(ReadingDirection::Rtl));
    for entry in [
        "book/cover.cr2",
        "book/movie.mp4",
        "book/audio.mp3",
        "book/cover.jpg",
    ] {
        let item = GridItem::ZipDir {
            zip_path: zip.clone(),
            dir_prefix: "book/".into(),
            is_archive: false,
            representative: Some(entry.into()),
        };
        let plain = request(&item, &HashMap::new(), auto);
        assert_eq!(plain.auto_cover_crop.is_some(), entry.ends_with("jpg"));
        let crop = CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap();
        let pins = HashMap::from([(
            crate::path_key::normalize_keep_drive(&book_container_key(&zip, "book/")),
            FolderPin {
                source: FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: entry.into(),
                },
                cover_crop: crop,
            },
        )]);
        let req = request(&item, &pins, auto);
        assert!(req.auto_cover_crop.is_none());
        assert!(
            !crate::thumb_loader::cache_key_for_request(&req)
                .unwrap()
                .contains("|autocrop:")
        );
    }
}

#[test]
fn cover_auto_folder_seeded_pin_stays_outside_automatic_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("book.zip"), b"identity").unwrap();
    let item = GridItem::Folder(folder.clone());
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&folder),
        FolderPinSource::ZipEntry {
            zip_rel: "book.zip".into(),
            entry: "cover.jpg".into(),
        }
        .into(),
    )]);
    let req = request(
        &item,
        &pins,
        Some(AutoCropIdentity::new(ReadingDirection::Rtl)),
    );
    assert_eq!(
        req.folder_thumb_provenance,
        Some(crate::catalog::FolderThumbProvenance::Seeded)
    );
    assert!(req.auto_cover_crop.is_none());
    let keep = folder_thumb_existing_keys_for_with_auto_crop(
        &item,
        Some((17, 23)),
        &pins,
        None,
        None,
        3,
        false,
        &Default::default(),
        Some(AutoCropIdentity::new(ReadingDirection::Rtl)),
    );
    assert!(
        keep.contains(
            &crate::thumb_loader::cache_key_for_request(&req)
                .unwrap()
                .into_owned()
        )
    );
}

#[test]
fn cover_auto_drive_guard_does_not_leak_into_incoming_list_settings() {
    let mut app = super::tests::phase_c_support::setup_app();
    assert!(app.thumbnail_auto_crop_identity().is_none());
    app.settings.thumb_cover_auto_crop = true;
    app.settings.default_reading_direction = ReadingDirection::Ltr;
    let expected = Some(AutoCropIdentity::new(ReadingDirection::Ltr));
    assert_eq!(app.thumbnail_auto_crop_identity(), expected);
    app.items_are_drive_list = true;
    assert!(app.thumbnail_auto_crop_identity().is_none());
    assert_eq!(configured_thumbnail_auto_crop(&app.settings), expected);
}

#[test]
fn cover_auto_folder_pin_keeps_folder_role_when_name_looks_like_media() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("shelf");
    std::fs::create_dir_all(root.join("movie.mp4")).unwrap();
    let item = GridItem::Folder(root.clone());
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&root),
        FolderPin::from(FolderPinSource::File {
            rel: "movie.mp4".into(),
            kind: FileKind::Folder,
        }),
    )]);
    let auto = AutoCropIdentity::new(ReadingDirection::Rtl);
    let req = request(&item, &pins, Some(auto));
    assert_eq!(req.auto_cover_crop, Some(auto));
    let key = crate::thumb_loader::cache_key_for_request(&req).unwrap();
    let existing = folder_thumb_existing_keys_for_with_auto_crop(
        &item,
        Some((17, 23)),
        &pins,
        None,
        None,
        3,
        false,
        &Default::default(),
        Some(auto),
    );
    assert!(existing.iter().any(|existing| existing == key.as_ref()));
}

#[test]
fn cover_auto_existing_file_keys_match_writer_and_retire_previous_binding() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["book.zip", "book.pdf"] {
        let path = temp.path().join(name);
        std::fs::write(&path, b"identity").unwrap();
        let item = if name.ends_with("zip") {
            GridItem::ZipFile(path.clone())
        } else {
            GridItem::PdfFile(path.clone())
        };
        let source = if name.ends_with("zip") {
            FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "cover.jpg".into(),
            }
        } else {
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            }
        };
        for crop in [None, CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap()] {
            let pins = HashMap::from([(
                crate::path_key::normalize_keep_drive(&path),
                FolderPin {
                    source: source.clone(),
                    cover_crop: crop,
                },
            )]);
            let mut previous = None;
            for direction in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
                let auto = Some(AutoCropIdentity::new(direction));
                let req = request(&item, &pins, auto);
                let key = crate::thumb_loader::cache_key_for_request(&req)
                    .unwrap()
                    .into_owned();
                let keep: HashSet<_> = folder_thumb_existing_keys_for_with_auto_crop(
                    &item,
                    Some((17, 23)),
                    &pins,
                    None,
                    None,
                    3,
                    false,
                    &Default::default(),
                    auto,
                )
                .into_iter()
                .collect();
                assert!(keep.contains(&key), "writer missing: {key}");
                let released =
                    crate::thumb_loader::cache_key_for_request(&request(&item, &pins, None))
                        .unwrap()
                        .into_owned();
                let base = container_cache_base_key(&item, false, None, 3).unwrap();
                assert!(
                    keep.contains(&released),
                    "released pin key missing: {released}"
                );
                assert!(keep.contains(&base), "page seed base key missing: {base}");
                if crop.is_none()
                    && let Some(previous) = &previous
                {
                    assert!(!keep.contains(previous));
                }
                let db = crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path())
                    .unwrap();
                for saved_key in [&key, &released, &base] {
                    db.save_thumb_bytes(
                        saved_key,
                        req.mtime,
                        req.file_size,
                        Some((4, 10)),
                        &saved_bytes(),
                        &crate::cover_identity::CoverIdentity::Unknown,
                    )
                    .unwrap();
                }
                db.delete_missing(&keep).unwrap();
                assert!(db.load_one(&key).unwrap().is_some());
                assert!(db.load_one(&released).unwrap().is_some());
                assert!(db.load_one(&base).unwrap().is_some());
                previous = Some(key);
            }
        }
    }
}

#[test]
fn cover_auto_zipdir_pinned_and_unpinned_rows_survive_current_binding_only() {
    let mut app = super::tests::phase_c_support::setup_app();
    app.settings.thumb_cover_auto_crop = true;
    let zip = app.tmp.path().join("books.zip");
    std::fs::write(&zip, b"identity").unwrap();
    let pin_db = app.folder_thumb_pin_db.as_ref().unwrap().clone();
    let owner = book_container_key(&zip, "book/");
    let source = FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "book/cover.jpg".into(),
    };
    for crop in [None, CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap()] {
        pin_db.set_with_cover_crop(&owner, &source, crop).unwrap();
        for direction in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
            app.settings.default_reading_direction = direction;
            let enumeration = crate::zip_loader::PreparedZipEnumeration::prepare(
                zip.clone(),
                crate::zip_loader::ZipEnumeration {
                    entries: ["book/cover.jpg", "other/page.jpg"]
                        .into_iter()
                        .map(|entry| crate::zip_loader::ZipImageEntry {
                            entry_name: entry.into(),
                            uncompressed_size: 23,
                            mtime: 17,
                            container_ancestry: Vec::new(),
                        })
                        .collect(),
                    has_foreign_archives: false,
                    legacy_renames: Vec::new(),
                },
                Default::default(),
            );
            let prepared = app.prepare_zip_grid(
                zip.clone(),
                enumeration,
                None,
                StartupListIntent::ExplicitList,
            );
            let pins = pin_db.lookup_many([owner.clone()]);
            for item in prepared
                .items
                .iter()
                .filter(|item| matches!(item, GridItem::ZipDir { .. }))
            {
                let req = request(item, &pins, configured_thumbnail_auto_crop(&app.settings));
                let key = crate::thumb_loader::cache_key_for_request(&req)
                    .unwrap()
                    .into_owned();
                assert!(
                    prepared.existing_keys.contains(&key),
                    "ZIP current key missing: {key}"
                );
                let released =
                    crate::thumb_loader::cache_key_for_request(&request(item, &pins, None))
                        .unwrap()
                        .into_owned();
                assert!(
                    prepared.existing_keys.contains(&released),
                    "ZIP released key missing: {released}"
                );
                let db =
                    crate::catalog::CatalogDb::open(&app.tmp.path().join("cache"), app.tmp.path())
                        .unwrap();
                db.save_thumb_bytes(
                    &key,
                    17,
                    23,
                    Some((4, 10)),
                    &saved_bytes(),
                    &crate::cover_identity::CoverIdentity::Unknown,
                )
                .unwrap();
                db.delete_missing(&prepared.existing_keys).unwrap();
                assert!(db.load_one(&key).unwrap().is_some());
                if req.auto_cover_crop.is_some() {
                    let old = key.replace(
                        direction_for_suffix(direction),
                        direction_for_suffix(if direction == ReadingDirection::Rtl {
                            ReadingDirection::Ltr
                        } else {
                            ReadingDirection::Rtl
                        }),
                    );
                    assert!(!prepared.existing_keys.contains(&old));
                }
            }
        }
    }
}

fn direction_for_suffix(direction: ReadingDirection) -> &'static str {
    AutoCropIdentity::new(direction).suffix()
}

#[test]
fn cover_auto_epub_completed_generation_retains_documented_1742_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let epub = temp.path().join("book.epub");
    std::fs::write(&epub, b"identity").unwrap();
    let item = GridItem::PdfFile(epub.clone());
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&epub),
        FolderPinSource::PdfPage {
            pdf_rel: String::new(),
            page: 1,
        }
        .into(),
    )]);
    for auto in [None, Some(AutoCropIdentity::new(ReadingDirection::Rtl))] {
        let pending = request(&item, &pins, auto);
        let read = crate::pdf_loader::generation_target_for_test(&epub, 91, 200);
        let completed = crate::thumb_loader::stamp_resolved_pdf_request(&pending, &read).unwrap();
        let key = crate::thumb_loader::cache_key_for_request(&completed)
            .unwrap()
            .into_owned();
        let keep = folder_thumb_existing_keys_for_with_auto_crop(
            &item,
            Some((17, 23)),
            &pins,
            None,
            None,
            3,
            false,
            &Default::default(),
            auto,
        )
        .into_iter()
        .collect();
        let db = crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap();
        db.save_thumb_bytes(
            &key,
            91,
            200,
            Some((4, 10)),
            &saved_bytes(),
            &crate::cover_identity::CoverIdentity::Unknown,
        )
        .unwrap();
        db.delete_missing(&keep).unwrap();
        assert!(
            db.load_one(&key).unwrap().is_none(),
            "§1.742 changed; update the documented boundary"
        );
    }
}

#[test]
fn cover_auto_collection_source_digest_and_reuse_follow_enabled_binding() {
    use super::top_level_grid_view::CollectionGridThumbnailSources;
    let off = super::collection_grid::prepare_collection_grid_thumbnail_sources(
        CollectionGridThumbnailSources::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let identity =
        |delivery: super::top_level_grid_view::CollectionGridPreparedThumbnailDelivery| {
            match delivery.presentation {
                super::top_level_grid_view::CollectionGridPresentationSources::Retained(
                    sources,
                ) => sources.identity,
                _ => panic!("small retained sources"),
            }
        };
    let off = identity(off);
    assert_eq!(off, Default::default());
    let mut identities = Vec::new();
    for direction in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        let delivery = super::collection_grid::prepare_collection_grid_thumbnail_sources(
            CollectionGridThumbnailSources {
                auto_cover_crop: Some(AutoCropIdentity::new(direction)),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let current = identity(delivery);
        assert_ne!(off, current);
        identities.push(current);
    }
    assert_ne!(identities[0], identities[1]);
    let id = crate::collection_store::CollectionId::new();
    let mut settings = crate::settings::Settings::default();
    let reuse = |settings: &crate::settings::Settings| {
        super::top_level_grid_view::CollectionGridPrepareReuseKey::new(
            id, 1, settings, None, None, 0,
        )
    };
    let off = reuse(&settings);
    settings.default_reading_direction = ReadingDirection::Ltr;
    assert_eq!(reuse(&settings), off);
    settings.thumb_cover_auto_crop = true;
    let ltr = reuse(&settings);
    assert_ne!(ltr, off);
    settings.default_reading_direction = ReadingDirection::Rtl;
    assert_ne!(reuse(&settings), ltr);
}
