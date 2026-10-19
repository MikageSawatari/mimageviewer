use super::*;
use std::cell::Cell;

thread_local! {
    static RECEIVING: Cell<bool> = const { Cell::new(false) };
    static EXPECTED_FINAL: Cell<Option<FinalCompositeKey>> = const { Cell::new(None) };
    static LOOKUPS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct ReceiptScope;
impl ReceiptScope {
    pub(super) fn enter() -> Self {
        RECEIVING.with(|receiving| assert!(!receiving.replace(true), "recursive final receipt"));
        Self
    }
}
impl Drop for ReceiptScope {
    fn drop(&mut self) {
        RECEIVING.with(|receiving| receiving.set(false));
    }
}
pub(crate) fn assert_outside_receipt() {
    RECEIVING.with(|receiving| assert!(!receiving.get(), "display selection during final receipt"));
}

pub(crate) fn observe_lookup(app: &App) {
    assert_outside_receipt();
    EXPECTED_FINAL.with(|expected| {
        if let Some(key) = expected.get() {
            assert!(
                app.final_composite_cache.get(&key).is_some(),
                "first lookup preceded exact final adoption"
            );
            LOOKUPS.with(|count| count.set(count.get() + 1));
        }
    });
}

pub(crate) fn expect_adopted_before_lookup(key: FinalCompositeKey, run: impl FnOnce()) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            EXPECTED_FINAL.with(|key| key.set(None));
        }
    }
    let _reset = Reset;
    EXPECTED_FINAL.with(|expected| assert!(expected.replace(Some(key)).is_none()));
    LOOKUPS.with(|count| count.set(0));
    run();
    LOOKUPS.with(|count| assert!(count.get() > 0, "fixture did not reach a selector"));
}

pub(crate) fn queued_final(app: &mut App, idx: usize) -> FinalCompositeKey {
    let size = [4, 6];
    let key = app.final_composite_key_for_pixels(
        app.current_edit_result_key(idx),
        size,
        app.effective_params(idx),
    );
    let (tx, rx) = mpsc::channel();
    tx.send(FinalCompositeResult::Ready {
        pixels: Arc::new(egui::ColorImage::filled(size, egui::Color32::GOLD)),
        elapsed_ms: 0.0,
        timing: FinalCompositeTiming::default(),
    })
    .unwrap();
    app.final_effect_pending.insert(
        key,
        FinalEffectPending {
            canonical_dims: None,
            source_size: size,
            cancel: Arc::new(AtomicBool::new(false)),
            state: FinalEffectState::Computing(FinalEffectReceiver::Legacy(rx)),
            items_generation: app.items_generation,
            output_complete: true,
            nearest_sampler: false,
            origin: FinalEffectOrigin::LegacyDisplay,
            gpu_diagnostics: Default::default(),
        },
    );
    key
}

fn app() -> super::tests::phase_c_support::AppTestEnv {
    let mut app = super::tests::phase_c_support::setup_app();
    app.items
        .push(GridItem::Image(PathBuf::from("C:/d3/final.png")));
    app.thumbnails.push(ThumbnailState::Pending);
    app.visible_indices = vec![0];
    app.details_order = vec![0];
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app
}

#[test]
fn d3_receipt_is_selector_free_nonblocking_and_publishes_once() {
    let ctx = egui::Context::default();
    let mut app = app();
    let key = queued_final(&mut app, 0);
    let received = app.collect_final_prepare_results();
    assert_eq!(received.len(), 1);
    assert!(app.final_composite_cache.is_empty());
    assert!(
        ctx.tex_manager()
            .read()
            .allocated()
            .all(|(_, meta)| !meta.name.starts_with("final_effect_"))
    );
    app.adopt_final_prepare_results_before_lookup(&ctx, received);
    let id = app.final_composite_cache.get(&key).unwrap().texture.id();
    ctx.tex_manager().write().take_delta().clear();
    app.service_final_prepare_before_lookup(&ctx);
    assert_eq!(
        app.final_composite_cache.get(&key).unwrap().texture.id(),
        id
    );
    assert!(ctx.tex_manager().write().take_delta().is_empty());
}

