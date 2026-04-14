//! フルスクリーン画像補正パネル。
//!
//! キー 7/8/9/0 またはホバーバーのプリセットボタンで表示される。
//! 画像の右側に固定幅パネルを表示し、補正パラメータのスライダーと
//! AI アップスケール設定を提供する。

use eframe::egui;

use crate::app::App;
use crate::adjustment::{AdjustParams, AutoMode};

const HEADER_H: f32 = 36.0;
const SECTION_FONT: f32 = 12.0;
/// スライダーラベルの色（暗い背景で読みやすい明るめの灰色）
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(210, 210, 210);

/// プリセットのスライダーUI を描画する（self を借用しない純関数）。
fn draw_preset_sliders(
    ui: &mut egui::Ui,
    params: &mut AdjustParams,
    panel_width: f32,
) -> (bool, bool) {
    let mut changed = false;
    let mut dragging = false;

    // ── 自動補正ボタン ──
    ui.horizontal(|ui| {
        let modes = [AutoMode::AutoLevel, AutoMode::AutoContrast, AutoMode::MangaCleanup, AutoMode::ScanFix];
        for mode in &modes {
            let is_active = params.auto_mode == Some(*mode);
            let text = match mode {
                AutoMode::AutoLevel => "自動",
                AutoMode::AutoContrast => "コントラスト",
                AutoMode::MangaCleanup => "漫画",
                AutoMode::ScanFix => "スキャン",
            };
            let btn = egui::Button::new(
                egui::RichText::new(text).size(11.0).color(egui::Color32::WHITE)
            ).fill(if is_active { egui::Color32::from_rgb(60, 120, 200) } else { egui::Color32::from_gray(60) });
            if ui.add(btn).clicked() {
                params.auto_mode = if is_active { None } else { Some(*mode) };
                changed = true;
            }
        }
    });
    ui.add_space(8.0);

    // ── マクロ的なスライダー描画ヘルパー ──
    macro_rules! slider {
        ($ui:expr, $label:expr, $val:expr, $range:expr) => {{
            $ui.label(egui::RichText::new($label).size(SECTION_FONT).color(LABEL_COLOR));
            let r = $ui.add(egui::Slider::new($val, $range).step_by(1.0));
            if r.changed() { changed = true; }
            if r.dragged() { dragging = true; }
        }};
    }
    macro_rules! slider_log {
        ($ui:expr, $label:expr, $val:expr, $range:expr, $step:expr) => {{
            $ui.label(egui::RichText::new($label).size(SECTION_FONT).color(LABEL_COLOR));
            let r = $ui.add(egui::Slider::new($val, $range).logarithmic(true).step_by($step));
            if r.changed() { changed = true; }
            if r.dragged() { dragging = true; }
        }};
    }

    slider!(ui, "明るさ", &mut params.brightness, -100.0..=100.0);
    slider!(ui, "コントラスト", &mut params.contrast, -100.0..=100.0);
    slider_log!(ui, "ガンマ", &mut params.gamma, 0.2..=5.0, 0.01);
    slider!(ui, "彩度", &mut params.saturation, -100.0..=100.0);
    slider!(ui, "色温度", &mut params.temperature, -100.0..=100.0);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(egui::RichText::new("レベル補正").size(SECTION_FONT).color(LABEL_COLOR));
    {
        let mut bp = params.black_point as f32;
        let r = ui.add(egui::Slider::new(&mut bp, 0.0..=254.0).text("黒点").step_by(1.0));
        if r.changed() { params.black_point = bp as u8; changed = true; }
        if r.dragged() { dragging = true; }
    }
    {
        let mut wp = params.white_point as f32;
        let r = ui.add(egui::Slider::new(&mut wp, 1.0..=255.0).text("白点").step_by(1.0));
        if r.changed() { params.white_point = wp as u8; changed = true; }
        if r.dragged() { dragging = true; }
    }
    {
        let r = ui.add(egui::Slider::new(&mut params.midtone, 0.1..=10.0).text("中間点").logarithmic(true).step_by(0.01));
        if r.changed() { changed = true; }
        if r.dragged() { dragging = true; }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(egui::RichText::new("シャープネス").size(SECTION_FONT).color(LABEL_COLOR));
    {
        let r = ui.add(egui::Slider::new(&mut params.sharpness, 0.0..=100.0).step_by(1.0));
        if r.changed() { changed = true; }
        if r.dragged() { dragging = true; }
    }
    {
        let mut radius = params.sharpen_radius as f32;
        let r = ui.add(egui::Slider::new(&mut radius, 1.0..=5.0).text("半径").step_by(1.0));
        if r.changed() { params.sharpen_radius = radius as u8; changed = true; }
        if r.dragged() { dragging = true; }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // ── AI アップスケール ──
    ui.label(egui::RichText::new("AI アップスケール").size(SECTION_FONT).color(LABEL_COLOR));

    let upscale_items: &[(&str, Option<&str>)] = &[
        ("なし", None),
        ("自動 (画像タイプ判別)", Some("auto")),
        ("写真/CG (Real-ESRGAN x4plus)", Some("realesrgan_x4plus")),
        ("イラスト (Real-ESRGAN Anime)", Some("realesrgan_anime6b")),
        ("漫画 (Real-CUGAN 4x)", Some("realcugan_4x")),
        ("汎用 (Real-ESRGAN General)", Some("realesr_general_v3")),
    ];
    let cur = upscale_items.iter().position(|(_, val)| {
        match (val, params.upscale_model.as_deref()) {
            (None, None) => true,
            (Some(a), Some(b)) => *a == b,
            _ => false,
        }
    }).unwrap_or(0);

    egui::ComboBox::from_id_salt("upscale_model")
        .selected_text(upscale_items[cur].0)
        .width(panel_width - 30.0)
        .show_ui(ui, |ui| {
            for (label, val) in upscale_items {
                let is_sel = match (val, params.upscale_model.as_deref()) {
                    (None, None) => true,
                    (Some(a), Some(b)) => *a == b,
                    _ => false,
                };
                if ui.selectable_label(is_sel, *label).clicked() {
                    params.upscale_model = val.map(|s| s.to_string());
                    changed = true;
                }
            }
        });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);

    // ── リセット（全パラメータ初期化）──
    if ui.button("すべてリセット").clicked() {
        *params = AdjustParams::default();
        changed = true;
    }
    ui.add_space(8.0);

    (changed, dragging)
}

impl App {
    /// 画像補正パネルを描画する。
    pub(crate) fn draw_adjustment_panel(
        &mut self,
        ui: &mut egui::Ui,
        panel_rect: egui::Rect,
    ) {
        let preset_idx = self.adjustment_active_preset.unwrap_or(0) as usize;

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

        // ── プリセットタブ ──
        let tab_y = panel_rect.min.y + HEADER_H + 4.0;
        let tab_w = (panel_rect.width() - 20.0) / 4.0;
        let tab_labels = ["1", "2", "3", "4"];
        let mut preset_switch: Option<Option<u8>> = None;
        for (i, label) in tab_labels.iter().enumerate() {
            let tab_rect = egui::Rect::from_min_size(
                egui::pos2(panel_rect.min.x + 10.0 + i as f32 * tab_w, tab_y),
                egui::vec2(tab_w - 2.0, 28.0),
            );
            let is_active = self.adjustment_active_preset == Some(i as u8);
            let bg = if is_active {
                egui::Color32::from_rgb(60, 120, 200)
            } else {
                egui::Color32::from_gray(50)
            };
            let resp = child.allocate_rect(tab_rect, egui::Sense::click());
            child.painter().rect_filled(tab_rect, 4.0, bg);
            child.painter().text(
                tab_rect.center(),
                egui::Align2::CENTER_CENTER,
                *label,
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
            if resp.clicked() {
                if is_active {
                    preset_switch = Some(None); // deactivate
                } else {
                    preset_switch = Some(Some(i as u8));
                }
            }
        }

        // ── スクロール領域: スライダー群 ──
        let content_top = tab_y + 36.0;
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, content_top),
            panel_rect.max,
        );
        let mut scroll_child = child.new_child(egui::UiBuilder::new().max_rect(content_rect));

        let (changed, is_dragging) = egui::ScrollArea::vertical()
            .max_height(content_rect.height())
            .show(&mut scroll_child, |ui| {
                ui.set_width(panel_rect.width() - 20.0);
                ui.add_space(8.0);
                draw_preset_sliders(ui, &mut self.adjustment_presets.presets[preset_idx], panel_rect.width())
            }).inner;

        self.adjustment_dragging = is_dragging;

        // プリセット切替
        if let Some(sw) = preset_switch {
            match sw {
                None => {
                    self.adjustment_active_preset = None;
                    self.adjustment_mode = false;
                }
                Some(n) => {
                    self.adjustment_active_preset = Some(n);
                    // ページにプリセットを割り当て
                    if let Some(fs_idx) = self.fullscreen_idx {
                        self.adjustment_page_preset.insert(fs_idx, n);
                        if let Some(key) = self.page_path_key(fs_idx) {
                            if let Some(db) = &self.adjustment_db {
                                let _ = db.set_page_preset(&key, Some(n));
                            }
                        }
                    }
                }
            }
            // キャッシュクリア
            if let Some(fs_idx) = self.fullscreen_idx {
                self.adjustment_cache.remove(&fs_idx);
                self.adjustment_sharpened.remove(&fs_idx);
                self.adjustment_preview_tex = None;
                self.ai_upscale_cache.clear();
                self.ai_upscale_failed.clear();
                for (_, (cancel, _)) in self.ai_upscale_pending.drain() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        // パラメータ変更があればキャッシュクリア + DB 保存
        if changed {
            if let Some(fs_idx) = self.fullscreen_idx {
                self.adjustment_cache.remove(&fs_idx);
                self.adjustment_sharpened.remove(&fs_idx);
                self.adjustment_preview_tex = None;
                self.ai_upscale_cache.clear();
                self.ai_upscale_failed.clear();
                for (_, (cancel, _)) in self.ai_upscale_pending.drain() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if let (Some(db), Some(folder)) = (&self.adjustment_db, &self.current_folder) {
                let _ = db.set_presets(folder, &self.adjustment_presets);
            }
        }
    }
}
