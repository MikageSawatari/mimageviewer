//! Exercise DB publication, table catch-up, UI reconciliation and worker replacement together.
use super::*;
use crate::thumb_loader::{CacheDecision, ThumbLoadOrigin};

#[test]
fn converted_source_change_matches_logical_first_volume_and_normalized_candidate_keys() {
    use ConvertedArchiveSourceState as S;
    let app = setup_app_for_test();
    let db = app.archive_cache_db.as_ref().unwrap();
    let first = PathBuf::from("M:/Books/book.part1.rar");
    let cached = app.tmp.path().join("converted.zip");
    let mut sources = ConvertedArchiveSources::new(db.revision());
    for (key, state) in [
        ("m:/books/book.part2.rar", S::Direct(first.clone())),
        (
            "m:/books/alias.rar",
            S::CachedZip {
                logical_source: first.clone(),
                path: cached.clone(),
            },
        ),
        (
            "m:/books/unavailable.rar",
            S::Unavailable {
                logical_source: Some(first.clone()),
            },
        ),
        (
            "m:/books/wrapped.rar",
            S::Rar {
                volume: crate::rar_loader::RarVolumeProof::First,
                source: Box::new(S::Direct(first.clone())),
            },
        ),
        (
            "m:/books/book.part1.rar",
            S::Unavailable {
                logical_source: None,
            },
        ),
        (
            "m:/books/unrelated.rar",
            S::Direct(PathBuf::from("M:/Books/unrelated.rar")),
        ),
    ] {
        sources.insert(key.into(), state);
    }
    // src_path_key drops the drive; comparison must use that same normalization.
    db.record(
        Path::new("N:/BOOKS/book.part1.rar"),
        1,
        2,
        crate::archive_converter::ArchiveFormat::Rar,
        &cached,
        3,
        1,
        false,
    )
    .unwrap();
    assert!(sources.catch_up(Some(db)));
    assert_eq!(sources.synced_revision(), db.revision());
    for (key, state) in sources.iter() {
        if key.ends_with("unrelated.rar") {
            assert!(matches!(state, S::Direct(_)));
        } else {
            assert_eq!(state, &S::Pending, "{key}");
        }
    }
    assert!(!sources.catch_up(Some(db)));
}

fn request(app: &App) -> LoadRequest {
    let mut request = make_load_request_with_auto_crop(
        &app.items[0],
        0,
        7,
        9,
        false,
        None,
        Some(crate::settings::SortOrder::Numeric),
        0,
        &HashMap::new(),
        &app.converted_archive_cache_paths,
        None,
        None,
        None,
        None,
        false,
        &app.settings.file_type_visibility,
        configured_thumbnail_auto_crop(&app.settings),
    )
    .unwrap();
    request.items_gen = app.items_generation;
    request
}

fn run_worker(
    app: &mut App,
    ctx: &egui::Context,
    catalog: &Arc<crate::catalog::CatalogDb>,
    memory: &std::sync::RwLock<HashMap<String, crate::catalog::CacheEntry>>,
    force: bool,
) -> ThumbLoadOrigin {
    let mut req = request(app);
    req.force_cache = force;
    req.request_id = app
        .requested
        .begin_for_request(&req, ThumbnailRequestKind::Normal, None)
        .unwrap();
    let (tx, rx) = mpsc::channel();
    crate::thumb_loader::process_load_request(
        &mut req,
        memory,
        &tx,
        Some(catalog),
        64,
        75,
        64,
        CacheDecision {
            policy: crate::settings::CachePolicy::Auto,
            threshold_ms: u32::MAX,
            size_threshold: u64::MAX,
            webp_always: false,
            pdf_always: false,
            zip_always: false,
            cache_book_cover_only: false,
        },
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        None,
        None,
        None,
        None,
    );
    let mut origin = None;
    for msg in rx.try_iter() {
        if let Some(pixels) = msg.pixels() {
            origin = Some(pixels.origin);
        }
        app.tx.send(msg).unwrap();
    }
    app.poll_thumbnails(ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.thumbnails[0].has_pixels());
    origin.unwrap()
}

fn publish(app: &mut App, ctx: &egui::Context, source: &Path, physical: &Path, converted: bool) {
    let (tx, rx) = mpsc::channel();
    app.converted_archive_cache_paths_pending = Some(ConvertedArchiveCachePathsPending {
        generation: app.items_generation,
        cascade_depth: app.settings.folder_thumb_depth as usize,
        cancel: Default::default(),
        desired_indices: Default::default(),
        rx,
        pin_archive_dependencies: Default::default(),
    });
    tx.send(ConvertedArchiveCachePathsMsg::Resolved {
        archive_key: crate::path_key::normalize_keep_drive(source),
        state: if converted {
            ConvertedArchiveSourceState::CachedZip {
                logical_source: source.to_path_buf(),
                path: physical.to_path_buf(),
            }
        } else {
            ConvertedArchiveSourceState::Direct(physical.to_path_buf())
        },
        idx: 0,
        ordinal: 0,
        elapsed_ms: 0.0,
        input_seq: 0,
    })
    .unwrap();
    app.poll_converted_archive_cache_paths(ctx);
}

