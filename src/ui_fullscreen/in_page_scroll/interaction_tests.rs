//! Menu lifecycle and canvas clicks share the production key/pointer owners.
use super::handler_tests::{
    app, deferred_wheel_proposal, frame, full_rect, seed, tap, wheel_event,
};
use super::*;
use crate::settings::{FullscreenPageStartPosition, FullscreenWheelAction};

fn click_position() -> egui::Pos2 {
    egui::pos2(450.0, 200.0)
}

fn button(pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: click_position(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn pointer_handler(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> FsPageNav {
    let idx = app.fullscreen_idx.unwrap();
    let state = app.prepare_fullscreen_state(ctx, idx, FsPageTurnDecision::normal());
    let (navigation, close) =
        app.handle_fs_wheel_and_click(ui, ctx, full_rect(), &state, false, 0, &mut None);
    assert!(!close);
    navigation
}

fn apply_navigation(app: &mut App, ctx: &egui::Context, navigation: FsPageNav) {
    let idx = app.fullscreen_idx.unwrap();
    app.handle_fs_navigation(ctx, false, false, None, None, None, navigation, None, idx);
}

fn press(app: &mut App, ctx: &egui::Context) {
    // egui resolves a press against the previous pass's hit targets. `seed`
    // paints image geometry only, unlike the full 745 viewport fixture, so
    // register the production canvas before delivering its first press.
    frame(
        ctx,
        vec![egui::Event::PointerMoved(click_position())],
        |ui, ctx| {
            assert!(pointer_handler(app, ui, ctx).is_none());
        },
    );
    frame(
        ctx,
        vec![egui::Event::PointerMoved(click_position()), button(true)],
        |ui, ctx| {
            assert!(pointer_handler(app, ui, ctx).is_none());
            assert!(
                ctx.read_response(egui::Id::unique("fs_click"))
                    .expect("production canvas registered in the preceding pass")
                    .is_pointer_button_down_on()
            );
        },
    );
}

#[test]
fn reading_order_keys_close_the_panel_then_continue_their_existing_action() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for key in [
        egui::Key::K,
        egui::Key::J,
        egui::Key::ArrowRight,
        egui::Key::ArrowLeft,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.fs_zoom = 3.0;
        seed(&ctx, &mut app);
        app.fs_overflow_panel_state = FsOverflowPanelState::PageReadingOrder;
        frame(&ctx, tap(key), |_, ctx| {
            let result = app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
            assert!(!result.close);
            match key {
                egui::Key::K => {
                    assert!(result.page_nav.is_none());
                    assert!(app.fs_pan.y < 0.0);
                }
                egui::Key::J => {
                    assert!(result.page_nav.is_none());
                    assert!(app.fs_pan.y > 0.0);
                }
                _ => {
                    assert!(!result.page_nav.is_none());
                    assert_eq!(app.fs_pan, Vec2::ZERO);
                }
            }
        });
    }
}

#[test]
fn reading_order_escape_matches_the_existing_root_panel_close_and_forward_policy() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let mut root_close = None;
    for panel in [
        FsOverflowPanelState::Root,
        FsOverflowPanelState::PageReadingOrder,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        app.fs_overflow_panel_state = panel;
        frame(&ctx, tap(egui::Key::Escape), |_, ctx| {
            let result = app.handle_fs_key_input(ctx, 0, false);
            assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
            assert!(result.page_nav.is_none());
            assert_eq!(app.fs_pan, Vec2::ZERO);
            if let Some(expected) = root_close {
                assert_eq!(
                    result.close, expected,
                    "the new submenu keeps the existing Esc policy"
                );
            } else {
                root_close = Some(result.close);
            }
        });
    }
    assert_eq!(
        root_close,
        Some(true),
        "the existing menu forwards Esc to ordinary fullscreen close"
    );
}

#[test]
fn reading_order_non_key_wheel_and_absolute_page_navigation_keep_the_panel_open() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for via_wheel in [true, false] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
        seed(&ctx, &mut app);
        app.fs_overflow_panel_state = FsOverflowPanelState::PageReadingOrder;
        let events = if via_wheel {
            vec![
                egui::Event::PointerMoved(egui::pos2(300.0, 300.0)),
                wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
            ]
        } else {
            Vec::new()
        };
        frame(&ctx, events, |ui, ctx| {
            let navigation = if via_wheel {
                let navigation = pointer_handler(&mut app, ui, ctx);
                assert_eq!(navigation, FsPageNav::Delta(1));
                navigation
            } else {
                // Seek/toolbar targets converge here after their own pointer owner.
                FsPageNav::Target(1)
            };
            apply_navigation(&mut app, ctx, navigation);
            assert_eq!(app.fullscreen_idx, Some(1));
            assert_eq!(
                app.fs_overflow_panel_state,
                FsOverflowPanelState::PageReadingOrder
            );
        });
    }
}

