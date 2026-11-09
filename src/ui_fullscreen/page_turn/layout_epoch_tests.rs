//! §1.750: presentation validity belongs to the accepted local display unit.
use super::clock_tests::TestClock;
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

const FAR: usize = 10;

fn diagnostics() -> Vec<serde_json::Value> {
    TEST_CANCEL_DIAGNOSTICS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

fn start(
    ctx: &egui::Context,
    mode: SpreadMode,
    native: bool,
    paired: bool,
) -> (
    crate::app::AppTestEnvForTest,
    Arc<PaintedUnit>,
    Arc<SubmissionTracker>,
) {
    start_with_offer_update(ctx, mode, native, paired, false)
}

fn start_with_offer_update(
    ctx: &egui::Context,
    mode: SpreadMode,
    native: bool,
    paired: bool,
    far_while_offered: bool,
) -> (
    crate::app::AppTestEnvForTest,
    Arc<PaintedUnit>,
    Arc<SubmissionTracker>,
) {
    install(ctx);
    let mut app = ready_app(ctx, mode);
    app.items.extend(
        (4..12).map(|idx| GridItem::Image(PathBuf::from(format!("c:/layout-epoch/{idx}.png")))),
    );
    app.visible_indices = (0..12).collect();
    app.items_generation += 1;
    app.fs_cache.clear();
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..12).map(|_| ThumbnailState::Pending).collect();
    for idx in 0..12 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    let size = if paired {
        [90, 130]
    } else if native {
        [1600, 900]
    } else {
        [160, 90]
    };
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::LIGHT_BLUE));
        let tex = ctx.load_texture(
            format!("layout-epoch-{idx}"),
            (*pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some(size),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
    }
    let target = if paired { 2 } else { 1 };
    let mut templates = Vec::new();
    for anchor in [target, 0] {
        app.fullscreen_idx = Some(anchor);
        frame(ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("ordinary flat");
        };
        templates.push(if native {
            super::prepare_boundary_tests::native_pages(&app, &unit.pages)
        } else {
            unit.pages.clone()
        });
    }
    if native {
        frame(ctx, |ui, ctx| {
            for page in &mut templates[1] {
                page.painter = ui.painter().with_clip_rect(page.painter.clip_rect());
            }
            app.submit_paged_display_unit(ctx, &templates[1], None);
        });
    }
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    frame(ctx, |ui, ctx| {
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "layout-epoch-regression",
        );
        app.offer_page_turn_with_operation(ctx, target, true, Some(operation));
        assert!(
            matches!(
                app.page_turn_presentation.state,
                PresentationState::Offered { .. }
            ),
            "real offer: {:?}",
            take_test_events()
        );
        if far_while_offered {
            rebuild_after_thumbnail(&mut app, FAR, target, true);
        }
        app.fullscreen_idx = Some(target);
        let receipt = if native {
            for page in &mut templates[0] {
                page.painter = ui.painter().with_clip_rect(page.painter.clip_rect());
            }
            app.submit_paged_display_unit(ctx, &templates[0], None)
                .unwrap()
        } else {
            draw_receipt(&mut app, ui, ctx)
        };
        assert!(receipt.curl, "real start: {:?}", take_test_events());
    });
    let PresentationState::Turning {
        incoming, offer, ..
    } = &app.page_turn_presentation.state
    else {
        panic!("turning");
    };
    let incoming = incoming.clone();
    let tracker = offer.submission.clone();
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    assert_eq!(incoming.pages.len(), if paired { 2 } else { 1 });
    assert!(
        incoming
            .pages
            .iter()
            .all(|p| p.resource.lanczos_output().is_some() == native)
    );
    diagnostics();
    take_test_events();
    (app, incoming, tracker)
}

fn draw_receipt(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let (_, sources) = draw_current(app, ui, ctx);
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("actual paged receipt");
    };
    receipt
}

/// Use the same paint-only Native payload as the existing prepare boundary fixtures.
/// Native allocation is synthetic; reuse/prepare, binding, submission and output acceptance are real.
fn prepare_and_submit_native(
    app: &mut App,
    ui: &egui::Ui,
    ctx: &egui::Context,
    old: &PaintedUnit,
) -> Option<SubmittedUnitReceipt> {
    let mut pages = Vec::new();
    for old in &old.pages {
        let painter = ui.painter().with_clip_rect(old.painter.clip_rect());
        let resource = app.prepare_fs_paged_paint_payload(
            &old.resource,
            &old.transform,
            &painter,
            &old.underlay,
            FsPageLayoutSource::CurrentItem,
            old.live,
        )?;
        pages.push(app.page_turn_bind_prepared(PreparedPagedPage {
            resource,
            painter,
            ..old.clone()
        }));
    }
    app.submit_paged_display_unit(ctx, &pages, old.white)
}

fn rebuild_after_thumbnail(app: &mut App, idx: usize, anchor: usize, unchanged: bool) {
    assert_eq!(app.page_dims_cache.get(app.items_generation, idx), None);
    assert!(
        app.spread_display_units_cache
            .tracks(app.items_generation, idx)
    );
    assert!(
        !app.spread_display_units_cache
            .landscape_indices
            .contains(&idx)
    );
    let before = app.spread_display_composition_for_anchor(anchor);
    let old_layout = Arc::clone(&app.spread_display_units_cache.units);
    let old_binding = app.page_turn_unit_binding(anchor).unwrap();
    let epoch = app.spread_display_units_cache.landscape_epoch;
    // The actual thumbnail-dimension adoption entry; no manual Arc replacement.
    app.record_thumbnail_page_dims_for_spread(idx, (1600, 900));
    assert_eq!(
        app.spread_display_units_cache.landscape_epoch,
        epoch.wrapping_add(1)
    );
    assert!(
        Arc::ptr_eq(&old_layout, &app.spread_display_units_cache.units),
        "rebuild is lazy"
    );
    let after = app.spread_display_composition_for_anchor(anchor);
    assert!(
        !Arc::ptr_eq(&old_layout, &app.spread_display_units_cache.units),
        "ordinary resolver rebuilds the book layout"
    );
    let current = app.page_turn_unit_binding(anchor).unwrap();
    if unchanged {
        assert_eq!(before, after, "turning occurrences and placement unchanged");
        assert_eq!(old_binding.layout, current.layout);
        assert!(old_binding.matches_current(app));
    } else {
        assert!(
            old_binding.layout != current.layout,
            "thumbnail dimensions really change this unit's formation"
        );
        assert!(!old_binding.matches_current(app));
    }
}

