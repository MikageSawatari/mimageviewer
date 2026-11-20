use super::*;
use crate::settings::FullscreenNScrollOrder;

#[test]
fn reading_order_overflow_real_controls_share_and_persist_the_global_setting() {
    use egui_kittest::kittest::Queryable;
    let mut app = crate::app::setup_app_for_test();
    app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    for order in [
        FullscreenNScrollOrder::HorizontalFirst,
        FullscreenNScrollOrder::VerticalFirst,
    ] {
        {
            let mut fonts_ready = false;
            let mut harness = crate::test_ui::builder()
                .with_size(egui::vec2(800.0, 800.0))
                .build_ui(|root| {
                    let ctx = root.ctx().clone();
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(&ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        app.draw_fs_overflow_panel(ui, &ctx, ui.max_rect(), false);
                    });
                });
            harness.run();
            harness.get_by_label("ページ内の読み順").click();
            harness.run();
            harness.get_by_label(order.label()).click();
            harness.run();
            harness.get_by_label("<  戻る").click();
            harness.run();
        }
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Root);
        assert_eq!(app.settings.fullscreen_n_scroll_order, order);
        let saved = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
        assert_eq!(saved.fullscreen_n_scroll_order, order);
    }
}

#[test]
fn reading_order_overflow_is_global_in_all_image_modes_and_closes_like_existing_panels() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    assert!(!FsOverflowPanelState::PageReadingOrder.closes_on_file_change());
    for flow in [
        ReadingFlow::Paged,
        ReadingFlow::Vertical,
        ReadingFlow::Horizontal,
    ] {
        app.reading_flow = flow;
        let view = app.fullscreen_overflow_panel_view(false);
        assert!(
            view.root_rows
                .iter()
                .any(|row| row.item == FullscreenOverflowItemId::PageReadingOrder && row.enabled)
        );
        app.apply_fs_overflow_panel_action(
            &ctx,
            &view,
            false,
            FsOverflowPanelAction::Activate(FullscreenOverflowItemId::PageReadingOrder),
        );
        assert_eq!(
            app.fs_overflow_panel_state,
            FsOverflowPanelState::PageReadingOrder
        );
        app.apply_fs_overflow_panel_action(&ctx, &view, false, FsOverflowPanelAction::Close);
        assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Closed);
    }
    assert_eq!(
        fs_overflow_panel_body_row_count(FsOverflowPanelState::PageReadingOrder),
        3
    );
    // §1.830 added the detached viewer toggle row after the fixed items.
    assert_eq!(
        fs_overflow_panel_body_row_count(FsOverflowPanelState::Root),
        12
    );
}
