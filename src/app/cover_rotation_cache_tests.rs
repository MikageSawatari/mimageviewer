use super::*;
use crate::cover_auto_material::{AutoCropResults, AutoMaterial};
use crate::cover_identity::CoverIdentity;
use crate::cover_rotation_cache::{
    AutoCoverSlot, BookCoverId, CoverCacheStamp, InvalidationCompletion,
};
use crate::cover_source_scope::LoadedCoverIdentity;
use crate::thumb_loader::{LoadRequest, ThumbLoadOrigin, ThumbMsg};

fn automatic_material() -> AutoMaterial {
    // Declining every direction is still automatic material and must be tracked.
    AutoMaterial::Automatic(AutoCropResults::new([None; 4]))
}

fn slot(path: &str) -> AutoCoverSlot {
    let path = PathBuf::from(path);
    AutoCoverSlot::new(
        BookCoverId::from_grid_item(&GridItem::ZipFile(path.clone())).unwrap(),
        crate::adjustment_db::zip_entry_key(&path, "cover.png"),
    )
}

fn loaded(ctx: &egui::Context, slot: &AutoCoverSlot, stamp: CoverCacheStamp) -> ThumbnailState {
    let mut state = ThumbnailState::loaded_raster(
        ctx.load_texture(
            "automatic-cover-test",
            egui::ColorImage::filled([2, 1], egui::Color32::WHITE),
            Default::default(),
        ),
        ThumbLoadOrigin::FinalCache,
        false,
        2,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((2, 1)),
        None,
    );
    if let ThumbnailState::Loaded {
        cover,
        auto_material,
        cover_cache_stamp,
        ..
    } = &mut state
    {
        *cover = LoadedCoverIdentity::Page(slot.representative_page_key.clone());
        *auto_material = automatic_material();
        *cover_cache_stamp = stamp;
    }
    state
}

fn stamp(state: &ThumbnailState) -> &CoverCacheStamp {
    let ThumbnailState::Loaded {
        cover_cache_stamp, ..
    } = state
    else {
        panic!("loaded material expected")
    };
    cover_cache_stamp
}

fn begin_automatic(app: &mut App, idx: usize) -> LoadRequest {
    let GridItem::ZipFile(path) = &app.items[idx] else {
        panic!("ZIP fixture expected")
    };
    let mut request = LoadRequest {
        idx,
        path: path.clone(),
        zip_entry: Some("cover.png".into()),
        items_gen: app.items_generation,
        auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
            crate::settings::ReadingDirection::Rtl,
        )),
        ..Default::default()
    };
    request.request_id = app
        .requested
        .begin_for_request(&request, ThumbnailRequestKind::Normal, None)
        .unwrap();
    request
}

fn pixels(request: &LoadRequest) -> ThumbMsg {
    ThumbMsg::from_legacy_parts(
        request.idx,
        Some(egui::ColorImage::filled([2, 1], egui::Color32::WHITE)),
        ThumbLoadOrigin::FinalCache,
        false,
        None,
        Some((2, 1)),
        None,
        false,
        false,
        0,
        request.items_gen,
    )
    .with_request_id(request.request_id)
    .with_cover(CoverIdentity::Page(crate::adjustment_db::zip_entry_key(
        &request.path,
        "cover.png",
    )))
    .with_auto_material(automatic_material())
}

