//! §4.2.2-10: whole-listing proof does not prove hidden ordinary columns.
use super::*;

#[derive(Clone, Copy)]
enum Restriction {
    Size,
    Rating,
    Name,
    None,
}

fn png_with_dimensions(model: &str, width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_text_chunk(
            "parameters".into(),
            format!("cat\nSteps: 25, Sampler: Euler, CFG scale: 7, Seed: 42, Size: {width}x{height}, Model: {model}"),
        )
        .unwrap();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&vec![0; (width * height * 3) as usize])
        .unwrap();
    bytes
}

fn session_starts() -> usize {
    let jobs = probe::jobs();
    let starts: Vec<_> = jobs
        .iter()
        .filter(|job| job.field == "details-meta-session" && job.outcome == probe::Outcome::Started)
        .collect();
    assert!(starts.iter().all(|job| job.query == "all"));
    starts.len()
}

struct Fixture {
    env: AppTestEnv,
    ctx: egui::Context,
    restriction: Restriction,
    generation: u64,
    _observations: probe::Session,
}

impl Fixture {
    fn new(restriction: Restriction, columns_from_start: bool) -> Self {
        // Build the environment before opening the opt-in observation session.
        let mut env = setup_app();
        configure(&mut env);
        env.settings.write_rating_to_xmp = false;
        let mut items = Vec::new();
        let mut metas = Vec::new();
        for (name, model, dims) in [
            ("A.png", "wanted", (7, 5)),
            ("B.png", "wanted", (13, 11)),
            ("C.png", "other", (17, 13)),
        ] {
            let path = env.tmp.path().join(name);
            std::fs::write(&path, png_with_dimensions(model, dims.0, dims.1)).unwrap();
            if name == "B.png" {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len(2 * 1024 * 1024)
                    .unwrap();
            }
            metas.push(crate::catalog::media_source_identity(&path));
            items.push(GridItem::Image(path));
        }
        env.install_new_items(items, metas);
        env.selected = None;
        let observations = probe::Session::new();
        let ctx = egui::Context::default();
        env.settings.facet_filter.ai_models.insert("wanted".into());
        match restriction {
            Restriction::Size => {
                env.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB);
            }
            Restriction::Rating => {
                assert!(env.set_rating(0, 5));
                assert!(env.set_rating(2, 5));
                env.settings.rating_filter = [false, false, false, false, false, true];
            }
            Restriction::Name => {
                env.settings.facet_filter.name_query = "A.png".into();
                env.sync_facet_name_runtime_from_filter();
                let deadline = Instant::now() + DEADLINE;
                while env.facet_name_cache_generation != Some(env.items_generation) {
                    env.poll_facet_name_cache(&ctx);
                    assert!(Instant::now() < deadline, "real name cache did not finish");
                    std::thread::yield_now();
                }
            }
            Restriction::None => {}
        }
        env.rebuild_visible_indices();
        env.details_tag_prewarm_indices = vec![0];
        env.keep_set = HashSet::from([0]);
        if columns_from_start {
            Self::configure_columns(&mut env);
        }
        let generation = env.items_generation;
        let mut fixture = Self {
            env,
            ctx,
            restriction,
            generation,
            _observations: observations,
        };
        fixture.env.start_details_meta_load(&fixture.ctx);
        fixture.ready();
        assert_eq!(session_starts(), 1);
        assert!(fixture.env.ai_model_facet_ready());
        assert!((0..3).all(|idx| fixture.meta(idx).ai_metadata_checked));
        assert_eq!(fixture.env.facet_ai_model_values(1), ["wanted"]);
        match restriction {
            Restriction::None => assert_eq!(fixture.env.visible_indices, [0, 1]),
            _ => assert_eq!(fixture.env.visible_indices, [0]),
        }
        if !matches!(restriction, Restriction::None) {
            fixture.assert_b_ai_only();
        }
        fixture
    }

    fn configure_columns(app: &mut App) {
        app.settings.grid_view_mode = GridViewMode::Details;
        app.settings.details_show_image_dimensions = true;
        app.settings.details_show_created = true;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::ImageDimensions;
        app.settings.details_sort_ascending = false;
    }

    fn begin_columns(&mut self) {
        Self::configure_columns(&mut self.env);
        self.env.invalidate_details_meta_requirements();
        self.env.start_details_meta_load(&self.ctx);
        assert_eq!(session_starts(), 2);
    }

    fn acquire_visible_columns(&mut self) {
        self.begin_columns();
        self.ready();
        self.assert_columns(0, (7, 5));
        if !matches!(self.restriction, Restriction::None) {
            self.assert_b_ai_only();
            assert_eq!(self.env.details_order, [0]);
        }
    }

    fn meta(&self, idx: usize) -> &DetailsLazyMeta {
        self.env
            .details_lazy_meta_for_idx(idx)
            .expect("actual worker metadata")
    }

    fn assert_columns(&self, idx: usize, dims: (u32, u32)) {
        let meta = self.meta(idx);
        assert_eq!(meta.image_dims, Some(dims));
        assert!(!meta.image_dims_failed);
        assert!(meta.created_at.is_some(), "real Windows creation time");
        assert!(!meta.created_at_failed);
        assert!(meta.ai_metadata_checked);
    }

    fn assert_b_ai_only(&self) {
        let meta = self.meta(1);
        assert!(meta.ai_metadata_checked);
        assert!(meta.image_dims.is_none() && !meta.image_dims_failed);
        assert!(meta.created_at.is_none() && !meta.created_at_failed);
    }

    fn ready(&mut self) {
        // Owner-level polling fixes the exact visible/prewarm set. It exercises
        // the real planner, worker, Finished adoption, projection and detail-sort
        // owners without a grid frame changing that set during delivery capture.
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.env.poll_details_meta_load(&self.ctx);
            assert_eq!(self.env.items_generation, self.generation);
            if self.env.details_meta_pending.is_none()
                && self.env.details_image_dims_state.is_ready()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "metadata owner did not reach Ready"
            );
            std::thread::yield_now();
        }
    }

    fn expand(&mut self) {
        match self.restriction {
            Restriction::Size => self.env.settings.facet_filter.size_preset = None,
            Restriction::Rating => self.env.settings.rating_filter = [true; 6],
            Restriction::Name => assert!(self.env.clear_facet_name_filter_state()),
            Restriction::None => panic!("already unrestricted"),
        }
        self.env.rebuild_visible_indices();
        assert_eq!(self.env.visible_indices, [0, 1]);
        assert_eq!(self.env.items_generation, self.generation);
    }

    fn assert_stable(&mut self, expected_starts: usize) {
        for _ in 0..6 {
            self.env.rebuild_visible_indices();
            self.env.poll_details_meta_load(&self.ctx);
            assert!(self.env.details_image_dims_state.is_ready());
            assert!(self.env.details_meta_pending.is_none());
            assert_eq!(session_starts(), expected_starts, "no new accepted session");
            assert_eq!(self.env.items_generation, self.generation);
        }
    }
}

