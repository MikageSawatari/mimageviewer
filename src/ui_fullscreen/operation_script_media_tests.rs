//! Stage 3 media bodies and real egui drawing-owner entry points.
use super::*;
use crate::operation_script::{CompiledOperationScript, OperationScriptState};

fn raw(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1093.0, 1800.0),
        )),
        focused: true,
        events,
        ..Default::default()
    }
}

fn key(pressed: bool) -> egui::Event {
    egui::Event::Key {
        key: egui::Key::F17,
        physical_key: Some(egui::Key::F17),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn ready(
    ctx: &egui::Context,
    media: Option<bool>,
    commands: &str,
) -> crate::app::AppTestEnvForTest {
    crate::ui_fonts::configure_fonts(ctx);
    install_fs_navigator_input_tracking(ctx);
    crate::key_input::install_operation_input_plugin(ctx);
    let mut app = page_turn_submission_tests::ready_app(ctx, SpreadMode::Single);
    app.settings.operation_script_enabled = true;
    app.settings.page_turn_animation_enabled = false;
    app.settings.video_in_window_mode = true;
    app.native_video_in_window_active = true;
    app.viewer_presentation = ViewerPresentation::MainWindow;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app.fs_info_panel.locked = false;
    app.keymap = crate::keymap::Keymap::from_ini_str("[Global]\nScriptSlot1 = F17\n");
    let source = format!(
        "fn slots() {{ [#{{slot:1,label:\"owner-media\",run:\"entry\"}}] }}\nfn entry(s) {{ {commands} }}"
    );
    app.operation_script.state =
        OperationScriptState::Ready(Box::new(CompiledOperationScript::compile(&source).unwrap()));
    if let Some(audio) = media {
        let path = app
            .tmp
            .path()
            .join(if audio { "owner.mp3" } else { "owner.mp4" });
        app.items[0] = if audio {
            GridItem::Audio(path.clone())
        } else {
            GridItem::Video(path.clone())
        };
        let player = crate::video::VideoPlayer::disconnected_for_test(path, 12.0);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 0,
            },
        );
        if !audio {
            // The released egui video drawing path is the video's music view.
            app.video_audio_mode = Some(0);
        }
    }
    app
}

fn paint(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    app.frame_counter += 1;
    let mut input = raw(events);
    input.time = Some(app.frame_counter as f64 / 30.0);
    crate::test_ui::headless_paint_output(
        ctx.run_ui(input, |ui| app.render_fullscreen_viewport(ui)),
    )
}

fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    paint(app, ctx, events).drop_without_applying_deltas();
}

fn text_center(output: &egui::FullOutput, expected: &str) -> egui::Pos2 {
    fn find(shape: &egui::Shape, clip: egui::Rect, expected: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == expected => {
                let pos = text.galley.rect.translate(text.pos.to_vec2()).center();
                clip.contains(pos).then_some(pos)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, clip, expected)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, shape.clip_rect, expected))
        .unwrap_or_else(|| panic!("actual fullscreen menu must paint {expected:?}"))
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    frame(app, ctx, vec![egui::Event::PointerMoved(pos)]);
    frame(app, ctx, vec![button(pos, true)]);
    frame(app, ctx, vec![button(pos, false)]);
}