#[test]
fn cover_cache_acceptance_stamps_declined_automatic_material_and_clone_preserves_stamp() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.items = vec![GridItem::ZipFile(PathBuf::from("c:/adoption.zip"))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![Some((1, 1))];
    app.visible_indices = vec![0];
    app.keep_set.insert(0);
    app.keep_range = (0, 1);
    app.settings.thumb_aspect_auto = false;
    let request = begin_automatic(&mut app, 0);
    app.send_thumbnail_for_test(pixels(&request)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    let cloned = app.thumbnails[0].clone();
    assert_eq!(
        stamp(&app.thumbnails[0]),
        &CoverCacheStamp::Tracked {
            slot: Arc::new(slot("c:/adoption.zip")),
            serial: 0,
        }
    );
    assert_eq!(stamp(&cloned), stamp(&app.thumbnails[0]));
    assert!(app.cover_rotation_cache.is_current(stamp(&cloned)));
}

#[test]
fn cover_cache_restored_old_material_evicts_once_and_removes_aspect_sample() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let target = slot("c:/book.zip");
    let sibling = slot("c:/sibling.zip");
    app.items = vec![
        GridItem::ZipFile(PathBuf::from("c:/book.zip")),
        GridItem::ZipFile(PathBuf::from("c:/sibling.zip")),
    ];
    let old = loaded(
        &ctx,
        &target,
        app.cover_rotation_cache.stamp(target.clone()),
    );
    app.thumbnails = vec![
        old.clone(),
        loaded(
            &ctx,
            &sibling,
            app.cover_rotation_cache.stamp(sibling.clone()),
        ),
    ];
    app.cover_rotation_cache
        .complete_for_test(InvalidationCompletion {
            affected_slots: vec![target],
            ..Default::default()
        });
    app.poll_cover_rotation_cache(&ctx);
    // Restoring a retained snapshot bypasses worker acceptance and keeps its old stamp.
    app.thumbnails[0] = old;
    let cached = |page: String, auto_material| crate::catalog::CacheEntry {
        mtime: 7,
        file_size: 9,
        jpeg_data: vec![1, 2, 3],
        cover: CoverIdentity::Page(page),
        auto_material,
        source_dims: Some((2, 1)),
        layout_dims: None,
        folder_provenance: None,
        selection_proof: None,
    };
    let memory = Arc::new(std::sync::RwLock::new(HashMap::from([
        (
            "zipthumb:book.zip|autocrop:v4:rtl".to_owned(),
            cached(
                slot("c:/book.zip").representative_page_key,
                automatic_material(),
            ),
        ),
        (
            "zipthumb:sibling.zip|autocrop:v4:rtl".to_owned(),
            cached(
                sibling.representative_page_key.clone(),
                automatic_material(),
            ),
        ),
        (
            "zipthumb:book.zip#pin:manual".to_owned(),
            cached(
                slot("c:/book.zip").representative_page_key,
                AutoMaterial::NotAutomatic,
            ),
        ),
    ])));
    app.thumbnail_pool_catalog = ThumbnailPoolCatalog::Folder(PathBuf::from("c:/"));
    app.current_color_cache_map = Some(memory.clone());
    app.auto_aspect.samples.insert(0, 0.5);
    app.auto_aspect.samples.insert(1, 0.5);
    assert!(app.reconcile_restored_cover_thumbnails());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert!(app.thumbnails[1].has_pixels());
    assert!(!app.auto_aspect.samples.contains_key(&0));
    assert!(app.auto_aspect.samples.contains_key(&1));
    let rows = memory.read().unwrap();
    assert!(!rows.contains_key("zipthumb:book.zip|autocrop:v4:rtl"));
    assert!(rows.contains_key("zipthumb:sibling.zip|autocrop:v4:rtl"));
    assert!(rows.contains_key("zipthumb:book.zip#pin:manual"));
    drop(rows);
    assert!(!app.reconcile_restored_cover_thumbnails());
}

fn seed_loaded_and_pending(app: &mut App, ctx: &egui::Context) {
    let target = slot("c:/book.zip");
    let sibling = slot("c:/sibling.zip");
    app.items = [
        "c:/book.zip",
        "c:/sibling.zip",
        "c:/book.zip",
        "c:/sibling.zip",
    ]
    .into_iter()
    .map(|path| GridItem::ZipFile(PathBuf::from(path)))
    .collect();
    app.thumbnails = vec![
        loaded(ctx, &target, app.cover_rotation_cache.stamp(target.clone())),
        loaded(
            ctx,
            &sibling,
            app.cover_rotation_cache.stamp(sibling.clone()),
        ),
        ThumbnailState::Pending,
        ThumbnailState::Pending,
    ];
    for idx in 0..4 {
        let request = begin_automatic(app, idx);
        if idx >= 2 {
            app.requested.receive(pixels(&request));
        }
        app.thumb_pixels.insert(
            idx,
            Arc::new(egui::ColorImage::filled([2, 1], egui::Color32::WHITE)),
        );
        app.auto_aspect.samples.insert(idx, 0.5);
    }
}

fn assert_target_evicted_and_sibling_retained(app: &App) {
    for idx in [0, 2] {
        assert!(matches!(app.thumbnails[idx], ThumbnailState::Evicted));
        assert!(!app.requested.contains_key(&idx));
        assert!(!app.thumb_pixels.contains_key(&idx));
        assert!(!app.auto_aspect.samples.contains_key(&idx));
    }
    assert!(app.thumbnails[1].has_pixels());
    assert!(matches!(app.thumbnails[3], ThumbnailState::Pending));
    for idx in [1, 3] {
        assert!(app.requested.contains_key(&idx));
        assert!(app.thumb_pixels.contains_key(&idx));
        assert!(app.auto_aspect.samples.contains_key(&idx));
    }
    let sibling_request = app.requested.id(3).unwrap();
    assert!(
        app.requested
            .diagnostic_projection(3, sibling_request, app.items_generation)
            .unwrap()
            .image_backlog_contains_request
    );
}

