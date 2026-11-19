fn converted_source_run(
    request: &mut LoadRequest,
    catalog: &Arc<crate::catalog::CatalogDb>,
    map: &std::sync::RwLock<HashMap<String, crate::catalog::CacheEntry>>,
    preview: Option<&Arc<crate::edit_preview_cache::EditPreviewCacheDb>>,
) -> Vec<ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    process_load_request(
        request,
        map,
        &tx,
        Some(catalog),
        64,
        75,
        64,
        CacheDecision {
            policy: CachePolicy::Auto,
            threshold_ms: u32::MAX,
            size_threshold: u64::MAX,
            webp_always: false,
            pdf_always: false,
            zip_always: false,
            cache_book_cover_only: false,
        },
        &done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        None,
        preview,
        None,
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    rx.try_iter().collect()
}

fn converted_source_zip(path: &Path) {
    use std::io::Write;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(12, 8)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("000.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(png.get_ref()).unwrap();
    zip.finish().unwrap();
}

fn converted_source_factory_fallback_dispatch(zipdir: bool, stamp_failure: bool) {
    use crate::app::ConvertedArchiveSourceState;
    use crate::folder_thumb_pins::FolderPinSource;
    use crate::grid_item::GridItem;

    let _data = crate::data_dir::TestDataDirGuard::new();
    for to_converted in [false, true] {
        // None also checks newly decoded pixels carry the fallback's own binding.
        for memory_hit in [None, Some(false), Some(true)] {
            let temp = TempDir::new().unwrap();
            let original = temp.path().join("original.rar");
            std::fs::copy(
                crate::cover_auto_sample::rar_tests::fixture_root().join("jacket.rar"),
                &original,
            )
            .unwrap();
            let converted = temp.path().join("converted.zip");
            converted_source_zip(&converted);
            let (old, current) = if to_converted {
                (&original, &converted)
            } else {
                (&converted, &original)
            };
            let archive_key = crate::path_key::normalize_keep_drive(&original);
            let sources = HashMap::from([(
                archive_key.clone(),
                if to_converted {
                    ConvertedArchiveSourceState::CachedZip {
                        logical_source: original.clone(),
                        path: converted.clone(),
                    }
                } else {
                    ConvertedArchiveSourceState::Direct(original.clone())
                },
            )]);
            let policy = crate::file_type_visibility::FileTypeVisibility {
                excluded_extensions: ["zip".into()].into_iter().collect(),
                ..Default::default()
            };
            let pins = HashMap::from([(
                archive_key.clone(),
                FolderPinSource::ZipEntry {
                    zip_rel: String::new(),
                    entry: "inner.zip/page.png".into(),
                }
                .into(),
            )]);
            let mut request = crate::app::converted_fallback_request_for_test(
                &GridItem::ConvertibleArchive {
                    path: original.clone(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                &pins,
                &sources,
                None,
                &policy,
            );
            assert!(request.epub_pin_fallback.is_some());
            assert!(!policy_archive_pin_visible(&request, None));
            if zipdir {
                // ZipDir's factory does not currently create epub_pin_fallback.
                // Compose a factory-generated ZipDir base with the real outer pin
                // to exercise the handler's source-fact transport independently.
                request.epub_pin_fallback = Some(Box::new(
                    crate::app::converted_fallback_request_for_test(
                        &GridItem::ZipDir {
                            zip_path: original.clone(),
                            dir_prefix: String::new(),
                            is_archive: false,
                            representative: Some("000.png".into()),
                        },
                        &HashMap::new(),
                        &sources,
                        None,
                        &policy,
                    ),
                ));
            }
            let fallback = request.epub_pin_fallback.as_ref().unwrap();
            assert_eq!(fallback.path, *current);
            assert_eq!(
                fallback.converted_source,
                Some(ConvertedSourceRequest {
                    archive_key: archive_key.clone(),
                    expected_archive: Some(current.clone()),
                })
            );
            let fallback_key = cache_key_for_request(fallback).unwrap().into_owned();
            // The visibility projection publishes under both the outer pin key
            // and outer stamp; stamp-error dispatch retains the fallback stamp.
            let (mtime, file_size) = if stamp_failure {
                (fallback.mtime, fallback.file_size)
            } else {
                (request.mtime, request.file_size)
            };
            let key = if stamp_failure {
                // The stamp-error dispatch normally serves folder/EPUB pins.
                // Compose its failed outer stamp with the same factory fallback
                // to check the second dispatch preserves source ownership too.
                request.path = temp.path().join("missing.epub");
                request.zip_entry = None;
                request.resolve_override = None;
                request.pdf_page = Some(0);
                request.pdf_stamp_policy = PdfStampPolicy::ResolveInWorker;
                fallback_key
            } else {
                // Excluded pins publish the automatic projection at the pin key.
                cache_key_for_request(&request).unwrap().into_owned()
            };
            let catalog = Arc::new(
                crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap(),
            );
            if memory_hit.is_some() {
                catalog
                    .save_thumb_bytes_with_layout_dims(
                        &key,
                        mtime,
                        file_size,
                        None,
                        None,
                        &fixture_webp(),
                        &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(old, "000.png")),
                        crate::cover_auto_material::AutoMaterial::NotAutomatic,
                    )
                    .unwrap();
            }
            let map = std::sync::RwLock::new(if memory_hit == Some(true) {
                catalog.load_all().unwrap()
            } else {
                HashMap::new()
            });
            assert!(!request.force_cache);
            let messages = converted_source_run(&mut request, &catalog, &map, None);
            let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
            assert!(
                matches!(pixels.origin, ThumbLoadOrigin::SourceGenerated { .. }),
                "zipdir={zipdir} stamp_failure={stamp_failure} converted={to_converted} memory={memory_hit:?}"
            );
            let expected_binding = ConvertedSourceBinding {
                archive_key: archive_key.clone(),
                read_archive: crate::adjustment_db::normalize_path(current),
            };
            assert_eq!(
                pixels.converted_source_binding.as_ref(),
                Some(&expected_binding)
            );
            assert_eq!(
                pixels.cover,
                CoverIdentity::Page(crate::adjustment_db::zip_entry_key(current, "000.png"))
            );
            if memory_hit.is_some() {
                // A mismatched row must be replaced below the normal save limits.
                let stored = catalog.load_one(&key).unwrap().unwrap();
                assert_eq!(stored.cover, pixels.cover);
                map.write().unwrap().clear();
                let messages = converted_source_run(&mut request, &catalog, &map, None);
                let cached = messages.iter().find_map(ThumbMsg::pixels).unwrap();
                assert_eq!(cached.origin, ThumbLoadOrigin::UpgradeableCache);
                assert_eq!(
                    cached.converted_source_binding.as_ref(),
                    Some(&expected_binding)
                );
                assert_eq!(cached.cover, stored.cover);
            }
        }
    }
}

#[test]
fn converted_source_factory_archive_visibility_fallback_preserves_own_source() {
    converted_source_factory_fallback_dispatch(false, false);
}

#[test]
fn converted_source_factory_archive_stamp_failure_fallback_preserves_own_source() {
    converted_source_factory_fallback_dispatch(false, true);
}

#[test]
fn converted_source_factory_zipdir_composed_visibility_fallback_preserves_own_source() {
    converted_source_factory_fallback_dispatch(true, false);
}

#[test]
fn converted_source_factory_zipdir_composed_stamp_failure_fallback_preserves_own_source() {
    converted_source_factory_fallback_dispatch(true, true);
}

#[test]
fn converted_source_worker_replaces_memory_catalog_zipdir_and_pin_rows_both_directions() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    for to_converted in [false, true] {
        for memory_hit in [false, true] {
            for route in ["archive", "zipdir", "pin"] {
                for automatic in [false, true] {
                    let temp = TempDir::new().unwrap();
                    let original = temp.path().join("original.rar");
                    std::fs::copy(
                        crate::cover_auto_sample::rar_tests::fixture_root().join("jacket.rar"),
                        &original,
                    )
                    .unwrap();
                    let converted = temp.path().join("converted.zip");
                    converted_source_zip(&converted);
                    let (old, current) = if to_converted {
                        (&original, &converted)
                    } else {
                        (&converted, &original)
                    };
                    let key = match route {
                        "pin" => "folderthumb:book#pin:archive",
                        "zipdir" => "zipdir:",
                        _ => "archivethumb:rar:original.rar",
                    };
                    let catalog = Arc::new(
                        crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path())
                            .unwrap(),
                    );
                    let mut request = LoadRequest {
                        path: current.clone(),
                        converted_source: Some(ConvertedSourceRequest {
                            archive_key: "original.rar".into(),
                            expected_archive: Some(current.clone()),
                        }),
                        zip_entry: (route != "zipdir").then(|| "000.png".into()),
                        zip_dir_prefix: (route == "zipdir").then(String::new),
                        resolve_override: (route == "zipdir")
                            .then_some(ResolveStrategy::ZipDirRepresentative),
                        folder_thumb_provenance: (route == "pin")
                            .then_some(crate::catalog::FolderThumbProvenance::Seeded),
                        cache_key_override: Some(key.into()),
                        mtime: 7,
                        file_size: 9,
                        ..Default::default()
                    };
                    request.set_auto_cover_crop(automatic.then(|| {
                        crate::cover_auto_crop::AutoCropIdentity::new(
                            crate::settings::ReadingDirection::Rtl,
                        )
                    }));
                    let key = cache_key_for_request(&request).unwrap().into_owned();
                    catalog
                        .save_thumb_bytes_with_layout_dims(
                            &key,
                            7,
                            9,
                            Some((8, 8)),
                            None,
                            &fixture_webp(),
                            &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(
                                old, "000.png",
                            )),
                            crate::cover_auto_material::AutoMaterial::NotAutomatic,
                        )
                        .unwrap();
                    let map = std::sync::RwLock::new(if memory_hit {
                        catalog.load_all().unwrap()
                    } else {
                        HashMap::new()
                    });
                    let messages = converted_source_run(&mut request, &catalog, &map, None);
                    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
                    assert!(
                        matches!(pixels.origin, ThumbLoadOrigin::SourceGenerated { .. }),
                        "{route}: memory={memory_hit} converted={to_converted}"
                    );
                    let binding = pixels.converted_source_binding.as_ref().unwrap();
                    assert_eq!(binding.archive_key, "original.rar");
                    assert_eq!(
                        binding.read_archive,
                        crate::adjustment_db::normalize_path(current)
                    );
                    let stored = catalog.load_one(&key).unwrap().unwrap();
                    assert_eq!(stored.cover, pixels.cover);
                    assert_eq!(
                        stored.cover,
                        CoverIdentity::Page(crate::adjustment_db::zip_entry_key(
                            current, "000.png"
                        ))
                    );
                    // The regenerated source is below every threshold and force_cache is false.
                    // Its replacement must nevertheless be permanent and immediately reusable.
                    map.write().unwrap().clear();
                    let messages = converted_source_run(&mut request, &catalog, &map, None);
                    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
                    assert_eq!(pixels.origin, ThumbLoadOrigin::UpgradeableCache);
                    assert_eq!(pixels.converted_source_binding.as_ref(), Some(binding));
                    if automatic && route == "archive" {
                        use crate::cover_rotation_cache::{
                            BookCoverId, CatalogScope, InvalidationRequest, SourceHint,
                        };
                        let mut hint = SourceHint::new(BookCoverId::Archive(original.clone()));
                        hint.aliases.push(converted.clone());
                        let CoverIdentity::Page(page) = &stored.cover else {
                            panic!("source cover");
                        };
                        hint.representative_page_keys.push(page.clone());
                        let completion =
                            crate::cover_rotation_cache::run_invalidation(InvalidationRequest {
                                changed_keys: vec![crate::adjustment_db::zip_entry_key(
                                    current, "001.png",
                                )],
                                source_hints: vec![hint],
                                scopes: vec![CatalogScope {
                                    folder: temp.path().to_owned(),
                                    memory: Vec::new(),
                                }],
                                work: crate::catalog::CatalogWork::capture(
                                    &temp.path().join("cache"),
                                ),
                                archive_cache: None,
                            });
                        assert!(completion.errors.is_empty(), "{:?}", completion.errors);
                        assert!(
                            catalog.load_one(&key).unwrap().is_none(),
                            "body rotation must invalidate replacement {key}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn converted_source_pending_cache_only_carries_old_binding_from_memory_and_catalog() {
    let temp = TempDir::new().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let old = temp.path().join("removed-converted.zip");
    let cover = CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&old, "a::b/000.png"));
    catalog
        .save_thumb_bytes_with_layout_dims(
            "archivethumb:rar:book.rar",
            7,
            9,
            None,
            None,
            &fixture_webp(),
            &cover,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    for memory in [false, true] {
        let map = std::sync::RwLock::new(if memory {
            catalog.load_all().unwrap()
        } else {
            HashMap::new()
        });
        for expected in [None, Some(temp.path().join("direct.rar"))] {
            let mut request = LoadRequest {
                path: temp.path().join("direct.rar"),
                source_policy: LoadSourcePolicy::CacheOnly,
                converted_source: Some(ConvertedSourceRequest {
                    archive_key: "book.rar".into(),
                    expected_archive: expected,
                }),
                cache_key_override: Some("archivethumb:rar:book.rar".into()),
                mtime: 7,
                file_size: 9,
                ..Default::default()
            };
            let messages = converted_source_run(&mut request, &catalog, &map, None);
            let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
            assert_eq!(pixels.cover, cover);
            assert_eq!(
                pixels.converted_source_binding,
                Some(ConvertedSourceBinding {
                    archive_key: "book.rar".into(),
                    read_archive: crate::adjustment_db::normalize_path(&old),
                })
            );
        }
    }
}

#[test]
fn converted_source_restored_drive_and_resolved_first_volume_use_physical_cover() {
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    let original_folder = Path::new("M:/books");
    let catalog = crate::catalog::CatalogDb::open(&cache, original_folder).unwrap();
    let cover = CoverIdentity::Page("m:/books/book.part1.rar::000.png".into());
    catalog
        .save_thumb_bytes_with_layout_dims(
            "archivethumb:rar:book.part2.rar",
            7,
            9,
            None,
            None,
            &fixture_webp(),
            &cover,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    let encoded: String =
        rusqlite::Connection::open(crate::catalog::db_path_for(&cache, original_folder))
            .unwrap()
            .query_row("SELECT cover_key FROM thumbnails", [], |row| row.get(0))
            .unwrap();
    assert_eq!(encoded, "q:/books/book.part1.rar::000.png");
    drop(catalog);
    let catalog = Arc::new(crate::catalog::CatalogDb::open(&cache, Path::new("N:/books")).unwrap());
    let mut request = LoadRequest {
        path: PathBuf::from("N:/books/book.part1.rar"),
        converted_source: Some(ConvertedSourceRequest {
            archive_key: "n:/books/book.part2.rar".into(),
            expected_archive: Some(PathBuf::from("N:/books/book.part1.rar")),
        }),
        cache_key_override: Some("archivethumb:rar:book.part2.rar".into()),
        mtime: 7,
        file_size: 9,
        ..Default::default()
    };
    let messages = converted_source_run(
        &mut request,
        &catalog,
        &std::sync::RwLock::new(HashMap::new()),
        None,
    );
    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
    assert_eq!(pixels.origin, ThumbLoadOrigin::UpgradeableCache);
    assert_eq!(
        pixels
            .converted_source_binding
            .as_ref()
            .unwrap()
            .read_archive,
        "n:/books/book.part1.rar"
    );
}

#[test]
fn converted_source_external_cover_and_relocated_cache_use_restored_physical_path() {
    let temp = TempDir::new().unwrap();
    let folder = Path::new("M:/books");
    let catalog = Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), folder).unwrap());
    let new_cache = temp.path().join("new-cache.zip");
    converted_source_zip(&new_cache);
    for (old, should_hit) in [(PathBuf::from("X:/old-cache/cache.zip"), false), (new_cache.clone(), true)] {
        let key = "archivethumb:rar:relocated.rar";
        catalog.save_thumb_bytes_with_layout_dims(
            key, 7, 9, None, None, &fixture_webp(),
            &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&old, "000.png")),
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        ).unwrap();
        let encoded: String = rusqlite::Connection::open(crate::catalog::db_path_for(&temp.path().join("cache"), folder))
            .unwrap().query_row("SELECT cover_key FROM thumbnails WHERE filename=?1", [key], |row| row.get(0)).unwrap();
        assert!(encoded.starts_with("p:"), "external cache must keep its physical archive");
        let mut req = LoadRequest {
            path: new_cache.clone(), zip_entry: Some("000.png".into()),
            cache_key_override: Some(key.into()), mtime: 7, file_size: 9,
            converted_source: Some(ConvertedSourceRequest { archive_key: "m:/books/relocated.rar".into(), expected_archive: Some(new_cache.clone()) }),
            ..Default::default()
        };
        let map = std::sync::RwLock::new(HashMap::new());
        let messages = converted_source_run(&mut req, &catalog, &map, None);
        let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
        assert_eq!(pixels.origin.from_cache(), should_hit);
        assert_eq!(pixels.converted_source_binding.as_ref().unwrap().read_archive, crate::adjustment_db::normalize_path(&new_cache));
        let next = converted_source_run(&mut req, &catalog, &map, None);
        assert_eq!(next.iter().find_map(ThumbMsg::pixels).unwrap().origin, ThumbLoadOrigin::UpgradeableCache);
    }
}