#[test]
fn operation_script_media_actual_menu_choice_reaches_owner_tail_after_menu_closes() {
    let _input = crate::key_input::lock_test_input();
    for media in [None, Some(true), Some(false)] {
        let ctx = egui::Context::default();
        let commands = if media.is_some() {
            r#"["VideoMute"]"#
        } else {
            r#"["FsSpreadRtl"]"#
        };
        let mut app = ready(&ctx, media, commands);
        assert!(app.fullscreen_embedded_still_active());
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        let before = app.settings.video_muted;
        // No registered HWND: the normal menu entry selects its egui fallback.
        app.fs_context_menu_idx = Some(0);
        app.fs_context_menu_pos = egui::pos2(30.0, 20.0);
        let output = paint(&mut app, &ctx, vec![]);
        let header = text_center(&output, "スクリプト");
        output.drop_without_applying_deltas();
        click(&mut app, &ctx, header);
        for _ in 0..20 {
            frame(&mut app, &ctx, vec![]);
        }
        let output = paint(&mut app, &ctx, vec![]);
        let row = text_center(&output, "スクリプト操作 1: owner-media");
        output.drop_without_applying_deltas();
        assert!(app.fs_context_menu_idx.is_some());
        assert_eq!(app.spread_mode, SpreadMode::Single);
        assert_eq!(app.settings.video_muted, before);
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(row), button(row, true)],
        );
        assert!(
            app.fs_context_menu_idx.is_some(),
            "press alone must not execute the row"
        );
        assert_eq!(app.settings.video_muted, before);
        frame(&mut app, &ctx, vec![button(row, false)]);
        assert!(
            app.fs_context_menu_idx.is_none(),
            "the chosen menu must close before execution"
        );
        if media.is_some() {
            assert_eq!(app.settings.video_muted, !before);
        } else {
            assert_eq!(
                app.spread_mode,
                SpreadMode::Rtl,
                "tail execution survives display-settings restoration"
            );
        }
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            app.settings.video_muted,
            if media.is_some() { !before } else { before }
        );
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn operation_script_media_actual_owner_keys_execute_for_audio_and_video_music_views() {
    let _input = crate::key_input::lock_test_input();
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, Some(audio), r#"["VideoMute"]"#);
        let before = app.settings.video_muted;
        frame(&mut app, &ctx, vec![key(true)]);
        assert_eq!(app.settings.video_muted, !before);
        frame(&mut app, &ctx, vec![key(false)]);
        assert_eq!(
            app.settings.video_muted, !before,
            "the local request executes only once"
        );
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn operation_script_media_real_forwarded_keys_reach_child_owner_tail() {
    let _input = crate::key_input::lock_test_input();
    for media in [None, Some(true), Some(false)] {
        let ctx = egui::Context::default();
        let root = egui::Context::default();
        crate::key_input::install_operation_input_plugin(&root);
        ctx.set_embed_viewports(false);
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_passes = observed.clone();
        ctx.on_begin_pass(
            "script_forwarded_owner_probe",
            Arc::new(move |ui| {
                observed_passes.lock().unwrap().push(ui.ctx().viewport_id());
            }),
        );
        egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
            let mut input = raw(vec![]);
            input.viewport_id = viewport.ids.this;
            input.viewports.insert(
                viewport.ids.this,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(true),
                    ..Default::default()
                },
            );
            ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui))
                .drop_without_applying_deltas();
        });
        let commands = if media.is_some() {
            r#"["VideoMute"]"#
        } else {
            r#"["FsSpreadRtl"]"#
        };
        let mut app = ready(&ctx, media, commands);
        app.settings.video_in_window_mode = false;
        app.native_video_in_window_active = false;
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        let before = app.settings.video_muted;
        root.run_pass(raw(vec![key(true)]), |source| {
            // The production forwarding value carries the source pass and recipient.
            let forwarded = FullscreenRootFrameInput {
                context: source.clone(),
                destination: app.page_turn_destination(),
            };
            let mut forwarded = Some(forwarded);
            ctx.run_ui(raw(vec![]), |ui| {
                app.render_fullscreen_viewport_with_root_input(ui, forwarded.take());
            })
            .drop_without_applying_deltas();
        })
        .drop_without_applying_deltas();
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .any(|id| *id != egui::ViewportId::ROOT)
        );
        if media.is_some() {
            assert_eq!(app.settings.video_muted, !before);
        } else {
            assert_eq!(app.spread_mode, SpreadMode::Rtl);
        }
        assert_eq!(ctx.viewport_id(), egui::ViewportId::ROOT);
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn operation_script_media_same_context_root_forwarding_reaches_child_owner_tail() {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, None, r#"["FsNavigatorToggle"]"#);
    app.settings.video_in_window_mode = false;
    app.native_video_in_window_active = false;
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    page_turn_submission_tests::seed_leaf(&ctx, &mut app);
    assert!(app.page_turn_barrier_active());
    ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
        let mut input = raw(vec![]);
        input.viewport_id = viewport.ids.this;
        input.viewports.insert(
            viewport.ids.this,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                focused: Some(true),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui))
            .drop_without_applying_deltas();
    });
    let before = app.settings.fullscreen_navigator_visible;
    ctx.run_ui(raw(vec![key(true)]), |ui| {
        let (handled, forwarded) = app.fullscreen_root_input_for_frame(ui.ctx());
        assert!(handled);
        let forwarded = forwarded.expect("a page-turn barrier forwards ROOT's input to the viewer");
        assert_eq!(
            app.settings.fullscreen_navigator_visible, before,
            "ROOT only forwards"
        );
        app.render_fullscreen_viewport_with_root_input(ui, Some(forwarded));
    })
    .drop_without_applying_deltas();
    assert_eq!(
        app.settings.fullscreen_navigator_visible, !before,
        "the genuine shared-Context ROOT input must reach the child drawing owner's tail"
    );
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_media_resolved_bodies_match_real_video_and_music_key_handlers() {
    let _input = crate::key_input::lock_test_input();
    for audio in [false, true] {
        for action in [
            KeyAction::VideoPlayPause,
            KeyAction::VideoMute,
            KeyAction::VideoVolumeUp,
            KeyAction::VideoVolumeDown,
            KeyAction::VideoLoop,
            KeyAction::VideoSeekStart,
            KeyAction::VideoSeekBackSmall,
            KeyAction::VideoSeekForwardSmall,
            KeyAction::VideoSeekBackMedium,
            KeyAction::VideoSeekForwardMedium,
            KeyAction::VideoSeekBackLarge,
            KeyAction::VideoSeekForwardLarge,
            KeyAction::VideoSeekStripToggle,
            KeyAction::VideoPrevFile,
            KeyAction::VideoNextFile,
        ] {
            if audio && action == KeyAction::VideoSeekStripToggle {
                continue;
            }
            let mut results = Vec::new();
            for from_key in [true, false] {
                let ctx = egui::Context::default();
                let mut app = ready(&ctx, Some(audio), "[]");
                if !audio {
                    app.video_audio_mode = None;
                }
                let path = match app.items.first().unwrap() {
                    GridItem::Video(p) | GridItem::Audio(p) => p.clone(),
                    _ => unreachable!(),
                };
                let mut player =
                    crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
                player.set_position_for_test(12.0);
                player.set_duration_for_test(120.0);
                app.fs_cache.insert(
                    0,
                    FsCacheEntry::Video {
                        player: Box::new(player),
                        load_seq: 0,
                    },
                );
                app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                    "[{}]\n{} = F17\n",
                    action.context().ini_name(),
                    action.ini_name()
                ));
                ctx.run_pass(
                    raw(if from_key { vec![key(true)] } else { vec![] }),
                    |ctx| {
                        let outcome = if from_key {
                            app.handle_fs_key_input(ctx, 0, false)
                        } else {
                            app.execute_fullscreen_script_action(ctx, action).0
                        };
                        let player = app.fs_video_player(0).unwrap();
                        results.push(format!(
                            "{:?}",
                            (
                                (
                                    outcome.close,
                                    outcome.close_to_page_list,
                                    outcome.page_nav,
                                    outcome.jump_to
                                ),
                                (
                                    player.volume(),
                                    player.is_paused_or_seeking(),
                                    player.user_seek_base_secs()
                                ),
                                (
                                    app.settings.video_muted,
                                    app.settings.video_volume,
                                    app.settings.video_loop_mode,
                                    app.video_seek_strip_is_open()
                                ),
                            )
                        ));
                    },
                )
                .drop_without_applying_deltas();
                crate::key_input::clear_test_frame();
            }
            assert_eq!(results[0], results[1], "audio={audio}, action={action:?}");
        }
    }
}