fn expansion_reads_columns(restriction: Restriction) {
    let mut fixture = Fixture::new(restriction, false);
    fixture.acquire_visible_columns();
    let proof = fixture.env.listing_meta_scan_proof;
    fixture.expand();
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
    assert_eq!(
        fixture.env.details_image_dims_state,
        LazyColumnState::NotRequested
    );
    assert_eq!(
        session_starts(),
        2,
        "projection schedules through the existing poll owner"
    );
    fixture.ready();
    fixture.assert_columns(1, (13, 11));
    assert_eq!(fixture.env.details_order, [1, 0], "larger B sorts before A");
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
    assert_eq!(session_starts(), 3);
    fixture.assert_stable(3);
}

#[test]
fn facet_835_size_expansion_acquires_hidden_columns_and_sorts_without_reload() {
    expansion_reads_columns(Restriction::Size);
}

#[test]
fn facet_835_rating_expansion_acquires_hidden_columns_and_sorts_without_reload() {
    expansion_reads_columns(Restriction::Rating);
}

#[test]
fn facet_835_name_clear_expansion_acquires_hidden_columns_through_common_owner() {
    expansion_reads_columns(Restriction::Name);
}

#[test]
fn facet_835_ordinary_columns_resume_after_cancelled_rating_or_size_projection_change() {
    for restriction in [Restriction::Rating, Restriction::Size] {
        let mut env = setup_app();
        configure(&mut env);
        env.settings.write_rating_to_xmp = false;
        env.settings.grid_view_mode = GridViewMode::Details;
        env.settings.details_show_image_dimensions = true;
        env.settings.details_sort_key = crate::settings::DetailsSortKey::ImageDimensions;
        env.settings.details_sort_ascending = false;
        let mut items = Vec::new();
        let mut metas = Vec::new();
        for (name, dims) in [("A.png", (7, 5)), ("B.png", (13, 11))] {
            let path = env.tmp.path().join(name);
            std::fs::write(&path, png_with_dimensions("wanted", dims.0, dims.1)).unwrap();
            if name == "B.png" {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len(2 * 1024 * 1024)
                    .unwrap();
            }
            metas.push(crate::catalog::media_source_identity(&path));
            items.push(GridItem::Image(path));
        }
        env.install_new_items(items, metas);
        match restriction {
            Restriction::Rating => {
                assert!(env.set_rating(0, 5));
                env.settings.rating_filter = [false, false, false, false, false, true];
            }
            Restriction::Size => {
                env.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB);
            }
            _ => unreachable!(),
        }
        env.rebuild_visible_indices();
        env.selected = None;
        env.details_tag_prewarm_indices = vec![0];
        let generation = env.items_generation;
        let ctx = egui::Context::default();
        let _observations = probe::Session::new();
        assert_eq!(env.visible_indices, [0]);
        assert!(!env.whole_listing_meta_scan_wanted());
        assert!(env.listing_meta_requirements().is_empty());
        assert!(env.listing_meta_scan_proof.is_none());

        env.start_details_meta_load(&ctx);
        let deadline = Instant::now() + DEADLINE;
        while env
            .details_meta_pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.phase, DetailsMetaPendingPhase::Planning(_)))
        {
            env.advance_details_meta_target_plan(&ctx);
            assert!(Instant::now() < deadline, "ordinary column planner stalled");
        }
        let pending = env
            .details_meta_pending
            .as_ref()
            .expect("real dimensions worker");
        assert!(matches!(
            pending.phase,
            DetailsMetaPendingPhase::Loading { .. }
        ));
        assert!(pending.listing_meta_scan.is_none());
        let cancel = Arc::clone(&pending.cancel);
        assert_eq!(session_starts(), 1);
        // Do not poll the receiver: even a fast worker's results remain unread,
        // so stopping cannot accidentally leave the resumed targets cached.
        env.cancel_details_meta_loading();
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(env.details_image_dims_state, LazyColumnState::Cancelled);
        assert!(env.details_meta_pending.is_none());
        assert!(env.details_lazy_meta_for_idx(0).is_none());

        match restriction {
            Restriction::Rating => env.settings.rating_filter = [true; 6],
            Restriction::Size => env.settings.facet_filter.size_preset = None,
            _ => unreachable!(),
        }
        env.rebuild_visible_indices();
        assert_eq!(env.visible_indices, [0, 1]);
        assert_eq!(env.details_image_dims_state, LazyColumnState::NotRequested);
        assert!(env.details_meta_pending.is_none());
        assert_eq!(session_starts(), 1, "rebuild leaves starting to poll");

        let deadline = Instant::now() + DEADLINE;
        loop {
            env.poll_details_meta_load(&ctx);
            assert_eq!(env.items_generation, generation);
            assert!(!env.whole_listing_meta_scan_wanted());
            assert!(env.listing_meta_scan_proof.is_none());
            if env.details_meta_pending.is_none() && env.details_image_dims_state.is_ready() {
                break;
            }
            assert!(Instant::now() < deadline, "ordinary columns did not resume");
            std::thread::yield_now();
        }
        assert_eq!(
            session_starts(),
            2,
            "poll automatically resumed ordinary columns"
        );
        for (idx, dims) in [(0, (7, 5)), (1, (13, 11))] {
            let meta = env.details_lazy_meta_for_idx(idx).expect("real dimensions");
            assert_eq!(meta.image_dims, Some(dims));
            assert!(!meta.image_dims_failed);
            assert!(
                !meta.ai_metadata_checked,
                "ordinary columns did not acquire AI"
            );
        }
        assert_eq!(
            env.details_order,
            [1, 0],
            "real dimensions determine sorting"
        );
    }
}

