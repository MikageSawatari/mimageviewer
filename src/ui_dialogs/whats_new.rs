//! 更新後 初回起動の「重要な変更点」ダイアログ (v2.0.0 で仕組み導入、version_highlights)。
//!
//! - 表示判定 (どのバージョンの変更点を出すか) は `App` 構築時に
//!   [`crate::version_highlights::highlights_to_show`] (純関数) で決め、`whats_new_entries` に入る。
//! - ここはその描画だけを担当する (display-only。移行の二択 UI は持たない)。
//! - `last_seen_version` の更新は `Settings::load` 側で済んでいるので、閉じる際に永続化は不要
//!   (次回起動では previous == current となり再表示されない)。

use crate::app::App;
use eframe::egui;

impl App {
    pub(crate) fn show_whats_new_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_whats_new || self.whats_new_entries.is_empty() {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let close;
        let open_changelog;
        // &'static 参照の Vec なので clone は安価 (借用衝突回避のためローカルへ)。
        // highlights_to_show は「またぎ累積」のため昇順で返すが、表示は更新履歴と揃えて
        // **新しいバージョンを上**にする (実機フィードバック 2026-06-20)。
        let mut entries = self.whats_new_entries.clone();
        entries.reverse();

        (close, open_changelog) = draw_whats_new_dialog(ctx, &mut open, &entries);

        if open_changelog {
            let url = crate::ui_helpers::manual_url("changelog.html", None);
            crate::ui_helpers::open_url(&url);
        }
        if close || !open || escape_pressed {
            self.show_whats_new = false;
        }
    }
}

