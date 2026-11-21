use super::*;

fn checked(h: &Harness, name: &str) -> bool {
    h.env
        .details_lazy_meta_for_idx(h.idx(name))
        .is_some_and(|meta| meta.ai_metadata_checked)
}

fn acquire_ai(h: &mut Harness) {
    h.env.request_ai_model_facet_load();
    h.until(|h| h.env.ai_model_facet_ready() && h.idle(), |_| {});
    assert!(checked(h, "A.png"), "fixture must really read PNG metadata");
    assert_eq!(h.env.facet_ai_model_values(h.idx("A.png")), ["wanted"]);
    assert!(!h.env.facet_ai_tool_value(h.idx("A.png")).is_empty());
}

fn choose_ai(h: &mut Harness) {
    let tool = h.env.facet_ai_tool_value(h.idx("A.png"));
    h.env
        .settings
        .facet_filter
        .ai_models
        .insert("wanted".into());
    h.env.settings.facet_filter.ai_tools.insert(tool);
    h.env.rebuild_visible_indices();
}

#[derive(Clone, Copy)]
enum Exclusion {
    Size,
    Rating,
}

struct Relaxation {
    final_b: bool,
    hidden_frames: Vec<String>,
    generations: Vec<u64>,
    initial_generation: u64,
    b_checked_before: bool,
}

fn relax(exclusion: Exclusion) -> Relaxation {
    let (mut h, _) = image_fixture();
    h.env.settings.write_rating_to_xmp = false;
    match exclusion {
        Exclusion::Size => {
            h.env.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB)
        }
        Exclusion::Rating => {
            for name in ["A.png", "C.png"] {
                let idx = h.idx(name);
                assert!(h.env.set_rating(idx, 5));
            }
            h.env.settings.rating_filter = [false, false, false, false, false, true];
        }
    }
    h.env.rebuild_visible_indices();
    h.settle();
    assert_eq!(h.visible_names(), ["A.png", "C.png"]);
    acquire_ai(&mut h);
    choose_ai(&mut h);
    h.settle();
    assert_eq!(h.visible_names(), ["A.png"]);
    let initial_generation = h.env.items_generation;
    let b_checked_before = checked(&h, "B.png");
    match exclusion {
        Exclusion::Size => h.env.settings.facet_filter.size_preset = None,
        Exclusion::Rating => h.env.settings.rating_filter = [true; 6],
    }
    h.stage = match exclusion {
        Exclusion::Size => "T4: relax size",
        Exclusion::Rating => "T5: relax rating",
    }
    .into();
    h.env.rebuild_visible_indices();
    let mut hidden_frames = Vec::new();
    let mut generations = Vec::new();
    let mut observe = |h: &Harness| {
        generations.push(h.env.items_generation);
        if !h.visible_names().iter().any(|name| name == "B.png") {
            hidden_frames.push(h.diagnostic());
        }
    };
    // The synchronous projection in the input handler is also an observation:
    // a later poll must not hide a momentary omission by repairing it afterwards.
    observe(&h);
    h.until(|h| h.idle() && h.env.ai_model_facet_ready(), &mut observe);
    let final_b = h.visible_names().iter().any(|name| name == "B.png");
    eprintln!(
        "{}: B checked before relaxation={b_checked_before}, generations={generations:?}, missing={hidden_frames:?}",
        h.stage
    );
    Relaxation {
        final_b,
        hidden_frames,
        generations,
        initial_generation,
        b_checked_before,
    }
}

#[test]
fn facet_835_t4_size_relaxation_final_result_and_no_transient_omission() {
    let result = relax(Exclusion::Size);
    // Keep the final-result assertion independent of the no-reload assertion.
    assert!(
        result.final_b,
        "T4 final result: matching B must be visible"
    );
    assert!(
        result.hidden_frames.is_empty(),
        "T4 B disappeared while acquiring metadata (previously checked={}): {:?}",
        result.b_checked_before,
        result.hidden_frames
    );
}

#[test]
fn facet_835_t4_size_relaxation_does_not_reload_listing() {
    let result = relax(Exclusion::Size);
    assert!(
        result
            .generations
            .iter()
            .all(|generation| *generation == result.initial_generation),
        "T4 no reload: {} -> {:?}",
        result.initial_generation,
        result.generations
    );
}

#[test]
fn facet_835_t5_rating_relaxation_keeps_matching_image_without_reload() {
    let result = relax(Exclusion::Rating);
    assert!(
        result.final_b,
        "T5 final result: matching B must be visible"
    );
    assert!(
        result.hidden_frames.is_empty(),
        "T5 B disappeared: {:?}",
        result.hidden_frames
    );
    assert!(
        result
            .generations
            .iter()
            .all(|generation| *generation == result.initial_generation),
        "T5 unexpectedly reloaded"
    );
}

