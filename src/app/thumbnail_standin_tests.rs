use super::*;
use crate::folder_order::physical_item_key;

fn texture(ctx: &egui::Context, name: &str) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::filled([120, 80], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    )
}

fn loaded(tex: egui::TextureHandle) -> ThumbnailState {
    ThumbnailState::loaded_raster(
        tex,
        crate::thumb_loader::ThumbLoadOrigin::FinalCache,
        false,
        120,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((120, 80)),
        None,
    )
}

fn fixture() -> (AppTestEnvForTest, PathBuf) {
    let mut app = setup_app_for_test();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.active_quick_folder_slot = None;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    let folder = app.tmp.path().join("thumbnail-standins");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        std::fs::write(folder.join(name), b"fixture").unwrap();
    }
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.items.len(), 3);
    assert!(app.image_metas.iter().all(Option::is_some));
    (app, folder)
}

fn finish_refresh(app: &mut App) {
    finish_refresh_adoption(app);
    app.finish_page_identity_preparation_for_test();
}

fn finish_refresh_adoption(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_sidecar_restore(&ctx);
        if let Some(ready) = app.poll_folder_pane_open(&ctx) {
            let FolderOpenScanPurpose::CurrentViewOrderRefresh {
                order,
                reload_owner,
            } = ready.purpose
            else {
                panic!("unexpected folder open purpose");
            };
            assert!(app.apply_current_view_order_refresh(
                ready.path,
                ready.scan.unwrap(),
                order,
                *reload_owner,
            ));
            return;
        }
        assert!(Instant::now() < deadline, "order refresh did not finish");
        std::thread::yield_now();
    }
}

fn seed_loaded(app: &mut App) -> HashMap<String, egui::TextureId> {
    let ctx = egui::Context::default();
    seed_loaded_with_context(app, &ctx)
}

fn seed_loaded_with_context(
    app: &mut App,
    ctx: &egui::Context,
) -> HashMap<String, egui::TextureId> {
    app.keep_set = (0..app.items.len()).collect();
    app.keep_range = (0, app.items.len());
    app.thumbnails = app
        .items
        .iter()
        .enumerate()
        .map(|(index, _)| loaded(texture(ctx, &format!("old-{index}"))))
        .collect();
    app.items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                physical_item_key(item).unwrap(),
                app.grid_thumbnail_display_image(index).unwrap().tex.id(),
            )
        })
        .collect()
}

fn assert_pending_images(app: &App, expected: &HashMap<String, egui::TextureId>) {
    assert_eq!(app.thumbnail_standins.images.len(), expected.len());
    assert_eq!(app.requested.len(), 0, "adoption still resets requests");
    for (index, item) in app.items.iter().enumerate() {
        assert!(matches!(app.thumbnails[index], ThumbnailState::Pending));
        let key = physical_item_key(item).unwrap();
        assert_eq!(
            app.grid_thumbnail_display_image(index).unwrap().tex.id(),
            expected[&key],
            "the new index must show this item's old material",
        );
    }
}

fn synthetic_loaded(app: &mut App, count: usize) {
    let ctx = egui::Context::default();
    synthetic_loaded_with_context(app, count, &ctx);
}

fn synthetic_loaded_with_context(app: &mut App, count: usize, ctx: &egui::Context) {
    app.items = (0..count)
        .map(|index| GridItem::Image(PathBuf::from(format!("c:/standins/{index}.jpg"))))
        .collect();
    app.image_metas = vec![Some((10, 20)); count];
    app.thumbnails = (0..count)
        .map(|index| loaded(texture(ctx, &format!("synthetic-{index}"))))
        .collect();
    app.keep_set = (0..count).collect();
    app.keep_range = (0, count);
}

fn install_pending_standins(app: &mut App) {
    let captured = app.capture_thumbnail_standins();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.thumbnail_standins.set_generation(app.items_generation);
    for (index, item) in app.items.iter().enumerate() {
        let key = physical_item_key(item).unwrap();
        app.thumbnail_standins
            .attach(index, &key, app.image_metas[index], &captured);
    }
}

