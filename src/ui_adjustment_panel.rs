//! フルスクリーン画像補正パネル（左側オーバーレイ表示）。
//!
//! マウスを画面端（左・上・右）に寄せるとオーバーレイとして表示される。
//! スコープは標準設定 + ページ個別の 2 つ。
//!
//! - パネルでスライダーを操作すると、その瞬間に「現在のページ個別パラメータ」が更新される
//!   (ページ個別設定が自動生成される)
//! - アクションボタン 4 種 (2x2 グリッド):
//!     - 「全画像に適用」   — 現在の一覧 (フォルダ/ZIP/PDF) の全画像ページに反映
//!     - 「全画像から削除」 — 現在の一覧の全画像ページから個別設定を削除 (標準に戻す)
//!     - 「標準にする」     — 現在のパラメータを settings.global_preset にコピー
//!     - 「個別設定を解除」 — 現在のページの個別設定を削除 (標準値に戻す)
//! - 保存スロット 10 個: クリック or Ctrl+数字で現在のページに適用

use eframe::egui;

use crate::adjustment::{AdjustParams, AutoMode, PostFilter, PresetSlot};
use crate::app::{AdjustSpreadTarget, App};
use crate::ui_fullscreen::SpreadPair;

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
            ui.label(
                egui::RichText::new($label)
                    .size(SECTION_FONT)
                    .color(LABEL_COLOR),
            );
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
        if r.changed() {
            $changed = true;
        }
        if r.dragged() {
            $dragging = true;
        }
    }};
}

macro_rules! slider_log_with_reset {
    ($ui:expr, $label:expr, $val:expr, $range:expr, $step:expr, $default:expr, $disabled:expr, $changed:expr, $dragging:expr) => {{
        $ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new($label)
                    .size(SECTION_FONT)
                    .color(LABEL_COLOR),
            );
            if (*$val - $default).abs() > 0.001 && !$disabled {
                let reset_resp = ui.small_button("↩");
                if reset_resp.clicked() {
                    *$val = $default;
                    $changed = true;
                }
                reset_resp.on_hover_text("デフォルトに戻す");
            }
        });
        let slider = egui::Slider::new($val, $range)
            .logarithmic(true)
            .step_by($step);
        let r = if $disabled {
            $ui.add_enabled(false, slider)
        } else {
            $ui.add(slider)
        };
        if r.changed() {
            $changed = true;
        }
        if r.dragged() {
            $dragging = true;
        }
    }};
}

