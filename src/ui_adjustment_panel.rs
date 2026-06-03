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
use crate::local_adjust_catalog::{
    EFFECT_GROUPS, EffectKind, effect_picker_button_width, effect_picker_matches_query,
};
use crate::local_adjust_effect_ui::draw_effect_params;
use crate::ui_fullscreen::SpreadPair;

const HEADER_H: f32 = 36.0;
const SECTION_FONT: f32 = 12.0;
/// ラベルの色（暗い背景で読みやすい白系）
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);

/// 左パネルの幅
pub const LEFT_PANEL_WIDTH: f32 = 260.0;
/// 左パネルの下端をウィンドウ下端から少し浮かせる余白。
pub const LEFT_PANEL_BOTTOM_MARGIN: f32 = 20.0;
/// 補正本文の左余白。画面端に文字が張り付かないようにする。
const BODY_PAD_LEFT: f32 = 10.0;
/// 補正本文の右余白。スクロールバーと保存スロットボタンの干渉を避ける。
const BODY_PAD_RIGHT: f32 = 10.0;
/// ScrollArea の縦バーが重なる分として、本文 widget 幅から差し引く余白。
const BODY_SCROLLBAR_RESERVE: f32 = 14.0;
/// 補正レイヤー独立パネルの幅。消しゴム / 隠蔽加工パネルと揃える。
const LOCAL_ADJUST_PANEL_W: f32 = 200.0;
const LOCAL_ADJUST_PANEL_MARGIN_X: f32 = 16.0;
const LOCAL_ADJUST_PANEL_MARGIN_Y: f32 = 60.0;
const LOCAL_ADJUST_PANEL_BOTTOM_MARGIN: f32 = 20.0;
const LOCAL_ADJUST_PANEL_MIN_BODY_H: f32 = 120.0;

#[derive(Debug, Clone, Copy)]
enum QuickLocalAdjustEffect {
    Vibrance,
    WarmPhotoFilter,
    MonochromeMixer,
    SoftBlur,
    Sharpen,
    Vignette,
    CrtDisplay,
    Anaglyph3d,
}

#[derive(Default)]
struct LocalEffectPanelRequests {
    load_cube_lut: Option<usize>,
    copy_effect: Option<usize>,
    paste_effect: Option<usize>,
    reset_effect: Option<usize>,
    start_selective_color_pick: bool,
    cancel_selective_color_pick: bool,
    start_rgb_pick: Option<crate::local_adjust_effect_ui::RgbPickTarget>,
    cancel_rgb_pick: bool,
    set_effect_position_handles_visible: Option<bool>,
}

impl QuickLocalAdjustEffect {
    const ALL: [Self; 8] = [
        Self::Vibrance,
        Self::WarmPhotoFilter,
        Self::MonochromeMixer,
        Self::SoftBlur,
        Self::Sharpen,
        Self::Vignette,
        Self::CrtDisplay,
        Self::Anaglyph3d,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Vibrance => "自然な彩度",
            Self::WarmPhotoFilter => "暖色フィルター",
            Self::MonochromeMixer => "白黒ミキサー",
            Self::SoftBlur => "ソフトぼかし",
            Self::Sharpen => "シャープ",
            Self::Vignette => "ビネット",
            Self::CrtDisplay => "CRT表示",
            Self::Anaglyph3d => "アナグリフ3D",
        }
    }

    fn effect(self) -> local_adjust_core::LocalEffect {
        match self {
            Self::Vibrance => local_adjust_core::LocalEffect::Tone(local_adjust_core::ToneParams {
                vibrance: 55.0,
                saturation: 8.0,
                ..Default::default()
            }),
            Self::WarmPhotoFilter => {
                local_adjust_core::LocalEffect::PhotoFilter(local_adjust_core::PhotoFilterParams {
                    preset: local_adjust_core::PhotoFilterPreset::Warm85,
                    density: 0.45,
                    preserve_luminosity: true,
                    strength: 1.0,
                    ..Default::default()
                })
            }
            Self::MonochromeMixer => local_adjust_core::LocalEffect::MonochromeMixer(
                local_adjust_core::MonochromeMixerParams {
                    red: 10.0,
                    yellow: 18.0,
                    green: 12.0,
                    cyan: 0.0,
                    blue: -8.0,
                    magenta: 0.0,
                    contrast: 14.0,
                    strength: 1.0,
                    ..Default::default()
                },
            ),
            Self::SoftBlur => local_adjust_core::LocalEffect::Blur(local_adjust_core::BlurParams {
                radius_px: 4.0,
            }),
            Self::Sharpen => {
                local_adjust_core::LocalEffect::Sharpen(local_adjust_core::SharpenParams {
                    amount: 1.0,
                    radius_px: 1.0,
                    threshold: 0.02,
                })
            }
            Self::Vignette => {
                local_adjust_core::LocalEffect::Vignette(local_adjust_core::VignetteParams {
                    strength: 0.55,
                    radius: 0.58,
                    feather: 0.38,
                })
            }
            Self::CrtDisplay => local_adjust_core::LocalEffect::CrtDisplay(
                local_adjust_core::CrtDisplayParams::preset(
                    local_adjust_core::CrtDisplayMode::Simple,
                ),
            ),
            Self::Anaglyph3d => {
                local_adjust_core::LocalEffect::Anaglyph3d(local_adjust_core::AnaglyphParams {
                    strength: 0.85,
                    ..Default::default()
                })
            }
        }
    }

    fn layer(self) -> local_adjust_core::LocalAdjustmentLayer {
        local_adjust_core::LocalAdjustmentLayer::new(
            self.label(),
            local_adjust_core::LocalMask::Full,
            self.effect(),
        )
    }
}

