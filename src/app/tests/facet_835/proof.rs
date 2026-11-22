//! Stage 3: qualifications belong to a listing and a captured requirement set.
use super::*;
use crate::app::duration_metadata::{ListingMetaRequirements, ListingMetaScanProof};

const LENGTHS: ListingMetaRequirements = ListingMetaRequirements {
    duration: true,
    ai: false,
};
const AI: ListingMetaRequirements = ListingMetaRequirements {
    duration: false,
    ai: true,
};
const BOTH: ListingMetaRequirements = ListingMetaRequirements {
    duration: true,
    ai: true,
};

// Only the pure proof algebra tests below synthesize proof snapshots. Acquisition
// tests use real PNG metadata workers; none set Ready or install a proof by hand.
fn snapshot(app: &App, requirements: ListingMetaRequirements) -> ListingMetaScanProof {
    ListingMetaScanProof {
        requirements,
        ..app.listing_meta_scan_snapshot()
    }
}

#[test]
fn facet_835_proof_unions_same_listing_requirements_in_both_orders() {
    for (first, second) in [(LENGTHS, AI), (AI, LENGTHS)] {
        let mut app = setup_app();
        configure(&mut app);
        let first_scan = snapshot(&app, first);
        let second_scan = snapshot(&app, second);
        assert_eq!(app.complete_listing_meta_scan(Some(first_scan)), first);
        assert_eq!(app.listing_meta_scan_proof, Some(first_scan));
        assert_eq!(app.complete_listing_meta_scan(Some(second_scan)), second);
        assert_eq!(
            app.listing_meta_scan_proof,
            Some(snapshot(&app, BOTH)),
            "the second completion must preserve the first qualification"
        );
        assert_eq!(
            app.complete_listing_meta_scan(Some(first_scan)),
            ListingMetaRequirements::default(),
            "repeated completion must not report an already proven field as new"
        );
        assert_eq!(
            app.complete_listing_meta_scan(None),
            ListingMetaRequirements::default()
        );
        assert_eq!(app.listing_meta_scan_proof, Some(snapshot(&app, BOTH)));
        assert!(app.duration_scan_ready() && app.ai_model_facet_ready());
    }
}

#[test]
fn facet_835_proof_does_not_union_different_generations_or_lengths() {
    for mismatch_generation in [true, false] {
        let mut app = setup_app();
        configure(&mut app);
        let current = snapshot(&app, AI);
        let mut old = snapshot(&app, LENGTHS);
        if mismatch_generation {
            old.generation = old.generation.wrapping_add(1);
        } else {
            old.items_len += 1;
        }
        assert!(!old.matches(&app));
        assert_eq!(
            app.complete_listing_meta_scan(Some(old)),
            ListingMetaRequirements::default()
        );
        assert!(app.listing_meta_scan_proof.is_none());
        // An existing proof can become stale between completions as well.
        app.listing_meta_scan_proof = Some(old);
        assert_eq!(app.complete_listing_meta_scan(Some(current)), AI);
        assert_eq!(app.listing_meta_scan_proof, Some(current));
        assert!(!app.duration_scan_ready());
        assert!(app.ai_model_facet_ready());
        assert_eq!(
            app.complete_listing_meta_scan(Some(old)),
            ListingMetaRequirements::default()
        );
        assert_eq!(app.listing_meta_scan_proof, Some(current));
    }
}

#[test]
fn facet_835_unproven_requirements_distinguish_partial_and_complete_proofs() {
    let mut app = setup_app();
    configure(&mut app);
    app.duration_facet_requested = true;
    app.ai_model_facet_requested = true;
    assert_eq!(app.listing_meta_requirements(), BOTH);
    for (proven, missing) in [
        (ListingMetaRequirements::default(), BOTH),
        (LENGTHS, AI),
        (AI, LENGTHS),
        (BOTH, ListingMetaRequirements::default()),
    ] {
        app.listing_meta_scan_proof = Some(snapshot(&app, proven));
        assert_eq!(app.unproven_listing_meta_requirements(), missing);
        assert_eq!(app.duration_scan_ready(), proven.duration);
        assert_eq!(app.ai_model_facet_ready(), proven.ai);
        assert_eq!(BOTH.without(proven), missing);
        assert!(proven.union(missing).contains(BOTH));
        assert_eq!(missing.is_empty(), proven == BOTH);
    }
    let mut stale = snapshot(&app, BOTH);
    stale.items_len += 1;
    app.listing_meta_scan_proof = Some(stale);
    assert_eq!(app.unproven_listing_meta_requirements(), BOTH);
    assert!(!app.duration_scan_ready() && !app.ai_model_facet_ready());
}