fn deliver_far_thumbnail(app: &mut App, old: &PaintedUnit) {
    rebuild_after_thumbnail(app, FAR, 1, true);
    assert_eq!(
        old.binding.layout,
        app.page_turn_unit_binding(1).unwrap().layout
    );
}

fn unchanged_far_thumbnail_must_complete(native: bool, mode: SpreadMode) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let now = Instant::now();
    let clock = TestClock::at(now);
    let ctx = egui::Context::default();
    let (mut app, old, tracker) = start(&ctx, mode, native, false);
    // Prove the fixture can keep the same Native output through real prepare
    // before introducing the single new input (far thumbnail dimensions).
    clock.set(now + Duration::from_millis(20));
    frame(&ctx, |ui, ctx| {
        let receipt = if native {
            prepare_and_submit_native(&mut app, ui, ctx, &old).unwrap()
        } else {
            draw_receipt(&mut app, ui, ctx)
        };
        assert!(receipt.curl, "baseline continues");
    });
    deliver_far_thumbnail(&mut app, &old);
    clock.set(now + Duration::from_millis(40));
    let mut continued = false;
    frame(&ctx, |ui, ctx| {
        let receipt = if native {
            prepare_and_submit_native(&mut app, ui, ctx, &old)
        } else {
            Some(draw_receipt(&mut app, ui, ctx))
        };
        continued = receipt.as_ref().is_some_and(|r| r.curl);
        if let Some(receipt) = receipt {
            assert!(
                receipt
                    .pages()
                    .iter()
                    .zip(&old.pages)
                    .all(|((occ, source, paint, _), old)| *occ == old.occurrence
                        && *source == old.resource.source_texture_id()
                        && *paint == old.resource.paint_texture_id()),
                "same content/material/geometry fixture"
            );
        }
    });
    let differences = diagnostics();
    assert!(
        differences.is_empty(),
        "unrelated rebuild must not cancel: {differences:?}"
    );
    eprintln!("§1.750 native={native} mode={mode:?}: {differences:?}");
    assert!(
        continued,
        "§1.750: unrelated far-page layout rebuild must preserve curl; {differences:?}"
    );
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    // The fixed implementation must reach the canonical flat landing with
    // the original immutable material.
    clock.set(now + Duration::from_millis(601));
    frame(&ctx, |ui, ctx| {
        let receipt = if native {
            prepare_and_submit_native(&mut app, ui, ctx, &old).unwrap()
        } else {
            draw_receipt(&mut app, ui, ctx)
        };
        assert!(!receipt.curl);
        assert!(
            receipt
                .pages()
                .iter()
                .zip(&old.pages)
                .all(|((occ, source, paint, _), old)| *occ == old.occurrence
                    && *source == old.resource.source_texture_id()
                    && *paint == old.resource.paint_texture_id())
        );
    });
    assert_eq!(tracker.status(), SubmissionStatus::Finished);
    assert!(
        take_test_events()
            .iter()
            .any(|(_, kind, reason)| *kind == "finish" && *reason == "canonical_submitted")
    );
}

#[test]
fn regression_1750_far_thumbnail_layout_rebuild_managed_must_continue_and_land() {
    unchanged_far_thumbnail_must_complete(false, SpreadMode::Ltr);
}

#[test]
fn regression_1750_far_thumbnail_layout_rebuild_native_prepare_must_continue_and_land() {
    unchanged_far_thumbnail_must_complete(true, SpreadMode::Ltr);
}

#[test]
fn regression_1750_far_thumbnail_layout_rebuild_managed_rtl_must_continue_and_land() {
    unchanged_far_thumbnail_must_complete(false, SpreadMode::Rtl);
}

#[test]
fn regression_1750_far_thumbnail_layout_rebuild_native_prepare_rtl_must_continue_and_land() {
    unchanged_far_thumbnail_must_complete(true, SpreadMode::Rtl);
}

#[test]
fn regression_1750_actual_partner_change_still_cancels() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let _clock = TestClock::at(Instant::now());
        let ctx = egui::Context::default();
        let (mut app, old, tracker) = start(&ctx, mode, false, true);
        // A live full-size source takes precedence over cached dimensions.
        // Rotate the real partner through the landscape reconciliation entry
        // instead of publishing dimensions shadowed by that resident source.
        app.rotation_cache
            .insert(3, crate::rotation_db::Rotation::Cw90);
        app.reconcile_spread_landscape_for_idx(3);
        assert!(
            app.spread_display_units_cache
                .landscape_indices
                .contains(&3)
        );
        frame(&ctx, |ui, ctx| {
            let composition = app.spread_display_composition_for_anchor(2);
            assert_eq!(composition.navigation_pages(), vec![2]);
            assert!(!draw_receipt(&mut app, ui, ctx).curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        let differences = diagnostics();
        assert_eq!(differences.len(), 1, "{differences:?}");
        assert_eq!(differences[0]["origin"], "turn_compare");
        assert_eq!(differences[0]["layout"]["occurrences_equal"], false);
        assert_eq!(differences[0]["layout"]["formation_equal"], false);
        assert!(old.pages.len() == 2);
    }
}

#[test]
fn regression_1750_resize_and_clip_still_cancel_native_prepare() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for resize in [true, false] {
            let _clock = TestClock::at(Instant::now());
            let ctx = egui::Context::default();
            let (mut app, old, tracker) = start(&ctx, mode, true, false);
            frame(&ctx, |ui, _| {
                let page = &old.pages[0];
                let transform = if resize {
                    super::prepare_boundary_tests::changed_transform(
                        &page.transform,
                        page.resource.size_vec2(),
                        super::prepare_boundary_tests::ViewChange::Resize,
                    )
                } else {
                    page.transform
                };
                let painter = ui.painter().with_clip_rect(if resize {
                    page.painter.clip_rect()
                } else {
                    page.painter.clip_rect().shrink(1.0)
                });
                assert!(
                    app.prepare_fs_paged_paint_payload(
                        &page.resource,
                        &transform,
                        &painter,
                        &page.underlay,
                        FsPageLayoutSource::CurrentItem,
                        true
                    )
                    .is_none()
                );
            });
            assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
            let differences = diagnostics();
            assert_eq!(differences.len(), 1, "{differences:?}");
            assert_eq!(differences[0]["origin"], "prepare_reuse");
            assert_eq!(
                differences[0]["different_fields"],
                serde_json::json!([if resize { "viewport_rect" } else { "clip_rect" }])
            );
        }
    }
}

