use super::*;

fn ready_duration_with_unread_visible_page_count() -> (phase_c_support::AppTestEnv, usize) {
    let mut app = fixture();
    let zip_path = app.tmp.path().join("memory-only.zip");
    let entries = ["book.zip/001.jpg", "book.zip/002.jpg", "book.zip/003.jpg"]
        .into_iter()
        .map(|entry_name| crate::zip_loader::ZipImageEntry {
            entry_name: entry_name.to_owned(),
            uncompressed_size: 100,
            mtime: 100,
            container_ancestry: Vec::new(),
        })
        .collect();
    let tree = Arc::new(crate::zip_tree::ZipTree::build(zip_path, entries));
    // Render the requested root, before ZipNavState collapses its single wrapper.
    let (warm_items, warm_metas) = tree.materialize_level(&[], SortOrder::FileName);
    let nav = crate::zip_tree::ZipNavState::new(tree);
    assert_eq!(warm_items.len(), 1);
    let page_idx = app.items.len();
    let mut items = app.items.clone();
    items.extend(warm_items);
    let mut metas = app.image_metas.clone();
    metas.extend(warm_metas);
    app.install_new_items(items, metas);
    app.zip_nav = Some(nav);
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_page_count = true;
    app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
    app.duration_facet_requested = true;
    app.visible_indices = (0..app.items.len()).collect();
    app.details_tag_prewarm_indices = vec![0];
    cache_all(&mut app);

    // All lengths are cached, while the offscreen page count stays unrequested.
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.details_meta_pending.is_none());
    assert!(app.duration_scan_ready());
    assert_eq!(
        app.details_image_dims_state,
        LazyColumnState::Ready { failed: 0 }
    );
    app.details_tag_prewarm_indices = vec![page_idx];
    let target = app
        .details_meta_target_for_idx(page_idx, &HashSet::from([page_idx]), true)
        .expect("newly visible page count must still need acquisition");
    assert_eq!(target.warm_page_count, Some(3));
    assert!(target.load_page_count);
    assert!(!target.load_video_meta && !target.load_image_dims);
    assert!(!target.load_created_at && !target.load_ai_metadata);
    assert!(app.details_visible_stage_needs_load());
    (app, page_idx)
}

fn finish_warm_visible_stage(
    app: &mut App,
    ctx: &egui::Context,
    proof: Option<ListingMetaScanProof>,
) {
    // The worker only copies the in-memory ZipTree count. No ZIP, PDF or media probe runs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.details_meta_pending.is_some() {
        app.poll_details_meta_load(ctx);
        assert_eq!(app.listing_meta_scan_proof, proof);
        assert!(
            std::time::Instant::now() < deadline,
            "warm visible stage did not finish"
        );
        std::thread::yield_now();
    }
    assert!(matches!(
        app.details_image_dims_state,
        LazyColumnState::Reconciling { .. }
    ));
    app.reconcile_details_lazy_session_after_grid(ctx);
    assert_eq!(
        app.details_image_dims_state,
        LazyColumnState::Ready { failed: 0 }
    );
    assert_eq!(app.listing_meta_scan_proof, proof);
    assert!(app.duration_scan_ready());
}

#[test]
fn duration_762_proven_lengths_allow_post_grid_visible_stage_and_keep_proof_through_finished() {
    let (mut app, page_idx) = ready_duration_with_unread_visible_page_count();
    let ctx = egui::Context::default();
    let proof = app.listing_meta_scan_proof;

    app.reconcile_details_lazy_session_after_grid(&ctx);

    let pending = app
        .details_meta_pending
        .as_ref()
        .expect("post-grid caller must start a stage");
    assert_eq!(pending.scan_scope, DetailsMetaScanScope::VisibleStage);
    assert!(pending.listing_meta_scan.is_none());
    assert!(matches!(
        pending.phase,
        DetailsMetaPendingPhase::Loading { .. }
    ));
    assert_eq!(app.listing_meta_scan_proof, proof);
    finish_warm_visible_stage(&mut app, &ctx, proof);
    assert_eq!(
        app.details_lazy_meta_for_idx(page_idx).unwrap().page_count,
        Some(3)
    );
}