fn real_images() -> AppTestEnv {
    let mut app = setup_app();
    configure(&mut app);
    let mut items = Vec::new();
    let mut metas = Vec::new();
    for (name, model) in [("A.png", "wanted"), ("B.png", "wanted"), ("C.png", "other")] {
        let path = app.tmp.path().join(name);
        std::fs::write(&path, png_bytes(model)).unwrap();
        metas.push(crate::catalog::media_source_identity(&path));
        items.push(GridItem::Image(path));
    }
    app.install_new_items(items, metas);
    app.selected = None;
    app.rebuild_visible_indices();
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    app
}

struct HeldFinished {
    delivery: mpsc::Sender<DetailsMetaEvent>,
    event: DetailsMetaEvent,
    cancel: Arc<AtomicBool>,
    scan: Option<ListingMetaScanProof>,
}

impl HeldFinished {
    fn release(self, app: &mut App, ctx: &egui::Context) {
        self.delivery
            .send(self.event)
            .expect("live completion owner");
        app.poll_details_meta_load(ctx);
    }
}

fn hold_real_finished(app: &mut App, ctx: &egui::Context) -> HeldFinished {
    // Advance the real bounded planner without polling its event receiver. Once
    // it launches, even an already finished worker still has all its payloads
    // available for capture below.
    let planning_deadline = Instant::now() + DEADLINE;
    while app
        .details_meta_pending
        .as_ref()
        .is_some_and(|pending| matches!(pending.phase, DetailsMetaPendingPhase::Planning(_)))
    {
        app.advance_details_meta_target_plan(ctx);
        assert!(Instant::now() < planning_deadline, "planner did not finish");
    }
    // Swap only the delivery receiver, retaining the real pending scope, captured
    // requirements, revision, cancellation token and worker priority channel.
    // This is a state-machine test: direct owner polling preserves the exact
    // prewarm range, which a complete grid frame would legitimately recalculate.
    let pending = app.details_meta_pending.as_mut().expect("real worker");
    let DetailsMetaPendingPhase::Loading { rx, .. } = &mut pending.phase else {
        unreachable!("planner was advanced to the real worker");
    };
    let (delivery, relay) = mpsc::channel();
    let worker = std::mem::replace(rx, relay);
    let cancel = Arc::clone(&pending.cancel);
    let scan = pending.listing_meta_scan;
    let deadline = Instant::now() + DEADLINE;
    let event = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = worker
            .recv_timeout(remaining)
            .expect("real worker Finished");
        if matches!(event, DetailsMetaEvent::Finished { .. }) {
            break event;
        }
        delivery.send(event).unwrap();
    };
    // Actual Item and Progress payloads are adopted normally; Finished remains
    // withheld so a cached Item cannot masquerade as whole-listing completion.
    app.poll_details_meta_load(ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &app.details_meta_pending.as_ref().unwrap().cancel,
        &cancel
    ));
    HeldFinished {
        delivery,
        event,
        cancel,
        scan,
    }
}

fn ai_checked(app: &App, idx: usize) -> bool {
    app.details_lazy_meta_for_idx(idx)
        .is_some_and(|meta| meta.ai_metadata_checked)
}

fn acquire_real_ai(app: &mut App, ctx: &egui::Context) {
    app.start_details_meta_load(ctx);
    let held = hold_real_finished(app, ctx);
    assert!(ai_checked(app, 0) && ai_checked(app, 1) && ai_checked(app, 2));
    assert!(!app.ai_model_facet_ready(), "Items do not prove completion");
    held.release(app, ctx);
    assert!(app.ai_model_facet_ready());
    assert_eq!(app.facet_ai_model_values(0), ["wanted"]);
    assert_eq!(app.facet_ai_model_values(2), ["other"]);
}