#[test]
fn regression_1750_far_thumbnail_while_offered_preserves_accepted_sources() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let origin = Instant::now();
        let clock = TestClock::at(origin);
        let ctx = egui::Context::default();
        let (mut app, incoming, tracker) = start_with_offer_update(&ctx, mode, false, false, true);
        assert_eq!(tracker.started_at(), Some(origin));
        clock.set(origin + Duration::from_millis(601));
        frame(&ctx, |ui, ctx| {
            let receipt = draw_receipt(&mut app, ui, ctx);
            assert!(!receipt.curl);
            assert!(receipt.matches_flat_unit(&incoming));
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
    }
}

fn completion_debug(app: &App, ctx: &egui::Context) -> serde_json::Value {
    let state = match &app.page_turn_presentation.state {
        PresentationState::Completing {
            incoming,
            completion,
            ..
        } => serde_json::json!({
            "current":incoming.binding.matches_current(app),
            "policy":incoming.binding.matches_prepare_policy(app),
            "view":incoming.binding.prepared_matches_view(app),
            "frame_view":incoming.binding.matches_viewport_geometry(ctx),
            "receipt": match completion {
                FlatCompletion::NeedFlat => serde_json::json!("NeedFlat"),
                FlatCompletion::Submitted(receipt) => serde_json::json!({
                    "accepted":receipt.was_accepted(), "frame":receipt.frame(), "current_frame":ctx.cumulative_frame_nr(),
                    "destination":receipt.destination()==app.page_turn_destination(),
                    "flat_unit":receipt.matches_flat_unit(incoming), "content":receipt.content_matches(app),
                }),
            },
        }),
        PresentationState::Flat(_) => serde_json::json!("Flat"),
        PresentationState::Turning { .. } => serde_json::json!("Turning"),
        _ => serde_json::json!("other"),
    };
    serde_json::json!({"state":state,"pending":app.pending_fs_page_turn_ready.is_some(),
        "observer_blocks":app.fs_navigation_sequence_blocks_new_target(),
        "owning_viewport":ctx.viewport_id()==app.page_turn_destination().viewport})
}

fn draw_observed(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("actual paged receipt");
    };
    receipt
}

#[test]
fn regression_1750_far_thumbnail_completing_retains_ready_and_commits_once() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for after_adoption in [false, true] {
            let origin = Instant::now();
            let _clock = TestClock::at(origin);
            let ctx = egui::Context::default();
            let (mut app, incoming, tracker) = start(&ctx, mode, false, false);
            // Install the same display observer that real ordinary navigation owns.
            assert!(app.begin_fs_page_navigation_sequence(&ctx, 0, 1, false));
            let operation = ViewerOperationId::Slideshow(next_operation_batch());
            let mut submitted = None;
            take_test_events();
            frame(&ctx, |ui, ctx| {
                assert!(app.page_turn_defer_navigation(
                    ctx,
                    2,
                    None,
                    crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                    Some(operation),
                ));
                let receipt = draw_observed(&mut app, ui, ctx);
                assert!(!receipt.curl);
                assert!(!receipt.was_accepted());
                assert!(app.pending_fs_page_turn_ready.is_some());
                assert!(app.fs_navigation_sequence_blocks_new_target());
                assert!(receipt.matches_flat_unit(&incoming));
                if !after_adoption {
                    deliver_far_thumbnail(&mut app, &incoming);
                }
                assert!(!app.commit_page_turn_continuation(ctx));
                submitted = Some(receipt);
            });
            let receipt = submitted.unwrap();
            assert!(receipt.was_accepted());
            if after_adoption {
                deliver_far_thumbnail(&mut app, &incoming);
            }
            assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
            frame(&ctx, |_, ctx| {
                assert!(
                    app.commit_page_turn_continuation(ctx),
                    "{}",
                    completion_debug(&app, ctx)
                );
                assert_eq!(app.fullscreen_idx, Some(2));
                assert!(app.pending_fs_page_turn_ready.is_none());
                assert!(!app.page_turn_barrier_active());
                assert!(!app.commit_page_turn_continuation(ctx));
            });
            let events = take_test_events();
            assert_eq!(
                events
                    .iter()
                    .filter(|(fact, kind, reason)| fact.operation_id == operation
                        && *kind == "skip"
                        && *reason == "stage1_continuation_instant")
                    .count(),
                1,
                "one resolved continuation after far rebuild: {events:?}"
            );
        }
    }
}

