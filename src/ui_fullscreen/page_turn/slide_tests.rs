//! Slide shares the accepted-output clock, cancellation and canonical landing owner.
use super::clock_tests::TestClock;
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{
    adopt_interrupted_target, draw_current, frame, ready_app, seed_leaf,
};

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

#[test]
fn operation_script_event_animation_toggle_commits_only_original_input_once_with_control() {
    use crate::app::operation_script_runtime::ScriptInputSurface;
    use crate::operation_script::{CompiledOperationScript, OperationScriptState};
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let _clock = TestClock::at(Instant::now());
    for toggle in [false, true] {
        // Sequential fixtures: never acquire the global data-dir mutex twice.
        take_test_events();
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, SpreadMode::Single);
        app.settings.page_turn_animation_style = PageTurnAnimationStyle::Slide;
        seed_leaf(&ctx, &mut app);
        let mut late = None;
        frame(&ctx, |ui, ctx| {
            let operation = ViewerOperationId::Slideshow(next_operation_batch());
            app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "slideshow");
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(2),
                None,
                1,
                Some(operation),
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(!draw(&mut app, ui, ctx).curl);
            ctx.with_plugin(|plugin: &mut SubmissionPlugin| {
                late = plugin.pending.remove(&ctx.viewport_id());
            });
        });
        let original = app
            .page_turn_presentation
            .continuation_operation_for_test()
            .unwrap();
        assert_eq!(original.1, 2);
        let late = late.unwrap();
        assert!(!late.receipt.was_accepted());
        app.settings.operation_script_enabled = true;
        let commands = if toggle {
            r#"["FsPageTurnAnimationToggle"]"#
        } else {
            "[]"
        };
        app.operation_script.state = OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&format!(
            r#"fn slots() {{ [] }} fn on_event(s) {{ if "open" in s.events {{ #{{run:{commands},notice:"initial event"}} }} else if "page" in s.events {{ #{{run:[],notice:"original input page"}} }} else {{ [] }} }}"#
        )).unwrap()));
        frame(&ctx, |_, ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, "initial event");
        });
        assert_eq!(app.settings.page_turn_animation_enabled, !toggle);
        assert_eq!(
            app.page_turn_presentation.continuation_operation_for_test(),
            Some(original)
        );
        let immediate = app.collect_operation_script_event_identity(ScriptInputSurface::Viewer);
        // Late old receipt is insufficient after settings cancellation. In the
        // control it is also withheld; both must accept a real current flat.
        adopt_interrupted_target(&ctx, &mut app);
        assert_eq!(
            app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
            immediate
        );
        take_test_events();
        frame(&ctx, |ui, ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            assert!(matches!(
                app.begin_page_turn_viewer_frame(ctx, false, None),
                PageTurnFrameEntry::Continue {
                    committed: true,
                    ..
                }
            ));
            assert_eq!(app.fullscreen_idx, Some(2));
            draw(&mut app, ui, ctx);
        });
        assert!(
            app.page_turn_presentation
                .continuation_operation_for_test()
                .is_none()
        );
        frame(&ctx, |ui, ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            assert_eq!(
                app.fs_feedback_toast.as_ref().unwrap().0,
                "original input page"
            );
            assert!(!app.commit_page_turn_continuation(ctx));
            assert_eq!(app.fullscreen_idx, Some(2));
            draw(&mut app, ui, ctx);
        });
        app.fs_feedback_toast = None;
        frame(&ctx, |ui, ctx| {
            app.detect_operation_script_events(ctx, ScriptInputSurface::Viewer);
            assert!(!app.commit_page_turn_continuation(ctx));
            assert_eq!(app.fullscreen_idx, Some(2));
            assert!(
                app.page_turn_presentation
                    .continuation_operation_for_test()
                    .is_none()
            );
            draw(&mut app, ui, ctx);
        });
        assert!(app.fs_feedback_toast.is_none());
        assert_eq!(app.settings.page_turn_animation_enabled, !toggle);
        assert_eq!(
            take_test_events()
                .iter()
                .filter(|(fact, kind, reason)| fact.operation_id == original.0
                    && *kind == "skip"
                    && *reason == "stage1_continuation_instant")
                .count(),
            1
        );
    }
}