#[test]
fn facet_835_old_requirement_snapshot_finished_cannot_prove_new_ai() {
    let mut app = real_images();
    // Padding has no requested metadata fields and causes no disk access. It
    // keeps the replacement's real bounded planner incomplete in the release
    // poll, so a legitimate fast AI Finished cannot race the qualification
    // assertions. The old worker still reads only the three real PNGs.
    let mut items = app.items.clone();
    let mut metas = app.image_metas.clone();
    for idx in 0..DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME * 3 + 1 {
        items.push(GridItem::Folder(
            app.tmp.path().join(format!("memory-padding-{idx:05}")),
        ));
        metas.push(None);
    }
    app.install_new_items(items, metas);
    app.rebuild_visible_indices();
    app.details_tag_prewarm_indices = vec![0];
    let ctx = egui::Context::default();
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_image_dimensions = true;
    app.request_duration_facet_load();
    assert!((3..app.items.len()).all(|idx| !app.details_item_requires_lazy_meta(idx)));
    app.start_details_meta_load(&ctx);
    assert!(matches!(
        app.details_meta_pending.as_ref().unwrap().phase,
        DetailsMetaPendingPhase::Planning(_)
    ));
    let held = hold_real_finished(&mut app, &ctx);
    assert_eq!(held.scan.unwrap().requirements, LENGTHS);
    assert_eq!(
        app.details_lazy_meta_for_idx(0).unwrap().image_dims,
        Some((7, 5))
    );
    assert!(!ai_checked(&app, 0));
    let old_cancel = Arc::clone(&held.cancel);
    app.settings.facet_filter.ai_models.insert("wanted".into());
    held.release(&mut app, &ctx);
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(!app.ai_model_facet_ready());
    assert!(
        app.listing_meta_scan_proof
            .is_none_or(|proof| !proof.requirements.ai)
    );
    let replacement = app
        .details_meta_pending
        .as_ref()
        .expect("new requirement scan");
    let DetailsMetaPendingPhase::Planning(plan) = &replacement.phase else {
        panic!("replacement must remain inside its bounded planning budget");
    };
    assert!(!plan.order.is_complete());
    assert!(
        replacement
            .listing_meta_scan
            .unwrap()
            .requirements
            .contains(BOTH)
    );
    assert!(!Arc::ptr_eq(&old_cancel, &replacement.cancel));
    app.cancel_details_meta_loading();
}

#[test]
fn facet_835_adoption_discards_proof_and_rejects_old_real_finished() {
    let mut app = real_images();
    let ctx = egui::Context::default();
    app.request_ai_model_facet_load();
    acquire_real_ai(&mut app, &ctx);
    let previous = app.listing_meta_scan_proof.expect("actual AI proof");
    // Start another actual worker in the old listing, requesting real dimensions.
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_image_dimensions = true;
    app.invalidate_details_meta_requirements();
    app.start_details_meta_load(&ctx);
    let held = hold_real_finished(&mut app, &ctx);
    let old_owner = app.details_meta_pending.take().unwrap();
    let items = app.items.clone();
    let metas = app.image_metas.clone();
    app.install_new_items(items, metas);
    assert_ne!(app.items_generation, previous.generation);
    assert!(app.listing_meta_scan_proof.is_none());
    // Model the late delivery with its original owner, rather than manufacturing
    // a Finished generation or assigning a proof from the old listing.
    app.details_meta_pending = Some(old_owner);
    let old_cancel = Arc::clone(&held.cancel);
    held.release(&mut app, &ctx);
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(app.listing_meta_scan_proof.is_none());
    assert!(!app.ai_model_facet_ready());
    app.cancel_details_meta_loading();
}

