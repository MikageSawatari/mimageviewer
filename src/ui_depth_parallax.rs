//! Pure drawing for the fullscreen 3D-photo controls; the viewer owns mode lifetime.

#[cfg(test)]
#[path = "ui_depth_parallax_tests.rs"]
mod tests;

use crate::depth_parallax::{DepthParallaxSettings, MotionKind};
use eframe::egui;

pub struct DepthPanelResponse {
    pub changed: bool,
    pub focus_adjusting: bool,
    pub close_requested: bool,
    pub export_requested: bool,
    pub rect: egui::Rect,
    pub header_rect: egui::Rect,
    pub footer_rect: egui::Rect,
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
    panel_toggle_hint: Option<String>,
) -> DepthPanelResponse {
    draw_depth_parallax_panel_with_export(ctx, settings, status, false, panel_toggle_hint)
}

pub fn draw_depth_parallax_panel_with_export(
    ctx: &egui::Context,
    settings: &mut DepthParallaxSettings,
    status: &str,
    export_enabled: bool,
    panel_toggle_hint: Option<String>,
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
                // Measure the very same wrapped galleys that the fixed footer paints.
                // Include both its row spacing and the gap after the scroll body.
                let footer_rows: Vec<_> = std::iter::once(
                    "左 Ctrl+クリック: その場所の深さで止める",
                )
                .chain(panel_toggle_hint.as_deref())
                .map(|text| {
                    egui::WidgetText::from(
                        egui::RichText::new(text)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Wrap),
                        content_width,
                        egui::TextStyle::Small,
                    )
                })
                .collect();
                let footer_height = footer_rows.iter().map(|row| row.size().y).sum::<f32>()
                    + ui.spacing().item_spacing.y * footer_rows.len() as f32;
                // Area's natural height is not a usable ScrollArea parent constraint.
                // Reserve the real body viewport, subtracting the measured header/footer and frame.
                let body_height = (screen.bottom()
                    - 12.0
                    - f32::from(frame.inner_margin.bottom)
                    - ui.cursor().top()
                    - footer_height)
                .clamp(0.0, 380.0);
                let mut changed = false;
                let mut focus_adjusting = false;
                let mut export_rect = egui::Rect::NOTHING;
                let mut export_requested = false;
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
                                    let motion_combo = egui::ComboBox::from_id_salt("depth_motion")
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
                                        });
                                    if motion_combo.inner.is_some() {
                                        // The fixed panel and menus both use Foreground.
                                        // Own this popup as a child so pressing a panel
                                        // control cannot raise the panel over its menu.
                                        ctx.set_sublayer(
                                            motion_combo.response.layer_id,
                                            egui::LayerId::new(
                                                egui::Order::Foreground,
                                                motion_combo.response.id.with("popup"),
                                            ),
                                        );
                                    }
                                    motion_combo.response.labelled_by(motion_label.id);
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
                                    let focus = ui
                                        .add(egui::Slider::new(&mut settings.focus, 0.0..=1.0))
                                        .labelled_by(focus_label.id);
                                    changed |= focus.changed();
                                    focus_adjusting |= focus.dragged() || focus.changed();
                                    ui.horizontal(|ui| {
                                        ui.weak("奥");
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.weak("手前");
                                            },
                                        );
                                    });
                                    if ui.small_button("既定値").clicked() {
                                        settings.focus = 0.5;
                                        changed = true;
                                        focus_adjusting = true;
                                    }
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
                                        .add_enabled(export_enabled, egui::Button::new("書き出し…"))
                                        .on_disabled_hover_text("奥行きの計算が終わると書き出せます");
                                    export_rect = export.rect;
                                    export_requested = export.clicked();
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
                let footer = ui.vertical(|ui| {
                    for row in footer_rows {
                        ui.label(row);
                    }
                });
                (
                    changed,
                    focus_adjusting,
                    close_requested,
                    header.response.rect,
                    footer.response.rect,
                    close_rect,
                    export_rect,
                    export_requested,
                    scroll,
                )
            })
        });
    let (
        changed,
        focus_adjusting,
        close_requested,
        header_rect,
        footer_rect,
        close_rect,
        export_rect,
        export_requested,
        scroll,
    ) = output.inner.inner;
    DepthPanelResponse {
        changed,
        focus_adjusting,
        close_requested,
        export_requested,
        rect: output.response.rect,
        header_rect,
        footer_rect,
        close_rect,
        export_rect,
        scroll,
    }
}
