//! Wait indication follows this pass's turn/landing output, not retained history.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> FsNavigatorTextureSources {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    sources
}

fn receipt(sources: &FsNavigatorTextureSources) -> &SubmittedUnitReceipt {
    let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
        panic!("paged output")
    };
    receipt
}

fn indicator_visible(
    app: &App,
    ctx: &egui::Context,
    sources: &FsNavigatorTextureSources,
    now: Instant,
) -> bool {
    fs_page_wait_indicator_visible(
        true,
        app.fs_navigation_target_presented_by_page_turn(ctx, sources),
        app.fs_holdover_tex.as_ref(),
        app.fullscreen_idx.unwrap(),
        app.items_generation,
        now,
    )
}

/// D2: use ordinary cached-source preparation, navigation and output acceptance.
/// Both pages exceed the viewport at 100%, so the reading edge hides its sibling.
fn cached_original_navigation_retires(
    mode: SpreadMode,
    position: crate::settings::FullscreenPageStartPosition,
    manual_pan_before_navigation: bool,
    provisional_before_complete: bool,
) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, mode);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.fullscreen_page_start_position = position;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    let size = if mode == SpreadMode::SplitRtl {
        [1440, 1020]
    } else {
        [720, 1020]
    };
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("d2-cached-{idx}"),
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
                source_dims: Some(size),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
    }
    app.reconcile_fullscreen_page_slice(0);
    app.request_fs_initial_placement(0);
    frame(&ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        draw_position_current(&mut app, ui, ctx);
    });
    if manual_pan_before_navigation {
        app.set_fs_pan_from_input(1.0, egui::vec2(-500.0, 310.0));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            draw_position_current(&mut app, ui, ctx);
        });
    }
    let target = if mode.is_spread() { 2 } else { 1 };
    frame(&ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "d2-cached-navigation",
        );
        let nav = if mode == SpreadMode::SplitRtl {
            FsPageNav::Split(crate::page_split::PresentationStep {
                source_idx: target,
                slice: crate::page_split::PageSlice::Right,
            })
        } else {
            FsPageNav::Target(target)
        };
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            nav,
            None,
            0,
            Some(operation),
        );
    });
    assert_eq!(app.fullscreen_idx, Some(target));
    let FsHoldover::NavigationSequence(sequence) =
        app.fs_holdover_tex.as_mut().expect("accepted navigation")
    else {
        panic!("navigation sequence");
    };
    sequence.opened_at = origin;
    let pan_after_open = app.fs_pan;
    if provisional_before_complete {
        // The accepted ready target loses its full caches before painting. Select
        // real thumbnail fallbacks, then publish the completed sources next frame.
        let target_pages = app
            .spread_display_composition_for_anchor(target)
            .pages_in_screen_order(mode);
        let mut arriving = Vec::new();
        for page in &target_pages {
            let idx = page.idx;
            arriving.push((idx, app.fs_cache.remove(&idx).unwrap()));
            let texture = ctx.load_texture(
                format!("d2-provisional-{idx}"),
                egui::ColorImage::filled([150, 200], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            );
            app.thumbnails[idx] = ThumbnailState::loaded_raster(
                texture,
                crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                    evaluated_display_px: 200,
                },
                false,
                200,
                crate::source_coverage::SourceCoverage::FullSource,
                Some((size[0] as u32, size[1] as u32)),
                None,
            );
        }
        app.edit_result_cache.clear();
        app.final_composite_cache.clear();
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.page_turn_pre_draw(ctx);
            let sources = draw_position_current(&mut app, ui, ctx);
            assert_eq!(sources.pages.len(), target_pages.len());
            assert!(
                sources
                    .pages
                    .iter()
                    .all(|page| page.resource.page_material_kind()
                        == crate::gpu_lanczos::FullscreenPageMaterialKind::Provisional)
            );
            assert!(
                app.fs_initial_placement_request.is_some(),
                "actual provisional paint retains placement"
            );
            assert!(
                app.fs_holdover_tex.is_some(),
                "output not accepted until the end of this pass"
            );
        });
        for (idx, entry) in arriving {
            app.fs_cache.insert(idx, entry);
        }
    }
    let mut last_sources = None;
    for n in 1..=6 {
        clock.set(origin + Duration::from_secs(n));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.page_turn_pre_draw(ctx);
            last_sources = Some(draw_position_current(&mut app, ui, ctx));
            assert!(
                app.fs_initial_placement_request.is_none(),
                "complete source commits placement"
            );
            if provisional_before_complete && n == 1 {
                assert!(
                    app.fs_holdover_tex.is_some(),
                    "the old thumbnail receipt cannot retire the newly completed sources before their output is accepted"
                );
            }
        });
    }
    let sources = last_sources.unwrap();
    let proof = receipt(&sources);
    assert!(proof.was_accepted());
    assert!(proof.matches_sources(app.page_turn_destination(), &sources));
    assert_eq!(sources.pages.len(), if mode.is_spread() { 2 } else { 1 });
    assert!(
        sources
            .pages
            .iter()
            .all(|page| page.provenance == FsDisplayUnitPageProvenance::Live)
    );
    if position == crate::settings::FullscreenPageStartPosition::ReadingStart && mode.is_spread() {
        assert!(
            proof.pages().iter().any(|(_, _, _, area)| *area == 0.0),
            "the regression must really submit an offscreen sibling"
        );
    }
    eprintln!(
        "D2 {mode:?} {position:?} manual_before={manual_pan_before_navigation} provisional={provisional_before_complete}: pan_after_open={pan_after_open:?} pan={:?} receipt_accepted={} receipt_matches={} receipt_pages={:?} provenance={:?} holdover={} pending={}",
        app.fs_pan,
        proof.was_accepted(),
        proof.matches_sources(app.page_turn_destination(), &sources),
        proof.pages(),
        sources
            .pages
            .iter()
            .map(|p| p.provenance)
            .collect::<Vec<_>>(),
        app.fs_holdover_tex.is_some(),
        app.pending_fs_page_turn_ready.is_some()
    );
    assert!(
        app.fs_holdover_tex.is_none(),
        "cached target must retire navigation after accepted flat output"
    );
    assert!(!app.fs_navigation_sequence_blocks_new_target());
    assert!(!indicator_visible(
        &app,
        &ctx,
        &sources,
        origin + Duration::from_secs(20)
    ));
}