#[test]
fn facet_835_empty_and_all_cache_completion_store_ai_in_integrated_proof() {
    let mut app = real_images();
    let ctx = egui::Context::default();
    app.request_ai_model_facet_load();
    acquire_real_ai(&mut app, &ctx);
    let items = app.items.clone();
    let metas = app.image_metas.clone();
    // Re-adoption clears the proof but the source-qualified metadata cache stays.
    app.install_new_items(items, metas);
    app.request_ai_model_facet_load();
    assert!(app.listing_meta_scan_proof.is_none());
    assert!(ai_checked(&app, 0) && ai_checked(&app, 1) && ai_checked(&app, 2));
    app.start_details_meta_load(&ctx);
    assert!(
        app.details_meta_pending.is_none(),
        "all-cache completion is immediate"
    );
    let proof = app
        .listing_meta_scan_proof
        .expect("all-cache integrated proof");
    assert_eq!(proof.requirements, AI);
    assert_eq!(proof.items_len, 3);
    assert!(proof.matches(&app));

    app.install_new_items(Vec::new(), Vec::new());
    app.request_ai_model_facet_load();
    assert!(app.listing_meta_scan_proof.is_none());
    app.start_details_meta_load(&ctx);
    assert!(
        app.details_meta_pending.is_none(),
        "empty completion is immediate"
    );
    let proof = app.listing_meta_scan_proof.expect("empty integrated proof");
    assert_eq!(proof.requirements, AI);
    assert_eq!(proof.items_len, 0);
    assert!(proof.matches(&app));
}

fn add_warm_zip_page_count(app: &mut App, zip_path: PathBuf) -> usize {
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
    let (warm_items, warm_metas) =
        tree.materialize_level(&[], crate::settings::SortOrder::FileName);
    assert_eq!(warm_items.len(), 1);
    let idx = app.items.len();
    let mut items = app.items.clone();
    items.extend(warm_items);
    let mut metas = app.image_metas.clone();
    metas.extend(warm_metas);
    app.install_new_items(items, metas);
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(tree));
    app.rebuild_visible_indices();
    app.details_tag_prewarm_indices = vec![0];
    idx
}

fn ai_with_staged_columns(details: bool) {
    let mut app = real_images();
    let page_idx = if details {
        let zip_path = app.tmp.path().join("memory-only.zip");
        Some(add_warm_zip_page_count(&mut app, zip_path))
    } else {
        None
    };
    let ctx = egui::Context::default();
    if details {
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_page_count = true;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
    } else {
        app.settings.thumb_show_media_duration = true;
    }
    app.request_ai_model_facet_load();
    app.start_details_meta_load(&ctx);
    assert_eq!(
        app.details_meta_pending.as_ref().unwrap().scan_scope,
        DetailsMetaScanScope::AllRequirements
    );
    let held = hold_real_finished(&mut app, &ctx);
    let scan = held.scan.expect("AI whole-listing snapshot");
    assert_eq!(scan.requirements, AI);
    assert!(ai_checked(&app, 0) && ai_checked(&app, 1) && ai_checked(&app, 2));
    assert!(app.listing_meta_scan_proof.is_none());
    assert!(!app.ai_model_facet_ready());
    let cancel = Arc::clone(&held.cancel);
    held.release(&mut app, &ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.details_meta_pending.is_none());
    assert_eq!(app.listing_meta_scan_proof, Some(scan));
    assert!(app.ai_model_facet_ready());
    app.reconcile_details_lazy_session_after_grid(&ctx);
    assert_eq!(app.listing_meta_scan_proof, Some(scan));
    assert!(app.details_image_dims_state.is_ready());

    if let Some(page_idx) = page_idx {
        // A real new visible range uses the warm ZipTree worker, after AI has
        // proved the listing. It must not be repeatedly canceled for lacking a
        // whole-listing snapshot of its own.
        app.details_tag_prewarm_indices = vec![page_idx];
        assert!(app.details_visible_stage_needs_load());
        app.reconcile_details_lazy_session_after_grid(&ctx);
        let pending = app
            .details_meta_pending
            .as_ref()
            .expect("actual visible stage");
        assert_eq!(pending.scan_scope, DetailsMetaScanScope::VisibleStage);
        assert!(pending.listing_meta_scan.is_none());
        let held = hold_real_finished(&mut app, &ctx);
        let cancel = Arc::clone(&held.cancel);
        assert!(held.scan.is_none());
        assert_eq!(app.listing_meta_scan_proof, Some(scan));
        held.release(&mut app, &ctx);
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(matches!(
            app.details_image_dims_state,
            LazyColumnState::Reconciling { .. }
        ));
        app.reconcile_details_lazy_session_after_grid(&ctx);
        assert!(app.details_image_dims_state.is_ready());
        assert!(app.details_meta_pending.is_none());
        assert_eq!(app.listing_meta_scan_proof, Some(scan));
        assert_eq!(
            app.details_lazy_meta_for_idx(page_idx).unwrap().page_count,
            Some(3)
        );
    }
}