#[test]
fn thumbnail_standins_standard_sort_adoption_keeps_identity_pending_and_requests() {
    let (mut app, _) = fixture();
    let before = seed_loaded(&mut app);
    let first = physical_item_key(&app.items[0]).unwrap();
    app.requested.insert(0, false);
    assert!(app.select_folder_sort(FolderSortChoice::Standard(
        crate::settings::SortOrder::FileNameDesc,
    )));
    app.apply_sort_change_reload();
    finish_refresh(&mut app);
    assert_ne!(physical_item_key(&app.items[0]).unwrap(), first);
    assert_pending_images(&app, &before);
}

#[test]
fn thumbnail_standins_manual_reorder_adoption_keeps_identity_pending_and_requests() {
    let (mut app, _) = fixture();
    assert!(app.select_folder_sort(FolderSortChoice::Manual));
    finish_refresh(&mut app);
    let before = seed_loaded(&mut app);
    let mut order = app
        .items
        .iter()
        .filter_map(physical_item_key)
        .collect::<Vec<_>>();
    order.reverse();
    app.requested.insert(0, false);
    app.commit_folder_manual_reorder(&order);
    finish_refresh(&mut app);
    assert_eq!(
        app.items
            .iter()
            .filter_map(physical_item_key)
            .collect::<Vec<_>>(),
        order,
    );
    assert_pending_images(&app, &before);
}

#[test]
fn thumbnail_standins_second_reorder_inherits_pending_original_identity_and_stamp() {
    let (mut app, _) = fixture();
    let expected = seed_loaded(&mut app);
    assert!(app.select_folder_sort(FolderSortChoice::Manual));
    finish_refresh(&mut app);
    assert_pending_images(&app, &expected);
    let original = app.thumbnail_standins.images.clone();
    // A completed keep projection establishes which of the first refresh's
    // still-Pending materials are actually retained for the second refresh.
    let count = app.items.len();
    app.install_thumbnail_keep_projection(
        ThumbnailKeepProjection {
            pages: (0..count).collect(),
            bounded_range: (0, count),
            ..Default::default()
        },
        false,
    );
    let mut order = app
        .items
        .iter()
        .filter_map(physical_item_key)
        .collect::<Vec<_>>();
    order.reverse();
    app.commit_folder_manual_reorder(&order);
    finish_refresh(&mut app);
    assert_pending_images(&app, &expected);
    for standin in app.thumbnail_standins.images.values() {
        let old = original
            .values()
            .find(|old| old.key == standin.key)
            .unwrap();
        assert_eq!(standin.stamp, old.stamp);
        assert_eq!(standin.image.tex.id(), old.image.tex.id());
    }
}

#[test]
fn thumbnail_standins_changed_file_stamp_is_not_attached_on_adoption() {
    let (mut app, folder) = fixture();
    let before = seed_loaded(&mut app);
    let changed = crate::folder_order::folder_key(&folder.join("a.jpg"));
    std::fs::write(folder.join("a.jpg"), b"different-size fixture").unwrap();
    assert!(app.select_folder_sort(FolderSortChoice::Standard(
        crate::settings::SortOrder::FileNameDesc,
    )));
    app.apply_sort_change_reload();
    finish_refresh(&mut app);
    assert_eq!(app.thumbnail_standins.images.len(), 2);
    for (index, item) in app.items.iter().enumerate() {
        assert!(matches!(app.thumbnails[index], ThumbnailState::Pending));
        let key = physical_item_key(item).unwrap();
        if key == changed {
            assert!(app.grid_thumbnail_display_image(index).is_none());
        } else {
            assert_eq!(
                app.grid_thumbnail_display_image(index).unwrap().tex.id(),
                before[&key],
            );
        }
    }
}

#[test]
fn thumbnail_standins_rejected_adoption_preserves_loaded_and_existing_standins() {
    let (mut app, folder) = fixture();
    let ctx = egui::Context::default();
    let before = seed_loaded_with_context(&mut app, &ctx);
    // Preserve both forms of display material across a rejected stale result.
    install_pending_standins(&mut app);
    let replacement = texture(&ctx, "loaded-after-capture");
    let replacement_id = replacement.id();
    assert!(!before.values().any(|id| *id == replacement_id));
    app.thumbnails[0] = loaded(replacement);
    app.requested.insert(1, false);
    let generation = app.items_generation;
    let mut order = CurrentViewOrderSnapshot::from_settings(&app.settings);
    order.order_revision = app.folder_order.order_revision;
    order.sort_order = crate::settings::SortOrder::FileNameDesc;
    let scan = app.folder_scan_policy().scan(&folder, None).unwrap();
    let owner = app.current_folder_reload_owner(&folder);
    assert!(!app.apply_current_view_order_refresh(folder, scan, order, owner));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.thumbnail_standins.images.len(), before.len());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        replacement_id,
    );
    assert!(app.requested.contains_key(&1));
    for index in 1..app.items.len() {
        let key = physical_item_key(&app.items[index]).unwrap();
        assert_eq!(
            app.grid_thumbnail_display_image(index).unwrap().tex.id(),
            before[&key],
        );
    }
}

