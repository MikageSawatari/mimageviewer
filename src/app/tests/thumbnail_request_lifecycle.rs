//! Handler-level regressions for the request/CPU-image lifetime boundary (§1.360).
//! Fake notifications make the source-save and upload ordering deterministic.

use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::thumb_loader::{ThumbLoadOrigin, ThumbMsg, ThumbMsgPayload, ThumbnailRequestId};

fn fixture(item_count: usize) -> AppTestEnv {
    let mut app = setup_app();
    app.items = (0..item_count)
        .map(|idx| GridItem::Image(PathBuf::from(format!(r"c:\pics\{idx}.png"))))
        .collect();
    app.image_metas = vec![Some((10, 20)); item_count];
    app.thumbnails = vec![ThumbnailState::Pending; item_count];
    app.keep_set.extend(0..item_count);
    app.keep_range = (0, item_count);
    app.last_vis_range = (0, item_count);
    app.visible_indices = (0..item_count).collect();
    app.settings.grid_cols = 1;
    app.last_cell_size = 120.0;
    app.last_cell_h = 120.0;
    app.last_viewport_h = 120.0 * item_count as f32;
    app.keep_start_shared.store(0, Ordering::Relaxed);
    app.keep_end_shared.store(item_count, Ordering::Relaxed);
    for idx in 0..item_count {
        let generation = app.items_generation;
        app.requested
            .begin(idx, generation, ThumbnailRequestKind::Normal, None)
            .unwrap();
    }
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.settings.thumb_idle_upgrade = true;
    app.last_scroll_change_time = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.last_input_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
    app.display_px_shared.store(374, Ordering::Relaxed);
    app
}

fn pixels(app: &App, idx: usize, origin: ThumbLoadOrigin) -> ThumbMsg {
    ThumbMsg::from_legacy_parts(
        idx,
        Some(egui::ColorImage::filled(
            [247, 124],
            egui::Color32::LIGHT_BLUE,
        )),
        origin,
        false,
        None,
        Some((884, 444)),
        None,
        false,
        false,
        0,
        app.items_generation,
    )
    .with_request_id(app.requested.id(idx).unwrap())
}

fn finalized(app: &App, idx: usize) -> ThumbMsg {
    ThumbMsg::from_legacy_parts(
        idx,
        None,
        ThumbLoadOrigin::SourceIntrinsic,
        false,
        None,
        None,
        None,
        false,
        true,
        0,
        app.items_generation,
    )
    .with_request_id(app.requested.id(idx).unwrap())
}

fn source_pixels(app: &App, idx: usize) -> ThumbMsg {
    pixels(
        app,
        idx,
        ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 374,
        },
    )
    .source_pixels()
}

fn assert_worker_queues_empty(app: &App) {
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(
        app.heavy_io_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
}

fn assert_one_audio_request_on_heavy_queue(app: &App) {
    // Audio discovery/extraction is routed by GridItem::is_heavy_io, just like representatives.
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    let queue = app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap();
    assert_eq!(queue.len(), 1);
    let request = &queue[0];
    assert_eq!(request.idx, 0);
    assert_eq!(request.items_gen, app.items_generation);
    assert_eq!(Some(request.request_id), app.requested.id(0));
    let GridItem::Audio(path) = &app.items[0] else {
        panic!("audio fixture")
    };
    assert_eq!(&request.path, path);
    assert!(matches!(
        request.raw_source,
        crate::thumb_loader::LoadRequestSource::AudioThumbnail(_)
    ));
}

#[derive(Clone, Copy)]
enum ExistingDisplay {
    Pending,
    Evicted,
    Upgrade,
}

fn ninth_source_image_keeps_request(existing: ExistingDisplay) {
    let mut app = fixture(9);
    let ctx = egui::Context::default();
    let previous_texture = ctx.load_texture(
        "lifecycle-old-upgrade",
        egui::ColorImage::filled([100, 50], egui::Color32::DARK_GRAY),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[8] = match existing {
        ExistingDisplay::Pending => ThumbnailState::Pending,
        ExistingDisplay::Evicted => ThumbnailState::Evicted,
        ExistingDisplay::Upgrade => ThumbnailState::Loaded {
            tex: previous_texture.clone(),
            origin: ThumbLoadOrigin::UpgradeableCache,
            from_edit_preview: false,
            rendered_at_px: 100,
            source_dims: Some((10, 20)),
            layout_dims: None,
        },
    };
    if matches!(existing, ExistingDisplay::Upgrade) {
        app.requested.remove(&8);
        let generation = app.items_generation;
        app.requested
            .begin(8, generation, ThumbnailRequestKind::Upgrade, None)
            .unwrap();
    }
    for idx in 0..8 {
        app.tx
            .send(pixels(&app, idx, ThumbLoadOrigin::FinalCache))
            .unwrap();
    }
    app.tx.send(source_pixels(&app, 8)).unwrap();
    app.tx.send(finalized(&app, 8)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);

    assert_eq!(
        app.requested.images_len(),
        1,
        "the ninth image must respect the eight-upload budget"
    );
    if matches!(existing, ExistingDisplay::Upgrade) {
        match &app.thumbnails[8] {
            ThumbnailState::Loaded {
                tex,
                origin,
                rendered_at_px,
                source_dims,
                ..
            } => {
                assert_eq!(tex.id(), previous_texture.id());
                assert_eq!(*origin, ThumbLoadOrigin::UpgradeableCache);
                assert_eq!(*rendered_at_px, 100);
                assert_eq!(*source_dims, Some((10, 20)));
            }
            _ => panic!("the previous upgrade texture must remain visible until adoption"),
        }
    }
    assert!(
        app.requested.contains_key(&8),
        "Finalized cannot release a request whose image is queued"
    );
    assert_eq!(
        app.image_metas[8],
        Some((10, 20)),
        "queued pixels must not publish adopted dimensions"
    );
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    app.enqueue_idle_upgrades();
    assert_worker_queues_empty(&app);

    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(
        app.thumbnails[8],
        ThumbnailState::Loaded {
            origin: ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 374
            },
            rendered_at_px: 247,
            source_dims: Some((884, 444)),
            ..
        }
    ));
    assert!(app.requested.images_len() == 0);
    assert!(!app.requested.contains_key(&8));
    app.enqueue_idle_upgrades();
    assert_worker_queues_empty(&app);
}

#[test]
fn lifecycle_ninth_pending_source_waits_for_adoption() {
    ninth_source_image_keeps_request(ExistingDisplay::Pending);
}

#[test]
fn lifecycle_ninth_evicted_source_waits_for_adoption() {
    ninth_source_image_keeps_request(ExistingDisplay::Evicted);
}

#[test]
fn lifecycle_ninth_loaded_upgrade_source_waits_for_adoption() {
    ninth_source_image_keeps_request(ExistingDisplay::Upgrade);
}

#[test]
fn lifecycle_keep_exit_discards_queued_source_but_waits_for_finalized() {
    let mut app = fixture(9);
    let ctx = egui::Context::default();
    for idx in 0..8 {
        app.tx
            .send(pixels(&app, idx, ThumbLoadOrigin::FinalCache))
            .unwrap();
    }
    app.tx.send(source_pixels(&app, 8)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(app.requested.images_len(), 1);

    app.install_thumbnail_keep_projection(
        ThumbnailKeepProjection {
            pages: (0..8).collect(),
            bounded_range: (0, 8),
            interactive_pages: HashSet::new(),
        },
        true,
    );
    assert!(app.requested.images_len() == 0);
    assert!(
        app.requested.contains_key(&8),
        "discarding an image does not prove source saving ended"
    );

    app.install_thumbnail_keep_projection(
        ThumbnailKeepProjection {
            pages: (0..9).collect(),
            bounded_range: (0, 9),
            interactive_pages: HashSet::new(),
        },
        true,
    );
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_worker_queues_empty(&app);
    app.tx.send(finalized(&app, 8)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(
        !app.requested.contains_key(&8),
        "both terminals must release the discarded request"
    );
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert!(app.requested.contains_key(&8));
    let queue_len = app.reload_queue.as_ref().unwrap().0.lock().unwrap().len()
        + app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap().len();
    assert_eq!(queue_len, 1, "terminal discard permits one required reload");
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_eq!(
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().len()
            + app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap().len(),
        1,
        "the new reservation must prevent duplicate reloads",
    );
}

#[test]
fn lifecycle_out_of_keep_source_pixels_wait_for_finalized() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    app.keep_set.clear();
    app.keep_range = (0, 0);
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.requested.images_len() == 0);
    assert!(
        app.requested.contains_key(&0),
        "out-of-range discard must retain the source-save reservation"
    );
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(!app.requested.contains_key(&0));
}