#[test]
fn facet_835_ai_only_thumbnail_badge_real_finished_publishes_snapshot() {
    ai_with_staged_columns(false);
}

#[test]
fn facet_835_ai_only_page_count_real_finished_then_visible_stage_preserves_proof() {
    ai_with_staged_columns(true);
}

#[test]
fn facet_835_ai_requirement_removed_mid_plan_still_reads_uncached_later_slice() {
    let mut app = real_images();
    let ctx = egui::Context::default();
    app.settings.facet_filter.ai_models.insert("wanted".into());
    acquire_real_ai(&mut app, &ctx);
    assert!(!app.ai_model_facet_requested);

    // Keep the first slice's real, source-qualified AI cache across adoption,
    // but clear its proof. No requested flag may keep AI alive after removal.
    app.settings.facet_filter.ai_models.clear();
    let mut items = app.items.clone();
    let mut metas = app.image_metas.clone();
    for idx in 0..DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME * 3 + 1 {
        items.push(GridItem::Folder(
            app.tmp
                .path()
                .join(format!("uncached-slice-padding-{idx:05}")),
        ));
        metas.push(None);
    }
    let unread = app.tmp.path().join("later-slice.png");
    std::fs::write(&unread, png_bytes("later-model")).unwrap();
    let unread_idx = items.len();
    metas.push(crate::catalog::media_source_identity(&unread));
    items.push(GridItem::Image(unread));
    app.install_new_items(items, metas);
    app.rebuild_visible_indices();
    app.settings.thumb_show_media_duration = true;
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    assert!(app.listing_meta_scan_proof.is_none());
    assert!(ai_checked(&app, 0) && ai_checked(&app, 1) && ai_checked(&app, 2));
    assert!(!ai_checked(&app, unread_idx));
    assert!(!app.ai_model_facet_requested);
    app.settings.facet_filter.ai_models.insert("wanted".into());
    app.rebuild_visible_indices();
    assert!((3..unread_idx).all(|idx| !app.details_item_requires_lazy_meta(idx)));
    app.start_details_meta_load(&ctx);
    let pending = app
        .details_meta_pending
        .as_ref()
        .expect("first bounded slice");
    let scan = pending.listing_meta_scan.expect("captured AI requirement");
    assert_eq!(scan.requirements, AI);
    let cancel = Arc::clone(&pending.cancel);
    let DetailsMetaPendingPhase::Planning(plan) = &pending.phase else {
        panic!("unread PNG must be beyond the first bounded slice");
    };
    assert_eq!(
        plan.order.scanned(),
        DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME
    );
    assert_eq!(plan.cached_done, 3);
    assert!(plan.visible_targets.is_empty() && plan.background_targets.is_empty());
    let generation = app.items_generation;
    let revision = app.details_lazy_visible_revision;

    app.settings.facet_filter.ai_models.clear();
    app.rebuild_visible_indices();
    assert_eq!(app.details_lazy_visible_revision, revision);
    assert!(app.listing_meta_requirements().is_empty());
    assert!(app.unproven_listing_meta_requirements().is_empty());
    assert!(
        app.details_lazy_columns_visible(),
        "badge retains load admission"
    );
    app.poll_details_meta_load(&ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(!ai_checked(&app, unread_idx));
    assert!(app.listing_meta_scan_proof.is_none());
    let held = hold_real_finished(&mut app, &ctx);
    assert_eq!(held.scan, Some(scan));
    assert!(Arc::ptr_eq(&held.cancel, &cancel));
    assert!(
        ai_checked(&app, unread_idx),
        "captured AI must read the later PNG"
    );
    assert_eq!(app.facet_ai_model_values(unread_idx), ["later-model"]);
    assert!(
        app.listing_meta_scan_proof.is_none(),
        "Items alone are not proof"
    );
    held.release(&mut app, &ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.details_lazy_visible_revision, revision);
    assert_eq!(app.listing_meta_scan_proof, Some(scan));
    assert!(app.ai_model_facet_ready());
    assert!(app.details_meta_pending.is_none());
    assert!(app.details_image_dims_state.is_ready());
}

#[test]
fn facet_835_ai_requirement_removed_before_real_finished_keeps_snapshot_out_of_reconciling() {
    let mut app = real_images();
    let ctx = egui::Context::default();
    app.settings.facet_filter.ai_models.insert("wanted".into());
    app.rebuild_visible_indices();
    app.start_details_meta_load(&ctx);
    let held = hold_real_finished(&mut app, &ctx);
    let scan = held.scan.expect("whole-listing AI snapshot");
    assert_eq!(scan.requirements, AI);
    assert!(!app.ai_model_facet_requested);
    assert!(ai_checked(&app, 0) && ai_checked(&app, 1) && ai_checked(&app, 2));
    assert!(app.listing_meta_scan_proof.is_none());
    let generation = app.items_generation;
    let revision = app.details_lazy_visible_revision;
    let cancel = Arc::clone(&held.cancel);

    app.settings.facet_filter.ai_models.clear();
    app.settings.thumb_show_media_duration = true;
    app.rebuild_visible_indices();
    assert_eq!(app.details_lazy_visible_revision, revision);
    assert!(app.listing_meta_requirements().is_empty());
    assert!(app.unproven_listing_meta_requirements().is_empty());
    assert!(app.details_lazy_columns_visible());
    assert!(
        app.details_lazy_uses_visible_stages(),
        "ordinary badge work is now staged"
    );
    held.release(&mut app, &ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.details_lazy_visible_revision, revision);
    assert_eq!(app.listing_meta_scan_proof, Some(scan));
    assert!(app.ai_model_facet_ready());
    assert!(app.details_meta_pending.is_none());
    assert!(
        app.details_image_dims_state.is_ready(),
        "whole-listing Finished must publish its snapshot immediately"
    );
    app.reconcile_details_lazy_session_after_grid(&ctx);
    assert_eq!(app.listing_meta_scan_proof, Some(scan));
    assert!(app.details_image_dims_state.is_ready());
}

fn add_media(app: &mut AppTestEnv) {
    let mut items = app.items.clone();
    let mut metas = app.image_metas.clone();
    for name in ["short.wav", "long.wav"] {
        let path = app.tmp.path().join(name);
        std::fs::write(&path, [0; 64]).unwrap();
        metas.push(crate::catalog::media_source_identity(&path));
        items.push(GridItem::Audio(path));
    }
    app.install_new_items(items, metas);
    app.rebuild_visible_indices();
    app.details_tag_prewarm_indices = vec![0];
}

fn cache_lengths(app: &mut App) {
    // Source-qualified media cache is the established duration fixture pattern.
    // AI fields always come from the real PNG worker, never this cache seeding.
    for (idx, seconds) in [(3, 30.0), (4, 600.0)] {
        let key = app.details_lazy_cache_key(idx).unwrap();
        let (source_mtime, source_size) = app.image_metas[idx].unwrap();
        let meta = app.details_lazy_meta.entry(key).or_default();
        meta.source_mtime = source_mtime;
        meta.source_size = source_size;
        meta.media = DetailsMediaMeta::Read(DetailsVideoProbe {
            duration_secs: Some(seconds),
            dims: None,
            codec: None,
        });
    }
}

#[test]
fn facet_835_duration_proof_survives_ai_stop_resume_and_only_unproven_ai_is_ignored() {
    let mut app = real_images();
    add_media(&mut app);
    cache_lengths(&mut app);
    let ctx = egui::Context::default();
    app.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Under1Minute);
    app.start_details_meta_load(&ctx);
    assert!(app.details_meta_pending.is_none());
    let partial = app
        .listing_meta_scan_proof
        .expect("cached length completion");
    assert_eq!(partial.requirements, LENGTHS);
    assert_eq!(app.visible_indices, [3]);

    app.settings.facet_filter.ai_models.insert("wanted".into());
    app.rebuild_visible_indices();
    app.start_details_meta_load(&ctx);
    let held = hold_real_finished(&mut app, &ctx);
    assert_eq!(held.scan.unwrap().requirements, BOTH);
    assert!(ai_checked(&app, 0) && ai_checked(&app, 2));
    app.cancel_details_meta_loading();
    assert!(held.cancel.load(Ordering::Relaxed));
    drop(held); // The canceled worker's withheld Finished must not be published.
    for _ in 0..3 {
        app.poll_details_meta_load(&ctx);
        app.rebuild_visible_indices();
        assert_eq!(app.details_image_dims_state, LazyColumnState::Cancelled);
        assert!(app.details_meta_pending.is_none());
        assert_eq!(app.listing_meta_scan_proof, Some(partial));
        assert_eq!(app.unproven_listing_meta_requirements(), AI);
        assert_eq!(app.visible_indices, [3], "proven lengths stay applied");
        // Duration excludes all images. Its existing facet-candidate route
        // ignores that field and exposes whether unproven AI hides C early.
        assert!(app.passes_facet_filter(2, Some(FacetField::Duration)));
    }
    app.resume_details_meta_loading();
    app.poll_details_meta_load(&ctx);
    assert!(
        app.details_meta_pending.is_none(),
        "real AI Items are now cached"
    );
    assert!(app.details_image_dims_state.is_ready());
    let complete = app.listing_meta_scan_proof.unwrap();
    assert_eq!(complete.requirements, BOTH);
    assert_eq!(complete.generation, partial.generation);
    assert_eq!(complete.items_len, partial.items_len);
    assert!(app.unproven_listing_meta_requirements().is_empty());
    assert!(
        app.visible_indices.is_empty(),
        "proven AI excludes audio; proven lengths exclude images"
    );
    assert!(app.passes_facet_filter(3, Some(FacetField::AiModel)));
    assert!(!app.passes_facet_filter(4, Some(FacetField::AiModel)));
    assert!(app.passes_facet_filter(0, Some(FacetField::Duration)));
    assert!(!app.passes_facet_filter(2, Some(FacetField::Duration)));
}

