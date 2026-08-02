//! フルスクリーン画像補正パネル（左側オーバーレイ表示）。
//!
//! マウスを画面端（左・上・右）に寄せるとオーバーレイとして表示される。
//! スコープは標準設定 + ページ個別の 2 つ。
//!
//! - パネルでスライダーを操作すると、その瞬間に「現在のページ個別パラメータ」が更新される
//!   (ページ個別設定が自動生成される)
//! - アクションは選択中スコープに合わせた操作行:
//!     - お気に入り標準では「共通の標準からコピー」
//!     - ページ個別では現在の実効値を場所の標準へ反映
//!     - 現在一覧の全画像を、場所の標準または現在ページの実効値へ揃える
//!     - ページ個別では現在ページの個別設定を解除する
//! - スライダー下の「すべてリセット」は選択中スコープの全補正値を初期値へ戻す
//! - 保存スロット 10 個: クリック or Ctrl+数字で現在のページに適用

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui;

use crate::adjustment::{AdjustParams, AutoMode, PostFilter, PresetSlot};
use crate::app::{
    AdjustScopeSelection, AdjustSpreadTarget, AdjustmentStandardDragSession,
    AdjustmentStandardScope, App, FavoriteDefaultClearConfirm, LocalAdjustEdgePreviewCache,
    LocalAdjustEdgePreviewKey, LocalAdjustGeneratedMask, LocalAdjustMaskColorPreset,
    LocalAdjustMaskEditTarget, LocalAdjustMaskShapeDrag, LocalAdjustMaskTool,
    LocalAdjustRegionSegmentationScope,
};
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::keymap::KeyAction;
use crate::local_adjust_catalog::{
    EFFECT_GROUPS, EffectKind, effect_picker_button_width, effect_picker_matches_query,
};
use crate::local_adjust_effect_ui::draw_effect_params;
use crate::ui_fullscreen::SpreadPair;

const HEADER_H: f32 = 64.0;
const TAB_ROW_H: f32 = 24.0;
const SECTION_FONT: f32 = 12.0;

/// 左パネルの幅
// ヘッダーには 画像補正 / 表示トリム / ブックマーク のタブと、画像補正タブ用のツール入口アイコン
// (消しゴム/補正/隠蔽/切り取り/テキスト/エクスポート) を置く。
pub const LEFT_PANEL_WIDTH: f32 = 292.0;
/// 左パネルの下端をウィンドウ下端から少し浮かせる余白。
pub const LEFT_PANEL_BOTTOM_MARGIN: f32 = 20.0;
/// 補正本文の左余白。画面端に文字が張り付かないようにする。
const BODY_PAD_LEFT: f32 = 10.0;
/// 補正本文の右余白。スクロールバーと保存スロットボタンの干渉を避ける。
const BODY_PAD_RIGHT: f32 = 10.0;
/// ScrollArea の縦バーが重なる分として、本文 widget 幅から差し引く余白。
const BODY_SCROLLBAR_RESERVE: f32 = 14.0;
/// 258px の本文幅でアプリフォント + 暗色 radio を実測し、7文字は252px、
/// 8文字は265pxだったため、1行に収まる最大の7文字を採る。
const ADJUST_SCOPE_FAVORITE_NAME_MAX_CHARS: usize = 7;
const ADJUST_ALIGN_ALL_STANDARD_LABEL: &str = "このフォルダの全画像を標準に揃える";
// 指定文言は実測 268px で本文幅 258px を超えるため、対象を保ったまま短縮する。
const ADJUST_ALIGN_ALL_PAGE_LABEL: &str = "フォルダ全画像をこのページに揃える";
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
/// 被写体マット (BiRefNet) の正方形入力サイズ。BiRefNet は 1024² 学習。
const LOCAL_ADJUST_SUBJECT_INPUT_SIZE: usize = 1024;
const LOCAL_ADJUST_REGION_SEGMENT_MAX_LABELS: usize = 2048;
const LOCAL_ADJUST_MASK_PREVIEW_BASE_ALPHA: f32 = 155.0;
const LOCAL_ADJUST_MASK_PREVIEW_EDIT_ALPHA: u8 = 225;
const LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS: f32 = 2048.0;
const LOCAL_ADJUST_REGION_BOUNDARY_ANIM_INTERVAL_MS: u64 = 160;
const LOCAL_ADJUST_EDGE_OVERLAY_REPAINT_MS: u64 = 90;
const LOCAL_ADJUST_EDGE_PREVIEW_MAX_SIDE: usize = 640;
pub(crate) const LOCAL_ADJUST_NUDGE_PIXELS: f32 = 1.0;
pub(crate) const LOCAL_ADJUST_NUDGE_PIXELS_FAST: f32 = 10.0;
pub(crate) const LOCAL_ADJUST_ROTATE_DEG_STEP: f32 = 0.1;
pub(crate) const LOCAL_ADJUST_ROTATE_DEG_STEP_FAST: f32 = 1.0;

fn initial_adjust_scope_selection(has_page_override: bool) -> AdjustScopeSelection {
    if has_page_override {
        AdjustScopeSelection::Page
    } else {
        AdjustScopeSelection::Standard
    }
}

fn effective_adjust_scope_selection(
    has_page_override: bool,
    stored: AdjustScopeSelection,
) -> AdjustScopeSelection {
    if has_page_override {
        AdjustScopeSelection::Page
    } else {
        stored
    }
}

fn should_save_settings(settings_changed: bool, is_dragging: bool) -> bool {
    settings_changed && !is_dragging
}

fn adjust_scope_standard_label(active_favorite_name: Option<&str>) -> String {
    adjust_scope_standard_label_with_limit(
        active_favorite_name,
        ADJUST_SCOPE_FAVORITE_NAME_MAX_CHARS,
    )
}

fn adjust_scope_standard_label_with_limit(
    active_favorite_name: Option<&str>,
    max_chars: usize,
) -> String {
    match active_favorite_name {
        Some(name) => format!(
            "標準（お気に入り「{}」）",
            crate::ui_helpers::truncate_name(name, max_chars)
        ),
        None => "標準（共通）".to_string(),
    }
}

fn favorite_default_clear_needs_confirmation(
    current: &AdjustParams,
    fallback: &AdjustParams,
) -> bool {
    current != fallback
}

#[cfg(test)]
mod adjust_scope_selector_tests {
    use super::*;

    #[test]
    fn adjust_scope_top_label_uses_active_favorite_or_common() {
        assert_eq!(adjust_scope_standard_label(None), "標準（共通）");
        assert_eq!(
            adjust_scope_standard_label(Some("スキャン画像集")),
            "標準（お気に入り「スキャン画像集」）"
        );
    }

    #[test]
    fn favorite_default_clear_confirmation_depends_on_fallback_difference() {
        let current = AdjustParams::default();
        let same = current.clone();
        let mut different = current.clone();
        different.brightness = 1.0;

        assert!(!favorite_default_clear_needs_confirmation(&current, &same));
        assert!(favorite_default_clear_needs_confirmation(
            &current, &different
        ));
    }

    #[test]
    fn adjust_scope_initial_selection_follows_page_override() {
        assert_eq!(
            initial_adjust_scope_selection(false),
            AdjustScopeSelection::Standard
        );
        assert_eq!(
            initial_adjust_scope_selection(true),
            AdjustScopeSelection::Page
        );
    }

    #[test]
    fn effective_adjust_scope_selection_forces_page_when_override_exists() {
        assert_eq!(
            effective_adjust_scope_selection(false, AdjustScopeSelection::Standard),
            AdjustScopeSelection::Standard
        );
        assert_eq!(
            effective_adjust_scope_selection(false, AdjustScopeSelection::Page),
            AdjustScopeSelection::Page
        );
        assert_eq!(
            effective_adjust_scope_selection(true, AdjustScopeSelection::Standard),
            AdjustScopeSelection::Page
        );
        assert_eq!(
            effective_adjust_scope_selection(true, AdjustScopeSelection::Page),
            AdjustScopeSelection::Page
        );
    }

    #[test]
    fn settings_save_waits_only_while_currently_dragging() {
        assert!(!should_save_settings(false, false));
        assert!(!should_save_settings(false, true));
        assert!(!should_save_settings(true, true));
        assert!(should_save_settings(true, false));
    }

    #[test]
    fn adjust_scope_top_radio_uses_maximum_limit_for_258px_body() {
        use egui_kittest::Harness;
        use std::sync::{Arc, Mutex};

        let measured = Arc::new(Mutex::new(Vec::new()));
        let measured_in_ui = measured.clone();
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(400.0, 72.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let mut measured = measured_in_ui.lock().unwrap();
                        measured.clear();
                        for max_chars in [7, 8] {
                            let label = adjust_scope_standard_label_with_limit(
                                Some("あいうえおかきくけこ"),
                                max_chars,
                            );
                            let response = ui.radio(false, label);
                            measured.push(response.rect.size());
                        }
                    });
            });
        harness.run();

        let sizes = measured.lock().unwrap();
        assert_eq!(sizes.len(), 2, "both radios were rendered");
        assert!(sizes[0].x <= 258.0, "7-char radio was {}px", sizes[0].x);
        assert!(sizes[0].y <= 24.0, "7-char radio was {}px high", sizes[0].y);
        assert!(sizes[1].x > 258.0, "8-char radio was only {}px", sizes[1].x);
    }

    #[test]
    fn adjust_scope_action_labels_fit_258px_body() {
        use egui_kittest::Harness;
        use std::sync::{Arc, Mutex};

        let measured = Arc::new(Mutex::new(Vec::new()));
        let measured_in_ui = measured.clone();
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(400.0, 180.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let mut measured = measured_in_ui.lock().unwrap();
                        measured.clear();
                        for label in [
                            "共通の標準からコピー",
                            "現在の設定値を標準に反映",
                            ADJUST_ALIGN_ALL_STANDARD_LABEL,
                            ADJUST_ALIGN_ALL_PAGE_LABEL,
                            "個別設定を解除 [Q]",
                        ] {
                            measured.push((label, ui.button(label).rect.size()));
                        }
                    });
            });
        harness.run();

        let sizes = measured.lock().unwrap();
        assert_eq!(sizes.len(), 5, "all action labels were rendered");
        for (label, size) in sizes.iter() {
            assert!(size.x <= 258.0, "{label} was {}px wide", size.x);
            assert!(size.y <= 24.0, "{label} was {}px high", size.y);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAdjustBitmapMaskOp {
    Expand,
    Shrink,
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
    start_repair_point_pick: Option<crate::local_adjust_effect_ui::RepairPointPickTarget>,
    cancel_repair_point_pick: bool,
    set_effect_position_handles_visible: Option<bool>,
    generate_subject_mask: Option<usize>,
    generate_region_mask: Option<(usize, LocalAdjustRegionSegmentationScope)>,
    /// 被写体マットモデル (編集用追加パック) が未導入のとき、ダウンロード導線を開く要求 (spec §9)。
    request_editing_addon_download: bool,
    /// このフレームでマスク (種類・形状・パラメータ等) を操作した → マスク表示を ON にする。
    /// ラボの reveal_mask_preview 相当。brush / 生成は App 側で別途検出する。
    mask_touched: bool,
    /// このフレームで効果パラメータを操作した → マスク表示を OFF にする。
    /// ラボの「効果 response.changed → hide_mask_preview」相当。
    effect_touched: bool,
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
            Self::Subject => "AI で被写体マスクを生成して使います。",
            Self::Segmentation => "色と境界から領域候補を生成してクリック選択します。",
        }
    }
}

#[cfg(test)]
mod local_adjust_segmentation_tests {
    use super::*;

