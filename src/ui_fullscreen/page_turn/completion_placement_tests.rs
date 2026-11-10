//! Complete flat output includes prepared occurrences clipped out by a new pan.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("paged output")
    };
    receipt
}

fn cancelled_offscreen_flat_resumes_navigation(discard_first_flat: bool) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let origin = Instant::now();
        let clock = super::clock_tests::TestClock::at(origin);
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
        app.settings.page_turn_animation_enabled = true;
        app.settings.page_turn_animation_duration_ms = 600;
        app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
        for idx in 0..4 {
            let pixels = Arc::new(egui::ColorImage::filled([400, 600], egui::Color32::WHITE));
            let tex = ctx.load_texture(
                format!("completion-placement-{idx}"),
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels,
                    source_dims: Some([400, 600]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.record_page_dims_for_spread(idx, (400, 600));
        }
        // The incoming turn starts from completed final materials, not raw
        // fallbacks which ordinary preparation could replace during the test.
        for anchor in [2, 0] {
            app.fullscreen_idx = Some(anchor);
            frame(&ctx, |ui, ctx| {
                draw(&mut app, ui, ctx);
            });
        }
        frame(&ctx, |_, ctx| {
            let op = ViewerOperationId::Pad {
                batch: next_operation_batch(),
                ordinal: 0,
            };
            app.page_turn_external_operation(
                ctx,
                op,
                OperationPhase::Press,
                "completion-placement",
            );
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(2),
                None,
                0,
                Some(op),
            );
            let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
                panic!("real offer")
            };
            assert!(
                offer
                    .ready_sources
                    .0
                    .iter()
                    .all(|source| source.resource.page_material_kind()
                        == FullscreenPageMaterialKind::Complete)
            );
        });
        frame(&ctx, |ui, ctx| {
            assert!(draw(&mut app, ui, ctx).curl);
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning { .. }
        ));
        clock.set(origin + Duration::from_millis(100));
        let mut flat_passes = Vec::new();
        let next_operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.fs_zoom = 3.0;
            app.set_fs_pan_from_input(3.0, egui::vec2(-300.0, 0.0));
            // Frame entry can already cancel the running turn on input. The
            // ordinary flat must complete either that cancellation or the
            // renderer's ineligible-view cancellation below.
            let receipt = draw(&mut app, ui, ctx);
            assert!(!receipt.curl);
            assert!(receipt.pages().iter().any(|(_, _, _, area)| *area == 0.0));
            assert!(receipt.pages().iter().any(|(_, _, _, area)| *area > 0.0));
            assert!(app.page_turn_completion_active());
            // Cancellation happens after this pass's ordinary preparation.
            // Completing refreshes a changed native paint/geometry on its next
            // draw; the first receipt need not match the old incoming paint ID.
            app.page_turn_external_operation(
                ctx,
                next_operation,
                OperationPhase::Press,
                "completion-placement-next",
            );
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(0),
                None,
                0,
                Some(next_operation),
            );
            assert_eq!(
                app.fullscreen_idx,
                Some(2),
                "the unadopted flat still owns navigation"
            );
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Completing {
                    continuation: Some(_),
                    ..
                }
            ));
            flat_passes.push(receipt);
            if discard_first_flat && ctx.current_pass_index() == 0 {
                ctx.request_discard("completion-placement-test");
            }
        });
        if discard_first_flat {
            assert_eq!(flat_passes.len(), 2, "discard replays the same frame");
            assert!(!flat_passes[0].was_accepted());
            assert!(flat_passes[1].was_accepted());
        }
        let mut resumed = false;
        for _ in 0..4 {
            let mut canonical = None;
            frame(&ctx, |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                if app.fullscreen_idx == Some(0) {
                    assert!(!app.page_turn_barrier_active());
                    resumed = true;
                    draw(&mut app, ui, ctx);
                    return;
                }
                assert_eq!(app.fullscreen_idx, Some(2));
                let receipt = draw(&mut app, ui, ctx);
                assert!(!receipt.curl);
                assert!(receipt.pages().iter().any(|(_, _, _, area)| *area == 0.0));
                assert!(
                    matches!(
                        app.page_turn_presentation.state,
                        PresentationState::Completing {
                            completion: FlatCompletion::Submitted(_),
                            ..
                        }
                    ),
                    "the fresh offscreen canonical flat must be recorded, not stay at NeedFlat"
                );
                canonical = Some(receipt);
            });
            if resumed {
                break;
            }
            let receipt = canonical.unwrap();
            assert!(receipt.was_accepted());
        }
        assert!(
            resumed,
            "adopted offscreen flat must release the queued navigation"
        );
    }
}

#[test]
fn initial_placement_cancelled_spread_offscreen_flat_releases_navigation_barrier() {
    cancelled_offscreen_flat_resumes_navigation(false);
}

#[test]
fn initial_placement_discarded_offscreen_completion_replay_releases_navigation_barrier() {
    cancelled_offscreen_flat_resumes_navigation(true);
}