#[test]
fn d3_exact_general_final_precedes_edit_upload_on_and_off() {
    for enabled in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app();
        app.settings.page_turn_animation_enabled = enabled;
        let key = queued_final(&mut app, 0);
        app.service_final_prepare_before_lookup(&ctx);
        let id = app.final_composite_cache.get(&key).unwrap().texture.id();
        ctx.tex_manager().write().take_delta().clear();
        assert_eq!(
            app.ensure_final_composite_texture(&ctx, 0).unwrap().id(),
            id
        );
        assert!(app.edit_result_cache.is_empty());
        assert!(ctx.tex_manager().write().take_delta().is_empty());
    }
}

#[test]
fn d3_resident_final_accounting_uses_registered_sampler_options() {
    for nearest in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app();
        let key = queued_final(&mut app, 0);
        app.final_effect_pending
            .get_mut(&key)
            .unwrap()
            .nearest_sampler = nearest;
        app.service_final_prepare_before_lookup(&ctx);
        let entry = app.final_composite_cache.get(&key).unwrap();
        assert_eq!(entry.texture_options.mipmap_mode.is_some(), !nearest);
        assert_eq!(
            app.vram_accounting_snapshot()
                .subsystem_texels(crate::vram_budget::VramSubsystem::FinalCompositeCache),
            if nearest { 24 } else { 31 }
        );
    }
}

#[test]
fn d3_adoption_revalidates_mutations_between_receive_and_lookup() {
    for mutation in 0..4 {
        let ctx = egui::Context::default();
        let mut app = app();
        let key = queued_final(&mut app, 0);
        let results = app.collect_final_prepare_results();
        match mutation {
            0 => {
                app.items_generation += 1;
            }
            1 => {
                app.input_generation.insert(0, 10);
            }
            2 => {
                app.items[0] = GridItem::Image(PathBuf::from("C:/d3/other.png"));
            }
            _ => {
                app.local_adjust_generation.insert(0, 10);
            }
        }
        app.adopt_final_prepare_results_before_lookup(&ctx, results);
        assert!(
            app.final_composite_cache.get(&key).is_none(),
            "mutation={mutation}"
        );
    }
}

#[test]
fn d3_early_nav_lock_adopts_final_before_selecting_and_unlocks_same_pass() {
    let ctx = egui::Context::default();
    let mut app = app();
    app.fs_nav_locked_gen = Some(app.items_generation.saturating_sub(1));
    if app.items_generation == 0 {
        app.items_generation = 1;
        app.fs_nav_locked_gen = Some(0);
    }
    let mut params = app.effective_params(0).clone();
    params.colorize.mode = crate::colorize::ColorizeMode::AllImages;
    app.adjustment_page_params.insert(0, params);
    let key = queued_final(&mut app, 0);
    expect_adopted_before_lookup(key, || app.poll_fs_nav_lock(&ctx));
    assert!(app.final_composite_cache.get(&key).is_some());
    assert!(app.fs_nav_locked_gen.is_none());
}

#[cfg(windows)]
#[test]
fn d3_sibling_pending_and_generations_survive_other_context_mutations_and_mounts() {
    for mutation in ["close", "book", "adjustment"] {
        let mut app = app();
        let ctx = egui::Context::default();
        let mut sibling_key = None;
        let sibling = app.build_window_context_for_test(91_411, |owner| {
            owner
                .items
                .push(GridItem::Image(PathBuf::from("C:/d3/sibling.png")));
            owner.thumbnails.push(ThumbnailState::Pending);
            owner.visible_indices = vec![0];
            owner.details_order = vec![0];
            owner.fullscreen_idx = Some(0);
            sibling_key = Some(queued_final(owner, 0));
        });
        let key = sibling_key.unwrap();
        let before = app
            .with_viewer_context(sibling, |owner| {
                assert!(
                    owner.final_composite_cache.is_empty(),
                    "mount is not display service"
                );
                (owner.items_generation, owner.current_edit_result_key(0))
            })
            .unwrap();
        queued_final(&mut app, 0);
        match mutation {
            "close" => app.close_fullscreen(),
            "book" => app.invalidate_idx_state_and_queues(),
            _ => app.bump_input_generation(0),
        }
        app.service_final_prepare_before_lookup(&ctx);
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(
                (owner.items_generation, owner.current_edit_result_key(0)),
                before
            );
            assert!(owner.final_effect_pending.contains_key(&key));
            assert!(owner.final_composite_cache.is_empty());
            assert!(
                !owner.final_effect_pending[&key]
                    .cancel
                    .load(Ordering::Relaxed)
            );
            owner.service_final_prepare_before_lookup(&ctx);
            assert!(owner.final_composite_cache.get(&key).is_some());
            assert!(!owner.final_effect_pending.contains_key(&key));
        })
        .unwrap();
    }
}

