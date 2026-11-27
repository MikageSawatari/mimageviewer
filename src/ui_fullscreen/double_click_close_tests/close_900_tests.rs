use super::*;

#[test]
fn close_mode_900_fast_second_double_click_closes_after_reset() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![]);
    canvas.app.fs_zoom = 2.0;
    for (count, closes) in [(1, false), (2, false), (3, false), (3, true)] {
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            (closes, false)
        );
        assert_eq!(canvas.raw_click_counts(), [count]);
        if count == 2 {
            assert_eq!(canvas.app.fs_zoom, 1.0);
        }
    }
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 2);
}

#[test]
fn close_mode_900_batched_four_clicks_only_reset_in_that_pass() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.merge(vec![]);
    canvas.app.fs_zoom = 2.0;
    let mut events = double_click(canvas_pos());
    events.extend(double_click(canvas_pos()));
    assert_eq!(canvas.merge(events), (false, false));
    assert_eq!(canvas.raw_click_counts(), [1, 2, 3, 3]);
    assert_eq!(canvas.app.fs_zoom, 1.0);
    assert_eq!(canvas.app.fs_front_pointer.clicks.current_accepted, 2);
}

#[test]
fn close_mode_900_three_fast_clicks_preserve_page_turn_behavior() {
    let _input = crate::key_input::lock_test_input();
    let mut canvas = Canvas::new();
    canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::PageTurn;
    canvas.merge(vec![]);
    canvas.app.fs_zoom = 2.0;
    for expected in [(false, false), (false, false), (false, true)] {
        assert_eq!(
            canvas.merge(click(canvas_pos(), egui::Modifiers::NONE)),
            expected
        );
    }
    assert_eq!(canvas.raw_click_counts(), [3]);
}

#[test]
fn close_mode_900_navigator_saturated_count_restarts_canvas_pairs() {
    let _input = crate::key_input::lock_test_input();
    for navigator_clicks in [2, 3] {
        let ctx = egui::Context::default();
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let (app, _, idx) = super::super::tests::setup_flat_navigator_input_test();
        let mut canvas = Canvas {
            ctx,
            app,
            time: 0.0,
        };
        canvas.app.settings.fullscreen_primary_click = FullscreenPrimaryClickMode::DoubleClickClose;
        canvas.app.settings.fullscreen_side_panel_mode =
            crate::settings::FsSidePanelMode::ClickToShow;
        canvas.app.fs_focus_regained_at = None;
        crate::app::source_texture_test_support::seed(&mut canvas.app, &canvas.ctx, idx);
        let media = canvas.app.fullscreen_media_rect_with_geometry(
            viewport(),
            idx,
            false,
            StillSeekGeometry::bar_only(),
        );
        let layout = canvas.app.fs_navigator_layout(media).unwrap();
        let pos = layout.canvas_rect.left_center() + egui::vec2(1.0, 0.0);
        let blank = layout.canvas_rect.left_center() - egui::vec2(2.0, 0.0);
        assert!(!layout.panel_rect.contains(blank));
        canvas.merge(vec![egui::Event::PointerMoved(pos)]);
        for _ in 0..navigator_clicks {
            assert_eq!(
                canvas.merge(click(pos, egui::Modifiers::NONE)),
                (false, false)
            );
        }
        assert_eq!(
            canvas.app.fs_front_pointer.clicks.current_accepted,
            navigator_clicks
        );
        canvas.app.settings.fullscreen_navigator_visible = false;
        canvas.app.fs_zoom = 2.0;
        for expected in [false, false, false, true] {
            assert_eq!(
                canvas.merge(click(blank, egui::Modifiers::NONE)),
                (expected, false)
            );
            assert_eq!(canvas.raw_click_counts(), [3]);
        }
        assert_eq!(canvas.app.fs_zoom, 1.0);
    }
}
