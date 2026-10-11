//! PDF work keeps the same context identity through admission and list replacement.
use super::phase_c_support::setup_app;
use super::*;
use crate::pdf_loader::{JobPriority, PdfRenderEpochOwner, PdfRenderValidity, TestPdfQueue};
use crate::thumb_loader::{ThumbLoadOrigin, ThumbMsg, ThumbMsgPayload};

fn prepare_pdf(app: &mut App) {
    // The file deliberately does not exist: these tests never start PDFium or do PDF I/O.
    app.items = vec![GridItem::PdfPage {
        pdf_path: PathBuf::from(r"C:\books\shared.pdf"),
        page_num: 0,
        content_type: None,
    }];
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.keep_set.insert(0);
    app.keep_range = (0, 1);
    app.keep_start_shared.store(0, Ordering::Relaxed);
    app.keep_end_shared.store(1, Ordering::Relaxed);
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.requested
        .begin(0, app.items_generation, ThumbnailRequestKind::Normal, None)
        .unwrap();
}

fn canceled(app: &App) -> ThumbMsg {
    ThumbMsg {
        idx: 0,
        input_seq: app.input_seq,
        items_gen: app.items_generation,
        request_id: app.requested.id(0).unwrap(),
        payload: ThumbMsgPayload::Canceled,
    }
}

fn ready(app: &App) -> ThumbMsg {
    ThumbMsg::from_legacy_parts(
        0,
        Some(egui::ColorImage::filled([2, 2], egui::Color32::LIGHT_BLUE)),
        ThumbLoadOrigin::SourceIntrinsic,
        false,
        None,
        Some((2, 2)),
        None,
        false,
        false,
        app.input_seq,
        app.items_generation,
    )
    .with_request_id(app.requested.id(0).unwrap())
}

#[test]
fn admission_stamps_projected_viewer_and_keeps_old_ticket_after_replacement() {
    let mut app = setup_app();
    let mut req = LoadRequest::default();
    app.attach_viewer_thumbnail_context(&mut req);
    assert_eq!(req.items_gen, app.items_generation);
    assert_eq!(req.input_seq, app.input_seq);
    let PdfRenderValidity::Viewer(ticket) = &req.pdf_render_validity else {
        panic!("UI requests must be Viewer scoped");
    };
    assert!(ticket.same_request(&app.pdf_render_epoch.snapshot()));
    app.bump_render_epoch_only();
    assert!(req.pdf_render_validity.is_stale());
    assert!(!app.pdf_render_epoch.snapshot().is_stale());
}

#[test]
fn visible_promotion_same_key_needs_new_owner_or_generation_and_late_arrival_retry() {
    let mut a = PdfRenderEpochOwner::fresh();
    let b = PdfRenderEpochOwner::fresh();
    let keys = HashSet::from(["same.pdf:p0".to_owned()]);
    let mut state = PdfVisiblePromotionState::Tracking {
        ticket: a.snapshot(),
        keys: keys.clone(),
        retry_needed: false,
    };
    assert!(!state.needs_attempt(&a.snapshot(), &keys));
    assert!(state.needs_attempt(&b.snapshot(), &keys));
    a.advance();
    assert!(state.needs_attempt(&a.snapshot(), &keys));
    state = PdfVisiblePromotionState::Tracking {
        ticket: a.snapshot(),
        keys: keys.clone(),
        retry_needed: true,
    };
    assert!(state.needs_attempt(&a.snapshot(), &keys));
    assert!(!state.needs_attempt(&a.snapshot(), &HashSet::new()));
}

