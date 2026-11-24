use super::*;
use crate::cover_auto_crop::CoverAutoCropScope;
use crate::folder_thumb_pins::{FolderPin, FolderPinSource};
use crate::settings::{FavoriteCoverAutoCropPreference as Preference, FavoriteEntry};

fn favorite(path: &Path, value: Preference) -> FavoriteEntry {
    let mut favorite = FavoriteEntry::new("crop policy".into(), path.to_owned());
    favorite.cover_auto_crop = value;
    favorite
}

fn scoped_request(
    item: &GridItem,
    pins: &HashMap<String, FolderPin>,
    converted: &HashMap<String, ConvertedArchiveSourceState>,
    logical_source: Option<&Path>,
    current: Option<&Path>,
    scope: &CoverAutoCropScope,
    full_path: bool,
) -> LoadRequest {
    make_load_request_with_crop_scope(
        item,
        0,
        17,
        23,
        false,
        None,
        None,
        3,
        pins,
        converted,
        logical_source,
        current,
        None,
        None,
        full_path,
        &Default::default(),
        scope,
    )
    .unwrap()
}

fn webp() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::WebP)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn favorite_cover_auto_crop_mixed_request_keys_survive_repeated_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let inside = temp.path().join("photos");
    std::fs::create_dir(&inside).unwrap();
    let mut settings = crate::settings::Settings {
        thumb_cover_auto_crop: true,
        ..Default::default()
    };
    settings.favorites.push(favorite(&inside, Preference::Off));
    let scope = CoverAutoCropScope::from_settings(&settings);
    let db = crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap();
    let mut existing = HashSet::new();
    let mut saved_keys = HashSet::new();
    for root in [&inside, &temp.path().to_owned()] {
        for (name, format) in [
            ("book.zip", None),
            (
                "book.rar",
                Some(crate::archive_converter::ArchiveFormat::Rar),
            ),
            (
                "book.7z",
                Some(crate::archive_converter::ArchiveFormat::SevenZ),
            ),
        ] {
            let path = root.join(name);
            std::fs::write(&path, b"identity").unwrap();
            let item = format.map_or_else(
                || GridItem::ZipFile(path.clone()),
                |format| GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format,
                },
            );
            let converted = match format {
                Some(crate::archive_converter::ArchiveFormat::Rar) => HashMap::from([(
                    crate::path_key::normalize_keep_drive(&path),
                    ConvertedArchiveSourceState::Direct(path.clone()),
                )]),
                Some(crate::archive_converter::ArchiveFormat::SevenZ) => HashMap::from([(
                    crate::path_key::normalize_keep_drive(&path),
                    ConvertedArchiveSourceState::CachedZip {
                        logical_source: path.clone(),
                        path: temp.path().join("read.zip"),
                    },
                )]),
                _ => HashMap::new(),
            };
            for pinned in [false, true] {
                let pins = if pinned {
                    HashMap::from([(
                        crate::path_key::normalize_keep_drive(&path),
                        FolderPin::from(FolderPinSource::ZipEntry {
                            zip_rel: String::new(),
                            entry: "cover.jpg".into(),
                        }),
                    )])
                } else {
                    HashMap::new()
                };
                let request = scoped_request(&item, &pins, &converted, None, None, &scope, false);
                let key = crate::thumb_loader::cache_key_for_request(&request)
                    .unwrap()
                    .into_owned();
                assert_eq!(
                    request.path.as_path(),
                    converted
                        .get(&crate::path_key::normalize_keep_drive(&path))
                        .and_then(ConvertedArchiveSourceState::load_path)
                        .unwrap_or(&path)
                );
                assert_eq!(request.zip_entry.as_deref(), pinned.then_some("cover.jpg"));
                assert_eq!(key.contains("#pin:"), pinned, "{key}");
                assert_eq!(key.contains("|autocrop:"), root != &inside, "{key}");
                let keep: HashSet<_> = folder_thumb_existing_keys_for_with_auto_crop(
                    &item,
                    Some((17, 23)),
                    &pins,
                    None,
                    None,
                    3,
                    false,
                    &Default::default(),
                    cover_auto_crop_for_item(&scope, &item, None, None),
                )
                .into_iter()
                .collect();
                assert!(keep.contains(&key), "{key} missing from {keep:?}");
                db.save_thumb_bytes(
                    &key,
                    request.mtime,
                    request.file_size,
                    Some((2, 2)),
                    &webp(),
                    &crate::cover_identity::CoverIdentity::Unknown,
                    crate::cover_auto_material::AutoMaterial::NotAutomatic,
                )
                .unwrap();
                existing.extend(keep);
                saved_keys.insert(key);
            }
        }
    }
    for _ in 0..2 {
        db.delete_missing(&existing).unwrap();
        for key in &saved_keys {
            assert!(db.load_one(key).unwrap().is_some(), "{key}");
        }
    }
}

