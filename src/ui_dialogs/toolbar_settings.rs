//! `show_toolbar_settings_dialog` ダイアログの実装。
//!
//! ツールバーに表示するセクション・個別項目を選ぶための設定ダイアログ。
//! 「設定 → ツールバー…」メニューから呼ばれる。

#![allow(unused_imports)]

use eframe::egui;

use crate::app::App;
use crate::settings::{SortOrder, ThumbAspect};

impl App {
    pub(crate) fn show_toolbar_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_toolbar_settings {
            return;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        egui::Window::new("ツールバーの表示")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(480.0);

                ui.label(
                    "チェックを外した項目はツールバーから隠れます。\n\
                     セクション内の全項目を外すとセクション自体が非表示になります。",
                );
                ui.add_space(6.0);

                ui.checkbox(&mut self.settings.show_toolbar_favorites, "お気に入り");
                ui.checkbox(&mut self.settings.show_toolbar_folder, "フォルダ (アドレスバー)");
                ui.checkbox(&mut self.settings.show_toolbar_parent_button, "上のフォルダへ (⬆ ボタン)");

                // ── 列 ──
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(2.0);
                ui.label(egui::RichText::new("列").strong());
                ui.horizontal_wrapped(|ui| {
                    for cols in crate::settings::MIN_GRID_COLS..=crate::settings::MAX_GRID_COLS {
                        let mut checked = self.settings.toolbar_cols_items.contains(&cols);
                        if ui.checkbox(&mut checked, format!("{cols}")).changed() {
                            if checked {
                                self.settings.toolbar_cols_items.push(cols);
                                self.settings.toolbar_cols_items.sort();
                            } else {
                                self.settings.toolbar_cols_items.retain(|&c| c != cols);
                            }
                        }
                    }
                });

                // ── 比率 ──
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(2.0);
                ui.label(egui::RichText::new("比率").strong());
                ui.horizontal_wrapped(|ui| {
                    for &aspect in ThumbAspect::all() {
                        let mut checked = self.settings.toolbar_aspect_items.contains(&aspect);
                        if ui.checkbox(&mut checked, aspect.label()).changed() {
                            if checked {
                                self.settings.toolbar_aspect_items.push(aspect);
                                // all() の順序を保つ
                                let order: Vec<_> = ThumbAspect::all().to_vec();
                                self.settings.toolbar_aspect_items.sort_by_key(|a| {
                                    order.iter().position(|o| o == a).unwrap_or(usize::MAX)
                                });
                            } else {
                                self.settings.toolbar_aspect_items.retain(|&a| a != aspect);
                            }
                        }
                    }
                });

                // ── ソート ──
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(2.0);
                ui.label(egui::RichText::new("ソート").strong());
                ui.horizontal_wrapped(|ui| {
                    for &order in SortOrder::all() {
                        let mut checked = self.settings.toolbar_sort_items.contains(&order);
                        if ui.checkbox(&mut checked, order.short_label()).changed() {
                            if checked {
                                self.settings.toolbar_sort_items.push(order);
                                let canonical: Vec<_> = SortOrder::all().to_vec();
                                self.settings.toolbar_sort_items.sort_by_key(|s| {
                                    canonical.iter().position(|o| o == s).unwrap_or(usize::MAX)
                                });
                            } else {
                                self.settings.toolbar_sort_items.retain(|&s| s != order);
                            }
                        }
                    }
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            self.settings.save();
            self.show_toolbar_settings = false;
        } else if cancel || !open {
            self.settings = crate::settings::Settings::load();
            self.show_toolbar_settings = false;
        }
    }
}
