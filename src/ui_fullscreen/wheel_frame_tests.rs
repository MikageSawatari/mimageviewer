//! Ordered wheel delivery through the production fullscreen merge/navigation owners.
use super::page_turn_submission_tests::ready_app;
use super::*;

fn wheel(y: f32, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, y),
        modifiers,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    }
}

fn frame(ctx: &egui::Context, events: Vec<egui::Event>, mut run: impl FnMut(&mut egui::Ui)) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            events: std::iter::once(egui::Event::PointerMoved(egui::pos2(300.0, 200.0)))
                .chain(events)
                .collect(),
            ..Default::default()
        },
        |ui| run(ui),
    )
    .drop_without_applying_deltas();
}

fn collect_actual_merge(
    app: &mut App,
    ui: &mut egui::Ui,
    suppress_navigation: bool,
) -> (FsWheelFrameResult, Option<ViewerOperationId>) {
    let ctx = ui.ctx().clone();
    // Exact operation identities are a contract of the enabled presentation
    // ingress. Mixed deliveries have several facts, so the existing one-input
    // animation budget declines an effect while navigation admission stays live.
    app.page_turn_pre_draw(&ctx);
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(&ctx, idx, FsPageTurnDecision::normal());
    let mut operation = None;
    let result = app.handle_fs_wheel_and_click_frame(
        ui,
        &ctx,
        ui.max_rect(),
        &state,
        StillSeekGeometry::bar_only(),
        false,
        0,
        &mut None,
        None,
        false,
        &mut operation,
        suppress_navigation,
        true,
    );
    (result, operation)
}

fn actual_merge(
    app: &mut App,
    ui: &mut egui::Ui,
    suppress_navigation: bool,
) -> (FsWheelFrameResult, Option<ViewerOperationId>) {
    let (mut result, mut operation) = collect_actual_merge(app, ui, suppress_navigation);
    if let Some(mut proposal) = result.deferred.take() {
        app.claim_fs_wheel_frame_after_ui(&mut proposal);
        result = app.apply_fs_wheel_frame_proposal(proposal, suppress_navigation, &mut operation);
    }
    (result, operation)
}

fn finish(
    app: &mut App,
    ui: &egui::Ui,
    result: &FsWheelFrameResult,
    operation: Option<ViewerOperationId>,
) {
    if let Some(idx) = app.fullscreen_idx {
        app.handle_fs_navigation_with_operation(
            ui.ctx(),
            result.close,
            false,
            None,
            None,
            None,
            result.page_nav,
            None,
            idx,
            operation,
        );
    }
}

fn install(ctx: &egui::Context) {
    page_turn::install(ctx);
    install_fs_navigator_input_tracking(ctx);
}

#[test]
fn egui037_wheel_frame_plain_ctrl_plain_preserves_order_and_existing_target_admission() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (result, operation) = actual_merge(&mut app, ui, false);
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "first plain interval is accepted before zoom"
            );
            assert!(app.fs_zoom > 1.0, "Ctrl zoom belongs to page one");
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            assert!(result.wheel_navigation_produced);
            assert!(matches!(
                operation,
                Some(ViewerOperationId::Pointer { ordinal: 3, .. })
            ));
            finish(&mut app, ui, &result, operation);
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "the existing unpainted Display sequence rejects the second target"
            );
            assert!(
                app.fs_zoom > 1.0,
                "a rejected target must retain page one's zoom"
            );
        },
    );
}

#[test]
fn egui037_wheel_frame_trailing_zoom_does_not_restore_superseded_key_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
        ],
        |ui| {
            let (result, operation) = actual_merge(&mut app, ui, false);
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(app.fs_zoom > 1.0);
            assert!(result.page_nav.is_none());
            assert!(
                result.wheel_navigation_produced,
                "frame merge must supersede the key even with no final nav"
            );
            finish(&mut app, ui, &result, operation);
            assert_eq!(app.fullscreen_idx, Some(1));
        },
    );
}

