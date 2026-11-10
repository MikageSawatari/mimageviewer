use super::*;
use crate::app::ThumbnailPoolCatalog;

fn memory_resources(
    work: crate::catalog::CatalogWork,
) -> everything::metadata::FolderThumbnailResources {
    everything::metadata::FolderThumbnailResources {
        cache: Arc::new(std::sync::RwLock::new(HashMap::new())),
        catalog: None,
        catalog_work: work,
        sort: Default::default(),
        depth: 3,
    }
}

fn video_folder_pin_fixture(
    root: &Path,
    thumbnail_owner: SearchThumbnailOwner,
) -> (SearchMetadataResources, Vec<GridItem>, Vec<u8>) {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
    let folder = root.join("pinned-folder");
    std::fs::create_dir(&folder).unwrap();
    let video = folder.join("movie.mp4");
    std::fs::write(&video, b"video").unwrap();
    let pins = Arc::new(FolderThumbPinDb::open_at(&root.join("folder-pins.db")).unwrap());
    pins.set(
        &folder,
        &FolderPinSource::File {
            rel: "movie.mp4".into(),
            kind: FileKind::Video,
        },
    )
    .unwrap();
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .encode(&[19, 73, 127], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    let video_db = crate::video_pins::VideoPinDb::open_at(&root.join("video-pins.db")).unwrap();
    video_db.set_pin(&video, 1.0, &webp).unwrap();
    (
        SearchMetadataResources {
            thumbnail_owner,
            pin_db: Some(pins),
            video_pins: video_db.read_source(),
            tags_db_path: None,
        },
        vec![GridItem::Folder(folder)],
        webp,
    )
}

fn prepare_pin_metadata(
    resources: &SearchMetadataResources,
    items: &[GridItem],
    thumbnails: &everything::metadata::FolderThumbnailResources,
    cancel: &AtomicBool,
) -> SearchMetadataInstall {
    everything::metadata::prepare(
        items,
        &[None],
        resources,
        Some(thumbnails),
        cancel,
        &Default::default(),
    )
    .unwrap()
}

fn poll_metadata_until(
    app: &mut App,
    ctx: &egui::Context,
    reason: &str,
    ready: impl Fn(&App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready(app) {
        app.poll_global_search_events(ctx);
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::yield_now();
    }
}

fn queue_metadata_with_held_ready(app: &mut App, ctx: &egui::Context, path: &Path) {
    app.open_global_search();
    app.global_search.metadata_draft = "test".into();
    app.global_search.aggregate_auto = false;
    app.begin_search_page_edit_prepare_for_test(ctx);
    app.hold_next_search_ready_for_test();
    let (stream, rx) = crossbeam_channel::unbounded();
    app.global_search.pending = Some(everything::SearchProducer::Metadata(SearchHandle {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    }));
    stream
        .send(SearchStreamEvent::Batch {
            hits: vec![GlobalHit {
                kind: IndexKind::Image,
                path: path.to_string_lossy().into_owned(),
                score: 1.0,
                stars: 0,
                mtime: 0,
                file_size: Some(0),
            }],
            scanned_candidates: 1,
            valid_hits: 1,
            visibility_notice: None,
        })
        .unwrap();
    stream
        .send(SearchStreamEvent::Done {
            truncated: false,
            reason: DoneReason::Complete,
        })
        .unwrap();
    poll_metadata_until(
        app,
        ctx,
        "initial Metadata Ready was not held",
        App::search_ready_is_held_for_test,
    );
}

fn metadata_adopted(app: &App, path: &Path) -> bool {
    let prepare = app.global_search.page_edit_prepare.as_ref().unwrap();
    app.global_search.done
        && app.items == [GridItem::Image(path.to_path_buf())]
        && matches!(
            &app.global_search.accepted,
            everything::SearchAccepted::Metadata { applied: Some(spec) } if spec.query == "test"
        )
        && prepare.in_flight.is_none()
        && prepare.desired.is_none()
}

#[test]
fn section1741_metadata_current_owner_replaced_while_ready_pending_retries_and_adopts() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let image = app.tmp.path().join("result.jpg");
    super::tests::g1_publish_search_hits(&mut app, &ctx, std::slice::from_ref(&image));
    app.global_search.metadata_draft = "test".into();
    app.hold_next_search_ready_for_test();
    app.rebuild_items_from_global_search();
    poll_metadata_until(
        &mut app,
        &ctx,
        "Metadata Ready was not held before owner replacement",
        App::search_ready_is_held_for_test,
    );
    let generation = app.items_generation;
    let prepare_cancel = app.search_prepare_cancel_for_test();
    let replacement = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.current_color_cache_map = Some(Arc::clone(&replacement));
    let replacement_cancel = Arc::clone(&app.cancel_token);
    app.release_search_ready_for_test();
    poll_metadata_until(
        &mut app,
        &ctx,
        "Metadata retry kept using the retired thumbnail owner",
        |app| app.items_generation != generation && metadata_adopted(app, &image),
    );
    assert!(app.search_prepare_metrics_for_test().stale_rejections > 0);
    assert!(Arc::ptr_eq(
        app.current_color_cache_map.as_ref().unwrap(),
        &replacement
    ));
    assert!(Arc::ptr_eq(&app.cancel_token, &replacement_cancel));
    assert!(!replacement_cancel.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &app.search_prepare_cancel_for_test(),
        &prepare_cancel
    ));
    assert!(!prepare_cancel.load(Ordering::Relaxed));
}

