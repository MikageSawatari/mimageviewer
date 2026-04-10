//! `show_stats_dialog_window` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_stats_dialog_window(ctx)` で呼ばれる。

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
    pub(crate) fn show_stats_dialog_window(&mut self, ctx: &egui::Context) {
        // ── 統計ダイアログ ──────────────────────────────────────────
        if self.show_stats_dialog {
            let mut open = true;
            let mut reset_clicked = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            // スナップショットを取得 (ロック時間を最小化)
            let snapshot: crate::stats::ThumbStats = {
                self.stats.lock().map(|s| s.clone()).unwrap_or_default()
            };

            egui::Window::new("統計")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(520.0);
                    ui.label(
                        "起動時から累計したサムネイル読み込み統計です。\n\
                         キャッシュ生成設定の参考にしてください。\n\
                         (キャッシュヒットは対象外。フルデコード時のみ記録)",
                    );
                    ui.add_space(8.0);

                    // ── 読み込み時間ヒストグラム ──
                    ui.heading("読み込み時間 (decode + display)");
                    ui.add_space(4.0);
                    draw_histogram(
                        ui,
                        &snapshot.load_time_hist,
                        |bucket| crate::stats::ThumbStats::load_time_label(bucket),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── ファイルサイズヒストグラム ──
                    ui.heading("ファイルサイズ");
                    ui.add_space(4.0);
                    draw_histogram(
                        ui,
                        &snapshot.size_hist,
                        |bucket| crate::stats::ThumbStats::size_label(bucket),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── フォーマット別件数 ──
                    ui.heading("フォーマット");
                    ui.add_space(4.0);
                    let format_rows: [(&str, u64); 7] = [
                        ("JPEG  ", snapshot.count_jpg),
                        ("PNG   ", snapshot.count_png),
                        ("WebP  ", snapshot.count_webp),
                        ("GIF   ", snapshot.count_gif),
                        ("BMP   ", snapshot.count_bmp),
                        ("動画  ", snapshot.count_video),
                        ("その他", snapshot.count_other),
                    ];
                    draw_format_rows(ui, &format_rows);

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── サマリ ──
                    let total_images = snapshot.total_images();
                    let total_all = total_images + snapshot.count_video;
                    ui.label(format!(
                        "合計: {} 件  (画像 {} / 動画 {} / 失敗 {})",
                        format_count(total_all),
                        format_count(total_images),
                        format_count(snapshot.count_video),
                        format_count(snapshot.count_failed),
                    ));

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("リセット").clicked() {
                            reset_clicked = true;
                        }
                        if ui.button("閉じる").clicked() {
                            // open = false でダイアログを閉じる
                        }
                    });
                });

            if reset_clicked {
                if let Ok(mut s) = self.stats.lock() {
                    s.reset();
                }
            }
            if !open {
                self.show_stats_dialog = false;
            }
        }

    }
}