fn draw_local_adjust_section(
    ui: &mut egui::Ui,
    content_width: f32,
    layers: &[local_adjust_core::LocalAdjustmentLayer],
    selected_layer: usize,
    image_dims: (usize, usize),
    effect_query: &mut String,
    add_quick_effect: &mut Option<QuickLocalAdjustEffect>,
    add_effect: &mut Option<EffectKind>,
    select_layer: &mut Option<usize>,
    set_enabled: &mut Option<(usize, bool)>,
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    delete_layer: &mut Option<usize>,
    clear_layers: &mut bool,
    effect_clipboard_available: bool,
    selective_color_pick_active: bool,
    rgb_pick_active: Option<crate::local_adjust_effect_ui::RgbPickTarget>,
    effect_position_handles_visible: bool,
    effect_requests: &mut LocalEffectPanelRequests,
) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("よく使う")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for effect in QuickLocalAdjustEffect::ALL {
            if ui
                .small_button(effect.label())
                .on_hover_text("画像全体に適用する補正レイヤーを追加")
                .clicked()
            {
                *add_quick_effect = Some(effect);
            }
        }
    });
    ui.add_space(8.0);

    ui.label(
        egui::RichText::new("効果を追加")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2((content_width - 34.0).max(80.0), 22.0),
            egui::TextEdit::singleline(effect_query)
                .hint_text("効果名で検索")
                .desired_width(f32::INFINITY),
        );
        if ui
            .add_enabled(!effect_query.is_empty(), egui::Button::new("×"))
            .on_hover_text("検索をクリア")
            .clicked()
        {
            effect_query.clear();
        }
    });
    let button_width = effect_picker_button_width(content_width);
    let query = effect_query.trim();
    let mut any_effect = false;
    for group in EFFECT_GROUPS {
        let matched: Vec<EffectKind> = group
            .kinds
            .iter()
            .copied()
            .filter(|kind| effect_picker_matches_query(*kind, query))
            .collect();
        if matched.is_empty() {
            continue;
        }
        any_effect = true;
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(group.title)
                .size(11.0)
                .color(egui::Color32::from_gray(170)),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for kind in matched {
                let resp = ui.add_sized(
                    egui::vec2(button_width, 22.0),
                    egui::Button::new(egui::RichText::new(kind.picker_label()).size(11.0)),
                );
                let resp = resp.on_hover_text(kind.description());
                if resp.clicked() {
                    *add_effect = Some(kind);
                }
            }
        });
    }
    if !any_effect {
        ui.label(
            egui::RichText::new("該当する効果がありません")
                .size(11.0)
                .color(egui::Color32::from_gray(160)),
        );
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("レイヤー")
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );

    if layers.is_empty() {
        ui.label(
            egui::RichText::new("このページには補正レイヤーがありません")
                .size(11.0)
                .color(egui::Color32::from_gray(170)),
        );
        return;
    }

    for (layer_idx, layer) in layers.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut enabled = layer.enabled;
            if ui.checkbox(&mut enabled, "").changed() {
                *set_enabled = Some((layer_idx, enabled));
            }
            let name = crate::ui_helpers::truncate_name(&layer.name, 12);
            let label = format!("{}: {}", layer_idx + 1, name);
            let label_resp = ui.add_sized(
                egui::vec2((content_width - 68.0).max(80.0), 18.0),
                egui::Button::selectable(
                    layer_idx == selected_layer,
                    egui::RichText::new(label)
                        .size(11.0)
                        .color(egui::Color32::from_gray(220)),
                ),
            );
            if label_resp.clicked() {
                *select_layer = Some(layer_idx);
            }
            label_resp.on_hover_text(format!(
                "{} / opacity {:.0}%",
                layer.effect.display_label(),
                layer.opacity * 100.0
            ));
            if ui.small_button("削除").clicked() {
                *delete_layer = Some(layer_idx);
            }
        });
    }

    if let Some(layer) = layers.get(selected_layer) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        draw_selected_local_adjust_layer_editor(
            ui,
            selected_layer,
            layer,
            image_dims,
            update_layer,
            effect_clipboard_available,
            selective_color_pick_active,
            rgb_pick_active,
            effect_position_handles_visible,
            effect_requests,
        );
    }

    if ui
        .add(
            egui::Button::new("補正レイヤーをすべて削除").min_size(egui::vec2(content_width, 22.0)),
        )
        .on_hover_text("このページの補正レイヤーをすべて削除")
        .clicked()
    {
        *clear_layers = true;
    }
}

