use super::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

fn setup_thumbnail_app(count: usize) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.items = (0..count)
        .map(|idx| {
            if idx % 2 == 0 {
                GridItem::Image(PathBuf::from(format!("C:/section-1200/{idx}.png")))
            } else {
                GridItem::ZipImage {
                    zip_path: PathBuf::from("C:/section-1200/book.zip"),
                    entry_name: format!("pages/{idx}.png"),
                }
            }
        })
        .collect();
    app.image_metas = vec![Some((1, 1)); count];
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.visible_indices = (0..count).collect();
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app
}

fn loaded_thumbnail(ctx: &egui::Context, name: &str) -> ThumbnailState {
    ThumbnailState::Loaded {
        tex: ctx.load_texture(
            name,
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ),
        origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 2,
        },
        from_edit_preview: false,
        rendered_at_px: 2,
        source_dims: Some((2, 2)),
        layout_dims: None,
    }
}

fn cached_all_items_are_images(app: &mut App) -> bool {
    let items_generation = app.items_generation;
    let reading_flow = app.reading_flow;
    let items = app.items.clone();
    app.viewer_navigation_caches
        .all_items_are_images(items_generation, reading_flow, &items)
}

fn navigation_sequence(items_generation: u64, pages: Vec<usize>) -> FsHoldover {
    let anchor_idx = *pages.first().expect("test target has an anchor");
    FsHoldover::NavigationSequence(FsNavigationSequence {
        previous: None,
        chrome: FsNavigationChromeContinuation::None,
        purpose: FsNavigationPurpose::Ordinary,
        opened_at: std::time::Instant::now(),
        target: FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation,
            anchor_idx,
            accept_rendition: true,
            phase: FsNavigationTargetPhase::awaiting_navigation(anchor_idx, pages),
        }),
    })
}

#[test]
fn keep_projection_unions_owners_but_keeps_sparse_still_pages_out_of_worker_bbox() {
    let projection = thumbnail_keep_projection(8, [0, 1], [3], Some(5), [6], [7, 20]);

    assert_eq!(projection.pages, HashSet::from([0, 1, 3, 5, 6, 7]));
    assert_eq!(projection.bounded_range, (0, 7));
    assert_eq!(projection.interactive_pages, HashSet::from([3, 5, 6, 7]));
    assert!(
        !projection.pages.contains(&20),
        "invalid owners must be filtered"
    );

    let still_only = thumbnail_keep_projection(8, [], [], None, [], [7]);
    assert_eq!(still_only.pages, HashSet::from([7]));
    assert_eq!(still_only.bounded_range, (0, 0));
}

#[test]
fn materialized_image_classification_memo_follows_generation_and_context_owner() {
    let mut app = setup_thumbnail_app(2);
    app.items = vec![
        GridItem::Image(PathBuf::from("C:/section-1200/images/0.png")),
        GridItem::Image(PathBuf::from("C:/section-1200/images/1.png")),
    ];
    let generation = app.items_generation;
    assert_eq!(generation, app.items_generation);
    assert!(cached_all_items_are_images(&mut app));

    app.items.push(GridItem::Video(PathBuf::from(
        "C:/section-1200/images/2.mp4",
    )));
    app.items_generation = app.items_generation.wrapping_add(1);
    assert!(!cached_all_items_are_images(&mut app));

    app.items.pop();
    app.items_generation = app.items_generation.wrapping_add(1);
    assert!(cached_all_items_are_images(&mut app));

    #[cfg(windows)]
    {
        let main_generation = app.items_generation;
        app.build_active_context_for_test(None, DetachedSource::Image, |detached| {
            detached.items_generation = main_generation;
            detached.items = vec![
                GridItem::Image(PathBuf::from("C:/detached/0.png")),
                GridItem::Video(PathBuf::from("C:/detached/1.mp4")),
            ];
            assert!(!cached_all_items_are_images(detached));
        });
        assert!(cached_all_items_are_images(&mut app));
    }
}