#[test]
fn lifecycle_adopted_source_remains_reserved_until_finalized_after_gpu_eviction() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert!(app.requested.contains_key(&0));
    app.evict_grid_thumbnail(0);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert!(
        app.requested.contains_key(&0),
        "GPU eviction does not retire a saving worker"
    );
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_worker_queues_empty(&app);
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(!app.requested.contains_key(&0));
}

#[test]
fn lifecycle_finalized_before_pixels_evicted_waits_for_image() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    app.thumbnails[0] = ThumbnailState::Evicted;
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(
        app.requested.contains_key(&0),
        "worker completion cannot substitute for image completion"
    );
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_worker_queues_empty(&app);
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert!(!app.requested.contains_key(&0));
}

#[test]
fn lifecycle_terminal_cache_image_finishes_only_after_backlog_adoption() {
    let mut app = fixture(9);
    let ctx = egui::Context::default();
    for idx in 0..9 {
        app.tx
            .send(pixels(&app, idx, ThumbLoadOrigin::FinalCache))
            .unwrap();
    }
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(app.requested.images_len(), 1);
    assert!(app.requested.contains_key(&8));
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_worker_queues_empty(&app);
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(
        app.requested.is_empty(),
        "a cache result has no second Finalized notification"
    );
}

#[test]
fn lifecycle_terminal_cache_image_outside_keep_finishes_on_discard() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    app.keep_set.clear();
    app.keep_range = (0, 0);
    app.tx
        .send(pixels(&app, 0, ThumbLoadOrigin::FinalCache))
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.requested.is_empty());
    assert!(app.requested.images_len() == 0);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
}

fn notification(
    idx: usize,
    generation: u64,
    id: ThumbnailRequestId,
    payload: ThumbMsgPayload,
) -> ThumbMsg {
    ThumbMsg {
        idx,
        items_gen: generation,
        request_id: id,
        input_seq: 0,
        payload,
    }
}

fn owner_pixels(idx: usize, generation: u64, id: ThumbnailRequestId) -> ThumbMsg {
    ThumbMsg::from_legacy_parts(
        idx,
        Some(egui::ColorImage::filled([1, 1], egui::Color32::LIGHT_BLUE)),
        ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 374,
        },
        false,
        None,
        Some((884, 444)),
        None,
        false,
        false,
        0,
        generation,
    )
    .with_request_id(id)
    .source_pixels()
}

fn settle_next(owner: &mut ThumbnailRequests) {
    let image = owner.next_image().expect("one owned upload");
    owner.settle_image(image.idx, image.request_id);
}

fn owner_seed(idx: usize, generation: u64, id: ThumbnailRequestId) -> ThumbMsg {
    let mut msg = owner_pixels(idx, generation, id);
    let ThumbMsgPayload::SourcePixels(pixels) = msg.payload else {
        panic!("source")
    };
    msg.payload = ThumbMsgPayload::ProvisionalPixels(pixels);
    msg
}

#[test]
fn lifecycle_source_event_orders_complete_at_both_terminals() {
    // S settles the image (adoption and explicit discard share this boundary).
    for events in ["PSF", "PFS", "FPS"] {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(0, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        for (step, event) in events.chars().enumerate() {
            match event {
                'P' => {
                    owner.receive(owner_pixels(0, 7, id));
                }
                'F' => {
                    owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized));
                }
                'S' => settle_next(&mut owner),
                _ => unreachable!(),
            }
            assert_eq!(
                owner.contains_key(&0),
                step != 2,
                "order {events}, step {step}"
            );
            if step != 2 {
                assert!(
                    owner
                        .begin(0, 7, ThumbnailRequestKind::Normal, None)
                        .is_none()
                );
            }
        }
        assert_eq!(owner.images_len(), 0);
        assert!(matches!(
            owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized)),
            ThumbnailReceipt::Ignored
        ));
        assert!(
            owner
                .begin(0, 7, ThumbnailRequestKind::Normal, None)
                .is_some()
        );
    }
}

#[test]
fn lifecycle_duplicate_pixels_and_finalized_do_not_grow_or_repeat_side_effects() {
    let mut owner = ThumbnailRequests::default();
    let id = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    owner.receive(owner_pixels(0, 7, id));
    assert!(matches!(
        owner.receive(owner_pixels(0, 7, id)),
        ThumbnailReceipt::Ignored
    ));
    assert_eq!(owner.images_len(), 1);
    assert!(matches!(
        owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized)),
        ThumbnailReceipt::Finalized {
            source_changed: true
        }
    ));
    assert!(matches!(
        owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized)),
        ThumbnailReceipt::Ignored
    ));
    settle_next(&mut owner);
    assert!(!owner.blocks_idle_upgrade());
}

#[test]
fn lifecycle_no_image_terminals_discard_owned_pixels_and_release_once() {
    for terminal in 0..3 {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(0, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(owner_pixels(0, 7, id));
        let payload = match terminal {
            0 => ThumbMsgPayload::NoArt,
            1 => ThumbMsgPayload::Failed {
                origin: ThumbLoadOrigin::SourceIntrinsic,
            },
            _ => ThumbMsgPayload::Canceled,
        };
        assert!(matches!(
            owner.receive(notification(0, 7, id, payload)),
            ThumbnailReceipt::Terminal(_)
        ));
        assert!(owner.is_empty());
        assert_eq!(owner.images_len(), 0);
        assert!(owner.next_image().is_none());
        assert!(matches!(
            owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized)),
            ThumbnailReceipt::Ignored
        ));
    }
}

#[test]
fn lifecycle_local_prune_cannot_cancel_a_new_request_at_same_index() {
    let mut owner = ThumbnailRequests::default();
    let first = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    owner.cancel_queued(0, first);
    assert!(owner.is_empty());
    let second = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    owner.cancel_queued(0, first);
    assert_eq!(owner.id(0), Some(second));
    owner.cancel_queued(0, second);
    assert!(owner.is_empty());
}

#[test]
fn lifecycle_provisional_seed_replacement_has_one_upload_and_miss_discards_it() {
    for adopt_seed in [false, true] {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(0, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(owner_seed(0, 7, id));
        if adopt_seed {
            settle_next(&mut owner);
        }
        let mut replacement = owner_pixels(0, 7, id);
        let ThumbMsgPayload::SourcePixels(pixels) = replacement.payload else {
            panic!("source image")
        };
        replacement.payload = ThumbMsgPayload::Pixels(pixels);
        owner.receive(replacement);
        assert_eq!(owner.images_len(), 1);
        settle_next(&mut owner);
        assert!(owner.is_empty());
        assert!(owner.next_image().is_none());
    }
    let mut owner = ThumbnailRequests::default();
    let id = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    owner.receive(owner_seed(0, 7, id));
    owner.receive(notification(
        0,
        7,
        id,
        ThumbMsgPayload::Failed {
            origin: ThumbLoadOrigin::DriveListChildMiss,
        },
    ));
    assert_eq!(owner.images_len(), 0);
    assert!(owner.is_empty());
}

#[test]
fn lifecycle_provisional_confirmation_retains_seed_until_adoption_without_source_invalidation() {
    let mut owner = ThumbnailRequests::default();
    let id = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    owner.receive(owner_seed(0, 7, id));
    assert!(matches!(
        owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized)),
        ThumbnailReceipt::Finalized {
            source_changed: false
        }
    ));
    assert!(owner.contains_key(&0));
    settle_next(&mut owner);
    assert!(owner.is_empty());
}

