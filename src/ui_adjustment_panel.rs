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

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui;

use crate::adjustment::{AdjustParams, AutoMode, PostFilter, PresetSlot};
use crate::app::{
    AdjustSpreadTarget, App, LocalAdjustGeneratedMask, LocalAdjustMaskColorPreset,
    LocalAdjustMaskEditTarget, LocalAdjustMaskShapeDrag, LocalAdjustMaskTool,
    LocalAdjustRegionSegmentationScope, LocalAdjustShapeHandle,
};
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
/// 補正レイヤー左パネルの幅。`local_adjust_lab` の `PANEL_W` と揃える。
const LOCAL_ADJUST_PANEL_W: f32 = 340.0;
/// 選択中レイヤーのマスク / 効果パラメータ用右パネル幅。`local_adjust_lab` と揃える。
const LOCAL_ADJUST_TOOL_PANEL_W: f32 = 300.0;
const LOCAL_ADJUST_PANEL_MARGIN_X: f32 = 16.0;
const LOCAL_ADJUST_PANEL_MARGIN_Y: f32 = 60.0;
const LOCAL_ADJUST_PANEL_BOTTOM_MARGIN: f32 = 20.0;
const LOCAL_ADJUST_PANEL_MIN_BODY_H: f32 = 160.0;
const LOCAL_ADJUST_PANEL_SECTION_MARGIN_LEFT: i8 = 10;
const LOCAL_ADJUST_PANEL_SECTION_MARGIN_RIGHT: i8 = 6;
const LOCAL_ADJUST_PANEL_SECTION_CONTENT_W_SHRINK: f32 =
    (LOCAL_ADJUST_PANEL_SECTION_MARGIN_LEFT + LOCAL_ADJUST_PANEL_SECTION_MARGIN_RIGHT) as f32;
const LOCAL_ADJUST_MASK_PICKER_BUTTON_SIZE: egui::Vec2 = egui::vec2(156.0, 30.0);
const LOCAL_ADJUST_EFFECT_PICKER_BUTTON_H: f32 = 30.0;
const LOCAL_ADJUST_EDGE_BRUSH_INCLUDE_BOUNDARY_RADIUS: isize = 2;
const LOCAL_ADJUST_U2NETP_INPUT_SIZE: usize = 320;
const LOCAL_ADJUST_REGION_SEGMENT_MAX_LABELS: usize = 2048;
const LOCAL_ADJUST_MASK_PREVIEW_BASE_ALPHA: f32 = 155.0;
const LOCAL_ADJUST_MASK_PREVIEW_EDIT_ALPHA: u8 = 225;
const LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS: f32 = 768.0;
const LOCAL_ADJUST_REGION_BOUNDARY_ANIM_INTERVAL_MS: u64 = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAdjustBitmapMaskOp {
    Expand,
    Shrink,
}

fn with_local_adjust_dark_window_style<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce() -> R,
) -> R {
    let previous_style = (*ctx.style()).clone();
    let mut dark_style = previous_style.clone();
    dark_style.visuals = egui::Visuals::dark();
    dark_style.visuals.override_text_color = Some(egui::Color32::WHITE);
    dark_style.visuals.window_fill = egui::Color32::from_rgba_unmultiplied(24, 24, 26, 245);
    dark_style.visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::WHITE);
    dark_style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    ctx.set_style(dark_style);
    let result = add_contents();
    ctx.set_style(previous_style);
    result
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
    generate_subject_mask: Option<usize>,
    generate_region_mask: Option<(usize, LocalAdjustRegionSegmentationScope)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAdjustPanelSection {
    General,
    Tool,
    Mask,
    Effect,
}

impl LocalAdjustPanelSection {
    fn accent_color(self) -> egui::Color32 {
        match self {
            Self::General => egui::Color32::from_rgb(115, 115, 122),
            Self::Tool => egui::Color32::from_rgb(120, 170, 235),
            Self::Mask => egui::Color32::from_rgb(80, 190, 165),
            Self::Effect => egui::Color32::from_rgb(225, 185, 80),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskKind {
    Full,
    Raster,
    LinearGradient,
    RadialGradient,
    LumaRange,
    ColorRange,
    Subject,
    Segmentation,
}

impl MaskKind {
    fn from_mask(mask: &local_adjust_core::LocalMask) -> Self {
        match mask {
            local_adjust_core::LocalMask::Full => Self::Full,
            local_adjust_core::LocalMask::Raster(_)
            | local_adjust_core::LocalMask::RasterVector(_) => Self::Raster,
            local_adjust_core::LocalMask::LinearGradient(_) => Self::LinearGradient,
            local_adjust_core::LocalMask::RadialGradient(_) => Self::RadialGradient,
            local_adjust_core::LocalMask::LumaRange(_) => Self::LumaRange,
            local_adjust_core::LocalMask::ColorRange(_) => Self::ColorRange,
            local_adjust_core::LocalMask::Subject(_) => Self::Subject,
            local_adjust_core::LocalMask::Segmentation(_) => Self::Segmentation,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Full => "全体",
            Self::Raster => "手動",
            Self::LinearGradient => "線形",
            Self::RadialGradient => "円形",
            Self::LumaRange => "輝度",
            Self::ColorRange => "カラー",
            Self::Subject => "被写体",
            Self::Segmentation => "領域",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Full => "画像全体に同じ効果をかけます。",
            Self::Raster => "ブラシ、囲み、図形で手動作成するマスクです。",
            Self::LinearGradient => "線に沿って段階的に効果をかけます。",
            Self::RadialGradient => "中心から外側へ段階的に効果をかけます。",
            Self::LumaRange => "明るさの範囲でマスクを作ります。",
            Self::ColorRange => "指定色に近い範囲をマスクにします。",
            Self::Subject => "U²-Netp で被写体マスクを生成して使います。",
            Self::Segmentation => "色と境界から領域候補を生成してクリック選択します。",
        }
    }
}

#[cfg(test)]
mod local_adjust_segmentation_tests {
    use super::*;

    #[test]
    fn local_adjust_mask_preview_alt_inverts_panel_toggle() {
        assert!(local_adjust_mask_preview_active(true, true, false));
        assert!(!local_adjust_mask_preview_active(true, true, true));
        assert!(!local_adjust_mask_preview_active(true, false, false));
        assert!(local_adjust_mask_preview_active(true, false, true));
        assert!(!local_adjust_mask_preview_active(false, true, false));
        assert!(!local_adjust_mask_preview_active(false, false, true));
    }

    #[test]
    fn local_adjust_shape_handle_hit_radius_can_expand_for_zoomed_out_view() {
        let shape = local_adjust_core::MaskShape::Ellipse {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            rx: 50.0,
            ry: 24.0,
            rotation_rad: 0.0,
        };
        assert_eq!(
            hit_local_adjust_shape_handles(shape, [170.0, 100.0], 12.0),
            None
        );
        assert_eq!(
            hit_local_adjust_shape_handles(shape, [170.0, 100.0], 24.0),
            Some(LocalAdjustShapeHandle::Radius)
        );
    }

    #[test]
    fn rect_and_ellipse_shape_handles_are_hit_testable() {
        let rect = local_adjust_core::MaskShape::Rect {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            half_w: 30.0,
            half_h: 20.0,
            rotation_rad: 0.0,
        };
        assert_eq!(
            hit_local_adjust_shape_handles(rect, [130.0, 120.0], 14.0),
            Some(LocalAdjustShapeHandle::Corner(2))
        );
        let ellipse = local_adjust_core::MaskShape::Ellipse {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            rx: 40.0,
            ry: 22.0,
            rotation_rad: 0.0,
        };
        assert_eq!(
            hit_local_adjust_shape_handles(ellipse, [140.0, 100.0], 14.0),
            Some(LocalAdjustShapeHandle::Radius)
        );
    }

    #[test]
    fn full_mask_preview_hides_plain_full_base_but_shows_subtract_result() {
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "full",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        );
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 2, 1, 0, 0),
            0.0
        );
        layer.manual_override.subtract = Some(local_adjust_core::RasterVectorMask {
            width: 2,
            height: 1,
            alpha: vec![1.0, 0.0],
            shapes: Vec::new(),
        });
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 2, 1, 0, 0),
            0.0
        );
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 2, 1, 1, 0),
            1.0
        );
    }

    #[test]
    fn subtract_override_edit_preview_uses_edit_color() {
        let colors = LocalAdjustMaskColorPreset::PinkCyan.colors();
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "full",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        );
        layer.manual_override.subtract = Some(local_adjust_core::RasterVectorMask {
            width: 1,
            height: 1,
            alpha: vec![1.0],
            shapes: Vec::new(),
        });
        assert_eq!(
            local_adjust_mask_preview_color(
                &layer,
                None,
                1,
                1,
                0,
                0,
                0.0,
                colors,
                Some(LocalAdjustMaskEditTarget::OverrideSubtract),
            ),
            colors.edit(LOCAL_ADJUST_MASK_PREVIEW_EDIT_ALPHA)
        );
    }

    #[test]
    fn bitmap_mask_expand_and_shrink_use_3x3_neighbors() {
        let src = vec![
            0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0,
        ];
        assert_eq!(local_adjust_morph_alpha_1px(&src, 3, 3, true), vec![1.0; 9]);
        assert_eq!(
            local_adjust_morph_alpha_1px(&src, 3, 3, false),
            vec![0.0; 9]
        );
    }

    #[test]
    fn linear_gradient_handle_hit_detects_endpoints() {
        let layer = local_adjust_core::LocalAdjustmentLayer::new(
            "linear",
            local_adjust_core::LocalMask::LinearGradient(local_adjust_core::LinearGradientMask {
                initialized: true,
                start: [0.2, 0.3],
                end: [0.8, 0.7],
            }),
            local_adjust_core::LocalEffect::None,
        );
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        assert_eq!(
            local_adjust_gradient_handle_hit(
                &layer,
                egui::pos2(80.0, 70.0),
                rect,
                (100, 100),
                None
            ),
            Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd)
        );
        assert_eq!(
            local_adjust_gradient_handle_hit(
                &layer,
                egui::pos2(20.0, 30.0),
                rect,
                (100, 100),
                None
            ),
            Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientStart)
        );
    }

    #[test]
    fn radial_gradient_handle_hit_detects_outer_and_center_handles() {
        let layer = local_adjust_core::LocalAdjustmentLayer::new(
            "radial",
            local_adjust_core::LocalMask::RadialGradient(local_adjust_core::RadialGradientMask {
                initialized: true,
                center: [0.5, 0.5],
                inner_radius: 0.1,
                inner_radius_y: 0.2,
                outer_radius: 0.3,
                outer_radius_y: 0.4,
            }),
            local_adjust_core::LocalEffect::None,
        );
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        assert_eq!(
            local_adjust_gradient_handle_hit(
                &layer,
                egui::pos2(160.0, 50.0),
                rect,
                (200, 100),
                None
            ),
            Some(crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX)
        );
        assert_eq!(
            local_adjust_gradient_handle_hit(
                &layer,
                egui::pos2(100.0, 50.0),
                rect,
                (200, 100),
                None
            ),
            Some(crate::app::LocalAdjustCanvasDragKind::RadialGradientCenter)
        );
    }

    #[test]
    fn gradient_create_pending_only_for_uninitialized_masks() {
        let pending_linear = local_adjust_core::LocalAdjustmentLayer::new(
            "linear",
            local_adjust_core::LocalMask::LinearGradient(Default::default()),
            local_adjust_core::LocalEffect::None,
        );
        assert!(local_adjust_gradient_create_pending(&pending_linear));

        let ready_linear = local_adjust_core::LocalAdjustmentLayer::new(
            "linear",
            local_adjust_core::LocalMask::LinearGradient(local_adjust_core::LinearGradientMask {
                initialized: true,
                start: [0.2, 0.2],
                end: [0.8, 0.8],
            }),
            local_adjust_core::LocalEffect::None,
        );
        assert!(!local_adjust_gradient_create_pending(&ready_linear));

        let pending_radial = local_adjust_core::LocalAdjustmentLayer::new(
            "radial",
            local_adjust_core::LocalMask::RadialGradient(Default::default()),
            local_adjust_core::LocalEffect::None,
        );
        assert!(local_adjust_gradient_create_pending(&pending_radial));

        let ready_radial = local_adjust_core::LocalAdjustmentLayer::new(
            "radial",
            local_adjust_core::LocalMask::RadialGradient(local_adjust_core::RadialGradientMask {
                initialized: true,
                center: [0.5, 0.5],
                inner_radius: 0.1,
                inner_radius_y: 0.1,
                outer_radius: 0.3,
                outer_radius_y: 0.3,
            }),
            local_adjust_core::LocalEffect::None,
        );
        assert!(!local_adjust_gradient_create_pending(&ready_radial));
    }

    #[test]
    fn region_boundary_color_animates_over_time() {
        let start = local_adjust_region_boundary_color(7, 0.0);
        let later = local_adjust_region_boundary_color(7, 1.0);
        assert_ne!(start, later);
    }

    #[test]
    fn tilt_shift_linear_focus_drag_updates_focus_width_and_angle() {
        let mut params = local_adjust_core::TiltShiftParams {
            range_initialized: true,
            center: [0.5, 0.5],
            ..Default::default()
        };
        assert!(apply_local_adjust_tilt_shift_handle_drag(
            &mut params,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus,
            [0.5, 0.7],
        ));
        assert!((params.focus_width - 0.2).abs() < 1e-5);
        assert!((params.angle_degrees - 90.0).abs() < 1e-4);
        assert!(params.range_initialized);
    }

    #[test]
    fn tilt_shift_radial_outer_drag_updates_falloff() {
        let mut params = local_adjust_core::TiltShiftParams {
            mode: local_adjust_core::TiltShiftMode::Radial,
            range_initialized: true,
            center: [0.5, 0.5],
            radius: [0.2, 0.25],
            falloff: 0.1,
            ..Default::default()
        };
        assert!(apply_local_adjust_tilt_shift_handle_drag(
            &mut params,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX,
            [0.8, 0.5],
        ));
        assert!((params.falloff - 0.5).abs() < 1e-5);
    }

    #[test]
    fn tilt_shift_handle_hit_detects_linear_outer_handle() {
        let params = local_adjust_core::TiltShiftParams {
            range_initialized: true,
            center: [0.5, 0.5],
            angle_degrees: 0.0,
            focus_width: 0.1,
            falloff: 0.2,
            ..Default::default()
        };
        let effect = local_adjust_core::LocalEffect::TiltShift(params);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        assert_eq!(
            local_adjust_tilt_shift_handle_hit(&effect, egui::pos2(80.0, 50.0), rect),
            Some(crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter)
        );
    }

    #[test]
    fn tilt_shift_range_drag_initializes_linear_range() {
        let mut params = local_adjust_core::TiltShiftParams {
            mode: local_adjust_core::TiltShiftMode::Linear,
            mode_selected: true,
            range_initialized: false,
            strength: 0.0,
            max_radius_px: 0.0,
            ..Default::default()
        };
        assert!(apply_local_adjust_tilt_shift_range_drag(
            &mut params,
            [0.25, 0.25],
            [0.25, 0.65],
        ));
        assert_eq!(params.center, [0.25, 0.25]);
        assert!(params.range_initialized);
        assert!(!params.mode_selected);
        assert!((params.angle_degrees - 90.0).abs() < 1e-4);
        assert!((params.focus_width - 0.14).abs() < 1e-5);
        assert!((params.falloff - 0.26).abs() < 1e-5);
        assert_eq!(params.strength, 1.0);
        assert_eq!(params.max_radius_px, 20.0);
    }

    #[test]
    fn tilt_shift_range_drag_initializes_radial_range() {
        let mut params = local_adjust_core::TiltShiftParams {
            mode: local_adjust_core::TiltShiftMode::Radial,
            mode_selected: true,
            range_initialized: false,
            ..Default::default()
        };
        assert!(apply_local_adjust_tilt_shift_range_drag(
            &mut params,
            [0.4, 0.5],
            [0.7, 0.8],
        ));
        assert_eq!(params.center, [0.4, 0.5]);
        assert!(params.range_initialized);
        assert!(!params.mode_selected);
        assert!((params.radius[0] - 0.3).abs() < 1e-5);
        assert!((params.radius[1] - 0.3).abs() < 1e-5);
        assert!((params.falloff - 0.4).abs() < 1e-5);
    }

    #[test]
    fn region_segmentation_splits_connected_color_regions() {
        let width = 6;
        let height = 4;
        let mut pixels = Vec::with_capacity(width * height);
        for _y in 0..height {
            for x in 0..width {
                let rgb = if x < 3 { [230, 40, 40] } else { [40, 80, 230] };
                pixels.push(egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            }
        }
        let source = egui::ColorImage::new([width, height], pixels);
        let mask = build_local_adjust_region_segmentation(
            &source,
            None,
            LocalAdjustRegionSegmentationScope::Full,
            8.0,
            1,
            255.0,
            255.0,
            0,
        )
        .unwrap();
        assert_eq!(mask.label_count(), 2);
        assert_ne!(mask.labels[0], mask.labels[width - 1]);
        assert!(mask.selected.iter().skip(1).all(|&selected| !selected));
    }

    #[test]
    fn region_segmentation_background_scope_excludes_subject_pixels() {
        let source = egui::ColorImage::new(
            [4, 1],
            vec![
                egui::Color32::from_rgb(220, 40, 40),
                egui::Color32::from_rgb(220, 40, 40),
                egui::Color32::from_rgb(40, 80, 230),
                egui::Color32::from_rgb(40, 80, 230),
            ],
        );
        let subject = local_adjust_core::RasterMask {
            width: 4,
            height: 1,
            alpha: vec![1.0, 1.0, 0.0, 0.0],
        };
        let mask = build_local_adjust_region_segmentation(
            &source,
            Some(&subject),
            LocalAdjustRegionSegmentationScope::Background,
            8.0,
            1,
            255.0,
            255.0,
            0,
        )
        .unwrap();
        assert_eq!(mask.labels[0], 0);
        assert_eq!(mask.labels[1], 0);
        assert_ne!(mask.labels[2], 0);
        assert_eq!(mask.labels[2], mask.labels[3]);
    }

    #[test]
    fn region_segmentation_fills_unlabeled_internal_gaps() {
        let mut labels = vec![1, 0, 2];
        let allowed = vec![true, true, true];
        fill_local_adjust_unlabeled_region_pixels(&mut labels, 3, 1, &allowed);
        assert_ne!(labels[1], 0);
    }
}

fn effective_local_mask_edit_target(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    selected: LocalAdjustMaskEditTarget,
) -> Option<LocalAdjustMaskEditTarget> {
    match &layer.mask {
        local_adjust_core::LocalMask::Raster(_) | local_adjust_core::LocalMask::RasterVector(_) => {
            Some(LocalAdjustMaskEditTarget::Base)
        }
        local_adjust_core::LocalMask::Full => (selected
            == LocalAdjustMaskEditTarget::OverrideSubtract)
            .then_some(LocalAdjustMaskEditTarget::OverrideSubtract),
        _ => matches!(
            selected,
            LocalAdjustMaskEditTarget::OverrideAdd | LocalAdjustMaskEditTarget::OverrideSubtract
        )
        .then_some(selected),
    }
}

fn local_mask_edit_target_label(target: LocalAdjustMaskEditTarget) -> &'static str {
    match target {
        LocalAdjustMaskEditTarget::None => "閉じる",
        LocalAdjustMaskEditTarget::Base => "手動マスク",
        LocalAdjustMaskEditTarget::OverrideAdd => "追加マスク",
        LocalAdjustMaskEditTarget::OverrideSubtract => "削除マスク",
    }
}

fn local_mask_tool_label(tool: LocalAdjustMaskTool) -> &'static str {
    match tool {
        LocalAdjustMaskTool::Select => "選択",
        LocalAdjustMaskTool::Brush => "筆",
        LocalAdjustMaskTool::EdgeBrush => "境界筆",
        LocalAdjustMaskTool::GapFillBrush => "隙間補完",
        LocalAdjustMaskTool::Lasso => "囲み",
        LocalAdjustMaskTool::Polygon => "多角形",
        LocalAdjustMaskTool::Line => "直線",
        LocalAdjustMaskTool::VertLine => "縦線",
        LocalAdjustMaskTool::HorizLine => "横線",
        LocalAdjustMaskTool::Rect => "矩形",
        LocalAdjustMaskTool::Ellipse => "楕円",
    }
}

fn local_mask_override_slot_mut(
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    target: LocalAdjustMaskEditTarget,
) -> Option<&mut Option<local_adjust_core::RasterVectorMask>> {
    match target {
        LocalAdjustMaskEditTarget::OverrideAdd => Some(&mut layer.manual_override.add),
        LocalAdjustMaskEditTarget::OverrideSubtract => Some(&mut layer.manual_override.subtract),
        _ => None,
    }
}

fn local_adjust_raster_vector_has_content(mask: &local_adjust_core::RasterVectorMask) -> bool {
    mask.alpha.iter().any(|a| *a > 0.0) || !mask.shapes.is_empty()
}

fn local_adjust_morph_alpha_1px(
    src: &[f32],
    width: usize,
    height: usize,
    dilate: bool,
) -> Vec<f32> {
    if width == 0 || height == 0 || src.len() < width.saturating_mul(height) {
        return src.to_vec();
    }
    let mut out = vec![0.0; src.len()];
    for y in 0..height {
        for x in 0..width {
            let mut value = if dilate { 0.0_f32 } else { 1.0_f32 };
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    let sample = src[yy * width + xx].clamp(0.0, 1.0);
                    if dilate {
                        value = value.max(sample);
                    } else {
                        value = value.min(sample);
                    }
                }
            }
            out[y * width + x] = value;
        }
    }
    out
}

fn local_adjust_target_raster_vector_mask_mut(
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    target: LocalAdjustMaskEditTarget,
    image_dims: (usize, usize),
    create: bool,
) -> Option<&mut local_adjust_core::RasterVectorMask> {
    let (width, height) = (image_dims.0.max(1), image_dims.1.max(1));
    if target == LocalAdjustMaskEditTarget::Base
        && matches!(layer.mask, local_adjust_core::LocalMask::Raster(_))
    {
        let old = std::mem::replace(&mut layer.mask, local_adjust_core::LocalMask::Full);
        if let local_adjust_core::LocalMask::Raster(mask) = old {
            layer.mask =
                local_adjust_core::LocalMask::RasterVector(local_adjust_core::RasterVectorMask {
                    width: mask.width,
                    height: mask.height,
                    alpha: mask.alpha,
                    shapes: Vec::new(),
                });
        }
    }

    match target {
        LocalAdjustMaskEditTarget::Base => match &mut layer.mask {
            local_adjust_core::LocalMask::RasterVector(mask) => Some(mask),
            _ => None,
        },
        LocalAdjustMaskEditTarget::OverrideAdd | LocalAdjustMaskEditTarget::OverrideSubtract => {
            let slot = local_mask_override_slot_mut(layer, target)?;
            if slot
                .as_ref()
                .is_none_or(|mask| mask.width != width || mask.height != height)
            {
                if !create {
                    return None;
                }
                *slot = Some(local_adjust_core::RasterVectorMask::empty(width, height));
            }
            slot.as_mut()
        }
        LocalAdjustMaskEditTarget::None => None,
    }
}

fn local_adjust_target_raster_vector_mask_ref(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    target: LocalAdjustMaskEditTarget,
) -> Option<&local_adjust_core::RasterVectorMask> {
    match target {
        LocalAdjustMaskEditTarget::Base => match &layer.mask {
            local_adjust_core::LocalMask::RasterVector(mask) => Some(mask),
            _ => None,
        },
        LocalAdjustMaskEditTarget::OverrideAdd => layer.manual_override.add.as_ref(),
        LocalAdjustMaskEditTarget::OverrideSubtract => layer.manual_override.subtract.as_ref(),
        LocalAdjustMaskEditTarget::None => None,
    }
}

fn compact_local_adjust_manual_override(layer: &mut local_adjust_core::LocalAdjustmentLayer) {
    if layer
        .manual_override
        .add
        .as_ref()
        .is_some_and(|mask| !local_adjust_raster_vector_has_content(mask))
    {
        layer.manual_override.add = None;
    }
    if layer
        .manual_override
        .subtract
        .as_ref()
        .is_some_and(|mask| !local_adjust_raster_vector_has_content(mask))
    {
        layer.manual_override.subtract = None;
    }
}

struct MaskGroup {
    title: &'static str,
    kinds: &'static [MaskKind],
}

const MASK_GROUPS: &[MaskGroup] = &[
    MaskGroup {
        title: "基本",
        kinds: &[MaskKind::Full, MaskKind::Raster],
    },
    MaskGroup {
        title: "グラデーション",
        kinds: &[MaskKind::LinearGradient, MaskKind::RadialGradient],
    },
    MaskGroup {
        title: "範囲",
        kinds: &[MaskKind::LumaRange, MaskKind::ColorRange],
    },
    MaskGroup {
        title: "自動",
        kinds: &[MaskKind::Subject, MaskKind::Segmentation],
    },
];

fn draw_local_adjust_left_panel(
    ui: &mut egui::Ui,
    panel_w: f32,
    layers: &[local_adjust_core::LocalAdjustmentLayer],
    selected_layer: usize,
    image_dims: (usize, usize),
    source: Option<&egui::ColorImage>,
    add_layer_dialog_open: &mut bool,
    effect_picker_dialog_open: &mut bool,
    select_layer: &mut Option<usize>,
    set_enabled: &mut Option<(usize, bool)>,
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    move_layer: &mut Option<(usize, usize)>,
    duplicate_layer: &mut Option<usize>,
    delete_layer: &mut Option<usize>,
    mask_edit_target: &mut LocalAdjustMaskEditTarget,
    mask_paint_add: &mut bool,
    mask_tool: &mut LocalAdjustMaskTool,
    bitmap_mask_op: &mut Option<LocalAdjustBitmapMaskOp>,
    show_source: &mut bool,
    show_mask: &mut bool,
    mask_color_preset: &mut LocalAdjustMaskColorPreset,
    preview_to_selected_layer: &mut bool,
) {
    let section_panel_w = panel_w - LOCAL_ADJUST_PANEL_SECTION_CONTENT_W_SHRINK;
    let btn_w = ((section_panel_w - 20.0 - 4.0) / 2.0).max(96.0);
    let btn_size = egui::vec2(btn_w, 24.0);
    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::General, |ui| {
        draw_local_adjust_display_controls(ui, btn_size, show_source, show_mask, mask_color_preset);
    });
    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::General, |ui| {
        ui.add_space(4.0);
        draw_local_adjust_layer_list(
            ui,
            section_panel_w,
            layers,
            selected_layer,
            image_dims,
            source,
            add_layer_dialog_open,
            select_layer,
            set_enabled,
            update_layer,
            move_layer,
            duplicate_layer,
            delete_layer,
            preview_to_selected_layer,
        );
    });

    if layers.is_empty() {
        return;
    }

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Effect, |ui| {
        draw_local_adjust_effect_selector(
            ui,
            section_panel_w,
            layers,
            selected_layer,
            effect_picker_dialog_open,
        );
    });
    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Mask, |ui| {
        draw_local_adjust_manual_tool_selector(
            ui,
            btn_size,
            selected_layer,
            layers.get(selected_layer),
            update_layer,
            mask_edit_target,
            mask_paint_add,
            mask_tool,
            bitmap_mask_op,
        );
    });
}

