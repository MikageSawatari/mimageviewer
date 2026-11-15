// Included in thumb_loader::tests so these regressions exercise the real K2
// selection, worker transport and existing disposable source fixtures.

fn auto_material_sample() -> crate::cover_auto_material::AutoMaterial {
    crate::cover_auto_material::AutoMaterial::Automatic(
        crate::cover_auto_material::AutoCropResults::new([
            Some(manual_cover_crop([160_000, 0, 560_000, 1_000_000])),
            None,
            Some(manual_cover_crop([440_000, 0, 840_000, 1_000_000])),
            None,
        ]),
    )
}

#[test]
fn cover_rotation_k2_keeps_parent_until_normal_reread_and_child_save() {
    use std::io::Write;
    use crate::cover_rotation_cache::{BookCoverId, CatalogScope, InvalidationRequest, SourceHint};
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let folder = temp.path().join("books");
    std::fs::create_dir(&folder).unwrap();
    let a = folder.join("a.zip");
    let b = folder.join("b.zip");
    let mut cover_bytes = std::io::Cursor::new(Vec::new());
    auto_cover_jacket().write_to(&mut cover_bytes, image::ImageFormat::Png).unwrap();
    let mut body_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(70, 100).write_to(&mut body_bytes, image::ImageFormat::Png).unwrap();
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&a).unwrap());
    for index in 0..9 {
        archive.start_file(format!("{index:03}.png"), zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(if index == 0 { cover_bytes.get_ref() } else { body_bytes.get_ref() }).unwrap();
    }
    archive.finish().unwrap();
    // B is intentionally not decodable: K2 may consume its finished row only.
    std::fs::write(&b, b"cached B source").unwrap();
    let catalog = Arc::new(crate::catalog::CatalogDb::open(&cache, &folder).unwrap());
    let pixels = fixture_webp();
    for path in [&a, &b] {
        let meta = std::fs::metadata(path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        catalog.save(&format!("zipthumb:{name}|autocrop:v4:rtl"), crate::ui_helpers::mtime_secs(&meta), meta.len() as i64,
            8, 8, None, &pixels, &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(path, "000.png")), auto_material_sample()).unwrap();
    }
    let auto = Some(crate::cover_auto_crop::AutoCropIdentity::new(crate::settings::ReadingDirection::Rtl));
    let select = || select_folder_thumb_image_with_policy_and_auto(&folder, crate::settings::SortOrder::Numeric, 3,
        None, &cache, None, &Default::default(), auto);
    let (first, proof) = select();
    let Some(FolderThumbResolution::CachedPinned(first)) = first else { panic!("A must be the initial cached child") };
    assert_eq!(first.source_path, a);
    let parent = crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap();
    let parent_key = representative_cache_key_for_policy_with_auto(
        &folder_thumb_auto_cache_key_for_path(&folder, false, crate::settings::SortOrder::Numeric, 3).unwrap(),
        &Default::default(), auto,
    );
    parent.save_auto_folder_bytes(&parent_key, 1, 0, first.source_dims, first.layout_dims,
        &first.webp_data, proof.as_ref(), &first.cover, first.auto_material).unwrap();
    let parent_request = LoadRequest { path: folder.clone(), cache_key_override: Some(parent_key.clone()),
        folder_thumb_provenance: Some(crate::catalog::FolderThumbProvenance::AutoSelected), ..Default::default() };
    let previous_parent = parent.load_one(&parent_key).unwrap().unwrap();
    assert!(folder_cached_row_usable_in_cache_dir(&parent_request, &previous_parent, &cache));
    let mut parent_hint = SourceHint::new(BookCoverId::Folder(folder.clone()));
    parent_hint.representative_page_keys.push(crate::adjustment_db::zip_entry_key(&a, "000.png"));
    let completion = crate::cover_rotation_cache::run_invalidation(InvalidationRequest {
        changed_keys: vec![crate::adjustment_db::zip_entry_key(&a, "001.png")],
        source_hints: vec![SourceHint::new(BookCoverId::Zip { path: a.clone(), prefix: None }), parent_hint],
        scopes: vec![CatalogScope { folder: temp.path().to_owned(), memory: vec![] }],
        work: crate::catalog::CatalogWork::capture(&cache), archive_cache: None,
    });
    assert!(completion.errors.is_empty(), "{:?}", completion.errors);
    assert_eq!(completion.affected_slots.len(), 1);
    assert!(catalog.load_one("zipthumb:a.zip|autocrop:v4:rtl").unwrap().is_none());
    assert!(parent.load_one(&parent_key).unwrap().is_some(), "write-side deletion never removes K2 parent rows");
    assert!(!folder_cached_row_usable_in_cache_dir(&parent_request, &previous_parent, &cache));
    let (replacement, replacement_proof) = select();
    let Some(FolderThumbResolution::CachedPinned(replacement)) = replacement else { panic!("cold A must be skipped in favor of cached B") };
    assert_eq!(replacement.source_path, b);
    parent.save_auto_folder_bytes(&parent_key, 1, 0, replacement.source_dims, replacement.layout_dims,
        &replacement.webp_data, replacement_proof.as_ref(), &replacement.cover, replacement.auto_material).unwrap();
    let b_parent = parent.load_one(&parent_key).unwrap().unwrap();
    assert!(folder_cached_row_usable_in_cache_dir(&parent_request, &b_parent, &cache));
    // A normal restart or a policy which has not saved A leaves the same cold
    // fact. Merely selecting the parent never starts a source job or saves A.
    drop(crate::catalog::CatalogDb::open_existing_read_only(&cache, &folder).unwrap().unwrap());
    let (again, _) = select();
    assert!(matches!(again, Some(FolderThumbResolution::CachedPinned(ref selected)) if selected.source_path == b));
    assert!(catalog.load_one("zipthumb:a.zip|autocrop:v4:rtl").unwrap().is_none());
    let meta = std::fs::metadata(&a).unwrap();
    let mut normal = LoadRequest { path: a.clone(), zip_entry: Some("000.png".into()),
        cache_key_override: Some("zipthumb:a.zip".into()), source_policy: LoadSourcePolicy::SourceOnly,
        mtime: crate::ui_helpers::mtime_secs(&meta), file_size: meta.len() as i64, ..Default::default() };
    normal.set_auto_cover_crop(auto);
    let generated = run_cover_crop_request(&mut normal, &catalog, None);
    assert!(generated.iter().any(|message| message.pixels().is_some()));
    assert!(catalog.load_one("zipthumb:a.zip|autocrop:v4:rtl").unwrap().is_some());
    assert!(!folder_cached_row_usable_in_cache_dir(&parent_request, &b_parent, &cache), "ordinary child save invalidates the existing selection proof");
    let (restored, _) = select();
    assert!(matches!(restored, Some(FolderThumbResolution::CachedPinned(ref selected)) if selected.source_path == a));
}