#[test]
fn lifecycle_old_same_generation_notifications_cannot_mutate_new_request_or_display() {
    let mut app = fixture(1);
    app.items[0] = GridItem::ZipFile(PathBuf::from(r"c:\pics\representative.zip"));
    let ctx = egui::Context::default();
    let old_id = app.requested.id(0).unwrap();
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    let generation = app.items_generation;
    let new_id = app
        .requested
        .begin(0, generation, ThumbnailRequestKind::Upgrade, None)
        .unwrap();
    let old_texture = match &app.thumbnails[0] {
        ThumbnailState::Loaded { tex, .. } => tex.id(),
        _ => panic!("loaded"),
    };
    let revisions = app.color_filter.palettes.representative_revisions.clone();
    assert_eq!(
        revisions.len(),
        1,
        "the fixture must exercise representative invalidation"
    );
    let metas = app.image_metas.clone();
    for terminal in 0..6 {
        let stale = match terminal {
            0 => owner_pixels(0, generation, old_id),
            1 => notification(0, generation, old_id, ThumbMsgPayload::Finalized),
            2 => notification(0, generation, old_id, ThumbMsgPayload::Canceled),
            3 => notification(
                0,
                generation,
                old_id,
                ThumbMsgPayload::Failed {
                    origin: ThumbLoadOrigin::SourceIntrinsic,
                },
            ),
            4 => notification(0, generation, old_id, ThumbMsgPayload::NoArt),
            _ => notification(0, generation, old_id, ThumbMsgPayload::CacheOnlyMiss),
        };
        app.tx.send(stale).unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert_eq!(app.requested.id(0), Some(new_id));
        assert_eq!(app.requested.images_len(), 0);
        assert_eq!(app.image_metas, metas);
        assert_eq!(
            app.color_filter.palettes.representative_revisions,
            revisions
        );
        match &app.thumbnails[0] {
            ThumbnailState::Loaded {
                tex,
                origin,
                source_dims,
                ..
            } => {
                assert_eq!(tex.id(), old_texture);
                assert_eq!(
                    *origin,
                    ThumbLoadOrigin::SourceGenerated {
                        evaluated_display_px: 374
                    }
                );
                assert_eq!(*source_dims, Some((884, 444)));
            }
            _ => panic!("late old terminal must preserve the current display"),
        }
    }
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.requested.is_empty());
}

#[test]
fn lifecycle_generation_and_source_retirement_reject_late_results() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    let stale = source_pixels(&app, 0);
    let old_id = stale.request_id;
    app.evict_thumbnail_for_reload(0);
    let generation = app.items_generation;
    let new_id = app
        .requested
        .begin(0, generation, ThumbnailRequestKind::Normal, None)
        .unwrap();
    assert_ne!(old_id, new_id);
    app.tx.send(stale).unwrap();
    app.tx
        .send(owner_pixels(0, generation.wrapping_sub(1), new_id))
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert_eq!(app.requested.id(0), Some(new_id));
    assert_eq!(app.requested.images_len(), 0);
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
}