/// Unknown page 3 is portrait by the normal fallback. No resident source can
/// shadow its subsequently arriving thumbnail dimensions.
fn portrait_boundary_book(ctx: &egui::Context, app: &mut App) {
    app.items = (0..14)
        .map(|idx| GridItem::Image(PathBuf::from(format!("c:/boundary-book/{idx}.png"))))
        .collect();
    app.visible_indices = (0..14).collect();
    app.items_generation = 1750;
    app.spread_mode = SpreadMode::Ltr;
    app.reading_flow = ReadingFlow::Paged;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.fs_cache.clear();
    app.page_dims_cache.clear();
    app.thumbnails = vec![ThumbnailState::Pending; 14];
    for idx in 0..14 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        if idx != 3 {
            app.record_page_dims_for_spread(idx, (90, 130));
        }
        if idx >= 8 {
            let pixels = Arc::new(egui::ColorImage::filled(
                [90, 130],
                egui::Color32::LIGHT_BLUE,
            ));
            let tex = ctx.load_texture(
                format!("boundary-{idx}"),
                (*pixels).clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels,
                    source_dims: Some([90, 130]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
        }
    }
    app.fullscreen_idx = Some(8);
}

fn start_boundary_turn(ctx: &egui::Context, app: &mut App) -> Arc<PaintedUnit> {
    let viewport = app.page_turn_destination().viewport;
    for anchor in [10, 8] {
        app.fullscreen_idx = Some(anchor);
        unit_frame(ctx, viewport, |ui, ctx| {
            draw_current(app, ui, ctx);
        });
    }
    unit_frame(ctx, viewport, |ui, ctx| {
        let operation = ViewerOperationId::Slideshow(next_operation_batch());
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "boundary-turn");
        app.offer_page_turn_with_operation(ctx, 10, true, Some(operation));
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Offered { .. }
        ));
        app.fullscreen_idx = Some(10);
        assert!(draw_receipt(app, ui, ctx).curl);
    });
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("boundary fixture starts a real turn");
    };
    assert_eq!(
        incoming
            .pages
            .iter()
            .map(|p| p.occurrence.idx)
            .collect::<Vec<_>>(),
        vec![10, 11]
    );
    incoming.clone()
}

fn unit_frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    sized_unit_frame(ctx, viewport, egui::vec2(600.0, 400.0), 1.0, run);
}

fn sized_unit_frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    size: egui::Vec2,
    pixels_per_point: f32,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            focused: Some(true),
            native_pixels_per_point: Some(pixels_per_point),
            ..Default::default()
        },
    );
    let _ = ctx
        .run_pass(raw, |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| run(ui, ctx));
            });
        })
        .drop_without_applying_deltas();
}

#[derive(Clone, Copy, Debug)]
enum ContinuationKind {
    Page,
    Slideshow,
    SpreadShift,
}

#[test]
fn regression_1750_far_boundary_retires_stale_shift_but_preserves_page_and_slideshow() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for kind in [
        ContinuationKind::Page,
        ContinuationKind::Slideshow,
        ContinuationKind::SpreadShift,
    ] {
        let _clock = TestClock::at(Instant::now());
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Ltr);
        portrait_boundary_book(&ctx, &mut app);
        let incoming = start_boundary_turn(&ctx, &mut app);
        let operation = ViewerOperationId::Slideshow(next_operation_batch());
        let old_nudge = app.compute_spread_offset_nudge(10, 1).unwrap();
        assert_eq!(old_nudge, (11, 1));
        assert_eq!(app.spread_shift_anchor_idx(), None);
        take_test_events();
        frame(&ctx, |ui, ctx| {
            assert!(match kind {
                ContinuationKind::Page => app.page_turn_defer_navigation(
                    ctx,
                    12,
                    None,
                    crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                    Some(operation)
                ),
                ContinuationKind::Slideshow => app.page_turn_defer_slideshow_navigation(
                    ctx,
                    12,
                    None,
                    crate::app::book_composition_permission::BookTargetPurpose::Next,
                    operation
                ),
                ContinuationKind::SpreadShift =>
                    app.page_turn_defer_spread_shift(ctx, old_nudge.0, Some(operation), old_nudge.1),
            });
            assert!(!draw_receipt(&mut app, ui, ctx).curl);
        });
        let old_ordinal = app.spread_display_units_cache.unit_position(10).unwrap();
        rebuild_after_thumbnail(&mut app, 3, 10, true);
        assert_ne!(
            old_ordinal,
            app.spread_display_units_cache.unit_position(10).unwrap(),
            "remote singleton formation changes this unit ordinal only"
        );
        assert_eq!(
            incoming.binding.layout,
            app.page_turn_unit_binding(10).unwrap().layout
        );
        assert_eq!(
            app.compute_spread_offset_nudge(10, 1),
            Some((11, 5)),
            "the accepted shift base is now stale"
        );
        frame(&ctx, |_, ctx| {
            let committed = app.commit_page_turn_continuation(ctx);
            assert_eq!(
                committed,
                !matches!(kind, ContinuationKind::SpreadShift),
                "{kind:?}: {}",
                completion_debug(&app, ctx)
            );
            assert_eq!(app.fullscreen_idx, Some(if committed { 12 } else { 10 }));
            assert_eq!(
                app.spread_shift_anchor_idx(),
                None,
                "never apply a stale shift base"
            );
            assert!(!app.page_turn_barrier_active());
            assert!(!app.commit_page_turn_continuation(ctx));
        });
        let events = take_test_events();
        let commits = events
            .iter()
            .filter(|(fact, kind, reason)| {
                fact.operation_id == operation
                    && *kind == "skip"
                    && *reason == "stage1_continuation_instant"
            })
            .count();
        assert_eq!(
            commits,
            usize::from(!matches!(kind, ContinuationKind::SpreadShift)),
            "{events:?}"
        );
    }
}