fn auto_material_candidate(
    cache: &Path,
    directory: &Path,
    key: &str,
) -> Option<CachedPinnedFolderThumb> {
    let policy = crate::file_type_visibility::FileTypeVisibility::default();
    let mut context = FolderResolveContext {
        cache_dir: cache,
        policy: &policy,
        auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
            crate::settings::ReadingDirection::Rtl,
        )),
        pin_db: None,
        cancel: None,
        catalogs: HashMap::new(),
        dependencies: Vec::new(),
        pin_store: None,
    };
    context.cached_candidate(directory, key, &directory.join("book.zip"), 7, 9)
}

fn auto_material_assert_roi(
    material: crate::cover_auto_material::AutoMaterial,
    size: [usize; 2],
    target: u32,
) {
    let crate::cover_auto_material::AutoMaterial::Automatic(results) = material else {
        panic!("expected automatic material");
    };
    let mut count = 0;
    for crop in results.crops().into_iter().flatten() {
        let (_, _, width, height) = crop.pixel_bounds(size[0] as u32, size[1] as u32);
        assert!(
            width.max(height) >= target,
            "ROI {width}x{height}, target={target}, full={size:?}"
        );
        count += 1;
    }
    assert!(
        count > 0,
        "fixture must exercise at least one selected crop"
    );
}