#[test]
fn egui037_wheel_frame_close_and_context_priority_do_not_commit_intermediate_pages() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (result, _) = actual_merge(&mut app, ui, true);
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(
                app.fs_zoom > 1.0,
                "released zoom side effect remains on the old page"
            );
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            assert!(result.wheel_navigation_produced);
        },
    );
}

#[test]
fn egui037_wheel_frame_spread_targets_and_boundary_are_resolved_after_prior_acceptance() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.page_turn_animation_enabled = true;
        frame(
            &ctx,
            vec![
                wheel(-120.0, egui::Modifiers::NONE),
                wheel(120.0, egui::Modifiers::CTRL),
                wheel(-120.0, egui::Modifiers::NONE),
            ],
            |ui| {
                let (result, operation) = actual_merge(&mut app, ui, false);
                assert_eq!(app.fullscreen_idx, Some(2));
                assert!(app.fs_zoom > 1.0);
                assert_eq!(result.page_nav, FsPageNav::Boundary { at_end: true });
                finish(&mut app, ui, &result, operation);
                assert_eq!(app.fullscreen_idx, Some(2));
                assert!(app.fs_boundary_hint.is_some());
            },
        );
    }
}

#[test]
fn egui037_wheel_frame_homogeneous_notches_keep_one_legacy_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (result, operation) = actual_merge(&mut app, ui, false);
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            assert!(matches!(
                operation,
                Some(ViewerOperationId::Pointer { ordinal: 2, .. })
            ));
            finish(&mut app, ui, &result, operation);
            assert_eq!(app.fullscreen_idx, Some(1));
        },
    );
}

#[test]
fn egui037_wheel_frame_real_discard_does_not_dispatch_consumed_intervals_again() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let mut passes = 0;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (result, operation) = actual_merge(&mut app, ui, false);
            finish(&mut app, ui, &result, operation);
            assert_eq!(app.fullscreen_idx, Some(1));
            passes += 1;
            if passes == 1 {
                ui.ctx()
                    .request_discard("replay the consumed mixed wheel frame");
            } else {
                assert!(!result.wheel_navigation_produced);
            }
        },
    );
    assert!(passes >= 2);
}

#[test]
fn egui037_wheel_frame_split_changes_slice_before_resolving_the_next_interval() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, mode);
        app.settings.page_turn_animation_enabled = true;
        if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
            *source_dims = Some([240, 120]);
        }
        app.record_page_dims_for_spread(0, (240, 120));
        let order = crate::page_split::SplitDirection::from_spread_mode(mode).unwrap();
        app.fullscreen_page_slice = order.first();
        frame(
            &ctx,
            vec![
                wheel(-120.0, egui::Modifiers::NONE),
                wheel(120.0, egui::Modifiers::CTRL),
                wheel(-120.0, egui::Modifiers::NONE),
            ],
            |ui| {
                let (result, operation) = actual_merge(&mut app, ui, false);
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.fullscreen_page_slice, order.second());
                assert!(app.fs_zoom > 1.0);
                assert_eq!(
                    result.page_nav,
                    FsPageNav::Split(crate::page_split::PresentationStep::whole(1))
                );
                finish(&mut app, ui, &result, operation);
                assert_eq!(app.fullscreen_idx, Some(1));
                assert_eq!(
                    app.fullscreen_page_slice,
                    crate::page_split::PageSlice::Full
                );
            },
        );
    }
}

#[test]
fn egui037_wheel_frame_click_classification_uses_released_zoom_before_click_order() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    // Register the production canvas before the real press/release input.
    frame(&ctx, Vec::new(), |ui| {
        let _ = actual_merge(&mut app, ui, false);
    });
    let click = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(450.0, 200.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &ctx,
        vec![
            egui::Event::PointerMoved(egui::pos2(450.0, 200.0)),
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
            click(true),
            click(false),
        ],
        |ui| {
            let (result, operation) = actual_merge(&mut app, ui, false);
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "zoomed click must not supersede both wheel intervals"
            );
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            assert!(
                matches!(
                    operation,
                    Some(ViewerOperationId::Pointer { ordinal: 4, .. })
                ),
                "the final wheel, rather than the non-navigating click, owns the command"
            );
            finish(&mut app, ui, &result, operation);
            assert_eq!(app.fullscreen_idx, Some(1));
        },
    );
}