#[test]
fn lifecycle_duplicate_finalized_does_not_repeat_color_invalidation() {
    let mut app = fixture(9);
    app.items[8] = GridItem::ZipFile(PathBuf::from(r"c:\pics\representative.zip"));
    let ctx = egui::Context::default();
    for idx in 0..8 {
        app.tx
            .send(pixels(&app, idx, ThumbLoadOrigin::FinalCache))
            .unwrap();
    }
    app.tx.send(source_pixels(&app, 8)).unwrap();
    app.tx.send(finalized(&app, 8)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    let revisions = app.color_filter.palettes.representative_revisions.clone();
    assert_eq!(revisions.len(), 1);
    // Duplicate is received in the next frame while the source image is still queued.
    app.tx.send(finalized(&app, 8)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.color_filter.palettes.representative_revisions,
        revisions
    );
}

#[test]
fn lifecycle_hide_restore_disposes_images_and_preserves_unfinished_source_requests() {
    // Hide before/after the worker terminal; restore before/after it. No native HWND exists.
    for phase in 0..4 {
        for restore_first in [false, true] {
            let mut app = fixture(1);
            let ctx = egui::Context::default();
            app.main_hwnd = None;
            let id = app.requested.id(0).unwrap();
            if phase != 0 {
                let msg = source_pixels(&app, 0);
                app.requested.receive(msg);
                if phase == 2 {
                    settle_next(&mut app.requested);
                }
                if phase == 3 {
                    let msg = finalized(&app, 0);
                    app.requested.receive(msg);
                }
            }
            app.window_visible = true;
            app.hide_to_tray(&ctx);
            assert_eq!(app.requested.images_len(), 0);
            assert_eq!(app.requested.id(0), (phase != 3).then_some(id));
            if restore_first {
                app.sync_after_restore(&ctx);
            }
            if phase == 0 {
                app.tx.send(source_pixels(&app, 0)).unwrap();
                app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            }
            if phase != 3 {
                assert_eq!(app.requested.id(0), Some(id));
                app.tx.send(finalized(&app, 0)).unwrap();
                app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            }
            if !restore_first {
                app.sync_after_restore(&ctx);
            }
            assert!(
                app.requested.is_empty(),
                "hide phase {phase}, restore_first {restore_first}"
            );
            assert!(app.window_visible);
        }
    }
}

#[test]
fn lifecycle_video_retry_does_not_block_idle_upgrade_or_deadline() {
    let mut app = fixture(2);
    let ctx = egui::Context::default();
    app.requested.clear();
    app.items[1] = GridItem::Video(PathBuf::from(r"c:\pics\retry.mp4"));
    app.keep_set = HashSet::from([0]);
    app.keep_range = (0, 1);
    app.thumbnails[0] = ThumbnailState::Loaded {
        tex: ctx.load_texture(
            "upgrade-alongside-video",
            egui::ColorImage::filled([100, 50], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        ),
        origin: ThumbLoadOrigin::UpgradeableCache,
        from_edit_preview: false,
        rendered_at_px: 100,
        source_dims: Some((884, 444)),
        layout_dims: None,
    };
    let generation = app.items_generation;
    let video = app
        .requested
        .begin(1, generation, ThumbnailRequestKind::VideoBatch, None)
        .unwrap();
    app.last_scroll_change_time = std::time::Instant::now();
    app.last_input_at = Some(std::time::Instant::now());
    let delay = app
        .thumb_idle_upgrade_recheck_delay()
        .expect("video retry must preserve the existing idle deadline");
    assert!(delay > std::time::Duration::from_millis(400));
    assert!(delay <= std::time::Duration::from_millis(550));
    app.enqueue_idle_upgrades();
    assert!(app.requested.id(0).is_none());
    app.last_scroll_change_time = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.last_input_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
    app.enqueue_idle_upgrades();
    assert!(app.requested.id(0).is_some());
    assert_eq!(app.requested.id(1), Some(video));
    assert_eq!(app.requested.get(&0), Some(&true));
    assert_eq!(
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().len(),
        1
    );
    app.tx.send(source_pixels(&app, 0)).unwrap();
    app.tx.send(finalized(&app, 0)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(!app.requested.blocks_idle_upgrade());
    assert_eq!(app.requested.id(1), Some(video));
}

#[test]
fn lifecycle_normal_and_upgrade_block_idle_in_every_image_phase_outside_keep() {
    for kind in [ThumbnailRequestKind::Normal, ThumbnailRequestKind::Upgrade] {
        for phase in 0..3 {
            let mut owner = ThumbnailRequests::default();
            let id = owner.begin(0, 7, kind, None).unwrap();
            if phase > 0 {
                owner.receive(owner_pixels(0, 7, id));
            }
            if phase > 1 {
                settle_next(&mut owner);
            }
            assert!(owner.blocks_idle_upgrade());
            assert_eq!(owner.progress_counts(&HashSet::new()), (0, 0, 0));
            owner.receive(notification(0, 7, id, ThumbMsgPayload::Finalized));
            if phase == 0 {
                owner.receive(owner_pixels(0, 7, id));
            }
            if phase != 2 {
                settle_next(&mut owner);
            }
            assert!(!owner.blocks_idle_upgrade());
        }
    }
}

fn repaint_projection(owner: &ThumbnailRequests, keep: &HashSet<usize>) -> (bool, bool, bool) {
    (
        owner.blocks_idle_upgrade(),
        owner.images_len() != 0,
        owner.video_image_pending(keep),
    )
}

#[test]
fn lifecycle_video_repaint_projection_changes_only_with_demand_or_owned_image() {
    let mut owner = ThumbnailRequests::default();
    let id = owner
        .begin(0, 7, ThumbnailRequestKind::VideoBatch, None)
        .unwrap();
    let outside = HashSet::new();
    let inside = HashSet::from([0]);
    for _ in 0..100 {
        assert_eq!(repaint_projection(&owner, &outside), (false, false, false));
        assert_eq!(owner.legacy_len(), 0);
    }
    assert_eq!(repaint_projection(&owner, &inside), (false, false, true));
    assert_eq!(repaint_projection(&owner, &outside), (false, false, false));
    let mut terminal = owner_pixels(0, 7, id);
    let ThumbMsgPayload::SourcePixels(pixels) = terminal.payload else {
        panic!("source")
    };
    terminal.payload = ThumbMsgPayload::Pixels(pixels);
    owner.receive(terminal);
    assert_eq!(repaint_projection(&owner, &outside), (false, true, false));
    assert_eq!(repaint_projection(&owner, &inside), (false, true, true));
    settle_next(&mut owner);
    assert_eq!(repaint_projection(&owner, &inside), (false, false, false));
}

#[test]
fn lifecycle_grid_keeps_out_of_range_video_and_rendition_discards_it() {
    for consumption in [
        ThumbnailConsumptionPolicy::Grid,
        ThumbnailConsumptionPolicy::PassthroughRendition,
    ] {
        let mut app = fixture(1);
        let ctx = egui::Context::default();
        app.requested.clear();
        app.items[0] = GridItem::Video(PathBuf::from(r"c:\pics\clip.mp4"));
        app.keep_set.clear();
        app.keep_range = (0, 0);
        let generation = app.items_generation;
        app.requested
            .begin(0, generation, ThumbnailRequestKind::VideoBatch, None)
            .unwrap();
        app.tx
            .send(pixels(&app, 0, ThumbLoadOrigin::SourceIntrinsic))
            .unwrap();
        app.poll_thumbnails(&ctx, consumption);
        assert!(app.requested.is_empty());
        assert_eq!(
            matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }),
            consumption.keeps_out_of_range_video_texture()
        );
    }
}

#[test]
fn lifecycle_video_and_direct_images_never_enter_normal_or_upgrade_progress() {
    let mut app = fixture(4);
    app.requested.clear();
    let generation = app.items_generation;
    for (idx, kind) in [
        (0, ThumbnailRequestKind::VideoBatch),
        (1, ThumbnailRequestKind::DirectPixels),
    ] {
        let id = app.requested.begin(idx, generation, kind, None).unwrap();
        assert_eq!(app.count_pending(), (0, 0));
        app.update_progress_peaks();
        assert_eq!(app.progress_snapshot(), ((0, 0), (0, 0)));
        let msg = pixels(&app, idx, ThumbLoadOrigin::FinalCache);
        app.requested.receive(msg);
        assert_eq!(app.requested.progress_counts(&app.keep_set), (0, 0, 0));
        assert!(!app.requested.blocks_idle_upgrade());
        app.requested.cancel_queued(idx, id);
    }
    assert_eq!(app.cache_gen_total, 0);
    assert_eq!(app.cache_gen_done.load(Ordering::Relaxed), 0);
}

#[test]
fn lifecycle_mixed_progress_subtracts_only_queued_normal_images_in_demand() {
    let mut app = fixture(6);
    app.requested.clear();
    app.keep_set = HashSet::from([0, 1, 2, 3, 4]);
    let generation = app.items_generation;
    for (idx, kind) in [
        (0, ThumbnailRequestKind::Normal),
        (1, ThumbnailRequestKind::Normal),
        (2, ThumbnailRequestKind::Upgrade),
        (3, ThumbnailRequestKind::VideoBatch),
        (4, ThumbnailRequestKind::DirectPixels),
        (5, ThumbnailRequestKind::Normal),
    ] {
        app.requested.begin(idx, generation, kind, None).unwrap();
    }
    for idx in [2, 3, 4, 5] {
        let msg = source_pixels(&app, idx);
        app.requested.receive(msg);
    }
    assert_eq!(app.requested.progress_counts(&app.keep_set), (2, 1, 0));
    app.update_progress_peaks();
    assert_eq!(
        app.progress_snapshot(),
        ((2, 2), (1, 1)),
        "unrelated images cannot lower the normal peak input"
    );
    let msg = source_pixels(&app, 0);
    app.requested.receive(msg);
    assert_eq!(app.requested.progress_counts(&app.keep_set), (2, 1, 1));
    app.progress_normal_peak = 0;
    app.update_progress_peaks();
    assert_eq!(app.progress_snapshot(), ((2, 1), (1, 1)));
    let keep = app.keep_set.clone();
    app.requested.discard_images_outside(&keep);
    assert_eq!(app.requested.progress_counts(&app.keep_set), (2, 1, 1));
    assert_eq!(app.cache_gen_total, 0);
    assert_eq!(app.cache_gen_done.load(Ordering::Relaxed), 0);
}

#[test]
#[cfg(windows)]
fn lifecycle_sibling_mount_hide_restore_and_retire_preserve_owned_resources() {
    let mut app = fixture(2);
    let ctx = egui::Context::default();
    app.main_hwnd = None;
    let main_generation = app.items_generation;
    let main_token = app.cancel_token.clone();
    let main_reload = app.reload_queue.clone().unwrap();
    let main_heavy = app.heavy_io_queue.clone().unwrap();
    let main_keep_start = app.keep_start_shared.clone();
    let main_keep_end = app.keep_end_shared.clone();
    let msg = source_pixels(&app, 0);
    app.requested.receive(msg);
    app.requested.remove(&1);
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    let main_snapshot = app.requested.snapshot();
    let main_queue_sizes = (
        main_reload.0.lock().unwrap().len(),
        main_heavy.0.lock().unwrap().len(),
    );
    let main_keep = app.keep_set.clone();
    let main_bounds = (
        main_keep_start.load(Ordering::Relaxed),
        main_keep_end.load(Ordering::Relaxed),
    );
    let main_tx = app.tx.clone();

    // build_viewer_context creates ViewerContextBundle::empty, mounts it, then saves it back.
    let sibling_texture = ctx.load_texture(
        "sibling-old",
        egui::ColorImage::filled([2, 2], egui::Color32::GRAY),
        egui::TextureOptions::LINEAR,
    );
    let sibling = app.build_window_context_for_test(1360, |mounted| {
        mounted.items_generation = main_generation;
        mounted.items = vec![GridItem::Image(PathBuf::from(r"c:\sibling\0.png"))];
        mounted.image_metas = vec![Some((10, 20))];
        mounted.thumbnails = vec![ThumbnailState::Loaded {
            tex: sibling_texture.clone(),
            origin: ThumbLoadOrigin::UpgradeableCache,
            from_edit_preview: false,
            rendered_at_px: 2,
            source_dims: Some((10, 20)),
            layout_dims: None,
        }];
        mounted.keep_set = HashSet::from([0]);
        mounted.keep_range = (0, 1);
        mounted.keep_start_shared.store(0, Ordering::Relaxed);
        mounted.keep_end_shared.store(1, Ordering::Relaxed);
        mounted.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        mounted.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        let id = mounted
            .requested
            .begin(0, main_generation, ThumbnailRequestKind::Normal, None)
            .unwrap();
        mounted
            .requested
            .receive(owner_pixels(0, main_generation, id));
    });
    assert_eq!(app.requested.snapshot(), main_snapshot);
    assert_eq!(app.requested.images_len(), 1);
    let (
        sibling_snapshot,
        sibling_token,
        sibling_reload,
        sibling_heavy,
        sibling_keep_start,
        sibling_keep_end,
        sibling_tx,
    ) = app
        .with_viewer_context(sibling, |mounted| {
            assert_eq!(mounted.items_generation, main_generation);
            assert_eq!(mounted.requested.images_len(), 1);
            assert!(!Arc::ptr_eq(&mounted.cancel_token, &main_token));
            (
                mounted.requested.snapshot(),
                mounted.cancel_token.clone(),
                mounted.reload_queue.clone().unwrap(),
                mounted.heavy_io_queue.clone().unwrap(),
                mounted.keep_start_shared.clone(),
                mounted.keep_end_shared.clone(),
                mounted.tx.clone(),
            )
        })
        .unwrap();
    let sibling_id = sibling_snapshot[0].1;
    // Prove the channels move with their context and are independent, rather than comparing Senders.
    sibling_tx
        .send(notification(
            0,
            main_generation,
            sibling_id,
            ThumbMsgPayload::Finalized,
        ))
        .unwrap();
    main_tx
        .send(notification(
            0,
            main_generation,
            main_snapshot[0].1,
            ThumbMsgPayload::Finalized,
        ))
        .unwrap();
    let main_message = app.rx.try_recv().unwrap();
    assert_eq!(main_message.request_id, main_snapshot[0].1);
    assert!(app.rx.try_recv().is_err());
    app.with_viewer_context(sibling, |mounted| {
        assert_eq!(mounted.rx.try_recv().unwrap().request_id, sibling_id);
        assert!(mounted.rx.try_recv().is_err());
    })
    .unwrap();

    app.window_visible = true;
    app.hide_to_tray(&ctx);
    app.sync_after_restore(&ctx);
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(
        app.requested
            .snapshot()
            .iter()
            .map(|r| r.1)
            .collect::<Vec<_>>(),
        main_snapshot.iter().map(|r| r.1).collect::<Vec<_>>()
    );
    assert_eq!(app.requested.images_len(), 0);
    assert!(Arc::ptr_eq(&app.cancel_token, &main_token));
    assert!(!main_token.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        app.reload_queue.as_ref().unwrap(),
        &main_reload
    ));
    assert!(Arc::ptr_eq(
        app.heavy_io_queue.as_ref().unwrap(),
        &main_heavy
    ));
    assert_eq!(
        (
            main_reload.0.lock().unwrap().len(),
            main_heavy.0.lock().unwrap().len()
        ),
        main_queue_sizes
    );
    assert_eq!(app.keep_set, main_keep);
    assert_eq!(
        (
            main_keep_start.load(Ordering::Relaxed),
            main_keep_end.load(Ordering::Relaxed)
        ),
        main_bounds
    );
    for operation in 0..5 {
        match operation {
            0 => {}
            1 => {
                app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), true);
            }
            2 => {
                if let Some(id) = app.requested.id(0) {
                    app.tx
                        .send(notification(
                            0,
                            app.items_generation,
                            id,
                            ThumbMsgPayload::Canceled,
                        ))
                        .unwrap();
                    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
                }
                app.cancel_token.store(true, Ordering::Relaxed);
            }
            3 => app.install_prepared_aggregate_items(
                vec![GridItem::Image(PathBuf::from(r"c:\new\0.png"))],
                vec![Some((10, 20))],
            ),
            4 => {
                app.fullscreen_idx = Some(0);
                app.close_fullscreen();
            }
            _ => unreachable!(),
        }
        sibling_tx
            .send(notification(
                0,
                main_generation,
                sibling_id,
                ThumbMsgPayload::Canceled,
            ))
            .unwrap();
        assert!(
            app.rx.try_recv().is_err(),
            "main operation cannot redirect the sibling channel"
        );
        app.with_viewer_context(sibling, |mounted| {
            assert_eq!(mounted.rx.try_recv().unwrap().request_id, sibling_id);
            assert!(mounted.rx.try_recv().is_err());
            assert_eq!(mounted.requested.snapshot(), sibling_snapshot);
            let image = mounted.requested.images().next().unwrap().pixels().unwrap();
            assert_eq!(image.source_dims, Some((884, 444)));
            assert_eq!(mounted.requested.images_len(), 1);
            assert_eq!(mounted.items_generation, main_generation);
            assert!(matches!(
                mounted.thumbnails[0],
                ThumbnailState::Loaded {
                    origin: ThumbLoadOrigin::UpgradeableCache,
                    rendered_at_px: 2,
                    source_dims: Some((10, 20)),
                    ..
                }
            ));
            match &mounted.thumbnails[0] {
                ThumbnailState::Loaded { tex, .. } => assert_eq!(tex.id(), sibling_texture.id()),
                _ => panic!("sibling texture must survive every main operation"),
            }
            assert_eq!(mounted.image_metas, vec![Some((10, 20))]);
            assert!(Arc::ptr_eq(&mounted.cancel_token, &sibling_token));
            assert!(!sibling_token.load(Ordering::Relaxed));
            assert!(Arc::ptr_eq(
                mounted.reload_queue.as_ref().unwrap(),
                &sibling_reload
            ));
            assert!(Arc::ptr_eq(
                mounted.heavy_io_queue.as_ref().unwrap(),
                &sibling_heavy
            ));
            assert_eq!(sibling_reload.0.lock().unwrap().len(), 0);
            assert_eq!(sibling_heavy.0.lock().unwrap().len(), 0);
            assert!(Arc::ptr_eq(&mounted.keep_start_shared, &sibling_keep_start));
            assert!(Arc::ptr_eq(&mounted.keep_end_shared, &sibling_keep_end));
            assert_eq!(mounted.keep_set, HashSet::from([0]));
            assert_eq!(
                (
                    sibling_keep_start.load(Ordering::Relaxed),
                    sibling_keep_end.load(Ordering::Relaxed)
                ),
                (0, 1)
            );
        })
        .unwrap();
    }
    let main_after_hide = app.requested.snapshot();
    let main_generation_after_operations = app.items_generation;
    app.retire_context(sibling, "thumbnail_lifecycle_test", |_| ())
        .unwrap();
    assert_eq!(app.requested.snapshot(), main_after_hide);
    assert_eq!(app.items_generation, main_generation_after_operations);
}

