//! フルスクリーン画像補正パネル（左側オーバーレイ表示）。
//!
//! マウスを画面端（左・上・右）に寄せるとオーバーレイとして表示される。
//! 0=グローバル / 1-4=個別プリセットの選択、
//! 補正モード（手動/自動/漫画）のラジオ選択、
//! スライダーによる補正パラメータ調整、AI 設定、保存スロットを提供する。

use eframe::egui;

use crate::app::App;
use crate::adjustment::{AdjustParams, AutoMode, PresetSlot};

const HEADER_H: f32 = 36.0;
const SECTION_FONT: f32 = 12.0;
/// ラベルの色（暗い背景で読みやすい白系）
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);

/// 左パネルの幅
pub const LEFT_PANEL_WIDTH: f32 = 260.0;

/// スライダーとリセットボタンを描画するヘルパー。
/// リセットボタン（↩）をクリックするとデフォルト値に戻す。
macro_rules! slider_with_reset {
    ($ui:expr, $label:expr, $val:expr, $range:expr, $default:expr, $disabled:expr, $changed:expr, $dragging:expr) => {{
        $ui.horizontal(|ui| {
            ui.label(egui::RichText::new($label).size(SECTION_FONT).color(LABEL_COLOR));
            // 右端にリセットボタン
            if *$val != $default && !$disabled {
                let reset_resp = ui.small_button("↩");
                if reset_resp.clicked() {
                    *$val = $default;
                    $changed = true;
                }
                reset_resp.on_hover_text("デフォルトに戻す");
            }
        });
        let slider = egui::Slider::new($val, $range).step_by(1.0);
        let r = if $disabled {
            $ui.add_enabled(false, slider)
        } else {
            $ui.add(slider)
        };
        if r.changed() { $changed = true; }
        if r.dragged() { $dragging = true; }
    }};
}

macro_rules! slider_log_with_reset {
    ($ui:expr, $label:expr, $val:expr, $range:expr, $step:expr, $default:expr, $disabled:expr, $changed:expr, $dragging:expr) => {{
        $ui.horizontal(|ui| {
            ui.label(egui::RichText::new($label).size(SECTION_FONT).color(LABEL_COLOR));
            if (*$val - $default).abs() > 0.001 && !$disabled {
                let reset_resp = ui.small_button("↩");
                if reset_resp.clicked() {
                    *$val = $default;
                    $changed = true;
                }
                reset_resp.on_hover_text("デフォルトに戻す");
            }
        });
        let slider = egui::Slider::new($val, $range).logarithmic(true).step_by($step);
        let r = if $disabled {
            $ui.add_enabled(false, slider)
        } else {
            $ui.add(slider)
        };
        if r.changed() { $changed = true; }
        if r.dragged() { $dragging = true; }
    }};
}

