//! `show_cache_policy_dialog` ダイアログの実装。
//!
//! `App` への impl 拡張として書かれており、フィールドアクセスは
//! `pub(crate)` 経由で行われる。`update()` から `self.show_cache_policy_dialog(ctx)` で呼ばれる。

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
    pub(crate) fn show_cache_policy_dialog(&mut self, ctx: &egui::Context) {
        // ── キャッシュ生成設定ポップアップ (段階 C) ─────────────────────
        if self.show_cache_policy_dialog {
            let mut open = true;
            let mut apply = false;
            let mut cancel = false;
            let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

            egui::Window::new("キャッシュ生成設定")
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_pos(dialog_pos)
                .show(ctx, |ui| {
                    ui.set_min_width(480.0);

                    ui.label(
                        "サムネイルキャッシュをいつ生成するかを指定します。\n\
                         リリースビルドではキャッシュが無くても十分高速ですが、\n\
                         重い画像や巨大ファイルはキャッシュすると再訪問時に高速化します。\n\
                         Off にしても既存のキャッシュは引き続き読み込まれます。",
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    ui.heading("モード");
                    ui.add_space(4.0);
                    for policy in [
                        crate::settings::CachePolicy::Off,
                        crate::settings::CachePolicy::Auto,
                        crate::settings::CachePolicy::Always,
                    ] {
                        if ui
                            .radio(
                                self.settings.cache_policy == policy,
                                policy.label(),
                            )
                            .clicked()
                        {
                            self.settings.cache_policy = policy;
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Auto モード時のみ以下の項目を活性化
                    let auto_active =
                        self.settings.cache_policy == crate::settings::CachePolicy::Auto;

                    ui.add_enabled_ui(auto_active, |ui| {
                        ui.heading("Auto モードのしきい値");
                        ui.add_space(4.0);

                        ui.label("時間しきい値 (decode + display の合計がこれ以上ならキャッシュ):");
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.cache_threshold_ms,
                                10..=100,
                            )
                            .step_by(5.0)
                            .suffix(" ms"),
                        );
                        ui.label("  小さいほど多くキャッシュ。25 ms 推奨。");

                        ui.add_space(8.0);

                        // サイズしきい値を MB 単位で編集
                        ui.label("サイズしきい値 (このサイズ以上は無条件キャッシュ):");
                        let mut size_mb =
                            (self.settings.cache_size_threshold_bytes as f64) / 1_000_000.0;
                        if ui
                            .add(
                                egui::Slider::new(&mut size_mb, 0.5..=10.0)
                                    .step_by(0.5)
                                    .suffix(" MB"),
                            )
                            .changed()
                        {
                            self.settings.cache_size_threshold_bytes =
                                (size_mb * 1_000_000.0) as u64;
                        }
                        ui.label("  2 MB 推奨。これ以上の重い画像が確実にキャッシュされます。");

                        ui.add_space(8.0);

                        ui.checkbox(
                            &mut self.settings.cache_webp_always,
                            "既存 .webp は常にキャッシュ (デコードが重いため推奨)",
                        );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
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
                self.show_cache_policy_dialog = false;
            } else if cancel || !open {
                // キャンセル/×ボタン: 変更を破棄するため再ロード
                self.settings = crate::settings::Settings::load();
                self.show_cache_policy_dialog = false;
            }
        }

    }
}