#[test]
fn cover_cache_completion_evicts_mounted_loaded_and_cpu_pending_without_resetting_sibling() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    seed_loaded_and_pending(&mut app, &ctx);
    let old_request = app
        .requested
        .automatic_cover_requests()
        .into_iter()
        .find(|request| request.idx == 0)
        .unwrap();
    let generation = app.items_generation;
    let cancel = app.cancel_token.clone();
    let cancellation = cancel.load(Ordering::Relaxed);
    let target = slot("c:/book.zip");
    app.cover_rotation_cache
        .complete_for_test(InvalidationCompletion {
            affected_slots: vec![target.clone()],
            pending_slots: vec![target],
            ..Default::default()
        });
    assert!(
        app.thumbnails[0].has_pixels(),
        "keep the shown pixels until completion is consumed"
    );
    assert_eq!(app.requested.id(0), Some(old_request.request_id));
    app.poll_cover_rotation_cache(&ctx);
    assert_target_evicted_and_sibling_retained(&app);
    assert_eq!(app.items_generation, generation);
    assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
    assert_eq!(cancel.load(Ordering::Relaxed), cancellation);
    assert!(
        app.requested
            .next_image()
            .is_some_and(|message| message.idx == 3)
    );
    assert!(app.requested.next_image().is_none());
    let fresh = begin_automatic(&mut app, 0);
    assert_ne!(fresh.request_id, old_request.request_id);
    assert!(matches!(
        app.requested.receive(pixels(&old_request)),
        ThumbnailReceipt::Ignored
    ));
    let mut finalized = pixels(&old_request);
    finalized.payload = crate::thumb_loader::ThumbMsgPayload::Finalized;
    assert!(matches!(
        app.requested.receive(finalized),
        ThumbnailReceipt::Ignored
    ));
    assert_eq!(app.requested.id(0), Some(fresh.request_id));
}

#[cfg(windows)]
#[test]
fn cover_cache_completion_reaches_at_rest_and_retiring_cpu_pending_without_mounting_them() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    seed_loaded_and_pending(&mut app, &ctx);
    let parked =
        app.build_window_context_for_test(177329, |owner| seed_loaded_and_pending(owner, &ctx));
    let retiring =
        app.build_window_context_for_test(177330, |owner| seed_loaded_and_pending(owner, &ctx));
    app.retire_cover_context_for_test(retiring);
    let mounted = app.projected_viewer_context_id();
    let target = slot("c:/book.zip");
    app.cover_rotation_cache
        .complete_for_test(InvalidationCompletion {
            affected_slots: vec![target],
            ..Default::default()
        });
    app.poll_cover_rotation_cache(&ctx);
    assert_eq!(app.projected_viewer_context_id(), mounted);
    assert_target_evicted_and_sibling_retained(&app);
    let expected = vec![
        (true, false, false, false),
        (false, true, true, true),
        (true, false, false, false),
        (false, true, true, true),
    ];
    for id in [parked, retiring] {
        assert_eq!(app.cover_cache_context_rows_for_test(id), expected);
    }
    app.with_owner_viewer_context(parked, |owner| {
        assert_target_evicted_and_sibling_retained(owner)
    })
    .unwrap();
    assert_eq!(app.projected_viewer_context_id(), mounted);
}