#[test]
fn bookmark_panel_reachability_matches_fullscreen_priority_branches() {
    let mut app = setup_thumbnail_app(3);
    app.fullscreen_idx = Some(1);
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;

    assert!(app.fullscreen_adjustment_panel_draw_reachable(1, false, false));

    app.analysis_mode = true;
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    assert!(
        app.fullscreen_adjustment_panel_draw_reachable(1, false, true),
        "double spread suppresses the single-page analysis branch"
    );
    app.analysis_mode = false;

    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, true, false));
    app.local_adjust_mode = true;
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    app.local_adjust_mode = false;

    app.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
        crate::sns_split::SnsTarget::X,
        3,
        [832, 1216],
    ));
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    app.sns_split = None;

    app.export_crop_mode = true;
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    app.export_crop_mode = false;

    app.view_trim_mode = true;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    app.reading_flow = crate::settings::ReadingFlow::Vertical;
    assert!(app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
    app.view_trim_mode = false;

    app.items[1] = GridItem::Audio(PathBuf::from("C:/section-1200/audio.flac"));
    assert!(!app.fullscreen_adjustment_panel_draw_reachable(1, false, false));
}

#[test]
fn details_hover_seek_and_navigation_release_only_their_own_thumbnail_pages() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(6);
    app.fs_holdover_tex = Some(navigation_sequence(app.items_generation, vec![2]));
    app.ensure_still_seek_thumbnail_requests(&ctx, &[4]);
    app.set_details_hover_thumbnail_idx(Some(4));
    app.thumbnails[2] = loaded_thumbnail(&ctx, "section_1200_nav");
    app.thumbnails[4] = loaded_thumbnail(&ctx, "section_1200_shared_hover_seek");

    assert_eq!(app.keep_set, HashSet::from([2, 4]));
    assert_eq!(app.keep_range, (2, 5));

    app.set_details_hover_thumbnail_idx(None);
    assert_eq!(app.keep_set, HashSet::from([2, 4]));
    assert_eq!(app.keep_range, (2, 3), "still seek stays exact-only");
    assert!(matches!(app.thumbnails[4], ThumbnailState::Loaded { .. }));

    app.set_details_hover_thumbnail_idx(Some(1));
    app.thumbnails[1] = loaded_thumbnail(&ctx, "section_1200_hover_only");
    assert_eq!(app.keep_set, HashSet::from([1, 2, 4]));
    app.set_details_hover_thumbnail_idx(None);
    assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));
    assert!(matches!(app.thumbnails[4], ThumbnailState::Loaded { .. }));

    app.clear_still_seek_thumbnail_requests();
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([2]));
    assert!(matches!(app.thumbnails[4], ThumbnailState::Evicted));

    app.fs_holdover_tex = Some(navigation_sequence(
        app.items_generation.wrapping_add(1),
        vec![2],
    ));
    app.reconcile_details_thumbnail_keep_owners();
    assert!(
        app.keep_set.is_empty(),
        "a stale generation must own no page"
    );
    assert!(matches!(app.thumbnails[2], ThumbnailState::Evicted));
}

#[test]
fn last_owner_prunes_queued_material_but_keeps_popped_request_until_worker_result() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(4);
    app.keep_set = HashSet::from([0, 1, 2, 3]);
    app.thumbnail_eviction_generation = Some(app.items_generation);
    app.requested.extend((0..4).map(|idx| (idx, false)));
    app.reload_queue
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .push(LoadRequest {
            idx: 0,
            ..Default::default()
        });
    let input_seq = app.input_seq;
    let items_generation = app.items_generation;
    app.texture_backlog.push(crate::thumb_loader::ThumbMsg {
        idx: 2,
        image: Some(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
        origin: crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
        from_edit_preview: false,
        edit_preview_adjustment: None,
        source_dims: Some((1, 1)),
        layout_dims: None,
        canceled: false,
        finalized: false,
        input_seq,
        items_gen: items_generation,
    });
    app.pending_finalize.insert(3);

    let item_count = app.items.len();
    let projection = thumbnail_keep_projection(item_count, [], [], None, [], []);
    app.install_thumbnail_keep_projection(projection, true);

    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.requested.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([1])
    );
    assert!(app.texture_backlog.is_empty());
    assert!(app.pending_finalize.is_empty());

    app.ensure_still_seek_thumbnail_requests(&ctx, &[1]);
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty(),
        "reacquiring an in-flight page must not queue a duplicate decode"
    );

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 1,
            image: None,
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: None,
            layout_dims: None,
            canceled: true,
            finalized: false,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert!(!app.requested.contains_key(&1));
    assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));

    app.ensure_still_seek_thumbnail_requests(&ctx, &[1]);
    assert!(app.requested.contains_key(&1));
    assert_eq!(
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().len(),
        1,
        "the canceled in-flight request must become retryable"
    );
}