fn draw_local_adjust_display_controls(
    ui: &mut egui::Ui,
    btn_size: egui::Vec2,
    show_source: &mut bool,
    show_mask: &mut bool,
    mask_color_preset: &mut LocalAdjustMaskColorPreset,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("表示:").color(egui::Color32::from_gray(200)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for preset in LocalAdjustMaskColorPreset::ALL.into_iter().rev() {
                let selected = *mask_color_preset == preset;
                let colors = preset.colors();
                let button = egui::Button::new(
                    egui::RichText::new(preset.label())
                        .strong()
                        .size(10.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(colors.base(if selected { 145 } else { 80 }))
                .stroke(if selected {
                    egui::Stroke::new(1.5, colors.edit(255))
                } else {
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 35),
                    )
                });
                if ui
                    .add_sized(egui::vec2(24.0, 18.0), button)
                    .on_hover_text(format!("マスクカラー: {}", preset.description()))
                    .clicked()
                {
                    *mask_color_preset = preset;
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if local_adjust_panel_toggle_button(ui, "元画像 [Q]", *show_source, Some(btn_size), false)
            .clicked()
        {
            *show_source = !*show_source;
        }
        if local_adjust_panel_toggle_button(ui, "マスク [W]", *show_mask, Some(btn_size), false)
            .clicked()
        {
            *show_mask = !*show_mask;
        }
    });
}

fn draw_local_adjust_layer_list(
    ui: &mut egui::Ui,
    panel_w: f32,
    layers: &[local_adjust_core::LocalAdjustmentLayer],
    selected_layer: usize,
    image_dims: (usize, usize),
    source: Option<&egui::ColorImage>,
    add_layer_dialog_open: &mut bool,
    select_layer: &mut Option<usize>,
    set_enabled: &mut Option<(usize, bool)>,
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    move_layer: &mut Option<(usize, usize)>,
    duplicate_layer: &mut Option<usize>,
    delete_layer: &mut Option<usize>,
    preview_to_selected_layer: &mut bool,
) {
    let btn_w = ((panel_w - 20.0 - 4.0) / 2.0).max(96.0);
    let action_row_w = btn_w * 2.0 + 4.0;
    ui.label(
        egui::RichText::new("レイヤー")
            .size(14.0)
            .strong()
            .color(egui::Color32::WHITE),
    );
    if ui
        .add_sized(
            egui::vec2(action_row_w, 24.0),
            egui::Button::new("+ 補正レイヤー"),
        )
        .clicked()
    {
        *add_layer_dialog_open = true;
    }
    ui.checkbox(preview_to_selected_layer, "選択レイヤーまでプレビュー");
    if *preview_to_selected_layer && !layers.is_empty() {
        ui.label(
            egui::RichText::new(format!(
                "表示中: 1〜{} / {}",
                selected_layer.min(layers.len() - 1) + 1,
                layers.len()
            ))
            .size(10.0)
            .color(egui::Color32::from_gray(170)),
        );
    }
    if layers.is_empty() {
        ui.label(
            egui::RichText::new("補正レイヤーを追加してください。")
                .size(11.0)
                .color(egui::Color32::from_gray(180)),
        );
        return;
    }

    let mut clicked_layer = None;
    for (idx, layer) in layers.iter().enumerate() {
        let selected = idx == selected_layer;
        let frame_response = egui::Frame::new()
            .fill(if selected {
                egui::Color32::from_rgba_unmultiplied(58, 96, 150, 170)
            } else {
                egui::Color32::from_rgba_unmultiplied(52, 52, 54, 120)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    egui::Color32::from_rgba_unmultiplied(150, 195, 255, 130)
                } else {
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 24)
                },
            ))
            .corner_radius(4.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_min_width(panel_w - 12.0);
                ui.set_min_height(56.0);
                let mut row_clicked = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.set_min_width(42.0);
                        let mut enabled = layer.enabled;
                        if ui.checkbox(&mut enabled, "").changed() {
                            *set_enabled = Some((idx, enabled));
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let before = draw_local_mask_application_button(
                                ui,
                                "前",
                                layer.mask_before_effect,
                            );
                            if before
                                .on_hover_text("ONで、マスク範囲だけを効果の入力素材にします。")
                                .clicked()
                            {
                                let mut edited = layer.clone();
                                edited.mask_before_effect = !edited.mask_before_effect;
                                *update_layer = Some((idx, edited));
                            }
                            let after = draw_local_mask_application_button(
                                ui,
                                "後",
                                layer.mask_after_effect,
                            );
                            if after
                                .on_hover_text("ONで、効果後の結果をマスク範囲で切り取ります。")
                                .clicked()
                            {
                                let mut edited = layer.clone();
                                edited.mask_after_effect = !edited.mask_after_effect;
                                *update_layer = Some((idx, edited));
                            }
                        });
                    });
                    if draw_local_layer_mask_thumbnail(ui, layer, image_dims, source, selected)
                        .clicked()
                    {
                        row_clicked = true;
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let text_color = if layer.enabled {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_gray(145)
                        };
                        let mask_line = MaskKind::from_mask(&layer.mask).label();
                        let effect_line = layer.effect.display_label();
                        if ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(mask_line).strong().color(text_color),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            row_clicked = true;
                        }
                        if ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(effect_line).size(11.0).color(
                                        if layer.enabled {
                                            egui::Color32::from_gray(205)
                                        } else {
                                            egui::Color32::from_gray(125)
                                        },
                                    ),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            row_clicked = true;
                        }
                        if !layer.enabled
                            && ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new("OFF")
                                            .size(10.0)
                                            .color(egui::Color32::from_gray(150)),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                        {
                            row_clicked = true;
                        }
                    });
                    let spacer_w = ui.available_width().max(0.0);
                    if spacer_w > 4.0 {
                        let (_, spacer_response) = ui
                            .allocate_exact_size(egui::vec2(spacer_w, 56.0), egui::Sense::click());
                        if spacer_response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            row_clicked = true;
                        }
                    }
                });
                row_clicked
            });
        if frame_response.inner {
            clicked_layer = Some(idx);
        }
    }
    if let Some(idx) = clicked_layer {
        *select_layer = Some(idx);
    }

    ui.horizontal(|ui| {
        let gap = 4.0;
        ui.spacing_mut().item_spacing.x = gap;
        let unit_w = ((action_row_w - gap * 3.0) / 6.0).max(24.0);
        let small_btn = egui::vec2(unit_w, 22.0);
        let wide_btn = egui::vec2(unit_w * 2.0, 22.0);
        if ui
            .add_enabled(
                selected_layer > 0,
                egui::Button::new("↑").min_size(small_btn),
            )
            .clicked()
        {
            *move_layer = Some((selected_layer, selected_layer - 1));
        }
        if ui
            .add_enabled(
                selected_layer + 1 < layers.len(),
                egui::Button::new("↓").min_size(small_btn),
            )
            .clicked()
        {
            *move_layer = Some((selected_layer, selected_layer + 1));
        }
        if ui.add_sized(wide_btn, egui::Button::new("複製")).clicked() {
            *duplicate_layer = Some(selected_layer);
        }
        if ui
            .add_sized(
                wide_btn,
                egui::Button::new("削除").fill(egui::Color32::from_rgb(120, 50, 50)),
            )
            .clicked()
        {
            *delete_layer = Some(selected_layer);
        }
    });
}

fn draw_local_adjust_effect_selector(
    ui: &mut egui::Ui,
    panel_w: f32,
    layers: &[local_adjust_core::LocalAdjustmentLayer],
    selected_layer: usize,
    effect_picker_dialog_open: &mut bool,
) {
    ui.label(egui::RichText::new("加工内容:").color(egui::Color32::from_gray(200)));
    let label = layers
        .get(selected_layer)
        .map(|layer| layer.effect.display_label().to_string())
        .unwrap_or_else(|| "効果なし".to_string());
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2((panel_w - 82.0).max(160.0), 24.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(egui::Color32::from_gray(230)),
            ),
        );
        if ui
            .add_sized(egui::vec2(74.0, 24.0), egui::Button::new("効果選択"))
            .on_hover_text("効果をグループ別の一覧から選びます。")
            .clicked()
        {
            *effect_picker_dialog_open = true;
        }
    });
}

fn draw_local_adjust_manual_tool_selector(
    ui: &mut egui::Ui,
    btn_size: egui::Vec2,
    selected_layer: usize,
    layer: Option<&local_adjust_core::LocalAdjustmentLayer>,
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    mask_edit_target: &mut LocalAdjustMaskEditTarget,
    mask_paint_add: &mut bool,
    mask_tool: &mut LocalAdjustMaskTool,
    bitmap_mask_op: &mut Option<LocalAdjustBitmapMaskOp>,
) {
    let Some(layer) = layer else {
        return;
    };
    let mask_kind = MaskKind::from_mask(&layer.mask);
    ui.label(
        egui::RichText::new(if mask_kind == MaskKind::Raster {
            "手動マスク:"
        } else if mask_kind == MaskKind::Full {
            "削除マスク:"
        } else {
            "追加/削除マスク:"
        })
        .color(egui::Color32::from_gray(200)),
    );
    match mask_kind {
        MaskKind::Raster => {
            *mask_edit_target = LocalAdjustMaskEditTarget::Base;
            draw_local_manual_mask_tool_panel(
                ui,
                btn_size,
                mask_paint_add,
                mask_tool,
                bitmap_mask_op,
            );
            return;
        }
        MaskKind::Full => {
            if matches!(
                *mask_edit_target,
                LocalAdjustMaskEditTarget::Base | LocalAdjustMaskEditTarget::OverrideAdd
            ) {
                *mask_edit_target = LocalAdjustMaskEditTarget::None;
            }
            let label = if layer.manual_override.subtract.is_some() {
                "削除マスクあり"
            } else {
                "削除マスク"
            };
            if local_adjust_panel_toggle_button(
                ui,
                label,
                *mask_edit_target == LocalAdjustMaskEditTarget::OverrideSubtract,
                Some(btn_size),
                false,
            )
            .clicked()
            {
                *mask_edit_target =
                    if *mask_edit_target == LocalAdjustMaskEditTarget::OverrideSubtract {
                        LocalAdjustMaskEditTarget::None
                    } else {
                        LocalAdjustMaskEditTarget::OverrideSubtract
                    };
            }
        }
        _ => {
            if *mask_edit_target == LocalAdjustMaskEditTarget::Base {
                *mask_edit_target = LocalAdjustMaskEditTarget::None;
            }
            ui.horizontal(|ui| {
                let add_label = if layer.manual_override.add.is_some() {
                    "追加マスクあり"
                } else {
                    "追加マスク"
                };
                if local_adjust_panel_toggle_button(
                    ui,
                    add_label,
                    *mask_edit_target == LocalAdjustMaskEditTarget::OverrideAdd,
                    Some(btn_size),
                    true,
                )
                .clicked()
                {
                    *mask_edit_target =
                        if *mask_edit_target == LocalAdjustMaskEditTarget::OverrideAdd {
                            LocalAdjustMaskEditTarget::None
                        } else {
                            LocalAdjustMaskEditTarget::OverrideAdd
                        };
                }
                let subtract_label = if layer.manual_override.subtract.is_some() {
                    "削除マスクあり"
                } else {
                    "削除マスク"
                };
                if local_adjust_panel_toggle_button(
                    ui,
                    subtract_label,
                    *mask_edit_target == LocalAdjustMaskEditTarget::OverrideSubtract,
                    Some(btn_size),
                    false,
                )
                .clicked()
                {
                    *mask_edit_target =
                        if *mask_edit_target == LocalAdjustMaskEditTarget::OverrideSubtract {
                            LocalAdjustMaskEditTarget::None
                        } else {
                            LocalAdjustMaskEditTarget::OverrideSubtract
                        };
                }
            });
        }
    }

    if let Some(active_target) = effective_local_mask_edit_target(layer, *mask_edit_target) {
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(34, 34, 36, 170))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            ))
            .corner_radius(4.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{}を編集中",
                        local_mask_edit_target_label(active_target)
                    ))
                    .strong()
                    .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new("追加は1.0、削除は0.0でベースマスクを上書きします。")
                        .size(10.0)
                        .color(egui::Color32::from_gray(170)),
                );
                draw_local_manual_mask_tool_panel(
                    ui,
                    btn_size,
                    mask_paint_add,
                    mask_tool,
                    bitmap_mask_op,
                );
                ui.separator();
                let has_target = match active_target {
                    LocalAdjustMaskEditTarget::OverrideAdd => layer.manual_override.add.is_some(),
                    LocalAdjustMaskEditTarget::OverrideSubtract => {
                        layer.manual_override.subtract.is_some()
                    }
                    _ => false,
                };
                let clear_label =
                    format!("{}を全消去", local_mask_edit_target_label(active_target));
                if ui
                    .add_enabled(
                        has_target,
                        egui::Button::new(clear_label).fill(egui::Color32::from_rgb(95, 45, 45)),
                    )
                    .on_hover_text(
                        "現在開いている追加/削除マスクだけを空にします。ベースマスクは残ります。",
                    )
                    .clicked()
                {
                    let mut edited = layer.clone();
                    match active_target {
                        LocalAdjustMaskEditTarget::OverrideAdd => {
                            edited.manual_override.add = None;
                        }
                        LocalAdjustMaskEditTarget::OverrideSubtract => {
                            edited.manual_override.subtract = None;
                        }
                        _ => {}
                    }
                    *update_layer = Some((selected_layer, edited));
                }
            });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            let help = if mask_kind == MaskKind::Full {
                "全体マスクでは削除マスクだけを開いて除外範囲を描きます。"
            } else {
                "必要なときだけ追加マスク/削除マスクを開いて手描きします。"
            };
            ui.label(
                egui::RichText::new(help)
                    .size(10.0)
                    .color(egui::Color32::from_gray(170)),
            );
        });
    }
}

fn draw_local_manual_mask_tool_panel(
    ui: &mut egui::Ui,
    btn_size: egui::Vec2,
    mask_paint_add: &mut bool,
    mask_tool: &mut LocalAdjustMaskTool,
    bitmap_mask_op: &mut Option<LocalAdjustBitmapMaskOp>,
) {
    ui.label(egui::RichText::new("描画 / 消去:").color(egui::Color32::from_gray(200)));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if local_adjust_panel_toggle_button(ui, "描画 [D]", *mask_paint_add, Some(btn_size), true)
            .clicked()
        {
            *mask_paint_add = true;
        }
        if local_adjust_panel_toggle_button(ui, "消去 [F]", !*mask_paint_add, Some(btn_size), false)
            .clicked()
        {
            *mask_paint_add = false;
        }
    });
    ui.separator();
    ui.label(egui::RichText::new("ビットマップ:").color(egui::Color32::from_gray(200)));
    for row in [
        &[
            (LocalAdjustMaskTool::Brush, "筆 [B]"),
            (LocalAdjustMaskTool::EdgeBrush, "境界筆 [A]"),
        ][..],
        &[
            (LocalAdjustMaskTool::GapFillBrush, "隙間補完 [G]"),
            (LocalAdjustMaskTool::Lasso, "囲み [L]"),
        ][..],
        &[(LocalAdjustMaskTool::Polygon, "多角形 [P]")][..],
    ] {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for &(tool, label) in row {
                if local_adjust_panel_toggle_button(
                    ui,
                    label,
                    *mask_tool == tool,
                    Some(btn_size),
                    false,
                )
                .clicked()
                {
                    *mask_tool = tool;
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if ui
            .add_sized(btn_size, egui::Button::new("1px拡張"))
            .clicked()
        {
            *bitmap_mask_op = Some(LocalAdjustBitmapMaskOp::Expand);
        }
        if ui
            .add_sized(btn_size, egui::Button::new("1px縮小"))
            .clicked()
        {
            *bitmap_mask_op = Some(LocalAdjustBitmapMaskOp::Shrink);
        }
    });
    ui.label(egui::RichText::new("オブジェクト:").color(egui::Color32::from_gray(200)));
    for row in [
        [
            (LocalAdjustMaskTool::Select, "選択 [S]"),
            (LocalAdjustMaskTool::Line, "直線 [I]"),
        ],
        [
            (LocalAdjustMaskTool::VertLine, "縦線 [V]"),
            (LocalAdjustMaskTool::HorizLine, "横線 [H]"),
        ],
        [
            (LocalAdjustMaskTool::Rect, "矩形 [R]"),
            (LocalAdjustMaskTool::Ellipse, "楕円 [O]"),
        ],
    ] {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (tool, label) in row {
                if local_adjust_panel_toggle_button(
                    ui,
                    label,
                    *mask_tool == tool,
                    Some(btn_size),
                    false,
                )
                .clicked()
                {
                    *mask_tool = tool;
                }
            }
        });
    }
}

fn local_adjust_manual_edit_controls_visible(
    layer: Option<&local_adjust_core::LocalAdjustmentLayer>,
    mask_edit_target: LocalAdjustMaskEditTarget,
) -> bool {
    let Some(layer) = layer else {
        return false;
    };
    MaskKind::from_mask(&layer.mask) == MaskKind::Raster
        || effective_local_mask_edit_target(layer, mask_edit_target).is_some()
}

fn draw_local_tool_settings(
    ui: &mut egui::Ui,
    layer: &local_adjust_core::LocalAdjustmentLayer,
    mask_edit_target: LocalAdjustMaskEditTarget,
    mask_tool: LocalAdjustMaskTool,
    mask_brush_radius: &mut f32,
    mask_line_width: &mut f32,
    mask_gap_fill_distance: &mut f32,
    boundary_edge_threshold: &mut f32,
    boundary_ink_threshold: &mut f32,
    boundary_gap_px: &mut f32,
    edge_brush_tolerance: &mut f32,
    edge_brush_include_boundary: &mut bool,
) {
    ui.label(
        egui::RichText::new("ツール設定")
            .size(14.0)
            .strong()
            .color(egui::Color32::WHITE),
    );
    ui.label(
        egui::RichText::new(local_mask_tool_label(mask_tool))
            .size(11.0)
            .color(egui::Color32::from_gray(180)),
    );
    ui.separator();

    match mask_tool {
        LocalAdjustMaskTool::Brush
        | LocalAdjustMaskTool::EdgeBrush
        | LocalAdjustMaskTool::GapFillBrush => {
            ui.add(egui::Slider::new(mask_brush_radius, 1.0..=160.0).text("筆サイズ"));
        }
        LocalAdjustMaskTool::Line
        | LocalAdjustMaskTool::VertLine
        | LocalAdjustMaskTool::HorizLine => {
            ui.add(egui::Slider::new(mask_line_width, 1.0..=160.0).text("線幅"));
        }
        _ => {}
    }

    match mask_tool {
        LocalAdjustMaskTool::EdgeBrush => {
            ui.add(egui::Slider::new(boundary_edge_threshold, 0.0..=120.0).text("境界しきい値"));
            ui.add(egui::Slider::new(boundary_ink_threshold, 0.0..=120.0).text("線内部しきい値"));
            ui.add(egui::Slider::new(boundary_gap_px, 0.0..=4.0).text("境界ギャップ補完"));
            ui.add(egui::Slider::new(edge_brush_tolerance, 0.0..=160.0).text("色差許容"));
            ui.checkbox(edge_brush_include_boundary, "境界線を含む");
            ui.label(
                egui::RichText::new(
                    "開始点から連結している近い色だけを、境界で止めて塗ります。Ctrl中は境界を表示しながら通常筆です。",
                )
                .size(10.0)
                .color(egui::Color32::from_gray(170)),
            );
        }
        LocalAdjustMaskTool::GapFillBrush => {
            ui.add(egui::Slider::new(mask_gap_fill_distance, 1.0..=48.0).text("隙間幅"));
            ui.label(
                egui::RichText::new("左右または上下のマスクに挟まれた細い未塗り部分を補完します。")
                    .size(10.0)
                    .color(egui::Color32::from_gray(170)),
            );
        }
        LocalAdjustMaskTool::Polygon => {
            ui.label(
                egui::RichText::new("右クリックまたは始点クリックで確定します。")
                    .size(10.0)
                    .color(egui::Color32::from_gray(170)),
            );
        }
        LocalAdjustMaskTool::Brush => {
            ui.label(
                egui::RichText::new("ドラッグした範囲をマスクに描画します。")
                    .size(10.0)
                    .color(egui::Color32::from_gray(170)),
            );
        }
        _ => {}
    }

    let Some(active_target) = effective_local_mask_edit_target(layer, mask_edit_target) else {
        return;
    };
    ui.separator();
    ui.label(
        egui::RichText::new(format!(
            "{}を編集中",
            local_mask_edit_target_label(active_target)
        ))
        .size(10.0)
        .color(egui::Color32::from_gray(170)),
    );
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
    segmentation_pending: bool,
    subject_model_available: bool,
    subject_mask_available: bool,
    mask_edit_target: &mut LocalAdjustMaskEditTarget,
    mask_brush_radius: &mut f32,
    mask_paint_add: &mut bool,
    mask_tool: &mut LocalAdjustMaskTool,
    mask_line_width: &mut f32,
    mask_gap_fill_distance: &mut f32,
    boundary_edge_threshold: &mut f32,
    boundary_ink_threshold: &mut f32,
    boundary_gap_px: &mut f32,
    edge_brush_tolerance: &mut f32,
    edge_brush_include_boundary: &mut bool,
    region_color_tolerance: &mut f32,
    region_min_area: &mut usize,
    change_mask_dialog_open: &mut bool,
    effect_requests: &mut LocalEffectPanelRequests,
) {
    let mut edited = layer.clone();
    let mut changed = false;
    let selected_mask_kind = Some(MaskKind::from_mask(&edited.mask));
    let manual_edit_controls_visible =
        local_adjust_manual_edit_controls_visible(Some(&edited), *mask_edit_target);

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Tool, |ui| {
        if manual_edit_controls_visible {
            draw_local_tool_settings(
                ui,
                &edited,
                *mask_edit_target,
                *mask_tool,
                mask_brush_radius,
                mask_line_width,
                mask_gap_fill_distance,
                boundary_edge_threshold,
                boundary_ink_threshold,
                boundary_gap_px,
                edge_brush_tolerance,
                edge_brush_include_boundary,
            );
        } else {
            ui.label(
                egui::RichText::new("ツール設定")
                    .size(14.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
            ui.label(
                egui::RichText::new(
                    "追加マスク/削除マスクを開くと、手描きツール設定を表示します。",
                )
                .size(11.0)
                .color(egui::Color32::from_gray(180)),
            );
        }
        if selected_mask_kind != Some(MaskKind::Raster) && manual_edit_controls_visible {
            let help = match selected_mask_kind {
                Some(MaskKind::LinearGradient) | Some(MaskKind::RadialGradient) => {
                    "選択ツールでは画像上のドラッグで生成/調整します。筆などに切り替えると追加/削除マスクを描けます。"
                }
                Some(MaskKind::ColorRange) => {
                    "選択ツールでは画像上クリックでスポイト指定します。筆などに切り替えると追加/削除マスクを描けます。"
                }
                Some(MaskKind::LumaRange) => {
                    "輝度範囲はスライダーで調整します。筆などで追加/削除マスクを描けます。"
                }
                Some(MaskKind::Full) => "全体マスクに対して削除マスクを描けます。",
                Some(MaskKind::Subject) => {
                    "被写体/背景マットを保ったまま、筆などで追加/削除マスクを描けます。"
                }
                Some(MaskKind::Segmentation) => {
                    "選択ツールでは領域候補をクリック/ドラッグでON/OFFします。筆などでは追加/削除マスクを描けます。"
                }
                None | Some(MaskKind::Raster) => "",
            };
            if !help.is_empty() {
                ui.label(
                    egui::RichText::new(help)
                        .size(11.0)
                        .color(egui::Color32::from_gray(180)),
                );
            }
        }
    });

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Mask, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("マスク設定")
                    .size(14.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
            ui.add_space(8.0);
            if ui
                .add_sized(egui::vec2(112.0, 24.0), egui::Button::new("マスク種類変更"))
                .on_hover_text("加工内容を残したまま、ベースマスクの種類を変更します。")
                .clicked()
            {
                *change_mask_dialog_open = true;
            }
        });
        changed |= ui
            .checkbox(&mut edited.mask_inverted, "マスク反転")
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut edited.opacity, 0.0..=1.0).text("不透明度"))
            .changed();
        ui.horizontal(|ui| {
            ui.label("マスク適用");
            let before_response =
                draw_local_mask_application_button(ui, "前", edited.mask_before_effect);
            let before_clicked = before_response.clicked();
            before_response.on_hover_text("ONで、マスク範囲だけを効果の入力素材にします。");
            if before_clicked {
                edited.mask_before_effect = !edited.mask_before_effect;
                changed = true;
            }
            let after_response =
                draw_local_mask_application_button(ui, "後", edited.mask_after_effect);
            let after_clicked = after_response.clicked();
            after_response.on_hover_text("ONで、効果後の結果をマスク範囲で切り取ります。");
            if after_clicked {
                edited.mask_after_effect = !edited.mask_after_effect;
                changed = true;
            }
        });
        changed |= ui
            .add(egui::Slider::new(&mut edited.mask_expand_px, -32.0..=32.0).text("拡張/縮小"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut edited.mask_feather_px, 0.0..=64.0).text("ぼかし境界"))
            .changed();
        ui.separator();
        changed |= draw_local_mask_editor(
            ui,
            &mut edited,
            image_dims,
            segmentation_pending,
            subject_model_available,
            subject_mask_available,
            mask_paint_add,
            region_color_tolerance,
            region_min_area,
            layer_idx,
            effect_requests,
        );
    });

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Effect, |ui| {
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
    });
    if changed {
        *update_layer = Some((layer_idx, edited));
    }
}

fn local_adjust_image_dims(app: &App, fs_idx: usize) -> (usize, usize) {
    if let Some(pixels) = app.current_local_adjust_source_pixels(fs_idx) {
        return (pixels.size[0].max(1), pixels.size[1].max(1));
    }
    match app.fs_cache.get(&fs_idx) {
        Some(crate::fs_animation::FsCacheEntry::Static { pixels, .. }) => {
            (pixels.size[0].max(1), pixels.size[1].max(1))
        }
        _ => (1, 1),
    }
}

fn local_adjust_image_layout(
    image_rect: egui::Rect,
    image_dims: (usize, usize),
    zoom_pan: Option<(f32, egui::Vec2)>,
) -> Option<(f32, egui::Rect)> {
    let (iw, ih) = (image_dims.0.max(1), image_dims.1.max(1));
    let display_size = egui::vec2(iw as f32, ih as f32);
    let fit_scale = (image_rect.width() / display_size.x).min(image_rect.height() / display_size.y);
    if !fit_scale.is_finite() || fit_scale <= 0.0 {
        return None;
    }
    let (total_scale, center) = match zoom_pan {
        Some((zoom, pan)) => (fit_scale * zoom, image_rect.center() + pan),
        None => (fit_scale, image_rect.center()),
    };
    Some((
        total_scale,
        egui::Rect::from_center_size(center, display_size * total_scale),
    ))
}

fn local_adjust_screen_to_norm(
    screen: egui::Pos2,
    image_rect: egui::Rect,
    image_dims: (usize, usize),
    zoom_pan: Option<(f32, egui::Vec2)>,
    require_inside: bool,
) -> Option<[f32; 2]> {
    let (_, rect) = local_adjust_image_layout(image_rect, image_dims, zoom_pan)?;
    if require_inside && !rect.contains(screen) {
        return None;
    }
    Some([
        ((screen.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((screen.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
    ])
}

fn local_adjust_norm_to_screen(
    norm: [f32; 2],
    image_rect: egui::Rect,
    image_dims: (usize, usize),
    zoom_pan: Option<(f32, egui::Vec2)>,
) -> Option<egui::Pos2> {
    let (_, rect) = local_adjust_image_layout(image_rect, image_dims, zoom_pan)?;
    Some(egui::pos2(
        rect.left() + norm[0] * rect.width(),
        rect.top() + norm[1] * rect.height(),
    ))
}

fn local_adjust_drawn_norm_to_screen(rect: egui::Rect, norm: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        rect.left() + norm[0].clamp(0.0, 1.0) * rect.width(),
        rect.top() + norm[1].clamp(0.0, 1.0) * rect.height(),
    )
}

fn local_adjust_drawn_norm_to_screen_unclamped(rect: egui::Rect, norm: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        rect.left() + norm[0] * rect.width(),
        rect.top() + norm[1] * rect.height(),
    )
}

fn local_adjust_offset_norm(base: [f32; 2], direction: [f32; 2], amount: f32) -> [f32; 2] {
    [
        base[0] + direction[0] * amount,
        base[1] + direction[1] * amount,
    ]
}

fn local_adjust_screen_px_per_source_px(rect: egui::Rect, image_dims: (usize, usize)) -> f32 {
    let sx = rect.width() / image_dims.0.max(1) as f32;
    let sy = rect.height() / image_dims.1.max(1) as f32;
    (sx + sy) * 0.5
}

fn local_adjust_distance_to_farthest_rect_corner(center: egui::Pos2, rect: egui::Rect) -> f32 {
    [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    ]
    .into_iter()
    .map(|corner| center.distance(corner))
    .fold(0.0, f32::max)
}

fn draw_local_adjust_ellipse_stroke(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius_x: f32,
    radius_y: f32,
    stroke: egui::Stroke,
) {
    if radius_x <= 0.5 || radius_y <= 0.5 {
        return;
    }
    let steps = 96;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let angle = std::f32::consts::TAU * i as f32 / steps as f32;
        points.push(egui::pos2(
            center.x + radius_x * angle.cos(),
            center.y + radius_y * angle.sin(),
        ));
    }
    painter.add(egui::Shape::line(points, stroke));
}

fn sample_local_adjust_rgb(app: &App, fs_idx: usize, norm: [f32; 2]) -> Option<[u8; 3]> {
    let pixels = app.current_local_adjust_source_pixels(fs_idx)?;
    let [w, h] = pixels.size;
    if w == 0 || h == 0 {
        return None;
    }
    let x = (norm[0].clamp(0.0, 1.0) * (w.saturating_sub(1)) as f32).round() as usize;
    let y = (norm[1].clamp(0.0, 1.0) * (h.saturating_sub(1)) as f32).round() as usize;
    let color = pixels.pixels[y.min(h - 1) * w + x.min(w - 1)];
    Some([color.r(), color.g(), color.b()])
}

fn local_adjust_subject_mask_has_content(mask: &local_adjust_core::SubjectMask) -> bool {
    mask.alpha.iter().any(|&alpha| alpha > 0.02)
        || mask
            .source_alpha
            .as_ref()
            .is_some_and(|alpha| alpha.iter().any(|&value| value > 0.02))
}

fn local_adjust_subject_mask_candidate_from_layers(
    layers: &[local_adjust_core::LocalAdjustmentLayer],
    image_dims: (usize, usize),
) -> Option<local_adjust_core::RasterMask> {
    layers.iter().find_map(|layer| match &layer.mask {
        local_adjust_core::LocalMask::Subject(mask)
            if mask.width == image_dims.0
                && mask.height == image_dims.1
                && local_adjust_subject_mask_has_content(mask) =>
        {
            Some(mask.current_raster_mask())
        }
        _ => None,
    })
}

fn build_local_adjust_u2netp_input(
    source: &egui::ColorImage,
) -> Result<ndarray::Array4<f32>, String> {
    let [width, height] = source.size;
    if width == 0 || height == 0 || source.pixels.len() != width.saturating_mul(height) {
        return Err("invalid source image".to_string());
    }
    let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
    for color in &source.pixels {
        rgb.extend_from_slice(&[color.r(), color.g(), color.b()]);
    }
    let Some(rgb_image) = image::RgbImage::from_raw(width as u32, height as u32, rgb) else {
        return Err("invalid source RGB buffer".to_string());
    };
    let resized = image::imageops::resize(
        &rgb_image,
        LOCAL_ADJUST_U2NETP_INPUT_SIZE as u32,
        LOCAL_ADJUST_U2NETP_INPUT_SIZE as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut input = ndarray::Array4::<f32>::zeros((
        1,
        3,
        LOCAL_ADJUST_U2NETP_INPUT_SIZE,
        LOCAL_ADJUST_U2NETP_INPUT_SIZE,
    ));
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    for y in 0..LOCAL_ADJUST_U2NETP_INPUT_SIZE {
        for x in 0..LOCAL_ADJUST_U2NETP_INPUT_SIZE {
            let p = resized.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                let v = p[c] as f32 / 255.0;
                input[[0, c, y, x]] = (v - mean[c]) / std[c];
            }
        }
    }
    Ok(input)
}

fn local_adjust_u2netp_output_size(shape: &[i64], raw_len: usize) -> (usize, usize) {
    if shape.len() >= 2 {
        let h = shape[shape.len() - 2].max(1) as usize;
        let w = shape[shape.len() - 1].max(1) as usize;
        if h.saturating_mul(w) <= raw_len {
            return (w, h);
        }
    }
    let side = (raw_len as f64).sqrt().round().max(1.0) as usize;
    if side.saturating_mul(side) == raw_len {
        (side, side)
    } else {
        (
            LOCAL_ADJUST_U2NETP_INPUT_SIZE,
            raw_len.max(1).div_ceil(LOCAL_ADJUST_U2NETP_INPUT_SIZE),
        )
    }
}

fn normalize_local_adjust_u2netp_output(raw: &[f32], width: usize, height: usize) -> Vec<f32> {
    let len = width.saturating_mul(height).min(raw.len());
    if len == 0 {
        return vec![0.0; width.saturating_mul(height)];
    }
    let offset = raw.len().saturating_sub(width.saturating_mul(height));
    let values = &raw[offset..offset + len];
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for &v in values {
        if v.is_finite() {
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
    }
    let range = max_v - min_v;
    let mut out = vec![0.0; width.saturating_mul(height)];
    for (idx, slot) in out.iter_mut().enumerate().take(len) {
        let value = values[idx];
        *slot = if range.is_finite() && range > 1.0e-6 {
            ((value - min_v) / range).clamp(0.0, 1.0)
        } else {
            value.clamp(0.0, 1.0)
        };
    }
    out
}

fn resize_local_adjust_mask_bilinear(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }
    let mut dst = vec![0.0; dst_w.saturating_mul(dst_h)];
    let scale_x = if dst_w > 1 {
        (src_w.saturating_sub(1)) as f32 / (dst_w.saturating_sub(1)) as f32
    } else {
        0.0
    };
    let scale_y = if dst_h > 1 {
        (src_h.saturating_sub(1)) as f32 / (dst_h.saturating_sub(1)) as f32
    } else {
        0.0
    };
    for y in 0..dst_h {
        let sy = y as f32 * scale_y;
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let fy = sy - y0 as f32;
        for x in 0..dst_w {
            let sx = x as f32 * scale_x;
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let fx = sx - x0 as f32;
            let a00 = src[y0 * src_w + x0];
            let a10 = src[y0 * src_w + x1];
            let a01 = src[y1 * src_w + x0];
            let a11 = src[y1 * src_w + x1];
            let top = a00 + (a10 - a00) * fx;
            let bottom = a01 + (a11 - a01) * fx;
            dst[y * dst_w + x] = (top + (bottom - top) * fy).clamp(0.0, 1.0);
        }
    }
    dst
}

fn run_local_adjust_u2netp_segmentation(
    runtime: Arc<crate::ai::runtime::AiRuntime>,
    model_path: std::path::PathBuf,
    source: Arc<egui::ColorImage>,
) -> Result<local_adjust_core::RasterMask, String> {
    runtime
        .load_model_cpu(crate::ai::ModelKind::SubjectU2Netp, &model_path)
        .map_err(|err| format!("U²-Netp load: {err}"))?;
    let input = build_local_adjust_u2netp_input(&source)?;
    let input_tensor =
        ort::value::Tensor::from_array(input).map_err(|err| format!("Tensor creation: {err}"))?;
    let (shape, raw) = runtime
        .with_session(crate::ai::ModelKind::SubjectU2Netp, |session| {
            let outputs = session
                .run(ort::inputs![input_tensor])
                .map_err(|err| crate::ai::AiError::Ort(format!("U²-Netp run: {err}")))?;
            let (shape, raw) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|err| crate::ai::AiError::Ort(format!("U²-Netp extract: {err}")))?;
            Ok((shape.iter().copied().collect::<Vec<i64>>(), raw.to_vec()))
        })
        .map_err(|err| err.to_string())?;
    let (small_w, small_h) = local_adjust_u2netp_output_size(&shape, raw.len());
    let small_mask = normalize_local_adjust_u2netp_output(&raw, small_w, small_h);
    let alpha = resize_local_adjust_mask_bilinear(
        &small_mask,
        small_w,
        small_h,
        source.size[0],
        source.size[1],
    );
    Ok(local_adjust_core::RasterMask {
        width: source.size[0],
        height: source.size[1],
        alpha,
    })
}