#[test]
fn converted_source_edit_preview_early_return_carries_archive_binding() {
    let data = crate::data_dir::TestDataDirGuard::new();
    let archive = data.path().join("converted.zip");
    let key = crate::adjustment_db::zip_entry_key(&archive, "000.png");
    let previews = crate::edit_preview_cache::EditPreviewCacheService::open().unwrap();
    previews.save(
        key.clone(),
        7,
        9,
        None,
        crate::page_dims::CanonicalPageDimensions::new(8, 8),
        None,
        Arc::new(egui::ColorImage::filled([8, 8], egui::Color32::GREEN)),
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
            Ok(_) => {}
            Err(mpsc::TryRecvError::Empty) => {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => panic!("preview service: {error}"),
        }
    }
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&data.path().join("cache"), data.path()).unwrap());
    let mut request = LoadRequest {
        path: archive.clone(),
        zip_entry: Some("000.png".into()),
        edit_preview_key: Some(key),
        mtime: 7,
        file_size: 9,
        converted_source: Some(ConvertedSourceRequest {
            archive_key: "book.rar".into(),
            expected_archive: Some(archive.clone()),
        }),
        ..Default::default()
    };
    let messages = converted_source_run(
        &mut request,
        &catalog,
        &std::sync::RwLock::new(HashMap::new()),
        Some(&previews.db()),
    );
    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
    assert!(pixels.from_edit_preview);
    assert_eq!(
        pixels
            .converted_source_binding
            .as_ref()
            .unwrap()
            .read_archive,
        crate::adjustment_db::normalize_path(&archive)
    );
}

