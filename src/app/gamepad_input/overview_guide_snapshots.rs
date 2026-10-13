//! Pure drawing snapshots complement the real-viewer chrome routing regressions.

use super::*;

fn overview_label(action: &RingActionId) -> String {
    App::ring_runtime_label_for_actions(
        RingShortcutContext::ImageFullscreen,
        action,
        Some(crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY),
    )
}

fn snapshot_guide(name: &str, draw: impl Fn(&egui::Ui) + 'static) {
    let mut fonts_ready = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(520.0, 440.0))
        .build(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    crate::os_theme::apply_dark_ui(ui);
                    draw(ui);
                });
        });
    harness.run();
    harness.snapshot(name);
}

#[test]
fn overview_guide_snapshot_gamepad_ring_dark() {
    let labels = [
        RingActionId::ImageSlideshow,
        RingActionId::ImagePageOverview,
        RingActionId::ToggleWindowMode,
        RingActionId::ToggleDetachedViewer,
        RingActionId::TreeFolderNext,
        RingActionId::SiblingFolderNext,
        RingActionId::ImageRotateLeft,
        RingActionId::ImageHome,
    ]
    .map(|action| overview_label(&action));

    assert_eq!(labels[0], "無効: スライドショー");
    assert_eq!(labels[1], "本のページ一覧");
    snapshot_guide("page_overview_gamepad_ring_dark", move |ui| {
        let full_rect = ui.max_rect();
        draw_ring_guide_donut(
            ui.painter(),
            full_rect.center(),
            ring_guide_radius_for_rect(full_rect),
            Some(RingDirection::UpRight),
            |direction| &labels[direction.slot_index()],
        );
    });
}

#[test]
fn overview_guide_snapshot_gesture_guide_dark() {
    let rows = [
        (MouseGestureDirection::Up, RingActionId::ImageSlideshow),
        (MouseGestureDirection::Left, RingActionId::ImageRotateLeft),
        (MouseGestureDirection::Down, RingActionId::ImagePageOverview),
    ]
    .into_iter()
    .map(|(direction, action)| RightDragGuideRow {
        label: format_mouse_gesture_pattern(&[direction]),
        value: overview_label(&action),
    })
    .collect();
    let guide = RightDragGuide::Gesture {
        current_pattern: format_mouse_gesture_pattern(&[MouseGestureDirection::Up]),
        rows,
        selected_row: Some(0),
        short_click_hint: "短押し操作なし".to_string(),
    };
    snapshot_guide("page_overview_gesture_guide_dark", move |ui| {
        draw_right_drag_guide(ui.painter(), ui.max_rect(), &guide);
    });
}