#[test]
fn initial_placement_cached_original_rtl_spread_navigation_retires_wait() {
    cached_original_navigation_retires(
        SpreadMode::Rtl,
        crate::settings::FullscreenPageStartPosition::ReadingStart,
        false,
        false,
    );
}

#[test]
fn page_wait_cached_original_rtl_spread_center_navigation_retires() {
    cached_original_navigation_retires(
        SpreadMode::Rtl,
        crate::settings::FullscreenPageStartPosition::Center,
        false,
        false,
    );
}

#[test]
fn page_wait_cached_original_rtl_spread_center_manual_pan_before_navigation_retires() {
    cached_original_navigation_retires(
        SpreadMode::Rtl,
        crate::settings::FullscreenPageStartPosition::Center,
        true,
        false,
    );
}

#[test]
fn initial_placement_cached_original_ltr_spread_navigation_retires_wait() {
    cached_original_navigation_retires(
        SpreadMode::Ltr,
        crate::settings::FullscreenPageStartPosition::ReadingStart,
        false,
        false,
    );
}

#[test]
fn initial_placement_cached_original_single_navigation_retires_wait() {
    cached_original_navigation_retires(
        SpreadMode::Single,
        crate::settings::FullscreenPageStartPosition::ReadingStart,
        false,
        false,
    );
}

#[test]
fn initial_placement_cached_original_split_navigation_retires_wait() {
    cached_original_navigation_retires(
        SpreadMode::SplitRtl,
        crate::settings::FullscreenPageStartPosition::ReadingStart,
        false,
        false,
    );
}

#[test]
fn initial_placement_provisional_then_complete_spread_navigation_retires_wait() {
    cached_original_navigation_retires(
        SpreadMode::Rtl,
        crate::settings::FullscreenPageStartPosition::ReadingStart,
        false,
        true,
    );
}