    fn test_transform(rect: egui::Rect, size: (usize, usize)) -> DisplayedImageTransform {
        DisplayedImageTransform::resolve(
            crate::displayed_image_transform::DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: rect,
                source_size: egui::vec2(size.0 as f32, size.1 as f32),
                texture_size: egui::vec2(size.0 as f32, size.1 as f32),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: crate::settings::FullscreenFitMode::Page,
                fit_scale_limits:
                    crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
                placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
                    zoom_pan: None,
                },
            },
        )
        .unwrap()
    }

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
    fn local_adjust_rect_handles_use_shared_vector_layout() {
        let shape = local_adjust_core::MaskShape::Rect {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            half_w: 30.0,
            half_h: 20.0,
            rotation_rad: 0.0,
        };
        let vector_shape = local_adjust_shape_to_vector_shape(shape);
        let layout = crate::vector_edit::compute_handle_layout(&vector_shape, 1.0);
        assert_eq!(
            crate::vector_edit::hit_test(&layout, (130.0, 120.0), 1.0),
            Some(crate::vector_edit::HoverTarget::Corner { idx: 2 })
        );
        assert_eq!(
            crate::vector_edit::hit_test(&layout, (100.0, 80.0), 1.0),
            Some(crate::vector_edit::HoverTarget::EdgeMidpoint { idx: 0 })
        );
        assert_eq!(
            crate::vector_edit::hit_test(&layout, (100.0, 52.0), 1.0),
            Some(crate::vector_edit::HoverTarget::RotateHandle)
        );
    }

    #[test]
    fn local_adjust_ellipse_handles_use_shared_vector_layout() {
        let shape = local_adjust_core::MaskShape::Ellipse {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            rx: 50.0,
            ry: 24.0,
            rotation_rad: 0.0,
        };
        let vector_shape = local_adjust_shape_to_vector_shape(shape);
        let layout = crate::vector_edit::compute_handle_layout(&vector_shape, 1.0);
        assert_eq!(
            crate::vector_edit::hit_test(&layout, (150.0, 100.0), 1.0),
            Some(crate::vector_edit::HoverTarget::EdgeMidpoint { idx: 1 })
        );
        assert_eq!(
            crate::vector_edit::hit_test(&layout, (100.0, 48.0), 1.0),
            Some(crate::vector_edit::HoverTarget::RotateHandle)
        );
    }

    #[test]
    fn change_mask_kind_resets_inversion() {
        // 被写体マスクで「背景を選択」= mask_inverted=true の状態から手動マスクへ種類変更
        // したとき、反転が引き継がれて全面マスクになる退行を防ぐ (mask_inverted がリセットされる)。
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "subject",
            local_adjust_core::LocalMask::Subject(local_adjust_core::SubjectMask::empty(4, 4)),
            local_adjust_core::LocalEffect::None,
        );
        layer.mask_inverted = true;
        replace_local_adjust_layer_base_mask(&mut layer, MaskKind::Raster, (4, 4), false);
        assert!(
            !layer.mask_inverted,
            "マスク種類変更で反転フラグがリセットされること"
        );
        assert!(matches!(
            layer.mask,
            local_adjust_core::LocalMask::RasterVector(_)
        ));
    }

    #[test]
    fn selected_local_adjust_line_thickness_updates_shape() {
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "line",
            local_adjust_core::LocalMask::RasterVector(local_adjust_core::RasterVectorMask {
                width: 200,
                height: 100,
                alpha: vec![0.0; 200 * 100],
                shapes: vec![local_adjust_core::MaskShape::Line {
                    op: local_adjust_core::ShapeOp::Add,
                    kind: local_adjust_core::LineKind::Diagonal,
                    p0: [10.0, 10.0],
                    p1: [120.0, 10.0],
                    thickness: 8.0,
                }],
            }),
            local_adjust_core::LocalEffect::None,
        );
        assert_eq!(
            selected_local_adjust_line_thickness(&layer, LocalAdjustMaskEditTarget::Base, Some(0),),
            Some(8.0)
        );
        assert!(set_selected_local_adjust_line_thickness(
            &mut layer,
            LocalAdjustMaskEditTarget::Base,
            Some(0),
            18.0,
            (200, 100),
        ));
        assert_eq!(
            selected_local_adjust_line_thickness(&layer, LocalAdjustMaskEditTarget::Base, Some(0),),
            Some(18.0)
        );

        let non_line = local_adjust_core::MaskShape::Ellipse {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            rx: 40.0,
            ry: 22.0,
            rotation_rad: 0.0,
        };
        if let local_adjust_core::LocalMask::RasterVector(mask) = &mut layer.mask {
            mask.shapes.push(non_line);
        }
        assert!(!set_selected_local_adjust_line_thickness(
            &mut layer,
            LocalAdjustMaskEditTarget::Base,
            Some(1),
            20.0,
            (200, 100),
        ));
    }

    #[test]
    fn line_shape_preview_uses_square_end_caps() {
        let layer = local_adjust_core::LocalAdjustmentLayer::new(
            "line",
            local_adjust_core::LocalMask::RasterVector(local_adjust_core::RasterVectorMask {
                width: 48,
                height: 24,
                alpha: vec![0.0; 48 * 24],
                shapes: vec![local_adjust_core::MaskShape::Line {
                    op: local_adjust_core::ShapeOp::Add,
                    kind: local_adjust_core::LineKind::Horizontal,
                    p0: [12.0, 12.0],
                    p1: [36.0, 12.0],
                    thickness: 10.0,
                }],
            }),
            local_adjust_core::LocalEffect::None,
        );

        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 48, 24, 8, 12),
            0.0
        );
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 48, 24, 12, 12),
            1.0
        );
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 48, 24, 35, 12),
            1.0
        );
        assert_eq!(
            local_adjust_mask_preview_alpha(&layer, None, 48, 24, 39, 12),
            0.0
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
    fn full_mask_with_large_subtract_preview_completes_quickly() {
        let width = 3840;
        let height = 2160;
        let mut subtract = local_adjust_core::RasterVectorMask::empty(width, height);
        subtract.alpha[width * height - 1] = 1.0;
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "full",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        );
        layer.manual_override.subtract = Some(subtract);

        let started = std::time::Instant::now();
        let image = build_local_adjust_mask_preview_image(
            &layer,
            None,
            (width, height),
            [64, 64],
            0.0,
            LocalAdjustMaskColorPreset::PinkCyan.colors(),
            None,
        );
        let elapsed = started.elapsed();

        assert_eq!(image.size, [64, 64]);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "Full + large subtract preview should be O(preview pixels + mask pixels), elapsed={elapsed:?}"
        );
    }

    #[test]
    fn mask_preview_overlay_at_2048_completes_in_30ms() {
        let width = 2048;
        let height = 1152;
        let mut labels = vec![0_u32; width * height];
        for y in 0..height {
            for x in 0..width {
                labels[y * width + x] = ((x / 64 + (y / 64) * 32) % 1024 + 1) as u32;
            }
        }
        let layer = local_adjust_core::LocalAdjustmentLayer::new(
            "segmentation-preview",
            local_adjust_core::LocalMask::Segmentation(local_adjust_core::RegionMask {
                width,
                height,
                labels,
                selected: vec![false; 1025],
            }),
            local_adjust_core::LocalEffect::None,
        );

        let started = std::time::Instant::now();
        let image = build_local_adjust_mask_preview_image(
            &layer,
            None,
            (width, height),
            [2048, 1152],
            0.25,
            LocalAdjustMaskColorPreset::PinkCyan.colors(),
            None,
        );
        let elapsed = started.elapsed();

        assert_eq!(image.size, [2048, 1152]);
        if cfg!(debug_assertions) {
            assert!(
                elapsed < std::time::Duration::from_millis(750),
                "debug 2048 preview overlay generation regressed badly, elapsed={elapsed:?}"
            );
        } else {
            assert!(
                elapsed < std::time::Duration::from_millis(30),
                "2048 preview overlay generation should stay under 30ms, elapsed={elapsed:?}"
            );
        }
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
        let transform = test_transform(rect, (100, 100));
        assert_eq!(
            local_adjust_gradient_handle_hit(&layer, egui::pos2(80.0, 70.0), &transform,),
            Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd)
        );
        assert_eq!(
            local_adjust_gradient_handle_hit(&layer, egui::pos2(20.0, 30.0), &transform,),
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
        let transform = test_transform(rect, (200, 100));
        assert_eq!(
            local_adjust_gradient_handle_hit(&layer, egui::pos2(160.0, 50.0), &transform,),
            Some(crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX)
        );
        assert_eq!(
            local_adjust_gradient_handle_hit(&layer, egui::pos2(100.0, 50.0), &transform,),
            Some(crate::app::LocalAdjustCanvasDragKind::RadialGradientCenter)
        );
    }

    #[test]
    fn effect_linear_gradient_handle_hit_uses_angle_points() {
        let effect =
            local_adjust_core::LocalEffect::ColorOverlay(local_adjust_core::ColorOverlayParams {
                shape: local_adjust_core::ColorOverlayShape::Linear,
                angle_degrees: 0.0,
                linear_points_enabled: false,
                ..Default::default()
            });
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        assert_eq!(
            local_adjust_effect_gradient_handle_hit(&effect, egui::pos2(100.0, 50.0), rect),
            Some(crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd)
        );
        assert_eq!(
            local_adjust_effect_gradient_handle_hit(&effect, egui::pos2(0.0, 50.0), rect),
            Some(crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart)
        );
    }

    #[test]
    fn effect_linear_gradient_drag_enables_custom_points() {
        let mut effect =
            local_adjust_core::LocalEffect::ColorFill(local_adjust_core::ColorFillParams {
                shape: local_adjust_core::ColorOverlayShape::Linear,
                ..Default::default()
            });
        assert!(apply_local_adjust_effect_gradient_handle_drag(
            &mut effect,
            crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd,
            [0.75, 0.25],
        ));
        let local_adjust_core::LocalEffect::ColorFill(params) = effect else {
            panic!("expected color fill effect");
        };
        assert!(params.linear_points_enabled);
        assert_eq!(params.linear_end, [0.75, 0.25]);
    }

    #[test]
    fn effect_radial_gradient_radius_drag_updates_radius() {
        let mut effect =
            local_adjust_core::LocalEffect::ColorOverlay(local_adjust_core::ColorOverlayParams {
                shape: local_adjust_core::ColorOverlayShape::Radial,
                center: [0.5, 0.5],
                radius: 0.2,
                ..Default::default()
            });
        assert!(apply_local_adjust_effect_gradient_handle_drag(
            &mut effect,
            crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius,
            [0.8, 0.5],
        ));
        let local_adjust_core::LocalEffect::ColorOverlay(params) = effect else {
            panic!("expected color overlay effect");
        };
        assert!((params.radius - 0.3).abs() < 1e-5);
    }

    #[test]
    fn effect_radial_gradient_handle_hit_allows_large_radius() {
        let effect =
            local_adjust_core::LocalEffect::ColorOverlay(local_adjust_core::ColorOverlayParams {
                shape: local_adjust_core::ColorOverlayShape::Radial,
                center: [0.5, 0.5],
                radius: 0.9,
                ..Default::default()
            });
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0));
        assert_eq!(
            local_adjust_effect_gradient_handle_hit(&effect, egui::pos2(140.0, 50.0), rect),
            Some(crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius)
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
    fn subject_mask_refinement_defaults_to_disabled() {
        assert!(!local_adjust_core::SubjectMaskRefinement::default().enabled);
    }

    #[test]
    fn subject_refinement_preset_values_match_lab() {
        let cases = [
            (
                LocalAdjustSubjectRefinementPreset::Standard,
                "標準",
                0.52,
                0,
                1,
            ),
            (
                LocalAdjustSubjectRefinementPreset::Firm,
                "硬め",
                0.58,
                -1,
                0,
            ),
            (
                LocalAdjustSubjectRefinementPreset::Soft,
                "柔らかめ",
                0.45,
                0,
                2,
            ),
        ];
        for (preset, label, threshold, expand_px, feather_px) in cases {
            let refinement = preset.refinement();
            assert_eq!(preset.label(), label);
            assert!(refinement.enabled);
            assert_eq!(refinement.threshold, threshold);
            assert_eq!(refinement.expand_px, expand_px);
            assert_eq!(refinement.feather_px, feather_px);
        }
    }

    #[test]
    fn subject_refinement_binarizes_soft_alpha() {
        let mask = local_adjust_core::RasterMask {
            width: 4,
            height: 1,
            alpha: vec![0.20, 0.49, 0.52, 0.90],
        };
        let refined = local_adjust_subject_refined_alpha(&mask, 0.5, 0, 0);
        assert_eq!(refined, vec![0.0, 0.0, 1.0, 1.0]);
        let stats = local_adjust_subject_mask_stats(&local_adjust_core::SubjectMask::from_raster(
            local_adjust_core::RasterMask {
                width: 4,
                height: 1,
                alpha: refined,
            },
        ));
        assert_eq!(stats.foreground_percent, 50.0);
        assert_eq!(stats.soft_percent, 0.0);
    }

    #[test]
    fn subject_refinement_regenerates_from_cached_source() {
        let source = local_adjust_core::RasterMask {
            width: 4,
            height: 1,
            alpha: vec![0.20, 0.45, 0.55, 0.90],
        };
        let mut subject = local_adjust_core::SubjectMask::from_raster(source);
        subject.alpha =
            local_adjust_subject_refined_alpha(&subject.source_raster_mask(), 0.60, 0, 0);
        assert_eq!(subject.alpha, vec![0.0, 0.0, 0.0, 1.0]);

        subject.alpha =
            local_adjust_subject_refined_alpha(&subject.source_raster_mask(), 0.40, 0, 0);
        assert_eq!(subject.alpha, vec![0.0, 1.0, 1.0, 1.0]);
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

    #[test]
    fn edge_preview_size_caps_long_side() {
        assert_eq!(local_adjust_edge_preview_size(4000, 2000), [640, 320]);
        assert_eq!(local_adjust_edge_preview_size(0, 0), [1, 1]);
    }

    #[test]
    fn polygon_candidate_snaps_to_nearby_edge_when_ctrl_is_held() {
        let width = 20;
        let height = 10;
        let mut pixels = vec![egui::Color32::from_rgb(245, 245, 245); width * height];
        for y in 0..height {
            pixels[y * width + 10] = egui::Color32::from_rgb(8, 8, 8);
        }
        let source = egui::ColorImage::new([width, height], pixels);
        let raw = [7.5, 5.5];
        let snapped = local_adjust_snap_point_to_edge(&source, raw, 5.0, 24.0, 28.0, 0);

        assert!(snapped[0] > raw[0] + 1.0);
        assert!(snapped[0] <= 11.5);
    }

    #[test]
    fn polygon_candidate_does_not_snap_without_ctrl() {
        let source =
            egui::ColorImage::new([4, 4], vec![egui::Color32::from_rgb(240, 240, 240); 16]);
        let norm = [0.4, 0.5];
        let (candidate, raw, snapping) = local_adjust_polygon_candidate_point(
            norm,
            (4, 4),
            Some(&source),
            1.0,
            false,
            16.0,
            24.0,
            28.0,
            0,
        );

        assert_eq!(candidate, raw);
        assert!(!snapping);
    }

    /// A-2 regression hardening: MAX_TEXELS は 2048 を下回ってはならない。
    ///
    /// 経緯: ef750308 で 768 → 2048 に上げた。理由は領域境界アニメーションが
    /// 動的色で点滅する際、小さな texture (768 上限) では境界線が「点」として
    /// 見えてしまい、アニメーション効果が崩壊するため。値が小さく戻されると
    /// 同じ事象が再発するので、定数の下限を **コンパイル時に固定** しておく
    /// (= 値を 2048 未満に戻したコミットはビルドが通らない)。
    /// (パフォーマンス上限は `mask_preview_overlay_at_2048_completes_in_30ms` 側で別途確認)
    #[test]
    fn mask_preview_max_texels_constant_stays_at_or_above_2048() {
        const {
            assert!(
                super::LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS >= 2048.0,
                "MAX_TEXELS < 2048 だと領域境界アニメーションが点になる (A-2 退行)"
            )
        };
    }

    /// A-1 関連: shape hit-test (`local_adjust_shape_contains`) も polygon 経由で
    /// **square cap** を保つ。distance-based に戻されると click-to-select の判定が
    /// 丸い半円範囲に広がり、ユーザーが「線の端より外側」をクリックしても誤選択する。
    #[test]
    fn shape_contains_line_uses_polygon_square_caps() {
        let line = local_adjust_core::MaskShape::Line {
            op: local_adjust_core::ShapeOp::Add,
            kind: local_adjust_core::LineKind::Horizontal,
            p0: [10.0, 10.0],
            p1: [50.0, 10.0],
            thickness: 8.0,
        };
        // 線分の真ん中: 含まれる
        assert!(super::local_adjust_shape_contains(line, [30.0, 10.0]));
        // 端点 p0 ぴったり: 含まれる
        assert!(super::local_adjust_shape_contains(line, [10.0, 10.0]));
        // p0 のすぐ手前 (X = 6.0, 端点から 4px 手前) → rounded cap なら含まれていた
        // square cap なら含まれない (= square cap の polygon の外側)
        assert!(
            !super::local_adjust_shape_contains(line, [6.0, 10.0]),
            "click 4px before p0 must miss with square cap (rounded would hit)"
        );
        // p1 のすぐ先 (X = 54.0)
        assert!(
            !super::local_adjust_shape_contains(line, [54.0, 10.0]),
            "click 4px past p1 must miss with square cap"
        );
        // 横方向に thickness/2 + 1 = 5 はずれた位置 (Y = 15) → 範囲外
        assert!(!super::local_adjust_shape_contains(line, [30.0, 15.5]));
    }

    /// Rect の hit-test は rotation を反転して local 座標で half_w / half_h 判定する。
    #[test]
    fn shape_contains_rect_respects_rotation() {
        let rect = local_adjust_core::MaskShape::Rect {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 100.0],
            half_w: 20.0,
            half_h: 10.0,
            rotation_rad: std::f32::consts::FRAC_PI_2, // 90°
        };
        // 90° 回転後: half_w が y 方向に伸びる
        // 中心: 含まれる
        assert!(super::local_adjust_shape_contains(rect, [100.0, 100.0]));
        // 元の half_w 方向 (= 回転後は短辺方向 ±10): 中心 ±10 で含まれる
        assert!(super::local_adjust_shape_contains(rect, [110.0, 100.0]));
        assert!(!super::local_adjust_shape_contains(rect, [115.0, 100.0]));
        // 元の half_h 方向 (= 回転後は長辺方向 ±20)
        assert!(super::local_adjust_shape_contains(rect, [100.0, 115.0]));
        assert!(!super::local_adjust_shape_contains(rect, [100.0, 125.0]));
    }

    /// Ellipse の hit-test は rotation を反転して楕円方程式で内外判定する。
    #[test]
    fn shape_contains_ellipse_uses_quadratic_form() {
        let ell = local_adjust_core::MaskShape::Ellipse {
            op: local_adjust_core::ShapeOp::Add,
            center: [200.0, 200.0],
            rx: 30.0,
            ry: 15.0,
            rotation_rad: 0.0,
        };
        assert!(super::local_adjust_shape_contains(ell, [200.0, 200.0])); // 中心
        assert!(super::local_adjust_shape_contains(ell, [229.0, 200.0])); // ほぼ rx
        assert!(!super::local_adjust_shape_contains(ell, [232.0, 200.0])); // rx 超過
        assert!(super::local_adjust_shape_contains(ell, [200.0, 214.0])); // ほぼ ry
        assert!(!super::local_adjust_shape_contains(ell, [200.0, 217.0])); // ry 超過
        // 軸外: ((dx/rx)^2 + (dy/ry)^2) で判定。中心から (20, 10) は 0.44+0.44 < 1 → 内
        assert!(super::local_adjust_shape_contains(ell, [220.0, 210.0]));
        // (30, 10) は (1.0 + 0.44) > 1 → 外
        assert!(!super::local_adjust_shape_contains(ell, [230.0, 210.0]));
    }

    #[test]
    fn shape_keyboard_transform_translates_and_snaps_rotation() {
        let rect = local_adjust_core::MaskShape::Rect {
            op: local_adjust_core::ShapeOp::Add,
            center: [100.0, 80.0],
            half_w: 20.0,
            half_h: 10.0,
            rotation_rad: 0.0,
        };
        let moved = local_adjust_translate_shape(rect, 3.0, -4.0);
        let local_adjust_core::MaskShape::Rect { center, .. } = moved else {
            panic!("expected rect");
        };
        assert_eq!(center, [103.0, 76.0]);

        let rotated = local_adjust_rotate_shape(rect, 7.0_f32.to_radians(), true);
        let local_adjust_core::MaskShape::Rect { rotation_rad, .. } = rotated else {
            panic!("expected rect");
        };
        assert!((rotation_rad - 0.0).abs() < 1e-6);
    }

    /// A-2 関連: アニメーション色は時間経過で十分に変化する。
    /// 既存 `region_boundary_color_animates_over_time` は t=0 と t=1 の 2 点比較
    /// だが、間隔の刻みが粗すぎて「ほぼ静止」していても通ってしまう。
    /// ここでは 0.1 秒刻みで色相変化量を測り、1 秒で 360° hue サイクルの
    /// 1/4 (= 90°) 以上回ることを assertion する。
    #[test]
    fn region_boundary_color_completes_meaningful_hue_rotation_in_one_second() {
        // hue 計算式 (src/ui_adjustment_panel.rs::local_adjust_region_boundary_color):
        //   hue = (time_sec * 130.0 + (label * 47 % 360)) mod 360
        // 1 秒で 130 度進む → 90 度以上のしきい値は安全マージン
        let label = 0_u32; // label のオフセット影響を切り捨て
        let mut max_diff: f32 = 0.0;
        for i in 1..=10 {
            let t = i as f32 * 0.1;
            let c0 = super::local_adjust_region_boundary_color(label, 0.0);
            let ct = super::local_adjust_region_boundary_color(label, t);
            let diff = ((c0.r() as i32 - ct.r() as i32).abs()
                + (c0.g() as i32 - ct.g() as i32).abs()
                + (c0.b() as i32 - ct.b() as i32).abs()) as f32;
            max_diff = max_diff.max(diff);
        }
        assert!(
            max_diff >= 100.0,
            "色相が 1 秒で 100/765 以上動かない → アニメーション周波数が遅すぎる (A-2 退行)"
        );
    }

    // ========================================================================
    // P8 / Phase 5: 補正レイヤーパネル UI スナップショット (egui_kittest)
    // ========================================================================
    //
    // 背景: `tests/ui_snapshot.rs` は lib crate (= `mimageviewer::*` の pub API のみ
    // アクセス可) として動くので、`pub(crate) fn draw_local_adjust_*` には届かない。
    // 代わりにこの bin test mod 内で egui_kittest の Harness を直接叩く。
    //
    // 目的: 補正レイヤーパネルの描画ロジック (= 大半が `pub(crate) fn draw_*`) に
    // 意図しない見た目変更 (配色 / レイアウト / ボタンサイズ) が入ったとき、
    // PNG 差分として検知する。
    //
    // 実行:
    //   cargo test --bin mimageviewer-core local_adjust_panel_snapshot
    // 更新:
    //   UPDATE_SNAPSHOTS=1 cargo test --bin mimageviewer-core local_adjust_panel_snapshot
    //
    // スナップショット保存先: `tests/snapshots/<name>.png` (cargo の test snapshot
    // 規約に従う; bin test と integration test で同じディレクトリを共有する)。
    //
    // ⚠ skeleton 段階 (Phase 5、smoke 1 件): 最小限の構成で「panel render が panic
    // しないこと」を確かめる。シナリオ拡充 (layer 追加 / mask preview / bypass mode)
    // は Phase 5+ で `local_adjust_panel_snapshot_*` を増やす。

    /// P8-3: 補正レイヤーパネルの「空 (= layer が 1 つも無い)」状態を snapshot 化する
    /// smoke テスト。`draw_local_adjust_layer_list` が空 layers に対して
    /// 「+ 補正レイヤー」ボタン + ガイドテキスト + 「選択レイヤーまでプレビュー」
    /// チェックボックス だけを描くことを符号化。
    ///
    /// 退行検知:
    /// - 空状態のガイドテキスト文言・配色変更
    /// - 「+ 補正レイヤー」ボタンのレイアウト
    /// - 「選択レイヤーまでプレビュー」チェックボックスのラベル文字列
    #[test]
    fn local_adjust_panel_snapshot_empty_layer_list() {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(280.0, 200.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    let layers: Vec<local_adjust_core::LocalAdjustmentLayer> = vec![];
                    let mut add_layer_dialog_open = false;
                    let mut select_layer: Option<usize> = None;
                    let mut set_enabled: Option<(usize, bool)> = None;
                    let mut update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)> =
                        None;
                    let mut move_layer: Option<(usize, usize)> = None;
                    let mut duplicate_layer: Option<usize> = None;
                    let mut delete_layer: Option<usize> = None;
                    let mut preview_to_selected_layer = false;
                    super::draw_local_adjust_layer_list(
                        ui,
                        260.0,
                        &layers,
                        0,
                        (1920, 1080),
                        None,
                        &mut add_layer_dialog_open,
                        &mut select_layer,
                        &mut set_enabled,
                        &mut update_layer,
                        &mut move_layer,
                        &mut duplicate_layer,
                        &mut delete_layer,
                        &mut preview_to_selected_layer,
                    );
                });
            });

        harness.run();
        harness.snapshot("local_adjust_panel_empty_layer_list");
    }

    #[test]
    fn colorize_preset_slots_snapshot_single_row() {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(280.0, 90.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut params = crate::colorize::ColorizeParams::default();
                    let mut slots = crate::colorize::ColorizePresetSlots {
                        slots: std::array::from_fn(|_| Some(params.clone())),
                    };
                    super::draw_colorize_preset_slots(ui, &mut params, &mut slots);
                });
            });

        harness.run();
        harness.snapshot("colorize_preset_slots_single_row");
    }

    #[test]
    fn colorize_gradient_preview_snapshot_custom_palette() {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(280.0, 92.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut params = crate::colorize::ColorizeParams::default();
                    params.palette = crate::colorize::ColorizePalette::Custom;
                    params.luminance_weight = 35;
                    super::draw_colorize_gradient_preview(ui, &params);
                });
            });

        harness.run();
        harness.snapshot("colorize_gradient_preview_custom_palette");
    }

    /// Snapshot シナリオ用の helper: 名前 / マスク / effect だけを受けて
    /// `LocalAdjustmentLayer` を作る (= 各 snapshot test の fixture を 1 行で書ける)。
    fn snapshot_layer(
        name: &str,
        mask: local_adjust_core::LocalMask,
        effect: local_adjust_core::LocalEffect,
        enabled: bool,
    ) -> local_adjust_core::LocalAdjustmentLayer {
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(name, mask, effect);
        layer.enabled = enabled;
        layer
    }

    /// Snapshot シナリオ共通ハーネス。`build_panel` closure 内で
    /// `draw_local_adjust_layer_list` を呼んで snapshot を撮る。
    /// closure に渡る引数は `(ui, panel_w)`、それ以外の `&mut` は内部 default。
    fn snapshot_panel_with_layers(
        name: &str,
        size: egui::Vec2,
        layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
        selected_layer: usize,
        preview_to_selected_layer: bool,
    ) {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let layers = std::sync::Arc::new(layers);
        let layers_for_render = std::sync::Arc::clone(&layers);
        let mut harness = Harness::builder().with_size(size).build(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut add_layer_dialog_open = false;
                let mut select_layer: Option<usize> = None;
                let mut set_enabled: Option<(usize, bool)> = None;
                let mut update_layer: Option<(usize, local_adjust_core::LocalAdjustmentLayer)> =
                    None;
                let mut move_layer: Option<(usize, usize)> = None;
                let mut duplicate_layer: Option<usize> = None;
                let mut delete_layer: Option<usize> = None;
                let mut preview_flag = preview_to_selected_layer;
                super::draw_local_adjust_layer_list(
                    ui,
                    260.0,
                    &layers_for_render,
                    selected_layer,
                    (1920, 1080),
                    None,
                    &mut add_layer_dialog_open,
                    &mut select_layer,
                    &mut set_enabled,
                    &mut update_layer,
                    &mut move_layer,
                    &mut duplicate_layer,
                    &mut delete_layer,
                    &mut preview_flag,
                );
            });
        });

        harness.run();
        harness.snapshot(name);
    }

    /// P8-4a: 単一 Full マスク layer 1 つ (有効状態)。
    /// 退行検知: layer 行のチェックボックス / 「前」「後」ボタンの hit 領域 / カラム
    /// レイアウトが崩れたら PNG 差分で気付く。
    #[test]
    fn local_adjust_panel_snapshot_one_full_layer() {
        snapshot_panel_with_layers(
            "local_adjust_panel_one_full_layer",
            egui::vec2(280.0, 260.0),
            vec![snapshot_layer(
                "Layer 1",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
                true,
            )],
            0,
            false,
        );
    }

    /// P8-4b: 2 layers、2 番目を選択した状態。選択ハイライトの色 / 強調表示が
    /// 切り替わっていることを符号化。
    #[test]
    fn local_adjust_panel_snapshot_two_layers_second_selected() {
        snapshot_panel_with_layers(
            "local_adjust_panel_two_layers_second_selected",
            egui::vec2(280.0, 320.0),
            vec![
                snapshot_layer(
                    "Layer 1",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                    true,
                ),
                snapshot_layer(
                    "Layer 2",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                    true,
                ),
            ],
            1,
            false,
        );
    }

    /// P8-4c: `preview_to_selected_layer` トグル ON 状態。「表示中: 1〜N / 総数」
    /// のラベルが見えることを符号化 (= L キーで preview に入ったときの UI 退行検知)。
    #[test]
    fn local_adjust_panel_snapshot_preview_to_selected_layer_active() {
        snapshot_panel_with_layers(
            "local_adjust_panel_preview_to_selected_layer_active",
            egui::vec2(280.0, 320.0),
            vec![
                snapshot_layer(
                    "Layer 1",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                    true,
                ),
                snapshot_layer(
                    "Layer 2",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                    true,
                ),
                snapshot_layer(
                    "Layer 3",
                    local_adjust_core::LocalMask::Full,
                    local_adjust_core::LocalEffect::None,
                    true,
                ),
            ],
            1,
            true,
        );
    }

    /// P8-4d: 無効化された layer。チェックボックスが unchecked、行の見た目が
    /// 通常 / 選択中とは違う薄い表現になることを符号化。
    #[test]
    fn local_adjust_panel_snapshot_layer_disabled() {
        snapshot_panel_with_layers(
            "local_adjust_panel_layer_disabled",
            egui::vec2(280.0, 260.0),
            vec![snapshot_layer(
                "Layer 1",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
                false,
            )],
            0,
            false,
        );
    }

    fn snapshot_repair_effect_panel(
        name: &str,
        params: local_adjust_core::RepairParams,
        mask: local_adjust_core::LocalMask,
    ) {
        use egui_kittest::Harness;

        let mut fonts_ready = false;
        let mut layer = snapshot_layer(
            "修復／塗り",
            mask,
            local_adjust_core::LocalEffect::Repair(params),
            true,
        );
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 590.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_min_width(300.0);
                    ui.set_max_width(300.0);
                    crate::local_adjust_effect_ui::draw_effect_params(
                        ui,
                        &mut layer,
                        (1920, 1080),
                        false,
                        None,
                        None,
                        false,
                        true,
                    );
                });
            });

        harness.run();
        harness.snapshot(name);
    }

    /// v2.5.0: 周囲修復の探索品質、テクスチャ / 色なじみ調整と
    /// 全体マスクの警告が右パネルに収まることを固定する。
    #[test]
    fn local_adjust_panel_snapshot_repair_effect() {
        snapshot_repair_effect_panel(
            "local_adjust_panel_repair_effect",
            local_adjust_core::RepairParams::default(),
            local_adjust_core::LocalMask::Full,
        );
    }

    /// v2.5.0: 固定オフセットクローンの2点指定と座標表示を固定する。
    #[test]
    fn local_adjust_panel_snapshot_repair_clone_effect() {
        snapshot_repair_effect_panel(
            "local_adjust_panel_repair_clone_effect",
            local_adjust_core::RepairParams {
                mode: local_adjust_core::RepairMode::Clone,
                clone_source_uv: Some([0.25, 0.35]),
                clone_destination_uv: Some([0.65, 0.55]),
                ..Default::default()
            },
            local_adjust_core::LocalMask::Raster(local_adjust_core::RasterMask {
                width: 1,
                height: 1,
                alpha: vec![1.0],
            }),
        );
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
            local_adjust_core::LocalMask::RasterVector(mask) => {
                mask.resize_to(width, height);
                Some(mask)
            }
            _ => None,
        },
        LocalAdjustMaskEditTarget::OverrideAdd | LocalAdjustMaskEditTarget::OverrideSubtract => {
            let slot = local_mask_override_slot_mut(layer, target)?;
            if let Some(mask) = slot.as_mut() {
                mask.resize_to(width, height);
            } else if create {
                *slot = Some(local_adjust_core::RasterVectorMask::empty(width, height));
            } else {
                return None;
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
        ui.label(egui::RichText::new("表示:").color(ui.visuals().text_color()));
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
            .weak(),
        );
    }
    if layers.is_empty() {
        ui.label(
            egui::RichText::new("補正レイヤーを追加してください。")
                .size(11.0)
                .weak(),
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
                                    egui::Label::new(egui::RichText::new("OFF").size(10.0).weak())
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
    ui.label(egui::RichText::new("加工内容:").color(ui.visuals().text_color()));
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
                    .color(ui.visuals().text_color()),
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
        .color(ui.visuals().text_color()),
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
                        .weak(),
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
            ui.label(egui::RichText::new(help).size(10.0).weak());
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
    ui.label(egui::RichText::new("描画 / 消去:").color(ui.visuals().text_color()));
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
    ui.label(egui::RichText::new("ビットマップ:").color(ui.visuals().text_color()));
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
    ui.label(egui::RichText::new("オブジェクト:").color(ui.visuals().text_color()));
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
    selected_line_thickness: Option<f32>,
    mask_brush_radius: &mut f32,
    mask_line_width: &mut f32,
    mask_gap_fill_distance: &mut f32,
    boundary_edge_threshold: &mut f32,
    boundary_ink_threshold: &mut f32,
    boundary_gap_px: &mut f32,
    edge_snap_radius: &mut f32,
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
            .weak(),
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
        | LocalAdjustMaskTool::HorizLine
        | LocalAdjustMaskTool::Select
            if selected_line_thickness.is_some()
                || matches!(
                    mask_tool,
                    LocalAdjustMaskTool::Line
                        | LocalAdjustMaskTool::VertLine
                        | LocalAdjustMaskTool::HorizLine
                ) =>
        {
            ui.add(egui::Slider::new(mask_line_width, 1.0..=160.0).text("線幅"));
            if selected_line_thickness.is_some() {
                ui.label(
                    egui::RichText::new("選択中の直線にも反映します。")
                        .size(10.0)
                        .weak(),
                );
            }
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
                .weak(),
            );
        }
        LocalAdjustMaskTool::GapFillBrush => {
            ui.add(egui::Slider::new(mask_gap_fill_distance, 1.0..=48.0).text("隙間幅"));
            ui.label(
                egui::RichText::new("左右または上下のマスクに挟まれた細い未塗り部分を補完します。")
                    .size(10.0)
                    .weak(),
            );
        }
        LocalAdjustMaskTool::Polygon => {
            ui.add(egui::Slider::new(boundary_edge_threshold, 0.0..=120.0).text("境界しきい値"));
            ui.add(egui::Slider::new(boundary_ink_threshold, 0.0..=120.0).text("線内部しきい値"));
            ui.add(egui::Slider::new(boundary_gap_px, 0.0..=4.0).text("境界ギャップ補完"));
            ui.add(egui::Slider::new(edge_snap_radius, 2.0..=64.0).text("吸着半径"));
            ui.label(
                egui::RichText::new(
                    "Ctrl中は候補点が近くの境界へ吸着します。右クリックまたは始点クリックで確定します。",
                )
                    .size(10.0)
                    .weak(),
            );
        }
        LocalAdjustMaskTool::Brush => {
            ui.label(
                egui::RichText::new("ドラッグした範囲をマスクに描画します。")
                    .size(10.0)
                    .weak(),
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
        .weak(),
    );
}

fn draw_selected_local_adjust_layer_editor(
    ui: &mut egui::Ui,
    layer_idx: usize,
    layer: &local_adjust_core::LocalAdjustmentLayer,
    image_dims: (usize, usize),
    selected_shape: Option<usize>,
    update_layer: &mut Option<(usize, local_adjust_core::LocalAdjustmentLayer)>,
    effect_clipboard_available: bool,
    selective_color_pick_active: bool,
    rgb_pick_active: Option<crate::local_adjust_effect_ui::RgbPickTarget>,
    repair_point_pick_active: Option<crate::local_adjust_effect_ui::RepairPointPickTarget>,
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
    edge_snap_radius: &mut f32,
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
    let selected_line_thickness = effective_local_mask_edit_target(&edited, *mask_edit_target)
        .and_then(|target| selected_local_adjust_line_thickness(&edited, target, selected_shape));
    if let Some(thickness) = selected_line_thickness {
        *mask_line_width = thickness.max(1.0);
    }

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Tool, |ui| {
        if manual_edit_controls_visible {
            let before_line_width = *mask_line_width;
            draw_local_tool_settings(
                ui,
                &edited,
                *mask_edit_target,
                *mask_tool,
                selected_line_thickness,
                mask_brush_radius,
                mask_line_width,
                mask_gap_fill_distance,
                boundary_edge_threshold,
                boundary_ink_threshold,
                boundary_gap_px,
                edge_snap_radius,
                edge_brush_tolerance,
                edge_brush_include_boundary,
            );
            if selected_line_thickness.is_some()
                && (*mask_line_width - before_line_width).abs() > f32::EPSILON
                && let Some(target) = effective_local_mask_edit_target(&edited, *mask_edit_target)
                && set_selected_local_adjust_line_thickness(
                    &mut edited,
                    target,
                    selected_shape,
                    *mask_line_width,
                    image_dims,
                )
            {
                changed = true;
            }
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
                .weak(),
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
                ui.label(egui::RichText::new(help).size(11.0).weak());
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
        if matches!(&edited.effect, local_adjust_core::LocalEffect::Repair(_)) {
            ui.label(
                egui::RichText::new("修復／塗りは常にマスク範囲だけへ適用します。")
                    .size(10.0)
                    .weak(),
            );
        } else {
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
        }
        changed |= ui
            .add(egui::Slider::new(&mut edited.mask_expand_px, -32.0..=32.0).text("拡張/縮小"))
            .changed();
        let is_repair = matches!(&edited.effect, local_adjust_core::LocalEffect::Repair(_));
        let feather = ui.add(
            egui::Slider::new(&mut edited.mask_feather_px, 0.0..=64.0).text(if is_repair {
                "境界なじませ"
            } else {
                "ぼかし境界"
            }),
        );
        changed |= feather.changed();
        if is_repair {
            feather.on_hover_text(
                "修復元の探索や生成テクスチャは変えず、最後の合成境界だけを内側へ滑らかになじませます。必要なら先に「拡張/縮小」で修復範囲を広げてください。",
            );
        }
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

    // 効果セクションより前に立った `changed` は、マスク種類 / 反転 / 不透明度 / 適用前後 /
    // 拡張縮小 / ぼかし / マスクエディタ (被写体・領域・筆設定) 由来 = マスク操作。
    // ラボの reveal_mask_preview 相当として「マスクを触った」フラグを立てる。
    if changed {
        effect_requests.mask_touched = true;
    }

    draw_local_adjust_panel_section(ui, LocalAdjustPanelSection::Effect, |ui| {
        let response = draw_effect_params(
            ui,
            &mut edited,
            image_dims,
            selective_color_pick_active,
            rgb_pick_active,
            repair_point_pick_active,
            effect_clipboard_available,
            effect_position_handles_visible,
        );
        if response.changed {
            changed = true;
            // ラボの「効果 response.changed → hide_mask_preview」相当。
            effect_requests.effect_touched = true;
        }
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
        if response.start_repair_point_pick.is_some() {
            effect_requests.start_repair_point_pick = response.start_repair_point_pick;
        }
        effect_requests.cancel_repair_point_pick |= response.cancel_repair_point_pick;
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
    transform: &DisplayedImageTransform,
    image_dims: (usize, usize),
) -> Option<(f32, egui::Rect)> {
    let (iw, ih) = (image_dims.0.max(1), image_dims.1.max(1));
    let scale = transform.screen_px_per_source_px(egui::vec2(iw as f32, ih as f32));
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some((scale, transform.full_image_rect))
}

fn local_adjust_screen_to_norm(
    screen: egui::Pos2,
    transform: &DisplayedImageTransform,
    require_inside: bool,
) -> Option<[f32; 2]> {
    if require_inside && !transform.contains_screen(screen) {
        return None;
    }
    let p = transform.screen_to_source_normalized(screen);
    Some([p.x.clamp(0.0, 1.0), p.y.clamp(0.0, 1.0)])
}

fn local_adjust_screen_to_norm_unclamped(
    screen: egui::Pos2,
    transform: &DisplayedImageTransform,
) -> Option<[f32; 2]> {
    let p = transform.screen_to_source_normalized(screen);
    Some([p.x, p.y])
}

fn local_adjust_norm_to_screen(
    norm: [f32; 2],
    transform: &DisplayedImageTransform,
) -> Option<egui::Pos2> {
    Some(transform.source_normalized_to_screen(egui::pos2(norm[0], norm[1])))
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
    sample_local_adjust_rgb_with_radius(app, fs_idx, norm, 0.0)
}

fn sample_local_adjust_rgb_with_radius(
    app: &App,
    fs_idx: usize,
    norm: [f32; 2],
    radius_px: f32,
) -> Option<[u8; 3]> {
    let pixels = app.current_local_adjust_source_pixels(fs_idx)?;
    let [w, h] = pixels.size;
    if w == 0 || h == 0 {
        return None;
    }
    let x = (norm[0].clamp(0.0, 1.0) * (w.saturating_sub(1)) as f32).round() as usize;
    let y = (norm[1].clamp(0.0, 1.0) * (h.saturating_sub(1)) as f32).round() as usize;
    let radius = radius_px.round().clamp(0.0, 64.0) as isize;
    if radius == 0 {
        let color = pixels.pixels[y.min(h - 1) * w + x.min(w - 1)];
        return Some([color.r(), color.g(), color.b()]);
    }
    let mut sum = [0.0_f64; 3];
    let mut weight_sum = 0.0_f64;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
            let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
            let color = pixels.pixels[sy * w + sx];
            let weight = color.a() as f64 / 255.0;
            if weight <= f64::EPSILON {
                continue;
            }
            sum[0] += color.r() as f64 * weight;
            sum[1] += color.g() as f64 * weight;
            sum[2] += color.b() as f64 * weight;
            weight_sum += weight;
        }
    }
    if weight_sum <= f64::EPSILON {
        return None;
    }
    Some([
        (sum[0] / weight_sum).round().clamp(0.0, 255.0) as u8,
        (sum[1] / weight_sum).round().clamp(0.0, 255.0) as u8,
        (sum[2] / weight_sum).round().clamp(0.0, 255.0) as u8,
    ])
}

fn local_adjust_subject_mask_has_content(mask: &local_adjust_core::SubjectMask) -> bool {
    mask.alpha.iter().any(|&alpha| alpha > 0.02)
        || mask
            .source_alpha
            .as_ref()
            .is_some_and(|alpha| alpha.iter().any(|&value| value > 0.02))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LocalAdjustSubjectMaskStats {
    foreground_percent: f32,
    soft_percent: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalAdjustSubjectRefinementPreset {
    Standard,
    Firm,
    Soft,
}

impl LocalAdjustSubjectRefinementPreset {
    const ALL: [Self; 3] = [Self::Standard, Self::Firm, Self::Soft];

    fn label(self) -> &'static str {
        match self {
            Self::Standard => "標準",
            Self::Firm => "硬め",
            Self::Soft => "柔らかめ",
        }
    }

    fn refinement(self) -> local_adjust_core::SubjectMaskRefinement {
        match self {
            Self::Standard => local_adjust_core::SubjectMaskRefinement {
                enabled: true,
                threshold: 0.52,
                expand_px: 0,
                feather_px: 1,
            },
            Self::Firm => local_adjust_core::SubjectMaskRefinement {
                enabled: true,
                threshold: 0.58,
                expand_px: -1,
                feather_px: 0,
            },
            Self::Soft => local_adjust_core::SubjectMaskRefinement {
                enabled: true,
                threshold: 0.45,
                expand_px: 0,
                feather_px: 2,
            },
        }
    }
}

fn local_adjust_subject_mask_stats(
    mask: &local_adjust_core::SubjectMask,
) -> LocalAdjustSubjectMaskStats {
    if mask.alpha.is_empty() {
        return LocalAdjustSubjectMaskStats {
            foreground_percent: 0.0,
            soft_percent: 0.0,
        };
    }
    let mut foreground = 0usize;
    let mut soft = 0usize;
    for &alpha in &mask.alpha {
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha >= 0.5 {
            foreground += 1;
        }
        if alpha > 0.02 && alpha < 0.98 {
            soft += 1;
        }
    }
    let total = mask.alpha.len() as f32;
    LocalAdjustSubjectMaskStats {
        foreground_percent: foreground as f32 * 100.0 / total,
        soft_percent: soft as f32 * 100.0 / total,
    }
}

fn local_adjust_subject_refined_alpha(
    mask: &local_adjust_core::RasterMask,
    threshold: f32,
    expand_px: i32,
    feather_px: usize,
) -> Vec<f32> {
    let threshold = threshold.clamp(0.0, 1.0);
    let mut alpha: Vec<f32> = mask
        .alpha
        .iter()
        .map(|&value| {
            if value.clamp(0.0, 1.0) >= threshold {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let steps = expand_px.unsigned_abs().min(16);
    for _ in 0..steps {
        alpha = local_adjust_morph_alpha_1px(&alpha, mask.width, mask.height, expand_px >= 0);
    }
    if feather_px > 0 {
        alpha = local_adjust_box_blur_alpha(&alpha, mask.width, mask.height, feather_px.min(16));
    }
    alpha
}

fn apply_local_adjust_subject_refinement(
    mask: &mut local_adjust_core::SubjectMask,
    refinement: local_adjust_core::SubjectMaskRefinement,
) {
    if mask.source_alpha.is_none() {
        mask.set_source_from_current();
    }
    let refinement = local_adjust_core::SubjectMaskRefinement {
        enabled: true,
        threshold: refinement.threshold,
        expand_px: refinement.expand_px,
        feather_px: refinement.feather_px.max(0),
    };
    let source = mask.source_raster_mask();
    mask.alpha = local_adjust_subject_refined_alpha(
        &source,
        refinement.threshold,
        refinement.expand_px,
        refinement.feather_px as usize,
    );
    mask.refinement = refinement;
}

fn local_adjust_box_blur_alpha(
    src: &[f32],
    width: usize,
    height: usize,
    radius: usize,
) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0; src.len()];
    let mut out = vec![0.0; src.len()];
    let mut prefix = vec![0.0; width.max(height) + 1];
    for y in 0..height {
        prefix[0] = 0.0;
        for x in 0..width {
            prefix[x + 1] = prefix[x] + src[y * width + x];
        }
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            let sum = prefix[x1 + 1] - prefix[x0];
            tmp[y * width + x] = sum / (x1 - x0 + 1) as f32;
        }
    }
    for x in 0..width {
        prefix[0] = 0.0;
        for y in 0..height {
            prefix[y + 1] = prefix[y] + tmp[y * width + x];
        }
        for y in 0..height {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius).min(height - 1);
            let sum = prefix[y1 + 1] - prefix[y0];
            out[y * width + x] = sum / (y1 - y0 + 1) as f32;
        }
    }
    out
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

fn build_local_adjust_subject_input(
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
        LOCAL_ADJUST_SUBJECT_INPUT_SIZE as u32,
        LOCAL_ADJUST_SUBJECT_INPUT_SIZE as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut input = ndarray::Array4::<f32>::zeros((
        1,
        3,
        LOCAL_ADJUST_SUBJECT_INPUT_SIZE,
        LOCAL_ADJUST_SUBJECT_INPUT_SIZE,
    ));
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    for y in 0..LOCAL_ADJUST_SUBJECT_INPUT_SIZE {
        for x in 0..LOCAL_ADJUST_SUBJECT_INPUT_SIZE {
            let p = resized.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                let v = p[c] as f32 / 255.0;
                input[[0, c, y, x]] = (v - mean[c]) / std[c];
            }
        }
    }
    Ok(input)
}

fn local_adjust_subject_output_size(shape: &[i64], raw_len: usize) -> (usize, usize) {
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
            LOCAL_ADJUST_SUBJECT_INPUT_SIZE,
            raw_len.max(1).div_ceil(LOCAL_ADJUST_SUBJECT_INPUT_SIZE),
        )
    }
}

/// BiRefNet の出力 (sigmoid 適用前のロジット、実測レンジ ±300) を 0-1 のマット alpha に変換する。
/// 旧 U²-Netp は出力が概ね 0-1 だったため min/max 正規化だったが、BiRefNet はロジット出力なので
/// sigmoid を直接適用する (min/max 正規化だと巨大レンジが線形圧縮されて灰色のぼやけたマットになる)。
/// 出力テンソルが `[.., 1, H, W]` の場合の最後の H*W を採用する。
fn sigmoid_local_adjust_subject_output(raw: &[f32], width: usize, height: usize) -> Vec<f32> {
    let total = width.saturating_mul(height);
    let len = total.min(raw.len());
    let offset = raw.len().saturating_sub(total);
    let mut out = vec![0.0_f32; total];
    for (idx, slot) in out.iter_mut().enumerate().take(len) {
        let v = raw[offset + idx];
        *slot = if v.is_finite() {
            (1.0 / (1.0 + (-v).exp())).clamp(0.0, 1.0)
        } else {
            0.0
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
    local_adjust_core::resize_mask_bilinear(src, src_w, src_h, dst_w, dst_h)
}

fn run_local_adjust_subject_segmentation(
    runtime: Arc<crate::ai::runtime::AiRuntime>,
    model_path: std::path::PathBuf,
    source: Arc<egui::ColorImage>,
) -> Result<local_adjust_core::RasterMask, String> {
    // BiRefNet は 1024² の重いモデルなので DirectML (GPU) でロードする。
    // (CPU では 1024² 推論が数十秒かかり実用にならない。DirectML EP 登録失敗時のみ
    //  register_directml_ep が CPU にフォールバックする。)
    runtime
        .load_model(crate::ai::ModelKind::SubjectMatte, &model_path)
        .map_err(|err| format!("BiRefNet load: {err}"))?;
    let input = build_local_adjust_subject_input(&source)?;
    let input_tensor =
        ort::value::Tensor::from_array(input).map_err(|err| format!("Tensor creation: {err}"))?;
    let (shape, raw) = runtime
        .with_session(crate::ai::ModelKind::SubjectMatte, |session| {
            let outputs = session
                .run(ort::inputs![input_tensor])
                .map_err(|err| crate::ai::AiError::Ort(format!("BiRefNet run: {err}")))?;
            let (shape, raw) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|err| crate::ai::AiError::Ort(format!("BiRefNet extract: {err}")))?;
            Ok((shape.iter().copied().collect::<Vec<i64>>(), raw.to_vec()))
        })
        .map_err(|err| err.to_string())?;
    let (small_w, small_h) = local_adjust_subject_output_size(&shape, raw.len());
    let small_mask = sigmoid_local_adjust_subject_output(&raw, small_w, small_h);
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

fn local_adjust_boundary_strength_at(image: &egui::ColorImage, x: usize, y: usize) -> f32 {
    local_adjust_edge_strength_at(image, x, y)
        .max(local_adjust_line_interior_strength_at(image, x, y))
}

fn local_adjust_edge_preview_size(width: usize, height: usize) -> [usize; 2] {
    let max_side = width.max(height).max(1);
    if max_side <= LOCAL_ADJUST_EDGE_PREVIEW_MAX_SIDE {
        [width.max(1), height.max(1)]
    } else {
        let scale = LOCAL_ADJUST_EDGE_PREVIEW_MAX_SIDE as f32 / max_side as f32;
        [
            ((width as f32 * scale).round() as usize).max(1),
            ((height as f32 * scale).round() as usize).max(1),
        ]
    }
}

fn build_local_adjust_edge_preview_image(
    source: &egui::ColorImage,
    preview_size: [usize; 2],
    edge_threshold: u8,
    ink_threshold: u8,
    gap_px: u8,
) -> egui::ColorImage {
    let [width, height] = source.size;
    let [preview_w, preview_h] = preview_size;
    if width == 0 || height == 0 || preview_w == 0 || preview_h == 0 {
        return egui::ColorImage::new([1, 1], vec![egui::Color32::TRANSPARENT]);
    }
    let mut pixels = vec![egui::Color32::TRANSPARENT; preview_w.saturating_mul(preview_h)];
    let edge_threshold_f = edge_threshold as f32;
    let ink_threshold_f = ink_threshold as f32;
    let gap_px = gap_px as usize;
    for py in 0..preview_h {
        let sy = ((py as f32 + 0.5) * height as f32 / preview_h as f32)
            .floor()
            .clamp(0.0, height.saturating_sub(1) as f32) as usize;
        for px in 0..preview_w {
            let sx = ((px as f32 + 0.5) * width as f32 / preview_w as f32)
                .floor()
                .clamp(0.0, width.saturating_sub(1) as f32) as usize;
            if local_adjust_boundary_pixel_at(
                source,
                sx,
                sy,
                edge_threshold_f,
                ink_threshold_f,
                gap_px,
            ) {
                let strength = local_adjust_boundary_strength_at(source, sx, sy);
                let base_threshold = edge_threshold_f.min(ink_threshold_f);
                let alpha = ((strength - base_threshold) / 96.0)
                    .clamp(0.18, 1.0)
                    .mul_add(180.0, 0.0)
                    .round() as u8;
                pixels[py * preview_w + px] =
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
            }
        }
    }
    egui::ColorImage::new([preview_w, preview_h], pixels)
}

fn local_adjust_edge_overlay_color(ctx: &egui::Context, alpha: u8) -> egui::Color32 {
    let t = ctx.input(|i| i.time);
    let phase = ((t * 3.0).sin() * 0.5 + 0.5) as f32;
    let r = 255_u8;
    let g = (72.0 + 168.0 * phase).round() as u8;
    let b = (220.0 - 156.0 * phase).round() as u8;
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

fn local_adjust_snap_point_to_edge(
    source: &egui::ColorImage,
    point: [f32; 2],
    radius: f32,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> [f32; 2] {
    let [width, height] = source.size;
    if width == 0 || height == 0 {
        return point;
    }
    let radius = radius.max(1.0);
    let min_x = (point[0] - radius).floor().max(0.0) as usize;
    let max_x = (point[0] + radius)
        .ceil()
        .min(width.saturating_sub(1) as f32) as usize;
    let min_y = (point[1] - radius).floor().max(0.0) as usize;
    let max_y = (point[1] + radius)
        .ceil()
        .min(height.saturating_sub(1) as f32) as usize;
    let radius_sq = radius * radius;
    let mut best = None;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - point[0];
            let dy = y as f32 + 0.5 - point[1];
            if dx * dx + dy * dy > radius_sq {
                continue;
            }
            if !local_adjust_boundary_pixel_at(source, x, y, edge_threshold, ink_threshold, gap_px)
            {
                continue;
            }
            let strength = local_adjust_boundary_strength_at(source, x, y);
            let distance = (dx * dx + dy * dy).sqrt();
            let normalized_distance = distance / radius;
            let score = strength / (1.0 + normalized_distance * 3.0);
            if best
                .map(|(_, _, best_score): (usize, usize, f32)| score > best_score)
                .unwrap_or(true)
            {
                best = Some((x, y, score));
            }
        }
    }
    best.map(|(x, y, _)| [x as f32 + 0.5, y as f32 + 0.5])
        .unwrap_or(point)
}

fn local_adjust_polygon_candidate_point(
    norm: [f32; 2],
    image_dims: (usize, usize),
    source: Option<&egui::ColorImage>,
    scale: f32,
    ctrl_held: bool,
    snap_radius: f32,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> ([f32; 2], [f32; 2], bool) {
    let raw = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
    let raw_point = [raw.0, raw.1];
    let Some(source) = source else {
        return (raw_point, raw_point, false);
    };
    if !ctrl_held || source.size != [image_dims.0, image_dims.1] {
        return (raw_point, raw_point, false);
    }
    let snap_radius_image = snap_radius.max(2.0) / scale.max(0.001);
    let snapped = local_adjust_snap_point_to_edge(
        source,
        raw_point,
        snap_radius_image,
        edge_threshold,
        ink_threshold,
        gap_px,
    );
    (snapped, raw_point, true)
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
            let thickness = (rx - lx).max(line_width.max(1.0));
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
            let thickness = (by - ty).max(line_width.max(1.0));
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

fn local_adjust_shape_op_to_vector(op: local_adjust_core::ShapeOp) -> crate::mask_db::ShapeOp {
    match op {
        local_adjust_core::ShapeOp::Add => crate::mask_db::ShapeOp::Add,
        local_adjust_core::ShapeOp::Subtract => crate::mask_db::ShapeOp::Subtract,
    }
}

fn vector_shape_op_to_local_adjust(op: crate::mask_db::ShapeOp) -> local_adjust_core::ShapeOp {
    match op {
        crate::mask_db::ShapeOp::Add => local_adjust_core::ShapeOp::Add,
        crate::mask_db::ShapeOp::Subtract => local_adjust_core::ShapeOp::Subtract,
    }
}

fn local_adjust_line_kind_to_vector(kind: local_adjust_core::LineKind) -> crate::mask_db::LineKind {
    match kind {
        local_adjust_core::LineKind::Vertical => crate::mask_db::LineKind::Vertical,
        local_adjust_core::LineKind::Horizontal => crate::mask_db::LineKind::Horizontal,
        local_adjust_core::LineKind::Diagonal => crate::mask_db::LineKind::Diagonal,
    }
}

fn vector_line_kind_to_local_adjust(kind: crate::mask_db::LineKind) -> local_adjust_core::LineKind {
    match kind {
        crate::mask_db::LineKind::Vertical => local_adjust_core::LineKind::Vertical,
        crate::mask_db::LineKind::Horizontal => local_adjust_core::LineKind::Horizontal,
        crate::mask_db::LineKind::Diagonal => local_adjust_core::LineKind::Diagonal,
    }
}

fn local_adjust_shape_to_vector_shape(
    shape: local_adjust_core::MaskShape,
) -> crate::mask_db::Shape {
    match shape {
        local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0,
            p1,
            thickness,
        } => crate::mask_db::Shape::Line {
            op: local_adjust_shape_op_to_vector(op),
            kind: local_adjust_line_kind_to_vector(kind),
            p0: (p0[0], p0[1]),
            p1: (p1[0], p1[1]),
            thickness,
        },
        local_adjust_core::MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => crate::mask_db::Shape::Rect {
            op: local_adjust_shape_op_to_vector(op),
            center: (center[0], center[1]),
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
        } => crate::mask_db::Shape::Ellipse {
            op: local_adjust_shape_op_to_vector(op),
            center: (center[0], center[1]),
            rx,
            ry,
            rotation_rad,
        },
    }
}

fn vector_shape_to_local_adjust_shape(
    shape: crate::mask_db::Shape,
) -> local_adjust_core::MaskShape {
    match shape {
        crate::mask_db::Shape::Line {
            op,
            kind,
            p0,
            p1,
            thickness,
        } => local_adjust_core::MaskShape::Line {
            op: vector_shape_op_to_local_adjust(op),
            kind: vector_line_kind_to_local_adjust(kind),
            p0: [p0.0, p0.1],
            p1: [p1.0, p1.1],
            thickness,
        },
        crate::mask_db::Shape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => local_adjust_core::MaskShape::Rect {
            op: vector_shape_op_to_local_adjust(op),
            center: [center.0, center.1],
            half_w,
            half_h,
            rotation_rad,
        },
        crate::mask_db::Shape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad,
        } => local_adjust_core::MaskShape::Ellipse {
            op: vector_shape_op_to_local_adjust(op),
            center: [center.0, center.1],
            rx,
            ry,
            rotation_rad,
        },
    }
}

fn local_adjust_translate_shape(
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

fn local_adjust_rotate_shape(
    shape: local_adjust_core::MaskShape,
    delta_rad: f32,
    snap_15deg: bool,
) -> local_adjust_core::MaskShape {
    let snap = |angle: f32| {
        if snap_15deg {
            let step = 15.0_f32.to_radians();
            (angle / step).round() * step
        } else {
            angle
        }
    };
    match shape {
        local_adjust_core::MaskShape::Line {
            op,
            kind,
            p0,
            p1,
            thickness,
        } => {
            let center = [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5];
            let current = (p1[1] - p0[1]).atan2(p1[0] - p0[0]);
            let next = snap(current + delta_rad);
            let half_len =
                (((p1[0] - p0[0]).powi(2) + (p1[1] - p0[1]).powi(2)).sqrt() * 0.5).max(0.5);
            let (s, c) = next.sin_cos();
            local_adjust_core::MaskShape::Line {
                op,
                kind,
                p0: [center[0] - c * half_len, center[1] - s * half_len],
                p1: [center[0] + c * half_len, center[1] + s * half_len],
                thickness,
            }
        }
        local_adjust_core::MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => local_adjust_core::MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad: snap(rotation_rad + delta_rad),
        },
        local_adjust_core::MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad,
        } => local_adjust_core::MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad: snap(rotation_rad + delta_rad),
        },
    }
}

fn local_adjust_point_in_vector_body(p: (f32, f32), poly: &[(f32, f32); 4]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) {
            let x_intersect = (xj - xi) * (p.1 - yi) / (yj - yi + 1e-9) + xi;
            if p.0 < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn selected_local_adjust_line_thickness(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    target: LocalAdjustMaskEditTarget,
    selected_shape: Option<usize>,
) -> Option<f32> {
    let selected = selected_shape?;
    let mask = local_adjust_target_raster_vector_mask_ref(layer, target)?;
    match mask.shapes.get(selected)? {
        local_adjust_core::MaskShape::Line { thickness, .. } => Some(*thickness),
        _ => None,
    }
}

fn set_selected_local_adjust_line_thickness(
    layer: &mut local_adjust_core::LocalAdjustmentLayer,
    target: LocalAdjustMaskEditTarget,
    selected_shape: Option<usize>,
    thickness: f32,
    image_dims: (usize, usize),
) -> bool {
    let Some(selected) = selected_shape else {
        return false;
    };
    let Some(mask) = local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, false)
    else {
        return false;
    };
    let Some(local_adjust_core::MaskShape::Line {
        thickness: line_thickness,
        ..
    }) = mask.shapes.get_mut(selected)
    else {
        return false;
    };
    let next = thickness.max(1.0);
    if (*line_thickness - next).abs() <= f32::EPSILON {
        return false;
    }
    *line_thickness = next;
    true
}

fn draw_local_adjust_ellipse(
    painter: &egui::Painter,
    transform: &DisplayedImageTransform,
    center: [f32; 2],
    rx: f32,
    ry: f32,
    stroke: egui::Stroke,
) {
    let mut points = Vec::with_capacity(73);
    for i in 0..=72 {
        let t = i as f32 / 72.0 * std::f32::consts::TAU;
        let norm = [center[0] + rx * t.cos(), center[1] + ry * t.sin()];
        if let Some(pos) = local_adjust_norm_to_screen(norm, transform) {
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

fn local_adjust_inverse_rotate_point(p: [f32; 2], center: [f32; 2], rotation_rad: f32) -> [f32; 2] {
    let (s, c) = (-rotation_rad).sin_cos();
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    [center[0] + dx * c - dy * s, center[1] + dx * s + dy * c]
}

fn local_adjust_shape_contains(shape: local_adjust_core::MaskShape, p: [f32; 2]) -> bool {
    match shape {
        local_adjust_core::MaskShape::Line {
            p0, p1, thickness, ..
        } => {
            let corners = local_adjust_line_corners(p0, p1, thickness.max(1.0));
            local_adjust_point_in_polygon(p, &corners)
        }
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

fn local_adjust_line_corners(p0: [f32; 2], p1: [f32; 2], thickness: f32) -> [[f32; 2]; 4] {
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let nx = -dy / len;
    let ny = dx / len;
    let half = thickness * 0.5;
    [
        [p0[0] + nx * half, p0[1] + ny * half],
        [p1[0] + nx * half, p1[1] + ny * half],
        [p1[0] - nx * half, p1[1] - ny * half],
        [p0[0] - nx * half, p0[1] - ny * half],
    ]
}

fn local_adjust_point_in_polygon(p: [f32; 2], points: &[[f32; 2]]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut prev = points.len() - 1;
    for i in 0..points.len() {
        let pi = points[i];
        let pj = points[prev];
        if (pi[1] > p[1]) != (pj[1] > p[1]) {
            let x = (pj[0] - pi[0]) * (p[1] - pi[1]) / (pj[1] - pi[1]) + pi[0];
            if p[0] < x {
                inside = !inside;
            }
        }
        prev = i;
    }
    inside
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

#[derive(Clone, Copy)]
struct LocalAdjustColorGradientGeometry {
    shape: local_adjust_core::ColorOverlayShape,
    angle_degrees: f32,
    linear_points_enabled: bool,
    linear_start: [f32; 2],
    linear_end: [f32; 2],
    center: [f32; 2],
    radius: f32,
}

fn local_adjust_linear_points_from_angle(angle_degrees: f32) -> ([f32; 2], [f32; 2]) {
    let angle = angle_degrees.to_radians();
    let dx = angle.cos();
    let dy = angle.sin();
    let tx = if dx.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        0.5 / dx.abs()
    };
    let ty = if dy.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        0.5 / dy.abs()
    };
    let t = tx.min(ty).max(0.001);
    (
        [
            (0.5 - dx * t).clamp(0.0, 1.0),
            (0.5 - dy * t).clamp(0.0, 1.0),
        ],
        [
            (0.5 + dx * t).clamp(0.0, 1.0),
            (0.5 + dy * t).clamp(0.0, 1.0),
        ],
    )
}

fn local_adjust_angle_from_linear_points(start: [f32; 2], end: [f32; 2]) -> Option<f32> {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    if dx * dx + dy * dy <= 0.000001 {
        None
    } else {
        Some(dy.atan2(dx).to_degrees())
    }
}

fn local_adjust_color_fill_gradient_geometry(
    params: &local_adjust_core::ColorFillParams,
) -> LocalAdjustColorGradientGeometry {
    LocalAdjustColorGradientGeometry {
        shape: params.shape,
        angle_degrees: params.angle_degrees,
        linear_points_enabled: params.linear_points_enabled,
        linear_start: params.linear_start,
        linear_end: params.linear_end,
        center: params.center,
        radius: params.radius,
    }
}

fn local_adjust_apply_color_fill_gradient_geometry(
    params: &mut local_adjust_core::ColorFillParams,
    geometry: LocalAdjustColorGradientGeometry,
) {
    params.angle_degrees = geometry.angle_degrees;
    params.linear_points_enabled = geometry.linear_points_enabled;
    params.linear_start = geometry.linear_start;
    params.linear_end = geometry.linear_end;
    params.center = geometry.center;
    params.radius = geometry.radius;
}

fn local_adjust_color_overlay_gradient_geometry(
    params: &local_adjust_core::ColorOverlayParams,
) -> LocalAdjustColorGradientGeometry {
    LocalAdjustColorGradientGeometry {
        shape: params.shape,
        angle_degrees: params.angle_degrees,
        linear_points_enabled: params.linear_points_enabled,
        linear_start: params.linear_start,
        linear_end: params.linear_end,
        center: params.center,
        radius: params.radius,
    }
}

fn local_adjust_apply_color_overlay_gradient_geometry(
    params: &mut local_adjust_core::ColorOverlayParams,
    geometry: LocalAdjustColorGradientGeometry,
) {
    params.angle_degrees = geometry.angle_degrees;
    params.linear_points_enabled = geometry.linear_points_enabled;
    params.linear_start = geometry.linear_start;
    params.linear_end = geometry.linear_end;
    params.center = geometry.center;
    params.radius = geometry.radius;
}

fn local_adjust_effect_gradient_geometry(
    effect: &local_adjust_core::LocalEffect,
) -> Option<LocalAdjustColorGradientGeometry> {
    match effect {
        local_adjust_core::LocalEffect::ColorFill(params) => {
            Some(local_adjust_color_fill_gradient_geometry(params))
        }
        local_adjust_core::LocalEffect::ColorOverlay(params) => {
            Some(local_adjust_color_overlay_gradient_geometry(params))
        }
        _ => None,
    }
}

fn local_adjust_apply_effect_gradient_geometry(
    effect: &mut local_adjust_core::LocalEffect,
    geometry: LocalAdjustColorGradientGeometry,
) -> bool {
    match effect {
        local_adjust_core::LocalEffect::ColorFill(params) => {
            if params.shape != geometry.shape {
                return false;
            }
            local_adjust_apply_color_fill_gradient_geometry(params, geometry);
            true
        }
        local_adjust_core::LocalEffect::ColorOverlay(params) => {
            if params.shape != geometry.shape {
                return false;
            }
            local_adjust_apply_color_overlay_gradient_geometry(params, geometry);
            true
        }
        _ => false,
    }
}

fn local_adjust_color_gradient_linear_points(
    geometry: LocalAdjustColorGradientGeometry,
) -> ([f32; 2], [f32; 2]) {
    if geometry.linear_points_enabled {
        (geometry.linear_start, geometry.linear_end)
    } else {
        local_adjust_linear_points_from_angle(geometry.angle_degrees)
    }
}

fn local_adjust_set_color_gradient_linear_points(
    geometry: &mut LocalAdjustColorGradientGeometry,
    start: [f32; 2],
    end: [f32; 2],
) {
    geometry.linear_points_enabled = true;
    geometry.linear_start = [start[0].clamp(0.0, 1.0), start[1].clamp(0.0, 1.0)];
    geometry.linear_end = [end[0].clamp(0.0, 1.0), end[1].clamp(0.0, 1.0)];
    if let Some(angle) =
        local_adjust_angle_from_linear_points(geometry.linear_start, geometry.linear_end)
    {
        geometry.angle_degrees = angle;
    }
}

#[cfg(test)]
fn local_adjust_effect_gradient_handle_positions(
    effect: &local_adjust_core::LocalEffect,
    rect: egui::Rect,
) -> Vec<(crate::app::LocalAdjustCanvasDragKind, egui::Pos2)> {
    let Some(geometry) = local_adjust_effect_gradient_geometry(effect) else {
        return Vec::new();
    };
    match geometry.shape {
        local_adjust_core::ColorOverlayShape::Linear => {
            let (start, end) = local_adjust_color_gradient_linear_points(geometry);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd,
                    local_adjust_drawn_norm_to_screen(rect, end),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart,
                    local_adjust_drawn_norm_to_screen(rect, start),
                ),
            ]
        }
        local_adjust_core::ColorOverlayShape::Radial => {
            let center = local_adjust_drawn_norm_to_screen(rect, geometry.center);
            let radius = geometry.radius.clamp(0.02, 2.0);
            let radius_handle = egui::pos2(center.x + radius * rect.width(), center.y);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius,
                    radius_handle,
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientCenter,
                    center,
                ),
            ]
        }
        local_adjust_core::ColorOverlayShape::Unselected
        | local_adjust_core::ColorOverlayShape::Solid => Vec::new(),
    }
}

#[cfg(test)]
fn local_adjust_effect_gradient_handle_hit(
    effect: &local_adjust_core::LocalEffect,
    pos: egui::Pos2,
    drawn_rect: egui::Rect,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    const HIT_RADIUS: f32 = 15.0;
    local_adjust_effect_gradient_handle_positions(effect, drawn_rect)
        .into_iter()
        .find_map(|(kind, handle)| (handle.distance(pos) <= HIT_RADIUS).then_some(kind))
}

fn local_adjust_effect_gradient_handle_hit_transform(
    effect: &local_adjust_core::LocalEffect,
    pos: egui::Pos2,
    transform: &DisplayedImageTransform,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    const HIT_RADIUS: f32 = 15.0;
    let geometry = local_adjust_effect_gradient_geometry(effect)?;
    let positions = match geometry.shape {
        local_adjust_core::ColorOverlayShape::Linear => {
            let (start, end) = local_adjust_color_gradient_linear_points(geometry);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd,
                    transform.source_normalized_to_screen(egui::pos2(end[0], end[1])),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart,
                    transform.source_normalized_to_screen(egui::pos2(start[0], start[1])),
                ),
            ]
        }
        local_adjust_core::ColorOverlayShape::Radial => {
            let radius = geometry.radius.clamp(0.02, 2.0);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius,
                    transform.source_normalized_to_screen(egui::pos2(
                        geometry.center[0] + radius,
                        geometry.center[1],
                    )),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientCenter,
                    transform.source_normalized_to_screen(egui::pos2(
                        geometry.center[0],
                        geometry.center[1],
                    )),
                ),
            ]
        }
        local_adjust_core::ColorOverlayShape::Unselected
        | local_adjust_core::ColorOverlayShape::Solid => Vec::new(),
    };
    positions
        .into_iter()
        .find_map(|(kind, handle)| (handle.distance(pos) <= HIT_RADIUS).then_some(kind))
}