#[test]
fn auto_material_kinds_preserve_save_reopen_k2_copy_and_send() {
    use crate::cover_auto_material::AutoMaterial;
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let child = temp.path().join("child");
    let parent = temp.path().join("parent");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&parent).unwrap();
    let bytes = fixture_webp();
    let cover = CoverIdentity::Page("synthetic-cover-page".into());
    let materials = [
        auto_material_sample(),
        AutoMaterial::NotAutomatic,
        AutoMaterial::LegacyV1,
    ];
    {
        let catalog = crate::catalog::CatalogDb::open(&cache, &child).unwrap();
        for (index, material) in materials.iter().copied().enumerate() {
            catalog
                .save_with_layout_dims(
                    &format!("zipthumb:book{index}.zip|autocrop:v4:rtl"),
                    7,
                    9,
                    8,
                    8,
                    Some((892, 512)),
                    Some((892_000, 512_000)),
                    &bytes,
                    &cover,
                    material,
                )
                .unwrap();
        }
    }
    for (index, material) in materials.into_iter().enumerate() {
        let key = format!("zipthumb:book{index}.zip|autocrop:v4:rtl");
        let reopened = crate::catalog::CatalogDb::open_existing_read_only(&cache, &child)
            .unwrap()
            .unwrap();
        assert_eq!(
            reopened.load_one(&key).unwrap().unwrap().auto_material,
            material
        );
        let selected = auto_material_candidate(&cache, &child, &key).unwrap();
        assert_eq!(selected.auto_material, material);
        assert_eq!(selected.webp_data, bytes);
        let parent_key = format!("folderthumb:auto-v3:parent{index}|autocrop:v4:rtl");
        {
            let writer = crate::catalog::CatalogDb::open(&cache, &parent).unwrap();
            assert!(
                writer
                    .save_auto_folder_bytes(
                        &parent_key,
                        7,
                        9,
                        selected.source_dims,
                        selected.layout_dims,
                        &selected.webp_data,
                        None,
                        &selected.cover,
                        selected.auto_material
                    )
                    .unwrap()
            );
        }
        let copied = crate::catalog::CatalogDb::open_existing_read_only(&cache, &parent)
            .unwrap()
            .unwrap()
            .load_one(&parent_key)
            .unwrap()
            .unwrap();
        assert_eq!(copied.jpeg_data, bytes);
        assert_eq!(copied.auto_material, material);
        assert_eq!(copied.source_dims, Some((892, 512)));
        assert_eq!(copied.layout_dims, Some((892_000, 512_000)));
        let expected_image = crate::catalog::decode_thumb_to_color_image(&bytes).unwrap();
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        send_cached_cover(
            3,
            4,
            5,
            ThumbnailRequestId::Registered(6),
            copied,
            None,
            &tx,
            &done,
        );
        let message = rx.recv().unwrap();
        let pixels = message.pixels().unwrap();
        assert_eq!(pixels.auto_material, material);
        assert_eq!(pixels.cover, cover);
        assert_eq!(pixels.image.size, expected_image.size);
        assert_eq!(pixels.image.pixels, expected_image.pixels);
        assert_eq!(message.request_id, ThumbnailRequestId::Registered(6));
        assert_eq!(done.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn auto_material_k2_prefers_v4_and_legacy_fallback_requires_missing_exact_v4() {
    use crate::cover_auto_material::AutoMaterial;
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let directory = temp.path().join("books");
    std::fs::create_dir_all(&directory).unwrap();
    let writer = crate::catalog::CatalogDb::open(&cache, &directory).unwrap();
    let key = "zipthumb:book.zip|autocrop:v4:rtl#pin:cover";
    let old = "zipthumb:book.zip|autocrop:v1:rtl#pin:cover";
    let opposite = "zipthumb:book.zip|autocrop:v1:ltr#pin:cover";
    let bytes = fixture_webp();
    writer
        .save(
            "zipthumb:book.zip|autocrop:v4:ltr#pin:cover",
            7,
            9,
            8,
            8,
            None,
            &bytes,
            &CoverIdentity::Legacy,
            AutoMaterial::NotAutomatic,
        )
        .unwrap();
    writer
        .save(
            "zipthumb:book.zip|autocrop:v1:rtl#pin:another-page",
            7,
            9,
            8,
            8,
            None,
            &bytes,
            &CoverIdentity::Legacy,
            AutoMaterial::NotAutomatic,
        )
        .unwrap();
    assert!(auto_material_candidate(&cache, &directory, key).is_none());
    for old_key in [
        "zipthumb:book.zip|autocrop:v2:rtl#pin:cover",
        "zipthumb:book.zip|autocrop:v2:ltr#pin:cover",
        "zipthumb:book.zip|autocrop:v3:rtl#pin:cover",
        "zipthumb:book.zip|autocrop:v3:ltr#pin:cover",
    ] {
        writer
            .save(
                old_key,
                7,
                9,
                8,
                8,
                None,
                &bytes,
                &CoverIdentity::Page("previous-version".into()),
                auto_material_sample(),
            )
            .unwrap();
    }
    assert!(
        auto_material_candidate(&cache, &directory, key).is_none(),
        "v2 and v3 rows must not be migrated into current v4 material"
    );
    writer.save(opposite, 7, 9, 8, 8, None, &bytes,
        &CoverIdentity::Page("old-ltr".into()), AutoMaterial::NotAutomatic).unwrap();
    let legacy = auto_material_candidate(&cache, &directory, key).unwrap();
    assert_eq!(legacy.auto_material, AutoMaterial::LegacyV1);
    assert_eq!(legacy.cover, CoverIdentity::Page("old-ltr".into()));
    // A stale preferred v1 must not hide the valid opposite-binding row.
    writer.save(old, 6, 9, 8, 8, None, &bytes,
        &CoverIdentity::Page("stale-rtl".into()), AutoMaterial::NotAutomatic).unwrap();
    let legacy = auto_material_candidate(&cache, &directory, key).unwrap();
    assert_eq!(legacy.auto_material, AutoMaterial::LegacyV1);
    assert_eq!(legacy.cover, CoverIdentity::Page("old-ltr".into()));
    writer
        .save(
            old,
            7,
            9,
            8,
            8,
            None,
            &bytes,
            &CoverIdentity::Page("old-rtl".into()),
            AutoMaterial::NotAutomatic,
        )
        .unwrap();
    let legacy = auto_material_candidate(&cache, &directory, key).unwrap();
    assert_eq!(legacy.auto_material, AutoMaterial::LegacyV1);
    assert_eq!(legacy.cover, CoverIdentity::Page("old-rtl".into()));
    assert_eq!(legacy.webp_data, bytes);
    writer
        .save(
            key,
            7,
            9,
            8,
            8,
            None,
            &bytes,
            &CoverIdentity::Legacy,
            auto_material_sample(),
        )
        .unwrap();
    assert_eq!(
        auto_material_candidate(&cache, &directory, key)
            .unwrap()
            .auto_material,
        auto_material_sample()
    );
    let raw = rusqlite::Connection::open(crate::catalog::db_path_for(&cache, &directory)).unwrap();
    raw.execute(
        "UPDATE thumbnails SET auto_crops=NULL WHERE filename=?1",
        [key],
    )
    .unwrap();
    assert!(
        auto_material_candidate(&cache, &directory, key).is_none(),
        "an existing v4 row with missing material must not resurrect v1"
    );
    // Existing v4 with a stale stamp or invalid raster is a miss; it must not
    // resurrect old v1 material merely because the v4 row cannot be used.
    writer
        .save(
            key,
            8,
            9,
            8,
            8,
            None,
            &bytes,
            &CoverIdentity::Legacy,
            auto_material_sample(),
        )
        .unwrap();
    assert!(auto_material_candidate(&cache, &directory, key).is_none());
    writer
        .save(
            key,
            7,
            9,
            8,
            8,
            None,
            b"invalid raster",
            &CoverIdentity::Legacy,
            auto_material_sample(),
        )
        .unwrap();
    assert!(auto_material_candidate(&cache, &directory, key).is_none());
}

#[test]
fn auto_material_v4_source_rebuilds_v3_with_preexisting_saved_body_rotation() {
    use crate::cover_auto_material::AutoMaterial;
    use crate::rotation_db::{Rotation, RotationDb};
    use std::io::{Cursor, Write};
    let data = crate::data_dir::TestDataDirGuard::new();
    let folder = data.path().join("folder");
    folder_auto_fixture(&folder);
    let zip_path = data.path().join("book.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    for page in 0..=8 {
        let mut bytes = Cursor::new(Vec::new());
        let image = if page == 0 {
            auto_cover_jacket()
        } else {
            image::DynamicImage::new_rgb8(70, 100)
        };
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        zip.start_file(format!("{page:03}.png"), zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes.get_ref()).unwrap();
    }
    zip.finish().unwrap();
    let rotations = RotationDb::open().unwrap();
    // These rotations already exist before the first v4 request: no rotation
    // change notification or write-side invalidation can repair the old rows.
    for page in 1..=8 {
        rotations.set_key(
            &crate::adjustment_db::normalize_path(&folder.join(format!("{page:03}.png"))),
            Rotation::Cw90,
        ).unwrap();
        rotations.set_key(
            &crate::adjustment_db::zip_entry_key(&zip_path, &format!("{page:03}.png")),
            Rotation::Cw90,
        ).unwrap();
    }
    for is_folder in [false, true] {
        let catalog = Arc::new(crate::catalog::CatalogDb::open(
            &data.path().join(if is_folder { "cache-folder" } else { "cache-zip" }),
            data.path(),
        ).unwrap());
        let base = if is_folder { "folderthumb:folder" } else { "zipthumb:book.zip" };
        let old_key = format!("{base}|autocrop:{}:rtl", if is_folder { "v3f" } else { "v3" });
        catalog.save(&old_key, 7, 9, 8, 8, None, &fixture_webp(),
            &CoverIdentity::Legacy, auto_material_sample()).unwrap();
        let old = catalog.load_one(&old_key).unwrap().unwrap();
        let mut request = LoadRequest {
            path: if is_folder { folder.clone() } else { zip_path.clone() },
            zip_entry: (!is_folder).then(|| "000.png".into()),
            cache_key_override: Some(base.into()),
            source_policy: LoadSourcePolicy::CacheOnly,
            force_cache: true,
            mtime: 7,
            file_size: 9,
            ..Default::default()
        };
        request.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity {
            binding: crate::settings::ReadingDirection::Rtl,
            folders: is_folder,
        }));
        let key = cache_key_for_request(&request).unwrap().into_owned();
        assert_eq!(key, format!("{base}|autocrop:{}:rtl", if is_folder { "v4f" } else { "v4" }));
        assert!(run_cover_crop_request(&mut request, &catalog, None)
            .iter().all(|message| message.pixels().is_none()));
        request.source_policy = LoadSourcePolicy::CacheOrSource;
        let map = std::sync::RwLock::new(catalog.load_all().unwrap());
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        process_load_request(
            &mut request, &map, &tx, Some(&catalog), 64, 75, 64,
            make_decision(CachePolicy::Always, 0, 0), &done,
            &Arc::new(Mutex::new(crate::stats::ThumbStats::default())), None,
            &Arc::new(AtomicUsize::new(0)), &Arc::new(AtomicUsize::new(1)),
            None, None, None, None, None,
        );
        assert_eq!(done.load(Ordering::Relaxed), 1);
        let pixels = rx.try_iter().find_map(ThumbMsg::into_pixels).unwrap();
        let AutoMaterial::Automatic(results) = pixels.auto_material else {
            panic!("expected regenerated automatic material");
        };
        assert!(results.crops().iter().all(Option::is_none), "landscape body must decline");
        assert_eq!(catalog.load_one(&key).unwrap().unwrap().auto_material, pixels.auto_material);
        let retained = catalog.load_one(&old_key).unwrap().unwrap();
        assert_eq!(retained.jpeg_data, old.jpeg_data);
        assert_eq!(retained.auto_material, old.auto_material);
    }
}