fn draw_position_current(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsNavigatorTextureSources {
    let idx = app.fullscreen_idx.unwrap();
    let decision = app.fs_page_turn_decision_for_frame(ctx, idx);
    assert!(
        app.fs_nav_holdover_for_draw().is_none(),
        "cached/accepted target is current"
    );
    let composition = app.spread_display_composition_for_anchor(idx);
    let screen = composition.pages_in_screen_order(app.spread_mode);
    if screen.len() == 2 {
        return draw(app, ui, ctx);
    }
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let payload = app.fullscreen_paint_payload_for_page(idx, material, false);
    let thumbnail = app.fs_thumbnail_resource_for_display(idx).ok();
    let source_size = app.fs_page_coordinate_source_size(idx);
    let zoom_pan = app.fs_zoom_pan();
    let fit = app.effective_fullscreen_fit_mode();
    let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
    let live = app.page_turn_view_eligible(idx);
    let mut sources = FsNavigatorTextureSources::default();
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            ui.max_rect(),
            idx,
            source_size,
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            composition.singleton_placement(),
            payload.as_ref(),
            thumbnail.as_ref(),
            false,
            false,
            false,
            crate::rotation_db::Rotation::None,
            zoom_pan,
            0.0,
            &FsBgStyle::Solid(egui::Color32::WHITE),
            "",
            false,
            fit,
            limits,
            None,
            Some(&mut sources),
            live,
            None,
            FlatCropPaint::WithPage,
            true,
        )
        .expect("ready single or split page");
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout
        .push_occurrence(transform, screen[0]);
    assert!(sources.rebind_occurrences(&screen, || {
        screen
            .first()
            .and_then(|o| app.page_turn_unit_binding(o.navigation_anchor_idx))
    }));
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    sources
}

fn seed_offer(
    ctx: &egui::Context,
    mode: SpreadMode,
    origin: Instant,
) -> crate::app::AppTestEnvForTest {
    seed_offer_in_app(ctx, ready_app(ctx, mode), origin)
}

fn seed_offer_in_app(
    ctx: &egui::Context,
    mut app: crate::app::AppTestEnvForTest,
    origin: Instant,
) -> crate::app::AppTestEnvForTest {
    install(ctx);
    let mode = app.spread_mode;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    frame(ctx, |ui, ctx| {
        draw(&mut app, ui, ctx);
    });
    let target = if mode.is_spread() { 2 } else { 1 };
    frame(ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "page-wait-test");
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(target),
            None,
            0,
            Some(operation),
        );
    });
    assert_eq!(app.fullscreen_idx, Some(target));
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Offered { .. }
    ));
    // Navigation uses the real monotonic clock; align its delay anchor with the
    // injected motion clock without sleeping or depending on test load.
    let FsHoldover::NavigationSequence(sequence) = app.fs_holdover_tex.as_mut().unwrap() else {
        panic!("navigation sequence")
    };
    sequence.opened_at = origin;
    app
}