#[test]
fn egui037_wheel_frame_final_bar_close_priority_is_known_before_any_navigation_dispatch() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
            assert_eq!(
                app.fullscreen_idx,
                Some(0),
                "collecting a proposal cannot navigate before the bar is drawn"
            );
            assert_eq!(app.fs_zoom, 1.0);
            let mut proposal = result
                .deferred
                .take()
                .expect("mixed navigation belongs to the final frame merge");
            app.claim_fs_wheel_frame_after_ui(&mut proposal);
            let result = app.apply_fs_wheel_frame_proposal(proposal, true, &mut operation);
            assert_eq!(
                app.fullscreen_idx,
                Some(0),
                "the bar's Close supersedes every wheel target"
            );
            assert!(
                app.fs_zoom > 1.0,
                "released non-navigation zoom effects remain intact"
            );
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
        },
    );
}

#[test]
fn egui037_wheel_frame_front_owner_consumption_before_final_merge_cannot_revive_motion() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
            crate::wheel_input::consume_wheel_input(ui.ctx());
            let mut proposal = result.deferred.take().unwrap();
            app.claim_fs_wheel_frame_after_ui(&mut proposal);
            let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
            assert!(!result.wheel_navigation_produced);
            assert!(result.page_nav.is_none());
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(app.fs_zoom, 1.0);
        },
    );
}

#[test]
fn egui037_wheel_frame_child_discard_preserves_owned_intervals_until_final_frame_merge() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let mut owned = None;
    let mut operation = None;
    let mut passes = 0;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (mut result, current_operation) = collect_actual_merge(&mut app, ui, false);
            if let Some(proposal) = result.deferred.take() {
                owned = Some(proposal);
            }
            if current_operation.is_some() {
                operation = current_operation;
            }
            if let Some(proposal) = owned.as_mut() {
                app.claim_fs_wheel_frame_after_ui(proposal);
            }
            assert_eq!(app.fullscreen_idx, Some(0));
            passes += 1;
            if passes == 1 {
                ui.ctx()
                    .request_discard("exercise child replay before final frame navigation");
            }
        },
    );
    assert!(passes >= 2);
    let result = app.apply_fs_wheel_frame_proposal(owned.unwrap(), false, &mut operation);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_zoom > 1.0);
    assert_eq!(result.page_nav, FsPageNav::Delta(1));
    assert!(matches!(
        operation,
        Some(ViewerOperationId::Pointer { ordinal: 3, .. })
    ));
}

#[cfg(windows)]
#[test]
fn egui037_wheel_frame_intermediate_audio_ctrl_uses_timeline_owner_once() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let audio = app.tmp.path().join("ordered-wheel.flac");
    app.items[1] = GridItem::Audio(audio.clone());
    app.fs_cache.insert(
        1,
        FsCacheEntry::Video {
            player: Box::new(crate::video::VideoPlayer::stream_ready_disconnected_for_test(audio)),
            load_seq: 1,
        },
    );
    let before = app.music_timeline_row_secs;
    frame(
        &ctx,
        vec![
            wheel(-120.0, egui::Modifiers::NONE),
            wheel(120.0, egui::Modifiers::CTRL),
            wheel(-120.0, egui::Modifiers::NONE),
        ],
        |ui| {
            let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
            let mut proposal = result.deferred.take().unwrap();
            proposal.music_ctrl_wheel_gate_reached = true;
            app.claim_fs_wheel_frame_after_ui(&mut proposal);
            let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
            assert_eq!(
                app.fullscreen_idx,
                Some(1),
                "the first interval adopted the cached audio page"
            );
            assert_ne!(
                app.music_timeline_row_secs, before,
                "Ctrl belongs to the intermediate timeline"
            );
            assert_eq!(
                app.fs_zoom, 1.0,
                "timeline wheel cannot change hidden image zoom"
            );
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            let after = app.music_timeline_row_secs;
            app.handle_music_timeline_ctrl_wheel_after_hud(ui.ctx(), 1, true);
            assert_eq!(
                app.music_timeline_row_secs, after,
                "the ordinary after-HUD owner cannot replay claimed input"
            );
        },
    );
}

