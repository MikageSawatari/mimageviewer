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
            let mut close_clicked = false;
            let escape_pressed = self.dialog_escape_pressed(ctx);
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            // スナップショットを取得 (ロック時間を最小化)
            let snapshot: crate::stats::ThumbStats = {
                self.stats.lock().map(|s| s.clone()).unwrap_or_default()
            };

            // ダイアログ高さは画面に収める。ボタン行を常時表示するため内部を ScrollArea で包む。
            // 下限は小さく: 画面高が極端に低いとき、下限が大きいと逆にダイアログ全体が
            // ビューポートを越えて下部ボタンが見切れる。
            let scroll_max_h = (ctx.content_rect().height() - 160.0).min(720.0).max(80.0);
            egui::Window::new("統計")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(520.0);
                    egui::ScrollArea::vertical()
                        .id_salt("stats_scroll")
                        .max_height(scroll_max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
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
                        None,
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
                        Some(&snapshot.size_time_hist),
                    );

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── フォーマット別件数 ──
                    ui.heading("フォーマット");
                    ui.add_space(4.0);
                    let format_rows: [(&str, u64, f64); 7] = [
                        ("JPEG  ", snapshot.count_jpg,   snapshot.time_jpg),
                        ("PNG   ", snapshot.count_png,   snapshot.time_png),
                        ("WebP  ", snapshot.count_webp,  snapshot.time_webp),
                        ("GIF   ", snapshot.count_gif,   snapshot.time_gif),
                        ("BMP   ", snapshot.count_bmp,   snapshot.time_bmp),
                        ("動画  ", snapshot.count_video, 0.0),
                        ("その他", snapshot.count_other, snapshot.time_other),
                    ];
                    draw_format_rows(ui, &format_rows);

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── デコーダ経路別件数 ──
                    // フォーマット集計とは独立した軸。同じ画像が両方に 1 件ずつ加算される。
                    ui.heading("デコーダ経路");
                    ui.add_space(4.0);
                    let total_imgs = snapshot.total_images();
                    let count_native = total_imgs.saturating_sub(snapshot.count_wic + snapshot.count_susie);
                    let time_native = (snapshot.time_jpg
                        + snapshot.time_png
                        + snapshot.time_webp
                        + snapshot.time_gif
                        + snapshot.time_bmp
                        + snapshot.time_other
                        - snapshot.time_wic
                        - snapshot.time_susie)
                        .max(0.0);
                    let decoder_rows: [(&str, u64, f64); 3] = [
                        ("Native", count_native,           time_native),
                        ("WIC   ", snapshot.count_wic,    snapshot.time_wic),
                        ("Susie ", snapshot.count_susie,  snapshot.time_susie),
                    ];
                    draw_format_rows(ui, &decoder_rows);

                    // Susie 拡張子別内訳 (Susie 経由が 1 件以上ある場合のみ)
                    if snapshot.count_susie > 0 {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("  Susie プラグイン (拡張子別)")
                                .strong(),
                        );
                        ui.add_space(2.0);
                        // BTreeMap なので拡張子順で並ぶ。表示時は (".mag", N, ms) 形式に整形。
                        let labels: Vec<String> = snapshot
                            .susie_by_ext
                            .keys()
                            .map(|e| format!(".{e:<5}"))
                            .collect();
                        let susie_rows: Vec<(&str, u64, f64)> = snapshot
                            .susie_by_ext
                            .values()
                            .zip(labels.iter())
                            .map(|((c, t), label)| (label.as_str(), *c, *t))
                            .collect();
                        draw_format_rows(ui, &susie_rows);
                    }

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
                        });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("リセット").clicked() {
                            reset_clicked = true;
                        }
                        if ui.button("閉じる").clicked() {
                            close_clicked = true;
                        }
                    });
                });

            if reset_clicked {
                if let Ok(mut s) = self.stats.lock() {
                    s.reset();
                }
            }
            if !open || close_clicked || escape_pressed {
                self.show_stats_dialog = false;
            }
        }

    }
}