fn styled_events() -> Vec<(&'static str, &'static str, PageTurnAnimationStyle)> {
    TEST_STYLED_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

#[test]
fn page_turn_slide_offered_turning_clock_visibility_and_normal_flat_landing() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let origin = Instant::now();
        let clock = TestClock::at(origin);
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.adopt_page_turn_animation_settings(true, 200, PageTurnAnimationStyle::Slide);
        app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
        frame(&ctx, |ui, ctx| {
            draw(&mut app, ui, ctx);
        });
        let target = if mode.is_spread() { 2 } else { 1 };
        frame(&ctx, |_, ctx| {
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Slideshow(next_operation_batch()),
                OperationPhase::Press,
                "slideshow",
            );
            app.offer_page_turn(ctx, target, true);
            let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
                panic!("offered")
            };
            assert_eq!(offer.style, PageTurnAnimationStyle::Slide);
            app.fullscreen_idx = Some(target);
        });
        // Preparation time never consumes the motion duration.
        clock.set(origin + Duration::from_secs(2));
        let mut first = None;
        frame(&ctx, |ui, ctx| {
            first = Some(draw(&mut app, ui, ctx));
        });
        let PresentationState::Turning {
            offer,
            incoming,
            projection,
            clock: motion,
            ..
        } = &app.page_turn_presentation.state
        else {
            panic!("turning")
        };
        assert!(matches!(projection.recipe, MotionRecipe::Slide(_)));
        assert_eq!(*motion, MotionClock::AwaitingAcceptedOutput);
        let tracker = offer.submission.clone();
        let held_texture_ids = incoming
            .pages
            .iter()
            .map(|page| page.resource.paint_texture_id())
            .collect::<Vec<_>>();
        assert_eq!(tracker.started_at(), Some(origin + Duration::from_secs(2)));
        let first = first.unwrap();
        assert!(first.curl && first.was_accepted());
        let visibility = actual_page_visibility(
            &projection.project(0.0, false).unwrap().visibility,
            first.pages().len(),
        );
        for (i, (_, _, _, area)) in first.pages().iter().enumerate() {
            assert!(
                (*area
                    - visibility
                        .iter()
                        .find(|v| v.occurrence == i as u64)
                        .map_or(0.0, |v| v.visible_area))
                .abs()
                    < 0.01
            );
        }
        assert!(first.pages().iter().any(|(_, _, _, area)| *area > 0.0));
        clock.set(origin + Duration::from_millis(2100));
        frame(&ctx, |ui, ctx| {
            assert!(draw(&mut app, ui, ctx).curl);
        });
        clock.set(origin + Duration::from_millis(2200));
        let landing = frame(&ctx, |ui, ctx| {
            assert!(!draw(&mut app, ui, ctx).curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
        let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state
        else {
            panic!("canonical landing")
        };
        let landing_texture_ids = incoming
            .pages
            .iter()
            .map(|page| page.resource.paint_texture_id())
            .collect::<Vec<_>>();
        assert_eq!(landing_texture_ids, held_texture_ids);
        let meshes = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Mesh(mesh) = &shape.shape {
                        Some((shape.clip_rect, mesh.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        let landing_meshes = meshes(&landing);
        assert_eq!(
            landing_meshes
                .iter()
                .map(|(_, mesh)| mesh.texture_id)
                .collect::<Vec<_>>(),
            landing_texture_ids,
            "the canonical landing must paint every held incoming material"
        );
        // Deadline uses normal flat geometry rather than a p=1 projection.
        // After that flat is adopted, the ordinary selector may upgrade textures.
        let normal = frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            assert!(!draw(&mut app, ui, ctx).curl);
        });
        let geometry = |meshes: &[(egui::Rect, Arc<egui::Mesh>)]| {
            meshes
                .iter()
                .map(|(clip, mesh)| (*clip, mesh.vertices.clone(), mesh.indices.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(geometry(&landing_meshes), geometry(&meshes(&normal)));
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Flat(_)
        ));
    }
}

#[test]
fn page_turn_slide_uses_the_accepted_recipe_without_rereading_settings() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let origin = Instant::now();
    let clock = TestClock::at(origin);
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_style = PageTurnAnimationStyle::Slide;
    seed_leaf(&ctx, &mut app);
    // Deliberately bypass adoption to prove the projection and diagnostics use
    // the accepted offer; production setting changes use adoption and cancel.
    app.settings.page_turn_animation_style = PageTurnAnimationStyle::Curl;
    styled_events();
    clock.set(origin + Duration::from_millis(100));
    let output = frame(&ctx, |ui, ctx| {
        assert!(draw(&mut app, ui, ctx).curl);
    });
    let PresentationState::Turning {
        offer, projection, ..
    } = &app.page_turn_presentation.state
    else {
        panic!("turning")
    };
    assert_eq!(offer.style, PageTurnAnimationStyle::Slide);
    assert_eq!(offer.submission.style, PageTurnAnimationStyle::Slide);
    assert!(matches!(projection.recipe, MotionRecipe::Slide(_)));
    for expected in projection.project(100.0 / 600.0, false).unwrap().meshes {
        assert!(output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) =>
                mesh.as_ref() == &expected.mesh && shape.clip_rect == expected.clip_rect,
            _ => false,
        }));
    }
    assert!(
        styled_events()
            .iter()
            .any(|(kind, _, style)| *kind == "frame" && *style == PageTurnAnimationStyle::Slide)
    );
}