#[test]
fn auto_material_k2_missing_binding_setting_keeps_parent_from_old_ltr_child() {
    let temp = TempDir::new().unwrap();
    let prefs = crate::settings_db::SettingsDb::create_new(&temp.path().join("prefs")).unwrap();
    prefs.save_full(&crate::settings::Settings {
        thumb_cover_auto_crop: true,
        default_reading_direction: crate::settings::ReadingDirection::Ltr,
        ..Default::default()
    }).unwrap();
    rusqlite::Connection::open(temp.path().join("prefs/settings.db")).unwrap()
        .execute("DELETE FROM settings_kv WHERE key='thumb_cover_auto_crop_binding'", []).unwrap();
    let settings = prefs.load_into_settings().unwrap();
    assert_eq!(settings.thumb_cover_auto_crop_binding, crate::settings::ReadingDirection::Rtl);
    let folder = temp.path().join("books");
    std::fs::create_dir_all(&folder).unwrap();
    let book = folder.join("book.zip");
    std::fs::write(&book, b"old bytes").unwrap();
    let meta = std::fs::metadata(&book).unwrap();
    let cache = temp.path().join("cache");
    let writer = crate::catalog::CatalogDb::open(&cache, &folder).unwrap();
    let bytes = fixture_webp();
    writer.save("zipthumb:book.zip|autocrop:v1:ltr", crate::ui_helpers::mtime_secs(&meta), meta.len() as i64,
        8, 8, None, &bytes, &CoverIdentity::Page("old-ltr".into()), crate::cover_auto_material::AutoMaterial::NotAutomatic).unwrap();
    let (selected, _) = select_folder_thumb_image_with_policy_and_auto(
        &folder, crate::settings::SortOrder::Numeric, 3, None, &cache, None,
        &Default::default(), crate::app::configured_thumbnail_auto_crop(&settings));
    let Some(FolderThumbResolution::CachedPinned(selected)) = selected else { panic!("parent representative must survive the upgrade"); };
    assert_eq!(selected.auto_material, crate::cover_auto_material::AutoMaterial::LegacyV1);
    assert_eq!(selected.webp_data, bytes);
}