#[test]
fn lifecycle_audio_terminal_image_needs_no_finalized_and_no_art_waits_for_maintenance() {
    for no_art in [false, true] {
        let mut app = fixture(1);
        let ctx = egui::Context::default();
        app.items[0] = GridItem::Audio(PathBuf::from(r"c:\pics\song.flac"));
        if no_art {
            let id = app.requested.id(0).unwrap();
            app.tx
                .send(notification(
                    0,
                    app.items_generation,
                    id,
                    ThumbMsgPayload::NoArt,
                ))
                .unwrap();
        } else {
            app.tx
                .send(pixels(
                    &app,
                    0,
                    ThumbLoadOrigin::SourceGenerated {
                        evaluated_display_px: 374,
                    },
                ))
                .unwrap();
        }
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(app.requested.is_empty());
        assert_eq!(matches!(app.thumbnails[0], ThumbnailState::NoArt), no_art);
        app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
        assert_worker_queues_empty(&app);
        if no_art {
            assert_eq!(
                app.invalidate_audio_art_terminals_after_catalog_maintenance(None),
                1
            );
            app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
            assert!(app.requested.contains_key(&0));
            assert_one_audio_request_on_heavy_queue(&app);
        }
    }
}

#[test]
fn lifecycle_audio_failed_and_canceled_keep_distinct_retry_behavior() {
    for canceled in [false, true] {
        let mut app = fixture(1);
        let ctx = egui::Context::default();
        app.items[0] = GridItem::Audio(PathBuf::from(r"c:\pics\song.flac"));
        let id = app.requested.id(0).unwrap();
        let payload = if canceled {
            ThumbMsgPayload::Canceled
        } else {
            ThumbMsgPayload::Failed {
                origin: ThumbLoadOrigin::SourceIntrinsic,
            }
        };
        app.tx
            .send(notification(0, app.items_generation, id, payload))
            .unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(app.requested.is_empty());
        assert_eq!(
            matches!(app.thumbnails[0], ThumbnailState::Evicted),
            canceled
        );
        app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
        assert_eq!(app.requested.contains_key(&0), canceled);
        if !canceled {
            assert_worker_queues_empty(&app);
            assert_eq!(
                app.invalidate_audio_art_terminals_after_catalog_maintenance(None),
                1
            );
            app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
            assert!(app.requested.contains_key(&0));
        }
        assert_one_audio_request_on_heavy_queue(&app);
    }
}