#[test]
fn finalized_first_and_error_results_preserve_existing_worker_lifecycle() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(2);
    app.keep_set = HashSet::from([0, 1]);
    app.thumbnail_eviction_generation = Some(app.items_generation);
    app.requested.extend([(0, false), (1, false)]);

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 0,
            image: None,
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: None,
            layout_dims: None,
            canceled: false,
            finalized: true,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert!(app.requested.contains_key(&0));
    assert!(app.pending_finalize.contains(&0));

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 0,
            image: Some(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: Some((1, 1)),
            layout_dims: None,
            canceled: false,
            finalized: false,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 1,
            image: None,
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: None,
            layout_dims: None,
            canceled: false,
            finalized: false,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);

    assert!(!app.requested.contains_key(&0));
    assert!(!app.pending_finalize.contains(&0));
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert!(!app.requested.contains_key(&1));
    assert!(matches!(app.thumbnails[1], ThumbnailState::Failed));
}

fn book_bookmark_for_item(app: &App, idx: usize, id: i64) -> crate::book_bookmarks::BookBookmark {
    let draft = app
        .current_book_bookmark_draft(idx)
        .expect("test item supports a stable book bookmark identity");
    crate::book_bookmarks::BookBookmark {
        id,
        container_key: crate::book_bookmarks::container_key(&draft.container_path),
        container_path: draft.container_path,
        container_kind: draft.container_kind,
        page_identity: draft.page_identity,
        page_index_hint: draft.page_index_hint,
        created_at_ms: id,
        title: None,
    }
}

fn show_bookmark_panel(
    app: &mut App,
    fullscreen_idx: usize,
    bookmarks: Vec<crate::book_bookmarks::BookBookmark>,
) {
    let container_key = crate::book_bookmarks::container_key(
        &app.current_book_bookmark_draft(fullscreen_idx)
            .expect("fullscreen page belongs to a book")
            .container_path,
    );
    app.fullscreen_idx = Some(fullscreen_idx);
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    app.settings.fullscreen_left_panel_tab = crate::settings::FullscreenLeftPanelTab::Bookmarks;
    app.current_book_bookmarks_key = Some(container_key);
    app.current_book_bookmarks = bookmarks;
}

#[test]
fn bookmark_panel_owner_survives_poll_then_projection_without_a_second_decode() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(6);
    let bookmark = book_bookmark_for_item(&app, 3, 30);
    show_bookmark_panel(&mut app, 1, vec![bookmark]);
    app.ensure_bookmark_panel_thumbnails(&[3]);
    assert!(app.requested.contains_key(&3));
    app.reload_queue.as_ref().unwrap().0.lock().unwrap().clear();

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 3,
            image: Some(egui::ColorImage::filled([2, 2], egui::Color32::WHITE)),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: Some((2, 2)),
            layout_dims: None,
            canceled: false,
            finalized: false,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());

    assert!(matches!(app.thumbnails[3], ThumbnailState::Loaded { .. }));
    assert_eq!(app.keep_set, HashSet::from([3]));
    assert!(
        app.requested.contains_key(&3),
        "source decode stays in flight until its cache-finalized signal"
    );
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 3,
            image: None,
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: Some((2, 2)),
            layout_dims: None,
            canceled: false,
            finalized: true,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());

    assert!(!app.requested.contains_key(&3));
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty(),
        "finalization must not cause a second decode while the bookmark owner remains"
    );
}

