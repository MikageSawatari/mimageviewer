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

fn seed_offer(
    ctx: &egui::Context,
    mode: SpreadMode,
    origin: Instant,
) -> crate::app::AppTestEnvForTest {
    install(ctx);
    let mut app = ready_app(ctx, mode);
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
                !indicator_visible(&app, ctx, &sources, waiting_at),
                "same-pass initial curl"
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