#[test]
fn egui037_wheel_frame_zoom_only_applies_before_paint_without_a_deferred_navigation() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Single);
    frame(&ctx, vec![wheel(120.0, egui::Modifiers::CTRL)], |ui| {
        let (result, _) = collect_actual_merge(&mut app, ui, false);
        assert!(result.deferred.is_none());
        assert!(!result.wheel_navigation_produced);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.fs_zoom > 1.0);
    });
}

fn recognized_command(
    action: crate::ring_shortcut::RingActionId,
    gesture: bool,
) -> FsSecondaryAction {
    use crate::ring_shortcut::*;
    let command = if gesture {
        RightDragCommand::MouseGesture {
            context: RightDragContext::ImageFullscreen,
            action,
            pattern_label: "R".into(),
            show_result_toast: false,
        }
    } else {
        RightDragCommand::RingShortcut {
            context: RingShortcutContext::ImageFullscreen,
            action,
            direction: RingDirection::Right,
            source: "ordered-wheel-regression",
        }
    };
    FsSecondaryAction::Recognized {
        context: RightDragContext::ImageFullscreen,
        pos: egui::pos2(300.0, 200.0),
        recognition: RightDragRecognition::command(command),
    }
}

#[test]
fn egui037_wheel_frame_recognized_home_end_keep_command_admission_before_final_wheel() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    use crate::ring_shortcut::RingActionId;
    for gesture in [false, true] {
        for (action, target, final_target) in [
            (RingActionId::ImageHome, 0, 1),
            (RingActionId::ImageEnd, 3, 3),
        ] {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Single);
            app.settings.page_turn_animation_enabled = true;
            app.fullscreen_idx = Some(1);
            frame(
                &ctx,
                vec![
                    wheel(-120.0, egui::Modifiers::NONE),
                    wheel(120.0, egui::Modifiers::CTRL),
                    wheel(-120.0, egui::Modifiers::NONE),
                ],
                |ui| {
                    let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
                    let mut proposal = result.deferred.take().unwrap();
                    proposal.secondary_action = recognized_command(action.clone(), gesture);
                    app.claim_fs_wheel_frame_after_ui(&mut proposal);
                    let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(target),
                        "the real command owner must accept Home/End before a wheel Display sequence exists"
                    );
                    // Home/End use the existing nonsequential jump owner. That
                    // owner lands without opening a Display sequence, so the
                    // released sole final wheel remains eligible after it.
                    assert!(!app.fs_navigation_sequence_blocks_new_target());
                    assert_eq!(result.page_nav, FsPageNav::Delta(1));
                    assert!(result.wheel_navigation_produced);
                    assert!(matches!(
                        operation,
                        Some(ViewerOperationId::Pointer { ordinal: 3, .. })
                    ));
                    finish(&mut app, ui, &result, operation);
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(final_target),
                        "the sole final wheel follows the accepted command using existing navigation/boundary admission"
                    );
                },
            );
        }
    }
}