#[test]
fn regression_1750_near_thumbnail_arrival_really_changes_formation_and_cancels() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let _clock = TestClock::at(Instant::now());
        let ctx = egui::Context::default();
        let (mut app, incoming, tracker) = start(&ctx, mode, false, true);
        // No live full-size or thumbnail metadata may shadow this real arrival.
        app.fs_cache.remove(&3).unwrap();
        app.page_dims_cache.clear();
        assert!(app.known_page_dims_for_idx(3).is_none());
        let old_layout = Arc::clone(&app.spread_display_units_cache.units);
        let old_binding = app.page_turn_unit_binding(2).unwrap();
        let epoch = app.spread_display_units_cache.landscape_epoch;
        let tex = ctx.load_texture(
            "near-landscape-thumbnail",
            egui::ColorImage::filled([160, 90], egui::Color32::LIGHT_BLUE),
            egui::TextureOptions::LINEAR,
        );
        app.thumbnails[3] = ThumbnailState::loaded_raster(
            tex,
            crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            false,
            160,
            crate::source_coverage::SourceCoverage::FullSource,
            Some((1600, 900)),
            None,
        );
        app.record_thumbnail_page_dims_for_spread(3, (1600, 900));
        assert_eq!(
            app.spread_display_units_cache.landscape_epoch,
            epoch.wrapping_add(1)
        );
        app.spread_display_composition_for_anchor(2);
        assert!(!Arc::ptr_eq(
            &old_layout,
            &app.spread_display_units_cache.units
        ));
        assert!(old_binding.layout != app.page_turn_unit_binding(2).unwrap().layout);
        assert!(!old_binding.matches_current(&app));
        frame(&ctx, |ui, ctx| {
            assert_eq!(
                app.spread_display_composition_for_anchor(2)
                    .navigation_pages(),
                vec![2]
            );
            assert!(!draw_receipt(&mut app, ui, ctx).curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        assert_eq!(incoming.pages.len(), 2);
    }
}

#[test]
fn regression_1750_changed_continuation_target_retires_without_resolving_another_next() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let _clock = TestClock::at(Instant::now());
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    portrait_boundary_book(&ctx, &mut app);
    let incoming = start_boundary_turn(&ctx, &mut app);
    let old_target = app.page_turn_unit_binding(12).unwrap();
    frame(&ctx, |ui, ctx| {
        assert!(app.page_turn_defer_navigation(
            ctx,
            12,
            None,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            Some(ViewerOperationId::Slideshow(next_operation_batch()))
        ));
        assert!(!draw_receipt(&mut app, ui, ctx).curl);
    });
    app.rotation_cache
        .insert(13, crate::rotation_db::Rotation::Cw90);
    app.reconcile_spread_landscape_for_idx(13);
    app.spread_display_composition_for_anchor(12);
    assert!(!old_target.matches_current(&app));
    assert!(incoming.binding.matches_current(&app));
    frame(&ctx, |_, ctx| {
        assert!(!app.commit_page_turn_continuation(ctx));
        assert_eq!(app.fullscreen_idx, Some(10));
        assert!(
            !app.page_turn_barrier_active(),
            "{}",
            completion_debug(&app, ctx)
        );
    });
}

#[test]
fn regression_1750_flat_receipt_rejects_different_binding_destination_and_generation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = TestClock::at(origin);
    let ctx = egui::Context::default();
    let (mut app, incoming, _) = start(&ctx, SpreadMode::Ltr, false, false);
    clock.set(origin + Duration::from_millis(601));
    let mut flat = None;
    frame(&ctx, |ui, ctx| {
        flat = Some(draw_receipt(&mut app, ui, ctx));
    });
    let receipt = flat.unwrap();
    assert!(receipt.matches_flat_unit(&incoming));
    for change in [
        "destination",
        "generation",
        "content",
        "geometry",
        "formation",
        "placement",
        "supplement",
        "direction",
        "slice",
        "white",
    ] {
        let mut other = (*incoming).clone();
        match change {
            "destination" => {
                other.binding.destination.viewport =
                    egui::ViewportId::from_hash_of("sibling-output")
            }
            "generation" => other.binding.items += 1,
            "content" => other.binding.content[0].gap += 1,
            "geometry" => {
                other.pages[0].painter = other.pages[0]
                    .painter
                    .with_clip_rect(other.pages[0].painter.clip_rect().shrink(1.0))
            }
            "formation" => {
                other.binding.layout.formation = SpreadDisplayUnitFormation::Singleton(
                    SpreadDisplaySingletonCause::ExplicitAlone,
                )
            }
            "placement" => {
                other.binding.layout.composition.singleton_placement =
                    SingletonSpreadPlacement::Left
            }
            "supplement" => {
                other
                    .binding
                    .layout
                    .composition
                    .supplemented_navigation_side = Some(SingletonSpreadPlacement::Right)
            }
            "direction" => other.binding.reading_rtl = !other.binding.reading_rtl,
            "slice" => other.binding.slice = crate::page_split::PageSlice::Left,
            "white" => {
                other.white = Some((
                    other.pages[0].transform.paint_rect,
                    other.pages[0].painter.clip_rect(),
                ))
            }
            _ => unreachable!(),
        }
        other.binding = other
            .binding
            .clone()
            .with_prepared(&other.pages, other.white);
        assert!(
            !receipt.matches_flat_unit(&other),
            "reject {change} with identical page/source/paint IDs"
        );
    }
}