#[test]
fn cover_cache_real_row_completion_cancels_requests_admitted_during_deletion() {
    use crate::cover_rotation_cache::{CatalogScope, InvalidationRequest, SourceHint};
    for queued_pixels in [false, true] {
        for explicit in [false, true] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let parent = app.tmp.path().join("during-deletion");
            let book = parent.join("book.zip");
            let cache = app.tmp.path().join("catalog");
            let cover = crate::adjustment_db::zip_entry_key(&book, "cover.png");
            let db = crate::catalog::CatalogDb::open(&cache, &parent).unwrap();
            let key = "zipthumb:book.zip|autocrop:v4:rtl";
            db.save(
                key,
                1,
                2,
                2,
                1,
                None,
                b"header only",
                &CoverIdentity::Page(cover),
                automatic_material(),
            )
            .unwrap();
            // Capture while the reader has no book-tile request. The worker can
            // discover the affected slot only from its existing catalog row.
            let captured = InvalidationRequest {
                changed_keys: vec![crate::adjustment_db::zip_entry_key(&book, "body.png")],
                source_hints: vec![SourceHint::new(
                    BookCoverId::from_grid_item(&GridItem::ZipFile(book.clone())).unwrap(),
                )],
                scopes: vec![CatalogScope {
                    folder: parent.clone(),
                    memory: vec![],
                }],
                work: crate::catalog::CatalogWork::capture(&cache),
                archive_cache: None,
            };
            app.thumbnail_pool_catalog = ThumbnailPoolCatalog::Folder(parent);
            // Opening the parent during deletion admits an old-cache request after
            // capture. Exercise both CPU upload backlog and still-in-flight pixels.
            app.items = vec![GridItem::ZipFile(book.clone())];
            app.thumbnails = vec![ThumbnailState::Pending];
            let old = if explicit {
                begin_automatic(&mut app, 0)
            } else {
                let mut request = LoadRequest {
                    idx: 0,
                    path: book,
                    cache_key_override: Some("zipthumb:book.zip".into()),
                    auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
                        crate::settings::ReadingDirection::Rtl,
                    )),
                    items_gen: app.items_generation,
                    ..Default::default()
                };
                request.request_id = app
                    .requested
                    .begin_for_request(&request, ThumbnailRequestKind::Normal, None)
                    .unwrap();
                request
            };
            if queued_pixels {
                app.requested.receive(pixels(&old));
            }
            let completion = crate::cover_rotation_cache::run_invalidation(captured);
            assert!(completion.errors.is_empty(), "{:?}", completion.errors);
            assert_eq!(completion.affected_slots.len(), 1);
            assert!(completion.pending_slots.is_empty());
            assert!(db.load_one(key).unwrap().is_none());
            app.cover_rotation_cache.complete_for_test(completion);
            app.poll_cover_rotation_cache(&ctx);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(!app.requested.contains_key(&0));
            assert!(app.requested.next_image().is_none());
            let fresh = begin_automatic(&mut app, 0);
            assert_ne!(fresh.request_id, old.request_id);
            assert!(matches!(
                app.requested.receive(pixels(&old)),
                ThumbnailReceipt::Ignored
            ));
            let mut finalized = pixels(&old);
            finalized.payload = crate::thumb_loader::ThumbMsgPayload::Finalized;
            assert!(matches!(
                app.requested.receive(finalized),
                ThumbnailReceipt::Ignored
            ));
            assert_eq!(app.requested.id(0), Some(fresh.request_id));
        }
    }
}