#[test]
fn section1741_metadata_prepare_owner_replaced_before_ready_retries_and_adopts() {
    for had_pool in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        if had_pool {
            let origin = app.tmp.path().join("origin");
            std::fs::create_dir(&origin).unwrap();
            app.load_folder(origin);
            app.finish_page_identity_preparation_for_test();
        } else {
            app.enter_drive_list(None);
        }
        let image = app.tmp.path().join("result.jpg");
        queue_metadata_with_held_ready(&mut app, &ctx, &image);
        let replacement = Arc::new(std::sync::RwLock::new(HashMap::new()));
        app.install_search_thumbnail_workers(Arc::clone(&replacement), None);
        let replacement_cancel = Arc::clone(&app.cancel_token);
        app.release_search_ready_for_test();
        poll_metadata_until(
            &mut app,
            &ctx,
            "stale Metadata Prepare did not retry with the replacement owner",
            |app| metadata_adopted(app, &image),
        );
        assert!(app.search_prepare_metrics_for_test().stale_rejections > 0);
        assert!(Arc::ptr_eq(
            app.current_color_cache_map.as_ref().unwrap(),
            &replacement
        ));
        assert!(Arc::ptr_eq(&app.cancel_token, &replacement_cancel));
        assert!(!replacement_cancel.load(Ordering::Relaxed));
    }
}

#[test]
fn section1741_metadata_ready_after_cache_clear_recaptures_admission_and_adopts() {
    for initial_prepare in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let image = app.tmp.path().join("result.jpg");
        if initial_prepare {
            app.enter_drive_list(None);
            queue_metadata_with_held_ready(&mut app, &ctx, &image);
        } else {
            super::tests::g1_publish_search_hits(&mut app, &ctx, std::slice::from_ref(&image));
            app.global_search.metadata_draft = "test".into();
            app.hold_next_search_ready_for_test();
            app.rebuild_items_from_global_search();
            poll_metadata_until(
                &mut app,
                &ctx,
                "Metadata Ready was not held before cache clear",
                App::search_ready_is_held_for_test,
            );
        }
        let generation = app.items_generation;
        let prepare_cancel = app.search_prepare_cancel_for_test();
        let captured = SearchMetadataResources::for_app(&app);
        let work = crate::catalog::CatalogWork::default();
        crate::catalog::delete_all_cache(work.identity().0);
        assert!(
            !captured
                .thumbnail_owner
                .is_current(&SearchMetadataResources::for_app(&app).thumbnail_owner)
        );
        app.release_search_ready_for_test();
        poll_metadata_until(
            &mut app,
            &ctx,
            "Metadata retry kept using the catalog admission from before clear",
            |app| app.items_generation != generation && metadata_adopted(app, &image),
        );
        assert!(app.search_prepare_metrics_for_test().stale_rejections > 0);
        assert!(Arc::ptr_eq(
            &app.search_prepare_cancel_for_test(),
            &prepare_cancel
        ));
        assert!(!prepare_cancel.load(Ordering::Relaxed));
        assert_eq!(
            app.thumbnail_pool_catalog,
            ThumbnailPoolCatalog::View(crate::app::search_results_synthetic_path())
        );
    }
}

