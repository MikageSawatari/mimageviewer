//! `show_cache_creator_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.cc.show_dialog(ctx)` で呼ばれる。

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
        if self.cc.show {
            // 完了初回に結果メッセージをセット
            if self.cc.finished.load(Ordering::Relaxed)
                && self.cc.result.is_none()
            {
                let done = self.cc.done.load(Ordering::Relaxed);
                let total = self.cc.total.load(Ordering::Relaxed);
                let cancelled = self.cc.cancel.load(Ordering::Relaxed);
                self.cc.result = Some(if cancelled {
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

                    if !self.cc.running
                        && !self.cc.finished.load(Ordering::Relaxed)
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
                                ui.checkbox(&mut self.cc.checked[i], label);
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);

                        ui.checkbox(
                            &mut self.settings.batch_cache_zip_contents,
                            "ZIP ファイルの中身もキャッシュ",
                        );
                        ui.checkbox(
                            &mut self.settings.batch_cache_pdf_contents,
                            "PDF ファイルの中身もキャッシュ",
                        );
                        ui.label(
                            egui::RichText::new(
                                "チェックなしでも先頭1枚/1ページはキャッシュされます"
                            )
                            .weak()
                            .small(),
                        );

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let any_checked = self.cc.checked.iter().any(|&b| b);
                        if ui
                            .add_enabled(
                                any_checked,
                                egui::Button::new("  キャッシュ作成  "),
                            )
                            .clicked()
                        {
                            self.settings.save();
                            self.start_cache_creation();
                        }
                    } else {
                        // ── 実行中 / 完了画面 ──
                        let counting = self.cc.counting.load(Ordering::Relaxed);
                        let total = self.cc.total.load(Ordering::Relaxed);
                        let done = self.cc.done.load(Ordering::Relaxed);
                        let size = self.cc.cache_size.load(Ordering::Relaxed);

                        if counting {
                            ui.label("フォルダを列挙中…");
                        } else {
                            ui.label(format!("フォルダ: {} / {}", done, total));
                        }

                        let current = self.cc.current.lock().unwrap().clone();
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

                        if self.cc.finished.load(Ordering::Relaxed) {
                            if let Some(ref msg) = self.cc.result {
                                ui.label(msg.as_str());
                                ui.add_space(4.0);
                            }
                            if ui.button("  閉じる  ").clicked() {
                                self.cc.show = false;
                                self.cc.running = false;
                            }
                        } else {
                            if ui.button("  キャンセル  ").clicked() {
                                self.cc.cancel.store(true, Ordering::Relaxed);
                            }
                            // リアルタイム更新のため繰り返し描画要求
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                    }
                });

            if !open {
                if self.cc.running
                    && !self.cc.finished.load(Ordering::Relaxed)
                {
                    self.cc.cancel.store(true, Ordering::Relaxed);
                }
                self.cc.show = false;
                self.cc.running = false;
            }
        }

    }
}