fn local_adjust_region_boundary_mask(
    source: &egui::ColorImage,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> Vec<u8> {
    let [width, height] = source.size;
    let mut out = vec![0_u8; width.saturating_mul(height)];
    for y in 0..height {
        for x in 0..width {
            if local_adjust_boundary_pixel_at(source, x, y, edge_threshold, ink_threshold, gap_px) {
                out[y * width + x] = 1;
            }
        }
    }
    out
}

fn local_adjust_region_source_rgb_at_index(source: &egui::ColorImage, idx: usize) -> [u8; 3] {
    let color = source
        .pixels
        .get(idx)
        .copied()
        .unwrap_or(egui::Color32::BLACK);
    [color.r(), color.g(), color.b()]
}

fn local_adjust_region_color_close(a: [u8; 3], b: [u8; 3], tolerance: f32) -> bool {
    let max_delta = (a[0] as f32 - b[0] as f32)
        .abs()
        .max((a[1] as f32 - b[1] as f32).abs())
        .max((a[2] as f32 - b[2] as f32).abs());
    max_delta <= tolerance
}

fn local_adjust_region_neighbors(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut out = [(usize::MAX, usize::MAX); 4];
    let mut len = 0;
    if x > 0 {
        out[len] = (x - 1, y);
        len += 1;
    }
    if x + 1 < width {
        out[len] = (x + 1, y);
        len += 1;
    }
    if y > 0 {
        out[len] = (x, y - 1);
        len += 1;
    }
    if y + 1 < height {
        out[len] = (x, y + 1);
        len += 1;
    }
    out.into_iter().take(len)
}

fn local_adjust_region_membership_allowed(
    source: &egui::ColorImage,
    subject: Option<&local_adjust_core::RasterMask>,
    scope: LocalAdjustRegionSegmentationScope,
    idx: usize,
) -> bool {
    if source
        .pixels
        .get(idx)
        .map(|color| color.a() < 8)
        .unwrap_or(true)
    {
        return false;
    }
    match scope {
        LocalAdjustRegionSegmentationScope::Full => true,
        LocalAdjustRegionSegmentationScope::Subject => subject
            .map(|mask| mask.alpha.get(idx).copied().unwrap_or(0.0) > 0.18)
            .unwrap_or(false),
        LocalAdjustRegionSegmentationScope::Background => subject
            .map(|mask| mask.alpha.get(idx).copied().unwrap_or(0.0) <= 0.18)
            .unwrap_or(false),
    }
}

fn local_adjust_region_seed_allowed(
    source: &egui::ColorImage,
    subject: Option<&local_adjust_core::RasterMask>,
    scope: LocalAdjustRegionSegmentationScope,
    boundary: &[u8],
    idx: usize,
) -> bool {
    boundary.get(idx).copied().unwrap_or(0) == 0
        && local_adjust_region_membership_allowed(source, subject, scope, idx)
}

fn fill_local_adjust_unlabeled_region_pixels(
    labels: &mut [u32],
    width: usize,
    height: usize,
    allowed: &[bool],
) {
    let mut queue = VecDeque::new();
    let len = width
        .saturating_mul(height)
        .min(labels.len())
        .min(allowed.len());
    for idx in 0..len {
        if labels[idx] != 0 {
            queue.push_back(idx);
        }
    }
    while let Some(idx) = queue.pop_front() {
        let label = labels[idx];
        if label == 0 {
            continue;
        }
        let x = idx % width;
        let y = idx / width;
        for (nx, ny) in local_adjust_region_neighbors(x, y, width, height) {
            let nidx = ny * width + nx;
            if nidx >= len || !allowed[nidx] || labels[nidx] != 0 {
                continue;
            }
            labels[nidx] = label;
            queue.push_back(nidx);
        }
    }
}

fn build_local_adjust_region_segmentation(
    source: &egui::ColorImage,
    subject: Option<&local_adjust_core::RasterMask>,
    scope: LocalAdjustRegionSegmentationScope,
    color_tolerance: f32,
    min_area: usize,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> Result<local_adjust_core::RegionMask, String> {
    let [width, height] = source.size;
    let len = width.saturating_mul(height);
    if len == 0 || source.pixels.len() != len {
        return Err("invalid source image".to_string());
    }
    if let Some(mask) = subject
        && (mask.width != width || mask.height != height || mask.alpha.len() != len)
    {
        return Err("subject mask size does not match image".to_string());
    }
    let boundary = local_adjust_region_boundary_mask(source, edge_threshold, ink_threshold, gap_px);
    let mut visited = vec![false; len];
    let mut labels = vec![0_u32; len];
    let mut label = 0_u32;
    let tolerance = color_tolerance.max(0.0);
    let min_area = min_area.max(1);
    let mut queue = VecDeque::new();
    let mut component = Vec::new();

    for start in 0..len {
        if visited[start] {
            continue;
        }
        if !local_adjust_region_seed_allowed(source, subject, scope, &boundary, start) {
            visited[start] = true;
            continue;
        }
        let seed = local_adjust_region_source_rgb_at_index(source, start);
        visited[start] = true;
        queue.clear();
        component.clear();
        queue.push_back(start);
        while let Some(idx) = queue.pop_front() {
            component.push(idx);
            let x = idx % width;
            let y = idx / width;
            for (nx, ny) in local_adjust_region_neighbors(x, y, width, height) {
                let nidx = ny * width + nx;
                if visited[nidx] {
                    continue;
                }
                if !local_adjust_region_seed_allowed(source, subject, scope, &boundary, nidx) {
                    visited[nidx] = true;
                    continue;
                }
                if local_adjust_region_color_close(
                    seed,
                    local_adjust_region_source_rgb_at_index(source, nidx),
                    tolerance,
                ) {
                    visited[nidx] = true;
                    queue.push_back(nidx);
                }
            }
        }
        if component.len() >= min_area && (label as usize) < LOCAL_ADJUST_REGION_SEGMENT_MAX_LABELS
        {
            label += 1;
            for &idx in &component {
                labels[idx] = label;
            }
        }
    }

    let membership_allowed: Vec<bool> = (0..len)
        .map(|idx| local_adjust_region_membership_allowed(source, subject, scope, idx))
        .collect();
    fill_local_adjust_unlabeled_region_pixels(&mut labels, width, height, &membership_allowed);

    Ok(local_adjust_core::RegionMask {
        width,
        height,
        labels,
        selected: vec![false; label as usize + 1],
    })
}

fn local_adjust_norm_to_pixel(norm: [f32; 2], width: usize, height: usize) -> (f32, f32) {
    (
        norm[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32,
        norm[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32,
    )
}

fn paint_local_adjust_alpha_line(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    from_norm: [f32; 2],
    to_norm: [f32; 2],
    radius: f32,
    paint: bool,
) -> bool {
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) {
        return false;
    }
    let from = local_adjust_norm_to_pixel(from_norm, width, height);
    let to = local_adjust_norm_to_pixel(to_norm, width, height);
    let radius = radius.max(1.0);
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let steps = (dist / (radius * 0.5)).ceil().max(1.0) as usize;
    let value = if paint { 1.0 } else { 0.0 };
    let mut changed = false;

    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let cx = from.0 + dx * t;
        let cy = from.1 + dy * t;
        let x0 = ((cx - radius).floor().max(0.0).min(width as f32)) as usize;
        let y0 = ((cy - radius).floor().max(0.0).min(height as f32)) as usize;
        let x1 = ((cx + radius).ceil().max(0.0).min(width as f32)) as usize;
        let y1 = ((cy + radius).ceil().max(0.0).min(height as f32)) as usize;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let radius_sq = radius * radius;
        for py in y0..y1 {
            for px in x0..x1 {
                let ddx = px as f32 + 0.5 - cx;
                let ddy = py as f32 + 0.5 - cy;
                if ddx * ddx + ddy * ddy <= radius_sq {
                    let idx = py * width + px;
                    if (alpha[idx] - value).abs() > f32::EPSILON {
                        alpha[idx] = value;
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

fn local_adjust_luma_at(image: &egui::ColorImage, x: usize, y: usize) -> f32 {
    let idx = y.saturating_mul(image.size[0]).saturating_add(x);
    let color = image
        .pixels
        .get(idx)
        .copied()
        .unwrap_or(egui::Color32::BLACK);
    color.r() as f32 * 0.299 + color.g() as f32 * 0.587 + color.b() as f32 * 0.114
}

fn local_adjust_luma_offset(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    dx: isize,
    dy: isize,
) -> Option<f32> {
    let nx = x as isize + dx;
    let ny = y as isize + dy;
    if nx < 0 || ny < 0 || nx >= image.size[0] as isize || ny >= image.size[1] as isize {
        return None;
    }
    Some(local_adjust_luma_at(image, nx as usize, ny as usize))
}

fn local_adjust_edge_strength_at(image: &egui::ColorImage, x: usize, y: usize) -> f32 {
    let width = image.size[0];
    let height = image.size[1];
    let xm = x.saturating_sub(1);
    let xp = (x + 1).min(width.saturating_sub(1));
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(height.saturating_sub(1));
    let left = local_adjust_luma_at(image, xm, y);
    let right = local_adjust_luma_at(image, xp, y);
    let top = local_adjust_luma_at(image, x, ym);
    let bottom = local_adjust_luma_at(image, x, yp);
    ((right - left).powi(2) + (bottom - top).powi(2)).sqrt()
}

fn local_adjust_line_interior_strength_at(image: &egui::ColorImage, x: usize, y: usize) -> f32 {
    if image.size[0] == 0 || image.size[1] == 0 {
        return 0.0;
    }
    let center = local_adjust_luma_at(image, x, y);
    let radius = 3_isize;
    let mut best = 0.0_f32;
    for (dx, dy) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
        let Some(a) = local_adjust_luma_offset(image, x, y, dx * radius, dy * radius) else {
            continue;
        };
        let Some(b) = local_adjust_luma_offset(image, x, y, -dx * radius, -dy * radius) else {
            continue;
        };
        let dark_line = (a - center).min(b - center);
        let bright_line = (center - a).min(center - b);
        best = best.max(dark_line.max(bright_line).max(0.0));
    }
    best
}

fn local_adjust_raw_boundary_pixel_at(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    edge_threshold: f32,
    ink_threshold: f32,
) -> bool {
    local_adjust_edge_strength_at(image, x, y) >= edge_threshold
        || local_adjust_line_interior_strength_at(image, x, y) >= ink_threshold
}

fn local_adjust_nearest_boundary_distance(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    dx: isize,
    dy: isize,
    edge_threshold: f32,
    ink_threshold: f32,
    max_distance: usize,
) -> Option<usize> {
    for step in 1..=max_distance {
        let nx = x as isize + dx * step as isize;
        let ny = y as isize + dy * step as isize;
        if nx < 0 || ny < 0 || nx >= image.size[0] as isize || ny >= image.size[1] as isize {
            return None;
        }
        if local_adjust_raw_boundary_pixel_at(
            image,
            nx as usize,
            ny as usize,
            edge_threshold,
            ink_threshold,
        ) {
            return Some(step);
        }
    }
    None
}

fn local_adjust_boundary_gap_bridge_at(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    edge_threshold: f32,
    ink_threshold: f32,
    max_gap: usize,
) -> bool {
    for ((dx0, dy0), (dx1, dy1)) in [
        ((-1, 0), (1, 0)),
        ((0, -1), (0, 1)),
        ((-1, -1), (1, 1)),
        ((-1, 1), (1, -1)),
    ] {
        let a = local_adjust_nearest_boundary_distance(
            image,
            x,
            y,
            dx0,
            dy0,
            edge_threshold,
            ink_threshold,
            max_gap,
        );
        let b = local_adjust_nearest_boundary_distance(
            image,
            x,
            y,
            dx1,
            dy1,
            edge_threshold,
            ink_threshold,
            max_gap,
        );
        if let (Some(a), Some(b)) = (a, b)
            && a + b <= max_gap + 1
        {
            return true;
        }
    }
    false
}

fn local_adjust_boundary_pixel_at(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> bool {
    local_adjust_raw_boundary_pixel_at(image, x, y, edge_threshold, ink_threshold)
        || (gap_px > 0
            && local_adjust_boundary_gap_bridge_at(
                image,
                x,
                y,
                edge_threshold,
                ink_threshold,
                gap_px,
            ))
}

fn local_adjust_edge_brush_pixel_allowed(
    image: &egui::ColorImage,
    x: usize,
    y: usize,
    seed: [u8; 3],
    tolerance: i16,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> bool {
    let idx = y.saturating_mul(image.size[0]).saturating_add(x);
    let color = image
        .pixels
        .get(idx)
        .copied()
        .unwrap_or(egui::Color32::BLACK);
    let rgb = [color.r(), color.g(), color.b()];
    let max_delta = seed
        .iter()
        .zip(rgb)
        .map(|(&a, b)| (a as i16 - b as i16).abs())
        .max()
        .unwrap_or(0);
    max_delta <= tolerance
        && !local_adjust_boundary_pixel_at(image, x, y, edge_threshold, ink_threshold, gap_px)
}

fn include_local_adjust_adjacent_boundary_pixels(
    image: &egui::ColorImage,
    targets: &mut Vec<usize>,
    target_map: &mut [bool],
    bounds: (usize, usize, usize, usize),
    bw: usize,
    center: [f32; 2],
    radius_sq: f32,
    thresholds: (f32, f32, usize),
) {
    if bw == 0 {
        return;
    }
    let (min_x, max_x, min_y, max_y) = bounds;
    let (edge_threshold, ink_threshold, gap_px) = thresholds;
    let initial_len = targets.len();
    let include_radius = LOCAL_ADJUST_EDGE_BRUSH_INCLUDE_BOUNDARY_RADIUS;
    let include_radius_sq = include_radius * include_radius;
    for i in 0..initial_len {
        let src_idx = targets[i];
        let x = src_idx % image.size[0];
        let y = src_idx / image.size[0];
        for dy in -include_radius..=include_radius {
            for dx in -include_radius..=include_radius {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if dx * dx + dy * dy > include_radius_sq {
                    continue;
                }
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < min_x as isize
                    || ny < min_y as isize
                    || nx > max_x as isize
                    || ny > max_y as isize
                {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;
                let local_idx = (ny - min_y) * bw + (nx - min_x);
                if target_map.get(local_idx).copied().unwrap_or(false) {
                    continue;
                }
                let brush_dx = nx as f32 + 0.5 - center[0];
                let brush_dy = ny as f32 + 0.5 - center[1];
                if brush_dx * brush_dx + brush_dy * brush_dy > radius_sq {
                    continue;
                }
                if local_adjust_boundary_pixel_at(
                    image,
                    nx,
                    ny,
                    edge_threshold,
                    ink_threshold,
                    gap_px,
                ) {
                    target_map[local_idx] = true;
                    targets.push(ny * image.size[0] + nx);
                }
            }
        }
    }
}

fn paint_local_adjust_edge_brush_stamp(
    alpha: &mut [f32],
    image: &egui::ColorImage,
    center: [f32; 2],
    radius: f32,
    paint: bool,
    seed: [u8; 3],
    tolerance: i16,
    thresholds: (f32, f32, usize),
    include_boundary: bool,
) -> bool {
    let (width, height) = (image.size[0], image.size[1]);
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) {
        return false;
    }
    let (edge_threshold, ink_threshold, gap_px) = thresholds;
    let min_x = (center[0] - radius).floor().max(0.0) as usize;
    let max_x = (center[0] + radius).ceil().min(width as f32 - 1.0) as usize;
    let min_y = (center[1] - radius).floor().max(0.0) as usize;
    let max_y = (center[1] + radius).ceil().min(height as f32 - 1.0) as usize;
    let radius_sq = radius * radius;
    let start_x = center[0].floor().clamp(min_x as f32, max_x as f32) as usize;
    let start_y = center[1].floor().clamp(min_y as f32, max_y as f32) as usize;
    if !local_adjust_edge_brush_pixel_allowed(
        image,
        start_x,
        start_y,
        seed,
        tolerance,
        edge_threshold,
        ink_threshold,
        gap_px,
    ) {
        return false;
    }

    let bw = max_x - min_x + 1;
    let bh = max_y - min_y + 1;
    let mut visited = vec![false; bw.saturating_mul(bh)];
    let mut target_map = vec![false; bw.saturating_mul(bh)];
    let mut queue = vec![(start_x, start_y)];
    visited[(start_y - min_y) * bw + (start_x - min_x)] = true;
    let mut targets = Vec::new();
    while let Some((x, y)) = queue.pop() {
        let dx = x as f32 + 0.5 - center[0];
        let dy = y as f32 + 0.5 - center[1];
        if dx * dx + dy * dy > radius_sq {
            continue;
        }
        if !local_adjust_edge_brush_pixel_allowed(
            image,
            x,
            y,
            seed,
            tolerance,
            edge_threshold,
            ink_threshold,
            gap_px,
        ) {
            continue;
        }
        targets.push(y * width + x);
        target_map[(y - min_y) * bw + (x - min_x)] = true;
        for (nx, ny) in [
            (x.saturating_sub(1), y),
            ((x + 1).min(max_x), y),
            (x, y.saturating_sub(1)),
            (x, (y + 1).min(max_y)),
        ] {
            if nx < min_x || nx > max_x || ny < min_y || ny > max_y {
                continue;
            }
            let local_idx = (ny - min_y) * bw + (nx - min_x);
            if !visited[local_idx] {
                visited[local_idx] = true;
                queue.push((nx, ny));
            }
        }
    }
    if targets.is_empty() {
        return false;
    }
    if include_boundary {
        include_local_adjust_adjacent_boundary_pixels(
            image,
            &mut targets,
            &mut target_map,
            (min_x, max_x, min_y, max_y),
            bw,
            center,
            radius_sq,
            thresholds,
        );
    }

    let value = if paint { 1.0 } else { 0.0 };
    let mut changed = false;
    for idx in targets {
        if let Some(alpha) = alpha.get_mut(idx) {
            let before = *alpha;
            *alpha = value;
            changed |= (*alpha - before).abs() > f32::EPSILON;
        }
    }
    changed
}

fn paint_local_adjust_alpha_edge_brush_line(
    alpha: &mut [f32],
    image: &egui::ColorImage,
    from_norm: [f32; 2],
    to_norm: [f32; 2],
    radius: f32,
    paint: bool,
    seed: Option<[u8; 3]>,
    tolerance: f32,
    thresholds: (f32, f32, usize),
    include_boundary: bool,
) -> bool {
    let Some(seed) = seed else {
        return false;
    };
    let (width, height) = (image.size[0], image.size[1]);
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) {
        return false;
    }
    let from = local_adjust_norm_to_pixel(from_norm, width, height);
    let to = local_adjust_norm_to_pixel(to_norm, width, height);
    let radius = radius.max(1.0);
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let steps = (dist / (radius * 0.5)).ceil().max(1.0) as usize;
    let tolerance = tolerance.clamp(0.0, 255.0).round() as i16;
    let mut changed = false;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let center = [from.0 + dx * t, from.1 + dy * t];
        changed |= paint_local_adjust_edge_brush_stamp(
            alpha,
            image,
            center,
            radius,
            paint,
            seed,
            tolerance,
            thresholds,
            include_boundary,
        );
    }
    changed
}

fn local_adjust_nearest_mask_distance(
    alpha: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    dx: isize,
    dy: isize,
    max_distance: usize,
) -> Option<usize> {
    for step in 1..=max_distance {
        let nx = x as isize + dx * step as isize;
        let ny = y as isize + dy * step as isize;
        if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
            return None;
        }
        let idx = ny as usize * width + nx as usize;
        if alpha.get(idx).copied().unwrap_or(0.0) > 0.5 {
            return Some(step);
        }
    }
    None
}

fn local_adjust_gap_between_masked_pixels(
    alpha: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    max_gap: usize,
) -> bool {
    if width == 0 || height == 0 || max_gap == 0 {
        return false;
    }
    let left = local_adjust_nearest_mask_distance(alpha, width, height, x, y, -1, 0, max_gap);
    let right = local_adjust_nearest_mask_distance(alpha, width, height, x, y, 1, 0, max_gap);
    if let (Some(l), Some(r)) = (left, right)
        && l + r <= max_gap + 1
    {
        return true;
    }
    let up = local_adjust_nearest_mask_distance(alpha, width, height, x, y, 0, -1, max_gap);
    let down = local_adjust_nearest_mask_distance(alpha, width, height, x, y, 0, 1, max_gap);
    if let (Some(u), Some(d)) = (up, down)
        && u + d <= max_gap + 1
    {
        return true;
    }
    false
}

fn paint_local_adjust_gap_fill_stamp(
    alpha: &mut [f32],
    src: &[f32],
    width: usize,
    height: usize,
    center: [f32; 2],
    radius: f32,
    gap: usize,
) -> bool {
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) {
        return false;
    }
    let min_x = (center[0] - radius).floor().max(0.0) as usize;
    let max_x = (center[0] + radius).ceil().min(width as f32 - 1.0) as usize;
    let min_y = (center[1] - radius).floor().max(0.0) as usize;
    let max_y = (center[1] + radius).ceil().min(height as f32 - 1.0) as usize;
    let radius_sq = radius * radius;
    let mut changed = false;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - center[0];
            let dy = y as f32 + 0.5 - center[1];
            if dx * dx + dy * dy > radius_sq {
                continue;
            }
            let idx = y * width + x;
            if src.get(idx).copied().unwrap_or(0.0) > 0.5 {
                continue;
            }
            if local_adjust_gap_between_masked_pixels(src, width, height, x, y, gap) {
                let before = alpha[idx];
                alpha[idx] = 1.0;
                changed |= (alpha[idx] - before).abs() > f32::EPSILON;
            }
        }
    }
    changed
}

fn paint_local_adjust_alpha_gap_fill_line(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    from_norm: [f32; 2],
    to_norm: [f32; 2],
    radius: f32,
    paint: bool,
    gap: f32,
) -> bool {
    if !paint {
        return paint_local_adjust_alpha_line(
            alpha, width, height, from_norm, to_norm, radius, false,
        );
    }
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) {
        return false;
    }
    let from = local_adjust_norm_to_pixel(from_norm, width, height);
    let to = local_adjust_norm_to_pixel(to_norm, width, height);
    let radius = radius.max(1.0);
    let gap = gap.round().clamp(1.0, 64.0) as usize;
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let steps = (dist / (radius * 0.5)).ceil().max(1.0) as usize;
    let src = alpha.to_vec();
    let mut changed = false;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let center = [from.0 + dx * t, from.1 + dy * t];
        changed |=
            paint_local_adjust_gap_fill_stamp(alpha, &src, width, height, center, radius, gap);
    }
    changed
}

fn fill_local_adjust_alpha_polygon(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    points: &[[f32; 2]],
    paint: bool,
) -> bool {
    if width == 0 || height == 0 || alpha.len() < width.saturating_mul(height) || points.len() < 3 {
        return false;
    }
    let value = if paint { 1.0 } else { 0.0 };
    let mut changed = false;
    let min_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(height.saturating_sub(1) as f32) as usize;

    for y in min_y..=max_y {
        let scan_y = y as f32 + 0.5;
        let mut xs = Vec::new();
        for i in 0..points.len() {
            let p0 = points[i];
            let p1 = points[(i + 1) % points.len()];
            if (p0[1] <= scan_y && p1[1] > scan_y) || (p1[1] <= scan_y && p0[1] > scan_y) {
                let t = (scan_y - p0[1]) / (p1[1] - p0[1]);
                xs.push(p0[0] + t * (p1[0] - p0[0]));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            let x0 = pair[0].floor().max(0.0) as usize;
            let x1 = pair[1].ceil().min(width as f32) as usize;
            for x in x0..x1 {
                let idx = y * width + x;
                if (alpha[idx] - value).abs() > f32::EPSILON {
                    alpha[idx] = value;
                    changed = true;
                }
            }
        }
    }
    changed
}

fn make_local_adjust_shape(
    tool: LocalAdjustMaskTool,
    start_norm: [f32; 2],
    end_norm: [f32; 2],
    line_width: f32,
    image_dims: (usize, usize),
    paint: bool,
) -> Option<local_adjust_core::MaskShape> {
    let (w, h) = (image_dims.0.max(1), image_dims.1.max(1));
    let start = local_adjust_norm_to_pixel(start_norm, w, h);
    let end = local_adjust_norm_to_pixel(end_norm, w, h);
    let op = if paint {
        local_adjust_core::ShapeOp::Add
    } else {
        local_adjust_core::ShapeOp::Subtract
    };
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    match tool {
        LocalAdjustMaskTool::Line => {
            if dx * dx + dy * dy <= 4.0 {
                return None;
            }
            Some(local_adjust_core::MaskShape::Line {
                op,
                kind: local_adjust_core::LineKind::Diagonal,
                p0: [start.0, start.1],
                p1: [end.0, end.1],
                thickness: line_width.max(1.0),
            })
        }
        LocalAdjustMaskTool::VertLine => {
            let lx = start.0.min(end.0);
            let rx = start.0.max(end.0);
            let thickness = (rx - lx).max(1.0);
            let cx = (lx + rx) * 0.5;
            Some(local_adjust_core::MaskShape::Line {
                op,
                kind: local_adjust_core::LineKind::Vertical,
                p0: [cx, 0.0],
                p1: [cx, h as f32],
                thickness,
            })
        }
        LocalAdjustMaskTool::HorizLine => {
            let ty = start.1.min(end.1);
            let by = start.1.max(end.1);
            let thickness = (by - ty).max(1.0);
            let cy = (ty + by) * 0.5;
            Some(local_adjust_core::MaskShape::Line {
                op,
                kind: local_adjust_core::LineKind::Horizontal,
                p0: [0.0, cy],
                p1: [w as f32, cy],
                thickness,
            })
        }
        LocalAdjustMaskTool::Rect => {
            if dx.abs() <= 1.0 || dy.abs() <= 1.0 {
                return None;
            }
            Some(local_adjust_core::MaskShape::Rect {
                op,
                center: [(start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5],
                half_w: dx.abs() * 0.5,
                half_h: dy.abs() * 0.5,
                rotation_rad: 0.0,
            })
        }
        LocalAdjustMaskTool::Ellipse => {
            if dx.abs() <= 1.0 || dy.abs() <= 1.0 {
                return None;
            }
            Some(local_adjust_core::MaskShape::Ellipse {
                op,
                center: [(start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5],
                rx: dx.abs() * 0.5,
                ry: dy.abs() * 0.5,
                rotation_rad: 0.0,
            })
        }
        _ => None,
    }
}

fn draw_local_adjust_ellipse(
    painter: &egui::Painter,
    image_rect: egui::Rect,
    image_dims: (usize, usize),
    zoom_pan: Option<(f32, egui::Vec2)>,
    center: [f32; 2],
    rx: f32,
    ry: f32,
    stroke: egui::Stroke,
) {
    let mut points = Vec::with_capacity(73);
    for i in 0..=72 {
        let t = i as f32 / 72.0 * std::f32::consts::TAU;
        let norm = [center[0] + rx * t.cos(), center[1] + ry * t.sin()];
        if let Some(pos) = local_adjust_norm_to_screen(norm, image_rect, image_dims, zoom_pan) {
            points.push(pos);
        }
    }
    if points.len() >= 2 {
        painter.add(egui::Shape::line(points, stroke));
    }
}

fn local_adjust_rotated_corners(
    center: [f32; 2],
    half_w: f32,
    half_h: f32,
    rotation_rad: f32,
) -> [[f32; 2]; 4] {
    let (s, c) = rotation_rad.sin_cos();
    [
        [-half_w, -half_h],
        [half_w, -half_h],
        [half_w, half_h],
        [-half_w, half_h],
    ]
    .map(|p| {
        [
            center[0] + p[0] * c - p[1] * s,
            center[1] + p[0] * s + p[1] * c,
        ]
    })
}

fn draw_local_adjust_shape_outline(
    painter: &egui::Painter,
    shape: local_adjust_core::MaskShape,
    to_screen: &impl Fn([f32; 2]) -> egui::Pos2,
    color: egui::Color32,
    selected: bool,
) {
    let stroke = egui::Stroke::new(if selected { 2.0 } else { 1.3 }, color);
    match shape {
        local_adjust_core::MaskShape::Line { p0, p1, .. } => {
            painter.line_segment([to_screen(p0), to_screen(p1)], stroke);
        }
        local_adjust_core::MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let points = local_adjust_rotated_corners(center, half_w, half_h, rotation_rad)
                .into_iter()
                .map(to_screen)
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        local_adjust_core::MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let mut points = Vec::with_capacity(64);
            let (s, c) = rotation_rad.sin_cos();
            for i in 0..64 {
                let t = i as f32 / 64.0 * std::f32::consts::TAU;
                let x = rx * t.cos();
                let y = ry * t.sin();
                points.push(to_screen([
                    center[0] + x * c - y * s,
                    center[1] + x * s + y * c,
                ]));
            }
            painter.add(egui::Shape::closed_line(points, stroke));
        }
    }
}

fn local_adjust_axis_corners(center: [f32; 2], half_w: f32, half_h: f32) -> [[f32; 2]; 4] {
    [
        [center[0] - half_w, center[1] - half_h],
        [center[0] + half_w, center[1] - half_h],
        [center[0] + half_w, center[1] + half_h],
        [center[0] - half_w, center[1] + half_h],
    ]
}

fn local_adjust_inverse_rotate_point(p: [f32; 2], center: [f32; 2], rotation_rad: f32) -> [f32; 2] {
    let (s, c) = (-rotation_rad).sin_cos();
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    [center[0] + dx * c - dy * s, center[1] + dx * s + dy * c]
}

fn local_adjust_dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

fn local_adjust_distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let denom = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if denom <= f32::EPSILON {
        0.0
    } else {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / denom).clamp(0.0, 1.0)
    };
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    local_adjust_dist2(p, q).sqrt()
}

fn local_adjust_shape_contains(shape: local_adjust_core::MaskShape, p: [f32; 2]) -> bool {
    match shape {
        local_adjust_core::MaskShape::Line {
            p0, p1, thickness, ..
        } => local_adjust_distance_to_segment(p, p0, p1) <= thickness * 0.5 + 3.0,
        local_adjust_core::MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let local = local_adjust_inverse_rotate_point(p, center, rotation_rad);
            (local[0] - center[0]).abs() <= half_w && (local[1] - center[1]).abs() <= half_h
        }
        local_adjust_core::MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let local = local_adjust_inverse_rotate_point(p, center, rotation_rad);
            ((local[0] - center[0]) / rx).powi(2) + ((local[1] - center[1]) / ry).powi(2) <= 1.0
        }
    }
}

