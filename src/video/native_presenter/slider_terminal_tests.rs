// Included in the existing GPU-free native overlay/batch fixture module.
#[test]
fn hud_press_native_first_move_and_release_in_one_pass() {
    for seek in [false, true] {
        let mut target = target(false);
        let name = if seek {
            "native_video_seek_hit"
        } else {
            "native_video_volume"
        };
        let start = press_focused_hud_control(&mut target, name);
        let end = (start + egui::vec2(22.0, 0.0)).round();
        let rect = target
            .state
            .egui_ctx
            .read_response(egui::Id::unique(name))
            .unwrap()
            .rect;
        let expected = if seek {
            seek_target(&target.state, end)
        } else {
            crate::settings::video_volume_fader_pos_to_linear(
                ((end.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64,
            )
        };
        let run = run_native_event_batch(&mut target, &[up(end)]).unwrap();
        if seek {
            assert_seek(&run.commands, expected);
        } else {
            assert!(
                matches!(run.commands.as_slice(), [NativeOverlayCommand::SetVolume { volume, persist: true }] if *volume == expected),
                "{:?}",
                run.commands
            );
        }
    }
}

#[test]
fn hud_press_native_seek_reset_retires_press_before_any_drag() {
    for move_after_reset in [false, true] {
        let mut target = target(false);
        let start = press_focused_hud_control(&mut target, "native_video_seek_hit");
        target.state.reset_side_panel_session();
        let end = if move_after_reset {
            start + egui::vec2(40.0, 0.0)
        } else {
            start
        };
        if move_after_reset {
            pointer(&mut target.state, end);
            let output = frame(&mut target.state);
            assert!(!output.commands.iter().any(|c| matches!(
                c,
                NativeOverlayCommand::Seek { .. } | NativeOverlayCommand::OpenSeekStrip
            )));
        }
        let run = run_native_event_batch(&mut target, &[up(end)]).unwrap();
        assert!(
            !run.commands.iter().any(|c| matches!(
                c,
                NativeOverlayCommand::Seek { .. } | NativeOverlayCommand::OpenSeekStrip
            )),
            "{:?}",
            run.commands
        );
        let next = press_focused_hud_control(&mut target, "native_video_seek_hit");
        let run = run_native_event_batch(&mut target, &[up(next)]).unwrap();
        assert_seek(&run.commands, seek_target(&target.state, next));
    }
}

#[test]
fn hud_slider_native_release_uses_receipt_before_later_move() {
    let mut target = target(false);
    let start = press_focused_hud_control(&mut target, "native_video_volume");
    pointer(&mut target.state, start + egui::vec2(10.0, 0.0));
    frame(&mut target.state);
    let rect = target
        .state
        .egui_ctx
        .read_response(egui::Id::unique("native_video_volume"))
        .unwrap()
        .rect;
    let release = (start + egui::vec2(22.0, 0.0)).round();
    let expected = crate::settings::video_volume_fader_pos_to_linear(
        ((release.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64,
    );
    let run = run_native_event_batch(&mut target, &[up(release), moved(start)]).unwrap();
    let values: Vec<_> = run
        .commands
        .iter()
        .filter_map(|command| match command {
            NativeOverlayCommand::SetVolume { volume, persist } => Some((*volume, *persist)),
            _ => None,
        })
        .collect();
    assert_eq!(values, vec![(expected, true)]);
    assert!(target.state.last_volume_target.is_none());
    assert_one_present(&target, 2);
}

#[test]
fn hud_slider_native_capture_loss_and_escape_restore_original_once() {
    for escape_cancel in [false, true] {
        let mut target = target(false);
        let original = target.state.video_volume;
        let start = press_focused_hud_control(&mut target, "native_video_volume");
        pointer(&mut target.state, start + egui::vec2(12.0, 0.0));
        frame(&mut target.state);
        let cancel = if escape_cancel {
            escape()
        } else {
            mouse(egui::Pos2::ZERO, false, NativeVideoMouseButton::Left, true)
        };
        let run = run_native_event_batch(&mut target, &[cancel, up(start + egui::vec2(30.0, 0.0))])
            .unwrap();
        let values: Vec<_> = run
            .commands
            .iter()
            .filter_map(|command| match command {
                NativeOverlayCommand::SetVolume { volume, persist } => Some((*volume, *persist)),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![(original, true)], "escape={escape_cancel}");
        assert!(target.state.last_volume_target.is_none());
    }
}

#[test]
fn hud_slider_native_source_or_mode_reset_retires_volume_and_seek() {
    for seek in [false, true] {
        let mut target = target(false);
        let name = if seek {
            "native_video_seek_hit"
        } else {
            "native_video_volume"
        };
        let start = press_focused_hud_control(&mut target, name);
        pointer(&mut target.state, start + egui::vec2(20.0, 0.0));
        frame(&mut target.state);
        assert!(if seek {
            target.state.seek_row_gesture.is_some()
        } else {
            target.state.last_volume_target.is_some()
        });
        // Both source replacement and video/audio mode changes call this owner.
        target.state.reset_side_panel_session();
        assert!(target.state.last_volume_target.is_none());
        assert!(target.state.seek_row_gesture.is_none());
        let other = target
            .state
            .egui_ctx
            .read_response(egui::Id::unique("native_video_play"))
            .unwrap()
            .rect
            .center();
        let run = run_native_event_batch(&mut target, &[up(other), moved(other)]).unwrap();
        assert!(!run.commands.iter().any(|c| matches!(
            c,
            NativeOverlayCommand::SetVolume { .. } | NativeOverlayCommand::Seek { .. }
        )));
        let run = run_native_event_batch(
            &mut target,
            &[
                mouse(other, true, NativeVideoMouseButton::Left, false),
                up(other),
            ],
        )
        .unwrap();
        assert!(
            run.commands
                .iter()
                .any(|c| matches!(c, NativeOverlayCommand::TogglePlay))
        );
        assert!(!run.commands.iter().any(|c| matches!(
            c,
            NativeOverlayCommand::SetVolume { .. } | NativeOverlayCommand::Seek { .. }
        )));
    }
}

#[test]
fn hud_slider_native_hidden_hud_ends_volume_before_redisplay() {
    let mut target = target(false);
    let start = press_focused_hud_control(&mut target, "native_video_volume");
    pointer(&mut target.state, start + egui::vec2(20.0, 0.0));
    frame(&mut target.state);
    assert!(target.state.last_volume_target.is_some());
    target
        .state
        .set_bar_lock_state(crate::video::NativeBarLockState::default());
    pointer(&mut target.state, egui::pos2(640.0, 360.0));
    let hidden = frame(&mut target.state);
    assert!(!hidden.hud_visible);
    assert!(target.state.last_volume_target.is_none());
    let run = run_native_event_batch(&mut target, &[up(egui::pos2(0.0, 360.0))]).unwrap();
    assert!(
        !run.commands
            .iter()
            .any(|c| matches!(c, NativeOverlayCommand::SetVolume { .. }))
    );
    target
        .state
        .set_bar_lock_state(crate::video::NativeBarLockState {
            bottom_lock: BottomBarLock::BarAndStrip,
            ..Default::default()
        });
    frame(&mut target.state);
    let other = press_focused_hud_control(&mut target, "native_video_play");
    let run = run_native_event_batch(&mut target, &[up(other)]).unwrap();
    assert!(
        run.commands
            .iter()
            .any(|c| matches!(c, NativeOverlayCommand::TogglePlay))
    );
    assert!(
        !run.commands
            .iter()
            .any(|c| matches!(c, NativeOverlayCommand::SetVolume { .. }))
    );
}