fn apply_local_adjust_effect_gradient_handle_drag(
    effect: &mut local_adjust_core::LocalEffect,
    kind: crate::app::LocalAdjustCanvasDragKind,
    norm: [f32; 2],
) -> bool {
    let Some(mut geometry) = local_adjust_effect_gradient_geometry(effect) else {
        return false;
    };
    match (geometry.shape, kind) {
        (
            local_adjust_core::ColorOverlayShape::Linear,
            crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart,
        ) => {
            let (_, end) = local_adjust_color_gradient_linear_points(geometry);
            local_adjust_set_color_gradient_linear_points(&mut geometry, norm, end);
        }
        (
            local_adjust_core::ColorOverlayShape::Linear,
            crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd,
        ) => {
            let (start, _) = local_adjust_color_gradient_linear_points(geometry);
            local_adjust_set_color_gradient_linear_points(&mut geometry, start, norm);
        }
        (
            local_adjust_core::ColorOverlayShape::Radial,
            crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientCenter,
        ) => {
            geometry.center = [norm[0].clamp(0.0, 1.0), norm[1].clamp(0.0, 1.0)];
        }
        (
            local_adjust_core::ColorOverlayShape::Radial,
            crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius,
        ) => {
            let dx = norm[0] - geometry.center[0];
            let dy = norm[1] - geometry.center[1];
            geometry.radius = (dx * dx + dy * dy).sqrt().clamp(0.02, 2.0);
        }
        _ => return false,
    }
    local_adjust_apply_effect_gradient_geometry(effect, geometry)
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

fn draw_local_adjust_effect_gradient_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    effect: &local_adjust_core::LocalEffect,
) {
    let Some(geometry) = local_adjust_effect_gradient_geometry(effect) else {
        return;
    };
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 220, 255));
    let soft_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(120, 220, 255, 110),
    );
    let start_fill = egui::Color32::from_rgb(215, 250, 255);
    let end_fill = egui::Color32::from_rgb(120, 220, 255);
    let handle_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(10, 30, 36));

    match geometry.shape {
        local_adjust_core::ColorOverlayShape::Linear => {
            let (start, end) = local_adjust_color_gradient_linear_points(geometry);
            let start_screen = local_adjust_drawn_norm_to_screen(rect, start);
            let end_screen = local_adjust_drawn_norm_to_screen(rect, end);
            painter.line_segment([start_screen, end_screen], stroke);
            painter.circle_filled(start_screen, 6.0, start_fill);
            painter.circle_stroke(start_screen, 6.0, handle_stroke);
            painter.circle_filled(end_screen, 6.0, end_fill);
            painter.circle_stroke(end_screen, 6.0, handle_stroke);
        }
        local_adjust_core::ColorOverlayShape::Radial => {
            let center = local_adjust_drawn_norm_to_screen(rect, geometry.center);
            let radius = geometry.radius.clamp(0.02, 2.0);
            let radius_x = radius * rect.width();
            let radius_y = radius * rect.height();
            let radius_handle = egui::pos2(center.x + radius_x, center.y);
            draw_local_adjust_ellipse_stroke(painter, center, radius_x, radius_y, stroke);
            painter.line_segment([center, radius_handle], soft_stroke);
            painter.circle_filled(center, 6.0, start_fill);
            painter.circle_stroke(center, 6.0, handle_stroke);
            painter.circle_filled(radius_handle, 6.0, end_fill);
            painter.circle_stroke(radius_handle, 6.0, handle_stroke);
        }
        local_adjust_core::ColorOverlayShape::Unselected
        | local_adjust_core::ColorOverlayShape::Solid => {}
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
        local_adjust_core::LocalEffect::Repair(params)
            if params.mode == local_adjust_core::RepairMode::Clone =>
        {
            let source = params
                .clone_source_uv
                .map(|point| local_adjust_drawn_norm_to_screen(rect, point));
            let destination = params
                .clone_destination_uv
                .map(|point| local_adjust_drawn_norm_to_screen(rect, point));
            if let (Some(source), Some(destination)) = (source, destination) {
                painter.line_segment(
                    [source, destination],
                    egui::Stroke::new(
                        1.5,
                        egui::Color32::from_rgba_unmultiplied(255, 225, 110, 190),
                    ),
                );
            }
            if let Some(source) = source {
                draw_local_adjust_effect_center_marker(
                    painter,
                    source,
                    "コピー元",
                    egui::Color32::from_rgb(90, 220, 255),
                );
            }
            if let Some(destination) = destination {
                draw_local_adjust_effect_center_marker(
                    painter,
                    destination,
                    "塗り先",
                    egui::Color32::from_rgb(255, 205, 90),
                );
            }
        }
        local_adjust_core::LocalEffect::ColorFill(_)
        | local_adjust_core::LocalEffect::ColorOverlay(_) => {
            draw_local_adjust_effect_gradient_overlay(painter, rect, effect);
        }
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

#[cfg(test)]
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

#[cfg(test)]
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

fn local_adjust_tilt_shift_handle_hit_transform(
    effect: &local_adjust_core::LocalEffect,
    pos: egui::Pos2,
    transform: &DisplayedImageTransform,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    let local_adjust_core::LocalEffect::TiltShift(params) = effect else {
        return None;
    };
    if !params.range_initialized {
        return None;
    }
    let positions = match params.mode {
        local_adjust_core::TiltShiftMode::Linear => {
            let angle = params.angle_degrees.to_radians();
            let dir = [angle.cos(), angle.sin()];
            let focus = params.focus_width.max(0.0);
            let outer = focus + params.falloff.max(0.001);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftFocus,
                    local_adjust_offset_norm(params.center, dir, focus),
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuter,
                    local_adjust_offset_norm(params.center, dir, outer),
                ),
            ]
        }
        local_adjust_core::TiltShiftMode::Radial => {
            let outer = 1.0 + params.falloff.max(0.001);
            vec![
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerX,
                    [
                        params.center[0] + params.radius[0].max(0.001),
                        params.center[1],
                    ],
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftInnerY,
                    [
                        params.center[0],
                        params.center[1] + params.radius[1].max(0.001),
                    ],
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterX,
                    [
                        params.center[0] + params.radius[0].max(0.001) * outer,
                        params.center[1],
                    ],
                ),
                (
                    crate::app::LocalAdjustCanvasDragKind::TiltShiftOuterY,
                    [
                        params.center[0],
                        params.center[1] + params.radius[1].max(0.001) * outer,
                    ],
                ),
            ]
        }
    };
    positions.into_iter().find_map(|(kind, norm)| {
        let handle = transform.source_normalized_to_screen(egui::pos2(norm[0], norm[1]));
        (handle.distance(pos) <= 15.0).then_some(kind)
    })
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
    // 新しいベースマスクは非反転で始める。被写体マスクで「背景を選択」(mask_inverted=true)
    // していた状態から手動マスク等へ種類変更すると、空マスク (alpha=0) が反転されて
    // 全面 alpha=1 になり、ビットマップ消去でも戻せなくなる退行があったため明示リセットする。
    layer.mask_inverted = false;
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
    let full_subtract_has_content = local_adjust_full_subtract_mask_has_content(layer);
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
            let alpha = local_adjust_mask_preview_alpha_cached(
                layer,
                source,
                width,
                height,
                x,
                y,
                full_subtract_has_content,
            )
            .clamp(0.0, 1.0);
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
    transform: &DisplayedImageTransform,
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
    let rect_w = transform.full_image_rect.width().max(1.0);
    let rect_h = transform.full_image_rect.height().max(1.0);
    let scale = (LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS / rect_w.max(rect_h)).min(1.0);
    let tex_w = (rect_w * scale)
        .round()
        .clamp(1.0, LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS) as usize;
    let tex_h = (rect_h * scale)
        .round()
        .clamp(1.0, LOCAL_ADJUST_MASK_PREVIEW_MAX_TEXELS) as usize;
    let image = build_local_adjust_mask_preview_image(
        layer,
        source,
        (width, height),
        [tex_w, tex_h],
        time_sec,
        colors,
        edit_target,
    );
    if texture_slot
        .as_ref()
        .is_some_and(|texture| texture.size() != [tex_w, tex_h])
    {
        *texture_slot = None;
    }
    if let Some(texture) = texture_slot.as_mut() {
        texture.set(image, egui::TextureOptions::LINEAR);
    } else {
        *texture_slot = Some(painter.ctx().load_texture(
            "local_adjust_mask_preview",
            image,
            egui::TextureOptions::LINEAR,
        ));
    }
    let Some(texture) = texture_slot.as_ref() else {
        return;
    };
    transform.paint_texture(painter, texture.id(), egui::Color32::WHITE);
}