#[test]
fn cover_cache_zipdir_external_pin_invalidates_writer_rows_and_retained_loaded() {
    use super::cover_rotation_cache::CoverCacheInputs;
    use crate::cover_rotation_cache::InvalidationRequest;
    use crate::folder_thumb_pins::FolderPinSource;
    use std::io::Write;

    for persisted in [false, true] {
        for admitted in [false, true] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let path = app.tmp.path().join("books.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            for entry in ["A/0.png", "A/1.png", "B/0.png", "B/1.png", "C/0.png"] {
                zip.start_file(entry, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"selection needs entry metadata only")
                    .unwrap();
            }
            zip.finish().unwrap();
            let pins = app.folder_thumb_pin_db.as_ref().unwrap().clone();
            let owner = book_container_key(&path, "A/");
            pins.set(
                &owner,
                &FolderPinSource::ZipDir {
                    zip_rel: String::new(),
                    dir_prefix: "B/".into(),
                },
            )
            .unwrap();
            app.settings.thumb_cover_auto_crop = true;
            app.items = ["A/", "C/"]
                .map(|prefix| GridItem::ZipDir {
                    zip_path: path.clone(),
                    dir_prefix: prefix.into(),
                    representative: Some(format!("{prefix}0.png")),
                    is_archive: false,
                })
                .to_vec();
            let target = AutoCoverSlot::new(
                BookCoverId::from_grid_item(&app.items[0]).unwrap(),
                crate::adjustment_db::zip_entry_key(&path, "B/0.png"),
            );
            let sibling = AutoCoverSlot::new(
                BookCoverId::from_grid_item(&app.items[1]).unwrap(),
                crate::adjustment_db::zip_entry_key(&path, "C/0.png"),
            );
            let retained = loaded(
                &ctx,
                &target,
                app.cover_rotation_cache.stamp(target.clone()),
            );
            app.thumbnails = vec![
                retained.clone(),
                loaded(
                    &ctx,
                    &sibling,
                    app.cover_rotation_cache.stamp(sibling.clone()),
                ),
            ];
            app.thumbnail_pool_catalog = ThumbnailPoolCatalog::Folder(path.clone());
            let mut req = make_load_request_with_auto_crop(
                &app.items[0],
                0,
                17,
                23,
                false,
                None,
                Some(app.settings.folder_thumb_sort),
                app.settings.folder_thumb_depth,
                &pins.lookup_many([owner]),
                &HashMap::new(),
                None,
                None,
                Some(&pins),
                None,
                false,
                &app.settings.file_type_visibility,
                configured_thumbnail_auto_crop(&app.settings),
            )
            .unwrap();
            assert_eq!(req.zip_dir_prefix.as_deref(), Some("B/"));
            let key = crate::thumb_loader::cache_key_for_request(&req)
                .unwrap()
                .into_owned();
            assert!(key.starts_with("zipdir:A/|autocrop:v4:"), "{key}");
            if admitted {
                req.items_gen = app.items_generation;
                app.requested
                    .begin_for_request(&req, ThumbnailRequestKind::Normal, None)
                    .unwrap();
            }
            let cache = app.tmp.path().join("catalog");
            let db = crate::catalog::CatalogDb::open(&cache, &path).unwrap();
            let sibling_key = "zipdir:C/|autocrop:v4:rtl";
            for (row, slot) in [(&key[..], &target), (sibling_key, &sibling)] {
                if persisted {
                    db.save(
                        row,
                        1,
                        2,
                        2,
                        1,
                        None,
                        b"header only",
                        &CoverIdentity::Page(slot.representative_page_key.clone()),
                        automatic_material(),
                    )
                    .unwrap();
                }
            }
            let inputs = || {
                let mut inputs = CoverCacheInputs::default();
                inputs.grid(
                    &app.items,
                    &app.thumbnails,
                    Some(&app.requested),
                    None,
                    &app.converted_archive_cache_paths,
                    &app.thumbnail_pool_catalog,
                    None,
                    &app.settings,
                    Some(&pins),
                );
                inputs
            };
            let invalidate = |changed: &str| {
                let inputs = inputs();
                crate::cover_rotation_cache::run_invalidation(InvalidationRequest {
                    changed_keys: vec![crate::adjustment_db::zip_entry_key(&path, changed)],
                    source_hints: inputs.hints,
                    scopes: inputs.scopes,
                    work: crate::catalog::CatalogWork::capture(&cache),
                    archive_cache: None,
                })
            };
            // Cover-only changes still preserve the row representing that page.
            // Other prefixes now intentionally share the archive's invalidation.
            for changed in ["B/0.png"] {
                let completion = invalidate(changed);
                assert!(completion.errors.is_empty(), "{:?}", completion.errors);
                assert!(!completion.affected_slots.contains(&target));
                if persisted {
                    assert!(db.load_one(&key).unwrap().is_some());
                }
            }
            let completion = invalidate("B/1.png");
            assert!(completion.errors.is_empty(), "{:?}", completion.errors);
            assert!(completion.affected_slots.contains(&target));
            assert!(completion.affected_slots.contains(&sibling));
            assert!(db.load_one(&key).unwrap().is_none());
            if persisted {
                assert!(db.load_one(sibling_key).unwrap().is_none());
            }
            app.cover_rotation_cache.complete_for_test(completion);
            app.poll_cover_rotation_cache(&ctx);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));
            assert!(!app.cover_rotation_cache.is_current(stamp(&retained)));
            app.thumbnails[0] = retained;
            app.auto_aspect.samples.insert(0, 0.5);
            assert!(app.reconcile_restored_cover_thumbnails());
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(!app.auto_aspect.samples.contains_key(&0));
            assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));
        }
    }
}

#[test]
fn cover_cache_historical_pin_completion_preserves_unresolved_cover_only_request() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.items = vec![GridItem::ZipFile(PathBuf::from("c:/book.zip"))];
    app.thumbnails = vec![ThumbnailState::Pending];
    let req = LoadRequest {
        idx: 0,
        path: PathBuf::from("c:/book.zip"),
        cache_key_override: Some("zipthumb:book.zip".into()),
        auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
            crate::settings::ReadingDirection::Rtl,
        )),
        items_gen: app.items_generation,
        ..Default::default()
    };
    let id = app
        .requested
        .begin_for_request(&req, ThumbnailRequestKind::Normal, None)
        .unwrap();
    // The changed cover is somebody else's historical pinned row's body. Its
    // deletion must not cancel the current unresolved first-page selection.
    let mut historical = slot("c:/book.zip");
    historical.representative_page_key = "c:/book.zip::old-pin.png".into();
    app.cover_rotation_cache
        .complete_for_test(InvalidationCompletion {
            affected_slots: vec![historical.clone()],
            deleted_rows: vec![crate::cover_rotation_cache::InvalidatedRow {
                slot: historical,
                catalog: PathBuf::from("c:/"),
                key: "zipthumb:book.zip|autocrop:v4:rtl#pin:old-pin.png".into(),
            }],
            ..Default::default()
        });
    app.poll_cover_rotation_cache(&ctx);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    assert_eq!(app.requested.id(0), Some(id));
}