#[test]
fn operation_script_media_music_capture_refusal_stops_before_notice() {
    let _input = crate::key_input::lock_test_input();
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(
            &ctx,
            Some(audio),
            r#"#{run:["VideoCapture"],notice:"unexpected"}"#,
        );
        frame(&mut app, &ctx, vec![key(true)]);
        assert!(app.capture_pending.is_none());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|(text, _)| text.contains("実行できませんでした"))
        );
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn operation_script_media_continuous_loop_refuses_script_without_changing_key_behavior() {
    let _input = crate::key_input::lock_test_input();
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, Some(audio), "[]");
        if !audio {
            app.video_audio_mode = None;
        }
        app.video_continuous_mode = crate::video::VideoContinuousMode::Continuous;
        let before = app.settings.video_loop_mode;
        ctx.run_pass(raw(vec![]), |ctx| {
            assert!(
                matches!(
                    app.execute_video_key_action(
                        ctx,
                        0,
                        KeyAction::VideoLoop,
                        if audio {
                            VideoKeyActionSurface::EguiMusic
                        } else {
                            VideoKeyActionSurface::EguiVideo
                        }
                    ),
                    VideoKeyActionResult::Accepted(_)
                ),
                "ordinary keys keep their consumed/no-op result"
            );
            assert_eq!(app.settings.video_loop_mode, before);
            assert!(
                app.execute_fullscreen_script_action(ctx, KeyAction::VideoLoop)
                    .1,
                "a script must refuse the no-op rather than announce completion"
            );
        })
        .drop_without_applying_deltas();
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn operation_script_media_music_loop_key_keeps_setting_changes_before_player_is_cached() {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, Some(true), "[]");
    app.fs_cache.remove(&0);
    app.keymap = crate::keymap::Keymap::from_ini_str("[FsVideo]\nVideoLoop = F17\n");
    let before = app.settings.video_loop_mode;
    ctx.run_pass(raw(vec![key(true)]), |ctx| {
        let _ = app.handle_fs_key_input(ctx, 0, false);
        assert_ne!(
            app.settings.video_loop_mode, before,
            "music's released loop setting remains usable while its player is loading"
        );
    })
    .drop_without_applying_deltas();
    crate::key_input::clear_test_frame();
}

#[test]
fn operation_script_media_close_keeps_immediate_video_and_deferred_music_results() {
    let _input = crate::key_input::lock_test_input();
    for audio in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, Some(audio), "[]");
        if !audio {
            app.video_audio_mode = None;
        }
        ctx.run_pass(raw(vec![]), |ctx| {
            let (action, rejected, delegated) =
                app.execute_fullscreen_script_action(ctx, KeyAction::VideoCloseFullscreen);
            assert!(!rejected && !delegated);
            if audio {
                assert!(action.close);
                assert_eq!(action.page_transition, FsKeyPageTransition::Unchanged);
                assert_eq!(app.fullscreen_idx, Some(0));
                app.finish_fullscreen_key_action(ctx, 0, action);
                assert_eq!(app.fullscreen_idx, None);
            } else {
                assert_eq!(app.fullscreen_idx, None);
                assert_eq!(action.page_transition, FsKeyPageTransition::Closed);
                assert!(
                    !action.close,
                    "already-closed video must not close a second time"
                );
            }
        })
        .drop_without_applying_deltas();
        crate::key_input::clear_test_frame();
    }
}
