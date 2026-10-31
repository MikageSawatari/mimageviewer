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
    ThumbnailState::loaded_raster(
        ctx.load_texture(
            name,
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ),
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 2,
        },
        false,
        2,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((2, 2)),
        None,
    )
}

fn main_grid_to_overview_app(kind: &str, ctx: &egui::Context) -> AppTestEnvForTest {
    let mut app = setup_thumbnail_app(40);
    app.items = (0..40)
        .map(|idx| match kind {
            "image" => GridItem::Image(PathBuf::from(format!("C:/overview/book/p{idx:03}.jpg"))),
            "zip" => GridItem::ZipImage {
                zip_path: PathBuf::from("C:/overview/book.zip"),
                entry_name: format!("p{idx:03}.jpg"),
            },
            "pdf" => GridItem::PdfPage {
                pdf_path: PathBuf::from("C:/overview/book.pdf"),
                page_num: idx as u32,
                content_type: None,
            },
            _ => unreachable!(),
        })
        .collect();
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.settings.grid_cols = 4;
    // The main grid painted twelve cells; source prefetch may already have loaded the rest.
    // Rotation hydration belongs to the painted cells, independent of source pixels.
    for idx in 0..12 {
        app.thumbnails[idx] = loaded_thumbnail(ctx, &format!("main-grid-{kind}-{idx}"));
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    install_overview_demand(&mut app);
    app.page_overview.close();
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    assert!(app.open_page_overview(
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200., 900.)),
        Default::default(),
    ));
    app
}

fn draw_overview_frame(app: &mut App, ctx: &egui::Context) {
    // Each test frame has one renderer pass, so a fast worker cannot be drained by an
    // implicit egui discard pass before the explicit completion-wait checkpoint.
    ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200., 900.),
            )),
            ..Default::default()
        },
        |root_ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root_ui, |ui| {
                    app.render_page_overview_surface(ui, ui.max_rect());
                });
        },
    )
    .drop_without_applying_deltas();
}

#[test]
fn overview_main_grid_prefetched_pixels_complete_rotation_without_seek_overlay() {
    let mut results = Vec::new();
    for kind in ["image", "zip", "pdf"] {
        let ctx = egui::Context::default();
        let mut app = main_grid_to_overview_app(kind, &ctx);
        for idx in 12..40 {
            app.thumbnails[idx] = loaded_thumbnail(&ctx, &format!("prefetched-{kind}-{idx}"));
        }
        draw_overview_frame(&mut app, &ctx);
        assert!(
            app.requested.is_empty(),
            "loaded pixels must not be redundantly decoded"
        );
        assert!(
            !app.page_overview_thumbnail_projection(&[12, 39])
                .contains_key(&12)
        );
        app.rotation_cache.wait_for_result_for_test();
        // This is the real overview frame path, never a manual rotation poll/strip draw.
        draw_overview_frame(&mut app, &ctx);
        let visible = app.page_overview_thumbnail_projection(&[12, 39]);
        results.push((kind, visible.contains_key(&12), visible.contains_key(&39)));
        assert!(app.requested.is_empty());
    }
    assert_eq!(
        results,
        vec![
            ("image", true, true),
            ("zip", true, true),
            ("pdf", true, true)
        ],
        "completed rotations must be drained by the real overview path"
    );
}

#[test]
fn overview_main_grid_partial_load_enqueues_later_pages_on_real_surface() {
    let ctx = egui::Context::default();
    for kind in ["image", "zip", "pdf"] {
        for state in [ThumbnailState::Pending, ThumbnailState::Evicted] {
            let mut app = main_grid_to_overview_app(kind, &ctx);
            app.thumbnails[12..].fill(state.clone());
            for _ in 0..40 {
                draw_overview_frame(&mut app, &ctx);
                app.update_thumbnail_frame_bookkeeping(&ctx, std::time::Instant::now());
                if (12..40).all(|idx| app.requested.contains_key(&idx)) {
                    break;
                }
            }
            assert!(
                (12..40).all(|idx| app.requested.contains_key(&idx)),
                "{kind}: pages 13-40 must be requested"
            );
            assert!((0..12).all(|idx| !app.requested.contains_key(&idx)));
            let queue = app.reload_queue.as_ref().unwrap().0.lock().unwrap();
            assert_eq!(queue.len(), 28, "{kind}: one reservation per later page");
            assert!(
                queue
                    .iter()
                    .all(|request| request.priority && app.keep_set.contains(&request.idx))
            );
        }
    }
}