#[test]
fn page_wait_indicator_appears_while_initial_spread_output_is_unaccepted_and_incoming_occluded() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Rtl);
    app.settings.spread_page_gap_px = 0;
    // RTL forward turns the left leaf. A wide outgoing left leaf fully covers
    // the narrower incoming left leaf at p=0, as in the width-swap regression.
    for idx in [0, 3] {
        let pixels = Arc::new(egui::ColorImage::filled([40, 120], egui::Color32::GREEN));
        let tex = ctx.load_texture(
            format!("initial-occluded-{idx}"),
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
                source_dims: Some([40, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (40, 120));
        app.record_page_layout_dims_for_spread(idx, (40, 120));
    }
    let mut app = seed_offer_in_app(&ctx, app, origin);
    let first_output_at = origin + Duration::from_millis(750);
    clock.set(first_output_at);
    frame(&ctx, |ui, ctx| {
        let sources = draw(&mut app, ui, ctx);
        assert!(receipt(&sources).curl);
        assert!(!receipt(&sources).was_accepted());
        let PresentationState::Turning {
            clock,
            projection,
            offer,
            ..
        } = &app.page_turn_presentation.state
        else {
            panic!("initial turn")
        };
        assert_eq!(*clock, MotionClock::AwaitingAcceptedOutput);
        assert_eq!(clock.progress(first_output_at, offer.duration), 0.0);
        let initial = crate::page_turn_animation::project_prepared(
            &projection.outgoing,
            &projection.incoming,
            projection.direction,
            projection.recipe,
            0.0,
            false,
        )
        .unwrap();
        assert!(
            initial
                .visibility
                .incoming
                .iter()
                .all(|page| page.visible_area == 0.0),
            "the regression must actually hide every incoming page"
        );
        assert!(initial.visibility.outgoing_front_area > 0.0);
        assert!(
            indicator_visible(&app, ctx, &sources, first_output_at),
            "unaccepted p=0 still shows only the old spread after a 750ms wait"
        );
        assert!(app.fs_navigation_sequence_blocks_new_target());
    });
    clock.set(first_output_at + Duration::from_millis(1));
    frame(&ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let sources = draw(&mut app, ui, ctx);
        assert!(receipt(&sources).curl);
        assert!(matches!(app.page_turn_presentation.state,
            PresentationState::Turning { clock: MotionClock::Running { start_at }, .. }
                if start_at == first_output_at));
        assert!(
            !indicator_visible(
                &app,
                ctx,
                &sources,
                first_output_at + Duration::from_millis(1)
            ),
            "a running curl is presentation without a per-page visibility threshold"
        );
        assert!(app.fs_navigation_sequence_blocks_new_target());
    });
}

#[test]
fn page_wait_indicator_hidden_during_600ms_turn_without_retiring_navigation_until_flat() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let origin = Instant::now();
        let clock = super::clock_tests::TestClock::at(origin);
        let ctx = egui::Context::default();
        let mut app = seed_offer(&ctx, mode, origin);
        let waiting_at = origin + Duration::from_secs(1);
        let mut first = None;
        frame(&ctx, |ui, ctx| {
            let absent = FsNavigatorTextureSources::default();
            assert!(
                indicator_visible(&app, ctx, &absent, waiting_at),
                "Offered is not output"
            );
            let sources = draw(&mut app, ui, ctx);
            assert!(receipt(&sources).curl);
            assert!(!receipt(&sources).was_accepted());
            assert!(
                indicator_visible(&app, ctx, &sources, waiting_at),
                "initial curl is still awaiting accepted output"
            );
            first = Some(sources);
        });
        clock.set(origin + Duration::from_millis(550));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                indicator_visible(&app, ctx, first.as_ref().unwrap(), waiting_at),
                "old frame is not current output"
            );
            let sources = draw(&mut app, ui, ctx);
            assert!(receipt(&sources).curl);
            assert!(!indicator_visible(
                &app,
                ctx,
                &sources,
                origin + Duration::from_millis(550)
            ));
            assert!(app.fs_navigation_sequence_blocks_new_target());
            let mut old_pass = sources.clone();
            let FsDisplayUnitSubmission::Paged(old) = &mut old_pass.submission else {
                unreachable!()
            };
            old.pass = old.pass.checked_sub(1).unwrap();
            assert!(indicator_visible(&app, ctx, &old_pass, waiting_at));
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning {
                clock: MotionClock::Running { .. },
                ..
            }
        ));
        let mut landing = None;
        clock.set(origin + Duration::from_millis(601));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let sources = draw(&mut app, ui, ctx);
            assert!(!receipt(&sources).curl);
            assert!(!receipt(&sources).was_accepted(), "output hook has not run");
            assert!(
                !indicator_visible(&app, ctx, &sources, origin + Duration::from_millis(601)),
                "landing must hide the badge before the output hook accepts its flat receipt"
            );
            assert!(
                app.fs_navigation_sequence_blocks_new_target(),
                "flat not accepted yet"
            );
            landing = Some(sources);
        });
        assert!(receipt(landing.as_ref().unwrap()).was_accepted());
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                app.fs_holdover_tex.is_none(),
                "canonical flat retires sequence"
            );
            let sources = draw(&mut app, ui, ctx);
            assert!(!receipt(&sources).curl);
            assert!(
                !app.fs_navigation_target_presented_by_page_turn(ctx, &sources),
                "ordinary Flat is not animation output"
            );
            assert!(!indicator_visible(&app, ctx, &sources, waiting_at));
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Flat(_)
        ));
    }
}