#[test]
fn thumbnail_standins_adoption_uses_adjusted_texture_without_generating_adjustment() {
    let (mut app, _) = fixture();
    let ctx = egui::Context::default();
    let mut expected = seed_loaded_with_context(&mut app, &ctx);
    let adjusted = texture(&ctx, "adjusted");
    assert!(!expected.values().any(|id| *id == adjusted.id()));
    expected.insert(physical_item_key(&app.items[1]).unwrap(), adjusted.id());
    app.thumb_adjust_tex.insert(1, adjusted);
    app.adjustment_dragging = false;
    let pixel_count = app.thumb_pixels.len();
    assert!(app.select_folder_sort(FolderSortChoice::Manual));
    finish_refresh(&mut app);
    assert_pending_images(&app, &expected);
    assert_eq!(app.thumb_pixels.len(), pixel_count);
    assert!(
        app.thumb_adjust_tex.is_empty(),
        "display material is not a new adjustment owner",
    );
}

#[test]
fn thumbnail_standins_do_not_change_priority_request_admission_or_queue_payload() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 1);
    install_pending_standins(&mut app);
    app.requested.clear();
    let queue: Arc<NotifyQueue> = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    app.reload_queue = Some(queue.clone());
    app.current_folder = Some(PathBuf::from("c:/standins"));
    assert_eq!(
        app.enqueue_priority_thumbnail(0),
        PriorityThumbnailAdmission::Enqueued,
    );
    let with_standin = queue.0.lock().unwrap().pop().unwrap();
    assert!(app.requested.contains_key(&0));
    app.requested.clear();
    app.thumbnail_standins.clear();
    assert_eq!(
        app.enqueue_priority_thumbnail(0),
        PriorityThumbnailAdmission::Enqueued,
    );
    let without_standin = queue.0.lock().unwrap().pop().unwrap();
    assert!(app.requested.contains_key(&0));
    assert_eq!(with_standin.idx, without_standin.idx);
    assert_eq!(with_standin.path, without_standin.path);
    assert_eq!(with_standin.items_gen, without_standin.items_gen);
    assert_eq!(with_standin.mtime, without_standin.mtime);
    assert_eq!(with_standin.file_size, without_standin.file_size);
    assert_eq!(with_standin.priority, without_standin.priority);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
}

#[test]
fn thumbnail_standins_capture_only_exact_sparse_keep_set_and_verified_stamp() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 5);
    app.keep_set = [0, 4].into_iter().collect();
    app.keep_range = (0, 5);
    let captured = app.capture_thumbnail_standins();
    assert_eq!(captured.len(), 2);
    assert!(captured.contains_key(&physical_item_key(&app.items[0]).unwrap()));
    assert!(captured.contains_key(&physical_item_key(&app.items[4]).unwrap()));
    assert!(!captured.contains_key(&physical_item_key(&app.items[2]).unwrap()));
    app.image_metas[4] = None;
    assert_eq!(app.capture_thumbnail_standins().len(), 1);
}

#[test]
fn thumbnail_standins_attach_requires_identity_and_both_stamp_components() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 1);
    let captured = app.capture_thumbnail_standins();
    let key = physical_item_key(&app.items[0]).unwrap();
    let mut owner = ThumbnailStandins::default();
    for (candidate_key, stamp) in [
        (key.as_str(), None),
        (key.as_str(), Some((11, 20))),
        (key.as_str(), Some((10, 21))),
        ("c:/standins/another.jpg", Some((10, 20))),
    ] {
        owner.attach(0, candidate_key, stamp, &captured);
        assert!(owner.images.is_empty());
    }
    owner.attach(0, &key, Some((10, 20)), &captured);
    assert_eq!(owner.images.len(), 1);
}

