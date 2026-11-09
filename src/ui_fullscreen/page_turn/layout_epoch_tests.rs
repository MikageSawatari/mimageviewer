//! §1.750: intentionally red until semantic presentation binding is reviewed/fixed.
//! No ignore/should_panic: normal page_turn runs must expose this regression.
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

fn deliver_far_thumbnail(app: &mut App, old: &PaintedUnit) {
    assert_eq!(app.page_dims_cache.get(app.items_generation, FAR), None);
    assert!(
        app.spread_display_units_cache
            .tracks(app.items_generation, FAR)
    );
    assert!(
        !app.spread_display_units_cache
            .landscape_indices
            .contains(&FAR)
    );
    let before = app.spread_display_composition_for_anchor(1);
    let epoch = app.spread_display_units_cache.landscape_epoch;
    // Same entry point as thumbnail texture adoption; no manual Arc replacement.
    app.record_thumbnail_page_dims_for_spread(FAR, (1600, 900));
    assert_eq!(
        app.spread_display_units_cache.landscape_epoch,
        epoch.wrapping_add(1)
    );
    assert!(
        Arc::ptr_eq(&old.layout, &app.spread_display_units_cache.units),
        "rebuild is lazy"
    );
    let after = app.spread_display_composition_for_anchor(1);
    assert!(
        !Arc::ptr_eq(&old.layout, &app.spread_display_units_cache.units),
        "ordinary resolver rebuilt the book layout"
    );
    assert_eq!(before, after, "turning occurrences and placement unchanged");
    let old_unit = old.layout.iter().find(|u| u.anchor_idx() == 1).unwrap();
    let new_unit = app
        .spread_display_units_cache
        .units
        .iter()
        .find(|u| u.anchor_idx() == 1)
        .unwrap();
    assert_eq!(old_unit.pages, new_unit.pages);
    assert_eq!(old_unit.formation, new_unit.formation);
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
    if !continued {
        assert_eq!(
            differences.len(),
            1,
            "must fail at the hypothesized cancellation, not fixture setup: {differences:?}"
        );
        assert_eq!(
            differences[0]["origin"],
            if native {
                "prepare_reuse"
            } else {
                "turn_compare"
            }
        );
        assert_eq!(
            differences[0]["different_fields"],
            serde_json::json!(["layout_arc"])
        );
        assert_eq!(
            differences[0]["layout"],
            serde_json::json!({"occurrences_equal":true, "formation_equal":true, "placement_equal":true})
        );
        if !native {
            let PresentationState::Flat(current) = &app.page_turn_presentation.state else {
                panic!("flat after compare cancel");
            };
            assert_eq!(
                old.different_fields(current),
                vec!["layout_arc"],
                "all compared material/geometry/content fields unchanged"
            );
        }
    }
    eprintln!("§1.750 native={native} mode={mode:?}: {differences:?}");
    assert!(
        continued,
        "§1.750: unrelated far-page layout rebuild must preserve curl; {differences:?}"
    );
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    // The fixed implementation must reach the canonical flat landing with
    // the original immutable material; this is deliberately unreachable today.
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