#[test]
fn section1741_nonsearch_pool_capture_never_borrows_outgoing_resources() {
    let mut app = crate::app::setup_app_for_test();
    let folder = app.tmp.path().join("origin");
    std::fs::create_dir(&folder).unwrap();
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    let token = Arc::clone(&app.cancel_token);
    for catalog in [
        ThumbnailPoolCatalog::Folder(folder),
        ThumbnailPoolCatalog::View(PathBuf::from("__rating_results__")),
        ThumbnailPoolCatalog::Collection,
    ] {
        app.thumbnail_pool_catalog = catalog;
        let resources = SearchMetadataResources::for_app(&app);
        assert!(resources.thumbnail_owner.folder_resources().is_none());
        match &resources.thumbnail_owner {
            SearchThumbnailOwner::Prepare {
                replaced: Some(replaced),
                ..
            } => {
                assert!(Arc::ptr_eq(replaced, &token));
            }
            _ => panic!("a non-search pool must prepare its own search resources"),
        }
        assert!(
            resources
                .thumbnail_owner
                .is_current(&SearchMetadataResources::for_app(&app).thumbnail_owner)
        );
    }
}

#[test]
fn section1741_prepare_checks_replaced_owner_and_settings() {
    let temp = tempfile::tempdir().unwrap();
    let request = SearchThumbnailInitRequest {
        catalog_work: crate::catalog::CatalogWork::capture(temp.path()),
        sort: Default::default(),
        depth: 3,
    };
    for replaced in [None, Some(Arc::new(AtomicBool::new(false)))] {
        let owner = SearchThumbnailOwner::Prepare {
            request: request.clone(),
            replaced: replaced.clone(),
        };
        assert!(owner.is_current(&owner.clone()));
        assert!(!owner.is_current(&SearchThumbnailOwner::Prepare {
            request: request.clone(),
            replaced: Some(Arc::new(AtomicBool::new(false))),
        }));
        let mut changed_request = request.clone();
        changed_request.depth += 1;
        assert!(!owner.is_current(&SearchThumbnailOwner::Prepare {
            request: changed_request,
            replaced,
        }));
        assert!(!owner.is_current(&SearchThumbnailOwner::Current {
            identity: Arc::new(AtomicBool::new(false)),
            resources: memory_resources(request.catalog_work.clone()),
        }));
    }
}

#[test]
fn section1741_current_catalog_failure_retains_pool_identity() {
    let mut app = crate::app::setup_app_for_test();
    let cache = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.enter_drive_list(None);
    app.install_search_thumbnail_workers(Arc::clone(&cache), None);
    let token = Arc::clone(&app.cancel_token);
    let resources = SearchMetadataResources::for_app(&app);
    assert!(matches!(&resources.thumbnail_owner,
        SearchThumbnailOwner::Current { identity, resources }
            if Arc::ptr_eq(identity, &token) && resources.catalog.is_none()
                && Arc::ptr_eq(&resources.cache, &cache)
    ));
    assert!(resources.thumbnail_owner.prepare().is_none());
    assert!(
        resources
            .thumbnail_owner
            .is_current(&SearchMetadataResources::for_app(&app).thumbnail_owner)
    );
    assert!(
        !resources
            .thumbnail_owner
            .is_current(&SearchThumbnailOwner::Current {
                identity: Arc::new(AtomicBool::new(false)),
                resources: resources
                    .thumbnail_owner
                    .folder_resources()
                    .unwrap()
                    .clone(),
            })
    );
    assert!(Arc::ptr_eq(
        app.current_color_cache_map.as_ref().unwrap(),
        &cache
    ));
    assert!(!token.load(Ordering::Relaxed));
}

#[test]
fn section1741_prepare_loads_search_catalog_instead_of_origin_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let request = SearchThumbnailInitRequest {
        catalog_work: crate::catalog::CatalogWork::capture(temp.path()),
        sort: Default::default(),
        depth: 3,
    };
    let search = request
        .catalog_work
        .open(&crate::app::search_results_synthetic_path())
        .unwrap();
    search
        .save(
            "search-row",
            11,
            22,
            1,
            1,
            None,
            &[1, 2, 3],
            &crate::cover_identity::CoverIdentity::Legacy,
        )
        .unwrap();
    let origin = request
        .catalog_work
        .open(&temp.path().join("origin"))
        .unwrap();
    origin
        .save(
            "origin-row",
            33,
            44,
            1,
            1,
            None,
            &[4, 5, 6],
            &crate::cover_identity::CoverIdentity::Legacy,
        )
        .unwrap();
    let owner = SearchThumbnailOwner::Prepare {
        request,
        replaced: Some(Arc::new(AtomicBool::new(false))),
    };
    let prepared = owner.prepare().unwrap();
    let cache = prepared.cache.read().unwrap();
    assert!(cache.contains_key("search-row"));
    assert!(!cache.contains_key("origin-row"));
    assert_eq!(origin.load_all().unwrap().len(), 1);
    assert!(origin.load_one("search-row").unwrap().is_none());
}