#[test]
fn converted_source_excluded_pin_fallback_keeps_valid_folder_rows_and_has_no_binding() {
    let data = crate::data_dir::TestDataDirGuard::new();
    let root = data.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    image::RgbImage::from_pixel(12, 8, image::Rgb([30, 80, 150]))
        .save(root.join("cover.png"))
        .unwrap();
    let archive = root.join("book.rar");
    std::fs::copy(
        crate::cover_auto_sample::rar_tests::fixture_root().join("jacket.rar"),
        &archive,
    )
    .unwrap();
    let policy = crate::file_type_visibility::FileTypeVisibility {
        excluded_extensions: ["zip".into()].into_iter().collect(),
        ..Default::default()
    };
    let sources = HashMap::from([(
        crate::path_key::normalize_keep_drive(&archive),
        crate::app::ConvertedArchiveSourceState::Direct(archive),
    )]);
    let pins = HashMap::from([(
        crate::path_key::normalize_keep_drive(&root),
        crate::folder_thumb_pins::FolderPinSource::ZipEntry {
            zip_rel: "book.rar".into(),
            entry: "inner.zip/page.png".into(),
        }
        .into(),
    )]);
    let mut request = crate::app::converted_fallback_request_for_test(
        &crate::grid_item::GridItem::Folder(root.clone()),
        &pins,
        &sources,
        None,
        &policy,
    );
    assert!(request.converted_source.is_some());
    assert_eq!(
        request.epub_pin_fallback.as_ref().unwrap().converted_source,
        None,
        "the folder factory already owns no converted source"
    );
    request.force_cache = true;
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&data.path().join("catalog"), &root).unwrap());
    let map = std::sync::RwLock::new(HashMap::new());
    let generated = converted_source_run(&mut request, &catalog, &map, None);
    let generated = generated.iter().find_map(ThumbMsg::pixels).unwrap();
    assert!(matches!(
        generated.origin,
        ThumbLoadOrigin::SourceGenerated { .. }
    ));
    assert_eq!(generated.converted_source_binding, None);
    // The ordinary fallback path must hit its valid saved row, rather than applying
    // the converted archive's expectation to the folder representative.
    let cached = converted_source_run(&mut request, &catalog, &map, None);
    let cached = cached.iter().find_map(ThumbMsg::pixels).unwrap();
    assert_eq!(cached.origin, ThumbLoadOrigin::UpgradeableCache);
    assert_eq!(cached.converted_source_binding, None);
    // Pending's cache-only path reaches that same automatic projection without
    // resolving visibility from the archive. Its persisted row is the branch fact.
    request.source_policy = LoadSourcePolicy::CacheOnly;
    request.converted_source.as_mut().unwrap().expected_archive = None;
    let cached = converted_source_run(&mut request, &catalog, &map, None);
    let cached = cached.iter().find_map(ThumbMsg::pixels).unwrap();
    assert_eq!(cached.origin, ThumbLoadOrigin::UpgradeableCache);
    assert_eq!(cached.converted_source_binding, None);
}

#[test]
fn converted_source_legacy_and_unrelated_rows_remain_unbound() {
    let temp = TempDir::new().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    for cover in [
        CoverIdentity::Legacy,
        CoverIdentity::Page("c:/unrelated.zip::000.png".into()),
    ] {
        catalog
            .save_thumb_bytes_with_layout_dims(
                "cover",
                7,
                9,
                None,
                None,
                &fixture_webp(),
                &cover,
                crate::cover_auto_material::AutoMaterial::NotAutomatic,
            )
            .unwrap();
        let mut request = LoadRequest {
            path: temp.path().join("book.rar"),
            mtime: 7,
            file_size: 9,
            cache_key_override: Some("cover".into()),
            source_policy: LoadSourcePolicy::CacheOnly,
            converted_source: (cover == CoverIdentity::Legacy).then(|| ConvertedSourceRequest {
                archive_key: "book.rar".into(),
                expected_archive: None,
            }),
            ..Default::default()
        };
        let cached = converted_source_run(
            &mut request,
            &catalog,
            &std::sync::RwLock::new(HashMap::new()),
            None,
        );
        assert_eq!(
            cached
                .iter()
                .find_map(ThumbMsg::pixels)
                .unwrap()
                .converted_source_binding,
            None
        );
    }
}