#[test]
fn page_wait_indicator_keeps_unpresented_targets_visible_and_tracks_cancelled_presentation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    let mut app = seed_offer(&ctx, SpreadMode::Ltr, origin);
    let waiting_at = origin + Duration::from_secs(1);
    frame(&ctx, |ui, ctx| {
        draw(&mut app, ui, ctx);
    });
    clock.set(origin + Duration::from_millis(550));
    frame(&ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let sources = draw(&mut app, ui, ctx);
        assert!(!indicator_visible(&app, ctx, &sources, waiting_at));
        let holdover = app.fs_holdover_tex.clone().unwrap();
        let FsHoldover::NavigationSequence(sequence) = app.fs_holdover_tex.as_mut().unwrap() else {
            unreachable!()
        };
        let FsNavigationSequenceTarget::Display(target) = &mut sequence.target else {
            unreachable!()
        };
        target.phase = FsNavigationTargetPhase::awaiting_navigation(2, vec![2, 99]);
        assert!(
            indicator_visible(&app, ctx, &sources, waiting_at),
            "missing spread page"
        );
        app.fs_holdover_tex = Some(holdover);
        app.fullscreen_idx = Some(0);
        assert!(!app.fs_navigation_target_presented_by_page_turn(ctx, &sources));
        app.fullscreen_idx = Some(2);
        let mut unbound = sources.clone();
        let FsDisplayUnitSubmission::Paged(unbound_receipt) = &mut unbound.submission else {
            unreachable!()
        };
        unbound_receipt.binding = SubmittedContentBinding::Ordinary;
        assert!(indicator_visible(&app, ctx, &unbound, waiting_at));
        let mut wrong_paint = sources.clone();
        let FsDisplayUnitSubmission::Paged(wrong_receipt) = &mut wrong_paint.submission else {
            unreachable!()
        };
        wrong_receipt.pages.as_mut_slice()[0].2 = egui::TextureId::Managed(u64::MAX);
        assert!(indicator_visible(&app, ctx, &wrong_paint, waiting_at));
        let mut legacy = sources.clone();
        legacy.submission = FsDisplayUnitSubmission::LegacyRendered;
        assert!(indicator_visible(&app, ctx, &legacy, waiting_at));
        app.page_turn_presentation.cancel("page-wait-test");
    });
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Completing { .. }
    ));
    frame(&ctx, |ui, ctx| {
        let absent = FsNavigatorTextureSources::default();
        assert!(
            indicator_visible(&app, ctx, &absent, waiting_at),
            "Completing memory alone is insufficient"
        );
        // A non-Live completion still owns this pass's exact incoming output.
        let PresentationState::Completing { incoming, .. } = &mut app.page_turn_presentation.state
        else {
            unreachable!()
        };
        for page in &mut Arc::make_mut(incoming).pages {
            page.live = false;
        }
        let sources = draw(&mut app, ui, ctx);
        assert!(!receipt(&sources).curl);
        assert!(!indicator_visible(&app, ctx, &sources, waiting_at));
        assert!(app.fs_navigation_sequence_blocks_new_target());
    });
    {
        let app: &mut App = &mut app;
        app.page_turn_presentation
            .cancel_for_settings(&mut app.page_turn_leases);
    }
    frame(&ctx, |_, ctx| {
        assert!(indicator_visible(
            &app,
            ctx,
            &FsNavigatorTextureSources::default(),
            waiting_at
        ));
        app.page_turn_presentation.deactivate("page-wait-test");
        assert!(indicator_visible(
            &app,
            ctx,
            &FsNavigatorTextureSources::default(),
            waiting_at
        ));
    });
}

#[test]
fn page_wait_indicator_appears_after_offered_turn_cancels_back_to_the_old_flat_unit() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let _clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    let mut app = seed_offer(&ctx, SpreadMode::Single, origin);
    app.page_turn_presentation.cancel("page-wait-test");
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Flat(_)
    ));
    frame(&ctx, |_, ctx| {
        assert!(indicator_visible(
            &app,
            ctx,
            &FsNavigatorTextureSources::default(),
            origin + Duration::from_secs(1)
        ));
        assert!(app.fs_navigation_sequence_blocks_new_target());
    });
}

