//! `show_preferences_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_preferences_dialog(ctx)` で呼ばれる。

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
    pub(crate) fn show_preferences_dialog(&mut self, ctx: &egui::Context) {
        // ── 環境設定ポップアップ ─────────────────────────────────────
        if self.show_preferences {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("環境設定")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);

                    ui.heading("並列読み込み");
                    ui.add_space(4.0);

                    let is_auto = self.settings.parallelism == crate::settings::Parallelism::Auto;
                    let auto_count = {
                        let cores = std::thread::available_parallelism()
                            .map(|n| n.get()).unwrap_or(2);
                        (cores / 2).max(1)
                    };

                    if ui.radio(is_auto, format!("自動（CPUコア数の半分: {} スレッド）", auto_count)).clicked() {
                        self.settings.parallelism = crate::settings::Parallelism::Auto;
                    }

                    ui.horizontal(|ui| {
                        if ui.radio(!is_auto, "手動").clicked() {
                            self.settings.parallelism =
                                crate::settings::Parallelism::Manual(self.pref_manual_threads);
                        }
                        ui.add_enabled(
                            !is_auto,
                            egui::DragValue::new(&mut self.pref_manual_threads)
                                .range(1..=64)
                                .suffix(" スレッド"),
                        );
                        if !is_auto {
                            self.settings.parallelism =
                                crate::settings::Parallelism::Manual(self.pref_manual_threads);
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("フルサイズ画像の先読み");
                    ui.add_space(4.0);
                    ui.label("フルサイズ表示時に前後の画像を先読みする枚数（各最大 50 枚）。");
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("後方（前の画像）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.prefetch_back)
                                .range(0..=50usize)
                                .suffix(" 枚"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("前方（次の画像）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.prefetch_forward)
                                .range(0..=50usize)
                                .suffix(" 枚"),
                        );
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("サムネイルの先読み");
                    ui.add_space(4.0);
                    ui.label(
                        "サムネイルグリッドで現在位置の前後に何ページ分を GPU に保持するか。\n\
                         範囲外はメモリから破棄され、スクロールで戻ると再読み込みされます。",
                    );
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("後方（前のページ）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.thumb_prev_pages)
                                .range(0..=20u32)
                                .suffix(" ページ"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("前方（次のページ）:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.thumb_next_pages)
                                .range(0..=20u32)
                                .suffix(" ページ"),
                        );
                    });

                    ui.add_space(6.0);
                    // プライマリ GPU の VRAM を問い合わせて表示に使う
                    let vram_mib = crate::gpu_info::query_vram_summary_mib();
                    let vram_label = match vram_mib {
                        Some(mib) if mib >= 1024 => {
                            format!("{:.1} GiB", mib as f64 / 1024.0)
                        }
                        Some(mib) => format!("{} MiB", mib),
                        None => "取得失敗 (4 GiB 仮定)".to_string(),
                    };
                    ui.label(format!(
                        "GPU メモリ上限 (安全ネット):\n\
                         超過時は先読み範囲を自動的に縮小します。\n\
                         検出した GPU の VRAM: {vram_label}",
                    ));

                    ui.horizontal(|ui| {
                        ui.label("上限:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.thumb_vram_cap_percent,
                                0..=100u32,
                            )
                            .step_by(5.0)
                            .suffix(" %"),
                        );
                    });

                    // 現在の % が実際に何 MiB に相当するかを補助表示
                    {
                        let pct = self.settings.thumb_vram_cap_percent;
                        let text = if pct == 0 {
                            "  ↑ 0% = 無制限 (推奨しない)".to_string()
                        } else {
                            let cap_mib = crate::gpu_info::vram_cap_from_percent(pct)
                                / (1024 * 1024);
                            format!(
                                "  ↑ VRAM の {}% = 約 {} MiB を上限とします (推奨: 50%)",
                                pct, cap_mib
                            )
                        };
                        ui.label(text);
                    }

                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.settings.thumb_idle_upgrade,
                        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
                    );
                    ui.label(
                        "  ↑ スクロール停止後、キャッシュ復元 (WebP q=75) のサムネイルを\n    \
                         元画像から再デコードして差し替えます。visible 側から順次処理。",
                    );

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("フォルダ移動");
                    ui.add_space(4.0);
                    ui.label("Ctrl+↑↓ で移動先フォルダに画像がない場合、自動でスキップする最大回数。");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("空フォルダのスキップ上限:");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.folder_skip_limit)
                                .range(1..=10usize)
                                .suffix(" 回"),
                        );
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    ui.heading("ツールバーの表示");
                    ui.add_space(4.0);
                    ui.label(
                        "ウィンドウ上部に表示するセクションを選びます。\n\
                         すべて非表示にすると、ツールバー自体が消えてグリッドが広く使えます。\n\
                         フォルダ (アドレスバー) を非表示にした場合は、\n\
                         「ファイル → フォルダを開く…」で別のフォルダへ移動できます。",
                    );
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.settings.show_toolbar_cols, "列");
                    ui.checkbox(&mut self.settings.show_toolbar_aspect, "比率");
                    ui.checkbox(&mut self.settings.show_toolbar_sort, "ソート");
                    ui.checkbox(&mut self.settings.show_toolbar_favorites, "お気に入り");
                    ui.checkbox(&mut self.settings.show_toolbar_folder, "フォルダ (アドレスバー)");

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
                self.show_preferences = false;
            } else if cancel || !open {
                // キャンセル/×ボタン: 変更を破棄するため再ロード
                self.settings = crate::settings::Settings::load();
                self.show_preferences = false;
            }
        }

    }
}