#[test]
fn down_and_single_click_same_frame_uses_the_key_scrolled_view() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    press(&mut app, &ctx);
    let mut events = tap(egui::Key::ArrowDown);
    events.push(button(false));
    frame(&ctx, events, |ui, ctx| {
        let result = app.handle_fs_key_input(ctx, 0, false);
        assert!(result.page_nav.is_none());
        assert!(app.fs_pan.y < 0.0);
        let pan = app.fs_pan;
        assert!(
            pointer_handler(&mut app, ui, ctx).is_none(),
            "single click cannot turn after the earlier key handler moved this page"
        );
        assert_eq!(app.fs_pan, pan);
        assert_eq!(app.fullscreen_idx, Some(0));
    });
}

#[test]
fn page_scroll_then_same_frame_wheel_and_double_click_resets_without_turning() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    seed(&ctx, &mut app);
    frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
        app.handle_fs_key_input(ctx, 0, false);
        assert!(app.fs_pan.y < 0.0);
    });
    press(&mut app, &ctx);
    frame(&ctx, vec![button(false)], |ui, ctx| {
        assert!(pointer_handler(&mut app, ui, ctx).is_none());
        assert!(app.fs_pan.y < 0.0, "first click stays on the moved page");
    });
    press(&mut app, &ctx);
    frame(
        &ctx,
        vec![
            button(false),
            wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
        ],
        |ui, ctx| {
            assert!(pointer_handler(&mut app, ui, ctx).is_none());
            assert!(
                ctx.read_response(egui::Id::unique("fs_click"))
                    .expect("production canvas")
                    .double_clicked(),
                "both real clicks reached the canvas"
            );
            assert_eq!(
                app.fs_pan,
                Vec2::ZERO,
                "the double click resets after the predicted wheel scroll"
            );
            assert_eq!(app.fs_zoom, 1.0);
            assert_eq!(app.fullscreen_idx, Some(0));
        },
    );
}

#[test]
fn returning_to_the_automatic_origin_with_scroll_keys_reenables_single_click() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::ReadingStart;
    app.request_fs_initial_placement(0);
    seed(&ctx, &mut app);
    let origin = app.fs_pan;
    let automatic = app.fs_automatic_initial_view;
    assert!(origin.y > 0.0);
    assert!(automatic.is_some());
    frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
        app.handle_fs_key_input(ctx, 0, false);
        assert!(app.fs_pan.y < origin.y);
        assert_eq!(app.fs_automatic_initial_view, automatic);
    });
    frame(&ctx, tap(egui::Key::ArrowUp), |_, ctx| {
        app.handle_fs_key_input(ctx, 0, false);
        assert_eq!(app.fs_pan, origin);
        assert_eq!(app.fs_automatic_initial_view, automatic);
    });
    press(&mut app, &ctx);
    frame(&ctx, vec![button(false)], |ui, ctx| {
        let navigation = pointer_handler(&mut app, ui, ctx);
        assert!(
            ctx.read_response(egui::Id::unique("fs_click"))
                .expect("production canvas")
                .clicked(),
            "the restored-origin click reached the real canvas"
        );
        assert!(
            !navigation.is_none(),
            "the restored automatic view accepts a single click"
        );
        apply_navigation(&mut app, ctx, navigation);
        assert_eq!(app.fullscreen_idx, Some(1));
    });
}