#[test]
fn converted_source_transition_save_convert_replace_and_delete_back_with_auto_off_and_on() {
    for automatic in [false, true] {
        for removal in ["delete", "prune", "lookup"] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            app.settings.thumb_cover_auto_crop = automatic;
            app.settings.thumb_aspect_auto = false;
            let original = app.tmp.path().join("original.rar");
            std::fs::copy(
                crate::cover_auto_sample::rar_tests::fixture_root().join("jacket.rar"),
                &original,
            )
            .unwrap();
            let converted = app.tmp.path().join("converted.zip");
            use std::io::Write;
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(12, 8)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&converted).unwrap());
            zip.start_file("000.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(png.get_ref()).unwrap();
            zip.finish().unwrap();
            let catalog = Arc::new(
                crate::catalog::CatalogDb::open(&app.tmp.path().join("thumbs"), app.tmp.path())
                    .unwrap(),
            );
            let memory = std::sync::RwLock::new(HashMap::new());
            let archive_key = crate::path_key::normalize_keep_drive(&original);
            app.items = vec![GridItem::ConvertibleArchive {
                path: original.clone(),
                format: crate::archive_converter::ArchiveFormat::Rar,
            }];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.image_metas = vec![Some((7, 9))];
            app.keep_set.insert(0);
            app.converted_archive_cache_paths.insert(
                archive_key.clone(),
                ConvertedArchiveSourceState::Direct(original.clone()),
            );
            run_worker(&mut app, &ctx, &catalog, &memory, true);
            let cache_key = request(&app).cache_key_override.unwrap();
            assert!(catalog.load_one(&cache_key).unwrap().is_some());
            let db = app.archive_cache_db.as_ref().unwrap().clone();
            db.record(
                &original,
                7,
                9,
                crate::archive_converter::ArchiveFormat::Rar,
                &converted,
                100,
                1,
                false,
            )
            .unwrap();
            app.catch_up_converted_archive_sources();
            assert!(matches!(
                app.converted_archive_cache_paths.get(&archive_key),
                Some(ConvertedArchiveSourceState::Pending)
            ));
            publish(&mut app, &ctx, &original, &converted, true);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(matches!(
                run_worker(&mut app, &ctx, &catalog, &memory, false),
                ThumbLoadOrigin::SourceGenerated { .. }
            ));
            let converted_page = crate::adjustment_db::zip_entry_key(&converted, "000.png");
            app.rotation_page_keys
                .insert(converted_page.clone(), crate::rotation_db::Rotation::Cw90);
            assert_eq!(
                app.grid_tile_rotation_memory(0),
                crate::rotation_db::Rotation::Cw90
            );
            assert_eq!(
                catalog.load_one(&cache_key).unwrap().unwrap().cover,
                crate::cover_identity::CoverIdentity::Page(converted_page)
            );
            match removal {
                "delete" => db.delete_entry(&original).unwrap(),
                "prune" => {
                    let _guard = db.begin_convert();
                    assert_eq!(
                        db.prune_to_size_limit_locked(1, &app.tmp.path().join("protected.rar"))
                            .unwrap(),
                        1
                    );
                }
                _ => {
                    assert!(db.lookup(&original, 8, 9).is_none());
                }
            }
            app.catch_up_converted_archive_sources();
            assert!(matches!(
                app.converted_archive_cache_paths.get(&archive_key),
                Some(ConvertedArchiveSourceState::Pending)
            ));
            publish(&mut app, &ctx, &original, &original, false);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(matches!(
                run_worker(&mut app, &ctx, &catalog, &memory, false),
                ThumbLoadOrigin::SourceGenerated { .. }
            ));
            assert_eq!(
                app.grid_tile_rotation_memory(0),
                crate::rotation_db::Rotation::None
            );
            assert_eq!(
                catalog.load_one(&cache_key).unwrap().unwrap().cover,
                crate::cover_identity::CoverIdentity::Page(crate::adjustment_db::zip_entry_key(
                    &original, "000.png"
                ))
            );
            app.evict_thumbnail_for_reload(0);
            memory.write().unwrap().clear();
            assert_eq!(
                run_worker(&mut app, &ctx, &catalog, &memory, false),
                ThumbLoadOrigin::UpgradeableCache
            );
        }
    }
}