fn build_local_adjust_mask_preview_image(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    image_dims: (usize, usize),
    preview_size: [usize; 2],
    time_sec: f32,
    colors: crate::app::LocalAdjustMaskPreviewColors,
    edit_target: Option<LocalAdjustMaskEditTarget>,
) -> egui::ColorImage {
    let width = image_dims.0.max(1);
    let height = image_dims.1.max(1);
    let [tex_w, tex_h] = [preview_size[0].max(1), preview_size[1].max(1)];
    let source = source.filter(|source| source.size == [width, height]);
    let full_subtract_has_content = local_adjust_full_subtract_mask_has_content(layer);
    let mut pixels = Vec::with_capacity(tex_w.saturating_mul(tex_h));

    for gy in 0..tex_h {
        for gx in 0..tex_w {
            let x = (((gx as f32 + 0.5) * width as f32 / tex_w as f32) as usize)
                .min(width.saturating_sub(1));
            let y = (((gy as f32 + 0.5) * height as f32 / tex_h as f32) as usize)
                .min(height.saturating_sub(1));
            pixels.push(local_adjust_mask_preview_color_cached(
                layer,
                source,
                width,
                height,
                x,
                y,
                time_sec,
                colors,
                edit_target,
                full_subtract_has_content,
            ));
        }
    }

    egui::ColorImage::new([tex_w, tex_h], pixels)
}

#[cfg(test)]
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
    local_adjust_mask_preview_color_cached(
        layer,
        source,
        width,
        height,
        x,
        y,
        time_sec,
        colors,
        edit_target,
        local_adjust_full_subtract_mask_has_content(layer),
    )
}

fn local_adjust_mask_preview_color_cached(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    time_sec: f32,
    colors: crate::app::LocalAdjustMaskPreviewColors,
    edit_target: Option<LocalAdjustMaskEditTarget>,
    full_subtract_has_content: bool,
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
            let alpha = local_adjust_mask_preview_alpha_cached(
                layer,
                source,
                width,
                height,
                x,
                y,
                full_subtract_has_content,
            );
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

#[cfg(test)]
fn local_adjust_mask_preview_alpha(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    local_adjust_mask_preview_alpha_cached(
        layer,
        source,
        width,
        height,
        x,
        y,
        local_adjust_full_subtract_mask_has_content(layer),
    )
}

fn local_adjust_mask_preview_alpha_cached(
    layer: &local_adjust_core::LocalAdjustmentLayer,
    source: Option<&egui::ColorImage>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    full_subtract_has_content: bool,
) -> f32 {
    let idx = y.saturating_mul(width).saturating_add(x);
    let mut alpha = match &layer.mask {
        local_adjust_core::LocalMask::Full => {
            if full_subtract_has_content {
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

fn local_adjust_full_subtract_mask_has_content(
    layer: &local_adjust_core::LocalAdjustmentLayer,
) -> bool {
    layer
        .manual_override
        .subtract
        .as_ref()
        .is_some_and(local_adjust_raster_vector_mask_has_content)
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
    transform: &DisplayedImageTransform,
) -> Option<crate::app::LocalAdjustCanvasDragKind> {
    const HIT_RADIUS: f32 = 14.0;
    match &layer.mask {
        local_adjust_core::LocalMask::LinearGradient(mask) if mask.initialized => {
            let start = local_adjust_norm_to_screen(mask.start, transform)?;
            let end = local_adjust_norm_to_screen(mask.end, transform)?;
            if end.distance(pos) <= HIT_RADIUS {
                Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientEnd)
            } else if start.distance(pos) <= HIT_RADIUS {
                Some(crate::app::LocalAdjustCanvasDragKind::LinearGradientStart)
            } else {
                None
            }
        }
        local_adjust_core::LocalMask::RadialGradient(mask) if mask.initialized => {
            let handles = [
                (
                    local_adjust_norm_to_screen(
                        [
                            mask.center[0] + mask.outer_radius.max(mask.inner_radius),
                            mask.center[1],
                        ],
                        transform,
                    )?,
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterX,
                ),
                (
                    local_adjust_norm_to_screen(
                        [
                            mask.center[0],
                            mask.center[1] + mask.outer_radius_y.max(mask.inner_radius_y),
                        ],
                        transform,
                    )?,
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientOuterY,
                ),
                (
                    local_adjust_norm_to_screen(
                        [mask.center[0] + mask.inner_radius.max(0.0), mask.center[1]],
                        transform,
                    )?,
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerX,
                ),
                (
                    local_adjust_norm_to_screen(
                        [
                            mask.center[0],
                            mask.center[1] + mask.inner_radius_y.max(0.0),
                        ],
                        transform,
                    )?,
                    crate::app::LocalAdjustCanvasDragKind::RadialGradientInnerY,
                ),
                (
                    local_adjust_norm_to_screen(mask.center, transform)?,
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
                    .weak(),
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
            ui.label(egui::RichText::new("輝度範囲").size(11.0).weak());
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
                "元画像から AI で被写体マスクを生成します。"
            } else {
                "被写体マスク生成には編集用追加ファイルが必要です。保存済みマスクの表示・編集は可能です。"
            };
            if generate_response.on_hover_text(generate_tip).clicked() {
                effect_requests.generate_subject_mask = Some(layer_idx);
            }
            // モデル未導入時はダウンロード導線を出す (spec §9)。
            if !subject_model_available {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("編集用追加ファイルが必要です")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(255, 180, 90)),
                    );
                    if ui
                        .small_button("ダウンロード")
                        .on_hover_text(
                            "被写体分離モデルを含む編集用追加ファイルをダウンロードします。",
                        )
                        .clicked()
                    {
                        effect_requests.request_editing_addon_download = true;
                    }
                });
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
            ui.separator();
            ui.label(
                egui::RichText::new("マスク補正")
                    .size(11.0)
                    .strong()
                    .color(ui.visuals().text_color()),
            );
            let mut refinement_enabled = mask.refinement.enabled;
            let enable_response = ui.checkbox(&mut refinement_enabled, "マスクを整形");
            if enable_response
                .on_hover_text("ONにすると、生成直後の元マットから境界向けのマスクを再生成します。")
                .changed()
            {
                if refinement_enabled {
                    apply_local_adjust_subject_refinement(
                        mask,
                        local_adjust_core::SubjectMaskRefinement {
                            enabled: true,
                            threshold: mask.refinement.threshold,
                            expand_px: mask.refinement.expand_px,
                            feather_px: mask.refinement.feather_px.max(0),
                        },
                    );
                } else {
                    let source = mask.source_alpha.as_ref().unwrap_or(&mask.alpha).clone();
                    mask.alpha = source;
                    mask.refinement.enabled = false;
                }
                changed = true;
            }

            let controls_enabled = mask.refinement.enabled;
            let mut preset_refinement = None;
            ui.add_enabled_ui(controls_enabled, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for preset in LocalAdjustSubjectRefinementPreset::ALL {
                        if ui.add(egui::Button::new(preset.label()).small()).clicked() {
                            preset_refinement = Some(preset.refinement());
                        }
                    }
                });
            });
            if let Some(refinement) = preset_refinement {
                apply_local_adjust_subject_refinement(mask, refinement);
                changed = true;
            }

            let mut threshold = mask.refinement.threshold;
            let mut expand = mask.refinement.expand_px;
            let mut feather = mask.refinement.feather_px.max(0);
            let threshold_response = ui
                .add_enabled(
                    controls_enabled,
                    egui::Slider::new(&mut threshold, 0.05..=0.95).text("しきい値"),
                )
                .on_hover_text(
                    "この値以上を被写体として残します。上げるほど背景側の半透明が減ります。",
                );
            let expand_response = ui
                .add_enabled(
                    controls_enabled,
                    egui::Slider::new(&mut expand, -4..=4).text("収縮/拡張"),
                )
                .on_hover_text("マイナスで少し内側へ縮め、プラスで外側へ広げます。");
            let feather_response = ui
                .add_enabled(
                    controls_enabled,
                    egui::Slider::new(&mut feather, 0..=8).text("境界なめらか"),
                )
                .on_hover_text("2値化後の境界だけをなじませます。0は完全な2値です。");
            if controls_enabled
                && (threshold_response.changed()
                    || expand_response.changed()
                    || feather_response.changed())
            {
                apply_local_adjust_subject_refinement(
                    mask,
                    local_adjust_core::SubjectMaskRefinement {
                        enabled: true,
                        threshold,
                        expand_px: expand,
                        feather_px: feather.max(0),
                    },
                );
                changed = true;
            }

            let stats = local_adjust_subject_mask_stats(mask);
            let mode_label = if mask.refinement.enabled {
                "整形済み"
            } else {
                "元マット"
            };
            ui.label(
                egui::RichText::new(format!(
                    "{mode_label} / 前景 {:.1}% / 半透明 {:.1}%",
                    stats.foreground_percent, stats.soft_percent
                ))
                .size(10.0)
                .weak(),
            );
        }
        local_adjust_core::LocalMask::Segmentation(mask) => {
            ui.label(egui::RichText::new("領域分割").size(11.0).weak());
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
                        .weak(),
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
                    .weak(),
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
    disabled_tooltip: Option<&str>,
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
        resp.on_hover_text(disabled_tooltip.unwrap_or("画像を開いているときのみ使用できます"))
    }
}

fn image_edit_tools_disabled_reason(
    detached_reason: Option<&'static str>,
    continuous_reading: bool,
) -> Option<&'static str> {
    detached_reason.or_else(|| {
        continuous_reading
            .then_some(crate::ui_fullscreen::CONTINUOUS_READING_EDIT_TOOLS_DISABLED_REASON)
    })
}

fn image_export_disabled_reason(continuous_reading: bool) -> Option<&'static str> {
    continuous_reading
        .then_some(crate::ui_fullscreen::CONTINUOUS_READING_EDIT_TOOLS_DISABLED_REASON)
}

#[cfg(test)]
mod image_edit_tools_disabled_reason_tests {
    use super::*;
    use crate::ui_fullscreen::CONTINUOUS_READING_EDIT_TOOLS_DISABLED_REASON;

    #[test]
    fn edit_tools_combine_detached_and_continuous_reasons() {
        assert_eq!(image_edit_tools_disabled_reason(None, false), None);
        assert_eq!(
            image_edit_tools_disabled_reason(None, true),
            Some(CONTINUOUS_READING_EDIT_TOOLS_DISABLED_REASON)
        );
        assert_eq!(
            image_edit_tools_disabled_reason(Some("detached"), true),
            Some("detached")
        );
    }

    #[test]
    fn export_ignores_detached_reason_but_not_continuous_reading() {
        let detached_reason = Some("detached");

        assert_eq!(
            image_edit_tools_disabled_reason(detached_reason, false),
            detached_reason
        );
        assert_eq!(image_export_disabled_reason(false), None);
        assert_eq!(
            image_export_disabled_reason(true),
            Some(CONTINUOUS_READING_EDIT_TOOLS_DISABLED_REASON)
        );
        assert_eq!(
            image_edit_tools_disabled_reason(detached_reason, true),
            detached_reason,
            "the other five tools keep the detached reason"
        );
    }
}

fn draw_left_panel_tab_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &'static str,
    tab: crate::settings::FullscreenLeftPanelTab,
    selected: &mut crate::settings::FullscreenLeftPanelTab,
) -> bool {
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
    let active = *selected == tab;
    let bg = if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 220)
    } else if resp.hovered() {
        egui::Color32::from_rgba_unmultiplied(95, 95, 95, 210)
    } else {
        egui::Color32::from_rgba_unmultiplied(55, 55, 55, 180)
    };
    ui.painter().rect_filled(rect, 5.0, bg);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        tab.label(),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    if resp.clicked() && !active {
        *selected = tab;
        true
    } else {
        false
    }
}

fn draw_left_panel_close_button(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
    let response = ui.interact(
        rect,
        egui::Id::new("left_panel_close"),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
        );
    }
    let center = rect.center();
    let delta = rect.width().min(rect.height()) * 0.22;
    let stroke = egui::Stroke::new(1.7, egui::Color32::from_gray(225));
    ui.painter().line_segment(
        [
            egui::pos2(center.x - delta, center.y - delta),
            egui::pos2(center.x + delta, center.y + delta),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(center.x + delta, center.y - delta),
            egui::pos2(center.x - delta, center.y + delta),
        ],
        stroke,
    );
    response.on_hover_text("閉じる")
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
                    .color(ui.visuals().text_color()),
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
                    .color(ui.visuals().text_color()),
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

fn draw_colorize_gradient_bar(
    ui: &mut egui::Ui,
    colors: &[[u8; 3]; 256],
    tooltip: &str,
) -> egui::Response {
    const BAR_HEIGHT: f32 = 18.0;
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), egui::Sense::hover());
    let inner = rect.shrink(1.0);
    let mut mesh = egui::Mesh::default();
    for (index, color) in colors.iter().enumerate() {
        let x = egui::lerp(inner.left()..=inner.right(), index as f32 / 255.0);
        let vertex = mesh.vertices.len() as u32;
        let color = egui::Color32::from_rgb(color[0], color[1], color[2]);
        mesh.colored_vertex(egui::pos2(x, inner.top()), color);
        mesh.colored_vertex(egui::pos2(x, inner.bottom()), color);
        if index > 0 {
            mesh.add_triangle(vertex - 2, vertex, vertex + 1);
            mesh.add_triangle(vertex - 2, vertex + 1, vertex - 1);
        }
    }
    ui.painter().rect_filled(
        rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
    );
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55),
        ),
        egui::StrokeKind::Inside,
    );
    response.on_hover_text(tooltip)
}

fn draw_colorize_gradient_preview(ui: &mut egui::Ui, params: &crate::colorize::ColorizeParams) {
    let grayscale = std::array::from_fn(|index| [index as u8; 3]);
    let colorized = crate::colorize::preview_lut(params);

    ui.label(
        egui::RichText::new("階調プレビュー（暗部 → 明部）")
            .size(SECTION_FONT)
            .color(ui.visuals().text_color()),
    );
    draw_colorize_gradient_bar(ui, &grayscale, "カラー化前の入力輝度です。");
    ui.add_space(2.0);
    draw_colorize_gradient_bar(
        ui,
        &colorized,
        "現在のパレット、制御点、強さ、元画像の明るさ保持を反映した階調です。",
    );
}

fn draw_colorize_preset_slots(
    ui: &mut egui::Ui,
    params: &mut crate::colorize::ColorizeParams,
    slots: &mut crate::colorize::ColorizePresetSlots,
) -> (bool, bool) {
    let mut changed = false;
    let mut settings_changed = false;

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("カラー化設定保存スロット")
            .size(SECTION_FONT)
            .color(ui.visuals().text_color()),
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for slot_index in 0..4 {
            let load = ui
                .add_enabled(
                    slots.slots[slot_index].is_some(),
                    egui::Button::new(format!("{}", slot_index + 1))
                        .small()
                        .min_size(egui::vec2(22.0, 22.0)),
                )
                .on_hover_text(format!("カラー化スロット{}を読み込む", slot_index + 1));
            if load.clicked()
                && let Some(saved) = slots.slots[slot_index].clone()
            {
                *params = saved;
                changed = true;
            }
            if ui
                .add(
                    egui::Button::new("💾")
                        .small()
                        .min_size(egui::vec2(22.0, 22.0)),
                )
                .on_hover_text(format!(
                    "現在のカラー化設定をスロット{}に保存",
                    slot_index + 1
                ))
                .clicked()
            {
                slots.slots[slot_index] = Some(params.clone());
                settings_changed = true;
            }
        }
    });

    (changed, settings_changed)
}

