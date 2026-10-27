// Included in the existing GPU-free native overlay/batch fixture module.
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