#[test]
fn thumbnail_standins_pending_recapture_rejects_changed_identity_or_stamp() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 2);
    install_pending_standins(&mut app);
    assert_eq!(app.capture_thumbnail_standins().len(), 2);
    app.image_metas[0] = Some((11, 20));
    app.items[1] = GridItem::Image(PathBuf::from("c:/standins/replacement.jpg"));
    assert!(app.capture_thumbnail_standins().is_empty());
}

#[test]
fn thumbnail_standins_wait_for_first_keep_projection_then_prune_sparse_holes() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 5);
    install_pending_standins(&mut app);
    app.keep_set.clear();
    // Adoption and hydration clear the set before a projection exists. Polling
    // terminal state alone must not interpret this as an empty viewport.
    app.reconcile_thumbnail_standins();
    assert_eq!(app.thumbnail_standins.images.len(), 5);
    app.install_thumbnail_keep_projection(
        ThumbnailKeepProjection {
            pages: [0, 4].into_iter().collect(),
            bounded_range: (0, 5),
            ..Default::default()
        },
        false,
    );
    assert_eq!(app.thumbnail_standins.images.len(), 2);
    assert!(app.grid_thumbnail_display_image(0).is_some());
    assert!(app.grid_thumbnail_display_image(2).is_none());
    assert!(app.grid_thumbnail_display_image(4).is_some());
    assert!(
        app.thumbnails
            .iter()
            .all(|thumb| matches!(thumb, ThumbnailState::Pending))
    );
    app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), false);
    assert!(app.thumbnail_standins.images.is_empty());
}

#[test]
fn thumbnail_standins_hydration_projection_keeps_material_until_first_usable_projection() {
    let (mut app, _) = fixture();
    let expected = seed_loaded(&mut app);
    assert!(app.select_folder_sort(FolderSortChoice::Manual));
    finish_refresh_adoption(&mut app);
    assert!(app.sidecar_restore_active());
    app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), false);
    assert_pending_images(&app, &expected);
    app.finish_page_identity_preparation_for_test();
    assert_pending_images(&app, &expected);
    // Details may reuse the same cached projection that its hover owner built
    // during hydration. It becomes authoritative only after hydration finishes.
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.update_keep_range_and_requests(&egui::Context::default(), Instant::now());
    assert!(app.thumbnail_standins.images.is_empty());
}

#[test]
fn thumbnail_standins_terminal_states_retire_and_loaded_replaces_old_texture() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    synthetic_loaded_with_context(&mut app, 5, &ctx);
    install_pending_standins(&mut app);
    let old_id = app.grid_thumbnail_display_image(0).unwrap().tex.id();
    let replacement = texture(&ctx, "real-replacement");
    let replacement_id = replacement.id();
    assert_ne!(old_id, replacement_id);
    app.thumbnails = vec![
        loaded(replacement),
        ThumbnailState::Failed,
        ThumbnailState::NoArt,
        ThumbnailState::Evicted,
        ThumbnailState::Pending,
    ];
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        replacement_id,
    );
    app.reconcile_thumbnail_standins();
    assert_eq!(
        app.thumbnail_standins
            .images
            .keys()
            .copied()
            .collect::<HashSet<_>>(),
        [4].into_iter().collect(),
    );
    for index in 1..4 {
        assert!(app.grid_thumbnail_display_image(index).is_none());
    }
}

#[test]
fn thumbnail_standins_generation_change_clears_and_stale_generation_cannot_draw() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 1);
    install_pending_standins(&mut app);
    app.thumbnail_standins.generation = app.items_generation.wrapping_sub(1);
    assert!(app.grid_thumbnail_display_image(0).is_none());
    app.reconcile_thumbnail_standins();
    assert!(app.thumbnail_standins.images.is_empty());
    synthetic_loaded(&mut app, 1);
    install_pending_standins(&mut app);
    app.bump_items_generation();
    assert!(app.thumbnail_standins.images.is_empty());
}

#[test]
fn thumbnail_standins_keep_projection_retires_pending_video_despite_eviction_early_return() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 2);
    app.items[1] = GridItem::Video(PathBuf::from("c:/standins/video.mp4"));
    install_pending_standins(&mut app);
    assert!(app.grid_thumbnail_display_image(0).is_some());
    assert!(app.grid_thumbnail_display_image(1).is_some());
    app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), false);
    assert!(app.thumbnail_standins.images.is_empty());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    assert!(matches!(app.thumbnails[1], ThumbnailState::Pending));
}