#[test]
fn facet_835_ai_proof_survives_duration_stop_resume_and_only_unproven_lengths_are_ignored() {
    let mut app = real_images();
    add_media(&mut app);
    let ctx = egui::Context::default();
    app.settings.facet_filter.ai_models.insert("wanted".into());
    acquire_real_ai(&mut app, &ctx);
    let partial = app.listing_meta_scan_proof.expect("real AI completion");
    assert_eq!(partial.requirements, AI);
    assert!(
        !app.ai_model_facet_requested,
        "direct application did not open a menu"
    );
    assert_eq!(app.visible_indices, [0, 1]);

    app.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Under1Minute);
    app.rebuild_visible_indices();
    app.start_details_meta_load(&ctx);
    let pending = app
        .details_meta_pending
        .as_ref()
        .expect("unread media worker");
    assert_eq!(pending.listing_meta_scan.unwrap().requirements, BOTH);
    let cancel = Arc::clone(&pending.cancel);
    app.cancel_details_meta_loading();
    assert!(cancel.load(Ordering::Relaxed));
    for _ in 0..3 {
        app.poll_details_meta_load(&ctx);
        app.rebuild_visible_indices();
        assert_eq!(app.details_image_dims_state, LazyColumnState::Cancelled);
        assert!(app.details_meta_pending.is_none());
        assert_eq!(app.listing_meta_scan_proof, Some(partial));
        assert_eq!(app.unproven_listing_meta_requirements(), LENGTHS);
        assert_eq!(
            app.visible_indices,
            [0, 1],
            "proven AI excludes C and audio; unproved lengths do not hide matching images"
        );
        assert!(app.passes_facet_filter(3, Some(FacetField::AiModel)));
        assert!(app.passes_facet_filter(4, Some(FacetField::AiModel)));
    }
    cache_lengths(&mut app);
    app.resume_details_meta_loading();
    app.poll_details_meta_load(&ctx);
    assert!(app.details_meta_pending.is_none());
    assert!(app.details_image_dims_state.is_ready());
    let complete = app.listing_meta_scan_proof.unwrap();
    assert_eq!(complete.requirements, BOTH);
    assert_eq!(complete.generation, partial.generation);
    assert_eq!(complete.items_len, partial.items_len);
    assert!(app.unproven_listing_meta_requirements().is_empty());
    assert!(
        app.visible_indices.is_empty(),
        "proven AI excludes audio; newly proven lengths exclude images"
    );
    assert!(app.passes_facet_filter(3, Some(FacetField::AiModel)));
    assert!(!app.passes_facet_filter(4, Some(FacetField::AiModel)));
    assert!(app.passes_facet_filter(0, Some(FacetField::Duration)));
    assert!(!app.passes_facet_filter(2, Some(FacetField::Duration)));
}