#[test]
fn favorite_cover_auto_crop_zipdir_uses_logical_source_and_physical_key_variant() {
    let mut app = setup_app_for_test();
    let logical = app.tmp.path().join("books.rar");
    let converted = app.tmp.path().join("converted.zip");
    std::fs::write(&logical, b"identity").unwrap();
    std::fs::write(&converted, b"identity").unwrap();
    app.settings.thumb_cover_auto_crop = false;
    app.settings.favorites = vec![
        favorite(&logical, Preference::On),
        favorite(&converted, Preference::Off),
    ];
    let scope = CoverAutoCropScope::from_settings(&app.settings);
    let owner = book_container_key(&logical, "book/");
    let pin_db = app.folder_thumb_pin_db.clone().unwrap();
    for physical in [&logical, &converted] {
        for pinned in [false, true] {
            if pinned {
                pin_db
                    .set(
                        &owner,
                        &FolderPinSource::ZipEntry {
                            zip_rel: String::new(),
                            entry: "book/cover.jpg".into(),
                        },
                    )
                    .unwrap();
            } else {
                pin_db.remove(&owner).unwrap();
            }
            let enumeration = crate::zip_loader::PreparedZipEnumeration::prepare(
                physical.clone(),
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
            // Staged preparation must use this source even before current_folder is published.
            let prepared = app.prepare_zip_grid(
                physical.clone(),
                enumeration,
                Some(&logical),
                StartupListIntent::ExplicitList,
            );
            let pins = pin_db.lookup_many([owner.clone()]);
            for item in prepared
                .items
                .iter()
                .filter(|item| matches!(item, GridItem::ZipDir { .. }))
            {
                let request = scoped_request(
                    item,
                    &pins,
                    &HashMap::new(),
                    Some(&logical),
                    Some(physical),
                    &scope,
                    false,
                );
                let key = crate::thumb_loader::cache_key_for_request(&request)
                    .unwrap()
                    .into_owned();
                assert!(
                    key.contains(if physical == &logical {
                        "|autocrop:v4r:"
                    } else {
                        "|autocrop:v4:"
                    }),
                    "{key}"
                );
                assert!(prepared.existing_keys.contains(&key), "{key}");
                assert_eq!(request.path, *physical);
            }
        }
    }
}

#[test]
fn favorite_cover_auto_crop_history_prepare_and_synthetic_requests_use_item_location() {
    let mut app = setup_app_for_test();
    let inside = app.tmp.path().join("photos");
    std::fs::create_dir(&inside).unwrap();
    let paths = [inside.join("book.zip"), app.tmp.path().join("book.zip")];
    for path in &paths {
        std::fs::write(path, b"identity").unwrap();
    }
    app.settings.thumb_cover_auto_crop = true;
    app.settings.favorites = vec![favorite(&inside, Preference::Off)];
    let entries = paths
        .iter()
        .map(|path| {
            let mut entry = crate::reading_history_db::ReadingHistoryEntry::new(
                path.clone(),
                crate::reading_history_db::ReadingHistoryKind::Zip,
                None,
                "book".into(),
                None,
                None,
            );
            entry.file_size = Some(23);
            entry.mtime_ms = Some(17_000);
            entry
        })
        .collect();
    let prepared = app.prepare_reading_history_entries(entries).unwrap();
    let scope = CoverAutoCropScope::from_settings(&app.settings);
    for item in &prepared.inputs.items {
        let request = scoped_request(
            item,
            &HashMap::new(),
            &HashMap::new(),
            None,
            None,
            &scope,
            true,
        );
        let key = crate::thumb_loader::cache_key_for_request(&request).unwrap();
        assert!(prepared.existing_keys.contains(key.as_ref()));
    }
    // These synthetic list locations must never replace the logical item location.
    for surface in ["__reading_history__", "__rating_4__", "__collection__"] {
        app.current_folder = Some(PathBuf::from(surface));
        app.items_are_reading_history_view = surface == "__reading_history__";
        app.items_are_rating_view = surface == "__rating_4__";
        app.items = paths.iter().cloned().map(GridItem::ZipFile).collect();
        app.image_metas = vec![Some((17, 23)); 2];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.requested = Default::default();
        app.heavy_io_queue = Some(Arc::new((
            Mutex::new(Vec::new()),
            std::sync::Condvar::new(),
        )));
        for idx in 0..2 {
            assert!(matches!(
                app.enqueue_priority_thumbnail(idx),
                PriorityThumbnailAdmission::Enqueued
            ));
        }
        let requests = app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let key = crate::thumb_loader::cache_key_for_request(request).unwrap();
            assert_eq!(
                key.contains("|autocrop:"),
                request.idx == 1,
                "{surface}: {key}"
            );
        }
    }
    app.items_are_drive_list = true;
    app.settings.favorites[0].cover_auto_crop = Preference::On;
    assert!(
        app.thumbnail_auto_crop_scope()
            .identity_for(&paths[0])
            .is_none()
    );
}

