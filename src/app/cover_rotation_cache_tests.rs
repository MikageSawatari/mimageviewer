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
    AutoCoverSlot {
        logical_book: BookCoverId::from_grid_item(&GridItem::ZipFile(path.clone())).unwrap(),
        representative_page_key: crate::adjustment_db::zip_entry_key(&path, "cover.png"),
    }
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
            affected_slots: vec![target.clone()],
            pending_slots: vec![target],
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
        affected_slots: vec![target],
        deleted_keys: vec![(PathBuf::from("c:/a"), vec![key.clone()])],
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
    let target = AutoCoverSlot {
        logical_book: BookCoverId::from_grid_item(&book_item).unwrap(),
        representative_page_key: crate::adjustment_db::zip_entry_key(&cached, "cover.png"),
    };
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
        let target = AutoCoverSlot {
            logical_book: BookCoverId::from_grid_item(&basis).unwrap(),
            representative_page_key: crate::adjustment_db::zip_entry_key(&book, "cover.png"),
        };
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
            saved_projection_recipe: app.projection_recipe.clone(),
            saved_viewing_state: app.capture_snapshot_viewing_state(),
            saved_page_identity_index: app.page_identity_index.clone(),
            saved_thumbnails: vec![retained],
            saved_image_metas: vec![None],
            saved_visible_indices: vec![0],
            saved_scroll_offset_y: 0.0,
            saved_selected: Some(0),
            saved_zip_nav: None,
            list_view_items: vec![basis.clone(), direct],
            list_view_projection_recipe: app.projection_recipe.clone(),
            list_view_thumbnails: vec![ThumbnailState::Pending; 2],
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