fn draw_colorize_settings(
    ui: &mut egui::Ui,
    params: &mut AdjustParams,
    slots: &mut crate::colorize::ColorizePresetSlots,
) -> (bool, bool, bool) {
    use crate::colorize::{ColorizeControlPoint, ColorizeMode, ColorizePalette, ToneDensityMethod};

    let mut changed = false;
    let mut dragging = false;
    let mut settings_changed = false;

    ui.label(
        egui::RichText::new("カラー化")
            .size(SECTION_FONT)
            .color(ui.visuals().text_color()),
    );
    // `ColorizeMode` は「OFF かどうか」と「どの画像に適用するか」を 1 つの enum で兼ねるので、
    // OFF にした時点で対象の選択が消える。有効な間の値をセッション内に控えておき、ON に戻す
    // ときに復元する。有効な間は毎フレーム上書きするため、スロット読み込みなど UI 以外の
    // 書き手が mode を変えても追随し、控えが実態からずれない (= 対象の正本は常に mode)。
    let restore_mode_id = egui::Id::new("colorize_last_enabled_mode");
    if params.colorize.mode != ColorizeMode::Disabled {
        let mode = params.colorize.mode;
        ui.data_mut(|data| data.insert_temp(restore_mode_id, mode));
    }
    let restore_mode = ui
        .data(|data| data.get_temp::<ColorizeMode>(restore_mode_id))
        .filter(|mode| *mode != ColorizeMode::Disabled)
        .unwrap_or(ColorizeMode::MonochromeOnly);

    let mut enabled = params.colorize.is_enabled();
    if ui
        .checkbox(&mut enabled, "モノクロ画像を階調カラー化")
        .changed()
    {
        params.colorize.mode = if enabled {
            restore_mode
        } else {
            ColorizeMode::Disabled
        };
        changed = true;
    }

    let controls_enabled = params.colorize.is_enabled();
    ui.add_enabled_ui(controls_enabled, |ui| {
        // OFF 中は mode が対象を語れないので、復元される予定の値を映す。そうしないと
        // 「全画像に適用」で切ったのにチェックが入って見え、ON に戻した瞬間に外れる。
        let displayed_mode = match params.colorize.mode {
            ColorizeMode::Disabled => restore_mode,
            mode => mode,
        };
        let mut only_monochrome = displayed_mode != ColorizeMode::AllImages;
        if ui
            .checkbox(&mut only_monochrome, "モノクロ系画像だけに適用")
            .on_hover_text(
                "純粋なグレースケールだけでなく、黄ばんだ紙や青みのあるスキャンも\n\
                 一本の色軸に沿う画像として判定します。",
            )
            .changed()
        {
            params.colorize.mode = if only_monochrome {
                ColorizeMode::MonochromeOnly
            } else {
                ColorizeMode::AllImages
            };
            changed = true;
        }
        // 表示条件は `mode` ではなく、いま描いたチェックボックスの状態に合わせる。
        // `mode` は OFF と「全画像に適用」を 1 つの enum で兼ねているため、`MonochromeOnly`
        // で判定するとカラー化を切った瞬間にこの行だけ消えて、下のパネル内容が動く。
        // チェックが入って見えている以上、その設定行は出したまま無効化するのが筋。
        if only_monochrome {
            ui.horizontal(|ui| {
                ui.label("色味の許容量");
                if params.colorize.mono_tolerance != 12
                    && ui
                        .small_button("↩")
                        .on_hover_text("デフォルトに戻す")
                        .clicked()
                {
                    params.colorize.mono_tolerance = 12;
                    changed = true;
                }
            });
            let mut tolerance = params.colorize.mono_tolerance as f32;
            let response = ui.add(egui::Slider::new(&mut tolerance, 1.0..=64.0).step_by(1.0));
            if response.changed() {
                params.colorize.mono_tolerance = tolerance as u8;
                changed = true;
            }
            dragging |= response.dragged();
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("濃さを整える");
            if params.colorize.density_normalization_strength != 0
                && ui
                    .small_button("↩")
                    .on_hover_text("補正なし（0%）に戻す")
                    .clicked()
            {
                params.colorize.density_normalization_strength = 0;
                changed = true;
            }
        });
        let mut density_strength = params.colorize.density_normalization_strength as f32;
        let response = ui
            .add(
                egui::Slider::new(&mut density_strength, 0.0..=100.0)
                    .text("強度")
                    .step_by(1.0)
                    .suffix("%"),
            )
            .on_hover_text(
                "着色前の輝度分布から黒点・白点を自動検出し、画像ごとの濃さと\n\
                 コントラストを揃えます。0%では補正しません。\n\
                 「モノクロ系画像だけに適用」がONなら、カラー画像には影響しません。",
            );
        if response.changed() {
            params.colorize.density_normalization_strength = density_strength as u8;
            changed = true;
        }
        dragging |= response.dragged();

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("カラー化プリセット")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        for palette in [
            ColorizePalette::Legacy4Color,
            ColorizePalette::LegacySkin,
            ColorizePalette::Custom,
        ] {
            if ui
                .radio_value(&mut params.colorize.palette, palette, palette.label())
                .changed()
            {
                changed = true;
            }
        }

        ui.add_space(8.0);
        draw_colorize_gradient_preview(ui, &params.colorize);

        if params.colorize.palette == ColorizePalette::Custom {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("制御点（暗部 → 明部）")
                    .size(SECTION_FONT)
                    .color(ui.visuals().text_color()),
            )
            .on_hover_text("各色の「強さ」が、その色の担当範囲と隣接色への補間カーブを決めます。");
            let mut remove_index = None;
            let mut move_action = None;
            let point_count = params.colorize.control_points.len();
            for index in 0..point_count {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", index + 1));
                    let point = &mut params.colorize.control_points[index];
                    if ui.color_edit_button_srgb(&mut point.color).changed() {
                        changed = true;
                    }
                    ui.label("強さ");
                    let response = ui.add(
                        egui::DragValue::new(&mut point.strength)
                            .range(0.0..=10.0)
                            .speed(0.05),
                    );
                    if response.changed() {
                        changed = true;
                    }
                    dragging |= response.dragged();
                    if ui
                        .add_enabled(index > 0, egui::Button::new("↑").small())
                        .clicked()
                    {
                        move_action = Some((index, index - 1));
                    }
                    if ui
                        .add_enabled(index + 1 < point_count, egui::Button::new("↓").small())
                        .clicked()
                    {
                        move_action = Some((index, index + 1));
                    }
                    if ui
                        .add_enabled(point_count > 2, egui::Button::new("×").small())
                        .on_hover_text("この制御点を削除")
                        .clicked()
                    {
                        remove_index = Some(index);
                    }
                });
            }
            if let Some((from, to)) = move_action {
                params.colorize.control_points.swap(from, to);
                changed = true;
            } else if let Some(index) = remove_index {
                params.colorize.control_points.remove(index);
                changed = true;
            }
            if ui
                .add_enabled(
                    params.colorize.control_points.len() < 10,
                    egui::Button::new("＋ 制御点を追加").small(),
                )
                .clicked()
            {
                let color = params
                    .colorize
                    .control_points
                    .last()
                    .map(|point| point.color)
                    .unwrap_or([255, 255, 255]);
                params
                    .colorize
                    .control_points
                    .push(ColorizeControlPoint::new(color, 1.0));
                changed = true;
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("元画像の明るさを保持");
            if params.colorize.luminance_weight != 100
                && ui
                    .small_button("↩")
                    .on_hover_text("デフォルトの 100% に戻す")
                    .clicked()
            {
                params.colorize.luminance_weight = 100;
                changed = true;
            }
        });
        let mut luminance_weight = params.colorize.luminance_weight as f32;
        let response = ui.add(
            egui::Slider::new(&mut luminance_weight, 0.0..=100.0)
                .step_by(1.0)
                .suffix("%"),
        );
        if response.changed() {
            params.colorize.luminance_weight = luminance_weight as u8;
            changed = true;
        }
        dragging |= response.dragged();

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("スクリーントーン濃淡変換")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        for method in ToneDensityMethod::ALL {
            if ui
                .radio_value(&mut params.colorize.tone_method, *method, method.label())
                .on_hover_text(method.description())
                .changed()
            {
                changed = true;
            }
        }
        ui.label(
            egui::RichText::new(params.colorize.tone_method.description())
                .size(SECTION_FONT - 1.0)
                .weak(),
        );
        if params.colorize.tone_method != ToneDensityMethod::Off {
            ui.horizontal(|ui| {
                ui.label("検出スケール（長辺比）");
                if (params.colorize.tone_radius - 1.0).abs() > 0.001
                    && ui
                        .small_button("↩")
                        .on_hover_text("デフォルトに戻す")
                        .clicked()
                {
                    params.colorize.tone_radius = 1.0;
                    changed = true;
                }
            });
            let response = ui
                .add(
                    egui::Slider::new(&mut params.colorize.tone_radius, 0.1..=4.0)
                        .step_by(0.1)
                        .fixed_decimals(1),
                )
                .on_hover_text(
                    "長辺 2048px の画像を基準にした値です。\n\
             実際の検出半径は画像の長辺に比例するため、AIアップスケール前後でも\n\
             同じ絵柄上ではほぼ同じ範囲を検出します。",
                );
            if response.changed() {
                changed = true;
            }
            dragging |= response.dragged();

            ui.horizontal(|ui| {
                ui.label("変換の強さ");
                if params.colorize.tone_strength != 100
                    && ui
                        .small_button("↩")
                        .on_hover_text("デフォルトに戻す")
                        .clicked()
                {
                    params.colorize.tone_strength = 100;
                    changed = true;
                }
            });
            let mut strength = params.colorize.tone_strength as f32;
            let response = ui.add(
                egui::Slider::new(&mut strength, 0.0..=100.0)
                    .step_by(1.0)
                    .suffix("%"),
            );
            if response.changed() {
                params.colorize.tone_strength = strength as u8;
                changed = true;
            }
            dragging |= response.dragged();
        }
    });

    // 保存スロットだけはゲートの外に置く。読み込みは `*params = saved` でカラー化の
    // ON/OFF ごと差し替えるので、OFF のときにこそ「保存済みの設定で ON にする」入口に
    // なる。ここを一緒に無効化すると、その入口が消える。
    let slot_result = draw_colorize_preset_slots(ui, &mut params.colorize, slots);
    changed |= slot_result.0;
    settings_changed |= slot_result.1;

    params.colorize.sanitize();
    (changed, dragging, settings_changed)
}

/// スライダー UI (純関数)。ai_denoise_disabled_limit / ai_upscale_disabled_limit が
/// Some なら画像サイズ上限により AI 機能が無効になる旨を表示する。
fn draw_sliders(
    ui: &mut egui::Ui,
    params: &mut AdjustParams,
    settings_tab: &mut crate::settings::AdjustmentSettingsTab,
    colorize_slots: &mut crate::colorize::ColorizePresetSlots,
    creative_luts: &[crate::creative_lut::CreativeLutEntry],
    creative_lut_library: &crate::creative_lut::CreativeLutLibrary,
    image_mipmap_moire_reduction_enabled: &mut bool,
    image_mipmap_lod_bias: &mut f32,
    ai_feature_mode: crate::settings::AiFeatureMode,
    ai_denoise_disabled_limit: Option<crate::ai::upscale::AiProcessSizeLimit>,
    ai_upscale_disabled_limit: Option<crate::ai::upscale::AiProcessSizeLimit>,
) -> (bool, bool, bool) {
    let mut changed = false;
    let mut dragging = false;
    let mut settings_changed = false;
    let is_auto = params.auto_mode.is_some();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for tab in crate::settings::AdjustmentSettingsTab::ALL {
            if ui
                .selectable_label(*settings_tab == *tab, tab.label())
                .clicked()
                && *settings_tab != *tab
            {
                *settings_tab = *tab;
                settings_changed = true;
            }
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);

    if *settings_tab == crate::settings::AdjustmentSettingsTab::ColorTone {
        // ── 補正モード ──
        ui.label(
            egui::RichText::new("補正モード")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        ui.add_space(2.0);
        {
            let mut mode_changed = false;
            if ui
                .radio(
                    params.auto_mode.is_none(),
                    egui::RichText::new("手動").color(ui.visuals().text_color()),
                )
                .clicked()
            {
                params.auto_mode = None;
                mode_changed = true;
            }
            if ui
                .radio(
                    params.auto_mode == Some(AutoMode::Auto),
                    egui::RichText::new("自動補正").color(ui.visuals().text_color()),
                )
                .clicked()
            {
                params.auto_mode = Some(AutoMode::Auto);
                mode_changed = true;
            }
            if ui
                .radio(
                    params.auto_mode == Some(AutoMode::MangaCleanup),
                    egui::RichText::new("モノクロ漫画補正").color(ui.visuals().text_color()),
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
                .color(ui.visuals().text_color()),
        );
        {
            let mut bp = params.black_point as f32;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("黒点")
                        .size(SECTION_FONT)
                        .color(ui.visuals().text_color()),
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
                        .color(ui.visuals().text_color()),
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
                        .color(ui.visuals().text_color()),
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
    }

    if *settings_tab == crate::settings::AdjustmentSettingsTab::Ai {
        ui.label(
            egui::RichText::new("AI ノイズ除去 [N: ON/OFF]")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        if let Some(limit) = ai_denoise_disabled_limit {
            ui.label(
                egui::RichText::new(format!(
                    "（この画像は処理対象サイズ {} 未満の範囲外なので実行されません）",
                    limit.label()
                ))
                .size(SECTION_FONT - 1.0)
                .weak()
                .italics(),
            );
        }
        if !ai_feature_mode.allows_denoise() {
            let note = match ai_feature_mode {
                crate::settings::AiFeatureMode::Disabled => {
                    "（AI機能なしではノイズ除去は実行されません）"
                }
                crate::settings::AiFeatureMode::Light => {
                    "（軽量ではノイズ除去は実行されません。保存済み設定は保持されます）"
                }
                crate::settings::AiFeatureMode::HighQuality => "",
            };
            if !note.is_empty() {
                ui.label(
                    egui::RichText::new(note)
                        .size(SECTION_FONT - 1.0)
                        .weak()
                        .italics(),
                );
            }
        }
        let is_on = params.denoise_model.is_some();
        let mut toggled = is_on;
        if ui
            .add_enabled(
                ai_feature_mode.allows_denoise(),
                egui::Checkbox::new(
                    &mut toggled,
                    egui::RichText::new("JPEG ノイズ除去を適用").color(ui.visuals().text_color()),
                ),
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
                .color(ui.visuals().text_color()),
        );
        if let Some(limit) = ai_upscale_disabled_limit {
            ui.label(
                egui::RichText::new(format!(
                    "（この画像は処理対象サイズ {} 未満の範囲外なので実行されません）",
                    limit.label()
                ))
                .size(SECTION_FONT - 1.0)
                .weak()
                .italics(),
            );
        }
        let upscale_items = crate::adjustment::upscale_menu_items_for_mode(ai_feature_mode);
        let current_upscale_blocked = match params.upscale_model_kind() {
            None => false,
            Some(None) => matches!(ai_feature_mode, crate::settings::AiFeatureMode::Disabled),
            Some(Some(kind)) => !ai_feature_mode.allows_upscale_model(kind),
        };
        if current_upscale_blocked {
            ui.label(
                egui::RichText::new(format!(
                    "（現在の選択「{}」は {} モードでは実行されません）",
                    crate::adjustment::upscale_model_label(params.upscale_model.as_deref()),
                    ai_feature_mode.label()
                ))
                .size(SECTION_FONT - 1.0)
                .weak()
                .italics(),
            );
        }
        for (label, val) in &upscale_items {
            let is_sel = match (val, params.upscale_model.as_deref()) {
                (None, None) => true,
                (Some(a), Some(b)) => *a == b,
                _ => false,
            };
            if ui
                .radio(
                    is_sel,
                    egui::RichText::new(*label).color(ui.visuals().text_color()),
                )
                .clicked()
            {
                params.upscale_model = val.map(|s| s.to_string());
                changed = true;
            }
        }

        // ── シャープ化 (最終表示段スマートシャープ、サムネ非反映) ──
        ui.add_space(12.0);
        {
            let mut strength = params.smart_sharpen as f32;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("シャープ化")
                        .size(SECTION_FONT)
                        .color(ui.visuals().text_color()),
                )
                .on_hover_text(
                    "最終表示に輪郭中心のシャープ化 (スマートシャープ) を適用します。\n\
                 コピー・書き出しにも反映されます。サムネイルには反映されません。\n\
                 AI アップスケールで拡大した画像は既に輪郭が強調されているため、\n\
                 アップスケール実行時は適用されません (ノイズ除去のみの場合や、\n\
                 サイズ上限でアップスケールされなかった画像には適用されます)。",
                );
                if strength != 0.0
                    && ui
                        .small_button("↩")
                        .on_hover_text("デフォルトに戻す")
                        .clicked()
                {
                    strength = 0.0;
                    params.smart_sharpen = 0;
                    changed = true;
                }
            });
            // AI アップスケールが実行される画像では掛からない (固定動作。チェックボックス
            // 案は「強度 0 のとき意味を持たないフラグが個別設定として残る」問題が UX と
            // 両立せず撤回、2026-06-10 ユーザー判断)。サイズ上限の無効表示と同じ形で案内する。
            let upscale_will_run = params.upscale_model.is_some()
                && !current_upscale_blocked
                && ai_upscale_disabled_limit.is_none();
            if upscale_will_run {
                ui.label(
                    egui::RichText::new("（AI アップスケール実行時は適用されません）")
                        .size(SECTION_FONT - 1.0)
                        .weak()
                        .italics(),
                );
            }
            let r = ui.add(egui::Slider::new(&mut strength, 0.0..=100.0).step_by(1.0));
            if r.changed() {
                params.smart_sharpen = strength as u8;
                changed = true;
            }
            if r.dragged() {
                dragging = true;
            }
        }
    }

    if *settings_tab == crate::settings::AdjustmentSettingsTab::Colorize {
        let colorize_result = draw_colorize_settings(ui, params, colorize_slots);
        changed |= colorize_result.0;
        dragging |= colorize_result.1;
        settings_changed |= colorize_result.2;
    }

    if *settings_tab == crate::settings::AdjustmentSettingsTab::PostFilter {
        ui.label(
            egui::RichText::new("Creative LUT")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        ui.label(
            egui::RichText::new("登録した .cube LUT を最終表示・コピー・書き出しに適用します。")
                .size(SECTION_FONT - 1.0)
                .weak(),
        );
        changed |= draw_image_creative_lut_combo(ui, &mut params.creative_lut.id, creative_luts);
        if let Some(id) = params.creative_lut.id {
            ui.horizontal(|ui| {
                ui.label("適用量");
                if (params.creative_lut.strength - 1.0).abs() > 0.001
                    && ui.small_button("↩").on_hover_text("100% に戻す").clicked()
                {
                    params.creative_lut.strength = 1.0;
                    changed = true;
                }
            });
            let response = ui.add(
                egui::Slider::new(&mut params.creative_lut.strength, 0.0..=1.0)
                    .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
                    .custom_parser(|text| {
                        text.trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|value| value / 100.0)
                    }),
            );
            changed |= response.changed();
            dragging |= response.dragged();

            if let Some(error) = creative_lut_library.error(id) {
                ui.label(
                    egui::RichText::new(format!("LUTを読み込めません: {error}"))
                        .size(SECTION_FONT - 1.0)
                        .color(ui.visuals().error_fg_color),
                );
            } else if creative_lut_library.get(Some(id)).is_none() {
                ui.label(
                    egui::RichText::new("LUTを読み込み中…")
                        .size(SECTION_FONT - 1.0)
                        .weak(),
                );
            }
        }
        ui.label(
            egui::RichText::new("追加・削除: 環境設定 > 表示 > LUT")
                .size(SECTION_FONT - 1.0)
                .weak(),
        );
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // ── 縮小表示とポストフィルタ (レトロ系 + 写真系エフェクト) ──
        let moire_toggle = ui.checkbox(
            image_mipmap_moire_reduction_enabled,
            "縮小表示のモアレを抑制する",
        )
        .on_hover_text(
            "ON: 縮小時のモアレやちらつきを抑えます。\nOFF: 線のくっきりさを優先しますが、モアレが出やすくなります。",
        );
        settings_changed |= moire_toggle.changed();
        ui.indent("image_mipmap_moire_reduction_controls", |ui| {
            ui.add_enabled_ui(*image_mipmap_moire_reduction_enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.label("より強く抑制:");
                    if *image_mipmap_lod_bias > 0.001
                        && ui
                            .small_button("↩")
                            .on_hover_text("標準の 0.0 に戻す")
                            .clicked()
                    {
                        *image_mipmap_lod_bias = 0.0;
                        settings_changed = true;
                    }
                });
                let strength_response = ui
                    .add(
                        egui::Slider::new(image_mipmap_lod_bias, 0.0..=1.5)
                            .step_by(0.1)
                            .fixed_decimals(1),
                    )
                    .on_hover_text(
                        "全画像・全ウィンドウ共通。値を上げるほどモアレを強く抑え、\n\
                 通常の写真やイラストは少し柔らかく見えます。\n\
                 目安: 0.0=標準、0.5=抑制、1.0=強い抑制。",
                    );
                if strength_response.dragged() {
                    dragging = true;
                }
                if strength_response.drag_stopped()
                    || (strength_response.changed() && !strength_response.dragged())
                {
                    settings_changed = true;
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label(
            egui::RichText::new("ポストフィルタ [T: 次 / Shift+T: 前 / Alt+T: リセット]")
                .size(SECTION_FONT)
                .color(ui.visuals().text_color()),
        );
        let before_pf = params.post_filter;
        {
            let group_heading = |ui: &mut egui::Ui, text: &str| {
                ui.label(egui::RichText::new(text).size(SECTION_FONT - 1.0).weak());
            };

            group_heading(ui, "── 基本 ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::None,
                PostFilter::None.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Nearest,
                PostFilter::Nearest.display_label(),
            );
            ui.separator();
            group_heading(ui, "── CRT ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::CrtSimple,
                PostFilter::CrtSimple.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::CrtFull,
                PostFilter::CrtFull.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::CrtArcade,
                PostFilter::CrtArcade.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 減色・ディザ (色数昇順) ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Dither1bit,
                PostFilter::Dither1bit.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::GameBoy,
                PostFilter::GameBoy.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Pc98,
                PostFilter::Pc98.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::GameGear,
                PostFilter::GameGear.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Famicom,
                PostFilter::Famicom.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::MegaDrive,
                PostFilter::MegaDrive.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Msx2Plus,
                PostFilter::Msx2Plus.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Sfc,
                PostFilter::Sfc.display_label(),
            );
            ui.separator();
            group_heading(ui, "── CRT × 非液晶機種 ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::ComboFamicomCrt,
                PostFilter::ComboFamicomCrt.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::ComboPc98Crt,
                PostFilter::ComboPc98Crt.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::ComboMsx2PlusCrt,
                PostFilter::ComboMsx2PlusCrt.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::ComboMegaDriveCrt,
                PostFilter::ComboMegaDriveCrt.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::ComboSfcCrt,
                PostFilter::ComboSfcCrt.display_label(),
            );
            ui.separator();
            group_heading(ui, "── カラーグレーディング ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Sepia,
                PostFilter::Sepia.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::MonoNeutral,
                PostFilter::MonoNeutral.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::MonoCool,
                PostFilter::MonoCool.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::MonoWarm,
                PostFilter::MonoWarm.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::WarmTone,
                PostFilter::WarmTone.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::CoolTone,
                PostFilter::CoolTone.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::TealOrange,
                PostFilter::TealOrange.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::KodakPortra,
                PostFilter::KodakPortra.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::FujiVelvia,
                PostFilter::FujiVelvia.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::BleachBypass,
                PostFilter::BleachBypass.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::CrossProcess,
                PostFilter::CrossProcess.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Vintage,
                PostFilter::Vintage.display_label(),
            );
            ui.separator();
            group_heading(ui, "── アナログフィルム ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::FilmGrain,
                PostFilter::FilmGrain.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Vignette,
                PostFilter::Vignette.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::LightLeak,
                PostFilter::LightLeak.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::SoftFocus,
                PostFilter::SoftFocus.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 絵画・描画風 ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Halftone,
                PostFilter::Halftone.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::OilPaint,
                PostFilter::OilPaint.display_label(),
            );
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Sketch,
                PostFilter::Sketch.display_label(),
            );
            ui.separator();
            group_heading(ui, "── 実用 ──");
            ui.radio_value(
                &mut params.post_filter,
                PostFilter::Sharpen,
                PostFilter::Sharpen.display_label(),
            );
        }
        if params.post_filter != before_pf {
            changed = true;
        }
    }

    (changed, dragging, settings_changed)
}

fn draw_image_creative_lut_combo(
    ui: &mut egui::Ui,
    selected_id: &mut Option<uuid::Uuid>,
    creative_luts: &[crate::creative_lut::CreativeLutEntry],
) -> bool {
    let selected_name = selected_id
        .and_then(|id| creative_luts.iter().find(|entry| entry.id == id))
        .map(|entry| entry.name.as_str())
        .unwrap_or_else(|| {
            if selected_id.is_some() {
                "（未登録）"
            } else {
                "なし"
            }
        });
    let before = *selected_id;
    egui::ComboBox::from_id_salt("image_creative_lut")
        .selected_text(selected_name)
        .width(ui.available_width().max(120.0))
        .height(420.0)
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            crate::os_theme::apply_dark_ui(ui);
            ui.selectable_value(selected_id, None, "なし");
            for entry in creative_luts {
                let label = if entry.is_builtin() {
                    format!("プリセット: {}", entry.name)
                } else {
                    entry.name.clone()
                };
                let hover = entry
                    .builtin
                    .map(|builtin| builtin.description().to_owned())
                    .unwrap_or_else(|| entry.path.display().to_string());
                ui.selectable_value(selected_id, Some(entry.id), label)
                    .on_hover_text(hover);
            }
        });
    *selected_id != before
}

#[cfg(test)]
mod creative_lut_ui_tests {
    use super::*;

    #[test]
    fn image_creative_lut_popup_is_dark_and_shows_builtin_presets_in_light_app() {
        use egui_kittest::{Harness, kittest::Queryable};

        let mut fonts_ready = false;
        let mut selected_id = None;
        let creative_luts = crate::creative_lut::builtin_creative_lut_entries();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(380.0, 520.0))
            .build(move |ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Light);
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(18, 18, 18)))
                    .show(ctx, |ui| {
                        crate::os_theme::apply_dark_ui(ui);
                        ui.set_max_width(320.0);
                        ui.label("Creative LUT");
                        draw_image_creative_lut_combo(ui, &mut selected_id, &creative_luts);
                    });
            });

        harness.get_by_role(egui::accesskit::Role::ComboBox).click();
        harness.run();
        assert!(
            harness
                .query_by_label("プリセット: モノクロフィルム")
                .is_some()
        );
        harness.snapshot("image_creative_lut_popup_dark_on_light_app");
        assert_eq!(
            harness.ctx.options(|options| options.theme_preference),
            egui::ThemePreference::Light
        );
        assert!(!harness.ctx.style().visuals.dark_mode);
    }
}

pub(crate) fn book_bookmark_title_edit_widget_id(bookmark_id: i64) -> egui::Id {
    egui::Id::new("book_bookmark_title_edit").with(bookmark_id)
}

fn draw_bookmark_title_edit(
    ui: &mut egui::Ui,
    bookmark_id: i64,
    title: &mut String,
    request_focus: &mut bool,
    enter_pressed: bool,
) -> (egui::Response, bool) {
    let available_width = ui.available_width();
    let response = crate::ime_focus::add_singleline(ui, title, Some(request_focus), |edit| {
        edit.id(book_bookmark_title_edit_widget_id(bookmark_id))
            .desired_width(available_width)
            .hint_text("未設定")
            .return_key(None::<egui::KeyboardShortcut>)
    });
    let submit = response.has_focus() && enter_pressed;
    (response, submit)
}

#[cfg(test)]
pub(crate) fn draw_bookmark_title_edit_for_test(
    ui: &mut egui::Ui,
    bookmark_id: i64,
    title: &mut String,
    request_focus: &mut bool,
) -> (egui::Response, bool) {
    draw_bookmark_title_edit(ui, bookmark_id, title, request_focus, false)
}

fn bookmark_row_should_jump(
    row_contains_pointer: bool,
    primary_clicked: bool,
    control_clicked: bool,
) -> bool {
    row_contains_pointer && primary_clicked && !control_clicked
}

#[cfg(test)]
mod bookmark_panel_input_tests {
    use super::bookmark_row_should_jump;

    #[test]
    fn child_controls_own_bookmark_row_clicks() {
        assert!(bookmark_row_should_jump(true, true, false));
        assert!(!bookmark_row_should_jump(true, true, true));
        assert!(!bookmark_row_should_jump(false, true, false));
        assert!(!bookmark_row_should_jump(true, false, false));
    }
}

#[cfg(test)]
mod bookmark_title_edit_tests {
    use super::draw_bookmark_title_edit;

    fn key_event(key: egui::Key) -> egui::Event {
        key_event_with_modifiers(key, egui::Modifiers::NONE)
    }

