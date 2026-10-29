//! Wait indication follows accepted presentation without retiring navigation early.
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

fn indicator_visible(app: &App, now: Instant) -> bool {
    fs_page_wait_indicator_visible(
        true,
        app.fs_navigation_target_presented_by_page_turn(),
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
        assert!(
            !app.fs_navigation_target_presented_by_page_turn(),
            "Offered"
        );
        assert!(indicator_visible(&app, waiting_at));
        frame(&ctx, |ui, ctx| {
            assert!(draw(&mut app, ui, ctx).curl);
        });
        assert!(
            !app.fs_navigation_target_presented_by_page_turn(),
            "clock not adopted yet"
        );
        clock.set(origin + Duration::from_millis(550));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(draw(&mut app, ui, ctx).curl);
            assert!(app.fs_navigation_target_presented_by_page_turn());
            assert!(!indicator_visible(
                &app,
                origin + Duration::from_millis(550)
            ));
            assert!(app.fs_navigation_sequence_blocks_new_target());
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning {
                clock: MotionClock::Running { .. },
                ..
            }
        ));
        assert!(
            app.fs_holdover_tex
                .as_ref()
                .and_then(FsHoldover::navigation_sequence)
                .is_some()
        );

        clock.set(origin + Duration::from_millis(601));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(!draw(&mut app, ui, ctx).curl);
            assert!(
                app.fs_navigation_sequence_blocks_new_target(),
                "flat not accepted yet"
            );
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Completing { .. }
        ));
        assert!(
            app.fs_navigation_target_presented_by_page_turn(),
            "accepted landing"
        );
        assert!(!indicator_visible(&app, waiting_at));
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(
                !app.fs_navigation_sequence_blocks_new_target(),
                "canonical flat retires sequence"
            );
            assert!(app.fs_holdover_tex.is_none());
            assert!(!draw(&mut app, ui, ctx).curl);
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
        assert!(draw(&mut app, ui, ctx).curl);
    });
    assert!(!indicator_visible(&app, waiting_at));

    let holdover = app.fs_holdover_tex.clone().unwrap();
    let FsHoldover::NavigationSequence(sequence) = app.fs_holdover_tex.as_mut().unwrap() else {
        panic!("navigation sequence")
    };
    let FsNavigationSequenceTarget::Display(target) = &mut sequence.target else {
        panic!("display target")
    };
    target.phase = FsNavigationTargetPhase::awaiting_navigation(2, vec![2, 99]);
    assert!(
        !app.fs_navigation_target_presented_by_page_turn(),
        "one spread page missing"
    );
    assert!(indicator_visible(&app, waiting_at));
    app.fs_holdover_tex = Some(holdover);

    // A unit retained for a different current anchor cannot explain this target.
    app.fullscreen_idx = Some(0);
    assert!(!app.fs_navigation_target_presented_by_page_turn());
    app.fullscreen_idx = Some(2);
    app.page_turn_presentation.cancel("page-wait-test");
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Completing { .. }
    ));
    assert!(
        app.fs_navigation_target_presented_by_page_turn(),
        "cancel retains accepted incoming"
    );
    assert!(!indicator_visible(&app, waiting_at));
    assert!(app.fs_navigation_sequence_blocks_new_target());
    // Presentation remains an independent fact even if a held page is no
    // longer Live for the navigation observer. Exercise accepted Flat as well.
    let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("cancelled incoming")
    };
    let mut nonlive = (**incoming).clone();
    for page in &mut nonlive.pages {
        page.live = false;
    }
    let completing = std::mem::replace(
        &mut app.page_turn_presentation.state,
        PresentationState::Flat(Arc::new(nonlive.clone())),
    );
    assert!(
        app.fs_navigation_target_presented_by_page_turn(),
        "accepted non-Live Flat"
    );
    assert!(!indicator_visible(&app, waiting_at));
    assert!(app.fs_navigation_sequence_blocks_new_target());
    nonlive.evidence = AcceptanceEvidence::Deferred(Arc::new(AtomicBool::new(false)));
    app.page_turn_presentation.state = PresentationState::Flat(Arc::new(nonlive));
    assert!(
        !app.fs_navigation_target_presented_by_page_turn(),
        "unaccepted Flat"
    );
    assert!(indicator_visible(&app, waiting_at));
    app.page_turn_presentation.state = completing;
    {
        let app: &mut App = &mut app;
        app.page_turn_presentation
            .cancel_for_settings(&mut app.page_turn_leases);
    }
    assert!(
        !app.fs_navigation_target_presented_by_page_turn(),
        "new settings require new acceptance"
    );
    assert!(indicator_visible(&app, waiting_at));
    app.page_turn_presentation.deactivate("page-wait-test");
    assert!(
        indicator_visible(&app, waiting_at),
        "Inactive does not hide a real wait"
    );
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
    assert!(
        !app.page_turn_current_unit_presented(),
        "retained output belongs to the old unit"
    );
    assert!(!app.fs_navigation_target_presented_by_page_turn());
    assert!(indicator_visible(&app, origin + Duration::from_secs(1)));
    assert!(app.fs_navigation_sequence_blocks_new_target());
}
