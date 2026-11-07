//! Pure drawing for the fullscreen 3D-photo controls; the viewer owns mode lifetime.

use crate::depth_parallax::{DepthParallaxSettings, MotionKind};
use eframe::egui;

pub struct DepthPanelResponse {
    pub changed: bool,
    pub close_requested: bool,
    pub rect: egui::Rect,
    pub header_rect: egui::Rect,
    pub close_rect: egui::Rect,
    pub export_rect: egui::Rect,
    pub scroll: egui::scroll_area::ScrollAreaOutput<()>,
}

/// Draw a fixed right panel. `status` is supplied by the request owner, with no I/O here.
/// The returned rectangle is also the viewer's canvas-input exclusion boundary.
pub fn draw_depth_parallax_panel(
    ctx: &egui::Context,
    settings: &mut DepthParallaxSettings,
    status: &str,
) -> DepthPanelResponse {
    let screen = ctx.content_rect();
    let frame = crate::os_theme::dark_popup_frame(ctx);
    let width = 300.0_f32.min((screen.width() - 24.0).max(120.0));
    let content_width = width - frame.inner_margin.sum().x;
    let position = egui::pos2(screen.right() - width - 12.0, screen.top() + 60.0);
    let output = egui::Area::new(egui::Id::unique("depth_parallax_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(position)
        // Position and body bounds already come from the viewport. Area's
        // estimated natural size must not move this fixed header while sizing.
        .constrain(false)
        .show(ctx, |ui| {
            crate::os_theme::apply_dark_ui(ui);
            frame.show(ui, |ui| {
                ui.set_width(content_width);
                let mut close_requested = false;
                let mut close_rect = egui::Rect::NOTHING;
                let header = ui.horizontal(|ui| {
                    ui.heading("3D 写真");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close = ui.button("×").on_hover_text("3D 写真を終了");
                        close_requested = close.clicked();
                        close_rect = close.rect;
                    });
                });
                ui.separator();
                // Area's natural height is not a usable ScrollArea parent constraint.
                // Reserve the real body viewport, subtracting the measured header and frame.
                let body_height = (screen.bottom()
                    - 12.0
                    - f32::from(frame.inner_margin.bottom)
                    - ui.cursor().top())
                .clamp(0.0, 380.0);
                let mut changed = false;
                let mut export_rect = egui::Rect::NOTHING;
                let scroll = ui
                    .allocate_ui_with_layout(
                        egui::vec2(content_width, body_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            // Keep floating bars out of labels and numeric controls.
                            let scroll_style = &mut ui.style_mut().spacing.scroll;
                            scroll_style.floating_allocated_width =
                                scroll_style.bar_inner_margin + scroll_style.bar_width;
                            egui::ScrollArea::vertical()
                                .id_salt("depth_parallax_controls")
                                .max_height(body_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let motion_label = ui.label("動き");
                                    egui::ComboBox::from_id_salt("depth_motion")
                                        .selected_text(settings.motion.label())
                                        .popup_style(crate::os_theme::dark_popup_style(ctx))
                                        .show_ui(ui, |ui| {
                                            for &motion in MotionKind::all() {
                                                changed |= ui
                                                    .selectable_value(
                                                        &mut settings.motion,
                                                        motion,
                                                        motion.label(),
                                                    )
                                                    .changed();
                                            }
                                        })
                                        .response.labelled_by(motion_label.id);
                                    ui.add_space(6.0);
                                    let strength_label = ui.label("強さ");
                                    let mut percent = settings.strength * 100.0;
                                    if ui
                                        .add(egui::Slider::new(&mut percent, 0.5..=5.0).suffix(" %"))
                                        .labelled_by(strength_label.id)
                                        .changed()
                                    {
                                        settings.strength = percent / 100.0;
                                        changed = true;
                                    }
                                    let period_label = ui.label("周期");
                                    changed |= ui
                                        .add_enabled(
                                            settings.motion != MotionKind::Mouse,
                                            egui::Slider::new(&mut settings.period_secs, 1.0..=10.0)
                                                .suffix(" 秒"),
                                        )
                                        .labelled_by(period_label.id)
                                        .changed();
                                    let focus_label = ui.label("止まる深さ");
                                    changed |= ui
                                        .add(egui::Slider::new(&mut settings.focus, 0.0..=1.0))
                                        .labelled_by(focus_label.id)
                                        .changed();
                                    ui.horizontal(|ui| {
                                        ui.weak("奥");
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.weak("手前");
                                            },
                                        );
                                    });
                                    ui.add_space(6.0);
                                    changed |= ui
                                        .checkbox(&mut settings.shape_depth, "奥行きを整える")
                                        .changed();
                                    ui.label(
                                        egui::RichText::new(
                                            "手前の床などが奥行きを占めて平らに見えるのを防ぎます。",
                                        )
                                        .small()
                                        .color(ui.visuals().weak_text_color()),
                                    );
                                    ui.separator();
                                    let export = ui
                                        .add_enabled(false, egui::Button::new("書き出し…"))
                                        .on_disabled_hover_text("書き出しは準備中です");
                                    export_rect = export.rect;
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(if status.is_empty() {
                                                " "
                                            } else {
                                                status
                                            })
                                            .small()
                                            .color(ui.visuals().weak_text_color()),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(status);
                                })
                        },
                    )
                    .inner;
                (
                    changed,
                    close_requested,
                    header.response.rect,
                    close_rect,
                    export_rect,
                    scroll,
                )
            })
        });
    let (changed, close_requested, header_rect, close_rect, export_rect, scroll) =
        output.inner.inner;
    DepthPanelResponse {
        changed,
        close_requested,
        rect: output.response.rect,
        header_rect,
        close_rect,
        export_rect,
        scroll,
    }
}