#[test]
fn regression_1750_incoming_and_outgoing_edits_invalidate_their_own_units() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for idx in [0, 1] {
        for change in ["rotation", "erase", "params", "comic", "crop"] {
            let _clock = TestClock::at(Instant::now());
            let ctx = egui::Context::default();
            let (mut app, _, tracker) = start(&ctx, SpreadMode::Ltr, false, false);
            let binding = app.page_turn_unit_binding(idx).unwrap();
            match change {
                "rotation" => {
                    app.rotation_cache
                        .insert(idx, crate::rotation_db::Rotation::Cw180);
                }
                "erase" => {
                    app.erase_mask_generation.insert(idx, 1);
                }
                "params" => {
                    let mut params = app.effective_params(idx).clone();
                    params.brightness += 10.0;
                    app.adjustment_page_params.insert(idx, params);
                }
                "comic" => {
                    app.comic_generation += 1;
                }
                "crop" => {
                    app.export_crop_page_settings.insert(
                        idx,
                        crate::export_crop::CropSettings::authored(
                            crate::export_crop::CropRect {
                                min_x: 1.0,
                                min_y: 1.0,
                                max_x: 100.0,
                                max_y: 70.0,
                            },
                            crate::export_crop::CropAspectMode::Free,
                            [160, 90],
                        ),
                    );
                }
                _ => unreachable!(),
            }
            assert!(!binding.matches_current(&app), "{idx} {change}");
            assert!(app.page_turn_held_resource(idx).is_none(), "{idx} {change}");
            frame(&ctx, |ui, ctx| {
                assert!(!draw_receipt(&mut app, ui, ctx).curl, "{idx} {change}");
            });
            assert_eq!(
                tracker.status(),
                SubmissionStatus::Cancelled,
                "{idx} {change}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn regression_1750_main_and_detached_same_book_bindings_and_lifecycles_are_isolated() {
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let _clock = TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    portrait_boundary_book(&ctx, &mut app);
    let main = start_boundary_turn(&ctx, &mut app);
    let main_tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => unreachable!(),
    };
    let main_pending = app.fs_pending.len();
    let main_generation = app.items_generation;
    let main_cache = Arc::clone(&app.spread_display_units_cache.units);
    let main_lease = app.page_turn_leases.reserved_bytes();
    let main_material = main
        .pages
        .iter()
        .map(|p| p.resource.paint_texture_id())
        .collect::<Vec<_>>();
    let sibling = app.build_window_context_for_test(175_002, |owner| {
        portrait_boundary_book(&ctx, owner);
        owner.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        owner.transition_detached_window_state(
            175_002,
            crate::app::DetachedWindowState::Active,
            "layout-binding-isolation",
        );
    });
    app.with_window_context_for_test(sibling, |owner| {
        let detached = start_boundary_turn(&ctx, owner);
        assert_eq!(owner.items_generation, main_generation);
        assert_eq!(detached.binding.layout, main.binding.layout);
        assert_ne!(detached.binding.destination, main.binding.destination);
        assert!(!main.binding.matches_current(owner));
        assert!(detached.binding.matches_current(owner));
        rebuild_after_thumbnail(owner, 3, 10, true);
        let viewport = owner.page_turn_destination().viewport;
        let mut flat = None;
        unit_frame(&ctx, viewport, |ui, ctx| {
            assert!(owner.page_turn_defer_navigation(
                ctx,
                12,
                None,
                crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                Some(ViewerOperationId::Slideshow(next_operation_batch()))
            ));
            flat = Some(draw_receipt(owner, ui, ctx));
        });
        let receipt = flat.unwrap();
        assert!(receipt.matches_flat_unit(&detached));
        assert!(!receipt.matches_flat_unit(&main));
        owner.close_fullscreen();
        assert!(!owner.page_turn_barrier_active());
        assert!(owner.pending_fs_page_turn_ready.is_none());
        assert!(!owner.commit_page_turn_continuation(&ctx));
    });
    assert!(main.binding.matches_current(&app));
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(app.fs_pending.len(), main_pending);
    assert!(Arc::ptr_eq(
        &main_cache,
        &app.spread_display_units_cache.units
    ));
    assert_eq!(app.page_turn_leases.reserved_bytes(), main_lease);
    assert_eq!(main_tracker.status(), SubmissionStatus::Started);
    assert_eq!(main_tracker.started_at(), Some(origin));
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("main presentation survives sibling lifecycle");
    };
    assert_eq!(
        incoming
            .pages
            .iter()
            .map(|p| p.resource.paint_texture_id())
            .collect::<Vec<_>>(),
        main_material
    );
    unit_frame(&ctx, main.binding.destination.viewport, |ui, ctx| {
        assert!(draw_receipt(&mut app, ui, ctx).curl);
    });
}

#[derive(Clone, Copy, Debug)]
enum PreparePolicyChange {
    Smoothing,
    PostFilter,
    UpscaleLimit,
}

fn change_prepare_policy(app: &mut App, change: PreparePolicyChange) {
    match change {
        PreparePolicyChange::Smoothing => {
            app.settings.downscale_smoothing_percent =
                (app.settings.downscale_smoothing_percent + 7) % 101
        }
        PreparePolicyChange::PostFilter => app.post_filter_bypassed = !app.post_filter_bypassed,
        PreparePolicyChange::UpscaleLimit => {
            app.settings.anime_upscale_source_limit = if app.settings.anime_upscale_source_limit
                == crate::settings::AnimeUpscaleSourceLimit::Unlimited
            {
                crate::settings::AnimeUpscaleSourceLimit::Px2048
            } else {
                crate::settings::AnimeUpscaleSourceLimit::Unlimited
            }
        }
    }
}

#[test]
fn regression_1750_offered_rejects_changed_outgoing_or_accepted_prepare_policy() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for change in [
        PreparePolicyChange::Smoothing,
        PreparePolicyChange::PostFilter,
        PreparePolicyChange::UpscaleLimit,
    ] {
        for before_offer in [false, true] {
            let _clock = TestClock::at(Instant::now());
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Single);
            app.settings.page_turn_animation_enabled = true;
            app.settings.page_turn_animation_duration_ms = 600;
            for anchor in [1, 0] {
                app.fullscreen_idx = Some(anchor);
                frame(&ctx, |ui, ctx| {
                    draw_current(&mut app, ui, ctx);
                });
            }
            let mut tracker = None;
            frame(&ctx, |ui, ctx| {
                if before_offer {
                    change_prepare_policy(&mut app, change);
                }
                let operation = ViewerOperationId::Slideshow(next_operation_batch());
                app.page_turn_external_operation(
                    ctx,
                    operation,
                    OperationPhase::Press,
                    "policy-offer",
                );
                app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
                // An outgoing policy mismatch may reject admission immediately.
                // Otherwise the ordinary prepare boundary must reject it.
                if let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state
                {
                    tracker = Some(offer.submission.clone());
                } else {
                    assert!(
                        before_offer,
                        "incoming policy changes only after real acceptance"
                    );
                }
                if !before_offer {
                    change_prepare_policy(&mut app, change);
                }
                app.fullscreen_idx = Some(1);
                assert!(
                    !draw_receipt(&mut app, ui, ctx).curl,
                    "{change:?} before_offer={before_offer}"
                );
            });
            if let Some(tracker) = tracker {
                assert_ne!(
                    tracker.status(),
                    SubmissionStatus::Started,
                    "{change:?} before_offer={before_offer}"
                );
                assert_eq!(tracker.started_at(), None);
            }
            assert!(!matches!(
                app.page_turn_presentation.state,
                PresentationState::Turning { .. }
            ));
        }
    }
}