#[test]
fn facet_835_cancelled_expansion_keeps_proof_and_waits_for_explicit_resume() {
    let mut fixture = Fixture::new(Restriction::Size, false);
    fixture.acquire_visible_columns();
    let proof = fixture.env.listing_meta_scan_proof;
    fixture.env.cancel_details_meta_loading();
    fixture.expand();
    for _ in 0..4 {
        fixture.env.poll_details_meta_load(&fixture.ctx);
        fixture.env.rebuild_visible_indices();
        assert_eq!(
            fixture.env.details_image_dims_state,
            LazyColumnState::Cancelled
        );
        assert!(fixture.env.details_meta_pending.is_none());
        assert_eq!(fixture.env.listing_meta_scan_proof, proof);
        assert_eq!(
            fixture.env.visible_indices,
            [0, 1],
            "proven AI still excludes C"
        );
        assert_eq!(session_starts(), 2);
        fixture.assert_b_ai_only();
    }
    fixture.env.resume_details_meta_loading();
    fixture.ready();
    fixture.assert_columns(1, (13, 11));
    assert_eq!(fixture.env.details_order, [1, 0]);
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
    assert_eq!(session_starts(), 3);
    fixture.assert_stable(3);
}

#[test]
fn facet_835_narrowing_ready_projection_does_not_start_metadata_session() {
    let mut fixture = Fixture::new(Restriction::None, false);
    fixture.acquire_visible_columns();
    fixture.assert_columns(1, (13, 11));
    let proof = fixture.env.listing_meta_scan_proof;
    fixture.env.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB);
    fixture.env.rebuild_visible_indices();
    assert_eq!(fixture.env.visible_indices, [0]);
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
    fixture.assert_stable(2);
}