fn draw_selected_local_adjust_layer_editor(
    ui: &mut egui::Ui,
    layer_idx: usize,
    layer: &local_adjust_core::LocalAdjustmentLayer,
    image_dims: (usize, usize),
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    effect_clipboard_available: bool,
    selective_color_pick_active: bool,
    rgb_pick_active: Option<crate::local_adjust_effect_ui::RgbPickTarget>,
    effect_position_handles_visible: bool,
    effect_requests: &mut LocalEffectPanelRequests,
) {
    let mut edited = layer.clone();
    let mut changed = false;
    ui.label(
        egui::RichText::new(format!("選択中: {}", edited.name))
            .size(SECTION_FONT)
            .color(LABEL_COLOR),
    );
    changed |= ui
        .add(
            egui::Slider::new(&mut edited.opacity, 0.0..=1.0)
                .text("不透明度")
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
        )
        .changed();
    let effect_kind = EffectKind::from_effect(&edited.effect);
    ui.label(
        egui::RichText::new(effect_kind.label())
            .size(11.0)
            .color(egui::Color32::from_gray(180)),
    );
    let response = draw_effect_params(
        ui,
        &mut edited,
        image_dims,
        selective_color_pick_active,
        rgb_pick_active,
        effect_clipboard_available,
        effect_position_handles_visible,
    );
    changed |= response.changed;
    if response.load_cube_lut {
        effect_requests.load_cube_lut = Some(layer_idx);
    }
    if response.copy_effect {
        effect_requests.copy_effect = Some(layer_idx);
    }
    if response.paste_effect {
        effect_requests.paste_effect = Some(layer_idx);
    }
    if response.reset_effect {
        effect_requests.reset_effect = Some(layer_idx);
    }
    effect_requests.start_selective_color_pick |= response.start_selective_color_pick;
    effect_requests.cancel_selective_color_pick |= response.cancel_selective_color_pick;
    if response.start_rgb_pick.is_some() {
        effect_requests.start_rgb_pick = response.start_rgb_pick;
    }
    effect_requests.cancel_rgb_pick |= response.cancel_rgb_pick;
    if response.set_effect_position_handles_visible.is_some() {
        effect_requests.set_effect_position_handles_visible =
            response.set_effect_position_handles_visible;
    }
    if changed {
        *update_layer = Some((layer_idx, edited));
    }
}

fn local_adjust_image_dims(app: &App, fs_idx: usize) -> (usize, usize) {
    match app.fs_cache.get(&fs_idx) {
        Some(crate::fs_animation::FsCacheEntry::Static {
            pixels,
            source_dims,
            ..
        }) => source_dims
            .map(|[w, h]| (w, h))
            .unwrap_or((pixels.size[0], pixels.size[1])),
        _ => (1, 1),
    }
}

fn paste_layer_effect(
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    effect: local_adjust_core::LocalEffect,
) {
    let current_kind = EffectKind::from_effect(&layer.effect);
    let pasted_kind = EffectKind::from_effect(&effect);
    layer.effect = effect;
    if current_kind != pasted_kind {
        let mask_application =
            local_adjust_core::default_mask_application_for_effect(&layer.effect);
        layer.mask_before_effect = mask_application.before_effect;
        layer.mask_after_effect = mask_application.after_effect;
    }
}

fn reset_layer_effect_params(layer: &mut local_adjust_core::LocalAdjustmentLayer) -> bool {
    let kind = EffectKind::from_effect(&layer.effect);
    if kind == EffectKind::None {
        return false;
    }
    layer.effect = kind.default_effect();
    true
}

fn draw_header_icon_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &'static str,
    enabled: bool,
    active: bool,
    tooltip: &str,
    icon_fn: impl FnOnce(&egui::Painter, egui::Pos2, f32),
) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    let bg = if !enabled {
        egui::Color32::from_rgba_unmultiplied(50, 50, 50, 120)
    } else if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 220)
    } else if resp.hovered() {
        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 220)
    } else {
        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    icon_fn(ui.painter(), rect.center(), rect.width() * 0.28);
    if enabled {
        resp.on_hover_text(tooltip)
    } else {
        resp.on_hover_text("画像を開いているときのみ使用できます")
    }
}