#[test]
fn normal_priority_and_upgrade_admission_use_the_viewer_ticket_and_registered_id() {
    let mut app = setup_app();
    prepare_pdf(&mut app);
    app.image_metas = vec![Some((10, 20))];
    app.requested.clear();
    app.thumbnails[0] = ThumbnailState::Evicted;
    app.settings.grid_cols = 1;
    app.last_cell_size = 120.0;
    app.last_cell_h = 120.0;
    app.last_viewport_h = 120.0;
    let ctx = egui::Context::default();
    let assert_queued = |app: &App| {
        let queue = app.reload_queue.as_ref().unwrap().0.lock().unwrap();
        assert_eq!(queue.len(), 1);
        let request = &queue[0];
        assert_eq!(request.items_gen, app.items_generation);
        assert_eq!(Some(request.request_id), app.requested.id(0));
        assert!(
            request
                .pdf_render_validity
                .same_request(&PdfRenderValidity::Viewer(app.pdf_render_epoch.snapshot()))
        );
    };
    app.enqueue_priority_thumbnail(0);
    assert_queued(&app);
    app.reload_queue.as_ref().unwrap().0.lock().unwrap().clear();
    app.requested.clear();
    app.update_keep_range_and_requests(&ctx, std::time::Instant::now());
    assert_queued(&app);
    app.reload_queue.as_ref().unwrap().0.lock().unwrap().clear();
    let message = ThumbMsg::from_legacy_parts(
        0,
        Some(egui::ColorImage::filled([2, 2], egui::Color32::LIGHT_BLUE)),
        ThumbLoadOrigin::UpgradeableCache,
        false,
        None,
        Some((2000, 2000)),
        None,
        false,
        false,
        app.input_seq,
        app.items_generation,
    )
    .with_request_id(app.requested.id(0).unwrap());
    app.tx.send(message).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(!app.requested.blocks_idle_upgrade());
    app.settings.thumb_idle_upgrade = true;
    app.last_scroll_change_time = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.last_input_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
    app.display_px_shared.store(374, Ordering::Relaxed);
    app.enqueue_idle_upgrades();
    assert_queued(&app);
    assert_eq!(app.requested.get(&0), Some(&true));
}