#[test]
fn facet_835_finished_ai_projection_does_not_cycle_metadata_session_starts() {
    // First Finished both proves AI and rebuilds the visible set (C disappears).
    // The ordinary columns of retained A are already complete in that worker.
    let mut fixture = Fixture::new(Restriction::Size, true);
    fixture.assert_columns(0, (7, 5));
    fixture.assert_b_ai_only();
    assert_eq!(fixture.env.details_order, [0]);
    let proof = fixture.env.listing_meta_scan_proof;
    fixture.assert_stable(1);
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
}

#[test]
fn facet_835_pending_expansion_uses_revision_replacement_without_double_start() {
    let mut fixture = Fixture::new(Restriction::Size, false);
    fixture.begin_columns();
    let pending = fixture
        .env
        .details_meta_pending
        .as_mut()
        .expect("real A worker");
    let DetailsMetaPendingPhase::Loading { rx, .. } = &mut pending.phase else {
        panic!("small A plan must launch its worker");
    };
    let (delivery, relay) = mpsc::channel();
    let worker = std::mem::replace(rx, relay);
    let cancel = Arc::clone(&pending.cancel);
    let deadline = Instant::now() + DEADLINE;
    let finished = loop {
        let event = worker
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("real A worker Finished");
        if matches!(event, DetailsMetaEvent::Finished { .. }) {
            break event;
        }
        delivery.send(event).unwrap();
    };
    fixture.env.poll_details_meta_load(&fixture.ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    fixture.assert_columns(0, (7, 5));
    fixture.assert_b_ai_only();
    let revision = fixture.env.details_lazy_visible_revision;
    let proof = fixture.env.listing_meta_scan_proof;
    fixture.expand();
    assert_ne!(fixture.env.details_lazy_visible_revision, revision);
    assert!(
        !cancel.load(Ordering::Relaxed),
        "rebuild delegates retirement to poll"
    );
    assert_eq!(session_starts(), 2);
    fixture.env.start_details_meta_load(&fixture.ctx);
    assert_eq!(
        session_starts(),
        2,
        "pending guard rejects a duplicate start"
    );
    delivery.send(finished).unwrap();
    fixture.env.poll_details_meta_load(&fixture.ctx);
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(session_starts(), 3, "one revision replacement");
    fixture.ready();
    fixture.assert_columns(1, (13, 11));
    assert_eq!(fixture.env.details_order, [1, 0]);
    assert_eq!(fixture.env.listing_meta_scan_proof, proof);
    fixture.assert_stable(3);
}
