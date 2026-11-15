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
                    &format!("zipthumb:book{index}.zip|autocrop:v3:rtl"),
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
        let key = format!("zipthumb:book{index}.zip|autocrop:v3:rtl");
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
        let parent_key = format!("folderthumb:auto-v3:parent{index}|autocrop:v3:rtl");
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
fn auto_material_k2_prefers_v3_and_legacy_fallback_requires_missing_exact_v3() {
    use crate::cover_auto_material::AutoMaterial;
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let directory = temp.path().join("books");
    std::fs::create_dir_all(&directory).unwrap();
    let writer = crate::catalog::CatalogDb::open(&cache, &directory).unwrap();
    let key = "zipthumb:book.zip|autocrop:v3:rtl#pin:cover";
    let old = "zipthumb:book.zip|autocrop:v1:rtl#pin:cover";
    let opposite = "zipthumb:book.zip|autocrop:v1:ltr#pin:cover";
    let bytes = fixture_webp();
    writer
        .save(
            "zipthumb:book.zip|autocrop:v3:ltr#pin:cover",
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
    for old_v2 in [
        "zipthumb:book.zip|autocrop:v2:rtl#pin:cover",
        "zipthumb:book.zip|autocrop:v2:ltr#pin:cover",
    ] {
        writer
            .save(
                old_v2,
                7,
                9,
                8,
                8,
                None,
                &bytes,
                &CoverIdentity::Page("unreleased-v2".into()),
                auto_material_sample(),
            )
            .unwrap();
    }
    assert!(
        auto_material_candidate(&cache, &directory, key).is_none(),
        "unreleased v2 rows must not be migrated into current v3 material"
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
        "an existing v3 row with missing material must not resurrect v1"
    );
    // Existing v3 with a stale stamp or invalid raster is a miss; it must not
    // resurrect old v1 material merely because the v3 row cannot be used.
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
    let parent_key = "folderthumb:auto-v3:books|autocrop:v3:rtl";
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