#[cfg(windows)]
#[test]
fn d3_parked_snapshot_activation_adopts_only_the_resumed_owner() {
    let ctx = egui::Context::default();
    let mut app = app();
    let main_key = queued_final(&mut app, 0);
    let mut resumed_key = None;
    app.push_window_context_for_test(&ctx, 91_412, |owner| {
        owner
            .items
            .push(GridItem::Image(PathBuf::from("C:/d3/parked.png")));
        owner.thumbnails.push(ThumbnailState::Pending);
        owner.visible_indices = vec![0];
        owner.details_order = vec![0];
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = crate::settings::SpreadMode::Single;
        resumed_key = Some(queued_final(owner, 0));
    });
    let key = resumed_key.unwrap();
    assert!(app.activate_detached_image_window_snapshot(&ctx, 91_412));
    app.with_active_viewer_context(|owner| {
        assert!(owner.final_composite_cache.get(&key).is_some());
        assert!(!owner.final_effect_pending.contains_key(&key));
        expect_adopted_before_lookup(key, || {
            let _ = owner.resolve_fs_display_tex(0, false).unwrap();
        });
    })
    .unwrap();
    assert!(app.final_effect_pending.contains_key(&main_key));
    assert!(app.final_composite_cache.get(&main_key).is_none());
}

/// Keep entry ordering reviewable even for routes requiring Windows/context
/// fixtures. Runtime receipt spies above also catch indirect selector calls.
#[test]
fn d3_entry_connections_and_receiver_contract() {
    let app = include_str!("../app.rs");
    let fs = include_str!("../ui_fullscreen.rs");
    let phases = include_str!("update_phases.rs");
    let permission = include_str!("book_composition_permission.rs");
    let barrier = include_str!("../ui_fullscreen/page_turn/barrier.rs");
    let gamepad = include_str!("gamepad_input.rs");
    fn ordered(source: &str, start: &str, first: &str, last: &str) {
        let body = &source[source.find(start).unwrap()..];
        assert!(
            body.find(first).unwrap() < body.find(last).unwrap(),
            "{start}: {first} before {last}"
        );
    }
    ordered(
        app,
        "self.reconcile_edit_preview_refreshes(ctx);",
        "self.service_final_prepare_before_lookup(ctx);",
        "self.poll_fs_nav_lock(ctx);",
    );
    ordered(
        app,
        "app.poll_retained_shift_readoption(ctx);",
        "app.service_final_prepare_before_lookup(ctx);",
        "app.dispatch_gamepad_batch(ctx, batch)",
    );
    ordered(
        phases,
        "self.sync_upscale_from_preset(fs_idx);",
        "self.service_final_prepare_before_lookup(ctx);",
        "self.fs_page_turn_decision_for_frame(ctx, fs_idx)",
    );
    ordered(
        permission,
        "fn resume_qualified_book_target(",
        "self.service_final_prepare_before_lookup(ctx);",
        "match &intent.route",
    );
    ordered(
        barrier,
        "fn page_turn_take_continuation(",
        "self.service_final_prepare_before_lookup(ctx);",
        "self.flush_submitted_page_turn_ready(ctx);",
    );
    ordered(
        gamepad,
        "for event in events {",
        "self.service_final_prepare_before_lookup(ctx);",
        "match event",
    );
    ordered(
        fs,
        "fn fs_page_turn_decision_for_frame_with_perf(",
        "self.service_final_prepare_before_lookup(ctx);",
        "self.book_target_presentation_demand(fs_idx)",
    );
    let collect = app
        .split("fn collect_final_prepare_results(")
        .nth(1)
        .unwrap()
        .split("fn adopt_final_prepare_results_before_lookup(")
        .next()
        .unwrap();
    for forbidden in [
        "fs_page_turn_decision_for_frame(",
        "resolve_fs_display_tex(",
        "ensure_final_composite_texture(",
        "load_texture(",
    ] {
        assert!(!collect.contains(forbidden), "receipt calls {forbidden}");
    }
}