    fn key_event_with_modifiers(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn draw_editor(
        ctx: &egui::Context,
        title: &mut String,
        request_focus: &mut bool,
        enter_pressed: bool,
    ) -> (egui::Id, bool) {
        let mut output = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, submit) =
                draw_bookmark_title_edit(ui, 42, title, request_focus, enter_pressed);
            output = Some((response.id, submit));
        });
        output.expect("bookmark title editor response")
    }

    fn ime_commit_and_enter_input() -> egui::RawInput {
        egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("\u{3042}".to_owned())),
                egui::Event::Ime(egui::ImeEvent::Commit("\u{3042}".to_owned())),
                key_event(egui::Key::Enter),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn ime_commit_and_enter_keep_bookmark_title_focus() {
        let ctx = egui::Context::default();
        let mut title = String::new();
        let mut request_focus = true;
        let _ = ctx.run(Default::default(), |ctx| {
            let _ = draw_editor(ctx, &mut title, &mut request_focus, false);
        });

        let mut editor_id = None;
        let _ = ctx.run(ime_commit_and_enter_input(), |ctx| {
            editor_id = Some(draw_editor(ctx, &mut title, &mut request_focus, false).0);
        });

        assert_eq!(title, "\u{3042}");
        assert_eq!(ctx.memory(|memory| memory.focused()), editor_id);
    }

    #[test]
    fn text_after_ime_commit_enter_is_appended_to_bookmark_title() {
        let ctx = egui::Context::default();
        let mut title = String::new();
        let mut request_focus = true;
        let _ = ctx.run(Default::default(), |ctx| {
            let _ = draw_editor(ctx, &mut title, &mut request_focus, false);
        });
        let _ = ctx.run(ime_commit_and_enter_input(), |ctx| {
            let _ = draw_editor(ctx, &mut title, &mut request_focus, false);
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("\u{7d9a}".to_owned())],
                ..Default::default()
            },
            |ctx| {
                let _ = draw_editor(ctx, &mut title, &mut request_focus, false);
            },
        );

        assert_eq!(title, "\u{3042}\u{7d9a}");
    }

    #[test]
    fn plain_enter_submits_bookmark_title_once() {
        let ctx = egui::Context::default();
        let mut title = String::from("title");
        let mut request_focus = true;
        let mut submit_count = 0;
        let _ = ctx.run(Default::default(), |ctx| {
            let _ = draw_editor(ctx, &mut title, &mut request_focus, false);
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Enter)],
                ..Default::default()
            },
            |ctx| {
                if draw_editor(ctx, &mut title, &mut request_focus, true).1 {
                    submit_count += 1;
                }
            },
        );
        let _ = ctx.run(Default::default(), |ctx| {
            if draw_editor(ctx, &mut title, &mut request_focus, false).1 {
                submit_count += 1;
            }
        });

        assert_eq!(submit_count, 1);
    }

    #[test]
    fn clearing_title_keeps_the_same_widget_and_focus_before_plain_text() {
        let ctx = egui::Context::default();
        let mut title = String::from("bookmark");
        let mut request_focus = true;
        let mut ids = Vec::new();
        let _ = ctx.run(Default::default(), |ctx| {
            ids.push(draw_editor(ctx, &mut title, &mut request_focus, false).0);
        });

        let _ = ctx.run(
            egui::RawInput {
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                events: vec![key_event_with_modifiers(
                    egui::Key::A,
                    egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                )],
                ..Default::default()
            },
            |ctx| {
                ids.push(draw_editor(ctx, &mut title, &mut request_focus, false).0);
            },
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Backspace)],
                ..Default::default()
            },
            |ctx| {
                ids.push(draw_editor(ctx, &mut title, &mut request_focus, false).0);
            },
        );

        assert!(title.is_empty());
        assert_eq!(ctx.memory(|memory| memory.focused()), ids.last().copied());

        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::A), egui::Event::Text("a".to_owned())],
                ..Default::default()
            },
            |ctx| {
                ids.push(draw_editor(ctx, &mut title, &mut request_focus, false).0);
            },
        );

        assert_eq!(title, "a");
        assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(ctx.memory(|memory| memory.focused()), ids.last().copied());
    }

    fn editor_id_for_row_order(order: &[i64], target: i64) -> egui::Id {
        let ctx = egui::Context::default();
        let mut title = String::new();
        let mut request_focus = false;
        let mut editor_id = None;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for row_id in order {
                    if *row_id == target {
                        editor_id = Some(
                            draw_bookmark_title_edit(
                                ui,
                                *row_id,
                                &mut title,
                                &mut request_focus,
                                false,
                            )
                            .0
                            .id,
                        );
                    } else {
                        let _ = ui.button(format!("row-{row_id}"));
                    }
                }
            });
        });
        editor_id.expect("target bookmark row")
    }

    #[test]
    fn bookmark_title_widget_id_survives_row_add_delete_and_reorder() {
        let original = editor_id_for_row_order(&[1, 42, 3], 42);
        assert_eq!(original, editor_id_for_row_order(&[9, 1, 42, 3], 42));
        assert_eq!(original, editor_id_for_row_order(&[42, 3], 42));
        assert_eq!(original, editor_id_for_row_order(&[3, 1, 42], 42));
    }
}