pub(super) fn draw_whats_new_dialog(
    ctx: &egui::Context,
    open: &mut bool,
    entries: &[&crate::version_highlights::VersionHighlights],
) -> (bool, bool) {
    let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
    // Preserve the previous 650pt outer reading width independently of content.
    // Window::default_width takes the inner width; use the actual frame margins
    // and let the existing Window owner handle later user/viewport resizing.
    // Round the viewport-limited width inward: a half-point logical viewport at
    // 200% must not gain an outward rounded frame edge.
    let default_width = (650.0_f32.min(ctx.content_rect().width()).floor()
        - egui::Frame::window(&ctx.style()).total_margin().sum().x)
        .max(1.0);
    let mut close = false;
    let mut open_changelog = false;
    egui::Window::new("重要な変更点")
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_pos(dialog_pos)
        .min_width(440.0)
        .default_width(default_width)
        .default_height((ctx.content_rect().height() - 80.0).max(1.0))
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label("mImageViewer が新しくなりました。主な変更点です。");
            ui.add_space(6.0);
            let footer =
                super::startup_dialog_footer_height(ui, &["すべての変更を見る", "閉じる"], 4.0);
            super::startup_dialog_scroll_body(ui, "whats_new_scroll", footer, |ui| {
                crate::version_highlights::render(ui, &entries);
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("すべての変更を見る").clicked() {
                    open_changelog = true;
                }
                if ui.button("閉じる").clicked() {
                    close = true;
                }
            });
        });
    (close, open_changelog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    #[test]
    fn version_highlights_initial_width_keeps_previous_reading_area() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
            for scale in [1.0, 2.0] {
                let mut fonts_ready = false;
                let mut harness = Harness::builder().with_size(size).build(move |ctx| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        crate::settings::apply_ui_scale_factor(ctx, scale);
                        fonts_ready = true;
                        return;
                    }
                    let entries: Vec<_> = crate::version_highlights::table().iter().rev().collect();
                    draw_whats_new_dialog(ctx, &mut true, &entries);
                });
                harness.run_steps(12);
                let viewport = harness.ctx.content_rect();
                let intended_width = 650.0_f32.min(viewport.width()).floor();
                for frame in 0..8 {
                    let window = harness
                        .ctx
                        .memory(|memory| memory.area_rect(egui::Id::new("重要な変更点")))
                        .unwrap();
                    assert!(
                        (window.width() - intended_width).abs() < 0.1,
                        "size={size:?} scale={scale} frame={frame}: intended={intended_width} actual={window:?}"
                    );
                    assert!(viewport.contains_rect(window));
                    harness.run_steps(1);
                }
            }
        }
    }

    #[test]
    fn version_highlights_wrapped_content_does_not_expand_parent_width() {
        for width in [440.0, 507.0, 640.0, 860.0] {
            for scale in [1.0, 2.0] {
                let mut fonts_ready = false;
                let mut harness = Harness::builder().build(move |ctx| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        crate::settings::apply_ui_scale_factor(ctx, scale);
                        fonts_ready = true;
                        return;
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 500.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                let right = ui.max_rect().right();
                                let entries: Vec<_> =
                                    crate::version_highlights::table().iter().rev().collect();
                                crate::version_highlights::render(ui, &entries);
                                assert!(
                                    ui.min_rect().right() <= right,
                                    "width={width} scale={scale}: {:?}",
                                    ui.min_rect()
                                );
                            },
                        );
                    });
                });
                harness.run_steps(3);
            }
        }
    }

    #[test]
    fn version_highlights_window_and_close_button_stay_stable() {
        use crate::version_highlights::{HighlightItem, VersionHighlights};
        const LONG: HighlightItem = HighlightItem {
            title: concat!(
                "長い告知見出しで折り返しとウィンドウ幅を確認します。",
                "長い告知見出しで折り返しとウィンドウ幅を確認します。",
                "長い告知見出しで折り返しとウィンドウ幅を確認します。"
            ),
            body: concat!(
                "長い本文も画面に合わせて折り返し、閉じる操作を移動させません。",
                "長い本文も画面に合わせて折り返し、閉じる操作を移動させません。",
                "長い本文も画面に合わせて折り返し、閉じる操作を移動させません。",
                "長い本文も画面に合わせて折り返し、閉じる操作を移動させません。"
            ),
        };
        const LONG_ENTRY: VersionHighlights = VersionHighlights {
            version: "99.0.0",
            must_read: &[LONG; 4],
            highlights: &[LONG; 4],
        };
        for extended in [false, true] {
            for scale in [1.0, 2.0] {
                let mut entries: Vec<_> = crate::version_highlights::table().iter().rev().collect();
                if extended {
                    entries.insert(0, &LONG_ENTRY);
                }
                let mut fonts_ready = false;
                let mut harness = Harness::builder()
                    .with_size(egui::vec2(1093.0, 614.0))
                    .build(move |ctx| {
                        if !fonts_ready {
                            crate::ui_fonts::configure_fonts(ctx);
                            crate::settings::apply_ui_scale_factor(ctx, scale);
                            fonts_ready = true;
                            return;
                        }
                        draw_whats_new_dialog(ctx, &mut true, &entries);
                    });
                for size in [
                    egui::vec2(1093.0, 614.0),
                    egui::vec2(1366.0, 728.0),
                    egui::vec2(1093.0, 614.0),
                ] {
                    harness.set_size(size / scale);
                    harness.run_steps(12);
                    let window = harness
                        .ctx
                        .memory(|memory| memory.area_rect(egui::Id::new("重要な変更点")))
                        .unwrap();
                    assert!(harness.ctx.content_rect().contains_rect(window));
                    for label in ["Close window", "すべての変更を見る", "閉じる"] {
                        let node = harness.get_by_label(label);
                        let id = unsafe {
                            egui::Id::from_high_entropy_bits(node.accesskit_node().id().0)
                        };
                        let initial = harness.ctx.read_response(id).unwrap().rect;
                        harness.hover_at(initial.center());
                        for frame in 0..8 {
                            harness.run_steps(1);
                            let current = harness.ctx.read_response(id).unwrap();
                            assert_eq!(
                                current.rect, initial,
                                "extended={extended} scale={scale} size={size:?} {label} frame={frame}"
                            );
                            assert_eq!(harness.ctx.memory(|memory| memory.area_rect(egui::Id::new("重要な変更点"))).unwrap(), window);
                            assert!(current.contains_pointer());
                            assert!(current.interact_rect.contains_rect(current.rect));
                        }
                        for pressed in [true, false] {
                            harness.event(egui::Event::PointerButton {
                                pos: initial.center(),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            });
                            harness.run_steps(1);
                        }
                        assert!(harness.ctx.read_response(id).unwrap().clicked());
                    }
                }
            }
        }
    }
}