fn hit_local_adjust_shape_handles(
    shape: local_adjust_core::MaskShape,
    p: [f32; 2],
    hit_radius_px: f32,
) -> Option<LocalAdjustShapeHandle> {
    let r2 = hit_radius_px.max(12.0).powi(2);
    match shape {
        local_adjust_core::MaskShape::Line { p0, p1, .. } => {
            if local_adjust_dist2(p, p0) <= r2 {
                Some(LocalAdjustShapeHandle::LineStart)
            } else if local_adjust_dist2(p, p1) <= r2 {
                Some(LocalAdjustShapeHandle::LineEnd)
            } else {
                None
            }
        }
        local_adjust_core::MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => local_adjust_axis_corners(center, half_w, half_h)
            .iter()
            .enumerate()
            .find(|&(_, &corner)| local_adjust_dist2(p, corner) <= r2 && rotation_rad.abs() < 0.001)
            .map(|(i, _)| LocalAdjustShapeHandle::Corner(i as u8)),
        local_adjust_core::MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let handle = [center[0] + rx, center[1]];
            if local_adjust_dist2(p, handle) <= r2 && rotation_rad.abs() < 0.001 {
                Some(LocalAdjustShapeHandle::Radius)
            } else {
                local_adjust_axis_corners(center, rx, ry)
                    .iter()
                    .enumerate()
                    .find(|&(_, &corner)| {
                        local_adjust_dist2(p, corner) <= r2 && rotation_rad.abs() < 0.001
                    })
                    .map(|(i, _)| LocalAdjustShapeHandle::Corner(i as u8))
            }
        }
    }
}

fn draw_local_adjust_shape_handles(
    painter: &egui::Painter,
    shape: local_adjust_core::MaskShape,
    to_screen: &impl Fn([f32; 2]) -> egui::Pos2,
) {
    let fill = egui::Color32::from_rgb(255, 250, 210);
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(35, 25, 10));
    let draw = |p: [f32; 2]| {
        let screen = to_screen(p);
        painter.circle_filled(screen, 5.5, fill);
        painter.circle_stroke(screen, 5.5, stroke);
    };
    match shape {
        local_adjust_core::MaskShape::Line { p0, p1, .. } => {
            draw(p0);
            draw(p1);
        }
        local_adjust_core::MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            for p in local_adjust_rotated_corners(center, half_w, half_h, rotation_rad) {
                draw(p);
            }
        }
        local_adjust_core::MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            for p in local_adjust_rotated_corners(center, rx, ry, rotation_rad) {
                draw(p);
            }
            draw([center[0] + rx, center[1]]);
        }
    }
}

fn translate_local_adjust_shape(
    shape: local_adjust_core::MaskShape,
    dx: f32,
    dy: f32,
) -> local_adjust_core::MaskShape {
    match shape {
        local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0,
            p1,
            thickness,
        } => local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0: [p0[0] + dx, p0[1] + dy],
            p1: [p1[0] + dx, p1[1] + dy],
            thickness,
        },
        local_adjust_core::MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => local_adjust_core::MaskShape::Rect {
            op,
            center: [center[0] + dx, center[1] + dy],
            half_w,
            half_h,
            rotation_rad,
        },
        local_adjust_core::MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad,
        } => local_adjust_core::MaskShape::Ellipse {
            op,
            center: [center[0] + dx, center[1] + dy],
            rx,
            ry,
            rotation_rad,
        },
    }
}

fn constrain_local_adjust_line_endpoint(
    cur: [f32; 2],
    anchor: [f32; 2],
    modifiers: egui::Modifiers,
) -> [f32; 2] {
    if !modifiers.shift {
        return cur;
    }
    let dx = cur[0] - anchor[0];
    let dy = cur[1] - anchor[1];
    if dx.abs() > dy.abs() {
        [cur[0], anchor[1]]
    } else {
        [anchor[0], cur[1]]
    }
}

fn resize_local_adjust_axis_rect(
    op: local_adjust_core::ShapeOp,
    center: [f32; 2],
    half_w: f32,
    half_h: f32,
    rotation_rad: f32,
    corner: u8,
    cur: [f32; 2],
    modifiers: egui::Modifiers,
) -> Option<local_adjust_core::MaskShape> {
    if rotation_rad.abs() > 0.001 {
        return None;
    }
    let corners = local_adjust_axis_corners(center, half_w, half_h);
    let anchor = corners[((corner as usize) + 2) % 4];
    let cx = (anchor[0] + cur[0]) * 0.5;
    let cy = (anchor[1] + cur[1]) * 0.5;
    let mut next_half_w = (cur[0] - anchor[0]).abs() * 0.5;
    let mut next_half_h = (cur[1] - anchor[1]).abs() * 0.5;
    if modifiers.shift {
        let m = next_half_w.max(next_half_h);
        next_half_w = m;
        next_half_h = m;
    }
    Some(local_adjust_core::MaskShape::Rect {
        op,
        center: [cx, cy],
        half_w: next_half_w.max(1.0),
        half_h: next_half_h.max(1.0),
        rotation_rad,
    })
}

fn resize_local_adjust_axis_ellipse(
    op: local_adjust_core::ShapeOp,
    center: [f32; 2],
    rx: f32,
    ry: f32,
    rotation_rad: f32,
    corner: u8,
    cur: [f32; 2],
    modifiers: egui::Modifiers,
) -> Option<local_adjust_core::MaskShape> {
    if rotation_rad.abs() > 0.001 {
        return None;
    }
    let corners = local_adjust_axis_corners(center, rx, ry);
    let anchor = corners[((corner as usize) + 2) % 4];
    let cx = (anchor[0] + cur[0]) * 0.5;
    let cy = (anchor[1] + cur[1]) * 0.5;
    let mut next_rx = (cur[0] - anchor[0]).abs() * 0.5;
    let mut next_ry = (cur[1] - anchor[1]).abs() * 0.5;
    if modifiers.shift {
        let m = next_rx.max(next_ry);
        next_rx = m;
        next_ry = m;
    }
    Some(local_adjust_core::MaskShape::Ellipse {
        op,
        center: [cx, cy],
        rx: next_rx.max(1.0),
        ry: next_ry.max(1.0),
        rotation_rad,
    })
}

fn apply_local_adjust_shape_drag(
    drag: LocalAdjustMaskShapeDrag,
    cur: [f32; 2],
    modifiers: egui::Modifiers,
) -> local_adjust_core::MaskShape {
    let dx = cur[0] - drag.origin[0];
    let dy = cur[1] - drag.origin[1];
    match (drag.base, drag.handle) {
        (shape, LocalAdjustShapeHandle::Body) => translate_local_adjust_shape(shape, dx, dy),
        (
            local_adjust_core::MaskShape::Line {
                op,
                kind,
                p0: _,
                p1,
                thickness,
            },
            LocalAdjustShapeHandle::LineStart,
        ) => local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0: constrain_local_adjust_line_endpoint(cur, p1, modifiers),
            p1,
            thickness,
        },
        (
            local_adjust_core::MaskShape::Line {
                op,
                kind,
                p0,
                p1: _,
                thickness,
            },
            LocalAdjustShapeHandle::LineEnd,
        ) => local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0,
            p1: constrain_local_adjust_line_endpoint(cur, p0, modifiers),
            thickness,
        },
        (
            local_adjust_core::MaskShape::Rect {
                op,
                center,
                half_w,
                half_h,
                rotation_rad,
            },
            LocalAdjustShapeHandle::Corner(corner),
        ) => resize_local_adjust_axis_rect(
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
            corner,
            cur,
            modifiers,
        )
        .unwrap_or(drag.base),
        (
            local_adjust_core::MaskShape::Ellipse {
                op,
                center,
                rx,
                ry,
                rotation_rad,
            },
            LocalAdjustShapeHandle::Corner(corner),
        ) => resize_local_adjust_axis_ellipse(
            op,
            center,
            rx,
            ry,
            rotation_rad,
            corner,
            cur,
            modifiers,
        )
        .unwrap_or(drag.base),
        (
            local_adjust_core::MaskShape::Ellipse {
                op,
                center,
                rx: _,
                ry,
                rotation_rad,
            },
            LocalAdjustShapeHandle::Radius,
        ) => {
            let next_rx = (cur[0] - center[0]).abs().max(1.0);
            let next_ry = if modifiers.shift { next_rx } else { ry };
            local_adjust_core::MaskShape::Ellipse {
                op,
                center,
                rx: next_rx,
                ry: next_ry,
                rotation_rad,
            }
        }
        _ => drag.base,
    }
}

fn local_adjust_effect_center_mut(
    effect: &mut local_adjust_core::LocalEffect,
) -> Option<(&mut [f32; 2], &'static str)> {
    match effect {
        local_adjust_core::LocalEffect::TiltShift(params) => {
            Some((&mut params.center, "チルトシフト中心"))
        }
        local_adjust_core::LocalEffect::RadialBlur(params) => {
            Some((&mut params.center, "放射ぼかし中心"))
        }
        local_adjust_core::LocalEffect::WaveDistortion(params)
            if params.mode == local_adjust_core::WaveDistortionMode::Ripple =>
        {
            Some((&mut params.center, "波形中心"))
        }
        local_adjust_core::LocalEffect::PinchSpherize(params) => {
            Some((&mut params.center, "つまむ/魚眼中心"))
        }
        local_adjust_core::LocalEffect::Twirl(params) => Some((&mut params.center, "渦巻き中心")),
        local_adjust_core::LocalEffect::PolarCoordinates(params) => {
            Some((&mut params.center, "極座標中心"))
        }
        local_adjust_core::LocalEffect::LensCorrection(params) => {
            Some((&mut params.center, "レンズ補正中心"))
        }
        local_adjust_core::LocalEffect::GodRays(params) => Some((&mut params.center, "光源位置")),
        local_adjust_core::LocalEffect::LensFlare(params) => {
            Some((&mut params.center, "フレア光源位置"))
        }
        local_adjust_core::LocalEffect::LightLeak(params) => {
            Some((&mut params.center, "ライトリーク位置"))
        }
        local_adjust_core::LocalEffect::BacklightHaze(params) => {
            Some((&mut params.center, "逆光ヘイズ位置"))
        }
        local_adjust_core::LocalEffect::SpeedLines(params) => {
            Some((&mut params.center, "集中線/スピード線中心"))
        }
        local_adjust_core::LocalEffect::RadialFlash(params) => {
            Some((&mut params.center, "集中線フラッシュ中心"))
        }
        local_adjust_core::LocalEffect::Spotlight(params) => {
            Some((&mut params.center, "スポットライト位置"))
        }
        _ => None,
    }
}

fn local_adjust_effect_center(
    effect: &local_adjust_core::LocalEffect,
) -> Option<([f32; 2], &'static str)> {
    match effect {
        local_adjust_core::LocalEffect::TiltShift(params) => {
            Some((params.center, "チルトシフト中心"))
        }
        local_adjust_core::LocalEffect::RadialBlur(params) => {
            Some((params.center, "放射ぼかし中心"))
        }
        local_adjust_core::LocalEffect::WaveDistortion(params)
            if params.mode == local_adjust_core::WaveDistortionMode::Ripple =>
        {
            Some((params.center, "波形中心"))
        }
        local_adjust_core::LocalEffect::PinchSpherize(params) => {
            Some((params.center, "つまむ/魚眼中心"))
        }
        local_adjust_core::LocalEffect::Twirl(params) => Some((params.center, "渦巻き中心")),
        local_adjust_core::LocalEffect::PolarCoordinates(params) => {
            Some((params.center, "極座標中心"))
        }
        local_adjust_core::LocalEffect::LensCorrection(params) => {
            Some((params.center, "レンズ補正中心"))
        }
        local_adjust_core::LocalEffect::GodRays(params) => Some((params.center, "光源位置")),
        local_adjust_core::LocalEffect::LensFlare(params) => {
            Some((params.center, "フレア光源位置"))
        }
        local_adjust_core::LocalEffect::LightLeak(params) => {
            Some((params.center, "ライトリーク位置"))
        }
        local_adjust_core::LocalEffect::BacklightHaze(params) => {
            Some((params.center, "逆光ヘイズ位置"))
        }
        local_adjust_core::LocalEffect::SpeedLines(params) => {
            Some((params.center, "集中線/スピード線中心"))
        }
        local_adjust_core::LocalEffect::RadialFlash(params) => {
            Some((params.center, "集中線フラッシュ中心"))
        }
        local_adjust_core::LocalEffect::Spotlight(params) => {
            Some((params.center, "スポットライト位置"))
        }
        _ => None,
    }
}

fn draw_local_adjust_effect_center_marker(
    painter: &egui::Painter,
    center: egui::Pos2,
    label: &str,
    fill: egui::Color32,
) {
    let guide = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 145),
    );
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(10, 30, 36));
    painter.circle_filled(center, 7.0, fill);
    painter.circle_stroke(center, 7.0, stroke);
    painter.line_segment(
        [
            egui::pos2(center.x - 14.0, center.y),
            egui::pos2(center.x + 14.0, center.y),
        ],
        guide,
    );
    painter.line_segment(
        [
            egui::pos2(center.x, center.y - 14.0),
            egui::pos2(center.x, center.y + 14.0),
        ],
        guide,
    );
    painter.text(
        center + egui::vec2(10.0, -12.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgba_unmultiplied(230, 245, 255, 220),
    );
}

fn draw_local_adjust_effect_source_radius(
    painter: &egui::Painter,
    rect: egui::Rect,
    center: egui::Pos2,
    radius_px: f32,
    source_px_scale: f32,
    color: egui::Color32,
) {
    let radius = if radius_px > 0.0 {
        radius_px * source_px_scale
    } else {
        local_adjust_distance_to_farthest_rect_corner(center, rect)
    };
    if radius > 1.5 {
        painter.circle_stroke(center, radius, egui::Stroke::new(1.0, color));
    }
}

fn draw_local_adjust_tilt_shift_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    params: local_adjust_core::TiltShiftParams,
) {
    if !params.range_initialized {
        draw_local_adjust_effect_center_marker(
            painter,
            local_adjust_drawn_norm_to_screen(rect, params.center),
            "チルトシフト中心",
            egui::Color32::from_rgb(185, 235, 255),
        );
        return;
    }

    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 220, 255));
    let soft_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(100, 220, 255, 150),
    );
    let focus_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(255, 230, 120, 180),
    );
    let handle_fill = egui::Color32::from_rgb(210, 245, 255);
    let focus_fill = egui::Color32::from_rgb(255, 238, 150);
    let outer_fill = egui::Color32::from_rgb(120, 220, 255);
    let handle_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(10, 30, 36));

    match params.mode {
        local_adjust_core::TiltShiftMode::Linear => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            let angle = params.angle_degrees.to_radians();
            let dir = [angle.cos(), angle.sin()];
            let perp = [-dir[1], dir[0]];
            let focus = params.focus_width.max(0.0);
            let outer = focus + params.falloff.max(0.001);
            let draw_boundary = |amount: f32, stroke: egui::Stroke| {
                let base = local_adjust_offset_norm(params.center, dir, amount);
                let a = local_adjust_drawn_norm_to_screen_unclamped(
                    rect,
                    local_adjust_offset_norm(base, perp, -1.6),
                );
                let b = local_adjust_drawn_norm_to_screen_unclamped(
                    rect,
                    local_adjust_offset_norm(base, perp, 1.6),
                );
                painter.line_segment([a, b], stroke);
            };

            if params.far_only {
                painter.line_segment(
                    [
                        center,
                        local_adjust_drawn_norm_to_screen_unclamped(
                            rect,
                            local_adjust_offset_norm(params.center, dir, outer),
                        ),
                    ],
                    stroke,
                );
                draw_boundary(focus, focus_stroke);
                draw_boundary(outer, stroke);
            } else {
                painter.line_segment(
                    [
                        local_adjust_drawn_norm_to_screen_unclamped(
                            rect,
                            local_adjust_offset_norm(params.center, dir, -outer),
                        ),
                        local_adjust_drawn_norm_to_screen_unclamped(
                            rect,
                            local_adjust_offset_norm(params.center, dir, outer),
                        ),
                    ],
                    stroke,
                );
                draw_boundary(-focus, focus_stroke);
                draw_boundary(focus, focus_stroke);
                draw_boundary(-outer, soft_stroke);
                draw_boundary(outer, stroke);
            }

            let focus_handle = local_adjust_drawn_norm_to_screen_unclamped(
                rect,
                local_adjust_offset_norm(params.center, dir, focus),
            );
            let outer_handle = local_adjust_drawn_norm_to_screen_unclamped(
                rect,
                local_adjust_offset_norm(params.center, dir, outer),
            );
            painter.circle_filled(center, 6.0, handle_fill);
            painter.circle_stroke(center, 6.0, handle_stroke);
            painter.circle_filled(focus_handle, 5.0, focus_fill);
            painter.circle_stroke(focus_handle, 5.0, handle_stroke);
            painter.circle_filled(outer_handle, 6.0, outer_fill);
            painter.circle_stroke(outer_handle, 6.0, handle_stroke);
        }
        local_adjust_core::TiltShiftMode::Radial => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            let inner_rx = params.radius[0].max(0.001) * rect.width();
            let inner_ry = params.radius[1].max(0.001) * rect.height();
            let outer_rx =
                params.radius[0].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.width();
            let outer_ry =
                params.radius[1].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.height();
            draw_local_adjust_ellipse_stroke(painter, center, inner_rx, inner_ry, focus_stroke);
            draw_local_adjust_ellipse_stroke(painter, center, outer_rx, outer_ry, stroke);
            let inner_x_handle = egui::pos2(center.x + inner_rx, center.y);
            let inner_y_handle = egui::pos2(center.x, center.y + inner_ry);
            let outer_x_handle = egui::pos2(center.x + outer_rx, center.y);
            let outer_y_handle = egui::pos2(center.x, center.y + outer_ry);
            painter.line_segment(
                [egui::pos2(center.x - outer_rx, center.y), outer_x_handle],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(100, 220, 255, 110),
                ),
            );
            painter.line_segment(
                [egui::pos2(center.x, center.y - outer_ry), outer_y_handle],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(100, 220, 255, 110),
                ),
            );
            for (handle, fill, radius) in [
                (center, handle_fill, 6.0),
                (inner_x_handle, focus_fill, 5.0),
                (inner_y_handle, focus_fill, 5.0),
                (outer_x_handle, outer_fill, 6.0),
                (outer_y_handle, outer_fill, 6.0),
            ] {
                painter.circle_filled(handle, radius, fill);
                painter.circle_stroke(handle, radius, handle_stroke);
            }
        }
    }
}

fn draw_local_adjust_effect_position_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    image_dims: (usize, usize),
    effect: &local_adjust_core::LocalEffect,
) {
    let source_px_scale = local_adjust_screen_px_per_source_px(rect, image_dims);
    match effect {
        local_adjust_core::LocalEffect::TiltShift(params) => {
            draw_local_adjust_tilt_shift_overlay(painter, rect, *params);
        }
        local_adjust_core::LocalEffect::RadialBlur(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "放射ぼかし中心",
                egui::Color32::from_rgb(185, 235, 255),
            );
            painter.circle_stroke(
                center,
                18.0,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(120, 220, 255, 130),
                ),
            );
        }
        local_adjust_core::LocalEffect::WaveDistortion(params)
            if params.mode == local_adjust_core::WaveDistortionMode::Ripple =>
        {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "波形中心",
                egui::Color32::from_rgb(170, 235, 255),
            );
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(120, 220, 255, 115),
            );
            for radius in [24.0, 48.0, 72.0] {
                painter.circle_stroke(center, radius, stroke);
            }
        }
        local_adjust_core::LocalEffect::PinchSpherize(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "つまむ/魚眼中心",
                egui::Color32::from_rgb(185, 235, 255),
            );
            draw_local_adjust_effect_source_radius(
                painter,
                rect,
                center,
                params.radius_px,
                source_px_scale,
                egui::Color32::from_rgba_unmultiplied(120, 220, 255, 150),
            );
        }
        local_adjust_core::LocalEffect::Twirl(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "渦巻き中心",
                egui::Color32::from_rgb(190, 225, 255),
            );
            draw_local_adjust_effect_source_radius(
                painter,
                rect,
                center,
                params.radius_px,
                source_px_scale,
                egui::Color32::from_rgba_unmultiplied(130, 205, 255, 150),
            );
        }
        local_adjust_core::LocalEffect::PolarCoordinates(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "極座標中心",
                egui::Color32::from_rgb(190, 225, 255),
            );
            draw_local_adjust_effect_source_radius(
                painter,
                rect,
                center,
                params.radius_px,
                source_px_scale,
                egui::Color32::from_rgba_unmultiplied(130, 205, 255, 150),
            );
        }
        local_adjust_core::LocalEffect::LensCorrection(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "レンズ補正中心",
                egui::Color32::from_rgb(190, 225, 255),
            );
            painter.circle_stroke(
                center,
                (rect.width().min(rect.height()) * 0.18).max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(150, 215, 255, 125),
                ),
            );
        }
        local_adjust_core::LocalEffect::GodRays(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "光源位置",
                egui::Color32::from_rgb(255, 238, 145),
            );
        }
        local_adjust_core::LocalEffect::LensFlare(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "フレア光源位置",
                egui::Color32::from_rgb(255, 232, 135),
            );
            draw_local_adjust_effect_source_radius(
                painter,
                rect,
                center,
                params.radius_px,
                source_px_scale,
                egui::Color32::from_rgba_unmultiplied(255, 226, 130, 145),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 28.0, center.y),
                    egui::pos2(center.x + 28.0, center.y),
                ],
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 245, 170, 135),
                ),
            );
        }
        local_adjust_core::LocalEffect::LightLeak(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "ライトリーク位置",
                egui::Color32::from_rgb(255, 188, 120),
            );
            let diag = rect.width().hypot(rect.height()).max(1.0);
            let radius_px = diag * params.radius.clamp(0.05, 1.6);
            painter.circle_stroke(
                center,
                radius_px.max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 178, 110, 145),
                ),
            );
            painter.circle_stroke(
                center,
                (radius_px * 1.85).max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 224, 160, 85),
                ),
            );
        }
        local_adjust_core::LocalEffect::BacklightHaze(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "逆光ヘイズ位置",
                egui::Color32::from_rgb(255, 226, 165),
            );
            let diag = rect.width().hypot(rect.height()).max(1.0);
            let radius_px = diag * params.radius.clamp(0.05, 1.6);
            painter.circle_stroke(
                center,
                radius_px.max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 230, 175, 145),
                ),
            );
            painter.circle_stroke(
                center,
                (radius_px * 1.55).max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 245, 210, 90),
                ),
            );
        }
        local_adjust_core::LocalEffect::SpeedLines(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "集中線/スピード線中心",
                egui::Color32::from_rgb(215, 250, 255),
            );
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(160, 235, 255, 130),
            );
            match params.mode {
                local_adjust_core::SpeedLinesMode::Radial => {
                    let max_radius =
                        local_adjust_distance_to_farthest_rect_corner(center, rect).max(1.0);
                    painter.circle_stroke(center, max_radius * params.inner_radius, stroke);
                    painter.circle_stroke(center, max_radius * params.outer_radius, stroke);
                }
                local_adjust_core::SpeedLinesMode::Parallel => {
                    let angle = params.angle_degrees.to_radians();
                    let dir = egui::vec2(angle.cos(), angle.sin());
                    let half = rect.width().hypot(rect.height()) * 0.5;
                    painter.line_segment([center - dir * half, center + dir * half], stroke);
                }
            }
        }
        local_adjust_core::LocalEffect::RadialFlash(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "集中線フラッシュ中心",
                egui::Color32::from_rgb(255, 245, 170),
            );
            let max_radius = local_adjust_distance_to_farthest_rect_corner(center, rect).max(1.0);
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 245, 170, 135),
            );
            painter.circle_stroke(center, max_radius * params.inner_radius, stroke);
            painter.circle_stroke(center, max_radius * params.outer_radius, stroke);
        }
        local_adjust_core::LocalEffect::Spotlight(params) => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            let radius = params.radius.clamp(0.0, 1.0);
            let feather = params.feather.clamp(0.001, 1.0);
            let max_dim = rect.width().max(rect.height());
            let radius_px = max_dim * radius * 0.5;
            let outer_px = max_dim * (radius + feather).min(1.5) * 0.5;
            painter.circle_stroke(
                center,
                outer_px.max(2.0),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 230, 130, 120),
                ),
            );
            painter.circle_stroke(
                center,
                radius_px.max(2.0),
                egui::Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_unmultiplied(255, 238, 160, 180),
                ),
            );
            draw_local_adjust_effect_center_marker(
                painter,
                center,
                "スポットライト位置",
                egui::Color32::from_rgb(255, 224, 110),
            );
        }
        _ => {
            if let Some((center_norm, label)) = local_adjust_effect_center(effect) {
                draw_local_adjust_effect_center_marker(
                    painter,
                    local_adjust_drawn_norm_to_screen(rect, center_norm),
                    label,
                    egui::Color32::from_rgb(185, 235, 255),
                );
            }
        }
    }
}