fn completing_policy_change_before_adoption_requires_fresh_flat(enabled: bool) {
    let _clock = TestClock::at(Instant::now());
    let ctx = egui::Context::default();
    let (mut app, incoming, _) = start(&ctx, SpreadMode::Ltr, false, false);
    assert!(app.begin_fs_page_navigation_sequence(&ctx, 0, 1, false));
    let operation = ViewerOperationId::Slideshow(next_operation_batch());
    let mut fresh = None;
    frame(&ctx, |ui, ctx| {
        assert!(app.page_turn_defer_navigation(
            ctx,
            2,
            None,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            Some(operation),
        ));
        app.settings.page_turn_animation_enabled = enabled;
        let old = draw_observed(&mut app, ui, ctx);
        assert!(!old.curl);
        assert!(!old.was_accepted());
        change_prepare_policy(&mut app, PreparePolicyChange::Smoothing);
        // The old output has not been adopted. The same normal prepare boundary
        // must rebind it before the replacement output is submitted.
        let receipt = draw_observed(&mut app, ui, ctx);
        assert!(!receipt.curl);
        assert!(!receipt.was_accepted());
        let PresentationState::Completing {
            incoming: rebound,
            continuation,
            ..
        } = &app.page_turn_presentation.state
        else {
            panic!("replacement canonical flat retains its resolved command");
        };
        assert_eq!(continuation.as_ref().unwrap().fact.operation_id, operation);
        assert!(!old.matches_flat_unit(rebound));
        assert!(receipt.matches_flat_unit(rebound));
        assert!(
            receipt
                .pages()
                .iter()
                .zip(&incoming.pages)
                .all(|((_, source, _, _), page)| *source == page.resource.source_texture_id())
        );
        assert!(!app.commit_page_turn_continuation(ctx));
        fresh = Some(receipt);
    });
    assert!(fresh.unwrap().was_accepted());
    frame(&ctx, |_, ctx| {
        assert!(
            app.commit_page_turn_continuation(ctx),
            "{}",
            completion_debug(&app, ctx)
        );
        assert_eq!(app.fullscreen_idx, Some(2));
        assert!(app.pending_fs_page_turn_ready.is_none());
        assert!(!app.page_turn_barrier_active());
        assert!(!app.commit_page_turn_continuation(ctx));
    });
}

#[test]
fn regression_1750_managed_and_off_policy_changes_before_adoption_require_new_output() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for enabled in [false, true] {
        completing_policy_change_before_adoption_requires_fresh_flat(enabled);
    }
}

#[test]
fn regression_1750_continuation_rejects_same_generation_image_replacement() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for kind in [ContinuationKind::Page, ContinuationKind::Slideshow] {
        for changed in [10, 12, 13] {
            let _clock = TestClock::at(Instant::now());
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Ltr);
            portrait_boundary_book(&ctx, &mut app);
            start_boundary_turn(&ctx, &mut app);
            let from = app.page_turn_unit_binding(10).unwrap();
            let target = app.page_turn_unit_binding(12).unwrap();
            let generation = app.items_generation;
            frame(&ctx, |ui, ctx| {
                let operation = ViewerOperationId::Slideshow(next_operation_batch());
                assert!(match kind {
                    ContinuationKind::Page => app.page_turn_defer_navigation(
                        ctx,
                        12,
                        None,
                        crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                        Some(operation),
                    ),
                    ContinuationKind::Slideshow => app.page_turn_defer_slideshow_navigation(
                        ctx,
                        12,
                        None,
                        crate::app::book_composition_permission::BookTargetPurpose::Next,
                        operation,
                    ),
                    _ => unreachable!(),
                });
                assert!(!draw_receipt(&mut app, ui, ctx).curl);
            });
            assert!(matches!(app.items[changed], GridItem::Image(_)));
            let old_id = app.current_edit_result_key(changed).item_id;
            app.items[changed] = GridItem::Image(PathBuf::from("c:/layout-epoch/replacement.png"));
            assert_eq!(app.items_generation, generation);
            assert_ne!(app.current_edit_result_key(changed).item_id, old_id);
            let current_from = app.page_turn_unit_binding(10).unwrap();
            let current_target = app.page_turn_unit_binding(12).unwrap();
            assert_eq!(from.layout, current_from.layout);
            assert_eq!(target.layout, current_target.layout);
            assert_eq!(from.matches_navigation(&app), changed != 10);
            assert_eq!(target.matches_navigation(&app), changed == 10);
            frame(&ctx, |_, ctx| {
                assert!(
                    !app.commit_page_turn_continuation(ctx),
                    "{kind:?}, occurrence {changed}"
                );
                assert_eq!(app.fullscreen_idx, Some(10));
                assert!(!app.page_turn_barrier_active());
                assert!(!app.commit_page_turn_continuation(ctx));
            });
        }
    }
}

#[test]
fn regression_1750_root_service_delivers_child_ready_with_different_geometry() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let _clock = TestClock::at(Instant::now());
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    portrait_boundary_book(&ctx, &mut app);
    start_boundary_turn(&ctx, &mut app);
    let viewport = app.page_turn_destination().viewport;
    assert_ne!(viewport, egui::ViewportId::ROOT);
    let mut flat = None;
    unit_frame(&ctx, viewport, |ui, ctx| {
        assert!(app.begin_fs_page_navigation_sequence(ctx, 8, 10, false));
        assert!(app.page_turn_defer_navigation(
            ctx,
            12,
            None,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            Some(ViewerOperationId::Slideshow(next_operation_batch())),
        ));
        flat = Some(draw_observed(&mut app, ui, ctx));
    });
    let flat = flat.unwrap();
    assert!(flat.was_accepted());
    assert!(app.pending_fs_page_turn_ready.is_some());
    assert!(app.fs_navigation_sequence_blocks_new_target());
    sized_unit_frame(
        &ctx,
        egui::ViewportId::ROOT,
        egui::vec2(1000.0, 700.0),
        2.0,
        |_, ctx| {
            ctx.show_viewport_deferred(viewport, Default::default(), |_, _| {});
            assert_eq!(ctx.pixels_per_point(), 2.0);
            let pending = app.pending_fs_page_turn_ready.as_ref().unwrap();
            assert!(!pending.binding.matches_viewport_geometry(ctx));
            assert!(flat.content_matches(&app));
            app.service_final_prepare_before_lookup(ctx);
            app.flush_submitted_page_turn_ready(ctx);
            assert!(app.pending_fs_page_turn_ready.is_none());
            assert!(!app.fs_navigation_sequence_blocks_new_target());
            assert_eq!(app.fullscreen_idx, Some(10));
            assert!(app.page_turn_barrier_active());
        },
    );
    unit_frame(&ctx, viewport, |_, ctx| {
        assert!(ctx.cumulative_frame_nr() > flat.frame());
        assert!(
            app.commit_page_turn_continuation(ctx),
            "{}",
            completion_debug(&app, ctx)
        );
        assert_eq!(app.fullscreen_idx, Some(12));
        assert!(!app.page_turn_barrier_active());
        assert!(!app.commit_page_turn_continuation(ctx));
    });
}