#[test]
fn auto_material_main_tile_cache_only_does_not_read_v1_compatibility_row() {
    use crate::cover_auto_material::AutoMaterial;
    let temp = TempDir::new().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    catalog
        .save(
            "zipthumb:book.zip|autocrop:v1:rtl",
            7,
            9,
            8,
            8,
            None,
            &fixture_webp(),
            &CoverIdentity::Legacy,
            AutoMaterial::NotAutomatic,
        )
        .unwrap();
    let mut request = LoadRequest {
        path: temp.path().join("book.zip"),
        zip_entry: Some("000.png".into()),
        cache_key_override: Some("zipthumb:book.zip".into()),
        mtime: 7,
        file_size: 9,
        source_policy: LoadSourcePolicy::CacheOnly,
        ..Default::default()
    };
    request.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity::new(
        crate::settings::ReadingDirection::Rtl,
    )));
    let messages = run_cover_crop_request(&mut request, &catalog, None);
    assert!(messages.iter().all(|message| message.pixels().is_none()));
    assert!(
        messages
            .iter()
            .any(|message| matches!(message.payload, ThumbMsgPayload::CacheOnlyMiss))
    );
    // SourceOnly also cannot adopt v1: the missing source must remain a failure.
    request.source_policy = LoadSourcePolicy::SourceOnly;
    let messages = run_cover_crop_request(&mut request, &catalog, None);
    assert!(messages.iter().all(|message| message.pixels().is_none()));
}