#[test]
fn lifecycle_edit_preview_epoch_cleanup_preserves_new_owner() {
    let mut owner = ThumbnailRequests::default();
    let mut thumbnails = vec![ThumbnailState::Pending];
    let old = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, Some(3))
        .unwrap();
    assert!(owner.owner_matches_edit_preview_epoch(0, 3));
    cleanup_stale_edit_preview_request(&mut owner, &mut thumbnails, 0, 3);
    assert!(!owner.contains_key(&0));
    let new = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, Some(4))
        .unwrap();
    assert_ne!(old, new);
    cleanup_stale_edit_preview_request(&mut owner, &mut thumbnails, 0, 3);
    assert_eq!(owner.id(0), Some(new));
    assert!(owner.owner_matches_edit_preview_epoch(0, 4));
    assert!(matches!(
        owner.receive(owner_pixels(0, 7, old)),
        ThumbnailReceipt::Ignored
    ));
    let mut accepted = owner_pixels(0, 7, new);
    let ThumbMsgPayload::SourcePixels(mut pixels) = accepted.payload else {
        panic!("source")
    };
    pixels.origin = ThumbLoadOrigin::EditPreviewCache { epoch: 4 };
    accepted.payload = ThumbMsgPayload::Pixels(pixels);
    owner.receive(accepted);
    settle_next(&mut owner);
    assert!(owner.is_empty());
}

#[test]
fn lifecycle_one_hide_then_unchanged_hidden_demand_converges_without_duplicate_work() {
    for hide_during_load in [false, true] {
        let mut app = fixture(25);
        let ctx = egui::Context::default();
        app.main_hwnd = None;
        for idx in [0, 2, 4] {
            app.thumbnails[idx] = ThumbnailState::Loaded {
                tex: ctx.load_texture(
                    format!("old-upgrade-{idx}"),
                    egui::ColorImage::filled([2, 2], egui::Color32::GRAY),
                    egui::TextureOptions::LINEAR,
                ),
                origin: ThumbLoadOrigin::UpgradeableCache,
                from_edit_preview: false,
                rendered_at_px: 2,
                source_dims: Some((10, 20)),
                layout_dims: None,
            };
            app.requested.remove(&idx);
            let generation = app.items_generation;
            app.requested
                .begin(idx, generation, ThumbnailRequestKind::Upgrade, None)
                .unwrap();
        }
        let initial_ids: Vec<_> = (0..25).map(|idx| app.requested.id(idx).unwrap()).collect();
        for idx in 0..25 {
            app.tx.send(source_pixels(&app, idx)).unwrap();
            if !hide_during_load {
                app.tx.send(finalized(&app, idx)).unwrap();
            }
        }
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert_eq!(app.requested.images_len(), 17);
        if !hide_during_load {
            for _ in 0..3 {
                app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            }
            assert!(app.requested.is_empty());
        }
        app.window_visible = true;
        app.hide_to_tray(&ctx);
        assert_eq!(app.requested.images_len(), 0);
        if hide_during_load {
            assert_eq!(app.requested.len(), 25);
            for (idx, id) in initial_ids.into_iter().enumerate() {
                app.tx
                    .send(notification(
                        idx,
                        app.items_generation,
                        id,
                        ThumbMsgPayload::Finalized,
                    ))
                    .unwrap();
            }
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert!(app.requested.is_empty());
        }
        let mut work = Vec::new();
        for _ in 0..6 {
            app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
            let mut started = Vec::new();
            for queue in [&app.reload_queue, &app.heavy_io_queue]
                .into_iter()
                .flatten()
            {
                started.extend(
                    queue
                        .0
                        .lock()
                        .unwrap()
                        .drain(..)
                        .map(|r| (r.idx, r.request_id)),
                );
            }
            for (idx, id) in started {
                assert!(
                    !work.contains(&idx),
                    "unchanged hidden demand must not repeat decode idx={idx}"
                );
                work.push(idx);
                assert_eq!(app.requested.id(idx), Some(id));
                app.tx.send(source_pixels(&app, idx)).unwrap();
                app.tx.send(finalized(&app, idx)).unwrap();
            }
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        }
        assert_eq!(work.len(), 25);
        assert!(app.requested.is_empty());
        assert_eq!(app.requested.images_len(), 0);
        assert!(
            app.thumbnails
                .iter()
                .all(|t| matches!(t, ThumbnailState::Loaded { .. }))
        );
        assert!(!app.window_visible);
        assert_worker_queues_empty(&app);
    }
}

#[test]
#[cfg(windows)]
fn lifecycle_same_generation_wrong_context_notifications_cannot_change_sibling() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    let generation = app.items_generation;
    let main_id = app.requested.id(0).unwrap();
    let sibling = app.build_window_context_for_test(1361, |mounted| {
        mounted.items_generation = generation;
        mounted.items = vec![GridItem::ZipFile(PathBuf::from(
            r"c:\sibling\representative.zip",
        ))];
        mounted.image_metas = vec![Some((10, 20))];
        mounted.thumbnails = vec![ThumbnailState::Pending];
        mounted.keep_set = HashSet::from([0]);
        mounted.keep_range = (0, 1);
        mounted
            .requested
            .begin(0, generation, ThumbnailRequestKind::Normal, None)
            .unwrap();
        mounted.tx.send(source_pixels(mounted, 0)).unwrap();
        mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    });
    app.with_viewer_context(sibling, |mounted| {
        let sibling_id = mounted.requested.id(0).unwrap();
        assert_ne!(main_id, sibling_id);
        let snapshot = mounted.requested.snapshot();
        let metas = mounted.image_metas.clone();
        let revisions = mounted
            .color_filter
            .palettes
            .representative_revisions
            .clone();
        let texture = match &mounted.thumbnails[0] {
            ThumbnailState::Loaded { tex, .. } => tex.id(),
            _ => panic!("adopted sibling source"),
        };
        for terminal in 0..6 {
            let msg = match terminal {
                0 => owner_pixels(0, generation, main_id),
                1 => notification(0, generation, main_id, ThumbMsgPayload::Finalized),
                2 => notification(0, generation, main_id, ThumbMsgPayload::Canceled),
                3 => notification(
                    0,
                    generation,
                    main_id,
                    ThumbMsgPayload::Failed {
                        origin: ThumbLoadOrigin::SourceIntrinsic,
                    },
                ),
                4 => notification(0, generation, main_id, ThumbMsgPayload::NoArt),
                _ => notification(0, generation, main_id, ThumbMsgPayload::CacheOnlyMiss),
            };
            mounted.tx.send(msg).unwrap();
            mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert_eq!(mounted.requested.snapshot(), snapshot);
            assert_eq!(mounted.requested.images_len(), 0);
            assert_eq!(mounted.image_metas, metas);
            assert_eq!(
                mounted.color_filter.palettes.representative_revisions,
                revisions
            );
            match &mounted.thumbnails[0] {
                ThumbnailState::Loaded {
                    tex,
                    origin,
                    source_dims,
                    ..
                } => {
                    assert_eq!(tex.id(), texture);
                    assert_eq!(
                        *origin,
                        ThumbLoadOrigin::SourceGenerated {
                            evaluated_display_px: 374
                        }
                    );
                    assert_eq!(*source_dims, Some((884, 444)));
                }
                _ => panic!("wrong-context terminal cannot modify sibling display"),
            }
        }
    })
    .unwrap();
    assert_eq!(app.requested.id(0), Some(main_id));
}