fn install_overview_demand(app: &mut App) {
    use crate::page_overview::{
        PageOverviewDirection, PageOverviewProjection, PageOverviewStamp, PageOverviewState,
    };
    app.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &app.items,
        None,
        app.settings.file_type_visibility.clone(),
    ));
    let projection = Arc::new(
        PageOverviewProjection::new(
            Arc::clone(app.page_identity_index.overview_metadata()),
            Arc::new((0..app.items.len()).collect()),
        )
        .unwrap(),
    );
    app.fullscreen_idx = Some(0);
    app.page_overview = PageOverviewState::open(
        projection,
        PageOverviewStamp {
            context_serial: app.projected_viewer_context_id().serial(),
            items_generation: app.items_generation,
            reader_order_revision: 1,
            incarnation: 1,
        },
        0,
        4,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 600.)),
        PageOverviewDirection::LeftToRight,
    )
    .unwrap();
}

fn prepare_pdf_overview(app: &mut App, path: &std::path::Path, count: usize) {
    // Only admission is exercised: no worker, PDFium, or physical PDF is needed.
    app.items = (0..count)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: path.to_path_buf(),
            page_num: page_num as u32,
            content_type: None,
        })
        .collect();
    app.image_metas = vec![Some((10, 20)); count];
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.visible_indices = (0..count).collect();
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    install_overview_demand(app);
}

fn overview_source_pixels(request: &LoadRequest) -> crate::thumb_loader::ThumbMsg {
    crate::thumb_loader::ThumbMsg::from_legacy_parts(
        request.idx,
        Some(egui::ColorImage::filled([2, 2], egui::Color32::WHITE)),
        crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
            evaluated_display_px: 320,
        },
        false,
        None,
        Some((2, 2)),
        None,
        false,
        false,
        request.input_seq,
        request.items_gen,
    )
    .with_request_id(request.request_id)
    .source_pixels()
}

fn overview_terminal(
    request: &LoadRequest,
    payload: crate::thumb_loader::ThumbMsgPayload,
) -> crate::thumb_loader::ThumbMsg {
    crate::thumb_loader::ThumbMsg {
        idx: request.idx,
        input_seq: request.input_seq,
        items_gen: request.items_gen,
        request_id: request.request_id,
        payload,
    }
}

fn assert_overview_pdf_promotion(app: &App, path: &std::path::Path, page_num: u32) {
    let PdfVisiblePromotionState::Tracking {
        ticket,
        keys,
        retry_needed,
    } = &app.pdf_visible_promotion
    else {
        panic!("overview admission must use the existing viewer promotion tracker");
    };
    assert!(ticket.same_request(&app.pdf_render_epoch.snapshot()));
    assert_eq!(
        keys,
        &HashSet::from([crate::grid_item::pdf_page_perf_key(path, page_num)])
    );
    assert!(
        *retry_needed,
        "thumbnail jobs have not reached the PDF pool yet"
    );
}

