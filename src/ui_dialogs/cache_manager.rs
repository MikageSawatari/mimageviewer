//! `show_cache_manager_dialog` ダイアログの実装 (サムネイルキャッシュ専用)。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_cache_manager_dialog(ctx)` で呼ばれる。
//!
//! 変換済みアーカイブキャッシュの管理は [`archive_cache_manager`] ダイアログに分離している。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc,
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
    CacheDecision, LoadRequest, ThumbMsg, build_and_save_one, compute_display_px,
};
use crate::ui_helpers::{
    draw_format_rows, draw_histogram, format_bytes, format_bytes_small, format_count,
    natural_sort_key, truncate_name,
};

impl App {
    pub(crate) fn show_cache_manager_dialog(&mut self, ctx: &egui::Context) {
        // ── キャッシュ管理ポップアップ ───────────────────────────────
        if self.show_cache_manager {
            let mut open = true;
            let escape_pressed = self.dialog_escape_pressed(ctx);
            let cache_dir = crate::catalog::default_cache_dir();
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            // 進行中は処理中ラベルを出し、ボタンを disable する。
            let busy = self.cache_maint_pending.is_some();

            egui::Window::new("サムネイルキャッシュ管理")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(380.0);
                    if busy {
                        // 進行中は worker 完了を待ちたいので再描画要求する。
                        ctx.request_repaint();
                    }

                    // ── 統計表示 ──────────────────────────────────
                    if let Some((folders, bytes)) = self.cache_manager_stats {
                        let size_str = if bytes >= 1024 * 1024 * 1024 {
                            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                        } else {
                            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
                        };
                        ui.label(format!("キャッシュ: {folders} フォルダ / {size_str}"));
                    } else {
                        ui.label("キャッシュ情報を取得中...");
                    }
                    // 動画タイル モード サムネ DB は別ファイル管理だが、
                    // 削除操作 (フォルダ単位 / すべて) は静止画キャッシュと一括で
                    // 走るので、サイズもこのダイアログに表示する。
                    if let Some(tile_bytes) = self.cache_manager_tile_bytes {
                        let tile_str = if tile_bytes >= 1024 * 1024 * 1024 {
                            format!("{:.2} GB", tile_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                        } else if tile_bytes >= 1024 * 1024 {
                            format!("{:.1} MB", tile_bytes as f64 / (1024.0 * 1024.0))
                        } else if tile_bytes > 0 {
                            format!("{:.0} KB", tile_bytes as f64 / 1024.0)
                        } else {
                            "0 MB".to_string()
                        };
                        ui.label(format!("うち動画タイル サムネ: {tile_str}"));
                    }
                    if let Some(entries) = self.cache_manager_auto_aspect_entries {
                        ui.label(format!("比率自動判定キャッシュ: {entries} フォルダ"));
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── 古いキャッシュの削除 ──────────────────────
                    ui.horizontal(|ui| {
                        let mut days_str = self.cache_manager_days.to_string();
                        ui.label("最終更新から");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut days_str)
                                .desired_width(48.0)
                                .horizontal_align(egui::Align::Center),
                        );
                        if resp.changed() {
                            if let Ok(v) = days_str.parse::<u32>() {
                                if v > 0 {
                                    self.cache_manager_days = v;
                                }
                            }
                        }
                        ui.label("日以上更新がないキャッシュを削除する");
                    });
                    ui.add_space(4.0);
                    let old_btn = egui::Button::new(format!(
                        "  {} 日以上古いキャッシュを削除  ",
                        self.cache_manager_days
                    ));
                    if ui.add_enabled(!busy, old_btn).clicked() {
                        // 開きっぱなしの SQLite Connection が握っている .db ファイルは
                        // remove_file で消せず silent fail するので、削除前に LRU を畳む
                        // (Codex P3)。
                        self.evict_all_catalog_cache();
                        self.cache_maint_pending = Some(crate::cache_maintenance::spawn(
                            crate::cache_maintenance::CacheMaintTask::DeleteOld {
                                days: self.cache_manager_days as u64,
                            },
                            cache_dir.clone(),
                            self.video_tile_cache.clone(),
                        ));
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── 現在のフォルダのキャッシュを削除 ─────────
                    let has_folder = self.current_folder.is_some();
                    let folder_btn = egui::Button::new("  現在のフォルダのキャッシュを削除  ");
                    if ui.add_enabled(has_folder && !busy, folder_btn).clicked() {
                        if let Some(folder) = self.current_folder.clone() {
                            let auto_aspect_folder = self.auto_aspect_cache_target_path();
                            // 削除前に Connection を畳む (Codex P3): 同上。
                            self.evict_all_catalog_cache();
                            self.cache_maint_pending = Some(crate::cache_maintenance::spawn(
                                crate::cache_maintenance::CacheMaintTask::DeleteFolder {
                                    folder,
                                    auto_aspect_folder,
                                },
                                cache_dir.clone(),
                                self.video_tile_cache.clone(),
                            ));
                        }
                    }
                    if !has_folder {
                        ui.label(
                            egui::RichText::new("(フォルダを開いていないため無効)")
                                .small()
                                .weak(),
                        );
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ── すべて削除 ────────────────────────────────
                    let all_btn = egui::Button::new("  すべてのキャッシュを削除する  ");
                    if ui.add_enabled(!busy, all_btn).clicked() {
                        self.cache_manager_confirm_delete_all = true;
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label("画像の評価・補正・タグなどの孤児データを確認します。");
                    if ui
                        .add_enabled(!busy, egui::Button::new("  メタデータを整理…  "))
                        .clicked()
                    {
                        self.metadata_cleanup_scan = None;
                        self.metadata_cleanup_result = None;
                        self.show_metadata_cleanup = true;
                        self.show_cache_manager = false;
                    }

                    // ── 処理中ラベル / 結果メッセージ ─────────────
                    if busy {
                        ui.add_space(8.0);
                        ui.label("処理中…");
                    } else if let Some(ref msg) = self.cache_manager_result {
                        ui.add_space(8.0);
                        ui.label(msg.as_str());
                    }
                });

            if !open || (escape_pressed && !self.cache_manager_confirm_delete_all) {
                self.show_cache_manager = false;
                self.cache_manager_confirm_delete_all = false;
            }
        }

        // ── 「すべて削除」確認ダイアログ（別ウィンドウ）────────────
        if self.cache_manager_confirm_delete_all {
            let mut confirm_open = true;
            let escape_pressed = self.dialog_escape_pressed(ctx);
            egui::Window::new("キャッシュの全削除")
                .open(&mut confirm_open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("すべてのサムネイルキャッシュを削除します。");
                    ui.label("(編集結果のプレビューキャッシュも一緒に削除されます)");
                    ui.label("(動画タイル モードのサムネ キャッシュも一緒に削除されます)");
                    ui.label("(比率自動判定キャッシュも一緒に削除されます)");
                    ui.label("この操作は元に戻せません。");
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let busy = self.cache_maint_pending.is_some();
                        let del_btn = egui::Button::new("  削除する  ");
                        if ui.add_enabled(!busy, del_btn).clicked() {
                            let cache_dir = crate::catalog::default_cache_dir();
                            if let Some(service) = &self.edit_preview_cache {
                                service.clear();
                            }
                            // 削除前に Connection を畳む (Codex P3): 同上。
                            self.evict_all_catalog_cache();
                            self.cache_maint_pending = Some(crate::cache_maintenance::spawn(
                                crate::cache_maintenance::CacheMaintTask::DeleteAll,
                                cache_dir,
                                self.video_tile_cache.clone(),
                            ));
                            self.cache_manager_confirm_delete_all = false;
                        }
                        if ui.button("  キャンセル  ").clicked() || escape_pressed {
                            self.cache_manager_confirm_delete_all = false;
                        }
                    });
                });
            if !confirm_open {
                self.cache_manager_confirm_delete_all = false;
            }
        }
    }
}
