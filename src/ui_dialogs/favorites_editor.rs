//! `show_favorites_editor_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_favorites_editor_dialog(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

use crate::app::App;
use crate::catalog;
use crate::folder_tree;
use crate::gpu_info;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings;
use crate::stats;
use crate::thumb_loader::{
    build_and_save_one, compute_display_px, CacheDecision, LoadRequest, ThumbMsg,
};
use crate::ui_helpers::{
    draw_format_rows, draw_histogram, format_bytes, format_bytes_small, format_count,
    natural_sort_key, truncate_name,
};

impl App {
    pub(crate) fn show_favorites_editor_dialog(&mut self, ctx: &egui::Context) {
        // ── お気に入り編集ポップアップ ───────────────────────────────
        if self.show_favorites_editor {
            let mut open = true;
            let mut swap: Option<(usize, usize)> = None;
            let mut remove: Option<usize> = None;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("お気に入りの編集")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    if self.settings.favorites.is_empty() {
                        ui.label("お気に入りはまだ登録されていません。");
                    } else {
                        let n = self.settings.favorites.len();
                        egui::Grid::new("fav_edit_grid")
                            .striped(true)
                            .num_columns(2)
                            .show(ui, |ui| {
                                for i in 0..n {
                                    let path_str = self.settings.favorites[i].to_string_lossy().to_string();
                                    ui.label(&path_str);
                                    ui.horizontal(|ui| {
                                        let up_en = i > 0;
                                        let dn_en = i + 1 < n;
                                        if ui.add_enabled(up_en, egui::Button::new("↑")).clicked() {
                                            swap = Some((i - 1, i));
                                        }
                                        if ui.add_enabled(dn_en, egui::Button::new("↓")).clicked() {
                                            swap = Some((i, i + 1));
                                        }
                                        if ui.button("削除").clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    }
                });

            if let Some((a, b)) = swap {
                self.settings.favorites.swap(a, b);
                self.settings.save();
            }
            if let Some(i) = remove {
                self.settings.favorites.remove(i);
                self.settings.save();
            }
            if !open {
                self.show_favorites_editor = false;
            }
        }

    }
}