#[test]
fn lifecycle_details_keep_prune_preserves_ninth_video_for_consumption_policy() {
    for kind in [
        ThumbnailRequestKind::VideoBatch,
        ThumbnailRequestKind::DirectPixels,
    ] {
        for consumption in [
            ThumbnailConsumptionPolicy::Grid,
            ThumbnailConsumptionPolicy::PassthroughRendition,
        ] {
            let mut app = fixture(9);
            let ctx = egui::Context::default();
            app.requested.clear();
            app.items = (0..9)
                .map(|idx| GridItem::Video(PathBuf::from(format!(r"c:\clips\{idx}.mp4"))))
                .collect();
            let generation = app.items_generation;
            for idx in 0..9 {
                app.requested.begin(idx, generation, kind, None).unwrap();
                app.tx
                    .send(pixels(&app, idx, ThumbLoadOrigin::SourceIntrinsic))
                    .unwrap();
            }
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert_eq!(app.requested.images_len(), 1);
            let ninth_id = app.requested.id(8).unwrap();
            assert!(matches!(app.thumbnails[8], ThumbnailState::Pending));
            app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
            app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), true);
            assert_eq!(
                app.requested.id(8),
                Some(ninth_id),
                "keep pruning cannot retire a video with no spawn-again path"
            );
            assert_eq!(
                app.requested.images_len(),
                1,
                "the ninth video image must survive until the consumer chooses its outcome"
            );
            assert_eq!(
                repaint_projection(&app.requested, &app.keep_set),
                (false, true, false)
            );
            assert_eq!(app.requested.legacy_len(), 0);
            assert_eq!(app.requested.progress_counts(&app.keep_set), (0, 0, 0));
            app.update_progress_peaks();
            assert_eq!(app.progress_snapshot(), ((0, 0), (0, 0)));
            assert_eq!(app.cache_gen_total, 0);
            assert_eq!(app.cache_gen_done.load(Ordering::Relaxed), 0);
            app.poll_thumbnails(&ctx, consumption);
            assert_eq!(
                matches!(app.thumbnails[8], ThumbnailState::Loaded { .. }),
                consumption.keeps_out_of_range_video_texture()
            );
            assert!(app.requested.is_empty());
            assert_eq!(
                repaint_projection(&app.requested, &app.keep_set),
                (false, false, false)
            );
            assert_eq!(app.progress_snapshot(), ((0, 0), (0, 0)));
        }
    }
}

#[test]
fn lifecycle_keep_prune_leaves_nonvideo_direct_image_for_poll_discard() {
    for consumption in [
        ThumbnailConsumptionPolicy::Grid,
        ThumbnailConsumptionPolicy::PassthroughRendition,
    ] {
        let mut app = fixture(9);
        let ctx = egui::Context::default();
        app.requested.clear();
        let generation = app.items_generation;
        for idx in 0..9 {
            app.requested
                .begin(idx, generation, ThumbnailRequestKind::DirectPixels, None)
                .unwrap();
            app.tx
                .send(pixels(&app, idx, ThumbLoadOrigin::FinalCache))
                .unwrap();
        }
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert_eq!(app.requested.images_len(), 1);
        let ninth_id = app.requested.id(8).unwrap();
        app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), true);
        assert_eq!(app.requested.id(8), Some(ninth_id));
        assert_eq!(
            app.requested.images_len(),
            1,
            "keep pruning cannot decide the outcome of a direct image"
        );
        assert_eq!(
            repaint_projection(&app.requested, &app.keep_set),
            (false, true, false)
        );
        assert_eq!(app.requested.progress_counts(&app.keep_set), (0, 0, 0));
        assert_eq!(app.image_metas[8], Some((10, 20)));
        app.poll_thumbnails(&ctx, consumption);
        assert!(
            matches!(app.thumbnails[8], ThumbnailState::Evicted),
            "both consumers discard out-of-keep nonvideo direct images"
        );
        assert_eq!(
            app.image_metas[8],
            Some((10, 20)),
            "discarded direct pixels cannot publish adopted dimensions"
        );
        assert!(app.requested.is_empty());
        assert_eq!(
            repaint_projection(&app.requested, &app.keep_set),
            (false, false, false)
        );
        app.update_progress_peaks();
        assert_eq!(app.progress_snapshot(), ((0, 0), (0, 0)));
        assert_eq!(app.cache_gen_total, 0);
        assert_eq!(app.cache_gen_done.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn lifecycle_real_generation_setter_retires_each_old_phase_but_same_generation_preserves_it() {
    for phase in 0..3 {
        let mut app = fixture(1);
        let ctx = egui::Context::default();
        let generation = app.items_generation;
        let old_id = app.requested.id(0).unwrap();
        if phase > 0 {
            let msg = source_pixels(&app, 0);
            app.requested.receive(msg);
        }
        if phase > 1 {
            settle_next(&mut app.requested);
        }
        let unchanged = app.requested.snapshot();
        let images = app.requested.images_len();
        app.set_items_generation(generation);
        assert_eq!(app.requested.snapshot(), unchanged);
        assert_eq!(app.requested.images_len(), images);
        assert!(
            app.requested
                .begin(0, generation, ThumbnailRequestKind::Normal, None)
                .is_none()
        );
        // A keep departure must not serve as a namespace retirement.
        app.install_thumbnail_keep_projection(ThumbnailKeepProjection::default(), true);
        assert_eq!(app.requested.id(0), Some(old_id));
        app.set_items_generation(generation.wrapping_add(1));
        assert!(
            app.requested.is_empty(),
            "real generation change retires old phase {phase}"
        );
        assert_eq!(app.requested.images_len(), 0);
        let current = app.items_generation;
        let new_id = app
            .requested
            .begin(0, current, ThumbnailRequestKind::Normal, None)
            .unwrap();
        let before_stale = app.requested.snapshot();
        app.tx.send(owner_pixels(0, generation, old_id)).unwrap();
        app.tx
            .send(notification(
                0,
                generation,
                old_id,
                ThumbMsgPayload::Finalized,
            ))
            .unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert_eq!(app.requested.snapshot(), before_stale);
        assert_eq!(app.requested.id(0), Some(new_id));
        assert_eq!(app.requested.images_len(), 0);
        app.keep_set.insert(0);
        app.tx.send(source_pixels(&app, 0)).unwrap();
        app.tx.send(finalized(&app, 0)).unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(app.requested.is_empty());
        assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    }
}

#[test]
#[cfg(windows)]
fn lifecycle_at_rest_generation_setter_retires_only_owner_and_preserves_sibling() {
    for phase in 0..3 {
        let mut app = fixture(1);
        let ctx = egui::Context::default();
        let generation = app.items_generation;
        let main_snapshot = app.requested.snapshot();
        let main_token = app.cancel_token.clone();
        let main_keep = app.keep_set.clone();
        let sibling = app.build_window_context_for_test(1362, |mounted| {
            mounted.items_generation = generation;
            mounted.items = vec![GridItem::Image(PathBuf::from(r"c:\sibling\0.png"))];
            mounted.image_metas = vec![Some((10, 20))];
            mounted.thumbnails = vec![ThumbnailState::Pending];
            mounted.keep_set = HashSet::from([0]);
            mounted.keep_range = (0, 1);
            let id = mounted
                .requested
                .begin(0, generation, ThumbnailRequestKind::Normal, None)
                .unwrap();
            if phase > 0 {
                mounted.requested.receive(owner_pixels(0, generation, id));
            }
            if phase > 1 {
                settle_next(&mut mounted.requested);
            }
        });
        let (old_id, owner_snapshot, image_count) = app
            .with_viewer_context(sibling, |mounted| {
                (
                    mounted.requested.id(0).unwrap(),
                    mounted.requested.snapshot(),
                    mounted.requested.images_len(),
                )
            })
            .unwrap();
        app.set_viewer_context_generation_for_test(sibling, generation);
        app.with_viewer_context(sibling, |mounted| {
            assert_eq!(mounted.requested.snapshot(), owner_snapshot);
            assert_eq!(mounted.requested.images_len(), image_count);
            assert!(
                mounted
                    .requested
                    .begin(0, generation, ThumbnailRequestKind::Normal, None)
                    .is_none()
            );
        })
        .unwrap();
        app.set_viewer_context_generation_for_test(sibling, generation.wrapping_add(1));
        app.with_viewer_context(sibling, |mounted| {
            assert!(mounted.requested.is_empty());
            assert_eq!(mounted.requested.images_len(), 0);
            let current = mounted.items_generation;
            let new_id = mounted
                .requested
                .begin(0, current, ThumbnailRequestKind::Normal, None)
                .unwrap();
            let new_snapshot = mounted.requested.snapshot();
            mounted
                .tx
                .send(owner_pixels(0, generation, old_id))
                .unwrap();
            mounted
                .tx
                .send(notification(
                    0,
                    generation,
                    old_id,
                    ThumbMsgPayload::Finalized,
                ))
                .unwrap();
            mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert_eq!(mounted.requested.snapshot(), new_snapshot);
            assert_eq!(mounted.requested.id(0), Some(new_id));
            assert!(matches!(mounted.thumbnails[0], ThumbnailState::Pending));
            mounted.tx.send(source_pixels(mounted, 0)).unwrap();
            mounted.tx.send(finalized(mounted, 0)).unwrap();
            mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert!(mounted.requested.is_empty());
            assert!(matches!(
                mounted.thumbnails[0],
                ThumbnailState::Loaded { .. }
            ));
        })
        .unwrap();
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.requested.snapshot(), main_snapshot);
        assert!(Arc::ptr_eq(&app.cancel_token, &main_token));
        assert!(!main_token.load(Ordering::Relaxed));
        assert_eq!(app.keep_set, main_keep);
    }
}

