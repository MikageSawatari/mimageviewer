//! `show_cache_creator_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_cache_creator_dialog(ctx)` で呼ばれる。

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
    pub(crate) fn show_cache_creator_dialog(&mut self, ctx: &egui::Context) {
        // ── キャッシュ作成ポップアップ ────────────────────────────────
        if self.show_cache_creator {
            // 完了初回に結果メッセージをセット
            if self.cache_creator_finished.load(Ordering::Relaxed)
                && self.cache_creator_result.is_none()
            {
                let done = self.cache_creator_done.load(Ordering::Relaxed);
                let total = self.cache_creator_total.load(Ordering::Relaxed);
                let cancelled = self.cache_creator_cancel.load(Ordering::Relaxed);
                self.cache_creator_result = Some(if cancelled {
                    format!("キャンセルされました（{} / {} フォルダ処理済み）", done, total)
                } else {
                    format!("{} フォルダの処理が完了しました。", done)
                });
            }

            let mut open = true;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
            egui::Window::new("キャッシュ作成")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(500.0);

                    if !self.cache_creator_running
                        && !self.cache_creator_finished.load(Ordering::Relaxed)
                    {
                        // ── 選択前画面 ──
                        ui.label("キャッシュを作成するお気に入りを選んでください：");
                        ui.add_space(6.0);

                        if self.settings.favorites.is_empty() {
                            ui.label(egui::RichText::new("（お気に入りが未登録です）").weak());
                        } else {
                            for (i, fav) in self.settings.favorites.iter().enumerate() {
                                // 「表示名 (パス)」の形式でチェックボックスのラベルを作る
                                let label =
                                    format!("{}  ({})", fav.name, fav.path.display());
                                ui.checkbox(&mut self.cache_creator_checked[i], label);
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let any_checked = self.cache_creator_checked.iter().any(|&b| b);
                        if ui
                            .add_enabled(
                                any_checked,
                                egui::Button::new("  キャッシュ作成  "),
                            )
                            .clicked()
                        {
                            self.start_cache_creation();
                        }
                    } else {
                        // ── 実行中 / 完了画面 ──
                        let counting = self.cache_creator_counting.load(Ordering::Relaxed);
                        let total = self.cache_creator_total.load(Ordering::Relaxed);
                        let done = self.cache_creator_done.load(Ordering::Relaxed);
                        let size = self.cache_creator_cache_size.load(Ordering::Relaxed);

                        if counting {
                            ui.label("フォルダを列挙中…");
                        } else {
                            ui.label(format!("フォルダ: {} / {}", done, total));
                        }

                        let current = self.cache_creator_current.lock().unwrap().clone();
                        if !current.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("現在: {}", current))
                                    .weak()
                                    .small(),
                            );
                        }

                        ui.add_space(4.0);
                        ui.label(format!("キャッシュ容量: {}", format_bytes(size)));

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        if self.cache_creator_finished.load(Ordering::Relaxed) {
                            if let Some(ref msg) = self.cache_creator_result {
                                ui.label(msg.as_str());
                                ui.add_space(4.0);
                            }
                            if ui.button("  閉じる  ").clicked() {
                                self.show_cache_creator = false;
                                self.cache_creator_running = false;
                            }
                        } else {
                            if ui.button("  キャンセル  ").clicked() {
                                self.cache_creator_cancel.store(true, Ordering::Relaxed);
                            }
                            // リアルタイム更新のため繰り返し描画要求
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                    }
                });

            if !open {
                if self.cache_creator_running
                    && !self.cache_creator_finished.load(Ordering::Relaxed)
                {
                    self.cache_creator_cancel.store(true, Ordering::Relaxed);
                }
                self.show_cache_creator = false;
                self.cache_creator_running = false;
            }
        }

    }
}