#[test]
fn overview_pdf_close_reopen_prunes_local_queue_and_preserves_source_save_lifetime() {
    use crate::pdf_loader::PdfRenderValidity;
    use crate::thumb_loader::ThumbMsgPayload;
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(2);
    prepare_pdf_overview(&mut app, std::path::Path::new("C:/overview/shared.pdf"), 2);
    let ticket = app.pdf_render_epoch.snapshot();
    let cancel = Arc::clone(&app.cancel_token);
    app.ensure_page_overview_requests(&ctx, &[0, 1]);
    let (popped, queued) = {
        let mut queue = app.reload_queue.as_ref().unwrap().0.lock().unwrap();
        assert_eq!(queue.len(), 2);
        let queued = queue[1].clone();
        (queue.remove(0), queued)
    };
    for request in [&popped, &queued] {
        assert!(request.priority);
        assert_eq!(Some(request.request_id), app.requested.id(request.idx));
        assert!(
            request
                .pdf_render_validity
                .same_request(&PdfRenderValidity::Viewer(ticket.clone()))
        );
    }
    app.close_page_overview_with_ctx(&ctx);
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(!app.requested.contains_key(&1));
    assert_eq!(app.requested.id(0), Some(popped.request_id));
    assert!(
        !ticket.is_stale(),
        "overview close must not invalidate the viewer epoch"
    );
    assert!(Arc::ptr_eq(&cancel, &app.cancel_token));
    assert!(!cancel.load(Ordering::Relaxed));

    install_overview_demand(&mut app);
    app.ensure_page_overview_requests(&ctx, &[0, 1]);
    let replacement = app.requested.id(1).unwrap();
    assert_ne!(replacement, queued.request_id);
    assert_eq!(
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().len(),
        1
    );
    send_thumbnail_for_test!(app, overview_terminal(&queued, ThumbMsgPayload::Canceled)).unwrap();
    send_thumbnail_for_test!(app, overview_source_pixels(&popped)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert_eq!(app.requested.id(1), Some(replacement));
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert_eq!(app.requested.id(0), Some(popped.request_id));

    // Uploaded pixels are disposable, but the source-save terminal is still outstanding.
    app.close_page_overview_with_ctx(&ctx);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert_eq!(app.requested.id(0), Some(popped.request_id));
    install_overview_demand(&mut app);
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    send_thumbnail_for_test!(app, overview_terminal(&popped, ThumbMsgPayload::Finalized)).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert!(!app.requested.contains_key(&0));
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert_ne!(app.requested.id(0), Some(popped.request_id));
    let queue = app.reload_queue.as_ref().unwrap().0.lock().unwrap();
    assert_eq!(
        queue.len(),
        1,
        "only the completed, evicted page may be requested again"
    );
    assert!(
        queue[0]
            .pdf_render_validity
            .same_request(&PdfRenderValidity::Viewer(ticket))
    );
}

#[test]
#[cfg(windows)]
fn overview_same_pdf_contexts_keep_separate_tickets_and_completion_channels() {
    use crate::pdf_loader::{JobPriority, PdfRenderValidity, TestPdfQueue};
    use crate::thumb_loader::ThumbMsgPayload;
    let ctx = egui::Context::default();
    let path = std::path::Path::new("C:/overview/sibling.pdf");
    let mut app = setup_thumbnail_app(1);
    prepare_pdf_overview(&mut app, path, 1);
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert_overview_pdf_promotion(&app, path, 0);
    let main_request = app
        .reload_queue
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .pop()
        .unwrap();
    let main_shared = Arc::clone(&app.still_seek_thumbnail_pages_shared);
    let main_cancel = Arc::clone(&app.cancel_token);
    let detached = app.build_window_context_for_test(1340, |mounted| {
        prepare_pdf_overview(mounted, path, 1);
        mounted.ensure_page_overview_requests(&ctx, &[0]);
        assert_overview_pdf_promotion(mounted, path, 0);
    });
    let detached_request = app
        .with_viewer_context(detached, |mounted| {
            assert!(!Arc::ptr_eq(
                &main_shared,
                &mounted.still_seek_thumbnail_pages_shared
            ));
            let request = mounted
                .reload_queue
                .as_ref()
                .unwrap()
                .0
                .lock()
                .unwrap()
                .pop()
                .unwrap();
            assert!(
                request
                    .pdf_render_validity
                    .same_request(&PdfRenderValidity::Viewer(
                        mounted.pdf_render_epoch.snapshot()
                    ))
            );
            request
        })
        .unwrap();
    assert_eq!(main_request.path, detached_request.path);
    assert_eq!(main_request.pdf_page, detached_request.pdf_page);
    assert_ne!(main_request.request_id, detached_request.request_id);
    assert!(
        !main_request
            .pdf_render_validity
            .same_request(&detached_request.pdf_render_validity)
    );
    let key = crate::grid_item::pdf_page_perf_key(path, 0);
    let mut pool = TestPdfQueue::new();
    let main_reply = pool.enqueue(
        &key,
        main_request.pdf_render_validity.clone(),
        JobPriority::HighNormal,
    );
    let detached_reply = pool.enqueue(
        &key,
        detached_request.pdf_render_validity.clone(),
        JobPriority::HighNormal,
    );

    app.close_page_overview_with_ctx(&ctx);
    install_overview_demand(&mut app);
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(Arc::ptr_eq(&main_cancel, &app.cancel_token));
    assert!(!main_request.pdf_render_validity.is_stale());
    assert!(!detached_request.pdf_render_validity.is_stale());
    pool.prune_stale();
    assert!(pool.complete_next());
    assert!(pool.complete_next());
    main_reply.recv().unwrap();
    detached_reply.recv().unwrap();

    send_thumbnail_for_test!(app, overview_source_pixels(&main_request)).unwrap();
    send_thumbnail_for_test!(
        app,
        overview_terminal(&main_request, ThumbMsgPayload::Finalized)
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
    assert!(!app.requested.contains_key(&0));
    app.with_viewer_context(detached, |mounted| {
        assert_eq!(mounted.still_seek_thumbnail_pages, HashSet::from([0]));
        assert_eq!(mounted.requested.id(0), Some(detached_request.request_id));
        assert!(matches!(mounted.thumbnails[0], ThumbnailState::Pending));
        send_thumbnail_for_test!(mounted, overview_source_pixels(&detached_request)).unwrap();
        send_thumbnail_for_test!(
            mounted,
            overview_terminal(&detached_request, ThumbMsgPayload::Finalized)
        )
        .unwrap();
        mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
        assert!(matches!(
            mounted.thumbnails[0],
            ThumbnailState::Loaded { .. }
        ));
        assert!(!mounted.requested.contains_key(&0));
    })
    .unwrap();
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
}

#[test]
fn overview_epub_admission_keeps_logical_identity_and_read_lease_across_close() {
    use crate::pdf_loader::{LeasedEpubPath, PdfRenderValidity, acquire_epub_delete_coverage};
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(1);
    // Unique disposable identity also prevents contention with unrelated EPUB gate tests.
    let temporary = tempfile::tempdir().unwrap();
    let logical = temporary.path().join("overview.epub");
    let _pin = crate::pdf_loader::pin_epub_for_test(&logical, 1340, 800);
    prepare_pdf_overview(&mut app, &logical, 1);
    app.display_epub_source = Some(LeasedEpubPath::try_new(logical.clone()).unwrap());
    let ticket = app.pdf_render_epoch.snapshot();
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert_overview_pdf_promotion(&app, &logical, 0);
    let request = app
        .reload_queue
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(request.path, logical);
    assert_eq!(request.pdf_page, Some(0));
    assert_eq!(
        request.pdf_stamp_policy,
        crate::thumb_loader::PdfStampPolicy::ResolveInWorker
    );
    assert!(
        request
            .pdf_render_validity
            .same_request(&PdfRenderValidity::Viewer(ticket.clone()))
    );
    assert_eq!(Some(request.request_id), app.requested.id(0));
    // Use the same generation-stamping helper as the worker, retaining outer UI correlation.
    let read = crate::pdf_loader::pinned_epub_target(&logical).unwrap();
    let derived = crate::thumb_loader::stamp_resolved_pdf_request(&request, &read).unwrap();
    assert_eq!(derived.path, logical);
    assert_eq!(derived.pdf_page, request.pdf_page);
    assert_eq!(derived.request_id, request.request_id);
    assert_eq!(derived.items_gen, request.items_gen);
    assert_eq!(derived.input_seq, request.input_seq);
    assert!(
        derived
            .pdf_render_validity
            .same_request(&request.pdf_render_validity)
    );
    assert_eq!(
        Some((derived.mtime, derived.file_size)),
        read.stamp.generation_catalog_pair()
    );
    assert!(acquire_epub_delete_coverage(&logical).is_err());
    app.close_page_overview_with_ctx(&ctx);
    assert!(app.display_epub_source.as_ref().unwrap().has_epub_lease());
    assert!(acquire_epub_delete_coverage(&logical).is_err());
    assert!(!ticket.is_stale());
    install_overview_demand(&mut app);
    app.ensure_page_overview_requests(&ctx, &[0]);
    assert_eq!(app.requested.id(0), Some(request.request_id));
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
        request
            .pdf_render_validity
            .same_request(&PdfRenderValidity::Viewer(app.pdf_render_epoch.snapshot()))
    );
    // Viewer-list replacement invalidates the original request, without freshening its ticket.
    app.bump_render_epoch_only();
    assert!(request.pdf_render_validity.is_stale());
    assert!(derived.pdf_render_validity.is_stale());
    assert!(acquire_epub_delete_coverage(&logical).is_err());
    app.display_epub_source.take();
    assert!(
        acquire_epub_delete_coverage(&logical).is_err(),
        "resolved generation still owns its read lease"
    );
    drop(read);
    assert!(acquire_epub_delete_coverage(&logical).is_ok());
}

#[test]
fn overview_detached_range_change_prunes_local_queue_but_preserves_popped_terminal() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(20);
    install_overview_demand(&mut app);
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.fs_holdover_tex = Some(navigation_sequence(app.items_generation, vec![2]));
    app.ensure_page_overview_requests(&ctx, &[4, 6]);
    let popped = app
        .reload_queue
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(popped.idx, 6);
    app.thumbnails[4] = loaded_thumbnail(&ctx, "overview-departing");
    app.ensure_page_overview_requests(&ctx, &[10]);
    assert_eq!(app.keep_set, HashSet::from([2, 10]));
    assert!(matches!(app.thumbnails[4], ThumbnailState::Evicted));
    assert!(!app.requested.contains_key(&4));
    assert_eq!(app.requested.id(6), Some(popped.request_id));
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|req| req.idx == 10)
    );
    app.close_page_overview_with_ctx(&ctx);
    assert_eq!(app.keep_set, HashSet::from([2]));
    assert!(
        app.still_seek_thumbnail_pages_shared
            .read()
            .unwrap()
            .is_empty()
    );
    assert_eq!(app.requested.id(6), Some(popped.request_id));
}