#[cfg(windows)]
#[test]
fn main_and_detached_reload_rotate_only_the_projected_context_resources() {
    for reload_detached in [false, true] {
        let mut app = setup_app();
        let main_ticket = app.pdf_render_epoch.snapshot();
        let main_cancel = app.cancel_token.clone();
        let detached = app.build_window_context_for_test(1375, |_| {});
        let (sibling_ticket, sibling_cancel) = app
            .with_viewer_context(detached, |mounted| {
                (
                    mounted.pdf_render_epoch.snapshot(),
                    mounted.cancel_token.clone(),
                )
            })
            .unwrap();
        let mut pool = TestPdfQueue::new();
        let main_reply = pool.enqueue(
            "same.pdf#0",
            PdfRenderValidity::Viewer(main_ticket.clone()),
            JobPriority::Normal,
        );
        let sibling_reply = pool.enqueue(
            "same.pdf#0",
            PdfRenderValidity::Viewer(sibling_ticket.clone()),
            JobPriority::Normal,
        );
        if reload_detached {
            app.with_viewer_context(detached, |mounted| {
                mounted.bump_full_context_for_load();
                assert!(!Arc::ptr_eq(&mounted.cancel_token, &sibling_cancel));
                assert!(!mounted.cancel_token.load(Ordering::Relaxed));
            })
            .unwrap();
        } else {
            app.bump_full_context_for_load();
            assert!(!Arc::ptr_eq(&app.cancel_token, &main_cancel));
            assert!(!app.cancel_token.load(Ordering::Relaxed));
        }
        assert_eq!(main_ticket.is_stale(), !reload_detached);
        assert_eq!(sibling_ticket.is_stale(), reload_detached);
        assert_eq!(main_cancel.load(Ordering::Relaxed), !reload_detached);
        assert_eq!(sibling_cancel.load(Ordering::Relaxed), reload_detached);
        pool.prune_stale();
        assert!(pool.complete_next());
        if reload_detached {
            main_reply.recv().unwrap();
            assert_eq!(
                sibling_reply.recv().unwrap_err().kind(),
                std::io::ErrorKind::Interrupted
            );
        } else {
            sibling_reply.recv().unwrap();
            assert_eq!(
                main_reply.recv().unwrap_err().kind(),
                std::io::ErrorKind::Interrupted
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn main_and_detached_same_pdf_search_replacement_preserves_sibling_completion() {
    for replace_detached in [false, true] {
        let mut app = setup_app();
        prepare_pdf(&mut app);
        let main_ticket = app.pdf_render_epoch.snapshot();
        let main_generation = app.items_generation;
        let main_request = app.requested.id(0).unwrap();
        let main_cancel = app.cancel_token.clone();
        let main_queue = app.reload_queue.clone().unwrap();
        let detached = app.build_window_context_for_test(1374, |mounted| {
            prepare_pdf(mounted);
            mounted.image_metas = vec![Some((10, 20))];
            mounted.requested.clear();
            mounted.ensure_still_seek_thumbnail_requests(&egui::Context::default(), &[0]);
            let queue = mounted.reload_queue.as_ref().unwrap().0.lock().unwrap();
            assert_eq!(queue.len(), 1);
            assert!(
                queue[0]
                    .pdf_render_validity
                    .same_request(&PdfRenderValidity::Viewer(
                        mounted.pdf_render_epoch.snapshot()
                    ))
            );
            assert_eq!(Some(queue[0].request_id), mounted.requested.id(0));
        });
        let (
            detached_ticket,
            detached_generation,
            detached_request,
            detached_cancel,
            detached_queue,
        ) = app
            .with_viewer_context(detached, |mounted| {
                (
                    mounted.pdf_render_epoch.snapshot(),
                    mounted.items_generation,
                    mounted.requested.id(0).unwrap(),
                    mounted.cancel_token.clone(),
                    mounted.reload_queue.clone().unwrap(),
                )
            })
            .unwrap();
        // The fake PDF consumer receives the actual seek request popped by the
        // thumbnail worker; the UI queue no longer owns that in-flight request.
        let detached_validity = {
            let request = detached_queue.0.lock().unwrap().pop().unwrap();
            assert_eq!(request.request_id, detached_request);
            assert_eq!(request.items_gen, detached_generation);
            request.pdf_render_validity
        };
        let mut pool = TestPdfQueue::new();
        let key = crate::grid_item::pdf_page_perf_key(Path::new(r"C:\books\shared.pdf"), 0);
        let main_reply = pool.enqueue(
            &key,
            PdfRenderValidity::Viewer(main_ticket.clone()),
            JobPriority::Normal,
        );
        let detached_reply = pool.enqueue(&key, detached_validity, JobPriority::HighNormal);
        let old_message = if replace_detached {
            app.with_viewer_context(detached, |mounted| {
                let message = canceled(mounted);
                mounted.replace_search_view_items(mounted.items.clone(), vec![None]);
                message
            })
            .unwrap()
        } else {
            let message = canceled(&app);
            let rows = app.items.clone();
            app.replace_search_view_items(rows, vec![None]);
            message
        };
        pool.prune_stale();
        assert!(pool.complete_next());
        let (invalidated, preserved) = if replace_detached {
            (detached_reply.recv(), main_reply.recv())
        } else {
            (main_reply.recv(), detached_reply.recv())
        };
        assert_eq!(
            invalidated.unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
        preserved.unwrap();
        let ctx = egui::Context::default();
        if replace_detached {
            assert!(!main_ticket.is_stale());
            assert_eq!(app.items_generation, main_generation);
            assert_eq!(app.requested.id(0), Some(main_request));
            assert!(Arc::ptr_eq(&app.cancel_token, &main_cancel));
            assert!(Arc::ptr_eq(app.reload_queue.as_ref().unwrap(), &main_queue));
            assert_eq!(app.keep_range, (0, 1));
            app.tx.send(ready(&app)).unwrap();
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
            app.with_viewer_context(detached, |mounted| {
                let mut new_request = LoadRequest::default();
                mounted.attach_viewer_thumbnail_context(&mut new_request);
                let new_id = mounted
                    .requested
                    .begin(
                        0,
                        mounted.items_generation,
                        ThumbnailRequestKind::Normal,
                        None,
                    )
                    .unwrap();
                mounted.tx.send(old_message).unwrap();
                mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
                assert_eq!(
                    mounted.requested.id(0),
                    Some(new_id),
                    "old Canceled must not end a new request"
                );
            })
            .unwrap();
        } else {
            app.tx.send(old_message).unwrap();
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            app.with_viewer_context(detached, |mounted| {
                assert!(!detached_ticket.is_stale());
                assert_eq!(mounted.items_generation, detached_generation);
                assert_eq!(mounted.requested.id(0), Some(detached_request));
                assert!(Arc::ptr_eq(&mounted.cancel_token, &detached_cancel));
                assert!(Arc::ptr_eq(
                    mounted.reload_queue.as_ref().unwrap(),
                    &detached_queue
                ));
                assert!(!mounted.cancel_token.load(Ordering::Relaxed));
                assert_eq!(mounted.keep_range, (0, 1));
                assert_eq!(mounted.still_seek_thumbnail_pages, HashSet::from([0]));
                assert!(detached_queue.0.lock().unwrap().is_empty());
                mounted.tx.send(ready(mounted)).unwrap();
                mounted.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
                assert!(matches!(
                    mounted.thumbnails[0],
                    ThumbnailState::Loaded { .. }
                ));
                assert!(!mounted.requested.contains_key(&0));
            })
            .unwrap();
        }
        assert!(!main_cancel.load(Ordering::Relaxed));
        assert!(!detached_cancel.load(Ordering::Relaxed));
    }
}