#[test]
fn facet_835_t6_restore_ai_after_child_dimensions_does_not_hide_unread_images() {
    let (mut h, parent) = image_fixture();
    acquire_ai(&mut h);
    choose_ai(&mut h);
    h.settle();
    let filter = h.env.settings.facet_filter.clone();
    let child = parent.join("child");
    std::fs::create_dir(&child).unwrap();
    std::fs::write(child.join("A.png"), png_bytes("wanted")).unwrap();
    std::fs::write(child.join("B.png"), png_bytes("wanted")).unwrap();
    std::fs::write(child.join("C.png"), png_bytes("other")).unwrap();
    // Use the production navigation owner; it creates the saved facet frame.
    assert!(h.env.start_physical_history_transition(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false
        },
        child.clone(),
        StartupListIntent::ExplicitList
    ));
    h.adopt(&child, 3);
    assert!(h.env.facet_filter_suppressed());
    assert!(!h.env.facet_ai_filter_active());
    assert!(!h.env.ai_model_facet_requested);
    h.env.settings.grid_view_mode = GridViewMode::Details;
    h.env.settings.details_show_image_dimensions = true;
    h.env.invalidate_details_meta_requirements();
    h.until(
        |h| h.env.details_image_dims_state.is_ready() && h.idle(),
        |_| {},
    );
    for name in ["A.png", "B.png", "C.png"] {
        let meta = h
            .env
            .details_lazy_meta_for_idx(h.idx(name))
            .expect("real dimension acquisition");
        assert_eq!(meta.image_dims, Some((7, 5)));
        assert!(
            !meta.ai_metadata_checked,
            "dimension completion must not fake AI completion"
        );
    }
    let generation = h.env.items_generation;
    h.stage = "T6: restore AI from suppression".into();
    assert!(h.env.restore_facet_filter_suppression());
    // The badge's caller rebuilds immediately after restore (the helper itself
    // only restores settings/name state). Exercise that same ownership boundary.
    h.env.rebuild_visible_indices();
    assert_eq!(h.env.settings.facet_filter, filter);
    assert_eq!(
        h.visible_names(),
        ["A.png", "B.png", "C.png"],
        "unproved AI must not filter synchronously"
    );
    h.until(
        |h| h.idle() && checked(h, "A.png") && checked(h, "B.png") && checked(h, "C.png"),
        |h| {
            assert!(
                h.visible_names().iter().any(|name| name == "B.png"),
                "T6 unread/matching B disappeared: {}",
                h.diagnostic()
            );
            assert_eq!(
                h.env.items_generation,
                generation,
                "T6 restore reloaded: {}",
                h.diagnostic()
            );
        },
    );
    assert_eq!(
        h.visible_names(),
        ["A.png", "B.png"],
        "completed AI filter must exclude C"
    );
}

#[test]
fn facet_835_duration_completion_cannot_stand_in_for_ai_completion() {
    let (mut h, _) = image_fixture();
    h.env.request_duration_facet_load();
    h.until(|h| h.env.duration_scan_ready() && h.idle(), |_| {});
    assert!(!checked(&h, "B.png"));
    h.env
        .settings
        .facet_filter
        .ai_models
        .insert("wanted".into());
    h.env.rebuild_visible_indices();
    assert_eq!(
        h.visible_names(),
        ["A.png", "B.png", "C.png"],
        "duration-only completion does not prove AI"
    );
    h.until(
        |h| h.idle() && checked(h, "B.png"),
        |h| {
            assert!(h.env.duration_scan_ready(), "adding AI lost proven lengths");
            assert!(h.visible_names().iter().any(|name| name == "B.png"));
        },
    );
    assert_eq!(h.visible_names(), ["A.png", "B.png"]);
}

fn planning_fixture(ai: bool) -> Harness {
    let mut env = setup_app();
    configure(&mut env);
    let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME * 8 + 1;
    let items = (0..count)
        .map(|idx| {
            if idx == 2 {
                GridItem::Video(env.tmp.path().join("hidden.mp4"))
            } else {
                GridItem::Image(env.tmp.path().join(format!("{idx:05}.png")))
            }
        })
        .collect();
    env.install_new_items(items, vec![Some((100, 2048)); count]);
    env.visible_indices = (0..count).filter(|idx| *idx != 1 && *idx != 2).collect();
    env.search_filter = Some(
        env.visible_indices
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into(),
    );
    env.details_tag_prewarm_indices = vec![0];
    env.selected = None;
    if ai {
        env.request_ai_model_facet_load();
    } else {
        env.request_duration_facet_load();
    }
    let mut h = Harness::new(env);
    // Large enough that one real frame cannot finish planning or touch disk.
    h.tick(Vec::new());
    assert!(matches!(
        h.env.details_meta_pending.as_ref().unwrap().phase,
        DetailsMetaPendingPhase::Planning(_)
    ));
    h
}