#[test]
fn cover_cache_correlated_deleted_rows_preserve_same_name_cover_only_request_in_other_catalog() {
    use crate::cover_rotation_cache::{CatalogScope, InvalidationRequest, SourceHint};
    use crate::thumb_loader::{ParentRow, RequestCatalog};
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let cache = app.tmp.path().join("catalog");
    let parents = [app.tmp.path().join("A"), app.tmp.path().join("B")];
    let books = parents.each_ref().map(|parent| parent.join("book.zip"));
    let key = "zipthumb:book.zip|autocrop:v4:rtl";
    let old_pin = "zipthumb:book.zip|autocrop:v4:rtl#pin:old.png";
    let covers = books
        .each_ref()
        .map(|book| crate::adjustment_db::zip_entry_key(book, "cover.png"));
    let dbs = parents
        .each_ref()
        .map(|parent| crate::catalog::CatalogDb::open(&cache, parent).unwrap());
    for (db, cover) in dbs.iter().zip(&covers) {
        db.save(
            key,
            1,
            2,
            2,
            1,
            None,
            b"header",
            &CoverIdentity::Page(cover.clone()),
            automatic_material(),
        )
        .unwrap();
    }
    dbs[0]
        .save(
            old_pin,
            1,
            2,
            2,
            1,
            None,
            b"header",
            &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&books[0], "old.png")),
            automatic_material(),
        )
        .unwrap();
    // Capture before either unresolved request is admitted. A resets only its
    // current cover; B resets its body. A's historical pin is also invalidated.
    let captured = InvalidationRequest {
        changed_keys: vec![
            covers[0].clone(),
            crate::adjustment_db::zip_entry_key(&books[1], "body.png"),
        ],
        source_hints: books
            .iter()
            .map(|book| {
                SourceHint::new(
                    BookCoverId::from_grid_item(&GridItem::ZipFile(book.clone())).unwrap(),
                )
            })
            .collect(),
        scopes: parents
            .iter()
            .map(|parent| CatalogScope {
                folder: parent.clone(),
                memory: vec![],
            })
            .collect(),
        work: crate::catalog::CatalogWork::capture(&cache),
        archive_cache: None,
    };
    app.items = books.iter().cloned().map(GridItem::ZipFile).collect();
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    let mut ids = Vec::new();
    for idx in 0..2 {
        let mut req = LoadRequest {
            idx,
            items_gen: app.items_generation,
            path: books[idx].clone(),
            cache_key_override: Some("zipthumb:book.zip".into()),
            auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
                crate::settings::ReadingDirection::Rtl,
            )),
            catalog_target: RequestCatalog::Parent {
                row: ParentRow {
                    source: books[idx].clone(),
                    folder: parents[idx].clone(),
                    row_key: key.into(),
                },
                work: crate::catalog::CatalogWork::capture(&cache),
            },
            ..Default::default()
        };
        req.request_id = app
            .requested
            .begin_for_request(&req, ThumbnailRequestKind::Normal, None)
            .unwrap();
        ids.push(req.request_id);
        app.requested.receive(pixels(&req));
    }
    let completion = crate::cover_rotation_cache::run_invalidation(captured);
    assert!(completion.errors.is_empty(), "{:?}", completion.errors);
    assert!(completion.pending_slots.is_empty());
    assert!(dbs[0].load_one(key).unwrap().is_some());
    assert!(dbs[0].load_one(old_pin).unwrap().is_none());
    assert!(dbs[1].load_one(key).unwrap().is_none());
    assert!(
        completion
            .deleted_rows
            .iter()
            .any(|row| row.key == old_pin
                && crate::path_key::eq_keep_drive(&row.catalog, &parents[0]))
    );
    assert!(
        completion
            .deleted_rows
            .iter()
            .any(|row| row.key == key && crate::path_key::eq_keep_drive(&row.catalog, &parents[1]))
    );
    app.cover_rotation_cache.complete_for_test(completion);
    app.poll_cover_rotation_cache(&ctx);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    assert_eq!(app.requested.id(0), Some(ids[0]));
    assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));
    assert!(!app.requested.contains_key(&1));
    assert_eq!(app.requested.next_image().unwrap().idx, 0);
    assert!(app.requested.next_image().is_none());
}