#[test]
fn section1741_prepare_catalog_failure_notifies_once_and_keeps_memory_owner() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache-is-a-file");
    std::fs::write(&cache_dir, b"keep this file").unwrap();
    let request = SearchThumbnailInitRequest {
        catalog_work: crate::catalog::CatalogWork::capture(&cache_dir),
        sort: Default::default(),
        depth: 3,
    };
    let access = request.catalog_work.access();
    assert!(access.take_cache_notice().is_none());
    let owner = SearchThumbnailOwner::Prepare {
        request,
        replaced: None,
    };
    let prepare_once = |owner: SearchThumbnailOwner| {
        std::thread::spawn(move || owner.prepare().unwrap())
            .join()
            .unwrap()
    };
    let prepared = prepare_once(owner.clone());
    assert!(prepared.catalog.is_none());
    assert!(prepared.cache.read().unwrap().is_empty());
    let folder_resources = owner.prepared_folder_resources(Some(&prepared)).unwrap();
    assert!(folder_resources.catalog.is_none());
    assert!(Arc::ptr_eq(&folder_resources.cache, &prepared.cache));
    assert!(
        access
            .take_cache_notice()
            .unwrap()
            .contains("search thumbnail catalog open")
    );
    let next = prepare_once(owner);
    assert!(next.catalog.is_none());
    assert!(next.cache.read().unwrap().is_empty());
    assert!(access.take_cache_notice().is_none());
    assert_eq!(std::fs::read(cache_dir).unwrap(), b"keep this file");
}

#[test]
fn section1741_prepare_catalog_open_failure_preserves_video_pin_seed_in_memory() {
    let _epochs = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache-is-a-file");
    std::fs::write(&cache_dir, b"keep this file").unwrap();
    let work = crate::catalog::CatalogWork::capture(&cache_dir);
    let access = work.access();
    let owner = SearchThumbnailOwner::Prepare {
        request: SearchThumbnailInitRequest {
            catalog_work: work,
            sort: Default::default(),
            depth: 3,
        },
        replaced: None,
    };
    let (resources, items, webp) = video_folder_pin_fixture(temp.path(), owner);
    let prepared = resources.thumbnail_owner.prepare().unwrap();
    let thumbnails = resources
        .thumbnail_owner
        .prepared_folder_resources(Some(&prepared))
        .unwrap();
    assert!(thumbnails.catalog.is_none());
    let metadata = prepare_pin_metadata(&resources, &items, &thumbnails, &AtomicBool::new(false));
    assert!(metadata.is_current(&resources));
    let cache = prepared.cache.read().unwrap();
    assert_eq!(cache.len(), 1);
    let entry = cache.values().next().unwrap();
    assert_eq!(entry.jpeg_data, webp);
    assert_eq!(
        entry.folder_provenance,
        Some(crate::catalog::FolderThumbProvenance::Seeded)
    );
    assert!(access.take_cache_notice().is_some());
    assert!(access.take_cache_notice().is_none());
    assert_eq!(std::fs::read(cache_dir).unwrap(), b"keep this file");
}

#[test]
fn section1741_seed_catalog_write_failure_preserves_memory_without_retry_or_stale_seed() {
    let _epochs = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let work = crate::catalog::CatalogWork::capture(&cache_dir);
    let access = work.access();
    let folder = crate::app::search_results_synthetic_path();
    let writer = work.open(&folder).unwrap();
    let readonly = Arc::new(
        crate::catalog::CatalogDb::open_existing_read_only(&cache_dir, &folder)
            .unwrap()
            .unwrap(),
    );
    assert!(readonly.is_read_only());
    let mut thumbnails = memory_resources(work);
    thumbnails.catalog = Some(Arc::clone(&readonly));
    let owner = SearchThumbnailOwner::Current {
        identity: Arc::new(AtomicBool::new(false)),
        resources: thumbnails.clone(),
    };
    let (resources, items, webp) = video_folder_pin_fixture(temp.path(), owner);
    let cancel = AtomicBool::new(false);
    let metadata = prepare_pin_metadata(&resources, &items, &thumbnails, &cancel);
    assert!(metadata.is_current(&resources));
    assert_eq!(
        thumbnails
            .cache
            .read()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .jpeg_data,
        webp
    );
    assert!(writer.load_all().unwrap().is_empty());
    assert!(
        access
            .take_cache_notice()
            .unwrap()
            .contains("search folder pin seed")
    );
    prepare_pin_metadata(&resources, &items, &thumbnails, &cancel);
    assert!(access.take_cache_notice().is_none());
    thumbnails.cache.write().unwrap().clear();
    cancel.store(true, Ordering::Relaxed);
    prepare_pin_metadata(&resources, &items, &thumbnails, &cancel);
    assert!(thumbnails.cache.read().unwrap().is_empty());
    cancel.store(false, Ordering::Relaxed);
    crate::catalog::delete_all_cache(&cache_dir);
    assert!(readonly.is_retired());
    prepare_pin_metadata(&resources, &items, &thumbnails, &cancel);
    assert!(thumbnails.cache.read().unwrap().is_empty());
    assert!(access.take_cache_notice().is_none());
}