fn local_adjust_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.max.y - panel_pos.y - LOCAL_ADJUST_PANEL_BOTTOM_MARGIN)
        .max(LOCAL_ADJUST_PANEL_MIN_BODY_H + 40.0)
}

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
    /// 補正レイヤー独立パネルの矩形を返す。
    pub(crate) fn local_adjust_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(
            full_rect.min.x + LOCAL_ADJUST_PANEL_MARGIN_X,
            full_rect.min.y + LOCAL_ADJUST_PANEL_MARGIN_Y,
        );
        let h = local_adjust_panel_outer_height(full_rect, panel_pos);
        egui::Rect::from_min_size(panel_pos, egui::vec2(LOCAL_ADJUST_PANEL_W, h))
    }

    fn apply_local_adjust_panel_actions(
        &mut self,
        fs_idx: usize,
        add_quick_effect: Option<QuickLocalAdjustEffect>,
        add_effect: Option<EffectKind>,
        select_layer: Option<usize>,
        set_enabled: Option<(usize, bool)>,
        update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
        delete_layer: Option<usize>,
        clear_layers: bool,
        effect_requests: LocalEffectPanelRequests,
    ) {
        if let Some(layer_idx) = select_layer {
            if let Some(layers) = self.local_adjust_page_layers.get(&fs_idx)
                && layer_idx < layers.len()
            {
                self.local_adjust_selected_layers.insert(fs_idx, layer_idx);
            }
        }

        let mut layers = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        let mut selected_after: Option<usize> = None;

        if let Some(effect) = add_quick_effect {
            layers.push(effect.layer());
            selected_after = Some(layers.len().saturating_sub(1));
            changed = true;
            self.show_feedback_toast(format!("補正レイヤーを追加: {}", effect.label()));
        }
        if let Some(kind) = add_effect {
            layers.push(kind.layer());
            selected_after = Some(layers.len().saturating_sub(1));
            changed = true;
            self.show_feedback_toast(format!("補正レイヤーを追加: {}", kind.label()));
        }
        if let Some((layer_idx, enabled)) = set_enabled {
            if let Some(layer) = layers.get_mut(layer_idx) {
                layer.enabled = enabled;
                changed = true;
                self.show_feedback_toast(if enabled {
                    "補正レイヤーを有効化".to_string()
                } else {
                    "補正レイヤーを無効化".to_string()
                });
            }
        }
        if let Some((layer_idx, layer)) = update_layer
            && layer_idx < layers.len()
        {
            layers[layer_idx] = layer;
            selected_after = Some(layer_idx);
            changed = true;
        }
        if let Some(layer_idx) = effect_requests.copy_effect
            && let Some(layer) = layers.get(layer_idx)
        {
            let kind = EffectKind::from_effect(&layer.effect);
            self.local_adjust_effect_clipboard = Some(layer.effect.clone());
            self.show_feedback_toast(format!("加工パラメータをコピー: {}", kind.label()));
        }
        if let Some(layer_idx) = effect_requests.paste_effect {
            if let Some(effect) = self.local_adjust_effect_clipboard.clone() {
                if let Some(layer) = layers.get_mut(layer_idx) {
                    let kind = EffectKind::from_effect(&effect);
                    paste_layer_effect(layer, effect);
                    selected_after = Some(layer_idx);
                    changed = true;
                    self.local_adjust_selective_color_pick_active = false;
                    self.local_adjust_rgb_pick_active = None;
                    self.show_feedback_toast(format!("加工パラメータをペースト: {}", kind.label()));
                }
            } else {
                self.show_feedback_toast("コピー済みの加工パラメータがありません".to_string());
            }
        }
        if let Some(layer_idx) = effect_requests.reset_effect
            && let Some(layer) = layers.get_mut(layer_idx)
        {
            let kind = EffectKind::from_effect(&layer.effect);
            if reset_layer_effect_params(layer) {
                selected_after = Some(layer_idx);
                changed = true;
                self.local_adjust_selective_color_pick_active = false;
                self.local_adjust_rgb_pick_active = None;
                self.show_feedback_toast(format!("加工パラメータをリセット: {}", kind.label()));
            }
        }
        if let Some(layer_idx) = delete_layer
            && layer_idx < layers.len()
        {
            layers.remove(layer_idx);
            selected_after = Some(layer_idx.min(layers.len().saturating_sub(1)));
            changed = true;
            self.show_feedback_toast("補正レイヤーを削除".to_string());
        }
        if clear_layers {
            layers.clear();
            changed = true;
            self.show_feedback_toast("補正レイヤーをすべて削除".to_string());
        }
        if changed {
            self.set_local_adjust_layers_for_idx(fs_idx, layers);
            if let Some(selected) = selected_after {
                if self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .is_some_and(|layers| selected < layers.len())
                {
                    self.local_adjust_selected_layers.insert(fs_idx, selected);
                }
            }
        }
        if let Some(visible) = effect_requests.set_effect_position_handles_visible {
            self.local_adjust_effect_position_handles_visible = visible;
        }
        if effect_requests.cancel_selective_color_pick {
            self.local_adjust_selective_color_pick_active = false;
        }
        if effect_requests.start_selective_color_pick {
            self.local_adjust_selective_color_pick_active = true;
            self.local_adjust_rgb_pick_active = None;
            self.show_feedback_toast("画像上の色をクリックして対象色を選択します".to_string());
        }
        if effect_requests.cancel_rgb_pick {
            self.local_adjust_rgb_pick_active = None;
        }
        if let Some(target) = effect_requests.start_rgb_pick {
            self.local_adjust_rgb_pick_active = Some(target);
            self.local_adjust_selective_color_pick_active = false;
            self.show_feedback_toast(format!("スポイト対象: {}", target.label()));
        }
        if let Some(layer_idx) = effect_requests.load_cube_lut {
            self.choose_local_adjust_cube_lut_for_layer(fs_idx, layer_idx);
        }
    }

    fn choose_local_adjust_cube_lut_for_layer(&mut self, fs_idx: usize, layer_idx: usize) {
        if self.local_adjust_lut_pending.is_some() {
            self.show_feedback_toast("LUT読み込み中です".to_string());
            return;
        }
        if !self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .is_some_and(|layer| matches!(layer.effect, local_adjust_core::LocalEffect::CubeLut(_)))
        {
            self.show_feedback_toast("3D LUT レイヤーを選択してください".to_string());
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("3D LUT (.cube)", &["cube"])
            .set_title("3D LUTを選択")
            .pick_file()
        else {
            return;
        };
        let fallback_name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("3D LUT")
            .to_string();
        let worker_path = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lut-load".to_string())
            .spawn(move || {
                let result = std::fs::read_to_string(&worker_path)
                    .map_err(|err| format!("LUTファイルを読めません: {err}"))
                    .and_then(|text| local_adjust_core::parse_cube_lut(&text, &fallback_name));
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.local_adjust_lut_pending = Some(crate::app::LocalAdjustLutLoadPending {
                    fs_idx,
                    layer_idx,
                    generation: self
                        .local_adjust_generation
                        .get(&fs_idx)
                        .copied()
                        .unwrap_or(0),
                    path: path.clone(),
                    rx,
                });
                self.show_feedback_toast(format!("LUT読み込み中: {}", path.display()));
            }
            Err(err) => {
                self.show_feedback_toast(format!("LUT読み込み worker 起動失敗: {err}"));
            }
        }
    }

    pub(crate) fn poll_local_adjust_lut_load(&mut self, ctx: &egui::Context) {
        let recv_result = {
            let Some(pending) = self.local_adjust_lut_pending.as_ref() else {
                return;
            };
            pending.rx.try_recv()
        };
        match recv_result {
            Ok(Ok(params)) => {
                let Some(pending) = self.local_adjust_lut_pending.take() else {
                    return;
                };
                let current_generation = self
                    .local_adjust_generation
                    .get(&pending.fs_idx)
                    .copied()
                    .unwrap_or(0);
                if pending.generation != current_generation {
                    self.show_feedback_toast(
                        "LUT読み込み結果を破棄しました。レイヤーが変更されています".to_string(),
                    );
                    return;
                }
                let mut layers = self
                    .local_adjust_page_layers
                    .get(&pending.fs_idx)
                    .cloned()
                    .unwrap_or_default();
                if !layers.get(pending.layer_idx).is_some_and(|layer| {
                    matches!(layer.effect, local_adjust_core::LocalEffect::CubeLut(_))
                }) {
                    self.show_feedback_toast(
                        "LUT読み込み結果を破棄しました。対象レイヤーが変更されています".to_string(),
                    );
                    return;
                }
                let name = params.name.clone();
                let size = params.size;
                layers[pending.layer_idx].effect = local_adjust_core::LocalEffect::CubeLut(params);
                self.local_adjust_selected_layers
                    .insert(pending.fs_idx, pending.layer_idx);
                self.set_local_adjust_layers_for_idx(pending.fs_idx, layers);
                self.show_feedback_toast(format!(
                    "LUT読み込み完了: {name} ({size}^3) / {}",
                    pending.path.display()
                ));
                ctx.request_repaint();
            }
            Ok(Err(err)) => {
                self.local_adjust_lut_pending = None;
                self.show_feedback_toast(format!("LUT読み込み失敗: {err}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.local_adjust_lut_pending = None;
                self.show_feedback_toast("LUT読み込み worker が停止しました".to_string());
            }
        }
    }

    /// 補正レイヤーの独立左パネルを描画する。
    pub(crate) fn draw_local_adjust_panel(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        if !self.local_adjust_mode {
            return;
        }
        let Some(fs_root_idx) = self.fullscreen_idx else {
            return;
        };

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
        let layers = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let selected_layer = self
            .local_adjust_selected_layers
            .get(&fs_idx)
            .copied()
            .unwrap_or(0)
            .min(layers.len().saturating_sub(1));
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let mut effect_query = self.local_adjust_effect_query.clone();
        let effect_clipboard_available = self.local_adjust_effect_clipboard.is_some();
        let selective_color_pick_active = self.local_adjust_selective_color_pick_active;
        let rgb_pick_active = self.local_adjust_rgb_pick_active;
        let effect_position_handles_visible = self.local_adjust_effect_position_handles_visible;

        let panel_pos = egui::pos2(
            full_rect.min.x + LOCAL_ADJUST_PANEL_MARGIN_X,
            full_rect.min.y + LOCAL_ADJUST_PANEL_MARGIN_Y,
        );
        let panel_h = local_adjust_panel_outer_height(full_rect, panel_pos);
        let sink_rect = egui::Rect::from_min_size(
            panel_pos,
            egui::vec2(LOCAL_ADJUST_PANEL_W + 4.0, panel_h + 8.0),
        );

        let mut close_clicked = false;
        let mut add_quick_effect: Option<QuickLocalAdjustEffect> = None;
        let mut add_effect: Option<EffectKind> = None;
        let mut select_layer: Option<usize> = None;
        let mut set_enabled: Option<(usize, bool)> = None;
        let mut update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)> = None;
        let mut delete_layer: Option<usize> = None;
        let mut clear_layers = false;
        let mut effect_requests = LocalEffectPanelRequests::default();

        egui::Area::new(egui::Id::new("local_adjust_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    sink_rect,
                    egui::Id::new("local_adjust_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(LOCAL_ADJUST_PANEL_W);
                        ui.set_max_width(LOCAL_ADJUST_PANEL_W);
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("補正レイヤー")
                                    .size(15.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let (close_rect, close_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    let close_bg = if close_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(220, 80, 80, 200)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
                                    };
                                    ui.painter().rect_filled(close_rect, 4.0, close_bg);
                                    crate::ui_fullscreen::draw_icons::draw_close_icon(
                                        ui.painter(),
                                        close_rect.center(),
                                        8.0,
                                    );
                                    if close_resp.clicked() {
                                        close_clicked = true;
                                    }
                                    close_resp.on_hover_text("閉じる");
                                },
                            );
                        });
                        ui.separator();

                        let body_height = (full_rect.max.y
                            - ui.cursor().top()
                            - LOCAL_ADJUST_PANEL_BOTTOM_MARGIN)
                            .max(LOCAL_ADJUST_PANEL_MIN_BODY_H);
                        ui.allocate_ui_with_layout(
                            egui::vec2(LOCAL_ADJUST_PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_width(LOCAL_ADJUST_PANEL_W);
                                ui.set_max_width(LOCAL_ADJUST_PANEL_W);
                                ui.set_min_height(body_height);
                                egui::ScrollArea::vertical()
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_min_width(LOCAL_ADJUST_PANEL_W);
                                        ui.set_max_width(LOCAL_ADJUST_PANEL_W);

                                        if let Some((left, right)) = spread_lr {
                                            ui.horizontal(|ui| {
                                                let is_left = self.adjust_spread_target
                                                    == AdjustSpreadTarget::Left;
                                                if ui
                                                    .selectable_label(is_left, "左ページ")
                                                    .clicked()
                                                {
                                                    self.adjust_spread_target =
                                                        AdjustSpreadTarget::Left;
                                                }
                                                if ui
                                                    .selectable_label(!is_left, "右ページ")
                                                    .clicked()
                                                {
                                                    self.adjust_spread_target =
                                                        AdjustSpreadTarget::Right;
                                                }
                                            });
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "対象: {}",
                                                    if fs_idx == left {
                                                        "左ページ"
                                                    } else if fs_idx == right {
                                                        "右ページ"
                                                    } else {
                                                        "現在ページ"
                                                    }
                                                ))
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(170)),
                                            );
                                            ui.add_space(4.0);
                                        }

                                        draw_local_adjust_section(
                                            ui,
                                            LOCAL_ADJUST_PANEL_W,
                                            &layers,
                                            selected_layer,
                                            image_dims,
                                            &mut effect_query,
                                            &mut add_quick_effect,
                                            &mut add_effect,
                                            &mut select_layer,
                                            &mut set_enabled,
                                            &mut update_layer,
                                            &mut delete_layer,
                                            &mut clear_layers,
                                            effect_clipboard_available,
                                            selective_color_pick_active,
                                            rgb_pick_active,
                                            effect_position_handles_visible,
                                            &mut effect_requests,
                                        );
                                    });
                            },
                        );
                    });
            });

        if close_clicked {
            self.local_adjust_mode = false;
        }
        self.local_adjust_effect_query = effect_query;
        self.apply_local_adjust_panel_actions(
            fs_idx,
            add_quick_effect,
            add_effect,
            select_layer,
            set_enabled,
            update_layer,
            delete_layer,
            clear_layers,
            effect_requests,
        );
    }

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
        // ⚠ テーマ非依存で常に DARK visuals を適用。Light テーマで slider/DragValue
        // の bg が near-white になり「白の上に白文字」で読めなくなる問題への対応
        // (= 消しゴム / 隠蔽パネルと同じ方針、CLAUDE.md「フルスクリーン内は黒背景
        // ベース統一」)。実機 FB R4 で配色統一の要望があった。
        *child.visuals_mut() = egui::Visuals::dark();
        child.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

        // ── ヘッダー ──
        // タイトルを左寄せにし、右側に処理順の入口
        // (消しゴム / 補正レイヤー / 隠蔽加工 / エクスポート) を並べる。
        // 補正レイヤーは消しゴム / 隠蔽加工と同じ独立左パネルとして開き、
        // 他 3 つは既存の編集モード / 書き出しダイアログへ合流する。
        let header_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), HEADER_H));
        const HEADER_BTN_SIZE: f32 = 28.0;
        const HEADER_BTN_GAP: f32 = 4.0;
        const HEADER_RIGHT_PAD: f32 = 8.0;
        // タイトル左寄せ (本文と同じ左余白、CENTER_Y 縦中央)
        child.painter().text(
            egui::pos2(header_rect.min.x + BODY_PAD_LEFT, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "画像補正",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
        // 起動可能か (= 画像のみ。動画 / セパレータ / コンテナ は無効化)。
        // `image_dims` が None なら未ロード / 非画像なので無効。
        let can_overlay_edit = image_dims.is_some()
            && matches!(
                self.items.get(fs_idx),
                Some(
                    crate::grid_item::GridItem::Image(_)
                        | crate::grid_item::GridItem::ZipImage { .. }
                        | crate::grid_item::GridItem::PdfPage { .. }
                )
            );
        // 右側 4 ボタン。左から 消しゴム / 補正レイヤー / 隠蔽加工 / エクスポート。
        let btn_y = header_rect.center().y - HEADER_BTN_SIZE / 2.0;
        let export_btn_x = header_rect.max.x - HEADER_RIGHT_PAD - HEADER_BTN_SIZE;
        let conceal_btn_x = export_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
        let local_adjust_btn_x = conceal_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
        let erase_btn_x = local_adjust_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
        let erase_rect = egui::Rect::from_min_size(
            egui::pos2(erase_btn_x, btn_y),
            egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
        );
        let local_adjust_rect = egui::Rect::from_min_size(
            egui::pos2(local_adjust_btn_x, btn_y),
            egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
        );
        let conceal_rect = egui::Rect::from_min_size(
            egui::pos2(conceal_btn_x, btn_y),
            egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
        );
        let export_rect = egui::Rect::from_min_size(
            egui::pos2(export_btn_x, btn_y),
            egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
        );
        let mut activate_erase = false;
        let mut activate_local_adjust = false;
        let mut activate_conceal = false;
        let mut activate_export = false;

        let erase_resp = draw_header_icon_button(
            &mut child,
            erase_rect,
            "adjust_panel_erase_btn",
            can_overlay_edit,
            false,
            "消しゴム (E)",
            crate::ui_fullscreen::draw_icons::draw_eraser_icon,
        );
        if can_overlay_edit && erase_resp.clicked() {
            activate_erase = true;
        }

        let local_adjust_resp = draw_header_icon_button(
            &mut child,
            local_adjust_rect,
            "adjust_panel_local_adjust_btn",
            can_overlay_edit,
            false,
            "補正レイヤー",
            crate::ui_fullscreen::draw_icons::draw_local_adjust_icon,
        );
        if can_overlay_edit && local_adjust_resp.clicked() {
            activate_local_adjust = true;
        }

        let conceal_resp = draw_header_icon_button(
            &mut child,
            conceal_rect,
            "adjust_panel_conceal_btn",
            can_overlay_edit,
            false,
            "隠蔽加工 (Ctrl+M)",
            crate::ui_fullscreen::draw_icons::draw_mosaic_icon,
        );
        if can_overlay_edit && conceal_resp.clicked() {
            activate_conceal = true;
        }

        let export_resp = draw_header_icon_button(
            &mut child,
            export_rect,
            "adjust_panel_export_btn",
            can_overlay_edit,
            false,
            "エクスポート (Ctrl+E)",
            crate::ui_fullscreen::draw_icons::draw_export_icon,
        );
        if can_overlay_edit && export_resp.clicked() {
            activate_export = true;
        }
        // クリック処理は描画後にディスパッチ (借用衝突回避)。
        // 補正パネルは「ホバーで自動閉じる」モードなので、消しゴム / 隠蔽に入る前に
        // adjustment_mode を倒しておく (enter_*_mode 内のガード `!self.adjustment_mode`
        // と整合させるためにも必要)。`enter_*_mode` 自身が必要なキャッシュ初期化と
        // post_filter バイパスを行うので、ここでは flag を倒すだけで十分。
        if activate_local_adjust {
            self.adjustment_mode = false;
            self.local_adjust_mode = true;
            return;
        }
        if activate_erase {
            self.adjustment_mode = false;
            self.enter_erase_mode(fs_root_idx);
            return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
        }
        if activate_conceal {
            self.adjustment_mode = false;
            self.enter_conceal_mode(fs_root_idx);
            return;
        }
        if activate_export {
            // エクスポートは編集モード中 (adjustment / erase / conceal / analysis) は
            // 弾かれる。補正パネルはホバーで出る自動オーバーレイなので、ここで閉じて
            // ビューモードへ戻すことで「消しゴム / 隠蔽 / モザイクまで合成した最終結果」を
            // 書き出せる状態にしてからダイアログを開く (Ctrl+E と同一経路)。
            self.adjustment_mode = false;
            let ctx = child.ctx().clone();
            self.open_export_dialog_for_current(&ctx, fs_root_idx);
            return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
        }

        // ── R5: パネル body 全体を 1 つの ScrollArea で囲む ──
        // 旧版は spread セレクタ / scope text / action buttons / 保存スロットを
        // **絶対位置**で配置し、中央スライダー領域だけが ScrollArea になっていた。
        // そのため「ウィンドウ縦幅が狭いと action buttons / 保存スロットが下端に
        // 沈んで触れない」「補正パネルだけ全体スクロールが効かない」状態だった
        // (実機 FB R5: 「画像補正パネルはまだ中央部分だけスクロールします」)。
        //
        // 新方針: ヘッダ (HEADER_H = 36px) は絶対位置で固定し、それより下を 1 つの
        // ScrollArea でフロー配置する。
        let body_rect = egui::Rect::from_min_max(
            egui::pos2(
                panel_rect.min.x + BODY_PAD_LEFT,
                panel_rect.min.y + HEADER_H,
            ),
            egui::pos2(panel_rect.max.x - BODY_PAD_RIGHT, panel_rect.max.y),
        );
        let mut body_child = child.new_child(egui::UiBuilder::new().max_rect(body_rect));
        let body_height = body_rect.height();
        let body_width = body_rect.width();
        let content_width = (body_width - BODY_SCROLLBAR_RESERVE).max(120.0);

        let mut apply_all_clicked = false;
        let mut clear_all_clicked = false;
        let mut set_as_favorite_clicked = false;
        let mut clear_favorite_clicked = false;
        let mut set_as_global_clicked = false;
        let mut clear_page_clicked = false;
        let mut save_to_slot: Option<usize> = None;
        let mut load_from_slot: Option<usize> = None;

        // 編集対象ページを含むお気に入り (なければ None)。
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

        // スコープ判定
        let has_override = self.adjustment_page_params.contains_key(&fs_idx);
        let fav_default_active = !has_override
            && self
                .current_favorite_id_for_idx(fs_idx)
                .map(|id| self.adjustment_favorite_params.contains_key(&id))
                .unwrap_or(false);
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

        let scroll_output = body_child.allocate_ui_with_layout(
            egui::vec2(body_width, body_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.set_min_width(body_width);
                ui.set_max_width(body_width);
                ui.set_min_height(body_height);
                // ScrollArea は親 UI の available_rect を上限にするため、body_rect と
                // 同じ高さの親領域を明示確保してから置く。これでコンテンツが短い
                // 場合もパネル下端近くまで暗背景 + 操作領域が伸びる。
                egui::ScrollArea::vertical()
                    .max_height(body_height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(content_width);
                        ui.add_space(4.0);

                        // ── 見開き L/R セレクタ ──
                        if let Some((left, right)) = spread_lr {
                            let same = self.effective_params(left) == self.effective_params(right);
                            ui.horizontal(|ui| {
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
                            ui.add_space(2.0);
                        }

                        // ── スコープ表示 ──
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(&scope_text)
                                .size(12.0)
                                .color(scope_color),
                        );
                        ui.add_space(4.0);

                        // ── アクションボタン (5 行) ──
                        let wide = egui::vec2(content_width, 24.0);
                        if ui
                            .add(egui::Button::new("このフォルダの全画像に適用").min_size(wide))
                            .on_hover_text(
                                "このフォルダ/ZIP/PDF の全画像に現在のパラメータを書き込む",
                            )
                            .clicked()
                        {
                            apply_all_clicked = true;
                        }
                        if ui
                            .add(egui::Button::new("このフォルダの全画像から解除").min_size(wide))
                            .on_hover_text(
                                "このフォルダ/ZIP/PDF の全画像の個別設定を削除し、標準設定に戻す",
                            )
                            .clicked()
                        {
                            clear_all_clicked = true;
                        }
                        let set_fav_label =
                            format!("このお気に入り「{}」の標準にする", fav_display_name);
                        let clear_fav_label =
                            format!("このお気に入り「{}」の標準を解除", fav_display_name);
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
                        ui.add_space(6.0);

                        // ── スライダー群 ──
                        let slider_result = draw_sliders(
                            ui,
                            &mut edit_params,
                            ai_denoise_disabled_threshold,
                            ai_upscale_disabled_threshold,
                        );

                        // ── 保存スロット (5x2 grid) ──
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("保存スロット")
                                .size(11.0)
                                .color(LABEL_COLOR),
                        );
                        ui.add_space(2.0);
                        let slot_gap = 4.0;
                        let save_btn_w = 22.0;
                        let btn_w =
                            ((content_width - save_btn_w * 2.0 - slot_gap * 3.0) * 0.5).max(60.0);
                        for row in 0..5 {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = slot_gap;
                                for col in 0..2 {
                                    let slot_idx = row * 2 + col;
                                    let key_label = crate::adjustment::slot_key_label(slot_idx);
                                    let slot_name = if let Some(s) =
                                        &self.settings.preset_slots.slots[slot_idx]
                                    {
                                        format!(
                                            "{}:{}",
                                            key_label,
                                            crate::ui_helpers::truncate_name(&s.name, 7)
                                        )
                                    } else {
                                        format!("{}:空", key_label)
                                    };
                                    let has_data =
                                        self.settings.preset_slots.slots[slot_idx].is_some();
                                    let name_btn = egui::Button::new(
                                        egui::RichText::new(&slot_name).size(10.5),
                                    )
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
                                    let save_btn =
                                        egui::Button::new(egui::RichText::new("💾").size(11.0))
                                            .min_size(egui::vec2(save_btn_w, 22.0));
                                    let save_resp = ui.add(save_btn);
                                    if save_resp.clicked() {
                                        save_to_slot = Some(slot_idx);
                                    }
                                    save_resp.on_hover_text(format!(
                                        "現在の設定をスロット{}に保存",
                                        key_label
                                    ));
                                }
                            });
                        }
                        slider_result
                    })
                    .inner
            },
        );
        let (changed, is_dragging) = scroll_output.inner;

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