#[test]
fn facet_835_ai_becoming_required_restarts_inflight_duration_scan() {
    let mut h = planning_fixture(false);
    let cancel = Arc::clone(&h.env.details_meta_pending.as_ref().unwrap().cancel);
    // Restoration/direct application does not call request_ai_model_facet_load.
    h.env
        .settings
        .facet_filter
        .ai_models
        .insert("wanted".into());
    h.tick(Vec::new());
    assert!(
        cancel.load(Ordering::Relaxed),
        "poll owner must retire a snapshot missing AI"
    );
    let replacement = h
        .env
        .details_meta_pending
        .as_ref()
        .expect("replacement whole-listing scan");
    assert!(!Arc::ptr_eq(&cancel, &replacement.cancel));
    h.env.cancel_details_meta_loading();
}

#[test]
fn facet_835_ai_only_plan_reads_hidden_images_without_unrelated_fields() {
    let mut h = planning_fixture(true);
    let DetailsMetaPendingPhase::Planning(plan) =
        &h.env.details_meta_pending.as_ref().unwrap().phase
    else {
        unreachable!()
    };
    assert!(
        matches!(plan.order, DetailsMetaScanOrder::WholeListing { .. }),
        "AI's universe is items, not current_grid_order"
    );
    let hidden = plan
        .background_targets
        .iter()
        .chain(&plan.visible_targets)
        .find(|target| target.idx == 1)
        .expect("hidden image must have an AI target");
    assert!(hidden.load_ai_metadata);
    assert!(
        !hidden.load_video_meta
            && !hidden.load_created_at
            && !hidden.load_image_dims
            && !hidden.load_page_count
    );
    assert!(
        !plan
            .background_targets
            .iter()
            .chain(&plan.visible_targets)
            .any(|target| target.idx == 2),
        "AI-only requirement must not probe a hidden video"
    );
    h.env.cancel_details_meta_loading();
}

#[test]
fn facet_835_ai_cached_completion_and_empty_listing_are_ready() {
    let (mut h, _) = image_fixture();
    acquire_ai(&mut h);
    assert!(checked(&h, "B.png") && checked(&h, "C.png"));
    h.env.invalidate_details_meta_requirements();
    h.env.start_details_meta_load(&h.ctx);
    assert!(
        h.env.details_meta_pending.is_none(),
        "all-cache plan must complete immediately"
    );
    assert!(h.env.ai_model_facet_ready());
    h.env.install_new_items(Vec::new(), Vec::new());
    h.env.request_ai_model_facet_load();
    assert!(
        !h.env.ai_model_facet_ready(),
        "adoption invalidates old AI completion"
    );
    h.env.start_details_meta_load(&h.ctx);
    assert!(h.env.details_meta_pending.is_none());
    assert!(
        h.env.ai_model_facet_ready(),
        "empty whole-listing acquisition completes immediately"
    );
}

#[test]
fn facet_835_ai_menus_preserve_stopped_unproven_duration_after_direct_ai_acquisition() {
    // A separate fixture per menu prevents the first menu from setting requested
    // and thereby masking the second menu's first-request invalidation.
    for menu in ["AIモデル", "生成ツール"] {
        // An in-memory listing of REAL files isolates this first-menu request
        // contract from T1's watcher key defect. A physical reload would set
        // requested=true in install_new_items and destroy the precondition.
        let mut env = setup_app();
        configure(&mut env);
        let folder = env.tmp.path().join("first-ai-menu");
        std::fs::create_dir(&folder).unwrap();
        let mut items = Vec::new();
        let mut metas = Vec::new();
        for (name, model) in [("A.png", "wanted"), ("B.png", "wanted"), ("C.png", "other")] {
            let path = folder.join(name);
            std::fs::write(&path, png_bytes(model)).unwrap();
            metas.push(crate::catalog::media_source_identity(&path));
            items.push(GridItem::Image(path));
        }
        let clip = folder.join("clip.wav");
        std::fs::write(&clip, [0; 64]).unwrap();
        metas.push(crate::catalog::media_source_identity(&clip));
        items.push(GridItem::Audio(clip));
        env.install_new_items(items, metas);
        env.rebuild_visible_indices();
        let mut h = Harness::new(env);
        h.env
            .settings
            .facet_filter
            .ai_models
            .insert("wanted".into());
        h.env.rebuild_visible_indices();
        h.until(
            |h| {
                h.env.details_meta_pending.is_none()
                    && h.env.details_image_dims_state.is_ready()
                    && checked(h, "A.png")
                    && checked(h, "B.png")
                    && checked(h, "C.png")
            },
            |_| {},
        );
        h.tick(Vec::new()); // Observe responses for the first frame's menu ids.
        assert!(
            !h.env.ai_model_facet_requested,
            "acquisition must not pretend the menu was opened"
        );
        assert!(h.env.ai_model_facet_ready());
        h.env.request_duration_facet_load();
        h.env.start_details_meta_load(&h.ctx);
        assert!(
            h.env.details_meta_pending.is_some(),
            "real unread media starts a worker"
        );
        assert!(!h.env.duration_scan_ready());
        h.env.cancel_details_meta_loading();
        assert_eq!(h.env.details_image_dims_state, LazyColumnState::Cancelled);
        let generation = h.env.items_generation;
        h.stage = format!("stopped lengths: first open of {menu}");
        h.open_menu(menu);
        assert_eq!(
            h.env.details_image_dims_state,
            LazyColumnState::Cancelled,
            "{}",
            h.diagnostic()
        );
        assert!(h.env.details_meta_pending.is_none(), "{}", h.diagnostic());
        assert!(!h.env.duration_scan_ready());
        assert!(
            h.env.ai_model_facet_ready(),
            "stopping lengths must retain proven AI"
        );
        assert_eq!(
            h.visible_names(),
            ["A.png", "B.png"],
            "proven AI excludes non-AI media even while lengths are unproved"
        );
        assert_eq!(h.env.items_generation, generation);
    }
}