#[test]
fn overview_root_exact_replacement_retains_canonical_grid_band() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(200);
    install_overview_demand(&mut app);
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    let band = app.grid_thumbnail_retention_geometry(&ctx);
    let grid = app.visible_indices[band.vis_keep_start_capped..band.vis_keep_end].to_vec();
    assert!(!grid.contains(&198));
    app.ensure_page_overview_requests(&ctx, &[198]);
    app.ensure_page_overview_requests(&ctx, &[196]);
    assert!(!app.keep_set.contains(&198));
    assert!(grid.iter().all(|idx| app.keep_set.contains(idx)));
    app.close_page_overview_with_ctx(&ctx);
    assert_eq!(app.keep_set, grid.into_iter().collect());
}

#[test]
#[cfg(windows)]
fn overview_park_withdraws_demand_without_closing_metadata() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(20);
    install_overview_demand(&mut app);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    let view = app.page_overview_view().unwrap();
    app.ensure_page_overview_requests(&ctx, &[4, 6]);
    app.pause_mounted_background_work_keep_current_frame();
    assert_eq!(app.page_overview_view().unwrap().stamp, view.stamp);
    assert!(Arc::ptr_eq(
        &app.page_overview_view().unwrap().projection,
        &view.projection
    ));
    assert!(app.still_seek_thumbnail_pages.is_empty());
    assert!(
        app.still_seek_thumbnail_pages_shared
            .read()
            .unwrap()
            .is_empty()
    );
    assert!(app.keep_set.is_empty());
    assert!(
        app.reload_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .is_empty()
    );
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
    let input_seq = app.input_seq;
    let items_generation = app.items_generation;
    for idx in 0..4 {
        app.requested
            .begin(
                idx,
                items_generation,
                crate::app::thumbnail_requests::ThumbnailRequestKind::Normal,
                None,
            )
            .unwrap();
    }
    let queued_id = app.requested.id(0).unwrap();
    let popped_id = app.requested.id(1).unwrap();
    let cached_id = app.requested.id(2).unwrap();
    let finalized_id = app.requested.id(3).unwrap();
    app.reload_queue
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .push(LoadRequest {
            idx: 0,
            request_id: queued_id,
            input_seq,
            items_gen: items_generation,
            ..Default::default()
        });
    queue_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            2,
            Some(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            false,
            None,
            Some((1, 1)),
            None,
            false,
            false,
            input_seq,
            items_generation,
        )
        .with_request_id(cached_id)
    );
    // A finalized source image still awaiting upload is disposable on release.
    // Finalized alone would instead model a worker result whose image has not arrived.
    queue_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            3,
            Some(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            Some((1, 1)),
            None,
            false,
            false,
            input_seq,
            items_generation,
        )
        .with_request_id(finalized_id)
        .source_pixels()
    );
    app.requested.mark_finalized_for_test(3);
    assert!(app.requested.finalized_image_wait(3));

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
    assert!((app.requested.images_len() == 0));
    assert!(!app.requested.any_finalized_image_wait());

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

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            1,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            None,
            None,
            true,
            false,
            app.input_seq,
            app.items_generation,
        )
        .with_request_id(popped_id)
    )
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

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            0,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            None,
            None,
            false,
            true,
            app.input_seq,
            app.items_generation,
        )
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);
    assert!(app.requested.contains_key(&0));
    assert!(app.requested.finalized_image_wait(0));

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            0,
            Some(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            Some((1, 1)),
            None,
            false,
            false,
            app.input_seq,
            app.items_generation,
        )
    )
    .unwrap();
    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            1,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            false,
            None,
            None,
            None,
            false,
            false,
            app.input_seq,
            app.items_generation,
        )
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::PassthroughRendition);

    assert!(!app.requested.contains_key(&0));
    assert!(!app.requested.finalized_image_wait(0));
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
    app.current_book_bookmarks_key = Some(BookBookmarkProjectionKey {
        container_key,
        visibility: app.listing_file_type_visibility().clone(),
        context_id: app.virtual_list_context_id(),
        items_generation: app.items_generation,
    });
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

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            3,
            Some(egui::ColorImage::filled([2, 2], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            Some((2, 2)),
            None,
            false,
            false,
            app.input_seq,
            app.items_generation,
        )
    )
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

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            3,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            Some((2, 2)),
            None,
            false,
            true,
            app.input_seq,
            app.items_generation,
        )
    )
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

    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            3,
            Some(egui::ColorImage::filled([2, 2], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 320,
            },
            false,
            None,
            Some((2, 2)),
            None,
            false,
            false,
            app.input_seq,
            old_generation,
        )
    )
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

