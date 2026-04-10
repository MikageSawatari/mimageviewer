//! `show_thumb_quality_dialog_window` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_thumb_quality_dialog_window(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};

use eframe::egui;

use crate::app::{tq_draw_preview, App};
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
    pub(crate) fn show_thumb_quality_dialog_window(&mut self, ctx: &egui::Context) {
        // ── サムネイル画質設定ポップアップ ────────────────────────────
        if self.show_thumb_quality_dialog {
            let mut open = true;
            let mut apply_a = false;
            let mut apply_b = false;
            let mut reencode_a = false;
            let mut reencode_b = false;
            let mut open_fs_a = false;
            let mut open_fs_b = false;

            // 実グリッドの現在のセルサイズを取得（最小値を確保してスライダーが入るように）
            let grid_cell_w = self.last_cell_size.max(200.0);
            let grid_cell_h = self.last_cell_h.max(150.0);
            // ダイアログのデフォルトサイズ（2カラム + パディング）
            let default_w = (grid_cell_w * 2.0 + 80.0).clamp(680.0, 1800.0);
            let default_h = (grid_cell_h + 260.0).clamp(480.0, 1200.0);
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("サムネイル画質設定")
                .open(&mut open)
                .resizable(true)
                .collapsible(false)
                .default_pos(dialog_pos)
                .default_size([default_w, default_h])
                .show(ctx, |ui| {
                    if self.tq_sample.is_none() {
                        ui.set_min_width(360.0);
                        ui.label("画像を1枚選択してからもう一度お試しください。");
                        ui.add_space(8.0);
                        if ui.button("  閉じる  ").clicked() {
                            self.show_thumb_quality_dialog = false;
                        }
                        return;
                    }

                    // サンプル画像情報
                    if let Some(ref p) = self.tq_sample_path {
                        ui.label(
                            egui::RichText::new(format!("サンプル: {}", p.to_string_lossy()))
                                .small(),
                        );
                    }
                    if let Some(ref img) = self.tq_sample {
                        let sz = self.tq_sample_original_size;
                        let sz_str = if sz >= 1024 * 1024 {
                            format!("{:.1} MB", sz as f64 / (1024.0 * 1024.0))
                        } else {
                            format!("{:.0} KB", sz as f64 / 1024.0)
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "（元サイズ {}x{} / {}）",
                                img.width(),
                                img.height(),
                                sz_str
                            ))
                            .weak()
                            .small(),
                        );
                    }

                    // 現在のグリッド表示サイズ（サイズ選択時の参考用）
                    ui.label(
                        egui::RichText::new(format!(
                            "現在のグリッド表示サイズ: {} × {} px  （{} 列 / アスペクト比 {}）",
                            self.last_cell_size.round() as i32,
                            self.last_cell_h.round() as i32,
                            self.settings.grid_cols,
                            self.settings.thumb_aspect.label(),
                        ))
                        .small(),
                    );

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── A / B 2 カラム ────────────────────────
                    ui.columns(2, |cols| {
                        // -- A --
                        cols[0].vertical(|ui| {
                            ui.heading("A");
                            ui.add_space(4.0);
                            let resp = tq_draw_preview(
                                ui,
                                &self.tq_a_texture,
                                grid_cell_w,
                                grid_cell_h,
                            );
                            if resp.clicked() {
                                open_fs_a = true;
                            }
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label("サイズ:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_a_size, 128..=1536)
                                        .text("px"),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_a = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("品質:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_a_quality, 1..=100),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_a = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(format!("{}  ({}x{})",
                                format_bytes_small(self.tq_a_bytes as u64),
                                self.tq_a_texture.as_ref().map(|t| t.size()[0]).unwrap_or(0),
                                self.tq_a_texture.as_ref().map(|t| t.size()[1]).unwrap_or(0),
                            ));
                            ui.add_space(4.0);
                            if ui.button("  A を適用  ").clicked() {
                                apply_a = true;
                            }
                        });

                        // -- B --
                        cols[1].vertical(|ui| {
                            ui.heading("B");
                            ui.add_space(4.0);
                            let resp = tq_draw_preview(
                                ui,
                                &self.tq_b_texture,
                                grid_cell_w,
                                grid_cell_h,
                            );
                            if resp.clicked() {
                                open_fs_b = true;
                            }
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label("サイズ:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_b_size, 128..=1536)
                                        .text("px"),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_b = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("品質:");
                                let resp = ui.add(
                                    egui::Slider::new(&mut self.tq_b_quality, 1..=100),
                                );
                                if resp.drag_stopped() || resp.lost_focus() {
                                    reencode_b = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(format!("{}  ({}x{})",
                                format_bytes_small(self.tq_b_bytes as u64),
                                self.tq_b_texture.as_ref().map(|t| t.size()[0]).unwrap_or(0),
                                self.tq_b_texture.as_ref().map(|t| t.size()[1]).unwrap_or(0),
                            ));
                            ui.add_space(4.0);
                            if ui.button("  B を適用  ").clicked() {
                                apply_b = true;
                            }
                        });
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "現在の設定: {}px / q={}",
                            self.settings.thumb_px, self.settings.thumb_quality
                        ));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("  閉じる  ").clicked() {
                                    self.show_thumb_quality_dialog = false;
                                }
                            },
                        );
                    });
                });

            if reencode_a {
                self.reencode_tq_panel(ctx, true);
            }
            if reencode_b {
                self.reencode_tq_panel(ctx, false);
            }
            if open_fs_a || open_fs_b {
                self.tq_fullscreen = true;
                // divider 位置はリセットせず、前回の位置を維持する
            }
            if apply_a {
                self.settings.thumb_px = self.tq_a_size;
                self.settings.thumb_quality = self.tq_a_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if apply_b {
                self.settings.thumb_px = self.tq_b_size;
                self.settings.thumb_quality = self.tq_b_quality;
                self.settings.save();
                self.close_thumb_quality_dialog();
            } else if !open {
                self.close_thumb_quality_dialog();
            }
        }

    }
}