#[test]
fn cover_cache_completion_keeps_foreign_basename_map_even_with_same_pinned_page() {
    let target = slot("c:/a/book.zip");
    let key = "zipthumb:book.zip|autocrop:v4:rtl".to_owned();
    let memory = Arc::new(std::sync::RwLock::new(HashMap::from([(
        key.clone(),
        crate::catalog::CacheEntry {
            mtime: 1,
            file_size: 2,
            jpeg_data: vec![],
            source_dims: None,
            layout_dims: None,
            cover: CoverIdentity::Page(target.representative_page_key.clone()),
            auto_material: automatic_material(),
            folder_provenance: None,
            selection_proof: None,
        },
    )])));
    let completion = InvalidationCompletion {
        affected_slots: vec![target.clone()],
        deleted_rows: vec![crate::cover_rotation_cache::InvalidatedRow {
            slot: target,
            catalog: PathBuf::from("c:/a"),
            key: key.clone(),
        }],
        ..Default::default()
    };
    super::cover_rotation_cache::clear_deleted_cover_memory(
        Some(&memory),
        &ThumbnailPoolCatalog::Folder(PathBuf::from("c:/b")),
        &[completion],
    );
    assert!(memory.read().unwrap().contains_key(&key));
}

#[test]
fn cover_cache_representative_only_completion_preserves_pixels_and_return_to_zero_advances_serial()
{
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    seed_loaded_and_pending(&mut app, &ctx);
    let target = slot("c:/book.zip");
    let initial = stamp(&app.thumbnails[0]).clone();
    // The worker deliberately returns no affected slot when only the representative rotates.
    app.cover_rotation_cache
        .complete_for_test(InvalidationCompletion::default());
    app.poll_cover_rotation_cache(&ctx);
    assert!(app.thumbnails[0].has_pixels());
    assert_eq!(stamp(&app.thumbnails[0]), &initial);
    assert!(app.requested.contains_key(&2));
    for serial in 1..=2 {
        // Both a nonzero body rotation and returning it to 0 are invalidations.
        app.cover_rotation_cache
            .complete_for_test(InvalidationCompletion {
                affected_slots: vec![target.clone()],
                ..Default::default()
            });
        app.poll_cover_rotation_cache(&ctx);
        let fresh = app.cover_rotation_cache.stamp(target.clone());
        assert_eq!(
            fresh,
            CoverCacheStamp::Tracked {
                slot: Arc::new(target.clone()),
                serial
            }
        );
        assert!(!app.cover_rotation_cache.is_current(&initial));
        app.thumbnails[0] = loaded(&ctx, &target, fresh);
    }
}