#[test]
fn bookmark_navigation_hover_and_seek_share_union_bbox_but_release_independently() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(8);
    let bookmark = book_bookmark_for_item(&app, 7, 70);
    show_bookmark_panel(&mut app, 1, vec![bookmark]);
    app.fs_holdover_tex = Some(navigation_sequence(app.items_generation, vec![0]));
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_range, (0, 1));
    app.ensure_bookmark_panel_thumbnails(&[7]);
    assert_eq!(
        app.keep_range,
        (0, 8),
        "bookmark assertion must merge with the current navigation worker bbox"
    );
    app.ensure_navigation_target_thumbnail_requests(&ctx, &[0]);
    assert_eq!(
        app.keep_range,
        (0, 8),
        "navigation assertion must not transiently narrow the bookmark worker bbox"
    );
    assert_eq!(app.keep_start_shared.load(Ordering::Relaxed), 0);
    assert_eq!(app.keep_end_shared.load(Ordering::Relaxed), 8);
    app.ensure_still_seek_thumbnail_requests(&ctx, &[7]);
    app.set_details_hover_thumbnail_idx(Some(7));
    app.thumbnails[7] = loaded_thumbnail(&ctx, "section_1200_all_owners");

    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([0, 7]));
    assert_eq!(app.keep_range, (0, 8));

    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::Closed;
    app.set_details_hover_thumbnail_idx(None);
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([0, 7]));
    assert!(matches!(app.thumbnails[7], ThumbnailState::Loaded { .. }));

    app.clear_still_seek_thumbnail_requests();
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([0]));
    assert!(matches!(app.thumbnails[7], ThumbnailState::Evicted));
}

#[test]
fn bookmark_owner_expires_on_remove_container_mismatch_or_panel_close() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(6);
    let bookmark = book_bookmark_for_item(&app, 3, 30);
    show_bookmark_panel(&mut app, 1, vec![bookmark.clone()]);
    app.ensure_bookmark_panel_thumbnails(&[3]);
    app.thumbnails[3] = loaded_thumbnail(&ctx, "section_1200_bookmark_remove");
    app.reconcile_details_thumbnail_keep_owners();
    assert!(app.keep_set.contains(&3));

    app.settings.fullscreen_left_panel_tab = crate::settings::FullscreenLeftPanelTab::Adjustment;
    app.reconcile_details_thumbnail_keep_owners();
    assert!(matches!(app.thumbnails[3], ThumbnailState::Evicted));

    app.settings.fullscreen_left_panel_tab = crate::settings::FullscreenLeftPanelTab::Bookmarks;
    app.current_book_bookmarks = vec![bookmark.clone()];
    app.ensure_bookmark_panel_thumbnails(&[3]);
    app.current_book_bookmarks.clear();
    app.reconcile_details_thumbnail_keep_owners();
    assert!(
        !app.keep_set.contains(&3),
        "removed bookmark releases its owner"
    );

    app.current_book_bookmarks = vec![crate::book_bookmarks::BookBookmark {
        container_key: crate::book_bookmarks::container_key(
            PathBuf::from("C:/other/book.zip").as_path(),
        ),
        container_path: PathBuf::from("C:/other/book.zip"),
        ..bookmark.clone()
    }];
    app.ensure_bookmark_panel_thumbnails(&[3]);
    app.reconcile_details_thumbnail_keep_owners();
    assert!(
        !app.keep_set.contains(&3),
        "same page identity from another container cannot own this thumbnail"
    );

    app.current_book_bookmarks = vec![bookmark];
    app.ensure_bookmark_panel_thumbnails(&[3]);
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::Closed;
    app.reconcile_details_thumbnail_keep_owners();
    assert!(!app.keep_set.contains(&3));
}

#[test]
fn stale_generation_result_cannot_restore_a_released_bookmark_owner() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(4);
    let bookmark = book_bookmark_for_item(&app, 3, 30);
    show_bookmark_panel(&mut app, 1, vec![bookmark]);
    app.ensure_bookmark_panel_thumbnails(&[3]);
    let old_generation = app.items_generation;
    app.items_generation = app.items_generation.wrapping_add(1);
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::Closed;
    app.reconcile_details_thumbnail_keep_owners();

    app.tx
        .send(crate::thumb_loader::ThumbMsg {
            idx: 3,
            image: Some(egui::ColorImage::filled([2, 2], egui::Color32::WHITE)),
            origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            from_edit_preview: false,
            edit_preview_adjustment: None,
            source_dims: Some((2, 2)),
            layout_dims: None,
            canceled: false,
            finalized: false,
            input_seq: app.input_seq,
            items_gen: old_generation,
        })
        .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);

    assert!(!app.keep_set.contains(&3));
    assert!(!matches!(app.thumbnails[3], ThumbnailState::Loaded { .. }));
}