#[test]
fn regression_1750_owning_resize_rebinds_before_adoption_and_commits_after_adoption() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for (adopted, draw_first, change_fit) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let _clock = TestClock::at(Instant::now());
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Ltr);
        portrait_boundary_book(&ctx, &mut app);
        let original = start_boundary_turn(&ctx, &mut app);
        let viewport = app.page_turn_destination().viewport;
        let mut old_flat = None;
        unit_frame(&ctx, viewport, |ui, ctx| {
            assert!(app.begin_fs_page_navigation_sequence(ctx, 8, 10, false));
            assert!(app.page_turn_defer_navigation(
                ctx,
                12,
                None,
                crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                Some(ViewerOperationId::Slideshow(next_operation_batch())),
            ));
            if adopted {
                old_flat = Some(draw_observed(&mut app, ui, ctx));
            } else {
                assert!(matches!(
                    app.page_turn_presentation.state,
                    PresentationState::Completing {
                        completion: FlatCompletion::NeedFlat,
                        ..
                    }
                ));
            }
        });
        assert_eq!(
            old_flat.as_ref().map(|receipt| receipt.was_accepted()),
            adopted.then_some(true)
        );
        let size = egui::vec2(900.0, 500.0);
        let mut replacement = None;
        sized_unit_frame(&ctx, viewport, size, 1.0, |ui, ctx| {
            assert_eq!(ctx.viewport_id(), app.page_turn_destination().viewport);
            if let Some(old) = &old_flat {
                assert!(ctx.cumulative_frame_nr() > old.frame());
            }
            if draw_first {
                // The usual ROOT/service prefix delivers the accepted output.
                // A content draw may then run while input ownership defers commit.
                app.service_final_prepare_before_lookup(ctx);
                app.flush_submitted_page_turn_ready(ctx);
                assert!(!app.fs_navigation_sequence_blocks_new_target());
                if change_fit {
                    app.set_fullscreen_fit_mode_for_current(ctx, 10, FullscreenFitMode::Height);
                    assert_eq!(
                        app.effective_fullscreen_fit_mode(),
                        FullscreenFitMode::Height
                    );
                    let PresentationState::Completing { incoming, .. } =
                        &app.page_turn_presentation.state
                    else {
                        panic!("fit changes keep the adopted obligation");
                    };
                    assert!(!incoming.binding.matches_prepare_policy(&app));
                }
                let resized = draw_observed(&mut app, ui, ctx);
                assert!(!resized.was_accepted());
                assert!(resized.has_presentation_binding());
                assert!(resized.content_matches(&app));
                let PresentationState::Completing {
                    incoming,
                    completion,
                    ..
                } = &app.page_turn_presentation.state
                else {
                    panic!("adopted obligation remains");
                };
                assert!(matches!(completion, FlatCompletion::Submitted(old) if old.was_accepted()));
                assert!(old_flat.as_ref().unwrap().matches_flat_unit(incoming));
                assert!(
                    !resized.matches_flat_unit(incoming),
                    "a new resized draw cannot claim the old geometry's proof"
                );
            }
            let entry = app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                matches!(entry, PageTurnFrameEntry::Continue { committed, .. } if committed == adopted),
                "adopted={adopted}: {}",
                completion_debug(&app, ctx)
            );
            assert_eq!(app.fullscreen_idx, Some(if adopted { 12 } else { 10 }));
            assert_eq!(app.page_turn_barrier_active(), !adopted);
            let receipt = draw_observed(&mut app, ui, ctx);
            assert!(!receipt.curl);
            assert!(!receipt.was_accepted());
            assert!(
                app.fullscreen_page_layout
                    .displayed_pages()
                    .all(|page| page.transform.viewport_rect == ui.max_rect())
            );
            if !adopted {
                let PresentationState::Completing {
                    incoming: rebound,
                    continuation,
                    ..
                } = &app.page_turn_presentation.state
                else {
                    panic!("unadopted completion rebinds at normal resized prepare");
                };
                assert!(continuation.is_some());
                assert!(!original.same_material_and_geometry(rebound));
                assert!(receipt.matches_flat_unit(rebound));
                assert!(
                    rebound
                        .pages
                        .iter()
                        .zip(&original.pages)
                        .all(|(fresh, old)| fresh.resource.source_texture_id()
                            == old.resource.source_texture_id())
                );
            }
            assert!(!app.commit_page_turn_continuation(ctx));
            replacement = Some(receipt);
        });
        assert!(replacement.unwrap().was_accepted());
        sized_unit_frame(&ctx, viewport, size, 1.0, |_, ctx| {
            let entry = app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                matches!(entry, PageTurnFrameEntry::Continue { committed, .. } if committed == !adopted)
            );
            assert_eq!(app.fullscreen_idx, Some(12));
            assert!(app.pending_fs_page_turn_ready.is_none());
            assert!(!app.page_turn_barrier_active());
            assert!(!app.commit_page_turn_continuation(ctx));
        });
    }
}