#[test]
fn auto_material_generated_and_cached_roi_use_distinct_display_and_storage_targets() {
    use std::io::Write;
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let directory = temp.path().join("books");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("book.zip");
    let mut cover = std::io::Cursor::new(Vec::new());
    auto_cover_jacket()
        .write_to(&mut cover, image::ImageFormat::Png)
        .unwrap();
    let mut page = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(70, 100)
        .write_to(&mut page, image::ImageFormat::Png)
        .unwrap();
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for index in 0..9 {
        archive
            .start_file(
                format!("{index:03}.png"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(if index == 0 {
                cover.get_ref()
            } else {
                page.get_ref()
            })
            .unwrap();
    }
    archive.finish().unwrap();
    let catalog = Arc::new(crate::catalog::CatalogDb::open(&cache, &directory).unwrap());
    let mut request = LoadRequest {
        path: path.clone(),
        zip_entry: Some("000.png".into()),
        cache_key_override: Some("zipthumb:book.zip".into()),
        source_policy: LoadSourcePolicy::SourceOnly,
        force_cache: true,
        mtime: 7,
        file_size: 9,
        ..Default::default()
    };
    request.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity::new(
        crate::settings::ReadingDirection::Rtl,
    )));
    let key = cache_key_for_request(&request).unwrap().into_owned();
    let generated = run_cover_crop_request_with_size(&mut request, &catalog, None, 256);
    let pixels = generated.iter().find_map(ThumbMsg::pixels).unwrap();
    auto_material_assert_roi(pixels.auto_material, pixels.image.size, 256);
    assert_eq!(pixels.source_dims, Some((892, 512)));
    let material = pixels.auto_material;
    drop(catalog);
    let reopened = Arc::new(crate::catalog::CatalogDb::open(&cache, &directory).unwrap());
    let stored = reopened.load_one(&key).unwrap().unwrap();
    assert_eq!(stored.auto_material, material);
    assert_eq!(stored.source_dims, Some((892, 512)));
    let stored_image = crate::catalog::decode_thumb_to_color_image(&stored.jpeg_data).unwrap();
    auto_material_assert_roi(material, stored_image.size, 64);
    request.source_policy = LoadSourcePolicy::CacheOnly;
    let cached = run_cover_crop_request_with_size(&mut request, &reopened, None, 256);
    let pixels = cached.iter().find_map(ThumbMsg::pixels).unwrap();
    assert_eq!(pixels.auto_material, material);
    auto_material_assert_roi(material, pixels.image.size, 64);
    let selected = auto_material_candidate(&cache, &directory, &key).unwrap();
    assert_eq!(selected.auto_material, material);
    auto_material_assert_roi(selected.auto_material, selected.image.size, 64);
    let parent = crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap();
    let parent_key = "folderthumb:auto-v3:books|autocrop:v4:rtl";
    assert!(
        parent
            .save_auto_folder_bytes(
                parent_key,
                7,
                9,
                selected.source_dims,
                selected.layout_dims,
                &selected.webp_data,
                None,
                &selected.cover,
                selected.auto_material
            )
            .unwrap()
    );
    let copied = parent.load_one(parent_key).unwrap().unwrap();
    assert_eq!(copied.jpeg_data, stored.jpeg_data);
    let (tx, rx) = mpsc::channel();
    send_cached_cover(
        0,
        0,
        0,
        ThumbnailRequestId::Standalone,
        copied,
        None,
        &tx,
        &Arc::new(AtomicUsize::new(0)),
    );
    let message = rx.recv().unwrap();
    auto_material_assert_roi(
        message.pixels().unwrap().auto_material,
        message.pixels().unwrap().image.size,
        64,
    );
}