#[cfg(windows)]
#[test]
fn mixed_image_to_audio_or_video_keeps_legacy_consumer_and_discards_accepted_scroll() {
    let _serial = crate::key_input::lock_test_input();
    for audio in [true, false] {
        let mut legacy_transform_and_timeline = None;
        for policy in [
            FullscreenWheelAction::PageTurn,
            FullscreenWheelAction::InPageScroll,
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            app.settings.fullscreen_wheel_action = policy;
            app.settings.ring_shortcuts.video_normal_wheel_action =
                crate::ring_shortcut::VideoNormalWheelActionId::AdjustVolume;
            let path = app.tmp.path().join(if audio {
                "mixed-scroll.flac"
            } else {
                "mixed-scroll.mp4"
            });
            app.items[1] = if audio {
                GridItem::Audio(path.clone())
            } else {
                GridItem::Video(path.clone())
            };
            let player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
            player.set_volume(1.0);
            app.fs_cache.insert(
                1,
                FsCacheEntry::Video {
                    player: Box::new(player),
                    load_seq: 1,
                },
            );
            seed(&ctx, &mut app);
            let initial_timeline = app.music_timeline_row_secs;
            frame(
                &ctx,
                vec![
                    egui::Event::PointerMoved(full_rect().center()),
                    wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::ALT),
                    wheel_event(Vec2::new(0.0, 120.0), egui::Modifiers::CTRL),
                    wheel_event(Vec2::new(0.0, -120.0), egui::Modifiers::NONE),
                ],
                |ui, ctx| {
                    let mut proposal = deferred_wheel_proposal(&mut app, ui, ctx);
                    let FsWheelFrameInput::Unconsumed(eligible) = &proposal.wheel_input else {
                        panic!("mixed proposal")
                    };
                    assert_eq!(eligible.len(), 3);
                    let page_scroll = policy == FullscreenWheelAction::InPageScroll;
                    assert_eq!(
                        matches!(eligible[2].class, FsWheelInPageClass::InPageScroll(_)),
                        page_scroll
                    );
                    if page_scroll {
                        assert!(
                            eligible[2].scroll_origin.is_some(),
                            "the image initially admitted this scroll"
                        );
                    }
                    // Same intermediate-audio owner as wheel_frame_tests: Ctrl must
                    // reach the adopted timeline exactly once after Alt navigation.
                    proposal.music_ctrl_wheel_gate_reached = true;
                    app.claim_fs_wheel_frame_after_ui(&mut proposal);
                    let result = app.apply_fs_wheel_frame_proposal(proposal, false, &mut None);
                    assert_eq!(app.fullscreen_idx, Some(1));
                    assert!(result.page_nav.is_none());
                    assert!(!result.close);
                    let state = (app.fs_zoom, app.fs_pan, app.music_timeline_row_secs);
                    if audio {
                        assert_ne!(app.music_timeline_row_secs, initial_timeline);
                        let after = app.music_timeline_row_secs;
                        app.handle_music_timeline_ctrl_wheel_after_hud(ctx, 1, true);
                        assert_eq!(
                            app.music_timeline_row_secs, after,
                            "claimed Ctrl input cannot replay after HUD"
                        );
                    } else {
                        assert!(
                            app.fs_zoom > 1.0,
                            "the existing Ctrl consumer continues on the adopted video"
                        );
                    }
                    let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&1) else {
                        panic!("resident media")
                    };
                    if page_scroll {
                        assert_eq!(
                            player.volume(),
                            1.0,
                            "accepted image scroll cannot become media volume input"
                        );
                        assert_eq!(
                            Some(state),
                            legacy_transform_and_timeline,
                            "the accepted image scroll changes neither the legacy transform nor timeline after switching media"
                        );
                    } else {
                        assert!(
                            player.volume() < 1.0,
                            "legacy plain input still reaches the media volume owner"
                        );
                        legacy_transform_and_timeline = Some(state);
                    }
                    let options = ctx.options(|options| options.input_options);
                    assert!(ctx.input(|input| {
                        crate::wheel_input::remaining_wheel_segments(input, options).is_empty()
                    }));
                },
            );
        }
    }
}

#[test]
fn vertical_and_horizontal_real_wheel_keep_the_legacy_continuous_scroll_owner() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
        for policy in [
            FullscreenWheelAction::PageTurn,
            FullscreenWheelAction::InPageScroll,
        ] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, SpreadMode::Single);
            seed(&ctx, &mut app);
            app.reading_flow = flow;
            app.settings.fullscreen_wheel_action = policy;
            app.settings.continuous_reading_wheel_scroll_percent = 20;
            app.fs_vertical_scroll = 200.0;
            app.fs_pan = Vec2::new(17.0, -23.0);
            let pan = app.fs_pan;
            let step = if flow == ReadingFlow::Horizontal {
                120.0
            } else {
                80.0
            };
            for (wheel_y, expected) in [(-120.0, 200.0 + step), (120.0, 200.0)] {
                frame(
                    &ctx,
                    vec![
                        egui::Event::PointerMoved(full_rect().center()),
                        wheel_event(Vec2::new(0.0, wheel_y), egui::Modifiers::NONE),
                    ],
                    |ui, ctx| {
                        assert!(app.continuous_reading_active_for_idx(0));
                        assert!(!app.fs_in_page_scroll_surface());
                        let navigation = pointer_handler(&mut app, ui, ctx);
                        assert!(navigation.is_none());
                        assert_eq!(app.fullscreen_idx, Some(0));
                        assert_eq!(
                            app.fs_pan, pan,
                            "the continuous axis never writes page-local pan"
                        );
                        assert_eq!(app.fs_zoom, 1.0);
                        assert_eq!(
                            app.fs_vertical_scroll, expected,
                            "flow={flow:?}, setting={policy:?} keeps the existing viewport-relative amount"
                        );
                        let options = ctx.options(|options| options.input_options);
                        assert!(ctx.input(|input| {
                            crate::wheel_input::remaining_wheel_segments(input, options).is_empty()
                        }));
                    },
                );
            }
        }
    }
}