#[test]
fn facet_835_duration_only_hidden_targets_do_not_acquire_ai_or_other_columns() {
    let mut env = setup_media_duration_thumbnail_app();
    configure(&mut env);
    env.duration_facet_requested = true;
    env.visible_indices = vec![0];
    env.details_tag_prewarm_indices = vec![0];
    let near = HashSet::from([0]);
    for idx in [1, 4] {
        let target = env
            .details_meta_target_for_idx_with_scope(idx, &near, false, true)
            .expect("hidden media needs length");
        assert!(target.load_video_meta);
        assert!(
            !target.load_ai_metadata
                && !target.load_created_at
                && !target.load_image_dims
                && !target.load_page_count
        );
    }
    for idx in [2, 3] {
        assert!(
            env.details_meta_target_for_idx_with_scope(idx, &near, false, true)
                .is_none()
        );
    }
}

#[test]
fn facet_835_stopping_and_resuming_ai_keeps_proven_duration_filter() {
    let mut env = setup_media_duration_thumbnail_app();
    configure(&mut env);
    let image = env.tmp.path().join("hidden-ai.png");
    std::fs::write(&image, png_bytes("wanted")).unwrap();
    let source = crate::catalog::media_source_identity(&image).expect("real PNG stamp");
    let mut items = env.items.clone();
    items[2] = GridItem::Image(image);
    let mut metas = env.image_metas.clone();
    metas[2] = Some(source);
    env.install_new_items(items, metas);
    for (idx, seconds) in [(0, 30.0), (1, 600.0), (4, 90.0)] {
        let key = env.details_lazy_cache_key(idx).unwrap();
        let (source_mtime, source_size) = env.image_metas[idx].unwrap();
        env.details_lazy_meta.insert(
            key,
            DetailsLazyMeta {
                source_mtime,
                source_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(seconds),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
    }
    env.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Under1Minute);
    let mut h = Harness::new(env);
    h.until(
        |h| h.env.duration_scan_ready() && h.env.details_meta_pending.is_none(),
        |_| {},
    );
    assert_eq!(h.env.visible_indices, [0]);
    assert!(
        !h.env
            .details_lazy_meta_for_idx(2)
            .is_some_and(|meta| meta.ai_metadata_checked)
    );
    h.env.request_ai_model_facet_load();
    h.env.start_details_meta_load(&h.ctx);
    assert!(
        h.env.details_meta_pending.is_some(),
        "new AI requirement must acquire the duration-hidden image"
    );
    h.env.cancel_details_meta_loading();
    h.env.rebuild_visible_indices();
    h.tick(Vec::new());
    assert_eq!(h.env.details_image_dims_state, LazyColumnState::Cancelled);
    assert!(
        h.env.duration_scan_ready(),
        "stopping AI must retain already proven lengths"
    );
    assert_eq!(
        h.env.visible_indices,
        [0],
        "length filter remains applied while AI is stopped"
    );
    h.env.resume_details_meta_loading();
    h.until(
        |h| {
            h.env.details_meta_pending.is_none()
                && h.env
                    .details_lazy_meta_for_idx(2)
                    .is_some_and(|meta| meta.ai_metadata_checked)
        },
        |h| {
            assert!(h.env.duration_scan_ready());
            assert_eq!(h.env.visible_indices, [0]);
        },
    );
    assert_eq!(h.env.facet_ai_model_values(2), ["wanted"]);
}