#[test]
fn duration_762_proven_lengths_allow_poll_ready_visible_stage() {
    let (mut app, page_idx) = ready_duration_with_unread_visible_page_count();
    let ctx = egui::Context::default();
    let proof = app.listing_meta_scan_proof;
    let revision = app.details_lazy_visible_revision;

    app.poll_details_meta_load(&ctx);

    assert_eq!(app.details_lazy_visible_revision, revision.wrapping_add(1));
    if let Some(pending) = app.details_meta_pending.as_ref() {
        assert_eq!(pending.scan_scope, DetailsMetaScanScope::VisibleStage);
        assert!(pending.listing_meta_scan.is_none());
    }
    // A fast warm worker may already have published Finished in the starting poll.
    finish_warm_visible_stage(&mut app, &ctx, proof);
    assert_eq!(
        app.details_lazy_meta_for_idx(page_idx).unwrap().page_count,
        Some(3)
    );
}

#[test]
fn duration_762_cancelled_keeps_visible_stages_stopped_with_or_without_duration_proof() {
    for proven in [false, true] {
        let (mut app, _) = ready_duration_with_unread_visible_page_count();
        if !proven {
            app.listing_meta_scan_proof = None;
        }
        app.cancel_details_meta_loading();
        let proof = app.listing_meta_scan_proof;
        let revision = app.details_lazy_visible_revision;
        let ctx = egui::Context::default();

        app.reconcile_details_lazy_session_after_grid(&ctx);
        app.poll_details_meta_load(&ctx);

        assert_eq!(app.details_image_dims_state, LazyColumnState::Cancelled);
        assert!(app.details_meta_pending.is_none());
        assert_eq!(app.details_lazy_visible_revision, revision);
        assert_eq!(app.listing_meta_scan_proof, proof);
        assert!(app.details_visible_stage_needs_load());
    }
}

#[test]
fn duration_762_thumbnail_cancelled_badge_only_scroll_keeps_existing_restart_behavior() {
    let mut app = fixture();
    app.duration_facet_requested = false;
    app.settings.thumb_show_media_duration = true;
    assert!(!app.whole_listing_meta_scan_wanted());
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([4]);
    app.last_prefetch_scroll_at = Some(std::time::Instant::now() - PREFETCH_IDLE_THRESHOLD);
    app.cancel_details_meta_loading();
    let revision = app.details_lazy_visible_revision;

    app.refresh_thumbnail_details_stage(&egui::Context::default());

    // The existing badge-only range owner invalidates even a stopped old range.
    assert_eq!(app.details_tag_prewarm_indices, vec![4]);
    assert_eq!(app.details_image_dims_state, LazyColumnState::NotRequested);
    assert_eq!(app.details_lazy_visible_revision, revision.wrapping_add(1));
    assert!(app.details_meta_pending.is_none());
}

#[test]
fn duration_762_thumbnail_cancelled_duration_owner_stays_stopped_after_scroll() {
    for proven in [false, true] {
        let mut app = fixture();
        if proven {
            cache_all(&mut app);
            app.start_details_meta_load(&egui::Context::default());
            assert!(app.duration_scan_ready());
            assert!(app.details_meta_pending.is_none());
        }
        app.settings.thumb_show_media_duration = true;
        assert!(app.whole_listing_meta_scan_wanted());
        app.details_tag_prewarm_indices = vec![0];
        app.keep_set = HashSet::from([4]);
        app.last_prefetch_scroll_at = Some(std::time::Instant::now() - PREFETCH_IDLE_THRESHOLD);
        app.cancel_details_meta_loading();
        let revision = app.details_lazy_visible_revision;
        let proof = app.listing_meta_scan_proof;

        app.refresh_thumbnail_details_stage(&egui::Context::default());

        // Like the existing AI owner, a whole-listing stop survives range changes.
        assert_eq!(app.details_tag_prewarm_indices, vec![4]);
        assert_eq!(app.details_image_dims_state, LazyColumnState::Cancelled);
        assert_eq!(app.details_lazy_visible_revision, revision);
        assert_eq!(app.listing_meta_scan_proof, proof);
        assert!(app.details_meta_pending.is_none());
    }
}
