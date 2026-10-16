//! Fake monotonic time at accept, projection and accepted-output boundaries.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};
use std::cell::Cell;

thread_local! {
    static NOW: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub(super) fn now() -> Option<Instant> {
    NOW.with(Cell::get)
}

pub(super) struct TestClock(Option<Instant>);
impl TestClock {
    pub(super) fn at(now: Instant) -> Self {
        Self(NOW.replace(Some(now)))
    }
    pub(super) fn set(&self, now: Instant) {
        NOW.set(Some(now));
    }
}
impl Drop for TestClock {
    fn drop(&mut self) {
        NOW.set(self.0);
    }
}

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("paged receipt")
    };
    receipt
}

#[test]
fn delayed_prepare_starts_at_zero_then_finishes_by_wall_time_after_first_accepted_output() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let accepted_at = Instant::now();
        let clock = TestClock::at(accepted_at);
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.page_turn_animation_enabled = true;
        app.settings.page_turn_animation_duration_ms = 200;
        app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
        frame(&ctx, |ui, ctx| {
            draw(&mut app, ui, ctx);
        });
        let target = if mode.is_spread() { 2 } else { 1 };
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        frame(&ctx, |_, ctx| {
            app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "clock-test");
            // The real navigation handler creates the offer before ordinary prepare.
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                if mode.is_spread() {
                    FsPageNav::Target(target)
                } else {
                    FsPageNav::Delta(1)
                },
                None,
                0,
                Some(operation),
            );
            assert_eq!(app.fullscreen_idx, Some(target));
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Offered { .. }
            ));
        });
        let prepared_at = accepted_at + Duration::from_secs(2);
        let first_output_at = prepared_at + Duration::from_millis(80);
        clock.set(prepared_at);
        let first = frame(&ctx, |ui, ctx| {
            assert!(
                draw(&mut app, ui, ctx).curl,
                "preparation must not expire the offer"
            );
            // Later widgets/backend-independent end-pass work can take time
            // after projection. Only the output hook may choose the origin.
            clock.set(first_output_at);
        });
        let (tracker, expected) = match &app.page_turn_presentation.state {
            PresentationState::Turning {
                offer,
                projection,
                clock,
                ..
            } => {
                assert_eq!(offer.accepted_at, accepted_at);
                assert_eq!(
                    *clock,
                    MotionClock::AwaitingAcceptedOutput,
                    "the hook publishes; it does not mutate UI state"
                );
                let expected = crate::page_turn_animation::project_prepared(
                    &projection.outgoing,
                    &projection.incoming,
                    projection.direction,
                    projection.recipe,
                    0.0,
                    false,
                )
                .unwrap();
                (offer.submission.clone(), expected)
            }
            _ => panic!("turning"),
        };
        assert_eq!(tracker.started_at(), Some(first_output_at));
        // Inspect actual submitted shapes against the p=0 recipe, including the
        // initial lift. This catches an accepted_at-based pose despite a new clock.
        for expected in expected.meshes {
            assert!(
                first.shapes.iter().any(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) =>
                        mesh.as_ref() == &expected.mesh && shape.clip_rect == expected.clip_rect,
                    _ => false,
                }),
                "first submitted mesh must be the initial pose"
            );
        }
        let samples: &[u64] = if mode == SpreadMode::Single {
            &[]
        } else {
            &[50, 150, 199]
        };
        for &elapsed in samples {
            clock.set(first_output_at + Duration::from_millis(elapsed));
            frame(&ctx, |ui, ctx| {
                assert!(draw(&mut app, ui, ctx).curl);
            });
            let PresentationState::Turning { clock, .. } = &app.page_turn_presentation.state else {
                panic!("turning")
            };
            assert_eq!(
                *clock,
                MotionClock::Running {
                    start_at: first_output_at
                }
            );
            assert_eq!(
                tracker.started_at(),
                Some(first_output_at),
                "later outputs never reset the origin"
            );
        }
        // A hitch does not pause the wall clock or require a minimum frame count.
        clock.set(first_output_at + Duration::from_secs(1));
        frame(&ctx, |ui, ctx| {
            assert!(!draw(&mut app, ui, ctx).curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
        let ledger = tracker.ledger.lock().unwrap();
        assert_eq!(ledger.motion_frames, samples.len() as u64 + 1);
        assert_eq!(
            ledger.max_curl_gap,
            Duration::from_millis(if samples.is_empty() { 1000 } else { 801 })
        );
        drop(ledger);
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(!draw(&mut app, ui, ctx).curl);
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Flat(_)
        ));
    }
}