fn local_adjust_tilt_shift_handle_positions(
    rect: egui::Rect,
    params: local_adjust_core::TiltShiftParams,
) -> Vec<(crate::app::LocalAdjustCanvasDragKind, egui::Pos2)> {
    if !params.range_initialized {
        return Vec::new();
    }
    match params.mode {
        local_adjust_core::TiltShiftMode::Linear => {
            let angle = params.angle_degrees.to_radians();
            let dir = [angle.cos(), angle.sin()];
            let focus = params.focus_width.max(0.0);
            let outer = focus + params.falloff.max(0.001);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus,
                    local_adjust_drawn_norm_to_screen_unclamped(
                        rect,
                        local_adjust_offset_norm(params.center, dir, focus),
                    ),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter,
                    local_adjust_drawn_norm_to_screen_unclamped(
                        rect,
                        local_adjust_offset_norm(params.center, dir, outer),
                    ),
                ),
            ]
        }
        local_adjust_core::TiltShiftMode::Radial => {
            let center = local_adjust_drawn_norm_to_screen(rect, params.center);
            let inner_rx = params.radius[0].max(0.001) * rect.width();
            let inner_ry = params.radius[1].max(0.001) * rect.height();
            let outer_rx =
                params.radius[0].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.width();
            let outer_ry =
                params.radius[1].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.height();
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerX,
                    egui::pos2(center.x + inner_rx, center.y),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerY,
                    egui::pos2(center.x, center.y + inner_ry),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX,
                    egui::pos2(center.x + outer_rx, center.y),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterY,
                    egui::pos2(center.x, center.y + outer_ry),
                ),
            ]
        }
    }
}

fn local_adjust_tilt_shift_handle_hit(
    effect: &local_adjust_core::LocalEffect,
    pos: egui::Pos2,
    drawn_rect: egui::Rect,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    let local_adjust_core::LocalEffect::TiltShift(params) = effect else {
        return None;
    };
    local_adjust_tilt_shift_handle_positions(drawn_rect, *params)
        .into_iter()
        .find_map(|(kind, handle)| (handle.distance(pos) <= 15.0).then_some(kind))
}

fn local_adjust_tilt_shift_range_create_pending(effect: &local_adjust_core::LocalEffect) -> bool {
    matches!(
        effect,
        local_adjust_core::LocalEffect::TiltShift(params)
            if params.mode_selected && !params.range_initialized
    )
}

fn apply_local_adjust_tilt_shift_range_drag(
    params: &mut local_adjust_core::TiltShiftParams,
    start: [f32; 2],
    norm: [f32; 2],
) -> bool {
    params.mode_selected = false;
    params.range_initialized = true;
    params.center = start;
    if params.strength <= f32::EPSILON {
        params.strength = 1.0;
    }
    if params.max_radius_px <= f32::EPSILON {
        params.max_radius_px = 20.0;
    }

    let dx = norm[0] - start[0];
    let dy = norm[1] - start[1];
    match params.mode {
        local_adjust_core::TiltShiftMode::Linear => {
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > 0.001 {
                params.angle_degrees = dy.atan2(dx).to_degrees();
                params.focus_width = (distance * 0.35).clamp(0.0, 0.8);
                params.falloff = (distance * 0.65).clamp(0.001, 1.2);
            } else {
                params.focus_width = 0.0;
                params.falloff = 0.001;
            }
        }
        local_adjust_core::TiltShiftMode::Radial => {
            let distance = (dx * dx + dy * dy).sqrt();
            params.falloff = 0.40;
            if distance > 0.001 {
                let rx = dx.abs().max(distance * 0.35).clamp(0.001, 1.2);
                let ry = dy.abs().max(distance * 0.35).clamp(0.001, 1.2);
                params.radius = [rx, ry];
            } else {
                params.radius = [0.001, 0.001];
            }
        }
    }
    true
}

fn apply_local_adjust_tilt_shift_handle_drag(
    params: &mut local_adjust_core::TiltShiftParams,
    kind: crate::app::LocalAdjustCanvasDragKind,
    norm: [f32; 2],
) -> bool {
    match (params.mode, kind) {
        (
            local_adjust_core::TiltShiftMode::Linear,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus,
        ) => {
            let dx = norm[0] - params.center[0];
            let dy = norm[1] - params.center[1];
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= 0.001 {
                return false;
            }
            params.angle_degrees = dy.atan2(dx).to_degrees();
            params.focus_width = distance.min(0.8);
            params.falloff = params.falloff.max(0.001);
        }
        (
            local_adjust_core::TiltShiftMode::Linear,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter,
        ) => {
            let dx = norm[0] - params.center[0];
            let dy = norm[1] - params.center[1];
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= 0.001 {
                return false;
            }
            params.angle_degrees = dy.atan2(dx).to_degrees();
            params.falloff = (distance - params.focus_width.max(0.0)).max(0.001).min(1.2);
        }
        (
            local_adjust_core::TiltShiftMode::Radial,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerX,
        ) => {
            params.radius[0] = (norm[0] - params.center[0]).abs().clamp(0.001, 1.2);
        }
        (
            local_adjust_core::TiltShiftMode::Radial,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerY,
        ) => {
            params.radius[1] = (norm[1] - params.center[1]).abs().clamp(0.001, 1.2);
        }
        (
            local_adjust_core::TiltShiftMode::Radial,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX,
        ) => {
            let outer = (norm[0] - params.center[0]).abs();
            params.falloff = (outer / params.radius[0].max(0.001) - 1.0)
                .max(0.001)
                .min(1.2);
        }
        (
            local_adjust_core::TiltShiftMode::Radial,
            crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterY,
        ) => {
            let outer = (norm[1] - params.center[1]).abs();
            params.falloff = (outer / params.radius[1].max(0.001) - 1.0)
                .max(0.001)
                .min(1.2);
        }
        _ => return false,
    }
    params.range_initialized = true;
    true
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

fn default_local_mask(kind: MaskKind, image_dims: (usize, usize)) -> local_adjust_core::LocalMask {
    let (w, h) = (image_dims.0.max(1), image_dims.1.max(1));
    match kind {
        MaskKind::Full => local_adjust_core::LocalMask::Full,
        MaskKind::Raster => local_adjust_core::LocalMask::RasterVector(
            local_adjust_core::RasterVectorMask::empty(w, h),
        ),
        MaskKind::LinearGradient => local_adjust_core::LocalMask::LinearGradient(
            local_adjust_core::LinearGradientMask::default(),
        ),
        MaskKind::RadialGradient => local_adjust_core::LocalMask::RadialGradient(
            local_adjust_core::RadialGradientMask::default(),
        ),
        MaskKind::LumaRange => {
            local_adjust_core::LocalMask::LumaRange(local_adjust_core::RangeMask::default())
        }
        MaskKind::ColorRange => {
            local_adjust_core::LocalMask::ColorRange(local_adjust_core::ColorRangeMask::default())
        }
        MaskKind::Subject => {
            local_adjust_core::LocalMask::Subject(local_adjust_core::SubjectMask::empty(w, h))
        }
        MaskKind::Segmentation => {
            local_adjust_core::LocalMask::Segmentation(local_adjust_core::RegionMask::empty(w, h))
        }
    }
}

fn layer_with_local_mask(
    name: impl Into<String>,
    mask_kind: MaskKind,
    image_dims: (usize, usize),
) -> local_adjust_core::LocalAdjustmentLayer {
    local_adjust_core::LocalAdjustmentLayer::new(
        name,
        default_local_mask(mask_kind, image_dims),
        local_adjust_core::LocalEffect::None,
    )
}

fn replace_local_adjust_layer_base_mask(
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    mask_kind: MaskKind,
    image_dims: (usize, usize),
    keep_manual_override: bool,
) {
    layer.mask = default_local_mask(mask_kind, image_dims);
    if !keep_manual_override {
        layer.manual_override = local_adjust_core::ManualMaskOverride::default();
    }
}

fn draw_local_adjust_panel_section<R>(
    ui: &mut egui::Ui,
    section: LocalAdjustPanelSection,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let response = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: LOCAL_ADJUST_PANEL_SECTION_MARGIN_LEFT,
            right: LOCAL_ADJUST_PANEL_SECTION_MARGIN_RIGHT,
            top: 6,
            bottom: 6,
        })
        .show(ui, add_contents);
    let rect = response.response.rect;
    let line_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 4.0),
        egui::pos2(rect.left() + 3.0, rect.bottom() - 4.0),
    );
    ui.painter()
        .rect_filled(line_rect, 1.5, section.accent_color());
    ui.add_space(2.0);
    response.inner
}

fn local_adjust_panel_toggle_button(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    size: Option<egui::Vec2>,
    paint_mode_button: bool,
) -> egui::Response {
    let fill = if active {
        if paint_mode_button {
            egui::Color32::from_rgb(130, 58, 58)
        } else {
            egui::Color32::from_rgb(58, 96, 150)
        }
    } else {
        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 170)
    };
    let button = egui::Button::new(label).fill(fill);
    if let Some(size) = size {
        ui.add_sized(size, button)
    } else {
        ui.add(button)
    }
}

fn draw_local_mask_application_button(
    ui: &mut egui::Ui,
    label: &'static str,
    active: bool,
) -> egui::Response {
    let fill = if active {
        egui::Color32::from_rgba_unmultiplied(92, 132, 190, 230)
    } else {
        egui::Color32::from_rgba_unmultiplied(36, 38, 42, 170)
    };
    let text_color = if active {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(165)
    };
    ui.add_sized(
        egui::vec2(20.0, 18.0),
        egui::Button::new(egui::RichText::new(label).size(10.0).color(text_color))
            .fill(fill)
            .corner_radius(3.0),
    )
}

fn draw_local_layer_mask_thumbnail(
    ui: &mut egui::Ui,
    layer: &local_adjust_core::LocalAdjustmentLayer,
    image_dims: (usize, usize),
    source: Option<&egui::ColorImage>,
    selected: bool,
) -> egui::Response {
    const SIZE: f32 = 48.0;
    const GRID: usize = 32;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        4.0,
        egui::Color32::from_rgba_unmultiplied(8, 8, 10, 180),
    );

    let width = image_dims.0.max(1);
    let height = image_dims.1.max(1);
    let source = source.filter(|source| source.size == [width, height]);
    let mut alphas = [0.0_f32; GRID * GRID];
    let mut min_x = GRID;
    let mut min_y = GRID;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for gy in 0..GRID {
        for gx in 0..GRID {
            let x = (((gx as f32 + 0.5) * width as f32 / GRID as f32) as usize)
                .min(width.saturating_sub(1));
            let y = (((gy as f32 + 0.5) * height as f32 / GRID as f32) as usize)
                .min(height.saturating_sub(1));
            let alpha =
                local_adjust_mask_preview_alpha(layer, source, width, height, x, y).clamp(0.0, 1.0);
            alphas[gy * GRID + gx] = alpha;
            if alpha > 0.02 {
                min_x = min_x.min(gx);
                min_y = min_y.min(gy);
                max_x = max_x.max(gx);
                max_y = max_y.max(gy);
                found = true;
            }
        }
    }

    if found {
        let inner = rect.shrink(5.0);
        let crop_w = (max_x - min_x + 1).max(1);
        let crop_h = (max_y - min_y + 1).max(1);
        for gy in 0..GRID {
            for gx in 0..GRID {
                let sx = min_x + ((gx as f32 + 0.5) * crop_w as f32 / GRID as f32) as usize;
                let sy = min_y + ((gy as f32 + 0.5) * crop_h as f32 / GRID as f32) as usize;
                let alpha = alphas[sy.min(GRID - 1) * GRID + sx.min(GRID - 1)];
                if alpha <= 0.02 {
                    continue;
                }
                let x0 = inner.left() + gx as f32 * inner.width() / GRID as f32;
                let y0 = inner.top() + gy as f32 * inner.height() / GRID as f32;
                let x1 = inner.left() + (gx + 1) as f32 * inner.width() / GRID as f32;
                let y1 = inner.top() + (gy + 1) as f32 * inner.height() / GRID as f32;
                let alpha_u8 = (60.0 + alpha * 185.0).round() as u8;
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 85, 125, alpha_u8),
                );
            }
        }
    } else {
        draw_local_empty_thumbnail_mark(painter, rect);
    }

    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            1.0,
            if selected {
                egui::Color32::from_rgba_unmultiplied(170, 215, 255, 210)
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 45)
            },
        ),
        egui::StrokeKind::Inside,
    );
    response
}