/// スライダー UI (純関数)。ai_denoise_disabled_threshold / ai_upscale_disabled_threshold が
/// Some なら画像サイズ閾値により AI 機能が無効になる旨を表示する。
fn draw_sliders(
    ui: &mut egui::Ui,
    params: &mut AdjustParams,
    ai_denoise_disabled_threshold: Option<u32>,
    ai_upscale_disabled_threshold: Option<u32>,
) -> (bool, bool) {
    let mut changed = false;
    let mut dragging = false;
    let is_auto = params.auto_mode.is_some();

    // ── 補正モード ──
    ui.label(
        egui::RichText::new("補正モード")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    ui.add_space(2.0);
    {
        let mut mode_changed = false;
        if ui
            .radio(
                params.auto_mode.is_none(),
                egui::RichText::new("手動").color(LABEL_COLOR),
            )
            .clicked()
        {
            params.auto_mode = None;
            mode_changed = true;
        }
        if ui
            .radio(
                params.auto_mode == Some(AutoMode::Auto),
                egui::RichText::new("自動補正").color(LABEL_COLOR),
            )
            .clicked()
        {
            params.auto_mode = Some(AutoMode::Auto);
            mode_changed = true;
        }
        if ui
            .radio(
                params.auto_mode == Some(AutoMode::MangaCleanup),
                egui::RichText::new("モノクロ漫画補正").color(LABEL_COLOR),
            )
            .clicked()
        {
            params.auto_mode = Some(AutoMode::MangaCleanup);
            mode_changed = true;
        }
        if mode_changed {
            changed = true;
        }
    }
    ui.add_space(8.0);

    slider_with_reset!(
        ui,
        "明るさ",
        &mut params.brightness,
        -100.0..=100.0,
        0.0_f32,
        is_auto,
        changed,
        dragging
    );
    slider_with_reset!(
        ui,
        "コントラスト",
        &mut params.contrast,
        -100.0..=100.0,
        0.0_f32,
        is_auto,
        changed,
        dragging
    );
    slider_log_with_reset!(
        ui,
        "ガンマ",
        &mut params.gamma,
        0.2..=5.0,
        0.01,
        1.0_f32,
        is_auto,
        changed,
        dragging
    );
    slider_with_reset!(
        ui,
        "彩度",
        &mut params.saturation,
        -100.0..=100.0,
        0.0_f32,
        is_auto,
        changed,
        dragging
    );
    slider_with_reset!(
        ui,
        "色温度",
        &mut params.temperature,
        -100.0..=100.0,
        0.0_f32,
        is_auto,
        changed,
        dragging
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("レベル補正")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    {
        let mut bp = params.black_point as f32;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("黒点")
                    .size(SECTION_FONT)
                    .color(LABEL_COLOR),
            );
            if bp != 0.0 && !is_auto {
                if ui
                    .small_button("↩")
                    .on_hover_text("デフォルトに戻す")
                    .clicked()
                {
                    bp = 0.0;
                    params.black_point = 0;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut bp, 0.0..=254.0).step_by(1.0);
        let r = if is_auto {
            ui.add_enabled(false, slider)
        } else {
            ui.add(slider)
        };
        if r.changed() {
            params.black_point = bp as u8;
            changed = true;
        }
        if r.dragged() {
            dragging = true;
        }
    }
    {
        let mut wp = params.white_point as f32;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("白点")
                    .size(SECTION_FONT)
                    .color(LABEL_COLOR),
            );
            if wp != 255.0 && !is_auto {
                if ui
                    .small_button("↩")
                    .on_hover_text("デフォルトに戻す")
                    .clicked()
                {
                    wp = 255.0;
                    params.white_point = 255;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut wp, 1.0..=255.0).step_by(1.0);
        let r = if is_auto {
            ui.add_enabled(false, slider)
        } else {
            ui.add(slider)
        };
        if r.changed() {
            params.white_point = wp as u8;
            changed = true;
        }
        if r.dragged() {
            dragging = true;
        }
    }
    {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("中間点")
                    .size(SECTION_FONT)
                    .color(LABEL_COLOR),
            );
            if (params.midtone - 1.0).abs() > 0.001 && !is_auto {
                if ui
                    .small_button("↩")
                    .on_hover_text("デフォルトに戻す")
                    .clicked()
                {
                    params.midtone = 1.0;
                    changed = true;
                }
            }
        });
        let slider = egui::Slider::new(&mut params.midtone, 0.1..=10.0)
            .logarithmic(true)
            .step_by(0.01);
        let r = if is_auto {
            ui.add_enabled(false, slider)
        } else {
            ui.add(slider)
        };
        if r.changed() {
            changed = true;
        }
        if r.dragged() {
            dragging = true;
        }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("AI ノイズ除去 [N: ON/OFF]")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    if let Some(px) = ai_denoise_disabled_threshold {
        ui.label(
            egui::RichText::new(format!("（この画像は {}px 以上なので実行されません）", px))
                .size(SECTION_FONT - 1.0)
                .color(egui::Color32::from_gray(150))
                .italics(),
        );
    }
    let is_on = params.denoise_model.is_some();
    let mut toggled = is_on;
    if ui
        .checkbox(
            &mut toggled,
            egui::RichText::new("JPEG ノイズ除去を適用").color(LABEL_COLOR),
        )
        .changed()
    {
        params.denoise_model = if toggled {
            Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().to_string())
        } else {
            None
        };
        changed = true;
    }
    ui.add_space(8.0);

    ui.label(
        egui::RichText::new("AI アップスケール [U: 次 / Shift+U: 前 / Alt+U: リセット]")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    if let Some(px) = ai_upscale_disabled_threshold {
        ui.label(
            egui::RichText::new(format!("（この画像は {}px 以上なので実行されません）", px))
                .size(SECTION_FONT - 1.0)
                .color(egui::Color32::from_gray(150))
                .italics(),
        );
    }
    for (label, val) in &crate::adjustment::upscale_menu_items() {
        let is_sel = match (val, params.upscale_model.as_deref()) {
            (None, None) => true,
            (Some(a), Some(b)) => *a == b,
            _ => false,
        };
        if ui
            .radio(is_sel, egui::RichText::new(*label).color(LABEL_COLOR))
            .clicked()
        {
            params.upscale_model = val.map(|s| s.to_string());
            changed = true;
        }
    }

    // ── ポストフィルタ (レトロ系 + 写真系エフェクト) ──
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("ポストフィルタ [T: 次 / Shift+T: 前 / Alt+T: リセット]")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    let before_pf = params.post_filter;
    egui::ComboBox::from_id_salt("post_filter_combo")
        .selected_text(params.post_filter.display_label())
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            let group_heading = |ui: &mut egui::Ui, text: &str| {
                ui.label(
                    egui::RichText::new(text)
                        .size(SECTION_FONT - 1.0)
                        .color(egui::Color32::from_gray(150)),
                );
            };

            group_heading(ui, "── 基本 ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::None,
                PostFilter::None.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Nearest,
                PostFilter::Nearest.display_label(),
            );
            ui.separator();
            group_heading(ui, "── CRT ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::CrtSimple,
                PostFilter::CrtSimple.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::CrtFull,
                PostFilter::CrtFull.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::CrtArcade,
                PostFilter::CrtArcade.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 減色・ディザ (色数昇順) ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Dither1bit,
                PostFilter::Dither1bit.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::GameBoy,
                PostFilter::GameBoy.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Pc98,
                PostFilter::Pc98.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::GameGear,
                PostFilter::GameGear.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Famicom,
                PostFilter::Famicom.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::MegaDrive,
                PostFilter::MegaDrive.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Msx2Plus,
                PostFilter::Msx2Plus.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Sfc,
                PostFilter::Sfc.display_label(),
            );
            ui.separator();
            group_heading(ui, "── CRT × 非液晶機種 ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::ComboFamicomCrt,
                PostFilter::ComboFamicomCrt.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::ComboPc98Crt,
                PostFilter::ComboPc98Crt.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::ComboMsx2PlusCrt,
                PostFilter::ComboMsx2PlusCrt.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::ComboMegaDriveCrt,
                PostFilter::ComboMegaDriveCrt.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::ComboSfcCrt,
                PostFilter::ComboSfcCrt.display_label(),
            );
            ui.separator();
            group_heading(ui, "── カラーグレーディング ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Sepia,
                PostFilter::Sepia.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::MonoNeutral,
                PostFilter::MonoNeutral.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::MonoCool,
                PostFilter::MonoCool.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::MonoWarm,
                PostFilter::MonoWarm.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::WarmTone,
                PostFilter::WarmTone.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::CoolTone,
                PostFilter::CoolTone.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::TealOrange,
                PostFilter::TealOrange.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::KodakPortra,
                PostFilter::KodakPortra.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::FujiVelvia,
                PostFilter::FujiVelvia.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::BleachBypass,
                PostFilter::BleachBypass.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::CrossProcess,
                PostFilter::CrossProcess.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Vintage,
                PostFilter::Vintage.display_label(),
            );
            ui.separator();
            group_heading(ui, "── アナログフィルム ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::FilmGrain,
                PostFilter::FilmGrain.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Vignette,
                PostFilter::Vignette.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::LightLeak,
                PostFilter::LightLeak.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::SoftFocus,
                PostFilter::SoftFocus.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 絵画・描画風 ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Halftone,
                PostFilter::Halftone.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::OilPaint,
                PostFilter::OilPaint.display_label(),
            );
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Sketch,
                PostFilter::Sketch.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 実用 ──");
            ui.selectable_value(
                &mut params.post_filter,
                PostFilter::Sharpen,
                PostFilter::Sharpen.display_label(),
            );
        });
    if params.post_filter != before_pf {
        changed = true;
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
        image_dims: Option<(u32, u32)>,
    ) {
        // フルスクリーン対象のページ idx
        let Some(fs_root_idx) = self.fullscreen_idx else {
            return;
        };

        // 見開き Double 表示中は adjust_spread_target に応じて左/右ページを編集対象に。
        // Single では fs_root_idx をそのまま使う。以降の `fs_idx` は編集対象 idx を指し、
        // 補正値読み書きパスは単ページ経路と同一。
        let (fs_idx, spread_lr): (usize, Option<(usize, usize)>) =
            match self.resolve_spread_pair(fs_root_idx) {
                SpreadPair::Double { left, right } => {
                    let target = match self.adjust_spread_target {
                        AdjustSpreadTarget::Left => left,
                        AdjustSpreadTarget::Right => right,
                    };
                    (target, Some((left, right)))
                }
                SpreadPair::Single => (fs_root_idx, None),
            };

        let painter = ui.painter_at(panel_rect);
        painter.rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230),
        );

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect));
        child.set_clip_rect(panel_rect);

        // ── ヘッダー ──
        let header_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), HEADER_H));
        child.painter().text(
            header_rect.center(),
            egui::Align2::CENTER_CENTER,
            "画像補正",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );

        // ── 見開き L/R セレクタ + コピーボタン (Double のときだけ) ──
        let selector_h = if spread_lr.is_some() { 30.0 } else { 0.0 };
        if let Some((left, right)) = spread_lr {
            let same = self.effective_params(left) == self.effective_params(right);
            let selector_rect = egui::Rect::from_min_size(
                egui::pos2(panel_rect.min.x + 6.0, panel_rect.min.y + HEADER_H + 4.0),
                egui::vec2(panel_rect.width() - 12.0, 24.0),
            );
            let mut selector_child =
                child.new_child(egui::UiBuilder::new().max_rect(selector_rect));
            selector_child.horizontal(|ui| {
                let is_left = self.adjust_spread_target == AdjustSpreadTarget::Left;
                if ui.selectable_label(is_left, "左ページ").clicked() {
                    self.adjust_spread_target = AdjustSpreadTarget::Left;
                }
                let copy_l = ui
                    .add_enabled(!same, egui::Button::new("←").small())
                    .on_hover_text(if same {
                        "左右の補正値が同一です"
                    } else {
                        "右ページの設定を左ページへコピー"
                    });
                if copy_l.clicked() {
                    self.copy_spread_adjust(right, left);
                }
                let copy_r = ui
                    .add_enabled(!same, egui::Button::new("→").small())
                    .on_hover_text(if same {
                        "左右の補正値が同一です"
                    } else {
                        "左ページの設定を右ページへコピー"
                    });
                if copy_r.clicked() {
                    self.copy_spread_adjust(left, right);
                }
                if ui.selectable_label(!is_left, "右ページ").clicked() {
                    self.adjust_spread_target = AdjustSpreadTarget::Right;
                }
            });
        }

        // ── スコープ表示: 個別 / お気に入り標準 / 標準 ──
        let has_override = self.adjustment_page_params.contains_key(&fs_idx);
        let fav_default_active = !has_override
            && self
                .current_favorite_id_for_idx(fs_idx)
                .map(|id| self.adjustment_favorite_params.contains_key(&id))
                .unwrap_or(false);
        let scope_y = panel_rect.min.y + HEADER_H + 4.0 + selector_h;
        let scope_text = if has_override {
            "個別設定を適用中".to_string()
        } else if fav_default_active {
            let name = self
                .current_favorite_id_for_idx(fs_idx)
                .and_then(|id| self.settings.favorite_by_id(id))
                .map(|f| crate::ui_helpers::truncate_name(&f.name, 10))
                .unwrap_or_default();
            format!("お気に入り「{}」の標準を適用中", name)
        } else {
            "標準設定を適用中".to_string()
        };
        let scope_color = if has_override {
            egui::Color32::from_rgb(220, 180, 80)
        } else if fav_default_active {
            egui::Color32::from_rgb(120, 180, 220)
        } else {
            egui::Color32::from_gray(180)
        };
        child.painter().text(
            egui::pos2(panel_rect.min.x + 10.0, scope_y),
            egui::Align2::LEFT_TOP,
            scope_text,
            egui::FontId::proportional(12.0),
            scope_color,
        );

        // ── アクションボタン (5 行: wide × 4 + 2 カラム × 1) ──
        let buttons_y = scope_y + 20.0;
        let buttons_h = 28.0 * 5.0 + 4.0 * 4.0; // 5 行分 + 行間マージン 4 つ
        let buttons_rect = egui::Rect::from_min_size(
            egui::pos2(panel_rect.min.x + 8.0, buttons_y),
            egui::vec2(panel_rect.width() - 16.0, buttons_h),
        );
        let mut actions_child = child.new_child(egui::UiBuilder::new().max_rect(buttons_rect));
        let mut apply_all_clicked = false;
        let mut clear_all_clicked = false;
        let mut set_as_favorite_clicked = false;
        let mut clear_favorite_clicked = false;
        let mut set_as_global_clicked = false;
        let mut clear_page_clicked = false;
        // 編集対象ページを含むお気に入り (なければ None)。
        // 見開き L/R 切替で対象ページが変わったら、それに応じた favorite を引き直す。
        let fav_info = self
            .current_favorite_id_for_idx(fs_idx)
            .and_then(|id| self.settings.favorite_by_id(id))
            .map(|f| (f.id, f.name.clone()));
        let fav_display_name = fav_info
            .as_ref()
            .map(|(_, n)| crate::ui_helpers::truncate_name(n, 10))
            .unwrap_or_else(|| "このお気に入り".to_string());
        let has_favorite_default = fav_info
            .as_ref()
            .map(|(id, _)| self.adjustment_favorite_params.contains_key(id))
            .unwrap_or(false);
        let under_favorite = fav_info.is_some();
        actions_child.vertical(|ui| {
            let wide = egui::vec2(panel_rect.width() - 20.0, 24.0);
            if ui
                .add(egui::Button::new("このフォルダの全画像に適用").min_size(wide))
                .on_hover_text("このフォルダ/ZIP/PDF の全画像に現在のパラメータを書き込む")
                .clicked()
            {
                apply_all_clicked = true;
            }
            if ui
                .add(egui::Button::new("このフォルダの全画像から解除").min_size(wide))
                .on_hover_text("このフォルダ/ZIP/PDF の全画像の個別設定を削除し、標準設定に戻す")
                .clicked()
            {
                clear_all_clicked = true;
            }
            // お気に入り行 (名前入り)
            let set_fav_label = format!("このお気に入り「{}」の標準にする", fav_display_name);
            let clear_fav_label = format!("このお気に入り「{}」の標準を解除", fav_display_name);
            let set_fav_resp = ui.add_enabled(
                under_favorite,
                egui::Button::new(set_fav_label).min_size(wide),
            );
            if set_fav_resp.clicked() {
                set_as_favorite_clicked = true;
            }
            set_fav_resp.on_hover_text(if under_favorite {
                "このお気に入り配下のページで効く標準設定を、現在のパラメータで上書きする"
            } else {
                "お気に入り登録されたフォルダ配下にいるときのみ使用できます"
            });
            let clear_fav_resp = ui.add_enabled(
                under_favorite && has_favorite_default,
                egui::Button::new(clear_fav_label).min_size(wide),
            );
            if clear_fav_resp.clicked() {
                clear_favorite_clicked = true;
            }
            clear_fav_resp.on_hover_text(
                "このお気に入りの標準設定を削除し、アプリ全体の標準設定にフォールバックする",
            );
            ui.horizontal(|ui| {
                if ui
                    .small_button("標準にする")
                    .on_hover_text("現在のパラメータをアプリ全体の標準設定にする")
                    .clicked()
                {
                    set_as_global_clicked = true;
                }
                if ui
                    .add_enabled(
                        has_override,
                        egui::Button::new("個別設定を解除 [Q]").small(),
                    )
                    .on_hover_text(
                        "このページの個別設定を削除し、標準値に戻す (Q または Ctrl+Backspace)",
                    )
                    .clicked()
                {
                    clear_page_clicked = true;
                }
            });
        });

        // ── レイアウト計算: スライダー領域と保存スロット領域 ──
        let content_top = buttons_y + buttons_h + 6.0;
        // 保存スロット: 5行×2列 + ラベル + マージン
        let slots_height = 5.0 * 26.0 + 28.0;
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, content_top),
            egui::pos2(panel_rect.max.x, panel_rect.max.y - slots_height),
        );
        let mut scroll_child = child.new_child(egui::UiBuilder::new().max_rect(content_rect));

        // 現在の有効パラメータを取得して編集用コピーを作る
        let mut edit_params = self.effective_params(fs_idx).clone();
        let original = edit_params.clone();

        // しきい値以上ならスキップされる → その場合は「無効」を UI に反映する
        let ai_denoise_disabled_threshold = match image_dims {
            Some((w, h))
                if !crate::ai::upscale::should_process(w, h, self.settings.ai_denoise_skip_px) =>
            {
                Some(self.settings.ai_denoise_skip_px)
            }
            _ => None,
        };
        let ai_upscale_disabled_threshold = match image_dims {
            Some((w, h))
                if !crate::ai::upscale::should_process(w, h, self.settings.ai_upscale_skip_px) =>
            {
                Some(self.settings.ai_upscale_skip_px)
            }
            _ => None,
        };

        let (changed, is_dragging) = egui::ScrollArea::vertical()
            .max_height(content_rect.height())
            .show(&mut scroll_child, |ui| {
                ui.set_width(panel_rect.width() - 20.0);
                ui.add_space(8.0);
                draw_sliders(
                    ui,
                    &mut edit_params,
                    ai_denoise_disabled_threshold,
                    ai_upscale_disabled_threshold,
                )
            })
            .inner;

        // ドラッグセッションのライフサイクル管理 (slider drag → release で 1 回だけ commit)
        let was_dragging = self.adjustment_dragging;
        self.adjustment_dragging = is_dragging;
        let drag_just_started = is_dragging && !was_dragging;
        if drag_just_started {
            self.adjustment_drag_session = Some(crate::app::AdjustmentDragSession {
                fs_idx,
                before: self.adjustment_page_params.get(&fs_idx).cloned(),
            });
        }
        // セッションが存在するが fs_idx がズレている (= ページ移動した) 場合は破棄。
        // 通常は open_fullscreen での clear_meta_undo が落とすが念のため。
        if let Some(s) = &self.adjustment_drag_session {
            if s.fs_idx != fs_idx {
                self.adjustment_drag_session = None;
            }
        }

        // ── 保存スロット ──
        let slots_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x, panel_rect.max.y - slots_height),
            panel_rect.max,
        );
        let mut slots_child = child.new_child(egui::UiBuilder::new().max_rect(slots_rect));
        let mut save_to_slot: Option<usize> = None;
        let mut load_from_slot: Option<usize> = None;

        slots_child.separator();
        slots_child.add_space(4.0);
        slots_child.label(
            egui::RichText::new("保存スロット")
                .size(11.0)
                .color(LABEL_COLOR),
        );
        slots_child.add_space(2.0);

        let btn_w = (panel_rect.width() - 20.0) * 0.5 - 24.0;
        for row in 0..5 {
            slots_child.horizontal(|ui| {
                for col in 0..2 {
                    let slot_idx = row * 2 + col;
                    let key_label = crate::adjustment::slot_key_label(slot_idx);
                    let slot_name = if let Some(s) = &self.settings.preset_slots.slots[slot_idx] {
                        format!(
                            "{}:{}",
                            key_label,
                            crate::ui_helpers::truncate_name(&s.name, 7)
                        )
                    } else {
                        format!("{}:空", key_label)
                    };
                    let has_data = self.settings.preset_slots.slots[slot_idx].is_some();

                    let name_btn = egui::Button::new(egui::RichText::new(&slot_name).size(10.5))
                        .min_size(egui::vec2(btn_w, 22.0));
                    let name_resp = ui.add_enabled(has_data, name_btn);
                    if name_resp.clicked() {
                        load_from_slot = Some(slot_idx);
                    }
                    if let Some(s) = &self.settings.preset_slots.slots[slot_idx] {
                        name_resp.on_hover_text(format!(
                            "{} をこのページに適用 (Ctrl+{})",
                            s.name, key_label
                        ));
                    }

                    let save_btn = egui::Button::new(egui::RichText::new("💾").size(11.0))
                        .min_size(egui::vec2(22.0, 22.0));
                    let save_resp = ui.add(save_btn);
                    if save_resp.clicked() {
                        save_to_slot = Some(slot_idx);
                    }
                    save_resp.on_hover_text(format!("現在の設定をスロット{}に保存", key_label));
                }
            });
        }

        // ── スライダー変更を反映 (自動的にページ個別化) ──
        // ドラッグ中は **in-memory のみ** 更新し DB / sidecar 書き込みをスキップする
        // (60 frames/sec の DB UPSERT を避ける)。ドラッグ終了時に session で 1 回だけ
        // 永続化 + Undo エントリを積む経路 (下部の `drag_just_ended` ブロック) に流す。
        // ラジオ・コンボボックス・リセット↩ ボタンなどの非ドラッグ変更は即時通常パス。
        if changed {
            let ai_changed = !original.ai_settings_eq(&edit_params);
            if is_dragging {
                self.adjustment_page_params
                    .insert(fs_idx, edit_params.clone());
            } else {
                let before = self
                    .adjustment_drag_session
                    .take()
                    .map(|s| s.before)
                    .unwrap_or_else(|| self.adjustment_page_params.get(&fs_idx).cloned());
                self.set_page_params(fs_idx, edit_params.clone());
                let after = self.adjustment_page_params.get(&fs_idx).cloned();
                self.capture_adjustment_undo(
                    crate::undo_stack::AdjustUndoScope::Page(fs_idx),
                    before,
                    after,
                    "ページ個別の補正".to_string(),
                );
            }
            if ai_changed {
                self.clear_all_adjustment_and_ai_caches(fs_idx);
            } else {
                self.clear_adjustment_caches(fs_idx);
            }
        }

        // ドラッグ終了 (release) フレーム: changed が立たないことが多いので別経路で確定。
        let drag_just_ended = !is_dragging && was_dragging;
        if drag_just_ended {
            if let Some(session) = self.adjustment_drag_session.take() {
                let in_memory = self.adjustment_page_params.get(&fs_idx).cloned();
                if session.before != in_memory {
                    // in-memory に書いた最終値を `set_page_params` で永続化
                    // (matches_default の正規化もここで走る)
                    if let Some(p) = in_memory.clone() {
                        self.set_page_params(fs_idx, p);
                    } else {
                        // ありえない (before != in_memory なのに in_memory が None) が念のため
                        self.clear_page_params(fs_idx);
                    }
                    let after = self.adjustment_page_params.get(&fs_idx).cloned();
                    self.capture_adjustment_undo(
                        crate::undo_stack::AdjustUndoScope::Page(fs_idx),
                        session.before,
                        after,
                        "ページ個別の補正".to_string(),
                    );
                }
            }
        }

        // ── アクションボタン処理 ──
        // バルク系 (apply_all / clear_all) も capture_adjust_full で囲む — 数百ページの
        // 個別設定を一括書き換えする操作だが、ヘルパーが 3 層全体の差分を取るので
        // Vec<AdjustmentChange> として正しく記録される (Codex P2)。
        if apply_all_clicked {
            let params = self.effective_params(fs_idx).clone();
            self.capture_adjust_full("全画像に適用".to_string(), |app| {
                app.apply_params_to_all_pages(params);
            });
            self.show_feedback_toast("全画像に適用".to_string());
        }
        if clear_all_clicked {
            self.capture_adjust_full("全画像の個別設定を削除".to_string(), |app| {
                app.clear_all_page_params();
            });
            self.show_feedback_toast("全画像の個別設定を削除".to_string());
        }
        if set_as_favorite_clicked {
            if let Some((fav_id, fav_name)) = fav_info.clone() {
                let params = self.effective_params(fs_idx).clone();
                let truncated = crate::ui_helpers::truncate_name(&fav_name, 10);
                self.capture_adjust_full(
                    format!("お気に入り「{}」の標準", truncated),
                    |app| app.set_favorite_default(fav_id, params),
                );
                self.show_feedback_toast(format!("お気に入り「{}」の標準を更新", truncated));
            }
        }
        if clear_favorite_clicked {
            if let Some((fav_id, fav_name)) = fav_info.clone() {
                let truncated = crate::ui_helpers::truncate_name(&fav_name, 10);
                self.capture_adjust_full(
                    format!("お気に入り「{}」の標準を解除", truncated),
                    |app| app.clear_favorite_default(fav_id),
                );
                self.show_feedback_toast(format!("お気に入り「{}」の標準を解除", truncated));
            }
        }
        if set_as_global_clicked {
            let params = self.effective_params(fs_idx).clone();
            self.capture_adjust_full("標準設定の更新".to_string(), |app| {
                app.copy_params_to_global(params)
            });
            self.show_feedback_toast("標準設定を更新".to_string());
        }
        if clear_page_clicked {
            self.capture_adjust_full("個別設定の解除".to_string(), |app| {
                app.clear_page_params(fs_idx)
            });
            self.show_feedback_toast("個別設定を解除".to_string());
        }

        // ── 保存スロット: ダイアログで名称を入力 ──
        if let Some(slot_idx) = save_to_slot {
            // 既存スロットがあればその名前を初期値に、なければ空で開く
            let default_name = self.settings.preset_slots.slots[slot_idx]
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            // 保存対象の補正値はクリック時点で確定 (見開き L/R 切替やスライダー操作で揺れない)
            let params = self.effective_params(fs_idx).clone();
            self.slot_save_dialog = Some((slot_idx, default_name, params));
        }
        if let Some(slot_idx) = load_from_slot {
            self.capture_adjust_full(
                format!(
                    "スロット{}を適用",
                    crate::adjustment::slot_key_label(slot_idx)
                ),
                |app| app.apply_slot_to_idx(slot_idx, fs_idx),
            );
        }
    }

    /// スロット保存ダイアログを描画する。`slot_save_dialog` が Some の間だけ表示。
    pub(crate) fn draw_slot_save_dialog(&mut self, ctx: &egui::Context) {
        let Some((slot_idx, mut name_input, params)) = self.slot_save_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut confirmed = false;
        let mut canceled = false;
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("保存スロット名")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let key_label = crate::adjustment::slot_key_label(slot_idx);
                ui.label(format!("スロット {} に保存する名前を入力:", key_label));
                ui.add_space(4.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut name_input)
                        .desired_width(240.0)
                        .hint_text("例: 漫画モノクロ / スキャン補正"),
                );
                if !resp.has_focus() && !resp.lost_focus() {
                    resp.request_focus();
                }
                if resp.lost_focus() && enter_pressed {
                    confirmed = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!name_input.trim().is_empty(), egui::Button::new("保存"))
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        canceled = true;
                    }
                });
                if escape_pressed {
                    canceled = true;
                }
            });

        if !open || canceled {
            // ダイアログを閉じる (State を Some に戻さない)
            return;
        }

        if confirmed && !name_input.trim().is_empty() {
            self.settings.preset_slots.slots[slot_idx] = Some(PresetSlot {
                name: name_input.trim().to_string(),
                params,
            });
            self.settings.save();
            let key_label = crate::adjustment::slot_key_label(slot_idx);
            self.show_feedback_toast(format!(
                "[スロット{}:{} に保存]",
                key_label,
                name_input.trim()
            ));
            return;
        }

        // まだ開いている → state を書き戻す
        self.slot_save_dialog = Some((slot_idx, name_input, params));
    }
}