#[test]
fn egui037_wheel_frame_recognized_tree_sibling_own_folder_admission_before_final_wheel() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    use crate::ring_shortcut::RingActionId;
    for gesture in [false, true] {
        for action in [
            RingActionId::TreeFolderNext,
            RingActionId::SiblingFolderNext,
        ] {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Single);
            app.settings.page_turn_animation_enabled = true;
            app.fullscreen_idx = Some(1);
            let current = app.tmp.path().join("a-current");
            std::fs::create_dir(&current).unwrap();
            std::fs::create_dir(app.tmp.path().join("b-next")).unwrap();
            app.current_folder = Some(current);
            frame(
                &ctx,
                vec![
                    wheel(-120.0, egui::Modifiers::NONE),
                    wheel(120.0, egui::Modifiers::CTRL),
                    wheel(-120.0, egui::Modifiers::NONE),
                ],
                |ui| {
                    let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
                    let mut proposal = result.deferred.take().unwrap();
                    proposal.secondary_action = recognized_command(action.clone(), gesture);
                    app.claim_fs_wheel_frame_after_ui(&mut proposal);
                    let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                    assert_eq!(app.fullscreen_idx, Some(1));
                    assert!(
                        matches!(
                            app.fs_holdover_tex
                                .as_ref()
                                .and_then(FsHoldover::navigation_sequence)
                                .map(|sequence| &sequence.target),
                            Some(crate::app::FsNavigationSequenceTarget::FolderItems { .. })
                        ),
                        "the real tree/sibling command must own folder admission, not a wheel Display sequence"
                    );
                    finish(&mut app, ui, &result, operation);
                    assert_eq!(app.fullscreen_idx, Some(1));
                },
            );
        }
    }
}

#[test]
fn egui037_wheel_frame_recognized_close_terminates_final_wheel_without_reopening() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    use crate::ring_shortcut::*;
    for variant in 0..3 {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Single);
        app.settings.page_turn_animation_enabled = true;
        app.fullscreen_idx = Some(1);
        frame(
            &ctx,
            vec![
                wheel(-120.0, egui::Modifiers::NONE),
                wheel(120.0, egui::Modifiers::CTRL),
                wheel(-120.0, egui::Modifiers::NONE),
            ],
            |ui| {
                let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
                let mut proposal = result.deferred.take().unwrap();
                proposal.secondary_action = if variant == 2 {
                    FsSecondaryAction::Recognized {
                        context: RightDragContext::ImageFullscreen,
                        pos: egui::pos2(300.0, 200.0),
                        recognition: RightDragRecognition::command(
                            RightDragCommand::ViewerShortRightClick {
                                context: RightDragContext::ImageFullscreen,
                                pos: egui::pos2(300.0, 200.0),
                            },
                        ),
                    }
                } else {
                    recognized_command(RingActionId::CloseFullscreen, variant == 1)
                };
                app.claim_fs_wheel_frame_after_ui(&mut proposal);
                let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                assert!(app.fullscreen_idx.is_none());
                assert!(result.page_nav.is_none());
                assert!(
                    result.wheel_navigation_produced,
                    "the terminal command must keep superseded key navigation from returning"
                );
                assert!(operation.is_none());
                app.handle_fs_navigation_with_operation(
                    ui.ctx(),
                    false,
                    false,
                    None,
                    None,
                    None,
                    result.page_nav,
                    None,
                    1,
                    operation,
                );
                assert!(
                    app.fullscreen_idx.is_none(),
                    "the final frame owner cannot reopen the closed viewer"
                );
            },
        );
    }
}

#[test]
fn egui037_wheel_frame_recognition_without_command_does_not_block_ordered_delivery() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    use crate::ring_shortcut::*;
    for outcome in [MouseFlickOutcome::None, MouseFlickOutcome::Cancelled] {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = ready_app(&ctx, SpreadMode::Single);
        app.settings.page_turn_animation_enabled = true;
        frame(
            &ctx,
            vec![
                wheel(-120.0, egui::Modifiers::NONE),
                wheel(120.0, egui::Modifiers::CTRL),
                wheel(-120.0, egui::Modifiers::NONE),
            ],
            |ui| {
                let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
                let mut proposal = result.deferred.take().unwrap();
                proposal.secondary_action = FsSecondaryAction::Recognized {
                    context: RightDragContext::ImageFullscreen,
                    pos: egui::pos2(300.0, 200.0),
                    recognition: RightDragRecognition::outcome(outcome),
                };
                app.claim_fs_wheel_frame_after_ui(&mut proposal);
                let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                assert_eq!(app.fullscreen_idx, Some(1));
                assert!(app.fs_zoom > 1.0);
                assert_eq!(result.page_nav, FsPageNav::Delta(1));
            },
        );
    }
}