#[test]
fn thumbnail_standins_unavailable_converted_source_retires_even_while_pending() {
    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 1);
    let path = PathBuf::from("c:/standins/book.7z");
    app.items[0] = GridItem::ConvertibleArchive {
        path: path.clone(),
        format: crate::archive_converter::ArchiveFormat::SevenZ,
    };
    install_pending_standins(&mut app);
    let key = crate::path_key::normalize_keep_drive(&path);
    app.converted_archive_cache_paths
        .insert(key.clone(), ConvertedArchiveSourceState::Pending);
    app.reconcile_thumbnail_standins();
    assert!(app.grid_thumbnail_display_image(0).is_some());
    app.converted_archive_cache_paths.insert(
        key.clone(),
        ConvertedArchiveSourceState::Unavailable {
            logical_source: None,
        },
    );
    assert!(app.grid_thumbnail_display_image(0).is_none());
    app.reconcile_converted_archive_bindings(&[key.clone()].into_iter().collect(), HashSet::new());
    assert!(app.thumbnail_standins.images.is_empty());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    app.converted_archive_cache_paths
        .insert(key, ConvertedArchiveSourceState::Pending);
    assert!(
        app.grid_thumbnail_display_image(0).is_none(),
        "retired material cannot reappear",
    );
}

#[test]
fn thumbnail_standins_image_and_video_keep_painted_index_rotation_after_reindex() {
    use crate::rotation_db::Rotation;

    let mut app = setup_app_for_test();
    synthetic_loaded(&mut app, 4);
    app.items[1] = GridItem::Video(PathBuf::from("c:/standins/1.mp4"));
    app.items[3] = GridItem::Video(PathBuf::from("c:/standins/3.mp4"));
    let angles = [
        Rotation::Cw270,
        Rotation::Cw270,
        Rotation::None,
        Rotation::Cw90,
    ];
    let owner_angles = [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ];
    for index in 0..4 {
        let key = app.rotation_key_for_idx(index).unwrap();
        app.rotation_page_keys.insert(key, owner_angles[index]);
        app.rotation_cache.insert(index, angles[index]);
        assert_eq!(app.grid_tile_rotation_memory(index), owner_angles[index]);
    }
    let reads = crate::rotation_db::read_operations_for_test();
    let expected = (0..4)
        .map(|index| {
            let angle = app.grid_tile_rotation(index);
            assert_eq!(angle, angles[index]);
            let thumb = &app.thumbnails[index];
            let ThumbnailState::Loaded { tex, .. } = thumb else {
                panic!("loaded fixture");
            };
            (
                physical_item_key(&app.items[index]).unwrap(),
                tex.id(),
                thumb.tile_view_for_texture(tex, angle),
                angle,
            )
        })
        .collect::<Vec<_>>();
    let captured = app.capture_thumbnail_standins();
    assert_eq!(crate::rotation_db::read_operations_for_test(), reads);
    app.items.reverse();
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.bump_items_generation();
    for index in 0..4 {
        let key = physical_item_key(&app.items[index]).unwrap();
        let stamp = app.image_metas[index];
        app.thumbnail_standins.attach(index, &key, stamp, &captured);
        app.rotation_cache.insert(index, Rotation::Cw180);
    }
    let reads = crate::rotation_db::read_operations_for_test();
    for (index, item) in app.items.iter().enumerate() {
        let key = physical_item_key(item).unwrap();
        let (_, id, view, angle) = expected.iter().find(|entry| entry.0 == key).unwrap();
        let image = app.grid_thumbnail_display_image(index).unwrap();
        assert_eq!(image.tex.id(), *id);
        assert_eq!(image.view, *view, "layout must use the old painted angle");
        assert_eq!(image.rotation, *angle);
    }
    assert_eq!(crate::rotation_db::read_operations_for_test(), reads);
}