#[test]
fn auto_material_raster_target_caps_full_image_at_4096() {
    use crate::cover_auto_material::{AutoCropResults, AutoMaterial};
    let tiny = manual_cover_crop([0, 0, 10_000, 10_000]);
    let material = AutoMaterial::Automatic(AutoCropResults::new([Some(tiny), None, None, None]));
    assert_eq!(material_raster_target((8000, 4000), material, 256), 4096);
    let broad = manual_cover_crop([0, 0, 1_000_000, 500_000]);
    let smallest_is_last =
        AutoMaterial::Automatic(AutoCropResults::new([Some(broad), None, None, Some(tiny)]));
    assert_eq!(
        material_raster_target((8000, 4000), smallest_is_last, 256),
        4096
    );
    assert_eq!(
        material_raster_target((8000, 4000), AutoMaterial::NotAutomatic, 256),
        256
    );
    assert_eq!(
        material_raster_target((8000, 4000), AutoMaterial::LegacyV1, 256),
        256
    );
    let image = image::DynamicImage::new_rgb8(8192, 64);
    let target = material_raster_target((8192, 64), material, 256);
    let display = resize_to_display_color_image(&image, target, Some((8192, 64)));
    assert_eq!(display.size, [4096, 32]);
    let (bytes, width, height) = crate::catalog::encode_thumb_webp(&image, target, 75.0).unwrap();
    assert_eq!((width, height), (4096, 32));
    assert_eq!(
        crate::catalog::decode_thumb_to_color_image(&bytes)
            .unwrap()
            .size,
        display.size
    );
}