#[test]
fn audio_no_art_is_terminal_across_grid_details_hover_and_stale_completions() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(1);
    app.items[0] = GridItem::Audio(PathBuf::from("C:/section-1347/song.mp3"));
    app.image_metas[0] = None;
    app.keep_set.insert(0);
    app.keep_range = (0, 1);
    app.requested.insert(0, false);
    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg {
            request_id: crate::thumb_loader::ThumbnailRequestId::Standalone,
            idx: 0,
            input_seq: app.input_seq,
            items_gen: app.items_generation,
            payload: crate::thumb_loader::ThumbMsgPayload::NoArt,
        }
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
    assert!(!app.requested.contains_key(&0));
    assert!(!app.requested.finalized_image_wait(0));
    for _ in 0..3 {
        app.set_details_hover_thumbnail_idx(Some(0));
        app.set_details_hover_thumbnail_idx(None);
    }
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

    app.thumbnails[0] = loaded_thumbnail(&ctx, "audio-current-generation");
    send_thumbnail_for_test!(
        app,
        crate::thumb_loader::ThumbMsg {
            request_id: crate::thumb_loader::ThumbnailRequestId::Standalone,
            idx: 0,
            input_seq: app.input_seq,
            items_gen: app.items_generation.wrapping_sub(1),
            payload: crate::thumb_loader::ThumbMsgPayload::NoArt,
        }
    )
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(
        matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }),
        "late absence belongs only to its old list generation"
    );
}