#[test]
fn thumbnail_standins_folder_and_zip_keep_old_cover_rotation_crop_after_reindex() {
    use crate::cover_auto_material::{AutoCropResults, AutoMaterial};
    use crate::cover_source_scope::LoadedCoverIdentity;
    use crate::rotation_db::Rotation;

    let mut env = setup_app_for_test();
    let app = &mut env.app;
    synthetic_loaded(app, 2);
    app.items = vec![
        GridItem::Folder(PathBuf::from("c:/standins/folder")),
        GridItem::ZipFile(PathBuf::from("c:/standins/book.zip")),
    ];
    let crop = crate::folder_thumb_pins::CoverCrop::new([100_000, 200_000, 900_000, 800_000])
        .unwrap()
        .unwrap();
    for (index, key) in [
        "c:/standins/folder/page.jpg",
        "c:/standins/book.zip::page.jpg",
    ]
    .into_iter()
    .enumerate()
    {
        if let ThumbnailState::Loaded {
            cover,
            auto_material,
            ..
        } = &mut app.thumbnails[index]
        {
            *cover = LoadedCoverIdentity::Page(key.into());
            *auto_material =
                AutoMaterial::Automatic(AutoCropResults::new([None, Some(crop), None, None]));
        }
        app.rotation_page_keys.insert(key.into(), Rotation::Cw90);
        app.rotation_cache.insert(index, Rotation::Cw270);
    }
    let old = (0..2)
        .map(|index| app.grid_thumbnail_display_image(index).unwrap())
        .collect::<Vec<_>>();
    assert!(old.iter().all(|image| image.rotation == Rotation::Cw90));
    let full_uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    assert!(old.iter().all(|image| image.view.uv != full_uv));
    let captured = app.capture_thumbnail_standins();
    app.items.reverse();
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.bump_items_generation();
    app.rotation_cache.insert(0, Rotation::Cw180);
    app.rotation_cache.insert(1, Rotation::Cw270);
    for (index, item) in app.items.iter().enumerate() {
        app.thumbnail_standins.attach(
            index,
            &physical_item_key(item).unwrap(),
            app.image_metas[index],
            &captured,
        );
    }
    let reads = crate::rotation_db::read_operations_for_test();
    for index in 0..2 {
        assert_eq!(app.grid_tile_rotation_memory(index), Rotation::None);
        let image = app.grid_thumbnail_display_image(index).unwrap();
        let original = &old[1 - index];
        assert_eq!(image.tex.id(), original.tex.id());
        assert_eq!(image.view, original.view);
        assert_eq!(image.rotation, Rotation::Cw90);
    }
    assert_eq!(crate::rotation_db::read_operations_for_test(), reads);
}

#[cfg(windows)]
#[test]
fn thumbnail_standins_context_swap_close_and_fork_keep_sibling_owner_independent() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    synthetic_loaded(&mut app, 1);
    app.thumbnails[0] = loaded(texture(&ctx, "main-context"));
    install_pending_standins(&mut app);
    let source_id = app.grid_thumbnail_display_image(0).unwrap().tex.id();
    let mut sibling = viewer_context_registry::ViewerContextBundle::empty();
    app.swap_viewer_context_bundle(&mut sibling);
    assert!(app.thumbnail_standins.images.is_empty());
    synthetic_loaded(&mut app, 1);
    app.thumbnails[0] = loaded(texture(&ctx, "sibling-context"));
    install_pending_standins(&mut app);
    let sibling_id = app.grid_thumbnail_display_image(0).unwrap().tex.id();
    app.bump_items_generation();
    assert!(app.thumbnail_standins.images.is_empty());
    assert!(ctx.tex_manager().read().meta(sibling_id).is_none());
    app.thumbnails[0] = loaded(texture(&ctx, "sibling-before-close"));
    app.keep_set.insert(0);
    install_pending_standins(&mut app);
    let closing_id = app.grid_thumbnail_display_image(0).unwrap().tex.id();
    assert!(ctx.tex_manager().read().meta(closing_id).is_some());
    app.swap_viewer_context_bundle(&mut sibling);
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        source_id
    );
    // Closing a parked sibling releases its material without invalidating main.
    drop(sibling);
    assert!(ctx.tex_manager().read().meta(closing_id).is_none());
    assert!(ctx.tex_manager().read().meta(source_id).is_some());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        source_id
    );
    let mut fork = app.split_current_context_preserving_main_grid();
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        source_id
    );
    app.swap_viewer_context_bundle(&mut fork);
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        source_id
    );
    app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), false);
    assert!(app.thumbnail_standins.images.is_empty());
    app.swap_viewer_context_bundle(&mut fork);
    drop(fork);
    assert_eq!(
        app.grid_thumbnail_display_image(0).unwrap().tex.id(),
        source_id
    );
    // Both seeded materials were distinct despite their identical path and stamp.
    assert_ne!(source_id, sibling_id);
    drop(app);
    assert!(ctx.tex_manager().read().meta(source_id).is_none());
}