#[test]
fn page_wait_indicator_appears_for_holdover_after_returning_to_a_stale_flat_unit() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let _clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    frame(&ctx, |ui, ctx| {
        draw(&mut app, ui, ctx);
    });
    assert!(app.page_turn_current_unit_presented());
    let saved_flat = app.page_turn_gpu_fingerprint_for_test();
    // Production captured/non-live fallback paints B without replacing Flat(A),
    // as in an ineligible comparison fallback. Its receipt has no presentation binding.
    app.fullscreen_idx = Some(1);
    let page_b = app.capture_fs_display_unit(1).unwrap();
    frame(&ctx, |ui, ctx| {
        let sources = app.draw_fs_display_unit_holdover_with_geometry(
            ui,
            ctx,
            ui.max_rect(),
            &page_b,
            StillSeekGeometry::bar_only(),
        );
        assert!(!receipt(&sources).has_presentation_binding());
        assert_eq!(sources.pages[0].idx(), 1);
    });
    assert_eq!(app.page_turn_gpu_fingerprint_for_test(), saved_flat);
    app.fs_cache.remove(&0);
    app.edit_result_cache.clear();
    app.final_composite_cache.clear();
    assert!(app.begin_fs_page_navigation_sequence(&ctx, 1, 0, false));
    app.fullscreen_idx = Some(0);
    let FsHoldover::NavigationSequence(sequence) = app.fs_holdover_tex.as_mut().unwrap() else {
        unreachable!()
    };
    sequence.opened_at = origin;
    assert!(
        app.page_turn_current_unit_presented(),
        "stale Flat(A) still matches all identities"
    );
    frame(&ctx, |ui, ctx| {
        let holdover = app.fs_nav_holdover_for_draw().expect("A is still loading");
        assert_eq!(holdover.pages[0].idx(), 1);
        let sources = app.draw_fs_display_unit_holdover_with_geometry(
            ui,
            ctx,
            ui.max_rect(),
            &holdover,
            StillSeekGeometry::bar_only(),
        );
        assert_eq!(sources.pages[0].idx(), 1);
        assert!(!receipt(&sources).has_presentation_binding());
        assert!(
            indicator_visible(&app, ctx, &sources, origin + Duration::from_secs(1)),
            "B holdover must explain the new wait for A"
        );
        assert!(app.fs_navigation_sequence_blocks_new_target());
    });
}

#[test]
fn page_wait_indicator_ordinary_flat_keeps_one_nonlive_spread_page_wait_visible() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let _clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    app.settings.page_turn_animation_enabled = true;
    frame(&ctx, |ui, ctx| {
        draw(&mut app, ui, ctx);
    });
    assert!(app.begin_fs_page_navigation_sequence(&ctx, 2, 0, false));
    let FsHoldover::NavigationSequence(sequence) = app.fs_holdover_tex.as_mut().unwrap() else {
        unreachable!()
    };
    sequence.opened_at = origin;
    for fully_live in [true, false] {
        frame(&ctx, |ui, ctx| {
            let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
                unreachable!()
            };
            let mut pages = unit.pages.clone();
            assert_eq!(pages.len(), 2);
            for page in &mut pages {
                page.painter = ui.painter().clone();
            }
            pages[1].live = fully_live;
            let receipt = app.submit_paged_display_unit(ctx, &pages, None).unwrap();
            assert_eq!(receipt.has_presentation_binding(), fully_live);
            let sources =
                FsNavigatorTextureSources::from_prepared(ctx, &pages, Some(receipt), fully_live);
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Flat(_)
            ));
            assert!(
                !app.fs_navigation_target_presented_by_page_turn(ctx, &sources),
                "ordinary Flat never replaces the observer"
            );
            assert!(indicator_visible(
                &app,
                ctx,
                &sources,
                origin + Duration::from_secs(1)
            ));
            assert!(app.fs_navigation_sequence_blocks_new_target());
        });
    }
}