#[test]
fn egui037_wheel_frame_recognized_live_return_keeps_original_page_and_return_owner() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    use crate::app::{BookmarkViewReturnGridState, BookmarkViewState, FullscreenCloseContinuation};
    use crate::bookmark_browser::BookmarkViewReturnTarget;
    use crate::ring_shortcut::*;
    for kind in ["book-opening", "effective-folder", "auto-open-zip"] {
        for variant in 0..3 {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Single);
            app.settings.page_turn_animation_enabled = true;
            app.settings.auto_fullscreen_zip_pdf = kind == "auto-open-zip";
            app.settings.auto_fullscreen_image_folders = false;
            app.fullscreen_idx = Some(1);
            app.selected = Some(1);
            app.scroll_offset_y = 73.0;
            app.slideshow_playing = true;
            app.slideshow_anchor_idx = app.fullscreen_idx;
            app.slideshow_next_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
            let logical = app.tmp.path().join("books").join(match kind {
                "book-opening" => "volume",
                "effective-folder" => "volume.7z",
                _ => "volume.zip",
            });
            app.current_folder = Some(if kind == "effective-folder" {
                app.tmp.path().join("cache").join("internal.zip")
            } else {
                logical.clone()
            });
            app.archive_source_override = (kind == "effective-folder").then(|| logical.clone());
            if kind != "auto-open-zip" {
                app.bookmark_view_state = Some(BookmarkViewState::Opening {
                    target: BookmarkViewReturnTarget::Book(logical.clone()),
                    grid: BookmarkViewReturnGridState {
                        row_keys: vec![(0, 1), (0, 2)],
                        selected_key: Some((0, 2)),
                        opened_key: (0, 2),
                        scroll_offset_y: 73.0,
                    },
                });
                assert_eq!(app.effective_folder(), Some(logical));
                assert!(app.bookmark_view_back_nav().is_some());
            } else {
                assert!(app.auto_open_for_current_container());
            }
            let bookmark_owner = app.bookmark_view_state.clone();
            let folder_owner = app.current_folder.clone();
            let history_before = app.last_reading_history_touch.clone();
            let destination = app.page_turn_destination();
            let items = app.items_generation;
            frame(
                &ctx,
                vec![
                    wheel(-120.0, egui::Modifiers::NONE),
                    wheel(120.0, egui::Modifiers::CTRL),
                    wheel(-120.0, egui::Modifiers::NONE),
                ],
                |ui| {
                    let (mut result, mut operation) = collect_actual_merge(&mut app, ui, false);
                    let mut proposal = result.deferred.take().unwrap();
                    proposal.secondary_action = if variant == 2 {
                        FsSecondaryAction::Recognized {
                            context: RightDragContext::ImageFullscreen,
                            pos: egui::pos2(300.0, 200.0),
                            recognition: RightDragRecognition::command(
                                RightDragCommand::ViewerShortRightClick {
                                    context: RightDragContext::ImageFullscreen,
                                    pos: egui::pos2(300.0, 200.0),
                                },
                            ),
                        }
                    } else {
                        recognized_command(RingActionId::CloseFullscreen, variant == 1)
                    };
                    app.claim_fs_wheel_frame_after_ui(&mut proposal);
                    assert!(app.slideshow_playing);
                    assert_eq!(app.slideshow_anchor_idx, Some(1));
                    assert!(app.slideshow_next_at <= std::time::Instant::now());
                    let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut operation);
                    assert!(
                        matches!(
                            app.pending_return_to_parent,
                            FullscreenCloseContinuation::Live(_)
                        ),
                        "{kind}: command {variant} must leave its accepted return pending"
                    );
                    assert!(app.pending_return_to_parent.is_pending());
                    assert!(!app.slideshow_playing);
                    assert!(app.slideshow_anchor_idx.is_none());
                    assert_eq!(app.fullscreen_idx, Some(1));
                    assert_eq!(app.page_turn_destination(), destination);
                    assert_eq!(app.items_generation, items);
                    assert!(result.page_nav.is_none());
                    assert!(operation.is_none());
                    assert!(result.wheel_navigation_produced);
                    finish(&mut app, ui, &result, operation);
                    assert!(matches!(
                        app.pending_return_to_parent,
                        FullscreenCloseContinuation::Live(_)
                    ));
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(1),
                        "old wheel must not advance before return consumption"
                    );
                    assert!(!app.slideshow_playing);
                    assert!(app.slideshow_anchor_idx.is_none());
                    assert_eq!(app.page_turn_destination(), destination);
                    assert_eq!(app.items_generation, items);
                    assert_eq!(app.selected, Some(1));
                    assert_eq!(app.scroll_offset_y, 73.0);
                    assert_eq!(app.bookmark_view_state, bookmark_owner);
                    assert_eq!(app.current_folder, folder_owner);
                    assert_eq!(app.last_reading_history_touch, history_before);
                },
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn egui037_wheel_frame_true_close_stops_only_projected_timer_and_manual_stop_closes_popup() {
    let _serial = crate::key_input::lock_test_input();
    use crate::app::{ContinuousReadingScrollTransition, HistoryTrigger};

    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    let closing = app.build_window_context_for_test(46019, |viewer| {
        viewer.fullscreen_idx = Some(0);
        viewer.slideshow_playing = true;
        viewer.slideshow_anchor_idx = Some(0);
        viewer.continuous_reading_scroll_transition =
            Some(ContinuousReadingScrollTransition::AwaitingReanchor {
                history_trigger: HistoryTrigger::UserChosen,
            });
        viewer.slideshow_scroll_range_cache = Some((0, 10.0, 20.0));
    });
    app.fullscreen_idx = Some(1);
    app.slideshow_playing = true;
    app.slideshow_anchor_idx = Some(1);
    app.continuous_reading_scroll_transition =
        Some(ContinuousReadingScrollTransition::AwaitingReanchor {
            history_trigger: HistoryTrigger::UserChosen,
        });
    app.slideshow_scroll_range_cache = Some((1, 30.0, 40.0));
    let main_deadline = app.slideshow_next_at;
    app.slideshow_popup_open = true;

    app.with_window_context_for_test(closing, |viewer| {
        assert!(viewer.slideshow_popup_open);
        viewer.finish_fullscreen_navigation_for_true_close();
        assert!(!viewer.slideshow_playing);
        assert!(viewer.slideshow_anchor_idx.is_none());
        assert!(viewer.continuous_reading_scroll_transition.is_none());
        assert!(viewer.slideshow_scroll_range_cache.is_none());
        assert!(viewer.slideshow_popup_open);
        assert_eq!(viewer.fullscreen_idx, Some(0));
    });
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.slideshow_playing);
    assert_eq!(app.slideshow_anchor_idx, Some(1));
    assert_eq!(app.slideshow_next_at, main_deadline);
    assert!(matches!(
        app.continuous_reading_scroll_transition,
        Some(ContinuousReadingScrollTransition::AwaitingReanchor { .. })
    ));
    assert_eq!(app.slideshow_scroll_range_cache, Some((1, 30.0, 40.0)));
    assert!(app.slideshow_popup_open);

    assert!(app.stop_slideshow_playback());
    assert!(!app.slideshow_playing);
    assert!(app.slideshow_anchor_idx.is_none());
    assert!(app.continuous_reading_scroll_transition.is_none());
    assert!(app.slideshow_scroll_range_cache.is_none());
    assert!(!app.slideshow_popup_open);
    app.slideshow_popup_open = true;
    assert!(!app.stop_slideshow_playback());
    assert!(!app.slideshow_popup_open);
}