/// プリセットのスライダーUI を描画する（self を借用しない純関数）。
fn draw_preset_sliders(
    ui: &mut egui::Ui,
    params: &mut AdjustParams,
    ai_denoise_available: bool,
    ai_upscale_available: bool,
) -> (bool, bool) {
    let mut changed = false;
    let mut dragging = false;

    let is_auto = params.auto_mode.is_some();

    // ── 補正モード（ラジオボタン）──
    ui.label(egui::RichText::new("補正モード").size(SECTION_FONT).color(LABEL_COLOR));
    ui.add_space(2.0);
    {
        let mut mode_changed = false;
        if ui.radio(params.auto_mode.is_none(),
            egui::RichText::new("手動").color(LABEL_COLOR)
        ).clicked() {
            params.auto_mode = None;
            mode_changed = true;
        }
        if ui.radio(params.auto_mode == Some(AutoMode::Auto),
            egui::RichText::new("自動補正").color(LABEL_COLOR)
        ).clicked() {
            params.auto_mode = Some(AutoMode::Auto);
            mode_changed = true;
        }
        if ui.radio(params.auto_mode == Some(AutoMode::MangaCleanup),
            egui::RichText::new("モノクロ漫画補正").color(LABEL_COLOR)
        ).clicked() {
            params.auto_mode = Some(AutoMode::MangaCleanup);
            mode_changed = true;
        }
        if mode_changed {
            changed = true;
        }
    }
    ui.add_space(8.0);

    slider_with_reset!(ui, "明るさ", &mut params.brightness, -100.0..=100.0, 0.0_f32, is_auto, changed, dragging);
    slider_with_reset!(ui, "コントラスト", &mut params.contrast, -100.0..=100.0, 0.0_f32, is_auto, changed, dragging);
    slider_log_with_reset!(ui, "ガンマ", &mut params.gamma, 0.2..=5.0, 0.01, 1.0_f32, is_auto, changed, dragging);
    slider_with_reset!(ui, "彩度", &mut params.saturation, -100.0..=100.0, 0.0_f32, is_auto, changed, dragging);
    slider_with_reset!(ui, "色温度", &mut params.temperature, -100.0..=100.0, 0.0_f32, is_auto, changed, dragging);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(egui::RichText::new("レベル補正").size(SECTION_FONT).color(LABEL_COLOR));
    {
        let mut bp = params.black_point as f32;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("黒点").size(SECTION_FONT).color(LABEL_COLOR));
            if bp != 0.0 && !is_auto {
                if ui.small_button("↩").on_hover_text("デフォルトに戻す").clicked() {
                    bp = 0.0;
                    params.black_point = 0;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut bp, 0.0..=254.0).step_by(1.0);
        let r = if is_auto { ui.add_enabled(false, slider) } else { ui.add(slider) };
        if r.changed() { params.black_point = bp as u8; changed = true; }
        if r.dragged() { dragging = true; }
    }
    {
        let mut wp = params.white_point as f32;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("白点").size(SECTION_FONT).color(LABEL_COLOR));
            if wp != 255.0 && !is_auto {
                if ui.small_button("↩").on_hover_text("デフォルトに戻す").clicked() {
                    wp = 255.0;
                    params.white_point = 255;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut wp, 1.0..=255.0).step_by(1.0);
        let r = if is_auto { ui.add_enabled(false, slider) } else { ui.add(slider) };
        if r.changed() { params.white_point = wp as u8; changed = true; }
        if r.dragged() { dragging = true; }
    }
    {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("中間点").size(SECTION_FONT).color(LABEL_COLOR));
            if (params.midtone - 1.0).abs() > 0.001 && !is_auto {
                if ui.small_button("↩").on_hover_text("デフォルトに戻す").clicked() {
                    params.midtone = 1.0;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut params.midtone, 0.1..=10.0).logarithmic(true).step_by(0.01);
        let r = if is_auto { ui.add_enabled(false, slider) } else { ui.add(slider) };
        if r.changed() { changed = true; }
        if r.dragged() { dragging = true; }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    if ai_denoise_available {
        ui.label(egui::RichText::new("AI ノイズ除去").size(SECTION_FONT).color(LABEL_COLOR));
        let is_on = params.denoise_model.is_some();
        let mut toggled = is_on;
        if ui.checkbox(&mut toggled, egui::RichText::new("JPEG ノイズ除去を適用").color(LABEL_COLOR)).changed() {
            params.denoise_model = if toggled {
                Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().to_string())
            } else {
                None
            };
            changed = true;
        }
        ui.add_space(8.0);
    }

    if ai_upscale_available {
        ui.label(egui::RichText::new("AI アップスケール").size(SECTION_FONT).color(LABEL_COLOR));

        for (label, val) in crate::adjustment::UPSCALE_MODELS {
            let is_sel = match (val, params.upscale_model.as_deref()) {
                (None, None) => true,
                (Some(a), Some(b)) => *a == b,
                _ => false,
            };
            if ui.radio(is_sel, egui::RichText::new(*label).color(LABEL_COLOR)).clicked() {
                params.upscale_model = val.map(|s| s.to_string());
                changed = true;
            }
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);

    if ui.button("すべてリセット").clicked() {
        *params = AdjustParams::default();
        changed = true;
    }
    ui.add_space(8.0);

    (changed, dragging)
}

impl App {
    /// 左パネルの画像補正パネルを描画する。
    pub(crate) fn draw_adjustment_panel(
        &mut self,
        ui: &mut egui::Ui,
        panel_rect: egui::Rect,
    ) {
        let preset_idx = self.adjustment_active_preset.unwrap_or(0);

        let painter = ui.painter_at(panel_rect);
        painter.rect_filled(panel_rect, 0.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230));

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect));
        child.set_clip_rect(panel_rect);

        // ── ヘッダー ──
        let header_rect = egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), HEADER_H));
        child.painter().text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            "画像補正",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );

        // ── プリセットタブ (0=グローバル, 1-4=個別) ──
        let tab_y = panel_rect.min.y + HEADER_H + 4.0;
        let tab_labels = ["0", "1", "2", "3", "4"];
        let tab_w = (panel_rect.width() - 20.0) / tab_labels.len() as f32;
        let mut preset_switch: Option<u8> = None;
        for (i, label) in tab_labels.iter().enumerate() {
            let pi = i as u8;
            let tab_rect = egui::Rect::from_min_size(
                egui::pos2(panel_rect.min.x + 10.0 + i as f32 * tab_w, tab_y),
                egui::vec2(tab_w - 2.0, 28.0),
            );
            let is_active = self.adjustment_active_preset == Some(pi);
            let bg = if is_active {
                egui::Color32::from_rgb(60, 120, 200)
            } else {
                egui::Color32::from_gray(50)
            };
            let resp = child.allocate_rect(tab_rect, egui::Sense::click());
            child.painter().rect_filled(tab_rect, 4.0, bg);

            let display_label = if pi == 0 { "G" } else { *label };
            child.painter().text(
                tab_rect.center(),
                egui::Align2::CENTER_CENTER,
                display_label,
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            );

            let tooltip = if pi == 0 {
                "グローバル [0]".to_string()
            } else {
                let name = &self.adjustment_presets.names[(pi - 1) as usize];
                format!("{} [{}]", name, pi)
            };
            let resp = resp.on_hover_text(tooltip);
            if resp.clicked() {
                preset_switch = Some(pi);
            }
        }

        // ── タブ下: プリセット種別表示 ──
        let label_y = tab_y + 32.0;
        let label_text = if preset_idx == 0 {
            "グローバル".to_string()
        } else {
            let name = &self.adjustment_presets.names[(preset_idx - 1) as usize];
            format!("個別: {}", name)
        };
        child.painter().text(
            egui::pos2(panel_rect.min.x + 10.0, label_y),
            egui::Align2::LEFT_TOP,
            &label_text,
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(180),
        );

        // ── プリセット名の編集 (個別 1-4 のみ) ──
        let name_edit_h = if preset_idx >= 1 { 28.0 } else { 0.0 };
        let name_edit_y = label_y + 16.0;
        if preset_idx >= 1 {
            let name_rect = egui::Rect::from_min_size(
                egui::pos2(panel_rect.min.x + 10.0, name_edit_y),
                egui::vec2(panel_rect.width() - 20.0, 22.0),
            );
            let mut name_child = child.new_child(egui::UiBuilder::new().max_rect(name_rect));
            let idx = (preset_idx - 1) as usize;
            let resp = name_child.add(
                egui::TextEdit::singleline(&mut self.adjustment_presets.names[idx])
                    .desired_width(name_rect.width())
                    .font(egui::FontId::proportional(12.0))
                    .hint_text("プリセット名")
            );
            self.editing_preset_name = resp.has_focus();
        } else {
            self.editing_preset_name = false;
        }

        // ── レイアウト計算: スライダー領域と保存スロット領域 ──
        let content_top = name_edit_y + name_edit_h + 8.0;
        // 保存スロット: 5行×2列 + ラベル + マージン
        let slots_height = 5.0 * 26.0 + 28.0;
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, content_top),
            egui::pos2(panel_rect.max.x, panel_rect.max.y - slots_height),
        );
        let mut scroll_child = child.new_child(egui::UiBuilder::new().max_rect(content_rect));

        let params = if preset_idx == 0 {
            &mut self.settings.global_preset
        } else {
            &mut self.adjustment_presets.presets[(preset_idx - 1) as usize]
        };

        let (changed, is_dragging) = egui::ScrollArea::vertical()
            .max_height(content_rect.height())
            .show(&mut scroll_child, |ui| {
                ui.set_width(panel_rect.width() - 20.0);
                ui.add_space(8.0);
                draw_preset_sliders(
                    ui,
                    params,
                    self.settings.ai_denoise_feature,
                    self.settings.ai_upscale_feature,
                )
            }).inner;

        self.adjustment_dragging = is_dragging;

        // ── 保存スロット (常時表示) ──
        let slots_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, panel_rect.max.y - slots_height),
            panel_rect.max,
        );
        let mut slots_child = child.new_child(egui::UiBuilder::new().max_rect(slots_rect));
        let mut save_to_slot: Option<usize> = None;
        let mut load_from_slot: Option<usize> = None;

        slots_child.separator();
        slots_child.add_space(4.0);
        slots_child.label(egui::RichText::new("保存スロット").size(11.0).color(LABEL_COLOR));
        slots_child.add_space(2.0);

        let btn_w = (panel_rect.width() - 20.0) * 0.5 - 24.0; // 保存アイコン分を確保
        // 5行×2列
        for row in 0..5 {
            slots_child.horizontal(|ui| {
                for col in 0..2 {
                    let slot_idx = row * 2 + col;
                    let key_label = crate::adjustment::slot_key_label(slot_idx);
                    let slot_name = if let Some(s) = &self.settings.preset_slots.slots[slot_idx] {
                        format!("{}:{}", key_label, crate::ui_helpers::truncate_name(&s.name, 7))
                    } else {
                        format!("{}:空", key_label)
                    };
                    let has_data = self.settings.preset_slots.slots[slot_idx].is_some();

                    let name_btn = egui::Button::new(
                        egui::RichText::new(&slot_name).size(10.5)
                    ).min_size(egui::vec2(btn_w, 22.0));
                    let name_resp = ui.add_enabled(has_data, name_btn);
                    if name_resp.clicked() {
                        load_from_slot = Some(slot_idx);
                    }
                    if let Some(s) = &self.settings.preset_slots.slots[slot_idx] {
                        name_resp.on_hover_text(format!("{} をロード (Shift+{})", s.name, key_label));
                    }

                    let save_btn = egui::Button::new(
                        egui::RichText::new("💾").size(11.0)
                    ).min_size(egui::vec2(22.0, 22.0));
                    let save_resp = ui.add(save_btn);
                    if save_resp.clicked() {
                        save_to_slot = Some(slot_idx);
                    }
                    save_resp.on_hover_text(format!("現在の設定をスロット{}に保存", key_label));
                }
            });
        }

        // 保存処理
        if let Some(slot_idx) = save_to_slot {
            if let Some(pi) = self.adjustment_active_preset {
                let p = self.get_preset_params(pi);
                let name = if pi == 0 {
                    "グローバル".to_string()
                } else {
                    self.adjustment_presets.names[(pi - 1) as usize].clone()
                };
                self.settings.preset_slots.slots[slot_idx] = Some(PresetSlot {
                    name,
                    params: p,
                });
                self.settings.save();
                let key_label = crate::adjustment::slot_key_label(slot_idx);
                self.show_feedback_toast(format!("[スロット{}に保存]", key_label));
            }
        }

        // ロード処理
        if let Some(slot_idx) = load_from_slot {
            self.load_slot_to_active_preset(slot_idx);
        }

        // プリセット切替
        if let Some(new_pi) = preset_switch {
            let old_params = self.adjustment_active_preset.map(|pi| self.get_preset_params(pi));
            self.adjustment_active_preset = Some(new_pi);
            if let Some(fs_idx) = self.fullscreen_idx {
                self.assign_page_preset(fs_idx, new_pi);
                let new_params = self.get_preset_params(new_pi);
                if old_params.as_ref().map_or(true, |old| !old.ai_settings_eq(&new_params)) {
                    self.clear_all_adjustment_and_ai_caches(fs_idx);
                } else {
                    self.clear_adjustment_caches(fs_idx);
                }
            }
        }

        // パラメータ変更があればキャッシュクリア + 保存（ドラッグ中はdisk書き込みを抑制）
        if changed {
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_adjustment_caches(fs_idx);
            }
            if !is_dragging {
                self.save_current_preset(preset_idx);
            }
        }
    }
}