#[test]
fn page_turn_style_change_cancels_offered_and_turning_but_off_change_preserves_completion() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for turning in [false, true] {
        let _clock = TestClock::at(Instant::now());
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Single);
        app.adopt_page_turn_animation_settings(true, 600, PageTurnAnimationStyle::Slide);
        if turning {
            seed_leaf(&ctx, &mut app);
        } else {
            frame(&ctx, |ui, ctx| {
                draw(&mut app, ui, ctx);
            });
            frame(&ctx, |_, ctx| {
                app.page_turn_external_operation(
                    ctx,
                    ViewerOperationId::Slideshow(next_operation_batch()),
                    OperationPhase::Press,
                    "slideshow",
                );
                app.offer_page_turn(ctx, 1, true);
            });
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Offered { .. }
            ));
        }
        styled_events();
        app.adopt_page_turn_animation_settings(true, 600, PageTurnAnimationStyle::Curl);
        assert!(!matches!(
            app.page_turn_presentation.state,
            PresentationState::Offered { .. } | PresentationState::Turning { .. }
        ));
        assert!(styled_events().iter().any(|(kind, reason, style)| matches!(
            *kind,
            "skip" | "cancel"
        ) && *reason
            == "settings_changed"
            && *style == PageTurnAnimationStyle::Slide));
        if turning {
            let PresentationState::Completing { diagnostics, .. } =
                &app.page_turn_presentation.state
            else {
                panic!("canonical landing required")
            };
            assert_eq!(diagnostics.style, PageTurnAnimationStyle::Slide);
            frame(&ctx, |ui, ctx| {
                assert!(!draw(&mut app, ui, ctx).curl);
            });
            assert!(
                styled_events()
                    .iter()
                    .any(|(kind, reason, style)| *kind == "fully_visible"
                        && *reason == "canonical_after_cancel"
                        && *style == PageTurnAnimationStyle::Slide)
            );
        }
        app.adopt_page_turn_animation_settings(false, 600, PageTurnAnimationStyle::Curl);
        let before = match &app.page_turn_presentation.state {
            PresentationState::Completing { incoming, .. } => Some(incoming.clone()),
            _ => None,
        };
        styled_events();
        app.adopt_page_turn_animation_settings(false, 400, PageTurnAnimationStyle::Slide);
        assert!(styled_events().is_empty());
        if let Some(before) = before {
            let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state
            else {
                panic!("off change retains completion")
            };
            assert!(
                Arc::ptr_eq(&before, incoming),
                "off style change must not refresh flat evidence"
            );
        }
    }
}

#[test]
fn page_turn_slide_style_change_keeps_continuation_and_rejects_late_old_flat_until_new_adoption() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let _clock = TestClock::at(Instant::now());
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_style = PageTurnAnimationStyle::Slide;
    seed_leaf(&ctx, &mut app);
    let mut late = None;
    frame(&ctx, |ui, ctx| {
        let operation = ViewerOperationId::Slideshow(next_operation_batch());
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "slideshow");
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(2),
            None,
            1,
            Some(operation),
        );
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(!draw(&mut app, ui, ctx).curl);
        ctx.with_plugin(|plugin: &mut SubmissionPlugin| {
            late = plugin.pending.remove(&ctx.viewport_id());
        });
    });
    let command = app
        .page_turn_presentation
        .continuation_operation_for_test()
        .unwrap();
    let late = late.unwrap();
    assert!(!late.receipt.was_accepted());
    app.adopt_page_turn_animation_settings(true, 600, PageTurnAnimationStyle::Curl);
    assert_eq!(
        app.page_turn_presentation.continuation_operation_for_test(),
        Some(command)
    );
    styled_events();
    frame(&ctx, |_, ctx| {
        queue_submission(ctx, late.receipt.clone(), late.event.clone());
        assert!(!app.commit_page_turn_continuation(ctx));
    });
    assert!(late.receipt.was_accepted());
    assert!(
        styled_events()
            .iter()
            .any(|(kind, reason, style)| *kind == "fully_visible"
                && *reason == "canonical_after_cancel"
                && *style == PageTurnAnimationStyle::Slide)
    );
    frame(&ctx, |_, ctx| {
        assert!(
            !app.commit_page_turn_continuation(ctx),
            "the old accepted flat cannot satisfy refreshed evidence"
        );
    });
    adopt_interrupted_target(&ctx, &mut app);
    take_test_events();
    frame(&ctx, |ui, ctx| {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(ctx, false, None),
            PageTurnFrameEntry::Continue {
                committed: true,
                ..
            }
        ));
        assert_eq!(app.fullscreen_idx, Some(2));
        draw(&mut app, ui, ctx);
    });
    frame(&ctx, |ui, ctx| {
        assert!(!app.commit_page_turn_continuation(ctx));
        assert_eq!(app.fullscreen_idx, Some(2));
        draw(&mut app, ui, ctx);
    });
    assert_eq!(
        take_test_events()
            .iter()
            .filter(|(_, kind, reason)| *kind == "skip" && *reason == "stage1_continuation_instant")
            .count(),
        1
    );
}