fn draw_local_empty_thumbnail_mark(painter: &egui::Painter, rect: egui::Rect) {
    painter.line_segment(
        [
            egui::pos2(rect.left() + 11.0, rect.center().y),
            egui::pos2(rect.right() - 11.0, rect.center().y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
    );
}

fn local_adjust_mask_preview_active(
    local_adjust_mode: bool,
    show_mask: bool,
    alt_down: bool,
) -> bool {
    local_adjust_mode && (show_mask != alt_down)
}

fn local_adjust_region_label_active(
    mask: &local_adjust_core::RegionMask,
    inverted: bool,
    label: u32,
) -> bool {
    if label == 0 {
        return false;
    }
    mask.selected.get(label as usize).copied().unwrap_or(false) ^ inverted
}

fn local_adjust_region_label_boundary(
    mask: &local_adjust_core::RegionMask,
    label: u32,
    x: usize,
    y: usize,
) -> bool {
    if x == 0 || y == 0 || x + 1 == mask.width || y + 1 == mask.height {
        return true;
    }
    local_adjust_region_neighbors(x, y, mask.width, mask.height)
        .any(|(nx, ny)| mask.labels[ny * mask.width + nx] != label)
}

fn local_adjust_region_active_boundary(
    mask: &local_adjust_core::RegionMask,
    inverted: bool,
    label: u32,
    x: usize,
    y: usize,
) -> bool {
    if x == 0 || y == 0 || x + 1 == mask.width || y + 1 == mask.height {
        return true;
    }
    local_adjust_region_neighbors(x, y, mask.width, mask.height).any(|(nx, ny)| {
        let n_label = mask.labels[ny * mask.width + nx];
        n_label != label || !local_adjust_region_label_active(mask, inverted, n_label)
    })
}

fn local_adjust_region_boundary_color(label: u32, time_sec: f32) -> egui::Color32 {
    let hue = (time_sec * 130.0 + (label.wrapping_mul(47) % 360) as f32).rem_euclid(360.0);
    let [r, g, b] = local_adjust_hsv_to_rgb(hue, 0.95, 1.0);
    egui::Color32::from_rgba_unmultiplied(r, g, b, 190)
}

fn local_adjust_hsv_to_rgb(hue: f32, sat: f32, val: f32) -> [u8; 3] {
    let h = (hue / 60.0).rem_euclid(6.0);
    let c = val * sat;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = val - c;
    let (r, g, b) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn draw_local_adjust_mask_preview_overlay(
    painter: &egui::Painter,
    drawn_rect: egui::Rect,
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    image_dims: (usize, usize),
    time_sec: f32,
    colors: crate::app::LocalAdjustMaskPreviewColors,
    edit_target: Option<LocalAdjustMaskEditTarget>,
    texture_slot: &mut Option<egui::TextureHandle>,
) {
    let width = image_dims.0.max(1);
    let height = image_dims.1.max(1);
    let rect_w = drawn_rect.width().max(1.0);
    let rect_h = drawn_rect.height().max(1.0);
    let scale = (LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS / rect_w.max(rect_h)).min(1.0);
    let tex_w = (rect_w * scale)
        .round()
        .clamp(1.0, LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS) as usize;
    let tex_h = (rect_h * scale)
        .round()
        .clamp(1.0, LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS) as usize;
    let source = source.filter(|source| source.size == [width, height]);
    let mut pixels = Vec::with_capacity(tex_w.saturating_mul(tex_h));

    for gy in 0..tex_h {
        for gx in 0..tex_w {
            let x = (((gx as f32 + 0.5) * width as f32 / tex_w as f32) as usize)
                .min(width.saturating_sub(1));
            let y = (((gy as f32 + 0.5) * height as f32 / tex_h as f32) as usize)
                .min(height.saturating_sub(1));
            pixels.push(local_adjust_mask_preview_color(
                layer,
                source,
                width,
                height,
                x,
                y,
                time_sec,
                colors,
                edit_target,
            ));
        }
    }

    let image = egui::ColorImage::new([tex_w, tex_h], pixels);
    if texture_slot
        .as_ref()
        .is_some_and(|texture| texture.size() != [tex_w, tex_h])
    {
        *texture_slot = None;
    }
    if let Some(texture) = texture_slot.as_mut() {
        texture.set(image, egui::TextureOptions::NEAREST);
    } else {
        *texture_slot = Some(painter.ctx().load_texture(
            "local_adjust_mask_preview",
            image,
            egui::TextureOptions::NEAREST,
        ));
    }
    let Some(texture) = texture_slot.as_ref() else {
        return;
    };
    painter.image(
        texture.id(),
        drawn_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn local_adjust_mask_preview_color(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    time_sec: f32,
    colors: crate::app::LocalAdjustMaskPreviewColors,
    edit_target: Option<LocalAdjustMaskEditTarget>,
) -> egui::Color32 {
    let idx = y.saturating_mul(width).saturating_add(x);
    if let Some(mask) = match edit_target {
        Some(LocalAdjustMaskEditTarget::OverrideAdd) => layer.manual_override.add.as_ref(),
        Some(LocalAdjustMaskEditTarget::OverrideSubtract) => {
            layer.manual_override.subtract.as_ref()
        }
        _ => None,
    } && local_adjust_raster_vector_preview_alpha(mask, width, height, idx, x, y).unwrap_or(0.0)
        >= 0.5
    {
        return colors.edit(LOCAL_ADJUST_MASK_PREVIEW_EDIT_ALPHA);
    }

    match &layer.mask {
        local_adjust_core::LocalMask::Segmentation(mask)
            if mask.width == width && mask.height == height =>
        {
            let label = mask.labels.get(idx).copied().unwrap_or(0);
            if label == 0 {
                egui::Color32::TRANSPARENT
            } else if local_adjust_region_label_active(mask, layer.mask_inverted, label) {
                if local_adjust_region_active_boundary(mask, layer.mask_inverted, label, x, y) {
                    colors.boundary(235)
                } else {
                    colors.base(188)
                }
            } else if local_adjust_region_label_boundary(mask, label, x, y) {
                local_adjust_region_boundary_color(label, time_sec)
            } else {
                egui::Color32::TRANSPARENT
            }
        }
        _ => {
            let alpha = local_adjust_mask_preview_alpha(layer, source, width, height, x, y);
            if alpha <= 0.02 {
                egui::Color32::TRANSPARENT
            } else {
                let a = (alpha * LOCAL_ADJUST_MASK_PREVIEW_BASE_ALPHA)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                colors.base(a)
            }
        }
    }
}

fn local_adjust_mask_preview_alpha(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    let idx = y.saturating_mul(width).saturating_add(x);
    let mut alpha = match &layer.mask {
        local_adjust_core::LocalMask::Full => {
            if layer
                .manual_override
                .subtract
                .as_ref()
                .is_some_and(local_adjust_raster_vector_mask_has_content)
            {
                1.0
            } else {
                0.0
            }
        }
        local_adjust_core::LocalMask::Raster(mask) => {
            if mask.width == width && mask.height == height {
                mask.alpha.get(idx).copied().unwrap_or(0.0)
            } else {
                0.0
            }
        }
        local_adjust_core::LocalMask::RasterVector(mask) => {
            local_adjust_raster_vector_preview_alpha(mask, width, height, idx, x, y).unwrap_or(0.0)
        }
        local_adjust_core::LocalMask::Subject(mask) => {
            if mask.width == width && mask.height == height {
                mask.alpha.get(idx).copied().unwrap_or(0.0)
            } else {
                0.0
            }
        }
        local_adjust_core::LocalMask::Segmentation(mask) => {
            if mask.width == width && mask.height == height {
                let label = mask.labels.get(idx).copied().unwrap_or(0);
                if mask.selected.get(label as usize).copied().unwrap_or(false) {
                    1.0
                } else {
                    0.0
                }
            } else {
                0.0
            }
        }
        local_adjust_core::LocalMask::LinearGradient(mask) => {
            local_adjust_linear_gradient_preview_alpha(*mask, width, height, x, y)
        }
        local_adjust_core::LocalMask::RadialGradient(mask) => {
            local_adjust_radial_gradient_preview_alpha(*mask, width, height, x, y)
        }
        local_adjust_core::LocalMask::LumaRange(mask) => {
            local_adjust_luma_range_preview_alpha(source, *mask, idx)
        }
        local_adjust_core::LocalMask::ColorRange(mask) => {
            local_adjust_color_range_preview_alpha(source, *mask, idx)
        }
    }
    .clamp(0.0, 1.0);

    if let Some(add) = &layer.manual_override.add
        && local_adjust_raster_vector_preview_alpha(add, width, height, idx, x, y).unwrap_or(0.0)
            >= 0.5
    {
        alpha = 1.0;
    }
    if let Some(subtract) = &layer.manual_override.subtract
        && local_adjust_raster_vector_preview_alpha(subtract, width, height, idx, x, y)
            .unwrap_or(0.0)
            >= 0.5
    {
        alpha = 0.0;
    }
    if layer.mask_inverted {
        alpha = 1.0 - alpha;
    }
    (alpha * layer.opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

fn local_adjust_raster_vector_mask_has_content(mask: &local_adjust_core::RasterVectorMask) -> bool {
    mask.alpha.iter().any(|&alpha| alpha >= 0.5)
        || mask.shapes.iter().any(|shape| shape.op().is_add())
}

fn local_adjust_raster_vector_preview_alpha(
    mask: &local_adjust_core::RasterVectorMask,
    width: usize,
    height: usize,
    idx: usize,
    x: usize,
    y: usize,
) -> Option<f32> {
    if mask.width != width
        || mask.height != height
        || mask.alpha.len() != width.saturating_mul(height)
    {
        return None;
    }
    let mut alpha = mask.alpha.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    if !mask.shapes.is_empty() {
        let point = [x as f32 + 0.5, y as f32 + 0.5];
        for &shape in &mask.shapes {
            if local_adjust_shape_contains(shape, point) {
                alpha = if shape.op().is_add() { 1.0 } else { 0.0 };
            }
        }
    }
    Some(alpha)
}

fn local_adjust_linear_gradient_preview_alpha(
    mask: local_adjust_core::LinearGradientMask,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    if !mask.initialized {
        return 0.0;
    }
    let sx = mask.start[0];
    let sy = mask.start[1];
    let dx = mask.end[0] - sx;
    let dy = mask.end[1] - sy;
    let denom = dx * dx + dy * dy;
    if denom <= f32::EPSILON {
        return 1.0;
    }
    let nx = (x as f32 + 0.5) / width.max(1) as f32;
    let ny = (y as f32 + 0.5) / height.max(1) as f32;
    (((nx - sx) * dx + (ny - sy) * dy) / denom).clamp(0.0, 1.0)
}

fn local_adjust_radial_gradient_preview_alpha(
    mask: local_adjust_core::RadialGradientMask,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    if !mask.initialized {
        return 0.0;
    }
    let nx = (x as f32 + 0.5) / width.max(1) as f32;
    let ny = (y as f32 + 0.5) / height.max(1) as f32;
    let dx = nx - mask.center[0];
    let dy = ny - mask.center[1];
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= f32::EPSILON {
        return 1.0;
    }
    let ux = dx / dist;
    let uy = dy / dist;
    let inner_x = mask.inner_radius.max(0.0);
    let inner_y = mask.inner_radius_y.max(0.0);
    let outer_x = mask.outer_radius.max(inner_x + 0.0001);
    let outer_y = mask.outer_radius_y.max(inner_y + 0.0001);
    let inner = local_adjust_ellipse_radius_for_direction(inner_x, inner_y, ux, uy);
    let outer =
        local_adjust_ellipse_radius_for_direction(outer_x, outer_y, ux, uy).max(inner + 0.0001);
    (1.0 - ((dist - inner) / (outer - inner))).clamp(0.0, 1.0)
}

fn local_adjust_ellipse_radius_for_direction(rx: f32, ry: f32, ux: f32, uy: f32) -> f32 {
    if rx <= f32::EPSILON || ry <= f32::EPSILON {
        return 0.0;
    }
    let denom = (ux / rx).powi(2) + (uy / ry).powi(2);
    if denom <= f32::EPSILON {
        0.0
    } else {
        1.0 / denom.sqrt()
    }
}

fn local_adjust_gradient_handle_hit(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    pos: egui::Pos2,
    image_rect: egui::Rect,
    image_dims: (usize, usize),
    zoom_pan: Option<(f32, egui::Vec2)>,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    const HIT_RADIUS: f32 = 14.0;
    match &layer.mask {
        local_adjust_core::LocalMask::LinearGradient(mask) if mask.initialized => {
            let start = local_adjust_norm_to_screen(mask.start, image_rect, image_dims, zoom_pan)?;
            let end = local_adjust_norm_to_screen(mask.end, image_rect, image_dims, zoom_pan)?;
            if end.distance(pos) <= HIT_RADIUS {
                Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd)
            } else if start.distance(pos) <= HIT_RADIUS {
                Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientStart)
            } else {
                None
            }
        }
        local_adjust_core::LocalMask::RadialGradient(mask) if mask.initialized => {
            let (_, drawn_rect) = local_adjust_image_layout(image_rect, image_dims, zoom_pan)?;
            let center =
                local_adjust_norm_to_screen(mask.center, image_rect, image_dims, zoom_pan)?;
            let inner_rx = mask.inner_radius.max(0.0) * drawn_rect.width();
            let inner_ry = mask.inner_radius_y.max(0.0) * drawn_rect.height();
            let outer_rx = mask.outer_radius.max(mask.inner_radius).max(0.0) * drawn_rect.width();
            let outer_ry =
                mask.outer_radius_y.max(mask.inner_radius_y).max(0.0) * drawn_rect.height();
            let handles = [
                (
                    egui::pos2(center.x + outer_rx, center.y),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX,
                ),
                (
                    egui::pos2(center.x, center.y + outer_ry),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterY,
                ),
                (
                    egui::pos2(center.x + inner_rx, center.y),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerX,
                ),
                (
                    egui::pos2(center.x, center.y + inner_ry),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerY,
                ),
                (
                    center,
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientCenter,
                ),
            ];
            handles.into_iter().find_map(|(handle_pos, kind)| {
                (handle_pos.distance(pos) <= HIT_RADIUS).then_some(kind)
            })
        }
        _ => None,
    }
}

fn local_adjust_gradient_create_pending(layer: &local_adjust_core::LocalAdjustmentLayer) -> bool {
    match &layer.mask {
        local_adjust_core::LocalMask::LinearGradient(mask) => !mask.initialized,
        local_adjust_core::LocalMask::RadialGradient(mask) => !mask.initialized,
        _ => false,
    }
}

fn local_adjust_luma_range_preview_alpha(
    source: Option<&egui::ColorImage>,
    mask: local_adjust_core::RangeMask,
    idx: usize,
) -> f32 {
    let Some(pixel) = source.and_then(|source| source.pixels.get(idx)) else {
        return 0.0;
    };
    let [r, g, b, _] = pixel.to_srgba_unmultiplied();
    let luma = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
    let (min, max) = local_adjust_ordered_pair(mask.min, mask.max);
    local_adjust_range_alpha(luma, min, max, mask.feather)
}

fn local_adjust_color_range_preview_alpha(
    source: Option<&egui::ColorImage>,
    mask: local_adjust_core::ColorRangeMask,
    idx: usize,
) -> f32 {
    if !mask.initialized {
        return 0.0;
    }
    let Some(pixel) = source.and_then(|source| source.pixels.get(idx)) else {
        return 0.0;
    };
    let [r, g, b, _] = pixel.to_srgba_unmultiplied();
    let tr = mask.target_rgb[0] as f32 / 255.0;
    let tg = mask.target_rgb[1] as f32 / 255.0;
    let tb = mask.target_rgb[2] as f32 / 255.0;
    let dr = r as f32 / 255.0 - tr;
    let dg = g as f32 / 255.0 - tg;
    let db = b as f32 / 255.0 - tb;
    let dist = ((dr * dr + dg * dg + db * db) / 3.0).sqrt();
    let tolerance = mask.tolerance.max(0.0);
    if dist <= tolerance {
        1.0
    } else {
        (1.0 - (dist - tolerance) / mask.feather.max(0.0001)).clamp(0.0, 1.0)
    }
}

fn local_adjust_range_alpha(value: f32, min: f32, max: f32, feather: f32) -> f32 {
    let feather = feather.max(0.0001);
    if value >= min && value <= max {
        1.0
    } else if value < min {
        (1.0 - (min - value) / feather).clamp(0.0, 1.0)
    } else {
        (1.0 - (value - max) / feather).clamp(0.0, 1.0)
    }
}

fn local_adjust_ordered_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

fn local_adjust_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &'static str,
) -> bool {
    ui.add(egui::Slider::new(value, range).text(label))
        .changed()
}

fn draw_range_mask_sliders(ui: &mut egui::Ui, mask: &mut local_adjust_core::RangeMask) -> bool {
    let mut changed = false;
    changed |= local_adjust_slider(ui, &mut mask.min, 0.0..=1.0, "下限");
    changed |= local_adjust_slider(ui, &mut mask.max, 0.0..=1.0, "上限");
    changed |= local_adjust_slider(ui, &mut mask.feather, 0.0..=1.0, "範囲ぼかし");
    if mask.max < mask.min {
        std::mem::swap(&mut mask.min, &mut mask.max);
        changed = true;
    }
    changed
}

fn draw_local_mask_editor(
    ui: &mut egui::Ui,
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    _image_dims: (usize, usize),
    segmentation_pending: bool,
    subject_model_available: bool,
    subject_mask_available: bool,
    mask_paint_add: &mut bool,
    region_color_tolerance: &mut f32,
    region_min_area: &mut usize,
    layer_idx: usize,
    effect_requests: &mut LocalEffectPanelRequests,
) -> bool {
    let mut changed = false;

    match &mut layer.mask {
        local_adjust_core::LocalMask::Full => {
            ui.label(
                egui::RichText::new("画像全体に効果を適用します。")
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            );
        }
        local_adjust_core::LocalMask::Raster(mask) => {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("クリア").clicked() {
                    mask.alpha.fill(0.0);
                    changed = true;
                }
                if ui.small_button("塗りつぶし").clicked() {
                    mask.alpha.fill(1.0);
                    changed = true;
                }
            });
            ui.label(format!("ビットマップ: {} x {}", mask.width, mask.height));
        }
        local_adjust_core::LocalMask::RasterVector(mask) => {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("ビットマップ消去").clicked() {
                    mask.alpha.fill(0.0);
                    changed = true;
                }
                if ui.small_button("オブジェクト消去").clicked() {
                    mask.shapes.clear();
                    changed = true;
                }
            });
            ui.label(format!(
                "手動マスク: {} x {} / オブジェクト {}",
                mask.width,
                mask.height,
                mask.shapes.len()
            ));
        }
        local_adjust_core::LocalMask::LinearGradient(mask) => {
            if !mask.initialized {
                ui.label("画像上でドラッグして範囲を生成します。");
            } else {
                if ui.small_button("グラデーションをクリア").clicked() {
                    *mask = local_adjust_core::LinearGradientMask::default();
                    changed = true;
                }
                changed |= local_adjust_slider(ui, &mut mask.start[0], 0.0..=1.0, "開始 X");
                changed |= local_adjust_slider(ui, &mut mask.start[1], 0.0..=1.0, "開始 Y");
                changed |= local_adjust_slider(ui, &mut mask.end[0], 0.0..=1.0, "終了 X");
                changed |= local_adjust_slider(ui, &mut mask.end[1], 0.0..=1.0, "終了 Y");
            }
        }
        local_adjust_core::LocalMask::RadialGradient(mask) => {
            if !mask.initialized {
                ui.label("画像上でドラッグして範囲を生成します。");
            } else {
                if ui.small_button("グラデーションをクリア").clicked() {
                    *mask = local_adjust_core::RadialGradientMask::default();
                    changed = true;
                }
                changed |= local_adjust_slider(ui, &mut mask.center[0], 0.0..=1.0, "中心 X");
                changed |= local_adjust_slider(ui, &mut mask.center[1], 0.0..=1.0, "中心 Y");
                changed |= local_adjust_slider(ui, &mut mask.inner_radius, 0.0..=1.5, "内側 横");
                changed |= local_adjust_slider(ui, &mut mask.inner_radius_y, 0.0..=1.5, "内側 縦");
                changed |= local_adjust_slider(ui, &mut mask.outer_radius, 0.0..=1.5, "外側 横");
                changed |= local_adjust_slider(ui, &mut mask.outer_radius_y, 0.0..=1.5, "外側 縦");
                mask.outer_radius = mask.outer_radius.max(mask.inner_radius + 0.001);
                mask.outer_radius_y = mask.outer_radius_y.max(mask.inner_radius_y + 0.001);
            }
        }
        local_adjust_core::LocalMask::LumaRange(mask) => {
            ui.label(
                egui::RichText::new("輝度範囲")
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            );
            changed |= draw_range_mask_sliders(ui, mask);
        }
        local_adjust_core::LocalMask::ColorRange(mask) => {
            if !mask.initialized && ui.small_button("白を対象色にする").clicked() {
                mask.initialized = true;
                mask.target_rgb = [255, 255, 255];
                changed = true;
            }
            let mut r = mask.target_rgb[0] as i32;
            let mut g = mask.target_rgb[1] as i32;
            let mut b = mask.target_rgb[2] as i32;
            let rgb_changed = ui
                .add(egui::Slider::new(&mut r, 0..=255).text("R"))
                .changed()
                | ui.add(egui::Slider::new(&mut g, 0..=255).text("G"))
                    .changed()
                | ui.add(egui::Slider::new(&mut b, 0..=255).text("B"))
                    .changed();
            if rgb_changed {
                mask.target_rgb = [r as u8, g as u8, b as u8];
                mask.initialized = true;
                changed = true;
            }
            changed |= local_adjust_slider(ui, &mut mask.tolerance, 0.0..=1.0, "許容幅");
            changed |= local_adjust_slider(ui, &mut mask.feather, 0.0..=1.0, "範囲ぼかし");
        }
        local_adjust_core::LocalMask::Subject(mask) => {
            ui.label(format!("被写体マスク: {} x {}", mask.width, mask.height));
            let generated_mask_available = local_adjust_subject_mask_has_content(mask);
            let generate_label = if segmentation_pending {
                "生成中..."
            } else if generated_mask_available {
                "元画像から再生成"
            } else {
                "被写体マスク生成"
            };
            let generate_response = ui.add_enabled(
                !segmentation_pending && subject_model_available,
                egui::Button::new(generate_label),
            );
            let generate_tip = if subject_model_available {
                "元画像から U²-Netp で被写体マスクを生成します。"
            } else {
                "U²-Netp モデルが見つからないため生成できません。保存済みマスクの適用は可能です。"
            };
            if generate_response.on_hover_text(generate_tip).clicked() {
                effect_requests.generate_subject_mask = Some(layer_idx);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("被写体を選択").clicked() {
                    layer.mask_inverted = false;
                    changed = true;
                }
                if ui.small_button("背景を選択").clicked() {
                    layer.mask_inverted = true;
                    changed = true;
                }
            });
            changed |= ui
                .checkbox(&mut mask.refinement.enabled, "輪郭補正")
                .changed();
            changed |=
                local_adjust_slider(ui, &mut mask.refinement.threshold, 0.0..=1.0, "しきい値");
            let mut expand = mask.refinement.expand_px;
            let mut feather = mask.refinement.feather_px;
            changed |= ui
                .add(egui::Slider::new(&mut expand, -32..=32).text("拡張"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut feather, 0..=32).text("ぼかし"))
                .changed();
            mask.refinement.expand_px = expand;
            mask.refinement.feather_px = feather;
            let foreground = mask.alpha.iter().filter(|&&alpha| alpha >= 0.5).count();
            let soft = mask
                .alpha
                .iter()
                .filter(|&&alpha| alpha > 0.02 && alpha < 0.98)
                .count();
            let total = mask.alpha.len().max(1) as f32;
            ui.label(
                egui::RichText::new(format!(
                    "前景 {:.1}% / 半透明 {:.1}%",
                    foreground as f32 / total * 100.0,
                    soft as f32 / total * 100.0
                ))
                .size(10.0)
                .color(egui::Color32::from_gray(170)),
            );
        }
        local_adjust_core::LocalMask::Segmentation(mask) => {
            ui.label(
                egui::RichText::new("領域分割")
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            );
            let response =
                ui.add(egui::Slider::new(region_color_tolerance, 4.0..=120.0).text("色差許容"));
            response.on_hover_text("大きいほど近い色が同じ領域にまとまります。");
            let mut min_area = (*region_min_area).clamp(1, 2048) as i32;
            if ui
                .add(egui::Slider::new(&mut min_area, 1..=2048).text("最小領域"))
                .on_hover_text("この面積より小さい候補を捨てます。")
                .changed()
            {
                *region_min_area = min_area.max(1) as usize;
            }
            if ui
                .add_enabled(
                    !segmentation_pending,
                    egui::Button::new("画像全体を領域分割"),
                )
                .clicked()
            {
                effect_requests.generate_region_mask =
                    Some((layer_idx, LocalAdjustRegionSegmentationScope::Full));
            }
            if ui
                .add_enabled(
                    !segmentation_pending && subject_mask_available,
                    egui::Button::new("被写体内を領域分割"),
                )
                .clicked()
            {
                effect_requests.generate_region_mask =
                    Some((layer_idx, LocalAdjustRegionSegmentationScope::Subject));
            }
            if ui
                .add_enabled(
                    !segmentation_pending && subject_mask_available,
                    egui::Button::new("背景を領域分割"),
                )
                .clicked()
            {
                effect_requests.generate_region_mask =
                    Some((layer_idx, LocalAdjustRegionSegmentationScope::Background));
            }
            if !subject_mask_available {
                ui.label(
                    egui::RichText::new("被写体マスクがあると、被写体内や背景だけを分割できます。")
                        .size(10.0)
                        .color(egui::Color32::from_gray(170)),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui.selectable_label(*mask_paint_add, "追加").clicked() {
                    *mask_paint_add = true;
                }
                if ui.selectable_label(!*mask_paint_add, "解除").clicked() {
                    *mask_paint_add = false;
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("全選択").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = true;
                    }
                    changed = true;
                }
                if ui.small_button("全解除").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = false;
                    }
                    changed = true;
                }
                if ui.small_button("選択反転").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = !*selected;
                    }
                    changed = true;
                }
            });
            let selected_count = mask.selected.iter().skip(1).filter(|&&v| v).count();
            ui.label(format!(
                "領域: {} / 選択: {}",
                mask.label_count(),
                selected_count
            ));
            ui.label(
                egui::RichText::new("画像上の領域をクリックして追加/解除します。")
                    .size(10.0)
                    .color(egui::Color32::from_gray(170)),
            );
        }
    }
    changed
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
    pub(crate) fn set_local_adjust_mask_tool_from_shortcut(&mut self, tool: LocalAdjustMaskTool) {
        if self.local_adjust_mask_tool != tool {
            self.local_adjust_mask_lasso_points.clear();
            self.local_adjust_mask_shape_drag_start = None;
            self.local_adjust_mask_shape_drag_end = None;
            self.local_adjust_mask_brush_stroke = None;
            self.local_adjust_shape_drag = None;
            self.local_adjust_shape_drag_before_layers = None;
        }
        self.local_adjust_mask_tool = tool;
        self.show_feedback_toast(format!("マスクツール: {}", local_mask_tool_label(tool)));
    }

    /// 補正レイヤー独立パネルの矩形を返す。
    pub(crate) fn local_adjust_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(
            full_rect.min.x + LOCAL_ADJUST_PANEL_MARGIN_X,
            full_rect.min.y + LOCAL_ADJUST_PANEL_MARGIN_Y,
        );
        let h = local_adjust_panel_outer_height(full_rect, panel_pos);
        egui::Rect::from_min_size(panel_pos, egui::vec2(LOCAL_ADJUST_PANEL_W, h))
    }

    /// 補正レイヤーの右ツールパネル矩形を返す。
    pub(crate) fn local_adjust_tool_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let x = (full_rect.max.x - LOCAL_ADJUST_TOOL_PANEL_W - LOCAL_ADJUST_PANEL_MARGIN_X)
            .max(full_rect.min.x + LOCAL_ADJUST_PANEL_MARGIN_X);
        let panel_pos = egui::pos2(x, full_rect.min.y + LOCAL_ADJUST_PANEL_MARGIN_Y);
        let h = local_adjust_panel_outer_height(full_rect, panel_pos);
        egui::Rect::from_min_size(panel_pos, egui::vec2(LOCAL_ADJUST_TOOL_PANEL_W, h))
    }

    fn current_local_adjust_edit_idx(&mut self) -> Option<usize> {
        let fs_root_idx = self.fullscreen_idx?;
        let (fs_idx, _) = match self.resolve_spread_pair(fs_root_idx) {
            SpreadPair::Double { left, right } => {
                let target = match self.adjust_spread_target {
                    AdjustSpreadTarget::Left => left,
                    AdjustSpreadTarget::Right => right,
                };
                (target, Some((left, right)))
            }
            SpreadPair::Single => (fs_root_idx, None),
        };
        Some(fs_idx)
    }

    pub(crate) fn selected_local_adjust_layer_idx(&self, fs_idx: usize) -> Option<usize> {
        let layers = self.local_adjust_page_layers.get(&fs_idx)?;
        if layers.is_empty() {
            return None;
        }
        Some(
            self.local_adjust_selected_layers
                .get(&fs_idx)
                .copied()
                .unwrap_or(0)
                .min(layers.len().saturating_sub(1)),
        )
    }

    fn mutate_local_adjust_layer_from_canvas(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        persist: bool,
        mutate: impl FnOnce(&mut local_adjust_core::LocalAdjustmentLayer) -> bool,
    ) -> bool {
        let mut layers = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let before_layers = layers.clone();
        let Some(layer) = layers.get_mut(layer_idx) else {
            return false;
        };
        if !mutate(layer) {
            return false;
        }
        self.local_adjust_selected_layers.insert(fs_idx, layer_idx);
        if persist {
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before_layers,
                layers,
                "補正レイヤーキャンバス操作".to_string(),
            );
        } else {
            self.set_local_adjust_layers_for_idx_memory_only(fs_idx, layers);
        }
        true
    }

    fn apply_local_adjust_bitmap_mask_op(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        edit_target: LocalAdjustMaskEditTarget,
        op: LocalAdjustBitmapMaskOp,
    ) {
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, true, |layer| {
                let Some(active_target) = effective_local_mask_edit_target(layer, edit_target)
                else {
                    return false;
                };
                let Some(mask) = local_adjust_target_raster_vector_mask_mut(
                    layer,
                    active_target,
                    image_dims,
                    true,
                ) else {
                    return false;
                };
                let expected_len = mask.width.saturating_mul(mask.height);
                if expected_len == 0 || mask.alpha.len() < expected_len {
                    return false;
                }
                let next = local_adjust_morph_alpha_1px(
                    &mask.alpha,
                    mask.width,
                    mask.height,
                    op == LocalAdjustBitmapMaskOp::Expand,
                );
                if next == mask.alpha {
                    return false;
                }
                mask.alpha = next;
                true
            });
        if changed {
            self.show_feedback_toast(
                match op {
                    LocalAdjustBitmapMaskOp::Expand => "手動マスクを1px拡張しました。",
                    LocalAdjustBitmapMaskOp::Shrink => "手動マスクを1px縮小しました。",
                }
                .to_string(),
            );
        }
    }

    fn apply_local_adjust_gradient_drag(
        &mut self,
        drag: crate::app::LocalAdjustCanvasDrag,
        norm: [f32; 2],
        persist: bool,
    ) -> bool {
        self.mutate_local_adjust_layer_from_canvas(drag.fs_idx, drag.layer_idx, persist, |layer| {
            match (&mut layer.mask, drag.kind) {
                (
                    local_adjust_core::LocalMask::LinearGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::LinearGradient,
                ) => {
                    mask.initialized = true;
                    mask.start = drag.start;
                    mask.end = norm;
                    true
                }
                (
                    local_adjust_core::LocalMask::LinearGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::LinearGradientStart,
                ) => {
                    mask.start = norm;
                    true
                }
                (
                    local_adjust_core::LocalMask::LinearGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd,
                ) => {
                    mask.end = norm;
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradient,
                ) => {
                    mask.initialized = true;
                    mask.center = drag.start;
                    let dx = norm[0] - drag.start[0];
                    let dy = norm[1] - drag.start[1];
                    let radius = (dx * dx + dy * dy).sqrt().max(0.001);
                    let inner = (radius * 0.45).min(radius - 0.001).max(0.0);
                    mask.inner_radius = inner;
                    mask.inner_radius_y = inner;
                    mask.outer_radius = radius;
                    mask.outer_radius_y = radius;
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientCenter,
                ) => {
                    mask.center = norm;
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerX,
                ) => {
                    mask.inner_radius = (norm[0] - mask.center[0])
                        .abs()
                        .min((mask.outer_radius - 0.001).max(0.0));
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerY,
                ) => {
                    mask.inner_radius_y = (norm[1] - mask.center[1])
                        .abs()
                        .min((mask.outer_radius_y - 0.001).max(0.0));
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX,
                ) => {
                    mask.outer_radius = (norm[0] - mask.center[0])
                        .abs()
                        .max(mask.inner_radius + 0.001);
                    true
                }
                (
                    local_adjust_core::LocalMask::RadialGradient(mask),
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterY,
                ) => {
                    mask.outer_radius_y = (norm[1] - mask.center[1])
                        .abs()
                        .max(mask.inner_radius_y + 0.001);
                    true
                }
                (_, crate::app::LocalAdjustCanvasDragKind::EffectCenter) => {
                    let Some((center, _)) = local_adjust_effect_center_mut(&mut layer.effect)
                    else {
                        return false;
                    };
                    *center = norm;
                    true
                }
                (_, crate::app::LocalAdjustCanvasDragKind::TiltShiftRange) => {
                    let local_adjust_core::LocalEffect::TiltShift(params) = &mut layer.effect
                    else {
                        return false;
                    };
                    apply_local_adjust_tilt_shift_range_drag(params, drag.start, norm)
                }
                (
                    _,
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus
                    | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter
                    | crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerX
                    | crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerY
                    | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX
                    | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterY,
                ) => {
                    let local_adjust_core::LocalEffect::TiltShift(params) = &mut layer.effect
                    else {
                        return false;
                    };
                    apply_local_adjust_tilt_shift_handle_drag(params, drag.kind, norm)
                }
                _ => false,
            }
        })
    }

    fn paint_local_adjust_mask_brush(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        from_norm: [f32; 2],
        to_norm: [f32; 2],
        paint: bool,
        persist: bool,
    ) -> bool {
        let radius = self.local_adjust_mask_brush_radius.max(1.0);
        let image_dims = local_adjust_image_dims(self, fs_idx);
        self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, persist, |layer| match target
        {
            LocalAdjustMaskEditTarget::Base => match &mut layer.mask {
                local_adjust_core::LocalMask::Raster(mask) => paint_local_adjust_alpha_line(
                    &mut mask.alpha,
                    mask.width,
                    mask.height,
                    from_norm,
                    to_norm,
                    radius,
                    paint,
                ),
                local_adjust_core::LocalMask::RasterVector(mask) => paint_local_adjust_alpha_line(
                    &mut mask.alpha,
                    mask.width,
                    mask.height,
                    from_norm,
                    to_norm,
                    radius,
                    paint,
                ),
                _ => false,
            },
            LocalAdjustMaskEditTarget::OverrideAdd
            | LocalAdjustMaskEditTarget::OverrideSubtract => {
                let Some(slot) = local_mask_override_slot_mut(layer, target) else {
                    return false;
                };
                let (width, height) = (image_dims.0.max(1), image_dims.1.max(1));
                if slot
                    .as_ref()
                    .is_none_or(|mask| mask.width != width || mask.height != height)
                {
                    if !paint {
                        return false;
                    }
                    *slot = Some(local_adjust_core::RasterVectorMask::empty(width, height));
                }
                let Some(mask) = slot.as_mut() else {
                    return false;
                };
                paint_local_adjust_alpha_line(
                    &mut mask.alpha,
                    mask.width,
                    mask.height,
                    from_norm,
                    to_norm,
                    radius,
                    paint,
                )
            }
            LocalAdjustMaskEditTarget::None => false,
        })
    }

    fn paint_local_adjust_mask_edge_brush(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        from_norm: [f32; 2],
        to_norm: [f32; 2],
        paint: bool,
        edge_seed: Option<[u8; 3]>,
        persist: bool,
    ) -> bool {
        let Some(source) = self.current_local_adjust_source_pixels(fs_idx) else {
            return false;
        };
        let radius = self.local_adjust_mask_brush_radius.max(1.0);
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let thresholds = (
            self.local_adjust_boundary_edge_threshold.clamp(0.0, 255.0),
            self.local_adjust_boundary_ink_threshold.clamp(0.0, 255.0),
            self.local_adjust_boundary_gap_px.clamp(0.0, 8.0).round() as usize,
        );
        let tolerance = self.local_adjust_edge_brush_tolerance;
        let include_boundary = self.local_adjust_edge_brush_include_boundary;
        self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, persist, |layer| {
            let Some(mask) =
                local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, paint)
            else {
                return false;
            };
            if source.size != [mask.width, mask.height] {
                return false;
            }
            paint_local_adjust_alpha_edge_brush_line(
                &mut mask.alpha,
                source.as_ref(),
                from_norm,
                to_norm,
                radius,
                paint,
                edge_seed,
                tolerance,
                thresholds,
                include_boundary,
            )
        })
    }

    fn paint_local_adjust_mask_gap_fill_brush(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        from_norm: [f32; 2],
        to_norm: [f32; 2],
        paint: bool,
        persist: bool,
    ) -> bool {
        let radius = self.local_adjust_mask_brush_radius.max(1.0);
        let gap = self.local_adjust_mask_gap_fill_distance;
        let image_dims = local_adjust_image_dims(self, fs_idx);
        self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, persist, |layer| {
            let Some(mask) =
                local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, paint)
            else {
                return false;
            };
            paint_local_adjust_alpha_gap_fill_line(
                &mut mask.alpha,
                mask.width,
                mask.height,
                from_norm,
                to_norm,
                radius,
                paint,
                gap,
            )
        })
    }

    fn paint_local_adjust_mask_tool_segment(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        tool: LocalAdjustMaskTool,
        from_norm: [f32; 2],
        to_norm: [f32; 2],
        paint: bool,
        edge_seed: Option<[u8; 3]>,
        persist: bool,
    ) -> bool {
        match tool {
            LocalAdjustMaskTool::Brush => self.paint_local_adjust_mask_brush(
                fs_idx, layer_idx, target, from_norm, to_norm, paint, persist,
            ),
            LocalAdjustMaskTool::EdgeBrush => self.paint_local_adjust_mask_edge_brush(
                fs_idx, layer_idx, target, from_norm, to_norm, paint, edge_seed, persist,
            ),
            LocalAdjustMaskTool::GapFillBrush => self.paint_local_adjust_mask_gap_fill_brush(
                fs_idx, layer_idx, target, from_norm, to_norm, paint, persist,
            ),
            _ => false,
        }
    }

    fn fill_local_adjust_mask_polygon(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        points: Vec<[f32; 2]>,
    ) -> bool {
        if points.len() < 3 {
            return false;
        }
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let before = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let paint = self.local_adjust_mask_paint_add;
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, false, |layer| {
                let Some(mask) =
                    local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, paint)
                else {
                    return false;
                };
                fill_local_adjust_alpha_polygon(
                    &mut mask.alpha,
                    mask.width,
                    mask.height,
                    &points,
                    paint,
                )
            });
        if changed {
            let layers = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .cloned()
                .unwrap_or_default();
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before,
                layers,
                "補正レイヤー囲みマスク".to_string(),
            );
        }
        changed
    }

    fn commit_local_adjust_mask_shape(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        shape: local_adjust_core::MaskShape,
    ) -> bool {
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let before = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, false, |layer| {
                let Some(mask) =
                    local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, true)
                else {
                    return false;
                };
                mask.shapes.push(shape);
                true
            });
        if changed {
            let layers = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .cloned()
                .unwrap_or_default();
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before,
                layers,
                "補正レイヤー図形マスク".to_string(),
            );
            if let Some(layers) = self.local_adjust_page_layers.get(&fs_idx)
                && let Some(layer) = layers.get(layer_idx)
                && let Some(mask) = local_adjust_target_raster_vector_mask_ref(layer, target)
            {
                self.local_adjust_selected_shape = mask.shapes.len().checked_sub(1);
            }
        }
        changed
    }

    fn hit_test_local_adjust_mask_shapes(
        &self,
        fs_idx: usize,
        layer_idx: usize,
        target: LocalAdjustMaskEditTarget,
        point: [f32; 2],
        handle_hit_radius_px: f32,
    ) -> Option<(usize, LocalAdjustShapeHandle)> {
        let layer = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))?;
        let mask = local_adjust_target_raster_vector_mask_ref(layer, target)?;
        if let Some(selected) = self.local_adjust_selected_shape
            && let Some(shape) = mask.shapes.get(selected)
            && let Some(handle) =
                hit_local_adjust_shape_handles(*shape, point, handle_hit_radius_px)
        {
            return Some((selected, handle));
        }
        for (idx, shape) in mask.shapes.iter().enumerate().rev() {
            if self.local_adjust_selected_shape == Some(idx) {
                continue;
            }
            if let Some(handle) =
                hit_local_adjust_shape_handles(*shape, point, handle_hit_radius_px)
            {
                return Some((idx, handle));
            }
        }
        for (idx, shape) in mask.shapes.iter().enumerate().rev() {
            if local_adjust_shape_contains(*shape, point) {
                return Some((idx, LocalAdjustShapeHandle::Body));
            }
        }
        None
    }

    fn apply_local_adjust_shape_drag_from_canvas(
        &mut self,
        drag: LocalAdjustMaskShapeDrag,
        point: [f32; 2],
        modifiers: egui::Modifiers,
    ) -> bool {
        let image_dims = local_adjust_image_dims(self, drag.fs_idx);
        self.mutate_local_adjust_layer_from_canvas(drag.fs_idx, drag.layer_idx, false, |layer| {
            let Some(mask) =
                local_adjust_target_raster_vector_mask_mut(layer, drag.target, image_dims, false)
            else {
                return false;
            };
            let Some(slot) = mask.shapes.get_mut(drag.shape_idx) else {
                return false;
            };
            *slot = apply_local_adjust_shape_drag(drag, point, modifiers);
            true
        })
    }

    fn persist_local_adjust_shape_drag(&mut self) {
        let Some(drag) = self.local_adjust_shape_drag.take() else {
            return;
        };
        let Some(mut layers) = self.local_adjust_page_layers.get(&drag.fs_idx).cloned() else {
            return;
        };
        if let Some(layer) = layers.get_mut(drag.layer_idx) {
            compact_local_adjust_manual_override(layer);
        }
        let before = self
            .local_adjust_shape_drag_before_layers
            .take()
            .unwrap_or_else(|| layers.clone());
        if before == layers {
            return;
        }
        self.local_adjust_selected_layers
            .insert(drag.fs_idx, drag.layer_idx);
        self.local_adjust_selected_shape = Some(drag.shape_idx);
        self.set_local_adjust_layers_for_idx_with_undo(
            drag.fs_idx,
            before,
            layers,
            "補正レイヤー図形マスク編集".to_string(),
        );
    }

    fn persist_local_adjust_mask_brush_stroke(&mut self) {
        let Some(stroke) = self.local_adjust_mask_brush_stroke.take() else {
            return;
        };
        let Some(mut layers) = self.local_adjust_page_layers.get(&stroke.fs_idx).cloned() else {
            return;
        };
        if let Some(layer) = layers.get_mut(stroke.layer_idx) {
            compact_local_adjust_manual_override(layer);
        }
        self.local_adjust_selected_layers
            .insert(stroke.fs_idx, stroke.layer_idx);
        let before = self
            .local_adjust_mask_brush_before_layers
            .take()
            .unwrap_or_else(|| layers.clone());
        self.set_local_adjust_layers_for_idx_with_undo(
            stroke.fs_idx,
            before,
            layers,
            "補正レイヤー手描きマスク".to_string(),
        );
    }

    fn persist_local_adjust_canvas_drag(&mut self) {
        let Some(drag) = self.local_adjust_canvas_drag.take() else {
            return;
        };
        let Some(layers) = self.local_adjust_page_layers.get(&drag.fs_idx).cloned() else {
            return;
        };
        let before = self
            .local_adjust_canvas_drag_before_layers
            .take()
            .unwrap_or_else(|| layers.clone());
        self.set_local_adjust_layers_for_idx_with_undo(
            drag.fs_idx,
            before,
            layers,
            "補正レイヤーキャンバス操作".to_string(),
        );
    }

    fn toggle_local_adjust_region_at(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        norm: [f32; 2],
        selected: bool,
    ) -> bool {
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let (px, py) = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
        let x = px.round().clamp(0.0, image_dims.0.saturating_sub(1) as f32) as usize;
        let y = py.round().clamp(0.0, image_dims.1.saturating_sub(1) as f32) as usize;
        let mut changed_label = None;
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, true, |layer| {
                let local_adjust_core::LocalMask::Segmentation(mask) = &mut layer.mask else {
                    return false;
                };
                if x >= mask.width || y >= mask.height {
                    return false;
                }
                let label = mask.labels[y * mask.width + x] as usize;
                if label == 0 {
                    return false;
                }
                let Some(slot) = mask.selected.get_mut(label) else {
                    return false;
                };
                if *slot == selected {
                    return false;
                }
                *slot = selected;
                changed_label = Some(label);
                true
            });
        if let Some(label) = changed_label {
            self.show_feedback_toast(if selected {
                format!("領域 {label} を選択しました")
            } else {
                format!("領域 {label} を解除しました")
            });
        }
        changed
    }

    pub(crate) fn handle_local_adjust_canvas_input(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if !self.local_adjust_mode {
            return;
        }
        if self.local_adjust_add_layer_dialog_open
            || self.local_adjust_change_mask_dialog_open
            || self.local_adjust_effect_picker_dialog_open
        {
            self.local_adjust_canvas_drag = None;
            self.local_adjust_mask_brush_stroke = None;
            self.local_adjust_shape_drag = None;
            return;
        }
        let Some(fs_idx) = self.current_local_adjust_edit_idx() else {
            self.local_adjust_mask_brush_stroke = None;
            return;
        };
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            self.local_adjust_canvas_drag = None;
            self.local_adjust_mask_brush_stroke = None;
            return;
        };
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let (
            primary_pressed,
            primary_down,
            primary_released,
            secondary_pressed,
            modifiers,
            pointer_pos,
        ) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.secondary_pressed(),
                i.modifiers,
                i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()),
            )
        });

        if primary_released {
            if self.local_adjust_shape_drag.is_some() {
                self.persist_local_adjust_shape_drag();
                ctx.request_repaint();
                return;
            }
            let active_mask_edit_target = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .and_then(|layer| {
                    effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
                });
            if let Some(target) = active_mask_edit_target {
                match self.local_adjust_mask_tool {
                    LocalAdjustMaskTool::Lasso => {
                        if let Some(pos) = pointer_pos
                            && let Some(norm) = local_adjust_screen_to_norm(
                                pos, image_rect, image_dims, zoom_pan, false,
                            )
                        {
                            let p = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                            self.local_adjust_mask_lasso_points.push([p.0, p.1]);
                        }
                        let points = std::mem::take(&mut self.local_adjust_mask_lasso_points);
                        if points.len() >= 3 {
                            self.fill_local_adjust_mask_polygon(fs_idx, layer_idx, target, points);
                            ctx.request_repaint();
                            return;
                        }
                    }
                    LocalAdjustMaskTool::Line
                    | LocalAdjustMaskTool::VertLine
                    | LocalAdjustMaskTool::HorizLine
                    | LocalAdjustMaskTool::Rect
                    | LocalAdjustMaskTool::Ellipse => {
                        if let (Some(start), Some(end)) = (
                            self.local_adjust_mask_shape_drag_start.take(),
                            self.local_adjust_mask_shape_drag_end.take(),
                        ) && let Some(shape) = make_local_adjust_shape(
                            self.local_adjust_mask_tool,
                            start,
                            end,
                            self.local_adjust_mask_line_width,
                            image_dims,
                            self.local_adjust_mask_paint_add,
                        ) {
                            self.commit_local_adjust_mask_shape(fs_idx, layer_idx, target, shape);
                            ctx.request_repaint();
                            return;
                        }
                    }
                    _ => {}
                }
            }
            self.persist_local_adjust_mask_brush_stroke();
            self.persist_local_adjust_canvas_drag();
            return;
        }

        if let Some(mut stroke) = self.local_adjust_mask_brush_stroke {
            if primary_down {
                if let Some(pos) = pointer_pos
                    && let Some(norm) =
                        local_adjust_screen_to_norm(pos, image_rect, image_dims, zoom_pan, false)
                {
                    self.paint_local_adjust_mask_tool_segment(
                        stroke.fs_idx,
                        stroke.layer_idx,
                        stroke.target,
                        stroke.tool,
                        stroke.previous,
                        norm,
                        stroke.paint,
                        stroke.edge_seed,
                        false,
                    );
                    stroke.previous = norm;
                    self.local_adjust_mask_brush_stroke = Some(stroke);
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    ctx.request_repaint();
                }
                return;
            }
            self.local_adjust_mask_brush_stroke = None;
        }

        if let Some(drag) = self.local_adjust_canvas_drag {
            if primary_down {
                if let Some(pos) = pointer_pos
                    && let Some(norm) =
                        local_adjust_screen_to_norm(pos, image_rect, image_dims, zoom_pan, false)
                {
                    self.apply_local_adjust_gradient_drag(drag, norm, false);
                    let cursor = if matches!(
                        drag.kind,
                        crate::app::LocalAdjustCanvasDragKind::LinearGradientStart
                            | crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd
                            | crate::app::LocalAdjustCanvasDragKind::RadialGradientCenter
                            | crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerX
                            | crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerY
                            | crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX
                            | crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterY
                            | crate::app::LocalAdjustCanvasDragKind::EffectCenter
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftRange
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerX
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerY
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX
                            | crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterY
                    ) {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Crosshair
                    };
                    ctx.set_cursor_icon(cursor);
                    ctx.request_repaint();
                }
                return;
            }
            self.local_adjust_canvas_drag = None;
        }

        let Some(pos) = pointer_pos else {
            return;
        };
        if self.local_adjust_panel_rect(full_rect).contains(pos)
            || self.local_adjust_tool_panel_rect(full_rect).contains(pos)
        {
            return;
        }
        let Some(norm) = local_adjust_screen_to_norm(pos, image_rect, image_dims, zoom_pan, true)
        else {
            return;
        };

        let active_mask_edit_target = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
            });

        if let Some(drag) = self.local_adjust_shape_drag
            && primary_down
        {
            let point = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
            self.apply_local_adjust_shape_drag_from_canvas(drag, [point.0, point.1], modifiers);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            ctx.request_repaint();
            return;
        }

        if let Some(target) = active_mask_edit_target {
            match self.local_adjust_mask_tool {
                LocalAdjustMaskTool::Lasso if primary_down && !primary_pressed => {
                    let p = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                    self.local_adjust_mask_lasso_points.push([p.0, p.1]);
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    ctx.request_repaint();
                    return;
                }
                LocalAdjustMaskTool::Line
                | LocalAdjustMaskTool::VertLine
                | LocalAdjustMaskTool::HorizLine
                | LocalAdjustMaskTool::Rect
                | LocalAdjustMaskTool::Ellipse
                    if primary_down && !primary_pressed =>
                {
                    self.local_adjust_mask_shape_drag_end = Some(norm);
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    ctx.request_repaint();
                    return;
                }
                LocalAdjustMaskTool::Polygon if secondary_pressed => {
                    let points = std::mem::take(&mut self.local_adjust_mask_lasso_points);
                    if points.len() >= 3 {
                        self.fill_local_adjust_mask_polygon(fs_idx, layer_idx, target, points);
                        ctx.request_repaint();
                    }
                    return;
                }
                _ => {}
            }
        }

        if primary_pressed {
            if self.local_adjust_selective_color_pick_active {
                if let Some(rgb) = sample_local_adjust_rgb(self, fs_idx, norm) {
                    let hue = crate::local_adjust_effect_ui::hue_degrees_from_rgb(rgb);
                    if self.mutate_local_adjust_layer_from_canvas(
                        fs_idx,
                        layer_idx,
                        true,
                        |layer| {
                            if let local_adjust_core::LocalEffect::SelectiveColor(params) =
                                &mut layer.effect
                            {
                                params.target_hue_degrees = hue;
                                true
                            } else {
                                false
                            }
                        },
                    ) {
                        self.show_feedback_toast(format!("選択色: {:.0}°", hue));
                    }
                }
                self.local_adjust_selective_color_pick_active = false;
                return;
            }

            if let Some(target) = self.local_adjust_rgb_pick_active {
                if let Some(rgb) = sample_local_adjust_rgb(self, fs_idx, norm) {
                    if self.mutate_local_adjust_layer_from_canvas(
                        fs_idx,
                        layer_idx,
                        true,
                        |layer| {
                            crate::local_adjust_effect_ui::set_rgb_pick_target(
                                &mut layer.effect,
                                target,
                                rgb,
                            )
                        },
                    ) {
                        self.show_feedback_toast(format!(
                            "{}: #{:02X}{:02X}{:02X}",
                            target.label(),
                            rgb[0],
                            rgb[1],
                            rgb[2]
                        ));
                    }
                }
                self.local_adjust_rgb_pick_active = None;
                return;
            }

            if let Some(kind) = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .and_then(|layer| {
                    local_adjust_gradient_handle_hit(layer, pos, image_rect, image_dims, zoom_pan)
                })
            {
                self.local_adjust_canvas_drag_before_layers =
                    self.local_adjust_page_layers.get(&fs_idx).cloned();
                let drag = crate::app::LocalAdjustCanvasDrag {
                    fs_idx,
                    layer_idx,
                    kind,
                    start: norm,
                };
                self.local_adjust_canvas_drag = Some(drag);
                self.apply_local_adjust_gradient_drag(drag, norm, false);
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                return;
            }

            if self.local_adjust_effect_position_handles_visible {
                let tilt_shift_range_pending = self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .is_some_and(|layer| {
                        local_adjust_tilt_shift_range_create_pending(&layer.effect)
                    });
                if tilt_shift_range_pending {
                    self.local_adjust_canvas_drag_before_layers =
                        self.local_adjust_page_layers.get(&fs_idx).cloned();
                    let drag = crate::app::LocalAdjustCanvasDrag {
                        fs_idx,
                        layer_idx,
                        kind: crate::app::LocalAdjustCanvasDragKind::TiltShiftRange,
                        start: norm,
                    };
                    self.local_adjust_canvas_drag = Some(drag);
                    self.apply_local_adjust_gradient_drag(drag, norm, false);
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    return;
                }
                let tilt_shift_handle = self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .and_then(|layer| {
                        let (_, drawn_rect) =
                            local_adjust_image_layout(image_rect, image_dims, zoom_pan)?;
                        local_adjust_tilt_shift_handle_hit(&layer.effect, pos, drawn_rect)
                    });
                if let Some(kind) = tilt_shift_handle {
                    self.local_adjust_canvas_drag_before_layers =
                        self.local_adjust_page_layers.get(&fs_idx).cloned();
                    let drag = crate::app::LocalAdjustCanvasDrag {
                        fs_idx,
                        layer_idx,
                        kind,
                        start: norm,
                    };
                    self.local_adjust_canvas_drag = Some(drag);
                    self.apply_local_adjust_gradient_drag(drag, norm, false);
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                    return;
                }
                let center_hit = self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .and_then(|layer| local_adjust_effect_center(&layer.effect))
                    .and_then(|(center, _)| {
                        local_adjust_norm_to_screen(center, image_rect, image_dims, zoom_pan)
                    })
                    .is_some_and(|center| center.distance(pos) <= 14.0);
                if center_hit {
                    self.local_adjust_canvas_drag_before_layers =
                        self.local_adjust_page_layers.get(&fs_idx).cloned();
                    let drag = crate::app::LocalAdjustCanvasDrag {
                        fs_idx,
                        layer_idx,
                        kind: crate::app::LocalAdjustCanvasDragKind::EffectCenter,
                        start: norm,
                    };
                    self.local_adjust_canvas_drag = Some(drag);
                    self.apply_local_adjust_gradient_drag(drag, norm, false);
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                    return;
                }
            }

            if let Some(target) = active_mask_edit_target {
                match self.local_adjust_mask_tool {
                    LocalAdjustMaskTool::Select => {
                        let point = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                        let handle_hit_radius_px =
                            local_adjust_image_layout(image_rect, image_dims, zoom_pan)
                                .map(|(scale, _)| (12.0 / scale.max(0.001)).clamp(12.0, 96.0))
                                .unwrap_or(12.0);
                        if let Some((shape_idx, handle)) = self.hit_test_local_adjust_mask_shapes(
                            fs_idx,
                            layer_idx,
                            target,
                            [point.0, point.1],
                            handle_hit_radius_px,
                        ) {
                            self.local_adjust_shape_drag_before_layers =
                                self.local_adjust_page_layers.get(&fs_idx).cloned();
                            self.local_adjust_selected_shape = Some(shape_idx);
                            if let Some(layer) = self
                                .local_adjust_page_layers
                                .get(&fs_idx)
                                .and_then(|layers| layers.get(layer_idx))
                                && let Some(mask) =
                                    local_adjust_target_raster_vector_mask_ref(layer, target)
                                && let Some(shape) = mask.shapes.get(shape_idx).copied()
                            {
                                self.local_adjust_shape_drag = Some(LocalAdjustMaskShapeDrag {
                                    fs_idx,
                                    layer_idx,
                                    target,
                                    shape_idx,
                                    handle,
                                    base: shape,
                                    origin: [point.0, point.1],
                                });
                            }
                        } else {
                            self.local_adjust_selected_shape = None;
                            self.local_adjust_shape_drag = None;
                        }
                    }
                    LocalAdjustMaskTool::Brush
                    | LocalAdjustMaskTool::EdgeBrush
                    | LocalAdjustMaskTool::GapFillBrush => {
                        self.local_adjust_mask_brush_before_layers =
                            self.local_adjust_page_layers.get(&fs_idx).cloned();
                        let tool = self.local_adjust_mask_tool;
                        let edge_seed = (tool == LocalAdjustMaskTool::EdgeBrush)
                            .then(|| sample_local_adjust_rgb(self, fs_idx, norm))
                            .flatten();
                        let stroke = crate::app::LocalAdjustMaskBrushStroke {
                            fs_idx,
                            layer_idx,
                            target,
                            tool,
                            paint: self.local_adjust_mask_paint_add,
                            edge_seed,
                            previous: norm,
                        };
                        self.local_adjust_mask_brush_stroke = Some(stroke);
                        self.paint_local_adjust_mask_tool_segment(
                            fs_idx,
                            layer_idx,
                            target,
                            tool,
                            norm,
                            norm,
                            self.local_adjust_mask_paint_add,
                            edge_seed,
                            false,
                        );
                    }
                    LocalAdjustMaskTool::Lasso => {
                        self.local_adjust_mask_lasso_points.clear();
                        let p = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                        self.local_adjust_mask_lasso_points.push([p.0, p.1]);
                    }
                    LocalAdjustMaskTool::Polygon => {
                        let p = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                        let close = self.local_adjust_mask_lasso_points.len() >= 3
                            && self
                                .local_adjust_mask_lasso_points
                                .first()
                                .is_some_and(|first| {
                                    let dx = first[0] - p.0;
                                    let dy = first[1] - p.1;
                                    dx * dx + dy * dy <= 12.0 * 12.0
                                });
                        if close {
                            let points = std::mem::take(&mut self.local_adjust_mask_lasso_points);
                            self.fill_local_adjust_mask_polygon(fs_idx, layer_idx, target, points);
                        } else {
                            self.local_adjust_mask_lasso_points.push([p.0, p.1]);
                        }
                    }
                    LocalAdjustMaskTool::Line
                    | LocalAdjustMaskTool::VertLine
                    | LocalAdjustMaskTool::HorizLine
                    | LocalAdjustMaskTool::Rect
                    | LocalAdjustMaskTool::Ellipse => {
                        self.local_adjust_mask_shape_drag_start = Some(norm);
                        self.local_adjust_mask_shape_drag_end = Some(norm);
                    }
                }
                ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                ctx.request_repaint();
                return;
            }

            let mask_kind = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .map(|layer| MaskKind::from_mask(&layer.mask));
            match mask_kind {
                Some(MaskKind::ColorRange) => {
                    if let Some(rgb) = sample_local_adjust_rgb(self, fs_idx, norm) {
                        if self.mutate_local_adjust_layer_from_canvas(
                            fs_idx,
                            layer_idx,
                            true,
                            |layer| {
                                if let local_adjust_core::LocalMask::ColorRange(mask) =
                                    &mut layer.mask
                                {
                                    mask.initialized = true;
                                    mask.target_rgb = rgb;
                                    true
                                } else {
                                    false
                                }
                            },
                        ) {
                            self.show_feedback_toast(format!(
                                "カラー範囲: #{:02X}{:02X}{:02X}",
                                rgb[0], rgb[1], rgb[2]
                            ));
                        }
                    }
                }
                Some(MaskKind::Segmentation) => {
                    self.toggle_local_adjust_region_at(
                        fs_idx,
                        layer_idx,
                        norm,
                        self.local_adjust_mask_paint_add,
                    );
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                Some(MaskKind::LinearGradient) => {
                    let create_pending = self
                        .local_adjust_page_layers
                        .get(&fs_idx)
                        .and_then(|layers| layers.get(layer_idx))
                        .is_some_and(local_adjust_gradient_create_pending);
                    if create_pending {
                        self.local_adjust_canvas_drag_before_layers =
                            self.local_adjust_page_layers.get(&fs_idx).cloned();
                        let drag = crate::app::LocalAdjustCanvasDrag {
                            fs_idx,
                            layer_idx,
                            kind: crate::app::LocalAdjustCanvasDragKind::LinearGradient,
                            start: norm,
                        };
                        self.local_adjust_canvas_drag = Some(drag);
                        self.apply_local_adjust_gradient_drag(drag, norm, false);
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                }
                Some(MaskKind::RadialGradient) => {
                    let create_pending = self
                        .local_adjust_page_layers
                        .get(&fs_idx)
                        .and_then(|layers| layers.get(layer_idx))
                        .is_some_and(local_adjust_gradient_create_pending);
                    if create_pending {
                        self.local_adjust_canvas_drag_before_layers =
                            self.local_adjust_page_layers.get(&fs_idx).cloned();
                        let drag = crate::app::LocalAdjustCanvasDrag {
                            fs_idx,
                            layer_idx,
                            kind: crate::app::LocalAdjustCanvasDragKind::RadialGradient,
                            start: norm,
                        };
                        self.local_adjust_canvas_drag = Some(drag);
                        self.apply_local_adjust_gradient_drag(drag, norm, false);
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                }
                _ => {}
            }
        } else if active_mask_edit_target.is_some_and(|target| {
            if self.local_adjust_mask_tool != LocalAdjustMaskTool::Select {
                return false;
            }
            let point = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
            let handle_hit_radius_px = local_adjust_image_layout(image_rect, image_dims, zoom_pan)
                .map(|(scale, _)| (12.0 / scale.max(0.001)).clamp(12.0, 96.0))
                .unwrap_or(12.0);
            self.hit_test_local_adjust_mask_shapes(
                fs_idx,
                layer_idx,
                target,
                [point.0, point.1],
                handle_hit_radius_px,
            )
            .is_some()
        }) {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        } else if self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                local_adjust_gradient_handle_hit(layer, pos, image_rect, image_dims, zoom_pan)
            })
            .is_some()
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        } else if self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
            })
            .is_some()
            || matches!(
                self.local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .map(|layer| MaskKind::from_mask(&layer.mask)),
                Some(MaskKind::ColorRange) | Some(MaskKind::Segmentation)
            )
            || self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .is_some_and(local_adjust_gradient_create_pending)
            || self.local_adjust_selective_color_pick_active
            || self.local_adjust_rgb_pick_active.is_some()
        {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if self.local_adjust_effect_position_handles_visible
            && self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .is_some_and(|layer| local_adjust_tilt_shift_range_create_pending(&layer.effect))
        {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if self.local_adjust_effect_position_handles_visible
            && self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .is_some_and(|layer| {
                    let tilt_shift_hit =
                        local_adjust_image_layout(image_rect, image_dims, zoom_pan)
                            .and_then(|(_, drawn_rect)| {
                                local_adjust_tilt_shift_handle_hit(&layer.effect, pos, drawn_rect)
                            })
                            .is_some();
                    let center_hit = local_adjust_effect_center(&layer.effect)
                        .and_then(|(center, _)| {
                            local_adjust_norm_to_screen(center, image_rect, image_dims, zoom_pan)
                        })
                        .is_some_and(|center| center.distance(pos) <= 14.0);
                    tilt_shift_hit || center_hit
                })
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    pub(crate) fn draw_local_adjust_canvas_overlay(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if !self.local_adjust_mode {
            return;
        }
        if self.local_adjust_add_layer_dialog_open
            || self.local_adjust_change_mask_dialog_open
            || self.local_adjust_effect_picker_dialog_open
        {
            return;
        }
        let Some(fs_idx) = self.current_local_adjust_edit_idx() else {
            return;
        };
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            return;
        };
        let Some(layer) = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
        else {
            return;
        };
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let painter = ui.painter();
        let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 226, 120));
        let source_pixels = self.current_local_adjust_source_pixels(fs_idx);
        let alt_down = ui.ctx().input(|i| i.modifiers.alt);
        if local_adjust_mask_preview_active(
            self.local_adjust_mode,
            self.local_adjust_show_mask,
            alt_down,
        ) && let Some((_, drawn_rect)) =
            local_adjust_image_layout(image_rect, image_dims, zoom_pan)
        {
            draw_local_adjust_mask_preview_overlay(
                painter,
                drawn_rect,
                layer,
                source_pixels.as_deref(),
                image_dims,
                ui.ctx().input(|i| i.time) as f32,
                self.local_adjust_mask_color_preset.colors(),
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target),
                &mut self.local_adjust_mask_preview_texture,
            );
            if matches!(layer.mask, local_adjust_core::LocalMask::Segmentation(_)) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(
                        LOCAL_ADJUST_REGION_BOUNDARY_ANIM_INTERVAL_MS,
                    ));
            }
        }
        let shape_to_screen =
            local_adjust_image_layout(image_rect, image_dims, zoom_pan).map(|(_, drawn_rect)| {
                let w = image_dims.0.max(1) as f32;
                let h = image_dims.1.max(1) as f32;
                move |p: [f32; 2]| -> egui::Pos2 {
                    egui::pos2(
                        drawn_rect.left() + p[0] / w * drawn_rect.width(),
                        drawn_rect.top() + p[1] / h * drawn_rect.height(),
                    )
                }
            });
        let active_mask_edit_target =
            effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target);
        if effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target).is_some()
            && matches!(
                self.local_adjust_mask_tool,
                LocalAdjustMaskTool::Brush
                    | LocalAdjustMaskTool::EdgeBrush
                    | LocalAdjustMaskTool::GapFillBrush
            )
            && let Some(pointer) = ui.ctx().input(|i| i.pointer.hover_pos())
            && local_adjust_screen_to_norm(pointer, image_rect, image_dims, zoom_pan, true)
                .is_some()
            && let Some((scale, _)) = local_adjust_image_layout(image_rect, image_dims, zoom_pan)
        {
            let color = match (
                self.local_adjust_mask_tool,
                self.local_adjust_mask_paint_add,
            ) {
                (_, false) => egui::Color32::from_rgb(255, 120, 120),
                (LocalAdjustMaskTool::EdgeBrush, true) => egui::Color32::from_rgb(120, 220, 255),
                (LocalAdjustMaskTool::GapFillBrush, true) => egui::Color32::from_rgb(160, 255, 150),
                _ => egui::Color32::from_rgb(255, 226, 120),
            };
            painter.circle_stroke(
                pointer,
                (self.local_adjust_mask_brush_radius * scale).max(1.0),
                egui::Stroke::new(1.5, color),
            );
        }
        if let (Some(target), Some(to_screen)) = (active_mask_edit_target, shape_to_screen.as_ref())
        {
            if let Some(mask) = local_adjust_target_raster_vector_mask_ref(layer, target) {
                for (idx, shape) in mask.shapes.iter().copied().enumerate() {
                    let selected = self.local_adjust_selected_shape == Some(idx);
                    let color = if shape.op().is_add() {
                        egui::Color32::from_rgb(255, 180, 64)
                    } else {
                        egui::Color32::from_rgb(80, 210, 255)
                    };
                    draw_local_adjust_shape_outline(painter, shape, to_screen, color, selected);
                    if selected {
                        draw_local_adjust_shape_handles(painter, shape, to_screen);
                    }
                }
            }

            if self.local_adjust_mask_lasso_points.len() >= 2 {
                let points: Vec<egui::Pos2> = self
                    .local_adjust_mask_lasso_points
                    .iter()
                    .map(|&p| to_screen(p))
                    .collect();
                painter.add(egui::Shape::line(
                    points.clone(),
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 220, 80)),
                ));
                painter.line_segment(
                    [*points.last().unwrap(), points[0]],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100),
                    ),
                );
                if self.local_adjust_mask_tool == LocalAdjustMaskTool::Polygon {
                    for (idx, point) in points.into_iter().enumerate() {
                        let fill = if idx == 0 {
                            egui::Color32::from_rgb(255, 245, 120)
                        } else {
                            egui::Color32::from_rgb(255, 220, 80)
                        };
                        painter.circle_filled(point, 4.0, fill);
                        painter.circle_stroke(
                            point,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::BLACK),
                        );
                    }
                }
            } else if self.local_adjust_mask_tool == LocalAdjustMaskTool::Polygon
                && self.local_adjust_mask_lasso_points.len() == 1
            {
                let point = to_screen(self.local_adjust_mask_lasso_points[0]);
                painter.circle_filled(point, 4.0, egui::Color32::from_rgb(255, 245, 120));
                painter.circle_stroke(point, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
            }

            if let (Some(start), Some(end)) = (
                self.local_adjust_mask_shape_drag_start,
                self.local_adjust_mask_shape_drag_end,
            ) && let Some(shape) = make_local_adjust_shape(
                self.local_adjust_mask_tool,
                start,
                end,
                self.local_adjust_mask_line_width,
                image_dims,
                self.local_adjust_mask_paint_add,
            ) {
                draw_local_adjust_shape_outline(
                    painter,
                    shape,
                    to_screen,
                    egui::Color32::from_rgb(255, 240, 120),
                    true,
                );
            }
        }
        if self.local_adjust_effect_position_handles_visible
            && let Some((_, drawn_rect)) =
                local_adjust_image_layout(image_rect, image_dims, zoom_pan)
        {
            draw_local_adjust_effect_position_overlay(
                painter,
                drawn_rect,
                image_dims,
                &layer.effect,
            );
        }
        match &layer.mask {
            local_adjust_core::LocalMask::LinearGradient(mask) if mask.initialized => {
                let Some(start) =
                    local_adjust_norm_to_screen(mask.start, image_rect, image_dims, zoom_pan)
                else {
                    return;
                };
                let Some(end) =
                    local_adjust_norm_to_screen(mask.end, image_rect, image_dims, zoom_pan)
                else {
                    return;
                };
                painter.line_segment([start, end], stroke);
                painter.circle_filled(start, 5.0, egui::Color32::from_rgb(255, 238, 145));
                painter.circle_stroke(start, 5.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
                painter.circle_filled(end, 5.0, egui::Color32::from_rgb(80, 210, 255));
                painter.circle_stroke(end, 5.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
            }
            local_adjust_core::LocalMask::RadialGradient(mask) if mask.initialized => {
                let Some(center) =
                    local_adjust_norm_to_screen(mask.center, image_rect, image_dims, zoom_pan)
                else {
                    return;
                };
                let Some((_, drawn_rect)) =
                    local_adjust_image_layout(image_rect, image_dims, zoom_pan)
                else {
                    return;
                };
                let inner_rx = mask.inner_radius.max(0.0) * drawn_rect.width();
                let inner_ry = mask.inner_radius_y.max(0.0) * drawn_rect.height();
                let outer_rx =
                    mask.outer_radius.max(mask.inner_radius).max(0.0) * drawn_rect.width();
                let outer_ry =
                    mask.outer_radius_y.max(mask.inner_radius_y).max(0.0) * drawn_rect.height();
                let inner_x_handle = egui::pos2(center.x + inner_rx, center.y);
                let inner_y_handle = egui::pos2(center.x, center.y + inner_ry);
                let outer_x_handle = egui::pos2(center.x + outer_rx, center.y);
                let outer_y_handle = egui::pos2(center.x, center.y + outer_ry);
                draw_local_adjust_ellipse(
                    painter,
                    image_rect,
                    image_dims,
                    zoom_pan,
                    mask.center,
                    mask.outer_radius,
                    mask.outer_radius_y,
                    stroke,
                );
                if mask.inner_radius > 0.001 || mask.inner_radius_y > 0.001 {
                    draw_local_adjust_ellipse(
                        painter,
                        image_rect,
                        image_dims,
                        zoom_pan,
                        mask.center,
                        mask.inner_radius,
                        mask.inner_radius_y,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 220, 255)),
                    );
                }
                painter.line_segment(
                    [egui::pos2(center.x - outer_rx, center.y), outer_x_handle],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 80, 100),
                    ),
                );
                painter.line_segment(
                    [egui::pos2(center.x, center.y - outer_ry), outer_y_handle],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 80, 100),
                    ),
                );
                painter.circle_filled(center, 5.0, egui::Color32::from_rgb(255, 238, 145));
                painter.circle_stroke(center, 5.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
                painter.circle_filled(inner_x_handle, 4.5, egui::Color32::from_rgb(255, 230, 140));
                painter.circle_stroke(
                    inner_x_handle,
                    4.5,
                    egui::Stroke::new(1.5, egui::Color32::BLACK),
                );
                painter.circle_filled(inner_y_handle, 4.5, egui::Color32::from_rgb(255, 230, 140));
                painter.circle_stroke(
                    inner_y_handle,
                    4.5,
                    egui::Stroke::new(1.5, egui::Color32::BLACK),
                );
                painter.circle_filled(outer_x_handle, 5.0, egui::Color32::from_rgb(255, 190, 110));
                painter.circle_stroke(
                    outer_x_handle,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::BLACK),
                );
                painter.circle_filled(outer_y_handle, 5.0, egui::Color32::from_rgb(255, 190, 110));
                painter.circle_stroke(
                    outer_y_handle,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::BLACK),
                );
            }
            _ => {}
        }
    }

    fn apply_local_adjust_panel_actions(
        &mut self,
        fs_idx: usize,
        add_layer_mask: Option<MaskKind>,
        change_layer_mask: Option<(usize, MaskKind, bool)>,
        set_layer_effect: Option<(usize, EffectKind)>,
        select_layer: Option<usize>,
        set_enabled: Option<(usize, bool)>,
        update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
        move_layer: Option<(usize, usize)>,
        duplicate_layer: Option<usize>,
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
        let before_layers = layers.clone();
        let mut changed = false;
        let mut selected_after: Option<usize> = None;
        let mut undo_summary: Option<String> = None;
        let mut auto_generate_after_change: Option<(usize, MaskKind)> = None;

        let image_dims = local_adjust_image_dims(self, fs_idx);
        if let Some(mask_kind) = add_layer_mask {
            layers.push(layer_with_local_mask(
                mask_kind.label(),
                mask_kind,
                image_dims,
            ));
            selected_after = Some(layers.len().saturating_sub(1));
            if matches!(mask_kind, MaskKind::Subject | MaskKind::Segmentation) {
                auto_generate_after_change = Some((layers.len().saturating_sub(1), mask_kind));
            }
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤー追加".to_string());
            self.show_feedback_toast(format!("補正レイヤーを追加しました: {}", mask_kind.label()));
        }
        if let Some((layer_idx, kind)) = set_layer_effect
            && let Some(layer) = layers.get_mut(layer_idx)
        {
            layer.effect = kind.default_effect();
            layer.name = kind.label().to_string();
            let mask_application =
                local_adjust_core::default_mask_application_for_effect(&layer.effect);
            layer.mask_before_effect = mask_application.before_effect;
            layer.mask_after_effect = mask_application.after_effect;
            selected_after = Some(layer_idx);
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤー効果選択".to_string());
            self.local_adjust_selective_color_pick_active = false;
            self.local_adjust_rgb_pick_active = None;
            self.show_feedback_toast(format!("加工内容を変更: {}", kind.label()));
        }
        if let Some((layer_idx, mask_kind, keep_manual_override)) = change_layer_mask
            && let Some(layer) = layers.get_mut(layer_idx)
        {
            replace_local_adjust_layer_base_mask(
                layer,
                mask_kind,
                image_dims,
                keep_manual_override,
            );
            selected_after = Some(layer_idx);
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤーマスク種類変更".to_string());
            self.local_adjust_mask_edit_target = match mask_kind {
                MaskKind::Raster => LocalAdjustMaskEditTarget::Base,
                _ => LocalAdjustMaskEditTarget::None,
            };
            self.local_adjust_selected_shape = None;
            if matches!(mask_kind, MaskKind::Subject | MaskKind::Segmentation) {
                auto_generate_after_change = Some((layer_idx, mask_kind));
            }
            self.show_feedback_toast(format!("マスク種類を変更: {}", mask_kind.label()));
        }
        if let Some((layer_idx, enabled)) = set_enabled {
            if let Some(layer) = layers.get_mut(layer_idx) {
                layer.enabled = enabled;
                changed = true;
                undo_summary.get_or_insert_with(|| "補正レイヤー切替".to_string());
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
            undo_summary.get_or_insert_with(|| "補正レイヤー編集".to_string());
        }
        if let Some((from, to)) = move_layer
            && from < layers.len()
            && to < layers.len()
            && from != to
        {
            let layer = layers.remove(from);
            layers.insert(to, layer);
            selected_after = Some(to);
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤー並べ替え".to_string());
            self.show_feedback_toast("補正レイヤーを並べ替えました".to_string());
        }
        if let Some(layer_idx) = duplicate_layer
            && let Some(layer) = layers.get(layer_idx).cloned()
        {
            let insert_at = layer_idx + 1;
            layers.insert(insert_at, layer);
            selected_after = Some(insert_at);
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤー複製".to_string());
            self.show_feedback_toast("補正レイヤーを複製しました".to_string());
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
                    undo_summary.get_or_insert_with(|| "補正レイヤー効果ペースト".to_string());
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
                undo_summary.get_or_insert_with(|| "補正レイヤー効果リセット".to_string());
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
            undo_summary.get_or_insert_with(|| "補正レイヤー削除".to_string());
            self.show_feedback_toast("補正レイヤーを削除".to_string());
        }
        if clear_layers {
            layers.clear();
            changed = true;
            undo_summary.get_or_insert_with(|| "補正レイヤー全削除".to_string());
            self.show_feedback_toast("補正レイヤーをすべて削除".to_string());
        }
        if changed {
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before_layers,
                layers,
                undo_summary.unwrap_or_else(|| "補正レイヤー編集".to_string()),
            );
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
        if let Some((layer_idx, mask_kind)) = auto_generate_after_change {
            match mask_kind {
                MaskKind::Subject => {
                    self.start_local_adjust_subject_segmentation(fs_idx, layer_idx)
                }
                MaskKind::Segmentation => self.start_local_adjust_region_segmentation(
                    fs_idx,
                    layer_idx,
                    LocalAdjustRegionSegmentationScope::Full,
                ),
                _ => {}
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
        if let Some(layer_idx) = effect_requests.generate_subject_mask {
            self.start_local_adjust_subject_segmentation(fs_idx, layer_idx);
        }
        if let Some((layer_idx, scope)) = effect_requests.generate_region_mask {
            self.start_local_adjust_region_segmentation(fs_idx, layer_idx, scope);
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
                let before_layers = layers.clone();
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
                self.set_local_adjust_layers_for_idx_with_undo(
                    pending.fs_idx,
                    before_layers,
                    layers,
                    "補正レイヤーLUT読み込み".to_string(),
                );
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

    fn local_adjust_subject_mask_candidate(
        &self,
        fs_idx: usize,
    ) -> Option<local_adjust_core::RasterMask> {
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let layers = self.local_adjust_page_layers.get(&fs_idx)?;
        local_adjust_subject_mask_candidate_from_layers(layers, image_dims)
    }

    fn start_local_adjust_subject_segmentation(&mut self, fs_idx: usize, layer_idx: usize) {
        if self.local_adjust_segmentation_pending.is_some() {
            self.show_feedback_toast("マスク生成中です".to_string());
            return;
        }
        if !self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .is_some_and(|layer| matches!(layer.mask, local_adjust_core::LocalMask::Subject(_)))
        {
            self.show_feedback_toast("被写体マスクのレイヤーを選択してください".to_string());
            return;
        }
        let Some(runtime) = self.ai_runtime.clone() else {
            self.show_feedback_toast("AI ランタイムが初期化されていません".to_string());
            return;
        };
        let Some(model_path) = self
            .ai_model_manager
            .model_path(crate::ai::ModelKind::SubjectU2Netp)
        else {
            self.show_feedback_toast("U²-Netp モデルが見つかりません".to_string());
            return;
        };
        let Some(source) = self.current_local_adjust_source_pixels(fs_idx) else {
            self.show_feedback_toast("被写体マスク生成用の画像を取得できません".to_string());
            return;
        };
        let generation = self
            .local_adjust_generation
            .get(&fs_idx)
            .copied()
            .unwrap_or(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-subject-segmentation".to_string())
            .spawn(move || {
                let result = run_local_adjust_u2netp_segmentation(runtime, model_path, source)
                    .map(LocalAdjustGeneratedMask::Subject);
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.local_adjust_segmentation_pending =
                    Some(crate::app::LocalAdjustSegmentationPending {
                        fs_idx,
                        layer_idx,
                        generation,
                        rx,
                    });
                self.show_feedback_toast("被写体マスク生成中...".to_string());
            }
            Err(err) => {
                self.show_feedback_toast(format!("被写体マスク worker 起動失敗: {err}"));
            }
        }
    }

    fn start_local_adjust_region_segmentation(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        scope: LocalAdjustRegionSegmentationScope,
    ) {
        if self.local_adjust_segmentation_pending.is_some() {
            self.show_feedback_toast("マスク生成中です".to_string());
            return;
        }
        if !self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .is_some_and(|layer| {
                matches!(layer.mask, local_adjust_core::LocalMask::Segmentation(_))
            })
        {
            self.show_feedback_toast("領域分割マスクのレイヤーを選択してください".to_string());
            return;
        }
        let Some(source) = self.current_local_adjust_source_pixels(fs_idx) else {
            self.show_feedback_toast("領域分割用の画像を取得できません".to_string());
            return;
        };
        let subject = if scope.requires_subject() {
            self.local_adjust_subject_mask_candidate(fs_idx)
        } else {
            None
        };
        if scope.requires_subject() && subject.is_none() {
            self.show_feedback_toast("利用できる被写体マスクがありません".to_string());
            return;
        }
        let color_tolerance = self.local_adjust_region_color_tolerance;
        let min_area = self.local_adjust_region_min_area.max(1);
        let edge_threshold = self.local_adjust_boundary_edge_threshold.clamp(0.0, 255.0);
        let ink_threshold = self.local_adjust_boundary_ink_threshold.clamp(0.0, 255.0);
        let gap_px = self.local_adjust_boundary_gap_px.round().clamp(0.0, 8.0) as usize;
        let generation = self
            .local_adjust_generation
            .get(&fs_idx)
            .copied()
            .unwrap_or(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-region-segmentation".to_string())
            .spawn(move || {
                let result = build_local_adjust_region_segmentation(
                    source.as_ref(),
                    subject.as_ref(),
                    scope,
                    color_tolerance,
                    min_area,
                    edge_threshold,
                    ink_threshold,
                    gap_px,
                )
                .map(LocalAdjustGeneratedMask::Regions);
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.local_adjust_segmentation_pending =
                    Some(crate::app::LocalAdjustSegmentationPending {
                        fs_idx,
                        layer_idx,
                        generation,
                        rx,
                    });
                self.show_feedback_toast(scope.pending_label().to_string());
            }
            Err(err) => {
                self.show_feedback_toast(format!("領域分割 worker 起動失敗: {err}"));
            }
        }
    }

    pub(crate) fn poll_local_adjust_segmentation(&mut self, ctx: &egui::Context) {
        let recv_result = {
            let Some(pending) = self.local_adjust_segmentation_pending.as_ref() else {
                return;
            };
            pending.rx.try_recv()
        };
        match recv_result {
            Ok(Ok(generated)) => {
                let Some(pending) = self.local_adjust_segmentation_pending.take() else {
                    return;
                };
                let current_generation = self
                    .local_adjust_generation
                    .get(&pending.fs_idx)
                    .copied()
                    .unwrap_or(0);
                if pending.generation != current_generation {
                    self.show_feedback_toast(
                        "マスク生成結果を破棄しました。レイヤーが変更されています".to_string(),
                    );
                    return;
                }
                let mut layers = self
                    .local_adjust_page_layers
                    .get(&pending.fs_idx)
                    .cloned()
                    .unwrap_or_default();
                let before_layers = layers.clone();
                let Some(layer) = layers.get_mut(pending.layer_idx) else {
                    self.show_feedback_toast(
                        "マスク生成結果を破棄しました。対象レイヤーがありません".to_string(),
                    );
                    return;
                };
                let status = match (&mut layer.mask, generated) {
                    (
                        local_adjust_core::LocalMask::Subject(slot),
                        LocalAdjustGeneratedMask::Subject(mask),
                    ) => {
                        let foreground = mask.alpha.iter().filter(|&&alpha| alpha >= 0.5).count();
                        let total = mask.alpha.len().max(1);
                        *slot = local_adjust_core::SubjectMask::from_raster(mask);
                        format!(
                            "被写体マスク生成完了: 前景 {:.1}%",
                            foreground as f32 / total as f32 * 100.0
                        )
                    }
                    (
                        local_adjust_core::LocalMask::Segmentation(slot),
                        LocalAdjustGeneratedMask::Regions(mask),
                    ) => {
                        let label_count = mask.label_count();
                        *slot = mask;
                        format!("領域分割完了: {label_count} 領域")
                    }
                    _ => {
                        self.show_feedback_toast(
                            "マスク生成結果を破棄しました。対象レイヤーが変更されています"
                                .to_string(),
                        );
                        return;
                    }
                };
                self.local_adjust_selected_layers
                    .insert(pending.fs_idx, pending.layer_idx);
                self.set_local_adjust_layers_for_idx_with_undo(
                    pending.fs_idx,
                    before_layers,
                    layers,
                    "補正レイヤーマスク生成".to_string(),
                );
                self.show_feedback_toast(status);
                ctx.request_repaint();
            }
            Ok(Err(err)) => {
                self.local_adjust_segmentation_pending = None;
                self.show_feedback_toast(format!("マスク生成失敗: {err}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.local_adjust_segmentation_pending = None;
                self.show_feedback_toast("マスク生成 worker が停止しました".to_string());
            }
        }
    }

    fn draw_local_adjust_add_layer_dialog(
        &mut self,
        ctx: &egui::Context,
        image_dims: (usize, usize),
        add_layer_mask: &mut Option<MaskKind>,
    ) {
        if !self.local_adjust_add_layer_dialog_open {
            return;
        }
        let mut open = self.local_adjust_add_layer_dialog_open;
        let mut add_requested = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 245))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        with_local_adjust_dark_window_style(ctx, || {
            egui::Window::new("補正レイヤーを追加")
                .order(egui::Order::Debug)
                .frame(dialog_frame)
                .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                .collapsible(false)
                .resizable(true)
                .default_width(500.0)
                .default_height(390.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    *ui.visuals_mut() = egui::Visuals::dark();
                    ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                    ui.label(
                    egui::RichText::new(
                        "使いたいマスク種類を選んでください。クリックするとレイヤーを追加します。",
                    )
                    .size(11.0)
                    .color(egui::Color32::from_gray(180)),
                );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for group in MASK_GROUPS {
                                ui.label(
                                    egui::RichText::new(group.title)
                                        .size(13.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                    for &kind in group.kinds {
                                        let response = ui
                                            .add_sized(
                                                LOCAL_ADJUST_MASK_PICKER_BUTTON_SIZE,
                                                egui::Button::new(
                                                    egui::RichText::new(kind.label())
                                                        .size(12.0)
                                                        .strong(),
                                                )
                                                .wrap(),
                                            )
                                            .on_hover_text(kind.description());
                                        if response.clicked() {
                                            add_requested = Some(kind);
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                            }
                            let (w, h) = image_dims;
                            ui.label(
                                egui::RichText::new(format!(
                                    "対象画像: {} x {}",
                                    w.max(1),
                                    h.max(1)
                                ))
                                .size(10.0)
                                .color(egui::Color32::from_gray(160)),
                            );
                        });
                });
        });
        if let Some(kind) = add_requested {
            *add_layer_mask = Some(kind);
            open = false;
        }
        if self.dialog_escape_pressed(ctx) {
            open = false;
        }
        self.local_adjust_add_layer_dialog_open = open;
    }

    fn draw_local_adjust_change_mask_dialog(
        &mut self,
        ctx: &egui::Context,
        layers: &[local_adjust_core::LocalAdjustmentLayer],
        selected_layer: usize,
        change_layer_mask: &mut Option<(usize, MaskKind, bool)>,
    ) {
        if !self.local_adjust_change_mask_dialog_open {
            return;
        }
        let Some(layer) = layers.get(selected_layer) else {
            self.local_adjust_change_mask_dialog_open = false;
            return;
        };
        let current_kind = MaskKind::from_mask(&layer.mask);
        let mut open = self.local_adjust_change_mask_dialog_open;
        let mut keep_manual_override = self.local_adjust_change_mask_keep_manual_override;
        let mut selected_kind = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 245))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        with_local_adjust_dark_window_style(ctx, || {
            egui::Window::new("マスク種類変更")
                .order(egui::Order::Debug)
                .frame(dialog_frame)
                .default_pos(ctx.content_rect().min + egui::vec2(70.0, 54.0))
                .collapsible(false)
                .resizable(true)
                .default_width(500.0)
                .default_height(420.0)
                .open(&mut open)
                .show(ctx, |ui| {
                    *ui.visuals_mut() = egui::Visuals::dark();
                    ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                    ui.checkbox(&mut keep_manual_override, "追加/削除マスクを維持");
                    ui.label(
                        egui::RichText::new(
                            "加工内容は残したまま、選択中レイヤーのベースマスクだけを変更します。",
                        )
                        .size(11.0)
                        .color(egui::Color32::from_gray(180)),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for group in MASK_GROUPS {
                                ui.label(
                                    egui::RichText::new(group.title)
                                        .size(13.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                    for &kind in group.kinds {
                                        let is_current = kind == current_kind;
                                        let response = ui
                                            .add_enabled_ui(!is_current, |ui| {
                                                ui.add_sized(
                                                    LOCAL_ADJUST_MASK_PICKER_BUTTON_SIZE,
                                                    egui::Button::new(
                                                        egui::RichText::new(kind.label())
                                                            .size(12.0)
                                                            .strong(),
                                                    )
                                                    .wrap(),
                                                )
                                            })
                                            .inner
                                            .on_hover_text(if is_current {
                                                "現在のマスク種類です。"
                                            } else {
                                                kind.description()
                                            });
                                        if response.clicked() {
                                            selected_kind = Some(kind);
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                            }
                        });
                });
        });
        if let Some(kind) = selected_kind {
            *change_layer_mask = Some((selected_layer, kind, keep_manual_override));
            open = false;
        }
        if self.dialog_escape_pressed(ctx) {
            open = false;
        }
        self.local_adjust_change_mask_keep_manual_override = keep_manual_override;
        self.local_adjust_change_mask_dialog_open = open;
    }

    fn draw_local_adjust_effect_picker_dialog(
        &mut self,
        ctx: &egui::Context,
        selected_layer: usize,
        effect_query: &mut String,
        set_layer_effect: &mut Option<(usize, EffectKind)>,
    ) {
        if !self.local_adjust_effect_picker_dialog_open {
            return;
        }
        let mut open = self.local_adjust_effect_picker_dialog_open;
        let mut picked_effect = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 245))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        with_local_adjust_dark_window_style(ctx, || {
            egui::Window::new("加工内容を選択")
                .order(egui::Order::Debug)
                .frame(dialog_frame)
                .default_pos(ctx.content_rect().min + egui::vec2(80.0, 64.0))
                .collapsible(false)
                .resizable(true)
                .default_size(egui::vec2(860.0, 560.0))
                .min_size(egui::vec2(560.0, 360.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    *ui.visuals_mut() = egui::Visuals::dark();
                    ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            egui::vec2((ui.available_width() - 32.0).max(120.0), 24.0),
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
                    ui.separator();
                    let query = effect_query.trim();
                    egui::ScrollArea::vertical()
                        .max_height(440.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let button_width = effect_picker_button_width(ui.available_width());
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
                                ui.label(
                                    egui::RichText::new(group.title)
                                        .size(13.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                    for kind in matched {
                                        let response = ui
                                            .add_sized(
                                                egui::vec2(
                                                    button_width,
                                                    LOCAL_ADJUST_EFFECT_PICKER_BUTTON_H,
                                                ),
                                                egui::Button::new(
                                                    egui::RichText::new(kind.picker_label())
                                                        .size(12.0),
                                                )
                                                .wrap(),
                                            )
                                            .on_hover_text(kind.description());
                                        if response.clicked() {
                                            picked_effect = Some(kind);
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                            }
                            if !any_effect {
                                ui.label(
                                    egui::RichText::new("該当する効果がありません")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(160)),
                                );
                            }
                        });
                });
        });
        if let Some(kind) = picked_effect {
            *set_layer_effect = Some((selected_layer, kind));
            open = false;
        }
        if self.dialog_escape_pressed(ctx) {
            open = false;
        }
        self.local_adjust_effect_picker_dialog_open = open;
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
        let local_adjust_source_pixels = self.current_local_adjust_source_pixels(fs_idx);
        let mut effect_query = self.local_adjust_effect_query.clone();
        let effect_clipboard_available = self.local_adjust_effect_clipboard.is_some();
        let selective_color_pick_active = self.local_adjust_selective_color_pick_active;
        let rgb_pick_active = self.local_adjust_rgb_pick_active;
        let effect_position_handles_visible = self.local_adjust_effect_position_handles_visible;
        let segmentation_pending = self.local_adjust_segmentation_pending.is_some();
        let active_local_adjust_layers = self.has_active_local_adjust_layers(fs_idx);
        let current_local_adjust_key = self.current_local_adjust_key(fs_idx);
        let local_adjust_render_pending = self
            .local_adjust_pending
            .get(&fs_idx)
            .is_some_and(|pending| pending.key == current_local_adjust_key);
        let local_adjust_render_ready = self.current_local_adjust_texture(fs_idx).is_some();
        let local_adjust_status = if segmentation_pending {
            "マスク生成中...".to_string()
        } else if self.local_adjust_lut_pending.is_some() {
            "LUT読み込み中...".to_string()
        } else if layers.is_empty() {
            "補正レイヤーなし".to_string()
        } else {
            format!("補正レイヤー {}件", layers.len())
        };
        let (local_adjust_indicator, local_adjust_indicator_color) = if segmentation_pending
            || self.local_adjust_lut_pending.is_some()
            || local_adjust_render_pending
        {
            ("処理中", egui::Color32::from_rgb(255, 210, 90))
        } else if active_local_adjust_layers && local_adjust_render_ready {
            ("反映済", egui::Color32::from_rgb(120, 220, 150))
        } else if active_local_adjust_layers {
            ("待機中", egui::Color32::from_rgb(255, 210, 90))
        } else {
            ("待機", egui::Color32::from_gray(165))
        };
        let subject_model_available = self
            .ai_model_manager
            .model_path(crate::ai::ModelKind::SubjectU2Netp)
            .is_some();
        let subject_mask_available =
            local_adjust_subject_mask_candidate_from_layers(&layers, image_dims).is_some();
        let previous_mask_edit_target = self.local_adjust_mask_edit_target;
        let mut mask_edit_target = self.local_adjust_mask_edit_target;
        let mut mask_brush_radius = self.local_adjust_mask_brush_radius;
        let mut mask_paint_add = self.local_adjust_mask_paint_add;
        let previous_mask_tool = self.local_adjust_mask_tool;
        let mut mask_tool = self.local_adjust_mask_tool;
        let mut mask_line_width = self.local_adjust_mask_line_width;
        let mut mask_gap_fill_distance = self.local_adjust_mask_gap_fill_distance;
        let mut boundary_edge_threshold = self.local_adjust_boundary_edge_threshold;
        let mut boundary_ink_threshold = self.local_adjust_boundary_ink_threshold;
        let mut boundary_gap_px = self.local_adjust_boundary_gap_px;
        let mut edge_brush_tolerance = self.local_adjust_edge_brush_tolerance;
        let mut edge_brush_include_boundary = self.local_adjust_edge_brush_include_boundary;
        let mut region_color_tolerance = self.local_adjust_region_color_tolerance;
        let mut region_min_area = self.local_adjust_region_min_area;

        let panel_rect = self.local_adjust_panel_rect(full_rect);
        let panel_pos = panel_rect.min;
        let sink_rect = panel_rect.expand2(egui::vec2(4.0, 8.0));
        let tool_panel_rect = self.local_adjust_tool_panel_rect(full_rect);
        let tool_panel_pos = tool_panel_rect.min;
        let tool_sink_rect = tool_panel_rect.expand2(egui::vec2(4.0, 8.0));

        let mut close_clicked = false;
        let mut add_layer_mask: Option<MaskKind> = None;
        let mut change_layer_mask: Option<(usize, MaskKind, bool)> = None;
        let mut set_layer_effect: Option<(usize, EffectKind)> = None;
        let mut select_layer: Option<usize> = None;
        let mut set_enabled: Option<(usize, bool)> = None;
        let mut update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)> = None;
        let mut move_layer: Option<(usize, usize)> = None;
        let mut duplicate_layer: Option<usize> = None;
        let mut delete_layer: Option<usize> = None;
        let clear_layers = false;
        let mut effect_requests = LocalEffectPanelRequests::default();
        let mut bitmap_mask_op = None;

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
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(LOCAL_ADJUST_PANEL_W - 112.0, 18.0),
                                egui::Label::new(
                                    egui::RichText::new(&local_adjust_status)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(190)),
                                )
                                .wrap(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("● {local_adjust_indicator}"))
                                            .size(11.0)
                                            .strong()
                                            .color(local_adjust_indicator_color),
                                    );
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

                                        draw_local_adjust_left_panel(
                                            ui,
                                            LOCAL_ADJUST_PANEL_W,
                                            &layers,
                                            selected_layer,
                                            image_dims,
                                            local_adjust_source_pixels.as_deref(),
                                            &mut self.local_adjust_add_layer_dialog_open,
                                            &mut self.local_adjust_effect_picker_dialog_open,
                                            &mut select_layer,
                                            &mut set_enabled,
                                            &mut update_layer,
                                            &mut move_layer,
                                            &mut duplicate_layer,
                                            &mut delete_layer,
                                            &mut mask_edit_target,
                                            &mut mask_paint_add,
                                            &mut mask_tool,
                                            &mut bitmap_mask_op,
                                            &mut self.local_adjust_show_source,
                                            &mut self.local_adjust_show_mask,
                                            &mut self.local_adjust_mask_color_preset,
                                            &mut self.local_adjust_preview_to_selected_layer,
                                        );
                                    });
                            },
                        );
                    });
            });

        egui::Area::new(egui::Id::new("local_adjust_tool_panel"))
            .fixed_pos(tool_panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    tool_sink_rect,
                    egui::Id::new("local_adjust_tool_panel_click_sink"),
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
                        ui.set_min_width(LOCAL_ADJUST_TOOL_PANEL_W);
                        ui.set_max_width(LOCAL_ADJUST_TOOL_PANEL_W);
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                        let body_height =
                            (tool_panel_rect.height() - 14.0).max(LOCAL_ADJUST_PANEL_MIN_BODY_H);
                        ui.set_min_height(body_height);
                        ui.set_max_height(body_height);
                        egui::ScrollArea::vertical()
                            .max_height(body_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(LOCAL_ADJUST_TOOL_PANEL_W);
                                ui.set_max_width(LOCAL_ADJUST_TOOL_PANEL_W);
                                if let Some(layer) = layers.get(selected_layer) {
                                    draw_selected_local_adjust_layer_editor(
                                        ui,
                                        selected_layer,
                                        layer,
                                        image_dims,
                                        &mut update_layer,
                                        effect_clipboard_available,
                                        selective_color_pick_active,
                                        rgb_pick_active,
                                        effect_position_handles_visible,
                                        segmentation_pending,
                                        subject_model_available,
                                        subject_mask_available,
                                        &mut mask_edit_target,
                                        &mut mask_brush_radius,
                                        &mut mask_paint_add,
                                        &mut mask_tool,
                                        &mut mask_line_width,
                                        &mut mask_gap_fill_distance,
                                        &mut boundary_edge_threshold,
                                        &mut boundary_ink_threshold,
                                        &mut boundary_gap_px,
                                        &mut edge_brush_tolerance,
                                        &mut edge_brush_include_boundary,
                                        &mut region_color_tolerance,
                                        &mut region_min_area,
                                        &mut self.local_adjust_change_mask_dialog_open,
                                        &mut effect_requests,
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new(
                                            "左側のレイヤーパネルからレイヤーを追加してください。",
                                        )
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(180)),
                                    );
                                }
                            });
                    });
            });

        self.draw_local_adjust_add_layer_dialog(ctx, image_dims, &mut add_layer_mask);
        self.draw_local_adjust_change_mask_dialog(
            ctx,
            &layers,
            selected_layer,
            &mut change_layer_mask,
        );
        self.draw_local_adjust_effect_picker_dialog(
            ctx,
            selected_layer,
            &mut effect_query,
            &mut set_layer_effect,
        );

        if close_clicked {
            self.local_adjust_mode = false;
            self.local_adjust_add_layer_dialog_open = false;
            self.local_adjust_change_mask_dialog_open = false;
            self.local_adjust_effect_picker_dialog_open = false;
        }
        self.local_adjust_effect_query = effect_query;
        self.local_adjust_mask_edit_target = mask_edit_target;
        self.local_adjust_mask_brush_radius = mask_brush_radius.max(1.0);
        self.local_adjust_mask_paint_add = mask_paint_add;
        if previous_mask_edit_target != mask_edit_target {
            self.local_adjust_selected_shape = None;
            self.local_adjust_shape_drag = None;
            self.local_adjust_shape_drag_before_layers = None;
        }
        if previous_mask_tool != mask_tool {
            self.local_adjust_mask_lasso_points.clear();
            self.local_adjust_mask_shape_drag_start = None;
            self.local_adjust_mask_shape_drag_end = None;
            self.local_adjust_mask_brush_stroke = None;
            self.local_adjust_shape_drag = None;
            self.local_adjust_shape_drag_before_layers = None;
        }
        self.local_adjust_mask_tool = mask_tool;
        self.local_adjust_mask_line_width = mask_line_width.max(1.0);
        self.local_adjust_mask_gap_fill_distance = mask_gap_fill_distance.clamp(1.0, 64.0);
        self.local_adjust_boundary_edge_threshold = boundary_edge_threshold.clamp(0.0, 255.0);
        self.local_adjust_boundary_ink_threshold = boundary_ink_threshold.clamp(0.0, 255.0);
        self.local_adjust_boundary_gap_px = boundary_gap_px.clamp(0.0, 8.0);
        self.local_adjust_edge_brush_tolerance = edge_brush_tolerance.clamp(0.0, 255.0);
        self.local_adjust_edge_brush_include_boundary = edge_brush_include_boundary;
        self.local_adjust_region_color_tolerance = region_color_tolerance.clamp(4.0, 120.0);
        self.local_adjust_region_min_area = region_min_area.clamp(1, 2048);
        if let Some(op) = bitmap_mask_op {
            self.apply_local_adjust_bitmap_mask_op(
                fs_idx,
                selected_layer,
                self.local_adjust_mask_edit_target,
                op,
            );
        }
        self.apply_local_adjust_panel_actions(
            fs_idx,
            add_layer_mask,
            change_layer_mask,
            set_layer_effect,
            select_layer,
            set_enabled,
            update_layer,
            move_layer,
            duplicate_layer,
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
        // 補正レイヤーは独立左パネル、エクスポートは Ctrl+E と同じダイアログで開く。
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
            "エクスポート / 切り取り",
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
            self.local_adjust_show_source = false;
            self.local_adjust_show_mask = true;
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
            self.adjustment_mode = false;
            self.export_crop_mode = false;
            let ctx = child.ctx().clone();
            self.open_export_dialog_for_current(&ctx, fs_idx);
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
