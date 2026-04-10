//! `show_thumb_quality_fullscreen_overlay` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_thumb_quality_fullscreen_overlay(ctx)` で呼ばれる。

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
    pub(crate) fn show_thumb_quality_fullscreen_overlay(&mut self, ctx: &egui::Context) {
        // ── サムネイル画質プレビュー全画面 A/B 比較オーバーレイ ────────
        if self.tq.fullscreen {
            let screen = ctx.content_rect();

            // A・B のテクスチャ。両方とも同じソース画像から作られたサムネイルなので
            // アスペクト比は同一。どちらかのサイズで fit 計算する。
            let ref_size = self
                .tq.a_texture
                .as_ref()
                .map(|t| t.size_vec2())
                .or_else(|| self.tq.b_texture.as_ref().map(|t| t.size_vec2()));

            // 画像表示領域を画面中央に計算（下部に情報バー分のスペースを確保）
            let img_rect_opt: Option<egui::Rect> = ref_size.map(|rs| {
                let margin = 40.0;
                let info_bar_h = 80.0;
                let avail_w = (screen.width() - margin * 2.0).max(1.0);
                let avail_h = (screen.height() - margin * 2.0 - info_bar_h).max(1.0);
                let scale = (avail_w / rs.x).min(avail_h / rs.y);
                let img_size = rs * scale;
                egui::Rect::from_center_size(
                    egui::pos2(screen.center().x, screen.center().y - info_bar_h * 0.5),
                    img_size,
                )
            });

            let divider_t = self.tq.fs_divider.clamp(0.0, 1.0);

            let area_resp = egui::Area::new(egui::Id::new("tq_fs_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        screen.size(),
                        egui::Sense::click_and_drag(),
                    );
                    let painter = ui.painter();
                    // 背景
                    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 20));

                    let Some(img_rect) = img_rect_opt else {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "プレビューがありません",
                            egui::FontId::proportional(18.0),
                            egui::Color32::from_gray(200),
                        );
                        return response;
                    };

                    let divider_x = img_rect.min.x + img_rect.width() * divider_t;

                    // A (左側) を divider まで描画
                    if let Some(ta) = &self.tq.a_texture {
                        let a_rect = egui::Rect::from_min_max(
                            img_rect.min,
                            egui::pos2(divider_x, img_rect.max.y),
                        );
                        let a_uv = egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(divider_t, 1.0),
                        );
                        if a_rect.width() > 0.0 {
                            painter.image(ta.id(), a_rect, a_uv, egui::Color32::WHITE);
                        }
                    }

                    // B (右側) を divider から描画
                    if let Some(tb) = &self.tq.b_texture {
                        let b_rect = egui::Rect::from_min_max(
                            egui::pos2(divider_x, img_rect.min.y),
                            img_rect.max,
                        );
                        let b_uv = egui::Rect::from_min_max(
                            egui::pos2(divider_t, 0.0),
                            egui::pos2(1.0, 1.0),
                        );
                        if b_rect.width() > 0.0 {
                            painter.image(tb.id(), b_rect, b_uv, egui::Color32::WHITE);
                        }
                    }

                    // 画像の外枠
                    painter.rect_stroke(
                        img_rect,
                        0.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                        egui::StrokeKind::Outside,
                    );

                    // 縦境界線
                    painter.line_segment(
                        [
                            egui::pos2(divider_x, img_rect.min.y),
                            egui::pos2(divider_x, img_rect.max.y),
                        ],
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                    );

                    // ドラッグハンドル（円 + 左右矢印）
                    let handle_center = egui::pos2(divider_x, img_rect.center().y);
                    let handle_r = 16.0;
                    painter.circle_filled(
                        handle_center,
                        handle_r,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 230),
                    );
                    painter.circle_stroke(
                        handle_center,
                        handle_r,
                        egui::Stroke::new(2.0, egui::Color32::from_gray(60)),
                    );
                    painter.text(
                        handle_center,
                        egui::Align2::CENTER_CENTER,
                        "◀ ▶",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(40),
                    );

                    // A / B ラベル（画像の角に半透明背景付き）
                    let label_pad = egui::vec2(10.0, 6.0);
                    let label_a = "A";
                    let label_b = "B";
                    let font = egui::FontId::proportional(24.0);

                    // A ラベル（左上、divider より左にあるときのみ）
                    if divider_t > 0.05 {
                        let pos = egui::pos2(img_rect.min.x + 12.0, img_rect.min.y + 12.0);
                        let galley = painter.layout_no_wrap(
                            label_a.to_string(),
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        let bg_rect = egui::Rect::from_min_size(
                            pos,
                            galley.size() + label_pad * 2.0,
                        );
                        painter.rect_filled(
                            bg_rect,
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                        );
                        painter.galley(pos + label_pad, galley, egui::Color32::WHITE);
                    }

                    // B ラベル（右上、divider より右にあるときのみ）
                    if divider_t < 0.95 {
                        let galley = painter.layout_no_wrap(
                            label_b.to_string(),
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        let bg_size = galley.size() + label_pad * 2.0;
                        let pos = egui::pos2(
                            img_rect.max.x - 12.0 - bg_size.x,
                            img_rect.min.y + 12.0,
                        );
                        let bg_rect = egui::Rect::from_min_size(pos, bg_size);
                        painter.rect_filled(
                            bg_rect,
                            4.0,
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                        );
                        painter.galley(pos + label_pad, galley, egui::Color32::WHITE);
                    }

                    // 情報バー（画像下）
                    let info_base_y = img_rect.max.y + 24.0;
                    let a_info = format!(
                        "A:  {}px  /  q={}  /  {}",
                        self.tq.a_size,
                        self.tq.a_quality,
                        format_bytes_small(self.tq.a_bytes as u64),
                    );
                    let b_info = format!(
                        "B:  {}px  /  q={}  /  {}",
                        self.tq.b_size,
                        self.tq.b_quality,
                        format_bytes_small(self.tq.b_bytes as u64),
                    );
                    let info_font = egui::FontId::proportional(14.0);
                    painter.text(
                        egui::pos2(rect.center().x - 24.0, info_base_y),
                        egui::Align2::RIGHT_CENTER,
                        a_info,
                        info_font.clone(),
                        egui::Color32::from_rgb(150, 200, 255),
                    );
                    painter.text(
                        egui::pos2(rect.center().x + 24.0, info_base_y),
                        egui::Align2::LEFT_CENTER,
                        b_info,
                        info_font.clone(),
                        egui::Color32::from_rgb(255, 220, 150),
                    );
                    painter.text(
                        egui::pos2(rect.center().x, info_base_y + 24.0),
                        egui::Align2::CENTER_CENTER,
                        "ドラッグで境界線を移動  /  クリック または ESC で戻る",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(180),
                    );

                    response
                });

            // ドラッグ → divider を更新
            if area_resp.inner.dragged() {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    if let Some(img_rect) = img_rect_opt {
                        if img_rect.width() > 0.0 {
                            let t = ((pos.x - img_rect.min.x) / img_rect.width())
                                .clamp(0.0, 1.0);
                            self.tq.fs_divider = t;
                            ctx.request_repaint();
                        }
                    }
                }
            }

            // 画像上にホバーしているときはリサイズ左右カーソル
            if let Some(img_rect) = img_rect_opt {
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    if img_rect.contains(pos) {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                }
            }

            // ドラッグしていないクリック → 閉じる
            let clicked = area_resp.inner.clicked();
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if clicked || esc {
                self.tq.fullscreen = false;
            }
        }

    }
}