#[cfg(windows)]
#[test]
fn cover_cache_unsaved_retained_archive_uses_mounted_reader_alias_before_source_filtering() {
    use std::io::Write;
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("book.rar");
    let cached = app.tmp.path().join("converted.zip");
    std::fs::write(&source, b"logical source metadata only").unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&cached).unwrap());
    for name in ["cover.png", "body.png"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"selection must not decode these entries")
            .unwrap();
    }
    zip.finish().unwrap();
    let book_item = GridItem::ConvertibleArchive {
        path: source.clone(),
        format: crate::archive_converter::ArchiveFormat::Rar,
    };
    let target = AutoCoverSlot::new(
        BookCoverId::from_grid_item(&book_item).unwrap(),
        crate::adjustment_db::zip_entry_key(&cached, "cover.png"),
    );
    app.items = vec![GridItem::ZipImage {
        zip_path: cached,
        entry_name: "body.png".into(),
    }];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.archive_source_override = Some(source);
    // No disk/memory row and no reverse cache DB lookup can rescue a discarded
    // retained tile. Only the mounted reader supplies the physical alias.
    app.archive_cache_db = None;
    app.folder_thumb_pin_db = None;
    app.settings.thumb_cover_auto_crop = true;
    let generation = app.items_generation;
    let page_request = app
        .requested
        .begin(0, generation, ThumbnailRequestKind::Normal, None)
        .unwrap();
    let before = app.cover_rotation_cache.stamp(target.clone());
    let parked = app.build_window_context_for_test(177332, |owner| {
        owner.items = vec![book_item];
        owner.thumbnails = vec![loaded(&ctx, &target, before.clone())];
        owner.archive_source_override = None;
        owner.converted_archive_cache_paths.clear();
        owner.thumbnail_pool_catalog = ThumbnailPoolCatalog::None;
        owner.current_color_cache_map = None;
    });
    let GridItem::ZipImage { zip_path, .. } = &app.items[0] else {
        unreachable!()
    };
    let changed = crate::adjustment_db::zip_entry_key(zip_path, "body.png");
    app.invalidate_automatic_covers_after_rotation(&[changed]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.cover_rotation_cache.is_current(&before) && std::time::Instant::now() < deadline {
        app.poll_cover_rotation_cache(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!app.cover_rotation_cache.is_current(&before));
    assert_eq!(
        app.cover_cache_context_rows_for_test(parked),
        vec![(true, false, false, false)]
    );
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    assert_eq!(app.requested.id(0), Some(page_request));
}

#[cfg(windows)]
#[test]
fn cover_cache_direct_reader_split_restores_stale_saved_basis_without_pixels_or_aspect_sample() {
    for materialized in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let root = app.tmp.path().join("automatic-cover-direct-restore");
        let book = root.join("book.zip");
        let basis = GridItem::ZipFile(book.clone());
        let direct = GridItem::Image(root.join("direct.png"));
        let target = AutoCoverSlot::new(
            BookCoverId::from_grid_item(&basis).unwrap(),
            crate::adjustment_db::zip_entry_key(&book, "cover.png"),
        );
        let retained = loaded(
            &ctx,
            &target,
            app.cover_rotation_cache.stamp(target.clone()),
        );
        let retained_stamp = stamp(&retained).clone();
        app.current_folder = Some(root.clone());
        app.items = vec![basis.clone(), direct.clone()];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.image_metas = vec![None; 2];
        app.visible_indices = vec![0, 1];
        app.fullscreen_idx = Some(1);
        app.auto_aspect.samples.insert(0, 0.5);
        app.snapshot = Some(crate::snapshot::SnapshotState {
            page_edit_source: crate::snapshot::SnapshotPageEditSource::PhysicalPrefix,
            items: Vec::new(),
            membership: Default::default(),
            origin: root.clone(),
            filter_at_capture: crate::snapshot::FilterState::from_rating([true; 6]),
            source_label: crate::snapshot::SnapshotSourceLabel::DirectMedia {
                target: root.join("direct.png"),
                policy: app.settings.file_type_visibility.clone(),
                reader_order: vec![0, 1],
                details_sort: None,
                basis_generation: app.items_generation,
            },
            generation_id: app.items_generation,
            saved_items: vec![basis.clone()],
            saved_folder_order: app.folder_order.clone(),
            saved_projection_recipe: app.projection_recipe.clone(),
            saved_viewing_state: app.capture_snapshot_viewing_state(),
            saved_page_identity_index: app.page_identity_index.clone(),
            saved_thumbnails: vec![retained],
            saved_converted_archive_sources: Default::default(),
            saved_image_metas: vec![None],
            saved_visible_indices: vec![0],
            saved_scroll_offset_y: 0.0,
            saved_selected: Some(0),
            saved_zip_nav: None,
            list_view_items: vec![basis.clone(), direct],
            list_view_projection_recipe: app.projection_recipe.clone(),
            list_view_thumbnails: vec![ThumbnailState::Pending; 2],
            list_view_converted_archive_sources: Default::default(),
            list_view_image_metas: vec![None; 2],
            list_view_page_identity_index: app.page_identity_index.clone(),
            pre_snapshot_search_origin: None,
        });
        assert!(app.direct_media_reader_active());
        app.cover_rotation_cache
            .complete_for_test(InvalidationCompletion {
                affected_slots: vec![target],
                ..Default::default()
            });
        app.poll_cover_rotation_cache(&ctx);
        assert!(!app.cover_rotation_cache.is_current(&retained_stamp));
        assert!(
            app.snapshot.as_ref().unwrap().saved_thumbnails[0].has_pixels(),
            "offscreen retained material stays passive until the actual restore boundary"
        );
        let _detached = if materialized {
            app.split_materialized_physical_context_for_detached_scope(&root, 1, 177331)
        } else {
            app.split_current_context_preserving_main_grid()
        };
        assert_eq!(app.items, vec![basis], "materialized={materialized}");
        assert!(!app.direct_media_reader_active());
        assert!(
            matches!(app.thumbnails[0], ThumbnailState::Evicted),
            "direct-reader restoration must reconcile the saved stamp, materialized={materialized}"
        );
        assert!(
            !app.auto_aspect.samples.contains_key(&0),
            "the restored aspect owner must not resurrect a stale sample, materialized={materialized}"
        );
    }
}