#[test]
fn lifecycle_cache_only_miss_keeps_admission_without_activity_or_correlation() {
    let mut owner = ThumbnailRequests::default();
    let id = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    // An image-less terminal also disposes any provisional image still owned by this request.
    owner.receive(owner_seed(0, 7, id));
    assert!(matches!(
        owner.receive(notification(0, 7, id, ThumbMsgPayload::CacheOnlyMiss)),
        ThumbnailReceipt::Terminal(ThumbMsgPayload::CacheOnlyMiss)
    ));
    assert_eq!(owner.id(0), None);
    assert_eq!(owner.legacy_len(), 0);
    assert_eq!(owner.iter().count(), 0);
    assert_eq!(owner.images_len(), 0);
    for keep in [HashSet::new(), HashSet::from([0])] {
        assert_eq!(repaint_projection(&owner, &keep), (false, false, false));
        assert_eq!(owner.progress_counts(&keep), (0, 0, 0));
        owner.discard_images_outside(&keep);
        owner.discard_images();
        owner.cancel_queued(0, id);
        assert!(
            owner
                .begin(0, 7, ThumbnailRequestKind::Normal, None)
                .is_none()
        );
    }
    for payload in [
        ThumbMsgPayload::CacheOnlyMiss,
        ThumbMsgPayload::Finalized,
        ThumbMsgPayload::Canceled,
        ThumbMsgPayload::NoArt,
    ] {
        assert!(matches!(
            owner.receive(notification(0, 7, id, payload)),
            ThumbnailReceipt::Ignored
        ));
        assert!(owner.contains_key(&0));
    }
    owner.remove(&0); // judgement/source retirement consumes the demand gate
    let next = owner
        .begin(0, 7, ThumbnailRequestKind::Normal, None)
        .unwrap();
    assert_ne!(next, id);
    assert!(matches!(
        owner.receive(notification(0, 7, id, ThumbMsgPayload::CacheOnlyMiss)),
        ThumbnailReceipt::Ignored
    ));
    assert_eq!(owner.id(0), Some(next));
    assert_eq!(owner.legacy_len(), 1);
    owner.clear();
    assert!(owner.is_empty());
    assert_eq!(owner.legacy_len(), 0);
}

#[test]
fn lifecycle_cache_only_miss_outside_keep_allows_other_image_upgrade() {
    let mut app = fixture(2);
    let ctx = egui::Context::default();
    app.items[1] = GridItem::ConvertibleArchive {
        path: PathBuf::from(r"c:\pics\undecided.rar"),
        format: crate::archive_converter::ArchiveFormat::Rar,
    };
    let waiting = app.requested.id(1).unwrap();
    app.tx
        .send(pixels(&app, 0, ThumbLoadOrigin::UpgradeableCache))
        .unwrap();
    app.tx
        .send(notification(
            1,
            app.items_generation,
            waiting,
            ThumbMsgPayload::CacheOnlyMiss,
        ))
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    app.install_thumbnail_keep_projection(
        ThumbnailKeepProjection {
            pages: HashSet::from([0]),
            bounded_range: (0, 1),
            interactive_pages: HashSet::new(),
        },
        true,
    );
    assert!(!app.requested.blocks_idle_upgrade());
    app.enqueue_idle_upgrades();
    assert_eq!(app.requested.get(&0), Some(&true));
    assert_eq!(
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().len(),
        1
    );
    assert_eq!(app.requested.id(1), None);
    assert!(app.requested.contains_key(&1));
}

#[test]
#[cfg(windows)]
fn lifecycle_cache_only_miss_hide_and_context_retirement_are_local() {
    let mut app = fixture(1);
    let ctx = egui::Context::default();
    let generation = app.items_generation;
    let old_id = app.requested.id(0).unwrap();
    app.tx
        .send(notification(
            0,
            generation,
            old_id,
            ThumbMsgPayload::CacheOnlyMiss,
        ))
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    let sibling = app.build_window_context_for_test(1363, |mounted| {
        mounted.items_generation = generation;
        mounted.items = vec![GridItem::Image(PathBuf::from(r"c:\sibling\0.png"))];
        mounted.thumbnails = vec![ThumbnailState::Pending];
        let id = mounted
            .requested
            .begin(0, generation, ThumbnailRequestKind::Normal, None)
            .unwrap();
        mounted.requested.receive(notification(
            0,
            generation,
            id,
            ThumbMsgPayload::CacheOnlyMiss,
        ));
    });
    app.main_hwnd = None;
    app.window_visible = true;
    app.hide_to_tray(&ctx);
    app.sync_after_restore(&ctx);
    app.set_items_generation(generation);
    assert!(app.requested.contains_key(&0));
    assert_eq!(app.requested.id(0), None);
    assert_eq!(app.requested.legacy_len(), 0);
    app.set_viewer_context_generation_for_test(sibling, generation);
    app.with_viewer_context(sibling, |mounted| {
        assert!(mounted.requested.contains_key(&0));
        assert_eq!(mounted.requested.id(0), None);
    })
    .unwrap();
    app.set_viewer_context_generation_for_test(sibling, generation.wrapping_add(1));
    app.with_viewer_context(sibling, |mounted| assert!(mounted.requested.is_empty()))
        .unwrap();
    assert!(
        app.requested.contains_key(&0),
        "retiring sibling must preserve main demand"
    );
    let sibling_id = app
        .with_viewer_context(sibling, |mounted| {
            mounted
                .requested
                .begin(
                    0,
                    mounted.items_generation,
                    ThumbnailRequestKind::Normal,
                    None,
                )
                .unwrap()
        })
        .unwrap();
    app.set_items_generation(generation.wrapping_add(1));
    assert!(app.requested.is_empty());
    app.tx
        .send(notification(
            0,
            generation,
            old_id,
            ThumbMsgPayload::CacheOnlyMiss,
        ))
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.requested.is_empty());
    app.with_viewer_context(sibling, |mounted| {
        assert_eq!(mounted.requested.id(0), Some(sibling_id))
    })
    .unwrap();
}