#[test]
fn favorite_cover_auto_crop_collection_digest_and_reuse_change_with_scope() {
    use super::top_level_grid_view::*;
    let mut settings = crate::settings::Settings::default();
    settings
        .favorites
        .push(favorite(Path::new("C:/Photos"), Preference::On));
    let id = crate::collection_store::CollectionId::new();
    let reuse = |settings: &crate::settings::Settings| {
        CollectionGridPrepareReuseKey::new(id, 1, settings, None, None, 0)
    };
    let digest = |settings: &crate::settings::Settings| {
        let delivery = super::collection_grid::prepare_collection_grid_thumbnail_sources(
            CollectionGridThumbnailSources {
                cover_auto_crop_scope: CoverAutoCropScope::from_settings(settings),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        match delivery.presentation {
            CollectionGridPresentationSources::Retained(sources) => sources.identity,
            _ => panic!("small policy must be retained"),
        }
    };
    let before = (reuse(&settings), digest(&settings));
    settings.thumb_cover_auto_crop_binding = crate::settings::ReadingDirection::Ltr;
    assert_ne!(before.0, reuse(&settings));
    assert_ne!(before.1, digest(&settings));
    let binding = (reuse(&settings), digest(&settings));
    settings.favorites[0].cover_auto_crop = Preference::Off;
    assert_ne!(binding.0, reuse(&settings));
    assert_ne!(binding.1, digest(&settings));
}

#[test]
fn favorite_cover_auto_crop_folder_pin_samples_use_request_identity() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("book");
    let target = parent.join("target");
    std::fs::create_dir_all(&target).unwrap();
    image::DynamicImage::new_rgb8(892, 512)
        .save(target.join("000.png"))
        .unwrap();
    for index in 1..=8 {
        image::DynamicImage::new_rgb8(70, 100)
            .save(target.join(format!("{index:03}.png")))
            .unwrap();
    }
    let settings = crate::settings::Settings {
        thumb_cover_auto_crop_folders: true,
        favorites: vec![
            favorite(&parent, Preference::On),
            favorite(&target, Preference::Off),
        ],
        ..Default::default()
    };
    let scope = CoverAutoCropScope::from_settings(&settings);
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&parent),
        FolderPin::from(FolderPinSource::File {
            rel: "target".into(),
            kind: crate::folder_thumb_pins::FileKind::Folder,
        }),
    )]);
    let request = scoped_request(
        &GridItem::Folder(parent.clone()),
        &pins,
        &HashMap::new(),
        None,
        None,
        &scope,
        false,
    );
    assert_eq!(request.path, target);
    assert_eq!(request.auto_cover_crop, scope.identity_for(&parent));
    assert!(request.auto_cover_crop.is_some());
    assert!(scope.identity_for(&target).is_none());
    assert!(
        crate::thumb_loader::cache_key_for_request(&request)
            .unwrap()
            .contains("|autocrop:v4f:")
    );
    let resolved = crate::thumb_loader::resolve_read_after_cache_miss(&request, None, None);
    let automatic = resolved
        .auto_cover_crop
        .expect("the pinned folder retains its requesting tile's identity");
    assert_eq!(automatic.identity, request.auto_cover_crop.unwrap());
    let crate::thumb_loader::AutoCoverInput::FolderPages(pages) = automatic.input else {
        panic!("direct image sample")
    };
    assert_eq!(pages.len(), 9);
    assert_eq!(
        crate::cover_auto_sample::sample_folder_pages(&pages, &pages[0]),
        Ok(0.7)
    );
}

#[test]
fn favorite_cover_auto_crop_priority_batches_build_scope_once() {
    for navigation in [false, true] {
        let mut app = setup_app_for_test();
        app.settings.thumb_cover_auto_crop = true;
        let root = app.tmp.path().to_owned();
        app.settings.favorites = vec![favorite(&root.join("inside.zip"), Preference::Off)];
        app.items = ["inside.zip", "outside.zip"]
            .map(|name| GridItem::ZipFile(root.join(name)))
            .into();
        app.image_metas = vec![Some((17, 23)); 2];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.heavy_io_queue = Some(Arc::new((
            Mutex::new(Vec::new()),
            std::sync::Condvar::new(),
        )));
        let before = CoverAutoCropScope::build_count_for_test();
        let ctx = egui::Context::default();
        if navigation {
            app.ensure_navigation_target_thumbnail_requests(&ctx, &[0, 1]);
        } else {
            app.ensure_still_seek_thumbnail_requests(&ctx, &[0, 1]);
        }
        assert_eq!(CoverAutoCropScope::build_count_for_test() - before, 1);
        let queue = app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap();
        assert_eq!(queue.len(), 2);
        for request in queue.iter() {
            assert_eq!(request.auto_cover_crop.is_some(), request.idx == 1);
        }
    }
}
