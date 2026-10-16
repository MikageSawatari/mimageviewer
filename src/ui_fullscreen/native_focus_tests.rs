use super::*;

#[test]
fn headless_fullscreen_focus_records_request_only_for_foreign_foreground() {
    for foreign in [false, true] {
        let foreground = 11;
        let target = if foreign { 22 } else { foreground };
        let _focus = crate::test_native_focus::scoped(foreground, target, !foreign);
        let ctx = egui::Context::default();
        install_fs_navigator_input_tracking(&ctx);
        let mut app = specified_scale_consumer_tests::pdf_app(&ctx, 1);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.settings.vst3_gui_visible = false;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let draw = |app: &mut App, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events,
                    ..Default::default()
                },
                |ctx| app.render_fullscreen_viewport(ctx),
            )
        };
        for _ in 0..5 {
            let _ = draw(&mut app, vec![]);
        }
        assert_eq!(app.fs_prev_foreground_hwnd, foreground as usize);
        // Opening has a separate focus grace period. Test a settled viewport click.
        app.fs_focus_regained_at = None;
        assert!(!app.fs_primary_suppression.is_active());
        let output = draw(
            &mut app,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(
            crate::test_native_focus::requests(),
            if foreign { vec![target] } else { vec![] },
            "the real fullscreen input handler must preserve its focus decision",
        );
        let focus_command = output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus))
        });
        assert_eq!(focus_command, foreign);
        assert_eq!(app.fs_primary_suppression.is_active(), foreign);
    }
}