#[test]
fn switching_back_to_thumbnail_rebuilds_from_the_existing_capped_grid_slice() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(30);
    app.settings.grid_cols = 2;
    app.settings.thumb_prev_pages = 0;
    app.settings.thumb_next_pages = 0;
    app.last_cell_size = 100.0;
    app.last_cell_h = 100.0;
    app.last_viewport_h = 100.0;
    app.set_details_hover_thumbnail_idx(Some(20));
    let bookmark = book_bookmark_for_item(&app, 29, 290);
    show_bookmark_panel(&mut app, 1, vec![bookmark]);
    app.ensure_bookmark_panel_thumbnails(&[29]);
    assert_eq!(app.keep_set, HashSet::from([20, 29]));

    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    let grid_keep = app.keep_set.clone();
    assert!(!grid_keep.is_empty());
    assert!(!grid_keep.contains(&20));
    assert!(
        grid_keep.contains(&29),
        "the visible bookmark panel remains an owner in Thumbnail mode"
    );

    app.clear_details_hover_keep();
    assert_eq!(
        app.keep_set, grid_keep,
        "clearing stale hover must not erase grid ownership"
    );
}

#[test]
#[cfg(windows)]
fn detached_thumbnail_owners_do_not_mutate_the_main_context_projection() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(3);
    app.keep_set = HashSet::from([0]);
    let main_shared = Arc::clone(&app.still_seek_thumbnail_pages_shared);

    app.build_active_context_for_test(None, DetachedSource::Image, |detached| {
        detached.items = (0..3)
            .map(|idx| GridItem::Image(PathBuf::from(format!("C:/detached/{idx}.png"))))
            .collect();
        detached.image_metas = vec![Some((1, 1)); 3];
        detached.thumbnails = vec![ThumbnailState::Pending; 3];
        detached.visible_indices = (0..3).collect();
        detached.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        detached.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        detached.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        detached.ensure_still_seek_thumbnail_requests(&ctx, &[2]);
        assert_eq!(detached.keep_set, HashSet::from([2]));
        assert!(!Arc::ptr_eq(
            &main_shared,
            &detached.still_seek_thumbnail_pages_shared
        ));
    });

    assert_eq!(app.keep_set, HashSet::from([0]));
    assert!(app.still_seek_thumbnail_pages.is_empty());
    assert!(main_shared.read().unwrap().is_empty());
    app.with_active_viewer_context(|detached| {
        assert_eq!(detached.keep_set, HashSet::from([2]));
        assert_eq!(
            detached
                .still_seek_thumbnail_pages_shared
                .read()
                .unwrap()
                .clone(),
            HashSet::from([2])
        );
    })
    .expect("detached context remains available");
}

#[test]
#[cfg(windows)]
fn bookmark_owner_for_same_index_is_scoped_by_context_and_container() {
    let mut app = setup_thumbnail_app(3);
    let main_bookmark = book_bookmark_for_item(&app, 1, 10);
    show_bookmark_panel(&mut app, 1, vec![main_bookmark.clone()]);
    app.ensure_bookmark_panel_thumbnails(&[1]);
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([1]));

    app.build_active_context_for_test(None, DetachedSource::Image, |detached| {
        detached.items = (0..3)
            .map(|idx| GridItem::ZipImage {
                zip_path: PathBuf::from("C:/detached/book.zip"),
                entry_name: format!("pages/{idx}.png"),
            })
            .collect();
        detached.image_metas = vec![Some((1, 1)); 3];
        detached.thumbnails = vec![ThumbnailState::Pending; 3];
        detached.visible_indices = (0..3).collect();
        detached.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        detached.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        detached.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        let detached_bookmark = book_bookmark_for_item(detached, 1, 20);
        show_bookmark_panel(detached, 1, vec![detached_bookmark]);
        detached.ensure_bookmark_panel_thumbnails(&[1]);
        detached.reconcile_details_thumbnail_keep_owners();
        assert_eq!(detached.keep_set, HashSet::from([1]));
    });

    assert_eq!(app.keep_set, HashSet::from([1]));
    show_bookmark_panel(&mut app, 1, vec![main_bookmark]);
    app.reconcile_details_thumbnail_keep_owners();
    assert_eq!(app.keep_set, HashSet::from([1]));
    app.with_active_viewer_context(|detached| {
        assert_eq!(detached.keep_set, HashSet::from([1]));
        assert!(matches!(
            detached.items.get(1),
            Some(GridItem::ZipImage { zip_path, .. })
                if zip_path == &PathBuf::from("C:/detached/book.zip")
        ));
    })
    .expect("detached bookmark context remains available");
}