#[test]
fn discarded_output_replay_and_late_cancelled_or_mismatched_hooks_cannot_set_origin() {
    let origin = Instant::now();
    let clock = TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    let fact = super::tests::fact(1);
    let tracker = Arc::new(SubmissionTracker::new(fact));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let receipt = super::tests::receipt(ctx, 1);
        queue_submission(
            ctx,
            receipt,
            Some(super::tests::event(&tracker, SubmissionKind::Curl)),
        );
        if ctx.current_pass_index() == 0 {
            ctx.request_discard("clock-test");
            clock.set(origin + Duration::from_secs(3));
        } else {
            assert_eq!(tracker.started_at(), None, "discard did not publish");
        }
    });
    let start_at = origin + Duration::from_secs(3);
    assert_eq!(tracker.started_at(), Some(start_at));
    clock.set(start_at + Duration::from_millis(50));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        queue_submission(
            ctx,
            super::tests::receipt(ctx, 1),
            Some(super::tests::event(&tracker, SubmissionKind::Curl)),
        );
    });
    assert_eq!(tracker.started_at(), Some(start_at));
    for cancel in [false, true] {
        let tracker = Arc::new(SubmissionTracker::new(fact));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let mut event = super::tests::event(&tracker, SubmissionKind::Curl);
            if cancel {
                tracker.end();
            } else {
                event.fact.operation_id = ViewerOperationId::Slideshow(999);
            }
            queue_submission(ctx, super::tests::receipt(ctx, 1), Some(event));
        });
        assert_eq!(tracker.started_at(), None);
        assert_eq!(
            tracker.ledger.lock().unwrap().motion,
            MotionClock::AwaitingAcceptedOutput
        );
    }
    let tracker = Arc::new(SubmissionTracker::new(fact));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let mut receipt = super::tests::receipt(ctx, 1);
        receipt.destination = super::tests::destination(2);
        queue_submission(
            ctx,
            receipt,
            Some(super::tests::event(&tracker, SubmissionKind::Curl)),
        );
    });
    assert_eq!(
        tracker.started_at(),
        None,
        "a sibling receipt is not this operation's output"
    );
}

#[test]
fn missing_source_at_accept_is_instant_and_late_source_does_not_restart() {
    let accepted_at = Instant::now();
    let clock = TestClock::at(accepted_at);
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    frame(&ctx, |ui, ctx| {
        draw(&mut app, ui, ctx);
    });
    let source = app.fs_cache.remove(&1).unwrap();
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    take_test_events();
    frame(&ctx, |_, ctx| {
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "clock-test");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        assert!(!matches!(
            app.page_turn_presentation.state,
            PresentationState::Offered { .. }
        ));
        app.fullscreen_idx = Some(1);
    });
    clock.set(accepted_at + Duration::from_secs(3));
    app.fs_cache.insert(1, source);
    frame(&ctx, |ui, ctx| {
        assert!(!draw(&mut app, ui, ctx).curl);
    });
    let events = take_test_events();
    assert!(
        events.iter().any(
            |(_, kind, reason)| *kind == "skip" && *reason == "no_displayable_source_at_accept"
        )
    );
    assert!(!events.iter().any(|(_, kind, _)| *kind == "start"));
}