impl App {
    /// 補正レイヤーモードへ入る共通の状態遷移。
    ///
    /// 左パネルのボタンと画像フルスクリーンのショートカットは、表示初期状態を
    /// この境界で揃える。終了時の状態が次回起動へ漏れないよう、元画像比較は OFF、
    /// マスク表示は ON から始める。
    pub(crate) fn enter_local_adjust_mode(&mut self) {
        self.adjustment_mode = false;
        self.local_adjust_mode = true;
        self.local_adjust_show_source = false;
        self.local_adjust_show_mask = true;
    }

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
        self.mutate_local_adjust_layer_from_canvas_impl(fs_idx, layer_idx, persist, false, mutate)
    }

    fn mutate_local_adjust_layer_from_canvas_defer_render(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        persist: bool,
        mutate: impl FnOnce(&mut local_adjust_core::LocalAdjustmentLayer) -> bool,
    ) -> bool {
        self.mutate_local_adjust_layer_from_canvas_impl(fs_idx, layer_idx, persist, true, mutate)
    }

    fn mutate_local_adjust_layer_from_canvas_impl(
        &mut self,
        fs_idx: usize,
        layer_idx: usize,
        persist: bool,
        defer_render: bool,
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
        } else if defer_render {
            self.set_local_adjust_layers_for_idx_memory_only_defer_render(fs_idx, layers);
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
                (
                    _,
                    crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart
                    | crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd
                    | crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientCenter
                    | crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius,
                ) => apply_local_adjust_effect_gradient_handle_drag(
                    &mut layer.effect,
                    drag.kind,
                    norm,
                ),
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
        self.mutate_local_adjust_layer_from_canvas_defer_render(
            fs_idx,
            layer_idx,
            persist,
            |layer| match target {
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
                    local_adjust_core::LocalMask::RasterVector(mask) => {
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
            },
        )
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
        self.mutate_local_adjust_layer_from_canvas_defer_render(
            fs_idx,
            layer_idx,
            persist,
            |layer| {
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
            },
        )
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
        self.mutate_local_adjust_layer_from_canvas_defer_render(
            fs_idx,
            layer_idx,
            persist,
            |layer| {
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
            },
        )
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
        ctrl: bool,
        persist: bool,
    ) -> bool {
        // 境界筆 + Ctrl = 境界を無視して通常筆で塗る (tooltip「Ctrl中は境界を表示しながら
        // 通常筆」/ tools/local_adjust_lab の `modifiers.ctrl` 分岐に対応)。tool 自体は
        // EdgeBrush のままにして境界オーバーレイは出し続け、塗りだけ通常筆へ差し替える。
        let tool = if tool == LocalAdjustMaskTool::EdgeBrush && ctrl {
            LocalAdjustMaskTool::Brush
        } else {
            tool
        };
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

    pub(crate) fn commit_local_adjust_polygon_from_shortcut(&mut self, fs_idx: usize) -> bool {
        if self.local_adjust_mask_tool != LocalAdjustMaskTool::Polygon
            || self.local_adjust_mask_lasso_points.len() < 3
        {
            return false;
        }
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            return false;
        };
        let Some(target) = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
            })
        else {
            return false;
        };
        let points = std::mem::take(&mut self.local_adjust_mask_lasso_points);
        self.fill_local_adjust_mask_polygon(fs_idx, layer_idx, target, points)
    }

    pub(crate) fn cancel_local_adjust_canvas_edit_from_shortcut(&mut self) -> bool {
        let shape_drag = self.local_adjust_shape_drag.take();
        let brush_stroke = self.local_adjust_mask_brush_stroke.take();
        let had_edit = self.local_adjust_selected_shape.take().is_some()
            || shape_drag.is_some()
            || self.local_adjust_mask_shape_drag_start.take().is_some()
            || self.local_adjust_mask_shape_drag_end.take().is_some()
            || brush_stroke.is_some()
            || !self.local_adjust_mask_lasso_points.is_empty();
        if let Some(drag) = shape_drag
            && let Some(before) = self.local_adjust_shape_drag_before_layers.take()
        {
            self.set_local_adjust_layers_for_idx_memory_only(drag.fs_idx, before);
        }
        if let Some(stroke) = brush_stroke {
            if let Some(before) = self.local_adjust_mask_brush_before_layers.take() {
                self.set_local_adjust_layers_for_idx_memory_only(stroke.fs_idx, before);
            }
            self.cancel_deferred_local_adjust_brush_render(stroke.fs_idx);
        }
        self.local_adjust_mask_lasso_points.clear();
        if had_edit {
            self.local_adjust_shape_drag_before_layers = None;
            self.local_adjust_mask_brush_before_layers = None;
        }
        had_edit
    }

    fn update_selected_local_adjust_shape(
        &mut self,
        fs_idx: usize,
        undo_summary: &'static str,
        mut update: impl FnMut(local_adjust_core::MaskShape) -> local_adjust_core::MaskShape,
    ) -> bool {
        let Some(selected) = self.local_adjust_selected_shape else {
            return false;
        };
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            return false;
        };
        let Some(target) = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
            })
        else {
            return false;
        };
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let before = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, false, |layer| {
                let Some(mask) =
                    local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, false)
                else {
                    return false;
                };
                let Some(slot) = mask.shapes.get_mut(selected) else {
                    return false;
                };
                *slot = update(*slot);
                true
            });
        if changed {
            self.local_adjust_show_mask = true;
            self.local_adjust_selected_shape = Some(selected);
            let layers = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .cloned()
                .unwrap_or_default();
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before,
                layers,
                undo_summary.to_string(),
            );
        }
        changed
    }

    pub(crate) fn delete_selected_local_adjust_shape_from_shortcut(
        &mut self,
        fs_idx: usize,
    ) -> bool {
        let Some(selected) = self.local_adjust_selected_shape else {
            return false;
        };
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            return false;
        };
        let Some(target) = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| {
                effective_local_mask_edit_target(layer, self.local_adjust_mask_edit_target)
            })
        else {
            return false;
        };
        let image_dims = local_adjust_image_dims(self, fs_idx);
        let before = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .cloned()
            .unwrap_or_default();
        let changed =
            self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, false, |layer| {
                let Some(mask) =
                    local_adjust_target_raster_vector_mask_mut(layer, target, image_dims, false)
                else {
                    return false;
                };
                if selected >= mask.shapes.len() {
                    return false;
                }
                mask.shapes.remove(selected);
                true
            });
        if changed {
            self.local_adjust_show_mask = true;
            self.local_adjust_selected_shape = None;
            self.local_adjust_shape_drag = None;
            let layers = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .cloned()
                .unwrap_or_default();
            self.set_local_adjust_layers_for_idx_with_undo(
                fs_idx,
                before,
                layers,
                "補正レイヤー図形マスク削除".to_string(),
            );
        }
        changed
    }

    pub(crate) fn nudge_selected_local_adjust_shape_from_shortcut(
        &mut self,
        fs_idx: usize,
        dx: f32,
        dy: f32,
    ) -> bool {
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        self.update_selected_local_adjust_shape(
            fs_idx,
            "補正レイヤー図形マスク移動",
            |shape| local_adjust_translate_shape(shape, dx, dy),
        )
    }

    pub(crate) fn rotate_selected_local_adjust_shape_from_shortcut(
        &mut self,
        fs_idx: usize,
        delta_rad: f32,
        snap_15deg: bool,
    ) -> bool {
        if delta_rad == 0.0 {
            return false;
        }
        self.update_selected_local_adjust_shape(
            fs_idx,
            "補正レイヤー図形マスク回転",
            |shape| local_adjust_rotate_shape(shape, delta_rad, snap_15deg),
        )
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
            // 図形マスク確定 = マスク操作 → マスク表示 ON (ラボ mark_mask_changed 相当)。
            self.local_adjust_show_mask = true;
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
        scale: f32,
    ) -> Option<(usize, crate::vector_edit::HoverTarget)> {
        let layer = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))?;
        let mask = local_adjust_target_raster_vector_mask_ref(layer, target)?;
        let point = (point[0], point[1]);
        if let Some(selected) = self.local_adjust_selected_shape
            && let Some(shape) = mask.shapes.get(selected)
        {
            let vector_shape = local_adjust_shape_to_vector_shape(*shape);
            let layout = crate::vector_edit::compute_handle_layout(&vector_shape, scale);
            if let Some(handle) = crate::vector_edit::hit_test(&layout, point, scale)
                && !matches!(handle, crate::vector_edit::HoverTarget::Body)
            {
                return Some((selected, handle));
            }
        }
        for (idx, shape) in mask.shapes.iter().enumerate().rev() {
            let vector_shape = local_adjust_shape_to_vector_shape(*shape);
            let layout = crate::vector_edit::compute_handle_layout(&vector_shape, scale);
            if local_adjust_point_in_vector_body(point, &layout.body_corners) {
                return Some((idx, crate::vector_edit::HoverTarget::Body));
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
            let vector_shape =
                crate::vector_edit::apply_drag(&drag.vector_drag, (point[0], point[1]), &modifiers);
            *slot = vector_shape_to_local_adjust_shape(vector_shape);
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
        // 手描きマスク確定 = マスク操作 → マスク表示 ON (ラボ mark_mask_changed 相当)。
        self.local_adjust_show_mask = true;
        let Some(mut layers) = self.local_adjust_page_layers.get(&stroke.fs_idx).cloned() else {
            self.cancel_deferred_local_adjust_brush_render(stroke.fs_idx);
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
        transform: &DisplayedImageTransform,
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
        if !self.ensure_local_adjust_masks_match_source_dims(fs_idx) {
            return;
        }
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

        let pointer_over_panel = pointer_pos.is_some_and(|pos| {
            self.local_adjust_panel_rect(full_rect).contains(pos)
                || self.local_adjust_tool_panel_rect(full_rect).contains(pos)
        });
        let drawing_in_progress = self.local_adjust_mask_brush_stroke.is_some()
            || self.local_adjust_shape_drag.is_some()
            || self.local_adjust_canvas_drag.is_some()
            || self.local_adjust_mask_shape_drag_start.is_some()
            || !self.local_adjust_mask_lasso_points.is_empty();
        if !drawing_in_progress
            && self.handle_overlay_space_pan_drag(
                ctx,
                self.keymap.key_held_action(ctx, KeyAction::LaSpacePan),
                !pointer_over_panel,
                primary_pressed,
                primary_down,
                primary_released,
                pointer_pos,
            )
        {
            return;
        }

        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            self.local_adjust_canvas_drag = None;
            self.local_adjust_mask_brush_stroke = None;
            return;
        };
        let image_dims = local_adjust_image_dims(self, fs_idx);
        // フルスクリーンビューポートでは `modifiers.ctrl` が stale (Ctrl を離しても true の
        // まま) になり得る。それを OR で混ぜると境界筆が常時「通常筆」化して境界の向こう側まで
        // 塗る回帰になるため、Ctrl 依存の境界筆→通常筆切替は OS 直読みのみを使う
        // (ソースプレビューの Ctrl 検出と同じ方式)。
        let ctrl_held = crate::ui_fullscreen::ctrl_held_via_os();

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
                            && let Some(norm) = local_adjust_screen_to_norm(pos, transform, false)
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
                    && let Some(norm) = local_adjust_screen_to_norm(pos, transform, false)
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
                        ctrl_held,
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
                    && let Some(norm) = if drag.kind
                        == crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius
                    {
                        local_adjust_screen_to_norm_unclamped(pos, transform)
                    } else {
                        local_adjust_screen_to_norm(pos, transform, false)
                    }
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
                            | crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientStart
                            | crate::app::LocalAdjustCanvasDragKind::EffectLinearGradientEnd
                            | crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientCenter
                            | crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius
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
        let Some(norm) = local_adjust_screen_to_norm(pos, transform, true) else {
            if self.local_adjust_effect_position_handles_visible
                && let Some(kind) = self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .and_then(|layer| {
                        local_adjust_effect_gradient_handle_hit_transform(
                            &layer.effect,
                            pos,
                            transform,
                        )
                    })
            {
                if primary_pressed
                    && !self.local_adjust_selective_color_pick_active
                    && self.local_adjust_rgb_pick_active.is_none()
                    && self.local_adjust_repair_point_pick_active.is_none()
                {
                    let drag_norm = if kind
                        == crate::app::LocalAdjustCanvasDragKind::EffectRadialGradientRadius
                    {
                        local_adjust_screen_to_norm_unclamped(pos, transform)
                    } else {
                        local_adjust_screen_to_norm(pos, transform, false)
                    };
                    if let Some(norm) = drag_norm {
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
                        ctx.request_repaint();
                    }
                } else {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }
            }
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
            if let Some(target) = self.local_adjust_repair_point_pick_active {
                if self.mutate_local_adjust_layer_from_canvas(fs_idx, layer_idx, true, |layer| {
                    let local_adjust_core::LocalEffect::Repair(params) = &mut layer.effect else {
                        return false;
                    };
                    match target {
                        crate::local_adjust_effect_ui::RepairPointPickTarget::Source => {
                            params.clone_source_uv = Some(norm);
                        }
                        crate::local_adjust_effect_ui::RepairPointPickTarget::Destination => {
                            params.clone_destination_uv = Some(norm);
                        }
                    }
                    true
                }) {
                    self.show_feedback_toast(format!("{}を指定しました", target.label()));
                }
                self.local_adjust_repair_point_pick_active = None;
                return;
            }
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
                let sample_radius =
                    if target == crate::local_adjust_effect_ui::RgbPickTarget::RepairColor {
                        self.local_adjust_page_layers
                            .get(&fs_idx)
                            .and_then(|layers| layers.get(layer_idx))
                            .and_then(|layer| match &layer.effect {
                                local_adjust_core::LocalEffect::Repair(params) => {
                                    Some(params.sample_radius_px)
                                }
                                _ => None,
                            })
                            .unwrap_or(0.0)
                    } else {
                        0.0
                    };
                if let Some(rgb) =
                    sample_local_adjust_rgb_with_radius(self, fs_idx, norm, sample_radius)
                {
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
                .and_then(|layer| local_adjust_gradient_handle_hit(layer, pos, transform))
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
                        local_adjust_tilt_shift_handle_hit_transform(&layer.effect, pos, transform)
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
                let effect_gradient_handle = self
                    .local_adjust_page_layers
                    .get(&fs_idx)
                    .and_then(|layers| layers.get(layer_idx))
                    .and_then(|layer| {
                        local_adjust_effect_gradient_handle_hit_transform(
                            &layer.effect,
                            pos,
                            transform,
                        )
                    });
                if let Some(kind) = effect_gradient_handle {
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
                    .and_then(|(center, _)| local_adjust_norm_to_screen(center, transform))
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
                let point = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                let shape_hit =
                    local_adjust_image_layout(transform, image_dims).and_then(|(scale, _)| {
                        self.hit_test_local_adjust_mask_shapes(
                            fs_idx,
                            layer_idx,
                            target,
                            [point.0, point.1],
                            scale,
                        )
                    });
                if let Some((shape_idx, handle)) = shape_hit {
                    let should_begin_shape_drag = self.local_adjust_mask_tool
                        == LocalAdjustMaskTool::Select
                        || !matches!(handle, crate::vector_edit::HoverTarget::Body);
                    if should_begin_shape_drag {
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
                            let vector_shape = local_adjust_shape_to_vector_shape(shape);
                            self.local_adjust_shape_drag = Some(LocalAdjustMaskShapeDrag {
                                fs_idx,
                                layer_idx,
                                target,
                                shape_idx,
                                vector_drag: crate::vector_edit::begin_drag(
                                    handle,
                                    shape_idx,
                                    vector_shape,
                                    (point.0, point.1),
                                ),
                            });
                            ctx.set_cursor_icon(crate::vector_edit::cursor_icon_for(
                                handle,
                                &vector_shape,
                            ));
                            ctx.request_repaint();
                            return;
                        }
                    }
                } else if self.local_adjust_mask_tool == LocalAdjustMaskTool::Select {
                    self.local_adjust_selected_shape = None;
                    self.local_adjust_shape_drag = None;
                    ctx.request_repaint();
                    return;
                }
                match self.local_adjust_mask_tool {
                    LocalAdjustMaskTool::Select => {}
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
                            ctrl_held,
                            false,
                        );
                    }
                    LocalAdjustMaskTool::Lasso => {
                        self.local_adjust_mask_lasso_points.clear();
                        let p = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
                        self.local_adjust_mask_lasso_points.push([p.0, p.1]);
                    }
                    LocalAdjustMaskTool::Polygon => {
                        let scale = local_adjust_image_layout(transform, image_dims)
                            .map(|(scale, _)| scale)
                            .unwrap_or(1.0);
                        let source_pixels = self.current_local_adjust_source_pixels(fs_idx);
                        let (point, _, _) = local_adjust_polygon_candidate_point(
                            norm,
                            image_dims,
                            source_pixels.as_deref(),
                            scale,
                            ctrl_held,
                            self.local_adjust_edge_snap_radius,
                            self.local_adjust_boundary_edge_threshold.clamp(0.0, 255.0),
                            self.local_adjust_boundary_ink_threshold.clamp(0.0, 255.0),
                            self.local_adjust_boundary_gap_px.clamp(0.0, 8.0).round() as usize,
                        );
                        let polygon_points: Vec<(f32, f32)> = self
                            .local_adjust_mask_lasso_points
                            .iter()
                            .map(|p| (p[0], p[1]))
                            .collect();
                        let close = self.local_adjust_mask_lasso_points.len() >= 3
                            && crate::manual_mask_tools::should_close_polygon(
                                &polygon_points,
                                (point[0], point[1]),
                                scale,
                            );
                        if close {
                            let points = std::mem::take(&mut self.local_adjust_mask_lasso_points);
                            self.fill_local_adjust_mask_polygon(fs_idx, layer_idx, target, points);
                        } else {
                            self.local_adjust_mask_lasso_points.push(point);
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
        } else if let Some((handle, vector_shape)) = active_mask_edit_target.and_then(|target| {
            if self.local_adjust_mask_tool != LocalAdjustMaskTool::Select {
                return None;
            }
            let point = local_adjust_norm_to_pixel(norm, image_dims.0, image_dims.1);
            let (scale, _) = local_adjust_image_layout(transform, image_dims)?;
            let (shape_idx, handle) = self.hit_test_local_adjust_mask_shapes(
                fs_idx,
                layer_idx,
                target,
                [point.0, point.1],
                scale,
            )?;
            let shape = self
                .local_adjust_page_layers
                .get(&fs_idx)
                .and_then(|layers| layers.get(layer_idx))
                .and_then(|layer| local_adjust_target_raster_vector_mask_ref(layer, target))
                .and_then(|mask| mask.shapes.get(shape_idx))
                .copied()?;
            Some((handle, local_adjust_shape_to_vector_shape(shape)))
        }) {
            ctx.set_cursor_icon(crate::vector_edit::cursor_icon_for(handle, &vector_shape));
        } else if self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .and_then(|layer| local_adjust_gradient_handle_hit(layer, pos, transform))
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
            || self.local_adjust_repair_point_pick_active.is_some()
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
                        local_adjust_tilt_shift_handle_hit_transform(&layer.effect, pos, transform)
                            .is_some();
                    let effect_gradient_hit = local_adjust_effect_gradient_handle_hit_transform(
                        &layer.effect,
                        pos,
                        transform,
                    )
                    .is_some();
                    let center_hit = local_adjust_effect_center(&layer.effect)
                        .and_then(|(center, _)| local_adjust_norm_to_screen(center, transform))
                        .is_some_and(|center| center.distance(pos) <= 14.0);
                    tilt_shift_hit || effect_gradient_hit || center_hit
                })
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    pub(crate) fn draw_local_adjust_canvas_overlay(
        &mut self,
        ui: &mut egui::Ui,
        transform: &DisplayedImageTransform,
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
        if !self.ensure_local_adjust_masks_match_source_dims(fs_idx) {
            return;
        }
        let Some(layer_idx) = self.selected_local_adjust_layer_idx(fs_idx) else {
            return;
        };
        let Some(layer) = self
            .local_adjust_page_layers
            .get(&fs_idx)
            .and_then(|layers| layers.get(layer_idx))
            .cloned()
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
        ) && local_adjust_image_layout(transform, image_dims).is_some()
        {
            draw_local_adjust_mask_preview_overlay(
                painter,
                transform,
                &layer,
                source_pixels.as_deref(),
                image_dims,
                ui.ctx().input(|i| i.time) as f32,
                self.local_adjust_mask_color_preset.colors(),
                effective_local_mask_edit_target(&layer, self.local_adjust_mask_edit_target),
                &mut self.local_adjust_mask_preview_texture,
            );
            if matches!(&layer.mask, local_adjust_core::LocalMask::Segmentation(_)) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(
                        LOCAL_ADJUST_REGION_BOUNDARY_ANIM_INTERVAL_MS,
                    ));
            }
        }
        let shape_layout = local_adjust_image_layout(transform, image_dims);
        let shape_to_screen = shape_layout.map(|_| {
            let w = image_dims.0.max(1) as f32;
            let h = image_dims.1.max(1) as f32;
            move |p: [f32; 2]| -> egui::Pos2 {
                transform.source_normalized_to_screen(egui::pos2(p[0] / w, p[1] / h))
            }
        });
        let active_mask_edit_target =
            effective_local_mask_edit_target(&layer, self.local_adjust_mask_edit_target);
        // 塗り経路 (`ctrl_held`) と同じく OS 直読みのみ。stale な `modifiers.ctrl` を混ぜると
        // Ctrl 非押下でも境界オーバーレイ表示/吸着が出てしまう。
        let ctrl_down = crate::ui_fullscreen::ctrl_held_via_os();
        if ctrl_down
            && active_mask_edit_target.is_some()
            && matches!(
                self.local_adjust_mask_tool,
                LocalAdjustMaskTool::EdgeBrush | LocalAdjustMaskTool::Polygon
            )
            && shape_layout.is_some()
            && let Some(edge_texture) =
                self.ensure_local_adjust_edge_preview_texture(ui.ctx(), fs_idx)
        {
            transform.paint_texture(
                painter,
                edge_texture.id(),
                local_adjust_edge_overlay_color(ui.ctx(), 230),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(
                    LOCAL_ADJUST_EDGE_OVERLAY_REPAINT_MS,
                ));
        }
        if effective_local_mask_edit_target(&layer, self.local_adjust_mask_edit_target).is_some()
            && matches!(
                self.local_adjust_mask_tool,
                LocalAdjustMaskTool::Brush
                    | LocalAdjustMaskTool::EdgeBrush
                    | LocalAdjustMaskTool::GapFillBrush
            )
            && let Some(pointer) = ui.ctx().input(|i| i.pointer.hover_pos())
            && local_adjust_screen_to_norm(pointer, transform, true).is_some()
            && let Some((scale, _)) = local_adjust_image_layout(transform, image_dims)
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
            if let Some(mask) = local_adjust_target_raster_vector_mask_ref(&layer, target) {
                let hovered_shape = if self.local_adjust_mask_tool == LocalAdjustMaskTool::Select
                    || self.local_adjust_selected_shape.is_some()
                {
                    ui.ctx().input(|i| i.pointer.hover_pos()).and_then(|pos| {
                        let (scale, _drawn_rect) = shape_layout?;
                        if !transform.contains_screen(pos) {
                            return None;
                        }
                        let norm = transform.screen_to_source_normalized(pos);
                        let point = (norm.x * image_dims.0 as f32, norm.y * image_dims.1 as f32);
                        let selected = self.local_adjust_selected_shape?;
                        let shape = mask.shapes.get(selected).copied()?;
                        let vector_shape = local_adjust_shape_to_vector_shape(shape);
                        let layout =
                            crate::vector_edit::compute_handle_layout(&vector_shape, scale);
                        crate::vector_edit::hit_test(&layout, point, scale)
                            .map(|target| (selected, target, vector_shape))
                    })
                } else {
                    None
                };
                if let Some((_, target, vector_shape)) = hovered_shape {
                    ui.ctx()
                        .set_cursor_icon(crate::vector_edit::cursor_icon_for(
                            target,
                            &vector_shape,
                        ));
                }
                for (idx, shape) in mask.shapes.iter().copied().enumerate() {
                    let selected = self.local_adjust_selected_shape == Some(idx);
                    let Some((scale, _)) = shape_layout else {
                        continue;
                    };
                    let vector_shape = local_adjust_shape_to_vector_shape(shape);
                    let layout = crate::vector_edit::compute_handle_layout(&vector_shape, scale);
                    let to_vector_screen = |p: (f32, f32)| to_screen([p.0, p.1]);
                    if selected {
                        let hovered = hovered_shape.and_then(|(hover_idx, target, _)| {
                            (hover_idx == idx).then_some(target)
                        });
                        crate::vector_edit::draw_handles(
                            painter,
                            &layout,
                            true,
                            hovered,
                            &to_vector_screen,
                        );
                    } else {
                        crate::vector_edit::draw_shape_outline(
                            painter,
                            &layout,
                            local_adjust_shape_op_to_vector(shape.op()),
                            &to_vector_screen,
                        );
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

            if self.local_adjust_mask_tool == LocalAdjustMaskTool::Polygon
                && let Some(pointer) = ui.ctx().input(|i| i.pointer.hover_pos())
                && let Some((scale, _drawn_rect)) = shape_layout
                && transform.contains_screen(pointer)
                && let Some(norm) = local_adjust_screen_to_norm(pointer, transform, true)
            {
                let (candidate, raw_candidate, snapping) = local_adjust_polygon_candidate_point(
                    norm,
                    image_dims,
                    source_pixels.as_deref(),
                    scale,
                    ctrl_down,
                    self.local_adjust_edge_snap_radius,
                    self.local_adjust_boundary_edge_threshold.clamp(0.0, 255.0),
                    self.local_adjust_boundary_ink_threshold.clamp(0.0, 255.0),
                    self.local_adjust_boundary_gap_px.clamp(0.0, 8.0).round() as usize,
                );
                let candidate_screen = to_screen(candidate);
                let raw_screen = to_screen(raw_candidate);
                let color = if snapping {
                    local_adjust_edge_overlay_color(ui.ctx(), 240)
                } else {
                    egui::Color32::from_rgb(255, 245, 120)
                };
                let guide_stroke = egui::Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_unmultiplied(255, 245, 120, 190),
                );
                if let Some(&last) = self.local_adjust_mask_lasso_points.last() {
                    painter.line_segment([to_screen(last), candidate_screen], guide_stroke);
                }
                if self.local_adjust_mask_lasso_points.len() >= 2 {
                    painter.line_segment(
                        [
                            candidate_screen,
                            to_screen(self.local_adjust_mask_lasso_points[0]),
                        ],
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 105),
                        ),
                    );
                }
                if snapping {
                    painter.circle_stroke(
                        raw_screen,
                        self.local_adjust_edge_snap_radius.max(2.0),
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
                        ),
                    );
                    if candidate_screen.distance(raw_screen) > 1.5 {
                        painter.line_segment(
                            [raw_screen, candidate_screen],
                            egui::Stroke::new(
                                1.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 125),
                            ),
                        );
                    }
                }
                painter.circle_filled(candidate_screen, 5.0, color);
                painter.circle_stroke(
                    candidate_screen,
                    5.0,
                    egui::Stroke::new(1.0, egui::Color32::BLACK),
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(
                        LOCAL_ADJUST_EDGE_OVERLAY_REPAINT_MS,
                    ));
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
        if (self.local_adjust_effect_position_handles_visible
            || matches!(
                &layer.effect,
                local_adjust_core::LocalEffect::Repair(params)
                    if params.mode == local_adjust_core::RepairMode::Clone
            ))
            && local_adjust_image_layout(transform, image_dims).is_some()
        {
            draw_local_adjust_effect_position_overlay(
                painter,
                transform.full_image_rect,
                image_dims,
                &layer.effect,
            );
        }
        match &layer.mask {
            local_adjust_core::LocalMask::LinearGradient(mask) if mask.initialized => {
                let Some(start) = local_adjust_norm_to_screen(mask.start, transform) else {
                    return;
                };
                let Some(end) = local_adjust_norm_to_screen(mask.end, transform) else {
                    return;
                };
                painter.line_segment([start, end], stroke);
                painter.circle_filled(start, 5.0, egui::Color32::from_rgb(255, 238, 145));
                painter.circle_stroke(start, 5.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
                painter.circle_filled(end, 5.0, egui::Color32::from_rgb(80, 210, 255));
                painter.circle_stroke(end, 5.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
            }
            local_adjust_core::LocalMask::RadialGradient(mask) if mask.initialized => {
                let Some(center) = local_adjust_norm_to_screen(mask.center, transform) else {
                    return;
                };
                let Some(inner_x_handle) = local_adjust_norm_to_screen(
                    [mask.center[0] + mask.inner_radius.max(0.0), mask.center[1]],
                    transform,
                ) else {
                    return;
                };
                let Some(inner_y_handle) = local_adjust_norm_to_screen(
                    [
                        mask.center[0],
                        mask.center[1] + mask.inner_radius_y.max(0.0),
                    ],
                    transform,
                ) else {
                    return;
                };
                let Some(outer_x_handle) = local_adjust_norm_to_screen(
                    [
                        mask.center[0] + mask.outer_radius.max(mask.inner_radius),
                        mask.center[1],
                    ],
                    transform,
                ) else {
                    return;
                };
                let Some(outer_y_handle) = local_adjust_norm_to_screen(
                    [
                        mask.center[0],
                        mask.center[1] + mask.outer_radius_y.max(mask.inner_radius_y),
                    ],
                    transform,
                ) else {
                    return;
                };
                draw_local_adjust_ellipse(
                    painter,
                    transform,
                    mask.center,
                    mask.outer_radius,
                    mask.outer_radius_y,
                    stroke,
                );
                if mask.inner_radius > 0.001 || mask.inner_radius_y > 0.001 {
                    draw_local_adjust_ellipse(
                        painter,
                        transform,
                        mask.center,
                        mask.inner_radius,
                        mask.inner_radius_y,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 220, 255)),
                    );
                }
                let Some(outer_x_opposite) = local_adjust_norm_to_screen(
                    [
                        mask.center[0] - mask.outer_radius.max(mask.inner_radius),
                        mask.center[1],
                    ],
                    transform,
                ) else {
                    return;
                };
                painter.line_segment(
                    [outer_x_opposite, outer_x_handle],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 80, 100),
                    ),
                );
                let Some(outer_y_opposite) = local_adjust_norm_to_screen(
                    [
                        mask.center[0],
                        mask.center[1] - mask.outer_radius_y.max(mask.inner_radius_y),
                    ],
                    transform,
                ) else {
                    return;
                };
                painter.line_segment(
                    [outer_y_opposite, outer_y_handle],
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
            self.local_adjust_repair_point_pick_active = None;
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
                    self.local_adjust_repair_point_pick_active = None;
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
                self.local_adjust_repair_point_pick_active = None;
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
            self.local_adjust_repair_point_pick_active = None;
            self.show_feedback_toast("画像上の色をクリックして対象色を選択します".to_string());
        }
        if effect_requests.cancel_rgb_pick {
            self.local_adjust_rgb_pick_active = None;
        }
        if let Some(target) = effect_requests.start_rgb_pick {
            self.local_adjust_rgb_pick_active = Some(target);
            self.local_adjust_selective_color_pick_active = false;
            self.local_adjust_repair_point_pick_active = None;
            self.show_feedback_toast(format!("スポイト対象: {}", target.label()));
        }
        if effect_requests.cancel_repair_point_pick {
            self.local_adjust_repair_point_pick_active = None;
        }
        if let Some(target) = effect_requests.start_repair_point_pick {
            self.local_adjust_repair_point_pick_active = Some(target);
            self.local_adjust_selective_color_pick_active = false;
            self.local_adjust_rgb_pick_active = None;
            self.show_feedback_toast(format!(
                "画像上をクリックして{}を指定します",
                target.label()
            ));
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
        if effect_requests.request_editing_addon_download {
            // 被写体マスク UI からの明示クリックなので、このセッションで辞退済みでも開く。
            self.editing_addon_declined_session = false;
            self.maybe_prompt_editing_addon();
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

    fn local_adjust_edge_preview_source_key(&self, fs_idx: usize) -> String {
        self.page_path_key(fs_idx)
            .or_else(|| self.perf_item_key(fs_idx))
            .unwrap_or_else(|| format!("idx:{fs_idx}"))
    }

    fn ensure_local_adjust_edge_preview_texture(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> Option<egui::TextureHandle> {
        let source = self.current_local_adjust_source_pixels(fs_idx)?;
        let preview_size = local_adjust_edge_preview_size(source.size[0], source.size[1]);
        let edge_threshold = self
            .local_adjust_boundary_edge_threshold
            .clamp(0.0, 255.0)
            .round() as u8;
        let ink_threshold = self
            .local_adjust_boundary_ink_threshold
            .clamp(0.0, 255.0)
            .round() as u8;
        let gap_px = self.local_adjust_boundary_gap_px.clamp(0.0, 8.0).round() as u8;
        let key = LocalAdjustEdgePreviewKey {
            source_key: self.local_adjust_edge_preview_source_key(fs_idx),
            input_gen: self.input_generation.get(&fs_idx).copied().unwrap_or(0),
            erase_mask_gen: self
                .erase_mask_generation
                .get(&fs_idx)
                .copied()
                .unwrap_or(0),
            source_size: source.size,
            preview_size,
            edge_threshold,
            ink_threshold,
            gap_px,
        };
        let rebuild = self
            .local_adjust_edge_preview_cache
            .as_ref()
            .map(|cache| cache.key != key)
            .unwrap_or(true);
        if rebuild {
            let image = build_local_adjust_edge_preview_image(
                source.as_ref(),
                preview_size,
                edge_threshold,
                ink_threshold,
                gap_px,
            );
            let texture = ctx.load_texture(
                format!("local_adjust_edge_preview_{fs_idx}"),
                image,
                crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            self.local_adjust_edge_preview_cache =
                Some(LocalAdjustEdgePreviewCache { key, texture });
        }
        self.local_adjust_edge_preview_cache
            .as_ref()
            .map(|cache| cache.texture.clone())
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
        // 被写体マットモデル (BiRefNet) は編集用追加パックから供給される。
        // App 構造体に毎フレーム I/O を避けてキャッシュしてあるパスを使う。
        let Some(model_path) = self.subject_matte_path.clone() else {
            self.show_feedback_toast(
                "被写体マットモデルが見つかりません (編集用追加ファイル未導入)".to_string(),
            );
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
                let result = run_local_adjust_subject_segmentation(runtime, model_path, source)
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
        crate::os_theme::with_dark_context_style(ctx, || {
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
                    crate::os_theme::apply_dark_ui(ui);
                    ui.label(
                    egui::RichText::new(
                        "使いたいマスク種類を選んでください。クリックするとレイヤーを追加します。",
                    )
                    .size(11.0)
                    .weak(),
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
                                .weak(),
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
        crate::os_theme::with_dark_context_style(ctx, || {
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
                    crate::os_theme::apply_dark_ui(ui);
                    ui.checkbox(&mut keep_manual_override, "追加/削除マスクを維持");
                    ui.label(
                        egui::RichText::new(
                            "加工内容は残したまま、選択中レイヤーのベースマスクだけを変更します。",
                        )
                        .size(11.0)
                        .weak(),
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
        crate::os_theme::with_dark_context_style(ctx, || {
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
                    crate::os_theme::apply_dark_ui(ui);
                    ui.horizontal(|ui| {
                        crate::ime_focus::add_sized_singleline(
                            ui,
                            egui::vec2((ui.available_width() - 32.0).max(120.0), 24.0),
                            effect_query,
                            None,
                            |edit| edit.hint_text("効果名で検索").desired_width(f32::INFINITY),
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
                                        .weak(),
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

        let spread_pair = self.resolve_spread_pair(fs_root_idx);
        let (fs_idx, spread_lr): (usize, Option<(usize, usize)>) = match spread_pair {
            SpreadPair::Double { left, right } => {
                let target = match self.adjust_spread_target {
                    AdjustSpreadTarget::Left => left,
                    AdjustSpreadTarget::Right => right,
                };
                (target, Some((left, right)))
            }
            SpreadPair::Single => (fs_root_idx, None),
        };

        self.maybe_start_local_adjust_render(fs_idx);
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
        let repair_point_pick_active = self.local_adjust_repair_point_pick_active;
        let effect_position_handles_visible = self.local_adjust_effect_position_handles_visible;
        let segmentation_pending = self.local_adjust_segmentation_pending.is_some();
        let active_local_adjust_layers = self.has_active_local_adjust_layers(fs_idx);
        let current_local_adjust_key = self.current_local_adjust_key(fs_idx);
        let local_adjust_render_pending =
            self.local_adjust_pending
                .get(&fs_idx)
                .is_some_and(|pending| {
                    matches!(
                        pending.request.kind,
                        crate::app::EditMaterializeKind::Local { key, .. }
                            if key == current_local_adjust_key
                    )
                });
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
        // 被写体マットモデル (編集用追加パック) の有無。毎フレーム I/O を避けるため
        // App キャッシュ (subject_matte_path) を読むだけ (install/uninstall で更新される)。
        let subject_model_available = self.subject_matte_path.is_some();
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
        let mut edge_snap_radius = self.local_adjust_edge_snap_radius;
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
                        crate::os_theme::apply_dark_ui(ui);

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
                                        .color(ui.visuals().text_color()),
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
                                                .weak(),
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
                        crate::os_theme::apply_dark_ui(ui);
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
                                        self.local_adjust_selected_shape,
                                        &mut update_layer,
                                        effect_clipboard_available,
                                        selective_color_pick_active,
                                        rgb_pick_active,
                                        repair_point_pick_active,
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
                                        &mut edge_snap_radius,
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
                                        .weak(),
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
            self.cache_current_edit_preview_if_ready();
            self.local_adjust_mode = false;
            self.local_adjust_repair_point_pick_active = None;
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
        self.local_adjust_edge_snap_radius = edge_snap_radius.clamp(2.0, 64.0);
        self.local_adjust_edge_brush_tolerance = edge_brush_tolerance.clamp(0.0, 255.0);
        self.local_adjust_edge_brush_include_boundary = edge_brush_include_boundary;
        self.local_adjust_region_color_tolerance = region_color_tolerance.clamp(4.0, 120.0);
        self.local_adjust_region_min_area = region_min_area.clamp(1, 2048);
        // マスク操作で表示 ON / 効果操作で表示 OFF (ラボ tools/local_adjust_lab の
        // reveal_mask_preview / hide_mask_preview 挙動の移植)。マスク操作 = パネルの
        // マスク設定変更 (mask_touched) / 筆ストローク (bitmap_mask_op) / 被写体・領域生成。
        // 効果操作 = 効果パラメータ変更 (effect_touched)。同フレームに両方立つことは
        // 実質ないが、その場合はマスク操作を優先 (= 後に評価して ON を勝たせる)。
        let mask_touched = effect_requests.mask_touched
            || bitmap_mask_op.is_some()
            || effect_requests.generate_subject_mask.is_some()
            || effect_requests.generate_region_mask.is_some();
        if effect_requests.effect_touched {
            self.local_adjust_show_mask = false;
        }
        if mask_touched {
            self.local_adjust_show_mask = true;
        }
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

    fn draw_bookmark_panel_body(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        fs_idx: usize,
        body_width: f32,
        body_height: f32,
    ) {
        if self.current_book_bookmark_draft(fs_idx).is_none() {
            ui.add_space(12.0);
            ui.label("この画像は本のブックマーク対象ではありません。");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "製本、画像のみフォルダ本、ZIP・PDF・対応アーカイブで利用できます。",
                )
                .small()
                .color(egui::Color32::from_gray(170)),
            );
            return;
        }

        self.ensure_current_book_bookmarks_loaded(fs_idx);
        let rows = self.current_book_bookmarks.clone();
        let resolved: Vec<_> = rows
            .iter()
            .map(|bookmark| self.book_bookmark_item_idx(bookmark))
            .collect();
        let thumb_indices: Vec<usize> = resolved.iter().flatten().copied().collect();
        self.ensure_bookmark_panel_thumbnails(&thumb_indices);

        let add_tooltip = self
            .keymap
            .chord_list_bracket_label("現在ページをブックマークに追加", KeyAction::FsBookBookmark);
        let empty_hint = match self.keymap.first_chord_label(KeyAction::FsBookBookmark) {
            Some(shortcut) => {
                format!("{shortcut} キーまたは上の追加ボタンで現在ページを追加できます。")
            }
            None => "上の追加ボタンで現在ページを追加できます。".to_string(),
        };
        let mut add_requested = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("この本のブックマーク").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add_response = ui.add_sized(egui::vec2(24.0, 20.0), egui::Button::new(""));
                crate::ui_fullscreen::draw_icons::draw_bookmark_icon(
                    ui.painter(),
                    add_response.rect.center(),
                    5.8,
                    egui::Color32::from_rgb(255, 220, 82),
                );
                add_requested = add_response.on_hover_text(&add_tooltip).clicked();
                ui.label(format!("{} 件", rows.len()));
            });
        });
        if add_requested {
            self.add_current_book_bookmark(fs_idx);
        }
        ui.separator();
        if self.current_book_bookmarks_request.is_some() && rows.is_empty() {
            ui.add_space(12.0);
            ui.spinner();
            ui.label("読み込み中…");
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        if rows.is_empty() {
            ui.add_space(12.0);
            ui.label("ブックマークはまだありません。");
            ui.label(
                egui::RichText::new(empty_hint)
                    .small()
                    .color(egui::Color32::from_gray(170)),
            );
            return;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let mut remove_id = None;
        let mut jump_to = None;
        let mut title_edit = self.book_bookmark_title_edit.take();
        let mut start_title_edit = None;
        let mut save_title = None;
        let mut cancel_title_edit = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(body_height - 32.0)
            .show(ui, |ui| {
                ui.set_width((body_width - BODY_SCROLLBAR_RESERVE).max(120.0));
                for (bookmark, item_idx) in rows.iter().zip(resolved.iter()) {
                    let is_current = *item_idx == Some(fs_idx);
                    let fill = if is_current {
                        egui::Color32::from_rgba_unmultiplied(55, 105, 165, 150)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(45, 45, 45, 175)
                    };
                    let frame = egui::Frame::new()
                        .fill(fill)
                        .corner_radius(5.0)
                        .inner_margin(egui::Margin::same(6));
                    let inner = frame.show(ui, |ui| {
                        let mut control_clicked = false;
                        ui.horizontal(|ui| {
                            let thumb_size = egui::vec2(58.0, 58.0);
                            let (thumb_rect, _) =
                                ui.allocate_exact_size(thumb_size, egui::Sense::hover());
                            ui.painter()
                                .rect_filled(thumb_rect, 3.0, egui::Color32::from_gray(28));
                            if let Some(idx) = item_idx
                                && let Some(crate::grid_item::ThumbnailState::Loaded {
                                    tex, ..
                                }) = self.thumbnails.get(*idx)
                            {
                                let [tw, th] = tex.size();
                                let scale = (thumb_rect.width() / tw.max(1) as f32)
                                    .min(thumb_rect.height() / th.max(1) as f32);
                                let paint_size = egui::vec2(tw as f32 * scale, th as f32 * scale);
                                let paint_rect =
                                    egui::Rect::from_center_size(thumb_rect.center(), paint_size);
                                ui.painter().image(
                                    tex.id(),
                                    paint_rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            } else {
                                ui.painter().text(
                                    thumb_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    if item_idx.is_some() { "…" } else { "!" },
                                    egui::FontId::proportional(18.0),
                                    egui::Color32::from_gray(165),
                                );
                            }

                            ui.vertical(|ui| {
                                ui.set_max_width((body_width - 86.0).max(100.0));
                                if title_edit.as_ref().map(|edit| edit.id) == Some(bookmark.id) {
                                    // TextEdit 内のクリックを行ジャンプへ流さない。
                                    control_clicked = true;
                                    let edit = title_edit
                                        .as_mut()
                                        .expect("matching book bookmark title edit");
                                    let (_, submit_title) = draw_bookmark_title_edit(
                                        ui,
                                        bookmark.id,
                                        &mut edit.title,
                                        &mut edit.request_focus,
                                        enter_pressed,
                                    );
                                    if submit_title {
                                        save_title = Some((bookmark.id, edit.title.clone()));
                                        control_clicked = true;
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button("保存").clicked() {
                                            save_title = Some((bookmark.id, edit.title.clone()));
                                            control_clicked = true;
                                        }
                                        if ui.small_button("名称なし").clicked() {
                                            save_title = Some((bookmark.id, String::new()));
                                            control_clicked = true;
                                        }
                                        if ui.small_button("キャンセル").clicked() {
                                            cancel_title_edit = true;
                                            control_clicked = true;
                                        }
                                    });
                                } else {
                                    ui.label(
                                        egui::RichText::new(
                                            bookmark.title.as_deref().unwrap_or("名称なし"),
                                        )
                                        .strong(),
                                    );
                                }
                                ui.label(format!(
                                    "{} ページ",
                                    (*item_idx)
                                        .unwrap_or(bookmark.page_index_hint)
                                        .saturating_add(1)
                                ));
                                ui.label(
                                    egui::RichText::new(bookmark.page_identity.display_name())
                                        .small(),
                                );
                                if item_idx.is_none() {
                                    ui.label(
                                        egui::RichText::new("ページが見つかりません")
                                            .small()
                                            .color(egui::Color32::from_rgb(240, 170, 90)),
                                    );
                                }
                                if title_edit.as_ref().map(|edit| edit.id) != Some(bookmark.id) {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("名前を編集").clicked() {
                                            start_title_edit =
                                                Some(crate::app::BookBookmarkTitleEdit {
                                                    id: bookmark.id,
                                                    title: bookmark
                                                        .title
                                                        .clone()
                                                        .unwrap_or_default(),
                                                    request_focus: true,
                                                });
                                            control_clicked = true;
                                        }
                                        if ui.small_button("削除").clicked() {
                                            remove_id = Some(bookmark.id);
                                            control_clicked = true;
                                        }
                                    });
                                }
                            });
                        });
                        control_clicked
                    });
                    // 行全体の click widget を子ボタンの後から重ねると、後勝ちの
                    // interaction が「名前を編集」「削除」の click を奪う。Frame は
                    // hover のままにして、行内の生 click を読み、子 control が処理した
                    // click だけを除外する。
                    let row_primary_clicked = ui.input(|input| input.pointer.primary_clicked());
                    if bookmark_row_should_jump(
                        inner.response.contains_pointer(),
                        row_primary_clicked,
                        inner.inner,
                    ) {
                        jump_to = Some(bookmark.clone());
                    }
                    ui.add_space(5.0);
                }
            });
        if let Some(edit) = start_title_edit {
            self.claim_pending_text_input_focus(
                ctx.viewport_id(),
                book_bookmark_title_edit_widget_id(edit.id),
                ctx.cumulative_pass_nr(),
            );
            title_edit = Some(edit);
        }
        if cancel_title_edit {
            title_edit = None;
            self.clear_pending_text_input_focus();
        }
        if let Some((id, title)) = save_title {
            title_edit = None;
            self.clear_pending_text_input_focus();
            self.set_book_bookmark_title(id, title);
        }
        self.book_bookmark_title_edit = title_edit;
        if let Some(id) = remove_id {
            self.remove_book_bookmark(id);
        } else if let Some(bookmark) = jump_to {
            self.jump_to_current_book_bookmark(ctx, &bookmark);
        }
    }

    fn adjustment_standard_params_for_scope(&self, scope: AdjustmentStandardScope) -> AdjustParams {
        match scope {
            AdjustmentStandardScope::Favorite(id) => self
                .adjustment_favorite_params
                .get(&id)
                .cloned()
                .unwrap_or_else(|| self.settings.global_preset.clone()),
            AdjustmentStandardScope::Global => self.settings.global_preset.clone(),
        }
    }

    /// ドラッグ中のプレビューだけを更新する。DB / settings.save / prune は行わない。
    fn set_adjustment_standard_params_in_memory(
        &mut self,
        scope: AdjustmentStandardScope,
        params: AdjustParams,
    ) {
        match scope {
            AdjustmentStandardScope::Favorite(id) => {
                self.adjustment_favorite_params.insert(id, params);
            }
            AdjustmentStandardScope::Global => self.settings.global_preset = params,
        }
    }

    fn persist_adjustment_standard_params(
        &mut self,
        scope: AdjustmentStandardScope,
        params: AdjustParams,
    ) {
        match scope {
            AdjustmentStandardScope::Favorite(id) => self.set_favorite_default(id, params),
            AdjustmentStandardScope::Global => self.copy_params_to_global(params),
        }
    }

    fn adjustment_standard_label_for_scope(&self, scope: AdjustmentStandardScope) -> String {
        let favorite_name = match scope {
            AdjustmentStandardScope::Favorite(id) => self
                .settings
                .favorite_by_id(id)
                .map(|favorite| favorite.name.as_str()),
            AdjustmentStandardScope::Global => None,
        };
        adjust_scope_standard_label(favorite_name)
    }

    /// 現在ページの実効値を場所の標準へ移し、同値になった個別設定を既存 prune で畳む。
    pub(crate) fn apply_current_params_to_standard(&mut self, fs_idx: usize) {
        let params = self.effective_params(fs_idx).clone();
        let scope = self.adjustment_standard_scope_for_idx(fs_idx);
        let label = self.adjustment_standard_label_for_scope(scope);
        self.capture_adjust_full(format!("現在の設定値を{label}に反映"), |app| {
            app.persist_adjustment_standard_params(scope, params)
        });
        self.adjust_scope_selection = AdjustScopeSelection::Standard;
        self.adjust_scope_selection_idx = Some(fs_idx);
        let remaining = self.page_override_toast_suffix(Some(fs_idx));
        self.show_feedback_toast(format!("{label}に反映{remaining}"));
    }

    pub(crate) fn create_favorite_specific_default(
        &mut self,
        favorite_id: uuid::Uuid,
        favorite_name: &str,
        seed: AdjustParams,
    ) {
        if self.adjustment_favorite_params.contains_key(&favorite_id) {
            return;
        }
        self.capture_adjust_full(
            format!("お気に入り「{favorite_name}」用の標準を作成"),
            |app| app.set_favorite_default(favorite_id, seed),
        );
        self.show_feedback_toast(format!("お気に入り「{favorite_name}」用の標準を作成"));
    }

    fn clear_favorite_specific_default(&mut self, favorite_id: uuid::Uuid, favorite_name: &str) {
        if !self.adjustment_favorite_params.contains_key(&favorite_id) {
            return;
        }
        self.capture_adjust_full(
            format!("お気に入り「{favorite_name}」用の標準を解除"),
            |app| app.clear_favorite_default(favorite_id),
        );
        self.show_feedback_toast(format!("お気に入り「{favorite_name}」用の標準を解除"));
    }

    pub(crate) fn request_favorite_specific_default_clear(
        &mut self,
        favorite_id: uuid::Uuid,
        favorite_name: String,
        fallback: AdjustParams,
    ) {
        let Some(current) = self.adjustment_favorite_params.get(&favorite_id) else {
            return;
        };
        if favorite_default_clear_needs_confirmation(current, &fallback) {
            self.favorite_default_clear_confirm = Some(FavoriteDefaultClearConfirm {
                favorite_id,
                favorite_name,
            });
        } else {
            self.clear_favorite_specific_default(favorite_id, &favorite_name);
        }
    }

    pub(crate) fn draw_favorite_default_clear_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.favorite_default_clear_confirm.clone() else {
            return;
        };

        // IME 判定は Window closure の前に確定する (共通ダイアログ規約)。
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut confirmed = false;
        let mut canceled = false;
        crate::os_theme::with_dark_context_style(ctx, || {
            egui::Window::new("このお気に入りの標準設定が削除されます")
                .order(egui::Order::Debug)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    crate::os_theme::apply_dark_ui(ui);
                    ui.set_min_width(420.0);
                    ui.label(format!(
                        "お気に入り「{}」の標準設定を解除します。",
                        pending.favorite_name
                    ));
                    ui.label("このお気に入りの標準設定が削除されます");
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            confirmed = true;
                        }
                        if ui.button("キャンセル").clicked() {
                            canceled = true;
                        }
                    });
                });
        });

        confirmed |= enter_pressed;
        canceled |= escape_pressed;
        if confirmed {
            self.favorite_default_clear_confirm = None;
            self.clear_favorite_specific_default(pending.favorite_id, &pending.favorite_name);
        } else if canceled {
            self.favorite_default_clear_confirm = None;
        }
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
        let spread_pair = self.resolve_spread_pair(fs_root_idx);
        let (fs_idx, spread_lr): (usize, Option<(usize, usize)>) = match spread_pair {
            SpreadPair::Double { left, right } => {
                let target = match self.adjust_spread_target {
                    AdjustSpreadTarget::Left => left,
                    AdjustSpreadTarget::Right => right,
                };
                (target, Some((left, right)))
            }
            SpreadPair::Single => (fs_root_idx, None),
        };

        // 選択はページごとの一時 UI 状態。ページ移動・見開き L/R 切替で対象 idx が
        // 変わったときだけ、個別行の有無から初期値を導出する。
        if self.adjust_scope_selection_idx != Some(fs_idx) {
            self.adjust_scope_selection =
                initial_adjust_scope_selection(self.adjustment_page_params.contains_key(&fs_idx));
            self.adjust_scope_selection_idx = Some(fs_idx);
        }

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
        crate::os_theme::apply_dark_ui(&mut child);

        // ── ヘッダー ──
        // 左ホバーパネルは 画像補正 / 表示トリム / ブックマーク のタブ式。
        // 画像補正タブだけ、処理順の入口 (消しゴム / 補正レイヤー / 隠蔽加工 /
        // 切り取り / テキスト / エクスポート) を 2 行目に並べる。
        let header_rect =
            egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), HEADER_H));
        const HEADER_BTN_SIZE: f32 = 28.0;
        const HEADER_BTN_GAP: f32 = 4.0;
        const HEADER_RIGHT_PAD: f32 = 8.0;
        const CLOSE_BTN_SIZE: f32 = 24.0;
        let mut selected_tab = self.settings.fullscreen_left_panel_tab;
        let click_to_show = self.settings.fullscreen_side_panel_mode.normalized()
            == crate::settings::FsSidePanelMode::ClickToShow;
        let tab_gap = 4.0;
        let close_reserved = if click_to_show {
            CLOSE_BTN_SIZE + tab_gap
        } else {
            0.0
        };
        let tab_w = ((header_rect.width()
            - BODY_PAD_LEFT
            - BODY_PAD_RIGHT
            - tab_gap * 2.0
            - close_reserved)
            / 3.0)
            .max(62.0);
        let tab_y = header_rect.min.y + 6.0;
        let tab_x = header_rect.min.x + BODY_PAD_LEFT;
        let adjustment_tab_rect =
            egui::Rect::from_min_size(egui::pos2(tab_x, tab_y), egui::vec2(tab_w, TAB_ROW_H));
        let view_trim_tab_rect = egui::Rect::from_min_size(
            egui::pos2(tab_x + tab_w + tab_gap, tab_y),
            egui::vec2(tab_w, TAB_ROW_H),
        );
        let bookmarks_tab_rect = egui::Rect::from_min_size(
            egui::pos2(tab_x + (tab_w + tab_gap) * 2.0, tab_y),
            egui::vec2(tab_w, TAB_ROW_H),
        );
        let close_clicked = click_to_show
            && draw_left_panel_close_button(
                &mut child,
                egui::Rect::from_min_size(
                    egui::pos2(bookmarks_tab_rect.right() + tab_gap, tab_y),
                    egui::vec2(CLOSE_BTN_SIZE, CLOSE_BTN_SIZE),
                ),
            )
            .clicked();
        let adjustment_tab_changed = draw_left_panel_tab_button(
            &mut child,
            adjustment_tab_rect,
            "left_panel_adjustment_tab",
            crate::settings::FullscreenLeftPanelTab::Adjustment,
            &mut selected_tab,
        );
        let view_trim_tab_changed = draw_left_panel_tab_button(
            &mut child,
            view_trim_tab_rect,
            "left_panel_view_trim_tab",
            crate::settings::FullscreenLeftPanelTab::ViewTrim,
            &mut selected_tab,
        );
        let bookmarks_tab_changed = draw_left_panel_tab_button(
            &mut child,
            bookmarks_tab_rect,
            "left_panel_bookmarks_tab",
            crate::settings::FullscreenLeftPanelTab::Bookmarks,
            &mut selected_tab,
        );
        let tab_changed = adjustment_tab_changed || view_trim_tab_changed || bookmarks_tab_changed;
        if tab_changed {
            if self.settings.fullscreen_left_panel_tab
                == crate::settings::FullscreenLeftPanelTab::ViewTrim
                && selected_tab != crate::settings::FullscreenLeftPanelTab::ViewTrim
            {
                self.persist_pending_view_trim_state();
            }
            self.settings.fullscreen_left_panel_tab = selected_tab;
            self.settings.save();
            child.ctx().request_repaint();
        }
        if close_clicked {
            crate::ime_focus::record_side_panel_close(
                child.ctx(),
                "ui_adjustment_panel::draw_adjustment_panel:close_button",
            );
            self.persist_pending_view_trim_state();
            self.adjustment_mode = false;
            child.ctx().request_repaint();
            return;
        }
        if selected_tab == crate::settings::FullscreenLeftPanelTab::Adjustment {
            // 起動可能か (= 画像のみ。動画 / セパレータ / コンテナ は無効化)。
            // `image_dims` が None なら未ロード / 非画像なので無効。
            let can_overlay_edit = image_dims.is_some()
                && !self.fs_entry_is_animated(fs_idx)
                && matches!(
                    self.items.get(fs_idx),
                    Some(
                        crate::grid_item::GridItem::Image(_)
                            | crate::grid_item::GridItem::ZipImage { .. }
                            | crate::grid_item::GridItem::PdfPage { .. }
                    )
                );
            let continuous_reading = self.continuous_reading_active_for_idx(fs_root_idx);
            let edit_tool_disabled_reason = image_edit_tools_disabled_reason(
                self.detached_viewer_image_edit_tools_disabled_reason(),
                continuous_reading,
            );
            let can_start_edit_tool = can_overlay_edit && edit_tool_disabled_reason.is_none();
            let export_disabled_reason = image_export_disabled_reason(continuous_reading);
            let can_export = can_overlay_edit && export_disabled_reason.is_none();
            // 右側 6 ボタン。左から 消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り / テキスト / エクスポート。
            // テキストは comic 注釈モード (最前面・パイプライン最終段) なので crop と export の間に置く。
            let btn_y = header_rect.min.y + 34.0;
            let export_btn_x = header_rect.max.x - HEADER_RIGHT_PAD - HEADER_BTN_SIZE;
            let text_btn_x = export_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
            let crop_btn_x = text_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
            let conceal_btn_x = crop_btn_x - HEADER_BTN_GAP - HEADER_BTN_SIZE;
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
            let crop_rect = egui::Rect::from_min_size(
                egui::pos2(crop_btn_x, btn_y),
                egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
            );
            let text_rect = egui::Rect::from_min_size(
                egui::pos2(text_btn_x, btn_y),
                egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
            );
            let export_rect = egui::Rect::from_min_size(
                egui::pos2(export_btn_x, btn_y),
                egui::vec2(HEADER_BTN_SIZE, HEADER_BTN_SIZE),
            );
            let mut activate_erase = false;
            let mut activate_local_adjust = false;
            let mut activate_conceal = false;
            let mut activate_crop = false;
            let mut activate_text = false;
            let mut activate_export = false;

            let erase_resp = draw_header_icon_button(
                &mut child,
                erase_rect,
                "adjust_panel_erase_btn",
                can_start_edit_tool,
                false,
                "消しゴム (E)",
                edit_tool_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_eraser_icon,
            );
            if can_start_edit_tool && erase_resp.clicked() {
                activate_erase = true;
            }

            let local_adjust_tooltip = self.keymap.first_chord_action_label(
                "補正レイヤー",
                crate::keymap::KeyAction::FsLocalAdjustMode,
            );
            let local_adjust_resp = draw_header_icon_button(
                &mut child,
                local_adjust_rect,
                "adjust_panel_local_adjust_btn",
                can_start_edit_tool,
                false,
                &local_adjust_tooltip,
                edit_tool_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_local_adjust_icon,
            );
            if can_start_edit_tool && local_adjust_resp.clicked() {
                activate_local_adjust = true;
            }

            let conceal_resp = draw_header_icon_button(
                &mut child,
                conceal_rect,
                "adjust_panel_conceal_btn",
                can_start_edit_tool,
                false,
                "隠蔽加工 (Ctrl+M)",
                edit_tool_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_mosaic_icon,
            );
            if can_start_edit_tool && conceal_resp.clicked() {
                activate_conceal = true;
            }

            let crop_resp = draw_header_icon_button(
                &mut child,
                crop_rect,
                "adjust_panel_crop_btn",
                can_start_edit_tool,
                false,
                "切り取り",
                edit_tool_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_crop_icon,
            );
            if can_start_edit_tool && crop_resp.clicked() {
                activate_crop = true;
            }

            let text_resp = draw_header_icon_button(
                &mut child,
                text_rect,
                "adjust_panel_text_btn",
                can_start_edit_tool,
                false,
                "テキスト注釈 (Ctrl+T)",
                edit_tool_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_text_icon,
            );
            if can_start_edit_tool && text_resp.clicked() {
                activate_text = true;
            }

            let export_resp = draw_header_icon_button(
                &mut child,
                export_rect,
                "adjust_panel_export_btn",
                can_export,
                false,
                "エクスポート",
                export_disabled_reason,
                crate::ui_fullscreen::draw_icons::draw_export_icon,
            );
            if can_export && export_resp.clicked() {
                activate_export = true;
            }
            // クリック処理は描画後にディスパッチ (借用衝突回避)。
            // 補正パネルは「ホバーで自動閉じる」モードなので、消しゴム / 隠蔽に入る前に
            // adjustment_mode を倒しておく (enter_*_mode 内のガード `!self.adjustment_mode`
            // と整合させるためにも必要)。`enter_*_mode` 自身が必要なキャッシュ初期化と
            // post_filter バイパスを行うので、ここでは flag を倒すだけで十分。
            if activate_local_adjust {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:enter_local_adjust",
                );
                self.enter_local_adjust_mode();
                return;
            }
            if activate_erase {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:enter_erase",
                );
                self.adjustment_mode = false;
                self.enter_erase_mode(fs_root_idx);
                return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
            }
            if activate_conceal {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:enter_conceal",
                );
                self.adjustment_mode = false;
                self.enter_conceal_mode(fs_root_idx);
                return;
            }
            if activate_crop {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:enter_crop",
                );
                self.adjustment_mode = false;
                self.enter_export_crop_mode(fs_root_idx);
                return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
            }
            if activate_text {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:enter_text",
                );
                self.adjustment_mode = false;
                self.enter_text_mode(fs_root_idx);
                return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
            }
            if activate_export {
                crate::ime_focus::record_side_panel_close(
                    child.ctx(),
                    "ui_adjustment_panel::draw_adjustment_panel:open_export",
                );
                self.adjustment_mode = false;
                let ctx = child.ctx().clone();
                self.open_export_dialog_for_current(&ctx, fs_idx);
                return; // 同フレーム内でモード分岐が変わるため以降の描画はスキップ
            }
        }

        // ── R5: パネル body 全体を 1 つの ScrollArea で囲む ──
        // 旧版は spread セレクタ / scope text / action buttons / 保存スロットを
        // **絶対位置**で配置し、中央スライダー領域だけが ScrollArea になっていた。
        // そのため「ウィンドウ縦幅が狭いと action buttons / 保存スロットが下端に
        // 沈んで触れない」「補正パネルだけ全体スクロールが効かない」状態だった
        // (実機 FB R5: 「画像補正パネルはまだ中央部分だけスクロールします」)。
        //
        // 新方針: ヘッダ (HEADER_H = 64px) は絶対位置で固定し、それより下を 1 つの
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

        if selected_tab == crate::settings::FullscreenLeftPanelTab::Bookmarks {
            let ctx = child.ctx().clone();
            body_child.allocate_ui_with_layout(
                egui::vec2(body_width, body_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_width(content_width);
                    self.draw_bookmark_panel_body(
                        ui,
                        &ctx,
                        fs_root_idx,
                        content_width,
                        body_height,
                    );
                },
            );
            return;
        }

        if selected_tab == crate::settings::FullscreenLeftPanelTab::ViewTrim {
            let ctx = child.ctx().clone();
            body_child.allocate_ui_with_layout(
                egui::vec2(body_width, body_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_width(content_width);
                    self.draw_view_trim_controls(
                        ui,
                        &ctx,
                        fs_root_idx,
                        spread_pair,
                        content_width,
                        body_height,
                    );
                },
            );
            return;
        }

        let mut copy_global_to_favorite_clicked = false;
        let mut apply_current_to_standard_clicked = false;
        let mut align_all_clicked = false;
        let mut clear_page_clicked = false;
        let mut favorite_default_toggle: Option<bool> = None;
        let mut save_to_slot: Option<usize> = None;
        let mut load_from_slot: Option<usize> = None;
        let has_override = self.adjustment_page_params.contains_key(&fs_idx);
        let scope_selection_before =
            effective_adjust_scope_selection(has_override, self.adjust_scope_selection);
        let mut requested_scope_selection = scope_selection_before;
        let mut scope_selection_clicked: Option<AdjustScopeSelection> = None;

        // 編集対象ページを含むお気に入り (なければ None)。
        let fav_info = self
            .current_favorite_id_for_idx(fs_idx)
            .and_then(|id| self.settings.favorite_by_id(id))
            .map(|f| (f.id, f.name.clone()));
        let has_favorite_default = fav_info
            .as_ref()
            .map(|(id, _)| self.adjustment_favorite_params.contains_key(id))
            .unwrap_or(false);

        // ラジオ上段は「現在地の最寄りお気に入り」ではなく、実際に書き込む
        // 最近祖先の ON 標準を表示する。OFF のお気に入り配下なら共通になる。
        let standard_scope = self.adjustment_standard_scope_for_idx(fs_idx);
        let standard_scope_label = self.adjustment_standard_label_for_scope(standard_scope);

        // 現在の有効パラメータを取得して編集用コピーを作る
        let mut edit_params = match scope_selection_before {
            AdjustScopeSelection::Standard => {
                self.adjustment_standard_params_for_scope(standard_scope)
            }
            AdjustScopeSelection::Page => self.effective_params(fs_idx).clone(),
        };
        let original = edit_params.clone();

        // サイズ上限以上ならスキップされる → その場合は「無効」を UI に反映する
        let ai_denoise_disabled_limit = match image_dims {
            Some((w, h))
                if !crate::ai::upscale::should_process_rect(
                    w,
                    h,
                    self.settings.ai_denoise_limit(),
                ) =>
            {
                Some(self.settings.ai_denoise_limit())
            }
            _ => None,
        };
        let ai_upscale_disabled_limit = match image_dims {
            Some((w, h))
                if !crate::ai::upscale::should_process_rect(
                    w,
                    h,
                    self.settings.ai_upscale_limit(),
                ) =>
            {
                Some(self.settings.ai_upscale_limit())
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

                        // ── 書き込みスコープ ──
                        ui.add_space(2.0);
                        let standard_radio = ui.radio_value(
                            &mut requested_scope_selection,
                            AdjustScopeSelection::Standard,
                            &standard_scope_label,
                        );
                        if standard_radio.clicked() {
                            scope_selection_clicked = Some(AdjustScopeSelection::Standard);
                        }
                        if let Some((_, favorite_name)) = fav_info.as_ref() {
                            let mut enabled = has_favorite_default;
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                let response = ui.add(egui::Checkbox::new(
                                    &mut enabled,
                                    egui::RichText::new(
                                        "このお気に入り用に標準を分ける",
                                    )
                                    .size(12.0),
                                ));
                                if response.changed() {
                                    favorite_default_toggle = Some(enabled);
                                }
                                response.on_hover_text(format!(
                                    "お気に入り「{favorite_name}」に、このお気に入り専用の標準設定を持たせます"
                                ));
                            });
                        }
                        let page_radio = ui.radio_value(
                            &mut requested_scope_selection,
                            AdjustScopeSelection::Page,
                            "このページ",
                        );
                        if page_radio.clicked() {
                            scope_selection_clicked = Some(AdjustScopeSelection::Page);
                        }
                        ui.add_space(4.0);

                        // ── アクションボタン (選択中スコープに応じた操作行) ──
                        let wide = egui::vec2(content_width, 24.0);
                        if matches!(standard_scope, AdjustmentStandardScope::Favorite(_))
                            && ui
                                .add(
                                    egui::Button::new("共通の標準からコピー").min_size(wide),
                                )
                                .on_hover_text(
                                    "共通の標準の内容を、この場所の標準へ複製します",
                                )
                                .clicked()
                        {
                            copy_global_to_favorite_clicked = true;
                        }

                        if scope_selection_before == AdjustScopeSelection::Page
                            && ui
                                .add(
                                    egui::Button::new("現在の設定値を標準に反映")
                                        .min_size(wide),
                                )
                                .on_hover_text(
                                    "現在のページに効いている補正値を、この場所の標準として保存します。\
                                     このページの個別設定は標準と同じ内容になるため解除されます",
                                )
                                .clicked()
                        {
                            apply_current_to_standard_clicked = true;
                        }

                        let (align_all_label, align_all_tooltip) = match scope_selection_before {
                            AdjustScopeSelection::Standard => (
                                ADJUST_ALIGN_ALL_STANDARD_LABEL,
                                format!(
                                    "このフォルダ/ZIP/PDF の全画像から個別設定を削除し、{standard_scope_label}に揃えます"
                                ),
                            ),
                            AdjustScopeSelection::Page => (
                                ADJUST_ALIGN_ALL_PAGE_LABEL,
                                "このフォルダ/ZIP/PDF の全画像に、現在のページの実効値を個別設定として書き込みます。\
                                 個別設定は標準より優先されるので、以後この一覧は標準設定の変更を受けなくなる"
                                    .to_string(),
                            ),
                        };
                        if ui
                            .add(egui::Button::new(align_all_label).min_size(wide))
                            .on_hover_text(align_all_tooltip)
                            .clicked()
                        {
                            align_all_clicked = true;
                        }

                        if scope_selection_before == AdjustScopeSelection::Page {
                            let response = ui
                                .add_enabled(
                                    has_override,
                                    egui::Button::new("個別設定を解除 [Q]").min_size(wide),
                                )
                                .on_hover_text(
                                    "このページの個別設定を削除し、標準値に戻す (Q または Ctrl+Backspace)",
                                );
                            if response.clicked() {
                                clear_page_clicked = true;
                            }
                        }
                        ui.add_space(6.0);

                        // ── スライダー群 ──
                        let mut slider_result = draw_sliders(
                            ui,
                            &mut edit_params,
                            &mut self.settings.adjustment_settings_tab,
                            &mut self.settings.colorize_preset_slots,
                            &self.settings.creative_luts,
                            &self.creative_lut_library,
                            &mut self.settings.image_mipmap_moire_reduction_enabled,
                            &mut self.settings.image_mipmap_lod_bias,
                            self.settings.ai_feature_mode,
                            ai_denoise_disabled_limit,
                            ai_upscale_disabled_limit,
                        );

                        // ── 全タブ共通操作 ──
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        let reset_tooltip = match scope_selection_before {
                            AdjustScopeSelection::Standard => format!(
                                "{standard_scope_label}のすべての補正値を初期値に戻します"
                            ),
                            AdjustScopeSelection::Page =>
                                "このページのすべての補正値を初期値に戻します（標準に従わせるのではなく、無補正で固定します）"
                                    .to_string(),
                        };
                        if ui
                            .button("すべてリセット")
                            .on_hover_text(reset_tooltip)
                            .clicked()
                        {
                            edit_params = AdjustParams::default();
                            slider_result.0 = true;
                        }

                        // ── 保存スロット (5x2 grid) ──
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("画像補正保存スロット")
                                .size(11.0)
                                .color(ui.visuals().text_color()),
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
                                            "{} をこのページに適用 (Ctrl+{})\n標準設定へ読み込む: Ctrl+Alt+{}",
                                            s.name, key_label, key_label
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
        let (changed, is_dragging, settings_changed) = scroll_output.inner;
        let was_dragging = self.adjustment_dragging;
        // モアレ抑制の強さ等の settings_changed は drag_stopped の release フレームでだけ立つ。
        // `was_dragging` まで除外するとその最終値を失うため、現在ドラッグ中かだけを見る。
        if should_save_settings(settings_changed, is_dragging) {
            self.settings.save();
        }

        // 「このページ」→「標準」は、選択変更そのものが個別解除操作になる。
        if requested_scope_selection != scope_selection_before {
            if scope_selection_before == AdjustScopeSelection::Page
                && requested_scope_selection == AdjustScopeSelection::Standard
                && self.adjustment_page_params.contains_key(&fs_idx)
            {
                self.capture_adjust_full("個別設定の解除".to_string(), |app| {
                    app.clear_page_params(fs_idx)
                });
                self.show_feedback_toast("個別設定を解除".to_string());
            }
            self.adjust_scope_selection = requested_scope_selection;
        } else if let Some(clicked) = scope_selection_clicked {
            // 個別行に強制されて表示上は既に Page の場合でも、明示クリックは保持値へ反映する。
            self.adjust_scope_selection = clicked;
        }
        let effective_scope_selection_after = effective_adjust_scope_selection(
            self.adjustment_page_params.contains_key(&fs_idx),
            self.adjust_scope_selection,
        );

        if let (Some(enabled), Some((favorite_id, favorite_name))) =
            (favorite_default_toggle, fav_info.as_ref())
        {
            if enabled {
                // OFF→ON は現在見えている標準を種にする。global 固定ではなく
                // effective_default を使うことで、入れ子の外側標準が ON の場合も
                // 見た目を保ったまま内側を独立させられる。
                let seed = self.effective_default_for_idx(fs_idx).clone();
                self.create_favorite_specific_default(*favorite_id, favorite_name, seed);
            } else {
                let fallback = self
                    .favorite_default_fallback_after_clear_for_idx(fs_idx, *favorite_id)
                    .clone();
                self.request_favorite_specific_default_clear(
                    *favorite_id,
                    favorite_name.clone(),
                    fallback,
                );
            }
        }

        // ドラッグセッションのライフサイクル管理 (slider drag → release で 1 回だけ commit)
        self.adjustment_dragging = is_dragging;
        let drag_just_started = is_dragging && !was_dragging;
        if drag_just_started {
            match effective_scope_selection_after {
                AdjustScopeSelection::Page => {
                    self.adjustment_drag_session = Some(crate::app::AdjustmentDragSession {
                        fs_idx,
                        before: self.adjustment_page_params.get(&fs_idx).cloned(),
                    });
                    self.adjustment_standard_drag_session = None;
                }
                AdjustScopeSelection::Standard => {
                    let scope = self.adjustment_standard_scope_for_idx(fs_idx);
                    self.adjustment_standard_drag_session = Some(AdjustmentStandardDragSession {
                        scope,
                        before: self.adjustment_standard_params_for_scope(scope),
                    });
                    self.adjustment_drag_session = None;
                }
            }
        }
        // セッションが存在するが fs_idx がズレている (= ページ移動した) 場合は破棄。
        // 通常は open_fullscreen での clear_meta_undo が落とすが念のため。
        if let Some(s) = &self.adjustment_drag_session {
            if s.fs_idx != fs_idx {
                self.adjustment_drag_session = None;
            }
        }

        // ── スライダー変更を選択中スコープへ反映 ──
        // ドラッグ中はページ / 標準とも in-memory のみ。release で 1 回だけ永続化する。
        // 非ドラッグのラジオ・コンボボックス・リセットは即時通常パスへ流す。
        if changed {
            if is_dragging || was_dragging {
                if let Some(session) = self.adjustment_standard_drag_session.as_ref() {
                    // scope はドラッグ開始時に固定し、途中の UI 状態変化に追従させない。
                    let scope = session.scope;
                    self.set_adjustment_standard_params_in_memory(scope, edit_params.clone());
                } else {
                    self.adjustment_page_params
                        .insert(fs_idx, edit_params.clone());
                }
            } else {
                match effective_scope_selection_after {
                    AdjustScopeSelection::Page => {
                        let before = self.adjustment_page_params.get(&fs_idx).cloned();
                        self.set_page_params(fs_idx, edit_params.clone());
                        let after = self.adjustment_page_params.get(&fs_idx).cloned();
                        self.capture_adjustment_undo(
                            crate::undo_stack::AdjustUndoScope::Page(fs_idx),
                            before,
                            after,
                            "ページ個別の補正".to_string(),
                        );
                    }
                    AdjustScopeSelection::Standard => {
                        let scope = self.adjustment_standard_scope_for_idx(fs_idx);
                        let label = self.adjustment_standard_label_for_scope(scope);
                        let params = edit_params.clone();
                        self.capture_adjust_full(format!("{label}の補正"), |app| {
                            app.persist_adjustment_standard_params(scope, params)
                        });
                        let remaining = self.page_override_toast_suffix(None);
                        self.show_feedback_toast(format!("{label}を更新{remaining}"));
                    }
                }
            }
            // 差分内容で clear を振り分け (AI 変更 / シャープ化のみ / 色調・post_filter)。
            self.clear_caches_for_param_change(fs_idx, &original, &edit_params);
            // ドラッグ中に色調が動いたら、release 時のサムネ補正テクスチャ全クリアを
            // 予約する (シャープ化だけのドラッグではサムネを無駄に再生成しない)。
            if (is_dragging || was_dragging) && !original.color_settings_eq(&edit_params) {
                self.thumb_adjust_drag_color_dirty = true;
            }
        }

        // ドラッグ終了 (release) フレーム: changed が立たないことが多いので別経路で確定。
        let drag_just_ended = !is_dragging && was_dragging;
        if drag_just_ended {
            if let Some(session) = self.adjustment_standard_drag_session.take() {
                let after = self.adjustment_standard_params_for_scope(session.scope);
                if session.before != after {
                    // capture_adjust_full が正しい before を取れるよう、プレビューだけに
                    // 書いた値を一瞬戻してから通常の永続化 API へ最終値を渡す。
                    self.set_adjustment_standard_params_in_memory(session.scope, session.before);
                    let label = self.adjustment_standard_label_for_scope(session.scope);
                    self.capture_adjust_full(format!("{label}の補正"), |app| {
                        app.persist_adjustment_standard_params(session.scope, after)
                    });
                    let remaining = self.page_override_toast_suffix(None);
                    self.show_feedback_toast(format!("{label}を更新{remaining}"));
                }
            } else if let Some(session) = self.adjustment_drag_session.take() {
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
        if copy_global_to_favorite_clicked {
            self.copy_global_default_to_current_favorite();
        }
        if apply_current_to_standard_clicked {
            self.apply_current_params_to_standard(fs_idx);
        }
        if align_all_clicked {
            match effective_scope_selection_after {
                AdjustScopeSelection::Standard => {
                    self.capture_adjust_full("全画像を標準に揃える".to_string(), |app| {
                        app.clear_all_page_params();
                    });
                    self.show_feedback_toast("全画像を標準に揃える".to_string());
                }
                AdjustScopeSelection::Page => {
                    let params = self.effective_params(fs_idx).clone();
                    self.capture_adjust_full(
                        "全画像をこのページに揃える".to_string(),
                        |app| {
                            app.apply_params_to_all_pages(params);
                        },
                    );
                    self.show_feedback_toast("全画像をこのページに揃える".to_string());
                }
            }
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
            crate::logger::log(format!(
                "[adjust slot] capture slot={} idx={} scope={} {}",
                crate::adjustment::slot_key_label(slot_idx),
                fs_idx,
                self.resolve_adjust_scope(fs_idx).label(),
                crate::adjustment::color_log_summary(&params),
            ));
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

        egui::Window::new("画像補正保存スロット名")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let key_label = crate::adjustment::slot_key_label(slot_idx);
                ui.label(format!("スロット {} に保存する名前を入力:", key_label));
                ui.add_space(4.0);
                let resp = crate::ime_focus::add_singleline(ui, &mut name_input, None, |edit| {
                    edit.desired_width(240.0)
                        .hint_text("例: 漫画モノクロ / スキャン補正")
                });
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
            crate::logger::log(format!(
                "[adjust slot] save slot={} name=\"{}\" {}",
                crate::adjustment::slot_key_label(slot_idx),
                name_input.trim(),
                crate::adjustment::color_log_summary(&params),
            ));
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