#[test]
fn audio_art_dimensions_participate_in_auto_aspect_and_no_art_settles_the_denominator() {
    let ctx = egui::Context::default();
    let mut app = setup_thumbnail_app(2);
    app.settings.thumb_aspect_auto = true;
    app.items = vec![
        GridItem::Audio(PathBuf::from("C:/section-1347/art.mp3")),
        GridItem::Audio(PathBuf::from("C:/section-1347/empty.flac")),
    ];
    let mut loaded = loaded_thumbnail(&ctx, "audio-aspect-source-dimensions");
    if let ThumbnailState::Loaded { source_dims, .. } = &mut loaded {
        *source_dims = Some((1600, 900));
    }
    app.thumbnails = vec![loaded, ThumbnailState::NoArt];
    app.rebuild_auto_aspect_samples_from_loaded();
    assert_eq!(app.auto_aspect.samples.get(&0), Some(&(900.0 / 1600.0)));
    assert!(!app.auto_aspect.samples.contains_key(&1));
    assert_eq!(app.auto_aspect_eligible_total(), 1);
    app.thumbnails = vec![ThumbnailState::NoArt, ThumbnailState::NoArt];
    app.auto_aspect = Default::default();
    let expected = app.effective_thumb_aspect();
    app.maybe_apply_auto_aspect(true);
    assert_eq!(app.auto_aspect_eligible_total(), 0);
    assert_eq!(
        app.auto_aspect.current,
        Some(expected),
        "absence is a settled result, not an unbounded wait for dimensions"
    );
}