#[cfg(windows)]
#[test]
fn section1741_search_pool_adoption_leaves_sibling_context_unchanged() {
    for everything in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let origin = app.tmp.path().join("origin");
        let sibling_folder = app.tmp.path().join("sibling");
        std::fs::create_dir(&origin).unwrap();
        std::fs::create_dir(&sibling_folder).unwrap();
        app.load_folder(origin);
        app.finish_page_identity_preparation_for_test();
        let main_cancel = Arc::clone(&app.cancel_token);
        let sibling = app.build_window_context_for_test(1741, |mounted| {
            mounted.load_folder(sibling_folder.clone());
            mounted.finish_page_identity_preparation_for_test();
        });
        let (marker, cancel, queue, heavy_queue, cache, catalog, generation, ticket, items) = app
            .with_window_context_for_test(sibling, |mounted| {
                (
                    mounted.thumbnail_pool_catalog.clone(),
                    Arc::clone(&mounted.cancel_token),
                    Arc::clone(mounted.reload_queue.as_ref().unwrap()),
                    Arc::clone(mounted.heavy_io_queue.as_ref().unwrap()),
                    Arc::clone(mounted.current_color_cache_map.as_ref().unwrap()),
                    Arc::clone(mounted.current_color_catalog.as_ref().unwrap()),
                    mounted.items_generation,
                    mounted.pdf_render_epoch.snapshot(),
                    mounted.items.clone(),
                )
            });
        assert_eq!(marker, ThumbnailPoolCatalog::Folder(sibling_folder));
        let image = app.tmp.path().join("result.jpg");
        std::fs::write(&image, b"image").unwrap();
        if everything {
            everything::tests::adopt(
                &mut app,
                &ctx,
                "result",
                everything::tests::dataset(vec![GridItem::Image(image)]),
            );
        } else {
            super::tests::g1_publish_search_hits(&mut app, &ctx, &[image]);
        }
        assert!(main_cancel.load(Ordering::Relaxed));
        assert_eq!(
            app.thumbnail_pool_catalog,
            ThumbnailPoolCatalog::View(crate::app::search_results_synthetic_path())
        );
        let search_cancel = Arc::clone(&app.cancel_token);
        let assert_sibling = |app: &mut App| {
            app.with_window_context_for_test(sibling, |mounted| {
                assert_eq!(mounted.thumbnail_pool_catalog, marker);
                assert!(Arc::ptr_eq(&mounted.cancel_token, &cancel));
                assert!(!cancel.load(Ordering::Relaxed));
                assert!(Arc::ptr_eq(mounted.reload_queue.as_ref().unwrap(), &queue));
                assert!(Arc::ptr_eq(
                    mounted.heavy_io_queue.as_ref().unwrap(),
                    &heavy_queue
                ));
                assert!(Arc::ptr_eq(
                    mounted.current_color_cache_map.as_ref().unwrap(),
                    &cache
                ));
                assert!(Arc::ptr_eq(
                    mounted.current_color_catalog.as_ref().unwrap(),
                    &catalog
                ));
                assert_eq!(mounted.items_generation, generation);
                assert_eq!(mounted.items, items);
                assert!(mounted.requested.is_empty());
                assert!(ticket.is_current());
                assert!(cache.read().unwrap().is_empty());
                assert!(catalog.load_all().unwrap().is_empty());
            });
        };
        assert_sibling(&mut app);
        app.close_global_search();
        app.finish_history_restore_preparation_for_test();
        app.finish_page_identity_preparation_for_test();
        assert!(search_cancel.load(Ordering::Relaxed));
        assert_sibling(&mut app);
    }
}
